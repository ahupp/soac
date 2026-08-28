# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:operand_model
# soac: module(strict_assign=true, checked_attr=true)

def captured_receiver(first, second, make):
    first().value = make()

def chained_receivers(first, second, make):
    first().value = second().value = make()
# module:ordinary_operand_model
def captured_receiver(first, second, make):
    first().value = make()

def chained_receivers(first, second, make):
    first().value = second().value = make()
# ok
# tests/test_strict_function_boundaries.py::test_attribute_assignment_captured_receivers_preserve_identity_and_cleanup
import sys
from soac import _soac_ext, import_hook

import ctypes
import operand_model as actual
import ordinary_operand_model as ordinary
from soac import _soac_ext
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert _soac_ext.strict_module_diagnostics(actual)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary) is None

def observe_captured_attribute_assignment(function, case, outcome, *, native_schedule=False):
    import gc
    import sys
    import weakref

    events = []
    values = []
    targets = []
    caller = KeyError('caller handler')
    failure = LookupError('assignment failed')
    failing_receiver = 'second' if case == 'chained_receivers' else 'first'

    def context():
        current = sys.exception()
        if current is caller:
            return 'caller'
        if current is failure:
            return 'assignment-error'
        return None if current is None else type(current).__name__

    def alive():
        return (values[0]() is not None, tuple(reference() is not None for reference in targets))

    class Payload:
        def __del__(self):
            events.append(('drop-value', context()))

    class Target:
        def __init__(self, label):
            object.__setattr__(self, 'label', label)

        def __setattr__(self, name, value):
            assert name == 'value'
            assert values[0]() is value
            assert any(reference() is self for reference in targets)
            self_count = sys.getrefcount(self) if native_schedule else None
            value_count = sys.getrefcount(value) if native_schedule else None
            events.append(('set', self.label, self_count, value_count, context()))
            if outcome == 'setter-error' and self.label == failing_receiver:
                raise failure

        def __del__(self):
            events.append(('drop-target', self.label, context()))

    def make():
        value = Payload()
        values.append(weakref.ref(value))
        events.append(('made', context()))
        return value

    def receiver(label):
        events.append(('receiver', label, context()))
        if outcome == 'receiver-error' and label == failing_receiver:
            raise failure
        value = Target(label)
        targets.append(weakref.ref(value))
        return value

    def first():
        return receiver('first')

    def second():
        return receiver('second')

    try:
        raise caller
    except KeyError:
        try:
            result = function(first, second, make)
        except LookupError as caught:
            assert outcome != 'success' and caught is failure
            assert caught.__context__ is caller
            events.append(('error', context(), alive()))
            # The Python setter's own traceback legitimately retains self and
            # value. Remove it at the same explicit point in both executions.
            caught.__traceback__ = None
            events.append(('traceback-cleared', context(), alive()))
        else:
            assert outcome == 'success' and result is None
            events.append(('returned', context(), alive()))
        assert sys.exception() is caller
        events.append(('after-call', context(), alive()))
    gc.collect()
    events.append(('after-handler', context(), alive()))
    return events

def captured_attribute_assignment_semantics(events):
    final = events[-1]
    assert final[:2] == ('after-handler', None), events
    assert final[2][0] is False and not any(final[2][1]), events
    assert sum(event[0] == 'drop-value' for event in events) == 1, events
    target_drops = [event[1] for event in events if event[0] == 'drop-target']
    assert len(target_drops) == len(set(target_drops)) == len(final[2][1]), events
    semantic = []
    for event in events:
        if event[0].startswith('drop-'):
            continue
        if event[0] == 'set':
            semantic.append((event[0], event[1], event[-1]))
        elif event[0] in {'returned', 'error', 'traceback-cleared', 'after-call', 'after-handler'}:
            semantic.append(event[:2])
        else:
            semantic.append(event)
    return semantic, sorted(target_drops)

mismatches = []
for case in ('captured_receiver', 'chained_receivers'):
    function = getattr(actual, case)
    control = getattr(ordinary, case)
    assert owner(function) and not owner(control)
    assert _soac_ext.strict_function_entry_kind(function) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
    for outcome in ('success', 'receiver-error', 'setter-error'):
        expected = observe_captured_attribute_assignment(control, case, outcome)
        observed = observe_captured_attribute_assignment(function, case, outcome)
        if captured_attribute_assignment_semantics(observed) != captured_attribute_assignment_semantics(expected):
            mismatches.append((case, outcome, observed, expected))
assert not mismatches, mismatches
