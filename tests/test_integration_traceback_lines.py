from __future__ import annotations

import ast
import builtins
import importlib.util
import json
import subprocess
import sys
import traceback
import xml.etree.ElementTree as ET
from pathlib import Path
from types import ModuleType

import pytest

from tests._integration import (
    ValidationBatch,
    exec_integration_validation,
    integration_module,
    split_integration_case,
)
from tests._strict_integration import _plain_function_witness


def test_strict_case_opt_in_preserves_module_docstring_and_validation(tmp_path):
    from tests.test_integration_cases import _strict_case_source

    path = tmp_path / "documented_case.py"
    source = '''"""Original module documentation."""
from __future__ import annotations
value = 7

# diet-python: validate
assert value == 7
'''
    path.write_text(source)
    original_source, validation = split_integration_case(path)
    prepared_source = _strict_case_source(path)
    assert prepared_source.startswith(
        "# soac: module(strict_assign=true, checked_attr=true)\n"
    )
    prepared = ast.parse(prepared_source)
    assert ast.get_docstring(prepared) == "Original module documentation."
    # Comment selection must preserve every original Python statement.
    assert ast.compare(prepared, ast.parse(original_source), compare_attributes=False)
    compile(prepared_source, str(path), "exec", dont_inherit=True)
    assert isinstance(prepared.body[-1], ast.Assign)
    assert prepared.body[-1].targets[0].id == "value"
    assert validation.strip() == "assert value == 7"


@pytest.mark.parametrize("partial", [None, "partial diagnostic", b"partial diagnostic"])
def test_strict_checker_timeout_keeps_partial_diagnostics(
    tmp_path, monkeypatch, partial
):
    from tests import _strict_integration as strict

    monkeypatch.setattr(strict, "_checker", lambda: tmp_path / "checker")

    def timeout(command, **kwargs):
        assert kwargs["timeout"] == 1.25
        raise subprocess.TimeoutExpired(
            command, kwargs["timeout"], output=partial, stderr=partial
        )

    monkeypatch.setattr(strict.subprocess, "run", timeout)
    with pytest.raises(AssertionError, match="actual checker timed out after 1.25s"):
        strict.create_strict_project(
            tmp_path,
            {"example.py": "# soac: module(strict_assign=true, checked_attr=true)\n"},
            modules={"example": "example.py"},
            analysis_timeout=1.25,
        )
    expected = "" if partial is None else "partial diagnostic"
    assert (tmp_path / "checker.stdout.log").read_text() == expected
    assert (tmp_path / "checker.stderr.log").read_text() == expected
    assert not (tmp_path / "authority/deployment.json").exists()


@pytest.mark.parametrize("failure", ["nonzero", "timeout", "spawn"])
def test_checker_build_preserves_each_invocation_log_after_later_success(
    tmp_path, monkeypatch, failure,
):
    from tests import _strict_integration as strict

    monkeypatch.setattr(strict, "ROOT", tmp_path)
    directory = tmp_path / "work/logs"
    directory.mkdir(parents=True)
    historical = directory / "strict-integration-checker-build.log"
    historical.write_text("earlier retained evidence\n")
    logs = []

    def invoke(command, **kwargs):
        assert command == [
            sys.executable, str(tmp_path / "scripts/run_ty.py"),
            "--debug-build", "--", "--help",
        ]
        assert kwargs["cwd"] == tmp_path
        assert kwargs["stderr"] == subprocess.STDOUT
        assert kwargs["timeout"] == 900
        output = kwargs["stdout"]
        logs.append(Path(output.name))
        output.write("first invocation diagnostic\n" if len(logs) == 1 else "later success\n")
        if len(logs) == 1:
            if failure == "timeout":
                raise subprocess.TimeoutExpired(command, 900)
            if failure == "spawn":
                raise FileNotFoundError(2, "checker launcher is missing")
            return subprocess.CompletedProcess(command, 7)
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(strict.subprocess, "run", invoke)
    # Exercise the real fixture body without changing its process-wide cache.
    with pytest.raises(AssertionError, match="checker build") as caught:
        strict._checker.__wrapped__()
    assert str(logs[0]) in str(caught.value)
    assert logs[0].read_text() == "first invocation diagnostic\n"
    assert strict._checker.__wrapped__() == tmp_path / "work/target-ty/debug/soac-ty"
    assert len(logs) == 2 and logs[0] != logs[1]
    assert set(directory.glob("strict-integration-checker-build-*.log")) == set(logs)
    assert logs[0].read_text() == "first invocation diagnostic\n"
    assert logs[1].read_text() == "later success\n"
    assert historical.read_text() == "earlier retained evidence\n"


def test_plain_function_witnesses_keep_raw_own_namespace_identity() -> None:
    module = ModuleType("plain_witnesses")

    def function():
        pass

    class Base:
        inherited = function

    class Outer(Base):
        class Inner:
            method = function

    module.function = function
    module.Outer = Outer
    assert _plain_function_witness(module, "function") is function
    assert _plain_function_witness(module, "Outer.Inner.method") is function
    with pytest.raises(KeyError):
        _plain_function_witness(module, "Outer.inherited")
    for path in ("", ".function", "Outer..Inner.method", "Outer.Inner."):
        with pytest.raises(ValueError):
            _plain_function_witness(module, path)


