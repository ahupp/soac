# modes:cpython
# module:native_lifecycle_set_cleanup
# soac: module(strict_assign=true, checked_attr=true)
def build(source):
    class C:
        result = {item for item in source()}
    return C
# module:ordinary_lifecycle_set_cleanup
def build(source):
    class C:
        result = {item for item in source()}
    return C
# module:native_lifecycle_dict_cleanup
# soac: module(strict_assign=true, checked_attr=true)
def build(source):
    class C:
        result = {item: item for item in source()}
    return C
# module:ordinary_lifecycle_dict_cleanup
def build(source):
    class C:
        result = {item: item for item in source()}
    return C
# module:native_lifecycle_conditional_equal_name
# soac: module(strict_assign=true, checked_attr=true)
def factory(value, enabled):
    class C:
        result = [([lambda: value for value in (7,)] if enabled else None, value) for unused in (0,)]
    return C
# module:ordinary_lifecycle_conditional_equal_name
def factory(value, enabled):
    class C:
        result = [([lambda: value for value in (7,)] if enabled else None, value) for unused in (0,)]
    return C
# module:native_lifecycle_namespace_delete_capture
# soac: module(strict_assign=true, checked_attr=true)
def factory(value):
    class C:
        value = 'namespace'
        seen = value
        callbacks = [lambda: value for unused in (0,)]
        del value
    return C
# module:ordinary_lifecycle_namespace_delete_capture
def factory(value):
    class C:
        value = 'namespace'
        seen = value
        callbacks = [lambda: value for unused in (0,)]
        del value
    return C
# module:native_lifecycle_finally_completion
# soac: module(strict_assign=true, checked_attr=true)
def build(checkpoint):
    class C:
        try:
            checkpoint()
        finally:
            callbacks = [lambda: item for item in (1,)]
    return C
# module:ordinary_lifecycle_finally_completion
def build(checkpoint):
    class C:
        try:
            checkpoint()
        finally:
            callbacks = [lambda: item for item in (1,)]
    return C
# module:native_lifecycle_pre_region_raise
# soac: module(strict_assign=true, checked_attr=true)
def build(source):
    class C:
        raise ValueError('before region')
        ignored = [lambda: item for item in source()]
    return C
# module:ordinary_lifecycle_pre_region_raise
def build(source):
    class C:
        raise ValueError('before region')
        ignored = [lambda: item for item in source()]
    return C
# ok
# test_cpython_class_lifecycle_distinct_behavior_matches_ordinary[set_cleanup]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
import ctypes
import importlib
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

def check_class_owner(cls):
    assert __dp_integration_mode__ == 'cpython'
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(cls) and info.owner is not None
    assert sealed(cls) == 1

def check_function_owner(function, *, interpreted=False):
    module = importlib.import_module('native_lifecycle_set_cleanup')
    diagnostic = _assert_cpython_function_witness(
        function, _soac_ext.strict_module_diagnostics(module),
    )
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(function)
    assert _soac_ext.strict_function_entry_kind(function) == 'original_code'
import ctypes
import gc
import sys
import weakref

def lifecycle_class(cls, native):
    if native:
        check_class_owner(cls)
    else:
        owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(cls) is None

def lifecycle_function(function, native):
    if native:
        check_function_owner(function)
    else:
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(function) is None

def observe_collection(build, kind, native=False):
    outcomes = []
    for outcome in ('success', 'source-error', 'next-error', 'hash-error'):
        events = []
        refs = {}
        marker = LookupError(outcome)

        def handled():
            error = sys.exception()
            return None if error is None else str(error.args[0])

        def live():
            return tuple(bool(refs.get(name) and refs[name]() is not None)
                         for name in ('item', 'iterator'))

        class Item:
            def __hash__(self):
                events.append(('hash', handled()))
                if outcome == 'hash-error':
                    raise marker
                return 7
            def __del__(self):
                events.append(('drop-item', handled(), live()))

        class Iterator:
            def __init__(self):
                self.started = False
                refs['iterator'] = weakref.ref(self)
            def __iter__(self):
                events.append(('iter', handled()))
                return self
            def __next__(self):
                if not self.started:
                    self.started = True
                    item = Item()
                    refs['item'] = weakref.ref(item)
                    events.append(('made-item', handled()))
                    return item
                if outcome == 'next-error':
                    raise marker
                raise StopIteration
            def __del__(self):
                events.append(('drop-iterator', handled(), live()))

        def source():
            events.append(('source', handled()))
            if outcome == 'source-error':
                raise marker
            return Iterator()

        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(source)
            except LookupError as error:
                assert outcome != 'success' and error is marker
                assert error.__context__ is caller
                events.append(('caught', handled(), live()))
                error.__traceback__ = None
                events.append(('traceback-cleared', handled(), live()))
            else:
                assert outcome == 'success'
                lifecycle_class(cls, native)
                result = vars(cls)['result']
                assert type(result) is (set if kind == 'set' else dict)
                assert len(result) == 1
                item = refs['item']()
                assert item is not None and item in result
                if kind == 'dict':
                    assert result[item] is item
                del item
                result.clear()
                events.append(('cleared', handled(), live()))
                del result, cls
            assert sys.exception() is caller
            events.append(('after-call', handled(), live()))
        gc.collect()
        assert live() == (False, False)
        events.append(('after-handler', handled(), live()))
        outcomes.append((outcome, events))
    return outcomes

