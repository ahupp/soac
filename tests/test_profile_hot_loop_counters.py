from __future__ import annotations

import json
from collections import defaultdict
from pathlib import Path

from tests._strict_integration import create_strict_project


def test_profile_preserves_counter_sites_inside_joined_hot_loop(tmp_path: Path) -> None:
    module_name = "profile_hot_loop_counter_case"
    project = create_strict_project(
        tmp_path,
        {
            f"{module_name}.py": """
# soac: module(strict_assign=true, checked_attr=true)

class Box:
    def __init__(self):
        self.value = 0


def advance(value: int) -> int:
    return value + 1


def checked(value: int) -> int:
    # Annotations do not restrict the ordinary arguments accepted by this body.
    return 42


def invoke(callback, value):
    return callback(value)


def entry_probe():
    return None


def run(values):
    box = Box()
    alias = None
    if values:
        alias = box
    else:
        alias = box

    total = 0
    for index in range(len(values)):
        alias.value = advance(values[index])
        values[index] = alias.value + index
        if index & 1:
            total += values[index]
        else:
            total += alias.value
    return total, values, alias.value
"""
        },
        modules={module_name: f"{module_name}.py"},
    )

    work_dir = tmp_path / "soac-work"
    script = "\n".join(
        [
            f"import {module_name} as module",
            "from soac import _soac_ext",
            "assert _soac_ext.strict_module_diagnostics(module)['sealed'] is True",
            "assert _soac_ext.strict_function_entry_kind(module.entry_probe) == 'checked_native'",
            "assert module.entry_probe() is None",
            "assert module.entry_probe() is None",
            "assert module.run([1, 2, 3, 4]) == (18, [2, 4, 6, 8], 5)",
            "assert module.invoke(module.checked, 41) == 42",
            "assert module.invoke(lambda value: value + 1, 41) == 42",
            "assert module.invoke(module.checked, 'wrong') == 42",
            "",
        ]
    )
    project.run(
        script,
        opt_mode="profile",
        extra_env={
            "SOAC_WORK_DIR": str(work_dir),
            "SOAC_ENABLE_PROFILED_COLD_BLOCKS": "1",
        },
    )

    import _soac_ext

    counter_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    # The former Rust runtime fixture bypassed actual source admission. Check
    # the exact entry count through a genuinely authenticated native function.
    entry_counts = [
        row["value"]
        for record in counter_dump["records"]
        if record["module_name"] == module_name
        for row in record["rows"]
        if row["function_qualname"] == "entry_probe" and row["kind"] == "block_entry"
    ]
    assert entry_counts and all(count == 2 for count in entry_counts), entry_counts
    run_rows = [
        row
        for record in counter_dump["records"]
        if record["module_name"] == module_name
        for row in record["rows"]
        if row["function_qualname"] == "run"
    ]

    def rows_of_kind(kind: str) -> list[dict]:
        return [row for row in run_rows if row["kind"] == kind]

    calls = [
        row
        for row in rows_of_kind("call_hot_targets")
        if row.get("observed_value") and row["value"] >= 4
    ]
    assert calls, run_rows

    # Source-owned calls stay profiled when arguments differ from annotations.
    # An ordinary callback still acquires no authenticated profile identity
    # merely because a strict function calls it.
    invoke_rows = [
        row
        for record in counter_dump["records"]
        if record["module_name"] == module_name
        for row in record["rows"]
        if row["function_qualname"] == "invoke" and row["kind"] == "call_hot_targets"
    ]
    assert any(row.get("observed_value") and row["value"] >= 2 for row in invoke_rows)
    assert any(
        row.get("observed_value") == 0 and row["value"] == 1 for row in invoke_rows
    )

    exact_int_operators = [
        row
        for row in rows_of_kind("operator_hot_shapes")
        if row.get("observed_value") == 0x0101 and row["value"] >= 4
    ]
    assert exact_int_operators, run_rows

    exact_list_gets = [
        row
        for row in rows_of_kind("getitem_hot_shapes")
        if row.get("observed_value") == 1 and row["value"] >= 4
    ]
    assert exact_list_gets, run_rows

    exact_list_sets = [
        row
        for row in rows_of_kind("setitem_hot_shapes")
        if row.get("observed_value") == 1 and row["value"] >= 4
    ]
    assert exact_list_sets, run_rows

    field_rows = rows_of_kind("field_access")
    assert (
        sum(row.get("branches", {}).get("generic_getattr", 0) for row in field_rows)
        >= 4
    ), run_rows
    assert (
        sum(row.get("branches", {}).get("generic_setattr", 0) for row in field_rows)
        >= 4
    ), run_rows

    branch_counts: dict[int, dict[int, int]] = defaultdict(dict)
    for row in rows_of_kind("branch_outcomes"):
        outcome = row.get("observed_value")
        if outcome in (0, 1) and row["value"]:
            branch_counts[row["instr_id"]][outcome] = row["value"]
    assert any(
        outcomes.get(0, 0) >= 2 and outcomes.get(1, 0) >= 2
        for outcomes in branch_counts.values()
    ), run_rows

    project.run(script, opt_mode="apply", extra_env={"SOAC_WORK_DIR": str(work_dir)})
