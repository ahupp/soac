# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[nested_nonlocal_classcell]
# module:nested_nonlocal_classcell
# soac: module(strict_assign=true, checked_attr=true)
class Outer:
    def value(self):
        class Inner:
            nonlocal __class__
            __class__ = 42

            def cls():
                return __class__

        outer_classcell_value = __class__
        return outer_classcell_value, Inner.cls(), Inner
# ok
# nested_nonlocal_classcell
import sys
import pytest
from soac import _soac_ext, import_hook
outer_value, inner_value, inner_cls = module.Outer().value()
assert outer_value == 42
assert inner_value is inner_cls
