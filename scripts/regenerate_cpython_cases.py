#!/usr/bin/env python3
"""Regenerate native cases in a disposable checkout, never an applied patch.

--check reproduces a pinned generated-only top commit from its exact logical
parent. A logical top commit that changes no generated files is checked by
regenerating its own exact tree and requiring unchanged output. Mixed commits
are rejected. --revision accepts an explicit full logical commit for producing
an ignored review artifact only; it cannot authorize a build or runtime.
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
    from . import prepare_cpython_source as preparation
else:
    import prepare_cpython_source as preparation


GENERATED_FILES = (
    "Include/opcode_ids.h",
    "Python/opcode_targets.h",
    "Include/internal/pycore_uop_ids.h",
    "Lib/_opcode_metadata.py",
    "Python/generated_cases.c.h",
    "Python/record_functions.c.h",
    "Modules/_testinternalcapi/test_cases.c.h",
    "Modules/_testinternalcapi/test_targets.h",
    "Python/executor_cases.c.h",
    "Python/optimizer_cases.c.h",
    "Include/internal/pycore_opcode_metadata.h",
    "Include/internal/pycore_uop_metadata.h",
)
CPythonEnvironmentError = preparation.CPythonEnvironmentError


def generation_commands(source: Path, build: Path) -> list[list[str]]:
    return [
        [str(source / "configure"), "--enable-shared", "--without-ensurepip"],
        ["make", "regen-cases", f"PYTHON_FOR_REGEN={sys.executable}"],
    ]


def generate_cases(source: Path, build: Path) -> None:
    build.mkdir()
    for command in generation_commands(source, build):
        try:
            subprocess.run(command, cwd=build, check=True, capture_output=True)
        except subprocess.CalledProcessError as error:
            detail = (error.stderr or error.stdout or b"").decode(errors="replace").strip()
            raise CPythonEnvironmentError(
                f"CPython case generation failed: {command!r}\n{detail}"
            ) from error


def generation_input_revision(source: Path, revision: str) -> str:
    # Read the actual commit object, not a locally grafted or shallow revision
    # walk. A missing parent is an explicit local-availability failure; no fetch.
    header = preparation.git_bytes(source, "cat-file", "commit", revision).split(b"\n\n", 1)[0]
    parents = [line[7:].decode() for line in header.splitlines() if line.startswith(b"parent ")]
    if len(parents) != 1:
        raise CPythonEnvironmentError("pinned regeneration reference must have exactly one parent")
    parent = parents[0]
    preparation.commit_tree(source, parent)
    changed = {
        os.fsdecode(name)
        for name in preparation.git_bytes(
            source, "diff-tree", "--no-commit-id", "--no-renames",
            "--name-only", "-r", "-z", parent, revision, "--",
        ).split(b"\0") if name
    }
    generated_changes = changed & set(GENERATED_FILES)
    if not generated_changes:
        # A logical change need not invent a generated-only commit when the
        # generator reproduces the already-committed outputs byte for byte.
        # Regenerate HEAD itself: dropping to its parent would skip the change
        # whose effect on generated output this check must establish.
        return revision
    if changed != generated_changes:
        raise CPythonEnvironmentError(
            f"generated changes require a generated-only top commit: {sorted(changed)}"
        )
    return parent


def _output_directory(repo: Path, source: Path, output: Path) -> Path:
    if output.is_symlink() or output.exists():
        raise CPythonEnvironmentError(f"generated review output already exists: {output}")
    output = output.resolve()
    if (
        not output.is_relative_to((repo / "work").resolve())
        or output.is_relative_to(source.resolve())
    ):
        raise CPythonEnvironmentError(
            "generated review output must be inside ignored work, outside native sources"
        )
    return output


def _publish_output(
    output: Path, contents: dict[str, bytes], record: dict[str, object],
) -> None:
    # Exclusive directory creation preserves prior output even on concurrent
    # retries. A failed write leaves review-only partial data with no completed
    # generation.json; it never replaces native sources or an existing packet.
    output.mkdir(parents=True, exist_ok=False)
    for name, data in contents.items():
        path = output / preparation.relative_path(name)
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        path.chmod(int(record["generated_files"][name]["mode"], 8) & 0o777)
    with (output / "generation.json").open("x") as stream:
        json.dump(record, stream, indent=2, sort_keys=True)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def regenerate(
    repo: Path, source: Path, *, output: Path | None = None,
    revision: str | None = None, check: bool = False,
) -> dict[str, object]:
    repo, source = repo.resolve(), source.resolve()
    if check and revision is not None:
        raise CPythonEnvironmentError("--check cannot use an explicit --revision")
    if check and output is not None:
        raise CPythonEnvironmentError("--check does not publish an --output")
    if not check and output is None:
        raise CPythonEnvironmentError("generation requires a new ignored --output directory")
    if output is not None:
        output = _output_directory(repo, source, output)

    with preparation.source_lock(repo, source, shared=True):
        pinned = preparation.pinned_revision(repo)
        selected = revision if revision is not None else pinned
        snapshot = preparation.revision_record(source, selected)
        reference = pinned if revision is None else None
        logical = generation_input_revision(source, pinned) if reference is not None else selected

        with preparation.canonical_checkout(source, logical) as staged:
            before = preparation.inventory(staged, logical)
            build = staged.parent / "build"
            commands = generation_commands(staged, build)
            generate_cases(staged, build)
            after = preparation.inventory(staged, logical)
            preparation.verify_generated_untracked(staged, logical)
            changed = sorted(name for name in before if before[name] != after[name])
            unexpected = sorted(set(changed) - set(GENERATED_FILES))
            if unexpected:
                raise CPythonEnvironmentError(
                    f"case regeneration changed non-generated source files: {unexpected}"
                )
            if not changed and logical != reference:
                raise CPythonEnvironmentError("case regeneration produced no generated changes")
            for name in changed:
                if (
                    before[name]["mode"] not in ("100644", "100755")
                    or after[name]["mode"] != before[name]["mode"]
                ):
                    raise CPythonEnvironmentError(
                        f"case regeneration changed a generated file's kind/mode: {name}"
                    )
            matches = after == snapshot["files"] if reference is not None else None
            if check and not matches:
                raise CPythonEnvironmentError(
                    "pinned generated outputs are stale: regenerated checkout does not match"
                )
            generated_files = {name: after[name] for name in changed}
            record = {
                "schema_version": 1,
                "input_revision": logical,
                "input_tree": preparation.commit_tree(source, logical),
                "reference_revision": reference,
                "reference_tree": snapshot["tree"] if reference is not None else None,
                "matches_reference": matches,
                "generated_files": generated_files,
                "result_checkout_sha256": preparation.checkout_digest(after),
                "commands": commands,
                "working_directory": str(build),
                "python_for_regen": str(Path(sys.executable).resolve()),
                "python_version": sys.version,
                "native_source_written": False,
            }
            contents = {}
            if output is not None:
                for name in changed:
                    value = (staged / preparation.relative_path(name)).read_bytes()
                    if hashlib.sha256(value).hexdigest() != generated_files[name]["sha256"]:
                        raise CPythonEnvironmentError(
                            "generated output changed while preparing the review artifact"
                        )
                    contents[name] = value

            # Recheck exact source bytes, HEAD and the SOAC pin after all
            # generator/configure work and before publishing any review output.
            preparation.verify_revision_record(source, snapshot)
            if preparation.pinned_revision(repo) != pinned:
                raise CPythonEnvironmentError("SOAC native gitlink changed during regeneration")
            if output is not None:
                _publish_output(output, contents, record)
            return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--source", type=Path)
    parser.add_argument(
        "--revision", help="full logical commit in an explicitly selected staging checkout",
    )
    parser.add_argument("--output", type=Path, help="new ignored review output directory")
    parser.add_argument("--check", action="store_true")
    options = parser.parse_args()
    repo = options.repo.resolve()
    source = (options.source or preparation.source_directory(repo, os.environ)).resolve()
    try:
        output = options.output
        if not options.check and output is None:
            selected = options.revision or preparation.pinned_revision(repo)
            output = repo / "work/cpython-generated" / selected
        record = regenerate(
            repo, source, output=output, revision=options.revision, check=options.check,
        )
        print(json.dumps(record, indent=2, sort_keys=True))
    except (CPythonEnvironmentError, OSError, subprocess.CalledProcessError) as error:
        print(f"CPython case regeneration: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
