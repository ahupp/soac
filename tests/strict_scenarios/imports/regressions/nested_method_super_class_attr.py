# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[nested_method_super_class_attr]
# module:nested_method_super_class_attr
# soac: module(strict_assign=true, checked_attr=true)
class Host:
    def value(self):
        class Inner:
            def method(self):
                return super().__class__

        return Inner().method()
# ok
# nested_method_super_class_attr
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.Host().value() is super
