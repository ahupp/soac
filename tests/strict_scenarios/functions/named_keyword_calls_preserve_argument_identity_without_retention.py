# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:keyword_operands
# soac: module(strict_assign=true, checked_attr=true)

from keyword_operand_support import keyword_referrers, make_marker, mixed_referrers

def inspect_keyword(value):
    return keyword_referrers(value=value)

def inspect_mixed(value):
    return mixed_referrers(value, named=1)

def release_keywords(callback):
    return callback(first=make_marker("first"), second=make_marker("second"))

def captured_keyword_attribute(holder, first, second):
    return holder.callback(first=first(), second=second())

def captured_keyword_cell(callback):
    def replace(value):
        nonlocal callback
        callback = value
    def invoke(first, second):
        return callback(first=first(), second=second())
    return invoke, replace
# module:keyword_operand_control
from keyword_operand_support import keyword_referrers, make_marker, mixed_referrers

def inspect_keyword(value):
    return keyword_referrers(value=value)

def inspect_mixed(value):
    return mixed_referrers(value, named=1)

def release_keywords(callback):
    return callback(first=make_marker("first"), second=make_marker("second"))

def captured_keyword_attribute(holder, first, second):
    return holder.callback(first=first(), second=second())

def captured_keyword_cell(callback):
    def replace(value):
        nonlocal callback
        callback = value
    def invoke(first, second):
        return callback(first=first(), second=second())
    return invoke, replace
# module:keyword_operand_support
import gc
import weakref

events: list[str] = []
observed_arguments = []

class Marker:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append("drop:" + self.name)

def make_marker(name):
    events.append("create:" + name)
    return Marker(name)

def keyword_referrers(*, value):
    observed_arguments.append(weakref.ref(value))
    return [
        (type(referrer).__name__, tuple(referrer))
        for referrer in gc.get_referrers(value)
        if type(referrer) is dict and referrer.get("value") is value
    ]

def mixed_referrers(value, *, named):
    assert named == 1
    observed_arguments.append(weakref.ref(value))
    return [
        (type(referrer).__name__, len(referrer))
        for referrer in gc.get_referrers(value)
        if type(referrer) is tuple and any(item is value for item in referrer)
    ]

def discard(*, first, second):
    events.append("body")

class Holder:
    def __init__(self, callback):
        self.callback = callback

class Sink:
    def __call__(self, *, first, second):
        events.append("body:original")

    def __del__(self):
        events.append("drop:callable")
# ok
# tests/test_strict_function_boundaries.py::test_named_keyword_calls_preserve_argument_identity_without_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('inspect_keyword',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

import gc
import weakref
import keyword_operand_control as control
from keyword_operand_support import observed_arguments
from soac import _soac_ext

