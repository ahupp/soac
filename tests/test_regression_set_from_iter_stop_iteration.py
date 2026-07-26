import pytest

from soac import runtime
from tests._integration import soac_module


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


def test_transformed_set_genexpr_preserves_stop_iteration_and_success(tmp_path):
    source = """
def collect(values):
    return set(value for value in values)
"""

    with soac_module(tmp_path, "set_genexpr_stop_iteration", source) as module:
        with pytest.raises(StopIteration, match="hash"):
            module.collect([_HashRaisesStopIteration()])

        with pytest.raises(StopIteration, match="collision"):
            module.collect(
                [_CollisionRaisesStopIteration(), _CollisionRaisesStopIteration()]
            )

        assert module.collect([]) == set()
        assert module.collect([1, 2, 2]) == {1, 2}
