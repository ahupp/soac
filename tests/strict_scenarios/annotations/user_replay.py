# modes:soac,entry
# module:user_annotations
# soac: module(strict_assign=true, checked_attr=true)
from annotationlib import Format

def annotate(format, /, __Format=Format, __Unsupported=NotImplementedError):
    if format == __Format.VALUE:
        return {'x': str}
    if format == __Format.VALUE_WITH_FAKE_GLOBALS:
        return {'x': int}
    raise __Unsupported(format)

def make_callback(local):
    def callback(format, /, __Format=Format, __Unsupported=NotImplementedError):
        if format == __Format.VALUE:
            return {'x': local}
        if format == __Format.VALUE_WITH_FAKE_GLOBALS:
            return {'x': (lambda: local)()}
        raise __Unsupported(format)
    return callback

def checked_callback(format: int, /, __Format=Format, __Unsupported=NotImplementedError):
    if format == __Format.VALUE:
        return {'x': str}
    if format == __Format.VALUE_WITH_FAKE_GLOBALS:
        return {'x': int}
    raise __Unsupported(format)

def nested_checked_callback(format, /):
    def checked(value: int) -> int:
        return value
    return {'x': checked}

def nested_class_callback(format, /):
    class Local:
        value: int
    return {'x': Local}
# module:user_annotations_control
from annotationlib import Format

def annotate(format, /, __Format=Format, __Unsupported=NotImplementedError):
    if format == __Format.VALUE:
        return {'x': str}
    if format == __Format.VALUE_WITH_FAKE_GLOBALS:
        return {'x': int}
    raise __Unsupported(format)

def make_callback(local):
    def callback(format, /, __Format=Format, __Unsupported=NotImplementedError):
        if format == __Format.VALUE:
            return {'x': local}
        if format == __Format.VALUE_WITH_FAKE_GLOBALS:
            return {'x': (lambda: local)()}
        raise __Unsupported(format)
    return callback

def checked_callback(format: int, /, __Format=Format, __Unsupported=NotImplementedError):
    if format == __Format.VALUE:
        return {'x': str}
    if format == __Format.VALUE_WITH_FAKE_GLOBALS:
        return {'x': int}
    raise __Unsupported(format)

def nested_checked_callback(format, /):
    def checked(value: int) -> int:
        return value
    return {'x': checked}

def nested_class_callback(format, /):
    class Local:
        value: int
    return {'x': Local}
# ok
# test_user_annotation_callback_replay_uses_ordinary_fake_globals [default]
import sys
from soac import _soac_ext
import annotationlib
import user_annotations as subject
import user_annotations_control as control

expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
for selected, ordinary in (
    (subject.annotate, control.annotate),
    (subject.checked_callback, control.checked_callback),
    (subject.make_callback(int), control.make_callback(int)),
    (subject.make_callback(str), control.make_callback(str)),
):
    assert _soac_ext.strict_function_entry_kind(selected) == expected_entry
    assert _soac_ext.strict_function_entry_kind(ordinary) is None
    for format in (
        annotationlib.Format.VALUE,
        annotationlib.Format.FORWARDREF,
        annotationlib.Format.STRING,
    ):
        expected = annotationlib.call_annotate_function(ordinary, format)
        assert annotationlib.call_annotate_function(selected, format) == expected
    assert _soac_ext.strict_function_entry_kind(selected) == expected_entry
assert annotationlib.call_annotate_function(
    subject.annotate, annotationlib.Format.STRING
) == {'x': 'int'}
# ok
# test_user_annotation_replay_tree_does_not_receive_source_or_jit_authority [default]
import sys
from soac import _soac_ext
import ctypes, types, _typing
import user_annotations as subject

source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
source_id.argtypes = [ctypes.py_object]
source_id.restype = ctypes.c_uint64
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p

for callback in (subject.annotate, subject.make_callback(int),
                 subject.checked_callback, subject.nested_checked_callback):
    original = callback.__code__
    replay = _typing._soac_annotation_replay_code(callback, None, 4)
    pending = [replay]
    while pending:
        code = pending.pop()
        assert source_id(code) == 0
        assert not code.co_flags & 0x10000000
        pending.extend(item for item in code.co_consts if type(item) is types.CodeType)
    copied = types.FunctionType(
        replay, {'__builtins__': callback.__builtins__, 'int': bytes},
        argdefs=callback.__defaults__, closure=callback.__closure__,
        kwdefaults=callback.__kwdefaults__,
    )
    assert owner(copied) is None
    assert metadata(copied) is None
    assert _soac_ext.strict_function_entry_kind(copied) is None
    if callback is subject.nested_checked_callback:
        nested = copied(2)['x']
        assert type(nested) is types.FunctionType
        assert owner(nested) is None and metadata(nested) is None
        assert source_id(nested.__code__) == 0
        assert _soac_ext.strict_function_entry_kind(nested) is None
        marker = object()
        assert nested(marker) is marker
    else:
        expected = bytes if callback in (subject.annotate, subject.checked_callback) else int
        assert copied(2) == {'x': expected}
    assert callback.__code__ is original
    assert source_id(original) != 0
    assert original.co_flags & 0x10000000
    assert owner(callback) is not None
    assert metadata(callback) is not None
# ok
# test_user_annotation_replay_cannot_copy_class_contracts_or_unowned_code [default]
import sys
from soac import _soac_ext
import annotationlib, marshal, types, _typing
import user_annotations as subject
import user_annotations_control as control
from soac.strict import StrictRuntimeUnavailableError

# A CodeType-only replay result cannot enforce a selected class
# contract. Function annotations alone no longer cause this refusal.
assert annotationlib.call_annotate_function(
    control.checked_callback, annotationlib.Format.STRING
) == {'x': 'int'}
for function in (
    subject.nested_class_callback,
):
    try:
        _typing._soac_annotation_replay_code(function, None, 4)
    except StrictRuntimeUnavailableError:
        pass
    else:
        raise AssertionError('replay stripped a required source contract')

for code in (
    subject.annotate.__code__,
    subject.annotate.__code__.replace(),
    marshal.loads(marshal.dumps(subject.annotate.__code__)),
):
    copied = types.FunctionType(
        code, subject.annotate.__globals__, argdefs=subject.annotate.__defaults__
    )
    for operation in (
        lambda: copied(2),
        lambda: _typing._soac_annotation_replay_code(copied, None, 4),
    ):
        try:
            operation()
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError('a copied function acquired source replay authority')
