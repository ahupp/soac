import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace

import pytest


def _load_cache_module():
    script = (
        Path(__file__).resolve().parents[1] / "scripts" / "run_pyperformance_cached.py"
    )
    spec = importlib.util.spec_from_file_location("run_pyperformance_cached", script)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _create_distribution(root: Path, name: str, version: str) -> Path:
    distribution = root / f"{name}-{version}.dist-info"
    distribution.mkdir(parents=True)
    (distribution / "METADATA").write_text(
        f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n",
        encoding="utf-8",
    )
    (distribution / "RECORD").write_text("", encoding="utf-8")
    return distribution


@pytest.fixture
def benchmark_cache(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    module = _load_cache_module()

    repo_venv = tmp_path / "repo-venv"
    repo_venv.mkdir()
    (repo_venv / ".soac-ready").write_text("ready\n", encoding="utf-8")
    monkeypatch.setenv("VENV_DIR", str(repo_venv))

    venv_root = tmp_path / "benchmark-venv"
    executable = venv_root / "bin" / "python"
    executable.parent.mkdir(parents=True)
    executable.write_text("python-one\n", encoding="utf-8")
    (venv_root / "pyvenv.cfg").write_text("version = 3.15\n", encoding="utf-8")
    packages = venv_root / "lib" / "python3.15" / "site-packages"
    packages.mkdir(parents=True)
    _create_distribution(packages, "pyperf", module.pyperf.__version__)
    _create_distribution(packages, "psutil", "7.2.2")

    lockfile = tmp_path / "benchmark-requirements.txt"
    lockfile.write_text("sample==1.0\n", encoding="utf-8")
    benchmark = SimpleNamespace(name="sample", requirements_lockfile=str(lockfile))
    venv = SimpleNamespace(
        root=str(venv_root),
        python=str(executable),
        _env={"HOME": str(tmp_path), "PATH": "/usr/bin"},
    )
    calls: list[object] = []

    def original(actual_venv, requirements):
        assert actual_venv is venv
        calls.append(requirements)
        if hasattr(requirements, "requirements_lockfile"):
            return module._benchmark_requirements(requirements)
        return requirements

    return SimpleNamespace(
        module=module,
        repo_venv=repo_venv,
        root=venv_root,
        executable=executable,
        packages=packages,
        lockfile=lockfile,
        benchmark=benchmark,
        venv=venv,
        calls=calls,
        original=original,
    )


def _ensure(fixture):
    return fixture.module.ensure_requirements_cached(
        fixture.venv,
        fixture.benchmark,
        fixture.original,
    )


@pytest.mark.parametrize("mode", ["stock", "soac"])
@pytest.mark.parametrize("inherit_form", ["equals", "separate"])
def test_pyperformance_runner_preserves_dependency_network_environment(
    benchmark_cache,
    monkeypatch: pytest.MonkeyPatch,
    mode: str,
    inherit_form: str,
) -> None:
    module = benchmark_cache.module
    expected_values = {
        "PIP_INDEX_URL": "https://fake-user:fake-password@packages.example.invalid/simple",
        "HTTP_PROXY": "http://proxy.example.invalid:3128",
        "CUSTOM_DEPENDENCY_CERT": "/fake/ca.pem",
    }
    for name, value in expected_values.items():
        monkeypatch.setenv(name, value)
    monkeypatch.setenv("PYPERFORMANCE_INHERIT_ENV_EXTRA", "CUSTOM_DEPENDENCY_CERT")
    monkeypatch.setenv("UNRELATED_SECRET", "not-inherited")
    monkeypatch.setenv("XDG_CONFIG_HOME", "/must-not-override-guest-defaults")
    monkeypatch.setenv("ALL_PROXY", "socks5h://127.0.0.1:1080")
    monkeypatch.setenv("all_proxy", "socks5h://127.0.0.1:1081")

    inherited_names = ["PIP_CACHE_DIR", "PIP_DISABLE_PIP_VERSION_CHECK"]
    if mode == "soac":
        monkeypatch.setenv("SOAC_OPT_MODE", "apply")
        inherited_names.append("SOAC_OPT_MODE")

    inherited_csv = ",".join(inherited_names)
    inherited_arguments = (
        [f"--inherit-environ={inherited_csv}"]
        if inherit_form == "equals"
        else ["--inherit-environ", inherited_csv]
    )
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["run_pyperformance_cached.py", "run", *inherited_arguments],
    )
    monkeypatch.setattr(module, "install_requirement_cache", lambda: None)
    monkeypatch.setattr(module.cli, "is_installed", lambda: True)
    monkeypatch.setattr(module.cli, "_benchmarks_from_options", lambda _options: [])

    observed: dict[str, object] = {}

    def capture_benchmark_environment(options, _benchmarks) -> None:
        venv = module.VenvForBenchmarks(
            str(benchmark_cache.root),
            inherit_environ=options.inherit_environ,
        )
        environment = venv._env
        observed["matches"] = {
            name: environment.get(name) == value
            for name, value in expected_values.items()
        }
        observed["names"] = set(environment)

    monkeypatch.setattr(module.cli, "cmd_run", capture_benchmark_environment)

    module.main()

    assert observed["matches"] == {name: True for name in expected_values}
    assert "UNRELATED_SECRET" not in observed["names"]
    assert "XDG_CONFIG_HOME" not in observed["names"]
    assert "ALL_PROXY" not in observed["names"]
    assert "all_proxy" not in observed["names"]
    assert ("SOAC_OPT_MODE" in observed["names"]) is (mode == "soac")
    assert module.sys.argv[0] == "run_pyperformance_cached.py"


