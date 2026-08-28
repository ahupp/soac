# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[init_subclass_for_loop]
# module:init_subclass_for_loop
# soac: module(strict_assign=true, checked_attr=true)
class Base:
    def __init_subclass__(cls, /, **kwargs):
        cls.SEEN = []
        for item in cls.__mro__:
            cls.SEEN.append(item.__name__)

class Child(Base):
    pass
# ok
# init_subclass_for_loop
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.Child.SEEN[:2] == ["Child", "Base"]
