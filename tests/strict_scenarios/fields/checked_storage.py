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
# test_attribute_unicode_payload_and_original_lookup_errors
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
operations = (setattr, object.__setattr__, c_setattr,
              c_generic_setattr, c_explicit_setattr)
for operation in operations:
    value = checked.Checked()
    dictionary = storage(value)
    original_items = tuple(dictionary.items())
    del value.number
    events = []
    class Name(str):
        def __hash__(self):
            events.append('hash')
            return str.__hash__(self)
        def __eq__(self, other):
            events.append('eq')
            return str.__eq__(self, other)
        def __str__(self):
            raise AssertionError('checking invoked user string conversion')
    name = Name('number')
    class Ordinary:
        pass
    ordinary = Ordinary()
    # Equal contents alone omit the deleted name in shared-key history.
    # Reproduce original insertion order, materialization, then deletion.
    for attr_name, original_value in original_items:
        setattr(ordinary, attr_name, original_value)
    ordinary_dictionary = vars(ordinary)
    del ordinary.number
    assert ordinary_dictionary == dictionary
    operation(ordinary, name, 'ordinary value')
    expected = list(events)
    events.clear()
    required_error(lambda: operation(value, name, 'bad subclass-name value'))
    assert events == expected, (events, expected)
    assert 'number' not in dictionary
    events.clear()
    operation(value, name, 7)
    assert events == expected, (events, expected)
    assert list(dictionary)[-1] is name and not no_aliases(dictionary)

    original = ValueError('name hash failure')
    class BadHash(str):
        def __hash__(self):
            raise original
    try:
        operation(value, BadHash('number'), 'bad value')
    except ValueError as error:
        assert error is original
    else:
        raise AssertionError('native name hash failure was replaced')

# The stored alias is resolved by the dictionary, after descriptor
# lookup. Its error must precede a required field-value error, including
# through the transformed source method's own attribute store.
for operation in (*operations, lambda obj, name, item: obj.store_number(item)):
    value = checked.Checked()
    dictionary = storage(value)
    del value.number
    events = []
    original = ValueError('original equality failure')
    reject_lookup = False
    class Alias:
        def __hash__(self):
            return hash('number')
        def __eq__(self, other):
            events.append('eq')
            if reject_lookup:
                raise original
            return other == 'number'
    alias = Alias()
    # Shared-key insertion may compare the deleted canonical name.
    # Arm the intended lookup failure only after the alias exists.
    dictionary[alias] = 1
    reject_lookup = True
    for unused in range(2):
        events.clear()
        try:
            operation(value, 'number', 'bad field value')
        except ValueError as error:
            assert error is original
        else:
            raise AssertionError('lookup exception was hidden by a value precheck')
        assert events == ['eq'], events
        assert list(dictionary)[-1] is alias
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
# ok
# test_mapping_aliases_and_supported_c_writes_use_the_actual_policy
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
value = checked.Checked()
dictionary = storage(value)
assert_ordinary_dictionary(dictionary)
events = []
class Alias:
    def __hash__(self):
        return hash('number')
    def __eq__(self, other):
        events.append('eq')
        return other == 'number'
alias = Alias()
for operation in (lambda item: dictionary.__setitem__(alias, item),
                  lambda item: c_setitem(dictionary, alias, item)):
    previous = dictionary['number']
    events.clear()
    required_error(lambda: operation('bad canonical field value'))
    assert events == ['eq'], events
    assert dictionary['number'] == previous
    assert all(type(key) is str for key in dictionary)
    events.clear()
    operation(2)
    assert events == ['eq'] and dictionary['number'] == 2
required_error(lambda: c_setattr(value, 'number', 'bad C attribute value'))
required_error(lambda: dictionary.update({'number': 'bad bulk value'}))
assert dictionary.setdefault('number', 'ignored non-write') == 2
del value.number
required_error(lambda: dictionary.setdefault('number', 'now a checked insert'))
assert_ordinary_dictionary(dictionary)

# A new arbitrary mapping key is not normalized into an attribute name.
# Once it aliases reads, a source attribute write still checks its
# original name; a resolved attribute operation must not become SET.
dictionary[alias] = 'ordinary alias-sensitive overflow'
assert list(dictionary)[-1] is alias and not no_aliases(dictionary)
for operation in (lambda: setattr(value, 'number', 'bad attribute value'),
                  lambda: value.store_number('bad transformed attribute value'),
                  lambda: c_setattr(value, 'number', 'bad C attribute value')):
    events.clear()
    required_error(operation)
    assert events == ['eq'], events
value.store_number(9)
assert value.number == 9 and list(dictionary)[-1] is alias

