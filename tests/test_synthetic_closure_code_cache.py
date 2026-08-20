from __future__ import annotations

import pytest

from tests._strict_integration import create_strict_project


# Original ordinary source, retained unchanged. Strict enrollment is explicit
# in the fixture; import-hook/mode settings alone are never admission evidence.
_MODULE_SOURCE = """
def cached(offset):
    return [offset + value for value in range(3)]


def prepatched(offset):
    return [offset + value for value in range(3)]


def postpatched(offset):
    return [offset + value for value in range(3)]


def reentrant(offset):
    return [offset + value for value in range(3)]


def replaced_module(offset):
    return [offset + value for value in range(3)]


def original_outer(offset):
    def original_inner(value):
        return offset + value

    return original_inner
"""


@pytest.fixture(scope="module", params=("soac", "cpython"))
def closure_project(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp(f"closure-code-{request.param}"),
        {
            "closure_source.py": "from __future__ import strict\n" + _MODULE_SOURCE,
            "closure_ordinary.py": _MODULE_SOURCE,
        },
        modules={"closure_source": "closure_source.py"},
        backend=request.param,
    )


def test_source_closures_keep_actual_code_and_independent_state(closure_project):
    _run_modes(closure_project, _SOURCE_VALIDATION)


def _run_modes(project, validation):
    path = project.root / "closure-validation.py"
    path.write_text(validation)
    for entry in ((False, True) if project.backend == "soac" else (False,)):
        project.run_case(
            "closure_source", validation, path, entry_interpreter=entry,
            required_functions=("cached", "prepatched", "postpatched", "reentrant", "replaced_module", "original_outer"),
        )


_SOURCE_VALIDATION = r'''
import ctypes
import closure_ordinary as stock

def validate_module(module):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    seal = ctypes.pythonapi.PyFunction_GetSoacStrictId
    seal.argtypes = [ctypes.py_object]
    seal.restype = ctypes.c_uint64
    assert owner(stock.cached) is None and seal(stock.cached) == 0
    for name, offsets in (
        ("cached", (1, 10, 100)), ("prepatched", (2, 20, 200)),
        ("postpatched", (3, 30, 300, 4)), ("reentrant", (4, 40)),
        ("replaced_module", (5, 50, 6)),
    ):
        expected = [getattr(stock, name)(offset) for offset in offsets]
        assert expected == [[offset + value for value in range(3)] for offset in offsets]
        assert [getattr(module, name)(offset) for offset in offsets] == expected
    first, second = module.original_outer(7), module.original_outer(70)
    control_first, control_second = stock.original_outer(7), stock.original_outer(70)
    for left, right in ((first, second), (control_first, control_second)):
        assert left is not right
        assert left(1) == 8 and right(1) == 71
        assert left.__code__ is right.__code__
        assert left.__code__.co_name == "original_inner"
        assert left.__code__.co_qualname == "original_outer.<locals>.original_inner"
        assert left.__code__.co_freevars == ("offset",)
        assert left.__closure__[0] is not right.__closure__[0]
        assert left.__closure__[0].cell_contents == 7
        assert right.__closure__[0].cell_contents == 70
    identities = [(owner(function), seal(function), function.__code__) for function in (first, second)]
    assert all(actual_owner and actual_seal for actual_owner, actual_seal, _ in identities)
    assert owner(control_first) is None and seal(control_first) == 0
    # Mutable cells remain ordinary closure state, not a frozen value fact.
    first.__closure__[0].cell_contents = 8
    control_first.__closure__[0].cell_contents = 8
    assert first(1) == control_first(1) == 9
    assert second(1) == control_second(1) == 71
    assert identities == [(owner(function), seal(function), function.__code__) for function in (first, second)]
'''


def test_production_code_factory_keeps_explicit_mutation_and_reentry(closure_project):
    # code_with_freevars remains production-used by function_instantiation's
    # synthetic_code_for_template. These are explicit ordinary API calls;
    # source comprehensions are NOT required to invoke that factory or allocate
    # an observable synthetic function on any particular schedule.
    _run_modes(closure_project, _FACTORY_VALIDATION)


