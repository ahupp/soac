from __future__ import annotations

import ast
import importlib.util
import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest
from tests._integration import stock_module
from tests._strict_integration import assert_strict_source_rejected, create_strict_project


def _inspect_counter_dump_json(path):
    import _soac_ext

    return json.loads(_soac_ext.inspect_counter_dump_json(str(path)))


def _read_jsonl(path):
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def _strict_counter_source(source):
    """Add only the explicit opt-in, preserving the original module docstring/body."""
    source = textwrap.dedent(source).lstrip("\n")
    tree = ast.parse(source)
    assert not any(
        isinstance(node, ast.ImportFrom) and node.module == "__future__"
        and any(alias.name == "strict" for alias in node.names)
        for node in tree.body
    ), "counter fixture source already opted in"
    position = 0
    if (
        tree.body
        and isinstance(tree.body[0], ast.Expr)
        and isinstance(tree.body[0].value, ast.Constant)
        and isinstance(tree.body[0].value.value, str)
    ):
        position = tree.body[0].end_lineno
    lines = source.splitlines(keepends=True)
    prefix = "".join(lines[:position])
    if prefix and not prefix.endswith("\n"):
        prefix += "\n"
    return prefix + "from __future__ import strict\n" + "".join(lines[position:])


def _counter_project(root, modules, *, env, ordinary_sources=None, backend="soac"):
    """Publish explicit source selections with the real checker/startup protocol."""
    sources = {
        path.name: _strict_counter_source(path.read_text(encoding="utf-8"))
        for path in modules.values()
    }
    assert len(sources) == len(modules), "counter module filenames must be unique"
    if ordinary_sources:
        assert not sources.keys() & ordinary_sources.keys(), "ordinary and selected sources overlap"
        sources.update(ordinary_sources)
    # These are existing execution/logging settings, not contract authority.
    # Capture the same requested lazy/eager/background setup before publication.
    with pytest.MonkeyPatch.context() as patch:
        for name in (
            "SOAC_MODULE_ENABLED", "SOAC_WORK_DIR", "SOAC_LOG",
            "SOAC_COMPILE_MODE", "SOAC_BACKGROUND_JIT", "SOAC_OPT_MODE",
        ):
            patch.delenv(name, raising=False)
        for name, value in env.items():
            patch.setenv(name, value)
        patch.setenv("SOAC_OPT_MODE", env.get("SOAC_OPT_MODE", "none"))
        return create_strict_project(
            root / "strict-publication", sources,
            modules={name: path.name for name, path in modules.items()},
            backend=backend,
        )


def _counter_env(*, work_dir=None, extra_env=None):
    # The old subprocess helper removed SOAC_COMPILE_MODE, selecting Lazy.
    # StrictProject.run defaults to Eager, so make the original choice explicit.
    env = {
        "SOAC_COMPILE_MODE": "lazy",
        "SOAC_BACKGROUND_JIT": os.environ.get("SOAC_BACKGROUND_JIT", "1"),
    }
    if work_dir is not None:
        env["SOAC_WORK_DIR"] = str(work_dir)
    if extra_env:
        env.update(extra_env)
    return env


def _counter_script(import_stmt, body):
    # The project is supplied explicitly at execution, after mode setup and
    # publication; this tuple carries no runtime authority.
    return import_stmt, textwrap.dedent(body).strip()


def _run_counter_project(project, script, *, env, timeout=None):
    options = dict(env)
    opt_mode = options.pop("SOAC_OPT_MODE", "none")
    assert options["SOAC_COMPILE_MODE"] == project.environment["SOAC_COMPILE_MODE"]
    assert options["SOAC_BACKGROUND_JIT"] == project.environment["SOAC_BACKGROUND_JIT"]
    import_stmt, body = script
    expected_modules = [
        (name, str(project.project / source), project.publication["generation"])
        for name, source in project.modules.items()
    ]
    witness = f"""
for _counter_name, _counter_path, _counter_generation in {expected_modules!r}:
    _counter_diagnostic = _soac_ext.strict_module_diagnostics(sys.modules[_counter_name])
    assert _counter_diagnostic is not None, 'counter subject ran without strict ownership'
    assert _counter_diagnostic['sealed'] is True
    assert _counter_diagnostic['module_name'] == _counter_name
    assert _counter_diagnostic['source_path'] == _counter_path
    assert _counter_diagnostic['artifact_generation'] == _counter_generation
    assert _counter_diagnostic['initializer_entry_kind'] == 'entry_interpreter'
"""
    return project.run(
        import_stmt + "\n" + textwrap.dedent(witness) + "\n" + body + "\n",
        opt_mode=opt_mode, extra_env=options, timeout=timeout, check=False,
        backend="soac",
    )


def _ordinary_subprocess_env(module_root, *, work_dir=None, extra_env=None):
    """Only unmarked-source controls use this ordinary CPython launcher."""
    env = dict(os.environ)
    env["SOAC_MODULE_ENABLED"] = f"path:{module_root}"
    env.pop("SOAC_COMPILE_MODE", None)
    env.pop("SOAC_OPT_MODE", None)
    env.pop("DIET_PYTHON_MODE", None)
    if work_dir is not None:
        env["SOAC_WORK_DIR"] = str(work_dir)
    else:
        env.pop("SOAC_WORK_DIR", None)
    env.pop("SOAC_LOG", None)
    if extra_env:
        env.update(extra_env)
    return env


