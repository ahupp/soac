# modes:soac,entry
# test_strict_import_admission.py::test_importlib_reload_cannot_reexecute_or_unseal_a_strict_module
# module:reload_helper
# soac: module(strict_assign=true, checked_attr=true)
import reload_audit
reload_audit.events.append("executed")
VALUE = 1
def read():
    return VALUE
# module:reload_audit
events = []
# ok
# test_importlib_reload_cannot_reexecute_or_unseal_a_strict_module
import sys
import pytest
from soac import _soac_ext, import_hook
import importlib
import reload_audit, reload_helper
from soac.strict import StrictMutationError
original = reload_helper
function = original.read
spec = original.__spec__
assert reload_audit.events == ["executed"]
try:
    importlib.reload(original)
except StrictMutationError:
    # importlib first replaces __spec__. This final binding rejects
    # reload before the loader can attempt a second body execution.
    pass
else:
    raise AssertionError("sealed module reloaded")
assert sys.modules["reload_helper"] is original
assert original.__spec__ is spec
assert reload_audit.events == ["executed"]
assert original.read is function and function() == 1
assert _soac_ext.strict_module_diagnostics(original)["sealed"] is True
