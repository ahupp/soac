# modes:soac,entry
# module:model
# soac: module(strict_assign=true, checked_attr=true)
import support

class Base:
    def __init_subclass__(cls):
        support.observe(cls)

class Child(Base):
    value: int = 7

    def method(self) -> int:
        return self.value
# module:support
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
# ok
# test_pending_class_completion_installs_checks_on_actual_field_writes
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
