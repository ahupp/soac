from __future__ import annotations

import hashlib
import json
import os
import shlex
from pathlib import Path
import subprocess
import sys

import pytest

from scripts import prepare_cpython_source as preparation
from scripts import regenerate_cpython_cases as regeneration
from scripts.cpython_sources import CPythonEnvironmentError, git_command, git_output


def git(directory: Path, *arguments: str) -> None:
    subprocess.run(git_command(directory, *arguments), check=True, capture_output=True)


def commit_source(source: Path, message: str) -> str:
    git(source, "add", ".")
    git(source, "-c", "user.name=CPython test",
        "-c", "user.email=cpython-test@example.invalid",
        "commit", "--quiet", "-m", message)
    return git_output(source, "rev-parse", "HEAD")


def pin_gitlink(repo: Path, submodule: str, revision: str) -> None:
    git(repo, "update-index", "--add", "--cacheinfo", f"160000,{revision},{submodule}")


def pin_source(repo: Path, revision: str) -> None:
    pin_gitlink(repo, "vendor/cpython", revision)


def make_committed_checkout(
    tmp_path: Path, submodule: str = "vendor/cpython",
) -> tuple[Path, Path]:
    """Real tiny commit/gitlink fixture shared by CPython and Ruff tooling tests."""
    repo = tmp_path / "repo"
    source = repo / submodule
    source.mkdir(parents=True)
    git(repo, "init", "--quiet")
    git(source, "init", "--quiet")
    (source / "a.h").write_text("value = 1\n")
    (source / "b.h").write_text("other = 1\n")
    (source / ".gitignore").write_text("ignored.txt\n")
    (source / ".gitattributes").write_text("*.vcxproj text eol=crlf\n")
    (source / "project.vcxproj").write_bytes(b"\xef\xbb\xbfbefore\r\n")
    (source / "binary.dat").write_bytes(b"\0\xffnative\n")
    executable = source / "executable.sh"
    executable.write_text("#!/bin/sh\nexit 0\n")
    executable.chmod(0o755)
    (source / "header-link").symlink_to("a.h")
    pin_gitlink(repo, submodule, commit_source(source, "Fixture source"))
    return repo, source


@pytest.fixture
def checkout(tmp_path: Path) -> tuple[Path, Path]:
    return make_committed_checkout(tmp_path)


def test_gitlink_alone_selects_complete_committed_native_sources(checkout) -> None:
    repo, source = checkout
    assert not (repo / "tools/cpython/toolchain.json").exists()
    before = git_output(source, "status", "--porcelain=v1")
    record = preparation.verify_source(repo, source)
    identity = record["source_identity"]
    assert identity["kind"] == "gitlink"
    assert identity["revision"] == git_output(source, "rev-parse", "HEAD")
    assert identity["tree"] == git_output(source, "rev-parse", "HEAD^{tree}")
    assert len(identity["checkout_sha256"]) == 64
    assert record["files"]["executable.sh"]["mode"] == "100755"
    assert record["files"]["header-link"] == {
        "mode": "120000", "size": 3,
        "sha256": hashlib.sha256(b"a.h").hexdigest(),
    }
    assert record["files"]["binary.dat"]["sha256"] == hashlib.sha256(
        b"\0\xffnative\n"
    ).hexdigest()
    assert preparation.verify_source(repo, source) == record
    assert git_output(source, "status", "--porcelain=v1") == before
    assert (source / "header-link").is_symlink()


def test_new_native_commit_requires_matching_gitlink_without_resetting_work(checkout) -> None:
    repo, source = checkout
    first = preparation.verify_source(repo, source)
    (source / "a.h").write_text("value = 2\n")
    second_revision = commit_source(source, "Logical native change")
    with pytest.raises(CPythonEnvironmentError, match="tracked gitlink"):
        preparation.verify_source(repo, source)
    assert (source / "a.h").read_text() == "value = 2\n"
    pin_source(repo, second_revision)
    second = preparation.verify_source(repo, source)
    assert first["source_identity"] != second["source_identity"]
    assert second["source_identity"]["revision"] == second_revision


