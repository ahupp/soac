# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[eval_for_loop_target_local]
# module:eval_for_loop_target_local
# soac: module(strict_assign=true, checked_attr=true)
def value():
    for item in [12]:
        bad_format_spec = "%M"
        try:
            eval("f'xx{item:{bad_format_spec}}yy'")
        except ValueError as exc:
            return "Invalid format specifier" in str(exc)
    return False
# ok
# eval_for_loop_target_local
import sys
import pytest
from soac import _soac_ext, import_hook
with pytest.raises(NotImplementedError, match="requires explicit globals"):
    module.value()
