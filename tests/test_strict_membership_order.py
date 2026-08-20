"""Membership evaluates its needle before its container in both entry modes."""

import pytest

from tests._strict_integration import create_strict_project

SOURCE = """from __future__ import strict
def contains(needle_factory, container_factory):
    return needle_factory() in container_factory()

def not_contains(needle_factory, container_factory):
    return needle_factory() not in container_factory()
"""


@pytest.fixture(scope="module")
def membership_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-membership-order"),
        {"membership.py": SOURCE},
        modules={"membership": "membership.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_membership_evaluates_source_operands_once_in_order(
    membership_project, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    stock = SOURCE.removeprefix("from __future__ import strict\n")
    membership_project.run(
        f"STOCK_SOURCE = {stock!r}\nEXPECTED_ENTRY = {expected_entry!r}\n"
        + """import types
import membership
from soac import _soac_ext

control = types.ModuleType('stock_membership')
exec(compile(STOCK_SOURCE, '<stock-membership>', 'exec', dont_inherit=True), vars(control))

def exercise(module, name, fail):
    events = []
    marker = ValueError('operand failure')
    def needle():
        events.append('needle')
        if fail == 'needle':
            raise marker
        return 1
    def container():
        events.append('container')
        if fail == 'container':
            raise marker
        return [1]
    function = getattr(module, name)
    if fail is None:
        assert function(needle, container) is (name == 'contains')
    else:
        try:
            function(needle, container)
        except ValueError as error:
            assert error is marker
        else:
            raise AssertionError('the original operand exception was lost')
    return events

for name in ('contains', 'not_contains'):
    function = getattr(membership, name)
    assert _soac_ext.strict_function_entry_kind(function) == EXPECTED_ENTRY
    for fail in (None, 'needle', 'container'):
        expected = exercise(control, name, fail)
        assert expected == (['needle'] if fail == 'needle' else ['needle', 'container'])
        actual = exercise(membership, name, fail)
        assert actual == expected, (name, fail, actual, expected)
    assert _soac_ext.strict_function_entry_kind(function) == EXPECTED_ENTRY
""",
        entry_interpreter=entry_interpreter,
    )
