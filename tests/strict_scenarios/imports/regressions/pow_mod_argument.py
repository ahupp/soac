# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[pow_mod_argument]
# module:pow_mod_argument
# soac: module(strict_assign=true, checked_attr=true)
VALUE = pow(2, 5, 7)
# ok
# pow_mod_argument
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.VALUE == 4
