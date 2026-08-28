# modes:cpython
# module:ordinary_class_frame_method_forwards_captured_cell_to_nested_functions
def build(saved):
    class Box:
        def read(self):
            def nested():
                return saved
            return (lambda: saved), nested
    return Box
# module:ordinary_class_frame_plain_target
def build(marker):
    class Box:
        values = [item for item in (marker,)]
    return Box
# module:ordinary_class_frame_captured_target
def build(marker):
    class Box:
        values = [lambda: item for item in (marker,)]
    return Box
# module:ordinary_class_frame_class_cell
def build(marker):
    class Box:
        values = [lambda: __class__ for __class__ in (marker,)]
        def read(self):
            return __class__
    return Box
# module:ordinary_class_frame_class_dictionary_cell
def build(marker):
    class Box:
        values = [lambda: __classdict__ for __classdict__ in (marker,)]
        field: int
    return Box
# module:ordinary_class_frame_conditional_annotation_cell
def build(marker, condition):
    class Box:
        values = [
            lambda: __conditional_annotations__
            for __conditional_annotations__ in (marker,)
        ]
        if condition:
            field: int
    return Box
# module:ordinary_class_frame_shadowed_lexical_free
def build(marker):
    outside = marker
    class Box:
        def read(self):
            return outside
        values = [lambda: outside for outside in (7, 8)]
    return Box
# ok
# test_class_frame_comprehension_cells_native_control[method_forwards_captured_cell_to_nested_functions]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ordinary_class_frame_method_forwards_captured_cell_to_nested_functions as module

import ctypes

def closure_cell(function, name):
    names = function.__code__.co_freevars
    assert names.count(name) == 1, (function, names, name)
    return function.__closure__[names.index(name)]

def check_class_owner(cls):
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(cls) == int(__dp_integration_soac__)

def check_function_owner(function, *, interpreted=False):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    if __dp_integration_soac__:
        from soac import _soac_ext
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) and metadata(function)
        expected = (
            'entry_interpreter'
            if interpreted or __dp_integration_entry__
            else 'checked_native'
        )
        actual = _soac_ext.strict_function_entry_kind(function)
        assert actual == expected, (function.__qualname__, actual, expected)
    else:
        assert not owner(function)
marker = object()
cls = module.build(marker)
check_class_owner(cls)
method = vars(cls)['read']
check_function_owner(method)
source_cell = closure_cell(method, 'saved')
assert source_cell.cell_contents is marker
functions = cls().read()
assert len(functions) == 2
for function in functions:
    check_function_owner(function)
    assert closure_cell(function, 'saved') is source_cell
    assert function() is marker
replacement = object()
source_cell.cell_contents = replacement
for function in (*functions, *cls().read()):
    check_function_owner(function)
    assert closure_cell(function, 'saved') is source_cell
    assert function() is replacement
# ok
# test_class_frame_comprehension_cells_native_control[plain_target]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ordinary_class_frame_plain_target as module

import ctypes

def closure_cell(function, name):
    names = function.__code__.co_freevars
    assert names.count(name) == 1, (function, names, name)
    return function.__closure__[names.index(name)]

def check_class_owner(cls):
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(cls) == int(__dp_integration_soac__)

def check_function_owner(function, *, interpreted=False):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    if __dp_integration_soac__:
        from soac import _soac_ext
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) and metadata(function)
        expected = (
            'entry_interpreter'
            if interpreted or __dp_integration_entry__
            else 'checked_native'
        )
        actual = _soac_ext.strict_function_entry_kind(function)
        assert actual == expected, (function.__qualname__, actual, expected)
    else:
        assert not owner(function)
marker = object()
cls = module.build(marker)
check_class_owner(cls)
assert cls.values == [marker]
assert 'item' not in vars(cls)
# ok
# test_class_frame_comprehension_cells_native_control[captured_target]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ordinary_class_frame_captured_target as module

import ctypes

def closure_cell(function, name):
    names = function.__code__.co_freevars
    assert names.count(name) == 1, (function, names, name)
    return function.__closure__[names.index(name)]

def check_class_owner(cls):
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(cls) == int(__dp_integration_soac__)

def check_function_owner(function, *, interpreted=False):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    if __dp_integration_soac__:
        from soac import _soac_ext
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) and metadata(function)
        expected = (
            'entry_interpreter'
            if interpreted or __dp_integration_entry__
            else 'checked_native'
        )
        actual = _soac_ext.strict_function_entry_kind(function)
        assert actual == expected, (function.__qualname__, actual, expected)
    else:
        assert not owner(function)
marker = object()
cls = module.build(marker)
check_class_owner(cls)
function = cls.values[0]
check_function_owner(function)
assert function() is marker
assert closure_cell(function, 'item').cell_contents is marker
assert 'item' not in vars(cls)
# ok
# test_class_frame_comprehension_cells_native_control[class_cell]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ordinary_class_frame_class_cell as module

import ctypes

def closure_cell(function, name):
    names = function.__code__.co_freevars
    assert names.count(name) == 1, (function, names, name)
    return function.__closure__[names.index(name)]

def check_class_owner(cls):
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(cls) == int(__dp_integration_soac__)

def check_function_owner(function, *, interpreted=False):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    if __dp_integration_soac__:
        from soac import _soac_ext
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) and metadata(function)
        expected = (
            'entry_interpreter'
            if interpreted or __dp_integration_entry__
            else 'checked_native'
        )
        actual = _soac_ext.strict_function_entry_kind(function)
        assert actual == expected, (function.__qualname__, actual, expected)
    else:
        assert not owner(function)
