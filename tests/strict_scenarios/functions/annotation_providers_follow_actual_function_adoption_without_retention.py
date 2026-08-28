# modes:soac
# Authenticated source and independent ordinary validation blocks.
# module:owned_annotations
# soac: module(strict_assign=true, checked_attr=true)
from provider_probe import dynamic, retain_and_replace, replace_and_observe_release

def owned(value: int) -> int:
    return value

def replaced(value: int) -> int:
    return value

retain_and_replace(replaced)

def released(value: int) -> int:
    return value

replace_and_observe_release(released)

@dynamic
def unsupported(value: int) -> int:
    return value
# module:provider_probe
import weakref

events = []
retained = []
released_providers = []

def foreign(format):
    raise AssertionError("annotation provider must not be evaluated by adoption")

def replacement(format):
    return {}

def retain_and_replace(function):
    retained.append(function.__annotate__)
    function.__annotate__ = foreign

def replace_and_observe_release(function):
    reference = weakref.ref(function.__annotate__, lambda _: events.append("provider released"))
    released_providers.append(reference)
    function.__annotate__ = foreign
    events.append("after replacement")

def dynamic(function):
    # An unknown decorator cannot grant a frozen source contract, even when
    # this particular invocation returns its input unchanged.
    return function
# ok
# tests/test_strict_function_boundaries.py::test_annotation_providers_follow_actual_function_adoption_without_retention
import sys
from soac import _soac_ext, import_hook

import gc
import owned_annotations as module
import provider_probe as probe

gc.collect()
assert all(reference() is None for reference in probe.released_providers)
assert sorted(probe.events) == ["after replacement", "provider released"], probe.events
assert module.owned(1) == 1
assert module.replaced(2) == 2
assert module.released(3) == 3

owned_provider = module.owned.__annotate__
try:
    owned_provider.__code__ = owned_provider.__code__
except TypeError:
    pass
else:
    raise AssertionError("the actually owned provider was not sealed with its function")

# None of these providers belongs to an adopted target now. Source helper
# provenance, a retained old pointer, or use as a replacement is not enough.
for provider in [probe.foreign, probe.retained[0], module.unsupported.__annotate__]:
    provider.__code__ = probe.replacement.__code__
    assert provider(1) == {}
assert module.replaced.__annotate__ is probe.foreign
assert module.released.__annotate__ is probe.foreign
print("annotation-provider-adoption")
