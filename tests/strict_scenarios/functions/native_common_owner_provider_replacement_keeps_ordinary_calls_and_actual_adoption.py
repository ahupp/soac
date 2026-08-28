# modes:cpython
# Authenticated source and independent ordinary validation blocks.
# module:native_provider_pair
# soac: module(strict_assign=true, checked_attr=true)
from provider_pair_probe import exercise

def build():
    pairs = []
    for ordinal in (0, 1):
        class Local:
            pass
        def checked(value: Local) -> Local:
            return value
        pairs.append((checked, Local))
    exercise(pairs)
    return pairs

pairs = build()
# module:ordinary_provider_pair
def build():
    pairs = []
    for ordinal in (0, 1):
        class Local:
            pass
        def checked(value: Local) -> Local:
            return value
        pairs.append((checked, Local))
    return pairs
pairs = build()
# module:provider_pair_probe
events = []

def exercise(pairs):
    from soac import _soac_ext
    (first, First), (second, Second) = pairs
    assert first.__code__ is second.__code__
    assert first.__annotate__.__code__ is second.__annotate__.__code__
    assert first.__annotate__ is not second.__annotate__
    assert not _soac_ext.strict_function_diagnostics(first)["finalized"]
    assert not _soac_ext.strict_function_diagnostics(second)["finalized"]
    saved = second.__annotate__
    second.__annotate__ = first.__annotate__
    try:
        try:
            second()
        except TypeError as error:
            assert "required" in str(error) and "value" in str(error)
            events.append("ordinary missing argument")
        else:
            raise AssertionError("the original missing-argument error was lost")
        value = Second()
        assert second(value) is value
        assert _soac_ext.strict_function_diagnostics(second)["original_code_entered"]
        events.append("foreign provider leaves calls ordinary")
    finally:
        second.__annotate__ = saved
    # The loop rebinds one real Local cell. Both original providers read
    # Second, just as the ordinary CPython control does.
    second_value = Second()
    assert first(second_value) is second_value
    assert second(second_value) is second_value
    first_value = First()
    assert second(first_value) is first_value
    events.append("restored provider leaves calls ordinary")
# ok
# tests/test_strict_function_boundaries.py::test_native_common_owner_provider_replacement_keeps_ordinary_calls_and_actual_adoption
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('build',):
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
    import annotationlib
    import ordinary_provider_pair as ordinary
    from provider_pair_probe import events
    from soac import _soac_ext
    for control, _ in ordinary.pairs:
        annotations = annotationlib.get_annotations(control)
        assert annotations == {"value": ordinary.pairs[-1][1], "return": ordinary.pairs[-1][1]}
    assert events == [
        "ordinary missing argument", "foreign provider leaves calls ordinary",
        "restored provider leaves calls ordinary",
    ]
    LastLocal = module.pairs[-1][1]
    for function, _ in module.pairs:
        diagnostic = _soac_ext.strict_function_diagnostics(function)
        assert diagnostic["backend"] == "cpython"
        assert diagnostic["finalized"] is True
        assert diagnostic["original_code_entered"] is True
        assert function(LastLocal()).__class__ is LastLocal
        provider = function.__annotate__
        assert annotationlib.get_annotations(function) == {
            "value": LastLocal, "return": LastLocal,
        }
        assert _soac_ext.strict_function_diagnostics(provider)["finalized"] is True
        try:
            provider.__code__ = provider.__code__
        except TypeError:
            pass
        else:
            raise AssertionError("original provider was not frozen with its function")

validate(module)

_assert_source_function_witnesses()
