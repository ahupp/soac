from __future__ import annotations

import json
import os
import subprocess
import textwrap
from pathlib import Path

from tests._integration import stock_module
from tests._strict_integration import (
    _VALIDATION_PRELUDE,
    StrictProject,
    StrictValidationCase,
    assert_strict_source_rejected,
    create_strict_project,
)


# These are the exact original source literals, shared by their ordinary
# controls and independently selected strict-admission decisions.
_BROKEN_SOURCE = """
        class Marker(Exception):
            pass

        class Broken:
            def __init__(self, value):
                raise Marker(f"boom:{value}")

        def run():
            try:
                Broken(7)
            except Marker as exc:
                return [type(exc).__name__, str(exc), exc.__context__ is None]
        """

_CUSTOM_NEW_SOURCE = """
        events = []

        class Custom:
            def __new__(cls, value):
                events.append(f"new:{value}")
                obj = object.__new__(cls)
                obj.from_new = value + 1
                return obj

            def __init__(self, value):
                events.append(f"init:{value}:{self.from_new}")
                self.value = value

        def run():
            obj = Custom(5)
            return [events, obj.value, obj.from_new]
        """


def _run_script(
    project: StrictProject,
    module_name: str,
    required_functions: tuple[str, ...],
    env: dict[str, str],
    *,
    opt_mode: str,
) -> subprocess.CompletedProcess[str]:
    program = _VALIDATION_PRELUDE + project._validation_program(
        module_name,
        StrictValidationCase(
            """
def validate_module(module):
    import json
    print(json.dumps(module.run()))
""",
            Path(__file__),
            required_functions,
        ),
        entry_interpreter=False,
        backend="soac",
    )
    return project.run(program, opt_mode=opt_mode, extra_env=env, check=False)


def _run_apply_module(
    tmp_path: Path,
    module_name: str,
    source: str,
    *,
    required_functions: tuple[str, ...],
) -> tuple[object, list[dict[str, object]]]:
    filename = f"{module_name}.py"
    project = create_strict_project(
        tmp_path / "strict",
        {filename: "from __future__ import strict\n" + textwrap.dedent(source)},
        modules={module_name: filename},
        backend="soac",
    )
    work_dir = tmp_path / "soac-work"
    log_path = work_dir / "apply-events.jsonl"

    # Preserve the original inherited Lazy/Eager and background settings.
    # The existing checked trampoline is installed even while its body is Lazy;
    # the standard validation program does not require eager body compilation.
    environment = project.environment if project.environment is not None else os.environ
    base_env = {
        "SOAC_WORK_DIR": str(work_dir),
        "SOAC_COMPILE_MODE": environment.get("SOAC_COMPILE_MODE", "lazy"),
        "SOAC_BACKGROUND_JIT": environment.get("SOAC_BACKGROUND_JIT", "1"),
        # Empty is the documented no-explicit-log value, matching the previous
        # helper's removal of SOAC_LOG for its profile subprocess.
        "SOAC_LOG": "",
    }
    profile_result = _run_script(
        project, module_name, required_functions, base_env, opt_mode="profile"
    )
    assert profile_result.returncode == 0, profile_result.stdout + profile_result.stderr
    assert (work_dir / "profile.bin").exists()

    apply_env = {
        **base_env,
        "SOAC_LOG": f"soac_jit_direct_edges=info;json={log_path}",
    }
    apply_result = _run_script(
        project, module_name, required_functions, apply_env, opt_mode="apply"
    )
    assert apply_result.returncode == 0, apply_result.stdout + apply_result.stderr

    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    return json.loads(apply_result.stdout.strip()), rows


def test_apply_mode_direct_call_failure_preserves_exception(tmp_path: Path) -> None:
    result, rows = _run_apply_module(
        tmp_path,
        "direct_call_failure_exception",
        """
        class Marker(Exception):
            pass

        def fail():
            raise Marker("boom")

        def run():
            try:
                fail()
            except Marker as exc:
                return [type(exc).__name__, str(exc), exc.__context__ is None]
        """,
        required_functions=('fail', 'run'),
    )

    direct_edge_rows = [
        row
        for row in rows
        if row.get("target") == "soac_jit_direct_edges" and row.get("clif_direct_edges", 0) > 0
    ]
    assert direct_edge_rows, rows
    assert result == ["Marker", "boom", True]


