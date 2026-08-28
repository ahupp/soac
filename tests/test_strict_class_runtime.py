"""Behavior at the genuine offline-artifact/native-construction boundary."""

import json
import sys
from pathlib import Path

import pytest

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import create_strict_project


@pytest.fixture(scope="module")
def cached_empty_annotations(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-empty-annotation-cache"),
        {
            "empty_annotation_cache.py": """
# soac: module(strict_assign=true, checked_attr=true)
from annotationlib import get_annotations

class Plain:
    def method(self, value: int) -> int:
        return value

# Introspection of an unannotated class lazily publishes native cache entries,
# including __annotate_func__ = None, before module sealing.
assert Plain.__annotate__ is None
assert Plain.__annotations__ == {}
assert get_annotations(Plain) == {}

class Annotated:
    value: int = 1

assert get_annotations(Annotated) == {'value': int}
""",
        },
        modules={"empty_annotation_cache": "empty_annotation_cache.py"},
        backend=getattr(request, "param", "soac"),
    )


# Retained harness: Reads profile.bin observations from the actual native construction path;
# requires profile artifact inspection.
@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_native_class_construction_retains_split_key_profile_observation(
    cached_empty_annotations, tmp_path, entry_interpreter
):
    """The real class path still observes ordinary dictionary key insertion."""
    work = tmp_path / "soac-work"
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    cached_empty_annotations.run(
        f"""
import ctypes
import empty_annotation_cache as module

sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert sealed(module.Plain) == 1
assert _soac_ext.strict_function_entry_kind(module.Plain.method) == {expected_entry!r}
instance = module.Plain()
instance.first = 11
instance.second = 12
assert vars(instance) == {{'first': 11, 'second': 12}}
assert instance.method(3) == 3
""",
        opt_mode="profile",
        entry_interpreter=entry_interpreter,
        extra_env={"SOAC_WORK_DIR": str(work)},
    )
    from soac import _soac_ext

    dump = json.loads(_soac_ext.inspect_counter_dump_json(str(work / "profile.bin")))
    owner_ids = {
        owner["type_id"]
        for record in dump["records"]
        for owner in record["type_table"]
        if owner["module_name"] == "empty_annotation_cache"
        and owner["qualname"] == "Plain"
    }
    assert owner_ids, "native class construction did not install the key-layout observer"
    keys = {
        item["key"]: item["index"]
        for record in dump["records"]
        for item in record["type_keys"]
        if item["owner_type_id"] in owner_ids
    }
    assert keys["first"] < keys["second"], keys


@pytest.fixture(scope="module")
def explicit_slots_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-explicit-slots"),
        {
            "slot_model.py": """
# soac: module(strict_assign=true, checked_attr=true)
import slot_support

class Probe:
    __slots__ = ()

    def __init_subclass__(cls):
        slot_support.observe(cls)

class Base(Probe):
    __slots__ = ('value', '__weakref__')
    value: int

    def __init__(self, value: int):
        self.value = value

    def read(self) -> int:
        return self.value

class Child(Base):
    __slots__ = ('other',)
    other: str

    def __init__(self, value: int, other: str):
        self.value = value
        self.other = other

    def text(self) -> str:
        return self.other

class WithDictionary(Base):
    extra: int

    def set_extra(self, value: int):
        self.extra = value
""",
            "slot_support.py": """
observations = []
ordinary_observations = []
phase = 'pending'

def observe(cls):
    import ctypes
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    from soac.strict import StrictMutationError
    if phase == 'ordinary_subclass':
        # The ordinary driver selects this phase explicitly after strict module
        # initialization. Absence of an owner never selects the phase: Pending
        # source classes also have no permanent owner at this callback.
        assert not owner(cls), 'ordinary subclass acquired its own type contract'
        assert type(object.__new__(cls)) is cls, 'ordinary subclass retained a pending barrier'
        ordinary_observations.append(cls)
        return
    assert phase == 'pending', phase
    assert not owner(cls), 'the provisional type acquired a permanent contract'
    try:
        object.__new__(cls)
    except StrictMutationError:
        blocked = True
    else:
        raise AssertionError('a pending slots type admitted an instance')
    observations.append((cls, bool(owner(cls)), bool(cls.__dictoffset__), blocked))
""",
        },
        modules={"slot_model": "slot_model.py"},
    )


