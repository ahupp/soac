"""Real stdlib transformations must preserve behavior and install native authority."""

import pytest

from tests._strict_integration import create_strict_project

_SUPPORT = """
events = []
classes = []
expect_pending = True

def new_items() -> list[int]:
    events.append('factory')
    return []

def post(seed: int) -> None:
    events.append(('post', seed))

def observe(cls):
    import ctypes
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    # The native owner is borrowed; ctypes must not take ownership of it.
    owner.restype = ctypes.c_void_p
    from soac.strict import StrictMutationError
    try:
        instance = object.__new__(cls)
    except StrictMutationError:
        assert expect_pending and not owner(cls)
        dictionary_bearing = bool(cls.__dictoffset__)
    else:
        assert not expect_pending, 'strict source type admitted before final selection'
        dictionary_bearing = hasattr(instance, '__dict__')
    classes.append((cls, bool(owner(cls)), dictionary_bearing))
"""

_MODELS = """
from __future__ import strict
from dataclasses import InitVar, dataclass, field
from typing import ClassVar
import adapter_support

@dataclass(slots=__SLOTS__)
class Base:
    first: int = 1

@dataclass(slots=__SLOTS__, weakref_slot=__SLOTS__, kw_only=True)
class Record(Base):
    value: int = 2
    items: list[int] = field(default_factory=adapter_support.new_items,
                            repr=False, compare=False)
    seed: InitVar[int] = 3
    shared: ClassVar[str] = 'classvar'

    def __post_init__(self, seed: int) -> None:
        adapter_support.post(seed)
        self.items.append(seed)

    def total(self) -> int:
        return self.first + self.value

@dataclass(slots=__SLOTS__, frozen=True, order=True)
class Frozen:
    x: int
    y: int = 2

@dataclass(slots=__SLOTS__, init=False, repr=False, eq=False,
           unsafe_hash=True, match_args=False)
class Manual:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value
"""

_CALLBACK_MODELS = """
from __future__ import strict
from dataclasses import dataclass
import adapter_support

@dataclass(slots=True)
class CallbackBase:
    marker: int = 1

    def __init_subclass__(cls):
        adapter_support.observe(cls)

@dataclass(slots=True)
class Observed(CallbackBase):
    value: int = 2
"""

_VERIFY_MODELS = """
import _testinternalcapi
import ctypes
import dataclasses
import reprlib
import sys
import types
import weakref
import adapter_support
import dataclass_model as model
from soac.strict import StrictMutationError

source_functions = (model.Record.__post_init__, model.Record.total, model.Manual.__init__)
entries_before = tuple(_soac_ext.strict_function_entry_kind(fn) for fn in source_functions)
stock = types.ModuleType('ordinary_dataclass_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict\\n', ''),
             '<ordinary dataclass control>', 'exec'), vars(stock))

def result_or_error(operation):
    try:
        return ('returned', operation())
    except Exception as error:
        return ('raised', type(error), str(error))

def exercise(module):
    adapter_support.events.clear()
    first = module.Record(4, value=8, seed=9)
    second = module.Record()
    assert first.items is not second.items
    assert weakref.ref(first)() is first
    assert first.total() == 12
    assert [field.name for field in dataclasses.fields(module.Record)] == [
        'first', 'value', 'items'
    ]
    if slots:
        assert not hasattr(first, '__dict__')
        assert module.Record.__slots__ == ('value', 'items', '__weakref__')
        assert module.Base.__slots__ == ('first',)
        assert type(vars(module.Record)['value']) is types.MemberDescriptorType
    else:
        assert type(vars(first)) is dict and vars(first) is first.__dict__
        assert list(vars(first)) == ['first', 'value', 'items']
    assert module.Record.__match_args__ == ('first',)
    assert 'seed' not in (vars(first) if not slots else module.Record.__slots__)
    assert 'shared' not in (vars(first) if not slots else module.Record.__slots__)
    constructor_error = result_or_error(lambda: module.Record(1, 2))
    assert constructor_error[0:2] == ('raised', TypeError)

    frozen = module.Frozen(5)
    assert frozen == module.Frozen(5) and frozen < module.Frozen(6)
    assert hash(frozen) == hash(module.Frozen(5))
    assign = result_or_error(lambda: setattr(frozen, 'x', 7))
    delete = result_or_error(lambda: delattr(frozen, 'x'))
    assert assign[0:2] == ('raised', dataclasses.FrozenInstanceError)
    assert delete[0:2] == ('raised', dataclasses.FrozenInstanceError)
    # Frozen dataclasses retain stock object.__setattr__ semantics. Field
    # checking is disabled in this fixture, independently of frozen=True.
    object.__setattr__(frozen, 'x', 8)
    assert frozen.x == 8

    manual = module.Manual(11)
    assert manual != module.Manual(11)
    assert hash(manual) == hash(module.Manual(11))
    assert '__repr__' not in vars(module.Manual)
    assert '__eq__' not in vars(module.Manual)
    assert '__match_args__' not in vars(module.Manual)
    option_names = (
        'init', 'repr', 'eq', 'order', 'unsafe_hash', 'frozen',
        'match_args', 'kw_only', 'slots', 'weakref_slot',
    )
    options = tuple(
        tuple(getattr(cls.__dataclass_params__, name) for name in option_names)
        for cls in (module.Base, module.Record, module.Frozen, module.Manual)
    )
    return (
        first.first, first.value, first.items, second.items,
        tuple(adapter_support.events), repr(first), constructor_error,
        assign, delete, options,
    )

assert exercise(model) == exercise(stock)

def api(name, arity, result=ctypes.c_int):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object] * arity
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', 1, ctypes.c_void_p)
class_sealed = api('PyType_IsSoacSealed', 1)
function_owner = api('PyFunction_GetSoacStrictOwner', 1, ctypes.c_void_p)

def rejected(operation):
    try:
        operation()
    except StrictMutationError:
        return
    raise AssertionError('native dataclass contract did not reject mutation')

diagnostic = _soac_ext.strict_module_diagnostics(model)
assert diagnostic is not None and diagnostic['sealed'] is True
assert entries_before == (expected_entry,) * len(source_functions), entries_before
assert tuple(_soac_ext.strict_function_entry_kind(fn) for fn in source_functions) == entries_before
for cls in (model.Base, model.Record, model.Frozen, model.Manual):
    assert class_owner(cls), (cls, 'dataclass silently used ordinary construction')
    assert class_sealed(cls) == 1
    rejected(lambda: setattr(cls, 'new_binding', object()))
for cls in (stock.Base, stock.Record, stock.Frozen, stock.Manual):
    assert not class_owner(cls)

for cls, names in (
    (model.Base, ('__init__', '__repr__', '__eq__')),
    (model.Record, ('__init__', '__repr__', '__eq__', '__post_init__', 'total')),
    (model.Frozen, ('__init__', '__repr__', '__eq__', '__lt__', '__le__',
                    '__gt__', '__ge__', '__hash__', '__setattr__', '__delattr__')),
    (model.Manual, ('__init__', '__hash__')),
):
    for name in names:
        function = vars(cls)[name]
        assert type(function) is types.FunctionType
        assert function_owner(function), (cls, name, 'generated ownership is absent')
        rejected(lambda: setattr(function, '__code__', function.__code__))

# Fresh owned components are adopted individually. This does not recursively
# freeze user factories, shared stdlib helpers, or arbitrary closure values.
metadata = api('PyFunction_GetSoacMetadata', 1, ctypes.c_void_p)
for cls in (model.Base, model.Record, model.Frozen):
    provider = cls.__init__.__annotate__
    assert type(provider) is types.FunctionType and function_owner(provider)
    assert not metadata(provider)
    rejected(lambda: setattr(provider, '__code__', provider.__code__))
    implementation = cls.__repr__.__wrapped__
    assert type(implementation) is types.FunctionType and function_owner(implementation)
    assert not metadata(implementation)
    rejected(lambda: setattr(implementation, '__code__', implementation.__code__))
for shared in (dataclasses._make_annotate_function, reprlib.recursive_repr,
               adapter_support.new_items):
    assert not function_owner(shared)
ordinary_repr = reprlib.recursive_repr()(lambda self: 'ordinary')
assert not function_owner(ordinary_repr)
ordinary_repr.__code__ = ordinary_repr.__code__
adapter_support.new_items.__code__ = adapter_support.new_items.__code__

# Sharing an implementation is not fresh-generation ownership. These stdlib
# functions must remain ordinary even when protected classes reference them.
assert model.Record.__replace__ is dataclasses._replace
assert not function_owner(dataclasses._replace)
if slots:
    assert model.Frozen.__getstate__ is dataclasses._dataclass_getstate
    assert model.Frozen.__setstate__ is dataclasses._dataclass_setstate
    assert not function_owner(dataclasses._dataclass_getstate)
    assert not function_owner(dataclasses._dataclass_setstate)
else:
    instance = model.Record()
    storage = vars(instance)
    assert _testinternalcapi.dict_has_indexed_keys(storage) is False
    assert _testinternalcapi.dict_has_indexed_keys(vars(model.Base())) is False
    storage['total'] = 'hidden dictionary value'
    assert instance.total() == 3
    rejected(lambda: setattr(instance, 'total', object()))
    assert storage is vars(instance)
"""