def test_apply_mode_constructor_failure_preserves_exception(
    tmp_path: Path,
) -> None:
    with stock_module(
        tmp_path / "ordinary",
        "ordinary_constructor_failure",
        textwrap.dedent(_BROKEN_SOURCE),
    ) as module:
        assert module.run() == ["Marker", "boom:7", True]

    result, _rows = _run_apply_module(
        tmp_path,
        "direct_constructor_failure_exception",
        _BROKEN_SOURCE,
        required_functions=('Broken.__init__', 'run'),
    )

    # The actual type keeps ordinary constructor dispatch. Its authenticated
    # initializer and caller still compile and must preserve the exception.
    summary_path = tmp_path / "soac-work" / "jit-code-summary.jsonl"
    compiled_functions = {
        row.get("function_qualname")
        for line in summary_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
        if (row := json.loads(line)).get("entry_kind") == "direct_function_body"
    }
    assert {"run", "Broken.__init__"} <= compiled_functions, compiled_functions
    assert result == ["Marker", "boom:7", True]


def test_apply_mode_runtime_unpack_path_preserves_exception_cleanup(
    tmp_path: Path,
) -> None:
    result, rows = _run_apply_module(
        tmp_path,
        "runtime_unpack_batched_direct_entry",
        """
        def run():
            iterator = iter((1, 2))
            result = []
            spec = (True, True)
            for _, flag in enumerate(spec):
                if flag:
                    try:
                        result.append(next(iterator))
                    except StopIteration:
                        raise ValueError
                else:
                    break
            try:
                next(iterator)
            except StopIteration:
                return result
            raise ValueError
        """,
        required_functions=('run',),
    )

    summary_path = tmp_path / "soac-work" / "jit-code-summary.jsonl"
    compiled_run_bodies = [
        row
        for line in summary_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
        if (row := json.loads(line)).get("entry_kind") == "direct_function_body"
        and row.get("function_qualname") == "run"
    ]
    assert compiled_run_bodies, rows
    assert result == [1, 2]


def test_custom_new_only_field_preserves_ordinary_behavior(tmp_path: Path) -> None:
    with stock_module(
        tmp_path / "ordinary",
        "ordinary_custom_new",
        textwrap.dedent(_CUSTOM_NEW_SOURCE),
    ) as module:
        assert module.run() == [["new:5", "init:5:6"], 5, 6]


def test_custom_new_only_field_reports_checker_attribute_limit(tmp_path: Path) -> None:
    # This is the checker's constructor-local attribute-discovery limit, not a
    # prohibition on custom allocation. Selected custom __new__ callback/value
    # coverage with normal self-field discovery lives in
    # test_import_time_constructor_registration.py.
    errors = assert_strict_source_rejected(
        tmp_path / "strict-rejection",
        "from __future__ import strict\n" + textwrap.dedent(_CUSTOM_NEW_SOURCE),
        module_name="direct_constructor_custom_new_fallback",
        diagnostic="unresolved-attribute",
    )
    for message in (
        "Unresolved attribute `from_new` on type `Self@__new__`",
        "Object of type `Self@__init__` has no attribute `from_new`",
        "Object of type `Custom` has no attribute `from_new`",
    ):
        assert message in errors, errors


def test_apply_mode_direct_call_miss_uses_generic_fallback(tmp_path: Path) -> None:
    result, rows = _run_apply_module(
        tmp_path,
        "direct_call_small_fallback",
        """
        import os

        def profiled_target(left, right):
            return left + right

        def fallback_target(left, right):
            return left * right

        def run():
            target = profiled_target
            if os.environ.get("SOAC_OPT_MODE") == "apply":
                target = fallback_target
            return target(6, 7)
        """,
        required_functions=('profiled_target', 'fallback_target', 'run'),
    )

    generic_fallback_rows = [
        row
        for row in rows
        if row.get("target") == "soac_jit_direct_edges"
        and row.get("generic_fallback_edges", 0) > 0
    ]
    assert generic_fallback_rows, rows
    assert result == 42
