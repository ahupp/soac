"""Source equivalence and immutable provenance for strict/stock comparisons."""

import ast
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tokenize
from pathlib import Path

import pytest
import tomllib


@pytest.fixture
def source_tools():
    path = (
        Path(__file__).resolve().parents[1]
        / "scripts"
        / "strict_pyperformance_sources.py"
    )
    spec = importlib.util.spec_from_file_location(
        "strict_pyperformance_sources_test", path
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize(
    "source, expected_line",
    [
        (b"result = 1\n", 1),
        (b'"""module docstring"""\nresult = 1\n', 1),
        (b'"""doc"""\nfrom __future__ import annotations\nresult = 1', 1),
        (b"#!/usr/bin/env python3\n# coding: latin-1\nlabel = '\xe9'\n", 3),
        (b"# coding: utf-8\r\nresult = 1\r\n", 2),
        (b"# Original CR-only header\rresult = 1\r", 2),
        (b"from __future__ import annotations", 1),
        (b'"""module docstring"""; result = 1\n', 1),
        (b"from __future__ import annotations; result = 1\n", 1),
        (b'"""doc"""; from __future__ import annotations; result = 1\n', 1),
        ('"""doc é"""; result = 1\r\n'.encode("utf-8"), 1),
        (b'\xef\xbb\xbf"""BOM and docstring"""\nresult = 1\n', 1),
        (b"# comment without a final newline", 2),
        (b"#!/usr/bin/env python3", 2),
        (b"", 1),
    ],
)
def test_strict_opt_in_preserves_source_semantics_and_encoding(
    source_tools, source, expected_line
):
    result, insertion = source_tools.strict_opt_in(source, "benchmark.py")
    assert insertion == expected_line
    # The added comment must not change real future flags or statement order.
    assert compile(result, "benchmark.py", "exec", dont_inherit=True).co_flags == compile(
        source, "benchmark.py", "exec", dont_inherit=True
    ).co_flags
    original = ast.parse(source)
    candidate = ast.parse(result)
    assert ast.dump(candidate, include_attributes=False) == ast.dump(
        original, include_attributes=False
    )
    comments = [
        token
        for token in tokenize.generate_tokens(
            io.StringIO(
                result.decode(tokenize.detect_encoding(io.BytesIO(result).readline)[0]),
                newline=None,
            ).readline
        )
        if token.type == tokenize.COMMENT
        and token.string == source_tools.SOURCE_DECLARATION
    ]
    assert len(comments) == 1 and comments[0].start[0] == insertion
    newline = b"\r\n" if b"\r\n" in source else b"\r" if b"\r" in source else b"\n"
    unchanged = result.replace(
        source_tools.SOURCE_DECLARATION.encode() + newline, b"", 1
    )
    # A header-only file without an EOL needs one separator before the comment.
    expected = (
        source + newline
        if source and not original.body and not source.endswith((b"\n", b"\r"))
        else source
    )
    assert unchanged == expected
    if b"\xe9" in source:
        assert b"\xe9" in result
        assert result.splitlines()[1] == b"# coding: latin-1"
    if source.startswith(b"\xef\xbb\xbf"):
        assert result.startswith(b"\xef\xbb\xbf")


@pytest.mark.parametrize(
    "source",
    [
        b"# soac: module(checked_attr=false)\nclass C: pass\n",
        b"# soac: class(checked_attr=false)\nclass C: pass\n",
        b"# soac module(checked_attr=true)\nclass C: pass\n",
        b"# CR-only header\r# soac: module(checked_attr=true)\rclass C: pass\r",
    ],
)
def test_strict_opt_in_rejects_existing_source_policy(source_tools, source):
    with pytest.raises(ValueError, match="already declares SOAC policy"):
        source_tools.strict_opt_in(source, "module.py")


@pytest.mark.parametrize(
    "source",
    [
        b"from __future__ import strict\nclass C: pass\n",
        b"'# soac: module(strict_assign=true, checked_attr=true)'\nclass C: pass\n",
    ],
)
def test_strict_opt_in_does_not_infer_authority_from_futures_or_strings(
    source_tools, source
):
    candidate, _ = source_tools.strict_opt_in(source, "module.py")
    assert (
        candidate.replace(source_tools.SOURCE_DECLARATION.encode() + b"\n", b"", 1)
        == source
    )
    assert ast.dump(ast.parse(candidate), include_attributes=False) == ast.dump(
        ast.parse(source), include_attributes=False
    )


def _benchmark(tmp_path):
    root = tmp_path / "stock"
    root.mkdir()
    script = root / "run_benchmark.py"
    script.write_text(
        '"""same workload"""\nfrom helper import calculate\nimport pyperf\nif __name__ == "__main__":\n    runner = pyperf.Runner()\n    runner.bench_func("sample", calculate, 3)\n'
    )
    (root / "helper.py").write_text("def calculate(value):\n    return value * value\n")
    (root / "input.dat").write_bytes(b"\0unchanged benchmark data\xff")
    return script


def test_strict_overlay_records_opt_in_and_has_path_independent_comparison_identity(
    source_tools, tmp_path
):
    script = _benchmark(tmp_path)
    original = script.read_bytes()
    first = source_tools.prepare_source_overlay(script, tmp_path / "first")
    second = source_tools.prepare_source_overlay(script, tmp_path / "second")
    assert first["source_fingerprint"] == second["source_fingerprint"]
    assert first["overlay_fingerprint"] != second["overlay_fingerprint"]
    assert first["modules"] == {"__main__": "run_benchmark.py", "helper": "helper.py"}
    assert script.read_bytes() == original
    assert source_tools.verify_source_overlay(tmp_path / "first") == first
    assert not (Path(first["project"]) / "pyproject.toml").exists()
    assert first["configuration_sha256"] is None
    assert first["configuration_provenance"]["presence"] == "absent"
    copied = Path(first["project"]) / "input.dat"
    assert copied.read_bytes() == (script.parent / "input.dat").read_bytes()


@pytest.mark.parametrize(
    "changed", ["stock", "strict", "policy", "inventory", "harness"]
)
def test_strict_overlay_rejects_stale_or_altered_comparison_inputs(
    source_tools, tmp_path, changed
):
    script = _benchmark(tmp_path)
    output = tmp_path / "strict"
    manifest = source_tools.prepare_source_overlay(script, output)
    project = Path(manifest["project"])
    if changed == "stock":
        script.write_text("result = 999\n")
    elif changed == "strict":
        (project / "helper.py").write_text("def calculate(value):\n    return 999\n")
    elif changed == "policy":
        (project / "pyproject.toml").write_text("[tool.soac.strict]\ninclude = []\n")
    elif changed == "inventory":
        (project / "extra.py").write_text("extra = True\n")
    else:
        Path(manifest["harness_script"]).write_text(
            "raise AssertionError('changed harness')\n"
        )
    with pytest.raises(ValueError):
        source_tools.verify_source_overlay(output)


def test_strict_overlay_rejects_existing_source_links_and_authority_confusion(
    source_tools, tmp_path
):
    script = _benchmark(tmp_path)
    script.write_text("# soac: module(strict_assign=true, checked_attr=true)\n")
    output = tmp_path / "strict"
    with pytest.raises(ValueError, match="already declares SOAC policy"):
        source_tools.prepare_source_overlay(script, output)
    assert not output.exists()
    script.write_text("result = 1\n")
    link = script.parent / "linked.py"
    link.symlink_to(script)
    with pytest.raises(ValueError, match="symlinks"):
        source_tools.prepare_source_overlay(script, output)
    assert not output.exists()
    link.unlink()
    output.mkdir()
    marker = output / "owned-by-another-run"
    marker.write_text("preserve")
    with pytest.raises(ValueError, match="existing strict overlay"):
        source_tools.prepare_source_overlay(script, output)
    assert marker.read_text() == "preserve"


def test_strict_overlay_follows_local_imports_without_rewriting_python_input_data(
    source_tools, tmp_path
):
    script = _benchmark(tmp_path)
    script.write_text(
        'from package import implementation\nimport pyperf\nif __name__ == "__main__":\n    runner = pyperf.Runner()\n    runner.bench_func("sample", implementation.calculate)\n'
    )
    package = script.parent / "package"
    package.mkdir()
    (package / "__init__.py").write_text("from . import common\n")
    (package / "common.py").write_text("value = 3\n")
    (package / "implementation.py").write_text("from .common import value\n")
    # Intentionally invalid Python used as parser input is not a module merely
    # because its filename has the .py suffix.
    data = script.parent / "parser_input.py"
    data.write_bytes(b"def invalid syntax input\n")
    output = tmp_path / "strict"
    manifest = source_tools.prepare_source_overlay(script, output)
    assert manifest["modules"] == {
        "__main__": "run_benchmark.py",
        "package": "package/__init__.py",
        "package.common": "package/common.py",
        "package.implementation": "package/implementation.py",
    }
    assert (
        Path(manifest["project"]) / "parser_input.py"
    ).read_bytes() == data.read_bytes()
    assert (Path(manifest["project"]) / "helper.py").read_bytes() == (
        script.parent / "helper.py"
    ).read_bytes()
    assert source_tools.verify_source_overlay(output) == manifest


@pytest.mark.parametrize("newline, encoding", [("\n", "utf-8"), ("\r\n", "latin-1")])
def test_driver_projection_keeps_setup_before_seal_and_measurement_after(
    source_tools, newline, encoding
):
    source = (
        '''# coding: ENCODING
"""unchanged workload é"""
from __future__ import annotations
events = []
def workload(value):
    events.append(("measure", shared, value))
    return shared + value
class Runner:
    def bench_func(self, name, function, *args):
        return function(*args)
if __name__ == "__main__":
    events.append(("setup",))
    shared = 40
    runner = Runner()
    measured = runner.bench_func("sample", workload, 2)
'''.replace("ENCODING", encoding)
        .replace("\n", newline)
        .encode(encoding)
    )
    prefix, harness, projection = source_tools.project_driver_harness(
        source, "driver.py"
    )
    stock = {"__name__": "__main__"}
    exec(compile(source, "driver.py", "exec", dont_inherit=True), stock)
    strict_globals = {"__name__": "__main__"}
    exec(compile(prefix, "driver.py", "exec", dont_inherit=True), strict_globals)
    assert strict_globals["events"] == [("setup",)]
    assert "measured" not in strict_globals
    harness_locals = dict(strict_globals)
    exec(
        compile(
            harness,
            "driver.py",
            "exec",
            flags=projection["compiler_flags"],
            dont_inherit=True,
        ),
        harness_locals,
    )
    assert strict_globals["events"] == stock["events"]
    assert harness_locals["measured"] == stock["measured"] == 42
    assert "measured" not in strict_globals
    assert projection["policy"] == source_tools.HARNESS_POLICY


def test_driver_projection_follows_measurement_function_calls(source_tools):
    source = b"""def workload():
    return 42
def main(runner):
    runner.bench_func("sample", workload)
def invoke(runner):
    main(runner)
if __name__ == "__main__":
    runner = make_runner()
    invoke(runner)
"""
    prefix, harness, projection = source_tools.project_driver_harness(
        source, "driver.py"
    )
    assert len(ast.parse(prefix).body[-1].body) == 1
    assert len(ast.parse(harness).body[-1].body) == 1
    assert projection["suffix_statement_index"] == 1


@pytest.mark.parametrize(
    "source, reason",
    [
        (b"runner.bench_func('sample', workload)\n", "terminal __main__ guard"),
        (
            b"if __name__ == '__main__': runner.bench_func('sample', workload)\n",
            "multiline body",
        ),
        (b"if __name__ == '__main__':\n    invoke_unknown()\n", "statically selected"),
        (
            b"def workload():\n    return shared\nif __name__ == '__main__':\n    runner.bench_func('sample', workload)\n    shared = 2\n",
            "rebinds workload globals: shared",
        ),
        (
            b"if __name__ == '__main__':\n    runner.bench_func('sample', workload)\n    globals()['shared'] = 2\n",
            "reflects on its namespace",
        ),
        (
            b"if __name__ == '__main__':\n    runner = make_runner(); runner.bench_func('sample', workload)\n",
            "shares a setup line",
        ),
    ],
)
def test_driver_projection_rejects_unsupported_or_semantically_changed_harness(
    source_tools, source, reason
):
    with pytest.raises(ValueError, match=reason):
        source_tools.project_driver_harness(source, "driver.py")


def test_driver_projection_preserves_an_empty_setup_block(source_tools):
    prefix, harness, projection = source_tools.project_driver_harness(
        b"if __name__ == '__main__':\n    runner.bench_func('sample', workload)\n",
        "driver.py",
    )
    assert isinstance(ast.parse(prefix).body[-1].body[0], ast.Pass)
    assert isinstance(ast.parse(harness).body[-1].body[0], ast.Expr)
    assert projection["suffix_statement_index"] == 0


def test_driver_projection_preserves_readonly_name_based_dispatch(source_tools):
    source = b"""def workload():
    return 42
class Runner:
    def bench_func(self, name, function):
        results.append(function())
results = []
if __name__ == "__main__":
    runner = Runner()
    for name in ["workload"]:
        function = globals()[name]
        runner.bench_func(name, function)
"""
    prefix, harness, projection = source_tools.project_driver_harness(
        source, "driver.py"
    )
    module_globals = {"__name__": "__main__"}
    exec(prefix, module_globals)
    ordinary_harness_globals = dict(module_globals)
    exec(
        compile(
            harness,
            "driver.py",
            "exec",
            flags=projection["compiler_flags"],
            dont_inherit=True,
        ),
        ordinary_harness_globals,
    )
    assert module_globals["results"] == [42]
    assert "name" not in module_globals


def _publication_fixture(source_tools, tmp_path, *, exit_code=0):
    script = _benchmark(tmp_path)
    native = tmp_path / "native-python"
    native.touch()
    selected = tmp_path / "benchmark-venv" / "bin" / "python"
    selected.parent.mkdir(parents=True)
    selected.symlink_to(native)
    calls = []

    def run(command, **kwargs):
        calls.append((command, kwargs))
        deployment = Path(command[command.index("--deployment") + 1])
        if not exit_code:
            deployment.write_text('{"unit_fixture": "not runtime authority"}\n')
        return subprocess.CompletedProcess(
            command,
            exit_code,
            json.dumps({"modules": 2, "generation": "fixture"}),
            "unit checker failure" if exit_code else "",
        )

    def prepare():
        return source_tools.prepare_strict_benchmark(
            script,
            selected,
            tmp_path / "bundle",
            tmp_path / "checker",
            {"HOME": "/guest/home", "PATH": "/guest/bin"},
            run=run,
        )

    return script, selected, calls, prepare


def test_offline_benchmark_preparation_uses_actual_venv_before_any_worker(
    source_tools, tmp_path
):
    script, selected, calls, prepare = _publication_fixture(source_tools, tmp_path)
    execution = prepare()
    command, kwargs = calls[0]
    assert command[command.index("--python") + 1] == str(selected)
    assert command[command.index("--python") + 1] != str(selected.resolve())
    assert "__main__=run_benchmark.py" in command
    assert "helper=helper.py" in command
    assert kwargs["env"] == {"HOME": "/guest/home", "PATH": "/guest/bin"}
    assert str(script) not in command  # Source is analyzed, not executed.
    key = Path(command[command.index("--signing-key") + 1])
    assert key.stat().st_size == 32 and key.stat().st_mode & 0o077 == 0
    assert not key.is_relative_to(Path(execution["source"]["project"]))
    manifest = Path(execution["manifest_path"])
    assert source_tools.verify_strict_benchmark(manifest, selected) == execution
    original_key = key.read_bytes()
    assert prepare()["source_fingerprint"] == execution["source_fingerprint"]
    assert key.read_bytes() == original_key


def test_failed_offline_analysis_never_publishes_worker_selection(
    source_tools, tmp_path
):
    _, _, calls, prepare = _publication_fixture(source_tools, tmp_path, exit_code=1)
    with pytest.raises(RuntimeError, match="analysis failed"):
        prepare()
    assert len(calls) == 1
    assert not (tmp_path / "bundle" / "execution.json").exists()


@pytest.mark.parametrize(
    "changed", ["python", "deployment", "source", "source_policy", "schema"]
)
def test_worker_selection_rejects_changed_offline_inputs(
    source_tools, tmp_path, changed
):
    script, selected, _, prepare = _publication_fixture(source_tools, tmp_path)
    execution = prepare()
    if changed == "python":
        selected = selected.resolve()
    elif changed == "deployment":
        Path(execution["deployment"]).write_text("{}\n")
    elif changed == "source":
        script.write_text("raise AssertionError('changed input')\n")
    else:
        manifest_path = Path(execution["manifest_path"])
        manifest = json.loads(manifest_path.read_text())
        manifest[changed] = (
            "legacy-future-selection"
            if changed == "source_policy"
            else source_tools.EXECUTION_SCHEMA - 1
        )
        manifest_path.write_text(json.dumps(manifest))
    with pytest.raises(ValueError):
        source_tools.verify_strict_benchmark(Path(execution["manifest_path"]), selected)


def test_real_offline_checker_to_sealed_pyperf_profile_and_apply_worker(
    source_tools, tmp_path
):
    from soac import _soac_ext

    from tests._strict_integration import ROOT, _checker

    stock = tmp_path / "stock"
    stock.mkdir()
    script = stock / "run_benchmark.py"
    script.write_text("""import pyperf
class Record:
    def __init__(self, value):
        self.value = value
    def read(self):
        return self.value
def workload(record):
    assert record.read() == 41
    return record.value + 1
if __name__ == "__main__":
    runner = pyperf.Runner()
    record = Record(41)
    runner.bench_func("strict_contract_smoke", workload, record)
""")
    environment = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("SOAC_PYPERFORMANCE_")
    }
    # Analysis observes interpreter search-path configuration. The worker
    # adapter must already be selected here, as it is in the real recipe;
    # changing PYTHONPATH after publication must remain an admission error.
    environment["PYTHONPATH"] = os.pathsep.join(
        [
            str(ROOT / "scripts" / "pyperformance_soac_sitecustomize"),
            str(ROOT / "soac_py" / "src"),
            str(Path(_soac_ext.__file__).parent),
        ]
    )
    execution = source_tools.prepare_strict_benchmark(
        script,
        Path(sys.executable),
        tmp_path / "bundle",
        _checker(),
        environment,
    )
    work_root = tmp_path / "worker-output"
    environment.update(
        SOAC_PYPERFORMANCE_ENABLE="1",
        SOAC_PYPERFORMANCE_STRICT_BUNDLE=execution["manifest_path"],
        SOAC_PYPERFORMANCE_WORK_ROOT=str(work_root),
        SOAC_WORK_DIR=str(work_root),
        SOAC_COMPILE_MODE="eager",
        SOAC_BACKGROUND_JIT="0",
        PYPERFORMANCE_RUNID="strict-contract-worker-smoke",
    )
    for mode in ("profile", "apply"):
        result = subprocess.run(
            [
                sys.executable,
                "-B",
                str(script),
                "--worker",
                "--worker-task=0",
                "--loops=1",
                "--values=1",
                "--warmups=0",
                "--pipe=1",
            ],
            env={**environment, "SOAC_OPT_MODE": mode},
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=180,
        )
        (tmp_path / f"{mode}.stdout.log").write_text(result.stdout)
        (tmp_path / f"{mode}.stderr.log").write_text(result.stderr)
        assert result.returncode == 0, f"{mode}: {result.stdout}\n{result.stderr}"
        assert json.loads(result.stdout)["benchmarks"]
    rows = [
        json.loads(line)
        for path in work_root.rglob("pyperformance-worker-timing.jsonl")
        for line in path.read_text().splitlines()
    ]
    assert {row["opt_mode"] for row in rows} == {"profile", "apply"}
    assert all(row["pyperf_benchmark_name"] == "strict_contract_smoke" for row in rows)
    assert all(
        row["language"] == "strict" and row["measured_batches"] == 1 for row in rows
    )
    for row in rows:
        main = next(
            state
            for state in row["sealed_strict_modules"]
            if state["module_name"] == "__main__"
        )
        assert main["schema"] == 2
        assert main["ready"] is True
        assert main["strict_assign"] is True
        assert main["checked_attr"] is True
        assert main["sealed"] is True
        assert main["artifact_generation"] == execution["publication"]["generation"]
        assert main["source_path"] == execution["source"]["strict_script"]
    native_records = [
        json.loads(line)
        for path in work_root.rglob("jit-code-summary.jsonl")
        for line in path.read_text().splitlines()
    ]
    assert any(
        row["function_qualname"] == "workload" and row["code_size"] > 0
        for row in native_records
    )


