"""Builtin descriptor admission through real offline facts and construction."""

import hashlib
import json
from pathlib import Path

import pytest

from tests._strict_integration import ROOT, create_strict_project

_CACHED_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from functools import cached_property

class Cached:
    def __init__(self):
        self.value = 3
        self.hits = 0

    @cached_property
    def computed(self) -> int:
        self.hits += 1
        return self.value * 2

    def echo(self, value: int) -> int:
        return value
"""


@pytest.fixture(scope="module")
def cached_descriptor_project(request, tmp_path_factory):
    backend = getattr(request, "param", "soac")
    return create_strict_project(
        tmp_path_factory.mktemp(f"strict-cached-descriptor-{backend}"),
        {"cached.py": _CACHED_SOURCE},
        modules={"cached": "cached.py"},
        backend=backend,
    )


@pytest.mark.parametrize(
    ("cached_descriptor_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["cached_descriptor_project"],
    scope="module",
)
def test_cached_property_keeps_original_descriptor_and_dynamic_cache_semantics(
    cached_descriptor_project, entry_interpreter
):
    results = []
    for strict in (False, True):
        source = _CACHED_SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1)
        load = (
            "import cached as module\n"
            if strict
            else "module = types.ModuleType('cached')\n"
            + "sys.modules['cached'] = module\n"
            + f"exec(compile({source!r}, '<ordinary cached property>', 'exec'), vars(module))\n"
        )
        expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
        witness = (
            "assert _soac_ext.strict_module_diagnostics(module)['sealed']\n"
            f"assert _soac_ext.strict_function_entry_kind(module.Cached.echo) == {expected_entry!r}\n"
            if strict
            else "assert _soac_ext.strict_module_diagnostics(module) is None\n"
        )
        completion_witness = ""
        if strict and cached_descriptor_project.backend == "cpython":
            source_path = cached_descriptor_project.project / "cached.py"
            source_sha256 = hashlib.sha256(source_path.read_bytes()).hexdigest()
            witness = (
                f"sys.path.insert(0, {str(ROOT)!r})\n"
                "from tests._strict_integration import (\n"
                "    _assert_cpython_function_witness, _assert_cpython_module_witness,\n"
                ")\n"
                "from tests.test_strict_type_native import ConstructionInfoV1\n"
                "diagnostic = _assert_cpython_module_witness(\n"
                "    module, module_name='cached',\n"
                f"    source_path={str(source_path)!r}, source_sha256={source_sha256!r},\n"
                f"    artifact_generation={cached_descriptor_project.publication['generation']!r},\n"
                ")\n"
                + """
for function in (module.Cached.__init__, module.Cached.echo,
                 vars(module.Cached)['computed'].func):
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed['finalized'] is False
del function
construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
construction.restype = ctypes.c_int
info = ConstructionInfoV1()
assert construction(module.Cached, ctypes.byref(info), ctypes.sizeof(info)) == 0
assert (info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction) == (
    0, 0, 0, 0, None, None,
)
"""
            )
            completion_witness = """
observed = _assert_cpython_function_witness(
    module.Cached.echo, diagnostic,
)
assert observed['finalized'] is False and observed['original_code_entered']
# Replacing this ordinary descriptor's component does not mint source authority.
assert _soac_ext.strict_function_diagnostics(descriptor.func) is None
generic_get = ctypes.pythonapi.PyObject_GenericGetAttr
generic_get.argtypes = [ctypes.py_object, ctypes.py_object]
generic_get.restype = ctypes.py_object
for _ in range(128):
    assert instance.computed == ('changed', 7)
assert generic_get(instance, 'computed') == ('changed', 7)
assert vars(instance) is replacement and instance.hits == 0
assert construction(module.Cached, ctypes.byref(info), ctypes.sizeof(info)) == 0
assert (info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction) == (
    0, 0, 0, 0, None, None,
)
"""
        completed = cached_descriptor_project.run(
            "import ctypes, functools, json, sys, types\n"
            + load
            + witness
            + """
owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert owner(module.Cached) is None
descriptor = vars(module.Cached)['computed']
assert type(descriptor) is functools.cached_property
assert descriptor.attrname == 'computed'
instance = module.Cached()
assert instance.__dict__ == {'value': 3, 'hits': 0}
assert instance.computed == 6 and instance.computed == 6 and instance.hits == 1
instance.value = 5
assert instance.computed == 6
instance.computed = 'assigned'
assert instance.computed == 'assigned' and instance.hits == 1
del instance.computed
assert instance.computed == 10 and instance.hits == 2
assert vars(module.Cached)['computed'] is descriptor

# The descriptor is an ordinary mutable stdlib object, not a replacement or a
# frozen dependency. Its actual changed component runs on the next cache miss.
descriptor.func = lambda self: ('changed', self.value)
assert instance.computed == 10
del instance.computed
assert instance.computed == ('changed', 5) and instance.hits == 2
replacement = {'value': 7, 'hits': 0, 'computed': 'replacement cache'}
instance.__dict__ = replacement
assert vars(instance) is replacement and instance.computed == 'replacement cache'
del instance.computed
assert instance.computed == ('changed', 7)
assert instance.echo('ordinary argument') == 'ordinary argument'
print(json.dumps({'values': vars(instance), 'descriptor_type': type(descriptor).__name__}))
"""
            + completion_witness,
            entry_interpreter=entry_interpreter,
        )
        results.append(json.loads(completed.stdout.splitlines()[-1]))
    assert results[1] == results[0]


@pytest.fixture(scope="module")
def descriptors(request, tmp_path_factory):
    backend = getattr(request, "param", "soac")
    return create_strict_project(
        tmp_path_factory.mktemp(f"strict-source-descriptors-{backend}"),
        {
            "descriptors.py": """
# soac: module(strict_assign=true, checked_attr=true)
from builtins import staticmethod, staticmethod as builtin_staticmethod
from descriptor_support import before_ready, default_value, identity, unknown_result

class Base:
    def __init_subclass__(cls):
        before_ready(cls)

class Methods(Base):
    value: int = 7

    @staticmethod
    def static(value: int = 3) -> int:
        return value

    @classmethod
    def class_method(cls, value: int) -> int:
        return value

    @property
    def read(self) -> int:
        return self.value

    @property
    def wrong(self) -> int:
        return unknown_result()

def family(callback):
    class Local:
        @builtin_staticmethod
        def method(value: int) -> int:
            return value
        callback(locals())
    return Local

class Chained:
    @staticmethod
    @identity
    def method(value: int) -> int:
        return value

# Evaluate the factory before the default expression mutates its binding.
class Ordered:
    @staticmethod
    def method(value: int = default_value(globals())) -> int:
        return value

# The same signed spelling now denotes an ordinary callable at runtime.
class Rebound:
    @staticmethod
    def method(value: int) -> int:
        return value
""",
            "descriptor_support.py": """
from typing import Any
events = []

def identity(function: Any) -> Any:
    events.append('identity')
    return function

def unknown_result() -> Any:
    return 'wrong'

def default_value(namespace: Any) -> int:
    def rebound(function):
        events.append('rebound')
        return staticmethod(function)
    events.append('default')
    namespace['staticmethod'] = rebound
    return 13

