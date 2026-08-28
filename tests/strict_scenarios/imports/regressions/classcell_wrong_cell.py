# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[classcell_wrong_cell]
# module:classcell_wrong_cell
# soac: module(strict_assign=true, checked_attr=true)
def value():
    class Meta(type):
        def __new__(cls, name, bases, namespace):
            cls = super().__new__(cls, name, bases, namespace)
            type("Other", (), namespace)
            return cls

    class WithClassRef(metaclass=Meta):
        def f(self):
            return __class__
# ok
# classcell_wrong_cell
import sys
import pytest
from soac import _soac_ext, import_hook
with pytest.raises(TypeError):
    module.value()
