# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:operand_model
# soac: module(strict_assign=true, checked_attr=true)

async def augmented_name(make, wait):
    value = make()
    value += await wait
    return value

async def augmented_attribute(make, wait):
    make().value += await wait

async def augmented_subscript(make, wait):
    make()[0] += await wait
# module:ordinary_operand_model
async def augmented_name(make, wait):
    value = make()
    value += await wait
    return value

async def augmented_attribute(make, wait):
    make().value += await wait

async def augmented_subscript(make, wait):
    make()[0] += await wait
# ok
# tests/test_strict_function_boundaries.py::test_augmented_await_retires_each_operand_once
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import operand_model
import ordinary_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(operand_model, 'augmented_' + 'name')
ordinary = getattr(ordinary_operand_model, 'augmented_' + 'name')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

def exercise(function):
    events = []
    live = [0, 0]

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    class Value:
        def __init__(self):
            live[0] += 1
        def __iadd__(self, other):
            events.append(('add', other, context()))
            return 'updated'
        def __del__(self):
            live[0] -= 1
            events.append(('drop-value', context()))

    class Target:
        def __init__(self):
            live[1] += 1
        @property
        def value(self):
            events.append(('get', context()))
            return Value()
        @value.setter
        def value(self, value):
            assert value == 'updated'
            events.append(('set', value, context()))
        def __getitem__(self, key):
            assert key == 0
            return self.value
        def __setitem__(self, key, value):
            assert key == 0
            self.value = value
        def __del__(self):
            live[1] -= 1
            events.append(('drop-target', context()))

    class Wait:
        def __await__(self):
            events.append(('wait', context()))
            yield 'ready'
            if 'complete' == 'fail':
                raise LookupError('await failed')
            return 4

    try:
        raise KeyError('caller')
    except KeyError as caller:
        coroutine = function(Value if 'name' == 'name' else Target, Wait())
        assert coroutine.send(None) == 'ready'
        assert live == [1, int('name' != 'name')]
        if 'complete' == 'close':
            coroutine.close()
            events.append(('closed', context()))
        else:
            try:
                coroutine.send(None)
            except StopIteration as done:
                assert 'complete' == 'complete'
                assert done.value == ('updated' if 'name' == 'name' else None)
                events.append(('complete', done.value, context()))
            except LookupError as failure:
                assert 'complete' == 'fail' and str(failure) == 'await failed'
                failure.__traceback__ = None
                events.append(('failed', context()))
            else:
                raise AssertionError('coroutine unexpectedly suspended twice')
        assert sys.exception() is caller
        del coroutine
    gc.collect()
    assert live == [0, 0], (events, live)
    return (
        [event for event in events if not event[0].startswith('drop-')],
        sorted(event[0] for event in events if event[0].startswith('drop-')),
    )

