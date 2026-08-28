# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[dict_conditional_value]
# module:dict_conditional_value
# soac: module(strict_assign=true, checked_attr=true)
VALUE = {"flags": tuple([1]) if True else None}
# ok
# dict_conditional_value
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.VALUE == {"flags": (1,)}
