from __future__ import annotations

from pathlib import Path

import pytest

from tests._integration import exec_integration_validation, stock_module
from tests._strict_integration import create_strict_project


@pytest.mark.parametrize("mode", ["stock", "cpython", "soac", "entry"])
def test_basic_block_lowering_while_break_continue_else(tmp_path: Path, mode: str) -> None:
    source = """
def run(limit):
    i = 0
    out = []
    while i < limit:
        i = i + 1
        if i == 2:
            continue
        if i == 5:
            break
        out.append(i)
    else:
        out.append(99)
    return out, i
"""
    validate_source = """
import pytest

def validate_module(module):
    assert module.run(3) == ([1, 3, 99], 3)
    assert module.run(10) == ([1, 3, 4], 5)
"""
    module_name = 'basic_blocks_while'
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
        required_functions=('run',), 
        entry_interpreter=mode == "entry",
    )