def before_ready(cls: Any) -> None:
    import ctypes
    from soac.strict import StrictMutationError
    class ConstructionInfo(ctypes.Structure):
        _fields_ = [
            ('abi_version', ctypes.c_uint32), ('struct_size', ctypes.c_uint32),
            ('phase', ctypes.c_uint32), ('permanent_contract_published', ctypes.c_uint32),
            ('owner', ctypes.c_void_p), ('root_construction', ctypes.c_void_p),
        ]
    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfo), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfo()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 1 and not info.permanent_contract_published
    assert info.owner and info.root_construction
    birth = ctypes.pythonapi.PySoac_GetDescriptorBirthOwner
    birth.argtypes = [ctypes.py_object]
    birth.restype = ctypes.c_void_p
    sealed = ctypes.pythonapi._PySoac_IsDescriptorSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    for name in ('static', 'class_method', 'read', 'wrong'):
        descriptor = vars(cls)[name]
        assert birth(descriptor), (name, 'callback saw no authenticated descriptor birth')
        assert sealed(descriptor) == 1, (name, 'callback saw an unsealed descriptor')
        try:
            type(descriptor).__init__(descriptor, lambda *args: None)
        except StrictMutationError:
            pass
        else:
            raise AssertionError('descriptor component changed during class callback')
    try:
        cls()
    except StrictMutationError:
        pass
    else:
        raise AssertionError('descriptor callback allocated a pending type')
    for call in (lambda: cls.static('wrong'), lambda: cls.class_method('wrong')):
        assert call() == 'wrong', 'a method annotation changed a pending-type callback'
    events.append('pre-ready')
""",
        },
        modules={"descriptors": "descriptors.py"},
        backend=backend,
    )


_PRELUDE = """
import ctypes
import descriptors as module
import descriptor_support as support
from soac.strict import StrictMutationError

sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
descriptor_sealed = ctypes.pythonapi._PySoac_IsDescriptorSealed
descriptor_sealed.argtypes = [ctypes.py_object]
descriptor_sealed.restype = ctypes.c_int
birth = ctypes.pythonapi.PySoac_GetDescriptorBirthOwner
birth.argtypes = [ctypes.py_object]
birth.restype = ctypes.c_void_p
assert _soac_ext.strict_module_diagnostics(module)['sealed']
"""


@pytest.mark.parametrize(
    ("descriptors", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["descriptors"],
    scope="module",
)
def test_builtin_descriptors_are_adopted_before_callbacks(descriptors, entry_interpreter):
    expected_entry = (
        "original_code" if descriptors.backend == "cpython"
        else "entry_interpreter" if entry_interpreter else "checked_native"
    )
    program = f"""
assert sealed(module.Methods) == 1
assert support.events == ['pre-ready', 'identity', 'default', 'rebound']
instance = module.Methods()
assert module.Methods.static() == 3 and instance.static(9) == 9
assert module.Methods.class_method(11) == 11 and instance.class_method(12) == 12
assert instance.read == 7
# A descriptor's authenticated birth and seal do not check its return value.
assert instance.wrong == 'wrong'
for name in ('static', 'class_method', 'read', 'wrong'):
    descriptor = vars(module.Methods)[name]
    function = descriptor.fget if type(descriptor) is property else descriptor.__func__
    assert birth(descriptor) and descriptor_sealed(descriptor)
    assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}
    for operation in (lambda: type(descriptor).__init__(descriptor, lambda *args: None),
                      lambda: setattr(function, '__code__', (lambda *args: None).__code__),
                      lambda: setattr(module.Methods, name, object())):
        try:
            operation()
        except StrictMutationError:
            pass
        else:
            raise AssertionError('sealed descriptor or component was mutable')
for operation in (lambda: setattr(instance, 'read', 1), lambda: delattr(instance, 'read')):
    try:
        operation()
    except AttributeError as error:
        assert not isinstance(error, StrictMutationError), type(error)
    else:
        raise AssertionError('getter-only property lost ordinary data-descriptor behavior')
vars(instance)['read'] = 99
assert instance.read == 7 and vars(instance)['read'] == 99
"""
    if descriptors.backend != "cpython":
        descriptors.run(_PRELUDE + program, entry_interpreter=entry_interpreter)
        return

    source_path = descriptors.project / "descriptors.py"
    source_sha256 = hashlib.sha256(source_path.read_bytes()).hexdigest()
    witness = (
        "from tests._strict_integration import (\n"
        "    _assert_cpython_function_witness, _assert_cpython_module_witness,\n"
        ")\n"
        "from tests.test_strict_type_native import ConstructionInfoV1\n"
        "diagnostic = _assert_cpython_module_witness(\n"
        "    module, module_name='descriptors',\n"
        f"    source_path={str(source_path)!r}, source_sha256={source_sha256!r},\n"
        f"    artifact_generation={descriptors.publication['generation']!r},\n"
        ")\n"
        + """
