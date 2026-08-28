# modes:soac,entry
# test_strict_module_preconditions.py::test_reviewed_preconditions_use_authenticated_entries[unsealed_module_global_replacement_matches_cpython_function_globals]
# module:unsealed_module_global_replacement_matches_cpython_function_globals
# soac: module(strict_assign=true, checked_attr=true)

def target(value):
    return value + 1


def replacement(value):
    return value + 10


def call(value):
    return target(value)
# module:ordinary_unsealed_module_global_replacement_matches_cpython_function_globals
def target(value):
    return value + 1


def replacement(value):
    return value + 10


def call(value):
    return target(value)
# ok
# unsealed_module_global_replacement_matches_cpython_function_globals
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
    stock = importlib.import_module('ordinary_unsealed_module_global_replacement_matches_cpython_function_globals')
    _assert_ordinary_precondition_module(stock, ('target', 'replacement', 'call'))

    mutation = 'function_globals'
    assert _observe_global_replacement(stock, mutation) == (3, 12)
    original = module.target
    namespace = module.__dict__
    assert module.call.__globals__ is namespace
    assert module.call(2) == 3
    with pytest.raises(StrictMutationError):
        _replace_global(module, mutation)
    assert namespace["target"] is original
    assert module.target is original
    assert module.call.__globals__ is namespace
    assert module.call(2) == 3

validate_module(module)
