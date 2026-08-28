# modes:soac,entry
# test_strict_module_preconditions.py::test_reviewed_preconditions_use_authenticated_entries[known_builtin_uses_its_initial_captured_mapping_like_cpython]
# module:known_builtin_uses_its_initial_captured_mapping_like_cpython
# soac: module(strict_assign=true, checked_attr=true)

__builtins__ = {"ord": lambda value: 41}


def call(value):
    return ord(value)
# module:ordinary_known_builtin_uses_its_initial_captured_mapping_like_cpython
__builtins__ = {"ord": lambda value: 41}


def call(value):
    return ord(value)
# ok
# known_builtin_uses_its_initial_captured_mapping_like_cpython
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
    stock = importlib.import_module('ordinary_known_builtin_uses_its_initial_captured_mapping_like_cpython')
    _assert_ordinary_precondition_module(stock, ('call',))
    assert stock.call.__builtins__ is stock.__dict__['__builtins__']
    expected = stock.call('a')
    assert expected == 41
    soac = module
    assert soac.call.__builtins__ is soac.__dict__['__builtins__']
    actual = soac.call('a')
    assert actual == expected

validate_module(module)