@pytest.mark.parametrize("change", ["tracked", "staged", "untracked"])
def test_committed_source_refuses_and_preserves_uncommitted_native_work(checkout, change) -> None:
    repo, source = checkout
    target = source / ("new_hook.c" if change == "untracked" else "a.h")
    target.write_text("user work\n")
    if change == "staged":
        git(source, "add", "a.h")
    with pytest.raises(CPythonEnvironmentError, match="uncommitted|staged|checkout bytes"):
        preparation.verify_source(repo, source)
    assert target.read_text() == "user work\n"


def test_canonical_git_checkout_filters_not_blob_or_normalized_diff_define_bytes(checkout) -> None:
    repo, source = checkout
    record = preparation.verify_source(repo, source)
    target = source / "project.vcxproj"
    assert target.read_bytes() == b"\xef\xbb\xbfbefore\r\n"
    blob = subprocess.check_output(git_command(source, "show", "HEAD:project.vcxproj"))
    assert blob == b"\xef\xbb\xbfbefore\n"
    assert record["files"]["project.vcxproj"]["sha256"] == hashlib.sha256(
        target.read_bytes()
    ).hexdigest()
    target.write_bytes(blob)
    assert git_output(source, "diff", "HEAD", "--") == ""
    with pytest.raises(CPythonEnvironmentError, match="checkout bytes"):
        preparation.verify_source(repo, source)
    assert target.read_bytes() == blob, "verification must not normalize user bytes"


def test_canonical_modes_are_checked_even_when_local_git_ignores_filemode(checkout) -> None:
    repo, source = checkout
    git(source, "config", "core.fileMode", "false")
    target = source / "executable.sh"
    target.chmod(0o644)
    assert git_output(source, "diff", "HEAD", "--") == ""
    with pytest.raises(CPythonEnvironmentError, match="checkout bytes"):
        preparation.verify_source(repo, source)
    assert target.stat().st_mode & 0o111 == 0


def test_local_attribute_overrides_cannot_redefine_canonical_committed_bytes(checkout) -> None:
    repo, source = checkout
    attributes = source / ".git/info/attributes"
    attributes.write_text("*.vcxproj text eol=lf\n")
    (source / "project.vcxproj").write_bytes(b"\xef\xbb\xbfbefore\n")
    assert git_output(source, "diff", "HEAD", "--") == ""
    with pytest.raises(CPythonEnvironmentError, match="checkout bytes"):
        preparation.verify_source(repo, source)


def test_untracked_build_record_and_ignored_data_are_not_committed_source_authority(checkout) -> None:
    repo, source = checkout
    before = preparation.verify_source(repo, source)
    (source / ".soac-cpython-build.json").write_text("not a source authority")
    (source / "ignored.txt").write_text("user data")
    assert preparation.verify_source(repo, source) == before
    key = hashlib.sha256(os.fsencode(source.resolve())).hexdigest()[:20]
    forged = repo / "work/cpython-patches" / key / "state.json"
    forged.parent.mkdir(parents=True, exist_ok=True)
    forged.write_text(json.dumps({"generation": "forged", "files": {}}))
    (source / "a.h").write_text("uncommitted replacement\n")
    with pytest.raises(CPythonEnvironmentError, match="uncommitted|checkout bytes"):
        preparation.verify_source(repo, source)
    assert (source / "ignored.txt").read_text() == "user data"
    assert forged.read_text() == json.dumps({"generation": "forged", "files": {}})


def test_verification_rechecks_immutable_expected_bytes_without_cached_manifest(checkout) -> None:
    repo, source = checkout
    with preparation.verified_source(repo, source) as record:
        (source / "project.vcxproj").write_bytes(b"\xef\xbb\xbfbefore\n")
        with pytest.raises(CPythonEnvironmentError, match="checkout bytes"):
            preparation.verify_record(repo, source, record)


