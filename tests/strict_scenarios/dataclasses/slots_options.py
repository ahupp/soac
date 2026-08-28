# modes:soac,entry
# module:dataclass_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import InitVar, dataclass, field
from typing import ClassVar
import adapter_support

@dataclass(slots=True)
class Base:
    first: int = 1

@dataclass(slots=True, weakref_slot=True, kw_only=True)
class Record(Base):
    value: int = 2
    items: list[int] = field(default_factory=adapter_support.new_items,
                            repr=False, compare=False)
    seed: InitVar[int] = 3
    shared: ClassVar[str] = 'classvar'

    def __post_init__(self, seed: int) -> None:
        adapter_support.post(seed)
        self.items.append(seed)

    def total(self) -> int:
        return self.first + self.value

@dataclass(slots=True, frozen=True, order=True)
class Frozen:
    x: int
    y: int = 2

@dataclass(slots=True, init=False, repr=False, eq=False,
           unsafe_hash=True, match_args=False)
class Manual:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value
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
# ok
# test_stdlib_dataclass_adapter_preserves_fields_options_and_generated_ownership [slots]
import sys
from soac import _soac_ext
source = "\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import InitVar, dataclass, field\nfrom typing import ClassVar\nimport adapter_support\n\n@dataclass(slots=True)\nclass Base:\n    first: int = 1\n\n@dataclass(slots=True, weakref_slot=True, kw_only=True)\nclass Record(Base):\n    value: int = 2\n    items: list[int] = field(default_factory=adapter_support.new_items,\n                            repr=False, compare=False)\n    seed: InitVar[int] = 3\n    shared: ClassVar[str] = 'classvar'\n\n    def __post_init__(self, seed: int) -> None:\n        adapter_support.post(seed)\n        self.items.append(seed)\n\n    def total(self) -> int:\n        return self.first + self.value\n\n@dataclass(slots=True, frozen=True, order=True)\nclass Frozen:\n    x: int\n    y: int = 2\n\n@dataclass(slots=True, init=False, repr=False, eq=False,\n           unsafe_hash=True, match_args=False)\nclass Manual:\n    value: int\n\n    def __init__(self, value: int) -> None:\n        self.value = value\n"
slots = True
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

import _testinternalcapi
import ctypes
import dataclasses
import reprlib
import sys
import types
import weakref
import adapter_support
import dataclass_model as model
from soac.strict import StrictMutationError

source_functions = (model.Record.__post_init__, model.Record.total, model.Manual.__init__)
entries_before = tuple(_soac_ext.strict_function_entry_kind(fn) for fn in source_functions)
stock = types.ModuleType('ordinary_dataclass_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)\n', ''),
             '<ordinary dataclass control>', 'exec'), vars(stock))

def result_or_error(operation):
    try:
        return ('returned', operation())
    except Exception as error:
        return ('raised', type(error), str(error))

