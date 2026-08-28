# modes:soac,entry
# module:adapter_support
events = []
classes = []
expect_pending = True

def new_items() -> list[int]:
    events.append('factory')
    return []

def post(seed: int) -> None:
    events.append(('post', seed))

def observe(cls):
    import ctypes
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    # The native owner is borrowed; ctypes must not take ownership of it.
    owner.restype = ctypes.c_void_p
    from soac.strict import StrictMutationError
    try:
        instance = object.__new__(cls)
    except StrictMutationError:
        assert expect_pending and not owner(cls)
        dictionary_bearing = bool(cls.__dictoffset__)
    else:
        assert not expect_pending, 'strict source type admitted before final selection'
        dictionary_bearing = hasattr(instance, '__dict__')
    classes.append((cls, bool(owner(cls)), dictionary_bearing))

held = []
# module:slot_lifecycle_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
import adapter_support

class Probe:
    __slots__ = ()

    def __init_subclass__(cls):
        adapter_support.observe(cls)

    def base(self):
        return 4

def make_record():
    @dataclass(slots=True, weakref_slot=True)
    class Record(Probe):
        value: int = 3

        def read(self):
            return super().base() + self.value
    return Record

# The result is deliberately not a class-valued module binding. A weak
# construction record, not an inventory scan, must finalize the selected class.
adapter_support.held.append(make_record())
# module:slot_hybrid_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass

@dataclass
class DictionaryBase:
    value: int = 1

@dataclass(slots=True)
class Hybrid(DictionaryBase):
    other: int = 2
# module:slot_hybrid_unchecked_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass

class DictionaryBase:
    def __init__(self):
        self.value = 0

@dataclass(slots=True)
class Hybrid(DictionaryBase):
    value: int = 7
# ok
# test_dataclass_slots_decorator_result_is_released_without_method_calls [default]
import sys
from soac import _soac_ext
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

import gc
import weakref
from soac import _soac_ext
import adapter_support as support
import slot_lifecycle_model as model

assert _soac_ext.strict_function_entry_kind(model.make_record) == expected_entry
assert len(support.classes) == 2 and len(support.held) == 1
original = weakref.ref(support.classes[0][0])
replacement = weakref.ref(support.held[0])
assert original() is not replacement()
# No instance, source method, or generated method has run. The only caller
# references to the returned class are these ordinary observer containers.
support.classes.clear()
support.held.clear()
gc.collect()
assert original() is None, 'class construction retained the original class'
assert replacement() is None, 'compiled decorator-result cleanup retained the replacement'
# ok
# test_dataclass_slots_module_drain_and_class_cell_repair_do_not_retain_originals [default]
import sys
from soac import _soac_ext
source = '\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nimport adapter_support\n\nclass Probe:\n    __slots__ = ()\n\n    def __init_subclass__(cls):\n        adapter_support.observe(cls)\n\n    def base(self):\n        return 4\n\ndef make_record():\n    @dataclass(slots=True, weakref_slot=True)\n    class Record(Probe):\n        value: int = 3\n\n        def read(self):\n            return super().base() + self.value\n    return Record\n\n# The result is deliberately not a class-valued module binding. A weak\n# construction record, not an inventory scan, must finalize the selected class.\nadapter_support.held.append(make_record())\n'

import _testinternalcapi
import ctypes
from soac.strict import StrictMutationError, StrictRuntimeUnavailableError

def api(name, arity, result=ctypes.c_int):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object] * arity
    function.restype = result
    return function

has_contract = api('PyType_HasSoacContract', 1)
sealed = api('PyType_IsSoacSealed', 1)

def rejected(operation):
    try:
        operation()
    except (StrictMutationError, StrictRuntimeUnavailableError):
        return
    raise AssertionError('an actual slots contract accepted a forbidden mutation')

def bad_type(operation):
    try:
        operation()
    except TypeError:
        return
    raise AssertionError('selected physical storage accepted an incompatible value')

import gc
import sys
import types
import weakref
import adapter_support as support
import slot_lifecycle_model as model

assert _soac_ext.strict_module_diagnostics(model)['sealed']
assert len(support.classes) == 2 and len(support.held) == 1
original = support.classes[0][0]
replacement = support.held[0]
assert replacement is support.classes[1][0] and replacement is not original
assert not any(owned for _, owned, _ in support.classes), 'a provisional was permanently admitted'
assert has_contract(original) == 0 and sealed(original) == 0
assert sealed(replacement) == 1, 'a list-only selected result missed module drain'
assert vars(original)['read'] is vars(replacement)['read']
assert vars(original)['__init__'] is vars(replacement)['__init__']
method = vars(replacement)['read']
cell = method.__closure__[method.__code__.co_freevars.index('__class__')]
assert cell.cell_contents is replacement
assert replacement().read() == 7
# Ordinary repair intentionally changes zero-argument super on the shared
# original method too. Do not silently retarget its source owner to hide it.
try:
    original().read()
except TypeError:
    pass
else:
    raise AssertionError('the original method did not retain ordinary shared-cell behavior')
original_ref, replacement_ref = weakref.ref(original), weakref.ref(replacement)
support.classes.clear()
del original, method, cell
gc.collect()
assert original_ref() is None, 'the adapter retained the original class after ordinary repair'
support.held.clear()
del replacement
gc.collect()
assert replacement_ref() is None, 'a completed invocation retained its replacement class'

# Verify the lifetime/control behavior with the same stdlib transformation.
support.expect_pending = False
stock = types.ModuleType('ordinary_slot_lifecycle_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)\n', ''),
             '<ordinary slots lifecycle control>', 'exec'), vars(stock))
ordinary_original = weakref.ref(support.classes[0][0])
ordinary_replacement = weakref.ref(support.held[0])
assert support.held[0]().read() == 7
support.classes.clear()
gc.collect()
assert ordinary_original() is None
support.held.clear()
gc.collect()
assert ordinary_replacement() is None
