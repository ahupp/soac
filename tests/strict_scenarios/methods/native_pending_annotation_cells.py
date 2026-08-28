# modes:cpython
# module:cpython_pending_class_scope
# soac: module(strict_assign=true, checked_attr=true)
from cpython_pending_class_scope_support import inspect_pending, events

class Base:
    def __init_subclass__(cls):
        inspect_pending(cls)

def factory():
    class Token:
        pass

    class Holder(Base):
        Alias = Token

        def accept(self, value: Alias) -> Alias:
            events.append("body")
            return value

    return Token, Holder

first = factory()
second = factory()
# module:cpython_pending_class_scope_support
from typing import Any

events = []
namespaces = []

def inspect_pending(cls: Any) -> None:
    from soac import _soac_ext
    from soac.strict import StrictRuntimeUnavailableError

    function = cls.accept
    diagnostic = _soac_ext.strict_function_diagnostics(function)
    assert diagnostic["backend"] == "cpython"
    assert diagnostic["entry_kind"] == "original_code"
    assert diagnostic["finalized"] is False
    assert diagnostic["original_code_entered"] is False
    provider = function.__annotate__
    provider_diagnostic = _soac_ext.strict_function_diagnostics(provider)
    assert provider_diagnostic["backend"] == "cpython"
    assert provider_diagnostic["entry_kind"] == "original_code"
    assert provider_diagnostic["original_code_entered"] is False
    for key in ("source_path", "source_sha256", "artifact_generation"):
        assert provider_diagnostic[key] == diagnostic[key]
    cells = provider.__closure__
    assert cells is not None and len(cells) == 1
    cell = cells[0]
    actual = cell.cell_contents
    assert type(actual) is dict
    assert actual["accept"] is function
    value = actual["Alias"]()
    # Do not instantiate the pending class. The receiver is unused and this
    # unbound call does not write protected storage or evaluate annotations.
    assert function(None, value) is value
    assert _soac_ext.strict_function_diagnostics(function)["finalized"] is False

    alternatives = [dict(actual)]
    if namespaces:
        previous = namespaces[-1]
        assert previous is not actual
        assert previous["accept"].__code__ is function.__code__
        assert previous["accept"].__annotate__.__code__ is provider.__code__
        alternatives.append(previous)
    try:
        for replacement in alternatives:
            cell.cell_contents = replacement
            before = list(events)
            assert function(None, value) is value
            assert events == before + ["body"]
            assert _soac_ext.strict_function_diagnostics(provider)["original_code_entered"] is False
    finally:
        cell.cell_contents = actual
    assert function(None, value) is value
    assert _soac_ext.strict_function_diagnostics(provider)["original_code_entered"] is False
    namespaces.append(actual)
    events.append(("pending ordinary call", len(namespaces)))
# ok
# test_cpython_pending_class_calls_keep_source_ownership_without_annotation_lookup
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('factory',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import cpython_pending_class_scope as module
from cpython_pending_class_scope_support import events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

assert events == [
    "body", "body", "body", ("pending ordinary call", 1),
    "body", "body", "body", "body", ("pending ordinary call", 2),
]
diagnostic = _soac_ext.strict_module_diagnostics(module)
FirstToken, FirstHolder = module.first
SecondToken, SecondHolder = module.second
assert FirstToken is not SecondToken and FirstHolder is not SecondHolder
for Token, Holder in (module.first, module.second):
    observed = _assert_cpython_function_witness(
        Holder.accept, diagnostic,
    )
    assert observed["finalized"] is True
    assert observed["original_code_entered"] is True
    provider = Holder.accept.__annotate__
    assert _assert_cpython_function_witness(
        provider, diagnostic,
    )["original_code_entered"] is False
    value = Token()
    assert Holder().accept(value) is value
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('factory',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
