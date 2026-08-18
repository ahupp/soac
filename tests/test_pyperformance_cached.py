import importlib.util
from pathlib import Path
from types import SimpleNamespace

import pytest


def _load_cache_module():
    script = (
        Path(__file__).resolve().parents[1]
        / "scripts"
        / "run_pyperformance_cached.py"
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
    assert "reusing validated benchmark requirements for sample" in capsys.readouterr().out


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
    benchmark_cache.venv._env["PIP_INDEX_URL"] = "https://packages.example.invalid/simple"
    _ensure(benchmark_cache)

    assert len(benchmark_cache.calls) == 2
    markers = list(
        (benchmark_cache.root / benchmark_cache.module._CACHE_DIRECTORY).glob("*.json")
    )
    assert len(markers) == 2
    assert all("packages.example.invalid" not in marker.read_text() for marker in markers)


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
