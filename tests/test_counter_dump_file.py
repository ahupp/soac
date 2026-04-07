from __future__ import annotations

import gc

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
