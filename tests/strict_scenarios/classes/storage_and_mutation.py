# module:model
# soac: module(strict_assign=true, checked_attr=true)

class Box:
    value: int = 0

    def __init__(self, value: int):
        self.value = value

    def method(self) -> int:
        return self.value + 1
# ok
# test_strict_class_storage_and_mutation_boundaries
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Box.__init__', 'Box.method'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import ctypes
import model
from soac.strict import StrictMutationError

def rejected(operation):
    try:
        operation()
    except StrictMutationError:
        return
    raise AssertionError('protected mutation unexpectedly succeeded')

box = model.Box(3)
storage = vars(box)
assert type(storage) is dict and storage is box.__dict__
assert list(storage) == ['value']
storage['method'] = 'hidden dictionary value'
assert box.method() == 4
assert object.__getattribute__(box, 'method')() == 4
assert storage['method'] == 'hidden dictionary value'

def ordinary_access(value):
    value.value = value.value + 1
    return value.method()

for unused in range(2000):
    ordinary_access(box)
assert storage['value'] == 2003 and box.method() == 2004
rejected(lambda: setattr(box, 'method', lambda: -1))
rejected(lambda: object.__setattr__(box, 'method', lambda: -1))
# Class sealing does not select indexed storage or reject ordinary replacement.
# The incoming object becomes the actual dictionary; the escaped old alias is
# neither copied into nor kept authoritative by the class's method policy.
escaped_storage = storage
incoming_storage = dict(storage)
box.__dict__ = incoming_storage
assert vars(box) is incoming_storage and box.__dict__ is incoming_storage
assert escaped_storage is not incoming_storage
escaped_storage['value'] = -1000
assert box.value == 2003 and box.method() == 2004
assert escaped_storage['value'] == -1000
storage = incoming_storage

set_attr = ctypes.pythonapi.PyObject_SetAttr
set_attr.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
set_attr.restype = ctypes.c_int
rejected(lambda: set_attr(box, 'method', object()))
set_item = ctypes.pythonapi.PyDict_SetItem
set_item.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
set_item.restype = ctypes.c_int
assert set_item(storage, 'value', 11) == 0
assert box.method() == 12
assert set_item(storage, 'method', 'still hidden') == 0
assert box.method() == 12

copied = storage.copy()
assert type(copied) is dict and copied is not storage
copied.clear()
assert box.value == 11
storage.clear()
assert storage is vars(box) and not storage and box.value == 0
storage['other'] = 17
storage['value'] = 5
assert list(storage) == ['other', 'value'] and box.method() == 6
del box.value
assert box.value == 0 and list(storage) == ['other']
box.value = 8
assert box.method() == 9 and storage['value'] == 8

class Ordinary:
    pass
rejected(lambda: setattr(box, '__class__', Ordinary))
rejected(lambda: setattr(model.Box, 'method', lambda self: 99))
rejected(lambda: setattr(model.Box.method, '__code__', ordinary_access.__code__))
rejected(lambda: setattr(model, 'Box', Ordinary))
rejected(lambda: set_item(vars(model), 'Box', Ordinary))

class_dict = ctypes.pythonapi.PyType_GetDict
class_dict.argtypes = [ctypes.py_object]
class_dict.restype = ctypes.py_object
rejected(lambda: set_item(class_dict(model.Box), 'method', object()))
assert box.method() == 9
if __dp_integration_mode__ == 'cpython':
    import ctypes
    from soac import _soac_ext
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    def assert_native_class(cls):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
        assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
        assert info.phase == 3 and info.permanent_contract_published == 1
        assert info.owner == get_type_owner(cls) and info.owner is not None
        return info.owner
    assert_native_class(model.Box)
    for function in (model.Box.__init__, model.Box.method):
        assert _soac_ext.strict_function_diagnostics(function)["finalized"]
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Box.__init__', 'Box.method'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
