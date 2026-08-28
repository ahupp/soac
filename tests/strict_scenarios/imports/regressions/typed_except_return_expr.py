# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[typed_except_return_expr]
# module:typed_except_return_expr
# soac: module(strict_assign=true, checked_attr=true)
def f():
    try:
        return {}["x"]
    except KeyError:
        return "ok"
# ok
# typed_except_return_expr
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.f() == "ok"
