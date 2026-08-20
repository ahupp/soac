from __future__ import annotations

import json
import os
import shlex
import subprocess
import sys
from contextlib import contextmanager, nullcontext
from pathlib import Path

import pytest

from scripts import cpython_environment as environment
from scripts import migrate_cpython_shared_source as migration


def separate_paths(tmp_path: Path) -> environment.CPythonPaths:
    return environment.CPythonPaths.from_environment(
        tmp_path / "shared", {"CPYTHON_BUILD_DIR": str(tmp_path / "guest-build")}
    )


def fake_stackref_debug_info(paths: environment.CPythonPaths) -> dict[str, object]:
    """File identities and a mocked subprocess reply, never a real debug build."""
    paths.library.mkdir(parents=True, exist_ok=True)
    soname = paths.library / "libpython-debug-control.so.1.0"
    linker_name = paths.library / "libpython-debug-control.so"
    soname.write_bytes(b"mocked native library")
    linker_name.hardlink_to(soname)
    return {
        "version": 1,
        "executable": str(paths.binary),
        "base_executable": str(paths.binary),
        "source": str(paths.source),
        "build": str(paths.build),
        "shared": 1,
        "py_debug": 1,
        "gil_disabled": 0,
        "pointer_bits": 64,
        "has_totalrefcount": True,
        "config_args": shlex.join([
            "--enable-shared", "--with-pydebug", "CPPFLAGS=-DPy_STACKREF_DEBUG=1",
        ]),
        "configure_cppflags": "-DPy_STACKREF_DEBUG=1",
        "cppflags": "",
        "configure_cflags_nodist": "-O0 -g",
        "ldlibrary": linker_name.name,
        "instsoname": soname.name,
        "loaded_libpython": [str(soname)],
        "deleted_libpython": False,
        "symbols": dict.fromkeys(environment.STACKREF_DEBUG_SYMBOLS, True),
    }


@pytest.mark.parametrize(
    ("configured", "expected"),
    [
        (None, []),
        ("", []),
        ("-DPy_STACKREF_DEBUG=1", ["-DPy_STACKREF_DEBUG=1"]),
        ("'-I/native include' -DPy_STACKREF_DEBUG=1",
         ["-I/native include", "-DPy_STACKREF_DEBUG=1"]),
    ],
)
def test_native_internal_header_probe_uses_actual_configured_cppflags(
    monkeypatch: pytest.MonkeyPatch, configured: str | None, expected: list[str],
) -> None:
    from tests import test_strict_generators_native as native_probe

    requested = []

    def config_var(name):
        requested.append(name)
        # A debug Python without an explicit macro must not invent that macro.
        return configured if name == "CONFIGURE_CPPFLAGS" else 1

    monkeypatch.setattr(native_probe.sysconfig, "get_config_var", config_var)
    assert native_probe._native_probe_cppflags() == expected
    assert requested == ["CONFIGURE_CPPFLAGS"]


def test_git_trust_is_scoped_to_the_selected_checkout_and_single_command(
    tmp_path: Path,
) -> None:
    directory = tmp_path / "host owned source"
    assert environment.git_command(directory, "status", "--porcelain") == [
        "git",
        "-c",
        f"safe.directory={directory}",
        "-C",
        str(directory),
        "status",
        "--porcelain",
    ]


def test_python_paths_separate_build_outputs_without_replacing_sources(
    tmp_path: Path,
) -> None:
    paths = separate_paths(tmp_path)
    assert paths.source == tmp_path / "shared/vendor/cpython"
    assert paths.binary == tmp_path / "guest-build/python"
    assert paths.library == tmp_path / "guest-build"
    selected = environment.CPythonPaths.from_environment(
        tmp_path,
        {
            "CPYTHON_SOURCE_DIR": "work/shared-source",
            "CPYTHON_BUILD_DIR": str(tmp_path / "guest-build"),
        },
    )
    assert selected.source == tmp_path / "work/shared-source"
    assert selected.build == paths.build
    default = environment.CPythonPaths.from_environment(tmp_path, {})
    assert default.build == default.source
    override = environment.CPythonPaths.from_environment(
        tmp_path,
        {"CPYTHON_LIB_DIR": "alternate", "CPYTHON_BIN": "alternate/custom-python"},
    )
    assert override.library == tmp_path / "alternate"
    assert override.binary == tmp_path / "alternate/custom-python"


