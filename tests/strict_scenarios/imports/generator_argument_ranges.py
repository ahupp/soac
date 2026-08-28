# modes:cpython
# test_strict_import_admission.py::test_cpython_backend_generator_argument_ranges_preserve_native_execution
# module:generator_arguments
# soac: module(strict_assign=true, checked_attr=true)

from generator_argument_support import events

def checked(value: int) -> int:
    events.append(value)
    return value

def sole(values):
    return list(checked(value) for value in values)

def explicit(values):
    return list((checked(value) for value in values))

def grouped(values):
    return ((list))((checked(value)) for value in values)

def multiline(values):
    return list(  # the argument delimiter belongs to the native genexpr
        checked(élément) for élément in values  # preserve the final comment
    )

def nested(groups):
    return list(tuple(checked(value) for value in values) for values in groups)
# module:ordinary_generator_arguments
from generator_argument_support import events

def checked(value: int) -> int:
    events.append(value)
    return value

def sole(values):
    return list(checked(value) for value in values)

def explicit(values):
    return list((checked(value) for value in values))

def grouped(values):
    return ((list))((checked(value)) for value in values)

def multiline(values):
    return list(  # the argument delimiter belongs to the native genexpr
        checked(élément) for élément in values  # preserve the final comment
    )

def nested(groups):
    return list(tuple(checked(value) for value in values) for values in groups)
# module:generator_argument_support
from typing import Any
events: list[Any] = []
# ok
# test_cpython_backend_generator_argument_ranges_preserve_native_execution
import sys
import pytest
from soac import _soac_ext, import_hook
import ctypes
import generator_arguments as module
import ordinary_generator_arguments as ordinary
from generator_argument_support import events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

diagnostic = _soac_ext.strict_module_diagnostics(module)
assert _soac_ext.strict_module_diagnostics(ordinary) is None
names = ("checked", "sole", "explicit", "grouped", "multiline", "nested")
for name in names:
    function = vars(module)[name]
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed["original_code_entered"] is False
    assert _soac_ext.strict_function_diagnostics(vars(ordinary)[name]) is None

for name in ("sole", "explicit", "grouped", "multiline"):
    native = vars(module)[name]
    stock = vars(ordinary)[name]
    events.clear()
    expected = stock([2, 3, 5])
    assert expected == [2, 3, 5] and events == [2, 3, 5]
    events.clear()
    assert native([2, 3, 5]) == expected and events == [2, 3, 5]
    assert _soac_ext.strict_function_diagnostics(native)["original_code_entered"] is True

events.clear()
expected = ordinary.nested([[2, 3], [], [5]])
assert expected == [(2, 3), (), (5,)] and events == [2, 3, 5]
events.clear()
assert module.nested([[2, 3], [], [5]]) == expected
assert events == [2, 3, 5]
for _ in range(128):
    events.clear()
    assert module.sole([7, 11]) == [7, 11]
    assert events == [7, 11]

call = ctypes.pythonapi.PyObject_CallOneArg
call.argtypes = [ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
events.clear()
assert call(module.sole, [13, 17]) == [13, 17]
assert events == [13, 17]
for invoke in (module.sole, lambda values: call(module.sole, values)):
    events.clear()
    assert invoke([1, "ordinary", 3]) == [1, "ordinary", 3]
    assert events == [1, "ordinary", 3], "an annotation skipped an original body callback"
events.clear()
assert ordinary.sole([1, "ordinary", 3]) == [1, "ordinary", 3]
assert events == [1, "ordinary", 3]
assert _soac_ext.strict_function_diagnostics(module.checked)["original_code_entered"] is True
assert _soac_ext.strict_function_diagnostics(module.nested)["original_code_entered"] is True
