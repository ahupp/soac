# module:empty_annotation_cache
# soac: module(strict_assign=true, checked_attr=true)
from annotationlib import get_annotations

class Plain:
    def method(self, value: int) -> int:
        return value

# Introspection of an unannotated class lazily publishes native cache entries,
# including __annotate_func__ = None, before module sealing.
assert Plain.__annotate__ is None
assert Plain.__annotations__ == {}
assert get_annotations(Plain) == {}

class Annotated:
    value: int = 1

assert get_annotations(Annotated) == {'value': int}
# ok
# test_ordinary_class_cannot_gain_or_drop_a_transitive_strict_ancestor
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Plain.method',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import ctypes
import empty_annotation_cache as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
set_attr = ctypes.pythonapi.PyObject_SetAttr
set_attr.argtypes = [ctypes.py_object] * 3
set_attr.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert owner(module.Plain)
assert _soac_ext.strict_function_entry_kind(module.Plain.method) == expected_entry
assert module.Plain().method(7) == 7

class OrdinaryBase:
    pass
class OrdinaryAlternative(OrdinaryBase):
    pass
class Middle(module.Plain):
    pass
class Leaf(Middle):
    pass
class Victim(OrdinaryBase):
    pass
assert not owner(Middle) and not owner(Leaf) and not owner(Victim)
victim = Victim()
victim.method = 'ordinary shadow before a class transition'
dictionary = vars(victim)
leaf = Leaf()

# Ordinary-only MRO changes remain supported.
Victim.__bases__ = (OrdinaryAlternative,)
Victim.__bases__ = (OrdinaryBase,)

def rejected(operation):
    try:
        operation()
    except StrictMutationError:
        return
    raise AssertionError('an ordinary intermediate class bypassed strict ancestry')

for setter in (setattr, type.__setattr__, set_attr):
    before_bases, before_mro = Victim.__bases__, Victim.__mro__
    rejected(lambda: setter(Victim, '__bases__', (Middle,)))
    assert Victim.__bases__ is before_bases and Victim.__mro__ is before_mro
    assert type(victim) is Victim and vars(victim) is dictionary
    assert victim.method == 'ordinary shadow before a class transition'
    before_bases, before_mro = Leaf.__bases__, Leaf.__mro__
    rejected(lambda: setter(Leaf, '__bases__', (OrdinaryBase,)))
    assert Leaf.__bases__ is before_bases and Leaf.__mro__ is before_mro
    assert leaf.method(9) == 9

for setter in (setattr, object.__setattr__, set_attr):
    rejected(lambda: setter(victim, '__class__', Middle))
    rejected(lambda: setter(leaf, '__class__', Victim))
    assert type(victim) is Victim and type(leaf) is Leaf
if __dp_integration_mode__ == 'cpython':
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
    assert_native_class(module.Plain)
    assert _soac_ext.strict_function_diagnostics(module.Plain.method)["finalized"]
    for cls in (Middle, Leaf, Victim):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
        assert (
            info.abi_version, info.struct_size, info.phase,
            info.permanent_contract_published, info.owner, info.root_construction,
        ) == (0, 0, 0, 0, None, None)
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Plain.method',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
