# Migrated from tests/test_strict_checked_fields.py::test_explicit_fields_check_without_coercion_or_inferred_requirements

# module:checked
# soac: module(strict_assign=true, checked_attr=true)
from typing import Any

class Checked:
    def __init__(self, initial: int = 1):
        self.number: int = initial
        self.flag: bool = False
        self.none_only: None = None
        self.maybe: str | None = None
        self.choice: int | str | None = None
        self.widened: float = 0.0
        self.inferred = initial

    def store_number(self, value: Any) -> None:
        self.number = value

    def store_choice(self, value: Any) -> None:
        self.choice = value

    def read_number(self):
        return self.number

    def read_inferred(self):
        return self.inferred

class SlotChecked:
    __slots__ = ('number', '__weakref__')

    def __init__(self, initial: int = 1):
        self.number: int = initial

    def store_number(self, value: Any) -> None:
        self.number = value

    def read_number(self):
        return self.number

class Defaults:
    number: int = 10

    def read_number(self):
        return self.number

class PredicateFree:
    # Participation does not turn Any or inferred declarations into predicates.
    def __init__(self, initial=1):
        self.payload: Any = initial
        self.inferred = initial

def make_reader(initial):
    class Reader:
        def __init__(self):
            self.value = initial
        def read(self):
            return self.value
    return Reader

# module:unchecked_base
# soac: module(strict_assign=true, checked_attr=true)

# soac: class(checked_attr=false)
class UncheckedBase:
    def __init__(self, initial: int = 1):
        self.inferred = initial
        self.annotation_opted_out: int = initial

# module:disabled_child
# soac: module(strict_assign=true, checked_attr=true)
from checked import Checked

# soac: class(checked_attr=false)
class DisabledChild(Checked):
    def __init__(self):
        super().__init__()
        self.own: int = 3

# module:enabled_child
# soac: module(strict_assign=true, checked_attr=true)
from unchecked_base import UncheckedBase

class EnabledChild(UncheckedBase):
    def __init__(self):
        super().__init__()
        self.own: int = 4

# ok

import ctypes
import checked
import disabled_child
import enabled_child
import unchecked_base

def api(name, count, result=ctypes.c_int):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object] * count
    function.restype = result
    return function

is_sealed = api('PyType_IsSoacSealed', 1)
has_policy = api('PyDict_HasSoacPolicy', 1)
field_index = api('_PyDict_IndexedKeyIndex', 2, ctypes.c_ssize_t)
no_aliases = api('_PyDict_HasNoLookupAliases', 1)
native_setattr = api('PyObject_SetAttr', 3)
native_generic_setattr = api('PyObject_GenericSetAttr', 3)
native_explicit_setattr = api('_PyObject_GenericSetAttrWithDict', 4)
native_setitem = api('PyDict_SetItem', 3)

def c_setattr(receiver, name, value):
    return native_setattr(*(ctypes.py_object(item) for item in (receiver, name, value)))

def c_generic_setattr(receiver, name, value):
    return native_generic_setattr(*(ctypes.py_object(item) for item in (receiver, name, value)))

def c_explicit_setattr(receiver, name, value):
    return native_explicit_setattr(*(ctypes.py_object(item) for item in (receiver, name, value, vars(receiver))))

def c_setitem(dictionary, key, value):
    return native_setitem(*(ctypes.py_object(item) for item in (dictionary, key, value)))

def assert_ordinary_dictionary(dictionary):
    assert type(dictionary) is dict
    # This native query accepts only an indexed table and an exact string key.
    # A literal exact key leaves the ordinary layout as the sole TypeError case.
    try:
        field_index(dictionary, 'number')
    except TypeError as error:
        assert type(error) is TypeError
    else:
        raise AssertionError('ordinary source storage acquired an indexed table')
    # This is an indexed-table guard, not a general no-alias predicate.
    assert no_aliases(dictionary) == 0

def required_error(operation):
    try:
        operation()
    except TypeError:
        return
    raise AssertionError('required field value check was skipped')

def storage(receiver):
    assert is_sealed(type(receiver)) == 1, 'fixture fell back to an ordinary class'
    dictionary = vars(receiver)
    assert type(dictionary) is dict and has_policy(dictionary) == 1
    return dictionary

def assert_cpython_contracts():
    if __dp_integration_mode__ != "cpython":
        return

    from soac import _soac_ext
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = api("PyType_GetSoacContractOwner", 1, ctypes.c_void_p)
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int
    # The scenario runner independently authenticates every module's actual
    # path, source digest, generation and native entry before and after this
    # block. Retain the original additional class/method witnesses here.
    for name, selected, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(selected)
        for cls in classes:
            if not participates:
                assert is_sealed(cls) == 0 and get_type_owner(cls) is None
                continue
            info = ConstructionInfoV1()
            assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
            assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
            assert info.phase == 3 and info.permanent_contract_published == 1
            assert info.owner == get_type_owner(cls) and info.owner is not None
        for function in {
            "checked": (
                checked.Checked.__init__,
                checked.Checked.store_number,
                checked.Checked.store_choice,
                checked.Checked.read_number,
                checked.Defaults.read_number,
                checked.PredicateFree.__init__,
            ),
            "unchecked_base": (unchecked_base.UncheckedBase.__init__,),
            "disabled_child": (disabled_child.DisabledChild.__init__,),
            "enabled_child": (enabled_child.EnabledChild.__init__,),
        }[name]:
            observed = _assert_cpython_function_witness(function, diagnostic)
            assert observed["finalized"] is participates


assert_cpython_contracts()

value = checked.Checked()
dictionary = storage(value)
assert_ordinary_dictionary(dictionary)
class Integer(int):
    pass
for accepted in (True, 17, Integer(19), 10 ** 100):
    value.number = accepted
    assert value.number is accepted
    value.widened = accepted
    assert value.widened is accepted
floating = float('2.5')
value.widened = floating
assert value.widened is floating
for name, invalid in [('number', 1.5), ('flag', 1), ('none_only', 0),
                      ('maybe', 1), ('choice', []), ('widened', '2.5')]:
    previous = getattr(value, name)
    required_error(lambda: setattr(value, name, invalid))
    assert getattr(value, name) is previous
for accepted in (None, 'text'):
    value.maybe = accepted
    assert value.maybe is accepted
for accepted in (None, 'text', 8, True):
    value.store_choice(accepted)
    assert value.choice is accepted
required_error(lambda: value.store_choice([]))
required_error(lambda: value.store_number('bad through a transformed method'))
assert dictionary is vars(value)
assert_ordinary_dictionary(dictionary)

# This ordinary field has an inferred integer type, but the source
# never selected a mandatory annotation or indexed storage for it.
marker = object()
value.inferred = marker
assert value.inferred is marker
dictionary['inferred'] = 'still unchecked'
assert value.inferred == 'still unchecked'

defaults = checked.Defaults()
default_storage = storage(defaults)
assert defaults.number == 10 and default_storage == {}
required_error(lambda: setattr(defaults, 'number', 'bad default shadow'))
defaults.number = 2
del defaults.number
assert defaults.number == 10 and default_storage == {}

if __dp_integration_mode__ == "cpython":
    # Repeated original-code operations must retain checks without SOAC compilation.
    warmed = checked.Checked()
    for item in range(128):
        warmed.store_number(item)
        assert warmed.read_number() == item
    required_error(lambda: warmed.store_number("bad after native warmup"))
    assert warmed.number == 127
    required_error(lambda: c_generic_setattr(warmed, "number", "bad generic C write"))
    assert c_generic_setattr(warmed, "number", 129) == 0
    assert warmed.read_number() == 129

assert_cpython_contracts()

