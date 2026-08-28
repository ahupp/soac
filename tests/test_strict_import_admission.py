"""Ordinary imports stay native; only authenticated strict imports enter SOAC."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import (
    ROOT,
    StrictValidationCase,
    assert_strict_source_rejected,
    create_strict_project,
)

_NATIVE_PREDICATES = """
import ctypes
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p
strict_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
strict_owner.argtypes = [ctypes.py_object]
strict_owner.restype = ctypes.c_void_p
"""


@pytest.mark.parametrize(
    "body",
    [
        r"def value(): return '\ud800'",
        r"def value(arg): return f'\ud800{arg}'",
        r"def value(arg): return f'{arg:\ud800}'",
        r"def value(arg): return t'\ud800{arg}'",
        r"def value(arg): return t'{arg:\ud800}'",
        r"""from typing import Literal
def accept(value: Literal["\ud800"]) -> Literal["\ud800"]: return value""",
    ],
    ids=["plain", "f-string", "f-format", "t-string", "t-format", "literal-contract"],
)
def test_strict_source_surrogate_escape_is_rejected_before_publication(tmp_path, body):
    source = f"# soac: module(strict_assign=true, checked_attr=true)\n{body}\n"
    error = assert_strict_source_rejected(
        tmp_path,
        source,
        module_name="unsupported_literal",
        diagnostic="unsupported Unicode surrogate escape U+D800",
    )
    start = source.index(r"\ud800")
    assert f"bytes {start}..{start + 6}" in error


@pytest.fixture(scope="module")
def strict_source_literal_controls(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-source-literal-controls"),
        {
            "literal_controls.py": r"""
                # soac: module(strict_assign=true, checked_attr=true)
                from typing import Literal
                import ordinary_literals

                def replacement(value: Literal["�"]) -> Literal["\ufffd"]:
                    return value

                def backslash(value: Literal[r"\ud800"]) -> Literal[r"\ud800"]:
                    return value

                def controls(value):
                    return (
                        "�", "\ufffd", r"\ud800", "\\ud800",
                        rf"\ud800{value}", rt"\ud800{value}".strings,
                        f"\\ud800{value}", t"\\ud800{value}".strings,
                    )

                def ordinary_values():
                    return ordinary_literals.values()
            """,
            "ordinary_literals.py": r"""
                def values():
                    return "\ud800", "\ud83d\udc0d", "\U0000DFFF"
            """,
        },
        modules={"literal_controls": "literal_controls.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_strict_source_literal_controls_and_ordinary_surrogates_remain_distinct(
    strict_source_literal_controls, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_source_literal_controls.run(
        _NATIVE_PREDICATES
        + f"expected_entry = {expected_entry!r}\n"
        + r"""
import literal_controls as checked
import ordinary_literals as ordinary
for name in ("replacement", "backslash", "controls", "ordinary_values"):
    function = vars(checked)[name]
    assert metadata(function) is not None
    assert strict_owner(function) is not None
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry
assert _soac_ext.strict_module_diagnostics(checked)["sealed"] is True
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert metadata(ordinary.values) is None
assert strict_owner(ordinary.values) is None
assert checked.replacement("�") == "�"
assert checked.backslash(r"\ud800") == r"\ud800"
# Literal annotations are outside the shared mandatory-check subset. They
# must not coerce a runtime surrogate to the source replacement character.
for function in (checked.replacement, checked.backslash):
    surrogate = chr(0xD800)
    assert function(surrogate) is surrogate
