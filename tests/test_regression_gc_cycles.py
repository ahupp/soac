from __future__ import annotations

import gc
from pathlib import Path
import sys
import weakref

import pytest

from tests._integration import soac_module, stock_module


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


def _configure_runtime(monkeypatch: pytest.MonkeyPatch, tmp_path: Path, mode: str) -> None:
    monkeypatch.setenv("SOAC_OPT_MODE", mode)
    monkeypatch.setenv("SOAC_WORK_DIR", str(tmp_path / f"gc-cycles-{mode}"))
    monkeypatch.setenv("SOAC_COMPILE_MODE", "eager")
    monkeypatch.setenv("SOAC_BACKGROUND_JIT", "0")


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
def test_transformed_borrowed_argument_alias_releases_owned_temporary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    _configure_runtime(monkeypatch, tmp_path, mode)

    with stock_module(tmp_path, f"argument_alias_stock_{mode}", _ALIAS_SOURCE) as stock:
        expected = _observe_borrowed_argument_alias(stock)
    assert expected[0] == expected[1]
    assert expected[2] is True

    with soac_module(tmp_path, f"argument_alias_soac_{mode}", _ALIAS_SOURCE) as transformed:
        actual = _observe_borrowed_argument_alias(transformed)

    assert actual == expected


@pytest.mark.parametrize("mode", ["none", "profile"])
def test_transformed_borrowed_alias_stays_borrowed_across_loop_edges(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    _configure_runtime(monkeypatch, tmp_path, mode)

    with stock_module(tmp_path, f"loop_alias_stock_{mode}", _ALIAS_SOURCE) as stock:
        expected = _observe_borrowed_loop_alias(stock)
    assert expected[1] == expected[0] + 1
    assert expected[2:] == (True, False)

    with soac_module(tmp_path, f"loop_alias_soac_{mode}", _ALIAS_SOURCE) as transformed:
        actual = _observe_borrowed_loop_alias(transformed)

    assert actual == expected


@pytest.mark.parametrize("mode", ["none", "profile"])
def test_transformed_cycle_builder_releases_returned_roots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    _configure_runtime(monkeypatch, tmp_path, mode)

    with stock_module(tmp_path, f"gc_cycles_external_stock_{mode}", _CYCLE_SOURCE) as stock:
        expected = _observe_external_collection(stock)

    assert expected["tracked"] == (True, True, True, True), expected
    assert expected["before_collection"] == (True, True, True, True), expected
    assert expected["after_collection"] == (False, False, False, False), expected

    with soac_module(tmp_path, f"gc_cycles_external_soac_{mode}", _CYCLE_SOURCE) as transformed:
        actual = _observe_external_collection(transformed)

    assert actual["tracked"] == expected["tracked"], actual
    assert actual["before_collection"] == expected["before_collection"], actual
    assert actual["after_collection"] == expected["after_collection"], actual
    assert actual["collected"] >= 12, actual


@pytest.mark.parametrize("mode", ["none", "profile"])
def test_transformed_cyclic_gc_matches_stock_inside_native_caller(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    _configure_runtime(monkeypatch, tmp_path, mode)

    with stock_module(tmp_path, f"gc_cycles_internal_stock_{mode}", _CYCLE_SOURCE) as stock:
        expected = stock.collect_inside(4, 2)

    expected_count, expected_tracked, expected_before, expected_after = expected
    assert expected_count >= 12, expected
    assert expected_tracked is True, expected
    assert expected_before is True, expected
    assert expected_after is False, expected

    with soac_module(tmp_path, f"gc_cycles_internal_soac_{mode}", _CYCLE_SOURCE) as transformed:
        actual = transformed.collect_inside(4, 2)

    collected, tracked, before_collection, after_collection = actual
    assert tracked == expected_tracked, actual
    assert before_collection == expected_before, actual
    assert after_collection == expected_after, actual
    assert collected >= 12, actual
