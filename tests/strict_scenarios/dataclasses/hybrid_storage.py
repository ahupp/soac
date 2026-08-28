# modes:soac,entry,cpython
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
# test_dataclass_hybrid_slots_and_inherited_dictionary_entries_are_independent [default]
import sys
from soac import _soac_ext
if __dp_integration_mode__ == 'cpython':

    import importlib
    from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
    _scenario_subject = importlib.import_module('slot_hybrid_model')
    def _scenario_check_source_functions():
        import ctypes
        diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        for name in ():
            function = _plain_function_witness(_scenario_subject, name)
            if __dp_integration_mode__ == 'cpython':
                _assert_cpython_function_witness(function, diagnostic)
            else:
                assert owner(function) and metadata(function), name
                expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
                assert _soac_ext.strict_function_entry_kind(function) == expected, name
    _scenario_check_source_functions()


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

    import types
    import slot_hybrid_model as model

    assert has_contract(model.DictionaryBase) == has_contract(model.Hybrid) == 1
    assert sealed(model.DictionaryBase) == sealed(model.Hybrid) == 1
    assert model.Hybrid.__slots__ == ('value', 'other')
    member = vars(model.Hybrid)['value']
    assert type(member) is types.MemberDescriptorType
    value = model.Hybrid(3, 4)
    storage = vars(value)
    assert storage is value.__dict__ and type(storage) is dict
    assert storage == {}, 'native member initialization was mirrored into the dictionary'
    base_storage = vars(model.DictionaryBase())
    assert _testinternalcapi.dict_has_indexed_keys(storage) is False
    assert _testinternalcapi.dict_has_indexed_keys(base_storage) is False

    storage['value'] = 10
    assert storage['value'] == 10 and value.value == 3 and member.__get__(value) == 3
    member.__set__(value, 11)
    assert value.value == 11 and storage['value'] == 10
    bad_type(lambda: member.__set__(value, 'wrong'))
    bad_type(lambda: storage.__setitem__('value', 'wrong'))
    bad_type(lambda: model.Hybrid('wrong', 4))
    assert value.value == 11 and storage['value'] == 10
    del storage['value']
    assert value.value == 11
    member.__delete__(value)
    assert not hasattr(value, 'value') and storage == {}
    storage['value'] = 12
    assert not hasattr(value, 'value'), 'hidden dictionary storage escaped a native slot'
    member.__set__(value, 13)
    assert value.value == 13 and storage['value'] == 12
    rejected(lambda: setattr(model.Hybrid, 'value', member))
    source_functions = ()

    from soac import _soac_ext
    from tests._strict_integration import _assert_cpython_function_witness

    function_owner = api('PyFunction_GetSoacStrictOwner', 1, ctypes.c_void_p)
    metadata = api('PyFunction_GetSoacMetadata', 1, ctypes.c_void_p)
    type_owner = api('PyType_GetSoacContractOwner', 1, ctypes.c_void_p)
    diagnostic = _soac_ext.strict_module_diagnostics(model)
    for cls in (model.DictionaryBase, model.Hybrid):
        assert type_owner(cls) and has_contract(cls) == 1 and sealed(cls) == 1
        initializer = vars(cls)['__init__']
        name = cls.__name__ + '.__init__'
        assert function_owner(initializer) and metadata(initializer) is None
        if name in source_functions:
            observed = _assert_cpython_function_witness(
                initializer, diagnostic,
            )
            assert observed['finalized'] and observed['original_code_entered']
        else:
            assert _soac_ext.strict_function_diagnostics(initializer) is None
        try:
            initializer.__code__ = initializer.__code__
        except StrictMutationError:
            pass
        else:
            raise AssertionError('admitted hybrid initializer metadata remained mutable')

    _scenario_check_source_functions()

