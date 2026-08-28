"""Authenticated function execution with ordinary calls and protected storage."""

import json
import textwrap
from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project

_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from typing import Any, cast, final
from support import events, marker, observe

def identity(value: int) -> int:
    return value

first_lambda, second_lambda = (lambda value: value), (lambda value: value + 1)

def widened(value: float) -> float:
    return value

def optional(value: int | str | None) -> int | str | None:
    return value

def shape(first: int, /, second: int = 2, *items: int,
          named: str | None = None, **extras: int) -> int:
    return first + second

def caller(value: Any) -> int:
    return identity(value)

def bad_return(value: Any) -> int:
    return cast(int, value)

def finish_with_result(factory, observer, result: Any) -> int:
    payload = factory()
    try:
        raise LookupError("source handler")
    except LookupError:
        observer("body")
        return cast(int, result)

def raises(value: int) -> int:
    raise LookupError("body wins")

def annotation_trap(format: int):
    events.append("annotation evaluated")
    raise AssertionError("annotation provider must never be called by a boundary")

identity.__annotate__ = annotation_trap

def active_default(value=marker("active-old")) -> None:
    active_default.__defaults__ = (marker("active-new"),)
    observe(value)

active_default()
events.append("after-active")

def idle_default(value=marker("idle-old")):
    return value

idle_default.__defaults__ = (marker("idle-new"),)
events.append("after-idle")

def make_cycle():
    captured = []
    def inner(value: int) -> int:
        return value + len(captured)
    captured.append(inner)
    return inner

class StoppingIterator:
    def __next__(self):
        raise StopIteration

def catch_stop(iterator, observer):
    try:
        return next(iterator)
    except StopIteration:
        return observer()

class ReturningIterator:
    def __next__(self):
        try:
            raise LookupError("callee handler")
        except LookupError:
            return 7

def replace_result(iterator, create):
    value = create()
    value = next(iterator)
    return value
"""

_SUPPORT = """
events = []

class Marker:
    def __init__(self, name):
        self.name = name
    def __del__(self):
        events.append("drop:" + self.name)

def marker(name):
    return Marker(name)

def observe(value):
    events.append("use:" + value.name)
"""


@pytest.fixture(scope="module")
def strict_functions(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-functions"),
        {"checked.py": _SOURCE, "support.py": _SUPPORT},
        modules={"checked": "checked.py"},
    )


def test_profiled_stop_iteration_handler_preserves_implicit_exception_observers(
    strict_functions, tmp_path
):
    program = """
import sys
import checked
from soac import _soac_ext

assert _soac_ext.strict_function_entry_kind(checked.catch_stop) == 'checked_native'
iterator = checked.StoppingIterator()
marker = ValueError('outside handler')
for _ in range(200):
    try:
        raise marker
    except ValueError:
        caught = checked.catch_stop(iterator, sys.exception)
        assert type(caught) is StopIteration
        assert caught.__context__ is marker
        assert sys.exception() is marker
    assert sys.exception() is None
"""
    work = tmp_path / "stop-iteration-observation"
    for mode in ("profile", "apply", "verify"):
        strict_functions.run(
            program, opt_mode=mode, extra_env={"SOAC_WORK_DIR": str(work)}
        )
    assert (work / "profile.bin").is_file()
    assert (work / "verify.bin").is_file()


def test_profiled_result_replacement_preserves_caller_handler_and_cleanup(
    strict_functions, tmp_path
):
    program = """
import gc
import sys
import checked
from soac import _soac_ext

assert _soac_ext.strict_function_entry_kind(checked.replace_result) == 'checked_native'
seen = []
class Previous:
    def __del__(self):
        seen.append("drop")

iterator = checked.ReturningIterator()
marker = ValueError('caller handler')
for _ in range(200):
    try:
        raise marker
    except ValueError:
        assert checked.replace_result(iterator, Previous) == 7
        assert sys.exception() is marker
    assert sys.exception() is None
gc.collect()
assert seen == ["drop"] * 200
"""
    work = tmp_path / "inline-result-replacement"
    for mode in ("profile", "apply", "verify"):
        strict_functions.run(
            program, opt_mode=mode, extra_env={"SOAC_WORK_DIR": str(work)}
        )
    assert (work / "profile.bin").is_file()
    assert (work / "verify.bin").is_file()


def test_cpython_callable_signature_facts_do_not_enforce_union_values(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "checked.py": _SOURCE + """
def mixed_union(value: int | list[int]) -> int | list[int]:
    return value
""",
            "support.py": _SUPPORT,
        },
        modules={"checked": "checked.py"},
        backend="cpython",
    )
    # Inspect the actual checker's structured output, not a manually supplied
    # contract. run_case below independently authenticates this publication.
    artifact = Path(project.publication["artifact_directory"])
    manifest = json.loads((artifact / "manifest.json").read_text())["manifest"]
    index, = [
        item for item in manifest["modules"]
        if item["module"]["module_name"] == "checked"
    ]
    shard = json.loads(
        (artifact / "modules" / f'{index["shard_digest"]}.soac-types').read_text()
    )
    mixed, = [
        item for item in shard["functions"]
        if item["identity"]["lexical_qualname"] == "mixed_union"
    ]
    signature = mixed["signature"]
    parameter, = signature["parameters"]
    assert parameter["annotation_origin"] == "explicit"
    assert signature["return_annotation_origin"] == "explicit"
    for value_type in (parameter["value_type"], signature["return_type"]):
        assert value_type["kind"] == "union", value_type
        assert len(value_type["data"]) == 2, value_type
        assert {
            "kind": "nominal_builtin",
            "data": {"builtin": "int", "allow_subclasses": True},
        } in value_type["data"], value_type
        assert {
            "kind": "unsupported",
            "data": {"kind": "mutable_generic", "reason": "no_runtime_enforcement"},
        } in value_type["data"], value_type

    project.run_case(
        "checked",
        """
import ctypes
import checked
from support import events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

one = ctypes.pythonapi.PyObject_CallOneArg
one.argtypes = [ctypes.py_object, ctypes.py_object]
one.restype = ctypes.py_object

def exercise(invoke):
    # Static signature facts remain available without turning either union
    # into a runtime predicate.
    for value in (7, [1], ["not an int"], "outside either arm", object()):
        assert invoke(checked.mixed_union, value) is value
    original = [1]
    assert invoke(checked.mixed_union, original) is original
    original.append(object())
    assert invoke(checked.mixed_union, original) is original

    assert invoke(checked.optional, "supported") == "supported"
    assert invoke(checked.optional, None) is None
    outside = []
    assert invoke(checked.optional, outside) is outside

def python_call(function, value):
    return function(value)

exercise(python_call)
for value in range(128):
    assert checked.mixed_union(value) == value
exercise(python_call)
exercise(one)
assert "annotation evaluated" not in events
diagnostic = _soac_ext.strict_module_diagnostics(checked)
observed = _assert_cpython_function_witness(
    checked.mixed_union, diagnostic)
assert observed["original_code_entered"] is True
_assert_cpython_function_witness(
    checked.optional, diagnostic)
""",
        Path(__file__),
        required_functions=("mixed_union", "optional", "identity"),
        
        backend="cpython",
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("same_code", [False, True])
def test_code_replacement_during_initialization_preserves_actual_body_semantics(
    tmp_path, same_code, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "support.py": """
from soac.strict import StrictMutationError

events = []

def replacement(value):
    return "not an int"

def patch(function, same_code):
    original = function.__code__
    try:
        function.__code__ = original if same_code else replacement.__code__
    except StrictMutationError:
        assert function.__code__ is original
        events.append("rejected before mutation")
    else:
        events.append("original" if same_code else "replaced")
    for value in ("bad argument", 1):
        result = function(value)
        if function.__code__ is original:
            assert result is value
        else:
            assert result == "not an int"
        events.append(("body", result))
""",
            "checked_patch.py": f"""
# soac: module(strict_assign=true, checked_attr=true)
from support import patch