type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
type_owner.argtypes = [ctypes.py_object]
type_owner.restype = ctypes.c_void_p
construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
construction.restype = ctypes.c_int
info = ConstructionInfoV1()
assert construction(module.Methods, ctypes.byref(info), ctypes.sizeof(info)) == 1
assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
assert info.phase == 3 and info.permanent_contract_published == 1
assert info.owner == type_owner(module.Methods) and info.owner is not None
for name in ('static', 'class_method', 'read', 'wrong'):
    raw = vars(module.Methods)[name]
    component = raw.fget if type(raw) is property else raw.__func__
    observed = _assert_cpython_function_witness(
        component, diagnostic,
    )
    assert observed['finalized']
"""
    )
    native_paths = """
generic_get = ctypes.pythonapi.PyObject_GenericGetAttr
generic_get.argtypes = [ctypes.py_object, ctypes.py_object]
generic_get.restype = ctypes.py_object
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
generic_set.restype = ctypes.c_int
# Ignored dictionary shadows must not replace protected method dispatch.
vars(instance)['static'] = 'ordinary dictionary shadow'
vars(instance)['class_method'] = 'ordinary class-method shadow'
for value in range(128):
    assert instance.read == 7
    assert instance.static(value) == value
    assert instance.class_method(value) == value
assert object.__getattribute__(instance, 'static')(31) == 31
assert generic_get(instance, 'static')(32) == 32
assert generic_get(instance, 'class_method')(33) == 33
assert generic_get(instance, 'read') == 7 and vars(instance)['read'] == 99
for operation in (lambda: setattr(instance, 'static', object()),
                  lambda: object.__setattr__(instance, 'class_method', object())):
    try:
        operation()
    except StrictMutationError:
        pass
    else:
        raise AssertionError('protected method name accepted an attribute replacement')
try:
    generic_set(instance, 'read', 4)
except AttributeError as error:
    assert not isinstance(error, StrictMutationError), type(error)
else:
    raise AssertionError('native getter-only property assignment succeeded')
assert generic_get(instance, 'wrong') == 'wrong'
assert vars(instance)['static'] == 'ordinary dictionary shadow'
assert vars(instance)['class_method'] == 'ordinary class-method shadow'
"""
    descriptors.run_case(
        "descriptors",
        "from soac import _soac_ext\n" + _PRELUDE + witness
        + program + native_paths + witness,
        Path(__file__),
        backend="cpython",
        required_functions=("Methods.static", "Methods.class_method"),
        
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_descriptor_evaluation_order_and_rebound_or_chained_fallback(
    descriptors, entry_interpreter
):
    descriptors.run(
        _PRELUDE
        + """