def _run_ordinary_subprocess(script, *, env, timeout=None):
    return subprocess.run(
        [sys.executable, "-c", script],
        check=False, capture_output=True, env=env, text=True, timeout=timeout,
    )


def _ordinary_script(module_root, import_stmt, body):
    return "\n".join(
        ["import sys", f"sys.path.insert(0, {str(module_root)!r})",
         import_stmt, textwrap.dedent(body).strip(), ""]
    )


def _assert_subprocess_ok(result):
    assert result.returncode == 0, result.stdout + result.stderr


def _counter_branch(row, branch):
    return row.get("branches", {}).get(branch, 0)


_BOX_GENERIC_COUNTER_ORIGINAL = """
class Box:
    pass

def write_and_read(value):
    box = Box()
    box.x = value
    return box.x
"""

_SPECIALIZATION_RUNTIME_ORIGINAL = """
VALUE = 9

class Point:
    pass

def run():
    point = Point()
    point.x = 33
    return point.x + VALUE
"""

# Driver-only observations. A source/profile identity is not permission to
# enter an unchecked body, and lazy execution need not already be native.
_COUNTER_FUNCTION_WITNESSES = """
import ctypes
_counter_apis = []
for _counter_api_name, _counter_result_type in (
    ("PyFunction_GetSoacFunctionId", ctypes.c_uint64),
    ("PyFunction_GetSoacStrictId", ctypes.c_uint64),
    ("PyFunction_GetSoacStrictOwner", ctypes.c_void_p),
    ("PyFunction_GetSoacMetadata", ctypes.c_void_p),
):
    _counter_api = getattr(ctypes.pythonapi, _counter_api_name)
    _counter_api.argtypes = [ctypes.py_object]
    _counter_api.restype = _counter_result_type
    _counter_apis.append(_counter_api)

def _counter_function_snapshot(function):
    return tuple(api(function) for api in _counter_apis) + (
        _soac_ext.strict_function_entry_kind(function),
    )

def _counter_source_id(function, entry_kind=None):
    actual = _counter_function_snapshot(function)
    assert actual[0] == 0 and actual[1] > 0, (function.__qualname__, actual)
    assert actual[2] and actual[3], (function.__qualname__, actual)
    if entry_kind is None:
        assert actual[4] in {"entry_interpreter", "checked_native"}, actual
    else:
        assert actual[4] == entry_kind, (function.__qualname__, actual)
    return actual[1]

def _counter_assert_ordinary(function):
    actual = _counter_function_snapshot(function)
    assert actual == (0, 0, None, None, None), (function.__qualname__, actual)
"""


@pytest.mark.parametrize("case", ["box", "point"])
def test_external_field_counter_originals_are_ordinary_and_strict_rejected(tmp_path, case):
    from soac import _soac_ext

    if case == "box":
        module_name, source = "field_generic_counter_case", _BOX_GENERIC_COUNTER_ORIGINAL
        function_name = "write_and_read"
    else:
        module_name, source = "specialization_runtime_case", _SPECIALIZATION_RUNTIME_ORIGINAL
        function_name = "run"
    with stock_module(tmp_path / "ordinary", module_name, source) as module:
        assert _soac_ext.strict_module_diagnostics(module) is None
        assert _soac_ext.strict_function_entry_kind(getattr(module, function_name)) is None
        if case == "box":
            for index in range(5):
                assert module.write_and_read(index) == index
        else:
            assert module.run() == 42
    # A plain candidate class's absent field is a blocking ty diagnostic, not
    # the automatic fallback reserved for unsupported framework receivers.
    assert_strict_source_rejected(
        tmp_path / "strict-original",
        _strict_counter_source(source),
        module_name=module_name,
        diagnostic="unresolved-attribute",
    )


@pytest.fixture(scope="module")
def profiled_specialization_runtime_case(tmp_path_factory):
    base_dir = tmp_path_factory.mktemp("counter-dump-specialization-runtime")
    module_name = "specialization_runtime_case"
    ordinary_name = "specialization_runtime_original"
    (base_dir / f"{module_name}.py").write_text(
        """
VALUE = 9

def run(point):
    point.x = 33
    return point.x + VALUE
""",
        encoding="utf-8",
    )
    work_dir = base_dir / "soac-work"
    script = _counter_script(
        f"import {module_name} as module\nimport {ordinary_name} as ordinary",
        _COUNTER_FUNCTION_WITNESSES + """
assert _soac_ext.strict_module_diagnostics(ordinary) is None
_counter_assert_ordinary(ordinary.run)
assert ordinary.run() == 42
point = ordinary.Point()
assert module.run(point) == 42
assert point.x == 33
_counter_source_id(module.run)
""",
    )
    base_env = _counter_env(work_dir=work_dir)
    project = _counter_project(
        base_dir, {module_name: base_dir / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        ordinary_sources={f"{ordinary_name}.py": _SPECIALIZATION_RUNTIME_ORIGINAL},
    )
    profile_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()
    return {
        "project": project,
        "base_dir": base_dir,
        "module_name": module_name,
        "script": script,
        "work_dir": work_dir,
        "base_env": base_env,
    }


def test_counter_dump_file_is_written_on_module_exit(tmp_path):
    work_dir = tmp_path / "soac-work"
    dump_path = work_dir / "profile.bin"

    source = """
VALUE = 7

def read():
    return VALUE
"""
    module_path = tmp_path / "counter_dump_file_case.py"
    module_path.write_text(source, encoding="utf-8")
    env = _counter_env(
        work_dir=work_dir,
        extra_env={
            "SOAC_OPT_MODE": "profile",
            "SOAC_COMPILE_MODE": os.environ.get("SOAC_COMPILE_MODE", "lazy"),
        },
    )
    project = _counter_project(tmp_path, {"counter_dump_file_case": module_path}, env=env)
    script = _counter_script(
        "import counter_dump_file_case as module",
        f"""
import gc
from pathlib import Path
assert module.read() == 7
assert module.read() == 7
# Observe module retirement before process exit; shutdown alone is not this test.
sys.modules.pop("counter_dump_file_case")
del module
gc.collect()
assert Path({str(dump_path)!r}).is_file()
""",
    )
    _assert_subprocess_ok(_run_counter_project(project, script, env=env))

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
    script = _counter_script(
        f"import {module_name} as module",
        "assert module.run() is True",
    )
    base_env = _counter_env(work_dir=work_dir)
    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").is_file()
    profile = _inspect_counter_dump_json(work_dir / "profile.bin")
    assert any(record["module_name"] == module_name for record in profile["records"])

    for opt_mode in ("verify", "apply"):
        result = _run_counter_project(
            project,
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
    script = _counter_script(
        f"import {module_name} as module",
        "module.run()",
    )
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)

    verify_result = _run_counter_project(
        project,
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
    script = _counter_script(
        f"import {module_name} as module",
        "module.run()",
    )
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)

    verify_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
    )
    _assert_subprocess_ok(verify_result)