def checked(value: int) -> int:
    return value

patch(checked, {same_code!r})
""",
        },
        modules={"checked_patch": "checked_patch.py"},
    )
    project.run(
        f"same_code = {same_code!r}\n"
        f"expected_entry_kind = {('entry_interpreter' if entry_interpreter else 'checked_native')!r}\n"
        + """
import sys
from support import events
from soac.strict import StrictMutationError, StrictRuntimeUnavailableError

if same_code:
    import checked_patch
    # No annotation-driven early freeze may reject the same-code assignment.
    assert events == ["original", ("body", "bad argument"), ("body", 1)]
    assert _soac_ext.strict_module_diagnostics(checked_patch)["sealed"]
    function = checked_patch.checked
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry_kind

    import ctypes
    get_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    get_owner.argtypes = [ctypes.py_object]
    get_owner.restype = ctypes.c_void_p
    get_seal = ctypes.pythonapi.PyFunction_GetSoacStrictId
    get_seal.argtypes = [ctypes.py_object]
    get_seal.restype = ctypes.c_uint64
    get_source = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
    get_source.argtypes = [ctypes.py_object]
    get_source.restype = ctypes.c_uint64
    assert get_owner(function) and get_seal(function)
    code = function.__code__
    assert get_source(code) > 0
    marker = object()
    assert function(marker) is marker  # No argument or return-type check.
    try:
        function.__code__ = code
    except StrictMutationError:
        pass
    else:
        raise AssertionError("successfully sealed source metadata stayed mutable")
    assert function.__code__ is code
    assert function("after seal") == "after seal"
else:
    # Ordinary replacement code can run before freezing, but it cannot receive
    # the original source contract at the independent module-sealing boundary.
    try:
        import checked_patch
    except StrictRuntimeUnavailableError as error:
        assert "strict function native metadata changed" in str(error)
    else:
        raise AssertionError("retained replacement acquired the original source contract")
    assert events == ["replaced", ("body", "not an int"), ("body", "not an int")]
    assert "checked_patch" not in sys.modules
print("preseal-body-and-source-sealing")
""",
        entry_interpreter=entry_interpreter,
    )


_BINDING_IDENTITY_FUNCTIONS = """
from binding_identity_probe import dynamic, events

@dynamic
def plain(*, value=1):
    return value

@dynamic
def stream(*, value=1):
    events.append(("source body", value))
    yield value
"""

_BINDING_IDENTITY_PROBE = """
events = []
held = []

def dynamic(function):
    return function

def replacement(*, value=1):
    yield value + 100

class IdentityKey:
    def __init__(self, function, reenter=False):
        self.function = function
        self.expected = function.__code__.co_varnames[0]
        self.reenter = reenter

    def __hash__(self):
        return hash(self.expected)

    def __eq__(self, other):
        events.append(("name identity", other is self.expected))
        assert other is self.expected, "binder replaced the native parameter-name object"
        if self.reenter:
            # This completes another binding/construction on the same function
            # without recursively consulting its defaults or running its body.
            held.append(self.function(value=99))
            self.function.__kwdefaults__ = {"value": 20}
            self.function.__code__ = replacement.__code__
        return True

def exercise(module):
    events.clear()
    held.clear()
    module.plain.__kwdefaults__ = {IdentityKey(module.plain): 7}
    assert module.plain() == 7
    assert events == [("name identity", True)], events
    module.plain.__kwdefaults__ = {}

    events.clear()
    module.stream.__kwdefaults__ = {IdentityKey(module.stream, reenter=True): 7}
    created = module.stream()
    assert events == [("name identity", True)], events
    assert list(created) == [7]
    assert list(held.pop()) == [99]
    assert events == [("name identity", True), ("source body", 7), ("source body", 99)], events
    assert list(module.stream()) == [120]
"""


@pytest.fixture(scope="module")
def binding_identity_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-binding-identity"),
        {
            "binding_identity.py": "# soac: module(strict_assign=true, checked_attr=true)\n"
            + _BINDING_IDENTITY_FUNCTIONS,
            "binding_identity_control.py": _BINDING_IDENTITY_FUNCTIONS,
            "binding_identity_probe.py": _BINDING_IDENTITY_PROBE,
        },
        modules={"binding_identity": "binding_identity.py"},
    )


_PRIVATE_CLASS_CAPTURE_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from private_capture_support import argument, observe_namespace, observe_target, pause, replace_public_closure

class Base:
    def __init_subclass__(cls, *, flag: bool = False):
        super().__init_subclass__()

def namespace_factory():
    marker = object()
    class Captured:
        captured = marker
    return Captured

def build(should_fail: bool = False):
    class Target:
        pass
    observe_target(Target)
    class Holder(Base, flag=argument(should_fail)):
        def __init__(self, value):
            self.payload: Target = value
    return Target, Holder

def nested_namespace(should_fail: bool = False):
    class Target:
        pass
    observe_target(Target)
    class Outer:
        observe_namespace(False)
        class Holder:
            observe_namespace(should_fail)
            def __init__(self, value):
                self.payload: Target = value
    return Target, Outer.Holder

def make_public_bridge():
    class Target:
        pass
    observe_target(Target)
    def create():
        current = Target
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return current, Holder
    return create

# An ordinary callback changes the native closure while this module is still
# initializing. The eligible source function freezes only at module sealing.
public_bridge = replace_public_closure(make_public_bridge())

def private_bridge_family():
    class Target:
        pass
    def create():
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Holder
    return Target, create

def private_generator_family():
    class Target:
        pass
    def create():
        yield None
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        yield Holder
    return Target, create

def private_coroutine_family():
    class Target:
        pass
    async def create():
        await pause()
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Holder
    return Target, create

def private_async_generator_family():
    class Target:
        pass
    async def create():
        yield None
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        yield Holder
    return Target, create

def terminal_generator_family(payload):
    def create():
        yield None
        return payload is None
    return create

def terminal_coroutine_family(payload):
    async def create():
        await pause()
        return payload is None
    return create

def terminal_async_generator_family(payload):
    async def create():
        yield None
        if payload is None:
            return
    return create
"""


