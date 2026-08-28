# modes:soac,entry
# module:generated_check_support
events = []
produced = 11
default_value = 7
factory_raises = False
factory_error = RuntimeError('ordinary factory failure')

def make_value() -> int:
    events.append('factory')
    if factory_raises:
        raise factory_error
    return produced
# module:generated_check_setup
# Ordinary runtime setup: preserve the analyzed support/model source bytes.
# This module must run before the selected model captures its dataclass default.
import sys
import generated_check_support as support

assert 'generated_check_model' not in sys.modules
support.default_value = 'wrong'
# module:generated_check_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass, field
import generated_check_support

@dataclass
class Factory:
    value: int = field(default_factory=generated_check_support.make_value)

@dataclass
class Default:
    value: int = generated_check_support.default_value

def make_watched():
    @dataclass
    class Watched:
        value: int

        def accept(self, value: int) -> int:
            generated_check_support.events.append(('source', value))
            return value

    return Watched
# ok
# test_generated_dataclass_checks_storage_after_binding_and_factory_effects [default]
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
import dataclasses
import generated_check_support as support

def assert_generated_owner(cls):
    class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    class_owner.argtypes = [ctypes.py_object]
    class_owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert class_owner(cls) and function_owner(cls.__init__)
    # Generated ownership does not manufacture source/JIT metadata.
    assert not metadata(cls.__init__)

from generated_check_model import Default, Factory
assert_generated_owner(Default)
assert_generated_owner(Factory)

default_receiver = object.__new__(Default)
field_write_rejected(lambda: Default.__init__(default_receiver))
assert vars(default_receiver) == {}
field_write_rejected(lambda: Default())
assert Default(4).value == 4

receiver = object.__new__(Factory)
support.events.clear()
field_write_rejected(lambda: Factory.__init__(receiver, 'wrong'))
assert vars(receiver) == {} and support.events == []
field_write_rejected(lambda: Factory('wrong'))
assert support.events == []

support.produced = 'wrong'
field_write_rejected(lambda: Factory.__init__(receiver))
assert support.events == ['factory'], 'field rejection skipped or replayed the factory'
assert vars(receiver) == {}, 'the failing field store partially installed its value'

support.produced = 11
support.events.clear()
assert Factory.__init__(receiver) is None
assert receiver.value == 11 and support.events == ['factory']
# ok
# test_generated_dataclass_uses_the_actually_bound_nonfactory_default [default]
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
import dataclasses
import generated_check_support as support

def assert_generated_owner(cls):
    class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    class_owner.argtypes = [ctypes.py_object]
    class_owner.restype = ctypes.c_void_p
    function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    function_owner.argtypes = [ctypes.py_object]
    function_owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert class_owner(cls) and function_owner(cls.__init__)
    # Generated ownership does not manufacture source/JIT metadata.
    assert not metadata(cls.__init__)

# An ordinary consumed module can change a value without changing its signed
# source bytes. The actual default, not the checker's type, reaches binding.
from generated_check_model import Default
assert Default.__init__.__defaults__ == ('wrong',)
class Foreign:
    pass
foreign = Foreign()
assert Default.__init__(foreign) is None and foreign.value == 'wrong'
# Explicit binding supplies a compatible value for the actual checked field;
# the separate storage case covers rejection of the captured default there.
assert Default(4).value == 4
assert_generated_owner(Default)
