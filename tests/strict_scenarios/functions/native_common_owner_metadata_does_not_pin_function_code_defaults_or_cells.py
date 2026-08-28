# modes:cpython
# Authenticated source and independent ordinary validation blocks.
# module:native_owner
# soac: module(strict_assign=true, checked_attr=true)

def make_cycle():
    saved = []
    def checked(value: int = 7) -> int:
        if saved:
            return value
        return 0
    saved.append(checked)
    return checked
# module:ordinary_owner
def make_cycle():
    saved = []
    def checked(value: int = 7) -> int:
        if saved:
            return value
        return 0
    saved.append(checked)
    return checked
# ok
# tests/test_strict_function_boundaries.py::test_native_common_owner_metadata_does_not_pin_function_code_defaults_or_cells
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_cycle',):
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
    import ctypes, gc, sys, weakref
    import ordinary_owner
    from soac import _soac_ext

    def counts(function):
        return tuple(sys.getrefcount(value) for value in (
            function, function.__code__, function.__defaults__, function.__closure__,
        ))

    ordinary = ordinary_owner.make_cycle()
    function = module.make_cycle()
    before = _soac_ext.strict_function_diagnostics(function)
    assert before["backend"] == "cpython"
    assert before["entry_kind"] == "original_code"
    assert before["finalized"] is True
    assert before["original_code_entered"] is False
    assert function("wrong") == "wrong"
    assert _soac_ext.strict_function_diagnostics(function)["original_code_entered"] is True
    assert function() == ordinary() == 7
    # The later referent genexpr makes only function a cell variable.
    # Measure both through the same argument-loading site, so a
    # LOAD_DEREF temporary is not mistaken for an owner-metadata edge.
    measured = [counts(value) for value in (function, ordinary)]
    assert measured[0] == measured[1], measured

    get_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    get_owner.argtypes = [ctypes.py_object]
    get_owner.restype = ctypes.c_void_p
    owner = ctypes.cast(get_owner(function), ctypes.py_object).value
    references = gc.get_referents(owner)
    assert not any(reference is value for reference in references for value in (
        function, function.__code__, function.__globals__,
        function.__defaults__, function.__kwdefaults__, function.__closure__,
    ) if value is not None)
    witness = weakref.ref(function)
    ordinary_witness = weakref.ref(ordinary)
    del references, function, ordinary
    gc.collect()
    assert ordinary_witness() is None
    assert witness() is None, "metadata retained a source function cycle"
    # Keeping the metadata shell itself alive must not keep the function.
    assert owner is not None

validate(module)

_assert_source_function_witnesses()
