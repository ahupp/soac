# modes:cpython
# Authenticated source and independent ordinary validation blocks.
# module:framework_methods
# soac: module(strict_assign=true, checked_attr=true)
from framework_probe import Meta, instrument

class Managed(metaclass=Meta):
    def method(self, value: int) -> int:
        return value

@instrument
class Decorated:
    def method(self, value: int) -> int:
        return value

def independent(value: int) -> int:
    return value
# module:framework_probe
def replacement(self, value):
    return ("framework", value)

def instrument(cls):
    vars(cls)["method"].__code__ = replacement.__code__
    return cls

class Meta(type):
    def __new__(metaclass, name, bases, namespace):
        namespace["method"].__code__ = replacement.__code__
        return super().__new__(metaclass, name, bases, namespace)
# ok
# tests/test_strict_function_boundaries.py::test_statically_dynamic_framework_methods_keep_ordinary_annotations
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('independent',):
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

import ctypes
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
get_type_owner.argtypes = [ctypes.py_object]
get_type_owner.restype = ctypes.c_void_p
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p

import framework_methods as module
module_witness = _soac_ext.strict_module_diagnostics(module)
for cls in (module.Managed, module.Decorated):
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
    assert get_type_owner(cls) is None
    function = vars(cls)["method"]
    witness = _soac_ext.strict_function_diagnostics(function)
    assert witness is not None
    assert witness["schema"] == 2 and witness["backend"] == "cpython"
    assert witness["entry_kind"] == "ordinary_replacement"
    assert witness["finalized"] is False
    assert metadata(function) is None
    for key in (
        "source_path", "source_sha256", "artifact_generation",
        "startup_identity", "interpreter_id",
    ):
        assert witness[key] == module_witness[key]
independent_witness = _assert_cpython_function_witness(
    module.independent, module_witness,
)
assert independent_witness["finalized"]

import framework_methods as module

# These source classes were already classified as dynamic before any method
# object existed. Framework instrumentation preserves ordinary code mutation.
for cls in (module.Managed, module.Decorated):
    assert cls().method("not an integer") == ("framework", "not an integer")

assert module.independent(3) == 3
assert module.independent("bad") == "bad"
print("static-framework-boundaries")

from soac.strict import StrictMutationError

call_one = ctypes.pythonapi.PyObject_CallOneArg
call_one.argtypes = [ctypes.py_object, ctypes.py_object]
call_one.restype = ctypes.py_object
for cls in (module.Managed, module.Decorated):
    instance = cls()
    for _ in range(128):
        assert instance.method("ordinary") == ("framework", "ordinary")
    assert call_one(instance.method, "C") == ("framework", "C")
assert call_one(module.independent, 4) == 4
assert call_one(module.independent, "bad") == "bad"
try:
    module.independent.__code__ = module.independent.__code__
except StrictMutationError:
    pass
else:
    raise AssertionError("framework fallback revoked independent code protection")
independent_after = _assert_cpython_function_witness(
    module.independent, module_witness,
)
assert independent_after["finalized"] and independent_after["original_code_entered"]

_assert_source_function_witnesses()
