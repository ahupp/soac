"""Ordinary execution controls and external native iterator fixtures.

Source-only entry, comprehension and loop semantics are authored under
tests/strict_scenarios/execution and use fresh authenticated validation.
These remaining tests preserve ordinary CPython observations and native C-slot
fixtures that do not fit a source-only scenario.
"""

import json
import textwrap
from dataclasses import dataclass
from pathlib import Path

import pytest

from tests._strict_integration import ROOT, create_strict_project


@dataclass(frozen=True)
class EntryCase:
    source: str
    witness: str
    assertions: str


# Only the two ordinary comprehension controls retain their original bodies here.
CASES = {
    'executes_comprehensions_with_captures': EntryCase(
        """
        def build(values):
            scale = 2
            odd_list = [value + scale for value in values if value % 2]
            odd_dict = {value: value + scale for value in values if value % 2}
            odd_set = {value + scale for value in values if value % 2}
            return odd_list == [3, 5] and odd_dict == {1: 3, 3: 5} and odd_set == {3, 5}
        """,
        "build",
        "assert module.build((1, 2, 3)) is True",
    ),
    'dictcomp_loop_target_and_containing_walrus_have_distinct_frames': EntryCase(
        """
        def build():
            result = {(saved := item): saved for item in (1, 2)}
            return result, saved
        """,
        "build",
        "assert module.build() == ({1: 1, 2: 2}, 2)",
    ),
}


_COMPREHENSION_CAPTURE_CASES = (
    "executes_comprehensions_with_captures",
    "dictcomp_loop_target_and_containing_walrus_have_distinct_frames",
)


@pytest.mark.parametrize("case_name", _COMPREHENSION_CAPTURE_CASES)
def test_eager_comprehension_original_stock_control(tmp_path, case_name):
    import ctypes
    from pathlib import Path
    from tests._integration import exec_integration_validation, stock_module

    case = CASES[case_name]
    source = textwrap.dedent(case.source).lstrip("\n")
    with stock_module(tmp_path, f"ordinary_{case_name}", source) as module:
        function = vars(module)[case.witness]
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) is None and metadata(function) is None
        validation = "def validate_module(module):\n" + textwrap.indent(
            textwrap.dedent(case.assertions).strip() + "\n", "    "
        )
        exec_integration_validation(
            validation, module, Path(module.__file__), mode="stock"
        )
        assert owner(function) is None and metadata(function) is None


_LOOP_RECEIVER_SOURCE = """
def exhaust(iterator, observe):
    observe('before')
    for value in iterator:
        observe('body')
    observe('after')
"""


def _for_loop_receiver_observations(module):
    """Keep native count diagnostics separate from callback and cleanup checks."""
    import gc
    import sys
    import weakref

    events = []
    reference = None

    def observe(label):
        receiver = reference()
        assert receiver is not None
        events.append((label, sys.getrefcount(receiver)))

    # This class and observer remain ordinary Python outside the strict project.
    # The observer holds only a weak reference between calls, and uses the same
    # temporary strong receiver reference at every measurement point.
    class ObservedIterator:
        def __init__(self):
            self.position = 0

        def __iter__(self):
            return self

        def __next__(self):
            observe("next")
            if self.position == 2:
                raise StopIteration
            self.position += 1
            return self.position

    iterator = ObservedIterator()
    reference = weakref.ref(iterator)
    assert module.exhaust(iterator, observe) is None
    assert [label for label, _ in events] == [
        "before",
        "next",
        "body",
        "next",
        "body",
        "next",
        "after",
    ]
    baseline = events[0][1]
    body_baseline = events[2][1]
    del iterator
    gc.collect()
    assert reference() is None, 'completed loop retained its iterator'
    return {
        "entry_relative": tuple(count - baseline for _, count in events),
        "next_over_body": tuple(
            count - body_baseline for label, count in events if label == "next"
        ),
        "after_over_entry": events[-1][1] - baseline,
    }, events


