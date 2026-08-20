"""Shared uncached identity checks for committed Git submodule sources.

Callers own their interoperable source locks. A returned record is transient
data derived from immutable Git objects, never a saved manifest or an execution
capability. This module does not fetch, update a selected checkout, or select a
build. Disposable local object sharing is not remote/CI availability evidence.
"""
from __future__ import annotations

from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import stat
import subprocess
import tempfile


class CommittedSourceError(RuntimeError):
    pass


def git_command(directory: Path, *arguments: str) -> list[str]:
    # Trust only this task-selected checkout for this invocation, never global/*.
    directory = directory.resolve()
    return [
        "git",
        "-c",
        f"safe.directory={directory}",
        "-C",
        str(directory),
        *arguments,
    ]


def _git_environment() -> dict[str, str]:
    # The expected bytes must not depend on this checkout's info/attributes,
    # hooks, global attributes/filter configuration, or caller index/worktree
    # overrides. Git's built-in conversion uses the committed attributes.
    environment = {
        name: value for name, value in os.environ.items()
        if name not in {
            "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_CONFIG", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT",
            "GIT_ATTR_SOURCE", "GIT_TEMPLATE_DIR",
        }
        and not name.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))
    }
    environment.update({
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_ATTR_NOSYSTEM": "1", "GIT_NO_REPLACE_OBJECTS": "1",
        "GIT_ALLOW_PROTOCOL": "file", "GIT_OPTIONAL_LOCKS": "0",
    })
    return environment


def git_bytes(source: Path, *arguments: str) -> bytes:
    return subprocess.check_output(
        git_command(source, *arguments), env=_git_environment(),
    )


def git_output(source: Path, *arguments: str) -> str:
    return git_bytes(source, *arguments).decode().strip()


def gitlink_revision(repo: Path, submodule: str) -> str:
    """Read the recorded JJ working revision, or the index in a plain Git repo.

    A colocated JJ workspace exports its parent to the Git index. Reading that
    index would silently verify the wrong toolchain after a working-revision
    rewrite. Query recorded state without snapshotting or changing the checkout;
    failure to read JJ is never permission to fall back to the parent index.
    """
    relative_path(submodule)
    repo = repo.resolve()
    if (repo / ".jj").exists():
        command = [
            "jj", "--repository", str(repo), "--ignore-working-copy",
            "--no-pager", "--color", "never", "log", "--no-graph",
            "-r", "@", "-T", "commit_id",
        ]
        try:
            revision = subprocess.check_output(
                command, env=_git_environment(), stderr=subprocess.PIPE,
            ).strip()
        except (OSError, subprocess.CalledProcessError) as error:
            details: dict[str, object] = {
                "command": shlex.join(command), "root": str(repo), "submodule": submodule,
            }
            if isinstance(error, subprocess.CalledProcessError):
                details.update(
                    returncode=error.returncode,
                    stdout=error.stdout or b"", stderr=error.stderr or b"",
                )
            else:
                details["os_error"] = f"{type(error).__name__}: {error}"
            # Keep local command evidence, not environment settings. Bound each
            # text field before decoding and escape child controls/newlines so
            # callers can retain one safe diagnostic line without losing cause.
            for name, value in details.items():
                if isinstance(value, (bytes, str)):
                    shortened = value[:4096]
                    if isinstance(shortened, bytes):
                        shortened = shortened.decode("utf-8", errors="replace")
                    details[name] = shortened + (" [truncated]" if len(value) > 4096 else "")
            raise CommittedSourceError(
                "cannot read Jujutsu working-revision submodule pin; "
                + json.dumps(details, sort_keys=True)
            ) from error
        if not re.fullmatch(b"[0-9a-f]{40}|[0-9a-f]{64}", revision):
            raise CommittedSourceError("invalid Jujutsu working-revision submodule pin")
        raw_entries = git_bytes(repo, "ls-tree", "-z", revision.decode(), "--", submodule)
        expected_kind = b"commit"
    else:
        raw_entries = git_bytes(repo, "ls-files", "--stage", "-z", "--", submodule)
        expected_kind = b"0"
    entries = [
        entry for entry in raw_entries.split(b"\0") if entry
    ]
    if len(entries) == 1:
        header, path = entries[0].split(b"\t", 1)
        fields = header.split()
        if expected_kind == b"commit" and len(fields) == 3:
            # ls-tree is mode/kind/oid; ls-files is mode/oid/stage.
            fields = [fields[0], fields[2], fields[1]]
        if (
            len(fields) == 3 and fields[0] == b"160000" and fields[2] == expected_kind
            and path == os.fsencode(submodule)
            and re.fullmatch(b"[0-9a-f]{40}|[0-9a-f]{64}", fields[1])
        ):
            return fields[1].decode()
    raise CommittedSourceError(f"{submodule} must have one resolved Git submodule pin")


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def relative_path(name: str) -> Path:
    path = PurePosixPath(name)
    if (
        not name or path.is_absolute() or str(path) != name
        or any(part in ("", ".", "..", ".git") for part in path.parts)
        or "\\" in name or any(ord(character) < 32 for character in name)
    ):
        raise CommittedSourceError(f"invalid committed source path: {name!r}")
    return Path(*path.parts)


