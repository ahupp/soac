from __future__ import annotations

import json
import os
import subprocess
import sys

import pytest

from tests._integration import decide_optimizations_for_work_dir, soac_module


def test_vectorcall_wrong_arity_is_checked_before_direct_entry(tmp_path):
    source = """
def add(a, b):
    return a + b

def missing():
    return add(1)

def extra():
    return add(1, 2, 3)
"""

    with soac_module(tmp_path, "direct_entry_wrong_arity_case", source) as module:
        with pytest.raises(TypeError, match="add\\(\\) missing required argument 'b'"):
            module.missing()
        with pytest.raises(
            TypeError,
            match="add\\(\\) takes 2 positional arguments but 3 were given",
        ):
            module.extra()


def _run_soac_module(tmp_path, module_name: str, env: dict[str, str]) -> None:
    script = f"""
import sys
sys.path.insert(0, {str(tmp_path)!r})
from soac.import_hook import install
install()
import {module_name} as module
assert module.run() == 42
"""
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env=env,
        text=True,
    )
    assert result.returncode == 0, result.stdout + result.stderr


def test_apply_mode_direct_call_with_omitted_default_emits_direct_edge(tmp_path):
    module_name = "direct_call_omitted_default_case"
    module_path = tmp_path / f"{module_name}.py"
    module_path.write_text(
        """
def callee(value, increment=5):
    return value + increment

def run():
    return callee(37)
""",
        encoding="utf-8",
    )
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
    _run_soac_module(tmp_path, module_name, profile_env)
    assert (work_dir / "profile.bin").exists()
    assert decide_optimizations_for_work_dir(work_dir) >= 1

    apply_env = {
        **base_env,
        "SOAC_OPT_MODE": "apply",
        "SOAC_LOG": f"soac_jit_direct_edges=info;json={log_path}",
    }
    _run_soac_module(tmp_path, module_name, apply_env)

    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    direct_edge_rows = [
        row
        for row in rows
        if row.get("target") == "soac_jit_direct_edges"
        and row.get("clif_direct_edges", 0) > 0
    ]
    assert direct_edge_rows, rows