def test_for_loop_next_receiver_native_control(tmp_path):
    from tests._integration import stock_module

    with stock_module(
        tmp_path, "ordinary_loop_receiver", _LOOP_RECEIVER_SOURCE
    ) as module:
        observed, counts = _for_loop_receiver_observations(module)
    (tmp_path / "receiver-counts.json").write_text(
        json.dumps(
            {
                "observed": observed,
                "absolute_counts": counts,
            },
            indent=2,
        )
        + "\n"
    )
    # FOR_ITER retains the loop iterator, and the ordinary __next__ activation
    # has its own self reference; it does not clone another call operand.
    assert observed["next_over_body"] == (1, 1, 1), (observed, counts)
    assert observed["after_over_entry"] == 0, (observed, counts)


_LOOP_EXIT_SOURCE = """
def exhaust(make, observe, target):
    for value in make():
        observe('body')
    else:
        observe('else')
    observe('after')

def break_loop(make, observe, target):
    for value in make():
        observe('body')
        break
    else:
        observe('unexpected-else')
    observe('after')

def return_loop(make, observe, target):
    for value in make():
        return observe('return-value')

def failing_target(make, observe, target):
    try:
        for target.value in make():
            observe('unexpected-body')
    except ValueError:
        observe('caught-outer')
    observe('after')

def failing_body(make, observe, target):
    try:
        for value in make():
            observe('body')
            raise ValueError('body error')
    except ValueError:
        observe('caught-outer')
    observe('after')

def caught_continue(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            observe('caught-inner')
            continue
    observe('after')

def caught_break(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            observe('caught-inner')
            break
    observe('after')

def caught_return(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            return observe('return-value')

def failing_finally(make, observe, target):
    try:
        for value in make():
            try:
                raise ValueError('body error')
            finally:
                observe('finally')
    except ValueError:
        observe('caught-outer')
    observe('after')
"""

_LOOP_EXIT_CASES = (
    "exhaust", "break_loop", "return_loop", "failing_target", "failing_body",
    "caught_continue", "caught_break", "caught_return", "failing_finally",
)


def _for_loop_exit_observations(module, case):
    """Observe a temporary iterator without adding a lasting receiver owner."""
    import gc
    import sys
    import weakref

    events = []
    reference = None

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    def observe(label):
        events.append((label, reference() is not None, context()))
        return "return-token"

    class Iterator:
        def __init__(self):
            self.position = 0

        def __iter__(self):
            return self

        def __next__(self):
            observe("next")
            if self.position == 2:
                raise StopIteration
            self.position += 1
            return self.position

        def __del__(self):
            events.append(("drop", context()))

    def make():
        nonlocal reference
        iterator = Iterator()
        reference = weakref.ref(iterator)
        return iterator

    class Target:
        def __setattr__(self, name, value):
            observe("target")
            raise ValueError("target error")

    # Explicit source callbacks inherit the real handled-exception context.
    # An implicit finalizer's timing/context is recorded separately below.
    try:
        raise KeyError("caller")
    except KeyError:
        result = getattr(module, case)(make, observe, Target())
        observe("caller")
    gc.collect()
    assert reference() is None, ('completed loop retained its iterator', case, events)
    assert sum(event[0] == 'drop' for event in events) == 1, (case, events)
    return {"events": events, "result": result}


@pytest.mark.parametrize("case", _LOOP_EXIT_CASES)
def test_for_loop_exit_native_control(tmp_path, case):
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_loop_exit", _LOOP_EXIT_SOURCE) as module:
        observed = _for_loop_exit_observations(module, case)
    (tmp_path / "loop-exit.json").write_text(json.dumps(observed, indent=2) + "\n")
    events = observed["events"]
    assert [event for event in events if event[0] == "drop"] == [("drop", "KeyError")]
    assert events[-1] == ("caller", False, "KeyError")
    for event in events:
        if event[0] in {"body", "next", "target", "caught-inner", "return-value", "finally"}:
            assert event[1] is True, (case, observed)
        elif event[0] in {"else", "after", "caught-outer"}:
            assert event[1] is False, (case, observed)
        assert not event[0].startswith("unexpected-"), (case, observed)
    if case in {"return_loop", "caught_return"}:
        assert observed["result"] == "return-token"
    else:
        assert observed["result"] is None