assert checked.controls("X") == (
    "�", "�", r"\ud800", r"\ud800", r"\ud800X", (r"\ud800", ""),
    r"\ud800X", (r"\ud800", ""),
)
expected = ([0xD800], [0xD83D, 0xDC0D], [0xDFFF])
assert tuple(list(map(ord, value)) for value in ordinary.values()) == expected
assert tuple(list(map(ord, value)) for value in checked.ordinary_values()) == expected
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_strict_module_diagnostics_report_actual_seal_not_public_attributes(
    tmp_path, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "observed.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                import sys
                from soac import _soac_ext
                initializing = _soac_ext.strict_module_diagnostics(sys.modules[__name__])
                answer = 6 * 7
            """,
        },
        modules={"observed": "observed.py"},
    )
    # The module initializer's lowering plan is always interpreted. This flag
    # selects source-function entries, not a different module-initializer path.
    expected_entry = "entry_interpreter"
    project.run(
        f"""
        import importlib.util
        import types
        spec = importlib.util.find_spec('observed')
        observed = importlib.util.module_from_spec(spec)
        sys.modules['observed'] = observed
        before = _soac_ext.strict_module_diagnostics(observed)
        assert before['sealed'] is False
        assert before['initializer_entry_kind'] is None
        assert spec.loader.exec_module(observed) is None
        initializing = observed.initializing
        assert initializing['sealed'] is False
        assert initializing['initializer_entry_kind'] == {expected_entry!r}, initializing
        sealed = _soac_ext.strict_module_diagnostics(observed)
        assert sealed['sealed'] is True
        assert sealed['initializer_entry_kind'] == {expected_entry!r}
        assert sealed['module_name'] == 'observed'
        assert sealed['artifact_generation'] == {project.publication["generation"]!r}
        assert sealed['source_path'] == {str(project.project / "observed.py")!r}
        assert sealed['source_sha256'] == __import__('hashlib').sha256(__import__('pathlib').Path(sealed['source_path']).read_bytes()).hexdigest()
        assert initializing['startup_identity'] == sealed['startup_identity']
        sealed['sealed'] = False
        sealed['initializer_entry_kind'] = 'public_override'
        unchanged = _soac_ext.strict_module_diagnostics(observed)
        assert unchanged['sealed'] is True
        assert unchanged['initializer_entry_kind'] == {expected_entry!r}
        fake = types.ModuleType('observed')
        fake.__dict__.update(vars(observed))
        fake.sealed = True
        assert _soac_ext.strict_module_diagnostics(fake) is None
        assert observed.answer == 42
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("backend", ["soac", "cpython"])
def test_dependency_changed_after_loader_construction_blocks_later_admission(
    tmp_path, backend
):
    project = create_strict_project(
        tmp_path,
        {
            "first.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                def answer() -> int:
                    return 1
            """,
            "second.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                from dependency import VALUE
                from audit import events
                events.append("second body executed")
                def answer() -> int:
                    return VALUE
            """,
            "dependency.py": "VALUE: int = 1\n",
            "audit.py": "events: list[str] = []\n",
        },
        modules={"first": "first.py", "second": "second.py"},
        backend=backend,
    )
    descriptor = json.loads(project.deployment.read_text())
    assert any(
        record["importer_module"] == "second"
        and record["module"]["module_name"] == "dependency"
        for record in descriptor["analysis_dependencies"]
    )
    native_witness = ""
    if backend == "cpython":
        import hashlib

        source_path = project.project / "first.py"
        native_witness = textwrap.dedent(f"""
        sys.path.insert(0, {str(ROOT)!r})
        from tests._strict_integration import (
            _assert_cpython_function_witness, _assert_cpython_module_witness,
        )

        def assert_first_native():
            diagnostic = _assert_cpython_module_witness(
                first, module_name="first", source_path={str(source_path)!r},
                source_sha256={hashlib.sha256(source_path.read_bytes()).hexdigest()!r},
                artifact_generation={project.publication["generation"]!r},
            )
            observed = _assert_cpython_function_witness(
                first.answer, diagnostic,
            )
            assert observed["original_code_entered"] is True
        """)
    project.run(
        _NATIVE_PREDICATES
        + native_witness
        + textwrap.dedent(f"""
        from pathlib import Path
        from soac import StrictRuntimeUnavailableError
        import audit
        import first
        # A real, sealed import establishes that the process's native loader
        # has already authenticated and published the complete generation.
        assert _soac_ext.strict_module_diagnostics(first)['sealed'] is True
        assert strict_owner(first.answer) is not None
        if {backend == "cpython"!r}:
            assert metadata(first.answer) is None
        else:
            assert metadata(first.answer) is not None
        assert first.answer() == 1
        if {backend == "cpython"!r}:
            assert_first_native()
        assert 'second' not in sys.modules
        dependency = Path({str(project.project / "dependency.py")!r})
        before = dependency.read_bytes()
        after = b'VALUE: int = 2\\n'
        assert len(before) == len(after)
        dependency.write_bytes(after)
        try:
            import second
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError('constructor observations bypassed fresh admission')
        assert audit.events == []
        assert 'second' not in sys.modules
        if {backend == "cpython"!r}:
            assert_first_native()
    """),
        backend=backend,
    )


def test_native_prefix_binds_selected_venv_and_ignores_sys_prefix_spoof(tmp_path: Path):
    from soac import _soac_ext

    environments = []
    for name, value in (("venv-a", 41), ("venv-b", 99)):
        prefix = tmp_path / name
        created = subprocess.run(
            [
                sys.executable,
                "-I",
                "-B",
                "-m",
                "venv",
                "--without-pip",
                "--symlinks",
                str(prefix),
            ],
            check=False,
            text=True,
            capture_output=True,
            timeout=60,
        )
        assert created.returncode == 0, created.stdout + created.stderr
        executable = prefix / "bin" / "python"
        queried = subprocess.run(
            [
                str(executable),
                "-I",
                "-B",
                "-c",
                (
                    "import ctypes, json, os, sys, sysconfig; "
                    "native_prefix = ctypes.pythonapi.PySoac_GetInterpreterPrefix; "
                    "native_prefix.restype = ctypes.c_wchar_p; "
                    "print(json.dumps(dict(executable=os.path.realpath(sys.executable), "
                    "prefix=sys.prefix, native_prefix=native_prefix(), "
                    "site_packages=sysconfig.get_path('purelib'))))"
                ),
            ],
            check=False,
            text=True,
            capture_output=True,
            timeout=30,
        )
        assert queried.returncode == 0, queried.stdout + queried.stderr
        identity = json.loads(queried.stdout)
        assert Path(identity["prefix"]) == prefix
        assert Path(identity["native_prefix"]) == prefix
        dependency = Path(identity["site_packages"]) / "prefix_dependency.py"
        dependency.write_text(f"VALUE: int = {value}\n")
        environments.append((executable, identity, dependency))

    selected, selected_identity, selected_dependency = environments[0]
    other, other_identity, _ = environments[1]
    assert selected_identity["executable"] == other_identity["executable"]
    assert selected.samefile(other)
    project = create_strict_project(
        tmp_path / "contract",
        {
            "prefix_target.py": """
            # soac: module(strict_assign=true, checked_attr=true)
            from prefix_dependency import VALUE
            def read() -> int:
                return VALUE
        """
        },
        modules={"prefix_target": "prefix_target.py"},
        python=selected,
    )
    deployment = json.loads(project.deployment.read_text())
    assert deployment["target_interpreter"]["prefix"] == selected_identity["prefix"]
    observed_paths = {item["path"] for item in deployment["analysis_inputs"]}
    assert str(selected_dependency) in observed_paths
    selected_configuration = selected.parent.parent / "pyvenv.cfg"
    preserved = {
        path: path.read_bytes()
        for path in (selected_dependency, selected_configuration, project.deployment)
    }
    paths = [
        str(ROOT / "soac_py" / "src"),
        str(Path(_soac_ext.__file__).parent),
        str(project.project),
    ]

    # A shared executable and unchanged A dependencies are not authority for B.
    # Spoofing sys.prefix must neither reject A nor make B match A. Repeat the
    # unmodified A case last to distinguish this mismatch from stale artifacts.
    cases = (
        (selected, None, True),
        (selected, other_identity["prefix"], True),
        (other, None, False),
        (other, selected_identity["prefix"], False),
        (selected, None, True),
    )
    for index, (executable, spoof, admitted) in enumerate(cases):
        output = tmp_path / f"prefix-runtime-{index}"
        output.mkdir()
        script = output / "driver.py"
        script.write_text(
            textwrap.dedent(f"""
            import ctypes, json, sys
            sys.path[:0] = {paths!r}
            native_prefix = ctypes.pythonapi.PySoac_GetInterpreterPrefix
            native_prefix.restype = ctypes.c_wchar_p
            original_prefix = native_prefix()
            unavailable = ctypes.pythonapi.PySoac_GetStrictRuntimeUnavailableError
            unavailable.restype = ctypes.c_void_p
            # This native getter borrows its result. A function restype of
            # py_object would consume that reference rather than pinning it.
            unavailable_error = ctypes.cast(unavailable(), ctypes.py_object).value
            spoof = {spoof!r}
            if spoof is not None:
                sys.prefix = spoof
            assert native_prefix() == original_prefix
            try:
                from soac import import_hook
                import_hook.install()
                import prefix_target
            except unavailable_error as error:
                result = dict(admitted=False, error=str(error))
            else:
                owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
                owner.argtypes = [ctypes.py_object]
                owner.restype = ctypes.c_void_p
                assert owner(prefix_target.read) is not None
                result = dict(admitted=True, value=prefix_target.read())
            result.update(native_prefix=original_prefix, visible_prefix=sys.prefix)
            print(json.dumps(result))
        """)
        )
        result = subprocess.run(
            [
                str(executable),
                "-I",
                "-B",
                "-X",
                f"soac_strict_config={project.deployment}",
                str(script),
            ],
            check=False,
            cwd=ROOT,
            env={
                **os.environ,
                "SOAC_MODULE_ENABLED": f"path:{project.project}",
                "SOAC_OPT_MODE": "none",
                "SOAC_WORK_DIR": str(output / "soac-work"),
                "SOAC_COMPILE_MODE": "eager",
                "SOAC_BACKGROUND_JIT": "0",
            },
            text=True,
            capture_output=True,
            timeout=90,
        )
        (output / "stdout.log").write_text(result.stdout)
        (output / "stderr.log").write_text(result.stderr)
        assert result.returncode == 0, result.stdout + result.stderr
        observed = json.loads(result.stdout)
        assert observed["admitted"] is admitted, observed
        if admitted:
            assert observed["value"] == 41
        for path, contents in preserved.items():
            assert path.read_bytes() == contents


@pytest.mark.parametrize("mode", ["profile", "apply"])
def test_ordinary_source_and_frozen_imports_keep_native_execution(
    tmp_path: Path, mode: str
):
    # A coding cookie must be handled by the ordinary loader, not UTF-8-only
    # lowering before deciding whether any strict authority exists.
    (tmp_path / "ordinary_probe.py").write_bytes(
        b"# coding: latin-1\nlabel = '\xe9'\n"
        b"def calculate(value):\n    return value + 1\n"
    )
    work = tmp_path / "soac-work"
    program = _NATIVE_PREDICATES + textwrap.dedent(
        f"""
        import importlib.machinery, importlib.util, os, types
        import sys
        sys.path.insert(0, {str(tmp_path)!r})
        from soac import _soac_ext, import_hook
        import_hook.install()
        import ordinary_probe

        assert type(ordinary_probe) is types.ModuleType
        assert type(ordinary_probe.__loader__) is importlib.machinery.SourceFileLoader
        assert ordinary_probe.label == '\u00e9'
        assert metadata(ordinary_probe.calculate) is None
        assert strict_owner(ordinary_probe.calculate) is None
        assert ordinary_probe.calculate(2 ** 100) == 2 ** 100 + 1
        assert _soac_ext.exec_module(ordinary_probe) is False

        # An ordinary file named after the compiler runtime is not provenance.
        spec = importlib.util.spec_from_file_location(
            'soac.runtime', {str(tmp_path / "ordinary_probe.py")!r}
        )
        spec = import_hook.SoacFinder.wrap_spec(spec)
        spoof = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(spoof)
        assert metadata(spoof.calculate) is None
        assert strict_owner(spoof.calculate) is None
        assert spoof.calculate(41) == 42

        # Preserve FrozenImporter itself and its metadata, not an execution of
        # the corresponding Lib source masquerading as the ordinary path.
        os.environ.pop('SOAC_MODULE_ENABLED', None)
        spec = importlib.machinery.FrozenImporter.find_spec('runpy')
        assert spec is not None and spec.origin == 'frozen'
        spec = import_hook.SoacFinder.wrap_spec(spec)
        frozen = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(frozen)
        assert spec.loader is importlib.machinery.FrozenImporter
        assert spec.origin == 'frozen' and not spec.has_location
        assert frozen.__loader__ is importlib.machinery.FrozenImporter
        assert metadata(frozen.run_module) is None
        """
    )
    result = subprocess.run(
        [sys.executable, "-c", program],
        check=False,
        text=True,
        capture_output=True,
        env={
            **os.environ,
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_OPT_MODE": mode,
            "SOAC_WORK_DIR": str(work),
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        },
        timeout=90,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    from soac import _soac_ext

    if (work / "profile.bin").exists():
        profile = json.loads(
            _soac_ext.inspect_counter_dump_json(str(work / "profile.bin"))
        )
        assert not any(
            record["module_name"] == "ordinary_probe" for record in profile["records"]
        )
    if (work / "jit-code-summary.jsonl").exists():
        rows = [
            json.loads(line)
            for line in (work / "jit-code-summary.jsonl").read_text().splitlines()
        ]
        assert not any(
            row.get("module_name") in {"ordinary_probe", "soac.runtime"} for row in rows
        )


def test_native_caller_enters_authenticated_strict_function_without_adopting_plain_code(
    tmp_path: Path,
):
    project = create_strict_project(
        tmp_path,
        {
            "strict_target.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                def add(value: int) -> int:
                    return value + 1
            """,
            "ordinary_caller.py": """
                def invoke(callback, value):
                    return callback(value)
            """,
            "ordinary_sink.py": """
                saved = []
            """,
            "failed_target.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                import sys
                import ordinary_sink
                current = sys.modules[__name__]
                assert current is not None
                ordinary_sink.saved.append((current, current.__loader__))
                raise ValueError('deliberate initialization failure')
            """,
        },
        modules={
            "strict_target": "strict_target.py",
            "failed_target": "failed_target.py",
        },
    )
    program = _NATIVE_PREDICATES + textwrap.dedent(
        """
        import importlib, importlib.machinery, types
        import ordinary_caller
        import soac.runtime as runtime
        # Mutable runtime metadata is not permission to skip strict loading.
        runtime._SOAC_RUNTIME_READY = False
        try:
            import strict_target
        finally:
            runtime._SOAC_RUNTIME_READY = True
        from soac import _soac_ext
        from soac.strict import StrictRuntimeUnavailableError

        assert type(ordinary_caller) is types.ModuleType
        assert type(ordinary_caller.__loader__) is importlib.machinery.SourceFileLoader
        assert metadata(ordinary_caller.invoke) is None
        assert strict_owner(ordinary_caller.invoke) is None
        assert metadata(strict_target.add) is not None
        assert strict_owner(strict_target.add) is not None
        for value in range(64):
            assert ordinary_caller.invoke(strict_target.add, value) == value + 1
        try:
            ordinary_caller.invoke(strict_target.add, 'invalid')
        except TypeError:
            pass
        else:
            raise AssertionError('the original addition accepted an invalid operand')

        events = []
        marker = object()
        class Operand:
            def __add__(self, increment):
                events.append(increment)
                return marker
        assert ordinary_caller.invoke(strict_target.add, Operand()) is marker
        assert events == [1], 'an annotation prevented the native caller from entering the body'

        plain = {}
        exec(compile('def loose(value): return value + 5', '<ordinary>', 'exec',
                     dont_inherit=True), plain)
        loose = types.FunctionType(plain['loose'].__code__, strict_target.__dict__)
        assert metadata(loose) is None and strict_owner(loose) is None
        assert loose(37) == 42

        copied = types.FunctionType(strict_target.add.__code__, strict_target.__dict__)
        assert metadata(copied) is None and strict_owner(copied) is None
        try:
            copied(41)
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError('copied strict code acquired source execution authority')

        # A sealed or terminal owned module never becomes an ordinary-loader
        # retry merely because exec_module failed. Its native identity remains.
        try:
            _soac_ext.exec_module(strict_target)
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError('strict module reload was treated as an ordinary import')
        assert strict_target.add(41) == 42

        # Failed initialization may leave an escaped, now-terminal module.
        # Neither extension nor Python-loader entry may retry its source.
        import ordinary_sink
        try:
            import failed_target
        except ValueError as error:
            assert str(error) == 'deliberate initialization failure'
        else:
            raise AssertionError('initialization fixture did not fail')
        escaped, loader = ordinary_sink.saved[0]
        for execute in (_soac_ext.exec_module, loader.exec_module):
            try:
                execute(escaped)
            except StrictRuntimeUnavailableError:
                pass
            else:
                raise AssertionError('terminal strict module retried ordinary execution')
        assert len(ordinary_sink.saved) == 1
        """
    )
    profile = project.run(
        program,
        opt_mode="profile",
        extra_env={"SOAC_MODULE_ENABLED": f"path:{project.project}"},
    )
    work = Path(profile.args[-1]).parent / "soac-work"
    from soac import _soac_ext

    counters = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work / "profile.bin"))
    )
    strict_records = [
        record
        for record in counters["records"]
        if record["module_name"] == "strict_target"
    ]
    assert strict_records and any(
        row["function_qualname"] == "add" and row["value"] > 0
        for record in strict_records
        for row in record["rows"]
    ), counters
    assert not any(
        record["module_name"] == "ordinary_caller" for record in counters["records"]
    )
    project.run(
        program,
        opt_mode="apply",
        extra_env={
            "SOAC_MODULE_ENABLED": f"path:{project.project}",
            "SOAC_WORK_DIR": str(work),
        },
    )


