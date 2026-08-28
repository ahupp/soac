# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[classcell_missing]
# module:classcell_missing
# soac: module(strict_assign=true, checked_attr=true)
def value():
    class Meta(type):
        def __new__(cls, name, bases, namespace):
            namespace.pop("__classcell__", None)
            return super().__new__(cls, name, bases, namespace)

    class WithClassRef(metaclass=Meta):
        def f(self):
            return __class__
# ok
# classcell_missing
import sys
import pytest
from soac import _soac_ext, import_hook
with pytest.raises(RuntimeError, match="__class__ not set.*__classcell__ propagated"):
    module.value()
