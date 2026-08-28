# modes:soac,entry
# module:nominal_initializing
# soac: module(strict_assign=true, checked_attr=true)
from nominal_initializing_support import before_adoption, move_alias

def factory():
    class Local:
        pass
    Alias = Local
    def accept(value: Alias) -> Alias:
        move_alias(accept.__annotate__, Local)
        return value
    before_adoption(accept, Local)
    return Local, accept

first = factory()
second = factory()
# module:nominal_initializing_support
from typing import Any

previous: Any = None
events = []

def alias_cell(function: Any) -> Any:
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    return cells["Alias"]

def move_alias(provider: Any, current: Any) -> None:
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    cells["Alias"].cell_contents = current
    events.append("body")

def before_adoption(function: Any, current: Any) -> None:
    global previous
    if previous is None:
        value = current()
        assert function(value) is value
    else:
        old_value = previous()
        class Keyword(str):
            __hash__ = str.__hash__
            def __eq__(self, other):
                # Normal binding runs this callback before the source body.
                alias_cell(function).cell_contents = previous
                events.append("keyword")
                return str.__eq__(self, other)
        assert function(**{Keyword("value"): old_value}) is old_value
        assert alias_cell(function).cell_contents is current
        # The first body changed the annotation cell. Calls still pass the
        # original values through independently of that cell's contents.
        new_value = current()
        assert function(new_value) is new_value
        assert function(old_value) is old_value
        events.append("next-call-ordinary")
    previous = current
# ok
# test_initializing_nominals_preserve_binding_callbacks_and_return_identity
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import nominal_initializing as module
from nominal_initializing_support import events

assert events == ["body", "keyword", "body", "body", "body", "next-call-ordinary"], events
left_class, left_accept = module.first
right_class, right_accept = module.second
left = left_class()
right = right_class()
assert left_accept(left) is left
assert right_accept(right) is right
print("initializing-nominal-snapshots")
