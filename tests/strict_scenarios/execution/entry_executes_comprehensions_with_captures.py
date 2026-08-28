# Authenticated source and independent ordinary validation blocks.
# module:entry_executes_comprehensions_with_captures
# soac: module(strict_assign=true, checked_attr=true)

def build(values):
    scale = 2
    odd_list = [value + scale for value in values if value % 2]
    odd_dict = {value: value + scale for value in values if value % 2}
    odd_set = {value + scale for value in values if value % 2}
    return odd_list == [3, 5] and odd_dict == {1: 3, 3: 5} and odd_set == {3, 5}
# ok
# tests/test_strict_entry_runtime.py::test_strict_entry_runtime
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

assert module.build((1, 2, 3)) is True
if __dp_integration_mode__ == 'cpython':
    assert _soac_ext.strict_function_diagnostics(module.build)['original_code_entered'] is True

_assert_source_function_witnesses()
