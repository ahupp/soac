# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:traceback_lifetime
# soac: module(strict_assign=true, checked_attr=true)
def lifetime_function(mode, make_payload, save, delegate):
    payload = make_payload('first')
    if mode == 'escape':
        raise ValueError('source failure')
    try:
        raise LookupError('retained source failure')
    except LookupError as error:
        save(error)
    if mode == 'replace':
        payload = make_payload('second')
    elif mode == 'delete':
        del payload
    return 41

def lifetime_generator(mode, make_payload, save, delegate):
    yield 'ready'
    payload = make_payload('first')
    if mode == 'escape':
        raise ValueError('source failure')
    try:
        raise LookupError('retained source failure')
    except LookupError as error:
        save(error)
    if mode == 'replace':
        payload = make_payload('second')
    elif mode == 'delete':
        del payload
    return 41

async def lifetime_coroutine(mode, make_payload, save, delegate):
    await delegate
    payload = make_payload('first')
    if mode == 'escape':
        raise ValueError('source failure')
    try:
        raise LookupError('retained source failure')
    except LookupError as error:
        save(error)
    if mode == 'replace':
        payload = make_payload('second')
    elif mode == 'delete':
        del payload
    return 41

async def lifetime_async_generator(mode, make_payload, save, delegate):
    yield 'ready'
    payload = make_payload('first')
    if mode == 'escape':
        raise ValueError('source failure')
    try:
        raise LookupError('retained source failure')
    except LookupError as error:
        save(error)
    if mode == 'replace':
        payload = make_payload('second')
    elif mode == 'delete':
        del payload

def make_lifetime(kind, mode, make_payload, save, delegate):
    if kind == 'function':
        return lifetime_function(mode, make_payload, save, delegate)
    if kind == 'generator':
        return lifetime_generator(mode, make_payload, save, delegate)
    if kind == 'coroutine':
        return lifetime_coroutine(mode, make_payload, save, delegate)
    return lifetime_async_generator(mode, make_payload, save, delegate)
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    KIND = 'function'
    CASE = 'escape'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    KIND = 'function'
    CASE = 'retain'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    KIND = 'function'
    CASE = 'replace'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    KIND = 'function'
    CASE = 'delete'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    KIND = 'generator'
    CASE = 'escape'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    KIND = 'generator'
    CASE = 'retain'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    KIND = 'generator'
    CASE = 'replace'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    KIND = 'generator'
    CASE = 'delete'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    CASE = 'escape'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    CASE = 'retain'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    CASE = 'replace'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    CASE = 'delete'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    CASE = 'escape'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    CASE = 'retain'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    CASE = 'replace'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_lifetime',):
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
    CASE = 'delete'
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()

validate(module)

_assert_source_function_witnesses()
