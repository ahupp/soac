from __future__ import annotations

from pathlib import Path

import pytest

from tests._integration import exec_integration_validation, stock_module
from tests._strict_integration import create_strict_project


@pytest.mark.parametrize("mode", ["stock", "cpython", "soac", "entry"])
def test_special_methods_bind_descriptor_before_call(tmp_path: Path, mode: str) -> None:
    source = """
class BindingDescriptor:
    def __init__(self, label):
        self.label = label

    def __get__(self, obj, owner):
        def bound(other):
            return (self.label, obj.value, getattr(other, "value", other))
        return bound

    def __call__(self, *args, **kwargs):
        raise AssertionError("unbound descriptor called")


class C:
    __add__ = BindingDescriptor("add")
    __eq__ = BindingDescriptor("eq")

    def __init__(self, value):
        self.value = value


def run():
    lhs = C(10)
    rhs = C(3)
    return lhs + 5, lhs == rhs
"""
    validate_source = """
import pytest

def validate_module(module):
    assert module.run() == (("add", 10, 5), ("eq", 10, 3))
"""
    module_name = 'descriptor_special_binding'
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
