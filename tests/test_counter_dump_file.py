from __future__ import annotations

import gc
import json
import os
import subprocess
import sys

from tests._integration import integration_module


def _inspect_counter_dump_json(path):
    import _soac_ext

    return json.loads(_soac_ext.inspect_counter_dump_json(str(path)))


def _read_jsonl(path):
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


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

    with integration_module(tmp_path, "counter_dump_file_case", source, mode="soac") as module:
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


def test_counter_dump_file_is_not_written_in_none_mode(tmp_path, monkeypatch):
    work_dir = tmp_path / "soac-work"
    monkeypatch.setenv("SOAC_WORK_DIR", str(work_dir))
    monkeypatch.setenv("SOAC_OPT_MODE", "none")

    source = """
VALUE = 7

def read():
    return VALUE
"""

    with integration_module(
        tmp_path, "counter_dump_none_mode_case", source, mode="soac"
    ) as module:
        assert module.read() == 7
        assert module.read() == 7

    gc.collect()

    assert not (work_dir / "profile.bin").exists()
    assert not (work_dir / "verify.bin").exists()


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
    for name in [
        "jit_codegen_total_us",
        "jit_clif_block_count",
        "jit_clif_inst_count",
        "jit_machine_code_size_bytes",
        "jit_machine_code_block_count",
        "jit_machine_code_edge_count",
    ]:
        assert jit_row[name] >= 0
        assert isinstance(jit_row[name], int)
    assert jit_row["jit_clif_block_count"] > 0
    assert jit_row["jit_clif_inst_count"] > 0
    assert jit_row["jit_machine_code_size_bytes"] > 0
    assert jit_row["jit_machine_code_block_count"] > 0


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

    rows = _read_jsonl(log_path)
    assert any(
        row.get("event") == "soac.module_load"
        and row["module_name"].endswith("work_dir_log_case")
        for row in rows
    )


