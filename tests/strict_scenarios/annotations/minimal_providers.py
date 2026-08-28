# modes:soac,entry
# module:minimal
# soac: module(strict_assign=true, checked_attr=true)
number: int = 1
def identity(value: int) -> int:
    return value
# ok
# test_minimal_original_providers_keep_public_and_body_parameter_layouts [default]
import sys
from soac import _soac_ext
import annotationlib, inspect
import minimal

for owner, expected in [(minimal, {'number': int}),
                        (minimal.identity, {'value': int, 'return': int})]:
    provider = owner.__annotate__
    assert provider.__code__.co_flags & 0x10000000
    parameter, = inspect.signature(provider).parameters.values()
    assert parameter.name == 'format'
    assert parameter.kind is inspect.Parameter.POSITIONAL_ONLY
    assert parameter.default is inspect.Parameter.empty
    assert provider(1) == expected
    assert annotationlib.get_annotations(owner, format=annotationlib.Format.VALUE) == expected
    assert annotationlib.get_annotations(owner, format=annotationlib.Format.STRING) == {
        name: 'int' for name in expected
    }
assert minimal.identity(3) == 3
print('verified-minimal-original-providers')
