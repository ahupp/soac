# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:loop_receiver
# soac: module(strict_assign=true, checked_attr=true)

def exhaust(iterator, observe):
    observe('before')
    for value in iterator:
        observe('body')
    observe('after')
# module:ordinary_loop_receiver
def exhaust(iterator, observe):
    observe('before')
    for value in iterator:
        observe('body')
    observe('after')
# ok
# tests/test_strict_entry_runtime.py::test_for_loop_next_receiver_preserves_callbacks_and_safe_ownership
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('exhaust',):
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

import ordinary_loop_receiver as ordinary

def _for_loop_receiver_observations(module):
    """Keep native count diagnostics separate from callback and cleanup checks."""
    import gc
    import sys
    import weakref

    events = []
    reference = None

    def observe(label):
        receiver = reference()
        assert receiver is not None
        events.append((label, sys.getrefcount(receiver)))

    # This class and observer remain ordinary Python outside the strict project.
    # The observer holds only a weak reference between calls, and uses the same
    # temporary strong receiver reference at every measurement point.
    class ObservedIterator:
        def __init__(self):
            self.position = 0

        def __iter__(self):
            return self

        def __next__(self):
            observe("next")
            if self.position == 2:
                raise StopIteration
            self.position += 1
            return self.position

    iterator = ObservedIterator()
    reference = weakref.ref(iterator)
    assert module.exhaust(iterator, observe) is None
    assert [label for label, _ in events] == [
        "before",
        "next",
        "body",
        "next",
        "body",
        "next",
        "after",
    ]
    baseline = events[0][1]
    body_baseline = events[2][1]
    del iterator
    gc.collect()
    assert reference() is None, 'completed loop retained its iterator'
    return {
        "entry_relative": tuple(count - baseline for _, count in events),
        "next_over_body": tuple(
            count - body_baseline for label, count in events if label == "next"
        ),
        "after_over_entry": events[-1][1] - baseline,
    }, events

_, native_counts = _for_loop_receiver_observations(ordinary)
expected_labels = [label for label, _ in native_counts]
actual, counts = _for_loop_receiver_observations(module)
import json
print(json.dumps({'actual': actual, 'absolute_counts': counts, 'native_counts': native_counts}))
assert [label for label, _ in counts] == expected_labels, counts

_assert_source_function_witnesses()