# Retained harness: Uses profile/apply/verify artifacts, emitted binding evidence and field-
# access counters.
def test_native_slot_reads_select_guarded_members_and_keep_lookup_fallback(
    explicit_slots_project, tmp_path
):
    work = tmp_path / "native-slot-reads"
    training = """
import ctypes
import slot_model as model

sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(model)['sealed']
assert sealed(model.Base) and sealed(model.Child)
assert _soac_ext.strict_function_entry_kind(model.Base.read) == 'checked_native'
assert _soac_ext.strict_function_entry_kind(model.Child.text) == 'checked_native'
base, child = model.Base(7), model.Child(11, 'member')
for unused in range(250):
    assert base.read() == 7 and child.text() == 'member'
"""
    explicit_slots_project.run(
        training, opt_mode="profile", extra_env={"SOAC_WORK_DIR": str(work)}
    )
    assert (work / "profile.bin").is_file()
    validation = training + """
base.value = 19
assert base.read() == 19
del base.value
try:
    base.read()
except AttributeError:
    pass
else:
    raise AssertionError('an unbound native member skipped normal lookup')
base.value = 23
assert base.read() == 23

# A nominal receiver is not an exact layout witness. An ordinary subclass
# may override the descriptor; the guarded load must run that lookup.
import slot_support as support
events = []
support.phase = 'ordinary_subclass'
try:
    class Ordinary(model.Base):
        @property
        def value(self):
            events.append('property')
            return 31
finally:
    support.phase = 'pending'
assert support.ordinary_observations == [Ordinary]

ordinary = object.__new__(Ordinary)
assert ordinary.read() == 31 and events == ['property']
# An admitted derived type also keeps the conservative exact-owner fallback.
assert child.read() == 11
"""
    events_path = tmp_path / "native-slot-apply.jsonl"
    explicit_slots_project.run(
        validation,
        opt_mode="apply",
        extra_env={
            "SOAC_WORK_DIR": str(work),
            "SOAC_LOG": f"soac_jit_codegen=info;json={events_path}",
        },
    )
    events = [json.loads(line) for line in events_path.read_text().splitlines()]
    events = [entry.get("fields", entry) for entry in events]
    emitted = {
        event["function_qualname"]: event
        for event in events
        if event.get("event") == "soac.strict_field_codegen"
    }
    bound = {
        event["function_qualname"]: event
        for event in events
        if event.get("event") == "soac.strict_field_capabilities"
    }
    for name in ("Base.read", "Child.text"):
        assert emitted[name]["sealed_field_site_count"] == 1
        assert emitted[name]["machine_code_size_bytes"] > 0
        assert bound[name]["native_object_slot_count"] == 1
        assert bound[name]["indexed_dictionary_slot_count"] == 0

    # Verify records branch use separately from the production apply run.
    explicit_slots_project.run(
        validation, opt_mode="verify", extra_env={"SOAC_WORK_DIR": str(work)}
    )
    from soac import _soac_ext

    verification = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work / "verify.bin"))
    )
    read_paths = {
        branch
        for record in verification["records"]
        if record["module_name"] == "slot_model"
        for row in record["rows"]
        if row["function_qualname"] == "Base.read"
        and row["kind"] == "field_access"
        for branch, value in row["branches"].items()
        if value > 0
    }
    # These existing counter names cover both indexed-dictionary and native
    # object-member capabilities; the binding event above proves the kind.
    assert {"indexed_hit", "indexed_fallback"} <= read_paths


