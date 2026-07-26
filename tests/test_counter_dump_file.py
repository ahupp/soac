from __future__ import annotations

import gc
import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest
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


def _soac_subprocess_env(module_root, *, work_dir=None, extra_env=None):
    env = dict(os.environ)
    env["SOAC_MODULE_ENABLED"] = f"path:{module_root}"
    env.pop("SOAC_COMPILE_MODE", None)
    if work_dir is not None:
        env["SOAC_WORK_DIR"] = str(work_dir)
    else:
        env.pop("SOAC_WORK_DIR", None)
    env.pop("SOAC_LOG", None)
    if extra_env:
        env.update(extra_env)
    return env


def _run_soac_subprocess(script, *, env, timeout=None):
    return subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env=env,
        text=True,
        timeout=timeout,
    )


def _assert_subprocess_ok(result):
    assert result.returncode == 0, result.stdout + result.stderr


def _counter_branch(row, branch):
    return row.get("branches", {}).get(branch, 0)


def _import_and_run_script(module_root, import_stmt, body):
    body_lines = textwrap.dedent(body).strip().splitlines()
    return "\n".join(
        [
            "import sys",
            f"sys.path.insert(0, {str(module_root)!r})",
            "from soac.import_hook import install",
            "install()",
            import_stmt,
            *body_lines,
            "",
        ]
    )


@pytest.fixture(scope="module")
def profiled_specialization_runtime_case(tmp_path_factory):
    base_dir = tmp_path_factory.mktemp("counter-dump-specialization-runtime")
    module_name = "specialization_runtime_case"
    (base_dir / f"{module_name}.py").write_text(
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
    work_dir = base_dir / "soac-work"
    script = _import_and_run_script(
        base_dir,
        f"import {module_name} as module",
        "assert module.run() == 42",
    )
    base_env = _soac_subprocess_env(base_dir, work_dir=work_dir)
    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()
    return {
        "base_dir": base_dir,
        "module_name": module_name,
        "script": script,
        "work_dir": work_dir,
        "base_env": base_env,
    }


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
    assert int.from_bytes(data[8:10], "little") > 0
    header_len = int.from_bytes(data[10:12], "little")
    payload_len = int.from_bytes(data[16:24], "little")
    assert header_len == 32
    assert payload_len > 0
    assert header_len + payload_len <= len(data)
    assert len(data) > 64
    dump = _inspect_counter_dump_json(dump_path)
    assert dump["records"]
    assert dump["records"][0]["source_hash"].startswith("0x")
    assert int(dump["records"][0]["source_hash"], 16) > 0


def test_default_optimizer_consumes_raw_profile_evidence(tmp_path):
    module_name = "counter_dump_raw_profile_case"
    (tmp_path / f"{module_name}.py").write_text(
        """
def compare(a, b):
    return a < b

def run():
    return compare(1, 2)
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        "assert module.run() is True",
    )
    base_env = _soac_subprocess_env(tmp_path, work_dir=work_dir)
    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)

    for opt_mode in ("verify", "apply"):
        result = _run_soac_subprocess(
            script,
            env={**base_env, "SOAC_OPT_MODE": opt_mode},
        )
        _assert_subprocess_ok(result)


def test_verify_counter_dump_records_refcount_decref_locations(tmp_path):
    module_name = "counter_dump_refcount_location_case"
    (tmp_path / f"{module_name}.py").write_text(
        """
def make(value):
    x = [value]
    return value

def run():
    for index in range(5):
        assert make(index) == index
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        "module.run()",
    )
    base_env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)

    verify_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
    )
    _assert_subprocess_ok(verify_result)
    verify = _inspect_counter_dump_json(work_dir / "verify.bin")

    location_counts = {}
    for record in verify["records"]:
        if record["module_name"] != module_name:
            continue
        for row in record["rows"]:
            if row["kind"] != "runtime_decref_location":
                continue
            for branch, value in row["branches"].items():
                location_counts[branch] = location_counts.get(branch, 0) + value

    assert any(
        value > 0
        and "name=x" in branch
        and ("reason=return" in branch or "purpose=stack_exit_sweep" in branch)
        for branch, value in location_counts.items()
    ), verify


