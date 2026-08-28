"""Builtin descriptor admission through real offline facts and construction."""


import pytest

from tests._strict_integration import create_strict_project


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


# Retained harness: Deliberately extracts native owner/birth pointers and invokes
# PySoac_NewBuiltinDescriptor to construct a new witness; retain the explicit manual native-ABI
# reconstruction test.
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
