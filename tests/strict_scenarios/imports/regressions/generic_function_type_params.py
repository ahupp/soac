# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[generic_function_type_params]
# module:generic_function_type_params
# soac: module(strict_assign=true, checked_attr=true)
import typing

def generic[T]():
    pass

T, = generic.__type_params__
VALUE = isinstance(T, typing.TypeVar), generic.__type_params__
# ok
# generic_function_type_params
import sys
import pytest
from soac import _soac_ext, import_hook
is_type_var, type_params = module.VALUE
assert is_type_var is True
assert type_params == (type_params[0],)