def test_verify_virtual_constructor_escape_keeps_materialization_inputs_bound(tmp_path):
    module_name = "counter_dump_virtual_escape_case"
    (tmp_path / f"{module_name}.py").write_text(
        """
class Box:
    def __init__(self, value):
        self.value = value

sink = None

def make(value):
    global sink
    x = Box(value)
    sink = x
    return value

def run():
    for index in range(5):
        assert make(index) == index
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        "module.run()",
    )
    base_env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)

    verify_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
    )
    _assert_subprocess_ok(verify_result)


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


def test_unplanned_field_access_records_generic_counters_not_indexed_fallback(tmp_path):
    module_name = "field_generic_counter_case"
    (tmp_path / f"{module_name}.py").write_text(
        """
class Box:
    pass

def write_and_read(value):
    box = Box()
    box.x = value
    return box.x
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        """
        for index in range(5):
            assert module.write_and_read(index) == index
        """,
    )
    base_env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )

    result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(result)
    profile = _inspect_counter_dump_json(work_dir / "profile.bin")

    by_kind = {}
    for record in profile["records"]:
        if record["module_name"] != module_name:
            continue
        for row in record["rows"]:
            if row["function_qualname"] != "write_and_read":
                continue
            if row["kind"] == "field_access":
                for branch, value in row["branches"].items():
                    by_kind[f"field_access.{branch}"] = (
                        by_kind.get(f"field_access.{branch}", 0) + value
                    )
            else:
                by_kind[row["kind"]] = by_kind.get(row["kind"], 0) + row["value"]

    assert by_kind["field_access.generic_getattr"] >= 5, profile
    assert by_kind["field_access.generic_setattr"] >= 5, profile
    assert by_kind.get("field_access.indexed_hit", 0) == 0, profile
    assert by_kind.get("field_access.indexed_fallback", 0) == 0, profile


