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
# test_slots_apply_keeps_native_failure_barriers_and_untraced_construction [default]
import sys
from soac import _soac_ext
if __dp_integration_mode__ == 'cpython':
    import sys
    sys.path.insert(0, str(__import__('tests._strict_integration', fromlist=['ROOT']).ROOT))
    backend = 'cpython'
    expected_entry = 'original_code'
    expected_source_path = str(__import__('pathlib').Path(sys.modules['slot_lifecycle_model'].__file__))
    expected_generation = _soac_ext.strict_module_diagnostics(sys.modules['slot_lifecycle_model'])['artifact_generation']

    def assert_observer_module(model):
        diagnostic = _soac_ext.strict_module_diagnostics(model)
        assert diagnostic is not None and diagnostic['sealed']
        assert diagnostic['backend'] == backend
        assert diagnostic['module_name'] == model.__name__
        assert diagnostic['source_path'] == expected_source_path
        assert diagnostic['artifact_generation'] == expected_generation
        if backend == 'cpython':
            assert diagnostic['initializer_entry_kind'] == 'original_code'
            assert diagnostic['original_code_entered']
            assert _soac_ext.runtime_compilation_activity() == {
                'schema': 1, 'lowering_entries': 0, 'blockpy_cache_entries': 0,
                'jit_engine_entries': 0,
            }
        else:
            assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
        return diagnostic

    def assert_observer_type(cls):
        type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        type_owner.argtypes = [ctypes.py_object]
        type_owner.restype = ctypes.c_void_p
        type_sealed = ctypes.pythonapi.PyType_IsSoacSealed
        type_sealed.argtypes = [ctypes.py_object]
        type_sealed.restype = ctypes.c_int
        assert type_owner(cls) and type_sealed(cls) == 1

    def assert_observer_function(model, function):
        diagnostic = assert_observer_module(model)
        function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        function_owner.argtypes = [ctypes.py_object]
        function_owner.restype = ctypes.c_void_p
        assert function_owner(function)
        assert _soac_ext.strict_function_entry_kind(function) == expected_entry
        if backend == 'cpython':
            from tests._strict_integration import _assert_cpython_function_witness
            observed = _assert_cpython_function_witness(function, diagnostic)
            assert observed['finalized'] and observed['original_code_entered']
        else:
            metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            metadata.argtypes = [ctypes.py_object]
            metadata.restype = ctypes.c_void_p
            assert metadata(function)

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

    import dataclasses
    import sys
    import adapter_support as support
    import slot_lifecycle_model as model

    assert_observer_module(model)
    support.classes.clear()
    slots_code = dataclasses._add_slots.__code__
    failure = RuntimeError('ordinary trace failure after replacement Ready')
    trace_events = []

    def trace(frame, event, argument):
        if frame.f_code is slots_code and event == 'return':
            trace_events.append('slots return')
            raise failure
        return trace

    if backend == 'cpython':
        sys.settrace(trace)
        try:
            try:
                model.make_record()
            except RuntimeError as error:
                assert error is failure
            else:
                raise AssertionError('native traced slots construction unexpectedly completed')
        finally:
            sys.settrace(None)
        assert len(support.classes) == 2 and trace_events == ['slots return']
    failed = tuple(cls for cls, _, _ in support.classes)
    class Foreign:
        pass

    for cls in failed:
        assert has_contract(cls) == 0
        rejected(cls)
        rejected(lambda: object.__new__(cls))
        # Failed type construction does not invent a call contract on fully
        # constructed generated code used with an ordinary foreign receiver.
        foreign = Foreign()
        assert vars(cls)['__init__'](foreign, 'ordinary') is None
        assert vars(foreign) == {'value': 'ordinary'}

    support.classes.clear()
    good = model.make_record()
    assert good not in failed and good().read() == 7
    assert sealed(good) == 1
    assert len(support.classes) == 2
    assert support.classes[1][0] is good
    assert sealed(support.classes[0][0]) == 0 and sealed(good) == 1
    for cls in failed:
        assert has_contract(cls) == 0
        rejected(cls)
        rejected(lambda: object.__new__(cls))
    assert_observer_function(model, model.make_record)

