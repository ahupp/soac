"""Real stdlib transformations must preserve behavior and install native authority."""


import pytest


from tests._strict_integration import create_strict_project


_FIELD_WRITE_ASSERTIONS = """
from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')
"""


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
# soac: module(strict_assign=true, checked_attr=true)
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


_GENERATED_CHECK_ASSERTIONS = _FIELD_WRITE_ASSERTIONS + """
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
        # Authentication marks the compiled code internally; a source policy
        # comment alone does not give ordinary compile this flag or authority.
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


_MUTATION_MODEL = """
# soac: module(strict_assign=true, checked_attr=true)
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
    exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)\\n', ''),
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
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
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