@pytest.mark.parametrize("proxy_name", ["ALL_PROXY", "all_proxy"])
def test_pyperformance_runner_preserves_explicit_all_proxy_opt_in(
    benchmark_cache,
    proxy_name: str,
) -> None:
    environment = {
        "ALL_PROXY": "socks5h://127.0.0.1:1080",
        "all_proxy": "socks5h://127.0.0.1:1081",
        "PYPERFORMANCE_INHERIT_ENV_EXTRA": proxy_name,
    }
    arguments = [
        "run_pyperformance_cached.py",
        "run",
        "--inherit-environ=PIP_CACHE_DIR",
    ]

    benchmark_cache.module._inherit_installer_environment(arguments, environment)

    inherited = set(arguments[-1].partition("=")[2].split(","))
    assert proxy_name in inherited
    assert ({"ALL_PROXY", "all_proxy"} - {proxy_name}).isdisjoint(inherited)


@pytest.mark.parametrize(
    ("driver_name", "result_names"),
    [
        ("fastapi", ("fastapi_http",)),
        ("base64", ("base64_small", "base64_large")),
    ],
)
def test_pyperformance_runner_attributes_all_results_to_their_driver(
    benchmark_cache,
    monkeypatch: pytest.MonkeyPatch,
    driver_name: str,
    result_names: tuple[str, ...],
) -> None:
    module = benchmark_cache.module
    suite = module.pyperf.BenchmarkSuite(
        [
            module.pyperf.Benchmark(
                [
                    module.pyperf.Run(
                        [1.0],
                        metadata={"name": result_name, "unit": "second", "loops": 1},
                        collect_metadata=False,
                    )
                ]
            )
            for result_name in result_names
        ]
    )
    script = benchmark_cache.root / "run_benchmark.py"
    script.write_text("result = 1\n")
    driver = SimpleNamespace(name=driver_name, runscript=str(script))

    def original_run(actual_driver, *_args, **_kwargs):
        assert actual_driver is driver
        return suite

    monkeypatch.setattr(module.PyperformanceBenchmark, "run", original_run)
    module.install_benchmark_driver_provenance()

    result = module.PyperformanceBenchmark.run(driver, "/fake/python")

    assert result is suite
    assert {
        benchmark.get_name(): benchmark.get_metadata()["soac_pyperformance_driver"]
        for benchmark in result.get_benchmarks()
    } == {result_name: driver_name for result_name in result_names}
    assert all(
        measured.get_metadata()["soac_pyperformance_language"] == "ordinary"
        for measured in result.get_benchmarks()
    )


def test_benchmark_requirement_cache_reuses_validated_environment(
    benchmark_cache,
    capsys: pytest.CaptureFixture[str],
) -> None:
    first = _ensure(benchmark_cache)
    second = _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 1
    assert list(first) == list(second)
    assert "sample==1.0" in list(second)
    assert any(requirement.startswith("pyperf==") for requirement in second)
    assert (
        "reusing validated benchmark requirements for sample" in capsys.readouterr().out
    )


@pytest.mark.parametrize("previous_bundle", [None, "/previous/bundle.json"])
@pytest.mark.parametrize("use_venv", [False, True])
def test_strict_driver_publishes_bundle_to_real_upstream_worker_environment(
    benchmark_cache, monkeypatch, previous_bundle, use_venv
):
    from pyperformance._benchmark import _prep_cmd

    module = benchmark_cache.module
    script = benchmark_cache.root / "run_benchmark.py"
    script.write_text("# source selection is covered by the source-preparation tests\n")
    driver = SimpleNamespace(runscript=str(script))
    calls = []
    selected_python = benchmark_cache.venv.python if use_venv else "/explicit/python"
    bundle = {
        "manifest_path": "/selected/bundle.json",
        "source_fingerprint": "fixture-fingerprint",
        "selection_policy": "fixture-policy",
        "source": {"harness_projection": {"policy": "fixture-harness-policy"}},
    }

    def prepare(script_arg, python, output, checker, environment):
        assert script_arg == script
        assert str(python) == selected_python
        assert environment is module.os.environ
        calls.append("prepared")
        return bundle

    monkeypatch.setattr(
        module,
        "_strict_source_tools",
        lambda: SimpleNamespace(
            stock_source_fingerprint=lambda _: "stock-fingerprint",
            prepare_strict_benchmark=prepare,
        ),
    )
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "1")
    monkeypatch.setenv("SOAC_WORK_DIR", str(benchmark_cache.root / "work"))
    monkeypatch.setenv("SOAC_PYPERFORMANCE_CHECKER", "/offline/checker")
    if previous_bundle is None:
        monkeypatch.delenv("SOAC_PYPERFORMANCE_STRICT_BUNDLE", raising=False)
    else:
        monkeypatch.setenv("SOAC_PYPERFORMANCE_STRICT_BUNDLE", previous_bundle)
    arguments = (module.sys.executable if use_venv else selected_python,)
    kwargs = {"venv": benchmark_cache.venv} if use_venv else {}
    with module._benchmark_execution(driver, arguments, kwargs) as metadata:
        assert calls == ["prepared"]
        _, environment = _prep_cmd(
            selected_python, str(script), [], "real-upstream-environment"
        )
        assert (
            environment["SOAC_PYPERFORMANCE_STRICT_BUNDLE"] == bundle["manifest_path"]
        )
        assert "SOAC_PYPERFORMANCE_STRICT_BUNDLE" not in benchmark_cache.venv._env
        assert metadata["soac_pyperformance_language"] == "strict"
    assert module.os.environ.get("SOAC_PYPERFORMANCE_STRICT_BUNDLE") == previous_bundle


