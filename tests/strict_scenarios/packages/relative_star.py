# modes:soac,entry
# test_strict_import_admission.py::test_authenticated_package_relative_star_keeps_ordinary_child_binding
# module:relative_star_pkg
# soac: module(strict_assign=true, checked_attr=true)
from .child import *
VALUE = child.MARKER
# module:relative_star_pkg.child
__all__ = ["EXPORTED"]
MARKER = "child"
EXPORTED = 3
# ok
# test_authenticated_package_relative_star_keeps_ordinary_child_binding
import sys
import pytest
from soac import _soac_ext, import_hook
import relative_star_pkg as module
assert module.VALUE == "child"
assert module.EXPORTED == 3
assert _soac_ext.strict_module_diagnostics(module)["sealed"] is True
assert _soac_ext.strict_module_diagnostics(module.child) is None
assert not isinstance(module.child.__spec__.loader, import_hook.SoacLoader)