else:
    import sys
    sys.path.insert(0, str(__import__('tests._strict_integration', fromlist=['ROOT']).ROOT))
    backend = 'soac'
    expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
    expected_source_path = str(__import__('pathlib').Path(sys.modules['slot_lifecycle_model'].__file__))
    expected_generation = _soac_ext.strict_module_diagnostics(sys.modules['slot_lifecycle_model'])['artifact_generation']

    def assert_observer_module(model):
        diagnostic = _soac_ext.strict_module_diagnostics(model)
        assert diagnostic is not None and diagnostic['sealed']
        assert diagnostic['backend'] == backend
        assert diagnostic['module_name'] == model.__name__
        assert diagnostic['source_path'] == expected_source_path
        assert diagnostic['artifact_generation'] == expected_generation
        if backend == 'cpython':
            assert diagnostic['initializer_entry_kind'] == 'original_code'
            assert diagnostic['original_code_entered']
            assert _soac_ext.runtime_compilation_activity() == {
                'schema': 1, 'lowering_entries': 0, 'blockpy_cache_entries': 0,
                'jit_engine_entries': 0,
            }
        else:
            assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
        return diagnostic

    def assert_observer_type(cls):
        type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        type_owner.argtypes = [ctypes.py_object]
        type_owner.restype = ctypes.c_void_p
        type_sealed = ctypes.pythonapi.PyType_IsSoacSealed
        type_sealed.argtypes = [ctypes.py_object]
        type_sealed.restype = ctypes.c_int
        assert type_owner(cls) and type_sealed(cls) == 1

    def assert_observer_function(model, function):
        diagnostic = assert_observer_module(model)
        function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        function_owner.argtypes = [ctypes.py_object]
        function_owner.restype = ctypes.c_void_p
        assert function_owner(function)
        assert _soac_ext.strict_function_entry_kind(function) == expected_entry
        if backend == 'cpython':
            from tests._strict_integration import _assert_cpython_function_witness
            observed = _assert_cpython_function_witness(function, diagnostic)
            assert observed['finalized'] and observed['original_code_entered']
        else:
            metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            metadata.argtypes = [ctypes.py_object]
            metadata.restype = ctypes.c_void_p
            assert metadata(function)

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

    import dataclasses
    import sys
    import adapter_support as support
    import slot_lifecycle_model as model

    assert_observer_module(model)
    support.classes.clear()
    slots_code = dataclasses._add_slots.__code__
    failure = RuntimeError('ordinary trace failure after replacement Ready')
    trace_events = []

    def trace(frame, event, argument):
        if frame.f_code is slots_code and event == 'return':
            trace_events.append('slots return')
            raise failure
        return trace

    if backend == 'cpython':
        sys.settrace(trace)
        try:
            try:
                model.make_record()
            except RuntimeError as error:
                assert error is failure
            else:
                raise AssertionError('native traced slots construction unexpectedly completed')
        finally:
            sys.settrace(None)
        assert len(support.classes) == 2 and trace_events == ['slots return']
    failed = tuple(cls for cls, _, _ in support.classes)
    class Foreign:
        pass

    for cls in failed:
        assert has_contract(cls) == 0
        rejected(cls)
        rejected(lambda: object.__new__(cls))
        # Failed type construction does not invent a call contract on fully
        # constructed generated code used with an ordinary foreign receiver.
        foreign = Foreign()
        assert vars(cls)['__init__'](foreign, 'ordinary') is None
        assert vars(foreign) == {'value': 'ordinary'}

    support.classes.clear()
    good = model.make_record()
    assert good not in failed and good().read() == 7
    assert sealed(good) == 1
    assert len(support.classes) == 2
    assert support.classes[1][0] is good
    assert sealed(support.classes[0][0]) == 0 and sealed(good) == 1
    for cls in failed:
        assert has_contract(cls) == 0
        rejected(cls)
        rejected(lambda: object.__new__(cls))
    assert_observer_function(model, model.make_record)