else:

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

    import types
    import slot_hybrid_model as model

    assert has_contract(model.DictionaryBase) == has_contract(model.Hybrid) == 1
    assert sealed(model.DictionaryBase) == sealed(model.Hybrid) == 1
    assert model.Hybrid.__slots__ == ('value', 'other')
    member = vars(model.Hybrid)['value']
    assert type(member) is types.MemberDescriptorType
    value = model.Hybrid(3, 4)
    storage = vars(value)
    assert storage is value.__dict__ and type(storage) is dict
    assert storage == {}, 'native member initialization was mirrored into the dictionary'
    base_storage = vars(model.DictionaryBase())
    assert _testinternalcapi.dict_has_indexed_keys(storage) is False
    assert _testinternalcapi.dict_has_indexed_keys(base_storage) is False

    storage['value'] = 10
    assert storage['value'] == 10 and value.value == 3 and member.__get__(value) == 3
    member.__set__(value, 11)
    assert value.value == 11 and storage['value'] == 10
    bad_type(lambda: member.__set__(value, 'wrong'))
    bad_type(lambda: storage.__setitem__('value', 'wrong'))
    bad_type(lambda: model.Hybrid('wrong', 4))
    assert value.value == 11 and storage['value'] == 10
    del storage['value']
    assert value.value == 11
    member.__delete__(value)
    assert not hasattr(value, 'value') and storage == {}
    storage['value'] = 12
    assert not hasattr(value, 'value'), 'hidden dictionary storage escaped a native slot'
    member.__set__(value, 13)
    assert value.value == 13 and storage['value'] == 12
    rejected(lambda: setattr(model.Hybrid, 'value', member))
# ok
# test_dataclass_type_state_uses_the_final_native_slots_and_dictionary_projection [default]
import sys
from soac import _soac_ext
if __dp_integration_mode__ == 'cpython':

    import importlib
    from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
    _scenario_subject = importlib.import_module('slot_hybrid_model')
    def _scenario_check_source_functions():
        import ctypes
        diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        for name in ():
            function = _plain_function_witness(_scenario_subject, name)
            if __dp_integration_mode__ == 'cpython':
                _assert_cpython_function_witness(function, diagnostic)
            else:
                assert owner(function) and metadata(function), name
                expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
                assert _soac_ext.strict_function_entry_kind(function) == expected, name
    _scenario_check_source_functions()


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
    import weakref
    import slot_hybrid_model as model

    info = _testinternalcapi.get_soac_type_state_info
    assert has_contract(model.Hybrid) == sealed(model.Hybrid) == 1
    assert model.Hybrid.__slots__ == ('value', 'other')
    first, second = model.Hybrid(3, 4), model.Hybrid(5, 6)
    first_info, second_info = info(first), info(second)
    assert first_info['has_slot'] and second_info['has_slot']
    assert first_info['storage_mode'] == second_info['storage_mode'] == 'direct'
    assert first_info['state_id'] == second_info['state_id']
    assert first_info['extra_slot_bytes'] == ctypes.sizeof(ctypes.c_void_p)
    dictionary = vars(first)
    sibling = vars(second)
    assert type(dictionary) is dict and dictionary == {}
    assert info(dictionary)['has_slot'] and info(dictionary)['storage_mode'] == 'direct'
    assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
    assert info(sibling)['state_id'] == first_info['dictionary_state_id']
    assert info(dictionary)['state_id'] != first_info['state_id']

    member = vars(model.Hybrid)['value']
    dictionary['value'] = 11
    dictionary['other'] = 'hidden, not a native member value'
    assert first.value == 3 and first.other == 4
    bad_type(lambda: member.__set__(first, 'wrong'))
    bad_type(lambda: object.__setattr__(first, 'other', 'wrong'))
    bad_type(lambda: dictionary.__setitem__('value', 'wrong'))
    assert first.value == 3 and dictionary['value'] == 11

    # Dictionary replacement keeps its identity and legacy representation, while
    # the original escaped dictionary independently keeps its projected contract.
    incoming = {'value': 13, 'other': 'another hidden entry'}
    incoming_id = id(incoming)
    object.__setattr__(first, '__dict__', incoming)
    assert vars(first) is incoming and id(incoming) == incoming_id
    assert not info(incoming)['has_slot'] and info(incoming)['storage_mode'] == 'legacy'
    assert first.value == 3 and first.other == 4
    assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
    bad_type(lambda: incoming.__setitem__('value', 'wrong'))
    first_ref = weakref.ref(first)
    del first
    gc.collect()
    assert first_ref() is None
    dictionary.clear()
    bad_type(lambda: dictionary.__setitem__('value', 'wrong after receiver death'))
    dictionary['other'] = object()
    bad_type(lambda: incoming.__setitem__('value', 'wrong after receiver death'))
    source_functions = ()

    from soac import _soac_ext
    from tests._strict_integration import _assert_cpython_function_witness

    function_owner = api('PyFunction_GetSoacStrictOwner', 1, ctypes.c_void_p)
    metadata = api('PyFunction_GetSoacMetadata', 1, ctypes.c_void_p)
    type_owner = api('PyType_GetSoacContractOwner', 1, ctypes.c_void_p)
    diagnostic = _soac_ext.strict_module_diagnostics(model)
    for cls in (model.DictionaryBase, model.Hybrid):
        assert type_owner(cls) and has_contract(cls) == 1 and sealed(cls) == 1
        initializer = vars(cls)['__init__']
        name = cls.__name__ + '.__init__'
        assert function_owner(initializer) and metadata(initializer) is None
        if name in source_functions:
            observed = _assert_cpython_function_witness(
                initializer, diagnostic,
            )
            assert observed['finalized'] and observed['original_code_entered']
        else:
            assert _soac_ext.strict_function_diagnostics(initializer) is None
        try:
            initializer.__code__ = initializer.__code__
        except StrictMutationError:
            pass
        else:
            raise AssertionError('admitted hybrid initializer metadata remained mutable')

    _scenario_check_source_functions()

