# modes:soac,entry
# test_strict_module_preconditions.py::test_reviewed_preconditions_use_authenticated_entries[dynamic_scalar_builtin_argument_preserves_big_integer_intermediates_add]
# module:dynamic_scalar_builtin_argument_preserves_big_integer_intermediates_add
# soac: module(strict_assign=true, checked_attr=true)
def call(value):
    return chr((9223372036854775807 + ord(value)) - 9223372036854775807 - ord(value))
# module:ordinary_dynamic_scalar_builtin_argument_preserves_big_integer_intermediates_add
def call(value):
    return chr((9223372036854775807 + ord(value)) - 9223372036854775807 - ord(value))
# ok
# dynamic_scalar_builtin_argument_preserves_big_integer_intermediates_add
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
    stock = importlib.import_module('ordinary_dynamic_scalar_builtin_argument_preserves_big_integer_intermediates_add')
    _assert_ordinary_precondition_module(stock, ('call',))
    operation = 'add'
    expression = '(9223372036854775807 + ord(value)) - 9223372036854775807 - ord(value)'
    value = '\x01'
    expected = stock.call(value)
    assert expected == '\x00'
    soac = module
    actual = soac.call(value)
    assert actual == expected

validate_module(module)
