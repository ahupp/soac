# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:lambdas
# soac: module(strict_assign=true, checked_attr=true)

module_list = [lambda: index for index in range(3)]
module_set = {lambda: index for index in range(3)}
module_dict = {index: (lambda: index) for index in range(3)}
module_generator = (lambda: index for index in range(3))
generator_input = (item for item in (lambda: range(3))())
module_nested = lambda: (lambda: "nested")

class Owner:
    values = [lambda: index for index in range(3)]
    generated = (lambda: index for index in range(3))
    nested = [lambda: (lambda: "class-nested")]

def factory():
    local = [lambda: index for index in range(3)]
    class Local:
        def method(self):
            super()
            return __class__
        values = [lambda: index for index in range(3)]
        generated = (lambda: index for index in range(3))
        nested = [lambda: (lambda: "local-nested")]
    return local, Local
# module:ordinary_lambdas
# ordinary source control

module_list = [lambda: index for index in range(3)]
module_set = {lambda: index for index in range(3)}
module_dict = {index: (lambda: index) for index in range(3)}
module_generator = (lambda: index for index in range(3))
generator_input = (item for item in (lambda: range(3))())
module_nested = lambda: (lambda: "nested")

class Owner:
    values = [lambda: index for index in range(3)]
    generated = (lambda: index for index in range(3))
    nested = [lambda: (lambda: "class-nested")]

def factory():
    local = [lambda: index for index in range(3)]
    class Local:
        def method(self):
            super()
            return __class__
        values = [lambda: index for index in range(3)]
        generated = (lambda: index for index in range(3))
        nested = [lambda: (lambda: "local-nested")]
    return local, Local
# ok
# tests/test_strict_function_boundaries.py::test_lambda_class_comprehensions_preserve_source_identities_and_scopes
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('factory',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

def validate(module):
    import ctypes
    import ordinary_lambdas
    from soac import _soac_ext

    source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
    source_id.argtypes = [ctypes.py_object]
    source_id.restype = ctypes.c_uint64

    def observations(mod, strict):
        local, cls = mod.factory()
        assert cls().method() is cls
        assert list(mod.generator_input) == [0, 1, 2]
        functions = [
            *mod.module_list, *mod.module_set, *mod.module_dict.values(),
            *mod.module_generator, mod.module_nested(),
            *mod.Owner.values, *mod.Owner.generated, mod.Owner.nested[0](),
            *local, *cls.values, *cls.generated, cls.nested[0](),
        ]
        expected = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native') if strict else None
        for function in functions:
            assert _soac_ext.strict_function_entry_kind(function) == expected
            assert bool(source_id(function.__code__)) is strict
        if strict:
            assert len({source_id(function.__code__) for function in functions}) == 1
        result = [
            (function.__qualname__, function.__code__.co_qualname,
             function.__code__.co_firstlineno, function.__code__.co_freevars,
             function())
            for function in functions
        ]
        for function in functions:
            assert _soac_ext.strict_function_entry_kind(function) == expected
        return result

    assert observations(module, True) == observations(ordinary_lambdas, False)

validate(module)

_assert_source_function_witnesses()
