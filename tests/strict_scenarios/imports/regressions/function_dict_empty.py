# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[function_dict_empty]
# module:function_dict_empty
# soac: module(strict_assign=true, checked_attr=true)
def value():
    pass

VALUE = value.__dict__
# ok
# function_dict_empty
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.VALUE == {}
