# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[nested_class_body_dunder_class]
# module:nested_class_body_dunder_class
# soac: module(strict_assign=true, checked_attr=true)
class Host:
    def value(self):
        class Inner:
            outer = __class__

            def cls():
                return __class__

        return Inner.outer, Inner.cls(), Inner
# ok
# nested_class_body_dunder_class
import sys
import pytest
from soac import _soac_ext, import_hook
outer_value, inner_value, inner_cls = module.Host().value()
assert outer_value is module.Host
assert inner_value is inner_cls