def _run_ordinary_import_control(
    tmp_path, program, modules, *, mode="profile", before_hook=""
):
    """Keep the old import behavior and prove that no source was adopted."""
    work = tmp_path / "ordinary-counters"
    environment = dict(os.environ)
    environment.pop("SOAC_MODULE_ENABLED", None)
    environment.update(
        SOAC_WORK_DIR=str(work),
        SOAC_OPT_MODE=mode,
        SOAC_COMPILE_MODE="eager",
        SOAC_BACKGROUND_JIT="0",
    )
    script = (
        "import sys\n"
        + f"sys.path.insert(0, {str(tmp_path)!r})\n"
        + textwrap.dedent(before_hook)
        + "\nfrom soac import _soac_ext, import_hook\nimport_hook.install()\n"
        + _NATIVE_PREDICATES
        + textwrap.dedent(program)
        + "\n"
        + textwrap.dedent(f"""
        import importlib, types
        for name in {modules!r}:
            module = importlib.import_module(name)
            assert not isinstance(module.__spec__.loader, import_hook.SoacLoader), name
            assert _soac_ext.strict_module_diagnostics(module) is None, name
            for function in vars(module).values():
                if type(function) is types.FunctionType:
                    assert metadata(function) is None, (name, function)
                    assert strict_owner(function) is None, (name, function)
        """)
    )
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env=environment,
        timeout=60,
    )
    (tmp_path / f"{mode}.stdout.log").write_text(result.stdout)
    (tmp_path / f"{mode}.stderr.log").write_text(result.stderr)
    assert result.returncode == 0, result.stdout + result.stderr
    from soac import _soac_ext

    counter_path = work / "profile.bin"
    if counter_path.is_file():
        counters = json.loads(_soac_ext.inspect_counter_dump_json(str(counter_path)))
        assert not any(row["module_name"] in modules for row in counters["records"])
    summary = work / "jit-code-summary.jsonl"
    if summary.is_file():
        rows = [json.loads(line) for line in summary.read_text().splitlines()]
        assert not any(row.get("module_name") in modules for row in rows)