def require_local_objects(source: Path) -> None:
    # Partial-clone object access can trigger an implicit fetch that writes the
    # selected object database. This verifier never acquires missing objects.
    result = subprocess.run(
        git_command(
            source, "config", "--local", "--get-regexp",
            r"^(extensions\.partialclone|remote\..*\.promisor)$",
        ),
        env=_git_environment(), capture_output=True,
    )
    if result.returncode == 0:
        raise CommittedSourceError(
            "partial/promisor source checkouts are unsupported by verify-only tooling; "
            "supply a complete local checkout"
        )
    if result.returncode != 1:
        raise subprocess.CalledProcessError(
            result.returncode, result.args, output=result.stdout, stderr=result.stderr,
        )


def commit_tree(source: Path, revision: str) -> str:
    require_local_objects(source)
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", revision):
        raise CommittedSourceError("committed source revision must be a full commit object ID")
    actual = git_bytes(source, "rev-parse", "--verify", f"{revision}^{{commit}}").strip().decode()
    if actual != revision:
        raise CommittedSourceError("committed source revision is not the exact commit")
    return git_bytes(source, "rev-parse", f"{revision}^{{tree}}").strip().decode()


# Git performs committed attribute/filter conversion. No custom filter
# configuration is copied from the selected source. Linux's native LF default
# and explicit committed CRLF/encoding attributes form this checkout policy.
_CANONICAL_OPTIONS = (
    "-c", "core.autocrlf=false", "-c", "core.eol=lf",
    "-c", "core.symlinks=true", "-c", f"core.attributesFile={os.devnull}",
    "-c", f"core.hooksPath={os.devnull}", "-c", f"core.excludesFile={os.devnull}",
)


@contextmanager
def canonical_checkout(source: Path, revision: str):
    """Use immutable objects in a disposable, never-published Git checkout.

    Object sharing here is only an ephemeral local read. It does not establish
    that the pinned commit is available through a clean remote checkout or CI.
    """
    source = source.resolve()
    commit_tree(source, revision)
    environment = _git_environment()
    with tempfile.TemporaryDirectory(prefix="soac-committed-source-") as temporary:
        staged = Path(temporary) / "source"
        subprocess.run(
            git_command(
                source, *_CANONICAL_OPTIONS,
                "clone", "--shared", "--no-checkout", "--template=",
                "--", str(source), str(staged),
            ),
            env=environment, capture_output=True, check=True,
        )
        subprocess.run(
            git_command(staged, *_CANONICAL_OPTIONS, "checkout", "--detach", revision),
            env=environment, capture_output=True, check=True,
        )
        yield staged


def tree_entries(source: Path, revision: str) -> dict[str, tuple[str, str]]:
    """Read immutable blob identity/mode, without checkout or clean filters."""
    result = {}
    for entry in git_bytes(source, "ls-tree", "-rz", "--full-tree", revision).split(b"\0"):
        if not entry:
            continue
        header, name = entry.split(b"\t", 1)
        mode, kind, oid = header.split()
        if mode not in (b"100644", b"100755", b"120000") or kind != b"blob":
            raise CommittedSourceError(
                "committed checkout contains an unsupported tree entry or nested submodule"
            )
        decoded = os.fsdecode(name)
        relative_path(decoded)
        result[decoded] = (mode.decode(), oid.decode())
    return dict(sorted(result.items()))


def tracked_paths(source: Path, revision: str) -> list[str]:
    return list(tree_entries(source, revision))


def index_entries(source: Path) -> dict[str, tuple[str, str]]:
    """Read the real index, not a diff that could execute a selected clean filter."""
    result = {}
    for entry in git_bytes(
        source, "-c", "core.fsmonitor=false", "ls-files", "--stage", "-z",
    ).split(b"\0"):
        if not entry:
            continue
        header, name = entry.split(b"\t", 1)
        mode, oid, stage = header.split()
        decoded = os.fsdecode(name)
        relative_path(decoded)
        if stage != b"0" or decoded in result:
            raise CommittedSourceError("checkout has unresolved/staged source entries")
        result[decoded] = (mode.decode(), oid.decode())
    return result


