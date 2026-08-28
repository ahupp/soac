"""Behavior at the genuine offline-artifact/native-construction boundary."""

import json
import sys
from pathlib import Path

import pytest

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import create_strict_project


_CPYTHON_CLASS_CONSTRUCTION = """
import ctypes
from soac import _soac_ext
from tests.test_strict_type_native import ConstructionInfoV1

get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
get_type_owner.argtypes = [ctypes.py_object]
get_type_owner.restype = ctypes.c_void_p
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int

def assert_native_class(cls):
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == get_type_owner(cls) and info.owner is not None
    return info.owner
"""


@pytest.fixture(scope="module")
def cached_empty_annotations(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-empty-annotation-cache"),
        {
            "empty_annotation_cache.py": """
# soac: module(strict_assign=true, checked_attr=true)
from annotationlib import get_annotations

class Plain:
    def method(self, value: int) -> int:
        return value

# Introspection of an unannotated class lazily publishes native cache entries,
# including __annotate_func__ = None, before module sealing.
assert Plain.__annotate__ is None
assert Plain.__annotations__ == {}
assert get_annotations(Plain) == {}

class Annotated:
    value: int = 1

assert get_annotations(Annotated) == {'value': int}
""",
        },
        modules={"empty_annotation_cache": "empty_annotation_cache.py"},
        backend=getattr(request, "param", "soac"),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_native_class_construction_retains_split_key_profile_observation(
    cached_empty_annotations, tmp_path, entry_interpreter
):
    """The real class path still observes ordinary dictionary key insertion."""
    work = tmp_path / "soac-work"
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    cached_empty_annotations.run(
        f"""
import ctypes
import empty_annotation_cache as module

sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert sealed(module.Plain) == 1
assert _soac_ext.strict_function_entry_kind(module.Plain.method) == {expected_entry!r}
instance = module.Plain()
instance.first = 11
instance.second = 12
assert vars(instance) == {{'first': 11, 'second': 12}}
assert instance.method(3) == 3
""",
        opt_mode="profile",
        entry_interpreter=entry_interpreter,
        extra_env={"SOAC_WORK_DIR": str(work)},
    )
    from soac import _soac_ext

    dump = json.loads(_soac_ext.inspect_counter_dump_json(str(work / "profile.bin")))
    owner_ids = {
        owner["type_id"]
        for record in dump["records"]
        for owner in record["type_table"]
        if owner["module_name"] == "empty_annotation_cache"
        and owner["qualname"] == "Plain"
    }
    assert owner_ids, "native class construction did not install the key-layout observer"
    keys = {
        item["key"]: item["index"]
        for record in dump["records"]
        for item in record["type_keys"]
        if item["owner_type_id"] in owner_ids
    }
    assert keys["first"] < keys["second"], keys


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_native_empty_annotation_cache_is_not_a_foreign_class_provider(
    cached_empty_annotations, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    cached_empty_annotations.run(
        f"""
import ctypes
import empty_annotation_cache as module

sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert sealed(module.Plain) == 1 and sealed(module.Annotated) == 1
assert vars(module.Plain)['__annotate_func__'] is None
assert module.Plain.__annotations__ == {{}}
assert module.Annotated.__annotations__ == {{'value': int}}
assert _soac_ext.strict_function_entry_kind(module.Plain.method) == {expected_entry!r}
assert module.Plain().method(3) == 3
assert module.Plain().method('ordinary argument') == 'ordinary argument'
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("cached_empty_annotations", "entry_interpreter"),
    [("soac", False), ("soac", True), ("cpython", False)],
    indirect=["cached_empty_annotations"],
    ids=["False", "True", "cpython"],
)
def test_ordinary_class_cannot_gain_or_drop_a_transitive_strict_ancestor(
    cached_empty_annotations, entry_interpreter
):
    expected = (
        "original_code" if cached_empty_annotations.backend == "cpython"
        else "entry_interpreter" if entry_interpreter else "checked_native"
    )
    program = f"""
import ctypes
import empty_annotation_cache as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
set_attr = ctypes.pythonapi.PyObject_SetAttr
set_attr.argtypes = [ctypes.py_object] * 3
set_attr.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert owner(module.Plain)
assert _soac_ext.strict_function_entry_kind(module.Plain.method) == {expected!r}
assert module.Plain().method(7) == 7

class OrdinaryBase:
    pass
class OrdinaryAlternative(OrdinaryBase):
    pass
class Middle(module.Plain):
    pass
class Leaf(Middle):
    pass
class Victim(OrdinaryBase):
    pass
assert not owner(Middle) and not owner(Leaf) and not owner(Victim)
victim = Victim()
victim.method = 'ordinary shadow before a class transition'
dictionary = vars(victim)
leaf = Leaf()

# Ordinary-only MRO changes remain supported.
Victim.__bases__ = (OrdinaryAlternative,)
Victim.__bases__ = (OrdinaryBase,)

def rejected(operation):
    try:
        operation()
    except StrictMutationError:
        return
    raise AssertionError('an ordinary intermediate class bypassed strict ancestry')

for setter in (setattr, type.__setattr__, set_attr):
    before_bases, before_mro = Victim.__bases__, Victim.__mro__
    rejected(lambda: setter(Victim, '__bases__', (Middle,)))
    assert Victim.__bases__ is before_bases and Victim.__mro__ is before_mro
    assert type(victim) is Victim and vars(victim) is dictionary
    assert victim.method == 'ordinary shadow before a class transition'
    before_bases, before_mro = Leaf.__bases__, Leaf.__mro__
    rejected(lambda: setter(Leaf, '__bases__', (OrdinaryBase,)))
    assert Leaf.__bases__ is before_bases and Leaf.__mro__ is before_mro
    assert leaf.method(9) == 9

for setter in (setattr, object.__setattr__, set_attr):
    rejected(lambda: setter(victim, '__class__', Middle))
    rejected(lambda: setter(leaf, '__class__', Victim))
    assert type(victim) is Victim and type(leaf) is Leaf
"""
    if cached_empty_annotations.backend == "cpython":
        cached_empty_annotations.run_case(
            "empty_annotation_cache",
            _CPYTHON_CLASS_CONSTRUCTION + program + """
assert_native_class(module.Plain)
assert _soac_ext.strict_function_diagnostics(module.Plain.method)["finalized"]
for cls in (Middle, Leaf, Victim):
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
""",
            Path(__file__),
            required_functions=("Plain.method",),
            
        )
    else:
        cached_empty_annotations.run(program, entry_interpreter=entry_interpreter)


@pytest.fixture(scope="module")
def explicit_object_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-explicit-object"),
        {
            "explicit_object.py": """
# soac: module(strict_assign=true, checked_attr=true)
from builtins import object as root_object

class Direct(object):
    value: int = 1

    def read(self) -> int:
        return self.value

class Aliased(root_object):
    value: int = 2

    def read(self) -> int:
        return self.value
""",
        },
        modules={"explicit_object": "explicit_object.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_explicit_builtin_object_base_installs_a_real_class_contract(
    explicit_object_project, entry_interpreter
):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    explicit_object_project.run(
        f"""
import _testinternalcapi
import ctypes
import explicit_object as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(module)['sealed']
for cls, default in ((module.Direct, 1), (module.Aliased, 2)):
    assert cls.__bases__ == (object,)
    assert owner(cls) and sealed(cls), 'a builtin base was treated as an unknown user type'
    assert _soac_ext.strict_function_entry_kind(cls.read) == {expected!r}
    instance = cls()
    dictionary = vars(instance)
    assert dictionary == {{}} and instance.read() == default
    assert _testinternalcapi.dict_has_indexed_keys(dictionary) is False, (
        'pending source storage must not acquire an indexed layout'
    )
    instance.value = 7
    assert instance.read() == 7 and list(dictionary) == ['value']
    assert _testinternalcapi.dict_has_indexed_keys(dictionary) is False
    try:
        instance.read = object()
    except StrictMutationError:
        pass
    else:
        raise AssertionError('explicit-object class did not protect its methods')
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def explicit_slots_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-explicit-slots"),
        {
            "slot_model.py": """
# soac: module(strict_assign=true, checked_attr=true)
import slot_support

class Probe:
    __slots__ = ()

    def __init_subclass__(cls):
        slot_support.observe(cls)

class Base(Probe):
    __slots__ = ('value', '__weakref__')
    value: int

    def __init__(self, value: int):
        self.value = value

    def read(self) -> int:
        return self.value

class Child(Base):
    __slots__ = ('other',)
    other: str

    def __init__(self, value: int, other: str):
        self.value = value
        self.other = other

    def text(self) -> str:
        return self.other

class WithDictionary(Base):
    extra: int

    def set_extra(self, value: int):
        self.extra = value
""",
            "slot_support.py": """
observations = []
ordinary_observations = []
phase = 'pending'

def observe(cls):
    import ctypes
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    from soac.strict import StrictMutationError
    if phase == 'ordinary_subclass':
        # The ordinary driver selects this phase explicitly after strict module
        # initialization. Absence of an owner never selects the phase: Pending
        # source classes also have no permanent owner at this callback.
        assert not owner(cls), 'ordinary subclass acquired its own type contract'
        assert type(object.__new__(cls)) is cls, 'ordinary subclass retained a pending barrier'
        ordinary_observations.append(cls)
        return
    assert phase == 'pending', phase
    assert not owner(cls), 'the provisional type acquired a permanent contract'
    try:
        object.__new__(cls)
    except StrictMutationError:
        blocked = True
    else:
        raise AssertionError('a pending slots type admitted an instance')
    observations.append((cls, bool(owner(cls)), bool(cls.__dictoffset__), blocked))
""",
        },
        modules={"slot_model": "slot_model.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_source_requested_slots_keep_real_members_pending_until_final_admission(
    explicit_slots_project, entry_interpreter
):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    explicit_slots_project.run(
        f"""
import ctypes
import types
import weakref
import slot_model as model
import slot_support as support
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object] * 3
generic_set.restype = ctypes.c_int

assert _soac_ext.strict_module_diagnostics(model)['sealed']
for cls in (model.Probe, model.Base, model.Child, model.WithDictionary):
    assert owner(cls) and sealed(cls), ('explicit slots silently declined', cls)
assert support.observations == [
    (model.Base, False, False, True),
    (model.Child, False, False, True),
    (model.WithDictionary, False, True, True),
], support.observations
assert _soac_ext.strict_function_entry_kind(model.Base.read) == {expected!r}
assert _soac_ext.strict_function_entry_kind(model.Child.text) == {expected!r}
assert type(vars(model.Base)['value']) is types.MemberDescriptorType
assert type(vars(model.Child)['other']) is types.MemberDescriptorType
base, child = model.Base(3), model.Child(4, 'ok')
assert not hasattr(base, '__dict__') and not hasattr(child, '__dict__')
assert weakref.ref(base)() is base and weakref.ref(child)() is child
assert base.read() == 3 and child.read() == 4 and child.text() == 'ok'

def rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('physical slot write missed its required check')

for setter in (setattr, object.__setattr__, generic_set):
    rejected(lambda: setter(base, 'value', 'wrong'))
    rejected(lambda: setter(child, 'value', 'wrong'))
    rejected(lambda: setter(child, 'other', 9))
    setter(child, 'value', 6)
    assert child.read() == 6
descriptor = vars(model.Base)['value']
rejected(lambda: descriptor.__set__(child, 'wrong'))
descriptor.__delete__(child)
try:
    child.read()
except AttributeError:
    pass
else:
    raise AssertionError('an unbound native slot became an initialized field')
descriptor.__set__(child, 7)
assert child.read() == 7

support.phase = 'ordinary_subclass'
try:
    class Ordinary(model.Child):
        pass
finally:
    support.phase = 'pending'
assert support.ordinary_observations == [Ordinary]

ordinary = Ordinary(8, 'ordinary')
assert not owner(Ordinary)
rejected(lambda: descriptor.__set__(ordinary, 'wrong'))
rejected(lambda: setattr(ordinary, 'other', 4))
assert ordinary.read() == 8
ordinary.extra = object()

# Plain CPython driver bytecode is warmed independently of transformed bodies.
# LOAD_ATTR_SLOT / STORE_ATTR_SLOT must use the same physical policy.
def warmed(receiver, value):
    receiver.value = value
    return receiver.value

for i in range(200):
    assert warmed(child, i) == i
rejected(lambda: warmed(child, 'wrong'))
assert child.value == 199

dictionary = model.WithDictionary(10)
dictionary.set_extra(11)
assert type(vars(dictionary)) is dict and vars(dictionary) == {{'extra': 11}}
vars(dictionary)['value'] = 'hidden'  # Independent mapping entry, not the slot.
assert dictionary.value == 10 and vars(dictionary)['value'] == 'hidden'
rejected(lambda: setattr(dictionary, 'value', 'wrong'))
""",
        entry_interpreter=entry_interpreter,
    )


def test_native_slot_reads_select_guarded_members_and_keep_lookup_fallback(
    explicit_slots_project, tmp_path
):
    work = tmp_path / "native-slot-reads"
    training = """
import ctypes
import slot_model as model

sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(model)['sealed']
assert sealed(model.Base) and sealed(model.Child)
assert _soac_ext.strict_function_entry_kind(model.Base.read) == 'checked_native'
assert _soac_ext.strict_function_entry_kind(model.Child.text) == 'checked_native'
base, child = model.Base(7), model.Child(11, 'member')
for unused in range(250):
    assert base.read() == 7 and child.text() == 'member'
"""
    explicit_slots_project.run(
        training, opt_mode="profile", extra_env={"SOAC_WORK_DIR": str(work)}
    )
    assert (work / "profile.bin").is_file()
    validation = training + """
base.value = 19
assert base.read() == 19
del base.value
try:
    base.read()
except AttributeError:
    pass
else:
    raise AssertionError('an unbound native member skipped normal lookup')
base.value = 23
assert base.read() == 23

# A nominal receiver is not an exact layout witness. An ordinary subclass
# may override the descriptor; the guarded load must run that lookup.
import slot_support as support
events = []
support.phase = 'ordinary_subclass'
try:
    class Ordinary(model.Base):
        @property
        def value(self):
            events.append('property')
            return 31
finally:
    support.phase = 'pending'
assert support.ordinary_observations == [Ordinary]

ordinary = object.__new__(Ordinary)
assert ordinary.read() == 31 and events == ['property']
# An admitted derived type also keeps the conservative exact-owner fallback.
assert child.read() == 11
"""
    events_path = tmp_path / "native-slot-apply.jsonl"
    explicit_slots_project.run(
        validation,
        opt_mode="apply",
        extra_env={
            "SOAC_WORK_DIR": str(work),
            "SOAC_LOG": f"soac_jit_codegen=info;json={events_path}",
        },
    )
    events = [json.loads(line) for line in events_path.read_text().splitlines()]
    events = [entry.get("fields", entry) for entry in events]
    emitted = {
        event["function_qualname"]: event
        for event in events
        if event.get("event") == "soac.strict_field_codegen"
    }
    bound = {
        event["function_qualname"]: event
        for event in events
        if event.get("event") == "soac.strict_field_capabilities"
    }
    for name in ("Base.read", "Child.text"):
        assert emitted[name]["sealed_field_site_count"] == 1
        assert emitted[name]["machine_code_size_bytes"] > 0
        assert bound[name]["native_object_slot_count"] == 1
        assert bound[name]["indexed_dictionary_slot_count"] == 0

    # Verify records branch use separately from the production apply run.
    explicit_slots_project.run(
        validation, opt_mode="verify", extra_env={"SOAC_WORK_DIR": str(work)}
    )
    from soac import _soac_ext

    verification = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work / "verify.bin"))
    )
    read_paths = {
        branch
        for record in verification["records"]
        if record["module_name"] == "slot_model"
        for row in record["rows"]
        if row["function_qualname"] == "Base.read"
        and row["kind"] == "field_access"
        for branch, value in row["branches"].items()
        if value > 0
    }
    # These existing counter names cover both indexed-dictionary and native
    # object-member capabilities; the binding event above proves the kind.
    assert {"indexed_hit", "indexed_fallback"} <= read_paths