def observe_conditional(build, native=False):
    marker = object()
    observed = []
    for enabled in (False, True):
        cls = build(marker, enabled)
        lifecycle_class(cls, native)
        assert len(cls.result) == 1
        callbacks, value = cls.result[0]
        # The pinned CPython emitter saves/restores the hidden slot but writes
        # the distinct FREE slot in the enabled inner comprehension. Preserve
        # that ordinary 3.15.0a5 behavior, not an assumed SOAC lifetime recipe.
        if enabled:
            assert type(value) is int and value == 7
        else:
            assert value is marker
        assert 'value' not in vars(cls) and 'unused' not in vars(cls)
        if enabled:
            assert len(callbacks) == 1
            callback = callbacks[0]
            lifecycle_function(callback, native)
            assert callback() == 7
            assert closure_cell(callback, 'value').cell_contents == 7
        else:
            assert callbacks is None
        observed.append((
            enabled,
            value is marker,
            None if value is marker else (type(value).__name__, value),
            None if callbacks is None else callbacks[0](),
        ))
    return observed

def observe_namespace(build, native=False):
    marker = object()
    cls = build(marker)
    lifecycle_class(cls, native)
    assert cls.seen == 'namespace' and 'value' not in vars(cls)
    assert 'unused' not in vars(cls)
    callback, = cls.callbacks
    lifecycle_function(callback, native)
    assert callback() is marker
    cell = closure_cell(callback, 'value')
    assert cell.cell_contents is marker
    replacement = object()
    cell.cell_contents = replacement
    assert callback() is replacement
    assert cls.seen == 'namespace'
    return ('namespace', callback() is replacement, 'value' in vars(cls))

def class_error_callback(error):
    traceback = error.__traceback__
    while traceback is not None:
        namespace = traceback.tb_frame.f_locals
        if 'callbacks' in namespace:
            callback, = namespace['callbacks']
            return callback
        traceback = traceback.tb_next
    raise AssertionError('original finally suite did not publish its callback')

def observe_finally(build, native=False):
    observed = []
    for fails in (False, True):
        events = []
        marker = ValueError('checkpoint')
        def checkpoint():
            events.append('checkpoint')
            if fails:
                raise marker
        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(checkpoint)
            except ValueError as error:
                assert fails and error is marker and error.__context__ is caller
                callback = class_error_callback(error)
                lifecycle_function(callback, native)
                assert callback() == 1
                reference = weakref.ref(callback)
                del callback
                error.__traceback__ = None
                gc.collect()
                assert reference() is None
                events.append('finally-error')
            else:
                assert not fails
                lifecycle_class(cls, native)
                callback, = cls.callbacks
                lifecycle_function(callback, native)
                assert callback() == 1
                events.append('finally-success')
                del callback, cls
            assert sys.exception() is caller
        observed.append((fails, events))
    return observed

def observe_pre_region_raise(build, native=False):
    events = []
    def source():
        events.append('unreachable-iterable')
        raise AssertionError('unreachable region evaluated its source')
    try:
        raise KeyError('caller')
    except KeyError as caller:
        try:
            build(source)
        except ValueError as error:
            assert error.args == ('before region',)
            assert error.__context__ is caller
            assert sys.exception() is error
            error.__traceback__ = None
        else:
            raise AssertionError('original class failure was swallowed')
        assert sys.exception() is caller
    assert events == []
    return ('before region', events)
import native_lifecycle_set_cleanup as module
import native_lifecycle_set_cleanup as actual
import ordinary_lifecycle_set_cleanup as ordinary
assert _soac_ext.strict_module_diagnostics(ordinary) is None
build = getattr(ordinary, 'build')
expected = observe_collection(build, 'set')
build = getattr(actual, 'build')
observed = observe_collection(build, 'set', native=True)
assert observed == expected, (observed, expected)
assert _soac_ext.strict_function_diagnostics(build)['original_code_entered']
# ok
# test_cpython_class_lifecycle_distinct_behavior_matches_ordinary[dict_cleanup]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
import ctypes
import importlib
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

def check_class_owner(cls):
    assert __dp_integration_mode__ == 'cpython'
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(cls) and info.owner is not None
    assert sealed(cls) == 1

def check_function_owner(function, *, interpreted=False):
    module = importlib.import_module('native_lifecycle_dict_cleanup')
    diagnostic = _assert_cpython_function_witness(
        function, _soac_ext.strict_module_diagnostics(module),
    )
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(function)
    assert _soac_ext.strict_function_entry_kind(function) == 'original_code'
import ctypes
import gc
import sys
import weakref

def lifecycle_class(cls, native):
    if native:
        check_class_owner(cls)
    else:
        owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(cls) is None

def lifecycle_function(function, native):
    if native:
        check_function_owner(function)
    else:
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(function) is None

