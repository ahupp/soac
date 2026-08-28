# modes:soac,entry
# module:explicit_object
# soac: module(strict_assign=true, checked_attr=true)
from builtins import object as root_object

class Direct(object):
    value: int = 1

    def read(self) -> int:
        return self.value

class Aliased(root_object):
    value: int = 2

    def read(self) -> int:
        return self.value
# ok
# test_explicit_builtin_object_base_installs_a_real_class_contract
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import _testinternalcapi
import ctypes
import explicit_object as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(module)['sealed']
for cls, default in ((module.Direct, 1), (module.Aliased, 2)):
    assert cls.__bases__ == (object,)
    assert owner(cls) and sealed(cls), 'a builtin base was treated as an unknown user type'
    assert _soac_ext.strict_function_entry_kind(cls.read) == expected_entry
    instance = cls()
    dictionary = vars(instance)
    assert dictionary == {} and instance.read() == default
    assert _testinternalcapi.dict_has_indexed_keys(dictionary) is False, (
        'pending source storage must not acquire an indexed layout'
    )
    instance.value = 7
    assert instance.read() == 7 and list(dictionary) == ['value']
    assert _testinternalcapi.dict_has_indexed_keys(dictionary) is False
    try:
        instance.read = object()
    except StrictMutationError:
        pass
    else:
        raise AssertionError('explicit-object class did not protect its methods')
