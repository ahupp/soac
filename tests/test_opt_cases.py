from __future__ import annotations

import ast
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from tests._integration import decide_optimizations_for_work_dir

OPT_TESTS_DIR = Path(__file__).resolve().parent / "opt_tests"
PLAN_MODE_RE = re.compile(r"^# soac: opt-plan-mode=(legacy|v3)\s*$", re.MULTILINE)
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


def _split_opt_case(case_path: Path) -> tuple[str, str, list[dict[str, Any]]]:
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
    plan_mode_matches = PLAN_MODE_RE.findall(raw_source)
    if len(plan_mode_matches) > 1:
        raise ValueError(f"{case_path} declares multiple opt-test plan modes")
    plan_mode = plan_mode_matches[0] if plan_mode_matches else "legacy"
    module_source = raw_source.rstrip() + "\n\n" + raw_verify.lstrip()
    return module_source.rstrip() + "\n", plan_mode, expectations


def _soac_subprocess_env(
    module_root: Path, *, work_dir: Path, plan_mode: str
) -> dict[str, str]:
    env = dict(os.environ)
    env["SOAC_MODULE_ENABLED"] = f"path:{module_root}"
    env["SOAC_WORK_DIR"] = str(work_dir)
    env["SOAC_OPT_PLAN_MODE"] = plan_mode
    env.pop("SOAC_LOG", None)
    env.pop("SOAC_COMPILE_MODE", None)
    return env


def _run_soac_subprocess(script: str, *, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env=env,
        text=True,
    )


def _assert_subprocess_ok(result: subprocess.CompletedProcess[str]) -> None:
    assert result.returncode == 0, result.stdout + result.stderr


def _run_script(module_root: Path, module_name: str) -> str:
    return "\n".join(
        [
            "import sys",
            f"sys.path.insert(0, {str(module_root)!r})",
            "from soac.import_hook import install",
            "install()",
            f"import {module_name}",
            "",
        ]
    )


def _inspect_counter_dump_json(path: Path) -> dict[str, Any]:
    import _soac_ext

    return json.loads(_soac_ext.inspect_counter_dump_json(str(path)))


def _inspect_v3_plan_json(path: Path) -> dict[str, Any]:
    import _soac_ext

    return json.loads(_soac_ext.inspect_optimization_artifacts_v3_json(str(path)))


def _counter_value(
    verify: dict[str, Any], expectation: dict[str, Any], *, module_name: str
) -> int:
    function = expectation.get("function")
    kind = expectation.get("kind")
    instr_id = expectation.get("instr_id")
    if kind is None:
        raise ValueError(f"counter expectation is missing kind: {expectation!r}")

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
            total += row["value"]
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
        for key in ("function", "kind", "instr_id")
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


def _assert_metric_expectation(
    actual: int, expected: Any, case_path: Path, label: dict[str, Any]
) -> None:
    if isinstance(expected, int):
        assert actual == expected, (case_path, label, actual)
        return
    if not isinstance(expected, dict):
        raise TypeError(f"metric expectation must be an int or dictionary: {expected!r}")
    if "equals" in expected:
        assert actual == expected["equals"], (case_path, label, actual)
    if "min" in expected:
        assert actual >= expected["min"], (case_path, label, actual)
    if "max" in expected:
        assert actual <= expected["max"], (case_path, label, actual)
    if not {"equals", "min", "max"} & expected.keys():
        raise ValueError(f"metric expectation has no comparator: {expected!r}")


def _v3_plan_for_module(work_dir: Path, module_name: str) -> dict[str, Any]:
    plans = [_inspect_v3_plan_json(path) for path in (work_dir / "modules").rglob("mod.optv3")]
    matches = [
        plan for plan in plans if plan["module"]["module_name"] == module_name
    ]
    assert len(matches) == 1, (module_name, plans)
    return matches[0]


def _assert_v3_plan_expectation(
    plan: dict[str, Any],
    expectation: dict[str, Any],
    case_path: Path,
) -> None:
    function_name = expectation.get("function")
    if function_name is None:
        raise ValueError(f"v3 plan expectation is missing function: {expectation!r}")
    functions = [
        function
        for function in plan["functions"]
        if function["debug_name"] == function_name
    ]
    assert len(functions) == 1, (case_path, function_name, plan)
    function = functions[0]
    for metric in (
        "regions",
        "emitted_regions",
        "scalar_threads",
        "direct_calls",
        "emitted_direct_calls",
        "method_calls",
        "emitted_method_calls",
        "exact_list_items",
        "emitted_exact_list_items",
        "indexed_fields",
        "emitted_indexed_fields",
        "indexed_globals",
        "emitted_indexed_globals",
        "diagnostics",
    ):
        if metric in expectation:
            _assert_metric_expectation(
                function[metric],
                expectation[metric],
                case_path,
                {"function": function_name, "metric": metric},
            )


@pytest.mark.integration
@pytest.mark.parametrize("case_path", _case_paths(), ids=lambda path: path.stem)
def test_opt_case_verify_counters(tmp_path: Path, case_path: Path) -> None:
    source, plan_mode, expectations = _split_opt_case(case_path)
    module_name = case_path.stem
    module_root = tmp_path / "modules"
    module_root.mkdir()
    (module_root / f"{module_name}.py").write_text(source, encoding="utf-8")

    work_dir = tmp_path / "soac-work"
    base_env = _soac_subprocess_env(
        module_root, work_dir=work_dir, plan_mode=plan_mode
    )
    script = _run_script(module_root, module_name)

    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()
    assert decide_optimizations_for_work_dir(work_dir, mode=plan_mode) >= 1
    plan_suffix = ".optv3" if plan_mode == "v3" else ".opt"
    assert list((work_dir / "modules").rglob(f"*{plan_suffix}"))
    v3_plan = _v3_plan_for_module(work_dir, module_name) if plan_mode == "v3" else None

    verify_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
    )
    _assert_subprocess_ok(verify_result)
    verify_path = work_dir / "verify.bin"
    assert verify_path.exists()
    verify = _inspect_counter_dump_json(verify_path)

    for expectation in expectations:
        if expectation.get("type") == "v3_plan":
            if v3_plan is None:
                raise ValueError(f"{case_path} has v3 plan expectation in {plan_mode} mode")
            _assert_v3_plan_expectation(v3_plan, expectation, case_path)
        else:
            _assert_counter_expectation(
                verify, expectation, case_path, module_name=module_name
            )