# Retained harness: Deletes sys.modules and the final module reference to prove module
# collection while a class/dictionary escapes. Scenario pre/post witnesses intentionally retain
# every imported module.
def test_sealed_class_and_detached_dictionary_preserve_object_lifetimes(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "support.py": """
events = []

class Token:
    def __del__(self):
        events.append('token released')
""",
            "model.py": """
# soac: module(strict_assign=true, checked_attr=true)
import support

token = support.Token()

class Bare:
    value = 3
""",
        },
        modules={"model": "model.py"},
    )
    project.run(
        """
import gc
import sys
import weakref
import model
import support
from soac.strict import StrictMutationError

bare = model.Bare
module_ref = weakref.ref(model)
token_ref = weakref.ref(model.token)
del sys.modules['model']
del model
gc.collect()
assert module_ref() is None
assert token_ref() is None and support.events == ['token released']
instance = bare()
assert instance.value == 3
instance.value = 10
dictionary = vars(instance)
type_ref = weakref.ref(bare)
instance_ref = weakref.ref(instance)
del instance
assert instance_ref() is None
del bare
gc.collect()
assert type_ref() is None
assert dictionary == {'value': 10}
dictionary['value'] = 11
dictionary.clear()
assert dictionary == {}
"""
    )


# Retained harness: Installs a native entry observer before importing the strict subject and
# releases/joins an active worker around that import, including failure cleanup. Eager scenario
# imports cannot preserve that validation ordering.
@pytest.mark.parametrize(
    ("backend", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
)
def test_active_class_method_call_keeps_ordinary_semantics_across_admission(
    tmp_path, backend, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "admission_support.py": """
import threading

entered = threading.Event()
released = threading.Event()
outcomes = []
worker = None

def pause():
    entered.set()
    if not released.wait(10):
        raise AssertionError('class construction did not release active method')

def begin(function):
    global worker
    def call():
        try:
            outcomes.append(function(None, 'already active'))
        except BaseException as error:
            outcomes.append(error)
            entered.set()
    worker = threading.Thread(target=call)
    worker.start()
    if not entered.wait(10):
        released.set()
        worker.join(10)
        raise AssertionError('source method did not reach its body')

def finish():
    released.set()
    worker.join(10)
    assert not worker.is_alive()
""",
            "admission_model.py": """
# soac: module(strict_assign=true, checked_attr=true)
import admission_support as support

class Model:
    def method(self, value: int) -> int:
        support.pause()
        return value

    support.begin(method)
""",
        },
        modules={"admission_model": "admission_model.py"},
        backend=backend,
    )
    program = """
import ctypes
import admission_support as support
try:
    import admission_model as model
finally:
    support.finish()

assert support.outcomes == ['already active'], support.outcomes
function_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
function_identity.argtypes = [ctypes.py_object]
function_identity.restype = ctypes.c_uint64
assert function_identity(model.Model.method) != 0
assert model.Model().method('later call') == 'later call'
assert model.Model().method(3) == 3
"""
    if backend == "cpython":
        import hashlib
        from tests._strict_integration import ROOT

        source_path = project.project / "admission_model.py"
        before = f"""
import ctypes
import admission_support as support
sys.path.insert(0, {str(ROOT)!r})
from tests._strict_integration import (
    _assert_cpython_function_witness, _assert_cpython_module_witness,
)
original_begin = support.begin
entry_observations = []

def begin_with_native_witness(function):
    diagnostic = _soac_ext.strict_module_diagnostics(sys.modules["admission_model"])
    assert diagnostic is not None and diagnostic["backend"] == "cpython"
    assert not diagnostic["sealed"] and diagnostic["original_code_entered"]
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert not observed["finalized"]
    entry_observations.append(("before", observed["original_code_entered"]))
    original_begin(function)
    # The real worker is inside the source body, while the class suite has not
    # yet returned to its actual native construction/admission boundary.
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert not observed["finalized"]
    entry_observations.append(("inside", observed["original_code_entered"]))

support.begin = begin_with_native_witness
"""
        after = f"""
assert entry_observations == [("before", False), ("inside", True)]
diagnostic = _assert_cpython_module_witness(
    model, module_name="admission_model", source_path={str(source_path)!r},
    source_sha256={hashlib.sha256(source_path.read_bytes()).hexdigest()!r},
    artifact_generation={project.publication["generation"]!r},
)
observed = _assert_cpython_function_witness(
    model.Model.method, diagnostic,
)
assert observed["finalized"] and observed["original_code_entered"]
assert function_identity(model.Model.method) != 0
type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
type_owner.argtypes = [ctypes.py_object]
type_owner.restype = ctypes.c_void_p
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert type_owner(model.Model) and sealed(model.Model) == 1
"""
        program = before + program + after
    project.run(program, entry_interpreter=entry_interpreter, backend=backend)


