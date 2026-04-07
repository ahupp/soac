from __future__ import annotations

import argparse
import importlib.machinery
import importlib.util
import os
import sys
import tempfile
from pathlib import Path

from . import _soac_ext


REPO_ROOT = Path(__file__).resolve().parents[3]


def _integration_only_enabled() -> bool:
    # Read dynamically so tests can toggle this per import context.
    return os.environ.get("DIET_PYTHON_INTEGRATION_ONLY") == "1"


def _runtime_bootstrap_in_progress() -> bool:
    runtime = sys.modules.get("soac.runtime")
    return runtime is not None and not getattr(runtime, "_SOAC_RUNTIME_READY", False)


def _create_module_from_path(path: str, spec):
    absolute_path = os.path.abspath(path)
    try:
        return _soac_ext.create_module(absolute_path, spec)
    except SyntaxError as err:
        if err.filename is None:
            err.filename = absolute_path
        raise
    except Exception as err:
        raise ImportError(f"diet-python failed for {absolute_path}: {err}") from err


def _is_integration_module(resolved: Path) -> bool:
    try:
        resolved.relative_to(REPO_ROOT / "tests" / "integration_modules")
        return True
    except (OSError, ValueError):
        pass
    for parent in resolved.parents:
        if parent.name.startswith("_dp_integration_"):
            return True
    return False


def _should_transform(path: str) -> bool:
    """Return ``True`` if ``path`` should be passed through the transform."""
    if _runtime_bootstrap_in_progress():
        return False
    if path.endswith(os.path.join("encodings", "__init__.py")):
        return False
    try:
        resolved = Path(path).resolve()
    except OSError:
        return False
    if resolved.name == "__init__.py" and resolved.parent.name == "encodings":
        return False
    if resolved.name == "templatelib.py" and resolved.parent.name == "string":
        return False
    if _integration_only_enabled() and not _is_integration_module(resolved):
        return False
    if os.environ.get("DIET_PYTHON_ALLOW_TEMP") != "1":
        try:
            resolved.relative_to(Path(tempfile.gettempdir()).resolve())
        except (OSError, ValueError):
            pass
        else:
            return False
    try:
        with open(path, "rb") as file:
            return b"diet-python: disable" not in file.read()
    except OSError:
        return False


def _source_path_for_frozen_spec(spec):
    loader_state = getattr(spec, "loader_state", None)
    filename = getattr(loader_state, "filename", None)
    if filename:
        return filename

    origname = getattr(loader_state, "origname", spec.name)
    candidates = [
        REPO_ROOT / "vendor" / "cpython" / "Lib" / f"{origname.replace('.', os.sep)}.py",
        REPO_ROOT
        / "vendor"
        / "cpython"
        / "Lib"
        / origname.replace(".", os.sep)
        / "__init__.py",
    ]
    for candidate in candidates:
        if candidate.is_file():
            return str(candidate)
    return None


class DietPythonLoader(importlib.machinery.SourceFileLoader):
    """Loader that applies the diet-python transform before executing a module."""

    def create_module(self, spec):
        return _create_module_from_path(self.path, spec)

    def exec_module(self, module):
        _soac_ext.exec_module(module)
        return None


class DietPythonFinder(importlib.machinery.PathFinder):
    """Finder that wraps loaders to apply diet-python transformations."""

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        spec = importlib.machinery.FrozenImporter.find_spec(fullname, path, target)
        if spec is None:
            spec = super().find_spec(fullname, path, target)
        return cls.wrap_spec(spec)

    @classmethod
    def wrap_spec(cls, spec):
        if spec is None:
            return None
        fullname = spec.name
        if (
            fullname != "encodings"
            and not fullname.startswith("encodings.")
            and isinstance(spec.loader, importlib.machinery.SourceFileLoader)
            and spec.origin
            and _should_transform(spec.origin)
        ):
            spec.loader = DietPythonLoader(fullname, spec.origin)
        elif (
            fullname != "encodings"
            and not fullname.startswith("encodings.")
            and spec.loader is importlib.machinery.FrozenImporter
            and spec.origin == "frozen"
        ):
            source_path = _source_path_for_frozen_spec(spec)
            if source_path and _should_transform(source_path):
                spec.origin = source_path
                spec.has_location = True
                spec.loader = DietPythonLoader(fullname, source_path)
        return spec


def install():
    """Install the diet-python import hook."""
    existing_typing = sys.modules.get("typing")
    if existing_typing is not None:
        loader = getattr(getattr(existing_typing, "__spec__", None), "loader", None)
        if not isinstance(loader, DietPythonLoader):
            sys.modules.pop("typing", None)

    if any(finder is DietPythonFinder for finder in sys.meta_path):
        return

    for index, finder in enumerate(sys.meta_path):
        if finder in {
            importlib.machinery.FrozenImporter,
            importlib.machinery.PathFinder,
        }:
            sys.meta_path.insert(index, DietPythonFinder)
            break
    else:
        sys.meta_path.insert(0, DietPythonFinder)


def _resolve_target(target: str) -> importlib.machinery.ModuleSpec:
    if os.sep in target or target.endswith(".py"):
        path = Path(target)
        if not path.is_file():
            raise SystemExit(f"soac.import_hook: file not found: {target}")
        path = path.resolve()
        module_name = "__main__"
        if path.name == "__init__.py":
            spec = importlib.util.spec_from_file_location(
                module_name,
                path,
                submodule_search_locations=[str(path.parent)],
            )
        else:
            spec = importlib.util.spec_from_file_location(module_name, path)
        if spec is None or spec.loader is None or spec.origin is None:
            raise SystemExit(f"soac.import_hook: could not resolve spec for file: {target}")
        return spec

    spec = importlib.util.find_spec(target)
    if spec is None or spec.loader is None or spec.origin is None:
        raise SystemExit(f"soac.import_hook: module not found: {target}")
    if spec.origin in {"built-in", "frozen"}:
        raise SystemExit(f"soac.import_hook: cannot execute built-in module: {target}")
    return spec


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Execute a module via transformed source + module init"
    )
    parser.add_argument("module", help="Module name or path to a .py file")
    parser.add_argument("args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)

    spec = DietPythonFinder.wrap_spec(_resolve_target(args.module))
    assert spec is not None
    path = Path(spec.origin).resolve()

    install()
    sys.argv = [str(path), *args.args]
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    sys.argv[0] = str(path)
    assert spec.loader is not None
    spec.loader.exec_module(module)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
