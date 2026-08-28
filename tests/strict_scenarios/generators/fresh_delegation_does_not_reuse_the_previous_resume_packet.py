# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:initial_entry
# soac: module(strict_assign=true, checked_attr=true)

def after_send(delegate):
    marker = yield "first"
    result = yield from delegate
    return marker, result

def after_caught_throw(delegate):
    try:
        yield "first"
    except ValueError:
        return (yield from delegate)

async def after_await(first, second):
    first_result = await first
    second_result = await second
    return first_result, second_result

async def after_caught_await(first, second):
    try:
        await first
    except ValueError:
        return await second

def make_after_send(delegate):
    return after_send(delegate)

def make_after_caught_throw(delegate):
    return after_caught_throw(delegate)

def make_after_await(first, second):
    return after_await(first, second)

def make_after_caught_await(first, second):
    return after_caught_await(first, second)
# module:ordinary_initial_entry
def after_send(delegate):
    marker = yield "first"
    result = yield from delegate
    return marker, result

def after_caught_throw(delegate):
    try:
        yield "first"
    except ValueError:
        return (yield from delegate)

async def after_await(first, second):
    first_result = await first
    second_result = await second
    return first_result, second_result

async def after_caught_await(first, second):
    try:
        await first
    except ValueError:
        return await second

def make_after_send(delegate):
    return after_send(delegate)

def make_after_caught_throw(delegate):
    return after_caught_throw(delegate)

def make_after_await(first, second):
    return after_await(first, second)

def make_after_caught_await(first, second):
    return after_caught_await(first, second)
# ok
# tests/test_strict_generator_protocols.py::test_fresh_delegation_does_not_reuse_the_previous_resume_packet
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_after_send',):
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

import ctypes
import pytest
import ordinary_initial_entry
from soac import _soac_ext

def delegate():
    yield "delegated"
    return "delegate-result"

def exercise(module, case):
    class Awaitable:
        def __init__(self, token):
            self.token = token

        def __await__(self):
            result = yield self.token
            return result

    if case == "after_send":
        generator = module.make_after_send(delegate())
        assert next(generator) == "first"
        assert generator.send("sent-value") == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == ("sent-value", "delegate-result")
    elif case == "after_caught_throw":
        generator = module.make_after_caught_throw(delegate())
        assert next(generator) == "first"
        assert generator.throw(ValueError("injected")) == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == "delegate-result"
    elif case == "after_await":
        coroutine = module.make_after_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.send("first-result") == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == ("first-result", "second-result")
    elif case == "after_caught_await":
        coroutine = module.make_after_caught_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.throw(ValueError("injected")) == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == "second-result"
    else:
        raise AssertionError(case)

def validate_module(module):
    case = 'after_send'
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    factory_name = "make_" + case
    assert not owner(getattr(ordinary_initial_entry, factory_name))
    assert _soac_ext.strict_module_diagnostics(ordinary_initial_entry) is None
    exercise(ordinary_initial_entry, case)
    assert owner(getattr(module, factory_name))
    # run_case already requires the exact requested native/entry execution kind.
    exercise(module, case)

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_fresh_delegation_does_not_reuse_the_previous_resume_packet
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_after_caught_throw',):
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

import ctypes
import pytest
import ordinary_initial_entry
from soac import _soac_ext

def delegate():
    yield "delegated"
    return "delegate-result"

def exercise(module, case):
    class Awaitable:
        def __init__(self, token):
            self.token = token

        def __await__(self):
            result = yield self.token
            return result

    if case == "after_send":
        generator = module.make_after_send(delegate())
        assert next(generator) == "first"
        assert generator.send("sent-value") == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == ("sent-value", "delegate-result")
    elif case == "after_caught_throw":
        generator = module.make_after_caught_throw(delegate())
        assert next(generator) == "first"
        assert generator.throw(ValueError("injected")) == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == "delegate-result"
    elif case == "after_await":
        coroutine = module.make_after_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.send("first-result") == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == ("first-result", "second-result")
    elif case == "after_caught_await":
        coroutine = module.make_after_caught_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.throw(ValueError("injected")) == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == "second-result"
    else:
        raise AssertionError(case)

def validate_module(module):
    case = 'after_caught_throw'
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    factory_name = "make_" + case
    assert not owner(getattr(ordinary_initial_entry, factory_name))
    assert _soac_ext.strict_module_diagnostics(ordinary_initial_entry) is None
    exercise(ordinary_initial_entry, case)
    assert owner(getattr(module, factory_name))
    # run_case already requires the exact requested native/entry execution kind.
    exercise(module, case)

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_fresh_delegation_does_not_reuse_the_previous_resume_packet
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_after_await',):
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

import ctypes
import pytest
import ordinary_initial_entry
from soac import _soac_ext

def delegate():
    yield "delegated"
    return "delegate-result"

def exercise(module, case):
    class Awaitable:
        def __init__(self, token):
            self.token = token

        def __await__(self):
            result = yield self.token
            return result

    if case == "after_send":
        generator = module.make_after_send(delegate())
        assert next(generator) == "first"
        assert generator.send("sent-value") == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == ("sent-value", "delegate-result")
    elif case == "after_caught_throw":
        generator = module.make_after_caught_throw(delegate())
        assert next(generator) == "first"
        assert generator.throw(ValueError("injected")) == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == "delegate-result"
    elif case == "after_await":
        coroutine = module.make_after_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.send("first-result") == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == ("first-result", "second-result")
    elif case == "after_caught_await":
        coroutine = module.make_after_caught_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.throw(ValueError("injected")) == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == "second-result"
    else:
        raise AssertionError(case)

def validate_module(module):
    case = 'after_await'
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    factory_name = "make_" + case
    assert not owner(getattr(ordinary_initial_entry, factory_name))
    assert _soac_ext.strict_module_diagnostics(ordinary_initial_entry) is None
    exercise(ordinary_initial_entry, case)
    assert owner(getattr(module, factory_name))
    # run_case already requires the exact requested native/entry execution kind.
    exercise(module, case)

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_generator_protocols.py::test_fresh_delegation_does_not_reuse_the_previous_resume_packet
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_after_caught_await',):
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

import ctypes
import pytest
import ordinary_initial_entry
from soac import _soac_ext

def delegate():
    yield "delegated"
    return "delegate-result"

def exercise(module, case):
    class Awaitable:
        def __init__(self, token):
            self.token = token

        def __await__(self):
            result = yield self.token
            return result

    if case == "after_send":
        generator = module.make_after_send(delegate())
        assert next(generator) == "first"
        assert generator.send("sent-value") == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == ("sent-value", "delegate-result")
    elif case == "after_caught_throw":
        generator = module.make_after_caught_throw(delegate())
        assert next(generator) == "first"
        assert generator.throw(ValueError("injected")) == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == "delegate-result"
    elif case == "after_await":
        coroutine = module.make_after_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.send("first-result") == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == ("first-result", "second-result")
    elif case == "after_caught_await":
        coroutine = module.make_after_caught_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.throw(ValueError("injected")) == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == "second-result"
    else:
        raise AssertionError(case)

def validate_module(module):
    case = 'after_caught_await'
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    factory_name = "make_" + case
    assert not owner(getattr(ordinary_initial_entry, factory_name))
    assert _soac_ext.strict_module_diagnostics(ordinary_initial_entry) is None
    exercise(ordinary_initial_entry, case)
    assert owner(getattr(module, factory_name))
    # run_case already requires the exact requested native/entry execution kind.
    exercise(module, case)

validate_module(module)

_assert_source_function_witnesses()
