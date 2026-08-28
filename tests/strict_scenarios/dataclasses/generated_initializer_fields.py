# modes:soac,entry
# module:generated_check_support
events = []
produced = 11
default_value = 7
factory_raises = False
factory_error = RuntimeError('ordinary factory failure')

def make_value() -> int:
    events.append('factory')
    if factory_raises:
        raise factory_error
    return produced
# module:generated_check_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass, field
import generated_check_support

@dataclass
class Factory:
    value: int = field(default_factory=generated_check_support.make_value)

@dataclass
class Default:
    value: int = generated_check_support.default_value

def make_watched():
    @dataclass
    class Watched:
        value: int

        def accept(self, value: int) -> int:
            generated_check_support.events.append(('source', value))
            return value

    return Watched
# ok
# test_generated_dataclass_factory_marker_keeps_ordinary_expression_semantics [default]
import sys
from soac import _soac_ext
from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ctypes
import dataclasses
import generated_check_support as support

def assert_generated_owner(cls):
    class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    class_owner.argtypes = [ctypes.py_object]
    class_owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert class_owner(cls) and function_owner(cls.__init__)
    # Generated ownership does not manufacture source/JIT metadata.
    assert not metadata(cls.__init__)

from generated_check_model import Factory
support.events.clear()
# The sentinel is not an int argument. Ordinary generated control flow must
# consume it and assign the actual compatible factory result to the field.
assert Factory(dataclasses._HAS_DEFAULT_FACTORY).value == 11
assert support.events == ['factory']
support.events.clear()
assert Factory().value == 11 and support.events == ['factory']
support.events.clear()
assert Factory(9).value == 9 and support.events == []
assert_generated_owner(Factory)
# ok
# test_generated_dataclass_factory_values_and_errors_preserve_foreign_assignment [default]
import sys
from soac import _soac_ext
from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ctypes
import dataclasses
import generated_check_support as support

def assert_generated_owner(cls):
    class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    class_owner.argtypes = [ctypes.py_object]
    class_owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert class_owner(cls) and function_owner(cls.__init__)
    # Generated ownership does not manufacture source/JIT metadata.
    assert not metadata(cls.__init__)

from generated_check_model import Factory
class Foreign:
    pass

foreign = Foreign()
support.produced = 'wrong'
support.events.clear()
assert Factory.__init__(foreign) is None
assert support.events == ['factory'], 'factory execution must not be replayed'
assert foreign.value == 'wrong'
support.events.clear()
assert Factory.__init__(foreign, 'explicit') is None
assert support.events == [] and foreign.value == 'explicit'
assert Factory.__init__(foreign, 5) is None and foreign.value == 5
del foreign.value
support.factory_raises = True
support.events.clear()
try:
    Factory.__init__(foreign)
except RuntimeError as error:
    assert error is support.factory_error
else:
    raise AssertionError('ordinary factory exception was lost')
assert support.events == ['factory'] and not hasattr(foreign, 'value')
assert_generated_owner(Factory)
# ok
# test_generated_dataclass_public_vectorcall_and_copies_keep_ordinary_calls [default]
import sys
from soac import _soac_ext
from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ctypes
import dataclasses
import generated_check_support as support

def assert_generated_owner(cls):
    class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    class_owner.argtypes = [ctypes.py_object]
    class_owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert class_owner(cls) and function_owner(cls.__init__)
    # Generated ownership does not manufacture source/JIT metadata.
    assert not metadata(cls.__init__)

import types
from generated_check_model import Factory
assert_generated_owner(Factory)
function = Factory.__init__
vectorcall = ctypes.pythonapi.PyVectorcall_Function
vectorcall.argtypes = [ctypes.py_object]
vectorcall.restype = ctypes.c_void_p
setter = ctypes.pythonapi.PyFunction_SetVectorcall
setter.argtypes = [ctypes.py_object, ctypes.c_void_p]
setter.restype = None
original_entry = vectorcall(function)
stock_entry = ctypes.cast(ctypes.pythonapi._PyFunction_Vectorcall, ctypes.c_void_p).value
assert original_entry and stock_entry
for _ in range(128):
    assert Factory(4).value == 4

class Foreign:
    pass

foreign = Foreign()
setter(function, stock_entry)
try:
    assert function(foreign, 'ordinary') is None
    assert foreign.value == 'ordinary'
    field_write_rejected(lambda: Factory('ordinary'))
finally:
    setter(function, original_entry)
assert Factory(5).value == 5

# Ordinary public copies get no creation-record ownership, and their ordinary
# bytecode remains executable with ordinary value semantics.
copy = types.FunctionType(function.__code__, function.__globals__,
                          argdefs=function.__defaults__, closure=function.__closure__,
                          kwdefaults=function.__kwdefaults__)
assert copy(foreign, 'ordinary') is None and foreign.value == 'ordinary'
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert not owner(copy)
receiver = Factory(5)
field_write_rejected(lambda: copy(receiver, 'ordinary'))
assert receiver.value == 5, 'an unowned public copy bypassed the storage predicate'
# ok
# test_generated_dataclass_factory_conditional_observes_native_frame_changes [default]
import sys
from soac import _soac_ext
from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ctypes
import dataclasses
import generated_check_support as support

def assert_generated_owner(cls):
    class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    class_owner.argtypes = [ctypes.py_object]
    class_owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert class_owner(cls) and function_owner(cls.__init__)
    # Generated ownership does not manufacture source/JIT metadata.
    assert not metadata(cls.__init__)

import sys
from generated_check_model import Factory
assert_generated_owner(Factory)
function = Factory.__init__
code = function.__code__
marker = dataclasses._HAS_DEFAULT_FACTORY
marker_cells = [cell for cell in function.__closure__ if cell.cell_contents is marker]
assert len(marker_cells) == 1
marker_cell = marker_cells[0]
events = []

class Foreign:
    pass

foreign = Foreign()
def change_marker(frame, event, argument):
    if frame.f_code is code and event == 'line':
        marker_cell.cell_contents = object()
        events.append('marker')
        return None
    return change_marker

support.events.clear()
sys.settrace(change_marker)
try:
    assert function(foreign) is None
finally:
    sys.settrace(None)
    marker_cell.cell_contents = marker
assert events and support.events == []
assert foreign.value is marker

def change_supplied(frame, event, argument):
    if frame.f_code is code and event == 'line':
        frame.f_locals['value'] = 'changed after entry'
        events.append('supplied')
        return None
    return change_supplied

sys.settrace(change_supplied)
try:
    assert function(foreign, 7) is None
finally:
    sys.settrace(None)
assert 'supplied' in events
assert foreign.value == 'changed after entry'
assert support.events == [], 'a supplied argument was misclassified as omitted'
assert Factory().value == 11
