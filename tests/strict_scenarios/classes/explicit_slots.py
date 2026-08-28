# modes:soac,entry
# module:slot_model
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
# module:slot_support
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
# ok
# test_source_requested_slots_keep_real_members_pending_until_final_admission
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
assert _soac_ext.strict_function_entry_kind(model.Base.read) == expected_entry
assert _soac_ext.strict_function_entry_kind(model.Child.text) == expected_entry
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
assert type(vars(dictionary)) is dict and vars(dictionary) == {'extra': 11}
vars(dictionary)['value'] = 'hidden'  # Independent mapping entry, not the slot.
assert dictionary.value == 10 and vars(dictionary)['value'] == 'hidden'
rejected(lambda: setattr(dictionary, 'value', 'wrong'))