@pytest.fixture(scope="module")
def private_class_capture_project(tmp_path_factory):
    project = create_strict_project(
        tmp_path_factory.mktemp("strict-private-class-captures"),
        {
            "private_capture_model.py": _PRIVATE_CLASS_CAPTURE_SOURCE,
            "ordinary_private_capture.py": _PRIVATE_CLASS_CAPTURE_SOURCE.replace(
                "# soac: module(strict_assign=true, checked_attr=true)", "# ordinary metadata control", 1
            ),
            "private_capture_support.py": """
from collections.abc import Callable
import gc
import weakref
import ctypes

targets: list[weakref.ReferenceType[type]] = []
replay: Callable[[], None] | None = None
namespace_handles: list[object] = []

class PublicReplacement:
    pass

class PublicAfter:
    pass

class Pause:
    def __await__(self):
        yield "paused"
        return None

def pause():
    return Pause()

def make_cell(value):
    return (lambda: value).__closure__[0]

def replace_public_closure(function):
    setter = ctypes.pythonapi.PyFunction_SetClosure
    setter.argtypes = [ctypes.py_object, ctypes.py_object]
    setter.restype = ctypes.c_int
    assert function.__code__.co_freevars == ("Target",)
    assert setter(function, (make_cell(PublicReplacement),)) == 0
    return function

def observe_target(value: type) -> None:
    targets.append(weakref.ref(value))

def observe_namespace(should_fail: bool) -> None:
    for value in gc.get_objects():
        if type(value).__name__ == "_StrictNamespaceExecution":
            if not any(value is previous for previous in namespace_handles):
                namespace_handles.append(value)
    if should_fail:
        raise RuntimeError("nested namespace failed")

def argument(should_fail: bool) -> bool:
    callback = replay
    if callback is not None:
        callback()
    if should_fail:
        raise RuntimeError("class argument failed")
    return False
""",
        },
        modules={"private_capture_model": "private_capture_model.py"},
    )
    # Native class ownership alone does not enable field predicates. Assert
    # the actual exported policy before treating a write as a checked boundary.
    shards = list((project.root / "artifacts/objects").glob("*.soac-types"))
    assert len(shards) == 1
    facts = json.loads(shards[0].read_text())
    assert facts["language_policy"]["checked_attr"] is True
    return project


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_private_class_helpers_reject_create_calls_and_clear_failed_argument_captures(
    private_class_capture_project, function_create_watch_extension, entry_interpreter
):
    private_class_capture_project.run_case(
        "private_capture_model",
        textwrap.dedent(f"""
        def validate(module):
            import gc
            import importlib.util
            import ordinary_private_capture
            import private_capture_support as support
            from soac.strict import StrictRuntimeUnavailableError

            spec = importlib.util.spec_from_file_location(
                "_strict_function_create_watch", {str(function_create_watch_extension)!r}
            )
            watcher = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(watcher)
            events = watcher.watch(module.__dict__, "_dp_class_ns_Captured", ())
            try:
                captured = module.namespace_factory()
            finally:
                watcher.stop()
            assert len(events) == 1
            event = events[0]
            assert event["freevars"] == 1 and not event["closure_present"]
            assert event["flags"] & 0x10000000
            assert event["source_id"] == 0 and not event["owner_present"]
            assert not event["success"]
            assert isinstance(event["result"], StrictRuntimeUnavailableError)
            assert captured.captured is not None
            events.clear()
            del event, captured

            events = watcher.watch(module.__dict__, "_dp_define_class_Holder", ())
            try:
                try:
                    module.build(True)
                except RuntimeError as error:
                    assert str(error) == "class argument failed"
                else:
                    raise AssertionError("class argument failure was lost")
            finally:
                watcher.stop()
            assert len(events) == 1
            event = events[0]
            assert event["freevars"] == 0 and not event["owner_present"]
            assert event["flags"] & 0x10000000 and event["source_id"] == 0
            assert isinstance(event["result"], StrictRuntimeUnavailableError)
            escaped_helper = event["function"]
            assert escaped_helper.__closure__ is None
            gc.collect()
            assert support.targets[-1]() is None, "escaped failed helper retained its original Target cell"
            assert module.build.__code__.co_cellvars == ordinary_private_capture.build.__code__.co_cellvars == ()
            assert module.build.__code__.co_freevars == ordinary_private_capture.build.__code__.co_freevars == ()
        """),
        private_class_capture_project.project / "private_capture_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=("build", "namespace_factory"),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_private_class_capture_rejects_another_factory_namespace_birth(
    private_class_capture_project, function_create_watch_extension, entry_interpreter
):
    private_class_capture_project.run_case(
        "private_capture_model",
        textwrap.dedent(f"""
        def validate(module):
            import gc
            import importlib.util
            import private_capture_support as support
            from soac import _soac_ext
            from soac.strict import StrictRuntimeUnavailableError

            spec = importlib.util.spec_from_file_location(
                "_strict_function_create_watch", {str(function_create_watch_extension)!r}
            )
            watcher = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(watcher)
            first_events = watcher.watch(module.__dict__, "_dp_class_ns_Holder", (), invoke=False)
            try:
                target, holder = module.build(False)
            finally:
                watcher.stop()
            assert len(first_events) == 1
            first_namespace = first_events[0]["function"]
            assert holder.__init__.__closure__ is None
            assert holder.__init__.__annotate__ is None
            value = holder(target())
            try:
                holder(object())
            except TypeError:
                pass
            else:
                raise AssertionError("required method-only field check was absent")

            second_events = watcher.watch(module.__dict__, "_dp_define_class_Holder", ())
            def replay():
                assert len(second_events) == 1
                helper = second_events[0]["function"]
                # Compiler-owned class constructors use the entry interpreter
                # in both modes; the source build function is witnessed below.
                assert _soac_ext.strict_function_entry_kind(helper) == "entry_interpreter"
                helper(first_namespace, module.__dict__, (module.Base,), {{"flag": False}})
            support.replay = replay
            try:
                try:
                    module.build(False)
                except StrictRuntimeUnavailableError as error:
                    assert "another namespace function" in str(error)
                else:
                    raise AssertionError("same-source foreign namespace acquired original cells")
            finally:
                support.replay = None
                watcher.stop()
            gc.collect()
            assert support.targets[-1]() is None
            assert value.payload.__class__ is target
        """),
        private_class_capture_project.project / "private_capture_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=("build",),
    )


_TEMPORARY_LIFETIME_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
import gc

def failed_unpack(make, record, reject, key):
    values = make(3)
    try:
        first, second = iter(values)
    except ValueError:
        record("handler")
    del values
    gc.collect()
    record("after")

def transient_unpack(make, record, reject, key):
    try:
        first, second = iter(make(3))
    except ValueError:
        record("handler")
    record("after")

def partial_target(make, record, reject, key):
    try:
        first, reject().field = make(2)
    except AttributeError:
        record("handler")
    del first
    record("after")

def subscript_target(make, record, reject, key):
    try:
        reject()[key()] = make(1)[0]
    except TypeError:
        record("handler")
    record("after")

def suspended_target(make, record, reject, key):
    try:
        first, reject().field = yield "ready"
    except AttributeError:
        record("handler")
    del first
    record("after")
    yield "done"

def choose_key():
    yield "key"
    return "slot"

def suspended_live_operand(make, record, reject, key):
    target = {}
    target[(yield from choose_key())] = make(1)[0]
    record("stored")
    del target
    record("after")
    yield "done"
"""


_UNPACKED_SETITEM_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def unpacked_subscript_target(make, record, reject, key):
    try:
        reject()[0], = make(1)
    except TypeError:
        record("handler")
    record("after")

def escaping_unpacked_subscript_target(make, record, reject, key):
    reject()[0], = make(1)
    record("after")

def escaping_named_subscript_target(make, record, reject, key):
    replacement = make(1)[0]
    reject()[0] = replacement
    del replacement

def escaping_prefixed_subscript_target(make, record, reject, key):
    _dp_tmp_source = make(1)[0]
    reject()[0] = _dp_tmp_source
    del _dp_tmp_source
"""


_SETITEM_LIFETIME_CASES = (
    "unpacked_subscript_target",
    "escaping_unpacked_subscript_target",
    "escaping_named_subscript_target",
    "escaping_prefixed_subscript_target",
)


def _operand_lifetime_project(tmp_path_factory, label, source):
    ordinary = source.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1)
    return create_strict_project(
        tmp_path_factory.mktemp(label),
        {
            "operand_model.py": source,
            "ordinary_operand_model.py": ordinary,
        },
        modules={"operand_model": "operand_model.py"},
    )


@pytest.fixture(scope="module")
def temporary_lifetime_project(tmp_path_factory):
    return _operand_lifetime_project(
        tmp_path_factory, "strict-temporary-lifetimes", _TEMPORARY_LIFETIME_SOURCE
    )


