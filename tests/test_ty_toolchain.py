"""Committed-source verification and canonical offline-checker workflow.

The shared Git fixture/validator owns filesystem and checkout semantics. These
tests exercise the Ruff wrapper's actual gate, identity, locks and Cargo calls;
Cargo/checker processes are intercepted, never replaced with source authority.
"""

from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import tomllib

import pytest

from scripts import prepare_ty_toolchain, run_ty
from scripts.prepare_ty_toolchain import (
    RUFF_SUBMODULE,
    ToolchainError,
    checker_fingerprint,
    prepare_toolchain,
    source_lock,
    source_lock_path,
)
from scripts.run_ty import cargo_configuration, exporter_fingerprint
from tests.test_cpython_patches import (
    commit_source,
    make_committed_checkout,
    pin_gitlink,
)


@pytest.fixture
def checker_checkout(tmp_path: Path) -> tuple[Path, Path]:
    repo, source = make_committed_checkout(
        tmp_path / "checker with spaces", submodule=RUFF_SUBMODULE,
    )
    (source / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
    (source / "Cargo.lock").write_text("version = 4\n")
    pin_gitlink(repo, RUFF_SUBMODULE, commit_source(source, "Checker fixture"))
    files = {
        "Cargo.toml": "[workspace]\nmembers = []\n",
        "tools/ty/Cargo.toml": "[package]\nname = 'soac_ty'\nversion = '0.1.0'\n",
        "tools/ty/Cargo.lock": "version = 4\n",
        "tools/ty/src/main.rs": "fn main() {}\n",
        "scripts/run_ty.py": "# runner identity input\n",
        "scripts/prepare_ty_toolchain.py": "# verifier wrapper identity input\n",
        "scripts/committed_source.py": "# shared verifier identity input\n",
    }
    for name in ("soac_contracts", "soac_source"):
        files[f"crates/{name}/Cargo.toml"] = f"[package]\nname = '{name}'\nversion = '0.1.0'\n"
        files[f"crates/{name}/src/lib.rs"] = "// shared checker dependency\n"
    for name, contents in files.items():
        path = repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents)
    return repo, source


def verified(checkout: tuple[Path, Path]) -> dict[str, object]:
    repo, source = checkout
    with source_lock(repo, source):
        actual, record = prepare_toolchain(repo)
    assert actual == source.resolve()
    return record


def test_preparation_reuses_only_complete_unchanged_sources(checker_checkout) -> None:
    _, source = checker_checkout
    first = verified(checker_checkout)
    assert verified(checker_checkout) == first
    assert first["source_identity"]["kind"] == "gitlink"
    assert not (source / ".soac-toolchain.json").exists()
    # Equal size is not equal source content; failed verification never repairs.
    original = (source / "a.h").read_bytes()
    changed = original.replace(b"1", b"2")
    assert changed != original and len(changed) == len(original)
    (source / "a.h").write_bytes(changed)
    with pytest.raises(ToolchainError):
        verified(checker_checkout)
    assert (source / "a.h").read_bytes() == changed


def test_legacy_marker_cannot_authenticate_changed_checkout(checker_checkout) -> None:
    _, source = checker_checkout
    record = verified(checker_checkout)
    (source / "a.h").write_text("changed source\n")
    marker = source / ".soac-toolchain.json"
    marker.write_text(json.dumps(record))
    with pytest.raises(ToolchainError):
        verified(checker_checkout)
    assert marker.is_file()
    assert (source / "a.h").read_text() == "changed source\n"


def test_preparation_selects_only_actual_gitlink_not_source_head(checker_checkout) -> None:
    repo, source = checker_checkout
    before = verified(checker_checkout)
    (source / "a.h").write_text("value = 2\n")
    revision = commit_source(source, "Unselected source change")
    with pytest.raises(ToolchainError):
        verified(checker_checkout)
    pin_gitlink(repo, RUFF_SUBMODULE, revision)
    after = verified(checker_checkout)
    assert after["source_identity"]["revision"] == revision
    assert checker_fingerprint(after) != checker_fingerprint(before)


