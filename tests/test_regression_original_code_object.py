from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from soac.runtime import ClosureAsyncGenerator, ClosureGenerator, Coroutine
from tests._integration import soac_module


def test_soac_functions_expose_original_code_objects(tmp_path: Path) -> None:
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

    with soac_module(tmp_path, "original_code_object", source) as module:
        inner = module.outer(3)
        other_inner = module.outer(9)

        assert module.outer.__code__.co_name == "outer"
        assert module.outer.__code__.co_qualname == "outer"
        assert module.outer.__code__.co_firstlineno == 2
        assert module.outer.__code__.co_filename.endswith("original_code_object.py")

        assert inner(4) == 17
        assert inner.__code__.co_name == "inner"
        assert inner.__code__.co_qualname == "outer.<locals>.inner"
        assert inner.__code__.co_firstlineno == 5
        assert inner.__code__.co_freevars == ("a", "x")
        assert inner is not other_inner
        assert inner.__code__ is other_inner.__code__
        assert other_inner(4) == 23

        assert module.Example().method() == 42
        assert module.Example.method.__code__.co_name == "method"
        assert module.Example.method.__code__.co_qualname == "Example.method"
        assert module.Example.method.__code__.co_firstlineno == 12


def _assert_generator_instances_reuse_original_code_objects(tmp_path: Path) -> None:
    source = '''
def generator(value):
    yield value
    yield value + 1


def generator_expression(offset):
    return (offset + value for value in range(2))


async def coroutine(value):
    return value


async def async_generator(value):
    yield value
'''

    with soac_module(tmp_path, "generator_original_code_object", source) as module:
        first_generator = module.generator(3)
        second_generator = module.generator(9)
        assert isinstance(first_generator, ClosureGenerator)
        assert first_generator.gi_code is module.generator.__code__
        assert second_generator.gi_code is first_generator.gi_code
        assert (
            next(first_generator),
            next(second_generator),
            next(first_generator),
            next(second_generator),
        ) == (3, 9, 4, 10)

        first_expression = module.generator_expression(3)
        second_expression = module.generator_expression(9)
        assert isinstance(first_expression, ClosureGenerator)
        expression_code = next(
            constant
            for constant in module.generator_expression.__code__.co_consts
            if getattr(constant, "co_name", None) == "<genexpr>"
        )
        assert first_expression.gi_code is expression_code
        assert second_expression.gi_code is expression_code
        assert (
            next(first_expression),
            next(second_expression),
            next(first_expression),
            next(second_expression),
        ) == (3, 9, 4, 10)

        first_coroutine = module.coroutine(3)
        second_coroutine = module.coroutine(9)
        assert isinstance(first_coroutine, Coroutine)
        assert first_coroutine.cr_code is module.coroutine.__code__
        assert second_coroutine.cr_code is first_coroutine.cr_code
        with pytest.raises(StopIteration) as first_result:
            first_coroutine.send(None)
        with pytest.raises(StopIteration) as second_result:
            second_coroutine.send(None)
        assert first_result.value.value == 3
        assert second_result.value.value == 9

        first_async_generator = module.async_generator(3)
        second_async_generator = module.async_generator(9)
        assert isinstance(first_async_generator, ClosureAsyncGenerator)
        assert first_async_generator.ag_code is module.async_generator.__code__
        assert second_async_generator.ag_code is first_async_generator.ag_code
        with pytest.raises(StopIteration) as first_async_result:
            first_async_generator.__anext__().send(None)
        with pytest.raises(StopIteration) as second_async_result:
            second_async_generator.__anext__().send(None)
        assert first_async_result.value.value == 3
        assert second_async_result.value.value == 9


def test_generator_instances_reuse_original_code_objects(tmp_path: Path) -> None:
    env = dict(os.environ)
    env.pop("SOAC_MODULE_ENABLED", None)
    env["SOAC_OPT_MODE"] = "profile"
    env["SOAC_WORK_DIR"] = str(tmp_path / "profile")
    script = (
        "import sys; "
        f"sys.path.insert(0, {str(Path(__file__).resolve().parents[1])!r}); "
        "from pathlib import Path; "
        "from tests.test_regression_original_code_object import "
        "_assert_generator_instances_reuse_original_code_objects; "
        f"_assert_generator_instances_reuse_original_code_objects(Path({str(tmp_path)!r}))"
    )
    result = subprocess.run(
        [sys.executable, "-c", script],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr


def test_generated_class_helpers_do_not_lazy_jit_during_import(tmp_path: Path) -> None:
    log_path = tmp_path / "events.jsonl"
    module_path = tmp_path / "class_helper_import_storm.py"
    class_defs = "\n".join(
        f"""
class C{index}:
    value = {index}

    def method(self):
        return self.value
"""
        for index in range(8)
    )
    module_path.write_text(class_defs, encoding="utf-8")
    env = {
        **os.environ,
        "SOAC_MODULE_ENABLED": f"path:{module_path}",
        # Background warmup intentionally compiles all ordinary-mode module callables,
        # including generated class helpers. Profile mode keeps testing the narrower
        # invariant: class helper execution during import should not itself trigger
        # foreground lazy JIT compilation.
        "SOAC_OPT_MODE": "profile",
        "SOAC_WORK_DIR": str(tmp_path / "profile"),
        "SOAC_LOG": f"soac_jit_codegen=info;json={log_path}",
    }

    subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                f"sys.path.insert(0, {str(tmp_path)!r}); "
                "from soac.import_hook import install; "
                "install(); "
                "import class_helper_import_storm as module; "
                "assert [getattr(module, f'C{i}')().method() for i in range(8)] == list(range(8))"
            ),
        ],
        check=True,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    class_helper_codegen = [
        row
        for row in rows
        if row.get("event") == "soac.jit_codegen"
        and row["module_name"].endswith("class_helper_import_storm")
        and row["function_qualname"].startswith(("_dp_class_ns_", "_dp_define_class_"))
    ]
    assert class_helper_codegen == []
