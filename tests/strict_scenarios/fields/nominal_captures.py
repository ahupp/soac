# modes:soac,entry
# module:nominal_fields
# soac: module(strict_assign=true, checked_attr=true)
from nominal_field_probe import OrdinaryTarget, change_target, observe

class MutableHolder:
    payload: OrdinaryTarget

    def read(self) -> OrdinaryTarget:
        return self.payload

def family():
    class Target:
        pass
    class Holder:
        payload: Target
    def replace_target(value: type[Target]):
        nonlocal Target
        Target = value
    return Target, Holder, replace_target

def method_family():
    class Target:
        pass
    class Holder:
        def __init__(self, value):
            self.payload: Target = value
    return Target, Holder

def method_family_with_body_callback():
    class Target:
        pass
    original = Target
    def replace_target(value: type[Target]):
        nonlocal Target
        Target = value
    class Holder:
        change_target(replace_target)
        def __init__(self, value):
            self.payload: Target = value
    return original, Holder, replace_target

def captured_method_family():
    class Target:
        pass
    def make_holder():
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Target, Holder
    return make_holder

def mixed_storage_family():
    class DictionaryTarget:
        pass
    class MemberTarget:
        pass
    class Holder:
        __slots__ = ('native', '__dict__')
        payload: DictionaryTarget
        native: MemberTarget
    return DictionaryTarget, MemberTarget, Holder

def uncaptured_method_family():
    class Target:
        pass
    def make_holder():
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Holder
    return Target, make_holder

def namespace_method_family():
    class Target:
        pass
    class Outer:
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
    return Target, Outer.Holder

class ProbeBase:
    def __init_subclass__(cls):
        observe(cls)

def self_family():
    class SelfHolder(ProbeBase):
        payload: SelfHolder | None
    return SelfHolder
# module:nominal_field_probe
from typing import Any

observed = []

class OrdinaryTarget:
    pass

def change_target(replace: Any) -> None:
    replace(OrdinaryTarget)

def observe(cls: Any) -> None:
    import ctypes
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    from soac.strict import StrictMutationError
    assert not owner(cls), 'the provisional field target was permanently admitted'
    try:
        object.__new__(cls)
    except StrictMutationError:
        pass
    else:
        raise AssertionError('self field allowed allocation before final selection')
    observed.append(cls)
# ok
# test_method_only_field_annotation_uses_an_explicit_construction_capture
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
import nominal_fields as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
set_item = ctypes.pythonapi.PyDict_SetItem
set_item.argtypes = [ctypes.py_object] * 3
set_item.restype = ctypes.c_int
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object] * 3
generic_set.restype = ctypes.c_int
module_state = _soac_ext.strict_module_diagnostics(module)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('a required nominal field accepted a foreign value')

def reject_all_writes(instance, wrong):
    previous = instance.payload
    for operation in (
        lambda: setattr(instance, 'payload', wrong),
        lambda: object.__setattr__(instance, 'payload', wrong),
        lambda: generic_set(instance, 'payload', wrong),
        lambda: vars(instance).__setitem__('payload', wrong),
        lambda: vars(instance).update(payload=wrong),
        lambda: set_item(vars(instance), 'payload', wrong),
    ):
        rejected(operation)
        assert instance.payload is previous
assert _soac_ext.strict_function_entry_kind(module.method_family) == expected_entry

first_target, first_holder = module.method_family()
second_target, second_holder = module.method_family()
assert owner(first_holder) and owner(second_holder)
assert '__annotate__' not in vars(first_holder)
assert first_holder.__init__.__annotate__ is None
assert first_holder.__init__.__closure__ is None
first, second = first_holder(first_target()), second_holder(second_target())
reject_all_writes(first, second.payload)
reject_all_writes(second, first.payload)
rejected(lambda: first_holder(second_target()))

# Private compiler cells must not become extra lifetime edges from the class
# or its source function. Only the required nominal target is retained by an
# escaped dictionary's permanent write policy.
import gc
import weakref
def escaped_dictionary():
    target, holder = module.method_family()
    instance = holder(target())
    dictionary = vars(instance)
    del dictionary['payload']
    return weakref.ref(target), weakref.ref(holder), dictionary
