# modes:soac,entry
# module:pending_slots_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
from typing import Any
import pending_slots_observer as support

def make_node():
    @dataclass(slots=True)
    class Node:
        next: Node | None = None

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
# test_soac_untraced_slots_preserve_selected_self_fields [Node | None]
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

source = "\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nfrom typing import Any\nimport pending_slots_observer as support\n\ndef make_node():\n    @dataclass(slots=True)\n    class Node:\n        next: Node | None = None\n\n        def accept(self, value: Node) -> Node:\n            support.events.append('accept body')\n            return value\n\n    return Node\n"
checked_field_writes = True
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

import ctypes
import sys
import types
import pending_slots_model as model
import pending_slots_observer as support
from soac import _soac_ext
from soac.strict import StrictMutationError
from tests.test_strict_type_native import ConstructionInfoV1

def api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
sealed = api('PyType_IsSoacSealed', ctypes.c_int)
assert _soac_ext.strict_function_entry_kind(model.make_node) == expected_entry

# CPython observer-positive original/final and pending-call controls stay above.
# SOAC proves actual final Self field ownership without an observer prerequisite.
assert support.attempts == support.originals == support.weak_originals == []
assert support.events == []

first = model.make_node()
assert owner(first) and sealed(first)
assert function_owner(first.accept) and metadata(first.accept)
assert _soac_ext.strict_function_entry_kind(first.accept) == expected_entry
good = first()
assert good.next is None
assert first.accept(None, good) is good
ordinary_return = object()
assert first.accept(None, ordinary_return) is ordinary_return
assert support.events == ['accept body', 'accept body']

# Repeated source construction creates independent actual Self field owners.
support.previous = good
support.events.clear()
second = model.make_node()
assert second is not first and owner(second) and sealed(second)
assert support.events == []
assert second.accept(None, good) is good
new = second()
assert new.next is None
assert second.accept(None, new) is new
assert first.accept(None, good) is good
assert support.events == ['accept body', 'accept body', 'accept body']
assert support.attempts == support.originals == support.weak_originals == []

get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
for selected in (first, second):
    info = ConstructionInfoV1()
    assert get_construction(selected, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(selected) and info.owner is not None
    assert selected.__slots__ == ('next',)
    assert function_owner(selected.__init__)
    assert function_owner(selected.accept) and metadata(selected.accept)
    assert _soac_ext.strict_function_entry_kind(selected.accept) == expected_entry

# The identical ordinary subject retains ordinary annotation behavior.
stock = types.ModuleType('ordinary_soac_pending_slots_control')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary SOAC pending slots control>', 'exec'), vars(stock))
ordinary_type = stock.make_node()
ordinary = ordinary_type()
assert not owner(ordinary_type) and not function_owner(ordinary_type.accept)
wrong_value = object()
ordinary.next = wrong_value
assert ordinary.next is wrong_value and ordinary_type(wrong_value).next is wrong_value

# A foreign generated-init receiver has no protected field, in either source control.
# Its ordinary explicit setattr callback still runs once with the original value.
recorder = support.Recorder()
assert first.__init__(recorder, wrong_value) is None
assert recorder.writes == [('next', wrong_value)]

if checked_field_writes:
    generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
    generic_set.argtypes = [ctypes.py_object] * 3
    generic_set.restype = ctypes.c_int
    slot = vars(first)['next']
    assert type(slot) is types.MemberDescriptorType
    good.next = good
    assert good.next is good
    assert generic_set(good, 'next', None) == 0 and good.next is None
    assert generic_set(good, 'next', good) == 0 and good.next is good
    for wrong in (ordinary, new, wrong_value):
        try:
            first(wrong)
        except TypeError as error:
            assert not isinstance(error, StrictMutationError), error
        else:
            raise AssertionError('generated assignment bypassed the selected Self field')
        for write in (
            lambda: setattr(good, 'next', wrong),
            lambda: object.__setattr__(good, 'next', wrong),
            lambda: generic_set(good, 'next', wrong),
            lambda: slot.__set__(good, wrong),
        ):
            try:
                write()
            except TypeError as error:
                assert not isinstance(error, StrictMutationError), error
            else:
                raise AssertionError('selected Self field accepted a different actual type')
            assert good.next is good
    new.next = new
    assert new.next is new
else:
    # Any is a source-level control with no predicate, not a policy exemption.
    good.next = wrong_value
    assert good.next is wrong_value
    assert first(ordinary).next is ordinary
    assert second(good).next is good
module_state = _soac_ext.strict_module_diagnostics(model)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

_scenario_check_source_functions()
