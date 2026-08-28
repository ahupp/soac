# modes:soac,entry
# module:decorated_classes
# soac: module(strict_assign=true, checked_attr=true)
from typing import final

@final
# Native class providers start at the first decorator, not this header.
class Item:
    value: int = 1
    @final
    def method(self, number: int) -> int:
        return number

def factory():
    @final
    # The same projection must survive a real factory execution.
    class Local:
        value: int = 2
        @final
        def method(self, number: int) -> int:
            return number
    return Local

def identity(value):
    return value

@identity
# A generic wrapper must preserve the same class/provider start line.
class Generic[T]:
    value: T

def generic_factory():
    @identity
    class Local[T]:
        value: T
    return Local

@identity
def generic_function[T](value: T) -> T:
    return value

@identity
async def generic_async[T](value: T) -> T:
    return value
# ok
# test_decorated_class_and_method_providers_match_their_distinct_native_lines [default]
import sys
from soac import _soac_ext
import annotationlib, asyncio
import types
from pathlib import Path
import decorated_classes as subject

assert _soac_ext.strict_module_diagnostics(subject)['sealed']
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
assert _soac_ext.strict_function_entry_kind(subject.factory) == expected_entry
local = subject.factory()
# Native compilation is a control only: these code objects are never
# executed and never supply runtime admission or annotation targets.
root = compile(Path(subject.__file__).read_text(), subject.__file__, 'exec')
codes = []
def collect(code):
    codes.append(code)
    for constant in code.co_consts:
        if isinstance(constant, types.CodeType):
            collect(constant)
collect(root)
for cls in (subject.Item, local):
    class_code, = [code for code in codes if code.co_qualname == cls.__qualname__]
    native_class_provider, = [code for code in class_code.co_consts
                              if isinstance(code, types.CodeType)
                              and code.co_name == '__annotate__'
                              and code.co_firstlineno == class_code.co_firstlineno]
    provider = cls.__annotate__
    assert provider.__code__.co_firstlineno == native_class_provider.co_firstlineno
    assert provider.__code__.co_freevars == native_class_provider.co_freevars
    assert provider(1) == {'value': int}
    assert annotationlib.get_annotations(cls, format=annotationlib.Format.STRING) == {
        'value': 'int',
    }
    method = vars(cls)['method']
    assert _soac_ext.strict_function_entry_kind(method) == expected_entry
    assert annotationlib.get_annotations(method) == {'number': int, 'return': int}
    assert method.__annotate__.__code__.co_firstlineno > method.__code__.co_firstlineno
    assert cls().method(7) == 7
assert _soac_ext.strict_function_entry_kind(subject.generic_factory) == expected_entry
for cls in (subject.Generic, subject.generic_factory()):
    parameter, = cls.__type_params__
    class_code, = [code for code in codes if code.co_qualname == cls.__qualname__]
    native_provider, = [code for code in class_code.co_consts
                        if isinstance(code, types.CodeType)
                        and code.co_name == '__annotate__']
    provider = cls.__annotate__
    assert provider.__code__.co_firstlineno == native_provider.co_firstlineno
    assert provider.__code__.co_firstlineno == class_code.co_firstlineno
    assert provider.__code__.co_freevars == native_provider.co_freevars
    assert provider(1) == {'value': parameter}
    assert annotationlib.get_annotations(cls, format=annotationlib.Format.STRING) == {
        'value': 'T',
    }
assert _soac_ext.strict_function_entry_kind(subject.generic_function) == expected_entry
for function in (subject.generic_function, subject.generic_async):
    parameter, = function.__type_params__
    assert annotationlib.get_annotations(function) == {'value': parameter, 'return': parameter}
    assert function.__annotate__.__code__.co_firstlineno > function.__code__.co_firstlineno
value = object()
assert subject.generic_function(value) is value
assert _soac_ext.strict_function_entry_kind(subject.generic_async) == 'generator_factory'
assert asyncio.run(subject.generic_async(value)) is value
