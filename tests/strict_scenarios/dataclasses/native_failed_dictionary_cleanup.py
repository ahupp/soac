# modes:cpython
# module:failed_apply_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
import failed_apply_observer as support

class Stable:
    def value(self) -> int:
        return 17

def build():
    try:
        @dataclass(slots=False)
        class Subject:
            value: int = 3
    except Exception as error:
        support.caught.append(error)
        support.events.append('caught')
        return None
    finally:
        support.events.append('finally')
    return Subject
# module:failed_apply_observer
import dataclasses

armed = False
mode = None
process_code = dataclasses._process_class.__code__
root_code = None
primary = None
context = None
poison = object()
classes = []
caught = []
events = []

def remember(actual):
    if all(actual is not previous for previous in classes):
        classes.append(actual)

def profile(frame, event, result):
    if not armed:
        return
    if event == 'call' and frame.f_code is process_code:
        remember(frame.f_locals['cls'])
    if (event == 'return' and frame.f_code is process_code
            and isinstance(result, type)):
        remember(result)
        if mode == 'body':
            events.append('body failure')
            raise primary
    if (event == 'return' and frame.f_code is root_code
            and isinstance(result, type) and mode == 'postclear'):
        # All stdlib code ran normally; only the real returned type is changed.
        # The native post-clear policy must reject, not the mutation itself.
        result.__init__ = poison
        events.append('postclear mutation')
# ok
# test_cpython_failed_dataclass_cleanup_preserves_primary_and_escaped_barriers [body]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('failed_apply_model')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('build', 'Stable.value'):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = "\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nimport failed_apply_observer as support\n\nclass Stable:\n    def value(self) -> int:\n        return 17\n\ndef build():\n    try:\n        @dataclass(slots=False)\n        class Subject:\n            value: int = 3\n    except Exception as error:\n        support.caught.append(error)\n        support.events.append('caught')\n        return None\n    finally:\n        support.events.append('finally')\n    return Subject\n"
slots = False
failure = 'body'

from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ctypes
import dataclasses
import sys
import types
import failed_apply_model as model
import failed_apply_observer as support
from soac import _soac_ext
from soac.strict import StrictMutationError, StrictRuntimeUnavailableError

stock = types.ModuleType('ordinary_failed_apply_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary failed Apply control>', 'exec'), vars(stock))
# The actual public factory gives the same original wrap code used by Apply.
support.root_code = dataclasses.dataclass(slots=slots).__code__
unraisable = []

def exercise(factory):
    support.classes.clear()
    support.caught.clear()
    support.events.clear()
    support.primary = LookupError('actual ordinary profiler failure')
    support.context = ValueError('active caller context')
    support.mode = failure
    previous_profile = sys.getprofile()
    previous_unraisable = sys.unraisablehook
    sys.unraisablehook = lambda args: unraisable.append(args.exc_type)
    support.armed = True
    sys.setprofile(support.profile)
    try:
        try:
            raise support.context
        except ValueError:
            result = factory()
            assert sys.exception() is support.context
    finally:
        support.armed = False
        sys.setprofile(previous_profile)
        sys.unraisablehook = previous_unraisable
    return result

ordinary = exercise(stock.build)
assert len(support.classes) == (2 if slots else 1)
if failure == 'body':
    assert ordinary is None
    assert support.caught == [support.primary]
    assert support.caught[0].__context__ is support.context
    assert support.events == ['body failure', 'caught', 'finally']
else:
    assert isinstance(ordinary, type)
    assert vars(ordinary)['__init__'] is support.poison
    assert support.caught == []
    assert support.events == ['postclear mutation', 'finally']
assert unraisable == []

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
stable_owner = owner(model.Stable)
assert stable_owner and sealed(model.Stable) == 1

rejected = exercise(model.build)
assert rejected is None, 'the caught Apply failure was replaced at caller return'
assert len(support.classes) == (2 if slots else 1)
assert len(support.caught) == 1
if failure == 'body':
    assert support.caught[0] is support.primary
    assert support.events == ['body failure', 'caught', 'finally']
else:
    assert isinstance(support.caught[0], StrictRuntimeUnavailableError)
    assert support.events == ['postclear mutation', 'caught', 'finally']
assert support.caught[0].__context__ is support.context
assert unraisable == [], 'failed weak records triggered secondary completion errors'
failed = tuple(support.classes)

class ConstructionInfo(ctypes.Structure):
    _fields_ = [
        ('abi_version', ctypes.c_uint32), ('struct_size', ctypes.c_uint32),
        ('phase', ctypes.c_uint32), ('permanent_contract_published', ctypes.c_uint32),
        ('owner', ctypes.c_void_p), ('root_construction', ctypes.c_void_p),
    ]

get_info = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_info.argtypes = [ctypes.py_object, ctypes.POINTER(ConstructionInfo), ctypes.c_size_t]
get_info.restype = ctypes.c_int