def test_plain_function_witnesses_bypass_metaclass_namespace_hooks() -> None:
    events = []
    module = ModuleType("metaclass_witnesses")

    def function():
        raise AssertionError("a witness must not invoke the function")

    class Meta(type):
        def __getattribute__(cls, name):
            events.append(("getattribute", name))
            raise AssertionError("a witness must not invoke metaclass lookup")

        def __getattr__(cls, name):
            events.append(("getattr", name))
            raise AssertionError("a witness must not invoke metaclass fallback")

        @property
        def __dict__(cls):
            events.append(("dict",))
            raise AssertionError("a witness must not invoke a metaclass descriptor")

    class Base(metaclass=Meta):
        inherited = function

    class Owner(Base):
        method = function
        static = staticmethod(function)
        class_method = classmethod(function)

    module.Owner = Owner
    for name in ("method", "static", "class_method"):
        assert _plain_function_witness(module, f"Owner.{name}") is function
    with pytest.raises(KeyError):
        _plain_function_witness(module, "Owner.inherited")
    assert events == []


@pytest.mark.parametrize("kind", ["instance", "descriptor", "module"])
def test_plain_function_witnesses_reject_effectful_namespace_lookup(kind: str) -> None:
    events = []
    module = ModuleType("effectful_witnesses")

    def function():
        pass

    class Instance:
        def __getattribute__(self, name):
            events.append(name)
            if name == "__class__":
                return type
            return super().__getattribute__(name)

    class Descriptor:
        def __get__(self, obj, owner):
            events.append("descriptor")
            return function

    class Plain:
        method = Descriptor()

    class CustomModule(ModuleType):
        def __getattribute__(self, name):
            events.append(name)
            return super().__getattribute__(name)

    if kind == "instance":
        module.holder = Instance()
    elif kind == "descriptor":
        module.holder = Plain
    else:
        module = CustomModule("custom_module")
    with pytest.raises(TypeError):
        _plain_function_witness(module, "holder.method")
    assert events == []


@pytest.mark.parametrize("descriptor", [staticmethod, classmethod])
def test_plain_function_witnesses_read_exact_builtin_wrapper_without_binding(
    descriptor,
) -> None:
    module = ModuleType("wrapped_function_witnesses")

    class Owner:
        @descriptor
        def method(self):
            raise AssertionError("a witness must not invoke the function")

    module.Owner = Owner
    assert (
        _plain_function_witness(module, "Owner.method")
        is vars(Owner)["method"].__func__
    )


def test_plain_function_witnesses_read_implicit_new_staticmethod() -> None:
    module = ModuleType("implicit_new_witness")

    class Meta(type):
        def __new__(cls, name, bases, namespace):
            raise AssertionError("a witness must not construct a class")

    module.Meta = Meta
    assert type(vars(Meta)["__new__"]) is staticmethod
    assert (
        _plain_function_witness(module, "Meta.__new__")
        is vars(Meta)["__new__"].__func__
    )


@pytest.mark.parametrize(
    "descriptor",
    [
        property,
        type("CustomStatic", (staticmethod,), {}),
        type("CustomClass", (classmethod,), {}),
    ],
)
def test_plain_function_witnesses_do_not_unwrap_custom_descriptors(descriptor) -> None:
    module = ModuleType("descriptor_witnesses")

    class Owner:
        @descriptor
        def method(self):
            raise AssertionError("a witness must not invoke a descriptor")

    module.Owner = Owner
    with pytest.raises(TypeError):
        _plain_function_witness(module, "Owner.method")


@pytest.mark.parametrize(
    "failure",
    ["exception", "pytest_fail", "pytest_skip", "pytest_xfail", "system_exit"],
)
def test_validation_batch_keeps_individual_failures_and_runs_independent_cases(
    tmp_path: Path, failure: str
) -> None:
    # This exercises only the ordinary collector, not strict authentication.
    batch = ValidationBatch((), tmp_path / "batch.jsonl")
    called = []

    def fails():
        if failure == "pytest_fail":
            pytest.fail("ordinary validation failed")
        if failure == "pytest_skip":
            pytest.skip("ordinary validation failed")
        if failure == "pytest_xfail":
            pytest.xfail("ordinary validation failed")
        if failure == "system_exit":
            raise SystemExit("ordinary validation failed")
        raise RuntimeError("ordinary validation failed")

    try:
        batch.run("first", fails)
    except KeyboardInterrupt:
        raise
    except BaseException as error:
        raise AssertionError("collector leaked a validation outcome") from error
    batch.run("second", lambda: called.append("second"))
    assert batch.results["first"] is not None
    assert "ordinary validation failed" in batch.results["first"]
    assert batch.results["second"] is None
    assert called == ["second"]
    records = [json.loads(line) for line in batch.journal.read_text().splitlines()]
    assert [record["case"] for record in records] == ["first", "second"]
    assert [record["error"] for record in records] == list(batch.results.values())
    with pytest.raises(ValueError, match="already reported"):
        batch.run("second", lambda: called.append("duplicate"))
    assert called == ["second"]


