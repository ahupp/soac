from __future__ import annotations

import gc
import sys
import weakref
from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project

_ALIAS_SOURCE = """
class Token:
    pass


def alias_only(value):
    alias = value


def alias_across_loop(value):
    alias = value
    for index in range(2):
        alias.next = alias
"""


_CYCLE_SOURCE = """
import gc
import weakref


class Node:
    def __init__(self):
        self.next = None
        self.prev = None

    def link_next(self, next):
        self.next = next
        self.next.prev = self


def create_cycle(node, n_links):
    if n_links == 0:
        return

    current = node
    for index in range(n_links):
        next_node = Node()
        current.link_next(next_node)
        current = next_node

    current.link_next(node)


def create_gc_cycles(n_cycles, n_links):
    cycles = []
    for index in range(n_cycles):
        node = Node()
        cycles.append(node)
        create_cycle(node, n_links)
    return cycles


def collect_inside(n_cycles, n_links):
    gc.collect()
    cycles = create_gc_cycles(n_cycles, n_links)
    reference = weakref.ref(cycles[0])
    tracked = gc.is_tracked(cycles[0])
    del cycles
    before_collection = reference() is not None
    collected = gc.collect()
    after_collection = reference() is not None
    return collected, tracked, before_collection, after_collection
"""


def _observe_external_collection(module: object) -> dict[str, object]:
    gc.collect()
    roots = module.create_gc_cycles(4, 2)
    references = tuple(weakref.ref(root) for root in roots)
    tracked = tuple(gc.is_tracked(root) for root in roots)
    del roots

    before_collection = tuple(reference() is not None for reference in references)
    collected = gc.collect()
    after_collection = tuple(reference() is not None for reference in references)

    return {
        "collected": collected,
        "tracked": tracked,
        "before_collection": before_collection,
        "after_collection": after_collection,
    }


@pytest.fixture(scope="module")
def strict_gc_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-gc-cycles"),
        {
            "alias_model.py": "from __future__ import strict\n" + _ALIAS_SOURCE,
            "ordinary_alias_model.py": _ALIAS_SOURCE,
            "cycle_model.py": "from __future__ import strict\n" + _CYCLE_SOURCE,
            "ordinary_cycle_model.py": _CYCLE_SOURCE,
        },
        modules={"alias_model": "alias_model.py", "cycle_model": "cycle_model.py"},
    )


def _run_gc_validation(project, model, validation, witnesses, *, mode, entry_interpreter):
    import textwrap

    project.run_case(
        model,
        "import ctypes\nimport importlib\n"
        "from soac import _soac_ext\n"
        "from tests.test_regression_gc_cycles import "
        "_observe_borrowed_argument_alias, _observe_borrowed_loop_alias, _observe_external_collection\n"
        "def validate_module(transformed):\n"
        + f"    stock = importlib.import_module({'ordinary_' + model!r})\n"
        + "    assert _soac_ext.strict_module_diagnostics(stock) is None\n"
        + "    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner\n"
        + "    owner.argtypes = [ctypes.py_object]\n"
        + "    owner.restype = ctypes.c_void_p\n"
        + f"    for name in {witnesses!r}:\n"
        + "        assert not owner(getattr(stock, name))\n"
        + textwrap.indent(textwrap.dedent(validation), "    "),
        Path(__file__),
        required_functions=witnesses,
        entry_interpreter=entry_interpreter,
        opt_mode=mode,
    )


def _observe_borrowed_argument_alias(module: object) -> tuple[int, int, bool]:
    value = module.Token()
    reference = weakref.ref(value)
    before = sys.getrefcount(value)
    module.alias_only(value)
    after = sys.getrefcount(value)
    del value
    return before, after, reference() is None


def _observe_borrowed_loop_alias(module: object) -> tuple[int, int, bool, bool]:
    gc.collect()
    value = module.Token()
    reference = weakref.ref(value)
    before = sys.getrefcount(value)
    module.alias_across_loop(value)
    after = sys.getrefcount(value)
    del value
    alive_before_collection = reference() is not None
    gc.collect()
    alive_after_collection = reference() is not None
    return before, after, alive_before_collection, alive_after_collection


@pytest.mark.parametrize("mode", ["none", "profile"])
@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_transformed_borrowed_argument_alias_releases_owned_temporary(
    strict_gc_project, mode: str, entry_interpreter: bool
) -> None:
    _run_gc_validation(
        strict_gc_project,
        "alias_model",
        """
    expected = _observe_borrowed_argument_alias(stock)
    assert expected[0] == expected[1]
    assert expected[2] is True

    actual = _observe_borrowed_argument_alias(transformed)
    assert actual == expected
""",
        ("alias_only",),
        mode=mode,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("mode", ["none", "profile"])
@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_transformed_borrowed_alias_stays_borrowed_across_loop_edges(
    strict_gc_project, mode: str, entry_interpreter: bool
) -> None:
    _run_gc_validation(
        strict_gc_project,
        "alias_model",
        """
    expected = _observe_borrowed_loop_alias(stock)
    assert expected[1] == expected[0] + 1
    assert expected[2:] == (True, False)

    actual = _observe_borrowed_loop_alias(transformed)
    assert actual == expected
""",
        ("alias_across_loop",),
        mode=mode,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("mode", ["none", "profile"])
@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_transformed_cycle_builder_releases_returned_roots(
    strict_gc_project, mode: str, entry_interpreter: bool
) -> None:
    _run_gc_validation(
        strict_gc_project,
        "cycle_model",
        """
    expected = _observe_external_collection(stock)
    assert expected["tracked"] == (True, True, True, True), expected
    assert expected["before_collection"] == (True, True, True, True), expected
    assert expected["after_collection"] == (False, False, False, False), expected

    actual = _observe_external_collection(transformed)
    assert actual["tracked"] == expected["tracked"], actual
    assert actual["before_collection"] == expected["before_collection"], actual
    assert actual["after_collection"] == expected["after_collection"], actual
    assert actual["collected"] >= 12, actual
""",
        ("create_gc_cycles", "create_cycle"),
        mode=mode,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("mode", ["none", "profile"])
@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_transformed_cyclic_gc_matches_stock_inside_native_caller(
    strict_gc_project, mode: str, entry_interpreter: bool
) -> None:
    _run_gc_validation(
        strict_gc_project,
        "cycle_model",
        """
    expected = stock.collect_inside(4, 2)
    expected_count, expected_tracked, expected_before, expected_after = expected
    assert expected_count >= 12, expected
    assert expected_tracked is True, expected
    assert expected_before is True, expected
    assert expected_after is False, expected

    actual = transformed.collect_inside(4, 2)
    collected, tracked, before_collection, after_collection = actual
    assert tracked == expected_tracked, actual
    assert before_collection == expected_before, actual
    assert after_collection == expected_after, actual
    assert collected >= 12, actual
""",
        ("collect_inside",),
        mode=mode,
        entry_interpreter=entry_interpreter,
    )
