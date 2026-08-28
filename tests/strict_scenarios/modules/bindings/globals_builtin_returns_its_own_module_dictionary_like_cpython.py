# modes:soac,entry
# test_strict_module_preconditions.py::test_reviewed_preconditions_use_authenticated_entries[globals_builtin_returns_its_own_module_dictionary_like_cpython]
# module:globals_builtin_returns_its_own_module_dictionary_like_cpython
# soac: module(strict_assign=true, checked_attr=true)

def read_globals():
    return globals()
# module:ordinary_globals_builtin_returns_its_own_module_dictionary_like_cpython
def read_globals():
    return globals()
# ok
# globals_builtin_returns_its_own_module_dictionary_like_cpython
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
    stock = importlib.import_module('ordinary_globals_builtin_returns_its_own_module_dictionary_like_cpython')
    _assert_ordinary_precondition_module(stock, ('read_globals',))
    expected = stock.read_globals() is stock.__dict__
    assert expected is True
    soac = module
    actual = soac.read_globals() is soac.__dict__
    assert actual is expected

validate_module(module)