@pytest.mark.parametrize("renderer_error_type", [NotImplementedError, SystemExit])
def test_validation_batch_preserves_primary_when_traceback_rendering_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, renderer_error_type: type[BaseException]
) -> None:
    batch = ValidationBatch((), tmp_path / "batch.jsonl")
    primary = RuntimeError("original validation failure")
    rendered = []
    called = []

    def unavailable(error, **kwargs):
        rendered.append(error)
        error.args = ("changed by a failing traceback renderer",)
        raise renderer_error_type("optimized source traceback instruction offset is unavailable")

    def fails():
        raise primary

    monkeypatch.setattr(traceback, "format_exception", unavailable)
    batch.run("first", fails)
    batch.run("second", lambda: called.append("second"))
    assert rendered == [primary]
    assert batch.results["first"] == (
        "RuntimeError: original validation failure\n"
        f"[traceback rendering failed: {renderer_error_type.__name__}: "
        "optimized source traceback instruction offset is unavailable]\n"
    )
    assert batch.results["second"] is None
    assert called == ["second"]
    records = [json.loads(line) for line in batch.journal.read_text().splitlines()]
    assert [record["case"] for record in records] == ["first", "second"]
    assert [record["error"] for record in records] == list(batch.results.values())


def test_validation_batch_traceback_failure_does_not_bypass_interference(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(sys, "path", list(sys.path))
    batch = ValidationBatch((), tmp_path / "batch.jsonl")
    rendered = []
    later_calls = []

    def unavailable(error, **kwargs):
        rendered.append(type(error))
        raise NotImplementedError("traceback metadata is unavailable")

    def interferes():
        sys.path.append(str(tmp_path))
        raise ValueError("callback failed before its state check")

    monkeypatch.setattr(traceback, "format_exception", unavailable)
    batch.run("interferes", interferes)
    batch.run("later", lambda: later_calls.append("incorrectly executed"))
    assert rendered == [ValueError, AssertionError]
    assert batch.contaminated is True
    assert "ValueError: callback failed before its state check" in batch.results["interferes"]
    assert "AssertionError: case changed sys.path" in batch.results["interferes"]
    assert batch.results["later"] == (
        "not executed: an earlier case changed shared process state"
    )
    assert later_calls == []
    records = [json.loads(line) for line in batch.journal.read_text().splitlines()]
    assert [record["error"] for record in records] == list(batch.results.values())


def test_validation_batch_survives_unprintable_primary_and_renderer_errors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class BrokenMessageError(Exception):
        def __str__(self):
            raise ValueError("exception string conversion failed")

    def unavailable(error, **kwargs):
        raise BrokenMessageError("secondary rendering failure")

    def fails():
        raise BrokenMessageError("primary validation failure")

    batch = ValidationBatch((), tmp_path / "batch.jsonl")
    monkeypatch.setattr(traceback, "format_exception", unavailable)
    batch.run("first", fails)
    batch.run("second", lambda: None)
    assert batch.results["first"] == (
        "BrokenMessageError: <message unavailable: ValueError>\n"
        "[traceback rendering failed: "
        "BrokenMessageError: <message unavailable: ValueError>]\n"
    )
    assert batch.results["second"] is None


@pytest.mark.parametrize("phase", ["summary", "traceback"])
def test_validation_batch_preserves_reporting_keyboard_interrupt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, phase: str
) -> None:
    rendered = []

    class InterruptedMessageError(Exception):
        def __str__(self):
            raise KeyboardInterrupt

    def interrupted(error, **kwargs):
        rendered.append(error)
        if phase == "summary":
            raise AssertionError("renderer ran after the primary summary was interrupted")
        raise KeyboardInterrupt

    def fails():
        if phase == "summary":
            raise InterruptedMessageError()
        raise RuntimeError("validation failed before cancellation")

    batch = ValidationBatch((), tmp_path / "batch.jsonl")
    monkeypatch.setattr(traceback, "format_exception", interrupted)
    with pytest.raises(KeyboardInterrupt):
        batch.run("interrupted", fails)
    assert batch.results == {}
    assert not batch.journal.exists()
    assert len(rendered) == (1 if phase == "traceback" else 0)


def test_validation_batch_preserves_keyboard_interrupt_cancellation(
    tmp_path: Path,
) -> None:
    batch = ValidationBatch((), tmp_path / "batch.jsonl")

    def interrupted():
        raise KeyboardInterrupt

    with pytest.raises(KeyboardInterrupt):
        batch.run("interrupted", interrupted)
    assert batch.results == {}
    assert not batch.journal.exists()