@pytest.fixture(scope="module")
def unpacked_setitem_project(tmp_path_factory):
    # Eager compilation must reach these assignments independently of unrelated
    # suspended functions covered by the other fixture.
    return _operand_lifetime_project(
        tmp_path_factory, "strict-unpacked-setitem", _UNPACKED_SETITEM_SOURCE
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize(
    "case",
    [
        "failed_unpack",
        "transient_unpack",
        "partial_target",
        "subscript_target",
        *_SETITEM_LIFETIME_CASES,
        "suspended_target",
        "suspended_live_operand",
    ],
)
def test_assignment_operands_preserve_exceptions_suspension_and_cleanup(
    request, tmp_path, entry_interpreter, case
):
    project = request.getfixturevalue(
        "unpacked_setitem_project"
        if case in _SETITEM_LIFETIME_CASES
        else "temporary_lifetime_project"
    )
    expected_entry = (
        "generator_factory"
        if case.startswith("suspended_")
        else "entry_interpreter"
        if entry_interpreter
        else "checked_native"
    )
    program = f"""
        import ctypes
        import gc
        import sys
        import operand_model
        import ordinary_operand_model
        from soac import _soac_ext

        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        function = getattr(operand_model, {case!r})
        ordinary = getattr(ordinary_operand_model, {case!r})
        assert owner(function)
        assert not owner(ordinary)
        assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
        assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None
        assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}

        def exercise(function, successful_setitem=False):
            events = []
            live = [0]
            def exception_name():
                current = sys.exception()
                return None if current is None else type(current).__name__
            class Tracked:
                def __init__(self, label):
                    self.label = label
                    live[0] += 1
                def __del__(self):
                    live[0] -= 1
                    events.append(('drop', self.label, exception_name()))
            class ReadOnly:
                @property
                def field(self):
                    raise AssertionError('the setter must not invoke the getter')
                def __del__(self):
                    events.append(('drop', 'container', exception_name()))
            class Writable(ReadOnly):
                def __setitem__(self, index, value):
                    events.append(('store', index.label, value.label, exception_name()))
            target_type = Writable if successful_setitem else ReadOnly
            def make(count):
                return [Tracked(str(index)) for index in range(count)]
            def key():
                return Tracked('key')
            def record(stage):
                events.append((stage, live[0], exception_name()))
            try:
                raise KeyError('caller handler')
            except KeyError as marker:
                if {case!r}.startswith('escaping_'):
                    try:
                        function(make, record, target_type, key)
                    except TypeError as failure:
                        record('caller handler')
                        if function is ordinary and {case!r} in {{'escaping_named_subscript_target',
                                                                 'escaping_prefixed_subscript_target'}}:
                            source_name = ('replacement' if {case!r} == 'escaping_named_subscript_target'
                                           else '_dp_tmp_source')
                            traceback = failure.__traceback__
                            while traceback.tb_frame.f_code.co_name != function.__name__:
                                traceback = traceback.tb_next
                                assert traceback is not None, 'error lost its source frame'
                            binding = traceback.tb_frame.f_locals[source_name]
                            assert isinstance(binding, Tracked) and binding.label == '0'
                            del binding, traceback
                        failure.__traceback__ = None
                        record('traceback cleared')
                    else:
                        raise AssertionError('unsupported item assignment did not fail')
                    result = None
                else:
                    result = function(make, record, target_type, key)
                if {case!r} == 'suspended_target':
                    assert next(result) == 'ready'
                    assert result.send(make(2)) == 'done'
                    result.close()
                elif {case!r} == 'suspended_live_operand':
                    assert next(result) == 'key'
                    assert result.send(None) == 'done'
                    result.close()
                else:
                    assert result is None
                assert sys.exception() is marker
            del result
            gc.collect()
            assert live[0] == 0, (events, live)
            # Explicit callbacks and handler order remain exact. Temporary
            # owners and implicit destructor order are engine-specific.
            semantic = [
                event if event[0] == 'store' else (event[0], event[-1])
                for event in events if event[0] != 'drop'
            ]
            cleanup = sorted(event[1] for event in events if event[0] == 'drop')
            return semantic, cleanup

        # Preserve the original failing-target case, and exercise the same
        # source's successful STORE_SUBSCR cleanup without retaining the value.
        for successful in ((False, True) if {case!r} == 'subscript_target' else (False,)):
            expected = exercise(ordinary, successful)
            observed = exercise(function, successful)
            assert observed == expected, (successful, observed, expected)
        """
    modes = (
        ("none", "profile", "apply", "verify")
        if case in _SETITEM_LIFETIME_CASES
        and not entry_interpreter
        else ("none",)
    )
    work = tmp_path / "assignment-operand-profile"
    for mode in modes:
        project.run(
            program,
            entry_interpreter=entry_interpreter,
            opt_mode=mode,
            extra_env={
                "SOAC_WORK_DIR": str(work),
                "SOAC_LOG": f"soac_jit_codegen=debug;json={tmp_path / f'operand-{mode}.jsonl'}",
            },
        )
    if "profile" in modes:
        assert (work / "profile.bin").is_file()
        assert (work / "verify.bin").is_file()


_SETATTR_OPERAND_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def attribute_assignment(target, make):
    target.value = make()
"""


_SETATTR_OPERAND_OBSERVER = """
def observe_attribute_assignment(function, outcome, *, native_schedule=False):
    import gc
    import sys
    import weakref

    events = []
    values = []
    caller = KeyError('caller handler')
    failure = LookupError('setter failed')

    def context():
        current = sys.exception()
        if current is caller:
            return 'caller'
        if current is failure:
            return 'setter-error'
        return None if current is None else type(current).__name__

    class Payload:
        def __del__(self):
            events.append(('drop-value', context()))

    class Target:
        def __setattr__(self, name, value):
            assert name == 'value'
            assert values[0]() is value
            count = sys.getrefcount(value) if native_schedule else None
            events.append(('set', count, context()))
            if outcome == 'error':
                raise failure

        def __del__(self):
            events.append(('drop-target', context()))

    def make():
        value = Payload()
        values.append(weakref.ref(value))
        events.append(('made', context()))
        return value

    try:
        raise caller
    except KeyError:
        try:
            result = function(Target(), make)
        except LookupError as caught:
            assert outcome == 'error' and caught is failure
            assert caught.__context__ is caller
            events.append(('error', context(), values[0]() is not None))
            caught.__traceback__ = None
            events.append(('traceback-cleared', context(), values[0]() is not None))
        else:
            assert outcome == 'success' and result is None
            events.append(('returned', context(), values[0]() is not None))
        assert sys.exception() is caller
        events.append(('after-call', context(), values[0]() is not None))
    gc.collect()
    events.append(('after-handler', context(), values[0]() is not None))
    return events

def attribute_assignment_semantics(events):
    labels = [event[0] for event in events]
    assert labels.count('drop-value') == labels.count('drop-target') == 1, events
    assert events[-1] == ('after-handler', None, False), events
    return [
        (event[0], event[-1]) if event[0] == 'set' else event[:2]
        for event in events if not event[0].startswith('drop-')
    ]
"""


@pytest.mark.parametrize("outcome", ["success", "error"])
def test_native_attribute_assignment_replacement_ownership(outcome):
    namespace = {}
    exec(_SETATTR_OPERAND_SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1), namespace)
    exec(_SETATTR_OPERAND_OBSERVER, namespace)
    events = namespace["observe_attribute_assignment"](
        namespace["attribute_assignment"], outcome, native_schedule=True
    )
    labels = [event[0] for event in events]
    assert labels[:2] == ["made", "set"]
    assert labels.count("set") == 1
    assert labels.count("drop-value") == labels.count("drop-target") == 1
    assert events[-2:] == [("after-call", "caller", False), ("after-handler", None, False)]
    if outcome == "success":
        assert labels.index("drop-value") < labels.index("drop-target") < labels.index("returned")
    else:
        assert ("error", "setter-error", True) in events
        assert labels.index("error") < labels.index("drop-value") < labels.index("traceback-cleared")
        assert labels.index("error") < labels.index("drop-target") < labels.index("traceback-cleared")


_SETATTR_CAPTURE_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def captured_receiver(first, second, make):
    first().value = make()

def chained_receivers(first, second, make):
    first().value = second().value = make()
"""


_SETATTR_CAPTURE_OBSERVER = """
def observe_captured_attribute_assignment(function, case, outcome, *, native_schedule=False):
    import gc
    import sys
    import weakref

    events = []
    values = []
    targets = []
    caller = KeyError('caller handler')
    failure = LookupError('assignment failed')
    failing_receiver = 'second' if case == 'chained_receivers' else 'first'

    def context():
        current = sys.exception()
        if current is caller:
            return 'caller'
        if current is failure:
            return 'assignment-error'
        return None if current is None else type(current).__name__

    def alive():
        return (values[0]() is not None, tuple(reference() is not None for reference in targets))

    class Payload:
        def __del__(self):
            events.append(('drop-value', context()))

    class Target:
        def __init__(self, label):
            object.__setattr__(self, 'label', label)

        def __setattr__(self, name, value):
            assert name == 'value'
            assert values[0]() is value
            assert any(reference() is self for reference in targets)
            self_count = sys.getrefcount(self) if native_schedule else None
            value_count = sys.getrefcount(value) if native_schedule else None
            events.append(('set', self.label, self_count, value_count, context()))
            if outcome == 'setter-error' and self.label == failing_receiver:
                raise failure

        def __del__(self):
            events.append(('drop-target', self.label, context()))

    def make():
        value = Payload()
        values.append(weakref.ref(value))
        events.append(('made', context()))
        return value

    def receiver(label):
        events.append(('receiver', label, context()))
        if outcome == 'receiver-error' and label == failing_receiver:
            raise failure
        value = Target(label)
        targets.append(weakref.ref(value))
        return value

    def first():
        return receiver('first')

    def second():
        return receiver('second')

    try:
        raise caller
    except KeyError:
        try:
            result = function(first, second, make)
        except LookupError as caught:
            assert outcome != 'success' and caught is failure
            assert caught.__context__ is caller
            events.append(('error', context(), alive()))
            # The Python setter's own traceback legitimately retains self and
            # value. Remove it at the same explicit point in both executions.
            caught.__traceback__ = None
            events.append(('traceback-cleared', context(), alive()))
        else:
            assert outcome == 'success' and result is None
            events.append(('returned', context(), alive()))
        assert sys.exception() is caller
        events.append(('after-call', context(), alive()))
    gc.collect()
    events.append(('after-handler', context(), alive()))
    return events

def captured_attribute_assignment_semantics(events):
    final = events[-1]
    assert final[:2] == ('after-handler', None), events
    assert final[2][0] is False and not any(final[2][1]), events
    assert sum(event[0] == 'drop-value' for event in events) == 1, events
    target_drops = [event[1] for event in events if event[0] == 'drop-target']
    assert len(target_drops) == len(set(target_drops)) == len(final[2][1]), events
    semantic = []
    for event in events:
        if event[0].startswith('drop-'):
            continue
        if event[0] == 'set':
            semantic.append((event[0], event[1], event[-1]))
        elif event[0] in {'returned', 'error', 'traceback-cleared', 'after-call', 'after-handler'}:
            semantic.append(event[:2])
        else:
            semantic.append(event)
    return semantic, sorted(target_drops)
"""


@pytest.mark.parametrize("case", ["captured_receiver", "chained_receivers"])
@pytest.mark.parametrize("outcome", ["success", "receiver-error", "setter-error"])
def test_native_attribute_assignment_captured_owners(case, outcome):
    namespace = {}
    exec(_SETATTR_CAPTURE_SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1), namespace)
    exec(_SETATTR_CAPTURE_OBSERVER, namespace)
    events = namespace["observe_captured_attribute_assignment"](
        namespace[case], case, outcome, native_schedule=True
    )
    sets = [event for event in events if event[0] == "set"]
    expected_values = [3, 2] if case == "chained_receivers" else [2]
    if outcome == "receiver-error":
        expected_values.pop()
    # The observer's identity-check genexpr captures self in a cell. Loading
    # that cell for getrefcount owns a reference in addition to STORE_ATTR's
    # receiver and the cell; this is not a borrowed local-argument load.
    assert [event[2] for event in sets] == [3] * len(sets)
    assert [event[3] for event in sets] == expected_values
    assert events[0] == ("made", "caller")
    assert events[-2][0:2] == ("after-call", "caller")
    assert events[-1][0:2] == ("after-handler", None)
    assert events[-1][2][0] is False
    assert not any(events[-1][2][1])
    assert sum(event[0] == "drop-value" for event in events) == 1
    if outcome == "success":
        target_drops = [index for index, event in enumerate(events) if event[0] == "drop-target"]
        value_drop = next(index for index, event in enumerate(events) if event[0] == "drop-value")
        returned = next(index for index, event in enumerate(events) if event[0] == "returned")
        assert target_drops and max(target_drops) < value_drop < returned
    elif outcome == "receiver-error":
        # No Python setter frame owns the RHS on this edge; it must unwind
        # before the caller catches the factory's error, without a GC cycle.
        value_drop = next(index for index, event in enumerate(events) if event[0] == "drop-value")
        caught = next(index for index, event in enumerate(events) if event[0] == "error")
        assert value_drop < caught


