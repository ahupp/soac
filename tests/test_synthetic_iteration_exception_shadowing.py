from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def test_synthetic_iteration_handlers_ignore_shadowed_exception_names(
    tmp_path: Path,
) -> None:
    module_name = "synthetic_iteration_exception_shadowing_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            import builtins

            StopIteration = ValueError
            StopAsyncIteration = RuntimeError


            def comprehension(values):
                return [value for value in values]


            def ordinary_loop(values):
                result = []
                for value in values:
                    result.append(value)
                return result


            def generator_expression(values):
                return tuple(value for value in values)


            def explicit_sync_handler(error):
                try:
                    raise error
                except StopIteration:
                    return "shadow"
                except BaseException:
                    return "other"


            class AsyncValues:
                def __init__(self, values):
                    self.values = values
                    self.position = 0

                def __aiter__(self):
                    return self

                async def __anext__(self):
                    if self.position == len(self.values):
                        raise builtins.StopAsyncIteration
                    value = self.values[self.position]
                    self.position += 1
                    return value


            async def asynchronous_loop(values):
                result = []
                async for value in AsyncValues(values):
                    result.append(value)
                return result


            async def asynchronous_comprehension(values):
                return [value async for value in AsyncValues(values)]


            async def explicit_async_handler(error):
                try:
                    raise error
                except StopAsyncIteration:
                    return "shadow"
                except BaseException:
                    return "other"
            """
        ),
        encoding="utf-8",
    )

    script = textwrap.dedent(
        f"""
        import asyncio
        import builtins
        import json
        import sys

        sys.path.insert(0, {str(tmp_path)!r})
        from soac.import_hook import install
        install()
        import {module_name} as module

        assert module.StopIteration is ValueError
        assert module.StopAsyncIteration is RuntimeError

        synchronous = [
            module.comprehension((1, 2, 3)),
            module.ordinary_loop((4, 5, 6)),
            list(module.generator_expression((7, 8, 9))),
            module.explicit_sync_handler(ValueError("shadow")),
            module.explicit_sync_handler(builtins.StopIteration("real")),
        ]

        async def run_async():
            return [
                await module.asynchronous_loop((10, 11)),
                await module.asynchronous_comprehension((12, 13)),
                await module.explicit_async_handler(RuntimeError("shadow")),
                await module.explicit_async_handler(
                    builtins.StopAsyncIteration("real")
                ),
            ]

        asynchronous = asyncio.run(run_async())
        print(json.dumps({{"sync": synchronous, "async": asynchronous}}))
        """
    )

    base_env = dict(os.environ)
    base_env.pop("SOAC_LOG", None)
    base_env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(tmp_path / "soac-work"),
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        }
    )

    for mode in ("profile", "apply"):
        completed = subprocess.run(
            [sys.executable, "-c", script],
            check=False,
            capture_output=True,
            text=True,
            env={**base_env, "SOAC_OPT_MODE": mode},
            timeout=60,
        )
        assert completed.returncode == 0, (
            f"{mode} mode must keep compiler-generated iteration handlers "
            "independent of user-shadowed exception names",
            completed.stdout,
            completed.stderr,
        )
        result = json.loads(completed.stdout.splitlines()[-1])
        assert result["sync"] == [
            [1, 2, 3],
            [4, 5, 6],
            [7, 8, 9],
            "shadow",
            "other",
        ]
        assert result["async"] == [
            [10, 11],
            [12, 13],
            "shadow",
            "other",
        ]
