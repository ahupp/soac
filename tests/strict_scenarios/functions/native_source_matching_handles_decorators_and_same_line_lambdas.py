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

class Decorated:
    @final
    def decorated(self, value: int) -> int:
        return value + 1

decorated = Decorated().decorated
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
# tests/test_strict_function_boundaries.py::test_native_source_matching_handles_decorators_and_same_line_lambdas
import sys
from soac import _soac_ext, import_hook

import ctypes, types
import checked
from soac.strict import StrictRuntimeUnavailableError
assert checked.decorated(2) == 3
assert checked.first_lambda(2) == 2
assert checked.second_lambda(2) == 3
assert checked.first_lambda.__code__ is not checked.second_lambda.__code__
assert checked.first_lambda.__code__.co_firstlineno == checked.second_lambda.__code__.co_firstlineno
source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
source_id.argtypes = [ctypes.py_object]
source_id.restype = ctypes.c_uint64
identities = {source_id(function.__code__) for function in
              [checked.identity, checked.decorated, checked.first_lambda, checked.second_lambda]}
assert len(identities) == 1 and 0 not in identities
clone = types.FunctionType(checked.identity.__code__, checked.identity.__globals__)
try:
    clone(1)
except StrictRuntimeUnavailableError:
    pass
else:
    raise AssertionError("a code object alone became strict entry authority")
print("authenticated-native-source-tree")
