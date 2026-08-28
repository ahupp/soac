# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:keyword_calls
# soac: module(strict_assign=true, checked_attr=true)

from keyword_probe import dynamic, events

@dynamic
def named(alpha=1, beta=2, *, gamma=3):
    events.append("named body")
    return (alpha, beta, gamma)

@dynamic
def positional_only(alpha=1, /, beta=2):
    events.append("positional body")
    return (alpha, beta)

@dynamic
def collecting(alpha=1, /, beta=2, **extras):
    events.append("collecting body")
    return (alpha, beta, extras)

@dynamic
def changing(alpha=1, beta=2):
    events.append("original changing body")
    return ("old", alpha, beta)
# module:keyword_control
from keyword_probe import dynamic, events

@dynamic
def named(alpha=1, beta=2, *, gamma=3):
    events.append("named body")
    return (alpha, beta, gamma)

@dynamic
def positional_only(alpha=1, /, beta=2):
    events.append("positional body")
    return (alpha, beta)

@dynamic
def collecting(alpha=1, /, beta=2, **extras):
    events.append("collecting body")
    return (alpha, beta, extras)

@dynamic
def changing(alpha=1, beta=2):
    events.append("original changing body")
    return ("old", alpha, beta)
# module:keyword_probe
events = []
comparison_error = LookupError("keyword comparison failed")

def dynamic(function):
    return function

class Keyword(str):
    __hash__ = str.__hash__

    def __new__(cls, text, target, *, error=None, callback=None):
        value = super().__new__(cls, text)
        value.target = target
        value.error = error
        value.callback = callback
        return value

    def __eq__(self, other):
        events.append(("compare", other))
        if self.callback is not None:
            self.callback(other)
        if self.error is not None:
            raise self.error
        return other == self.target

def replacement(alpha=1, beta=2):
    return ("new", alpha, beta)

def exercise(module):
    events.clear()
    key = Keyword("not-a-parameter", "beta")
    assert module.named(**{key: 9}) == (1, 9, 3)
    assert events == [("compare", "alpha"), ("compare", "beta"), "named body"], events

    # String payload equality must not bypass a subclass's false comparison.
    events.clear()
    key = Keyword("alpha", "absent")
    try:
        module.named(**{key: 9})
    except TypeError:
        pass
    else:
        raise AssertionError("keyword payload bypassed __eq__")
    assert events == [("compare", "alpha"), ("compare", "beta"), ("compare", "gamma")], events

    # Explicit-keyword errors precede excess positional arguments and defaults.
    events.clear()
    key = Keyword("not-a-parameter", "alpha", error=comparison_error)
    try:
        module.named(1, 2, 3, **{key: 4})
    except LookupError as error:
        assert error is comparison_error
    else:
        raise AssertionError("keyword comparison exception disappeared")
    assert events == [("compare", "alpha")], events

    events.clear()
    key = Keyword("not-a-parameter", "alpha")
    try:
        module.named(1, **{key: 4})
    except TypeError as error:
        assert "multiple values" in str(error), str(error)
    else:
        raise AssertionError("duplicate binding bypassed keyword equality")
    assert events == [("compare", "alpha")], events

    # Positional-only names are excluded from ordinary keyword matching.
    events.clear()
    key = Keyword("alpha", "alpha")
    alpha, beta, extras = module.collecting(**{key: 9})
    assert (alpha, beta) == (1, 2)
    assert list(extras.values()) == [9]
    assert next(iter(extras)) is key
    assert events == [("compare", "beta"), "collecting body"], events

    events.clear()
    key = Keyword("not-a-parameter", "alpha")
    try:
        module.positional_only(1, **{key: 4})
    except TypeError as error:
        assert "positional-only" in str(error), str(error)
    else:
        raise AssertionError("positional-only conflict was accepted")
    assert events == [("compare", "beta"), ("compare", "alpha")], events

    # The original active frame and name objects survive code replacement.
    events.clear()
    def replace_code(other):
        if other == "alpha":
            module.changing.__code__ = replacement.__code__
    key = Keyword("not-a-parameter", "beta", callback=replace_code)
    assert module.changing(**{key: 11}) == ("old", 1, 11)
    assert events == [("compare", "alpha"), ("compare", "beta"), "original changing body"], events
    assert module.changing(beta=12) == ("new", 1, 12)
# ok
# tests/test_strict_function_boundaries.py::test_explicit_keyword_subclasses_preserve_native_binding_callbacks
import sys
from soac import _soac_ext, import_hook

import keyword_calls
import keyword_control
from keyword_probe import exercise

exercise(keyword_control)
exercise(keyword_calls)
print("keyword-comparison-binding")