def test_source_lock_interoperates_with_old_patch_preparer_lock(checkout) -> None:
    import fcntl

    repo, source = checkout
    key = hashlib.sha256(os.fsencode(source.resolve())).hexdigest()[:20]
    legacy_lock = repo / "work/cpython-patches" / key / "source.lock"
    legacy_lock.parent.mkdir(parents=True, exist_ok=True)
    with legacy_lock.open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with pytest.raises(CPythonEnvironmentError, match="already in use"):
            preparation.verify_source(repo, source)
    with preparation.source_lock(repo, source, shared=True):
        with pytest.raises(CPythonEnvironmentError, match="already in use"):
            with preparation.verified_source(repo, source):
                pytest.fail("a build must not overlap a source reader")


def generated_checkout(checkout) -> tuple[Path, Path, str, str]:
    repo, source = checkout
    (source / "a.h").write_text("value = 2\n")
    logical = commit_source(source, "Logical native source")
    (source / "b.h").write_text("other = 3\n")
    generated = commit_source(source, "Generated cases only")
    pin_source(repo, generated)
    return repo, source, logical, generated


def test_generated_top_commit_is_reproduced_without_source_or_patch_publication(
    checkout, monkeypatch: pytest.MonkeyPatch,
) -> None:
    repo, source, logical, generated = generated_checkout(checkout)
    monkeypatch.setattr(regeneration, "GENERATED_FILES", ("b.h",))
    emitted = "other = 3\n"

    def generate(staged: Path, build: Path) -> None:
        assert staged != source
        assert (staged / "a.h").read_text() == "value = 2\n"
        assert (staged / "b.h").read_text() == "other = 1\n"
        (staged / "b.h").write_text(emitted)

    monkeypatch.setattr(regeneration, "generate_cases", generate)
    before = preparation.verify_source(repo, source)
    result = regeneration.regenerate(repo, source, check=True)
    assert result["input_revision"] == logical
    assert result["reference_revision"] == generated
    assert set(result["generated_files"]) == {"b.h"}
    output = repo / "work/generated-output"
    regeneration.regenerate(repo, source, output=output)
    assert (output / "b.h").read_text() == emitted
    assert (output / "generation.json").is_file()
    assert not list(output.rglob("*.patch"))
    assert preparation.verify_source(repo, source) == before
    emitted = "other = 4\n"
    with pytest.raises(CPythonEnvironmentError, match="generated.*stale|does not match"):
        regeneration.regenerate(repo, source, check=True)
    assert (output / "b.h").read_text() == "other = 3\n"


@pytest.mark.parametrize("change", ["non_generated", "live_input"])
def test_regeneration_preserves_failed_inputs_and_publishes_nothing(
    checkout, monkeypatch: pytest.MonkeyPatch, change: str,
) -> None:
    repo, source, _, _ = generated_checkout(checkout)
    monkeypatch.setattr(regeneration, "GENERATED_FILES", ("b.h",))
    output = repo / "work/failed-output"

    def generate(staged: Path, build: Path) -> None:
        (staged / "b.h").write_text("other = 3\n")
        target = staged if change == "non_generated" else source
        (target / "a.h").write_text("external change\n")

    monkeypatch.setattr(regeneration, "generate_cases", generate)
    with pytest.raises(CPythonEnvironmentError, match="non-generated|uncommitted|checkout bytes"):
        regeneration.regenerate(repo, source, output=output)
    assert not output.exists()
    expected = "external change\n" if change == "live_input" else "value = 2\n"
    assert (source / "a.h").read_text() == expected


def test_mixed_source_generated_top_is_not_a_regeneration_parent_guess(
    checkout, monkeypatch: pytest.MonkeyPatch,
) -> None:
    repo, source = checkout
    (source / "a.h").write_text("value = 2\n")
    (source / "b.h").write_text("other = 3\n")
    pin_source(repo, commit_source(source, "Invalid mixed top"))
    monkeypatch.setattr(regeneration, "GENERATED_FILES", ("b.h",))
    monkeypatch.setattr(
        regeneration, "generate_cases",
        lambda *_: pytest.fail("reject mixed commit before invoking the generator"),
    )
    with pytest.raises(CPythonEnvironmentError, match="generated-only"):
        regeneration.regenerate(repo, source, check=True)


