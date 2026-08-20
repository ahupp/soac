#!/usr/bin/env python3
"""Verify the committed Ruff/ty source selected by SOAC's actual gitlink.

This command never fetches, extracts, patches, resets, or repairs source. It
creates no source marker and grants no runtime or analysis authority by itself.
"""

from __future__ import annotations

import argparse
from collections.abc import Iterator
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess

if __package__:
    from .committed_source import CommittedSourceError as ToolchainError
    from .committed_source import gitlink_record
else:
    from committed_source import CommittedSourceError as ToolchainError
    from committed_source import gitlink_record


RUFF_SUBMODULE = "vendor/ruff"


def source_lock_path(root: Path, source: Path) -> Path:
    """One lock for the resolved source path, shared by readers and migrations."""
    identity = hashlib.sha256(os.fsencode(source.resolve())).hexdigest()
    return root / "work/ty-source" / identity / "source.lock"


@contextmanager
def source_lock(root: Path, source: Path, *, exclusive: bool = False) -> Iterator[None]:
    """Hold once around verification/use/recheck; helpers never reacquire it.

    Normal checker/preparation callers use a shared lock. Authorized source or
    gitlink migration uses the exclusive mode; neither mode silently waits.
    This is not the outer strict-integration checker-build serialization lock.
    """
    path = source_lock_path(root, source)
    path.parent.mkdir(parents=True, exist_ok=True)
    operation = fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH
    with path.open("a+b") as lock:
        try:
            fcntl.flock(lock, operation | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ToolchainError(
                f"Ruff source is busy: {source}; another reader or source/pin "
                f"migration holds {path}; retry after it finishes"
            ) from error
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def prepare_toolchain(root: Path) -> tuple[Path, dict[str, object]]:
    """Return a freshly verified in-memory core record; caller holds source_lock."""
    source = (root / RUFF_SUBMODULE).resolve()
    record = gitlink_record(root, RUFF_SUBMODULE, source)
    if (root / RUFF_SUBMODULE).resolve() != source:
        raise ToolchainError("declared Ruff source path changed during verification")
    return source, record


def checker_fingerprint(record: dict[str, object]) -> str:
    """Fingerprint the core's committed identity, not an on-disk cache claim."""
    identity = json.dumps(
        record["source_identity"], sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(b"SOAC-TY-CHECKER-v2\0" + identity).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    try:
        locked_source = (root / RUFF_SUBMODULE).resolve()
        with source_lock(root, locked_source):
            source, record = prepare_toolchain(root)
            if source != locked_source:
                raise ToolchainError("declared Ruff source path changed before verification")
            print(json.dumps({
                "schema_version": 2,
                "source": str(source),
                "source_identity": record["source_identity"],
                "checker_source_fingerprint": checker_fingerprint(record),
                "source_files": len(record["files"]),
            }, sort_keys=True))
    except (ToolchainError, OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"checker source verification failed: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
