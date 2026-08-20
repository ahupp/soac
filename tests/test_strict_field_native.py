"""Checked attribute transactions against the real native dictionary kernel.

The C test fixture owns an exact-int storage policy, not artifact authority.
Run with the selected patched CPython; ordinary and policy-bearing receivers
exercise the same public attribute entrypoints and descriptor/key callbacks.
"""

import _testinternalcapi
import ctypes
import gc
import struct
import subprocess
import sys
import unittest
import weakref


class StrictFieldNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native_setattr = ctypes.pythonapi.PyObject_SetAttr
        cls.native_setattr.argtypes = [ctypes.py_object] * 3
        cls.native_setattr.restype = ctypes.c_int
        cls.generic_setattr = ctypes.pythonapi.PyObject_GenericSetAttr
        cls.generic_setattr.argtypes = [ctypes.py_object] * 3
        cls.generic_setattr.restype = ctypes.c_int
        cls.explicit_setattr = ctypes.pythonapi._PyObject_GenericSetAttrWithDict
        cls.explicit_setattr.argtypes = [ctypes.py_object] * 4
        cls.explicit_setattr.restype = ctypes.c_int
        cls.set_dictionary = ctypes.pythonapi.PyObject_GenericSetDict
        cls.set_dictionary.argtypes = [
            ctypes.py_object,
            ctypes.py_object,
            ctypes.c_void_p,
        ]
        cls.set_dictionary.restype = ctypes.c_int
        cls.has_policy = ctypes.pythonapi.PyDict_HasSoacPolicy
        cls.has_policy.argtypes = [ctypes.py_object]
        cls.has_policy.restype = ctypes.c_int
        cls.has_contract = ctypes.pythonapi.PyType_HasSoacContract
        cls.has_contract.argtypes = [ctypes.py_object]
        cls.has_contract.restype = ctypes.c_int

    @staticmethod
    def native_type(name, fields=(), *, members=None, protected=(), final=()):
        def namespace_function():
            pass

        arguments = (
            name,
            (),
            {"__module__": __name__, **(members or {})},
            fields,
            namespace_function,
        )
        if protected or final:
            arguments += (0, protected, final)
        return _testinternalcapi.dict_new_soac_type(*arguments)

    def receiver(self, protected, members=None):
        namespace = {"__module__": __name__, **(members or {})}
        if protected:

            def namespace_function():
                pass

            kind = _testinternalcapi.dict_new_soac_type(
                "Checked", (), namespace, ("field",), namespace_function
            )
        else:
            kind = type("Ordinary", (), namespace)
        instance = kind()
        vars(instance)
        return instance

    def write(self, operation, receiver, name, value):
        if operation == "setattr":
            setattr(receiver, name, value)
        elif operation == "object_setattr":
            object.__setattr__(receiver, name, value)
        else:
            arguments = [ctypes.py_object(item) for item in (receiver, name, value)]
            if operation == "native":
                self.native_setattr(*arguments)
            elif operation == "generic":
                self.generic_setattr(*arguments)
            elif operation == "explicit":
                self.explicit_setattr(*arguments, ctypes.py_object(vars(receiver)))
            else:
                self.fail(f"unknown test operation {operation}")

    operations = ("setattr", "object_setattr", "native", "generic", "explicit")

    def test_ordinary_subclass_of_empty_native_factory_keeps_dictionary_authority(self):
        # Unlike the older type-only fixture, this parent always installs the
        # production-shaped non-NULL native instance dictionary factory.
        base = self.native_type("EmptyParent", members={"method": lambda self: 1})
        actual = base()
        self.assertEqual(self.has_policy(vars(actual)), 1)
        with self.assertRaisesRegex(TypeError, "strict instance dictionary"):
            self.set_dictionary(actual, {}, None)
        for requested_slots in (False, True):
            with self.subTest(requested_slots=requested_slots):
                namespace = {"__slots__": ()} if requested_slots else {}
                ordinary = type("OrdinaryChild", (base,), namespace)
                instance = ordinary()
                self.assertEqual(self.has_contract(ordinary), 0)
                original = vars(instance)
                replacement = {"method": lambda: 29}
                self.assertEqual(self.set_dictionary(instance, replacement, None), 0)
                self.assertEqual(self.has_policy(original), 0)
                self.assertIs(vars(instance), replacement)
                self.assertEqual(instance.method(), 29)
                instance.method = lambda: 31
                self.assertEqual(instance.method(), 31)
                replacement = {"method": lambda: 37}
                object.__setattr__(instance, "__dict__", replacement)
                self.assertIs(vars(instance), replacement)
                self.assertEqual(instance.method(), 37)
                # An actual strict receiver still cannot be reached by an
                # ordinary class reassignment across its installed contract.
                with self.assertRaisesRegex(TypeError, "strict contract"):
                    instance.__class__ = base

    def test_nonempty_inherited_fields_keep_policy_even_after_empty_factory_base(self):
        empty = self.native_type("EmptyPrefix")
        checked = self.native_type("CheckedPrefix", ("field",))
        for bases in ((checked,), (empty, checked)):
            with self.subTest(bases=bases):
                ordinary = type("OrdinaryFields", bases, {"__slots__": ()})
                instance = ordinary()
                self.assertEqual(self.has_contract(ordinary), 0)
                self.assertEqual(self.has_policy(vars(instance)), 1)
                instance.field = 41
                self.assertEqual(instance.field, 41)
                with self.assertRaisesRegex(TypeError, "exact int"):
                    instance.field = "wrong"
                with self.assertRaisesRegex(TypeError, "exact int"):
                    vars(instance)["field"] = "wrong"
                with self.assertRaisesRegex(TypeError, "strict instance dictionary"):
                    self.set_dictionary(instance, {"field": 43}, None)
                unrelated = type("OrdinaryNoFields", (empty,), {"__slots__": ()})
                with self.assertRaisesRegex(TypeError, "strict contract"):
                    instance.__class__ = unrelated

    def test_empty_factory_does_not_remove_inherited_final_method_barriers(self):
        base = self.native_type(
            "FinalMethodParent",
            members={"method": lambda self: 47},
            protected=("method",),
            final=("method",),
        )
        ordinary = type("OrdinaryFinalChild", (base,), {})
        instance = ordinary()
        self.assertEqual(self.has_policy(vars(instance)), 0)
        self.set_dictionary(instance, {}, None)
        self.assertEqual(instance.method(), 47)
        with self.assertRaisesRegex(TypeError, "final"):
            type("InvalidOverride", (base,), {"method": lambda self: 0})
        with self.assertRaisesRegex(TypeError, "final"):
            ordinary.method = lambda self: 0

    def test_string_subclass_attribute_payload_is_checked_without_extra_hooks(self):
        for operation in self.operations:
            with self.subTest(operation=operation):
                events = []

                class Name(str):
                    def __hash__(self):
                        events.append("hash")
                        return str.__hash__(self)

                    def __eq__(self, other):
                        events.append("eq")
                        return str.__eq__(self, other)

                    def __str__(self):
                        raise AssertionError("attribute checking called __str__")

                name = Name("field")
                ordinary = self.receiver(False)
                self.write(operation, ordinary, name, "wrong value")
                expected_events = list(events)
                events.clear()

                checked = self.receiver(True)
                with self.assertRaisesRegex(TypeError, "exact int"):
                    self.write(operation, checked, name, "wrong value")
                self.assertEqual(events, expected_events)
                self.assertEqual(vars(checked), {})
                events.clear()
                self.write(operation, checked, name, 7)
                self.assertEqual(events, expected_events)
                self.assertEqual(len(vars(checked)), 1)

    def test_key_lookup_exception_precedes_required_value_error(self):
        for operation in self.operations:
            for protected in (False, True):
                with self.subTest(operation=operation, protected=protected):
                    events = []
                    expected = ValueError("original equality failure")

                    class Alias:
                        def __hash__(self):
                            return hash("field")

                        def __eq__(self, other):
                            events.append("eq")
                            raise expected

                    receiver = self.receiver(protected)
                    dictionary = vars(receiver)
                    dictionary[Alias()] = 1
                    events.clear()
                    with self.assertRaises(ValueError) as caught:
                        self.write(operation, receiver, "field", "wrong value")
                    self.assertIs(caught.exception, expected)
                    self.assertEqual(events, ["eq"])
                    self.assertEqual(len(dictionary), 1)

    def test_attribute_name_hash_error_has_stock_order_and_identity(self):
        for operation in self.operations:
            observed = []
            for protected in (False, True):
                events = []
                expected = ValueError("original name hash failure")

                class Name(str):
                    def __hash__(self):
                        events.append("hash")
                        raise expected

                receiver = self.receiver(protected)
                with self.subTest(operation=operation, protected=protected):
                    with self.assertRaises(ValueError) as caught:
                        self.write(operation, receiver, Name("field"), "wrong value")
                    self.assertIs(caught.exception, expected)
                    self.assertEqual(vars(receiver), {})
                    observed.append(events)
            self.assertEqual(observed[0], observed[1])

    def test_descriptor_error_precedes_dictionary_value_check(self):
        for operation in self.operations:
            for protected in (False, True):
                with self.subTest(operation=operation, protected=protected):
                    events = []
                    expected = ValueError("original descriptor failure")

                    def setter(receiver, value):
                        events.append(value)
                        raise expected

                    receiver = self.receiver(
                        protected, {"field": property(fset=setter)}
                    )
                    with self.assertRaises(ValueError) as caught:
                        self.write(operation, receiver, "field", "wrong value")
                    self.assertIs(caught.exception, expected)
                    self.assertEqual(events, ["wrong value"])
                    self.assertEqual(vars(receiver), {})

    def test_mapping_keys_are_not_normalized_into_attribute_names(self):
        class Name(str):
            def __hash__(self):
                return str.__hash__(self)

        receiver = self.receiver(True)
        dictionary = vars(receiver)
        key = Name("field")
        dictionary[key] = "ordinary alias-sensitive mapping value"
        self.assertIs(next(iter(dictionary)), key)
        self.assertFalse(_testinternalcapi.dict_has_no_lookup_aliases(dictionary))
        self.assertEqual(
            _testinternalcapi.dict_indexed_key_index(dictionary, "field"), 0
        )

    def test_attribute_replacement_releases_guard_before_old_value_finalizer(self):
        for operation in self.operations:
            for protected in (False, True):
                with self.subTest(operation=operation, protected=protected):
                    events = []
                    receiver = self.receiver(protected)

                    class Previous:
                        def __del__(self):
                            events.append(receiver.other)
                            receiver.field = 7

                    receiver.other = Previous()
                    self.write(operation, receiver, "other", None)
                    self.assertEqual(events, [None])
                    self.assertEqual(receiver.field, 7)
                    self.assertIsNone(receiver.other)



