# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:changed_code
# soac: module(strict_assign=true, checked_attr=true)
from fallback_probe import marker, replace

@replace
def changed(value):
    return value
# module:fallback_probe
marker = LookupError("replacement body exception")

def replacement(value):
    if value is marker:
        raise marker
    return value

def replace(function):
    function.__code__ = replacement.__code__
    return function
# ok
# tests/test_strict_function_boundaries.py::test_warmed_changed_code_calls_keep_live_owner_checks
import sys
from soac import _soac_ext, import_hook

disposition = 'sealed'

import ctypes
import gc
import changed_code
import fallback_probe
from soac.strict import StrictRuntimeUnavailableError

function = changed_code.changed
vectorcall = ctypes.pythonapi.PyVectorcall_Function
vectorcall.argtypes = [ctypes.py_object]
vectorcall.restype = ctypes.c_void_p
native_vectorcall = vectorcall(fallback_probe.replacement)

# This caller is ordinary native CPython code. Repeated calls exercise its
# adaptive CALL path, not a manufactured Rust helper or SOAC direct target.
def invoke():
    return function(7)

for _ in range(256):
    assert invoke() == 7
assert vectorcall(function) != native_vectorcall, "fallback published unchecked native vectorcall"
try:
    function(fallback_probe.marker)
except LookupError as error:
    assert error is fallback_probe.marker
else:
    raise AssertionError("native replacement lost its original exception")

owner, = [value for value in gc.get_referents(function)
          if type(value).__name__ == "_StrictFunctionOwner"]

# Trusted C test probes change only the real native/GC state. They are not
# production authority paths and do not manufacture a replacement contract.
if disposition == "sealed":
    seal = ctypes.pythonapi.PyFunction_SealSoacStrict
    seal.argtypes = [ctypes.py_object, ctypes.c_uint64]
    seal.restype = ctypes.c_int
    assert seal(function, 0x5EA1) == 0
else:
    get_slot = ctypes.pythonapi.PyType_GetSlot
    get_slot.argtypes = [ctypes.py_object, ctypes.c_int]
    get_slot.restype = ctypes.c_void_p
    # Py_tp_clear from the selected CPython's stable typeslots.h API.
    clear_address = get_slot(type(owner), 51)
    assert clear_address
    clear = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object)(clear_address)
    assert clear(owner) == 0

for _ in range(256):
    try:
        invoke()
    except StrictRuntimeUnavailableError:
        pass
    else:
        raise AssertionError("a warmed native CALL bypassed owner/contract rejection")
print("warmed-replacement-owner-check", disposition)
# ok
# tests/test_strict_function_boundaries.py::test_warmed_changed_code_calls_keep_live_owner_checks
import sys
from soac import _soac_ext, import_hook

disposition = 'terminal'

import ctypes
import gc
import changed_code
import fallback_probe
from soac.strict import StrictRuntimeUnavailableError

function = changed_code.changed
vectorcall = ctypes.pythonapi.PyVectorcall_Function
vectorcall.argtypes = [ctypes.py_object]
vectorcall.restype = ctypes.c_void_p
native_vectorcall = vectorcall(fallback_probe.replacement)

# This caller is ordinary native CPython code. Repeated calls exercise its
# adaptive CALL path, not a manufactured Rust helper or SOAC direct target.
def invoke():
    return function(7)

for _ in range(256):
    assert invoke() == 7
assert vectorcall(function) != native_vectorcall, "fallback published unchecked native vectorcall"
try:
    function(fallback_probe.marker)
except LookupError as error:
    assert error is fallback_probe.marker
else:
    raise AssertionError("native replacement lost its original exception")

owner, = [value for value in gc.get_referents(function)
          if type(value).__name__ == "_StrictFunctionOwner"]

# Trusted C test probes change only the real native/GC state. They are not
# production authority paths and do not manufacture a replacement contract.
if disposition == "sealed":
    seal = ctypes.pythonapi.PyFunction_SealSoacStrict
    seal.argtypes = [ctypes.py_object, ctypes.c_uint64]
    seal.restype = ctypes.c_int
    assert seal(function, 0x5EA1) == 0
else:
    get_slot = ctypes.pythonapi.PyType_GetSlot
    get_slot.argtypes = [ctypes.py_object, ctypes.c_int]
    get_slot.restype = ctypes.c_void_p
    # Py_tp_clear from the selected CPython's stable typeslots.h API.
    clear_address = get_slot(type(owner), 51)
    assert clear_address
    clear = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object)(clear_address)
    assert clear(owner) == 0

for _ in range(256):
    try:
        invoke()
    except StrictRuntimeUnavailableError:
        pass
    else:
        raise AssertionError("a warmed native CALL bypassed owner/contract rejection")
print("warmed-replacement-owner-check", disposition)
