# modes:cpython
# module:captured_exec_globals
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass

def make():
    @dataclass
    class Item:
        value: int = 3
    return Item
# ok
# test_cpython_dataclass_compiler_uses_actual_captured_exec_globals [after_capture]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('captured_exec_globals')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('make',):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = '\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\n\ndef make():\n    @dataclass\n    class Item:\n        value: int = 3\n    return Item\n'
mutation = 'after_capture'

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
import captured_exec_globals as model
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError

stock = types.ModuleType('ordinary_captured_exec_globals')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary captured exec globals>', 'exec'), vars(stock))
builder_code = dataclasses._FuncBuilder.add_fns_to_class.__code__
foreign_globals = [None]
active = [False]
changed = []
compiled = []

def replace_globals(frame):
    # Actual ordinary builder instance; no helper/code/decorator is replaced.
    builder = frame.f_locals['self']
    captured = builder.globals
    changed.append(id(captured))
    # Keep all contents identical: only the actual dictionary owner changes.
    replacement = dict(captured)
    assert replacement == captured and replacement is not captured
    foreign_globals[0] = replacement
    builder.globals = replacement

def profile(frame, event, argument):
    if (active[0] and mutation == 'before_capture' and event == 'call'
            and frame.f_code is builder_code):
        replace_globals(frame)

def audit(event, arguments):
    if not active[0] or event != 'compile':
        return
    frame = sys._getframe(1)
    if frame.f_code is not builder_code:
        return
    assert arguments[1] == '<string>'
    compiled.append(True)
    if mutation == 'after_capture':
        # Both ordinary exec and the selected native bridge have already
        # evaluated and own their real globals operand at this audit boundary.
        replace_globals(frame)

sys.addaudithook(audit)

def exercise(factory):
    changed.clear()
    compiled.clear()
    previous = sys.getprofile()
    active[0] = True
    sys.setprofile(profile)
    try:
        return factory(), None
    except Exception as error:
        return None, error
    finally:
        active[0] = False
        sys.setprofile(previous)

ordinary, error = exercise(stock.make)
assert error is None
assert changed == [id(vars(stock))] and compiled == [True]
assert ordinary.__init__.__globals__ is (
    vars(stock) if mutation == 'after_capture' else foreign_globals[0]
)
assert ordinary('ordinary unchecked').value == 'ordinary unchecked'

selected, error = exercise(model.make)
assert changed == [id(vars(model))]
if mutation == 'before_capture':
    # This must still fail at the actual EXEC operand check, before compiling
    # any generated source. A same-content foreign dictionary is not authority.
    assert isinstance(error, StrictRuntimeUnavailableError)
    assert selected is None and compiled == []
    del error
    # A failed graph cannot poison a later independent source invocation.
    selected = model.make()
else:
    assert error is None and compiled == [True]
assert selected.__init__.__globals__ is vars(model)

def api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
class_sealed = api('PyType_IsSoacSealed', ctypes.c_int)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
assert class_owner(selected) and class_sealed(selected) == 1
assert function_owner(selected.__init__)
assert not metadata(selected.__init__)
assert not class_owner(ordinary) and not function_owner(ordinary.__init__)
assert selected(7).value == 7
invoke = ctypes.pythonapi.PyObject_Call
invoke.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
invoke.restype = ctypes.py_object
assert invoke(selected, (8,), {}).value == 8
for operation in (lambda: selected('wrong'), lambda: invoke(selected, ('wrong',), {})):
    field_write_rejected(operation)
assert _soac_ext.strict_function_diagnostics(model.make)['original_code_entered']

_scenario_check_source_functions()
# ok
# test_cpython_dataclass_compiler_uses_actual_captured_exec_globals [before_capture]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('captured_exec_globals')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('make',):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = '\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\n\ndef make():\n    @dataclass\n    class Item:\n        value: int = 3\n    return Item\n'
mutation = 'before_capture'

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
import captured_exec_globals as model
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError

stock = types.ModuleType('ordinary_captured_exec_globals')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary captured exec globals>', 'exec'), vars(stock))
builder_code = dataclasses._FuncBuilder.add_fns_to_class.__code__
foreign_globals = [None]
active = [False]
changed = []
compiled = []

def replace_globals(frame):
    # Actual ordinary builder instance; no helper/code/decorator is replaced.
    builder = frame.f_locals['self']
    captured = builder.globals
    changed.append(id(captured))
    # Keep all contents identical: only the actual dictionary owner changes.
    replacement = dict(captured)
    assert replacement == captured and replacement is not captured
    foreign_globals[0] = replacement
    builder.globals = replacement

def profile(frame, event, argument):
    if (active[0] and mutation == 'before_capture' and event == 'call'
            and frame.f_code is builder_code):
        replace_globals(frame)

def audit(event, arguments):
    if not active[0] or event != 'compile':
        return
    frame = sys._getframe(1)
    if frame.f_code is not builder_code:
        return
    assert arguments[1] == '<string>'
    compiled.append(True)
    if mutation == 'after_capture':
        # Both ordinary exec and the selected native bridge have already
        # evaluated and own their real globals operand at this audit boundary.
        replace_globals(frame)

sys.addaudithook(audit)

def exercise(factory):
    changed.clear()
    compiled.clear()
    previous = sys.getprofile()
    active[0] = True
    sys.setprofile(profile)
    try:
        return factory(), None
    except Exception as error:
        return None, error
    finally:
        active[0] = False
        sys.setprofile(previous)

ordinary, error = exercise(stock.make)
assert error is None
assert changed == [id(vars(stock))] and compiled == [True]
assert ordinary.__init__.__globals__ is (
    vars(stock) if mutation == 'after_capture' else foreign_globals[0]
)
assert ordinary('ordinary unchecked').value == 'ordinary unchecked'

selected, error = exercise(model.make)
assert changed == [id(vars(model))]
if mutation == 'before_capture':
    # This must still fail at the actual EXEC operand check, before compiling
    # any generated source. A same-content foreign dictionary is not authority.
    assert isinstance(error, StrictRuntimeUnavailableError)
    assert selected is None and compiled == []
    del error
    # A failed graph cannot poison a later independent source invocation.
    selected = model.make()
else:
    assert error is None and compiled == [True]
assert selected.__init__.__globals__ is vars(model)

def api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
class_sealed = api('PyType_IsSoacSealed', ctypes.c_int)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
assert class_owner(selected) and class_sealed(selected) == 1
assert function_owner(selected.__init__)
assert not metadata(selected.__init__)
assert not class_owner(ordinary) and not function_owner(ordinary.__init__)
assert selected(7).value == 7
invoke = ctypes.pythonapi.PyObject_Call
invoke.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
invoke.restype = ctypes.py_object
assert invoke(selected, (8,), {}).value == 8
for operation in (lambda: selected('wrong'), lambda: invoke(selected, ('wrong',), {})):
    field_write_rejected(operation)
assert _soac_ext.strict_function_diagnostics(model.make)['original_code_entered']

_scenario_check_source_functions()
