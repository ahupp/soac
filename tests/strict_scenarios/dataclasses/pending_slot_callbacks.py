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
# module:dataclass_callbacks
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
import adapter_support

@dataclass(slots=True)
class CallbackBase:
    marker: int = 1

    def __init_subclass__(cls):
        adapter_support.observe(cls)

@dataclass(slots=True)
class Observed(CallbackBase):
    value: int = 2
# ok
# test_slotted_dataclass_original_and_replacement_stay_pending_through_callbacks [default]
import sys
from soac import _soac_ext
source = '\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nimport adapter_support\n\n@dataclass(slots=True)\nclass CallbackBase:\n    marker: int = 1\n\n    def __init_subclass__(cls):\n        adapter_support.observe(cls)\n\n@dataclass(slots=True)\nclass Observed(CallbackBase):\n    value: int = 2\n'
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

import ctypes
import sys
import types
import adapter_support
import dataclass_callbacks as model
from soac.strict import StrictMutationError

observed = tuple(adapter_support.classes)
adapter_support.classes.clear()
adapter_support.expect_pending = False
stock = types.ModuleType('ordinary_dataclass_callbacks')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)\n', ''),
             '<ordinary dataclass replacement control>', 'exec'), vars(stock))
stock_observed = tuple(adapter_support.classes)
assert len(stock_observed) == 2
assert stock_observed[0][0] is not stock_observed[1][0]
assert stock_observed[1][0] is stock.Observed
assert tuple(event[2] for event in stock_observed) == (True, False)
assert not any(event[1] for event in stock_observed)
assert len(observed) == 2, observed
(original, original_owned, original_dict), (replacement, replacement_owned, replacement_dict) = observed
assert original is not replacement and replacement is model.Observed
assert original.__bases__ == replacement.__bases__ == (model.CallbackBase,)
assert original_dict is True and replacement_dict is False
assert not original_owned and not replacement_owned, 'a provisional acquired a permanent contract'
hook = model.CallbackBase.__init_subclass__.__func__
assert _soac_ext.strict_function_entry_kind(hook) == expected_entry
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert sealed(replacement) == 1 and sealed(original) == 0
owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes, owner.restype = [ctypes.py_object], ctypes.c_void_p
assert owner(replacement) and not owner(original)
try:
    replacement.new_binding = object()
except StrictMutationError:
    pass
else:
    raise AssertionError('selected dataclass lost its permanent contract')
original.new_binding = object()  # Exact resolved lineage permits dynamic disposal.
assert vars(original()) == {'value': 2}
assert not hasattr(replacement(), '__dict__')
assert replacement().marker == 1 and replacement().value == 2
