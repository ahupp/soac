from __future__ import annotations

import os
import importlib
import sys
from contextlib import contextmanager
from uuid import uuid4
from pathlib import Path
from types import ModuleType
from typing import Iterator

ROOT = Path(__file__).resolve().parent.parent
PYTHON_SRC = ROOT / "soac_py" / "src"
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
if str(PYTHON_SRC) not in sys.path:
    sys.path.insert(0, str(PYTHON_SRC))

from soac import _soac_ext, import_hook

_VALIDATE_DELIMITER = "# diet-python: validate"

_REGISTERED_MODULES: list[Path] = []
_PRINTED_MODULES: set[Path] = set()


def register_integration_module(module_path: Path) -> None:
    _REGISTERED_MODULES.append(module_path)


def mark_integration_module_printed(module_path: Path) -> None:
    _PRINTED_MODULES.add(module_path)


def clear_integration_modules() -> None:
    _REGISTERED_MODULES.clear()
    _PRINTED_MODULES.clear()


def print_integration_failure_contexts() -> None:
    seen: set[Path] = set()
    for module_path in _REGISTERED_MODULES:
        if module_path in seen or module_path in _PRINTED_MODULES:
            continue
        seen.add(module_path)
        _print_integration_failure_context(module_path)
        _PRINTED_MODULES.add(module_path)


def split_integration_case(module_path: Path) -> tuple[str, str]:
    source = module_path.read_text(encoding="utf-8")
    if _VALIDATE_DELIMITER not in source:
        raise ValueError(f"missing integration validate delimiter in {module_path}")
    raw_source, raw_validate = source.split(_VALIDATE_DELIMITER, 1)
    line_offset = raw_source.count("\n")
    padded_validate = "\n" * line_offset + raw_validate
    return raw_source.rstrip() + "\n", padded_validate


@contextmanager
def _force_entry_interpreter_for_tests(enabled: bool) -> Iterator[None]:
    previous = _soac_ext.force_entry_interpreter_for_tests(enabled)
    try:
        yield
    finally:
        _soac_ext.force_entry_interpreter_for_tests(previous)


def _print_integration_failure_context(module_path: Path, mode: str | None = None) -> None:
    if module_path in _PRINTED_MODULES:
        return
    try:
        source = module_path.read_text(encoding="utf-8")
    except OSError as err:
        source = f"<<failed to read source: {err}>>"

    print("\n--- diet-python integration failure context ---", file=sys.stderr)
    print(f"module: {module_path}", file=sys.stderr)
    if mode is not None:
        print(f"mode: {mode}", file=sys.stderr)
    print("--- input module ---", file=sys.stderr)
    print(source, file=sys.stderr)
    print("--- end diet-python integration context ---", file=sys.stderr)
    _PRINTED_MODULES.add(module_path)


@contextmanager
def _disable_import_hook() -> Iterator[None]:
    removed_indexes: list[int] = []
    for index in range(len(sys.meta_path) - 1, -1, -1):
        if sys.meta_path[index] is import_hook.SoacFinder:
            removed_indexes.append(index)
            sys.meta_path.pop(index)
    try:
        yield
    finally:
        for index in reversed(removed_indexes):
            sys.meta_path.insert(index, import_hook.SoacFinder)


@contextmanager
def _load_module(
    tmp_path: Path, module_name: str, source: str, *, mode: str
) -> Iterator[ModuleType]:
    package_name = f"_dp_integration_{uuid4().hex}"
    package_dir = tmp_path / package_name
    package_dir.mkdir(parents=True, exist_ok=True)
    (package_dir / "__init__.py").write_text("", encoding="utf-8")

    module_path = package_dir / f"{module_name}.py"
    module_path.write_text(source, encoding="utf-8")
    register_integration_module(module_path)

    package_root = str(tmp_path)
    sys.path.insert(0, package_root)
    prior_mode = os.environ.get("DIET_PYTHON_MODE")
    prior_enabled_modules = os.environ.get("SOAC_MODULE_ENABLED")

    full_name = f"{package_name}.{module_name}"

    try:
        if mode in {"soac", "entry"}:
            os.environ["DIET_PYTHON_MODE"] = "transform"
            if prior_enabled_modules is None:
                os.environ["SOAC_MODULE_ENABLED"] = f"path:{package_dir}"
            import_hook.install()
            sys.modules.pop(full_name, None)
            sys.modules.pop(package_name, None)
            with _force_entry_interpreter_for_tests(mode == "entry"):
                module = importlib.import_module(full_name)
                yield module
        elif mode == "stock":
            os.environ.pop("DIET_PYTHON_MODE", None)
            with _disable_import_hook():
                sys.modules.pop(full_name, None)
                sys.modules.pop(package_name, None)
                module = importlib.import_module(full_name)
                yield module
        else:
            raise ValueError(f"unsupported integration mode: {mode!r}")
    except Exception:
        _print_integration_failure_context(module_path, mode=mode)
        raise
    finally:
        sys.modules.pop(full_name, None)
        sys.modules.pop(package_name, None)
        if sys.path and sys.path[0] == package_root:
            sys.path.pop(0)
        else:
            try:
                sys.path.remove(package_root)
            except ValueError:
                pass
        if prior_mode is None:
            os.environ.pop("DIET_PYTHON_MODE", None)
        else:
            os.environ["DIET_PYTHON_MODE"] = prior_mode
        if prior_enabled_modules is None:
            os.environ.pop("SOAC_MODULE_ENABLED", None)
        else:
            os.environ["SOAC_MODULE_ENABLED"] = prior_enabled_modules


@contextmanager
def soac_module(
    tmp_path: Path, module_name: str, source: str
) -> Iterator[ModuleType]:
    with _load_module(tmp_path, module_name, source, mode="soac") as module:
        yield module


@contextmanager
def stock_module(
    tmp_path: Path, module_name: str, source: str
) -> Iterator[ModuleType]:
    with _load_module(tmp_path, module_name, source, mode="stock") as module:
        yield module


def exec_integration_validation(
    validate_source: str, module: ModuleType, module_path: Path, *, mode: str
) -> None:
    module.__dict__["__dp_integration_soac__"] = mode in {"soac", "entry"}
    module.__dict__["__dp_integration_entry__"] = mode == "entry"
    module.__dict__["__dp_integration_mode__"] = mode
    globals_dict = dict(module.__dict__)
    exec(compile(validate_source, str(module_path), "exec"), globals_dict)


@contextmanager
def integration_module(
    tmp_path: Path, module_name: str, source: str, *, mode: str
) -> Iterator[ModuleType]:
    with _load_module(tmp_path, module_name, source, mode=mode) as module:
        yield module


def decide_optimizations_for_work_dir(work_dir: Path) -> int:
    counters_path = work_dir / "profile.bin"
    module_root = work_dir / "modules"
    return _soac_ext.decide_optimizations_for_counter_dump(
        str(counters_path),
        str(module_root),
        str(module_root),
    )