_ORDINARY_IMPORT_PROTOCOLS = {
    "typing": (
        "assert 'typing' not in sys.modules\n",
        """
        import typing
        assert typing.Callable[..., typing.Any].__args__[-1] is typing.Any
        """,
        ("typing",),
    ),
    "stdlib_edges": (
        (
            "assert 'encodings.idna' not in sys.modules\n"
            "assert 'string.templatelib' not in sys.modules\n"
        ),
        """
        import string.templatelib as templatelib
        import encodings.idna as idna
        assert templatelib.convert("value", "s") == "value"
        """,
        ("string.templatelib", "encodings.idna"),
    ),
    "shutil_rmtree": (
        "",
        """
        import os
        from pathlib import Path
        root = Path(sys.path[0]) / "to-remove"
        (root / "child").mkdir(parents=True)
        (root / "child/marker.txt").write_text("marker")
        import shutil
        shutil.rmtree(root)
        assert not os.path.exists(root)
        """,
        ("shutil",),
    ),
    "dataclasses_reload": (
        "import dataclasses\n",
        """
        import importlib
        original_loader = dataclasses.__spec__.loader
        reloaded = importlib.reload(dataclasses)
        assert reloaded is dataclasses
        assert type(reloaded.__spec__.loader) is type(original_loader)
        """,
        ("dataclasses",),
    ),
    "assert_raises_context": (
        "",
        r"""
        import ast
        import unittest
        case = unittest.TestCase()
        try:
            1 / 0
        except Exception:
            with case.assertRaises(SyntaxError) as caught:
                ast.literal_eval(r"'\U'")
            assert caught.exception.__context__ is not None
            assert type(caught.exception.__context__).__name__ == "ZeroDivisionError"
        """,
        ("ast", "unittest"),
    ),
    "runtime_bootstrap": (
        "assert 'soac.runtime' not in sys.modules\n",
        """
        import soac.runtime as runtime
        assert runtime._SOAC_RUNTIME_READY is True
        for name in ("globals", "locals", "eval", "exec"):
            try:
                getattr(runtime, name)()
            except NotImplementedError as error:
                assert "frame-sensitive globals/locals/eval/exec" in str(error)
            else:
                raise AssertionError(name)
        assert runtime.range is range
        assert runtime.range.__module__ == range.__module__
        assert runtime.range.__name__ == range.__name__
        for name in ("__init__", "__next__"):
            function = vars(runtime.IterRange)[name]
            assert metadata(function) is None
            assert strict_owner(function) is None
        """,
        ("soac.runtime",),
    ),
    "runtime_profile": (
        "",
        """
        import soac.runtime as runtime
        assert runtime._SOAC_RUNTIME_READY is True
        assert isinstance(runtime.AsyncGenComplete(), Exception)
        """,
        ("soac.runtime",),
    ),
    "runtime_profile_verify": (
        "",
        """
        import soac.runtime as runtime
        assert runtime._SOAC_RUNTIME_READY is True
        assert runtime.typing_Generic is not None
        """,
        ("soac.runtime",),
    ),
}


@pytest.mark.parametrize("case", _ORDINARY_IMPORT_PROTOCOLS)
def test_original_stdlib_import_protocols_remain_ordinary(tmp_path, case):
    before_hook, program, modules = _ORDINARY_IMPORT_PROTOCOLS[case]
    modes = ("profile", "verify") if case == "runtime_profile_verify" else ("profile",)
    for mode in modes:
        _run_ordinary_import_control(
            tmp_path, program, modules, mode=mode, before_hook=before_hook
        )


def test_ordinary_package_relative_star_still_binds_its_child(tmp_path):
    package = tmp_path / "relative_star_pkg"
    package.mkdir()
    (package / "child.py").write_text(
        '__all__ = ["EXPORTED"]\nMARKER = "child"\nEXPORTED = 3\n'
    )
    (package / "__init__.py").write_text("from .child import *\nVALUE = child.MARKER\n")
    _run_ordinary_import_control(
        tmp_path,
        """
        import relative_star_pkg as module
        assert module.VALUE == "child"
        assert module.EXPORTED == 3
        """,
        ("relative_star_pkg", "relative_star_pkg.child"),
    )


def test_path_selection_and_runtime_names_do_not_authenticate_ordinary_sources(
    tmp_path,
):
    enabled = tmp_path / "enabled"
    skipped = tmp_path / "skipped"
    enabled.mkdir()
    skipped.mkdir()
    (enabled / "enabled_probe.py").write_text("VALUE = 1\n")
    (skipped / "skipped_probe.py").write_text("VALUE = 2\n")
    _run_ordinary_import_control(
        tmp_path,
        f"""
        import importlib.util, os
        from pathlib import Path
        os.environ["SOAC_MODULE_ENABLED"] = "path:" + {str(enabled)!r}
        sys.path[:0] = [{str(enabled)!r}, {str(skipped)!r}]
        assert import_hook._should_transform({str(enabled / "enabled_probe.py")!r})
        assert not import_hook._should_transform({str(skipped / "skipped_probe.py")!r})
        import enabled_probe
        import skipped_probe
        assert (enabled_probe.VALUE, skipped_probe.VALUE) == (1, 2)
        for name in ("soac", "soac.runtime", "outside.runtime"):
            spec = importlib.util.spec_from_file_location(
                name, {str(skipped / "skipped_probe.py")!r}
            )
            original_loader = spec.loader
            assert import_hook.SoacFinder.wrap_spec(spec).loader is original_loader
        """,
        ("enabled_probe", "skipped_probe"),
    )


def test_ordinary_cross_module_reload_keeps_original_behavior(tmp_path):
    (tmp_path / "soac_helper.py").write_text("VALUE = 1\n")
    (tmp_path / "reload_probe.py").write_text(
        "import importlib\nimport soac_helper\n"
        "soac_helper = importlib.reload(soac_helper)\nVALUE = soac_helper.VALUE + 1\n"
    )
    _run_ordinary_import_control(
        tmp_path,
        """
        import soac_helper
        import reload_probe
        assert reload_probe.VALUE == 2
        assert reload_probe.soac_helper is soac_helper
        """,
        ("soac_helper", "reload_probe"),
    )


