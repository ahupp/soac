# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:class_cell
# soac: module(strict_assign=true, checked_attr=true)

def factory():
    class Model:
        def reader(self):
            def read():
                nonlocal __class__
                return __class__
            return read

        def replace(self, value):
            nonlocal __class__
            __class__ = value

        def erase(self):
            nonlocal __class__
            del __class__

        def direct(self):
            return __class__
    return Model
# module:ordinary_class_cell
# ordinary source control

def factory():
    class Model:
        def reader(self):
            def read():
                nonlocal __class__
                return __class__
            return read

        def replace(self, value):
            nonlocal __class__
            __class__ = value

        def erase(self):
            nonlocal __class__
            del __class__

        def direct(self):
            return __class__
    return Model
# ok
# tests/test_strict_function_boundaries.py::test_nonlocal_implicit_class_cell_read_write_delete
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
    import ordinary_class_cell
    from soac import _soac_ext

    class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    class_owner.argtypes = [ctypes.py_object]
    class_owner.restype = ctypes.c_void_p

    def observations(candidate, strict):
        first = candidate.factory()
        second = candidate.factory()
        instance = first()
        other = second()
        read = instance.reader()
        other_read = other.reader()
        expected = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native') if strict else None
        assert _soac_ext.strict_function_entry_kind(read) == expected
        assert _soac_ext.strict_function_entry_kind(other_read) == expected
        assert bool(class_owner(first)) is strict
        assert bool(class_owner(second)) is strict
        assert read() is first and instance.direct() is first
        assert other_read() is second and other.direct() is second

        replacement = object()
        instance.replace(replacement)
        assert read() is replacement and instance.direct() is replacement
        assert other_read() is second and other.direct() is second
        instance.erase()
        errors = []
        for callback in (read, instance.direct):
            try:
                callback()
            except NameError as error:
                errors.append((type(error).__name__, error.args))
            else:
                raise AssertionError("deleted class cell remained readable")
        instance.replace(first)
        assert read() is first and instance.direct() is first
        assert other_read() is second and other.direct() is second
        assert "__class__" not in vars(candidate)
        assert "__class__" not in vars(first)
        assert "__class__" not in vars(second)
        assert _soac_ext.strict_function_entry_kind(read) == expected
        return errors

    expected = observations(ordinary_class_cell, False)
    actual = observations(module, True)
    assert actual == expected, (actual, expected)

validate(module)

_assert_source_function_witnesses()