@pytest.mark.parametrize("newline", (b"\n", b"\r\n"))
def test_strict_overlay_preserves_upstream_configuration_and_discloses_source_policy(
    source_tools, tmp_path, newline
):
    script = _benchmark(tmp_path)
    upstream = newline.join(
        (
            b"# Upstream package metadata is an input, not disposable policy.",
            b"[project]",
            b'name = "original-benchmark"',
            b'version = "1.2.3"',
            b'dependencies = ["unchanged-package>=4"]',
            b"",
            b"[tool.pyperformance]",
            b'name = "sample"',
            b'tags = ["math", "unchanged"]',
            b"",
            b"[tool.other]",
            b'literal = "[tool.soac.strict] is data, not a table header"',
        )
    )
    original = script.parent / "pyproject.toml"
    original.write_bytes(upstream)
    ty_config = newline.join(
        (
            b"# Keep ty configuration bytes",
            b"[environment]",
            b'python-version = "3.15"',
        )
    )
    (script.parent / "ty.toml").write_bytes(ty_config)
    first = source_tools.prepare_source_overlay(script, tmp_path / "first")
    second = source_tools.prepare_source_overlay(script, tmp_path / "second")
    candidate = (Path(first["project"]) / "pyproject.toml").read_bytes()
    assert original.read_bytes() == upstream
    assert candidate == upstream
    assert tomllib.loads(candidate.decode()) == tomllib.loads(upstream.decode())
    assert (Path(first["project"]) / "ty.toml").read_bytes() == ty_config
    record = next(
        record
        for record in first["files"]
        if record["relative_path"] == "pyproject.toml"
    )
    assert record["module_name"] is None
    assert record["strict_directive_line"] is None
    assert record["stock_sha256"] == hashlib.sha256(upstream).hexdigest()
    assert record["strict_sha256"] == hashlib.sha256(candidate).hexdigest()
    assert first["configuration_sha256"] == record["strict_sha256"]
    provenance = first["configuration_provenance"]
    assert provenance["upstream_sha256"] == record["stock_sha256"]
    assert provenance["presence"] == "preserved"
    assert first["source_policy"] == source_tools.SOURCE_POLICY
    assert first["source_fingerprint"] == second["source_fingerprint"]
    assert source_tools.verify_source_overlay(tmp_path / "first") == first


