# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[method_local_class_base]
# module:method_local_class_base
# soac: module(strict_assign=true, checked_attr=true)
class Container:
    def method(self):
        class RawBase:
            pass

        class Derived(RawBase):
            pass

        return Derived.__mro__[1] is RawBase

VALUE = Container().method()
# ok
# method_local_class_base
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.VALUE is True
