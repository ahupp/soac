# module:model
# soac: module(strict_assign=true, checked_attr=true)
import support

def make():
    class Box:
        def method(self, value: int = 1) -> int:
            return value

        support.rewrite(locals())
    return Box
# module:support
created = []

class UnexpectedDescriptor:
    def __get__(self, instance, owner):
        return 0

def replacement(self, value):
    return ('ordinary replacement', value)

def rewrite(namespace):
    created.append(namespace['method'])
    if len(created) == 1:
        # Only this execution is dynamic; the source class remains eligible.
        namespace['unexpected'] = UnexpectedDescriptor()
    elif len(created) == 2:
        # Same source/code, but owned by the earlier dynamic class execution.
        namespace['method'] = created[0]
    elif len(created) == 4:
        # An unadmitted class-owned method remains instrumentable before the
        # class decision, even though its source annotations are supported.
        namespace['method'].__code__ = replacement.__code__
# ok
# test_same_source_method_from_earlier_dynamic_class_is_not_adopted
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('make',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import ctypes
import model
import support

is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int
function_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
function_identity.argtypes = [ctypes.py_object]
function_identity.restype = ctypes.c_uint64

first = model.make()
assert is_sealed(first) == 0
original = vars(first)['method']
assert function_identity(original) == 0
assert first().method('dynamic argument') == 'dynamic argument'
# Runtime decline must not seal a function and later revoke its protection.
# Even a same-code write remains legal here.
original.__code__ = original.__code__
original.__defaults__ = (11,)
assert first().method() == 11

second = model.make()
assert vars(second)['method'] is original
# Adoption must not freeze a function shared with the earlier dynamic class.
original.__defaults__ = (23,)
assert first().method() == second().method() == 23
assert is_sealed(second) == 0

fresh = model.make()
assert is_sealed(fresh) == 1
assert function_identity(fresh.method) != 0
assert fresh.method is support.created[2] and fresh.method is not original
assert fresh.method.__code__ is original.__code__
assert fresh().method() == 1 and first().method() == 23
assert fresh().method('not an integer') == 'not an integer'

changed = model.make()
assert is_sealed(changed) == 0
assert function_identity(changed.method) == 0
assert changed().method('dynamic') == ('ordinary replacement', 'dynamic')
if __dp_integration_mode__ == 'cpython':
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

    module_witness = _soac_ext.strict_module_diagnostics(model)
    make_witness = _assert_cpython_function_witness(
        model.make, module_witness,
    )
    assert make_witness["original_code_entered"]
    first_witness = _assert_cpython_function_witness(
        original, module_witness,
    )
    assert first_witness["finalized"] is False
    assert first_witness["original_code_entered"]
    fresh_witness = _assert_cpython_function_witness(
        fresh.method, module_witness,
    )
    assert fresh_witness["finalized"] and fresh_witness["original_code_entered"]
    assert fresh_witness["native_code_ordinal"] == first_witness["native_code_ordinal"]
    changed_witness = _soac_ext.strict_function_diagnostics(changed.method)
    assert changed_witness is not None
    assert changed_witness["schema"] == 2 and changed_witness["backend"] == "cpython"
    assert changed_witness["entry_kind"] == "ordinary_replacement"
    assert changed_witness["finalized"] is False
    assert changed_witness["native_code_ordinal"] == first_witness["native_code_ordinal"]
    for key in (
        "source_path", "source_sha256", "artifact_generation",
        "startup_identity", "interpreter_id",
    ):
        assert changed_witness[key] == module_witness[key]
    assert metadata(changed.method) is None
    for cls in (first, second, changed):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
        assert (
            info.abi_version, info.struct_size, info.phase,
            info.permanent_contract_published, info.owner, info.root_construction,
        ) == (0, 0, 0, 0, None, None)
        assert get_type_owner(cls) is None

    info = ConstructionInfoV1()
    assert get_construction(fresh, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == get_type_owner(fresh) and info.owner is not None

    dynamic_instance = first()
    checked_instance = fresh()
    for _ in range(128):
        assert dynamic_instance.method("ordinary") == "ordinary"
        assert checked_instance.method(7) == 7
    call_one = ctypes.pythonapi.PyObject_CallOneArg
    call_one.argtypes = [ctypes.py_object, ctypes.py_object]
    call_one.restype = ctypes.py_object
    assert call_one(dynamic_instance.method, "ordinary C") == "ordinary C"
    assert call_one(checked_instance.method, 8) == 8
    assert call_one(checked_instance.method, "ordinary C") == "ordinary C"
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('make',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