def test_python_preflight_rejects_hidden_guest_source_mount() -> None:
    with pytest.raises(environment.CPythonEnvironmentError, match="VM-local mount"):
        environment.validate_source_mount({"fstype": "virtiofs"}, {"fstype": "ext4"})
    environment.validate_source_mount({"fstype": "virtiofs"}, {"fstype": "virtiofs"})
    environment.validate_source_mount({"fstype": "ext4"}, {"fstype": "ext4"})


def test_python_preflight_does_not_mistake_python_directory_for_executable(
    tmp_path: Path,
) -> None:
    paths = separate_paths(tmp_path)
    paths.binary.mkdir(parents=True)
    with pytest.raises(
        environment.CPythonEnvironmentError, match="executable is missing"
    ):
        environment.runtime_info(paths)


@pytest.mark.parametrize("field", ["source", "build"])
def test_python_preflight_checks_compiled_source_and_build_identity(
    tmp_path: Path, field: str
) -> None:
    paths = separate_paths(tmp_path)
    runtime = {"shared": 1, "source": str(paths.source), "build": str(paths.build)}
    environment.validate_runtime_layout(paths, runtime)
    runtime[field] = str(tmp_path / "other-checkout")
    with pytest.raises(environment.CPythonEnvironmentError, match=f"{field} path"):
        environment.validate_runtime_layout(paths, runtime)


@pytest.mark.parametrize(
    ("mode", "failure"),
    [(mode, failure)
     for mode in ("optimized", "development", "stackref-debug")
     for failure in (None, "header", "extension")]
    + [("stackref-debug", "debug-proof"), ("optimized", "source")],
)
@pytest.mark.parametrize("select", [False, True])
def test_python_build_selects_only_after_header_and_native_extension_checks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: str | None,
    mode: str,
    select: bool,
) -> None:
    paths = environment.CPythonPaths.from_environment(
        tmp_path, {"CPYTHON_BUILD_DIR": "guest-build"}
    )
    observed, debug_calls, case_checks = [], [], []
    monkeypatch.setenv("CPPFLAGS", "-Dordinary_caller_option=1")
    monkeypatch.setattr(environment, "check_source", lambda _: None)

    def case_sensitive_build(directory):
        # This orchestration unit mocks every build command. Its success
        # precondition must not depend on pytest's temporary filesystem.
        assert directory == paths.build and not observed
        case_checks.append(directory)

    monkeypatch.setattr(environment, "require_case_sensitive_build", case_sensitive_build)
    monkeypatch.setattr(
        environment.source_preparation,
        "verified_source",
        lambda *_: nullcontext({"source_identity": {"kind": "gitlink", "revision": "pinned", "tree": "tree", "checkout_sha256": "bytes"}}),
    )

    def verify_record(*_):
        if failure == "source":
            raise environment.CPythonEnvironmentError("committed source changed during build")

    monkeypatch.setattr(environment.source_preparation, "verify_record", verify_record)
    monkeypatch.setattr(
        environment,
        "runtime_info",
        lambda _: {"shared": 1, "source": str(paths.source), "build": str(paths.build)},
    )
    proof = fake_stackref_debug_info(paths) if mode == "stackref-debug" else None

    def debug_info(actual_paths):
        assert actual_paths == paths and mode == "stackref-debug"
        assert observed[-1][0][-1] == "import _ctypes, _testcapi, _testinternalcapi"
        debug_calls.append(actual_paths)
        if failure == "debug-proof":
            return {**proof, "symbols": {}}
        return proof

    monkeypatch.setattr(environment, "stackref_debug_info", debug_info)

    def run(args, **kwargs):
        observed.append((args, kwargs))
        if args[0] == "make" and "-f" in args and failure == "header":
            raise subprocess.CalledProcessError(
                2, args, stderr="Python.h: invalid C++ declaration"
            )
        if args[0] == str(paths.binary) and failure == "extension":
            raise subprocess.CalledProcessError(
                1,
                args,
                stderr="ImportError: _testinternalcapi: undefined symbol: native_hook",
            )

    monkeypatch.setattr(environment.subprocess, "run", run)
    old_build = tmp_path / "previous-valid-build"
    environment.sources.save_selected_build(paths.repo, paths.source, old_build)
    build_options = {} if select else {"select": False}

    if failure is not None:
        with pytest.raises(
            environment.CPythonEnvironmentError,
            match={
                "header": "invalid C\\+\\+ declaration",
                "extension": "undefined symbol: native_hook",
                "debug-proof": "missing native StackRef debug-only exports",
                "source": "committed source changed during build",
            }[failure],
        ):
            environment.build_python(paths, mode, **build_options)
    else:
        environment.build_python(paths, mode, **build_options)

    assert case_checks == [paths.build]
    configure, header, *remaining = observed
    assert configure[0][0] == str(paths.source / "configure")
    assert configure[1]["cwd"] == paths.build
    assert "--enable-shared" in configure[0]
    assert ("--with-pydebug" in configure[0]) == (mode == "stackref-debug")
    assert ("--enable-optimizations" in configure[0]) == (mode == "optimized")
    assert ("--with-lto" in configure[0]) == (mode == "optimized")
    assert ("CPPFLAGS=-DPy_STACKREF_DEBUG=1" in configure[0]) == (mode == "stackref-debug")
    assert configure[1]["env"]["CPPFLAGS"] == (
        "-DPy_STACKREF_DEBUG=1" if mode == "stackref-debug" else "-Dordinary_caller_option=1"
    )
    optimization = "-O0" if mode == "stackref-debug" else "-O3"
    assert (
        f"CFLAGS_NODIST={optimization} -g -fno-omit-frame-pointer -fasynchronous-unwind-tables"
    ) in configure[0]
    assert header[0][:4] == ["make", "--no-print-directory", "-f", "Makefile"]
    assert header[1]["cwd"] == paths.build
    assert header[1]["env"] == configure[1]["env"]
    record_path = paths.build / ".soac-cpython-build.json"
    selected = environment.CPythonPaths.from_environment(paths.repo, {}).build
    if failure == "header":
        assert not remaining, "header failure must stop before PGO or native imports"
        assert not debug_calls
        assert not record_path.exists()
        assert selected == old_build
        return

    make, imports = remaining
    assert make[0][0] == "make"
    assert make[1]["cwd"] == paths.build
    assert imports[0] == [
        str(paths.binary),
        "-I",
        "-S",
        "-B",
        "-c",
        "import _ctypes, _testcapi, _testinternalcapi",
    ]
    assert imports[1]["cwd"] == paths.build
    assert imports[1]["env"]["LD_LIBRARY_PATH"].split(":")[0] == str(paths.library)
    assert debug_calls == ([paths] if mode == "stackref-debug" and failure != "extension" else [])
    if failure in ("extension", "debug-proof", "source"):
        assert not record_path.exists()
        assert selected == old_build
    else:
        record = json.loads(record_path.read_text())
        assert record["source"] == str(paths.source)
        assert record["schema_version"] == 2
        assert record["build"] == str(paths.build)
        assert record["source_identity"] == {
            "kind": "gitlink", "revision": "pinned", "tree": "tree", "checkout_sha256": "bytes",
        }
        assert "patch_generation" not in record
        assert record["build_mode"] == mode
        assert record["configure"] == configure[0]
        assert record.get("stackref_debug") == proof
        assert ("stackref_debug" in record) == (mode == "stackref-debug")
        assert selected == (paths.build if select else old_build)