@pytest.mark.parametrize(
    ("backend", "entry_interpreter"),
    [("soac", False), ("soac", True), ("cpython", False)],
    ids=["False", "True", "cpython"],
)
def test_strict_class_storage_and_mutation_boundaries(tmp_path, backend, entry_interpreter):
    project = create_strict_project(
        tmp_path,
        {
            "model.py": """
# soac: module(strict_assign=true, checked_attr=true)

class Box:
    value: int = 0

    def __init__(self, value: int):
        self.value = value

    def method(self) -> int:
        return self.value + 1
""",
        },
        modules={"model": "model.py"},
        backend=backend,
    )
    validation = """
import ctypes
import model
from soac.strict import StrictMutationError

def rejected(operation):
    try:
        operation()
    except StrictMutationError:
        return
    raise AssertionError('protected mutation unexpectedly succeeded')

box = model.Box(3)
storage = vars(box)
assert type(storage) is dict and storage is box.__dict__
assert list(storage) == ['value']
storage['method'] = 'hidden dictionary value'
assert box.method() == 4
assert object.__getattribute__(box, 'method')() == 4
assert storage['method'] == 'hidden dictionary value'

def ordinary_access(value):
    value.value = value.value + 1
    return value.method()

for unused in range(2000):
    ordinary_access(box)
assert storage['value'] == 2003 and box.method() == 2004
rejected(lambda: setattr(box, 'method', lambda: -1))
rejected(lambda: object.__setattr__(box, 'method', lambda: -1))
# Class sealing does not select indexed storage or reject ordinary replacement.
# The incoming object becomes the actual dictionary; the escaped old alias is
# neither copied into nor kept authoritative by the class's method policy.
escaped_storage = storage
incoming_storage = dict(storage)
box.__dict__ = incoming_storage
assert vars(box) is incoming_storage and box.__dict__ is incoming_storage
assert escaped_storage is not incoming_storage
escaped_storage['value'] = -1000
assert box.value == 2003 and box.method() == 2004
assert escaped_storage['value'] == -1000
storage = incoming_storage

set_attr = ctypes.pythonapi.PyObject_SetAttr
set_attr.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
set_attr.restype = ctypes.c_int
rejected(lambda: set_attr(box, 'method', object()))
set_item = ctypes.pythonapi.PyDict_SetItem
set_item.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
set_item.restype = ctypes.c_int
assert set_item(storage, 'value', 11) == 0
assert box.method() == 12
assert set_item(storage, 'method', 'still hidden') == 0
assert box.method() == 12

copied = storage.copy()
assert type(copied) is dict and copied is not storage
copied.clear()
assert box.value == 11
storage.clear()
assert storage is vars(box) and not storage and box.value == 0
storage['other'] = 17
storage['value'] = 5
assert list(storage) == ['other', 'value'] and box.method() == 6
del box.value
assert box.value == 0 and list(storage) == ['other']
box.value = 8
assert box.method() == 9 and storage['value'] == 8

class Ordinary:
    pass
rejected(lambda: setattr(box, '__class__', Ordinary))
rejected(lambda: setattr(model.Box, 'method', lambda self: 99))
rejected(lambda: setattr(model.Box.method, '__code__', ordinary_access.__code__))
rejected(lambda: setattr(model, 'Box', Ordinary))
rejected(lambda: set_item(vars(model), 'Box', Ordinary))

class_dict = ctypes.pythonapi.PyType_GetDict
class_dict.argtypes = [ctypes.py_object]
class_dict.restype = ctypes.py_object
rejected(lambda: set_item(class_dict(model.Box), 'method', object()))
assert box.method() == 9
"""
    if backend == "cpython":
        project.run_case(
            "model", _CPYTHON_CLASS_CONSTRUCTION + validation + """
assert_native_class(model.Box)
for function in (model.Box.__init__, model.Box.method):
    assert _soac_ext.strict_function_diagnostics(function)["finalized"]
""",
            Path(__file__),
            required_functions=("Box.__init__", "Box.method"),
            
        )
    else:
        project.run(validation, entry_interpreter=entry_interpreter)


@pytest.fixture(scope="module")
def pending_method_calls_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-pending-method-calls"),
        {
            "support.py": """
events = []

def observe(cls):
    from soac.strict import StrictMutationError
    try:
        object.__new__(cls)
    except StrictMutationError:
        events.append(('pending', cls.__name__))
    else:
        raise AssertionError('callback allocated an unfinished source type')
    class Foreign:
        value = 'wrong return type'
    assert cls.method(Foreign()) == 'wrong return type'
    events.append(('ordinary-result', cls.__name__))
""",
            "model.py": """
# soac: module(strict_assign=true, checked_attr=true)
import support

class Base:
    def __init_subclass__(cls):
        support.observe(cls)

class Child(Base):
    value: int = 7

    def method(self) -> int:
        return self.value
""",
        },
        modules={"model": "model.py"},
    )


