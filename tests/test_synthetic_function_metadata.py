from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def test_synthetic_functions_reuse_prepared_code_metadata_without_extra_events(
    tmp_path: Path,
) -> None:
    module_name = "synthetic_function_metadata_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            def captured(offset):
                return [offset + value for value in range(3)]


            def noncanonical(offset):
                return [offset + value for value in range(3)]


            def original_outer(offset):
                def original_inner(value):
                    return offset + value

                return original_inner
            """
        ),
        encoding="utf-8",
    )

    script = textwrap.dedent(
        f"""
        import ctypes
        import json
        import sys

        sys.path.insert(0, {str(tmp_path)!r})
        from soac.import_hook import install
        install()
        import {module_name} as module
        import soac.runtime as runtime

        EVENT_CREATE = 0
        EVENT_MODIFY_QUALNAME = 5
        created = []
        qualname_changes = []
        callback_errors = []

        callback_type = ctypes.CFUNCTYPE(
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_void_p,
        )

        @callback_type
        def watch_function(event, function_ptr, _new_value):
            if event not in (EVENT_CREATE, EVENT_MODIFY_QUALNAME):
                return 0
            try:
                function = ctypes.cast(function_ptr, ctypes.py_object).value
                code = function.__code__
                if (
                    code.co_name == "<listcomp>"
                    and code.co_qualname.startswith("captured.<locals>.")
                    and function.__globals__ is module.__dict__
                ):
                    if event == EVENT_CREATE:
                        created.append(function)
                    else:
                        qualname_changes.append(code.co_qualname)
            except BaseException as error:
                callback_errors.append(type(error).__name__)
            return 0

        add_watcher = ctypes.pythonapi.PyFunction_AddWatcher
        add_watcher.argtypes = [callback_type]
        add_watcher.restype = ctypes.c_int
        clear_watcher = ctypes.pythonapi.PyFunction_ClearWatcher
        clear_watcher.argtypes = [ctypes.c_int]
        clear_watcher.restype = ctypes.c_int

        watcher_id = add_watcher(watch_function)
        assert watcher_id >= 0, watcher_id

        try:
            values = []
            for offset in (1, 10, 100):
                values.append(module.captured(offset))
            assert values == [[1, 2, 3], [10, 11, 12], [100, 101, 102]]
            assert len(created) == 3, (len(created), callback_errors)

            cells = []
            for function in created:
                offset_index = function.__code__.co_freevars.index("offset")
                cells.append(function.__closure__[offset_index])
            assert len({{id(function) for function in created}}) == 3
            assert len({{id(cell) for cell in cells}}) == 3
            assert [cell.cell_contents for cell in cells] == [1, 10, 100]

            name_identity = [
                function.__name__ is function.__code__.co_name
                for function in created
            ]
            qualname_identity = [
                function.__qualname__ is function.__code__.co_qualname
                for function in created
            ]
            canonical_qualname_changes = list(qualname_changes)

            original_factory = runtime.code_with_freevars
            fallback_calls = []

            def noncanonical_factory(names, is_async, is_generator):
                fallback_calls.append(tuple(names))
                return original_factory(names, is_async, is_generator)

            runtime.code_with_freevars = noncanonical_factory
            try:
                assert module.noncanonical(5) == [5, 6, 7]
                assert module.noncanonical(50) == [50, 51, 52]
            finally:
                runtime.code_with_freevars = original_factory
            assert len(fallback_calls) == 2, fallback_calls

            first = module.original_outer(7)
            second = module.original_outer(70)
            assert first is not second
            assert first(1) == 8
            assert second(1) == 71
            assert first.__code__ is second.__code__
            assert first.__name__ == "original_inner"
            assert first.__qualname__ == "original_outer.<locals>.original_inner"
        finally:
            assert clear_watcher(watcher_id) == 0

        created[0].__name__ = "user_name"
        created[0].__qualname__ = "user.qualname"
        assert created[0].__name__ == "user_name"
        assert created[0].__qualname__ == "user.qualname"
        assert created[1].__name__ == "<listcomp>"

        print(json.dumps({{
            "created": len(created),
            "name_identity": name_identity,
            "qualname_identity": qualname_identity,
            "qualname_changes": canonical_qualname_changes,
            "fallback_calls": len(fallback_calls),
            "callback_errors": callback_errors,
        }}))
        """
    )

    env = dict(os.environ)
    env.pop("SOAC_LOG", None)
    env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(tmp_path / "soac-work"),
            "SOAC_OPT_MODE": "apply",
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        }
    )
    completed = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env=env,
        timeout=45,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    result = json.loads(completed.stdout.splitlines()[-1])
    assert result["callback_errors"] == []
    assert result["created"] == 3
    assert result["name_identity"] == [True, True, True], result
    assert result["qualname_identity"] == [True, True, True], result
    assert result["qualname_changes"] == [], result
    assert result["fallback_calls"] == 2
