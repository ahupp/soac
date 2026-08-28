# modes:cpython
# module:ordinary_field_alias_setup
from typing import Any

class Checked:
    def __init__(self, initial: int = 1):
        self.number: int = initial
        self.flag: bool = False
        self.none_only: None = None
        self.maybe: str | None = None
        self.choice: int | str | None = None
        self.widened: float = 0.0
        self.inferred = initial

    def store_number(self, value: Any) -> None:
        self.number = value

    def store_choice(self, value: Any) -> None:
        self.choice = value

    def read_number(self):
        return self.number

    def read_inferred(self):
        return self.inferred

class SlotChecked:
    __slots__ = ('number', '__weakref__')

    def __init__(self, initial: int = 1):
        self.number: int = initial

    def store_number(self, value: Any) -> None:
        self.number = value

    def read_number(self):
        return self.number

class Defaults:
    number: int = 10

    def read_number(self):
        return self.number

class PredicateFree:
    # Participation does not turn Any or inferred declarations into predicates.
    def __init__(self, initial=1):
        self.payload: Any = initial
        self.inferred = initial

def make_reader(initial):
    class Reader:
        def __init__(self):
            self.value = initial
        def read(self):
            return self.value
    return Reader
# ok
# test_ordinary_deleted_field_key_can_compare_during_alias_setup
import sys
import pytest
from soac import _soac_ext
value = module.Checked(7)
dictionary = vars(value)
del value.number
original = ValueError("alias lookup must run")
events = []

class Alias:
    armed = True

    def __hash__(self):
        return hash("number")

    def __eq__(self, other):
        events.append((self.armed, other))
        if self.armed:
            raise original
        return False

alias = Alias()
with pytest.raises(ValueError) as insertion:
    dictionary[alias] = 23
assert insertion.value is original
assert events == [(True, "number")]
assert all(key is not alias for key in dictionary)

alias.armed = False
dictionary[alias] = 23
assert list(dictionary)[-1] is alias
alias.armed = True
with pytest.raises(ValueError) as read:
    value.read_number()
assert read.value is original
assert events[-1] == (True, "number")
