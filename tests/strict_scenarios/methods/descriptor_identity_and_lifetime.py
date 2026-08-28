# modes:soac,entry
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
# test_descriptor_evaluation_order_and_rebound_or_chained_fallback
import sys
from builtins import staticmethod
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
# ok
# test_descriptor_birth_belongs_to_one_namespace_not_source_text
import sys
from builtins import staticmethod
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
# ok
# test_descriptor_birth_does_not_retain_class_or_an_unreachable_function_cycle
import sys
from builtins import staticmethod
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
