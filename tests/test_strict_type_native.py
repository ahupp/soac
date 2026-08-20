"""Native construction barriers; the fixture is not an artifact authority.

Run with the selected vendored CPython. The production loader, not this
ctypes test fixture, is responsible for authenticating compiler plans.
"""

import ctypes
import gc
import types
import unittest
import weakref


class TypeContractSpecV4(ctypes.Structure):
    _fields_ = [
        ("flags", ctypes.c_uint32),
        ("dictionary_mode", ctypes.c_uint32),
        ("fields", ctypes.py_object),
        ("protected_names", ctypes.py_object),
        ("final_methods", ctypes.py_object),
        ("object_slot_fields", ctypes.py_object),
        ("check_instance_write", ctypes.c_void_p),
        ("new_instance_dict", ctypes.c_void_p),
        ("prepare_instance_dictionary_policy", ctypes.c_void_p),
    ]


class ConstructionSpec(ctypes.Structure):
    _fields_ = [
        ("abi_version", ctypes.c_uint32),
        ("struct_size", ctypes.c_uint32),
        ("construction_mode", ctypes.c_uint32),
        ("reserved", ctypes.c_uint32),
        ("owner", ctypes.py_object),
        ("namespace_function", ctypes.py_object),
        ("name", ctypes.py_object),
        ("bases", ctypes.py_object),
        ("namespace_dict", ctypes.py_object),
        ("keywords", ctypes.py_object),
        ("bind_type", ctypes.c_void_p),
        ("commit_final", ctypes.c_void_p),
        ("contract", TypeContractSpecV4),
    ]


class ConstructionInfoV1(ctypes.Structure):
    _fields_ = [
        ("abi_version", ctypes.c_uint32),
        ("struct_size", ctypes.c_uint32),
        ("phase", ctypes.c_uint32),
        ("permanent_contract_published", ctypes.c_uint32),
        ("owner", ctypes.c_void_p),
        ("root_construction", ctypes.c_void_p),
    ]


class TypeSlot(ctypes.Structure):
    _fields_ = [("slot", ctypes.c_int), ("value", ctypes.c_void_p)]


class TypeSpec(ctypes.Structure):
    _fields_ = [
        ("name", ctypes.c_char_p),
        ("basicsize", ctypes.c_int),
        ("itemsize", ctypes.c_int),
        ("flags", ctypes.c_uint),
        ("slots", ctypes.POINTER(TypeSlot)),
    ]


class StrictDescriptorNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.static_function = ctypes.pythonapi._PySoac_StaticMethodFunction
        cls.static_function.argtypes = [ctypes.py_object]
        cls.static_function.restype = ctypes.c_void_p
        cls.class_function = ctypes.pythonapi._PySoac_ClassMethodFunction
        cls.class_function.argtypes = [ctypes.py_object]
        cls.class_function.restype = ctypes.c_void_p
        cls.property_function = ctypes.pythonapi._PySoac_PropertyFunction
        cls.property_function.argtypes = [ctypes.py_object, ctypes.c_int]
        cls.property_function.restype = ctypes.c_void_p
        cls.is_sealed = ctypes.pythonapi._PySoac_IsDescriptorSealed
        cls.is_sealed.argtypes = [ctypes.py_object]
        cls.is_sealed.restype = ctypes.c_int
        cls.seal_static = ctypes.pythonapi._PySoac_SealStaticMethod
        cls.seal_static.argtypes = [ctypes.py_object, ctypes.py_object]
        cls.seal_static.restype = ctypes.c_int
        cls.seal_class = ctypes.pythonapi._PySoac_SealClassMethod
        cls.seal_class.argtypes = [ctypes.py_object, ctypes.py_object]
        cls.seal_class.restype = ctypes.c_int
        cls.seal_property = ctypes.pythonapi._PySoac_SealProperty
        cls.seal_property.argtypes = [ctypes.py_object] * 4
        cls.seal_property.restype = ctypes.c_int
        cls.allocate = ctypes.pythonapi.PyType_GenericAlloc
        cls.allocate.argtypes = [ctypes.py_object, ctypes.c_ssize_t]
        cls.allocate.restype = ctypes.py_object
        error = ctypes.pythonapi.PySoac_GetStrictMutationError
        error.argtypes = []
        error.restype = ctypes.c_void_p
        cls.mutation_error = ctypes.cast(error(), ctypes.py_object).value

    def test_exact_method_descriptors_return_the_actual_borrowed_callable(self):
        function = lambda: None
        for descriptor_type, getter in (
            (staticmethod, self.static_function),
            (classmethod, self.class_function),
        ):
            descriptor = descriptor_type(function)
            self.assertEqual(getter(descriptor), id(function))
            with self.assertRaisesRegex(TypeError, "uninitialized"):
                getter(self.allocate(descriptor_type, 0))
            with self.assertRaises(TypeError):
                getter(function)

            class Subclass(descriptor_type):
                def __getattribute__(self, name):
                    raise AssertionError(
                        "native descriptor reads must not invoke Python"
                    )

            subclass = Subclass(function)
            with self.assertRaisesRegex(TypeError, "exact"):
                # Bypass ctypes' own from_param lookup on the user object.
                getter(ctypes.py_object(subclass))
            self.assertEqual(self.is_sealed(ctypes.py_object(subclass)), 0)

    def test_layout_descriptor_matches_only_exact_name_and_interpreter_cache(self):
        predicate = ctypes.pythonapi._PySoac_IsLayoutDescriptor
        predicate.argtypes = [ctypes.py_object, ctypes.py_object]
        predicate.restype = ctypes.c_int

        class First:
            pass

        class Second:
            pass

        for name, other_name in (
            ("__dict__", "__weakref__"),
            ("__weakref__", "__dict__"),
        ):
            descriptor = First.__dict__[name]
            self.assertIs(descriptor, Second.__dict__[name])
            self.assertIs(descriptor.__objclass__, object)
            self.assertEqual(predicate(name, descriptor), 1)
            self.assertEqual(predicate(other_name, descriptor), 0)
            self.assertEqual(predicate(name, object.__dict__["__class__"]), 0)
            self.assertEqual(predicate(name, property()), 0)
            self.assertEqual(predicate(name, None), 0)
        self.assertEqual(predicate("__class__", object.__dict__["__class__"]), 0)
        self.assertEqual(predicate(None, First.__dict__["__dict__"]), 0)

    def test_layout_descriptor_predicate_never_invokes_operand_hooks(self):
        predicate = ctypes.pythonapi._PySoac_IsLayoutDescriptor
        predicate.argtypes = [ctypes.py_object, ctypes.py_object]
        predicate.restype = ctypes.c_int

        class Hostile:
            def __getattribute__(self, name):
                raise AssertionError("descriptor predicate called attribute lookup")

            def __eq__(self, other):
                raise AssertionError("descriptor predicate called equality")

        class Name(str):
            def __hash__(self):
                raise AssertionError("descriptor predicate called hash")

            def __eq__(self, other):
                raise AssertionError("descriptor predicate called equality")

        class Receiver:
            pass

        self.assertEqual(predicate("__dict__", ctypes.py_object(Hostile())), 0)
        self.assertEqual(predicate(Name("__dict__"), Receiver.__dict__["__dict__"]), 0)
        exact_name = "".join(("__", "dict", "__"))  # noqa: FLY002 -- distinct equal string
        self.assertEqual(predicate(exact_name, Receiver.__dict__["__dict__"]), 1)

    def test_property_slots_are_borrowed_and_missing_slots_are_none(self):
        get = lambda self: None
        set = lambda self, value: None
        delete = lambda self: None
        descriptor = property(get, set, delete)
        for slot, function in enumerate((get, set, delete)):
            self.assertEqual(self.property_function(descriptor, slot), id(function))
            self.assertEqual(self.property_function(property(), slot), id(None))
        for slot in (-1, 3):
            with self.assertRaises(ValueError):
                self.property_function(descriptor, slot)

        class Subclass(property):
            def __getattribute__(self, name):
                raise AssertionError("native property reads must not invoke Python")

        subclass = Subclass(get)
        with self.assertRaisesRegex(TypeError, "exact"):
            self.property_function(ctypes.py_object(subclass), 0)
        self.assertEqual(self.is_sealed(ctypes.py_object(subclass)), 0)

    def test_method_seals_compare_identity_and_reject_even_identical_reinitialization(
        self,
    ):
        original = lambda: 1
        replacement = lambda: 2
        for descriptor_type, seal in (
            (staticmethod, self.seal_static),
            (classmethod, self.seal_class),
        ):
            descriptor = descriptor_type(original)
            with self.assertRaises(self.mutation_error):
                seal(descriptor, replacement)
            self.assertEqual(self.is_sealed(descriptor), 0)
            descriptor.__init__(replacement)
            self.assertEqual(seal(descriptor, replacement), 0)
            self.assertEqual(seal(descriptor, replacement), 0)
            self.assertEqual(self.is_sealed(descriptor), 1)
            for value in (original, replacement):
                with self.assertRaises(self.mutation_error):
                    descriptor.__init__(value)
                self.assertIs(descriptor.__func__, replacement)

            class Subclass(descriptor_type):
                pass

            subclass = Subclass(original)
            with self.assertRaisesRegex(TypeError, "exact"):
                seal(subclass, original)
            self.assertEqual(self.is_sealed(subclass), 0)
            subclass.__init__(replacement)
            self.assertIs(subclass.__func__, replacement)

    def test_property_seal_is_atomic_and_new_accessor_copies_remain_mutable(self):
        get = lambda self: 1
        set = lambda self, value: None
        delete = lambda self: None
        replacement = lambda self: 2
        descriptor = property(get, set, delete)
        with self.assertRaises(self.mutation_error):
            self.seal_property(descriptor, get, set, None)
        self.assertEqual(self.is_sealed(descriptor), 0)
        descriptor.__init__(get, set)
        self.assertEqual(self.seal_property(descriptor, get, set, None), 0)
        self.assertEqual(self.seal_property(descriptor, get, set, None), 0)
        self.assertEqual(self.is_sealed(descriptor), 1)
        for arguments in ((get, set), (replacement, set, delete)):
            with self.assertRaises(self.mutation_error):
                descriptor.__init__(*arguments)
            self.assertEqual(
                (descriptor.fget, descriptor.fset, descriptor.fdel), (get, set, None)
            )

        copied = descriptor.getter(replacement)
        self.assertEqual(self.is_sealed(copied), 0)
        copied.__init__(get)
        self.assertIs(copied.fget, get)
        self.assertIsNone(copied.fset)
        self.assertEqual(self.is_sealed(object()), 0)
        self.assertEqual(self.is_sealed(get), 0)

        class Subclass(property):
            pass

        subclass = Subclass(get)
        with self.assertRaisesRegex(TypeError, "exact"):
            self.seal_property(subclass, get, None, None)
        self.assertEqual(self.is_sealed(subclass), 0)

    def test_sealed_initializers_run_no_wrapped_callbacks_and_ordinary_failures_are_unchanged(
        self,
    ):
        callbacks = []

        class BadCallable:
            def __call__(self, *args):
                return None

            def __getattribute__(self, name):
                if name in ("__module__", "__doc__"):
                    callbacks.append(name)
                    raise ValueError("wrapped attribute failed")
                return object.__getattribute__(self, name)

        original = lambda: None
        replacement = BadCallable()
        for descriptor_type, seal in (
            (staticmethod, self.seal_static),
            (classmethod, self.seal_class),
            (
                property,
                lambda descriptor, value: self.seal_property(
                    descriptor, value, None, None
                ),
            ),
        ):
            descriptor = descriptor_type(original)
            seal(descriptor, original)
            callbacks.clear()
            with self.assertRaises(self.mutation_error):
                descriptor.__init__(replacement)
            self.assertEqual(callbacks, [])
            ordinary = descriptor_type(original)
            with self.assertRaisesRegex(ValueError, "wrapped attribute failed"):
                ordinary.__init__(replacement)
            self.assertTrue(callbacks)
            actual = ordinary.fget if descriptor_type is property else ordinary.__func__
            self.assertIs(actual, replacement)
            self.assertEqual(self.is_sealed(ordinary), 0)

    def test_property_init_rechecks_seal_after_displaced_component_finalizers(self):
        for displaced in ("getter", "setter"):
            with self.subTest(displaced=displaced):
                observed = []
                holder = {}
                get_before = lambda self: 1
                set_before = lambda self, value: None
                del_before = lambda self: None
                get_after = lambda self: 2
                set_after = lambda self, value: None
                del_after = lambda self: None

                class Previous:
                    def __call__(self, *args):
                        return None

                    def __del__(previous, holder=holder, observed=observed):
                        descriptor = holder["descriptor"]
                        components = (
                            descriptor.fget,
                            descriptor.fset,
                            descriptor.fdel,
                        )
                        observed.append(components)
                        self.seal_property(descriptor, *components)

                descriptor = property(
                    Previous() if displaced == "getter" else get_before,
                    Previous() if displaced == "setter" else set_before,
                    del_before,
                )
                holder["descriptor"] = descriptor
                with self.assertRaises(self.mutation_error):
                    descriptor.__init__(get_after, set_after, del_after)
                expected = (
                    get_after,
                    set_before if displaced == "getter" else set_after,
                    del_before,
                )
                # The earlier write stays visible; only later writes fail.
                self.assertEqual(observed, [expected])
                self.assertEqual(
                    (descriptor.fget, descriptor.fset, descriptor.fdel), expected
                )
                self.assertEqual(self.is_sealed(descriptor), 1)

    def test_seals_preserve_callable_release_and_cycle_collection(self):
        released = []

        class Callable:
            def __init__(self, label):
                self.label = label

            def __call__(self, *args):
                return None

            def __del__(self):
                released.append(self.label)

        for descriptor_type, seal in (
            (staticmethod, self.seal_static),
            (classmethod, self.seal_class),
            (
                property,
                lambda descriptor, value: self.seal_property(
                    descriptor, value, None, None
                ),
            ),
        ):
            released.clear()
            original = Callable("original")
            replacement = Callable("replacement")
            original_ref = weakref.ref(original)
            descriptor = descriptor_type(original)
            seal(descriptor, original)
            del original
            with self.assertRaises(self.mutation_error):
                descriptor.__init__(replacement)
            del replacement
            self.assertEqual(released, ["replacement"])
            self.assertIsNotNone(original_ref())
            del descriptor
            self.assertEqual(released, ["replacement", "original"])

            target = Callable("cycle")
            target_ref = weakref.ref(target)
            descriptor = descriptor_type(target)
            seal(descriptor, target)
            target.descriptor = descriptor
            del target, descriptor
            gc.collect()
            self.assertIsNone(target_ref())
            self.assertEqual(released[-1], "cycle")


class StrictClassPreparationNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.prepare = ctypes.pythonapi.PySoac_PrepareClass
        cls.prepare.argtypes = [
            ctypes.py_object,
            ctypes.py_object,
            ctypes.py_object,
        ]
        cls.prepare.restype = ctypes.py_object
        cls.complete = ctypes.pythonapi.PySoac_CompleteClassNamespace
        cls.complete.argtypes = [ctypes.py_object, ctypes.py_object]
        cls.complete.restype = ctypes.c_int
        cls.finish = ctypes.pythonapi.PySoac_FinishClass
        cls.finish.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
        cls.finish.restype = ctypes.c_int

    def test_native_resolution_and_namespace_completion_preserve_callback_order(self):
        events = []

        class Namespace(dict):
            def __setitem__(self, key, value):
                events.append(("store", key))
                return super().__setitem__(key, value)

            def get(self, *args):
                raise AssertionError("class preparation must use mapping operations")

            def pop(self, *args):
                raise AssertionError("class completion must not pop a prepared mapping")

        class Base:
            pass

        class Alias:
            def __mro_entries__(self, original):
                events.append(("resolve", original is bases))
                return (Base,)

        class Meta(type):
            @classmethod
            def __prepare__(meta, name, resolved, **keywords):
                events.append(("prepare", name, resolved == (Base,), keywords))
                return Namespace(__classcell__="prepared value")

            def __new__(meta, name, resolved, namespace, **keywords):
                events.append(("construct", tuple(namespace), keywords))
                return super().__new__(meta, name, resolved, namespace)

        bases = (Alias(),)
        keywords = {"metaclass": Meta, "tag": 17}
        original_resolve = types.resolve_bases
        original_prepare = types.prepare_class

        def forbidden(*args, **kwargs):
            raise AssertionError("mutable types helpers must not be native authority")

        types.resolve_bases = types.prepare_class = forbidden
        try:
            preparation = self.prepare("Prepared", bases, keywords)
        finally:
            types.resolve_bases = original_resolve
            types.prepare_class = original_prepare
        meta, namespace, resolved, copied_keywords = preparation
        self.assertIs(meta, Meta)
        self.assertEqual(keywords, {"metaclass": Meta, "tag": 17})
        self.assertEqual(copied_keywords, {"tag": 17})
        self.assertEqual(namespace["__classcell__"], "prepared value")
        cell = types.CellType()
        namespace["__module__"] = __name__
        namespace["__qualname__"] = "Prepared"
        namespace["__firstlineno__"] = 7
        namespace["value"] = 23
        namespace["__orig_bases__"] = "body value"
        namespace["__classcell__"] = cell
        before_complete = len(events)
        self.assertEqual(self.complete(preparation, bases), 0)
        self.assertEqual(events[before_complete:], [("store", "__orig_bases__")])
        self.assertIs(namespace["__classcell__"], cell)
        self.assertIs(namespace["__orig_bases__"], bases)
        result = meta("Prepared", resolved, namespace, **copied_keywords)
        self.assertEqual(self.finish("Prepared", cell, result), 0)
        self.assertIs(cell.cell_contents, result)
        self.assertIs(namespace["__classcell__"], cell)
        self.assertNotIn("__classcell__", result.__dict__)
        self.assertEqual(
            events[:2], [("resolve", True), ("prepare", "Prepared", True, {"tag": 17})]
        )
        self.assertEqual(
            [event[1] for event in events if event[0] == "store"],
            [
                "__module__",
                "__qualname__",
                "__firstlineno__",
                "value",
                "__orig_bases__",
                "__classcell__",
                "__orig_bases__",
            ],
        )
        self.assertEqual(events[-1][0], "construct")

    def test_finish_checks_classcell_only_for_actual_type_results(self):
        marker = object()
        bases = ()
        preparation = self.prepare(
            "NonType", bases, {"metaclass": lambda *args: marker}
        )
        cell = types.CellType()
        preparation[1]["__classcell__"] = cell
        self.complete(preparation, bases)
        result = preparation[0]("NonType", preparation[2], preparation[1])
        self.assertIs(result, marker)
        self.assertEqual(self.finish("NonType", cell, result), 0)

        cell = types.CellType()
        result = type("MissingCell", bases, {})
        with self.assertRaisesRegex(RuntimeError, "__class__ not set"):
            self.finish("MissingCell", cell, result)
        cell.cell_contents = marker
        with self.assertRaisesRegex(TypeError, "__class__ set to"):
            self.finish("MissingCell", cell, result)
        self.assertEqual(self.finish("MissingCell", None, result), 0)

    def test_finish_validates_the_actual_returned_cell_not_the_prepared_mapping(self):
        preparation = self.prepare("ActualCell", (), {})
        mapped_cell = types.CellType()
        returned_cell = types.CellType()
        preparation[1]["__classcell__"] = mapped_cell
        result = type("ActualCell", (), preparation[1])
        self.assertIs(mapped_cell.cell_contents, result)
        with self.assertRaisesRegex(RuntimeError, "__class__ not set"):
            self.finish("ActualCell", returned_cell, result)
        returned_cell.cell_contents = result
        self.assertEqual(self.finish("ActualCell", returned_cell, result), 0)
        with self.assertRaisesRegex(TypeError, "None or an actual cell"):
            self.finish("ActualCell", preparation, result)
        with self.assertRaisesRegex(TypeError, "invalid native class preparation tuple"):
            self.complete((*preparation, returned_cell), ())

    def test_keyword_bindings_are_captured_before_base_resolution_like_class_statement(
        self,
    ):
        def exercise(native):
            observed = []
            original = object()

            class Meta(type):
                @classmethod
                def __prepare__(meta, name, bases, **received):
                    observed.append(received["tag"] is original)
                    return {}

                def __new__(meta, name, bases, namespace, **received):
                    return super().__new__(meta, name, bases, namespace)

            keywords = {"metaclass": Meta, "tag": original}

            class Alias:
                def __mro_entries__(self, bases):
                    keywords["metaclass"] = None
                    keywords["tag"] = "changed after argument evaluation"
                    return ()

            alias = Alias()
            if native:
                preparation = self.prepare("Captured", (alias,), keywords)
                self.assertIs(preparation[0], Meta)
            else:

                class Captured(alias, **keywords):
                    pass

            return observed

        self.assertEqual(exercise(False), [True])
        self.assertEqual(exercise(True), [True])

    def test_unresolved_bases_preserve_explicit_orig_bases_and_prepared_classcell(self):
        class Meta(type):
            @classmethod
            def __prepare__(meta, name, bases):
                return {"__classcell__": "invalid prepared cell"}

        bases = ()
        preparation = self.prepare("Prepared", bases, {"metaclass": Meta})
        namespace = preparation[1]
        namespace["__orig_bases__"] = "explicit body value"
        self.complete(preparation, bases)
        self.assertIs(preparation[2], bases)
        self.assertEqual(len(preparation), 4)
        self.assertEqual(namespace["__orig_bases__"], "explicit body value")
        self.assertEqual(namespace["__classcell__"], "invalid prepared cell")
        with self.assertRaisesRegex(TypeError, "__classcell__ must be"):
            Meta("Prepared", bases, namespace)

    def test_metaclass_conflict_and_invalid_prepare_fail_before_body(self):
        class FirstMeta(type):
            pass

        class SecondMeta(type):
            pass

        first = FirstMeta("First", (), {})
        second = SecondMeta("Second", (), {})
        with self.assertRaisesRegex(TypeError, "metaclass conflict"):
            self.prepare("Conflict", (first, second), {})

        class Invalid(type):
            @classmethod
            def __prepare__(meta, name, bases):
                return 17

        with self.assertRaisesRegex(TypeError, "must return a mapping"):
            self.prepare("Invalid", (), {"metaclass": Invalid})


class StrictTypeNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        api = ctypes.pythonapi
        cls.new_handle = api.PyType_NewSoacConstructionHandle
        cls.new_handle.argtypes = [ctypes.POINTER(ConstructionSpec)]
        cls.new_handle.restype = ctypes.py_object
        cls.construct = api.PyType_FromSoacConstructionHandle
        cls.construct.argtypes = [ctypes.py_object, ctypes.py_object]
        cls.construct.restype = ctypes.py_object
        cls.seal = api.PyType_SealSoacContract
        cls.seal.argtypes = [ctypes.py_object, ctypes.py_object]
        cls.seal.restype = ctypes.c_int
        cls.has_contract = api.PyType_HasSoacContract
        cls.has_contract.argtypes = [ctypes.py_object]
        cls.has_contract.restype = ctypes.c_int
        cls.get_owner = api.PyType_GetSoacContractOwner
        cls.get_owner.argtypes = [ctypes.py_object]
        cls.get_owner.restype = ctypes.c_void_p
        cls.get_dict = api.PyType_GetDict
        cls.get_dict.argtypes = [ctypes.py_object]
        cls.get_dict.restype = ctypes.py_object
        cls.c_getattr = api.PyObject_GenericGetAttr
        cls.c_getattr.argtypes = [ctypes.py_object, ctypes.py_object]
        cls.c_getattr.restype = ctypes.py_object
        cls.c_setattr = api.PyObject_GenericSetAttr
        cls.c_setattr.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
        cls.c_setattr.restype = ctypes.c_int
        cls.descriptor_is_sealed = api._PySoac_IsDescriptorSealed
        cls.descriptor_is_sealed.argtypes = [ctypes.py_object]
        cls.descriptor_is_sealed.restype = ctypes.c_int
        mutation_error = api.PySoac_GetStrictMutationError
        mutation_error.argtypes = []
        mutation_error.restype = ctypes.c_void_p
        cls.mutation_error = ctypes.cast(mutation_error(), ctypes.py_object).value

    def build(
        self,
        namespace,
        *,
        name="Example",
        bases=(),
        fields=(),
        protected=(),
        final=(),
        flags=0,
        seal=True,
        owner=None,
        bind_type=None,
        namespace_function=None,
    ):
        if owner is None:
            owner = object()
        if namespace_function is None:
            namespace_function = lambda namespace, cell: None
        spec = ConstructionSpec(
                   4,
                   ctypes.sizeof(ConstructionSpec),
                   0,
                   0,
                   owner,
                   namespace_function,
                   name,
                   bases,
                   namespace,
                   {},
                   ctypes.cast(bind_type, ctypes.c_void_p) if bind_type else None,
                   None,
                   TypeContractSpecV4(flags=flags, fields=fields, protected_names=protected, final_methods=final, check_instance_write=None, new_instance_dict=None),
               )
        handle = self.new_handle(ctypes.byref(spec))
        result = self.construct(handle, namespace_function)
        if seal:
            self.assertEqual(self.seal(result, owner), 0)
        return result, owner, handle, namespace_function

    def test_layout_descriptor_identity_is_available_before_type_ready(self):
        predicate = ctypes.pythonapi._PySoac_IsLayoutDescriptor
        predicate.argtypes = [ctypes.py_object, ctypes.py_object]
        predicate.restype = ctypes.c_int
        type_flags = ctypes.pythonapi.PyType_GetFlags
        type_flags.argtypes = [ctypes.py_object]
        type_flags.restype = ctypes.c_ulong
        ready_flag = 1 << 12
        observations = []

        @ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object, ctypes.py_object)
        def bind(owner, actual):
            namespace = self.get_dict(actual)
            observations.append(
                (
                    bool(type_flags(actual) & ready_flag),
                    tuple(
                        predicate(name, namespace[name])
                        for name in ("__dict__", "__weakref__")
                    ),
                )
            )
            return 0

        actual, *_ = self.build({}, bind_type=bind)
        self.assertEqual(observations, [(False, (1, 1))])
        self.assertTrue(type_flags(actual) & ready_flag)

    def test_policy_precedes_set_name_and_init_subclass_callbacks(self):
        seen = []
        case = self

        class Descriptor:
            def __set_name__(self, owner, name):
                instance = owner()
                seen.append(
                    ("set_name", case.has_contract(owner), vars(instance).copy())
                )
                with case.assertRaises(TypeError):
                    instance.method = lambda: "shadow"
                instance.__dict__["method"] = lambda: "hidden"
                case.assertEqual(instance.method(), "method")

        class Base:
            def __init_subclass__(cls):
                seen.append(("init_subclass", case.has_contract(cls), cls().method()))

        base, *_ = self.build(
            {
                "__init_subclass__": classmethod(
                    Base.__dict__["__init_subclass__"].__func__
                )
            }
        )
        child, *_ = self.build(
            {"descriptor": Descriptor(), "method": lambda self: "method"},
            bases=(base,),
            protected=("method",),
        )
        self.assertEqual(seen, [("set_name", 1, {}), ("init_subclass", 1, "method")])
        self.assertEqual(child().method(), "method")

    def test_only_fresh_implicit_wrappers_are_sealed_before_native_callbacks(self):
        case = self
        observed = []
        functions = {
            "__new__": lambda cls: object.__new__(cls),
            "__init_subclass__": lambda cls: None,
            "__class_getitem__": lambda cls, item: (cls, item),
        }

        def check_wrappers(actual, phase):
            for name, function in functions.items():
                descriptor = actual.__dict__[name]
                case.assertEqual(case.descriptor_is_sealed(descriptor), 1)
                case.assertIs(descriptor.__func__, function)
                with case.assertRaises(TypeError):
                    descriptor.__init__(function)
            observed.append(phase)

        class Descriptor:
            def __set_name__(self, actual, name):
                check_wrappers(actual, "set_name")

        def init_subclass(actual):
            check_wrappers(actual, "init_subclass")

        base, *_ = self.build({"__init_subclass__": init_subclass})
        child, *_ = self.build(functions | {"descriptor": Descriptor()}, bases=(base,))
        self.assertEqual(observed, ["set_name", "init_subclass"])
        self.assertIsInstance(child(), child)
        self.assertEqual(child[17], (child, 17))

        ordinary = type("Ordinary", (), functions)
        supplied = {
            name: (staticmethod if name == "__new__" else classmethod)(function)
            for name, function in functions.items()
        }
        explicit, *_ = self.build(supplied)
        replacement = lambda *args: None
        for actual in (ordinary, explicit):
            for name in functions:
                descriptor = actual.__dict__[name]
                self.assertEqual(self.descriptor_is_sealed(descriptor), 0)
                if actual is explicit:
                    self.assertIs(descriptor, supplied[name])
                descriptor.__init__(replacement)
                self.assertIs(descriptor.__func__, replacement)

    def test_actual_owner_binding_precedes_callbacks_and_rejects_owner_replay(self):
        events = []
        bound = []
        rejected = []
        owner = object()
        case = self

        @ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object, ctypes.py_object)
        def bind(actual_owner, actual_type):
            if bound:
                rejected.append(actual_type)
                events.append("rebind rejected")
                return -1
            bound.append(actual_type)
            events.append(("bound", case.get_owner(actual_type) == id(actual_owner)))
            return 0

        class Descriptor:
            def __set_name__(self, actual_type, name):
                events.append(("set_name", bound[0] is actual_type))

        result, actual_owner, *_ = self.build(
            {"descriptor": Descriptor()}, owner=owner, bind_type=bind
        )
        self.assertIs(actual_owner, owner)
        self.assertEqual(self.get_owner(result), id(owner))
        self.assertEqual(events, [("bound", True), ("set_name", True)])
        with self.assertRaises(ImportError):
            self.build({"descriptor": Descriptor()}, owner=owner, bind_type=bind)
        self.assertEqual(events[-1], "rebind rejected")
        self.assertEqual(sum(event == ("set_name", True) for event in events), 1)
        with self.assertRaises(ImportError):
            self.get_owner(rejected[0])
        abandoned = weakref.ref(rejected[0])
        rejected.clear()
        gc.collect()
        self.assertIsNone(abandoned())
        self.assertIsNone(self.get_owner(object))
        self.assertIsNone(self.get_owner(type("Ordinary", (), {})))

    def test_construction_spec_matches_the_actual_native_layout(self):
        import _testinternalcapi

        layout = _testinternalcapi.soac_type_construction_layout()
        self.assertEqual(layout["abi_version"], 4)
        self.assertEqual(ctypes.sizeof(ConstructionSpec), layout["spec_size"])
        self.assertEqual(ctypes.sizeof(TypeContractSpecV4), layout["contract_size"])
        self.assertEqual(ConstructionSpec.contract.offset, layout["contract"])
        self.assertEqual(ConstructionSpec.commit_final.offset, layout["commit_final"])
        self.assertEqual(
            ConstructionSpec.contract.offset + TypeContractSpecV4.object_slot_fields.offset,
            layout["object_slot_fields"],
        )

    def test_old_construction_abi_is_rejected_before_native_callbacks(self):
        spec = ConstructionSpec(
                   2,
                   ctypes.sizeof(ConstructionSpec),
                   0,
                   0,
                   object(),
                   lambda namespace, cell: None,
                   "OldAbi",
                   (),
                   {},
                   {},
                   None,
                   None,
                   TypeContractSpecV4(flags=0, fields=(), protected_names=(), final_methods=(), check_instance_write=None, new_instance_dict=None),
               )
        for old_abi in (2, 3):
            spec.abi_version = old_abi
            with self.subTest(abi=old_abi), self.assertRaises(TypeError):
                self.new_handle(ctypes.byref(spec))

    def test_protected_lookup_and_store_cover_warmed_bytecodes_and_c_api(self):
        cls, *_ = self.build(
            {"method": lambda self: 7, "shared": 11}, protected=("method", "shared")
        )
        instance = cls()
        instance.__dict__["method"] = lambda: 88
        instance.__dict__["shared"] = 99

        def read(value):
            return value.method(), value.shared

        def write(value):
            value.method = None

        for _ in range(2000):
            self.assertEqual(read(instance), (7, 11))
            with self.assertRaises(TypeError):
                write(instance)
        self.assertEqual(object.__getattribute__(instance, "method")(), 7)
        self.assertEqual(self.c_getattr(instance, "method")(), 7)
        with self.assertRaises(TypeError):
            object.__setattr__(instance, "method", None)
        with self.assertRaises(TypeError):
            self.c_setattr(instance, "shared", None)
        self.assertEqual(vars(instance)["shared"], 99)

    def test_declared_field_overrides_inherited_nondata_method_but_not_descriptor(self):
        base, *_ = self.build({"field": lambda self: 1}, protected=("field",))
        child, *_ = self.build({}, bases=(base,), fields=("field",))
        instance = child()
        instance.field = "value"
        self.assertEqual(instance.field, "value")
        self.assertEqual(vars(instance), {"field": "value"})
        descriptor_cls, *_ = self.build(
            {"field": property(lambda self: 33)}, fields=("field",)
        )
        descriptor_instance = descriptor_cls()
        descriptor_instance.__dict__["field"] = 44
        self.assertEqual(descriptor_instance.field, 33)
        with self.assertRaises(AttributeError):
            descriptor_instance.field = 55

    def test_class_seal_covers_native_dictionary_and_descriptor_writes(self):
        cls, owner, *_ = self.build({"value": 1})
        self.assertEqual(self.seal(cls, owner), 0)
        with self.assertRaises(TypeError):
            self.seal(cls, object())
        with self.assertRaises(TypeError):
            cls.value = 2
        with self.assertRaises(TypeError):
            type.__setattr__(cls, "value", 2)
        with self.assertRaises(TypeError):
            self.get_dict(cls)["value"] = 2
        with self.assertRaises(TypeError):
            self.get_dict(cls).clear()
        with self.assertRaises(TypeError):
            type.__dict__["__name__"].__set__(cls, "Changed")
        with self.assertRaises(TypeError):
            type.__dict__["__annotations__"].__set__(cls, {"value": str})  # noqa: RUF063 -- native setter boundary
        self.assertEqual(cls.value, 1)

    def test_generic_dictionary_setter_preserves_authoritative_class_namespaces(self):
        setter = ctypes.pythonapi.PyObject_GenericSetDict
        setter.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.c_void_p]
        setter.restype = ctypes.c_int
        for seal in (False, True):
            for inherited in (False, True):
                for copied in (False, True):
                    with self.subTest(sealed=seal, inherited=inherited, copied=copied):
                        actual, *_ = self.build(
                            {"method": lambda self: 17}, protected=("method",),
                            final=("method",), seal=seal,
                        )
                        if inherited:
                            actual = type("OrdinaryDescendant", (actual,), {})
                            self.assertEqual(self.has_contract(actual), 0)
                        namespace = self.get_dict(actual)
                        replacement = namespace.copy() if copied else namespace
                        with self.assertRaises(self.mutation_error):
                            setter(actual, replacement, None)
                        self.assertIs(self.get_dict(actual), namespace)
                        self.assertEqual(actual().method(), 17)
                        if seal or inherited:
                            with self.assertRaises(self.mutation_error):
                                namespace["method"] = lambda self: "forbidden override"

        ordinary = type("OrdinaryControl", (), {})
        replacement = self.get_dict(ordinary).copy()
        self.assertEqual(setter(ordinary, replacement, None), 0)
        self.assertIs(self.get_dict(ordinary), replacement)

    def test_none_dictionary_replacement_preserves_aliases_and_installed_class_barriers(self):
        cls, *_ = self.build(
            {"method": lambda self: 17, "constant": 19},
            protected=("method", "constant"), seal=False,
        )
        instance = cls()
        original = vars(instance)
        replacement = {"method": lambda: "shadow", "constant": "shadow", "extra": 23}
        instance.__dict__ = replacement
        self.assertIs(vars(instance), replacement)
        self.assertIsNot(vars(instance), original)
        self.assertEqual((instance.method(), instance.constant, instance.extra), (17, 19, 23))
        another = {"extra": 29}
        self.c_setattr(instance, "__dict__", another)
        self.assertIs(vars(instance), another)
        with self.assertRaises(TypeError):
            instance.method = lambda: "override"
        with self.assertRaises(TypeError):
            instance.__class__ = type("Other", (), {})
        with self.assertRaises(TypeError):
            cls.__bases__ = (type("OtherBase", (), {}),)
        self.assertIs(vars(instance), another)

    def test_indexed_dictionary_replacement_still_refuses_before_changing_storage(self):
        import _testinternalcapi

        namespace_function = lambda: None
        cls = _testinternalcapi.dict_new_soac_type(
            "IndexedReplacement", (), {"__module__": __name__},
            ("value",), namespace_function,
        )
        instance = cls()
        original = vars(instance)
        for replace in (
            lambda: setattr(instance, "__dict__", {}),
            lambda: self.c_setattr(instance, "__dict__", {}),
        ):
            with self.assertRaises(TypeError):
                replace()
            self.assertIs(vars(instance), original)

    def test_incompatible_inline_base_rejected_before_factory_contract_installation(
        self,
    ):
        base, *_ = self.build({})
        owner = object()
        namespace_function = lambda namespace, cell: None
        # This trusted fixture callback must never run: admission fails before
        # allocation, callbacks, or any attempt to replace an inherited layout.
        factory = ctypes.PYFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p)(
            lambda owner, instance: None
        )
        spec = ConstructionSpec(
                   4,
                   ctypes.sizeof(ConstructionSpec),
                   0,
                   0,
                   owner,
                   namespace_function,
                   "Incompatible",
                   (base,),
                   {},
                   {},
                   None,
                   None,
                   TypeContractSpecV4(flags=0, dictionary_mode=1, fields=(), protected_names=(), final_methods=(), check_instance_write=None, new_instance_dict=ctypes.cast(factory, ctypes.c_void_p)),
               )
        with self.assertRaises(TypeError):
            self.new_handle(ctypes.byref(spec))

    def test_finality_covers_ordinary_and_dynamic_type_factories(self):
        final_cls, *_ = self.build({}, flags=1)
        with self.assertRaises(TypeError):
            type("Child", (final_cls,), {})

        class Meta(type):
            pass

        with self.assertRaises(TypeError):
            Meta("CustomMetaChild", (final_cls,), {})
        from_spec = ctypes.pythonapi.PyType_FromSpecWithBases
        from_spec.argtypes = [ctypes.POINTER(TypeSpec), ctypes.py_object]
        from_spec.restype = ctypes.py_object
        slots = (TypeSlot * 1)(TypeSlot(0, None))
        spec = TypeSpec(b"native.FinalChild", 0, 0, 0, slots)
        with self.assertRaises(TypeError):
            from_spec(ctypes.byref(spec), (final_cls,))
        base, *_ = self.build(
            {"method": lambda self: 1}, protected=("method",), final=("method",)
        )
        with self.assertRaises(TypeError):
            type("Child", (base,), {"method": lambda self: 2})
        ordinary = type("OrdinaryChild", (base,), {})
        with self.assertRaises(TypeError):
            ordinary.method = lambda self: 2
        with self.assertRaises(TypeError):
            self.get_dict(ordinary)["method"] = lambda self: 2
        self.assertEqual(ordinary().method(), 1)

    def test_ordinary_subclass_stays_dynamic_and_keeps_live_instance_dictionary(self):
        base, *_ = self.build({"method": lambda self: 1}, protected=("method",))
        ordinary = type("OrdinaryChild", (base,), {})
        self.assertEqual(self.has_contract(ordinary), 0)
        instance = ordinary()
        replacement = {"method": lambda: 99}
        instance.__dict__ = replacement
        self.assertIs(vars(instance), replacement)
        self.assertEqual(instance.method(), 99)
        instance.method = lambda: 88
        self.assertEqual(instance.method(), 88)

    def test_base_reassignment_checks_each_proposed_bases_transitive_strict_mro(self):
        strict, *_ = self.build(
            {"method": lambda self: "strict"},
            protected=("method",),
            final=("method",),
        )
        middle = type("Middle", (strict,), {})
        leaf = type("Leaf", (middle,), {})
        ordinary = type("Ordinary", (), {})
        alternate = type("Alternate", (), {})
        self.assertEqual(self.has_contract(middle), 0)
        self.assertEqual(self.has_contract(leaf), 0)
        c_setattr = ctypes.pythonapi.PyObject_SetAttr
        c_setattr.argtypes = [ctypes.py_object] * 3
        c_setattr.restype = ctypes.c_int
        setters = (
            ("setattr", setattr),
            ("type setter", type.__setattr__),
            (
                "bases descriptor",
                lambda obj, _name, value: type.__dict__["__bases__"].__set__(
                    obj, value
                ),
            ),
            ("C attribute setter", c_setattr),
        )
        for label, setter in setters:
            with self.subTest(setter=label, direction="ordinary control"):
                control = type("Control", (ordinary,), {})
                old_instance = control()
                old_instance.value = object()
                dictionary = vars(old_instance)
                setter(control, "__bases__", (alternate,))
                self.assertEqual(control.__bases__, (alternate,))
                self.assertIs(type(old_instance), control)
                self.assertIs(vars(old_instance), dictionary)
            for direction in ("gain", "drop"):
                with self.subTest(setter=label, direction=direction):
                    if direction == "gain":
                        actual = type(
                            "Victim", (ordinary,), {"method": lambda self: "shadow"}
                        )
                        proposed = (leaf,)
                    else:
                        actual = type("Descendant", (leaf,), {})
                        proposed = (ordinary,)
                    old_instance = actual()
                    old_instance.value = object()
                    dictionary = vars(old_instance)
                    before_bases, before_mro = actual.__bases__, actual.__mro__
                    before_namespace = self.get_dict(actual).copy()
                    before_value = old_instance.value
                    before_method = old_instance.method()
                    with self.assertRaises(self.mutation_error):
                        setter(actual, "__bases__", proposed)
                    self.assertIs(actual.__bases__, before_bases)
                    self.assertIs(actual.__mro__, before_mro)
                    self.assertEqual(self.get_dict(actual), before_namespace)
                    self.assertIs(type(old_instance), actual)
                    self.assertIs(vars(old_instance), dictionary)
                    self.assertIs(old_instance.value, before_value)
                    self.assertEqual(old_instance.method(), before_method)

    def test_class_reassignment_checks_both_actual_transitive_strict_mros(self):
        strict, *_ = self.build({"method": lambda self: "strict"})
        middle = type("Middle", (strict,), {})
        leaf = type("Leaf", (middle,), {})
        ordinary = type("Ordinary", (), {})
        alternate = type("Alternate", (), {})
        self.assertEqual(self.has_contract(middle), 0)
        self.assertEqual(self.has_contract(leaf), 0)
        c_setattr = ctypes.pythonapi.PyObject_SetAttr
        c_setattr.argtypes = [ctypes.py_object] * 3
        c_setattr.restype = ctypes.c_int
        setters = (
            ("setattr", setattr),
            ("object setter", object.__setattr__),
            (
                "class descriptor",
                lambda obj, _name, value: object.__dict__["__class__"].__set__(
                    obj, value
                ),
            ),
            ("C attribute setter", c_setattr),
            ("C generic setter", self.c_setattr),
        )
        for label, setter in setters:
            with self.subTest(setter=label, direction="ordinary control"):
                control = ordinary()
                control.value = object()
                dictionary = vars(control)
                setter(control, "__class__", alternate)
                self.assertIs(type(control), alternate)
                self.assertIs(vars(control), dictionary)
            for direction, original, proposed in (
                ("gain", ordinary, leaf),
                ("drop", leaf, ordinary),
            ):
                with self.subTest(setter=label, direction=direction):
                    actual = original()
                    actual.value = object()
                    dictionary = vars(actual)
                    before = dictionary.copy()
                    with self.assertRaises(self.mutation_error):
                        setter(actual, "__class__", proposed)
                    self.assertIs(type(actual), original)
                    self.assertIs(vars(actual), dictionary)
                    self.assertEqual(dictionary, before)

    def test_custom_mro_cannot_publish_strict_ancestry_during_ordinary_rebase(self):
        strict, *_ = self.build(
            {"method": lambda self: "strict"},
            protected=("method",),
            final=("method",),
        )
        ordinary = type("Ordinary", (), {})
        alternate = type("Alternate", (), {})
        mode = ["ordinary"]
        events = []
        callback_error = LookupError("mro callback failed")

        class Meta(type):
            def mro(cls):
                result = type.mro(cls)
                events.append(mode[0])
                if mode[0] == "raise":
                    raise callback_error
                if mode[0] == "inject":
                    result.insert(-1, strict)
                return result

        actual = Meta("Victim", (ordinary,), {"method": lambda self: "shadow"})
        instance = actual()
        instance.value = object()
        dictionary = vars(instance)
        actual.__bases__ = (alternate,)
        self.assertEqual(actual.__bases__, (alternate,))
        self.assertIs(vars(instance), dictionary)
        for action in ("raise", "inject"):
            with self.subTest(action=action):
                mode[0] = action
                events.clear()
                before_bases, before_mro = actual.__bases__, actual.__mro__
                before_namespace = self.get_dict(actual).copy()
                error_type = LookupError if action == "raise" else self.mutation_error
                with self.assertRaises(error_type) as raised:
                    actual.__bases__ = (ordinary,)
                if action == "raise":
                    self.assertIs(raised.exception, callback_error)
                self.assertEqual(events, [action])
                self.assertIs(actual.__bases__, before_bases)
                self.assertIs(actual.__mro__, before_mro)
                self.assertEqual(self.get_dict(actual), before_namespace)
                self.assertIs(type(instance), actual)
                self.assertIs(vars(instance), dictionary)
                self.assertEqual(instance.method(), "shadow")

    def test_initial_custom_mro_cannot_hide_inherited_strict_contracts(self):
        strict, *_ = self.build(
            {"method": lambda self: "strict"},
            protected=("method",),
            final=("method",),
        )
        middle = type("Middle", (strict,), {})
        ordinary = type("Ordinary", (), {})

        class ReversedOrdinaryMeta(type):
            def mro(cls):
                return list(reversed(type.mro(cls)))

        control = ReversedOrdinaryMeta("Control", (ordinary,), {})
        self.assertEqual(control.__mro__, (object, ordinary, control))

        for shape in ("hidden strict base", "strict first entry"):
            with self.subTest(shape=shape):

                class Meta(type):
                    def mro(cls, shape=shape):
                        result = type.mro(cls)
                        if shape == "hidden strict base":
                            return [base for base in result if base is not strict]
                        return [strict, *result]

                bases = (middle,) if shape == "hidden strict base" else (ordinary,)
                with self.assertRaises(self.mutation_error):
                    Meta("HiddenContract", bases, {"method": lambda self: "shadow"})

        class InjectedMeta(type):
            def mro(cls):
                result = type.mro(cls)
                result.insert(-1, strict)
                return result

        with self.assertRaises(self.mutation_error):
            InjectedMeta("FinalOverride", (ordinary,), {"method": lambda self: 1})
        injected = InjectedMeta("InheritedOnly", (ordinary,), {})
        self.assertEqual(self.has_contract(injected), 0)
        self.assertIn(strict, injected.__mro__)
        with self.assertRaises(self.mutation_error):
            injected.method = lambda self: "override"
        instance = injected()
        replacement = {"method": lambda: "dynamic shadow"}
        instance.__dict__ = replacement
        self.assertIs(vars(instance), replacement)
        self.assertEqual(instance.method(), "dynamic shadow")

    def test_gc_introspection_cannot_mutate_published_policy_catalogs(self):
        cls, *_ = self.build({"method": lambda self: 1}, protected=("method",))
        states = [
            value
            for value in gc.get_referents(cls)
            if type(value).__name__ == "NativeTypeContract"
        ]
        self.assertEqual(len(states), 1)
        for value in gc.get_referents(states[0]):
            self.assertNotIsInstance(value, set)
        with self.assertRaises((AttributeError, TypeError)):
            states[0].protected_names = frozenset()
        with self.assertRaises(TypeError):
            cls().method = None

    def test_annotation_cache_preserves_laziness_visibility_and_dictionary_identity(
        self,
    ):
        events = []

        def annotate(format):
            events.append(format)
            return {"field": int}

        cls, *_ = self.build({"__annotate_func__": annotate})
        self.assertEqual(events, [])
        self.assertNotIn("__annotations_cache__", vars(cls))
        annotations = cls.__annotations__
        self.assertEqual(events, [1])
        self.assertIs(cls.__annotations__, annotations)
        self.assertIs(vars(cls)["__annotations_cache__"], annotations)
        annotations["field"] = str
        self.assertIs(cls.__annotations__["field"], str)
        with self.assertRaises(TypeError):
            self.get_dict(cls)["__annotations_cache__"] = {}

    def test_unannotated_class_materializes_caches_only_on_the_corresponding_read(self):
        cls, *_ = self.build({})
        self.assertNotIn("__annotate_func__", vars(cls))
        self.assertNotIn("__annotations_cache__", vars(cls))
        self.assertIsNone(cls.__annotate__)
        self.assertIsNone(vars(cls)["__annotate_func__"])
        self.assertNotIn("__annotations_cache__", vars(cls))
        self.assertEqual(cls.__annotations__, {})
        self.assertIs(cls.__annotations__, vars(cls)["__annotations_cache__"])

    def test_recursive_class_annotation_completion_matches_cpython(self):
        events = []
        inner = {"inner": int}
        outer = {"outer": str}

        def annotate(format):
            events.append(format)
            if len(events) == 1:
                self.assertIs(cls.__annotations__, inner)
                return outer
            return inner

        cls, *_ = self.build({"__annotate_func__": annotate})
        self.assertIs(cls.__annotations__, outer)
        self.assertIs(cls.__annotations__, outer)
        self.assertEqual(events, [1, 1])

    def test_failed_annotation_provider_can_retry_without_relaxing_the_class_seal(self):
        events = []

        def annotate(format):
            events.append(format)
            if len(events) == 1:
                raise NameError("not yet defined")
            return {"field": int}

        cls, *_ = self.build({"__annotate_func__": annotate})
        with self.assertRaises(NameError):
            _ = cls.__annotations__
        self.assertNotIn("__annotations_cache__", vars(cls))
        self.assertEqual(cls.__annotations__, {"field": int})
        with self.assertRaises(TypeError):
            cls.__annotate__ = lambda format: {}

    def test_handles_are_single_use_and_bound_to_exact_namespace_function(self):
        cls, _, handle, namespace_function = self.build({})
        with self.assertRaises(TypeError):
            self.construct(handle, namespace_function)
        with self.assertRaises(TypeError):
            self.construct(object(), namespace_function)
        self.assertEqual(self.has_contract(cls), 1)

    def test_type_contract_cycles_collect_and_do_not_disappear_on_live_class(self):
        cls, owner, handle, namespace_function = self.build({})
        reference = weakref.ref(cls)
        del cls, owner, handle, namespace_function
        gc.collect()
        self.assertIsNone(reference())

    def test_escaped_derived_namespace_does_not_own_the_class(self):
        base, *_ = self.build({}, name="NamespaceLifetimeBase")
        cls, *_ = self.build({}, name="NamespaceLifetimeChild", bases=(base,))
        namespace = self.get_dict(cls)
        self.assertNotIn("__dict__", namespace)
        events = []
        reference = weakref.ref(cls, lambda unused: events.append("class released"))
        del cls
        gc.collect()
        self.assertIsNone(reference())
        self.assertEqual(events, ["class released"])
        self.assertEqual(namespace, {})

    def test_class_namespace_predicate_binds_role_owner_and_live_type(self):
        import _testcapi

        matches = ctypes.pythonapi.PyDict_MatchesSoacClassNamespace
        matches.argtypes = [ctypes.py_object, ctypes.c_void_p]
        matches.restype = ctypes.c_int
        unavailable = ctypes.pythonapi.PySoac_GetStrictRuntimeUnavailableError
        unavailable.restype = ctypes.c_void_p
        unavailable_error = ctypes.cast(unavailable(), ctypes.py_object).value
        owner = object()
        during_bind = []

        @ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object, ctypes.py_object)
        def bind(actual_owner, actual_type):
            # The dictionary policy is not installed until binding returns.
            during_bind.append(matches(self.get_dict(actual_type), id(actual_owner)))
            return 0

        base, *_ = self.build({}, name="NamespacePolicyBase")
        cls, *_ = self.build({}, bases=(base,), owner=owner, bind_type=bind, seal=False)
        namespace = self.get_dict(cls)
        self.assertEqual(during_bind, [0])
        self.assertEqual(matches(namespace, id(owner)), 1)
        self.assertEqual(matches(namespace, 1), 0)  # compare, never dereference
        self.assertEqual(matches(namespace, 0), 0)
        self.assertEqual(matches(namespace.copy(), id(owner)), 0)
        unrelated = {}
        _testcapi.dict_set_soac_policy(unrelated, {}, set())
        self.assertEqual(matches(unrelated, id(owner)), 0)
        self.assertEqual(matches(vars(cls), id(owner)), 0)
        self.assertEqual(self.seal(cls, owner), 0)
        self.assertEqual(matches(namespace, id(owner)), 1)
        reference = weakref.ref(cls)
        del cls
        gc.collect()
        self.assertIsNone(reference())
        self.assertEqual(namespace, {})
        with self.assertRaises(unavailable_error):
            matches(namespace, id(owner))

    def test_sealed_class_outlives_actual_module_globals_without_retaining_them(self):
        import _testcapi

        class Token:
            pass

        cls, owner, *_ = self.build(
            {"constant": 17}, fields=("value",), protected=("constant",)
        )
        module = types.ModuleType("strict_class_globals_lifetime")
        token = Token()
        module.token = token
        module.result = cls
        namespace = module.__dict__
        _testcapi.dict_set_soac_policy(
            namespace, dict.fromkeys(namespace), set(namespace)
        )
        _testcapi.dict_seal_soac_namespace(namespace)
        token_reference = weakref.ref(token)
        module_reference = weakref.ref(module)
        del token, namespace, module
        self.assertIsNone(module_reference())
        gc.collect()
        self.assertIsNone(token_reference())
        self.assertEqual(self.get_owner(cls), id(owner))
        instance = cls()
        instance.value = 23
        self.assertEqual((instance.value, instance.constant), (23, 17))
        with self.assertRaises(TypeError):
            instance.constant = 42


