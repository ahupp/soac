# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[try_finally_keeps_handled_exception]
# module:try_finally_keeps_handled_exception
# soac: module(strict_assign=true, checked_attr=true)
import sys

def inner():
    try:
        pass
    finally:
        marker = 1
    return marker

def value():
    try:
        1 / 0
    except Exception:
        before = type(sys.exception()).__name__
        inner()
        after = sys.exception()
        return before, type(after).__name__ if after is not None else None
# ok
# try_finally_keeps_handled_exception
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.value() == ("ZeroDivisionError", "ZeroDivisionError")
