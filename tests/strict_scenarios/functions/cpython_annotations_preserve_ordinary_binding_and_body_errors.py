# modes:cpython
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
# tests/test_strict_function_boundaries.py::test_cpython_annotations_preserve_ordinary_binding_and_body_errors
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('identity', 'optional', 'shape', 'raises', 'bad_return', 'idle_default'):
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

import ctypes
from pathlib import Path
from types import ModuleType
import checked
from support import events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

# The ordinary binder control is exactly the analyzed source without opt-in;
# it does not borrow authenticated code objects or publish another contract.
ordinary = ModuleType("ordinary_disabled_boundaries")
source = Path(checked.__file__).read_text()
exec(compile(source.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
             "<ordinary-disabled-boundaries>", "exec", dont_inherit=True),
     vars(ordinary))

call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object

def python_call(function, args, keywords):
    return function(*args, **keywords)

def error_from(operation, expected_type, expected_args=None):
    try:
        operation()
    except Exception as error:
        # StrictMutationError (a TypeError subclass) is not a value-check result.
        assert type(error) is expected_type, (type(error), error)
        if expected_args is not None:
            assert error.args == expected_args, error.args
        return error.args
    raise AssertionError("required failure was skipped")

def exercise(invoke):
    assert invoke(checked.identity, (7,), {}) == 7
    assert invoke(checked.optional, (None,), {}) is None
    assert invoke(checked.shape, (3,), {}) == 5
    error_from(lambda: invoke(checked.raises, (1,), {}),
               LookupError, ("body wins",))
    error_from(lambda: invoke(checked.raises, ("bad",), {}),
               LookupError, ("body wins",))

    result = object()
    assert invoke(checked.bad_return, (result,), {}) is result
    assert invoke(checked.shape, (1, 2, "bad"),
                  {"named": [], "extra": "bad"}) == 3
    assert invoke(checked.identity, (result,), {}) is result
    value = []
    assert invoke(checked.optional, (value,), {}) is value

    # Ordinary argument binding still precedes the body. An annotation does
    # not change the native error's kind, arguments or priority.
    for name, args, keywords in (
        ("identity", (), {}),
        ("identity", ("bad", 2), {}),
        ("identity", ("bad",), {"unexpected": 1}),
        ("shape", ("bad", 2), {"second": 3}),
    ):
        expected = error_from(
            lambda: invoke(getattr(ordinary, name), args, keywords), TypeError)
        assert error_from(
            lambda: invoke(getattr(checked, name), args, keywords), TypeError
        ) == expected

exercise(python_call)
for value in range(128):
    assert checked.identity(value) == value
    assert checked.shape(value) == value + 2
exercise(python_call)
exercise(call)
assert "annotation evaluated" not in events
diagnostic = _soac_ext.strict_module_diagnostics(checked)
for function in (checked.identity, checked.shape, checked.raises):
    observed = _assert_cpython_function_witness(
        function, diagnostic)
    assert observed["original_code_entered"] is True
_assert_cpython_function_witness(
    checked.bad_return, diagnostic)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_cpython_backend_public_binders_returns_and_c_callers
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('identity', 'shape', 'bad_return', 'raises', 'idle_default'):
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

import ctypes
import types
import checked
from support import events
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError
from tests._strict_integration import _assert_cpython_function_witness

def rejected(call, contains=None):
    try:
        call()
    except TypeError as error:
        if contains is not None:
            assert contains in str(error), str(error)
    else:
        raise AssertionError('ordinary argument binding error was skipped')

def exercise():
    assert checked.identity(True) is True
    value = int('12345678901234567890')
    assert checked.identity(value) is value
    assert checked.widened(value) is value
    assert checked.optional(None) is None
    assert checked.optional('ok') == 'ok'
    assert checked.shape(3, 4, 5, 6, named=None, extra=7) == 7
    assert checked.shape(3) == 5
    assert checked.identity('bad') == 'bad'
    marker = []
    assert checked.optional(marker) is marker
    assert checked.shape(1, 2, 'bad') == 3
    assert checked.shape(1, extra='bad') == 3
    rejected(lambda: checked.shape('bad', 2, second=3), 'multiple values')
    rejected(lambda: checked.identity('bad', unexpected=1), 'unexpected keyword')
    rejected(lambda: checked.identity(), 'missing')
    assert checked.bad_return('bad') == 'bad'
    assert checked.caller('bad') == 'bad'
    try:
        checked.raises(1)
    except LookupError as error:
        assert str(error) == 'body wins'
    else:
        raise AssertionError('body exception was replaced or lost')

# Cold and then repeatedly exercised ordinary CPython caller bytecode. Safe
# generic fallback is allowed; no particular specialization opcode is required.
exercise()
for number in range(128):
    assert checked.caller(number) == number
    assert checked.shape(number, named=None) == number + 2
