# modes:soac,entry
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
# tests/test_strict_function_boundaries.py::test_actual_public_binder_preserves_annotation_only_values_and_errors
import sys
from soac import _soac_ext, import_hook

import checked
from support import events

def rejected(call, contains=None):
    try:
        call()
    except TypeError as error:
        if contains is not None:
            assert contains in str(error), str(error)
    else:
        raise AssertionError("ordinary argument binding accepted an invalid call")

assert checked.identity(True) is True
value = int("12345678901234567890")
assert checked.identity(value) is value
assert checked.widened(value) is value
assert checked.optional(None) is None
assert checked.optional("ok") == "ok"
assert checked.shape(3, 4, 5, 6, named=None, extra=7) == 7
assert checked.shape(3) == 5
assert checked.identity("bad") == "bad"
outside = []
assert checked.optional(outside) is outside
assert checked.shape(1, 2, "bad") == 3
assert checked.shape(1, extra="bad") == 3
rejected(lambda: checked.shape("bad", 2, second=3), "multiple values")
rejected(lambda: checked.identity("bad", unexpected=1), "unexpected keyword")
rejected(lambda: checked.identity(), "missing")
assert checked.bad_return(outside) is outside
for number in range(30):
    assert checked.caller(number) == number
assert checked.caller("bad") == "bad"
try:
    checked.raises("bad")
except LookupError as error:
    assert str(error) == "body wins"
else:
    raise AssertionError("body exception was replaced or lost")
assert "annotation evaluated" not in events
print("ordinary-binders-and-annotation-only-values")
# ok
# tests/test_strict_function_boundaries.py::test_return_identity_and_body_errors_preserve_source_and_cleanup
import sys
from soac import _soac_ext, import_hook

import gc, sys, weakref
import checked
from soac import _soac_ext

assert _soac_ext.strict_function_entry_kind(checked.finish_with_result) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
events = []
references = []
class Payload:
    def __del__(self):
        events.append("drop")
def create():
    payload = Payload()
    references.append(weakref.ref(payload))
    return payload

outer = ValueError("caller handler")
try:
    raise outer
except ValueError:
    assert checked.finish_with_result(create, events.append, 7) == 7
    gc.collect()
    assert events.count("body") == events.count("drop") == 1, events
    assert references[-1]() is None
    events.clear()

    result = object()
    assert checked.finish_with_result(create, events.append, result) is result
    gc.collect()
    assert references[-1]() is None
    assert events.count("body") == events.count("drop") == 1, events
    events.clear()

    original = RuntimeError("explicit observer failure")
    def fail_observer(stage):
        events.append(stage)
        raise original
    try:
        checked.finish_with_result(create, fail_observer, result)
    except RuntimeError as error:
        assert error is original
        assert isinstance(error.__context__, LookupError)
        assert str(error.__context__) == "source handler"
        error.__context__.__traceback__ = None
        error.__context__ = None
        error.__traceback__ = None
        gc.collect()
        assert references[-1]() is None
        assert events.count("body") == events.count("drop") == 1, events
    else:
        raise AssertionError("explicit observer exception was lost")
    assert sys.exception() is outer
assert sys.exception() is None
print("return-identity-body-error-and-cleanup")
# ok
# tests/test_strict_function_boundaries.py::test_unsealed_default_replacement_keeps_only_active_values_alive
import sys
from soac import _soac_ext, import_hook

import gc
import checked
from support import events
gc.collect()
assert [event for event in events if not event.startswith("drop:")] == [
    "use:active-old", "after-active", "after-idle",
], events
assert sorted(event for event in events if event.startswith("drop:")) == [
    "drop:active-old", "drop:idle-old",
], events
assert checked.active_default.__defaults__[0].name == "active-new"
assert checked.idle_default().name == "idle-new"
print("default-replacement-and-cleanup")
# ok
# tests/test_strict_function_boundaries.py::test_untraced_source_callbacks_preserve_values_and_cleanup
import sys
from soac import _soac_ext, import_hook

expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

import gc
import sys
import weakref
import checked
from soac import _soac_ext

assert _soac_ext.strict_function_entry_kind(checked.replace_result) == expected_entry
calls, released, references = [], [], []

class Iterator:
    def __init__(self):
        references.append(weakref.ref(self))
    def __next__(self):
        calls.append("next")
        return 41
    def __del__(self):
        released.append("iterator")

def factory():
    calls.append("factory")
    return object()

# Source-defined callbacks and cleanup do not depend on observer coverage.
assert checked.replace_result(Iterator(), factory) == 41
gc.collect()
assert calls == ["factory", "next"], calls
assert released == ["iterator"], released
assert all(reference() is None for reference in references)
assert _soac_ext.strict_function_entry_kind(checked.replace_result) == expected_entry
assert checked.identity(13) == 13
assert checked.identity("bad") == "bad"

# Ordinary CPython functions still receive their actual trace events.
observed = []
def ordinary(value):
    return value
def ordinary_trace(frame, event, arg):
    if frame.f_code is ordinary.__code__:
        observed.append(event)
    return ordinary_trace
sys.settrace(ordinary_trace)
try:
    value = object()
    assert ordinary(value) is value
finally:
    sys.settrace(None)
assert observed[0] == "call" and observed[-1] == "return", observed
