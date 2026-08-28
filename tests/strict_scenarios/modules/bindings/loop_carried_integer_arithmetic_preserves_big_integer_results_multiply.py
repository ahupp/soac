# modes:soac,entry
# test_strict_module_preconditions.py::test_reviewed_preconditions_use_authenticated_entries[loop_carried_integer_arithmetic_preserves_big_integer_results_multiply]
# module:loop_carried_integer_arithmetic_preserves_big_integer_results_multiply
# soac: module(strict_assign=true, checked_attr=true)
def call(active):
    value = 4611686018427387904
    while active:
        value = value * 2
        active = False
    return value
# module:ordinary_loop_carried_integer_arithmetic_preserves_big_integer_results_multiply
def call(active):
    value = 4611686018427387904
    while active:
        value = value * 2
        active = False
    return value
# ok
# loop_carried_integer_arithmetic_preserves_big_integer_results_multiply
import sys
import pytest
from soac import _soac_ext, import_hook
import importlib
import pytest
from soac.strict import StrictMutationError
from tests.test_strict_module_preconditions import (
    _assert_ordinary_precondition_module, _replace_global,
    _observe_global_replacement, _observe_late_builtin_shadow,
    _observe_captured_builtin_mutation,
)
def validate_module(module):
    stock = importlib.import_module('ordinary_loop_carried_integer_arithmetic_preserves_big_integer_results_multiply')
    _assert_ordinary_precondition_module(stock, ('call',))
    operation = 'multiply'
    initial = 4611686018427387904
    expression = 'value * 2'
    expected = 9223372036854775808
    stock_result = stock.call(True)
    assert stock_result == expected
    soac = module
    actual = soac.call(True)
    assert actual == stock_result

validate_module(module)
