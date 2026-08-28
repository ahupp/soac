# modes:soac,entry
# test_strict_module_preconditions.py::test_reviewed_preconditions_use_authenticated_entries[named_builtin_uses_its_live_captured_mapping_like_cpython]
# module:named_builtin_uses_its_live_captured_mapping_like_cpython
# soac: module(strict_assign=true, checked_attr=true)

__builtins__ = {"len": lambda value: 41}


def call(value):
    return len(value)
# module:ordinary_named_builtin_uses_its_live_captured_mapping_like_cpython
__builtins__ = {"len": lambda value: 41}


def call(value):
    return len(value)
# ok
# named_builtin_uses_its_live_captured_mapping_like_cpython
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
    stock = importlib.import_module('ordinary_named_builtin_uses_its_live_captured_mapping_like_cpython')
    _assert_ordinary_precondition_module(stock, ('call',))
    expected = _observe_captured_builtin_mutation(stock)
    assert expected == (41, 52)
    soac = module
    actual = _observe_captured_builtin_mutation(soac)
    assert actual == expected

validate_module(module)