def still_failed(actual):
    info = ConstructionInfo()
    assert get_info(actual, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 4 and not info.permanent_contract_published
    assert not owner(actual)
    for allocate in (lambda: actual(), lambda: object.__new__(actual)):
        try:
            allocate()
        except StrictMutationError:
            pass
        else:
            raise AssertionError('weak-record cleanup revoked an escaped Failed barrier')

for actual in failed:
    still_failed(actual)
# The same source produces a new, independently guarded graph after the catch.
support.events.clear()
selected = model.build()
assert all(selected is not actual for actual in failed)
assert owner(selected) and sealed(selected) == 1
assert selected(9).value == 9
field_write_rejected(lambda: selected('wrong'))
assert support.events == ['finally']
assert owner(model.Stable) == stable_owner and model.Stable().value() == 17
for actual in failed:
    still_failed(actual)
assert _soac_ext.strict_function_diagnostics(model.build)['original_code_entered']

_scenario_check_source_functions()
# ok
# test_cpython_failed_dataclass_cleanup_preserves_primary_and_escaped_barriers [postclear]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('failed_apply_model')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('build', 'Stable.value'):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = "\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nimport failed_apply_observer as support\n\nclass Stable:\n    def value(self) -> int:\n        return 17\n\ndef build():\n    try:\n        @dataclass(slots=False)\n        class Subject:\n            value: int = 3\n    except Exception as error:\n        support.caught.append(error)\n        support.events.append('caught')\n        return None\n    finally:\n        support.events.append('finally')\n    return Subject\n"
slots = False
failure = 'postclear'

from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ctypes
import dataclasses
import sys
import types
import failed_apply_model as model
import failed_apply_observer as support
from soac import _soac_ext
from soac.strict import StrictMutationError, StrictRuntimeUnavailableError

stock = types.ModuleType('ordinary_failed_apply_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary failed Apply control>', 'exec'), vars(stock))
# The actual public factory gives the same original wrap code used by Apply.
support.root_code = dataclasses.dataclass(slots=slots).__code__
unraisable = []

def exercise(factory):
    support.classes.clear()
    support.caught.clear()
    support.events.clear()
    support.primary = LookupError('actual ordinary profiler failure')
    support.context = ValueError('active caller context')
    support.mode = failure
    previous_profile = sys.getprofile()
    previous_unraisable = sys.unraisablehook
    sys.unraisablehook = lambda args: unraisable.append(args.exc_type)
    support.armed = True
    sys.setprofile(support.profile)
    try:
        try:
            raise support.context
        except ValueError:
            result = factory()
            assert sys.exception() is support.context
    finally:
        support.armed = False
        sys.setprofile(previous_profile)
        sys.unraisablehook = previous_unraisable
    return result

ordinary = exercise(stock.build)
assert len(support.classes) == (2 if slots else 1)
if failure == 'body':
    assert ordinary is None
    assert support.caught == [support.primary]
    assert support.caught[0].__context__ is support.context
    assert support.events == ['body failure', 'caught', 'finally']
else:
    assert isinstance(ordinary, type)
    assert vars(ordinary)['__init__'] is support.poison
    assert support.caught == []
    assert support.events == ['postclear mutation', 'finally']
assert unraisable == []

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
stable_owner = owner(model.Stable)
assert stable_owner and sealed(model.Stable) == 1

rejected = exercise(model.build)
assert rejected is None, 'the caught Apply failure was replaced at caller return'
assert len(support.classes) == (2 if slots else 1)
assert len(support.caught) == 1
if failure == 'body':
    assert support.caught[0] is support.primary
    assert support.events == ['body failure', 'caught', 'finally']
else:
    assert isinstance(support.caught[0], StrictRuntimeUnavailableError)
    assert support.events == ['postclear mutation', 'caught', 'finally']
assert support.caught[0].__context__ is support.context
assert unraisable == [], 'failed weak records triggered secondary completion errors'
failed = tuple(support.classes)

class ConstructionInfo(ctypes.Structure):
    _fields_ = [
        ('abi_version', ctypes.c_uint32), ('struct_size', ctypes.c_uint32),
        ('phase', ctypes.c_uint32), ('permanent_contract_published', ctypes.c_uint32),
        ('owner', ctypes.c_void_p), ('root_construction', ctypes.c_void_p),
    ]

get_info = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_info.argtypes = [ctypes.py_object, ctypes.POINTER(ConstructionInfo), ctypes.c_size_t]
get_info.restype = ctypes.c_int

def still_failed(actual):
    info = ConstructionInfo()
    assert get_info(actual, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 4 and not info.permanent_contract_published
    assert not owner(actual)
    for allocate in (lambda: actual(), lambda: object.__new__(actual)):
        try:
            allocate()
        except StrictMutationError:
            pass
        else:
            raise AssertionError('weak-record cleanup revoked an escaped Failed barrier')

for actual in failed:
    still_failed(actual)
# The same source produces a new, independently guarded graph after the catch.
support.events.clear()
selected = model.build()
assert all(selected is not actual for actual in failed)
assert owner(selected) and sealed(selected) == 1
assert selected(9).value == 9
field_write_rejected(lambda: selected('wrong'))
assert support.events == ['finally']
assert owner(model.Stable) == stable_owner and model.Stable().value() == 17
for actual in failed:
    still_failed(actual)
assert _soac_ext.strict_function_diagnostics(model.build)['original_code_entered']

_scenario_check_source_functions()
