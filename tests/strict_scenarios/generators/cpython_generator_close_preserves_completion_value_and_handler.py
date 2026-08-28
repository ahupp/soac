# modes:cpython
# Authenticated source and independent ordinary validation blocks.
# module:generator_protocol
# soac: module(strict_assign=true, checked_attr=true)
def make_delegate(delegate, observe):
    def values():
        try:
            raise KeyError('source handler')
        except KeyError:
            try:
                return (yield from delegate)
            finally:
                observe('finally')
    return values()

def make_plain(observe):
    def values():
        try:
            raise KeyError('source handler')
        except KeyError:
            try:
                observe('start')
                value = yield 'ready'
                observe('sent', value)
                yield 'again'
            except GeneratorExit:
                observe('close')
                return 73
            except ValueError as error:
                observe('caught', error.args)
                yield 'caught'
            finally:
                observe('finally')
    return values()

def injection_plain(observe):
    try:
        yield 'ready'
    except BaseException as error:
        observe(error)

def injection_handled(observe):
    try:
        raise KeyError('source handler')
    except KeyError:
        try:
            yield 'ready'
        except BaseException as error:
            observe(error)

def injection_delegated(delegate, observe):
    try:
        yield from delegate
    except BaseException as error:
        observe(error)

def make_injection(mode, delegate, observe):
    if mode == 'handled_throw':
        return injection_handled(observe)
    if mode == 'delegate_error':
        return injection_delegated(delegate, observe)
    return injection_plain(observe)
# module:ordinary_generator_protocol
def make_delegate(delegate, observe):
    def values():
        try:
            raise KeyError('source handler')
        except KeyError:
            try:
                return (yield from delegate)
            finally:
                observe('finally')
    return values()

def make_plain(observe):
    def values():
        try:
            raise KeyError('source handler')
        except KeyError:
            try:
                observe('start')
                value = yield 'ready'
                observe('sent', value)
                yield 'again'
            except GeneratorExit:
                observe('close')
                return 73
            except ValueError as error:
                observe('caught', error.args)
                yield 'caught'
            finally:
                observe('finally')
    return values()

def injection_plain(observe):
    try:
        yield 'ready'
    except BaseException as error:
        observe(error)

def injection_handled(observe):
    try:
        raise KeyError('source handler')
    except KeyError:
        try:
            yield 'ready'
        except BaseException as error:
            observe(error)

def injection_delegated(delegate, observe):
    try:
        yield from delegate
    except BaseException as error:
        observe(error)

def make_injection(mode, delegate, observe):
    if mode == 'handled_throw':
        return injection_handled(observe)
    if mode == 'delegate_error':
        return injection_delegated(delegate, observe)
    return injection_plain(observe)
# ok
# tests/test_strict_generator_protocols.py::test_cpython_generator_close_preserves_completion_value_and_handler
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_delegate', 'make_plain'):
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

import ordinary_generator_protocol as ordinary

def exercise(module, CASE, EXPECTED):
    RESULTS = []
    import sys
    import warnings

    events = []
    generator = None
    reentry_attempted = False

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    def observe(label, value=None):
        nonlocal reentry_attempted
        events.append((label, value, handled()))
        if CASE == 'reentrant_send' and label == 'start':
            assert not reentry_attempted, 'reentry executed the body before its guard'
            reentry_attempted = True
            try:
                generator.send(None)
            except ValueError as error:
                assert str(error) == 'generator already executing'
                events.append(('reentrant-rejected',))
            else:
                raise AssertionError('an executing generator accepted another resume')

    class Thrown(ValueError):
        def __init__(self, *args):
            observe('exception-constructor', args)
            super().__init__(*args)

    class Delegate:
        def __iter__(self):
            return self

        def __next__(self):
            observe('delegate-next')
            return 'delegated-ready'

        @property
        def throw(self):
            observe('delegate-lookup')
            if CASE == 'throw_lookup_error':
                raise LookupError('throw lookup')
            if CASE == 'missing_throw_normalizes_after_lookup':
                raise AttributeError('throw')

            def invoke(*args):
                first = args[0]
                shape = (
                    first.__name__ if isinstance(first, type) else first,
                    args[1:],
                )
                observe('delegate-throw', shape)
                if CASE == 'raw_throw_completes_delegate':
                    raise StopIteration(('delegate result', 42))
                return 'delegated-throw'

            return invoke

        def close(self):
            observe('delegate-close')

    def capture(call):
        try:
            result = call()
        except BaseException as error:
            events.append(('error', type(error).__name__, error.args))
        else:
            events.append(('result', result))

    try:
        raise RuntimeError('caller handler')
    except RuntimeError as caller:
        if CASE in ('close_return_value', 'invalid_throw_keeps_suspended', 'created_throw', 'created_invalid_throw', 'reentrant_send'):
            generator = module.make_plain(observe)
            if not CASE.startswith('created_'):
                assert next(generator) == 'ready'
        else:
            generator = module.make_delegate(Delegate(), observe)
            assert next(generator) == 'delegated-ready'
        assert sys.exception() is caller

        with warnings.catch_warnings():
            warnings.simplefilter('ignore', DeprecationWarning)
            if CASE.startswith('raw_throw_'):
                capture(lambda: generator.throw(Thrown, 'payload', None))
            elif CASE in ('throw_lookup_error', 'missing_throw_normalizes_after_lookup'):
                capture(lambda: generator.throw(Thrown))
            elif CASE == 'delegate_receives_invalid_exception_type':
                capture(lambda: generator.throw(17))
            elif CASE.startswith('close_'):
                capture(generator.close)
            elif CASE in ('invalid_throw_keeps_suspended', 'created_invalid_throw'):
                capture(lambda: generator.throw(17))
            elif CASE == 'created_throw':
                capture(lambda: generator.throw(Thrown))
            elif CASE == 'reentrant_send':
                capture(lambda: generator.send(11))
            else:
                raise AssertionError(CASE)
        assert sys.exception() is caller

        if CASE in ('throw_lookup_error', 'invalid_throw_keeps_suspended', 'created_invalid_throw'):
            capture(lambda: generator.send(None))
        assert sys.exception() is caller
        capture(generator.close)
        assert sys.exception() is caller

    # Keep a full native parity assertion, including lookup/construction order,
    # raw deprecated throw arguments, finally callbacks, and completion values.
    if EXPECTED is not None:
        assert events == EXPECTED, (CASE, events, EXPECTED)
    RESULTS.append(events)
    return RESULTS[0]

expected = exercise(ordinary, 'close_return_value', None)
assert ('result', 73) in expected, expected
exercise(module, 'close_return_value', expected)

_assert_source_function_witnesses()
