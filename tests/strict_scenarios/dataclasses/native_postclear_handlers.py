# modes:cpython
# module:postclear_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
import postclear_observer as support

def build():
    try:
        @dataclass
        class Subject:
            value: int = 1
    except Exception as error:
        support.capture(error)
        return None
    finally:
        support.events.append('finally')
    return Subject
# module:postclear_observer
import dataclasses
import weakref

armed = False
poison = object()
events = []
mutations = []
errors = []
tracebacks = []
results = []

def profile(frame, event, result):
    if not armed or event != 'return' or frame.f_code is not dataclasses.dataclass.__code__:
        return
    if not isinstance(result, type) or result.__name__ != 'Subject':
        return
    # The actual stdlib Apply has finished its ordinary construction. No helper,
    # decorator, code/defaults, or native owner is replaced or fabricated.
    setattr(result, '__init__', poison)
    mutations.append(id(result))
    results.append(weakref.ref(result))
    events.append('stdlib apply returned')

def capture(error):
    errors.append(type(error))
    frames = []
    current = error.__traceback__
    while current is not None:
        frames.append((current.tb_frame.f_code.co_filename,
                       current.tb_frame.f_code.co_name, current.tb_lineno))
        current = current.tb_next
    # Keep scalars only; no root frame, traceback, class, namespace or result pin.
    tracebacks.append(tuple(frames))
    events.append('caught')
# ok
# test_cpython_dataclass_postclear_completion_error_uses_caller_handlers_and_traceback [default]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('postclear_model')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('build',):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = "\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nimport postclear_observer as support\n\ndef build():\n    try:\n        @dataclass\n        class Subject:\n            value: int = 1\n    except Exception as error:\n        support.capture(error)\n        return None\n    finally:\n        support.events.append('finally')\n    return Subject\n"

from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ast
import ctypes
from pathlib import Path
import sys
import types
import postclear_model as model
import postclear_observer as support
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p

# Exact ordinary source control: only the policy comment is absent. A normal
# dataclass accepts this late mutation and returns the resulting ordinary type.
stock = types.ModuleType('ordinary_postclear_control')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary postclear control>', 'exec'), vars(stock))
previous_profile = sys.getprofile()
support.armed = True
sys.setprofile(support.profile)
try:
    ordinary = stock.build()
finally:
    sys.setprofile(previous_profile)
    support.armed = False
assert isinstance(ordinary, type) and not owner(ordinary)
assert vars(ordinary)['__init__'] is support.poison
assert support.events == ['stdlib apply returned', 'finally']
assert support.errors == []

support.events.clear()
support.mutations.clear()
support.results.clear()
support.errors.clear()
support.tracebacks.clear()
unraisable = []
previous_unraisable = sys.unraisablehook
sys.unraisablehook = lambda args: unraisable.append(args.exc_type)
support.armed = True
sys.setprofile(support.profile)
try:
    rejected = model.build()
finally:
    sys.setprofile(previous_profile)
    sys.unraisablehook = previous_unraisable
    support.armed = False

# The mutation must actually succeed at Apply return; a mutation-time rejection
# or a descriptor's C completion error cannot substitute for this handoff.
assert len(support.mutations) == 1
assert rejected is None
assert support.events == ['stdlib apply returned', 'caught', 'finally']
assert support.errors == [StrictRuntimeUnavailableError]
assert unraisable == []
assert len(support.tracebacks) == 1
frames = support.tracebacks[0]
own = [entry for entry in frames if Path(entry[0]) == Path(model.__file__)]
parsed = ast.parse(Path(model.__file__).read_text())
build = next(node for node in parsed.body
             if isinstance(node, ast.FunctionDef) and node.name == 'build')
statement = next(node for node in ast.walk(build)
                 if isinstance(node, ast.ClassDef) and node.name == 'Subject')
assert own == [(model.__file__, 'build', statement.decorator_list[0].lineno)]
assert all(name not in ('dataclass', '_process_class', '_add_slots')
           for _, name, _ in frames), 'retired stdlib root leaked into caller traceback'

# Failure is terminal for the attempted graph, not for a later independent
# execution of the same authenticated source class.
support.events.clear()
selected = model.build()
assert owner(selected)
assert selected(4).value == 4
field_write_rejected(lambda: selected('wrong'))
assert support.events == ['finally']
assert _soac_ext.strict_function_diagnostics(model.build)['original_code_entered']

_scenario_check_source_functions()