copied = dictionary.copy()
assert type(copied) is dict and not has_policy(copied)
copied['number'] = 'a copy has no source storage authority'
dictionary.clear()
assert dictionary is vars(value) and dictionary == {}
assert_ordinary_dictionary(dictionary)
required_error(lambda: c_setitem(dictionary, 'number', 'bad after clear'))
value.number = 12
assert value.number == 12
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
# ok
# test_string_subclass_dictionary_keys_keep_selected_field_checks
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
events = []
class Name(str):
    def __str__(self):
        raise AssertionError('field checking must use the Unicode payload')
    def __hash__(self):
        events.append('hash')
        return str.__hash__(self)
    def __eq__(self, other):
        events.append('eq')
        return str.__eq__(self, other)

writes = (
    lambda mapping, item: mapping.__setitem__('number', item),
    lambda mapping, item: c_setitem(mapping, 'number', item),
    lambda mapping, item: mapping.update({'number': item}),
    lambda mapping, item: mapping.update([('number', item)]),
    lambda mapping, item: mapping.__ior__({'number': item}),
)
for write in writes:
    value = checked.Checked()
    dictionary = storage(value)
    dictionary.clear()
    key = Name('number')
    dictionary[key] = 7
    assert list(dictionary)[0] is key

    # The ordinary control supplies the lookup callback schedule. The
    # selected policy must not rerun hashing, equality or conversion.
    ordinary = {key: 7}
    events.clear()
    write(ordinary, 'ordinary untyped value')
    expected_events = events.copy()
    assert ordinary['number'] == 'ordinary untyped value'
    events.clear()
    required_error(lambda: write(dictionary, 'bad selected value'))
    assert events == expected_events, (events, expected_events)
    assert dictionary['number'] == 7 and value.number == 7
    assert list(dictionary)[0] is key
    write(dictionary, 9)
    assert value.number == 9

dictionary.clear()
required_error(lambda: c_setitem(dictionary, Name('number'), 'bad C insert'))
assert dictionary == {}
required_error(lambda: dictionary.setdefault(Name('number'), 'bad insert'))
assert dictionary == {}
assert dictionary.setdefault(Name('number'), 11) == 11
# A setdefault hit is a read, not a forbidden write of its default.
assert dictionary.setdefault('number', 'unused default') == 11
assert value.number == 11

# Initial policy validation must check subclass keys in the actual
# incoming dictionary, before replacing the receiver's storage.
incoming_key = Name('number')
rejected = {incoming_key: 'bad incoming value'}
required_error(lambda: setattr(value, '__dict__', rejected))
assert vars(value) is dictionary and value.number == 11
assert list(rejected)[0] is incoming_key
accepted = {incoming_key: 13}
value.__dict__ = accepted
assert vars(value) is accepted and value.number == 13
required_error(lambda: c_setitem(accepted, 'number', 'bad after admission'))
assert accepted['number'] == 13 and list(accepted)[0] is incoming_key
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
# ok
# test_fresh_checked_storage_has_shared_direct_state_and_plain_copies
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
import _testinternalcapi
import gc
import weakref
info = _testinternalcapi.get_soac_type_state_info

first, second = checked.Checked(), checked.Checked()
assert _testinternalcapi.has_inline_values(first)
first_info, second_info = info(first), info(second)
assert first_info['has_slot'] and second_info['has_slot']
assert first_info['storage_mode'] == second_info['storage_mode'] == 'direct'
assert first_info['extra_slot_bytes'] == ctypes.sizeof(ctypes.c_void_p)
assert first_info['state_id'] == second_info['state_id']
assert first_info['dictionary_state_id'] == second_info['dictionary_state_id']

def store(receiver, value):
    receiver.number = value

for value in range(400):
    store(first, value)
required_error(lambda: store(first, 'wrong after warmup'))
assert first.number == 399
assert _testinternalcapi.has_inline_values(first)
dictionary, sibling = vars(first), vars(second)
assert info(dictionary)['has_slot'] and info(sibling)['has_slot']
assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
assert info(sibling)['state_id'] == first_info['dictionary_state_id']
assert info(dictionary)['storage_mode'] == 'direct'
assert_ordinary_dictionary(dictionary)
required_error(lambda: c_setitem(dictionary, 'number', 'wrong'))
assert dictionary['number'] == first.number == 399

# Force the actual dictionary tp_clear path, not merely refcount destruction
# or dict.clear(). Retiring one attachment must not clear shared rule owners.
class Marker:
    pass

third = checked.Checked()
cyclic = vars(third)
assert info(cyclic)['state_id'] == first_info['dictionary_state_id']
marker = Marker()
marker_ref, third_ref = weakref.ref(marker), weakref.ref(third)
cyclic['cycle'] = cyclic
cyclic['marker'] = marker
del marker, third, cyclic
gc.collect()
assert marker_ref() is third_ref() is None
assert info(sibling)['state_id'] == first_info['dictionary_state_id']
assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
second.number = 41
c_setitem(sibling, 'number', 43)
required_error(lambda: setattr(second, 'number', 'wrong after sibling GC'))
required_error(lambda: c_setitem(sibling, 'number', 'wrong after sibling GC'))
assert second.number == 43

