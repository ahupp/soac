# modes:soac,entry
# module:future_subject
# soac: module(strict_assign=true, checked_attr=true)

"""future annotation module"""
from __future__ import annotations
from typing import Literal
from future_probe import snapshot

start = snapshot(globals())
value: list[tuple[int, str]] = []
if False:
    absent: str

class Item:
    """future annotation class"""
    start = snapshot(locals())
    quoted: "Item"
    if True:
        selected: Literal["a", "b"]
    def method(self, item: "Item") -> "Item":
        return item

def shape(left: "Item", /, right: tuple["Item", ...]) -> list["Item"]:
    return []
# module:future_control
"""future annotation module"""
from __future__ import annotations
from typing import Literal
from future_probe import snapshot

start = snapshot(globals())
value: list[tuple[int, str]] = []
if False:
    absent: str

class Item:
    """future annotation class"""
    start = snapshot(locals())
    quoted: "Item"
    if True:
        selected: Literal["a", "b"]
    def method(self, item: "Item") -> "Item":
        return item

def shape(left: "Item", /, right: tuple["Item", ...]) -> list["Item"]:
    return []
# module:future_probe
def snapshot(namespace):
    return {
        "has_annotations": "__annotations__" in namespace,
        "annotations": dict(namespace.get("__annotations__", {})),
        "has_provider": "__annotate__" in namespace,
        "doc": namespace.get("__doc__"),
    }
# ok
# test_future_annotations_match_native_scope_timing_strings_and_provider_layout [default]
import sys
from soac import _soac_ext
import annotationlib, inspect
import future_control as control
import future_subject as subject

for candidate, ordinary in [(subject, control),
                            (subject.Item, control.Item)]:
    assert candidate.start == ordinary.start
    assert candidate.start["has_annotations"]
    assert candidate.start["annotations"] == {}
    assert not candidate.start["has_provider"]
    assert candidate.__annotate__ is ordinary.__annotate__ is None
    assert list(candidate.__annotations__.items()) == list(ordinary.__annotations__.items())
    for format in [annotationlib.Format.VALUE, annotationlib.Format.STRING,
                   annotationlib.Format.FORWARDREF]:
        assert annotationlib.get_annotations(candidate, format=format) == (
            annotationlib.get_annotations(ordinary, format=format)
        )

for candidate, ordinary in [(subject.shape, control.shape),
                            (subject.Item.method, control.Item.method)]:
    provider = candidate.__annotate__
    baseline = ordinary.__annotate__
    assert provider is not None and baseline is not None
    assert str(inspect.signature(provider)) == str(inspect.signature(baseline))
    assert provider.__code__.co_freevars == baseline.__code__.co_freevars == ()
    assert provider.__closure__ is baseline.__closure__ is None
    assert provider.__code__.co_flags & 0x10000000
    assert list(provider(1).items()) == list(baseline(1).items())
    for format in [annotationlib.Format.VALUE, annotationlib.Format.STRING,
                   annotationlib.Format.FORWARDREF]:
        assert annotationlib.get_annotations(candidate, format=format) == (
            annotationlib.get_annotations(ordinary, format=format)
        )
print("verified-future-annotation-native-protocol")