def observe_collection(build, kind, native=False):
    outcomes = []
    for outcome in ('success', 'source-error', 'next-error', 'hash-error'):
        events = []
        refs = {}
        marker = LookupError(outcome)

        def handled():
            error = sys.exception()
            return None if error is None else str(error.args[0])

        def live():
            return tuple(bool(refs.get(name) and refs[name]() is not None)
                         for name in ('item', 'iterator'))

        class Item:
            def __hash__(self):
                events.append(('hash', handled()))
                if outcome == 'hash-error':
                    raise marker
                return 7
            def __del__(self):
                events.append(('drop-item', handled(), live()))

        class Iterator:
            def __init__(self):
                self.started = False
                refs['iterator'] = weakref.ref(self)
            def __iter__(self):
                events.append(('iter', handled()))
                return self
            def __next__(self):
                if not self.started:
                    self.started = True
                    item = Item()
                    refs['item'] = weakref.ref(item)
                    events.append(('made-item', handled()))
                    return item
                if outcome == 'next-error':
                    raise marker
                raise StopIteration
            def __del__(self):
                events.append(('drop-iterator', handled(), live()))

        def source():
            events.append(('source', handled()))
            if outcome == 'source-error':
                raise marker
            return Iterator()

        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(source)
            except LookupError as error:
                assert outcome != 'success' and error is marker
                assert error.__context__ is caller
                events.append(('caught', handled(), live()))
                error.__traceback__ = None
                events.append(('traceback-cleared', handled(), live()))
            else:
                assert outcome == 'success'
                lifecycle_class(cls, native)
                result = vars(cls)['result']
                assert type(result) is (set if kind == 'set' else dict)
                assert len(result) == 1
                item = refs['item']()
                assert item is not None and item in result
                if kind == 'dict':
                    assert result[item] is item
                del item
                result.clear()
                events.append(('cleared', handled(), live()))
                del result, cls
            assert sys.exception() is caller
            events.append(('after-call', handled(), live()))
        gc.collect()
        assert live() == (False, False)
        events.append(('after-handler', handled(), live()))
        outcomes.append((outcome, events))
    return outcomes

def observe_conditional(build, native=False):
    marker = object()
    observed = []
    for enabled in (False, True):
        cls = build(marker, enabled)
        lifecycle_class(cls, native)
        assert len(cls.result) == 1
        callbacks, value = cls.result[0]
        # The pinned CPython emitter saves/restores the hidden slot but writes
        # the distinct FREE slot in the enabled inner comprehension. Preserve
        # that ordinary 3.15.0a5 behavior, not an assumed SOAC lifetime recipe.
        if enabled:
            assert type(value) is int and value == 7
        else:
            assert value is marker
        assert 'value' not in vars(cls) and 'unused' not in vars(cls)
        if enabled:
            assert len(callbacks) == 1
            callback = callbacks[0]
            lifecycle_function(callback, native)
            assert callback() == 7
            assert closure_cell(callback, 'value').cell_contents == 7
        else:
            assert callbacks is None
        observed.append((
            enabled,
            value is marker,
            None if value is marker else (type(value).__name__, value),
            None if callbacks is None else callbacks[0](),
        ))
    return observed

def observe_namespace(build, native=False):
    marker = object()
    cls = build(marker)
    lifecycle_class(cls, native)
    assert cls.seen == 'namespace' and 'value' not in vars(cls)
    assert 'unused' not in vars(cls)
    callback, = cls.callbacks
    lifecycle_function(callback, native)
    assert callback() is marker
    cell = closure_cell(callback, 'value')
    assert cell.cell_contents is marker
    replacement = object()
    cell.cell_contents = replacement
    assert callback() is replacement
    assert cls.seen == 'namespace'
    return ('namespace', callback() is replacement, 'value' in vars(cls))

def class_error_callback(error):
    traceback = error.__traceback__
    while traceback is not None:
        namespace = traceback.tb_frame.f_locals
        if 'callbacks' in namespace:
            callback, = namespace['callbacks']
            return callback
        traceback = traceback.tb_next
    raise AssertionError('original finally suite did not publish its callback')

def observe_finally(build, native=False):
    observed = []
    for fails in (False, True):
        events = []
        marker = ValueError('checkpoint')
        def checkpoint():
            events.append('checkpoint')
            if fails:
                raise marker
        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(checkpoint)
            except ValueError as error:
                assert fails and error is marker and error.__context__ is caller
                callback = class_error_callback(error)
                lifecycle_function(callback, native)
                assert callback() == 1
                reference = weakref.ref(callback)
                del callback
                error.__traceback__ = None
                gc.collect()
                assert reference() is None
                events.append('finally-error')
            else:
                assert not fails
                lifecycle_class(cls, native)
                callback, = cls.callbacks
                lifecycle_function(callback, native)
                assert callback() == 1
                events.append('finally-success')
                del callback, cls
            assert sys.exception() is caller
        observed.append((fails, events))
    return observed

def observe_pre_region_raise(build, native=False):
    events = []
    def source():
        events.append('unreachable-iterable')
        raise AssertionError('unreachable region evaluated its source')
    try:
        raise KeyError('caller')
    except KeyError as caller:
        try:
            build(source)
        except ValueError as error:
            assert error.args == ('before region',)
            assert error.__context__ is caller
            assert sys.exception() is error
            error.__traceback__ = None
        else:
            raise AssertionError('original class failure was swallowed')
        assert sys.exception() is caller
    assert events == []
    return ('before region', events)
import native_lifecycle_dict_cleanup as module
import native_lifecycle_dict_cleanup as actual
import ordinary_lifecycle_dict_cleanup as ordinary
assert _soac_ext.strict_module_diagnostics(ordinary) is None
build = getattr(ordinary, 'build')
expected = observe_collection(build, 'dict')
build = getattr(actual, 'build')
observed = observe_collection(build, 'dict', native=True)
assert observed == expected, (observed, expected)
assert _soac_ext.strict_function_diagnostics(build)['original_code_entered']
# ok
# test_cpython_class_lifecycle_distinct_behavior_matches_ordinary[conditional_equal_name]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
import ctypes
import importlib
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

def check_class_owner(cls):
    assert __dp_integration_mode__ == 'cpython'
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(cls) and info.owner is not None
    assert sealed(cls) == 1

