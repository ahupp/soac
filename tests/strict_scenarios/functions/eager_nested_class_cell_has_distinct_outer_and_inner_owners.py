# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:eager_cell
# soac: module(strict_assign=true, checked_attr=true)
__class__ = 100
def factory():
    class Outer:
        class Inner:
            nonlocal __class__
            __class__ = "construction"
            saved: str = __class__
            def own_class(self):
                return __class__
            def replace(self, value):
                nonlocal __class__
                __class__ = value
        def own_class(self):
            return __class__
        def replace(self, value):
            nonlocal __class__
            __class__ = value
    return Outer
# module:ordinary_eager_cell
# ordinary source control
__class__ = 100
def factory():
    class Outer:
        class Inner:
            nonlocal __class__
            __class__ = "construction"
            saved: str = __class__
            def own_class(self):
                return __class__
            def replace(self, value):
                nonlocal __class__
                __class__ = value
        def own_class(self):
            return __class__
        def replace(self, value):
            nonlocal __class__
            __class__ = value
    return Outer
# ok
# tests/test_strict_function_boundaries.py::test_eager_nested_class_cell_has_distinct_outer_and_inner_owners
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
    import ordinary_eager_cell
    from soac import _soac_ext

    class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    class_owner.argtypes = [ctypes.py_object]
    class_owner.restype = ctypes.c_void_p

    def observe(candidate, strict):
        first, second = candidate.factory(), candidate.factory()
        expected = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native') if strict else None
        for outer in (first, second):
            inner = outer.Inner
            assert bool(class_owner(outer)) is strict
            assert bool(class_owner(inner)) is strict
            assert inner.saved == "construction"
            for cls in (outer, inner):
                assert cls.own_class.__code__.co_freevars == ("__class__",)
                assert _soac_ext.strict_function_entry_kind(cls.own_class) == expected
                assert cls().own_class() is cls
                assert "__class__" not in vars(cls)
            assert outer.own_class.__closure__[0] is not inner.own_class.__closure__[0]
        assert first.own_class.__closure__[0] is not second.own_class.__closure__[0]
        assert first.Inner.own_class.__closure__[0] is not second.Inner.own_class.__closure__[0]

        outer_marker, inner_marker = object(), object()
        first().replace(outer_marker)
        assert first().own_class() is outer_marker
        assert first.Inner().own_class() is first.Inner
        first.Inner().replace(inner_marker)
        assert first().own_class() is outer_marker
        assert first.Inner().own_class() is inner_marker
        assert second().own_class() is second
        assert second.Inner().own_class() is second.Inner
        assert candidate.__dict__["__class__"] == 100
        first().replace(first)
        first.Inner().replace(first.Inner)

    observe(ordinary_eager_cell, False)
    observe(module, True)

validate(module)

_assert_source_function_witnesses()
