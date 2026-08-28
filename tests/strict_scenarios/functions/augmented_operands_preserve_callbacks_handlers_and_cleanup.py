# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:augmented_operand_model
# soac: module(strict_assign=true, checked_attr=true)

def local_target(start, update, target, key, record):
    value = start()
    try:
        value += update()
    except (LookupError, OSError):
        record("handler")
    else:
        record("after")
        del value
    record("end")

def attribute_target(start, update, target, key, record):
    try:
        target().field += update()
    except (LookupError, OSError):
        record("handler")
    else:
        record("after")
    record("end")

def subscript_target(start, update, target, key, record):
    try:
        target()[key()] += update()
    except (LookupError, OSError):
        record("handler")
    else:
        record("after")
    record("end")
# module:ordinary_augmented_operand_model
def local_target(start, update, target, key, record):
    value = start()
    try:
        value += update()
    except (LookupError, OSError):
        record("handler")
    else:
        record("after")
        del value
    record("end")

def attribute_target(start, update, target, key, record):
    try:
        target().field += update()
    except (LookupError, OSError):
        record("handler")
    else:
        record("after")
    record("end")

def subscript_target(start, update, target, key, record):
    try:
        target()[key()] += update()
    except (LookupError, OSError):
        record("handler")
    else:
        record("after")
    record("end")
# ok
# tests/test_strict_function_boundaries.py::test_augmented_operands_preserve_callbacks_handlers_and_cleanup
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import augmented_operand_model
import ordinary_augmented_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(augmented_operand_model, 'local_target')
ordinary = getattr(ordinary_augmented_operand_model, 'local_target')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(augmented_operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_augmented_operand_model) is None
assert _soac_ext.strict_function_entry_kind(function) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

def exercise(function, outcome):
    events = []
    live = [0]
    def record(*event):
        current = sys.exception()
        events.append((*event, None if current is None else type(current).__name__))
    class Value:
        def __init__(self, label):
            self.label = label
            live[0] += 1
        def __iadd__(self, other):
            record('iadd', self.label, other.label)
            if outcome == 'operation_error':
                raise LookupError('operation failed')
            return self if outcome == 'inplace' else Value('result')
        def __del__(self):
            live[0] -= 1
            record('drop', self.label)
    class Target:
        def __init__(self):
            live[0] += 1
        @property
        def field(self):
            record('get')
            return Value('old')
        @field.setter
        def field(self, value):
            record('set', value.label)
            if outcome == 'target_error':
                raise OSError('target failed')
        def __getitem__(self, key):
            record('getitem')
            return Value('old')
        def __setitem__(self, key, value):
            record('setitem', value.label)
            if outcome == 'target_error':
                raise OSError('target failed')
        def __del__(self):
            live[0] -= 1
            record('drop', 'target')
    class Key:
        def __init__(self):
            live[0] += 1
        def __del__(self):
            live[0] -= 1
            record('drop', 'key')
    try:
        raise KeyError('caller handler')
    except KeyError as marker:
        function(lambda: Value('old'), lambda: Value('rhs'), Target, Key, record)
        assert sys.exception() is marker
    gc.collect()
    assert live[0] == 0, (outcome, events, live)
    return (
        [event for event in events if event[0] != 'drop'],
        sorted(event[1] for event in events if event[0] == 'drop'),
    )

