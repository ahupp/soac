# module:nominal_aliases
# soac: module(strict_assign=true, checked_attr=true)
from nominal_alias_support import retarget_aliases

def factory():
    class Local:
        def accepts_referenced(self, value: Alias) -> Alias:
            return value
    Alias = Local
    First = Local
    Second = Local
    def two(first: First, second: Second) -> Second:
        return second
    def either(value: First | Second) -> First | Second:
        return value
    def wrong_return(first: First, second: Second) -> Second:
        return first
    retarget_aliases(Local.accepts_referenced.__annotate__, two.__annotate__,
                     either.__annotate__, Local)
    return Local, two, either, wrong_return

first = factory()
second = factory()
unresolved = factory()
# module:nominal_alias_support
from typing import Any

previous: Any = None
calls = 0

def retarget_aliases(method: Any, two: Any, either: Any, current: Any) -> None:
    global previous, calls
    if previous is not None:
        # Real ordinary metadata mutation before strict adoption. These are
        # actual original provider cells, not fabricated source/type facts.
        for provider, expected_name in ((method, "Alias"), (two, "Second"),
                                        (either, "Second")):
            cells = dict(zip(provider.__code__.co_freevars,
                             provider.__closure__ or ()))
            assert expected_name in cells, (expected_name, cells)
            cells[expected_name].cell_contents = previous if calls == 1 else None
    previous = current
    calls += 1
# ok
# test_nominal_aliases_preserve_actual_cells_without_runtime_call_predicates
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
import nominal_aliases as module

left_class, left_two, left_either, _ = module.first
right_class, right_two, right_either, right_wrong = module.second
left = left_class()
right = right_class()
assert type(left) is not type(right)
assert type(left).__qualname__ == type(right).__qualname__

# These methods froze when their own class completed, before the source
# assigned Alias. Later annotation-cell contents do not constrain calls.
method_rows = (
    (left_class.accepts_referenced, left, left_class),
    (right_class.accepts_referenced, right, left_class),
    (module.unresolved[0].accepts_referenced, module.unresolved[0](), None),
)
for function, receiver, actual_alias in method_rows:
    assert_adopted_function(function, entered=False)
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    assert cells["Alias"].cell_contents is actual_alias
    for value in (left, right, object()):
        assert function(receiver, value) is value
    assert_adopted_function(function, entered=True)

# Free functions still finish adoption at this initializing module's seal.
# Actual aliases and unions remain static facts, not runtime call predicates.
for function in (left_two, right_two, left_either, right_either, right_wrong):
    assert_adopted_function(function)
for function, accepted, rejected in (
    (left_either, left, right),
):
    assert function(accepted) is accepted
    assert function(rejected) is rejected

assert right_two(right, left) is left
assert left_two(left, left) is left
for arguments in ((left, left), (right, right)):
    assert right_two(*arguments) is arguments[-1]
for value in (left, right):
    assert right_either(value) is value
assert right_wrong(right, left) is right
marker = object()
assert right_either(marker) is marker
unresolved_class, _, unresolved_either, _ = module.unresolved
unresolved_value = unresolved_class()
assert unresolved_either(unresolved_value) is unresolved_value

# Metadata seals do not freeze the contents of annotation cells or turn those
# contents into a runtime call contract or layout proof.
for function, name in ((right_two, "Second"), (right_either, "First"),
                       (right_class.accepts_referenced, "Alias")):
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    assert name in cells
    cells[name].cell_contents = None
assert right_two(right, left) is left
assert right_class.accepts_referenced(right, left) is left
for value in (left, right):
    assert right_either(value) is value
# Mutating every genuine method-provider cell leaves body values and actual
# function ownership unchanged, including cells containing None.
for function, receiver, _ in method_rows:
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    for replacement in (right_class, left_class, None):
        cells["Alias"].cell_contents = replacement
        assert cells["Alias"].cell_contents is replacement
        for value in (left, right):
            assert function(receiver, value) is value
        assert_adopted_function(function, entered=True)
for function in (left_two, right_two, left_either, right_either, right_wrong):
    assert_adopted_function(function)
print("same-source-alias-bindings")
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('factory',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
