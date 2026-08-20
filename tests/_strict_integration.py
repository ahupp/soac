"""Real offline-checker -> native startup -> explicitly selected backend boundary.

This helper never manufactures facts, capabilities, or a replacement loader.
The selected interpreter and extension must already be built by the repository
test entrypoint. Checker compilation uses its own target directory, once per
test process; it does not rebuild the runtime or create another venv.
"""

from __future__ import annotations

import fcntl
import json
import os
import subprocess
import sys
import tempfile
import textwrap
from collections.abc import Mapping
from dataclasses import dataclass, field
from functools import lru_cache
from pathlib import Path
from types import FunctionType, MappingProxyType, ModuleType

ROOT = Path(__file__).resolve().parents[1]


@lru_cache(maxsize=1)
def _checker() -> Path:
    log_directory = ROOT / "work" / "logs"
    log_directory.mkdir(parents=True, exist_ok=True)
    # Separate pytest workers may reach this fixture together. They must not
    # run Cargo concurrently against the one offline-checker target directory.
    with (log_directory / "strict-integration-checker.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        # Keep every invocation, including partial timeout output. A later
        # worker's success must never replace the failure's only evidence.
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", dir=log_directory,
            prefix="strict-integration-checker-build-", suffix=".log", delete=False,
        ) as output:
            log = Path(output.name)
            try:
                result = subprocess.run(
                    [
                        sys.executable,
                        str(ROOT / "scripts" / "run_ty.py"),
                        "--debug-build",
                        "--",
                        "--help",
                    ],
                    check=False,
                    cwd=ROOT,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    timeout=900,
                )
            except (OSError, subprocess.TimeoutExpired) as error:
                raise AssertionError(f"checker build did not finish; {log}\n{error}") from error
        assert result.returncode == 0, (
            f"checker build failed (exit {result.returncode}); {log}\n"
            + log.read_text(errors="replace")
        )
    return ROOT / "work" / "target-ty" / "debug" / "soac-ty"


@dataclass(frozen=True)
class StrictValidationCase:
    """One reviewed source's ordinary validation tail and native witnesses."""

    validate_source: str
    module_path: Path
    required_functions: tuple[str, ...] = ()


def _plain_function_witness(module: ModuleType, path: str) -> FunctionType:
    """Read an explicit function/method test witness without binding it.

    Dotted components traverse only own class namespaces through the built-in
    type namespace descriptor, including classes with custom metaclasses.
    Exact staticmethod/classmethod wrappers expose their stored function; no
    user-defined lookup hook or descriptor is invoked. Inheritance, instances
    and custom descriptors are deliberately not lookup mechanisms. This selects
    an object for native authentication; it never supplies authority or an
    inferred execution mode.
    """
    if type(module) is not ModuleType:
        raise TypeError("function witness root must be an exact module")
    components = path.split(".")
    if not all(components):
        raise ValueError("function witness path has an empty component")
    value = vars(module)[components[0]]
    class_namespace = type.__dict__["__dict__"]
    for component in components[1:]:
        if not issubclass(type(value), type):
            raise TypeError("function witness path requires a class namespace")
        value = class_namespace.__get__(value, type(value))[component]
    if type(value) in (staticmethod, classmethod):
        value = value.__func__
    if type(value) is not FunctionType:
        raise TypeError("function witness must be a plain Python function")
    return value


