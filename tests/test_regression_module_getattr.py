from __future__ import annotations

from pathlib import Path

import pytest

from tests._integration import integration_module


@pytest.mark.parametrize(
    "mode",
    ["stock", "soac"],
    ids=["stock", "soac"],
)
def test_module_getattr_lazy_attribute(tmp_path: Path, mode: str) -> None:
    source = """
value = 41

def __getattr__(name):
    if name == "lazy":
        return value + 1
    raise AttributeError(name)
"""
    with integration_module(tmp_path, "module_getattr_lazy", source, mode=mode) as module:
        assert module.value == 41
        assert module.lazy == 42