_LOOP_TRACEBACK_SOURCE = """
def exhaust(iterator, make_payload):
    payload = make_payload()
    for item in iterator:
        pass
"""


@pytest.fixture(scope="module")
def loop_error_native_extension(tmp_path_factory):
    """A real tp_iternext error without a Python callback frame."""
    import hashlib
    from pathlib import Path
    import shlex
    import subprocess
    import sys
    import sysconfig

    source = Path(sysconfig.get_config_var("abs_srcdir"))
    build = Path(sysconfig.get_config_var("abs_builddir"))
    assert (build / "python").resolve() == Path(sys._base_executable).resolve()
    out = tmp_path_factory.mktemp("native-loop-error")
    extension = out / ("_loop_error_native" + sysconfig.get_config_var("EXT_SUFFIX"))
    probe = ROOT / "tests/native/iterator_error.c"
    command = [
        *shlex.split(sysconfig.get_config_var("LDSHARED")),
        *shlex.split(sysconfig.get_config_var("CCSHARED")),
        "-O0", "-g", "-Wall", "-Wextra", "-Werror",
        f"-I{source / 'Include'}", f"-I{build}", str(probe), "-o", str(extension),
    ]
    result = subprocess.run(command, capture_output=True, text=True, timeout=60, check=False)
    (out / "build.log").write_text(shlex.join(command) + "\n" + result.stdout + result.stderr)
    assert result.returncode == 0, result.stderr
    (out / "identity.json").write_text(json.dumps({
        "source": str(probe),
        "source_sha256": hashlib.sha256(probe.read_bytes()).hexdigest(),
        "extension": str(extension),
        "extension_sha256": hashlib.sha256(extension.read_bytes()).hexdigest(),
        "native_source": str(source),
        "native_build": str(build),
    }, indent=2) + "\n")
    return extension


def _for_loop_error_traceback_observations(
    module, extension_path, native_slot, exhaustion, *, implicit=True, native_frames=True,
):
    import gc
    import importlib.util
    import weakref

    spec = importlib.util.spec_from_file_location("_loop_error_native", extension_path)
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    events = []
    payload_refs = []
    retained = StopIteration("exhausted") if exhaustion else ValueError("real error")

    class Payload:
        def __del__(self):
            events.append("deleted")

    def make_payload():
        value = Payload()
        events.append("created")
        payload_refs.append(weakref.ref(value))
        return value

    class PythonIterator:
        def __iter__(self):
            return self

        def __next__(self):
            raise retained

    iterator = native.make(retained) if native_slot else PythonIterator()
    if exhaustion and implicit:
        assert module.exhaust(iterator, make_payload) is None
    else:
        try:
            module.exhaust(iterator, make_payload)
        except type(retained) as error:
            assert error is retained
        else:
            raise AssertionError("the iterator error must propagate")

    # Frame shape and retained-local lifetime are ordinary CPython controls,
    # not prerequisites for SOAC exception propagation or required cleanup.
    source_lines = []
    if native_frames:
        source_code = module.exhaust.__code__
        traceback = retained.__traceback__
        while traceback is not None:
            if traceback.tb_frame.f_code is source_code:
                source_lines.append(traceback.tb_lineno - source_code.co_firstlineno)
            traceback = traceback.tb_next
    observed = {
        "payload_alive_before_clear": payload_refs[0]() is not None,
        "events_before_clear": events.copy(),
        "source_lines": source_lines,
        "traceback_absent": retained.__traceback__ is None,
    }
    retained.__traceback__ = None
    gc.collect()
    observed["payload_alive_after_clear"] = payload_refs[0]() is not None
    observed["events_after_clear"] = events.copy()
    return observed


def _loop_error_semantics(observed):
    assert observed["payload_alive_after_clear"] is False, observed
    assert sorted(observed["events_after_clear"]) == ["created", "deleted"], observed
    return observed["payload_alive_after_clear"], sorted(observed["events_after_clear"])