@pytest.mark.parametrize("case_sensitive", [False, True])
def test_python_build_case_sensitivity_precedes_verified_build(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, case_sensitive: bool,
) -> None:
    paths = separate_paths(tmp_path)
    old_build = paths.repo / "work" / "previous-valid-build"
    environment.sources.save_selected_build(paths.repo, paths.source, old_build)
    monkeypatch.setattr(environment, "check_source", lambda _: None)
    original_exists = Path.exists
    probes, observed = [], []
    prepared = {"source_identity": {"kind": "gitlink", "revision": "pinned"}}

    def exists(path):
        if (path.name == "LOWERCASE" and path.parent.parent == paths.build
                and path.parent.name.startswith(".soac-case-check-")):
            assert path.with_name("lowercase").is_file()
            probes.append(path)
            # Exercise the real probe and exception path on either host-backed
            # or guest-local pytest storage; only emulate its filesystem reply.
            return not case_sensitive
        return original_exists(path)

    @contextmanager
    def verified_source(repo, source):
        assert (repo, source) == (paths.repo, paths.source)
        observed.append("verified-source")
        yield prepared

    def build(actual_paths, actual_prepared, mode, *, select):
        assert actual_paths == paths and actual_prepared is prepared
        assert mode == "development" and select is False
        observed.append("verified-build")

    monkeypatch.setattr(Path, "exists", exists)
    monkeypatch.setattr(environment.source_preparation, "verified_source", verified_source)
    monkeypatch.setattr(environment, "_build_verified_python", build)
    monkeypatch.setattr(
        environment.subprocess, "run",
        lambda *args, **kwargs: pytest.fail("no real build command in a layout-probe unit"),
    )
    if case_sensitive:
        environment.build_python(paths, "development", select=False)
        assert observed == ["verified-source", "verified-build"]
    else:
        with pytest.raises(environment.CPythonEnvironmentError, match="case-insensitive"):
            environment.build_python(paths, "development", select=False)
        assert not observed, "layout refusal must precede source admission and any build"
    assert len(probes) == 1
    assert not probes[0].parent.exists(), "the real probe must clean up on both exits"
    assert not (paths.build / ".soac-cpython-build.json").exists()
    assert environment.CPythonPaths.from_environment(paths.repo, {}).build == old_build


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("version", 2, "proof version"),
        ("executable", "/different/python", "candidate executable"),
        ("base_executable", "/different/python", "candidate executable"),
        ("py_debug", 0, "Py_DEBUG"),
        ("has_totalrefcount", False, "refcount support"),
        ("gil_disabled", 1, "GIL-enabled"),
        ("pointer_bits", 32, "64 bits"),
        ("configure_cppflags", "", "CPPFLAGS"),
        ("config_args", "--enable-shared", "configure arguments"),
        ("loaded_libpython", ["/different/libpython.so"], "actual loaded libpython"),
        ("deleted_libpython", True, "actual loaded libpython"),
        ("symbols", {}, "debug-only exports"),
    ],
)
def test_stackref_debug_proof_rejects_flags_without_actual_runtime_identity(
    tmp_path: Path, field: str, value: object, message: str,
) -> None:
    paths = separate_paths(tmp_path)
    proof = fake_stackref_debug_info(paths)
    # A real Linux build may have distinct hard-link filenames for one library.
    environment.validate_stackref_debug_runtime(paths, proof)
    proof[field] = value
    with pytest.raises(environment.CPythonEnvironmentError, match=message):
        environment.validate_stackref_debug_runtime(paths, proof)