_FAILED_NAMESPACE_SUPPORT = """
import weakref

events = []
references = []

class Payload:
    def __init__(self):
        events.append('created')
        references.append(weakref.ref(self))

    def __del__(self):
        events.append('released')
"""

_FAILED_NAMESPACE_SOURCES = {
    "failed_class_namespace": """
import namespace_failure_support as support

def fail_class():
    class Broken:
        value = support.Payload()
        raise ValueError('namespace failure')
""",
    "failed_module_namespace": """
import namespace_failure_support as support

value = support.Payload()
raise ValueError('namespace failure')
""",
}

_FAILED_NAMESPACE_CHECK = """
def check_failed_namespace(action, failed_import=None, *, native_frames=True):
    import gc
    import sys
    import namespace_failure_support as support

    try:
        action()
    except ValueError as error:
        assert type(error) is ValueError and error.args == ('namespace failure',)
        retained_traceback = error.__traceback__
    else:
        raise AssertionError('source body did not raise its original ValueError')

    # The ordinary CPython control retains its source namespace through the
    # traceback. SOAC does not reconstruct or retain a source frame for this.
    if native_frames:
        assert retained_traceback is not None
    if failed_import is not None:
        assert failed_import not in sys.modules, 'failed import remained published'
    assert len(support.references) == 1, support.events
    reference = support.references[0]
    gc.collect()
    if native_frames:
        assert reference() is not None, ('ordinary traceback lost namespace owner', support.events)
        assert support.events == ['created'], support.events

    retained_traceback = None
    gc.collect()
    assert reference() is None, ('namespace survived traceback release', support.events)
    assert support.events == ['created', 'released'], support.events
"""


# Retained harness: Parameterized ordinary control includes a module initializer that must fail
# inside the traceback/cleanup observer. Declared scenario module imports must succeed before
# validation.
@pytest.mark.parametrize("module_name", tuple(_FAILED_NAMESPACE_SOURCES))
def test_failed_namespace_traceback_native_control(tmp_path, monkeypatch, module_name):
    (tmp_path / "namespace_failure_support.py").write_text(
        _FAILED_NAMESPACE_SUPPORT.lstrip("\n")
    )
    (tmp_path / f"{module_name}.py").write_text(
        _FAILED_NAMESPACE_SOURCES[module_name].lstrip("\n")
    )
    monkeypatch.syspath_prepend(tmp_path)
    if module_name == "failed_class_namespace":
        invocation = """
import importlib
module = importlib.import_module(MODULE_NAME)
check_failed_namespace(module.fail_class)
"""
    else:
        invocation = """
import importlib
check_failed_namespace(lambda: importlib.import_module(MODULE_NAME), MODULE_NAME)
"""
    try:
        exec(  # noqa: S102 - shared literal validator around ordinary source imports.
            compile(
                _FAILED_NAMESPACE_CHECK + invocation,
                str(Path(__file__)),
                "exec",
                dont_inherit=True,
            ),
            {"MODULE_NAME": module_name, "__name__": "ordinary_namespace_control"},
        )
    finally:
        sys.modules.pop(module_name, None)
        sys.modules.pop("namespace_failure_support", None)