def check_function_owner(function, *, interpreted=False):
    module = importlib.import_module('native_lifecycle_conditional_equal_name')
    diagnostic = _assert_cpython_function_witness(
        function, _soac_ext.strict_module_diagnostics(module),
    )
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(function)
    assert _soac_ext.strict_function_entry_kind(function) == 'original_code'
import ctypes
import gc
import sys
import weakref

def lifecycle_class(cls, native):
    if native:
        check_class_owner(cls)
    else:
        owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(cls) is None

def lifecycle_function(function, native):
    if native:
        check_function_owner(function)
    else:
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(function) is None

def observe_collection(build, kind, native=False):
    outcomes = []
    for outcome in ('success', 'source-error', 'next-error', 'hash-error'):
        events = []
        refs = {}
        marker = LookupError(outcome)

        def handled():
            error = sys.exception()
            return None if error is None else str(error.args[0])

        def live():
            return tuple(bool(refs.get(name) and refs[name]() is not None)
                         for name in ('item', 'iterator'))

        class Item:
            def __hash__(self):
                events.append(('hash', handled()))
                if outcome == 'hash-error':
                    raise marker
                return 7
            def __del__(self):
                events.append(('drop-item', handled(), live()))

        class Iterator:
            def __init__(self):
                self.started = False
                refs['iterator'] = weakref.ref(self)
            def __iter__(self):
                events.append(('iter', handled()))
                return self
            def __next__(self):
                if not self.started:
                    self.started = True
                    item = Item()
                    refs['item'] = weakref.ref(item)
                    events.append(('made-item', handled()))
                    return item
                if outcome == 'next-error':
                    raise marker
                raise StopIteration
            def __del__(self):
                events.append(('drop-iterator', handled(), live()))

        def source():
            events.append(('source', handled()))
            if outcome == 'source-error':
                raise marker
            return Iterator()

        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(source)
            except LookupError as error:
                assert outcome != 'success' and error is marker
                assert error.__context__ is caller
                events.append(('caught', handled(), live()))
                error.__traceback__ = None
                events.append(('traceback-cleared', handled(), live()))
            else:
                assert outcome == 'success'
                lifecycle_class(cls, native)
                result = vars(cls)['result']
                assert type(result) is (set if kind == 'set' else dict)
                assert len(result) == 1
                item = refs['item']()
                assert item is not None and item in result
                if kind == 'dict':
                    assert result[item] is item
                del item
                result.clear()
                events.append(('cleared', handled(), live()))
                del result, cls
            assert sys.exception() is caller
            events.append(('after-call', handled(), live()))
        gc.collect()
        assert live() == (False, False)
        events.append(('after-handler', handled(), live()))
        outcomes.append((outcome, events))
    return outcomes

def observe_conditional(build, native=False):
    marker = object()
    observed = []
    for enabled in (False, True):
        cls = build(marker, enabled)
        lifecycle_class(cls, native)
        assert len(cls.result) == 1
        callbacks, value = cls.result[0]
        # The pinned CPython emitter saves/restores the hidden slot but writes
        # the distinct FREE slot in the enabled inner comprehension. Preserve
        # that ordinary 3.15.0a5 behavior, not an assumed SOAC lifetime recipe.
        if enabled:
            assert type(value) is int and value == 7
        else:
            assert value is marker
        assert 'value' not in vars(cls) and 'unused' not in vars(cls)
        if enabled:
            assert len(callbacks) == 1
            callback = callbacks[0]
            lifecycle_function(callback, native)
            assert callback() == 7
            assert closure_cell(callback, 'value').cell_contents == 7
        else:
            assert callbacks is None
        observed.append((
            enabled,
            value is marker,
            None if value is marker else (type(value).__name__, value),
            None if callbacks is None else callbacks[0](),
        ))
    return observed

def observe_namespace(build, native=False):
    marker = object()
    cls = build(marker)
    lifecycle_class(cls, native)
    assert cls.seen == 'namespace' and 'value' not in vars(cls)
    assert 'unused' not in vars(cls)
    callback, = cls.callbacks
    lifecycle_function(callback, native)
    assert callback() is marker
    cell = closure_cell(callback, 'value')
    assert cell.cell_contents is marker
    replacement = object()
    cell.cell_contents = replacement
    assert callback() is replacement
    assert cls.seen == 'namespace'
    return ('namespace', callback() is replacement, 'value' in vars(cls))

def class_error_callback(error):
    traceback = error.__traceback__
    while traceback is not None:
        namespace = traceback.tb_frame.f_locals
        if 'callbacks' in namespace:
            callback, = namespace['callbacks']
            return callback
        traceback = traceback.tb_next
    raise AssertionError('original finally suite did not publish its callback')

def observe_finally(build, native=False):
    observed = []
    for fails in (False, True):
        events = []
        marker = ValueError('checkpoint')
        def checkpoint():
            events.append('checkpoint')
            if fails:
                raise marker
        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(checkpoint)
            except ValueError as error:
                assert fails and error is marker and error.__context__ is caller
                callback = class_error_callback(error)
                lifecycle_function(callback, native)
                assert callback() == 1
                reference = weakref.ref(callback)
                del callback
                error.__traceback__ = None
                gc.collect()
                assert reference() is None
                events.append('finally-error')
            else:
                assert not fails
                lifecycle_class(cls, native)
                callback, = cls.callbacks
                lifecycle_function(callback, native)
                assert callback() == 1
                events.append('finally-success')
                del callback, cls
            assert sys.exception() is caller
        observed.append((fails, events))
    return observed

