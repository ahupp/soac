# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:prefixed_model
# soac: module(strict_assign=true, checked_attr=true)

_dp_module_value = 40

def make_prefixed(_dp_parameter):
    _dp_local = _dp_parameter + 1

    def read():
        return _dp_parameter, _dp_local, _dp_module_value

    def replace(value):
        nonlocal _dp_local
        _dp_local = value

    def clear():
        nonlocal _dp_local
        del _dp_local

    class Box:
        _dp_field = _dp_parameter

        def read(self):
            return _dp_parameter, _dp_local

    def expressions():
        return ((_dp_parameter, _dp_local, item) for item in (1, 2))

    return read, replace, clear, Box, expressions
# module:ordinary_prefixed_model
_dp_module_value = 40

def make_prefixed(_dp_parameter):
    _dp_local = _dp_parameter + 1

    def read():
        return _dp_parameter, _dp_local, _dp_module_value

    def replace(value):
        nonlocal _dp_local
        _dp_local = value

    def clear():
        nonlocal _dp_local
        del _dp_local

    class Box:
        _dp_field = _dp_parameter

        def read(self):
            return _dp_parameter, _dp_local

    def expressions():
        return ((_dp_parameter, _dp_local, item) for item in (1, 2))

    return read, replace, clear, Box, expressions
# ok
# tests/test_strict_function_boundaries.py::test_source_prefixed_bindings_keep_ordinary_closure_and_namespace_semantics
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_prefixed',):
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
    import ordinary_prefixed_model

    def exercise(factory):
        read, replace, clear, Box, expressions = factory(_dp_parameter=3)
        assert read.__code__.co_freevars == ("_dp_local", "_dp_parameter")
        assert Box.read.__code__.co_freevars == ("_dp_local", "_dp_parameter")
        assert Box._dp_field == 3
        assert read() == (3, 4, 40)
        assert Box().read() == (3, 4)
        assert list(expressions()) == [(3, 4, 1), (3, 4, 2)]
        replace(9)
        assert read() == (3, 9, 40)
        assert Box().read() == (3, 9)
        assert list(expressions()) == [(3, 9, 1), (3, 9, 2)]
        clear()
        try:
            read()
        except NameError:
            pass
        else:
            raise AssertionError("the source nonlocal delete was ignored")
        replace(12)
        assert read() == (3, 12, 40)
        return Box().read(), list(expressions())

    expected = exercise(ordinary_prefixed_model.make_prefixed)
    assert exercise(module.make_prefixed) == expected

validate(module)

_assert_source_function_witnesses()
