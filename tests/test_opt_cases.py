from __future__ import annotations

import ast
import json
import os
import subprocess
from pathlib import Path
from typing import Any

import pytest

from tests._strict_integration import _VALIDATION_PRELUDE, create_strict_project

OPT_TESTS_DIR = Path(__file__).resolve().parent / "opt_tests"
VERIFY_DELIMITER = "# soac: verify"
COUNTER_DELIMITER = "# soac: verify-counters"


def _case_paths() -> list[Path]:
    cases: list[Path] = []
    for path in sorted(OPT_TESTS_DIR.glob("*.py")):
        try:
            source = path.read_text(encoding="utf-8")
            if VERIFY_DELIMITER in source and COUNTER_DELIMITER in source:
                cases.append(path)
        except OSError:
            continue
    return cases


def _split_opt_case(case_path: Path) -> tuple[str, list[dict[str, Any]]]:
    source = case_path.read_text(encoding="utf-8")
    if VERIFY_DELIMITER not in source:
        raise ValueError(f"missing opt-test verify delimiter in {case_path}")
    if COUNTER_DELIMITER not in source:
        raise ValueError(f"missing opt-test counter delimiter in {case_path}")
    raw_source, rest = source.split(VERIFY_DELIMITER, 1)
    raw_verify, raw_expectations = rest.split(COUNTER_DELIMITER, 1)
    expectations = ast.literal_eval(raw_expectations.strip())
    if not isinstance(expectations, list) or not all(
        isinstance(expectation, dict) for expectation in expectations
    ):
        raise TypeError(f"{case_path} expectations must be a list of dictionaries")
    for expectation in expectations:
        if "module" in expectation:
            raise ValueError(
                f"{case_path} counter expectations must not include module; "
                "the module is implied by the opt-test filename"
            )
    module_source = raw_source.rstrip() + "\n\n" + raw_verify.lstrip()
    return module_source.rstrip() + "\n", expectations


# Each source is reviewed explicitly. Counter rows alone cannot prove native
# admission; these witnesses name the actual functions used by the old cases.
_REQUIRED_FUNCTIONS = {
    "direct_call_v3": ("target", "caller", "exercise_direct_call"),
    "exact_list_item_get_set_v3": ("list_get_set", "exercise_exact_list_items"),
    "exact_tuple_item_get_v3": (
        "OverrideTuple.__getitem__", "CustomIndex.__index__", "tuple_get",
        "tuple_set", "exercise_exact_tuple_items",
    ),
    "global_indexed_get_set": ("set_get_global", "exercise_global_indexed"),
    "indexed_field_branch_compare": ("Record.__init__", "branch_fields", "exercise_branch_fields"),
    "indexed_field_get_set": ("Record.__init__", "read_fields", "write_fields", "exercise_indexed_fields"),
    "indexed_field_get_set_v3": ("Record.__init__", "read_fields", "write_fields", "exercise_indexed_fields"),
}


def _opt_environment(work_dir: Path) -> dict[str, str]:
    # The original helper removed SOAC_COMPILE_MODE (Lazy). Preserve that
    # choice instead of accepting StrictProject.run's eager test default.
    return {
        "SOAC_WORK_DIR": str(work_dir),
        "SOAC_COMPILE_MODE": "lazy",
        "SOAC_BACKGROUND_JIT": os.environ.get("SOAC_BACKGROUND_JIT", "1"),
    }


