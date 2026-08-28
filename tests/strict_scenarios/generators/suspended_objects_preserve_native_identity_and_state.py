# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:suspended_native
# soac: module(strict_assign=true, checked_attr=true)
async def source_coroutine(delegate, observe):
    observe('enter')
    try:
        result = await delegate
        observe('after-await')
        return result
    finally:
        observe('finally')

async def source_async_generator(delegate, observe):
    observe('enter')
    try:
        result = await delegate
        observe('after-await')
        yield result
    finally:
        observe('finally')

def make_suspended(kind, delegate, observe):
    if kind == 'coroutine':
        return source_coroutine(delegate, observe)
    return source_async_generator(delegate, observe)
# ok
# tests/test_strict_generator_protocols.py::test_suspended_objects_preserve_native_identity_and_state
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended',):
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

def validate(module):
    KIND = 'coroutine'
    CASE = 'identity'
    NATIVE = False
    import types

    events = []
    holder = []

    def observe(label):
        value = holder[0]
        events.append((label, value.cr_running if KIND == 'coroutine' else value.ag_running))

    class Delegate:
        def __await__(self):
            yield 'waiting'
            return 'source-value'

    value = module.make_suspended(KIND, Delegate(), observe)
    holder.append(value)

    def finish(awaitable):
        iterator = awaitable.__await__()
        try:
            next(iterator)
        except StopIteration as complete:
            return complete.value
        else:
            raise AssertionError('cleanup unexpectedly suspended')

    def live_frame():
        frame = value.cr_frame if KIND == 'coroutine' else value.ag_frame
        assert isinstance(frame, types.FrameType), 'a live frame must not be fabricated as None'
        expected = module.source_coroutine if KIND == 'coroutine' else module.source_async_generator
        assert frame.f_code is expected.__code__
        assert frame.f_generator is value

    try:
        if CASE == 'identity':
            expected = types.CoroutineType if KIND == 'coroutine' else types.AsyncGeneratorType
            assert type(value) is expected, (type(value), expected)
            return

        if KIND == 'coroutine':
            assert value.cr_running is False
            if CASE == 'state':
                if NATIVE:
                    live_frame()
                assert value.send(None) == 'waiting'
                assert events == [('enter', True)], events
                assert value.cr_running is False
                assert value.cr_suspended is True
                assert value.cr_await is not None
                if NATIVE:
                    live_frame()
                try:
                    value.send(None)
                except StopIteration as complete:
                    assert complete.value == 'source-value'
                else:
                    raise AssertionError('coroutine did not complete')
                assert events == [('enter', True), ('after-await', True), ('finally', True)], events
                if NATIVE:
                    assert value.cr_frame is None
                assert value.cr_await is None
            else:
                async def await_same():
                    return await value
                first, second = await_same(), await_same()
                try:
                    assert first.send(None) == 'waiting'
                    try:
                        second.send(None)
                    except RuntimeError as error:
                        assert str(error) == 'coroutine is being awaited already'
                    else:
                        raise AssertionError('concurrent await was accepted')
                finally:
                    first.close()
                    second.close()
        else:
            assert value.ag_running is False
            if CASE == 'state' and NATIVE:
                live_frame()
            first = value.__anext__()
            try:
                assert first.send(None) == 'waiting'
                if CASE == 'state':
                    assert events == [('enter', True)], events
                    # The native ASend operation owns running state across an await.
                    assert value.ag_running is True
                    assert value.ag_await is not None
                    if NATIVE:
                        live_frame()
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
                    else:
                        raise AssertionError('async yield was not delivered')
                    assert events == [('enter', True), ('after-await', True)], events
                    assert value.ag_running is False
                    assert value.ag_await is None
                else:
                    second = value.__anext__()
                    try:
                        try:
                            second.send(None)
                        except RuntimeError as error:
                            assert str(error) == 'anext(): asynchronous generator is already running'
                        else:
                            raise AssertionError('concurrent async-generator operation was accepted')
                    finally:
                        second.close()
                    # Finish the first operation before closing the generator.
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
            finally:
                first.close()
            assert finish(value.aclose()) is None
            assert value.ag_running is False
            if NATIVE:
                assert value.ag_frame is None
            assert value.ag_await is None
    finally:
        if KIND == 'coroutine':
            value.close()
        else:
            finish(value.aclose())

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_objects_preserve_native_identity_and_state
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended',):
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