def test_apply_mode_specialization_runtime_logs_indexed_hits(tmp_path):
    log_path = tmp_path / "apply-events.jsonl"
    module_path = tmp_path / "specialization_runtime_case.py"
    module_path.write_text(
        """
VALUE = 9

class Point:
    pass

def run():
    point = Point()
    point.x = 33
    return point.x + VALUE
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    base_env = {
        **os.environ,
        "SOAC_WORK_DIR": str(work_dir),
    }
    base_env.pop("SOAC_LOG", None)

    script = f"""
import sys
sys.path.insert(0, {str(tmp_path)!r})
from soac.import_hook import install
install()
import specialization_runtime_case as module
for _ in range(20):
    assert module.run() == 42
"""

    subprocess.run(
        [sys.executable, "-c", script],
        check=True,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert (work_dir / "profile.bin").exists()

    subprocess.run(
        [sys.executable, "-c", script],
        check=True,
        env={
            **base_env,
            "SOAC_OPT_MODE": "apply",
            "SOAC_LOG": f"soac_specialization_runtime=info;json={log_path}",
        },
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    rows = _read_jsonl(log_path)
    runtime_rows = [
        row
        for row in rows
        if row.get("event") == "soac.specialization_runtime"
        and row["module_name"].endswith("specialization_runtime_case")
    ]
    assert any(
        row["kind"] == "global_indexed_hit"
        and row["function_qualname"] == "run"
        and row["value"] > 0
        for row in runtime_rows
    ), runtime_rows
    assert any(
        row["kind"] == "field_indexed_hit"
        and row["function_qualname"] == "run"
        and row["value"] > 0
        for row in runtime_rows
    ), runtime_rows
    assert not any(
        row["kind"] in {"global_indexed_fallback", "field_indexed_fallback"}
        and row["value"] > 0
        for row in runtime_rows
    ), runtime_rows


def test_apply_mode_default_event_log_includes_specialization_runtime(tmp_path):
    module_path = tmp_path / "specialization_runtime_default_case.py"
    module_path.write_text(
        """
VALUE = 9

class Point:
    pass

def run():
    point = Point()
    point.x = 33
    return point.x + VALUE
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    log_path = work_dir / "events.jsonl"
    base_env = {
        **os.environ,
        "SOAC_WORK_DIR": str(work_dir),
    }
    base_env.pop("SOAC_LOG", None)

    script = f"""
import sys
sys.path.insert(0, {str(tmp_path)!r})
from soac.import_hook import install
install()
import specialization_runtime_default_case as module
for _ in range(20):
    assert module.run() == 42
"""

    subprocess.run(
        [sys.executable, "-c", script],
        check=True,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert (work_dir / "profile.bin").exists()

    subprocess.run(
        [sys.executable, "-c", script],
        check=True,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    rows = _read_jsonl(log_path)
    runtime_rows = [
        row
        for row in rows
        if row.get("event") == "soac.specialization_runtime"
        and row["module_name"].endswith("specialization_runtime_default_case")
    ]
    assert any(
        row["kind"] == "global_indexed_hit"
        and row["function_qualname"] == "run"
        and row["value"] > 0
        for row in runtime_rows
    ), runtime_rows
    assert any(
        row["kind"] == "field_indexed_hit"
        and row["function_qualname"] == "run"
        and row["value"] > 0
        for row in runtime_rows
    ), runtime_rows
    assert not any(
        row["kind"] in {"global_indexed_fallback", "field_indexed_fallback"}
        and row["value"] > 0
        for row in runtime_rows
    ), runtime_rows


def test_cross_module_field_profile_uses_type_id_table(tmp_path):
    owner_path = tmp_path / "field_owner_case.py"
    owner_path.write_text(
        """
class Point:
    pass

class Vector:
    pass
""",
        encoding="utf-8",
    )
    user_path = tmp_path / "field_user_case.py"
    user_path.write_text(
        """
from field_owner_case import Point, Vector

def read_fields():
    point = Point()
    vector = Vector()
    point.x = 40
    point.y = 2
    vector.x = 30
    vector.y = 12
    return point.x + point.y + vector.x + vector.y

def write_field():
    point = Point()
    vector = Vector()
    point.x = 1
    point.y = 2
    point.x = 40
    vector.x = 1
    vector.y = 2
    vector.x = 30
    vector.y = 12
    return point.x + point.y + vector.x + vector.y
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"

    script = f"""
import sys
sys.path.insert(0, {str(tmp_path)!r})
from soac.import_hook import install
install()
import field_user_case
for _ in range(20):
    assert field_user_case.read_fields() == 84
    assert field_user_case.write_field() == 84
"""
    base_env = os.environ.copy()
    base_env.pop("SOAC_LOG", None)
    base_env.pop("SOAC_MODULE_ENABLED", None)
    base_env.update(
        {
            "DIET_PYTHON_MODE": "transform",
            "SOAC_WORK_DIR": str(work_dir),
        }
    )

    profile_result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        text=True,
    )
    assert profile_result.returncode == 0, profile_result.stdout + profile_result.stderr
    profile_dump_path = work_dir / "profile.bin"
    assert profile_dump_path.exists()

    profile = _inspect_counter_dump_json(profile_dump_path)
    owner_entries = [
        entry
        for record in profile["records"]
        for entry in record["type_table"]
        if entry["module_name"] == "field_owner_case"
        and entry["qualname"] in {"Point", "Vector"}
    ]
    assert {entry["qualname"] for entry in owner_entries} == {"Point", "Vector"}
    owner_type_ids = {entry["type_id"] for entry in owner_entries}
    owner_type_keys = [
        key
        for record in profile["records"]
        for key in record["type_keys"]
        if key["owner_type_id"] in owner_type_ids
    ]
    keys_by_owner = {
        entry["qualname"]: {
            key["key"]
            for key in owner_type_keys
            if key["owner_type_id"] == entry["type_id"]
        }
        for entry in owner_entries
    }
    assert keys_by_owner == {"Point": {"x", "y"}, "Vector": {"x", "y"}}

    verify_result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
        text=True,
    )
    assert verify_result.returncode == 0, verify_result.stdout + verify_result.stderr
    verify_dump_path = work_dir / "verify.bin"
    assert verify_dump_path.exists()

    verify = _inspect_counter_dump_json(verify_dump_path)
    field_hits = [
        row
        for record in verify["records"]
        if record["module_name"] == "field_user_case"
        for row in record["rows"]
        if row["kind"] == "field_indexed_hit" and row["value"] > 0
    ]
    assert field_hits, verify
    hit_counts_by_function = {}
    for row in field_hits:
        hit_counts_by_function[row["function_qualname"]] = (
            hit_counts_by_function.get(row["function_qualname"], 0) + 1
        )
    assert hit_counts_by_function == {"read_fields": 4, "write_field": 4}, verify