def test_profile_records_runtime_protocol_iter_target(tmp_path):
    module_name = "runtime_protocol_iter_profile_case"
    (tmp_path / f"{module_name}.py").write_text(
        """
class SelfIter:
    def __init__(self, stop):
        self.current = 0
        self.stop = stop

    def __iter__(self):
        return self

    def __next__(self):
        if self.current >= self.stop:
            raise StopIteration
        value = self.current
        self.current = value + 1
        return value

def run(stop):
    total = 0
    for value in SelfIter(stop):
        total += value
    return total
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    ids_path = tmp_path / "function-ids.json"
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        f"""
        import ctypes
        import json
        from pathlib import Path
        assert module.run(4) == 6
        get_function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
        get_function_id.argtypes = [ctypes.py_object]
        get_function_id.restype = ctypes.c_uint64
        Path({str(ids_path)!r}).write_text(json.dumps({{
            "iter": get_function_id(module.SelfIter.__dict__["__iter__"]),
            "run": get_function_id(module.run),
        }}), encoding="utf-8")
        """,
    )
    result = _run_soac_subprocess(
        script,
        env=_soac_subprocess_env(
            tmp_path,
            work_dir=work_dir,
            extra_env={
                "SOAC_COMPILE_MODE": "eager",
                "SOAC_OPT_MODE": "profile",
            },
        ),
    )
    _assert_subprocess_ok(result)

    profile = _inspect_counter_dump_json(work_dir / "profile.bin")
    function_ids = json.loads(ids_path.read_text(encoding="utf-8"))
    iter_id = function_ids["iter"]
    run_id = function_ids["run"]
    assert any(
        row["kind"] == "call_hot_targets"
        and row["function_id"] == run_id
        and row.get("observed_value") == iter_id
        for record in profile["records"]
        if record["module_name"] == module_name
        for row in record["rows"]
    ), profile


def test_module_load_event_is_written_to_soac_log_json(tmp_path):
    log_path = tmp_path / "soac-events.jsonl"
    module_path = tmp_path / "module_load_log_case.py"
    source = """
VALUE = 5

def read():
    return VALUE
"""
    module_path.write_text(source, encoding="utf-8")
    result = _run_soac_subprocess(
        _import_and_run_script(
            tmp_path,
            "import module_load_log_case",
            "assert module_load_log_case.read() == 5",
        ),
        env=_soac_subprocess_env(
            tmp_path,
            extra_env={
                "SOAC_LOG": f"soac_module_load=info,soac_jit_codegen=info;json={log_path}",
            },
        ),
    )
    _assert_subprocess_ok(result)

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
        "blockpy.blockpy",
        "exec_module.call_module_init",
        "exec_module.register_function_owner_types",
        "exec_module.eager_jit_compile",
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


def test_eager_compile_keeps_named_generators_on_source_vectorcall(tmp_path):
    module_name = "eager_generator_compile_case"
    (tmp_path / f"{module_name}.py").write_text(
        """
def explicit_items(limit):
    for item in range(limit):
        yield item + 1

def gen_sum(limit):
    return sum(item + 1 for item in range(limit))

def run(repeats):
    total = 0
    for _ in range(repeats):
        total += gen_sum(8)
        total += sum(explicit_items(8))
    return total
""",
        encoding="utf-8",
    )

    result = _run_soac_subprocess(
        _import_and_run_script(
            tmp_path,
            f"import {module_name} as module",
            "assert callable(module.explicit_items)",
        ),
        env=_soac_subprocess_env(
            tmp_path,
            work_dir=tmp_path / "soac-work",
            extra_env={
                "SOAC_COMPILE_MODE": "eager",
            },
        ),
    )
    _assert_subprocess_ok(result)

    code_summary_rows = _read_jsonl(tmp_path / "soac-work" / "jit-code-summary.jsonl")
    explicit_codegen_rows = [
        row
        for row in code_summary_rows
        if row.get("entry_kind") == "direct_function_body"
        and row.get("function_qualname", "").endswith("explicit_items")
    ]

    assert not explicit_codegen_rows


def test_apply_eager_compile_preserves_named_generators_and_prewarms_genexpr(tmp_path):
    module_name = "apply_eager_source_named_generator_case"
    work_dir = tmp_path / "soac-work"
    (tmp_path / f"{module_name}.py").write_text(
        """
def explicit_items(limit):
    for item in range(limit):
        yield item + 1

def gen_sum(limit):
    return sum(item + 1 for item in range(limit))

def run():
    return sum(explicit_items(8)) + gen_sum(8)
""",
        encoding="utf-8",
    )
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        "assert module.run() == 72",
    )
    base_env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    summary_path = work_dir / "jit-code-summary.jsonl"
    profile_codegen_rows = _read_jsonl(summary_path)
    assert any(
        row.get("entry_kind") == "direct_function_body"
        and row.get("function_qualname", "").endswith("explicit_items")
        for row in profile_codegen_rows
    ), profile_codegen_rows

    apply_script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        """
        import dis

        assert module.run() == 72
        named_items = module.explicit_items
        expected = tuple(range(1, 9))

        def call_named(value):
            return named_items(value)

        for _ in range(128):
            assert tuple(call_named(8)) == expected

        call_opnames = {
            instruction.opname
            for instruction in dis.get_instructions(call_named, adaptive=True)
            if instruction.opname.startswith("CALL")
        }
        assert "CALL_PY_EXACT_ARGS" in call_opnames, call_opnames

        named_items.__defaults__ = (8,)
        for _ in range(128):
            assert tuple(call_named(8)) == expected

        call_opnames = {
            instruction.opname
            for instruction in dis.get_instructions(call_named, adaptive=True)
            if instruction.opname.startswith("CALL")
        }
        assert not any(
            opname.startswith("CALL_PY_") for opname in call_opnames
        ), call_opnames
        """,
    )
    apply_result = _run_soac_subprocess(
        apply_script,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
    )
    _assert_subprocess_ok(apply_result)
    apply_codegen_rows = _read_jsonl(summary_path)[len(profile_codegen_rows) :]

    assert apply_codegen_rows
    assert not any(
        row.get("entry_kind") == "direct_function_body"
        and row.get("function_qualname", "").endswith("explicit_items")
        for row in apply_codegen_rows
    ), apply_codegen_rows
    assert any(
        row.get("entry_kind") == "direct_function_body"
        and row.get("function_qualname", "").endswith("<genexpr>")
        for row in apply_codegen_rows
    ), apply_codegen_rows


def test_apply_promoted_generator_globals_preserve_indexed_global_fallbacks(tmp_path):
    module_name = "apply_promoted_generator_globals_case"
    work_dir = tmp_path / "soac-work"
    (tmp_path / f"{module_name}.py").write_text(
        """
VALUE = 3

class Reader:
    def current(self):
        return VALUE

def named_items(limit):
    for item in range(limit):
        yield VALUE + item

def named_lengths(values):
    yield len(values)

def bump_value(delta):
    global VALUE
    VALUE += delta
    return VALUE

def shadow_and_restore_len(values):
    global len
    original = next(named_lengths(values))
    len = lambda ignored: 19
    shadowed = next(named_lengths(values))
    del len
    return original, shadowed, next(named_lengths(values))

def run():
    before = tuple(named_items(2))
    updated = bump_value(4)
    after = tuple(named_items(2))
    class_value = Reader().current()
    lengths = shadow_and_restore_len((1, 2, 3))
    return before, updated, after, class_value, lengths
""",
        encoding="utf-8",
    )
    profile_script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        """
        assert module.named_items.__globals__ is module.__dict__
        assert module.named_lengths.__globals__ is module.__dict__
        assert tuple(module.named_items(2)) == (3, 4)
        assert tuple(module.named_lengths((1, 2, 3))) == (3,)
        assert module.bump_value(4) == 7
        assert module.Reader().current() == 7
        assert tuple(module.named_items(2)) == (7, 8)
        assert tuple(module.named_lengths((1, 2, 3))) == (3,)
        assert module.VALUE == 7
        assert "len" not in module.__dict__
        """,
    )
    apply_script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        """
        assert module.named_items.__globals__ is module.__dict__
        assert module.named_lengths.__globals__ is module.__dict__
        assert module.run() == ((3, 4), 7, (7, 8), 7, (3, 19, 3))
        assert module.VALUE == 7
        assert tuple(module.named_items(2)) == (7, 8)
        assert "len" not in module.__dict__
        assert not any(
            key.startswith("__soac_source_named_generator_globals_promotion__")
            for key in module.__dict__
        )
        """,
    )
    base_env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    profile_result = _run_soac_subprocess(
        profile_script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        timeout=30,
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    apply_result = _run_soac_subprocess(
        apply_script,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
        timeout=30,
    )
    _assert_subprocess_ok(apply_result)


def test_apply_eager_compile_prewarms_nested_genexpr_direct_entries(tmp_path):
    module_name = "eager_nested_genexpr_precompile_case"
    work_dir = tmp_path / "soac-work"
    (tmp_path / f"{module_name}.py").write_text(
        """
def consume(limit):
    return sum(item + 1 for item in range(limit))

def run():
    return consume(8)
""",
        encoding="utf-8",
    )
    profile_script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        "assert module.run() == 36",
    )
    base_env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    profile_result = _run_soac_subprocess(
        profile_script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    summary_path = work_dir / "jit-code-summary.jsonl"
    if summary_path.exists():
        summary_path.unlink()

    apply_script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        "assert module.run() == 36",
    )
    apply_result = _run_soac_subprocess(
        apply_script,
        env={
            **base_env,
            "SOAC_OPT_MODE": "apply",
        },
    )
    _assert_subprocess_ok(apply_result)

    rows = _read_jsonl(summary_path)
    genexpr_codegen_rows = [
        row
        for row in rows
        if row.get("entry_kind") == "direct_function_body"
        and row.get("function_qualname", "").endswith("<genexpr>")
    ]
    assert genexpr_codegen_rows
    assert len(genexpr_codegen_rows) == 1, genexpr_codegen_rows


def test_apply_mode_diagonal_set_consumers_preserves_outer_cols_binding(tmp_path):
    module_name = "apply_diagonal_set_consumers_case"
    work_dir = tmp_path / "soac-work"
    (tmp_path / f"{module_name}.py").write_text(
        """
def diagonal_set_consumers(queen_count):
    cols = range(queen_count)
    vec = tuple(range(queen_count))
    total = 0
    total += len(set(vec[i] + i for i in cols))
    total += len(set(vec[i] - i for i in cols))
    return total

def run():
    return diagonal_set_consumers(4)
""",
        encoding="utf-8",
    )
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        "assert module.run() == 5",
    )
    base_env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    apply_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
    )
    _assert_subprocess_ok(apply_result)


def test_apply_mode_lazy_diagonal_slice_runner_preserves_expected_result_binding(tmp_path):
    bench_dir = Path(__file__).resolve().parents[1] / "bench"
    work_dir = tmp_path / "soac-work"
    script = _import_and_run_script(
        bench_dir,
        "import nqueens_slice_diagonal_set_consumers as module",
        'assert module.main(["nqueens_slice_diagonal_set_consumers.py", "4", "1"]) == 0',
    )
    base_env = _soac_subprocess_env(bench_dir, work_dir=work_dir)
    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    apply_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
    )
    _assert_subprocess_ok(apply_result)


def test_specialized_nested_generator_identity_iter_preserves_generator_state(tmp_path):
    module_name = "nested_generator_identity_iter_case"
    work_dir = tmp_path / "soac-work"
    (tmp_path / f"{module_name}.py").write_text(
        """
def values(start, count):
    for offset in range(count):
        yield start + offset


def nested_identity_iterator(start):
    iterator = iter(values(start, 3))
    return next(iterator), next(iterator), next(iterator)


def observable_identity_iterator(start):
    generator = values(start, 3)
    iterator = iter(generator)
    return iterator is generator, next(iterator), next(generator), next(iterator)


def nested_generator_loop(start):
    total = 0
    for value in values(start, 3):
        total += value
    return total


def independent_nested_iterators():
    left = iter(values(3, 2))
    right = iter(values(9, 2))
    return next(left), next(right), next(left), next(right)


def resume_generator(first, second, third, fourth, fifth):
    return first + second + third + fourth + fifth


def independently_rebound_resume_generator():
    return resume_generator(1, 2, 3, 4, 5)


def run():
    return (
        nested_identity_iterator(3),
        observable_identity_iterator(3),
        nested_generator_loop(3),
        independent_nested_iterators(),
        independently_rebound_resume_generator(),
    )
""",
        encoding="utf-8",
    )
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        "assert module.run() == ((3, 4, 5), (True, 3, 4, 5), 12, (3, 9, 4, 10), 15)",
    )
    base_env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )

    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        timeout=60,
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    for opt_mode in ("verify", "apply"):
        specialized_result = _run_soac_subprocess(
            script,
            env={**base_env, "SOAC_OPT_MODE": opt_mode},
            timeout=60,
        )
        _assert_subprocess_ok(specialized_result)


def test_specialized_full_nqueens_slice_preserves_generator_state(tmp_path):
    bench_dir = Path(__file__).resolve().parents[1] / "bench"
    work_dir = tmp_path / "soac-work"
    script = _import_and_run_script(
        bench_dir,
        "import nqueens_slice_full_nqueens_list_consumer as module",
        'assert module.main(["nqueens_slice_full_nqueens_list_consumer.py", "4", "1"]) == 0',
    )
    base_env = _soac_subprocess_env(
        bench_dir,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )

    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        timeout=60,
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    profile = _inspect_counter_dump_json(work_dir / "profile.bin")
    nqueens_record = next(
        record
        for record in profile["records"]
        if record["module_name"] == "nqueens_slice_full_nqueens_list_consumer"
    )
    for function_qualname in ("permutations", "n_queens"):
        assert any(
            row["function_qualname"] == function_qualname
            and row["kind"] == "call_hot_targets"
            and row["value"] > 0
            for row in nqueens_record["rows"]
        ), (function_qualname, profile)
    assert any(
        row["kind"] == "getitem_hot_shapes"
        and row["observed_value"] == 2
        and row["value"] > 0
        for row in nqueens_record["rows"]
    ), profile

    for opt_mode in ("verify", "apply"):
        specialized_result = _run_soac_subprocess(
            script,
            env={**base_env, "SOAC_OPT_MODE": opt_mode},
            timeout=60,
        )
        _assert_subprocess_ok(specialized_result)


def test_apply_mode_callback_pair_inline_preserves_genexpr_argument_binding(tmp_path):
    support_module_name = "callback_pair_inline_support_case"
    module_name = "callback_pair_inline_genexpr_binding_case"
    work_dir = tmp_path / "soac-work"
    (tmp_path / f"{support_module_name}.py").write_text(
        """
def parse_args(argv):
    compile_only = False
    args = list(argv[1:])
    if args and args[-1] == "--compile-only":
        compile_only = True
        args.pop()
    queen_count = int(args[0])
    loops = int(args[1])
    return queen_count, loops, compile_only

def run(name, workload, expected_result, argv):
    queen_count, loops, compile_only = parse_args(argv)
    if compile_only:
        return workload(1)
    expected = expected_result(queen_count)
    result = None
    for _ in range(loops):
        result = workload(queen_count)
    if result != expected:
        return 1
    return 0
""",
        encoding="utf-8",
    )
    (tmp_path / f"{module_name}.py").write_text(
        """
from callback_pair_inline_support_case import run

def workload(queen_count):
    cols = range(queen_count)
    vec = tuple(range(queen_count))
    total = 0
    total += len(set(vec[i] + i for i in cols))
    total += len(set(vec[i] - i for i in cols))
    return total

def expected_result(queen_count):
    return queen_count + 1

def main(argv):
    return run("diag", workload, expected_result, argv)
""",
        encoding="utf-8",
    )
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        'assert module.main(["callback_pair_inline_genexpr_binding_case.py", "4", "1"]) == 0',
    )
    base_env = _soac_subprocess_env(tmp_path, work_dir=work_dir)
    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    apply_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
    )
    _assert_subprocess_ok(apply_result)


def test_profile_eager_runtime_import_does_not_compile_runtime_entries(tmp_path):
    log_path = tmp_path / "runtime-import-events.jsonl"
    result = _run_soac_subprocess(
        "\n".join(
            [
                "import builtins",
                "from soac.import_hook import install",
                "install()",
                "import soac.runtime as runtime",
                "assert runtime.range is builtins.range",
                "",
            ]
        ),
        env=_soac_subprocess_env(
            tmp_path,
            work_dir=tmp_path / "soac-work",
            extra_env={
                "SOAC_COMPILE_MODE": "eager",
                "SOAC_OPT_MODE": "profile",
                "SOAC_LOG": f"soac_jit_codegen=info;json={log_path}",
            },
        ),
    )
    _assert_subprocess_ok(result)

    rows = _read_jsonl(log_path)
    runtime_codegen_rows = [
        row
        for row in rows
        if row.get("event") == "soac.jit_codegen"
        and row.get("module_name") == "soac.runtime"
    ]
    assert not runtime_codegen_rows


def test_pre_optimization_blockpy_module_cache_is_reused(tmp_path):
    work_dir = tmp_path / "soac-work"
    cache_dir = work_dir / "modules"
    log_path = tmp_path / "cache-events.jsonl"
    module_path = tmp_path / "module_cache_case.py"
    module_path.write_text(
        """
def value():
    return 42
""",
        encoding="utf-8",
    )
    env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={
            "SOAC_LOG": f"soac_blockpy_module_cache=info;json={log_path}",
        },
    )
    script = _import_and_run_script(
        tmp_path,
        "import module_cache_case",
        "assert module_cache_case.value() == 42",
    )

    first = _run_soac_subprocess(script, env=env)
    _assert_subprocess_ok(first)
    second = _run_soac_subprocess(script, env=env)
    _assert_subprocess_ok(second)

    cache_files = list(cache_dir.rglob("mod.blockpy"))
    assert cache_files
    rows = _read_jsonl(log_path)
    assert any(
        row.get("event") == "soac.blockpy_module_cache" and row.get("cache_hit") is True
        for row in rows
    )


def test_soac_work_dir_is_default_module_artifact_root(tmp_path):
    work_dir = tmp_path / "soac-work"
    module_path = tmp_path / "work_dir_cache_case.py"
    module_path.write_text("def read():\n    return 17\n", encoding="utf-8")

    result = _run_soac_subprocess(
        _import_and_run_script(
            tmp_path,
            "import work_dir_cache_case",
            "assert work_dir_cache_case.read() == 17",
        ),
        env=_soac_subprocess_env(
            tmp_path,
            work_dir=work_dir,
        ),
    )
    _assert_subprocess_ok(result)

    assert list((work_dir / "modules").rglob("mod.blockpy"))


def test_soac_work_dir_is_default_event_log_dir(tmp_path):
    work_dir = tmp_path / "soac-work"
    log_path = work_dir / "events.jsonl"
    module_path = tmp_path / "work_dir_log_case.py"
    module_path.write_text("def read():\n    return 11\n", encoding="utf-8")
    result = _run_soac_subprocess(
        _import_and_run_script(
            tmp_path,
            "import work_dir_log_case",
            "assert work_dir_log_case.read() == 11",
        ),
        env=_soac_subprocess_env(tmp_path, work_dir=work_dir),
    )
    _assert_subprocess_ok(result)

    rows = _read_jsonl(log_path)
    assert any(
        row.get("event") == "soac.module_load"
        and row["module_name"].endswith("work_dir_log_case")
        for row in rows
    )


def test_apply_mode_does_not_emit_specialization_runtime_counter_logs(
    profiled_specialization_runtime_case,
):
    log_path = profiled_specialization_runtime_case["base_dir"] / "apply-events.jsonl"

    result = _run_soac_subprocess(
        profiled_specialization_runtime_case["script"],
        env={
            **profiled_specialization_runtime_case["base_env"],
            "SOAC_OPT_MODE": "apply",
            "SOAC_LOG": f"soac_specialization_runtime=info;json={log_path}",
        },
    )
    _assert_subprocess_ok(result)

    rows = _read_jsonl(log_path)
    runtime_rows = [
        row
        for row in rows
        if row.get("event") == "soac.specialization_runtime"
        and row["module_name"].endswith("specialization_runtime_case")
    ]
    assert not runtime_rows, runtime_rows


def test_apply_mode_default_event_log_omits_specialization_runtime_counters(
    profiled_specialization_runtime_case,
):
    log_path = profiled_specialization_runtime_case["work_dir"] / "events.jsonl"

    result = _run_soac_subprocess(
        profiled_specialization_runtime_case["script"],
        env={
            **profiled_specialization_runtime_case["base_env"],
            "SOAC_OPT_MODE": "apply",
        },
    )
    _assert_subprocess_ok(result)

    rows = _read_jsonl(log_path)
    runtime_rows = [
        row
        for row in rows
        if row.get("event") == "soac.specialization_runtime"
        and row["module_name"].endswith("specialization_runtime_case")
    ]
    assert not runtime_rows, runtime_rows


def test_getitem_v3_profile_replay_records_hit_and_fallback_counters(tmp_path):
    module_path = tmp_path / "getitem_specialization_case.py"
    module_path.write_text(
        """
class OverrideList(list):
    def __getitem__(self, index):
        return 100 + index


def get_item(obj, index):
    return obj[index]


def run_case():
    total = 0
    total += get_item([10, 20, 30], 1)
    total += get_item([40, 50, 60], -1)
    total += get_item(OverrideList([1, 2, 3]), 2)
    return total
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    script = _import_and_run_script(
        tmp_path,
        "import getitem_specialization_case",
        "assert getitem_specialization_case.run_case() == 182",
    )
    base_env = _soac_subprocess_env(tmp_path, work_dir=work_dir)

    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    profile_dump_path = work_dir / "profile.bin"
    profile = _inspect_counter_dump_json(profile_dump_path)
    profiled_shapes = [
        row
        for record in profile["records"]
        if record["module_name"] == "getitem_specialization_case"
        for row in record["rows"]
        if row["kind"] == "getitem_hot_shapes"
        and row["observed_value"] == 1
        and row["value"] > 0
    ]
    assert profiled_shapes, profile

    verify_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
    )
    _assert_subprocess_ok(verify_result)
    verify_dump_path = work_dir / "verify.bin"
    verify = _inspect_counter_dump_json(verify_dump_path)
    hit_count = sum(
        _counter_branch(row, "hit")
        for record in verify["records"]
        if record["module_name"] == "getitem_specialization_case"
        for row in record["rows"]
        if row["kind"] == "getitem_specialized"
    )
    fallback_count = sum(
        _counter_branch(row, "fallback")
        for record in verify["records"]
        if record["module_name"] == "getitem_specialization_case"
        for row in record["rows"]
        if row["kind"] == "getitem_specialized"
    )
    assert hit_count >= 2, verify
    assert fallback_count >= 1, verify