def test_native_attribute_assignment_source_local_uses_borrowed_rhs():
    import sys

    observed = []

    class Target:
        def __setattr__(self, name, value):
            observed.append(sys.getrefcount(value))

    def source_local(target, value):
        before = sys.getrefcount(value)
        target.value = value
        return before

    before = source_local(Target(), object())
    # The setter's Python argument is the one extra owner; there is no owned
    # expression-stack COPY of the existing source local.
    assert observed == [before + 1]


# Public argument ownership and cleanup, with a separate ordinary-CPython
# schedule control. SOAC comparisons exclude transient counts and opcode choice.

_SOURCE_ARGUMENT_OWNERSHIP_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def argument_keep(value, probe, make, finish):
    probe("entered")
    finish()
    probe("before-return")

def argument_delete(value, probe, make, finish):
    probe("entered")
    del value
    probe("after-delete")
    finish()
    probe("before-return")

def argument_rebind(value, probe, make, finish):
    probe("entered")
    value = make("replacement")
    probe("after-rebind")
    finish()
    probe("before-return")

def argument_alias(value, probe, make, finish):
    probe("entered")
    alias = value
    probe("aliased")
    del value
    probe("after-delete")
    finish()
    probe("before-return")

def argument_expanded(value, *, keyword, probe, finish):
    probe("entered")
    del value
    del keyword
    probe("after-delete")
    finish()
    probe("before-return")

def retire_arguments(first, second):
    return None
