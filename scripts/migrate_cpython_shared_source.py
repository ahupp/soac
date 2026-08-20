#!/usr/bin/env python3
"""Preserve both CPython checkouts while removing a Lima source bind overlay."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys

if __package__:
    from .cpython_environment import (
        CPythonEnvironmentError,
        git_command,
        git_output,
        mount_for,
        pinned_revision,
    )
else:
    from cpython_environment import (
        CPythonEnvironmentError,
        git_command,
        git_output,
        mount_for,
        pinned_revision,
    )


def without_source_bind(contents: str, guest_source: Path, shared_source: Path) -> str:
    """Remove exactly the requested bind, preserving every other fstab byte."""
    kept = []
    removed = 0
    for line in contents.splitlines(keepends=True):
        fields = line.split()
        if (
            fields
            and not fields[0].startswith("#")
            and len(fields) >= 4
            and fields[0] == str(guest_source)
            and fields[1] == str(shared_source)
            and fields[2] == "none"
            and "bind" in fields[3].split(",")
        ):
            removed += 1
        else:
            kept.append(line)
    if removed != 1:
        raise CPythonEnvironmentError(
            f"expected exactly one fstab bind from {guest_source} to {shared_source}; "
            f"found {removed}; no mount or fstab changes made"
        )
    return "".join(kept)


def require_clean_revision(source: Path, expected: str) -> None:
    if git_output(source, "rev-parse", "HEAD") != expected:
        raise CPythonEnvironmentError(f"unexpected CPython revision at {source}")
    if git_output(source, "status", "--porcelain=v1", "--untracked-files=normal"):
        raise CPythonEnvironmentError(
            f"CPython source work changed at {source}; preserve it first"
        )


def require_overlay(repo: Path, guest_source: Path) -> Path:
    source = repo / "vendor/cpython"
    if source.is_symlink() or not source.samefile(guest_source):
        raise CPythonEnvironmentError(
            f"{source} is not the specified guest source bind"
        )
    if mount_for(repo)["fstype"] != "virtiofs":
        raise CPythonEnvironmentError(
            "the repository must be the shared virtiofs checkout"
        )
    mount = mount_for(source)
    if Path(mount["target"]) != source or mount["fstype"] == "virtiofs":
        raise CPythonEnvironmentError(
            "the CPython path is not a separate guest-local mount"
        )
    return source


def stage(repo: Path, migration: Path, guest_source: Path, host_revision: str) -> None:
    require_overlay(repo, guest_source)
    revision = pinned_revision(repo)
    require_clean_revision(guest_source, revision)
    if migration.exists():
        raise CPythonEnvironmentError(
            f"migration directory already exists: {migration}; inspect it before retrying"
        )
    migration.mkdir(parents=True)
    staged = migration / "shared-source"
    subprocess.run(
        git_command(
            guest_source,
            "clone",
            "--no-hardlinks",
            "--no-checkout",
            "--",
            str(guest_source),
            str(staged),
        ),
        check=True,
    )
    subprocess.run(git_command(staged, "checkout", "--detach", revision), check=True)
    origin = git_output(
        repo,
        "config",
        "-f",
        str(repo / ".gitmodules"),
        "--get",
        "submodule.vendor/cpython.url",
    )
    subprocess.run(
        git_command(staged, "remote", "set-url", "origin", origin), check=True
    )
    require_clean_revision(staged, revision)
    record = {
        "repo": str(repo),
        "guest_source": str(guest_source),
        "source_revision": revision,
        "host_revision": host_revision,
        "promoted": False,
    }
    (migration / "migration.json").write_text(json.dumps(record, indent=2) + "\n")
    print(f"Staged clean pinned CPython {revision}: {staged}")
    print(f"Guest original retained: {guest_source}")
    print("No mount, fstab, or existing host-source changes made.")


def write_fstab(contents: str) -> None:
    # tee preserves the existing file's ownership/mode. Never run this script
    # itself as root: Git and all checkout artifacts remain owned by the user.
    subprocess.run(
        ["sudo", "tee", "/etc/fstab"],
        input=contents,
        text=True,
        stdout=subprocess.DEVNULL,
        check=True,
    )


def promote(repo: Path, migration: Path, *, fstab: Path = Path("/etc/fstab")) -> None:
    record_path = migration / "migration.json"
    record = json.loads(record_path.read_text())
    if record["repo"] != str(repo) or record["promoted"]:
        raise CPythonEnvironmentError(
            "migration record is for another repo or already promoted"
        )
    guest_source = Path(record["guest_source"])
    source = require_overlay(repo, guest_source)
    revision = pinned_revision(repo)
    if revision != record["source_revision"]:
        raise CPythonEnvironmentError("the CPython pin changed after staging")
    require_clean_revision(guest_source, revision)
    staged = migration / "shared-source"
    require_clean_revision(staged, revision)
    host_backup = migration / "host-original"
    if host_backup.exists():
        raise CPythonEnvironmentError(
            f"host-source backup already exists: {host_backup}"
        )
    original = fstab.read_text()
    replacement = without_source_bind(original, guest_source, source)
    backup = migration / "fstab.before"
    if backup.exists() and backup.read_text() != original:
        raise CPythonEnvironmentError(
            "fstab changed since an earlier promotion attempt"
        )
    backup.write_text(original)
    (migration / "fstab.after").write_text(replacement)
    if fstab.read_text() != original:
        raise CPythonEnvironmentError(
            "fstab changed during preflight; retry after inspecting it"
        )
    write_fstab(replacement)
    try:
        # No force/lazy unmount: the kernel must reject a busy source mount.
        subprocess.run(["sudo", "umount", "--", str(source)], check=True)
    except BaseException:
        write_fstab(original)
        raise
    try:
        require_clean_revision(source, record["host_revision"])
        if mount_for(source)["fstype"] != "virtiofs":
            raise CPythonEnvironmentError(
                "removing the overlay did not reveal shared sources"
            )
        source.rename(host_backup)
        try:
            staged.rename(source)
        except BaseException:
            host_backup.rename(source)
            raise
    except BaseException:
        subprocess.run(
            ["sudo", "mount", "--bind", str(guest_source), str(source)], check=True
        )
        write_fstab(original)
        raise
    require_clean_revision(source, revision)
    record["promoted"] = True
    record_path.write_text(json.dumps(record, indent=2) + "\n")
    print(f"Shared CPython source: {source} ({revision})")
    print(f"Old host source retained: {host_backup}")
    print(f"Old guest source/build retained: {guest_source}")
    print(f"Original fstab retained: {backup}")
    print("Select a NEW guest-local CPYTHON_BUILD_DIR and run 'just build-python'.")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("stage", "promote"))
    parser.add_argument(
        "--repo", type=Path, default=Path(__file__).resolve().parents[1]
    )
    parser.add_argument("--migration-dir", type=Path)
    parser.add_argument("--guest-source", type=Path)
    parser.add_argument("--host-revision")
    options = parser.parse_args()
    repo = options.repo.resolve()
    migration = (
        options.migration_dir or repo / "work/cpython-source-migration"
    ).resolve()
    if not migration.is_relative_to((repo / "work").resolve()):
        parser.error(
            "--migration-dir must be inside this checkout's ignored work directory"
        )
    try:
        if options.command == "stage":
            if options.guest_source is None or options.host_revision is None:
                parser.error("stage requires --guest-source and --host-revision")
            stage(
                repo, migration, options.guest_source.resolve(), options.host_revision
            )
        else:
            if options.guest_source is not None or options.host_revision is not None:
                parser.error(
                    "promote reads the approved paths/revisions from the staging record"
                )
            promote(repo, migration)
    except (CPythonEnvironmentError, OSError, subprocess.CalledProcessError) as error:
        print(f"CPython source migration: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
