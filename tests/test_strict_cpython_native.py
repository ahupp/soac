"""Raw CPython contract boundaries; ctypes here is a trusted native test fixture.

Production authority comes from the authenticated Rust loader, never from a
Python wrapper around these APIs. Run this suite with the selected CPython.
"""

import ctypes
import gc
import json
import marshal
import opcode
import os
from pathlib import Path
import shlex
import subprocess
import sys
import sysconfig
import tempfile
import types
import unittest
import weakref


STRICT = 0x10000000

_READ_STARTUP_CONFIG = r"""
import ctypes, json
api = ctypes.pythonapi.PySoac_GetStrictConfig
api.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(ctypes.c_ssize_t), ctypes.POINTER(ctypes.c_wchar_p)]
api.restype = ctypes.c_int
data, size, path = ctypes.c_void_p(), ctypes.c_ssize_t(), ctypes.c_wchar_p()
assert api(ctypes.byref(data), ctypes.byref(size), ctypes.byref(path)) == 1
print(json.dumps([ctypes.string_at(data, size.value).decode(), path.value]))
"""


def native_api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.restype = result
    function.argtypes = list(arguments)
    return function


def borrowed_object_api(name):
    # A py_object restype assumes a new reference. These getters deliberately
    # return interpreter-owned borrowed references, so convert the pointer.
    address = native_api(name, ctypes.c_void_p)()
    return ctypes.cast(address, ctypes.py_object).value


class NativeHeaderCompatibilityTests(unittest.TestCase):
    def test_public_python_header_compiles_as_cplusplus(self):
        source = Path(sysconfig.get_config_var("abs_srcdir"))
        build = Path(sysconfig.get_config_var("abs_builddir"))
        self.assertEqual(
            (build / "python").resolve(), Path(sys._base_executable).resolve()
        )
        self.assertTrue((source / "Include/Python.h").is_file())
        self.assertTrue((build / "pyconfig.h").is_file())
        compiler = shlex.split(sysconfig.get_config_var("CXX"))
        self.assertTrue(
            compiler, "the selected native toolchain must provide a C++ compiler"
        )
        result = subprocess.run(
            [
                *compiler,
                "-fsyntax-only",
                "-x",
                "c++",
                f"-I{source / 'Include'}",
                f"-I{build}",
                "-",
            ],
            input=(
                "#include <Python.h>\n"
                "static_assert(Py_SOAC_TYPE_CONTRACT_ABI == 4);\n"
                "static auto info = &PyType_GetSoacConstructionInfoV1;\n"
                "static auto admit = &PyType_AdmitSoacPendingV1;\n"
                "static auto fail = &PyType_FailSoacPendingV1;\n"
                "static auto dispose = &PyType_DisposeSoacProvisionalV1;\n"
                "static int final_commit(PyObject*, PyObject*, const PySoacTypeContractSpecV4*) { return 0; }\n"
                "int cpp_header_smoke() {\n"
                "  PySoacTypeConstructionSpec construction{};\n"
                "  PySoacTypeContractSpecV4 contract{};\n"
                "  construction.struct_size = sizeof(construction);\n"
                "  construction.construction_mode = Py_SOAC_TYPE_CONSTRUCT_PENDING;\n"
                "  construction.commit_final = final_commit;\n"
                "  construction.contract = contract;\n"
                "  return info && admit && fail && dispose ? 0 : 1;\n"
                "}\n"
            ),
            capture_output=True,
            text=True,
            check=False,
            timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


class TypeParameterScopeNativeTests(unittest.TestCase):
    @staticmethod
    def subscript(parameters):
        return native_api(
            "PySoac_SubscriptGeneric", ctypes.py_object, ctypes.py_object,
        )(parameters)

    @staticmethod
    def set_parameters(function, parameters):
        return native_api(
            "PySoac_SetFunctionTypeParameters", ctypes.py_object,
            ctypes.py_object, ctypes.py_object,
        )(function, parameters)

    def test_native_generic_base_preserves_unpack_exception(self):
        # The compiler's native intrinsic and the explicit SOAC consumer share
        # the same implementation. A failed Unpack callback must never reach
        # vectorcall as a NULL positional argument.
        result = subprocess.run(
            [sys.executable, "-I", "-S", "-B", "-c", '''
import typing
error = RuntimeError("native unpack failed")
class BrokenUnpack:
    def __getitem__(self, item):
        raise error
typing.Unpack = BrokenUnpack()
try:
    class Example[*Ts]:
        pass
except RuntimeError as caught:
    assert caught is error
else:
    raise AssertionError("the original unpack failure was lost")
'''],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_generic_base_matches_native_parameter_unpacking(self):
        import typing

        class Original[T, *Ts]:
            pass

        parameters = Original.__type_params__
        expected = Original.__orig_bases__[-1]
        actual = self.subscript(parameters)
        self.assertIs(actual.__origin__, typing.Generic)
        self.assertEqual(actual.__args__, expected.__args__)
        self.assertIs(actual.__args__[0], parameters[0])
        self.assertIs(actual.__args__[1].__args__[0], parameters[1])
        self.assertEqual(actual.__parameters__, parameters)

    def test_generic_base_preserves_typing_callback_without_using_its_type_alias(self):
        import typing

        class Original[T]:
            pass

        parameters = Original.__type_params__
        actual_generic = typing.Generic
        original_alias = typing._GenericAlias
        events = []
        result = object()

        def recording_alias(origin, arguments):
            events.append((origin, arguments))
            return result

        try:
            typing._GenericAlias = recording_alias
            typing.Generic = object()
            self.assertIs(self.subscript(parameters), result)
        finally:
            typing.Generic = actual_generic
            typing._GenericAlias = original_alias
        self.assertEqual(len(events), 1)
        self.assertIs(events[0][0], actual_generic)
        self.assertIs(events[0][1], parameters)

    def test_generic_base_consumer_preserves_each_callback_exception(self):
        import typing

        class Original[*Ts]:
            pass

        parameters = Original.__type_params__
        original_unpack = typing.Unpack
        original_alias = typing._GenericAlias
        error = RuntimeError("generic callback failed")
        events = []

        class BrokenUnpack:
            def __getitem__(self, parameter):
                events.append(parameter)
                raise error

        def broken_alias(origin, arguments):
            events.append(origin)
            raise error

        try:
            typing.Unpack = BrokenUnpack()
            with self.assertRaises(RuntimeError) as caught:
                self.subscript(parameters)
            self.assertIs(caught.exception, error)
            self.assertIs(events.pop(), parameters[0])
            typing.Unpack = original_unpack
            typing._GenericAlias = broken_alias
            with self.assertRaises(RuntimeError) as caught:
                self.subscript(parameters)
            self.assertIs(caught.exception, error)
            self.assertIs(events.pop(), typing.Generic)
        finally:
            typing.Unpack = original_unpack
            typing._GenericAlias = original_alias
        self.assertEqual(events, [])

    def test_function_parameter_attachment_preserves_exact_tuple_and_native_release_order(self):
        import typing

        def function(value=17):
            return value

        events = []
        parameters = (typing.TypeVar("T"),)

        class OldParameter:
            def __del__(self):
                events.append(function.__type_params__)

        function.__type_params__ = (OldParameter(),)
        code = function.__code__
        defaults = function.__defaults__
        self.assertIs(self.set_parameters(function, parameters), function)
        self.assertIs(function.__type_params__, parameters)
        self.assertEqual(len(events), 1)
        self.assertIs(events[0], parameters)
        self.assertIs(function.__code__, code)
        self.assertIs(function.__defaults__, defaults)
        self.assertEqual(function(), 17)
        self.assertEqual(
            native_api("PyFunction_GetSoacStrictId", ctypes.c_uint64,
                       ctypes.py_object)(function), 0,
        )

    def test_function_parameter_attachment_obeys_permanent_function_seal(self):
        import typing

        def function():
            return 17

        parameters = (typing.TypeVar("T"),)
        self.set_parameters(function, parameters)
        native_api("PyFunction_SealSoacStrict", ctypes.c_int,
                   ctypes.py_object, ctypes.c_uint64)(function, 87001)
        error = borrowed_object_api("PySoac_GetStrictMutationError")
        for replacement in (parameters, (typing.TypeVar("S"),)):
            with self.subTest(replacement=replacement):
                with self.assertRaises(error):
                    self.set_parameters(function, replacement)
                self.assertIs(function.__type_params__, parameters)

    def test_scope_consumers_reject_invalid_inputs_before_callbacks(self):
        import typing

        def function():
            pass

        class TupleSubclass(tuple):
            def __iter__(self):
                raise AssertionError("compiler tuple validation called Python")

        valid = (typing.TypeVar("T"),)
        for parameters in (None, [], TupleSubclass(valid), (object(),)):
            with self.subTest(parameters=parameters):
                with self.assertRaises(TypeError):
                    self.subscript(parameters)
                with self.assertRaises(TypeError):
                    self.set_parameters(function, parameters)
                self.assertEqual(function.__type_params__, ())
        with self.assertRaises(TypeError):
            self.set_parameters(len, valid)

    def test_type_parameter_metadata_does_not_authorize_original_strict_frames(self):
        import typing

        source = b"from __future__ import strict\ndef value[T](): return 17\n"
        root = native_api(
            "PySoac_CompileVerifiedSource", ctypes.py_object,
            ctypes.c_char_p, ctypes.c_ssize_t, ctypes.py_object, ctypes.c_int,
        )(source, len(source), "<strict-parameter-scope>", -1)
        scope = next(item for item in root.co_consts if type(item) is types.CodeType)
        code = next(item for item in scope.co_consts if type(item) is types.CodeType)
        function = types.FunctionType(code, {"__builtins__": __builtins__})
        parameters = (typing.TypeVar("T"),)
        self.set_parameters(function, parameters)
        self.assertIs(function.__type_params__, parameters)
        error = borrowed_object_api("PySoac_GetStrictRuntimeUnavailableError")
        with self.assertRaisesRegex(error, "strict code execution"):
            function()


class TypeExpressionFactoryNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.new_alias = native_api(
            "PySoac_NewTypeAlias", ctypes.py_object,
            ctypes.py_object, ctypes.py_object, ctypes.py_object,
        )
        cls.new_parameter = native_api(
            "PySoac_NewTypeParameter", ctypes.py_object,
            ctypes.c_int, ctypes.py_object, ctypes.c_void_p,
        )
        cls.set_default = native_api(
            "PySoac_SetTypeParameterDefault", ctypes.py_object,
            ctypes.py_object, ctypes.py_object,
        )
        cls.matches = native_api(
            "PySoac_MatchesTypeExpression", ctypes.c_int,
            ctypes.py_object, ctypes.c_int, ctypes.py_object,
        )

    def test_alias_factory_keeps_actual_evaluator_lazy_and_native_cache(self):
        import typing

        events = []
        value = []

        def evaluate(format=1, /):
            events.append(format)
            return value

        alias = self.new_alias("Alias", None, evaluate)
        self.assertIs(type(alias), typing.TypeAliasType)
        self.assertEqual(alias.__name__, "Alias")
        self.assertEqual(alias.__qualname__, evaluate.__qualname__)
        self.assertIs(alias.evaluate_value, evaluate)
        self.assertEqual(alias.__type_params__, ())
        self.assertEqual(events, [])
        self.assertIs(alias.__value__, value)
        self.assertIs(alias.__value__, value)
        self.assertEqual(events, [1])
        self.assertEqual(self.matches(alias, 0, evaluate), 1)

    def test_bound_constraints_and_separate_default_are_lazy_native_slots(self):
        import typing

        events = []

        def bound(format=1, /):
            events.append("bound")
            return int

        def constraints(format=1, /):
            events.append("constraints")
            return (int, str)

        def default(format=1, /):
            events.append("default")
            return bytes

        parameter = self.new_parameter(1, "T", id(bound))
        constrained = self.new_parameter(2, "S", id(constraints))
        self.assertIs(type(parameter), typing.TypeVar)
        self.assertTrue(parameter.__infer_variance__)
        self.assertIs(parameter.__default__, typing.NoDefault)
        self.assertIs(self.set_default(parameter, default), parameter)
        self.assertIs(parameter.evaluate_bound, bound)
        self.assertIs(constrained.evaluate_constraints, constraints)
        self.assertIs(parameter.evaluate_default, default)
        self.assertEqual(events, [])
        self.assertEqual(self.matches(parameter, 1, bound), 1)
        self.assertEqual(self.matches(constrained, 2, constraints), 1)
        self.assertEqual(self.matches(parameter, 3, default), 1)
        self.assertIs(parameter.__bound__, int)
        self.assertIs(parameter.__bound__, int)
        self.assertEqual(constrained.__constraints__, (int, str))
        self.assertIs(parameter.__default__, bytes)
        self.assertIs(parameter.__default__, bytes)
        self.assertEqual(events, ["bound", "constraints", "default"])

    def test_unbounded_and_variadic_parameters_use_current_native_types(self):
        import typing

        def default(format=1, /):
            return (int,)

        for kind, expected in [(0, typing.TypeVar), (3, typing.ParamSpec),
                               (4, typing.TypeVarTuple)]:
            with self.subTest(kind=kind):
                parameter = self.new_parameter(kind, "Parameter", None)
                self.assertIs(type(parameter), expected)
                self.assertIs(parameter.__default__, typing.NoDefault)
                self.assertIs(self.set_default(parameter, default), parameter)
                self.assertEqual(self.matches(parameter, 3, default), 1)
                self.assertEqual(parameter.__default__, (int,))
        parameter = self.new_parameter(0, "T", None)
        alias = self.new_alias("Generic", (parameter,), default)
        self.assertIs(alias.__type_params__[0], parameter)

    def test_target_matching_uses_private_identity_without_python_attributes(self):
        import typing

        def evaluate(format=1, /):
            return int

        alias = self.new_alias("Alias", None, evaluate)
        clone = types.FunctionType(evaluate.__code__, evaluate.__globals__, argdefs=(1,))

        class Spoof:
            def __getattribute__(self, name):
                raise AssertionError("native predicate consulted a Python attribute")

        self.assertEqual(self.matches(alias, 0, clone), 0)
        self.assertEqual(self.matches(alias, 1, evaluate), 0)
        # ctypes' py_object argument converter itself consults attributes.
        # Pin both objects and cross this native-only predicate as raw pointers.
        raw_matches = ctypes.PYFUNCTYPE(
            ctypes.c_int, ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p
        )(("PySoac_MatchesTypeExpression", ctypes.pythonapi))
        spoof = Spoof()
        self.assertEqual(raw_matches(id(spoof), 0, id(evaluate)), 0)
        self.assertEqual(self.matches(typing.TypeAliasType("Other", int), 0, evaluate), 0)

    def test_factory_validation_does_not_invoke_or_replace_evaluators(self):
        events = []

        def evaluate(format=1, /):
            events.append(format)
            return int

        for name, parameters, evaluator in [(42, None, evaluate),
                                             ("Alias", [], evaluate),
                                             ("Alias", None, len)]:
            with self.subTest(name=name, parameters=parameters, evaluator=evaluator):
                with self.assertRaises(TypeError):
                    self.new_alias(name, parameters, evaluator)
        with self.assertRaises(ValueError):
            self.new_parameter(99, "T", None)
        with self.assertRaises(TypeError):
            self.new_parameter(1, "T", None)
        with self.assertRaises(TypeError):
            self.new_parameter(0, "T", id(evaluate))
        parameter = self.new_parameter(0, "T", None)
        self.set_default(parameter, evaluate)
        with self.assertRaises(TypeError):
            self.set_default(parameter, evaluate)
        self.assertIs(parameter.evaluate_default, evaluate)
        self.assertEqual(events, [])

    def test_factory_does_not_add_target_or_evaluator_lifetime_edges(self):
        class Payload:
            pass

        def make():
            payload = Payload()

            def evaluate(format=1, /):
                return payload

            return (self.new_alias("Alias", None, evaluate),
                    weakref.ref(evaluate), weakref.ref(payload))

        alias, evaluator, payload = make()
        self.assertIsNotNone(evaluator())
        self.assertIsNotNone(payload())
        del alias
        self.assertIsNone(evaluator())
        self.assertIsNone(payload())

    def test_factory_identity_match_never_authorizes_strict_bytecode(self):
        compile_details = native_api(
            "PySoac_CompileVerifiedSourceDetails", ctypes.py_object,
            ctypes.c_char_p, ctypes.c_ssize_t, ctypes.py_object, ctypes.c_int,
        )
        source = b"from __future__ import strict\ntype Alias = list[int]\n"
        root, _, _ = compile_details(source, len(source), "<strict-alias-factory>", -1)
        code = next(value for value in root.co_consts
                    if isinstance(value, types.CodeType) and value.co_name == "Alias")
        evaluate = types.FunctionType(code, {"__builtins__": __builtins__}, argdefs=(1,))
        alias = self.new_alias("Alias", None, evaluate)
        self.assertEqual(self.matches(alias, 0, evaluate), 1)
        error = borrowed_object_api("PySoac_GetStrictRuntimeUnavailableError")
        with self.assertRaisesRegex(error, "strict code execution"):
            alias.__value__


class ClassBindingMetadataNativeTests(unittest.TestCase):
    # These declarations mirror the six maintained class-coupling cases. The
    # native controls execute ordinary source; strict code is inspected only.
    SOURCES = {
        "plain_target": """
def build(marker):
    class Box:
        values = [item for item in (marker,)]
    return Box
""",
        "captured_target": """
def build(marker):
    class Box:
        values = [lambda: item for item in (marker,)]
    return Box
""",
        "class_cell": """
def build(marker):
    class Box:
        values = [lambda: __class__ for __class__ in (marker,)]
        def read(self):
            return __class__
    return Box
""",
        "class_dictionary_cell": """
def build(marker):
    class Box:
        values = [lambda: __classdict__ for __classdict__ in (marker,)]
        field: int
    return Box
""",
        "conditional_annotation_cell": """
def build(marker, condition):
    class Box:
        values = [
            lambda: __conditional_annotations__
            for __conditional_annotations__ in (marker,)
        ]
        if condition:
            field: int
    return Box
""",
        "shadowed_lexical_free": """
def build(marker):
    outside = marker
    class Box:
        def read(self):
            return outside
        values = [lambda: outside for outside in (7, 8)]
    return Box
""",
    }

    CONDITIONAL_FREE_SOURCE = """
def build(marker, enabled):
    value = marker
    class Box:
        values = [
            (
                [lambda: value for value in (7,)] if enabled else None,
                value,
            )
            for unused in (0,)
        ]
    return Box
"""

    FINALLY_SOURCE = """def build(callback, escaped):
    class Box:
        try:
            callback()
        finally:
            values = [lambda: value for value in (1,)]
            escaped.append(values[0])
    return Box
"""

    class RawPySoacCodeView(ctypes.Structure):
        _fields_ = [
            ("abi_version", ctypes.c_uint),
            *[(name, ctypes.c_int) for name in (
                "flags", "argcount", "posonlyargcount", "kwonlyargcount",
                "stacksize", "firstlineno", "nlocalsplus", "framesize",
                "nlocals", "ncellvars", "nfreevars",
            )],
            ("code_units", ctypes.c_ssize_t),
            ("strict_source_id", ctypes.c_uint64),
            *[(name, ctypes.c_void_p) for name in (
                "consts", "names", "localsplusnames", "localspluskinds",
                "filename", "name", "qualname", "linetable", "exceptiontable",
            )],
        ]

    @classmethod
    def setUpClass(cls):
        cls.compile_details = native_api(
            "PySoac_CompileVerifiedSourceDetails", ctypes.py_object,
            ctypes.c_char_p, ctypes.c_ssize_t, ctypes.py_object, ctypes.c_int,
        )
        cls.source_id = native_api(
            "PyCode_GetSoacStrictSourceId", ctypes.c_uint64, ctypes.py_object
        )
        cls.get_view = native_api(
            "PySoac_GetCodeView", ctypes.c_int, ctypes.py_object,
            ctypes.POINTER(cls.RawPySoacCodeView), ctypes.c_size_t,
        )
        cls.runtime_error = borrowed_object_api("PySoac_GetStrictRuntimeUnavailableError")

    def code_slots(self, code):
        view = self.RawPySoacCodeView()
        self.assertEqual(self.get_view(code, ctypes.byref(view), ctypes.sizeof(view)), 0)
        self.assertEqual(view.abi_version, 1)
        # Both are borrowed native pointers, pinned by the exact code argument.
        names = ctypes.cast(view.localsplusnames, ctypes.py_object).value
        kinds = ctypes.cast(view.localspluskinds, ctypes.py_object).value
        self.assertEqual(len(names), view.nlocalsplus)
        self.assertEqual(len(kinds), view.nlocalsplus)
        return names, kinds

    def compile_scopes(self, body, optimize=-1):
        source = ("from __future__ import strict\n" + body).encode()
        root, _, section = self.compile_details(
            source, len(source), "<native-class-bindings>", optimize
        )
        version, nodes, recipes, tables = section
        self.assertEqual(version, 7)
        self.assertEqual([table[0] for table in tables], list(range(len(nodes))))
        self.assertEqual([recipe[0] for recipe in recipes], list(range(len(nodes))))
        self.assert_is_immutable(section)
        self.assertIs(nodes[0][2], root)
        actual_tree = []

        def visit(code, parent):
            node_id = len(actual_tree)
            actual_tree.append((code, parent))
            for constant in code.co_consts:
                if type(constant) is types.CodeType:
                    visit(constant, node_id)

        visit(root, None)
        self.assertEqual(len(nodes), len(actual_tree))
        identity = self.source_id(root)
        self.assertGreater(identity, 0)
        for index, (node, (code, parent)) in enumerate(zip(nodes, actual_tree, strict=True)):
            self.assertEqual(node[0], index)
            self.assertEqual(node[1], parent)
            self.assertIs(node[2], code)
            self.assertEqual(self.source_id(code), identity)
            self.assert_recipe(nodes, recipes[index], tables[index])
        return source, root, nodes, recipes, tables

    def compile_source(self, body, optimize=-1):
        # Existing class scenarios select their actual class recipes from the
        # shared product; no old-wire reconstruction or separate compilation.
        source, root, nodes, recipes, _ = self.compile_scopes(body, optimize)
        return source, root, nodes, tuple(recipe for recipe in recipes if nodes[recipe[0]][3] == 1)

    def assert_recipe(self, nodes, recipe, table):
        import dis

        self.assertEqual(len(recipe), 7)
        code_id, seeds, owners, regions, captures, accesses, actions = recipe
        code = nodes[code_id][2]
        names, kinds = self.code_slots(code)
        parameters = code.co_argcount + code.co_kwonlyargcount + bool(code.co_flags & 0x04) + bool(code.co_flags & 0x08)
        self.assertEqual(seeds, tuple(
            (slot, kind, int(slot < parameters), slot if slot < parameters else None)
            for slot, kind in enumerate(kinds)))
        self.assertEqual([slot for slot, kind, *_ in seeds if kind & 0x0e], list(range(parameters)))
        free_slots = [slot for slot, kind in enumerate(kinds) if kind & 0x80]
        self.assertEqual(free_slots, list(range(len(names) - len(code.co_freevars), len(names))))
        self.assertEqual(tuple(names[slot] for slot in free_slots), code.co_freevars)
        entry_slots = set()
        for index, (owner_id, kind, slot, native_kind, region) in enumerate(owners):
            self.assertEqual(owner_id, index)
            self.assertGreaterEqual(slot, 0)
            self.assertLess(slot, len(names))
            self.assertEqual(native_kind, kinds[slot])
            if kind == 0:
                self.assertIsNone(region)
                self.assertNotIn(slot, entry_slots)
                entry_slots.add(slot)
            else:
                self.assertIn(kind, (1, 2))
                self.assertGreaterEqual(region, 0)
                self.assertLess(region, len(regions))
        self.assertEqual(entry_slots, set(range(len(names))))

        # This is native publication/CALL authority, not a SOAC execution plan.
        # Its source store origins still authenticate comprehension targets.
        self.assertEqual(len(table), 7)
        self.assertEqual(table[0], code_id)
        self.assertIs(table[3], code.co_names)
        instructions = [item for item in dis.get_instructions(code, adaptive=False)
                        if item.opcode != opcode.opmap["EXTENDED_ARG"]]
        self.assertEqual(table[1], len(instructions))
        self.assertEqual(table[2], len(code.co_code))
        saved_owners = set()
        current_regions = {None, *range(len(regions))}
        store_origins = {origin for origin, _ in table[4]}
        for index, region in enumerate(regions):
            self.assertEqual(len(region), 8)
            region_id, parent, comp_kind, span, outer, asynchronous, operations, bindings = region
            self.assertEqual(region_id, index)
            self.assertIn(comp_kind, (0, 1, 2))
            self.assertIsNotNone(span)
            self.assertIsNotNone(outer)
            self.assertIn(asynchronous, (0, 1))
            if parent is not None:
                self.assertGreaterEqual(parent, 0)
                self.assertLess(parent, region_id)
            for operation, slot, owner in operations:
                self.assertGreaterEqual(owner, 0)
                self.assertLess(owner, len(owners))
                self.assertEqual(owners[owner][2], slot)
                self.assertEqual(owners[owner][4], region_id)
                self.assertIn(operation, (0, 1))
                if operation == 0:
                    self.assertEqual(owners[owner][1], 2)
                    self.assertNotIn(owner, saved_owners)
                    saved_owners.add(owner)
                else:
                    self.assertEqual(owners[owner][1], 1)
                    self.assertTrue(kinds[slot] & (0x40 | 0x80))
            for role, generator, origin, form, operand in bindings:
                self.assertIn(role, (0, 1))
                if role == 0:
                    self.assertIsInstance(generator, int)
                    self.assertGreaterEqual(generator, 0)
                else:
                    self.assertIsNone(generator)
                self.assertIn(origin, store_origins)
                self.assertIn(form, (0, 3, 4, 5, 6, 7, 8))
                self.assertIn(operand[0], (0, 1, 2))

        for child, span, ordinal, (kind, slot), region in captures:
            self.assertGreaterEqual(child, 0)
            self.assertLess(child, len(nodes))
            self.assertEqual(nodes[child][1], code_id)
            self.assertIsNotNone(span)
            self.assertGreaterEqual(ordinal, 0)
            self.assertLess(ordinal, len(nodes[child][2].co_freevars))
            self.assertEqual(kind, 0)
            self.assertGreaterEqual(slot, 0)
            self.assertLess(slot, len(names))
            self.assertTrue(kinds[slot] & (0x40 | 0x80))
            self.assertEqual(names[slot], nodes[child][2].co_freevars[ordinal])
            self.assertIn(region, current_regions)
        for span, context, mode, (kind, slot), region in accesses:
            self.assertIsNotNone(span)
            self.assertIn(context, (0, 1, 2))
            self.assertIn(mode, (0, 1, 2))
            self.assertEqual(kind, 0)
            self.assertGreaterEqual(slot, 0)
            self.assertLess(slot, len(names))
            self.assertTrue(kinds[slot] & (0x20 if mode == 0 else 0x40 | 0x80))
            self.assertIn(region, current_regions)
            if mode == 2:
                self.assertEqual(context, 0)
        if nodes[code_id][3] == 1:
            header, exports = actions
            for owner, role, operand in header:
                self.assertEqual(owners[owner][1], 0)
                self.assertIn(role, (3, 4))
                self.assertIsNone(operand)
                self.assertTrue(kinds[owners[owner][2]] & 0x40)
                self.assertFalse(kinds[owners[owner][2]] & 0x80)
            for role, (kind, slot) in exports:
                self.assertIn(role, (0, 1))
                self.assertEqual(kind, 0)
                self.assertTrue(kinds[slot] & 0x40)
        else:
            self.assertIsNone(actions)

    @staticmethod
    def source_span(node):
        return (node.lineno, node.col_offset, node.end_lineno, node.end_col_offset)

    def source_scope(self, nodes, recipes, original, scope_kind):
        selected = [node for node in nodes if node[3] == scope_kind and node[5] == self.source_span(original)]
        self.assertEqual(len(selected), 1)
        node, = selected
        return node, recipes[node[0]]

    def assert_ordinary_code_equal(self, left, right):
        for attribute in ("co_code", "co_flags", "co_names", "co_varnames", "co_argcount",
                          "co_posonlyargcount", "co_kwonlyargcount", "co_cellvars", "co_freevars",
                          "co_linetable", "co_exceptiontable", "co_name", "co_qualname"):
            self.assertEqual(getattr(left, attribute), getattr(right, attribute))
        self.assertEqual(tuple(left.co_positions()), tuple(right.co_positions()))
        self.assertEqual(len(left.co_consts), len(right.co_consts))
        for a, b in zip(left.co_consts, right.co_consts, strict=True):
            if type(a) is types.CodeType:
                self.assert_ordinary_code_equal(a, b)
            else:
                self.assertEqual(a, b)

    def assert_is_immutable(self, value):
        if type(value) is tuple:
            for item in value:
                self.assert_is_immutable(item)
        else:
            self.assertIn(type(value), (int, str, type(None), types.CodeType))



    @staticmethod
    def ordinary_module(body):
        module = types.ModuleType("native_class_binding_control")
        exec(compile(body, "<native-class-control>", "exec", dont_inherit=True), module.__dict__)
        return module

    @staticmethod
    def closure_cell(function, name):
        return function.__closure__[function.__code__.co_freevars.index(name)]

    def test_six_original_coupled_sources_have_complete_native_recipes(self):
        for name, body in self.SOURCES.items():
            with self.subTest(case=name):
                _, _, _, recipes = self.compile_source(body)
                self.assertEqual(len(recipes), 1)
                self.assertTrue(recipes[0][3])

    def test_annotation_helpers_keep_final_native_free_slots_and_exact_captures(self):
        cases = (
            ("class_dictionary_cell", {"__classdict__"}),
            ("conditional_annotation_cell", {"__classdict__", "__conditional_annotations__"}),
        )
        for case, expected_free in cases:
            for optimize in (0, 1, 2):
                with self.subTest(case=case, optimize=optimize):
                    source, root, nodes, recipes, _ = self.compile_scopes(self.SOURCES[case], optimize)
                    class_node, = [node for node in nodes if node[3] == 1]
                    provider, = [node for node in nodes
                                 if node[1] == class_node[0] and node[4] == 3]
                    code = provider[2]
                    self.assertEqual(set(code.co_freevars), expected_free)
                    recipe = recipes[provider[0]]
                    names, kinds = self.code_slots(code)
                    free_slots = [slot for slot, kind in enumerate(kinds) if kind & 0x80]
                    self.assertEqual(tuple(names[slot] for slot in free_slots), code.co_freevars)
                    self.assertEqual([recipe[1][slot][2:] for slot in free_slots],
                                     [(0, None)] * len(code.co_freevars))
                    captures = sorted(
                        (edge for edge in recipes[class_node[0]][4] if edge[0] == provider[0]),
                        key=lambda edge: edge[2],
                    )
                    self.assertEqual([edge[2] for edge in captures], list(range(len(code.co_freevars))))
                    class_names, _ = self.code_slots(class_node[2])
                    self.assertEqual(tuple(class_names[edge[3][1]] for edge in captures),
                                     code.co_freevars)

                    # The collector observes the finished native maps; it must
                    # not add closure slots or alter ordinary compilation.
                    ordinary = compile(source, "<native-class-bindings>", "exec",
                                       dont_inherit=True, optimize=optimize)
                    self.assert_ordinary_code_equal(root, ordinary)
                    self.assertEqual(self.source_id(ordinary), 0)

    def test_six_original_coupled_sources_preserve_native_cell_behavior(self):
        for name, body in self.SOURCES.items():
            with self.subTest(case=name):
                marker = object()
                module = self.ordinary_module(body)
                cls = module.build(marker, True) if name == "conditional_annotation_cell" else module.build(marker)
                if name == "plain_target":
                    self.assertEqual(cls.values, [marker])
                    self.assertNotIn("item", vars(cls))
                    continue
                transient = cls.values[0]
                if name == "shadowed_lexical_free":
                    self.assertTrue(all(function() == 8 for function in cls.values))
                    method = vars(cls)["read"]
                    cell = self.closure_cell(method, "outside")
                    with self.assertRaises(ValueError):
                        cell.cell_contents
                    with self.assertRaises(NameError):
                        cls().read()
                    self.assertIsNot(cell, self.closure_cell(transient, "outside"))
                    continue
                self.assertIs(transient(), marker)
                if name == "class_cell":
                    self.assertIs(cls().read(), cls)
                    self.assertIsNot(self.closure_cell(vars(cls)["read"], "__class__"),
                                     self.closure_cell(transient, "__class__"))
                elif name == "class_dictionary_cell":
                    provider = vars(cls)["__annotate_func__"]
                    cell = self.closure_cell(provider, "__classdict__")
                    self.assertIsNot(cell, self.closure_cell(transient, "__classdict__"))
                    with self.assertRaises(ValueError):
                        cell.cell_contents
                    cell.cell_contents = {"int": str}
                    self.assertEqual(provider(1), {"field": str})
                    del cell.cell_contents
                    with self.assertRaises(NameError):
                        provider(1)
                elif name == "conditional_annotation_cell":
                    provider = vars(cls)["__annotate_func__"]
                    cell = self.closure_cell(provider, "__conditional_annotations__")
                    self.assertIsNot(cell, self.closure_cell(transient, "__conditional_annotations__"))
                    indices = cell.cell_contents
                    self.assertEqual(provider(1), {"field": int})
                    indices.clear()
                    self.assertEqual(provider(1), {})
                    self.assertIs(cell.cell_contents, indices)

    def test_hidden_classdict_cell_is_not_inferred_to_be_namespace_initialized(self):
        _, _, nodes, (recipe,) = self.compile_source(self.SOURCES["class_dictionary_cell"])
        names, kinds = self.code_slots(nodes[recipe[0]][2])
        provider = next(node for node in nodes if node[1] == recipe[0] and node[4] == 3)
        ordinal = provider[2].co_freevars.index("__classdict__")
        edge = next(edge for edge in recipe[4] if edge[:1] == (provider[0],) and edge[2] == ordinal)
        slot = edge[3][1]
        self.assertEqual(names[slot], "__classdict__")
        self.assertTrue(kinds[slot] & 0x40)
        entry, = [owner for owner in recipe[2] if owner[1] == 0 and owner[2] == slot]
        self.assertIsNone(entry[4])
        self.assertFalse(any(role == 3 and recipe[2][owner][2] == slot
                             for owner, role, _ in recipe[6][0]))
        self.assertTrue(any(op[0] == 1 and op[1] == slot for region in recipe[3] for op in region[6]))

    def test_same_spelling_cell_and_free_remain_distinct_native_slots(self):
        _, _, nodes, (recipe,) = self.compile_source(self.SOURCES["shadowed_lexical_free"])
        names, kinds = self.code_slots(nodes[recipe[0]][2])
        slots = [index for index, name in enumerate(names) if name == "outside"]
        self.assertEqual(len(slots), 2)
        cell_slot = next(index for index in slots if kinds[index] & 0x40)
        free_slot = next(index for index in slots if kinds[index] & 0x80)
        self.assertNotEqual(cell_slot, free_slot)
        method = next(node for node in nodes if node[1] == recipe[0] and node[2].co_name == "read")
        edge = next(edge for edge in recipe[4] if edge[0] == method[0])
        self.assertEqual(edge[3], (0, cell_slot))
        self.assertEqual(recipe[1][free_slot][2:], (0, None))
        free_slots = [slot for slot, kind in enumerate(kinds) if kind & 0x80]
        self.assertEqual(tuple(names[slot] for slot in free_slots), nodes[recipe[0]][2].co_freevars)

    def test_interleaved_targets_keep_distinct_semantic_cell_and_snapshot_owners(self):
        import ast

        body = """
def build(marker):
    class Box:
        values = [lambda: (left, right) for left in (marker,) for right in (marker,)]
    return Box
"""
        source, _, nodes, (recipe,) = self.compile_source(body)
        names, kinds = self.code_slots(nodes[recipe[0]][2])
        region, = recipe[3]
        original = next(node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.ListComp))
        self.assertEqual(region[3], self.source_span(original))
        targets = {row[4][1] for row in region[7] if row[0] == 0}
        self.assertEqual({names[slot] for slot in targets}, {"left", "right"})
        self.assertEqual({row[1] for row in region[7] if row[0] == 0}, {0, 1})
        for operation in (0, 1):
            self.assertEqual({slot for kind, slot, _ in region[6] if kind == operation}, targets)
        self.assertTrue(all(kinds[slot] & 0x40 for slot in targets))
        self.assertEqual(len({owner for _, _, owner in region[6]}), 4)
        self.assertEqual({edge[3][1] for edge in recipe[4]}, targets)
        self.assertTrue(all(edge[4] == region[0] for edge in recipe[4]))
        marker = object()
        cls = self.ordinary_module(body).build(marker)
        self.assertEqual(cls.values[0](), (marker, marker))

    def test_nested_and_repeated_regions_keep_distinct_lexical_snapshot_owners(self):
        body = """
def build(marker):
    class Box:
        values = [[lambda: (outer, inner) for inner in (outer,)] for outer in (marker,)]
        again = [lambda: outer for outer in (marker,)]
    return Box
"""
        _, _, _, (recipe,) = self.compile_source(body)
        regions = recipe[3]
        self.assertEqual(len(regions), 3)
        self.assertEqual([region[1] for region in regions], [None, 0, None])
        marker = object()
        cls = self.ordinary_module(body).build(marker)
        self.assertEqual(cls.values[0][0](), (marker, marker))
        self.assertIs(cls.again[0](), marker)

    def test_conditional_free_replacement_ordinary_control(self):
        module = self.ordinary_module(self.CONDITIONAL_FREE_SOURCE)
        marker = object()
        skipped = module.build(marker, False)
        executed = module.build(marker, True)
        self.assertIsNone(skipped.values[0][0])
        self.assertIs(skipped.values[0][1], marker)
        self.assertEqual(executed.values[0][1], 7)
        captured = executed.values[0][0][0]
        self.assertEqual(captured(), 7)
        self.assertEqual(self.closure_cell(captured, "value").cell_contents, 7)

    def test_conditional_free_replacement_captures_current_slot_not_static_generation(self):
        import ast

        source, _, nodes, (recipe,) = self.compile_source(self.CONDITIONAL_FREE_SOURCE)
        names, kinds = self.code_slots(nodes[recipe[0]][2])
        free_slot = next(i for i, name in enumerate(names) if name == "value" and kinds[i] & 0x80)
        self.assertTrue(any(
            any(op[0] == 1 and op[1] == free_slot for op in region[6])
            for region in recipe[3]
        ))
        self.assertTrue(any(edge[3] == (0, free_slot) for edge in recipe[4]))
        class_ast = next(node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.ClassDef))
        outer = class_ast.body[0].value
        read = outer.elt.elts[1]
        target = outer.elt.elts[0].body.generators[0].target
        for node, context in ((read, 0), (target, 1)):
            self.assertIsInstance(node, ast.Name)
            span = (node.lineno, node.col_offset, node.end_lineno, node.end_col_offset)
            region_expression = outer if node is read else outer.elt.elts[0].body
            region_id = next(region[0] for region in recipe[3]
                             if region[3] == self.source_span(region_expression))
            self.assertIn((span, context, 1, (0, free_slot), region_id), recipe[5])

    def test_ordinary_augassign_positions_match_native19_before_metadata_hooks(self):
        # Captured from the pinned native19 binary before this compiler change.
        # co_positions is a public diagnostic API, not rendered compiler text.
        cases = (
            (
                "def update(value):\n    value += 7\n    return value\n",
                ("update",),
                ((1, 1, 0, 0), (2, 2, 4, 9), (2, 2, 13, 14))
                + ((2, 2, 4, 14),) * 6
                + ((2, 2, 4, 9), (3, 3, 11, 16), (3, 3, 4, 16)),
            ),
            (
                "def build(value):\n    class Box:\n        nonlocal value\n"
                "        value += 7\n        seen = value\n    return Box\n",
                ("build", "Box"),
                ((None, None, None, None),) + ((2, 2, 0, 0),) * 7
                + ((4, 4, 8, 13),) * 2 + ((4, 4, 17, 18),)
                + ((4, 4, 8, 18),) * 6 + ((4, 4, 8, 13),)
                + ((5, 5, 15, 20),) * 2 + ((5, 5, 8, 12),) * 5,
            ),
        )
        for source, path, expected in cases:
            with self.subTest(code_path=path):
                code = compile(source, "<ordinary-augassign-position>", "exec", dont_inherit=True)
                for name in path:
                    code, = [item for item in code.co_consts
                             if type(item) is types.CodeType and item.co_name == name]
                self.assertEqual(tuple(code.co_positions()), expected)

    def test_finally_reemission_ordinary_control(self):
        module = self.ordinary_module(self.FINALLY_SOURCE)
        normal = []
        cls = module.build(lambda: None, normal)
        self.assertEqual(len(normal), 1)
        self.assertIs(normal[0], cls.values[0])
        self.assertEqual(normal[0](), 1)
        exceptional = []

        def fail():
            raise ValueError("exercise the copied finally path")

        with self.assertRaises(ValueError):
            module.build(fail, exceptional)
        self.assertEqual(len(exceptional), 1)
        self.assertEqual(exceptional[0](), 1)
        self.assertIsNot(normal[0].__closure__[0], exceptional[0].__closure__[0])

    def test_finally_reemission_normalizes_only_equal_semantic_bindings(self):
        _, _, nodes, (recipe,) = self.compile_source(self.FINALLY_SOURCE)
        code = nodes[recipe[0]][2]
        self.assertEqual(len(recipe[3]), 1)
        region, = recipe[3]
        self.assertCountEqual([operation[0] for operation in region[6]], [0, 1])
        self.assertEqual(len(recipe[2]), len(self.code_slots(code)[0]) + 2)
        child, = [node for node in nodes if node[1] == recipe[0]]
        self.assertEqual(len(recipe[4]), 1)
        self.assertEqual(recipe[4][0][0], child[0])
        self.assertIs(child[2], next(item for item in code.co_consts if type(item) is types.CodeType))
        self.assertEqual(len(recipe[5]), 3)
        self.assertEqual(len({(row[0], row[1]) for row in recipe[5]}), 3)

    OUTER_FINALLY_SOURCES = {
        "plain": """
def build(callback, escaped):
    try:
        callback()
    finally:
        class Box:
            values = [lambda: value for value in (1,)]
            def read(self):
                return __class__
        escaped.append(Box)
    return Box
""",
        "decorated": """
def build(callback, escaped, decorate):
    try:
        callback()
    finally:
        @decorate
        class Box:
            value: int
            values = [lambda: item for item in (1,)]
        escaped.append(Box)
    return Box
""",
    }

    def test_outer_finally_class_reemission_ordinary_control(self):
        for name, body in self.OUTER_FINALLY_SOURCES.items():
            with self.subTest(case=name):
                module = self.ordinary_module(body)
                decorated = []

                def decorate(cls):
                    decorated.append(cls)
                    return cls

                def fail():
                    raise ValueError("the original outer failure")

                classes = []
                for exceptional in (False, True):
                    escaped = []
                    arguments = [fail if exceptional else lambda: None, escaped]
                    if name == "decorated":
                        arguments.append(decorate)
                    if exceptional:
                        with self.assertRaisesRegex(ValueError, "original outer failure"):
                            module.build(*arguments)
                    else:
                        cls = module.build(*arguments)
                        self.assertIs(cls, escaped[0])
                    self.assertEqual(len(escaped), 1)
                    cls = escaped[0]
                    classes.append(cls)
                    self.assertEqual(cls.values[0](), 1)
                    if name == "plain":
                        self.assertIs(cls().read(), cls)
                    else:
                        self.assertEqual(cls.__annotate__(1), {"value": int})
                self.assertIsNot(classes[0], classes[1])
                self.assertIsNot(classes[0].values[0].__closure__[0],
                                 classes[1].values[0].__closure__[0])
                self.assertEqual(decorated, classes if name == "decorated" else [])

    def test_outer_finally_class_reemission_keeps_exact_retained_parent_tree(self):
        for name, body in self.OUTER_FINALLY_SOURCES.items():
            with self.subTest(case=name):
                _, _, nodes, (recipe,) = self.compile_source(body)
                class_node = nodes[recipe[0]]
                parent_node = nodes[class_node[1]]
                native_children = [item for item in parent_node[2].co_consts
                                   if type(item) is types.CodeType]
                self.assertEqual(len(native_children), 1)
                self.assertIs(native_children[0], class_node[2])
                self.assertEqual(len(recipe[3]), 1)
                children = [node for node in nodes if node[1] == recipe[0]]
                expected_captures = {(node[0], ordinal) for node in children
                                     for ordinal in range(len(node[2].co_freevars))}
                self.assertEqual({(row[0], row[2]) for row in recipe[4]}, expected_captures)
                self.assertTrue(expected_captures)

    def test_source_name_access_modes_and_augassign_have_exact_original_spans(self):
        import ast

        _, _, _, (plain,) = self.compile_source(self.SOURCES["plain_target"])
        self.assertTrue(any(row[2] == 0 for row in plain[5]))
        body = """
def build():
    réponse = 3
    class Box:
        nonlocal réponse
        réponse += 2
        del réponse
    return Box
"""
        source, _, nodes, (recipe,) = self.compile_source(body)
        names, kinds = self.code_slots(nodes[recipe[0]][2])
        slots = [index for index, kind in enumerate(kinds) if kind == 0x80]
        self.assertEqual(len(slots), 1)
        slot = slots[0]
        self.assertEqual(names[slot], "réponse")
        class_ast = next(node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.ClassDef))
        augmented = class_ast.body[1].target
        deleted = class_ast.body[2].targets[0]
        def span(node):
            return (node.lineno, node.col_offset, node.end_lineno, node.end_col_offset)
        # The source target occurs twice with distinct contexts. Header names
        # and the nonlocal declaration must not manufacture original Name rows.
        self.assertCountEqual(recipe[5], [
            (span(augmented), 0, 2, (0, slot), None),
            (span(augmented), 1, 1, (0, slot), None),
            (span(deleted), 2, 1, (0, slot), None),
        ])

    def test_exceptional_restore_keeps_escaped_transient_and_class_cell_distinct(self):
        body = """
def build(marker, fail):
    escaped = []
    class Box:
        try:
            values = [(escaped.append(lambda: __class__), fail())[1]
                      for __class__ in (marker,)]
        except ValueError:
            pass
        def read(self):
            return __class__
    return Box, escaped[0]
"""
        _, _, _, (recipe,) = self.compile_source(body)

        def fail():
            raise ValueError("restore the actual saved class cell")

        marker = object()
        cls, escaped = self.ordinary_module(body).build(marker, fail)
        self.assertIs(escaped(), marker)
        self.assertIs(cls().read(), cls)
        self.assertIsNot(self.closure_cell(escaped, "__class__"),
                         self.closure_cell(vars(cls)["read"], "__class__"))

    def test_decorated_repeated_and_generic_class_origins_are_exact(self):
        body = """
def decorate(cls):
    return cls
def build(marker):
    @decorate
    class Box:
        values = [lambda: item for item in (marker,)]
    first = Box
    @decorate
    class Box[T]:
        values = [lambda: item for item in (T,)]
        field: T
    return first, Box
"""
        source, _, nodes, recipes = self.compile_source(body)
        classes = [nodes[recipe[0]] for recipe in recipes]
        self.assertEqual(len(classes), 2)
        self.assertEqual([node[2].co_name for node in classes], ["Box", "Box"])
        self.assertNotEqual(classes[0][5], classes[1][5])
        lines = source.decode().splitlines()
        for node in classes:
            line, column, _, _ = node[5]
            self.assertTrue(lines[line - 1][column:].startswith("class Box"))
            self.assertLess(node[2].co_firstlineno, line)
        self.assertTrue(any(node[4] == 5 for node in nodes))
        self.assertTrue(any(node[4] == 3 for node in nodes))

    def test_metadata_changes_no_ordinary_code_and_grants_no_execution(self):
        source, root, nodes, _ = self.compile_source(self.SOURCES["class_cell"])
        ordinary = compile(source, "<native-class-bindings>", "exec", dont_inherit=True)

        def compare(left, right):
            for attribute in ("co_code", "co_flags", "co_names", "co_varnames",
                              "co_cellvars", "co_freevars", "co_linetable",
                              "co_exceptiontable", "co_name", "co_qualname"):
                self.assertEqual(getattr(left, attribute), getattr(right, attribute))
            self.assertEqual(len(left.co_consts), len(right.co_consts))
            for a, b in zip(left.co_consts, right.co_consts, strict=True):
                if type(a) is types.CodeType:
                    compare(a, b)
                else:
                    self.assertEqual(a, b)

        compare(root, ordinary)
        self.assertEqual(self.source_id(ordinary), 0)
        for code in (root, root.replace(), marshal.loads(marshal.dumps(root))):
            with self.assertRaisesRegex(self.runtime_error, "strict.*execution"):
                exec(code, {})
        for node in nodes:
            self.assertEqual(self.source_id(node[2].replace()), 0)


    def test_scope_seeds_cover_actual_parameters_defaults_variadics_and_unused_slots(self):
        import ast

        body = """
def complete(positional, /, ordinary=1, *arguments, keyword=2, **mapping):
    def capture():
        return positional
    return [item for item in arguments]
def empty():
    return 1
choice = lambda used, unused=3: [item for item in used]
"""
        source, _, nodes, recipes, _ = self.compile_scopes(body)
        tree = ast.parse(source)
        node, recipe = self.source_scope(nodes, recipes, tree.body[1], 2)
        names, kinds = self.code_slots(node[2])
        expected = ("positional", "ordinary", "keyword", "arguments", "mapping")
        self.assertEqual(names[:5], expected)
        self.assertEqual([row[3] for row in recipe[1] if row[2] == 1], list(range(5)))
        self.assertEqual(tuple(names[row[0]] for row in recipe[1] if row[2] == 1), expected)
        self.assertTrue(kinds[0] & 0x40)
        entry, = [owner for owner in recipe[2] if owner[1] == 0 and owner[2] == 0]
        self.assertEqual(entry[3], kinds[0])
        # The seed describes successful argument binding, not a stored
        # default value. Even unused parameters keep slots.
        self.assertTrue(all(row[2:] == (0, None) for row in recipe[1][5:]))
        _, empty = self.source_scope(nodes, recipes, tree.body[2], 2)
        self.assertEqual(empty[1], ())
        _, choice = self.source_scope(nodes, recipes, tree.body[3].value, 4)
        self.assertEqual([row[3] for row in choice[1] if row[2] == 1], [0, 1])

    def test_scope_shadowed_parameter_and_native_cleared_target_have_distinct_seeds(self):
        import ast

        source, _, nodes, recipes, _ = self.compile_scopes("""
def shadowed(item, values):
    result = [item for item in values]
    return item, result
def fresh(values):
    return [item for item in values]
""")
        for original, expected_seed in zip(ast.parse(source).body[1:], (1, 0), strict=True):
            node, recipe = self.source_scope(nodes, recipes, original, 2)
            names, _ = self.code_slots(node[2])
            slot = names.index("item")
            self.assertEqual(recipe[1][slot][2], expected_seed)
            region, = recipe[3]
            saved, = [operation for operation in region[6] if operation[0] == 0]
            self.assertEqual(saved[1], slot)
            self.assertEqual(recipe[2][saved[2]][1:], (2, slot, recipe[1][slot][1], region[0]))

    def test_scope_function_lambda_and_dict_walrus_keep_original_binding_roles(self):
        import ast

        source, _, nodes, recipes, tables = self.compile_scopes("""
def projection(values):
    c = 0
    result = {__: (c := __ + 1) for __ in values}
    return c, result
list_scope = lambda values: [item for item in values]
set_scope = lambda values: {item for item in values}
""")
        tree = ast.parse(source)
        original = tree.body[1]
        node, recipe = self.source_scope(nodes, recipes, original, 2)
        names, _ = self.code_slots(node[2])
        comp = original.body[1].value
        region, = recipe[3]
        self.assertEqual((region[2], region[3], region[4]), (2, self.source_span(comp), self.source_span(comp.generators[0].iter)))
        target = comp.generators[0].target
        walrus = comp.value.target
        target_origin = (0, self.source_span(target), 0, None)
        walrus_origin = (0, self.source_span(walrus), 0, None)
        target_row, = [row for row in region[7] if row[2] == target_origin]
        walrus_row, = [row for row in region[7] if row[2] == walrus_origin]
        self.assertEqual(target_row[:2], (0, 0))
        self.assertEqual(walrus_row[:2], (1, None))
        self.assertEqual(names[target_row[4][1]], "__")
        self.assertEqual(names[walrus_row[4][1]], "c")
        self.assertNotEqual(target_row[4][1], walrus_row[4][1])
        self.assertIn(target_origin, {origin for origin, _ in tables[node[0]][4]})
        self.assertIn(walrus_origin, {origin for origin, _ in tables[node[0]][4]})
        for assignment, kind in zip(tree.body[2:], (0, 1), strict=True):
            _, recipe = self.source_scope(nodes, recipes, assignment.value, 4)
            self.assertEqual(recipe[3][0][2], kind)

    def test_scope_captured_targets_keep_owned_cells_with_or_without_outer_handler(self):
        import ast

        source, _, nodes, recipes, _ = self.compile_scopes("""
def plain(values):
    return [lambda: (left, right) for left in values for right in values]
def guarded(values):
    try:
        return [lambda: item for item in values]
    except MemoryError:
        return None
""")
        for original in ast.parse(source).body[1:]:
            node, recipe = self.source_scope(nodes, recipes, original, 2)
            region, = recipe[3]
            names, kinds = self.code_slots(node[2])
            saved = {slot: owner for operation, slot, owner in region[6] if operation == 0}
            fresh = {slot: owner for operation, slot, owner in region[6] if operation == 1}
            self.assertEqual(set(saved), set(fresh))
            self.assertTrue(fresh)
            self.assertTrue(set(saved.values()).isdisjoint(fresh.values()))
            self.assertEqual({names[slot] for slot in fresh},
                             {"left", "right"} if original.name == "plain" else {"item"})
            self.assertTrue(all(kinds[slot] & 0x40 for slot in fresh))
            self.assertEqual({edge[3][1] for edge in recipe[4]}, set(fresh))
            self.assertTrue(all(edge[4] == region[0] for edge in recipe[4]))

    def test_scope_cleanup_keeps_original_target_and_exact_lexical_restore(self):
        import ast

        source, _, nodes, recipes, tables = self.compile_scopes(
            "def cleanup(values): return [item for item in values]\n")
        node, recipe = self.source_scope(nodes, recipes, ast.parse(source).body[1], 2)
        region, = recipe[3]
        names, _ = self.code_slots(node[2])
        saved, = [operation for operation in region[6] if operation[0] == 0]
        self.assertEqual(names[saved[1]], "item")
        target, = region[7]
        original = ast.parse(source).body[1].body[0].value
        self.assertEqual(region[3], self.source_span(original))
        self.assertEqual(target[:3], (0, 0, (0, self.source_span(original.generators[0].target), 0, None)))
        self.assertIn(target[2], dict(tables[node[0]][4]))

    def test_scope_free_walrus_uses_current_enclosing_cell_without_snapshot(self):
        import ast

        source, _, nodes, recipes, _ = self.compile_scopes("""
def outer(value, values):
    def inner():
        nonlocal value
        return [(value := item) for item in values]
    return inner
""")
        outer = ast.parse(source).body[1]
        node, recipe = self.source_scope(nodes, recipes, outer.body[0], 2)
        names, kinds = self.code_slots(node[2])
        slot = names.index("value")
        self.assertTrue(kinds[slot] & 0x80)
        self.assertEqual(recipe[1][slot][2:], (0, None))
        self.assertIn(names[slot], node[2].co_freevars)
        region, = recipe[3]
        walrus, = [row for row in region[7] if row[0] == 1]
        self.assertEqual(walrus[3:], (3, (0, slot)))
        self.assertNotIn(slot, {operation[1] for operation in region[6]})
        outer_node, outer_recipe = self.source_scope(nodes, recipes, outer, 2)
        self.assertEqual({(edge[0], edge[2]) for edge in outer_recipe[4]},
                         {(node[0], ordinal) for ordinal in range(len(node[2].co_freevars))})
        self.assertIsNone(outer_recipe[6])
        self.assertIsNone(recipe[6])

    def test_scope_nested_and_zero_iteration_regions_retain_real_parent_and_seed(self):
        import ast

        source, _, nodes, recipes, _ = self.compile_scopes("""
def nested(values):
    return [[lambda: (outer, inner) for inner in (outer,)] for outer in values]
def empty(item):
    return [item for item in ()], item
""")
        nested, empty = ast.parse(source).body[1:]
        _, nested_recipe = self.source_scope(nodes, recipes, nested, 2)
        self.assertEqual([region[1] for region in nested_recipe[3]], [None, 0])
        node, empty_recipe = self.source_scope(nodes, recipes, empty, 2)
        slot = self.code_slots(node[2])[0].index("item")
        self.assertEqual(empty_recipe[1][slot][2:], (1, slot))
        region, = empty_recipe[3]
        self.assertTrue(region[7])

    def test_scope_finally_copies_keep_one_original_lexical_region(self):
        import ast

        body = """
def copied(flag, values):
    try:
        if flag:
            return 1
        return 2
    finally:
        kept = [lambda: item for item in values]
"""
        # Both source return forms retain the same original finally scope.
        cases = (
            ("constant", body),
            ("evaluated", body.replace("return 1", "return flag").replace("return 2", "return values")),
        )
        for case, source_body in cases:
            with self.subTest(case=case):
                source, root, nodes, recipes, _ = self.compile_scopes(source_body)
                original = ast.parse(source).body[1]
                _, recipe = self.source_scope(nodes, recipes, original, 2)
                region, = recipe[3]
                original_comp = original.body[0].finalbody[0].value
                self.assertEqual(region[3], self.source_span(original_comp))
                self.assertEqual(region[4], self.source_span(original_comp.generators[0].iter))
                saved, = [operation for operation in region[6] if operation[0] == 0]
                self.assertEqual(len(recipe[4]), 1)
                child = nodes[recipe[4][0][0]]
                self.assertEqual(child[1], recipe[0])
                self.assertEqual(child[5], self.source_span(original_comp.elt))
                ordinary = compile(source, "<native-class-bindings>", "exec", dont_inherit=True)
                self.assert_ordinary_code_equal(root, ordinary)

    def test_scope_one_element_inner_iterable_keeps_original_generator_bindings(self):
        import ast

        source, _, nodes, recipes, _ = self.compile_scopes(
            "def singleton(values): return [(left, right) for left in values for right in (left,)]\n")
        node, recipe = self.source_scope(nodes, recipes, ast.parse(source).body[1], 2)
        region, = recipe[3]
        original = ast.parse(source).body[1].body[0].value
        targets = [row for row in region[7] if row[0] == 0]
        self.assertCountEqual([(row[1], row[2]) for row in targets],
                              [(ordinal, (0, self.source_span(generator.target), 0, None))
                               for ordinal, generator in enumerate(original.generators)])
        names, _ = self.code_slots(node[2])
        self.assertEqual({names[row[4][1]] for row in targets}, {"left", "right"})

    def test_scope_async_and_generator_sources_keep_semantic_flags(self):
        import ast

        source, _, nodes, recipes, _ = self.compile_scopes("""
async def asynchronous(values):
    return [item async for item in values]
def suspended(values):
    yield [item for item in values]
""")
        asynchronous, suspended = ast.parse(source).body[1:]
        node, recipe = self.source_scope(nodes, recipes, asynchronous, 3)
        region, = recipe[3]
        self.assertEqual(region[5], 1)
        self.assertTrue(node[2].co_flags & 0x80)
        self.assertEqual(region[3], self.source_span(asynchronous.body[0].value))
        node, recipe = self.source_scope(nodes, recipes, suspended, 2)
        self.assertEqual(recipe[3][0][5], 0)
        self.assertTrue(node[2].co_flags & 0x20)
        self.assertEqual(recipe[3][0][3], self.source_span(suspended.body[0].value.value))

    def test_scope_setter_targets_do_not_invent_private_snapshot_owners(self):
        import ast

        source, _, nodes, recipes, _ = self.compile_scopes("""
def setters(holder, values):
    return [0 for holder.value in values], [0 for holder[0] in values]
""")
        _, recipe = self.source_scope(nodes, recipes, ast.parse(source).body[1], 2)
        self.assertEqual(len(recipe[3]), 2)
        for region, form, domain in zip(recipe[3], (6, 7), (1, 2), strict=True):
            self.assertEqual(region[6], ())
            target, = region[7]
            self.assertEqual((target[0], target[1], target[3], target[4][0]), (0, 0, form, domain))

    def test_scope_metadata_preserves_ordinary_compiler_bytes_positions_and_tables(self):
        body = """
def layout(value, /, default=1, *items, flag=True, **keywords):
    def capture(): return value
    try:
        return [{item: (last := item) for item in items} for unused in (0,)]
    finally:
        cleanup = [lambda: item for item in items]
choice = lambda values: {item for item in values}
async def asynchronous(values):
    return [item async for item in values]
"""
        for optimize in (0, 1, 2):
            with self.subTest(optimize=optimize):
                source, root, nodes, recipes, _ = self.compile_scopes(body, optimize)
                ordinary = compile(source, "<native-class-bindings>", "exec", dont_inherit=True, optimize=optimize)
                self.assertEqual(len(recipes), len(nodes))
                self.assert_ordinary_code_equal(root, ordinary)
                self.assertEqual(self.source_id(ordinary), 0)



