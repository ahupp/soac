# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[kwonly_default]
# module:kwonly_default
# soac: module(strict_assign=true, checked_attr=true)
def value(*, marker=3):
    return marker
# ok
# kwonly_default
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.value() == 3
