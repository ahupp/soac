"""Native writes must observe policies installed by their lookup callbacks."""

import ctypes
import unittest

from tests.test_strict_cpython_native import borrowed_object_api, native_api


class StrictDictionaryPolicyTransitionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.set_owner = native_api(
            "PyFunction_SetSoacStrictOwner",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
        )
        cls.seal = native_api(
            "PyFunction_SealSoacStrict",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.c_uint64,
        )
        cls.has_policy = native_api(
            "PyDict_HasSoacPolicy", ctypes.c_int, ctypes.py_object
        )
        cls.set_item = native_api(
            "PyDict_SetItem",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
            ctypes.py_object,
        )
        cls.mutation_error = borrowed_object_api("PySoac_GetStrictMutationError")

    def exercise(self, operation, trigger, *, protected, present):
        events = []
        armed = False
        sealed = False

        def function(*, value=7):
            return value

        self.set_owner(function, object())

        class Key:
            def __init__(self, label):
                self.label = label

            def __hash__(key):
                observe("hash", key.label)
                return 41

            def __eq__(key, other):
                observe("equality", key.label, other.label)
                return present

        def observe(kind, *details):
            nonlocal sealed
            if not armed:
                return
            events.append((kind, *details))
            if protected and kind == trigger and not sealed:
                sealed = True
                self.assertEqual(self.seal(function, 22001), 0)

        existing = Key("existing")
        candidate = Key("candidate")
        namespace = {existing: 11}
        function.__kwdefaults__ = namespace
        # Prepare an exact source dictionary before arming its key callbacks.
        source = {candidate: 23}
        armed = True
        error = None
        try:
            if operation == "assignment":
                namespace[candidate] = 23
            elif operation == "c_api":
                self.set_item(namespace, candidate, 23)
            elif operation == "update_pairs":
                namespace.update([(candidate, 23)])
            elif operation == "update_dict":
                namespace.update(source)
            elif operation == "setdefault":
                namespace.setdefault(candidate, 23)
            elif operation == "delete":
                del namespace[candidate]
            elif operation == "pop":
                namespace.pop(candidate)
            else:
                raise AssertionError(operation)
        except self.mutation_error as caught:
            error = caught
        return namespace, events, sealed, error

    def test_lookup_callback_seal_prevents_the_pending_write(self):
        for operation in (
            "assignment",
            "c_api",
            "update_pairs",
            "update_dict",
            "setdefault",
            "delete",
            "pop",
        ):
            for trigger in ("hash", "equality"):
                if operation == "update_dict" and trigger == "hash":
                    # Exact-dict update reuses the source's already known hash.
                    continue
                present_values = (
                    (True,)
                    if operation in ("delete", "pop")
                    else (False,)
                    if operation == "setdefault"
                    else (False, True)
                )
                for present in present_values:
                    with self.subTest(
                        operation=operation, trigger=trigger, present=present
                    ):
                        _, expected_events, _, ordinary_error = self.exercise(
                            operation, trigger, protected=False, present=present
                        )
                        self.assertIsNone(ordinary_error)
                        namespace, events, sealed, error = self.exercise(
                            operation, trigger, protected=True, present=present
                        )
                        self.assertTrue(sealed)
                        self.assertEqual(self.has_policy(namespace), 1)
                        self.assertIsInstance(error, self.mutation_error)
                        self.assertEqual(list(namespace.values()), [11])
                        self.assertEqual(events, expected_events)

    def test_setdefault_hit_is_still_a_read_after_a_lookup_callback_seals(self):
        for trigger in ("hash", "equality"):
            with self.subTest(trigger=trigger):
                namespace, events, sealed, error = self.exercise(
                    "setdefault", trigger, protected=True, present=True
                )
                self.assertTrue(sealed)
                self.assertEqual(self.has_policy(namespace), 1)
                self.assertIsNone(error)
                self.assertEqual(list(namespace.values()), [11])
                self.assertEqual(
                    events,
                    [("hash", "candidate"), ("equality", "existing", "candidate")],
                )

    def test_watcher_seal_prevents_the_pending_write(self):
        watcher_type = ctypes.CFUNCTYPE(
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
        )
        add_watcher = native_api("PyDict_AddWatcher", ctypes.c_int, watcher_type)
        clear_watcher = native_api("PyDict_ClearWatcher", ctypes.c_int, ctypes.c_int)
        watch = native_api("PyDict_Watch", ctypes.c_int, ctypes.c_int, ctypes.py_object)
        unwatch = native_api(
            "PyDict_Unwatch", ctypes.c_int, ctypes.c_int, ctypes.py_object
        )
        for operation, empty in (
            ("assignment", True),
            ("assignment", False),
            ("setdefault", False),
            ("delete", False),
            ("pop", False),
            ("popitem", False),
            ("clear", False),
            ("update", True),
            ("attribute", False),
            ("attribute_delete", False),
        ):
            with self.subTest(operation=operation, empty=empty):

                def function(*, value=7):
                    return value

                self.set_owner(function, object())

                class Holder:
                    pass

                holder = Holder()
                if operation.startswith("attribute"):
                    holder.value = 11
                    namespace = vars(holder)
                else:
                    namespace = {} if empty else {"value": 11}
                function.__kwdefaults__ = namespace
                observed = []
                callback_errors = []

                @watcher_type
                def on_event(event, dictionary, key, value):
                    if dictionary == id(namespace) and not observed:
                        observed.append(event)
                        try:
                            self.seal(function, 22002)
                        except BaseException as error:
                            callback_errors.append(error)
                    return 0

                identity = add_watcher(on_event)
                self.assertGreaterEqual(identity, 0)
                watch(identity, namespace)
                error = None
                try:
                    if operation == "assignment":
                        namespace["value"] = 23
                    elif operation == "setdefault":
                        namespace.setdefault("new", 23)
                    elif operation == "delete":
                        del namespace["value"]
                    elif operation == "pop":
                        namespace.pop("value")
                    elif operation == "popitem":
                        namespace.popitem()
                    elif operation == "clear":
                        namespace.clear()
                    elif operation == "update":
                        namespace.update({"new": 23})
                    elif operation == "attribute":
                        holder.value = 23
                    elif operation == "attribute_delete":
                        del holder.value
                except self.mutation_error as caught:
                    error = caught
                finally:
                    unwatch(identity, namespace)
                    clear_watcher(identity)
                self.assertTrue(observed)
                self.assertEqual(callback_errors, [])
                self.assertEqual(self.has_policy(namespace), 1)
                self.assertIsInstance(error, self.mutation_error)
                self.assertEqual(list(namespace.values()), [] if empty else [11])

    def test_split_clear_finalizer_cannot_publish_a_policy_mid_mutation(self):
        def function(*, value=7):
            return value

        self.set_owner(function, object())
        events = []

        class Holder:
            pass

        class First:
            def __del__(first):
                try:
                    self.seal(function, 22003)
                except self.mutation_error:
                    events.append("declined")
                else:
                    events.append("sealed-before-remaining-clear")

        holder = Holder()
        holder.first = First()
        holder.value = 11
        namespace = vars(holder)
        function.__kwdefaults__ = namespace
        namespace.clear()
        self.assertEqual(events, ["declined"])
        self.assertEqual(namespace, {})
        self.assertEqual(self.has_policy(namespace), 0)
        # The unsuccessful transition did not consume an ordinary function's
        # future ability to freeze the now-consistent actual mapping.
        self.assertEqual(self.seal(function, 22003), 0)
        self.assertEqual(self.has_policy(namespace), 1)


if __name__ == "__main__":
    unittest.main()
