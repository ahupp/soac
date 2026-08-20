from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import textwrap

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import ROOT, StrictProject, create_strict_project


_SOURCE = textwrap.dedent(
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
)

_FUNCTION_APIS = """
def function_api(name, result_type):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result_type
    return function

function_owner = function_api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
function_metadata = function_api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
unchecked_target = function_api('PyFunction_GetSoacFunctionId', ctypes.c_uint64)
source_id = function_api('PyFunction_GetSoacStrictId', ctypes.c_uint64)
"""


def _checked_function_witnesses(project: StrictProject, module_name: str) -> str:
    return _FUNCTION_APIS + textwrap.dedent(
        f"""
        diagnostic = extension.strict_module_diagnostics(workload)
        assert diagnostic is not None and diagnostic['sealed']
        assert diagnostic['module_name'] == {module_name!r}
        assert diagnostic['source_path'] == {str(project.project / f'{module_name}.py')!r}
        assert diagnostic['artifact_generation'] == {project.publication['generation']!r}
        assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
        source_ids = {{}}
        for name in ('run', 'mark'):
            function = vars(workload)[name]
            witness = (function_owner(function), function_metadata(function),
                       unchecked_target(function), source_id(function),
                       extension.strict_function_entry_kind(function))
            assert witness[0] and witness[1], (name, witness)
            assert witness[2] == 0 and witness[3] > 0, (name, witness)
            assert witness[4] == 'checked_native', (name, witness)
            source_ids[name] = witness[3]
        assert len(set(source_ids.values())) == 2, source_ids
        """
    )


def _run_ordinary_worker(module_root: Path, module_name: str) -> dict[str, object]:
    observation_path = module_root / "ordinary-exit-observation.json"
    script = textwrap.dedent(
        f"""
        import atexit
        import ctypes
        import json
        import sys
        import types
        from pathlib import Path

        held_modules = []
        exit_order = []

        def observe_before_module_teardown():
            exit_order.append('observer')
            Path({str(observation_path)!r}).write_text(json.dumps({{
                'seen': list(held_modules[0].SEEN), 'exit_order': exit_order,
            }}), encoding='utf-8')

        atexit.register(observe_before_module_teardown)
        sys.path.insert(0, {str(module_root)!r})
        import {module_name} as workload
        assert type(workload) is types.ModuleType
        held_modules.append(workload)
        """
    ) + _FUNCTION_APIS + textwrap.dedent(
        """
        for name in ('run', 'mark'):
            function = vars(workload)[name]
            assert function_owner(function) is None
            assert function_metadata(function) is None
            assert unchecked_target(function) == source_id(function) == 0
        assert workload.run(10) == 10

        def late_user_exit_callback():
            exit_order.append('user')
            assert workload.run(20) == 20

        atexit.register(late_user_exit_callback)
        """
    )
    driver = module_root / "ordinary-shutdown-driver.py"
    driver.write_text(script, encoding="utf-8")
    result = subprocess.run(
        [sys.executable, "-I", "-B", str(driver)],
        check=False,
        capture_output=True,
        text=True,
        timeout=30,
    )
    (module_root / "ordinary-shutdown.stdout.log").write_text(result.stdout)
    (module_root / "ordinary-shutdown.stderr.log").write_text(result.stderr)
    assert result.returncode == 0, result.stdout + result.stderr
    assert observation_path.exists(), result.stdout + result.stderr
    return json.loads(observation_path.read_text(encoding="utf-8"))


