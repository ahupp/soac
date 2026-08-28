# module:cached
# soac: module(strict_assign=true, checked_attr=true)
from functools import cached_property

class Cached:
    def __init__(self):
        self.value = 3
        self.hits = 0

    @cached_property
    def computed(self) -> int:
        self.hits += 1
        return self.value * 2

    def echo(self, value: int) -> int:
        return value
# module:ordinary_cached
from functools import cached_property

class Cached:
    def __init__(self):
        self.value = 3
        self.hits = 0

    @cached_property
    def computed(self) -> int:
        self.hits += 1
        return self.value * 2

    def echo(self, value: int) -> int:
        return value
# ok
# test_cached_property_keeps_original_descriptor_and_dynamic_cache_semantics
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes, functools, json, subprocess, types
# Keep the ordinary comparison in its own process, as in the original test.
ordinary_program = "import sys\nsys.path[:] = " + repr(sys.path) + "\n" + "import ctypes, functools, json, sys, types\nfrom soac import _soac_ext\nimport ordinary_cached as module\nassert _soac_ext.strict_module_diagnostics(module) is None\nowner = ctypes.pythonapi.PyType_GetSoacContractOwner\nowner.argtypes = [ctypes.py_object]\nowner.restype = ctypes.c_void_p\nassert owner(module.Cached) is None\ndescriptor = vars(module.Cached)['computed']\nassert type(descriptor) is functools.cached_property\nassert descriptor.attrname == 'computed'\ninstance = module.Cached()\nassert instance.__dict__ == {'value': 3, 'hits': 0}\nassert instance.computed == 6 and instance.computed == 6 and instance.hits == 1\ninstance.value = 5\nassert instance.computed == 6\ninstance.computed = 'assigned'\nassert instance.computed == 'assigned' and instance.hits == 1\ndel instance.computed\nassert instance.computed == 10 and instance.hits == 2\nassert vars(module.Cached)['computed'] is descriptor\n\n# The descriptor is an ordinary mutable stdlib object, not a replacement or a\n# frozen dependency. Its actual changed component runs on the next cache miss.\ndescriptor.func = lambda self: ('changed', self.value)\nassert instance.computed == 10\ndel instance.computed\nassert instance.computed == ('changed', 5) and instance.hits == 2\nreplacement = {'value': 7, 'hits': 0, 'computed': 'replacement cache'}\ninstance.__dict__ = replacement\nassert vars(instance) is replacement and instance.computed == 'replacement cache'\ndel instance.computed\nassert instance.computed == ('changed', 7)\nassert instance.echo('ordinary argument') == 'ordinary argument'\nprint(json.dumps({'values': vars(instance), 'descriptor_type': type(descriptor).__name__}))\n"
ordinary_result = subprocess.run([sys.executable, "-c", ordinary_program], capture_output=True, text=True)
assert ordinary_result.returncode == 0, ordinary_result.stdout + ordinary_result.stderr
expected_result = json.loads(ordinary_result.stdout.splitlines()[-1])
assert _soac_ext.strict_module_diagnostics(module)['sealed']
if __dp_integration_mode__ != 'cpython':
    assert _soac_ext.strict_function_entry_kind(module.Cached.echo) == expected_entry
if __dp_integration_mode__ == 'cpython':
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    for function in (module.Cached.__init__, module.Cached.echo,
                     vars(module.Cached)['computed'].func):
        observed = _assert_cpython_function_witness(
            function, diagnostic,
        )
        assert observed['finalized'] is False
    del function
    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(module.Cached, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (info.abi_version, info.struct_size, info.phase,
            info.permanent_contract_published, info.owner, info.root_construction) == (
        0, 0, 0, 0, None, None,
    )
owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert owner(module.Cached) is None
descriptor = vars(module.Cached)['computed']
assert type(descriptor) is functools.cached_property
assert descriptor.attrname == 'computed'
instance = module.Cached()
assert instance.__dict__ == {'value': 3, 'hits': 0}
assert instance.computed == 6 and instance.computed == 6 and instance.hits == 1
instance.value = 5
assert instance.computed == 6
instance.computed = 'assigned'
assert instance.computed == 'assigned' and instance.hits == 1
del instance.computed
assert instance.computed == 10 and instance.hits == 2
assert vars(module.Cached)['computed'] is descriptor

# The descriptor is an ordinary mutable stdlib object, not a replacement or a
# frozen dependency. Its actual changed component runs on the next cache miss.
descriptor.func = lambda self: ('changed', self.value)
assert instance.computed == 10
del instance.computed
assert instance.computed == ('changed', 5) and instance.hits == 2
replacement = {'value': 7, 'hits': 0, 'computed': 'replacement cache'}
instance.__dict__ = replacement
assert vars(instance) is replacement and instance.computed == 'replacement cache'
del instance.computed
assert instance.computed == ('changed', 7)
assert instance.echo('ordinary argument') == 'ordinary argument'
print(json.dumps({'values': vars(instance), 'descriptor_type': type(descriptor).__name__}))
assert json.loads(json.dumps({'values': vars(instance), 'descriptor_type': type(descriptor).__name__})) == expected_result
if __dp_integration_mode__ == 'cpython':
    observed = _assert_cpython_function_witness(
        module.Cached.echo, diagnostic,
    )
    assert observed['finalized'] is False and observed['original_code_entered']
    # Replacing this ordinary descriptor's component does not mint source authority.
    assert _soac_ext.strict_function_diagnostics(descriptor.func) is None
    generic_get = ctypes.pythonapi.PyObject_GenericGetAttr
    generic_get.argtypes = [ctypes.py_object, ctypes.py_object]
    generic_get.restype = ctypes.py_object
    for _ in range(128):
        assert instance.computed == ('changed', 7)
    assert generic_get(instance, 'computed') == ('changed', 7)
    assert vars(instance) is replacement and instance.hits == 0
    assert construction(module.Cached, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (info.abi_version, info.struct_size, info.phase,
            info.permanent_contract_published, info.owner, info.root_construction) == (
        0, 0, 0, 0, None, None,
    )