def test_pending_allocation_and_ordinary_method_calls_precede_init_subclass(
    pending_method_calls_project,
):
    pending_method_calls_project.run(
        """
import model
import support
from soac.strict import StrictMutationError
assert support.events == [('pending', 'Child'), ('ordinary-result', 'Child')]
instance = model.Child()
storage = vars(instance)
assert type(storage) is dict and instance.method() == 7
storage['method'] = 'hidden dictionary value'
assert instance.method() == 7
try:
    instance.method = 'forbidden shadow'
except StrictMutationError:
    pass
else:
    raise AssertionError('admitted type lost its protected method')
# Ordinary calls keep their value semantics after admission as well. This
# foreign receiver has no selected storage, unlike the real Child instance.
class Foreign:
    value = 'wrong return type'
assert model.Child.method(Foreign()) == 'wrong return type'
assert instance.value == 7
"""
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_pending_class_completion_installs_checks_on_actual_field_writes(
    pending_method_calls_project, entry_interpreter
):
    pending_method_calls_project.run(
        """
import ctypes
import model
import support
from soac.strict import StrictMutationError

assert support.events == [('pending', 'Child'), ('ordinary-result', 'Child')]
owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert owner(model.Child)
instance = model.Child()
storage = vars(instance)
for write in (
    lambda: setattr(instance, 'value', 'wrong return type'),
    lambda: object.__setattr__(instance, 'value', 'wrong return type'),
    lambda: storage.__setitem__('value', 'wrong return type'),
):
    try:
        write()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
    else:
        raise AssertionError('completed class did not constrain its real field')
    assert instance.value == 7 and instance.method() == 7
    assert storage == {}
instance.value = 9
assert instance.method() == 9 and storage == {'value': 9}
""",
        entry_interpreter=entry_interpreter,
    )


def test_sealed_class_and_detached_dictionary_preserve_object_lifetimes(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "support.py": """
events = []

class Token:
    def __del__(self):
        events.append('token released')
""",
            "model.py": """
# soac: module(strict_assign=true, checked_attr=true)
import support

token = support.Token()

class Bare:
    value = 3
""",
        },
        modules={"model": "model.py"},
    )
    project.run(
        """
import gc
import sys
import weakref
import model
import support
from soac.strict import StrictMutationError

bare = model.Bare
module_ref = weakref.ref(model)
token_ref = weakref.ref(model.token)
del sys.modules['model']
del model
gc.collect()
assert module_ref() is None
assert token_ref() is None and support.events == ['token released']
instance = bare()
assert instance.value == 3
instance.value = 10
dictionary = vars(instance)
type_ref = weakref.ref(bare)
instance_ref = weakref.ref(instance)
del instance
assert instance_ref() is None
del bare
gc.collect()
assert type_ref() is None
assert dictionary == {'value': 10}
dictionary['value'] = 11
dictionary.clear()
assert dictionary == {}
"""
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_escaped_derived_class_namespace_does_not_retain_its_type(tmp_path, entry_interpreter):
    body = """
class Base:
    pass

def make():
    class Child(Base):
        pass
    return Child
"""
    project = create_strict_project(
        tmp_path,
        {"model.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + body},
        modules={"model": "model.py"},
    )
    project.run(
        f"""
import ctypes
import gc
import weakref
import model

is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int

# The control executes the same class/factory declarations as ordinary code.
# A derived methodless class has no own __dict__ descriptor retaining its type.
ordinary = {{'__name__': 'ordinary_lifetime_control'}}
exec(compile({body!r}, '<ordinary-lifetime-control>', 'exec', dont_inherit=True), ordinary)

def collect_with_escaped_dictionary(factory, expected_sealed, class_namespace):
    cls = factory()
    assert is_sealed(cls) == expected_sealed
    assert '__dict__' not in vars(cls)
    events = []
    reference = weakref.ref(cls, lambda unused: events.append('class released'))
    if class_namespace:
        escaped = vars(cls)
        assert escaped
    else:
        instance = cls()
        escaped = vars(instance)
        del instance
    del cls
    gc.collect()
    return reference() is None, events, dict(escaped)

for class_namespace in (False, True):
    expected = collect_with_escaped_dictionary(ordinary['make'], 0, class_namespace)
    assert expected == (True, ['class released'], {{}}), expected
    actual = collect_with_escaped_dictionary(model.make, 1, class_namespace)
    assert actual == expected, (class_namespace, actual, expected)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("backend", ["soac", "cpython"])
def test_unknown_decorators_and_metaclasses_remain_ordinary(tmp_path, backend):
    project = create_strict_project(
        tmp_path,
        {
            "framework.py": """
def instrument(cls):
    cls.instrumented = True
    return cls

class Meta(type):
    pass
""",
            "model.py": """
# soac: module(strict_assign=true, checked_attr=true)
from framework import Meta, instrument

@instrument
class Decorated:
    value: int = 1

    def method(self):
        return 1

class Managed(metaclass=Meta):
    value: int = 2

    def method(self):
        return 2
""",
        },
        modules={"model": "model.py"},
        backend=backend,
    )
    validation = """
from pathlib import Path
import sys
import types
import model

# The exact same class/decorator/metaclass source, with only strict opt-in
# removed, establishes ordinary function code/closure compatibility.
stock = types.ModuleType('ordinary_unknown_class_control')
sys.modules[stock.__name__] = stock
source = Path(model.__file__).read_text()
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary unknown class control>', 'exec'), vars(stock))

def replacement(self):
    return 42

def replacement_annotations(format, runtime=None, type_params=()):
    return {'value': str}

def annotation_replacement():
    namespace_marker = None

    def compatible_annotations(format, /):
        # The existing provider's one class-namespace cell is retained.
        namespace_marker
        return {'value': str}

    return compatible_annotations

compatible = annotation_replacement()
assert compatible.__code__.co_argcount == compatible.__code__.co_posonlyargcount == 1
assert len(compatible.__code__.co_freevars) == 1
assert replacement_annotations.__code__.co_freevars == ()

for cls in (stock.Decorated, stock.Managed):
    provider = vars(cls)['__annotate_func__']
    previous_code, previous_closure = provider.__code__, provider.__closure__
    assert len(previous_closure) == 1
    try:
        provider.__code__ = replacement_annotations.__code__
    except ValueError:
        pass
    else:
        raise AssertionError('ordinary provider accepted incompatible closure arity')
    assert provider.__code__ is previous_code
    assert provider.__closure__ is previous_closure

for actual in (stock, model):
    for cls in (actual.Decorated, actual.Managed):
        original = cls.method
        original.__code__ = replacement.__code__
        assert cls().method() == 42
        cls.method = lambda self: 9
        instance = cls()
        instance.method = lambda: 17
        assert instance.method() == 17
        instance.__dict__ = {'value': 31}
        assert vars(instance) == {'value': 31}
        provider = vars(cls)['__annotate_func__']
        previous_closure, previous_defaults = provider.__closure__, provider.__defaults__
        assert len(previous_closure) == len(compatible.__code__.co_freevars)
        provider.__code__ = compatible.__code__
        assert provider.__code__ is compatible.__code__
        assert provider.__closure__ is previous_closure
        assert provider.__defaults__ is previous_defaults
        assert cls.__annotations__ == {'value': str}
    assert actual.Decorated.instrumented
"""
    if backend == "cpython":
        native_before = """
import ctypes
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
get_type_owner.argtypes = [ctypes.py_object]
get_type_owner.restype = ctypes.c_void_p
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p

import model
module_witness = _soac_ext.strict_module_diagnostics(model)
originals = []
call_no_args = ctypes.pythonapi.PyObject_CallNoArgs
call_no_args.argtypes = [ctypes.py_object]
call_no_args.restype = ctypes.py_object
for cls, expected in ((model.Decorated, 1), (model.Managed, 2)):
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
    assert get_type_owner(cls) is None
    method = vars(cls)["method"]
    provider = vars(cls)["__annotate_func__"]
    for function in (method, provider):
        witness = _assert_cpython_function_witness(
            function, module_witness,
        )
        assert witness["finalized"] is False
    instance = cls()
    for _ in range(128):
        assert instance.method() == expected
    assert call_no_args(instance.method) == expected
    assert _soac_ext.strict_function_diagnostics(method)["original_code_entered"]
    originals.append((cls, method, provider))
"""
        native_after = """
# The original source functions remain ordinary after the same metadata writes
# exercised above. A changed code object must not retain source-body authority.
for cls, method, provider in originals:
    for function in (method, provider):
        witness = _soac_ext.strict_function_diagnostics(function)
        assert witness is not None
        assert witness["schema"] == 2 and witness["backend"] == "cpython"
        assert witness["entry_kind"] == "ordinary_replacement"
        assert witness["finalized"] is False
        for key in (
            "source_path", "source_sha256", "artifact_generation",
            "startup_identity", "interpreter_id",
        ):
            assert witness[key] == module_witness[key]
        assert metadata(function) is None
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
    assert get_type_owner(cls) is None
"""
        project.run_case(
            "model", native_before + validation + native_after, Path(__file__),
            backend="cpython",
        )
    else:
        project.run(validation)


def test_public_strict_errors_are_native_shared_classes(tmp_path):
    project = create_strict_project(
        tmp_path,
        {"model.py": "# soac: module(strict_assign=true, checked_attr=true)\nvalue = 1\n"},
        modules={"model": "model.py"},
    )
    project.run(
        """
import pickle
import soac
import soac.strict
import _soac_ext
import model

for name, base in [('StrictMutationError', TypeError), ('StrictRuntimeUnavailableError', ImportError)]:
    exception = getattr(soac.strict, name)
    assert exception is getattr(soac, name) is getattr(_soac_ext, name)
    assert issubclass(exception, base)
    assert type(pickle.loads(pickle.dumps(exception('message')))) is exception
    try:
        exception.changed = True
    except TypeError:
        pass
    else:
        raise AssertionError('native strict exception class is mutable')
try:
    model.value = 2
except soac.strict.StrictMutationError:
    pass
else:
    raise AssertionError('module mutation did not use the shared exception')
"""
    )


@pytest.mark.parametrize(
    ("backend", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
)
def test_same_source_method_from_earlier_dynamic_class_is_not_adopted(
    tmp_path, backend, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "support.py": """
created = []

class UnexpectedDescriptor:
    def __get__(self, instance, owner):
        return 0

def replacement(self, value):
    return ('ordinary replacement', value)

def rewrite(namespace):
    created.append(namespace['method'])
    if len(created) == 1:
        # Only this execution is dynamic; the source class remains eligible.
        namespace['unexpected'] = UnexpectedDescriptor()
    elif len(created) == 2:
        # Same source/code, but owned by the earlier dynamic class execution.
        namespace['method'] = created[0]
    elif len(created) == 4:
        # An unadmitted class-owned method remains instrumentable before the
        # class decision, even though its source annotations are supported.
        namespace['method'].__code__ = replacement.__code__
""",
            "model.py": """
# soac: module(strict_assign=true, checked_attr=true)
import support

def make():
    class Box:
        def method(self, value: int = 1) -> int:
            return value

        support.rewrite(locals())
    return Box
""",
        },
        modules={"model": "model.py"},
        backend=backend,
    )
    validation = """
import ctypes
import model
import support

is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int
function_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
function_identity.argtypes = [ctypes.py_object]
function_identity.restype = ctypes.c_uint64

first = model.make()
assert is_sealed(first) == 0
original = vars(first)['method']
assert function_identity(original) == 0
assert first().method('dynamic argument') == 'dynamic argument'
# Runtime decline must not seal a function and later revoke its protection.
# Even a same-code write remains legal here.
original.__code__ = original.__code__
original.__defaults__ = (11,)
assert first().method() == 11

second = model.make()
assert vars(second)['method'] is original
# Adoption must not freeze a function shared with the earlier dynamic class.
original.__defaults__ = (23,)
assert first().method() == second().method() == 23
assert is_sealed(second) == 0

fresh = model.make()
assert is_sealed(fresh) == 1
assert function_identity(fresh.method) != 0
assert fresh.method is support.created[2] and fresh.method is not original
assert fresh.method.__code__ is original.__code__
assert fresh().method() == 1 and first().method() == 23
assert fresh().method('not an integer') == 'not an integer'

changed = model.make()
assert is_sealed(changed) == 0
assert function_identity(changed.method) == 0
assert changed().method('dynamic') == ('ordinary replacement', 'dynamic')
"""
    if backend == "cpython":
        native_after = """
import ctypes
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
get_type_owner.argtypes = [ctypes.py_object]
get_type_owner.restype = ctypes.c_void_p
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p

module_witness = _soac_ext.strict_module_diagnostics(model)
make_witness = _assert_cpython_function_witness(
    model.make, module_witness,
)
assert make_witness["original_code_entered"]
first_witness = _assert_cpython_function_witness(
    original, module_witness,
)
assert first_witness["finalized"] is False
assert first_witness["original_code_entered"]
fresh_witness = _assert_cpython_function_witness(
    fresh.method, module_witness,
)
assert fresh_witness["finalized"] and fresh_witness["original_code_entered"]
assert fresh_witness["native_code_ordinal"] == first_witness["native_code_ordinal"]
changed_witness = _soac_ext.strict_function_diagnostics(changed.method)
assert changed_witness is not None
assert changed_witness["schema"] == 2 and changed_witness["backend"] == "cpython"
assert changed_witness["entry_kind"] == "ordinary_replacement"
assert changed_witness["finalized"] is False
assert changed_witness["native_code_ordinal"] == first_witness["native_code_ordinal"]
for key in (
    "source_path", "source_sha256", "artifact_generation",
    "startup_identity", "interpreter_id",
):
    assert changed_witness[key] == module_witness[key]
assert metadata(changed.method) is None
for cls in (first, second, changed):
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
    assert get_type_owner(cls) is None

info = ConstructionInfoV1()
assert get_construction(fresh, ctypes.byref(info), ctypes.sizeof(info)) == 1
assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
assert info.phase == 3 and info.permanent_contract_published == 1
assert info.owner == get_type_owner(fresh) and info.owner is not None

dynamic_instance = first()
checked_instance = fresh()
for _ in range(128):
    assert dynamic_instance.method("ordinary") == "ordinary"
    assert checked_instance.method(7) == 7
call_one = ctypes.pythonapi.PyObject_CallOneArg
call_one.argtypes = [ctypes.py_object, ctypes.py_object]
call_one.restype = ctypes.py_object
assert call_one(dynamic_instance.method, "ordinary C") == "ordinary C"
assert call_one(checked_instance.method, 8) == 8
assert call_one(checked_instance.method, "ordinary C") == "ordinary C"
"""
        project.run_case(
            "model", validation + native_after, Path(__file__),
            required_functions=("make",), 
            backend="cpython",
        )
    else:
        project.run(validation, entry_interpreter=entry_interpreter)


@pytest.mark.parametrize(
    ("backend", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
)
def test_active_class_method_call_keeps_ordinary_semantics_across_admission(
    tmp_path, backend, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "admission_support.py": """
import threading

entered = threading.Event()
released = threading.Event()
outcomes = []
worker = None

def pause():
    entered.set()
    if not released.wait(10):
        raise AssertionError('class construction did not release active method')

def begin(function):
    global worker
    def call():
        try:
            outcomes.append(function(None, 'already active'))
        except BaseException as error:
            outcomes.append(error)
            entered.set()
    worker = threading.Thread(target=call)
    worker.start()
    if not entered.wait(10):
        released.set()
        worker.join(10)
        raise AssertionError('source method did not reach its body')

def finish():
    released.set()
    worker.join(10)
    assert not worker.is_alive()
""",
            "admission_model.py": """
# soac: module(strict_assign=true, checked_attr=true)
import admission_support as support

class Model:
    def method(self, value: int) -> int:
        support.pause()
        return value

    support.begin(method)
""",
        },
        modules={"admission_model": "admission_model.py"},
        backend=backend,
    )
    program = """
import ctypes
import admission_support as support
try:
    import admission_model as model
finally:
    support.finish()

assert support.outcomes == ['already active'], support.outcomes
function_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
function_identity.argtypes = [ctypes.py_object]
function_identity.restype = ctypes.c_uint64
assert function_identity(model.Model.method) != 0
assert model.Model().method('later call') == 'later call'
assert model.Model().method(3) == 3
"""
    if backend == "cpython":
        import hashlib
        from tests._strict_integration import ROOT

        source_path = project.project / "admission_model.py"
        before = f"""
import ctypes
import admission_support as support
sys.path.insert(0, {str(ROOT)!r})
from tests._strict_integration import (
    _assert_cpython_function_witness, _assert_cpython_module_witness,
)
original_begin = support.begin
entry_observations = []

def begin_with_native_witness(function):
    diagnostic = _soac_ext.strict_module_diagnostics(sys.modules["admission_model"])
    assert diagnostic is not None and diagnostic["backend"] == "cpython"
    assert not diagnostic["sealed"] and diagnostic["original_code_entered"]
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert not observed["finalized"]
    entry_observations.append(("before", observed["original_code_entered"]))
    original_begin(function)
    # The real worker is inside the source body, while the class suite has not
    # yet returned to its actual native construction/admission boundary.
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert not observed["finalized"]
    entry_observations.append(("inside", observed["original_code_entered"]))

support.begin = begin_with_native_witness
"""
        after = f"""
assert entry_observations == [("before", False), ("inside", True)]
diagnostic = _assert_cpython_module_witness(
    model, module_name="admission_model", source_path={str(source_path)!r},
    source_sha256={hashlib.sha256(source_path.read_bytes()).hexdigest()!r},
    artifact_generation={project.publication["generation"]!r},
)
observed = _assert_cpython_function_witness(
    model.Model.method, diagnostic,
)
assert observed["finalized"] and observed["original_code_entered"]
assert function_identity(model.Model.method) != 0
type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
type_owner.argtypes = [ctypes.py_object]
type_owner.restype = ctypes.c_void_p
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert type_owner(model.Model) and sealed(model.Model) == 1
"""
        program = before + program + after
    project.run(program, entry_interpreter=entry_interpreter, backend=backend)


_FAILED_NAMESPACE_SUPPORT = """
import weakref

events = []
references = []

class Payload:
    def __init__(self):
        events.append('created')
        references.append(weakref.ref(self))

    def __del__(self):
        events.append('released')
"""

_FAILED_NAMESPACE_SOURCES = {
    "failed_class_namespace": """
import namespace_failure_support as support

def fail_class():
    class Broken:
        value = support.Payload()
        raise ValueError('namespace failure')
""",
    "failed_module_namespace": """
import namespace_failure_support as support

value = support.Payload()
raise ValueError('namespace failure')
""",
}

_FAILED_NAMESPACE_CHECK = """
def check_failed_namespace(action, failed_import=None, *, native_frames=True):
    import gc
    import sys
    import namespace_failure_support as support

    try:
        action()
    except ValueError as error:
        assert type(error) is ValueError and error.args == ('namespace failure',)
        retained_traceback = error.__traceback__
    else:
        raise AssertionError('source body did not raise its original ValueError')

    # The ordinary CPython control retains its source namespace through the
    # traceback. SOAC does not reconstruct or retain a source frame for this.
    if native_frames:
        assert retained_traceback is not None
    if failed_import is not None:
        assert failed_import not in sys.modules, 'failed import remained published'
    assert len(support.references) == 1, support.events
    reference = support.references[0]
    gc.collect()
    if native_frames:
        assert reference() is not None, ('ordinary traceback lost namespace owner', support.events)
        assert support.events == ['created'], support.events

    retained_traceback = None
    gc.collect()
    assert reference() is None, ('namespace survived traceback release', support.events)
    assert support.events == ['created', 'released'], support.events
"""


@pytest.mark.parametrize("module_name", tuple(_FAILED_NAMESPACE_SOURCES))
def test_failed_namespace_traceback_native_control(tmp_path, monkeypatch, module_name):
    (tmp_path / "namespace_failure_support.py").write_text(
        _FAILED_NAMESPACE_SUPPORT.lstrip("\n")
    )
    (tmp_path / f"{module_name}.py").write_text(
        _FAILED_NAMESPACE_SOURCES[module_name].lstrip("\n")
    )
    monkeypatch.syspath_prepend(tmp_path)
    if module_name == "failed_class_namespace":
        invocation = """
import importlib
module = importlib.import_module(MODULE_NAME)
check_failed_namespace(module.fail_class)
"""
    else:
        invocation = """
import importlib
check_failed_namespace(lambda: importlib.import_module(MODULE_NAME), MODULE_NAME)
"""
    try:
        exec(  # noqa: S102 - shared literal validator around ordinary source imports.
            compile(
                _FAILED_NAMESPACE_CHECK + invocation,
                str(Path(__file__)),
                "exec",
                dont_inherit=True,
            ),
            {"MODULE_NAME": module_name, "__name__": "ordinary_namespace_control"},
        )
    finally:
        sys.modules.pop(module_name, None)
        sys.modules.pop("namespace_failure_support", None)


@pytest.fixture(scope="module")
def failed_namespace_project(tmp_path_factory):
    sources = {"namespace_failure_support.py": _FAILED_NAMESPACE_SUPPORT}
    modules = {}
    for name, body in _FAILED_NAMESPACE_SOURCES.items():
        path = f"{name}.py"
        sources[path] = strict_opt_in(body.encode(), path)[0].decode()
        modules[name] = path
    return create_strict_project(
        tmp_path_factory.mktemp("strict-failed-namespace"), sources, modules=modules
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_failed_class_namespace_preserves_errors_and_releases_values(
    failed_namespace_project, entry_interpreter
):
    failed_namespace_project.run_case(
        "failed_class_namespace",
        _FAILED_NAMESPACE_CHECK
        + "\ndef validate(module):\n    check_failed_namespace(module.fail_class, native_frames=False)\n",
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("fail_class",),
    )


def test_failed_module_namespace_preserves_errors_and_releases_values(failed_namespace_project):
    # Module initializers always use their explicit interpreted lowering plan.
    failed_namespace_project.run(
        _FAILED_NAMESPACE_CHECK
        + """
import importlib
check_failed_namespace(
    lambda: importlib.import_module('failed_module_namespace'),
    'failed_module_namespace',
    native_frames=False,
)
"""
    )


# These are class-body inline comprehensions, not separate native comprehension
# frames. The ordinary controls and strict cases share the exact declarations
# and validator; only the explicit strict opt-in differs.
_CLASS_FRAME_COUPLING_CASES = {
    "method_forwards_captured_cell_to_nested_functions": (
        """
def build(saved):
    class Box:
        def read(self):
            def nested():
                return saved
            return (lambda: saved), nested
    return Box
""",
        """
def validate(module):
    marker = object()
    cls = module.build(marker)
    check_class_owner(cls)
    method = vars(cls)['read']
    check_function_owner(method)
    source_cell = closure_cell(method, 'saved')
    assert source_cell.cell_contents is marker
    functions = cls().read()
    assert len(functions) == 2
    for function in functions:
        check_function_owner(function)
        assert closure_cell(function, 'saved') is source_cell
        assert function() is marker
    replacement = object()
    source_cell.cell_contents = replacement
    for function in (*functions, *cls().read()):
        check_function_owner(function)
        assert closure_cell(function, 'saved') is source_cell
        assert function() is replacement
""",
    ),
    "plain_target": (
        """
def build(marker):
    class Box:
        values = [item for item in (marker,)]
    return Box
""",
        """
def validate(module):
    marker = object()
    cls = module.build(marker)
    check_class_owner(cls)
    assert cls.values == [marker]
    assert 'item' not in vars(cls)
""",
    ),
    "captured_target": (
        """
def build(marker):
    class Box:
        values = [lambda: item for item in (marker,)]
    return Box
""",
        """
def validate(module):
    marker = object()
    cls = module.build(marker)
    check_class_owner(cls)
    function = cls.values[0]
    check_function_owner(function)
    assert function() is marker
    assert closure_cell(function, 'item').cell_contents is marker
    assert 'item' not in vars(cls)
""",
    ),
    "class_cell": (
        """
def build(marker):
    class Box:
        values = [lambda: __class__ for __class__ in (marker,)]
        def read(self):
            return __class__
    return Box
""",
        """
def validate(module):
    marker = object()
    cls = module.build(marker)
    check_class_owner(cls)
    transient = cls.values[0]
    method = vars(cls)['read']
    check_function_owner(transient)
    check_function_owner(method)
    assert transient() is marker
    assert cls().read() is cls
    transient_cell = closure_cell(transient, '__class__')
    source_cell = closure_cell(method, '__class__')
    assert transient_cell is not source_cell
    assert transient_cell.cell_contents is marker
    assert source_cell.cell_contents is cls
""",
    ),
    "class_dictionary_cell": (
        """
def build(marker):
    class Box:
        values = [lambda: __classdict__ for __classdict__ in (marker,)]
        field: int
    return Box
""",
        """
def validate(module):
    marker = object()
    cls = module.build(marker)
    check_class_owner(cls)
    transient = cls.values[0]
    provider = vars(cls)['__annotate_func__']
    check_function_owner(transient)
    check_function_owner(provider, interpreted=True)
    assert transient() is marker
    transient_cell = closure_cell(transient, '__classdict__')
    source_cell = closure_cell(provider, '__classdict__')
    assert transient_cell is not source_cell
    assert transient_cell.cell_contents is marker
    try:
        source_cell.cell_contents
    except ValueError:
        pass
    else:
        raise AssertionError('original hidden class dictionary cell is not empty')
    # This is the same cell later read by the public native provider, not a
    # permanently empty traceback-only replacement or the transient target.
    source_cell.cell_contents = {'int': str}
    assert provider(1) == {'field': str}
    del source_cell.cell_contents
    try:
        provider(1)
    except NameError:
        pass
    else:
        raise AssertionError('annotation provider lost its original cell')
""",
    ),
    "conditional_annotation_cell": (
        """
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
        """
def validate(module):
    marker = object()
    cls = module.build(marker, True)
    check_class_owner(cls)
    transient = cls.values[0]
    provider = vars(cls)['__annotate_func__']
    check_function_owner(transient)
    check_function_owner(provider, interpreted=True)
    assert transient() is marker
    transient_cell = closure_cell(transient, '__conditional_annotations__')
    source_cell = closure_cell(provider, '__conditional_annotations__')
    assert transient_cell is not source_cell
    assert transient_cell.cell_contents is marker
    indices = source_cell.cell_contents
    assert type(indices) is set and len(indices) == 1
    assert provider(1) == {'field': int}
    saved = indices.copy()
    indices.clear()
    assert provider(1) == {}
    indices.update(saved)
    assert source_cell.cell_contents is indices
    assert provider(1) == {'field': int}
""",
    ),
    "shadowed_lexical_free": (
        """
def build(marker):
    outside = marker
    class Box:
        def read(self):
            return outside
        values = [lambda: outside for outside in (7, 8)]
    return Box
""",
        """
def validate(module):
    marker = object()
    cls = module.build(marker)
    check_class_owner(cls)
    method = vars(cls)['read']
    check_function_owner(method)
    for function in cls.values:
        check_function_owner(function)
        assert function() == 8
    source_cell = closure_cell(method, 'outside')
    transient_cell = closure_cell(cls.values[0], 'outside')
    assert source_cell is not transient_cell
    # The selected native compiler restores an empty class-owned cell for
    # this same-spelling CELL/FREE collision. The source method captures that
    # cell, rather than the separate outer lexical owner.
    try:
        source_cell.cell_contents
    except ValueError:
        pass
    else:
        raise AssertionError('native class-owned restored cell is not empty')
    assert transient_cell.cell_contents == 8
    try:
        cls().read()
    except NameError:
        pass
    else:
        raise AssertionError('method did not retain the native empty-cell binding')
    assert 'outside' not in vars(cls)
""",
    ),
}

_CLASS_FRAME_COUPLING_VALIDATOR = """
import ctypes

def closure_cell(function, name):
    names = function.__code__.co_freevars
    assert names.count(name) == 1, (function, names, name)
    return function.__closure__[names.index(name)]

def check_class_owner(cls):
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(cls) == int(__dp_integration_soac__)

def check_function_owner(function, *, interpreted=False):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    if __dp_integration_soac__:
        from soac import _soac_ext
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) and metadata(function)
        expected = (
            'entry_interpreter'
            if interpreted or __dp_integration_entry__
            else 'checked_native'
        )
        actual = _soac_ext.strict_function_entry_kind(function)
        assert actual == expected, (function.__qualname__, actual, expected)
    else:
        assert not owner(function)
"""


@pytest.mark.parametrize("case_name", tuple(_CLASS_FRAME_COUPLING_CASES))
def test_class_frame_comprehension_cells_native_control(case_name):
    from types import ModuleType

    from tests._integration import exec_integration_validation

    source, validation = _CLASS_FRAME_COUPLING_CASES[case_name]
    module = ModuleType(f"ordinary_class_frame_{case_name}")
    exec(  # noqa: S102 - unchanged source declarations, ordinary native control.
        compile(source, str(Path(__file__)), "exec", dont_inherit=True),
        module.__dict__,
    )
    exec_integration_validation(
        _CLASS_FRAME_COUPLING_VALIDATOR + validation,
        module,
        Path(__file__),
        mode="stock",
    )


@pytest.fixture(scope="module")
def class_frame_coupling_project(tmp_path_factory, request):
    sources = {}
    modules = {}
    for case_name, (body, _) in _CLASS_FRAME_COUPLING_CASES.items():
        module_name = f"class_frame_{case_name}"
        path = f"{module_name}.py"
        sources[path] = strict_opt_in(body.encode(), path)[0].decode()
        modules[module_name] = path
    return create_strict_project(
        tmp_path_factory.mktemp("strict-class-frame-coupling"),
        sources,
        modules=modules,
        backend=getattr(request, "param", "soac"),
    )


# Source lexical cells remain real function-owned objects even though retained
# SOAC execution does not reconstruct CPython frames.
@pytest.mark.parametrize(
    "case_name", ["method_forwards_captured_cell_to_nested_functions"]
)
@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_class_frame_comprehension_cells_keep_original_owners(
    class_frame_coupling_project, case_name, entry_interpreter
):
    _, validation = _CLASS_FRAME_COUPLING_CASES[case_name]
    class_frame_coupling_project.run_case(
        f"class_frame_{case_name}",
        _CLASS_FRAME_COUPLING_VALIDATOR + validation,
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("build",),
    )


_CLASS_FRAME_REGION_CASES = (
    "plain_target",
    "captured_target",
    "class_cell",
    "class_dictionary_cell",
    "conditional_annotation_cell",
    "shadowed_lexical_free",
)




@pytest.mark.parametrize("case_name", _CLASS_FRAME_REGION_CASES)
@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_class_comprehension_regions_preserve_lexical_cells_and_owners(
    class_frame_coupling_project, case_name, entry_interpreter
):
    _, validation = _CLASS_FRAME_COUPLING_CASES[case_name]
    class_frame_coupling_project.run_case(
        f"class_frame_{case_name}",
        _CLASS_FRAME_COUPLING_VALIDATOR + validation,
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("build",),
    )


def _class_frame_cpython_validator(module_name):
    # Only the backend witnesses change. The original closure/cell observers and
    # every source body remain exactly the same as their ordinary controls.
    return f"""
import ctypes
import importlib
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

def check_class_owner(cls):
    assert __dp_integration_mode__ == 'cpython'
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(cls) and info.owner is not None
    assert sealed(cls) == 1

def check_function_owner(function, *, interpreted=False):
    module = importlib.import_module({module_name!r})
    diagnostic = _assert_cpython_function_witness(
        function, _soac_ext.strict_module_diagnostics(module),
    )
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(function)
    assert _soac_ext.strict_function_entry_kind(function) == 'original_code'
"""


@pytest.mark.parametrize("class_frame_coupling_project", ["cpython"], indirect=True)
@pytest.mark.parametrize("case_name", tuple(_CLASS_FRAME_COUPLING_CASES))
def test_cpython_class_frame_comprehension_cells_keep_original_owners(
    class_frame_coupling_project, case_name
):
    module_name = f"class_frame_{case_name}"
    _, validation = _CLASS_FRAME_COUPLING_CASES[case_name]
    class_frame_coupling_project.run_case(
        module_name,
        _CLASS_FRAME_COUPLING_VALIDATOR
        + _class_frame_cpython_validator(module_name)
        + validation,
        Path(__file__),
        required_functions=("build",),
        
        backend="cpython",
    )


_CLASS_COMPREHENSION_PREFIX_SOURCE = """
def build(sink, prefix, source, later):
    class C:
        result = sink(prefix(), [lambda: item for item in source()], later())
    return C
"""

_CLASS_COMPREHENSION_PREFIX_OBSERVER = """
import gc
import sys
import weakref

def observe_class_prefix(build, outcome):
    events = []
    refs = {}
    marker = ValueError('comprehension-error')

    def handled():
        error = sys.exception()
        return None if error is None else str(error.args[0])

    def live():
        return tuple(bool(refs.get(name) and refs[name]() is not None)
                     for name in ('prefix', 'item', 'iterator'))

    class Value:
        def __init__(self, name):
            self.name = name
            refs[name] = weakref.ref(self)
            events.append(('made', name, handled()))
        def __del__(self):
            events.append(('drop', self.name, handled(), live()))

    class Iterator:
        def __init__(self):
            self.started = False
            refs['iterator'] = weakref.ref(self)
        def __iter__(self):
            events.append(('iter', handled()))
            return self
        def __next__(self):
            if not self.started:
                self.started = True
                return Value('item')
            if outcome == 'next-error':
                raise marker
            raise StopIteration
        def __del__(self):
            events.append(('drop', 'iterator', handled(), live()))

    def source():
        events.append(('source', handled()))
        if outcome == 'source-error':
            raise marker
        return Iterator()

    def prefix():
        return Value('prefix')

    def later():
        assert refs['prefix']() is not None and refs['item']() is not None
        events.append(('later', handled(), live()))
        return None

    def sink(first, callbacks, last):
        assert first is refs['prefix']() and callbacks[0]() is refs['item']()
        events.append(('sink', handled(), live(), callbacks[0]() is refs['item']()))
        return None

    try:
        raise KeyError('caller')
    except KeyError:
        try:
            build(sink, prefix, source, later)
        except ValueError as error:
            assert outcome != 'success' and error is marker
            events.append(('caught', handled(), live()))
            error.__traceback__ = None
            events.append(('traceback-cleared', handled(), live()))
        else:
            assert outcome == 'success'
            events.append(('returned', handled(), live()))
        events.append(('after-call', handled(), live()))
    gc.collect()
    events.append(('after-handler', handled(), live()))
    return events

def class_prefix_semantics(events):
    assert events[-1] == ('after-handler', None, (False, False, False)), events
    drops = sorted(event[1] for event in events if event[0] == 'drop')
    assert len(drops) == len(set(drops)), events
    return [
        event if event[0] == 'made' else event[:2]
        for event in events if event[0] != 'drop'
    ], drops
"""


@pytest.mark.parametrize("outcome", ["success", "source-error", "next-error"])
def test_class_comprehension_prefix_cleanup_native_control(outcome):
    namespace = {}
    exec(
        compile(_CLASS_COMPREHENSION_PREFIX_SOURCE, "<class-prefix-native>", "exec"),
        namespace,
    )
    exec(_CLASS_COMPREHENSION_PREFIX_OBSERVER, namespace)
    events = namespace["observe_class_prefix"](namespace["build"], outcome)
    assert events[-1] == ("after-handler", None, (False, False, False))
    if outcome == "next-error":
        drops = [event for event in events if event[:1] == ("drop",)]
        item = next(event for event in drops if event[1] == "item")
        assert item[3][0], (
            "native cell restoration happens while the older prefix is owned"
        )
        assert [event[1] for event in drops].index("item") < [
            event[1] for event in drops
        ].index("prefix")


@pytest.fixture(scope="module")
def class_comprehension_prefix_project(tmp_path_factory, request):
    body = _CLASS_COMPREHENSION_PREFIX_SOURCE
    return create_strict_project(
        tmp_path_factory.mktemp("strict-class-comprehension-prefix"),
        {
            "prefix_model.py": strict_opt_in(body.encode(), "prefix_model.py")[
                0
            ].decode(),
            "ordinary_prefix_model.py": body,
        },
        modules={"prefix_model": "prefix_model.py"},
        backend=getattr(request, "param", "soac"),
    )


@pytest.mark.parametrize(
    "class_comprehension_prefix_project", ["cpython"], indirect=True
)
def test_class_comprehension_prefix_cleanup_matches_native(
    class_comprehension_prefix_project,
):
    class_comprehension_prefix_project.run_case(
        "prefix_model",
        "import prefix_model as actual\n"
        "import ordinary_prefix_model as ordinary\n"
        "from soac import _soac_ext\n"
        "assert _soac_ext.strict_module_diagnostics(actual)['sealed']\n"
        "assert _soac_ext.strict_module_diagnostics(ordinary) is None\n"
        "assert _soac_ext.strict_function_entry_kind(actual.build) == 'original_code'\n"
        + _CLASS_COMPREHENSION_PREFIX_OBSERVER
        + "\nmismatches = []\n"
        "for outcome in ('success', 'source-error', 'next-error'):\n"
        "    expected = observe_class_prefix(ordinary.build, outcome)\n"
        "    observed = observe_class_prefix(actual.build, outcome)\n"
        "    if observed != expected:\n"
        "        mismatches.append((outcome, observed, expected))\n"
        "assert not mismatches, mismatches\n"
        "assert _soac_ext.strict_function_diagnostics(actual.build)['original_code_entered']\n",
        Path(__file__),
        required_functions=("build",),
        
        backend="cpython",
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_class_comprehension_prefix_preserves_evaluation_errors_and_cleanup(
    class_comprehension_prefix_project, entry_interpreter
):
    class_comprehension_prefix_project.run_case(
        "prefix_model",
        "import prefix_model as actual\n"
        "import ordinary_prefix_model as ordinary\n"
        "from soac import _soac_ext\n"
        "assert _soac_ext.strict_module_diagnostics(ordinary) is None\n"
        + _CLASS_COMPREHENSION_PREFIX_OBSERVER
        + "\nfor outcome in ('success', 'source-error', 'next-error'):\n"
        "    expected = class_prefix_semantics(observe_class_prefix(ordinary.build, outcome))\n"
        "    observed = class_prefix_semantics(observe_class_prefix(actual.build, outcome))\n"
        "    assert observed == expected, (outcome, observed, expected)\n",
        Path(__file__),
        required_functions=("build",),
        entry_interpreter=entry_interpreter,
    )


_PENDING_TYPE_BODY = """
from pending_type_support import observe, events

class Base:
    __slots__ = ()

    def __init_subclass__(cls) -> None:
        observe(cls)

class Child(Base):
    {slots}
    value: int

    def __init__(self) -> None:
        events.append("init")
        self.value = 1

    def checked(self, value: int) -> int:
        return value

# Instance admission must finish at the real class definition, before the
# enclosing module seals, rather than waiting for the end of import.
created = Child()
"""

_PENDING_TYPE_SUPPORT = """
import ctypes

expect_pending = True
observing = False
events = []
observed = []

def observe(cls):
    global observing
    if observing:
        return
    observing = True
    try:
        from soac.strict import StrictMutationError
        from soac import _soac_ext

        alloc = ctypes.pythonapi.PyType_GenericAlloc
        alloc.argtypes = [ctypes.py_object, ctypes.c_ssize_t]
        alloc.restype = ctypes.py_object
        call = ctypes.pythonapi.PyObject_CallNoArgs
        call.argtypes = [ctypes.py_object]
        call.restype = ctypes.py_object
        assign = ctypes.pythonapi.PyObject_SetAttr
        assign.argtypes = [ctypes.py_object] * 3
        assign.restype = ctypes.c_int
        own_contract = ctypes.pythonapi.PyType_HasSoacContract
        own_contract.argtypes = [ctypes.py_object]
        own_contract.restype = ctypes.c_int

        own_slots = vars(cls).get('__slots__')
        donor_namespace = {} if own_slots is None else {'__slots__': own_slots}
        Donor = type('Donor', cls.__bases__, donor_namespace)
        # Prove this really is a layout-compatible type transition. The
        # ordinary control executes every assignment successfully below.
        layout = ('__basicsize__', '__itemsize__', '__dictoffset__', '__weakrefoffset__')
        assert tuple(getattr(cls, key) for key in layout) == tuple(getattr(Donor, key) for key in layout)
        assert bool(cls.__flags__ & 4) == bool(Donor.__flags__ & 4)
        victim = Donor()
        victim.value = 41
        dictionary = vars(victim) if own_slots is None else None
        identity = id(victim)

        operations = (
            ('call', lambda: cls()),
            ('object-new', lambda: object.__new__(cls)),
            ('native-alloc', lambda: alloc(cls, 0)),
            ('native-call', lambda: call(cls)),
            ('subtype', lambda: type('EscapedSubtype', (cls,), {})),
        )
        for label, operation in operations:
            before = list(events)
            if expect_pending:
                try:
                    operation()
                except StrictMutationError:
                    pass
                else:
                    raise AssertionError('pending type admitted ' + label)
                assert events == before, 'rejection followed a constructor callback'
            else:
                operation()

        for setter in (setattr, object.__setattr__, assign):
            if expect_pending:
                try:
                    setter(victim, '__class__', cls)
                except StrictMutationError:
                    pass
                else:
                    raise AssertionError('pending type admitted a compatible __class__ assignment')
            else:
                setter(victim, '__class__', cls)
                assert type(victim) is cls
                setter(victim, '__class__', Donor)
            assert type(victim) is Donor and id(victim) == identity and victim.value == 41
            if dictionary is not None:
                assert vars(victim) is dictionary and dictionary == {'value': 41}

        if expect_pending:
            assert own_contract(cls) == 0, 'the final type contract was published provisionally'
            method = cls.checked
            witness = _soac_ext.strict_function_diagnostics(method)
            if witness is not None:
                assert not witness['finalized']
                assert not witness['original_code_entered']
            else:
                strict_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
                strict_id.argtypes = [ctypes.py_object]
                strict_id.restype = ctypes.c_uint64
                assert strict_id(method) == 0
                assert _soac_ext.strict_function_entry_kind(method) in (
                    'checked_native', 'entry_interpreter',
                )
            before = list(events)
            assert method(None, 'ordinary argument') == 'ordinary argument'
            if witness is not None:
                assert _soac_ext.strict_function_diagnostics(method)['original_code_entered']
            assert method(None, 3) == 3
            if witness is None:
                assert events == before + ['checked body', 'checked body']
            else:
                assert events == before
        observed.append(cls)
    finally:
        observing = False
"""


@pytest.mark.parametrize("slotted", [False, True], ids=["dictionary", "slots"])
def test_pending_type_observation_ordinary_control(tmp_path, monkeypatch, slotted):
    from tests._integration import stock_module

    body = _PENDING_TYPE_BODY.format(slots="__slots__ = ('value',)" if slotted else "")
    with stock_module(tmp_path, "pending_type_support", _PENDING_TYPE_SUPPORT) as support:
        # stock_module isolates its import name. Publish only this fixture's
        # explicit ordinary dependency for the unchanged source import below.
        monkeypatch.setitem(sys.modules, "pending_type_support", support)
        support.expect_pending = False
        with stock_module(tmp_path, "ordinary_pending_type", body) as module:
            assert support.observed == [module.Child]
            assert type(module.created) is module.Child and module.created.value == 1
            assert module.created.checked("ordinary") == "ordinary"


@pytest.mark.parametrize("slotted", [False, True], ids=["dictionary", "slots"])
def test_cpython_pending_type_blocks_callback_admission_then_enforces_final_type(tmp_path, slotted):
    body = _PENDING_TYPE_BODY.format(slots="__slots__ = ('value',)" if slotted else "")
    project = create_strict_project(
        tmp_path,
        {
            "pending_type.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + body,
            "pending_type_support.py": _PENDING_TYPE_SUPPORT,
        },
        modules={"pending_type": "pending_type.py"},
        backend="cpython",
    )
    project.run_case(
        "pending_type",
        """
import ctypes
import pending_type as module
import pending_type_support as support
from soac import _soac_ext

assert support.observed == [module.Child]
assert support.events == ['init']
assert type(module.created) is module.Child and module.created.value == 1
assert module.created.checked(4) == 4
for write in (setattr, object.__setattr__):
    try:
        write(module.created, 'value', 'bad')
    except TypeError:
        pass
    else:
        raise AssertionError('final admission opened before selected field checks')
    assert module.created.value == 1
own_contract = ctypes.pythonapi.PyType_HasSoacContract
own_contract.argtypes = [ctypes.py_object]
own_contract.restype = ctypes.c_int
assert own_contract(module.Child) == 1
assert _soac_ext.strict_function_diagnostics(module.Child.checked)['finalized']
""",
        Path(__file__),
        required_functions=("Base.__init_subclass__", "Child.__init__", "Child.checked"),
        
        backend="cpython",
    )


_EARLY_CLASS_ADMISSION_BODY = """
from early_class_probe import observe, after_later

class First:
    pass

class Consumer:
    def early(self, value: First) -> First:
        return value

    def forward(self, value: "Later") -> "Later":
        return value

first = First()
consumer = Consumer()
observe(consumer, first)

class Later:
    pass

later = Later()
after_later(consumer, first, later)
"""

_EARLY_CLASS_ADMISSION_PROBE = """
import ctypes
from soac import _soac_ext
from soac.strict import StrictMutationError

events = []
one = ctypes.pythonapi.PyObject_CallOneArg
one.argtypes = [ctypes.py_object, ctypes.py_object]
one.restype = ctypes.py_object
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int

def observe(receiver, first):
    cls = type(receiver)
    diagnostic = _soac_ext.strict_function_diagnostics(cls.early)
    if diagnostic is None:
        assert receiver.early(first) is first
        assert receiver.forward(first) is first
        events.append('ordinary before Later')
        return
    assert diagnostic['backend'] == 'cpython'
    assert diagnostic['finalized'], 'instances opened before method metadata sealing'
    assert sealed(cls) == 1, 'instances opened before class sealing'
    assert receiver.early(first) is first
    assert one(receiver.early, first) is first
    foreign = object()
    assert receiver.early(foreign) is foreign
    assert receiver.forward(first) is first
    assert _soac_ext.strict_function_diagnostics(cls.forward)['original_code_entered']
    try:
        cls.early.__defaults__ = (first,)
    except StrictMutationError:
        pass
    else:
        raise AssertionError('pending module globals reopened frozen method metadata')
    events.append('strict before Later')

def after_later(receiver, first, later):
    assert receiver.early(first) is first
    assert receiver.forward(later) is later
    if _soac_ext.strict_function_diagnostics(type(receiver).forward) is None:
        assert receiver.forward(first) is first
        events.append('ordinary after Later')
        return
    assert one(receiver.forward, later) is later
    assert one(receiver.forward, first) is first
    events.append('strict after Later')
"""


def test_cpython_class_admission_seals_metadata_without_checking_forward_annotations(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "early_class.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _EARLY_CLASS_ADMISSION_BODY,
            "ordinary_early_class.py": _EARLY_CLASS_ADMISSION_BODY,
            "early_class_probe.py": _EARLY_CLASS_ADMISSION_PROBE,
        },
        modules={"early_class": "early_class.py"},
        backend="cpython",
    )
    project.run_case(
        "early_class",
        """
import early_class as module
import ordinary_early_class as ordinary
from early_class_probe import events, one
from soac import _soac_ext

assert events == [
    'strict before Later', 'strict after Later',
    'ordinary before Later', 'ordinary after Later',
]
assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary) is None
for number in range(128):
    assert module.consumer.early(module.first) is module.first
    assert module.consumer.forward(module.later) is module.later
assert one(module.consumer.forward, module.first) is module.first
assert module.consumer.early(module.later) is module.later
assert module.Consumer.early.__defaults__ is None
assert module.Consumer.forward.__defaults__ is None
""",
        Path(__file__),
        required_functions=("Consumer.early", "Consumer.forward"),
        
        backend="cpython",
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("slotted", [False, True], ids=["dictionary", "slots"])
def test_soac_pending_type_preserves_layout_and_admits_only_after_required_constraints(
    tmp_path, entry_interpreter, slotted
):
    # Reuse every ordinary/native allocation and compatible class-assignment
    # control. Body events prove ordinary calls run during callbacks while
    # instance admission remains blocked until field policy installation.
    body = _PENDING_TYPE_BODY.format(slots="__slots__ = ('value',)" if slotted else "")
    body = body.replace(
        "    def checked(self, value: int) -> int:\n        return value",
        "    def checked(self, value: int) -> int:\n"
        "        events.append('checked body')\n        return value",
    )
    project = create_strict_project(
        tmp_path,
        {
            "retained_pending_type.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + body,
            "pending_type_support.py": _PENDING_TYPE_SUPPORT,
        },
        modules={"retained_pending_type": "retained_pending_type.py"},
        backend="soac",
    )
    project.run_case(
        "retained_pending_type",
        """
import ctypes
import retained_pending_type as module
import pending_type_support as support

assert support.observed == [module.Child]
assert support.events == ['checked body', 'checked body', 'init']
assert type(module.created) is module.Child and module.created.value == 1
assert module.created.checked(4) == 4
for write in (setattr, object.__setattr__):
    try:
        write(module.created, 'value', 'bad')
    except TypeError:
        pass
    else:
        raise AssertionError('instances opened before the selected field checks')
    assert module.created.value == 1
own_contract = ctypes.pythonapi.PyType_HasSoacContract
own_contract.argtypes = [ctypes.py_object]
own_contract.restype = ctypes.c_int
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert own_contract(module.Child) == 1 and sealed(module.Child) == 1
""",
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("Base.__init_subclass__", "Child.__init__", "Child.checked"),
        
        backend="soac",
        opt_mode="none",
    )


_FROZEN_MODULE_NOMINAL_BODY = """
from frozen_module_nominal_probe import exercise, body

class First:
    pass

class Second:
    pass

# This is FinalAfterSeal, not an explicitly mutable global. The ordinary probe
# changes its actual globals entry only while this module is initializing.
Alias = First

class Consumer:
    def accept(self, value: Alias) -> Alias:
        body(globals(), value)
        return value

first = First()
second = Second()
consumer = Consumer()
exercise(globals(), consumer, first, second, {span_seal!r})
"""

_FROZEN_MODULE_NOMINAL_PROBE = """
import ctypes
import sys
import threading
from typing import Any

from soac import _soac_ext
from soac.strict import StrictMutationError

states = {}
call_object = ctypes.pythonapi.PyObject_Call
call_object.argtypes = [ctypes.py_object] * 3
call_object.restype = ctypes.py_object
call_one = ctypes.pythonapi.PyObject_CallOneArg
call_one.argtypes = [ctypes.py_object] * 2
call_one.restype = ctypes.py_object
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
get_type_dict = ctypes.pythonapi.PyType_GetDict
get_type_dict.argtypes = [ctypes.py_object]
get_type_dict.restype = ctypes.py_object
set_dict = ctypes.pythonapi.PyObject_GenericSetDict
set_dict.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.c_void_p]
set_dict.restype = ctypes.c_int

def reject(error_type, call):
    try:
        call()
    except error_type:
        return
    raise AssertionError('missing mandatory boundary: ' + error_type.__name__)

def no_compilation():
    assert _soac_ext.runtime_compilation_activity() == {
        'schema': 1, 'lowering_entries': 0, 'blockpy_cache_entries': 0,
        'jit_engine_entries': 0,
    }

def frozen(function, module, *, entered):
    witness = _soac_ext.strict_function_diagnostics(function)
    assert witness is not None
    assert witness['schema'] == 2 and witness['backend'] == 'cpython'
    assert witness['entry_kind'] == 'original_code'
    assert witness['finalized'] is True
    assert witness['original_code_entered'] is entered
    module_witness = _soac_ext.strict_module_diagnostics(module)
    for key in ('source_path', 'source_sha256', 'artifact_generation'):
        assert witness[key] == module_witness[key], (key, witness)
    assert sealed(module.Consumer) == 1
    reject(StrictMutationError, lambda: setattr(function, '__defaults__', (module.first,)))
    reject(StrictMutationError, lambda: setattr(function, '__code__', function.__code__))
    assert function.__defaults__ is None

def body(namespace: Any, value: Any) -> None:
    state = states[namespace['__name__']]
    assert namespace is state['namespace']
    state['events'].append('body')
    state['values'].append(value)
    if state['shift_body']:
        namespace['Alias'] = namespace['First']
    if threading.current_thread() is state['worker']:
        state['entered'].set()
        if not state['release'].wait(10):
            raise AssertionError('module sealing did not release its active call')

def exercise(namespace: Any, receiver: Any, first: Any, second: Any, span_seal: bool) -> None:
    module = sys.modules[namespace['__name__']]
    function = type(receiver).accept
    assert vars(module) is namespace is function.__globals__
    module_witness = _soac_ext.strict_module_diagnostics(module)
    strict = module_witness is not None
    state = states[namespace['__name__']] = {
        'namespace': namespace, 'events': [], 'values': [], 'shift_body': True,
        'worker': None, 'outcomes': [], 'entered': threading.Event(),
        'release': threading.Event(),
    }
    if strict:
        assert module_witness['backend'] == 'cpython' and not module_witness['sealed']
        assert module_witness['initializer_entry_kind'] == 'original_code'
        assert module_witness['original_code_entered'] is True
        frozen(function, module, entered=False)
        assert call_one(receiver.accept, second) is second
        assert state['values'] == [second]
        assert _soac_ext.strict_function_diagnostics(function)['original_code_entered']
    else:
        assert _soac_ext.strict_function_diagnostics(function) is None
        # The same ordinary source has no nominal restriction or metadata seal.
        assert call_one(receiver.accept, second) is second
        function.__defaults__ = (first,)
        assert function.__defaults__ == (first,)
        function.__defaults__ = None

    class Keyword(str):
        __hash__ = str.__hash__
        def __eq__(self, other):
            equal = str.__eq__(self, other)
            if equal:
                namespace['Alias'] = namespace['Second']
                state['events'].append('keyword')
            return equal

    def python_call(function, arguments, keywords):
        return function(*arguments, **keywords)

    for invoke in (python_call, call_object):
        assert namespace['Alias'] is namespace['First']
        before = len(state['events'])
        assert invoke(receiver.accept, (), {Keyword('value'): second}) is second
        # The keyword callback changes the annotation's global and the body
        # restores it. Neither mutation changes ordinary argument/result values.
        assert state['events'][before:] == ['keyword', 'body']
        assert namespace['Alias'] is namespace['First']
        assert call_one(receiver.accept, first) is first
        before = len(state['values'])
        assert invoke(receiver.accept, (second,), {}) is second
        assert len(state['values']) == before + 1

    if span_seal:
        def active_call():
            try:
                state['outcomes'].append(
                    call_object(receiver.accept, (), {Keyword('value'): second})
                )
            except BaseException as error:
                state['outcomes'].append(error)
                state['entered'].set()

        state['worker'] = threading.Thread(target=active_call)
        state['worker'].start()
        if not state['entered'].wait(10):
            state['release'].set()
            state['worker'].join(10)
            raise AssertionError('the C-entered call did not reach its source body')
        if state['outcomes'] and isinstance(state['outcomes'][0], BaseException):
            raise state['outcomes'][0]
        assert state['outcomes'] == [], state['outcomes']
        assert state['worker'].is_alive()
        assert state['values'][-1] is second
        assert namespace['Alias'] is namespace['First']
        if strict:
            assert not _soac_ext.strict_module_diagnostics(module)['sealed']
    # No later body writes a now-sealed global. The already active call has
    # completed its write before entered is signaled above.
    state['shift_body'] = False
    if strict:
        frozen(function, module, entered=True)
    no_compilation()

def validate(module: Any, *, strict: bool, span_seal: bool) -> None:
    namespace = vars(module)
    state = states[module.__name__]
    try:
        assert namespace is state['namespace']
        assert module.Alias is module.First
        witness = _soac_ext.strict_module_diagnostics(module)
        if strict:
            assert witness is not None and witness['sealed'] is True
            frozen(module.Consumer.accept, module, entered=True)
        else:
            assert witness is None
            assert _soac_ext.strict_function_diagnostics(module.Consumer.accept) is None
        if span_seal:
            assert state['worker'].is_alive() and state['outcomes'] == []
            assert state['values'][-1] is module.second
    finally:
        if state['worker'] is not None:
            state['release'].set()
            state['worker'].join(10)
            assert not state['worker'].is_alive()

    if span_seal:
        # Sealing the module must not change an already-running call's result.
        assert len(state['outcomes']) == 1, state['outcomes']
        if isinstance(state['outcomes'][0], BaseException):
            raise state['outcomes'][0]
        assert state['outcomes'][0] is module.second
    for _ in range(128):
        assert module.consumer.accept(module.first) is module.first
    assert call_one(module.consumer.accept, module.first) is module.first
    before = len(state['values'])
    assert module.consumer.accept(module.second) is module.second
    assert call_one(module.consumer.accept, module.second) is module.second
    assert len(state['values']) == before + 2
    if strict:
        reject(StrictMutationError, lambda: namespace.__setitem__('Alias', module.Second))
        assert module.Alias is module.First

        # These are the real authoritative dictionaries, not mappingproxy IDs.
        # PyType_GetDict returns a new reference to the actual class dictionary.
        type_dictionary = get_type_dict(module.Consumer)
        module_contents, type_contents = dict(namespace), dict(type_dictionary)
        reject(StrictMutationError, lambda: set_dict(module, {}, None))
        assert vars(module) is namespace and namespace == module_contents
        reject(StrictMutationError, lambda: set_dict(module.Consumer, {}, None))
        assert get_type_dict(module.Consumer) is type_dictionary
        assert type_dictionary == type_contents
        frozen(module.Consumer.accept, module, entered=True)
        assert call_one(module.consumer.accept, module.first) is module.first
    else:
        namespace['Alias'] = module.Second
        assert module.Alias is module.Second
    no_compilation()
"""


@pytest.fixture(scope="module")
def frozen_module_nominal_projects(tmp_path_factory):
    sources = {"frozen_module_nominal_probe.py": _FROZEN_MODULE_NOMINAL_PROBE}
    modules = {}
    for suffix, span_seal in (("sync", False), ("spans_seal", True)):
        name = f"frozen_module_nominal_{suffix}"
        body = _FROZEN_MODULE_NOMINAL_BODY.format(span_seal=span_seal)
        sources[f"{name}.py"] = "# soac: module(strict_assign=true, checked_attr=true)\n" + body
        sources[f"ordinary_{name}.py"] = body
        modules[name] = f"{name}.py"
    return create_strict_project(
        tmp_path_factory.mktemp("cpython-frozen-module-nominal"),
        sources,
        modules=modules,
        backend="cpython",
    )


@pytest.mark.parametrize(
    ("suffix", "span_seal"),
    [("sync", False), ("spans_seal", True)],
    ids=["binder-body", "active-call-across-module-seal"],
)
def test_cpython_frozen_method_preserves_calls_and_module_seals_across_callbacks(
    frozen_module_nominal_projects, suffix, span_seal
):
    name = f"frozen_module_nominal_{suffix}"
    frozen_module_nominal_projects.run_case(
        name,
        f"""
def validate(module):
    import importlib
    from frozen_module_nominal_probe import validate as check

    check(module, strict=True, span_seal={span_seal!r})
    ordinary = importlib.import_module('ordinary_' + module.__name__)
    check(ordinary, strict=False, span_seal={span_seal!r})
""",
        Path(__file__),
        required_functions=("Consumer.accept",),
        
        backend="cpython",
    )


# Distinct native behaviors not covered by the original eight cell/prefix
# observers. Their original plan-only source variants also remain in the native
# compile-data inventory; these complete callable subjects support real startup.
_CLASS_LIFECYCLE_NATIVE_CASES = {
    "set_cleanup": (
        """
def build(source):
    class C:
        result = {item for item in source()}
    return C
""",
        "build",
        "observe_collection(build, 'set')",
    ),
    "dict_cleanup": (
        """
def build(source):
    class C:
        result = {item: item for item in source()}
    return C
""",
        "build",
        "observe_collection(build, 'dict')",
    ),
    "conditional_equal_name": (
        """
def factory(value, enabled):
    class C:
        result = [([lambda: value for value in (7,)] if enabled else None, value) for unused in (0,)]
    return C
""",
        "factory",
        "observe_conditional(build)",
    ),
    "namespace_delete_capture": (
        """
def factory(value):
    class C:
        value = 'namespace'
        seen = value
        callbacks = [lambda: value for unused in (0,)]
        del value
    return C
""",
        "factory",
        "observe_namespace(build)",
    ),
    "finally_completion": (
        """
def build(checkpoint):
    class C:
        try:
            checkpoint()
        finally:
            callbacks = [lambda: item for item in (1,)]
    return C
""",
        "build",
        "observe_finally(build)",
    ),
    "pre_region_raise": (
        """
def build(source):
    class C:
        raise ValueError('before region')
        ignored = [lambda: item for item in source()]
    return C
""",
        "build",
        "observe_pre_region_raise(build)",
    ),
}


_CLASS_LIFECYCLE_NATIVE_OBSERVERS = """
import ctypes
import gc
import sys
import weakref

def lifecycle_class(cls, native):
    if native:
        check_class_owner(cls)
    else:
        owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(cls) is None

def lifecycle_function(function, native):
    if native:
        check_function_owner(function)
    else:
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(function) is None

def observe_collection(build, kind, native=False):
    outcomes = []
    for outcome in ('success', 'source-error', 'next-error', 'hash-error'):
        events = []
        refs = {}
        marker = LookupError(outcome)

        def handled():
            error = sys.exception()
            return None if error is None else str(error.args[0])

        def live():
            return tuple(bool(refs.get(name) and refs[name]() is not None)
                         for name in ('item', 'iterator'))

        class Item:
            def __hash__(self):
                events.append(('hash', handled()))
                if outcome == 'hash-error':
                    raise marker
                return 7
            def __del__(self):
                events.append(('drop-item', handled(), live()))

        class Iterator:
            def __init__(self):
                self.started = False
                refs['iterator'] = weakref.ref(self)
            def __iter__(self):
                events.append(('iter', handled()))
                return self
            def __next__(self):
                if not self.started:
                    self.started = True
                    item = Item()
                    refs['item'] = weakref.ref(item)
                    events.append(('made-item', handled()))
                    return item
                if outcome == 'next-error':
                    raise marker
                raise StopIteration
            def __del__(self):
                events.append(('drop-iterator', handled(), live()))

        def source():
            events.append(('source', handled()))
            if outcome == 'source-error':
                raise marker
            return Iterator()

        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(source)
            except LookupError as error:
                assert outcome != 'success' and error is marker
                assert error.__context__ is caller
                events.append(('caught', handled(), live()))
                error.__traceback__ = None
                events.append(('traceback-cleared', handled(), live()))
            else:
                assert outcome == 'success'
                lifecycle_class(cls, native)
                result = vars(cls)['result']
                assert type(result) is (set if kind == 'set' else dict)
                assert len(result) == 1
                item = refs['item']()
                assert item is not None and item in result
                if kind == 'dict':
                    assert result[item] is item
                del item
                result.clear()
                events.append(('cleared', handled(), live()))
                del result, cls
            assert sys.exception() is caller
            events.append(('after-call', handled(), live()))
        gc.collect()
        assert live() == (False, False)
        events.append(('after-handler', handled(), live()))
        outcomes.append((outcome, events))
    return outcomes

def observe_conditional(build, native=False):
    marker = object()
    observed = []
    for enabled in (False, True):
        cls = build(marker, enabled)
        lifecycle_class(cls, native)
        assert len(cls.result) == 1
        callbacks, value = cls.result[0]
        # The pinned CPython emitter saves/restores the hidden slot but writes
        # the distinct FREE slot in the enabled inner comprehension. Preserve
        # that ordinary 3.15.0a5 behavior, not an assumed SOAC lifetime recipe.
        if enabled:
            assert type(value) is int and value == 7
        else:
            assert value is marker
        assert 'value' not in vars(cls) and 'unused' not in vars(cls)
        if enabled:
            assert len(callbacks) == 1
            callback = callbacks[0]
            lifecycle_function(callback, native)
            assert callback() == 7
            assert closure_cell(callback, 'value').cell_contents == 7
        else:
            assert callbacks is None
        observed.append((
            enabled,
            value is marker,
            None if value is marker else (type(value).__name__, value),
            None if callbacks is None else callbacks[0](),
        ))
    return observed

def observe_namespace(build, native=False):
    marker = object()
    cls = build(marker)
    lifecycle_class(cls, native)
    assert cls.seen == 'namespace' and 'value' not in vars(cls)
    assert 'unused' not in vars(cls)
    callback, = cls.callbacks
    lifecycle_function(callback, native)
    assert callback() is marker
    cell = closure_cell(callback, 'value')
    assert cell.cell_contents is marker
    replacement = object()
    cell.cell_contents = replacement
    assert callback() is replacement
    assert cls.seen == 'namespace'
    return ('namespace', callback() is replacement, 'value' in vars(cls))

def class_error_callback(error):
    traceback = error.__traceback__
    while traceback is not None:
        namespace = traceback.tb_frame.f_locals
        if 'callbacks' in namespace:
            callback, = namespace['callbacks']
            return callback
        traceback = traceback.tb_next
    raise AssertionError('original finally suite did not publish its callback')

def observe_finally(build, native=False):
    observed = []
    for fails in (False, True):
        events = []
        marker = ValueError('checkpoint')
        def checkpoint():
            events.append('checkpoint')
            if fails:
                raise marker
        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(checkpoint)
            except ValueError as error:
                assert fails and error is marker and error.__context__ is caller
                callback = class_error_callback(error)
                lifecycle_function(callback, native)
                assert callback() == 1
                reference = weakref.ref(callback)
                del callback
                error.__traceback__ = None
                gc.collect()
                assert reference() is None
                events.append('finally-error')
            else:
                assert not fails
                lifecycle_class(cls, native)
                callback, = cls.callbacks
                lifecycle_function(callback, native)
                assert callback() == 1
                events.append('finally-success')
                del callback, cls
            assert sys.exception() is caller
        observed.append((fails, events))
    return observed

def observe_pre_region_raise(build, native=False):
    events = []
    def source():
        events.append('unreachable-iterable')
        raise AssertionError('unreachable region evaluated its source')
    try:
        raise KeyError('caller')
    except KeyError as caller:
        try:
            build(source)
        except ValueError as error:
            assert error.args == ('before region',)
            assert error.__context__ is caller
            assert sys.exception() is error
            error.__traceback__ = None
        else:
            raise AssertionError('original class failure was swallowed')
        assert sys.exception() is caller
    assert events == []
    return ('before region', events)
"""


@pytest.mark.parametrize("case_name", tuple(_CLASS_LIFECYCLE_NATIVE_CASES))
def test_class_lifecycle_distinct_native_behavior_ordinary_control(case_name):
    body, function_name, observe = _CLASS_LIFECYCLE_NATIVE_CASES[case_name]
    namespace = {"__name__": f"ordinary_lifecycle_{case_name}"}
    exec(compile(body, str(Path(__file__)), "exec", dont_inherit=True), namespace)
    exec(_CLASS_FRAME_COUPLING_VALIDATOR + _CLASS_LIFECYCLE_NATIVE_OBSERVERS, namespace)
    namespace["build"] = namespace[function_name]
    exec(observe, namespace)


@pytest.fixture(scope="module")
def class_lifecycle_native_project(tmp_path_factory):
    sources = {}
    modules = {}
    for name, (body, _, _) in _CLASS_LIFECYCLE_NATIVE_CASES.items():
        module_name = f"native_lifecycle_{name}"
        path = f"{module_name}.py"
        sources[path] = strict_opt_in(body.encode(), path)[0].decode()
        sources[f"ordinary_lifecycle_{name}.py"] = body
        modules[module_name] = path
    return create_strict_project(
        tmp_path_factory.mktemp("strict-class-lifecycle-native"),
        sources,
        modules=modules,
        backend="cpython",
    )


@pytest.mark.parametrize("case_name", tuple(_CLASS_LIFECYCLE_NATIVE_CASES))
def test_cpython_class_lifecycle_distinct_behavior_matches_ordinary(
    class_lifecycle_native_project, case_name
):
    _, function_name, observe = _CLASS_LIFECYCLE_NATIVE_CASES[case_name]
    module_name = f"native_lifecycle_{case_name}"
    native_observe = observe[:-1] + ", native=True)"
    class_lifecycle_native_project.run_case(
        module_name,
        _CLASS_FRAME_COUPLING_VALIDATOR
        + _class_frame_cpython_validator(module_name)
        + _CLASS_LIFECYCLE_NATIVE_OBSERVERS
        + f"""
import {module_name} as actual
import ordinary_lifecycle_{case_name} as ordinary
assert _soac_ext.strict_module_diagnostics(ordinary) is None
build = getattr(ordinary, {function_name!r})
expected = {observe}
build = getattr(actual, {function_name!r})
observed = {native_observe}
assert observed == expected, (observed, expected)
assert _soac_ext.strict_function_diagnostics(build)['original_code_entered']
""",
        Path(__file__),
        required_functions=(function_name,),
        
        backend="cpython",
    )
