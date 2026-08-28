# module:methods
# soac: module(strict_assign=true, checked_attr=true)
from collections.abc import Callable

EVENTS = []
LIFETIME_EVENTS = []

class Base:
    def method(self, value: int = 1) -> int:
        EVENTS.append('base')
        return value + 10

    def invoke(self, argument):
        return self.method(argument())

class Override(Base):
    def method(self, value: int = 1) -> int:
        EVENTS.append('override')
        return value + 20

class Inherited(Base):
    pass

class FieldShadow(Base):
    method: Callable[[int], int]

    def __init__(self, callback):
        self.method = callback

def make_family(offset):
    class Local:
        def method(self, value):
            return offset + value

        def invoke(self, argument):
            return self.method(argument())
    return Local

def evaluate_pair(factory, first, second):
    return factory()(first(), second())

def temporary_method(factory):
    return factory().method()

class LifetimeTarget:
    def __init__(self, label, fail=False):
        self.label = label
        self.fail = fail

    def __del__(self):
        LIFETIME_EVENTS.append(self.label)

    def make_target(self, fail):
        return LifetimeTarget('receiver', fail)

    def method(self, first, second):
        if self.fail:
            raise ValueError('method failed')
        return 7

    def invoke(self, fail, first, second):
        return self.make_target(fail).method(first(), second())

    def invoke_then(self, fail, first, second):
        result = self.make_target(fail).method(first(), second())
        LIFETIME_EVENTS.append('continued')
        return result

    def replace_result(self, fail, first, second):
        result = LifetimeTarget('previous')
        result = self.make_target(fail).method(first(), second())
        LIFETIME_EVENTS.append('continued')
        return result
# ok
# test_virtual_calls_preserve_actual_binding_and_body_effects
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Base.method', 'Base.invoke', 'Override.method', 'FieldShadow.__init__', 'make_family'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import ctypes
import methods

is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int
for cls in (methods.Base, methods.Override, methods.Inherited, methods.FieldShadow):
    assert is_sealed(cls) == 1, cls

base, override, inherited = methods.Base(), methods.Override(), methods.Inherited()
field = methods.FieldShadow(lambda value: value + 90)
first, second = methods.make_family(30), methods.make_family(40)
assert first is not second and is_sealed(first) and is_sealed(second)
left, right = first(), second()
for unused in range(100):
    assert base.invoke(lambda: 2) == 12
    assert override.invoke(lambda: 2) == 22
    assert inherited.invoke(lambda: 2) == 12
    assert field.invoke(lambda: 2) == 92
    assert left.invoke(lambda: 2) == 32 and right.invoke(lambda: 2) == 42

methods.EVENTS.clear()
for receiver in (base, override, inherited):
    methods.EVENTS.clear()
    try:
        receiver.invoke(lambda: 'wrong')
    except TypeError:
        pass
    else:
        raise AssertionError('virtual dispatch lost the original addition error')
    expected_body = 'override' if receiver is override else 'base'
    assert methods.EVENTS == [expected_body], 'an annotation prevented body entry'

operand_events = []
class Operand:
    def __add__(self, offset):
        operand_events.append(offset)
        return ('ordinary result', offset)
for receiver, offset in ((base, 10), (override, 20), (inherited, 10)):
    assert receiver.invoke(Operand) == ('ordinary result', offset)
assert operand_events == [10, 20, 10]

events = []
class OrdinaryChild(methods.Base):
    @property
    def method(self):
        events.append('lookup')
        return lambda value: ('ordinary', value)
child = OrdinaryChild()
assert not is_sealed(OrdinaryChild)
def argument():
    events.append('argument')
    return 3
assert child.invoke(argument) == ('ordinary', 3)
assert events == ['lookup', 'argument']

original = ValueError('lookup failure precedes argument effects')
class Broken(methods.Base):
    @property
    def method(self):
        raise original
try:
    Broken().invoke(argument)
except ValueError as error:
    assert error is original