"""


_SOURCE_ARGUMENT_OWNERSHIP_NAMES = (
    "argument_keep",
    "argument_delete",
    "argument_rebind",
    "argument_alias",
    "argument_expanded",
)


_SOURCE_ARGUMENT_OWNERSHIP_CASES = (
    tuple(
        (name, caller, outcome, 0)
        for name in _SOURCE_ARGUMENT_OWNERSHIP_NAMES[:-1]
        for caller in ("factory", "local")
        for outcome in ("success", "error")
    )
    + tuple(
        ("argument_keep", "borrowed-c", outcome, 0) for outcome in ("success", "error")
    )
    + tuple(
        ("argument_expanded", "expanded", outcome, warmups)
        for warmups in (0, 64)
        for outcome in ("success", "error")
    )
)


_SOURCE_ARGUMENT_OWNERSHIP_OBSERVER = r"""
def observe_source_argument_ownership(
    function, caller_kind, outcome, warmups, *, native_schedule=False,
):
    import dis
    import gc
    import sys
    import weakref

    events = []
    references = {}
    caller_error = KeyError("caller handler")
    failure = LookupError("source failure")
    measuring = False
    labels = ("input", "replacement", "keyword")

    def context():
        current = sys.exception()
        if current is caller_error:
            return "caller"
        if current is failure:
            return "failure"
        return None if current is None else type(current).__name__

    def snapshot():
        values = []
        for label in labels:
            reference = references.get(label)
            value = None if reference is None else reference()
            # The ordinary observer's temporary strong reference is the only
            # observer edge. No observed payload is passed into a source-call
            # argument, so outbound argument preparation cannot inflate this.
            count = 0 if value is None else sys.getrefcount(value) - 1 if native_schedule else 1
            values.append((label, count))
            del value
        return tuple(values)

    def probe(label):
        events.append(("probe", label, snapshot(), context()))

    class Payload:
        def __init__(self, label):
            self.label = label
        def __del__(self):
            events.append(("drop", self.label, context()))

    def make(label):
        value = Payload(label)
        references[label] = weakref.ref(value)
        events.append(("made", label, context()))
        return value

    def finish():
        events.append(("finish", context()))
        if measuring and outcome == "error":
            raise failure

    # A keyword-name subclass is owned by the transient kwargs/kwnames
    # containers but is not the canonical string stored in the callee's code.
    # Its destructor exposes container retirement without a referrer snapshot.
    class Keyword(str):
        def __del__(self):
            events.append(("drop-key", context()))

    def positionals():
        return (make("input"),)

    def keywords():
        return {Keyword("keyword"): make("keyword")}

    # Fresh ordinary code gives the cold case a genuinely cold CALL_FUNCTION_EX
    # site. Only this caller is compiled here; `function` is the actual
    # published source function, never a replacement or synthetic grant.
    caller_namespace = {}
    exec(compile(
        "def invoke(function, positionals, keywords, probe, finish):\n"
        "    return function(*positionals(), probe=probe, finish=finish, **keywords())\n",
        "<ordinary expanded source caller>", "exec", dont_inherit=True,
    ), caller_namespace)
    invoke_expanded = caller_namespace["invoke"]

    def expanded_opcode():
        instructions = [
            instruction.opname
            for instruction in dis.get_instructions(invoke_expanded, adaptive=True)
            if instruction.opname in {
                "CALL_FUNCTION_EX", "CALL_EX_PY", "CALL_EX_NON_PY_GENERAL",
                "INSTRUMENTED_CALL_FUNCTION_EX",
            }
        ]
        assert len(instructions) == 1, instructions
        return instructions[0]

    def invoke():
        if caller_kind == "factory":
            return function(make("input"), probe, make, finish)
        if caller_kind == "local":
            value = make("input")
            try:
                return function(value, probe, make, finish)
            finally:
                probe("caller-before-release")
                del value
                probe("caller-after-release")
        if caller_kind == "borrowed-c":
            import _testcapi
            arguments = (make("input"), probe, make, finish)
            try:
                # This existing helper borrows the tuple's element array and
                # calls the public PyObject_Vectorcall. It does not grant an
                # interpreter-owned source-stack transfer.
                return _testcapi.pyobject_vectorcall(function, arguments, None)
            finally:
                probe("caller-before-release")
                assert references["input"]() is not None
                assert arguments[0] is references["input"]()
                del arguments
                probe("caller-after-release")
        assert caller_kind == "expanded"
        return invoke_expanded(function, positionals, keywords, probe, finish)

    call_shape = None
    try:
        raise caller_error
    except KeyError:
        if caller_kind == "expanded":
            if native_schedule:
                # Exact opcode/container observations belong only to the
                # ordinary CPython control, never to SOAC's calling convention.
                assert sys.gettrace() is None and sys.getprofile() is None
                call_events = (
                    sys.monitoring.events.CALL
                    | sys.monitoring.events.C_RETURN
                    | sys.monitoring.events.C_RAISE
                )
                for tool in range(6):
                    if sys.monitoring.get_tool(tool) is not None:
                        assert not sys.monitoring.get_events(tool) & call_events
            for _ in range(warmups):
                assert invoke() is None
                gc.collect()
                assert not any(reference() is not None for reference in references.values())
            if native_schedule:
                call_shape = expanded_opcode()
                if warmups:
                    assert call_shape in {"CALL_EX_PY", "CALL_EX_NON_PY_GENERAL"}, call_shape
                else:
                    assert call_shape == "CALL_FUNCTION_EX", call_shape
            events.clear()
            references.clear()
        else:
            assert warmups == 0
        measuring = True
        try:
            result = invoke()
        except LookupError as caught:
            assert outcome == "error" and caught is failure
            assert caught.__context__ is caller_error
            probe("caught")
            # A retained native source frame may own a surviving source alias
            # or replacement. Clear the traceback at the same explicit point.
            caught.__traceback__ = None
            probe("traceback-cleared")
        else:
            assert outcome == "success" and result is None
            probe("returned")
        assert sys.exception() is caller_error
        probe("after-call")
    gc.collect()
    probe("after-handler")
    return {"events": events, "expanded_call_shape": call_shape, "caller_kind": caller_kind}

def source_argument_semantics(observed):
    events = observed['events']
    final = events[-1]
    assert final[:2] == ('probe', 'after-handler') and final[3] is None, events
    assert not any(dict(final[2]).values()), ('dead argument retained after collection', events)
    made = sorted(event[1] for event in events if event[0] == 'made')
    dropped = sorted(event[1] for event in events if event[0] == 'drop')
    assert made == dropped, ('missing or duplicate required finalizer', events)
    assert sum(event[0] == 'drop-key' for event in events) == int(observed['caller_kind'] == 'expanded'), events
    assert sum(event[0] == 'finish' for event in events) == 1, events
    return [
        (event[0], event[1], event[3]) if event[0] == 'probe' else event
        for event in events if event[0] not in {'drop', 'drop-key'}
    ]
"""


def _source_argument_probe(events, label):
    matching = [event for event in events if event[:2] == ("probe", label)]
    assert len(matching) == 1, (label, matching, events)
    return dict(matching[0][2]), matching[0][3]


@pytest.mark.parametrize(
    "case",
    _SOURCE_ARGUMENT_OWNERSHIP_CASES,
    ids=["-".join(map(str, case)) for case in _SOURCE_ARGUMENT_OWNERSHIP_CASES],
)
def test_native_source_argument_owner_handoff(case):
    name, caller, outcome, warmups = case
    namespace = {}
    exec(
        _SOURCE_ARGUMENT_OWNERSHIP_SOURCE.replace(
            "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1
        ),
        namespace,
    )
    exec(_SOURCE_ARGUMENT_OWNERSHIP_OBSERVER, namespace)
    observed = namespace["observe_source_argument_ownership"](
        namespace[name], caller, outcome, warmups, native_schedule=True
    )
    events = observed["events"]
    entered, context = _source_argument_probe(events, "entered")
    assert context == "caller"
    assert entered["input"] == (2 if caller == "borrowed-c" else 1)
    if name == "argument_alias":
        aliased, _ = _source_argument_probe(events, "aliased")
        assert aliased["input"] == (2 if caller == "factory" else 1)
        after_delete, _ = _source_argument_probe(events, "after-delete")
        assert after_delete["input"] == 1
    elif name == "argument_delete":
        after_delete, _ = _source_argument_probe(events, "after-delete")
        assert after_delete["input"] == (1 if caller == "local" else 0)
    elif name == "argument_rebind":
        after_rebind, _ = _source_argument_probe(events, "after-rebind")
        assert after_rebind["input"] == (1 if caller == "local" else 0)
        assert after_rebind["replacement"] == 1
    elif name == "argument_expanded":
        assert entered["keyword"] == 1
        assert events.count(("drop-key", "caller")) == 1
        assert events.index(("drop-key", "caller")) < next(
            index
            for index, event in enumerate(events)
            if event[:2] == ("probe", "entered")
        )
        after_delete, _ = _source_argument_probe(events, "after-delete")
        assert after_delete["input"] == after_delete["keyword"] == 0
        if warmups:
            assert observed["expanded_call_shape"] == "CALL_EX_PY"
    if caller == "borrowed-c" and outcome == "success":
        before_release, _ = _source_argument_probe(events, "caller-before-release")
        assert before_release["input"] == 1
    if outcome == "error":
        _, context = _source_argument_probe(events, "caught")
        assert context == "failure"
        _, context = _source_argument_probe(events, "traceback-cleared")
        assert context == "failure"
    final, context = _source_argument_probe(events, "after-handler")
    assert context is None and not any(final.values())
    assert sum(event[:2] == ("drop", "input") for event in events) == 1
    if name == "argument_rebind":
        assert sum(event[:2] == ("drop", "replacement") for event in events) == 1




_METADATA_REPLACEMENT_API = r"""
import ctypes
import gc
import pytest
from soac import _soac_ext

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = arguments
    function.restype = result
    return function

