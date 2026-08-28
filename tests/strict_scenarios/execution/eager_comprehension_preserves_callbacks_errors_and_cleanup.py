# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:comprehension_source_frame
# soac: module(strict_assign=true, checked_attr=true)

def nested_builtin_positional(mapping, key):
    return [mapping.get(value) for value in (key,)][0]

def schedule(owners):
    return [owner.link for owner in owners]

def preserve_outer(make, visit):
    item = make('outer')
    [visit() for item in (make('inner'),)]
    return 'done'
# module:ordinary_comprehension_source_frame
def nested_builtin_positional(mapping, key):
    return [mapping.get(value) for value in (key,)][0]

def schedule(owners):
    return [owner.link for owner in owners]

def preserve_outer(make, visit):
    item = make('outer')
    [visit() for item in (make('inner'),)]
    return 'done'
# ok
# tests/test_strict_entry_runtime.py::test_eager_comprehension_preserves_callbacks_errors_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('nested_builtin_positional', 'schedule', 'preserve_outer'):
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

import ordinary_comprehension_source_frame as ordinary

def _eager_comprehension_frame_observations(module, exceptional=False):
    import gc
    import weakref

    class Owner:
        def __init__(self, link):
            self.link = link

    # These are the unchanged bodies that exposed the missing parent-slot
    # projection. No hand-written loop or replacement validator stands in.
    result = {
        'builtin': module.nested_builtin_positional({'key': 41}, 'key'),
        'field': module.schedule([Owner(11), Owner(31)]),
    }
    events = []
    references = {}
    errors = []
    callbacks = []
    marker = ValueError('unwind the original region')

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(self.label)

    def make(label):
        callbacks.append(('make', label))
        value = Payload(label)
        references[label] = weakref.ref(value)
        return value

    def visit():
        callbacks.append(('visit',))
        if exceptional:
            raise marker
        try:
            raise ValueError('retain the callback traceback')
        except ValueError as error:
            errors.append(error)

    try:
        if exceptional:
            try:
                module.preserve_outer(make, visit)
            except ValueError as error:
                assert error is marker
                errors.append(error)
            else:
                raise AssertionError('the unchanged callback must raise')
        else:
            assert module.preserve_outer(make, visit) == 'done'
        assert len(errors) == 1
        assert callbacks == [('make', 'outer'), ('make', 'inner'), ('visit',)]
        # Ordinary retained traceback/frame-back ownership keeps the restored
        # outer local, not the temporary target, after normal or error exit.
        # No f_locals/f_back introspection is needed to observe this lifetime.
        result['outer_before_clear'] = references['outer']() is not None
        result['inner_before_clear'] = references['inner']() is not None
        result['events_before_clear'] = events.copy()
        errors[0].__traceback__ = None
        gc.collect()
        result['outer_after_clear'] = references['outer']() is not None
        result['inner_after_clear'] = references['inner']() is not None
        result['events_after_clear'] = events.copy()
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()
    return result

def _eager_comprehension_semantics(observed):
    assert not observed['outer_after_clear'] and not observed['inner_after_clear'], observed
    assert sorted(observed['events_after_clear']) == ['inner', 'outer'], observed
    return observed['builtin'], observed['field'], sorted(observed['events_after_clear'])

expected = _eager_comprehension_semantics(_eager_comprehension_frame_observations(ordinary, False))
actual = _eager_comprehension_semantics(_eager_comprehension_frame_observations(module, False))
assert actual == expected, (actual, expected)

_assert_source_function_witnesses()
# ok
# tests/test_strict_entry_runtime.py::test_eager_comprehension_preserves_callbacks_errors_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('nested_builtin_positional', 'schedule', 'preserve_outer'):
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

import ordinary_comprehension_source_frame as ordinary

def _eager_comprehension_frame_observations(module, exceptional=False):
    import gc
    import weakref

    class Owner:
        def __init__(self, link):
            self.link = link

    # These are the unchanged bodies that exposed the missing parent-slot
    # projection. No hand-written loop or replacement validator stands in.
    result = {
        'builtin': module.nested_builtin_positional({'key': 41}, 'key'),
        'field': module.schedule([Owner(11), Owner(31)]),
    }
    events = []
    references = {}
    errors = []
    callbacks = []
    marker = ValueError('unwind the original region')

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(self.label)

    def make(label):
        callbacks.append(('make', label))
        value = Payload(label)
        references[label] = weakref.ref(value)
        return value

    def visit():
        callbacks.append(('visit',))
        if exceptional:
            raise marker
        try:
            raise ValueError('retain the callback traceback')
        except ValueError as error:
            errors.append(error)

    try:
        if exceptional:
            try:
                module.preserve_outer(make, visit)
            except ValueError as error:
                assert error is marker
                errors.append(error)
            else:
                raise AssertionError('the unchanged callback must raise')
        else:
            assert module.preserve_outer(make, visit) == 'done'
        assert len(errors) == 1
        assert callbacks == [('make', 'outer'), ('make', 'inner'), ('visit',)]
        # Ordinary retained traceback/frame-back ownership keeps the restored
        # outer local, not the temporary target, after normal or error exit.
        # No f_locals/f_back introspection is needed to observe this lifetime.
        result['outer_before_clear'] = references['outer']() is not None
        result['inner_before_clear'] = references['inner']() is not None
        result['events_before_clear'] = events.copy()
        errors[0].__traceback__ = None
        gc.collect()
        result['outer_after_clear'] = references['outer']() is not None
        result['inner_after_clear'] = references['inner']() is not None
        result['events_after_clear'] = events.copy()
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()
    return result

def _eager_comprehension_semantics(observed):
    assert not observed['outer_after_clear'] and not observed['inner_after_clear'], observed
    assert sorted(observed['events_after_clear']) == ['inner', 'outer'], observed
    return observed['builtin'], observed['field'], sorted(observed['events_after_clear'])

expected = _eager_comprehension_semantics(_eager_comprehension_frame_observations(ordinary, True))
actual = _eager_comprehension_semantics(_eager_comprehension_frame_observations(module, True))
assert actual == expected, (actual, expected)

_assert_source_function_witnesses()