def test_counter_dump_file_is_not_written_in_none_mode(tmp_path):
    work_dir = tmp_path / "soac-work"
    dump_path = work_dir / "profile.bin"

    source = """
VALUE = 7

def read():
    return VALUE
"""
    module_path = tmp_path / "counter_dump_none_mode_case.py"
    module_path.write_text(source, encoding="utf-8")
    env = _counter_env(
        work_dir=work_dir,
        extra_env={
            "SOAC_OPT_MODE": "none",
            "SOAC_COMPILE_MODE": os.environ.get("SOAC_COMPILE_MODE", "lazy"),
        },
    )
    project = _counter_project(tmp_path, {"counter_dump_none_mode_case": module_path}, env=env)
    script = _counter_script(
        "import counter_dump_none_mode_case as module",
        f"""
import gc
from pathlib import Path
assert module.read() == 7
assert module.read() == 7
# Observe module retirement before process exit; shutdown alone is not this test.
sys.modules.pop("counter_dump_none_mode_case")
del module
gc.collect()
assert not (Path({str(work_dir)!r}) / "profile.bin").exists()
assert not (Path({str(work_dir)!r}) / "verify.bin").exists()
""",
    )
    _assert_subprocess_ok(_run_counter_project(project, script, env=env))

    assert not (work_dir / "profile.bin").exists()
    assert not (work_dir / "verify.bin").exists()


