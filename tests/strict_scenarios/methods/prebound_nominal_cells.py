# module:nominal_prebound_closures
# soac: module(strict_assign=true, checked_attr=true)

def factory():
    class Target:
        pass
    Alias = Target
    class Holder:
        def accept(self, value: Alias) -> Alias:
            return value
    return Target, Holder

first = factory()
second = factory()
# ok
# test_prebound_closure_aliases_keep_owned_calls_after_cell_mutation
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
for _path in ('factory',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
left_target, left_holder = module.first
right_target, right_holder = module.second
assert left_target is not right_target and left_holder is not right_holder
assert left_target.__qualname__ == right_target.__qualname__
assert left_holder.__qualname__ == right_holder.__qualname__
left = left_target()
right = right_target()
rows = (
    (left_holder.accept, left_holder(), left, right, left_target, right_target),
    (right_holder.accept, right_holder(), right, left, right_target, left_target),
)
for function, receiver, accepted, rejected, selected, other in rows:
    assert_adopted_function(function, entered=False)
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    assert cells["Alias"].cell_contents is selected
    assert function(receiver, accepted) is accepted
    assert function(receiver, rejected) is rejected
    marker = object()
    assert function(receiver, marker) is marker
    assert_adopted_function(function, entered=True)
    for replacement in (other, None):
        cells["Alias"].cell_contents = replacement
        assert cells["Alias"].cell_contents is replacement
        assert function(receiver, accepted) is accepted
        assert function(receiver, rejected) is rejected
        assert function(receiver, marker) is marker
        assert_adopted_function(function, entered=True)
print("prebound-closure-calls-remain-ordinary")
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('factory',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
