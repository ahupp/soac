from __future__ import annotations

import argparse
import builtins
import importlib.machinery
import importlib.util
import os
import sys
from pathlib import Path

from . import _soac_ext


REPO_ROOT = Path(__file__).resolve().parents[3]


def _enabled_module_roots() -> list[Path] | None:
    raw_spec = os.environ.get("SOAC_MODULE_ENABLED")
    if raw_spec is None:
        return None

    roots: list[Path] = []
    for raw_entry in raw_spec.split(","):
        entry = raw_entry.strip()
        if not entry:
            continue
        kind, separator, value = entry.partition(":")
        if kind != "path" or separator != ":" or not value:
            raise ValueError(
                "SOAC_MODULE_ENABLED entries must be path:<file-or-directory>"
            )
        roots.append(Path(value).expanduser().resolve())
    return roots


def _create_module_from_path(path: str, spec):
    absolute_path = os.path.abspath(path)
    try:
        return _soac_ext.create_module(absolute_path, spec)
    except SyntaxError as err:
        if err.filename is None:
            err.filename = absolute_path
        raise
    except _soac_ext.StrictRuntimeUnavailableError:
        # The native ImportError subclass identifies missing/stale strict
        # authority. Preserve it instead of replacing it with a generic error.
        raise
    except Exception as err:
        raise ImportError(f"diet-python failed for {absolute_path}: {err}") from err


def _module_is_enabled(resolved: Path) -> bool:
    roots = _enabled_module_roots()
    if roots is None:
        return True
    return any(resolved.is_relative_to(root) for root in roots)


def _should_transform(path: str) -> bool:
    """Return whether to ask the native loader about this source path."""
    try:
        resolved = Path(path).resolve()
    except OSError:
        return False
    if not _module_is_enabled(resolved):
        return False
    return True


def _source_path_for_frozen_spec(spec):
    loader_state = getattr(spec, "loader_state", None)
    filename = getattr(loader_state, "filename", None)
    if filename:
        return filename

    origname = getattr(loader_state, "origname", spec.name)
    source_dir = REPO_ROOT / os.environ.get("CPYTHON_SOURCE_DIR", "vendor/cpython")
    candidates = [
        source_dir / "Lib" / f"{origname.replace('.', os.sep)}.py",
        source_dir / "Lib" / origname.replace(".", os.sep) / "__init__.py",
    ]
    for candidate in candidates:
        if candidate.is_file():
            return str(candidate)
    return None


class SoacLoader(importlib.machinery.SourceFileLoader):
    """Authenticate strict imports and leave ordinary imports on their loader."""

    def __init__(self, fullname, path, native_loader=None):
        super().__init__(fullname, path)
        self._native_loader = (
            importlib.machinery.SourceFileLoader(fullname, path)
            if native_loader is None
            else native_loader
        )

    def create_module(self, spec):
        frozen = self._native_loader is importlib.machinery.FrozenImporter
        origin, has_location = spec.origin, spec.has_location
        if frozen:
            # An authenticated strict replacement executes its verified source.
            # A declined ordinary import must retain FrozenImporter's code and
            # metadata, not execute the corresponding Lib source instead.
            spec.origin, spec.has_location = self.path, True
        try:
            module = _create_module_from_path(self.path, spec)
        except BaseException:
            if frozen:
                spec.origin, spec.has_location = origin, has_location
            raise
        if module is None:
            if frozen:
                spec.origin, spec.has_location = origin, has_location
            spec.loader = self._native_loader
            return self._native_loader.create_module(spec)
        return module

    def exec_module(self, module):
        # Only native module-definition identity selects this branch. An
        # exception from an owned (including terminal) strict module never
        # retries ordinary source execution.
        if not _soac_ext.exec_module(module):
            self._native_loader.exec_module(module)
        return None


class SoacFinder(importlib.machinery.PathFinder):
    """Finder that wraps loaders to apply SOAC transformations."""

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        spec = importlib.machinery.FrozenImporter.find_spec(fullname, path, target)
        if spec is None:
            spec = super().find_spec(fullname, path, target)
        return cls.wrap_spec(spec, target)

    @classmethod
    def wrap_spec(cls, spec, target=None):
        if spec is None:
            return None
        if target is not None:
            loader = getattr(getattr(target, "__spec__", None), "loader", None)
            if not isinstance(loader, SoacLoader):
                return spec
        fullname = spec.name
        if (
            isinstance(spec.loader, importlib.machinery.SourceFileLoader)
            and spec.origin
            and _should_transform(spec.origin)
        ):
            spec.loader = SoacLoader(fullname, spec.origin, spec.loader)
        elif (
            spec.loader is importlib.machinery.FrozenImporter
            and spec.origin == "frozen"
        ):
            source_path = _source_path_for_frozen_spec(spec)
            if source_path and _should_transform(source_path):
                spec.loader = SoacLoader(fullname, source_path, spec.loader)
        return spec


def install(*, backend=None):
    """Select an immutable native execution backend and install the import hook.

    Omitting the backend preserves an existing selection, or selects SOAC on
    first use. This choice never grants strict source/deployment authority.
    """
    _soac_ext.configure_strict_backend(backend)
    if any(finder is SoacFinder for finder in sys.meta_path):
        return

    for index, finder in enumerate(sys.meta_path):
        if finder in {
            importlib.machinery.FrozenImporter,
            importlib.machinery.PathFinder,
        }:
            sys.meta_path.insert(index, SoacFinder)
            break
    else:
        sys.meta_path.insert(0, SoacFinder)


def _target_is_path(target: str) -> bool:
    return os.sep in target or target.endswith(".py")


def _resolve_target(target: str) -> importlib.machinery.ModuleSpec:
    if _target_is_path(target):
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

    target_is_path = _target_is_path(args.module)
    spec = SoacFinder.wrap_spec(_resolve_target(args.module))
    assert spec is not None
    path = Path(spec.origin).resolve()

    install()
    if target_is_path:
        script_dir = str(path.parent)
        if sys.path:
            sys.path[0] = script_dir
        else:
            sys.path.insert(0, script_dir)
    sys.argv = [str(path), *args.args]
    module = importlib.util.module_from_spec(spec)
    if spec.name == "__main__" and target_is_path:
        # Match `python path.py`: multiprocessing uses a missing __spec__ to
        # rediscover the main module by path in forkserver/spawn workers.
        module.__spec__ = None
        module.__dict__["__builtins__"] = builtins
    sys.modules[spec.name] = module
    sys.argv[0] = str(path)
    assert spec.loader is not None
    spec.loader.exec_module(module)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