# The same exact Python dict type has two allocation forms. Copying values
# does not adopt the source dictionary's constraints or allocate a null tail.
plain = {}
copied = dictionary.copy()
for ordinary in (plain, copied):
    assert type(ordinary) is type(dictionary) is dict
    assert not info(ordinary)['has_slot']
    assert info(ordinary)['extra_slot_bytes'] == 0
    assert not info(ordinary)['state_id']
    assert info(ordinary)['storage_mode'] == 'ordinary'
    assert has_policy(ordinary) == 0
    ordinary['number'] = 'ordinary value'

# Even empty/atomic-valued selected storage needs its state before exposure.
empty = checked.Defaults()
assert info(empty)['has_slot'] and _testinternalcapi.has_inline_values(empty)
empty_dictionary = vars(empty)
assert not empty_dictionary and info(empty_dictionary)['has_slot']
required_error(lambda: c_setitem(empty_dictionary, 'number', 'wrong'))
assert not empty_dictionary and empty.number == 10

# An admitted class with only Any/inferred fields needs no storage predicate.
# This is a source-type control, not the removed sealed-but-disabled policy.
unchecked = checked.PredicateFree()
assert is_sealed(type(unchecked)) == 1
assert not info(unchecked)['has_slot']
assert not info(vars(unchecked))['has_slot']
unchecked.payload = 'Any does not impose a value predicate'
unchecked.inferred = object()
from soac.strict import StrictMutationError
original_code = checked.PredicateFree.__init__.__code__
try:
    checked.PredicateFree.__init__.__code__ = original_code
except StrictMutationError:
    pass
else:
    raise AssertionError('no storage bit disabled the installed method seal')
assert checked.PredicateFree.__init__.__code__ is original_code
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
# ok
# test_type_state_keeps_legacy_replacement_and_custom_allocation_enforced
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
import _testinternalcapi
import gc
import weakref
info = _testinternalcapi.get_soac_type_state_info

first, second = checked.Checked(), checked.Checked()
escaped = vars(first)
escaped_state = info(escaped)['state_id']
incoming = {'number': 7}
incoming_identity = id(incoming)
assert not info(incoming)['has_slot'] and has_policy(incoming) == 0
object.__setattr__(first, '__dict__', incoming)
object.__setattr__(second, '__dict__', incoming)
assert vars(first) is vars(second) is incoming and id(incoming) == incoming_identity
assert not info(incoming)['has_slot']
assert info(incoming)['extra_slot_bytes'] == 0
assert info(incoming)['storage_mode'] == 'legacy'
assert has_policy(incoming) == has_policy(escaped) == 1
assert info(escaped)['state_id'] == escaped_state
assert info(escaped)['storage_mode'] == 'direct'
c_setitem(incoming, 'number', 9)
assert first.number == second.number == 9
for dictionary in (incoming, escaped):
    required_error(lambda: c_setitem(dictionary, 'number', 'wrong'))
    dictionary.clear()
    required_error(lambda: c_setitem(dictionary, 'number', 'wrong after clear'))
    c_setitem(dictionary, 'number', 11)
first_ref, second_ref = weakref.ref(first), weakref.ref(second)
del first, second
gc.collect()
assert first_ref() is second_ref() is None
required_error(lambda: c_setitem(incoming, 'number', 'wrong after receiver death'))
required_error(lambda: c_setitem(escaped, 'number', 'wrong after receiver death'))

# A custom constructor is explicitly outside fresh direct-state admission.
# Calling the ordinary allocator inside it cannot waive inherited constraints.
events = []
class Custom(checked.Checked):
    def __new__(cls):
        result = object.__new__(cls)
        events.append('created')
        required_error(lambda: setattr(result, 'number', 'wrong before return'))
        return result

custom = Custom()
assert events == ['created'] and custom.number == 1
assert is_sealed(Custom) == 0
assert not info(custom)['has_slot']
legacy = vars(custom)
assert not info(legacy)['has_slot'] and has_policy(legacy) == 1
assert info(legacy)['storage_mode'] == 'legacy'
required_error(lambda: c_setattr(custom, 'number', 'wrong'))
required_error(lambda: c_setitem(legacy, 'number', 'wrong'))
assert custom.number == 1
if __dp_integration_mode__ == 'cpython':
    from soac import _soac_ext
    from tests._strict_integration import (
        _assert_cpython_function_witness, _assert_cpython_module_witness,
    )
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int

    for name, module, classes, participates in (
        ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
        ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
        ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
        ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
    ):
        diagnostic = _soac_ext.strict_module_diagnostics(module)
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
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is participates
