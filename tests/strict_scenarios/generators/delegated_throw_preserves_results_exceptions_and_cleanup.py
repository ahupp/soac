# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:delegated_throw
# soac: module(strict_assign=true, checked_attr=true)
def suspended_delegation(make, values):
    local = make()
    return (yield from values)

def make_suspended_delegation(make, values):
    return suspended_delegation(make, values)
# module:ordinary_delegated_throw
def suspended_delegation(make, values):
    local = make()
    return (yield from values)

def make_suspended_delegation(make, values):
    return suspended_delegation(make, values)
# ok
# tests/test_strict_generator_protocols.py::test_delegated_throw_preserves_results_exceptions_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended_delegation',):
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

import ordinary_delegated_throw as ordinary

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

    drops = []
    refs = []
    failure = ValueError('delegated throw')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __del__(self):
            drops.append(handled())

    def make():
        value = Payload()
        refs.append(weakref.ref(value))
        return value

    def raising_delegate():
        yield 'ready'

    def returning_delegate():
        try:
            yield 'ready'
        except ValueError:
            return 73

    if CASE == 'missing_throw':
        delegate = iter(('ready',))
    elif CASE == 'delegate_raises':
        delegate = raising_delegate()
    else:
        delegate = returning_delegate()
    generator = module.make_suspended_delegation(make, delegate)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_delegation.__code__
    assert generator.gi_code is source_code

    def source_lines(error):
        lines = []
        traceback = error.__traceback__
        while traceback is not None:
            if traceback.tb_frame.f_code is source_code:
                lines.append(traceback.tb_lineno - source_code.co_firstlineno)
            traceback = traceback.tb_next
        return lines

    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        if EXPECTED is None:
            assert refs[0]() is not None
        assert generator.gi_yieldfrom is delegate
        try:
            generator.throw(failure)
        except ValueError as error:
            assert CASE != 'delegate_returns'
            assert error is failure
            if EXPECTED is None:
                lines = source_lines(error)
                assert lines == [2], ('ordinary delegated error needs one original source TB', lines)
                assert refs[0]() is not None, 'ordinary traceback must retain the source local'
                assert drops == [], drops
            error.__traceback__ = None
            gc.collect()
            if EXPECTED is None:
                assert refs[0]() is None, 'the last traceback must release its source local'
                assert len(drops) == 1, drops
                assert drops == [('ValueError', ('delegated throw',))], drops
            result = ('raised', type(error).__name__, error.args)
        except StopIteration as complete:
            assert CASE == 'delegate_returns'
            assert complete.value == 73
            if EXPECTED is None:
                lines = source_lines(failure)
                assert lines == [], ('consumed ordinary delegation error must not gain a source TB', lines)
            result = ('returned', complete.value)
            # A retained delegate traceback can own its ordinary f_back.
            # This control distinguishes source TB events, not that separate
            # callback-parent lifetime. Release the actual traceback explicitly.
            failure.__traceback__ = None
        else:
            raise AssertionError('throw must terminate this source activation')
        assert sys.exception() is caller
        del generator
        del delegate
        gc.collect()
        assert refs[0]() is None
        assert len(drops) == 1, drops
    if EXPECTED is not None:
        assert result == EXPECTED, (CASE, result, EXPECTED)
    RESULTS.append(result)
    return RESULTS[0]

