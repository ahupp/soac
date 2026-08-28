# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:dir_context
# soac: module(strict_assign=true, checked_attr=true)
from builtins import dir as aliased_dir

MODULE_MARKER = object()
module_names = aliased_dir()

class Namespace:
    CLASS_MARKER = object()
    names = aliased_dir()

def function_names():
    return aliased_dir()
# ok
# tests/test_strict_call_context.py::test_dir_alias_uses_actual_module_and_class_namespaces
import sys
from soac import _soac_ext, import_hook

import dir_context as module

assert _soac_ext.strict_module_diagnostics(module)["sealed"] is True
assert _soac_ext.strict_function_entry_kind(module.function_names) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
assert "MODULE_MARKER" in module.module_names
assert "CLASS_MARKER" not in module.module_names
assert module.module_names == sorted(module.module_names)
assert "CLASS_MARKER" in module.Namespace.names
assert "MODULE_MARKER" not in module.Namespace.names
assert module.Namespace.names == sorted(module.Namespace.names)
# Function-local inspection is excluded. Its definition still must not
# block admission of the actual module/class namespace operations above.