def test_explicit_logical_generation_is_not_build_or_runtime_admission(
    checkout, monkeypatch: pytest.MonkeyPatch,
) -> None:
    repo, source, logical, generated = generated_checkout(checkout)
    git(source, "checkout", "--detach", logical)
    monkeypatch.setattr(regeneration, "GENERATED_FILES", ("b.h",))
    monkeypatch.setattr(
        regeneration, "generate_cases",
        lambda staged, _: (staged / "b.h").write_text("other = 3\n"),
    )
    output = repo / "work/unselected-generated-output"
    result = regeneration.regenerate(repo, source, revision=logical, output=output)
    assert result["input_revision"] == logical
    assert result["reference_revision"] is None
    assert git_output(source, "rev-parse", "HEAD") == logical
    assert git_output(repo, "ls-files", "--stage", "--", "vendor/cpython").split()[1] == generated
    with pytest.raises(CPythonEnvironmentError, match="tracked gitlink"):
        preparation.verify_source(repo, source)
    with pytest.raises(CPythonEnvironmentError, match="check.*revision|revision.*check"):
        regeneration.regenerate(repo, source, revision=logical, check=True)


def test_existing_generated_output_is_preserved_on_retry(
    checkout, monkeypatch: pytest.MonkeyPatch,
) -> None:
    repo, source, _, _ = generated_checkout(checkout)
    monkeypatch.setattr(regeneration, "GENERATED_FILES", ("b.h",))
    output = repo / "work/existing-output"
    output.mkdir(parents=True)
    (output / "user-data").write_text("preserve")
    monkeypatch.setattr(
        regeneration, "generate_cases",
        lambda *_: pytest.fail("do not regenerate over an existing output"),
    )
    with pytest.raises(CPythonEnvironmentError, match="already exists"):
        regeneration.regenerate(repo, source, output=output)
    assert (output / "user-data").read_text() == "preserve"


@pytest.mark.parametrize("submodule", ["vendor/cpython", "vendor/ruff"])
def test_shared_committed_source_core_uses_the_actual_requested_gitlink(checkout, submodule) -> None:
    from scripts import committed_source

    repo, original = checkout
    revision = git_output(original, "rev-parse", "HEAD")
    source = repo / submodule
    if source != original:
        original.rename(source)
        git(repo, "update-index", "--force-remove", "--", "vendor/cpython")
        git(repo, "update-index", "--add", "--cacheinfo", f"160000,{revision},{submodule}")
    record = committed_source.gitlink_record(repo, submodule, source)
    committed_source.verify_gitlink_record(repo, submodule, source, record)
    assert committed_source.gitlink_record(
        repo, submodule, Path(os.path.relpath(source)),
    ) == record
    assert record["source_identity"]["revision"] == revision
    assert record["files"]["header-link"]["mode"] == "120000"
    assert record["files"]["project.vcxproj"]["sha256"] == hashlib.sha256(
        b"\xef\xbb\xbfbefore\r\n"
    ).hexdigest()
    (source / ".old-archive-patch-marker.json").write_text('{"prepared":true}')
    with pytest.raises(committed_source.CommittedSourceError, match="uncommitted"):
        committed_source.gitlink_record(repo, submodule, source)


def make_jj_committed_checkout(tmp_path: Path, submodule: str):
    repo, source = make_committed_checkout(tmp_path, submodule)
    first = git_output(source, "rev-parse", "HEAD")
    commit_source(repo, "Original submodule pin")
    (source / "a.h").write_text("value = 2\n")
    second = commit_source(source, "Second source revision")
    pin_gitlink(repo, submodule, second)
    current = commit_source(repo, "Updated submodule pin")
    command = [
        "jj", "--no-pager", "--color", "never",
        "--config", "user.name=Source fixture",
        "--config", "user.email=source-fixture@example.invalid",
    ]
    environment = dict(os.environ, JJ_CONFIG=os.devnull)
    subprocess.run(
        [*command, "git", "init", "--colocate"], cwd=repo, env=environment,
        capture_output=True, check=True,
    )
    subprocess.run(
        [*command, "edit", current], cwd=repo, env=environment,
        capture_output=True, check=True,
    )
    # Colocated JJ exports the parent as Git HEAD/index. The working revision
    # itself is the new pin, and the verifier must not snapshot or rewrite it.
    assert git_output(repo, "ls-files", "--stage", "--", submodule).split()[1] == first
    return repo, source, first, second, current