def validate(module):
    KIND = 'coroutine'
    CASE = 'state'
    NATIVE = False
    import types

    events = []
    holder = []

    def observe(label):
        value = holder[0]
        events.append((label, value.cr_running if KIND == 'coroutine' else value.ag_running))

    class Delegate:
        def __await__(self):
            yield 'waiting'
            return 'source-value'

    value = module.make_suspended(KIND, Delegate(), observe)
    holder.append(value)

    def finish(awaitable):
        iterator = awaitable.__await__()
        try:
            next(iterator)
        except StopIteration as complete:
            return complete.value
        else:
            raise AssertionError('cleanup unexpectedly suspended')

    def live_frame():
        frame = value.cr_frame if KIND == 'coroutine' else value.ag_frame
        assert isinstance(frame, types.FrameType), 'a live frame must not be fabricated as None'
        expected = module.source_coroutine if KIND == 'coroutine' else module.source_async_generator
        assert frame.f_code is expected.__code__
        assert frame.f_generator is value

    try:
        if CASE == 'identity':
            expected = types.CoroutineType if KIND == 'coroutine' else types.AsyncGeneratorType
            assert type(value) is expected, (type(value), expected)
            return

        if KIND == 'coroutine':
            assert value.cr_running is False
            if CASE == 'state':
                if NATIVE:
                    live_frame()
                assert value.send(None) == 'waiting'
                assert events == [('enter', True)], events
                assert value.cr_running is False
                assert value.cr_suspended is True
                assert value.cr_await is not None
                if NATIVE:
                    live_frame()
                try:
                    value.send(None)
                except StopIteration as complete:
                    assert complete.value == 'source-value'
                else:
                    raise AssertionError('coroutine did not complete')
                assert events == [('enter', True), ('after-await', True), ('finally', True)], events
                if NATIVE:
                    assert value.cr_frame is None
                assert value.cr_await is None
            else:
                async def await_same():
                    return await value
                first, second = await_same(), await_same()
                try:
                    assert first.send(None) == 'waiting'
                    try:
                        second.send(None)
                    except RuntimeError as error:
                        assert str(error) == 'coroutine is being awaited already'
                    else:
                        raise AssertionError('concurrent await was accepted')
                finally:
                    first.close()
                    second.close()
        else:
            assert value.ag_running is False
            if CASE == 'state' and NATIVE:
                live_frame()
            first = value.__anext__()
            try:
                assert first.send(None) == 'waiting'
                if CASE == 'state':
                    assert events == [('enter', True)], events
                    # The native ASend operation owns running state across an await.
                    assert value.ag_running is True
                    assert value.ag_await is not None
                    if NATIVE:
                        live_frame()
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
                    else:
                        raise AssertionError('async yield was not delivered')
                    assert events == [('enter', True), ('after-await', True)], events
                    assert value.ag_running is False
                    assert value.ag_await is None
                else:
                    second = value.__anext__()
                    try:
                        try:
                            second.send(None)
                        except RuntimeError as error:
                            assert str(error) == 'anext(): asynchronous generator is already running'
                        else:
                            raise AssertionError('concurrent async-generator operation was accepted')
                    finally:
                        second.close()
                    # Finish the first operation before closing the generator.
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
            finally:
                first.close()
            assert finish(value.aclose()) is None
            assert value.ag_running is False
            if NATIVE:
                assert value.ag_frame is None
            assert value.ag_await is None
    finally:
        if KIND == 'coroutine':
            value.close()
        else:
            finish(value.aclose())

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_objects_preserve_native_identity_and_state
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended',):
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

