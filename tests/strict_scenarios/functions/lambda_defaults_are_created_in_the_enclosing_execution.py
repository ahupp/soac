# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:defaults
# soac: module(strict_assign=true, checked_attr=true)

events = []
def mark(name, value):
    events.append(name)
    return value

module_lambda = lambda positional=mark("positional", lambda: 7), /, *, keyword=mark("keyword", lambda: 9): (positional(), keyword())

def factory(value):
    result = lambda callback=(lambda: value): callback()
    value += 1
    return result
# module:ordinary_defaults
# ordinary source control

events = []
def mark(name, value):
    events.append(name)
    return value

module_lambda = lambda positional=mark("positional", lambda: 7), /, *, keyword=mark("keyword", lambda: 9): (positional(), keyword())

def factory(value):
    result = lambda callback=(lambda: value): callback()
    value += 1
    return result
# ok
# tests/test_strict_function_boundaries.py::test_lambda_defaults_are_created_in_the_enclosing_execution
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('factory', 'module_lambda'):
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
    import ordinary_defaults
    from soac import _soac_ext

    def observations(mod, strict):
        assert mod.events == ["positional", "keyword"]
        assert mod.module_lambda() == (7, 9)
        first = mod.factory(20)
        second = mod.factory(40)
        assert first() == 21 and second() == 41
        functions = [
            mod.module_lambda, mod.module_lambda.__defaults__[0],
            mod.module_lambda.__kwdefaults__["keyword"],
            first, first.__defaults__[0], second, second.__defaults__[0],
        ]
        expected = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native') if strict else None
        for function in functions:
            assert _soac_ext.strict_function_entry_kind(function) == expected
        return [
            (function.__qualname__, function.__code__.co_qualname,
             function.__code__.co_firstlineno, function.__code__.co_freevars)
            for function in functions
        ]

    assert observations(module, True) == observations(ordinary_defaults, False)

validate(module)

_assert_source_function_witnesses()