def test_stackref_debug_proof_rejects_a_different_configured_library_file(
    tmp_path: Path,
) -> None:
    paths = separate_paths(tmp_path)
    proof = fake_stackref_debug_info(paths)
    other = paths.library / "libpython-other.so"
    other.write_bytes(b"different native library")
    proof["ldlibrary"] = other.name
    with pytest.raises(environment.CPythonEnvironmentError, match="different files"):
        environment.validate_stackref_debug_runtime(paths, proof)


@pytest.mark.parametrize("failure", [False, True])
def test_stackref_debug_probe_uses_the_candidate_in_an_isolated_process(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failure: bool,
) -> None:
    paths = separate_paths(tmp_path)
    proof = fake_stackref_debug_info(paths)
    observed = []

    def check_output(arguments, **options):
        observed.append((arguments, options))
        if failure:
            raise subprocess.CalledProcessError(1, arguments, stderr="debug probe failed")
        return json.dumps(proof)

    monkeypatch.setattr(environment.subprocess, "check_output", check_output)
    if failure:
        with pytest.raises(environment.CPythonEnvironmentError, match="debug probe failed"):
            environment.stackref_debug_info(paths)
    else:
        assert environment.stackref_debug_info(paths) == proof
    arguments, options = observed.pop()
    assert arguments[:5] == [str(paths.binary), "-I", "-S", "-B", "-c"]
    assert json.loads(arguments[-1]) == list(environment.STACKREF_DEBUG_SYMBOLS)
    assert options["cwd"] == paths.build
    assert options["env"]["LD_LIBRARY_PATH"].split(":")[0] == str(paths.library)
    assert options["stderr"] == subprocess.PIPE
    assert not observed


def test_public_header_preflight_uses_the_configured_makefile_cxx(
    tmp_path: Path,
) -> None:
    paths = environment.CPythonPaths.from_environment(
        tmp_path, {"CPYTHON_BUILD_DIR": "guest build"}
    )
    paths.build.mkdir()
    compiler = tmp_path / "configured compiler.py"
    recorded = tmp_path / "compiler-arguments.json"
    compiler.write_text(
        "import json, pathlib, sys\n"
        f"pathlib.Path({str(recorded)!r}).write_text(json.dumps(sys.argv[1:]))\n"
    )
    command = shlex.join([sys.executable, str(compiler)]).replace("$", "$$")
    cppflags = ["-DPy_STACKREF_DEBUG=1", f"-I{tmp_path / 'configured include'}"]
    configured_cppflags = shlex.join(cppflags).replace("$", "$$")
    (paths.build / "Makefile").write_text(
        f"CXX = {command}\nCONFIGURE_CPPFLAGS = {configured_cppflags}\n"
    )
    variables = dict(
        os.environ, CXX="missing-ambient-compiler",
        CPPFLAGS="-Dwrong_ambient_cppflags",
        CONFIGURE_CPPFLAGS="-Dwrong_ambient_configure_cppflags",
    )
    variables.pop("MAKEFLAGS", None)
    environment.check_public_cxx_header(paths, variables)
    arguments = json.loads(recorded.read_text())
    assert arguments[:-1] == [
        *cppflags,
        "-fsyntax-only",
        "-x",
        "c++",
        f"-I{paths.source / 'Include'}",
        f"-I{paths.build}",
    ]
    assert Path(arguments[-1]).parent.parent == paths.build
    assert not Path(arguments[-1]).exists(), (
        "the syntax probe must not leave build files"
    )


