# modes:soac,entry
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
# tests/test_strict_function_boundaries.py::test_public_vectorcall_preserves_owned_continuation [same-pointer]
import sys
from soac import _soac_ext, import_hook
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
mutation = 'same-pointer'

import ctypes
import pytest
import checked
from soac import _soac_ext

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = arguments
    function.restype = result
    return function

obj = ctypes.py_object
owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
set_vectorcall = api("PyFunction_SetVectorcall", None, obj, ctypes.c_void_p)
incref = api("Py_IncRef", None, obj)
vectorcall = api(
    "PyObject_Vectorcall", obj, obj, ctypes.POINTER(obj),
    ctypes.c_size_t, ctypes.c_void_p,
)

def ordinary(value):
    return value

function = checked.identity
assert _soac_ext.strict_module_diagnostics(checked)["sealed"]
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
assert function("wrong") == "wrong"
assert ordinary("wrong") == "wrong"

code, globals_, source_owner = function.__code__, function.__globals__, owner(function)
original_pointer = get_vectorcall(function)
assert original_pointer
signature = ctypes.PYFUNCTYPE(
    ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
    ctypes.c_size_t, ctypes.c_void_p,
)
original = signature(original_pointer)
argument_count_mask = (1 << (8 * ctypes.sizeof(ctypes.c_size_t) - 1)) - 1
calls, callback_errors = [], []
safe_failure_result = object()

@signature
def forward(actual, arguments, nargsf, kwnames):
    # Forward the saved checked ABI, not stock bytecode or public vectorcall
    # again. Record failures outside ctypes so it cannot swallow an exception
    # and return an undefined pointer; successful results transfer unchanged.
    try:
        calls.append((actual, nargsf & argument_count_mask, kwnames))
        result = original(actual, arguments, nargsf, kwnames)
        if result:
            return result
        callback_errors.append("saved entry returned NULL without an exception")
    except BaseException as error:
        callback_errors.append((type(error).__name__, str(error)))
    incref(safe_failure_result)
    return id(safe_failure_result)

wrapper_pointer = ctypes.cast(forward, ctypes.c_void_p).value

def python_caller(value):
    return function(value)

def c_caller(value):
    arguments = (obj * 1)(value)
    return vectorcall(function, arguments, 1, None)

try:
    if mutation == "same-pointer":
        set_vectorcall(function, original_pointer)
    else:
        set_vectorcall(function, wrapper_pointer)
        if mutation == "restored":
            set_vectorcall(function, original_pointer)
    # Real public entry replacement preserves source ownership and the saved
    # continuation's ordinary callable semantics.
    assert owner(function) == source_owner
    for invoke in (python_caller, c_caller):
        for value in range(32):
            result = invoke(value)
            assert not callback_errors, callback_errors
            assert result == value
    expected_calls = [(id(function), 1, None)] * 64 if mutation == "forwarder" else []
    assert calls == expected_calls, calls
    if mutation == "forwarder":
        assert get_vectorcall(function) == wrapper_pointer
        assert _soac_ext.strict_function_entry_kind(function) == "public_override"
    else:
        assert function("wrong") == "wrong"
finally:
    # Keep the ctypes callback alive until the real public entry is restored,
    # including assertion failures.
    set_vectorcall(function, original_pointer)

assert not callback_errors, callback_errors
assert function(73) == 73
assert function("wrong") == "wrong"
assert get_vectorcall(function) == original_pointer
assert function.__code__ is code and function.__globals__ is globals_
assert owner(function) == source_owner
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
# ok
# tests/test_strict_function_boundaries.py::test_public_vectorcall_preserves_owned_continuation [forwarder]
import sys
from soac import _soac_ext, import_hook
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
mutation = 'forwarder'

import ctypes
import pytest
import checked
from soac import _soac_ext

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = arguments
    function.restype = result
    return function

obj = ctypes.py_object
owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
set_vectorcall = api("PyFunction_SetVectorcall", None, obj, ctypes.c_void_p)
incref = api("Py_IncRef", None, obj)
vectorcall = api(
    "PyObject_Vectorcall", obj, obj, ctypes.POINTER(obj),
    ctypes.c_size_t, ctypes.c_void_p,
)

def ordinary(value):
    return value

function = checked.identity
assert _soac_ext.strict_module_diagnostics(checked)["sealed"]
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
assert function("wrong") == "wrong"
assert ordinary("wrong") == "wrong"

code, globals_, source_owner = function.__code__, function.__globals__, owner(function)
original_pointer = get_vectorcall(function)
assert original_pointer
signature = ctypes.PYFUNCTYPE(
    ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
    ctypes.c_size_t, ctypes.c_void_p,
)
original = signature(original_pointer)
argument_count_mask = (1 << (8 * ctypes.sizeof(ctypes.c_size_t) - 1)) - 1
calls, callback_errors = [], []
safe_failure_result = object()

@signature
def forward(actual, arguments, nargsf, kwnames):
    # Forward the saved checked ABI, not stock bytecode or public vectorcall
    # again. Record failures outside ctypes so it cannot swallow an exception
    # and return an undefined pointer; successful results transfer unchanged.
    try:
        calls.append((actual, nargsf & argument_count_mask, kwnames))
        result = original(actual, arguments, nargsf, kwnames)
        if result:
            return result
        callback_errors.append("saved entry returned NULL without an exception")
    except BaseException as error:
        callback_errors.append((type(error).__name__, str(error)))
    incref(safe_failure_result)
    return id(safe_failure_result)