exercise()

call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
one = ctypes.pythonapi.PyObject_CallOneArg
one.argtypes = [ctypes.py_object, ctypes.py_object]
one.restype = ctypes.py_object
assert one(checked.identity, 7) == 7
assert call(checked.shape, (3, 4, 5), {'named': None, 'extra': 6}) == 7
assert one(checked.identity, 'bad') == 'bad'
assert call(checked.shape, (1,), {'extra': 'bad'}) == 3
rejected(lambda: call(checked.identity, ('bad',), {'unexpected': 1}), 'unexpected keyword')
assert one(checked.bad_return, 'bad') == 'bad'
assert 'annotation evaluated' not in events

diagnostic = _soac_ext.strict_module_diagnostics(checked)
for function in (checked.identity, checked.shape, checked.bad_return, checked.raises):
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed['original_code_entered'] is True
ordinary = lambda value: value
assert _soac_ext.strict_function_diagnostics(ordinary) is None
copy = types.FunctionType(checked.identity.__code__, checked.__dict__)
copy.__dict__.update(checked.identity.__dict__)
assert _soac_ext.strict_function_diagnostics(copy) is None
try:
    copy(7)
except StrictRuntimeUnavailableError:
    pass
else:
    raise AssertionError('copied source code acquired original-body ownership')
assert checked.identity(8) == 8

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_cpython_backend_returns_and_callback_errors_preserve_frame_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('finish_with_result',):
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

import gc
import sys
import weakref
import checked
from soac import _soac_ext

events = []
references = []
class Payload:
    def __del__(self):
        events.append(('drop', sys.exception()))
def create():
    payload = Payload()
    references.append(weakref.ref(payload))
    return payload

outer = ValueError('caller handler')
try:
    raise outer
except ValueError:
    assert checked.finish_with_result(create, events.append, 7) == 7
    assert events == ['body', ('drop', outer)], events
    assert references[-1]() is None
    events.clear()
    assert checked.finish_with_result(create, events.append, 'wrong') == 'wrong'
    assert events == ['body', ('drop', outer)], events
    assert references[-1]() is None
    events.clear()
    failure = RuntimeError('explicit callback failure')
    def fail(stage):
        events.append(stage)
        raise failure
    try:
        checked.finish_with_result(create, fail, 'wrong')
    except RuntimeError as error:
        assert error is failure
        assert isinstance(error.__context__, LookupError)
        assert str(error.__context__) == 'source handler'
        assert error.__context__.__context__ is outer
        assert [event for event in events if event == 'body'] == ['body']
        error.__context__.__traceback__ = None
        error.__traceback__ = None
    else:
        raise AssertionError('the explicit callback error was lost')
    gc.collect()
    assert references[-1]() is None
    assert len([event for event in events if isinstance(event, tuple) and event[0] == 'drop']) == 1
    assert sys.exception() is outer
assert sys.exception() is None
assert _soac_ext.strict_function_diagnostics(
    checked.finish_with_result
)['original_code_entered'] is True

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_cpython_function_c_metadata_setters_preserve_frozen_source_ownership
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('shape', 'make_cycle'):
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

import ctypes
import types
import pytest
import checked
from soac import _soac_ext
from soac.strict import StrictMutationError
from tests._strict_integration import _assert_cpython_function_witness
ordinary = types.ModuleType('ordinary_function_capi_control')
exec(compile('\nfrom typing import Any, cast, final\nfrom support import events, marker, observe\n\ndef identity(value: int) -> int:\n    return value\n\nfirst_lambda, second_lambda = (lambda value: value), (lambda value: value + 1)\n\ndef widened(value: float) -> float:\n    return value\n\ndef optional(value: int | str | None) -> int | str | None:\n    return value\n\ndef shape(first: int, /, second: int = 2, *items: int,\n          named: str | None = None, **extras: int) -> int:\n    return first + second\n\ndef caller(value: Any) -> int:\n    return identity(value)\n\ndef bad_return(value: Any) -> int:\n    return cast(int, value)\n\ndef finish_with_result(factory, observer, result: Any) -> int:\n    payload = factory()\n    try:\n        raise LookupError("source handler")\n    except LookupError:\n        observer("body")\n        return cast(int, result)\n\ndef raises(value: int) -> int:\n    raise LookupError("body wins")\n\ndef annotation_trap(format: int):\n    events.append("annotation evaluated")\n    raise AssertionError("annotation provider must never be called by a boundary")\n\nidentity.__annotate__ = annotation_trap\n\ndef active_default(value=marker("active-old")) -> None:\n    active_default.__defaults__ = (marker("active-new"),)\n    observe(value)\n\nactive_default()\nevents.append("after-active")\n\ndef idle_default(value=marker("idle-old")):\n    return value\n\nidle_default.__defaults__ = (marker("idle-new"),)\nevents.append("after-idle")\n\ndef make_cycle():\n    captured = []\n    def inner(value: int) -> int:\n        return value + len(captured)\n    captured.append(inner)\n    return inner\n\nclass StoppingIterator:\n    def __next__(self):\n        raise StopIteration\n\ndef catch_stop(iterator, observer):\n    try:\n        return next(iterator)\n    except StopIteration:\n        return observer()\n\nclass ReturningIterator:\n    def __next__(self):\n        try:\n            raise LookupError("callee handler")\n        except LookupError:\n            return 7\n\ndef replace_result(iterator, create):\n    value = create()\n    value = next(iterator)\n    return value\n', '<ordinary-function-capi>', 'exec', dont_inherit=True), ordinary.__dict__)

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = list(arguments)
    function.restype = result
    return function