def test_benchmark_requirement_cache_does_not_intercept_non_benchmark_calls(
    benchmark_cache,
) -> None:
    requirements = ["psutil"]
    result = benchmark_cache.module.ensure_requirements_cached(
        benchmark_cache.venv,
        requirements,
        benchmark_cache.original,
    )

    assert result is requirements
    assert benchmark_cache.calls == [requirements]
    assert not (benchmark_cache.root / benchmark_cache.module._CACHE_DIRECTORY).exists()


def test_benchmark_requirement_cache_invalidates_changed_lockfile(
    benchmark_cache,
) -> None:
    _ensure(benchmark_cache)
    benchmark_cache.lockfile.write_text("sample==2.0\n", encoding="utf-8")
    result = _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2
    assert "sample==2.0" in list(result)


def test_benchmark_requirement_cache_invalidates_included_requirements(
    benchmark_cache,
) -> None:
    included = benchmark_cache.lockfile.parent / "included.txt"
    included.write_text("transitive==1.0\n", encoding="utf-8")
    benchmark_cache.lockfile.write_text("-r included.txt\n", encoding="utf-8")

    _ensure(benchmark_cache)
    _ensure(benchmark_cache)
    included.write_text("transitive==2.0\n", encoding="utf-8")
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2


def test_benchmark_requirement_cache_invalidates_changed_interpreter(
    benchmark_cache,
) -> None:
    _ensure(benchmark_cache)
    benchmark_cache.executable.write_text("different-interpreter\n", encoding="utf-8")
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2


def test_benchmark_requirement_cache_invalidates_recreated_venv_config(
    benchmark_cache,
) -> None:
    _ensure(benchmark_cache)
    (benchmark_cache.root / "pyvenv.cfg").write_text(
        "version = 3.16\n", encoding="utf-8"
    )
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2


def test_benchmark_requirement_cache_keeps_separate_pythonpath_contexts(
    benchmark_cache,
) -> None:
    stock_environment = dict(benchmark_cache.venv._env)
    _ensure(benchmark_cache)

    dependency_root = benchmark_cache.root.parent / "driver-packages"
    dependency_root.mkdir()
    _create_distribution(dependency_root, "driver_dependency", "1.0")
    benchmark_cache.venv._env["PYTHONPATH"] = str(dependency_root)
    _ensure(benchmark_cache)
    _ensure(benchmark_cache)

    benchmark_cache.venv._env = stock_environment
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2
    markers = list(
        (benchmark_cache.root / benchmark_cache.module._CACHE_DIRECTORY).glob("*.json")
    )
    assert len(markers) == 2


def test_benchmark_requirement_cache_reuses_profile_and_apply_environment(
    benchmark_cache,
) -> None:
    benchmark_cache.venv._env["SOAC_OPT_MODE"] = "profile"
    benchmark_cache.venv._env["SOAC_WORK_DIR"] = "/tmp/profile-work"
    _ensure(benchmark_cache)

    benchmark_cache.venv._env["SOAC_OPT_MODE"] = "apply"
    benchmark_cache.venv._env["SOAC_WORK_DIR"] = "/tmp/apply-work"
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 1


def test_benchmark_requirement_cache_invalidates_changed_environment(
    benchmark_cache,
) -> None:
    _ensure(benchmark_cache)
    fake_credential = "fake-credential-never-in-marker"
    benchmark_cache.venv._env["PIP_INDEX_URL"] = (
        f"https://fake-user:{fake_credential}@packages.example.invalid/simple"
    )
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2
    markers = list(
        (benchmark_cache.root / benchmark_cache.module._CACHE_DIRECTORY).glob("*.json")
    )
    assert len(markers) == 2
    assert all(
        "packages.example.invalid" not in marker.read_text() for marker in markers
    )
    assert all(fake_credential not in marker.read_text() for marker in markers)


def test_benchmark_requirement_cache_invalidates_changed_installed_dependencies(
    benchmark_cache,
) -> None:
    _ensure(benchmark_cache)
    _create_distribution(benchmark_cache.packages, "another_dependency", "1.0")
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2