@pytest.mark.parametrize("submodule", ["vendor/cpython", "vendor/ruff"])
def test_jj_source_pin_uses_recorded_working_revision_not_parent_index(tmp_path, submodule) -> None:
    from scripts import committed_source

    repo, source, first, second, current = make_jj_committed_checkout(tmp_path, submodule)
    assert committed_source.gitlink_revision(repo, submodule) == second
    record = committed_source.gitlink_record(repo, submodule, source)
    assert record["source_identity"]["revision"] == second
    assert git_output(repo, "ls-tree", current, "--", submodule).split()[2] == second
    assert git_output(repo, "ls-files", "--stage", "--", submodule).split()[1] == first

    # Neither stale index state nor changing it to match a different source
    # checkout grants authority to a pin that the JJ revision did not record.
    git(source, "checkout", "--quiet", "--detach", first)
    pin_gitlink(repo, submodule, first)
    with pytest.raises(committed_source.CommittedSourceError, match="tracked gitlink"):
        committed_source.gitlink_record(repo, submodule, source)


@pytest.mark.parametrize("failure", [
    "missing_jj", "query_error", "query_signal", "long_query_error", "invalid_revision",
])
def test_jj_pin_query_failure_never_falls_back_to_git_index(tmp_path, monkeypatch, failure) -> None:
    from scripts import committed_source

    repo, _, first, _, _ = make_jj_committed_checkout(tmp_path, "vendor/ruff")
    assert git_output(repo, "ls-files", "--stage", "--", "vendor/ruff").split()[1] == first
    calls = []
    injected = []

    def failed_query(command, **kwargs):
        calls.append(command)
        assert command[0] == "jj", "failed JJ query reached a Git-index fallback"
        assert "--ignore-working-copy" in command
        assert kwargs["stderr"] == subprocess.PIPE
        if failure == "invalid_revision":
            return b"not-one-commit\n"
        if failure == "missing_jj":
            error = FileNotFoundError(2, "jj not installed", "jj")
        else:
            stdout = b"partial revision\n"
            stderr = b"invalid workspace\n"
            if failure == "long_query_error":
                stdout += b"x" * 5000 + b"stdout-tail-must-be-truncated"
                stderr += b"\xff\x1b" + b"x" * 5000 + b"stderr-tail-must-be-truncated"
            error = subprocess.CalledProcessError(
                -9 if failure == "query_signal" else 7,
                command, output=stdout, stderr=stderr,
            )
        injected.append(error)
        raise error

    monkeypatch.setattr(subprocess, "check_output", failed_query)
    with pytest.raises(committed_source.CommittedSourceError, match="Jujutsu.*pin") as caught:
        committed_source.gitlink_revision(repo, "vendor/ruff")
    assert len(calls) == 1, "failed JJ pin queries must not retry"
    if failure == "invalid_revision":
        return
    message = str(caught.value)
    assert len(message.splitlines()) == 1, "child diagnostics must be escaped"
    details = json.loads(message.split("; ", 1)[1])
    assert details["command"] == shlex.join(calls[0])
    assert details["root"] == str(repo.resolve())
    assert details["submodule"] == "vendor/ruff"
    assert caught.value.__cause__ is injected[0]
    if failure == "missing_jj":
        assert details["os_error"].startswith("FileNotFoundError: [Errno 2]")
        assert "jj not installed" in details["os_error"]
        assert "returncode" not in details
    else:
        assert details["returncode"] == (-9 if failure == "query_signal" else 7)
        if failure == "long_query_error":
            for stream in ("stdout", "stderr"):
                assert details[stream].endswith(" [truncated]")
                assert len(details[stream]) <= 4096 + len(" [truncated]")
                assert "tail-must-be-truncated" not in details[stream]
            assert "\ufffd" in details["stderr"]
            assert "\x1b" not in message
        else:
            assert details["stdout"] == "partial revision\n"
            assert details["stderr"] == "invalid workspace\n"


