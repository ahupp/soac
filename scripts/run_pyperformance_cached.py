#!/usr/bin/env python3
"""Run pyperformance without reinstalling unchanged benchmark requirements."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shlex
import sys
from typing import Any, Callable

import pyperf
import pyperformance
from pyperformance import cli
from pyperformance.venv import REQUIREMENTS_FILE, Requirements, VenvForBenchmarks


_CACHE_SCHEMA_VERSION = 1
_CACHE_DIRECTORY = ".soac-pyperformance-ready-v1"
_VOLATILE_ENVIRONMENT_PREFIXES = ("SOAC_",)
_VOLATILE_ENVIRONMENT_NAMES = {"PYPERFORMANCE_RUNID"}


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
                sorted(library.glob("python*/site-packages"), key=lambda path: str(path))
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


def _cache_state(venv: Any, benchmark: Any) -> dict[str, Any]:
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

    return {
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


def _marker_path(venv: Any, benchmark: Any, environment_digest: str) -> Path:
    benchmark_name = "".join(
        char if char.isalnum() or char in {"-", "_", "."} else "_"
        for char in str(benchmark.name)
    ).strip("._-") or "benchmark"
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
    try:
        requested = _benchmark_requirements(benchmark)
        state = _cache_state(venv, benchmark)
        marker = _marker_path(venv, benchmark, state["environment_sha256"])
    except (OSError, UnicodeError, ValueError, TypeError, AttributeError):
        return original(venv, requirements)

    if _read_marker(marker) == state:
        print(f"(reusing validated benchmark requirements for {benchmark.name})")
        return requested

    result = original(venv, requirements)
    try:
        refreshed = _cache_state(venv, benchmark)
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


def main() -> Any:
    install_requirement_cache()
    return cli.main()


if __name__ == "__main__":
    sys.exit(main())
