# modes:cpython
# module:unknown_option_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
import unknown_option_support as support

def checked(value: int) -> int:
    return value

def build():
    @dataclass(eq=support.option())
    class Item:
        support.events.append("body")
        value: int

        def echo(self, value: int) -> int:
            return value

    return Item
# module:unknown_option_support
from typing import Any

events = []

class Truth:
    def __bool__(self):
        events.append("truth")
        return False

def option() -> Any:
    events.append("option")
    return Truth()
# ok
# test_cpython_unknown_dataclass_option_preserves_stdlib_truth_and_dynamic_class [default]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('unknown_option_model')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('build', 'checked'):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = '\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nimport unknown_option_support as support\n\ndef checked(value: int) -> int:\n    return value\n\ndef build():\n    @dataclass(eq=support.option())\n    class Item:\n        support.events.append("body")\n        value: int\n\n        def echo(self, value: int) -> int:\n            return value\n\n    return Item\n'

import ctypes
import dataclasses
import sys
import types
import unknown_option_model as model
import unknown_option_support as support
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
function_owner.argtypes = [ctypes.py_object]
function_owner.restype = ctypes.c_void_p
construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
construction.restype = ctypes.c_int
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object] * 3
generic_set.restype = ctypes.c_int
call_one = ctypes.pythonapi.PyObject_CallOneArg
call_one.argtypes = [ctypes.py_object, ctypes.py_object]
call_one.restype = ctypes.py_object

stock = types.ModuleType("ordinary_unknown_dataclass_option")
sys.modules[stock.__name__] = stock
exec(compile(source.replace("# soac: module(strict_assign=true, checked_attr=true)", ""),
             "<ordinary unknown dataclass option>", "exec"), vars(stock))

def exercise(source_module):
    support.events.clear()
    cls = source_module.build()
    assert dataclasses.is_dataclass(cls)
    assert owner(cls) is None, "an unknown option granted permanent class authority"
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert not function_owner(cls.__init__)
    value = object()
    instance = cls(value)
    assert instance.value is value
    assert instance.echo(value) is value
    replacement = object()
    assert generic_set(instance, "value", replacement) == 0
    assert instance.value is replacement
    assert call_one(cls, value).value is value
    if source_module is model:
        diagnostic = _soac_ext.strict_module_diagnostics(model)
        observed = _assert_cpython_function_witness(
            cls.echo, diagnostic,
        )
        assert observed["original_code_entered"]
    # Do not guess how often the real dataclass implementation consults eq.
    # Its original truth calls must happen, in exactly the ordinary order.
    events = tuple(support.events)
    assert events.count("option") == 1 and events.count("body") == 1
    assert events.count("truth") > 0
    return events

expected = exercise(stock)
actual = exercise(model)
assert actual == expected, (actual, expected)
assert model.checked(3) == 3
assert model.checked("ordinary annotated value") == "ordinary annotated value"
diagnostic = _soac_ext.strict_module_diagnostics(model)
for function in (model.build, model.checked):
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed["original_code_entered"]

_scenario_check_source_functions()