def test_benchmark_requirement_cache_invalidates_changed_dependency_metadata(
    benchmark_cache,
) -> None:
    _ensure(benchmark_cache)
    metadata = (
        benchmark_cache.packages
        / f"pyperf-{benchmark_cache.module.pyperf.__version__}.dist-info"
        / "METADATA"
    )
    metadata.write_text(
        "Metadata-Version: 2.1\nName: pyperf\nVersion: 99.0\n",
        encoding="utf-8",
    )
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2


def test_benchmark_requirement_cache_invalidates_repo_venv_refresh(
    benchmark_cache,
) -> None:
    _ensure(benchmark_cache)
    (benchmark_cache.repo_venv / ".soac-ready").write_text(
        "refreshed\n", encoding="utf-8"
    )
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2


def test_benchmark_requirement_cache_does_not_cache_failed_installation(
    benchmark_cache,
) -> None:
    def failed(_venv, _requirements):
        raise RuntimeError("installation failed")

    with pytest.raises(RuntimeError, match="installation failed"):
        benchmark_cache.module.ensure_requirements_cached(
            benchmark_cache.venv,
            benchmark_cache.benchmark,
            failed,
        )

    assert not (benchmark_cache.root / benchmark_cache.module._CACHE_DIRECTORY).exists()
    _ensure(benchmark_cache)
    assert len(benchmark_cache.calls) == 1


def test_benchmark_requirement_cache_handles_corrupted_marker(
    benchmark_cache,
) -> None:
    _ensure(benchmark_cache)
    marker = next(
        (benchmark_cache.root / benchmark_cache.module._CACHE_DIRECTORY).glob("*.json")
    )
    marker.write_text("{not valid JSON", encoding="utf-8")
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2


def test_benchmark_requirement_cache_preserves_result_when_marker_write_fails(
    benchmark_cache,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def failed_write(_path, _state):
        raise OSError("read-only benchmark venv")

    monkeypatch.setattr(benchmark_cache.module, "_write_marker", failed_write)
    result = _ensure(benchmark_cache)

    assert "sample==1.0" in list(result)
    assert len(benchmark_cache.calls) == 1
    assert "benchmark requirement cache unavailable" in capsys.readouterr().err


def test_driver_report_preserves_preparation_worker_and_dependency_failures(
    benchmark_cache, monkeypatch, tmp_path
):
    module = benchmark_cache.module
    output = tmp_path / "apply.json"
    report = module.BenchmarkRunReport(output)
    drivers = [
        SimpleNamespace(name=name, runscript=str(tmp_path / f"{name}.py"))
        for name in ("accepted", "rejected", "worker_error", "dependency_error")
    ]
    for driver in drivers:
        Path(driver.runscript).write_text("result = 1\n")
    worker_calls = []

    def prepare(script, python, directory, checker, environment):
        if script.stem == "rejected":
            raise RuntimeError("synthetic checker diagnostic")
        return {
            "manifest_path": str(directory / "execution.json"),
            "source_fingerprint": "synthetic-source",
            "selection_policy": "synthetic-selection",
            "source": {"harness_projection": {"policy": "synthetic-harness"}},
        }

    def original_run(driver, *_args, **_kwargs):
        worker_calls.append(driver.name)
        if driver.name == "worker_error":
            raise RuntimeError("synthetic worker failure")
        return module.pyperf.Benchmark(
            [
                module.pyperf.Run(
                    [1.0],
                    metadata={"name": driver.name, "unit": "second", "loops": 1},
                    collect_metadata=False,
                )
            ]
        )

    def upstream_run(selected, python, _options):
        errors = []
        suite = None
        for driver in selected:
            if driver.name == "dependency_error":
                errors.append((driver.name, "Install requirements error"))
                continue
            try:
                measured = module.PyperformanceBenchmark.run(driver, python)
            except RuntimeError as error:
                errors.append((driver.name, error))
            else:
                suite = module.pyperf.BenchmarkSuite([measured])
        return suite, errors

    monkeypatch.setattr(
        module,
        "_strict_source_tools",
        lambda: SimpleNamespace(
            stock_source_fingerprint=lambda _script: "synthetic-stock",
            prepare_strict_benchmark=prepare,
        ),
    )
    monkeypatch.setattr(module.PyperformanceBenchmark, "run", original_run)
    monkeypatch.setattr(module.pyperformance_run, "run_benchmarks", upstream_run)
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "1")
    monkeypatch.setenv("SOAC_OPT_MODE", "apply")
    monkeypatch.setenv("SOAC_WORK_DIR", str(tmp_path / "work"))
    monkeypatch.setenv("SOAC_PYPERFORMANCE_CHECKER", "/checker")
    monkeypatch.setenv("UNRELATED_SECRET", "never-write-this-secret")
    module.install_benchmark_driver_provenance(report)
    module.install_benchmark_run_reporting(report)
    suite, errors = module.pyperformance_run.run_benchmarks(drivers, "/python", None)
    report.finish(1)
    assert suite.get_benchmark_names() == ["accepted"]
    assert len(errors) == 3
    assert worker_calls == ["accepted", "worker_error"]
    path = Path(str(output) + ".status.json")
    contents = path.read_text()
    assert "never-write-this-secret" not in contents
    data = json.loads(contents)
    assert data["requested_drivers"] == sorted(driver.name for driver in drivers)
    assert data["exit_code"] == 1 and data["complete"] is False
    records = {row["benchmark"]: row for row in data["records"]}
    assert records["accepted"]["status"] == "succeeded"
    assert records["accepted"]["emitted_results"] == ["accepted"]
    assert records["rejected"]["stage"] == "strict_preparation"
    assert records["worker_error"]["stage"] == "worker"
    assert records["dependency_error"]["stage"] == "dependency_preparation"
    assert all(records[name]["error"] for name in records if name != "accepted")


