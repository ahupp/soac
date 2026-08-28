# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[lambda_function_decorator]
# module:lambda_function_decorator
# soac: module(strict_assign=true, checked_attr=true)
def keep(value):
    def decorator(func):
        return value
    return decorator

sentinel = object()

@keep(lambda: sentinel)
def chosen():
    return None

VALUE = chosen()
# ok
# lambda_function_decorator
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.VALUE is module.sentinel