class OrdinaryStrictFieldNativeTests(unittest.TestCase):
    """Native field policy on actual ordinary inline/split/combined storage.

    This fixture supplies only native policy callbacks, never source/checker
    authority. The backend=cpython integration cases cover authenticated entry.
    """

    @classmethod
    def setUpClass(cls):
        StrictFieldNativeTests.setUpClass.__func__(cls)

    operations = StrictFieldNativeTests.operations
    write = StrictFieldNativeTests.write

    @staticmethod
    def native_type(name="OrdinaryStorage", fields=("field",), *, members=None):
        def namespace_function():
            pass

        return _testinternalcapi.dict_new_soac_ordinary_type(
            name, (), {"__module__": __name__, **(members or {})},
            fields, namespace_function,
        )

    def receiver(self, protected, members=None):
        kind = (
            self.native_type(members=members)
            if protected
            else type("Ordinary", (), {"__module__": __name__, **(members or {})})
        )
        instance = kind()
        vars(instance)
        return instance

    # The same descriptor/hash/alias/finalizer probes must now pass on real
    # ordinary storage as well as the existing indexed fixture.
    test_string_subclass_attribute_payload_is_checked_without_extra_hooks = (
        StrictFieldNativeTests.test_string_subclass_attribute_payload_is_checked_without_extra_hooks
    )
    test_key_lookup_exception_precedes_required_value_error = (
        StrictFieldNativeTests.test_key_lookup_exception_precedes_required_value_error
    )
    test_attribute_name_hash_error_has_stock_order_and_identity = (
        StrictFieldNativeTests.test_attribute_name_hash_error_has_stock_order_and_identity
    )
    test_descriptor_error_precedes_dictionary_value_check = (
        StrictFieldNativeTests.test_descriptor_error_precedes_dictionary_value_check
    )
    test_attribute_replacement_releases_guard_before_old_value_finalizer = (
        StrictFieldNativeTests.test_attribute_replacement_releases_guard_before_old_value_finalizer
    )

    def test_actual_inline_then_split_storage_and_no_indexed_capability(self):
        kind = self.native_type(members={"field": 17})
        instance = kind()
        self.assertTrue(_testinternalcapi.has_inline_values(instance))
        self.assertEqual(instance.field, 17)
        instance.field = 19
        self.assertTrue(_testinternalcapi.has_inline_values(instance))
        with self.assertRaisesRegex(TypeError, "exact int"):
            instance.field = "wrong"
        self.assertEqual(instance.field, 19)
        dictionary = vars(instance)
        self.assertIs(vars(instance), dictionary)
        self.assertTrue(_testinternalcapi.has_split_table(dictionary))
        self.assertEqual(self.has_policy(dictionary), 1)
        self.assertEqual(dict(dictionary), {"field": 19})
        with self.assertRaisesRegex(TypeError, "expected an indexed dictionary"):
            _testinternalcapi.dict_indexed_key_index(dictionary, "field")
        with self.assertRaisesRegex(TypeError, "exact int"):
            dictionary["field"] = "wrong"
        del dictionary["field"]
        self.assertEqual(instance.field, 17)

    def test_warmed_ordinary_subclass_keeps_native_inherited_checks(self):
        base = self.native_type()
        kind = type("OrdinaryChild", (base,), {})
        self.assertEqual(self.has_contract(kind), 0)
        instance = kind()

        def store(receiver, value):
            receiver.field = value

        for i in range(300):
            store(instance, i)
        self.assertTrue(_testinternalcapi.has_inline_values(instance))
        with self.assertRaisesRegex(TypeError, "exact int"):
            store(instance, "wrong")
        self.assertEqual(instance.field, 299)
        for operation in self.operations:
            with self.subTest(operation=operation):
                with self.assertRaisesRegex(TypeError, "exact int"):
                    self.write(operation, instance, "field", "wrong")
        self.assertEqual(instance.field, 299)

    def test_replacement_preserves_incoming_identity_alias_policy_and_receiver_lifetime(self):
        import gc
        import weakref

        class DictSubclass(dict):
            pass

        for operation in ("python", "object", "native"):
            for dictionary_type in (dict, DictSubclass):
                with self.subTest(operation=operation, dictionary_type=dictionary_type):
                    kind = self.native_type()
                    instance = kind()
                    instance.field = 23
                    previous = vars(instance)
                    incoming = dictionary_type(field=29, extra=31)
                    if operation == "python":
                        instance.__dict__ = incoming
                    elif operation == "object":
                        object.__setattr__(instance, "__dict__", incoming)
                    else:
                        self.set_dictionary(instance, incoming, None)
                    self.assertIs(vars(instance), incoming)
                    self.assertEqual(instance.field, 29)
                    self.assertEqual(previous, {"field": 23})
                    self.assertEqual(self.has_policy(previous), 1)
                    self.assertEqual(self.has_policy(incoming), 1)
                    for alias in (previous, incoming):
                        with self.assertRaisesRegex(TypeError, "exact int"):
                            alias["field"] = "wrong"
                    witness = weakref.ref(instance)
                    del instance
                    gc.collect()
                    self.assertIsNone(witness())
                    previous["field"] = 37
                    incoming["field"] = 41

    def test_failed_candidate_validation_leaves_receiver_and_candidate_unmodified(self):
        class DictSubclass(dict):
            pass

        for dictionary_type in (dict, DictSubclass):
            instance = self.native_type()()
            instance.field = 43
            previous = vars(instance)
            incoming = dictionary_type(field="wrong", extra=47)
            with self.assertRaisesRegex(TypeError, "exact int"):
                self.set_dictionary(instance, incoming, None)
            self.assertIs(vars(instance), previous)
            self.assertEqual(instance.field, 43)
            self.assertEqual(dict(incoming), {"field": "wrong", "extra": 47})
            self.assertEqual(self.has_policy(incoming), 0)
            incoming["field"] = "still ordinary after failed attachment"

    def test_compatible_dictionary_sharing_and_foreign_policy_refusal(self):
        kind = self.native_type()
        first, second = kind(), kind()
        first.field = 53
        shared = vars(first)
        second.__dict__ = shared
        self.assertIs(vars(second), shared)
        second.field = 59
        self.assertEqual(first.field, 59)

        class Ordinary:
            pass

        unowned = Ordinary()
        unowned.__dict__ = shared
        with self.assertRaisesRegex(TypeError, "exact int"):
            unowned.field = "wrong"
        replacement = {"field": "ordinary"}
        unowned.__dict__ = replacement
        self.assertIs(vars(unowned), replacement)
        unowned.field = "allowed after dropping the protected alias"
        self.assertEqual(first.field, 59)
        foreign = self.native_type("DifferentActualPolicy")()
        before = vars(foreign)
        with self.assertRaisesRegex(TypeError, "incompatible"):
            foreign.__dict__ = shared
        self.assertIs(vars(foreign), before)
        self.assertIs(vars(first), shared)

    def test_setdefault_keeps_unused_default_and_update_keeps_valid_prefix(self):
        instance = self.native_type()()
        dictionary = vars(instance)
        dictionary["field"] = 61
        wrong = object()
        self.assertEqual(dictionary.setdefault("field", wrong), 61)
        dictionary.clear()
        with self.assertRaisesRegex(TypeError, "exact int"):
            dictionary.setdefault("field", wrong)
        self.assertEqual(dictionary, {})
        with self.assertRaisesRegex(TypeError, "exact int"):
            dictionary.update([("extra", 67), ("field", "wrong"), ("later", 71)])
        self.assertEqual(dictionary, {"extra": 67})
        with self.assertRaisesRegex(TypeError, "exact int"):
            dictionary |= {"field": "wrong"}
        self.assertEqual(dictionary, {"extra": 67})

    def test_equality_is_not_replayed_after_resolving_a_stored_alias(self):
        for operation in ("direct", "setdefault", "update", "attribute"):
            events = []

            class Alias:
                def __hash__(self):
                    return hash("field")

                def __eq__(self, other):
                    events.append(other)
                    return other == "field"

            instance = self.native_type()()
            dictionary = vars(instance)
            key = Alias()
            dictionary[key] = "raw mapping alias"
            events.clear()
            if operation == "direct":
                dictionary["field"] = "another raw alias value"
            elif operation == "setdefault":
                self.assertEqual(dictionary.setdefault("field", "unused"), "raw mapping alias")
            elif operation == "update":
                dictionary.update({"field": "another raw alias value"})
            else:
                with self.assertRaisesRegex(TypeError, "exact int"):
                    instance.field = "wrong attribute value"
            self.assertEqual(events, ["field"], operation)
            self.assertIs(next(iter(dictionary)), key)
            self.assertEqual(len(dictionary), 1)

    def test_explicit_clear_has_ordinary_split_finalizer_visibility_and_counts(self):
        import sys

        def observe(kind):
            instance = kind()
            dictionary = vars(instance)
            events = []

            class Value:
                def __init__(self, name):
                    self.name = name

                def __del__(self):
                    peer = "right" if self.name == "left" else "left"
                    events.append((
                        self.name, "left" in dictionary, "right" in dictionary,
                        sys.getrefcount(dictionary[peer]) if peer in dictionary else None,
                    ))

            dictionary["left"] = Value("left")
            dictionary["right"] = Value("right")
            self.assertTrue(_testinternalcapi.has_split_table(dictionary))
            dictionary.clear()
            self.assertEqual(dictionary, {})
            self.assertEqual(sorted(event[0] for event in events), ["left", "right"])
            return events

        ordinary = type("Ordinary", (), {})
        self.assertEqual(observe(self.native_type()), observe(ordinary))


    def test_preparing_materialization_refuses_reentrant_mutations_but_preserves_reads(self):
        import gc
        import sys

        instance = self.native_type()()
        payload = object()
        instance.field = 79
        instance.extra = payload
        before = sys.getrefcount(payload)
        observations = []

        def during_prepare():
            observations.append(_testinternalcapi.dict_ordinary_inline_state(instance))
            self.assertEqual(instance.field, 79)
            self.assertIs(instance.extra, payload)
            self.assertIn(payload, gc.get_referents(instance))
            for action in (
                lambda: setattr(instance, "field", 83),
                lambda: setattr(instance, "new", 89),
                lambda: delattr(instance, "extra"),
                lambda: vars(instance),
                lambda: setattr(instance, "__dict__", {"field": 97}),
                lambda: delattr(instance, "__dict__"),
                lambda: self.generic_setattr(instance, "field", 101),
            ):
                with self.assertRaisesRegex(RuntimeError, "preparation is busy"):
                    action()
                self.assertEqual(instance.field, 79)
                self.assertIs(instance.extra, payload)
            error = _testinternalcapi.dict_ordinary_clear_managed_probe(instance, None)
            self.assertIsInstance(error, RuntimeError)
            marker = LookupError("existing primary")
            self.assertIs(
                _testinternalcapi.dict_ordinary_clear_managed_probe(instance, marker),
                marker,
            )
            self.assertEqual(
                _testinternalcapi.dict_ordinary_inline_state(instance), observations[0]
            )

        _testinternalcapi.dict_arm_soac_ordinary_hook(type(instance), during_prepare)
        dictionary = vars(instance)
        self.assertEqual(observations, [(2, False)])
        self.assertIs(vars(instance), dictionary)
        self.assertEqual(dictionary, {"field": 79, "extra": payload})
        self.assertEqual(self.has_policy(dictionary), 1)
        self.assertEqual(sys.getrefcount(payload), before)
        instance.field = 103
        self.assertEqual(instance.field, 103)
        # The transient refusal does not suppress ordinary explicit cleanup.
        self.assertIsNone(
            _testinternalcapi.dict_ordinary_clear_managed_probe(instance, None)
        )
        self.assertEqual(vars(instance), {})
        self.assertIs(dictionary["extra"], payload)

    def test_preparing_callback_error_restores_exact_inline_state_and_primary(self):
        import sys

        instance = self.native_type()()
        payload = object()
        instance.field = 107
        instance.extra = payload
        before = sys.getrefcount(payload)
        marker = LookupError("factory marker")
        context = ValueError("factory context")

        def fail():
            self.assertEqual(_testinternalcapi.dict_ordinary_inline_state(instance), (2, False))
            try:
                raise context
            except ValueError:
                raise marker

        _testinternalcapi.dict_arm_soac_ordinary_hook(type(instance), fail)
        with self.assertRaises(LookupError) as raised:
            vars(instance)
        self.assertIs(raised.exception, marker)
        self.assertIs(marker.__context__, context)
        self.assertEqual(_testinternalcapi.dict_ordinary_inline_state(instance), (1, False))
        self.assertEqual(instance.field, 107)
        self.assertIs(instance.extra, payload)
        self.assertEqual(sys.getrefcount(payload), before)
        dictionary = vars(instance)
        self.assertEqual(dictionary, {"field": 107, "extra": payload})
        self.assertEqual(self.has_policy(dictionary), 1)

    def test_actual_detach_allocation_failure_aborts_new_candidate_policy(self):
        import sys

        ordinary_error = None
        for protected in (False, True):
            with self.subTest(protected=protected):
                kind = self.native_type() if protected else type("OrdinaryDetach", (), {})
                instance = kind()
                payload = object()
                instance.field = 109
                instance.extra = payload
                previous = vars(instance)
                self.assertTrue(_testinternalcapi.has_split_table(previous))
                def payload_refcount():
                    # Both observations execute this same LOAD_DEREF body;
                    # no caller supplies an owning/borrowed payload argument.
                    return sys.getrefcount(payload)

                before = payload_refcount()
                incoming = {"field": 113}
                context = LookupError("detach context")
                try:
                    raise context
                except LookupError:
                    with self.assertRaises(MemoryError) as raised:
                        _testinternalcapi.dict_ordinary_replace_detach_oom(instance, incoming)
                    self.assertIs(sys.exception(), context)
                if not protected:
                    # Native PyErr_NoMemory takes its preallocated instance
                    # directly through SetRaisedException, without implicit
                    # handled-exception chaining. Use this actual unprotected
                    # detach allocation as the oracle for the policy path.
                    ordinary_error = raised.exception
                    self.assertIsNone(ordinary_error.__context__)
                else:
                    self.assertIs(type(raised.exception), type(ordinary_error))
                    self.assertEqual(raised.exception.args, ordinary_error.args)
                    self.assertIs(raised.exception.__context__, ordinary_error.__context__)
                    self.assertIs(raised.exception.__cause__, ordinary_error.__cause__)
                    self.assertIs(
                        raised.exception.__suppress_context__,
                        ordinary_error.__suppress_context__,
                    )
                self.assertIs(vars(instance), previous)
                self.assertEqual(_testinternalcapi.dict_ordinary_inline_state(instance), (1, True))
                self.assertEqual(instance.field, 109)
                self.assertIs(instance.extra, payload)
                after = payload_refcount()
                self.assertEqual(after, before)
                self.assertEqual(incoming, {"field": 113})
                self.assertEqual(self.has_policy(incoming), 0)
                incoming["field"] = "ordinary after aborted attachment"
                incoming["field"] = 127
                instance.__dict__ = incoming
                self.assertIs(vars(instance), incoming)
                self.assertEqual(instance.field, 127)
                self.assertIs(previous["extra"], payload)
                self.assertEqual(self.has_policy(incoming), int(protected))

    def test_popitem_uses_cached_hash_and_preserves_split_conversion(self):
        for protected in (False, True):
            with self.subTest(protected=protected):
                instance = self.receiver(protected)
                dictionary = vars(instance)
                dictionary["field"] = 131
                dictionary["extra"] = 137
                self.assertTrue(_testinternalcapi.has_split_table(dictionary))
                self.assertEqual(dictionary.popitem(), ("extra", 137))
                self.assertFalse(_testinternalcapi.has_split_table(dictionary))
                events = []

                class Key:
                    fail = False

                    def __hash__(self):
                        events.append("hash")
                        if self.fail:
                            raise AssertionError("popitem must not rehash its stored key")
                        return 139

                key = Key()
                dictionary[key] = 149
                events.clear()
                key.fail = True
                actual_key, value = dictionary.popitem()
                self.assertIs(actual_key, key)
                self.assertEqual(value, 149)
                self.assertEqual(events, [])


    def test_split_setdefault_watcher_keeps_ordinary_borrowed_input_counts(self):
        import sys

        watcher_type = ctypes.PYFUNCTYPE(
            ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p
        )
        add = ctypes.pythonapi.PyDict_AddWatcher
        add.argtypes, add.restype = [watcher_type], ctypes.c_int
        watch = ctypes.pythonapi.PyDict_Watch
        watch.argtypes, watch.restype = [ctypes.c_int, ctypes.py_object], ctypes.c_int
        unwatch = ctypes.pythonapi.PyDict_Unwatch
        unwatch.argtypes, unwatch.restype = [ctypes.c_int, ctypes.py_object], ctypes.c_int
        clear = ctypes.pythonapi.PyDict_ClearWatcher
        clear.argtypes, clear.restype = [ctypes.c_int], ctypes.c_int

        def observe(kind):
            instance = kind()
            dictionary = vars(instance)
            self.assertTrue(_testinternalcapi.has_split_table(dictionary))
            key = "untyped_" + str(id(instance))
            value = object()
            observations = []

            @watcher_type
            def callback(event, actual_dict, actual_key, actual_value):
                observations.append((
                    actual_dict == id(dictionary),
                    actual_key == id(key),
                    actual_value == id(value),
                    sys.getrefcount(key),
                    sys.getrefcount(value),
                ))
                return 0

            watcher = add(callback)
            self.assertGreaterEqual(watcher, 0)
            try:
                watch(watcher, dictionary)
                self.assertIs(dictionary.setdefault(key, value), value)
            finally:
                unwatch(watcher, dictionary)
                clear(watcher)
            self.assertEqual(len(observations), 1)
            self.assertEqual(observations[0][:3], (True, True, True))
            return observations[0][3:]

        self.assertEqual(observe(self.native_type()), observe(type("OrdinaryCounts", (), {})))

