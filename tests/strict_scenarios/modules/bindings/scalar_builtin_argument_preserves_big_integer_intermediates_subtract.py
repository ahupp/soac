# modes:soac,entry
# test_strict_module_preconditions.py::test_reviewed_preconditions_use_authenticated_entries[scalar_builtin_argument_preserves_big_integer_intermediates_subtract]
# module:scalar_builtin_argument_preserves_big_integer_intermediates_subtract
# soac: module(strict_assign=true, checked_attr=true)
def call():
    return chr(((0 - 9223372036854775807) - 2) + 9223372036854775807 + 2)
# module:ordinary_scalar_builtin_argument_preserves_big_integer_intermediates_subtract
def call():
    return chr(((0 - 9223372036854775807) - 2) + 9223372036854775807 + 2)
# ok
# scalar_builtin_argument_preserves_big_integer_intermediates_subtract
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
    stock = importlib.import_module('ordinary_scalar_builtin_argument_preserves_big_integer_intermediates_subtract')
    _assert_ordinary_precondition_module(stock, ('call',))
    operation = 'subtract'
    expression = '((0 - 9223372036854775807) - 2) + 9223372036854775807 + 2'
    expected = stock.call()
    assert expected == '\x00'
    soac = module
    actual = soac.call()
    assert actual == expected

validate_module(module)