def validate(module):
    KIND = 'coroutine'
    CASE = 'concurrent'
    NATIVE = False
    import types

    events = []
    holder = []

    def observe(label):
        value = holder[0]
        events.append((label, value.cr_running if KIND == 'coroutine' else value.ag_running))

    class Delegate:
        def __await__(self):
            yield 'waiting'
            return 'source-value'

    value = module.make_suspended(KIND, Delegate(), observe)
    holder.append(value)

    def finish(awaitable):
        iterator = awaitable.__await__()
        try:
            next(iterator)
        except StopIteration as complete:
            return complete.value
        else:
            raise AssertionError('cleanup unexpectedly suspended')

    def live_frame():
        frame = value.cr_frame if KIND == 'coroutine' else value.ag_frame
        assert isinstance(frame, types.FrameType), 'a live frame must not be fabricated as None'
        expected = module.source_coroutine if KIND == 'coroutine' else module.source_async_generator
        assert frame.f_code is expected.__code__
        assert frame.f_generator is value

    try:
        if CASE == 'identity':
            expected = types.CoroutineType if KIND == 'coroutine' else types.AsyncGeneratorType
            assert type(value) is expected, (type(value), expected)
            return

        if KIND == 'coroutine':
            assert value.cr_running is False
            if CASE == 'state':
                if NATIVE:
                    live_frame()
                assert value.send(None) == 'waiting'
                assert events == [('enter', True)], events
                assert value.cr_running is False
                assert value.cr_suspended is True
                assert value.cr_await is not None
                if NATIVE:
                    live_frame()
                try:
                    value.send(None)
                except StopIteration as complete:
                    assert complete.value == 'source-value'
                else:
                    raise AssertionError('coroutine did not complete')
                assert events == [('enter', True), ('after-await', True), ('finally', True)], events
                if NATIVE:
                    assert value.cr_frame is None
                assert value.cr_await is None
            else:
                async def await_same():
                    return await value
                first, second = await_same(), await_same()
                try:
                    assert first.send(None) == 'waiting'
                    try:
                        second.send(None)
                    except RuntimeError as error:
                        assert str(error) == 'coroutine is being awaited already'
                    else:
                        raise AssertionError('concurrent await was accepted')
                finally:
                    first.close()
                    second.close()
        else:
            assert value.ag_running is False
            if CASE == 'state' and NATIVE:
                live_frame()
            first = value.__anext__()
            try:
                assert first.send(None) == 'waiting'
                if CASE == 'state':
                    assert events == [('enter', True)], events
                    # The native ASend operation owns running state across an await.
                    assert value.ag_running is True
                    assert value.ag_await is not None
                    if NATIVE:
                        live_frame()
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
                    else:
                        raise AssertionError('async yield was not delivered')
                    assert events == [('enter', True), ('after-await', True)], events
                    assert value.ag_running is False
                    assert value.ag_await is None
                else:
                    second = value.__anext__()
                    try:
                        try:
                            second.send(None)
                        except RuntimeError as error:
                            assert str(error) == 'anext(): asynchronous generator is already running'
                        else:
                            raise AssertionError('concurrent async-generator operation was accepted')
                    finally:
                        second.close()
                    # Finish the first operation before closing the generator.
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
            finally:
                first.close()
            assert finish(value.aclose()) is None
            assert value.ag_running is False
            if NATIVE:
                assert value.ag_frame is None
            assert value.ag_await is None
    finally:
        if KIND == 'coroutine':
            value.close()
        else:
            finish(value.aclose())

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_objects_preserve_native_identity_and_state
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended',):
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

def validate(module):
    KIND = 'async_generator'
    CASE = 'identity'
    NATIVE = False
    import types

    events = []
    holder = []

    def observe(label):
        value = holder[0]
        events.append((label, value.cr_running if KIND == 'coroutine' else value.ag_running))

    class Delegate:
        def __await__(self):
            yield 'waiting'
            return 'source-value'

    value = module.make_suspended(KIND, Delegate(), observe)
    holder.append(value)

    def finish(awaitable):
        iterator = awaitable.__await__()
        try:
            next(iterator)
        except StopIteration as complete:
            return complete.value
        else:
            raise AssertionError('cleanup unexpectedly suspended')

    def live_frame():
        frame = value.cr_frame if KIND == 'coroutine' else value.ag_frame
        assert isinstance(frame, types.FrameType), 'a live frame must not be fabricated as None'
        expected = module.source_coroutine if KIND == 'coroutine' else module.source_async_generator
        assert frame.f_code is expected.__code__
        assert frame.f_generator is value

    try:
        if CASE == 'identity':
            expected = types.CoroutineType if KIND == 'coroutine' else types.AsyncGeneratorType
            assert type(value) is expected, (type(value), expected)
            return

        if KIND == 'coroutine':
            assert value.cr_running is False
            if CASE == 'state':
                if NATIVE:
                    live_frame()
                assert value.send(None) == 'waiting'
                assert events == [('enter', True)], events
                assert value.cr_running is False
                assert value.cr_suspended is True
                assert value.cr_await is not None
                if NATIVE:
                    live_frame()
                try:
                    value.send(None)
                except StopIteration as complete:
                    assert complete.value == 'source-value'
                else:
                    raise AssertionError('coroutine did not complete')
                assert events == [('enter', True), ('after-await', True), ('finally', True)], events
                if NATIVE:
                    assert value.cr_frame is None
                assert value.cr_await is None
            else:
                async def await_same():
                    return await value
                first, second = await_same(), await_same()
                try:
                    assert first.send(None) == 'waiting'
                    try:
                        second.send(None)
                    except RuntimeError as error:
                        assert str(error) == 'coroutine is being awaited already'
                    else:
                        raise AssertionError('concurrent await was accepted')
                finally:
                    first.close()
                    second.close()
        else:
            assert value.ag_running is False
            if CASE == 'state' and NATIVE:
                live_frame()
            first = value.__anext__()
            try:
                assert first.send(None) == 'waiting'
                if CASE == 'state':
                    assert events == [('enter', True)], events
                    # The native ASend operation owns running state across an await.
                    assert value.ag_running is True
                    assert value.ag_await is not None
                    if NATIVE:
                        live_frame()
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
                    else:
                        raise AssertionError('async yield was not delivered')
                    assert events == [('enter', True), ('after-await', True)], events
                    assert value.ag_running is False
                    assert value.ag_await is None
                else:
                    second = value.__anext__()
                    try:
                        try:
                            second.send(None)
                        except RuntimeError as error:
                            assert str(error) == 'anext(): asynchronous generator is already running'
                        else:
                            raise AssertionError('concurrent async-generator operation was accepted')
                    finally:
                        second.close()
                    # Finish the first operation before closing the generator.
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
            finally:
                first.close()
            assert finish(value.aclose()) is None
            assert value.ag_running is False
            if NATIVE:
                assert value.ag_frame is None
            assert value.ag_await is None
    finally:
        if KIND == 'coroutine':
            value.close()
        else:
            finish(value.aclose())

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_objects_preserve_native_identity_and_state
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended',):
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

