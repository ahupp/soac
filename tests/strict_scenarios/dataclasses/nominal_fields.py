# modes:soac,entry
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
# test_generated_initvars_are_ordinary_but_actual_nominal_fields_are_checked [default]
import sys
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
assert _soac_ext.strict_function_entry_kind(model.Direct.__post_init__) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

generated_owner(model.Direct)
good = support.Target()
wrong = object()
support.events.clear()
rejected_write(lambda: model.Direct(wrong, good))
assert support.events == [], 'a failed field write reached post-init'
assert model.Direct(good, wrong).payload is good
assert support.events == [('post', wrong)], 'InitVar unexpectedly became an argument predicate'
support.events.clear()
value = model.Direct(good, good)
assert value.payload is good and support.events == [('post', good)]
assert 'seed' not in vars(value), 'InitVar became an instance storage field'

# Storage is selected independently of the generated constructor's arguments.
rejected_write(lambda: setattr(value, 'payload', wrong))
rejected_write(lambda: vars(value).__setitem__('payload', wrong))
assert value.payload is good
vars(value)['payload'] = good
assert value.payload is good

class Foreign:
    def __post_init__(self, seed):
        support.post(seed)

foreign = Foreign()
support.events.clear()
assert model.Direct.__init__(foreign, wrong, good) is None
assert vars(foreign) == {'payload': wrong}
assert model.Direct.__init__(foreign, good, wrong) is None
assert vars(foreign) == {'payload': good}
assert support.events == [('post', good), ('post', wrong)]
# ok
# test_inherited_generated_initializers_preserve_distinct_local_class_ownership [default]
import sys
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
assert _soac_ext.strict_function_entry_kind(model.family) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

first_target, first_base, replace, make_first = model.family()
second_target, second_base, unused, make_second = model.family()
assert first_target is not second_target and first_base is not second_base
assert class_owner(first_base) and class_owner(second_base)
assert '__init__' not in vars(first_base) and '__init__' not in vars(second_base)

# Neither the genuine annotation cell nor the mutable stdlib Field display
# cache may retarget the already-installed ancestor storage requirement.
replace(second_target)
for name in ('payload', 'seed'):
    first_base.__dataclass_fields__[name].type = second_target
first_child, second_child = make_first(), make_second()
generated_owner(first_child)
generated_owner(second_child)
left, right = first_target(), second_target()
support.events.clear()
rejected_write(lambda: first_child(right, left))
assert support.events == []
assert first_child(left, right).payload is left
rejected_write(lambda: second_child(left, right))
assert second_child(right, left).payload is right
assert support.events == [('post', right), ('post', left)]
support.events.clear()
first, second = first_child(left, left), second_child(right, right)
assert first.payload is left and second.payload is right
assert support.events == [('post', left), ('post', right)]
assert 'seed' not in vars(first) and first.tag == 0
rejected_write(lambda: setattr(first, 'payload', right))
assert first.payload is left
# ok
# test_generated_nominal_factory_runs_once_and_assigns_its_actual_result [default]
import sys
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

generated_owner(model.Factory)
class Foreign:
    pass

foreign = Foreign()
support.current = object()
support.events.clear()
rejected_write(model.Factory)
assert support.events == ['factory'], 'field rejection moved ahead of the ordinary factory call'
support.events.clear()
assert model.Factory.__init__(foreign) is None
assert support.events == ['factory'] and vars(foreign) == {'payload': support.current}
support.current = support.Target()
support.events.clear()
model.Factory.__init__(foreign)
assert support.events == ['factory'] and foreign.payload is support.current
support.events.clear()
assert model.Factory(support.current).payload is support.current
assert support.events == [], 'an explicitly supplied value invoked the factory'
assert not function_owner(support.new_target), 'the ordinary user factory was sealed'
# ok
# test_self_nominal_slots_admits_only_selected_type_without_call_predicates [default]
import sys
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
assert _soac_ext.strict_function_entry_kind(model.self_slots) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

support.observed.clear()
replacement = model.self_slots()
assert len(support.observed) == 2
(original, original_owned), (observed_replacement, replacement_owned) = support.observed
assert original is not replacement and observed_replacement is replacement
assert not original_owned and not replacement_owned
assert not class_owner(original) and class_owner(replacement)
assert original.__init__ is replacement.__init__
generated_owner(replacement)
good = replacement()
for cls in (original, replacement):
    assert function_owner(cls.__init__)
    assert cls(good).next is good
marker = object()
assert original(marker).next is marker, 'disposed original storage became constrained'
rejected_write(lambda: replacement(marker))
ordinary = original()
rejected_write(lambda: replacement(ordinary))
# A distinct invocation has its own field target, while the same generated
# initializer still accepts wrong nominal arguments on ordinary storage.
other = model.self_slots()
other_value = other()
rejected_write(lambda: replacement(other_value))
assert original(other_value).next is other_value

# Dataclasses repairs the owned provider's cell, not its callable metadata.
# Individual component adoption grants no source/JIT authority.
provider = replacement.__init__.__annotate__
index = provider.__code__.co_freevars.index('__class__')
assert provider.__closure__[index].cell_contents is replacement
has_creation = api('PyFunction_HasSoacDataclassCreation', ctypes.c_int)
strict_id = api('PyFunction_GetSoacStrictId', ctypes.c_uint64)
assert has_creation(provider) == 1
assert function_owner(provider)
assert metadata(provider) is None
assert strict_id(provider) != 0
