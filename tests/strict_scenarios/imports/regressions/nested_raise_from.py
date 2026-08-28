# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[nested_raise_from]
# module:nested_raise_from
# soac: module(strict_assign=true, checked_attr=true)
def wrap(should_wrap):
    if should_wrap:
        try:
            raise ValueError("inner")
        except ValueError as exc:
            raise RuntimeError("outer") from exc

try:
    wrap(True)
except RuntimeError as exc:
    WRAPPED = str(exc)
# ok
# nested_raise_from
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.WRAPPED == "outer"
