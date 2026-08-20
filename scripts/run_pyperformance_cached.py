#!/usr/bin/env python3
"""Run pyperformance without reinstalling unchanged benchmark requirements."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import re
import shlex
import shutil
import sys
from collections.abc import Callable, Mapping
from contextlib import contextmanager
from functools import lru_cache
from pathlib import Path
from typing import Any

import pyperf
import pyperformance
from pyperformance import cli
from pyperformance import run as pyperformance_run
from pyperformance._benchmark import Benchmark as PyperformanceBenchmark
from pyperformance.venv import REQUIREMENTS_FILE, Requirements, VenvForBenchmarks

_CACHE_SCHEMA_VERSION = 1
_CACHE_DIRECTORY = ".soac-pyperformance-ready-v1"
_VOLATILE_ENVIRONMENT_PREFIXES = ("SOAC_",)
_VOLATILE_ENVIRONMENT_NAMES = {"PYPERFORMANCE_RUNID"}
_INSTALLER_ENVIRONMENT_NAMES = (
    "PIP_INDEX_URL",
    "PIP_EXTRA_INDEX_URL",
    "PIP_FIND_LINKS",
    "PIP_NO_INDEX",
    "PIP_TRUSTED_HOST",
    "PIP_PROXY",
    "PIP_CERT",
    "PIP_CLIENT_CERT",
    "PIP_CONFIG_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
)
_ENVIRONMENT_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z", re.ASCII)


def _installer_environment_names(environment: Mapping[str, str]) -> list[str]:
    extras = environment.get("PYPERFORMANCE_INHERIT_ENV_EXTRA", "")
    extra_names = [name.strip() for name in extras.split(",") if name.strip()]
    if any(_ENVIRONMENT_NAME.fullmatch(name) is None for name in extra_names):
        raise ValueError("extra installer environment name is not permitted")

    return list(
        dict.fromkeys(
            name
            for name in (*_INSTALLER_ENVIRONMENT_NAMES, *extra_names)
            if name in environment
        )
    )


def _inherit_installer_environment(
    arguments: list[str], environment: Mapping[str, str]
) -> None:
    _inherit_environment_names(arguments, _installer_environment_names(environment))


def _inherit_environment_names(arguments: list[str], names: list[str]) -> None:
    if not names:
        return

    for index, argument in enumerate(arguments[1:], start=1):
        if argument == "--inherit-environ":
            if index + 1 == len(arguments):
                raise ValueError("--inherit-environ requires environment names")
            value_index = index + 1
            current = arguments[value_index]
            prefix = ""
        elif argument.startswith("--inherit-environ="):
            value_index = index
            current = argument.partition("=")[2]
            prefix = "--inherit-environ="
        else:
            continue

        inherited = dict.fromkeys(name for name in current.split(",") if name)
        inherited.update(dict.fromkeys(names))
        arguments[value_index] = prefix + ",".join(inherited)
        return

    arguments.append("--inherit-environ=" + ",".join(names))


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _file_identity(path: Path, *, contents: bool = False) -> dict[str, Any]:
    identity: dict[str, Any] = {"path": str(path.absolute())}
    try:
        stat = path.stat()
    except OSError:
        identity["missing"] = True
        return identity

    identity.update(
        {
            "resolved": str(path.resolve()),
            "device": stat.st_dev,
            "inode": stat.st_ino,
            "size": stat.st_size,
            "mtime_ns": stat.st_mtime_ns,
        }
    )
    if contents:
        try:
            identity["sha256"] = _sha256(path.read_bytes())
        except OSError:
            identity["unreadable"] = True
    return identity


def _included_requirement_paths(path: Path) -> list[Path]:
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return []

    included: list[Path] = []
    for raw_line in text.splitlines():
        try:
            parts = shlex.split(raw_line, comments=True)
        except ValueError:
            continue
        if not parts:
            continue

        argument: str | None = None
        if parts[0] in {"-r", "--requirement", "-c", "--constraint"}:
            if len(parts) > 1:
                argument = parts[1]
        elif parts[0].startswith(("--requirement=", "--constraint=")):
            argument = parts[0].partition("=")[2]
        elif parts[0].startswith(("-r", "-c")) and len(parts[0]) > 2:
            argument = parts[0][2:]
        elif len(parts) == 1:
            candidate = path.parent / parts[0]
            if candidate.is_file():
                argument = parts[0]

        if argument:
            included.append((path.parent / argument).absolute())
    return included


def _requirement_file_identities(
    path: Path, *, visited: set[str]
) -> list[dict[str, Any]]:
    key = str(path.absolute())
    if key in visited:
        return []
    visited.add(key)

    identities = [_file_identity(path, contents=True)]
    for included in _included_requirement_paths(path):
        identities.extend(_requirement_file_identities(included, visited=visited))
    return identities


def _dependency_environment(venv: Any) -> dict[str, str]:
    environment = getattr(venv, "_env", None) or {}
    return {
        name: str(value)
        for name, value in environment.items()
        if name not in _VOLATILE_ENVIRONMENT_NAMES
        and not name.startswith(_VOLATILE_ENVIRONMENT_PREFIXES)
    }


def _environment_digest(environment: dict[str, str]) -> str:
    payload = json.dumps(environment, sort_keys=True, separators=(",", ":"))
    return _sha256(payload.encode("utf-8"))


def _visible_package_roots(venv_root: Path, environment: dict[str, str]) -> list[Path]:
    candidates: list[Path] = []
    pythonpath = environment.get("PYTHONPATH")
    if pythonpath is not None:
        candidates.extend(
            Path(entry or os.getcwd()).absolute()
            for entry in pythonpath.split(os.pathsep)
        )

    for library in (venv_root / "lib", venv_root / "lib64"):
        if library.is_dir():
            candidates.extend(
                sorted(
                    library.glob("python*/site-packages"), key=lambda path: str(path)
                )
            )
    candidates.append(venv_root / "Lib" / "site-packages")

    visible: list[Path] = []
    seen: set[str] = set()
    for candidate in candidates:
        if not candidate.is_dir():
            continue
        resolved = str(candidate.resolve())
        if resolved in seen:
            continue
        seen.add(resolved)
        visible.append(candidate)
    return visible


def _installed_dependency_identities(
    venv_root: Path, environment: dict[str, str]
) -> list[dict[str, Any]]:
    dependencies: list[dict[str, Any]] = []
    for root in _visible_package_roots(venv_root, environment):
        try:
            entries = sorted(root.iterdir(), key=lambda entry: entry.name)
        except OSError:
            continue
        for entry in entries:
            if not entry.name.endswith((".dist-info", ".egg-info")):
                continue
            identity: dict[str, Any] = {
                "root": str(root.absolute()),
                "distribution": entry.name,
                "entry": _file_identity(entry),
            }
            if entry.is_dir():
                metadata = entry / "METADATA"
                if not metadata.exists():
                    metadata = entry / "PKG-INFO"
                identity["metadata"] = _file_identity(metadata, contents=True)
                record = entry / "RECORD"
                if record.exists():
                    identity["record"] = _file_identity(record)
            dependencies.append(identity)
    return dependencies


def _benchmark_requirements(benchmark: Any) -> Requirements:
    requirements = Requirements.from_benchmarks([benchmark])
    if not requirements.get("pyperf"):
        pyperf_requirement = Requirements.from_file(REQUIREMENTS_FILE).get("pyperf")
        if not pyperf_requirement:
            raise NotImplementedError
        requirements.specs.append(pyperf_requirement)
    return requirements


def _readiness_stamp() -> dict[str, Any]:
    repo_venv = Path(os.environ.get("VENV_DIR", sys.prefix))
    return _file_identity(repo_venv / ".soac-ready", contents=True)


def _cache_state(venv: Any, benchmark: Any, local_packages=None) -> dict[str, Any]:
    venv_root = Path(venv.root).absolute()
    environment = _dependency_environment(venv)
    visited: set[str] = set()
    requirement_files = _requirement_file_identities(
        Path(REQUIREMENTS_FILE), visited=visited
    )
    lockfile = benchmark.requirements_lockfile
    if lockfile:
        requirement_files.extend(
            _requirement_file_identities(Path(lockfile), visited=visited)
        )

    state = {
        "schema": _CACHE_SCHEMA_VERSION,
        "benchmark": str(benchmark.name),
        "venv_root": str(venv_root),
        "pyperformance_version": pyperformance.__version__,
        "pyperf_version": pyperf.__version__,
        "python": _file_identity(Path(venv.python)),
        "venv_config": _file_identity(venv_root / "pyvenv.cfg", contents=True),
        "environment_sha256": _environment_digest(environment),
        "environment_names": sorted(environment),
        "repo_venv_ready": _readiness_stamp(),
        "requirement_files": requirement_files,
        "installed_distributions": _installed_dependency_identities(
            venv_root, environment
        ),
    }
    if local_packages is not None:
        state["local_packages"] = _local_package_tools().state(
            venv, local_packages, _visible_package_roots(venv_root, environment)
        )
    return state


def _marker_path(venv: Any, benchmark: Any, environment_digest: str) -> Path:
    benchmark_name = (
        "".join(
            char if char.isalnum() or char in {"-", "_", "."} else "_"
            for char in str(benchmark.name)
        ).strip("._-")
        or "benchmark"
    )
    filename = f"{benchmark_name}-{environment_digest[:20]}.json"
    return Path(venv.root) / _CACHE_DIRECTORY / filename


def _read_marker(path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def _write_marker(path: Path, state: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        temporary.write_text(
            json.dumps(state, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        os.replace(temporary, path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def ensure_requirements_cached(
    venv: Any,
    requirements: Any,
    original: Callable[[Any, Any], Any],
) -> Any:
    """Preserve upstream setup while reusing a proven, unchanged benchmark venv."""
    if not hasattr(requirements, "requirements_lockfile"):
        return original(venv, requirements)

    benchmark = requirements
    local_tools = _local_package_tools()
    local_packages = local_tools.for_environment(
        local_tools.resolve(benchmark, _strict_source_tools()), venv
    )
    cached = None
    try:
        requested = _benchmark_requirements(benchmark)
        state = _cache_state(venv, benchmark, local_packages)
        marker = _marker_path(venv, benchmark, state["environment_sha256"])
        cached = _read_marker(marker)
    except (OSError, UnicodeError, ValueError, TypeError, AttributeError):
        # A cache failure must not bypass a declared dependency preparation.
        pass

    if (
        cached is not None
        and cached == state
        and (local_packages is None or state["local_packages"]["ready"])
    ):
        print(f"(reusing validated benchmark requirements for {benchmark.name})")
        return requested

    result = original(venv, requirements)
    if local_packages is not None:
        local_tools.prepare(
            venv,
            local_packages,
            original,
            _visible_package_roots(Path(venv.root), _dependency_environment(venv)),
            base_requirements=result,
        )
        # Normal packaging is permitted, but changing original benchmark inputs
        # during installation is not. The archive isolates backend build output.
        rechecked = local_tools.for_environment(
            local_tools.resolve(benchmark, _strict_source_tools()), venv
        )
        if local_tools.public(rechecked) != local_tools.public(local_packages):
            raise local_tools.RequirementsInstallationFailedError(
                "local-package inputs changed during dependency preparation"
            )
    try:
        refreshed = _cache_state(venv, benchmark, local_packages)
        refreshed_marker = _marker_path(
            venv, benchmark, refreshed["environment_sha256"]
        )
        _write_marker(refreshed_marker, refreshed)
    except (OSError, UnicodeError, ValueError, TypeError, AttributeError) as error:
        print(f"(benchmark requirement cache unavailable: {error})", file=sys.stderr)
    return result


def install_requirement_cache() -> None:
    original = VenvForBenchmarks.ensure_reqs
    if getattr(original, "_soac_requirement_cache", False):
        return

    def cached_ensure_reqs(venv: VenvForBenchmarks, requirements: Any = None) -> Any:
        return ensure_requirements_cached(venv, requirements, original)

    cached_ensure_reqs._soac_requirement_cache = True  # type: ignore[attr-defined]
    VenvForBenchmarks.ensure_reqs = cached_ensure_reqs


class BenchmarkRunReport:
    """Durable outcome of every requested driver, including pre-worker failures."""

    def __init__(self, output: Path):
        self.output = output.absolute()
        self.path = Path(str(self.output) + ".status.json")
        if self.path.exists() or self.output.exists():
            raise ValueError(f"benchmark output/report already exists: {self.output}")
        self.records: dict[str, dict[str, Any]] = {}
        self.data: dict[str, Any] = {
            "schema": 1,
            "output": str(self.output),
            "language": (
                "strict"
                if os.environ.get("SOAC_PYPERFORMANCE_ENABLE", "").lower()
                in {"1", "true", "yes", "on"}
                else "ordinary"
            ),
            "optimization_mode": os.environ.get("SOAC_OPT_MODE"),
            "requested_drivers": [],
            "records": [],
            "exit_code": None,
            "complete": False,
        }
        self.write()

    def write(self):
        self.data["records"] = [self.records[name] for name in sorted(self.records)]
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temporary = self.path.with_name(self.path.name + ".tmp")
        temporary.write_text(json.dumps(self.data, sort_keys=True, indent=2) + "\n")
        temporary.replace(self.path)

    def begin(self, benchmarks):
        names = sorted(benchmark.name for benchmark in benchmarks)
        if len(set(names)) != len(names):
            raise ValueError("benchmark driver selection contains duplicate names")
        self.data["requested_drivers"] = names
        self.records = {
            name: {
                "benchmark": name,
                "status": "not_run",
                "stage": "dependency_preparation",
                "emitted_results": [],
            }
            for name in names
        }
        self.write()

    def stage(self, name, stage):
        record = self.records.setdefault(
            name, {"benchmark": name, "emitted_results": []}
        )
        record.update(status="running", stage=stage)
        self.write()

    def fail(self, name, error):
        record = self.records.setdefault(
            name,
            {
                "benchmark": name,
                "stage": "dependency_preparation",
                "emitted_results": [],
            },
        )
        record.update(status="failed", error=f"{type(error).__name__}: {error}")
        self.write()

    def succeed(self, name, results):
        self.records[name].update(
            status="succeeded",
            stage="complete",
            emitted_results=[result.get_name() for result in results],
        )
        self.write()

    def preserve_checker_logs(self, name, directory, previous):
        diagnostics = []
        for filename in ("checker.stdout.log", "checker.stderr.log"):
            try:
                source = directory / filename
                if not source.is_file():
                    continue
                info = source.stat()
                current = (info.st_mtime_ns, info.st_ctime_ns, info.st_size)
                if previous.get(filename) == current:
                    continue
                destination = (
                    Path(str(self.output) + ".diagnostics")
                    / _sha256(name.encode())[:16]
                    / filename
                )
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, destination)
                diagnostics.append({"kind": filename, "path": str(destination)})
            except OSError as error:
                # Keep the primary checker failure instead of replacing it
                # with an error while copying its diagnostic artifact.
                diagnostics.append({"kind": filename, "error": str(error)})
        self.records[name]["diagnostics"] = diagnostics
        self.write()

    def finish(self, exit_code, error=None):
        self.data["exit_code"] = exit_code
        self.data["complete"] = (
            exit_code == 0
            and bool(self.data["requested_drivers"])
            and set(self.records) == set(self.data["requested_drivers"])
            and all(record["status"] == "succeeded" for record in self.records.values())
        )
        if error is not None:
            self.data["error"] = f"{type(error).__name__}: {error}"
        self.write()


def install_benchmark_run_reporting(report: BenchmarkRunReport) -> None:
    original = pyperformance_run.run_benchmarks

    def reported_run(benchmarks, python, options):
        benchmarks = list(benchmarks)
        report.begin(benchmarks)
        suite, errors = original(benchmarks, python, options)
        # Dependency failures never reach Benchmark.run. Preserve those without
        # changing upstream's all-driver continuation or exception handling.
        for name, error in errors:
            if report.records[name]["status"] != "failed":
                report.fail(name, error)
        return suite, errors

    pyperformance_run.run_benchmarks = reported_run


def _output_from_arguments(arguments):
    for index, argument in enumerate(arguments):
        if argument in {"-o", "--output"} and index + 1 < len(arguments):
            return Path(arguments[index + 1])
        if argument.startswith("--output="):
            return Path(argument.partition("=")[2])
    return None


def install_benchmark_driver_provenance(
    report: BenchmarkRunReport | None = None,
) -> None:
    original = PyperformanceBenchmark.run
    if getattr(original, "_soac_benchmark_driver_provenance", False):
        return

    def attributed_run(
        benchmark: PyperformanceBenchmark, *args: Any, **kwargs: Any
    ) -> Any:
        try:
            if report is not None:
                report.stage(benchmark.name, "source_preparation")
            with _benchmark_execution(
                benchmark, args, kwargs, report
            ) as execution_metadata:
                if report is not None:
                    report.stage(benchmark.name, "worker")
                result = original(benchmark, *args, **kwargs)
            results = (
                result.get_benchmarks()
                if isinstance(result, pyperf.BenchmarkSuite)
                else (result,)
            )
        except Exception as error:
            if report is not None:
                report.fail(benchmark.name, error)
            raise
        for measured in results:
            measured.update_metadata(
                {
                    "soac_pyperformance_driver": benchmark.name,
                    **execution_metadata,
                }
            )
        if report is not None:
            report.succeed(benchmark.name, results)
        return result

    attributed_run._soac_benchmark_driver_provenance = True  # type: ignore[attr-defined]
    PyperformanceBenchmark.run = attributed_run


@lru_cache(maxsize=1)
def _local_package_tools():
    path = Path(__file__).with_name("pyperformance_local_packages.py")
    spec = importlib.util.spec_from_file_location("_soac_local_packages", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@lru_cache(maxsize=1)
def _strict_source_tools():
    path = Path(__file__).with_name("strict_pyperformance_sources.py")
    spec = importlib.util.spec_from_file_location("_soac_benchmark_sources", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@contextmanager
def _benchmark_execution(benchmark, args, kwargs, report=None):
    """Prepare strict sources outside every pyperf worker's timed lifetime."""
    sources = _strict_source_tools()
    script = Path(benchmark.runscript).resolve()
    metadata = {
        "soac_pyperformance_language": "ordinary",
        "soac_pyperformance_stock_source_fingerprint": sources.stock_source_fingerprint(
            script
        ),
    }
    python = kwargs.get("python", args[0] if args else None)
    venv = kwargs.get("venv")
    if venv is not None and python == sys.executable:
        python = venv.python
    local_tools = _local_package_tools()
    local_packages = local_tools.resolve(benchmark, sources)
    if local_packages is not None:
        if (
            venv is None
            or not python
            or Path(python).absolute() != Path(venv.python).absolute()
        ):
            raise ValueError(
                "declared local packages require the actual prepared benchmark venv"
            )
        local_packages = local_tools.for_environment(local_packages, venv)
        ready_packages = local_tools.require_ready(
            venv,
            local_packages,
            _visible_package_roots(Path(venv.root), dict(os.environ)),
        )
        metadata["soac_pyperformance_local_packages_fingerprint"] = ready_packages[
            "fingerprint"
        ]
    if os.environ.get("SOAC_PYPERFORMANCE_ENABLE", "").lower() not in {
        "1",
        "true",
        "yes",
        "on",
    }:
        yield metadata
        return

    if not python:
        raise ValueError("strict benchmark has no selected Python interpreter")
    # Benchmark.run's _prep_cmd copies the driver's environment; venv._env is
    # only used while installing requirements. Updating the latter would leave
    # real workers without a bundle even though a mock runner accepted it.
    environment = os.environ
    work_root = environment.get("SOAC_WORK_DIR")
    checker = os.environ.get("SOAC_PYPERFORMANCE_CHECKER")
    if not work_root or not checker:
        raise ValueError(
            "strict pyperformance requires a work directory and prebuilt offline checker"
        )
    key = _sha256(str(script).encode())[:16]
    directory = Path(work_root) / "strict-sources" / key
    previous_logs = {}
    if report is not None:
        report.stage(benchmark.name, "strict_preparation")
        for filename in ("checker.stdout.log", "checker.stderr.log"):
            path = directory / filename
            if path.is_file():
                info = path.stat()
                previous_logs[filename] = (
                    info.st_mtime_ns,
                    info.st_ctime_ns,
                    info.st_size,
                )
    try:
        bundle = sources.prepare_strict_benchmark(
            script, Path(python), directory, Path(checker), environment
        )
    finally:
        if report is not None:
            report.preserve_checker_logs(benchmark.name, directory, previous_logs)
    metadata.update(
        {
            "soac_pyperformance_language": "strict",
            "soac_pyperformance_strict_source_fingerprint": bundle[
                "source_fingerprint"
            ],
            "soac_pyperformance_selection_policy": bundle["selection_policy"],
            "soac_pyperformance_harness_policy": bundle["source"]["harness_projection"][
                "policy"
            ],
        }
    )
    name = "SOAC_PYPERFORMANCE_STRICT_BUNDLE"
    previous = environment.get(name)
    environment[name] = bundle["manifest_path"]
    try:
        yield metadata
    finally:
        if previous is None:
            environment.pop(name, None)
        else:
            environment[name] = previous


def main() -> Any:
    _inherit_installer_environment(sys.argv, os.environ)
    if os.environ.get("SOAC_PYPERFORMANCE_ENABLE", "").lower() in {
        "1",
        "true",
        "yes",
        "on",
    }:
        _inherit_environment_names(
            sys.argv,
            [
                "SOAC_PYPERFORMANCE_STRICT_BUNDLE",
                "HOME",
                "XDG_CONFIG_HOME",
                "LD_LIBRARY_PATH",
            ],
        )
    output = _output_from_arguments(sys.argv[1:])
    report = BenchmarkRunReport(output) if output is not None else None
    install_requirement_cache()
    install_benchmark_driver_provenance(report)
    if report is not None:
        install_benchmark_run_reporting(report)
    try:
        result = cli.main()
    except SystemExit as error:
        if report is not None:
            code = (
                error.code
                if isinstance(error.code, int)
                else int(error.code is not None)
            )
            report.finish(code)
        raise
    except BaseException as error:
        if report is not None:
            report.finish(130 if isinstance(error, KeyboardInterrupt) else 1, error)
        raise
    if report is not None:
        report.finish(result if isinstance(result, int) else 0)
    return result


if __name__ == "__main__":
    sys.exit(main())