def validate_module(module):
    assert _soac_ext.strict_module_diagnostics(control) is None
    assert _soac_ext.strict_function_entry_kind(control.inspect_keyword) is None
    class Payload:
        pass
    for candidate in (control, module):
        observed_arguments.clear()
        released = []
        payload = Payload()
        reference = weakref.ref(payload, lambda _: released.append("payload"))
        result = candidate.inspect_keyword(payload)
        assert len(observed_arguments) == 1
        assert observed_arguments[0]() is payload
        assert isinstance(result, list)
        if candidate is control:
            assert result == [], result
        # SOAC may own different temporary containers during the call. None
        # may keep the argument alive after the call and explicit collection.
        del payload
        gc.collect()
        assert reference() is None, result
        assert released == ["payload"], released

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_named_keyword_calls_preserve_argument_identity_without_retention
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('inspect_mixed',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

import gc
import weakref
import keyword_operand_control as control
from keyword_operand_support import observed_arguments
from soac import _soac_ext

def validate_module(module):
    assert _soac_ext.strict_module_diagnostics(control) is None
    assert _soac_ext.strict_function_entry_kind(control.inspect_mixed) is None
    class Payload:
        pass
    for candidate in (control, module):
        observed_arguments.clear()
        released = []
        payload = Payload()
        reference = weakref.ref(payload, lambda _: released.append("payload"))
        result = candidate.inspect_mixed(payload)
        assert len(observed_arguments) == 1
        assert observed_arguments[0]() is payload
        assert isinstance(result, list)
        if candidate is control:
            assert result == [], result
        # SOAC may own different temporary containers during the call. None
        # may keep the argument alive after the call and explicit collection.
        del payload
        gc.collect()
        assert reference() is None, result
        assert released == ["payload"], released

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_named_keyword_call_preserves_binding_and_releases_values_once
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('release_keywords',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

import gc
import keyword_operand_control as control
from keyword_operand_support import discard, events
from soac import _soac_ext

def validate_module(module):
    assert _soac_ext.strict_module_diagnostics(control) is None
    assert _soac_ext.strict_function_entry_kind(control.release_keywords) is None
    def observe(function):
        events.clear()
        error_info = None
        try:
            function(object if False else discard)
        except TypeError as error:
            error_info = (type(error).__name__, error.args)
        events.append("after")
        gc.collect()
        return tuple(events), error_info

    expected = observe(control.release_keywords)
    actual = observe(module.release_keywords)
    prefix = ("create:first", "create:second")
    if not False:
        prefix += ("body",)
    assert expected[0] == prefix + ("drop:second", "drop:first", "after"), expected
    assert (expected[1] is not None) == False, expected
    assert actual[1] == expected[1], (actual, expected)
    assert tuple(event for event in actual[0] if not event.startswith("drop:")) == prefix + ("after",), actual
    assert sorted(event for event in actual[0] if event.startswith("drop:")) == [
        "drop:first", "drop:second",
    ], actual

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_named_keyword_call_preserves_binding_and_releases_values_once
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('release_keywords',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

import gc
import keyword_operand_control as control
from keyword_operand_support import discard, events
from soac import _soac_ext

def validate_module(module):
    assert _soac_ext.strict_module_diagnostics(control) is None
    assert _soac_ext.strict_function_entry_kind(control.release_keywords) is None
    def observe(function):
        events.clear()
        error_info = None
        try:
            function(object if True else discard)
        except TypeError as error:
            error_info = (type(error).__name__, error.args)
        events.append("after")
        gc.collect()
        return tuple(events), error_info

    expected = observe(control.release_keywords)
    actual = observe(module.release_keywords)
    prefix = ("create:first", "create:second")
    if not True:
        prefix += ("body",)
    assert expected[0] == prefix + ("drop:second", "drop:first", "after"), expected
    assert (expected[1] is not None) == True, expected
    assert actual[1] == expected[1], (actual, expected)
    assert tuple(event for event in actual[0] if not event.startswith("drop:")) == prefix + ("after",), actual
    assert sorted(event for event in actual[0] if event.startswith("drop:")) == [
        "drop:first", "drop:second",
    ], actual

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_named_keyword_callbacks_keep_the_captured_callable_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('captured_keyword_attribute',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

import gc
import keyword_operand_control as control
from keyword_operand_support import Holder, Sink, events, make_marker
from soac import _soac_ext

def validate_module(module):
    def observe(candidate):
        events.clear()
        marker = LookupError("keyword expression failed")
        def replacement(*, first, second):
            raise AssertionError("call target was reloaded after an argument callback")
        if 'attribute' == "cell":
            invoke, replace = candidate.captured_keyword_cell(Sink())
            entry = _soac_ext.strict_function_entry_kind(invoke)
            expected_entry = None if candidate is control else (
                "entry_interpreter" if __dp_integration_entry__ else "checked_native"
            )
            assert entry == expected_entry, entry
        else:
            holder = Holder(Sink())
            def replace(value):
                holder.callback = value
            def invoke(first, second):
                return candidate.captured_keyword_attribute(holder, first, second)
        def first():
            return make_marker("first")
        def second():
            events.append("second")
            replace(replacement)
            if False:
                raise marker
            return make_marker("second")
        try:
            invoke(first, second)
        except LookupError as error:
            assert False
            assert error is marker
            events.append("caught")
        else:
            assert not False
        events.append("after")
        marker.__traceback__ = None
        gc.collect()
        return tuple(events)

    expected = observe(control)
    actual = observe(module)
    prefix = ("create:first", "second")
    if False:
        suffix = ("drop:first", "drop:callable", "caught", "after")
    else:
        suffix = ("create:second", "body:original", "drop:second", "drop:first", "drop:callable", "after")
    assert expected == prefix + suffix, expected
    assert tuple(event for event in actual if not event.startswith("drop:")) == tuple(
        event for event in expected if not event.startswith("drop:")
    ), (actual, expected)
    assert sorted(event for event in actual if event.startswith("drop:")) == sorted(
        event for event in expected if event.startswith("drop:")
    ), (actual, expected)

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_named_keyword_callbacks_keep_the_captured_callable_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('captured_keyword_attribute',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

import gc
import keyword_operand_control as control
from keyword_operand_support import Holder, Sink, events, make_marker
from soac import _soac_ext

def validate_module(module):
    def observe(candidate):
        events.clear()
        marker = LookupError("keyword expression failed")
        def replacement(*, first, second):
            raise AssertionError("call target was reloaded after an argument callback")
        if 'attribute' == "cell":
            invoke, replace = candidate.captured_keyword_cell(Sink())
            entry = _soac_ext.strict_function_entry_kind(invoke)
            expected_entry = None if candidate is control else (
                "entry_interpreter" if __dp_integration_entry__ else "checked_native"
            )
            assert entry == expected_entry, entry
        else:
            holder = Holder(Sink())
            def replace(value):
                holder.callback = value
            def invoke(first, second):
                return candidate.captured_keyword_attribute(holder, first, second)
        def first():
            return make_marker("first")
        def second():
            events.append("second")
            replace(replacement)
            if True:
                raise marker
            return make_marker("second")
        try:
            invoke(first, second)
        except LookupError as error:
            assert True
            assert error is marker
            events.append("caught")
        else:
            assert not True
        events.append("after")
        marker.__traceback__ = None
        gc.collect()
        return tuple(events)

    expected = observe(control)
    actual = observe(module)
    prefix = ("create:first", "second")
    if True:
        suffix = ("drop:first", "drop:callable", "caught", "after")
    else:
        suffix = ("create:second", "body:original", "drop:second", "drop:first", "drop:callable", "after")
    assert expected == prefix + suffix, expected
    assert tuple(event for event in actual if not event.startswith("drop:")) == tuple(
        event for event in expected if not event.startswith("drop:")
    ), (actual, expected)
    assert sorted(event for event in actual if event.startswith("drop:")) == sorted(
        event for event in expected if event.startswith("drop:")
    ), (actual, expected)

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_named_keyword_callbacks_keep_the_captured_callable_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('captured_keyword_cell',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

import gc
import keyword_operand_control as control
from keyword_operand_support import Holder, Sink, events, make_marker
from soac import _soac_ext

def validate_module(module):
    def observe(candidate):
        events.clear()
        marker = LookupError("keyword expression failed")
        def replacement(*, first, second):
            raise AssertionError("call target was reloaded after an argument callback")
        if 'cell' == "cell":
            invoke, replace = candidate.captured_keyword_cell(Sink())
            entry = _soac_ext.strict_function_entry_kind(invoke)
            expected_entry = None if candidate is control else (
                "entry_interpreter" if __dp_integration_entry__ else "checked_native"
            )
            assert entry == expected_entry, entry
        else:
            holder = Holder(Sink())
            def replace(value):
                holder.callback = value
            def invoke(first, second):
                return candidate.captured_keyword_attribute(holder, first, second)
        def first():
            return make_marker("first")
        def second():
            events.append("second")
            replace(replacement)
            if False:
                raise marker
            return make_marker("second")
        try:
            invoke(first, second)
        except LookupError as error:
            assert False
            assert error is marker
            events.append("caught")
        else:
            assert not False
        events.append("after")
        marker.__traceback__ = None
        gc.collect()
        return tuple(events)

    expected = observe(control)
    actual = observe(module)
    prefix = ("create:first", "second")
    if False:
        suffix = ("drop:first", "drop:callable", "caught", "after")
    else:
        suffix = ("create:second", "body:original", "drop:second", "drop:first", "drop:callable", "after")
    assert expected == prefix + suffix, expected
    assert tuple(event for event in actual if not event.startswith("drop:")) == tuple(
        event for event in expected if not event.startswith("drop:")
    ), (actual, expected)
    assert sorted(event for event in actual if event.startswith("drop:")) == sorted(
        event for event in expected if event.startswith("drop:")
    ), (actual, expected)

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_named_keyword_callbacks_keep_the_captured_callable_and_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('captured_keyword_cell',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

import gc
import keyword_operand_control as control
from keyword_operand_support import Holder, Sink, events, make_marker
from soac import _soac_ext

def validate_module(module):
    def observe(candidate):
        events.clear()
        marker = LookupError("keyword expression failed")
        def replacement(*, first, second):
            raise AssertionError("call target was reloaded after an argument callback")
        if 'cell' == "cell":
            invoke, replace = candidate.captured_keyword_cell(Sink())
            entry = _soac_ext.strict_function_entry_kind(invoke)
            expected_entry = None if candidate is control else (
                "entry_interpreter" if __dp_integration_entry__ else "checked_native"
            )
            assert entry == expected_entry, entry
        else:
            holder = Holder(Sink())
            def replace(value):
                holder.callback = value
            def invoke(first, second):
                return candidate.captured_keyword_attribute(holder, first, second)
        def first():
            return make_marker("first")
        def second():
            events.append("second")
            replace(replacement)
            if True:
                raise marker
            return make_marker("second")
        try:
            invoke(first, second)
        except LookupError as error:
            assert True
            assert error is marker
            events.append("caught")
        else:
            assert not True
        events.append("after")
        marker.__traceback__ = None
        gc.collect()
        return tuple(events)

    expected = observe(control)
    actual = observe(module)
    prefix = ("create:first", "second")
    if True:
        suffix = ("drop:first", "drop:callable", "caught", "after")
    else:
        suffix = ("create:second", "body:original", "drop:second", "drop:first", "drop:callable", "after")
    assert expected == prefix + suffix, expected
    assert tuple(event for event in actual if not event.startswith("drop:")) == tuple(
        event for event in expected if not event.startswith("drop:")
    ), (actual, expected)
    assert sorted(event for event in actual if event.startswith("drop:")) == sorted(
        event for event in expected if event.startswith("drop:")
    ), (actual, expected)

validate_module(module)

_assert_source_function_witnesses()