def test_raw_symlink_target_remains_checked_when_git_skip_worktree_hides_it(checkout) -> None:
    repo, source = checkout
    git(source, "update-index", "--skip-worktree", "header-link")
    link = source / "header-link"
    link.unlink()
    link.symlink_to("b.h")
    assert git_output(source, "diff", "HEAD", "--") == ""
    with pytest.raises(CPythonEnvironmentError, match="checkout bytes"):
        preparation.verify_source(repo, source)
    assert os.readlink(link) == "b.h"


def test_verify_only_refuses_promisor_configuration_before_checkout_or_fetch(
    checkout, monkeypatch: pytest.MonkeyPatch,
) -> None:
    from scripts import committed_source

    repo, source = checkout
    git(source, "config", "remote.origin.promisor", "true")
    monkeypatch.setattr(
        committed_source, "canonical_checkout",
        lambda *_: pytest.fail("reject lazy-fetch source before any checkout"),
    )
    with pytest.raises(CPythonEnvironmentError, match="partial/promisor"):
        preparation.verify_source(repo, source)


def test_selected_clean_filter_is_never_called_by_source_verification(checkout) -> None:
    repo, source = checkout
    attributes = source / ".gitattributes"
    attributes.write_text(attributes.read_text() + "a.h filter=selected\n")
    pin_source(repo, commit_source(source, "Committed filter attribute without a local driver"))
    before = preparation.verify_source(repo, source)
    marker = repo / "clean-filter-ran"
    observer = repo / "observe-filter.py"
    observer.write_text(
        "from pathlib import Path\nimport sys\n"
        f"Path({str(marker)!r}).touch()\n"
        "sys.stdout.buffer.write(sys.stdin.buffer.read())\n"
    )
    command = shlex.join([sys.executable, str(observer)])
    git(source, "config", "filter.selected.clean", command)
    git(source, "config", "filter.selected.smudge", command)
    # Invalidate the stat cache without changing bytes. A diff-based verifier
    # would execute the selected clean driver to compare this tracked file.
    target = source / "a.h"
    target.write_bytes(target.read_bytes())
    current = target.stat()
    os.utime(target, ns=(current.st_atime_ns, current.st_mtime_ns + 2_000_000_000))
    assert preparation.verify_source(repo, source) == before
    preparation.verify_record(repo, source, before)
    assert not marker.exists(), "verification must not execute selected checkout filters"


@pytest.mark.parametrize("rule", ["info", "global", "untracked"])
def test_selected_ignore_rules_cannot_hide_new_source_files(
    checkout, monkeypatch: pytest.MonkeyPatch, rule: str,
) -> None:
    repo, source = checkout
    target = source / "new_hook.c"
    if rule == "info":
        (source / ".git/info/exclude").write_text("new_hook.c\n")
    elif rule == "global":
        excludes = repo / "global-ignore"
        excludes.write_text("new_hook.c\n")
        config = repo / "global-config"
        config.write_text(f"[core]\n\texcludesFile = {excludes}\n")
        monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(config))
    else:
        parent = source / "local"
        parent.mkdir()
        (parent / ".gitignore").write_text("*\n")
        target = parent / "new_hook.c"
    target.write_text("user source that Git's selected ignore rules hide\n")
    assert subprocess.check_output(
        git_command(source, "ls-files", "--others", "--exclude-standard"),
    ) == b""
    with pytest.raises(CPythonEnvironmentError, match="uncommitted/untracked"):
        preparation.verify_source(repo, source)
    assert target.read_text() == "user source that Git's selected ignore rules hide\n"