@pytest.mark.parametrize("native_slot", [True, False], ids=["c-slot", "python-next"])
@pytest.mark.parametrize("exhaustion", [True, False], ids=["exhaustion", "real-error"])
def test_for_loop_error_traceback_native_control(
    tmp_path, loop_error_native_extension, native_slot, exhaustion,
):
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_loop_traceback", _LOOP_TRACEBACK_SOURCE) as module:
        observed = _for_loop_error_traceback_observations(
            module, loop_error_native_extension, native_slot, exhaustion,
        )
    (tmp_path / "loop-traceback.json").write_text(json.dumps(observed, indent=2) + "\n")
    # C exhaustion has no traceback. Python __next__ does retain its callback
    # traceback, whose frame-back edge keeps the source frame's payload alive.
    # A real error adds the source loop frame for either iterator backend.
    should_retain = not exhaustion or not native_slot
    assert observed == {
        "payload_alive_before_clear": should_retain,
        "events_before_clear": ["created"] if should_retain else ["created", "deleted"],
        "source_lines": [] if exhaustion else [2],
        "traceback_absent": native_slot and exhaustion,
        "payload_alive_after_clear": False,
        "events_after_clear": ["created", "deleted"],
    }


@pytest.fixture(scope="module")
def strict_loop_traceback_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-loop-traceback"),
        {"loop_traceback.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _LOOP_TRACEBACK_SOURCE},
        modules={"loop_traceback": "loop_traceback.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("native_slot", [True, False], ids=["c-slot", "python-next"])
@pytest.mark.parametrize("exhaustion", [True, False], ids=["exhaustion", "real-error"])
def test_for_loop_error_preserves_exception_identity_and_cleanup(
    strict_loop_traceback_project, tmp_path, loop_error_native_extension,
    entry_interpreter, native_slot, exhaustion,
):
    from pathlib import Path
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_loop_traceback", _LOOP_TRACEBACK_SOURCE) as module:
        expected = _loop_error_semantics(_for_loop_error_traceback_observations(
            module, loop_error_native_extension, native_slot, exhaustion,
        ))
    validation = f"""
def validate(module):
    import json
    from tests.test_strict_entry_runtime import (
        _for_loop_error_traceback_observations, _loop_error_semantics,
    )
    actual = _loop_error_semantics(_for_loop_error_traceback_observations(
        module, {str(loop_error_native_extension)!r}, {native_slot!r}, {exhaustion!r},
        native_frames=False,
    ))
    print(json.dumps({{'actual': actual, 'expected': {expected!r}}}))
    assert actual == {expected!r}, (actual, {expected!r})
"""
    strict_loop_traceback_project.run_case(
        "loop_traceback", validation, Path(__file__),
        entry_interpreter=entry_interpreter, required_functions=("exhaust",),
    )


_EXPLICIT_NEXT_TRACEBACK_SOURCE = """
def exhaust(iterator, make_payload):
    payload = make_payload()
    next(iterator)
"""


@pytest.mark.parametrize("native_slot", [True, False], ids=["c-slot", "python-next"])
def test_explicit_next_traceback_native_control(tmp_path, loop_error_native_extension, native_slot):
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_next_traceback", _EXPLICIT_NEXT_TRACEBACK_SOURCE) as module:
        observed = _for_loop_error_traceback_observations(
            module, loop_error_native_extension, native_slot, True, implicit=False,
        )
    assert observed == {
        "payload_alive_before_clear": True,
        "events_before_clear": ["created"],
        "source_lines": [2],
        "traceback_absent": False,
        "payload_alive_after_clear": False,
        "events_after_clear": ["created", "deleted"],
    }


@pytest.fixture(scope="module")
def strict_explicit_next_traceback_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-next-traceback"),
        {"next_traceback.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _EXPLICIT_NEXT_TRACEBACK_SOURCE},
        modules={"next_traceback": "next_traceback.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("native_slot", [True, False], ids=["c-slot", "python-next"])
def test_explicit_next_preserves_exception_identity_and_cleanup(
    strict_explicit_next_traceback_project, tmp_path, loop_error_native_extension,
    entry_interpreter, native_slot,
):
    from pathlib import Path
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_next_traceback", _EXPLICIT_NEXT_TRACEBACK_SOURCE) as module:
        expected = _loop_error_semantics(_for_loop_error_traceback_observations(
            module, loop_error_native_extension, native_slot, True, implicit=False,
        ))
    validation = f"""
def validate(module):
    import json
    from tests.test_strict_entry_runtime import (
        _for_loop_error_traceback_observations, _loop_error_semantics,
    )
    actual = _loop_error_semantics(_for_loop_error_traceback_observations(
        module, {str(loop_error_native_extension)!r}, {native_slot!r}, True, implicit=False,
        native_frames=False,
    ))
    print(json.dumps({{'actual': actual, 'expected': {expected!r}}}))
    assert actual == {expected!r}, (actual, {expected!r})
"""
    strict_explicit_next_traceback_project.run_case(
        "next_traceback", validation, Path(__file__),
        entry_interpreter=entry_interpreter, required_functions=("exhaust",),
    )


_EAGER_COMPREHENSION_FRAME_SOURCE = """
def nested_builtin_positional(mapping, key):
    return [mapping.get(value) for value in (key,)][0]

def schedule(owners):
    return [owner.link for owner in owners]

def preserve_outer(make, visit):
    item = make('outer')
    [visit() for item in (make('inner'),)]
    return 'done'
"""


def _eager_comprehension_frame_observations(module, exceptional=False):
    import gc
    import weakref

    class Owner:
        def __init__(self, link):
            self.link = link

    # These are the unchanged bodies that exposed the missing parent-slot
    # projection. No hand-written loop or replacement validator stands in.
    result = {
        'builtin': module.nested_builtin_positional({'key': 41}, 'key'),
        'field': module.schedule([Owner(11), Owner(31)]),
    }
    events = []
    references = {}
    errors = []
    callbacks = []
    marker = ValueError('unwind the original region')

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(self.label)

    def make(label):
        callbacks.append(('make', label))
        value = Payload(label)
        references[label] = weakref.ref(value)
        return value

    def visit():
        callbacks.append(('visit',))
        if exceptional:
            raise marker
        try:
            raise ValueError('retain the callback traceback')
        except ValueError as error:
            errors.append(error)

    try:
        if exceptional:
            try:
                module.preserve_outer(make, visit)
            except ValueError as error:
                assert error is marker
                errors.append(error)
            else:
                raise AssertionError('the unchanged callback must raise')
        else:
            assert module.preserve_outer(make, visit) == 'done'
        assert len(errors) == 1
        assert callbacks == [('make', 'outer'), ('make', 'inner'), ('visit',)]
        # Ordinary retained traceback/frame-back ownership keeps the restored
        # outer local, not the temporary target, after normal or error exit.
        # No f_locals/f_back introspection is needed to observe this lifetime.
        result['outer_before_clear'] = references['outer']() is not None
        result['inner_before_clear'] = references['inner']() is not None
        result['events_before_clear'] = events.copy()
        errors[0].__traceback__ = None
        gc.collect()
        result['outer_after_clear'] = references['outer']() is not None
        result['inner_after_clear'] = references['inner']() is not None
        result['events_after_clear'] = events.copy()
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()
    return result


@pytest.mark.parametrize('exceptional', [False, True], ids=['normal', 'exception'])
def test_eager_comprehension_source_frame_native_control(tmp_path, exceptional):
    from tests._integration import stock_module

    with stock_module(
        tmp_path, 'ordinary_comprehension_source_frame',
        _EAGER_COMPREHENSION_FRAME_SOURCE,
    ) as module:
        observed = _eager_comprehension_frame_observations(module, exceptional)
    assert observed == {
        'builtin': 41,
        'field': [11, 31],
        'outer_before_clear': True,
        'inner_before_clear': False,
        'events_before_clear': ['inner'],
        'outer_after_clear': False,
        'inner_after_clear': False,
        'events_after_clear': ['inner', 'outer'],
    }

