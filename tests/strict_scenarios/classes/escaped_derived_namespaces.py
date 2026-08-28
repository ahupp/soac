# modes:soac,entry
# module:model
# soac: module(strict_assign=true, checked_attr=true)

class Base:
    pass

def make():
    class Child(Base):
        pass
    return Child
# ok
# test_escaped_derived_class_namespace_does_not_retain_its_type
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
import gc
import weakref
import model

is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int

# The control executes the same class/factory declarations as ordinary code.
# A derived methodless class has no own __dict__ descriptor retaining its type.
ordinary = {'__name__': 'ordinary_lifetime_control'}
exec(compile('\nclass Base:\n    pass\n\ndef make():\n    class Child(Base):\n        pass\n    return Child\n', '<ordinary-lifetime-control>', 'exec', dont_inherit=True), ordinary)

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
    assert expected == (True, ['class released'], {}), expected
    actual = collect_with_escaped_dictionary(model.make, 1, class_namespace)
    assert actual == expected, (class_namespace, actual, expected)