@pytest.mark.parametrize("field", ["revision", "tree", "checkout_sha256"])
def test_checker_identity_binds_every_committed_source_component(checker_checkout, field) -> None:
    record = verified(checker_checkout)
    changed = copy.deepcopy(record)
    original = changed["source_identity"][field]
    changed["source_identity"][field] = ("0" if original[0] != "0" else "1") + original[1:]
    assert checker_fingerprint(changed) != checker_fingerprint(record)
    expected = hashlib.sha256(
        b"SOAC-TY-CHECKER-v2\0"
        + json.dumps(record["source_identity"], sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    assert checker_fingerprint(record) == expected


def test_source_lock_is_shared_nonblocking_and_not_the_outer_build_lock(checker_checkout) -> None:
    repo, source = checker_checkout
    source_hash = hashlib.sha256(os.fsencode(source.resolve())).hexdigest()
    expected = repo / "work/ty-source" / source_hash / "source.lock"
    assert source_lock_path(repo, source) == expected
    with source_lock(repo, source):
        # Verification is a lock-free helper inside the caller's transaction.
        first = prepare_toolchain(repo)
        with source_lock(repo, source):
            assert prepare_toolchain(repo) == first
        with pytest.raises(ToolchainError, match="Ruff source is busy"):
            with source_lock(repo, source, exclusive=True):
                pytest.fail("migration acquired an active reader's source")
    with source_lock(repo, source, exclusive=True):
        with pytest.raises(ToolchainError, match="Ruff source is busy"):
            with source_lock(repo, source):
                pytest.fail("reader entered an active migration")


@pytest.mark.parametrize("relative", [
    "Cargo.toml",
    "tools/ty/Cargo.toml",
    "tools/ty/Cargo.lock",
    "tools/ty/src/main.rs",
    "crates/soac_contracts/src/lib.rs",
    "crates/soac_source/src/lib.rs",
    "scripts/run_ty.py",
    "scripts/prepare_ty_toolchain.py",
    "scripts/committed_source.py",
])
def test_exporter_identity_binds_dependency_locks_and_shared_contract_code(
    checker_checkout, relative,
) -> None:
    repo, _ = checker_checkout
    before = exporter_fingerprint(repo)
    path = repo / relative
    path.write_bytes(path.read_bytes() + b"\n# changed build input\n")
    assert exporter_fingerprint(repo) != before


def test_offline_configuration_bridges_only_shared_soac_crates(checker_checkout) -> None:
    repo, _ = checker_checkout
    configuration = tomllib.loads(cargo_configuration(repo))
    assert configuration == {
        "patch": {
            "crates-io": {
                name: {"path": str(repo / "crates" / name)}
                for name in ("soac_contracts", "soac_source")
            }
        }
    }
    (repo / "crates/soac_source/Cargo.toml").unlink()
    with pytest.raises(ValueError, match="missing shared checker dependency"):
        cargo_configuration(repo)


def setup_runner(checker_checkout, monkeypatch, arguments):
    repo, source = checker_checkout
    monkeypatch.setattr(run_ty, "__file__", str(repo / "scripts/run_ty.py"))
    monkeypatch.setattr(sys, "argv", ["run_ty.py", *arguments])
    return repo, source, verified(checker_checkout)


def assert_source_reader_held(repo: Path, source: Path) -> None:
    with pytest.raises(ToolchainError, match="Ruff source is busy"):
        with source_lock(repo, source, exclusive=True):
            pytest.fail("checker use did not hold its source reader lock")


@pytest.mark.parametrize("cargo_status", [0, 7])
def test_lockfile_refresh_uses_verified_workspace_and_returns_cargo_status(
    checker_checkout, monkeypatch, cargo_status,
) -> None:
    repo, source, _ = setup_runner(checker_checkout, monkeypatch, ["--update-lockfile"])
    commands = []

    def cargo(command, *, cwd):
        commands.append((command, cwd))
        assert_source_reader_held(repo, source)
        (repo / "tools/ty/Cargo.lock").write_text("version = 4\n# intended workspace refresh\n")
        return cargo_status

    monkeypatch.setattr(run_ty.subprocess, "call", cargo)
    assert run_ty.main() == cargo_status
    assert len(commands) == 1
    command, directory = commands[0]
    assert command[:6] == [
        "cargo", "+stable", "update", "--workspace",
        "--manifest-path", str(repo / "tools/ty/Cargo.toml"),
    ]
    assert command[6] == "--config" and len(command) == 8
    assert directory == repo
    assert Path(command[7]).read_text() == cargo_configuration(repo)
    assert verified(checker_checkout)["source_identity"]["kind"] == "gitlink"


@pytest.mark.parametrize("package", [None, "ty_project", "ty_module_resolver"])
@pytest.mark.parametrize("outcome", ["success", "cargo-error", "changed-lock"])
def test_checker_test_runner_locks_and_revalidates_the_actual_workspace(
    checker_checkout, monkeypatch, package, outcome,
) -> None:
    selection = ["--test"] if package is None else ["--test-upstream", package]
    repo, source, record = setup_runner(
        checker_checkout, monkeypatch,
        ["--debug-build", *selection, "--", "selected_test", "--nocapture"],
    )
    expected_exporter = exporter_fingerprint(repo)
    commands = []
    for name in (
        "SOAC_TY_RUFF_REVISION", "SOAC_TY_CHECKER_FINGERPRINT", "SOAC_TY_EXPORTER_FINGERPRINT",
    ):
        monkeypatch.setenv(name, "untrusted inherited value")

    def cargo(command, *, cwd, env):
        commands.append((command, cwd, env))
        assert_source_reader_held(repo, source)
        if outcome == "changed-lock":
            (source / "Cargo.lock").write_text("unexpected upstream lock mutation\n")
        return 7 if outcome == "cargo-error" else 0

    monkeypatch.setattr(run_ty.subprocess, "call", cargo)
    assert run_ty.main() == {"success": 0, "cargo-error": 7, "changed-lock": 1}[outcome]
    assert len(commands) == 1, "tests must never execute the normal checker binary"
    command, directory, environment = commands[0]
    assert command[:4] == ["cargo", "+stable", "test", "--locked"]
    manifest = source / "Cargo.toml" if package else repo / "tools/ty/Cargo.toml"
    assert command[command.index("--manifest-path") + 1] == str(manifest)
    assert command[command.index("--target-dir") + 1] == str(repo / "work/target-ty")
    assert command[-3:] == ["--", "selected_test", "--nocapture"]
    if package:
        assert command[command.index("--package") + 1] == package and "--lib" in command
    else:
        assert "--package" not in command
    assert directory == repo
    assert environment["SOAC_TY_RUFF_REVISION"] == record["source_identity"]["revision"]
    assert environment["SOAC_TY_CHECKER_FINGERPRINT"] == checker_fingerprint(record)
    assert environment["SOAC_TY_EXPORTER_FINGERPRINT"] == expected_exporter


@pytest.mark.parametrize("outcome", [
    "success", "cargo-error", "source-build", "source-run",
    "exporter-build", "exporter-run", "configuration-build", "configuration-run",
])
def test_normal_checker_executes_only_under_revalidated_inputs(
    checker_checkout, monkeypatch, outcome,
) -> None:
    repo, source, record = setup_runner(
        checker_checkout, monkeypatch, ["--debug-build", "--", "--help"],
    )
    commands = []
    expected_exporter = exporter_fingerprint(repo)

    def invoke(command, *, cwd, env):
        commands.append((command, cwd, env))
        assert_source_reader_held(repo, source)
        phase = "build" if len(commands) == 1 else "run"
        if outcome == f"source-{phase}":
            (source / "a.h").write_text("unexpected source mutation\n")
        elif outcome == f"exporter-{phase}":
            (repo / "tools/ty/Cargo.lock").write_text("unexpected exporter mutation\n")
        elif outcome == f"configuration-{phase}":
            cargo_command = commands[0][0]
            configuration = Path(cargo_command[cargo_command.index("--config") + 1])
            configuration.write_text("changed dependency configuration\n")
        return 7 if outcome == "cargo-error" else 0

    monkeypatch.setattr(run_ty.subprocess, "call", invoke)
    expected = 0 if outcome == "success" else 7 if outcome == "cargo-error" else 1
    assert run_ty.main() == expected
    expects_run = outcome == "success" or outcome.endswith("-run")
    assert len(commands) == (2 if expects_run else 1)
    cargo, directory, environment = commands[0]
    assert cargo[:4] == ["cargo", "+stable", "build", "--locked"]
    assert cargo[-2:] == ["--bin", "soac-ty"] and directory == repo
    assert environment["SOAC_TY_RUFF_REVISION"] == record["source_identity"]["revision"]
    assert environment["SOAC_TY_CHECKER_FINGERPRINT"] == checker_fingerprint(record)
    assert environment["SOAC_TY_EXPORTER_FINGERPRINT"] == expected_exporter
    if expects_run:
        assert commands[1][0] == [str(repo / "work/target-ty/debug/soac-ty"), "--help"]
    with source_lock(repo, source, exclusive=True):
        pass


@pytest.mark.parametrize(("phase", "completed_commands"), [
    ("initial source verification", 0),
    ("post-build verification", 1),
    ("post-execution verification", 2),
])
def test_checker_reports_which_source_verification_failed_without_continuing(
    checker_checkout, monkeypatch, capsys, phase, completed_commands,
) -> None:
    repo, source, _ = setup_runner(
        checker_checkout, monkeypatch, ["--debug-build", "--", "--help"],
    )
    commands = []

    def invoke(command, *, cwd, env):
        commands.append(command)
        return 0

    original_prepare = run_ty.prepare_toolchain
    original_verify = run_ty.verify_gitlink_record

    def prepare(*args, **kwargs):
        if completed_commands == 0:
            raise ToolchainError("local JJ query failure: exit 7; invalid workspace")
        return original_prepare(*args, **kwargs)

    def verify(*args, **kwargs):
        if len(commands) == completed_commands:
            raise ToolchainError("local JJ query failure: exit 7; invalid workspace")
        return original_verify(*args, **kwargs)

    monkeypatch.setattr(run_ty, "prepare_toolchain", prepare)
    monkeypatch.setattr(run_ty, "verify_gitlink_record", verify)
    monkeypatch.setattr(run_ty.subprocess, "call", invoke)
    assert run_ty.main() == 1
    assert len(commands) == completed_commands
    output = capsys.readouterr()
    assert output.out == ""
    assert output.err == (
        f"offline ty: {phase}: local JJ query failure: exit 7; invalid workspace\n"
    )
    with source_lock(repo, source, exclusive=True):
        pass


@pytest.mark.parametrize("option", ["--pin", "--archive", "--cache"])
def test_old_archive_options_are_not_alternate_source_authority(monkeypatch, option) -> None:
    monkeypatch.setattr(sys, "argv", ["prepare_ty_toolchain.py", option, "untrusted"])
    with pytest.raises(SystemExit) as error:
        prepare_ty_toolchain.main()
    assert error.value.code == 2


def test_preparation_cli_reports_only_verified_v2_gitlink_identity(
    checker_checkout, monkeypatch, capsys,
) -> None:
    repo, source = checker_checkout
    record = verified(checker_checkout)
    monkeypatch.setattr(
        prepare_ty_toolchain, "__file__", str(repo / "scripts/prepare_ty_toolchain.py"),
    )
    monkeypatch.setattr(sys, "argv", ["prepare_ty_toolchain.py"])
    assert prepare_ty_toolchain.main() == 0
    assert json.loads(capsys.readouterr().out) == {
        "schema_version": 2,
        "source": str(source.resolve()),
        "source_identity": record["source_identity"],
        "checker_source_fingerprint": checker_fingerprint(record),
        "source_files": len(record["files"]),
    }
    assert not (source / ".soac-toolchain.json").exists()


@pytest.mark.parametrize("failure", ["changed-source", "source-migration"])
def test_preparation_cli_rejects_real_source_or_lock_failure(
    checker_checkout, monkeypatch, capsys, failure,
) -> None:
    repo, source = checker_checkout
    verified(checker_checkout)
    monkeypatch.setattr(
        prepare_ty_toolchain, "__file__", str(repo / "scripts/prepare_ty_toolchain.py"),
    )
    monkeypatch.setattr(sys, "argv", ["prepare_ty_toolchain.py"])
    original = (source / "a.h").read_bytes()
    expected = original
    if failure == "changed-source":
        expected = original.replace(b"1", b"2")
        assert expected != original and len(expected) == len(original)
        (source / "a.h").write_bytes(expected)
    capsys.readouterr()

    def invoke() -> None:
        # The real Git verifier/lock raises its typed error. The CLI must turn
        # it into its documented failure status, not leak that exception.
        with pytest.raises(SystemExit) as error:
            prepare_ty_toolchain.main()
        assert error.value.code == 1

    if failure == "source-migration":
        with source_lock(repo, source, exclusive=True):
            invoke()
    else:
        invoke()

    output = capsys.readouterr()
    assert output.out == ""
    assert output.err.startswith("checker source verification failed: ")
    assert len(output.err.splitlines()) == 1
    assert (source / "a.h").read_bytes() == expected
    assert not (source / ".soac-toolchain.json").exists()
    with source_lock(repo, source, exclusive=True):
        pass


@pytest.mark.parametrize("failure", ["changed-source", "source-migration"])
def test_invalid_or_busy_source_rejects_before_cargo(
    checker_checkout, monkeypatch, capsys, failure,
) -> None:
    repo, source, _ = setup_runner(
        checker_checkout, monkeypatch, ["--debug-build", "--", "--help"],
    )
    calls = []

    def unexpected(*args, **kwargs):
        calls.append(args)
        pytest.fail("Cargo was reached without a verified, available source")

    monkeypatch.setattr(run_ty.subprocess, "call", unexpected)
    capsys.readouterr()
    if failure == "changed-source":
        (source / "a.h").write_text("uncommitted\n")
        assert run_ty.main() == 1
        assert (source / "a.h").read_text() == "uncommitted\n"
    else:
        with source_lock(repo, source, exclusive=True):
            assert run_ty.main() == 1
    assert calls == []
    output = capsys.readouterr()
    assert output.out == ""
    assert output.err.startswith("offline ty: ")
    assert len(output.err.splitlines()) == 1


@pytest.mark.parametrize("failure", ["upstream-source", "configuration"])
def test_lock_refresh_rechecks_immutable_source_and_dependency_bridge(
    checker_checkout, monkeypatch, failure,
) -> None:
    repo, source, _ = setup_runner(checker_checkout, monkeypatch, ["--update-lockfile"])

    def cargo(command, *, cwd):
        assert_source_reader_held(repo, source)
        if failure == "upstream-source":
            (source / "Cargo.lock").write_text("must not refresh the committed upstream lock\n")
        else:
            Path(command[command.index("--config") + 1]).write_text("changed shared bridge\n")
        return 0

    monkeypatch.setattr(run_ty.subprocess, "call", cargo)
    assert run_ty.main() == 1


def test_cargo_cannot_retarget_declared_vendor_path_away_from_locked_source(
    checker_checkout, monkeypatch,
) -> None:
    repo, declared_source = checker_checkout
    locked_source = repo / "original Ruff checkout"
    declared_source.rename(locked_source)
    declared_source.symlink_to(locked_source, target_is_directory=True)
    replacement = repo / "unverified replacement"
    shutil.copytree(locked_source, replacement, symlinks=True)
    (replacement / "a.h").write_text("different unchecked dependency bytes\n")
    setup_runner(checker_checkout, monkeypatch, ["--debug-build", "--", "--help"])
    commands = []

    def cargo(command, *, cwd, env):
        commands.append(command)
        assert_source_reader_held(repo, locked_source)
        if len(commands) == 1:
            declared_source.unlink()
            declared_source.symlink_to(replacement, target_is_directory=True)
        return 0

    monkeypatch.setattr(run_ty.subprocess, "call", cargo)
    assert run_ty.main() == 1
    assert len(commands) == 1, "normal checker ran after Cargo's declared source changed"
    assert (locked_source / "a.h").read_text() == "value = 1\n"
    assert (declared_source / "a.h").read_text() == "different unchecked dependency bytes\n"