def test_setitem_v3_profile_replay_records_hit_and_fallback_counters(tmp_path):
    module_path = tmp_path / "setitem_specialization_case.py"
    module_path.write_text(
        """
class OverrideList(list):
    def __setitem__(self, index, value):
        super().__setitem__(index, 100 + value)


def set_item(obj, index, value):
    obj[index] = value
    return obj


def run_case():
    first = [10, 20, 30]
    second = [40, 50, 60]
    override = OverrideList([1, 2, 3])
    set_item(first, 1, 99)
    set_item(second, -1, 77)
    set_item(override, 2, 5)
    return first[1] + second[2] + override[2]
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    script = _import_and_run_script(
        tmp_path,
        "import setitem_specialization_case",
        "assert setitem_specialization_case.run_case() == 281",
    )
    base_env = _soac_subprocess_env(tmp_path, work_dir=work_dir)

    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    profile_dump_path = work_dir / "profile.bin"
    profile = _inspect_counter_dump_json(profile_dump_path)
    profiled_shapes = [
        row
        for record in profile["records"]
        if record["module_name"] == "setitem_specialization_case"
        for row in record["rows"]
        if row["kind"] == "setitem_hot_shapes"
        and row["observed_value"] == 1
        and row["value"] > 0
    ]
    assert profiled_shapes, profile

    verify_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
    )
    _assert_subprocess_ok(verify_result)
    verify_dump_path = work_dir / "verify.bin"
    verify = _inspect_counter_dump_json(verify_dump_path)
    hit_count = sum(
        _counter_branch(row, "hit")
        for record in verify["records"]
        if record["module_name"] == "setitem_specialization_case"
        for row in record["rows"]
        if row["kind"] == "setitem_specialized"
    )
    fallback_count = sum(
        _counter_branch(row, "fallback")
        for record in verify["records"]
        if record["module_name"] == "setitem_specialization_case"
        for row in record["rows"]
        if row["kind"] == "setitem_specialized"
    )
    assert hit_count >= 2, verify
    assert fallback_count >= 1, verify


def test_cross_module_field_profile_uses_type_id_table(tmp_path):
    owner_path = tmp_path / "field_owner_case.py"
    owner_path.write_text(
        """
class Point:
    def __init__(self):
        self.x = 0
        self.y = 0

class Vector:
    def __init__(self):
        self.x = 0
        self.y = 0

_warmup_point = Point()
_warmup_vector = Vector()
""",
        encoding="utf-8",
    )
    user_path = tmp_path / "field_user_case.py"
    user_path.write_text(
        """
from field_owner_case import Point, Vector

point = Point()
vector = Vector()

def read_fields():
    point.x = 40
    point.y = 2
    vector.x = 30
    vector.y = 12
    return point.x + point.y + vector.x + vector.y

def write_field():
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

    script = _import_and_run_script(
        tmp_path,
        "import field_user_case",
        textwrap.dedent(
            """
            for _ in range(50):
                assert field_user_case.read_fields() == 84
                assert field_user_case.write_field() == 84
            """
        ).strip(),
    )
    base_env = _soac_subprocess_env(tmp_path, work_dir=work_dir)

    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
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

    verify_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
    )
    _assert_subprocess_ok(verify_result)
    verify_dump_path = work_dir / "verify.bin"
    assert verify_dump_path.exists()

    verify = _inspect_counter_dump_json(verify_dump_path)
    field_hits = [
        row
        for record in verify["records"]
        if record["module_name"] == "field_user_case"
        for row in record["rows"]
        if row["kind"] == "field_access" and _counter_branch(row, "indexed_hit") > 0
    ]
    assert field_hits, verify
    hit_counts_by_function = {}
    for row in field_hits:
        hit_counts_by_function[row["function_qualname"]] = (
            hit_counts_by_function.get(row["function_qualname"], 0) + 1
        )
    # Verify mode should measure both field loads and field stores on the
    # specialized path: one JIT run of read_fields has 4 stores + 4 loads, and
    # one JIT run of write_field has 7 stores + 4 loads. Background compilation
    # may finish before any run, or only after the first warmup run, so assert
    # the minimum specialized-path coverage instead of an exact run count.
    assert hit_counts_by_function["read_fields"] >= 8, verify
    assert hit_counts_by_function["write_field"] >= 11, verify


