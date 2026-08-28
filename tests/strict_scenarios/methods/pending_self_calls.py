# modes:soac,entry
# module:nominal_construction
# soac: module(strict_assign=true, checked_attr=true)
from nominal_construction_support import check_before_name_store

class Base:
    def __init_subclass__(cls):
        check_before_name_store(cls)

class Child(Base):
    def accept(self, value: Child) -> Child:
        return value
    alias = accept
# module:nominal_construction_support
from typing import Any

events = []

def check_before_name_store(cls: Any) -> None:
    # type_new has not returned and the module has not stored Child yet.
    namespace = cls.accept.__globals__
    assert "Child" not in namespace
    from soac.strict import StrictMutationError
    try:
        cls()
    except StrictMutationError:
        events.append("pending-allocation")
    else:
        raise AssertionError("construction callback allocated an unfinished type")
    for method in (cls.accept, cls.alias):
        value = object()
        assert method(object(), value) is value
        events.append("pending ordinary call")
# ok
# test_pending_self_type_rejects_allocation_but_not_ordinary_method_calls
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import nominal_construction as module
from nominal_construction_support import events

assert events == ["pending-allocation", "pending ordinary call", "pending ordinary call"]
value = module.Child()
assert value.accept(value) is value and value.alias(value) is value
marker = object()
assert value.accept(marker) is marker
