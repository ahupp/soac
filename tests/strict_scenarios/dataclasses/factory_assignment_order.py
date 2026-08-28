# modes:soac,entry
# module:factory_site_support
events = []
keyword_value = 3
positional_value = 4

def items() -> list[int]:
    events.append('items')
    return []

def keyword() -> int:
    events.append('keyword')
    return keyword_value

def positional() -> int:
    events.append('positional')
    return positional_value
# module:mixed_factory_site
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass, field
import factory_site_support as support

@dataclass
class Value:
    checked: int = 1
    items: list[int] = field(default_factory=support.items)
# module:ordered_factory_sites
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass, field
import factory_site_support as support

@dataclass
class Value:
    keyword: int = field(default_factory=support.keyword, kw_only=True)
    positional: int = field(default_factory=support.positional)
# ok
# test_generated_dataclass_preserves_independent_mutable_factory_results [default]
import sys
from soac import _soac_ext
from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ctypes
import factory_site_support as support

def adopted(cls):
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    assert owner(cls) and function_owner(cls.__init__)

from mixed_factory_site import Value
adopted(Value)
support.events.clear()
class Foreign:
    pass
foreign = Foreign()
assert Value.__init__(foreign, 'ordinary') is None
assert foreign.checked == 'ordinary' and foreign.items == []
assert support.events == ['items']
support.events.clear()
first, second = Value(), Value()
assert first.checked == 1 and first.items == second.items == []
assert first.items is not second.items and support.events == ['items', 'items']
support.events.clear()
unselected = object()
assert Value(2, unselected).items is unselected
assert support.events == [], 'an explicitly supplied unchecked field invoked its factory'
# ok
# test_generated_factories_follow_assignment_order_not_parameter_order [default]
import sys
from soac import _soac_ext
from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ctypes
import factory_site_support as support

def adopted(cls):
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    assert owner(cls) and function_owner(cls.__init__)

from ordered_factory_sites import Value
adopted(Value)
support.events.clear()
value = Value()
assert value.keyword == 3 and value.positional == 4
assert support.events == ['keyword', 'positional']
class Foreign:
    pass
foreign = Foreign()
support.keyword_value = 'ordinary'
support.events.clear()
assert Value.__init__(foreign) is None
assert foreign.keyword == 'ordinary' and foreign.positional == 4
assert support.events == ['keyword', 'positional']
support.events.clear()
value = Value(9, keyword=8)
assert value.positional == 9 and value.keyword == 8
assert support.events == []
# ok
# test_generated_dataclass_field_rejections_preserve_factory_prefix_effects [default]
import sys
from soac import _soac_ext
from soac.strict import StrictMutationError

def field_write_rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError)
        return
    raise AssertionError('selected instance storage accepted an incompatible value')

import ctypes
import factory_site_support as support

def adopted(cls):
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    assert owner(cls) and function_owner(cls.__init__)

from mixed_factory_site import Value as Mixed
from ordered_factory_sites import Value as Ordered
adopted(Mixed)
adopted(Ordered)

# The first actual store fails before the later items factory is evaluated.
mixed = object.__new__(Mixed)
support.events.clear()
field_write_rejected(lambda: Mixed.__init__(mixed, 'wrong'))
assert vars(mixed) == {} and support.events == []

# Generated stores follow source field order, not constructor parameter order.
# Rejection retains completed effects and prevents only the incompatible store.
ordered = object.__new__(Ordered)
support.keyword_value = 'wrong'
support.events.clear()
field_write_rejected(lambda: Ordered.__init__(ordered))
assert support.events == ['keyword'] and vars(ordered) == {}

support.keyword_value = 3
support.positional_value = 'wrong'
support.events.clear()
field_write_rejected(lambda: Ordered.__init__(ordered))
assert support.events == ['keyword', 'positional']
assert vars(ordered) == {'keyword': 3}

support.positional_value = 4
support.events.clear()
assert Ordered.__init__(ordered) is None
assert vars(ordered) == {'keyword': 3, 'positional': 4}
assert support.events == ['keyword', 'positional']
