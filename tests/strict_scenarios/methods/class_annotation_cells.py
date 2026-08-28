# module:nominal_class_dictionary
# soac: module(strict_assign=true, checked_attr=true)
from nominal_class_dictionary_support import before_ready, events

class Token:
    pass

class Base:
    def __init_subclass__(cls):
        before_ready(cls)

class Child(Base):
    Alias = Token

    def accept(self, value: Alias) -> Alias:
        events.append("body")
        return value
# module:nominal_class_dictionary_support
from typing import Any

events = []

def class_dictionary_cell(function: Any) -> Any:
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    return cells["__classdict__"]

def before_ready(cls: Any) -> None:
    import ctypes
    from soac.strict import StrictMutationError
    from tests.test_strict_type_native import ConstructionInfoV1

    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 1 and info.permanent_contract_published == 0
    assert info.owner is not None and info.root_construction is not None
    try:
        cls()
    except StrictMutationError as error:
        assert type(error) is StrictMutationError
        events.append("pending-allocation")
    else:
        raise AssertionError("class namespace observer allocated a pending type")

    function = cls.accept
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(function)
    cell = class_dictionary_cell(function)
    actual = cell.cell_contents
    assert type(actual) is dict
    assert actual["accept"] is function
    value = actual["Alias"]()
    # No Child instance exists before admission. This unbound source method
    # does not access protected storage, so its call remains ordinary.
    receiver = object()
    assert function(receiver, value) is value
    events.append("before-ready")

    # A replacement annotation dictionary is not the type's namespace and
    # does not affect a body that merely returns its argument.
    cell.cell_contents = dict(actual)
    before = list(events)
    try:
        assert function(receiver, value) is value
    finally:
        cell.cell_contents = actual
    assert events == before + ["body"]
    assert function(receiver, value) is value
    events.append("restored")
# ok
# test_class_scoped_annotations_use_actual_cells_without_constraining_calls
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
from types import FunctionType
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

def native_api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

function_owner = native_api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p)
strict_id = native_api("PyFunction_GetSoacStrictId", ctypes.c_uint64)
function_metadata = native_api("PyFunction_GetSoacMetadata", ctypes.c_void_p)

def assert_adopted_function(function, *, entered=None):
    assert type(function) is FunctionType
    assert function_owner(function), "function lost its actual creation owner"
    assert strict_id(function) != 0, "adopted function is not natively sealed"
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry
    if expected_entry == "original_code":
        diagnostic = _assert_cpython_function_witness(
            function, _soac_ext.strict_module_diagnostics(module),
        )
        assert diagnostic["finalized"] is True, diagnostic
        if entered is not None:
            assert diagnostic["original_code_entered"] is entered, diagnostic
    else:
        assert function_metadata(function), "retained function lacks entry metadata"

from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Child.accept',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import nominal_class_dictionary as module
from nominal_class_dictionary_support import class_dictionary_cell, events

assert events == ["pending-allocation", "body", "before-ready", "body", "body", "restored"]
assert_adopted_function(module.Child.accept, entered=True)
receiver = module.Child()
value = module.Token()
assert receiver.accept(value) is value

class Foreign:
    pass

function = module.Child.accept
cell = class_dictionary_cell(function)
cell.cell_contents = {"Alias": Foreign}

# Annotation evaluation observes its actual cell, while calls remain ordinary
# and actual function ownership stays sealed.
assert function.__annotate__(1)["value"] is Foreign
assert receiver.accept(value) is value
before = list(events)
foreign = Foreign()
assert receiver.accept(foreign) is foreign
assert events == before + ["body"]
assert_adopted_function(module.Child.accept, entered=True)
print("class-dictionary-nominal-boundaries")
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Child.accept',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
