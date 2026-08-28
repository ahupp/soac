# modes:cpython
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
# test_cpython_backend_dataclass_behavior_and_actual_generated_ownership [slots]
import sys
from soac import _soac_ext
import importlib
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
_scenario_subject = importlib.import_module('dataclass_model')
def _scenario_check_source_functions():
    import ctypes
    diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in ('Record.__post_init__', 'Record.total', 'Manual.__init__'):
        function = _plain_function_witness(_scenario_subject, name)
        if __dp_integration_mode__ == 'cpython':
            _assert_cpython_function_witness(function, diagnostic)
        else:
            assert owner(function) and metadata(function), name
            expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
            assert _soac_ext.strict_function_entry_kind(function) == expected, name
_scenario_check_source_functions()

source = "\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import InitVar, dataclass, field\nfrom typing import ClassVar\nimport adapter_support\n\n@dataclass(slots=True)\nclass Base:\n    first: int = 1\n\n@dataclass(slots=True, weakref_slot=True, kw_only=True)\nclass Record(Base):\n    value: int = 2\n    items: list[int] = field(default_factory=adapter_support.new_items,\n                            repr=False, compare=False)\n    seed: InitVar[int] = 3\n    shared: ClassVar[str] = 'classvar'\n\n    def __post_init__(self, seed: int) -> None:\n        adapter_support.post(seed)\n        self.items.append(seed)\n\n    def total(self) -> int:\n        return self.first + self.value\n\n@dataclass(slots=True, frozen=True, order=True)\nclass Frozen:\n    x: int\n    y: int = 2\n\n@dataclass(slots=True, init=False, repr=False, eq=False,\n           unsafe_hash=True, match_args=False)\nclass Manual:\n    value: int\n\n    def __init__(self, value: int) -> None:\n        self.value = value\n"
slots = True

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
import reprlib
import sys
import types
import weakref
import adapter_support
import dataclass_model as model
from soac import _soac_ext
from soac.strict import StrictMutationError

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
    # InitVar is a call value, not instance storage constrained by its annotation.
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

# Same original subject and behavioral oracle as the existing SOAC family;
# storage-compatibility assertions remain in that retained-path test.
assert exercise(model) == exercise(stock)

def api(name, arity, result=ctypes.c_int):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object] * arity
    function.restype = result
    return function

class_owner = api('PyType_GetSoacContractOwner', 1, ctypes.c_void_p)
class_sealed = api('PyType_IsSoacSealed', 1)
function_owner = api('PyFunction_GetSoacStrictOwner', 1, ctypes.c_void_p)
metadata = api('PyFunction_GetSoacMetadata', 1, ctypes.c_void_p)

def rejected(operation):
    try:
        operation()
    except StrictMutationError:
        return
    raise AssertionError('native dataclass contract did not reject mutation')

for cls in (model.Base, model.Record, model.Frozen, model.Manual):
    assert class_owner(cls) and class_sealed(cls) == 1
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
        assert function_owner(function)
        assert metadata(function) is None
        rejected(lambda: setattr(function, '__code__', function.__code__))
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
    assert type(storage) is dict and list(storage) == ['first', 'value', 'items']
    storage['total'] = 'hidden dictionary value'
    assert instance.total() == 3
    rejected(lambda: setattr(instance, 'total', object()))
    assert storage is vars(instance)

for number in range(128):
    assert model.Record(number).total() == number + 2
call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
assert call(model.Record, (4,), {'value': 5}).total() == 9
field_write_rejected(lambda: call(model.Record, ('wrong',), {}))
assert call(model.Record, (4,), {'seed': 'ordinary InitVar'}).items == ['ordinary InitVar']
assert stock.Record('ordinary').first == 'ordinary'
for function in (model.Record.__post_init__, model.Record.total, model.Manual.__init__):
    assert _soac_ext.strict_function_diagnostics(function)['original_code_entered'] is True

_scenario_check_source_functions()