target_ref, holder_ref, dictionary = escaped_dictionary()
gc.collect()
assert holder_ref() is None
assert target_ref() is not None
rejected(lambda: set_item(dictionary, 'payload', object()))
del dictionary
gc.collect()
assert target_ref() is None
# ok
# test_private_field_captures_read_original_cells_after_the_namespace_body
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
import nominal_fields as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
set_item = ctypes.pythonapi.PyDict_SetItem
set_item.argtypes = [ctypes.py_object] * 3
set_item.restype = ctypes.c_int
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object] * 3
generic_set.restype = ctypes.c_int
module_state = _soac_ext.strict_module_diagnostics(module)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('a required nominal field accepted a foreign value')

def reject_all_writes(instance, wrong):
    previous = instance.payload
    for operation in (
        lambda: setattr(instance, 'payload', wrong),
        lambda: object.__setattr__(instance, 'payload', wrong),
        lambda: generic_set(instance, 'payload', wrong),
        lambda: vars(instance).__setitem__('payload', wrong),
        lambda: vars(instance).update(payload=wrong),
        lambda: set_item(vars(instance), 'payload', wrong),
    ):
        rejected(operation)
        assert instance.payload is previous

import nominal_field_probe as probe
original, holder, replace = module.method_family_with_body_callback()
assert owner(holder)
assert holder.__init__.__closure__ is None
assert holder.__init__.__annotate__ is None
instance = holder(probe.OrdinaryTarget())
reject_all_writes(instance, original())

# Cell identities were captured before construction, but their values were
# read after the ordinary class body changed Target. A later change cannot
# revise the committed predicate.
replace(original)
instance.payload = probe.OrdinaryTarget()
reject_all_writes(instance, original())
# ok
# test_nominal_field_cells_forward_through_the_actual_lexical_owner[captured_function]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
import nominal_fields as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
set_item = ctypes.pythonapi.PyDict_SetItem
set_item.argtypes = [ctypes.py_object] * 3
set_item.restype = ctypes.c_int
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object] * 3
generic_set.restype = ctypes.c_int
module_state = _soac_ext.strict_module_diagnostics(module)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('a required nominal field accepted a foreign value')

def reject_all_writes(instance, wrong):
    previous = instance.payload
    for operation in (
        lambda: setattr(instance, 'payload', wrong),
        lambda: object.__setattr__(instance, 'payload', wrong),
        lambda: generic_set(instance, 'payload', wrong),
        lambda: vars(instance).__setitem__('payload', wrong),
        lambda: vars(instance).update(payload=wrong),
        lambda: set_item(vars(instance), 'payload', wrong),
    ):
        rejected(operation)
        assert instance.payload is previous
scope = 'captured_function'

def construct():
    if scope == 'captured_function':
        factory = module.captured_method_family()
        assert factory.__code__.co_freevars == ('Target',)
        target, holder = factory()
        assert factory.__closure__[0].cell_contents is target
        return target, holder
    if scope == 'private_function':
        target, factory = module.uncaptured_method_family()
        assert factory.__code__.co_freevars == ()
        assert factory.__closure__ is None
        assert factory.__annotate__ is None
        assert module.uncaptured_method_family.__code__.co_cellvars == ()
        return target, factory()
    assert module.namespace_method_family.__code__.co_cellvars == ()
    return module.namespace_method_family()

first_target, first_holder = construct()
second_target, second_holder = construct()
assert owner(first_holder) and owner(second_holder)
assert first_target is not second_target and first_holder is not second_holder
assert first_holder.__init__.__closure__ is None
assert first_holder.__init__.__annotate__ is None
first, second = first_holder(first_target()), second_holder(second_target())
reject_all_writes(first, second.payload)
reject_all_writes(second, first.payload)
rejected(lambda: first_holder(second_target()))
# ok
# test_nominal_field_cells_forward_through_the_actual_lexical_owner[private_function]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
import nominal_fields as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
set_item = ctypes.pythonapi.PyDict_SetItem
set_item.argtypes = [ctypes.py_object] * 3
set_item.restype = ctypes.c_int
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object] * 3
generic_set.restype = ctypes.c_int
module_state = _soac_ext.strict_module_diagnostics(module)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('a required nominal field accepted a foreign value')