def exercise(module):
    adapter_support.events.clear()
    first = module.Record(4, value=8, seed=9)
    second = module.Record()
    assert first.items is not second.items
    assert weakref.ref(first)() is first
    assert first.total() == 12
    assert [field.name for field in dataclasses.fields(module.Record)] == [
        'first', 'value', 'items'
    ]
    if slots:
        assert not hasattr(first, '__dict__')
        assert module.Record.__slots__ == ('value', 'items', '__weakref__')
        assert module.Base.__slots__ == ('first',)
        assert type(vars(module.Record)['value']) is types.MemberDescriptorType
    else:
        assert type(vars(first)) is dict and vars(first) is first.__dict__
        assert list(vars(first)) == ['first', 'value', 'items']
    assert module.Record.__match_args__ == ('first',)
    assert 'seed' not in (vars(first) if not slots else module.Record.__slots__)
    assert 'shared' not in (vars(first) if not slots else module.Record.__slots__)
    constructor_error = result_or_error(lambda: module.Record(1, 2))
    assert constructor_error[0:2] == ('raised', TypeError)

    # InitVar and __post_init__ arguments are ordinary call values. Only the
    # real int fields are selected; no predicate is installed for seed.
    seeded = module.Record(seed='ordinary InitVar')
    assert seeded.items == ['ordinary InitVar']
    assert 'seed' not in (vars(seeded) if not slots else module.Record.__slots__)

    frozen = module.Frozen(5)
    assert frozen == module.Frozen(5) and frozen < module.Frozen(6)
    assert hash(frozen) == hash(module.Frozen(5))
    assign = result_or_error(lambda: setattr(frozen, 'x', 7))
    delete = result_or_error(lambda: delattr(frozen, 'x'))
    assert assign[0:2] == ('raised', dataclasses.FrozenInstanceError)
    assert delete[0:2] == ('raised', dataclasses.FrozenInstanceError)
    # The frozen wrapper does not block object.__setattr__; the selected field
    # policy still applies, and this compatible int write remains legal.
    object.__setattr__(frozen, 'x', 8)
    assert frozen.x == 8

    manual = module.Manual(11)
    assert manual != module.Manual(11)
    assert hash(manual) == hash(module.Manual(11))
    assert '__repr__' not in vars(module.Manual)
    assert '__eq__' not in vars(module.Manual)
    assert '__match_args__' not in vars(module.Manual)
    option_names = (
        'init', 'repr', 'eq', 'order', 'unsafe_hash', 'frozen',
        'match_args', 'kw_only', 'slots', 'weakref_slot',
    )
    options = tuple(
        tuple(getattr(cls.__dataclass_params__, name) for name in option_names)
        for cls in (module.Base, module.Record, module.Frozen, module.Manual)
    )
    return (
        first.first, first.value, first.items, second.items,
        tuple(adapter_support.events), repr(first), constructor_error,
        assign, delete, options,
    )

assert exercise(model) == exercise(stock)

def api(name, arity, result=ctypes.c_int):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object] * arity
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', 1, ctypes.c_void_p)
class_sealed = api('PyType_IsSoacSealed', 1)
function_owner = api('PyFunction_GetSoacStrictOwner', 1, ctypes.c_void_p)

def rejected(operation):
    try:
        operation()
    except StrictMutationError:
        return
    raise AssertionError('native dataclass contract did not reject mutation')

diagnostic = _soac_ext.strict_module_diagnostics(model)
assert diagnostic is not None and diagnostic['sealed'] is True
assert entries_before == (expected_entry,) * len(source_functions), entries_before
assert tuple(_soac_ext.strict_function_entry_kind(fn) for fn in source_functions) == entries_before
for cls in (model.Base, model.Record, model.Frozen, model.Manual):
    assert class_owner(cls), (cls, 'dataclass silently used ordinary construction')
    assert class_sealed(cls) == 1
    rejected(lambda: setattr(cls, 'new_binding', object()))
for cls in (stock.Base, stock.Record, stock.Frozen, stock.Manual):
    assert not class_owner(cls)

for cls, names in (
    (model.Base, ('__init__', '__repr__', '__eq__')),
    (model.Record, ('__init__', '__repr__', '__eq__', '__post_init__', 'total')),
    (model.Frozen, ('__init__', '__repr__', '__eq__', '__lt__', '__le__',
                    '__gt__', '__ge__', '__hash__', '__setattr__', '__delattr__')),
    (model.Manual, ('__init__', '__hash__')),
):
    for name in names:
        function = vars(cls)[name]
        assert type(function) is types.FunctionType
        assert function_owner(function), (cls, name, 'generated ownership is absent')
        rejected(lambda: setattr(function, '__code__', function.__code__))