obj = ctypes.py_object
get_metadata = api("PyFunction_GetSoacMetadata", ctypes.c_void_p, obj)
set_metadata = api(
    "PyFunction_SetSoacMetadata", ctypes.c_int,
    obj, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_void_p,
)
get_owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
get_seal = api("PyFunction_GetSoacStrictId", ctypes.c_uint64, obj)
source_id = api("PyCode_GetSoacStrictSourceId", ctypes.c_uint64, obj)
legacy_id = api("PyFunction_GetSoacFunctionId", ctypes.c_uint64, obj)
get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
vectorcall = api(
    "PyObject_Vectorcall", obj, obj, ctypes.POINTER(obj),
    ctypes.c_size_t, ctypes.c_void_p,
)
entry_signature = ctypes.PYFUNCTYPE(
    obj, obj, ctypes.POINTER(obj), ctypes.c_size_t, ctypes.c_void_p,
)
destructor_signature = ctypes.CFUNCTYPE(None, ctypes.c_void_p)

def c_call(function, *values):
    arguments = (obj * len(values))(*values) if values else None
    return vectorcall(function, arguments, len(values), None)

def saved_entry(function):
    # Keep the actual public ABI before replacement. PYFUNCTYPE checks the
    # pending exception, so NULL-without-error is not a passing refusal.
    pointer = get_vectorcall(function)
    assert pointer
    entry = entry_signature(pointer)
    def invoke(*values):
        arguments = (obj * len(values))(*values) if values else None
        return entry(function, arguments, len(values), None)
    return invoke

class MetadataSlot:
    def __init__(self, kind):
        self.kind = kind
        # This is a real, deliberately small foreign allocation with its own
        # destructor, never a forged SOAC payload/destructor pairing.
        self.storage = ctypes.c_ubyte(123)
        self.pointer = ctypes.addressof(self.storage)
        self.releases = []
        self.destructor = destructor_signature(self.releases.append)

    def replace(self, function):
        if self.kind == "foreign":
            pointer = self.pointer
            destructor = ctypes.cast(self.destructor, ctypes.c_void_p)
        else:
            pointer = destructor = None
        assert set_metadata(function, 0, pointer, destructor) == 0
        assert get_metadata(function) == pointer
        assert legacy_id(function) == 0

    def clear(self, function):
        # Keep the callback and its allocation alive until native ownership
        # ends. A Python function reference does not own an opaque payload.
        assert set_metadata(function, 0, None, None) == 0
        gc.collect()
        assert self.releases == ([self.pointer] if self.kind == "foreign" else [])

def assert_metadata_unavailable(function, saved, *values):
    for invoke in (lambda: function(*values), lambda: c_call(function, *values),
                   lambda: saved(*values)):
        with pytest.raises(RuntimeError, match="metadata"):
            invoke()
    with pytest.raises(RuntimeError, match="metadata"):
        _soac_ext.strict_function_entry_kind(function)
"""


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("replacement", ["cleared", "foreign"])
def test_saved_checked_entries_reject_unowned_metadata(
    strict_functions, entry_interpreter, replacement
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_functions.run(
        f"expected_entry = {expected_entry!r}\nreplacement = {replacement!r}\n"
        + _METADATA_REPLACEMENT_API
        + r"""
import checked

function = checked.identity
assert _soac_ext.strict_module_diagnostics(checked)["sealed"]
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
source_owner, seal, code_source = get_owner(function), get_seal(function), source_id(function.__code__)
assert source_owner and seal and code_source and get_metadata(function)
assert legacy_id(function) == 0
saved = saved_entry(function)
assert saved(7) == 7
assert saved("wrong") == "wrong"
original_entry = get_vectorcall(function)
slot = MetadataSlot(replacement)
try:
    slot.replace(function)
    assert get_vectorcall(function) == original_entry
    assert_metadata_unavailable(function, saved, 7)
    # Replacing an optional implementation neither revokes the native owner
    # nor grants permission to execute without an authenticated implementation.
    assert get_owner(function) == source_owner and get_seal(function) == seal
    assert source_id(function.__code__) == code_source
finally:
    slot.clear(function)

def ordinary(value):
    return value

ordinary_saved = saved_entry(ordinary)
slot = MetadataSlot(replacement)
try:
    slot.replace(ordinary)
    ordinary.__code__ = ordinary.__code__
    ordinary.__defaults__ = ("default",)
    ordinary.__kwdefaults__ = {}
    marker = object()
    assert ordinary(marker) is marker
    assert c_call(ordinary, marker) is marker
    assert ordinary_saved(marker) is marker
    assert ordinary() == "default"
    assert get_owner(ordinary) is None
    assert _soac_ext.strict_function_entry_kind(ordinary) is None
    assert get_metadata(ordinary) == (slot.pointer if replacement == "foreign" else None)
finally:
    slot.clear(ordinary)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("replacement", ["cleared", "foreign"])
