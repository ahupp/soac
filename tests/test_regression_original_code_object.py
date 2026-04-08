from __future__ import annotations

from pathlib import Path

from tests._integration import transformed_module


def test_transformed_functions_expose_original_code_objects(tmp_path: Path) -> None:
    source = '''
def outer(a):
    x = 10

    def inner(b):
        return a + b + x

    return inner


class Example:
    def method(self):
        return 42
'''

    with transformed_module(tmp_path, "original_code_object", source) as module:
        inner = module.outer(3)

        assert module.outer.__code__.co_name == "outer"
        assert module.outer.__code__.co_qualname == "outer"
        assert module.outer.__code__.co_firstlineno == 2
        assert module.outer.__code__.co_filename.endswith("original_code_object.py")

        assert inner(4) == 17
        assert inner.__code__.co_name == "inner"
        assert inner.__code__.co_qualname == "outer.<locals>.inner"
        assert inner.__code__.co_firstlineno == 5
        assert inner.__code__.co_freevars == ("a", "x")

        assert module.Example().method() == 42
        assert module.Example.method.__code__.co_name == "method"
        assert module.Example.method.__code__.co_qualname == "Example.method"
        assert module.Example.method.__code__.co_firstlineno == 12
