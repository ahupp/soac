import os
import textwrap
from pathlib import Path

import pytest

from tests._strict_integration import (
    _VALIDATION_PRELUDE,
    StrictValidationCase,
    create_strict_project,
)


def _run_basic_block_case(
    tmp_path, module_name, source, validation, *, required_functions, mode="soac"
):
    assert mode == "soac"
    filename = f"{module_name}.py"
    project = create_strict_project(
        tmp_path / "strict",
        {filename: "from __future__ import strict\n" + textwrap.dedent(source)},
        modules={module_name: filename},
        backend="soac",
    )
    # Reuse run_case's exact before/after owner and public-entry witnesses, but
    # retain the old test's effective compile mode instead of run()'s Eager
    # default. A Lazy body already has its checked native entry trampoline.
    environment = project.environment if project.environment is not None else os.environ
    program = _VALIDATION_PRELUDE + project._validation_program(
        module_name,
        StrictValidationCase(textwrap.dedent(validation), Path(__file__), required_functions),
        entry_interpreter=False,
        backend="soac",
    )
    project.run(
        program,
        opt_mode=environment.get("SOAC_OPT_MODE", "none"),
        extra_env={
            "SOAC_COMPILE_MODE": environment.get("SOAC_COMPILE_MODE", "lazy"),
            "SOAC_BACKGROUND_JIT": environment.get("SOAC_BACKGROUND_JIT", "1"),
        },
    )


@pytest.mark.parametrize("mode", ["soac"])
def test_basic_block_lowering_if_else(tmp_path, mode):
    source = """
def foo(a, b):
    c = a + b
    if c > 5:
        return ("hi", c)
    else:
        d = b + 1
        return ("lo", d)
"""
    _run_basic_block_case(
        tmp_path, 'basic_blocks_if_else', source,
        """
def validate_module(module):
    assert module.foo(4, 3) == ("hi", 7)
    assert module.foo(1, 2) == ("lo", 3)
""",
        required_functions=('foo',),
        mode=mode,
    )


def test_basic_block_lowering_preserves_raise(tmp_path):
    source = """
def trigger(name):
    raise AttributeError(f"module has no attribute {name!r}")
"""
    _run_basic_block_case(
        tmp_path, 'basic_blocks_raise', source,
        """
import pytest

def validate_module(module):
    with pytest.raises(AttributeError, match="module has no attribute"):
        module.trigger("missing")
""",
        required_functions=('trigger',),
    )


def test_basic_block_lowering_preserves_class_annotation_scope(tmp_path):
    source = """
class Z[T]:
    value: T

A = Z.__annotations__
TP = Z.__type_params__[0]
"""
    _run_basic_block_case(
        tmp_path, 'basic_blocks_annotation_scope', source,
        """
def validate_module(module):
    assert module.A["value"] is module.TP
""",
        required_functions=(),
    )


def test_basic_block_lowering_nested_generator_def(tmp_path):
    source = """
def outer():
    x = 3
    def gen():
        yield x
        yield x + 1
    return list(gen())
"""
    _run_basic_block_case(
        tmp_path, 'basic_blocks_nested_generator_def', source,
        """
def validate_module(module):
    assert module.outer() == [3, 4]
""",
        required_functions=('outer',),
    )


def test_basic_block_lowering_try_except_else_finally(tmp_path):
    source = """
events = []

def f(mode):
    try:
        if mode == "ret":
            return 10
        if mode == "raise":
            raise ValueError("boom")
        events.append("body")
    except ValueError:
        events.append("except")
    else:
        events.append("else")
    finally:
        events.append("finally")
    return 20
"""
    _run_basic_block_case(
        tmp_path, 'basic_blocks_try_except_else_finally', source,
        """
def validate_module(module):
    assert module.f("ret") == 10
    assert module.events == ["finally"]
    module.events.clear()

    assert module.f("raise") == 20
    assert module.events == ["except", "finally"]
    module.events.clear()

    assert module.f("ok") == 20
    assert module.events == ["body", "else", "finally"]
""",
        required_functions=('f',),
    )


def test_basic_block_lowering_try_finally_loop_abrupt_edges(tmp_path):
    source = """
def break_through_finally():
    total = 0
    for value in (1, 2, 3):
        try:
            break
        finally:
            total = total + 40
    return total + value

def continue_through_finally():
    total = 0
    for value in (1, 2, 3):
        try:
            if value == 2:
                continue
            total = total + value
        finally:
            total = total + 10
    return total
"""
    _run_basic_block_case(
        tmp_path, 'basic_blocks_try_finally_loop_abrupt', source,
        """
def validate_module(module):
    assert module.break_through_finally() == 41
    assert module.continue_through_finally() == 34
""",
        required_functions=('break_through_finally', 'continue_through_finally'),
    )


