# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:operand_model
# soac: module(strict_assign=true, checked_attr=true)

def attribute_assignment(target, make):
    target.value = make()
# module:ordinary_operand_model
def attribute_assignment(target, make):
    target.value = make()
# ok
# tests/test_strict_function_boundaries.py::test_attribute_assignment_preserves_callbacks_and_cleans_up
import sys
from soac import _soac_ext, import_hook

import ctypes
import operand_model as actual
import ordinary_operand_model as ordinary
from soac import _soac_ext
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert owner(actual.attribute_assignment)
assert not owner(ordinary.attribute_assignment)
assert _soac_ext.strict_module_diagnostics(actual)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert _soac_ext.strict_function_entry_kind(actual.attribute_assignment) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

def observe_attribute_assignment(function, outcome, *, native_schedule=False):
    import gc
    import sys
    import weakref

    events = []
    values = []
    caller = KeyError('caller handler')
    failure = LookupError('setter failed')

    def context():
        current = sys.exception()
        if current is caller:
            return 'caller'
        if current is failure:
            return 'setter-error'
        return None if current is None else type(current).__name__

    class Payload:
        def __del__(self):
            events.append(('drop-value', context()))

    class Target:
        def __setattr__(self, name, value):
            assert name == 'value'
            assert values[0]() is value
            count = sys.getrefcount(value) if native_schedule else None
            events.append(('set', count, context()))
            if outcome == 'error':
                raise failure

        def __del__(self):
            events.append(('drop-target', context()))

    def make():
        value = Payload()
        values.append(weakref.ref(value))
        events.append(('made', context()))
        return value

    try:
        raise caller
    except KeyError:
        try:
            result = function(Target(), make)
        except LookupError as caught:
            assert outcome == 'error' and caught is failure
            assert caught.__context__ is caller
            events.append(('error', context(), values[0]() is not None))
            caught.__traceback__ = None
            events.append(('traceback-cleared', context(), values[0]() is not None))
        else:
            assert outcome == 'success' and result is None
            events.append(('returned', context(), values[0]() is not None))
        assert sys.exception() is caller
        events.append(('after-call', context(), values[0]() is not None))
    gc.collect()
    events.append(('after-handler', context(), values[0]() is not None))
    return events

def attribute_assignment_semantics(events):
    labels = [event[0] for event in events]
    assert labels.count('drop-value') == labels.count('drop-target') == 1, events
    assert events[-1] == ('after-handler', None, False), events
    return [
        (event[0], event[-1]) if event[0] == 'set' else event[:2]
        for event in events if not event[0].startswith('drop-')
    ]

expected = observe_attribute_assignment(ordinary.attribute_assignment, 'success')
observed = observe_attribute_assignment(actual.attribute_assignment, 'success')
assert attribute_assignment_semantics(observed) == attribute_assignment_semantics(expected), (observed, expected)
# ok
# tests/test_strict_function_boundaries.py::test_attribute_assignment_preserves_callbacks_and_cleans_up
import sys
from soac import _soac_ext, import_hook

import ctypes
import operand_model as actual
import ordinary_operand_model as ordinary
from soac import _soac_ext
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert owner(actual.attribute_assignment)
assert not owner(ordinary.attribute_assignment)
assert _soac_ext.strict_module_diagnostics(actual)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert _soac_ext.strict_function_entry_kind(actual.attribute_assignment) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

def observe_attribute_assignment(function, outcome, *, native_schedule=False):
    import gc
    import sys
    import weakref

    events = []
    values = []
    caller = KeyError('caller handler')
    failure = LookupError('setter failed')

    def context():
        current = sys.exception()
        if current is caller:
            return 'caller'
        if current is failure:
            return 'setter-error'
        return None if current is None else type(current).__name__

    class Payload:
        def __del__(self):
            events.append(('drop-value', context()))

    class Target:
        def __setattr__(self, name, value):
            assert name == 'value'
            assert values[0]() is value
            count = sys.getrefcount(value) if native_schedule else None
            events.append(('set', count, context()))
            if outcome == 'error':
                raise failure

        def __del__(self):
            events.append(('drop-target', context()))

    def make():
        value = Payload()
        values.append(weakref.ref(value))
        events.append(('made', context()))
        return value

    try:
        raise caller
    except KeyError:
        try:
            result = function(Target(), make)
        except LookupError as caught:
            assert outcome == 'error' and caught is failure
            assert caught.__context__ is caller
            events.append(('error', context(), values[0]() is not None))
            caught.__traceback__ = None
            events.append(('traceback-cleared', context(), values[0]() is not None))
        else:
            assert outcome == 'success' and result is None
            events.append(('returned', context(), values[0]() is not None))
        assert sys.exception() is caller
        events.append(('after-call', context(), values[0]() is not None))
    gc.collect()
    events.append(('after-handler', context(), values[0]() is not None))
    return events

def attribute_assignment_semantics(events):
    labels = [event[0] for event in events]
    assert labels.count('drop-value') == labels.count('drop-target') == 1, events
    assert events[-1] == ('after-handler', None, False), events
    return [
        (event[0], event[-1]) if event[0] == 'set' else event[:2]
        for event in events if not event[0].startswith('drop-')
    ]

expected = observe_attribute_assignment(ordinary.attribute_assignment, 'error')
observed = observe_attribute_assignment(actual.attribute_assignment, 'error')
assert attribute_assignment_semantics(observed) == attribute_assignment_semantics(expected), (observed, expected)