@pytest.mark.parametrize(
    "upstream",
    [
        b'# Preserve an inline namespace without extending it.\ntool = {other = "upstream-data"}\n',
        b'[tool]\nsoac = "upstream-data"\n',
        b'[tool.ty.environment]\npython-version = "3.15"\n',
    ],
)
def test_strict_overlay_preserves_unrelated_configuration_without_reserializing(
    source_tools, tmp_path, upstream
):
    script = _benchmark(tmp_path)
    (script.parent / "pyproject.toml").write_bytes(upstream)
    output = tmp_path / "existing"
    manifest = source_tools.prepare_source_overlay(script, output)
    assert (Path(manifest["project"]) / "pyproject.toml").read_bytes() == upstream
    assert manifest["configuration_provenance"]["presence"] == "preserved"
    assert (
        manifest["configuration_provenance"]["upstream_sha256"]
        == hashlib.sha256(upstream).hexdigest()
    )
    assert source_tools.verify_source_overlay(output) == manifest


@pytest.mark.parametrize(
    "upstream",
    (
        b'[tool.soac.strict]\ninclude = ["helper.py", "run_benchmark.py"]\ndefault_class_policy = "automatic"\nunsupported_class_policy = "dynamic"\nchecked_fields = "disabled"\n',
        b'[tool.soac.strict]\nchecked_fields = "supported_annotations"\n',
        b'[[tool.soac.strict.overrides]]\ninclude = ["helper.py"]\n',
        b'tool = {soac = {strict = {}}}\n',
        b'[tool."soac".strict]\n',
    ),
)
def test_strict_overlay_rejects_retired_config_policy_even_when_previously_equivalent(
    source_tools, tmp_path, upstream
):
    script = _benchmark(tmp_path)
    original = script.parent / "pyproject.toml"
    original.write_bytes(upstream)
    output = tmp_path / "conflict"
    with pytest.raises(ValueError, match="retired"):
        source_tools.prepare_source_overlay(script, output)
    assert original.read_bytes() == upstream
    assert not output.exists()


