# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:generator_delivery
# soac: module(strict_assign=true, checked_attr=true)
def delegated_delivery(delegate_factory, events):
    try:
        result = yield from delegate_factory()
        events.append(('after-yieldfrom', result))
        return ('returned', result)
    finally:
        events.append(('finally',))

def handled_delivery(events):
    try:
        yield 'ready'
    except ValueError:
        events.append(('handled',))
    events.append(('after-handler',))
    yield 'after'

def make_delivery(case, delegate_factory, events):
    if case == 'normalized_exception_lifetime':
        return handled_delivery(events)
    return delegated_delivery(delegate_factory, events)
# module:ordinary_generator_delivery
def delegated_delivery(delegate_factory, events):
    try:
        result = yield from delegate_factory()
        events.append(('after-yieldfrom', result))
        return ('returned', result)
    finally:
        events.append(('finally',))

def handled_delivery(events):
    try:
        yield 'ready'
    except ValueError:
        events.append(('handled',))
    events.append(('after-handler',))
    yield 'after'

def make_delivery(case, delegate_factory, events):
    if case == 'normalized_exception_lifetime':
        return handled_delivery(events)
    return delegated_delivery(delegate_factory, events)
# ok
# tests/test_strict_generator_protocols.py::test_generator_delivery_preserves_completion_callbacks_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_delivery',):
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

import ordinary_generator_delivery as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    events = []

    class MissingThrow:
        def __iter__(self):
            return self

        def __next__(self):
            return 'ready'

    class CloseStops(MissingThrow):
        def close(self):
            events.append(('delegate-close',))
            raise StopIteration(41)

    class TemporaryDelegate(MissingThrow):
        def throw(self, *args):
            events.append(('delegate-throw',))
            raise StopIteration(7)

        def close(self):
            events.append(('delegate-close',))

        def __del__(self):
            events.append(('delegate-finalized',))

    class Injection(ValueError):
        def __del__(self):
            events.append(('injection-finalized',))

    if CASE in ('close_stop_iteration', 'throw_exit_close_stop_iteration'):
        delegate_factory = CloseStops
    elif CASE in ('delegate_throw_lifetime', 'delegate_close_lifetime'):
        delegate_factory = TemporaryDelegate
    else:
        delegate_factory = MissingThrow

    generator = module.make_delivery(CASE, delegate_factory, events)
    try:
        assert next(generator) == 'ready'
        if CASE in ('close_stop_iteration', 'delegate_close_lifetime'):
            outcome = ('return', generator.close())
        else:
            if CASE == 'missing_throw_stop_iteration':
                injected = StopIteration(40)
            elif CASE == 'throw_exit_close_stop_iteration':
                injected = GeneratorExit
            else:
                # Only the exception class is retained by this caller. The
                # normalized instance must be released by completed cleanup.
                injected = Injection
            try:
                value = generator.throw(injected)
            except StopIteration as complete:
                outcome = ('stop', complete.value)
            else:
                outcome = ('yield', value)
    finally:
        generator.close()
    del generator
    gc.collect()
    observed = (events.copy(), outcome)
    if EXPECTED is not None:
        implicit = {'delegate-finalized', 'injection-finalized'}
        def semantics(observation):
            recorded, result = observation
            return ([event for event in recorded if event[0] not in implicit], result)
        # Delegate calls, handler/finally order and completion values are exact.
        assert semantics(observed) == semantics(EXPECTED), (CASE, observed, EXPECTED)
        # Required finalizers must each run once, not on CPython's schedule.
        assert sorted(event for event in events if event[0] in implicit) == sorted(
            event for event in EXPECTED[0] if event[0] in implicit
        ), (CASE, observed, EXPECTED)
    RESULTS.append(observed)
    return RESULTS[0]