@pytest.fixture(scope="module")
def strict_import_protocols(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-import-protocols"),
        {
            "enabled/enabled_probe.py": "# soac: module(strict_assign=true, checked_attr=true)\nVALUE = 1\n",
            "skipped/skipped_probe.py": "VALUE = 2\n",
            "recursive_bg_pkg/__init__.py": "",
            "recursive_bg_pkg/child.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                VALUE = 5
                def child_value():
                    return VALUE
            """,
            "recursive_bg_pkg/parent.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                from recursive_bg_pkg import child
                VALUE = child.VALUE + 1
                def parent_value():
                    return VALUE
            """,
            "immediate_bg_pkg/__init__.py": "",
            "immediate_bg_pkg/mod.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                def identity(value):
                    return value
            """,
            "nested_function_helper.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                def outer():
                    def inner():
                        return 7
                    return inner()
            """,
            "nested_function_main.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                import nested_function_helper
                VALUE = nested_function_helper.outer()
            """,
            "reload_audit.py": "events = []\n",
            "reload_helper.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                import reload_audit
                reload_audit.events.append("executed")
                VALUE = 1
                def read():
                    return VALUE
            """,
        },
        modules={
            "enabled_probe": "enabled/enabled_probe.py",
            "recursive_bg_pkg.child": "recursive_bg_pkg/child.py",
            "recursive_bg_pkg.parent": "recursive_bg_pkg/parent.py",
            "immediate_bg_pkg.mod": "immediate_bg_pkg/mod.py",
            "nested_function_helper": "nested_function_helper.py",
            "nested_function_main": "nested_function_main.py",
            "reload_helper": "reload_helper.py",
        },
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_path_filter_admits_only_the_authenticated_selected_tree(
    strict_import_protocols, entry_interpreter
):
    project = strict_import_protocols
    enabled = project.project / "enabled"
    skipped = project.project / "skipped"
    project.run(
        f"""
        sys.path[:0] = [{str(enabled)!r}, {str(skipped)!r}]
        import enabled_probe, skipped_probe
        assert (enabled_probe.VALUE, skipped_probe.VALUE) == (1, 2)
        assert _soac_ext.strict_module_diagnostics(enabled_probe)["sealed"] is True
        assert _soac_ext.strict_module_diagnostics(skipped_probe) is None
        assert isinstance(enabled_probe.__spec__.loader, import_hook.SoacLoader)
        assert not isinstance(skipped_probe.__spec__.loader, import_hook.SoacLoader)
        """,
        entry_interpreter=entry_interpreter,
        extra_env={"SOAC_MODULE_ENABLED": f"path:{enabled}"},
    )


def test_authenticated_background_jit_does_not_recursively_schedule_imports(
    strict_import_protocols, tmp_path
):
    log = tmp_path / "background.jsonl"
    strict_import_protocols.run(
        _NATIVE_PREDICATES
        + textwrap.dedent(f"""
        import json, time
        from pathlib import Path
        from recursive_bg_pkg import parent, child
        assert parent.parent_value() == 6
        for module, function in ((parent, parent.parent_value), (child, child.child_value)):
            assert _soac_ext.strict_module_diagnostics(module)["sealed"] is True
            assert strict_owner(function) is not None
            assert metadata(function) is not None
        log = Path({str(log)!r})
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            rows = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
            if any(row.get("message") == "jit_background_module_compile_done"
                   and row.get("module_name") == "recursive_bg_pkg.parent" for row in rows):
                break
            time.sleep(0.05)
        else:
            raise AssertionError("parent background compilation did not finish")
        assert _soac_ext.strict_function_entry_kind(parent.parent_value) == "checked_native"
        time.sleep(0.25)
        """),
        opt_mode="none",
        extra_env={
            "SOAC_COMPILE_MODE": "lazy",
            "SOAC_BACKGROUND_JIT": "1",
            "SOAC_LOG": f"soac_jit_codegen=info;json={log}",
        },
        # Keep a bounded hang detector, allowing the selected debug runtime's
        # authenticated startup in addition to the original five-second wait.
        timeout=30,
    )
    rows = [json.loads(line) for line in log.read_text().splitlines()]
    completed = {
        row["module_name"]
        for row in rows
        if row.get("message") == "jit_background_module_compile_done"
    }
    assert "recursive_bg_pkg.parent" in completed
    assert "recursive_bg_pkg.child" not in completed


def test_authenticated_immediate_call_with_background_jit_does_not_deadlock(
    strict_import_protocols, tmp_path
):
    strict_import_protocols.run(
        _NATIVE_PREDICATES
        + """
from immediate_bg_pkg import mod
assert mod.identity(4) == 4
assert _soac_ext.strict_module_diagnostics(mod)["sealed"] is True
assert strict_owner(mod.identity) is not None
assert metadata(mod.identity) is not None
assert _soac_ext.strict_function_entry_kind(mod.identity) == "checked_native"
""",
        opt_mode="none",
        extra_env={
            "SOAC_COMPILE_MODE": "lazy",
            "SOAC_BACKGROUND_JIT": "1",
            "SOAC_LOG": f"soac_jit_codegen=info;json={tmp_path / 'immediate.jsonl'}",
        },
        timeout=30,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_cross_module_nested_creation_keeps_the_callee_source_owner(
    strict_import_protocols, function_create_watch_extension, entry_interpreter
):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_import_protocols.run(
        _NATIVE_PREDICATES
        + textwrap.dedent(f"""
        import importlib.util
        import nested_function_helper as helper
        spec = importlib.util.spec_from_file_location(
            "_strict_function_create_watch", {str(function_create_watch_extension)!r}
        )
        watcher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(watcher)
        events = watcher.watch(vars(helper), "inner", (), invoke=False)
        try:
            import nested_function_main as main
        finally:
            watcher.stop()
        assert main.VALUE == 7
        assert len(events) == 1 and events[0]["invoked"] is False
        inner = events[0]["function"]
        assert inner.__globals__ is vars(helper)
        assert inner.__module__ == helper.__name__
        for module in (main, helper):
            assert _soac_ext.strict_module_diagnostics(module)["sealed"] is True
        for function in (helper.outer, inner):
            assert strict_owner(function) is not None
            assert metadata(function) is not None
            assert _soac_ext.strict_function_entry_kind(function) == {expected!r}
        assert inner() == 7
        """),
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_importlib_reload_cannot_reexecute_or_unseal_a_strict_module(
    strict_import_protocols, entry_interpreter
):
    strict_import_protocols.run(
        """
        import importlib
        import reload_audit, reload_helper
        from soac.strict import StrictMutationError
        original = reload_helper
        function = original.read
        spec = original.__spec__
        assert reload_audit.events == ["executed"]
        try:
            importlib.reload(original)
        except StrictMutationError:
            # importlib first replaces __spec__. This final binding rejects
            # reload before the loader can attempt a second body execution.
            pass
        else:
            raise AssertionError("sealed module reloaded")
        assert sys.modules["reload_helper"] is original
        assert original.__spec__ is spec
        assert reload_audit.events == ["executed"]
        assert original.read is function and function() == 1
        assert _soac_ext.strict_module_diagnostics(original)["sealed"] is True
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def strict_relative_star_package(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-relative-star-package"),
        {
            "relative_star_pkg/__init__.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                from .child import *
                VALUE = child.MARKER
            """,
            "relative_star_pkg/child.py": (
                '__all__ = ["EXPORTED"]\nMARKER = "child"\nEXPORTED = 3\n'
            ),
        },
        modules={"relative_star_pkg": "relative_star_pkg/__init__.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_authenticated_package_relative_star_keeps_ordinary_child_binding(
    strict_relative_star_package, entry_interpreter
):
    strict_relative_star_package.run(
        """
        import relative_star_pkg as module
        assert module.VALUE == "child"
        assert module.EXPORTED == 3
        assert _soac_ext.strict_module_diagnostics(module)["sealed"] is True
        assert _soac_ext.strict_module_diagnostics(module.child) is None
        assert not isinstance(module.child.__spec__.loader, import_hook.SoacLoader)
        """,
        entry_interpreter=entry_interpreter,
    )


def test_strict_module_docstring_is_visible_before_body_callbacks(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "doc_plain.py": '''
                """plain docs"""
                # soac: module(strict_assign=true, checked_attr=true)
                from doc_support import observe
                observe(__name__)
                VALUE = 1
            ''',
            "doc_deferred.py": '''
                """deferred docs"""
                # soac: module(strict_assign=true, checked_attr=true)
                from doc_support import observe
                observe(__name__)
                VALUE: int = 1
            ''',
            "doc_stringized.py": '''
                """stringized docs"""
                # soac: module(strict_assign=true, checked_attr=true)
                from __future__ import annotations
                from doc_support import observe
                observe(__name__)
                VALUE: int = 1
            ''',
            "doc_support.py": """
                import sys
                events = []
                def observe(name):
                    events.append((name, sys.modules[name].__doc__))
            """,
        },
        modules={
            name: f"{name}.py"
            for name in ("doc_plain", "doc_deferred", "doc_stringized")
        },
    )
    project.run(
        """
        import doc_plain, doc_deferred, doc_stringized, doc_support
        assert doc_support.events == [
            ("doc_plain", "plain docs"),
            ("doc_deferred", "deferred docs"),
            ("doc_stringized", "stringized docs"),
        ]
        for module, document in (
            (doc_plain, "plain docs"),
            (doc_deferred, "deferred docs"),
            (doc_stringized, "stringized docs"),
        ):
            assert _soac_ext.strict_module_diagnostics(module)["sealed"] is True
            assert module.__doc__ == document and module.VALUE == 1
        """
    )


# The original import regressions are shared by the ordinary controls and
# authenticated cases. The fixture retains their original test-name mapping,
# source, validation, explicit function witnesses, and ordinary dependencies.
_REVIEWED_IMPORT_CASES = json.loads(
    (Path(__file__).parent / "fixtures/strict_import_regressions.json").read_text()
)
_REVIEWED_IMPORT_REJECTIONS = {
    "assignment_temp_gc_cycle": "CheckerError: unresolved-attribute:",
    "deleted_closure_cell": "CheckerError: not-subscriptable:",
    "empty_cell_comparison": "CheckerError: not-subscriptable:",
    "empty_closure_cell": "CheckerError: not-subscriptable:",
    "helper_scoped_to_temp_module": "CheckerError: unresolved-attribute:",
    "lone_surrogate_string_literal": "unsupported Unicode surrogate escape U+D82A",
    "missing_from_import_attr": "CheckerError: unresolved-import:",
    "mutating_closure_cell": "CheckerError: not-subscriptable:",
    "nested_class_base_local": "StrictClassMutation:",
    "nested_zero_arg_super_no_args": "CheckerError: unavailable-implicit-super-arguments:",
    "outer_try_with_body_exception": "CheckerError: invalid-argument-type:",
    "updated_positional_defaults": "CheckerError: missing-argument:",
}
_REVIEWED_IMPORT_RUNTIME_REJECTIONS = {
    "function_replaced_code": (
        "strict code execution requires an authenticated interpreter activation"
    ),
}
# This case only observes implicit function locals. Preserve its complete
# ordinary control without requiring either SOAC frame inspection or refusal.
_REVIEWED_IMPORT_ORDINARY_ONLY_CASES = {"locals_recent_assignment"}
# The original formatter case checks the exception message and handled state,
# not reconstructed SOAC frames. Keep its positive validator in every engine.
_REVIEWED_TRACEBACK_FORMATTING_CASES = ("except_handled_exception_state",)
_REVIEWED_IMPORT_POSITIVES = tuple(
    name for name in _REVIEWED_IMPORT_CASES
    if name not in _REVIEWED_IMPORT_REJECTIONS
    and name not in _REVIEWED_IMPORT_RUNTIME_REJECTIONS
    and name not in _REVIEWED_IMPORT_ORDINARY_ONLY_CASES
)
# Implicit eval still lacks the explicit namespace required by the supported
# dynamic-code protocol. Its refusal is independent of SOAC frame inspection.
# Override only selected validation; preserve the original fixture and source.
_REVIEWED_IMPORT_SELECTED_VALIDATIONS = {
    name: (
        'with pytest.raises(NotImplementedError, match="requires explicit globals"):\n'
        "    module.value()\n"
    )
    for name in ("eval_current_locals", "eval_for_loop_target_local")
}
# Ordinary execution keeps the original locals and implicit-eval results.
_REVIEWED_IMPORT_ORDINARY_VALIDATIONS = {
    "locals_recent_assignment": (
        "assert module.value() == \"{'verbose': ''} {'verbose': ''} []\"\n"
    ),
    "eval_current_locals": "assert module.value() == 7\n",
    "eval_for_loop_target_local": "assert module.value() is True\n",
}


@pytest.mark.parametrize("name", _REVIEWED_IMPORT_CASES)
def test_reviewed_import_regression_keeps_ordinary_execution(tmp_path, name):
    case = _REVIEWED_IMPORT_CASES[name]
    for relative, source in {
        f"{name}.py": case["source"],
        **case["dependencies"],
    }.items():
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
    validation = _REVIEWED_IMPORT_ORDINARY_VALIDATIONS.get(name, case["validation"])
    _run_ordinary_import_control(
        tmp_path,
        "import importlib\nimport pytest\n"
        + f"module = importlib.import_module({name!r})\n"
        + validation,
        (name,),
    )


def _selected_reviewed_import_case_modes(
    items, *, test_path, positive_test, runtime_rejection_test,
):
    """Read actual worker collection; this schedules tests, never admission."""
    test_path = Path(test_path).resolve()
    selected = set()
    for item in items:
        function = getattr(item, "obj", None)
        if function is positive_test:
            allowed = _REVIEWED_IMPORT_POSITIVES
        elif function is runtime_rejection_test:
            allowed = _REVIEWED_IMPORT_RUNTIME_REJECTIONS
        else:
            continue
        if Path(item.path).resolve() != test_path:
            continue
        params = getattr(getattr(item, "callspec", None), "params", None)
        if not isinstance(params, dict) or not {"name", "entry_interpreter"} <= params.keys():
            raise ValueError("collected reviewed import case is missing its parameters")
        name = params["name"]
        mode = params["entry_interpreter"]
        if type(name) is not str or name not in allowed:
            raise ValueError("collected reviewed import case is not in its reviewed role")
        if type(mode) is not bool:
            raise ValueError("collected reviewed import case has an unexpected mode")
        selected.add((name, mode))
    return frozenset(selected)


@pytest.fixture(scope="module")
def strict_reviewed_import_selected_case_modes(request):
    return _selected_reviewed_import_case_modes(
        request.session.items,
        test_path=Path(__file__),
        positive_test=test_reviewed_import_regressions_use_authenticated_entries,
        runtime_rejection_test=test_reviewed_import_regression_runtime_rejection_is_terminal,
    )


@pytest.fixture(scope="module")
def strict_reviewed_import_regressions(
    tmp_path_factory, strict_reviewed_import_selected_case_modes,
):
    selected_names = {name for name, _ in strict_reviewed_import_selected_case_modes}
    if not selected_names:
        raise ValueError("reviewed import project has no collected cases")
    sources = {}
    modules = {}
    for name, case in _REVIEWED_IMPORT_CASES.items():
        if name not in selected_names:
            continue
        relative = f"{name}.py"
        sources[relative] = strict_opt_in(case["source"].encode(), relative)[0].decode()
        modules[name] = relative
        for dependency, source in case["dependencies"].items():
            assert dependency not in sources
            sources[dependency] = source
    return create_strict_project(
        tmp_path_factory.mktemp("strict-reviewed-import-regressions"),
        sources,
        modules=modules,
        analysis_timeout=600,
    )


@pytest.mark.parametrize("name", _REVIEWED_IMPORT_RUNTIME_REJECTIONS)
@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_reviewed_import_regression_runtime_rejection_is_terminal(
    strict_reviewed_import_regressions, name, entry_interpreter
):
    # Unchecked initialization permits ordinary replacement code, as covered by
    # the function-boundary tests. Copying another function's strict code does
    # not transfer its source execution authority, even in the same globals.
    project = strict_reviewed_import_regressions
    project.run(
        f"""
        import importlib.util
        import pytest
        from soac.strict import StrictRuntimeUnavailableError

        name = {name!r}
        spec = importlib.util.find_spec(name)
        module = importlib.util.module_from_spec(spec)
        before = _soac_ext.strict_module_diagnostics(module)
        assert before["sealed"] is False
        assert before["artifact_generation"] == {project.publication["generation"]!r}
        sys.modules[name] = module
        try:
            with pytest.raises(StrictRuntimeUnavailableError) as caught:
                spec.loader.exec_module(module)
            assert type(caught.value) is StrictRuntimeUnavailableError
            assert str(caught.value) == {_REVIEWED_IMPORT_RUNTIME_REJECTIONS[name]!r}, str(caught.value)
            assert "VALUE" not in vars(module)
            assert module.target.__code__ is module.replacement.__code__

            # The actual failed object cannot report a live seal or publish
            # source-entry facts. Neither its donor nor its recipient can run,
            # and both extension and loader retries must remain terminal.
            with pytest.raises(StrictRuntimeUnavailableError):
                _soac_ext.strict_module_diagnostics(module)
            for function in (module.target, module.replacement):
                with pytest.raises(StrictRuntimeUnavailableError):
                    function()
                with pytest.raises(StrictRuntimeUnavailableError):
                    _soac_ext.strict_function_entry_kind(function)
                with pytest.raises(StrictRuntimeUnavailableError):
                    _soac_ext.strict_function_call_statistics(function)
            for execute in (_soac_ext.exec_module, spec.loader.exec_module):
                with pytest.raises(StrictRuntimeUnavailableError):
                    execute(module)
            assert "VALUE" not in vars(module)
        finally:
            del sys.modules[name]
        assert name not in sys.modules
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def strict_reviewed_import_results(
    strict_reviewed_import_regressions, strict_reviewed_import_selected_case_modes,
):
    results_by_mode = {}
    for entry_interpreter in (False, True):
        selected_names = tuple(
            name for name in _REVIEWED_IMPORT_POSITIVES
            if (name, entry_interpreter) in strict_reviewed_import_selected_case_modes
        )
        if not selected_names:
            continue
        # Each worker runs only its collected positive pairs. Rejection cases
        # share source preparation but keep their original isolated validator.
        cases = {
            name: StrictValidationCase(
                "import pytest\nimport sys\n"
                + "def validate_module(module):\n"
                + textwrap.indent(
                    _REVIEWED_IMPORT_SELECTED_VALIDATIONS.get(name, case["validation"])
                    + (
                        "assert module.capture(False) == (False, 'ValueError', None)\n"
                        "assert sys.exception() is None\n"
                        if name in _REVIEWED_TRACEBACK_FORMATTING_CASES else ""
                    ),
                    "    ",
                ),
                Path(__file__),
                required_functions=tuple(case["required_functions"]),
            )
            for name, case in _REVIEWED_IMPORT_CASES.items()
            if name in selected_names
        }
        results = strict_reviewed_import_regressions.run_cases(
            cases, entry_interpreter=entry_interpreter
        )
        assert set(results) == set(selected_names), "runtime did not report every requested case"
        results_by_mode[entry_interpreter] = results
    if not results_by_mode:
        raise ValueError("reviewed import results have no collected positive cases")
    return results_by_mode


@pytest.mark.parametrize("name", _REVIEWED_IMPORT_POSITIVES)
@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_reviewed_import_regressions_use_authenticated_entries(
    strict_reviewed_import_results, name, entry_interpreter
):
    error = strict_reviewed_import_results[entry_interpreter][name]
    assert error is None, f"{name} failed through the real strict import:\n{error}"


@pytest.mark.parametrize("name", _REVIEWED_TRACEBACK_FORMATTING_CASES)
def test_reviewed_traceback_formatting_preserves_original_native_behavior(tmp_path, name):
    case = _REVIEWED_IMPORT_CASES[name]
    relative = f"{name}.py"
    source = strict_opt_in(case["source"].encode(), relative)[0].decode()
    project = create_strict_project(
        tmp_path,
        {relative: source, **case["dependencies"]},
        modules={name: relative},
        backend="cpython",
    )
    project.run_case(
        name,
        "def validate_module(module):\n" + textwrap.indent(case["validation"], "    "),
        Path(__file__),
        required_functions=tuple(case["required_functions"]),
        backend="cpython",
    )


@pytest.mark.parametrize("name", _REVIEWED_IMPORT_REJECTIONS)
def test_reviewed_import_regression_rejection_does_not_publish_authority(
    tmp_path, name
):
    case = _REVIEWED_IMPORT_CASES[name]
    relative = f"{name}.py"
    source = strict_opt_in(case["source"].encode(), relative)[0].decode()
    with pytest.raises(AssertionError, match="actual checker rejected fixture"):
        create_strict_project(
            tmp_path,
            {relative: source, **case["dependencies"]},
            modules={name: relative},
        )
    diagnostic = (tmp_path / "checker.stderr.log").read_text()
    assert _REVIEWED_IMPORT_REJECTIONS[name] in diagnostic, diagnostic
    assert not (tmp_path / "authority/deployment.json").exists()



@pytest.mark.parametrize("checked_attr", [False, True])
def test_cpython_backend_module_diagnostics_authenticate_original_execution(
    tmp_path, checked_attr
):
    project = create_strict_project(
        tmp_path,
        {
            "observed.py": f"""
                # soac: module(strict_assign=true, checked_attr={str(checked_attr).lower()})
                import sys
                from soac import _soac_ext
                initializing = _soac_ext.strict_module_diagnostics(sys.modules[__name__])
                answer = 6 * 7
            """,
        },
        modules={"observed": "observed.py"},
        backend="cpython",
    )
    project.run(
        f"""
        import hashlib
        import importlib.util
        import types
        from pathlib import Path
        from soac.strict import StrictMutationError
        sys.path.insert(0, {str(Path(__file__).resolve().parents[1])!r})
        from tests._strict_integration import _assert_cpython_module_witness

        spec = importlib.util.find_spec('observed')
        observed = importlib.util.module_from_spec(spec)
        sys.modules['observed'] = observed
        before = _soac_ext.strict_module_diagnostics(observed)
        assert before['schema'] == 2
        assert before['backend'] == 'cpython'
        assert before['ready'] is False
        assert before['strict_assign'] is True
        assert before['checked_attr'] is {checked_attr!r}
        assert before['sealed'] is False
        assert before['initializer_entry_kind'] is None
        assert before['original_code_entered'] is False
        assert spec.loader.exec_module(observed) is None
        initializing = observed.initializing
        assert initializing['sealed'] is False
        assert initializing['initializer_entry_kind'] == 'original_code'
        assert initializing['original_code_entered'] is True
        source_path = {str(project.project / "observed.py")!r}
        sealed = _assert_cpython_module_witness(
            observed, module_name='observed', source_path=source_path,
            source_sha256=hashlib.sha256(Path(source_path).read_bytes()).hexdigest(),
            artifact_generation={project.publication["generation"]!r},
        )
        for key in ('startup_identity', 'interpreter_id', 'source_sha256',
                    'artifact_generation', 'source_path', 'checked_attr'):
            assert before[key] == initializing[key] == sealed[key], key
        sealed['sealed'] = False
        sealed['checked_attr'] = {not checked_attr!r}
        sealed['backend'] = 'soac'
        sealed['original_code_entered'] = False
        unchanged = _soac_ext.strict_module_diagnostics(observed)
        assert unchanged['sealed'] is True
        assert unchanged['backend'] == 'cpython'
        assert unchanged['checked_attr'] is {checked_attr!r}
        assert unchanged['original_code_entered'] is True
        fake = types.ModuleType('observed')
        fake.__dict__.update(vars(observed))
        fake.backend = 'cpython'
        fake.original_code_entered = True
        fake.sealed = True
        assert _soac_ext.strict_module_diagnostics(fake) is None
        assert observed.answer == 42
        # Strict modules are append-once namespaces, not closed namespaces.
        observed.new_binding = 1
        assert observed.new_binding == 1
        for mutation in (
            lambda: setattr(observed, 'answer', 43),
            lambda: setattr(observed, 'new_binding', 2),
            lambda: delattr(observed, 'answer'),
            lambda: delattr(observed, 'new_binding'),
        ):
            try:
                mutation()
            except StrictMutationError:
                pass
            else:
                raise AssertionError('native module seal was bypassed')
        assert observed.answer == 42 and observed.new_binding == 1
        """,
        backend="cpython",
    )


_CPYTHON_GENERATOR_ARGUMENT_SOURCE = """
from generator_argument_support import events

def checked(value: int) -> int:
    events.append(value)
    return value

def sole(values):
    return list(checked(value) for value in values)

def explicit(values):
    return list((checked(value) for value in values))

def grouped(values):
    return ((list))((checked(value)) for value in values)

def multiline(values):
    return list(  # the argument delimiter belongs to the native genexpr
        checked(élément) for élément in values  # preserve the final comment
    )

def nested(groups):
    return list(tuple(checked(value) for value in values) for values in groups)
"""


def test_cpython_backend_generator_argument_ranges_preserve_native_execution(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "generator_arguments.py": (
                "# soac: module(strict_assign=true, checked_attr=true)\n" + _CPYTHON_GENERATOR_ARGUMENT_SOURCE
            ),
            "ordinary_generator_arguments.py": _CPYTHON_GENERATOR_ARGUMENT_SOURCE,
            "generator_argument_support.py": "from typing import Any\nevents: list[Any] = []\n",
        },
        modules={"generator_arguments": "generator_arguments.py"},
        backend="cpython",
    )
    project.run_case(
        "generator_arguments",
        """
import ctypes
import generator_arguments as module
import ordinary_generator_arguments as ordinary
from generator_argument_support import events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

diagnostic = _soac_ext.strict_module_diagnostics(module)
assert _soac_ext.strict_module_diagnostics(ordinary) is None
names = ("checked", "sole", "explicit", "grouped", "multiline", "nested")
for name in names:
    function = vars(module)[name]
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed["original_code_entered"] is False
    assert _soac_ext.strict_function_diagnostics(vars(ordinary)[name]) is None

for name in ("sole", "explicit", "grouped", "multiline"):
    native = vars(module)[name]
    stock = vars(ordinary)[name]
    events.clear()
    expected = stock([2, 3, 5])
    assert expected == [2, 3, 5] and events == [2, 3, 5]
    events.clear()
    assert native([2, 3, 5]) == expected and events == [2, 3, 5]
    assert _soac_ext.strict_function_diagnostics(native)["original_code_entered"] is True

events.clear()
expected = ordinary.nested([[2, 3], [], [5]])
assert expected == [(2, 3), (), (5,)] and events == [2, 3, 5]
events.clear()
assert module.nested([[2, 3], [], [5]]) == expected
assert events == [2, 3, 5]
for _ in range(128):
    events.clear()
    assert module.sole([7, 11]) == [7, 11]
    assert events == [7, 11]

call = ctypes.pythonapi.PyObject_CallOneArg
call.argtypes = [ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
events.clear()
assert call(module.sole, [13, 17]) == [13, 17]
assert events == [13, 17]
for invoke in (module.sole, lambda values: call(module.sole, values)):
    events.clear()
    assert invoke([1, "ordinary", 3]) == [1, "ordinary", 3]
    assert events == [1, "ordinary", 3], "an annotation skipped an original body callback"
events.clear()
assert ordinary.sole([1, "ordinary", 3]) == [1, "ordinary", 3]
assert events == [1, "ordinary", 3]
assert _soac_ext.strict_function_diagnostics(module.checked)["original_code_entered"] is True
assert _soac_ext.strict_function_diagnostics(module.nested)["original_code_entered"] is True
""",
        Path(__file__),
        required_functions=("checked", "sole", "explicit", "grouped", "multiline", "nested"),
        
        backend="cpython",
    )


@pytest.fixture(scope="module")
def cpython_mandatory_artifact_project(tmp_path_factory):
    root = tmp_path_factory.mktemp("cpython-mandatory-artifact-admission")
    sentinel = root / "module-body-executed"
    project = create_strict_project(
        root,
        {
            "artifact_requested.py": f"""
                # soac: module(strict_assign=true, checked_attr=true)
                from pathlib import Path
                Path({str(sentinel)!r}).write_text("module body executed", encoding="utf-8")

                def answer() -> int:
                    return 42
            """,
            "artifact_unrequested.py": """
                # soac: module(strict_assign=true, checked_attr=true)

                def answer() -> int:
                    return 17
            """,
        },
        modules={
            "artifact_requested": "artifact_requested.py",
            "artifact_unrequested": "artifact_unrequested.py",
        },
        backend="cpython",
    )
    assert not sentinel.exists(), "the offline checker executed the analyzed module"
    return project, sentinel


@pytest.mark.parametrize(
    "damage",
    [
        "manifest-signature",
        "tampered-signed-version",
        "unsupported-deployment-version",
        "missing-unrequested-shard",
        "corrupt-unrequested-shard",
    ],
)
def test_cpython_admission_rejects_invalid_mandatory_artifacts_before_module_body(
    cpython_mandatory_artifact_project, damage
):
    project, sentinel = cpython_mandatory_artifact_project

    def assert_admitted():
        project.run_case(
            "artifact_requested",
            """
import sys

def validate_module(module):
    assert module.answer() == 42
    assert "artifact_unrequested" not in sys.modules
""",
            Path(__file__),
            required_functions=("answer",),
            
            backend="cpython",
        )
        assert sentinel.read_text() == "module body executed"
        sentinel.unlink()

    # Prove this actual ty publication admits native original-code execution
    # before damage, independently of the negative process's error text.
    assert_admitted()
    artifact = Path(project.publication["artifact_directory"])
    manifest_path = artifact / "manifest.json"
    envelope = json.loads(manifest_path.read_bytes())
    index, = [
        item for item in envelope["manifest"]["modules"]
        if item["module"]["module_name"] == "artifact_unrequested"
    ]
    shard = artifact / "modules" / f'{index["shard_digest"]}.soac-types'

    if damage == "unsupported-deployment-version":
        path = project.deployment
        descriptor = json.loads(path.read_bytes())
        descriptor["schema_version"] += 1
        replacement = json.dumps(descriptor).encode()
        diagnostic = "unsupported strict deployment schema"
    elif damage in {"manifest-signature", "tampered-signed-version"}:
        path = manifest_path

        def canonical_bytes(value):
            return json.dumps(
                value, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
            ).encode()

        # Preserve canonical encoding so rejection reaches authentication,
        # rather than succeeding merely because JSON whitespace changed.
        assert canonical_bytes(envelope) == path.read_bytes()
        if damage == "manifest-signature":
            signature = envelope["signature"]
            envelope["signature"] = (
                ("0" if signature[0] != "0" else "1") + signature[1:]
            )
        else:
            envelope["manifest"]["versions"]["schema_version"] += 1
        # The existing signature is never replaced with newly minted authority.
        # Signed version tampering must fail authentication before version use;
        # the deployment case above separately exercises a version validator.
        replacement = canonical_bytes(envelope)
        diagnostic = "manifest signature is not trusted"
    else:
        path = shard
        if damage == "missing-unrequested-shard":
            replacement = None
            diagnostic = f'read complete generation shard {index["shard_digest"]}'
        else:
            corrupted = bytearray(path.read_bytes())
            corrupted[len(corrupted) // 2] ^= 1
            replacement = bytes(corrupted)
            diagnostic = "module shard does not match its signed index: artifact_unrequested"

    original = path.read_bytes()
    mode = path.stat().st_mode & 0o7777
    try:
        if replacement is None:
            path.unlink()
        else:
            path.write_bytes(replacement)
        # Native startup captures this real deployment afresh. The first
        # selected import must verify *every* mandatory shard before its body,
        # even though artifact_unrequested is never imported by the program.
        result = project.run("import artifact_requested\n", check=False, backend="cpython")
        assert result.returncode == 1, (result.returncode, result.stdout, result.stderr)
        assert "StrictRuntimeUnavailableError:" in result.stderr, result.stderr
        assert "strict startup deployment " in result.stderr, result.stderr
        assert diagnostic in result.stderr, result.stderr
        assert not sentinel.exists(), "invalid authority reached the selected module body"
    finally:
        path.write_bytes(original)
        path.chmod(mode)
        sentinel.unlink(missing_ok=True)

    # Restore exact published bytes, not a re-signed or re-analyzed replacement.
    assert path.read_bytes() == original
    assert_admitted()