marker = object()
cls = module.build(marker)
check_class_owner(cls)
transient = cls.values[0]
method = vars(cls)['read']
check_function_owner(transient)
check_function_owner(method)
assert transient() is marker
assert cls().read() is cls
transient_cell = closure_cell(transient, '__class__')
source_cell = closure_cell(method, '__class__')
assert transient_cell is not source_cell
assert transient_cell.cell_contents is marker
assert source_cell.cell_contents is cls
# ok
# test_class_frame_comprehension_cells_native_control[class_dictionary_cell]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ordinary_class_frame_class_dictionary_cell as module

import ctypes

def closure_cell(function, name):
    names = function.__code__.co_freevars
    assert names.count(name) == 1, (function, names, name)
    return function.__closure__[names.index(name)]

def check_class_owner(cls):
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(cls) == int(__dp_integration_soac__)

def check_function_owner(function, *, interpreted=False):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    if __dp_integration_soac__:
        from soac import _soac_ext
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) and metadata(function)
        expected = (
            'entry_interpreter'
            if interpreted or __dp_integration_entry__
            else 'checked_native'
        )
        actual = _soac_ext.strict_function_entry_kind(function)
        assert actual == expected, (function.__qualname__, actual, expected)
    else:
        assert not owner(function)
marker = object()
cls = module.build(marker)
check_class_owner(cls)
transient = cls.values[0]
provider = vars(cls)['__annotate_func__']
check_function_owner(transient)
check_function_owner(provider, interpreted=True)
assert transient() is marker
transient_cell = closure_cell(transient, '__classdict__')
source_cell = closure_cell(provider, '__classdict__')
assert transient_cell is not source_cell
assert transient_cell.cell_contents is marker
try:
    source_cell.cell_contents
except ValueError:
    pass
else:
    raise AssertionError('original hidden class dictionary cell is not empty')
# This is the same cell later read by the public native provider, not a
# permanently empty traceback-only replacement or the transient target.
source_cell.cell_contents = {'int': str}
assert provider(1) == {'field': str}
del source_cell.cell_contents
try:
    provider(1)
except NameError:
    pass
else:
    raise AssertionError('annotation provider lost its original cell')
# ok
# test_class_frame_comprehension_cells_native_control[conditional_annotation_cell]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ordinary_class_frame_conditional_annotation_cell as module

import ctypes

def closure_cell(function, name):
    names = function.__code__.co_freevars
    assert names.count(name) == 1, (function, names, name)
    return function.__closure__[names.index(name)]

def check_class_owner(cls):
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(cls) == int(__dp_integration_soac__)

def check_function_owner(function, *, interpreted=False):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    if __dp_integration_soac__:
        from soac import _soac_ext
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) and metadata(function)
        expected = (
            'entry_interpreter'
            if interpreted or __dp_integration_entry__
            else 'checked_native'
        )
        actual = _soac_ext.strict_function_entry_kind(function)
        assert actual == expected, (function.__qualname__, actual, expected)
    else:
        assert not owner(function)
marker = object()
cls = module.build(marker, True)
check_class_owner(cls)
transient = cls.values[0]
provider = vars(cls)['__annotate_func__']
check_function_owner(transient)
check_function_owner(provider, interpreted=True)
assert transient() is marker
transient_cell = closure_cell(transient, '__conditional_annotations__')
source_cell = closure_cell(provider, '__conditional_annotations__')
assert transient_cell is not source_cell
assert transient_cell.cell_contents is marker
indices = source_cell.cell_contents
assert type(indices) is set and len(indices) == 1
assert provider(1) == {'field': int}
saved = indices.copy()
indices.clear()
assert provider(1) == {}
indices.update(saved)
assert source_cell.cell_contents is indices
assert provider(1) == {'field': int}
# ok
# test_class_frame_comprehension_cells_native_control[shadowed_lexical_free]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ordinary_class_frame_shadowed_lexical_free as module

import ctypes

def closure_cell(function, name):
    names = function.__code__.co_freevars
    assert names.count(name) == 1, (function, names, name)
    return function.__closure__[names.index(name)]

def check_class_owner(cls):
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(cls) == int(__dp_integration_soac__)

def check_function_owner(function, *, interpreted=False):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    if __dp_integration_soac__:
        from soac import _soac_ext
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) and metadata(function)
        expected = (
            'entry_interpreter'
            if interpreted or __dp_integration_entry__
            else 'checked_native'
        )
        actual = _soac_ext.strict_function_entry_kind(function)
        assert actual == expected, (function.__qualname__, actual, expected)
    else:
        assert not owner(function)
marker = object()
cls = module.build(marker)
check_class_owner(cls)
method = vars(cls)['read']
check_function_owner(method)
for function in cls.values:
    check_function_owner(function)
    assert function() == 8
source_cell = closure_cell(method, 'outside')
transient_cell = closure_cell(cls.values[0], 'outside')
assert source_cell is not transient_cell
# The selected native compiler restores an empty class-owned cell for
# this same-spelling CELL/FREE collision. The source method captures that
# cell, rather than the separate outer lexical owner.
try:
    source_cell.cell_contents
except ValueError:
    pass
else:
    raise AssertionError('native class-owned restored cell is not empty')
assert transient_cell.cell_contents == 8
try:
    cls().read()
except NameError:
    pass
else:
    raise AssertionError('method did not retain the native empty-cell binding')
assert 'outside' not in vars(cls)
