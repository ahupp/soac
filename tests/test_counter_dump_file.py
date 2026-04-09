from __future__ import annotations

import gc
import json
import os
import subprocess
import sys

from tests._integration import integration_module


def test_counter_dump_file_is_written_on_module_exit(tmp_path, monkeypatch):
    work_dir = tmp_path / "soac-work"
    dump_path = work_dir / "profile.bin"
    monkeypatch.setenv("SOAC_WORK_DIR", str(work_dir))
    monkeypatch.setenv("SOAC_OPT_MODE", "profile")

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


def test_module_load_event_is_written_to_soac_log_json(tmp_path):
    log_path = tmp_path / "soac-events.jsonl"
    module_path = tmp_path / "module_load_log_case.py"
    source = """
VALUE = 5

def read():
    return VALUE
"""
    module_path.write_text(source, encoding="utf-8")
    env = {
        **os.environ,
        "DIET_PYTHON_ALLOW_TEMP": "1",
        "DIET_PYTHON_INTEGRATION_ONLY": "0",
        "DIET_PYTHON_MODE": "transform",
        "SOAC_LOG": f"soac_module_load=info,soac_jit_codegen=info;json={log_path}",
    }

    subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                f"sys.path.insert(0, {str(tmp_path)!r}); "
                "from soac.import_hook import install; "
                "install(); "
                "import module_load_log_case; "
                "assert module_load_log_case.read() == 5"
            ),
        ],
        check=True,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    row = next(
        row
        for row in rows
        if row.get("event") == "soac.module_load"
        and row["module_name"].endswith("module_load_log_case")
    )
    phases = {
        row["phase"]: row
        for row in rows
        if row.get("event") == "soac.module_load.phase"
        and row["module_name"].endswith("module_load_log_case")
    }

    assert row["status"] == "ok"
    assert row["error"] == ""
    assert row["path"].endswith("module_load_log_case.py")
    assert row["source_hash"].startswith("0x")
    assert int(row["source_hash"], 16) > 0
    assert row["function_count"] >= 2

    for name in [
        "module_load_total_us",
        "create_module_total_us",
        "blockpy_total_us",
        "exec_module_total_us",
    ]:
        assert row[name] >= 0
        assert isinstance(row[name], int)
    for phase in [
        "create_module.source_read",
        "create_module.lower_blockpy",
        "blockpy.parse",
        "blockpy.bb_codegen",
        "exec_module.call_module_init",
        "exec_module.register_function_owner_types",
    ]:
        assert phases[phase]["elapsed_us"] >= 0
        assert isinstance(phases[phase]["elapsed_us"], int)

    jit_row = next(
        row
        for row in rows
        if row.get("event") == "soac.jit_codegen"
        and row["module_name"].endswith("module_load_log_case")
        and row["function_qualname"] == "read"
    )
    assert jit_row["status"] == "ok"
    assert jit_row["error"] == ""
    assert jit_row["function_entry_kind"] == "direct_function_body"
    assert jit_row["jit_codegen_total_us"] >= 0
    assert isinstance(jit_row["jit_codegen_total_us"], int)


def test_soac_work_dir_is_default_event_log_dir(tmp_path):
    work_dir = tmp_path / "soac-work"
    log_path = work_dir / "events.jsonl"
    module_path = tmp_path / "work_dir_log_case.py"
    module_path.write_text("def read():\n    return 11\n", encoding="utf-8")
    env = {
        **os.environ,
        "SOAC_WORK_DIR": str(work_dir),
    }
    env.pop("SOAC_LOG", None)

    subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                f"sys.path.insert(0, {str(tmp_path)!r}); "
                "from soac.import_hook import install; "
                "install(); "
                "import work_dir_log_case; "
                "assert work_dir_log_case.read() == 11"
            ),
        ],
        check=True,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    assert any(
        row.get("event") == "soac.module_load"
        and row["module_name"].endswith("work_dir_log_case")
        for row in rows
    )