def validate(module):
    KIND = 'async_generator'
    CASE = 'state'
    NATIVE = False
    import types

    events = []
    holder = []

    def observe(label):
        value = holder[0]
        events.append((label, value.cr_running if KIND == 'coroutine' else value.ag_running))

    class Delegate:
        def __await__(self):
            yield 'waiting'
            return 'source-value'

    value = module.make_suspended(KIND, Delegate(), observe)
    holder.append(value)

    def finish(awaitable):
        iterator = awaitable.__await__()
        try:
            next(iterator)
        except StopIteration as complete:
            return complete.value
        else:
            raise AssertionError('cleanup unexpectedly suspended')

    def live_frame():
        frame = value.cr_frame if KIND == 'coroutine' else value.ag_frame
        assert isinstance(frame, types.FrameType), 'a live frame must not be fabricated as None'
        expected = module.source_coroutine if KIND == 'coroutine' else module.source_async_generator
        assert frame.f_code is expected.__code__
        assert frame.f_generator is value

    try:
        if CASE == 'identity':
            expected = types.CoroutineType if KIND == 'coroutine' else types.AsyncGeneratorType
            assert type(value) is expected, (type(value), expected)
            return

        if KIND == 'coroutine':
            assert value.cr_running is False
            if CASE == 'state':
                if NATIVE:
                    live_frame()
                assert value.send(None) == 'waiting'
                assert events == [('enter', True)], events
                assert value.cr_running is False
                assert value.cr_suspended is True
                assert value.cr_await is not None
                if NATIVE:
                    live_frame()
                try:
                    value.send(None)
                except StopIteration as complete:
                    assert complete.value == 'source-value'
                else:
                    raise AssertionError('coroutine did not complete')
                assert events == [('enter', True), ('after-await', True), ('finally', True)], events
                if NATIVE:
                    assert value.cr_frame is None
                assert value.cr_await is None
            else:
                async def await_same():
                    return await value
                first, second = await_same(), await_same()
                try:
                    assert first.send(None) == 'waiting'
                    try:
                        second.send(None)
                    except RuntimeError as error:
                        assert str(error) == 'coroutine is being awaited already'
                    else:
                        raise AssertionError('concurrent await was accepted')
                finally:
                    first.close()
                    second.close()
        else:
            assert value.ag_running is False
            if CASE == 'state' and NATIVE:
                live_frame()
            first = value.__anext__()
            try:
                assert first.send(None) == 'waiting'
                if CASE == 'state':
                    assert events == [('enter', True)], events
                    # The native ASend operation owns running state across an await.
                    assert value.ag_running is True
                    assert value.ag_await is not None
                    if NATIVE:
                        live_frame()
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
                    else:
                        raise AssertionError('async yield was not delivered')
                    assert events == [('enter', True), ('after-await', True)], events
                    assert value.ag_running is False
                    assert value.ag_await is None
                else:
                    second = value.__anext__()
                    try:
                        try:
                            second.send(None)
                        except RuntimeError as error:
                            assert str(error) == 'anext(): asynchronous generator is already running'
                        else:
                            raise AssertionError('concurrent async-generator operation was accepted')
                    finally:
                        second.close()
                    # Finish the first operation before closing the generator.
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
            finally:
                first.close()
            assert finish(value.aclose()) is None
            assert value.ag_running is False
            if NATIVE:
                assert value.ag_frame is None
            assert value.ag_await is None
    finally:
        if KIND == 'coroutine':
            value.close()
        else:
            finish(value.aclose())

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_suspended_objects_preserve_native_identity_and_state
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_suspended',):
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

