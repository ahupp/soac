#!/usr/bin/env python3
"""Verify SOAC's committed CPython gitlink without modifying selected sources.

Git supplies canonical checkout/filter bytes in an uncached disposable checkout.
There is no applied patch list, source stamp, expected-manifest cache, fetch,
selected-checkout update, reset, or build selection.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import sys

if __package__:
    from . import committed_source
    from . import cpython_sources as sources
else:
    import committed_source
    import cpython_sources as sources


CPythonEnvironmentError = sources.CPythonEnvironmentError
source_directory = sources.source_directory
source_lock = sources.source_lock
pinned_revision = sources.pinned_revision
# These pure data helpers use the one shared representation. The higher-level
# revision helpers below add CPython's committed-ignore/build-record policy.
canonical = committed_source.canonical
relative_path = committed_source.relative_path
checkout_digest = committed_source.checkout_digest


def _native_call(function, *arguments, **options):
    try:
        return function(*arguments, **options)
    except committed_source.CommittedSourceError as error:
        raise CPythonEnvironmentError(f"CPython source: {error}") from error


def git_bytes(source: Path, *arguments: str) -> bytes:
    return _native_call(committed_source.git_bytes, source, *arguments)


def commit_tree(source: Path, revision: str) -> str:
    return _native_call(committed_source.commit_tree, source, revision)


def inventory(source: Path, revision: str) -> dict[str, object]:
    return _native_call(committed_source.inventory, source, revision)


@contextmanager
def canonical_checkout(source: Path, revision: str):
    try:
        with committed_source.canonical_checkout(source, revision) as staged:
            yield staged
    except committed_source.CommittedSourceError as error:
        raise CPythonEnvironmentError(f"CPython source: {error}") from error


def revision_record(source: Path, revision: str) -> dict[str, object]:
    return _native_call(
        committed_source.revision_record, source, revision,
        allowed_untracked=sources._BUILD_RECORD_NAMES, allow_committed_ignored=True,
    )


def verify_generated_untracked(source: Path, revision: str) -> None:
    # Generators intentionally change tracked output, but may not hide new
    # source files with selected or newly written ignore rules.
    _native_call(
        committed_source.verify_untracked, source, revision,
        allow_committed_ignored=True,
    )


def verify_revision_record(source: Path, record: dict[str, object]) -> None:
    _native_call(
        committed_source.verify_revision_record, source, record,
        allowed_untracked=sources._BUILD_RECORD_NAMES, allow_committed_ignored=True,
    )


def verify_record(repo: Path, source: Path, record: dict[str, object]) -> None:
    _native_call(
        committed_source.verify_gitlink_record, repo, "vendor/cpython", source, record,
        allowed_untracked=sources._BUILD_RECORD_NAMES, allow_committed_ignored=True,
    )


@contextmanager
def verified_source(repo: Path, source: Path, *, shared: bool = False):
    """Keep the exact old source lock through the caller's build or check."""
    repo, source = repo.resolve(), source.resolve()
    with source_lock(repo, source, shared=shared):
        record = _native_call(
            committed_source.gitlink_record, repo, "vendor/cpython", source,
            allowed_untracked=sources._BUILD_RECORD_NAMES, allow_committed_ignored=True,
        )
        yield record


def verify_source(repo: Path, source: Path) -> dict[str, object]:
    with verified_source(repo, source, shared=True) as record:
        return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--source", type=Path)
    parser.add_argument(
        "--check", action="store_true",
        help="explicit verification spelling; both forms are verify-only",
    )
    options = parser.parse_args()
    repo = options.repo.resolve()
    source = (options.source or source_directory(repo, os.environ)).resolve()
    try:
        record = verify_source(repo, source)
        print(json.dumps({
            "source": str(source), "source_identity": record["source_identity"],
            "tracked_files": len(record["files"]), "verified": True,
        }, sort_keys=True))
    except (CPythonEnvironmentError, committed_source.CommittedSourceError,
            OSError, subprocess.CalledProcessError) as error:
        print(f"CPython source verification: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
