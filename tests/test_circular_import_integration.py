from __future__ import annotations

import importlib
import sys
from pathlib import Path

import pytest

from soac import import_hook


def _write(path: Path, source: str) -> None:
    path.write_text(source, encoding="utf-8")


def test_circular_relative_import(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    pkg = tmp_path / "pkg"
    pkg.mkdir()

    _write(pkg / "__init__.py", "from . import a\n")
    _write(pkg / "a.py", "from . import b\n")
    _write(pkg / "b.py", "from . import a\n")

    monkeypatch.setenv("SOAC_MODULE_ENABLED", f"path:{pkg}")
    sys.path.insert(0, str(tmp_path))
    import_hook.install()

    try:
        module = importlib.import_module("pkg.a")
        assert hasattr(module, "b")
    finally:
        for name in ["pkg", "pkg.a", "pkg.b"]:
            sys.modules.pop(name, None)
        sys.path.remove(str(tmp_path))
