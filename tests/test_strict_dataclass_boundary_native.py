"""Raw generated-function provenance, construction safety and metadata adoption.

The C fixture attests exact reached producer/code identities. It grants no
argument, return or default-factory value-type guarantee. Production must
authenticate its signed compiler/stdlib facts independently.
"""

import _testinternalcapi
import ctypes
import dis
import gc
import subprocess
import sys
import textwrap
import types
import unittest
import weakref

from tests.test_strict_cpython_native import borrowed_object_api, native_api
from tests.test_strict_dataclass_native import child_code, instruction
from tests.test_strict_type_native import TypeContractSpecV4
from tests.test_strict_type_native import ConstructionSpec


def create_required(actual, output, inputs):
    def generated(self, value):
        self.value = value

    output.append(generated)
    return actual

def create_defaults(actual, output, inputs):
    def generated(self, value=5, /, *, other=7):
        self.value = (value, other)

    output.append(generated)
    return actual

def create_factory(actual, output, inputs):
    marker, factory, bridge = inputs

    def generated(self, value=marker):
        self.value = bridge(factory() if value is marker else value)

    output.append(generated)
    return actual

def create_raising(actual, output, inputs):
    def generated(self, value):
        self.value = value

    output.append(generated)
    raise LookupError("application failed after fresh function escaped")

def create_body_error(actual, output, inputs):
    def generated(self, value):
        raise ValueError("generated body failed")

    output.append(generated)
    return actual

def create_varargs(actual, output, inputs):
    def generated(self, *values):
        self.value = values

    output.append(generated)
    return actual

def create_varkwargs(actual, output, inputs):
    def generated(self, **values):
        self.value = values

    output.append(generated)
    return actual

def create_annotation_component(actual, output, install):
    def provider(format):
        return {"value": int}

    def method(self):
        return 17

    method.__annotate__ = provider
    output[:] = method, provider
    install(method, provider)
    return actual

def create_repr_component(actual, output, install):
    def implementation(self):
        return "generated representation"

    def method(self):
        return implementation(self)

    output[:] = method, implementation
    install(method, implementation)
    return actual


class GeneratedFunctionNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.strict_id = native_api(
            "PyFunction_GetSoacStrictId", ctypes.c_uint64, ctypes.py_object
        )
        cls.metadata = native_api(
            "PyFunction_GetSoacMetadata", ctypes.c_void_p, ctypes.py_object
        )
        cls.has_creation = native_api(
            "PyFunction_HasSoacDataclassCreation", ctypes.c_int, ctypes.py_object
        )
        cls.matches_creation = native_api(
            "PyFunction_MatchesSoacDataclassCreation", ctypes.c_int,
            ctypes.py_object, ctypes.py_object, ctypes.c_uint,
        )
        cls.bind = native_api(
            "PySoac_DataclassBindClass", ctypes.c_int, *([ctypes.py_object] * 3)
        )
        cls.complete = native_api(
            "PySoac_CompleteDataclassInvocation", ctypes.c_int, ctypes.py_object
        )
        cls.adopt_component = native_api(
            "PyFunction_AdoptSoacDataclassComponent", ctypes.c_int,
            ctypes.py_object, ctypes.py_object, ctypes.py_object, ctypes.c_uint, ctypes.c_ssize_t,
        )
        cls.new_handle = native_api(
            "PyType_NewSoacConstructionHandle", ctypes.py_object,
            ctypes.POINTER(ConstructionSpec),
        )
        cls.construct = native_api(
            "PyType_FromSoacConstructionHandle", ctypes.py_object,
            ctypes.py_object, ctypes.py_object,
        )
        cls.unavailable = borrowed_object_api("PySoac_GetStrictRuntimeUnavailableError")
        cls.mutation = borrowed_object_api("PySoac_GetStrictMutationError")

    def build(self):
        owner = object()
        namespace_function = lambda namespace, cell: None
        spec = ConstructionSpec(
                   4,
                   ctypes.sizeof(ConstructionSpec),
                   0,
                   0,
                   owner,
                   namespace_function,
                   "GeneratedTarget",
                   (),
                   {},
                   {},
                   None,
                   None,
                   TypeContractSpecV4(flags=0, fields=(), protected_names=(), final_methods=(), check_instance_write=None, new_instance_dict=None),
               )
        handle = self.new_handle(ctypes.byref(spec))
        return self.construct(handle, namespace_function), owner

    def fixture(self, root=create_required, *, inputs=(), creation_payload=None):
        actual, class_owner = self.build()
        output = []
        code = child_code(root)
        invocation = _testinternalcapi.soac_dataclass_fixture(
            root,
            ((0, actual), (1, output), (2, inputs)),
            (),
            ((root, instruction(root, "MAKE_FUNCTION"), code, 1, ()),),
            creation_payload,
        )
        self.assertEqual(self.bind(invocation, actual, class_owner), 0)
        return invocation, actual, output, inputs, root

    def create(self, fixture, *, finish=True):
        invocation, actual, output, inputs, root = fixture
        result = _testinternalcapi.soac_dataclass_fixture_call(
            invocation, 2, root, (actual, output, inputs), {}
        )
        self.assertIs(result, actual)
        self.assertEqual(len(output), 1)
        if finish:
            self.assertEqual(self.complete(invocation), 0)
        return output[0]

    def factory_fixture(self, factory):
        marker = object()
        # Keep the original generated body/cells; the callable is now an ordinary
        # identity function, not a native value-check bridge or new authority.
        fixture = self.fixture(create_factory, inputs=(marker, factory, lambda value: value))
        return self.create(fixture), marker

    def test_complete_no_closure_body_is_callable_from_create_watcher_without_type_checks(self):
        fixture = self.fixture()
        code = child_code(create_required)
        events, unexpected = [], []
        callback_type = ctypes.CFUNCTYPE(
            ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p
        )

        @callback_type
        def watcher(event, address, replacement):
            if event != 0:
                return 0
            function = ctypes.cast(address, ctypes.py_object).value
            if function.__code__ is not code:
                return 0
            target = types.SimpleNamespace()
            try:
                function(target, "bad")
                events.append((self.has_creation(function), target.value))
            except BaseException as error:
                unexpected.append(error)
            return 0

        add = native_api("PyFunction_AddWatcher", ctypes.c_int, callback_type)
        clear = native_api("PyFunction_ClearWatcher", ctypes.c_int, ctypes.c_int)
        identifier = add(watcher)
        try:
            function = self.create(fixture)
        finally:
            clear(identifier)
        self.assertEqual(events, [(1, "bad")])
        self.assertEqual(unexpected, [])
        self.assertEqual(self.has_creation(function), 1)
        self.assertEqual(self.strict_id(function), 0)
        self.assertIsNone(self.metadata(function))
        self.assertFalse(function.__code__.co_flags & 0x10000000)

    def test_native_binding_keeps_positional_keyword_and_defaults_without_type_predicates(self):
        function = self.create(self.fixture(create_defaults))
        target = types.SimpleNamespace()
        function(target, 11)
        self.assertEqual(target.value, (11, 7))
        function(target, other=13)
        self.assertEqual(target.value, (5, 13))
        function(target)
        self.assertEqual(target.value, (5, 7))
        function(target, other="bad")
        self.assertEqual(target.value, (5, "bad"))

    def test_binding_errors_keep_the_ordinary_error_without_entering_the_body(self):
        function = self.create(self.fixture())
        ordinary = types.FunctionType(function.__code__, function.__globals__)
        target = types.SimpleNamespace()
        cases = [
            ((), {}),
            ((target,), {}),
            ((target, 1, 2), {}),
            ((target, 1), {"value": 2}),
            ((target,), {"missing": 3}),
        ]
        for args, kwargs in cases:
            with self.subTest(args=args, kwargs=kwargs):
                with self.assertRaises(TypeError) as expected:
                    ordinary(*args, **kwargs)
                with self.assertRaises(TypeError) as observed:
                    function(*args, **kwargs)
                self.assertEqual(str(observed.exception), str(expected.exception))
                self.assertFalse(hasattr(target, "value"))

    def test_factory_conditional_has_ordinary_value_and_explicit_marker_semantics(self):
        calls = []

        def factory():
            calls.append("factory")
            return "bad"

        function, marker = self.factory_fixture(factory)
        target = types.SimpleNamespace()
        function(target)
        self.assertEqual(target.value, "bad")
        self.assertEqual(calls, ["factory"])
        function(target, "supplied")
        self.assertEqual(target.value, "supplied")
        self.assertEqual(calls, ["factory"])
        function(target, marker)
        self.assertEqual(target.value, "bad")
        self.assertEqual(calls, ["factory", "factory"])

    def test_public_function_copy_remains_ordinary_without_record_or_checks(self):
        function, _ = self.factory_fixture(lambda: "ordinary result")
        ordinary = types.FunctionType(
            function.__code__,
            function.__globals__,
            argdefs=function.__defaults__,
            closure=function.__closure__,
        )
        self.assertEqual(self.has_creation(ordinary), 0)
        target = types.SimpleNamespace()
        ordinary(target)
        self.assertEqual(target.value, "ordinary result")
        ordinary(target, "ordinary supplied")
        self.assertEqual(target.value, "ordinary supplied")

    def test_failed_application_expires_adoption_but_not_ordinary_escaped_body_execution(self):
        fixture = self.fixture(create_raising)
        with self.assertRaisesRegex(LookupError, "application failed"):
            self.create(fixture)
        (function,) = fixture[2]
        target = types.SimpleNamespace()
        function(target, 29)
        self.assertEqual(target.value, 29)
        function(target, "bad")
        self.assertEqual(target.value, "bad")
        self.assertEqual(self.has_creation(function), 1)
        self.assertEqual(self.strict_id(function), 0)
        with self.assertRaises(self.unavailable):
            self.matches_creation(function, fixture[0], 1)
        function.__code__ = function.__code__

    def test_incomplete_closure_call_from_create_watcher_fails_before_body(self):
        code = child_code(create_factory)
        observed = []
        unexpected = []
        callback_type = ctypes.CFUNCTYPE(
            ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p
        )

        @callback_type
        def watcher(event, address, replacement):
            if event != 0:
                return 0
            function = ctypes.cast(address, ctypes.py_object).value
            if function.__code__ is not code:
                return 0
            target = types.SimpleNamespace()
            try:
                function(target, 17)
            except self.unavailable:
                observed.append((function.__closure__, hasattr(target, "value")))
            except BaseException as error:  # noqa: BLE001 - report C callback errors.
                unexpected.append(error)
            else:
                observed.append("incomplete function ran")
            return 0

        add = native_api("PyFunction_AddWatcher", ctypes.c_int, callback_type)
        clear = native_api("PyFunction_ClearWatcher", ctypes.c_int, ctypes.c_int)
        identifier = add(watcher)
        try:
            function, _ = self.factory_fixture(lambda: 19)
        finally:
            clear(identifier)
        self.assertEqual(observed, [(None, False)])
        self.assertEqual(unexpected, [])
        target = types.SimpleNamespace()
        function(target)
        self.assertEqual(target.value, 19)

    def test_unpublished_creation_failure_clears_weakrefs_and_respects_escaped_owners(
        self,
    ):
        for retained in (False, True):
            with self.subTest(retained=retained):
                program = textwrap.dedent("""
                    import types
                    from tests.test_strict_dataclass_boundary_native import GeneratedFunctionNativeTests

                    GeneratedFunctionNativeTests.setUpClass()
                    case = GeneratedFunctionNativeTests()
                    slots = [None, None]
                    released = []
                    observed = []

                    class Payload:
                        def __del__(self):
                            released.append('released')

                    class Observer:
                        def __del__(self):
                            # The active callback owner's last release occurs
                            # before the constructor drops its function ref.
                            function = slots[0]()
                            if function is None:
                                observed.append('weak referent disappeared too early')
                                return
                            target = types.SimpleNamespace()
                            try:
                                function(target, 17)
                            except case.unavailable:
                                observed.append('terminal before owner finalizer')
                            except BaseException as error:
                                observed.append(type(error).__name__)
                            else:
                                observed.append('unpublished body ran')

                    fixture = case.fixture(creation_payload=(
                        'created-weak-v1', slots, RETAINED, (Payload(),), Observer(),
                    ))
                    try:
                        case.create(fixture)
                    except RuntimeError as error:
                        assert str(error) == 'fixture failed after unpublished weak witness'
                    else:
                        raise AssertionError('unpublished construction unexpectedly completed')
                    assert fixture[2] == []
                    assert observed == ['terminal before owner finalizer'], observed
                    if RETAINED:
                        function = slots[1]
                        assert slots[0]() is function
                        assert released == []
                        target = types.SimpleNamespace()
                        for call in (lambda: function(target, 17),
                                     lambda: case.has_creation(function)):
                            try:
                                call()
                            except case.unavailable:
                                pass
                            else:
                                raise AssertionError('failed unpublished function is not terminal')
                        del call
                        assert not hasattr(target, 'value')
                        slots[1] = None
                        del function
                    assert slots[0]() is None
                    assert released == ['released']
                    print('unpublished cleanup PASS')
                """).replace("RETAINED", repr(retained))
                result = subprocess.run(
                    [sys.executable, "-S", "-B", "-c", program],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("unpublished cleanup PASS", result.stdout)

    def test_binding_and_body_errors_release_arguments_without_hidden_roots(self):
        class Value:
            pass

        function = self.create(self.fixture(create_body_error))
        target = types.SimpleNamespace()
        for bad_binding in (False, True):
            with self.subTest(bad_binding=bad_binding):
                value = Value()
                reference = weakref.ref(value)
                if bad_binding:
                    with self.assertRaises(TypeError):
                        function(target, value, unknown=17)
                else:
                    with self.assertRaisesRegex(ValueError, "generated body failed"):
                        function(target, value)
                del value
                self.assertIsNone(reference())

    def test_generated_varargs_and_varkwargs_use_ordinary_binding(self):
        positional = self.create(self.fixture(create_varargs))
        keyword = self.create(self.fixture(create_varkwargs))
        target = types.SimpleNamespace()
        value = object()
        positional(target, "text", value)
        self.assertEqual(target.value, ("text", value))
        self.assertIs(target.value[1], value)
        keyword(target, text="text", value=value)
        self.assertEqual(target.value, {"text": "text", "value": value})
        self.assertIs(target.value["value"], value)

    def test_creation_record_keeps_no_class_or_active_catalog_backedge(self):
        events = []

        class Payload:
            def __del__(self):
                events.append("released")

        fixture = self.fixture(creation_payload=Payload())
        actual_ref = weakref.ref(fixture[1])
        function = self.create(fixture, finish=False)
        function_ref = weakref.ref(function)
        self.assertEqual(events, [])
        self.assertEqual(self.complete(fixture[0]), 0)
        self.assertEqual(events, ["released"])
        del fixture
        gc.collect()
        self.assertIsNone(actual_ref())
        self.assertIsNotNone(function_ref())
        del function
        self.assertIsNone(function_ref())
        self.assertEqual(events, ["released"])

    def component_fixture(self, root, install, *, component_role=None, policy=True):
        actual, owner = self.build()
        output = []
        kind, index, default_role = (
            (1, -1, 258) if root is create_annotation_component else (2, 0, 259)
        )
        if component_role is None:
            component_role = default_role
        codes = {
            value.co_name: value
            for value in root.__code__.co_consts
            if isinstance(value, types.CodeType)
        }
        method_code = codes.pop("method")
        (component_code,) = codes.values()
        sites = []
        previous_code = None
        for op in dis.get_instructions(root):
            if op.opname == "LOAD_CONST" and isinstance(op.argval, types.CodeType):
                previous_code = op.argval
            if op.opname == "MAKE_FUNCTION":
                role = 1 if previous_code is method_code else component_role
                sites.append((root, op.offset // 2, previous_code, role, ()))
        self.assertEqual(len(sites), 2)
        components = (
            ((method_code, component_code, kind, index, root.__globals__),)
            if policy
            else ()
        )
        invocation = _testinternalcapi.soac_dataclass_fixture(
            root,
            ((0, actual), (1, output), (2, install)),
            (),
            tuple(sites),
            None,
            (),
            components,
        )
        self.assertEqual(self.bind(invocation, actual, owner), 0)
        return invocation, actual, output, kind, index

    def test_fresh_owned_components_get_only_metadata_sealing(self):
        for root in (create_annotation_component, create_repr_component):
            with self.subTest(root=root):
                binding = []

                def install(method, component, binding=binding):
                    invocation, kind, index = binding
                    self.assertEqual(
                        self.adopt_component(
                            invocation, method, component, kind, index
                        ),
                        0,
                    )

                invocation, actual, output, kind, index = self.component_fixture(
                    root, install
                )
                binding.extend((invocation, kind, index))
                self.assertIs(
                    _testinternalcapi.soac_dataclass_fixture_call(
                        invocation, 2, root, (actual, output, install), {}
                    ),
                    actual,
                )
                self.assertEqual(self.complete(invocation), 0)
                method, component = output
                self.assertGreater(self.strict_id(component), 0)
                self.assertEqual(self.strict_id(method), 0)
                self.assertIsNone(self.metadata(component))
                self.assertFalse(component.__code__.co_flags & 0x10000000)
                with self.assertRaises(self.mutation):
                    component.__code__ = component.__code__
                self.assertEqual(
                    method(actual()), 17 if kind == 1 else "generated representation"
                )
                if kind == 1:
                    self.assertEqual(method.__annotations__, {"value": int})
                copied = types.FunctionType(component.__code__, component.__globals__)
                copied.__defaults__ = (None,)
                self.assertEqual(self.has_creation(copied), 0)
                self.assertEqual(self.strict_id(copied), 0)

    def test_component_adoption_requires_birth_role_relationship_and_policy(self):
        for root in (create_annotation_component, create_repr_component):
            for failure in ("copy", "role", "detached", "changed-code", "policy"):
                with self.subTest(root=root, failure=failure):
                    binding = []

                    def install(method, component, binding=binding, failure=failure):
                        invocation, kind, index = binding
                        if failure == "copy":
                            component = types.FunctionType(
                                component.__code__, component.__globals__
                            )
                        if failure in ("copy", "detached"):
                            replacement = (
                                component if failure == "copy" else lambda value: None
                            )
                            if kind == 1:
                                method.__annotate__ = replacement
                            else:
                                method.__closure__[index].cell_contents = replacement
                        if failure == "changed-code":
                            component.__code__ = component.__code__.replace()
                        self.adopt_component(invocation, method, component, kind, index)

                    invocation, actual, output, kind, index = self.component_fixture(
                        root,
                        install,
                        component_role=1 if failure == "role" else None,
                        policy=failure != "policy",
                    )
                    binding.extend((invocation, kind, index))
                    with self.assertRaises(self.unavailable):
                        _testinternalcapi.soac_dataclass_fixture_call(
                            invocation, 2, root, (actual, output, install), {}
                        )
                    method, component = output
                    self.assertEqual(self.strict_id(method), 0)
                    self.assertEqual(self.strict_id(component), 0)

    def test_component_records_do_not_keep_the_method_or_class_alive(self):
        binding = []

        def install(method, component):
            self.adopt_component(binding[0], method, component, 1, -1)

        invocation, actual, output, _, _ = self.component_fixture(
            create_annotation_component, install
        )
        binding.append(invocation)
        _testinternalcapi.soac_dataclass_fixture_call(
            invocation, 2, create_annotation_component, (actual, output, install), {}
        )
        self.complete(invocation)
        method, component = output
        method_ref, component_ref, class_ref = (
            weakref.ref(method),
            weakref.ref(component),
            weakref.ref(actual),
        )
        output.clear()
        binding.clear()
        del method, actual, invocation
        self.assertIsNone(method_ref())
        gc.collect()
        self.assertIsNone(class_ref())
        self.assertIsNotNone(component_ref())
        del component
        self.assertIsNone(component_ref())


if __name__ == "__main__":
    unittest.main()
