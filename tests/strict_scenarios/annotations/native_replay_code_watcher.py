# modes:cpython
# module:native_replay_owner
# soac: module(strict_assign=true, checked_attr=true)
from native_replay_probe import inspect_before_seal

def build():
    class Local:
        pass
    def target(value: Local) -> Local:
        return value
    inspect_before_seal(target)
    return target, Local

target, Local = build()
# module:native_replay_probe
MUTATION = 'code'

import ctypes
import types

events = []

def inspect_before_seal(function):
    import _typing
    from soac import _soac_ext
    from soac.strict import StrictRuntimeUnavailableError
    provider = function.__annotate__
    assert _soac_ext.strict_function_diagnostics(provider)["backend"] == "cpython"
    assert _soac_ext.strict_function_diagnostics(provider)["finalized"] is False
    original = provider.__code__
    closure = provider.__closure__
    assert len(closure) == 1
    replacement_code = original.replace()
    replacement_closure = (types.CellType(str),)
    setter = ctypes.pythonapi.PyFunction_SetClosure
    setter.argtypes = [ctypes.py_object, ctypes.py_object]
    setter.restype = ctypes.c_int
    watcher_type = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.c_int, ctypes.py_object)
    created_codes = []
    callback_errors = []

    @watcher_type
    def callback(event, created):
        if event == 0 and not created_codes:
            created_codes.append(created)
            try:
                if MUTATION == "code":
                    provider.__code__ = replacement_code
                else:
                    setter(provider, replacement_closure)
            except BaseException as error:
                callback_errors.append(error)
        return 0

    add = ctypes.pythonapi.PyCode_AddWatcher
    add.argtypes = [watcher_type]
    add.restype = ctypes.c_int
    clear = ctypes.pythonapi.PyCode_ClearWatcher
    clear.argtypes = [ctypes.c_int]
    clear.restype = ctypes.c_int
    watcher = add(callback)
    try:
        try:
            _typing._soac_annotation_replay_code(provider, None, 4)
        except StrictRuntimeUnavailableError:
            events.append("watcher mutation refused")
        else:
            raise AssertionError("replay accepted a callback-mutated original provider")
        assert callback_errors == [], callback_errors
        assert len(created_codes) == 1
        source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
        source_id.argtypes = [ctypes.py_object]
        source_id.restype = ctypes.c_uint64
        assert source_id(created_codes[0]) == 0
        assert not created_codes[0].co_flags & 0x10000000
        if MUTATION == "code":
            assert provider.__code__ is replacement_code
        else:
            assert provider.__closure__ is replacement_closure
    finally:
        clear(watcher)
        provider.__code__ = original
        setter(provider, closure)
    assert provider.__code__ is original and provider.__closure__ is closure
# module:ordinary_replay_owner
def inspect_before_seal(function):
    pass

def build():
    class Local:
        pass
    def target(value: Local) -> Local:
        return value
    inspect_before_seal(target)
    return target, Local

target, Local = build()
# ok
# test_native_common_owner_replay_rechecks_actual_provider_after_code_watcher [code]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('native_replay_owner')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('build', 'target'):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

module = _scenario_subject
import annotationlib
import ordinary_replay_owner as ordinary
from native_replay_probe import events
from soac import _soac_ext
assert events == ["watcher mutation refused"]
for function, Local in [(module.target, module.Local), (ordinary.target, ordinary.Local)]:
    assert annotationlib.get_annotations(function) == {"value": Local, "return": Local}
    value = Local()
    assert function(value) is value
provider = module.target.__annotate__
assert _soac_ext.strict_function_diagnostics(provider)["finalized"] is True
assert annotationlib.get_annotations(
    module.target, format=annotationlib.Format.STRING,
) == {"value": "Local", "return": "Local"}

_scenario_check_source_functions()
