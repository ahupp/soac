# modes:cpython
# module:generic_descriptor_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass

@dataclass
class Box[T]:
    value: int

class Operations:
    @staticmethod
    def static(value: int) -> int:
        return value

    @classmethod
    def class_(cls, value: int) -> int:
        return value

    @property
    def value(self) -> int:
        return 7
# ok
# test_cpython_call_join_generic_dataclass_and_builtin_descriptor_births [default]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('generic_descriptor_model')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('Operations.static', 'Operations.class_'):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = '\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\n\n@dataclass\nclass Box[T]:\n    value: int\n\nclass Operations:\n    @staticmethod\n    def static(value: int) -> int:\n        return value\n\n    @classmethod\n    def class_(cls, value: int) -> int:\n        return value\n\n    @property\n    def value(self) -> int:\n        return 7\n'

import ctypes
import sys
import types
import typing
import generic_descriptor_model as model
from soac import _soac_ext
from tests.test_strict_type_native import ConstructionInfoV1

def api(name, result):
    f = getattr(ctypes.pythonapi, name)
    f.argtypes = [ctypes.py_object]
    f.restype = result
    return f

type_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
birth = api('PySoac_GetDescriptorBirthId', ctypes.c_uint64)
type_sealed = api('PyType_IsSoacSealed', ctypes.c_int)
construction_info = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
construction_info.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
construction_info.restype = ctypes.c_int

def no_native_class_contract(actual):
    info = ConstructionInfoV1()
    assert construction_info(actual, ctypes.byref(info), ctypes.sizeof(info)) == 0
    # A preconstruction decline has NO native state: it is not an unadmitted
    # Pending/Failed class or an already constructed phase-5 disposal.
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
    assert not type_owner(actual) and type_sealed(actual) == 0

stock = types.ModuleType('ordinary_generic_descriptor_control')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary generic descriptor control>', 'exec'), vars(stock))
assert not type_owner(stock.Box) and not type_owner(stock.Operations)
assert stock.Box('ordinary').value == 'ordinary'
assert stock.Operations.static('ordinary') == 'ordinary'
assert stock.Operations.class_('ordinary') == 'ordinary'
assert stock.Operations().value == model.Operations().value == 7
assert all(birth(vars(stock.Operations)[name]) == 0 for name in ('static', 'class_', 'value'))
# The real implicit Generic base is not a protected participating base.
# Box therefore stays an ordinary dataclass inside the still-strict module;
# no forced constructor/field authority is inferred from its int annotation.
no_native_class_contract(typing.Generic)
no_native_class_contract(model.Box)
assert model.Box.__bases__ == stock.Box.__bases__ == (typing.Generic,)
assert len(model.Box.__orig_bases__) == 1
assert typing.get_origin(model.Box.__orig_bases__[0]) is typing.Generic
assert typing.get_args(model.Box.__orig_bases__[0]) == model.Box.__type_params__
assert model.Box('ordinary').value == stock.Box('ordinary').value == 'ordinary'
assert not function_owner(model.Box.__init__) and not metadata(model.Box.__init__)
assert _soac_ext.strict_function_diagnostics(model.Box.__init__) is None

# The independent nongeneric class must still be genuinely admitted.
assert type_owner(model.Operations)
operations = ConstructionInfoV1()
assert construction_info(
    model.Operations, ctypes.byref(operations), ctypes.sizeof(operations)
) == 1
assert operations.abi_version == 1 and operations.struct_size == ctypes.sizeof(operations)
assert operations.phase == 3 and operations.permanent_contract_published == 1
assert operations.owner == type_owner(model.Operations) and operations.owner
assert type_sealed(model.Operations) == 1
module_witness = _soac_ext.strict_module_diagnostics(model)
assert module_witness['backend'] == 'cpython' and module_witness['sealed']
assert module_witness['initializer_entry_kind'] == 'original_code'
assert module_witness['original_code_entered'] is True

descriptors = [vars(model.Operations)[name] for name in ('static', 'class_', 'value')]
assert [type(value) for value in descriptors] == [staticmethod, classmethod, property]
assert all(birth(value) > 0 for value in descriptors)
assert len({birth(value) for value in descriptors}) == 3
for function in (descriptors[0].__func__, descriptors[1].__func__, descriptors[2].fget):
    assert function_owner(function) and not metadata(function)
for value in range(128):
    assert model.Box(value).value == value
    assert model.Operations.static(value) == value
    assert model.Operations.class_(value) == value
    assert model.Operations().value == 7
for invoke in (
    lambda: model.Operations.static('wrong'),
    lambda: model.Operations.class_('wrong'),
):
    assert invoke() == 'wrong'

call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
assert call(model.Operations.static, (5,), {}) == 5
assert call(model.Operations.static, ('wrong',), {}) == 'wrong'

assert call(model.Box, ('ordinary C argument',), {}).value == 'ordinary C argument'
for function in (descriptors[0].__func__, descriptors[1].__func__, descriptors[2].fget):
    witness = _soac_ext.strict_function_diagnostics(function)
    assert witness is not None and witness['backend'] == 'cpython'
    assert witness['entry_kind'] == 'original_code' and witness['original_code_entered']
    assert witness['finalized']
    for key in ('source_path', 'source_sha256', 'artifact_generation'):
        assert witness[key] == module_witness[key], (key, witness)
no_native_class_contract(model.Box)
no_native_class_contract(typing.Generic)
assert _soac_ext.runtime_compilation_activity() == {
    'schema': 1, 'lowering_entries': 0, 'blockpy_cache_entries': 0,
    'jit_engine_entries': 0,
}

_scenario_check_source_functions()