@pytest.fixture(scope="module", params=[False, True], ids=["dictionary", "slots"])
def dataclass_models(tmp_path_factory, request):
    slots = request.param
    source = _MODELS.replace("__SLOTS__", str(slots))
    project = create_strict_project(
        tmp_path_factory.mktemp(f"strict-dataclass-{'slots' if slots else 'dict'}"),
        {"dataclass_model.py": source, "adapter_support.py": _SUPPORT},
        modules={"dataclass_model": "dataclass_model.py"},
    )
    return project, source, slots


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_stdlib_dataclass_adapter_preserves_fields_options_and_generated_ownership(
    dataclass_models, entry_interpreter
):
    project, source, slots = dataclass_models
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    project.run(
        f"source = {source!r}\nslots = {slots!r}\nexpected_entry = {expected_entry!r}\n"
        + _VERIFY_MODELS,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_frozen_dataclass_pickle_uses_ordinary_shared_helpers(
    dataclass_models, entry_interpreter
):
    project, source, slots = dataclass_models
    project.run(
        f"source = {source!r}\nslots = {slots!r}\n"
        + """
import ctypes
import dataclasses
import pickle
import sys
import types
import dataclass_model as model

stock = types.ModuleType('ordinary_pickle_dataclass_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict\\n', ''),
             '<ordinary dataclass pickle control>', 'exec'), vars(stock))

def exercise(module):
    value = module.Frozen(5, 6)
    results = []
    for protocol in range(2, pickle.HIGHEST_PROTOCOL + 1):
        restored = pickle.loads(pickle.dumps(value, protocol=protocol))
        assert type(restored) is module.Frozen and restored == value
        results.append((restored.x, restored.y))
    return results

assert exercise(model) == exercise(stock)
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
if slots:
    for name in ('__getstate__', '__setstate__'):
        helper = getattr(dataclasses, '_dataclass' + name[1:-2])
        assert vars(model.Frozen)[name] is helper
        assert not owner(helper), 'a shared pickle helper acquired source ownership'
        helper.__code__ = helper.__code__
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def slotted_dataclass_callbacks(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-dataclass-replacement"),
        {
            "adapter_support.py": _SUPPORT,
            "dataclass_callbacks.py": _CALLBACK_MODELS,
        },
        modules={"dataclass_callbacks": "dataclass_callbacks.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_slotted_dataclass_original_and_replacement_stay_pending_through_callbacks(
    slotted_dataclass_callbacks, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    slotted_dataclass_callbacks.run(
        f"source = {_CALLBACK_MODELS!r}\nexpected_entry = {expected_entry!r}\n"
        + """
import ctypes
import sys
import types
import adapter_support
import dataclass_callbacks as model
from soac.strict import StrictMutationError

observed = tuple(adapter_support.classes)
adapter_support.classes.clear()
adapter_support.expect_pending = False
stock = types.ModuleType('ordinary_dataclass_callbacks')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict\\n', ''),
             '<ordinary dataclass replacement control>', 'exec'), vars(stock))
stock_observed = tuple(adapter_support.classes)
assert len(stock_observed) == 2
assert stock_observed[0][0] is not stock_observed[1][0]
assert stock_observed[1][0] is stock.Observed
assert tuple(event[2] for event in stock_observed) == (True, False)
assert not any(event[1] for event in stock_observed)
assert len(observed) == 2, observed
(original, original_owned, original_dict), (replacement, replacement_owned, replacement_dict) = observed
assert original is not replacement and replacement is model.Observed
assert original.__bases__ == replacement.__bases__ == (model.CallbackBase,)
assert original_dict is True and replacement_dict is False
assert not original_owned and not replacement_owned, 'a provisional acquired a permanent contract'
hook = model.CallbackBase.__init_subclass__.__func__
assert _soac_ext.strict_function_entry_kind(hook) == expected_entry
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert sealed(replacement) == 1 and sealed(original) == 0
owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes, owner.restype = [ctypes.py_object], ctypes.c_void_p
assert owner(replacement) and not owner(original)
try:
    replacement.new_binding = object()
except StrictMutationError:
    pass
else:
    raise AssertionError('selected dataclass lost its permanent contract')
original.new_binding = object()  # Exact resolved lineage permits dynamic disposal.
assert vars(original()) == {'value': 2}
assert not hasattr(replacement(), '__dict__')
assert replacement().marker == 1 and replacement().value == 2
""",
        entry_interpreter=entry_interpreter,
    )


_GENERATED_CHECK_SUPPORT = """
events = []
produced = 11
default_value = 7
factory_raises = False
factory_error = RuntimeError('ordinary factory failure')

def make_value() -> int:
    events.append('factory')
    if factory_raises:
        raise factory_error
    return produced
"""

_GENERATED_CHECK_MODELS = """
from __future__ import strict
from dataclasses import dataclass, field
import generated_check_support

@dataclass
class Factory:
    value: int = field(default_factory=generated_check_support.make_value)

@dataclass
class Default:
    value: int = generated_check_support.default_value

def make_watched():
    @dataclass
    class Watched:
        value: int

        def accept(self, value: int) -> int:
            generated_check_support.events.append(('source', value))
            return value

    return Watched
"""


@pytest.fixture(scope="module")
def generated_dataclass_checks(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-dataclass-generated-checks"),
        {
            "generated_check_support.py": _GENERATED_CHECK_SUPPORT,
            "generated_check_model.py": _GENERATED_CHECK_MODELS,
        },
        modules={"generated_check_model": "generated_check_model.py"},
    )


_GENERATED_CHECK_ASSERTIONS = """
import ctypes
import dataclasses
import generated_check_support as support

def assert_generated_owner(cls):
    class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    class_owner.argtypes = [ctypes.py_object]
    class_owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert class_owner(cls) and function_owner(cls.__init__)
    # Generated ownership does not manufacture source/JIT metadata.
    assert not metadata(cls.__init__)
"""


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_actual_dataclass_create_calls_preserve_source_and_generated_ownership(
    generated_dataclass_checks, function_create_watch_extension, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    generated_dataclass_checks.run(
        f"watch_extension = {str(function_create_watch_extension)!r}\n"
        f"expected_entry = {expected_entry!r}\n"
        + _GENERATED_CHECK_ASSERTIONS
        + """
import __future__
import importlib.util
import sys
import generated_check_model as model
from soac.strict import StrictMutationError, StrictRuntimeUnavailableError

spec = importlib.util.spec_from_file_location('_strict_function_create_watch', watch_extension)
watcher = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = watcher
spec.loader.exec_module(watcher)
assert _soac_ext.strict_module_diagnostics(model)['sealed']
assert _soac_ext.strict_function_entry_kind(model.make_watched) == expected_entry

# The observer really calls an ordinary newly created function, and its
# capture-only path does not. Neither filter value grants any strict role.
ordinary = {'events': []}
events = watcher.watch(ordinary, 'plain', (7,))
try:
    exec('def plain(value):\\n    events.append(value)\\n    return value\\n', ordinary)
finally:
    watcher.stop()
assert len(events) == 1 and events[0]['function'] is ordinary['plain']
assert events[0]['invoked'] and events[0]['success'] and events[0]['result'] == 7
assert not events[0]['owner_present'] and events[0]['source_id'] == 0
assert ordinary['events'] == [7]
ordinary['events'].clear()
events = watcher.watch(ordinary, 'plain', (7,), invoke=False)
try:
    exec('def plain(value):\\n    events.append(value)\\n', ordinary)
finally:
    watcher.stop()
assert len(events) == 1 and not events[0]['invoked']
assert events[0]['success'] is None and events[0]['result'] is None
assert ordinary['events'] == []

assignments = []
class Foreign:
    def __setattr__(self, name, value):
        assignments.append((name, value))
        object.__setattr__(self, name, value)

metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p