def observe_pre_region_raise(build, native=False):
    events = []
    def source():
        events.append('unreachable-iterable')
        raise AssertionError('unreachable region evaluated its source')
    try:
        raise KeyError('caller')
    except KeyError as caller:
        try:
            build(source)
        except ValueError as error:
            assert error.args == ('before region',)
            assert error.__context__ is caller
            assert sys.exception() is error
            error.__traceback__ = None
        else:
            raise AssertionError('original class failure was swallowed')
        assert sys.exception() is caller
    assert events == []
    return ('before region', events)
import native_lifecycle_conditional_equal_name as module
import native_lifecycle_conditional_equal_name as actual
import ordinary_lifecycle_conditional_equal_name as ordinary
assert _soac_ext.strict_module_diagnostics(ordinary) is None
build = getattr(ordinary, 'factory')
expected = observe_conditional(build)
build = getattr(actual, 'factory')
observed = observe_conditional(build, native=True)
assert observed == expected, (observed, expected)
assert _soac_ext.strict_function_diagnostics(build)['original_code_entered']
# ok
# test_cpython_class_lifecycle_distinct_behavior_matches_ordinary[namespace_delete_capture]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
import ctypes
import importlib
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

def check_class_owner(cls):
    assert __dp_integration_mode__ == 'cpython'
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(cls) and info.owner is not None
    assert sealed(cls) == 1

def check_function_owner(function, *, interpreted=False):
    module = importlib.import_module('native_lifecycle_namespace_delete_capture')
    diagnostic = _assert_cpython_function_witness(
        function, _soac_ext.strict_module_diagnostics(module),
    )
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(function)
    assert _soac_ext.strict_function_entry_kind(function) == 'original_code'
import ctypes
import gc
import sys
import weakref

def lifecycle_class(cls, native):
    if native:
        check_class_owner(cls)
    else:
        owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(cls) is None

def lifecycle_function(function, native):
    if native:
        check_function_owner(function)
    else:
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(function) is None

def observe_collection(build, kind, native=False):
    outcomes = []
    for outcome in ('success', 'source-error', 'next-error', 'hash-error'):
        events = []
        refs = {}
        marker = LookupError(outcome)

        def handled():
            error = sys.exception()
            return None if error is None else str(error.args[0])

        def live():
            return tuple(bool(refs.get(name) and refs[name]() is not None)
                         for name in ('item', 'iterator'))

        class Item:
            def __hash__(self):
                events.append(('hash', handled()))
                if outcome == 'hash-error':
                    raise marker
                return 7
            def __del__(self):
                events.append(('drop-item', handled(), live()))

        class Iterator:
            def __init__(self):
                self.started = False
                refs['iterator'] = weakref.ref(self)
            def __iter__(self):
                events.append(('iter', handled()))
                return self
            def __next__(self):
                if not self.started:
                    self.started = True
                    item = Item()
                    refs['item'] = weakref.ref(item)
                    events.append(('made-item', handled()))
                    return item
                if outcome == 'next-error':
                    raise marker
                raise StopIteration
            def __del__(self):
                events.append(('drop-iterator', handled(), live()))

        def source():
            events.append(('source', handled()))
            if outcome == 'source-error':
                raise marker
            return Iterator()

        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(source)
            except LookupError as error:
                assert outcome != 'success' and error is marker
                assert error.__context__ is caller
                events.append(('caught', handled(), live()))
                error.__traceback__ = None
                events.append(('traceback-cleared', handled(), live()))
            else:
                assert outcome == 'success'
                lifecycle_class(cls, native)
                result = vars(cls)['result']
                assert type(result) is (set if kind == 'set' else dict)
                assert len(result) == 1
                item = refs['item']()
                assert item is not None and item in result
                if kind == 'dict':
                    assert result[item] is item
                del item
                result.clear()
                events.append(('cleared', handled(), live()))
                del result, cls
            assert sys.exception() is caller
            events.append(('after-call', handled(), live()))
        gc.collect()
        assert live() == (False, False)
        events.append(('after-handler', handled(), live()))
        outcomes.append((outcome, events))
    return outcomes

def observe_conditional(build, native=False):
    marker = object()
    observed = []
    for enabled in (False, True):
        cls = build(marker, enabled)
        lifecycle_class(cls, native)
        assert len(cls.result) == 1
        callbacks, value = cls.result[0]
        # The pinned CPython emitter saves/restores the hidden slot but writes
        # the distinct FREE slot in the enabled inner comprehension. Preserve
        # that ordinary 3.15.0a5 behavior, not an assumed SOAC lifetime recipe.
        if enabled:
            assert type(value) is int and value == 7
        else:
            assert value is marker
        assert 'value' not in vars(cls) and 'unused' not in vars(cls)
        if enabled:
            assert len(callbacks) == 1
            callback = callbacks[0]
            lifecycle_function(callback, native)
            assert callback() == 7
            assert closure_cell(callback, 'value').cell_contents == 7
        else:
            assert callbacks is None
        observed.append((
            enabled,
            value is marker,
            None if value is marker else (type(value).__name__, value),
            None if callbacks is None else callbacks[0](),
        ))
    return observed

