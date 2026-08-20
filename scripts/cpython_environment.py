#!/usr/bin/env python3
"""Build and verify vendored CPython without hiding its shared source tree."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import tempfile
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

if __package__:
    from . import cpython_sources as sources
    from . import prepare_cpython_source as source_preparation
else:
    import cpython_sources as sources
    import prepare_cpython_source as source_preparation

CPythonEnvironmentError = sources.CPythonEnvironmentError
git_command = sources.git_command
git_output = sources.git_output
pinned_revision = sources.pinned_revision

BUILD_MODES = ("optimized", "development", "stackref-debug")
STACKREF_DEBUG_CPPFLAGS = "-DPy_STACKREF_DEBUG=1"
STACKREF_DEBUG_SYMBOLS = (
    "_Py_stackref_get_object",
    "_Py_stackref_create",
    "_Py_stackref_close",
    "_Py_stackref_get_borrowed_from",
)
STACKREF_DEBUG_PROBE = """
import ctypes, json, sys, sysconfig
from pathlib import Path

libraries = set()
deleted_library = False
for line in Path('/proc/self/maps').read_text().splitlines():
    fields = line.split(maxsplit=5)
    if len(fields) != 6 or not fields[5].startswith('/'):
        continue
    filename = fields[5]
    deleted = filename.endswith(' (deleted)')
    filename = filename.removesuffix(' (deleted)')
    if Path(filename).name.startswith('libpython'):
        libraries.add(str(Path(filename).resolve()))
        deleted_library |= deleted

