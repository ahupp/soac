# modes:entry
# Authenticated source and independent ordinary validation blocks.
# module:framework_methods
# soac: module(strict_assign=true, checked_attr=true)
from framework_probe import Meta, instrument

class Managed(metaclass=Meta):
    def method(self, value: int) -> int:
        return value

@instrument
class Decorated:
    def method(self, value: int) -> int:
        return value

def independent(value: int) -> int:
    return value
# module:framework_probe
def replacement(self, value):
    return ("framework", value)

def instrument(cls):
    vars(cls)["method"].__code__ = replacement.__code__
    return cls

class Meta(type):
    def __new__(metaclass, name, bases, namespace):
        namespace["method"].__code__ = replacement.__code__
        return super().__new__(metaclass, name, bases, namespace)
# ok
# tests/test_strict_function_boundaries.py::test_statically_dynamic_framework_methods_keep_ordinary_annotations
import sys
from soac import _soac_ext, import_hook

import framework_methods as module

# These source classes were already classified as dynamic before any method
# object existed. Framework instrumentation preserves ordinary code mutation.
for cls in (module.Managed, module.Decorated):
    assert cls().method("not an integer") == ("framework", "not an integer")

assert module.independent(3) == 3
assert module.independent("bad") == "bad"
print("static-framework-boundaries")
