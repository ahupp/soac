# module:descriptors
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
# module:descriptor_support
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
# ok
# test_builtin_descriptors_are_adopted_before_callbacks
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Methods.static', 'Methods.class_method'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
if __dp_integration_mode__ == 'cpython':
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1
    diagnostic = _soac_ext.strict_module_diagnostics(module)
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
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry
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
if __dp_integration_mode__ == 'cpython':
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
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1
    diagnostic = _soac_ext.strict_module_diagnostics(module)
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
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Methods.static', 'Methods.class_method'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
