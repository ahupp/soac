# modes:soac
# Authenticated source and independent ordinary validation blocks.
# module:dynamic_function
# soac: module(strict_assign=true, checked_attr=true)
from dynamic_probe import replace

@replace
def dynamic(value):
    return ("source", value)
# module:dynamic_probe
def replacement(value):
    return ("replacement", value)

def later(value):
    return ("later", value)

def replace(function):
    function.__code__ = replacement.__code__
    return function
# ok
# tests/test_strict_function_boundaries.py::test_dynamic_function_code_replaced_by_decorator_is_not_adopted
import sys
from soac import _soac_ext, import_hook

import dynamic_function
import dynamic_probe

assert dynamic_function.dynamic(1) == ("replacement", 1)
dynamic_function.dynamic.__code__ = dynamic_probe.later.__code__
assert dynamic_function.dynamic(2) == ("later", 2)
print("dynamic-function-not-adopted")