wrapper_pointer = ctypes.cast(forward, ctypes.c_void_p).value

def python_caller(value):
    return function(value)

def c_caller(value):
    arguments = (obj * 1)(value)
    return vectorcall(function, arguments, 1, None)

try:
    if mutation == "same-pointer":
        set_vectorcall(function, original_pointer)
    else:
        set_vectorcall(function, wrapper_pointer)
        if mutation == "restored":
            set_vectorcall(function, original_pointer)
    # Real public entry replacement preserves source ownership and the saved
    # continuation's ordinary callable semantics.
    assert owner(function) == source_owner
    for invoke in (python_caller, c_caller):
        for value in range(32):
            result = invoke(value)
            assert not callback_errors, callback_errors
            assert result == value
    expected_calls = [(id(function), 1, None)] * 64 if mutation == "forwarder" else []
    assert calls == expected_calls, calls
    if mutation == "forwarder":
        assert get_vectorcall(function) == wrapper_pointer
        assert _soac_ext.strict_function_entry_kind(function) == "public_override"
    else:
        assert function("wrong") == "wrong"
finally:
    # Keep the ctypes callback alive until the real public entry is restored,
    # including assertion failures.
    set_vectorcall(function, original_pointer)

assert not callback_errors, callback_errors
assert function(73) == 73
assert function("wrong") == "wrong"
assert get_vectorcall(function) == original_pointer
assert function.__code__ is code and function.__globals__ is globals_
assert owner(function) == source_owner
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
# ok
# tests/test_strict_function_boundaries.py::test_public_vectorcall_preserves_owned_continuation [restored]
import sys
from soac import _soac_ext, import_hook
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
mutation = 'restored'

import ctypes
import pytest
import checked
from soac import _soac_ext

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = arguments
    function.restype = result
    return function

obj = ctypes.py_object
owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
set_vectorcall = api("PyFunction_SetVectorcall", None, obj, ctypes.c_void_p)
incref = api("Py_IncRef", None, obj)
vectorcall = api(
    "PyObject_Vectorcall", obj, obj, ctypes.POINTER(obj),
    ctypes.c_size_t, ctypes.c_void_p,
)

def ordinary(value):
    return value

function = checked.identity
assert _soac_ext.strict_module_diagnostics(checked)["sealed"]
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
assert function("wrong") == "wrong"
assert ordinary("wrong") == "wrong"

code, globals_, source_owner = function.__code__, function.__globals__, owner(function)
original_pointer = get_vectorcall(function)
assert original_pointer
signature = ctypes.PYFUNCTYPE(
    ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
    ctypes.c_size_t, ctypes.c_void_p,
)
original = signature(original_pointer)
argument_count_mask = (1 << (8 * ctypes.sizeof(ctypes.c_size_t) - 1)) - 1
calls, callback_errors = [], []
safe_failure_result = object()

@signature
def forward(actual, arguments, nargsf, kwnames):
    # Forward the saved checked ABI, not stock bytecode or public vectorcall
    # again. Record failures outside ctypes so it cannot swallow an exception
    # and return an undefined pointer; successful results transfer unchanged.
    try:
        calls.append((actual, nargsf & argument_count_mask, kwnames))
        result = original(actual, arguments, nargsf, kwnames)
        if result:
            return result
        callback_errors.append("saved entry returned NULL without an exception")
    except BaseException as error:
        callback_errors.append((type(error).__name__, str(error)))
    incref(safe_failure_result)
    return id(safe_failure_result)

wrapper_pointer = ctypes.cast(forward, ctypes.c_void_p).value

def python_caller(value):
    return function(value)

def c_caller(value):
    arguments = (obj * 1)(value)
    return vectorcall(function, arguments, 1, None)

try:
    if mutation == "same-pointer":
        set_vectorcall(function, original_pointer)
    else:
        set_vectorcall(function, wrapper_pointer)
        if mutation == "restored":
            set_vectorcall(function, original_pointer)
    # Real public entry replacement preserves source ownership and the saved
    # continuation's ordinary callable semantics.
    assert owner(function) == source_owner
    for invoke in (python_caller, c_caller):
        for value in range(32):
            result = invoke(value)
            assert not callback_errors, callback_errors
            assert result == value
    expected_calls = [(id(function), 1, None)] * 64 if mutation == "forwarder" else []
    assert calls == expected_calls, calls
    if mutation == "forwarder":
        assert get_vectorcall(function) == wrapper_pointer
        assert _soac_ext.strict_function_entry_kind(function) == "public_override"
    else:
        assert function("wrong") == "wrong"
finally:
    # Keep the ctypes callback alive until the real public entry is restored,
    # including assertion failures.
    set_vectorcall(function, original_pointer)

assert not callback_errors, callback_errors
assert function(73) == 73
assert function("wrong") == "wrong"
assert get_vectorcall(function) == original_pointer
assert function.__code__ is code and function.__globals__ is globals_
assert owner(function) == source_owner
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
