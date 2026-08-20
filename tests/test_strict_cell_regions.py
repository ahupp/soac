"""Actual closure-cell ownership through profiled arithmetic hot/fallback regions."""

import pytest

from tests._strict_integration import create_strict_project

SOURCE = """from __future__ import strict
def factory(left, right):
    def add():
        return left + right
    def replace(first, second):
        nonlocal left, right
        left = first
        right = second
    def discard_right():
        nonlocal right
        del right
    return add, replace, discard_right

def membership_factory(needle, container):
    def read():
        return needle in container
    def clear(new_needle, new_container):
        nonlocal needle, container
        needle = new_needle
        container = new_container
    return read, clear
"""

TRAINING = """import cells
from soac import _soac_ext
assert _soac_ext.strict_function_entry_kind(cells.factory) == EXPECTED_ENTRY
add, replace, discard = cells.factory(1000, 2000)
assert _soac_ext.strict_function_entry_kind(add) == EXPECTED_ENTRY
for _ in range(1000):
    assert add() == 3000
assert _soac_ext.strict_function_entry_kind(add) == EXPECTED_ENTRY
"""

VALIDATION = """import gc
import sys
import types
import cells
from soac import _soac_ext

control = types.ModuleType('stock_cells')
exec(compile(STOCK_SOURCE, '<stock-cells>', 'exec', dont_inherit=True), vars(control))

def witness(module, *functions):
    expected = EXPECTED_ENTRY if module is cells else None
    for function in functions:
        actual = _soac_ext.strict_function_entry_kind(function)
        assert actual == expected, (function.__qualname__, actual, expected)

def exercise(module, fail):
    events = []
    marker = ValueError('cell arithmetic failed')
    class Operand:
        def __init__(self, name):
            self.name = name
        def __add__(self, other):
            events.append('add')
            replace(None, None)
            if fail:
                raise marker
            return 17
        def __del__(self):
            events.append(self.name)

    witness(module, module.factory)
    add, replace, discard = module.factory(Operand('left'), Operand('right'))
    witness(module, add, replace, discard)
    if fail:
        try:
            add()
        except ValueError as error:
            assert error is marker
        else:
            raise AssertionError('missing arithmetic exception')
        marker.__traceback__ = None
    else:
        assert add() == 17
    gc.collect()
    # The explicit arithmetic callback and both required finalizers remain
    # observable. Their implicit relative release order is not a SOAC contract.
    assert events[0] == 'add', events
    assert sorted(events[1:]) == ['left', 'right'], events
    if module is control:
        assert events == ['add', 'right', 'left'], events
    witness(module, module.factory, add, replace, discard)
    return (events[0], sorted(events[1:]))

for fail in (False, True):
    assert exercise(cells, fail) == exercise(control, fail)

# Loading the first cell must not leak its owning snapshot when the second
# cell is unbound. The control and strict paths both keep only the live cell.
def check_unbound(module):
    value = object()
    add, replace, discard = module.factory(value, 1)
    witness(module, module.factory, add, replace, discard)
    discard()
    before = sys.getrefcount(value)
    for _ in range(20):
        try:
            add()
        except NameError:
            pass
        else:
            raise AssertionError('an empty cell must raise')
    assert sys.getrefcount(value) == before
    replace(1000, 2000)
    assert add() == 3000
    witness(module, module.factory, add, replace, discard)

check_unbound(control)
check_unbound(cells)

# Membership's C-call operand order is container/needle, unlike source order.
# Clearing the captured cells leaves only the operation and traceback owners.
def check_membership(module, fail):
    events = []
    marker = ValueError('membership failed')
    class Needle:
        def __del__(self):
            events.append('needle')
    class Container:
        def __contains__(self, needle):
            events.append('contains')
            clear(None, None)
            if fail:
                raise marker
            return True
        def __del__(self):
            events.append('container')
    read, clear = module.membership_factory(Needle(), Container())
    witness(module, module.membership_factory, read, clear)
    if fail:
        try:
            read()
        except ValueError as error:
            assert error is marker
        else:
            raise AssertionError('missing membership exception')
        marker.__traceback__ = None
    else:
        assert read() is True
        if module is control:
            assert events == ['contains', 'container', 'needle'], events
    gc.collect()
    assert events[0] == 'contains', events
    assert sorted(events[1:]) == ['container', 'needle'], events
    witness(module, read, clear)
    return (events[0], sorted(events[1:]))

for fail in (False, True):
    assert check_membership(cells, fail) == check_membership(control, fail)
"""


@pytest.fixture(scope="module")
def cells_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-cell-regions"),
        {"cells.py": SOURCE},
        modules={"cells": "cells.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_profiled_cell_regions_preserve_owned_inputs_and_fallback_order(
    cells_project, tmp_path, entry_interpreter
):
    work = tmp_path / "profile"
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    mode = f"EXPECTED_ENTRY = {expected_entry!r}\n"
    cells_project.run(
        mode + TRAINING,
        entry_interpreter=entry_interpreter,
        extra_env={"SOAC_OPT_MODE": "profile", "SOAC_WORK_DIR": str(work)},
    )
    stock = SOURCE.removeprefix("from __future__ import strict\n")
    cells_project.run(
        mode + f"STOCK_SOURCE = {stock!r}\n" + VALIDATION,
        entry_interpreter=entry_interpreter,
        extra_env={"SOAC_OPT_MODE": "apply", "SOAC_WORK_DIR": str(work)},
    )
