from __future__ import annotations

import ctypes
from pathlib import Path

import pytest

from tests._integration import soac_module, stock_module


_MODULE_CALL_SOURCE = """
def target(value):
    return value + 1


def replacement(value):
    return value + 10


def call(value):
    return target(value)
"""

_LATE_BUILTIN_SHADOW_SOURCE = """
def call(value):
    return len(value)
"""

_CAPTURED_BUILTIN_SOURCE = """
__builtins__ = {"len": lambda value: 41}


def call(value):
    return len(value)
"""

_GLOBALS_IDENTITY_SOURCE = """
def read_globals():
    return globals()
"""


def _configure_transformed_runtime(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("SOAC_OPT_MODE", "none")
    monkeypatch.setenv("SOAC_WORK_DIR", str(tmp_path / "soac-work"))
    monkeypatch.setenv("SOAC_COMPILE_MODE", "eager")
    monkeypatch.setenv("SOAC_BACKGROUND_JIT", "0")


def _replace_global(module: object, mutation: str) -> None:
    if mutation == "module_attribute":
        module.target = module.replacement
    elif mutation == "module_dictionary":
        module.__dict__["target"] = module.replacement
    elif mutation == "function_globals":
        module.call.__globals__["target"] = module.replacement
    elif mutation == "exec":
        exec("target = replacement", module.__dict__)
    elif mutation == "c_api":
        set_item = ctypes.pythonapi.PyDict_SetItem
        set_item.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
        set_item.restype = ctypes.c_int
        assert set_item(module.__dict__, "target", module.replacement) == 0
    else:
        raise AssertionError(f"unexpected mutation path: {mutation}")


def _observe_global_replacement(module: object, mutation: str) -> tuple[int, int]:
    before = module.call(2)
    _replace_global(module, mutation)
    return before, module.call(2)


@pytest.mark.parametrize(
    "mutation",
    ["module_attribute", "module_dictionary", "function_globals", "exec", "c_api"],
)
def test_unsealed_module_global_replacement_matches_cpython(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    _configure_transformed_runtime(monkeypatch, tmp_path)

    with stock_module(tmp_path, f"stock_global_{mutation}", _MODULE_CALL_SOURCE) as stock:
        expected = _observe_global_replacement(stock, mutation)
    assert expected == (3, 12)

    with soac_module(tmp_path, f"soac_global_{mutation}", _MODULE_CALL_SOURCE) as soac:
        actual = _observe_global_replacement(soac, mutation)

    assert actual == expected


def _observe_late_builtin_shadow(module: object) -> tuple[int, int]:
    before = module.call([1, 2, 3])
    module.__dict__["len"] = lambda value: 41
    return before, module.call([1, 2, 3])


def test_late_module_global_shadows_builtin_like_cpython(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure_transformed_runtime(monkeypatch, tmp_path)

    with stock_module(tmp_path, "stock_late_builtin_shadow", _LATE_BUILTIN_SHADOW_SOURCE) as stock:
        expected = _observe_late_builtin_shadow(stock)
    assert expected == (3, 41)

    with soac_module(tmp_path, "soac_late_builtin_shadow", _LATE_BUILTIN_SHADOW_SOURCE) as soac:
        actual = _observe_late_builtin_shadow(soac)

    assert actual == expected


def _observe_captured_builtin_mutation(module: object) -> tuple[int, int]:
    before = module.call([1, 2, 3])
    module.call.__builtins__["len"] = lambda value: 52
    return before, module.call([1, 2, 3])


def test_named_builtin_uses_its_live_captured_mapping_like_cpython(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure_transformed_runtime(monkeypatch, tmp_path)

    with stock_module(tmp_path, "stock_captured_builtin", _CAPTURED_BUILTIN_SOURCE) as stock:
        expected = _observe_captured_builtin_mutation(stock)
    assert expected == (41, 52)

    with soac_module(tmp_path, "soac_captured_builtin", _CAPTURED_BUILTIN_SOURCE) as soac:
        actual = _observe_captured_builtin_mutation(soac)

    assert actual == expected


def test_globals_builtin_returns_its_own_module_dictionary_like_cpython(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure_transformed_runtime(monkeypatch, tmp_path)

    with stock_module(tmp_path, "stock_globals_identity", _GLOBALS_IDENTITY_SOURCE) as stock:
        expected = stock.read_globals() is stock.__dict__
    assert expected is True

    with soac_module(tmp_path, "soac_globals_identity", _GLOBALS_IDENTITY_SOURCE) as soac:
        actual = soac.read_globals() is soac.__dict__

    assert actual is expected


@pytest.mark.parametrize(
    ("operation", "expression"),
    [
        ("add", "(9223372036854775807 + 1) - 9223372036854775807 - 1"),
        ("subtract", "((0 - 9223372036854775807) - 2) + 9223372036854775807 + 2"),
        ("multiply", "((4611686018427387904 * 2) - 9223372036854775807) - 1"),
    ],
)
def test_scalar_builtin_argument_preserves_big_integer_intermediates(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str, expression: str
) -> None:
    _configure_transformed_runtime(monkeypatch, tmp_path)
    source = f"def call():\n    return chr({expression})\n"

    with stock_module(tmp_path, f"stock_bigint_{operation}", source) as stock:
        expected = stock.call()
    assert expected == "\x00"

    with soac_module(tmp_path, f"soac_bigint_{operation}", source) as soac:
        actual = soac.call()

    assert actual == expected


@pytest.mark.parametrize(
    ("operation", "expression", "value"),
    [
        (
            "add",
            "(9223372036854775807 + ord(value)) - 9223372036854775807 - ord(value)",
            "\x01",
        ),
        (
            "subtract",
            "((0 - 9223372036854775807) - ord(value)) + 9223372036854775807 + ord(value)",
            "\x02",
        ),
        (
            "multiply",
            "((4611686018427387904 * ord(value)) - 9223372036854775807) - 1",
            "\x02",
        ),
    ],
)
def test_dynamic_scalar_builtin_argument_preserves_big_integer_intermediates(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    operation: str,
    expression: str,
    value: str,
) -> None:
    _configure_transformed_runtime(monkeypatch, tmp_path)
    source = f"def call(value):\n    return chr({expression})\n"

    with stock_module(tmp_path, f"stock_dynamic_bigint_{operation}", source) as stock:
        expected = stock.call(value)
    assert expected == "\x00"

    with soac_module(tmp_path, f"soac_dynamic_bigint_{operation}", source) as soac:
        actual = soac.call(value)

    assert actual == expected


@pytest.mark.parametrize(
    ("operation", "initial", "expression", "expected"),
    [
        ("add", 9223372036854775807, "value + 1", 9223372036854775808),
        (
            "subtract",
            0,
            "(value - 9223372036854775807) - 2",
            -9223372036854775809,
        ),
        (
            "multiply",
            4611686018427387904,
            "value * 2",
            9223372036854775808,
        ),
    ],
)
def test_loop_carried_integer_arithmetic_preserves_big_integer_results(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    operation: str,
    initial: int,
    expression: str,
    expected: int,
) -> None:
    _configure_transformed_runtime(monkeypatch, tmp_path)
    source = (
        "def call(active):\n"
        f"    value = {initial}\n"
        "    while active:\n"
        f"        value = {expression}\n"
        "        active = False\n"
        "    return value\n"
    )

    with stock_module(tmp_path, f"stock_loop_bigint_{operation}", source) as stock:
        stock_result = stock.call(True)
    assert stock_result == expected

    with soac_module(tmp_path, f"soac_loop_bigint_{operation}", source) as soac:
        actual = soac.call(True)

    assert actual == stock_result