for name in ('accept', '__init__'):
    foreign = Foreign()
    support.events.clear()
    assignments.clear()
    events = watcher.watch(vars(model), name, (foreign, 'bad'))
    try:
        cls = model.make_watched()
    finally:
        watcher.stop()
    assert len(events) == 1, events
    row = events[0]
    function = vars(cls)[name]
    assert row['function'] is function and row['invoked']
    if name == 'accept':
        # This is the actual source MakeFunction before runtime attachment,
        # not a standalone native function manufactured by the test helper.
        assert row['source_id'] > 0 and row['flags'] & __future__.strict.compiler_flag
        assert not row['owner_present'] and row['creation'] == 0
        assert not row['success']
        assert isinstance(row['result'], StrictRuntimeUnavailableError)
        assert support.events == assignments == [] and vars(foreign) == {}
        assert metadata(function)
    else:
        # Its creation record precedes CREATE. This initializer has no closure
        # or defaults to install, so the ordinary call is already well formed.
        assert row['source_id'] == 0 and not row['flags'] & __future__.strict.compiler_flag
        assert row['owner_present'] and row['creation'] == 1
        assert not function.__code__.co_freevars
        assert row['success'] and row['result'] is None
        assert assignments == [('value', 'bad')] and support.events == []
        assert vars(foreign) == {'value': 'bad'}
        assert not metadata(function)
    assert_generated_owner(cls)
    assignments.clear()
    if name == 'accept':
        assert function(foreign, 'bad') == 'bad'
        assert support.events == [('source', 'bad')] and assignments == []
        support.events.clear()
        assert function(foreign, 7) == 7
        assert support.events == [('source', 7)] and assignments == []
        assert _soac_ext.strict_function_entry_kind(function) == expected_entry
    else:
        assert function(foreign, 'bad') is None
        assert assignments == [('value', 'bad')] and support.events == []
        assignments.clear()
        assert function(foreign, 7) is None
        assert assignments == [('value', 7)] and support.events == []
        assert vars(foreign) == {'value': 7}
    try:
        function.__code__ = function.__code__
    except StrictMutationError:
        pass
    else:
        raise AssertionError('the adopted observed function was not sealed')
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generated_dataclass_factory_marker_keeps_ordinary_expression_semantics(
    generated_dataclass_checks, entry_interpreter
):
    generated_dataclass_checks.run(
        _GENERATED_CHECK_ASSERTIONS
        + """
from generated_check_model import Factory
support.events.clear()
assert Factory('wrong').value == 'wrong'
assert support.events == []
assert Factory(dataclasses._HAS_DEFAULT_FACTORY).value == 11
assert support.events == ['factory']
support.events.clear()
assert Factory().value == 11 and support.events == ['factory']
support.events.clear()
assert Factory(9).value == 9 and support.events == []
assert_generated_owner(Factory)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generated_dataclass_factory_values_and_errors_preserve_foreign_assignment(
    generated_dataclass_checks, entry_interpreter
):
    generated_dataclass_checks.run(
        _GENERATED_CHECK_ASSERTIONS
        + """
from generated_check_model import Factory
class Foreign:
    pass

foreign = Foreign()
support.produced = 'wrong'
support.events.clear()
assert Factory.__init__(foreign) is None
assert support.events == ['factory'], 'factory execution must not be replayed'
assert foreign.value == 'wrong'
support.events.clear()
assert Factory.__init__(foreign, 'explicit') is None
assert support.events == [] and foreign.value == 'explicit'
assert Factory.__init__(foreign, 5) is None and foreign.value == 5
del foreign.value
support.factory_raises = True
support.events.clear()
try:
    Factory.__init__(foreign)
except RuntimeError as error:
    assert error is support.factory_error
else:
    raise AssertionError('ordinary factory exception was lost')
assert support.events == ['factory'] and not hasattr(foreign, 'value')
assert_generated_owner(Factory)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generated_dataclass_uses_the_actually_bound_nonfactory_default(
    generated_dataclass_checks, entry_interpreter
):
    generated_dataclass_checks.run(
        _GENERATED_CHECK_ASSERTIONS
        + """
# An ordinary consumed module can change a value without changing its signed
# source bytes. The actual default, not the checker's type, reaches binding.
support.default_value = 'wrong'
from generated_check_model import Default
assert Default().value == 'wrong'
assert Default(4).value == 4
assert_generated_owner(Default)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generated_dataclass_public_vectorcall_and_copies_keep_ordinary_calls(
    generated_dataclass_checks, entry_interpreter
):
    generated_dataclass_checks.run(
        _GENERATED_CHECK_ASSERTIONS
        + """
import types
from generated_check_model import Factory
assert_generated_owner(Factory)
function = Factory.__init__
vectorcall = ctypes.pythonapi.PyVectorcall_Function
vectorcall.argtypes = [ctypes.py_object]
vectorcall.restype = ctypes.c_void_p
setter = ctypes.pythonapi.PyFunction_SetVectorcall
setter.argtypes = [ctypes.py_object, ctypes.c_void_p]
setter.restype = None
original_entry = vectorcall(function)
stock_entry = ctypes.cast(ctypes.pythonapi._PyFunction_Vectorcall, ctypes.c_void_p).value
assert original_entry and stock_entry
for _ in range(128):
    assert Factory(4).value == 4

class Foreign:
    pass

foreign = Foreign()
setter(function, stock_entry)
try:
    assert function(foreign, 'ordinary') is None
    assert foreign.value == 'ordinary'
    assert Factory('ordinary').value == 'ordinary'
finally:
    setter(function, original_entry)
assert Factory(5).value == 5

# Ordinary public copies get no creation-record ownership, and their ordinary
# bytecode remains executable with ordinary value semantics.
copy = types.FunctionType(function.__code__, function.__globals__,
                          argdefs=function.__defaults__, closure=function.__closure__,
                          kwdefaults=function.__kwdefaults__)
assert copy(foreign, 'ordinary') is None and foreign.value == 'ordinary'
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert not owner(copy)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generated_dataclass_factory_conditional_observes_native_frame_changes(
    generated_dataclass_checks, entry_interpreter
):
    generated_dataclass_checks.run(
        _GENERATED_CHECK_ASSERTIONS
        + """
import sys
from generated_check_model import Factory
assert_generated_owner(Factory)
function = Factory.__init__
code = function.__code__
marker = dataclasses._HAS_DEFAULT_FACTORY
marker_cells = [cell for cell in function.__closure__ if cell.cell_contents is marker]
assert len(marker_cells) == 1
marker_cell = marker_cells[0]
events = []

class Foreign:
    pass

foreign = Foreign()
def change_marker(frame, event, argument):
    if frame.f_code is code and event == 'line':
        marker_cell.cell_contents = object()
        events.append('marker')
        return None
    return change_marker

support.events.clear()
sys.settrace(change_marker)
try:
    assert function(foreign) is None
finally:
    sys.settrace(None)
    marker_cell.cell_contents = marker
assert events and support.events == []
assert foreign.value is marker

def change_supplied(frame, event, argument):
    if frame.f_code is code and event == 'line':
        frame.f_locals['value'] = 'changed after entry'
        events.append('supplied')
        return None
    return change_supplied

sys.settrace(change_supplied)
try:
    assert function(foreign, 7) is None
finally:
    sys.settrace(None)
assert 'supplied' in events
assert foreign.value == 'changed after entry'
assert support.events == [], 'a supplied argument was misclassified as omitted'
assert Factory().value == 11
""",
        entry_interpreter=entry_interpreter,
    )


_MUTATION_MODEL = """
from __future__ import strict
from dataclasses import dataclass

@dataclass(init=False, eq=False)
class Record:
    value: int = 1

def make_record():
    @dataclass(init=False, eq=False)
    class Later:
        value: int = 1
    return Later
"""


@pytest.fixture(scope="module")
def dataclass_mutation_project(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-dataclass-producer-mutation"),
        {"mutated_dataclass_model.py": _MUTATION_MODEL},
        modules={"mutated_dataclass_model": "mutated_dataclass_model.py"},
        backend=getattr(request, "param", "soac"),
    )


_MUTATION_ASSERTIONS = """
import ctypes
import dataclasses
import importlib
import sys
import types
from soac.strict import StrictRuntimeUnavailableError, StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
has_contract = ctypes.pythonapi.PyType_HasSoacContract
has_contract.argtypes = [ctypes.py_object]
has_contract.restype = ctypes.c_int

def still_protected(cls):
    assert has_contract(cls) == 0, 'failed Pending acquired a permanent type contract'
    for allocate in (cls, lambda: object.__new__(cls)):
        try:
            allocate()
        except (StrictMutationError, StrictRuntimeUnavailableError):
            pass
        else:
            raise AssertionError('an escaped failed construction admitted an instance')

leaked = []
process_code = dataclasses._process_class.__code__
"""



def _dataclass_observer_witnesses(project, module_name, entry_interpreter):
    from pathlib import Path

    expected_entry = (
        "original_code" if project.backend == "cpython"
        else "entry_interpreter" if entry_interpreter else "checked_native"
    )
    # Direct runs need the same repository helper path as run_case validations.
    return (
        "import sys\n"
        f"sys.path.insert(0, {str(Path(__file__).resolve().parents[1])!r})\n"
        f"backend = {project.backend!r}\nexpected_entry = {expected_entry!r}\n"
        f"expected_source_path = {str(project.project / project.modules[module_name])!r}\n"
        f"expected_generation = {project.publication['generation']!r}\n"
        + """
def assert_observer_module(model):
    diagnostic = _soac_ext.strict_module_diagnostics(model)
    assert diagnostic is not None and diagnostic['sealed']
    assert diagnostic['backend'] == backend
    assert diagnostic['module_name'] == model.__name__
    assert diagnostic['source_path'] == expected_source_path
    assert diagnostic['artifact_generation'] == expected_generation
    if backend == 'cpython':
        assert diagnostic['initializer_entry_kind'] == 'original_code'
        assert diagnostic['original_code_entered']
        assert _soac_ext.runtime_compilation_activity() == {
            'schema': 1, 'lowering_entries': 0, 'blockpy_cache_entries': 0,
            'jit_engine_entries': 0,
        }
    else:
        assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
    return diagnostic

def assert_observer_type(cls):
    type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    type_owner.argtypes = [ctypes.py_object]
    type_owner.restype = ctypes.c_void_p
    type_sealed = ctypes.pythonapi.PyType_IsSoacSealed
    type_sealed.argtypes = [ctypes.py_object]
    type_sealed.restype = ctypes.c_int
    assert type_owner(cls) and type_sealed(cls) == 1

def assert_observer_function(model, function):
    diagnostic = assert_observer_module(model)
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    assert function_owner(function)
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry
    if backend == 'cpython':
        from tests._strict_integration import _assert_cpython_function_witness
        observed = _assert_cpython_function_witness(function, diagnostic)
        assert observed['finalized'] and observed['original_code_entered']
    else:
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert metadata(function)
"""
    )


@pytest.mark.parametrize(
    ("dataclass_mutation_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["dataclass_mutation_project"],
    scope="module",
)
def test_dataclass_source_roles_preserve_native_mutation_checks_and_untraced_construction(
    dataclass_mutation_project, entry_interpreter
):
    dataclass_mutation_project.run(
        _dataclass_observer_witnesses(
            dataclass_mutation_project, "mutated_dataclass_model", entry_interpreter
        )
        + f"source = {_MUTATION_MODEL!r}\n"
        + _MUTATION_ASSERTIONS
        + """
add_code = dataclasses._FuncBuilder.add_fn.__code__
modified = []

def trace(frame, event, argument):
    if event == 'call' and frame.f_code is process_code:
        leaked.append(frame.f_locals['cls'])
    if (event == 'call' and frame.f_code is add_code
            and frame.f_locals['name'] == '__repr__'):
        frame.f_locals['body'] = ["  return 'injected'"]
        modified.append('body')
    return trace

# Prove the same public frame-local mutation changes the ordinary producer.
stock = types.ModuleType('ordinary_mutated_dataclass_model')
sys.modules[stock.__name__] = stock
sys.settrace(trace)
try:
    exec(compile(source.replace('from __future__ import strict\\n', ''),
                 '<ordinary dataclass mutation>', 'exec'), vars(stock))
finally:
    sys.settrace(None)
assert modified == ['body'] and repr(stock.Record()) == 'injected'
assert not owner(stock.Record)

modified.clear()
leaked.clear()
failed = None
if backend == 'cpython':
    sys.settrace(trace)
    try:
        try:
            importlib.import_module('mutated_dataclass_model')
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError('mutated native producer unexpectedly completed')
    finally:
        sys.settrace(None)
    assert modified == ['body'] and len(leaked) == 1
    failed = leaked[0]
    still_protected(failed)
assert 'mutated_dataclass_model' not in sys.modules

# Keep genuine native mutation failure separate from untraced SOAC semantics.
# A fresh construction must remain healthy; a failed type is never re-admitted.
model = importlib.import_module('mutated_dataclass_model')
assert_observer_module(model)
assert owner(model.Record) and repr(model.Record()) == 'Record(value=1)'
later = model.make_record()
assert_observer_type(model.Record)
assert_observer_type(later)
assert repr(later()) == repr(stock.make_record()())
assert_observer_function(model, model.make_record)
if failed is not None:
    assert model.Record is not failed
    still_protected(failed)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("dataclass_mutation_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["dataclass_mutation_project"],
    scope="module",
)
def test_dataclass_member_creation_authentication_and_untraced_construction(
    dataclass_mutation_project, entry_interpreter
):
    dataclass_mutation_project.run(
        _dataclass_observer_witnesses(
            dataclass_mutation_project, "mutated_dataclass_model", entry_interpreter
        )
        + f"source = {_MUTATION_MODEL!r}\n"
        + _MUTATION_ASSERTIONS
        + """
has_creation = ctypes.pythonapi.PyFunction_HasSoacDataclassCreation
has_creation.argtypes = [ctypes.py_object]
has_creation.restype = ctypes.c_int
from _testinternalcapi import soac_function_create_watch, soac_function_create_unwatch
captured = []
changed = []
blocked = []
watcher = None

def replacement(self):
    return 'injected'

def trace(frame, event, argument):
    global watcher
    if event == 'call' and frame.f_code is process_code:
        leaked.append(frame.f_locals['cls'])
        assert watcher is None
        watcher = soac_function_create_watch(
            sys.modules['mutated_dataclass_model'].__dict__, '__repr__', captured
        )
    if event == 'line' and captured and not changed and not blocked:
        # A C-only CREATE watcher captures the actual fresh function and
        # preserves any pending exception before returning. Mutation happens
        # later, in an ordinary trace callback: the watcher API itself forbids
        # changing its watched function, and a ctypes callback cannot preserve
        # an error that was already pending before it entered Python.
        assert len(captured) == 1
        function = captured[0]
        assert has_creation(function) == 1
        try:
            function.__code__ = replacement.__code__
        except StrictMutationError:
            blocked.append(function)
        else:
            changed.append(function)
    return trace

unavailable = False
model = None
failed = None
if backend == 'cpython':
    sys.settrace(trace)
    try:
        try:
            model = importlib.import_module('mutated_dataclass_model')
        except StrictRuntimeUnavailableError:
            unavailable = True
    finally:
        sys.settrace(None)
        if watcher is not None:
            soac_function_create_unwatch(watcher)
    assert watcher is not None
    assert len(captured) == 1
    assert len(changed) + len(blocked) == 1
    assert len(leaked) == 1
    if changed:
        assert unavailable and model is None, 'the mutated created function was installed'
        failed = leaked[0]
        still_protected(failed)
    else:
        assert not unavailable and model is not None
        assert model.Record is leaked[0]
        assert_observer_type(model.Record)
        assert repr(model.Record()) == 'Record(value=1)'

if model is None:
    assert 'mutated_dataclass_model' not in sys.modules
    model = importlib.import_module('mutated_dataclass_model')
assert_observer_module(model)
assert owner(model.Record) and repr(model.Record()) == 'Record(value=1)'
later = model.make_record()
assert_observer_type(model.Record)
assert_observer_type(later)
stock = types.ModuleType('ordinary_watched_dataclass_recovery')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict', ''),
             '<ordinary dataclass watcher recovery>', 'exec'), vars(stock))
ordinary_later = stock.make_record()
assert not owner(stock.Record) and not owner(ordinary_later)
assert repr(later()) == repr(ordinary_later())
assert_observer_function(model, model.make_record)
if failed is not None:
    assert model.Record is not failed
    still_protected(failed)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("dataclass_mutation_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["dataclass_mutation_project"],
    scope="module",
)
def test_dataclass_construction_keeps_native_failure_barriers_and_later_owners(
    dataclass_mutation_project, entry_interpreter
):
    dataclass_mutation_project.run(
        _dataclass_observer_witnesses(
            dataclass_mutation_project, "mutated_dataclass_model", entry_interpreter
        )
        + f"source = {_MUTATION_MODEL!r}\n"
        + _MUTATION_ASSERTIONS
        + """
model = importlib.import_module('mutated_dataclass_model')
assert_observer_module(model)
assert _soac_ext.strict_function_entry_kind(model.make_record) == expected_entry
add_code = dataclasses._FuncBuilder.add_fn.__code__
modified = []

def trace(frame, event, argument):
    if event == 'call' and frame.f_code is process_code:
        leaked.append(frame.f_locals['cls'])
    if (event == 'call' and frame.f_code is add_code
            and frame.f_locals['name'] == '__repr__'):
        frame.f_locals['body'] = ["  return 'injected'"]
        modified.append('body')
    return trace

failed = None
if backend == 'cpython':
    sys.settrace(trace)
    try:
        try:
            model.make_record()
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError('mutated native construction unexpectedly completed')
    finally:
        sys.settrace(None)
    assert modified == ['body'] and len(leaked) == 1
    failed = leaked[0]
    still_protected(failed)

# A failed CPython construction stays alive. All backends exercise the same
# untraced construction; no stale pending-adoption record may supply its owner.
good = model.make_record()
assert good is not failed and owner(good)
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert sealed(good) == 1
stock = types.ModuleType('ordinary_later_dataclass_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict\\n', ''),
             '<ordinary later dataclass control>', 'exec'), vars(stock))
assert repr(good()) == repr(stock.make_record()())
if failed is not None:
    still_protected(failed)
assert_observer_function(model, model.make_record)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def generated_factory_sites(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-dataclass-factory-sites"),
        {
            "factory_site_support.py": """
events = []
keyword_value = 3
positional_value = 4

def items() -> list[int]:
    events.append('items')
    return []

def keyword() -> int:
    events.append('keyword')
    return keyword_value

def positional() -> int:
    events.append('positional')
    return positional_value
""",
            "mixed_factory_site.py": """
from __future__ import strict
from dataclasses import dataclass, field
import factory_site_support as support

@dataclass
class Value:
    checked: int = 1
    items: list[int] = field(default_factory=support.items)
""",
            "ordered_factory_sites.py": """
from __future__ import strict
from dataclasses import dataclass, field
import factory_site_support as support

@dataclass
class Value:
    keyword: int = field(default_factory=support.keyword, kw_only=True)
    positional: int = field(default_factory=support.positional)
""",
        },
        modules={
            "mixed_factory_site": "mixed_factory_site.py",
            "ordered_factory_sites": "ordered_factory_sites.py",
        },
    )


_FACTORY_SITE_ASSERTIONS = """
import ctypes
import factory_site_support as support

def adopted(cls):
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    assert owner(cls) and function_owner(cls.__init__)
"""


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generated_dataclass_preserves_independent_mutable_factory_results(
    generated_factory_sites, entry_interpreter
):
    generated_factory_sites.run(
        _FACTORY_SITE_ASSERTIONS
        + """
from mixed_factory_site import Value
adopted(Value)
support.events.clear()
assert Value('wrong').checked == 'wrong'
assert support.events == ['items']
support.events.clear()
first, second = Value(), Value()
assert first.checked == 1 and first.items == second.items == []
assert first.items is not second.items and support.events == ['items', 'items']
support.events.clear()
unselected = object()
assert Value(2, unselected).items is unselected
assert support.events == [], 'an explicitly supplied unchecked field invoked its factory'
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generated_factories_follow_assignment_order_not_parameter_order(
    generated_factory_sites, entry_interpreter
):
    generated_factory_sites.run(
        _FACTORY_SITE_ASSERTIONS
        + """
from ordered_factory_sites import Value
adopted(Value)
support.events.clear()
value = Value()
assert value.keyword == 3 and value.positional == 4
assert support.events == ['keyword', 'positional']
support.keyword_value = 'wrong'
support.events.clear()
value = Value()
assert value.keyword == 'wrong' and value.positional == 4
assert support.events == ['keyword', 'positional']
support.events.clear()
value = Value(9, keyword=8)
assert value.positional == 9 and value.keyword == 8
assert support.events == []
""",
        entry_interpreter=entry_interpreter,
    )


_SLOTS_LIFECYCLE_MODEL = """
from __future__ import strict
from dataclasses import dataclass
import adapter_support

class Probe:
    __slots__ = ()

    def __init_subclass__(cls):
        adapter_support.observe(cls)

    def base(self):
        return 4

def make_record():
    @dataclass(slots=True, weakref_slot=True)
    class Record(Probe):
        value: int = 3

        def read(self):
            return super().base() + self.value
    return Record

# The result is deliberately not a class-valued module binding. A weak
# construction record, not an inventory scan, must finalize the selected class.
adapter_support.held.append(make_record())
"""


@pytest.fixture(scope="module")
def slotted_dataclass_lifecycle(tmp_path_factory, request):
    backend = getattr(request, "param", "soac")
    return create_strict_project(
        tmp_path_factory.mktemp("strict-dataclass-slots-lifecycle"),
        {
            "adapter_support.py": _SUPPORT + "\nheld = []\n",
            "slot_lifecycle_model.py": _SLOTS_LIFECYCLE_MODEL,
            "slot_hybrid_model.py": """
from __future__ import strict
from dataclasses import dataclass

@dataclass
class DictionaryBase:
    value: int = 1

@dataclass(slots=True)
class Hybrid(DictionaryBase):
    other: int = 2
""",
            "slot_hybrid_unchecked_model.py": """
from __future__ import strict
from dataclasses import dataclass

class DictionaryBase:
    def __init__(self):
        self.value = 0

@dataclass(slots=True)
class Hybrid(DictionaryBase):
    value: int = 7
""",
        },
        modules={
            "slot_lifecycle_model": "slot_lifecycle_model.py",
            "slot_hybrid_model": "slot_hybrid_model.py",
            "slot_hybrid_unchecked_model": "slot_hybrid_unchecked_model.py",
        },
        policy="""
[tool.soac.strict]
include = ["slot_lifecycle_model.py", "slot_hybrid_model.py", "slot_hybrid_unchecked_model.py"]
checked_fields = "supported_annotations"
""",
        backend=backend,
    )


_SLOTS_APIS = """
import _testinternalcapi
import ctypes
from soac.strict import StrictMutationError, StrictRuntimeUnavailableError

def api(name, arity, result=ctypes.c_int):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object] * arity
    function.restype = result
    return function

has_contract = api('PyType_HasSoacContract', 1)
sealed = api('PyType_IsSoacSealed', 1)

def rejected(operation):
    try:
        operation()
    except (StrictMutationError, StrictRuntimeUnavailableError):
        return
    raise AssertionError('an actual slots contract accepted a forbidden mutation')

def bad_type(operation):
    try:
        operation()
    except TypeError:
        return
    raise AssertionError('a selected field or initializer accepted the wrong type')
"""


def _run_hybrid_dataclass_case(
    project, module_name, validation, *, entry_interpreter, source_functions=(),
):
    if project.backend == "soac":
        project.run(validation, entry_interpreter=entry_interpreter)
        return

    from pathlib import Path

    # Generated initializers have native dataclass ownership, not source/JIT
    # function diagnostics. The actual module still needs signed source proof.
    witness = f"source_functions = {source_functions!r}\n" + """
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

function_owner = api('PyFunction_GetSoacStrictOwner', 1, ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', 1, ctypes.c_void_p)
type_owner = api('PyType_GetSoacContractOwner', 1, ctypes.c_void_p)
diagnostic = _soac_ext.strict_module_diagnostics(model)
for cls in (model.DictionaryBase, model.Hybrid):
    assert type_owner(cls) and has_contract(cls) == 1 and sealed(cls) == 1
    initializer = vars(cls)['__init__']
    name = cls.__name__ + '.__init__'
    assert function_owner(initializer) and metadata(initializer) is None
    if name in source_functions:
        observed = _assert_cpython_function_witness(
            initializer, diagnostic,
        )
        assert observed['finalized'] and observed['original_code_entered']
    else:
        assert _soac_ext.strict_function_diagnostics(initializer) is None
    try:
        initializer.__code__ = initializer.__code__
    except StrictMutationError:
        pass
    else:
        raise AssertionError('admitted hybrid initializer metadata remained mutable')
"""
    project.run_case(
        module_name, validation + witness, Path(__file__),
        required_functions=source_functions,
        
        backend="cpython",
    )


@pytest.mark.parametrize(
    ("slotted_dataclass_lifecycle", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["slotted_dataclass_lifecycle"],
    scope="module",
)
def test_dataclass_hybrid_slots_and_inherited_dictionary_entries_are_independent(
    slotted_dataclass_lifecycle, entry_interpreter
):
    _run_hybrid_dataclass_case(
        slotted_dataclass_lifecycle, "slot_hybrid_model",
        _SLOTS_APIS
        + """
import types
import slot_hybrid_model as model

assert has_contract(model.DictionaryBase) == has_contract(model.Hybrid) == 1
assert sealed(model.DictionaryBase) == sealed(model.Hybrid) == 1
assert model.Hybrid.__slots__ == ('value', 'other')
member = vars(model.Hybrid)['value']
assert type(member) is types.MemberDescriptorType
value = model.Hybrid(3, 4)
storage = vars(value)
assert storage is value.__dict__ and type(storage) is dict
assert storage == {}, 'native member initialization was mirrored into the dictionary'
base_storage = vars(model.DictionaryBase())
assert _testinternalcapi.dict_has_indexed_keys(storage) is False
assert _testinternalcapi.dict_has_indexed_keys(base_storage) is False

storage['value'] = 10
assert storage['value'] == 10 and value.value == 3 and member.__get__(value) == 3
member.__set__(value, 11)
assert value.value == 11 and storage['value'] == 10
bad_type(lambda: member.__set__(value, 'wrong'))
bad_type(lambda: storage.__setitem__('value', 'wrong'))
bad_type(lambda: model.Hybrid('wrong', 4))
assert value.value == 11 and storage['value'] == 10
del storage['value']
assert value.value == 11
member.__delete__(value)
assert not hasattr(value, 'value') and storage == {}
storage['value'] = 12
assert not hasattr(value, 'value'), 'hidden dictionary storage escaped a native slot'
member.__set__(value, 13)
assert value.value == 13 and storage['value'] == 12
rejected(lambda: setattr(model.Hybrid, 'value', member))
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("slotted_dataclass_lifecycle", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["slotted_dataclass_lifecycle"],
    scope="module",
)
def test_dataclass_type_state_uses_the_final_native_slots_and_dictionary_projection(
    slotted_dataclass_lifecycle, entry_interpreter
):
    _run_hybrid_dataclass_case(
        slotted_dataclass_lifecycle, "slot_hybrid_model",
        _SLOTS_APIS
        + """
import gc
import weakref
import slot_hybrid_model as model

info = _testinternalcapi.get_soac_type_state_info
assert has_contract(model.Hybrid) == sealed(model.Hybrid) == 1
assert model.Hybrid.__slots__ == ('value', 'other')
first, second = model.Hybrid(3, 4), model.Hybrid(5, 6)
first_info, second_info = info(first), info(second)
assert first_info['has_slot'] and second_info['has_slot']
assert first_info['storage_mode'] == second_info['storage_mode'] == 'direct'
assert first_info['state_id'] == second_info['state_id']
assert first_info['extra_slot_bytes'] == ctypes.sizeof(ctypes.c_void_p)
dictionary = vars(first)
sibling = vars(second)
assert type(dictionary) is dict and dictionary == {}
assert info(dictionary)['has_slot'] and info(dictionary)['storage_mode'] == 'direct'
assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
assert info(sibling)['state_id'] == first_info['dictionary_state_id']
assert info(dictionary)['state_id'] != first_info['state_id']

member = vars(model.Hybrid)['value']
dictionary['value'] = 11
dictionary['other'] = 'hidden, not a native member value'
assert first.value == 3 and first.other == 4
bad_type(lambda: member.__set__(first, 'wrong'))
bad_type(lambda: object.__setattr__(first, 'other', 'wrong'))
bad_type(lambda: dictionary.__setitem__('value', 'wrong'))
assert first.value == 3 and dictionary['value'] == 11

# Dictionary replacement keeps its identity and legacy representation, while
# the original escaped dictionary independently keeps its projected contract.
incoming = {'value': 13, 'other': 'another hidden entry'}
incoming_id = id(incoming)
object.__setattr__(first, '__dict__', incoming)
assert vars(first) is incoming and id(incoming) == incoming_id
assert not info(incoming)['has_slot'] and info(incoming)['storage_mode'] == 'legacy'
assert first.value == 3 and first.other == 4
assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
bad_type(lambda: incoming.__setitem__('value', 'wrong'))
first_ref = weakref.ref(first)
del first
gc.collect()
assert first_ref() is None
dictionary.clear()
bad_type(lambda: dictionary.__setitem__('value', 'wrong after receiver death'))
dictionary['other'] = object()
bad_type(lambda: incoming.__setitem__('value', 'wrong after receiver death'))
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("slotted_dataclass_lifecycle", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["slotted_dataclass_lifecycle"],
    scope="module",
)
def test_dataclass_native_slot_checks_do_not_constrain_an_unchecked_inherited_dict_prefix(
    slotted_dataclass_lifecycle, entry_interpreter
):
    _run_hybrid_dataclass_case(
        slotted_dataclass_lifecycle, "slot_hybrid_unchecked_model",
        _SLOTS_APIS
        + """
import slot_hybrid_unchecked_model as model

assert has_contract(model.DictionaryBase) == has_contract(model.Hybrid) == 1
assert sealed(model.DictionaryBase) == sealed(model.Hybrid) == 1
value = model.Hybrid()
storage = vars(value)
member = vars(model.Hybrid)['value']
assert value.value == 7 and storage == {}
assert _testinternalcapi.dict_has_indexed_keys(storage) is False
assert _testinternalcapi.dict_has_indexed_keys(vars(model.DictionaryBase())) is False

# This position came from an unannotated base declaration. The new slot's
# selected int predicate must not become a requirement on hidden dict data.
storage['value'] = 'hidden'
assert storage['value'] == 'hidden' and member.__get__(value) == 7
set_item = api('PyDict_SetItem', 3)
assert set_item(storage, 'value', 'C hidden') == 0
assert storage['value'] == 'C hidden' and value.value == 7
bad_type(lambda: member.__set__(value, 'wrong'))
bad_type(lambda: object.__setattr__(value, 'value', 'wrong'))
bad_type(lambda: model.Hybrid('wrong'))
assert storage['value'] == 'C hidden' and value.value == 7
""",
        entry_interpreter=entry_interpreter,
        source_functions=("DictionaryBase.__init__",),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_dataclass_slots_decorator_result_is_released_without_method_calls(
    slotted_dataclass_lifecycle, entry_interpreter
):
    slotted_dataclass_lifecycle.run(
        f"expected_entry = {'entry_interpreter' if entry_interpreter else 'checked_native'!r}\n"
        + """
import gc
import weakref
from soac import _soac_ext
import adapter_support as support
import slot_lifecycle_model as model

assert _soac_ext.strict_function_entry_kind(model.make_record) == expected_entry
assert len(support.classes) == 2 and len(support.held) == 1
original = weakref.ref(support.classes[0][0])
replacement = weakref.ref(support.held[0])
assert original() is not replacement()
# No instance, source method, or generated method has run. The only caller
# references to the returned class are these ordinary observer containers.
support.classes.clear()
support.held.clear()
gc.collect()
assert original() is None, 'class construction retained the original class'
assert replacement() is None, 'compiled decorator-result cleanup retained the replacement'
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_dataclass_slots_module_drain_and_class_cell_repair_do_not_retain_originals(
    slotted_dataclass_lifecycle, entry_interpreter
):
    slotted_dataclass_lifecycle.run(
        f"source = {_SLOTS_LIFECYCLE_MODEL!r}\n"
        + _SLOTS_APIS
        + """
import gc
import sys
import types
import weakref
import adapter_support as support
import slot_lifecycle_model as model

assert _soac_ext.strict_module_diagnostics(model)['sealed']
assert len(support.classes) == 2 and len(support.held) == 1
original = support.classes[0][0]
replacement = support.held[0]
assert replacement is support.classes[1][0] and replacement is not original
assert not any(owned for _, owned, _ in support.classes), 'a provisional was permanently admitted'
assert has_contract(original) == 0 and sealed(original) == 0
assert sealed(replacement) == 1, 'a list-only selected result missed module drain'
assert vars(original)['read'] is vars(replacement)['read']
assert vars(original)['__init__'] is vars(replacement)['__init__']
method = vars(replacement)['read']
cell = method.__closure__[method.__code__.co_freevars.index('__class__')]
assert cell.cell_contents is replacement
assert replacement().read() == 7
# Ordinary repair intentionally changes zero-argument super on the shared
# original method too. Do not silently retarget its source owner to hide it.
try:
    original().read()
except TypeError:
    pass
else:
    raise AssertionError('the original method did not retain ordinary shared-cell behavior')
original_ref, replacement_ref = weakref.ref(original), weakref.ref(replacement)
support.classes.clear()
del original, method, cell
gc.collect()
assert original_ref() is None, 'the adapter retained the original class after ordinary repair'
support.held.clear()
del replacement
gc.collect()
assert replacement_ref() is None, 'a completed invocation retained its replacement class'

# Verify the lifetime/control behavior with the same stdlib transformation.
support.expect_pending = False
stock = types.ModuleType('ordinary_slot_lifecycle_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict\\n', ''),
             '<ordinary slots lifecycle control>', 'exec'), vars(stock))
ordinary_original = weakref.ref(support.classes[0][0])
ordinary_replacement = weakref.ref(support.held[0])
assert support.held[0]().read() == 7
support.classes.clear()
gc.collect()
assert ordinary_original() is None
support.held.clear()
gc.collect()
assert ordinary_replacement() is None
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("slotted_dataclass_lifecycle", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["slotted_dataclass_lifecycle"],
    scope="module",
)
def test_slots_apply_keeps_native_failure_barriers_and_untraced_construction(
    slotted_dataclass_lifecycle, entry_interpreter
):
    slotted_dataclass_lifecycle.run(
        _dataclass_observer_witnesses(
            slotted_dataclass_lifecycle, "slot_lifecycle_model", entry_interpreter
        )
        + _SLOTS_APIS
        + """
import dataclasses
import sys
import adapter_support as support
import slot_lifecycle_model as model

assert_observer_module(model)
support.classes.clear()
slots_code = dataclasses._add_slots.__code__
failure = RuntimeError('ordinary trace failure after replacement Ready')
trace_events = []

def trace(frame, event, argument):
    if frame.f_code is slots_code and event == 'return':
        trace_events.append('slots return')
        raise failure
    return trace

if backend == 'cpython':
    sys.settrace(trace)
    try:
        try:
            model.make_record()
        except RuntimeError as error:
            assert error is failure
        else:
            raise AssertionError('native traced slots construction unexpectedly completed')
    finally:
        sys.settrace(None)
    assert len(support.classes) == 2 and trace_events == ['slots return']
failed = tuple(cls for cls, _, _ in support.classes)
class Foreign:
    pass

for cls in failed:
    assert has_contract(cls) == 0
    rejected(cls)
    rejected(lambda: object.__new__(cls))
    # Failed type construction does not invent a call contract on fully
    # constructed generated code used with an ordinary foreign receiver.
    foreign = Foreign()
    assert vars(cls)['__init__'](foreign, 'ordinary') is None
    assert vars(foreign) == {'value': 'ordinary'}

support.classes.clear()
good = model.make_record()
assert good not in failed and good().read() == 7
assert sealed(good) == 1
assert len(support.classes) == 2
assert support.classes[1][0] is good
assert sealed(support.classes[0][0]) == 0 and sealed(good) == 1
for cls in failed:
    assert has_contract(cls) == 0
    rejected(cls)
    rejected(lambda: object.__new__(cls))
assert_observer_function(model, model.make_record)
""",
        entry_interpreter=entry_interpreter,
    )



@pytest.fixture(scope="module", params=[False, True], ids=["dictionary", "slots"])
def cpython_dataclass_models(tmp_path_factory, request):
    slots = request.param
    source = _MODELS.replace("__SLOTS__", str(slots))
    project = create_strict_project(
        tmp_path_factory.mktemp(f"cpython-dataclass-{'slots' if slots else 'dict'}"),
        {"dataclass_model.py": source, "adapter_support.py": _SUPPORT},
        modules={"dataclass_model": "dataclass_model.py"}, backend="cpython",
    )
    return project, source, slots


def test_cpython_backend_dataclass_behavior_and_actual_generated_ownership(cpython_dataclass_models):
    from pathlib import Path

    project, source, slots = cpython_dataclass_models
    project.run_case(
        "dataclass_model",
        f"source = {source!r}\nslots = {slots!r}\n"
        + """
import ctypes
import dataclasses
import reprlib
import sys
import types
import weakref
import adapter_support
import dataclass_model as model
from soac import _soac_ext
from soac.strict import StrictMutationError

stock = types.ModuleType('ordinary_dataclass_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict\\n', ''),
             '<ordinary dataclass control>', 'exec'), vars(stock))

def result_or_error(operation):
    try:
        return ('returned', operation())
    except Exception as error:
        return ('raised', type(error), str(error))

def exercise(module):
    adapter_support.events.clear()
    first = module.Record(4, value=8, seed=9)
    second = module.Record()
    assert first.items is not second.items
    assert weakref.ref(first)() is first
    assert first.total() == 12
    assert [field.name for field in dataclasses.fields(module.Record)] == [
        'first', 'value', 'items'
    ]
    if slots:
        assert not hasattr(first, '__dict__')
        assert module.Record.__slots__ == ('value', 'items', '__weakref__')
        assert module.Base.__slots__ == ('first',)
        assert type(vars(module.Record)['value']) is types.MemberDescriptorType
    else:
        assert type(vars(first)) is dict and vars(first) is first.__dict__
        assert list(vars(first)) == ['first', 'value', 'items']
    assert module.Record.__match_args__ == ('first',)
    assert 'seed' not in (vars(first) if not slots else module.Record.__slots__)
    assert 'shared' not in (vars(first) if not slots else module.Record.__slots__)
    constructor_error = result_or_error(lambda: module.Record(1, 2))
    assert constructor_error[0:2] == ('raised', TypeError)
    frozen = module.Frozen(5)
    assert frozen == module.Frozen(5) and frozen < module.Frozen(6)
    assert hash(frozen) == hash(module.Frozen(5))
    assign = result_or_error(lambda: setattr(frozen, 'x', 7))
    delete = result_or_error(lambda: delattr(frozen, 'x'))
    assert assign[0:2] == ('raised', dataclasses.FrozenInstanceError)
    assert delete[0:2] == ('raised', dataclasses.FrozenInstanceError)
    object.__setattr__(frozen, 'x', 8)
    assert frozen.x == 8
    manual = module.Manual(11)
    assert manual != module.Manual(11)
    assert hash(manual) == hash(module.Manual(11))
    assert '__repr__' not in vars(module.Manual)
    assert '__eq__' not in vars(module.Manual)
    assert '__match_args__' not in vars(module.Manual)
    option_names = (
        'init', 'repr', 'eq', 'order', 'unsafe_hash', 'frozen',
        'match_args', 'kw_only', 'slots', 'weakref_slot',
    )
    options = tuple(
        tuple(getattr(cls.__dataclass_params__, name) for name in option_names)
        for cls in (module.Base, module.Record, module.Frozen, module.Manual)
    )
    return (
        first.first, first.value, first.items, second.items,
        tuple(adapter_support.events), repr(first), constructor_error,
        assign, delete, options,
    )

# Same original subject and behavioral oracle as the existing SOAC family;
# storage-compatibility assertions remain in that retained-path test.
assert exercise(model) == exercise(stock)

def api(name, arity, result=ctypes.c_int):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object] * arity
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', 1, ctypes.c_void_p)
class_sealed = api('PyType_IsSoacSealed', 1)
function_owner = api('PyFunction_GetSoacStrictOwner', 1, ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', 1, ctypes.c_void_p)

def rejected(operation):
    try:
        operation()
    except StrictMutationError:
        return
    raise AssertionError('native dataclass contract did not reject mutation')

for cls in (model.Base, model.Record, model.Frozen, model.Manual):
    assert class_owner(cls) and class_sealed(cls) == 1
    rejected(lambda: setattr(cls, 'new_binding', object()))
for cls in (stock.Base, stock.Record, stock.Frozen, stock.Manual):
    assert not class_owner(cls)
for cls, names in (
    (model.Base, ('__init__', '__repr__', '__eq__')),
    (model.Record, ('__init__', '__repr__', '__eq__', '__post_init__', 'total')),
    (model.Frozen, ('__init__', '__repr__', '__eq__', '__lt__', '__le__',
                    '__gt__', '__ge__', '__hash__', '__setattr__', '__delattr__')),
    (model.Manual, ('__init__', '__hash__')),
):
    for name in names:
        function = vars(cls)[name]
        assert type(function) is types.FunctionType
        assert function_owner(function)
        assert metadata(function) is None
        rejected(lambda: setattr(function, '__code__', function.__code__))
for cls in (model.Base, model.Record, model.Frozen):
    provider = cls.__init__.__annotate__
    assert type(provider) is types.FunctionType and function_owner(provider)
    assert not metadata(provider)
    rejected(lambda: setattr(provider, '__code__', provider.__code__))
    implementation = cls.__repr__.__wrapped__
    assert type(implementation) is types.FunctionType and function_owner(implementation)
    assert not metadata(implementation)
    rejected(lambda: setattr(implementation, '__code__', implementation.__code__))
for shared in (dataclasses._make_annotate_function, reprlib.recursive_repr,
               adapter_support.new_items):
    assert not function_owner(shared)
ordinary_repr = reprlib.recursive_repr()(lambda self: 'ordinary')
assert not function_owner(ordinary_repr)
ordinary_repr.__code__ = ordinary_repr.__code__
adapter_support.new_items.__code__ = adapter_support.new_items.__code__
assert model.Record.__replace__ is dataclasses._replace
assert not function_owner(dataclasses._replace)
if slots:
    assert model.Frozen.__getstate__ is dataclasses._dataclass_getstate
    assert model.Frozen.__setstate__ is dataclasses._dataclass_setstate
    assert not function_owner(dataclasses._dataclass_getstate)
    assert not function_owner(dataclasses._dataclass_setstate)
else:
    instance = model.Record()
    storage = vars(instance)
    assert type(storage) is dict and list(storage) == ['first', 'value', 'items']
    storage['total'] = 'hidden dictionary value'
    assert instance.total() == 3
    rejected(lambda: setattr(instance, 'total', object()))
    assert storage is vars(instance)

for number in range(128):
    assert model.Record(number).total() == number + 2
call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
assert call(model.Record, (4,), {'value': 5}).total() == 9
assert call(model.Record, ('wrong',), {}).first == 'wrong'
assert stock.Record('ordinary').first == 'ordinary'
for function in (model.Record.__post_init__, model.Record.total, model.Manual.__init__):
    assert _soac_ext.strict_function_diagnostics(function)['original_code_entered'] is True
""",
        Path(__file__),
        required_functions=("Record.__post_init__", "Record.total", "Manual.__init__"),
        
        backend="cpython",
    )


def test_cpython_call_join_generic_dataclass_and_builtin_descriptor_births(tmp_path):
    from pathlib import Path

    source = """
from __future__ import strict
from dataclasses import dataclass

@dataclass
class Box[T]:
    value: int

class Operations:
    @staticmethod
    def static(value: int) -> int:
        return value

    @classmethod
    def class_(cls, value: int) -> int:
        return value

    @property
    def value(self) -> int:
        return 7
"""
    project = create_strict_project(
        tmp_path,
        {"generic_descriptor_model.py": source},
        modules={"generic_descriptor_model": "generic_descriptor_model.py"},
        backend="cpython",
    )
    project.run_case(
        "generic_descriptor_model",
        f"source = {source!r}\n"
        + """
import ctypes
import sys
import types
import typing
import generic_descriptor_model as model
from soac import _soac_ext
from tests.test_strict_type_native import ConstructionInfoV1

def api(name, result):
    f = getattr(ctypes.pythonapi, name)
    f.argtypes = [ctypes.py_object]
    f.restype = result
    return f

type_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
birth = api('PySoac_GetDescriptorBirthId', ctypes.c_uint64)
type_sealed = api('PyType_IsSoacSealed', ctypes.c_int)
construction_info = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
construction_info.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
construction_info.restype = ctypes.c_int

def no_native_class_contract(actual):
    info = ConstructionInfoV1()
    assert construction_info(actual, ctypes.byref(info), ctypes.sizeof(info)) == 0
    # A preconstruction decline has NO native state: it is not an unadmitted
    # Pending/Failed class or an already constructed phase-5 disposal.
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
    assert not type_owner(actual) and type_sealed(actual) == 0

stock = types.ModuleType('ordinary_generic_descriptor_control')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict', ''),
             '<ordinary generic descriptor control>', 'exec'), vars(stock))
assert not type_owner(stock.Box) and not type_owner(stock.Operations)
assert stock.Box('ordinary').value == 'ordinary'
assert stock.Operations.static('ordinary') == 'ordinary'
assert stock.Operations.class_('ordinary') == 'ordinary'
assert stock.Operations().value == model.Operations().value == 7
assert all(birth(vars(stock.Operations)[name]) == 0 for name in ('static', 'class_', 'value'))
# The real implicit Generic base is not a protected participating base.
# Box therefore stays an ordinary dataclass inside the still-strict module;
# no forced constructor/field authority is inferred from its int annotation.
no_native_class_contract(typing.Generic)
no_native_class_contract(model.Box)
assert model.Box.__bases__ == stock.Box.__bases__ == (typing.Generic,)
assert len(model.Box.__orig_bases__) == 1
assert typing.get_origin(model.Box.__orig_bases__[0]) is typing.Generic
assert typing.get_args(model.Box.__orig_bases__[0]) == model.Box.__type_params__
assert model.Box('ordinary').value == stock.Box('ordinary').value == 'ordinary'
assert not function_owner(model.Box.__init__) and not metadata(model.Box.__init__)
assert _soac_ext.strict_function_diagnostics(model.Box.__init__) is None

# The independent nongeneric class must still be genuinely admitted.
assert type_owner(model.Operations)
operations = ConstructionInfoV1()
assert construction_info(
    model.Operations, ctypes.byref(operations), ctypes.sizeof(operations)
) == 1
assert operations.abi_version == 1 and operations.struct_size == ctypes.sizeof(operations)
assert operations.phase == 3 and operations.permanent_contract_published == 1
assert operations.owner == type_owner(model.Operations) and operations.owner
assert type_sealed(model.Operations) == 1
module_witness = _soac_ext.strict_module_diagnostics(model)
assert module_witness['backend'] == 'cpython' and module_witness['sealed']
assert module_witness['initializer_entry_kind'] == 'original_code'
assert module_witness['original_code_entered'] is True

descriptors = [vars(model.Operations)[name] for name in ('static', 'class_', 'value')]
assert [type(value) for value in descriptors] == [staticmethod, classmethod, property]
assert all(birth(value) > 0 for value in descriptors)
assert len({birth(value) for value in descriptors}) == 3
for function in (descriptors[0].__func__, descriptors[1].__func__, descriptors[2].fget):
    assert function_owner(function) and not metadata(function)
for value in range(128):
    assert model.Box(value).value == value
    assert model.Operations.static(value) == value
    assert model.Operations.class_(value) == value
    assert model.Operations().value == 7
for invoke in (
    lambda: model.Operations.static('wrong'),
    lambda: model.Operations.class_('wrong'),
):
    assert invoke() == 'wrong'

call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
assert call(model.Operations.static, (5,), {}) == 5
assert call(model.Operations.static, ('wrong',), {}) == 'wrong'

assert call(model.Box, ('ordinary C argument',), {}).value == 'ordinary C argument'
for function in (descriptors[0].__func__, descriptors[1].__func__, descriptors[2].fget):
    witness = _soac_ext.strict_function_diagnostics(function)
    assert witness is not None and witness['backend'] == 'cpython'
    assert witness['entry_kind'] == 'original_code' and witness['original_code_entered']
    assert witness['finalized']
    for key in ('source_path', 'source_sha256', 'artifact_generation'):
        assert witness[key] == module_witness[key], (key, witness)
no_native_class_contract(model.Box)
no_native_class_contract(typing.Generic)
assert _soac_ext.runtime_compilation_activity() == {
    'schema': 1, 'lowering_entries': 0, 'blockpy_cache_entries': 0,
    'jit_engine_entries': 0,
}
""",
        Path(__file__),
        required_functions=("Operations.static", "Operations.class_"),
        
        backend="cpython",
    )


def test_cpython_dataclass_postclear_completion_error_uses_caller_handlers_and_traceback(tmp_path):
    from pathlib import Path

    source = """
from __future__ import strict
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
"""
    support = """
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
"""
    project = create_strict_project(
        tmp_path,
        {"postclear_model.py": source, "postclear_observer.py": support},
        modules={"postclear_model": "postclear_model.py"},
        backend="cpython",
    )
    project.run_case(
        "postclear_model",
        f"source = {source!r}\n"
        + """
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

# Exact ordinary source control: only the future opt-in is absent. A normal
# dataclass accepts this late mutation and returns the resulting ordinary type.
stock = types.ModuleType('ordinary_postclear_control')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict', ''),
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
assert selected('wrong').value == 'wrong'
assert support.events == ['finally']
assert _soac_ext.strict_function_diagnostics(model.build)['original_code_entered']
""",
        Path(__file__),
        required_functions=("build",),
        
        backend="cpython",
    )


@pytest.mark.parametrize("mutation", ["after_capture", "before_capture"])
def test_cpython_dataclass_compiler_uses_actual_captured_exec_globals(tmp_path, mutation):
    from pathlib import Path

    source = """
from __future__ import strict
from dataclasses import dataclass

def make():
    @dataclass
    class Item:
        value: int = 3
    return Item
"""
    project = create_strict_project(
        tmp_path,
        {"captured_exec_globals.py": source},
        modules={"captured_exec_globals": "captured_exec_globals.py"},
        backend="cpython",
    )
    project.run_case(
        "captured_exec_globals",
        f"source = {source!r}\nmutation = {mutation!r}\n"
        + """
import ctypes
import dataclasses
import sys
import types
import captured_exec_globals as model
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError

stock = types.ModuleType('ordinary_captured_exec_globals')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('from __future__ import strict', ''),
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
    assert operation().value == 'wrong'
assert _soac_ext.strict_function_diagnostics(model.make)['original_code_entered']
""",
        Path(__file__),
        required_functions=("make",),
        
        backend="cpython",
    )


@pytest.mark.parametrize("slots", [False, True], ids=["dictionary", "slots"])
@pytest.mark.parametrize("failure", ["body", "postclear"])
def test_cpython_failed_dataclass_cleanup_preserves_primary_and_escaped_barriers(
    tmp_path, slots, failure
):
    from pathlib import Path

    source = f"""
from __future__ import strict
from dataclasses import dataclass
import failed_apply_observer as support

class Stable:
    def value(self) -> int:
        return 17

def build():
    try:
        @dataclass(slots={slots!r})
        class Subject:
            value: int = 3
    except Exception as error:
        support.caught.append(error)
        support.events.append('caught')
        return None
    finally:
        support.events.append('finally')
    return Subject
"""
    support = """
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
"""
    project = create_strict_project(
        tmp_path,
        {"failed_apply_model.py": source, "failed_apply_observer.py": support},
        modules={"failed_apply_model": "failed_apply_model.py"},
        backend="cpython",
    )
    project.run_case(
        "failed_apply_model",
        f"source = {source!r}\nslots = {slots!r}\nfailure = {failure!r}\n"
        + """
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
exec(compile(source.replace('from __future__ import strict', ''),
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
assert selected('wrong').value == 'wrong'
assert support.events == ['finally']
assert owner(model.Stable) == stable_owner and model.Stable().value() == 17
for actual in failed:
    still_failed(actual)
assert _soac_ext.strict_function_diagnostics(model.build)['original_code_entered']
""",
        Path(__file__),
        required_functions=("build", "Stable.value"),
        
        backend="cpython",
    )
