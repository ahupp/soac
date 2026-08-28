# modes:soac,entry,cpython
# module:mutated_dataclass_model
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass

@dataclass(init=False, eq=False)
class Record:
    value: int = 1

def make_record():
    @dataclass(init=False, eq=False)
    class Later:
        value: int = 1
    return Later
# ok
# test_dataclass_construction_keeps_native_failure_barriers_and_later_owners [default]
import sys
from soac import _soac_ext
if __dp_integration_mode__ == 'cpython':
    import sys
    sys.path.insert(0, str(__import__('tests._strict_integration', fromlist=['ROOT']).ROOT))
    backend = 'cpython'
    expected_entry = 'original_code'
    expected_source_path = str(__import__('pathlib').Path(sys.modules['mutated_dataclass_model'].__file__))
    expected_generation = _soac_ext.strict_module_diagnostics(sys.modules['mutated_dataclass_model'])['artifact_generation']

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
    source = '\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\n\n@dataclass(init=False, eq=False)\nclass Record:\n    value: int = 1\n\ndef make_record():\n    @dataclass(init=False, eq=False)\n    class Later:\n        value: int = 1\n    return Later\n'

    import ctypes
    import dataclasses
    import importlib
    import sys
    import types
    from soac.strict import StrictRuntimeUnavailableError, StrictMutationError

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    has_contract = ctypes.pythonapi.PyType_HasSoacContract
    has_contract.argtypes = [ctypes.py_object]
    has_contract.restype = ctypes.c_int

    def still_protected(cls):
        assert has_contract(cls) == 0, 'failed Pending acquired a permanent type contract'
        for allocate in (cls, lambda: object.__new__(cls)):
            try:
                allocate()
            except (StrictMutationError, StrictRuntimeUnavailableError):
                pass
            else:
                raise AssertionError('an escaped failed construction admitted an instance')

    leaked = []
    process_code = dataclasses._process_class.__code__

    model = importlib.import_module('mutated_dataclass_model')
    assert_observer_module(model)
    assert _soac_ext.strict_function_entry_kind(model.make_record) == expected_entry
    add_code = dataclasses._FuncBuilder.add_fn.__code__
    modified = []

    def trace(frame, event, argument):
        if event == 'call' and frame.f_code is process_code:
            leaked.append(frame.f_locals['cls'])
        if (event == 'call' and frame.f_code is add_code
                and frame.f_locals['name'] == '__repr__'):
            frame.f_locals['body'] = ["  return 'injected'"]
            modified.append('body')
        return trace

    failed = None
    if backend == 'cpython':
        sys.settrace(trace)
        try:
            try:
                model.make_record()
            except StrictRuntimeUnavailableError:
                pass
            else:
                raise AssertionError('mutated native construction unexpectedly completed')
        finally:
            sys.settrace(None)
        assert modified == ['body'] and len(leaked) == 1
        failed = leaked[0]
        still_protected(failed)

    # A failed CPython construction stays alive. All backends exercise the same
    # untraced construction; no stale pending-adoption record may supply its owner.
    good = model.make_record()
    assert good is not failed and owner(good)
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(good) == 1
    stock = types.ModuleType('ordinary_later_dataclass_model')
    sys.modules[stock.__name__] = stock
    exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)\n', ''),
                 '<ordinary later dataclass control>', 'exec'), vars(stock))
    assert repr(good()) == repr(stock.make_record()())
    if failed is not None:
        still_protected(failed)
    assert_observer_function(model, model.make_record)

else:
    import sys
    sys.path.insert(0, str(__import__('tests._strict_integration', fromlist=['ROOT']).ROOT))
    backend = 'soac'
    expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
    expected_source_path = str(__import__('pathlib').Path(sys.modules['mutated_dataclass_model'].__file__))
    expected_generation = _soac_ext.strict_module_diagnostics(sys.modules['mutated_dataclass_model'])['artifact_generation']

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
    source = '\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\n\n@dataclass(init=False, eq=False)\nclass Record:\n    value: int = 1\n\ndef make_record():\n    @dataclass(init=False, eq=False)\n    class Later:\n        value: int = 1\n    return Later\n'

    import ctypes
    import dataclasses
    import importlib
    import sys
    import types
    from soac.strict import StrictRuntimeUnavailableError, StrictMutationError

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    has_contract = ctypes.pythonapi.PyType_HasSoacContract
    has_contract.argtypes = [ctypes.py_object]
    has_contract.restype = ctypes.c_int

    def still_protected(cls):
        assert has_contract(cls) == 0, 'failed Pending acquired a permanent type contract'
        for allocate in (cls, lambda: object.__new__(cls)):
            try:
                allocate()
            except (StrictMutationError, StrictRuntimeUnavailableError):
                pass
            else:
                raise AssertionError('an escaped failed construction admitted an instance')

    leaked = []
    process_code = dataclasses._process_class.__code__

    model = importlib.import_module('mutated_dataclass_model')
    assert_observer_module(model)
    assert _soac_ext.strict_function_entry_kind(model.make_record) == expected_entry
    add_code = dataclasses._FuncBuilder.add_fn.__code__
    modified = []

    def trace(frame, event, argument):
        if event == 'call' and frame.f_code is process_code:
            leaked.append(frame.f_locals['cls'])
        if (event == 'call' and frame.f_code is add_code
                and frame.f_locals['name'] == '__repr__'):
            frame.f_locals['body'] = ["  return 'injected'"]
            modified.append('body')
        return trace

    failed = None
    if backend == 'cpython':
        sys.settrace(trace)
        try:
            try:
                model.make_record()
            except StrictRuntimeUnavailableError:
                pass
            else:
                raise AssertionError('mutated native construction unexpectedly completed')
        finally:
            sys.settrace(None)
        assert modified == ['body'] and len(leaked) == 1
        failed = leaked[0]
        still_protected(failed)

    # A failed CPython construction stays alive. All backends exercise the same
    # untraced construction; no stale pending-adoption record may supply its owner.
    good = model.make_record()
    assert good is not failed and owner(good)
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    assert sealed(good) == 1
    stock = types.ModuleType('ordinary_later_dataclass_model')
    sys.modules[stock.__name__] = stock
    exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)\n', ''),
                 '<ordinary later dataclass control>', 'exec'), vars(stock))
    assert repr(good()) == repr(stock.make_record()())
    if failed is not None:
        still_protected(failed)
    assert_observer_function(model, model.make_record)
