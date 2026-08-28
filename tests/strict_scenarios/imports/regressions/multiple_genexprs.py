# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[multiple_genexprs]
# module:multiple_genexprs
# soac: module(strict_assign=true, checked_attr=true)
def convert(value):
    return value

def positive(value):
    return value > 0

def value(items):
    converted = tuple(convert(item) for item in items)
    return all(positive(item) for item in converted)
# ok
# multiple_genexprs
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.value([1, 2, 3]) is True
assert sys.exception() is None
