"""Dataclass ownership and field checks do not impose function type predicates."""

import pytest

from tests._strict_integration import create_strict_project

_SUPPORT = """
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
"""

_MODELS = """
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
"""


@pytest.fixture(scope="module")
def nominal_dataclasses(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-dataclass-nominal-bindings"),
        {
            "nominal_dataclass_model.py": _MODELS,
            "nominal_dataclass_support.py": _SUPPORT,
        },
        modules={"nominal_dataclass_model": "nominal_dataclass_model.py"},
    )


_PRELUDE = """
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
"""


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generated_initvars_are_ordinary_but_actual_nominal_fields_are_checked(
    nominal_dataclasses, entry_interpreter
):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    nominal_dataclasses.run(
        _PRELUDE
        + f"assert _soac_ext.strict_function_entry_kind(model.Direct.__post_init__) == {expected!r}\n"
        + """
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
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_inherited_generated_initializers_preserve_distinct_local_class_ownership(
    nominal_dataclasses, entry_interpreter
):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    nominal_dataclasses.run(
        _PRELUDE
        + f"assert _soac_ext.strict_function_entry_kind(model.family) == {expected!r}\n"
        + """
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
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generated_nominal_factory_runs_once_and_assigns_its_actual_result(
    nominal_dataclasses, entry_interpreter
):
    nominal_dataclasses.run(
        _PRELUDE
        + """
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
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_self_nominal_slots_admits_only_selected_type_without_call_predicates(
    nominal_dataclasses, entry_interpreter
):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    nominal_dataclasses.run(
        _PRELUDE
        + f"assert _soac_ext.strict_function_entry_kind(model.self_slots) == {expected!r}\n"
        + """
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
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def named_self_dataclass(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-dataclass-initvar-named-self"),
        {
            "named_self_model.py": """
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import InitVar, dataclass
from nominal_dataclass_support import Target, post

@dataclass
class Record:
    self: InitVar[Target]
    payload: Target

    def __post_init__(self, seed):
        post(seed)
""",
            "nominal_dataclass_support.py": _SUPPORT,
        },
        modules={"named_self_model": "named_self_model.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_nominal_initvar_named_self_is_not_the_generated_receiver(
    named_self_dataclass, entry_interpreter
):
    named_self_dataclass.run(
        _PRELUDE.replace("nominal_dataclass_model", "named_self_model")
        + """
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
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def source_self_slots(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-dataclass-source-self-slots"),
        {
            "source_self_slots_model.py": """
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
import nominal_dataclass_support as support

class Probe:
    def __init_subclass__(cls):
        support.observe(cls)

def parameter_case():
    @dataclass(slots=True)
    class Node(Probe):
        value: int = 1

        def accept(self, other: Node) -> object:
            return other

    return Node

def return_case():
    @dataclass(slots=True)
    class Node(Probe):
        value: int = 1

        def accept(self) -> Node:
            return self

    return Node

def receiver_case():
    @dataclass(slots=True)
    class Node(Probe):
        value: int = 1

        def accept(self: Node) -> object:
            return self

    return Node
""",
            "nominal_dataclass_support.py": _SUPPORT,
        },
        modules={"source_self_slots_model": "source_self_slots_model.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("factory", ["parameter_case", "return_case", "receiver_case"])
def test_source_self_slots_keeps_shared_method_ownership_and_ordinary_calls(
    source_self_slots, entry_interpreter, factory
):
    source_self_slots.run(
        _PRELUDE.replace("nominal_dataclass_model", "source_self_slots_model")
        + f"factory_name = {factory!r}\n"
        + """
seal = api('PyFunction_GetSoacStrictId', ctypes.c_uint64)
support.observed.clear()
replacement = getattr(model, factory_name)()
assert len(support.observed) == 2
(original, original_bound), (observed_replacement, replacement_bound) = support.observed
assert replacement is observed_replacement and replacement is not original
assert not original_bound and not replacement_bound
assert not class_owner(original) and class_owner(replacement)
method = original.__dict__['accept']
assert method is replacement.__dict__['accept']
assert function_owner(method) and seal(method)
try:
    method.__code__ = method.__code__
except StrictMutationError:
    pass
else:
    raise AssertionError('shared source method metadata remained mutable')
good = replacement()
wrong = object()
if factory_name == 'parameter_case':
    assert method(good, good) is good
    assert method(original(), good) is good
    assert method(good, wrong) is wrong
    ordinary = original()
    assert method(good, ordinary) is ordinary
else:
    assert method(good) is good
    assert method(wrong) is wrong
    ordinary = original()
    assert method(ordinary) is ordinary
# Generated calls stay ordinary; their writes follow the receiver's storage.
for cls in (original, replacement):
    assert function_owner(cls.__init__)
    assert cls(7).value == 7
    foreign = original()
    assert cls.__init__(foreign, 'ordinary') is None
    assert foreign.value == 'ordinary'
    rejected_write(lambda: cls.__init__(good, 'ordinary'))
    assert good.value == 1
# Disposing the original class does not give it the replacement's field check.
assert original('ordinary').value == 'ordinary'
rejected_write(lambda: replacement('ordinary'))
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def cpython_nominal_dataclasses(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("cpython-dataclass-field-provider"),
        {
            "nominal_dataclass_model.py": _MODELS,
            "nominal_dataclass_support.py": _SUPPORT,
        },
        modules={"nominal_dataclass_model": "nominal_dataclass_model.py"},
        backend="cpython",
    )


def test_cpython_dataclass_field_initvar_and_factory_use_actual_native_globals(
    cpython_nominal_dataclasses,
):
    from pathlib import Path

    cpython_nominal_dataclasses.run_case(
        "nominal_dataclass_model",
        f"source = {_MODELS!r}\nfrom soac import _soac_ext\n"
        + _PRELUDE
        + """
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
""",
        Path(__file__),
        required_functions=("family",),
        
        backend="cpython",
    )


def test_cpython_dataclass_local_provider_forwarding_preserves_class_identity(
    cpython_nominal_dataclasses,
):
    from pathlib import Path

    # Each construction captures its own actual nominal field target. The
    # generated call's InitVar still has no runtime argument predicate.
    cpython_nominal_dataclasses.run_case(
        "nominal_dataclass_model",
        "from soac import _soac_ext\n"
        + _PRELUDE
        + """
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

""",
        Path(__file__),
        required_functions=("family",),
        
        backend="cpython",
    )


_CPYTHON_PENDING_SLOTS_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
from typing import Any
import pending_slots_observer as support

def make_node():
    @dataclass(slots=True)
    class Node:
        next: Node | None = None

        def accept(self, value: Node) -> Node:
            support.events.append('accept body')
            return value

    return Node
"""


def _pending_slots_source(field_annotation):
    # Both controls participate. Any deliberately has no write predicate;
    # the nominal control keeps the real construction-time Self requirement.
    assert field_annotation in {"Any", "Node | None"}
    return _CPYTHON_PENDING_SLOTS_SOURCE.replace(
        "next: Node | None = None", f"next: {field_annotation} = None"
    )

_CPYTHON_PENDING_SLOTS_OBSERVER = """
import dataclasses
import weakref

events = []
attempts = []
originals = []
weak_originals = []
keep_original = False
previous = None

class Recorder:
    def __init__(self):
        object.__setattr__(self, 'writes', [])

    def __setattr__(self, name, value):
        self.writes.append((name, value))

def observe(frame, event, arg):
    if event != 'call' or frame.f_code is not dataclasses._add_slots.__code__:
        return
    original = frame.f_locals['cls']
    if original.__name__ != 'Node':
        return
    weak_originals.append(weakref.ref(original))
    if keep_original:
        originals.append(original)
    receiver = Recorder()
    for name, values in (
        ('__init__', (receiver, object())),
        ('accept', (None, previous if previous is not None else object())),
    ):
        try:
            vars(original)[name](*values)
        except TypeError:
            attempts.append((name, 'rejected', tuple(receiver.writes)))
        else:
            attempts.append((name, 'entered', tuple(receiver.writes)))
"""


@pytest.fixture(scope="module")
def cpython_pending_slots(request, tmp_path_factory):
    field_annotation = getattr(request, "param", "Node | None")
    return create_strict_project(
        tmp_path_factory.mktemp("cpython-pending-slots-self"),
        {
            "pending_slots_model.py": _pending_slots_source(field_annotation),
            "pending_slots_observer.py": _CPYTHON_PENDING_SLOTS_OBSERVER,
        },
        modules={"pending_slots_model": "pending_slots_model.py"},
        backend="cpython",
    )


@pytest.mark.parametrize(
    ("cpython_pending_slots", "checked_field_writes"),
    [
        pytest.param("Any", False, id="any-field"),
        pytest.param("Node | None", True, id="nominal-field"),
    ],
    indirect=["cpython_pending_slots"],
    scope="module",
)
def test_cpython_dataclass_pending_calls_are_ordinary_and_selected_self_fields_are_checked(
    cpython_pending_slots, checked_field_writes,
):
    from pathlib import Path

    cpython_pending_slots.run_case(
        "pending_slots_model",
        f"source = {(cpython_pending_slots.project / 'pending_slots_model.py').read_text()!r}\n"
        f"checked_field_writes = {checked_field_writes!r}\n"
        + """
import ctypes
import sys
import types
import pending_slots_model as model
import pending_slots_observer as support
from soac import _soac_ext

def api(name, result):
    f = getattr(ctypes.pythonapi, name)
    f.argtypes = [ctypes.py_object]
    f.restype = result
    return f

owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
sealed = api('PyType_IsSoacSealed', ctypes.c_int)

stock = types.ModuleType('ordinary_pending_slots_control')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary pending slots control>', 'exec'), vars(stock))
support.keep_original = True
old_profile = sys.getprofile()
sys.setprofile(support.observe)
try:
    ordinary_selected = stock.make_node()
finally:
    sys.setprofile(old_profile)
assert not owner(ordinary_selected)
assert [row[:2] for row in support.attempts] == [
    ('__init__', 'entered'), ('accept', 'entered'),
]
assert support.attempts[0][2][0][0] == 'next'
assert support.events == ['accept body']
support.originals.clear()
support.weak_originals.clear()
support.events.clear()
support.attempts.clear()

sys.setprofile(support.observe)
try:
    first = model.make_node()
finally:
    sys.setprofile(old_profile)
assert [row[:2] for row in support.attempts] == [
    ('__init__', 'entered'), ('accept', 'entered'),
]
assert len(support.attempts[0][2]) == 1
assert support.attempts[0][2][0][0] == 'next'
assert support.attempts[0][2] == support.attempts[1][2]
assert support.events == ['accept body']
original = support.originals[0]
assert original is not first and owner(first) and sealed(first)
assert not owner(original), 'unselected original was admitted'
assert original.accept is first.accept and function_owner(first.accept)
assert not metadata(first.accept)
good = first()
assert first.accept(None, good) is good
ordinary = object.__new__(original)
ordinary.unrelated = 'ordinary dictionary after disposal'
assert vars(ordinary)['unrelated'] == 'ordinary dictionary after disposal'
assert original.accept(None, ordinary) is ordinary
assert first.accept(None, ordinary) is ordinary
assert original.accept(None, good) is good

# Repeating the same source creates a distinct class without a call predicate.
support.previous = good
support.events.clear()
support.attempts.clear()
sys.setprofile(support.observe)
try:
    second = model.make_node()
finally:
    sys.setprofile(old_profile)
assert first is not second and owner(second) and sealed(second)
assert [row[:2] for row in support.attempts] == [
    ('__init__', 'entered'), ('accept', 'entered'),
]
assert len(support.attempts[0][2]) == 1
assert support.attempts[0][2][0][0] == 'next'
assert support.attempts[0][2] == support.attempts[1][2]
assert support.events == ['accept body']
assert second.accept(None, good) is good
new = second()
assert second.accept(None, new) is new
assert first.accept(None, good) is good
assert _soac_ext.strict_function_diagnostics(first.accept)['original_code_entered']

from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1
from soac.strict import StrictMutationError
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
diagnostic = _soac_ext.strict_module_diagnostics(model)
for selected in (first, second):
    info = ConstructionInfoV1()
    assert get_construction(selected, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(selected) and info.owner is not None
    assert selected.__slots__ == ("next",)
    assert function_owner(selected.__init__) and not metadata(selected.__init__)
    observed = _assert_cpython_function_witness(
        selected.accept, diagnostic,
    )
    assert observed["original_code_entered"]

if checked_field_writes:
    generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
    generic_set.argtypes = [ctypes.py_object] * 3
    generic_set.restype = ctypes.c_int
    slot = vars(first)["next"]
    assert type(slot) is types.MemberDescriptorType
    good.next = good
    assert good.next is good
    assert generic_set(good, "next", None) == 0 and good.next is None
    assert generic_set(good, "next", good) == 0 and good.next is good
    for wrong in (ordinary, new, object()):
        try:
            first(wrong)
        except TypeError as error:
            assert not isinstance(error, StrictMutationError), error
        else:
            raise AssertionError('generated assignment bypassed the selected Self field')
        for write in (
            lambda: setattr(good, "next", wrong),
            lambda: object.__setattr__(good, "next", wrong),
            lambda: generic_set(good, "next", wrong),
            lambda: slot.__set__(good, wrong),
        ):
            try:
                write()
            except TypeError as error:
                assert not isinstance(error, StrictMutationError), error
            else:
                raise AssertionError("selected Self field accepted a different actual type")
            assert good.next is good
    new.next = new
    assert new.next is new
    # Disposition does not retroactively install the replacement's field
    # predicate on the original ordinary dictionary-backed type.
    ordinary.next = ordinary
    assert vars(ordinary)["next"] is ordinary and not owner(original)
else:
    # An explicit Any field has no predicate even though its class participates.
    unchecked = object()
    good.next = unchecked
    assert good.next is unchecked
    assert first(ordinary).next is ordinary
    assert second(good).next is good
""",
        Path(__file__),
        required_functions=("make_node",),
        
        backend="cpython",
    )


def test_cpython_dataclass_slots_original_is_not_kept_by_completion_metadata(cpython_pending_slots):
    from pathlib import Path

    cpython_pending_slots.run_case(
        "pending_slots_model",
        """
import ctypes
import gc
import sys
import pending_slots_model as model
import pending_slots_observer as support

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
support.keep_original = False
old_profile = sys.getprofile()
old_threshold = gc.get_threshold()
gc.set_threshold(1, 1, 1)
sys.setprofile(support.observe)
try:
    selected = model.make_node()
finally:
    sys.setprofile(old_profile)
    gc.set_threshold(*old_threshold)
assert support.originals == []
assert len(support.weak_originals) == 1
assert [row[:2] for row in support.attempts] == [
    ('__init__', 'entered'), ('accept', 'entered'),
]
assert len(support.attempts[0][2]) == 1
assert support.attempts[0][2][0][0] == 'next'
assert support.attempts[0][2] == support.attempts[1][2]
assert support.events == ['accept body']
gc.collect()
assert support.weak_originals[0]() is None
assert owner(selected), 'original retirement poisoned selected application'
instance = selected()
assert selected.accept(None, instance) is instance
""",
        Path(__file__),
        required_functions=("make_node",),
        
        backend="cpython",
    )



@pytest.mark.parametrize(
    ("entry_interpreter", "field_annotation"),
    [
        pytest.param(False, "Any", id="any-field-compiled"),
        pytest.param(True, "Any", id="any-field-entry"),
        pytest.param(False, "Node | None", id="nominal-field-compiled"),
        pytest.param(True, "Node | None", id="nominal-field-entry"),
    ],
)
def test_soac_untraced_slots_preserve_selected_self_fields(
    tmp_path, entry_interpreter, field_annotation
):
    from pathlib import Path

    project = create_strict_project(
        tmp_path,
        {
            "pending_slots_model.py": _pending_slots_source(field_annotation),
            "pending_slots_observer.py": _CPYTHON_PENDING_SLOTS_OBSERVER,
        },
        modules={"pending_slots_model": "pending_slots_model.py"},
        backend="soac",
    )
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    project.run_case(
        "pending_slots_model",
        f"source = {_pending_slots_source(field_annotation)!r}\n"
        f"checked_field_writes = {field_annotation != 'Any'!r}\n"
        f"expected_entry = {expected_entry!r}\n"
        + """
import ctypes
import sys
import types
import pending_slots_model as model
import pending_slots_observer as support
from soac import _soac_ext
from soac.strict import StrictMutationError
from tests.test_strict_type_native import ConstructionInfoV1

def api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
sealed = api('PyType_IsSoacSealed', ctypes.c_int)
assert _soac_ext.strict_function_entry_kind(model.make_node) == expected_entry

# CPython observer-positive original/final and pending-call controls stay above.
# SOAC proves actual final Self field ownership without an observer prerequisite.
assert support.attempts == support.originals == support.weak_originals == []
assert support.events == []

first = model.make_node()
assert owner(first) and sealed(first)
assert function_owner(first.accept) and metadata(first.accept)
assert _soac_ext.strict_function_entry_kind(first.accept) == expected_entry
good = first()
assert good.next is None
assert first.accept(None, good) is good
ordinary_return = object()
assert first.accept(None, ordinary_return) is ordinary_return
assert support.events == ['accept body', 'accept body']

# Repeated source construction creates independent actual Self field owners.
support.previous = good
support.events.clear()
second = model.make_node()
assert second is not first and owner(second) and sealed(second)
assert support.events == []
assert second.accept(None, good) is good
new = second()
assert new.next is None
assert second.accept(None, new) is new
assert first.accept(None, good) is good
assert support.events == ['accept body', 'accept body', 'accept body']
assert support.attempts == support.originals == support.weak_originals == []

get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
for selected in (first, second):
    info = ConstructionInfoV1()
    assert get_construction(selected, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(selected) and info.owner is not None
    assert selected.__slots__ == ('next',)
    assert function_owner(selected.__init__)
    assert function_owner(selected.accept) and metadata(selected.accept)
    assert _soac_ext.strict_function_entry_kind(selected.accept) == expected_entry

# The identical ordinary subject retains ordinary annotation behavior.
stock = types.ModuleType('ordinary_soac_pending_slots_control')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary SOAC pending slots control>', 'exec'), vars(stock))
ordinary_type = stock.make_node()
ordinary = ordinary_type()
assert not owner(ordinary_type) and not function_owner(ordinary_type.accept)
wrong_value = object()
ordinary.next = wrong_value
assert ordinary.next is wrong_value and ordinary_type(wrong_value).next is wrong_value

# A foreign generated-init receiver has no protected field, in either source control.
# Its ordinary explicit setattr callback still runs once with the original value.
recorder = support.Recorder()
assert first.__init__(recorder, wrong_value) is None
assert recorder.writes == [('next', wrong_value)]

if checked_field_writes:
    generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
    generic_set.argtypes = [ctypes.py_object] * 3
    generic_set.restype = ctypes.c_int
    slot = vars(first)['next']
    assert type(slot) is types.MemberDescriptorType
    good.next = good
    assert good.next is good
    assert generic_set(good, 'next', None) == 0 and good.next is None
    assert generic_set(good, 'next', good) == 0 and good.next is good
    for wrong in (ordinary, new, wrong_value):
        try:
            first(wrong)
        except TypeError as error:
            assert not isinstance(error, StrictMutationError), error
        else:
            raise AssertionError('generated assignment bypassed the selected Self field')
        for write in (
            lambda: setattr(good, 'next', wrong),
            lambda: object.__setattr__(good, 'next', wrong),
            lambda: generic_set(good, 'next', wrong),
            lambda: slot.__set__(good, wrong),
        ):
            try:
                write()
            except TypeError as error:
                assert not isinstance(error, StrictMutationError), error
            else:
                raise AssertionError('selected Self field accepted a different actual type')
            assert good.next is good
    new.next = new
    assert new.next is new
else:
    # Any is a source-level control with no predicate, not a policy exemption.
    good.next = wrong_value
    assert good.next is wrong_value
    assert first(ordinary).next is ordinary
    assert second(good).next is good
module_state = _soac_ext.strict_module_diagnostics(model)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']
""",
        Path(__file__),
        required_functions=("make_node",),
        entry_interpreter=entry_interpreter,
        backend="soac",
        opt_mode="none",
    )
