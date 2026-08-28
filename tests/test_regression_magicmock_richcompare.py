from __future__ import annotations

from pathlib import Path

import pytest

from tests._integration import exec_integration_validation, stock_module
from tests._strict_integration import create_strict_project


@pytest.mark.parametrize("mode", ["stock", "cpython", "soac", "entry"])
def test_magicmock_richcompare_uses_bound_special_method(tmp_path: Path, mode: str) -> None:
    source = """
from unittest import mock


def run():
    value = mock.MagicMock()
    return value == 1, value != 1
"""
    validate_source = """
import pytest

def validate_module(module):
    assert module.run() == (False, True)
"""
    module_name = 'magicmock_richcompare'
    if mode == "stock":
        with stock_module(tmp_path, module_name, source) as module:
            exec_integration_validation(validate_source, module, Path(__file__), mode="stock")
        return
    project = create_strict_project(
        tmp_path,
        {f"{module_name}.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + source},
        modules={module_name: f"{module_name}.py"},
        backend="cpython" if mode == "cpython" else "soac",
    )
    project.run_case(
        module_name, validate_source, Path(__file__),
        required_functions=('run',), 
        entry_interpreter=mode == "entry",
    )