def reject_all_writes(instance, wrong):
    previous = instance.payload
    for operation in (
        lambda: setattr(instance, 'payload', wrong),
        lambda: object.__setattr__(instance, 'payload', wrong),
        lambda: generic_set(instance, 'payload', wrong),
        lambda: vars(instance).__setitem__('payload', wrong),
        lambda: vars(instance).update(payload=wrong),
        lambda: set_item(vars(instance), 'payload', wrong),
    ):
        rejected(operation)
        assert instance.payload is previous
scope = 'private_function'

def construct():
    if scope == 'captured_function':
        factory = module.captured_method_family()
        assert factory.__code__.co_freevars == ('Target',)
        target, holder = factory()
        assert factory.__closure__[0].cell_contents is target
        return target, holder
    if scope == 'private_function':
        target, factory = module.uncaptured_method_family()
        assert factory.__code__.co_freevars == ()
        assert factory.__closure__ is None
        assert factory.__annotate__ is None
        assert module.uncaptured_method_family.__code__.co_cellvars == ()
        return target, factory()
    assert module.namespace_method_family.__code__.co_cellvars == ()
    return module.namespace_method_family()

first_target, first_holder = construct()
second_target, second_holder = construct()
assert owner(first_holder) and owner(second_holder)
assert first_target is not second_target and first_holder is not second_holder
assert first_holder.__init__.__closure__ is None
assert first_holder.__init__.__annotate__ is None
first, second = first_holder(first_target()), second_holder(second_target())
reject_all_writes(first, second.payload)
reject_all_writes(second, first.payload)
rejected(lambda: first_holder(second_target()))
# ok
# test_nominal_field_cells_forward_through_the_actual_lexical_owner[class_namespace]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
import nominal_fields as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
set_item = ctypes.pythonapi.PyDict_SetItem
set_item.argtypes = [ctypes.py_object] * 3
set_item.restype = ctypes.c_int
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object] * 3
generic_set.restype = ctypes.c_int
module_state = _soac_ext.strict_module_diagnostics(module)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('a required nominal field accepted a foreign value')

def reject_all_writes(instance, wrong):
    previous = instance.payload
    for operation in (
        lambda: setattr(instance, 'payload', wrong),
        lambda: object.__setattr__(instance, 'payload', wrong),
        lambda: generic_set(instance, 'payload', wrong),
        lambda: vars(instance).__setitem__('payload', wrong),
        lambda: vars(instance).update(payload=wrong),
        lambda: set_item(vars(instance), 'payload', wrong),
    ):
        rejected(operation)
        assert instance.payload is previous
scope = 'class_namespace'

def construct():
    if scope == 'captured_function':
        factory = module.captured_method_family()
        assert factory.__code__.co_freevars == ('Target',)
        target, holder = factory()
        assert factory.__closure__[0].cell_contents is target
        return target, holder
    if scope == 'private_function':
        target, factory = module.uncaptured_method_family()
        assert factory.__code__.co_freevars == ()
        assert factory.__closure__ is None
        assert factory.__annotate__ is None
        assert module.uncaptured_method_family.__code__.co_cellvars == ()
        return target, factory()
    assert module.namespace_method_family.__code__.co_cellvars == ()
    return module.namespace_method_family()

first_target, first_holder = construct()
second_target, second_holder = construct()
assert owner(first_holder) and owner(second_holder)
assert first_target is not second_target and first_holder is not second_holder
assert first_holder.__init__.__closure__ is None
assert first_holder.__init__.__annotate__ is None
first, second = first_holder(first_target()), second_holder(second_target())
reject_all_writes(first, second.payload)
reject_all_writes(second, first.payload)
rejected(lambda: first_holder(second_target()))
