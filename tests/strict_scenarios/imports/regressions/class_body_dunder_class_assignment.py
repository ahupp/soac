# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[class_body_dunder_class_assignment]
# module:class_body_dunder_class_assignment
# soac: module(strict_assign=true, checked_attr=true)
class Base:
    def marker(self):
        return "base"

class Host:
    def value(self):
        class First(Base):
            def marker(self):
                return super().marker()

            __class__ = 413

        class Second:
            outer = __class__

            def cls():
                return __class__

        return First().marker(), First().__class__, Second.outer, Second.cls(), Second
# ok
# class_body_dunder_class_assignment
import sys
import pytest
from soac import _soac_ext, import_hook
first_marker, first_class_attr, second_outer, second_value, second_cls = (
    module.Host().value()
)
assert first_marker == "base"
assert first_class_attr == 413
assert second_outer is module.Host
assert second_value is second_cls
