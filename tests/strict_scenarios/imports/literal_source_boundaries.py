# modes:soac,entry
# test_strict_import_admission.py::test_strict_source_literal_controls_and_ordinary_surrogates_remain_distinct
# module:literal_controls
# soac: module(strict_assign=true, checked_attr=true)
from typing import Literal
import ordinary_literals

def replacement(value: Literal["�"]) -> Literal["\ufffd"]:
    return value

def backslash(value: Literal[r"\ud800"]) -> Literal[r"\ud800"]:
    return value

def controls(value):
    return (
        "�", "\ufffd", r"\ud800", "\\ud800",
        rf"\ud800{value}", rt"\ud800{value}".strings,
        f"\\ud800{value}", t"\\ud800{value}".strings,
    )

def ordinary_values():
    return ordinary_literals.values()
# module:ordinary_literals
def values():
    return "\ud800", "\ud83d\udc0d", "\U0000DFFF"
# ok
# test_strict_source_literal_controls_and_ordinary_surrogates_remain_distinct
import sys
import pytest
from soac import _soac_ext, import_hook
import ctypes
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p
strict_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
strict_owner.argtypes = [ctypes.py_object]
strict_owner.restype = ctypes.c_void_p
expected_entry = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'

import literal_controls as checked
import ordinary_literals as ordinary
for name in ("replacement", "backslash", "controls", "ordinary_values"):
    function = vars(checked)[name]
    assert metadata(function) is not None
    assert strict_owner(function) is not None
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry
assert _soac_ext.strict_module_diagnostics(checked)["sealed"] is True
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert metadata(ordinary.values) is None
assert strict_owner(ordinary.values) is None
assert checked.replacement("�") == "�"
assert checked.backslash(r"\ud800") == r"\ud800"
# Literal annotations are outside the shared mandatory-check subset. They
# must not coerce a runtime surrogate to the source replacement character.
for function in (checked.replacement, checked.backslash):
    surrogate = chr(0xD800)
    assert function(surrogate) is surrogate
assert checked.controls("X") == (
    "�", "�", r"\ud800", r"\ud800", r"\ud800X", (r"\ud800", ""),
    r"\ud800X", (r"\ud800", ""),
)
expected = ([0xD800], [0xD83D, 0xDC0D], [0xDFFF])
assert tuple(list(map(ord, value)) for value in ordinary.values()) == expected
assert tuple(list(map(ord, value)) for value in checked.ordinary_values()) == expected
