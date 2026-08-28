# modes:soac,entry
# test_strict_module_preconditions.py::test_reviewed_preconditions_use_authenticated_entries[late_module_global_shadows_builtin_like_cpython]
# module:late_module_global_shadows_builtin_like_cpython
# soac: module(strict_assign=true, checked_attr=true)

def call(value):
    return len(value)
# module:ordinary_late_module_global_shadows_builtin_like_cpython
def call(value):
    return len(value)
# ok
# late_module_global_shadows_builtin_like_cpython
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
    stock = importlib.import_module('ordinary_late_module_global_shadows_builtin_like_cpython')
    _assert_ordinary_precondition_module(stock, ('call',))
    expected = _observe_late_builtin_shadow(stock)
    assert expected == (3, 41)
    soac = module
    actual = _observe_late_builtin_shadow(soac)
    assert actual == expected

validate_module(module)
