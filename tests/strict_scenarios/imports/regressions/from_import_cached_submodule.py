# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[from_import_cached_submodule]
# module:from_import_cached_submodule
# soac: module(strict_assign=true, checked_attr=true)
import from_import_cached_pkg.child as child
import from_import_cached_pkg as parent
del parent.child
from from_import_cached_pkg import child as imported
VALUE = (imported is child, hasattr(parent, "child"))
# module:from_import_cached_pkg
# module:from_import_cached_pkg.child
VALUE = 42
# ok
# from_import_cached_submodule
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.VALUE == (True, False)