@pytest.mark.parametrize(
    "mutation",
    ["path", "hooks", "cwd", "builtin_names", "builtin_binding", "selected_module"],
)
def test_validation_batch_interference_prevents_later_false_passes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    monkeypatch.setattr(sys, "path", list(sys.path))
    monkeypatch.setattr(sys, "meta_path", list(sys.meta_path))
    prior_name = "_soac_validation_batch_prior"
    monkeypatch.setitem(sys.modules, prior_name, ModuleType(prior_name))
    batch = ValidationBatch((prior_name,), tmp_path / "batch.jsonl")
    later_calls = []

    def interferes():
        if mutation == "path":
            sys.path.append(str(tmp_path))
            raise RuntimeError("callback failed before its state check")
        if mutation == "hooks":
            sys.meta_path.append(sys.meta_path[0])
        elif mutation == "cwd":
            monkeypatch.chdir(tmp_path)
        elif mutation == "builtin_names":
            monkeypatch.setattr(builtins, "_soac_batch_marker", object(), raising=False)
        elif mutation == "builtin_binding":
            monkeypatch.setattr(builtins, "__doc__", "changed by a validation callback")
        elif mutation == "selected_module":
            monkeypatch.setitem(sys.modules, prior_name, ModuleType(prior_name))

    batch.run("interferes", interferes)
    batch.run("later", lambda: later_calls.append("incorrectly executed"))
    assert batch.contaminated is True
    assert batch.results["interferes"] is not None
    if mutation == "path":
        assert "callback failed before its state check" in batch.results["interferes"]
        assert "case changed sys.path" in batch.results["interferes"]
    assert batch.results["later"] is not None
    assert later_calls == []


@pytest.mark.parametrize("validator_name", ["validate", "validate_module"])
def test_validate_traceback_line_numbers(tmp_path: Path, validator_name: str) -> None:
    source = (
        "def global_function():\n"
        "    return (lambda: None).__qualname__\n"
        "\n"
        "# diet-python: validate\n"
        "\n"
        f"def {validator_name}(module):\n"
        "    assert False\n"
    )
    module_path = tmp_path / "case.py"
    module_path.write_text(source, encoding="utf-8")

    _, validate_source = split_integration_case(module_path)
    module = ModuleType("tests.integration_validate.case")
    module.__spec__ = importlib.util.spec_from_file_location(
        module.__name__, module_path
    )
    expected_line = next(
        idx
        for idx, line in enumerate(source.splitlines(), 1)
        if line.strip() == "assert False"
    )

    with pytest.raises(AssertionError) as exc_info:
        exec_integration_validation(validate_source, module, module_path, mode="stock")

    last_frame = traceback.extract_tb(exc_info.value.__traceback__)[-1]
    assert last_frame.filename == str(module_path)
    assert last_frame.lineno == expected_line


@pytest.mark.parametrize("mode", ["soac", "entry"])
def test_in_process_strict_mode_rejects_before_executing_source(
    tmp_path: Path, mode: str
) -> None:
    with (
        pytest.raises(AssertionError, match="native startup authority"),
        integration_module(
            tmp_path,
            "not_authenticated",
            "raise RuntimeError('must not execute')\n",
            mode=mode,
        ),
    ):
        pytest.fail("an unauthenticated module must never reach validation")
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize("mode", ["stock", "soac", "entry"])
def test_validator_uses_ordinary_builtins_without_changing_source_capture(
    tmp_path, mode
):
    module = ModuleType("minimal_captured_builtins")
    captured = {"len": lambda value: 41}
    module.__dict__.update(__builtins__=captured, observations=[])
    exec("def count(value): return len(value)", module.__dict__)  # noqa: S102
    original_names = set(vars(module))

    exec_integration_validation(
        """
import builtins

def validate_module(module):
    assert len([1, 2]) == builtins.len([1, 2]) == 2
    module.observations.append(module.count([1, 2]))
""",
        module,
        tmp_path / "minimal_captured_builtins.py",
        mode=mode,
    )

    assert module.observations == [41]
    assert module.__builtins__ is captured
    assert module.count.__builtins__ is captured
    assert set(vars(module)) == original_names


@pytest.mark.parametrize("validator_name", ["validate", "validate_module"])
def test_declared_validator_runs_once_without_writing_module_flags(
    tmp_path: Path, validator_name: str
) -> None:
    module = ModuleType("validation_once")
    module.calls = []
    previous_keys = set(vars(module))
    exec_integration_validation(
        f"def {validator_name}(module):\n"
        "    assert __dp_integration_mode__ == 'stock'\n"
        "    assert __dp_integration_soac__ is False\n"
        "    module.calls.append(module.__name__)\n",
        module,
        tmp_path / "once.py",
        mode="stock",
    )
    assert module.calls == ["validation_once"]
    assert set(vars(module)) == previous_keys


def test_top_level_tail_does_not_invoke_a_source_function_named_validate(
    tmp_path: Path,
) -> None:
    module = ModuleType("top_level_validation")
    module.calls = []
    module.validate_module = lambda _: module.calls.append("source function")
    exec_integration_validation(
        "calls.append('top-level')\nassert calls == ['top-level']\n",
        module,
        tmp_path / "top_level.py",
        mode="stock",
    )
    assert module.calls == ["top-level"]


@pytest.mark.parametrize(
    "validation",
    [
        "def validate(module): pass\ndef validate_module(module): pass\n",
        "async def validate_module(module): pass\n",
        "def validate(module): pass\nvalidate(None)\n",
    ],
)
def test_ambiguous_or_async_validator_is_an_explicit_harness_error(
    tmp_path: Path, validation: str
) -> None:
    with pytest.raises(ValueError):
        exec_integration_validation(
            validation,
            ModuleType("invalid_validation"),
            tmp_path / "invalid.py",
            mode="stock",
        )