expected = exercise(ordinary, 'missing_throw_stop_iteration', None, 'stock')
assert expected == ([('after-yieldfrom', 40), ('finally',)], ('stop', ('returned', 40))), expected
exercise(module, 'missing_throw_stop_iteration', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_generator_delivery_preserves_completion_callbacks_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_delivery',):
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

import ordinary_generator_delivery as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    events = []

    class MissingThrow:
        def __iter__(self):
            return self

        def __next__(self):
            return 'ready'

    class CloseStops(MissingThrow):
        def close(self):
            events.append(('delegate-close',))
            raise StopIteration(41)

    class TemporaryDelegate(MissingThrow):
        def throw(self, *args):
            events.append(('delegate-throw',))
            raise StopIteration(7)

        def close(self):
            events.append(('delegate-close',))

        def __del__(self):
            events.append(('delegate-finalized',))

    class Injection(ValueError):
        def __del__(self):
            events.append(('injection-finalized',))

    if CASE in ('close_stop_iteration', 'throw_exit_close_stop_iteration'):
        delegate_factory = CloseStops
    elif CASE in ('delegate_throw_lifetime', 'delegate_close_lifetime'):
        delegate_factory = TemporaryDelegate
    else:
        delegate_factory = MissingThrow

    generator = module.make_delivery(CASE, delegate_factory, events)
    try:
        assert next(generator) == 'ready'
        if CASE in ('close_stop_iteration', 'delegate_close_lifetime'):
            outcome = ('return', generator.close())
        else:
            if CASE == 'missing_throw_stop_iteration':
                injected = StopIteration(40)
            elif CASE == 'throw_exit_close_stop_iteration':
                injected = GeneratorExit
            else:
                # Only the exception class is retained by this caller. The
                # normalized instance must be released by completed cleanup.
                injected = Injection
            try:
                value = generator.throw(injected)
            except StopIteration as complete:
                outcome = ('stop', complete.value)
            else:
                outcome = ('yield', value)
    finally:
        generator.close()
    del generator
    gc.collect()
    observed = (events.copy(), outcome)
    if EXPECTED is not None:
        implicit = {'delegate-finalized', 'injection-finalized'}
        def semantics(observation):
            recorded, result = observation
            return ([event for event in recorded if event[0] not in implicit], result)
        # Delegate calls, handler/finally order and completion values are exact.
        assert semantics(observed) == semantics(EXPECTED), (CASE, observed, EXPECTED)
        # Required finalizers must each run once, not on CPython's schedule.
        assert sorted(event for event in events if event[0] in implicit) == sorted(
            event for event in EXPECTED[0] if event[0] in implicit
        ), (CASE, observed, EXPECTED)
    RESULTS.append(observed)
    return RESULTS[0]

expected = exercise(ordinary, 'close_stop_iteration', None, 'stock')
assert expected == ([('delegate-close',), ('after-yieldfrom', 41), ('finally',)], ('return', ('returned', 41))), expected
exercise(module, 'close_stop_iteration', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_generator_delivery_preserves_completion_callbacks_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_delivery',):
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

import ordinary_generator_delivery as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    events = []

    class MissingThrow:
        def __iter__(self):
            return self

        def __next__(self):
            return 'ready'

    class CloseStops(MissingThrow):
        def close(self):
            events.append(('delegate-close',))
            raise StopIteration(41)

    class TemporaryDelegate(MissingThrow):
        def throw(self, *args):
            events.append(('delegate-throw',))
            raise StopIteration(7)

        def close(self):
            events.append(('delegate-close',))

        def __del__(self):
            events.append(('delegate-finalized',))

    class Injection(ValueError):
        def __del__(self):
            events.append(('injection-finalized',))

    if CASE in ('close_stop_iteration', 'throw_exit_close_stop_iteration'):
        delegate_factory = CloseStops
    elif CASE in ('delegate_throw_lifetime', 'delegate_close_lifetime'):
        delegate_factory = TemporaryDelegate
    else:
        delegate_factory = MissingThrow

    generator = module.make_delivery(CASE, delegate_factory, events)
    try:
        assert next(generator) == 'ready'
        if CASE in ('close_stop_iteration', 'delegate_close_lifetime'):
            outcome = ('return', generator.close())
        else:
            if CASE == 'missing_throw_stop_iteration':
                injected = StopIteration(40)
            elif CASE == 'throw_exit_close_stop_iteration':
                injected = GeneratorExit
            else:
                # Only the exception class is retained by this caller. The
                # normalized instance must be released by completed cleanup.
                injected = Injection
            try:
                value = generator.throw(injected)
            except StopIteration as complete:
                outcome = ('stop', complete.value)
            else:
                outcome = ('yield', value)
    finally:
        generator.close()
    del generator
    gc.collect()
    observed = (events.copy(), outcome)
    if EXPECTED is not None:
        implicit = {'delegate-finalized', 'injection-finalized'}
        def semantics(observation):
            recorded, result = observation
            return ([event for event in recorded if event[0] not in implicit], result)
        # Delegate calls, handler/finally order and completion values are exact.
        assert semantics(observed) == semantics(EXPECTED), (CASE, observed, EXPECTED)
        # Required finalizers must each run once, not on CPython's schedule.
        assert sorted(event for event in events if event[0] in implicit) == sorted(
            event for event in EXPECTED[0] if event[0] in implicit
        ), (CASE, observed, EXPECTED)
    RESULTS.append(observed)
    return RESULTS[0]

expected = exercise(ordinary, 'throw_exit_close_stop_iteration', None, 'stock')
assert expected == ([('delegate-close',), ('after-yieldfrom', 41), ('finally',)], ('stop', ('returned', 41))), expected
exercise(module, 'throw_exit_close_stop_iteration', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_generator_delivery_preserves_completion_callbacks_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_delivery',):
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

import ordinary_generator_delivery as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    events = []

    class MissingThrow:
        def __iter__(self):
            return self

        def __next__(self):
            return 'ready'

    class CloseStops(MissingThrow):
        def close(self):
            events.append(('delegate-close',))
            raise StopIteration(41)

    class TemporaryDelegate(MissingThrow):
        def throw(self, *args):
            events.append(('delegate-throw',))
            raise StopIteration(7)

        def close(self):
            events.append(('delegate-close',))

        def __del__(self):
            events.append(('delegate-finalized',))

    class Injection(ValueError):
        def __del__(self):
            events.append(('injection-finalized',))

    if CASE in ('close_stop_iteration', 'throw_exit_close_stop_iteration'):
        delegate_factory = CloseStops
    elif CASE in ('delegate_throw_lifetime', 'delegate_close_lifetime'):
        delegate_factory = TemporaryDelegate
    else:
        delegate_factory = MissingThrow

    generator = module.make_delivery(CASE, delegate_factory, events)
    try:
        assert next(generator) == 'ready'
        if CASE in ('close_stop_iteration', 'delegate_close_lifetime'):
            outcome = ('return', generator.close())
        else:
            if CASE == 'missing_throw_stop_iteration':
                injected = StopIteration(40)
            elif CASE == 'throw_exit_close_stop_iteration':
                injected = GeneratorExit
            else:
                # Only the exception class is retained by this caller. The
                # normalized instance must be released by completed cleanup.
                injected = Injection
            try:
                value = generator.throw(injected)
            except StopIteration as complete:
                outcome = ('stop', complete.value)
            else:
                outcome = ('yield', value)
    finally:
        generator.close()
    del generator
    gc.collect()
    observed = (events.copy(), outcome)
    if EXPECTED is not None:
        implicit = {'delegate-finalized', 'injection-finalized'}
        def semantics(observation):
            recorded, result = observation
            return ([event for event in recorded if event[0] not in implicit], result)
        # Delegate calls, handler/finally order and completion values are exact.
        assert semantics(observed) == semantics(EXPECTED), (CASE, observed, EXPECTED)
        # Required finalizers must each run once, not on CPython's schedule.
        assert sorted(event for event in events if event[0] in implicit) == sorted(
            event for event in EXPECTED[0] if event[0] in implicit
        ), (CASE, observed, EXPECTED)
    RESULTS.append(observed)
    return RESULTS[0]

expected = exercise(ordinary, 'delegate_throw_lifetime', None, 'stock')
assert expected == ([('delegate-throw',), ('delegate-finalized',), ('after-yieldfrom', 7), ('finally',)], ('stop', ('returned', 7))), expected
exercise(module, 'delegate_throw_lifetime', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_generator_delivery_preserves_completion_callbacks_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_delivery',):
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

import ordinary_generator_delivery as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    events = []

    class MissingThrow:
        def __iter__(self):
            return self

        def __next__(self):
            return 'ready'

    class CloseStops(MissingThrow):
        def close(self):
            events.append(('delegate-close',))
            raise StopIteration(41)

    class TemporaryDelegate(MissingThrow):
        def throw(self, *args):
            events.append(('delegate-throw',))
            raise StopIteration(7)

        def close(self):
            events.append(('delegate-close',))

        def __del__(self):
            events.append(('delegate-finalized',))

    class Injection(ValueError):
        def __del__(self):
            events.append(('injection-finalized',))

    if CASE in ('close_stop_iteration', 'throw_exit_close_stop_iteration'):
        delegate_factory = CloseStops
    elif CASE in ('delegate_throw_lifetime', 'delegate_close_lifetime'):
        delegate_factory = TemporaryDelegate
    else:
        delegate_factory = MissingThrow

    generator = module.make_delivery(CASE, delegate_factory, events)
    try:
        assert next(generator) == 'ready'
        if CASE in ('close_stop_iteration', 'delegate_close_lifetime'):
            outcome = ('return', generator.close())
        else:
            if CASE == 'missing_throw_stop_iteration':
                injected = StopIteration(40)
            elif CASE == 'throw_exit_close_stop_iteration':
                injected = GeneratorExit
            else:
                # Only the exception class is retained by this caller. The
                # normalized instance must be released by completed cleanup.
                injected = Injection
            try:
                value = generator.throw(injected)
            except StopIteration as complete:
                outcome = ('stop', complete.value)
            else:
                outcome = ('yield', value)
    finally:
        generator.close()
    del generator
    gc.collect()
    observed = (events.copy(), outcome)
    if EXPECTED is not None:
        implicit = {'delegate-finalized', 'injection-finalized'}
        def semantics(observation):
            recorded, result = observation
            return ([event for event in recorded if event[0] not in implicit], result)
        # Delegate calls, handler/finally order and completion values are exact.
        assert semantics(observed) == semantics(EXPECTED), (CASE, observed, EXPECTED)
        # Required finalizers must each run once, not on CPython's schedule.
        assert sorted(event for event in events if event[0] in implicit) == sorted(
            event for event in EXPECTED[0] if event[0] in implicit
        ), (CASE, observed, EXPECTED)
    RESULTS.append(observed)
    return RESULTS[0]

expected = exercise(ordinary, 'delegate_close_lifetime', None, 'stock')
assert expected == ([('delegate-close',), ('delegate-finalized',), ('finally',)], ('return', None)), expected
exercise(module, 'delegate_close_lifetime', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_generator_delivery_preserves_completion_callbacks_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_delivery',):
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

import ordinary_generator_delivery as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    events = []

    class MissingThrow:
        def __iter__(self):
            return self

        def __next__(self):
            return 'ready'

    class CloseStops(MissingThrow):
        def close(self):
            events.append(('delegate-close',))
            raise StopIteration(41)

    class TemporaryDelegate(MissingThrow):
        def throw(self, *args):
            events.append(('delegate-throw',))
            raise StopIteration(7)

        def close(self):
            events.append(('delegate-close',))

        def __del__(self):
            events.append(('delegate-finalized',))

    class Injection(ValueError):
        def __del__(self):
            events.append(('injection-finalized',))

    if CASE in ('close_stop_iteration', 'throw_exit_close_stop_iteration'):
        delegate_factory = CloseStops
    elif CASE in ('delegate_throw_lifetime', 'delegate_close_lifetime'):
        delegate_factory = TemporaryDelegate
    else:
        delegate_factory = MissingThrow

    generator = module.make_delivery(CASE, delegate_factory, events)
    try:
        assert next(generator) == 'ready'
        if CASE in ('close_stop_iteration', 'delegate_close_lifetime'):
            outcome = ('return', generator.close())
        else:
            if CASE == 'missing_throw_stop_iteration':
                injected = StopIteration(40)
            elif CASE == 'throw_exit_close_stop_iteration':
                injected = GeneratorExit
            else:
                # Only the exception class is retained by this caller. The
                # normalized instance must be released by completed cleanup.
                injected = Injection
            try:
                value = generator.throw(injected)
            except StopIteration as complete:
                outcome = ('stop', complete.value)
            else:
                outcome = ('yield', value)
    finally:
        generator.close()
    del generator
    gc.collect()
    observed = (events.copy(), outcome)
    if EXPECTED is not None:
        implicit = {'delegate-finalized', 'injection-finalized'}
        def semantics(observation):
            recorded, result = observation
            return ([event for event in recorded if event[0] not in implicit], result)
        # Delegate calls, handler/finally order and completion values are exact.
        assert semantics(observed) == semantics(EXPECTED), (CASE, observed, EXPECTED)
        # Required finalizers must each run once, not on CPython's schedule.
        assert sorted(event for event in events if event[0] in implicit) == sorted(
            event for event in EXPECTED[0] if event[0] in implicit
        ), (CASE, observed, EXPECTED)
    RESULTS.append(observed)
    return RESULTS[0]

expected = exercise(ordinary, 'normalized_exception_lifetime', None, 'stock')
assert expected == ([('handled',), ('injection-finalized',), ('after-handler',)], ('yield', 'after')), expected
exercise(module, 'normalized_exception_lifetime', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