def test_phase_reports_preserve_checker_diagnostics_before_later_phases_overwrite_them(
    benchmark_cache, monkeypatch, tmp_path
):
    module = benchmark_cache.module
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "1")
    monkeypatch.setenv("SOAC_OPT_MODE", "profile")
    directory = tmp_path / "sources"
    directory.mkdir()
    source = directory / "checker.stderr.log"
    source.write_text("profile checker diagnostic\n")
    report = module.BenchmarkRunReport(tmp_path / "profile.json")
    report.begin([SimpleNamespace(name="driver")])
    report.stage("driver", "strict_preparation")
    report.preserve_checker_logs("driver", directory, {})
    report.fail("driver", RuntimeError("profile rejected"))
    report.finish(1)

    source.write_text("later apply checker diagnostic\n")
    data = json.loads(report.path.read_text())
    copy = Path(data["records"][0]["diagnostics"][0]["path"])
    assert copy.read_text() == "profile checker diagnostic\n"
    assert data["exit_code"] == 1
    assert data["records"][0]["error"] == "RuntimeError: profile rejected"

    # A reused bundle did not run the checker, so don't attribute old logs to it.
    info = source.stat()
    unchanged = {
        "checker.stderr.log": (info.st_mtime_ns, info.st_ctime_ns, info.st_size)
    }
    monkeypatch.setenv("SOAC_OPT_MODE", "apply")
    apply = module.BenchmarkRunReport(tmp_path / "apply.json")
    apply.begin([SimpleNamespace(name="driver")])
    apply.preserve_checker_logs("driver", directory, unchanged)
    assert apply.records["driver"]["diagnostics"] == []


def test_diagnostic_copy_failure_does_not_replace_the_checker_rejection(
    benchmark_cache, monkeypatch, tmp_path
):
    module = benchmark_cache.module
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "1")
    monkeypatch.setenv("SOAC_OPT_MODE", "profile")
    monkeypatch.setenv("SOAC_WORK_DIR", str(tmp_path / "work"))
    monkeypatch.setenv("SOAC_PYPERFORMANCE_CHECKER", "/checker")
    driver = SimpleNamespace(name="driver", runscript=str(tmp_path / "driver.py"))
    report = module.BenchmarkRunReport(tmp_path / "profile.json")
    report.begin([driver])

    def prepare(_script, _python, directory, _checker, _environment):
        directory.mkdir(parents=True)
        (directory / "checker.stderr.log").write_text("original rejection\n")
        raise RuntimeError("original rejection")

    def copy_failure(*_args):
        raise OSError("synthetic diagnostic-copy failure")

    def unexpected_worker(*_args, **_kwargs):
        pytest.fail("rejected source must not start a benchmark worker")

    monkeypatch.setattr(
        module,
        "_strict_source_tools",
        lambda: SimpleNamespace(
            stock_source_fingerprint=lambda _script: "synthetic",
            prepare_strict_benchmark=prepare,
        ),
    )
    monkeypatch.setattr(module.shutil, "copyfile", copy_failure)
    monkeypatch.setattr(module.PyperformanceBenchmark, "run", unexpected_worker)
    module.install_benchmark_driver_provenance(report)
    with pytest.raises(RuntimeError, match="original rejection"):
        module.PyperformanceBenchmark.run(driver, "/python")
    report.finish(1)
    data = json.loads(report.path.read_text())
    assert data["records"][0]["error"] == "RuntimeError: original rejection"
    assert data["records"][0]["diagnostics"] == [
        {"kind": "checker.stderr.log", "error": "synthetic diagnostic-copy failure"}
    ]