@pytest.mark.parametrize(
    "exception",
    [
        "NotImplementedError('not supported')",
        "TypeError('An asyncio.Future, a coroutine or an awaitable is required')",
        "AssertionError('context: not supported')",
        "AssertionError('frame inspection mismatch')",
        "AssertionError('traceback frame differs')",
        "AssertionError('exact finalizer order differs')",
    ],
)
def test_unexpected_failures_are_not_converted_to_xfails(
    tmp_path: Path, exception: str
) -> None:
    root = Path(__file__).resolve().parents[1]
    (tmp_path / "conftest.py").write_text(
        "from tests.conftest import pytest_runtest_makereport\n"
    )
    (tmp_path / "test_unexpected.py").write_text(
        f"def test_unexpected():\n    raise {exception}\n"
    )
    report = tmp_path / "report.xml"
    program = (
        "import sys\n"
        f"sys.path.insert(0, {str(root)!r})\n"
        "import pytest\n"
        f"raise SystemExit(pytest.main([{str(tmp_path)!r}, '-q', '--junitxml={report}']))\n"
    )
    result = subprocess.run(
        [sys.executable, "-I", "-B", "-c", program],
        check=False,
        cwd=tmp_path,
        text=True,
        capture_output=True,
        timeout=30,
    )
    assert result.returncode == 1, result.stdout + result.stderr
    suite = ET.parse(report).getroot().find("testsuite")
    assert suite is not None
    assert suite.attrib["tests"] == "1"
    assert suite.attrib["failures"] == "1"
    assert suite.attrib["skipped"] == "0"


def test_excluded_frame_cases_are_collected_with_narrow_xfail_marks():
    from tests.test_integration_cases import _case_parameters

    frame_cases = {
        "yield_from_stack_names",
        "dir_filters",
        "locals_cell_contents",
        "named_expression_locals_unbound",
        "exception_cleanup_name",
    }
    parameters = {
        (parameter.values[0], parameter.values[1].stem): parameter
        for parameter in _case_parameters()
    }
    expected = {
        (mode, name) for mode in ("soac", "entry") for name in frame_cases
    }
    missing = expected - parameters.keys()
    assert not missing, f"excluded observations must remain visible in collection: {sorted(missing)}"
    for key, parameter in parameters.items():
        marks = [mark for mark in parameter.marks if mark.name == "xfail"]
        if key not in expected:
            assert not marks, f"ordinary or semantic case was unexpectedly xfailed: {key}"
            continue
        assert len(marks) == 1
        assert marks[0].kwargs["run"] is False
        assert "frame inspection" in marks[0].kwargs["reason"]
        assert "2026-08-25" in marks[0].kwargs["reason"]
    assert {("stock", name) for name in frame_cases} <= parameters.keys()


# These are fixture-scheduling tests, not synthetic strict-admission evidence.
# They mock the project boundary so a routing regression cannot run a checker,
# execute an integration initializer, or create native compilation artifacts.


def test_cohort_selection_uses_this_workers_actual_parameter_pairs(tmp_path):
    from types import SimpleNamespace

    from tests.test_integration_cases import (
        _selected_cohort_cases,
        _selected_integration_case_modes,
    )

    target = object()
    test_path = tmp_path / "test_cases.py"
    case_directory = tmp_path / "cases"

    def item(name, mode, *, function=target, path=test_path):
        return SimpleNamespace(
            path=path,
            obj=function,
            # Deliberately misleading: selection must use callspec, not text.
            nodeid="test_cases.py::test_case[soac-not_requested]",
            callspec=SimpleNamespace(
                params={"case_path": case_directory / f"{name}.py", "mode": mode}
            ),
        )

    arguments = {
        "test_path": test_path, "test_function": target, "case_directory": case_directory,
    }
    first = _selected_integration_case_modes(
        [
            item("second", "entry"),
            item("first", "soac"),
            item("first", "entry"),
            item("first", "cpython"),
            item("second", "entry"),
            item("stock_only", "stock"),
            item("other_function", "entry", function=object()),
            item("other_file", "entry", path=tmp_path / "other.py"),
            SimpleNamespace(path=test_path, obj=None),
        ],
        **arguments,
    )
    assert first == frozenset(
        {("first", "soac"), ("first", "entry"), ("second", "entry"), ("first", "cpython")}
    )
    assert _selected_cohort_cases(
        {"first": (), "second": (), "not_requested": ()}, first
    ) == {"soac": ("first",), "entry": ("first", "second"), "cpython": ("first",)}
    # A fresh worker has no inherited selection and no implicit opposite mode.
    assert _selected_integration_case_modes(
        [item("third", "entry")], **arguments
    ) == frozenset({("third", "entry")})
    assert _selected_integration_case_modes(
        [item("first", "stock")], **arguments
    ) == frozenset()