# Fresh owned components are adopted individually. This does not recursively
# freeze user factories, shared stdlib helpers, or arbitrary closure values.
metadata = api('PyFunction_GetSoacMetadata', 1, ctypes.c_void_p)
for cls in (model.Base, model.Record, model.Frozen):
    provider = cls.__init__.__annotate__
    assert type(provider) is types.FunctionType and function_owner(provider)
    assert not metadata(provider)
    rejected(lambda: setattr(provider, '__code__', provider.__code__))
    implementation = cls.__repr__.__wrapped__
    assert type(implementation) is types.FunctionType and function_owner(implementation)
    assert not metadata(implementation)
    rejected(lambda: setattr(implementation, '__code__', implementation.__code__))
for shared in (dataclasses._make_annotate_function, reprlib.recursive_repr,
               adapter_support.new_items):
    assert not function_owner(shared)
ordinary_repr = reprlib.recursive_repr()(lambda self: 'ordinary')
assert not function_owner(ordinary_repr)
ordinary_repr.__code__ = ordinary_repr.__code__
adapter_support.new_items.__code__ = adapter_support.new_items.__code__

# Sharing an implementation is not fresh-generation ownership. These stdlib
# functions must remain ordinary even when protected classes reference them.
assert model.Record.__replace__ is dataclasses._replace
assert not function_owner(dataclasses._replace)
if slots:
    assert model.Frozen.__getstate__ is dataclasses._dataclass_getstate
    assert model.Frozen.__setstate__ is dataclasses._dataclass_setstate
    assert not function_owner(dataclasses._dataclass_getstate)
    assert not function_owner(dataclasses._dataclass_setstate)
else:
    instance = model.Record()
    storage = vars(instance)
    assert _testinternalcapi.dict_has_indexed_keys(storage) is False
    assert _testinternalcapi.dict_has_indexed_keys(vars(model.Base())) is False
    storage['total'] = 'hidden dictionary value'
    assert instance.total() == 3
    rejected(lambda: setattr(instance, 'total', object()))
    assert storage is vars(instance)
# ok
# test_frozen_dataclass_pickle_uses_ordinary_shared_helpers [slots]
import sys
from soac import _soac_ext
source = "\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import InitVar, dataclass, field\nfrom typing import ClassVar\nimport adapter_support\n\n@dataclass(slots=True)\nclass Base:\n    first: int = 1\n\n@dataclass(slots=True, weakref_slot=True, kw_only=True)\nclass Record(Base):\n    value: int = 2\n    items: list[int] = field(default_factory=adapter_support.new_items,\n                            repr=False, compare=False)\n    seed: InitVar[int] = 3\n    shared: ClassVar[str] = 'classvar'\n\n    def __post_init__(self, seed: int) -> None:\n        adapter_support.post(seed)\n        self.items.append(seed)\n\n    def total(self) -> int:\n        return self.first + self.value\n\n@dataclass(slots=True, frozen=True, order=True)\nclass Frozen:\n    x: int\n    y: int = 2\n\n@dataclass(slots=True, init=False, repr=False, eq=False,\n           unsafe_hash=True, match_args=False)\nclass Manual:\n    value: int\n\n    def __init__(self, value: int) -> None:\n        self.value = value\n"
slots = True

import ctypes
import dataclasses
import pickle
import sys
import types
import dataclass_model as model

stock = types.ModuleType('ordinary_pickle_dataclass_model')
sys.modules[stock.__name__] = stock
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)\n', ''),
             '<ordinary dataclass pickle control>', 'exec'), vars(stock))

def exercise(module):
    value = module.Frozen(5, 6)
    results = []
    for protocol in range(2, pickle.HIGHEST_PROTOCOL + 1):
        restored = pickle.loads(pickle.dumps(value, protocol=protocol))
        assert type(restored) is module.Frozen and restored == value
        results.append((restored.x, restored.y))
    return results

assert exercise(model) == exercise(stock)
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
if slots:
    for name in ('__getstate__', '__setstate__'):
        helper = getattr(dataclasses, '_dataclass' + name[1:-2])
        assert vars(model.Frozen)[name] is helper
        assert not owner(helper), 'a shared pickle helper acquired source ownership'
        helper.__code__ = helper.__code__
