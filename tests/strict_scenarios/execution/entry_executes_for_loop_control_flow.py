# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:entry_executes_for_loop_control_flow
# soac: module(strict_assign=true, checked_attr=true)

def build(values):
    exhausted = []
    for value in values:
        if value == 2:
            continue
        exhausted.append(value)
    else:
        exhausted.append(99)
    exhausted_last = value

    stopped = []
    for value in values:
        if value == 2:
            continue
        if value == 3:
            break
        stopped.append(value)
    else:
        stopped.append(99)
    stopped_last = value

    return (exhausted == [1, 3, 99] and exhausted_last == 3
            and stopped == [1] and stopped_last == 3)
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

_assert_source_function_witnesses()