class ScopeRegionMetadataNativeTests(unittest.TestCase):
    """Original lexical bindings; no strict body or native schedule is executed."""

    RawPySoacCodeView = ClassBindingMetadataNativeTests.RawPySoacCodeView
    code_slots = ClassBindingMetadataNativeTests.code_slots
    assert_is_immutable = ClassBindingMetadataNativeTests.assert_is_immutable
    assert_ordinary_code_equal = ClassBindingMetadataNativeTests.assert_ordinary_code_equal
    assert_recipe = ClassBindingMetadataNativeTests.assert_recipe

    @classmethod
    def setUpClass(cls):
        ClassBindingMetadataNativeTests.setUpClass.__func__(cls)

    def scope_source(self, body):
        import ast

        source = ("from __future__ import strict\n" + body).encode()
        root, _, product = self.compile_details(
            source, len(source), "<native-scope-lifecycle>", -1)
        self.assert_is_immutable(product)
        version, nodes, recipes, operations = product
        self.assertEqual(version, 7)
        self.assertEqual([node[0] for node in nodes], list(range(len(nodes))))
        self.assertEqual([recipe[0] for recipe in recipes], list(range(len(nodes))))
        self.assertEqual([table[0] for table in operations], list(range(len(nodes))))
        self.assertIs(nodes[0][2], root)
        self.assertGreater(self.source_id(root), 0)
        for node in nodes:
            self.assertEqual(self.source_id(node[2]), self.source_id(root))
            self.assert_recipe(nodes, recipes[node[0]], operations[node[0]])
        return source, ast.parse(source), root, version, nodes, recipes, operations

    @staticmethod
    def source_span(node):
        return node.lineno, node.col_offset, node.end_lineno, node.end_col_offset

    def original_scope(self, data, original):
        _, _, _, _, nodes, recipes, operations = data
        matches = [node for node in nodes if node[5] == self.source_span(original)]
        self.assertEqual(len(matches), 1)
        node, = matches
        return node[2], recipes[node[0]], operations[node[0]]

    def assert_original_region(self, original, region):
        import ast

        self.assertEqual(len(region), 8)
        self.assertEqual(region[2], {ast.ListComp: 0, ast.SetComp: 1, ast.DictComp: 2}[type(original)])
        self.assertEqual(region[3], self.source_span(original))
        self.assertEqual(region[4], self.source_span(original.generators[0].iter))
        self.assertEqual(region[5], any(generator.is_async for generator in original.generators))
        # The unchanged subjects in this family use simple Name targets. Their
        # original positions and generator ordinals distinguish nested scopes.
        self.assertCountEqual(
            [(row[1], row[2]) for row in region[7] if row[0] == 0],
            [(ordinal, (0, self.source_span(generator.target), 0, None))
             for ordinal, generator in enumerate(original.generators)],
        )

    def check_one(self, body):
        data = self.scope_source(body)
        original = data[1].body[1]
        _, recipe, operations = self.original_scope(data, original)
        region, = recipe[3]
        return data, original, recipe, operations, region

    def test_scope_region_discard_keeps_original_target_scope(self):
        _, original, _, _, region = self.check_one(
            "def build(values):\n    [item for item in values]\n    return None\n")
        self.assert_original_region(original.body[0].value, region)

    def test_scope_region_publish_keeps_exact_original_name_store(self):
        for binding in ("result = [item for item in values]",
                        "result: list = [item for item in values]"):
            with self.subTest(binding=binding):
                _, original, recipe, operations, region = self.check_one(
                    "def build(values):\n    " + binding + "\n    return result\n")
                statement = original.body[0]
                target = statement.targets[0] if hasattr(statement, "targets") else statement.target
                origin = (0, self.source_span(target), 0, None)
                stores = dict(operations[4])
                self.assertIn(origin, stores)
                self.assertTrue(stores[origin])
                self.assert_original_region(statement.value, region)
                # Publishing the result is an enclosing binding, not another
                # isolated target or a saved comprehension owner.
                self.assertNotIn(origin, {row[2] for row in region[7]})
                self.assertTrue(all(owner[4] in (None, region[0]) for owner in recipe[2]))

    def test_scope_region_return_keeps_original_result_expression(self):
        _, original, _, _, region = self.check_one(
            "def build(values):\n    return [item for item in values]\n")
        self.assert_original_region(original.body[0].value, region)
        saved, = [operation for operation in region[6] if operation[0] == 0]

    def test_scope_region_captured_target_has_distinct_entry_fresh_and_saved_owners(self):
        data, original, recipe, _, region = self.check_one(
            "def build(values):\n    [lambda: item for item in values]\n    return None\n")
        self.assert_original_region(original.body[0].value, region)
        saved, = [operation for operation in region[6] if operation[0] == 0]
        fresh, = [operation for operation in region[6] if operation[0] == 1]
        self.assertEqual(saved[1], fresh[1])
        self.assertNotEqual(saved[2], fresh[2])
        entry, = [owner for owner in recipe[2] if owner[1] == 0 and owner[2] == saved[1]]
        self.assertNotIn(entry[0], (saved[2], fresh[2]))
        capture, = recipe[4]
        child = data[4][capture[0]]
        self.assertEqual(child[1], recipe[0])
        self.assertEqual(child[2].co_freevars, ("item",))
        self.assertEqual(capture[2:], (0, (0, saved[1]), region[0]))

    def test_scope_region_nested_original_scopes_are_not_flattened(self):
        data = self.scope_source(
            "def build(values):\n    return [[inner for inner in values] for outer in values]\n")
        original = data[1].body[1]
        _, recipe, _ = self.original_scope(data, original)
        self.assertEqual(len(recipe[3]), 2)
        outer, = [region for region in recipe[3] if region[1] is None]
        inner, = [region for region in recipe[3] if region[1] == outer[0]]
        self.assert_original_region(original.body[0].value, outer)
        self.assert_original_region(original.body[0].value.elt, inner)
        self.assertTrue({owner for operation, _, owner in outer[6] if operation == 0}.isdisjoint(
            owner for operation, _, owner in inner[6] if operation == 0))

    def test_scope_region_finally_returns_share_only_the_same_original_scope(self):
        data = self.scope_source(
            "def build(values, flag):\n    try:\n        if flag:\n            return 1\n"
            "        return 2\n    finally:\n        [item for item in values]\n")
        original = data[1].body[1]
        _, recipe, _ = self.original_scope(data, original)
        region, = recipe[3]
        self.assert_original_region(original.body[0].finalbody[0].value, region)
        self.assertEqual(len({owner[0] for owner in recipe[2]}), len(recipe[2]))

    def test_scope_region_finally_fallthrough_keeps_original_scope(self):
        data = self.scope_source(
            "def build(values, flag):\n    try:\n        if flag:\n            return 1\n"
            "    finally:\n        [item for item in values]\n    return 2\n")
        original = data[1].body[1]
        _, recipe, _ = self.original_scope(data, original)
        region, = recipe[3]
        self.assert_original_region(original.body[0].finalbody[0].value, region)

    def test_scope_region_unreachable_source_needs_no_lifecycle_emission(self):
        data = self.scope_source(
            "def build(values):\n    return None\n    [item for item in values]\n")
        original = data[1].body[1]
        _, recipe, _ = self.original_scope(data, original)
        region, = recipe[3]
        self.assert_original_region(original.body[1].value, region)

    def test_scope_region_large_expression_keeps_exact_source_target(self):
        values = ", ".join("item" for _ in range(200))
        _, original, _, _, region = self.check_one(
            "def build(values):\n    return [(" + values + ") for item in values]\n")
        self.assert_original_region(original.body[0].value, region)
        self.assertEqual(len(original.body[0].value.elt.elts), 200)

    def test_scope_region_chained_and_async_sources_need_no_completion_schedule(self):
        for body in (
            "def build(values):\n    first = second = [item for item in values]\n    return first\n",
            "async def build(values):\n    return [item async for item in values]\n",
        ):
            with self.subTest(body=body):
                data = self.scope_source(body)
                original = data[1].body[1]
                _, recipe, operations = self.original_scope(data, original)
                region, = recipe[3]
                statement = original.body[0]
                self.assert_original_region(statement.value, region)
                if hasattr(statement, "targets"):
                    stores = dict(operations[4])
                    for target in statement.targets:
                        self.assertIn((0, self.source_span(target), 0, None), stores)

    def test_scope_region_collection_changes_no_original_code_or_execution_grant(self):
        data = self.scope_source(
            "def build(values):\n    result = [lambda: item for item in values]\n    return result\n")
        self.assertEqual(data[3], 7)
        original = compile(data[0], "<native-scope-lifecycle>", "exec", dont_inherit=True)
        self.assert_ordinary_code_equal(data[2], original)
        self.assertEqual(self.source_id(original), 0)
        for code in (data[2], data[2].replace(), marshal.loads(marshal.dumps(data[2]))):
            with self.assertRaises(self.runtime_error):
                exec(code, {})


class FutureAnnotationMetadataNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.compile_details = native_api(
            "PySoac_CompileVerifiedSourceDetails",
            ctypes.py_object,
            ctypes.c_char_p,
            ctypes.c_ssize_t,
            ctypes.py_object,
            ctypes.c_int,
        )
        cls.source_id = native_api(
            "PyCode_GetSoacStrictSourceId", ctypes.c_uint64, ctypes.py_object
        )
        cls.setup_annotations = native_api(
            "PySoac_SetupAnnotations", ctypes.c_int, ctypes.py_object
        )
        cls.runtime_error = borrowed_object_api(
            "PySoac_GetStrictRuntimeUnavailableError"
        )

    def test_native_strings_cover_exact_ranges_and_optimized_away_annotations(self):
        import dis

        source = '''from __future__ import strict, annotations
ordinary: list["Thing"]
if False:
    hidden: tuple["Thing", ...]
def method(positional: int, /, argument: str = "") -> "Thing":
    inside: 1 + 2
    return None
class Example:
    déjà: tuple["é", int]
    def variadic(self, *args: *tuple[int, str]):
        pass
'''.encode()
        code, rows, _ = self.compile_details(source, len(source), "<native-strings>", 2)
        self.assertIs(type(code), types.CodeType)
        self.assertIs(type(rows), tuple)
        lines = source.splitlines(keepends=True)
        line_starts = [0]
        for line in lines:
            line_starts.append(line_starts[-1] + len(line))
        by_position = {}
        by_source = {}
        for row in rows:
            self.assertIs(type(row), tuple)
            self.assertEqual(len(row), 5)
            start_line, start_column, end_line, end_column, text = row
            self.assertIs(type(text), str)
            position = (start_line, end_line, start_column, end_column)
            self.assertNotIn(position, by_position)
            by_position[position] = text
            start = line_starts[start_line - 1] + start_column
            end = line_starts[end_line - 1] + end_column
            by_source[source[start:end].decode()] = text
        self.assertEqual(by_source['tuple["Thing", ...]'], "tuple['Thing', ...]")
        self.assertEqual(by_source["1 + 2"], "1 + 2")
        self.assertEqual(by_source['tuple["é", int]'], "tuple['é', int]")
        self.assertEqual(by_source["*tuple[int, str]"], "*tuple[int, str]")
        pending = [code]
        matched = set()
        while pending:
            current = pending.pop()
            self.assertEqual(self.source_id(current), self.source_id(code))
            self.assertGreater(self.source_id(current), 0)
            for instruction in dis.get_instructions(current):
                position = tuple(instruction.positions)
                if instruction.opname == "LOAD_CONST" and position in by_position:
                    self.assertEqual(instruction.argval, by_position[position])
                    matched.add(position)
            pending.extend(
                value for value in current.co_consts if isinstance(value, types.CodeType)
            )
        self.assertGreater(len(matched), 3)
        self.assertLess(len(matched), len(rows))

    def test_all_statement_suites_are_scanned_without_stringizing_type_evaluators(self):
        source = b'''from __future__ import strict, annotations
type Alias[Parameter: Bound = Default] = list[Parameter]
for item in ():
    for_body: "for_body"
else:
    for_else: "for_else"
while False:
    while_body: "while_body"
else:
    while_else: "while_else"
with manager():
    with_body: "with_body"
try:
    try_body: "try_body"
except Exception:
    except_body: "except_body"
else:
    try_else: "try_else"
finally:
    try_finally: "try_finally"
try:
    star_body: "star_body"
except* Exception:
    star_handler: "star_handler"
else:
    star_else: "star_else"
finally:
    star_finally: "star_finally"
match value:
    case 0:
        match_body: "match_body"
async def coroutine(argument: "argument") -> "result":
    async for item in stream:
        async_for: "async_for"
    else:
        async_else: "async_else"
    async with manager():
        async_with: "async_with"
'''
        _, rows, _ = self.compile_details(source, len(source), "<native-suites>", 2)
        expected = {
            "for_body", "for_else", "while_body", "while_else", "with_body",
            "try_body", "except_body", "try_else", "try_finally", "star_body",
            "star_handler", "star_else", "star_finally", "match_body", "argument",
            "result", "async_for", "async_else", "async_with",
        }
        self.assertEqual({row[-1] for row in rows}, {repr(value) for value in expected})
        self.assertEqual(len(rows), len(expected))

    def test_metadata_does_not_authorize_original_or_copied_code_execution(self):
        source = b"from __future__ import strict, annotations\ndef value(arg: int): return arg\n"
        root, rows, _ = self.compile_details(source, len(source), "<native-strings>", -1)
        self.assertTrue(rows)
        with self.assertRaisesRegex(self.runtime_error, "strict.*execution"):
            exec(root, {})
        child = next(
            value for value in root.co_consts
            if isinstance(value, types.CodeType) and value.co_name == "value"
        )
        for code in (child, child.replace(), marshal.loads(marshal.dumps(child))):
            with self.assertRaisesRegex(self.runtime_error, "strict.*execution"):
                types.FunctionType(code, {})(1)
        self.assertEqual(self.source_id(child.replace()), 0)

    def test_nonfuture_source_has_no_string_metadata_and_errors_stay_native(self):
        for source in (
            b"from __future__ import strict\nvalue: int\n",
            b"# soac: module(checked_attr=true)\nvalue: int\n",
            b"value: int\n",
        ):
            with self.subTest(source=source):
                code, rows, _ = self.compile_details(
                    source, len(source), "<native-strings>", -1
                )
                self.assertGreater(self.source_id(code), 0)
                self.assertEqual(rows, ())
                # Authenticated compilation marks the code independently of
                # comment syntax; it never authorizes an unowned execution.
                with self.assertRaisesRegex(self.runtime_error, "strict.*execution"):
                    exec(code, {})
        for source, error in (
            (b"from __future__ import strict\ndef broken(:\n", SyntaxError),
            (b"from __future__ import strict\n\x00", ValueError),
        ):
            with self.subTest(source=source), self.assertRaises(error):
                self.compile_details(source, len(source), "<native-strings>", -1)

    def test_details_perform_one_source_parse_without_python_ast_callbacks(self):
        script = '''
import ast, ctypes, json, sys
events = []
def audit(event, arguments):
    if event == "compile" and arguments[1] == "<native-single-parse>":
        events.append(event)
sys.addaudithook(audit)
ast.unparse = lambda value: (_ for _ in ()).throw(AssertionError("mutable AST helper"))
api = ctypes.pythonapi.PySoac_CompileVerifiedSourceDetails
api.argtypes = [ctypes.c_char_p, ctypes.c_ssize_t, ctypes.py_object, ctypes.c_int]
api.restype = ctypes.py_object
source = b"from __future__ import strict, annotations\\nvalue: list[int]\\n"
code, rows, _ = api(source, len(source), "<native-single-parse>", -1)
assert len(events) == 1, events
assert rows[0][-1] == "list[int]", rows
print(json.dumps(events))
'''
        completed = subprocess.run(
            [sys.executable, "-I", "-S", "-B", "-c", script],
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertEqual(json.loads(completed.stdout), ["compile"])

    def test_setup_annotations_preserves_existing_bindings_and_mapping_protocol(self):
        namespace = {}
        self.assertEqual(self.setup_annotations(namespace), 0)
        annotations = namespace["__annotations__"]
        self.assertIs(type(annotations), dict)
        self.assertEqual(self.setup_annotations(namespace), 0)
        self.assertIs(namespace["__annotations__"], annotations)
        namespace["__annotations__"] = None
        self.assertEqual(self.setup_annotations(namespace), 0)
        self.assertIsNone(namespace["__annotations__"])

        class Mapping:
            def __init__(self):
                self.values = {}
                self.events = []

            def __getitem__(self, key):
                self.events.append(("get", key))
                return self.values[key]

            def __setitem__(self, key, value):
                self.events.append(("set", key))
                self.values[key] = value

        mapping = Mapping()
        self.assertEqual(self.setup_annotations(mapping), 0)
        self.assertEqual(mapping.events, [("get", "__annotations__"), ("set", "__annotations__")])
        mapping.events.clear()
        self.assertEqual(self.setup_annotations(mapping), 0)
        self.assertEqual(mapping.events, [("get", "__annotations__")])
        self.assertIs(type(mapping.values["__annotations__"]), dict)

    def test_setup_annotations_propagates_lookup_and_write_errors_without_fallback(self):
        error = RuntimeError("annotation namespace lookup")
        writes = []

        class Mapping:
            def __getitem__(self, key):
                raise error

            def __setitem__(self, key, value):
                writes.append(value)

        with self.assertRaises(RuntimeError) as caught:
            self.setup_annotations(Mapping())
        self.assertIs(caught.exception, error)
        self.assertEqual(writes, [])

        class WriteFailure:
            def __getitem__(self, key):
                raise KeyError(key)

            def __setitem__(self, key, value):
                raise error

        with self.assertRaises(RuntimeError) as caught:
            self.setup_annotations(WriteFailure())
        self.assertIs(caught.exception, error)
        with self.assertRaises(TypeError):
            self.setup_annotations(17)


def exercise_metadata_reentry(outer, nested, authority):
    setter = native_api(
        "PyFunction_SetSoacMetadata", ctypes.c_int, ctypes.py_object,
        ctypes.c_uint64, ctypes.c_void_p, ctypes.c_void_p,
    )
    getter = native_api(
        "PyFunction_GetSoacMetadata", ctypes.c_void_p, ctypes.py_object,
    )
    owned_getter = native_api(
        "PyFunction_GetSoacMetadataForDestructorV1", ctypes.c_void_p,
        ctypes.py_object, ctypes.c_void_p,
    )
    function_id = native_api(
        "PyFunction_GetSoacFunctionId", ctypes.c_uint64, ctypes.py_object,
    )
    get_owner = native_api(
        "PyFunction_GetSoacStrictOwner", ctypes.c_void_p, ctypes.py_object,
    )
    strict_id = native_api(
        "PyFunction_GetSoacStrictId", ctypes.c_uint64, ctypes.py_object,
    )
    source_id = native_api(
        "PyCode_GetSoacStrictSourceId", ctypes.c_uint64, ctypes.py_object,
    )
    allocate = native_api("PyMem_Malloc", ctypes.c_void_p, ctypes.c_size_t)
    release = native_api("PyMem_Free", None, ctypes.c_void_p)

    def function(value):
        return value

    code = function.__code__
    owner = object()
    if authority != "ordinary":
        assert native_api(
            "PyFunction_SetSoacStrictOwner", ctypes.c_int,
            ctypes.py_object, ctypes.py_object,
        )(function, owner) == 0
        if authority == "sealed":
            assert native_api(
                "PyFunction_SealSoacStrict", ctypes.c_int,
                ctypes.py_object, ctypes.c_uint64,
            )(function, 97021) == 0

    def contract_state():
        return {
            "owner_intact": get_owner(function) == (
                None if authority == "ordinary" else id(owner)
            ),
            "seal": strict_id(function),
            "source": source_id(function.__code__),
            "same_code": function.__code__ is code,
            "unchecked_id": function_id(function),
        }

    addresses = {}
    magic = {"A": 0xA1, "B": 0xB2, "C": 0xC3}
    counts = {name: 0 for name in magic}
    live = set()
    callbacks = {}
    callback_errors = []
    callback_contracts = []
    reenter = [True]

    def install(name):
        if name is None:
            return setter(function, 0, None, None)
        return setter(function, 0, addresses[name], callbacks[name])

    def destructor_for(name):
        @ctypes.PYFUNCTYPE(None, ctypes.c_void_p)
        def destructor(address):
            # Do not let ctypes swallow assertion failures or double-free on a
            # broken runtime. Record the failure and reclaim each pointer once.
            try:
                counts[name] += 1
                assert address == addresses[name], (name, "wrong pointer")
                assert name in live, (name, "duplicate destructor")
                assert ctypes.c_uint64.from_address(address).value == magic[name]
                live.remove(name)
                release(address)
                callback_contracts.append(contract_state())
                if name == "A" and reenter[0]:
                    reenter[0] = False
                    assert install("B" if nested == "replace" else None) == 0
            except BaseException as error:
                callback_errors.append((name, type(error).__name__, str(error)))
        return destructor

    before = contract_state()
    try:
        for name in ("A", "B", "C"):
            if name == "B" and nested != "replace":
                continue
            if name == "C" and outer != "replace":
                continue
            address = allocate(ctypes.sizeof(ctypes.c_uint64))
            if not address:
                raise MemoryError("metadata test allocation")
            addresses[name] = address
            live.add(name)
            ctypes.c_uint64.from_address(address).value = magic[name]
            callbacks[name] = destructor_for(name)

        assert install("A") == 0
        assert install("C" if outer == "replace" else None) == 0
        current = getter(function)
        current_name = next(
            (name for name, address in addresses.items() if address == current),
            None if current is None else "unknown",
        )
        # Inspect only still-owned storage; never dereference a released pointer.
        valid = current is None or (
            current_name in live
            and ctypes.c_uint64.from_address(current).value == magic[current_name]
            and owned_getter(function, callbacks[current_name]) == current
        )
        after = contract_state()
        ordinary_result = True
        if authority == "ordinary":
            argument = object()
            ordinary_result = function(argument) is argument

        # Disarm reentry, then test the actual final association's cleanup.
        reenter[0] = False
        assert install(None) == 0
        result = {
            "case": [outer, nested, authority],
            "current": current_name,
            "current_valid": bool(valid),
            "destructors": dict(counts),
            "unreleased": sorted(live),
            "callback_errors": list(callback_errors),
            "contract_before": before,
            "contract_preserved": after == before and all(
                state == before for state in callback_contracts
            ) and contract_state() == before,
            "ordinary_result": ordinary_result,
            "cleared": getter(function) is None and function_id(function) == 0,
        }
    finally:
        reenter[0] = False
        install(None)
        # Preserve the observed leak in result; reclaim only after observing it.
        # All associations are detached before callback objects leave scope.
        for name in tuple(live):
            release(addresses[name])
            live.remove(name)
    return result

def metadata_reentry_failures(result):
    outer, nested, authority = result["case"]
    expected_counts = {
        "A": 1, "B": int(nested == "replace"), "C": int(outer == "replace"),
    }
    expected_contract = {
        "owner_intact": True,
        "seal": 97021 if authority == "sealed" else 0,
        "source": 0, "same_code": True, "unchecked_id": 0,
    }
    expected = {
        "current": "B" if nested == "replace" else None,
        "current_valid": True, "destructors": expected_counts,
        "unreleased": [], "callback_errors": [],
        "contract_before": expected_contract, "contract_preserved": True,
        "ordinary_result": True, "cleared": True,
    }
    return {
        key: {"actual": result[key], "expected": value}
        for key, value in expected.items() if result[key] != value
    }


class StrictCPythonNativeTests(unittest.TestCase):
    def test_reentrant_keyword_default_lookup_keeps_the_current_mapping_alive(self):
        # The ordinary CPython binder is the interoperability boundary. A
        # default-key equality callback may replace the mapping and code while
        # an earlier activation is still binding. Debug allocation makes the
        # otherwise heap-layout-dependent use-after-free deterministic.
        result = subprocess.run(
            [sys.executable, "-I", "-S", "-B", "-X", "dev", "-c", '''
import gc
import sys

def plain(*, value=1):
    return value
def stream(*, value=1):
    yield value
def replacement_plain(*, value=1):
    return value + 100
def replacement_stream(*, value=1):
    yield value + 100

for function, replacement, suspended in (
    (plain, replacement_plain, False),
    (stream, replacement_stream, True),
):
    held = []
    class Key:
        expected = function.__code__.co_varnames[0]
        def __hash__(self):
            return hash(self.expected)
        def __eq__(self, other):
            assert other is self.expected
            held.append(function(value=99))
            function.__kwdefaults__ = {'value': 20}
            function.__code__ = replacement.__code__
            return True

    function.__kwdefaults__ = {Key(): 7}
    value = function()
    assert (list(value) if suspended else value) == ([7] if suspended else 7)
    value = held.pop()
    assert (list(value) if suspended else value) == ([99] if suspended else 99)
    value = function()
    assert (list(value) if suspended else value) == ([120] if suspended else 120)
    gc.collect()

assert 'soac' not in sys.modules and 'soac._soac_ext' not in sys.modules
'''],
            check=False, capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_reentrant_code_change_uses_the_active_generator_frame(self):
        # Keep the defaults dictionary alive so this independently exercises
        # frame allocation and kind selection, not the mapping-lifetime bug.
        # RETURN_GENERATOR must use its own code even when argument binding
        # replaces the function's public code with a smaller or different kind.
        for kind in ("generator", "coroutine", "async_generator"):
            for change_kind in (False, True):
                with self.subTest(kind=kind, change_kind=change_kind):
                    result = subprocess.run(
                        [sys.executable, "-I", "-S", "-B", "-X", "dev", "-c", '''
import gc
import sys
import types

kind, change_kind = sys.argv[1], sys.argv[2] == 'True'
prefix = 'async ' if kind != 'generator' else ''
terminal = 'return' if kind == 'coroutine' else 'yield'
namespace = {'events': []}
exec(prefix + """def original(*, value=1):
    events.append(('body', value, object(), object()))
    """ + terminal + ' value', namespace)
if change_kind:
    exec('def replacement(*, value=1): return value + 100', namespace)
else:
    exec(prefix + 'def replacement(*, value=1): ' + terminal + ' value + 100', namespace)
function = namespace['original']
replacement = namespace['replacement']
assert function.__code__.co_stacksize > replacement.__code__.co_stacksize

class Key:
    expected = function.__code__.co_varnames[0]
    def __hash__(self):
        return hash(self.expected)
    def __eq__(self, other):
        assert other is self.expected
        function.__kwdefaults__ = {'value': 20}
        function.__code__ = replacement.__code__
        return True

retained_defaults = {Key(): 7}
function.__kwdefaults__ = retained_defaults
value = function()
assert type(value) is {
    'generator': types.GeneratorType,
    'coroutine': types.CoroutineType,
    'async_generator': types.AsyncGeneratorType,
}[kind]

def consume(value):
    if isinstance(value, types.GeneratorType):
        return list(value)
    if isinstance(value, types.AsyncGeneratorType):
        step = value.__anext__()
    else:
        step = value
    try:
        step.send(None)
    except StopIteration as result:
        answer = result.value
    else:
        raise AssertionError('the body unexpectedly suspended')
    if isinstance(value, types.AsyncGeneratorType):
        try:
            value.__anext__().send(None)
        except StopAsyncIteration:
            pass
        else:
            raise AssertionError('the async generator did not finish')
    return answer

assert consume(value) == ([7] if kind == 'generator' else 7)
assert namespace['events'][0][:2] == ('body', 7)
del value
gc.collect()
later = function()
assert (later if change_kind else consume(later)) == (
    [120] if kind == 'generator' and not change_kind else 120
)
del later
gc.collect()
assert 'soac' not in sys.modules and 'soac._soac_ext' not in sys.modules
''', kind, str(change_kind)],
                        check=False, capture_output=True, text=True, timeout=30,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)

    @classmethod
    def setUpClass(cls):
        cls.compile_verified = native_api(
            "PySoac_CompileVerifiedSource",
            ctypes.py_object,
            ctypes.c_char_p,
            ctypes.c_ssize_t,
            ctypes.py_object,
            ctypes.c_int,
        )
        cls.source_id = native_api(
            "PyCode_GetSoacStrictSourceId",
            ctypes.c_uint64,
            ctypes.py_object,
        )
        cls.seal = native_api(
            "PyFunction_SealSoacStrict",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.c_uint64,
        )
        cls.function_id = native_api(
            "PyFunction_GetSoacStrictId",
            ctypes.c_uint64,
            ctypes.py_object,
        )
        cls.mutation_error = borrowed_object_api("PySoac_GetStrictMutationError")
        cls.runtime_error = borrowed_object_api(
            "PySoac_GetStrictRuntimeUnavailableError"
        )

    @staticmethod
    def make_unowned_strict_function(source):
        ordinary = compile(
            source, "<strict-native-warmed-entry>", "exec", dont_inherit=True
        )
        child = next(
            item for item in ordinary.co_consts if isinstance(item, types.CodeType)
        )
        strict_child = child.replace(co_flags=child.co_flags | STRICT)
        creator = ordinary.replace(
            co_consts=tuple(
                strict_child if item is child else item for item in ordinary.co_consts
            )
        )
        namespace = {"events": []}
        exec(creator, namespace)
        return namespace[child.co_name], namespace["events"]

    def test_flag_is_real_but_never_mints_compiler_provenance(self):
        import __future__

        self.assertEqual(__future__.strict.compiler_flag, STRICT)
        source = b"from __future__ import strict\ndef value(): return 17\n"
        ordinary = compile(source, "<strict-native>", "exec", dont_inherit=True)
        nested = next(
            item for item in ordinary.co_consts if isinstance(item, types.CodeType)
        )
        self.assertTrue(ordinary.co_flags & STRICT)
        self.assertTrue(nested.co_flags & STRICT)
        self.assertEqual(self.source_id(ordinary), 0)
        self.assertEqual(self.source_id(nested), 0)
        explicit = compile(
            "value = 17", "<strict-native>", "exec", flags=STRICT, dont_inherit=True
        )
        self.assertEqual(self.source_id(explicit), 0)
        for code in (ordinary, explicit):
            with self.assertRaisesRegex(self.runtime_error, "strict.*execution"):
                exec(code, {})

    def test_native_error_classes_cannot_replace_constructor_or_base_metadata(self):
        absent = object()
        for error_type, base, replacement_base, getter in (
            (
                self.mutation_error,
                TypeError,
                RuntimeError,
                "PySoac_GetStrictMutationError",
            ),
            (
                self.runtime_error,
                ImportError,
                TypeError,
                "PySoac_GetStrictRuntimeUnavailableError",
            ),
        ):
            self.assertIs(error_type.__bases__[0], base)
            self.assertIs(borrowed_object_api(getter), error_type)
            old_bases = error_type.__bases__
            try:
                with self.assertRaises(TypeError):
                    error_type.__bases__ = (replacement_base,)
            finally:
                if error_type.__bases__ != old_bases:
                    error_type.__bases__ = old_bases
            for name, replacement in (
                ("__module__", "not_the_native_owner"),
                ("__new__", staticmethod(lambda cls, *args: None)),
                ("__init__", lambda self, *args: None),
            ):
                original = error_type.__dict__.get(name, absent)
                try:
                    with self.assertRaises(TypeError):
                        setattr(error_type, name, replacement)
                finally:
                    # Keep the unpatched-runtime regression run isolated from
                    # later tests even if this forbidden assignment succeeds.
                    current = error_type.__dict__.get(name, absent)
                    if current is not original:
                        if original is absent:
                            delattr(error_type, name)
                        else:
                            setattr(error_type, name, original)
            self.assertIsInstance(error_type("native"), base)

    def test_verified_compilation_marks_exact_tree_without_native_fallback(self):
        # The trusted caller resolves selection, including inherited policy
        # without a local directive. Compilation marks source, not an execution owner.
        for prefix, ordinary_strict_flag in (
            (b"", False),
            (b"# soac: module(strict_assign=true)\n", False),
            (b"from __future__ import strict\n", True),
        ):
            with self.subTest(prefix=prefix):
                source = prefix + b"def value(): return 17\n"
                ordinary = compile(
                    source, "<strict-native>", "exec", dont_inherit=True
                )
                self.assertEqual(self.source_id(ordinary), 0)
                self.assertEqual(bool(ordinary.co_flags & STRICT), ordinary_strict_flag)
                code = self.compile_verified(source, len(source), "<strict-native>", -1)
                nested = next(
                    item for item in code.co_consts if isinstance(item, types.CodeType)
                )
                identity = self.source_id(code)
                self.assertGreater(identity, 0)
                self.assertEqual(self.source_id(nested), identity)
                self.assertTrue(code.co_flags & STRICT)
                self.assertTrue(nested.co_flags & STRICT)
                with self.assertRaisesRegex(self.runtime_error, "strict.*execution"):
                    exec(code, {})
                call = types.FunctionType(nested, {})
                for _ in range(20):
                    with self.assertRaisesRegex(self.runtime_error, "strict.*execution"):
                        call()
                copied = nested.replace(co_flags=0)
                self.assertEqual(self.source_id(copied), 0)
                self.assertTrue(copied.co_flags & STRICT)
                with self.assertRaisesRegex(self.runtime_error, "strict.*execution"):
                    types.FunctionType(copied, {})()
                unmarshaled = marshal.loads(marshal.dumps(nested))
                self.assertEqual(self.source_id(unmarshaled), 0)
                self.assertTrue(unmarshaled.co_flags & STRICT)
                with self.assertRaises(self.runtime_error):
                    types.FunctionType(unmarshaled, {})()

    def test_warmed_make_function_rejects_unowned_strict_code_before_body(self):
        ordinary = compile(
            "def value():\n    events.append('executed')\n    return 17\n",
            "<strict-native-warmed-call>",
            "exec",
            dont_inherit=True,
        )
        plain = next(
            item for item in ordinary.co_consts if isinstance(item, types.CodeType)
        )
        source = (
            b"from __future__ import strict\n"
            b"def value():\n    events.append('executed')\n    return 17\n"
        )
        verified = self.compile_verified(
            source, len(source), "<strict-native-warmed-call>", -1
        )
        verified_child = next(
            item for item in verified.co_consts if isinstance(item, types.CodeType)
        )
        cases = (
            ("explicit flag", plain.replace(co_flags=plain.co_flags | STRICT)),
            ("authenticated source without an execution owner", verified_child),
            ("code.replace", verified_child.replace()),
            ("marshal", marshal.loads(marshal.dumps(verified_child))),
        )
        for description, child in cases:
            with self.subTest(description=description):
                # Unlike types.FunctionType, MAKE_FUNCTION assigns a valid
                # function version, allowing CALL_PY specialization. The
                # ordinary outer code grants no authority to its constants.
                creator = ordinary.replace(
                    co_consts=tuple(
                        child if item is plain else item for item in ordinary.co_consts
                    )
                )
                namespace = {"events": []}
                exec(creator, namespace)
                function = namespace["value"]
                for _ in range(128):
                    with self.assertRaises(self.runtime_error):
                        function()
                    self.assertEqual(namespace["events"], [])

        namespace = {"events": []}
        exec(ordinary, namespace)
        function = namespace["value"]
        for _ in range(128):
            self.assertEqual(function(), 17)
        self.assertEqual(namespace["events"], ["executed"] * 128)

    def test_warmed_argument_entries_reject_and_release_bound_values(self):
        function, events = self.make_unowned_strict_function(
            "def value(first, second=2, *, keyword=3):\n"
            "    events.append('executed')\n"
            "    return first\n"
        )
        released = []

        class Argument:
            def __del__(self):
                released.append("released")

        for invoke in (
            lambda: function(Argument()),
            lambda: function(Argument(), 2),
            lambda: function(Argument(), keyword=3),
            lambda: function(*(Argument(),), **{"keyword": 3}),
        ):
            for _ in range(64):
                before = len(released)
                with self.assertRaises(self.runtime_error):
                    invoke()
                self.assertEqual(len(released), before + 1)
                self.assertEqual(events, [])

    def test_warmed_bound_method_and_property_entries_reject_before_body(self):
        function, events = self.make_unowned_strict_function(
            "def value(self):\n    events.append('executed')\n    return 17\n"
        )
        receiver = type(
            "Receiver", (), {"method": function, "property": property(function)}
        )()
        for invoke in (lambda: receiver.method(), lambda: receiver.property):
            for _ in range(128):
                with self.assertRaises(self.runtime_error):
                    invoke()
                self.assertEqual(events, [])

    def test_warmed_suspended_frame_creation_rejects_without_execution_owner(self):
        for source in (
            "def value():\n    events.append('executed')\n    yield 17\n",
            "async def value():\n    events.append('executed')\n    return 17\n",
        ):
            with self.subTest(source=source):
                function, events = self.make_unowned_strict_function(source)
                for _ in range(128):
                    with self.assertRaises(self.runtime_error):
                        unexpected = function()
                        # Keep a before-fix regression run free of unawaited
                        # coroutine warnings if the denied creation succeeds.
                        unexpected.close()
                    self.assertEqual(events, [])

    def test_verified_compile_rejects_embedded_nul(self):
        for source in (
            b"\0value = 17\n",
            b"# soac: module(strict_assign=true)\n\0value = 17\n",
            b"from __future__ import strict\n\0value = 17\n",
        ):
            with self.subTest(source=source):
                with self.assertRaises((ValueError, SyntaxError)):
                    self.compile_verified(source, len(source), "<strict-native>", -1)

    def test_function_semantic_setters_reject_before_replacing_values(self):
        def value(argument=1, *, keyword=2):
            return argument, keyword

        original = value.__code__, value.__defaults__, value.__kwdefaults__
        self.assertEqual(self.seal(value, 71), 0)
        self.assertEqual(self.seal(value, 71), 0)
        with self.assertRaises(TypeError):
            self.seal(value, 72)
        for attribute, replacement in (
            ("__code__", (lambda: 3).__code__),
            ("__defaults__", (4,)),
            ("__kwdefaults__", {"keyword": 5}),
            ("__annotations__", {}),
            ("__annotate__", lambda format: {}),
            ("__type_params__", ()),
        ):
            with self.subTest(attribute=attribute):
                with self.assertRaises(TypeError):
                    setattr(value, attribute, replacement)
                with self.assertRaises(TypeError):
                    delattr(value, attribute)
        for api_name, replacement in (
            ("PyFunction_SetDefaults", (8,)),
            ("PyFunction_SetKwDefaults", {"keyword": 8}),
            ("PyFunction_SetAnnotations", {}),
            ("PyFunction_SetClosure", ()),
        ):
            api = native_api(api_name, ctypes.c_int, ctypes.py_object, ctypes.py_object)
            with self.subTest(api=api_name):
                with self.assertRaises(TypeError):
                    api(value, replacement)
        for actual, expected in zip(
            (value.__code__, value.__defaults__, value.__kwdefaults__), original
        ):
            self.assertIs(actual, expected)
        self.assertEqual(value(), (1, 2))
        self.assertEqual(self.function_id(value), 71)

    def test_direct_function_attribute_bytecode_cannot_bypass_the_native_seal(self):
        def value():
            return 17

        self.seal(value, 73)
        # A CodeType can supply this ordinary opcode without calling setattr.
        # The native SET_FUNCTION_ATTRIBUTE boundary must enforce the seal too.
        instructions = (
            ("RESUME", 0),
            ("LOAD_CONST", 1),
            ("LOAD_CONST", 2),
            ("SET_FUNCTION_ATTRIBUTE", 1),
            ("RETURN_VALUE", 0),
        )
        bytecode = bytes(
            part
            for name, argument in instructions
            for part in (opcode.opmap[name], argument)
        )
        template = (lambda: None).__code__
        code = template.replace(
            co_code=bytecode, co_consts=(None, (5,), value), co_stacksize=2
        )
        with self.assertRaises(self.mutation_error):
            types.FunctionType(code, {})()
        self.assertIsNone(value.__defaults__)
        self.assertEqual(value(), 17)

    def test_keyword_default_bindings_freeze_but_values_and_shared_dict_stay_real(self):
        shared = {"items": []}

        def first(*, items=None):
            return items

        def second(*, items=None):
            return items

        first.__kwdefaults__ = second.__kwdefaults__ = shared
        self.seal(first, 81)
        self.seal(second, 82)
        self.assertIs(first.__kwdefaults__, shared)
        self.assertIs(second.__kwdefaults__, shared)
        for mutate in (
            lambda: shared.__setitem__("items", [1]),
            lambda: shared.__setitem__("extra", 1),
            lambda: shared.__delitem__("items"),
            shared.clear,
            lambda: shared.update(items=[1]),
        ):
            with self.assertRaises(TypeError):
                mutate()
        shared["items"].append(3)
        self.assertIs(first(), shared["items"])
        self.assertEqual(second(), [3])

    def test_keyword_defaults_freeze_preserves_arbitrary_keys_and_lookup_hooks(self):
        observations = []

        class Alias:
            def __hash__(self):
                observations.append("hash")
                return hash("left")

            def __eq__(self, other):
                observations.append(("equal", other))
                return other == "left"

        alias = Alias()
        shared = {alias: 7, 42: 3, "right": 2}

        def function(*, left=1, right=2):
            return left, right

        function.__kwdefaults__ = shared
        original_keys = tuple(map(id, shared))
        observations.clear()
        self.seal(function, 83)
        self.assertEqual(observations, [])
        self.assertIs(function.__kwdefaults__, shared)
        self.assertEqual(tuple(map(id, shared)), original_keys)
        self.assertEqual(tuple(shared.values()), (7, 3, 2))
        self.assertEqual(function(), (7, 2))
        self.assertEqual(observations, [("equal", "left")])
        for mutate in (
            lambda: shared.__setitem__(alias, 9),
            lambda: shared.__setitem__("left", 9),
            lambda: shared.__delitem__(42),
            lambda: shared.update({"new": 9}),
            shared.clear,
        ):
            with self.assertRaises(self.mutation_error):
                mutate()
        self.assertEqual(tuple(map(id, shared)), original_keys)
        self.assertEqual(tuple(shared.values()), (7, 3, 2))

    def test_keyword_defaults_alias_does_not_freeze_an_ordinary_object_identity(self):
        class Holder:
            pass

        holder = Holder()
        holder.value = 7

        def function(*, value=1):
            return value

        shared = holder.__dict__
        function.__kwdefaults__ = shared
        self.seal(function, 84)
        with self.assertRaises(self.mutation_error):
            holder.value = 9
        replacement = {"value": 11}
        holder.__dict__ = replacement
        self.assertIs(holder.__dict__, replacement)
        self.assertIs(function.__kwdefaults__, shared)
        self.assertEqual((holder.value, function()), (11, 7))
        with self.assertRaises(self.mutation_error):
            shared["value"] = 13

    def test_terminal_keyword_default_alias_rejects_new_entries_not_bound_frames(self):
        check_defaults = native_api(
            "PyFunction_CheckSoacStrictDefaults", ctypes.c_int, ctypes.py_object
        )
        set_owner = native_api(
            "PyFunction_SetSoacStrictOwner",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
        )
        get_owner = native_api(
            "PyFunction_GetSoacStrictOwner", ctypes.c_void_p, ctypes.py_object
        )
        type_slot = native_api(
            "PyType_GetSlot", ctypes.c_void_p, ctypes.py_object, ctypes.c_int
        )
        clear_dictionary = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object)(
            type_slot(dict, 51)  # Py_tp_clear: real terminal unreachable-GC path.
        )

        class Value:
            pass

        module = types.ModuleType("strict_keyword_defaults_lifetime")
        module.value = Value()
        value_reference = weakref.ref(module.value)
        module_reference = weakref.ref(module)
        shared = vars(module)

        def function(*, value=None):
            yield value

        owner = object()
        set_owner(function, owner)
        function.__kwdefaults__ = shared
        self.seal(function, 85)
        self.assertEqual(check_defaults(function), 0)
        already_bound = function()
        del module
        self.assertIsNone(module_reference())
        self.assertEqual(clear_dictionary(shared), 0)
        self.assertEqual(shared, {})
        with self.assertRaises(self.runtime_error):
            check_defaults(function)
        with self.assertRaises(self.runtime_error):
            self.seal(function, 85)
        self.assertEqual(self.function_id(function), 85)
        # The separate new-entry check must not invalidate owner retrieval or
        # arguments already captured by a real suspended native frame.
        self.assertEqual(get_owner(function), id(owner))
        result = next(already_bound)
        self.assertIs(result, value_reference())
        already_bound.close()
        del result, already_bound
        self.assertIsNone(value_reference())

    def test_readonly_write_lookup_can_reenter_a_sealed_function(self):
        check_defaults = native_api(
            "PyFunction_CheckSoacStrictDefaults", ctypes.c_int, ctypes.py_object
        )
        observations = []
        collision_hash = 42 if hash("value") != 42 else 43

        def function(*, value=1):
            return value

        def observe(kind):
            self.assertEqual(check_defaults(function), 0)
            observations.append((kind, function()))

        class StoredKey:
            def __hash__(self):
                return collision_hash

            def __eq__(self, other):
                observe("equal")
                return False

        class IncomingKey:
            def __hash__(self):
                observe("hash")
                return collision_hash

        stored = StoredKey()
        shared = {"value": 7, stored: 9}
        function.__kwdefaults__ = shared
        self.seal(function, 86)
        self.assertEqual(observations, [])
        with self.assertRaises(self.mutation_error):
            shared[IncomingKey()] = 11
        self.assertEqual(observations, [("hash", 7), ("equal", 7)])
        self.assertEqual(tuple(shared.values()), (7, 9))

    def test_annotation_cache_stays_lazy_and_contents_remain_mutable(self):
        reads = []

        def value(argument: (reads.append("read"), int)[1]):
            return argument

        provider = value.__annotate__
        self.seal(value, 91)
        self.assertEqual(reads, [])
        self.assertIs(value.__annotate__, provider)
        annotations = value.__annotations__
        self.assertEqual(reads, ["read"])
        self.assertIs(value.__annotations__, annotations)
        annotations["argument"] = str
        self.assertIs(value.__annotations__["argument"], str)

    def test_closure_values_and_jit_metadata_are_independent_of_permanent_seal(self):
        state = 1

        def value(default=[]):
            return state, default

        self.seal(value, 101)
        value.__closure__[0].cell_contents = 2
        value.__defaults__[0].append(3)
        metadata = native_api(
            "PyFunction_SetSoacMetadata",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.c_uint64,
            ctypes.c_void_p,
            ctypes.c_void_p,
        )
        self.assertEqual(metadata(value, 0, None, None), 0)
        self.assertEqual(self.function_id(value), 101)
        self.assertEqual(value(), (2, [3]))


    def test_metadata_destructor_reentry_keeps_the_final_association_and_releases_each_payload(self):
        # These are trusted raw native storage controls, not authenticated source
        # execution: the original ordinary code keeps source ID zero throughout.
        for authority in ("ordinary", "owned", "sealed"):
            for outer, nested in (
                ("replace", "replace"),
                ("clear", "replace"),
                ("replace", "clear"),
            ):
                with self.subTest(outer=outer, nested=nested, authority=authority):
                    result = exercise_metadata_reentry(outer, nested, authority)
                    self.assertEqual(metadata_reentry_failures(result), {}, result)


class StrictStartupCaptureTests(unittest.TestCase):
    def run_python(self, *arguments, environment=None):
        return subprocess.run(
            [sys.executable, *arguments],
            text=True,
            capture_output=True,
            env=environment if environment is not None else os.environ.copy(),
            timeout=20,
        )

    def test_descriptor_bytes_and_path_are_captured_before_application_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "deployment.json"
            original = b'{"original":true}'
            path.write_bytes(original)
            program = (
                r"""
import pathlib, sys
pathlib.Path(sys.argv[1]).write_text('{"replacement":true}')
sys._xoptions["soac_strict_config"] = "/wrong/replacement"
"""
                + _READ_STARTUP_CONFIG
            )
            result = self.run_python(
                "-I", "-S", "-X", f"soac_strict_config={path}", "-c", program, str(path)
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), [original.decode(), str(path)])

    def test_descriptor_capture_precedes_sitecustomize(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "deployment.json"
            original = b'{"before_site":true}'
            path.write_bytes(original)
            (root / "sitecustomize.py").write_text(
                f"from pathlib import Path\nPath({str(path)!r}).write_text('{{}}')\n"
                "import sys\nsys._soac_test_site_ran = True\n"
            )
            environment = dict(os.environ, PYTHONPATH=str(root))
            program = (
                "import sys\nassert sys._soac_test_site_ran\n" + _READ_STARTUP_CONFIG
            )
            result = self.run_python(
                "-X",
                f"soac_strict_config={path}",
                "-c",
                program,
                environment=environment,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), [original.decode(), str(path)])

    def test_subinterpreter_copies_startup_snapshot_without_rereading_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "deployment.json"
            original = b'{"before_subinterpreter":true}'
            path.write_bytes(original)
            program = (
                "import _interpreters, pathlib\n"
                f"pathlib.Path({str(path)!r}).write_text('{{}}')\n"
                "identity = _interpreters.create()\n"
                "try:\n"
                f"    error = _interpreters.run_string(identity, {_READ_STARTUP_CONFIG!r})\n"
                "    assert error is None, error\n"
                "finally:\n"
                "    _interpreters.destroy(identity)\n"
            )
            result = self.run_python(
                "-I", "-S", "-X", f"soac_strict_config={path}", "-c", program
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), [original.decode(), str(path)])

    def test_bad_descriptor_framing_fails_before_application_runs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            empty = root / "empty"
            empty.write_bytes(b"")
            nul = root / "nul"
            nul.write_bytes(b"{\0}")
            large = root / "large"
            with large.open("wb") as stream:
                stream.truncate(16 * 1024 * 1024 + 1)
            for option in (
                "soac_strict_config",
                "soac_strict_config=relative.json",
                f"soac_strict_config={root / 'missing'}",
                f"soac_strict_config={empty}",
                f"soac_strict_config={nul}",
                f"soac_strict_config={large}",
                f"soac_strict_config={root}",
            ):
                with self.subTest(option=option):
                    result = self.run_python(
                        "-I", "-S", "-X", option, "-c", "print('APPLICATION RAN')"
                    )
                    self.assertNotEqual(result.returncode, 0)
                    self.assertNotIn("APPLICATION RAN", result.stdout)
                    self.assertIn("soac_strict_config", result.stderr)
            valid = root / "valid"
            valid.write_text("{}")
            result = self.run_python(
                "-I",
                "-S",
                "-X",
                f"soac_strict_config={valid}",
                "-X",
                f"soac_strict_config={valid}",
                "-c",
                "print('APPLICATION RAN')",
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("APPLICATION RAN", result.stdout)


class InterpreterPrefixNativeTests(unittest.TestCase):
    def test_prefix_comes_from_native_configuration_not_python_attributes(self):
        import _testcapi
        import _testinternalcapi

        prefix = native_api("PySoac_GetInterpreterPrefix", ctypes.c_wchar_p)
        expected = _testinternalcapi.get_configs()["config"]["prefix"]
        self.assertTrue(expected)
        self.assertEqual(prefix(), expected)
        original = sys.prefix
        try:
            sys.prefix = "/soac-test-spoofed-prefix"
            self.assertEqual(prefix(), expected)
            for replacement in ("/soac-test-config-set-prefix", None):
                _testcapi.config_set("prefix", replacement)
                self.assertEqual(sys.prefix, replacement)
                self.assertEqual(prefix(), expected)
        finally:
            _testcapi.config_set("prefix", original)

    def test_prefix_borrow_is_owned_by_the_current_subinterpreter(self):
        import _interpreters

        prefix = native_api("PySoac_GetInterpreterPrefix", ctypes.c_void_p)
        main_address = prefix()
        self.assertTrue(main_address)
        expected = ctypes.wstring_at(main_address)
        identity = _interpreters.create()
        try:
            error = _interpreters.run_string(
                identity,
                f"""
import ctypes, sys, _testcapi, _testinternalcapi
prefix = ctypes.pythonapi.PySoac_GetInterpreterPrefix
prefix.argtypes = []
prefix.restype = ctypes.c_void_p
address = prefix()
assert address and address != {main_address}, (address, {main_address})
assert ctypes.wstring_at(address) == {expected!r}
assert ctypes.wstring_at(address) == _testinternalcapi.get_configs()['config']['prefix']
sys.prefix = '/soac-test-subinterpreter-prefix'
_testcapi.config_set('prefix', '/soac-test-subinterpreter-config-prefix')
assert prefix() == address
assert ctypes.wstring_at(address) == {expected!r}
""",
            )
            self.assertIsNone(error, error)
        finally:
            _interpreters.destroy(identity)
        self.assertEqual(prefix(), main_address)
        self.assertEqual(ctypes.wstring_at(main_address), expected)


class StrictFunctionOwnerNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.set_owner = native_api(
            "PyFunction_SetSoacStrictOwner",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
        )
        cls.get_owner = native_api(
            "PyFunction_GetSoacStrictOwner", ctypes.c_void_p, ctypes.py_object
        )
        cls.seal = native_api(
            "PyFunction_SealSoacStrict",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.c_uint64,
        )
        cls.strict_id = native_api(
            "PyFunction_GetSoacStrictId", ctypes.c_uint64, ctypes.py_object
        )
        cls.mutation_error = borrowed_object_api("PySoac_GetStrictMutationError")
        cls.runtime_error = borrowed_object_api(
            "PySoac_GetStrictRuntimeUnavailableError"
        )

    def test_owner_and_seal_do_not_transfer_to_a_public_function_copy(self):
        def function(value: int) -> int:
            return value

        owner = object()
        self.assertIsNone(self.get_owner(function))
        self.assertEqual(self.strict_id(function), 0)
        self.set_owner(function, owner)
        self.assertEqual(self.get_owner(function), id(owner))
        self.assertEqual(self.strict_id(function), 0)
        self.seal(function, 600)
        self.assertEqual(self.strict_id(function), 600)
        self.assertEqual(function("ordinary value"), "ordinary value")
        clone = types.FunctionType(function.__code__, function.__globals__)
        self.assertIsNone(self.get_owner(clone))
        self.assertEqual(self.strict_id(clone), 0)
        self.assertEqual(clone("copied value"), "copied value")

    @staticmethod
    def install_noop_vectorcall(function):
        import _testinternalcapi

        def template():
            pass

        _testinternalcapi.set_vectorcall_nop(template)
        address = native_api(
            "PyVectorcall_Function", ctypes.c_void_p, ctypes.py_object
        )(template)
        native_api("PyFunction_SetVectorcall", None, ctypes.py_object, ctypes.c_void_p)(
            function, address
        )

    def test_constructor_specialization_honors_an_existing_custom_vectorcall(self):
        class Subject:
            def __init__(self):
                raise AssertionError("initializer bytecode bypassed its vectorcall")

        self.install_noop_vectorcall(Subject.__init__)
        for _ in range(256):
            self.assertIs(type(Subject()), Subject)

    def test_warmed_constructor_deopts_after_public_vectorcall_replacement(self):
        observations = []

        class Subject:
            def __init__(self):
                observations.append("bytecode")

        def construct():
            return Subject()

        for _ in range(256):
            construct()
        self.assertEqual(len(observations), 256)
        observations.clear()
        self.install_noop_vectorcall(Subject.__init__)
        for _ in range(256):
            construct()
        self.assertEqual(observations, [])

    def test_warmed_strict_initializer_uses_vectorcall_without_native_permission(self):
        compile_verified = native_api(
            "PySoac_CompileVerifiedSource",
            ctypes.py_object,
            ctypes.c_char_p,
            ctypes.c_ssize_t,
            ctypes.py_object,
            ctypes.c_int,
        )
        source = b"from __future__ import strict\ndef __init__(self): pass\n"
        module_code = compile_verified(source, len(source), "<strict-init>", -1)
        code = next(
            item for item in module_code.co_consts if isinstance(item, types.CodeType)
        )
        initializer = types.FunctionType(code, {})
        self.set_owner(initializer, object())
        self.seal(initializer, 607)
        self.install_noop_vectorcall(initializer)
        subject = type("StrictInitializerSubject", (), {"__init__": initializer})
        for _ in range(256):
            self.assertIs(type(subject()), subject)

        def native_template():
            pass

        native_vectorcall = native_api(
            "PyVectorcall_Function", ctypes.c_void_p, ctypes.py_object
        )(native_template)
        native_api("PyFunction_SetVectorcall", None, ctypes.py_object, ctypes.c_void_p)(
            initializer, native_vectorcall
        )
        with self.assertRaises(self.runtime_error):
            initializer(object.__new__(subject))

    def test_owner_attachment_leaves_metadata_mutable_until_sealing(self):
        def function(value=1, *, other=2):
            return value, other

        owner = object()
        self.set_owner(function, owner)
        original = function.__code__
        function.__code__ = original
        self.assertEqual(self.strict_id(function), 0)
        function.__defaults__ = (3,)
        function.__kwdefaults__ = {"other": 4}
        function.__kwdefaults__["other"] = 5
        self.assertEqual(function(), (3, 5))
        self.assertEqual(self.get_owner(function), id(owner))
        self.seal(function, 601)
        with self.assertRaises(self.mutation_error):
            function.__code__ = original
        with self.assertRaises(self.mutation_error):
            function.__defaults__ = (7,)

        ordinary = lambda value: value
        self.set_owner(ordinary, object())
        ordinary.__code__ = (lambda value: value + 1).__code__
        self.assertEqual(ordinary(2), 3)

    def test_sealed_code_guard_precedes_audit_and_watchers_for_python_and_c_setters(
        self,
    ):
        def function(value):
            return value

        original = function.__code__
        replacement = (lambda value: value + 1).__code__
        target = id(function)
        events = []
        audit_events = []
        listening = [True]

        def audit(event, arguments):
            if (
                listening[0]
                and event == "object.__setattr__"
                and id(arguments[0]) == target
            ):
                audit_events.append(arguments[1])

        sys.addaudithook(audit)

        @ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
        def watch(event, actual, new_value):
            if actual == target:
                events.append(event)
            return 0

        add_watcher = native_api("PyFunction_AddWatcher", ctypes.c_int, ctypes.c_void_p)
        clear_watcher = native_api(
            "PyFunction_ClearWatcher", ctypes.c_int, ctypes.c_int
        )
        c_setattr = native_api(
            "PyObject_SetAttrString",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.c_char_p,
            ctypes.py_object,
        )
        watcher = add_watcher(ctypes.cast(watch, ctypes.c_void_p))
        try:
            # A same-code assignment really does notify ordinary CPython
            # watchers; the full metadata seal must reject it before that event.
            function.__code__ = original
            self.assertEqual(events, [2])
            self.assertEqual(audit_events, ["__code__"])
            owner = object()
            self.set_owner(function, owner)
            self.seal(function, 603)
            events.clear()
            audit_events.clear()
            for code in (original, replacement):
                with self.assertRaises(self.mutation_error):
                    function.__code__ = code
                with self.assertRaises(self.mutation_error):
                    c_setattr(function, b"__code__", code)
                self.assertIs(function.__code__, original)
            self.assertEqual(events, [])
            self.assertEqual(audit_events, [])
            self.assertEqual(function(17), 17)
        finally:
            listening[0] = False
            clear_watcher(watcher)

    def test_audit_callbacks_cannot_write_past_new_function_restrictions(self):
        replacements = (
            ("__code__", (lambda value=1, *, keyword=2: 99).__code__),
            ("__defaults__", (9,)),
            ("__kwdefaults__", {"keyword": 9}),
            ("__defaults__", None),
            ("__kwdefaults__", None),
        )
        seal = self.seal
        for attribute, replacement in replacements:
            with self.subTest(attribute=attribute, deletion=replacement is None):
                function = lambda value=1, *, keyword=2: (value, keyword)
                original = getattr(function, attribute)
                target = id(function)
                listening = [True]
                events = []

                def audit(event, arguments, *, listening=listening,
                          target=target, attribute=attribute, events=events):
                    if (listening[0]
                        and event in ("object.__setattr__", "object.__delattr__")
                        and id(arguments[0]) == target
                        and arguments[1] == attribute):
                        events.append(event)
                        assert seal(arguments[0], 9501) == 0

                sys.addaudithook(audit)
                try:
                    with self.assertRaises(self.mutation_error):
                        if replacement is None:
                            delattr(function, attribute)
                        else:
                            setattr(function, attribute, replacement)
                    self.assertEqual(len(events), 1)
                    self.assertIs(getattr(function, attribute), original)
                    self.assertEqual(function(), (1, 2))
                finally:
                    listening[0] = False

    def test_function_watchers_cannot_write_past_new_seals(self):
        add_watcher = native_api("PyFunction_AddWatcher", ctypes.c_int, ctypes.c_void_p)
        clear_watcher = native_api("PyFunction_ClearWatcher", ctypes.c_int, ctypes.c_int)
        seal = self.seal
        operations = (
            ("__code__", 2, (lambda value=1, *, keyword=2: 99).__code__, None),
            ("__defaults__", 3, (9,), None),
            ("__kwdefaults__", 4, {"keyword": 9}, None),
            ("__defaults__", 3, (9,), "PyFunction_SetDefaults"),
            ("__kwdefaults__", 4, {"keyword": 9}, "PyFunction_SetKwDefaults"),
        )
        for attribute, expected_event, replacement, api_name in operations:
            with self.subTest(attribute=attribute, api=api_name):
                function = lambda value=1, *, keyword=2: (value, keyword)
                original = getattr(function, attribute)
                target = id(function)
                events = []

                @ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.c_int,
                                  ctypes.c_void_p, ctypes.c_void_p)
                def watch(event, actual, new_value):
                    if actual == target and event == expected_event:
                        events.append(event)
                        assert seal(ctypes.cast(actual, ctypes.py_object).value, 9502) == 0
                    return 0

                watcher = add_watcher(ctypes.cast(watch, ctypes.c_void_p))
                try:
                    setter = None if api_name is None else native_api(
                        api_name, ctypes.c_int, ctypes.py_object, ctypes.py_object,
                    )
                    references = sys.getrefcount(replacement)
                    with self.assertRaises(self.mutation_error):
                        if setter is None:
                            setattr(function, attribute, replacement)
                        else:
                            setter(function, replacement)
                    self.assertEqual(sys.getrefcount(replacement), references)
                    self.assertEqual(events, [expected_event])
                    self.assertIs(getattr(function, attribute), original)
                    self.assertEqual(function(), (1, 2))
                finally:
                    clear_watcher(watcher)

    def test_native_create_capture_observes_only_exact_globals_and_name(self):
        import _testinternalcapi

        captures = []
        environment = {}
        code = (lambda: None).__code__.replace(co_name="observed")
        watcher = _testinternalcapi.soac_function_create_watch(
            environment, "observed", captures,
        )
        try:
            actual = types.FunctionType(code, environment)
            unrelated_globals = types.FunctionType(code, {})
            unrelated_name = types.FunctionType(code.replace(co_name="other"), environment)
            self.assertEqual(captures, [actual])
            self.assertIsNone(unrelated_globals())
            self.assertIsNone(unrelated_name())
            with self.assertRaises(ValueError):
                _testinternalcapi.soac_function_create_watch(environment, "observed", [])
        finally:
            _testinternalcapi.soac_function_create_unwatch(watcher)
        reference = weakref.ref(actual)
        del actual
        self.assertIsNotNone(reference())
        captures.clear()
        self.assertIsNone(reference())
        with self.assertRaises(ValueError):
            _testinternalcapi.soac_function_create_unwatch(watcher)

    def test_native_create_capture_preserves_traced_error_unwind(self):
        import _testinternalcapi

        def trace(frame, event, argument):
            return trace

        original = ZeroDivisionError("original error")

        def fail():
            raise original

        captures = []
        watcher = _testinternalcapi.soac_function_create_watch(
            globals(), "never_created", captures,
        )
        previous_trace = sys.gettrace()
        caught = None
        try:
            sys.settrace(trace)
            try:
                # The temporary function dies with the error still pending.
                # A ctypes callback would enter traced Python even for this
                # DESTROY event; the C-only observer must leave it untouched.
                (lambda: None, fail())
            except ZeroDivisionError as error:
                caught = error
        finally:
            sys.settrace(previous_trace)
            _testinternalcapi.soac_function_create_unwatch(watcher)
        self.assertIs(caught, original)
        self.assertEqual(captures, [])

    def test_function_constructor_cannot_write_after_create_watcher_seals(self):
        add_watcher = native_api("PyFunction_AddWatcher", ctypes.c_int, ctypes.c_void_p)
        clear_watcher = native_api("PyFunction_ClearWatcher", ctypes.c_int, ctypes.c_int)
        seal = self.seal
        ordinary = lambda value, *, keyword: (value, keyword)
        captured = (lambda value: lambda: value)(19)
        cases = (
            (ordinary.__code__, {"argdefs": (23,)}, "__defaults__"),
            (ordinary.__code__, {"kwdefaults": {"keyword": 23}}, "__kwdefaults__"),
            (captured.__code__, {"closure": captured.__closure__}, "__closure__"),
            (ordinary.__code__, {}, None),
        )
        for code, arguments, attribute in cases:
            with self.subTest(attribute=attribute):
                observed = []

                @ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.c_int,
                                  ctypes.c_void_p, ctypes.c_void_p)
                def watch(event, actual, new_value):
                    if event == 0:
                        function = ctypes.cast(actual, ctypes.py_object).value
                        if function.__code__ is code:
                            observed.append(function)
                            assert seal(function, 9506) == 0
                    return 0

                watcher = add_watcher(ctypes.cast(watch, ctypes.c_void_p))
                try:
                    if attribute is None:
                        result = types.FunctionType(code, {}, **arguments)
                        self.assertIs(result, observed[0])
                    else:
                        with self.assertRaises(self.mutation_error):
                            types.FunctionType(code, {}, **arguments)
                        self.assertIsNone(getattr(observed[0], attribute))
                    self.assertEqual(len(observed), 1)
                    self.assertEqual(self.strict_id(observed[0]), 9506)
                finally:
                    clear_watcher(watcher)
                unsealed = types.FunctionType(code, {}, **arguments)
                self.assertEqual(self.strict_id(unsealed), 0)
                if attribute is not None:
                    self.assertIs(getattr(unsealed, attribute), next(iter(arguments.values())))

    def test_code_warning_callback_cannot_write_past_new_restrictions(self):
        import warnings

        def generator():
            yield 9

        function = lambda: 7
        original = function.__code__
        owner = object()
        self.set_owner(function, owner)
        events = []

        def warning(*args, **kwargs):
            events.append(args[0])
            self.seal(function, 9503)

        with warnings.catch_warnings():
            warnings.simplefilter("always", DeprecationWarning)
            warnings.showwarning = warning
            with self.assertRaises(self.mutation_error):
                function.__code__ = generator.__code__
        self.assertEqual(len(events), 1)
        self.assertIs(function.__code__, original)
        self.assertEqual(function(), 7)

    def test_annotation_finalizers_cannot_clear_metadata_after_sealing(self):
        seal = self.seal
        function = lambda: None
        events = []

        class Provider:
            def __call__(self, format):
                return {"original": int}

            def __del__(self):
                events.append("provider released")
                assert seal(function, 9504) == 0

        function.__annotate__ = Provider()
        original_cache = function.__annotations__
        replacement = lambda format: {"replacement": str}
        with self.assertRaises(self.mutation_error):
            function.__annotate__ = replacement
        # The first store happened before the finalizer sealed the function.
        # Preserve that visible progress and release order, but do not perform
        # the companion cache clear after the irreversible seal.
        self.assertEqual(events, ["provider released"])
        self.assertIs(function.__annotate__, replacement)
        self.assertIs(function.__annotations__, original_cache)

    def test_annotation_cache_finalizers_cannot_clear_provider_after_sealing(self):
        seal = self.seal
        for api_name in (None, "PyFunction_SetAnnotations"):
            with self.subTest(api=api_name):
                function = lambda: None
                events = []

                class CachedValue:
                    def __del__(self):
                        events.append("cache released")
                        assert seal(function, 9505) == 0

                provider = lambda format: {"value": CachedValue()}
                function.__annotate__ = provider
                function.__annotations__  # The function alone owns this cache.
                replacement = {"replacement": str}
                setter = None if api_name is None else native_api(
                    api_name, ctypes.c_int, ctypes.py_object, ctypes.py_object,
                )
                with self.assertRaises(self.mutation_error):
                    if setter is None:
                        function.__annotations__ = replacement
                    else:
                        setter(function, replacement)
                self.assertEqual(events, ["cache released"])
                self.assertIs(function.__annotations__, replacement)
                self.assertIs(function.__annotate__, provider)

    def test_owner_and_seal_stay_terminal_after_gc_clear_without_marking_new_functions(self):
        function = lambda value: value
        owner = object()
        self.set_owner(function, owner)
        self.seal(function, 604)
        code = function.__code__
        get_slot = native_api(
            "PyType_GetSlot", ctypes.c_void_p, ctypes.py_object, ctypes.c_int
        )
        clear = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object)(
            get_slot(types.FunctionType, 51)
        )
        self.assertEqual(clear(function), 0)
        with self.assertRaises(self.runtime_error):
            self.set_owner(function, owner)
        with self.assertRaises(self.runtime_error):
            self.get_owner(function)
        self.assertEqual(self.strict_id(function), 604)
        with self.assertRaises(self.mutation_error):
            function.__code__ = code
        for ordinary in (lambda value: value, types.FunctionType(code, {})):
            ordinary.__code__ = (lambda value: value + 1).__code__
            self.assertEqual(ordinary(3), 4)
            self.assertIsNone(self.get_owner(ordinary))

    def test_owner_is_single_assignment_and_independent_of_python_or_jit_metadata(self):
        function = lambda: 17
        owner = object()
        self.assertIsNone(self.get_owner(function))
        self.assertEqual(self.set_owner(function, owner), 0)
        self.assertEqual(self.get_owner(function), id(owner))
        self.assertEqual(self.set_owner(function, owner), 0)
        with self.assertRaises(self.mutation_error):
            self.set_owner(function, object())
        clear_owner = ctypes.PYFUNCTYPE(
            ctypes.c_int, ctypes.py_object, ctypes.c_void_p
        )(ctypes.cast(self.set_owner, ctypes.c_void_p).value)
        with self.assertRaises(self.mutation_error):
            clear_owner(function, None)
        self.assertEqual(self.seal(function, 901), 0)
        self.assertEqual(self.set_owner(function, owner), 0)
        function.__dict__["soac_strict_owner"] = object()
        metadata = native_api(
            "PyFunction_SetSoacMetadata",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.c_uint64,
            ctypes.c_void_p,
            ctypes.c_void_p,
        )
        self.assertEqual(metadata(function, 0, None, None), 0)
        self.assertEqual(self.get_owner(function), id(owner))
        self.assertEqual(self.strict_id(function), 901)
        self.assertEqual(function(), 17)

    def test_sealed_function_cannot_acquire_a_first_owner_later(self):
        function = lambda: 17
        self.seal(function, 902)
        with self.assertRaises(self.mutation_error):
            self.set_owner(function, object())
        self.assertIsNone(self.get_owner(function))

    def test_owner_function_cycle_is_visible_to_cpython_gc(self):
        class Owner:
            pass

        def make_cycle():
            function = lambda: 17
            owner = Owner()
            owner.function = function
            self.set_owner(function, owner)
            self.seal(function, 903)
            self.assertTrue(any(value is owner for value in gc.get_referents(function)))
            return weakref.ref(function), weakref.ref(owner)

        function, owner = make_cycle()
        gc.collect()
        self.assertIsNone(function())
        self.assertIsNone(owner())

    def test_native_gc_clear_marks_terminal_before_any_owned_reference_is_released(
        self,
    ):
        events = []
        get_owner = self.get_owner
        runtime_error = self.runtime_error

        class Observer:
            def __init__(self, function, label):
                self.function = function
                self.label = label

            def __del__(self):
                try:
                    get_owner(self.function)
                except runtime_error:
                    events.append((self.label, "terminal"))
                else:
                    events.append((self.label, "live"))

        function = lambda: 17
        function.__defaults__ = (Observer(function, "defaults"),)
        owner = Observer(function, "owner")
        self.set_owner(function, owner)
        self.seal(function, 904)
        del owner
        get_slot = native_api(
            "PyType_GetSlot", ctypes.c_void_p, ctypes.py_object, ctypes.c_int
        )
        # Py_tp_clear is stable slot 51 in Include/typeslots.h. This is an
        # intentionally privileged native fixture, not a Python teardown API.
        clear = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object)(
            get_slot(types.FunctionType, 51)
        )
        self.assertEqual(clear(function), 0)
        self.assertEqual(events, [("defaults", "terminal"), ("owner", "terminal")])
        with self.assertRaises(self.runtime_error):
            self.get_owner(function)
        with self.assertRaises(self.runtime_error):
            self.set_owner(function, object())
        with self.assertRaises(self.runtime_error):
            self.seal(function, 904)
        self.assertEqual(self.strict_id(function), 904)


class StrictModuleNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.mutation_error = borrowed_object_api("PySoac_GetStrictMutationError")
        cls.runtime_error = borrowed_object_api(
            "PySoac_GetStrictRuntimeUnavailableError"
        )

    @staticmethod
    def protect(module, *, mutable=(), seal=True):
        import _testcapi

        namespace = module.__dict__
        names = set(namespace) | {"__annotations__", "__annotate__", "late"}
        owner = _testcapi.dict_set_soac_policy(
            namespace, dict.fromkeys(names), names - set(mutable)
        )
        if seal:
            _testcapi.dict_seal_soac_namespace(namespace)
        return owner

    def test_module_wrapper_weakrefs_die_without_terminalizing_escaped_globals(self):
        for strict in (False, True):
            with self.subTest(strict=strict):
                module = types.ModuleType("module_wrapper_lifetime")
                namespace = module.__dict__
                owner = self.protect(module) if strict else None
                events = []

                def observe_teardown(reference):
                    events.append((reference(), owner.terminal if owner else False))
                    namespace["late"] = 17

                reference = weakref.ref(module, observe_teardown)
                del module
                self.assertIsNone(reference())
                self.assertEqual(events, [(None, False)])
                self.assertEqual(namespace["late"], 17)

    def test_gc_visible_function_owner_preserves_stock_module_wrapper_lifetime(self):
        set_owner = native_api(
            "PyFunction_SetSoacStrictOwner",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
        )
        seal = native_api(
            "PyFunction_SealSoacStrict",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.c_uint64,
        )

        class Owner:
            pass

        def make_module(strict):
            module = types.ModuleType("strict_function_globals_lifetime")
            exec("value = 17\ndef function(): return value", module.__dict__)
            function = module.function
            owner_reference = None
            if strict:
                owner = Owner()
                owner.policy = self.protect(module)
                owner.globals = module.__dict__
                self.assertEqual(set_owner(function, owner), 0)
                self.assertEqual(seal(function, 905), 0)
                self.assertTrue(
                    any(value is owner for value in gc.get_referents(function))
                )
                owner_reference = weakref.ref(owner)
            return function, weakref.ref(module), owner_reference

        for strict in (False, True):
            with self.subTest(strict=strict):
                function, module, owner = make_module(strict)
                self.assertIsNone(module())
                self.assertEqual(function(), 17)
                function.__globals__["late"] = 23
                self.assertEqual(function.__globals__["late"], 23)
                reference = weakref.ref(function)
                del function
                gc.collect()
                self.assertIsNone(reference())
                if owner is not None:
                    self.assertIsNone(owner())

    def test_native_class_descriptor_cannot_replace_protected_module_identity(self):
        class Replacement(types.ModuleType):
            pass

        module = types.ModuleType("strict_module_identity")
        namespace = module.__dict__
        self.protect(module, seal=False)
        for setter in (
            lambda: setattr(module, "__class__", Replacement),
            lambda: object.__setattr__(module, "__class__", Replacement),
            lambda: types.ModuleType.__setattr__(module, "__class__", Replacement),
            lambda: object.__dict__["__class__"].__set__(module, Replacement),
        ):
            with self.assertRaises(self.mutation_error):
                setter()
            self.assertIs(type(module), types.ModuleType)
            self.assertIs(module.__dict__, namespace)
        ordinary = types.ModuleType("ordinary_module_identity")
        ordinary.__class__ = Replacement
        self.assertIs(type(ordinary), Replacement)

    def test_supported_generic_dictionary_setter_preserves_authoritative_identity(self):
        class ModuleSubclass(types.ModuleType):
            pass

        set_dictionary = native_api(
            "PyObject_GenericSetDict",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
            ctypes.c_void_p,
        )
        for module_type in (types.ModuleType, ModuleSubclass):
            for seal in (False, True):
                with self.subTest(module_type=module_type, sealed=seal):
                    module = module_type("strict_module_native_dict")
                    namespace = module.__dict__
                    self.protect(module, seal=seal)
                    before = namespace.copy()
                    for replacement in ({}, namespace):
                        with self.assertRaises(self.mutation_error):
                            set_dictionary(module, replacement, None)
                        self.assertIs(module.__dict__, namespace)
                        self.assertEqual(namespace, before)

    def test_generic_dictionary_setter_preserves_ordinary_module_and_alias_controls(self):
        class ModuleSubclass(types.ModuleType):
            pass

        class ManagedInstance:
            pass

        class ExplicitDictionary:
            __slots__ = ("__dict__",)

        set_dictionary = native_api(
            "PyObject_GenericSetDict",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
            ctypes.c_void_p,
        )
        for module_type in (types.ModuleType, ModuleSubclass):
            with self.subTest(ordinary_module=module_type):
                module = module_type("ordinary_module_native_dict")
                replacement = {"value": 17}
                self.assertEqual(set_dictionary(module, replacement, None), 0)
                self.assertIs(module.__dict__, replacement)
                self.assertEqual(module.value, 17)

        module = types.ModuleType("strict_module_dictionary_alias")
        namespace = module.__dict__
        self.protect(module)
        for instance_type in (ManagedInstance, ExplicitDictionary):
            with self.subTest(ordinary_instance=instance_type):
                instance = instance_type()
                self.assertEqual(set_dictionary(instance, namespace, None), 0)
                self.assertIs(instance.__dict__, namespace)
                replacement = {"value": 23}
                self.assertEqual(set_dictionary(instance, replacement, None), 0)
                self.assertIs(instance.__dict__, replacement)
                self.assertEqual(instance.value, 23)
                self.assertIs(module.__dict__, namespace)
                # The fixture's binding validator raises ordinary TypeError.
                with self.assertRaisesRegex(TypeError, "immutable SOAC test binding"):
                    namespace["__name__"] = "cannot_thaw_authoritative_globals"

    def test_annotation_setter_rejects_before_appending_then_deleting_final_provider(
        self,
    ):
        module = types.ModuleType("strict_module_annotation_set")
        provider = lambda format: {"value": int}
        module.__annotate__ = provider
        self.protect(module)
        with self.assertRaises(TypeError):
            module.__annotations__ = {"replacement": str}
        self.assertNotIn("__annotations__", module.__dict__)
        self.assertIs(module.__dict__["__annotate__"], provider)

    def test_provider_setter_rejects_before_appending_then_deleting_final_cache(self):
        module = types.ModuleType("strict_module_provider_set")
        annotations = {"value": int}
        module.__annotations__ = annotations
        self.protect(module)
        with self.assertRaises(TypeError):
            module.__annotate__ = lambda format: {}
        self.assertNotIn("__annotate__", module.__dict__)
        self.assertIs(module.__dict__["__annotations__"], annotations)
        # None does not request the companion cache deletion in CPython.
        module.__annotate__ = None
        self.assertIsNone(module.__dict__["__annotate__"])
        self.assertIs(module.__dict__["__annotations__"], annotations)

    def test_compound_annotation_delete_preserves_both_bindings_on_failure(self):
        module = types.ModuleType("strict_module_annotation_delete")
        annotations = {"value": int}
        provider = lambda format: {}
        module.__dict__.update(__annotations__=annotations, __annotate__=provider)
        self.protect(module, mutable={"__annotations__"})
        with self.assertRaises(TypeError):
            del module.__annotations__
        self.assertIs(module.__dict__["__annotations__"], annotations)
        self.assertIs(module.__dict__["__annotate__"], provider)

    def test_sealed_compound_setters_do_not_claim_atomic_mutable_replacement(self):
        module = types.ModuleType("strict_module_mutable_metadata")
        annotations = {"value": int}
        module.__annotations__ = annotations
        self.protect(module, mutable={"__annotations__", "__annotate__"})
        with self.assertRaises(TypeError):
            module.__annotations__ = {"value": str}
        self.assertIs(module.__dict__["__annotations__"], annotations)
        # Ordinary one-key writes retain the explicitly mutable permission.
        replacement = {"value": str}
        module.__dict__["__annotations__"] = replacement
        self.assertIs(module.__annotations__, replacement)

    def test_initializing_compound_setters_preserve_sequential_finalizer_order(self):
        module = types.ModuleType("strict_module_initializing_metadata")
        events = []

        class OldAnnotations:
            def __del__(self):
                events.append(("primary", "__annotate__" in module.__dict__))

        class OldProvider:
            def __call__(self, format):
                return {}

            def __del__(self):
                events.append(("companion", "__annotate__" in module.__dict__))

        module.__dict__["__annotations__"] = OldAnnotations()
        module.__dict__["__annotate__"] = OldProvider()
        self.protect(module, mutable={"__annotations__", "__annotate__"}, seal=False)
        module.__annotations__ = {}
        self.assertEqual(events, [("primary", True), ("companion", False)])

    def test_module_lazy_annotation_getter_keeps_append_once_and_initializing_rules(
        self,
    ):
        module = types.ModuleType("strict_module_lazy_annotations")
        module.__spec__ = types.SimpleNamespace(_initializing=True)
        calls = []

        def provider(format):
            calls.append(format)
            return {"value": int}

        module.__annotate__ = provider
        self.protect(module)
        first = module.__annotations__
        self.assertNotIn("__annotations__", module.__dict__)
        self.assertIsNot(first, module.__annotations__)
        module.__spec__._initializing = False
        cached = module.__annotations__
        self.assertIs(module.__annotations__, cached)
        cached["value"] = str
        self.assertIs(module.__annotations__["value"], str)
        self.assertEqual(calls, [1, 1, 1])

        nested = types.ModuleType("strict_module_recursive_annotations")
        entered = False

        def recursive(format):
            nonlocal entered
            if not entered:
                entered = True
                nested.__annotations__
                return {"outer": int}
            return {"inner": str}

        nested.__annotate__ = recursive
        self.protect(nested)
        with self.assertRaises(TypeError):
            nested.__annotations__
        self.assertEqual(nested.__dict__["__annotations__"], {"inner": str})


class NativeCallContextTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.vectorcall = native_api(
            "PySoac_VectorcallWithContext",
            ctypes.py_object,
            ctypes.py_object,
            ctypes.POINTER(ctypes.py_object),
            ctypes.c_size_t,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
        )
        cls.object_call = native_api(
            "PySoac_ObjectCallWithContext",
            ctypes.py_object,
            ctypes.py_object,
            ctypes.py_object,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
        )

    def setUp(self):
        from collections import UserDict

        self.global_namespace = {"global_value": 17}
        self.local_namespace = UserDict({"local_value": 23})
        self.builtin_namespace = self.call.__func__.__builtins__

    def call(self, target, args=(), kwargs=None, *, with_locals=True, offset=False):
        kwargs = {} if kwargs is None else kwargs
        names = tuple(kwargs) if kwargs else None
        values = (None,) * int(offset) + tuple(args) + tuple(kwargs.values())
        storage = (ctypes.py_object * len(values))(*values)
        arguments = ctypes.cast(
            ctypes.byref(storage, int(offset) * ctypes.sizeof(ctypes.py_object)),
            ctypes.POINTER(ctypes.py_object),
        )
        nargsf = len(args)
        if offset:
            nargsf |= 1 << (8 * ctypes.sizeof(ctypes.c_size_t) - 1)
        return self.vectorcall(
            target,
            arguments,
            nargsf,
            None if names is None else id(names),
            id(self.global_namespace),
            id(self.local_namespace) if with_locals else None,
            id(self.builtin_namespace),
        )

    def test_zero_argument_builtins_return_the_actual_explicit_namespaces(self):
        import builtins

        for offset in (False, True):
            self.assertIs(
                self.call(builtins.globals, offset=offset), self.global_namespace
            )
            self.assertIs(
                self.call(builtins.locals, offset=offset), self.local_namespace
            )
            self.assertIs(self.call(builtins.vars, offset=offset), self.local_namespace)

    def test_globals_uses_explicit_context_without_a_local_namespace(self):
        import builtins

        self.assertIs(
            self.call(builtins.globals, with_locals=False), self.global_namespace
        )

    def test_dir_sorts_the_explicit_namespace_keys_without_using_object_dir(self):
        import builtins
        import weakref

        events = []

        class Namespace:
            def keys(self):
                events.append("keys")
                return ("zebra", "alpha")

            def __dir__(self):
                raise AssertionError("dir() must use local keys, not object attributes")

        self.local_namespace = Namespace()
        alias = builtins.dir
        for offset in (False, True):
            self.assertEqual(self.call(alias, offset=offset), ["alpha", "zebra"])
        for kwargs in (None, {}):
            result = self.object_call(
                alias,
                (),
                None if kwargs is None else id(kwargs),
                id(self.global_namespace),
                id(self.local_namespace),
                id(self.builtin_namespace),
            )
            self.assertEqual(result, ["alpha", "zebra"])
        self.assertEqual(events, ["keys"] * 4)

        # keys() may release the caller's last owning edge before its returned
        # iterable is consumed. The explicit context must remain alive until
        # PyMapping_Keys has finished, without retaining it after the call.
        class ReturnedKeys:
            def __iter__(keys):
                events.append(("mapping alive", mapping_ref() is not None))
                return iter(("zebra", "alpha"))

        class ReentrantNamespace:
            def keys(mapping):
                self.local_namespace = None
                return ReturnedKeys()

        self.local_namespace = ReentrantNamespace()
        mapping_ref = weakref.ref(self.local_namespace)
        self.assertEqual(self.call(alias), ["alpha", "zebra"])
        self.assertEqual(events[-1], ("mapping alive", True))
        self.assertIsNone(mapping_ref())

    def test_dir_preserves_key_callback_and_sort_errors(self):
        import builtins

        error = LookupError("explicit namespace keys failed")

        class Namespace:
            def keys(self):
                raise error

        self.local_namespace = Namespace()
        with self.assertRaises(LookupError) as caught:
            self.call(builtins.dir)
        self.assertIs(caught.exception, error)

        class UnsortableNamespace:
            def keys(self):
                return ("name", 1)

        self.local_namespace = UnsortableNamespace()
        with self.assertRaises(TypeError) as expected:
            sorted(self.local_namespace.keys())
        with self.assertRaises(TypeError) as actual:
            self.object_call(
                builtins.dir,
                (),
                None,
                id(self.global_namespace),
                id(self.local_namespace),
                id(self.builtin_namespace),
            )
        self.assertEqual(str(actual.exception), str(expected.exception))

    def test_vars_with_an_object_and_invalid_zero_argument_shapes_stay_native(self):
        import builtins

        value = types.SimpleNamespace(item=31)
        self.assertIs(self.call(builtins.vars, (value,)), vars(value))
        self.assertEqual(self.call(builtins.dir, (value,)), dir(value))
        for target, args, kwargs in (
            (builtins.globals, (1,), {}),
            (builtins.locals, (), {"unexpected": 1}),
            (builtins.vars, (1,), {}),
            (builtins.vars, (value, value), {}),
            (builtins.vars, (), {"object": value}),
            (builtins.dir, (value, value), {}),
            (builtins.dir, (), {"object": value}),
        ):
            with self.assertRaises(TypeError) as expected:
                target(*args, **kwargs)
            with self.assertRaises(TypeError) as actual:
                self.call(target, args, kwargs)
            self.assertEqual(str(actual.exception), str(expected.exception))

    def test_rebound_python_names_do_not_acquire_builtin_identity(self):
        observations = []

        def replacement(*args, **kwargs):
            observations.append((args, kwargs))
            return 41

        for name in ("locals", "globals", "vars", "dir", "eval", "exec", "compile"):
            replacement.__name__ = name
            self.assertEqual(self.call(replacement, (1,), {"value": 2}), 41)
        self.assertEqual(observations, [((1,), {"value": 2})] * 7)

    def test_nested_python_callbacks_do_not_inherit_an_ambient_context(self):
        def callback():
            nested_value = 53
            return locals()

        result = self.call(callback)
        self.assertEqual(result, {"nested_value": 53})
        self.assertIsNot(result, self.local_namespace)

    def test_inherited_dynamic_code_requires_an_explicit_protocol(self):
        import builtins

        for target, args in (
            (builtins.eval, ("global_value",)),
            (builtins.exec, ("changed = True",)),
            (builtins.compile, ("global_value", "<context>", "eval")),
        ):
            with self.assertRaises(NotImplementedError):
                self.call(target, args)

        # Valid ordinary strings execute normally in the stock control.
        # Rejecting their inherited SOAC form must not even publish builtins
        # into a caller-supplied empty target namespace.
        for target, source in (
            (builtins.eval, "40 + 2"),
            (builtins.exec, "created = True"),
        ):
            ordinary = {}
            result = target(source, ordinary)
            self.assertIn("__builtins__", ordinary)
            if target is builtins.eval:
                self.assertEqual(result, 42)
            else:
                self.assertIsNone(result)
                self.assertIs(ordinary["created"], True)
            for style in ("vectorcall", "offset", "object_call"):
                namespace = {}
                with self.subTest(target=target.__name__, style=style):
                    with self.assertRaisesRegex(
                        NotImplementedError, "authenticated dynamic-code protocol"
                    ):
                        if style == "object_call":
                            self.object_call(
                                target, (source, namespace), None,
                                id(self.global_namespace), None,
                                id(self.builtin_namespace),
                            )
                        else:
                            self.call(
                                target, (source, namespace), with_locals=False,
                                offset=style == "offset",
                            )
                    self.assertEqual(namespace, {})
        self.assertEqual(self.global_namespace, {"global_value": 17})
        self.assertEqual(dict(self.local_namespace), {"local_value": 23})

    def test_ordinary_dynamic_code_preserves_captured_builtins_across_audit_callbacks(self):
        import builtins

        def ordinary_call(target, code, namespace, keywords):
            return target(code, namespace, **keywords)

        def closure_factory(sink):
            def body():
                sink.append(abs(-1))
            return body

        captured_marker = object()
        captured = dict(vars(builtins), CONTEXT_MARKER=captured_marker)
        captured["abs"] = lambda value: captured_marker
        source_globals = {"__builtins__": captured}
        source = types.FunctionType(ordinary_call.__code__, source_globals)
        source_globals["__builtins__"] = dict(
            vars(builtins), CONTEXT_MARKER=object()
        )
        self.assertIs(source.__builtins__, captured)
        self.assertIsNot(source.__builtins__, source_globals["__builtins__"])
        self.global_namespace = source_globals
        self.builtin_namespace = source.__builtins__
        closure_results = []
        closure_function = closure_factory(closure_results)
        statements = (
            ("exec", builtins.exec, compile(
                "result = CONTEXT_MARKER", "<explicit-context-exec>", "exec",
                dont_inherit=True,
            ), {}),
            ("eval", builtins.eval, compile(
                "CONTEXT_MARKER", "<explicit-context-eval>", "eval",
                dont_inherit=True,
            ), {}),
            ("closure", builtins.exec, closure_function.__code__,
             {"closure": closure_function.__closure__}),
        )
        active = {}

        # Audit hooks are process-owned. Leave only this empty gating dictionary
        # after the test, never a captured function, namespace or exception.
        def audit(event, arguments):
            if event != "exec" or arguments[0] is not active.get("code"):
                return
            namespace = active["namespace"]
            active["events"].append(namespace["__builtins__"])
            action = active["action"]
            if action == "delete":
                del namespace["__builtins__"]
            elif action == "replace":
                namespace["__builtins__"] = active["replacement"]
            elif action == "raise":
                raise active["error"]

        sys.addaudithook(audit)
        try:
            for style in ("ordinary", "vectorcall", "offset", "object_call"):
                for operation, target, code, keywords in statements:
                    for preexisting in (False, True):
                        for action in ("keep", "delete", "replace", "raise"):
                            with self.subTest(
                                style=style, operation=operation,
                                preexisting=preexisting, action=action,
                            ):
                                existing_marker = object()
                                existing = dict(
                                    vars(builtins), CONTEXT_MARKER=existing_marker
                                )
                                existing["abs"] = lambda value: existing_marker
                                replacement_marker = object()
                                replacement = dict(
                                    vars(builtins), CONTEXT_MARKER=replacement_marker
                                )
                                replacement["abs"] = lambda value: replacement_marker
                                namespace = (
                                    {"__builtins__": existing} if preexisting else {}
                                )
                                events = []
                                closure_results.clear()
                                marker = ValueError("exec audit callback")
                                active.update(
                                    code=code, namespace=namespace, events=events,
                                    action=action, replacement=replacement,
                                    error=marker,
                                )

                                def invoke():
                                    if style == "ordinary":
                                        return source(target, code, namespace, keywords)
                                    if style == "object_call":
                                        return self.object_call(
                                            target, (code, namespace),
                                            id(keywords) if keywords else None,
                                            id(source_globals), None, id(captured),
                                        )
                                    return self.call(
                                        target, (code, namespace), keywords,
                                        with_locals=False,
                                        offset=style == "offset",
                                    )

                                if action == "raise":
                                    with self.assertRaises(ValueError) as raised:
                                        invoke()
                                    self.assertIs(raised.exception, marker)
                                    marker.__traceback__ = None
                                    self.assertNotIn("result", namespace)
                                    self.assertEqual(closure_results, [])
                                else:
                                    result = invoke()
                                    expected = (
                                        replacement_marker if action == "replace"
                                        else captured_marker if action == "delete"
                                        else existing_marker if preexisting
                                        else captured_marker
                                    )
                                    if operation == "closure":
                                        self.assertIsNone(result)
                                        self.assertEqual(closure_results, [expected])
                                    elif operation == "exec":
                                        self.assertIsNone(result)
                                        self.assertIs(namespace["result"], expected)
                                    else:
                                        self.assertIs(result, expected)
                                self.assertEqual(len(events), 1)
                                self.assertIs(events[0], existing if preexisting else captured)
                                if action == "delete":
                                    # Native execution reloads the post-audit mapping.
                                    # A missing entry uses the capture without rewriting it.
                                    self.assertNotIn("__builtins__", namespace)
                                elif action == "replace":
                                    self.assertIs(namespace["__builtins__"], replacement)
                                else:
                                    self.assertIs(
                                        namespace["__builtins__"],
                                        existing if preexisting else captured,
                                    )
                                active.clear()
        finally:
            active.clear()

    def test_tuple_dictionary_calls_use_the_same_actual_context(self):
        import builtins

        for kwargs in (None, {}):
            for target, expected in (
                (builtins.globals, self.global_namespace),
                (builtins.locals, self.local_namespace),
                (builtins.vars, self.local_namespace),
            ):
                result = self.object_call(
                    target,
                    (),
                    None if kwargs is None else id(kwargs),
                    id(self.global_namespace),
                    id(self.local_namespace),
                    id(self.builtin_namespace),
                )
                self.assertIs(result, expected)

    def test_tuple_dictionary_calls_preserve_native_binding_and_user_calls(self):
        import builtins

        def callback(*args, **kwargs):
            return args, kwargs

        value = types.SimpleNamespace(item=67)
        for target, args, kwargs in (
            (builtins.vars, (value,), {}),
            (builtins.dir, (value,), {}),
            (callback, (1,), {"item": 2}),
        ):
            expected = target(*args, **kwargs)
            result = self.object_call(
                target, args, id(kwargs), id(self.global_namespace), None,
                id(self.builtin_namespace),
            )
            self.assertEqual(result, expected)
        for target, args, kwargs in (
            (builtins.locals, (1,), {}),
            (builtins.globals, (), {"unexpected": 1}),
            (builtins.vars, (), {"object": value}),
            (builtins.dir, (value, value), {}),
            (builtins.dir, (), {"object": value}),
            (callback, (), {1: 2}),
        ):
            with self.assertRaises(TypeError) as expected:
                target(*args, **kwargs)
            with self.assertRaises(TypeError) as actual:
                self.object_call(
                    target, args, id(kwargs), id(self.global_namespace), None,
                    id(self.builtin_namespace),
                )
            self.assertEqual(str(actual.exception), str(expected.exception))