@pytest.fixture
def local_package_cache(benchmark_cache, monkeypatch, tmp_path):
    """A pinned ordinary package and a metadata-writing installer, no workers."""
    import base64
    import csv
    import hashlib
    import tarfile

    fixture = benchmark_cache
    module = fixture.module
    local = module._local_package_tools()
    sources = module._strict_source_tools()
    stock = tmp_path / "stock-source"
    stock.mkdir()
    script = stock / "run_benchmark.py"
    script.write_text("raise AssertionError('benchmark initializer must not run')\n")
    package = stock / "vendor"
    payload = package / "src" / "sample_dependency" / "__init__.py"
    payload.parent.mkdir(parents=True)
    payload.write_text("VALUE = 1\n")
    (package / "pyproject.toml").write_text(
        '[build-system]\nrequires = ["setuptools>=61"]\nbuild-backend = "setuptools.build_meta"\n'
        '[project]\nname = "sample-dependency"\nversion = "1.0"\n'
    )
    manifest_path = tmp_path / "local-packages.json"
    monkeypatch.setattr(local, "MANIFEST", manifest_path)
    fixture.benchmark.runscript = str(script)
    fixture.venv.info = SimpleNamespace(
        sys=SimpleNamespace(version_info=(3, 15, 0, "alpha", 1))
    )

    def pin():
        files = [
            {
                "path": path.as_posix(),
                "sha256": hashlib.sha256((package / path).read_bytes()).hexdigest(),
                "mode": (package / path).stat().st_mode & 0o777,
            }
            for path in sources._source_inventory(package)
        ]
        manifest_path.write_bytes(
            local._json(
                {
                    "schema": 1,
                    "benchmarks": {
                        fixture.benchmark.name: {
                            "script": script.name,
                            "stock_source_fingerprint": sources.stock_source_fingerprint(
                                script
                            ),
                            "packages": [
                                {
                                    "path": "vendor",
                                    "distribution": "sample-dependency",
                                    "version": "1.0",
                                    "when_python": ">=3.13.0a0",
                                    "source_sha256": local._digest(local._json(files)),
                                }
                            ],
                        }
                    },
                }
            )
        )

    pin()
    events = []
    installs = []

    def installer(venv, requirements):
        if hasattr(requirements, "requirements_lockfile"):
            events.append("requirements")
            return module._benchmark_requirements(requirements)
        assert requirements[0] == "--force-reinstall"
        assert "sample==1.0" in requirements
        assert any(value.startswith("pyperf==") for value in requirements)
        archives = [Path(value) for value in requirements if value.endswith(".tar.gz")]
        assert len(archives) == 1
        archive = archives[0]
        assert not archive.is_relative_to(stock)
        installs.append((venv.root, archive))
        events.append("local-package")
        packages = Path(venv.root) / "lib" / "python3.15" / "site-packages"
        directory = packages / "sample_dependency-1.0.dist-info"
        directory.mkdir(parents=True, exist_ok=True)
        metadata = directory / "METADATA"
        metadata.write_text(
            "Metadata-Version: 2.1\nName: sample-dependency\nVersion: 1.0\n"
        )
        direct = directory / "direct_url.json"
        direct.write_text(
            json.dumps(
                {
                    "url": archive.as_uri(),
                    "archive_info": {
                        "hashes": {
                            "sha256": hashlib.sha256(archive.read_bytes()).hexdigest()
                        }
                    },
                }
            )
        )
        installed = packages / "sample_dependency" / "__init__.py"
        installed.parent.mkdir(exist_ok=True)
        with tarfile.open(archive) as contents:
            # Inspect the retained package archive, never execute setup or driver code.
            installed.write_bytes(
                contents.extractfile("source/src/sample_dependency/__init__.py").read()
            )
        with (directory / "RECORD").open("w", newline="") as output:
            rows = csv.writer(output)
            for path in (metadata, direct, installed):
                data = path.read_bytes()
                digest = (
                    base64.urlsafe_b64encode(hashlib.sha256(data).digest())
                    .rstrip(b"=")
                    .decode()
                )
                rows.writerow(
                    [
                        path.relative_to(packages).as_posix(),
                        "sha256=" + digest,
                        len(data),
                    ]
                )
            rows.writerow([directory.name + "/RECORD", "", ""])
        return requirements

    fixture.original = installer
    return SimpleNamespace(
        **vars(fixture),
        local=local,
        sources=sources,
        script=script,
        stock=stock,
        payload=payload,
        manifest=manifest_path,
        pin=pin,
        events=events,
        installs=installs,
    )


def test_declared_local_package_is_identical_and_ready_before_stock_or_strict_driver(
    local_package_cache, monkeypatch
):
    fixture = local_package_cache
    module = fixture.module
    original = {
        path: path.read_bytes() for path in fixture.stock.rglob("*") if path.is_file()
    }
    fingerprints = []

    def analyze(_script, _python, _output, _checker, _environment):
        assert fixture.events == ["requirements", "local-package", "stock-worker"]
        fixture.events.append("offline-analysis")
        return {
            "manifest_path": "/synthetic/execution.json",
            "source_fingerprint": "source",
            "selection_policy": "policy",
            "source": {"harness_projection": {"policy": "harness"}},
        }

    monkeypatch.setattr(fixture.sources, "prepare_strict_benchmark", analyze)
    monkeypatch.setenv("SOAC_WORK_DIR", str(fixture.root / "strict-work"))
    monkeypatch.setenv("SOAC_PYPERFORMANCE_CHECKER", "/never-executed/checker")
    for strict in (False, True):
        monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "1" if strict else "0")
        _ensure(fixture)
        with module._benchmark_execution(
            fixture.benchmark, (fixture.venv.python,), {"venv": fixture.venv}
        ) as metadata:
            fingerprints.append(
                metadata["soac_pyperformance_local_packages_fingerprint"]
            )
            fixture.events.append("strict-worker" if strict else "stock-worker")
    assert len(set(fingerprints)) == 1
    assert len(fixture.installs) == 1
    assert fixture.events[-2:] == ["offline-analysis", "strict-worker"]
    assert original == {
        path: path.read_bytes() for path in fixture.stock.rglob("*") if path.is_file()
    }
    marker = next((fixture.root / module._CACHE_DIRECTORY).glob("*.json"))
    retained = json.loads(marker.read_text())["local_packages"]
    assert retained["ready"] is True
    assert retained["provenance"][
        "stock_source_fingerprint"
    ] == fixture.sources.stock_source_fingerprint(fixture.script)
    assert Path(retained["source_manifest"]).is_file()


