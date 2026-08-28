# modes:soac,entry
# module:quoted_nominals
# soac: module(strict_assign=true, checked_attr=true)
from typing import Optional
from quoted_nominal_support import before_ready, arbitrary_result, annotation_trap

class Token:
    def accept(self, value: "Token") -> "Token":
        return value

GlobalAlias = Token

def accept_global(value: "GlobalAlias") -> "Token":
    return value

def optional(value: "Optional[Token | GlobalAlias]") -> "Optional[Token]":
    return value

def two(first: "Token", second: "GlobalAlias") -> "GlobalAlias":
    return second

def wrong_return() -> "Token":
    return arbitrary_result()

accept_global.__annotate__ = annotation_trap

class Base:
    def __init_subclass__(cls):
        before_ready(cls)

class Ready(Base):
    def accept(self, value: "Ready") -> "Ready":
        return value

def factory():
    class Local:
        def accept(self, value: "Local") -> "Local":
            return value
    return Local
# module:quoted_nominal_support
from typing import Any
events = []

def before_ready(cls: Any) -> None:
    from soac.strict import StrictMutationError

    try:
        cls()
    except StrictMutationError:
        events.append("pending allocation rejected")
    else:
        raise AssertionError("quoted callback allocated a pending type")
    # The type is Pending, but a method with no protected write remains an
    # ordinary call even when its annotations name that unfinished class.
    value = object()
    assert cls.accept(None, value) is value
    events.append("pre-ready ordinary call")

def arbitrary_result() -> Any:
    return object()

def annotation_trap(format: int) -> Any:
    events.append("annotation evaluated")
    raise AssertionError("nominal binding evaluated the annotation provider")
# ok
# test_quoted_nominals_keep_ordinary_calls_without_provider_evaluation
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from soac import _soac_ext
import quoted_nominals as module
from quoted_nominal_support import annotation_trap, events

diagnostic = _soac_ext.strict_module_diagnostics(module)
assert diagnostic['sealed']
assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
# The adapter checks the actual published artifact generation before and after this block.
for function in (module.Token.accept, module.Ready.accept, module.accept_global,
                 module.optional, module.two, module.wrong_return, module.factory):
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry
assert events == ["pending allocation rejected", "pre-ready ordinary call"], events
assert module.accept_global.__annotate__ is annotation_trap

# The original successful self-call now occurs after actual final admission.
ready = module.Ready()
assert ready.accept(ready) is ready
marker = object()
assert ready.accept(marker) is marker

class OrdinaryChild(module.Token):
    pass

for value in (module.Token(), OrdinaryChild()):
    assert module.Token().accept(value) is value
    assert module.accept_global(value) is value
    assert module.optional(value) is value
    assert module.two(module.Token(), value) is value
assert module.optional(None) is None

for function, arguments in (
    (module.accept_global, (object(),)),
    (module.optional, (object(),)),
    (module.two, (object(), module.Token())),
    (module.two, (module.Token(), object())),
    (module.wrong_return, ()),
):
    result = function(*arguments)
    if arguments:
        assert result is arguments[-1]
    else:
        assert type(result) is object

def ordinary_factory():
    class Local:
        def accept(self, value: "Local") -> "Local":
            return value
    return Local

ordinary = ordinary_factory()
assert _soac_ext.strict_function_entry_kind(ordinary.accept) is None
native_captures = ordinary.accept.__annotate__.__code__.co_freevars
first, second = module.factory(), module.factory()
assert first is not second
assert first.__qualname__ == second.__qualname__
for actual, other in ((first, second), (second, first)):
    assert _soac_ext.strict_function_entry_kind(actual.accept) == expected_entry
    # Keep exactly the native provider layout, including special class-dict
    # captures. Do not manufacture a lexical cell from the quoted class name.
    assert actual.accept.__annotate__.__code__.co_freevars == native_captures
    value = actual()
    assert value.accept(value) is value
    other_value = other()
    assert value.accept(other_value) is other_value
assert events == ["pending allocation rejected", "pre-ready ordinary call"], events
