# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:late_functions
# soac: module(strict_assign=true, checked_attr=true)
from late_function_support import make_default, reuse_previous

def factory():
    class Token:
        pass
    Alias = Token
    def accept(value: Alias = Token()) -> Alias:
        return value
    return Token, accept

def decorated_factory():
    @reuse_previous
    def candidate(value=make_default()):
        return value
    return candidate
# module:late_function_support
from typing import Any

events = []
previous = None
sequence = 0

class Marker:
    def __init__(self, index: int):
        self.index = index

    def __del__(self):
        events.append(f"release:{self.index}")

def make_default() -> Any:
    global sequence
    sequence += 1
    events.append(f"create:{sequence}")
    return Marker(sequence)

def reuse_previous(function: Any) -> Any:
    global previous
    if previous is None:
        events.append("keep")
        previous = function
    else:
        events.append("reuse")
    return previous
# ok
# tests/test_strict_function_boundaries.py::test_late_free_function_completion_freezes_defaults_not_annotated_value_types
import sys
from soac import _soac_ext, import_hook

import ctypes
import late_functions as module
from soac.strict import StrictMutationError

get_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
get_identity.argtypes = [ctypes.py_object]
get_identity.restype = ctypes.c_uint64

first, function = module.factory()
second, _ = module.factory()
assert get_identity(function) != 0, "late definition escaped its completion boundary"
defaults = function.__defaults__
assert type(defaults[0]) is first
assert function() is defaults[0]

try:
    function.__defaults__ = (second(),)
except StrictMutationError:
    pass
else:
    raise AssertionError("completed free function still has replaceable defaults")
assert function.__defaults__ is defaults

provider = function.__annotate__
cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
cells["Alias"].cell_contents = second
assert function() is defaults[0]
value = first()
assert function(value) is value
other = second()
assert function(other) is other
print("late-function-completion")
# ok
# tests/test_strict_function_boundaries.py::test_late_free_function_decorator_cannot_adopt_a_previous_execution
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import late_functions as module
from late_function_support import events

get_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
get_identity.argtypes = [ctypes.py_object]
get_identity.restype = ctypes.c_uint64

first = module.decorated_factory()
assert events == ["create:1", "keep"]
second = module.decorated_factory()
assert second is first
gc.collect()
assert [event for event in events if not event.startswith("release:")] == [
    "create:1", "keep", "create:2", "reuse",
]
assert [event for event in events if event.startswith("release:")] == ["release:2"]
assert get_identity(first) == 0, "source equality authorized foreign decorator output"

# Neither a completion ticket nor idle metadata may retain discarded default
# objects, or freeze a function whose arbitrary decorator remains dynamic.
first.__defaults__ = ("replacement",)
gc.collect()
assert sorted(event for event in events if event.startswith("release:")) == [
    "release:1", "release:2",
]
assert first() == "replacement"
print("late-decorator-execution-isolation")
