from __future__ import annotations

import gc
import json

from tests._integration import integration_module


def test_counter_dump_file_is_written_on_module_exit(tmp_path, monkeypatch):
    dump_path = tmp_path / "counters.bin"
    monkeypatch.setenv("DIET_PYTHON_GLOBAL_LOAD_COUNTERS", "1")
    monkeypatch.setenv("DIET_PYTHON_COUNTERS_OUTPUT_FILE", str(dump_path))

    source = """
VALUE = 7

def read():
    return VALUE
"""

    with integration_module(tmp_path, "counter_dump_file_case", source, mode="transform") as module:
        assert module.read() == 7
        assert module.read() == 7

    gc.collect()

    data = dump_path.read_bytes()
    assert data.startswith(b"SOACRKV1")
    assert int.from_bytes(data[8:10], "little") == 1
    header_len = int.from_bytes(data[10:12], "little")
    payload_len = int.from_bytes(data[16:24], "little")
    assert header_len == 32
    assert payload_len > 0
    assert header_len + payload_len <= len(data)
    assert len(data) > 64


def test_module_load_log_is_written_next_to_counter_dir(tmp_path, monkeypatch):
    counters_dir = tmp_path / "counters"
    monkeypatch.setenv("DIET_PYTHON_COUNTERS_DIR", str(counters_dir))

    source = """
VALUE = 5

def read():
    return VALUE
"""

    with integration_module(tmp_path, "module_load_log_case", source, mode="transform") as module:
        assert module.read() == 5

    log_path = counters_dir / "module_loads.jsonl"
    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    row = next(
        row
        for row in rows
        if row["module"]["module_name"].endswith(".module_load_log_case")
    )

    assert row["event"] == "soac.module_load"
    assert row["status"] == "ok"
    assert row["error"] is None
    assert row["module"]["path"].endswith("module_load_log_case.py")
    assert row["module"]["function_count"] >= 2

    timings = row["timings_ms"]
    for name in [
        "module_load_total",
        "create_module_total",
        "create_module.source_read",
        "create_module.lower_blockpy",
        "blockpy_total",
        "blockpy.parse",
        "blockpy.bb_codegen",
        "exec_module_total",
        "exec_module.call_module_init",
        "exec_module.register_function_owner_types",
    ]:
        assert timings[name] >= 0
