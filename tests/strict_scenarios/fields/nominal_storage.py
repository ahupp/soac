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
# test_nominal_fields_bind_actual_factory_targets_and_survive_alias_mutation
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
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
if __dp_integration_mode__ == 'cpython':
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1

    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int
    is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
    is_sealed.argtypes = [ctypes.py_object]
    is_sealed.restype = ctypes.c_int

    def assert_native_class(cls):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
        assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
        assert info.phase == 3 and info.permanent_contract_published == 1
        assert info.owner == owner(cls) and info.owner is not None
        assert is_sealed(cls) == 1

    assert_native_class(module.MutableHolder)
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    observed_read = _assert_cpython_function_witness(
        module.MutableHolder.read, diagnostic,
    )
    assert observed_read["finalized"]
first_target, first_holder, replace = module.family()
second_target, second_holder, unused = module.family()
assert first_target is not second_target and first_holder is not second_holder
assert owner(first_holder) and owner(second_holder)
first, second = first_holder(), second_holder()
first.payload = first_target()
second.payload = second_target()
reject_all_writes(first, second.payload)
reject_all_writes(second, first.payload)
class Ordinary(first_target):
    pass
assert not owner(Ordinary)
first.payload = Ordinary()
replace(second_target)
first.payload = first_target()
reject_all_writes(first, second_target())
assert set_item(vars(first), 'payload', Ordinary()) == 0
assert isinstance(first.payload, Ordinary)
if __dp_integration_mode__ == 'cpython':
    assert_native_class(first_holder)
    assert_native_class(second_holder)
    has_policy = ctypes.pythonapi.PyDict_HasSoacPolicy
    has_policy.argtypes = [ctypes.py_object]
    has_policy.restype = ctypes.c_int
    set_dictionary = ctypes.pythonapi.PyObject_GenericSetDict
    set_dictionary.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.c_void_p]
    set_dictionary.restype = ctypes.c_int

    # The actual supplied dictionary, not a cloned or indexed surrogate, acquires
    # this class execution's predicate. An already-escaped old dictionary keeps it.
    escaped = vars(first)
    accepted = first_target()
    incoming = {"payload": accepted, "extra": object()}
    assert has_policy(incoming) == 0
    first.__dict__ = incoming
    assert vars(first) is incoming and first.payload is accepted
    assert has_policy(incoming) == 1 and has_policy(escaped) == 1
    reject_all_writes(first, second.payload)
    rejected(lambda: set_item(escaped, "payload", second.payload))

    # Compatible receivers may share that exact dictionary through the public C
    # setter; both ordinary attribute and raw dictionary C writes stay checked.
    alias = first_holder()
    assert set_dictionary(alias, incoming, None) == 0
    assert vars(alias) is incoming and vars(first) is incoming
    replacement = first_target()
    assert generic_set(alias, "payload", replacement) == 0
    assert first.payload is replacement and alias.payload is replacement
    assert set_item(incoming, "payload", accepted) == 0
    assert first.payload is accepted and alias.payload is accepted
    reject_all_writes(alias, second.payload)

    # Refusal validates the incoming contents before installing a policy or
    # replacing either receiver's authoritative dictionary.
    invalid = {"payload": second.payload}
    invalid_items = tuple(invalid.items())
    rejected(lambda: set_dictionary(first, invalid, None))
    assert vars(first) is incoming and vars(alias) is incoming
    assert tuple(invalid.items()) == invalid_items and has_policy(invalid) == 0
    unrestricted = object()
    assert set_item(invalid, "payload", unrestricted) == 0
    assert invalid["payload"] is unrestricted
    assert first.payload is accepted and alias.payload is accepted
if __dp_integration_mode__ == 'cpython':
    observed = _assert_cpython_function_witness(module.family, diagnostic)
    assert observed['original_code_entered']
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
# ok
# test_inherited_nominal_field_constraints_do_not_merge_equal_source_targets
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
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
if __dp_integration_mode__ == 'cpython':
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1

    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int
    is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
    is_sealed.argtypes = [ctypes.py_object]
    is_sealed.restype = ctypes.c_int

    def assert_native_class(cls):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
        assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
        assert info.phase == 3 and info.permanent_contract_published == 1
        assert info.owner == owner(cls) and info.owner is not None
        assert is_sealed(cls) == 1

    assert_native_class(module.MutableHolder)
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    observed_read = _assert_cpython_function_witness(
        module.MutableHolder.read, diagnostic,
    )
    assert observed_read["finalized"]
