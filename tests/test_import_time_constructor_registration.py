from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys


def test_import_time_constructor_registration_reaches_profile_and_apply(
    tmp_path: Path,
) -> None:
    module_name = "import_time_constructor_registration_case"
    (tmp_path / f"{module_name}.py").write_text(
        """
import ctypes

_get_type_id = ctypes.pythonapi.PyType_GetSoacFunctionId
_get_type_id.argtypes = [ctypes.py_object]
_get_type_id.restype = ctypes.c_uint64

_get_function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
_get_function_id.argtypes = [ctypes.py_object]
_get_function_id.restype = ctypes.c_uint64


class Box:
    def __init__(self, value):
        self.value = value


EARLY_BOX_ID = _get_type_id(Box)
INIT_FUNCTION_ID = _get_function_id(Box.__dict__["__init__"])
EVENTS = []
MODULE_LOOKUPS = []
NESTED_MODULE_LOOKUPS = []


class CustomNew:
    def __new__(cls, value):
        EVENTS.append(("new", value))
        return object.__new__(cls)

    def __init__(self, value):
        EVENTS.append(("new-init", value))
        self.value = value


EARLY_CUSTOM_NEW_ID = _get_type_id(CustomNew)


class InterceptingMeta(type):
    def __getattribute__(cls, name):
        if name == "__module__":
            class_name = type.__getattribute__(cls, "__name__")
            if class_name == "Inner":
                NESTED_MODULE_LOOKUPS.append("Outer" in globals())
            else:
                MODULE_LOOKUPS.append("CustomMeta" in globals())
        return type.__getattribute__(cls, name)

    def __call__(cls, value):
        EVENTS.append(("meta", value))
        return type.__call__(cls, value)


class CustomMeta(metaclass=InterceptingMeta):
    def __init__(self, value):
        EVENTS.append(("meta-init", value))
        self.value = value


EARLY_CUSTOM_META_ID = _get_type_id(CustomMeta)


class Outer:
    class Inner(metaclass=InterceptingMeta):
        def __init__(self, value):
            self.value = value

    def __init__(self, value):
        self.value = value


EARLY_INNER_ID = _get_type_id(Outer.Inner)


def run():
    total = 0
    for value in (1, 2, 3, 4):
        total += Box(value).value
    return total


RUN_FUNCTION_ID = _get_function_id(run)
RESULT = run()
FALLBACK_RESULTS = (CustomNew(8).value, CustomMeta(9).value)
""",
        encoding="utf-8",
    )

    work_dir = tmp_path / "soac-work"
    apply_log = work_dir / "apply-direct-edges.jsonl"
    script = "\n".join(
        [
            "import json",
            "import sys",
            f"sys.path.insert(0, {str(tmp_path)!r})",
            "from soac.import_hook import install",
            "install()",
            f"import {module_name} as module",
            "print(json.dumps({",
            '    "box_id": module.EARLY_BOX_ID,',
            '    "init_id": module.INIT_FUNCTION_ID,',
            '    "run_id": module.RUN_FUNCTION_ID,',
            '    "custom_new_id": module.EARLY_CUSTOM_NEW_ID,',
            '    "custom_meta_id": module.EARLY_CUSTOM_META_ID,',
            '    "nested_meta_id": module.EARLY_INNER_ID,',
            '    "result": module.RESULT,',
            '    "fallback_results": module.FALLBACK_RESULTS,',
            '    "events": module.EVENTS,',
            '    "module_lookups": module.MODULE_LOOKUPS,',
            '    "nested_module_lookups": module.NESTED_MODULE_LOOKUPS,',
            "}))",
            "",
        ]
    )
    base_env = dict(os.environ)
    base_env.pop("SOAC_LOG", None)
    base_env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(work_dir),
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        }
    )

    def run_mode(mode: str, *, log: Path | None = None) -> dict:
        env = {**base_env, "SOAC_OPT_MODE": mode}
        if log is not None:
            env["SOAC_LOG"] = f"soac_jit_direct_edges=info;json={log}"
        completed = subprocess.run(
            [sys.executable, "-c", script],
            check=False,
            capture_output=True,
            text=True,
            env=env,
            timeout=60,
        )
        assert completed.returncode == 0, completed.stdout + completed.stderr
        lines = [line for line in completed.stdout.splitlines() if line.strip()]
        result = json.loads(lines[-1])
        assert result["result"] == 10
        assert result["fallback_results"] == [8, 9]
        assert result["events"] == [
            ["new", 8],
            ["new-init", 8],
            ["meta", 9],
            ["meta-init", 9],
        ]
        assert result["custom_new_id"] == 0, result
        assert result["custom_meta_id"] == 0, result
        assert result["nested_meta_id"] == 0, result
        assert result["module_lookups"], result
        assert all(result["module_lookups"]), (
            "unsupported metaclass attribute hooks must not run before "
            "their class is assigned to its module globals",
            result,
        )
        assert result["nested_module_lookups"], result
        assert all(result["nested_module_lookups"]), (
            "nested unsupported metaclass hooks must not run before their "
            "owning class is assigned to its module globals",
            result,
        )
        assert result["init_id"] != 0, result
        assert result["run_id"] != 0, result
        return result

    profile = run_mode("profile")
    assert profile["box_id"] != 0, (
        "a safe constructor must be registered immediately after class creation, "
        "before module-level calls or post-import owner registration",
        profile,
    )
    assert profile["box_id"] != profile["init_id"], profile

    import _soac_ext

    counter_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    run_rows = [
        row
        for record in counter_dump["records"]
        if record["module_name"] == module_name
        for row in record["rows"]
        if row["function_qualname"] == "run"
        and row["function_id"] == profile["run_id"]
        and row["kind"] == "call_hot_targets"
    ]
    assert any(
        row.get("observed_value") == profile["box_id"] and row["value"] >= 4
        for row in run_rows
    ), run_rows

    apply = run_mode("apply", log=apply_log)
    assert apply["box_id"] != 0, apply
    assert apply["box_id"] != apply["init_id"], apply
    assert apply["box_id"] == profile["box_id"], (profile, apply)

    events = [
        json.loads(line)
        for line in apply_log.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    assert any(
        event.get("target") == "soac_jit_direct_edges"
        and event.get("module") == module_name
        and event.get("qualname") == "run"
        and event.get("clif_direct_edges", 0) > 0
        for event in events
    ), events