else:
    raise AssertionError('lookup exception was lost')
assert events == ['lookup', 'argument']

# Equal source identities do not put independent factory classes in one family.
assert first.invoke(right, lambda: 2) == 42
assert second.invoke(left, lambda: 2) == 32
if __dp_integration_mode__ != 'cpython':
    for function in (methods.Base.invoke, methods.Base.method, methods.Override.method, methods.evaluate_pair, methods.LifetimeTarget.invoke, methods.LifetimeTarget.invoke_then, methods.LifetimeTarget.replace_result):
        assert _soac_ext.strict_function_entry_kind(function) == expected_entry
if __dp_integration_mode__ == 'cpython':
    import ctypes
    from soac import _soac_ext
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    def assert_native_class(cls):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
        assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
        assert info.phase == 3 and info.permanent_contract_published == 1
        assert info.owner == get_type_owner(cls) and info.owner is not None
        return info.owner
    ordinary_source = "\nfrom collections.abc import Callable\n\nEVENTS = []\nLIFETIME_EVENTS = []\n\nclass Base:\n    def method(self, value: int = 1) -> int:\n        EVENTS.append('base')\n        return value + 10\n\n    def invoke(self, argument):\n        return self.method(argument())\n\nclass Override(Base):\n    def method(self, value: int = 1) -> int:\n        EVENTS.append('override')\n        return value + 20\n\nclass Inherited(Base):\n    pass\n\nclass FieldShadow(Base):\n    method: Callable[[int], int]\n\n    def __init__(self, callback):\n        self.method = callback\n\ndef make_family(offset):\n    class Local:\n        def method(self, value):\n            return offset + value\n\n        def invoke(self, argument):\n            return self.method(argument())\n    return Local\n\ndef evaluate_pair(factory, first, second):\n    return factory()(first(), second())\n\ndef temporary_method(factory):\n    return factory().method()\n\nclass LifetimeTarget:\n    def __init__(self, label, fail=False):\n        self.label = label\n        self.fail = fail\n\n    def __del__(self):\n        LIFETIME_EVENTS.append(self.label)\n\n    def make_target(self, fail):\n        return LifetimeTarget('receiver', fail)\n\n    def method(self, first, second):\n        if self.fail:\n            raise ValueError('method failed')\n        return 7\n\n    def invoke(self, fail, first, second):\n        return self.make_target(fail).method(first(), second())\n\n    def invoke_then(self, fail, first, second):\n        result = self.make_target(fail).method(first(), second())\n        LIFETIME_EVENTS.append('continued')\n        return result\n\n    def replace_result(self, fail, first, second):\n        result = LifetimeTarget('previous')\n        result = self.make_target(fail).method(first(), second())\n        LIFETIME_EVENTS.append('continued')\n        return result\n"

    from tests._strict_integration import _assert_cpython_function_witness

    module_witness = _soac_ext.strict_module_diagnostics(methods)
    for cls in (methods.Base, methods.Override, methods.Inherited, methods.FieldShadow, first, second):
        assert_native_class(cls)
    assert get_type_owner(first) != get_type_owner(second)
    for cls in (first, second):
        for name in ("method", "invoke"):
            witness = _assert_cpython_function_witness(
                vars(cls)[name], module_witness,
            )
            assert witness["finalized"] is True
            assert witness["original_code_entered"] is True

    # Exercise the owning-result helper directly, and its StackRef sibling through
    # the public vectorcall method API. No private StackRef layout is mirrored.
    get_method = ctypes.pythonapi._PyObject_GetMethod
    get_method.argtypes = [
        ctypes.py_object, ctypes.py_object, ctypes.POINTER(ctypes.c_void_p),
    ]
    get_method.restype = ctypes.c_int
    decref = ctypes.pythonapi.Py_DecRef
    decref.argtypes = [ctypes.c_void_p]
    decref.restype = None
    vectorcall_method = ctypes.pythonapi.PyObject_VectorcallMethod
    vectorcall_method.argtypes = [
        ctypes.py_object, ctypes.POINTER(ctypes.py_object), ctypes.c_size_t, ctypes.c_void_p,
    ]
    vectorcall_method.restype = ctypes.py_object

    def resolve(receiver):
        result = ctypes.c_void_p()
        unbound = get_method(receiver, "method", ctypes.byref(result))
        assert unbound in (0, 1) and result.value is not None
        try:
            target = ctypes.cast(result, ctypes.py_object).value
        finally:
            # target has its own Python reference; consume the helper's owned one.
            decref(result)
        return unbound, target

    def owning_method_call(receiver, *arguments):
        unbound, target = resolve(receiver)
        return target(receiver, *arguments) if unbound else target(*arguments)

    def stackref_method_call(receiver, *arguments):
        values = (ctypes.py_object * (1 + len(arguments)))(receiver, *arguments)
        return vectorcall_method("method", values, len(values), None)

    ordinary_namespace = {"__name__": "ordinary_method_control"}
    exec(compile(ordinary_source, "<ordinary-method-control>", "exec"), ordinary_namespace)
    ordinary_receivers = (
        ordinary_namespace["Base"](), ordinary_namespace["Override"](),
        ordinary_namespace["Inherited"](),
        ordinary_namespace["FieldShadow"](lambda value: value + 90),
    )
    for receiver, control, declaring, expected in (
        (base, ordinary_receivers[0], methods.Base, 12),
        (override, ordinary_receivers[1], methods.Override, 22),
        (inherited, ordinary_receivers[2], methods.Base, 12),
        (field, ordinary_receivers[3], None, 92),
    ):
        actual_kind, target = resolve(receiver)
        control_kind, control_target = resolve(control)
        assert actual_kind == control_kind == (declaring is not None)
        if declaring is not None:
            assert target is vars(declaring)["method"]
        else:
            assert target is vars(receiver)["method"]
            assert control_target is vars(control)["method"]
        for call in (owning_method_call, stackref_method_call):
            assert call(receiver, 2) == call(control, 2) == expected

    # Both native lookup APIs must ignore hidden dictionary values only for actual
    # protected receivers. The same original source without opt-in stays ordinary.
    for receiver, control, expected in zip(
        (base, override, inherited), ordinary_receivers[:3], (11, 21, 11),
    ):
        hidden = lambda *arguments: "dictionary shadow"
        vars(receiver)["method"] = hidden
        vars(control)["method"] = hidden
        assert resolve(receiver)[0] == 1
        assert resolve(control) == (0, hidden)
        for call in (owning_method_call, stackref_method_call):
            assert call(receiver) == expected
            assert call(control) == "dictionary shadow"
            methods.EVENTS.clear()
            try:
                call(receiver, "wrong")
            except TypeError as error:
                assert type(error) is TypeError
            else:
                raise AssertionError("C method lookup lost the original addition error")
            expected_body = "override" if receiver is override else "base"
            assert methods.EVENTS == [expected_body], "an annotation prevented C-call body entry"
        assert vars(receiver)["method"] is hidden

    # The existing ordinary property override retains descriptor binding and lookup
    # errors through both helpers, not a source-catalogued base-method shortcut.
    events.clear()
    kind, target = resolve(child)
    assert kind == 0 and events == ["lookup"]
    assert target(argument()) == ("ordinary", 3)
    assert events == ["lookup", "argument"]
    events.clear()
    assert stackref_method_call(child, 3) == ("ordinary", 3)
    assert events == ["lookup"]
    for call in (owning_method_call, stackref_method_call):
        try:
            call(Broken(), 3)
        except ValueError as error:
            assert error is original
        else:
            raise AssertionError("C method lookup discarded the actual descriptor error")
    assert events == ["lookup"]
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('Base.method', 'Base.invoke', 'Override.method', 'FieldShadow.__init__', 'make_family'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
