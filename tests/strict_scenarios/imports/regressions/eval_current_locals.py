# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[eval_current_locals]
# module:eval_current_locals
# soac: module(strict_assign=true, checked_attr=true)
def value():
    left = 3
    right = 4
    return eval("left + right")
# ok
# eval_current_locals
import sys
import pytest
from soac import _soac_ext, import_hook
with pytest.raises(NotImplementedError, match="requires explicit globals"):
    module.value()