def test_shared_verifier_defaults_to_rejecting_even_committed_ignored_files(checkout) -> None:
    from scripts import committed_source

    repo, source = checkout
    before = committed_source.gitlink_record(repo, "vendor/cpython", source)
    (source / "ignored.txt").write_text("not part of source authority")
    with pytest.raises(committed_source.CommittedSourceError, match="uncommitted/untracked"):
        committed_source.gitlink_record(repo, "vendor/cpython", source)
    with pytest.raises(committed_source.CommittedSourceError, match="uncommitted/untracked"):
        committed_source.verify_gitlink_record(repo, "vendor/cpython", source, before)
    assert preparation.verify_source(repo, source) == before
    committed_source.verify_gitlink_record(
        repo, "vendor/cpython", source, before, allow_committed_ignored=True,
    )


@pytest.mark.parametrize(
    ("name", "mode"), [("a.h", 0o654), ("a.h", 0o645), ("executable.sh", 0o655)],
)
def test_group_or_other_only_execution_cannot_hide_in_git_filemode_policy(
    checkout, name: str, mode: int,
) -> None:
    repo, source = checkout
    git(source, "config", "core.fileMode", "false")
    target = source / name
    target.chmod(mode)
    with pytest.raises(CPythonEnvironmentError, match="group/other-only executable"):
        preparation.verify_source(repo, source)
    assert target.stat().st_mode & 0o777 == mode


def test_owner_only_execution_preserves_gits_tracked_executable_mode(checkout) -> None:
    repo, source = checkout
    before = preparation.verify_source(repo, source)
    (source / "executable.sh").chmod(0o744)
    assert preparation.verify_source(repo, source) == before


def test_raw_file_bytes_remain_checked_when_git_assume_unchanged_hides_them(checkout) -> None:
    repo, source = checkout
    git(source, "update-index", "--assume-unchanged", "a.h")
    target = source / "a.h"
    target.write_text("different source bytes\n")
    assert git_output(source, "diff", "HEAD", "--") == ""
    with pytest.raises(CPythonEnvironmentError, match="checkout bytes"):
        preparation.verify_source(repo, source)
    assert target.read_text() == "different source bytes\n"


@pytest.mark.parametrize("change_source", [False, True])
def test_supplied_record_cannot_replace_immutable_git_expectations(
    checkout, change_source: bool,
) -> None:
    from scripts import committed_source

    repo, source = checkout
    record = preparation.verify_source(repo, source)
    if change_source:
        (source / "a.h").write_text("forged but self-consistent checkout\n")
        record["files"]["a.h"] = committed_source.file_identity(source, "a.h")
    else:
        record["files"]["a.h"]["sha256"] = "0" * 64
    record["source_identity"]["checkout_sha256"] = committed_source.checkout_digest(
        record["files"],
    )
    with pytest.raises(CPythonEnvironmentError, match="checkout bytes|freshly derived"):
        preparation.verify_record(repo, source, record)


@pytest.mark.parametrize("hidden", [False, True])
def test_generation_cannot_introduce_new_sources_with_an_untracked_ignore_rule(
    checkout, monkeypatch: pytest.MonkeyPatch, hidden: bool,
) -> None:
    repo, source, _, _ = generated_checkout(checkout)
    monkeypatch.setattr(regeneration, "GENERATED_FILES", ("b.h",))
    output = repo / "work/new-source-output"

    def generate(staged: Path, _: Path) -> None:
        (staged / "b.h").write_text("other = 3\n")
        parent = staged / "unexpected"
        parent.mkdir()
        if hidden:
            (parent / ".gitignore").write_text("*\n")
        (parent / "new_hook.c").write_text("unexpected source\n")

    monkeypatch.setattr(regeneration, "generate_cases", generate)
    with pytest.raises(CPythonEnvironmentError, match="uncommitted/untracked"):
        regeneration.regenerate(repo, source, output=output)
    assert not output.exists()
    assert not (source / "unexpected").exists()
