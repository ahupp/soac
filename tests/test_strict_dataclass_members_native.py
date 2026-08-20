"""Native member transactions, not signed stdlib adapter admission.

The shared C fixture supplies exact frame, creation, and member catalogs.
Production must obtain those witnesses from its verified stdlib invocation.
"""

import _testinternalcapi
import ctypes
import dis
import gc
import weakref
import types
import unittest

from tests.test_strict_cpython_native import borrowed_object_api, native_api
from tests.test_strict_dataclass_native import child_code, instruction
from tests.test_strict_type_native import TypeContractSpecV4
from tests.test_strict_type_native import ConstructionInfoV1, ConstructionSpec


def apply_member(actual, invocation, install):
    def generated(self, value):
        return value

    install(invocation, actual, generated)
    return actual


def apply_setattr(actual, invocation, install):
    def generated(self, name, value):
        raise AttributeError("generated frozen setter")

    install(invocation, actual, generated)
    return actual


def apply_delattr(actual, invocation, install):
    def generated(self, name):
        raise AttributeError("generated frozen deleter")

    install(invocation, actual, generated)
    return actual


class DataclassMemberNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.set_member = native_api(
            "PyType_SetSoacDataclassMember", ctypes.c_int, *([ctypes.py_object] * 4)
        )
        cls.bind = native_api(
            "PySoac_DataclassBindClass", ctypes.c_int, *([ctypes.py_object] * 3)
        )
        cls.complete = native_api(
            "PySoac_CompleteDataclassInvocation", ctypes.c_int, ctypes.py_object
        )
        cls.fail = native_api(
            "PySoac_FailDataclassInvocation", ctypes.c_int, ctypes.py_object
        )
        cls.new_handle = native_api(
            "PyType_NewSoacConstructionHandle",
            ctypes.py_object,
            ctypes.POINTER(ConstructionSpec),
        )
        cls.construct = native_api(
            "PyType_FromSoacConstructionHandle",
            ctypes.py_object,
            ctypes.py_object,
            ctypes.py_object,
        )
        cls.seal_class = native_api(
            "PyType_SealSoacContract", ctypes.c_int, ctypes.py_object, ctypes.py_object
        )
        cls.has_class = native_api(
            "PyType_HasSoacContract", ctypes.c_int, ctypes.py_object
        )
        cls.get_dict = native_api("PyType_GetDict", ctypes.py_object, ctypes.py_object)
        cls.strict_id = native_api(
            "PyFunction_GetSoacStrictId", ctypes.c_uint64, ctypes.py_object
        )
        cls.has_creation = native_api(
            "PyFunction_HasSoacDataclassCreation", ctypes.c_int, ctypes.py_object
        )
        cls.metadata = native_api(
            "PyFunction_GetSoacMetadata", ctypes.c_void_p, ctypes.py_object
        )
        cls.mutation_error = borrowed_object_api("PySoac_GetStrictMutationError")
        cls.unavailable_error = borrowed_object_api(
            "PySoac_GetStrictRuntimeUnavailableError"
        )

    def build(self, namespace=None, *, bases=(), fields=(), final=()):
        owner = object()
        namespace_function = lambda namespace, cell: None
        spec = ConstructionSpec(
                   4,
                   ctypes.sizeof(ConstructionSpec),
                   0,
                   0,
                   owner,
                   namespace_function,
                   "MemberTarget",
                   bases,
                   {} if namespace is None else namespace,
                   {},
                   None,
                   None,
                   TypeContractSpecV4(flags=0, fields=fields, protected_names=(), final_methods=final, check_instance_write=None, new_instance_dict=None),
               )
        handle = self.new_handle(ctypes.byref(spec))
        return self.construct(handle, namespace_function), owner

    def fixture(
        self,
        actual,
        owner,
        install,
        *,
        name="generated",
        role=1,
        root=apply_member,
        members=None,
        sites=None,
        payload=None,
    ):
        if sites is None:
            sites = (
                (root, instruction(root, "MAKE_FUNCTION"), child_code(root), role, ()),
            )
        if members is None:
            members = ((name, role, child_code(root), root.__globals__, None, None),)
        invocation = _testinternalcapi.soac_dataclass_fixture(
            root, ((0, actual), (2, install)), (), sites, payload, members
        )
        self.assertEqual(self.bind(invocation, actual, owner), 0)
        return invocation

    def call(self, invocation, actual, install, root=apply_member):
        return _testinternalcapi.soac_dataclass_fixture_call(
            invocation, 2, root, (actual, invocation, install), {}
        )

    def test_actual_fresh_member_is_frozen_without_a_source_or_jit_capability(self):
        for replace in (False, True):
            with self.subTest(replace=replace):
                actual, owner = self.build({"generated": object()} if replace else {})
                observed = []

                def install(invocation, cls, function, observed=observed):
                    observed.append(function)
                    self.assertEqual(self.has_creation(function), 1)
                    self.assertEqual(self.strict_id(function), 0)
                    self.assertEqual(
                        self.set_member(invocation, cls, "generated", function), 0
                    )

                invocation = self.fixture(actual, owner, install)
                self.assertIs(self.call(invocation, actual, install), actual)
                self.assertEqual(self.complete(invocation), 0)
                (function,) = observed
                self.assertIs(vars(actual)["generated"], function)
                self.assertEqual(actual().generated(17), 17)
                self.assertGreater(self.strict_id(function), 0)
                self.assertIsNone(self.metadata(function))
                with self.assertRaises(self.mutation_error):
                    function.__defaults__ = (17,)
                copied = types.FunctionType(function.__code__, function.__globals__)
                self.assertEqual(self.has_creation(copied), 0)
                self.assertEqual(self.strict_id(copied), 0)
                copied.__defaults__ = (23,)
                self.assertEqual(copied(None), 23)
                self.assertEqual(self.seal_class(actual, owner), 0)
                with self.assertRaises(self.mutation_error):
                    actual.generated = copied
                with self.assertRaises(self.mutation_error):
                    self.get_dict(actual)["generated"] = copied

    def test_frozen_hooks_use_exact_roles_and_the_normal_type_slot_update(self):
        for name, role, root, message in (
            ("__setattr__", 2, apply_setattr, "generated frozen setter"),
            ("__delattr__", 3, apply_delattr, "generated frozen deleter"),
        ):
            with self.subTest(name=name):
                actual, owner = self.build()

                def install(invocation, cls, function, name=name):
                    with self.assertRaises(self.mutation_error):
                        setattr(cls, name, function)
                    with self.assertRaises(self.mutation_error):
                        self.get_dict(cls)[name] = function
                    self.assertEqual(
                        self.set_member(invocation, cls, name, function), 0
                    )

                invocation = self.fixture(
                    actual, owner, install, name=name, role=role, root=root
                )
                self.assertIs(self.call(invocation, actual, install, root), actual)
                self.assertEqual(self.complete(invocation), 0)
                with self.assertRaisesRegex(AttributeError, message):
                    if name == "__setattr__":
                        actual().field = 1
                    else:
                        del actual().field
                self.assertEqual(self.seal_class(actual, owner), 0)
                with self.assertRaises(self.mutation_error):
                    setattr(actual, name, object.__setattr__)

    def test_copies_changed_metadata_wrong_names_and_foreign_classes_do_not_adopt(self):
        for change in ("copy", "defaults", "name", "class", "hook_role"):
            with self.subTest(change=change):
                actual, owner = self.build()
                foreign, _ = self.build()
                observed = []
                name = "__setattr__" if change == "hook_role" else "generated"

                def install(
                    invocation,
                    cls,
                    function,
                    observed=observed,
                    name=name,
                    change=change,
                    foreign=foreign,
                ):
                    observed.append(function)
                    target, installed_name, installed = cls, name, function
                    if change == "copy":
                        installed = types.FunctionType(
                            function.__code__, function.__globals__
                        )
                    elif change == "defaults":
                        function.__defaults__ = (19,)
                    elif change == "name":
                        installed_name = "other"
                    elif change == "class":
                        target = foreign
                    self.set_member(invocation, target, installed_name, installed)

                invocation = self.fixture(actual, owner, install, name=name)
                with self.assertRaises(self.unavailable_error):
                    self.call(invocation, actual, install)
                self.assertNotIn(name, vars(actual))
                self.assertNotIn("other", vars(actual))
                self.assertNotIn(name, vars(foreign))
                self.assertEqual(self.has_class(actual), 1)
                self.assertEqual(self.seal_class(actual, owner), 0)
                with self.assertRaises(self.mutation_error):
                    actual.extra = 1
                self.assertEqual(len(observed), 1)

    def test_consumed_record_cannot_be_replayed_and_failure_does_not_unseal(self):
        actual, owner = self.build()
        observed = []

        def install(invocation, cls, function):
            observed.append(function)
            self.assertEqual(self.set_member(invocation, cls, "generated", function), 0)
            self.set_member(invocation, cls, "generated", function)

        invocation = self.fixture(actual, owner, install)
        with self.assertRaises(self.unavailable_error):
            self.call(invocation, actual, install)
        (function,) = observed
        self.assertIs(vars(actual)["generated"], function)
        with self.assertRaises(self.mutation_error):
            function.__code__ = function.__code__
        self.assertEqual(self.seal_class(actual, owner), 0)
        with self.assertRaises(self.mutation_error):
            actual.generated = None

    def test_member_path_preserves_final_field_descriptor_and_sealed_barriers(self):
        for barrier in ("final", "field", "sealed"):
            with self.subTest(barrier=barrier):
                if barrier == "final":
                    base, base_owner = self.build(
                        {"generated": lambda self, value: value}, final=("generated",)
                    )
                    self.assertEqual(self.seal_class(base, base_owner), 0)
                    actual, owner = self.build(bases=(base,))
                elif barrier == "field":
                    actual, owner = self.build({"generated": 29}, fields=("generated",))
                else:
                    actual, owner = self.build()
                    self.assertEqual(self.seal_class(actual, owner), 0)
                before = dict(vars(actual))

                def install(invocation, cls, function):
                    self.set_member(invocation, cls, "generated", function)

                invocation = self.fixture(actual, owner, install)
                with self.assertRaises(self.mutation_error):
                    self.call(invocation, actual, install)
                self.assertEqual(dict(vars(actual)), before)
                self.assertEqual(self.has_class(actual), 1)

    def watch(self, namespace, callback):
        callback_type = ctypes.CFUNCTYPE(
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
        )
        wrapped = callback_type(callback)
        add = native_api("PyDict_AddWatcher", ctypes.c_int, callback_type)
        watch = native_api("PyDict_Watch", ctypes.c_int, ctypes.c_int, ctypes.py_object)
        identity = add(wrapped)
        self.assertGreaterEqual(identity, 0)
        self.assertEqual(watch(identity, namespace), 0)
        return identity, wrapped

    def unwatch(self, namespace, identity):
        native_api("PyDict_Unwatch", ctypes.c_int, ctypes.c_int, ctypes.py_object)(
            identity, namespace
        )
        native_api("PyDict_ClearWatcher", ctypes.c_int, ctypes.c_int)(identity)

    def test_watcher_failure_revalidates_the_same_policy_before_insert_or_replace(self):
        for replace in (False, True):
            with self.subTest(replace=replace):
                previous = object()
                actual, owner = self.build({"generated": previous} if replace else {})
                namespace = self.get_dict(actual)
                observed, errors = [], []

                def install(
                    invocation,
                    cls,
                    function,
                    namespace=namespace,
                    observed=observed,
                    errors=errors,
                ):
                    def callback(event, dictionary, key, value):
                        if dictionary == id(namespace):
                            try:
                                observed.append(self.strict_id(function))
                                self.fail(invocation)
                            except BaseException as error:  # noqa: BLE001 -- asserted after the C callback
                                errors.append(error)
                        return 0

                    identity, keep_callback_alive = self.watch(namespace, callback)
                    try:
                        self.set_member(invocation, cls, "generated", function)
                    finally:
                        self.unwatch(namespace, identity)
                        del keep_callback_alive

                invocation = self.fixture(actual, owner, install)
                with self.assertRaises(self.unavailable_error):
                    self.call(invocation, actual, install)
                self.assertEqual(errors, [])
                self.assertEqual(len(observed), 1)
                self.assertGreater(observed[0], 0)
                if replace:
                    self.assertIs(namespace["generated"], previous)
                else:
                    self.assertNotIn("generated", namespace)

    def test_watcher_mapping_write_has_no_member_authority(self):
        actual, owner = self.build()
        namespace = self.get_dict(actual)
        errors, rejected = [], []

        def install(invocation, cls, function):
            def callback(event, dictionary, key, value):
                if dictionary == id(namespace):
                    try:
                        namespace["forged"] = function
                    except self.mutation_error:
                        rejected.append(True)
                    except BaseException as error:  # noqa: BLE001 -- asserted after the C callback
                        errors.append(error)
                return 0

            identity, keep_callback_alive = self.watch(namespace, callback)
            try:
                self.set_member(invocation, cls, "generated", function)
            finally:
                self.unwatch(namespace, identity)
                del keep_callback_alive

        invocation = self.fixture(actual, owner, install)
        self.assertIs(self.call(invocation, actual, install), actual)
        self.assertEqual(self.complete(invocation), 0)
        self.assertEqual(errors, [])
        self.assertEqual(rejected, [True])
        self.assertNotIn("forged", namespace)

    def test_displaced_value_finalizer_runs_after_the_operation_is_finished(self):
        state, events = {}, []

        class Previous:
            def __del__(self):
                try:
                    cls, invocation, first, second = state["operands"]
                    events.append((vars(cls)["generated"] is first, cls().generated(7)))
                    # A new, independently fresh operation is legal here. The
                    # displaced-value release must not leave the first pending.
                    self.set_member(invocation, cls, "other", second)
                except BaseException as error:  # noqa: BLE001 -- asserted after finalization
                    events.append(error)

        # Do not retain the replaced object outside its actual type dictionary.
        Previous.set_member = self.set_member
        actual, owner = self.build({"generated": Previous()})

        def root(actual, invocation, install):
            def first(self, value):
                return value

            def second(self, value):
                return value + 1

            install(invocation, actual, first, second)
            return actual

        codes = [
            item for item in root.__code__.co_consts if isinstance(item, types.CodeType)
        ]
        offsets = [
            item.offset // 2
            for item in dis.get_instructions(root)
            if item.opname == "MAKE_FUNCTION"
        ]
        self.assertEqual(len(codes), 2)
        self.assertEqual(len(offsets), 2)
        sites = tuple(
            (root, offset, code, 1, ())
            for offset, code in zip(offsets, codes, strict=True)
        )
        members = tuple(
            (name, 1, code, root.__globals__, None, None)
            for name, code in zip(("generated", "other"), codes, strict=True)
        )

        def install(invocation, cls, first, second):
            state["operands"] = cls, invocation, first, second
            self.set_member(invocation, cls, "generated", first)

        invocation = self.fixture(
            actual, owner, install, root=root, members=members, sites=sites
        )
        self.assertIs(self.call(invocation, actual, install, root), actual)
        self.assertEqual(self.complete(invocation), 0)
        self.assertEqual(events, [(True, 7)])
        self.assertEqual(actual().other(7), 8)

    def test_direct_native_member_bridge_uses_the_actual_frame_and_fresh_function(self):
        import _types

        actual, owner = self.build()
        bridge = _types._dataclass_set_member

        def root(actual, invocation, member_bridge):
            def generated(self, value):
                return value

            member_bridge(setattr, actual, "generated", generated)
            return actual

        payload = (
            "bridges-v1",
            ((instruction(root, "CALL"), 3, (setattr, actual, "generated", None)),),
            None,
            [None],
        )
        invocation = self.fixture(actual, owner, bridge, root=root, payload=payload)
        self.assertIs(self.call(invocation, actual, bridge, root), actual)
        self.assertEqual(self.complete(invocation), 0)
        function = vars(actual)["generated"]
        self.assertEqual(self.has_creation(function), 1)
        self.assertGreater(self.strict_id(function), 0)
        self.assertIsNone(self.metadata(function))
        self.assertEqual(actual().generated(31), 31)
        with self.assertRaises(self.mutation_error):
            function.__code__ = function.__code__


    def pending_build(self, namespace=None, *, bases=()):
        owner = ([False] * 4, None, (), None)
        function = lambda: None
        actual, root = _testinternalcapi.soac_pending_type_construct(
            "PendingMemberTarget", bases, {} if namespace is None else namespace,
            function, owner,
        )
        return actual, owner, root

    def pending_info(self, actual):
        info = ConstructionInfoV1()
        query = native_api(
            "PyType_GetSoacConstructionInfoV1", ctypes.c_int,
            ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
        )
        self.assertEqual(query(actual, ctypes.byref(info), ctypes.sizeof(info)), 1)
        return info

    def pending_admit(self, actual, owner, root):
        return _testinternalcapi.soac_pending_type_admit(
            actual, owner, root, (), (), (), None,
        )

    def test_pending_native_member_and_frozen_roles_survive_actual_completion_then_admission(self):
        for name, role, body in (
            ("generated", 1, apply_member),
            ("__setattr__", 2, apply_setattr),
            ("__delattr__", 3, apply_delattr),
        ):
            with self.subTest(name=name):
                actual, owner, root = self.pending_build()
                observed = []

                def install(invocation, cls, function):
                    self.assertEqual(self.has_creation(function), 1)
                    self.assertEqual(self.strict_id(function), 0)
                    self.assertEqual(self.set_member(invocation, cls, name, function), 0)
                    observed.append(weakref.ref(function))

                invocation = self.fixture(actual, owner, install, name=name, role=role, root=body)
                self.assertIs(self.call(invocation, actual, install, body), actual)
                self.assertEqual(self.complete(invocation), 0)
                self.assertEqual(self.pending_info(actual).phase, 1)
                self.assertEqual(self.has_class(actual), 0)
                self.assertIs(vars(actual)[name], observed[0]())
                self.pending_admit(actual, owner, root)
                self.assertEqual(self.pending_info(actual).phase, 3)
                if role == 1:
                    self.assertEqual(actual().generated(41), 41)
                elif role == 2:
                    with self.assertRaisesRegex(AttributeError, "generated frozen setter"):
                        actual().value = 41
                else:
                    with self.assertRaisesRegex(AttributeError, "generated frozen deleter"):
                        del actual().value
                self.assertEqual(self.seal_class(actual, owner), 0)
                with self.assertRaises(self.mutation_error):
                    setattr(actual, name, lambda *args: None)

    def test_pending_frozen_birth_does_not_accept_a_same_id_code_copy(self):
        actual, owner, root = self.pending_build()
        functions = []

        def install(invocation, cls, function):
            self.set_member(invocation, cls, "__setattr__", function)
            functions.append(function)

        invocation = self.fixture(
            actual, owner, install, name="__setattr__", role=2, root=apply_setattr,
        )
        self.call(invocation, actual, install, apply_setattr)
        self.complete(invocation)
        original = functions.pop()
        copied = types.FunctionType(original.__code__, original.__globals__)
        seal_function = native_api(
            "PyFunction_SealSoacStrict", ctypes.c_int, ctypes.py_object, ctypes.c_uint64,
        )
        self.assertEqual(seal_function(copied, self.strict_id(original)), 0)
        self.assertEqual(self.strict_id(copied), self.strict_id(original))
        self.assertEqual(self.has_creation(copied), 0)
        actual.__setattr__ = copied
        self.assertIs(vars(actual)["__setattr__"], copied)
        with self.assertRaises(self.mutation_error):
            self.pending_admit(actual, owner, root)
        self.assertEqual(self.pending_info(actual).phase, 4)
        with self.assertRaises(self.mutation_error):
            actual()
        self.assertIs(vars(actual)["__setattr__"], copied)

    def test_pending_member_collision_preserves_the_ordinary_type_set_lookup_schedule(self):
        def prepare(pending):
            events = []

            class Key:
                def __hash__(self):
                    return hash("generated")

                def __eq__(self, other):
                    events.append(("eq", other))
                    return other == "generated"

            class Previous:
                def __del__(self):
                    events.append(("drop",))

            key = Key()
            if pending:
                actual, owner, root = self.pending_build({key: Previous()})
            else:
                actual = type("OrdinaryMemberTarget", (), {key: Previous()})
                owner = root = None
            return actual, owner, root, key, events

        ordinary, _, _, ordinary_key, ordinary_events = prepare(False)
        ordinary_events.clear()
        setattr(ordinary, "generated", lambda self, value: value)
        expected = list(ordinary_events)
        self.assertTrue(expected)
        self.assertEqual(expected[-1], ("drop",))
        self.assertTrue(any(key is ordinary_key for key in vars(ordinary)))

        actual, owner, root, key, events = prepare(True)

        def install(invocation, cls, function):
            events.clear()
            self.set_member(invocation, cls, "generated", function)

        invocation = self.fixture(actual, owner, install)
        self.call(invocation, actual, install)
        self.assertEqual(events, expected)
        self.assertTrue(any(candidate is key for candidate in vars(actual)))
        self.complete(invocation)
        # No final policy/admission claim for this non-string class namespace.
        self.assertEqual(self.pending_info(actual).phase, 1)
        with self.assertRaises(self.mutation_error):
            actual()

    def test_pending_member_watcher_value_substitution_invalidates_the_resolved_commit(self):
        actual, owner, root = self.pending_build({"generated": object()})
        namespace = self.get_dict(actual)
        replacement = object()
        observed, errors, functions = [], [], []

        def install(invocation, cls, function):
            functions.append(weakref.ref(function))

            def callback(event, dictionary, key, value):
                if dictionary == id(namespace) and not observed:
                    observed.append(event)
                    try:
                        # Existing key/keys object survive, but the exact value
                        # association changes after the selected lookup.
                        namespace["generated"] = replacement
                    except BaseException as error:  # noqa: BLE001 -- checked outside C
                        errors.append(error)
                return 0

            identity, keep_alive = self.watch(namespace, callback)
            try:
                self.set_member(invocation, cls, "generated", function)
            finally:
                self.unwatch(namespace, identity)
                del keep_alive

        invocation = self.fixture(actual, owner, install)
        with self.assertRaises(self.mutation_error):
            self.call(invocation, actual, install)
        self.assertEqual(errors, [])
        self.assertEqual(len(observed), 1)
        self.assertIs(namespace["generated"], replacement)
        self.assertEqual(self.pending_info(actual).phase, 4)
        gc.collect()
        self.assertIsNone(functions[0]())
        with self.assertRaises(self.mutation_error):
            actual()

    def test_pending_member_keeps_inherited_finality_without_a_full_own_policy(self):
        base, base_owner = self.build(
            {"fixed": lambda self: 1}, final=("fixed",),
        )
        self.seal_class(base, base_owner)
        actual, owner, root = self.pending_build(bases=(base,))

        def install(invocation, cls, function):
            self.set_member(invocation, cls, "generated", function)

        invocation = self.fixture(actual, owner, install)
        self.call(invocation, actual, install)
        self.complete(invocation)
        self.assertEqual(self.pending_info(actual).phase, 1)
        namespace = self.get_dict(actual)
        with self.assertRaises(self.mutation_error):
            namespace["fixed"] = None
        self.pending_admit(actual, owner, root)
        self.assertEqual(actual().generated(43), 43)
        self.assertEqual(actual().fixed(), 1)

    def test_pending_commit_does_not_reenter_registered_member_validation_after_watcher(self):
        actual, owner, root = self.pending_build()
        namespace = self.get_dict(actual)
        marker = object()
        validations, watched, functions = [], [], []
        late_error = LookupError("registered member validation ran after the watcher")

        def validate(candidate, candidate_owner, name, function):
            self.assertIs(candidate, actual)
            self.assertIs(candidate_owner, owner)
            self.assertEqual(name, "generated")
            validations.append(bool(watched))
            # This is the real registered callback, not a synthetic commit hook.
            # Mutation and GC are allowed before the native setter resolves.
            namespace["validation_seen"] = marker
            gc.collect()
            if watched:
                # In the broken implementation this invalidates native storage,
                # then returns an error before an unsafe stale physical write.
                for index in range(32):
                    namespace[f"late_validation_{index}"] = index
                raise late_error

        def install(invocation, cls, function):
            functions.append(weakref.ref(function))

            def watcher(event, dictionary, key, value):
                if dictionary == id(namespace) and value == id(function):
                    watched.append(True)
                return 0

            identity, keep_alive = self.watch(namespace, watcher)
            try:
                self.set_member(invocation, cls, "generated", function)
            finally:
                self.unwatch(namespace, identity)
                del keep_alive

        invocation = self.fixture(
            actual, owner, install, payload=("member-validation-v1", validate),
        )
        self.assertIs(self.call(invocation, actual, install), actual)
        self.assertTrue(validations)
        self.assertEqual(validations, [False] * len(validations))
        self.assertEqual(watched, [True])
        self.assertIs(namespace["validation_seen"], marker)
        self.assertNotIn("late_validation_0", namespace)
        self.assertIs(namespace["generated"], functions[0]())
        self.assertEqual(self.complete(invocation), 0)
        self.pending_admit(actual, owner, root)
        self.assertEqual(actual().generated(17), 17)

    def test_pending_initial_member_validation_failure_preserves_primary_and_target(self):
        previous = object()
        actual, owner, root = self.pending_build({"generated": previous})
        namespace = self.get_dict(actual)
        primary = LookupError("actual registered member validation failed")
        validations = []

        def validate(candidate, candidate_owner, name, function):
            self.assertIs(candidate, actual)
            self.assertIs(candidate_owner, owner)
            validations.append((name, self.has_creation(function)))
            gc.collect()
            raise primary

        def install(invocation, cls, function):
            self.set_member(invocation, cls, "generated", function)

        invocation = self.fixture(
            actual, owner, install, payload=("member-validation-v1", validate),
        )
        with self.assertRaises(LookupError) as caught:
            self.call(invocation, actual, install)
        self.assertIs(caught.exception, primary)
        self.assertIsNone(primary.__cause__)
        self.assertIsNone(primary.__context__)
        self.assertEqual(validations, [("generated", 1)])
        self.assertIs(namespace["generated"], previous)
        self.assertEqual(self.pending_info(actual).phase, 4)
        with self.assertRaises(self.mutation_error):
            actual()

    def test_failed_pending_member_releases_inherited_policy_guard_before_retry(self):
        base, _ = self.build({"kept": lambda self: None}, final=("kept",))
        actual, owner, root = self.pending_build(bases=(base,))
        namespace = self.get_dict(actual)
        watched, errors = [], []

        def install(invocation, cls, function):
            def watcher(event, dictionary, key, value):
                if dictionary == id(namespace) and value == id(function) and not watched:
                    watched.append(True)
                    try:
                        self.fail(invocation)
                    except BaseException as error:  # noqa: BLE001 -- checked after C
                        errors.append(error)
                return 0

            identity, keep_alive = self.watch(namespace, watcher)
            try:
                self.set_member(invocation, cls, "generated", function)
            finally:
                self.unwatch(namespace, identity)
                del keep_alive

        invocation = self.fixture(actual, owner, install)
        with self.assertRaises(self.unavailable_error):
            self.call(invocation, actual, install)
        self.assertEqual(watched, [True])
        self.assertEqual(errors, [])
        self.assertNotIn("generated", namespace)
        self.assertEqual(self.pending_info(actual).phase, 4)
        # A failed type stays non-instantiable. Its actual inherited-only raw
        # namespace policy must still allow unrelated ordinary dictionary writes.
        marker = object()
        namespace["after_failed_member"] = marker
        self.assertIs(namespace["after_failed_member"], marker)
        with self.assertRaises(self.mutation_error):
            namespace["kept"] = lambda self: None
        with self.assertRaises(self.mutation_error):
            actual()


if __name__ == "__main__":
    unittest.main()