def validate(module):
    KIND = 'async_generator'
    CASE = 'concurrent'
    NATIVE = False
    import types

    events = []
    holder = []

    def observe(label):
        value = holder[0]
        events.append((label, value.cr_running if KIND == 'coroutine' else value.ag_running))

    class Delegate:
        def __await__(self):
            yield 'waiting'
            return 'source-value'

    value = module.make_suspended(KIND, Delegate(), observe)
    holder.append(value)

    def finish(awaitable):
        iterator = awaitable.__await__()
        try:
            next(iterator)
        except StopIteration as complete:
            return complete.value
        else:
            raise AssertionError('cleanup unexpectedly suspended')

    def live_frame():
        frame = value.cr_frame if KIND == 'coroutine' else value.ag_frame
        assert isinstance(frame, types.FrameType), 'a live frame must not be fabricated as None'
        expected = module.source_coroutine if KIND == 'coroutine' else module.source_async_generator
        assert frame.f_code is expected.__code__
        assert frame.f_generator is value

    try:
        if CASE == 'identity':
            expected = types.CoroutineType if KIND == 'coroutine' else types.AsyncGeneratorType
            assert type(value) is expected, (type(value), expected)
            return

        if KIND == 'coroutine':
            assert value.cr_running is False
            if CASE == 'state':
                if NATIVE:
                    live_frame()
                assert value.send(None) == 'waiting'
                assert events == [('enter', True)], events
                assert value.cr_running is False
                assert value.cr_suspended is True
                assert value.cr_await is not None
                if NATIVE:
                    live_frame()
                try:
                    value.send(None)
                except StopIteration as complete:
                    assert complete.value == 'source-value'
                else:
                    raise AssertionError('coroutine did not complete')
                assert events == [('enter', True), ('after-await', True), ('finally', True)], events
                if NATIVE:
                    assert value.cr_frame is None
                assert value.cr_await is None
            else:
                async def await_same():
                    return await value
                first, second = await_same(), await_same()
                try:
                    assert first.send(None) == 'waiting'
                    try:
                        second.send(None)
                    except RuntimeError as error:
                        assert str(error) == 'coroutine is being awaited already'
                    else:
                        raise AssertionError('concurrent await was accepted')
                finally:
                    first.close()
                    second.close()
        else:
            assert value.ag_running is False
            if CASE == 'state' and NATIVE:
                live_frame()
            first = value.__anext__()
            try:
                assert first.send(None) == 'waiting'
                if CASE == 'state':
                    assert events == [('enter', True)], events
                    # The native ASend operation owns running state across an await.
                    assert value.ag_running is True
                    assert value.ag_await is not None
                    if NATIVE:
                        live_frame()
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
                    else:
                        raise AssertionError('async yield was not delivered')
                    assert events == [('enter', True), ('after-await', True)], events
                    assert value.ag_running is False
                    assert value.ag_await is None
                else:
                    second = value.__anext__()
                    try:
                        try:
                            second.send(None)
                        except RuntimeError as error:
                            assert str(error) == 'anext(): asynchronous generator is already running'
                        else:
                            raise AssertionError('concurrent async-generator operation was accepted')
                    finally:
                        second.close()
                    # Finish the first operation before closing the generator.
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
            finally:
                first.close()
            assert finish(value.aclose()) is None
            assert value.ag_running is False
            if NATIVE:
                assert value.ag_frame is None
            assert value.ag_await is None
    finally:
        if KIND == 'coroutine':
            value.close()
        else:
            finish(value.aclose())

validate(module)

_assert_source_function_witnesses()
