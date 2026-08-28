# modes:soac,entry
# module:annotated
# soac: module(strict_assign=true, checked_attr=true)
from typing import TYPE_CHECKING
from annotation_probe import remember, annotation_values
if TYPE_CHECKING:
    class Missing:
        pass
number: int
items: list[Missing]
format = str

def factory():
    class Local:
        pass
    def target(value: Local, other: format) -> list[Missing]:
        return []
    return target, Local

first, FirstLocal = factory()
second, SecondLocal = factory()

class Item:
    Alias = bytes
    remember(locals())
    item: Alias
    if format:
        reached: Alias
    def method(self, value: Alias):
        return value
    early = annotation_values(method)
# module:annotation_probe
prepared = []
def remember(namespace):
    prepared.append(namespace)
def annotation_values(function):
    return function.__annotate__(1)
# ok
# test_function_replay_preserves_real_lexical_cells_and_source_format_name [default]
import sys
from soac import _soac_ext
import annotationlib
import annotated

first = annotated.first.__annotate__
second = annotated.second.__annotate__
assert first.__code__ is second.__code__
assert first.__closure__ is not second.__closure__
assert first.__code__.co_freevars == ('Local',)
for function, expected in [(annotated.first, annotated.FirstLocal),
                           (annotated.second, annotated.SecondLocal)]:
    values = annotationlib.get_annotations(function, format=annotationlib.Format.FORWARDREF)
    assert values['value'] is expected
    assert values['other'] is str
    assert isinstance(values['return'].__args__[0], annotationlib.ForwardRef)
    assert annotationlib.get_annotations(function, format=annotationlib.Format.STRING) == {
        'value': 'Local', 'other': 'format', 'return': 'list[Missing]'
    }
    # The contextual owner is not an authorization token. This public
    # annotationlib entrypoint deliberately supports a missing owner.
    assert annotationlib.call_annotate_function(
        function.__annotate__, annotationlib.Format.FORWARDREF, owner=None
    )['value'] is expected
print('verified-function-annotation-cells')
# ok
# test_class_replay_uses_the_copied_dictionary_not_the_prepared_namespace [default]
import sys
from soac import _soac_ext
import annotationlib
import annotated
import annotation_probe

cls = annotated.Item
assert cls.early == {'value': bytes}
prepared, = annotation_probe.prepared
prepared['Alias'] = float
assert cls.Alias is bytes
assert cls.__annotate__.__code__.co_freevars == (
    '__classdict__', '__conditional_annotations__'
)
assert cls.method.__annotate__.__code__.co_freevars == ('__classdict__',)
for format in [annotationlib.Format.VALUE, annotationlib.Format.FORWARDREF]:
    assert annotationlib.get_annotations(cls, format=format) == {
        'item': bytes, 'reached': bytes
    }
    assert annotationlib.get_annotations(cls.method, format=format) == {'value': bytes}
assert annotationlib.get_annotations(cls, format=annotationlib.Format.STRING) == {
    'item': 'Alias', 'reached': 'Alias'
}
assert annotationlib.get_annotations(cls.method, format=annotationlib.Format.STRING) == {
    'value': 'Alias'
}
assert not hasattr(cls, '__classdictcell__')
print('verified-class-annotation-cell')
# ok
# test_nested_forwardref_uses_native_annotationlib_replay [default]
import sys
from soac import _soac_ext
import annotationlib, inspect, marshal, types, _typing
import annotated
from soac.strict import StrictRuntimeUnavailableError

provider = annotated.__annotate__
parameters = list(inspect.signature(provider).parameters.values())
assert [(item.name, item.kind, item.default) for item in parameters] == [
    ("format", inspect.Parameter.POSITIONAL_ONLY, inspect.Parameter.empty)
]
assert provider.__code__.co_flags & 0x10000000
result = annotationlib.get_annotations(
    annotated, format=annotationlib.Format.FORWARDREF
)
assert result["number"] is int
assert isinstance(result["items"], types.GenericAlias), result
assert result["items"].__origin__ is list
missing, = result["items"].__args__
assert isinstance(missing, annotationlib.ForwardRef), result
assert missing.__forward_arg__ == "Missing"
assert annotationlib.get_annotations(
    annotated, format=annotationlib.Format.STRING
) == {"number": "int", "items": "list[Missing]"}

for code in (provider.__code__, provider.__code__.replace(),
             marshal.loads(marshal.dumps(provider.__code__))):
    forged = types.FunctionType(code, annotated.__dict__)
    try:
        forged(2)
    except StrictRuntimeUnavailableError:
        pass
    else:
        raise AssertionError("replay authorized original strict bytecode")
    try:
        _typing._soac_annotation_replay_code(forged, None, 4)
    except StrictRuntimeUnavailableError:
        pass
    else:
        raise AssertionError("copied strict code gained replay provenance")
try:
    _typing._soac_annotation_replay_code(annotated.factory, None, 4)
except StrictRuntimeUnavailableError:
    pass
else:
    raise AssertionError("a source-function owner was accepted as an annotation provider")
print("verified-annotation-replay")
