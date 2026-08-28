from __future__ import annotations

from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project


@pytest.fixture(scope="module")
def strict_unbound_local_project(tmp_path_factory):
    source = """
# soac: module(strict_assign=true, checked_attr=true)

def f(flag):
    if flag:
        x = 1
    return x
"""
    return create_strict_project(
        tmp_path_factory.mktemp("strict-unbound-local"),
        {
            "unbound_local_model.py": source,
            "ordinary_unbound_local_model.py": source.replace(
                "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1
            ),
        },
        modules={"unbound_local_model": "unbound_local_model.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_entry_maybe_unbound_local_raises_without_stack_seed(
    strict_unbound_local_project, entry_interpreter
):
    strict_unbound_local_project.run_case(
        "unbound_local_model",
        """
import ctypes
import pytest
from soac import _soac_ext
import ordinary_unbound_local_model

def validate_module(module):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert not owner(ordinary_unbound_local_model.f)
    assert _soac_ext.strict_module_diagnostics(ordinary_unbound_local_model) is None
    for module in (ordinary_unbound_local_model, module):
        assert module.f(True) == 1
        with pytest.raises(UnboundLocalError):
            module.f(False)
""",
        Path(__file__),
        required_functions=("f",),
        entry_interpreter=entry_interpreter,
    )