obj = ctypes.py_object
get_owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
diagnostic = _soac_ext.strict_module_diagnostics(checked)
for name in ("identity", "shape", "bad_return", "caller"):
    function = getattr(checked, name)
    assert get_owner(function)
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed["finalized"] is True
    control = getattr(ordinary, name)
    assert not get_owner(control)
    assert _soac_ext.strict_function_diagnostics(control) is None

inner = checked.make_cycle()
control_inner = ordinary.make_cycle()
assert inner(3) == control_inner(3) == 4
assert get_owner(inner)
observed = _assert_cpython_function_witness(
    inner, diagnostic,
)
assert observed["finalized"] is True and observed["original_code_entered"] is True
assert not get_owner(control_inner)
assert _soac_ext.strict_function_diagnostics(control_inner) is None

functions = (checked.shape, inner)
originals = tuple(
    (function.__code__, function.__globals__, function.__defaults__,
     function.__kwdefaults__, function.__closure__, function.__annotate__,
     get_owner(function))
    for function in functions
)
keywords = checked.shape.__kwdefaults__
assert type(keywords) is dict and keywords == {"named": None}
keyword_items = tuple(keywords.items())
defaults = (9,)
replacements = {"named": "changed"}
annotations = {"first": str}
closure = (types.CellType(["first", "second"]),)
assert len(closure) == len(inner.__closure__) == len(control_inner.__closure__) == 1

for name, native, control, attribute, replacement in (
    ("PyFunction_SetDefaults", checked.shape, ordinary.shape, "__defaults__", defaults),
    ("PyFunction_SetKwDefaults", checked.shape, ordinary.shape, "__kwdefaults__", replacements),
    ("PyFunction_SetAnnotations", checked.shape, ordinary.shape, "__annotations__", annotations),
    ("PyFunction_SetClosure", inner, control_inner, "__closure__", closure),
):
    setter = api(name, ctypes.c_int, obj, obj)
    assert setter(control, replacement) == 0
    assert getattr(control, attribute) is replacement
    with pytest.raises(StrictMutationError):
        setter(native, replacement)
    for function, original in zip(functions, originals):
        actual = (
            function.__code__, function.__globals__, function.__defaults__,
            function.__kwdefaults__, function.__closure__, function.__annotate__,
        )
        assert all(value is expected for value, expected in zip(actual, original[:-1]))
        assert get_owner(function) == original[-1]
    assert checked.shape.__kwdefaults__ is keywords
    assert tuple(keywords.items()) == keyword_items

# The actual keyword-default mapping remains protected against aliases,
# not only against replacement through the function's setter.
set_item = api("PyDict_SetItem", ctypes.c_int, obj, obj, obj)
del_item = api("PyDict_DelItem", ctypes.c_int, obj, obj)
for operation in (
    lambda mapping: set_item(mapping, "named", "other"),
    lambda mapping: del_item(mapping, "named"),
):
    assert operation(replacements) == 0
    with pytest.raises(StrictMutationError):
        operation(keywords)
    assert tuple(keywords.items()) == keyword_items
assert checked.shape(1) == 3
assert ordinary.shape(1, named=None) == 10
assert inner(3) == 4 and control_inner(3) == 5
with pytest.raises(TypeError):
    checked.shape("wrong")
with pytest.raises(TypeError):
    inner("wrong")
from support import events
assert "annotation evaluated" not in events
for function in functions:
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed["original_code_entered"] is True

assert _soac_ext.runtime_compilation_activity() == {
    "schema": 1, "lowering_entries": 0, "blockpy_cache_entries": 0,
    "jit_engine_entries": 0,
}

_assert_source_function_witnesses()
