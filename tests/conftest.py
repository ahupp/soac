from __future__ import annotations

import os
import shlex
import subprocess
import sys
import sysconfig
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from types import ModuleType

os.environ.setdefault("DIET_PYTHON_VALIDATE_MIN_AST", "1")

ROOT = Path(__file__).resolve().parent.parent
PYTHON_SRC = ROOT / "soac_py" / "src"
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
if str(PYTHON_SRC) not in sys.path:
    sys.path.insert(0, str(PYTHON_SRC))

import pytest

from tests import _integration

_MODULES_DIR = Path(__file__).resolve().parent / "integration_modules"
_TRUE_ENV_VALUES = {"1", "true", "yes", "on"}


@pytest.fixture(scope="session")
def function_create_watch_extension(tmp_path_factory) -> Path:
    """Build the preserving C watcher once against this selected guest Python."""
    source = ROOT / "tests" / "native" / "function_create_watch.c"
    configured_source = sysconfig.get_config_var("abs_srcdir")
    configured_build = sysconfig.get_config_var("abs_builddir")
    linker = sysconfig.get_config_var("LDSHARED")
    shared_flags = sysconfig.get_config_var("CCSHARED")
    suffix = sysconfig.get_config_var("EXT_SUFFIX")
    assert all((configured_source, configured_build, linker, shared_flags, suffix)), (
        "CREATE fixture needs the selected native build's sysconfig, not host Python"
    )
    native_source = Path(configured_source).resolve(strict=True)
    native_build = Path(configured_build).resolve(strict=True)
    assert (native_build / "python").resolve(strict=True) == Path(
        sys._base_executable
    ).resolve(strict=True), "CREATE fixture sysconfig does not name the running native build"
    for header in (
        native_source / "Include" / "Python.h",
        native_source / "Include" / "cpython" / "funcobject.h",
        native_build / "pyconfig.h",
    ):
        assert header.is_file(), f"CREATE fixture selected native header is absent: {header}"
    output = tmp_path_factory.mktemp("function-create-watch-native")
    extension = output / f"_strict_function_create_watch{suffix}"
    command = [
        *shlex.split(linker),
        *shlex.split(shared_flags),
        "-O0",
        "-g",
        "-Wall",
        "-Wextra",
        "-Werror",
        f"-I{native_source / 'Include'}",
        f"-I{native_build}",
        str(source),
        "-o",
        str(extension),
    ]
    result = subprocess.run(
        command, capture_output=True, text=True, check=False, timeout=120
    )
    log = output / "build.log"
    log.write_text(shlex.join(command) + "\n" + result.stdout + result.stderr)
    assert result.returncode == 0, f"CREATE fixture build failed: {log}\n{result.stderr}"
    assert extension.is_file(), f"CREATE fixture compiler produced no extension: {log}"
    return extension


def _print_integration_failure_context(module_path: Path) -> None:
    try:
        source = module_path.read_text(encoding="utf-8")
    except OSError as err:
        source = f"<<failed to read source: {err}>>"

    print("\n--- diet-python integration failure context ---", file=sys.stderr)
    print(f"module: {module_path}", file=sys.stderr)
    print("--- input module ---", file=sys.stderr)
    print(source, file=sys.stderr)
    print("--- end diet-python integration context ---", file=sys.stderr)


@contextmanager
def _load_integration_module(tmp_path: Path, module_name: str) -> Iterator[ModuleType]:
    module_path = _MODULES_DIR / f"{module_name}.py"
    if not module_path.exists():
        raise FileNotFoundError(
            f"Integration module '{module_name}' not found at {module_path}"
        )
    source, _ = _integration.split_integration_case(module_path)
    try:
        with _integration.integration_module(
            tmp_path, module_name, source, mode="soac"
        ) as module:
            yield module
    except Exception:
        _print_integration_failure_context(module_path)
        raise


def pytest_addoption(parser):
    parser.addoption(
        "--run-slow",
        action="store_true",
        default=False,
        help="include tests marked slow; normal correctness runs deselect them",
    )


def _slow_tests_enabled(config) -> bool:
    if config.getoption("--run-slow"):
        return True
    if getattr(config.option, "markexpr", ""):
        return True
    return os.environ.get("SOAC_RUN_SLOW_TESTS", "").lower() in _TRUE_ENV_VALUES


def pytest_collection_modifyitems(config, items):
    if _slow_tests_enabled(config):
        return

    selected = []
    deselected = []
    for item in items:
        if item.get_closest_marker("slow") is None:
            selected.append(item)
        else:
            deselected.append(item)

    if deselected:
        config.hook.pytest_deselected(items=deselected)
        items[:] = selected


@pytest.fixture
def run_integration_module(tmp_path: Path):
    @contextmanager
    def _runner(module_name: str) -> Iterator[ModuleType]:
        with _load_integration_module(tmp_path, module_name) as module:
            yield module

    return _runner


@pytest.fixture(autouse=True)
def _restore_import_hook_state():
    prior_meta_path = list(sys.meta_path)
    yield
    sys.meta_path[:] = prior_meta_path


@pytest.hookimpl(hookwrapper=True, tryfirst=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    rep = outcome.get_result()
    setattr(item, f"rep_{rep.when}", rep)


@pytest.fixture(autouse=True)
def _integration_failure_context(request):
    _integration.clear_integration_modules()
    yield
    rep = getattr(request.node, "rep_call", None)
    if rep is not None and rep.failed:
        _integration.print_integration_failure_contexts()
