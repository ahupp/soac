"""Opcode-only dataclass bridges, separate from production catalog admission."""

import _testinternalcapi
import ctypes
import dis
import subprocess
import sys
import types
import unittest
import weakref

import _types

from tests.test_strict_dataclass_native import api, instruction


class DataclassBridgeNativeTests(unittest.TestCase):
    def setUp(self):
        self.source = _types._dataclass_record_source
        self.execute = _types._dataclass_exec
        self.member = _types._dataclass_set_member
        self.canonical = api(
            "PySoac_GetDataclassBuiltin", ctypes.c_void_p, ctypes.c_uint
        )

    @staticmethod
    def fixture(root, bridges, compilation=None, bindings=()):
        reached = [None]
        payload = ("bridges-v1", bridges, compilation, reached)
        invocation = _testinternalcapi.soac_dataclass_fixture(
            root,
            bindings,
            (),
            (),
            payload,
        )
        return invocation, reached

    @staticmethod
    def call(invocation, root, *args):
        return _testinternalcapi.soac_dataclass_fixture_call(
            invocation, 1, root, args, {}
        )

    @staticmethod
    def decline(invocation):
        api("PySoac_DeclineDataclassInvocation", ctypes.c_int, ctypes.py_object)(
            invocation
        )

    @staticmethod
    def has(function):
        return api(
            "PyFunction_HasSoacDataclassCreation", ctypes.c_int, ctypes.py_object
        )(function)

    @staticmethod
    def compilation(root, source, globals, namespace):
        expected = compile(source, "<string>", "exec")

        def only_child(code):
            children = [
                (index, value)
                for index, value in enumerate(code.co_consts)
                if isinstance(value, types.CodeType)
            ]
            if len(children) != 1:
                raise AssertionError(children)
            return children[0]

        factory_index, factory = only_child(expected)
        member_index, _ = only_child(factory)
        factory_path = (factory_index,)
        member_path = (*factory_path, member_index)
        weak_slot = [None]
        return (
            source,
            globals,
            namespace,
            weak_slot,
            ((instruction(root, "CALL_FUNCTION_EX"), factory_path, 257, ()),),
            (
                ((), instruction(expected, "MAKE_FUNCTION"), factory_path, 257),
                (factory_path, instruction(factory, "MAKE_FUNCTION"), member_path, 1),
            ),
        )

    def test_build_recipes_are_fresh_ordinary_code_and_never_execute_modules(self):
        recipe = api("PySoac_GetDataclassRecipe", ctypes.py_object, ctypes.c_uint)
        before = dict(sys.modules)
        for kind, name in ((1, "dataclasses"), (2, "reprlib")):
            with self.subTest(kind=kind):
                first, second = recipe(kind), recipe(kind)
                self.assertIsInstance(first, types.CodeType)
                self.assertIsNot(first, second)
                self.assertEqual(first.co_filename, f"<frozen {name}>")
                self.assertEqual(first, second)
                view = _testinternalcapi.soac_code_view(first)
                self.assertEqual(view["strict_source_id"], 0)
                self.assertEqual(view["flags"], first.co_flags)
                reference = weakref.ref(first)
                del first
                self.assertIsNone(reference())
        self.assertEqual(dict(sys.modules), before)
        with self.assertRaises(ValueError):
            recipe(0)

    def test_build_recipe_does_not_trust_python_module_bindings(self):
        import dataclasses

        recipe = api("PySoac_GetDataclassRecipe", ctypes.py_object, ctypes.c_uint)
        before = recipe(1)
        old_filename, old_function = dataclasses.__file__, dataclasses.dataclass
        try:
            dataclasses.__file__ = "/untrusted/replaced/dataclasses.py"
            dataclasses.dataclass = lambda cls: cls
            after = recipe(1)
        finally:
            dataclasses.__file__ = old_filename
            dataclasses.dataclass = old_function
        self.assertEqual(before, after)
        self.assertIsNot(before, after)

    def test_borrowed_code_view_matches_layout_and_rejects_bad_operands(self):
        def outer(value, /, *args, keyword=3, **kwargs):
            def inner(extra):
                return value + keyword + extra

            return inner

        for code in (outer.__code__, outer(1).__code__):
            view = _testinternalcapi.soac_code_view(code)
            self.assertEqual(view["abi_version"], 1)
            for field in (
                "flags",
                "argcount",
                "posonlyargcount",
                "kwonlyargcount",
                "stacksize",
                "firstlineno",
                "nlocals",
            ):
                self.assertEqual(view[field], getattr(code, f"co_{field}"))
            for field in (
                "consts",
                "names",
                "filename",
                "name",
                "qualname",
                "linetable",
                "exceptiontable",
            ):
                self.assertIs(view[field], getattr(code, f"co_{field}"))
            self.assertEqual(view["code_units"], len(code.co_code) // 2)
            self.assertEqual(view["ncellvars"], len(code.co_cellvars))
            self.assertEqual(view["nfreevars"], len(code.co_freevars))
            self.assertEqual(view["nlocalsplus"], len(view["localsplusnames"]))
            self.assertEqual(view["nlocalsplus"], len(view["localspluskinds"]))
            self.assertIsInstance(view["localsplusnames"], tuple)
            self.assertIsInstance(view["localspluskinds"], bytes)
            with self.assertRaises(ValueError):
                _testinternalcapi.soac_code_view(code, False)
        with self.assertRaises(TypeError):
            _testinternalcapi.soac_code_view(object())

    def test_builtin_graph_matcher_accepts_only_native_semantic_implementations(self):
        matches = api(
            "PySoac_MatchesBuiltinFunction",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.c_char_p,
            ctypes.c_ssize_t,
        )
        copy = _testinternalcapi.soac_dataclass_fixture_copy_builtin
        for name, function in (
            (b"getattr", getattr),
            (b"hasattr", hasattr),
            (b"isinstance", isinstance),
            (b"issubclass", issubclass),
            (b"len", len),
            (b"dir", dir),
            (b"globals", globals),
            (b"max", max),
        ):
            with self.subTest(name=name):
                self.assertEqual(matches(function, name, len(name)), 1)
                cloned = copy(function)
                self.assertIsNot(cloned, function)
                self.assertEqual(matches(cloned, name, len(name)), 1)
                self.assertEqual(matches(copy(function, None), name, len(name)), 0)
                self.assertEqual(
                    matches(copy(function, function.__self__, True), name, len(name)),
                    0,
                )
                self.assertEqual(matches(function, b"abs", 3), 0)
                self.assertEqual(matches(function, name, len(name) - 1), 0)

    def test_builtin_graph_matcher_never_uses_python_attributes(self):
        matches = api(
            "PySoac_MatchesBuiltinFunction",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.c_char_p,
            ctypes.c_ssize_t,
        )

        class Spoof:
            def __getattribute__(self, name):
                raise AssertionError("builtin matcher invoked Python")

        for function in (ctypes.py_object(Spoof()), lambda: None, int, None):
            self.assertEqual(matches(function, b"getattr", 7), 0)
        self.assertEqual(matches(getattr, b"getattr\0suffix", 14), 0)
        with self.assertRaises(SystemError):
            matches(getattr, None, 0)
        with self.assertRaises(SystemError):
            matches(getattr, b"getattr", -1)

    def test_reached_source_requires_the_exact_callsite_and_operands(self):
        recorder = self.source
        fragment = " def result(): return 17"

        def root():
            return recorder(fragment)

        invocation, reached = self.fixture(
            root,
            ((instruction(root, "CALL"), 1, (fragment,)),),
        )
        self.assertIs(self.call(invocation, root), fragment)
        self.assertIs(reached[0], True)
        self.decline(invocation)

        invocation, reached = self.fixture(
            root,
            ((instruction(root, "CALL") + 1, 1, (fragment,)),),
        )
        with self.assertRaises(ImportError):
            self.call(invocation, root)
        self.assertIsNone(reached[0])

    def test_warmed_builtin_site_deopts_for_the_contextual_source_bridge(self):
        fragment = " def result(): return 17"

        def root(function, value):
            return function(value)

        for _ in range(80):
            root(format, fragment)
        self.assertTrue(
            any(
                operation.opname == "CALL_BUILTIN_FAST"
                for operation in dis.get_instructions(root, adaptive=True)
            )
        )
        invocation, reached = self.fixture(
            root,
            ((instruction(root, "CALL"), 1, (fragment,)),),
            bindings=((0, self.source), (1, fragment)),
        )
        self.assertIs(self.call(invocation, root, self.source, fragment), fragment)
        self.assertIs(reached[0], True)
        self.decline(invocation)

    def test_monitoring_and_unpack_calls_keep_explicit_source_operands(self):
        recorder = self.source
        fragment = "fragment"

        def direct():
            return recorder(fragment)

        def unpacked():
            return recorder(*(fragment,))

        tool = 4
        sys.monitoring.use_tool_id(tool, "native dataclass bridge regression")
        sys.monitoring.register_callback(
            tool, sys.monitoring.events.CALL, lambda *args: None
        )
        try:
            for root, opcode in ((direct, "CALL"), (unpacked, "CALL_FUNCTION_EX")):
                for monitored in (False, True):
                    with self.subTest(opcode=opcode, monitored=monitored):
                        for _ in range(80):
                            self.assertIs(root(), fragment)
                        if monitored:
                            sys.monitoring.set_local_events(
                                tool, root.__code__, sys.monitoring.events.CALL
                            )
                        invocation, reached = self.fixture(
                            root,
                            ((instruction(root, opcode), 1, (fragment,)),),
                        )
                        try:
                            self.assertIs(self.call(invocation, root), fragment)
                            self.assertIs(reached[0], True)
                        finally:
                            self.decline(invocation)
                            sys.monitoring.set_local_events(tool, root.__code__, 0)
        finally:
            sys.monitoring.register_callback(tool, sys.monitoring.events.CALL, None)
            sys.monitoring.free_tool_id(tool)

    def test_exact_exec_bridge_records_only_the_actual_factory_code_tree(self):
        execute = self.execute
        source = "def create():\n def result(): return 17\n return result\n"
        globals, namespace = {}, {}

        def root():
            execute(exec, source, globals, namespace)
            return namespace["create"](**{})  # noqa: PIE804 - retain the actual factory CALL_FUNCTION_EX

        plan = self.compilation(root, source, globals, namespace)
        invocation, reached = self.fixture(
            root,
            ((instruction(root, "CALL"), 2, (exec, source, globals, namespace)),),
            plan,
        )
        result = self.call(invocation, root)
        self.assertIs(reached[0], True)
        self.assertEqual(self.has(namespace["create"]), 1)
        self.assertEqual(self.has(result), 1)
        self.assertEqual(result(), 17)
        # The root code is already dead: the retained catalog is weak-only.
        self.assertIsNone(plan[3][0][0]())
        clone = types.FunctionType(result.__code__, globals)
        self.assertEqual(self.has(clone), 0)
        self.assertEqual(clone(), 17)
        self.decline(invocation)

    def test_exec_audit_boundaries_revalidate_parent_code_and_actual_builtins(self):
        execute = self.execute
        source = "def create():\n def result(): return 17\n return result\n"
        active = []

        def audit(event, arguments):
            if active and event == active[0]:
                callback = active[1]
                active.clear()
                callback()

        sys.addaudithook(audit)
        for event in ("compile", "exec"):
            with self.subTest(event=event):
                globals, namespace = {}, {}

                def root(globals=globals, namespace=namespace):
                    execute(exec, source, globals, namespace)
                    return namespace["create"](**{})  # noqa: PIE804 - factory CALL_FUNCTION_EX

                plan = self.compilation(root, source, globals, namespace)
                invocation, _ = self.fixture(
                    root,
                    (
                        (
                            instruction(root, "CALL"),
                            2,
                            (exec, source, globals, namespace),
                        ),
                    ),
                    plan,
                )
                if event == "compile":
                    original = root.__code__

                    def change(root=root, original=original):
                        root.__code__ = original.replace(
                            co_name="mutated_during_compile"
                        )
                else:

                    def change(globals=globals):
                        globals["__builtins__"] = {}

                active[:] = [event, change]
                try:
                    with self.assertRaises(ImportError):
                        self.call(invocation, root)
                finally:
                    active.clear()
                self.assertNotIn("create", namespace)
                if event == "compile":
                    self.assertIsNone(plan[3][0])
                else:
                    self.assertIsNone(plan[3][0][0]())

    def test_exec_bridge_never_grants_strict_original_code_authority(self):
        execute = self.execute
        source = (
            "from __future__ import strict\n"
            "def create():\n def result(): return 17\n return result\n"
        )
        globals, namespace = {}, {}

        def root():
            execute(exec, source, globals, namespace)
            return namespace["create"](**{})  # noqa: PIE804 - factory CALL_FUNCTION_EX

        plan = self.compilation(root, source, globals, namespace)
        invocation, _ = self.fixture(
            root,
            ((instruction(root, "CALL"), 2, (exec, source, globals, namespace)),),
            plan,
        )
        with self.assertRaises(ImportError):
            self.call(invocation, root)
        self.assertNotIn("create", namespace)
        self.assertIsNone(plan[3][0])

    def test_code_identical_factory_clone_cannot_enter_as_fresh_factory(self):
        execute = self.execute
        source = "def create():\n def result(): return 17\n return result\n"
        globals, namespace = {}, {}

        def replace_factory():
            namespace["create"] = types.FunctionType(
                namespace["create"].__code__, globals
            )

        def root():
            execute(exec, source, globals, namespace)
            replace_factory()
            return namespace["create"](**{})  # noqa: PIE804 - factory CALL_FUNCTION_EX

        plan = self.compilation(root, source, globals, namespace)
        first_call = next(
            op.offset // 2 for op in dis.get_instructions(root) if op.opname == "CALL"
        )
        invocation, _ = self.fixture(
            root,
            ((first_call, 2, (exec, source, globals, namespace)),),
            plan,
        )
        with self.assertRaises(ImportError):
            self.call(invocation, root)
        self.assertEqual(self.has(namespace["create"]), 0)

    def test_c_proxy_forwarding_cannot_inherit_the_direct_exec_edge(self):
        execute = self.execute
        forward = _testinternalcapi.soac_dataclass_fixture_c_forward
        source = "def create():\n def result(): return 17\n return result\n"
        globals, namespace = {}, {}

        def root():
            forward(execute, exec, source, globals, namespace)
            return namespace["create"](**{})  # noqa: PIE804 - factory CALL_FUNCTION_EX

        self.assertEqual(root()(), 17)
        namespace.clear()
        plan = self.compilation(root, source, globals, namespace)
        invocation, reached = self.fixture(
            root,
            ((instruction(root, "CALL"), 2, (exec, source, globals, namespace)),),
            plan,
        )
        with self.assertRaises(ImportError):
            self.call(invocation, root)
        self.assertIsNone(reached[0])
        self.assertIsNone(plan[3][0])
        self.assertEqual(self.has(namespace["create"]), 0)

    def test_copies_of_the_native_helper_or_original_exec_are_not_canonical(self):
        copy = _testinternalcapi.soac_dataclass_fixture_copy_builtin
        fragment = "fragment"
        recorder = copy(self.source)

        def root():
            return recorder(fragment)

        invocation, reached = self.fixture(
            root,
            ((instruction(root, "CALL"), 1, (fragment,)),),
        )
        with self.assertRaises(ImportError):
            self.call(invocation, root)
        self.assertIsNone(reached[0])
        self.assertIs(recorder(fragment), fragment)

        execute, original_copy = self.execute, copy(exec)
        source, globals, namespace = "value = 17", {}, {}

        def root():
            execute(original_copy, source, globals, namespace)

        invocation, reached = self.fixture(
            root,
            (
                (
                    instruction(root, "CALL"),
                    2,
                    (original_copy, source, globals, namespace),
                ),
            ),
        )
        with self.assertRaises(ImportError):
            self.call(invocation, root)
        self.assertNotIn("value", namespace)
        self.assertIsNone(reached[0])

    def test_native_helpers_match_exact_canonical_objects(self):
        for kind, value in enumerate((self.source, self.execute, self.member), 1):
            with self.subTest(kind=kind):
                self.assertEqual(self.canonical(kind), id(value))
        with self.assertRaises(ValueError):
            self.canonical(0)

    def test_public_calls_are_ordinary_and_preserve_actual_callable_operands(self):
        value = object()
        self.assertIs(self.source(value), value)
        namespace = {}
        self.assertIsNone(self.execute(exec, "value = 17", namespace, namespace))
        self.assertEqual(namespace["value"], 17)
        events = []

        def supplied_exec(*args):
            events.append(args)
            return value

        self.assertIs(self.execute(supplied_exec, value, None, None), value)
        self.assertEqual(events, [(value, None, None)])

        class Target:
            pass

        self.assertIsNone(self.member(setattr, Target, "value", value))
        self.assertIs(Target.value, value)

        def supplied_setattr(*args):
            events.append(args)
            return value

        self.assertIs(self.member(supplied_setattr, Target, "value", None), value)
        self.assertEqual(events[-1], (Target, "value", None))

    def test_builtin_originals_are_not_blessed_from_late_python_bindings(self):
        code = r"""
import builtins
original_exec = builtins.exec
original_setattr = builtins.setattr
fake_exec = builtins.exec = lambda *args: None
fake_setattr = builtins.setattr = lambda *args: None
import _types
# Restore ordinary import execution only after the late native module init.
builtins.exec = original_exec
builtins.setattr = original_setattr
import ctypes
getter = ctypes.pythonapi.PySoac_GetDataclassBuiltin
getter.restype = ctypes.c_void_p
getter.argtypes = [ctypes.c_uint]
assert getter(4) == id(original_exec)
assert getter(5) == id(original_setattr)
assert getter(4) != id(fake_exec)
assert getter(5) != id(fake_setattr)
"""
        subprocess.run([sys.executable, "-I", "-S", "-B", "-c", code], check=True)

    def test_canonical_witness_does_not_retain_or_regrant_dead_helper(self):
        code = r"""
import _types, ctypes, weakref, sys, gc
getter = ctypes.pythonapi.PySoac_GetDataclassBuiltin
getter.restype = ctypes.c_void_p
getter.argtypes = [ctypes.c_uint]
original = _types._dataclass_record_source
assert getter(1) == id(original)
reference = weakref.ref(original)
del _types._dataclass_record_source, original
gc.collect()
assert reference() is None
assert getter(1) is None
del sys.modules['_types'], _types
import _types
assert _types._dataclass_record_source('ordinary') == 'ordinary'
assert getter(1) is None
"""
        subprocess.run([sys.executable, "-I", "-S", "-B", "-c", code], check=True)


if __name__ == "__main__":
    unittest.main()