expected = exercise(ordinary)
observed = exercise(function)
assert observed == expected, (observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_await_retires_each_operand_once
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import operand_model
import ordinary_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(operand_model, 'augmented_' + 'name')
ordinary = getattr(ordinary_operand_model, 'augmented_' + 'name')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

def exercise(function):
    events = []
    live = [0, 0]

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    class Value:
        def __init__(self):
            live[0] += 1
        def __iadd__(self, other):
            events.append(('add', other, context()))
            return 'updated'
        def __del__(self):
            live[0] -= 1
            events.append(('drop-value', context()))

    class Target:
        def __init__(self):
            live[1] += 1
        @property
        def value(self):
            events.append(('get', context()))
            return Value()
        @value.setter
        def value(self, value):
            assert value == 'updated'
            events.append(('set', value, context()))
        def __getitem__(self, key):
            assert key == 0
            return self.value
        def __setitem__(self, key, value):
            assert key == 0
            self.value = value
        def __del__(self):
            live[1] -= 1
            events.append(('drop-target', context()))

    class Wait:
        def __await__(self):
            events.append(('wait', context()))
            yield 'ready'
            if 'fail' == 'fail':
                raise LookupError('await failed')
            return 4

    try:
        raise KeyError('caller')
    except KeyError as caller:
        coroutine = function(Value if 'name' == 'name' else Target, Wait())
        assert coroutine.send(None) == 'ready'
        assert live == [1, int('name' != 'name')]
        if 'fail' == 'close':
            coroutine.close()
            events.append(('closed', context()))
        else:
            try:
                coroutine.send(None)
            except StopIteration as done:
                assert 'fail' == 'complete'
                assert done.value == ('updated' if 'name' == 'name' else None)
                events.append(('complete', done.value, context()))
            except LookupError as failure:
                assert 'fail' == 'fail' and str(failure) == 'await failed'
                failure.__traceback__ = None
                events.append(('failed', context()))
            else:
                raise AssertionError('coroutine unexpectedly suspended twice')
        assert sys.exception() is caller
        del coroutine
    gc.collect()
    assert live == [0, 0], (events, live)
    return (
        [event for event in events if not event[0].startswith('drop-')],
        sorted(event[0] for event in events if event[0].startswith('drop-')),
    )

expected = exercise(ordinary)
observed = exercise(function)
assert observed == expected, (observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_await_retires_each_operand_once
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import operand_model
import ordinary_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(operand_model, 'augmented_' + 'name')
ordinary = getattr(ordinary_operand_model, 'augmented_' + 'name')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

def exercise(function):
    events = []
    live = [0, 0]

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    class Value:
        def __init__(self):
            live[0] += 1
        def __iadd__(self, other):
            events.append(('add', other, context()))
            return 'updated'
        def __del__(self):
            live[0] -= 1
            events.append(('drop-value', context()))

    class Target:
        def __init__(self):
            live[1] += 1
        @property
        def value(self):
            events.append(('get', context()))
            return Value()
        @value.setter
        def value(self, value):
            assert value == 'updated'
            events.append(('set', value, context()))
        def __getitem__(self, key):
            assert key == 0
            return self.value
        def __setitem__(self, key, value):
            assert key == 0
            self.value = value
        def __del__(self):
            live[1] -= 1
            events.append(('drop-target', context()))

    class Wait:
        def __await__(self):
            events.append(('wait', context()))
            yield 'ready'
            if 'close' == 'fail':
                raise LookupError('await failed')
            return 4

    try:
        raise KeyError('caller')
    except KeyError as caller:
        coroutine = function(Value if 'name' == 'name' else Target, Wait())
        assert coroutine.send(None) == 'ready'
        assert live == [1, int('name' != 'name')]
        if 'close' == 'close':
            coroutine.close()
            events.append(('closed', context()))
        else:
            try:
                coroutine.send(None)
            except StopIteration as done:
                assert 'close' == 'complete'
                assert done.value == ('updated' if 'name' == 'name' else None)
                events.append(('complete', done.value, context()))
            except LookupError as failure:
                assert 'close' == 'fail' and str(failure) == 'await failed'
                failure.__traceback__ = None
                events.append(('failed', context()))
            else:
                raise AssertionError('coroutine unexpectedly suspended twice')
        assert sys.exception() is caller
        del coroutine
    gc.collect()
    assert live == [0, 0], (events, live)
    return (
        [event for event in events if not event[0].startswith('drop-')],
        sorted(event[0] for event in events if event[0].startswith('drop-')),
    )

expected = exercise(ordinary)
observed = exercise(function)
assert observed == expected, (observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_await_retires_each_operand_once
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import operand_model
import ordinary_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(operand_model, 'augmented_' + 'attribute')
ordinary = getattr(ordinary_operand_model, 'augmented_' + 'attribute')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

def exercise(function):
    events = []
    live = [0, 0]

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    class Value:
        def __init__(self):
            live[0] += 1
        def __iadd__(self, other):
            events.append(('add', other, context()))
            return 'updated'
        def __del__(self):
            live[0] -= 1
            events.append(('drop-value', context()))

    class Target:
        def __init__(self):
            live[1] += 1
        @property
        def value(self):
            events.append(('get', context()))
            return Value()
        @value.setter
        def value(self, value):
            assert value == 'updated'
            events.append(('set', value, context()))
        def __getitem__(self, key):
            assert key == 0
            return self.value
        def __setitem__(self, key, value):
            assert key == 0
            self.value = value
        def __del__(self):
            live[1] -= 1
            events.append(('drop-target', context()))

    class Wait:
        def __await__(self):
            events.append(('wait', context()))
            yield 'ready'
            if 'complete' == 'fail':
                raise LookupError('await failed')
            return 4

    try:
        raise KeyError('caller')
    except KeyError as caller:
        coroutine = function(Value if 'attribute' == 'name' else Target, Wait())
        assert coroutine.send(None) == 'ready'
        assert live == [1, int('attribute' != 'name')]
        if 'complete' == 'close':
            coroutine.close()
            events.append(('closed', context()))
        else:
            try:
                coroutine.send(None)
            except StopIteration as done:
                assert 'complete' == 'complete'
                assert done.value == ('updated' if 'attribute' == 'name' else None)
                events.append(('complete', done.value, context()))
            except LookupError as failure:
                assert 'complete' == 'fail' and str(failure) == 'await failed'
                failure.__traceback__ = None
                events.append(('failed', context()))
            else:
                raise AssertionError('coroutine unexpectedly suspended twice')
        assert sys.exception() is caller
        del coroutine
    gc.collect()
    assert live == [0, 0], (events, live)
    return (
        [event for event in events if not event[0].startswith('drop-')],
        sorted(event[0] for event in events if event[0].startswith('drop-')),
    )

expected = exercise(ordinary)
observed = exercise(function)
assert observed == expected, (observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_await_retires_each_operand_once
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import operand_model
import ordinary_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(operand_model, 'augmented_' + 'attribute')
ordinary = getattr(ordinary_operand_model, 'augmented_' + 'attribute')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

def exercise(function):
    events = []
    live = [0, 0]

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    class Value:
        def __init__(self):
            live[0] += 1
        def __iadd__(self, other):
            events.append(('add', other, context()))
            return 'updated'
        def __del__(self):
            live[0] -= 1
            events.append(('drop-value', context()))

    class Target:
        def __init__(self):
            live[1] += 1
        @property
        def value(self):
            events.append(('get', context()))
            return Value()
        @value.setter
        def value(self, value):
            assert value == 'updated'
            events.append(('set', value, context()))
        def __getitem__(self, key):
            assert key == 0
            return self.value
        def __setitem__(self, key, value):
            assert key == 0
            self.value = value
        def __del__(self):
            live[1] -= 1
            events.append(('drop-target', context()))

    class Wait:
        def __await__(self):
            events.append(('wait', context()))
            yield 'ready'
            if 'fail' == 'fail':
                raise LookupError('await failed')
            return 4

    try:
        raise KeyError('caller')
    except KeyError as caller:
        coroutine = function(Value if 'attribute' == 'name' else Target, Wait())
        assert coroutine.send(None) == 'ready'
        assert live == [1, int('attribute' != 'name')]
        if 'fail' == 'close':
            coroutine.close()
            events.append(('closed', context()))
        else:
            try:
                coroutine.send(None)
            except StopIteration as done:
                assert 'fail' == 'complete'
                assert done.value == ('updated' if 'attribute' == 'name' else None)
                events.append(('complete', done.value, context()))
            except LookupError as failure:
                assert 'fail' == 'fail' and str(failure) == 'await failed'
                failure.__traceback__ = None
                events.append(('failed', context()))
            else:
                raise AssertionError('coroutine unexpectedly suspended twice')
        assert sys.exception() is caller
        del coroutine
    gc.collect()
    assert live == [0, 0], (events, live)
    return (
        [event for event in events if not event[0].startswith('drop-')],
        sorted(event[0] for event in events if event[0].startswith('drop-')),
    )

expected = exercise(ordinary)
observed = exercise(function)
assert observed == expected, (observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_await_retires_each_operand_once
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import operand_model
import ordinary_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(operand_model, 'augmented_' + 'attribute')
ordinary = getattr(ordinary_operand_model, 'augmented_' + 'attribute')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

def exercise(function):
    events = []
    live = [0, 0]

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    class Value:
        def __init__(self):
            live[0] += 1
        def __iadd__(self, other):
            events.append(('add', other, context()))
            return 'updated'
        def __del__(self):
            live[0] -= 1
            events.append(('drop-value', context()))

    class Target:
        def __init__(self):
            live[1] += 1
        @property
        def value(self):
            events.append(('get', context()))
            return Value()
        @value.setter
        def value(self, value):
            assert value == 'updated'
            events.append(('set', value, context()))
        def __getitem__(self, key):
            assert key == 0
            return self.value
        def __setitem__(self, key, value):
            assert key == 0
            self.value = value
        def __del__(self):
            live[1] -= 1
            events.append(('drop-target', context()))

    class Wait:
        def __await__(self):
            events.append(('wait', context()))
            yield 'ready'
            if 'close' == 'fail':
                raise LookupError('await failed')
            return 4

    try:
        raise KeyError('caller')
    except KeyError as caller:
        coroutine = function(Value if 'attribute' == 'name' else Target, Wait())
        assert coroutine.send(None) == 'ready'
        assert live == [1, int('attribute' != 'name')]
        if 'close' == 'close':
            coroutine.close()
            events.append(('closed', context()))
        else:
            try:
                coroutine.send(None)
            except StopIteration as done:
                assert 'close' == 'complete'
                assert done.value == ('updated' if 'attribute' == 'name' else None)
                events.append(('complete', done.value, context()))
            except LookupError as failure:
                assert 'close' == 'fail' and str(failure) == 'await failed'
                failure.__traceback__ = None
                events.append(('failed', context()))
            else:
                raise AssertionError('coroutine unexpectedly suspended twice')
        assert sys.exception() is caller
        del coroutine
    gc.collect()
    assert live == [0, 0], (events, live)
    return (
        [event for event in events if not event[0].startswith('drop-')],
        sorted(event[0] for event in events if event[0].startswith('drop-')),
    )

expected = exercise(ordinary)
observed = exercise(function)
assert observed == expected, (observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_await_retires_each_operand_once
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import operand_model
import ordinary_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(operand_model, 'augmented_' + 'subscript')
ordinary = getattr(ordinary_operand_model, 'augmented_' + 'subscript')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

def exercise(function):
    events = []
    live = [0, 0]

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    class Value:
        def __init__(self):
            live[0] += 1
        def __iadd__(self, other):
            events.append(('add', other, context()))
            return 'updated'
        def __del__(self):
            live[0] -= 1
            events.append(('drop-value', context()))

    class Target:
        def __init__(self):
            live[1] += 1
        @property
        def value(self):
            events.append(('get', context()))
            return Value()
        @value.setter
        def value(self, value):
            assert value == 'updated'
            events.append(('set', value, context()))
        def __getitem__(self, key):
            assert key == 0
            return self.value
        def __setitem__(self, key, value):
            assert key == 0
            self.value = value
        def __del__(self):
            live[1] -= 1
            events.append(('drop-target', context()))

    class Wait:
        def __await__(self):
            events.append(('wait', context()))
            yield 'ready'
            if 'complete' == 'fail':
                raise LookupError('await failed')
            return 4

    try:
        raise KeyError('caller')
    except KeyError as caller:
        coroutine = function(Value if 'subscript' == 'name' else Target, Wait())
        assert coroutine.send(None) == 'ready'
        assert live == [1, int('subscript' != 'name')]
        if 'complete' == 'close':
            coroutine.close()
            events.append(('closed', context()))
        else:
            try:
                coroutine.send(None)
            except StopIteration as done:
                assert 'complete' == 'complete'
                assert done.value == ('updated' if 'subscript' == 'name' else None)
                events.append(('complete', done.value, context()))
            except LookupError as failure:
                assert 'complete' == 'fail' and str(failure) == 'await failed'
                failure.__traceback__ = None
                events.append(('failed', context()))
            else:
                raise AssertionError('coroutine unexpectedly suspended twice')
        assert sys.exception() is caller
        del coroutine
    gc.collect()
    assert live == [0, 0], (events, live)
    return (
        [event for event in events if not event[0].startswith('drop-')],
        sorted(event[0] for event in events if event[0].startswith('drop-')),
    )

expected = exercise(ordinary)
observed = exercise(function)
assert observed == expected, (observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_await_retires_each_operand_once
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import operand_model
import ordinary_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(operand_model, 'augmented_' + 'subscript')
ordinary = getattr(ordinary_operand_model, 'augmented_' + 'subscript')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

def exercise(function):
    events = []
    live = [0, 0]

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    class Value:
        def __init__(self):
            live[0] += 1
        def __iadd__(self, other):
            events.append(('add', other, context()))
            return 'updated'
        def __del__(self):
            live[0] -= 1
            events.append(('drop-value', context()))

    class Target:
        def __init__(self):
            live[1] += 1
        @property
        def value(self):
            events.append(('get', context()))
            return Value()
        @value.setter
        def value(self, value):
            assert value == 'updated'
            events.append(('set', value, context()))
        def __getitem__(self, key):
            assert key == 0
            return self.value
        def __setitem__(self, key, value):
            assert key == 0
            self.value = value
        def __del__(self):
            live[1] -= 1
            events.append(('drop-target', context()))

    class Wait:
        def __await__(self):
            events.append(('wait', context()))
            yield 'ready'
            if 'fail' == 'fail':
                raise LookupError('await failed')
            return 4

    try:
        raise KeyError('caller')
    except KeyError as caller:
        coroutine = function(Value if 'subscript' == 'name' else Target, Wait())
        assert coroutine.send(None) == 'ready'
        assert live == [1, int('subscript' != 'name')]
        if 'fail' == 'close':
            coroutine.close()
            events.append(('closed', context()))
        else:
            try:
                coroutine.send(None)
            except StopIteration as done:
                assert 'fail' == 'complete'
                assert done.value == ('updated' if 'subscript' == 'name' else None)
                events.append(('complete', done.value, context()))
            except LookupError as failure:
                assert 'fail' == 'fail' and str(failure) == 'await failed'
                failure.__traceback__ = None
                events.append(('failed', context()))
            else:
                raise AssertionError('coroutine unexpectedly suspended twice')
        assert sys.exception() is caller
        del coroutine
    gc.collect()
    assert live == [0, 0], (events, live)
    return (
        [event for event in events if not event[0].startswith('drop-')],
        sorted(event[0] for event in events if event[0].startswith('drop-')),
    )

expected = exercise(ordinary)
observed = exercise(function)
assert observed == expected, (observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_augmented_await_retires_each_operand_once
import sys
from soac import _soac_ext, import_hook

import ctypes
import gc
import sys
import operand_model
import ordinary_operand_model
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function = getattr(operand_model, 'augmented_' + 'subscript')
ordinary = getattr(ordinary_operand_model, 'augmented_' + 'subscript')
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

def exercise(function):
    events = []
    live = [0, 0]

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    class Value:
        def __init__(self):
            live[0] += 1
        def __iadd__(self, other):
            events.append(('add', other, context()))
            return 'updated'
        def __del__(self):
            live[0] -= 1
            events.append(('drop-value', context()))

    class Target:
        def __init__(self):
            live[1] += 1
        @property
        def value(self):
            events.append(('get', context()))
            return Value()
        @value.setter
        def value(self, value):
            assert value == 'updated'
            events.append(('set', value, context()))
        def __getitem__(self, key):
            assert key == 0
            return self.value
        def __setitem__(self, key, value):
            assert key == 0
            self.value = value
        def __del__(self):
            live[1] -= 1
            events.append(('drop-target', context()))

    class Wait:
        def __await__(self):
            events.append(('wait', context()))
            yield 'ready'
            if 'close' == 'fail':
                raise LookupError('await failed')
            return 4

    try:
        raise KeyError('caller')
    except KeyError as caller:
        coroutine = function(Value if 'subscript' == 'name' else Target, Wait())
        assert coroutine.send(None) == 'ready'
        assert live == [1, int('subscript' != 'name')]
        if 'close' == 'close':
            coroutine.close()
            events.append(('closed', context()))
        else:
            try:
                coroutine.send(None)
            except StopIteration as done:
                assert 'close' == 'complete'
                assert done.value == ('updated' if 'subscript' == 'name' else None)
                events.append(('complete', done.value, context()))
            except LookupError as failure:
                assert 'close' == 'fail' and str(failure) == 'await failed'
                failure.__traceback__ = None
                events.append(('failed', context()))
            else:
                raise AssertionError('coroutine unexpectedly suspended twice')
        assert sys.exception() is caller
        del coroutine
    gc.collect()
    assert live == [0, 0], (events, live)
    return (
        [event for event in events if not event[0].startswith('drop-')],
        sorted(event[0] for event in events if event[0].startswith('drop-')),
    )

expected = exercise(ordinary)
observed = exercise(function)
assert observed == expected, (observed, expected)
