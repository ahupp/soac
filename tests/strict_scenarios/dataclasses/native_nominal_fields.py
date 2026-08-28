# modes:cpython
# module:nominal_dataclass_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import InitVar, dataclass, field
from nominal_dataclass_support import Target, post
import nominal_dataclass_support as support

@dataclass
class Direct:
    payload: Target
    seed: InitVar[Target]

    def __post_init__(self, seed):
        post(seed)

@dataclass
class Factory:
    payload: Target = field(default_factory=support.new_target)

def family():
    class LocalTarget:
        pass

    @dataclass(init=False)
    class Base:
        payload: LocalTarget
        seed: InitVar[LocalTarget]

    def replace_target(value: type[LocalTarget]):
        nonlocal LocalTarget
        LocalTarget = value

    def make_child():
        @dataclass
        class Child(Base):
            tag: int = 0

            def __post_init__(self, seed):
                post(seed)

        return Child

    return LocalTarget, Base, replace_target, make_child

class SelfProbe:
    def __init_subclass__(cls):
        support.observe(cls)

def self_slots():
    @dataclass(slots=True)
    class Node(SelfProbe):
        next: Node | None = None
    return Node
# module:nominal_dataclass_support
events = []
observed = []

class Target:
    pass

current = Target()

def new_target() -> Target:
    events.append('factory')
    return current

def post(seed: object) -> None:
    events.append(('post', seed))

def observe(cls):
    import ctypes
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    observed.append((cls, bool(owner(cls))))
# ok
# test_cpython_dataclass_field_initvar_and_factory_use_actual_native_globals [default]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('nominal_dataclass_model')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('family',):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = '\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import InitVar, dataclass, field\nfrom nominal_dataclass_support import Target, post\nimport nominal_dataclass_support as support\n\n@dataclass\nclass Direct:\n    payload: Target\n    seed: InitVar[Target]\n\n    def __post_init__(self, seed):\n        post(seed)\n\n@dataclass\nclass Factory:\n    payload: Target = field(default_factory=support.new_target)\n\ndef family():\n    class LocalTarget:\n        pass\n\n    @dataclass(init=False)\n    class Base:\n        payload: LocalTarget\n        seed: InitVar[LocalTarget]\n\n    def replace_target(value: type[LocalTarget]):\n        nonlocal LocalTarget\n        LocalTarget = value\n\n    def make_child():\n        @dataclass\n        class Child(Base):\n            tag: int = 0\n\n            def __post_init__(self, seed):\n                post(seed)\n\n        return Child\n\n    return LocalTarget, Base, replace_target, make_child\n\nclass SelfProbe:\n    def __init_subclass__(cls):\n        support.observe(cls)\n\ndef self_slots():\n    @dataclass(slots=True)\n    class Node(SelfProbe):\n        next: Node | None = None\n    return Node\n'
from soac import _soac_ext

import ctypes
import nominal_dataclass_model as model
import nominal_dataclass_support as support
from soac.strict import StrictMutationError

def api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
module_state = _soac_ext.strict_module_diagnostics(model)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def generated_owner(cls):
    assert class_owner(cls), 'the dataclass silently declined construction'
    initializer = vars(cls)['__init__']
    assert function_owner(initializer)
    assert not metadata(initializer), 'generated code acquired source/JIT authority'

def rejected_write(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('selected field storage accepted a foreign value')

import sys
import types

stock = types.ModuleType('ordinary_native_nominal_control')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary native nominal control>', 'exec'), vars(stock))
good = support.Target()
wrong = object()
assert not class_owner(stock.Direct)
support.events.clear()
assert stock.Direct(wrong, wrong).payload is wrong
assert support.events == [('post', wrong)]

generated_owner(model.Direct)
support.events.clear()
rejected_write(lambda: model.Direct(wrong, good))
assert support.events == []
assert model.Direct(good, wrong).payload is good
assert support.events == [('post', wrong)]
support.events.clear()
record = model.Direct(good, good)
assert record.payload is good and support.events == [('post', good)]
assert 'seed' not in vars(record)
rejected_write(lambda: setattr(record, 'payload', wrong))
rejected_write(lambda: vars(record).__setitem__('payload', wrong))
assert record.payload is good

# The original stdlib body still calls an ordinary factory exactly once.
class Foreign:
    pass

foreign = Foreign()
generated_owner(model.Factory)
support.current = wrong
support.events.clear()
rejected_write(model.Factory)
assert support.events == ['factory']
support.events.clear()
assert model.Factory.__init__(foreign) is None
assert support.events == ['factory'] and vars(foreign) == {'payload': wrong}
support.current = good
model.Factory.__init__(foreign)
assert foreign.payload is good
assert not function_owner(support.new_target)

# A C caller uses the same original generated body and ordinary binding.
invoke = ctypes.pythonapi.PyObject_Call
invoke.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
invoke.restype = ctypes.py_object
assert invoke(model.Direct, (good, good), {}).payload is good
assert invoke(model.Direct, (good, wrong), {}).payload is good
support.events.clear()
rejected_write(lambda: invoke(model.Direct, (wrong, good), {}))
assert support.events == []

_scenario_check_source_functions()
# ok
# test_cpython_dataclass_local_provider_forwarding_preserves_class_identity [default]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('nominal_dataclass_model')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('family',):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

from soac import _soac_ext

import ctypes
import nominal_dataclass_model as model
import nominal_dataclass_support as support
from soac.strict import StrictMutationError

def api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
module_state = _soac_ext.strict_module_diagnostics(model)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def generated_owner(cls):
    assert class_owner(cls), 'the dataclass silently declined construction'
    initializer = vars(cls)['__init__']
    assert function_owner(initializer)
    assert not metadata(initializer), 'generated code acquired source/JIT authority'

def rejected_write(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('selected field storage accepted a foreign value')

# These two source-identical class/provider trees are different activations.
left_target, left_base, replace, make_left = model.family()
right_target, right_base, unused, make_right = model.family()
assert left_target is not right_target and left_base is not right_base
assert class_owner(left_base) and class_owner(right_base)
replace(right_target)
for name in ('payload', 'seed'):
    left_base.__dataclass_fields__[name].type = right_target
left_class, right_class = make_left(), make_right()
generated_owner(left_class)
generated_owner(right_class)
left, right = left_target(), right_target()
support.events.clear()
rejected_write(lambda: left_class(right, left))
assert support.events == []
assert left_class(left, right).payload is left
rejected_write(lambda: right_class(left, right))
assert right_class(right, left).payload is right
assert support.events == [('post', right), ('post', left)]
assert left_class(left, left).payload is left
assert right_class(right, right).payload is right


_scenario_check_source_functions()