def file_identity(source: Path, name: str) -> dict[str, object]:
    path = source / relative_path(name)
    for parent in path.parents:
        if parent == source:
            break
        if parent.is_symlink():
            raise CommittedSourceError(f"committed checkout bytes have a symlink parent: {name}")
    try:
        before = path.lstat()
        if stat.S_ISLNK(before.st_mode):
            value = os.fsencode(os.readlink(path))
            digest = hashlib.sha256(value).hexdigest()
            size = len(value)
            mode = "120000"
        elif stat.S_ISREG(before.st_mode):
            hasher = hashlib.sha256()
            with path.open("rb") as contents:
                while chunk := contents.read(1024 * 1024):
                    hasher.update(chunk)
            digest = hasher.hexdigest()
            size = before.st_size
            executable = before.st_mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
            if executable and not executable & stat.S_IXUSR:
                raise CommittedSourceError(
                    f"committed checkout bytes have group/other-only executable mode: {name}"
                )
            # Git records the owner executable bit. Restrictive umasks may
            # legitimately remove group/other execution from a tracked script.
            mode = "100755" if executable else "100644"
        else:
            raise CommittedSourceError(f"committed checkout bytes are not a file/link: {name}")
        after = path.lstat()
    except OSError as error:
        raise CommittedSourceError(f"committed checkout bytes are unavailable: {name}") from error
    fields = ("st_dev", "st_ino", "st_mode", "st_size", "st_mtime_ns", "st_ctime_ns")
    if any(getattr(before, field) != getattr(after, field) for field in fields):
        raise CommittedSourceError(f"committed source changed while hashing checkout bytes: {name}")
    return {"mode": mode, "size": size, "sha256": digest}


def inventory(source: Path, revision: str) -> dict[str, dict[str, object]]:
    return {name: file_identity(source, name) for name in tracked_paths(source, revision)}


def checkout_digest(files: dict[str, object]) -> str:
    return hashlib.sha256(canonical({
        "schema_version": 1,
        "checkout_policy": "git-attributes-linux-lf-v1",
        "files": files,
    })).hexdigest()


def check_revision(source: Path, revision: str) -> None:
    if git_bytes(source, "rev-parse", "HEAD").strip().decode() != revision:
        raise CommittedSourceError(
            "committed checkout differs from the tracked gitlink or explicit staging revision; "
            "preserve local work and synchronize the intended checkout"
        )
    if index_entries(source) != tree_entries(source, revision):
        raise CommittedSourceError(
            "checkout has staged source changes; verification never resets work"
        )


def _untracked_paths(source: Path) -> list[str]:
    # No --exclude-standard: selected info/global/untracked ignore rules are not
    # authority, and no Git dirtiness/clean-filter path is part of this check.
    paths = [
        os.fsdecode(name) for name in git_bytes(
            source, "-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false",
            "ls-files", "--others", "-z",
        ).split(b"\0") if name
    ]
    for name in paths:
        relative_path(name)
    return sorted(paths)


def _check_untracked(
    source: Path, canonical_source: Path | None, *,
    allowed_untracked: tuple[str, ...] = (),
) -> None:
    allowed = set(allowed_untracked)
    for name in allowed:
        relative_path(name)
    candidates = [name for name in _untracked_paths(source) if name not in allowed]
    if not candidates:
        return
    if canonical_source is not None:
        # Only committed .gitignore files exist in this pristine checkout.
        # No selected info/exclude, global excludes, or untracked .gitignore is
        # copied. These ignored outputs are NOT part of the source identity.
        result = subprocess.run(
            git_command(
                canonical_source, *_CANONICAL_OPTIONS,
                "check-ignore", "--no-index", "--stdin", "-z",
            ),
            input=b"".join(os.fsencode(name) + b"\0" for name in candidates),
            env=_git_environment(), capture_output=True,
        )
        if result.returncode not in (0, 1):
            raise subprocess.CalledProcessError(
                result.returncode, result.args, output=result.stdout, stderr=result.stderr,
            )
        ignored = {os.fsdecode(name) for name in result.stdout.split(b"\0") if name}
        candidates = [name for name in candidates if name not in ignored]
    if candidates:
        raise CommittedSourceError(
            "checkout has uncommitted/untracked source files; preserve work before "
            f"verification: {candidates[:5]}"
        )


