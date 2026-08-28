# modes:soac
# Authenticated source and independent ordinary validation blocks.
# module:checked
# soac: module(strict_assign=true, checked_attr=true)
from typing import Any, cast, final
from support import events, marker, observe

def identity(value: int) -> int:
    return value

first_lambda, second_lambda = (lambda value: value), (lambda value: value + 1)

def widened(value: float) -> float:
    return value

def optional(value: int | str | None) -> int | str | None:
    return value

def shape(first: int, /, second: int = 2, *items: int,
          named: str | None = None, **extras: int) -> int:
    return first + second

def caller(value: Any) -> int:
    return identity(value)

def bad_return(value: Any) -> int:
    return cast(int, value)

def finish_with_result(factory, observer, result: Any) -> int:
    payload = factory()
    try:
        raise LookupError("source handler")
    except LookupError:
        observer("body")
        return cast(int, result)

def raises(value: int) -> int:
    raise LookupError("body wins")

def annotation_trap(format: int):
    events.append("annotation evaluated")
    raise AssertionError("annotation provider must never be called by a boundary")

identity.__annotate__ = annotation_trap

def active_default(value=marker("active-old")) -> None:
    active_default.__defaults__ = (marker("active-new"),)
    observe(value)

active_default()
events.append("after-active")

def idle_default(value=marker("idle-old")):
    return value

idle_default.__defaults__ = (marker("idle-new"),)
events.append("after-idle")

def make_cycle():
    captured = []
    def inner(value: int) -> int:
        return value + len(captured)
    captured.append(inner)
    return inner

class StoppingIterator:
    def __next__(self):
        raise StopIteration

def catch_stop(iterator, observer):
    try:
        return next(iterator)
    except StopIteration:
        return observer()

class ReturningIterator:
    def __next__(self):
        try:
            raise LookupError("callee handler")
        except LookupError:
            return 7

def replace_result(iterator, create):
    value = create()
    value = next(iterator)
    return value
# module:support
events = []

class Marker:
    def __init__(self, name):
        self.name = name
    def __del__(self):
        events.append("drop:" + self.name)

def marker(name):
    return Marker(name)

def observe(value):
    events.append("use:" + value.name)
# ok
# tests/test_strict_function_boundaries.py::test_owner_is_opaque_cycles_collect_and_public_ids_cannot_forge_functions
import sys
from soac import _soac_ext, import_hook

import ctypes, gc, weakref
import checked
from soac.strict import StrictRuntimeUnavailableError

owners = [value for value in gc.get_referents(checked.identity)
          if type(value).__name__ == "_StrictFunctionOwner"]
assert len(owners) == 1
for operation in [lambda: type(owners[0])(),
                  lambda: setattr(type(owners[0]), "mutable", True),
                  lambda: setattr(owners[0], "mutable", True)]:
    try:
        operation()
    except (TypeError, AttributeError):
        pass
    else:
        raise AssertionError("opaque source authority was mutable")

inner = checked.make_cycle()
assert inner(3) == 4
reference = weakref.ref(inner)
del inner
gc.collect()
assert reference() is None, "hidden environment edges retained a closure cycle"

# Trusted native test introspection, not a production authority path:
# an ID readable from implementation metadata must not be a capability.
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p
class Prefix(ctypes.Structure):
    _fields_ = [("environment", ctypes.c_void_p), ("function_id", ctypes.c_uint64)]
pointer = metadata(checked.identity)
assert pointer
identifier = Prefix.from_address(pointer).function_id
try:
    _soac_ext.make_function(identifier, "function", (), (), module_globals=checked.__dict__)
except StrictRuntimeUnavailableError as error:
    assert "Python-supplied function IDs" in str(error), str(error)
else:
    raise AssertionError("a public integer forged strict source provenance")
print("opaque-owner-and-cycle")
