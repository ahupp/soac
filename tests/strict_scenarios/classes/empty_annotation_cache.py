# modes:soac,entry
# module:empty_annotation_cache
# soac: module(strict_assign=true, checked_attr=true)
from annotationlib import get_annotations

class Plain:
    def method(self, value: int) -> int:
        return value

# Introspection of an unannotated class lazily publishes native cache entries,
# including __annotate_func__ = None, before module sealing.
assert Plain.__annotate__ is None
assert Plain.__annotations__ == {}
assert get_annotations(Plain) == {}

class Annotated:
    value: int = 1

assert get_annotations(Annotated) == {'value': int}
# ok
# test_native_empty_annotation_cache_is_not_a_foreign_class_provider
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import ctypes
import empty_annotation_cache as module

sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert sealed(module.Plain) == 1 and sealed(module.Annotated) == 1
assert vars(module.Plain)['__annotate_func__'] is None
assert module.Plain.__annotations__ == {}
assert module.Annotated.__annotations__ == {'value': int}
assert _soac_ext.strict_function_entry_kind(module.Plain.method) == expected_entry
assert module.Plain().method(3) == 3
assert module.Plain().method('ordinary argument') == 'ordinary argument'
