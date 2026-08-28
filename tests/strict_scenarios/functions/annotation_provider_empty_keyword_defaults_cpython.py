# modes:cpython
# Authenticated source and independent ordinary validation blocks.
# module:provider_defaults
# soac: module(strict_assign=true, checked_attr=true)

from provider_defaults_probe import prepare

def build():
    class Local:
        pass
    def checked(value: Local) -> Local:
        return value
    prepare(checked)
    return checked, Local

pair = build()
# module:provider_defaults_probe
import weakref
events = []
records = []

class UnusedKey:
    def __hash__(self):
        events.append('hash')
        return hash('unused')
    def __eq__(self, other):
        events.append('equality')
        raise AssertionError('provider sealing must not look up unused keys')

def prepare(function):
    provider = function.__annotate__
    mapping = {UnusedKey(): 7} if False else {}
    provider.__kwdefaults__ = mapping
    records.append((mapping, weakref.ref(provider)))
    events.clear()
# ok
# tests/test_strict_function_boundaries.py::test_cpython_original_annotation_provider_preserves_keyword_defaults
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('build',):
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

def validate(module):
    import ctypes, sys
    import provider_defaults_probe as probe
    from soac import _soac_ext
    assert probe.events == [], probe.events
    function, Local = module.pair
    mapping, witness = probe.records[0]
    provider = function.__annotate__
    assert provider is witness()
    assert provider.__kwdefaults__ is mapping
    assert _soac_ext.strict_function_diagnostics(provider)['finalized'] is True
    assert _soac_ext.strict_function_diagnostics(provider)['original_code_entered'] is False
    value = Local()
    assert function(value) is value
    call = ctypes.pythonapi.PyObject_CallOneArg
    call.argtypes = [ctypes.py_object, ctypes.py_object]
    call.restype = ctypes.py_object
    assert call(function, value) is value
    for invoke in (function, lambda value: call(function, value)):
        marker = object()
        assert invoke(marker) is marker
    assert probe.events == [], probe.events
    assert provider(1) == {'value': Local, 'return': Local}
    try:
        mapping.clear()
    except TypeError:
        pass
    else:
        raise AssertionError('provider keyword defaults were not protected')
    import types
    ordinary = types.ModuleType('ordinary_provider_defaults')
    sys.modules[ordinary.__name__] = ordinary
    exec(compile('from provider_defaults_probe import prepare\n\ndef build():\n    class Local:\n        pass\n    def checked(value: Local) -> Local:\n        return value\n    prepare(checked)\n    return checked, Local\n\npair = build()\n', '<ordinary-provider-defaults>', 'exec', dont_inherit=True), vars(ordinary))
    control = ordinary.pair[0].__annotate__
    assert tuple(sys.getrefcount(item) for item in (provider.__code__, provider.__closure__)) == tuple(
        sys.getrefcount(item) for item in (control.__code__, control.__closure__)
    )

validate(module)

_assert_source_function_witnesses()
