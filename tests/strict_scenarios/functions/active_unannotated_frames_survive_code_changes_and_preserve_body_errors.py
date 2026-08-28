# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:active_frames
# soac: module(strict_assign=true, checked_attr=true)
from active_probe import dynamic, events, error, replace_body, replace_error

# Unknown decorators deliberately keep these functions outside frozen source
# contracts. Their authentic first calls still enter transformed source bodies.
@dynamic
def from_default(*, value=1):
    events.append("old default body")
    return ("old-default", value)

@dynamic
def from_body(value):
    replace_body(from_body)
    events.append("old body continued")
    return ("old-body", value)

@dynamic
def from_error():
    replace_error(from_error)
    raise error
# module:active_probe
events = []
error = LookupError("the original body exception")

def dynamic(function):
    return function

def new_default(*, value):
    return ("new-default", value)

def new_body(value):
    return ("new-body", value)

def new_error():
    return "new-error"

def replace_body(function):
    function.__code__ = new_body.__code__

def replace_error(function):
    function.__code__ = new_error.__code__

def install_changing_default(function):
    class Key:
        def __hash__(self):
            return hash("value")
        def __eq__(self, other):
            events.append("default callback")
            function.__code__ = new_default.__code__
            return other == "value"
    function.__kwdefaults__ = {Key(): 7}
# ok
# tests/test_strict_function_boundaries.py::test_active_unannotated_frames_survive_code_changes_and_preserve_body_errors
import sys
from soac import _soac_ext, import_hook

import active_frames as module
import active_probe as probe

probe.install_changing_default(module.from_default)
assert module.from_default() == ("old-default", 7)
assert probe.events == ["default callback", "old default body"], probe.events
assert module.from_default(value=8) == ("new-default", 8)

assert module.from_body(9) == ("old-body", 9)
assert probe.events[-1] == "old body continued"
assert module.from_body(10) == ("new-body", 10)

try:
    module.from_error()
except LookupError as error:
    assert error is probe.error
else:
    raise AssertionError("the original frame's exception disappeared")
assert module.from_error() == "new-error"
print("active-source-frame-survives-mutation")
