# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:lambda_comprehension_frame
# soac: module(strict_assign=true, checked_attr=true)

def make():
    return lambda target, values, inner, visit, item: (
        [visit(target.value, item) for target.value in values for item in inner()],
        item,
    )
# module:ordinary_lambda_comprehension_frame
def make():
    return lambda target, values, inner, visit, item: (
        [visit(target.value, item) for target.value in values for item in inner()],
        item,
    )
# ok
# tests/test_strict_entry_runtime.py::test_lambda_comprehension_preserves_targets_outer_binding_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make',):
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

import ordinary_lambda_comprehension_frame as ordinary

def _lambda_comprehension_frame_observations(function, exceptional):
    import gc
    import weakref

    events = []
    finalized = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            references.append(weakref.ref(self))

        def __del__(self):
            finalized.append(self.label)

    class Target:
        def __setattr__(self, name, value):
            assert name == 'value', name
            events.append(('store', value))
            object.__setattr__(self, name, value)

    target = Target()

    def inner():
        events.append(('inner', target.value))
        return [Payload(f'{target.value}:a'), Payload(f'{target.value}:b')]

    def visit(outer, item):
        events.append(('visit', outer, item.label))
        if exceptional and item.label == '2:b':
            raise ValueError('lambda target callback')
        return outer, item.label

    saved = Payload('saved')
    try:
        result = function(target, (1, 2), inner, visit, saved)
    except ValueError as error:
        assert exceptional
        assert type(error) is ValueError
        outcome = ('error', str(error))
        # Explicitly retire the retained traceback before checking eventual
        # cleanup; implicit release timing is not a cross-engine requirement.
        error.__traceback__ = None
    else:
        assert not exceptional
        values, restored = result
        assert restored is saved, 'the comprehension must restore its outer parameter'
        outcome = ('return', values)
        del result, restored
    del saved
    gc.collect()
    assert all(reference() is None for reference in references)
    assert sorted(finalized) == ['1:a', '1:b', '2:a', '2:b', 'saved']
    assert events == [
        ('store', 1), ('inner', 1), ('visit', 1, '1:a'), ('visit', 1, '1:b'),
        ('store', 2), ('inner', 2), ('visit', 2, '2:a'), ('visit', 2, '2:b'),
    ]
    assert target.value == 2
    return {'events': events, 'outcome': outcome, 'finalized': sorted(finalized)}

expected = _lambda_comprehension_frame_observations(ordinary.make(), False)
function = module.make()
import ctypes
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
unchecked = ctypes.pythonapi.PyFunction_GetSoacFunctionId
unchecked.argtypes = [ctypes.py_object]
unchecked.restype = ctypes.c_uint64
source = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
source.argtypes = [ctypes.py_object]
source.restype = ctypes.c_uint64
assert metadata(function) and owner(function) and source(function.__code__)
assert unchecked(function) == 0
actual = _lambda_comprehension_frame_observations(function, False)
assert actual == expected, (actual, expected)

_assert_source_function_witnesses()
# ok
# tests/test_strict_entry_runtime.py::test_lambda_comprehension_preserves_targets_outer_binding_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make',):
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

import ordinary_lambda_comprehension_frame as ordinary

def _lambda_comprehension_frame_observations(function, exceptional):
    import gc
    import weakref

    events = []
    finalized = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            references.append(weakref.ref(self))

        def __del__(self):
            finalized.append(self.label)

    class Target:
        def __setattr__(self, name, value):
            assert name == 'value', name
            events.append(('store', value))
            object.__setattr__(self, name, value)

    target = Target()

    def inner():
        events.append(('inner', target.value))
        return [Payload(f'{target.value}:a'), Payload(f'{target.value}:b')]

    def visit(outer, item):
        events.append(('visit', outer, item.label))
        if exceptional and item.label == '2:b':
            raise ValueError('lambda target callback')
        return outer, item.label

    saved = Payload('saved')
    try:
        result = function(target, (1, 2), inner, visit, saved)
    except ValueError as error:
        assert exceptional
        assert type(error) is ValueError
        outcome = ('error', str(error))
        # Explicitly retire the retained traceback before checking eventual
        # cleanup; implicit release timing is not a cross-engine requirement.
        error.__traceback__ = None
    else:
        assert not exceptional
        values, restored = result
        assert restored is saved, 'the comprehension must restore its outer parameter'
        outcome = ('return', values)
        del result, restored
    del saved
    gc.collect()
    assert all(reference() is None for reference in references)
    assert sorted(finalized) == ['1:a', '1:b', '2:a', '2:b', 'saved']
    assert events == [
        ('store', 1), ('inner', 1), ('visit', 1, '1:a'), ('visit', 1, '1:b'),
        ('store', 2), ('inner', 2), ('visit', 2, '2:a'), ('visit', 2, '2:b'),
    ]
    assert target.value == 2
    return {'events': events, 'outcome': outcome, 'finalized': sorted(finalized)}

expected = _lambda_comprehension_frame_observations(ordinary.make(), True)
function = module.make()
import ctypes
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
unchecked = ctypes.pythonapi.PyFunction_GetSoacFunctionId
unchecked.argtypes = [ctypes.py_object]
unchecked.restype = ctypes.c_uint64
source = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
source.argtypes = [ctypes.py_object]
source.restype = ctypes.c_uint64
assert metadata(function) and owner(function) and source(function.__code__)
assert unchecked(function) == 0
actual = _lambda_comprehension_frame_observations(function, True)
assert actual == expected, (actual, expected)

_assert_source_function_witnesses()