def observe_namespace(build, native=False):
    marker = object()
    cls = build(marker)
    lifecycle_class(cls, native)
    assert cls.seen == 'namespace' and 'value' not in vars(cls)
    assert 'unused' not in vars(cls)
    callback, = cls.callbacks
    lifecycle_function(callback, native)
    assert callback() is marker
    cell = closure_cell(callback, 'value')
    assert cell.cell_contents is marker
    replacement = object()
    cell.cell_contents = replacement
    assert callback() is replacement
    assert cls.seen == 'namespace'
    return ('namespace', callback() is replacement, 'value' in vars(cls))

def class_error_callback(error):
    traceback = error.__traceback__
    while traceback is not None:
        namespace = traceback.tb_frame.f_locals
        if 'callbacks' in namespace:
            callback, = namespace['callbacks']
            return callback
        traceback = traceback.tb_next
    raise AssertionError('original finally suite did not publish its callback')

def observe_finally(build, native=False):
    observed = []
    for fails in (False, True):
        events = []
        marker = ValueError('checkpoint')
        def checkpoint():
            events.append('checkpoint')
            if fails:
                raise marker
        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(checkpoint)
            except ValueError as error:
                assert fails and error is marker and error.__context__ is caller
                callback = class_error_callback(error)
                lifecycle_function(callback, native)
                assert callback() == 1
                reference = weakref.ref(callback)
                del callback
                error.__traceback__ = None
                gc.collect()
                assert reference() is None
                events.append('finally-error')
            else:
                assert not fails
                lifecycle_class(cls, native)
                callback, = cls.callbacks
                lifecycle_function(callback, native)
                assert callback() == 1
                events.append('finally-success')
                del callback, cls
            assert sys.exception() is caller
        observed.append((fails, events))
    return observed

def observe_pre_region_raise(build, native=False):
    events = []
    def source():
        events.append('unreachable-iterable')
        raise AssertionError('unreachable region evaluated its source')
    try:
        raise KeyError('caller')
    except KeyError as caller:
        try:
            build(source)
        except ValueError as error:
            assert error.args == ('before region',)
            assert error.__context__ is caller
            assert sys.exception() is error
            error.__traceback__ = None
        else:
            raise AssertionError('original class failure was swallowed')
        assert sys.exception() is caller
    assert events == []
    return ('before region', events)
import native_lifecycle_namespace_delete_capture as module
import native_lifecycle_namespace_delete_capture as actual
import ordinary_lifecycle_namespace_delete_capture as ordinary
assert _soac_ext.strict_module_diagnostics(ordinary) is None
build = getattr(ordinary, 'factory')
expected = observe_namespace(build)
build = getattr(actual, 'factory')
observed = observe_namespace(build, native=True)
assert observed == expected, (observed, expected)
assert _soac_ext.strict_function_diagnostics(build)['original_code_entered']
# ok
# test_cpython_class_lifecycle_distinct_behavior_matches_ordinary[finally_completion]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
import ctypes
import importlib
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

def check_class_owner(cls):
    assert __dp_integration_mode__ == 'cpython'
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(cls) and info.owner is not None
    assert sealed(cls) == 1

def check_function_owner(function, *, interpreted=False):
    module = importlib.import_module('native_lifecycle_finally_completion')
    diagnostic = _assert_cpython_function_witness(
        function, _soac_ext.strict_module_diagnostics(module),
    )
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(function)
    assert _soac_ext.strict_function_entry_kind(function) == 'original_code'
import ctypes
import gc
import sys
import weakref

def lifecycle_class(cls, native):
    if native:
        check_class_owner(cls)
    else:
        owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(cls) is None

def lifecycle_function(function, native):
    if native:
        check_function_owner(function)
    else:
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(function) is None

def observe_collection(build, kind, native=False):
    outcomes = []
    for outcome in ('success', 'source-error', 'next-error', 'hash-error'):
        events = []
        refs = {}
        marker = LookupError(outcome)

        def handled():
            error = sys.exception()
            return None if error is None else str(error.args[0])

        def live():
            return tuple(bool(refs.get(name) and refs[name]() is not None)
                         for name in ('item', 'iterator'))

        class Item:
            def __hash__(self):
                events.append(('hash', handled()))
                if outcome == 'hash-error':
                    raise marker
                return 7
            def __del__(self):
                events.append(('drop-item', handled(), live()))

        class Iterator:
            def __init__(self):
                self.started = False
                refs['iterator'] = weakref.ref(self)
            def __iter__(self):
                events.append(('iter', handled()))
                return self
            def __next__(self):
                if not self.started:
                    self.started = True
                    item = Item()
                    refs['item'] = weakref.ref(item)
                    events.append(('made-item', handled()))
                    return item
                if outcome == 'next-error':
                    raise marker
                raise StopIteration
            def __del__(self):
                events.append(('drop-iterator', handled(), live()))

        def source():
            events.append(('source', handled()))
            if outcome == 'source-error':
                raise marker
            return Iterator()

        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(source)
            except LookupError as error:
                assert outcome != 'success' and error is marker
                assert error.__context__ is caller
                events.append(('caught', handled(), live()))
                error.__traceback__ = None
                events.append(('traceback-cleared', handled(), live()))
            else:
                assert outcome == 'success'
                lifecycle_class(cls, native)
                result = vars(cls)['result']
                assert type(result) is (set if kind == 'set' else dict)
                assert len(result) == 1
                item = refs['item']()
                assert item is not None and item in result
                if kind == 'dict':
                    assert result[item] is item
                del item
                result.clear()
                events.append(('cleared', handled(), live()))
                del result, cls
            assert sys.exception() is caller
            events.append(('after-call', handled(), live()))
        gc.collect()
        assert live() == (False, False)
        events.append(('after-handler', handled(), live()))
        outcomes.append((outcome, events))
    return outcomes