for outcome in ('replacement', 'inplace', 'operation_error', 'target_error'):
    expected = exercise(ordinary, outcome)
    observed = exercise(function, outcome)
    assert observed == expected, (outcome, observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_operands_preserve_callbacks_handlers_and_cleanup
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import augmented_operand_model
import ordinary_augmented_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(augmented_operand_model, 'attribute_target')
ordinary = getattr(ordinary_augmented_operand_model, 'attribute_target')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(augmented_operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_augmented_operand_model) is None
assert _soac_ext.strict_function_entry_kind(function) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

def exercise(function, outcome):
    events = []
    live = [0]
    def record(*event):
        current = sys.exception()
        events.append((*event, None if current is None else type(current).__name__))
    class Value:
        def __init__(self, label):
            self.label = label
            live[0] += 1
        def __iadd__(self, other):
            record('iadd', self.label, other.label)
            if outcome == 'operation_error':
                raise LookupError('operation failed')
            return self if outcome == 'inplace' else Value('result')
        def __del__(self):
            live[0] -= 1
            record('drop', self.label)
    class Target:
        def __init__(self):
            live[0] += 1
        @property
        def field(self):
            record('get')
            return Value('old')
        @field.setter
        def field(self, value):
            record('set', value.label)
            if outcome == 'target_error':
                raise OSError('target failed')
        def __getitem__(self, key):
            record('getitem')
            return Value('old')
        def __setitem__(self, key, value):
            record('setitem', value.label)
            if outcome == 'target_error':
                raise OSError('target failed')
        def __del__(self):
            live[0] -= 1
            record('drop', 'target')
    class Key:
        def __init__(self):
            live[0] += 1
        def __del__(self):
            live[0] -= 1
            record('drop', 'key')
    try:
        raise KeyError('caller handler')
    except KeyError as marker:
        function(lambda: Value('old'), lambda: Value('rhs'), Target, Key, record)
        assert sys.exception() is marker
    gc.collect()
    assert live[0] == 0, (outcome, events, live)
    return (
        [event for event in events if event[0] != 'drop'],
        sorted(event[1] for event in events if event[0] == 'drop'),
    )

for outcome in ('replacement', 'inplace', 'operation_error', 'target_error'):
    expected = exercise(ordinary, outcome)
    observed = exercise(function, outcome)
    assert observed == expected, (outcome, observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_operands_preserve_callbacks_handlers_and_cleanup
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import augmented_operand_model
import ordinary_augmented_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(augmented_operand_model, 'subscript_target')
ordinary = getattr(ordinary_augmented_operand_model, 'subscript_target')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(augmented_operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_augmented_operand_model) is None
assert _soac_ext.strict_function_entry_kind(function) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

def exercise(function, outcome):
    events = []
    live = [0]
    def record(*event):
        current = sys.exception()
        events.append((*event, None if current is None else type(current).__name__))
    class Value:
        def __init__(self, label):
            self.label = label
            live[0] += 1
        def __iadd__(self, other):
            record('iadd', self.label, other.label)
            if outcome == 'operation_error':
                raise LookupError('operation failed')
            return self if outcome == 'inplace' else Value('result')
        def __del__(self):
            live[0] -= 1
            record('drop', self.label)
    class Target:
        def __init__(self):
            live[0] += 1
        @property
        def field(self):
            record('get')
            return Value('old')
        @field.setter
        def field(self, value):
            record('set', value.label)
            if outcome == 'target_error':
                raise OSError('target failed')
        def __getitem__(self, key):
            record('getitem')
            return Value('old')
        def __setitem__(self, key, value):
            record('setitem', value.label)
            if outcome == 'target_error':
                raise OSError('target failed')
        def __del__(self):
            live[0] -= 1
            record('drop', 'target')
    class Key:
        def __init__(self):
            live[0] += 1
        def __del__(self):
            live[0] -= 1
            record('drop', 'key')
    try:
        raise KeyError('caller handler')
    except KeyError as marker:
        function(lambda: Value('old'), lambda: Value('rhs'), Target, Key, record)
        assert sys.exception() is marker
    gc.collect()
    assert live[0] == 0, (outcome, events, live)
    return (
        [event for event in events if event[0] != 'drop'],
        sorted(event[1] for event in events if event[0] == 'drop'),
    )

for outcome in ('replacement', 'inplace', 'operation_error', 'target_error'):
    expected = exercise(ordinary, outcome)
    observed = exercise(function, outcome)
    assert observed == expected, (outcome, observed, expected)
