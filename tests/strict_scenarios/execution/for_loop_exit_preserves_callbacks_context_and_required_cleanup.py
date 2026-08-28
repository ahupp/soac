# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:loop_exit
# soac: module(strict_assign=true, checked_attr=true)

def exhaust(make, observe, target):
    for value in make():
        observe('body')
    else:
        observe('else')
    observe('after')

def break_loop(make, observe, target):
    for value in make():
        observe('body')
        break
    else:
        observe('unexpected-else')
    observe('after')

def return_loop(make, observe, target):
    for value in make():
        return observe('return-value')

def failing_target(make, observe, target):
    try:
        for target.value in make():
            observe('unexpected-body')
    except ValueError:
        observe('caught-outer')
    observe('after')

def failing_body(make, observe, target):
    try:
        for value in make():
            observe('body')
            raise ValueError('body error')
    except ValueError:
        observe('caught-outer')
    observe('after')

def caught_continue(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            observe('caught-inner')
            continue
    observe('after')

def caught_break(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            observe('caught-inner')
            break
    observe('after')

def caught_return(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            return observe('return-value')

def failing_finally(make, observe, target):
    try:
        for value in make():
            try:
                raise ValueError('body error')
            finally:
                observe('finally')
    except ValueError:
        observe('caught-outer')
    observe('after')
# module:ordinary_loop_exit
def exhaust(make, observe, target):
    for value in make():
        observe('body')
    else:
        observe('else')
    observe('after')

def break_loop(make, observe, target):
    for value in make():
        observe('body')
        break
    else:
        observe('unexpected-else')
    observe('after')

def return_loop(make, observe, target):
    for value in make():
        return observe('return-value')

def failing_target(make, observe, target):
    try:
        for target.value in make():
            observe('unexpected-body')
    except ValueError:
        observe('caught-outer')
    observe('after')

def failing_body(make, observe, target):
    try:
        for value in make():
            observe('body')
            raise ValueError('body error')
    except ValueError:
        observe('caught-outer')
    observe('after')

def caught_continue(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            observe('caught-inner')
            continue
    observe('after')

def caught_break(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            observe('caught-inner')
            break
    observe('after')

def caught_return(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            return observe('return-value')

def failing_finally(make, observe, target):
    try:
        for value in make():
            try:
                raise ValueError('body error')
            finally:
                observe('finally')
    except ValueError:
        observe('caught-outer')
    observe('after')
# ok
# tests/test_strict_entry_runtime.py::test_for_loop_exit_preserves_callbacks_context_and_required_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('exhaust', 'break_loop', 'return_loop', 'failing_target', 'failing_body', 'caught_continue', 'caught_break', 'caught_return', 'failing_finally'):
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

import ordinary_loop_exit as ordinary

def _for_loop_exit_observations(module, case):
    """Observe a temporary iterator without adding a lasting receiver owner."""
    import gc
    import sys
    import weakref

    events = []
    reference = None

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    def observe(label):
        events.append((label, reference() is not None, context()))
        return "return-token"

    class Iterator:
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

        def __del__(self):
            events.append(("drop", context()))

    def make():
        nonlocal reference
        iterator = Iterator()
        reference = weakref.ref(iterator)
        return iterator

    class Target:
        def __setattr__(self, name, value):
            observe("target")
            raise ValueError("target error")

    # Explicit source callbacks inherit the real handled-exception context.
    # An implicit finalizer's timing/context is recorded separately below.
    try:
        raise KeyError("caller")
    except KeyError:
        result = getattr(module, case)(make, observe, Target())
        observe("caller")
    gc.collect()
    assert reference() is None, ('completed loop retained its iterator', case, events)
    assert sum(event[0] == 'drop' for event in events) == 1, (case, events)
    return {"events": events, "result": result}

def _for_loop_exit_semantics(observed):
    """Only explicit callback order/context and the computed result are parity."""
    return {
        "events": [
            (event[0], event[2]) for event in observed["events"] if event[0] != "drop"
        ],
        "result": observed["result"],
    }

expected = {
    case: _for_loop_exit_semantics(_for_loop_exit_observations(ordinary, case))
    for case in ('exhaust', 'break_loop', 'return_loop', 'failing_target', 'failing_body', 'caught_continue', 'caught_break', 'caught_return', 'failing_finally')
}
actual = {
    case: _for_loop_exit_semantics(_for_loop_exit_observations(module, case))
    for case in expected
}
import json
print(json.dumps({'actual': actual, 'expected': expected}))
failures = {case: actual[case] for case in expected if actual[case] != expected[case]}
assert not failures, (failures, expected)

_assert_source_function_witnesses()
