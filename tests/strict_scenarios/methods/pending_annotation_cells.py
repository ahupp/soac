# modes:soac,entry
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
import ctypes

events = []
namespaces = []

def inspect_pending(cls):
    from soac import _soac_ext
    from soac.strict import StrictRuntimeUnavailableError

    function = cls.accept
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    strict_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
    strict_id.argtypes = [ctypes.py_object]
    strict_id.restype = ctypes.c_uint64
    own = ctypes.pythonapi.PyType_HasSoacContract
    own.argtypes = [ctypes.py_object]
    own.restype = ctypes.c_int
    assert owner(function) and strict_id(function) == 0
    assert own(cls) == 0
    assert _soac_ext.strict_function_entry_kind(function) in (
        'checked_native', 'entry_interpreter',
    )
    provider = function.__annotate__
    # Observe the actual provider cell, not a reconstructed module dictionary.
    cells = [
        cell for cell in provider.__closure__ or ()
        if type(cell.cell_contents) is dict
        and cell.cell_contents.get('accept') is function
    ]
    assert len(cells) == 1
    cell = cells[0]
    actual = cell.cell_contents
    value = actual['Alias']()
    assert function(None, value) is value
    alternatives = [dict(actual)]
    if namespaces:
        previous = namespaces[-1]
        assert previous is not actual
        assert previous['accept'].__code__ is function.__code__
        alternatives.append(previous)
    try:
        for replacement in alternatives:
            cell.cell_contents = replacement
            before = list(events)
            assert function(None, value) is value
            assert events == before + ['body']
    finally:
        cell.cell_contents = actual
    assert function(None, value) is value
    namespaces.append(actual)
    events.append(('pending ordinary call', len(namespaces)))
# ok
# test_soac_pending_class_calls_keep_source_ownership_without_annotation_lookup
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

assert events == [
    'body', 'body', 'body', ('pending ordinary call', 1),
    'body', 'body', 'body', 'body', ('pending ordinary call', 2),
]
FirstToken, FirstHolder = module.first
SecondToken, SecondHolder = module.second
assert FirstToken is not SecondToken and FirstHolder is not SecondHolder
for Token, Holder in (module.first, module.second):
    value = Token()
    assert Holder().accept(value) is value
other = SecondToken()
assert FirstHolder().accept(other) is other
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('factory',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
