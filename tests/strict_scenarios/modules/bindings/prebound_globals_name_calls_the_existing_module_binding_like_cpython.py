# modes:soac,entry
# test_strict_module_preconditions.py::test_reviewed_preconditions_use_authenticated_entries[prebound_globals_name_calls_the_existing_module_binding_like_cpython]
# module:prebound_globals_name_calls_the_existing_module_binding_like_cpython
# soac: module(strict_assign=true, checked_attr=true)

def replacement():
    return 41


globals = replacement


def call():
    return globals()
# module:ordinary_prebound_globals_name_calls_the_existing_module_binding_like_cpython
def replacement():
    return 41


globals = replacement


def call():
    return globals()
# ok
# prebound_globals_name_calls_the_existing_module_binding_like_cpython
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
    stock = importlib.import_module('ordinary_prebound_globals_name_calls_the_existing_module_binding_like_cpython')
    _assert_ordinary_precondition_module(stock, ('replacement', 'call'))
    expected = stock.call()
    assert expected == 41
    soac = module
    actual = soac.call()
    assert actual == expected

validate_module(module)
