# modes:cpython
# Authenticated source and independent ordinary validation blocks.
# module:checked_calls
# soac: module(strict_assign=true, checked_attr=true)
from typing import final

EVENTS = []

class Calls:
    def checked(self, value: int) -> int:
        EVENTS.append('body')
        return value

    def forward(self, value: int) -> int:
        return self.checked(value)

    def rebind(self, value: int, replacement) -> int:
        value = replacement
        return self.checked(value)

    def after_callback(self, value: int, callback) -> int:
        callback()
        return self.checked(value)

    def computed_argument(self, value: int, callback) -> int:
        return self.checked(callback(value))

    def with_keyword(self, value: int) -> int:
        return self.checked(value=value)

    def defaulted(self, value: int = 7000) -> int:
        return value

    def without_argument(self) -> int:
        return self.defaulted()

    def broken_return(self, callback) -> int:
        return callback()

    def call_broken_return(self, callback) -> int:
        return self.broken_return(callback)

class Override(Calls):
    def checked(self, value: int) -> int:
        EVENTS.append('override')
        return value + 1

class FinalCalls:
    @final
    def checked(self, value: int) -> int:
        return value + 2

    def forward(self, value: int) -> int:
        return self.checked(value)

def make_calls(offset: int):
    class Local:
        def checked(self, value: int) -> int:
            return value + offset

        def forward(self, value: int) -> int:
            return self.checked(value)
    return Local
# ok
# tests/test_strict_checked_calls.py::test_cpython_final_method_policy_uses_the_admitted_class_and_actual_c_mutations
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('FinalCalls.checked', 'FinalCalls.forward'):
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
from tests.test_strict_type_native import ConstructionInfoV1

get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
get_type_owner.argtypes = [ctypes.py_object]
get_type_owner.restype = ctypes.c_void_p
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int

def assert_native_class(cls):
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == get_type_owner(cls) and info.owner is not None
    return info.owner


ordinary_source = "\nfrom typing import final\n\nEVENTS = []\n\nclass Calls:\n    def checked(self, value: int) -> int:\n        EVENTS.append('body')\n        return value\n\n    def forward(self, value: int) -> int:\n        return self.checked(value)\n\n    def rebind(self, value: int, replacement) -> int:\n        value = replacement\n        return self.checked(value)\n\n    def after_callback(self, value: int, callback) -> int:\n        callback()\n        return self.checked(value)\n\n    def computed_argument(self, value: int, callback) -> int:\n        return self.checked(callback(value))\n\n    def with_keyword(self, value: int) -> int:\n        return self.checked(value=value)\n\n    def defaulted(self, value: int = 7000) -> int:\n        return value\n\n    def without_argument(self) -> int:\n        return self.defaulted()\n\n    def broken_return(self, callback) -> int:\n        return callback()\n\n    def call_broken_return(self, callback) -> int:\n        return self.broken_return(callback)\n\nclass Override(Calls):\n    def checked(self, value: int) -> int:\n        EVENTS.append('override')\n        return value + 1\n\nclass FinalCalls:\n    @final\n    def checked(self, value: int) -> int:\n        return value + 2\n\n    def forward(self, value: int) -> int:\n        return self.checked(value)\n\ndef make_calls(offset: int):\n    class Local:\n        def checked(self, value: int) -> int:\n            return value + offset\n\n        def forward(self, value: int) -> int:\n            return self.checked(value)\n    return Local\n"

import checked_calls as subject
from soac.strict import StrictMutationError

assert_native_class(subject.FinalCalls)
checked = subject.FinalCalls.checked
assert checked.__final__ is True
assert _soac_ext.strict_function_diagnostics(checked)["finalized"] is True
receiver = subject.FinalCalls()
for _ in range(128):
    assert receiver.checked(3) == receiver.forward(3) == 5
for call in (receiver.checked, receiver.forward):
    try:
        call("wrong")
    except TypeError as error:
        assert type(error) is TypeError
    else:
        raise AssertionError("a final method lost its original addition error")

# Ordinary Python final is advisory. Reuse the exact subject, only without its
# module opt-in, as the control for every overridden-name operation below.
ordinary = {"__name__": "ordinary_final_method_control"}
exec(compile(ordinary_source, "<ordinary-final-method-control>", "exec"), ordinary)
control = type("Control", (ordinary["FinalCalls"],), {"checked": lambda self, value: value})
assert control().forward("ordinary") == "ordinary"

body_effects = []
class WrongOperand:
    def __add__(self, value):
        body_effects.append(value)
        return "ordinary body"
assert ordinary["FinalCalls"]().forward(WrongOperand()) == "ordinary body"
assert body_effects == [2]
body_effects.clear()
for call in (receiver.checked, receiver.forward):
    body_effects.clear()
    assert call(WrongOperand()) == "ordinary body"
    assert body_effects == [2]

def rejected(operation):
    try:
        operation()
    except StrictMutationError:
        return
    raise AssertionError("actual inherited final-method policy was bypassed")

rejected(lambda: type("OverrideFinal", (subject.FinalCalls,), {"checked": lambda self, value: value}))
class InheritsFinal(subject.FinalCalls):
    pass
assert get_type_owner(InheritsFinal) is None
info = ConstructionInfoV1()
assert get_construction(InheritsFinal, ctypes.byref(info), ctypes.sizeof(info)) == 0
assert info.owner is None and info.permanent_contract_published == 0
assert InheritsFinal().forward(3) == 5

set_attr = ctypes.pythonapi.PyObject_SetAttr
set_attr.argtypes = [ctypes.py_object] * 3
set_attr.restype = ctypes.c_int
set_item = ctypes.pythonapi.PyDict_SetItem
set_item.argtypes = [ctypes.py_object] * 3
set_item.restype = ctypes.c_int
class_dict = ctypes.pythonapi.PyType_GetDict
class_dict.argtypes = [ctypes.py_object]
class_dict.restype = ctypes.py_object
replacement = lambda self, value: "ordinary replacement"
for setter in (setattr, type.__setattr__, set_attr):
    setter(control, "checked", replacement)
    assert control().forward(3) == "ordinary replacement"
    rejected(lambda: setter(InheritsFinal, "checked", replacement))
    assert "checked" not in vars(InheritsFinal)
    assert InheritsFinal().forward(3) == 5
assert set_item(class_dict(control), "checked", replacement) == 0
rejected(lambda: set_item(class_dict(InheritsFinal), "checked", replacement))
assert "checked" not in vars(InheritsFinal)
assert subject.FinalCalls.checked is checked and InheritsFinal().forward(3) == 5

_assert_source_function_witnesses()
