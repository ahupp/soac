# modes:soac,entry
# module:source_self_slots_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
import nominal_dataclass_support as support

class Probe:
    def __init_subclass__(cls):
        support.observe(cls)

def parameter_case():
    @dataclass(slots=True)
    class Node(Probe):
        value: int = 1

        def accept(self, other: Node) -> object:
            return other

    return Node

def return_case():
    @dataclass(slots=True)
    class Node(Probe):
        value: int = 1

        def accept(self) -> Node:
            return self

    return Node

def receiver_case():
    @dataclass(slots=True)
    class Node(Probe):
        value: int = 1

        def accept(self: Node) -> object:
            return self

    return Node
# module:nominal_dataclass_support
events = []
observed = []

class Target:
    pass

current = Target()

def new_target() -> Target:
    events.append('factory')
    return current

def post(seed: object) -> None:
    events.append(('post', seed))

def observe(cls):
    import ctypes
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    observed.append((cls, bool(owner(cls))))
# ok
# test_source_self_slots_keeps_shared_method_ownership_and_ordinary_calls [parameter_case]
import sys
from soac import _soac_ext
import ctypes
import source_self_slots_model as model
import nominal_dataclass_support as support
from soac.strict import StrictMutationError

def api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
module_state = _soac_ext.strict_module_diagnostics(model)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def generated_owner(cls):
    assert class_owner(cls), 'the dataclass silently declined construction'
    initializer = vars(cls)['__init__']
    assert function_owner(initializer)
    assert not metadata(initializer), 'generated code acquired source/JIT authority'

