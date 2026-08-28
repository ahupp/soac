# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:generator_terminal
# soac: module(strict_assign=true, checked_attr=true)
def terminal_values(mode, make_payload):
    payload = make_payload()
    yield 'ready'
    if payload is None:
        raise AssertionError('source local was not initialized')
    if mode == 'raise':
        raise LookupError('source failure')
    return 71

def make_terminal(mode, make_payload):
    return terminal_values(mode, make_payload)
# module:ordinary_generator_terminal
def terminal_values(mode, make_payload):
    payload = make_payload()
    yield 'ready'
    if payload is None:
        raise AssertionError('source local was not initialized')
    if mode == 'raise':
        raise LookupError('source failure')
    return 71

def make_terminal(mode, make_payload):
    return terminal_values(mode, make_payload)
# ok
# tests/test_strict_generator_protocols.py::test_generator_terminal_cleanup_is_reentry_safe_and_finishes_closed
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_terminal',):
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

import ordinary_generator_terminal as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    events = []
    holder = []
    injected = RuntimeError('reentrant throw')

    def observe(call):
        try:
            result = call()
        except BaseException as error:
            return ('error', type(error).__name__, error.args, error is injected)
        return ('value', result)

    class Payload:
        def __del__(self):
            generator = holder[0]
            observed = (
                observe(lambda: generator.gi_running),
                observe(lambda: generator.gi_state),
                observe(lambda: generator.gi_suspended),
            )
            if EXPECTED is None:
                # Frame inspection belongs only to the ordinary control.
                observed += (observe(lambda: generator.gi_frame is None),)
            events.append(observed + (
                observe(lambda: generator.gi_yieldfrom),
                observe(lambda: generator.send(None)),
                observe(lambda: generator.throw(injected)),
                observe(generator.close),
            ))

    generator = module.make_terminal(CASE, Payload)
    holder.append(generator)
    assert next(generator) == 'ready'
    assert events == [], 'the local must remain live while its generator is suspended'
    if CASE == 'close':
        assert generator.close() is None
    elif CASE == 'raise':
        try:
            next(generator)
        except LookupError as error:
            assert error.args == ('source failure',)
        else:
            raise AssertionError('source failure was lost')
    else:
        try:
            next(generator)
        except StopIteration as complete:
            assert complete.value == 71
        else:
            raise AssertionError('generator did not return')
    gc.collect()
    assert len(events) == 1, ('terminal local was not released', CASE, events)
    # Completion is a semantic boundary, independent of when implicit release
    # caused the reentrant finalizer to run.
    assert generator.gi_running is False
    assert generator.gi_state == 'GEN_CLOSED'
    assert generator.gi_suspended is False
    if EXPECTED is None:
        assert generator.gi_frame is None
    assert generator.gi_yieldfrom is None
    if EXPECTED is not None:
        observed = events[0]
        expected_state = EXPECTED[0][:3] + EXPECTED[0][4:]
        if observed[0] == ('value', False):
            assert observed == expected_state, (CASE, events, expected_state)
        else:
            # A safe SOAC cleanup point may precede the terminal-state flag.
            # Such reentry must refuse execution, not resume a retiring body.
            assert observed[0] == ('value', True), observed
            assert observed[1] == ('value', 'GEN_RUNNING'), observed
            assert observed[2] == ('value', False), observed
            assert observed[3] == ('value', None), observed
            for result in observed[4:]:
                assert result[0] == 'error' and result[1] == 'ValueError', observed
    RESULTS.append(events)
    holder.clear()
    return RESULTS[0]