class TypeStateStrictFieldNativeTests(unittest.TestCase):
    @staticmethod
    def native_type(name="TypeState", *, members=None, direct=True):
        if not direct:
            return OrdinaryStrictFieldNativeTests.native_type(name, members=members)

        def namespace_function():
            pass

        return _testinternalcapi.dict_new_soac_type_state_type(
            name, (), {"__module__": __name__, **(members or {})},
            ("field",), namespace_function,
        )

    @classmethod
    def setUpClass(cls):
        getter = ctypes.pythonapi.PySoac_GetStrictRuntimeUnavailableError
        getter.argtypes = []
        getter.restype = ctypes.c_void_p  # interpreter-owned borrowed type
        cls.unavailable_error = ctypes.cast(getter(), ctypes.py_object).value

    def assert_checked(self, receiver):
        receiver.field = 23
        self.assertEqual(receiver.field, 23)
        with self.assertRaisesRegex(TypeError, "exact int"):
            receiver.field = "bad"
        self.assertEqual(receiver.field, 23)

    def assert_dictionary_checked(self, dictionary):
        dictionary["field"] = 31
        with self.assertRaisesRegex(TypeError, "exact int"):
            dictionary["field"] = "bad"
        self.assertEqual(dictionary["field"], 31)

    def test_ordinary_storage_and_legacy_contract_do_not_claim_a_tail(self):
        class Ordinary:
            pass

        legacy = self.native_type(direct=False)()
        for value in ({}, Ordinary(), vars(Ordinary())):
            with self.subTest(kind=type(value)):
                info = _testinternalcapi.get_soac_type_state_info(value)
                self.assertFalse(info["has_slot"])
                self.assertEqual(info["extra_slot_bytes"], 0)
                self.assertIsNone(info["tail_offset"])
                self.assertIsNone(info["state_id"])
                self.assertEqual(info["storage_mode"], "ordinary")
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(legacy)["storage_mode"], "legacy")
        self.assertFalse(_testinternalcapi.get_soac_type_state_info(legacy)["has_slot"])
        self.assert_checked(legacy)
        self.assert_dictionary_checked(vars(legacy))

    def test_fresh_instances_and_dictionaries_share_only_resolved_projections(self):
        kind = self.native_type()
        first, second = kind(), kind()
        first_info = _testinternalcapi.get_soac_type_state_info(first)
        second_info = _testinternalcapi.get_soac_type_state_info(second)
        self.assertTrue(first_info["has_slot"])
        self.assertEqual(first_info["extra_slot_bytes"], struct.calcsize("P"))
        self.assertEqual(first_info["storage_mode"], "direct")
        self.assertEqual(first_info["state_id"], second_info["state_id"])
        self.assertNotEqual(first_info["state_id"], first_info["dictionary_state_id"])
        for receiver in (first, second):
            dictionary = vars(receiver)
            info = _testinternalcapi.get_soac_type_state_info(dictionary)
            self.assertIs(type(dictionary), dict)
            self.assertIs(type(receiver), kind)
            self.assertEqual(info["state_id"], first_info["dictionary_state_id"])
            self.assertEqual(info["extra_slot_bytes"], struct.calcsize("P"))
            self.assert_checked(receiver)

    def test_actual_exact_dict_allocation_request_is_one_pointer_larger(self):
        receiver = self.native_type()()
        gc.collect()  # empty the ordinary dictionary freelist before capture
        ordinary, ordinary_bytes = _testinternalcapi.capture_soac_type_state_allocation(dict)
        extended, extended_bytes = _testinternalcapi.capture_soac_type_state_allocation(lambda: vars(receiver))
        self.assertIs(type(ordinary), type(extended))
        self.assertIs(type(extended), dict)
        self.assertIsInstance(ordinary_bytes, int)
        self.assertIsInstance(extended_bytes, int)
        self.assertEqual(extended_bytes, ordinary_bytes + struct.calcsize("P"))
        self.assertFalse(_testinternalcapi.get_soac_type_state_info(ordinary)["has_slot"])
        self.assertTrue(_testinternalcapi.get_soac_type_state_info(extended)["has_slot"])

    def test_actual_default_instance_allocation_preserves_ordinary_layout_size(self):
        kind = self.native_type()
        ordinary = type("Ordinary", (), {"__module__": __name__})
        # Cached-key capacity shrinks during early allocations in both ordinary
        # and participating types. Compare after both reach the same capacity.
        warm_direct = [kind() for _ in range(40)]
        warm_ordinary = [ordinary() for _ in range(40)]
        self.assertEqual(kind.__basicsize__, ordinary.__basicsize__)
        checked, checked_bytes = _testinternalcapi.capture_soac_type_state_allocation(kind)
        control, control_bytes = _testinternalcapi.capture_soac_type_state_allocation(ordinary)
        self.assertIsInstance(checked_bytes, int)
        self.assertIsInstance(control_bytes, int)
        self.assertEqual(checked_bytes, control_bytes + struct.calcsize("P"))
        self.assertTrue(_testinternalcapi.get_soac_type_state_info(checked)["has_slot"])
        self.assertFalse(_testinternalcapi.get_soac_type_state_info(control)["has_slot"])
        self.assertEqual(len(warm_direct), len(warm_ordinary))

    def test_warmed_inline_dict_and_supported_c_api_writes_keep_checks(self):
        kind = self.native_type()
        receiver = kind()

        def write(value):
            receiver.field = value

        for value in range(1000):
            write(value)
        with self.assertRaisesRegex(TypeError, "exact int"):
            write("bad inline")
        dictionary = vars(receiver)
        for value in range(1000):
            write(value)
        with self.assertRaisesRegex(TypeError, "exact int"):
            write("bad materialized")
        set_item = ctypes.pythonapi.PyDict_SetItem
        set_item.argtypes = [ctypes.py_object] * 3
        set_item.restype = ctypes.c_int
        set_attr = ctypes.pythonapi.PyObject_GenericSetAttr
        set_attr.argtypes = [ctypes.py_object] * 3
        set_attr.restype = ctypes.c_int
        for function, target in ((set_item, dictionary), (set_attr, receiver)):
            self.assertEqual(function(target, "field", 37), 0)
            with self.assertRaisesRegex(TypeError, "exact int"):
                function(target, "field", "bad C write")
            self.assertEqual(receiver.field, 37)

    def test_normal_dict_clear_does_not_retire_shared_state(self):
        kind = self.native_type()
        first, second = kind(), kind()
        dictionary, sibling = vars(first), vars(second)
        before = _testinternalcapi.get_soac_type_state_info(dictionary)
        dictionary["field"] = 1
        dictionary.clear()
        self.assertEqual(dictionary, {})
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(dictionary), before)
        self.assert_dictionary_checked(dictionary)
        self.assert_dictionary_checked(sibling)

    def test_direct_stores_do_not_discover_legacy_rules_or_dictionary_identity(self):
        for direct in (False, True):
            base = self.native_type(direct=direct)

            class Child(base):
                pass

            receiver = Child()

            def write_attributes():
                for value in range(1000):
                    receiver.field = value
                return receiver.field

            with self.subTest(direct=direct, storage="inline"):
                result, counts = _testinternalcapi.probe_soac_type_state_lookups(Child, None, write_attributes)
                self.assertEqual(result, 999)
                if direct:
                    self.assertEqual(counts, {
                        "ordinary_type_lookups": 0, "slot_type_lookups": 0,
                        "dictionary_identity_lookups": 0,
                    })
                else:
                    self.assertGreater(counts["ordinary_type_lookups"], 0)
            dictionary = vars(receiver)

            def write_mapping():
                for value in range(1000):
                    dictionary["field"] = value
                    receiver.field = value
                return receiver.field

            with self.subTest(direct=direct, storage="dictionary"):
                result, counts = _testinternalcapi.probe_soac_type_state_lookups(Child, dictionary, write_mapping)
                self.assertEqual(result, 999)
                if direct:
                    self.assertEqual(counts, {
                        "ordinary_type_lookups": 0, "slot_type_lookups": 0,
                        "dictionary_identity_lookups": 0,
                    })
                else:
                    self.assertGreater(counts["dictionary_identity_lookups"], 0)
                self.assert_checked(receiver)

    def test_lookup_probe_unarms_on_error_and_rejects_nested_scope(self):
        kind = self.native_type()
        receiver = kind()
        primary = ValueError("original probe failure")

        def fail():
            raise primary

        with self.assertRaises(ValueError) as caught:
            _testinternalcapi.probe_soac_type_state_lookups(kind, None, fail)
        self.assertIs(caught.exception, primary)

        def nested():
            with self.assertRaisesRegex(RuntimeError, "probe is active"):
                _testinternalcapi.probe_soac_type_state_lookups(kind, None, lambda: None)
            receiver.field = 17
            return receiver

        result, counts = _testinternalcapi.probe_soac_type_state_lookups(kind, None, nested)
        self.assertIs(result, receiver)
        self.assertEqual(counts, {
            "ordinary_type_lookups": 0, "slot_type_lookups": 0,
            "dictionary_identity_lookups": 0,
        })
        self.assert_checked(receiver)

    def test_private_dictionary_callback_cannot_escape_to_another_receiver(self):
        class Name(str):
            pass

        class Ordinary:
            pass

        for ordinary_recipient in (False, True):
            for abort in (False, True):
                with self.subTest(ordinary_recipient=ordinary_recipient, abort=abort):
                    kind = self.native_type()
                    receiver, sibling = kind(), kind()
                    recipient = Ordinary() if ordinary_recipient else kind()
                    original = vars(recipient)
                    sibling_dict = vars(sibling)
                    shared = _testinternalcapi.get_soac_type_state_info(sibling_dict)["state_id"]
                    primary = ValueError("abort private write") if abort else None
                    result = _testinternalcapi.probe_soac_type_state_private_escape(
                        receiver, recipient, sibling_dict, Name("field"), 67, primary,
                    )
                    self.assertIs(result, primary)
                    self.assertIs(vars(recipient), original)
                    self.assertEqual(original, {})
                    self.assertEqual(sibling_dict["field"], 67)
                    self.assertEqual(_testinternalcapi.get_soac_type_state_info(sibling_dict)["state_id"], shared)
                    self.assert_dictionary_checked(sibling_dict)
                    if abort:
                        self.assertEqual(_testinternalcapi.dict_ordinary_inline_state(receiver), (1, False))
                        with self.assertRaises(AttributeError):
                            _ = receiver.field
                    else:
                        self.assertEqual(receiver.field, 67)
                    self.assert_checked(receiver)
                    self.assert_dictionary_checked(vars(receiver))

    def test_string_subclass_stored_keys_remain_checked(self):
        class Name(str):
            def __str__(self):
                raise AssertionError("field validation must inspect Unicode data")

        for direct in (False, True):
            with self.subTest(direct=direct):
                receiver = self.native_type(direct=direct)()
                name = Name("field")
                setattr(receiver, name, 67)
                dictionary = vars(receiver)
                self.assertIs(next(iter(dictionary)), name)
                with self.assertRaisesRegex(TypeError, "exact int"):
                    dictionary["field"] = "bad"
                self.assertEqual(receiver.field, 67)
                self.assert_dictionary_checked(dictionary)
                self.assertIs(next(iter(dictionary)), name)

    def test_generic_allocation_preserves_pending_error_and_distinguishes_new_failure(self):
        for family in ("ordinary", "legacy", "custom", "direct_cold", "direct_warm"):
            for fail_at in (0, 1):
                with self.subTest(family=family, fail_at=fail_at):
                    if family == "ordinary":
                        kind = type("Ordinary", (), {})
                    elif family == "custom":
                        base = self.native_type(direct=False)
                        kind = type("Custom", (base,), {"__new__": lambda cls: object.__new__(cls)})
                    else:
                        kind = self.native_type(direct=family != "legacy")
                    if family == "direct_warm":
                        kind()
                    primary = ValueError("preserved allocation primary")
                    result, error, requests = _testinternalcapi.soac_type_state_alloc_pending_error(kind, primary, fail_at)
                    self.assertGreater(requests, 0)
                    if fail_at:
                        self.assertIsNone(result)
                        self.assertIsInstance(error, MemoryError)
                        self.assertIsNot(error, primary)
                    else:
                        self.assertIs(type(result), kind)
                        self.assertIs(error, primary)
                        self.assertEqual(_testinternalcapi.get_soac_type_state_info(result)["has_slot"],
                                         family in ("direct_cold", "direct_warm"))
                        if family != "ordinary":
                            self.assert_checked(result)

    def test_tp_clear_retires_only_one_dictionary_attachment(self):
        kind = self.native_type()
        first, second = kind(), kind()
        dictionary, sibling = vars(first), vars(second)
        shared = _testinternalcapi.get_soac_type_state_info(sibling)["state_id"]
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(dictionary)["state_id"], shared)
        dictionary["cycle"] = dictionary
        _testinternalcapi.soac_type_state_clear(dictionary)  # the actual dict tp_clear
        self.assertEqual(dictionary, {})
        retired = _testinternalcapi.get_soac_type_state_info(dictionary)
        self.assertTrue(retired["has_slot"])
        self.assertTrue(retired["terminal"])
        self.assertIsNone(retired["state_id"])
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(sibling)["state_id"], shared)
        self.assert_dictionary_checked(sibling)
        self.assert_checked(second)
        with self.assertRaises(self.unavailable_error):
            dictionary["field"] = 41

    def test_instance_tp_clear_keeps_escaped_dict_and_sibling_live(self):
        kind = self.native_type()
        first, second = kind(), kind()
        escaped = vars(first)
        before = _testinternalcapi.get_soac_type_state_info(escaped)["state_id"]
        first.field = 43
        _testinternalcapi.soac_type_state_clear(first)  # defined instance terminal path
        retired = _testinternalcapi.get_soac_type_state_info(first)
        self.assertTrue(retired["has_slot"])
        self.assertTrue(retired["terminal"])
        self.assertIsNone(retired["state_id"])
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(escaped)["state_id"], before)
        self.assert_dictionary_checked(escaped)
        self.assert_checked(second)
        with self.assertRaises(self.unavailable_error):
            first.field = 47

    def test_own_finalizer_resurrection_keeps_the_original_state(self):
        saved, finalized = [], []

        def finalize(self):
            finalized.append("called")
            saved.append(self)

        kind = self.native_type(members={"__del__": finalize})
        receiver = kind()
        receiver.field = 53
        before = _testinternalcapi.get_soac_type_state_info(receiver)
        del receiver
        gc.collect()
        self.assertEqual(finalized, ["called"])
        receiver = saved.pop()
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(receiver), before)
        self.assert_checked(receiver)
        self.assert_dictionary_checked(vars(receiver))
        reference = weakref.ref(receiver)
        del receiver
        gc.collect()
        self.assertIsNone(reference())
        self.assertEqual(finalized, ["called"])

    def test_cycle_collection_does_not_clear_a_live_siblings_shared_rules(self):
        finalized = []
        kind = self.native_type(members={"__del__": lambda self: finalized.append("called")})
        sibling = kind()
        receiver = kind()
        receiver.cycle = receiver
        receiver.dictionary = vars(receiver)
        before = _testinternalcapi.get_soac_type_state_info(sibling)["state_id"]
        reference = weakref.ref(receiver)
        del receiver
        gc.collect()
        self.assertIsNone(reference())
        self.assertEqual(finalized, ["called"])
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(sibling)["state_id"], before)
        self.assert_checked(sibling)

    def test_escaped_dict_retains_rules_without_retaining_receiver_or_type(self):
        kind = self.native_type()
        receiver = kind()
        escaped = vars(receiver)
        instance_reference, type_reference = weakref.ref(receiver), weakref.ref(kind)
        before = _testinternalcapi.get_soac_type_state_info(escaped)["state_id"]
        del receiver, kind
        gc.collect()
        self.assertIsNone(instance_reference())
        self.assertIsNone(type_reference())
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(escaped)["state_id"], before)
        self.assert_dictionary_checked(escaped)

    def test_existing_replacement_dictionary_keeps_identity_and_legacy_allocation(self):
        receiver = self.native_type()()
        old = vars(receiver)
        incoming = {"field": 59}
        receiver.__dict__ = incoming
        self.assertIs(vars(receiver), incoming)
        info = _testinternalcapi.get_soac_type_state_info(incoming)
        self.assertFalse(info["has_slot"])
        self.assertEqual(info["storage_mode"], "legacy")
        self.assert_dictionary_checked(incoming)
        self.assert_dictionary_checked(old)
        self.assert_checked(receiver)

    def test_custom_new_descendant_remains_explicitly_legacy_and_enforced(self):
        base = self.native_type()

        class Custom(base):
            def __new__(cls):
                return object.__new__(cls)

        receiver = Custom()
        info = _testinternalcapi.get_soac_type_state_info(receiver)
        self.assertFalse(info["has_slot"])
        self.assertEqual(info["storage_mode"], "legacy")
        self.assert_checked(receiver)
        self.assert_dictionary_checked(vars(receiver))

    def test_ordinary_descendant_type_mutation_refreshes_only_future_cache(self):
        base = self.native_type()

        class Child(base):
            pass

        old = Child()
        old_info = _testinternalcapi.get_soac_type_state_info(old)
        Child.unrelated = object()
        new = Child()
        self.assertNotEqual(_testinternalcapi.get_soac_type_state_info(new)["state_id"], old_info["state_id"])
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(old), old_info)
        self.assert_checked(old)
        self.assert_checked(new)

    def test_create_reftracer_sees_complete_instance_and_dictionary(self):
        kind = self.native_type()

        def allocate():
            receiver = kind()
            return receiver, vars(receiver)

        result, created, _destroyed = _testinternalcapi.check_soac_type_state_reftracer(allocate)
        self.assertGreaterEqual(created, 2)
        receiver, dictionary = result
        self.assertIs(vars(receiver), dictionary)
        self.assert_checked(receiver)

    def test_member_api_rejects_layout_metadata_before_conversion(self):
        receiver = self.native_type()()
        receiver.field = 71
        before = _testinternalcapi.get_soac_type_state_info(receiver)
        inline_before = _testinternalcapi.dict_ordinary_inline_state(receiver)
        conversions = []

        class Index:
            def __index__(self):
                conversions.append("converted")
                return 0

        for target in ("flags", "inline_extent"):
            with self.subTest(target=target):
                with self.assertRaisesRegex(TypeError, "incompatible with protected storage"):
                    _testinternalcapi.soac_type_state_write_member(receiver, target, Index())
                self.assertEqual(conversions, [])
                self.assertEqual(_testinternalcapi.get_soac_type_state_info(receiver), before)
                self.assertEqual(_testinternalcapi.dict_ordinary_inline_state(receiver), inline_before)
                self.assertEqual(receiver.field, 71)
        ordinary_member = self.native_type(members={"__slots__": ("ordinary_slot", "__dict__")})()
        value = object()
        member_before = _testinternalcapi.get_soac_type_state_info(ordinary_member)
        _testinternalcapi.soac_type_state_write_member(ordinary_member, "ordinary_slot", value)
        self.assertIs(ordinary_member.ordinary_slot, value)
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(ordinary_member), member_before)
        self.assert_checked(ordinary_member)

    def test_populated_native_slot_state_checks_actual_rows_and_avoids_legacy_lookup(self):
        def build(corruption=0):
            def namespace_function():
                pass

            return _testinternalcapi.slot_new_soac_type_state_type(
                "ActualSlotState", (),
                {"__module__": __name__, "__slots__": ("left", "right")},
                ("left", "right"), namespace_function, corruption,
            )

        kind = build()
        receiver = kind()
        info = _testinternalcapi.get_soac_type_state_info(receiver)
        self.assertTrue(info["has_slot"])
        self.assertEqual(info["native_slot_count"], 2)
        self.assertIsNone(info["dictionary_state_id"])

        def stores():
            for value in range(1000):
                receiver.left = value
                _testinternalcapi.soac_type_state_write_member(receiver, "right", value)
            return receiver.left, receiver.right

        result, counts = _testinternalcapi.probe_soac_type_state_lookups(kind, None, stores)
        self.assertEqual(result, (999, 999))
        self.assertEqual(counts, {
            "ordinary_type_lookups": 0, "slot_type_lookups": 0,
            "dictionary_identity_lookups": 0,
        })
        with self.assertRaisesRegex(TypeError, "exact int"):
            receiver.left = "bad"
        with self.assertRaisesRegex(TypeError, "exact int"):
            _testinternalcapi.soac_type_state_write_member(receiver, "right", "bad")
        conversions = []

        class Index:
            def __index__(self):
                conversions.append("converted")
                return 0

        with self.assertRaisesRegex(TypeError, "incompatible with protected storage"):
            _testinternalcapi.soac_type_state_write_member(receiver, "slot_as_integer", Index())
        self.assertEqual(conversions, [])
        self.assertEqual((receiver.left, receiver.right), (999, 999))
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(receiver), info)
        del receiver.left
        with self.assertRaises(AttributeError):
            _ = receiver.left
        receiver.left = 73
        self.assertEqual(receiver.left, 73)
        for corruption in (1, 2, 3):
            with self.subTest(corruption=corruption):
                invalid = build(corruption)
                with self.assertRaisesRegex((TypeError, RuntimeError), "storage-state"):
                    invalid()

    def test_oom_does_not_publish_failed_instance_or_clear_cached_rules(self):
        finalized = []
        kind = self.native_type(members={"__del__": lambda self: finalized.append("called")})
        warm = kind()
        before = _testinternalcapi.get_soac_type_state_info(warm)
        with self.assertRaises(MemoryError):
            _testinternalcapi.capture_soac_type_state_allocation(kind, 1)
        self.assertEqual(finalized, [])
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(warm), before)
        self.assert_checked(warm)
        following = kind()
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(following)["state_id"], before["state_id"])

        # The global native state type is now ready, but this separate class
        # has never prepared its own cold profile/cache. Fail its first actual
        # metadata allocation, then prove recovery and eventual collection.
        cold = self.native_type("ColdState")
        cold_reference = weakref.ref(cold)
        with self.assertRaises(MemoryError):
            _testinternalcapi.capture_soac_type_state_allocation(cold, 1)
        recovered = cold()
        self.assertTrue(_testinternalcapi.get_soac_type_state_info(recovered)["has_slot"])
        self.assert_checked(recovered)
        del recovered, cold
        gc.collect()
        self.assertIsNone(cold_reference())

        # No dictionary header has escaped from warm: this fault is its first
        # fresh exact-dict materialization, while the authoritative value stays
        # in the unchanged inline array. It must not leave a preparing marker.
        inline_before = _testinternalcapi.dict_ordinary_inline_state(warm)
        self.assertEqual(inline_before, (1, False))
        with self.assertRaises(MemoryError):
            _testinternalcapi.capture_soac_type_state_allocation(lambda: vars(warm), 1)
        self.assertEqual(_testinternalcapi.dict_ordinary_inline_state(warm), inline_before)
        self.assertEqual(_testinternalcapi.get_soac_type_state_info(warm), before)
        self.assertEqual(warm.field, 23)
        materialized = vars(warm)
        self.assertEqual(materialized["field"], 23)
        self.assertTrue(_testinternalcapi.get_soac_type_state_info(materialized)["has_slot"])
        self.assert_dictionary_checked(materialized)

    def test_extended_dict_never_enters_the_ordinary_freelist(self):
        kind = self.native_type()
        enabled = gc.isenabled()
        gc.disable()
        try:
            for cycle in range(32):
                with self.subTest(cycle=cycle):
                    receiver = kind()
                    dictionary = vars(receiver)
                    self.assertTrue(_testinternalcapi.get_soac_type_state_info(dictionary)["has_slot"])
                    self.assert_checked(receiver)
                    del receiver
                    gc.collect()
                    self.assert_dictionary_checked(dictionary)

                    # Drain after all helpers: their result dictionaries can
                    # replenish the ordinary freelist. No dictionary-producing
                    # helper may run between releasing the extended allocation
                    # and observing the next ordinary allocation request.
                    held = [{} for _ in range(512)]
                    del dictionary
                    ordinary, requested = _testinternalcapi.capture_soac_type_state_allocation(dict)
                    self.assertIsInstance(requested, int)
                    self.assertFalse(_testinternalcapi.get_soac_type_state_info(ordinary)["has_slot"])
                    self.assertEqual(len(held), 512)
                    # The next cycle must also work after ordinary frees and
                    # reuse interleave with fresh extended allocations.
                    del ordinary, held
        finally:
            if enabled:
                gc.enable()

    @unittest.skipUnless(hasattr(sys, "gettotalrefcount"), "requires the native Py_REF_DEBUG diagnostic")
    def test_debug_zero_refcount_diagnostic_preserves_layout_marker(self):
        code = """\
import _testinternalcapi
import resource
resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
def namespace_function():
    pass
kind = _testinternalcapi.dict_new_soac_type_state_type("Marked", (), {}, ("field",), namespace_function)
receiver = kind()
assert _testinternalcapi.get_soac_type_state_info(receiver)["has_slot"]
print("actual-stateful-allocation", flush=True)
_testinternalcapi.soac_type_state_negative_refcount(receiver)
"""
        result = subprocess.run(
            [sys.executable, "-I", "-S", "-B", "-c", code],
            capture_output=True, text=True, timeout=30, check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("actual-stateful-allocation", result.stdout)
        self.assertIn("object has negative ref count", result.stderr)
        self.assertNotIn("zero-refcount decrement was not diagnosed", result.stderr)


if __name__ == "__main__":
    unittest.main()
