# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:checked_defaults
# soac: module(strict_assign=true, checked_attr=true)
import probe

def checked(first: int = 1, *, left: int = 2, right: int = 3) -> int:
    return first + left + right

def lifetime(*, value):
    probe.replace_then_observe(lifetime, value)

def factory():
    value = 10
    def captured(*, left: int = 1) -> int:
        return value + left
    return captured

probe.compare(checked, lifetime, factory)
# module:probe
events = []
results = []

def stock(first: int = 1, *, left: int = 2, right: int = 3) -> int:
    return first + left + right

def stock_lifetime(*, value):
    replace_then_observe(stock_lifetime, value)

def stock_factory():
    value = 10
    def captured(*, left: int = 1) -> int:
        return value + left
    return captured

class Marker:
    def __init__(self, name):
        self.name = name
    def __del__(self):
        events.append("drop:" + self.name)

def replace_then_observe(function, value):
    function.__kwdefaults__ = {"value": None}
    events.append("use:" + value.name)

def scenarios(function, lifetime):
    import gc
    saved = function.__kwdefaults__
    saved_lifetime = lifetime.__kwdefaults__
    observed = {}

    def attempt(name, call):
        events.clear()
        try:
            value = call()
        except BaseException as error:
            value = (type(error).__name__, str(error) if isinstance(error, LookupError) else None)
        observed[name] = (value, tuple(events))

    class RaisingKey:
        def __hash__(self):
            return hash("left")
        def __eq__(self, other):
            events.append("lookup:left")
            raise LookupError("default-key equality failed")

    events.clear()
    function.__kwdefaults__ = {RaisingKey(): 7, "right": 3}
    assert events == [], "assigning kwdefaults must not look up parameter keys"
    attempt("provided", lambda: function(left=4, right=5))
    attempt("duplicate", lambda: function(1, first=2, left=4, right=5))
    attempt("unexpected", lambda: function(extra=1))
    attempt("lookup-error", lambda: function(right=5))

    class RaisingLaterKey:
        def __hash__(self):
            return hash("right")
        def __eq__(self, other):
            events.append("lookup:right")
            raise LookupError("later default lookup failed")

    function.__kwdefaults__ = {RaisingLaterKey(): 3}
    attempt("missing-before-later-error", lambda: function())

    class ReplacingKey:
        def __hash__(self):
            return hash("left")
        def __eq__(self, other):
            events.append("replace:left")
            function.__kwdefaults__ = {"left": 11, "right": 20}
            return other == "left"

    # Retain this dictionary to isolate lookup order from the lifetime case:
    # each missing parameter must observe the then-current function metadata.
    original = {ReplacingKey(): 7, "right": 3}
    function.__kwdefaults__ = original
    attempt("replaced", lambda: function())
    function.__kwdefaults__ = saved

    events.clear()
    lifetime.__kwdefaults__ = {"value": Marker("old-keyword")}
    lifetime()
    events.append("after-call")
    gc.collect()
    observed["lifetime"] = tuple(events)
    lifetime.__kwdefaults__ = saved_lifetime
    return observed

def closure_scenario(factory):
    from ctypes import c_int, py_object, pythonapi
    set_closure = pythonapi.PyFunction_SetClosure
    set_closure.argtypes = [py_object, py_object]
    set_closure.restype = c_int
    function = factory()
    value = 30
    replacement = (lambda: value).__closure__
    class ClosureKey:
        def __hash__(self):
            return hash("left")
        def __eq__(self, other):
            events.append("replace:closure")
            assert set_closure(function, replacement) == 0
            return other == "left"
    original = {ClosureKey(): 1}
    function.__kwdefaults__ = original
    events.clear()
    return function(), tuple(events)

def compare(function, lifetime, factory):
    expected = scenarios(stock, stock_lifetime)
    assert expected["provided"] == (10, ())
    assert expected["duplicate"] == (("TypeError", None), ())
    assert expected["unexpected"] == (("TypeError", None), ())
    assert expected["lookup-error"] == (("LookupError", "default-key equality failed"), ("lookup:left",))
    assert expected["missing-before-later-error"] == (("LookupError", "later default lookup failed"), ("lookup:right",))
    assert expected["replaced"] == (28, ("replace:left",))
    assert expected["lifetime"] == ("use:old-keyword", "drop:old-keyword", "after-call")
    actual = scenarios(function, lifetime)
    actual_lifetime = actual.pop("lifetime")
    expected_lifetime = expected.pop("lifetime")
    assert actual == expected, (actual, expected)
    assert tuple(event for event in actual_lifetime if not event.startswith("drop:")) == (
        "use:old-keyword", "after-call",
    ), actual_lifetime
    assert actual_lifetime.count("drop:old-keyword") == 1, actual_lifetime
    actual["lifetime"] = actual_lifetime
    expected["lifetime"] = expected_lifetime
    expected_closure = closure_scenario(stock_factory)
    assert expected_closure == (31, ("replace:closure",))
    assert closure_scenario(factory) == expected_closure
    results.append(actual)
# ok
# tests/test_strict_function_boundaries.py::test_keyword_defaults_match_native_lookup_order_errors_and_lifetimes
import sys
from soac import _soac_ext, import_hook

import checked_defaults
import probe
assert len(probe.results) == 1
assert checked_defaults.checked() == 6
print("native-keyword-default-order")
