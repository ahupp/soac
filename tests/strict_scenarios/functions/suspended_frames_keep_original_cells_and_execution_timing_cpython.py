# modes:cpython
# Authenticated source and independent ordinary validation blocks.
# module:suspended
# soac: module(strict_assign=true, checked_attr=true)
from support import capture_frames, events, pause

def make_frames():
    captured = 10
    def numbers(delta: int):
        events.append("number-entered")
        yield captured + delta
        events.append("number-resumed")
        yield captured + delta + 1
    return numbers

old_frame, new_frame = capture_frames(make_frames)

def make_async(base):
    async def compute(value: int):
        events.append("coroutine-entered")
        await pause()
        return base + value
    async def stream(value: int):
        events.append("async-generator-entered")
        yield base + value
        yield base + value + 1
    return compute, stream

def four(first: int, second: int, third: int, fourth: int) -> int:
    return first + second + third + fourth
# module:support
import ctypes
events = []

class Pause:
    def __await__(self):
        yield "paused"
        return None

def pause():
    return Pause()

def capture_frames(factory):
    function = factory()
    first = function(1)
    def cell(value):
        return (lambda: value).__closure__[0]
    replace = ctypes.pythonapi.PyFunction_SetClosure
    replace.argtypes = [ctypes.py_object, ctypes.py_object]
    replace.restype = ctypes.c_int
    assert len(function.__closure__) == 1
    assert replace(function, (cell(30),)) == 0
    second = function(2)
    return first, second
# ok
# tests/test_strict_function_boundaries.py::test_strict_suspended_frames_keep_original_cells_and_execution_timing
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_frames', 'make_async', 'four'):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

from soac import _soac_ext
CPYTHON_BACKEND = True

import asyncio
import suspended
from support import events
from soac.strict import StrictRuntimeUnavailableError
assert events == [], events
assert next(suspended.old_frame) == 11
assert next(suspended.new_frame) == 32
assert next(suspended.old_frame) == 12
assert next(suspended.new_frame) == 33
assert next(suspended.old_frame, "done") == "done"
assert next(suspended.new_frame, "done") == "done"
assert events == ["number-entered", "number-entered", "number-resumed", "number-resumed"]

compute, stream = suspended.make_async(100)
coroutine = compute(2)
async_generator = stream(3)
assert len(events) == 4, "suspended bodies ran at object creation"
assert coroutine.send(None) == "paused"
try:
    coroutine.send(None)
except StopIteration as completed:
    assert completed.value == 102
else:
    raise AssertionError("coroutine completion was lost")
async def consume():
    return [value async for value in async_generator]
assert asyncio.run(consume()) == [103, 104]
assert events[-2:] == ["coroutine-entered", "async-generator-entered"]

# The synchronous mandatory subset must not be applied to suspended function
# annotations at object creation, or to the internal resume-control operands.
pending = compute("bad")
assert events[-1] == "async-generator-entered"
pending.close()
# This helper takes a retained resume implementation and preserved-state
# capsule, not a source function's ordinary arguments. Test malformed input
# separately so it cannot stand in for the strict wrong-role barrier.
import ctypes
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p
if CPYTHON_BACKEND:
    assert metadata(suspended.four) is None
else:
    assert metadata(suspended.four) is not None

def rejected_resume(state, expected_error, expected_message):
    before_probe = tuple(events)
    try:
        _soac_ext.resume_generator(suspended.four, object(), state, None, None)
    except expected_error as error:
        assert type(error) is expected_error, (type(error), error)
        assert str(error) == expected_message, str(error)
    else:
        raise AssertionError("public resume control ABI entered a synchronous body")
    assert tuple(events) == before_probe, events

if CPYTHON_BACKEND:
    rejected_resume(object(), RuntimeError, "missing CLIF vectorcall metadata")
else:
    rejected_resume(
        object(), ValueError,
        "PyCapsule_GetPointer called with invalid PyCapsule object")

# Existing public construction creates a valid empty, unmanaged state. It
# contains no source snapshot, generator identity or permission to run a body.
empty_state = _soac_ext.make_preserved_state((), (), [])
if CPYTHON_BACKEND:
    # Native source witnesses and ordinary binders below own this backend's
    # checks; a retained-only API cannot invent a JIT owner for the function.
    rejected_resume(empty_state, RuntimeError, "missing CLIF vectorcall metadata")
    assert metadata(suspended.four) is None
else:
    rejected_resume(
        empty_state, StrictRuntimeUnavailableError,
        "strict resume entry requires an authenticated generator or coroutine body")
print("owned-suspended-frames")

import ctypes
from pathlib import Path
from types import ModuleType, GeneratorType, CoroutineType, AsyncGeneratorType
from tests._strict_integration import _assert_cpython_function_witness

# Compile only the ordinary reference, without source opt-in or inherited
# flags. The strict functions below retain their genuine original native code.
ordinary = ModuleType("ordinary_suspended_binders")
source = Path(suspended.__file__).read_text()
exec(compile(source.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
             "<ordinary-suspended-binders>", "exec", dont_inherit=True),
     vars(ordinary))
ordinary.old_frame.close()
ordinary.new_frame.close()
numbers = suspended.make_frames()
compute, stream = suspended.make_async(100)
plain_compute, plain_stream = ordinary.make_async(100)
pairs = (
    (numbers, ordinary.make_frames(), GeneratorType, "gi_frame", "delta"),
    (compute, plain_compute, CoroutineType, "cr_frame", "value"),
    (stream, plain_stream, AsyncGeneratorType, "ag_frame", "value"),
)
call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object

def python_call(function, args, keywords):
    return function(*args, **keywords)

def close_unstarted(value, kind, frame_name):
    assert type(value) is kind, type(value)
    if kind is AsyncGeneratorType:
        pending_close = value.aclose()
        try:
            pending_close.send(None)
        except StopIteration as stopped:
            assert stopped.value is None
        else:
            raise AssertionError("unstarted async generator close did not complete")
    else:
        assert value.close() is None
    assert getattr(value, frame_name) is None

def binding_error(invoke, function, args, keywords):
    try:
        invoke(function, args, keywords)
    except TypeError as error:
        assert type(error) is TypeError, (type(error), error)
        return error.args
    raise AssertionError("invalid ordinary binding returned a suspended object")

diagnostic = _soac_ext.strict_module_diagnostics(suspended)
before = tuple(events)
for function, control, kind, frame_name, argument in pairs:
    original_code = function.__code__
    _assert_cpython_function_witness(function, diagnostic)
    assert _soac_ext.strict_function_diagnostics(control) is None
    for invoke in (python_call, call):
        for args, keywords in (
            ((), {}), ((1, 2), {}), ((1,), {"unexpected": 2}),
            ((1,), {argument: 2}),
        ):
            expected = binding_error(invoke, control, args, keywords)
            assert binding_error(invoke, function, args, keywords) == expected
            assert tuple(events) == before

        # The annotation is int, but object creation uses ordinary binding,
        # not the synchronous selected-value predicate or body execution.
        for value in (1, "bad"):
            close_unstarted(invoke(control, (value,), {}), kind, frame_name)
            close_unstarted(invoke(function, (value,), {}), kind, frame_name)
            assert tuple(events) == before
    for index in range(128):
        close_unstarted(python_call(function, ("bad",), {}), kind, frame_name)
    assert tuple(events) == before
    assert function.__code__ is original_code
    _assert_cpython_function_witness(function, diagnostic)

assert suspended.four(1, 2, 3, 4) == 10
# Keep the same final source/function/zero-compilation witnesses as cold entry;
# a suspended object's existence alone never supplies strict source authority.

_assert_source_function_witnesses()
