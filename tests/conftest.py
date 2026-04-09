from __future__ import annotations

import os
import sys
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

from tests import _integration
import pytest

_MODULES_DIR = Path(__file__).resolve().parent / "integration_modules"


def _print_integration_failure_context(module_path: Path) -> None:
    try:
        source = module_path.read_text(encoding="utf-8")
    except OSError as err:
        source = f"<<failed to read source: {err}>>"

    try:
        transformed = _integration.render_transformed_source(module_path)
    except Exception as err:
        transformed = f"<<failed to transform source: {err}>>"

    print("\n--- diet-python integration failure context ---", file=sys.stderr)
    print(f"module: {module_path}", file=sys.stderr)
    print("--- input module ---", file=sys.stderr)
    print(source, file=sys.stderr)
    print("--- transformed module ---", file=sys.stderr)
    print(transformed, file=sys.stderr)
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
            tmp_path, module_name, source, mode="transform"
        ) as module:
            yield module
    except Exception:
        _print_integration_failure_context(module_path)
        raise


def pytest_configure(config):
    config.addinivalue_line("markers", "integration: mark a test as using integration modules")


def _is_unsupported_exception(exc: BaseException, longrepr_text: str | None = None) -> bool:
    if isinstance(exc, NotImplementedError):
        return "not supported" in str(exc)
    if isinstance(exc, TypeError):
        return str(exc) == "An asyncio.Future, a coroutine or an awaitable is required"
    if longrepr_text and "not supported" in longrepr_text:
        return True
    return False


@pytest.fixture
def run_integration_module(tmp_path: Path):
    @contextmanager
    def _runner(module_name: str) -> Iterator[ModuleType]:
        with _load_integration_module(tmp_path, module_name) as module:
            yield module

    return _runner


@pytest.hookimpl(hookwrapper=True, tryfirst=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    rep = outcome.get_result()
    setattr(item, f"rep_{rep.when}", rep)
    if rep.failed:
        longrepr_text = str(rep.longrepr) if rep.longrepr is not None else None
        exc = call.excinfo.value if call.excinfo is not None else None
        if (exc and _is_unsupported_exception(exc, longrepr_text)) or (
            longrepr_text and "not supported" in longrepr_text
        ):
            rep.outcome = "skipped"
            rep.wasxfail = f"unsupported: {exc or 'not supported'}"
            rep.longrepr = None

@pytest.fixture(autouse=True)
def _integration_failure_context(request):
    _integration.clear_integration_modules()
    yield
    rep = getattr(request.node, "rep_call", None)
    if rep is not None and rep.failed:
        _integration.print_integration_failure_contexts()