@pytest.mark.parametrize("no_select", [False, True])
@pytest.mark.parametrize("mode", ["optimized", "development", "stackref-debug"])
def test_python_build_cli_forwards_explicit_selection_choice(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, no_select: bool, mode: str,
) -> None:
    observed = []

    def build(paths, mode, *, select=True):
        observed.append((paths.repo, mode, select))

    monkeypatch.setattr(environment, "build_python", build)
    arguments = [
        "cpython_environment.py",
        "--repo",
        str(tmp_path),
        "build",
        "--mode",
        mode,
    ]
    if no_select:
        arguments.append("--no-select")
    monkeypatch.setattr(sys, "argv", arguments)
    assert environment.main() == 0
    assert observed == [(tmp_path, mode, not no_select)]


def test_check_runtime_cli_accepts_the_explicit_debug_mode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed = []

    def check(paths, *, require_mode=None):
        observed.append((paths.repo, require_mode))

    monkeypatch.setattr(environment, "check_runtime", check)
    monkeypatch.setattr(sys, "argv", [
        "cpython_environment.py", "--repo", str(tmp_path),
        "check-runtime", "--require-mode", "stackref-debug",
    ])
    assert environment.main() == 0
    assert observed == [(tmp_path, "stackref-debug")]


def test_no_select_cannot_be_silently_ignored_by_explicit_selection(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        sys, "argv", ["cpython_environment.py", "select-build", "--no-select"]
    )
    with pytest.raises(SystemExit) as caught:
        environment.main()
    assert caught.value.code == 2


def mock_recorded_runtime(paths, monkeypatch):
    paths.build.mkdir(parents=True, exist_ok=True)
    identity = {
        "kind": "gitlink", "revision": "a" * 40,
        "tree": "b" * 40, "checkout_sha256": "c" * 64,
    }
    prepared = {"source_identity": identity, "files": {}}
    runtime = {"shared": 1, "source": str(paths.source), "build": str(paths.build)}
    monkeypatch.setattr(environment, "check_source", lambda _: None)
    monkeypatch.setattr(environment, "runtime_info", lambda _: runtime)
    monkeypatch.setattr(
        environment.source_preparation, "verified_source",
        lambda *_, **__: nullcontext(prepared),
    )
    monkeypatch.setattr(environment.source_preparation, "verify_record", lambda *_: None)
    record = {
        "schema_version": 2, "source": str(paths.source), "build": str(paths.build),
        "source_identity": dict(identity), "runtime": runtime,
        "runtime_files": environment.runtime_files(paths),
        "build_mode": "optimized", "configure": [],
    }
    return prepared, record


@pytest.mark.parametrize(
    ("changed", "message"),
    [
        ("source", "changed since"),
        ("revision", "committed source identity"),
        ("raw_bytes", "committed source identity"),
        ("legacy", "provenance schema"),
        ("artifacts", "changed since"),
    ],
)
def test_python_provenance_rejects_old_native_commit_raw_bytes_or_legacy_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, changed: str, message: str,
) -> None:
    paths = separate_paths(tmp_path)
    prepared, record = mock_recorded_runtime(paths, monkeypatch)
    record_path = paths.build / ".soac-cpython-build.json"
    record_path.write_text(json.dumps(record))
    assert environment.check_runtime(paths) == prepared["source_identity"]
    if changed == "source":
        record["source"] = "/another/source"
    elif changed == "revision":
        prepared["source_identity"]["revision"] = "d" * 40
    elif changed == "raw_bytes":
        prepared["source_identity"]["checkout_sha256"] = "e" * 64
    elif changed == "legacy":
        record.pop("schema_version")
        record["patch_generation"] = "previously-built-and-byte-equivalent"
    else:
        record["runtime_files"] = {"old-library": [1, 2]}
    record_path.write_text(json.dumps(record))
    before = record_path.read_bytes()
    monkeypatch.setattr(
        environment, "runtime_info",
        lambda _: pytest.fail("reject invalid provenance before launching the interpreter"),
    )
    with pytest.raises(environment.CPythonEnvironmentError, match=message):
        environment.check_runtime(paths)
    assert record_path.read_bytes() == before, "verification must never restamp an old build"


