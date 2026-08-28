# modes:soac,entry
# module:nominal_methods
# soac: module(strict_assign=true, checked_attr=true)
from nominal_support import (
    annotation_trap, arbitrary_result, assert_method_provider_frozen, events,
)

class Token:
    def accept(self, value: Token) -> Token:
        events.append("accept")
        return value

    def optional(self, value: Token | None) -> Token | None:
        return value

    def wrong_return(self) -> Token:
        events.append("return")
        return arbitrary_result()

class Child(Token):
    def accept_base(self, value: Token) -> Token:
        return value

GlobalAlias = Token

def accept_global(value: GlobalAlias) -> Token:
    return value

# The free function is still initializing. Its replaceable provider is not
# executed or trusted for required contracts during adoption or calls.
accept_global.__annotate__ = annotation_trap
# Token already completed its actual class Store, before this module seals.
assert_method_provider_frozen(Token.accept, module_sealed=False)

def factory():
    class Local:
        def accept(self, value: Local) -> Local:
            return value
    return Local
# module:nominal_support
from typing import Any

events = []

def annotation_trap(format: int) -> Any:
    events.append("annotation evaluated")
    raise AssertionError("nominal binding evaluated an annotation provider")

def arbitrary_result() -> Any:
    return object()


def assert_method_provider_frozen(method: Any, *, module_sealed: bool) -> None:
    import sys
    from soac import _soac_ext
    from soac.strict import StrictMutationError

    namespace = method.__globals__
    module = sys.modules[namespace["__name__"]]
    assert vars(module) is namespace
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    assert diagnostic is not None and diagnostic["sealed"] is module_sealed
    provider = method.__annotate__
    assert provider is not None and provider is not annotation_trap
    try:
        method.__annotate__ = annotation_trap
    except StrictMutationError:
        pass
    else:
        raise AssertionError("admitted method accepted a replacement annotation provider")
    assert method.__annotate__ is provider
# ok
# test_owned_method_nominals_do_not_constrain_values_or_consult_membership_hooks
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
import nominal_methods as module
from nominal_support import annotation_trap, assert_method_provider_frozen, events

assert events == []
assert module.accept_global.__annotate__ is annotation_trap
assert_method_provider_frozen(module.Token.accept, module_sealed=True)

class OrdinaryChild(module.Token):
    pass

is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int
assert is_sealed(module.Token) == 1
assert is_sealed(module.Child) == 1
assert is_sealed(OrdinaryChild) == 0

receiver = module.Token()
for value in (module.Token(), module.Child(), OrdinaryChild()):
    assert receiver.accept(value) is value
    assert receiver.optional(value) is value
    assert module.Child().accept_base(value) is value
    assert module.accept_global(value) is value
assert receiver.optional(None) is None
assert events == ["accept"] * 3

class Spoof:
    @property
    def __class__(self):
        events.append("spoof consulted")
        return module.Token

for value in (object(), Spoof()):
    before = list(events)
    assert receiver.accept(value) is value
    assert events == before + ["accept"], "an overridable membership hook ran"

assert type(receiver.wrong_return()) is object
assert events[-1] == "return"
assert "annotation evaluated" not in events
print("owned-method-nominal-boundaries")
# ok
# test_same_source_factory_methods_keep_distinct_classes_and_collectable_owners
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import gc
import weakref
import nominal_methods as module

first = module.factory()
second = module.factory()
assert first is not second
assert first.__qualname__ == second.__qualname__
left = first()
right = second()
assert left.accept(left) is left
assert right.accept(right) is right

# Changing the original annotation cell does not alter the function body or
# merge the identities of two executions of the same class source.
provider = first.accept.__annotate__
cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
assert "Local" in cells
cells["Local"].cell_contents = second
assert left.accept(left) is left

for receiver, value in ((left, right), (right, left)):
    assert receiver.accept(value) is value

class OrdinaryChild(first):
    pass

child = OrdinaryChild()
assert left.accept(child) is child
assert right.accept(child) is child

def collectable_contract_cycle():
    local = module.factory()
    return weakref.ref(local), weakref.ref(local.accept)

references = collectable_contract_cycle()
gc.collect()
assert all(reference() is None for reference in references)
print("factory-nominal-isolation")
