from __future__ import annotations

import ctypes
from pathlib import Path

import pytest

from tests._integration import integration_module, soac_module


def _soac_function_id(function: object) -> int:
    get_function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
    get_function_id.argtypes = [ctypes.py_object]
    get_function_id.restype = ctypes.c_uint64
    return int(get_function_id(function))


def test_warmed_direct_call_observes_replaced_function_code(tmp_path: Path) -> None:
    source = """
def target():
    return 41


def replacement():
    return 42


def run():
    return target()
"""

    with soac_module(tmp_path, "warmed_replaced_function_code", source) as module:
        for _ in range(16):
            assert module.run() == 41

        assert _soac_function_id(module.target) != 0
        module.target.__code__ = module.replacement.__code__

        assert module.run() == 42
        assert _soac_function_id(module.target) == 0
        assert module.target() == 42
        for _ in range(16):
            assert module.run() == 42


def test_warmed_direct_call_observes_replaced_positional_defaults(
    tmp_path: Path,
) -> None:
    source = """
def target(increment=1):
    return 40 + increment


def run():
    return target()
"""

    with soac_module(tmp_path, "warmed_replaced_function_defaults", source) as module:
        for _ in range(16):
            assert module.run() == 41

        module.target.__defaults__ = (2,)

        assert module.run() == 42
        assert module.target() == 42
        for _ in range(16):
            assert module.run() == 42


def test_warmed_direct_call_observes_replaced_keyword_defaults(
    tmp_path: Path,
) -> None:
    source = """
def target(*, increment=1):
    return 40 + increment


def run():
    return target()
"""

    with soac_module(tmp_path, "warmed_replaced_keyword_defaults", source) as module:
        for _ in range(16):
            assert module.run() == 41

        module.target.__kwdefaults__ = {"increment": 2}

        assert module.run() == 42
        assert module.target() == 42
        for _ in range(16):
            assert module.run() == 42


def test_warmed_direct_call_observes_in_place_keyword_default_mutation(
    tmp_path: Path,
) -> None:
    source = """
def target(*, increment=1):
    return 40 + increment


def run():
    return target()
"""

    with soac_module(tmp_path, "warmed_mutated_keyword_defaults", source) as module:
        for _ in range(16):
            assert module.run() == 41

        kwdefaults = module.target.__kwdefaults__
        assert kwdefaults is not None
        kwdefaults["increment"] = 2

        assert module.target.__kwdefaults__ is kwdefaults
        assert module.run() == 42
        assert module.target() == 42

        del kwdefaults["increment"]

        with pytest.raises(TypeError):
            module.run()
        with pytest.raises(TypeError):
            module.target()


def test_warmed_method_call_observes_replaced_defaults_and_code(
    tmp_path: Path,
) -> None:
    source = """
class Example:
    def target(self, increment=1):
        return 40 + increment

    def replacement(self, increment=1):
        return 50 + increment


def run(instance):
    return instance.target()
"""

    with soac_module(tmp_path, "warmed_replaced_method", source) as module:
        instance = module.Example()

        for _ in range(16):
            assert module.run(instance) == 41

        module.Example.target.__defaults__ = (2,)
        assert instance.target() == 42
        for _ in range(16):
            assert module.run(instance) == 42

        module.Example.target.__code__ = module.Example.replacement.__code__
        assert instance.target() == 52
        for _ in range(16):
            assert module.run(instance) == 52


def test_named_generator_observes_replaced_defaults_and_code(tmp_path: Path) -> None:
    source = """
def target(increment=1):
    yield 40 + increment


def replacement(increment=1):
    yield 50 + increment


def run():
    return next(target())
"""

    with soac_module(tmp_path, "warmed_replaced_generator", source) as module:
        for _ in range(16):
            assert module.run() == 41

        module.target.__defaults__ = (2,)
        assert module.run() == 42
        assert next(module.target()) == 42

        module.target.__code__ = module.replacement.__code__
        assert module.run() == 52
        assert next(module.target()) == 52


def test_interpreted_entry_observes_replaced_code_and_defaults(tmp_path: Path) -> None:
    source = """
def target(increment=1):
    return 40 + increment


def replacement(increment=1):
    return 50 + increment


def run():
    return target()
"""

    with integration_module(
        tmp_path,
        "entry_replaced_function_code_and_defaults",
        source,
        mode="entry",
    ) as module:
        assert module.run() == 41

        module.target.__defaults__ = (2,)
        assert module.run() == 42

        module.target.__code__ = module.replacement.__code__
        assert module.run() == 52
        assert module.target() == 52