def observe_conditional(build, native=False):
    marker = object()
    observed = []
    for enabled in (False, True):
        cls = build(marker, enabled)
        lifecycle_class(cls, native)
        assert len(cls.result) == 1
        callbacks, value = cls.result[0]
        # The pinned CPython emitter saves/restores the hidden slot but writes
        # the distinct FREE slot in the enabled inner comprehension. Preserve
        # that ordinary 3.15.0a5 behavior, not an assumed SOAC lifetime recipe.
        if enabled:
            assert type(value) is int and value == 7
        else:
            assert value is marker
        assert 'value' not in vars(cls) and 'unused' not in vars(cls)
        if enabled:
            assert len(callbacks) == 1
            callback = callbacks[0]
            lifecycle_function(callback, native)
            assert callback() == 7
            assert closure_cell(callback, 'value').cell_contents == 7
        else:
            assert callbacks is None
        observed.append((
            enabled,
            value is marker,
            None if value is marker else (type(value).__name__, value),
            None if callbacks is None else callbacks[0](),
        ))
    return observed

def observe_namespace(build, native=False):
    marker = object()
    cls = build(marker)
    lifecycle_class(cls, native)
    assert cls.seen == 'namespace' and 'value' not in vars(cls)
    assert 'unused' not in vars(cls)
    callback, = cls.callbacks
    lifecycle_function(callback, native)
    assert callback() is marker
    cell = closure_cell(callback, 'value')
    assert cell.cell_contents is marker
    replacement = object()
    cell.cell_contents = replacement
    assert callback() is replacement
    assert cls.seen == 'namespace'
    return ('namespace', callback() is replacement, 'value' in vars(cls))

def class_error_callback(error):
    traceback = error.__traceback__
    while traceback is not None:
        namespace = traceback.tb_frame.f_locals
        if 'callbacks' in namespace:
            callback, = namespace['callbacks']
            return callback
        traceback = traceback.tb_next
    raise AssertionError('original finally suite did not publish its callback')

def observe_finally(build, native=False):
    observed = []
    for fails in (False, True):
        events = []
        marker = ValueError('checkpoint')
        def checkpoint():
            events.append('checkpoint')
            if fails:
                raise marker
        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(checkpoint)
            except ValueError as error:
                assert fails and error is marker and error.__context__ is caller
                callback = class_error_callback(error)
                lifecycle_function(callback, native)
                assert callback() == 1
                reference = weakref.ref(callback)
                del callback
                error.__traceback__ = None
                gc.collect()
                assert reference() is None
                events.append('finally-error')
            else:
                assert not fails
                lifecycle_class(cls, native)
                callback, = cls.callbacks
                lifecycle_function(callback, native)
                assert callback() == 1
                events.append('finally-success')
                del callback, cls
            assert sys.exception() is caller
        observed.append((fails, events))
    return observed

def observe_pre_region_raise(build, native=False):
    events = []
    def source():
        events.append('unreachable-iterable')
        raise AssertionError('unreachable region evaluated its source')
    try:
        raise KeyError('caller')
    except KeyError as caller:
        try:
            build(source)
        except ValueError as error:
            assert error.args == ('before region',)
            assert error.__context__ is caller
            assert sys.exception() is error
            error.__traceback__ = None
        else:
            raise AssertionError('original class failure was swallowed')
        assert sys.exception() is caller
    assert events == []
    return ('before region', events)
import native_lifecycle_finally_completion as module
import native_lifecycle_finally_completion as actual
import ordinary_lifecycle_finally_completion as ordinary
assert _soac_ext.strict_module_diagnostics(ordinary) is None
build = getattr(ordinary, 'build')
expected = observe_finally(build)
build = getattr(actual, 'build')
observed = observe_finally(build, native=True)
assert observed == expected, (observed, expected)
assert _soac_ext.strict_function_diagnostics(build)['original_code_entered']
# ok
# test_cpython_class_lifecycle_distinct_behavior_matches_ordinary[pre_region_raise]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
import ctypes
import importlib
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

def check_class_owner(cls):
    assert __dp_integration_mode__ == 'cpython'
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(cls) and info.owner is not None
    assert sealed(cls) == 1

def check_function_owner(function, *, interpreted=False):
    module = importlib.import_module('native_lifecycle_pre_region_raise')
    diagnostic = _assert_cpython_function_witness(
        function, _soac_ext.strict_module_diagnostics(module),
    )
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(function)
    assert _soac_ext.strict_function_entry_kind(function) == 'original_code'
import ctypes
import gc
import sys
import weakref

def lifecycle_class(cls, native):
    if native:
        check_class_owner(cls)
    else:
        owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(cls) is None

def lifecycle_function(function, native):
    if native:
        check_function_owner(function)
    else:
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert owner(function) is None

