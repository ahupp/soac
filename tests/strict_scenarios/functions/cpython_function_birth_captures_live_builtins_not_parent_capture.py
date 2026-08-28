# modes:cpython
# Authenticated source and independent ordinary validation blocks.
# module:builtin_birth
# soac: module(strict_assign=true, checked_attr=true)

import builtins

INITIAL_BUILTINS = builtins.__dict__

def make():
    def read():
        return len((1, 2, 3))
    return read

def replacement_len(values):
    return 37

CAPTURED_BUILTINS = dict(INITIAL_BUILTINS)
CAPTURED_BUILTINS['len'] = replacement_len
__builtins__ = CAPTURED_BUILTINS
created = make()
__builtins__ = INITIAL_BUILTINS
# module:ordinary_builtin_birth
import builtins

INITIAL_BUILTINS = builtins.__dict__

def make():
    def read():
        return len((1, 2, 3))
    return read

def replacement_len(values):
    return 37

CAPTURED_BUILTINS = dict(INITIAL_BUILTINS)
CAPTURED_BUILTINS['len'] = replacement_len
__builtins__ = CAPTURED_BUILTINS
created = make()
__builtins__ = INITIAL_BUILTINS
# ok
# tests/test_strict_function_boundaries.py::test_cpython_function_birth_captures_live_builtins_not_parent_capture
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make', 'replacement_len', 'created'):
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

def validate_module(module):
    import ctypes
    import types
    import pytest
    import ordinary_builtin_birth as ordinary
    from soac import _soac_ext, StrictRuntimeUnavailableError
    from tests._strict_integration import _assert_cpython_function_witness

    get_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    get_owner.argtypes = [ctypes.py_object]
    get_owner.restype = ctypes.c_void_p
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    assert diagnostic['sealed'] and diagnostic['backend'] == 'cpython'
    assert _soac_ext.strict_module_diagnostics(ordinary) is None

    for target in (ordinary, module):
        assert target.make.__builtins__ is target.INITIAL_BUILTINS
        assert vars(target)['__builtins__'] is target.INITIAL_BUILTINS
        assert target.created.__globals__ is vars(target)
        assert target.created.__builtins__ is target.CAPTURED_BUILTINS
        assert target.created.__builtins__ is not target.make.__builtins__
        assert target.created() == 37
        later = target.make()
        assert later.__code__ is target.created.__code__
        assert later.__globals__ is vars(target)
        assert later.__builtins__ is target.INITIAL_BUILTINS
        assert later() == 3
        assert target.created() == 37
        for function in (target.make, target.replacement_len, target.created, later):
            if target is ordinary:
                assert get_owner(function) is None
                assert _soac_ext.strict_function_diagnostics(function) is None
            else:
                assert get_owner(function)
                observed = _assert_cpython_function_witness(function, diagnostic)
                assert observed['original_code_entered'] is True

    # Captured builtins do not grant a copied strict code object an owner.
    ordinary_copy = types.FunctionType(ordinary.created.__code__, vars(ordinary))
    assert ordinary_copy.__builtins__ is ordinary.INITIAL_BUILTINS
    assert ordinary_copy() == 3
    unowned = types.FunctionType(module.created.__code__, vars(module))
    assert get_owner(unowned) is None
    assert _soac_ext.strict_function_diagnostics(unowned) is None
    with pytest.raises(StrictRuntimeUnavailableError):
        unowned()
    assert module.created() == 37

validate_module(module)

_assert_source_function_witnesses()