else:

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
    import weakref
    import slot_hybrid_model as model

    info = _testinternalcapi.get_soac_type_state_info
    assert has_contract(model.Hybrid) == sealed(model.Hybrid) == 1
    assert model.Hybrid.__slots__ == ('value', 'other')
    first, second = model.Hybrid(3, 4), model.Hybrid(5, 6)
    first_info, second_info = info(first), info(second)
    assert first_info['has_slot'] and second_info['has_slot']
    assert first_info['storage_mode'] == second_info['storage_mode'] == 'direct'
    assert first_info['state_id'] == second_info['state_id']
    assert first_info['extra_slot_bytes'] == ctypes.sizeof(ctypes.c_void_p)
    dictionary = vars(first)
    sibling = vars(second)
    assert type(dictionary) is dict and dictionary == {}
    assert info(dictionary)['has_slot'] and info(dictionary)['storage_mode'] == 'direct'
    assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
    assert info(sibling)['state_id'] == first_info['dictionary_state_id']
    assert info(dictionary)['state_id'] != first_info['state_id']

    member = vars(model.Hybrid)['value']
    dictionary['value'] = 11
    dictionary['other'] = 'hidden, not a native member value'
    assert first.value == 3 and first.other == 4
    bad_type(lambda: member.__set__(first, 'wrong'))
    bad_type(lambda: object.__setattr__(first, 'other', 'wrong'))
    bad_type(lambda: dictionary.__setitem__('value', 'wrong'))
    assert first.value == 3 and dictionary['value'] == 11

    # Dictionary replacement keeps its identity and legacy representation, while
    # the original escaped dictionary independently keeps its projected contract.
    incoming = {'value': 13, 'other': 'another hidden entry'}
    incoming_id = id(incoming)
    object.__setattr__(first, '__dict__', incoming)
    assert vars(first) is incoming and id(incoming) == incoming_id
    assert not info(incoming)['has_slot'] and info(incoming)['storage_mode'] == 'legacy'
    assert first.value == 3 and first.other == 4
    assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
    bad_type(lambda: incoming.__setitem__('value', 'wrong'))
    first_ref = weakref.ref(first)
    del first
    gc.collect()
    assert first_ref() is None
    dictionary.clear()
    bad_type(lambda: dictionary.__setitem__('value', 'wrong after receiver death'))
    dictionary['other'] = object()
    bad_type(lambda: incoming.__setitem__('value', 'wrong after receiver death'))
