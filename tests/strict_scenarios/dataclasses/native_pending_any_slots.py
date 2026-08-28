# modes:cpython
# module:pending_slots_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
from typing import Any
import pending_slots_observer as support

def make_node():
    @dataclass(slots=True)
    class Node:
        next: Any = None

        def accept(self, value: Node) -> Node:
            support.events.append('accept body')
            return value

    return Node
# module:pending_slots_observer
import dataclasses
import weakref

events = []
attempts = []
originals = []
weak_originals = []
keep_original = False
previous = None

class Recorder:
    def __init__(self):
        object.__setattr__(self, 'writes', [])

    def __setattr__(self, name, value):
        self.writes.append((name, value))

def observe(frame, event, arg):
    if event != 'call' or frame.f_code is not dataclasses._add_slots.__code__:
        return
    original = frame.f_locals['cls']
    if original.__name__ != 'Node':
        return
    weak_originals.append(weakref.ref(original))
    if keep_original:
        originals.append(original)
    receiver = Recorder()
    for name, values in (
        ('__init__', (receiver, object())),
        ('accept', (None, previous if previous is not None else object())),
    ):
        try:
            vars(original)[name](*values)
        except TypeError:
            attempts.append((name, 'rejected', tuple(receiver.writes)))
        else:
            attempts.append((name, 'entered', tuple(receiver.writes)))
# ok
# test_cpython_dataclass_pending_calls_are_ordinary_and_selected_self_fields_are_checked [Any]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('pending_slots_model')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('make_node',):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = "# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nfrom typing import Any\nimport pending_slots_observer as support\n\ndef make_node():\n    @dataclass(slots=True)\n    class Node:\n        next: Any = None\n\n        def accept(self, value: Node) -> Node:\n            support.events.append('accept body')\n            return value\n\n    return Node\n"
checked_field_writes = False

import ctypes
import sys
import types
import pending_slots_model as model
import pending_slots_observer as support
from soac import _soac_ext

def api(name, result):
    f = getattr(ctypes.pythonapi, name)
    f.argtypes = [ctypes.py_object]
    f.restype = result
    return f

owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
sealed = api('PyType_IsSoacSealed', ctypes.c_int)

stock = types.ModuleType('ordinary_pending_slots_control')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary pending slots control>', 'exec'), vars(stock))
support.keep_original = True
old_profile = sys.getprofile()
sys.setprofile(support.observe)
try:
    ordinary_selected = stock.make_node()
finally:
    sys.setprofile(old_profile)
assert not owner(ordinary_selected)
assert [row[:2] for row in support.attempts] == [
    ('__init__', 'entered'), ('accept', 'entered'),
]
assert support.attempts[0][2][0][0] == 'next'
assert support.events == ['accept body']
support.originals.clear()
support.weak_originals.clear()
support.events.clear()
support.attempts.clear()

sys.setprofile(support.observe)
try:
    first = model.make_node()
finally:
    sys.setprofile(old_profile)
assert [row[:2] for row in support.attempts] == [
    ('__init__', 'entered'), ('accept', 'entered'),
]
assert len(support.attempts[0][2]) == 1
assert support.attempts[0][2][0][0] == 'next'
assert support.attempts[0][2] == support.attempts[1][2]
assert support.events == ['accept body']
original = support.originals[0]
assert original is not first and owner(first) and sealed(first)
assert not owner(original), 'unselected original was admitted'
assert original.accept is first.accept and function_owner(first.accept)
assert not metadata(first.accept)
good = first()
assert first.accept(None, good) is good
ordinary = object.__new__(original)
ordinary.unrelated = 'ordinary dictionary after disposal'
assert vars(ordinary)['unrelated'] == 'ordinary dictionary after disposal'
assert original.accept(None, ordinary) is ordinary
assert first.accept(None, ordinary) is ordinary
assert original.accept(None, good) is good

# Repeating the same source creates a distinct class without a call predicate.
support.previous = good
support.events.clear()
support.attempts.clear()
sys.setprofile(support.observe)
try:
    second = model.make_node()
finally:
    sys.setprofile(old_profile)
assert first is not second and owner(second) and sealed(second)
assert [row[:2] for row in support.attempts] == [
    ('__init__', 'entered'), ('accept', 'entered'),
]
assert len(support.attempts[0][2]) == 1
assert support.attempts[0][2][0][0] == 'next'
assert support.attempts[0][2] == support.attempts[1][2]
assert support.events == ['accept body']
assert second.accept(None, good) is good
new = second()
assert second.accept(None, new) is new
assert first.accept(None, good) is good
assert _soac_ext.strict_function_diagnostics(first.accept)['original_code_entered']

from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1
from soac.strict import StrictMutationError
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
diagnostic = _soac_ext.strict_module_diagnostics(model)
for selected in (first, second):
    info = ConstructionInfoV1()
    assert get_construction(selected, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(selected) and info.owner is not None
    assert selected.__slots__ == ("next",)
    assert function_owner(selected.__init__) and not metadata(selected.__init__)
    observed = _assert_cpython_function_witness(
        selected.accept, diagnostic,
    )
    assert observed["original_code_entered"]

if checked_field_writes:
    generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
    generic_set.argtypes = [ctypes.py_object] * 3
    generic_set.restype = ctypes.c_int
    slot = vars(first)["next"]
    assert type(slot) is types.MemberDescriptorType
    good.next = good
    assert good.next is good
    assert generic_set(good, "next", None) == 0 and good.next is None
    assert generic_set(good, "next", good) == 0 and good.next is good
    for wrong in (ordinary, new, object()):
        try:
            first(wrong)
        except TypeError as error:
            assert not isinstance(error, StrictMutationError), error
        else:
            raise AssertionError('generated assignment bypassed the selected Self field')
        for write in (
            lambda: setattr(good, "next", wrong),
            lambda: object.__setattr__(good, "next", wrong),
            lambda: generic_set(good, "next", wrong),
            lambda: slot.__set__(good, wrong),
        ):
            try:
                write()
            except TypeError as error:
                assert not isinstance(error, StrictMutationError), error
            else:
                raise AssertionError("selected Self field accepted a different actual type")
            assert good.next is good
    new.next = new
    assert new.next is new
    # Disposition does not retroactively install the replacement's field
    # predicate on the original ordinary dictionary-backed type.
    ordinary.next = ordinary
    assert vars(ordinary)["next"] is ordinary and not owner(original)
else:
    # An explicit Any field has no predicate even though its class participates.
    unchecked = object()
    good.next = unchecked
    assert good.next is unchecked
    assert first(ordinary).next is ordinary
    assert second(good).next is good

_scenario_check_source_functions()