first_target, first_holder, unused = module.family()
second_target, second_holder, unused = module.family()
assert owner(first_holder) and owner(second_holder)
class BothHolders(first_holder, second_holder):
    pass
class BothTargets(first_target, second_target):
    pass
assert not owner(BothHolders) and not owner(BothTargets)
instance = BothHolders()
instance.payload = BothTargets()
reject_all_writes(instance, first_target())
reject_all_writes(instance, second_target())
assert set_item(vars(instance), 'payload', BothTargets()) == 0
if __dp_integration_mode__ == 'cpython':
    observed = _assert_cpython_function_witness(module.family, diagnostic)
    assert observed['original_code_entered']
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
# ok
# test_self_nominal_field_binds_only_after_pending_class_callbacks
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
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('self_family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
if __dp_integration_mode__ == 'cpython':
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1

    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int
    is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
    is_sealed.argtypes = [ctypes.py_object]
    is_sealed.restype = ctypes.c_int

    def assert_native_class(cls):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
        assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
        assert info.phase == 3 and info.permanent_contract_published == 1
        assert info.owner == owner(cls) and info.owner is not None
        assert is_sealed(cls) == 1

    assert_native_class(module.MutableHolder)
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    observed_read = _assert_cpython_function_witness(
        module.MutableHolder.read, diagnostic,
    )
    assert observed_read["finalized"]
import nominal_field_probe as probe
first, second = module.self_family(), module.self_family()
assert first is not second and owner(first) and owner(second)
assert len(probe.observed) == 2
assert probe.observed == [first, second]
left, right = first(), second()
left.payload, right.payload = left, right
assert left.payload is left and right.payload is right
vars(left)['payload'] = None
vars(right)['payload'] = None
assert left.payload is None and right.payload is None
left.payload = first()
right.payload = second()
reject_all_writes(left, right)
reject_all_writes(right, left)
if __dp_integration_mode__ == 'cpython':
    assert_native_class(first)
    assert_native_class(second)
if __dp_integration_mode__ == 'cpython':
    observed = _assert_cpython_function_witness(module.self_family, diagnostic)
    assert observed['original_code_entered']
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('self_family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
# ok
# test_nominal_field_write_does_not_prove_a_mutable_referents_future_type
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
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('MutableHolder.read', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
if __dp_integration_mode__ == 'cpython':
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1

    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int
    is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
    is_sealed.argtypes = [ctypes.py_object]
    is_sealed.restype = ctypes.c_int

    def assert_native_class(cls):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
        assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
        assert info.phase == 3 and info.permanent_contract_published == 1
        assert info.owner == owner(cls) and info.owner is not None
        assert is_sealed(cls) == 1

    assert_native_class(module.MutableHolder)
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    observed_read = _assert_cpython_function_witness(
        module.MutableHolder.read, diagnostic,
    )
    assert observed_read["finalized"]
import nominal_field_probe as probe
assert owner(module.MutableHolder) and not owner(probe.OrdinaryTarget)
instance = module.MutableHolder()
value = probe.OrdinaryTarget()
instance.payload = value
assert instance.read() is value
class Foreign:
    pass
value.__class__ = Foreign
assert type(value) is Foreign
assert instance.payload is value
# A protected write does not guarantee the referent's future type, and
# annotations do not impose an additional check when that value is returned.
assert instance.read() is value
reject_all_writes(instance, value)
if __dp_integration_mode__ == 'cpython':
    observed = _assert_cpython_function_witness(module.MutableHolder.read, diagnostic)
    assert observed['original_code_entered']
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('MutableHolder.read', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
# ok
# test_nominal_field_dictionary_retains_only_its_required_type_targets
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
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
if __dp_integration_mode__ == 'cpython':
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1

    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int
    is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
    is_sealed.argtypes = [ctypes.py_object]
    is_sealed.restype = ctypes.c_int

    def assert_native_class(cls):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
        assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
        assert info.phase == 3 and info.permanent_contract_published == 1
        assert info.owner == owner(cls) and info.owner is not None
        assert is_sealed(cls) == 1

    assert_native_class(module.MutableHolder)
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    observed_read = _assert_cpython_function_witness(
        module.MutableHolder.read, diagnostic,
    )
    assert observed_read["finalized"]
import gc
import weakref
import nominal_field_probe as probe
target, holder, replace = module.family()
assert owner(holder)
instance = holder()
dictionary = vars(instance)
target_ref, holder_ref = weakref.ref(target), weakref.ref(holder)
del target, holder, replace, instance
gc.collect()
assert holder_ref() is None, 'an escaped dictionary retained its receiver class'
assert target_ref() is not None, 'a required nominal target was not retained'
rejected(lambda: set_item(dictionary, 'payload', object()))
del dictionary
gc.collect()
assert target_ref() is None, 'a dropped field policy retained its nominal target'

self_type = module.self_family()
assert probe.observed.pop() is self_type
instance = self_type()
dictionary = vars(instance)
self_ref = weakref.ref(self_type)
del self_type, instance
gc.collect()
assert self_ref() is not None, 'a direct-self field lost its required target'
rejected(lambda: set_item(dictionary, 'payload', object()))
del dictionary
gc.collect()
assert self_ref() is None, 'the direct-self policy cycle was not traversed'
if __dp_integration_mode__ == 'cpython':
    observed = _assert_cpython_function_witness(module.family, diagnostic)
    assert observed['original_code_entered']
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
# ok
# test_dictionary_type_state_drops_unrelated_native_slot_nominal_targets
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
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('mixed_storage_family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
if __dp_integration_mode__ == 'cpython':
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1

    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int
    is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
    is_sealed.argtypes = [ctypes.py_object]
    is_sealed.restype = ctypes.c_int

    def assert_native_class(cls):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
        assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
        assert info.phase == 3 and info.permanent_contract_published == 1
        assert info.owner == owner(cls) and info.owner is not None
        assert is_sealed(cls) == 1

    assert_native_class(module.MutableHolder)
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    observed_read = _assert_cpython_function_witness(
        module.MutableHolder.read, diagnostic,
    )
    assert observed_read["finalized"]
import _testinternalcapi
import gc
import weakref
info = _testinternalcapi.get_soac_type_state_info

dictionary_target, member_target, holder = module.mixed_storage_family()
assert owner(holder) and owner(dictionary_target) and owner(member_target)
first, second = holder(), holder()
assert info(first)['has_slot'] and info(second)['has_slot']
assert info(first)['state_id'] == info(second)['state_id']
first.payload = dictionary_target()
first.native = member_target()
reject_all_writes(first, member_target())
rejected(lambda: generic_set(first, 'native', dictionary_target()))
escaped = vars(first)
assert info(escaped)['state_id'] == info(first)['dictionary_state_id']
assert info(escaped)['state_id'] != info(first)['state_id']
assert info(escaped)['storage_mode'] == 'direct'
# A hidden mapping entry is not the actual native member, and must not acquire
# its nominal target or predicate merely because the names are equal.
escaped['native'] = object()
assert isinstance(first.native, member_target)
del first.native
escaped.clear()
dictionary_ref, member_ref, holder_ref = (
    weakref.ref(dictionary_target), weakref.ref(member_target), weakref.ref(holder),
)
del first, second, dictionary_target, member_target, holder
gc.collect()
assert holder_ref() is None, 'dictionary state retained its receiver class'
assert member_ref() is None, 'dictionary state retained an unrelated native-slot target'
assert dictionary_ref() is not None, 'dictionary state dropped its required nominal target'
rejected(lambda: set_item(escaped, 'payload', object()))
set_item(escaped, 'native', object())
del escaped
gc.collect()
assert dictionary_ref() is None, 'released dictionary state leaked its remaining target'
if __dp_integration_mode__ == 'cpython':
    observed = _assert_cpython_function_witness(module.mixed_storage_family, diagnostic)
    assert observed['original_code_entered']
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('mixed_storage_family', 'MutableHolder.read'):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
