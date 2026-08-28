# modes:cpython
# module:decorated_classes
# soac: module(strict_assign=true, checked_attr=true)
from typing import final

@final
# Native class providers start at the first decorator, not this header.
class Item:
    value: int = 1
    @final
    def method(self, number: int) -> int:
        return number

def factory():
    @final
    # The same projection must survive a real factory execution.
    class Local:
        value: int = 2
        @final
        def method(self, number: int) -> int:
            return number
    return Local

def identity(value):
    return value

@identity
# A generic wrapper must preserve the same class/provider start line.
class Generic[T]:
    value: T

def generic_factory():
    @identity
    class Local[T]:
        value: T
    return Local

@identity
def generic_function[T](value: T) -> T:
    return value

@identity
async def generic_async[T](value: T) -> T:
    return value
# ok
# test_cpython_class_final_decorator_remains_dynamic_without_a_supported_class_adapter [default]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('decorated_classes')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('Item.method', 'factory'):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()


import ctypes
from pathlib import Path
import decorated_classes as subject
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
module_witness = _soac_ext.strict_module_diagnostics(subject)
ordinary_source = Path(subject.__file__).read_text().replace(
    "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1,
)
ordinary = {"__name__": "ordinary_final_class_control"}
exec(compile(ordinary_source, "<ordinary-final-class-control>", "exec"), ordinary)
first, second = subject.factory(), subject.factory()
assert first is not second
for cls, control in (
    (subject.Item, ordinary["Item"]),
    (first, ordinary["factory"]()),
    (second, ordinary["factory"]()),
):
    # The annotation remains visible, but an unsupported class decorator
    # declines before any Pending or permanent instance/finality contract.
    assert vars(cls)["__final__"] is True
    assert vars(cls)["method"].__final__ is True
    assert get_type_owner(cls) is None
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
    function = vars(cls)["method"]
    witness = _assert_cpython_function_witness(
        function, module_witness,
    )
    assert witness["finalized"] is False
    assert cls().method("ordinary") == control().method("ordinary") == "ordinary"
    assert _soac_ext.strict_function_diagnostics(function)["original_code_entered"] is True
    child = type("ExternalChild", (cls,), {"method": lambda self, number: number})
    ordinary_child = type("OrdinaryChild", (control,), {"method": lambda self, number: number})
    assert child().method("overridden") == ordinary_child().method("overridden") == "overridden"
    child.method = lambda self, number: ("changed", number)
    ordinary_child.method = lambda self, number: ("changed", number)
    assert child().method(2) == ordinary_child().method(2) == ("changed", 2)
    assert get_type_owner(child) is None

_scenario_check_source_functions()
