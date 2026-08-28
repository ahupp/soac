# module:model
# soac: module(strict_assign=true, checked_attr=true)
from framework import Meta, instrument

@instrument
class Decorated:
    value: int = 1

    def method(self):
        return 1

class Managed(metaclass=Meta):
    value: int = 2

    def method(self):
        return 2
# module:framework
def instrument(cls):
    cls.instrumented = True
    return cls

class Meta(type):
    pass
# ok
# test_unknown_decorators_and_metaclasses_remain_ordinary
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
if __dp_integration_mode__ == 'cpython':
    import ctypes
    from soac import _soac_ext
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_type_native import ConstructionInfoV1

    get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    get_type_owner.argtypes = [ctypes.py_object]
    get_type_owner.restype = ctypes.c_void_p
    get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    get_construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    get_construction.restype = ctypes.c_int
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p

    import model
    module_witness = _soac_ext.strict_module_diagnostics(model)
    originals = []
    call_no_args = ctypes.pythonapi.PyObject_CallNoArgs
    call_no_args.argtypes = [ctypes.py_object]
    call_no_args.restype = ctypes.py_object
    for cls, expected in ((model.Decorated, 1), (model.Managed, 2)):
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
        assert (
            info.abi_version, info.struct_size, info.phase,
            info.permanent_contract_published, info.owner, info.root_construction,
        ) == (0, 0, 0, 0, None, None)
        assert get_type_owner(cls) is None
        method = vars(cls)["method"]
        provider = vars(cls)["__annotate_func__"]
        for function in (method, provider):
            witness = _assert_cpython_function_witness(
                function, module_witness,
            )
            assert witness["finalized"] is False
        instance = cls()
        for _ in range(128):
            assert instance.method() == expected
        assert call_no_args(instance.method) == expected
        assert _soac_ext.strict_function_diagnostics(method)["original_code_entered"]
        originals.append((cls, method, provider))
from pathlib import Path
import sys
import types
import model

# The exact same class/decorator/metaclass source, with only strict opt-in
# removed, establishes ordinary function code/closure compatibility.
stock = types.ModuleType('ordinary_unknown_class_control')
sys.modules[stock.__name__] = stock
source = Path(model.__file__).read_text()
exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)', ''),
             '<ordinary unknown class control>', 'exec'), vars(stock))

def replacement(self):
    return 42

def replacement_annotations(format, runtime=None, type_params=()):
    return {'value': str}

def annotation_replacement():
    namespace_marker = None

    def compatible_annotations(format, /):
        # The existing provider's one class-namespace cell is retained.
        namespace_marker
        return {'value': str}

    return compatible_annotations

compatible = annotation_replacement()
assert compatible.__code__.co_argcount == compatible.__code__.co_posonlyargcount == 1
assert len(compatible.__code__.co_freevars) == 1
assert replacement_annotations.__code__.co_freevars == ()

for cls in (stock.Decorated, stock.Managed):
    provider = vars(cls)['__annotate_func__']
    previous_code, previous_closure = provider.__code__, provider.__closure__
    assert len(previous_closure) == 1
    try:
        provider.__code__ = replacement_annotations.__code__
    except ValueError:
        pass
    else:
        raise AssertionError('ordinary provider accepted incompatible closure arity')
    assert provider.__code__ is previous_code
    assert provider.__closure__ is previous_closure

for actual in (stock, model):
    for cls in (actual.Decorated, actual.Managed):
        original = cls.method
        original.__code__ = replacement.__code__
        assert cls().method() == 42
        cls.method = lambda self: 9
        instance = cls()
        instance.method = lambda: 17
        assert instance.method() == 17
        instance.__dict__ = {'value': 31}
        assert vars(instance) == {'value': 31}
        provider = vars(cls)['__annotate_func__']
        previous_closure, previous_defaults = provider.__closure__, provider.__defaults__
        assert len(previous_closure) == len(compatible.__code__.co_freevars)
        provider.__code__ = compatible.__code__
        assert provider.__code__ is compatible.__code__
        assert provider.__closure__ is previous_closure
        assert provider.__defaults__ is previous_defaults
        assert cls.__annotations__ == {'value': str}
    assert actual.Decorated.instrumented
if __dp_integration_mode__ == 'cpython':
    # The original source functions remain ordinary after the same metadata writes
    # exercised above. A changed code object must not retain source-body authority.
    for cls, method, provider in originals:
        for function in (method, provider):
            witness = _soac_ext.strict_function_diagnostics(function)
            assert witness is not None
            assert witness["schema"] == 2 and witness["backend"] == "cpython"
            assert witness["entry_kind"] == "ordinary_replacement"
            assert witness["finalized"] is False
            for key in (
                "source_path", "source_sha256", "artifact_generation",
                "startup_identity", "interpreter_id",
            ):
                assert witness[key] == module_witness[key]
            assert metadata(function) is None
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
        assert (
            info.abi_version, info.struct_size, info.phase,
            info.permanent_contract_published, info.owner, info.root_construction,
        ) == (0, 0, 0, 0, None, None)
        assert get_type_owner(cls) is None