class AnnotationReplayNativeTests(unittest.TestCase):
    """Native replay mechanics; the C fixture is not production source authority."""

    @classmethod
    def setUpClass(cls):
        cls.compile_verified = native_api(
            "PySoac_CompileVerifiedSource",
            ctypes.py_object,
            ctypes.c_char_p,
            ctypes.c_ssize_t,
            ctypes.py_object,
            ctypes.c_int,
        )
        cls.clone_code = native_api(
            "PySoac_CloneAnnotationReplayCode",
            ctypes.py_object,
            ctypes.py_object,
            ctypes.py_object,
            ctypes.py_object,
        )
        cls.source_id = native_api(
            "PyCode_GetSoacStrictSourceId", ctypes.c_uint64, ctypes.py_object
        )
        cls.set_owner = native_api(
            "PyFunction_SetSoacStrictOwner",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
        )
        cls.runtime_error = borrowed_object_api(
            "PySoac_GetStrictRuntimeUnavailableError"
        )

    def provider(self, source, *, namespace=None, closure=None):
        if namespace is None:
            # The normal module execution creates CPython's reached-annotation
            # index set. The verified strict tree remains inspection-only.
            namespace = {}
            ordinary_source = source.replace("from __future__ import strict\n", "")
            exec(
                compile(
                    ordinary_source,
                    "<ordinary-annotation-fixture>",
                    "exec",
                    dont_inherit=True,
                ),
                namespace,
            )
        encoded = source.encode()
        root = self.compile_verified(
            encoded, len(encoded), "<strict-annotation-replay>", -1
        )
        pending = [root]
        while pending:
            code = pending.pop()
            if code.co_name == "__annotate__":
                return types.FunctionType(code, namespace, closure=closure)
            pending.extend(
                item for item in code.co_consts if isinstance(item, types.CodeType)
            )
        self.fail("fixture has no compiler-generated annotation provider")

    def test_clone_requires_exact_live_owner_and_actual_verified_code(self):
        provider = self.provider("from __future__ import strict\nitem: int\n")
        owner = object()
        code = provider.__code__
        with self.assertRaises(self.runtime_error):
            self.clone_code(provider, owner, code)
        self.set_owner(provider, owner)
        with self.assertRaises(self.runtime_error):
            self.clone_code(provider, object(), code)
        with self.assertRaises(self.runtime_error):
            self.clone_code(provider, owner, code.replace())
        ordinary = compile("value = 1", "<ordinary>", "exec", dont_inherit=True)
        with self.assertRaises(self.runtime_error):
            self.clone_code(provider, owner, ordinary)
        for invalid in (None, code, object()):
            with self.assertRaises(TypeError):
                self.clone_code(invalid, owner, code)
        self.assertGreater(self.source_id(code), 0)
        provider.__code__ = code.replace()
        with self.assertRaises(self.runtime_error):
            self.clone_code(provider, owner, provider.__code__)
        provider.__code__ = code
        get_slot = native_api(
            "PyType_GetSlot", ctypes.c_void_p, ctypes.py_object, ctypes.c_int
        )
        clear = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object)(
            get_slot(types.FunctionType, 51)
        )
        self.assertEqual(clear(provider), 0)
        with self.assertRaises(self.runtime_error):
            self.clone_code(provider, owner, code)

    def test_code_watcher_cannot_swap_the_provider_during_clone_preparation(self):
        provider = self.provider("from __future__ import strict\nitem: int\n")
        owner = object()
        self.set_owner(provider, owner)
        code = provider.__code__
        replacement = code.replace()
        events = []
        watcher_type = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.c_int, ctypes.py_object)

        @watcher_type
        def callback(event, created):
            if event == 0:
                events.append(created)
                provider.__code__ = replacement
            return 0

        add = native_api("PyCode_AddWatcher", ctypes.c_int, watcher_type)
        clear = native_api("PyCode_ClearWatcher", ctypes.c_int, ctypes.c_int)
        watcher = add(callback)
        try:
            with self.assertRaisesRegex(self.runtime_error, "changed during replay"):
                self.clone_code(provider, owner, code)
            self.assertEqual(len(events), 1)
            self.assertIs(provider.__code__, replacement)
            self.assertFalse(events[0].co_flags & STRICT)
            self.assertEqual(self.source_id(events[0]), 0)
        finally:
            clear(watcher)

    def test_recursive_clone_is_ordinary_without_changing_original_tree(self):
        provider = self.provider(
            "from __future__ import strict\n"
            "first: (lambda: 17)\n"
            "second: (item for item in (1, 2))\n"
        )
        owner = object()
        self.set_owner(provider, owner)
        original = provider.__code__
        replay = self.clone_code(provider, owner, original)
        pending = [(original, replay)]
        while pending:
            strict_code, ordinary_code = pending.pop()
            self.assertIsNot(strict_code, ordinary_code)
            self.assertTrue(strict_code.co_flags & STRICT)
            self.assertGreater(self.source_id(strict_code), 0)
            self.assertFalse(ordinary_code.co_flags & STRICT)
            self.assertEqual(self.source_id(ordinary_code), 0)
            self.assertEqual(strict_code.co_freevars, ordinary_code.co_freevars)
            self.assertEqual(strict_code.co_varnames, ordinary_code.co_varnames)
            for strict_item, ordinary_item in zip(
                strict_code.co_consts, ordinary_code.co_consts, strict=True
            ):
                if isinstance(strict_item, types.CodeType):
                    pending.append((strict_item, ordinary_item))
        result = types.FunctionType(replay, provider.__globals__)(2)
        self.assertEqual(result["first"](), 17)
        self.assertEqual(list(result["second"]), [1, 2])
        for clone in (
            original,
            original.replace(co_flags=original.co_flags & ~STRICT),
            marshal.loads(marshal.dumps(original)),
        ):
            with self.assertRaises(self.runtime_error):
                types.FunctionType(clone, {})(2)

    def test_annotationlib_replay_uses_real_nested_forwardrefs_and_strings(self):
        import _testinternalcapi
        import annotationlib

        provider = self.provider(
            "from __future__ import strict\nitem: list[Missing]\nknown: int\n"
        )
        _testinternalcapi.soac_prepare_annotation_replay_fixture(provider)
        _testinternalcapi.soac_install_annotation_replay_resolver(False)
        result = annotationlib.call_annotate_function(
            provider, annotationlib.Format.FORWARDREF
        )
        self.assertIsInstance(result["item"], types.GenericAlias)
        self.assertIs(result["item"].__origin__, list)
        (missing,) = result["item"].__args__
        self.assertIsInstance(missing, annotationlib.ForwardRef)
        self.assertEqual(missing.__forward_arg__, "Missing")
        self.assertIs(result["known"], int)
        strings = annotationlib.call_annotate_function(
            provider, annotationlib.Format.STRING
        )
        self.assertEqual(strings, {"item": "list[Missing]", "known": "int"})
        with self.assertRaises(self.runtime_error):
            types.FunctionType(provider.__code__, provider.__globals__)(2)

    def test_replay_preserves_actual_lexical_and_classdict_closure_slots(self):
        import _testinternalcapi
        import annotationlib

        cell = types.CellType(int)
        provider = self.provider(
            "from __future__ import strict\n"
            "def outer():\n"
            "    captured = None\n"
            "    def target(value: captured): pass\n",
            closure=(cell,),
        )
        self.assertEqual(provider.__code__.co_freevars, ("captured",))
        _testinternalcapi.soac_prepare_annotation_replay_fixture(provider)
        _testinternalcapi.soac_install_annotation_replay_resolver(False)
        self.assertEqual(provider(1), {"value": int})
        self.assertEqual(
            annotationlib.call_annotate_function(provider, annotationlib.Format.STRING),
            {"value": "captured"},
        )
        cell.cell_contents = str
        self.assertEqual(
            annotationlib.call_annotate_function(
                provider, annotationlib.Format.FORWARDREF
            ),
            {"value": str},
        )

        class Subject:
            Known = int

        class_cell = types.CellType(Subject.__dict__)
        provider = self.provider(
            "from __future__ import strict\nclass Subject:\n    field: Known\n",
            closure=(class_cell,),
        )
        self.assertEqual(provider.__code__.co_freevars, ("__classdict__",))
        _testinternalcapi.soac_prepare_annotation_replay_fixture(provider)
        self.assertEqual(provider(1), {"field": int})
        self.assertEqual(
            annotationlib.call_annotate_function(
                provider, annotationlib.Format.FORWARDREF, owner=Subject
            ),
            {"field": int},
        )

    def test_ordinary_callbacks_and_python_attributes_cannot_gain_authority(self):
        import _testinternalcapi
        import _typing

        _testinternalcapi.soac_install_annotation_replay_resolver(False)

        def ordinary(format, /):
            return {"value": int}

        self.assertIs(
            _typing._soac_annotation_replay_code(ordinary, None, 3), ordinary.__code__
        )
        provider = self.provider("from __future__ import strict\nitem: int\n")
        provider.soac_owner = None
        provider.__soac_annotation_replay__ = True
        with self.assertRaises(self.runtime_error):
            _typing._soac_annotation_replay_code(provider, None, 3)
        _testinternalcapi.soac_prepare_annotation_replay_fixture(provider)
        for invalid in (1, 2, 5):
            with self.assertRaises(ValueError):
                _typing._soac_annotation_replay_code(provider, None, invalid)

    def test_resolver_is_single_assignment_and_not_inherited_by_subinterpreters(self):
        program = r"""
import _interpreters, _testinternalcapi, _typing, ctypes, types
compile_verified = ctypes.pythonapi.PySoac_CompileVerifiedSource
compile_verified.argtypes = [ctypes.c_char_p, ctypes.c_ssize_t, ctypes.py_object, ctypes.c_int]
compile_verified.restype = ctypes.py_object
source = b"from __future__ import strict\nitem: int\n"
root = compile_verified(source, len(source), "<replay-interpreter>", -1)
code = next(item for item in root.co_consts if isinstance(item, types.CodeType))
provider = types.FunctionType(code, {})
_testinternalcapi.soac_prepare_annotation_replay_fixture(provider)
try:
    _typing._soac_annotation_replay_code(provider, None, 3)
except ImportError:
    pass
else:
    raise AssertionError("resolver unexpectedly present in fresh interpreter")
_testinternalcapi.soac_install_annotation_replay_resolver(False)
_testinternalcapi.soac_install_annotation_replay_resolver(False)
for replacement in (True, None):
    try:
        _testinternalcapi.soac_install_annotation_replay_resolver(replacement)
    except ImportError:
        pass
    else:
        raise AssertionError("resolver was replaced or removed")
assert not (_typing._soac_annotation_replay_code(provider, None, 3).co_flags & 0x10000000)
"""
        child = (
            program
            + "\nidentity = _interpreters.create()\n"
            + "try:\n"
            + f"    error = _interpreters.run_string(identity, {program!r})\n"
            + "    assert error is None, error\n"
            + "finally:\n"
            + "    _interpreters.destroy(identity)\n"
        )
        result = subprocess.run(
            [sys.executable, "-I", "-S", "-B", "-c", child],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


class NativeSourceOperationMetadataTests(unittest.TestCase):
    """Actual native definition and call sites used for interpreter admission."""

    RawPySoacCodeView = ClassBindingMetadataNativeTests.RawPySoacCodeView
    code_slots = ClassBindingMetadataNativeTests.code_slots
    assert_is_immutable = ClassBindingMetadataNativeTests.assert_is_immutable

    @classmethod
    def setUpClass(cls):
        ClassBindingMetadataNativeTests.setUpClass.__func__(cls)

    @staticmethod
    def span(node):
        return (node.lineno, node.col_offset, node.end_lineno, node.end_col_offset)

    def compile_operations(self, body):
        import ast
        import dis

        source = ("from __future__ import strict\n" + body).encode()
        root, _, product = self.compile_details(source, len(source), "<native-source-operations>", -1)
        version, nodes, recipes, tables = product
        self.assertEqual(version, 7)
        self.assert_is_immutable(product)
        self.assertEqual([table[0] for table in tables], list(range(len(nodes))))
        actual = []

        def visit(code, parent):
            index = len(actual)
            actual.append((code, parent))
            for item in code.co_consts:
                if type(item) is types.CodeType:
                    visit(item, index)

        visit(root, None)
        self.assertEqual(len(nodes), len(actual))
        for node, (code, parent) in zip(nodes, actual, strict=True):
            self.assertIs(node[2], code)
            self.assertEqual(node[1], parent)
            self.assertEqual(self.source_id(code), self.source_id(root))
        self.assertEqual([recipe[0] for recipe in recipes], list(range(len(nodes))))
        tree = ast.parse(source)
        source_spans = {self.span(node) for node in ast.walk(tree)
                        if hasattr(node, "end_lineno") and node.end_lineno is not None}
        store_forms = {opcode.opmap[name]: form for form, name in enumerate((
            "STORE_FAST", "STORE_FAST_STORE_FAST", "STORE_FAST_LOAD_FAST", "STORE_DEREF",
            "STORE_NAME", "STORE_GLOBAL", "STORE_ATTR", "STORE_SUBSCR", "STORE_SLICE",
            "DELETE_FAST", "DELETE_DEREF", "DELETE_NAME", "DELETE_GLOBAL", "DELETE_ATTR",
            "DELETE_SUBSCR"))}
        call_forms = {opcode.opmap[name]: form for form, name in enumerate((
            "CALL", "CALL_KW", "CALL_FUNCTION_EX"))}
        for code_id, count, code_size, native_names, stores, calls, gaps in tables:
            code = nodes[code_id][2]
            local_names, _ = self.code_slots(code)
            self.assertEqual(native_names, code.co_names)
            self.assertEqual(code_size, len(code.co_code))
            # Independent structured native compiler oracle. This test does not
            # execute code; the production producer never parses disassembly.
            instructions = [item for item in dis.get_instructions(code, adaptive=False)
                            if item.opcode != opcode.opmap["EXTENDED_ARG"]]
            self.assertEqual(count, len(instructions))
            occupied = {}
            store_by_lane, call_by_lane = {}, {}
            self.assertEqual(len({origin for origin, _ in stores}), len(stores))
            self.assertEqual(len({origin for origin, _ in calls}), len(calls))

            def claim(ordinal, lane, family, context, origin):
                self.assertIn(ordinal, range(count))
                self.assertIn(lane, (0, 1))
                self.assertNotIn((ordinal, lane), occupied)
                occupied[ordinal, lane] = family
                if context is None:
                    self.assertIn((9, (family, origin), ordinal, lane,
                                   instructions[ordinal].opcode, None), gaps)
                else:
                    self.assertIs(type(context), tuple)
                    for owner_kind, owner_span, item, entry, transfer, payload in context:
                        self.assertIn(owner_kind, range(6))
                        self.assertIn(owner_span, source_spans)
                        self.assertEqual(item is not None, owner_kind in (4, 5))
                        self.assertIn(entry, range(5))
                        self.assertEqual(transfer is not None, entry in (2, 3, 4))
                        if transfer is not None:
                            self.assertIn(transfer, source_spans)
                        self.assertIn(payload, (0, 1, 2))

            for origin, emissions in stores:
                kind, span, phase, detail = origin
                self.assertIn(kind, range(13))
                self.assertIn(span, source_spans)
                self.assertIn(phase, range(4))
                if kind == 10:
                    self.assertTrue(detail)
                    self.assertEqual(tuple(sorted(detail, key=lambda leaf: (leaf[1], leaf[0]))), detail)
                    self.assertEqual(len(set(detail)), len(detail))
                    for leaf_kind, leaf_span in detail:
                        self.assertIn(leaf_kind, (0, 1, 2))
                        self.assertIn(leaf_span, source_spans)
                else:
                    self.assertIsNone(detail)
                if not emissions:
                    self.assertTrue(any(reason in (0, 1) and missing == (1, origin)
                                        for reason, missing, *_ in gaps))
                for ordinal, form, first, second, lane, context in emissions:
                    claim(ordinal, lane, 1, context, origin)
                    native = instructions[ordinal]
                    self.assertEqual(store_forms[native.opcode], form)
                    for operand in (first,) if second is None else (first, second):
                        domain, index = operand
                        self.assertIn(domain, (0, 1, 2))
                        if domain == 2:
                            self.assertIsNone(index)
                        else:
                            self.assertIn(index, range(len(local_names if domain == 0 else native_names)))
                    self.assertEqual(second is not None, form in (1, 2))
                    if second is not None:
                        self.assertEqual((first, second), ((0, native.arg >> 4), (0, native.arg & 15)))
                    elif first[0] != 2:
                        self.assertEqual(first[1], native.arg)
                    store_by_lane[ordinal, lane] = (origin, (ordinal, form, first, second, lane, context))

            for origin, emissions in calls:
                kind, span, detail = origin
                self.assertIn(kind, range(10))
                self.assertIn(span, source_spans)
                if kind in (2, 3, 4):
                    self.assertIn(detail, range(len(nodes)))
                    self.assertEqual(nodes[detail][1], code_id)
                if not emissions:
                    self.assertTrue(any(reason in (0, 1, 7, 8) and missing == (2, origin)
                                        for reason, missing, *_ in gaps))
                for ordinal, offset, form, argument_count, layout, context in emissions:
                    claim(ordinal, 0, 2, context, origin)
                    native = instructions[ordinal]
                    self.assertEqual(call_forms[native.opcode], form)
                    self.assertEqual(offset, native.offset)
                    self.assertEqual(code.co_code[offset], native.opcode)
                    self.assertEqual(argument_count, None if form == 2 else native.arg)
                    channel, preloaded, positional, keywords = layout
                    self.assertIn(channel, (0, 1, 2))
                    self.assertGreaterEqual(preloaded, 0)
                    self.assertIn(positional[0], range(6))
                    self.assertIn(keywords[0], range(3))
                    if keywords[0] == 1:
                        self.assertEqual(keywords[1], tuple(item[2] for item in keywords[2]))
                    call_by_lane[ordinal, 0] = (origin, (ordinal, offset, form, argument_count, layout, context))

            for reason, origin, ordinal, lane, operation, context in gaps:
                self.assertIn(reason, range(12))
                if origin is not None:
                    self.assertIn(origin[0], (1, 2))
                if ordinal is not None:
                    self.assertIn(ordinal, range(count))
                    self.assertEqual(operation, instructions[ordinal].opcode)
                    self.assertIn(lane, (0, 1))
                if reason in (0, 4):
                    self.assertIsNotNone(origin)
                    self.assertEqual((ordinal, lane, operation, context), (None, None, None, None))

            # Complete native publication/call inventory is an authority check,
            # not a requirement that SOAC use the same instruction schedule.
            for ordinal, native in enumerate(instructions):
                store = store_forms.get(native.opcode)
                if store is not None:
                    for lane in (0, 1) if store == 1 else (0,):
                        if (ordinal, lane) not in store_by_lane:
                            self.assertIn((3, None, ordinal, lane, native.opcode, None), gaps)
                if native.opcode in call_forms and (ordinal, 0) not in call_by_lane:
                    self.assertIn((5, None, ordinal, 0, native.opcode, None), gaps)
        return source, root, nodes, tables, tree

    def function_table(self, nodes, tables, original):
        matches = [node for node in nodes
                   if node[3] in (2, 3) and node[5] == self.span(original)]
        self.assertEqual(len(matches), 1)
        return matches[0], tables[matches[0][0]]

    def test_import_store_uses_actual_alias_origin_not_a_fabricated_name(self):
        _, _, nodes, tables, tree = self.compile_operations(
            "def imported(): import math as result; return result\n")
        function = tree.body[1]
        node, table = self.function_table(nodes, tables, function)
        origin, rows = self.stores_at(table, function.body[0].names[0], kind=4)
        self.assertEqual(origin, (4, self.span(function.body[0].names[0]), 0, None))
        self.assertTrue(rows)
        names, _ = self.code_slots(node[2])
        self.assertTrue(all(row[2 + row[4]] == (0, names.index("result")) for row in rows))

    def test_pattern_store_retains_actual_capture_origin_and_slot(self):
        _, _, nodes, tables, tree = self.compile_operations("""
def match_value(value):
    match value:
        case captured: return captured
""")
        function = tree.body[1]
        node, table = self.function_table(nodes, tables, function)
        pattern = function.body[0].cases[0].pattern
        binding = (10, self.span(pattern), 0, ((0, self.span(pattern)),))
        [(stored_origin, stored_rows)] = [row for row in table[4] if row[0] == binding]
        self.assertEqual(stored_origin, binding)
        self.assertTrue(stored_rows)
        names, _ = self.code_slots(node[2])
        self.assertTrue(all(row[2 + row[4]] == (0, names.index("captured"))
                            for row in stored_rows))

    def test_finally_keeps_actual_call_publication_sites_and_canonical_child(self):
        _, _, nodes, tables, tree = self.compile_operations("""
def repeated(value, callback):
    try:
        callback()
    finally:
        callback(value)
        def child(item): return item
""")
        function = tree.body[1]
        parent, table = self.function_table(nodes, tables, function)
        original = function.body[0].finalbody[0].value
        _, rows = self.calls_at(table, original)
        self.assertGreaterEqual(len(rows), 2)
        self.assertEqual(len({row[0] for row in rows}), len(rows))
        child = function.body[0].finalbody[1]
        child_node, _ = self.function_table(nodes, tables, child)
        self.assertEqual(child_node[1], parent[0])
        self.assertIn(child_node[2], parent[2].co_consts)
        _, publications = self.stores_at(table, child, kind=1)
        self.assertGreaterEqual(len(publications), 2)
        self.assertEqual(len({(row[0], row[4]) for row in publications}), len(publications))

    def test_unicode_private_names_use_native_slots_and_utf8_source_ranges(self):
        _, _, nodes, tables, tree = self.compile_operations("""
class Holder:
    def read(self, __é): return __é
""")
        function = tree.body[1].body[0]
        node, _ = self.function_table(nodes, tables, function)
        names, _ = self.code_slots(node[2])
        self.assertIn("_Holder__é", names)
        argument = function.args.args[1]
        self.assertEqual(argument.end_col_offset - argument.col_offset,
                         len(argument.arg.encode()))
        self.assertEqual(node[5], self.span(function))
        _, publications = self.stores_at(tables[node[1]], function, kind=1)
        self.assertTrue(publications)

    def test_collection_changes_no_stock_compile_code_positions_or_exception_table(self):
        sources = (
            "def augmented(first, second): first += second; return first\n",
            "def imported(): import math as result; return result\n",
            "def pair(first, second): a, b = first, second; return a\n",
            "def coupled(first, second): return first, (alias := second)\n",
            "def capture(value): return lambda: value\n",
            "def f(value, cb):\n    try: cb()\n    finally: cb(value)\n",
            "def outer(cb):\n    try: cb()\n    finally:\n        @cb\n        class C:\n            def read(self, value): return value\n",
        )

        def equal(left, right):
            for attribute in ("co_code", "co_flags", "co_names", "co_varnames", "co_cellvars",
                              "co_freevars", "co_linetable", "co_exceptiontable", "co_firstlineno"):
                self.assertEqual(getattr(left, attribute), getattr(right, attribute))
            self.assertEqual(tuple(left.co_positions()), tuple(right.co_positions()))
            self.assertEqual(len(left.co_consts), len(right.co_consts))
            for a, b in zip(left.co_consts, right.co_consts, strict=True):
                if type(a) is types.CodeType:
                    equal(a, b)
                else:
                    self.assertEqual(a, b)

        for body in sources:
            with self.subTest(source=body):
                source, root, _, _, _ = self.compile_operations(body)
                ordinary = compile(source, "<native-source-operations>", "exec", dont_inherit=True)
                self.assertEqual(self.source_id(ordinary), 0)
                equal(root, ordinary)
                with self.assertRaisesRegex(self.runtime_error, "strict.*execution"):
                    exec(root, {})

    def stores_at(self, table, original, kind=0, phase=0):
        matches = [(origin, rows) for origin, rows in table[4]
                   if origin[:3] == (kind, self.span(original), phase)]
        self.assertEqual(len(matches), 1)
        return matches[0]

    def calls_at(self, table, original, kind=0, detail=None):
        matches = [(origin, rows) for origin, rows in table[5]
                   if origin[:2] == (kind, self.span(original))
                   and (detail is None or origin[2] == detail)]
        self.assertEqual(len(matches), 1)
        return matches[0]

    def test_source_store_pair_preserves_both_publications_and_slots(self):
        _, _, nodes, tables, tree = self.compile_operations(
            "def copies(value): first = second = value; return first, second\n")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        first, second = [self.stores_at(table, target)[1]
                         for target in function.body[0].targets]
        self.assertEqual([len(first), len(second)], [1, 1])
        self.assertEqual(first[0][:4], second[0][:4])
        self.assertEqual((first[0][1], first[0][4], second[0][4]), (1, 0, 1))

    def test_namespace_global_cell_and_delete_operands_are_final_native_indices(self):
        import dis

        source, root, nodes, tables, tree = self.compile_operations("""
namespace_only = 0
del namespace_only
published = 1
del published
def changes(value):
    global published
    def child(): return value
    value = child()
    published = value
    del value
    del published
""")
        module = tables[0]
        _, rows = self.stores_at(module, tree.body[1].targets[0])
        self.assertEqual({row[1] for row in rows}, {4})
        self.assertEqual(module[3][rows[0][2][1]], "namespace_only")
        _, rows = self.stores_at(module, tree.body[2].targets[0], phase=1)
        self.assertEqual({row[1] for row in rows}, {11})
        # The nested global directive also marks the module's same spelling
        # GLOBAL_EXPLICIT during the native symbol-table pass.
        _, rows = self.stores_at(module, tree.body[3].targets[0])
        self.assertEqual({row[1] for row in rows}, {5})
        self.assertEqual(module[3][rows[0][2][1]], "published")
        _, rows = self.stores_at(module, tree.body[4].targets[0], phase=1)
        self.assertEqual({row[1] for row in rows}, {12})
        ordinary = compile(source, "<native-source-operations>", "exec", dont_inherit=True)
        self.assertEqual(root.co_code, ordinary.co_code)
        self.assertEqual(
            [(instruction.opname, instruction.argval)
             for instruction in dis.get_instructions(ordinary, adaptive=False)
             if instruction.opname in ("STORE_NAME", "DELETE_NAME", "STORE_GLOBAL", "DELETE_GLOBAL")
             and instruction.argval in ("namespace_only", "published")],
            [("STORE_NAME", "namespace_only"), ("DELETE_NAME", "namespace_only"),
             ("STORE_GLOBAL", "published"), ("DELETE_GLOBAL", "published")],
        )
        function = tree.body[5]
        node, table = self.function_table(nodes, tables, function)
        names, kinds = self.code_slots(node[2])
        _, cell = self.stores_at(table, function.body[2].targets[0])
        self.assertEqual({row[1] for row in cell}, {3})
        self.assertEqual(names[cell[0][2][1]], "value")
        self.assertTrue(kinds[cell[0][2][1]] & 0x40)
        _, global_rows = self.stores_at(table, function.body[3].targets[0])
        self.assertEqual({row[1] for row in global_rows}, {5})
        self.assertEqual(table[3][global_rows[0][2][1]], "published")
        self.assertEqual({row[1] for row in self.stores_at(table, function.body[4].targets[0], phase=1)[1]}, {10})
        self.assertEqual({row[1] for row in self.stores_at(table, function.body[5].targets[0], phase=1)[1]}, {12})

    def test_nonlocal_store_and_delete_keep_free_slot_remapping(self):
        _, _, nodes, tables, tree = self.compile_operations("""
def outer(value):
    def change(replacement):
        nonlocal value
        value = replacement
        del value
    return change
""")
        function = tree.body[1].body[0]
        node, table = self.function_table(nodes, tables, function)
        names, kinds = self.code_slots(node[2])
        for statement, phase, form in ((function.body[1], 0, 3), (function.body[2], 1, 10)):
            _, [row] = self.stores_at(table, statement.targets[0], phase=phase)
            self.assertEqual(row[1], form)
            self.assertEqual(row[2][0], 0)
            slot = row[2][1]
            self.assertEqual(names[slot], "value")
            self.assertTrue(kinds[slot] & 0x80)
            self.assertFalse(kinds[slot] & 0x40)

    def test_attribute_subscript_slice_and_augassign_keep_original_targets(self):
        _, _, nodes, tables, tree = self.compile_operations("""
def targets(receiver, key, value):
    receiver.attr = value
    receiver[key] = value
    receiver[key:value] = value
    receiver.attr += value
    receiver[key] += value
    receiver[key:value] += value
    del receiver.attr
    del receiver[key]
""")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        for statement, kind, form in zip(function.body, (11, 12, 12, 11, 12, 12, 11, 12),
                                         (6, 7, 8, 6, 7, 8, 13, 14), strict=True):
            target = statement.targets[0] if hasattr(statement, "targets") else statement.target
            phase = 1 if form in (13, 14) else 0
            origin, rows = self.stores_at(table, target, kind=kind, phase=phase)
            self.assertIsNone(origin[3])
            self.assertEqual({row[1] for row in rows}, {form})

    def test_declarations_imports_and_type_parameters_use_actual_ast_roles(self):
        _, _, nodes, tables, tree = self.compile_operations("""
def declarations():
    import math as imported
    from math import floor as selected
    def child(): pass
    async def asynchronous(): pass
    class Nested: pass
    type Alias[T, *Ts, **P] = tuple[T, *Ts]
    return imported, selected, child, asynchronous, Nested, Alias
""")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        for statement, kind in zip(function.body[:5], (4, 5, 1, 2, 3), strict=True):
            original = statement.names[0] if kind in (4, 5) else statement
            self.assertTrue(self.stores_at(table, original, kind=kind)[1])
        alias = function.body[5]
        self.assertTrue(self.stores_at(table, alias.name)[1])
        helper = [node for node in nodes if node[1] == table[0] and node[4] == 5]
        self.assertEqual(len(helper), 1)
        for parameter, kind in zip(alias.type_params, (7, 8, 9), strict=True):
            self.assertTrue(self.stores_at(tables[helper[0][0]], parameter, kind=kind)[1])

    def test_or_pattern_capture_provenance_follows_native_reordering(self):
        import ast

        _, _, nodes, tables, tree = self.compile_operations("""
def match_order(subject):
    match subject:
        case [first, second] | (second, first):
            return first, second
""")
        function = tree.body[1]
        node, table = self.function_table(nodes, tables, function)
        owner = function.body[0].cases[0].pattern
        names, _ = self.code_slots(node[2])
        captures = [(origin, rows) for origin, rows in table[4]
                    if origin[:3] == (10, self.span(owner), 0)]
        self.assertEqual(len(captures), 2)
        for origin, rows in captures:
            self.assertTrue(rows)
            slot = rows[0][2 + rows[0][4]][1]
            expected = tuple(sorted(((0, self.span(leaf)) for leaf in ast.walk(owner)
                                     if isinstance(leaf, ast.MatchAs) and leaf.name == names[slot]),
                                    key=lambda leaf: (leaf[1], leaf[0])))
            self.assertEqual(len(expected), 2)
            self.assertEqual(origin[3], expected)

    def test_mapping_rest_and_star_pattern_leaves_are_not_name_spans(self):
        import ast

        _, _, nodes, tables, tree = self.compile_operations("""
def capture_kinds(subject):
    match subject:
        case {"key": value, **rest}: return value, rest
        case [first, *tail]: return first, tail
""")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        details = [leaf for origin, _ in table[4] if origin[0] == 10 for leaf in origin[3]]
        mapping, sequence = [case.pattern for case in function.body[0].cases]
        star = next(node for node in ast.walk(sequence) if isinstance(node, ast.MatchStar))
        self.assertIn((2, self.span(mapping)), details)
        self.assertIn((1, self.span(star)), details)

    def test_except_alias_publication_and_both_cleanup_phases_are_separate(self):
        for keyword, owner_kind in (("except", 2), ("except*", 3)):
            with self.subTest(keyword=keyword):
                _, _, nodes, tables, tree = self.compile_operations(
                    "def handlers(callback):\n"
                    "    try: callback()\n"
                    f"    {keyword} RuntimeError as error:\n"
                    "        callback(error)\n")
                function = tree.body[1]
                _, table = self.function_table(nodes, tables, function)
                handler = function.body[0].handlers[0]
                self.assertTrue(self.stores_at(table, handler, kind=6, phase=0)[1])
                for phase in (2, 3):
                    _, rows = self.stores_at(table, handler, kind=6, phase=phase)
                    self.assertGreaterEqual(len(rows), 2)
                    contexts = {row[-1][-1] for row in rows}
                    self.assertIn((owner_kind, self.span(handler), None, 0, None, 0), contexts)
                    self.assertIn((owner_kind, self.span(handler), None, 1, None, 2), contexts)

    def test_finally_return_contexts_keep_original_transfer_statements(self):
        _, _, nodes, tables, tree = self.compile_operations("""
def returns(flag, first, second, callback):
    try:
        if flag: return first
        return second
    finally:
        alias = first
        callback(alias)
""")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        owner = function.body[0]
        call = owner.finalbody[1].value
        origin, rows = self.calls_at(table, call)
        transfers = (owner.body[0].body[0], owner.body[1])
        contexts = {row[-1][-1] for row in rows if row[-1] is not None}
        for transfer in transfers:
            self.assertIn((0, self.span(owner), None, 2, self.span(transfer), 1), contexts)
        self.assertIn((0, self.span(owner), None, 1, None, 2), contexts)
        self.assertGreaterEqual(len(self.stores_at(table, owner.finalbody[0].targets[0])[1]), 3)
        self.assertNotIn((4, (2, origin), None, None, None, None), table[6])

    def test_replacing_pending_return_is_explicitly_unavailable_not_stale_context(self):
        _, _, nodes, tables, tree = self.compile_operations("""
def replacement(callback):
    try:
        try:
            return 1
        finally:
            return 2
    finally:
        callback()
""")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        call = function.body[0].finalbody[0].value
        origin, rows = self.calls_at(table, call)
        unavailable = [row for row in rows if row[-1] is None]
        self.assertTrue(unavailable)
        for row in unavailable:
            self.assertIn((9, (2, origin), row[0], 0, opcode.opmap["CALL"], None), table[6])

    def test_source_call_forms_channels_and_expansion_preparation_are_exact(self):
        _, _, nodes, tables, tree = self.compile_operations("""
def plans(function, receiver, first, items, last, mapping):
    function(first)
    receiver.method(first, named=last)
    function(*items)
    function(first, *items, last, **mapping)
    function(first, last, **mapping)
    function(**mapping)
    function(*items, named=first, **mapping, tail=last)
""")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        expected = ((0, 0, 0, 0), (1, 1, 0, 1), (2, 0, 2, 0),
                    (2, 0, 4, 2), (2, 0, 3, 2), (2, 0, 1, 2), (2, 0, 2, 2))
        for statement, (form, channel, positional, keywords) in zip(function.body, expected, strict=True):
            _, [row] = self.calls_at(table, statement.value)
            self.assertEqual((row[2], row[4][0], row[4][2][0], row[4][3][0]),
                             (form, channel, positional, keywords))
            self.assertEqual(row[4][1], 0)
        _, [last] = self.calls_at(table, function.body[-1].value)
        self.assertEqual(last[4][3][3], ((0, 0, 1, 0), (1, 1, 1, None), (0, 2, 1, 0)))

    def test_large_call_builds_list_before_arguments_and_preserves_map_strategy(self):
        positional = ", ".join("value" for _ in range(31))
        keywords = ", ".join(f"key{i}=value" for i in range(20))
        _, _, nodes, tables, tree = self.compile_operations(
            f"def large(function, value):\n    function({positional})\n    function({keywords})\n")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        _, [first] = self.calls_at(table, function.body[0].value)
        self.assertEqual(first[4][2][0], 5)
        self.assertEqual(len(first[4][2][1]), 31)
        self.assertTrue(all(kind == 0 for kind, _ in first[4][2][1]))
        _, [second] = self.calls_at(table, function.body[1].value)
        self.assertEqual(second[4][3][3], ((0, 0, 20, 1),))

    def test_folded_argument_builder_remains_an_explicit_call_input_gap(self):
        _, _, nodes, tables, tree = self.compile_operations(
            "def folded(function, mapping): return function(1, 2, **mapping)\n")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        origin, [row] = self.calls_at(table, function.body[0].value)
        self.assertEqual(row[4][2][0], 3)
        self.assertIn((10, (2, origin), row[0], 0, opcode.opmap["CALL_FUNCTION_EX"], row[-1]), table[6])

    def test_guarded_builtin_and_lowered_super_do_not_claim_complete_call_coverage(self):
        _, _, nodes, tables, tree = self.compile_operations("""
def guarded(values): return list(value for value in values)
class Derived:
    def method(self): return super().method()
    def attribute(self): return super().field
""")
        function = tree.body[1]
        _, table = self.function_table(nodes, tables, function)
        origin, rows = self.calls_at(table, function.body[0].value)
        self.assertTrue(rows)
        self.assertTrue(any(row[0] == 7 and row[1] == (2, origin) for row in table[6]))
        for function in tree.body[2].body:
            _, table = self.function_table(nodes, tables, function)
            returned = function.body[0].value
            attribute = returned.func if hasattr(returned, "func") else returned
            origin, rows = self.calls_at(table, attribute.value)
            self.assertEqual(rows, ())
            self.assertIn((8, (2, origin), None, None, None, ()), table[6])

    def test_compiler_call_roles_use_same_tree_children_and_native_prefixes(self):
        _, _, nodes, tables, tree = self.compile_operations("""
@decorate
def generic[T](value=1, *, keyword=2): return value
@decorate
class Generic[T](Base, metaclass=Meta): pass
def generator(values): return (item for item in values)
def assertion(value): assert value, message
""")
        for declaration in tree.body[1:3]:
            _, [decorator] = self.calls_at(tables[0], declaration, kind=1, detail=0)
            self.assertEqual((decorator[2], decorator[3], decorator[4][:2]), (0, 0, (2, 0)))
            origin, [scope] = self.calls_at(tables[0], declaration, kind=3)
            self.assertEqual(nodes[origin[2]][1], 0)
            if declaration is tree.body[1]:
                self.assertEqual((scope[3], scope[4][:2]), (1, (2, 1)))
            else:
                self.assertEqual((scope[3], scope[4][:2]), (0, (0, 0)))
                constructor_origin, [constructor] = self.calls_at(tables[origin[2]], declaration, kind=2)
                self.assertEqual(nodes[constructor_origin[2]][1], origin[2])
                self.assertEqual(constructor[4][:2], (0, 2))
                self.assertEqual(constructor[4][2][1][-1], (2, None))
        function = tree.body[3]
        _, table = self.function_table(nodes, tables, function)
        origin, [generator] = self.calls_at(table, function.body[0].value, kind=4)
        self.assertEqual(nodes[origin[2]][1], table[0])
        self.assertEqual((generator[3], generator[4][:2]), (0, (2, 0)))
        function = tree.body[4]
        _, table = self.function_table(nodes, tables, function)
        _, [assertion] = self.calls_at(table, function.body[0], kind=9)
        self.assertEqual((assertion[3], assertion[4][:2]), (0, (2, 0)))

    def test_exceptional_with_exits_keep_noncall_gaps_and_exact_native_context(self):
        import dis

        for asynchronous, exit_kind, owner_kind in ((False, 6, 4), (True, 8, 5)):
            with self.subTest(asynchronous=asynchronous):
                prefix = "async " if asynchronous else ""
                source, _, nodes, tables, tree = self.compile_operations(
                    f"{prefix}def scoped(first, second, value, finish, callback):\n"
                    f"    {prefix}with first, second:\n"
                    "        if finish:\n"
                    "            return value\n"
                    "        callback(value)\n"
                    "    return value\n")
                function = tree.body[1]
                node, table = self.function_table(nodes, tables, function)
                owner = function.body[0]
                transfer = owner.body[0].body[0]
                for item in (0, 1):
                    origin, exits = self.calls_at(table, owner, exit_kind, item)
                    self.assertTrue(exits)
                    for row in exits:
                        self.assertEqual((row[2], row[3], row[4][:2]), (0, 3, (1, 3)))
                    fallthrough = (owner_kind, self.span(owner), item, 0, None, 0)
                    returning = (owner_kind, self.span(owner), item, 2,
                                 self.span(transfer), 1)
                    exceptional = (owner_kind, self.span(owner), item, 1, None, 2)
                    contexts = {row[-1] for row in exits}
                    self.assertIn((fallthrough,), contexts)
                    self.assertIn((returning,), contexts)
                    self.assertNotIn((exceptional,), contexts)
                    # WITH_EXCEPT_START is not a CALL opcode. Keep the exact
                    # original operation and exception continuation as a gap,
                    # without an invented instruction, lane, or byte offset.
                    alternatives = [gap for gap in table[6]
                                    if gap[0] == 8 and gap[1] == (2, origin)]
                    self.assertEqual(alternatives, [
                        (8, (2, origin), None, None, None, (exceptional,))])
                    self.assertFalse(any(reason == 0 and missing == (2, origin)
                                         for reason, missing, *_ in table[6]))

                actual = node[2]
                instructions = list(dis.get_instructions(actual, adaptive=False))
                self.assertEqual(sum(item.opcode == opcode.opmap["WITH_EXCEPT_START"]
                                     for item in instructions), 2)
                # Compile the unchanged source through the ordinary compiler
                # path too. Neither code object is executed by this control.
                ordinary = compile(source, "<native-source-operations>", "exec", dont_inherit=True)
                [ordinary_function] = [item for item in ordinary.co_consts
                                       if type(item) is types.CodeType]
                self.assertEqual(actual.co_code, ordinary_function.co_code)
                self.assertEqual(actual.co_linetable, ordinary_function.co_linetable)
                self.assertEqual(actual.co_exceptiontable, ordinary_function.co_exceptiontable)


    def test_with_and_async_with_keep_item_and_return_cleanup_contexts(self):
        for asynchronous, enter_kind, exit_kind, owner_kind in ((False, 5, 6, 4), (True, 7, 8, 5)):
            with self.subTest(asynchronous=asynchronous):
                prefix = "async " if asynchronous else ""
                _, _, nodes, tables, tree = self.compile_operations(
                    f"{prefix}def scoped(first, second, value):\n"
                    f"    {prefix}with first, second:\n"
                    "        return value\n")
                function = tree.body[1]
                _, table = self.function_table(nodes, tables, function)
                owner = function.body[0]
                transfer = owner.body[0]
                for item in (0, 1):
                    _, [enter] = self.calls_at(table, owner, enter_kind, item)
                    self.assertEqual((enter[3], enter[4][:2]), (0, (1, 0)))
                    _, exits = self.calls_at(table, owner, exit_kind, item)
                    self.assertTrue(exits)
                    for row in exits:
                        self.assertEqual((row[3], row[4][:2]), (3, (1, 3)))
                    self.assertIn((owner_kind, self.span(owner), item, 2, self.span(transfer), 1),
                                  {row[-1][-1] for row in exits})

    def test_assembled_call_offsets_include_real_prefix_and_cache_sizes(self):
        # More than 255 names require native EXTENDED_ARGs before the Call.
        assignments = "\n".join(f"name_{index} = {index}" for index in range(300))
        source, root, _, tables, tree = self.compile_operations(assignments + "\ncallback(name_299)\n")
        origin, [row] = self.calls_at(tables[0], tree.body[-1].value)
        self.assertEqual(origin[0], 0)
        self.assertGreater(row[1], 2 * row[0])
        self.assertEqual(root.co_code[row[1]], opcode.opmap["CALL"])
        ordinary = compile(source, "<native-source-operations>", "exec", dont_inherit=True)
        self.assertEqual(root.co_code, ordinary.co_code)
        self.assertEqual(root.co_linetable, ordinary.co_linetable)
        self.assertEqual(root.co_exceptiontable, ordinary.co_exceptiontable)

    def test_eager_comprehension_target_is_source_store_but_snapshots_remain_gaps(self):
        _, _, nodes, tables, tree = self.compile_operations(
            "def eager(values): return [item for item in values]\n")
        function = tree.body[1]
        node, table = self.function_table(nodes, tables, function)
        target = function.body[0].value.generators[0].target
        origin, rows = self.stores_at(table, target)
        self.assertEqual(origin, (0, self.span(target), 0, None))
        names, kinds = self.code_slots(node[2])
        slot = names.index("item")
        self.assertTrue(kinds[slot] & 0x20)
        self.assertTrue(all(row[2 + row[4]] == (0, slot) for row in rows))
        self.assertTrue(any(reason == 11 and operation == opcode.opmap["LOAD_FAST_AND_CLEAR"]
                            for reason, _, _, _, operation, _ in table[6]))
        self.assertTrue(any(reason == 3 and operation in (opcode.opmap["STORE_FAST"],
                                                         opcode.opmap["STORE_FAST_LOAD_FAST"],
                                                         opcode.opmap["STORE_FAST_STORE_FAST"])
                            for reason, _, _, _, operation, _ in table[6]))

    def test_removed_compiler_children_have_no_invented_role_or_code_id(self):
        _, _, nodes, tables, tree = self.compile_operations("""
if False:
    class Dead: pass
    def dead_generic[T](): pass
    dead_generator = (item for item in ())
    dead_call = factory(item for item in ())
class Live: pass
def live_generic[T](): pass
live_generator = (item for item in ())
""")
        module = tables[0]
        dead = tree.body[1].body
        for original, role in ((dead[0], 2), (dead[1], 3),
                               (dead[2].value, 4), (dead[3].value.args[0], 4)):
            self.assertFalse(any(origin[:2] == (role, self.span(original))
                                 for origin, _ in module[5]))
        for original, kind in ((dead[0], 3), (dead[1], 1), (dead[2].targets[0], 0)):
            origin, rows = self.stores_at(module, original, kind=kind)
            self.assertEqual(rows, ())
            self.assertIn((0, (1, origin), None, None, None, None), module[6])
        source_call, rows = self.calls_at(module, dead[3].value)
        self.assertEqual(rows, ())
        self.assertIn((0, (2, source_call), None, None, None, None), module[6])
        for original, role in ((tree.body[2], 2), (tree.body[3], 3), (tree.body[4].value, 4)):
            origin, rows = self.calls_at(module, original, kind=role)
            self.assertTrue(rows)
            self.assertEqual(nodes[origin[2]][1], 0)
            self.assertEqual(nodes[origin[2]][5], self.span(original))

    def test_surviving_call_with_removed_child_rejects_real_native_product(self):
        # A private test-only watcher replaces a fresh root's owned constants
        # tuple. The corrupted code is never executed or handed to a source
        # body. This exercises the producer's lost-child error, not authority.
        source_dir = Path(sysconfig.get_config_var("abs_srcdir"))
        build_dir = Path(sysconfig.get_config_var("abs_builddir"))
        self.assertEqual((build_dir / "python").resolve(), Path(sys._base_executable).resolve())
        compiler = shlex.split(sysconfig.get_config_var("CC"))
        configured = shlex.split(sysconfig.get_config_var("CONFIGURE_CPPFLAGS") or "")
        self.assertTrue(compiler)
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            c_source = directory / "missing_child.c"
            library = directory / "missing_child.so"
            c_source.write_text(r'''
#include <Python.h>

typedef struct {
    PyObject *filename;  /* Borrowed for the one synchronous native call. */
    PyObject *error;
    int fired;
} FaultScope;

/* Test instrumentation only: caller-owned stack record, GIL-held, no nesting.
 * No pointer is installed in a compiler/runtime object or retained afterward. */
static FaultScope *active_scope;

static int
remove_fresh_child(PyCodeEvent event, PyCodeObject *code)
{
    FaultScope *scope = active_scope;
    if (scope == NULL || scope->fired || event != PY_CODE_EVENT_CREATE ||
        PyUnicode_CompareWithASCIIString(code->co_name, "<module>") != 0 ||
        PyUnicode_Compare(code->co_filename, scope->filename) != 0) {
        return 0;
    }
    scope->fired = 1;  /* Publish before any allocation/reentrant callback. */
    Py_ssize_t count = PyTuple_GET_SIZE(code->co_consts);
    Py_ssize_t selected = -1;
    for (Py_ssize_t i = 0; i < count; i++) {
        if (PyCode_Check(PyTuple_GET_ITEM(code->co_consts, i))) {
            if (selected >= 0) {
                PyErr_SetString(PyExc_SystemError, "fault fixture needs exactly one root child");
                scope->error = PyErr_GetRaisedException();
                return 0;
            }
            selected = i;
        }
    }
    if (selected < 0) {
        PyErr_SetString(PyExc_SystemError, "fault fixture did not find the fresh root child");
        scope->error = PyErr_GetRaisedException();
        return 0;
    }
    PyObject *replacement = PyTuple_New(count);
    if (replacement == NULL) {
        scope->error = PyErr_GetRaisedException();
        return 0;
    }
    for (Py_ssize_t i = 0; i < count; i++) {
        PyObject *value = i == selected ? Py_None : PyTuple_GET_ITEM(code->co_consts, i);
        PyTuple_SET_ITEM(replacement, i, Py_NewRef(value));
    }
    /* Publish before releasing the old owned tuple. Never dereference the
     * removed child after this point; the compiler validates its own owners. */
    Py_SETREF(code->co_consts, replacement);
    return 0;
}

PyObject *
soac_test_missing_child_compile(const char *source, Py_ssize_t length,
                                PyObject *filename, int *fired)
{
    if (PyErr_Occurred()) {
        return NULL;
    }
    if (active_scope != NULL || !PyUnicode_CheckExact(filename)) {
        PyErr_SetString(PyExc_SystemError, "invalid or nested private compile fault fixture");
        return NULL;
    }
    *fired = 0;
    FaultScope scope = {filename, NULL, 0};
    int watcher = PyCode_AddWatcher(remove_fresh_child);
    if (watcher < 0) {
        return NULL;
    }
    active_scope = &scope;
    PyObject *result = PySoac_CompileVerifiedSourceDetails(source, length, filename, -1);
    PyObject *error = PyErr_GetRaisedException();
    active_scope = NULL;
    if (PyCode_ClearWatcher(watcher) < 0) {
        PyObject *clear_error = PyErr_GetRaisedException();
        if (error == NULL) {
            error = clear_error;
        }
        else {
            Py_XDECREF(clear_error);
        }
    }
    *fired = scope.fired;
    if (scope.error != NULL) {
        Py_XDECREF(error);
        error = scope.error;
    }
    if (error != NULL) {
        Py_XDECREF(result);
        PyErr_SetRaisedException(error);
        return NULL;
    }
    if (result == NULL || !scope.fired) {
        Py_XDECREF(result);
        PyErr_SetString(PyExc_SystemError, "private compile fault did not reach its root");
        return NULL;
    }
    return result;
}
''')
            compiled = subprocess.run(
                [*compiler, *configured, "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
                 f"-I{source_dir / 'Include'}", f"-I{build_dir}",
                 str(c_source), "-o", str(library)],
                capture_output=True, text=True, check=False, timeout=60,
            )
            self.assertEqual(compiled.returncode, 0, compiled.stdout + compiled.stderr)
            probe = ctypes.PyDLL(str(library))
            compile_missing = probe.soac_test_missing_child_compile
            compile_missing.argtypes = [ctypes.c_char_p, ctypes.c_ssize_t, ctypes.py_object,
                                        ctypes.POINTER(ctypes.c_int)]
            compile_missing.restype = ctypes.py_object
            source = b"from __future__ import strict\nclass Survives: pass\n"
            filename = str(directory / "only-this-fresh-root.py")
            fired = ctypes.c_int()
            with self.assertRaisesRegex(SystemError, "surviving compiler Call lost its retained native child"):
                compile_missing(source, len(source), filename, ctypes.byref(fired))
            self.assertEqual(fired.value, 1)
            # The watcher is gone even on the producer error path.
            root, _, product = self.compile_details(source, len(source), filename, -1)
            self.assertGreater(len(product[1]), 1)
            self.assertTrue(any(type(value) is types.CodeType for value in root.co_consts))

    def test_wire6_metadata_changes_no_ordinary_compile_output_for_new_source_roles(self):
        bodies = (
            "def f(x): a = b = x; return a, b\n",
            "def f(obj, key, value): obj.attr = value; obj[key] += value; del obj[key]\n",
            "def f(cb):\n    try: cb()\n    except ValueError as err: cb(err)\n",
            "def f(cb):\n    try: cb()\n    except* ValueError as err: cb(err)\n",
            "def f(x):\n    match x:\n        case [a, b] | (b, a): return a, b\n",
            "def f(cb, x, kw): return cb(1, *x, named=x, **kw)\n",
            "@decorate\ndef f[T](x=1, *, y=2): return x\n",
            "@decorate\nclass C[T](Base, metaclass=Meta): pass\n",
            "def f(ctx, value):\n    with ctx: return value\n",
            "async def f(ctx, value):\n    async with ctx: return value\n",
            "def f(cb):\n    try:\n        try: return 1\n        finally: return 2\n    finally: cb()\n",
            "def f(values): return {item: item for item in values}\n",
        )

        def equal(left, right):
            for attribute in ("co_code", "co_flags", "co_names", "co_varnames", "co_cellvars",
                              "co_freevars", "co_linetable", "co_exceptiontable", "co_firstlineno"):
                self.assertEqual(getattr(left, attribute), getattr(right, attribute))
            self.assertEqual(tuple(left.co_positions()), tuple(right.co_positions()))
            self.assertEqual(len(left.co_consts), len(right.co_consts))
            for a, b in zip(left.co_consts, right.co_consts, strict=True):
                if type(a) is types.CodeType:
                    equal(a, b)
                else:
                    self.assertEqual(a, b)

        for body in bodies:
            with self.subTest(source=body):
                source, root, _, _, _ = self.compile_operations(body)
                ordinary = compile(source, "<native-source-operations>", "exec", dont_inherit=True)
                self.assertEqual(self.source_id(ordinary), 0)
                equal(root, ordinary)


class StrictInterpreterHookNativeTests(unittest.TestCase):
    """Real ordinary native frames with trusted C fixture policy, NOT ty authority.

    No _soac_ext/JIT, managed generator or SOAC lifetime-frame fixture is loaded.
    The production authenticated loader remains a separate end-to-end gate.
    """

    ROOT_BEGIN, ROOT_END, BIRTH, ENTER = range(1, 5)
    RETURN, LEAVE, PREPARE, BIND, STORE = range(6, 11)
    ATTRIBUTE, STARTED, FAILED = range(11, 14)
    ENFORCED_CLASS, DYNAMIC_CLASS = range(2)

    @classmethod
    def setUpClass(cls):
        import _testinternalcapi

        cls.capi = _testinternalcapi
        cls.details_api = native_api(
            "PySoac_CompileVerifiedSourceDetails", ctypes.py_object,
            ctypes.c_char_p, ctypes.c_ssize_t, ctypes.py_object, ctypes.c_int,
        )
        cls.owner = native_api(
            "PyFunction_GetSoacStrictOwner", ctypes.c_void_p, ctypes.py_object
        )
        cls.sealed = native_api(
            "PyFunction_GetSoacStrictId", ctypes.c_uint64, ctypes.py_object
        )
        cls.type_contract = native_api(
            "PyType_HasSoacContract", ctypes.c_int, ctypes.py_object
        )
        cls.c_call = native_api(
            "PyObject_CallObject", ctypes.py_object, ctypes.py_object, ctypes.py_object
        )
        cls.set_vectorcall = native_api(
            "PyFunction_SetVectorcall", None, ctypes.py_object, ctypes.c_void_p
        )
        cls.stock_vectorcall = ctypes.cast(
            ctypes.pythonapi._PyFunction_Vectorcall, ctypes.c_void_p
        ).value
        cls.unavailable = borrowed_object_api("PySoac_GetStrictRuntimeUnavailableError")
        cls.mutation = borrowed_object_api("PySoac_GetStrictMutationError")

    def prepare(
        self, body, *, probe=None, namespace=None, call_fault=0,
        class_decision=ENFORCED_CLASS,
    ):
        import builtins

        source = ("from __future__ import strict\n" + body).encode()
        details = self.details_api(source, len(source), "<native-interpreter-hooks>", -1)
        self.assertEqual(details[2][0], 7)
        module = types.ModuleType("_native_interpreter_fixture")
        module.__dict__["__builtins__"] = builtins.__dict__
        if namespace:
            module.__dict__.update(namespace)
        fixture = self.capi.soac_interpreter_fixture(
            module, details, probe, call_fault, class_decision
        )
        return fixture, module, details

    def node(self, details, name):
        matches = [row[0] for row in details[2][1] if row[2].co_name == name]
        self.assertEqual(len(matches), 1, (name, matches))
        return matches[0]

    def execute(self, prepared):
        fixture, module, details = prepared
        self.assertIsNone(self.capi.soac_interpreter_eval(fixture, module, details[0]))
        return module

    def events(self, prepared, event=None, name=None):
        result = self.capi.soac_interpreter_events(prepared[0])
        if event is not None:
            result = [row for row in result if row["event"] == event]
        if name is not None:
            node = self.node(prepared[2], name)
            result = [row for row in result if row["node"] == node]
        return result

    def test_root_one_use_is_consumed_before_create_watcher_and_not_inherited(self):
        effects, observed, captures = [], [], []
        prepared = self.prepare("effects.append('root body')\n", namespace={"effects": effects})
        fixture, module, details = prepared

        def watch(function):
            # Assertions are recorded outside a watcher: an exception escaping a
            # watcher is unraisable, not an ordinary unittest failure.
            observed.append(("owner", self.owner(function)))
            for label, invoke in (
                ("wrapper", function),
                ("root replay", lambda: self.capi.soac_interpreter_eval(fixture, module, details[0])),
            ):
                try:
                    invoke()
                except BaseException as error:
                    observed.append((label, type(error)))
                else:
                    observed.append((label, "unexpected entry"))

        watcher = self.capi.soac_function_create_watch(
            module.__dict__, "<module>", captures, watch
        )
        try:
            self.execute(prepared)
        finally:
            self.capi.soac_function_create_unwatch(watcher)
        self.assertEqual(effects, ["root body"])
        self.assertEqual(observed, [
            ("owner", None), ("wrapper", self.unavailable), ("root replay", self.unavailable)
        ])
        self.assertEqual(len(captures), 1)
        for invoke in (
            captures[0],
            lambda: self.capi.soac_interpreter_eval(fixture, module, details[0]),
            lambda: exec(details[0], module.__dict__),
        ):
            with self.assertRaises(self.unavailable):
                invoke()
        self.assertEqual(len(self.events(prepared, self.ROOT_BEGIN)), 1)
        self.assertEqual([row["aux"] for row in self.events(prepared, self.ROOT_END)], [1])

    def test_birth_owner_precedes_create_but_completion_follows_defaults_and_decorator(self):
        observations, captures = [], []

        def decorate(function):
            observations.append(("decorator", function.__defaults__, self.sealed(function)))
            function.__defaults__ = (29,)
            observations.append(("decorator call", function()))
            return function

        prepared = self.prepare(
            "@decorate\ndef checked(value=23):\n    return value\n",
            namespace={"decorate": decorate},
        )

        def watch(function):
            observations.append(("create",
                                 self.owner(function) is not None,
                                 function.__defaults__, self.sealed(function)))
            try:
                function("bad")
            except BaseException as error:
                observations.append(("early bad", type(error)))
            else:
                observations.append(("early bad", "entered"))
            try:
                observations.append(("early good", function(17)))
            except BaseException as error:
                observations.append(("early good", type(error)))

        watcher = self.capi.soac_function_create_watch(
            prepared[1].__dict__, "checked", captures, watch
        )
        try:
            module = self.execute(prepared)
        finally:
            self.capi.soac_function_create_unwatch(watcher)
        self.assertEqual(observations, [
            ("create", True, None, 0), ("early bad", "entered"),
            ("early good", 17), ("decorator", (23,), 0), ("decorator call", 29),
        ])
        self.assertIs(module.checked, captures[0])
        self.assertGreater(self.sealed(module.checked), 0)
        self.assertEqual(module.checked(), 29)
        with self.assertRaises(self.mutation):
            module.checked.__defaults__ = (31,)

    def test_native_binder_keeps_defaults_varargs_varkwargs_without_type_predicates(self):
        body = []
        prepared = self.prepare(
            "def checked(a, /, b=23, *args, c=29, **kw):\n"
            "    body.append('body')\n"
            "    return a, b, c, args, kw\n",
            namespace={"body": body},
        )
        function = self.execute(prepared).checked
        for invoke in (lambda: function(b="bad"), lambda: function(1, 2, b=3)):
            with self.assertRaises(TypeError):
                invoke()
            self.assertEqual(self.events(prepared, self.STARTED, "checked"), [])
        self.assertEqual(body, [])
        self.assertEqual(function(5), (5, 23, 29, (), {}))
        self.assertEqual(function(5, 7, 11, 13, c=17, unused=19),
                         (5, 7, 17, (11, 13), {"unused": 19}))
        for invoke, expected in (
            (lambda: function(5, "bad"), (5, "bad", 29, (), {})),
            (lambda: function(5, 7, "bad"), (5, 7, 29, ("bad",), {})),
            (lambda: function(5, c="bad"), (5, 23, "bad", (), {})),
            (lambda: function(5, unused="bad"), (5, 23, 29, (), {"unused": "bad"})),
        ):
            before = len(body)
            self.assertEqual(invoke(), expected)
            self.assertEqual(len(body), before + 1)
        defaults = self.prepare(
            "def checked(unused='bad'):\n    body.append('wrong')\n    return 1\n",
            namespace={"body": body},
        )
        self.assertEqual(self.execute(defaults).checked(), 1)
        self.assertEqual(body[-1], "wrong")

    def test_actual_owner_survives_stock_restoration_forwarding_and_warmed_c_calls(self):
        prepared = self.prepare(
            "def checked(value):\n    return value if value >= 0 else 'bad result'\n",

        )
        function = self.execute(prepared).checked
        for entry in ("installed", "restored stock", "C forwarder"):
            with self.subTest(entry=entry):
                if entry == "restored stock":
                    self.set_vectorcall(function, self.stock_vectorcall)
                elif entry == "C forwarder":
                    self.capi.soac_interpreter_forward_entry(function)
                for number in range(32):
                    self.assertEqual(function(number), number)
                    self.assertEqual(self.c_call(function, (number,)), number)
                for invoke in (
                    lambda: function("bad"), lambda: self.c_call(function, ("bad",)),
                ):
                    # Ordinary comparison, not an annotation predicate.
                    with self.assertRaises(TypeError):
                        invoke()
                for invoke in (
                    lambda: function(-1), lambda: self.c_call(function, (-1,)),
                ):
                    self.assertEqual(invoke(), "bad result")
                self.assertIsNotNone(self.owner(function))
                self.assertGreater(self.sealed(function), 0)

    def test_copied_function_foreign_globals_and_unowned_code_never_gain_entry(self):
        effects = []
        prepared = self.prepare(
            "def checked(value):\n    effects.append(value)\n    return value\n",
            namespace={"effects": effects},
        )
        function = self.execute(prepared).checked
        copies = (
            types.FunctionType(function.__code__, prepared[1].__dict__),
            types.FunctionType(function.__code__, {"effects": effects}),
            types.FunctionType(function.__code__.replace(), prepared[1].__dict__),
        )
        for copy in copies:
            self.assertIsNone(self.owner(copy))
            for invoke in (lambda: copy(11), lambda: self.c_call(copy, (11,))):
                with self.assertRaises(self.unavailable):
                    invoke()
        self.assertEqual(effects, [])
        foreign = types.ModuleType("_foreign_interpreter_fixture")
        with self.assertRaises(self.unavailable):
            self.capi.soac_interpreter_eval(prepared[0], foreign, prepared[2][0])
        self.assertEqual(function(17), 17)
        self.assertEqual(effects, [17])

    def test_failed_completion_follows_finally_restores_handler_and_closes_result_once(self):
        effects, local_refs, holder = [], [], {}
        completion_error = TypeError("exact borrowed completion failure")
        outer_error = RuntimeError("caller handled exception")

        class Local:
            def __init__(self):
                local_refs.append(weakref.ref(self))

            def __del__(self):
                effects.append("local released")

        class Result:
            def __del__(self):
                effects.append(("result released", holder["module"].helper(19)))

        def probe(event, node, serial):
            if event == self.RETURN and node == holder.get("checked"):
                effects.append(("completion", sys.exception() is outer_error,
                                local_refs[-1]() is not None))
                raise completion_error

        prepared = self.prepare(
            "def helper(value):\n    return value\n"
            "def checked():\n"
            "    local = Local()\n"
            "    try:\n"
            "        try:\n"
            "            raise ValueError('inner')\n"
            "        except ValueError:\n"
            "            try:\n"
            "                return Result()\n"
            "            finally:\n"
            "                effects.append('finally')\n"
            "    except TypeError:\n"
            "        effects.append('callee caught return check')\n"
            "        return 0\n",

            probe=probe, namespace={"Local": Local, "Result": Result, "effects": effects},
        )
        holder["checked"] = self.node(prepared[2], "checked")
        holder["module"] = self.execute(prepared)
        try:
            raise outer_error
        except RuntimeError:
            try:
                holder["module"].checked()
            except TypeError as caught:
                self.assertIs(caught, completion_error)
            else:
                self.fail("native completion failure was not propagated")
        self.assertEqual(effects, [
            "finally", ("completion", True, True), ("result released", 19)
        ])
        returned = self.events(prepared, self.RETURN, "checked")
        self.assertEqual(len(returned), 1)
        self.assertEqual(returned[0]["refcount"], 1)  # Original result token only.
        self.assertEqual(len(self.events(prepared, self.RETURN, "helper")), 1)
        self.assertEqual(len(self.events(prepared, self.LEAVE, "checked")), 1)
        # An actual exception traceback retains ordinary locals. Do not claim
        # immediate local destruction while deliberately retaining that frame.
        self.assertIsNotNone(local_refs[0]())
        completion_error.__traceback__ = None
        gc.collect()
        self.assertIsNone(local_refs[0]())
        self.assertEqual(effects[-1], "local released")

    def test_body_error_is_unchanged_and_successful_return_identity_is_not_normalized(self):
        effects = []
        marker = ValueError("exact native body error")
        prepared = self.prepare(
            "def checked(value):\n    return value\n"
            "def failed():\n"
            "    try:\n        raise marker\n"
            "    finally:\n        effects.append('body finally')\n",
            
            namespace={"effects": effects, "marker": marker},
        )
        module = self.execute(prepared)
        value = object()
        self.assertIs(module.checked(value), value)
        with self.assertRaises(ValueError) as caught:
            module.failed()
        self.assertIs(caught.exception, marker)
        self.assertEqual(effects, ["body finally"])
        self.assertEqual(self.events(prepared, self.RETURN, "failed"), [])
        self.assertEqual(len(self.events(prepared, self.LEAVE, "failed")), 1)
        marker.__traceback__ = None

    def test_native_keyword_comparison_callback_preserves_owner_and_ordinary_values(self):
        prepared = self.prepare("def checked(value):\n    return value\n")
        function = self.execute(prepared).checked
        comparisons, owners = [], []
        owner = self.owner
        original_owner = owner(function)

        class Keyword(str):
            __hash__ = str.__hash__

            def __eq__(self, other):
                comparisons.append(other)
                owners.append(owner(function))
                return str.__eq__(self, other)

        self.assertEqual(function(**{Keyword("value"): "ordinary first call"}), "ordinary first call")
        self.assertTrue(comparisons)
        self.assertEqual(owners, [original_owner] * len(comparisons))
        self.assertIsNotNone(original_owner)
        self.assertEqual(function("ordinary next call"), "ordinary next call")
        self.assertEqual(function(31), 31)

    def test_actual_class_is_bound_before_ready_callbacks_and_namespace_refs_match_native(self):
        def run(strict):
            import builtins

            captures, observations, blocked = [], [], []

            def observe(kind, actual_type):
                observations.append((kind, sys.getrefcount(captures[0])))
                if strict:
                    blocked.append(self.type_contract(actual_type))
                    previous_lookup = actual_type.__getattribute__
                    try:
                        actual_type.__getattribute__ = lambda obj, name: None
                    except BaseException as error:
                        blocked.append(type(error))
                    else:
                        blocked.append("mutation entered")
                    self.assertIs(actual_type.__getattribute__, previous_lookup)

            class Descriptor:
                def __set_name__(self, owner, name):
                    observe("set_name", owner)

            body = (
                "class Base:\n"
                "    def __init_subclass__(cls):\n"
                "        observe('init_subclass', cls)\n"
                "\n"
                "class Example(Base):\n"
                "    locked = 1\n"
                "    member = descriptor\n"
            )
            namespace = {"observe": observe, "descriptor": Descriptor()}
            if strict:
                prepared = self.prepare(body, namespace=namespace)
                module = prepared[1]
            else:
                prepared = None
                module = types.ModuleType("_ordinary_class_owner_oracle")
                module.__dict__.update(namespace, __builtins__=builtins.__dict__)
            watcher = self.capi.soac_function_create_watch(
                module.__dict__, "Example", captures
            )
            try:
                if strict:
                    self.execute(prepared)
                else:
                    exec(compile(body, "<ordinary-class-owner>", "exec", dont_inherit=True),
                         module.__dict__)
            finally:
                self.capi.soac_function_create_unwatch(watcher)
            self.assertEqual(len(captures), 1)
            self.assertEqual(self.type_contract(module.Base), int(strict))
            self.assertEqual([kind for kind, _ in observations], ["set_name", "init_subclass"])
            if strict:
                self.assertEqual(blocked, [1, self.mutation, 1, self.mutation])
                bound = self.events(prepared, self.BIND, "Example")
                self.assertEqual(len(bound), 1)
                self.assertEqual(bound[0]["value_id"], id(module.Example))
                self.assertEqual(bound[0]["aux"], 0)  # Not Py_TPFLAGS_READY yet.
                self.assertEqual(len(self.events(prepared, self.PREPARE, "Example")), 1)
                self.assertEqual(module.Example.locked, 1)
                with self.assertRaises(self.mutation):
                    module.Example.locked = 3
            return observations

        ordinary = run(False)
        interpreter = run(True)
        # Compare the actual callback-time namespace-function owners; moving
        # the base into source must not hide an extra construction-handle edge.
        self.assertEqual(interpreter, ordinary)

    def test_external_unprotected_base_remains_rejected_before_type_binding(self):
        class ExternalBase:
            pass

        prepared = self.prepare(
            "class Example(Base):\n    locked = 1\n",
            namespace={"Base": ExternalBase},
        )
        with self.assertRaisesRegex(self.mutation, "requires protected bases"):
            self.execute(prepared)
        self.assertNotIn("Example", prepared[1].__dict__)
        self.assertEqual(self.type_contract(ExternalBase), 0)
        self.assertEqual(self.events(prepared, self.BIND, "Example"), [])
        ExternalBase.dynamic = 7
        self.assertEqual(ExternalBase.dynamic, 7)

    def test_class_decorator_replacement_does_not_revoke_original_policy(self):
        originals = []

        class Replacement:
            pass

        def replace(original):
            originals.append(original)
            return Replacement

        prepared = self.prepare(
            "@replace\nclass Example:\n    locked = 1\n",
            namespace={"replace": replace},
        )
        module = self.execute(prepared)
        self.assertIs(module.Example, Replacement)
        self.assertEqual(self.type_contract(Replacement), 0)
        self.assertEqual(len(originals), 1)
        self.assertEqual(self.type_contract(originals[0]), 1)
        # The C fixture seals only the actual final Store value. It does not
        # implement the Rust pending registry's discarded-original completion.
        # The already-installed lookup policy must still remain intact.
        previous_lookup = originals[0].__getattribute__
        with self.assertRaises(self.mutation):
            originals[0].__getattribute__ = lambda obj, name: None
        self.assertIs(originals[0].__getattribute__, previous_lookup)
        Replacement.dynamic = 3
        self.assertEqual(Replacement.dynamic, 3)

    def test_unadmitted_metaclass_declines_before_policy_installation(self):
        effects = []

        class Meta(type):
            def __new__(metaclass, name, bases, namespace):
                effects.append("metaclass")
                return super().__new__(metaclass, name, bases, namespace)

        prepared = self.prepare(
            "class Example(metaclass=Meta):\n    locked = 1\n",
            namespace={"Meta": Meta},
        )
        module = self.execute(prepared)
        self.assertEqual(effects, ["metaclass"])
        self.assertEqual(self.type_contract(module.Example), 0)
        self.assertEqual(self.events(prepared, self.BIND), [])
        module.Example.locked = 2
        self.assertEqual(module.Example.locked, 2)

    def test_no_store_lambda_has_birth_owner_without_fake_definition_completion(self):
        prepared = self.prepare(
            "def factory():\n    return lambda value: value\n",

        )
        function = self.execute(prepared).factory()
        self.assertIsNotNone(self.owner(function))
        self.assertEqual(function(37), 37)
        self.assertEqual(function("bad"), "bad")
        # This C fixture has no pending-definition finalizer. It must not claim
        # final source immutability from birth alone or invent a FUNCTION Store
        # for a lambda. The real Rust pending registry is a separate obligation.
        self.assertEqual(self.sealed(function), 0)

    def test_loop_store_operand_remains_published_across_gc_with_tagged_state(self):
        import gc

        holder, probes = {}, []

        def probe(event, node, serial):
            if event == self.STORE and node == holder["loop"]:
                gc.collect()
                probes.append(serial)

        prepared = self.prepare(
            "def loop():\n"
            "    seen = []\n"
            "    for value in (3, 5):\n"
            "        seen.append(value)\n"
            "    return seen\n",
            probe=probe,
        )
        holder["loop"] = self.node(prepared[2], "loop")
        result = self.execute(prepared).loop()
        self.assertEqual(result, [3, 5])
        rows = self.events(prepared, self.STORE, "loop")
        # Actual GET_ITER retains its tuple and tagged index below each loop
        # target. The real Store value must still be found on both sides of GC.
        self.assertEqual(len(rows), 3)
        self.assertEqual([row["aux"] for row in rows], [3, 3, 3])
        self.assertEqual([row["value_id"] for row in rows],
                         [id(result), id(3), id(5)])
        self.assertEqual(probes, [row["serial"] for row in rows])

    def test_paired_store_callbacks_follow_actual_publish_then_displaced_close_order(self):
        retired, holder = [], {}
        read_events, store_event = self.events, self.STORE

        class Old:
            def __init__(self, label):
                self.label = label

            def __del__(self):
                rows = read_events(holder["prepared"], store_event, "replace")
                row = rows[-1]
                retired.append((self.label, row["ordinal"], row["lane"], row["aux"]))

        prepared = self.prepare(
            "def replace():\n"
            "    left = Old('left')\n"
            "    right = Old('right')\n"
            "    left, right = new_pair()\n"
            "    return left, right\n",
            namespace={"Old": Old, "new_pair": lambda: (41, 43)},
        )
        holder["prepared"] = prepared
        module = self.execute(prepared)
        node = self.node(prepared[2], "replace")
        code = prepared[2][2][1][node][2]
        stores = prepared[2][2][3][node][4]
        expected = []
        for origin, emissions in stores:
            for emission in emissions:
                ordinal, form, first, second, lane, context = emission
                if (form == 1 and first == (0, code.co_varnames.index("left"))
                        and second == (0, code.co_varnames.index("right"))):
                    expected.append((ordinal, lane))
        self.assertEqual(len(expected), 2, "the actual source must produce the paired Store control")
        self.assertEqual(sorted(lane for _, lane in expected), [0, 1])
        self.assertEqual(len({ordinal for ordinal, _ in expected}), 1)
        self.assertEqual(module.replace(), (41, 43))
        ordinal = expected[0][0]
        self.assertEqual(retired, [("left", ordinal, 0, 3), ("right", ordinal, 1, 3)])
        emitted = [row for row in self.events(prepared, self.STORE, "replace")
                   if row["ordinal"] == ordinal]
        self.assertEqual([row["lane"] for row in emitted], [0, 1])
        self.assertEqual([row["value_id"] for row in emitted], [id(41), id(43)])

    def test_extended_arg_definition_coordinate_is_native_and_stack_survives_gc_reentry(self):
        holder, probes = {}, []
        locals_prefix = "".join(f"    value_{index} = {index}\n" for index in range(270))
        source = (
            "def helper(value):\n    return value\n"
            "def build():\n" + locals_prefix +
            "    def made(value):\n        return value\n"
            "    return made\n"
        )

        def probe(event, node, serial):
            if event != self.STORE or node != holder.get("build"):
                return
            row = self.events(holder["prepared"], self.STORE, "build")[-1]
            if row["ordinal"] != holder["definition"]:
                return
            gc.collect()
            probes.append(holder["module"].helper(47))

        prepared = self.prepare(
            source, 
            probe=probe,
        )
        holder["prepared"] = prepared
        holder["build"] = self.node(prepared[2], "build")
        holder["module"] = self.execute(prepared)
        table = prepared[2][2][3][holder["build"]]
        definitions = [emission for origin, emissions in table[4]
                       if origin[0] == 1 for emission in emissions]
        self.assertEqual(len(definitions), 1)
        holder["definition"] = definitions[0][0]

        def trace(frame, event, arg):
            return trace

        previous = sys.gettrace()
        try:
            # Exercise native base-op traversal under actual instrumentation.
            sys.settrace(trace)
            function = holder["module"].build()
        finally:
            sys.settrace(previous)
        self.assertEqual(probes, [47])
        self.assertEqual(function(53), 53)
        self.assertGreater(self.sealed(function), 0)
        rows = [row for row in self.events(prepared, self.STORE, "build")
                if row["ordinal"] == holder["definition"]]
        self.assertEqual(len(rows), 1)
        row = rows[0]
        self.assertEqual(row["lane"], definitions[0][4])
        self.assertEqual(row["aux"], 3)  # Operand is in published stack both sides.
        self.assertEqual(row["value_id"], id(function))
        code = prepared[2][2][1][holder["build"]][2]
        self.assertGreater(row["units"], row["ordinal"])
        self.assertEqual(code.co_code[2 * row["units"] - 2], opcode.opmap["EXTENDED_ARG"])
        self.assertEqual(code.co_code[2 * row["units"]], opcode.opmap["STORE_FAST"])

    def test_escaped_ordinary_frame_has_no_second_checked_activation_owner(self):
        prepared = self.prepare(
            "def checked(value):\n    return sys._getframe()\n",
            namespace={"sys": sys},
        )
        module = self.execute(prepared)
        value = object()
        frame = module.checked(value)
        self.assertIs(type(frame), types.FrameType)
        code = prepared[2][2][1][self.node(prepared[2], "checked")][2]
        self.assertIs(frame.f_code, code)
        self.assertIs(frame.f_globals, module.__dict__)
        self.assertIs(frame.f_locals["value"], value)
        offsets = self.capi.soac_dataclass_frame_offsets()
        # The existing native offset probe, not a handwritten frame mirror.
        # Returned frame now owns _f_frame_data; it remains strongly held here.
        address = id(frame) + offsets["frame_object_data_offset"] + offsets["checked_activation"]
        self.assertIsNone(ctypes.c_void_p.from_address(address).value)
        self.assertEqual(len(self.events(prepared, self.LEAVE, "checked")), 1)
        frame.clear()
        self.assertIsNone(ctypes.c_void_p.from_address(address).value)

    def test_failed_root_is_not_reusable_and_escaped_children_do_not_outlive_admission(self):
        effects = []
        marker = RuntimeError("root body failed")
        prepared = self.prepare(
            "def child():\n    effects.append('child entered')\n"
            "effects.append('root entered')\n"
            "raise marker\n",
            namespace={"effects": effects, "marker": marker},
        )
        fixture, module, details = prepared
        with self.assertRaises(RuntimeError) as caught:
            self.capi.soac_interpreter_eval(fixture, module, details[0])
        self.assertIs(caught.exception, marker)
        self.assertEqual([row["aux"] for row in self.events(prepared, self.ROOT_END)], [0])
        self.assertEqual(effects, ["root entered"])
        for invoke in (
            module.child,
            lambda: self.capi.soac_interpreter_eval(fixture, module, details[0]),
        ):
            with self.assertRaises(self.unavailable):
                invoke()
        self.assertEqual(effects, ["root entered"])
        marker.__traceback__ = None


    def test_unchecked_ordinary_code_replacement_keeps_dispatch_without_source_authority(self):
        set_owner = native_api(
            "PyFunction_SetSoacStrictOwner", ctypes.c_int, ctypes.py_object, ctypes.py_object
        )
        for entry in ("restored stock", "C forwarder"):
            with self.subTest(entry=entry):
                effects, captures, observed = [], [], []
                dynamic = {"effects": effects}
                exec(compile(
                    "def replacement(value):\n"
                    "    effects.append(value)\n"
                    "    def child():\n        return value\n"
                    "    return value, child\n",
                    "<ordinary-code-replacement>", "exec", dont_inherit=True,
                ), dynamic)
                replacement_code = dynamic["replacement"].__code__
                self.assertFalse(replacement_code.co_flags & STRICT)
                prepared = self.prepare(
                    "def pending(value):\n    return value\n",
                    namespace={"effects": effects},
                )

                def generic(value):
                    return value

                # A generic owner on ordinary code is not the new native
                # interpreter-birth kind. Its ordinary behavior is unchanged.
                generic_owner = object()
                self.assertEqual(generic("before"), "before")
                self.assertEqual(set_owner(generic, generic_owner), 0)
                self.assertEqual(self.owner(generic), id(generic_owner))
                self.assertEqual(generic("after"), "after")
                self.assertEqual(self.events(prepared), [])

                def watch(function):
                    observed.append(("create",
                                     self.sealed(function), self.owner(function) is not None))
                    try:
                        if entry == "restored stock":
                            self.set_vectorcall(function, self.stock_vectorcall)
                        else:
                            self.capi.soac_interpreter_forward_entry(function)
                        function.__code__ = replacement_code
                        value, child = function("early dynamic")
                        observed.append(("call", value, child(), self.owner(child)))
                    except BaseException as error:
                        observed.append(("failure", type(error)))

                watcher = self.capi.soac_function_create_watch(
                    prepared[1].__dict__, "pending", captures, watch
                )
                try:
                    module = self.execute(prepared)
                finally:
                    self.capi.soac_function_create_unwatch(watcher)
                self.assertEqual(observed, [
                    ("create", 0, True), ("call", "early dynamic", "early dynamic", None),
                ])
                self.assertEqual(len(captures), 1)
                self.assertIs(module.pending, captures[0])
                self.assertIs(module.pending.__code__, replacement_code)
                self.assertGreater(self.sealed(module.pending), 0)
                # Unsealed is a replacement-time condition. Ordinary calls
                # remain legal after final source publication freezes this code.
                value, child = self.c_call(module.pending, ("late dynamic",))
                self.assertEqual((value, child()), ("late dynamic", "late dynamic"))
                self.assertIsNone(self.owner(child))
                self.assertEqual(effects, ["early dynamic", "late dynamic"])
                entered = self.events(prepared, self.ENTER, "pending")
                self.assertEqual(len(entered), 2)
                self.assertEqual([row["aux"] for row in entered], [1, 1])
                self.assertEqual([row["value_id"] for row in entered], [id(replacement_code)] * 2)
                self.assertEqual(self.events(prepared, self.RETURN, "pending"), [])
                # No source receipt for the ordinary replacement's internal
                # child definition; no new source owner at ordinary birth.
                self.assertEqual(self.events(prepared, self.STORE, "pending"), [])
                self.assertEqual(len(self.events(prepared, self.BIRTH)), 1)
                with self.assertRaises(self.mutation):
                    module.pending.__code__ = replacement_code

    def test_strict_code_transplant_refuses_while_ordinary_replacement_waits_for_sealing(self):
        for entry in ("restored stock", "C forwarder"):
            with self.subTest(entry=entry):
                effects, captures, observed = [], [], []
                prepared = self.prepare(
                    "def donor(value):\n    effects.append('donor body')\n    return value\n"
                    "def pending(value):\n    return value\n",
                    namespace={"effects": effects},
                )
                donor_code = prepared[2][2][1][self.node(prepared[2], "donor")][2]

                def watch(function):
                    try:
                        if entry == "restored stock":
                            self.set_vectorcall(function, self.stock_vectorcall)
                        else:
                            self.capi.soac_interpreter_forward_entry(function)
                        function.__code__ = donor_code
                        observed.append(("assigned", function.__code__ is donor_code))
                        function(1)
                    except BaseException as error:
                        observed.append(("call", type(error)))
                    else:
                        observed.append(("call", "unexpected donor body"))

                watcher = self.capi.soac_function_create_watch(
                    prepared[1].__dict__, "pending", captures, watch
                )
                try:
                    with self.assertRaises(self.unavailable):
                        self.execute(prepared)
                finally:
                    self.capi.soac_function_create_unwatch(watcher)
                self.assertEqual(observed, [("assigned", True), ("call", self.unavailable)])
                self.assertEqual(effects, [])
                self.assertNotIn("pending", prepared[1].__dict__)
                self.assertEqual(len(captures), 1)
                self.assertEqual(self.sealed(captures[0]), 0)
                self.assertEqual([row["aux"] for row in self.events(prepared, self.ROOT_END)], [0])

        def ordinary(value):
            return value

        captures, observed = [], []
        prepared = self.prepare(
            "def checked(value):\n    return value\n", 
        )

        def watch_checked(function):
            original = function.__code__
            try:
                function.__code__ = ordinary.__code__
            except BaseException as error:
                observed.append((type(error), function.__code__ is original, self.sealed(function)))
            else:
                observed.append(("ordinary replacement", function.__code__ is ordinary.__code__,
                                 self.sealed(function)))

        watcher = self.capi.soac_function_create_watch(
            prepared[1].__dict__, "checked", captures, watch_checked
        )
        try:
            function = self.execute(prepared).checked
        finally:
            self.capi.soac_function_create_unwatch(watcher)
        self.assertEqual(observed, [("ordinary replacement", True, 0)])
        self.assertEqual(function(59), 59)
        self.assertGreater(self.sealed(function), 0)
        with self.assertRaises(self.mutation):
            function.__code__ = ordinary.__code__


    def test_external_eval_frame_refuses_source_but_native_observers_remain_supported(self):
        effects = []
        prepared = self.prepare(
            "def checked(value):\n"
            "    effects.append(value)\n"
            "    return value if value >= 0 else 'bad result'\n",
            namespace={"effects": effects},
        )
        function = self.execute(prepared).checked
        root_effects = []
        new_root = self.prepare("root_effects.append('entered')\n",
                                namespace={"root_effects": root_effects})
        recorded = []

        def ordinary():
            return 61

        self.capi.set_eval_frame_record(recorded)
        try:
            self.assertEqual(ordinary(), 61)
            with self.assertRaises(self.unavailable):
                function(1)
            with self.assertRaises(self.unavailable):
                self.execute(new_root)
        finally:
            self.capi.set_eval_frame_default()
        self.assertIn("ordinary", recorded)
        self.assertNotIn("checked", recorded)  # Refuse BEFORE external dispatch.
        self.assertNotIn("<module>", recorded)
        self.assertEqual(effects, [])
        self.assertEqual(root_effects, [])
        self.assertEqual(function(67), 67)

        # Monitoring is part of the normal native VM, unlike an external frame
        # evaluator. Both ordinary result types notify it.
        monitoring = sys.monitoring
        tool = next((candidate for candidate in range(6)
                     if monitoring.get_tool(candidate) is None), None)
        self.assertIsNotNone(tool, "native monitoring control requires a free test tool id")
        observed = []
        source_code = function.__code__

        def on_return(code, offset, value):
            if code is source_code:
                observed.append((offset, id(value)))

        monitoring.use_tool_id(tool, "strict-interpreter-hook-control")
        try:
            monitoring.register_callback(tool, monitoring.events.PY_RETURN, on_return)
            monitoring.set_events(tool, monitoring.events.PY_RETURN)
            self.assertEqual(function(71), 71)
            other_result = function(-1)
            self.assertEqual(other_result, "bad result")
        finally:
            monitoring.set_events(tool, 0)
            monitoring.register_callback(tool, monitoring.events.PY_RETURN, None)
            monitoring.free_tool_id(tool)
        self.assertEqual(len(observed), 2)
        self.assertEqual([address for _, address in observed], [id(71), id(other_result)])
        self.assertEqual(effects, [67, 71, -1])
        # The EXTENDED_ARG test separately exercises an actual tracing callback.

    def test_started_is_committed_vm_entry_not_binding_or_eval_dispatch(self):
        observations = []
        holder = []

        def observe():
            observations.append(len(self.events(holder[0], self.STARTED, "checked")))

        prepared = self.prepare(
            "def checked(value):\n    observe()\n    return value\n",
            namespace={"observe": observe},
        )
        holder.append(prepared)
        function = self.execute(prepared).checked
        with self.assertRaises(TypeError):
            function()
        self.assertEqual(self.events(prepared, self.STARTED, "checked"), [])
        self.capi.set_eval_frame_record([])
        try:
            with self.assertRaises(self.unavailable):
                function(1)
        finally:
            self.capi.set_eval_frame_default()
        self.assertEqual(self.events(prepared, self.STARTED, "checked"), [])
        self.assertEqual(observations, [])
        self.assertEqual(function(73), 73)
        self.assertEqual(observations, [1])  # visible before the first source body callback
        started = self.events(prepared, self.STARTED, "checked")
        self.assertEqual([row["aux"] for row in started], [1])
        returned = self.events(prepared, self.RETURN, "checked")
        self.assertEqual(returned[0]["serial"], started[0]["serial"])
        self.assertEqual(function("wrong"), "wrong")
        self.assertEqual(observations, [1, 2])

    def test_attribute_records_actual_same_activation_provider_before_decorator(self):
        originals, decorated = [], []

        def decorate(function):
            provider = function.__annotate__
            originals.append(provider)
            if len(originals) == 2:
                function.__annotate__ = originals[0]
            decorated.append((id(function), id(provider), id(function.__annotate__)))
            return function

        prepared = self.prepare(
            "def factory():\n"
            "    result = []\n"
            "    for ordinal in (0, 1):\n"
            "        @decorate\n"
            "        def convert(value: Target) -> Target:\n"
            "            return value\n"
            "        result.append(convert)\n"
            "    return result\n",
            namespace={"decorate": decorate, "Target": int},
        )
        functions = self.execute(prepared).factory()
        self.assertEqual(len(functions), 2)
        self.assertIsNot(originals[0], originals[1])
        self.assertIs(originals[0].__code__, originals[1].__code__)
        self.assertIs(functions[1].__annotate__, originals[0])  # deliberate legal pre-seal transplant
        rows = self.events(prepared, self.ATTRIBUTE, "convert")
        annotate = [row for row in rows if row["aux"] == 0x10]  # actual MAKE_FUNCTION_ANNOTATE
        self.assertEqual(len(annotate), 2)
        self.assertEqual(annotate[0]["serial"], annotate[1]["serial"])
        self.assertEqual([row["value_id"] for row in annotate], [id(f) for f in functions])
        self.assertEqual([row["slots"][0][1] for row in annotate], [id(p) for p in originals])
        self.assertEqual(decorated, [
            (id(functions[0]), id(originals[0]), id(originals[0])),
            (id(functions[1]), id(originals[1]), id(originals[0])),
        ])
        # This kernel proves the actual publication witness, not production
        # nominal resolution. The authenticated Rust owner must use that witness.

    def test_failed_completion_restores_primary_and_terminalizes_only_pending_children(self):
        for completion_fails in (False, True):
            with self.subTest(completion_fails=completion_fails):
                primary = LookupError("primary body error")
                context = OSError("caller handled context")
                secondary = RuntimeError("secondary pending completion")
                effects, pending, observed, holder = [], [], [], []

                def probe(event, node, serial):
                    if event == self.FAILED and completion_fails:
                        self.assertEqual(node, self.node(holder[0][2], "broken"))
                        raise secondary

                source = (
                    "def helper(value):\n    return value\n"
                    "def broken():\n"
                    "    pending.append(lambda: 'still pending')\n"
                    "    try:\n"
                    "        raise primary\n"
                    "    finally:\n"
                    "        effects.append('finally')\n"
                )
                prepared = self.prepare(
                    source, probe=probe,
                    namespace={"pending": pending, "primary": primary, "effects": effects},
                )
                holder.append(prepared)
                module = self.execute(prepared)

                def unraisable(args):
                    # Store only already-held exception identities and scalars,
                    # not the hook args/traceback/frame cycle.
                    entry = [args.exc_value is secondary, args.object is module.broken]
                    try:
                        pending[0]()
                    except BaseException as error:
                        entry.append(type(error))
                    else:
                        entry.append("unexpected pending entry")
                    entry.append(module.helper(79))
                    observed.append(tuple(entry))

                previous = sys.unraisablehook
                sys.unraisablehook = unraisable
                try:
                    try:
                        raise context
                    except OSError:
                        with self.assertRaises(LookupError) as caught:
                            module.broken()
                        self.assertIs(caught.exception, primary)
                        self.assertIs(primary.__context__, context)
                finally:
                    sys.unraisablehook = previous
                self.assertEqual(effects, ["finally"])
                self.assertEqual(len(self.events(prepared, self.FAILED, "broken")), 1)
                self.assertEqual(self.events(prepared, self.RETURN, "broken"), [])
                self.assertGreater(self.sealed(module.helper), 0)
                self.assertEqual(module.helper(83), 83)
                if completion_fails:
                    self.assertEqual(observed, [(True, True, self.unavailable, 79)])
                    with self.assertRaises(self.unavailable):
                        pending[0]()
                else:
                    self.assertEqual(observed, [])
                    self.assertEqual(pending[0](), "still pending")

        # Ordinary control: same source handlers/finally and caller exception
        # identity/context, without a callback table or source authority.
        ordinary_primary, ordinary_context = LookupError("ordinary"), OSError("caller")
        ordinary_effects = []
        namespace = {"pending": [], "primary": ordinary_primary, "effects": ordinary_effects}
        exec(source, namespace)
        try:
            raise ordinary_context
        except OSError:
            with self.assertRaises(LookupError) as caught:
                namespace["broken"]()
            self.assertIs(caught.exception, ordinary_primary)
            self.assertIs(caught.exception.__context__, ordinary_context)
        self.assertEqual(ordinary_effects, ["finally"])

    def test_success_completes_metadata_without_a_return_predicate(self):
        sentinel = object()
        prepared = self.prepare(
            "def unchecked():\n    return sentinel\n",
            namespace={"sentinel": sentinel},
        )
        function = self.execute(prepared).unchecked
        self.assertIs(function(), sentinel)
        returned = self.events(prepared, self.RETURN, "unchecked")
        self.assertEqual(len(returned), 1)
        self.assertEqual(returned[0]["value_id"], id(sentinel))
        self.assertEqual(len(self.events(prepared, self.STARTED, "unchecked")), 1)

    def test_started_on_generator_throw_preserves_exact_incoming_exception(self):
        source = (
            "def generator():\n"
            "    try:\n"
            "        yield 'ready'\n"
            "    except LookupError as error:\n"
            "        seen.append(error)\n"
            "        yield error\n"
        )
        ordinary_seen = []
        ordinary = {"seen": ordinary_seen}
        exec(source, ordinary)
        marker = LookupError("ordinary throw")
        control = ordinary["generator"]()
        self.assertEqual(next(control), "ready")
        self.assertIs(control.throw(marker), marker)
        self.assertEqual(ordinary_seen, [marker])
        control.close()

        seen = []
        prepared = self.prepare(source, namespace={"seen": seen})
        generator = self.execute(prepared).generator()
        marker = LookupError("strict throw")
        self.assertEqual(next(generator), "ready")
        self.assertIs(generator.throw(marker), marker)
        self.assertEqual(seen, [marker])
        rows = self.events(prepared, self.STARTED, "generator")
        self.assertGreaterEqual(len(rows), 2)  # creation plus real native resume(s)
        self.assertEqual(len({row["serial"] for row in rows}), 1)
        self.assertEqual(sum(row["aux"] == 1 for row in rows), 1)
        generator.close()



    def test_interpreter_call_view_actual_vector_keyword_expanded_operands(self):
        first, second = object(), object()
        prepared = self.prepare(
            "def target(a, b):\n    return a, b\n"
            "pos = target(first, second)\n"
            "kw = target(b=second, a=first)\n"
            "ex = target(*(first,), **{'b': second})\n",
            namespace={"first": first, "second": second},
        )
        module = self.execute(prepared)
        self.assertEqual((module.pos, module.kw, module.ex), ((first, second),) * 3)
        rows = [
            row for row in self.events(prepared, 14)
            if row["value_id"] == id(module.target)
        ]
        self.assertEqual(len(rows), 3)
        self.assertEqual([row["call_form"] for row in rows], [1, 2, 3])
        self.assertEqual([row["channel"] for row in rows], [0, 0, 0])
        self.assertEqual([(row["positional"], row["keywords"]) for row in rows],
                         [(2, 0), (0, 2), (1, 1)])
        self.assertEqual([[entry[1] for entry in row["slots"]] for row in rows],
                         [[id(first), id(second)], [id(second), id(first)], [id(first)]])
        self.assertTrue(all(row["call_flags"] == 15 for row in rows))
        self.assertTrue(all(row["ordinal"] >= 0 and row["units"] >= 0 for row in rows))

    def test_interpreter_call_view_method_channel_and_warmed_native_calls(self):
        prepared = self.prepare(
            "class Holder:\n"
            "    def get(self, value):\n        return value\n"
            "holder = Holder()\n"
            "def drive(value):\n    return holder.get(value)\n",
        )
        module = self.execute(prepared)
        value = object()
        # This exceeds native specialization warmup without overflowing the
        # fixture's bounded complete enter/call/return event inventory.
        for _ in range(128):
            self.assertIs(module.drive(value), value)
        rows = [row for row in self.events(prepared, 14, "drive")
                if row["value_id"] == id(module.Holder.get)]
        self.assertEqual(len(rows), 128)
        self.assertTrue(all(row["channel"] == 1 and row["positional"] == 2
                            and row["keywords"] == 0 for row in rows))
        self.assertTrue(all([entry[1] for entry in row["slots"]]
                            == [id(module.holder), id(value)] for row in rows))


    def test_interpreter_expanded_shared_ordinary_code_warm_replacement_keeps_no_source_grant(self):
        import dis

        def identity(value):
            return value

        payload = (object(),)
        for target, expected_opcode in (
            (identity, "CALL_EX_PY"),
            (tuple, "CALL_EX_NON_PY_GENERAL"),
        ):
            with self.subTest(opcode=expected_opcode):
                ordinary_globals = {}
                exec(compile(
                    "def drive(target, args):\n    return target(*args)\n",
                    "<ordinary-shared-expanded-call>", "exec", dont_inherit=True,
                ), ordinary_globals)
                code = ordinary_globals["drive"].__code__
                ordinary_copy = types.FunctionType(code, ordinary_globals)
                self.assertFalse(code.co_flags & STRICT)
                self.assertIsNone(self.owner(ordinary_copy))
                for _ in range(128):
                    self.assertIs(ordinary_copy(target, (payload,)), payload)
                # Observe the actual warmed opcode. This is not a synthetic
                # specialization receipt or an authenticated source-code copy.
                self.assertIn(expected_opcode, [
                    instruction.opname
                    for instruction in dis.get_instructions(code, adaptive=True)
                ])

                captures, failures = [], []
                prepared = self.prepare(
                    "def pending(target, args):\n    return target(*args)\n",
                    
                )

                def watch(function):
                    try:
                        # The existing documented ordinary replacement remains
                        # ordinary, even though its birth owner is authenticated.
                        function.__code__ = code
                    except BaseException as error:
                        failures.append(type(error))

                watcher = self.capi.soac_function_create_watch(
                    prepared[1].__dict__, "pending", captures, watch
                )
                try:
                    module = self.execute(prepared)
                finally:
                    self.capi.soac_function_create_unwatch(watcher)
                self.assertEqual(failures, [])
                self.assertEqual(len(captures), 1)
                self.assertIs(module.pending.__code__, ordinary_copy.__code__)
                self.assertIsNotNone(self.owner(module.pending))
                self.assertIs(module.pending(target, (payload,)), payload)
                self.assertIs(self.c_call(module.pending, (target, (payload,))), payload)
                entered = self.events(prepared, self.ENTER, "pending")
                self.assertEqual([row["value_id"] for row in entered], [id(code)] * 2)
                self.assertEqual([row["aux"] for row in entered], [1, 1])
                self.assertEqual(self.events(prepared, self.RETURN, "pending"), [])
                self.assertEqual(self.events(prepared, self.STORE, "pending"), [])
                self.assertEqual(self.events(prepared, 14, "pending"), [])
                self.assertEqual(len(self.events(prepared, self.BIRTH)), 1)
                self.assertIs(ordinary_copy(target, (payload,)), payload)

    def test_interpreter_call_selection_failure_preserves_original_inputs_and_error(self):
        effects = []
        failure = RuntimeError("actual call selection")
        armed = False

        def probe(event, node, serial):
            if armed and event == 14:
                raise failure

        prepared = self.prepare(
            "def target(value):\n    effects.append(value)\n    return value\n"
            "def drive(value):\n    return target(value)\n",
            namespace={"effects": effects}, probe=probe,
        )
        module = self.execute(prepared)
        value = object()
        before = sys.getrefcount(value)
        armed = True
        try:
            module.drive(value)
        except RuntimeError as caught:
            self.assertIs(caught, failure)
        else:
            self.fail("actual native selection did not refuse")
        armed = False
        failure.__traceback__ = None
        self.assertEqual(effects, [])
        self.assertEqual(sys.getrefcount(value), before)
        self.assertIs(module.drive(value), value)
        self.assertEqual(effects, [value])

    def test_interpreter_call_decorator_window_uses_evaluated_prefix_and_exact_generic_edge(self):
        import typing

        type_sealed = native_api("PyType_IsSoacSealed", ctypes.c_int, ctypes.py_object)
        self.assertEqual(self.type_contract(typing.Generic), 0)
        for generic in (False, True):
            with self.subTest(generic=generic):
                effects = []

                def outer(cls):
                    effects.append(("outer", cls))
                    return cls

                def inner(cls):
                    effects.append(("inner", cls))
                    return cls

                suffix = "[T]" if generic else ""
                prepared = self.prepare(
                    "@outer\n@inner\nclass Subject" + suffix + ":\n    pass\n",
                    namespace={"outer": outer, "inner": inner},
                    # This is a CALL-operand observer, not class/base admission.
                    # Use the same explicit decision in both unchanged-source arms.
                    class_decision=self.DYNAMIC_CLASS,
                )
                module = self.execute(prepared)
                self.assertEqual(effects, [("inner", module.Subject), ("outer", module.Subject)])
                rows = self.events(prepared, self.PREPARE, "Subject")
                self.assertEqual(len(rows), 1)
                self.assertEqual(rows[0]["decorator_source"], 2 if generic else 1)
                self.assertEqual([entry[1] for entry in rows[0]["slots"]], [id(outer), id(inner)])
                if generic:
                    self.assertGreaterEqual(rows[0]["incoming_node"], 0)
                    self.assertGreaterEqual(rows[0]["incoming_ordinal"], 0)
                else:
                    self.assertEqual(rows[0]["incoming_node"], -1)
                self.assertEqual(self.type_contract(module.Subject), 0)
                self.assertEqual(type_sealed(module.Subject), 0)
                self.assertEqual(self.events(prepared, self.BIND, "Subject"), [])
                self.assertEqual(
                    module.Subject.__bases__, (typing.Generic,) if generic else (object,)
                )
                self.assertEqual(self.type_contract(typing.Generic), 0)
                if generic:
                    self.assertEqual(len(module.Subject.__orig_bases__), 1)
                    self.assertIs(
                        typing.get_origin(module.Subject.__orig_bases__[0]), typing.Generic
                    )
                    self.assertEqual(
                        typing.get_args(module.Subject.__orig_bases__[0]),
                        module.Subject.__type_params__,
                    )

    def test_interpreter_class_fixture_rejects_unknown_construction_decisions(self):
        prepared = self.prepare("class Subject:\n    pass\n")
        for decision in (-1, 2, 99):
            with self.subTest(decision=decision):
                with self.assertRaises(ValueError):
                    self.capi.soac_interpreter_fixture(
                        prepared[1], prepared[2], None, 0, decision
                    )
                self.assertEqual(self.events(prepared), [])
        # Bad cold decisions did not consume the genuine root or change the
        # existing default Enforced decision.
        module = self.execute(prepared)
        self.assertEqual(self.type_contract(module.Subject), 1)
        self.assertEqual(len(self.events(prepared, self.BIND, "Subject")), 1)

    def test_interpreter_generic_definition_requires_its_actual_scope_call_edge(self):
        import builtins

        source = (
            "from __future__ import strict\n"
            "class Subject[T]:\n"
            "    pass\n"
        ).encode()
        details = self.details_api(
            source, len(source), "<native-generic-definition-edge>", -1
        )
        self.assertEqual(details[2][0], 7)
        catalog = details[2]
        nodes = catalog[1]
        classes = [node for node in nodes if node[3] == 1]
        self.assertEqual(len(classes), 1)
        body = classes[0]
        helper = nodes[body[1]]
        self.assertEqual(helper[3:5], (6, 5))  # annotations / type parameters
        self.assertEqual(helper[1], 0)
        self.assertTrue(any(value is body[2] for value in helper[2].co_consts))
        self.assertEqual(helper[5], body[5])

        tables = catalog[3]
        calls = tables[0][5]
        selected = [
            (index, row) for index, row in enumerate(calls)
            if row[0][0] == 3 and row[0][2] == helper[0]
        ]
        self.assertEqual(len(selected), 1)
        index, edge = selected[0]
        self.assertEqual(edge[0][1], body[5])
        self.assertTrue(edge[1])

        for fault in ("missing", "wrong_child", "no_emission", "duplicate"):
            with self.subTest(fault=fault):
                altered_calls = list(calls)
                if fault == "missing":
                    del altered_calls[index]
                elif fault == "wrong_child":
                    origin = list(edge[0])
                    origin[2] = body[0]
                    altered_calls[index] = (tuple(origin), edge[1])
                elif fault == "no_emission":
                    altered_calls[index] = (edge[0], ())
                else:
                    altered_calls.append(edge)
                table = list(tables[0])
                table[5] = tuple(altered_calls)
                changed_tables = list(tables)
                changed_tables[0] = tuple(table)
                changed_catalog = list(catalog)
                changed_catalog[3] = tuple(changed_tables)
                changed_details = list(details)
                changed_details[2] = tuple(changed_catalog)
                changed_details = tuple(changed_details)
                self.assertIs(changed_details[0], details[0])
                module = types.ModuleType("_native_generic_definition_fixture")
                module.__dict__["__builtins__"] = builtins.__dict__
                with self.assertRaises(self.unavailable):
                    self.capi.soac_interpreter_fixture(
                        module, changed_details, None, 0
                    )
                self.assertNotIn("Subject", module.__dict__)

    def test_interpreter_descriptor_completion_sees_one_actual_native_birth(self):
        prepared = self.prepare(
            "class Subject:\n"
            "    @staticmethod\n    def static(value):\n        return value\n"
            "    @classmethod\n    def class_(cls, value):\n        return value\n"
            "    @property\n    def value(self):\n        return 7\n"
        )
        module = self.execute(prepared)
        objects = [module.Subject.__dict__[name] for name in ("static", "class_", "value")]
        getter = native_api("PySoac_GetDescriptorBirthId", ctypes.c_uint64, ctypes.py_object)
        self.assertTrue(all(getter(value) > 0 for value in objects))
        rows = self.events(prepared, 15, "Subject")
        self.assertEqual([row["value_id"] for row in rows], [id(value) for value in objects])
        self.assertTrue(all(row["aux"] == 2 for row in rows))
        self.assertEqual(module.Subject.static(4), 4)
        self.assertEqual(module.Subject.class_(5), 5)
        self.assertEqual(module.Subject().value, 7)


    def test_interpreter_call_closed_decision_kind_and_descriptor_identity_refuse_forgery(self):
        errors = {
            1: self.unavailable,  # Unknown selected kind.
            2: self.unavailable,  # Ordinary cannot carry selected metadata.
            3: self.unavailable,  # Dataclass cannot use a descriptor owner/stage.
            4: self.mutation,     # Actual component owner differs.
            5: TypeError,        # Actual component code differs.
            6: self.unavailable,  # Descriptor cannot carry a dataclass stage.
        }
        for fault, expected_error in errors.items():
            with self.subTest(fault=fault):
                effects = []
                prepared = self.prepare(
                    "class Subject:\n"
                    "    @staticmethod\n"
                    "    def method(value):\n"
                    "        effects.append(value)\n"
                    "        return value\n",
                    namespace={"effects": effects}, call_fault=fault,
                )
                with self.assertRaises(expected_error):
                    self.execute(prepared)
                self.assertNotIn("Subject", prepared[1].__dict__)
                self.assertEqual(effects, [])
                self.assertEqual(self.events(prepared, 15), [])
                self.assertEqual([row["aux"] for row in self.events(prepared, self.ROOT_END)], [0])

    def test_interpreter_descriptor_completion_failure_precedes_publication_and_keeps_error(self):
        failure = RuntimeError("actual descriptor completion")

        def probe(event, node, serial):
            if event == 15:
                raise failure

        prepared = self.prepare(
            "class Subject:\n"
            "    @staticmethod\n"
            "    def method(value):\n"
            "        return value\n",
            probe=probe,
        )
        try:
            self.execute(prepared)
        except RuntimeError as caught:
            self.assertIs(caught, failure)
        else:
            self.fail("native descriptor completion did not propagate its failure")
        self.assertNotIn("Subject", prepared[1].__dict__)
        self.assertEqual(len(self.events(prepared, 15)), 1)
        self.assertEqual([row["aux"] for row in self.events(prepared, self.ROOT_END)], [0])
        failure.__traceback__ = None



if __name__ == "__main__":
    unittest.main()