@pytest.fixture(scope="module")
def failed_namespace_project(tmp_path_factory):
    sources = {"namespace_failure_support.py": _FAILED_NAMESPACE_SUPPORT}
    modules = {}
    for name, body in _FAILED_NAMESPACE_SOURCES.items():
        path = f"{name}.py"
        sources[path] = strict_opt_in(body.encode(), path)[0].decode()
        modules[name] = path
    return create_strict_project(
        tmp_path_factory.mktemp("strict-failed-namespace"), sources, modules=modules
    )


# Retained harness: The subject is a failing authenticated module initializer and unpublished
# namespace cleanup. Scenario admission/import setup cannot be caught by validation
# expectations.
def test_failed_module_namespace_preserves_errors_and_releases_values(failed_namespace_project):
    # Module initializers always use their explicit interpreted lowering plan.
    failed_namespace_project.run(
        _FAILED_NAMESPACE_CHECK
        + """
import importlib
check_failed_namespace(
    lambda: importlib.import_module('failed_module_namespace'),
    'failed_module_namespace',
    native_frames=False,
)
"""
    )


_FROZEN_MODULE_NOMINAL_BODY = """
from frozen_module_nominal_probe import exercise, body

class First:
    pass

class Second:
    pass

# This is FinalAfterSeal, not an explicitly mutable global. The ordinary probe
# changes its actual globals entry only while this module is initializing.
Alias = First

class Consumer:
    def accept(self, value: Alias) -> Alias:
        body(globals(), value)
        return value

first = First()
second = Second()
consumer = Consumer()
exercise(globals(), consumer, first, second, {span_seal!r})
"""

