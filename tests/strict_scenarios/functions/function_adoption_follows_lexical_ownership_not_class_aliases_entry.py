# modes:entry
# Authenticated source and independent ordinary validation blocks.
# module:lexical_functions
# soac: module(strict_assign=true, checked_attr=true)
from lexical_function_support import DynamicMeta, remember, replacement

def standalone(value: int = 1) -> int:
    return value

class Dynamic(metaclass=DynamicMeta):
    borrowed = standalone

    def overwritten(self):
        return "original"

    preserved = remember(overwritten)
    overwritten = replacement()

    def factory(self):
        def nested(value: int = 3) -> int:
            return value
        return nested
# module:lexical_function_support
from typing import Any

class DynamicMeta(type):
    pass

def remember(function: Any) -> Any:
    return function

def changed(self):
    return "changed"

def replacement() -> Any:
    return changed
# ok
# tests/test_strict_function_boundaries.py::test_function_adoption_follows_lexical_ownership_not_class_aliases
import sys
from soac import _soac_ext, import_hook

import ctypes
import lexical_functions as module
from lexical_function_support import changed
from soac.strict import StrictMutationError

get_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
get_identity.argtypes = [ctypes.py_object]
get_identity.restype = ctypes.c_uint64

assert module.Dynamic.borrowed is module.standalone
assert get_identity(module.standalone) != 0
assert module.standalone(4) == 4
assert module.standalone("bad") == "bad"

# Overwriting the final class member does not make the old lexical method a
# free function. Its statically dynamic framework keeps ordinary mutability.
preserved = module.Dynamic.preserved
assert get_identity(preserved) == 0
preserved.__code__ = changed.__code__
assert preserved(None) == "changed"
assert get_identity(module.Dynamic.factory) == 0

# A definition inside a method has that function as its immediate scope, not
# the enclosing class. Its late free-function completion still applies.
nested = module.Dynamic().factory()
assert get_identity(nested) != 0
assert nested() == 3
try:
    nested.__defaults__ = (5,)
except StrictMutationError:
    pass
else:
    raise AssertionError("an enclosing dynamic class captured a nested free definition")
print("lexical-function-ownership")