expected = exercise(ordinary, 'missing_throw', None, 'stock')
exercise(module, 'missing_throw', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_delegated_throw_preserves_results_exceptions_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended_delegation',):
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

import ordinary_delegated_throw as ordinary

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

    drops = []
    refs = []
    failure = ValueError('delegated throw')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __del__(self):
            drops.append(handled())

    def make():
        value = Payload()
        refs.append(weakref.ref(value))
        return value

    def raising_delegate():
        yield 'ready'

    def returning_delegate():
        try:
            yield 'ready'
        except ValueError:
            return 73

    if CASE == 'missing_throw':
        delegate = iter(('ready',))
    elif CASE == 'delegate_raises':
        delegate = raising_delegate()
    else:
        delegate = returning_delegate()
    generator = module.make_suspended_delegation(make, delegate)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_delegation.__code__
    assert generator.gi_code is source_code

    def source_lines(error):
        lines = []
        traceback = error.__traceback__
        while traceback is not None:
            if traceback.tb_frame.f_code is source_code:
                lines.append(traceback.tb_lineno - source_code.co_firstlineno)
            traceback = traceback.tb_next
        return lines

    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        if EXPECTED is None:
            assert refs[0]() is not None
        assert generator.gi_yieldfrom is delegate
        try:
            generator.throw(failure)
        except ValueError as error:
            assert CASE != 'delegate_returns'
            assert error is failure
            if EXPECTED is None:
                lines = source_lines(error)
                assert lines == [2], ('ordinary delegated error needs one original source TB', lines)
                assert refs[0]() is not None, 'ordinary traceback must retain the source local'
                assert drops == [], drops
            error.__traceback__ = None
            gc.collect()
            if EXPECTED is None:
                assert refs[0]() is None, 'the last traceback must release its source local'
                assert len(drops) == 1, drops
                assert drops == [('ValueError', ('delegated throw',))], drops
            result = ('raised', type(error).__name__, error.args)
        except StopIteration as complete:
            assert CASE == 'delegate_returns'
            assert complete.value == 73
            if EXPECTED is None:
                lines = source_lines(failure)
                assert lines == [], ('consumed ordinary delegation error must not gain a source TB', lines)
            result = ('returned', complete.value)
            # A retained delegate traceback can own its ordinary f_back.
            # This control distinguishes source TB events, not that separate
            # callback-parent lifetime. Release the actual traceback explicitly.
            failure.__traceback__ = None
        else:
            raise AssertionError('throw must terminate this source activation')
        assert sys.exception() is caller
        del generator
        del delegate
        gc.collect()
        assert refs[0]() is None
        assert len(drops) == 1, drops
    if EXPECTED is not None:
        assert result == EXPECTED, (CASE, result, EXPECTED)
    RESULTS.append(result)
    return RESULTS[0]

expected = exercise(ordinary, 'delegate_raises', None, 'stock')
exercise(module, 'delegate_raises', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_delegated_throw_preserves_results_exceptions_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended_delegation',):
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

import ordinary_delegated_throw as ordinary

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

    drops = []
    refs = []
    failure = ValueError('delegated throw')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __del__(self):
            drops.append(handled())

    def make():
        value = Payload()
        refs.append(weakref.ref(value))
        return value

    def raising_delegate():
        yield 'ready'

    def returning_delegate():
        try:
            yield 'ready'
        except ValueError:
            return 73

    if CASE == 'missing_throw':
        delegate = iter(('ready',))
    elif CASE == 'delegate_raises':
        delegate = raising_delegate()
    else:
        delegate = returning_delegate()
    generator = module.make_suspended_delegation(make, delegate)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_delegation.__code__
    assert generator.gi_code is source_code

    def source_lines(error):
        lines = []
        traceback = error.__traceback__
        while traceback is not None:
            if traceback.tb_frame.f_code is source_code:
                lines.append(traceback.tb_lineno - source_code.co_firstlineno)
            traceback = traceback.tb_next
        return lines

    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        if EXPECTED is None:
            assert refs[0]() is not None
        assert generator.gi_yieldfrom is delegate
        try:
            generator.throw(failure)
        except ValueError as error:
            assert CASE != 'delegate_returns'
            assert error is failure
            if EXPECTED is None:
                lines = source_lines(error)
                assert lines == [2], ('ordinary delegated error needs one original source TB', lines)
                assert refs[0]() is not None, 'ordinary traceback must retain the source local'
                assert drops == [], drops
            error.__traceback__ = None
            gc.collect()
            if EXPECTED is None:
                assert refs[0]() is None, 'the last traceback must release its source local'
                assert len(drops) == 1, drops
                assert drops == [('ValueError', ('delegated throw',))], drops
            result = ('raised', type(error).__name__, error.args)
        except StopIteration as complete:
            assert CASE == 'delegate_returns'
            assert complete.value == 73
            if EXPECTED is None:
                lines = source_lines(failure)
                assert lines == [], ('consumed ordinary delegation error must not gain a source TB', lines)
            result = ('returned', complete.value)
            # A retained delegate traceback can own its ordinary f_back.
            # This control distinguishes source TB events, not that separate
            # callback-parent lifetime. Release the actual traceback explicitly.
            failure.__traceback__ = None
        else:
            raise AssertionError('throw must terminate this source activation')
        assert sys.exception() is caller
        del generator
        del delegate
        gc.collect()
        assert refs[0]() is None
        assert len(drops) == 1, drops
    if EXPECTED is not None:
        assert result == EXPECTED, (CASE, result, EXPECTED)
    RESULTS.append(result)
    return RESULTS[0]

expected = exercise(ordinary, 'delegate_returns', None, 'stock')
exercise(module, 'delegate_returns', expected, __dp_integration_mode__)

_assert_source_function_witnesses()