def test_runtime_verification_holds_shared_source_lock_through_actual_runtime_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    paths = separate_paths(tmp_path)
    prepared, record = mock_recorded_runtime(paths, monkeypatch)
    (paths.build / ".soac-cpython-build.json").write_text(json.dumps(record))
    events = []
    active = False

    @contextmanager
    def verified(repo, source, *, shared=False):
        nonlocal active
        assert repo == paths.repo and source == paths.source and shared
        active = True
        events.append("locked")
        try:
            yield prepared
        finally:
            active = False
            events.append("unlocked")

    def runtime_info(actual):
        assert active and actual == paths
        events.append("runtime")
        return record["runtime"]

    def recheck(repo, source, expected):
        assert active and (repo, source, expected) == (paths.repo, paths.source, prepared)
        events.append("source-rechecked")

    monkeypatch.setattr(environment.source_preparation, "verified_source", verified)
    monkeypatch.setattr(environment.source_preparation, "verify_record", recheck)
    monkeypatch.setattr(environment, "runtime_info", runtime_info)
    assert environment.check_runtime(paths) == prepared["source_identity"]
    assert events == ["locked", "runtime", "source-rechecked", "unlocked"]


def test_benchmark_readiness_rejects_nonoptimized_or_unrecorded_builds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    paths = separate_paths(tmp_path)
    proof = fake_stackref_debug_info(paths)
    _, record = mock_recorded_runtime(paths, monkeypatch)
    debug_calls = []

    def debug_info(actual_paths):
        debug_calls.append(actual_paths)
        return proof

    monkeypatch.setattr(environment, "stackref_debug_info", debug_info)
    record_path = paths.build / ".soac-cpython-build.json"
    for mode in (None, "development", "stackref-debug", "optimized"):
        debug_calls.clear()
        record["build_mode"] = mode
        if mode == "stackref-debug":
            record["stackref_debug"] = proof
        else:
            record.pop("stackref_debug", None)
        record_path.write_text(json.dumps(record))
        if mode is None:
            with pytest.raises(environment.CPythonEnvironmentError, match="recorded build mode"):
                environment.check_runtime(paths)
            continue
        environment.check_runtime(paths)
        if mode != "optimized":
            with pytest.raises(
                environment.CPythonEnvironmentError, match="requires CPython build mode",
            ):
                environment.check_runtime(paths, require_mode="optimized")
        else:
            environment.check_runtime(paths, require_mode="optimized")
        assert debug_calls == ([paths] if mode == "stackref-debug" else [])


@pytest.mark.parametrize("changed", ["missing", "proof"])
def test_stackref_debug_readiness_requires_the_recorded_live_proof(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, changed: str,
) -> None:
    paths = separate_paths(tmp_path)
    proof = fake_stackref_debug_info(paths)
    _, record = mock_recorded_runtime(paths, monkeypatch)
    monkeypatch.setattr(environment, "stackref_debug_info", lambda _: proof)
    record["build_mode"] = "stackref-debug"
    record["stackref_debug"] = dict(proof)
    record_path = paths.build / ".soac-cpython-build.json"
    record_path.write_text(json.dumps(record))
    environment.check_runtime(paths, require_mode="stackref-debug")
    if changed == "missing":
        record.pop("stackref_debug")
        record_path.write_text(json.dumps(record))
        message = "no recorded runtime proof"
    else:
        proof["configure_cflags_nodist"] = "-O0 -g -fno-inline"
        message = "runtime proof changed"
    with pytest.raises(environment.CPythonEnvironmentError, match=message):
        environment.check_runtime(paths, require_mode="stackref-debug")


