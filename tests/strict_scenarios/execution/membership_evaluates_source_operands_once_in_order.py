# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:membership
# soac: module(strict_assign=true, checked_attr=true)
def contains(needle_factory, container_factory):
    return needle_factory() in container_factory()

def not_contains(needle_factory, container_factory):
    return needle_factory() not in container_factory()
# ok
# tests/test_strict_membership_order.py::test_membership_evaluates_source_operands_once_in_order
import sys
from soac import _soac_ext, import_hook

STOCK_SOURCE = 'def contains(needle_factory, container_factory):\n    return needle_factory() in container_factory()\n\ndef not_contains(needle_factory, container_factory):\n    return needle_factory() not in container_factory()\n'
EXPECTED_ENTRY = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import types
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