def test_declared_package_drift_is_not_silently_treated_as_ordinary_requirements(
    local_package_cache,
    capsys,
):
    fixture = local_package_cache
    _ensure(fixture)
    fixture.payload.write_text("VALUE = 2\n")
    with pytest.raises(
        fixture.local.RequirementsInstallationFailedError,
        match="original source fingerprint",
    ):
        _ensure(fixture)
    assert fixture.events == ["requirements", "local-package"]
    assert "original source fingerprint mismatch" in capsys.readouterr().err


def test_explicit_manifest_update_invalidates_cache_and_preserves_old_archive(
    local_package_cache,
):
    fixture = local_package_cache
    _ensure(fixture)
    old_archive = fixture.installs[0][1]
    old_bytes = old_archive.read_bytes()
    fixture.payload.write_text("VALUE = 2\n")
    fixture.pin()
    _ensure(fixture)
    assert len(fixture.installs) == 2
    assert fixture.installs[1][1] != old_archive
    assert old_archive.read_bytes() == old_bytes
    assert (
        fixture.packages / "sample_dependency" / "__init__.py"
    ).read_bytes() == fixture.payload.read_bytes()


def test_local_package_preparation_is_not_bypassed_when_generic_cache_is_unavailable(
    local_package_cache, monkeypatch
):
    fixture = local_package_cache

    def unavailable(*_args):
        raise OSError("synthetic generic cache failure")

    monkeypatch.setattr(fixture.module, "_cache_state", unavailable)
    _ensure(fixture)
    assert fixture.events == ["requirements", "local-package"]
    plan = fixture.local.for_environment(
        fixture.local.resolve(fixture.benchmark, fixture.sources), fixture.venv
    )
    assert fixture.local.require_ready(fixture.venv, plan, [fixture.packages])["ready"]


def test_local_package_installed_payload_drift_invalidates_cache_and_blocks_analysis(
    local_package_cache, monkeypatch
):
    fixture = local_package_cache
    _ensure(fixture)
    installed = fixture.packages / "sample_dependency" / "__init__.py"
    installed.write_text("VALUE = 'changed after install'\n")
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "1")
    with pytest.raises(
        fixture.local.RequirementsInstallationFailedError, match="not ready"
    ):
        with fixture.module._benchmark_execution(
            fixture.benchmark, (fixture.venv.python,), {"venv": fixture.venv}
        ):
            pytest.fail("stale dependency must not reach checker or worker")
    _ensure(fixture)
    assert len(fixture.installs) == 2
    assert installed.read_bytes() == fixture.payload.read_bytes()


def test_local_package_failure_retains_upstream_common_to_unique_venv_fallback(
    local_package_cache, tmp_path
):
    fixture = local_package_cache
    module = fixture.module
    unique_root = tmp_path / "unique-venv"
    executable = unique_root / "bin" / "python"
    executable.parent.mkdir(parents=True)
    executable.write_bytes(fixture.executable.read_bytes())
    (unique_root / "pyvenv.cfg").write_text("version = 3.15\n")
    (unique_root / "lib" / "python3.15" / "site-packages").mkdir(parents=True)
    unique = SimpleNamespace(
        root=str(unique_root),
        python=str(executable),
        _env=dict(fixture.venv._env),
        info=fixture.venv.info,
    )

    def common_fails(venv, requirements):
        if venv is fixture.venv and isinstance(requirements, list):
            raise fixture.local.RequirementsInstallationFailedError(
                "synthetic local package installation failure"
            )
        return fixture.original(venv, requirements)

    with pytest.raises(fixture.local.RequirementsInstallationFailedError):
        module.ensure_requirements_cached(fixture.venv, fixture.benchmark, common_fails)
    assert not (fixture.root / module._CACHE_DIRECTORY).exists()
    module.ensure_requirements_cached(unique, fixture.benchmark, common_fails)
    module.ensure_requirements_cached(unique, fixture.benchmark, common_fails)
    assert len(fixture.installs) == 1 and fixture.installs[0][0] == unique.root
    common_archive = next(
        (fixture.root / fixture.local.CACHE_DIRECTORY).rglob("*.tar.gz")
    )
    assert common_archive != fixture.installs[0][1]
    assert common_archive.read_bytes() == fixture.installs[0][1].read_bytes()
    assert next((unique_root / module._CACHE_DIRECTORY).glob("*.json")).is_file()


@pytest.mark.parametrize(
    "path", ["..", "../outside", "/outside", ".", "vendor/../vendor"]
)
def test_local_package_manifest_rejects_escaping_or_noncanonical_package_paths(
    local_package_cache, path
):
    fixture = local_package_cache
    data = json.loads(fixture.manifest.read_text())
    data["benchmarks"][fixture.benchmark.name]["packages"][0]["path"] = path
    fixture.manifest.write_text(json.dumps(data))
    with pytest.raises(
        fixture.local.RequirementsInstallationFailedError, match="path must"
    ):
        _ensure(fixture)
    assert not fixture.events