_FROZEN_MODULE_NOMINAL_PROBE = """
import ctypes
import sys
import threading
from typing import Any

from soac import _soac_ext
from soac.strict import StrictMutationError

states = {}
call_object = ctypes.pythonapi.PyObject_Call
call_object.argtypes = [ctypes.py_object] * 3
call_object.restype = ctypes.py_object
call_one = ctypes.pythonapi.PyObject_CallOneArg
call_one.argtypes = [ctypes.py_object] * 2
call_one.restype = ctypes.py_object
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
get_type_dict = ctypes.pythonapi.PyType_GetDict
get_type_dict.argtypes = [ctypes.py_object]
get_type_dict.restype = ctypes.py_object
set_dict = ctypes.pythonapi.PyObject_GenericSetDict
set_dict.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.c_void_p]
set_dict.restype = ctypes.c_int

def reject(error_type, call):
    try:
        call()
    except error_type:
        return
    raise AssertionError('missing mandatory boundary: ' + error_type.__name__)

def no_compilation():
    assert _soac_ext.runtime_compilation_activity() == {
        'schema': 1, 'lowering_entries': 0, 'blockpy_cache_entries': 0,
        'jit_engine_entries': 0,
    }

def frozen(function, module, *, entered):
    witness = _soac_ext.strict_function_diagnostics(function)
    assert witness is not None
    assert witness['schema'] == 2 and witness['backend'] == 'cpython'
    assert witness['entry_kind'] == 'original_code'
    assert witness['finalized'] is True
    assert witness['original_code_entered'] is entered
    module_witness = _soac_ext.strict_module_diagnostics(module)
    for key in ('source_path', 'source_sha256', 'artifact_generation'):
        assert witness[key] == module_witness[key], (key, witness)
    assert sealed(module.Consumer) == 1
    reject(StrictMutationError, lambda: setattr(function, '__defaults__', (module.first,)))
    reject(StrictMutationError, lambda: setattr(function, '__code__', function.__code__))
    assert function.__defaults__ is None

def body(namespace: Any, value: Any) -> None:
    state = states[namespace['__name__']]
    assert namespace is state['namespace']
    state['events'].append('body')
    state['values'].append(value)
    if state['shift_body']:
        namespace['Alias'] = namespace['First']
    if threading.current_thread() is state['worker']:
        state['entered'].set()
        if not state['release'].wait(10):
            raise AssertionError('module sealing did not release its active call')

def exercise(namespace: Any, receiver: Any, first: Any, second: Any, span_seal: bool) -> None:
    module = sys.modules[namespace['__name__']]
    function = type(receiver).accept
    assert vars(module) is namespace is function.__globals__
    module_witness = _soac_ext.strict_module_diagnostics(module)
    strict = module_witness is not None
    state = states[namespace['__name__']] = {
        'namespace': namespace, 'events': [], 'values': [], 'shift_body': True,
        'worker': None, 'outcomes': [], 'entered': threading.Event(),
        'release': threading.Event(),
    }
    if strict:
        assert module_witness['backend'] == 'cpython' and not module_witness['sealed']
        assert module_witness['initializer_entry_kind'] == 'original_code'
        assert module_witness['original_code_entered'] is True
        frozen(function, module, entered=False)
        assert call_one(receiver.accept, second) is second
        assert state['values'] == [second]
        assert _soac_ext.strict_function_diagnostics(function)['original_code_entered']
    else:
        assert _soac_ext.strict_function_diagnostics(function) is None
        # The same ordinary source has no nominal restriction or metadata seal.
        assert call_one(receiver.accept, second) is second
        function.__defaults__ = (first,)
        assert function.__defaults__ == (first,)
        function.__defaults__ = None

    class Keyword(str):
        __hash__ = str.__hash__
        def __eq__(self, other):
            equal = str.__eq__(self, other)
            if equal:
                namespace['Alias'] = namespace['Second']
                state['events'].append('keyword')
            return equal

    def python_call(function, arguments, keywords):
        return function(*arguments, **keywords)

    for invoke in (python_call, call_object):
        assert namespace['Alias'] is namespace['First']
        before = len(state['events'])
        assert invoke(receiver.accept, (), {Keyword('value'): second}) is second
        # The keyword callback changes the annotation's global and the body
        # restores it. Neither mutation changes ordinary argument/result values.
        assert state['events'][before:] == ['keyword', 'body']
        assert namespace['Alias'] is namespace['First']
        assert call_one(receiver.accept, first) is first
        before = len(state['values'])
        assert invoke(receiver.accept, (second,), {}) is second
        assert len(state['values']) == before + 1

    if span_seal:
        def active_call():
            try:
                state['outcomes'].append(
                    call_object(receiver.accept, (), {Keyword('value'): second})
                )
            except BaseException as error:
                state['outcomes'].append(error)
                state['entered'].set()

        state['worker'] = threading.Thread(target=active_call)
        state['worker'].start()
        if not state['entered'].wait(10):
            state['release'].set()
            state['worker'].join(10)
            raise AssertionError('the C-entered call did not reach its source body')
        if state['outcomes'] and isinstance(state['outcomes'][0], BaseException):
            raise state['outcomes'][0]
        assert state['outcomes'] == [], state['outcomes']
        assert state['worker'].is_alive()
        assert state['values'][-1] is second
        assert namespace['Alias'] is namespace['First']
        if strict:
            assert not _soac_ext.strict_module_diagnostics(module)['sealed']
    # No later body writes a now-sealed global. The already active call has
    # completed its write before entered is signaled above.
    state['shift_body'] = False
    if strict:
        frozen(function, module, entered=True)
    no_compilation()

def validate(module: Any, *, strict: bool, span_seal: bool) -> None:
    namespace = vars(module)
    state = states[module.__name__]
    try:
        assert namespace is state['namespace']
        assert module.Alias is module.First
        witness = _soac_ext.strict_module_diagnostics(module)
        if strict:
            assert witness is not None and witness['sealed'] is True
            frozen(module.Consumer.accept, module, entered=True)
        else:
            assert witness is None
            assert _soac_ext.strict_function_diagnostics(module.Consumer.accept) is None
        if span_seal:
            assert state['worker'].is_alive() and state['outcomes'] == []
            assert state['values'][-1] is module.second
    finally:
        if state['worker'] is not None:
            state['release'].set()
            state['worker'].join(10)
            assert not state['worker'].is_alive()

    if span_seal:
        # Sealing the module must not change an already-running call's result.
        assert len(state['outcomes']) == 1, state['outcomes']
        if isinstance(state['outcomes'][0], BaseException):
            raise state['outcomes'][0]
        assert state['outcomes'][0] is module.second
    for _ in range(128):
        assert module.consumer.accept(module.first) is module.first
    assert call_one(module.consumer.accept, module.first) is module.first
    before = len(state['values'])
    assert module.consumer.accept(module.second) is module.second
    assert call_one(module.consumer.accept, module.second) is module.second
    assert len(state['values']) == before + 2
    if strict:
        reject(StrictMutationError, lambda: namespace.__setitem__('Alias', module.Second))
        assert module.Alias is module.First

        # These are the real authoritative dictionaries, not mappingproxy IDs.
        # PyType_GetDict returns a new reference to the actual class dictionary.
        type_dictionary = get_type_dict(module.Consumer)
        module_contents, type_contents = dict(namespace), dict(type_dictionary)
        reject(StrictMutationError, lambda: set_dict(module, {}, None))
        assert vars(module) is namespace and namespace == module_contents
        reject(StrictMutationError, lambda: set_dict(module.Consumer, {}, None))
        assert get_type_dict(module.Consumer) is type_dictionary
        assert type_dictionary == type_contents
        frozen(module.Consumer.accept, module, entered=True)
        assert call_one(module.consumer.accept, module.first) is module.first
    else:
        namespace['Alias'] = module.Second
        assert module.Alias is module.Second
    no_compilation()
"""


