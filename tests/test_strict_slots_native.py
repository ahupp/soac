"""Physical native member contracts, using a C-owned exact-int test policy.

The test fixture supplies no artifact, source-function, or JIT authority. The
production constructor separately authenticates its explicit slot-field plan.
"""

import _testinternalcapi
import ctypes
import dis
import gc
import types
import unittest
import weakref

from tests.test_strict_type_native import TypeContractSpecV4
from tests.test_strict_type_native import ConstructionInfoV1, ConstructionSpec


def replace_frozen(cls, creator, name, bases, namespace, install):
    def generated_setattr(self, name, value):
        raise AttributeError("generated frozen setter")

    def generated_delattr(self, name):
        raise AttributeError("generated frozen deleter")

    install(cls, namespace, generated_setattr, generated_delattr)
    return creator(type(cls), name, bases, namespace, cls)


class StrictObjectSlotNativeTests(unittest.TestCase):
    def build(
        self,
        *,
        name="CheckedSlots",
        bases=(),
        fields=("value",),
        checked=("value",),
        slots=None,
        members=None,
        payload=None,
        watcher=-1,
        dictionary_fields=(),
    ):
        namespace_function = lambda namespace, cell: None
        namespace = {
            "__module__": __name__,
            "__slots__": fields if slots is None else slots,
            **(members or {}),
        }
        observations = [False, False]
        actual = _testinternalcapi.soac_slot_type(
            name,
            bases,
            namespace,
            fields,
            checked,
            namespace_function,
            observations,
            payload,
            watcher,
            dictionary_fields,
        )
        self.assertEqual(observations, [True, True])
        return actual

    @staticmethod
    def native_setattr(receiver, name, value):
        setter = ctypes.pythonapi.PyObject_SetAttr
        setter.argtypes = [ctypes.py_object] * 3
        setter.restype = ctypes.c_int
        setter(receiver, name, value)

    @staticmethod
    def generic_setattr(receiver, name, value):
        setter = ctypes.pythonapi.PyObject_GenericSetAttr
        setter.argtypes = [ctypes.py_object] * 3
        setter.restype = ctypes.c_int
        setter(receiver, name, value)

    def test_slot_plan_precedes_actual_type_ready_callbacks(self):
        events = []
        case = self

        class Descriptor:
            def __set_name__(self, actual, name):
                instance = actual()
                instance.value = 7
                with case.assertRaisesRegex(TypeError, "exact int"):
                    instance.value = "wrong"
                events.append((name, instance.value))

        actual = self.build(members={"hook": Descriptor()})
        self.assertEqual(events, [("hook", 7)])
        instance = actual()
        with self.assertRaises(TypeError):
            vars(instance)
        with self.assertRaises(AttributeError):
            _ = instance.value

    def test_all_public_object_slot_write_paths_check_the_canonical_field(self):
        actual = self.build()
        descriptor = actual.__dict__["value"]
        operations = (
            ("setattr", lambda obj, value: setattr(obj, "value", value)),
            (
                "object setter",
                lambda obj, value: object.__setattr__(obj, "value", value),
            ),
            ("descriptor", descriptor.__set__),
            ("C setter", lambda obj, value: self.native_setattr(obj, "value", value)),
            ("C generic", lambda obj, value: self.generic_setattr(obj, "value", value)),
            (
                "copied member",
                lambda obj, value: _testinternalcapi.soac_slot_view_set(
                    descriptor, obj, value
                ),
            ),
            (
                "renamed member",
                lambda obj, value: _testinternalcapi.soac_slot_view_set(
                    descriptor, obj, value, name="unrelated"
                ),
            ),
            (
                "object-kind view",
                lambda obj, value: _testinternalcapi.soac_slot_view_set(
                    descriptor, obj, value, name="other", member_type=6
                ),
            ),
        )
        for label, write in operations:
            with self.subTest(operation=label):
                instance = actual()
                write(instance, 17)
                with self.assertRaisesRegex(TypeError, "exact int"):
                    write(instance, "wrong")
                self.assertEqual(instance.value, 17)
                write(instance, 23)
                self.assertEqual(instance.value, 23)

    def test_member_view_names_and_unbound_read_delete_semantics_stay_native(self):
        actual = self.build(fields=("λ",), checked=("λ",))
        descriptor = actual.__dict__["λ"]
        instance = actual()
        with self.assertRaisesRegex(AttributeError, "renamed"):
            _testinternalcapi.soac_slot_view_get(descriptor, instance, name="renamed")
        self.assertIsNone(
            _testinternalcapi.soac_slot_view_get(
                descriptor, instance, name="renamed", member_type=6
            )
        )
        with self.assertRaises(AttributeError):
            _testinternalcapi.soac_slot_view_set(
                descriptor, instance, None, delete=True
            )
        _testinternalcapi.soac_slot_view_set(
            descriptor, instance, None, member_type=6, delete=True
        )
        _testinternalcapi.soac_slot_view_set(descriptor, instance, 31, name="renamed")
        self.assertEqual(instance.λ, 31)
        with self.assertRaisesRegex(AttributeError, "readonly"):
            _testinternalcapi.soac_slot_view_set(descriptor, instance, "wrong", flags=1)
        self.assertEqual(instance.λ, 31)
        _testinternalcapi.soac_slot_view_set(descriptor, instance, None, delete=True)
        with self.assertRaises(AttributeError):
            _ = instance.λ

    def test_native_slot_preserves_a_hidden_inherited_dictionary_prefix(self):
        namespace_function = lambda namespace, cell: None
        base = _testinternalcapi.dict_new_soac_type(
            "DictionaryBase",
            (),
            {"__module__": __name__},
            ("value",),
            namespace_function,
        )
        actual = self.build(bases=(base,), dictionary_fields=("value",))
        self.assertIsInstance(actual.__dict__["value"], types.MemberDescriptorType)
        for receiver_type in (actual, type("OrdinaryHybrid", (actual,), {})):
            with self.subTest(receiver=receiver_type):
                instance = receiver_type()
                dictionary = vars(instance)
                self.assertEqual(dictionary, {})
                instance.value = 101
                self.assertEqual(instance.value, 101)
                # Attribute storage is the member. The inherited indexed
                # dictionary location remains unset; there is no mirroring.
                self.assertEqual(dictionary, {})
                dictionary["value"] = 103
                self.assertEqual(instance.value, 101)
                self.assertEqual(dictionary, {"value": 103})
                with self.assertRaisesRegex(TypeError, "exact int"):
                    instance.value = "wrong slot"
                with self.assertRaisesRegex(TypeError, "exact int"):
                    dictionary["value"] = "wrong dictionary"
                with self.assertRaises(TypeError):
                    instance.__dict__ = {}
                del instance.value
                with self.assertRaises(AttributeError):
                    _ = instance.value
                self.assertEqual(dictionary, {"value": 103})
                del dictionary["value"]
                self.assertEqual(dictionary, {})
                instance.value = 107
                self.assertEqual(dictionary, {})
        base_instance = base()
        base_instance.value = 109
        self.assertEqual(vars(base_instance), {"value": 109})

    def test_native_slot_preserves_an_inherited_ordinary_dictionary_field(self):
        namespace_function = lambda namespace, cell: None
        base = _testinternalcapi.dict_new_soac_ordinary_type(
            "OrdinaryDictionaryBase",
            (),
            {"__module__": __name__},
            ("value",),
            namespace_function,
        )
        actual = self.build(bases=(base,), dictionary_fields=("value",))
        self.assertIsInstance(actual.__dict__["value"], types.MemberDescriptorType)
        for receiver_type in (actual, type("OrdinaryHybridChild", (actual,), {})):
            with self.subTest(receiver=receiver_type):
                instance = receiver_type()
                dictionary = vars(instance)
                self.assertEqual(dictionary, {})
                with self.assertRaisesRegex(TypeError, "expected an indexed dictionary"):
                    _testinternalcapi.dict_indexed_key_index(dictionary, "value")
                instance.value = 101
                self.assertEqual(dictionary, {})
                dictionary["value"] = 103
                self.assertEqual(instance.value, 101)
                with self.assertRaisesRegex(TypeError, "exact int"):
                    instance.value = "wrong slot"
                with self.assertRaisesRegex(TypeError, "exact int"):
                    dictionary["value"] = "wrong dictionary"
                self.assertEqual(dictionary, {"value": 103})
                replacement = {"value": 107, "extra": "ordinary"}
                instance.__dict__ = replacement
                self.assertIs(vars(instance), replacement)
                self.assertEqual(instance.value, 101)
                for alias in (dictionary, replacement):
                    with self.assertRaisesRegex(TypeError, "exact int"):
                        alias["value"] = "wrong alias"
                del instance.value
                with self.assertRaises(AttributeError):
                    _ = instance.value
                self.assertEqual(replacement, {"value": 107, "extra": "ordinary"})
                instance.value = 109
                self.assertEqual(replacement["value"], 107)

    def test_native_slot_preserves_an_unchecked_inherited_dictionary_field(self):
        # NONE selects no dictionary value policy; the inherited logical field
        # and actual dictionary layout still exist independently of that choice.
        base = self.build(
            name="UncheckedDictionaryBase",
            fields=(),
            checked=(),
            slots=("__dict__",),
            dictionary_fields=("value",),
        )
        actual = self.build(bases=(base,), dictionary_fields=("value",))
        for receiver_type in (actual, type("UncheckedHybridChild", (actual,), {})):
            with self.subTest(receiver=receiver_type):
                instance = receiver_type()
                dictionary = vars(instance)
                instance.value = 113
                self.assertEqual(dictionary, {})
                dictionary["value"] = "unchecked hidden value"
                self.assertEqual(instance.value, 113)
                with self.assertRaisesRegex(TypeError, "exact int"):
                    instance.value = "wrong slot"
                replacement = {"value": "unchecked replacement"}
                instance.__dict__ = replacement
                self.assertIs(vars(instance), replacement)
                self.assertEqual(instance.value, 113)
                dictionary["value"] = "old alias remains unchecked"
                self.assertEqual(replacement, {"value": "unchecked replacement"})
                del instance.value
                with self.assertRaises(AttributeError):
                    _ = instance.value
                self.assertEqual(replacement["value"], "unchecked replacement")

    def test_slot_overlap_requires_an_inherited_field_and_actual_dictionary(self):
        namespace_function = lambda namespace, cell: None
        bases = (
            self.build(
                name="DictionaryWithoutField",
                fields=(), checked=(), slots=("__dict__",),
            ),
            self.build(
                name="FieldWithoutDictionary",
                fields=(), checked=(), slots=(), dictionary_fields=("value",),
            ),
            _testinternalcapi.dict_new_soac_ordinary_type(
                "DifferentOrdinaryField", (), {"__module__": __name__},
                ("other",), namespace_function,
            ),
        )
        for base in bases:
            with self.subTest(base=base.__name__):
                with self.assertRaisesRegex(TypeError, "dictionary.*native-slot"):
                    self.build(bases=(base,), dictionary_fields=("value",))

        # A fresh pending sidecar has no published field catalogue. It cannot
        # supply overlap permission merely because its type has a dictionary.
        owner = ([False] * 4, None, (), None)
        pending, construction = _testinternalcapi.soac_pending_type_construct(
            "UnpreparedDictionary", (), {"__module__": __name__},
            namespace_function, owner,
        )
        with self.assertRaisesRegex(TypeError, "pending or failed"):
            self.build(bases=(pending,), dictionary_fields=("value",))
        self.assertEqual(owner[0], [True, False, False, False])
        with self.assertRaisesRegex(TypeError, "pending or failed"):
            pending()
        _testinternalcapi.soac_pending_type_admit(
            pending, owner, construction, (), (), (), None,
        )
        self.assertEqual(vars(pending()), {})

    def test_two_new_storage_locations_cannot_be_selected_for_one_field(self):
        with self.assertRaisesRegex(TypeError, "dictionary.*native-slot"):
            self.build(dictionary_fields=("value",))

    def test_unsafe_member_representation_views_cannot_overlap_an_object_slot(self):
        actual = self.build()
        descriptor = actual.__dict__["value"]
        instance = actual()
        instance.value = 41
        for member_type, offset_delta in ((1, 0), (1, 1), (6, 1)):
            with self.subTest(member_type=member_type, offset_delta=offset_delta):
                with self.assertRaises(TypeError):
                    _testinternalcapi.soac_slot_view_get(
                        descriptor,
                        instance,
                        member_type=member_type,
                        offset_delta=offset_delta,
                    )
                with self.assertRaises(TypeError):
                    _testinternalcapi.soac_slot_view_set(
                        descriptor,
                        instance,
                        1,
                        member_type=member_type,
                        offset_delta=offset_delta,
                    )
                self.assertEqual(instance.value, 41)

    def test_warmed_slot_bytecode_reaches_the_same_native_contract(self):
        ordinary = type("OrdinarySlots", (), {"__slots__": ("value",)})

        def write_and_read(receiver, value):
            receiver.value = value
            return receiver.value

        control = ordinary()
        for number in range(2000):
            self.assertEqual(write_and_read(control, number), number)
        instructions = list(dis.get_instructions(write_and_read, adaptive=True))
        self.assertIn(
            dis._all_opmap["STORE_ATTR_SLOT"],
            [operation.opcode for operation in instructions],
        )
        self.assertIn(
            dis._all_opmap["LOAD_ATTR_SLOT"],
            [operation.opcode for operation in instructions],
        )
        actual = self.build()
        checked = actual()
        for number in range(2000):
            self.assertEqual(write_and_read(checked, number), number)
        with self.assertRaisesRegex(TypeError, "exact int"):
            write_and_read(checked, "wrong")
        self.assertEqual(checked.value, 1999)
        self.assertEqual(write_and_read(control, "ordinary"), "ordinary")

    def test_ordinary_descendants_keep_dictionary_and_independent_slot_authority(self):
        base = self.build()
        ordinary = type("OrdinaryDescendant", (base,), {})
        instance = ordinary()
        replacement = {"value": "hidden", "extra": "ordinary"}
        instance.__dict__ = replacement
        self.assertIs(vars(instance), replacement)
        instance.value = 43
        self.assertEqual(instance.value, 43)
        with self.assertRaisesRegex(TypeError, "exact int"):
            instance.value = "wrong"
        instance.extra = 47
        self.assertEqual(replacement["extra"], 47)

        shadow = type("IndependentSlot", (base,), {"__slots__": ("value",)})
        second = shadow()
        second.value = "ordinary independent slot"
        descriptor = base.__dict__["value"]
        with self.assertRaisesRegex(TypeError, "exact int"):
            descriptor.__set__(second, "wrong")
        _testinternalcapi.soac_slot_view_set(descriptor, second, 53, name="new_name")
        self.assertEqual(descriptor.__get__(second, shadow), 53)
        self.assertEqual(second.value, "ordinary independent slot")

    def test_physical_offset_not_descriptor_spelling_selects_between_two_fields(self):
        actual = self.build(fields=("value", "payload"), checked=("value",))
        value = actual.__dict__["value"]
        payload = actual.__dict__["payload"]
        delta = (
            _testinternalcapi.soac_slot_definition(payload)[1]
            - _testinternalcapi.soac_slot_definition(value)[1]
        )
        instance = actual()
        _testinternalcapi.soac_slot_view_set(
            value, instance, "ordinary payload", offset_delta=delta
        )
        self.assertEqual(instance.payload, "ordinary payload")
        with self.assertRaisesRegex(TypeError, "exact int"):
            _testinternalcapi.soac_slot_view_set(
                payload, instance, "wrong", offset_delta=-delta
            )
        _testinternalcapi.soac_slot_view_set(payload, instance, 59, offset_delta=-delta)
        self.assertEqual(instance.value, 59)

    def test_each_strict_declaring_owner_checks_the_actual_inherited_slot(self):
        for base_checks in (False, True):
            with self.subTest(base_checks=base_checks):
                base = self.build(checked=("value",) if base_checks else ())
                derived = self.build(
                    bases=(base,),
                    slots=(),
                    checked=() if base_checks else ("value",),
                )
                instance = derived()
                instance.value = 61
                self.assertEqual(instance.value, 61)
                with self.assertRaisesRegex(TypeError, "exact int"):
                    instance.value = "wrong"
                self.assertEqual(instance.value, 61)

    def test_slot_plan_rejects_missing_or_duplicate_native_fields(self):
        for fields, slots in (
            (("missing",), ("value",)),
            (("value", "value"), ("value",)),
            (("__dict__",), ("__dict__",)),
        ):
            with self.subTest(fields=fields), self.assertRaises(TypeError):
                self.build(fields=fields, slots=slots)

    def test_ready_descriptor_publication_does_not_grant_reentrant_mapping_writes(self):
        callback_type = ctypes.CFUNCTYPE(
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
        )
        add = ctypes.pythonapi.PyDict_AddWatcher
        add.argtypes = [callback_type]
        add.restype = ctypes.c_int
        clear = ctypes.pythonapi.PyDict_ClearWatcher
        clear.argtypes = [ctypes.c_int]
        clear.restype = ctypes.c_int
        observations, errors = [], []

        @callback_type
        def on_event(event, dictionary, key, value):
            if (
                event != 0
                or not key
                or ctypes.cast(key, ctypes.py_object).value != "value"
            ):
                return 0
            try:
                namespace = ctypes.cast(dictionary, ctypes.py_object).value
                descriptor = ctypes.cast(value, ctypes.py_object).value
                actual = descriptor.__objclass__
                for write in (
                    lambda: namespace.__setitem__("value", "wrong"),
                    lambda: setattr(actual, "value", "wrong"),
                ):
                    try:
                        write()
                    except TypeError:
                        observations.append("rejected")
                    else:
                        errors.append("reentrant slot write succeeded")
            except BaseException as error:  # noqa: BLE001 - exceptions cannot escape a C callback
                errors.append(error)
            return 0

        identity = add(on_event)
        self.assertGreaterEqual(identity, 0)
        try:
            actual = self.build(watcher=identity)
        finally:
            clear(identity)
        self.assertEqual(errors, [])
        self.assertEqual(observations, ["rejected", "rejected"])
        instance = actual()
        instance.value = 67
        self.assertEqual(instance.value, 67)
        with self.assertRaisesRegex(TypeError, "exact int"):
            instance.value = "wrong"

    def test_declared_slot_offset_is_bound_to_the_actual_mro_layout_and_owner(self):
        actual = self.build(fields=("value", "payload"), checked=("value",))
        getter = ctypes.pythonapi.PyType_GetSoacObjectSlotOffset
        getter.argtypes = [
            ctypes.py_object,
            ctypes.c_void_p,
            ctypes.c_ssize_t,
            ctypes.POINTER(ctypes.c_ssize_t),
        ]
        getter.restype = ctypes.c_int
        matches = ctypes.pythonapi.PyType_MatchesSoacObjectSlotDescriptor
        matches.argtypes = [
            ctypes.py_object,
            ctypes.c_void_p,
            ctypes.c_ssize_t,
            ctypes.py_object,
        ]
        matches.restype = ctypes.c_int
        owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        offset = ctypes.c_ssize_t(-1)
        for index, name in enumerate(("value", "payload")):
            self.assertEqual(
                getter(actual, owner(actual), index, ctypes.byref(offset)), 1
            )
            definition = _testinternalcapi.soac_slot_definition(actual.__dict__[name])
            self.assertEqual(offset.value, definition[1])
            self.assertEqual(
                matches(actual, owner(actual), index, actual.__dict__[name]), 1
            )
        self.assertEqual(getter(actual, 1, 0, ctypes.byref(offset)), 0)
        ordinary = type("Ordinary", (actual,), {})
        self.assertEqual(getter(ordinary, owner(actual), 0, ctypes.byref(offset)), 1)
        self.assertEqual(
            matches(ordinary, owner(actual), 0, actual.__dict__["value"]), 1
        )
        self.assertEqual(
            offset.value,
            _testinternalcapi.soac_slot_definition(actual.__dict__["value"])[1],
        )
        unrelated = type("Unrelated", (), {"__slots__": ("value", "payload")})
        self.assertEqual(getter(unrelated, owner(actual), 0, ctypes.byref(offset)), 0)
        self.assertEqual(
            matches(actual, owner(actual), 0, unrelated.__dict__["value"]), 0
        )
        self.assertEqual(
            matches(actual, owner(actual), 0, actual.__dict__["payload"]), 0
        )
        shadow = type("ShadowingSlot", (actual,), {"__slots__": ("value",)})
        self.assertEqual(matches(shadow, owner(actual), 0, shadow.__dict__["value"]), 0)
        self.assertEqual(matches(shadow, owner(actual), 0, actual.__dict__["value"]), 1)
        with self.assertRaises(IndexError):
            getter(actual, owner(actual), 2, ctypes.byref(offset))

    def test_slot_overwrite_delete_and_cycles_preserve_normal_finalizer_order(self):
        events = []
        actual = self.build(
            fields=("value", "payload"),
            checked=("value",),
            slots=("value", "payload", "__weakref__"),
        )
        instance = actual()
        reference = weakref.ref(instance)

        class Previous:
            def __del__(self):
                current = reference()
                events.append(getattr(current, "payload", "missing"))

        instance.payload = Previous()
        instance.payload = "new"
        self.assertEqual(events, ["new"])
        instance.payload = Previous()
        del instance.payload
        self.assertEqual(events, ["new", "missing"])
        instance.payload = instance
        type_reference = weakref.ref(actual)
        del instance, actual
        gc.collect()
        self.assertIsNone(reference())
        self.assertIsNone(type_reference())


class DataclassSlotReplacementNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        error = ctypes.pythonapi.PySoac_GetStrictRuntimeUnavailableError
        error.argtypes = []
        error.restype = ctypes.c_void_p
        cls.runtime_error = ctypes.cast(error(), ctypes.py_object).value

    @staticmethod
    def api(name, result, *arguments):
        function = getattr(ctypes.pythonapi, name)
        function.restype = result
        function.argtypes = arguments
        return function

    def original(self, *, unsealed=False):
        namespace_function = lambda namespace, cell: None
        reference = weakref.ref(namespace_function)
        if unsealed:
            spec = ConstructionSpec(
                       4,
                       ctypes.sizeof(ConstructionSpec),
                       0,
                       0,
                       object(),
                       namespace_function,
                       "Original",
                       (),
                       {"__module__": __name__},
                       {},
                       None,
                       None,
                       TypeContractSpecV4(flags=0, fields=(), protected_names=(), final_methods=(), check_instance_write=None, new_instance_dict=None),
                   )
            new_handle = self.api(
                "PyType_NewSoacConstructionHandle",
                ctypes.py_object,
                ctypes.POINTER(ConstructionSpec),
            )
            construct = self.api(
                "PyType_FromSoacConstructionHandle",
                ctypes.py_object,
                ctypes.py_object,
                ctypes.py_object,
            )
            handle = new_handle(ctypes.byref(spec))
            actual = construct(handle, namespace_function)
            del handle, spec
        else:
            actual = _testinternalcapi.dict_new_soac_type(
                "Original",
                (),
                {"__module__": __name__},
                ("value",),
                namespace_function,
            )
        del namespace_function
        self.assertIsNone(reference())
        return actual

    def fixture(
        self,
        original,
        *,
        members=None,
        mode=0,
        maker=None,
        root=None,
        extra_arguments=(),
        creations=(),
        native_members=(),
    ):
        import _types

        helper = _types._dataclass_new_slots
        name = original.__name__
        bases = original.__bases__
        namespace = {
            "__module__": __name__,
            "__slots__": ("value",),
            **(members or {}),
        }

        def replace(cls, creator, name, bases, namespace):
            return creator(type(cls), name, bases, namespace, cls)

        function = replace if root is None else root
        call = [
            operation.offset // 2
            for operation in dis.get_instructions(function)
            if operation.opname == "CALL"
        ][-1]
        creator = helper if maker is None else maker
        arguments = (original, creator, name, bases, namespace, *extra_arguments)
        handles = [None]
        observations = [False] * (4 if mode == 3 else 2)
        invocation = _testinternalcapi.soac_dataclass_fixture(
            function,
            tuple(enumerate(arguments)),
            (),
            creations,
            (
                "bridges-v1",
                ((call, 8, (type, name, bases, namespace, original)),),
                None,
                [None],
            ),
            native_members,
            (),
            (("value",), ("value",), observations, handles, None, mode),
        )
        owner = self.api(
            "PyType_GetSoacContractOwner", ctypes.c_void_p, ctypes.py_object
        )
        bind = self.api(
            "PySoac_DataclassBindClass",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
            ctypes.c_void_p,
        )
        expected_owner = (
            self.pending_info(original).owner if mode == 3 else owner(original)
        )
        self.assertEqual(bind(invocation, original, expected_owner), 0)
        return invocation, function, arguments, observations, handles

    def frozen_fixture(self, *, copy_kind=None, pending=False):
        if pending:
            original, original_owner, pending_root = self.pending_original()
        else:
            original = self.original(unsealed=True)
        invocation_slot = []
        installed = []
        set_member = self.api(
            "PyType_SetSoacDataclassMember",
            ctypes.c_int,
            *([ctypes.py_object] * 4),
        )

        def install(actual, namespace, setter, deleter):
            for name, function in (("__setattr__", setter), ("__delattr__", deleter)):
                set_member(invocation_slot[0], actual, name, function)
                installed.append(function)
                if copy_kind == "clone":
                    function = types.FunctionType(
                        function.__code__, function.__globals__
                    )
                elif copy_kind == "plain":
                    function = (
                        object.__setattr__
                        if name == "__setattr__"
                        else object.__delattr__
                    )
                namespace[name] = function

        codes = [
            constant
            for constant in replace_frozen.__code__.co_consts
            if isinstance(constant, types.CodeType)
        ]
        offsets = [
            operation.offset // 2
            for operation in dis.get_instructions(replace_frozen)
            if operation.opname == "MAKE_FUNCTION"
        ]
        creations = tuple(
            (replace_frozen, offset, code, role, ())
            for offset, code, role in zip(offsets, codes, (2, 3), strict=True)
        )
        native_members = tuple(
            (name, role, code, replace_frozen.__globals__, None, None)
            for name, role, code in zip(
                ("__setattr__", "__delattr__"),
                (2, 3),
                codes,
                strict=True,
            )
        )
        result = self.fixture(
            original,
            mode=3 if pending else 0,
            root=replace_frozen,
            extra_arguments=(install,),
            creations=creations,
            native_members=native_members,
        )
        invocation_slot.append(result[0])
        if pending:
            return original, result, installed, pending_root
        return original, result, installed

    @staticmethod
    def call(invocation, function, arguments):
        return _testinternalcapi.soac_dataclass_fixture_call(
            invocation, 2, function, arguments, {}
        )

    def complete(self, invocation):
        self.api("PySoac_CompleteDataclassInvocation", ctypes.c_int, ctypes.py_object)(
            invocation
        )

    def matches_replacement(self, invocation, actual, owner=None):
        if owner is None:
            owner = self.api(
                "PyType_GetSoacContractOwner",
                ctypes.c_void_p,
                ctypes.py_object,
            )(actual)
        return self.api(
            "PySoac_DataclassMatchesSlotsClass",
            ctypes.c_int,
            ctypes.py_object,
            ctypes.py_object,
            ctypes.c_void_p,
        )(invocation, actual, owner)

    def test_public_bridge_call_keeps_ordinary_metaclass_behavior(self):
        import _types

        events = []

        class Meta(type):
            def __new__(mcls, name, bases, namespace):
                events.append((name, bases, namespace))
                return super().__new__(mcls, name, bases, namespace)

        namespace = {"__slots__": ("value",)}
        actual = _types._dataclass_new_slots(Meta, "Ordinary", (), namespace, object)
        self.assertEqual(events, [("Ordinary", (), namespace)])
        instance = actual()
        instance.value = "ordinary"
        has = self.api("PyType_HasSoacContract", ctypes.c_int, ctypes.py_object)
        self.assertEqual(has(actual), 0)

    def test_linked_replacement_has_independent_native_storage_before_callbacks(self):
        original = self.original()
        observations = []
        invocations = []
        case = self

        class Descriptor:
            def __set_name__(self, actual, name):
                case.assertEqual(case.matches_replacement(invocations[0], actual), 1)
                instance = actual()
                instance.value = 71
                with case.assertRaisesRegex(TypeError, "exact int"):
                    instance.value = "wrong"
                observations.append(instance.value)

        invocation, function, arguments, bound, handles = self.fixture(
            original, members={"hook": Descriptor()}
        )
        invocations.append(invocation)
        replacement = self.call(invocation, function, arguments)
        self.assertIsNot(replacement, original)
        self.assertEqual(observations, [71])
        self.assertEqual(bound, [True, True])
        self.assertIsNotNone(handles[0])
        owner = self.api(
            "PyType_GetSoacContractOwner", ctypes.c_void_p, ctypes.py_object
        )
        self.assertNotEqual(owner(original), owner(replacement))
        self.assertEqual(self.matches_replacement(invocation, replacement), 1)
        self.assertEqual(self.matches_replacement(invocation, original), 0)
        self.assertEqual(
            self.matches_replacement(invocation, replacement, owner(original)), 0
        )
        first, second = original(), replacement()
        first.value, second.value = 73, 79
        self.assertEqual(vars(first), {"value": 73})
        with self.assertRaises(TypeError):
            vars(second)
        with self.assertRaises(TypeError):
            first.value = "wrong"
        with self.assertRaisesRegex(TypeError, "exact int"):
            second.value = "wrong"
        self.complete(invocation)
        self.assertEqual((first.value, second.value), (73, 79))
        with self.assertRaises(self.runtime_error):
            self.matches_replacement(invocation, replacement)

    def test_replacement_copies_only_its_already_installed_frozen_role_functions(self):
        original, (invocation, function, arguments, observations, _), installed = (
            self.frozen_fixture()
        )
        replacement = self.call(invocation, function, arguments)
        self.assertEqual(observations, [True, True])
        self.assertIs(replacement.__dict__["__setattr__"], installed[0])
        self.assertIs(replacement.__dict__["__delattr__"], installed[1])
        for actual in (original, replacement):
            with self.assertRaisesRegex(AttributeError, "generated frozen setter"):
                actual().value = 1
            with self.assertRaisesRegex(AttributeError, "generated frozen deleter"):
                del actual().value
        instance = replacement()
        object.__setattr__(instance, "value", 113)
        self.assertEqual(instance.value, 113)
        with self.assertRaisesRegex(TypeError, "exact int"):
            object.__setattr__(instance, "value", "wrong")
        self.complete(invocation)
        self.assertEqual(instance.value, 113)

    def test_pending_gc_cannot_run_between_replacement_handle_and_owner_binding(self):
        original = self.original()

        class Descriptor:
            def __set_name__(self, actual, name):
                # A real Python callback during construction is safe only
                # after both the native association and owner binding.
                gc.collect()

        invocation, function, arguments, bound, handles = self.fixture(
            original,
            members={"hook": Descriptor()},
        )
        observations = []
        violations = []

        def observe(phase, information):
            if handles[0] is not None:
                observations.append(phase)
                if bound != [True, True]:
                    violations.append(phase)

        thresholds = gc.get_threshold()
        enabled = gc.isenabled()
        gc.callbacks.append(observe)
        try:
            gc.enable()
            gc.set_threshold(1, 1, 1)
            replacement = self.call(invocation, function, arguments)
            gc.collect()
        finally:
            gc.callbacks.remove(observe)
            gc.set_threshold(*thresholds)
            if not enabled:
                gc.disable()
        self.assertTrue(observations)
        self.assertEqual(violations, [])
        self.assertEqual(self.matches_replacement(invocation, replacement), 1)
        self.complete(invocation)

    def test_copied_or_plain_frozen_hooks_never_acquire_replacement_authority(self):
        for copy_kind in ("clone", "plain"):
            with self.subTest(copy_kind=copy_kind):
                (
                    original,
                    (invocation, function, arguments, observations, _),
                    installed,
                ) = self.frozen_fixture(copy_kind=copy_kind)
                with self.assertRaises(self.runtime_error):
                    self.call(invocation, function, arguments)
                self.assertEqual(observations, [False, False])
                self.assertIs(original.__dict__["__setattr__"], installed[0])
                with self.assertRaisesRegex(AttributeError, "generated frozen setter"):
                    original().value = 1

    def test_frozen_hooks_from_another_invocation_cannot_be_copied(self):
        original, (invocation, function, arguments, _, _), installed = (
            self.frozen_fixture()
        )
        replacement = self.call(invocation, function, arguments)
        self.complete(invocation)
        next_invocation, function, arguments, observations, _ = self.fixture(
            original,
            members=dict(zip(("__setattr__", "__delattr__"), installed, strict=True)),
        )
        with self.assertRaises(self.runtime_error):
            self.call(next_invocation, function, arguments)
        self.assertEqual(observations, [False, False])
        for actual in (original, replacement):
            with self.assertRaisesRegex(AttributeError, "generated frozen setter"):
                actual().value = 1

    def test_failed_ready_callback_expires_replacement_association_without_revocation(
        self,
    ):
        original = self.original()
        escaped = []
        invocations = []
        case = self

        class Descriptor:
            def __set_name__(self, actual, name):
                case.assertEqual(case.matches_replacement(invocations[0], actual), 1)
                escaped.append(actual)
                raise ValueError("replacement callback failed")

        invocation, function, arguments, _, _ = self.fixture(
            original,
            members={"hook": Descriptor()},
        )
        invocations.append(invocation)
        with self.assertRaisesRegex(ValueError, "replacement callback failed"):
            self.call(invocation, function, arguments)
        self.assertEqual(len(escaped), 1)
        with self.assertRaises(self.runtime_error):
            self.matches_replacement(invocation, escaped[0])
        with self.assertRaises(TypeError):
            escaped[0].__getattribute__ = object.__getattribute__
        instance = original()
        instance.value = 127
        self.assertEqual(instance.value, 127)

    def test_one_replacement_handle_cannot_be_minted_twice_or_use_the_original_owner(
        self,
    ):
        for mode in (1, 2):
            with self.subTest(mode=mode):
                original = self.original()
                invocation, function, arguments, observations, _ = self.fixture(
                    original, mode=mode
                )
                with self.assertRaises(self.runtime_error):
                    self.call(invocation, function, arguments)
                self.assertEqual(observations, [False, False])
                with self.assertRaises(self.runtime_error):
                    self.call(invocation, function, arguments)
                instance = original()
                instance.value = 83
                self.assertEqual(instance.value, 83)

    def test_public_source_handle_consumer_never_accepts_replacement_mode(self):
        original = self.original()
        invocation, function, arguments, _, handles = self.fixture(original)
        replacement = self.call(invocation, function, arguments)
        consume = self.api(
            "PyType_FromSoacConstructionHandle",
            ctypes.py_object,
            ctypes.py_object,
            ctypes.py_object,
        )
        with self.assertRaises(TypeError):
            consume(handles[0], None)
        self.complete(invocation)
        instance = replacement()
        instance.value = 89
        self.assertEqual(instance.value, 89)

    def test_c_forwarding_cannot_inherit_replacement_authority(self):
        import _types

        escaped = []

        def proxy(*arguments):
            result = _testinternalcapi.soac_dataclass_fixture_c_forward(
                _types._dataclass_new_slots, *arguments
            )
            escaped.append(result)
            return result

        original = self.original()
        invocation, function, arguments, observations, handles = self.fixture(
            original, maker=proxy
        )
        with self.assertRaises(self.runtime_error):
            self.call(invocation, function, arguments)
        self.assertEqual(observations, [False, False])
        self.assertEqual(handles, [None])
        self.assertEqual(len(escaped), 1)
        has = self.api("PyType_HasSoacContract", ctypes.c_int, ctypes.py_object)
        self.assertEqual(has(escaped[0]), 0)
        instance = escaped[0]()
        instance.value = "ordinary"

    def test_completed_replacement_and_escaped_consumed_handle_do_not_retain_original(
        self,
    ):
        original = self.original()
        reference = weakref.ref(original)
        invocation, function, arguments, _, handles = self.fixture(original)
        replacement = self.call(invocation, function, arguments)
        self.complete(invocation)
        del arguments, original, function
        gc.collect()
        self.assertIsNone(reference())
        self.assertIsNotNone(handles[0])
        instance = replacement()
        instance.value = 97
        self.assertEqual(instance.value, 97)


    def pending_info(self, actual):
        result = ConstructionInfoV1()
        query = self.api(
            "PyType_GetSoacConstructionInfoV1", ctypes.c_int,
            ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
        )
        self.assertEqual(query(actual, ctypes.byref(result), ctypes.sizeof(result)), 1)
        return result

    def pending_original(self, **namespace):
        owner = ([False] * 4, None, (), None)
        function = lambda: None
        actual, root = _testinternalcapi.soac_pending_type_construct(
            "PendingOriginal", (), {"__module__": __name__, **namespace}, function, owner,
        )
        return actual, owner, root

    def pending_admit(self, actual, root, *, slots=("value",), owner=None):
        if owner is None:
            info = self.pending_info(actual)
            owner = ctypes.cast(info.owner, ctypes.py_object).value
        _testinternalcapi.soac_pending_type_admit(
            actual, owner, root, (), (), (), slots,
        )
        return owner

    def test_pending_replacement_links_distinct_owner_before_ready_then_disposes_original(self):
        original, original_owner, root = self.pending_original()
        invocations, ready = [], []
        case = self
        mutation = self.api("PySoac_GetStrictMutationError", ctypes.c_void_p)()
        mutation = ctypes.cast(mutation, ctypes.py_object).value

        class Descriptor:
            def __set_name__(self, actual, name):
                info = case.pending_info(actual)
                case.assertEqual((info.phase, info.permanent_contract_published), (1, 0))
                case.assertEqual(info.root_construction, id(root))
                case.assertNotEqual(info.owner, id(original_owner))
                case.assertEqual(case.matches_replacement(invocations[0], actual, info.owner), 1)
                with case.assertRaises(mutation):
                    actual()
                ready.append(name)

        invocation, function, arguments, observations, handles = self.fixture(
            original, mode=3, members={"hook": Descriptor()},
        )
        invocations.append(invocation)
        replacement = self.call(invocation, function, arguments)
        self.assertEqual(ready, ["hook"])
        self.assertEqual(observations, [True, False, False, False])
        self.assertIsNot(replacement, original)
        self.assertIsNot(handles[0], root)
        self.complete(invocation)
        before_flags, before_size = replacement.__flags__, replacement.__basicsize__
        replacement_owner = self.pending_admit(replacement, root)
        self.assertEqual(observations, [True, True, True, True])
        self.assertIsNot(replacement_owner, original_owner)
        self.assertEqual(replacement.__basicsize__, before_size)
        self.assertEqual(replacement.__flags__ & (1 << 2), before_flags & (1 << 2))
        instance = replacement()
        instance.value = 73
        with self.assertRaisesRegex(TypeError, "exact int"):
            instance.value = "wrong"
        with self.assertRaises(mutation):
            original()
        dispose = self.api(
            "PyType_DisposeSoacProvisionalV1", ctypes.c_int, *([ctypes.py_object] * 3),
        )
        self.assertEqual(dispose(original, original_owner, root), 0)
        self.assertEqual(self.pending_info(original).phase, 5)
        dynamic = original()
        dynamic.value = "ordinary provisional"
        self.assertEqual(dynamic.value, "ordinary provisional")
        with self.assertRaises(mutation):
            dispose(replacement, replacement_owner, root)

    def test_pending_replacement_ready_error_terminalizes_whole_lineage_before_release(self):
        original, owner, root = self.pending_original()
        escaped, replacement_owners = [], []
        marker = LookupError("replacement readiness")
        case = self

        class Descriptor:
            def __init__(self, observer, error):
                self.observer = observer
                self.error = error

            def __set_name__(self, actual, name):
                self.observer(actual)
                raise self.error

        # The pinned ordinary type pipeline preserves the original exception
        # and adds a note; it does not wrap __set_name__ in RuntimeError.
        ordinary_error = LookupError("replacement readiness")
        with self.assertRaises(LookupError) as ordinary:
            type(original.__name__, original.__bases__, {
                "__module__": __name__,
                "__slots__": ("value",),
                "hook": Descriptor(lambda actual: None, ordinary_error),
            })
        self.assertIs(ordinary.exception, ordinary_error)

        def observe_pending(actual):
            replacement_owners.append(
                ctypes.cast(case.pending_info(actual).owner, ctypes.py_object).value
            )
            escaped.append(actual)

        invocation, function, arguments, _, _ = self.fixture(
            original, mode=3, members={"hook": Descriptor(observe_pending, marker)},
        )
        with self.assertRaises(LookupError) as raised:
            self.call(invocation, function, arguments)
        self.assertIs(raised.exception, marker)
        self.assertEqual(raised.exception.__notes__, ordinary.exception.__notes__)
        self.assertIs(raised.exception.__cause__, ordinary.exception.__cause__)
        self.assertIs(raised.exception.__context__, ordinary.exception.__context__)
        self.assertEqual(len(escaped), 1)
        mutation = ctypes.cast(
            self.api("PySoac_GetStrictMutationError", ctypes.c_void_p)(), ctypes.py_object,
        ).value
        for actual in (original, escaped[0]):
            self.assertEqual(self.pending_info(actual).phase, 4)
            with self.assertRaises(mutation):
                actual()
        with self.assertRaises(mutation):
            self.pending_admit(escaped[0], root, owner=replacement_owners[0])

    def test_pending_replacement_can_admit_after_original_dies_without_a_hidden_type_pin(self):
        original, owner, root = self.pending_original()
        reference = weakref.ref(original)
        invocation, function, arguments, _, handles = self.fixture(original, mode=3)
        replacement = self.call(invocation, function, arguments)
        self.complete(invocation)
        del arguments, original, function, invocation, owner
        gc.collect()
        self.assertIsNone(reference())
        self.assertIsNotNone(handles[0])
        self.assertEqual(self.pending_info(replacement).root_construction, id(root))
        self.pending_admit(replacement, root)
        instance = replacement()
        instance.value = 79
        self.assertEqual(instance.value, 79)

    def test_pending_root_failure_still_blocks_replacement_after_original_dies(self):
        original, owner, root = self.pending_original()
        reference = weakref.ref(original)
        invocation, function, arguments, _, handles = self.fixture(original, mode=3)
        replacement = self.call(invocation, function, arguments)
        self.complete(invocation)
        del arguments, original, function, invocation, owner
        gc.collect()
        self.assertIsNone(reference())
        replacement_owner = ctypes.cast(
            self.pending_info(replacement).owner, ctypes.py_object,
        ).value
        fail = self.api("PyType_FailSoacPendingV1", ctypes.c_int, ctypes.py_object)
        self.assertEqual(fail(root), 0)
        self.assertEqual(self.pending_info(replacement).phase, 4)
        mutation = ctypes.cast(
            self.api("PySoac_GetStrictMutationError", ctypes.c_void_p)(), ctypes.py_object,
        ).value
        with self.assertRaises(mutation):
            replacement()
        with self.assertRaises(mutation):
            self.pending_admit(replacement, root, owner=replacement_owner)
        self.assertIsNotNone(handles[0])

    def test_pending_replacement_cannot_link_to_a_permanent_original(self):
        original = self.original()
        invocation, function, arguments, observations, _ = self.fixture(original, mode=3)
        mutation = ctypes.cast(
            self.api("PySoac_GetStrictMutationError", ctypes.c_void_p)(), ctypes.py_object,
        ).value
        with self.assertRaises(mutation):
            self.call(invocation, function, arguments)
        self.assertEqual(observations, [False] * 4)
        # An invalid replacement never revokes the already installed original.
        instance = original()
        instance.value = 83
        self.assertEqual(instance.value, 83)


    def test_pending_slots_copy_keeps_native_frozen_birth_after_original_completion(self):
        original, (invocation, function, arguments, observations, _), installed, root = (
            self.frozen_fixture(pending=True)
        )
        replacement = self.call(invocation, function, arguments)
        self.complete(invocation)
        self.assertEqual(observations, [True, False, False, False])
        self.assertIs(replacement.__dict__["__setattr__"], installed[0])
        self.assertIs(replacement.__dict__["__delattr__"], installed[1])
        self.pending_admit(replacement, root)
        instance = replacement()
        object.__setattr__(instance, "value", 47)
        self.assertEqual(instance.value, 47)
        with self.assertRaisesRegex(AttributeError, "generated frozen setter"):
            instance.value = 49
        with self.assertRaisesRegex(AttributeError, "generated frozen deleter"):
            del instance.value

    def test_pending_slots_copy_rejects_an_ambiguous_stored_hook_without_equality(self):
        original, original_owner, root_handle = self.pending_original()
        invocation_slot, snapshots, functions, equalities = [], [], [], []
        set_member = self.api(
            "PyType_SetSoacDataclassMember", ctypes.c_int, *([ctypes.py_object] * 4),
        )
        get_dict = self.api("PyType_GetDict", ctypes.py_object, ctypes.py_object)
        namespace = get_dict(original)
        armed = [False]

        class Collision:
            def __hash__(self):
                return hash("__setattr__")

            def __eq__(self, other):
                if armed[0]:
                    equalities.append(other)
                    raise AssertionError("pending hook scan invoked equality")
                return False

        collision, sentinel = Collision(), object()

        def install(actual, copied_namespace, setter, deleter):
            for name, function in (("__setattr__", setter), ("__delattr__", deleter)):
                set_member(invocation_slot[0], actual, name, function)
                functions.append(weakref.ref(function))
                copied_namespace[name] = function
            namespace[collision] = sentinel
            snapshots.append(tuple((id(k), id(v)) for k, v in namespace.items()))
            armed[0] = True

        codes = [
            constant for constant in replace_frozen.__code__.co_consts
            if isinstance(constant, types.CodeType)
        ]
        offsets = [
            operation.offset // 2 for operation in dis.get_instructions(replace_frozen)
            if operation.opname == "MAKE_FUNCTION"
        ]
        creations = tuple(
            (replace_frozen, offset, code, role, ())
            for offset, code, role in zip(offsets, codes, (2, 3), strict=True)
        )
        members = tuple(
            (name, role, code, replace_frozen.__globals__, None, None)
            for name, role, code in zip(
                ("__setattr__", "__delattr__"), (2, 3), codes, strict=True,
            )
        )
        invocation, function, arguments, observations, _ = self.fixture(
            original, mode=3, root=replace_frozen, extra_arguments=(install,),
            creations=creations, native_members=members,
        )
        invocation_slot.append(invocation)
        with self.assertRaises(self.runtime_error):
            self.call(invocation, function, arguments)
        self.assertEqual(equalities, [])
        self.assertEqual(
            tuple((id(k), id(v)) for k, v in namespace.items()), snapshots[0],
        )
        self.assertTrue(all(reference() is not None for reference in functions))
        self.assertEqual(self.pending_info(original).phase, 4)
        # No pre-set PyErr is injected at the private inner scan. Public
        # query/failure primary-error controls remain a separate boundary.


if __name__ == "__main__":
    unittest.main()
