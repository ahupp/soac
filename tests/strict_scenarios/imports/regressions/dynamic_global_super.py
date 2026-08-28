# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[dynamic_global_super]
# module:dynamic_global_super
# soac: module(strict_assign=true, checked_attr=true)
class MySuper:
    msg = "super super"

class C:
    def method(self):
        return super().msg

def value():
    global super
    previous = super
    super = MySuper
    try:
        return C().method()
    finally:
        super = previous
# ok
# dynamic_global_super
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.value() == "super super"