print(json.dumps({
    'version': 1,
    'executable': sys.executable,
    'base_executable': sys._base_executable,
    'source': sysconfig.get_config_var('abs_srcdir'),
    'build': sysconfig.get_config_var('abs_builddir'),
    'shared': sysconfig.get_config_var('Py_ENABLE_SHARED'),
    'py_debug': sysconfig.get_config_var('Py_DEBUG'),
    'gil_disabled': sysconfig.get_config_var('Py_GIL_DISABLED'),
    'pointer_bits': ctypes.sizeof(ctypes.c_void_p) * 8,
    'has_totalrefcount': hasattr(sys, 'gettotalrefcount'),
    'config_args': sysconfig.get_config_var('CONFIG_ARGS'),
    'configure_cppflags': sysconfig.get_config_var('CONFIGURE_CPPFLAGS'),
    'cppflags': sysconfig.get_config_var('CPPFLAGS'),
    'configure_cflags_nodist': sysconfig.get_config_var('CONFIGURE_CFLAGS_NODIST'),
    'ldlibrary': sysconfig.get_config_var('LDLIBRARY'),
    'instsoname': sysconfig.get_config_var('INSTSONAME'),
    'loaded_libpython': sorted(libraries),
    'deleted_libpython': deleted_library,
    'symbols': {name: hasattr(ctypes.pythonapi, name) for name in json.loads(sys.argv[1])},
}))
"""


@dataclass(frozen=True)
class CPythonPaths:
    repo: Path
    source: Path
    build: Path
    binary: Path
    library: Path

    @classmethod
    def from_environment(
        cls, repo: Path, environment: Mapping[str, str]
    ) -> CPythonPaths:
        repo = repo.resolve()

        def path(value: str | Path) -> Path:
            candidate = Path(value)
            return (repo / candidate).resolve()

        source = sources.source_directory(repo, environment)
        build = sources.selected_build_directory(repo, source, environment)
        return cls(
            repo,
            source,
            build,
            path(environment.get("CPYTHON_BIN") or build / "python"),
            path(environment.get("CPYTHON_LIB_DIR") or build),
        )


def mount_for(path: Path) -> dict[str, str]:
    result = subprocess.check_output(
        [
            "findmnt",
            "--json",
            "--output",
            "TARGET,SOURCE,FSTYPE",
            "--target",
            str(path),
        ],
        text=True,
    )
    return json.loads(result)["filesystems"][0]


def validate_source_mount(
    repo_mount: Mapping[str, str], source_mount: Mapping[str, str]
) -> None:
    if repo_mount["fstype"] == "virtiofs" and source_mount["fstype"] != "virtiofs":
        raise CPythonEnvironmentError(
            "the selected CPython source is hidden by a VM-local mount while the repository is shared "
            "over virtiofs; preserve both source trees and remove the overlay before "
            "building. Keep only CPYTHON_BUILD_DIR on the guest filesystem."
        )


def check_source(paths: CPythonPaths) -> None:
    if not (paths.source / "configure").is_file():
        raise CPythonEnvironmentError(
            f"CPython source checkout is missing: {paths.source}"
        )
    validate_source_mount(mount_for(paths.repo), mount_for(paths.source))
    actual = git_output(paths.source, "rev-parse", "HEAD")
    expected = pinned_revision(paths.repo)
    if actual != expected:
        raise CPythonEnvironmentError(
            f"CPython source HEAD {actual} differs from the repository pin {expected}; "
            "preserve local work and synchronize the submodule before building"
        )


def runtime_environment(paths: CPythonPaths) -> dict[str, str]:
    environment = dict(os.environ)
    environment["LD_LIBRARY_PATH"] = str(paths.library) + (
        ":" + environment["LD_LIBRARY_PATH"]
        if environment.get("LD_LIBRARY_PATH")
        else ""
    )
    return environment


def runtime_info(paths: CPythonPaths) -> dict[str, object]:
    # os.access(..., X_OK) alone accepts Python/ on a case-insensitive mount.
    if not paths.binary.is_file() or not os.access(paths.binary, os.X_OK):
        raise CPythonEnvironmentError(
            f"CPython executable is missing: {paths.binary}; run 'just build-python'"
        )
    result = subprocess.check_output(
        [
            str(paths.binary),
            "-I",
            "-S",
            "-c",
            "import json,sys,sysconfig; print(json.dumps({"
            "'executable':sys.executable,"
            "'source':sysconfig.get_config_var('abs_srcdir'),"
            "'build':sysconfig.get_config_var('abs_builddir'),"
            "'shared':sysconfig.get_config_var('Py_ENABLE_SHARED')}))",
        ],
        env=runtime_environment(paths),
        text=True,
    )
    return json.loads(result)


def check_native_extensions(paths: CPythonPaths) -> None:
    # CPython's make can succeed even when a new C symbol leaves an extension
    # unimportable. Do not record/select that build as a usable SOAC runtime.
    try:
        subprocess.run(
            [
                str(paths.binary),
                "-I",
                "-S",
                "-B",
                "-c",
                "import _ctypes, _testcapi, _testinternalcapi",
            ],
            cwd=paths.build,
            env=runtime_environment(paths),
            capture_output=True,
            text=True,
            check=True,
        )
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or str(error)).strip()
        raise CPythonEnvironmentError(
            "CPython required native extensions failed their import readiness check "
            f"under {paths.binary}; build was not selected:\n{detail}"
        ) from error


def stackref_debug_info(paths: CPythonPaths) -> dict[str, object]:
    # Keep this separate from runtime_info: existing nondebug build receipts
    # must not change merely because an optional diagnostic mode was added.
    try:
        output = subprocess.check_output(
            [
                str(paths.binary),
                "-I",
                "-S",
                "-B",
                "-c",
                STACKREF_DEBUG_PROBE,
                json.dumps(STACKREF_DEBUG_SYMBOLS),
            ],
            cwd=paths.build,
            env=runtime_environment(paths),
            stderr=subprocess.PIPE,
            text=True,
        )
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or str(error)).strip()
        raise CPythonEnvironmentError(
            f"CPython stackref-debug runtime proof failed under {paths.binary}:\n{detail}"
        ) from error
    try:
        proof = json.loads(output)
    except ValueError as error:
        raise CPythonEnvironmentError(
            "CPython stackref-debug proof is not JSON"
        ) from error
    if not isinstance(proof, dict):
        raise CPythonEnvironmentError("CPython stackref-debug proof is not an object")
    return proof


def validate_stackref_debug_runtime(
    paths: CPythonPaths, proof: Mapping[str, object]
) -> None:
    def require(valid: bool, detail: str) -> None:
        if not valid:
            raise CPythonEnvironmentError(f"CPython stackref-debug proof: {detail}")

    require(proof.get("version") == 1, "unsupported proof version")
    validate_runtime_layout(paths, proof)
    for field in ("executable", "base_executable"):
        actual = proof.get(field)
        require(
            isinstance(actual, str) and Path(actual).resolve() == paths.binary.resolve(),
            f"{field} does not identify the candidate executable",
        )
    require(proof.get("py_debug") == 1, "Py_DEBUG is not enabled")
    require(
        proof.get("has_totalrefcount") is True,
        "actual runtime lacks debug refcount support",
    )
    require(proof.get("gil_disabled") in (None, 0), "a GIL-enabled build is required")
    require(proof.get("pointer_bits") == 64, "native StackRef debugging requires 64 bits")
    require(
        proof.get("configure_cppflags") == STACKREF_DEBUG_CPPFLAGS,
        "configured CPPFLAGS do not select Py_STACKREF_DEBUG=1",
    )
    config_args = proof.get("config_args")
    if not isinstance(config_args, str):
        raise CPythonEnvironmentError(
            "CPython stackref-debug proof: missing actual configure arguments"
        )
    try:
        arguments = shlex.split(config_args)
    except ValueError as error:
        raise CPythonEnvironmentError(
            "CPython stackref-debug proof: invalid configure arguments"
        ) from error
    require(
        {"--enable-shared", "--with-pydebug", f"CPPFLAGS={STACKREF_DEBUG_CPPFLAGS}"}
        <= set(arguments),
        "actual configure arguments do not select the debug mode",
    )
    require(
        "--enable-optimizations" not in arguments and "--with-lto" not in arguments,
        "debug mode must not use PGO/LTO",
    )
    libraries = []
    for field in ("instsoname", "ldlibrary"):
        name = proof.get(field)
        if not isinstance(name, str) or Path(name).name != name or not name.startswith("libpython"):
            raise CPythonEnvironmentError(
                f"CPython stackref-debug proof: invalid configured {field}"
            )
        path = (paths.library / name).resolve()
        require(
            path.parent == paths.library and path.is_file(),
            f"{field} is outside the candidate build or missing",
        )
        libraries.append(path)
    # Linux's build recipe may hard-link LDLIBRARY to INSTSONAME rather than
    # symlink it. Check the actual SONAME mapping and that both names share it.
    require(
        libraries[0].samefile(libraries[1]),
        "configured libpython names identify different files",
    )
    require(
        proof.get("loaded_libpython") == [str(libraries[0])]
        and proof.get("deleted_libpython") is False,
        "actual loaded libpython is not the candidate's configured SONAME",
    )
    require(
        proof.get("symbols") == dict.fromkeys(STACKREF_DEBUG_SYMBOLS, True),
        "actual runtime is missing native StackRef debug-only exports",
    )


def check_public_cxx_header(
    paths: CPythonPaths, environment: Mapping[str, str]
) -> None:
    # Run after configure creates pyconfig.h, before the expensive PGO build.
    # Make expands its actual configured CXX, including any compiler wrapper.
    with tempfile.TemporaryDirectory(
        prefix=".soac-cxx-header-", dir=paths.build
    ) as probe:
        directory = Path(probe)
        source = directory / "header.cpp"
        source.write_text("#include <Python.h>\n")
        arguments = shlex.join(
            [
                "-fsyntax-only",
                "-x",
                "c++",
                f"-I{paths.source / 'Include'}",
                f"-I{paths.build}",
                str(source),
            ]
        ).replace("$", "$$")  # Preserve literal dollars through make, then the shell.
        target = "soac-public-cxx-header-check"
        makefile = directory / "Makefile"
        makefile.write_text(
            f".PHONY: {target}\n{target}:\n"
            "\t$(if $(strip $(CXX)),,$(error configured CPython Makefile has no CXX))\n"
            f"\t$(CXX) $(CONFIGURE_CPPFLAGS) {arguments}\n"
        )
        try:
            subprocess.run(
                [
                    "make",
                    "--no-print-directory",
                    "-f",
                    "Makefile",
                    "-f",
                    str(makefile),
                    target,
                ],
                cwd=paths.build,
                env=environment,
                capture_output=True,
                text=True,
                check=True,
            )
        except subprocess.CalledProcessError as error:
            detail = (error.stderr or error.stdout or str(error)).strip()
            raise CPythonEnvironmentError(
                "CPython public headers failed their configured C++ syntax check; "
                f"build stopped before compilation or selection:\n{detail}"
            ) from error


def validate_runtime_layout(paths: CPythonPaths, runtime: Mapping[str, object]) -> None:
    if runtime.get("shared") != 1:
        raise CPythonEnvironmentError("CPython must be built with --enable-shared")
    for field, expected in (("source", paths.source), ("build", paths.library)):
        actual = runtime.get(field)
        if not isinstance(actual, str) or Path(actual).resolve() != expected:
            raise CPythonEnvironmentError(
                f"CPython {field} path {actual!r} does not match {expected}; "
                "rebuild against this source/build layout with 'just build-python'"
            )


def runtime_files(paths: CPythonPaths) -> dict[str, list[int]]:
    files = [paths.binary, paths.library / "pyconfig.h"]
    files.extend(sorted(paths.library.glob("libpython*.so*")))
    return {
        str(path): [path.stat().st_size, path.stat().st_mtime_ns]
        for path in files
        if path.is_file()
    }


def check_runtime(
    paths: CPythonPaths, *, require_mode: str | None = None
) -> dict[str, object]:
    return dict(_verified_runtime_record(paths, require_mode=require_mode)["source_identity"])


def _verified_runtime_record(
    paths: CPythonPaths, *, require_mode: str | None = None
) -> dict[str, object]:
    check_source(paths)
    with source_preparation.verified_source(paths.repo, paths.source, shared=True) as prepared:
        return _check_verified_runtime(paths, prepared, require_mode=require_mode)


def _check_verified_runtime(
    paths: CPythonPaths, prepared: dict[str, object], *, require_mode: str | None
) -> dict[str, object]:
    # Reject old/mismatched records before starting an interpreter that would
    # otherwise read the new source tree's stdlib with an old native library.
    record_path = paths.library / ".soac-cpython-build.json"
    if not record_path.is_file():
        raise CPythonEnvironmentError(
            f"CPython build provenance is missing at {record_path}; "
            "run 'just build-python' once to record the source and runtime identity"
        )
    try:
        record = json.loads(record_path.read_text())
    except ValueError as error:
        raise CPythonEnvironmentError("CPython build provenance is not valid JSON") from error
    required = {
        "schema_version", "source", "build", "source_identity", "runtime",
        "runtime_files", "build_mode", "configure",
    }
    if (
        not isinstance(record, dict) or record.get("schema_version") != 2
        or not required <= record.keys() <= required | {"stackref_debug"}
    ):
        raise CPythonEnvironmentError(
            "CPython build provenance schema is obsolete or invalid; "
            "rebuild from the committed source; old records are never restamped"
        )
    if not isinstance(record["configure"], list) or not all(
        isinstance(argument, str) for argument in record["configure"]
    ):
        raise CPythonEnvironmentError("CPython build has invalid configure provenance")
    if record["source_identity"] != prepared["source_identity"]:
        raise CPythonEnvironmentError(
            "CPython committed source identity differs from the built interpreter; "
            "run 'just build-python' in a fresh build directory"
        )
    if record["build_mode"] not in BUILD_MODES:
        raise CPythonEnvironmentError("CPython has no supported recorded build mode; rebuild it")
    if require_mode is not None and record["build_mode"] != require_mode:
        raise CPythonEnvironmentError(
            f"this command requires CPython build mode {require_mode!r}; selected build "
            f"mode is {record['build_mode']!r}. "
            f"Run 'just build-python {require_mode}' in a separate build directory"
        )
    if (
        record["source"] != str(paths.source)
        or record["build"] != str(paths.build)
        or record["runtime_files"] != runtime_files(paths)
    ):
        raise CPythonEnvironmentError(
            "CPython sources or build artifacts changed since 'just build-python'; "
            "rebuild before reusing this interpreter"
        )
    runtime = runtime_info(paths)
    validate_runtime_layout(paths, runtime)
    if record["runtime"] != runtime:
        raise CPythonEnvironmentError(
            "CPython runtime identity changed since 'just build-python'; rebuild it"
        )
    if record["build_mode"] == "stackref-debug":
        if not isinstance(record.get("stackref_debug"), dict):
            raise CPythonEnvironmentError(
                "CPython stackref-debug build has no recorded runtime proof"
            )
        proof = stackref_debug_info(paths)
        validate_stackref_debug_runtime(paths, proof)
        if record["stackref_debug"] != proof:
            raise CPythonEnvironmentError(
                "CPython stackref-debug runtime proof changed since the build"
            )
    elif "stackref_debug" in record:
        raise CPythonEnvironmentError("CPython nondebug build has unexpected diagnostic provenance")
    source_preparation.verify_record(paths.repo, paths.source, prepared)
    if record["runtime_files"] != runtime_files(paths):
        raise CPythonEnvironmentError("CPython build artifacts changed during runtime verification")
    return record


def require_case_sensitive_build(directory: Path) -> None:
    with tempfile.TemporaryDirectory(
        prefix=".soac-case-check-", dir=directory
    ) as probe:
        path = Path(probe)
        (path / "lowercase").touch()
        if (path / "LOWERCASE").exists():
            raise CPythonEnvironmentError(
                f"CPython build directory is case-insensitive: {directory}; set "
                "CPYTHON_BUILD_DIR to a guest-local ext4 directory, leaving "
                "vendor/cpython shared with the host"
            )


def build_python(
    paths: CPythonPaths, mode: str = "optimized", *, select: bool = True
) -> None:
    if mode not in BUILD_MODES:
        raise CPythonEnvironmentError(f"unknown CPython build mode: {mode!r}")
    check_source(paths)
    if paths.binary != paths.build / "python" or paths.library != paths.build:
        raise CPythonEnvironmentError(
            "build-python produces CPYTHON_BUILD_DIR/python and its adjacent libpython; "
            "unset conflicting CPYTHON_BIN/CPYTHON_LIB_DIR overrides before building"
        )
    paths.build.mkdir(parents=True, exist_ok=True)
    require_case_sensitive_build(paths.build)
    if paths.source != paths.build and any(
        (paths.source / name).exists() for name in ("Makefile", "pyconfig.h")
    ):
        raise CPythonEnvironmentError(
            "out-of-tree CPython builds need an unconfigured source tree; preserve "
            "existing source/build artifacts before cleaning the source checkout"
        )
    with source_preparation.verified_source(paths.repo, paths.source) as prepared:
        _build_verified_python(paths, prepared, mode, select=select)


def _build_verified_python(
    paths: CPythonPaths, prepared: dict[str, object], mode: str, *, select: bool
) -> None:
    before = dict(prepared["source_identity"])
    environment = dict(os.environ)
    environment["LDFLAGS"] = "-Wl,-rpath,'$$ORIGIN'"
    if mode == "stackref-debug":
        # This explicit diagnostic mode owns CPPFLAGS. Ordinary modes retain
        # their existing ambient-environment handling.
        environment["CPPFLAGS"] = STACKREF_DEBUG_CPPFLAGS
    if (paths.build / "Makefile").is_file():
        subprocess.run(["make", "clean"], cwd=paths.build, env=environment, check=True)
    configure = [str(paths.source / "configure"), "--enable-shared"]
    if mode == "optimized":
        configure.extend(("--enable-optimizations", "--with-lto"))
    elif mode == "stackref-debug":
        configure.extend(("--with-pydebug", f"CPPFLAGS={STACKREF_DEBUG_CPPFLAGS}"))
    optimization = "-O0" if mode == "stackref-debug" else "-O3"
    configure.append(
        f"CFLAGS_NODIST={optimization} -g -fno-omit-frame-pointer -fasynchronous-unwind-tables"
    )
    subprocess.run(
        configure,
        cwd=paths.build,
        env=environment,
        check=True,
    )
    check_public_cxx_header(paths, environment)
    jobs = len(os.sched_getaffinity(0))
    subprocess.run(["make", f"-j{jobs}"], cwd=paths.build, env=environment, check=True)
    runtime = runtime_info(paths)
    validate_runtime_layout(paths, runtime)
    check_native_extensions(paths)
    debug_proof = None
    if mode == "stackref-debug":
        debug_proof = stackref_debug_info(paths)
        validate_stackref_debug_runtime(paths, debug_proof)
    source_preparation.verify_record(paths.repo, paths.source, prepared)
    record = {
        "schema_version": 2,
        "source": str(paths.source),
        "build": str(paths.build),
        "source_identity": before,
        "runtime": runtime,
        "runtime_files": runtime_files(paths),
        "build_mode": mode,
        "configure": configure,
    }
    if debug_proof is not None:
        record["stackref_debug"] = debug_proof
    record_path = paths.build / ".soac-cpython-build.json"
    temporary = record_path.with_suffix(".tmp")
    temporary.write_text(json.dumps(record, indent=2) + "\n")
    temporary.replace(record_path)
    if select:
        sources.save_selected_build(paths.repo, paths.source, paths.build)
    print(f"CPython {mode} build from {before['revision']} at {paths.source}")
    print(f"CPython build provenance: {record_path}")
    if not select:
        print("CPython candidate verified; saved build selection is unchanged")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=(
            "info",
            "check-source",
            "check-runtime",
            "build",
            "build-dir",
            "select-build",
        ),
    )
    parser.add_argument(
        "--repo", type=Path, default=Path(__file__).resolve().parents[1]
    )
    parser.add_argument(
        "--mode", choices=BUILD_MODES, default="optimized"
    )
    parser.add_argument("--require-mode", choices=BUILD_MODES)
    parser.add_argument(
        "--no-select",
        action="store_true",
        help="build and verify a candidate without changing the saved build selection",
    )
    options = parser.parse_args()
    if options.no_select and options.command != "build":
        parser.error("--no-select is only valid with build")
    try:
        paths = CPythonPaths.from_environment(options.repo, os.environ)
        if options.command == "build-dir":
            print(paths.build)
        elif options.command == "select-build":
            check_runtime(paths)
            sources.save_selected_build(paths.repo, paths.source, paths.build)
            print(f"Selected verified CPython build: {paths.build}")
        elif options.command == "info":
            report: dict[str, object] = {
                "source": str(paths.source),
                "source_revision": git_output(paths.source, "rev-parse", "HEAD"),
                "pinned_revision": pinned_revision(paths.repo),
                "source_mount": mount_for(paths.source),
                "repository_mount": mount_for(paths.repo),
                "vendored_source_mount": mount_for(paths.repo / "vendor/cpython"),
                "using_source_override": paths.source
                != (paths.repo / "vendor/cpython").resolve(),
                "build": str(paths.build),
                "binary": str(paths.binary),
                "library": str(paths.library),
            }
            try:
                record = _verified_runtime_record(paths)
                report["runtime"] = record["runtime"]
                report["source_identity"] = record["source_identity"]
                report["build_mode"] = record.get("build_mode", "unrecorded")
                if report["build_mode"] == "stackref-debug":
                    report["stackref_debug"] = record["stackref_debug"]
                report["verified"] = True
            except (CPythonEnvironmentError, subprocess.CalledProcessError) as error:
                report["verified"] = False
                report["problem"] = str(error)
            print(json.dumps(report, indent=2))
        elif options.command == "check-source":
            check_source(paths)
            source_preparation.verify_source(paths.repo, paths.source)
        elif options.command == "check-runtime":
            check_runtime(paths, require_mode=options.require_mode)
        else:
            build_python(paths, options.mode, select=not options.no_select)
    except (CPythonEnvironmentError, subprocess.CalledProcessError, OSError) as error:
        print(f"CPython environment: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
