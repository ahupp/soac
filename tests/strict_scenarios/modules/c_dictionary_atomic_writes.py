# modes:cpython
# test_strict_module_preconditions.py::test_cpython_module_c_dictionary_mutations_are_atomic_and_honor_explicit_globals
# module:capi_module
# soac: module(strict_assign=true, checked_attr=true)

fixed = 7
counter = 0

def update(value: int) -> int:
    global counter
    counter = value
    return counter

def size(value) -> int:
    return len(value)
# module:ordinary_capi_module
fixed = 7
counter = 0

def update(value: int) -> int:
    global counter
    counter = value
    return counter

def size(value) -> int:
    return len(value)
# ok
# test_cpython_module_c_dictionary_mutations_are_atomic_and_honor_explicit_globals
import sys
import pytest
from soac import _soac_ext, import_hook
def validate(module):
    import ctypes
    import pytest
    import ordinary_capi_module as ordinary
    from soac import _soac_ext
    from soac.strict import StrictMutationError
    from tests._strict_integration import _assert_cpython_function_witness
    from tests.test_strict_module_preconditions import _assert_ordinary_precondition_module

    def api(name, *arguments):
        function = getattr(ctypes.pythonapi, name)
        function.argtypes = list(arguments)
        function.restype = ctypes.c_int
        return function

    obj = ctypes.py_object
    set_item = api("PyDict_SetItem", obj, obj, obj)
    set_string = api("PyDict_SetItemString", obj, ctypes.c_char_p, obj)
    del_item = api("PyDict_DelItem", obj, obj)
    del_string = api("PyDict_DelItemString", obj, ctypes.c_char_p)
    merge = api("PyDict_Merge", obj, obj, ctypes.c_int)
    update = api("PyDict_Update", obj, obj)
    merge_pairs = api("PyDict_MergeFromSeq2", obj, obj, ctypes.c_int)
    has_policy = api("PyDict_HasSoacPolicy", obj)
    namespace = module.__dict__
    control = ordinary.__dict__
    assert type(namespace) is dict and type(control) is dict
    assert module.update.__globals__ is namespace
    assert module.size.__globals__ is namespace
    assert has_policy(namespace) == 1 and has_policy(control) == 0
    _assert_ordinary_precondition_module(ordinary, ("update", "size"))
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    for name in ("update", "size"):
        function = getattr(module, name)
        _assert_cpython_function_witness(function, diagnostic)

    def denied(operation):
        before = tuple(namespace.items())
        with pytest.raises(StrictMutationError):
            operation()
        assert tuple(namespace.items()) == before
        assert module.__dict__ is namespace
        assert module.update.__globals__ is namespace

    # Both supported delete APIs and both set APIs act on the actual md_dict.
    for remove in (
        lambda mapping, key: del_item(mapping, key),
        lambda mapping, key: del_string(mapping, key.encode()),
    ):
        assert set_item(control, "fixed", 7) == 0
        assert remove(control, "fixed") == 0
        assert "fixed" not in control
        denied(lambda: remove(namespace, "fixed"))
    assert set_item(control, "fixed", 7) == 0
    for store in (
        lambda mapping, key, value: set_item(mapping, key, value),
        lambda mapping, key, value: set_string(mapping, key.encode(), value),
    ):
        assert store(control, "fixed", 9) == 0
        assert ordinary.fixed == 9
        denied(lambda: store(namespace, "fixed", 9))

    # A valid mutable update and a new final binding precede an illegal final
    # replacement. No part of the batch may be visible on rejection.
    for index, combine in enumerate((
        lambda mapping, pairs: merge(mapping, dict(pairs), 1),
        lambda mapping, pairs: update(mapping, dict(pairs)),
        lambda mapping, pairs: merge_pairs(mapping, pairs, 1),
    )):
        fresh = "atomic_new_" + str(index)
        pairs = [("counter", 31), (fresh, 42), ("fixed", 99)]
        assert combine(control, pairs) == 0
        assert ordinary.counter == 31 and control[fresh] == 42 and ordinary.fixed == 99
        denied(lambda: combine(namespace, pairs))
        assert fresh not in namespace and module.counter == 0 and module.fixed == 7

    # override=0 really skips the existing final key; it is not a replacement.
    for mapping in (control, namespace):
        original = mapping["fixed"]
        assert merge(mapping, {"fixed": 123, "append_once": 11}, 0) == 0
        assert mapping["fixed"] is original and mapping["append_once"] == 11
    denied(lambda: set_item(namespace, "append_once", 12))
    denied(lambda: del_string(namespace, b"append_once"))

    # Explicit source global ownership stays mutable through all these APIs.
    marker = object()
    for target in (ordinary, module):
        mapping = target.__dict__
        assert set_item(mapping, "counter", marker) == 0
        assert target.counter is marker
        assert target.update(4) == 4
        assert del_item(mapping, "counter") == 0 and "counter" not in mapping
        assert set_string(mapping, b"counter", 5) == 0
        assert del_string(mapping, b"counter") == 0 and "counter" not in mapping
        assert update(mapping, {"counter": 6}) == 0 and target.counter == 6
        assert merge_pairs(mapping, [("counter", 7)], 1) == 0
        assert target.update(8) == 8
    assert module.update("ordinary annotated value") == "ordinary annotated value"
    assert module.counter == "ordinary annotated value"
    assert ordinary.update("ordinary annotated value") == module.counter

    # Native LOAD_GLOBAL observes a late C insertion even after ordinary VM
    # warmup; that newly inserted binding is then final, unlike counter.
    for _ in range(128):
        assert module.size([1, 2, 3]) == ordinary.size([1, 2, 3]) == 3
    replacement = lambda value: 41
    assert set_string(namespace, b"len", replacement) == 0
    assert set_string(control, b"len", replacement) == 0
    assert module.size([]) == ordinary.size([]) == 41
    denied(lambda: set_string(namespace, b"len", len))
    denied(lambda: del_item(namespace, "len"))
    assert set_item(control, "len", len) == 0 and ordinary.size([]) == 0
    assert module.size([]) == 41

    for name in ("update", "size"):
        observed = _assert_cpython_function_witness(
            getattr(module, name), diagnostic,
        )
        assert observed["original_code_entered"] is True
    assert _soac_ext.runtime_compilation_activity() == {
        "schema": 1, "lowering_entries": 0, "blockpy_cache_entries": 0,
        "jit_engine_entries": 0,
    }
validate(module)
