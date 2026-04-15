from __future__ import annotations

import importlib.util
import json
import statistics  # noqa: F401 - preload before SOAC import hooks installed by other tests
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
                "module_name": "fast",
                "module_load_total_us": 10_000,
                "create_module_total_us": 9_000,
                "blockpy_total_us": 8_000,
            },
            {
                "event": "soac.module_load",
                "status": "ok",
                "module_name": "slow",
                "module_load_total_us": 30_000,
                "create_module_total_us": 25_000,
                "blockpy_total_us": 20_000,
            },
            {
                "event": "soac.module_load.phase",
                "status": "ok",
                "module_name": "fast",
                "phase": "blockpy.parse",
                "elapsed_us": 1_000,
            },
            {
                "event": "soac.module_load.phase",
                "status": "ok",
                "module_name": "slow",
                "phase": "blockpy.parse",
                "elapsed_us": 3_000,
            },
            {
                "event": "soac.module_load.phase",
                "status": "ok",
                "module_name": "fast",
                "phase": "blockpy.name_binding",
                "elapsed_us": 4_000,
            },
            {
                "event": "soac.module_load.phase",
                "status": "ok",
                "module_name": "slow",
                "phase": "blockpy.name_binding",
                "elapsed_us": 8_000,
            },
            {
                "event": "soac.jit_codegen",
                "status": "ok",
                "module_name": "fast",
                "function_qualname": "small",
                "function_entry_kind": "vectorcall_function_body",
                "jit_codegen_total_us": 2_000,
                "function_block_count": 3,
                "jit_clif_block_count": 5,
                "jit_clif_inst_count": 40,
                "jit_machine_code_size_bytes": 128,
                "jit_machine_code_block_count": 4,
                "jit_machine_code_edge_count": 3,
            },
            {
                "event": "soac.jit_codegen",
                "status": "ok",
                "module_name": "soac.runtime",
                "function_qualname": "_dp_module_init",
                "function_entry_kind": "direct_function_body",
                "jit_codegen_total_us": 10_000,
                "function_block_count": 99,
                "jit_clif_block_count": 999,
                "jit_clif_inst_count": 9_999,
                "jit_machine_code_size_bytes": 99_999,
                "jit_machine_code_block_count": 888,
                "jit_machine_code_edge_count": 777,
            },
            {
                "event": "soac.jit_codegen",
                "status": "ok",
                "module_name": "slow",
                "function_qualname": "expensive",
                "function_entry_kind": "direct_function_body",
                "jit_codegen_total_us": 7_000,
                "function_block_count": 9,
                "jit_clif_block_count": 11,
                "jit_clif_inst_count": 90,
                "jit_machine_code_size_bytes": 456,
                "jit_machine_code_block_count": 8,
                "jit_machine_code_edge_count": 10,
            },
            {
                "event": "soac.jit_batch_codegen",
                "status": "ok",
                "module_name": "slow",
                "root_function_qualname": "_dp_module_init",
                "batch_function_count": 9,
                "functions_to_define_count": 7,
                "requested_worker_count": 4,
                "worker_module_count": 4,
                "worker_function_count_min": 1,
                "worker_function_count_max": 2,
                "jit_batch_collect_us": 100,
                "jit_batch_reservation_us": 1_000,
                "jit_batch_codegen_us": 20_000,
                "jit_batch_commit_us": 2_000,
                "jit_batch_total_us": 24_000,
                "jit_batch_worker_total_sum_us": 40_000,
                "jit_batch_worker_total_max_us": 12_000,
                "jit_batch_worker_module_build_sum_us": 7_000,
                "jit_batch_worker_module_build_max_us": 3_000,
                "jit_batch_worker_compile_sum_us": 30_000,
                "jit_batch_worker_compile_max_us": 9_000,
                "jit_batch_worker_validate_sum_us": 2_000,
                "jit_batch_worker_validate_max_us": 800,
            },
        ],
    )

    summary = summary_script.summarize_log(log_path)
    assert summary.cumulative_module_load_ms == 40.0
    assert summary.module_timing_stats["module_load_total"].cumulative_ms == 40.0
    assert summary.module_timing_stats["create_module_total"].cumulative_ms == 34.0
    assert summary.module_timing_stats["blockpy_total"].cumulative_ms == 28.0
    assert summary.module_timing_stats["blockpy.parse"].median_ms == 2.0
    assert summary.module_timing_stats["blockpy.name_binding"].max_ms == 8.0
    assert summary.module_timing_stats["blockpy.name_binding"].cumulative_ms == 12.0
    assert summary.module_timing_stats["blockpy.name_binding"].max_owner == "slow"
    assert summary.cumulative_jit_codegen_ms == 19.0
    assert summary.max_jit_codegen.qualname == "_dp_module_init"
    assert summary.jit_counter_stats["jit_machine_code_size_bytes"].total == 584
    assert summary.jit_counter_stats["jit_machine_code_size_bytes"].max == 456
    assert summary.jit_counter_stats["jit_machine_code_size_bytes"].max_owner == "slow.expensive (direct_function_body)"
    assert summary.jit_counter_stats["jit_machine_code_block_count"].total == 12
    assert summary.jit_counter_stats["function_block_count"].total == 12
    assert summary.jit_batch_event_count == 1
    assert summary.cumulative_jit_batch_codegen_ms == 24.0
    assert summary.jit_batch_timing_stats["jit_batch_worker_compile_sum"].max_ms == 30.0
    assert summary.jit_batch_timing_stats["jit_batch_worker_compile_sum"].max_owner == "slow._dp_module_init"

    summary_script.print_summary(summary)
    out = capsys.readouterr().out
    assert "cumulative module_load_total:" in out
    assert "blockpy.name_binding" in out
    assert "slow.expensive" in out
    assert "jit-codegen counters (excluding soac.* modules):" in out
    assert "jit_machine_code_size_bytes" in out
    assert "jit-batch-codegen events:" in out
    assert "jit_batch_worker_compile_sum" in out
