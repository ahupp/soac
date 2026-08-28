# modes:cpython
# module:nominal_methods
# soac: module(strict_assign=true, checked_attr=true)
from nominal_support import (
    annotation_trap, arbitrary_result, assert_method_provider_frozen, events,
)

class Token:
    def accept(self, value: Token) -> Token:
        events.append("accept")
        return value

    def optional(self, value: Token | None) -> Token | None:
        return value

    def wrong_return(self) -> Token:
        events.append("return")
        return arbitrary_result()

class Child(Token):
    def accept_base(self, value: Token) -> Token:
        return value

GlobalAlias = Token

def accept_global(value: GlobalAlias) -> Token:
    return value

# The free function is still initializing. Its replaceable provider is not
# executed or trusted for required contracts during adoption or calls.
accept_global.__annotate__ = annotation_trap
# Token already completed its actual class Store, before this module seals.
assert_method_provider_frozen(Token.accept, module_sealed=False)

def factory():
    class Local:
        def accept(self, value: Local) -> Local:
            return value
    return Local
# module:nominal_support
from typing import Any

events = []

def annotation_trap(format: int) -> Any:
    events.append("annotation evaluated")
    raise AssertionError("nominal binding evaluated an annotation provider")

def arbitrary_result() -> Any:
    return object()


def assert_method_provider_frozen(method: Any, *, module_sealed: bool) -> None:
    import sys
    from soac import _soac_ext
    from soac.strict import StrictMutationError

    namespace = method.__globals__
    module = sys.modules[namespace["__name__"]]
    assert vars(module) is namespace
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    assert diagnostic is not None and diagnostic["sealed"] is module_sealed
    provider = method.__annotate__
    assert provider is not None and provider is not annotation_trap
    try:
        method.__annotate__ = annotation_trap
    except StrictMutationError:
        pass
    else:
        raise AssertionError("admitted method accepted a replacement annotation provider")
    assert method.__annotate__ is provider
# ok
# test_cpython_backend_factory_methods_keep_actual_class_ownership_and_ordinary_calls
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('factory', 'Token.accept', 'Token.optional', 'Token.wrong_return', 'accept_global'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import ctypes
import gc
import weakref
import nominal_methods as module
from nominal_support import annotation_trap, assert_method_provider_frozen, events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

diagnostic = _soac_ext.strict_module_diagnostics(module)
assert events == []
assert module.accept_global.__annotate__ is annotation_trap
assert_method_provider_frozen(module.Token.accept, module_sealed=True)
first = module.factory()
second = module.factory()
assert first is not second
assert first.__qualname__ == second.__qualname__
assert first.accept.__code__ is second.accept.__code__
left = first()
right = second()
for function in (first.accept, second.accept):
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed['original_code_entered'] is False
assert left.accept(right) is right
assert _soac_ext.strict_function_diagnostics(first.accept)['original_code_entered'] is True
assert left.accept(left) is left
assert _soac_ext.strict_function_diagnostics(first.accept)['original_code_entered'] is True
assert _soac_ext.strict_function_diagnostics(second.accept)['original_code_entered'] is False
assert right.accept(right) is right
assert _soac_ext.strict_function_diagnostics(second.accept)['original_code_entered'] is True

# Preserve the original native annotation provider and its actual closure.
# A mutable provider cell does not change the actual class or call body.
provider = first.accept.__annotate__
cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
assert 'Local' in cells
cells['Local'].cell_contents = second
assert left.accept(left) is left
for receiver, value in ((left, right), (right, left)):
    assert receiver.accept(value) is value

class OrdinaryChild(first):
    pass
child = OrdinaryChild()
assert left.accept(child) is child
assert right.accept(child) is child
class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
class_owner.argtypes = [ctypes.py_object]
class_owner.restype = ctypes.c_void_p
assert class_owner(first) and class_owner(second)
assert not class_owner(OrdinaryChild)

call = ctypes.pythonapi.PyObject_CallOneArg
call.argtypes = [ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
for _ in range(128):
    assert left.accept(left) is left
    assert right.accept(right) is right
assert call(left.accept, child) is child
assert call(right.accept, child) is child

base = module.Token()
assert base.accept(base) is base
assert module.accept_global(base) is base
assert call(module.accept_global, base) is base
for invoke in (module.accept_global, lambda value: call(module.accept_global, value)):
    marker = object()
    assert invoke(marker) is marker
assert base.optional(None) is None
assert type(base.wrong_return()) is object
assert 'annotation evaluated' not in events
assert _soac_ext.strict_function_diagnostics(module.factory)['original_code_entered'] is True

def collectable_contract_cycle():
    local = module.factory()
    return weakref.ref(local), weakref.ref(local.accept)
references = collectable_contract_cycle()
gc.collect()
assert all(reference() is None for reference in references)
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('factory', 'Token.accept', 'Token.optional', 'Token.wrong_return', 'accept_global'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