def test_runtime_source_check_allows_its_receipt_but_rejects_new_source_before_launch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tests.test_cpython_patches import make_committed_checkout

    repo, source = make_committed_checkout(tmp_path)
    paths = environment.CPythonPaths.from_environment(repo, {})
    assert paths.source == source and paths.build == source
    prepared = environment.source_preparation.verify_source(repo, source)
    runtime = {"shared": 1, "source": str(source), "build": str(source)}
    record = {
        "schema_version": 2, "source": str(source), "build": str(source),
        "source_identity": prepared["source_identity"], "runtime": runtime,
        "runtime_files": environment.runtime_files(paths),
        "build_mode": "optimized", "configure": [],
    }
    (source / ".soac-cpython-build.json").write_text(json.dumps(record))
    # Only native mount/runtime probes are replaced; source/gitlink/index/raw
    # checkout verification is the real implementation.
    monkeypatch.setattr(environment, "check_source", lambda _: None)
    monkeypatch.setattr(environment, "runtime_info", lambda _: runtime)
    assert environment.check_runtime(paths) == prepared["source_identity"]
    added_source = source / "new_hook.c"
    added_source.write_text("new source\n")
    monkeypatch.setattr(
        environment, "runtime_info",
        lambda _: pytest.fail("reject new sources before starting the interpreter"),
    )
    with pytest.raises(environment.CPythonEnvironmentError, match="uncommitted/untracked"):
        environment.check_runtime(paths)
    assert added_source.read_text() == "new source\n"


def test_source_migration_removes_only_the_exact_fstab_bind() -> None:
    guest = Path("/guest/cpython")
    shared = Path("/shared/vendor/cpython")
    kept = "# preserve comments\n/dev/root / ext4 defaults 0 1\n"
    unrelated = "/other/cpython /other/vendor/cpython none bind,nofail 0 0\n"
    bind = f"{guest} {shared} none bind,nofail,x-systemd.requires=/shared 0 0\n"
    assert (
        migration.without_source_bind(kept + bind + unrelated, guest, shared)
        == kept + unrelated
    )
    for invalid in (kept, kept + bind + bind):
        with pytest.raises(environment.CPythonEnvironmentError, match="exactly one"):
            migration.without_source_bind(invalid, guest, shared)


def prepared_migration(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    repo = tmp_path / "repo"
    work = repo / "work/migration"
    guest = tmp_path / "guest"
    source = repo / "vendor/cpython"
    staged = work / "shared-source"
    for path in (source, staged, guest):
        path.mkdir(parents=True)
    (source / "identity").write_text("old-host")
    (staged / "identity").write_text("pinned-guest")
    (guest / "identity").write_text("pinned-guest-original")
    (work / "migration.json").write_text(
        json.dumps(
            {
                "repo": str(repo),
                "guest_source": str(guest),
                "source_revision": "pinned",
                "host_revision": "old-host",
                "promoted": False,
            }
        )
    )
    fstab = tmp_path / "fstab"
    fstab.write_text(
        f"/dev/root / ext4 defaults 0 1\n{guest} {source} none bind,nofail 0 0\n"
    )
    return repo, work, guest, fstab


def test_successful_build_selection_is_repo_local_and_explicit_overrides_win(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        environment.sources.tempfile, "gettempdir", lambda: str(tmp_path.parent)
    )
    source = tmp_path / "vendor/cpython"
    build = tmp_path / "guest-build"
    environment.sources.save_selected_build(tmp_path, source, build)
    assert environment.CPythonPaths.from_environment(tmp_path, {}).build == build
    assert (
        environment.CPythonPaths.from_environment(
            tmp_path, {"CPYTHON_BUILD_DIR": "another-build"}
        ).build
        == tmp_path / "another-build"
    )
    alternate = environment.CPythonPaths.from_environment(
        tmp_path, {"CPYTHON_SOURCE_DIR": "other-source"}
    )
    assert alternate.build == alternate.source


@pytest.mark.parametrize(
    "temporary_root", ["/tmp", "/var/tmp", "/configured-system-temporary"]
)
@pytest.mark.parametrize("symlink_alias", [False, True])
def test_build_selection_rejects_external_temporary_storage_without_replacing_it(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    temporary_root: str,
    symlink_alias: bool,
) -> None:
    repo = tmp_path / "checkout"
    source = repo / "vendor/cpython"
    previous_build = repo / "work/previous-build"
    environment.sources.save_selected_build(repo, source, previous_build)
    selection = repo / "work/cpython-selected-build.json"
    previous_selection = selection.read_bytes()
    monkeypatch.setattr(
        environment.sources.tempfile,
        "gettempdir",
        lambda: "/configured-system-temporary",
    )
    build = Path(temporary_root) / "soac-disposable-build"
    if symlink_alias:
        alias = repo / "guest-build-alias"
        alias.symlink_to(build, target_is_directory=True)
        build = alias

    with pytest.raises(
        environment.CPythonEnvironmentError, match="persist across VM restarts"
    ):
        environment.sources.save_selected_build(repo, source, build)

    assert selection.read_bytes() == previous_selection
    assert environment.CPythonPaths.from_environment(repo, {}).build == previous_build


def test_temporary_build_rejection_does_not_create_a_selection(tmp_path: Path) -> None:
    repo = tmp_path / "unselected-checkout"
    with pytest.raises(
        environment.CPythonEnvironmentError, match="persist across VM restarts"
    ):
        environment.sources.save_selected_build(
            repo, repo / "vendor/cpython", Path("/tmp/soac-disposable-build")
        )
    assert not repo.exists()


def test_build_selection_allows_persistent_sibling_of_configured_temporary_storage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        environment.sources.tempfile,
        "gettempdir",
        lambda: "/configured-system-temporary",
    )
    build = Path("/configured-system-temporary-sibling/cpython-build")
    environment.sources.save_selected_build(
        tmp_path, tmp_path / "vendor/cpython", build
    )
    assert environment.CPythonPaths.from_environment(tmp_path, {}).build == build


