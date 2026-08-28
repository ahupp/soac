# modes:soac,entry
# module:dynamic_nominals
# soac: module(strict_assign=true, checked_attr=true)
from dynamic_nominal_support import Meta, Outside, events, wrong_value

class Record(metaclass=Meta):
    def echo(self, value: int) -> int:
        return value

def accept(value: Record) -> Record:
    events.append('accept')
    return value

def external(value: Outside) -> Outside:
    events.append('external')
    return value

def optional(value: Record | None) -> Record | None:
    return value

def wrong_return() -> Record:
    return wrong_value()

def invoke(value: Record, argument):
    return value.echo(argument)

def factory():
    class Local(metaclass=Meta):
        pass
    def accept_local(value: Local) -> Local:
        return value
    return Local, accept_local
# module:dynamic_nominal_support
from typing import Any

events = []

class Meta(type):
    def __instancecheck__(cls, value):
        raise AssertionError('nominal checking called __instancecheck__')

    def __subclasscheck__(cls, value):
        raise AssertionError('nominal checking called __subclasscheck__')

class Outside(metaclass=Meta):
    pass

def wrong_value() -> Any:
    return object()
# ok
# test_dynamic_and_external_nominals_do_not_require_a_layout_contract
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
import dynamic_nominals as module
from dynamic_nominal_support import Outside, events

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
function_owner.argtypes = [ctypes.py_object]
function_owner.restype = ctypes.c_void_p
sealed_function = ctypes.pythonapi.PyFunction_GetSoacStrictId
sealed_function.argtypes = [ctypes.py_object]
sealed_function.restype = ctypes.c_uint64
assert _soac_ext.strict_module_diagnostics(module)['sealed']
for cls in (module.Record, Outside):
    assert owner(cls) is None
for function in (module.accept, module.external, module.optional,
                 module.wrong_return, module.invoke):
    assert function_owner(function) and sealed_function(function)
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry
assert sealed_function(module.Record.echo) == 0

class Child(module.Record):
    pass

for value in (module.Record(), Child()):
    assert module.accept(value) is value
    assert module.optional(value) is value
assert module.optional(None) is None
outside = Outside()
assert module.external(outside) is outside
assert events == ['accept', 'accept', 'external']

class Spoof:
    @property
    def __class__(self):
        raise AssertionError('nominal checking called the __class__ property')

for value in (object(), outside, Spoof()):
    before = list(events)
    assert module.accept(value) is value
    assert events == before + ['accept']
for function, arguments in ((module.external, (module.Record(),)),
                            (module.optional, (object(),)),
                            (module.wrong_return, ())):
    result = function(*arguments)
    if arguments:
        assert result is arguments[-1]
    else:
        assert type(result) is object

# A function annotation grants no class, field, or method capability.
record = module.Record()
assert module.invoke(record, 'ordinary method') == 'ordinary method'
module.Record.echo = lambda self, value: ('replacement', value)
assert module.invoke(record, 3) == ('replacement', 3)
record.echo = lambda value: ('instance', value)
assert module.invoke(record, 4) == ('instance', 4)
assert owner(module.Record) is None
# ok
# test_dynamic_factory_nominals_keep_distinct_classes_and_collectable_cycles
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import gc
import weakref
import dynamic_nominals as module

first, accept_first = module.factory()
second, accept_second = module.factory()
assert first is not second and first.__qualname__ == second.__qualname__
for actual, accepted, rejected in ((accept_first, first(), second()),
                                   (accept_second, second(), first())):
    assert actual(accepted) is accepted
    assert actual(rejected) is rejected

provider = accept_first.__annotate__
cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
assert 'Local' in cells
cells['Local'].cell_contents = second
value = first()
assert accept_first(value) is value
other_value = second()
assert accept_first(other_value) is other_value

def collectable():
    cls, function = module.factory()
    return weakref.ref(cls), weakref.ref(function)

references = collectable()
gc.collect()
assert all(reference() is None for reference in references)