_FACTORY_VALIDATION = r'''
import builtins as _builtins
import ctypes
import importlib
import sys
import types
import soac.bootstrap as bootstrap
import soac.runtime as runtime
import closure_ordinary as stock

def validate_module(module):
    source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
    source_id.argtypes = [ctypes.py_object]
    source_id.restype = ctypes.c_uint64
    original_factory = runtime.code_with_freevars
    assert original_factory is bootstrap.code_with_freevars
    code = original_factory(("offset",), False, False)
    assert original_factory(("offset",), False, False) is code
    assert code.co_freevars == ("offset",) and source_id(code) == 0

    prepatch_calls, postpatch_calls = [], []
    def prepatched_factory(names, is_async, is_generator):
        prepatch_calls.append(tuple(names))
        return original_factory(names, is_async, is_generator)
    runtime.code_with_freevars = prepatched_factory
    try:
        for _ in (2, 20, 200):
            assert runtime.code_with_freevars(("offset",), False, False) is code
    finally:
        runtime.code_with_freevars = original_factory
    assert prepatch_calls == [("offset",)] * 3
    assert [module.prepatched(value) for value in (2, 20, 200)] == [stock.prepatched(value) for value in (2, 20, 200)]

    assert module.postpatched(3) == [3, 4, 5]
    def postpatched_factory(names, is_async, is_generator):
        postpatch_calls.append(tuple(names))
        return original_factory(names, is_async, is_generator)
    runtime.code_with_freevars = postpatched_factory
    try:
        for _ in (30, 300):
            assert runtime.code_with_freevars(("offset",), False, False) is code
    finally:
        runtime.code_with_freevars = original_factory
    assert postpatch_calls == [("offset",)] * 2
    assert [module.postpatched(value) for value in (30, 300, 4)] == [stock.postpatched(value) for value in (30, 300, 4)]

    # The factory's own ordinary Python __code__ mutation remains observable.
    # No altered code is installed on a strict source function or granted an owner.
    original_code = original_factory.__code__
    delegated = types.FunctionType(original_code, original_factory.__globals__,
        original_factory.__name__, original_factory.__defaults__, original_factory.__closure__)
    assert not hasattr(_builtins, "_soac_eager_delegate")
    assert not hasattr(_builtins, "_soac_eager_code_calls")
    _builtins._soac_eager_delegate = delegated
    _builtins._soac_eager_code_calls = []
    def alternate_code(names, is_async, is_generator):
        _builtins._soac_eager_code_calls.append(tuple(names))
        return _builtins._soac_eager_delegate(names, is_async, is_generator)
    try:
        original_factory.__code__ = alternate_code.__code__
        assert runtime.code_with_freevars(("offset",), False, False) is code
        assert _builtins._soac_eager_code_calls == [("offset",)]
    finally:
        original_factory.__code__ = original_code
        del _builtins._soac_eager_delegate
        del _builtins._soac_eager_code_calls

    # Both aliases may change for explicit calls without granting that callable
    # compiler-helper authority. Restore before calling source comprehensions.
    runtime.code_with_freevars = prepatched_factory
    bootstrap.code_with_freevars = prepatched_factory
    try:
        assert runtime.code_with_freevars(("offset",), False, False) is code
    finally:
        runtime.code_with_freevars = original_factory
        bootstrap.code_with_freevars = original_factory
    assert prepatch_calls == [("offset",)] * 4

    # Prepare the source body normally; the reentrant callback is then an
    # explicit operation of the production ordinary cache API.
    assert module.reentrant(4) == [4, 5, 6]
    original_cache = bootstrap._DP_CODE_WITH_FREEVARS_CACHE
    nested_results, order = [], []
    class ReentrantCodeCache(dict):
        active = False
        def get(self, key, default=None):
            if not self.active:
                self.active = True
                order.append("enter")
                try:
                    nested_results.append(module.reentrant(40))
                finally:
                    order.append("leave")
                    self.active = False
            return super().get(key, default)
    bootstrap._DP_CODE_WITH_FREEVARS_CACHE = ReentrantCodeCache(original_cache)
    try:
        assert original_factory(("offset",), False, False) is code
    finally:
        bootstrap._DP_CODE_WITH_FREEVARS_CACHE = original_cache
    assert nested_results == [[40, 41, 42]] and order == ["enter", "leave"]

    original_runtime = sys.modules["soac.runtime"]
    replacement = types.ModuleType("soac.runtime")
    replacement.__dict__.update(original_runtime.__dict__)
    replacement_calls = []
    def replacement_factory(names, is_async, is_generator):
        replacement_calls.append(tuple(names))
        return original_factory(names, is_async, is_generator)
    replacement.code_with_freevars = replacement_factory
    sys.modules["soac.runtime"] = replacement
    try:
        for _ in (5, 50):
            actual = importlib.import_module("soac.runtime").code_with_freevars(("offset",), False, False)
            assert actual is code and source_id(actual) == 0
    finally:
        sys.modules["soac.runtime"] = original_runtime
    assert replacement_calls == [("offset",)] * 2
    assert [module.replaced_module(value) for value in (5, 50, 6)] == [stock.replaced_module(value) for value in (5, 50, 6)]
'''
