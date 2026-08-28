# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[assigned_dunder_class_method_super]
# module:assigned_dunder_class_method_super
# soac: module(strict_assign=true, checked_attr=true)
class Base:
    def marker(self):
        return "base"

def value():
    class Derived(Base):
        def marker(self):
            return super().marker()

        __class__ = 413

    instance = Derived()
    return instance.marker(), instance.__class__
# ok
# assigned_dunder_class_method_super
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.value() == ("base", 413)