def _opt_project(root: Path, module_name: str, source: str, environment):
    with pytest.MonkeyPatch.context() as patch:
        for name in (
            "SOAC_MODULE_ENABLED", "SOAC_LOG", "SOAC_WORK_DIR",
            "SOAC_COMPILE_MODE", "SOAC_BACKGROUND_JIT", "SOAC_OPT_MODE",
        ):
            patch.delenv(name, raising=False)
        for name, value in environment.items():
            patch.setenv(name, value)
        patch.setenv("SOAC_OPT_MODE", "profile")
        return create_strict_project(
            root / "strict-publication",
            {f"{module_name}.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + source},
            modules={module_name: f"{module_name}.py"},
            backend="soac",
        )


def _assert_subprocess_ok(result: subprocess.CompletedProcess[str]) -> None:
    assert result.returncode == 0, result.stdout + result.stderr


def _run_script(project, module_name: str) -> str:
    return _VALIDATION_PRELUDE + f"""
import ctypes
import importlib
from soac import _soac_ext

module = importlib.import_module({module_name!r})
diagnostic = _soac_ext.strict_module_diagnostics(module)
assert diagnostic is not None, 'opt case ran without strict ownership'
assert diagnostic['sealed'] is True
assert diagnostic['module_name'] == {module_name!r}
assert diagnostic['source_path'] == {str(project.project / (module_name + '.py'))!r}
assert diagnostic['artifact_generation'] == {project.publication['generation']!r}
assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = (ctypes.py_object,)
owner.restype = ctypes.c_void_p
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = (ctypes.py_object,)
metadata.restype = ctypes.c_void_p
for name in {_REQUIRED_FUNCTIONS[module_name]!r}:
    function = _plain_function_witness(module, name)
    assert owner(function), name
    assert metadata(function), name
del function
verify = getattr(module, '_soac_opt_verify', None)
if callable(verify):
    verify()
assert _soac_ext.strict_module_diagnostics(module) == diagnostic
"""


def _inspect_counter_dump_json(path: Path) -> dict[str, Any]:
    import _soac_ext

    return json.loads(_soac_ext.inspect_counter_dump_json(str(path)))


def _counter_value(
    verify: dict[str, Any], expectation: dict[str, Any], *, module_name: str
) -> int:
    function = expectation.get("function")
    kind = expectation.get("kind")
    branch = expectation.get("branch")
    instr_id = expectation.get("instr_id")
    observed_value = expectation.get("observed_value")
    if kind is None:
        raise ValueError(f"counter expectation is missing kind: {expectation!r}")
    if observed_value not in (None, "present") and not isinstance(observed_value, int):
        raise ValueError(
            "counter expectation observed_value must be an integer or 'present': "
            f"{expectation!r}"
        )

    total = 0
    for record in verify["records"]:
        if record["module_name"] != module_name:
            continue
        for row in record["rows"]:
            if row["kind"] != kind:
                continue
            if function is not None and row["function_qualname"] != function:
                continue
            if instr_id is not None and row["instr_id"] != instr_id:
                continue
            if observed_value == "present" and row["observed_value"] is None:
                continue
            if isinstance(observed_value, int) and row["observed_value"] != observed_value:
                continue
            if branch is None:
                total += row["value"]
            else:
                total += row["branches"].get(branch, 0)
    return total


def _assert_counter_expectation(
    verify: dict[str, Any],
    expectation: dict[str, Any],
    case_path: Path,
    *,
    module_name: str,
) -> None:
    value = _counter_value(verify, expectation, module_name=module_name)
    label = {
        key: expectation[key]
        for key in ("function", "kind", "branch", "instr_id", "observed_value")
        if key in expectation
    }
    if "equals" in expectation:
        assert value == expectation["equals"], (case_path, label, value, verify)
    if "min" in expectation:
        assert value >= expectation["min"], (case_path, label, value, verify)
    if "max" in expectation:
        assert value <= expectation["max"], (case_path, label, value, verify)
    if not {"equals", "min", "max"} & expectation.keys():
        raise ValueError(f"counter expectation has no comparator: {expectation!r}")


@pytest.mark.integration
@pytest.mark.parametrize("case_path", _case_paths(), ids=lambda path: path.stem)
def test_opt_case_verify_counters(tmp_path: Path, case_path: Path) -> None:
    source, expectations = _split_opt_case(case_path)
    module_name = case_path.stem
    assert module_name in _REQUIRED_FUNCTIONS, "opt case needs explicit admission review"
    work_dir = tmp_path / "soac-work"
    environment = _opt_environment(work_dir)
    project = _opt_project(tmp_path, module_name, source, environment)
    script = _run_script(project, module_name)

    profile_result = project.run(
        script, opt_mode="profile", extra_env=environment, check=False,
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    verify_result = project.run(
        script, opt_mode="verify", extra_env=environment, check=False,
    )
    _assert_subprocess_ok(verify_result)
    verify_path = work_dir / "verify.bin"
    assert verify_path.exists()
    verify = _inspect_counter_dump_json(verify_path)

    for expectation in expectations:
        _assert_counter_expectation(
            verify, expectation, case_path, module_name=module_name
        )


@pytest.mark.integration
def test_opt_case_driver_uses_canonical_witnesses_under_isolation(
    tmp_path: Path,
) -> None:
    # Keep this focused on the real isolated driver, before counter inspection.
    # These are the existing direct_call_v3 witness names and callable shapes.
    source = """
def target(left, right):
    return left + right


def caller(fn, left, right):
    return fn(left, right)


def exercise_direct_call():
    assert caller(target, 20, 22) == 42
"""
    module_name = "direct_call_v3"
    environment = _opt_environment(tmp_path / "soac-work")
    project = _opt_project(tmp_path, module_name, source, environment)
    helper_path = Path(__file__).with_name("_strict_integration.py").resolve()
    script = (
        "assert sys.flags.isolated == 1\n"
        f"assert {str(OPT_TESTS_DIR.parents[1])!r} not in sys.path\n"
        + _run_script(project, module_name)
        + f"""
from pathlib import Path
assert Path(_plain_function_witness.__code__.co_filename).resolve() == Path({str(helper_path)!r})
assert _plain_function_witness(module, 'caller') is module.caller
assert module.exercise_direct_call() is None
"""
    )
    result = project.run(
        script, opt_mode="profile", extra_env=environment, check=False,
    )
    _assert_subprocess_ok(result)
