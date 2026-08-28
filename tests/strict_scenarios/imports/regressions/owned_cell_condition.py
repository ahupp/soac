# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[owned_cell_condition]
# module:owned_cell_condition
# soac: module(strict_assign=true, checked_attr=true)
def outer(reason):
    def decorator(test_item):
        return reason

    if isinstance(reason, int):
        return decorator(reason)
    return decorator

VALUE = outer("why")(object)
# ok
# owned_cell_condition
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.VALUE == "why"
