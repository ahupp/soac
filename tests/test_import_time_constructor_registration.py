from __future__ import annotations

import json
from pathlib import Path

import pytest

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import _VALIDATION_PRELUDE, create_strict_project


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

    relative = f"{module_name}.py"
    original_source = (tmp_path / relative).read_bytes()
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in(original_source, relative)[0].decode()},
        modules={module_name: relative},
    )
    work_dir = tmp_path / "soac-work"
    apply_log = work_dir / "apply-direct-edges.jsonl"
    script = "\n".join(
        [
            "import json",
            "import sys",
            "from types import ModuleType",
            _VALIDATION_PRELUDE,
            "from soac import _soac_ext",
            "ordinary = ModuleType('ordinary_import_constructor_control')",
            f"exec(compile({original_source.decode()!r}, {str(tmp_path / relative)!r}, 'exec', dont_inherit=True), vars(ordinary))",
            "assert ordinary.RESULT == 10",
            "assert ordinary.FALLBACK_RESULTS == (8, 9)",
            "assert ordinary.EVENTS == [('new', 8), ('new-init', 8), ('meta', 9), ('meta-init', 9)]",
            "assert ordinary.EARLY_BOX_ID == ordinary.INIT_FUNCTION_ID == ordinary.RUN_FUNCTION_ID == 0",
            "assert ordinary.EARLY_CUSTOM_NEW_ID == ordinary.EARLY_CUSTOM_META_ID == ordinary.EARLY_INNER_ID == 0",
            "assert ordinary.MODULE_LOOKUPS == ordinary.NESTED_MODULE_LOOKUPS == []",
            "assert _soac_ext.strict_module_diagnostics(ordinary) is None",
            "assert owner(ordinary.run) is None and metadata(ordinary.run) is None",
            f"import {module_name} as module",
            "diagnostic = _soac_ext.strict_module_diagnostics(module)",
            "assert diagnostic is not None and diagnostic['sealed']",
            f"assert diagnostic['module_name'] == {module_name!r}",
            f"assert diagnostic['source_path'] == {str(project.project / relative)!r}",
            f"assert diagnostic['artifact_generation'] == {project.publication['generation']!r}",
            "assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'",
            "sealed_id = ctypes.pythonapi.PyFunction_GetSoacStrictId",
            "sealed_id.argtypes = [ctypes.py_object]",
            "sealed_id.restype = ctypes.c_uint64",
            "source_ids = {}",
            "for name in ('Box.__init__', 'run'):",
            "    function = _plain_function_witness(module, name)",
            "    source_ids[name] = sealed_id(function)",
            "    assert source_ids[name] > 0, name",
            "    assert owner(function) and metadata(function), name",
            "    assert module._get_function_id(function) == 0, name",
            "    assert _soac_ext.strict_function_entry_kind(function) == 'checked_native', name",
            "    control = _plain_function_witness(ordinary, name)",
            "    assert sealed_id(control) == 0 and owner(control) is None, name",
            "    assert metadata(control) is None, name",
            "type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner",
            "type_owner.argtypes = [ctypes.py_object]",
            "type_owner.restype = ctypes.c_void_p",
            "assert type_owner(module.Box) and type_owner(ordinary.Box) is None",
            # A custom allocator does not itself imply a dynamic class. Its
            # constructor fast path and its enforced type are separate facts.
            "assert type_owner(module.CustomNew)",
            "for cls in (module.Box, module.CustomNew, module.CustomMeta, module.Outer.Inner):",
            "    assert module._get_type_id(cls) == 0, cls",
            "for cls in (module.CustomMeta, module.Outer.Inner):",
            "    assert type_owner(cls) is None, cls",
            "print(json.dumps({",
            '    "box_id": module.EARLY_BOX_ID,',
            '    "init_id": module.INIT_FUNCTION_ID,',
            '    "run_id": module.RUN_FUNCTION_ID,',
            '    "source_ids": source_ids,',
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

    def run_mode(mode: str, *, log: Path | None = None) -> dict:
        env = {
            "SOAC_WORK_DIR": str(work_dir),
            "SOAC_LOG": "",
            # Record actual body execution, including the import-time calls,
            # without manufacturing optional unchecked constructor identities.
            "SOAC_ENABLE_PROFILED_COLD_BLOCKS": "1",
        }
        if log is not None:
            env["SOAC_LOG"] = f"soac_jit_direct_edges=info;json={log}"
        completed = project.run(
            script,
            opt_mode=mode,
            extra_env=env,
            check=False,
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
        assert result["box_id"] == result["init_id"] == result["run_id"] == 0, (
            "mandatory checked entries must not publish unchecked function or "
            "constructor targets, including during the original module body",
            result,
        )
        assert result["source_ids"]["Box.__init__"] > 0, result
        assert result["source_ids"]["run"] > 0, result
        assert result["source_ids"]["Box.__init__"] != result["source_ids"]["run"], result
        return result

    profile = run_mode("profile")

    import _soac_ext

    counter_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    body_rows = [
        row
        for record in counter_dump["records"]
        if record["module_name"] == module_name
        for row in record["rows"]
        if row["kind"] == "block_entry"
    ]
    for qualname, minimum in (("Box.__init__", 4), ("run", 1)):
        selected = [
            row for row in body_rows
            if row["function_qualname"] == qualname
            and row["function_id"] == profile["source_ids"][qualname]
        ]
        assert any(row["value"] >= minimum for row in selected), (
            "the original import-time calls must execute the admitted body; "
            "a sealed identity or a compiled-but-unused body is insufficient",
            qualname,
            selected,
        )

    apply = run_mode("apply", log=apply_log)
    assert apply["source_ids"] == profile["source_ids"], (profile, apply)

    native = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text().splitlines()
        if line.strip()
    ]
    for qualname in ("Box.__init__", "run"):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == qualname
            and row.get("code_size", 0) > 0
            for row in native
        ), (qualname, native)

    events = [
        json.loads(line)
        for line in apply_log.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    assert not any(
        event.get("target") == "soac_jit_direct_edges"
        and event.get("module") == module_name
        and event.get("qualname") == "run"
        and event.get("clif_direct_edges", 0) > 0
        for event in events
    ), (
        "profile observations do not authorize bypassing the constructor's "
        "mandatory checked entry",
        events,
    )


@pytest.mark.parametrize("ordinary_owner", [False, True], ids=["strict-owner", "ordinary-owner"])
def test_field_map_resolution_does_not_run_inherited_instance_callbacks(
    tmp_path: Path, ordinary_owner: bool,
) -> None:
    from tests._strict_integration import StrictValidationCase

    module_name = "field_map_callback_case"
    relative = f"{module_name}.py"
    source = """
EVENTS = []


class Base:
    def __init__(self):
        EVENTS.append("init")
        self.value = 17

    def __del__(self):
        EVENTS.append("del")


class Child(Base):
    pass


def read_value(instance):
    return instance.value
"""
    validation = f"""
def validate_module(module):
    import gc
    import weakref

    if {ordinary_owner!r}:
        import ctypes
        type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
        type_owner.argtypes = [ctypes.py_object]
        type_owner.restype = ctypes.c_void_p
        function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        function_owner.argtypes = [ctypes.py_object]
        function_owner.restype = ctypes.c_void_p
        assert type_owner(module.Child) is None
        assert function_owner(module.Base.__init__) is None
    assert module.EVENTS == [], ("import ran instance callbacks", module.EVENTS)
    instance = module.Child()
    assert module.EVENTS == ["init"], module.EVENTS
    reference = weakref.ref(instance)
    for _ in range(64):
        assert module.read_value(instance) == 17
        assert module.EVENTS == ["init"], (
            "field-map compilation ran instance callbacks", module.EVENTS,
        )
    del instance
    gc.collect()
    assert reference() is None, "the actual instance was retained"
    assert module.EVENTS == ["init", "del"], module.EVENTS
"""
    owner_module_name = module_name
    selected_source = source
    sources = {}
    required_functions = ("Base.__init__", "Base.__del__", "read_value")
    if ordinary_owner:
        ordinary_module_name = "ordinary_field_map_owner"
        # The ordinary Base has no installed class contract. Creating Child
        # in selected source automatically falls back for that mutable base,
        # but still arms production observation of its real split-key layout.
        sources[f"{ordinary_module_name}.py"] = source
        selected_source = (
            f"from {ordinary_module_name} import Base, EVENTS\n\n"
            + source[source.index("class Child(Base):"):]
        )
        required_functions = ("read_value",)
    sources[relative] = strict_opt_in(selected_source.encode(), relative)[0].decode()
    project = create_strict_project(
        tmp_path / "strict-project",
        sources,
        modules={module_name: relative},
    )
    case = StrictValidationCase(
        validation, Path(__file__),
        required_functions=required_functions,
    )
    program = _VALIDATION_PRELUDE + f"""
from tests._integration import stock_module
from soac import _soac_ext

with stock_module(Path({str(tmp_path / "ordinary")!r}), "field_map_control", {source!r}) as ordinary:
    assert _soac_ext.strict_module_diagnostics(ordinary) is None
    assert owner(ordinary.read_value) is None and metadata(ordinary.read_value) is None
    exec_integration_validation({validation!r}, ordinary, Path({str(__file__)!r}), mode="stock")
"""
    if ordinary_owner:
        program += f"import {ordinary_module_name}\n"
    program += project._validation_program(module_name, case, entry_interpreter=False)
    work_dir = tmp_path / "soac-work"

    def run_mode(mode: str) -> None:
        completed = project.run(
            program, opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(work_dir)}, check=False, timeout=90,
        )
        assert completed.returncode == 0, (
            mode, completed.stdout, completed.stderr,
        )

    run_mode("profile")

    from soac import _soac_ext

    profile = json.loads(_soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin")))
    records = [record for record in profile["records"] if record["module_name"] == module_name]
    assert any(
        row["kind"] == "field_access"
        and row["function_qualname"] == "read_value"
        and row.get("branches", {}).get("generic_getattr", 0) >= 64
        for record in records for row in record["rows"]
    ), records
    child_ids = {
        entry["type_id"]
        for record in profile["records"] for entry in record["type_table"]
        if entry["module_name"] == owner_module_name and entry["qualname"] == "Child"
    }
    assert any(
        key["owner_type_id"] in child_ids and key["key"] == "value"
        for record in profile["records"] for key in record["type_keys"]
    ), profile

    run_mode("verify")
    run_mode("apply")