expected = exercise(ordinary, 'return', None, 'stock')
assert expected == [(('value', False), ('value', 'GEN_CLOSED'), ('value', False), ('value', True), ('value', None), ('error', 'StopIteration', (), False), ('error', 'RuntimeError', ('reentrant throw',), True), ('value', None))], expected
exercise(module, 'return', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_generator_terminal_cleanup_is_reentry_safe_and_finishes_closed
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_terminal',):
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

import ordinary_generator_terminal as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    events = []
    holder = []
    injected = RuntimeError('reentrant throw')

    def observe(call):
        try:
            result = call()
        except BaseException as error:
            return ('error', type(error).__name__, error.args, error is injected)
        return ('value', result)

    class Payload:
        def __del__(self):
            generator = holder[0]
            observed = (
                observe(lambda: generator.gi_running),
                observe(lambda: generator.gi_state),
                observe(lambda: generator.gi_suspended),
            )
            if EXPECTED is None:
                # Frame inspection belongs only to the ordinary control.
                observed += (observe(lambda: generator.gi_frame is None),)
            events.append(observed + (
                observe(lambda: generator.gi_yieldfrom),
                observe(lambda: generator.send(None)),
                observe(lambda: generator.throw(injected)),
                observe(generator.close),
            ))

    generator = module.make_terminal(CASE, Payload)
    holder.append(generator)
    assert next(generator) == 'ready'
    assert events == [], 'the local must remain live while its generator is suspended'
    if CASE == 'close':
        assert generator.close() is None
    elif CASE == 'raise':
        try:
            next(generator)
        except LookupError as error:
            assert error.args == ('source failure',)
        else:
            raise AssertionError('source failure was lost')
    else:
        try:
            next(generator)
        except StopIteration as complete:
            assert complete.value == 71
        else:
            raise AssertionError('generator did not return')
    gc.collect()
    assert len(events) == 1, ('terminal local was not released', CASE, events)
    # Completion is a semantic boundary, independent of when implicit release
    # caused the reentrant finalizer to run.
    assert generator.gi_running is False
    assert generator.gi_state == 'GEN_CLOSED'
    assert generator.gi_suspended is False
    if EXPECTED is None:
        assert generator.gi_frame is None
    assert generator.gi_yieldfrom is None
    if EXPECTED is not None:
        observed = events[0]
        expected_state = EXPECTED[0][:3] + EXPECTED[0][4:]
        if observed[0] == ('value', False):
            assert observed == expected_state, (CASE, events, expected_state)
        else:
            # A safe SOAC cleanup point may precede the terminal-state flag.
            # Such reentry must refuse execution, not resume a retiring body.
            assert observed[0] == ('value', True), observed
            assert observed[1] == ('value', 'GEN_RUNNING'), observed
            assert observed[2] == ('value', False), observed
            assert observed[3] == ('value', None), observed
            for result in observed[4:]:
                assert result[0] == 'error' and result[1] == 'ValueError', observed
    RESULTS.append(events)
    holder.clear()
    return RESULTS[0]

expected = exercise(ordinary, 'raise', None, 'stock')
assert expected == [(('value', False), ('value', 'GEN_CLOSED'), ('value', False), ('value', True), ('value', None), ('error', 'StopIteration', (), False), ('error', 'RuntimeError', ('reentrant throw',), True), ('value', None))], expected
exercise(module, 'raise', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_generator_terminal_cleanup_is_reentry_safe_and_finishes_closed
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_terminal',):
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

import ordinary_generator_terminal as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    events = []
    holder = []
    injected = RuntimeError('reentrant throw')

    def observe(call):
        try:
            result = call()
        except BaseException as error:
            return ('error', type(error).__name__, error.args, error is injected)
        return ('value', result)

    class Payload:
        def __del__(self):
            generator = holder[0]
            observed = (
                observe(lambda: generator.gi_running),
                observe(lambda: generator.gi_state),
                observe(lambda: generator.gi_suspended),
            )
            if EXPECTED is None:
                # Frame inspection belongs only to the ordinary control.
                observed += (observe(lambda: generator.gi_frame is None),)
            events.append(observed + (
                observe(lambda: generator.gi_yieldfrom),
                observe(lambda: generator.send(None)),
                observe(lambda: generator.throw(injected)),
                observe(generator.close),
            ))

    generator = module.make_terminal(CASE, Payload)
    holder.append(generator)
    assert next(generator) == 'ready'
    assert events == [], 'the local must remain live while its generator is suspended'
    if CASE == 'close':
        assert generator.close() is None
    elif CASE == 'raise':
        try:
            next(generator)
        except LookupError as error:
            assert error.args == ('source failure',)
        else:
            raise AssertionError('source failure was lost')
    else:
        try:
            next(generator)
        except StopIteration as complete:
            assert complete.value == 71
        else:
            raise AssertionError('generator did not return')
    gc.collect()
    assert len(events) == 1, ('terminal local was not released', CASE, events)
    # Completion is a semantic boundary, independent of when implicit release
    # caused the reentrant finalizer to run.
    assert generator.gi_running is False
    assert generator.gi_state == 'GEN_CLOSED'
    assert generator.gi_suspended is False
    if EXPECTED is None:
        assert generator.gi_frame is None
    assert generator.gi_yieldfrom is None
    if EXPECTED is not None:
        observed = events[0]
        expected_state = EXPECTED[0][:3] + EXPECTED[0][4:]
        if observed[0] == ('value', False):
            assert observed == expected_state, (CASE, events, expected_state)
        else:
            # A safe SOAC cleanup point may precede the terminal-state flag.
            # Such reentry must refuse execution, not resume a retiring body.
            assert observed[0] == ('value', True), observed
            assert observed[1] == ('value', 'GEN_RUNNING'), observed
            assert observed[2] == ('value', False), observed
            assert observed[3] == ('value', None), observed
            for result in observed[4:]:
                assert result[0] == 'error' and result[1] == 'ValueError', observed
    RESULTS.append(events)
    holder.clear()
    return RESULTS[0]

expected = exercise(ordinary, 'close', None, 'stock')
assert expected == [(('value', False), ('value', 'GEN_CLOSED'), ('value', False), ('value', True), ('value', None), ('error', 'StopIteration', (), False), ('error', 'RuntimeError', ('reentrant throw',), True), ('value', None))], expected
exercise(module, 'close', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