def _run_countered_worker(
    project: StrictProject,
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
        exit_order = []

        def observe_before_module_teardown():
            exit_order.append('observer')
            observation = {{"seen": list(held_modules[1].SEEN), "exit_order": exit_order}}
            try:
                records = json.loads(
                    extension.inspect_counter_dump_json(str(dump_path))
                )["records"]
                module_counts = {{}}
                for record in records:
                    name = record["module_name"]
                    module_counts[name] = module_counts.get(name, 0) + 1
                observation["module_counts"] = module_counts
                observation['body_entry_counts'] = {{
                    name: max((
                        row['value']
                        for record in records
                        if record['module_name'] == {module_name!r}
                        for row in record['rows']
                        if row['kind'] == 'block_entry'
                        and row['function_qualname'] == name
                        and row['function_id'] == identity
                    ), default=0)
                    for name, identity in source_ids.items()
                }}
                # Mandatory checked functions publish no unchecked target ID.
                # Actual body activity must still include the late user call.
                observation['late_call_count'] = observation['body_entry_counts']['mark']
            except Exception as error:
                observation["error"] = str(error)
            observation_path.write_text(json.dumps(observation), encoding="utf-8")

        # Importing any soac package initializes its native extension, so install the
        # observer first. The extension's later exit hook must run before this one.
        atexit.register(observe_before_module_teardown)

        from soac import _soac_ext as extension, import_hook

        extension.force_entry_interpreter_for_tests(False)
        import_hook.install(backend='soac')
        import soac.runtime as runtime
        import {module_name} as workload

        # Infrastructure is an ordinary dependency, not one of this project's
        # authenticated selected modules. It must not acquire a counter record.
        assert extension.strict_module_diagnostics(runtime) is None

        # Keep both modules alive across the observer: module m_clear
        # cannot accidentally make this regression pass before atexit runs.
        held_modules.extend((runtime, workload))
        """
    ) + _checked_function_witnesses(project, module_name) + textwrap.dedent(
        """
        assert workload.run(10) == 10

        def late_user_exit_callback():
            exit_order.append('user')
            assert workload.run(20) == 20

        # LIFO order: user callback, SOAC session flush, original observer.
        atexit.register(late_user_exit_callback)
        """
    )
    # This one subprocess must register its observer before importing SOAC.
    # StrictProject.run imports the extension in its bootstrap first, which
    # would reverse the atexit order under test. Use the same authenticated
    # deployment and captured environment, never hook installation as authority.
    from soac import _soac_ext

    paths = [
        str(ROOT / "soac_py" / "src"),
        str(Path(_soac_ext.__file__).parent),
        str(project.project),
    ]
    script = "import sys\n" + f"sys.path[:0] = {paths!r}\n" + script
    driver = module_root / f"{specialization_mode}-shutdown-driver.py"
    driver.write_text(script, encoding="utf-8")
    env = dict(project.environment)
    env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{project.project / f'{module_name}.py'}",
            "SOAC_WORK_DIR": str(work_dir),
            "SOAC_OPT_MODE": specialization_mode,
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
            "DIET_PYTHON_MODE": "transform",
            "SOAC_LOG": "",
            "SOAC_ENABLE_PROFILED_COLD_BLOCKS": "1",
        }
    )
    result = subprocess.run(
        [
            sys.executable,
            "-I",
            "-B",
            "-X",
            f"soac_strict_config={project.deployment}",
            str(driver),
        ],
        check=False,
        capture_output=True,
        text=True,
        env=env,
        cwd=ROOT,
        timeout=30,
    )
    (module_root / f"{specialization_mode}-shutdown.stdout.log").write_text(result.stdout)
    (module_root / f"{specialization_mode}-shutdown.stderr.log").write_text(result.stderr)
    assert result.returncode == 0, result.stdout + result.stderr
    assert observation_path.exists(), result.stdout + result.stderr
    return json.loads(observation_path.read_text(encoding="utf-8"))


def _assert_single_module_records(path: Path, module_name: str) -> None:
    from soac import _soac_ext

    records = json.loads(_soac_ext.inspect_counter_dump_json(str(path)))["records"]
    assert sum(record["module_name"] == module_name for record in records) == 1
    assert not any(record["module_name"] == "soac.runtime" for record in records)


def test_counter_dump_flushes_live_modules_once_after_user_exit_callbacks(
    tmp_path: Path,
) -> None:
    module_name = "counter_shutdown_flush_case"
    relative = f"{module_name}.py"
    (tmp_path / relative).write_text(_SOURCE, encoding="utf-8")
    ordinary = _run_ordinary_worker(tmp_path, module_name)
    assert ordinary == {"seen": [10, 20], "exit_order": ["user", "observer"]}, ordinary
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in(_SOURCE.encode(), relative)[0].decode()},
        modules={module_name: relative},
    )
    work_dir = tmp_path / "soac-work"

    for mode, dump_name in (("profile", "profile.bin"), ("verify", "verify.bin")):
        observation = _run_countered_worker(project, tmp_path, work_dir, module_name, mode)
        assert "error" not in observation, observation
        assert observation["seen"] == [10, 20], observation
        assert observation["exit_order"] == ordinary["exit_order"], observation
        module_counts = observation["module_counts"]
        assert module_counts.get(module_name) == 1, observation
        assert "soac.runtime" not in module_counts, observation
        if mode == "profile":
            assert observation["late_call_count"] >= 2, observation
            assert observation["body_entry_counts"]["run"] >= 2, observation
        _assert_single_module_records(work_dir / dump_name, module_name)

    apply_script = textwrap.dedent(
        f"""
        import ctypes
        from soac import _soac_ext as extension
        import {module_name} as workload
        """
    ) + _checked_function_witnesses(project, module_name) + "\nassert workload.run(30) == 30\n"
    result = project.run(
        apply_script,
        opt_mode="apply",
        extra_env={
            "SOAC_WORK_DIR": str(work_dir),
            "SOAC_LOG": "",
            "SOAC_ENABLE_PROFILED_COLD_BLOCKS": "1",
        },
        check=False,
        timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
