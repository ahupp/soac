from __future__ import annotations

from pathlib import Path

import pytest

from tests._integration import integration_module


@pytest.mark.parametrize("mode", ["soac"], ids=["soac"])
def test_asyncgen_anext_send_non_none_raises_type_error(tmp_path: Path, mode: str) -> None:
    source = """
def make_anext():
    async def gen():
        yield 123

    return gen().__anext__()
"""
    with integration_module(tmp_path, "asyncgen_anext_send_non_none", source, mode=mode) as module:
        anext_obj = module.make_anext()
        with pytest.raises(TypeError, match=r"non-None value .* async generator"):
            anext_obj.send(100)
