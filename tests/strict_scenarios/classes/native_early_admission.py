# modes:cpython
# module:early_class
# soac: module(strict_assign=true, checked_attr=true)

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
# module:ordinary_early_class
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
# module:early_class_probe
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
# ok
# test_cpython_class_admission_seals_metadata_without_checking_forward_annotations
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Consumer.early', 'Consumer.forward'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
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
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Consumer.early', 'Consumer.forward'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
