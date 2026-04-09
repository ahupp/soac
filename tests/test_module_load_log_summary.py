from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


def load_summary_script():
    path = REPO_ROOT / "scripts" / "summarize_module_load_log.py"
    spec = importlib.util.spec_from_file_location("summarize_module_load_log_for_test", path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_jsonl(path: Path, entries):
    path.write_text(
        "".join(json.dumps(entry) + "\n" for entry in entries),
        encoding="utf-8",
    )


def test_summarize_log_reports_load_phase_and_jit_codegen_max(tmp_path, capsys):
    summary_script = load_summary_script()
    log_path = tmp_path / "module_loads.jsonl"
    write_jsonl(
        log_path,
        [
            {
                "event": "soac.module_load",
                "status": "ok",
                "module": {"module_name": "fast"},
                "timings_ms": {
                    "module_load_total": 10.0,
                    "blockpy.parse": 1.0,
                    "blockpy.name_binding": 4.0,
                },
            },
            {
                "event": "soac.module_load",
                "status": "ok",
                "module": {"module_name": "slow"},
                "timings_ms": {
                    "module_load_total": 30.0,
                    "blockpy.parse": 3.0,
                    "blockpy.name_binding": 8.0,
                },
            },
            {
                "event": "soac.jit_codegen",
                "status": "ok",
                "module": {"module_name": "fast"},
                "function": {
                    "qualname": "small",
                    "entry_kind": "vectorcall_function_body",
                },
                "timings_ms": {"jit_codegen_total": 2.0},
            },
            {
                "event": "soac.jit_codegen",
                "status": "ok",
                "module": {"module_name": "slow"},
                "function": {
                    "qualname": "expensive",
                    "entry_kind": "direct_function_body",
                },
                "timings_ms": {"jit_codegen_total": 7.0},
            },
        ],
    )

    summary = summary_script.summarize_log(log_path)
    assert summary.cumulative_module_load_ms == 40.0
    assert summary.module_timing_stats["blockpy.parse"].median_ms == 2.0
    assert summary.module_timing_stats["blockpy.name_binding"].max_ms == 8.0
    assert summary.module_timing_stats["blockpy.name_binding"].max_owner == "slow"
    assert summary.cumulative_jit_codegen_ms == 9.0
    assert summary.max_jit_codegen.qualname == "expensive"

    summary_script.print_summary(summary)
    out = capsys.readouterr().out
    assert "cumulative module_load_total:" in out
    assert "blockpy.name_binding" in out
    assert "slow.expensive" in out