assert sealed(module.Ordered) == 1
assert module.Ordered.method() == 13
assert birth(vars(module.Ordered)['method'])
assert support.events == ['pre-ready', 'identity', 'default', 'rebound']
for cls in (module.Rebound, module.Chained):
    descriptor = vars(cls)['method']
    assert sealed(cls) == 0 and descriptor_sealed(descriptor) == 0 and not birth(descriptor)
    assert cls.method('ordinary') == 'ordinary'
    staticmethod.__init__(descriptor, lambda value: ('replaced', value))
    assert cls.method(4) == ('replaced', 4)
    cls.method = 'ordinary mutation'
    assert cls.method == 'ordinary mutation'
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_descriptor_birth_belongs_to_one_namespace_not_source_text(
    descriptors, entry_interpreter
):
    descriptors.run(
        _PRELUDE
        + """
# Each call executes the same source definition with an independent namespace.
first = module.family(lambda namespace: None)
assert sealed(first) == 1
original = vars(first)['method']
assert birth(original) and descriptor_sealed(original)
second = module.family(lambda namespace: namespace.__setitem__('method', original))
assert second is not first and sealed(second) == 0
assert vars(second)['method'] is original
# Its metadata contract remains permanent inside the dynamic second class;
# the annotated call itself retains ordinary value semantics.
assert second.method('wrong') == 'wrong'
assert descriptor_sealed(original)
try:
    original.__func__.__code__ = original.__func__.__code__
except StrictMutationError:
    pass
else:
    raise AssertionError('borrowing a sealed descriptor revoked its component seal')
second.method = lambda value: value
assert second.method('ordinary') == 'ordinary'

def copy_component(namespace):
    current = namespace['method']
    assert birth(current) and not descriptor_sealed(current)
    namespace['method'] = staticmethod(current.__func__)
third = module.family(copy_component)
assert sealed(third) == 0
copied = vars(third)['method']
assert not birth(copied) and not descriptor_sealed(copied)
assert third.method('ordinary') == 'ordinary'
staticmethod.__init__(copied, lambda value: ('ordinary replacement', value))
assert third.method(6) == ('ordinary replacement', 6)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_descriptor_birth_does_not_retain_class_or_an_unreachable_function_cycle(
    descriptors, entry_interpreter
):
    descriptors.run(
        _PRELUDE
        + """
import gc
import weakref

def exercise(make):
    cls = make(lambda namespace: None)
    cls_ref = weakref.ref(cls)
    descriptor = vars(cls)['method']
    function_ref = weakref.ref(descriptor.__func__)
    del cls
    gc.collect()
    assert cls_ref() is None, 'descriptor birth retained its defining type'
    assert descriptor(3) == 3 and function_ref() is not None
    del descriptor
    gc.collect()
    assert function_ref() is None, 'birth retained the released function'

    def make_cycle(namespace):
        descriptor = namespace['method']
        descriptor.__func__.cycle = descriptor
    cls = make(make_cycle)
    function_ref = weakref.ref(vars(cls)['method'].__func__)
    cls_ref = weakref.ref(cls)
    del cls
    gc.collect()
    assert cls_ref() is None and function_ref() is None

# Compare the same ownership patterns with ordinary builtin descriptors.
def ordinary(callback):
    class Local:
        @staticmethod
        def method(value):
            return value
        callback(locals())
    return Local

exercise(ordinary)
exercise(module.family)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_descriptor_native_reconstruction_cannot_reuse_an_exposed_birth_witness(
    descriptors, entry_interpreter
):
    descriptors.run(
        _PRELUDE
        + """
function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
function_owner.argtypes = [ctypes.py_object]
function_owner.restype = ctypes.c_void_p
new_descriptor = ctypes.pythonapi.PySoac_NewBuiltinDescriptor
new_descriptor.argtypes = [ctypes.py_object] * 5
new_descriptor.restype = ctypes.py_object
birth_id = ctypes.pythonapi.PySoac_GetDescriptorBirthId
birth_id.argtypes = [ctypes.py_object]
birth_id.restype = ctypes.c_uint64

def reconstruct(namespace):
    original = namespace['method']
    function = original.__func__
    witness = ctypes.cast(birth(original), ctypes.py_object).value
    owner = ctypes.cast(function_owner(function), ctypes.py_object).value
    replacement = new_descriptor(staticmethod, function, owner, function.__code__, witness)
    assert replacement is not original and birth(replacement) == birth(original)
    assert birth_id(original) and birth_id(replacement) != birth_id(original)
    namespace['method'] = replacement

cls = module.family(reconstruct)
assert sealed(cls) == 0, 'a new native birth reused another descriptor producer witness'
assert descriptor_sealed(vars(cls)['method']) == 0
assert cls.method('ordinary') == 'ordinary'
""",
        entry_interpreter=entry_interpreter,
    )
