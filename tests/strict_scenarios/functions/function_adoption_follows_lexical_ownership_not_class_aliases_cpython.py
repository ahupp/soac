# modes:cpython
# Authenticated source and independent ordinary validation blocks.
# module:lexical_functions
# soac: module(strict_assign=true, checked_attr=true)
from lexical_function_support import DynamicMeta, remember, replacement

def standalone(value: int = 1) -> int:
    return value

class Dynamic(metaclass=DynamicMeta):
    borrowed = standalone

    def overwritten(self):
        return "original"

    preserved = remember(overwritten)
    overwritten = replacement()

    def factory(self):
        def nested(value: int = 3) -> int:
            return value
        return nested
# module:lexical_function_support
from typing import Any

class DynamicMeta(type):
    pass

def remember(function: Any) -> Any:
    return function

def changed(self):
    return "changed"

def replacement() -> Any:
    return changed
# ok
# tests/test_strict_function_boundaries.py::test_function_adoption_follows_lexical_ownership_not_class_aliases
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('standalone',):
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
import lexical_functions as module
from lexical_function_support import changed
from soac.strict import StrictMutationError

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

module_witness = _soac_ext.strict_module_diagnostics(module)
cls = module.Dynamic
info = ConstructionInfoV1()
assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
assert (
    info.abi_version, info.struct_size, info.phase,
    info.permanent_contract_published, info.owner, info.root_construction,
) == (0, 0, 0, 0, None, None)
assert get_type_owner(cls) is None

standalone_witness = _assert_cpython_function_witness(
    module.standalone, module_witness,
)
assert standalone_witness["finalized"]

assert module.Dynamic.borrowed is module.standalone
assert module.standalone(4) == 4
assert module.standalone("bad") == "bad"

# Overwriting the final class member does not make the old lexical method a
# free function. Its statically dynamic framework keeps ordinary mutability.
preserved = module.Dynamic.preserved
preserved_witness = _assert_cpython_function_witness(
    preserved, module_witness,
)
assert preserved_witness["finalized"] is False
preserved.__code__ = changed.__code__
assert preserved(None) == "changed"
factory_witness = _assert_cpython_function_witness(
    vars(module.Dynamic)["factory"], module_witness,
)
assert factory_witness["finalized"] is False

# A definition inside a method has that function as its immediate scope, not
# the enclosing class. Its late free-function completion still applies.
nested = module.Dynamic().factory()
nested_witness = _assert_cpython_function_witness(
    nested, module_witness,
)
assert nested_witness["finalized"]
assert nested() == 3
try:
    nested.__defaults__ = (5,)
except StrictMutationError:
    pass
else:
    raise AssertionError("an enclosing dynamic class captured a nested free definition")
print("lexical-function-ownership")

# Dynamic class writes must not revoke or retarget the independent function.
module.Dynamic.borrowed = changed
assert module.standalone is not module.Dynamic.borrowed
for _ in range(128):
    assert module.standalone(4) == 4
    assert nested(5) == 5
call_one = ctypes.pythonapi.PyObject_CallOneArg
call_one.argtypes = [ctypes.py_object, ctypes.py_object]
call_one.restype = ctypes.py_object
for function, value in ((module.standalone, 6), (nested, 7)):
    assert call_one(function, value) == value
    assert call_one(function, "bad") == "bad"
    try:
        function.__code__ = function.__code__
    except StrictMutationError:
        pass
    else:
        raise AssertionError("dynamic class revoked independent code protection")
    witness = _assert_cpython_function_witness(
        function, module_witness,
    )
    assert witness["finalized"] and witness["original_code_entered"]
preserved_after = _soac_ext.strict_function_diagnostics(preserved)
assert preserved_after is not None
assert preserved_after["backend"] == "cpython"
assert preserved_after["entry_kind"] == "ordinary_replacement"
assert preserved_after["finalized"] is False
for key in (
    "source_path", "source_sha256", "artifact_generation",
    "startup_identity", "interpreter_id",
):
    assert preserved_after[key] == module_witness[key]
assert metadata(preserved) is None

_assert_source_function_witnesses()