def test_corrupt_build_selection_is_not_silently_reused(tmp_path: Path) -> None:
    selection = tmp_path / "work/cpython-selected-build.json"
    selection.parent.mkdir()
    selection.write_text("{}")
    with pytest.raises(environment.CPythonEnvironmentError, match="build selection"):
        environment.CPythonPaths.from_environment(tmp_path, {})


def test_source_migration_restores_mount_and_fstab_if_host_revision_changed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo, work, guest, fstab = prepared_migration(tmp_path)
    source = repo / "vendor/cpython"
    writes, commands = [], []
    monkeypatch.setattr(migration, "require_overlay", lambda *_: source)
    monkeypatch.setattr(migration, "pinned_revision", lambda _: "pinned")
    monkeypatch.setattr(migration, "write_fstab", writes.append)
    monkeypatch.setattr(
        migration.subprocess, "run", lambda arguments, **_: commands.append(arguments)
    )

    def check_revision(path: Path, expected: str) -> None:
        if path == source:
            raise environment.CPythonEnvironmentError("host changed")

    monkeypatch.setattr(migration, "require_clean_revision", check_revision)
    with pytest.raises(environment.CPythonEnvironmentError, match="host changed"):
        migration.promote(repo, work, fstab=fstab)

    assert writes[-1] == fstab.read_text()
    assert commands == [
        ["sudo", "umount", "--", str(source)],
        ["sudo", "mount", "--bind", str(guest), str(source)],
    ]
    assert (source / "identity").read_text() == "old-host"
    assert (work / "shared-source/identity").read_text() == "pinned-guest"


def test_source_migration_preserves_both_original_trees(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo, work, guest, fstab = prepared_migration(tmp_path)
    source = repo / "vendor/cpython"
    monkeypatch.setattr(migration, "require_overlay", lambda *_: source)
    monkeypatch.setattr(migration, "pinned_revision", lambda _: "pinned")
    monkeypatch.setattr(migration, "require_clean_revision", lambda *_: None)
    monkeypatch.setattr(migration, "mount_for", lambda _: {"fstype": "virtiofs"})
    monkeypatch.setattr(migration, "write_fstab", lambda _: None)
    monkeypatch.setattr(migration.subprocess, "run", lambda *_, **__: None)

    migration.promote(repo, work, fstab=fstab)

    assert (source / "identity").read_text() == "pinned-guest"
    assert (work / "host-original/identity").read_text() == "old-host"
    assert (guest / "identity").read_text() == "pinned-guest-original"
    assert (work / "fstab.before").read_text() == fstab.read_text()
    assert json.loads((work / "migration.json").read_text())["promoted"]


@pytest.mark.parametrize("configured_source", [None, "separate-source"])
def test_check_source_cli_performs_locked_committed_byte_verification(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, configured_source: str | None,
) -> None:
    # The Justfile intentionally exports the selected source. This fixture
    # owns its source choice and exercises both default and explicit routing.
    if configured_source is None:
        monkeypatch.delenv("CPYTHON_SOURCE_DIR", raising=False)
    else:
        monkeypatch.setenv("CPYTHON_SOURCE_DIR", configured_source)
    expected_source = tmp_path / (configured_source or "vendor/cpython")
    calls = []
    monkeypatch.setattr(environment, "check_source", lambda paths: calls.append(("layout", paths.source)))
    monkeypatch.setattr(
        environment.source_preparation, "verify_source",
        lambda repo, source: calls.append(("committed", repo, source)),
    )
    monkeypatch.setattr(sys, "argv", [
        "cpython_environment.py", "--repo", str(tmp_path), "check-source",
    ])
    assert environment.main() == 0
    assert calls == [
        ("layout", expected_source),
        ("committed", tmp_path, expected_source),
    ]
