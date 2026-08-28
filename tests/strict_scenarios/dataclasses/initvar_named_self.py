# modes:soac,entry
# module:named_self_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import InitVar, dataclass
from nominal_dataclass_support import Target, post

@dataclass
class Record:
    self: InitVar[Target]
    payload: Target

    def __post_init__(self, seed):
        post(seed)
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
# test_nominal_initvar_named_self_is_not_the_generated_receiver [default]
import sys
from soac import _soac_ext
import ctypes
import named_self_model as model
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

generated_owner(model.Record)
initializer = model.Record.__init__
assert initializer.__code__.co_varnames[:3] == ('__dataclass_self__', 'self', 'payload')
good = support.Target()
wrong = object()
support.events.clear()
assert model.Record(self=wrong, payload=good).payload is good
assert support.events == [('post', wrong)]
support.events.clear()
rejected_write(lambda: model.Record(self=good, payload=wrong))
assert support.events == []
support.events.clear()
record = model.Record(self=good, payload=good)
assert record.payload is good and vars(record) == {'payload': good}
assert support.events == [('post', good)]

class Foreign:
    # Ordinary generated code dispatches the post-init hook on its receiver.
    def __post_init__(self, seed):
        support.post(seed)

foreign = Foreign()
support.events.clear()
assert initializer(foreign, self=wrong, payload=good) is None
assert vars(foreign) == {'payload': good} and support.events == [('post', wrong)]
support.events.clear()
initializer(foreign, self=good, payload=good)
assert vars(foreign) == {'payload': good} and support.events == [('post', good)]
support.events.clear()
initializer(foreign, self=good, payload=wrong)
assert vars(foreign) == {'payload': wrong} and support.events == [('post', good)]
