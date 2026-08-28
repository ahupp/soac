from pathlib import Path

import pytest
from soac import runtime

from tests._strict_integration import create_strict_project


class _HashRaisesStopIteration:
    def __hash__(self):
        raise StopIteration("hash")


class _CollisionRaisesStopIteration:
    def __hash__(self):
        return 0

    def __eq__(self, other):
        raise StopIteration("collision")


def test_set_from_iter_propagates_stop_iteration_from_hash():
    with pytest.raises(StopIteration, match="hash"):
        runtime.set_from_iter([_HashRaisesStopIteration()])


def test_set_from_iter_propagates_stop_iteration_from_collision():
    with pytest.raises(StopIteration, match="collision"):
        runtime.set_from_iter(
            [_CollisionRaisesStopIteration(), _CollisionRaisesStopIteration()]
        )


@pytest.mark.parametrize(
    ("values", "expected"),
    [
        pytest.param([], set(), id="empty"),
        pytest.param([1, 2, 2], {1, 2}, id="ordinary"),
    ],
)
def test_set_from_iter_preserves_successful_consumption(values, expected):
    assert runtime.set_from_iter(values) == expected


@pytest.fixture(scope="module")
def strict_set_consumption_project(tmp_path_factory):
    source = """
# soac: module(strict_assign=true, checked_attr=true)

def collect(values):
    return set(value for value in values)
"""
    return create_strict_project(
        tmp_path_factory.mktemp("strict-set-consumption"),
        {
            "set_consumption_model.py": source,
            "ordinary_set_consumption_model.py": source.replace(
                "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1
            ),
        },
        modules={"set_consumption_model": "set_consumption_model.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_transformed_set_genexpr_preserves_stop_iteration_and_success(
    strict_set_consumption_project, entry_interpreter
):
    strict_set_consumption_project.run_case(
        "set_consumption_model",
        """
import ctypes
import pytest
from soac import _soac_ext
from tests.test_regression_set_from_iter_stop_iteration import _HashRaisesStopIteration, _CollisionRaisesStopIteration
import ordinary_set_consumption_model

def validate_module(module):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert not owner(ordinary_set_consumption_model.collect)
    assert _soac_ext.strict_module_diagnostics(ordinary_set_consumption_model) is None
    for module in (ordinary_set_consumption_model, module):
        with pytest.raises(StopIteration, match="hash"):
            module.collect([_HashRaisesStopIteration()])

        with pytest.raises(StopIteration, match="collision"):
            module.collect(
                [_CollisionRaisesStopIteration(), _CollisionRaisesStopIteration()]
            )

        assert module.collect([]) == set()
        assert module.collect([1, 2, 2]) == {1, 2}
""",
        Path(__file__),
        required_functions=("collect",),
        entry_interpreter=entry_interpreter,
    )
