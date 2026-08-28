# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:binding_identity
# soac: module(strict_assign=true, checked_attr=true)

from binding_identity_probe import dynamic, events

@dynamic
def plain(*, value=1):
    return value

@dynamic
def stream(*, value=1):
    events.append(("source body", value))
    yield value
# module:binding_identity_control
from binding_identity_probe import dynamic, events

@dynamic
def plain(*, value=1):
    return value

@dynamic
def stream(*, value=1):
    events.append(("source body", value))
    yield value
# module:binding_identity_probe
events = []
held = []

def dynamic(function):
    return function

def replacement(*, value=1):
    yield value + 100

class IdentityKey:
    def __init__(self, function, reenter=False):
        self.function = function
        self.expected = function.__code__.co_varnames[0]
        self.reenter = reenter

    def __hash__(self):
        return hash(self.expected)

    def __eq__(self, other):
        events.append(("name identity", other is self.expected))
        assert other is self.expected, "binder replaced the native parameter-name object"
        if self.reenter:
            # This completes another binding/construction on the same function
            # without recursively consulting its defaults or running its body.
            held.append(self.function(value=99))
            self.function.__kwdefaults__ = {"value": 20}
            self.function.__code__ = replacement.__code__
        return True

def exercise(module):
    events.clear()
    held.clear()
    module.plain.__kwdefaults__ = {IdentityKey(module.plain): 7}
    assert module.plain() == 7
    assert events == [("name identity", True)], events
    module.plain.__kwdefaults__ = {}

    events.clear()
    module.stream.__kwdefaults__ = {IdentityKey(module.stream, reenter=True): 7}
    created = module.stream()
    assert events == [("name identity", True)], events
    assert list(created) == [7]
    assert list(held.pop()) == [99]
    assert events == [("name identity", True), ("source body", 7), ("source body", 99)], events
    assert list(module.stream()) == [120]
# ok
# tests/test_strict_function_boundaries.py::test_name_identity_and_reentrant_generator_creation_match_native
import sys
from soac import _soac_ext, import_hook

import binding_identity
import binding_identity_control
from binding_identity_probe import exercise

exercise(binding_identity_control)
exercise(binding_identity)
print("original-name-and-generator-binding")
# ok
# tests/test_strict_function_boundaries.py::test_same_code_assignment_keeps_dynamic_source_owner
import sys
from soac import _soac_ext, import_hook

expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
timing = 'idle'

import ctypes
import binding_identity as actual
import binding_identity_control as ordinary
from soac import _soac_ext

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = arguments
    function.restype = result
    return function

obj = ctypes.py_object
owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
seal = api("PyFunction_GetSoacStrictId", ctypes.c_uint64, obj)
get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
assert _soac_ext.strict_module_diagnostics(actual)["sealed"]
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert owner(actual.plain) and not owner(ordinary.plain)
assert _soac_ext.strict_function_entry_kind(actual.plain) == expected_entry

def exercise(function):
    # The existing unknown-decorator fixture is deliberately dynamic. This
    # must not weaken the same-code rejection for metadata-sealed functions.
    assert seal(function) == 0
    code = function.__code__
    original_owner = owner(function)
    original_entry = get_vectorcall(function)
    original_defaults = function.__kwdefaults__
    marker = object()
    events = []
    parameter_name = code.co_varnames[0]

    class SameCodeKey:
        def __hash__(self):
            return hash(parameter_name)
        def __eq__(self, other):
            events.append(other is parameter_name)
            function.__code__ = code
            return other == parameter_name

    try:
        if timing == "binding":
            function.__kwdefaults__ = {SameCodeKey(): marker}
            assert function() is marker
            assert events == [True], events
        else:
            function.__code__ = code
        assert function.__code__ is code and owner(function) == original_owner
        assert get_vectorcall(function) == original_entry
        # The active binder must finish once, and the next invocation must
        # preserve source authority after the public same-code assignment.
        assert function(value=marker) is marker
        assert events == ([True] if timing == "binding" else []), events
    finally:
        function.__kwdefaults__ = original_defaults
    assert function() == 1

exercise(ordinary.plain)
exercise(actual.plain)
# ok
# tests/test_strict_function_boundaries.py::test_same_code_assignment_keeps_dynamic_source_owner
import sys
from soac import _soac_ext, import_hook

expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
timing = 'binding'

import ctypes
import binding_identity as actual
import binding_identity_control as ordinary
from soac import _soac_ext

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = arguments
    function.restype = result
    return function

obj = ctypes.py_object
owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
seal = api("PyFunction_GetSoacStrictId", ctypes.c_uint64, obj)
get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
assert _soac_ext.strict_module_diagnostics(actual)["sealed"]
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert owner(actual.plain) and not owner(ordinary.plain)
assert _soac_ext.strict_function_entry_kind(actual.plain) == expected_entry

def exercise(function):
    # The existing unknown-decorator fixture is deliberately dynamic. This
    # must not weaken the same-code rejection for metadata-sealed functions.
    assert seal(function) == 0
    code = function.__code__
    original_owner = owner(function)
    original_entry = get_vectorcall(function)
    original_defaults = function.__kwdefaults__
    marker = object()
    events = []
    parameter_name = code.co_varnames[0]

    class SameCodeKey:
        def __hash__(self):
            return hash(parameter_name)
        def __eq__(self, other):
            events.append(other is parameter_name)
            function.__code__ = code
            return other == parameter_name

    try:
        if timing == "binding":
            function.__kwdefaults__ = {SameCodeKey(): marker}
            assert function() is marker
            assert events == [True], events
        else:
            function.__code__ = code
        assert function.__code__ is code and owner(function) == original_owner
        assert get_vectorcall(function) == original_entry
        # The active binder must finish once, and the next invocation must
        # preserve source authority after the public same-code assignment.
        assert function(value=marker) is marker
        assert events == ([True] if timing == "binding" else []), events
    finally:
        function.__kwdefaults__ = original_defaults
    assert function() == 1

exercise(ordinary.plain)
exercise(actual.plain)