def test_method_class_field_profile_uses_indexed_get_set_in_verify(tmp_path):
    module_name = "field_method_case"
    module_path = tmp_path / f"{module_name}.py"
    module_path.write_text(
        """
class Record:
    def __init__(self, x=0, y=0):
        self.x = x
        self.y = y

    def copy(self):
        return Record(self.x, self.y)

_warmup_record = Record()

def run():
    record = Record(1, 2)
    record.x = 3
    copied = record.copy()
    return copied.x + copied.y + record.x
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    script = _import_and_run_script(
        tmp_path,
        f"import {module_name} as module",
        "for _ in range(50):\n    assert module.run() == 8",
    )
    base_env = _soac_subprocess_env(
        tmp_path,
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "lazy", "SOAC_BACKGROUND_JIT": "0"},
    )

    profile_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    profile = _inspect_counter_dump_json(work_dir / "profile.bin")
    record_type_entries = {
        (entry["type_id"], entry["module_name"], entry["qualname"])
        for record in profile["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name and entry["qualname"] == "Record"
    }
    assert len(record_type_entries) == 1, profile
    record_type_id = next(iter(record_type_entries))[0]
    profiled_keys = {
        key["key"]
        for record in profile["records"]
        for key in record["type_keys"]
        if key["owner_type_id"] == record_type_id
    }
    assert profiled_keys == {"x", "y"}, profile

    verify_result = _run_soac_subprocess(
        script,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
    )
    _assert_subprocess_ok(verify_result)
    verify = _inspect_counter_dump_json(work_dir / "verify.bin")

    hit_values_by_function = {}
    fallback_values_by_function = {}
    for record in verify["records"]:
        if record["module_name"] != module_name:
            continue
        for row in record["rows"]:
            if row["kind"] == "field_access":
                hit_values_by_function[row["function_qualname"]] = (
                    hit_values_by_function.get(row["function_qualname"], 0)
                    + _counter_branch(row, "indexed_hit")
                )
                fallback_values_by_function[row["function_qualname"]] = (
                    fallback_values_by_function.get(row["function_qualname"], 0)
                    + _counter_branch(row, "indexed_fallback")
                )

    assert hit_values_by_function["run"] >= 4, verify
    assert sum(hit_values_by_function.values()) >= 4, verify
    assert not {
        function: value
        for function, value in fallback_values_by_function.items()
        if value and function == "run"
    }, verify