def rejected_write(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('selected field storage accepted a foreign value')
factory_name = 'parameter_case'

seal = api('PyFunction_GetSoacStrictId', ctypes.c_uint64)
support.observed.clear()
replacement = getattr(model, factory_name)()
assert len(support.observed) == 2
(original, original_bound), (observed_replacement, replacement_bound) = support.observed
assert replacement is observed_replacement and replacement is not original
assert not original_bound and not replacement_bound
assert not class_owner(original) and class_owner(replacement)
method = original.__dict__['accept']
assert method is replacement.__dict__['accept']
assert function_owner(method) and seal(method)
try:
    method.__code__ = method.__code__
except StrictMutationError:
    pass
else:
    raise AssertionError('shared source method metadata remained mutable')
good = replacement()
wrong = object()
if factory_name == 'parameter_case':
    assert method(good, good) is good
    assert method(original(), good) is good
    assert method(good, wrong) is wrong
    ordinary = original()
    assert method(good, ordinary) is ordinary
else:
    assert method(good) is good
    assert method(wrong) is wrong
    ordinary = original()
    assert method(ordinary) is ordinary
# Generated calls stay ordinary; their writes follow the receiver's storage.
for cls in (original, replacement):
    assert function_owner(cls.__init__)
    assert cls(7).value == 7
    foreign = original()
    assert cls.__init__(foreign, 'ordinary') is None
    assert foreign.value == 'ordinary'
    rejected_write(lambda: cls.__init__(good, 'ordinary'))
    assert good.value == 1
# Disposing the original class does not give it the replacement's field check.
assert original('ordinary').value == 'ordinary'
rejected_write(lambda: replacement('ordinary'))
# ok
# test_source_self_slots_keeps_shared_method_ownership_and_ordinary_calls [return_case]
import sys
from soac import _soac_ext
import ctypes
import source_self_slots_model as model
import nominal_dataclass_support as support
from soac.strict import StrictMutationError

def api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
module_state = _soac_ext.strict_module_diagnostics(model)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def generated_owner(cls):
    assert class_owner(cls), 'the dataclass silently declined construction'
    initializer = vars(cls)['__init__']
    assert function_owner(initializer)
    assert not metadata(initializer), 'generated code acquired source/JIT authority'

def rejected_write(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('selected field storage accepted a foreign value')
factory_name = 'return_case'

seal = api('PyFunction_GetSoacStrictId', ctypes.c_uint64)
support.observed.clear()
replacement = getattr(model, factory_name)()
assert len(support.observed) == 2
(original, original_bound), (observed_replacement, replacement_bound) = support.observed
assert replacement is observed_replacement and replacement is not original
assert not original_bound and not replacement_bound
assert not class_owner(original) and class_owner(replacement)
method = original.__dict__['accept']
assert method is replacement.__dict__['accept']
assert function_owner(method) and seal(method)
try:
    method.__code__ = method.__code__
except StrictMutationError:
    pass
else:
    raise AssertionError('shared source method metadata remained mutable')
good = replacement()
wrong = object()
if factory_name == 'parameter_case':
    assert method(good, good) is good
    assert method(original(), good) is good
    assert method(good, wrong) is wrong
    ordinary = original()
    assert method(good, ordinary) is ordinary
else:
    assert method(good) is good
    assert method(wrong) is wrong
    ordinary = original()
    assert method(ordinary) is ordinary
# Generated calls stay ordinary; their writes follow the receiver's storage.
for cls in (original, replacement):
    assert function_owner(cls.__init__)
    assert cls(7).value == 7
    foreign = original()
    assert cls.__init__(foreign, 'ordinary') is None
    assert foreign.value == 'ordinary'
    rejected_write(lambda: cls.__init__(good, 'ordinary'))
    assert good.value == 1
# Disposing the original class does not give it the replacement's field check.
assert original('ordinary').value == 'ordinary'
rejected_write(lambda: replacement('ordinary'))
# ok
# test_source_self_slots_keeps_shared_method_ownership_and_ordinary_calls [receiver_case]
import sys
from soac import _soac_ext
import ctypes
import source_self_slots_model as model
import nominal_dataclass_support as support
from soac.strict import StrictMutationError

def api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
module_state = _soac_ext.strict_module_diagnostics(model)
assert module_state['ready'] and module_state['strict_assign'] and module_state['sealed']

def generated_owner(cls):
    assert class_owner(cls), 'the dataclass silently declined construction'
    initializer = vars(cls)['__init__']
    assert function_owner(initializer)
    assert not metadata(initializer), 'generated code acquired source/JIT authority'

def rejected_write(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('selected field storage accepted a foreign value')
factory_name = 'receiver_case'

seal = api('PyFunction_GetSoacStrictId', ctypes.c_uint64)
support.observed.clear()
replacement = getattr(model, factory_name)()
assert len(support.observed) == 2
(original, original_bound), (observed_replacement, replacement_bound) = support.observed
assert replacement is observed_replacement and replacement is not original
assert not original_bound and not replacement_bound
assert not class_owner(original) and class_owner(replacement)
method = original.__dict__['accept']
assert method is replacement.__dict__['accept']
assert function_owner(method) and seal(method)
try:
    method.__code__ = method.__code__
except StrictMutationError:
    pass
else:
    raise AssertionError('shared source method metadata remained mutable')
good = replacement()
wrong = object()
if factory_name == 'parameter_case':
    assert method(good, good) is good
    assert method(original(), good) is good
    assert method(good, wrong) is wrong
    ordinary = original()
    assert method(good, ordinary) is ordinary
else:
    assert method(good) is good
    assert method(wrong) is wrong
    ordinary = original()
    assert method(ordinary) is ordinary
# Generated calls stay ordinary; their writes follow the receiver's storage.
for cls in (original, replacement):
    assert function_owner(cls.__init__)
    assert cls(7).value == 7
    foreign = original()
    assert cls.__init__(foreign, 'ordinary') is None
    assert foreign.value == 'ordinary'
    rejected_write(lambda: cls.__init__(good, 'ordinary'))
    assert good.value == 1
# Disposing the original class does not give it the replacement's field check.
assert original('ordinary').value == 'ordinary'
rejected_write(lambda: replacement('ordinary'))
