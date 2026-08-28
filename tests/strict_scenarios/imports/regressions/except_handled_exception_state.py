# modes:soac,entry,cpython
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[except_handled_exception_state]
# module:except_handled_exception_state
# soac: module(strict_assign=true, checked_attr=true)
import sys
import traceback

def capture(flag):
    try:
        raise ValueError("boom")
    except:
        if flag:
            text = traceback.format_exc()
            active_type = type(sys.exception()).__name__
        else:
            text = "missing"
            active_type = type(sys.exception()).__name__
    return "ValueError: boom" in text, active_type, sys.exception()
# ok
# except_handled_exception_state
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.capture(True) == (True, "ValueError", None)
assert module.capture(False) == (False, 'ValueError', None)
assert sys.exception() is None
