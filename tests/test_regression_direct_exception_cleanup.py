from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path


def _run_script(module_name: str, module_dir: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    script = textwrap.dedent(
        f"""
        import json
        import sys

        sys.path.insert(0, {str(module_dir)!r})
        from soac.import_hook import install

        install()
        import {module_name} as module

        print(json.dumps(module.run()))
        """
    )
    return subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env=env,
        text=True,
    )


def _run_apply_module(tmp_path: Path, module_name: str, source: str) -> tuple[object, list[dict[str, object]]]:
    module_path = tmp_path / f"{module_name}.py"
    module_path.write_text(textwrap.dedent(source), encoding="utf-8")

    work_dir = tmp_path / "soac-work"
    log_path = work_dir / "apply-events.jsonl"

    base_env = os.environ.copy()
    base_env.pop("SOAC_LOG", None)
    base_env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(work_dir),
        }
    )

    profile_env = {**base_env, "SOAC_OPT_MODE": "profile"}
    profile_result = _run_script(module_name, tmp_path, profile_env)
    assert profile_result.returncode == 0, profile_result.stdout + profile_result.stderr
    assert (work_dir / "profile.bin").exists()

    apply_env = {
        **base_env,
        "SOAC_OPT_MODE": "apply",
        "SOAC_LOG": f"soac_jit_direct_edges=info;json={log_path}",
    }
    apply_result = _run_script(module_name, tmp_path, apply_env)
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
    )

    direct_edge_rows = [
        row
        for row in rows
        if row.get("target") == "soac_jit_direct_edges" and row.get("clif_direct_edges", 0) > 0
    ]
    assert direct_edge_rows, rows
    assert result == ["Marker", "boom", True]


def test_apply_mode_constructor_entry_failure_preserves_exception(
    tmp_path: Path,
) -> None:
    result, rows = _run_apply_module(
        tmp_path,
        "direct_constructor_failure_exception",
        """
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
        """,
    )

    direct_edge_rows = [
        row
        for row in rows
        if row.get("target") == "soac_jit_direct_edges" and row.get("clif_direct_edges", 0) > 0
    ]
    assert direct_edge_rows, rows
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


def test_apply_mode_constructor_entry_custom_new_stays_generic(
    tmp_path: Path,
) -> None:
    result, rows = _run_apply_module(
        tmp_path,
        "direct_constructor_custom_new_fallback",
        """
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
        """,
    )

    direct_edge_rows = [
        row
        for row in rows
        if row.get("target") == "soac_jit_direct_edges" and row.get("clif_direct_edges", 0) > 0
    ]
    assert not direct_edge_rows, rows
    assert result == [["new:5", "init:5:6"], 5, 6]


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
    )

    generic_fallback_rows = [
        row
        for row in rows
        if row.get("target") == "soac_jit_direct_edges"
        and row.get("generic_fallback_edges", 0) > 0
    ]
    assert generic_fallback_rows, rows
    assert result == 42
