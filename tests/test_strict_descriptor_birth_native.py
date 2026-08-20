"""Raw descriptor-birth kernels, not strict-source admission fixtures.

The native caller supplies an already authenticated function and an inert
namespace witness. Production must additionally prove the actual MakeFunction
creation operand and its NamespaceExecution; assigning a test owner proves neither.
"""

import _testcapi
import _testinternalcapi
import ctypes
import gc
import itertools
import sys
import types
import unittest
import weakref


def native_api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.restype = result
    function.argtypes = arguments
    return function


class InertOwner:
    """Identity-only stand-in for the caller's zero-Python-edge Rust payload."""

    __slots__ = ("__weakref__",)


class DescriptorBirthNativeTests(unittest.TestCase):
    factories = (staticmethod, classmethod, property)

    @classmethod
    def setUpClass(cls):
        cls.new = native_api(
            "PySoac_NewBuiltinDescriptor", ctypes.py_object, *([ctypes.py_object] * 5)
        )
        cls.birth_owner = native_api(
            "PySoac_GetDescriptorBirthOwner", ctypes.c_void_p, ctypes.py_object
        )
        cls.matches = native_api(
            "PySoac_MatchesDescriptorBirth", ctypes.c_int, *([ctypes.py_object] * 5)
        )
        cls.adopt = native_api(
            "PySoac_AdoptBuiltinDescriptor", ctypes.c_int, *([ctypes.py_object] * 5)
        )
        cls.set_function_owner = native_api(
            "PyFunction_SetSoacStrictOwner",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
        )
        cls.is_sealed = native_api(
            "_PySoac_IsDescriptorSealed", ctypes.c_int, ctypes.py_object
        )
        cls.strict_id = native_api(
            "PyFunction_GetSoacStrictId", ctypes.c_uint64, ctypes.py_object
        )
        error = native_api("PySoac_GetStrictMutationError", ctypes.c_void_p)
        cls.mutation_error = ctypes.cast(error(), ctypes.py_object).value

    def function(self):
        def function(*arguments):
            """The original function docstring."""
            return arguments

        owner = InertOwner()
        self.set_function_owner(function, owner)
        return function, owner

    @staticmethod
    def component(descriptor):
        if type(descriptor) is property:
            return descriptor.fget
        return descriptor.__func__

    @staticmethod
    def birth_id(descriptor):
        return native_api(
            "PySoac_GetDescriptorBirthId", ctypes.c_uint64, ctypes.py_object
        )(descriptor)

    @staticmethod
    def runtime_error():
        getter = native_api("PySoac_GetStrictRuntimeUnavailableError", ctypes.c_void_p)
        return ctypes.cast(getter(), ctypes.py_object).value

    def test_birth_id_distinguishes_reconstruction_with_the_same_exposed_witness(self):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                code = function.__code__
                original = self.new(factory, function, owner, code, namespace)
                identity = self.birth_id(original)
                exposed = ctypes.cast(
                    self.birth_owner(original), ctypes.py_object
                ).value
                reconstructed = self.new(factory, function, owner, code, exposed)
                self.assertGreater(identity, 0)
                self.assertGreater(self.birth_id(reconstructed), identity)
                # The old operand matcher intentionally recognizes both native
                # births. Source adoption must additionally pin the original ID.
                self.assertEqual(
                    self.matches(reconstructed, namespace, function, owner, code), 1
                )
                self.adopt(original, namespace, function, owner, code)
                original.__doc__ = "ordinary metadata does not change birth identity"
                self.assertEqual(self.birth_id(original), identity)
                self.assertNotEqual(self.birth_id(reconstructed), identity)

    def test_birth_ids_are_not_reused_after_descriptor_release(self):
        function, owner = self.function()
        namespace = InertOwner()
        code = function.__code__
        identities = []
        for index in range(96):
            factory = self.factories[index % len(self.factories)]
            descriptor = self.new(factory, function, owner, code, namespace)
            identities.append(self.birth_id(descriptor))
            ordinary = factory(function)
            copied = (
                descriptor.getter(function)
                if factory is property
                else factory(descriptor.__func__)
            )
            self.assertEqual(self.birth_id(ordinary), 0)
            self.assertEqual(self.birth_id(copied), 0)
            del descriptor, ordinary, copied
        self.assertGreater(identities[0], 0)
        self.assertTrue(
            all(left < right for left, right in itertools.pairwise(identities))
        )

    def test_birth_id_reinitialization_and_code_replacement_invalidate_the_observation(
        self,
    ):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                descriptor = self.new(
                    factory, function, owner, function.__code__, namespace
                )
                self.assertGreater(self.birth_id(descriptor), 0)
                descriptor.__init__(function)
                self.assertEqual(self.birth_id(descriptor), 0)
                descriptor = self.new(
                    factory, function, owner, function.__code__, namespace
                )
                self.assertGreater(self.birth_id(descriptor), 0)
                function.__code__ = function.__code__.replace()
                self.assertEqual(self.birth_id(descriptor), 0)

    def test_birth_id_is_unique_during_gc_reentrant_construction(self):
        function, owner = self.function()
        namespace = InertOwner()
        code = function.__code__
        new = _testinternalcapi.soac_new_builtin_descriptor
        inner = []
        errors = []

        def callback(phase, info):
            if phase != "start" or inner or errors:
                return
            try:
                # Observe only the inert native record while the outer
                # constructor is allocating its ordinary descriptor/metadata.
                if not any(
                    type(value).__name__ == "_SoacDescriptorBirth"
                    and any(edge is namespace for edge in gc.get_referents(value))
                    for value in gc.get_objects()
                ):
                    return
                inner.append(new(staticmethod, function, owner, code, namespace))
            except BaseException as error:  # noqa: BLE001 - surface callback errors in the test.
                errors.append(error)

        gc.collect()
        thresholds = gc.get_threshold()
        gc.callbacks.append(callback)
        try:
            gc.set_threshold(1, 1, 1)
            outer = new(staticmethod, function, owner, code, namespace)
        finally:
            gc.set_threshold(*thresholds)
            gc.callbacks.remove(callback)
        self.assertEqual(errors, [])
        self.assertEqual(len(inner), 1)
        self.assertGreater(self.birth_id(outer), 0)
        self.assertGreater(self.birth_id(inner[0]), self.birth_id(outer))

    def test_birth_id_exhaustion_fails_without_publication_or_counter_reuse(self):
        function, owner = self.function()
        namespace = InertOwner()
        code = function.__code__
        before = self.new(staticmethod, function, owner, code, namespace)
        identity = self.birth_id(before)
        with self.assertRaisesRegex(
            self.runtime_error(), "identity space is exhausted"
        ):
            # The fixture only blocks reservation at UINT64_MAX; it cannot mint
            # IDs or expose an API for resetting a used identity counter.
            _testinternalcapi.soac_descriptor_birth_exhaustion(
                staticmethod, function, owner, code, namespace
            )
        after = self.new(staticmethod, function, owner, code, namespace)
        self.assertEqual(self.birth_id(before), identity)
        self.assertGreater(self.birth_id(after), identity)

    def test_birth_id_and_other_birth_observers_reject_foreign_interpreters(self):
        function, owner = self.function()
        namespace = InertOwner()
        code = function.__code__
        descriptor = self.new(staticmethod, function, owner, code, namespace)
        identity = self.birth_id(descriptor)
        self.assertEqual(
            _testinternalcapi.soac_descriptor_birth_foreign(
                descriptor, namespace, function, owner, code
            ),
            4,
        )
        self.assertEqual(self.birth_id(descriptor), identity)
        self.assertEqual(self.matches(descriptor, namespace, function, owner, code), 1)
        self.assertEqual(self.is_sealed(descriptor), 0)

    def test_birth_id_preserves_terminal_errors_and_does_not_read_python_attributes(
        self,
    ):
        function, owner = self.function()
        namespace = InertOwner()
        descriptor = self.new(
            staticmethod, function, owner, function.__code__, namespace
        )
        self.assertGreater(self.birth_id(descriptor), 0)
        get_slot = native_api(
            "PyType_GetSlot", ctypes.c_void_p, ctypes.py_object, ctypes.c_int
        )
        clear = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object)(
            get_slot(types.FunctionType, 51)
        )
        self.assertEqual(clear(function), 0)
        with self.assertRaises(self.runtime_error()):
            self.birth_id(descriptor)

        class Hostile:
            def __getattribute__(self, name):
                raise AssertionError(
                    "birth observation must not read Python attributes"
                )

            def __eq__(self, other):
                raise AssertionError("birth observation must not call equality")

        self.assertEqual(self.birth_id(ctypes.py_object(Hostile())), 0)
        raw = native_api(
            "PySoac_GetDescriptorBirthId", ctypes.c_uint64, ctypes.c_void_p
        )
        with self.assertRaises(TypeError):
            raw(None)

    def test_fresh_exact_builtin_wrappers_keep_ordinary_dispatch_and_metadata(self):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                descriptor = self.new(
                    factory, function, owner, function.__code__, namespace
                )
                self.assertIs(type(descriptor), factory)
                self.assertIs(self.component(descriptor), function)
                self.assertEqual(self.birth_owner(descriptor), id(namespace))
                self.assertEqual(
                    self.matches(
                        descriptor, namespace, function, owner, function.__code__
                    ),
                    1,
                )
                self.assertEqual(self.is_sealed(descriptor), 0)
                self.assertEqual(self.strict_id(function), 0)
                self.assertEqual(descriptor.__doc__, function.__doc__)

                class Receiver:
                    member = descriptor

                receiver = Receiver()
                if factory is property:
                    self.assertEqual(receiver.member, (receiver,))
                elif factory is classmethod:
                    self.assertEqual(receiver.member(7), (Receiver, 7))
                else:
                    self.assertEqual(receiver.member(7), (7,))
                    self.assertEqual(descriptor(8), (8,))

    def test_adoption_is_identity_checked_idempotent_and_permanently_seals_components(
        self,
    ):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                code = function.__code__
                descriptor = self.new(factory, function, owner, code, namespace)
                self.assertEqual(
                    self.adopt(descriptor, namespace, function, owner, code), 0
                )
                self.assertEqual(
                    self.adopt(descriptor, namespace, function, owner, code), 0
                )
                self.assertEqual(self.is_sealed(descriptor), 1)
                for replacement in (function, lambda: None):
                    with self.assertRaises(self.mutation_error):
                        descriptor.__init__(replacement)
                self.assertIs(self.component(descriptor), function)
                self.assertEqual(
                    self.matches(descriptor, namespace, function, owner, code), 1
                )
                # The birth API gives no function-body, checked-entry, or JIT authority.
                self.assertEqual(self.strict_id(function), 0)
                function.__defaults__ = ()
                descriptor.__doc__ = "ordinary metadata remains mutable"
                if factory is not property:
                    descriptor.__dict__ = {"note": "not native authority"}
                self.assertEqual(
                    self.matches(descriptor, namespace, function, owner, code), 1
                )

    def test_wrong_namespace_function_owner_and_code_do_not_adopt_or_revoke(self):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                code = function.__code__
                descriptor = self.new(factory, function, owner, code, namespace)
                clone = types.FunctionType(code, function.__globals__)
                self.set_function_owner(clone, owner)
                wrong_operands = (
                    (InertOwner(), function, owner, code),
                    (namespace, clone, owner, code),
                    (namespace, function, InertOwner(), code),
                    (namespace, function, owner, code.replace()),
                )
                for operands in wrong_operands:
                    self.assertEqual(self.matches(descriptor, *operands), 0)
                    with self.assertRaises(self.mutation_error):
                        self.adopt(descriptor, *operands)
                self.assertEqual(self.is_sealed(descriptor), 0)
                self.adopt(descriptor, namespace, function, owner, code)
                for operands in wrong_operands:
                    with self.assertRaises(self.mutation_error):
                        self.adopt(descriptor, *operands)
                self.assertEqual(self.is_sealed(descriptor), 1)

    def test_public_old_and_copied_wrappers_never_inherit_birth(self):
        function, owner = self.function()
        namespace = InertOwner()
        code = function.__code__
        for factory in self.factories:
            with self.subTest(factory=factory):
                fresh = self.new(factory, function, owner, code, namespace)
                ordinary = factory(function)
                if factory is property:
                    copied = fresh.getter(function)
                    mixed = fresh.setter(lambda self, value: None)
                else:
                    copied = factory(fresh.__func__)
                    copied.__dict__.update(fresh.__dict__)
                    mixed = factory(fresh)
                for descriptor in (ordinary, copied, mixed):
                    self.assertIsNone(self.birth_owner(descriptor))
                    self.assertEqual(
                        self.matches(descriptor, namespace, function, owner, code), 0
                    )
                    with self.assertRaises(self.mutation_error):
                        self.adopt(descriptor, namespace, function, owner, code)
                    self.assertEqual(self.is_sealed(descriptor), 0)

    def test_only_exact_builtin_factories_and_owned_exact_functions_are_accepted(self):
        function, owner = self.function()
        namespace = InertOwner()

        class Subclass(staticmethod):
            def __new__(cls, *arguments):
                raise AssertionError("unknown factories must not execute")

        for factory in (Subclass, lambda value: value, None, object()):
            with self.assertRaises(TypeError):
                self.new(factory, function, owner, function.__code__, namespace)
        with self.assertRaises(TypeError):
            self.new(staticmethod, object(), owner, function.__code__, namespace)
        for candidate, candidate_owner, code in (
            (lambda: None, owner, function.__code__),
            (function, InertOwner(), function.__code__),
            (function, owner, function.__code__.replace()),
        ):
            with self.assertRaises(self.mutation_error):
                self.new(staticmethod, candidate, candidate_owner, code, namespace)
        with self.assertRaises(TypeError):
            self.new(staticmethod, function, owner, None, namespace)

    def test_valid_reinitialization_invalidates_birth_even_when_component_is_identical(
        self,
    ):
        for factory in self.factories:
            for replace in (False, True):
                with self.subTest(factory=factory, replace=replace):
                    function, owner = self.function()
                    namespace = InertOwner()
                    code = function.__code__
                    descriptor = self.new(factory, function, owner, code, namespace)
                    replacement = (lambda: 19) if replace else function
                    descriptor.__init__(replacement)
                    self.assertIs(self.component(descriptor), replacement)
                    self.assertIsNone(self.birth_owner(descriptor))
                    self.assertEqual(
                        self.matches(descriptor, namespace, function, owner, code), 0
                    )
                    descriptor.__init__(function)
                    with self.assertRaises(self.mutation_error):
                        self.adopt(descriptor, namespace, function, owner, code)
                    self.assertEqual(self.is_sealed(descriptor), 0)

    def test_argument_errors_before_reinitialization_leave_birth_unchanged(self):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                code = function.__code__
                descriptor = self.new(factory, function, owner, code, namespace)
                with self.assertRaises(TypeError):
                    descriptor.__init__(unexpected_keyword=function)
                self.assertEqual(
                    self.matches(descriptor, namespace, function, owner, code), 1
                )

    def test_birth_is_invalid_before_displaced_function_weakref_callbacks(self):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                descriptor = self.new(
                    factory, function, owner, function.__code__, namespace
                )
                events = []
                reference = weakref.ref(
                    function,
                    lambda reference, descriptor=descriptor, events=events: (
                        events.append(
                            (self.birth_owner(descriptor), self.is_sealed(descriptor))
                        )
                    ),
                )
                del function
                replacement = lambda: 21
                descriptor.__init__(replacement)
                self.assertIsNone(reference())
                self.assertEqual(events, [(None, 0)])
                self.assertIs(self.component(descriptor), replacement)

    def test_current_function_code_and_native_owner_are_rechecked_without_attributes(
        self,
    ):
        function, owner = self.function()
        namespace = InertOwner()
        code = function.__code__
        descriptor = self.new(staticmethod, function, owner, code, namespace)
        function.__code__ = code.replace()
        self.assertIsNone(self.birth_owner(descriptor))
        self.assertEqual(self.matches(descriptor, namespace, function, owner, code), 0)
        with self.assertRaises(self.mutation_error):
            self.adopt(descriptor, namespace, function, owner, code)
        self.assertEqual(function(23), (23,))
        self.assertEqual(self.is_sealed(descriptor), 0)

        class Hostile:
            def __getattribute__(self, name):
                raise AssertionError("native matching must not read Python attributes")

            def __eq__(self, other):
                raise AssertionError("native matching must not call equality")

        hostile = ctypes.py_object(Hostile())
        self.assertIsNone(self.birth_owner(hostile))
        self.assertEqual(self.matches(hostile, namespace, function, owner, code), 0)
        self.assertEqual(self.matches(descriptor, hostile, function, owner, code), 0)

    def test_record_adds_no_function_or_function_owner_or_code_reference(self):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                code = function.__code__
                before = tuple(
                    sys.getrefcount(value) for value in (function, owner, code)
                )
                descriptor = self.new(factory, function, owner, code, namespace)
                after = tuple(
                    sys.getrefcount(value) for value in (function, owner, code)
                )
                self.assertEqual(after, (before[0] + 1, before[1], before[2]))
                self.adopt(descriptor, namespace, function, owner, code)
                del descriptor
                self.assertEqual(
                    tuple(sys.getrefcount(value) for value in (function, owner, code)),
                    before,
                )

    def test_original_code_witness_is_weak_after_unsealed_code_replacement(self):
        function, owner = self.function()
        function.__code__ = function.__code__.replace()
        code = function.__code__
        reference = weakref.ref(code)
        namespace = InertOwner()
        descriptor = self.new(staticmethod, function, owner, code, namespace)
        function.__code__ = code.replace()
        del code
        self.assertIsNone(reference())
        self.assertIsNone(self.birth_owner(descriptor))

    def test_allocation_failures_release_partial_birth_and_component_references(self):
        set_nomemory = _testcapi.set_nomemory
        remove_mem_hooks = _testcapi.remove_mem_hooks
        new = _testinternalcapi.soac_new_builtin_descriptor
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                code = function.__code__
                # A direct FASTCALL fixture avoids ctypes.ArgumentError wrapping
                # failures in argument conversion before the native API runs.
                descriptor = new(factory, function, owner, code, namespace)
                del descriptor
                before = tuple(
                    sys.getrefcount(value)
                    for value in (function, owner, code, namespace)
                )
                failures = successes = 0
                for start in range(1, 48):
                    descriptor = None
                    try:
                        set_nomemory(start, start + 1)
                        try:
                            descriptor = new(factory, function, owner, code, namespace)
                        finally:
                            remove_mem_hooks()
                    except MemoryError:
                        failures += 1
                    else:
                        successes += 1
                    finally:
                        remove_mem_hooks()
                    del descriptor
                    self.assertEqual(
                        tuple(
                            sys.getrefcount(value)
                            for value in (function, owner, code, namespace)
                        ),
                        before,
                    )
                self.assertGreater(failures, 0)
                self.assertGreater(successes, 0)

    def test_escaped_inert_record_cannot_retain_descriptor_function_or_namespace(self):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                references = tuple(
                    weakref.ref(value) for value in (function, owner, namespace)
                )
                descriptor = self.new(
                    factory, function, owner, function.__code__, namespace
                )
                record = next(
                    value
                    for value in gc.get_referents(descriptor)
                    if type(value).__name__ == "_SoacDescriptorBirth"
                )
                with self.assertRaises(TypeError):
                    type(record)()
                self.assertFalse(
                    any(value is function for value in gc.get_referents(record))
                )
                del descriptor, function, owner, namespace
                self.assertEqual(
                    tuple(reference() for reference in references), (None,) * 3
                )
                self.assertEqual(gc.get_referents(record), [])

    def test_function_descriptor_cycles_are_collectable_without_extra_lifetime_edges(
        self,
    ):
        for factory in self.factories:
            with self.subTest(factory=factory):
                function, owner = self.function()
                namespace = InertOwner()
                references = tuple(
                    weakref.ref(value) for value in (function, owner, namespace)
                )
                descriptor = self.new(
                    factory, function, owner, function.__code__, namespace
                )
                function.wrapped = descriptor
                self.adopt(descriptor, namespace, function, owner, function.__code__)
                del descriptor, function, owner, namespace
                gc.collect()
                self.assertEqual(
                    tuple(reference() for reference in references), (None,) * 3
                )

    def test_null_native_operands_are_reported_without_dereference(self):
        function, owner = self.function()
        namespace = InertOwner()
        arguments = [
            id(staticmethod),
            id(function),
            id(owner),
            id(function.__code__),
            id(namespace),
        ]
        raw_new = ctypes.PYFUNCTYPE(ctypes.py_object, *([ctypes.c_void_p] * 5))(
            ctypes.cast(self.new, ctypes.c_void_p).value
        )
        for index in range(5):
            invalid = arguments[:]
            invalid[index] = None
            with self.subTest(index=index), self.assertRaises(TypeError):
                raw_new(*invalid)


if __name__ == "__main__":
    unittest.main()