def test_keyword_default_metadata_replacement_keeps_captured_invocation(
    binding_identity_project, entry_interpreter, replacement
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    binding_identity_project.run(
        f"expected_entry = {expected_entry!r}\nreplacement = {replacement!r}\n"
        + _METADATA_REPLACEMENT_API
        + r"""
import binding_identity as actual
import binding_identity_control as ordinary

assert _soac_ext.strict_module_diagnostics(actual)["sealed"]
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert _soac_ext.strict_function_entry_kind(actual.plain) == expected_entry

def exercise(function, selected):
    # Unknown decorators classify this existing source function as dynamic;
    # it owns an authenticated SOAC entry but has no sealed metadata/defaults.
    assert get_seal(function) == 0
    source_owner = get_owner(function)
    assert bool(source_owner) is selected
    assert bool(get_metadata(function)) is selected
    saved = saved_entry(function)
    assert saved() == 1
    original_entry, defaults = get_vectorcall(function), function.__kwdefaults__
    parameter_name = function.__code__.co_varnames[0]
    slot = MetadataSlot(replacement)
    marker = object()
    events = []
    class ReplacingKey:
        def __hash__(self):
            return hash(parameter_name)
        def __eq__(self, other):
            events.append(("default", other is parameter_name))
            slot.replace(function)
            if selected:
                with pytest.raises(RuntimeError, match="metadata"):
                    function(value=marker)
            else:
                assert function(value=marker) is marker
            events.append(("reentered", True))
            return other == parameter_name
    try:
        function.__kwdefaults__ = {ReplacingKey(): marker}
        assert function() is marker
        assert events == [("default", True), ("reentered", True)], events
        assert get_owner(function) == source_owner
        assert get_vectorcall(function) == original_entry
        assert get_seal(function) == 0
        function.__kwdefaults__ = defaults
        if selected:
            assert_metadata_unavailable(function, saved)
        else:
            assert function() == c_call(function) == saved() == 1
        assert events == [("default", True), ("reentered", True)], events
    finally:
        function.__kwdefaults__ = defaults
        slot.clear(function)

exercise(ordinary.plain, False)
exercise(actual.plain, True)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("replacement", ["cleared", "foreign"])
@pytest.mark.parametrize("outcome", ["value", "annotated-value", "callback-error"])
def test_body_metadata_replacement_preserves_results_exceptions_and_cleanup(
    strict_functions, entry_interpreter, replacement, outcome
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_functions.run(
        f"expected_entry = {expected_entry!r}\nreplacement = {replacement!r}\noutcome = {outcome!r}\n"
        + _METADATA_REPLACEMENT_API
        + r"""
import sys
import weakref
import checked

function = checked.finish_with_result
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
source_owner, seal = get_owner(function), get_seal(function)
assert source_owner and seal
assert get_metadata(function) and legacy_id(function) == 0
saved = saved_entry(function)
slot = MetadataSlot(replacement)
events, references, callbacks = [], [], []
class Payload:
    def __del__(self):
        events.append("drop")
def create():
    events.append("create")
    payload = Payload()
    references.append(weakref.ref(payload, lambda _: callbacks.append("weakref")))
    return payload

callback_error = RuntimeError("callback wins")
def observe(stage):
    events.append(stage)
    assert isinstance(sys.exception(), LookupError)
    assert str(sys.exception()) == "source handler"
    slot.replace(function)
    if outcome == "callback-error":
        raise callback_error

value = int("12345678901234567890") if outcome == "value" else "wrong"
outer = ValueError("caller handler")
try:
    try:
        raise outer
    except ValueError:
        try:
            result = function(create, observe, value)
        except Exception as error:
            assert outcome == "callback-error" and error is callback_error
            assert isinstance(error.__context__, LookupError)
            assert str(error.__context__) == "source handler"
            error.__context__.__traceback__ = None
            error.__context__ = None
            error.__traceback__ = None
        else:
            assert outcome in ("value", "annotated-value") and result is value
        assert sys.exception() is outer
    assert sys.exception() is None
    assert [event for event in events if event != "drop"] == ["create", "body"]
    assert get_owner(function) == source_owner and get_seal(function) == seal
    assert_metadata_unavailable(function, saved, create, observe, 7)
finally:
    slot.clear(function)
gc.collect()
assert len(references) == 1 and references[0]() is None
assert events.count("drop") == 1 and callbacks == ["weakref"], (events, callbacks)
assert [event for event in events if event != "drop"] == ["create", "body"]
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def cpython_strict_functions(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("cpython-strict-functions"),
        {"checked.py": _SOURCE, "support.py": _SUPPORT},
        modules={"checked": "checked.py"}, backend="cpython",
    )


def _native_function_capi_validation(body):
    """Reuse the original signed subject and its ordinary source-only control."""
    ordinary_source = _SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1)
    return (
        "import ctypes\nimport types\nimport pytest\nimport checked\n"
        "from soac import _soac_ext\n"
        "from soac.strict import StrictMutationError\n"
        "from tests._strict_integration import _assert_cpython_function_witness\n"
        "ordinary = types.ModuleType('ordinary_function_capi_control')\n"
        f"exec(compile({ordinary_source!r}, '<ordinary-function-capi>', 'exec', "
        "dont_inherit=True), ordinary.__dict__)\n"
        + textwrap.dedent("""
        def api(name, result, *arguments):
            function = getattr(ctypes.pythonapi, name)
            function.argtypes = list(arguments)
            function.restype = result
            return function

        obj = ctypes.py_object
        get_owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
        diagnostic = _soac_ext.strict_module_diagnostics(checked)
        for name in ("identity", "shape", "bad_return", "caller"):
            function = getattr(checked, name)
            assert get_owner(function)
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is True
            control = getattr(ordinary, name)
            assert not get_owner(control)
            assert _soac_ext.strict_function_diagnostics(control) is None
        """)
        + textwrap.dedent(body)
        + textwrap.dedent("""
        assert _soac_ext.runtime_compilation_activity() == {
            "schema": 1, "lowering_entries": 0, "blockpy_cache_entries": 0,
            "jit_engine_entries": 0,
        }
        """)
    )


@pytest.mark.parametrize("entry", ["restored-stock", "forwarder"])
def test_cpython_function_public_vectorcall_preserves_ordinary_calls_and_source_ownership(
    cpython_strict_functions, function_create_watch_extension, entry,
):
    validation = (
        f"entry = {entry!r}\nextension_path = {str(function_create_watch_extension)!r}\n"
        + textwrap.dedent("""
        import importlib.util
        spec = importlib.util.spec_from_file_location("_strict_function_create_watch", extension_path)
        native_probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(native_probe)
        with pytest.raises(TypeError):
            native_probe.install_stock_forwarder(object())

        get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
        set_vectorcall = api("PyFunction_SetVectorcall", None, obj, ctypes.c_void_p)
        stock = ctypes.cast(ctypes.pythonapi._PyFunction_Vectorcall, ctypes.c_void_p).value
        assert stock
        targets = tuple(
            getattr(module, name)
            for module in (checked, ordinary)
            for name in ("identity", "shape", "bad_return", "caller")
        )
        original = tuple(
            (function, get_vectorcall(function), get_owner(function),
             function.__code__, function.__globals__)
            for function in targets
        )
        call = api("PyObject_Call", obj, obj, obj, obj)
        one = api("PyObject_CallOneArg", obj, obj, obj)
        vector = api(
            "PyObject_Vectorcall", obj, obj, ctypes.POINTER(obj),
            ctypes.c_size_t, ctypes.c_void_p,
        )

        def vector_one(function, value):
            arguments = (obj * 1)(value)
            return vector(function, arguments, 1, None)

        def invoke_direct(function, value):
            return function(value)

        callers = (
            invoke_direct,
            lambda function, value: call(function, (value,), {}),
            one,
            vector_one,
        )
        try:
            for function, _, _, _, _ in original:
                native_probe.install_stock_forwarder(function)
                forwarder = get_vectorcall(function)
                assert forwarder and forwarder != stock
                if entry == "restored-stock":
                    set_vectorcall(function, stock)
                    assert get_vectorcall(function) == stock

            # Legal public entry changes preserve original-code execution;
            # annotations do not add argument or result predicates.
            assert _soac_ext.strict_function_diagnostics(checked.identity)["original_code_entered"] is False
            for invoke in callers:
                assert invoke(ordinary.identity, "wrong") == "wrong"
                assert invoke(checked.identity, "wrong") == "wrong"
                assert _soac_ext.strict_function_diagnostics(checked.identity)["original_code_entered"] is True
                assert invoke(ordinary.bad_return, "wrong") == "wrong"
                assert invoke(checked.bad_return, "wrong") == "wrong"
                assert _soac_ext.strict_function_diagnostics(checked.bad_return)["original_code_entered"] is True

            # Warm ordinary caller bytecode as well as each public C entry.
            # The inner call in caller must still use identity's actual owner.
            for value in range(128):
                for invoke in callers:
                    assert invoke(checked.identity, value) == invoke(ordinary.identity, value) == value
                    assert invoke(checked.caller, value) == invoke(ordinary.caller, value) == value
                    assert invoke(checked.shape, value) == invoke(ordinary.shape, value) == value + 2
            for invoke in callers:
                assert invoke(checked.identity, "wrong") == "wrong"
                assert invoke(checked.caller, "wrong") == "wrong"
                assert invoke(checked.bad_return, "wrong") == "wrong"
            assert call(checked.shape, (3, 4, 5), {"named": None, "extra": 6}) == 7
            assert call(ordinary.shape, (3, 4, 5), {"named": None, "extra": 6}) == 7
            with pytest.raises(TypeError, match="unexpected keyword"):
                call(checked.identity, ("wrong",), {"unexpected": 1})
            assert call(checked.shape, (1,), {"extra": "wrong"}) == 3
            assert call(ordinary.shape, (1,), {"extra": "wrong"}) == 3
            for function, _, owner, code, globals_ in original:
                assert get_owner(function) == owner
                assert function.__code__ is code and function.__globals__ is globals_
                if owner:
                    observed = _assert_cpython_function_witness(
                        function, diagnostic,
                    )
                    assert observed["finalized"] is True and observed["original_code_entered"] is True
                else:
                    assert _soac_ext.strict_function_diagnostics(function) is None
            from support import events
            assert "annotation evaluated" not in events
        finally:
            for function, pointer, _, _, _ in original:
                set_vectorcall(function, pointer)
                assert get_vectorcall(function) == pointer
        assert checked.identity(11) == 11
        assert checked.identity("wrong") == "wrong"
        """)
    )
    cpython_strict_functions.run_case(
        "checked",
        _native_function_capi_validation(validation),
        Path(__file__),
        required_functions=("identity", "shape", "bad_return", "caller"),
        
        backend="cpython",
    )


