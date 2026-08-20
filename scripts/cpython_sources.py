"""Shared source identity and path selection for CPython build tooling."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import tempfile
from collections.abc import Mapping
from contextlib import contextmanager
from pathlib import Path


if __package__:
    from . import committed_source
else:
    import committed_source


class CPythonEnvironmentError(RuntimeError):
    pass


# Keep the existing CPython tooling names while sharing one Git implementation.
git_command = committed_source.git_command
git_output = committed_source.git_output
_BUILD_RECORD_NAMES = (".soac-cpython-build.json", ".soac-cpython-build.tmp")


def pinned_revision(repo: Path) -> str:
    try:
        return committed_source.gitlink_revision(repo, "vendor/cpython")
    except committed_source.CommittedSourceError as error:
        raise CPythonEnvironmentError(f"CPython source: {error}") from error


def source_directory(repo: Path, environment: Mapping[str, str]) -> Path:
    return (repo / environment.get("CPYTHON_SOURCE_DIR", "vendor/cpython")).resolve()


def source_lock_path(repo: Path, source: Path) -> Path:
    # Retain the exact old patch-preparer lock path: older processes may still
    # hold it. The neighboring state.json is not read by source verification.
    key = hashlib.sha256(os.fsencode(source.resolve())).hexdigest()[:20]
    return repo / "work/cpython-patches" / key / "source.lock"


@contextmanager
def source_lock(repo: Path, source: Path, *, shared: bool = False):
    path = source_lock_path(repo, source)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+") as lock:
        try:
            fcntl.flock(
                lock, (fcntl.LOCK_SH if shared else fcntl.LOCK_EX) | fcntl.LOCK_NB
            )
        except BlockingIOError as error:
            raise CPythonEnvironmentError(
                "CPython source/build is already in use by another prepare/build command"
            ) from error
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def atomic_json(path: Path, record: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        dir=path.parent, prefix=f".{path.name}.", delete=False
    ) as file:
        temporary = Path(file.name)
        try:
            file.write((json.dumps(record, indent=2, sort_keys=True) + "\n").encode())
            file.flush()
            os.fsync(file.fileno())
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


def selected_build_directory(
    repo: Path, source: Path, environment: Mapping[str, str]
) -> Path:
    explicit = environment.get("CPYTHON_BUILD_DIR") or environment.get(
        "CPYTHON_LIB_DIR"
    )
    if explicit:
        return (repo / explicit).resolve()
    selection = repo / "work/cpython-selected-build.json"
    if not selection.exists():
        return source
    try:
        record = json.loads(selection.read_text())
        if (
            not isinstance(record, dict)
            or set(record) != {"schema_version", "source", "build"}
            or record["schema_version"] != 1
            or not isinstance(record["source"], str)
            or not isinstance(record["build"], str)
            or not Path(record["source"]).is_absolute()
            or not Path(record["build"]).is_absolute()
        ):
            raise ValueError("invalid selected build fields")
    except (OSError, ValueError) as error:
        raise CPythonEnvironmentError(
            f"invalid CPython build selection at {selection}"
        ) from error
    return (
        Path(record["build"]) if Path(record["source"]).resolve() == source else source
    )


def save_selected_build(repo: Path, source: Path, build: Path) -> None:
    build = build.resolve()
    # Checkout-owned build fixtures share the checkout's lifetime, even when the
    # checkout itself is temporary. Resolve aliases first so a checkout symlink
    # cannot disguise an external build that disappears on a VM restart.
    if not build.is_relative_to(repo.resolve()) and any(
        build.is_relative_to(root.resolve())
        for root in (Path("/tmp"), Path("/var/tmp"), Path(tempfile.gettempdir()))
    ):
        raise CPythonEnvironmentError(
            f"cannot save CPython build selection {build}: builds outside the "
            "checkout must persist across VM restarts; choose a directory outside "
            "system temporary storage, such as ~/.local/share/soac/builds, or use "
            "--no-select for a disposable build"
        )
    atomic_json(
        repo / "work/cpython-selected-build.json",
        {
            "schema_version": 1,
            "source": str(source.resolve()),
            "build": str(build),
        },
    )