def test_unplanned_field_access_records_generic_counters_not_indexed_fallback(tmp_path):
    module_name = "field_generic_counter_case"
    ordinary_name = "field_generic_counter_original"
    (tmp_path / f"{module_name}.py").write_text(
        """
def write_and_read(box, value):
    box.x = value
    return box.x
""",
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"
    script = _counter_script(
        f"import {module_name} as module\nimport {ordinary_name} as ordinary",
        _COUNTER_FUNCTION_WITNESSES + """
assert _soac_ext.strict_module_diagnostics(ordinary) is None
_counter_assert_ordinary(ordinary.write_and_read)
for index in range(5):
    assert ordinary.write_and_read(index) == index
    box = ordinary.Box()
    assert module.write_and_read(box, index) == index
    assert box.x == index
_counter_source_id(module.write_and_read, "checked_native")
""",
    )
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )

    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        ordinary_sources={f"{ordinary_name}.py": _BOX_GENERIC_COUNTER_ORIGINAL},
    )
    result = _run_counter_project(
        project,
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
    ids_path = tmp_path / "source-function-ids.json"
    script = _counter_script(
        f"import {module_name} as module",
        _COUNTER_FUNCTION_WITNESSES + f"""
import json
from pathlib import Path
assert module.run(4) == 6
Path({str(ids_path)!r}).write_text(json.dumps({{
    "iter": _counter_source_id(module.SelfIter.__dict__["__iter__"], "checked_native"),
    "run": _counter_source_id(module.run, "checked_native"),
}}), encoding="utf-8")
""",
    )
    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env=_counter_env(
            work_dir=work_dir,
            extra_env={
                "SOAC_COMPILE_MODE": "eager",
                "SOAC_OPT_MODE": "profile",
            },
        ),
    )
    result = _run_counter_project(
        project,
        script,
        env=_counter_env(
            work_dir=work_dir,
            extra_env={
                "SOAC_COMPILE_MODE": "eager",
                "SOAC_OPT_MODE": "profile",
            },
        ),
    )
    _assert_subprocess_ok(result)

    profile = _inspect_counter_dump_json(work_dir / "profile.bin")
    source_function_ids = json.loads(ids_path.read_text(encoding="utf-8"))
    iter_id = source_function_ids["iter"]
    run_id = source_function_ids["run"]
    assert iter_id != run_id, source_function_ids
    assert any(
        row["kind"] == "call_hot_targets"
        and row["function_id"] == run_id
        and row.get("observed_value") == iter_id
        and row["value"] > 0
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
    project = _counter_project(
        tmp_path, {"module_load_log_case": module_path},
        env=_counter_env(
            extra_env={
                "SOAC_LOG": f"soac_module_load=info,soac_jit_codegen=info;json={log_path}",
            },
        ),
    )
    result = _run_counter_project(
        project,
        _counter_script(
            "import module_load_log_case",
            "assert module_load_log_case.read() == 5",
        ),
        env=_counter_env(
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


def test_eager_compile_prewarms_named_resume_without_replacing_generator_factory(tmp_path):
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

    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env=_counter_env(
            work_dir=tmp_path / "soac-work",
            extra_env={
                "SOAC_COMPILE_MODE": "eager",
            },
        ),
    )
    result = _run_counter_project(
        project,
        _counter_script(
            f"import {module_name} as module",
            """
import gc
import types
import weakref
from soac import _soac_ext

assert callable(module.explicit_items)
named_items = module.explicit_items
assert _soac_ext.strict_function_entry_kind(named_items) == "generator_factory"
source_code = named_items.__code__

class Limit:
    def __index__(self):
        raise AssertionError("a created-and-closed generator entered its body")

limit = Limit()
limit_ref = weakref.ref(limit)
generator = named_items(limit)
assert type(generator) is types.GeneratorType
assert generator.gi_code is source_code
assert generator.__name__ == named_items.__name__
assert generator.__qualname__ == named_items.__qualname__
del limit
gc.collect()
assert limit_ref() is not None
generator.close()
assert tuple(generator) == ()
assert generator.gi_code is source_code
gc.collect()
assert limit_ref() is None
assert _soac_ext.strict_function_entry_kind(named_items) == "generator_factory"
""",
        ),
        env=_counter_env(
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

    # The machine-code record is the resume body, not the public factory
    # entry. Creation and close above never resume it, so this is eager work.
    assert len(explicit_codegen_rows) == 1, explicit_codegen_rows
    assert explicit_codegen_rows[0]["code_size"] > 0


def test_apply_eager_compile_prewarms_native_generator_resumes_and_genexpr(tmp_path):
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
    script = _counter_script(
        f"import {module_name} as module",
        """
from soac import _soac_ext
assert module.run() == 72
assert _soac_ext.strict_function_entry_kind(module.explicit_items) == "generator_factory"
""",
    )
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
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

    ordinary_apply_script = _ordinary_script(tmp_path,
        f"import {module_name} as module",
        """
        assert module.run() == 72
        named_items = module.explicit_items
        expected = tuple(range(1, 9))

        def call_named(value):
            return named_items(value)

        for _ in range(128):
            assert tuple(call_named(8)) == expected

        named_items.__defaults__ = (8,)
        for _ in range(128):
            assert tuple(call_named(8)) == expected

        """,
    )
    # Keep ordinary defaults semantics. The former CALL_PY_* assertions
    # described SOAC dispatch after a now-forbidden owned-defaults mutation,
    # not a CPython behavior guarantee; their exact text is archived in the
    # migration evidence. Other profile/apply observers below remain intact.
    _assert_subprocess_ok(_run_ordinary_subprocess(
        ordinary_apply_script, env=_ordinary_subprocess_env(tmp_path),
    ))
    apply_script = _counter_script(
        f"import {module_name} as module",
        """
import types
from soac import StrictMutationError, _soac_ext
assert module.run() == 72
named_items = module.explicit_items
expected = tuple(range(1, 9))
source_code = named_items.__code__
assert _soac_ext.strict_function_entry_kind(named_items) == "generator_factory"
assert named_items.__globals__ is module.__dict__

def call_named(value):
    return named_items(value)

generator = call_named(8)
assert type(generator) is types.GeneratorType
assert generator.gi_code is source_code
assert tuple(generator) == expected
assert generator.gi_code is source_code

for _ in range(128):
    assert tuple(call_named(8)) == expected
before_code = named_items.__code__
before_defaults = named_items.__defaults__
before_kwdefaults = named_items.__kwdefaults__
try:
    named_items.__defaults__ = (8,)
except StrictMutationError:
    pass
else:
    raise AssertionError("a sealed generator accepted defaults replacement")
assert named_items.__code__ is before_code
assert named_items.__defaults__ is before_defaults
assert named_items.__kwdefaults__ is before_kwdefaults
for _ in range(128):
    assert tuple(call_named(8)) == expected
assert _soac_ext.strict_function_entry_kind(named_items) == "generator_factory"
""",
    )
    apply_result = _run_counter_project(
        project,
        apply_script,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
    )
    _assert_subprocess_ok(apply_result)
    apply_codegen_rows = _read_jsonl(summary_path)[len(profile_codegen_rows) :]

    assert apply_codegen_rows
    # Strict named generators keep their checked native factory while their
    # authenticated resume body is compiled, including outside counter modes.
    explicit_codegen_rows = [
        row
        for row in apply_codegen_rows
        if row.get("entry_kind") == "direct_function_body"
        and row.get("function_qualname", "").endswith("explicit_items")
    ]
    assert len(explicit_codegen_rows) == 1, explicit_codegen_rows
    assert explicit_codegen_rows[0]["code_size"] > 0
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
    profile_script = _counter_script(
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
    apply_script = _counter_script(
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
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
        profile_script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        timeout=30,
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    apply_result = _run_counter_project(
        project,
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
    profile_script = _counter_script(
        f"import {module_name} as module",
        "assert module.run() == 36",
    )
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
        profile_script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    summary_path = work_dir / "jit-code-summary.jsonl"
    if summary_path.exists():
        summary_path.unlink()

    apply_script = _counter_script(
        f"import {module_name} as module",
        "assert module.run() == 36",
    )
    apply_result = _run_counter_project(
        project,
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
    script = _counter_script(
        f"import {module_name} as module",
        "assert module.run() == 5",
    )
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )
    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    apply_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
    )
    _assert_subprocess_ok(apply_result)


def test_apply_mode_lazy_diagonal_slice_runner_preserves_expected_result_binding(tmp_path):
    bench_dir = Path(__file__).resolve().parents[1] / "bench"
    work_dir = tmp_path / "soac-work"
    script = _counter_script(
        "import nqueens_slice_diagonal_set_consumers as module",
        'assert module.main(["nqueens_slice_diagonal_set_consumers.py", "4", "1"]) == 0',
    )
    base_env = _counter_env(work_dir=work_dir)
    project = _counter_project(
        tmp_path, {"nqueens_slice_diagonal_set_consumers": bench_dir / "nqueens_slice_diagonal_set_consumers.py", "nqueens_slice_support": bench_dir / "nqueens_slice_support.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    apply_result = _run_counter_project(
        project,
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
    script = _counter_script(
        f"import {module_name} as module",
        "assert module.run() == ((3, 4, 5), (True, 3, 4, 5), 12, (3, 9, 4, 10), 15)",
    )
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )

    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        timeout=60,
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    for opt_mode in ("verify", "apply"):
        specialized_result = _run_counter_project(
            project,
            script,
            env={**base_env, "SOAC_OPT_MODE": opt_mode},
            timeout=60,
        )
        _assert_subprocess_ok(specialized_result)


def test_profiled_full_nqueens_slice_preserves_results_mutations_and_ordinary_tracing(tmp_path):
    bench_dir = Path(__file__).resolve().parents[1] / "bench"
    work_dir = tmp_path / "soac-work"
    module_paths = {
        "nqueens_slice_full_nqueens_list_consumer": bench_dir / "nqueens_slice_full_nqueens_list_consumer.py",
        "nqueens_slice_support": bench_dir / "nqueens_slice_support.py",
    }
    script = _counter_script(
        "import nqueens_slice_full_nqueens_list_consumer as module",
        'assert module.main(["nqueens_slice_full_nqueens_list_consumer.py", "4", "1"]) == 0',
    )
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "eager"},
    )

    project = _counter_project(
        tmp_path, module_paths,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
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
        specialized_result = _run_counter_project(
            project,
            script,
            env={**base_env, "SOAC_OPT_MODE": opt_mode},
            timeout=60,
        )
        _assert_subprocess_ok(specialized_result)

    apply_bodies = [
        """
        expected_counts = [1, 1, 0, 0, 2, 10, 4, 40, 92]
        assert [
            module.full_nqueens_list_consumer(width)
            for width in range(len(expected_counts))
        ] == expected_counts
        assert module.full_nqueens_list_consumer(9) == 352
        assert module.full_nqueens_list_consumer(-1) == 0

        class IntSubclass(int):
            equality_calls = 0

            def __eq__(self, other):
                type(self).equality_calls += 1
                return False

        assert module.full_nqueens_list_consumer(IntSubclass(4)) == 0
        assert IntSubclass.equality_calls > 0
        """,
        """
        import sys

        frame_events = []

        def trace(frame, event, _arg):
            if event in ("call", "line") and frame.f_code.co_name in {
                "n_queens",
                "permutations",
            }:
                frame_events.append((frame.f_code.co_name, event))
            return trace

        sys.settrace(trace)
        try:
            assert list(module.n_queens(1)) == [(0,)]
            assert frame_events, "direct generator consumption must exercise the tracer"
            frame_events.clear()
            assert module.full_nqueens_list_consumer(4) == 2
        finally:
            sys.settrace(None)
        assert {"n_queens", "permutations"}.issubset(
            {function_name for function_name, _event in frame_events}
        ), frame_events
        assert {"call", "line"}.issubset(
            {event for _function_name, event in frame_events}
        ), frame_events
        """,
        """
        original_n_queens = module.n_queens

        def rebound_n_queens(_queen_count):
            yield "first"
            yield "second"
            yield "third"

        module.n_queens = rebound_n_queens
        try:
            assert module.full_nqueens_list_consumer(4) == 3
        finally:
            module.n_queens = original_n_queens
        assert module.full_nqueens_list_consumer(4) == 2
        """,
        """
        original_permutations = module.permutations

        def rebound_permutations(_iterable, r=None):
            yield (1, 3, 0, 2)

        module.permutations = rebound_permutations
        try:
            assert module.full_nqueens_list_consumer(4) == 1
        finally:
            module.permutations = original_permutations
        assert module.full_nqueens_list_consumer(4) == 2
        """,
        """
        original_defaults = module.permutations.__defaults__
        module.permutations.__defaults__ = (1,)
        try:
            try:
                module.full_nqueens_list_consumer(4)
            except IndexError:
                pass
            else:
                raise AssertionError("mutated omitted-r default did not take effect")
        finally:
            module.permutations.__defaults__ = original_defaults
        assert module.full_nqueens_list_consumer(4) == 2
        """,
        """
        original_defaults = module.permutations.__defaults__
        module.permutations.__defaults__ = (None, 1)
        try:
            try:
                module.full_nqueens_list_consumer(4)
            except IndexError:
                pass
            else:
                raise AssertionError("right-aligned mutated omitted-r default did not take effect")
        finally:
            module.permutations.__defaults__ = original_defaults
        assert module.full_nqueens_list_consumer(4) == 2
        """,
        """
        import ctypes

        get_vectorcall = ctypes.pythonapi.PyVectorcall_Function
        get_vectorcall.argtypes = [ctypes.py_object]
        get_vectorcall.restype = ctypes.c_void_p
        set_vectorcall = ctypes.pythonapi.PyFunction_SetVectorcall
        set_vectorcall.argtypes = [ctypes.py_object, ctypes.c_void_p]
        set_vectorcall.restype = None

        original_vectorcall = get_vectorcall(module.n_queens)
        assert original_vectorcall
        set_vectorcall(module.n_queens, None)
        try:
            try:
                module.full_nqueens_list_consumer(4)
            except TypeError:
                pass
            else:
                raise AssertionError("cleared n_queens vectorcall did not reach fallback")
        finally:
            set_vectorcall(module.n_queens, original_vectorcall)
        assert module.full_nqueens_list_consumer(4) == 2
        """,
    ]
    # All seven original observers still run against the unmarked source.
    # NULL via the public PyFunction_SetVectorcall setter changes semantics and
    # is outside the supported strict contract (STRICT_MODULES.md); it is not
    # a supported setter that promises strict mutation rejection.
    for body in apply_bodies:
        ordinary_result = _run_ordinary_subprocess(
            _ordinary_script(
                bench_dir,
                "import nqueens_slice_full_nqueens_list_consumer as module",
                body,
            ),
            env=_ordinary_subprocess_env(bench_dir),
            timeout=60,
        )
        _assert_subprocess_ok(ordinary_result)

    # The authenticated interpreter backend retains ordinary native trace
    # events. Run the original positive observer unchanged, not on a lookalike
    # unselected module; run_case proves original-code ownership and zero JIT.
    cpython_project = _counter_project(
        tmp_path / "cpython-tracing", module_paths, env={}, backend="cpython",
    )
    cpython_project.run_case(
        "nqueens_slice_full_nqueens_list_consumer",
        "def validate_module(module):\n" + textwrap.indent(
            textwrap.dedent(apply_bodies[1]).strip() + "\n", "    "
        ),
        Path(__file__),
        required_functions=("permutations", "n_queens", "full_nqueens_list_consumer"),
        backend="cpython",
    )

    strict_semantics_body = """
assert list(module.n_queens(1)) == [(0,)]
assert module.full_nqueens_list_consumer(4) == 2
"""
    strict_mutation_body = """
from soac import StrictMutationError

def function_state(function):
    return (
        function.__code__, function.__defaults__, function.__kwdefaults__,
        function.__closure__, function.__globals__,
    )

def assert_function_unchanged(function, before):
    assert all(left is right for left, right in zip(function_state(function), before))

def reject_global(name, replacement):
    original = getattr(module, name)
    before = function_state(original)
    try:
        setattr(module, name, replacement)
    except StrictMutationError:
        pass
    else:
        raise AssertionError("a sealed final function binding accepted replacement")
    assert getattr(module, name) is original
    assert vars(module)[name] is original
    assert_function_unchanged(original, before)
    assert module.full_nqueens_list_consumer(4) == 2

def rebound_n_queens(_queen_count):
    yield "first"
    yield "second"
    yield "third"

def rebound_permutations(_iterable, r=None):
    yield (1, 3, 0, 2)

assert module.full_nqueens_list_consumer(4) == 2
reject_global("n_queens", rebound_n_queens)
reject_global("permutations", rebound_permutations)

original = module.permutations
for replacement_defaults in ((1,), (None, 1)):
    before = function_state(original)
    try:
        original.__defaults__ = replacement_defaults
    except StrictMutationError:
        pass
    else:
        raise AssertionError("a sealed generator accepted defaults replacement")
    assert module.permutations is original
    assert_function_unchanged(original, before)
    assert module.full_nqueens_list_consumer(4) == 2
"""
    # Values and sealed-binding/default mutation barriers remain in scope for
    # untraced SOAC execution. Positive ordinary/native tracing is checked above;
    # neither SOAC observer events nor observer refusal are required.
    for body in [apply_bodies[0], strict_semantics_body + strict_mutation_body]:
        apply_script = _counter_script(
            "import nqueens_slice_full_nqueens_list_consumer as module",
            body,
        )
        apply_result = _run_counter_project(
            project,
            apply_script,
            env={**base_env, "SOAC_OPT_MODE": "apply"},
            # The unchanged width-9 result/subclass workload completes in about
            # 98 seconds on the bounded validation VM; 60 seconds cut it off.
            timeout=180,
        )
        _assert_subprocess_ok(apply_result)


def test_profiled_pyperformance_nqueens_preserves_rebinding_and_ordinary_tracing(
    tmp_path,
):
    spec = importlib.util.find_spec("benchmarks.bm_nqueens.run_benchmark")
    assert spec is not None and spec.origin is not None
    benchmark_source = Path(spec.origin)
    benchmark_root = tmp_path / "benchmarks" / "bm_nqueens"
    benchmark_root.mkdir(parents=True)
    benchmark_path = benchmark_root / "run_benchmark.py"
    benchmark_path.write_bytes(benchmark_source.read_bytes())
    profile_script = _counter_script(
        "import run_benchmark as module",
        "assert module.bench_n_queens(8) is None",
    )
    ordinary_apply_script = _ordinary_script(
        benchmark_root,
        "import run_benchmark as module",
        """
        frame_events = []

        def trace(frame, event, _arg):
            if event in ("call", "line") and frame.f_code.co_name in {
                "n_queens",
                "permutations",
            }:
                frame_events.append((frame.f_code.co_name, event))
            return trace

        sys.settrace(trace)
        try:
            assert list(module.n_queens(1)) == [(0,)]
            assert frame_events, "direct generator consumption must exercise the tracer"
            frame_events.clear()
            assert module.bench_n_queens(4) is None
        finally:
            sys.settrace(None)
        assert {"n_queens", "permutations"}.issubset(
            {function_name for function_name, _event in frame_events}
        ), frame_events
        assert {"call", "line"}.issubset(
            {event for _function_name, event in frame_events}
        ), frame_events

        original_n_queens = module.n_queens
        fallback_calls = []

        def rebound_n_queens(width):
            fallback_calls.append(width)
            yield (0, 1, 2, 3)

        module.n_queens = rebound_n_queens
        try:
            assert module.bench_n_queens(8) is None
        finally:
            module.n_queens = original_n_queens
        assert fallback_calls == [8]
        """,
    )
    _assert_subprocess_ok(_run_ordinary_subprocess(
        ordinary_apply_script,
        env=_ordinary_subprocess_env(benchmark_root),
        timeout=60,
    ))
    tracing_body = """
        frame_events = []

        def trace(frame, event, _arg):
            if event in ("call", "line") and frame.f_code.co_name in {
                "n_queens",
                "permutations",
            }:
                frame_events.append((frame.f_code.co_name, event))
            return trace

        sys.settrace(trace)
        try:
            assert list(module.n_queens(1)) == [(0,)]
            assert frame_events, "direct generator consumption must exercise the tracer"
            frame_events.clear()
            assert module.bench_n_queens(4) is None
        finally:
            sys.settrace(None)
        assert {"n_queens", "permutations"}.issubset(
            {function_name for function_name, _event in frame_events}
        ), frame_events
        assert {"call", "line"}.issubset(
            {event for _function_name, event in frame_events}
        ), frame_events
        """

    strict_semantics_body = """
        assert list(module.n_queens(1)) == [(0,)]
        assert module.bench_n_queens(4) is None
        """
    strict_mutation_body = """
        from soac import StrictMutationError

        original_n_queens = module.n_queens
        original_code = original_n_queens.__code__
        original_defaults = original_n_queens.__defaults__
        original_kwdefaults = original_n_queens.__kwdefaults__
        fallback_calls = []

        def rebound_n_queens(width):
            fallback_calls.append(width)
            yield (0, 1, 2, 3)

        try:
            module.n_queens = rebound_n_queens
        except StrictMutationError:
            pass
        else:
            raise AssertionError("a sealed final generator binding accepted replacement")
        assert module.n_queens is original_n_queens
        assert vars(module)["n_queens"] is original_n_queens
        assert original_n_queens.__code__ is original_code
        assert original_n_queens.__defaults__ is original_defaults
        assert original_n_queens.__kwdefaults__ is original_kwdefaults
        assert fallback_calls == []
        assert module.bench_n_queens(8) is None
        assert fallback_calls == []
        """
    apply_script = _counter_script(
        "import run_benchmark as module",
        strict_semantics_body + strict_mutation_body,
    )
    work_dir = tmp_path / "soac-work"
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        },
    )
    project = _counter_project(
        tmp_path, {"run_benchmark": benchmark_path},
        env={
            **base_env,
            "SOAC_OPT_MODE": "profile",
        },
    )
    profile_result = _run_counter_project(
        project,
        profile_script,
        env={
            **base_env,
            "SOAC_OPT_MODE": "profile",
        },
        timeout=60,
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    # Preserve the complete original positive trace observer under its native
    # interpreter contract, with a separate authenticated publication.
    cpython_project = _counter_project(
        tmp_path / "cpython-tracing", {"run_benchmark": benchmark_path},
        env={}, backend="cpython",
    )
    cpython_project.run_case(
        "run_benchmark",
        "def validate_module(module):\n    import sys\n" + textwrap.indent(
            textwrap.dedent(tracing_body).strip() + "\n", "    "
        ),
        Path(__file__),
        required_functions=("permutations", "n_queens", "bench_n_queens"),
        backend="cpython",
    )

    apply_result = _run_counter_project(
        project,
        apply_script,
        env={
            **base_env,
            "SOAC_OPT_MODE": "apply",
        },
        timeout=60,
    )
    _assert_subprocess_ok(apply_result)


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
    script = _counter_script(
        f"import {module_name} as module",
        'assert module.main(["callback_pair_inline_genexpr_binding_case.py", "4", "1"]) == 0',
    )
    base_env = _counter_env(work_dir=work_dir)
    project = _counter_project(
        tmp_path, {support_module_name: tmp_path / f"{support_module_name}.py", module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    _assert_subprocess_ok(profile_result)
    assert (work_dir / "profile.bin").exists()

    apply_result = _run_counter_project(
        project,
        script,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
    )
    _assert_subprocess_ok(apply_result)


def test_profile_eager_runtime_import_does_not_compile_runtime_entries(tmp_path):
    log_path = tmp_path / "runtime-import-events.jsonl"
    result = _run_ordinary_subprocess(
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
        env=_ordinary_subprocess_env(
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


def test_strict_import_ignores_unsigned_pre_optimization_blockpy_cache(tmp_path):
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
    env = _counter_env(
        work_dir=work_dir,
        extra_env={
            "SOAC_LOG": f"soac_blockpy_module_cache=info;json={log_path}",
        },
    )
    script = _counter_script(
        "import module_cache_case",
        "assert module_cache_case.value() == 42",
    )

    project = _counter_project(
        tmp_path, {"module_cache_case": module_path},
        env=env,
    )
    # Neither cache subtree is strict authority. Keep poisoned cache bytes in
    # both locations so this also covers fixtures outside the repository root.
    cache_files = [
        cache_dir / subtree / "module_cache_case" / "mod.blockpy"
        for subtree in ("project", "python-stdlib")
    ]
    unsigned_cache = b"unsigned executable cache must never be read or repaired"
    for cache_path in cache_files:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        cache_path.write_bytes(unsigned_cache)
    first = _run_counter_project(project, script, env=env)
    _assert_subprocess_ok(first)
    second = _run_counter_project(project, script, env=env)
    _assert_subprocess_ok(second)

    assert set(cache_dir.rglob("mod.blockpy")) == set(cache_files)
    assert all(path.read_bytes() == unsigned_cache for path in cache_files)
    rows = _read_jsonl(log_path) if log_path.exists() else []
    assert not any(
        row.get("event") in {"soac.blockpy_module_cache", "soac.blockpy_module_cache_store"}
        for row in rows
    )


def test_strict_import_does_not_publish_unsigned_module_artifacts(tmp_path):
    work_dir = tmp_path / "soac-work"
    module_path = tmp_path / "work_dir_cache_case.py"
    module_path.write_text("def read():\n    return 17\n", encoding="utf-8")

    project = _counter_project(
        tmp_path, {"work_dir_cache_case": module_path},
        env=_counter_env(
            work_dir=work_dir,
        ),
    )
    result = _run_counter_project(
        project,
        _counter_script(
            "import work_dir_cache_case",
            "assert work_dir_cache_case.read() == 17",
        ),
        env=_counter_env(
            work_dir=work_dir,
        ),
    )
    _assert_subprocess_ok(result)

    # Ordinary compiler cache routing/reuse is covered in soac_driver. An
    # authenticated import lowers verified source, not writable serialized IR.
    assert not list((work_dir / "modules").rglob("mod.blockpy"))


def test_soac_work_dir_is_default_event_log_dir(tmp_path):
    work_dir = tmp_path / "soac-work"
    log_path = work_dir / "events.jsonl"
    module_path = tmp_path / "work_dir_log_case.py"
    module_path.write_text("def read():\n    return 11\n", encoding="utf-8")
    project = _counter_project(
        tmp_path, {"work_dir_log_case": module_path},
        env=_counter_env(work_dir=work_dir),
    )
    result = _run_counter_project(
        project,
        _counter_script(
            "import work_dir_log_case",
            "assert work_dir_log_case.read() == 11",
        ),
        env=_counter_env(work_dir=work_dir),
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

    result = _run_counter_project(
        profiled_specialization_runtime_case["project"],
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

    result = _run_counter_project(
        profiled_specialization_runtime_case["project"],
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
    script = _counter_script(
        "import getitem_specialization_case",
        "assert getitem_specialization_case.run_case() == 182",
    )
    base_env = _counter_env(work_dir=work_dir)

    project = _counter_project(
        tmp_path, {"getitem_specialization_case": module_path},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
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

    verify_result = _run_counter_project(
        project,
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
    script = _counter_script(
        "import setitem_specialization_case",
        "assert setitem_specialization_case.run_case() == 281",
    )
    base_env = _counter_env(work_dir=work_dir)

    project = _counter_project(
        tmp_path, {"setitem_specialization_case": module_path},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
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

    verify_result = _run_counter_project(
        project,
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

    script = _counter_script(
        "import field_user_case",
        textwrap.dedent(
            """
            for _ in range(50):
                assert field_user_case.read_fields() == 84
                assert field_user_case.write_field() == 84
            import _testinternalcapi
            # The indexed_hit counter can describe a guarded ordinary split-dict path.
            assert not _testinternalcapi.dict_has_indexed_keys(vars(field_user_case.point))
            assert not _testinternalcapi.dict_has_indexed_keys(vars(field_user_case.vector))
            """
        ).strip(),
    )
    base_env = _counter_env(work_dir=work_dir)

    project = _counter_project(
        tmp_path, {"field_owner_case": owner_path, "field_user_case": user_path},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
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

    verify_result = _run_counter_project(
        project,
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
    # Preserve the positive guarded-path counters below; their label does not
    # assert indexed-key storage on this admitted source class.
    script = _counter_script(
        f"import {module_name} as module",
        "for _ in range(50):\n    assert module.run() == 8"
        "\nimport _testinternalcapi"
        "\nassert not _testinternalcapi.dict_has_indexed_keys(vars(module.Record(1, 2)))",
    )
    base_env = _counter_env(
        work_dir=work_dir,
        extra_env={"SOAC_COMPILE_MODE": "lazy", "SOAC_BACKGROUND_JIT": "0"},
    )

    project = _counter_project(
        tmp_path, {module_name: tmp_path / f"{module_name}.py"},
        env={**base_env, "SOAC_OPT_MODE": "profile"},
    )
    profile_result = _run_counter_project(
        project,
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

    verify_result = _run_counter_project(
        project,
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
