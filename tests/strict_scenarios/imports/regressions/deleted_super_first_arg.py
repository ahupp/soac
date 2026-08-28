# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[deleted_super_first_arg]
# module:deleted_super_first_arg
# soac: module(strict_assign=true, checked_attr=true)
class Host:
    def value(self):
        def nested(x):
            del x
            super()

        nested(self)
# ok
# deleted_super_first_arg
import sys
import pytest
from soac import _soac_ext, import_hook
with pytest.raises(RuntimeError, match=r"arg\[0\] deleted"):
    module.Host().value()
