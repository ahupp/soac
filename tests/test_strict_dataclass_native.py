"""Raw native dataclass provenance kernels, not production admission proof.

The _testinternalcapi fixture supplies an exact native callback catalog. The
signed compiler/stdlib adapter must separately prove its production catalog.
"""

import _testinternalcapi
import ctypes
import dis
import gc
import sys
import types
import unittest
import weakref


def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.restype = result
    function.argtypes = arguments
    return function


def instruction(function, name):
    matches = [
        op.offset // 2 for op in dis.get_instructions(function) if op.opname == name
    ]
    if len(matches) != 1:
        raise AssertionError((name, matches))
    return matches[0]


def child_code(function):
    codes = [
        value
        for value in function.__code__.co_consts
        if isinstance(value, types.CodeType)
    ]
    if len(codes) != 1:
        raise AssertionError(codes)
    return codes[0]


class DataclassCreationNativeTests(unittest.TestCase):
    def fixture(self, payload=None):
        value = object()

        def create(value):
            def result():
                return value

            return result

        def root(value):
            return create(value)

        invocation = _testinternalcapi.soac_dataclass_fixture(
            root,
            ((0, value),),
            ((root, instruction(root, "CALL"), create, 1001, ((0, value),)),),
            (
                (
                    create,
                    instruction(create, "MAKE_FUNCTION"),
                    child_code(create),
                    1,
                    ((0, value),),
                ),
            ),
            payload,
        )
        return invocation, root, create, value

    @staticmethod
    def call(invocation, root, value):
        return _testinternalcapi.soac_dataclass_fixture_call(
            invocation, 1, root, (value,), {}
        )

    @staticmethod
    def has(function):
        return api(
            "PyFunction_HasSoacDataclassCreation", ctypes.c_int, ctypes.py_object
        )(function)

    @staticmethod
    def matches(function, invocation, role=1):
        return api(
            "PyFunction_MatchesSoacDataclassCreation",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
            ctypes.c_uint,
        )(function, invocation, role)

    @staticmethod
    def decline(invocation):
        return api("PySoac_DeclineDataclassInvocation", ctypes.c_int, ctypes.py_object)(
            invocation
        )

    def test_actual_bound_frame_and_create_site_produce_native_record(self):
        invocation, root, _, value = self.fixture()
        result = self.call(invocation, root, value)
        self.assertEqual(self.has(result), 1)
        self.assertEqual(self.matches(result, invocation), 1)
        self.assertIs(result(), value)
        self.assertEqual(
            api("PyFunction_GetSoacStrictId", ctypes.c_uint64, ctypes.py_object)(
                result
            ),
            0,
        )
        with self.assertRaises(TypeError):
            type(invocation)()
        self.decline(invocation)

    def test_warmed_python_call_preserves_exact_frame_provenance(self):
        value = object()

        def create(value):
            def result():
                return value

            return result

        def root(value):
            count = 80
            results = ()
            while count:
                results += (create(value),)
                count -= 1
            return results

        invocation = _testinternalcapi.soac_dataclass_fixture(
            root,
            ((0, value),),
            ((root, instruction(root, "CALL"), create, 1001, ((0, value),)),),
            (
                (
                    create,
                    instruction(create, "MAKE_FUNCTION"),
                    child_code(create),
                    1,
                    ((0, value),),
                ),
            ),
            None,
        )
        results = self.call(invocation, root, value)
        self.assertEqual(len(results), 80)
        self.assertTrue(
            all(self.matches(result, invocation) == 1 for result in results)
        )
        self.assertTrue(all(result() is value for result in results))
        self.assertTrue(
            any(
                op.opname == "CALL_PY_EXACT_ARGS"
                for op in dis.get_instructions(root, adaptive=True)
            )
        )
        self.decline(invocation)

    def test_native_creation_record_precedes_create_watchers(self):
        invocation, root, create, value = self.fixture()
        code = child_code(create)
        events = []
        has = api("PyFunction_HasSoacDataclassCreation", ctypes.c_int, ctypes.py_object)
        callback_type = ctypes.CFUNCTYPE(
            ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p
        )

        @callback_type
        def watcher(event, function_address, replacement_address):
            if event == 0:
                function = ctypes.cast(function_address, ctypes.py_object).value
                if function.__code__ is code:
                    events.append(has(function))
            return 0

        add = api("PyFunction_AddWatcher", ctypes.c_int, callback_type)
        clear = api("PyFunction_ClearWatcher", ctypes.c_int, ctypes.c_int)
        watcher_id = add(watcher)
        try:
            result = self.call(invocation, root, value)
        finally:
            clear(watcher_id)
        self.assertEqual(events, [1])
        self.assertIs(result(), value)
        self.decline(invocation)

    def test_direct_keyword_unpack_call_preserves_frame_and_argument_lifetime(self):
        events = []

        class Value:
            def __del__(self):
                events.append("released")

        def create(value):
            del value
            events.append("after delete")

            def result():
                return 17

            return result

        def root():
            return create(**{"value": Value()})  # noqa: PIE804 - exercise CALL_FUNCTION_EX

        ordinary = root()
        self.assertEqual(ordinary(), 17)
        expected = events[:]
        self.assertEqual(expected, ["released", "after delete"])
        events.clear()
        invocation = _testinternalcapi.soac_dataclass_fixture(
            root,
            (),
            ((root, instruction(root, "CALL_FUNCTION_EX"), create, 1001, ()),),
            (
                (
                    create,
                    instruction(create, "MAKE_FUNCTION"),
                    child_code(create),
                    1,
                    (),
                ),
            ),
            None,
        )
        result = _testinternalcapi.soac_dataclass_fixture_call(
            invocation, 1, root, (), {}
        )
        self.assertEqual(self.matches(result, invocation), 1)
        self.assertEqual(result(), 17)
        self.assertEqual(events, expected)
        self.decline(invocation)

    def test_instrumented_call_ex_preserves_provenance_and_native_cleanup(self):
        events = []

        class Value:
            def __del__(self):
                events.append("released")

        def create(value):
            del value
            events.append("after delete")

            def result():
                return 17

            return result

        def root():
            return create(**{"value": Value()})  # noqa: PIE804 - instrumented CALL_FUNCTION_EX

        tool = 4
        sys.monitoring.use_tool_id(tool, "native dataclass call-ex regression")
        sys.monitoring.register_callback(
            tool, sys.monitoring.events.CALL, lambda *args: None
        )
        try:
            sys.monitoring.set_local_events(
                tool, root.__code__, sys.monitoring.events.CALL
            )
            ordinary = root()
            self.assertEqual(ordinary(), 17)
            expected = events[:]
            events.clear()
            invocation = _testinternalcapi.soac_dataclass_fixture(
                root,
                (),
                ((root, instruction(root, "CALL_FUNCTION_EX"), create, 1001, ()),),
                (
                    (
                        create,
                        instruction(create, "MAKE_FUNCTION"),
                        child_code(create),
                        1,
                        (),
                    ),
                ),
                None,
            )
            try:
                result = _testinternalcapi.soac_dataclass_fixture_call(
                    invocation, 1, root, (), {}
                )
                self.assertEqual(self.matches(result, invocation), 1)
                self.assertEqual(result(), 17)
                self.assertEqual(events, expected)
            finally:
                self.decline(invocation)
        finally:
            sys.monitoring.set_local_events(tool, root.__code__, 0)
            sys.monitoring.register_callback(tool, sys.monitoring.events.CALL, None)
            sys.monitoring.free_tool_id(tool)

    def test_wrong_actual_bound_value_rejects_before_source_body(self):
        invocation, root, _, value = self.fixture()
        with self.assertRaises(ImportError):
            self.call(invocation, root, object())
        # The function itself remains ordinary, independent of the failed
        # invocation's permission to produce adoption records.
        result = root(value)
        self.assertEqual(self.has(result), 0)
        self.assertIs(result(), value)

    def test_function_clones_do_not_copy_creation_records(self):
        invocation, root, _, value = self.fixture()
        result = self.call(invocation, root, value)
        clone = types.FunctionType(
            result.__code__, result.__globals__, closure=result.__closure__
        )
        self.assertEqual(self.has(clone), 0)
        owner_address = api(
            "PyFunction_GetSoacStrictOwner", ctypes.c_void_p, ctypes.py_object
        )(result)
        owner = ctypes.cast(owner_address, ctypes.py_object).value
        api(
            "PyFunction_SetSoacStrictOwner",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
        )(clone, owner)
        with self.assertRaises(ImportError):
            self.has(clone)
        self.assertEqual(self.has(result), 1)
        with self.assertRaises(TypeError):
            type(owner)()
        self.decline(invocation)

    def test_copied_creation_record_cannot_authenticate_a_privileged_frame(self):
        original, root, _, value = self.fixture()
        result = self.call(original, root, value)
        invocation, other_root, creator, other_value = self.fixture()
        owner_address = api(
            "PyFunction_GetSoacStrictOwner", ctypes.c_void_p, ctypes.py_object
        )(result)
        owner = ctypes.cast(owner_address, ctypes.py_object).value
        api(
            "PyFunction_SetSoacStrictOwner",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
        )(creator, owner)
        with self.assertRaises(ImportError):
            self.call(invocation, other_root, other_value)
        self.assertEqual(self.has(result), 1)
        self.assertIs(result(), value)
        self.decline(original)

    def test_declined_record_has_no_adoption_or_metadata_restriction(self):
        class Payload:
            pass

        payload = Payload()
        weak_payload = weakref.ref(payload)
        invocation, root, _, value = self.fixture(payload)
        del payload
        result = self.call(invocation, root, value)
        self.assertIsNotNone(weak_payload())
        self.decline(invocation)
        self.assertIsNone(weak_payload())

        def make_replacement(captured):
            return lambda: (captured, 17)

        replacement = make_replacement(value)
        result.__code__ = replacement.__code__
        self.assertEqual(self.has(result), 1)
        self.assertEqual(result(), (value, 17))
        with self.assertRaises(ImportError):
            self.matches(result, invocation)

    def test_declined_record_does_not_retain_replaced_code(self):
        namespace = {}
        exec(  # noqa: S102 - disposable native code tree for weak-lifetime regression
            compile(
                "def create(value):\n"
                "    def result(): return value\n"
                "    return result\n"
                "def root(value): return create(value)\n",
                "<native dataclass record code lifetime>",
                "exec",
            ),
            namespace,
        )
        root, create = namespace["root"], namespace["create"]
        value = object()
        invocation = _testinternalcapi.soac_dataclass_fixture(
            root,
            ((0, value),),
            ((root, instruction(root, "CALL"), create, 1001, ((0, value),)),),
            (
                (
                    create,
                    instruction(create, "MAKE_FUNCTION"),
                    child_code(create),
                    1,
                    ((0, value),),
                ),
            ),
            None,
        )
        result = self.call(invocation, root, value)
        old_code = weakref.ref(result.__code__)
        self.decline(invocation)

        def make_replacement(capture):
            return lambda: capture

        replacement = make_replacement(value)
        result.__code__ = replacement.__code__
        namespace.clear()
        del root, create
        self.assertIsNone(old_code())
        self.assertEqual(self.has(result), 1)
        self.assertIs(result(), value)

    def test_ordinary_generator_callback_does_not_acquire_or_block_provenance(self):
        def ordinary():
            yield 17

        def root():
            generator = ordinary()
            value = next(generator)

            def result():
                return value

            return result

        code = child_code(root)
        # This code has exactly one non-argument cell, appended after its fast
        # locals. Project that native slot, not a lookup in materialized locals.
        self.assertEqual(root.__code__.co_cellvars, ("value",))
        cell_index = root.__code__.co_nlocals
        invocation = _testinternalcapi.soac_dataclass_fixture(
            root,
            (),
            (),
            ((root, instruction(root, "MAKE_FUNCTION"), code, 1, ((cell_index, 17),)),),
            None,
        )
        result = _testinternalcapi.soac_dataclass_fixture_call(
            invocation, 1, root, (), {}
        )
        self.assertEqual(self.matches(result, invocation), 1)
        self.assertEqual(result(), 17)
        self.decline(invocation)

    def test_ordinary_c_proxy_does_not_transmit_frame_provenance(self):
        def create():
            def result():
                return 17

            return result

        def root():
            return _testinternalcapi.soac_dataclass_fixture_c_proxy(create)

        invocation = _testinternalcapi.soac_dataclass_fixture(
            root,
            (),
            ((root, instruction(root, "CALL"), create, 1001, ()),),
            (
                (
                    create,
                    instruction(create, "MAKE_FUNCTION"),
                    child_code(create),
                    1,
                    (),
                ),
            ),
            None,
        )
        result = _testinternalcapi.soac_dataclass_fixture_call(
            invocation, 1, root, (), {}
        )
        self.assertEqual(self.has(result), 0)
        self.assertEqual(result(), 17)
        self.decline(invocation)

    def test_escaped_record_does_not_retain_actual_function(self):
        invocation, root, _, value = self.fixture()
        result = self.call(invocation, root, value)
        owner_address = api(
            "PyFunction_GetSoacStrictOwner", ctypes.c_void_p, ctypes.py_object
        )(result)
        owner = ctypes.cast(owner_address, ctypes.py_object).value
        weak_result = weakref.ref(result)
        self.decline(invocation)
        del result
        self.assertIsNone(weak_result())
        # Retaining the private record cannot turn another function into its
        # original creation, even after the original address is dead.
        replacement = root(value)
        api(
            "PyFunction_SetSoacStrictOwner",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
        )(replacement, owner)
        with self.assertRaises(ImportError):
            self.has(replacement)
        del replacement, owner, invocation
        gc.collect()


if __name__ == "__main__":
    unittest.main()