def _effective_backend_environment(
    backend: str,
    environment: Mapping[str, str],
    *,
    entry_interpreter: bool = False,
    opt_mode: str = "none",
    extra_env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    """One publication/replay policy; preserve every unrelated signed input."""
    if backend not in {"soac", "cpython"}:
        raise ValueError(f"unknown strict execution backend: {backend!r}")
    result = dict(environment)
    if backend == "cpython":
        if entry_interpreter or opt_mode != "none":
            raise ValueError("the CPython backend cannot select a SOAC entry or optimizer mode")
        execution_keys = {
            "DIET_PYTHON_MODE", "SOAC_OPT_MODE", "SOAC_COMPILE_MODE",
            "SOAC_BACKGROUND_JIT",
        }
        unsupported = execution_keys.intersection(extra_env or ())
        if unsupported:
            raise ValueError(
                "the CPython backend does not accept SOAC execution overrides: "
                + ", ".join(sorted(unsupported))
            )
        for name in execution_keys:
            result.pop(name, None)
    if extra_env:
        result.update(extra_env)
    return result


def _assert_cpython_module_witness(
    module: ModuleType,
    *,
    module_name: str,
    source_path: str,
    source_sha256: str,
    artifact_generation: str,
) -> dict:
    """Check the native owner's diagnostic, not mutable module attributes."""
    from soac import _soac_ext

    assert _soac_ext.runtime_compilation_activity() == {
        "schema": 1,
        "lowering_entries": 0,
        "blockpy_cache_entries": 0,
        "jit_engine_entries": 0,
    }, "the interpreter backend entered a SOAC compilation path"
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    assert diagnostic is not None, "selected source executed without native ownership"
    assert diagnostic["schema"] == 1
    assert diagnostic["backend"] == "cpython"
    assert diagnostic["initializer_entry_kind"] == "original_code"
    assert diagnostic["original_code_entered"] is True
    assert diagnostic["sealed"] is True
    assert diagnostic["module_name"] == module_name
    assert diagnostic["source_path"] == source_path
    assert diagnostic["source_sha256"] == source_sha256
    assert diagnostic["artifact_generation"] == artifact_generation
    assert type(diagnostic["startup_identity"]) is str
    assert len(diagnostic["startup_identity"]) == 64
    assert type(diagnostic["interpreter_id"]) is int
    assert diagnostic["interpreter_id"] >= 0
    return diagnostic


def _assert_cpython_function_witness(
    function: FunctionType,
    module_diagnostic: Mapping,
) -> dict:
    """Require an authenticated original-code body; declarations need not run."""
    import ctypes
    from soac import _soac_ext

    assert type(function) is FunctionType
    diagnostic = _soac_ext.strict_function_diagnostics(function)
    assert diagnostic is not None, "function has no matching native source/body owner"
    assert diagnostic["schema"] == 2
    assert diagnostic["backend"] == "cpython"
    assert diagnostic["entry_kind"] == "original_code"
    assert type(diagnostic["original_code_entered"]) is bool
    for key in ("source_path", "source_sha256", "artifact_generation"):
        assert diagnostic[key] == module_diagnostic[key], (key, diagnostic)
    # This is only secondary negative evidence. Native diagnostics above own
    # authentication; lack of JIT metadata never proves strict admission.
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert metadata(function) is None, "original-code function acquired JIT metadata"
    return diagnostic


_VALIDATION_PRELUDE = f"""
import ctypes
import importlib
from pathlib import Path
sys.path.insert(0, {str(ROOT)!r})
from tests._integration import exec_integration_validation
from tests._strict_integration import (
    _plain_function_witness, _assert_cpython_module_witness,
    _assert_cpython_function_witness,
)
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
"""


@dataclass
class StrictProject:
    root: Path
    project: Path
    deployment: Path
    publication: dict
    modules: Mapping[str, str]
    backend: str = "soac"
    environment: Mapping[str, str] | None = field(default=None, repr=False)
    _invocations: int = field(default=0, init=False)

    def _selected_backend(self, requested: str | None) -> str:
        backend = self.backend if requested is None else requested
        if backend != self.backend:
            raise ValueError("run backend contradicts the checker's selected project backend")
        return backend

    def _validation_program(
        self,
        module_name: str,
        case: StrictValidationCase,
        *,
        entry_interpreter: bool,
        backend: str = "soac",
    ) -> str:
        if module_name not in self.modules:
            raise ValueError(
                f"integration module {module_name!r} was not selected for analysis"
            )
        source_path = self.project / self.modules[module_name]
        if backend == "cpython":
            import hashlib

            source_sha256 = hashlib.sha256(source_path.read_bytes()).hexdigest()
            return textwrap.dedent(
                f"""
                assert {module_name!r} not in sys.modules, 'case was already initialized'
                module = importlib.import_module({module_name!r})
                diagnostic = _assert_cpython_module_witness(
                    module, module_name={module_name!r},
                    source_path={str(source_path)!r},
                    source_sha256={source_sha256!r},
                    artifact_generation={self.publication["generation"]!r},
                )
                for name in {case.required_functions!r}:
                    _assert_cpython_function_witness(
                        _plain_function_witness(module, name), diagnostic,
                    )
                exec_integration_validation(
                    {case.validate_source!r}, module, Path({str(case.module_path)!r}),
                    mode='cpython',
                )
                after = _assert_cpython_module_witness(
                    module, module_name={module_name!r},
                    source_path={str(source_path)!r},
                    source_sha256={source_sha256!r},
                    artifact_generation={self.publication["generation"]!r},
                )
                assert after['startup_identity'] == diagnostic['startup_identity']
                assert after['interpreter_id'] == diagnostic['interpreter_id']
                for name in {case.required_functions!r}:
                    _assert_cpython_function_witness(
                        _plain_function_witness(module, name), after,
                    )
                """
            )
        mode = "entry" if entry_interpreter else "soac"
        expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
        return textwrap.dedent(
            f"""
            assert {module_name!r} not in sys.modules, 'case was already initialized'
            module = importlib.import_module({module_name!r})
            diagnostic = _soac_ext.strict_module_diagnostics(module)
            assert diagnostic is not None, 'selected source executed as an ordinary module'
            assert diagnostic['sealed'] is True
            assert diagnostic['module_name'] == {module_name!r}
            assert diagnostic['source_path'] == {str(source_path)!r}
            assert diagnostic['artifact_generation'] == {self.publication["generation"]!r}
            # Module initializers have an explicit interpreted lowering plan,
            # independent of the requested source-function execution mode.
            assert diagnostic['initializer_entry_kind'] == 'entry_interpreter', diagnostic

            for name in {case.required_functions!r}:
                function = _plain_function_witness(module, name)
                assert metadata(function), f'{{name}} did not register with SOAC'
                assert owner(function), f'{{name}} has no native strict owner'
                actual_entry = _soac_ext.strict_function_entry_kind(function)
                assert actual_entry == {expected_entry!r}, (name, actual_entry)
            exec_integration_validation(
                {case.validate_source!r}, module, Path({str(case.module_path)!r}), mode={mode!r}
            )
            for name in {case.required_functions!r}:
                function = _plain_function_witness(module, name)
                actual_entry = _soac_ext.strict_function_entry_kind(function)
                assert actual_entry == {expected_entry!r}, (name, actual_entry)
            """
        )

    def run_case(
        self,
        module_name: str,
        validate_source: str,
        module_path: Path,
        *,
        entry_interpreter: bool = False,
        opt_mode: str = "none",
        required_functions: tuple[str, ...] = (),
        backend: str | None = None,
    ) -> subprocess.CompletedProcess[str]:
        """Import one selected case and run its ordinary validator after sealing.

        The validation tail is not analyzed and never writes test flags into
        the sealed module. Native diagnostics prove source, generation, and
        the observed initializer entry. Requested function witnesses are
        synchronous and additionally prove native registration and the actual
        public entry path before/after use.
        Dotted witnesses select raw own-namespace functions on exact classes,
        including exact staticmethod/classmethod wrappers; they never invoke
        descriptors or custom metaclass lookup. CPython witnesses use native
        source/body diagnostics, not JIT registration. Omitted backend inherits
        the project selection. An annotation is never a runtime call predicate.
        """
        backend = self._selected_backend(backend)
        _effective_backend_environment(
            backend, self.environment or {}, entry_interpreter=entry_interpreter,
            opt_mode=opt_mode,
        )
        return self.run(
            _VALIDATION_PRELUDE
            + self._validation_program(
                module_name,
                StrictValidationCase(
                    validate_source, module_path, required_functions,
                ),
                entry_interpreter=entry_interpreter,
                backend=backend,
            ),
            entry_interpreter=entry_interpreter,
            opt_mode=opt_mode,
            backend=backend,
        )

    def run_cases(
        self,
        cases: Mapping[str, StrictValidationCase],
        *,
        entry_interpreter: bool = False,
        backend: str | None = None,
    ) -> dict[str, str | None]:
        """Batch explicitly reviewed independent cases in one mode/process.

        This intentionally does not provide per-case process isolation. Every
        module still traverses actual native admission and the same checks as
        run_case. Individual assertion failures are retained, and changes to
        shared interpreter state prevent later cases from being reported as
        passes. Use only for cases whose semantics do not require isolation.
        """
        backend = self._selected_backend(backend)
        _effective_backend_environment(
            backend, self.environment or {}, entry_interpreter=entry_interpreter,
        )
        if not cases:
            raise ValueError("a strict validation batch must contain a reviewed case")
        programs = [
            _VALIDATION_PRELUDE,
            textwrap.dedent("""
                import json
                import os
                from tests._integration import ValidationBatch

                journal = Path(os.environ['SOAC_WORK_DIR']).parent / 'validation-cases.jsonl'
            """),
            f"batch = ValidationBatch({tuple(self.modules)!r}, journal)\n",
        ]
        for index, (name, case) in enumerate(cases.items()):
            programs.append(f"\ndef case_{index}():\n")
            programs.append(
                textwrap.indent(
                    self._validation_program(
                        name, case, entry_interpreter=entry_interpreter, backend=backend
                    ),
                    "    ",
                )
            )
            programs.append(f"\nbatch.run({name!r}, case_{index})\n")
        programs.append("print(json.dumps(batch.results))\n")
        completed = self.run(
            "".join(programs),
            entry_interpreter=entry_interpreter,
            backend=backend,
            timeout=600,
            check=False,
        )
        if completed.returncode != 0:
            error = (
                f"validation batch exited {completed.returncode}; fixture {self.root}\n"
                f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
            )
            return dict.fromkeys(cases, error)
        results = json.loads(completed.stdout.strip().splitlines()[-1])
        assert set(results) == set(cases), "runtime did not report every selected case"
        return results

    def run(
        self,
        program: str,
        *,
        entry_interpreter: bool = False,
        opt_mode: str = "none",
        extra_env: Mapping[str, str] | None = None,
        timeout: int = 120,
        check: bool = True,
        backend: str | None = None,
    ) -> subprocess.CompletedProcess[str]:
        backend = self._selected_backend(backend)
        environment = _effective_backend_environment(
            backend, self.environment if self.environment is not None else os.environ,
            entry_interpreter=entry_interpreter, opt_mode=opt_mode,
            extra_env=extra_env,
        )
        from soac import _soac_ext

        self._invocations += 1
        output = self.root / f"runtime-{self._invocations}"
        output.mkdir()
        # Keep all generated files outside the analyzed project. Do not
        # weaken the production input observations to make a test pass.
        environment.update(
            SOAC_WORK_DIR=str(output / "soac-work"),
            SOAC_MODULE_ENABLED=",".join(
                f"path:{self.project / path}" for path in self.modules.values()
            ),
        )
        if backend == "soac":
            environment.update(
                SOAC_OPT_MODE=opt_mode,
                SOAC_COMPILE_MODE="eager",
                SOAC_BACKGROUND_JIT="0",
                DIET_PYTHON_MODE="transform",
            )
        # Preserve the existing explicit test overrides after runtime defaults.
        # The shared policy already rejected CPython execution-mode overrides.
        if extra_env:
            environment.update(extra_env)
        paths = [
            str(ROOT / "soac_py" / "src"),
            str(Path(_soac_ext.__file__).parent),
            str(self.project),
        ]
        bootstrap = (
            "import sys\n"
            f"sys.path[:0] = {paths!r}\n"
            "from soac import _soac_ext, import_hook\n"
        )
        if backend == "soac":
            bootstrap += (
                f"_soac_ext.force_entry_interpreter_for_tests({entry_interpreter!r})\n"
            )
        bootstrap += f"import_hook.install(backend={backend!r})\n"
        compilation_assertion = ""
        if backend == "cpython":
            compilation_assertion = (
                "\nassert _soac_ext.runtime_compilation_activity() == "
                "{'schema': 1, 'lowering_entries': 0, 'blockpy_cache_entries': 0, "
                "'jit_engine_entries': 0}, _soac_ext.runtime_compilation_activity()\n"
            )
            bootstrap += compilation_assertion
        script = output / "driver.py"
        script.write_text(bootstrap + textwrap.dedent(program) + compilation_assertion)
        command = [
            sys.executable,
            "-I",
            "-B",
            "-X",
            f"soac_strict_config={self.deployment}",
            str(script),
        ]
        result = subprocess.run(
            command,
            check=False,
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            timeout=timeout,
        )
        (output / "stdout.log").write_text(result.stdout)
        (output / "stderr.log").write_text(result.stderr)
        if check:
            assert result.returncode == 0, (
                f"strict runtime exited {result.returncode}; fixture {script}\n"
                f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
            )
        return result


def create_strict_project(
    root: Path,
    sources: Mapping[str, str],
    *,
    modules: Mapping[str, str],
    policy: str | None = None,
    python: str | Path | None = None,
    analysis_timeout: float = 180,
    backend: str = "soac",
) -> StrictProject:
    """Analyze explicit module-name/path pairs with the actual pinned checker.

    ``policy`` optionally supplies the complete pyproject TOML. Other source
    files may be ordinary imported dependencies and are not transformed unless
    selected in ``modules``. Selected sources must contain their own valid
    ``from __future__ import strict`` opt-in; this helper never injects one.
    Native startup authority is published by the CLI.
    ``backend`` selects the fixture's execution choice, not authority.
    Its effective environment is captured before checker publication and reused
    at replay; CPython enrollment removes only SOAC execution-mode variables.
    The actual library/search environment is preserved, including LD_LIBRARY_PATH.
    ``python`` selects the actual interpreter queried by the checker; callers
    testing another venv must also launch their runtime with that executable.
    ``analysis_timeout`` is an explicit budget for larger reviewed projects.
    Timeout failures preserve the checker's partial output beside the source.
    """
    environment = _effective_backend_environment(backend, os.environ)
    root = root.resolve()
    root.mkdir(parents=True, exist_ok=True)
    project = root / "project"
    project.mkdir()
    for name, source in sources.items():
        path = project / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(source).lstrip("\n"))
    if policy is None:
        policy = (
            "[tool.soac.strict]\ninclude = " + json.dumps(list(modules.values())) + "\n"
        )
    (project / "pyproject.toml").write_text(policy)
    authority = root / "authority"
    authority.mkdir()
    signing_key = authority / "signing.key"
    signing_key.write_bytes(bytes(range(32)))
    signing_key.chmod(0o600)
    deployment = authority / "deployment.json"
    command = [
        str(_checker()),
        "check",
        "--project",
        str(project),
        "--python",
        str(python) if python is not None else sys.executable,
        "--signing-key",
        str(signing_key),
        "--output",
        str(root / "artifacts"),
        "--deployment",
        str(deployment),
    ]
    for name, path in modules.items():
        command.extend(["--module", f"{name}={path}"])
    try:
        result = subprocess.run(
            command,
            cwd=ROOT,
            check=False,
            text=True,
            capture_output=True,
            timeout=analysis_timeout,
            env=environment,
        )
    except subprocess.TimeoutExpired as error:
        # subprocess may return bytes on timeout even with text=True. Preserve
        # diagnostics before reporting the setup failure, not one false
        # behavioral failure for every case sharing this analyzed project.
        for stream, output in (("stdout", error.stdout), ("stderr", error.stderr)):
            if isinstance(output, bytes):
                output = output.decode("utf-8", errors="replace")
            (root / f"checker.{stream}.log").write_text(output or "")
        raise AssertionError(
            f"actual checker timed out after {analysis_timeout}s; fixture {project}; "
            f"partial diagnostics: {root / 'checker.stderr.log'}"
        ) from error
    (root / "checker.stdout.log").write_text(result.stdout)
    (root / "checker.stderr.log").write_text(result.stderr)
    assert result.returncode == 0, (
        f"actual checker rejected fixture {project}\n{result.stderr}"
    )
    publication = json.loads(result.stdout)
    assert deployment.is_file()
    return StrictProject(
        root, project, deployment, publication, dict(modules),
        backend=backend, environment=MappingProxyType(dict(environment)),
    )


def assert_strict_source_rejected(
    root: Path,
    source: str,
    *,
    module_name: str,
    diagnostic: str,
) -> str:
    """Require a real checker rejection, with no published startup authority.

    This is an analysis-only negative, not a runtime error or an admission
    witness. Callers supply their actual unsupported source and expected public
    diagnostic; no source or policy is changed to manufacture the rejection.
    """
    filename = f"{module_name}.py"
    try:
        create_strict_project(root, {filename: source}, modules={module_name: filename})
    except AssertionError as error:
        assert "actual checker rejected fixture" in str(error), str(error)
    else:
        raise AssertionError("unsupported strict source was published")
    errors = (root / "checker.stderr.log").read_text()
    assert diagnostic in errors, errors
    assert not (root / "authority" / "deployment.json").exists()
    return errors
