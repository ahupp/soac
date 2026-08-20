#!/usr/bin/env python3
"""Build and run SOAC's offline exporter from the verified Ruff gitlink.

Ruff-family dependencies are declared path dependencies into vendor/ruff.
The only generated Cargo bridge resolves SOAC's shared crates for upstream
checker libraries. The source lock spans verification, build/run and rechecks.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

if __package__:
    from .committed_source import verify_gitlink_record
    from .prepare_ty_toolchain import (
        RUFF_SUBMODULE,
        ToolchainError,
        checker_fingerprint,
        prepare_toolchain,
        source_lock,
    )
else:
    from committed_source import verify_gitlink_record
    from prepare_ty_toolchain import (
        RUFF_SUBMODULE,
        ToolchainError,
        checker_fingerprint,
        prepare_toolchain,
        source_lock,
    )


def cargo_configuration(root: Path) -> str:
    # These registry dependencies are declared by the committed upstream
    # checker crates. Never substitute another Ruff tree through a Git patch.
    lines = ["[patch.crates-io]"]
    for name in ("soac_contracts", "soac_source"):
        path = root / "crates" / name
        if not (path / "Cargo.toml").is_file():
            raise ValueError(f"missing shared checker dependency: {path}")
        lines.append(f"{name} = {{ path = {json.dumps(str(path))} }}")
    return "\n".join([*lines, ""])


def exporter_fingerprint(root: Path) -> str:
    files = [
        root / "Cargo.toml",
        root / "scripts/run_ty.py",
        root / "scripts/prepare_ty_toolchain.py",
        root / "scripts/committed_source.py",
    ]
    for directory in (
        root / "tools/ty",
        root / "crates/soac_contracts",
        root / "crates/soac_source",
    ):
        files.extend(
            path
            for path in directory.rglob("*")
            if path.is_file()
            and (path.suffix == ".rs" or path.name in {"Cargo.toml", "Cargo.lock"})
        )
    digest = hashlib.sha256(b"SOAC-TY-EXPORTER-v1\0")
    for path in sorted(set(files)):
        contents = path.read_bytes()
        digest.update(path.relative_to(root).as_posix().encode() + b"\0")
        digest.update(len(contents).to_bytes(8, "big"))
        digest.update(contents)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--debug-build", action="store_true")
    operation = parser.add_mutually_exclusive_group()
    operation.add_argument(
        "--test", action="store_true", help="run the offline executable's tests"
    )
    operation.add_argument(
        "--test-upstream",
        choices=("ty_project", "ty_module_resolver"),
        help="run a committed upstream library's tests with its pinned lockfile",
    )
    operation.add_argument(
        "--update-lockfile",
        action="store_true",
        help=(
            "refresh the offline workspace lockfile while preserving unchanged "
            "external dependencies"
        ),
    )
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    options = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    arguments = options.arguments
    if arguments and arguments[0] == "--":
        arguments = arguments[1:]
    phase = "source locking"
    try:
        locked_source = (root / RUFF_SUBMODULE).resolve()
        with source_lock(root, locked_source):
            phase = "initial source verification"
            source, record = prepare_toolchain(root)
            if source != locked_source:
                raise ValueError("declared Ruff source path changed before verification")
            phase = "checker configuration"
            contents = cargo_configuration(root)
            configuration_fingerprint = hashlib.sha256(contents.encode()).hexdigest()
            configuration = (
                root / "work/toolchains" / f"soac-ty-{configuration_fingerprint}.toml"
            )
            configuration.parent.mkdir(parents=True, exist_ok=True)
            if configuration.exists():
                if configuration.read_text() != contents:
                    raise ValueError("existing checker Cargo configuration changed")
            else:
                configuration.write_text(contents)

            if options.update_lockfile:
                # Only tools/ty/Cargo.lock is intentionally mutable. Upstream
                # lock regeneration is an explicit separate source migration.
                phase = "lockfile refresh"
                status = subprocess.call(
                    [
                        "cargo",
                        "+stable",
                        "update",
                        "--workspace",
                        "--manifest-path",
                        str(root / "tools/ty/Cargo.toml"),
                        "--config",
                        str(configuration),
                    ],
                    cwd=root,
                )
                phase = "post-lockfile verification"
                if (root / RUFF_SUBMODULE).resolve() != source:
                    raise ValueError("declared Ruff source path changed during lock refresh")
                verify_gitlink_record(root, RUFF_SUBMODULE, source, record)
                if configuration.read_text() != contents:
                    raise ValueError("checker Cargo configuration changed during lock refresh")
                return status

            environment = dict(os.environ)
            environment["SOAC_TY_RUFF_REVISION"] = record["source_identity"]["revision"]
            environment["SOAC_TY_CHECKER_FINGERPRINT"] = checker_fingerprint(record)
            environment["SOAC_TY_EXPORTER_FINGERPRINT"] = exporter_fingerprint(root)
            testing = options.test or options.test_upstream is not None
            manifest = (
                source / "Cargo.toml"
                if options.test_upstream
                else root / "tools/ty/Cargo.toml"
            )
            command = [
                "cargo",
                "+stable",
                "test" if testing else "build",
                "--locked",
                "--manifest-path",
                str(manifest),
                "--config",
                str(configuration),
                "--target-dir",
                str(root / "work/target-ty"),
            ]
            if not options.debug_build:
                command.append("--release")
            if options.test_upstream:
                command.extend(["--package", options.test_upstream, "--lib"])
            if not testing:
                command.extend(["--bin", "soac-ty"])
            if testing:
                command.extend(["--", *arguments])

            def recheck_inputs() -> None:
                if (root / RUFF_SUBMODULE).resolve() != source:
                    raise ValueError("declared Ruff source path changed during checker use")
                verify_gitlink_record(root, RUFF_SUBMODULE, source, record)
                if configuration.read_text() != contents:
                    raise ValueError("checker Cargo configuration changed during checker use")
                if exporter_fingerprint(root) != environment["SOAC_TY_EXPORTER_FINGERPRINT"]:
                    raise ValueError(
                        "exporter sources or dependency locks changed during checker use; "
                        "rerun from stable inputs"
                    )

            phase = "checker build"
            status = subprocess.call(command, cwd=root, env=environment)
            phase = "post-build verification"
            recheck_inputs()
            if status:
                return status
            if testing:
                return 0
            executable = (
                root / "work/target-ty"
                / ("debug" if options.debug_build else "release")
                / "soac-ty"
            )
            phase = "checker execution"
            status = subprocess.call([str(executable), *arguments], cwd=root, env=environment)
            phase = "post-execution verification"
            recheck_inputs()
            return status
    except (ToolchainError, OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"offline ty: {phase}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