@pytest.mark.parametrize("malformed", ["missing", "mode", "outside", "string_path"])
def test_cohort_selection_rejects_malformed_requested_items(tmp_path, malformed):
    from types import SimpleNamespace

    from tests.test_integration_cases import _selected_integration_case_modes

    target = object()
    path = tmp_path / "test_cases.py"
    directory = tmp_path / "cases"
    params = {"case_path": directory / "example.py", "mode": "entry"}
    if malformed == "missing":
        del params["mode"]
    elif malformed == "mode":
        params["mode"] = "unreviewed"
    elif malformed == "outside":
        params["case_path"] = tmp_path / "example.py"
    else:
        params["case_path"] = str(params["case_path"])
    item = SimpleNamespace(path=path, obj=target, callspec=SimpleNamespace(params=params))
    with pytest.raises(ValueError, match="collected integration case"):
        _selected_integration_case_modes(
            [item], test_path=path, test_function=target, case_directory=directory
        )


def _recording_cohort_project(tmp_path, monkeypatch):
    from types import SimpleNamespace

    from tests import test_integration_cases as family

    directory = tmp_path / "cases"
    directory.mkdir()
    reviewed = dict.fromkeys(("first", "second", "isolated", "unrequested"), ("run",))
    for name in reviewed:
        (directory / f"{name}.py").write_text(
            f"def run():\n    return {name!r}\n\n"
            "# diet-python: validate\n"
            f"def validate_module(module):\n    assert module.run() == {name!r}\n"
        )
    (directory / "support.py").write_text("sentinel = object()\n")
    monkeypatch.setattr(family, "MODULES_DIR", directory)
    state = SimpleNamespace(prepared=[], backends=[], runs=[], cases={}, failures={})

    class Project:
        def __init__(self, backend):
            self.backend = backend

        def mode(self, entry_interpreter):
            assert not (self.backend == "cpython" and entry_interpreter)
            return "entry" if entry_interpreter else self.backend

        def run_cases(self, cases, *, entry_interpreter):
            assert cases, "an isolated-only selection must not start an empty batch"
            mode = self.mode(entry_interpreter)
            state.runs.append(("batch", mode, tuple(cases)))
            state.cases.update(cases)
            return {name: state.failures.get((mode, name)) for name in cases}

        def run_case(
            self, name, validation, path, *, required_functions, entry_interpreter
        ):
            mode = self.mode(entry_interpreter)
            state.runs.append(("isolated", mode, (name,)))
            state.cases[name] = family.StrictValidationCase(
                validation, path, required_functions
            )
            error = state.failures.get((mode, name))
            if error is not None:
                raise AssertionError(error)

    def prepare(root, sources, *, modules, analysis_timeout, backend="soac"):
        state.prepared.append((dict(sources), dict(modules), analysis_timeout))
        state.backends.append(backend)
        return Project(backend)

    monkeypatch.setattr(family, "create_strict_project", prepare)
    return family, reviewed, state


def test_cohort_selection_limits_runtime_and_retains_analysis_and_failures(
    tmp_path, tmp_path_factory, monkeypatch
):
    family, reviewed, state = _recording_cohort_project(tmp_path, monkeypatch)
    state.failures["soac", "second"] = "batch validation failed"
    state.failures["entry", "isolated"] = "isolated validation failed"
    override = "def validate_module(module):\n    raise AssertionError('contract')\n"
    result = family._strict_cohort_results(
        tmp_path_factory,
        "selection-test",
        reviewed,
        selected_case_modes=frozenset(
            {("first", "entry"), ("second", "soac"), ("isolated", "entry"), ("first", "cpython")}
        ),
        dependencies=("support.py",),
        isolated=("isolated",),
        validators={"second": override},
        analysis_timeout=321,
    )
    assert result == {
        "soac": {"second": "batch validation failed"},
        "entry": {"first": None, "isolated": "isolated validation failed"},
        "cpython": {"first": None},
    }
    assert state.runs == [
        ("batch", "soac", ("second",)),
        ("batch", "entry", ("first",)),
        ("isolated", "entry", ("isolated",)),
        ("batch", "cpython", ("first",)),
    ]
    assert len(state.prepared) == 2
    assert state.backends == ["soac", "cpython"]
    assert state.prepared[0] == state.prepared[1]
    sources, modules, timeout = state.prepared[0]
    assert modules == {name: f"{name}.py" for name in reviewed}
    assert sources == {
        **{path: family._strict_case_source(family.MODULES_DIR / path) for path in modules.values()},
        "support.py": (family.MODULES_DIR / "support.py").read_text(),
    }
    assert timeout == 321
    assert state.cases["second"].validate_source == override
    assert state.cases["first"].validate_source == split_integration_case(
        family.MODULES_DIR / "first.py"
    )[1]
    assert state.cases["first"].required_functions == ("run",)


@pytest.mark.parametrize("empty", [False, True])
def test_cohort_selection_isolated_only_or_empty_never_starts_a_batch(
    tmp_path, tmp_path_factory, monkeypatch, empty
):
    family, reviewed, state = _recording_cohort_project(tmp_path, monkeypatch)
    selected = frozenset() if empty else frozenset({("isolated", "entry")})
    result = family._strict_cohort_results(
        tmp_path_factory,
        "isolated-selection",
        reviewed,
        selected_case_modes=selected,
        isolated=("isolated",),
    )
    assert result == ({} if empty else {"entry": {"isolated": None}})
    assert state.runs == ([] if empty else [("isolated", "entry", ("isolated",))])
    assert len(state.prepared) == (0 if empty else 1)