class PendingTypeNativeTests(unittest.TestCase):
    """Actual native pending state; these C fixtures grant no source authority."""

    @classmethod
    def setUpClass(cls):
        import _testinternalcapi

        StrictTypeNativeTests.setUpClass.__func__(cls)
        cls.native = _testinternalcapi
        api = ctypes.pythonapi
        cls.get_info = api.PyType_GetSoacConstructionInfoV1
        cls.get_info.argtypes = [
            ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
        ]
        cls.get_info.restype = ctypes.c_int
        cls.fail_pending = api.PyType_FailSoacPendingV1
        cls.fail_pending.argtypes = [ctypes.py_object]
        cls.fail_pending.restype = ctypes.c_int
        cls.dispose_pending = api.PyType_DisposeSoacProvisionalV1
        cls.dispose_pending.argtypes = [ctypes.py_object] * 3
        cls.dispose_pending.restype = ctypes.c_int

    enforced = StrictTypeNativeTests.build

    @staticmethod
    def owner(*, error=None, checked=(), payload=None):
        return ([False] * 4, error, checked, payload)

    def pending(self, namespace=None, *, name="Pending", bases=(), owner=None):
        if owner is None:
            owner = self.owner()
        namespace_function = lambda namespace, cell: None
        actual, root = self.native.soac_pending_type_construct(
            name, bases, {} if namespace is None else namespace,
            namespace_function, owner,
        )
        return actual, owner, root

    def info(self, actual):
        result = ConstructionInfoV1()
        present = self.get_info(actual, ctypes.byref(result), ctypes.sizeof(result))
        self.assertEqual(present, 1)
        self.assertEqual(result.abi_version, 1)
        self.assertEqual(result.struct_size, ctypes.sizeof(result))
        return result

    def admit(self, actual, owner, root, *, fields=(), protected=(), final=(), slots=None):
        return self.native.soac_pending_type_admit(
            actual, owner, root, fields, protected, final, slots,
        )

    def test_generic_dictionary_setter_preserves_pending_admitted_and_failed_type_identity(self):
        setter = ctypes.pythonapi.PyObject_GenericSetDict
        setter.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.c_void_p]
        setter.restype = ctypes.c_int
        for phase in ("pending", "admitted", "failed"):
            for copied in (False, True):
                with self.subTest(phase=phase, copied=copied):
                    actual, owner, root = self.pending({"value": 17})
                    if phase == "admitted":
                        self.admit(actual, owner, root)
                    elif phase == "failed":
                        self.assertEqual(self.fail_pending(root), 0)
                    namespace = self.get_dict(actual)
                    before = self.info(actual)
                    replacement = namespace.copy() if copied else namespace
                    with self.assertRaises(self.mutation_error):
                        setter(actual, replacement, None)
                    self.assertIs(self.get_dict(actual), namespace)
                    self.assertEqual(self.info(actual).phase, before.phase)
                    self.assertEqual(actual.value, 17)
                    if phase == "pending":
                        self.admit(actual, owner, root)
                    if phase != "failed":
                        self.assertEqual(actual().value, 17)
                    else:
                        with self.assertRaises(self.mutation_error):
                            actual()

    def test_ready_callback_cannot_replace_the_pending_type_dictionary(self):
        setter = ctypes.pythonapi.PyObject_GenericSetDict
        setter.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.c_void_p]
        setter.restype = ctypes.c_int
        observations = []
        case = self

        class Descriptor:
            def __set_name__(self, actual, name):
                namespace = case.get_dict(actual)
                with case.assertRaises(case.mutation_error):
                    setter(actual, namespace.copy(), None)
                case.assertIs(case.get_dict(actual), namespace)
                observations.append(case.info(actual).phase)

        actual, owner, root = self.pending({"descriptor": Descriptor()})
        self.assertEqual(observations, [1])
        self.admit(actual, owner, root)
        actual()

    def test_admission_cannot_substitute_a_noop_for_the_construction_callback(self):
        actual, owner, root = self.pending()
        before = self.info(actual)
        with self.assertRaisesRegex(self.mutation_error, "callback cannot be substituted"):
            self.native.soac_pending_type_try_weaker_admission(actual, owner, root, True)
        after = self.info(actual)
        self.assertEqual((after.phase, after.permanent_contract_published), (1, 0))
        self.assertEqual((after.owner, after.root_construction), (before.owner, before.root_construction))
        self.assertEqual(owner[0], [True, False, False, False])
        with self.assertRaises(self.mutation_error):
            actual()
        self.admit(actual, owner, root)
        actual()

    def test_recorded_commit_rejects_a_weaker_contract_and_keeps_the_barrier_closed(self):
        owner = self.owner(checked=("value",))
        actual, _, root = self.pending({"__slots__": ("value",)}, owner=owner)
        with self.assertRaisesRegex(AssertionError, "required field was omitted"):
            self.native.soac_pending_type_try_weaker_admission(actual, owner, root, False)
        info = self.info(actual)
        self.assertEqual((info.phase, info.permanent_contract_published), (4, 1))
        with self.assertRaises(self.mutation_error):
            actual()
        with self.assertRaises(self.mutation_error):
            self.admit(actual, owner, root, slots=("value",))

    def test_query_distinguishes_ordinary_pending_and_permanent_without_grant(self):
        class Ordinary:
            pass

        info = ConstructionInfoV1()
        ctypes.memset(ctypes.byref(info), 0xA5, ctypes.sizeof(info))
        self.assertEqual(self.get_info(Ordinary, ctypes.byref(info), ctypes.sizeof(info)), 0)
        self.assertEqual(bytes(info), bytes(ctypes.sizeof(info)))
        actual, owner, root = self.pending()
        info = self.info(actual)
        self.assertEqual((info.phase, info.permanent_contract_published), (1, 0))
        self.assertEqual((info.owner, info.root_construction), (id(owner), id(root)))
        self.assertEqual(self.has_contract(actual), 0)
        self.assertIsNone(self.get_owner(actual))
        self.assertEqual(owner[0], [True, False, False, False])
        with self.assertRaises(self.mutation_error):
            self.seal(actual, owner)
        self.admit(actual, owner, root)
        self.assertEqual(self.info(actual).phase, 3)
        self.assertEqual(self.has_contract(actual), 1)
        self.assertEqual(self.get_owner(actual), id(owner))
        self.assertEqual(owner[0][:3], [True, True, True])

    def test_pending_early_bind_accepts_actual_dataclass_invocation_before_ready(self):
        def apply(actual):
            return actual

        invocation = self.native.soac_dataclass_fixture(apply, (), (), (), None)
        bind = ctypes.pythonapi.PySoac_DataclassBindClass
        bind.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.c_void_p]
        bind.restype = ctypes.c_int
        observed = []

        def prepare(actual):
            info = self.info(actual)
            self.assertEqual((info.phase, info.permanent_contract_published), (1, 0))
            self.assertFalse(actual.__flags__ & ((1 << 12) | (1 << 13)))
            self.assertEqual(bind(invocation, actual, info.owner), 0)
            observed.append(info.root_construction)

        owner = self.owner(payload=prepare)
        actual, _, root = self.pending(owner=owner)
        self.assertEqual(observed, [id(root)])
        self.assertIs(
            self.native.soac_dataclass_fixture_call(invocation, 2, apply, (actual,), {}),
            actual,
        )
        complete = ctypes.pythonapi.PySoac_CompleteDataclassInvocation
        complete.argtypes = [ctypes.py_object]
        complete.restype = ctypes.c_int
        self.assertEqual(complete(invocation), 0)
        self.admit(actual, owner, root)
        actual()


    def test_enforced_early_classcell_rejects_native_allocation_and_class_assignment(self):
        import sys

        class Ordinary:
            __slots__ = ()

        class Other:
            __slots__ = ()

        instance = Ordinary()
        instance.__class__ = Other
        instance.__class__ = Ordinary
        active, audits = [], []

        def audit(event, args):
            if (event == "object.__setattr__" and active
                    and args[0] is instance and args[2] is active[0]):
                audits.append(args[1])

        sys.addaudithook(audit)
        observations, observer_errors = {}, []
        try:
            for enforced in (False, True):
                cell = types.CellType()
                rows = []

                class Displaced:
                    def __del__(displaced):
                        try:
                            actual = cell.cell_contents
                            info = ConstructionInfoV1()
                            present = self.get_info(
                                actual, ctypes.byref(info), ctypes.sizeof(info),
                            )
                            ready = bool(actual.__flags__ & (1 << 12))
                            try:
                                self.native.soac_pending_type_allocate(actual, 0)
                            except BaseException as error:
                                allocation_error = type(error)
                            else:
                                allocation_error = None
                            error, null, unchanged, refs = (
                                self.native.soac_pending_type_init_buffer(
                                    actual, False, None,
                                )
                            )
                            buffer = (type(error) if error is not None else None,
                                      null, unchanged, refs)
                            active[:] = [actual]
                            audits.clear()
                            try:
                                instance.__class__ = actual
                            except BaseException as error:
                                assignment_error = type(error)
                            else:
                                assignment_error = None
                            assignment = (assignment_error, type(instance) is Ordinary,
                                          tuple(audits))
                            active.clear()
                            if type(instance) is not Ordinary:
                                # The assigned ordinary type is not Ready yet:
                                # call the actual descriptor, not its missing
                                # pre-Ready attribute dispatch, for cleanup.
                                object.__dict__["__class__"].__set__(instance, Ordinary)
                            rows.append((present, info.phase,
                                         info.permanent_contract_published,
                                         ready, allocation_error, buffer, assignment))
                        except BaseException as error:
                            observer_errors.append(error)
                        finally:
                            active.clear()

                cell.cell_contents = Displaced()
                namespace = {"__slots__": (), "__classcell__": cell}
                if enforced:
                    actual, *_ = self.enforced(
                        namespace, name="EarlyEnforced", seal=False,
                    )
                else:
                    actual = type("EarlyOrdinary", (), namespace)
                self.assertIs(cell.cell_contents, actual)
                self.assertEqual(
                    (actual.__basicsize__, actual.__itemsize__,
                     actual.__dictoffset__, actual.__weakrefoffset__),
                    (Ordinary.__basicsize__, Ordinary.__itemsize__,
                     Ordinary.__dictoffset__, Ordinary.__weakrefoffset__),
                )
                observations[enforced] = rows
                self.assertIs(type(instance), Ordinary)
                self.assertIs(type(actual()), actual)
        finally:
            active.clear()
        self.assertEqual(observer_errors, [])
        # The same actual classcell publication is an ordinary native oracle.
        # It proves both the C allocator and destination layout are viable.
        self.assertEqual(observations[False], [
            (0, 0, 0, False, None, (None, 0, 0, 0),
             (None, False, ("__class__",))),
        ])
        self.assertEqual(observations[True], [
            (1, 1, 0, False, self.mutation_error,
             (self.mutation_error, 1, 1, 1),
             (self.mutation_error, True, ())),
        ])

    def test_enforced_gc_binding_keeps_instances_closed_until_policy_commit(self):
        actual_name = "EnforcedGCBeforePolicyCommit"
        events, observer_errors, bindings = [], [], []
        observing = [True]

        def observe(phase, details):
            if phase != "start" or not observing:
                return
            try:
                for candidate in gc.get_objects():
                    if type(candidate) is not type or candidate.__name__ != actual_name:
                        continue
                    info = ConstructionInfoV1()
                    present = self.get_info(
                        candidate, ctypes.byref(info), ctypes.sizeof(info),
                    )
                    # Init has a caller-buffer return channel even before Ready;
                    # no incomplete object is exposed or run through deallocation.
                    error, null, unchanged, refs = (
                        self.native.soac_pending_type_init_buffer(
                            candidate, False, None,
                        )
                    )
                    events.append((
                        present, info.phase, info.permanent_contract_published,
                        bool(candidate.__flags__ & (1 << 12)),
                        type(error) if error is not None else None,
                        null, unchanged, refs,
                    ))
            except BaseException as error:
                observer_errors.append(error)

        @ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object, ctypes.py_object)
        def bind(owner, actual):
            try:
                bindings.append((
                    self.get_owner(actual) == id(owner),
                    self.has_contract(actual),
                    bool(actual.__flags__ & (1 << 12)),
                ))
                gc.collect()
            except BaseException as error:
                observer_errors.append(error)
            finally:
                # ENFORCED must open before the existing Ready callbacks.
                observing.clear()
            return 0

        thresholds = gc.get_threshold()
        gc.callbacks.append(observe)
        try:
            gc.set_threshold(1, 1, 1)
            actual, *_ = self.enforced(
                {"__slots__": ()}, name=actual_name, bind_type=bind,
            )
        finally:
            observing.clear()
            gc.set_threshold(*thresholds)
            gc.callbacks.remove(observe)
        self.assertEqual(observer_errors, [])
        self.assertEqual(bindings, [(True, 1, False)])
        self.assertTrue(events)
        self.assertIn((1, 1, 1, False, self.mutation_error, 1, 1, 1), events)
        self.assertTrue(all(
            present == 1 and phase == 1 and not ready
            and error is self.mutation_error and (null, unchanged, refs) == (1, 1, 1)
            for present, phase, permanent, ready, error, null, unchanged, refs in events
        ), events)
        self.assertEqual(self.info(actual).phase, 3)
        self.assertIs(type(actual()), actual)

    def test_enforced_failed_prepolicy_construction_keeps_escaped_type_closed(self):
        # The second cell is checked after the first has published the real type.
        # Keep the ordinary native error and the escaped original type intact.
        errors = []
        for enforced in (False, True):
            cell = types.CellType()
            namespace = {
                "__slots__": (), "__classcell__": cell,
                "__classdictcell__": object(),
            }
            with self.assertRaises(TypeError) as raised:
                if enforced:
                    self.enforced(namespace, seal=False)
                else:
                    type("OrdinaryFailedCell", (), namespace)
            errors.append(raised.exception)
            actual = cell.cell_contents
            if not enforced:
                self.native.soac_pending_type_allocate(actual, 0)
                continue
            info = self.info(actual)
            self.assertEqual((info.phase, info.permanent_contract_published), (4, 0))
            self.assertIsNone(info.root_construction)
            with self.assertRaises(self.mutation_error):
                self.native.soac_pending_type_allocate(actual, 0)
            error, null, unchanged, refs = self.native.soac_pending_type_init_buffer(
                actual, False, None,
            )
            self.assertIsInstance(error, self.mutation_error)
            self.assertEqual((null, unchanged, refs), (1, 1, 1))
        self.assertEqual(errors[0].args, errors[1].args)
        self.assertIsNone(errors[1].__context__)
        self.assertIsNone(errors[1].__cause__)


    def test_enforced_early_public_ready_does_not_open_before_policy_and_bind(self):
        ready = ctypes.pythonapi.PyType_Ready
        ready.argtypes = [ctypes.py_object]
        ready.restype = ctypes.c_int
        case = self
        observations, unexpected = {}, []
        for inherited in (False, True):
            for enforced in (False, True):
                rows, cell = [], types.CellType()
                if inherited:
                    namespace = {"__slots__": (), "method": lambda self: 17}
                    if enforced:
                        base, *_ = self.enforced(namespace, final=("method",))
                    else:
                        base = type("OrdinaryFinalBaseControl", (), namespace)
                    bases = (base,)
                else:
                    bases = ()

                class Displaced:
                    def __del__(displaced):
                        try:
                            actual = cell.cell_contents
                            before = bool(actual.__flags__ & (1 << 12))
                            status = ready(actual)
                            after = bool(actual.__flags__ & (1 << 12))
                            try:
                                actual()
                            except BaseException as error:
                                result = type(error)
                            else:
                                result = None
                            rows.append(("classcell", before, status, after, result))
                        except BaseException as error:
                            unexpected.append((inherited, enforced, "classcell", error))

                @ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object, ctypes.py_object)
                def bind(owner, actual):
                    try:
                        info = case.info(actual)
                        try:
                            case.native.soac_pending_type_allocate(actual, 0)
                        except BaseException as error:
                            result = type(error)
                        else:
                            result = None
                        rows.append(("bind", info.phase,
                                     info.permanent_contract_published, result))
                    except BaseException as error:
                        unexpected.append((inherited, enforced, "bind", error))
                    return 0

                class Descriptor:
                    def __set_name__(descriptor, actual, name):
                        rows.append(("set_name", type(actual()) is actual))

                cell.cell_contents = Displaced()
                namespace = {
                    "__slots__": (), "__classcell__": cell,
                    "descriptor": Descriptor(), "constant": 23,
                }
                try:
                    if enforced:
                        actual, *_ = self.enforced(
                            namespace, bases=bases, protected=("constant",),
                            bind_type=bind,
                        )
                    else:
                        actual = type("OrdinaryEarlyReadyControl", bases, namespace)
                except BaseException as error:
                    unexpected.append((inherited, enforced, "construction", error))
                else:
                    self.assertIs(type(actual()), actual)
                    if inherited:
                        self.assertEqual(actual().method(), 17)
                        if enforced:
                            with self.assertRaises(self.mutation_error):
                                self.get_dict(actual)["method"] = lambda self: 19
                observations[inherited, enforced] = rows
        self.assertEqual(unexpected, [])
        for inherited in (False, True):
            self.assertEqual(observations[inherited, False], [
                ("classcell", False, 0, True, None), ("set_name", True),
            ])
            self.assertEqual(observations[inherited, True], [
                ("classcell", False, 0, True, self.mutation_error),
                ("bind", 1, 1, self.mutation_error), ("set_name", True),
            ])

    def test_enforced_constructor_state_does_not_pin_execution_inputs(self):
        class Token:
            pass

        for reject in (False, True):
            with self.subTest(reject=reject):
                escaped = []

                @ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object, ctypes.py_object)
                def bind(owner, actual):
                    escaped.append(actual)
                    return -int(reject)

                token = Token()
                namespace = {"payload": token}
                function = types.FunctionType((lambda: None).__code__, namespace)
                function_ref, token_ref = weakref.ref(function), weakref.ref(token)
                if reject:
                    with self.assertRaises(ImportError):
                        self.enforced(
                            {"__slots__": ()}, bind_type=bind, seal=False,
                            namespace_function=function,
                        )
                else:
                    made = self.enforced(
                        {"__slots__": ()}, bind_type=bind, seal=False,
                        namespace_function=function,
                    )
                    self.assertIs(made[0], escaped[0])
                    del made
                del function, namespace, token
                self.assertIsNone(function_ref())
                self.assertIsNone(token_ref())
                info = self.info(escaped[0])
                self.assertEqual(info.phase, 4 if reject else 3)
                self.assertIsNone(info.root_construction)
                if reject:
                    with self.assertRaises(self.mutation_error):
                        self.native.soac_pending_type_allocate(escaped[0], 0)
                else:
                    self.assertIs(type(escaped[0]()), escaped[0])

    def test_barrier_precedes_classcell_old_owner_and_ready_callbacks(self):
        observations = []
        case = self
        cell = types.CellType()

        def inspect(actual, where):
            info = case.info(actual)
            before = bool(actual.__flags__ & (1 << 12))
            try:
                actual()
            except case.mutation_error:
                observations.append((where, info.phase, before, "blocked"))
            else:
                observations.append((where, info.phase, before, "allocated"))

        class Displaced:
            def __del__(self):
                inspect(cell.cell_contents, "classcell")

        class Descriptor:
            def __set_name__(self, actual, name):
                inspect(actual, "set_name")

        def init_subclass(actual):
            inspect(actual, "init_subclass")

        base, *_ = self.enforced(
            {"__init_subclass__": classmethod(init_subclass)}, name="PendingBase",
        )
        cell.cell_contents = Displaced()
        actual, owner, root = self.pending(
            {"__classcell__": cell, "descriptor": Descriptor()}, bases=(base,),
        )
        self.assertIs(cell.cell_contents, actual)
        self.assertEqual(observations, [
            ("classcell", 1, False, "blocked"),
            ("set_name", 1, True, "blocked"),
            ("init_subclass", 1, True, "blocked"),
        ])
        self.admit(actual, owner, root)
        self.assertIs(type(actual()), actual)

    def test_gc_observer_sees_barrier_before_ready_and_copied_namespace(self):
        events = []
        observer_errors = []
        actual_name = "PendingGCBeforeReady"
        case = self
        owner = None

        def observe(phase, details):
            if phase != "start":
                return
            try:
                for candidate in gc.get_objects():
                    if type(candidate) is not type or candidate.__name__ != actual_name:
                        continue
                    info = case.info(candidate)
                    try:
                        candidate()
                    except case.mutation_error:
                        blocked = True
                    except BaseException as error:
                        observer_errors.append(error)
                        blocked = False
                    else:
                        blocked = False
                    events.append((info.phase, bool(candidate.__flags__ & (1 << 12)), blocked))
            except BaseException as error:
                observer_errors.append(error)

        def prepare(actual):
            namespace = case.get_dict(actual)
            case.assertEqual(namespace["original_value"], 23)
            case.assertFalse(actual.__flags__ & (1 << 12))
            info = case.info(actual)
            root = ctypes.cast(info.root_construction, ctypes.py_object).value
            with case.assertRaises(case.mutation_error):
                case.admit(actual, owner, root)
            # This is the actual early hook on the actual type, not a made-up
            # state or delayed Ready observer. Its allocation can reenter GC.
            gc.collect()

        owner = self.owner(payload=prepare)
        thresholds = gc.get_threshold()
        gc.callbacks.append(observe)
        try:
            gc.set_threshold(1, 1, 1)
            actual, _, root = self.pending(
                {"original_value": 23, "__init_subclass__": lambda cls: None},
                name=actual_name, owner=owner,
            )
        finally:
            gc.set_threshold(*thresholds)
            gc.callbacks.remove(observe)
        self.assertEqual(observer_errors, [])
        self.assertIn((1, False, True), events)
        self.assertTrue(all(phase == 1 and blocked for phase, _, blocked in events))
        self.admit(actual, owner, root)

    def test_reentrant_early_bind_failure_keeps_escaped_type_terminal(self):
        primary = LookupError("early preparation failed")
        escaped = []

        def prepare(actual):
            escaped.append(actual)
            raise primary

        owner = self.owner(payload=prepare)
        try:
            self.pending(owner=owner)
        except LookupError as error:
            self.assertIs(error, primary)
        else:
            self.fail("early preparation did not fail")
        self.assertEqual(len(escaped), 1)
        self.assertEqual(self.info(escaped[0]).phase, 4)
        with self.assertRaises(self.mutation_error):
            escaped[0]()

    def test_python_and_warmed_allocator_paths_refuse_before_new(self):
        called = []

        def fresh(cls):
            called.append(cls)
            return object.__new__(cls)

        actual, owner, root = self.pending({"__new__": staticmethod(fresh)})

        class Ordinary:
            __new__ = staticmethod(fresh)

        def call(cls):
            return cls()

        for _ in range(100):
            call(Ordinary)
        called.clear()
        for _ in range(20):
            with self.assertRaises(self.mutation_error):
                call(actual)
        self.assertEqual(called, [])
        with self.assertRaises(self.mutation_error):
            object.__new__(actual)
        self.assertEqual(called, [])
        self.admit(actual, owner, root)
        self.assertIs(type(call(actual)), actual)
        self.assertEqual(called, [actual])

    def test_supported_c_allocators_refuse_in_their_actual_status_channel(self):
        # Actual list layout makes NewVar and GC-NewVar valid C API operands.
        # No final admission of this deliberately unsupported ordinary base.
        actual, _, _ = self.pending(bases=(list,), name="PendingList")
        for operation in range(7):
            with self.subTest(operation=operation):
                with self.assertRaises(self.mutation_error):
                    self.native.soac_pending_type_allocate(actual, operation)

    def test_public_init_and_initvar_do_not_touch_or_own_rejected_buffers(self):
        actual, _, _ = self.pending(bases=(list,), name="PendingInit")
        primary = LookupError("preserve exact pending allocation error")
        for variable in (False, True):
            with self.subTest(variable=variable):
                error, null, unchanged, refs = self.native.soac_pending_type_init_buffer(
                    actual, variable, None,
                )
                self.assertIsInstance(error, self.mutation_error)
                self.assertEqual((null, unchanged, refs), (1, 1, 1))
                error, null, unchanged, refs = self.native.soac_pending_type_init_buffer(
                    actual, variable, primary,
                )
                self.assertIs(error, primary)
                self.assertEqual((null, unchanged, refs), (1, 1, 1))

        class Ordinary:
            pass

        error, null, unchanged, refs = self.native.soac_pending_type_init_buffer(
            Ordinary, False, None,
        )
        self.assertIsNone(error)
        self.assertEqual((null, unchanged, refs), (0, 0, 0))

    def test_layout_compatible_destination_is_rejected_before_audit(self):
        import sys

        class Ordinary:
            __slots__ = ()

        class Other:
            __slots__ = ()

        ordinary = Ordinary()
        ordinary.__class__ = Other
        ordinary.__class__ = Ordinary
        actual, owner, root = self.pending({"__slots__": ()})
        self.assertEqual(actual.__basicsize__, Ordinary.__basicsize__)
        self.assertEqual(actual.__dictoffset__, Ordinary.__dictoffset__)
        events = []
        active = [ordinary]

        def audit(event, args):
            if event == "object.__setattr__" and active and args[0] is active[0]:
                events.append(args[1])

        sys.addaudithook(audit)
        try:
            with self.assertRaises(self.mutation_error):
                ordinary.__class__ = actual
            self.assertEqual(events, [])
            self.assertIs(type(ordinary), Ordinary)
            ordinary.__class__ = Other
            self.assertEqual(events, ["__class__"])
            self.admit(actual, owner, root)
            with self.assertRaises(self.mutation_error):
                ordinary.__class__ = actual
            self.assertIs(type(ordinary), Other)
        finally:
            active.clear()

    def test_pending_bases_refuse_python_c_and_base_reassignment(self):
        actual, _, _ = self.pending()

        class Ordinary:
            pass

        class Child(Ordinary):
            pass

        with self.assertRaises(self.mutation_error):
            type("Rejected", (actual,), {})
        with self.assertRaises(self.mutation_error):
            self.native.soac_pending_type_from_spec(actual)
        with self.assertRaises(self.mutation_error):
            Child.__bases__ = (actual,)
        with self.assertRaises(self.mutation_error):
            actual.__bases__ = (Ordinary,)
        self.assertEqual(Child.__bases__, (Ordinary,))
        derived = self.native.soac_pending_type_from_spec(Ordinary)
        self.assertEqual(derived.__bases__, (Ordinary,))

    def test_failed_constructor_terminalizes_escaped_type_before_cleanup(self):
        escaped = []
        primary = LookupError("set_name failed")

        class Descriptor:
            def __set_name__(self, actual, name):
                escaped.append(actual)
                raise primary

        try:
            self.pending({"descriptor": Descriptor()})
        except LookupError as error:
            self.assertIs(error, primary)
        else:
            self.fail("native type construction must propagate the exact callback error")
        self.assertEqual(len(escaped), 1)
        self.assertEqual(self.info(escaped[0]).phase, 4)
        with self.assertRaises(self.mutation_error):
            escaped[0]()

    def test_final_commit_failure_is_terminal_and_preserves_primary(self):
        primary = LookupError("final owner commit refused")
        owner = self.owner(error=primary)
        actual, _, root = self.pending(owner=owner)
        try:
            self.admit(actual, owner, root)
        except LookupError as error:
            self.assertIs(error, primary)
        else:
            self.fail("actual final commit callback did not reject")
        self.assertEqual(owner[0][:3], [True, True, True])
        self.assertEqual(self.info(actual).phase, 4)
        with self.assertRaises(self.mutation_error):
            actual()
        with self.assertRaises(self.mutation_error):
            self.admit(actual, owner, root)
        self.assertEqual(self.fail_pending(root), 0)

    def test_wrong_owner_root_and_unselected_disposal_cannot_grant(self):
        actual, owner, root = self.pending()
        other, other_owner, other_root = self.pending(name="OtherPending")
        for wrong_owner, wrong_root in ((other_owner, root), (owner, other_root)):
            with self.subTest(owner=wrong_owner is owner, root=wrong_root is root):
                with self.assertRaises(self.mutation_error):
                    self.admit(actual, wrong_owner, wrong_root)
                self.assertEqual(self.info(actual).phase, 1)
        with self.assertRaises(self.mutation_error):
            self.dispose_pending(actual, owner, root)
        self.admit(actual, owner, root)
        with self.assertRaises(self.mutation_error):
            self.dispose_pending(actual, owner, root)
        with self.assertRaises(self.mutation_error):
            self.dispose_pending(other, other_owner, root)
        self.assertEqual(self.info(other).phase, 1)
        self.assertEqual(self.fail_pending(root), 0)
        self.assertEqual(self.info(actual).phase, 3)

    def test_query_and_lineage_failure_preserve_incoming_exception(self):
        actual, owner, root = self.pending()
        primary = KeyError("pending before native state operations")
        self.assertEqual(
            self.native.soac_pending_type_preserve_error(actual, root, primary),
            (1, 0, 1, 1),
        )
        self.assertEqual(self.info(actual).phase, 4)
        with self.assertRaises(self.mutation_error):
            actual()
        self.assertEqual(self.fail_pending(root), 0)
        with self.assertRaises(self.mutation_error):
            self.admit(actual, owner, root)

    def test_admission_preserves_ordinary_layout_and_installs_actual_slot_policy(self):
        namespace = {"__slots__": ("value",)}
        owner = self.owner(checked=("value",))
        actual, _, root = self.pending(namespace, owner=owner)
        layout = (actual.__basicsize__, actual.__itemsize__, actual.__dictoffset__,
                  actual.__weakrefoffset__, bool(actual.__flags__ & (1 << 2)))
        descriptor = actual.__dict__["value"]
        self.admit(actual, owner, root, slots=("value",))
        self.assertEqual(
            (actual.__basicsize__, actual.__itemsize__, actual.__dictoffset__,
             actual.__weakrefoffset__, bool(actual.__flags__ & (1 << 2))),
            layout,
        )
        self.assertIs(actual.__dict__["value"], descriptor)
        instance = actual()
        instance.value = 7
        with self.assertRaises(TypeError):
            instance.value = "not an int"
        self.assertEqual(instance.value, 7)

    def test_inherited_only_dictionary_policy_strengthens_without_replacement(self):
        base, *_ = self.enforced(
            {"final_method": lambda self: 7}, final=("final_method",), name="FinalBase",
        )
        actual, owner, root = self.pending(bases=(base,))
        namespace = self.get_dict(actual)
        with self.assertRaises(self.mutation_error):
            namespace["final_method"] = lambda self: 9
        namespace["ordinary"] = 17
        self.admit(actual, owner, root, protected=("ordinary",))
        self.assertIs(self.get_dict(actual), namespace)
        with self.assertRaises(self.mutation_error):
            namespace["__getattribute__"] = object.__getattribute__
        instance = actual()
        with self.assertRaises(self.mutation_error):
            instance.ordinary = 19
        self.assertEqual((instance.ordinary, instance.final_method()), (17, 7))

    def test_consumed_root_does_not_keep_namespace_function_or_globals_alive(self):
        class Token:
            pass

        token = Token()
        token_ref = weakref.ref(token)
        namespace = {"payload": token}
        function = types.FunctionType((lambda: None).__code__, namespace)
        function_ref = weakref.ref(function)
        owner = self.owner()
        actual, root = self.native.soac_pending_type_construct(
            "NoExecutionPins", (), {}, function, owner,
        )
        del token, namespace, function
        self.assertIsNone(function_ref())
        self.assertIsNone(token_ref())
        self.assertEqual(self.info(actual).root_construction, id(root))
        self.admit(actual, owner, root)
        type_ref = weakref.ref(actual)
        del actual
        gc.collect()
        self.assertIsNone(type_ref())
        # Resolved canonical handle remains a metadata identity, not a type pin.
        self.assertEqual(self.fail_pending(root), 0)


    def test_final_admission_is_sealed_before_first_c_caller_release(self):
        annotations, releases, unraisable = [], [], []
        case = self
        is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
        is_sealed.argtypes = [ctypes.py_object]
        is_sealed.restype = ctypes.c_int

        def annotate(format):
            annotations.append(format)
            return {"field": int}

        actual, owner, root = self.pending({"__annotate_func__": annotate})
        namespace = self.get_dict(actual)

        class Retired:
            def __del__(self):
                # Capture outcomes, not assertions hidden in an unraisable hook.
                row = [case.info(actual).phase, is_sealed(actual), list(annotations)]
                try:
                    value = actual()
                except BaseException as error:  # noqa: BLE001 -- recorded below
                    row.append(error)
                else:
                    row.append(type(value) is actual)
                for store in (
                    lambda: type.__setattr__(actual, "late", 1),
                    lambda: namespace.__setitem__("late", 1),
                ):
                    try:
                        store()
                    except BaseException as error:  # noqa: BLE001 -- recorded below
                        row.append(type(error))
                    else:
                        row.append(None)
                try:
                    row.append(actual.__annotations__)
                except BaseException as error:  # noqa: BLE001 -- recorded below
                    row.append(error)
                releases.append(row)

        import sys
        previous_hook = sys.unraisablehook
        sys.unraisablehook = unraisable.append
        holder = [Retired()]
        try:
            self.native.soac_pending_type_admit_and_retire(
                (actual, owner, root, (), (), (), None), holder,
            )
        finally:
            sys.unraisablehook = previous_hook
        self.assertEqual(holder, [None])
        self.assertEqual(unraisable, [])
        self.assertEqual(releases, [
            [3, 1, [], True, self.mutation_error, self.mutation_error, {"field": int}],
        ])
        self.assertEqual(annotations, [1])
        self.assertNotIn("late", namespace)
        self.assertEqual(is_sealed(actual), 1)
        self.assertEqual(self.seal(actual, owner), 0)  # existing API remains idempotent
        self.assertIs(actual.__annotations__, releases[0][-1])

    def test_failed_final_admission_keeps_barrier_during_first_c_caller_release(self):
        primary = LookupError("captured owner commit failed")
        owner = self.owner(error=primary)
        actual, _, root = self.pending(owner=owner)
        case, releases, unraisable = self, [], []

        class Retired:
            def __del__(self):
                row = [case.info(actual).phase]
                try:
                    actual()
                except BaseException as error:  # noqa: BLE001 -- recorded below
                    row.append(type(error))
                else:
                    row.append(None)
                releases.append(row)

        import sys
        previous_hook = sys.unraisablehook
        sys.unraisablehook = unraisable.append
        holder = [Retired()]
        try:
            with self.assertRaises(LookupError) as raised:
                self.native.soac_pending_type_admit_and_retire(
                    (actual, owner, root, (), (), (), None), holder,
                )
        finally:
            sys.unraisablehook = previous_hook
        self.assertIs(raised.exception, primary)
        self.assertIsNone(raised.exception.__cause__)
        self.assertIsNone(raised.exception.__context__)
        self.assertEqual(holder, [None])
        self.assertEqual(unraisable, [])
        self.assertEqual(releases, [[4, self.mutation_error]])
        with self.assertRaises(self.mutation_error):
            actual()


if __name__ == "__main__":
    unittest.main()