def observe_collection(build, kind, native=False):
    outcomes = []
    for outcome in ('success', 'source-error', 'next-error', 'hash-error'):
        events = []
        refs = {}
        marker = LookupError(outcome)

        def handled():
            error = sys.exception()
            return None if error is None else str(error.args[0])

        def live():
            return tuple(bool(refs.get(name) and refs[name]() is not None)
                         for name in ('item', 'iterator'))

        class Item:
            def __hash__(self):
                events.append(('hash', handled()))
                if outcome == 'hash-error':
                    raise marker
                return 7
            def __del__(self):
                events.append(('drop-item', handled(), live()))

        class Iterator:
            def __init__(self):
                self.started = False
                refs['iterator'] = weakref.ref(self)
            def __iter__(self):
                events.append(('iter', handled()))
                return self
            def __next__(self):
                if not self.started:
                    self.started = True
                    item = Item()
                    refs['item'] = weakref.ref(item)
                    events.append(('made-item', handled()))
                    return item
                if outcome == 'next-error':
                    raise marker
                raise StopIteration
            def __del__(self):
                events.append(('drop-iterator', handled(), live()))

        def source():
            events.append(('source', handled()))
            if outcome == 'source-error':
                raise marker
            return Iterator()

        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(source)
            except LookupError as error:
                assert outcome != 'success' and error is marker
                assert error.__context__ is caller
                events.append(('caught', handled(), live()))
                error.__traceback__ = None
                events.append(('traceback-cleared', handled(), live()))
            else:
                assert outcome == 'success'
                lifecycle_class(cls, native)
                result = vars(cls)['result']
                assert type(result) is (set if kind == 'set' else dict)
                assert len(result) == 1
                item = refs['item']()
                assert item is not None and item in result
                if kind == 'dict':
                    assert result[item] is item
                del item
                result.clear()
                events.append(('cleared', handled(), live()))
                del result, cls
            assert sys.exception() is caller
            events.append(('after-call', handled(), live()))
        gc.collect()
        assert live() == (False, False)
        events.append(('after-handler', handled(), live()))
        outcomes.append((outcome, events))
    return outcomes

def observe_conditional(build, native=False):
    marker = object()
    observed = []
    for enabled in (False, True):
        cls = build(marker, enabled)
        lifecycle_class(cls, native)
        assert len(cls.result) == 1
        callbacks, value = cls.result[0]
        # The pinned CPython emitter saves/restores the hidden slot but writes
        # the distinct FREE slot in the enabled inner comprehension. Preserve
        # that ordinary 3.15.0a5 behavior, not an assumed SOAC lifetime recipe.
        if enabled:
            assert type(value) is int and value == 7
        else:
            assert value is marker
        assert 'value' not in vars(cls) and 'unused' not in vars(cls)
        if enabled:
            assert len(callbacks) == 1
            callback = callbacks[0]
            lifecycle_function(callback, native)
            assert callback() == 7
            assert closure_cell(callback, 'value').cell_contents == 7
        else:
            assert callbacks is None
        observed.append((
            enabled,
            value is marker,
            None if value is marker else (type(value).__name__, value),
            None if callbacks is None else callbacks[0](),
        ))
    return observed

def observe_namespace(build, native=False):
    marker = object()
    cls = build(marker)
    lifecycle_class(cls, native)
    assert cls.seen == 'namespace' and 'value' not in vars(cls)
    assert 'unused' not in vars(cls)
    callback, = cls.callbacks
    lifecycle_function(callback, native)
    assert callback() is marker
    cell = closure_cell(callback, 'value')
    assert cell.cell_contents is marker
    replacement = object()
    cell.cell_contents = replacement
    assert callback() is replacement
    assert cls.seen == 'namespace'
    return ('namespace', callback() is replacement, 'value' in vars(cls))

def class_error_callback(error):
    traceback = error.__traceback__
    while traceback is not None:
        namespace = traceback.tb_frame.f_locals
        if 'callbacks' in namespace:
            callback, = namespace['callbacks']
            return callback
        traceback = traceback.tb_next
    raise AssertionError('original finally suite did not publish its callback')

def observe_finally(build, native=False):
    observed = []
    for fails in (False, True):
        events = []
        marker = ValueError('checkpoint')
        def checkpoint():
            events.append('checkpoint')
            if fails:
                raise marker
        try:
            raise KeyError('caller')
        except KeyError as caller:
            try:
                cls = build(checkpoint)
            except ValueError as error:
                assert fails and error is marker and error.__context__ is caller
                callback = class_error_callback(error)
                lifecycle_function(callback, native)
                assert callback() == 1
                reference = weakref.ref(callback)
                del callback
                error.__traceback__ = None
                gc.collect()
                assert reference() is None
                events.append('finally-error')
            else:
                assert not fails
                lifecycle_class(cls, native)
                callback, = cls.callbacks
                lifecycle_function(callback, native)
                assert callback() == 1
                events.append('finally-success')
                del callback, cls
            assert sys.exception() is caller
        observed.append((fails, events))
    return observed

def observe_pre_region_raise(build, native=False):
    events = []
    def source():
        events.append('unreachable-iterable')
        raise AssertionError('unreachable region evaluated its source')
    try:
        raise KeyError('caller')
    except KeyError as caller:
        try:
            build(source)
        except ValueError as error:
            assert error.args == ('before region',)
            assert error.__context__ is caller
            assert sys.exception() is error
            error.__traceback__ = None
        else:
            raise AssertionError('original class failure was swallowed')
        assert sys.exception() is caller
    assert events == []
    return ('before region', events)
import native_lifecycle_pre_region_raise as module
import native_lifecycle_pre_region_raise as actual
import ordinary_lifecycle_pre_region_raise as ordinary
assert _soac_ext.strict_module_diagnostics(ordinary) is None
build = getattr(ordinary, 'build')
expected = observe_pre_region_raise(build)
build = getattr(actual, 'build')
observed = observe_pre_region_raise(build, native=True)
assert observed == expected, (observed, expected)
assert _soac_ext.strict_function_diagnostics(build)['original_code_entered']