# ok
# test_dataclass_native_slot_checks_do_not_constrain_an_unchecked_inherited_dict_prefix [default]
import sys
from soac import _soac_ext
if __dp_integration_mode__ == 'cpython':

    import importlib
    from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
    _scenario_subject = importlib.import_module('slot_hybrid_unchecked_model')
    def _scenario_check_source_functions():
        import ctypes
        diagnostic = _soac_ext.strict_module_diagnostics(_scenario_subject)
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        for name in ('DictionaryBase.__init__',):
            function = _plain_function_witness(_scenario_subject, name)
            if __dp_integration_mode__ == 'cpython':
                _assert_cpython_function_witness(function, diagnostic)
            else:
                assert owner(function) and metadata(function), name
                expected = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
                assert _soac_ext.strict_function_entry_kind(function) == expected, name
    _scenario_check_source_functions()


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

    import slot_hybrid_unchecked_model as model

    assert has_contract(model.DictionaryBase) == has_contract(model.Hybrid) == 1
    assert sealed(model.DictionaryBase) == sealed(model.Hybrid) == 1
    value = model.Hybrid()
    storage = vars(value)
    member = vars(model.Hybrid)['value']
    assert value.value == 7 and storage == {}
    assert _testinternalcapi.dict_has_indexed_keys(storage) is False
    assert _testinternalcapi.dict_has_indexed_keys(vars(model.DictionaryBase())) is False

    # This position came from an unannotated base declaration. The new slot's
    # selected int predicate must not become a requirement on hidden dict data.
    storage['value'] = 'hidden'
    assert storage['value'] == 'hidden' and member.__get__(value) == 7
    set_item = api('PyDict_SetItem', 3)
    assert set_item(storage, 'value', 'C hidden') == 0
    assert storage['value'] == 'C hidden' and value.value == 7
    bad_type(lambda: member.__set__(value, 'wrong'))
    bad_type(lambda: object.__setattr__(value, 'value', 'wrong'))
    bad_type(lambda: model.Hybrid('wrong'))
    assert storage['value'] == 'C hidden' and value.value == 7
    source_functions = ('DictionaryBase.__init__',)

    from soac import _soac_ext
    from tests._strict_integration import _assert_cpython_function_witness

    function_owner = api('PyFunction_GetSoacStrictOwner', 1, ctypes.c_void_p)
    metadata = api('PyFunction_GetSoacMetadata', 1, ctypes.c_void_p)
    type_owner = api('PyType_GetSoacContractOwner', 1, ctypes.c_void_p)
    diagnostic = _soac_ext.strict_module_diagnostics(model)
    for cls in (model.DictionaryBase, model.Hybrid):
        assert type_owner(cls) and has_contract(cls) == 1 and sealed(cls) == 1
        initializer = vars(cls)['__init__']
        name = cls.__name__ + '.__init__'
        assert function_owner(initializer) and metadata(initializer) is None
        if name in source_functions:
            observed = _assert_cpython_function_witness(
                initializer, diagnostic,
            )
            assert observed['finalized'] and observed['original_code_entered']
        else:
            assert _soac_ext.strict_function_diagnostics(initializer) is None
        try:
            initializer.__code__ = initializer.__code__
        except StrictMutationError:
            pass
        else:
            raise AssertionError('admitted hybrid initializer metadata remained mutable')

    _scenario_check_source_functions()

else:

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

    import slot_hybrid_unchecked_model as model

    assert has_contract(model.DictionaryBase) == has_contract(model.Hybrid) == 1
    assert sealed(model.DictionaryBase) == sealed(model.Hybrid) == 1
    value = model.Hybrid()
    storage = vars(value)
    member = vars(model.Hybrid)['value']
    assert value.value == 7 and storage == {}
    assert _testinternalcapi.dict_has_indexed_keys(storage) is False
    assert _testinternalcapi.dict_has_indexed_keys(vars(model.DictionaryBase())) is False

    # This position came from an unannotated base declaration. The new slot's
    # selected int predicate must not become a requirement on hidden dict data.
    storage['value'] = 'hidden'
    assert storage['value'] == 'hidden' and member.__get__(value) == 7
    set_item = api('PyDict_SetItem', 3)
    assert set_item(storage, 'value', 'C hidden') == 0
    assert storage['value'] == 'C hidden' and value.value == 7
    bad_type(lambda: member.__set__(value, 'wrong'))
    bad_type(lambda: object.__setattr__(value, 'value', 'wrong'))
    bad_type(lambda: model.Hybrid('wrong'))
    assert storage['value'] == 'C hidden' and value.value == 7