@pytest.mark.parametrize("upstream", [b"[unterminated", b"\xff"])
def test_strict_overlay_rejects_invalid_upstream_configuration_without_overwrite(
    source_tools, tmp_path, upstream
):
    script = _benchmark(tmp_path)
    original = script.parent / "pyproject.toml"
    original.write_bytes(upstream)
    output = tmp_path / "invalid"
    with pytest.raises(ValueError, match="valid UTF-8 TOML"):
        source_tools.prepare_source_overlay(script, output)
    assert original.read_bytes() == upstream
    assert not output.exists()


@pytest.mark.parametrize(
    "changed",
    ("metadata", "policy", "disclosure", "source_policy", "directive_line", "schema"),
)
def test_strict_overlay_verifies_source_and_configuration_provenance_after_rehashing(
    source_tools, tmp_path, changed
):
    script = _benchmark(tmp_path)
    (script.parent / "pyproject.toml").write_bytes(b'[project]\nname = "original"\n')
    output = tmp_path / "strict"
    manifest = source_tools.prepare_source_overlay(script, output)
    path = Path(manifest["project"]) / "pyproject.toml"
    original = path.read_bytes()
    if changed == "metadata":
        candidate = original.replace(b'name = "original"', b'name = "altered"')
        assert candidate != original
    elif changed == "policy":
        candidate = original + b"\n[tool.soac.strict]\n"
    else:
        candidate = original
        if changed == "disclosure":
            manifest["configuration_provenance"]["presence"] = "absent"
        elif changed == "source_policy":
            manifest["source_policy"] = "legacy-future-selection"
        elif changed == "directive_line":
            module_record = next(
                record
                for record in manifest["files"]
                if record["module_name"] is not None
            )
            module_record["strict_directive_line"] += 1
        else:
            manifest["schema"] = source_tools.SCHEMA - 1
    path.write_bytes(candidate)
    digest = hashlib.sha256(candidate).hexdigest()
    manifest["configuration_sha256"] = digest
    for record in manifest["files"]:
        if record["relative_path"] == "pyproject.toml":
            record["strict_sha256"] = digest
    # These unkeyed provenance hashes are not runtime authority. Recomputing
    # them must not waive exact reconstruction from original inputs and rules.
    manifest["source_fingerprint"] = source_tools._source_fingerprint(manifest)
    manifest.pop("overlay_fingerprint")
    manifest["overlay_fingerprint"] = hashlib.sha256(
        json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    (output / "source-manifest.json").write_text(json.dumps(manifest))
    with pytest.raises(
        ValueError,
        match=(
            "data changed|configuration provenance|source comment policy|"
            "non-opt-in source change|fingerprint/schema"
        ),
    ):
        source_tools.verify_source_overlay(output)