@pytest.fixture(scope="module")
def frozen_module_nominal_projects(tmp_path_factory):
    sources = {"frozen_module_nominal_probe.py": _FROZEN_MODULE_NOMINAL_PROBE}
    modules = {}
    for suffix, span_seal in (("sync", False), ("spans_seal", True)):
        name = f"frozen_module_nominal_{suffix}"
        body = _FROZEN_MODULE_NOMINAL_BODY.format(span_seal=span_seal)
        sources[f"{name}.py"] = "# soac: module(strict_assign=true, checked_attr=true)\n" + body
        sources[f"ordinary_{name}.py"] = body
        modules[name] = f"{name}.py"
    return create_strict_project(
        tmp_path_factory.mktemp("cpython-frozen-module-nominal"),
        sources,
        modules=modules,
        backend="cpython",
    )


# Retained harness: Joins the strict module active worker and validates its seal before
# importing the ordinary control, whose own active call starts during import. Eagerly importing
# both subjects changes that lifecycle ordering.
@pytest.mark.parametrize(
    ("suffix", "span_seal"),
    [("sync", False), ("spans_seal", True)],
    ids=["binder-body", "active-call-across-module-seal"],
)
def test_cpython_frozen_method_preserves_calls_and_module_seals_across_callbacks(
    frozen_module_nominal_projects, suffix, span_seal
):
    name = f"frozen_module_nominal_{suffix}"
    frozen_module_nominal_projects.run_case(
        name,
        f"""
def validate(module):
    import importlib
    from frozen_module_nominal_probe import validate as check

    check(module, strict=True, span_seal={span_seal!r})
    ordinary = importlib.import_module('ordinary_' + module.__name__)
    check(ordinary, strict=False, span_seal={span_seal!r})
""",
        Path(__file__),
        required_functions=("Consumer.accept",),
        
        backend="cpython",
    )
