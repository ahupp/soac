from __future__ import annotations

from pathlib import Path

import pytest

from tests._integration import exec_integration_validation, stock_module
from tests._strict_integration import create_strict_project


@pytest.mark.parametrize("mode", ["stock", "cpython", "soac", "entry"])
def test_asyncgen_anext_send_non_none_raises_type_error(tmp_path: Path, mode: str) -> None:
    source = """
def make_anext():
    async def gen():
        yield 123

    return gen().__anext__()
"""
    validate_source = """
import pytest

def validate_module(module):
    anext_obj = module.make_anext()
    with pytest.raises(TypeError, match=r"non-None value .* async generator"):
        anext_obj.send(100)
"""
    module_name = 'asyncgen_anext_send_non_none'
    if mode == "stock":
        with stock_module(tmp_path, module_name, source) as module:
            exec_integration_validation(validate_source, module, Path(__file__), mode="stock")
        return
    project = create_strict_project(
        tmp_path,
        {f"{module_name}.py": "from __future__ import strict\n" + source},
        modules={module_name: f"{module_name}.py"},
        backend="cpython" if mode == "cpython" else "soac",
    )
    project.run_case(
        module_name, validate_source, Path(__file__),
        required_functions=('make_anext',), 
        entry_interpreter=mode == "entry",
    )