_NAMED_EXPRESSION_SOURCE = """
from namedexpr_namespace import Meta, make_value

def build():
    class Example(metaclass=Meta):
        result = (assigned := make_value())
    return Example
"""

_NAMED_EXPRESSION_NAMESPACE = """
import weakref

events = []
current = None
replacement = object()
fail_store = False

class StoreFailed(Exception):
    pass

failure = StoreFailed("assignment refused")

class Value:
    def __del__(self):
        events.append("released")

def reset(failing):
    global current, fail_store, failure
    events.clear()
    current = None
    fail_store = failing
    failure = StoreFailed("assignment refused")

def make_value():
    global current
    events.append("make")
    value = Value()
    current = weakref.ref(value)
    return value

class Namespace(dict):
    def __getitem__(self, name):
        if name == "assigned":
            events.append("readback")
            raise AssertionError("named expression reloaded its source target")
        return super().__getitem__(name)

    def __setitem__(self, name, value):
        if name == "assigned":
            events.append("store")
            if fail_store:
                # Do not pin the failed operand in this callback's traceback.
                # Only the caller's actual expression stack supports it now.
                del value
                raise failure
            value = replacement
        return super().__setitem__(name, value)

class Meta(type):
    @classmethod
    def __prepare__(cls, name, bases, **kwargs):
        return Namespace()
"""


@pytest.fixture(scope="module", params=["cpython", "soac", "entry"])
def namedexpr_namespace_project(tmp_path_factory, request):
    mode = request.param
    project = create_strict_project(
        tmp_path_factory.mktemp(f"strict-namedexpr-namespace-{mode}"),
        {
            "namedexpr_model.py": "from __future__ import strict\n" + _NAMED_EXPRESSION_SOURCE,
            "ordinary_namedexpr.py": _NAMED_EXPRESSION_SOURCE,
            "namedexpr_namespace.py": _NAMED_EXPRESSION_NAMESPACE,
        },
        modules={"namedexpr_model": "namedexpr_model.py"},
        backend="cpython" if mode == "cpython" else "soac",
    )
    return project, mode == "entry"


def test_named_expression_returns_original_value_without_namespace_readback(
    namedexpr_namespace_project,
):
    project, entry_interpreter = namedexpr_namespace_project
    project.run_case(
        "namedexpr_model",
        """
def validate_module(module):
    import ctypes
    import gc
    import namedexpr_namespace as support
    import ordinary_namedexpr as ordinary
    from soac import _soac_ext

    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    has_contract = ctypes.pythonapi.PyType_HasSoacContract
    has_contract.argtypes = [ctypes.py_object]
    has_contract.restype = ctypes.c_int
    is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
    is_sealed.argtypes = [ctypes.py_object]
    is_sealed.restype = ctypes.c_int
    assert _soac_ext.strict_module_diagnostics(ordinary) is None
    assert owner(ordinary.build) is None
    assert _soac_ext.strict_module_diagnostics(support) is None
    assert owner(support.make_value) is None

    def observe(build):
        support.reset(False)
        cls = build()
        # The external metaclass makes this class automatically Dynamic; the
        # selected enclosing build function still has its real native owner.
        assert has_contract(cls) == 0
        assert is_sealed(cls) == 0
        assert type(cls()) is cls, "Dynamic class retained a pending allocation barrier"
        assert vars(cls)["assigned"] is support.replacement
        assert support.current() is vars(cls)["result"]
        assert support.events == ["make", "store"]
        reference = support.current
        del cls
        gc.collect()
        assert reference() is None
        assert support.events == ["make", "store", "released"]
        return tuple(support.events)

    expected = observe(ordinary.build)
    assert observe(module.build) == expected
""",
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("build",),
        
    )


def test_named_expression_failed_namespace_store_releases_original_value(
    namedexpr_namespace_project,
):
    project, entry_interpreter = namedexpr_namespace_project
    project.run_case(
        "namedexpr_model",
        """
def validate_module(module):
    import ctypes
    import namedexpr_namespace as support
    import ordinary_namedexpr as ordinary
    from soac import _soac_ext

    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert _soac_ext.strict_module_diagnostics(ordinary) is None
    assert owner(ordinary.build) is None
    assert _soac_ext.strict_module_diagnostics(support) is None
    assert owner(support.make_value) is None

    def observe(build):
        support.reset(True)
        try:
            build()
        except support.StoreFailed as error:
            assert error is support.failure
            assert error.__context__ is None
            assert support.current is not None
            # The callback deleted its own argument before raising. The failed
            # expression's temporary must be closed before this handler runs.
            assert support.current() is None
            assert support.events == ["make", "store", "released"]
        else:
            raise AssertionError("the actual namespace store did not fail")
        return tuple(support.events)

    expected = observe(ordinary.build)
    assert observe(module.build) == expected
""",
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("build",),
        
    )
