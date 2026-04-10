from __future__ import annotations

import json
import os
import subprocess
import sys


def _run_soac_module(tmp_path, module_name: str, env: dict[str, str]) -> None:
    script = f"""
import sys
sys.path.insert(0, {str(tmp_path)!r})
from soac.import_hook import install
install()
import {module_name} as module
for _ in range(20):
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
    base_env.pop("SOAC_MODULE_ENABLED", None)
    base_env.update(
        {
            "DIET_PYTHON_ALLOW_TEMP": "1",
            "DIET_PYTHON_INTEGRATION_ONLY": "0",
            "DIET_PYTHON_MODE": "transform",
            "SOAC_WORK_DIR": str(work_dir),
        }
    )

    profile_env = {**base_env, "SOAC_OPT_MODE": "profile"}
    _run_soac_module(tmp_path, module_name, profile_env)
    assert (work_dir / "profile.bin").exists()

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
