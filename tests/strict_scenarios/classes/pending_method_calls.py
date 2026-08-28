# modes:soac
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
# test_pending_allocation_and_ordinary_method_calls_precede_init_subclass
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
