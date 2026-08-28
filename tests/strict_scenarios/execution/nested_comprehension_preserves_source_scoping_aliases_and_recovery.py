# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:nested_comprehension_binding
# soac: module(strict_assign=true, checked_attr=true)

def nested(values, value, inner, visit, observe):
    try:
        result = {value: [visit(value, inner) for inner in (value, value)]
                  for value in values}
    except ValueError:
        observe('error', value, inner)
        raise
    finally:
        observe('finally', value, inner)
    return result, value, inner
# module:ordinary_nested_comprehension_binding
def nested(values, value, inner, visit, observe):
    try:
        result = {value: [visit(value, inner) for inner in (value, value)]
                  for value in values}
    except ValueError:
        observe('error', value, inner)
        raise
    finally:
        observe('finally', value, inner)
    return result, value, inner
# ok
# tests/test_strict_entry_runtime.py::test_nested_comprehension_preserves_source_scoping_aliases_and_recovery
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('nested',):
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

import ordinary_nested_comprehension_binding as ordinary

def _nested_comprehension_binding_observations(module, exceptional):
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

    first = Payload('first')
    second = Payload('second')
    saved_value = Payload('outer-value')
    saved_inner = Payload('outer-inner')
    failure = ValueError('nested iteration callback')
    failing = exceptional

    def visit(value, inner):
        assert inner is value, 'the nested target must alias its own iterable element'
        events.append(('visit', value.label))
        if failing and value is second:
            raise failure
        return inner

    def observe(event, value, inner):
        assert value is saved_value, 'outer target must be restored before source cleanup'
        assert inner is saved_inner, 'child target must be restored before source cleanup'
        events.append((event, value.label, inner.label))

    outcomes = []
    # The second call proves recovery after error; the third never enters the
    # child region. All observations are explicit source operations or aliases.
    for failing, values in [(exceptional, (first, second)), (False, (first, second)), (False, ())]:
        start = len(events)
        try:
            result, outer, inner = module.nested(
                values, saved_value, saved_inner, visit, observe,
            )
        except ValueError as error:
            assert failing and error is failure
            failure.__traceback__ = None
            outcomes.append('error')
            expected_events = [
                ('visit', 'first'), ('visit', 'first'), ('visit', 'second'),
                ('error', 'outer-value', 'outer-inner'),
                ('finally', 'outer-value', 'outer-inner'),
            ]
        else:
            assert not failing
            assert outer is saved_value and inner is saved_inner
            assert list(result) == list(values)
            for value in values:
                assert len(result[value]) == 2
                assert result[value][0] is value and result[value][1] is value
            if values:
                del value
            outcomes.append([(key.label, [item.label for item in result[key]]) for key in values])
            expected_events = [
                *[('visit', value.label) for value in values for _ in range(2)],
                ('finally', 'outer-value', 'outer-inner'),
            ]
            del result, outer, inner
        assert events[start:] == expected_events
    del first, second, saved_value, saved_inner, visit, observe, failure
    gc.collect()
    assert all(reference() is None for reference in references)
    assert sorted(finalized) == ['first', 'outer-inner', 'outer-value', 'second']
    return {'events': events, 'outcomes': outcomes, 'finalized': sorted(finalized)}

expected = _nested_comprehension_binding_observations(ordinary, False)
actual = _nested_comprehension_binding_observations(module, False)
assert actual == expected, (actual, expected)

_assert_source_function_witnesses()
# ok
# tests/test_strict_entry_runtime.py::test_nested_comprehension_preserves_source_scoping_aliases_and_recovery
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('nested',):
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

import ordinary_nested_comprehension_binding as ordinary

def _nested_comprehension_binding_observations(module, exceptional):
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

    first = Payload('first')
    second = Payload('second')
    saved_value = Payload('outer-value')
    saved_inner = Payload('outer-inner')
    failure = ValueError('nested iteration callback')
    failing = exceptional

    def visit(value, inner):
        assert inner is value, 'the nested target must alias its own iterable element'
        events.append(('visit', value.label))
        if failing and value is second:
            raise failure
        return inner

    def observe(event, value, inner):
        assert value is saved_value, 'outer target must be restored before source cleanup'
        assert inner is saved_inner, 'child target must be restored before source cleanup'
        events.append((event, value.label, inner.label))

    outcomes = []
    # The second call proves recovery after error; the third never enters the
    # child region. All observations are explicit source operations or aliases.
    for failing, values in [(exceptional, (first, second)), (False, (first, second)), (False, ())]:
        start = len(events)
        try:
            result, outer, inner = module.nested(
                values, saved_value, saved_inner, visit, observe,
            )
        except ValueError as error:
            assert failing and error is failure
            failure.__traceback__ = None
            outcomes.append('error')
            expected_events = [
                ('visit', 'first'), ('visit', 'first'), ('visit', 'second'),
                ('error', 'outer-value', 'outer-inner'),
                ('finally', 'outer-value', 'outer-inner'),
            ]
        else:
            assert not failing
            assert outer is saved_value and inner is saved_inner
            assert list(result) == list(values)
            for value in values:
                assert len(result[value]) == 2
                assert result[value][0] is value and result[value][1] is value
            if values:
                del value
            outcomes.append([(key.label, [item.label for item in result[key]]) for key in values])
            expected_events = [
                *[('visit', value.label) for value in values for _ in range(2)],
                ('finally', 'outer-value', 'outer-inner'),
            ]
            del result, outer, inner
        assert events[start:] == expected_events
    del first, second, saved_value, saved_inner, visit, observe, failure
    gc.collect()
    assert all(reference() is None for reference in references)
    assert sorted(finalized) == ['first', 'outer-inner', 'outer-value', 'second']
    return {'events': events, 'outcomes': outcomes, 'finalized': sorted(finalized)}

expected = _nested_comprehension_binding_observations(ordinary, True)
actual = _nested_comprehension_binding_observations(module, True)
assert actual == expected, (actual, expected)

_assert_source_function_witnesses()
