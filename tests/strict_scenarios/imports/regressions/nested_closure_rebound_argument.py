# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[nested_closure_rebound_argument]
# module:nested_closure_rebound_argument
# soac: module(strict_assign=true, checked_attr=true)
def f(value=None):
    value = "updated"

    def inner():
        return value

    return value, inner()
# ok
# nested_closure_rebound_argument
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.f() == ("updated", "updated")
