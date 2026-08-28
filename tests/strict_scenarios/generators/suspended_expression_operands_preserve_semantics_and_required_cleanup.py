# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:suspended_operands
# soac: module(strict_assign=true, checked_attr=true)
def suspended_call(make, consume, later):
    local = make('local')
    consume(make('operand'), (yield 'ready'), later())
    return 73

def make_suspended_call(make, consume, later):
    return suspended_call(make, consume, later)
# module:ordinary_suspended_operands
def suspended_call(make, consume, later):
    local = make('local')
    consume(make('operand'), (yield 'ready'), later())
    return 73

def make_suspended_call(make, consume, later):
    return suspended_call(make, consume, later)
# ok
# tests/test_strict_generator_protocols.py::test_suspended_expression_operands_preserve_semantics_and_required_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended_call',):
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

import ordinary_suspended_operands as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    import sys
    import types
    import weakref

    events = []
    refs = {}
    retained = []
    failure = ValueError('later operand failed')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(('drop', self.label, handled()))

    def make(label):
        value = Payload(label)
        refs[label] = weakref.ref(value)
        events.append(('make', label))
        return value

    def later():
        assert refs['operand']() is not None, 'suspended operand was released before use'
        events.append(('later', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        if CASE in ('later_error', 'retained_traceback'):
            raise failure
        return 'tail'

    def consume(value, sent, tail):
        assert value is refs['operand'](), 'resumed call changed operand identity'
        events.append(('call', value.label, sent, tail, sys.getrefcount(value)))

    generator = module.make_suspended_call(make, consume, later)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_call.__code__
    assert generator.gi_code is source_code
    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        # Only the suspended expression stack owns this value. The source
        # local has its independent activation lifetime.
        assert refs['operand']() is not None, 'yield lost its unevaluated call operand'
        events.append(('suspended', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        try:
            if CASE in ('resume', 'later_error', 'retained_traceback'):
                generator.send('sent')
            elif CASE == 'throw':
                generator.throw(failure)
            elif CASE == 'close':
                generator.close()
            else:
                del generator
                gc.collect()
        except StopIteration as complete:
            assert CASE == 'resume'
            assert complete.value == 73
            events.append(('returned', complete.value))
        except ValueError as error:
            assert CASE in ('later_error', 'retained_traceback', 'throw')
            assert error is failure
            if CASE == 'throw' and EXPECTED is None:
                source_lines = []
                traceback = error.__traceback__
                while traceback is not None:
                    if traceback.tb_frame.f_code is source_code:
                        source_lines.append(traceback.tb_lineno)
                    traceback = traceback.tb_next
                # Keep only line numbers here: retaining the traceback/frame
                # in the observer would mask the clear-time lifetime check.
                assert source_lines == [source_code.co_firstlineno + 2], (
                    'throw must attach exactly once at the suspended yield',
                    source_lines,
                )
            gc.collect()
            events.append(('raised', refs['operand']() is None, refs['local']() is not None))
            if EXPECTED is None:
                assert refs['operand']() is None, 'ordinary traceback retained an evaluated operand'
            if CASE == 'retained_traceback':
                retained.append(error)
            else:
                error.__traceback__ = None
        assert sys.exception() is caller
        if CASE != 'gc':
            del generator
        if retained:
            if EXPECTED is None:
                assert refs['local']() is not None, 'ordinary traceback must keep source locals'
            retained.pop().__traceback__ = None
        gc.collect()
        assert refs['operand']() is None
        assert refs['local']() is None
        events.append(('complete', handled()))
    assert sum(event[:2] == ('drop', 'operand') for event in events) == 1
    assert sum(event[:2] == ('drop', 'local') for event in events) == 1
    drops = [event[1] for event in events if event[0] == 'drop']
    assert sorted(drops) == ['local', 'operand'], events
    if EXPECTED is not None:
        def semantics(recorded):
            result = []
            for event in recorded:
                if event[0] == 'drop':
                    continue
                if event[0] in {'later', 'suspended', 'raised'}:
                    # Source-local lifetime and traceback retention are not
                    # SOAC observations; explicit operation order still is.
                    result.append((event[0],))
                elif event[0] == 'call':
                    result.append(event[:-1])
                else:
                    result.append(event)
            return result
        assert semantics(events) == semantics(EXPECTED), (CASE, events, EXPECTED)
    else:
        # Retain the stock-only schedule observation separately from SOAC.
        assert drops == ['operand', 'local'], events
    RESULTS.append(events)
    return RESULTS[0]

expected = exercise(ordinary, 'resume', None, 'stock')
exercise(module, 'resume', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_expression_operands_preserve_semantics_and_required_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended_call',):
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

import ordinary_suspended_operands as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    import sys
    import types
    import weakref

    events = []
    refs = {}
    retained = []
    failure = ValueError('later operand failed')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(('drop', self.label, handled()))

    def make(label):
        value = Payload(label)
        refs[label] = weakref.ref(value)
        events.append(('make', label))
        return value

    def later():
        assert refs['operand']() is not None, 'suspended operand was released before use'
        events.append(('later', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        if CASE in ('later_error', 'retained_traceback'):
            raise failure
        return 'tail'

    def consume(value, sent, tail):
        assert value is refs['operand'](), 'resumed call changed operand identity'
        events.append(('call', value.label, sent, tail, sys.getrefcount(value)))

    generator = module.make_suspended_call(make, consume, later)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_call.__code__
    assert generator.gi_code is source_code
    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        # Only the suspended expression stack owns this value. The source
        # local has its independent activation lifetime.
        assert refs['operand']() is not None, 'yield lost its unevaluated call operand'
        events.append(('suspended', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        try:
            if CASE in ('resume', 'later_error', 'retained_traceback'):
                generator.send('sent')
            elif CASE == 'throw':
                generator.throw(failure)
            elif CASE == 'close':
                generator.close()
            else:
                del generator
                gc.collect()
        except StopIteration as complete:
            assert CASE == 'resume'
            assert complete.value == 73
            events.append(('returned', complete.value))
        except ValueError as error:
            assert CASE in ('later_error', 'retained_traceback', 'throw')
            assert error is failure
            if CASE == 'throw' and EXPECTED is None:
                source_lines = []
                traceback = error.__traceback__
                while traceback is not None:
                    if traceback.tb_frame.f_code is source_code:
                        source_lines.append(traceback.tb_lineno)
                    traceback = traceback.tb_next
                # Keep only line numbers here: retaining the traceback/frame
                # in the observer would mask the clear-time lifetime check.
                assert source_lines == [source_code.co_firstlineno + 2], (
                    'throw must attach exactly once at the suspended yield',
                    source_lines,
                )
            gc.collect()
            events.append(('raised', refs['operand']() is None, refs['local']() is not None))
            if EXPECTED is None:
                assert refs['operand']() is None, 'ordinary traceback retained an evaluated operand'
            if CASE == 'retained_traceback':
                retained.append(error)
            else:
                error.__traceback__ = None
        assert sys.exception() is caller
        if CASE != 'gc':
            del generator
        if retained:
            if EXPECTED is None:
                assert refs['local']() is not None, 'ordinary traceback must keep source locals'
            retained.pop().__traceback__ = None
        gc.collect()
        assert refs['operand']() is None
        assert refs['local']() is None
        events.append(('complete', handled()))
    assert sum(event[:2] == ('drop', 'operand') for event in events) == 1
    assert sum(event[:2] == ('drop', 'local') for event in events) == 1
    drops = [event[1] for event in events if event[0] == 'drop']
    assert sorted(drops) == ['local', 'operand'], events
    if EXPECTED is not None:
        def semantics(recorded):
            result = []
            for event in recorded:
                if event[0] == 'drop':
                    continue
                if event[0] in {'later', 'suspended', 'raised'}:
                    # Source-local lifetime and traceback retention are not
                    # SOAC observations; explicit operation order still is.
                    result.append((event[0],))
                elif event[0] == 'call':
                    result.append(event[:-1])
                else:
                    result.append(event)
            return result
        assert semantics(events) == semantics(EXPECTED), (CASE, events, EXPECTED)
    else:
        # Retain the stock-only schedule observation separately from SOAC.
        assert drops == ['operand', 'local'], events
    RESULTS.append(events)
    return RESULTS[0]

expected = exercise(ordinary, 'later_error', None, 'stock')
exercise(module, 'later_error', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_expression_operands_preserve_semantics_and_required_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended_call',):
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

import ordinary_suspended_operands as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    import sys
    import types
    import weakref

    events = []
    refs = {}
    retained = []
    failure = ValueError('later operand failed')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(('drop', self.label, handled()))

    def make(label):
        value = Payload(label)
        refs[label] = weakref.ref(value)
        events.append(('make', label))
        return value

    def later():
        assert refs['operand']() is not None, 'suspended operand was released before use'
        events.append(('later', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        if CASE in ('later_error', 'retained_traceback'):
            raise failure
        return 'tail'

    def consume(value, sent, tail):
        assert value is refs['operand'](), 'resumed call changed operand identity'
        events.append(('call', value.label, sent, tail, sys.getrefcount(value)))

    generator = module.make_suspended_call(make, consume, later)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_call.__code__
    assert generator.gi_code is source_code
    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        # Only the suspended expression stack owns this value. The source
        # local has its independent activation lifetime.
        assert refs['operand']() is not None, 'yield lost its unevaluated call operand'
        events.append(('suspended', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        try:
            if CASE in ('resume', 'later_error', 'retained_traceback'):
                generator.send('sent')
            elif CASE == 'throw':
                generator.throw(failure)
            elif CASE == 'close':
                generator.close()
            else:
                del generator
                gc.collect()
        except StopIteration as complete:
            assert CASE == 'resume'
            assert complete.value == 73
            events.append(('returned', complete.value))
        except ValueError as error:
            assert CASE in ('later_error', 'retained_traceback', 'throw')
            assert error is failure
            if CASE == 'throw' and EXPECTED is None:
                source_lines = []
                traceback = error.__traceback__
                while traceback is not None:
                    if traceback.tb_frame.f_code is source_code:
                        source_lines.append(traceback.tb_lineno)
                    traceback = traceback.tb_next
                # Keep only line numbers here: retaining the traceback/frame
                # in the observer would mask the clear-time lifetime check.
                assert source_lines == [source_code.co_firstlineno + 2], (
                    'throw must attach exactly once at the suspended yield',
                    source_lines,
                )
            gc.collect()
            events.append(('raised', refs['operand']() is None, refs['local']() is not None))
            if EXPECTED is None:
                assert refs['operand']() is None, 'ordinary traceback retained an evaluated operand'
            if CASE == 'retained_traceback':
                retained.append(error)
            else:
                error.__traceback__ = None
        assert sys.exception() is caller
        if CASE != 'gc':
            del generator
        if retained:
            if EXPECTED is None:
                assert refs['local']() is not None, 'ordinary traceback must keep source locals'
            retained.pop().__traceback__ = None
        gc.collect()
        assert refs['operand']() is None
        assert refs['local']() is None
        events.append(('complete', handled()))
    assert sum(event[:2] == ('drop', 'operand') for event in events) == 1
    assert sum(event[:2] == ('drop', 'local') for event in events) == 1
    drops = [event[1] for event in events if event[0] == 'drop']
    assert sorted(drops) == ['local', 'operand'], events
    if EXPECTED is not None:
        def semantics(recorded):
            result = []
            for event in recorded:
                if event[0] == 'drop':
                    continue
                if event[0] in {'later', 'suspended', 'raised'}:
                    # Source-local lifetime and traceback retention are not
                    # SOAC observations; explicit operation order still is.
                    result.append((event[0],))
                elif event[0] == 'call':
                    result.append(event[:-1])
                else:
                    result.append(event)
            return result
        assert semantics(events) == semantics(EXPECTED), (CASE, events, EXPECTED)
    else:
        # Retain the stock-only schedule observation separately from SOAC.
        assert drops == ['operand', 'local'], events
    RESULTS.append(events)
    return RESULTS[0]

expected = exercise(ordinary, 'throw', None, 'stock')
exercise(module, 'throw', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_expression_operands_preserve_semantics_and_required_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended_call',):
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

import ordinary_suspended_operands as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    import sys
    import types
    import weakref

    events = []
    refs = {}
    retained = []
    failure = ValueError('later operand failed')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(('drop', self.label, handled()))

    def make(label):
        value = Payload(label)
        refs[label] = weakref.ref(value)
        events.append(('make', label))
        return value

    def later():
        assert refs['operand']() is not None, 'suspended operand was released before use'
        events.append(('later', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        if CASE in ('later_error', 'retained_traceback'):
            raise failure
        return 'tail'

    def consume(value, sent, tail):
        assert value is refs['operand'](), 'resumed call changed operand identity'
        events.append(('call', value.label, sent, tail, sys.getrefcount(value)))

    generator = module.make_suspended_call(make, consume, later)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_call.__code__
    assert generator.gi_code is source_code
    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        # Only the suspended expression stack owns this value. The source
        # local has its independent activation lifetime.
        assert refs['operand']() is not None, 'yield lost its unevaluated call operand'
        events.append(('suspended', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        try:
            if CASE in ('resume', 'later_error', 'retained_traceback'):
                generator.send('sent')
            elif CASE == 'throw':
                generator.throw(failure)
            elif CASE == 'close':
                generator.close()
            else:
                del generator
                gc.collect()
        except StopIteration as complete:
            assert CASE == 'resume'
            assert complete.value == 73
            events.append(('returned', complete.value))
        except ValueError as error:
            assert CASE in ('later_error', 'retained_traceback', 'throw')
            assert error is failure
            if CASE == 'throw' and EXPECTED is None:
                source_lines = []
                traceback = error.__traceback__
                while traceback is not None:
                    if traceback.tb_frame.f_code is source_code:
                        source_lines.append(traceback.tb_lineno)
                    traceback = traceback.tb_next
                # Keep only line numbers here: retaining the traceback/frame
                # in the observer would mask the clear-time lifetime check.
                assert source_lines == [source_code.co_firstlineno + 2], (
                    'throw must attach exactly once at the suspended yield',
                    source_lines,
                )
            gc.collect()
            events.append(('raised', refs['operand']() is None, refs['local']() is not None))
            if EXPECTED is None:
                assert refs['operand']() is None, 'ordinary traceback retained an evaluated operand'
            if CASE == 'retained_traceback':
                retained.append(error)
            else:
                error.__traceback__ = None
        assert sys.exception() is caller
        if CASE != 'gc':
            del generator
        if retained:
            if EXPECTED is None:
                assert refs['local']() is not None, 'ordinary traceback must keep source locals'
            retained.pop().__traceback__ = None
        gc.collect()
        assert refs['operand']() is None
        assert refs['local']() is None
        events.append(('complete', handled()))
    assert sum(event[:2] == ('drop', 'operand') for event in events) == 1
    assert sum(event[:2] == ('drop', 'local') for event in events) == 1
    drops = [event[1] for event in events if event[0] == 'drop']
    assert sorted(drops) == ['local', 'operand'], events
    if EXPECTED is not None:
        def semantics(recorded):
            result = []
            for event in recorded:
                if event[0] == 'drop':
                    continue
                if event[0] in {'later', 'suspended', 'raised'}:
                    # Source-local lifetime and traceback retention are not
                    # SOAC observations; explicit operation order still is.
                    result.append((event[0],))
                elif event[0] == 'call':
                    result.append(event[:-1])
                else:
                    result.append(event)
            return result
        assert semantics(events) == semantics(EXPECTED), (CASE, events, EXPECTED)
    else:
        # Retain the stock-only schedule observation separately from SOAC.
        assert drops == ['operand', 'local'], events
    RESULTS.append(events)
    return RESULTS[0]

expected = exercise(ordinary, 'close', None, 'stock')
exercise(module, 'close', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_expression_operands_preserve_semantics_and_required_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended_call',):
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

import ordinary_suspended_operands as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    import sys
    import types
    import weakref

    events = []
    refs = {}
    retained = []
    failure = ValueError('later operand failed')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(('drop', self.label, handled()))

    def make(label):
        value = Payload(label)
        refs[label] = weakref.ref(value)
        events.append(('make', label))
        return value

    def later():
        assert refs['operand']() is not None, 'suspended operand was released before use'
        events.append(('later', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        if CASE in ('later_error', 'retained_traceback'):
            raise failure
        return 'tail'

    def consume(value, sent, tail):
        assert value is refs['operand'](), 'resumed call changed operand identity'
        events.append(('call', value.label, sent, tail, sys.getrefcount(value)))

    generator = module.make_suspended_call(make, consume, later)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_call.__code__
    assert generator.gi_code is source_code
    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        # Only the suspended expression stack owns this value. The source
        # local has its independent activation lifetime.
        assert refs['operand']() is not None, 'yield lost its unevaluated call operand'
        events.append(('suspended', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        try:
            if CASE in ('resume', 'later_error', 'retained_traceback'):
                generator.send('sent')
            elif CASE == 'throw':
                generator.throw(failure)
            elif CASE == 'close':
                generator.close()
            else:
                del generator
                gc.collect()
        except StopIteration as complete:
            assert CASE == 'resume'
            assert complete.value == 73
            events.append(('returned', complete.value))
        except ValueError as error:
            assert CASE in ('later_error', 'retained_traceback', 'throw')
            assert error is failure
            if CASE == 'throw' and EXPECTED is None:
                source_lines = []
                traceback = error.__traceback__
                while traceback is not None:
                    if traceback.tb_frame.f_code is source_code:
                        source_lines.append(traceback.tb_lineno)
                    traceback = traceback.tb_next
                # Keep only line numbers here: retaining the traceback/frame
                # in the observer would mask the clear-time lifetime check.
                assert source_lines == [source_code.co_firstlineno + 2], (
                    'throw must attach exactly once at the suspended yield',
                    source_lines,
                )
            gc.collect()
            events.append(('raised', refs['operand']() is None, refs['local']() is not None))
            if EXPECTED is None:
                assert refs['operand']() is None, 'ordinary traceback retained an evaluated operand'
            if CASE == 'retained_traceback':
                retained.append(error)
            else:
                error.__traceback__ = None
        assert sys.exception() is caller
        if CASE != 'gc':
            del generator
        if retained:
            if EXPECTED is None:
                assert refs['local']() is not None, 'ordinary traceback must keep source locals'
            retained.pop().__traceback__ = None
        gc.collect()
        assert refs['operand']() is None
        assert refs['local']() is None
        events.append(('complete', handled()))
    assert sum(event[:2] == ('drop', 'operand') for event in events) == 1
    assert sum(event[:2] == ('drop', 'local') for event in events) == 1
    drops = [event[1] for event in events if event[0] == 'drop']
    assert sorted(drops) == ['local', 'operand'], events
    if EXPECTED is not None:
        def semantics(recorded):
            result = []
            for event in recorded:
                if event[0] == 'drop':
                    continue
                if event[0] in {'later', 'suspended', 'raised'}:
                    # Source-local lifetime and traceback retention are not
                    # SOAC observations; explicit operation order still is.
                    result.append((event[0],))
                elif event[0] == 'call':
                    result.append(event[:-1])
                else:
                    result.append(event)
            return result
        assert semantics(events) == semantics(EXPECTED), (CASE, events, EXPECTED)
    else:
        # Retain the stock-only schedule observation separately from SOAC.
        assert drops == ['operand', 'local'], events
    RESULTS.append(events)
    return RESULTS[0]

expected = exercise(ordinary, 'gc', None, 'stock')
exercise(module, 'gc', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_expression_operands_preserve_semantics_and_required_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended_call',):
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

import ordinary_suspended_operands as ordinary

def exercise(module, CASE, EXPECTED, mode):
    RESULTS = []
    __dp_integration_mode__ = mode
    __dp_integration_soac__ = mode in ('soac', 'entry')
    __dp_integration_entry__ = mode == 'entry'
    __dp_integration_strict__ = mode != 'stock'
    import gc
    import sys
    import types
    import weakref

    events = []
    refs = {}
    retained = []
    failure = ValueError('later operand failed')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(('drop', self.label, handled()))

    def make(label):
        value = Payload(label)
        refs[label] = weakref.ref(value)
        events.append(('make', label))
        return value

    def later():
        assert refs['operand']() is not None, 'suspended operand was released before use'
        events.append(('later', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        if CASE in ('later_error', 'retained_traceback'):
            raise failure
        return 'tail'

    def consume(value, sent, tail):
        assert value is refs['operand'](), 'resumed call changed operand identity'
        events.append(('call', value.label, sent, tail, sys.getrefcount(value)))

    generator = module.make_suspended_call(make, consume, later)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_call.__code__
    assert generator.gi_code is source_code
    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        # Only the suspended expression stack owns this value. The source
        # local has its independent activation lifetime.
        assert refs['operand']() is not None, 'yield lost its unevaluated call operand'
        events.append(('suspended', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        try:
            if CASE in ('resume', 'later_error', 'retained_traceback'):
                generator.send('sent')
            elif CASE == 'throw':
                generator.throw(failure)
            elif CASE == 'close':
                generator.close()
            else:
                del generator
                gc.collect()
        except StopIteration as complete:
            assert CASE == 'resume'
            assert complete.value == 73
            events.append(('returned', complete.value))
        except ValueError as error:
            assert CASE in ('later_error', 'retained_traceback', 'throw')
            assert error is failure
            if CASE == 'throw' and EXPECTED is None:
                source_lines = []
                traceback = error.__traceback__
                while traceback is not None:
                    if traceback.tb_frame.f_code is source_code:
                        source_lines.append(traceback.tb_lineno)
                    traceback = traceback.tb_next
                # Keep only line numbers here: retaining the traceback/frame
                # in the observer would mask the clear-time lifetime check.
                assert source_lines == [source_code.co_firstlineno + 2], (
                    'throw must attach exactly once at the suspended yield',
                    source_lines,
                )
            gc.collect()
            events.append(('raised', refs['operand']() is None, refs['local']() is not None))
            if EXPECTED is None:
                assert refs['operand']() is None, 'ordinary traceback retained an evaluated operand'
            if CASE == 'retained_traceback':
                retained.append(error)
            else:
                error.__traceback__ = None
        assert sys.exception() is caller
        if CASE != 'gc':
            del generator
        if retained:
            if EXPECTED is None:
                assert refs['local']() is not None, 'ordinary traceback must keep source locals'
            retained.pop().__traceback__ = None
        gc.collect()
        assert refs['operand']() is None
        assert refs['local']() is None
        events.append(('complete', handled()))
    assert sum(event[:2] == ('drop', 'operand') for event in events) == 1
    assert sum(event[:2] == ('drop', 'local') for event in events) == 1
    drops = [event[1] for event in events if event[0] == 'drop']
    assert sorted(drops) == ['local', 'operand'], events
    if EXPECTED is not None:
        def semantics(recorded):
            result = []
            for event in recorded:
                if event[0] == 'drop':
                    continue
                if event[0] in {'later', 'suspended', 'raised'}:
                    # Source-local lifetime and traceback retention are not
                    # SOAC observations; explicit operation order still is.
                    result.append((event[0],))
                elif event[0] == 'call':
                    result.append(event[:-1])
                else:
                    result.append(event)
            return result
        assert semantics(events) == semantics(EXPECTED), (CASE, events, EXPECTED)
    else:
        # Retain the stock-only schedule observation separately from SOAC.
        assert drops == ['operand', 'local'], events
    RESULTS.append(events)
    return RESULTS[0]

expected = exercise(ordinary, 'retained_traceback', None, 'stock')
exercise(module, 'retained_traceback', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