def test_local_package_predicate_uses_selected_venv_python_not_host_python(
    local_package_cache,
):
    fixture = local_package_cache
    fixture.venv.info.sys.version_info = (3, 12, 10, "final", 0)
    _ensure(fixture)
    _ensure(fixture)
    assert fixture.events == ["requirements"]
    assert not fixture.installs


def _rewrite_local_package_record(packages):
    """Model coherent installer metadata, not a trivially stale RECORD hash."""
    import base64
    import csv
    import hashlib

    record = packages / "sample_dependency-1.0.dist-info" / "RECORD"
    with record.open(newline="") as stream:
        rows = list(csv.reader(stream))
    for row in rows:
        if row[1]:
            data = (packages / row[0]).read_bytes()
            digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
            row[1:] = ["sha256=" + digest, str(len(data))]
    with record.open("w", newline="") as stream:
        csv.writer(stream).writerows(rows)


def _additional_local_package_venv(fixture, root):
    executable = root / "bin" / "python"
    executable.parent.mkdir(parents=True)
    executable.write_bytes(fixture.executable.read_bytes())
    (root / "pyvenv.cfg").write_text("version = 3.15\n")
    packages = root / "lib" / "python3.15" / "site-packages"
    packages.mkdir(parents=True)
    return SimpleNamespace(
        root=str(root), python=str(executable), _env=dict(fixture.venv._env),
        info=fixture.venv.info,
    ), packages


def _local_package_result_fingerprint(fixture, venv):
    with fixture.module._benchmark_execution(
        fixture.benchmark, (venv.python,), {"venv": venv}
    ) as metadata:
        return metadata["soac_pyperformance_local_packages_fingerprint"]


def test_local_package_environment_selection_is_idempotent(local_package_cache):
    fixture = local_package_cache
    selected = fixture.local.for_environment(
        fixture.local.resolve(fixture.benchmark, fixture.sources), fixture.venv,
    )
    assert fixture.local.for_environment(selected, fixture.venv) == selected


def test_local_package_selected_plan_cannot_silently_retarget_python(local_package_cache):
    fixture = local_package_cache
    selected = fixture.local.for_environment(
        fixture.local.resolve(fixture.benchmark, fixture.sources), fixture.venv,
    )
    different = SimpleNamespace(info=SimpleNamespace(
        sys=SimpleNamespace(version_info=(3, 12, 10, "final", 0)),
    ))
    with pytest.raises(fixture.local.RequirementsInstallationFailedError, match="retarget"):
        fixture.local.for_environment(selected, different)


def test_local_package_coherent_record_rewrite_is_not_an_accepted_installation(
    local_package_cache, monkeypatch,
):
    fixture = local_package_cache
    _ensure(fixture)
    installed = fixture.packages / "sample_dependency" / "__init__.py"
    installed.write_text("VALUE = 'rewritten with its RECORD'\n")
    _rewrite_local_package_record(fixture.packages)
    # Prove the rewritten payload satisfies its current installation metadata.
    plan = fixture.local.for_environment(
        fixture.local.resolve(fixture.benchmark, fixture.sources), fixture.venv,
    )
    assert "error" not in fixture.local._installed(fixture.venv, plan["packages"][0], [fixture.packages])
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "0")
    with pytest.raises(fixture.local.RequirementsInstallationFailedError, match="not ready"):
        _local_package_result_fingerprint(fixture, fixture.venv)
    _ensure(fixture)
    assert len(fixture.installs) == 2
    assert installed.read_bytes() == fixture.payload.read_bytes()


def test_local_package_result_fingerprint_binds_backend_output_not_only_archive(
    local_package_cache, monkeypatch, tmp_path,
):
    fixture = local_package_cache
    second, packages = _additional_local_package_venv(fixture, tmp_path / "different-build")

    def backend_output(venv, requirements):
        result = fixture.original(venv, requirements)
        if isinstance(requirements, list):
            (packages / "sample_dependency" / "__init__.py").write_text("VALUE = 'backend output'\n")
            _rewrite_local_package_record(packages)
        return result

    _ensure(fixture)
    fixture.module.ensure_requirements_cached(second, fixture.benchmark, backend_output)
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "0")
    assert fixture.installs[0][1].read_bytes() == fixture.installs[1][1].read_bytes()
    assert _local_package_result_fingerprint(fixture, fixture.venv) != _local_package_result_fingerprint(fixture, second)


def test_local_package_equal_payloads_compare_across_different_venv_urls(
    local_package_cache, monkeypatch, tmp_path,
):
    fixture = local_package_cache
    second, packages = _additional_local_package_venv(fixture, tmp_path / "same-build")
    _ensure(fixture)
    fixture.module.ensure_requirements_cached(second, fixture.benchmark, fixture.original)
    direct = "sample_dependency-1.0.dist-info/direct_url.json"
    assert (fixture.packages / direct).read_bytes() != (packages / direct).read_bytes()
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "0")
    assert _local_package_result_fingerprint(fixture, fixture.venv) == _local_package_result_fingerprint(fixture, second)