def verify_untracked(
    source: Path, revision: str, *, allowed_untracked: tuple[str, ...] = (),
    allow_committed_ignored: bool = False,
) -> None:
    """Check only untracked paths, including a generator's intentionally dirty tree.

    This helper is NOT full source verification. Ignore permission, when
    requested, is rederived from an untouched canonical committed checkout.
    """
    source = source.resolve()
    if allow_committed_ignored:
        with canonical_checkout(source, revision) as staged:
            _check_untracked(source, staged, allowed_untracked=allowed_untracked)
    else:
        _check_untracked(source, None, allowed_untracked=allowed_untracked)


def revision_record(
    source: Path, revision: str, *, allowed_untracked: tuple[str, ...] = (),
    allow_committed_ignored: bool = False,
) -> dict[str, object]:
    """Derive and check uncached commit data; callers must keep their source lock.

    The default rejects ALL untracked files. CPython explicitly allows committed
    ignores for in-source build products; their build/runtime provenance remains
    a separate obligation and is never established by this source record.
    """
    source = source.resolve()
    tree = commit_tree(source, revision)
    check_revision(source, revision)
    with canonical_checkout(source, revision) as staged:
        files = inventory(staged, revision)
        actual = inventory(source, revision)
        if actual != files:
            changed = sorted(
                name for name in set(actual) | set(files)
                if actual.get(name) != files.get(name)
            )
            raise CommittedSourceError(
                "committed checkout bytes/modes differ from the canonical committed tree: "
                f"{changed[:5]}"
            )
        _check_untracked(
            source, staged if allow_committed_ignored else None,
            allowed_untracked=allowed_untracked,
        )
    check_revision(source, revision)
    return {
        "revision": revision, "tree": tree,
        "checkout_sha256": checkout_digest(files), "files": files,
    }


def verify_revision_record(
    source: Path, record: dict[str, object], *, allowed_untracked: tuple[str, ...] = (),
    allow_committed_ignored: bool = False,
) -> None:
    """Rederive Git expectations; even a supplied record is never an authority."""
    if not isinstance(record, dict) or set(record) != {
        "revision", "tree", "checkout_sha256", "files",
    }:
        raise CommittedSourceError("invalid committed source record")
    actual = revision_record(
        source, record["revision"], allowed_untracked=allowed_untracked,
        allow_committed_ignored=allow_committed_ignored,
    )
    if actual != record:
        raise CommittedSourceError(
            "committed source record differs from freshly derived Git identity"
        )


def gitlink_record(
    repo: Path, submodule: str, source: Path, *,
    allowed_untracked: tuple[str, ...] = (),
    allow_committed_ignored: bool = False,
) -> dict[str, object]:
    """Authenticate the transient inventory against the caller's actual gitlink."""
    revision = gitlink_revision(repo, submodule)
    snapshot = revision_record(
        source, revision, allowed_untracked=allowed_untracked,
        allow_committed_ignored=allow_committed_ignored,
    )
    if gitlink_revision(repo, submodule) != revision:
        raise CommittedSourceError("tracked gitlink changed during verification")
    return {
        "source_identity": {
            "kind": "gitlink", "revision": revision, "tree": snapshot["tree"],
            "checkout_sha256": snapshot["checkout_sha256"],
        },
        "files": snapshot["files"],
    }


def verify_gitlink_record(
    repo: Path, submodule: str, source: Path, record: dict[str, object], *,
    allowed_untracked: tuple[str, ...] = (),
    allow_committed_ignored: bool = False,
) -> None:
    """Recheck a current in-memory record while the caller still holds its lock."""
    if not isinstance(record, dict) or set(record) != {"source_identity", "files"}:
        raise CommittedSourceError("invalid gitlink source record")
    identity = record["source_identity"]
    if (
        not isinstance(identity, dict)
        or set(identity) != {"kind", "revision", "tree", "checkout_sha256"}
        or identity["kind"] != "gitlink"
        or gitlink_revision(repo, submodule) != identity["revision"]
    ):
        raise CommittedSourceError("committed source identity differs from the tracked gitlink")
    verify_revision_record(source, {
        "revision": identity["revision"], "tree": identity["tree"],
        "checkout_sha256": identity["checkout_sha256"], "files": record["files"],
    }, allowed_untracked=allowed_untracked, allow_committed_ignored=allow_committed_ignored)
    if gitlink_revision(repo, submodule) != identity["revision"]:
        raise CommittedSourceError("tracked gitlink changed during verification")
