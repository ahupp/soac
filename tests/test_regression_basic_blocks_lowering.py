import pytest

from tests._integration import integration_module


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
    with integration_module(tmp_path, "basic_blocks_if_else", source, mode=mode) as module:
        assert module.foo(4, 3) == ("hi", 7)
        assert module.foo(1, 2) == ("lo", 3)


def test_basic_block_lowering_preserves_raise(tmp_path):
    source = """
def trigger(name):
    raise AttributeError(f"module has no attribute {name!r}")
"""
    with integration_module(tmp_path, "basic_blocks_raise", source, mode="soac") as module:
        with pytest.raises(AttributeError, match="module has no attribute"):
            module.trigger("missing")


def test_basic_block_lowering_preserves_class_annotation_scope(tmp_path):
    source = """
class Z[T]:
    value: T

A = Z.__annotations__
TP = Z.__type_params__[0]
"""
    with integration_module(tmp_path, "basic_blocks_annotation_scope", source, mode="soac") as module:
        assert module.A["value"] is module.TP


def test_basic_block_lowering_nested_generator_def(tmp_path):
    source = """
def outer():
    x = 3
    def gen():
        yield x
        yield x + 1
    return list(gen())
"""
    with integration_module(tmp_path, "basic_blocks_nested_generator_def", source, mode="soac") as module:
        assert module.outer() == [3, 4]


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
    with integration_module(tmp_path, "basic_blocks_try_except_else_finally", source, mode="soac") as module:
        assert module.f("ret") == 10
        assert module.events == ["finally"]
        module.events.clear()

        assert module.f("raise") == 20
        assert module.events == ["except", "finally"]
        module.events.clear()

        assert module.f("ok") == 20
        assert module.events == ["body", "else", "finally"]