@pytest.mark.parametrize("isolated", [False, True])
def test_cohort_selection_limits_interop_without_reclassifying_original_sources(
    tmp_path, tmp_path_factory, monkeypatch, isolated
):
    family, reviewed, state = _recording_cohort_project(tmp_path, monkeypatch)
    state.failures["entry", "interop_second"] = "ordinary validation failed"
    result = family._ordinary_interop_cohort_results(
        tmp_path_factory,
        "interop-selection",
        {name: ("diagnostic", witnesses) for name, witnesses in reviewed.items()},
        selected_case_modes=frozenset({("second", "entry")}),
        dependencies=("support.py",),
        isolated=isolated,
        analysis_timeout=321,
    )
    assert result == {"entry": {"second": "ordinary validation failed"}}
    assert state.runs == [
        ("isolated" if isolated else "batch", "entry", ("interop_second",))
    ]
    assert len(state.prepared) == 1
    sources, modules, timeout = state.prepared[0]
    assert modules == {f"interop_{name}": f"interop_{name}.py" for name in reviewed}
    for name in reviewed:
        assert sources[f"{name}.py"] == split_integration_case(
            family.MODULES_DIR / f"{name}.py"
        )[0]
        assert name not in modules
    assert sources["support.py"] == (family.MODULES_DIR / "support.py").read_text()
    assert timeout == 321
    assert state.cases["interop_second"].required_functions == ("invoke_validation",)


