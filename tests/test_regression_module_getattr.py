from __future__ import annotations

from pathlib import Path

import pytest

from tests._integration import exec_integration_validation, stock_module
from tests._strict_integration import create_strict_project


@pytest.mark.parametrize("mode", ["stock", "cpython", "soac", "entry"])
def test_module_getattr_lazy_attribute(tmp_path: Path, mode: str) -> None:
    source = """
value = 41

def __getattr__(name):
    if name == "lazy":
        return value + 1
    raise AttributeError(name)
"""
    validate_source = """
import pytest

def validate_module(module):
    assert module.value == 41
    assert module.lazy == 42
"""
    module_name = 'module_getattr_lazy'
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
        required_functions=('__getattr__',), 
        entry_interpreter=mode == "entry",
    )
