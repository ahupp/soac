from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def _run_countered_worker(
    module_root: Path,
    work_dir: Path,
    module_name: str,
    specialization_mode: str,
) -> dict[str, object]:
    observation_path = module_root / f"{specialization_mode}-exit-observation.json"
    dump_name = "profile.bin" if specialization_mode == "profile" else "verify.bin"
    script = textwrap.dedent(
        f"""
        import atexit
        import ctypes
        import json
        import sys
        from pathlib import Path

        observation_path = Path({str(observation_path)!r})
        dump_path = Path({str(work_dir / dump_name)!r})
        held_modules = []

        def observe_before_module_teardown():
            observation = {{"seen": list(held_modules[1].SEEN)}}
            try:
                records = json.loads(
                    extension.inspect_counter_dump_json(str(dump_path))
                )["records"]
                module_counts = {{}}
                for record in records:
                    name = record["module_name"]
                    module_counts[name] = module_counts.get(name, 0) + 1
                observation["module_counts"] = module_counts
                observation["late_call_count"] = sum(
                    row["value"]
                    for record in records
                    if record["module_name"] == {module_name!r}
                    for row in record["rows"]
                    if row["kind"] == "call_hot_targets"
                    and row["function_id"] == run_function_id
                    and row["observed_value"] == mark_function_id
                )
            except Exception as error:
                observation["error"] = str(error)
            observation_path.write_text(json.dumps(observation), encoding="utf-8")

        # Importing any soac package initializes its native extension, so install the
        # observer first. The extension's later exit hook must run before this one.
        atexit.register(observe_before_module_teardown)

        sys.path.insert(0, {str(module_root)!r})
        from soac.import_hook import install
        import _soac_ext as extension

        install()
        import soac.runtime as runtime
        import {module_name} as workload

        # Keep both transformed modules alive across the observer: module m_clear
        # cannot accidentally make this regression pass before atexit runs.
        held_modules.extend((runtime, workload))
        get_function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
        get_function_id.argtypes = [ctypes.py_object]
        get_function_id.restype = ctypes.c_uint64
        run_function_id = int(get_function_id(workload.run))
        mark_function_id = int(get_function_id(workload.mark))
        assert run_function_id != 0
        assert mark_function_id != 0

        assert workload.run(10) == 10

        def late_user_exit_callback():
            assert workload.run(20) == 20

        # LIFO order: user callback, SOAC session flush, original observer.
        atexit.register(late_user_exit_callback)
        """
    )
    env = dict(os.environ)
    env.pop("SOAC_LOG", None)
    env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{module_root}",
            "SOAC_WORK_DIR": str(work_dir),
            "SOAC_OPT_MODE": specialization_mode,
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        }
    )
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env=env,
        timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert observation_path.exists(), result.stdout + result.stderr
    return json.loads(observation_path.read_text(encoding="utf-8"))


def _assert_single_module_records(path: Path, module_name: str) -> None:
    from soac import _soac_ext

    records = json.loads(_soac_ext.inspect_counter_dump_json(str(path)))["records"]
    assert sum(record["module_name"] == module_name for record in records) == 1
    assert sum(record["module_name"] == "soac.runtime" for record in records) == 1


def test_counter_dump_flushes_live_modules_once_after_user_exit_callbacks(
    tmp_path: Path,
) -> None:
    module_name = "counter_shutdown_flush_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            SEEN = []


            def mark(value):
                SEEN.append(value)
                return value


            def run(value):
                try:
                    next(iter(()))
                except StopIteration:
                    return mark(value)
                raise AssertionError("empty iterator did not stop")
            """
        ),
        encoding="utf-8",
    )
    work_dir = tmp_path / "soac-work"

    for mode, dump_name in (("profile", "profile.bin"), ("verify", "verify.bin")):
        observation = _run_countered_worker(tmp_path, work_dir, module_name, mode)
        assert "error" not in observation, observation
        assert observation["seen"] == [10, 20], observation
        module_counts = observation["module_counts"]
        assert module_counts.get(module_name) == 1, observation
        assert module_counts.get("soac.runtime") == 1, observation
        if mode == "profile":
            assert observation["late_call_count"] >= 2, observation
        _assert_single_module_records(work_dir / dump_name, module_name)

    apply_script = textwrap.dedent(
        f"""
        import sys

        sys.path.insert(0, {str(tmp_path)!r})
        from soac.import_hook import install
        install()
        import {module_name} as workload

        assert workload.run(30) == 30
        """
    )
    apply_env = dict(os.environ)
    apply_env.pop("SOAC_LOG", None)
    apply_env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(work_dir),
            "SOAC_OPT_MODE": "apply",
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        }
    )
    result = subprocess.run(
        [sys.executable, "-c", apply_script],
        check=False,
        capture_output=True,
        text=True,
        env=apply_env,
        timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