def test_cohort_selection_real_pytest_collection_reaches_all_fixture_routes(tmp_path):
    import textwrap

    root = Path(__file__).resolve().parents[1]
    journal = tmp_path / "cohort-calls.jsonl"
    # The actual pytest function/fixtures see only these worker node IDs. The
    # external project boundary records scheduling; no source is executed or
    # authenticated by this workflow control.
    nodes = [
        "entry-bounded_loop",
        "soac-slice_binding",
        "entry-mutated_function_defaults",
        "entry-frozen_dataclass",
        "entry-class_annotations_mutation",
    ]
    selectors = [
        f"{root / 'tests/test_integration_cases.py'}::test_integration_case[{node}]"
        for node in nodes
    ]
    program = textwrap.dedent(f"""\
        import json
        from pathlib import Path
        import sys
        sys.path.insert(0, {str(root)!r})
        import pytest
        from tests import test_integration_cases as family

        journal = Path({str(journal)!r})
        def record(kind, names, entry_interpreter):
            with journal.open('a') as output:
                output.write(json.dumps([kind, list(names), entry_interpreter]) + '\\n')
        class Project:
            def run_cases(self, cases, *, entry_interpreter):
                assert cases
                record('batch', cases, entry_interpreter)
                return dict.fromkeys(cases)
            def run_case(self, name, *args, entry_interpreter, **kwargs):
                record('isolated', (name,), entry_interpreter)
            def run(self, program, *, entry_interpreter):
                record('import-error', (), entry_interpreter)
        def prepare(*args, **kwargs):
            return Project()
        family.create_strict_project = prepare
        raise SystemExit(pytest.main({selectors!r} + ['-q', '-p', 'no:cacheprovider']))
        """)
    result = subprocess.run(
        [sys.executable, "-I", "-B", "-c", program],
        check=False,
        cwd=tmp_path,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert [json.loads(line) for line in journal.read_text().splitlines()] == [
        ["batch", ["slice_binding"], False],
        ["batch", ["bounded_loop"], True],
        ["batch", ["mutated_function_defaults"], True],
        ["isolated", ["interop_frozen_dataclass"], True],
        ["import-error", [], True],
    ]


def test_cpython_validation_flags_do_not_claim_soac_execution_or_mutate_source(tmp_path):
    module = ModuleType("original_code_validation")
    module.observed = []
    original_names = set(vars(module))
    exec_integration_validation(
        """
def validate_module(module):
    assert __dp_integration_mode__ == 'cpython'
    assert __dp_integration_strict__ is True
    assert __dp_integration_soac__ is False
    assert __dp_integration_entry__ is False
    module.observed.append('ordinary validator')
""",
        module, tmp_path / "original_code_validation.py", mode="cpython",
    )
    assert module.observed == ["ordinary validator"]
    assert set(vars(module)) == original_names


def test_cpython_in_process_integration_still_refuses_unauthenticated_source(tmp_path):
    with (
        pytest.raises(AssertionError, match="native startup authority"),
        integration_module(
            tmp_path, "not_authenticated",
            "raise RuntimeError('must not execute')\n", mode="cpython",
        ),
    ):
        pytest.fail("CPython selection is not startup authority")
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize("arguments", [
    {"entry_interpreter": True},
    {"opt_mode": "profile"},
    {"opt_mode": "apply"},
    {"opt_mode": "verify"},
    {"extra_env": {"SOAC_OPT_MODE": "none"}},
    {"extra_env": {"SOAC_COMPILE_MODE": "eager"}},
    {"extra_env": {"SOAC_BACKGROUND_JIT": "0"}},
    {"extra_env": {"DIET_PYTHON_MODE": "transform"}},
    {"backend": "soac"},
])
def test_cpython_fixture_rejects_conflicting_selection_before_runtime_setup(
    tmp_path, monkeypatch, arguments,
):
    from tests import _strict_integration as strict

    # This tests only fixture argument validation, never an artifact/body grant.
    project = strict.StrictProject(
        tmp_path, tmp_path / "project", tmp_path / "deployment.json",
        {}, {}, backend="cpython", environment={},
    )
    calls = []
    def unexpected(*args, **kwargs):
        calls.append((args, kwargs))
        pytest.fail("invalid backend selection reached a subprocess")
    monkeypatch.setattr(strict.subprocess, "run", unexpected)
    with pytest.raises(ValueError):
        project.run("raise AssertionError('must not execute')", **arguments)
    assert calls == []
    assert project._invocations == 0
    assert list(tmp_path.iterdir()) == []


def test_unknown_backend_refuses_before_checker_publication(tmp_path, monkeypatch):
    from tests import _strict_integration as strict

    def unexpected():
        pytest.fail("invalid backend reached checker preparation")
    monkeypatch.setattr(strict, "_checker", unexpected)
    with pytest.raises(ValueError, match="unknown strict execution backend"):
        strict.create_strict_project(
            tmp_path / "untouched", {"example.py": "# soac: module(strict_assign=true, checked_attr=true)\n"},
            modules={"example": "example.py"}, backend="not-a-backend",
        )
    assert list(tmp_path.iterdir()) == []


def test_cpython_publication_and_replay_share_one_effective_environment_policy():
    from tests._strict_integration import _effective_backend_environment

    original = {
        "LD_LIBRARY_PATH": "/actual/native/lib:/preserved/second",
        "SOAC_OPT_MODE": "apply", "SOAC_COMPILE_MODE": "eager",
        "SOAC_BACKGROUND_JIT": "1", "DIET_PYTHON_MODE": "transform",
        "UNRELATED_INPUT": "preserved",
    }
    selected = _effective_backend_environment("cpython", original)
    assert selected == {
        "LD_LIBRARY_PATH": original["LD_LIBRARY_PATH"],
        "UNRELATED_INPUT": "preserved",
    }
    assert _effective_backend_environment("cpython", selected) == selected
    assert _effective_backend_environment("soac", original) == original
    assert original["SOAC_OPT_MODE"] == "apply"
    assert _effective_backend_environment(
        "cpython", selected, extra_env={"LD_LIBRARY_PATH": "/intentional-negative"},
    )["LD_LIBRARY_PATH"] == "/intentional-negative"


@pytest.mark.parametrize("method", ["run_case", "run_cases"])
def test_cpython_project_backend_contradiction_precedes_case_preparation(
    tmp_path, monkeypatch, method,
):
    from tests import _strict_integration as strict

    project = strict.StrictProject(
        tmp_path, tmp_path / "absent-project", tmp_path / "deployment.json",
        {}, {}, backend="cpython", environment={},
    )
    def unexpected(*args, **kwargs):
        pytest.fail("contradictory backend reached checker or runtime subprocess")
    monkeypatch.setattr(strict.subprocess, "run", unexpected)
    with pytest.raises(ValueError, match="contradicts"):
        if method == "run_case":
            project.run_case("missing", "", tmp_path / "case.py", backend="soac")
        else:
            project.run_cases({}, backend="soac")
    assert project._invocations == 0
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_retained_function_witnesses_authenticate_entries_around_validation(
    tmp_path, entry_interpreter,
):
    from tests._strict_integration import StrictProject, StrictValidationCase

    # Driver construction only: the explicit policy selects the sealed-binding
    # witness shape but supplies no checker facts or execution authority.
    # Real strict-project cases exercise the resulting witnesses.
    project = StrictProject(
        tmp_path, tmp_path, tmp_path / "deployment.json",
        {"generation": "driver-only"}, {"example": "example.py"},
        backend="soac", environment={},
        policies={
            "example": {
                "strict_assign": True,
                "checked_attr": True,
                "class_overrides": [],
            },
        },
    )
    case = StrictValidationCase(
        "pass\n", tmp_path / "validate.py",
        ("first", "second"),
    )
    program = ast.parse(project._validation_program(
        "example", case, entry_interpreter=entry_interpreter, backend="soac",
    ))
    validation = next(
        node for node in ast.walk(program)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
        and node.func.id == "exec_integration_validation"
    )
    loops = [node for node in program.body if isinstance(node, ast.For)]
    assert len(loops) == 2
    for loop in loops:
        assert ast.literal_eval(loop.iter) == case.required_functions
        queries = [
            node for node in ast.walk(loop)
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
            and node.func.attr == "strict_function_entry_kind"
        ]
        assert len(queries) == 1
        assert isinstance(queries[0].args[0], ast.Name)
        assert queries[0].args[0].id == "function"
    for name in ("metadata", "owner"):
        assert any(
            isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
            and node.func.id == name
            for node in ast.walk(loops[0])
        )
    assert loops[0].end_lineno < validation.lineno < loops[1].lineno
