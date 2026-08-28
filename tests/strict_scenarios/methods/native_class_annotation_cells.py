# modes:cpython
# module:cpython_class_scope
# soac: module(strict_assign=true, checked_attr=true)
from cpython_class_scope_support import arbitrary_result, events

class Token:
    pass

class Holder:
    Alias = Token

    def accept(self, value: Alias) -> Alias:
        events.append("accept")
        return value

    def wrong_return(self) -> Alias:
        events.append("wrong return")
        return arbitrary_result()

def factory():
    class LocalToken:
        pass

    class LocalHolder:
        Alias = LocalToken

        def accept(self, value: Alias) -> Alias:
            events.append("factory body")
            return value

    return LocalToken, LocalHolder
# module:cpython_class_scope_support
from typing import Any

events = []

def arbitrary_result() -> Any:
    return object()

def class_dictionary_cell(function: Any) -> Any:
    # This fixture has exactly one native capture. The test mutates that actual
    # cell; neither its spelling nor its value grants runtime binding authority.
    provider = function.__annotate__
    cells = provider.__closure__
    assert cells is not None and len(cells) == 1
    return cells[0]
# ok
# test_cpython_class_scope_annotations_do_not_run_during_ordinary_calls
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Holder.accept', 'Holder.wrong_return', 'factory'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import ctypes
import cpython_class_scope as module
from cpython_class_scope_support import class_dictionary_cell, events
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError
from tests._strict_integration import _assert_cpython_function_witness

diagnostic = _soac_ext.strict_module_diagnostics(module)
functions = (module.Holder.accept, module.Holder.wrong_return)
providers = tuple(function.__annotate__ for function in functions)
for function, provider in zip(functions, providers):
    witness = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert witness["finalized"] is True
    assert witness["original_code_entered"] is False
    observed = _assert_cpython_function_witness(
        provider, diagnostic,
    )
    assert observed["original_code_entered"] is False
    namespace = class_dictionary_cell(function).cell_contents
    assert type(namespace) is dict
    assert namespace["Alias"] is module.Token

receiver = module.Holder()
value = module.Token()
before = list(events)
marker = object()
assert receiver.accept(marker) is marker
assert events == before + ["accept"]
assert _soac_ext.strict_function_diagnostics(module.Holder.accept)["original_code_entered"] is True
assert receiver.accept(value) is value

class OrdinaryChild(module.Token):
    pass

child = OrdinaryChild()
assert receiver.accept(child) is child
for _ in range(128):
    assert receiver.accept(value) is value

call = ctypes.pythonapi.PyObject_CallOneArg
call.argtypes = [ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
assert call(receiver.accept, child) is child
before = list(events)
assert call(receiver.accept, marker) is marker
assert events == before + ["accept"]

before = list(events)
assert type(receiver.wrong_return()) is object
assert events == before + ["wrong return"]
assert _soac_ext.strict_function_diagnostics(module.Holder.wrong_return)["original_code_entered"] is True
for provider in providers:
    assert _assert_cpython_function_witness(
        provider, diagnostic,
    )["original_code_entered"] is False
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Holder.accept', 'Holder.wrong_return', 'factory'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
# ok
# test_cpython_class_scope_factory_calls_ignore_annotation_dictionary_replacement
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Holder.accept', 'Holder.wrong_return', 'factory'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import cpython_class_scope as module
from cpython_class_scope_support import class_dictionary_cell, events
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError
from tests._strict_integration import _assert_cpython_function_witness

diagnostic = _soac_ext.strict_module_diagnostics(module)
FirstToken, FirstHolder = module.factory()
SecondToken, SecondHolder = module.factory()
assert FirstToken is not SecondToken and FirstHolder is not SecondHolder
assert FirstHolder.__qualname__ == SecondHolder.__qualname__
first_function, second_function = FirstHolder.accept, SecondHolder.accept
first_provider, second_provider = first_function.__annotate__, second_function.__annotate__
assert first_function is not second_function
assert first_function.__code__ is second_function.__code__
assert first_provider is not second_provider
assert first_provider.__code__ is second_provider.__code__
first_cell = class_dictionary_cell(first_function)
second_cell = class_dictionary_cell(second_function)
assert first_cell is not second_cell
first_namespace, second_namespace = first_cell.cell_contents, second_cell.cell_contents
assert type(first_namespace) is dict and type(second_namespace) is dict
assert first_namespace is not second_namespace
assert first_namespace["Alias"] is FirstToken
assert second_namespace["Alias"] is SecondToken
assert first_namespace["accept"] is first_function
assert second_namespace["accept"] is second_function
for function in (first_function, second_function):
    assert _assert_cpython_function_witness(
        function, diagnostic,
    )["finalized"] is True
for provider in (first_provider, second_provider):
    assert _assert_cpython_function_witness(
        provider, diagnostic,
    )["original_code_entered"] is False

left, right = FirstHolder(), SecondHolder()
left_value, right_value = FirstToken(), SecondToken()
assert left.accept(left_value) is left_value
assert right.accept(right_value) is right_value
copied = dict(first_namespace)
copied["Alias"] = SecondToken
try:
    for replacement in (copied, second_namespace, {"Alias": SecondToken}):
        first_cell.cell_contents = replacement
        # Replacing annotation cells does not change original body execution.
        assert left.accept(left_value) is left_value
        assert right.accept(right_value) is right_value
        for method, value in ((left.accept, right_value), (right.accept, left_value)):
            before = list(events)
            assert method(value) is value
            assert events == before + ["factory body"]
finally:
    first_cell.cell_contents = first_namespace
assert left.accept(left_value) is left_value
for provider in (first_provider, second_provider):
    assert _assert_cpython_function_witness(
        provider, diagnostic,
    )["original_code_entered"] is False
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Holder.accept', 'Holder.wrong_return', 'factory'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
