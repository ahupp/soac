import importlib.util
import json
from pathlib import Path


def _load_summary_module():
    script = Path(__file__).resolve().parents[1] / "scripts" / "summarize_benchmark_result.py"
    spec = importlib.util.spec_from_file_location("summarize_benchmark_result", script)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_code_size_uses_last_successful_refcount_apply_process(tmp_path: Path) -> None:
    summary = _load_summary_module()
    jit_bb_map = tmp_path / "jit-bb-map.jsonl"
    rows = [
        {
            "process_id": process_id,
            "function_id": function_id,
            "function_qualname": qualname,
            "entry_kind": "direct_function_body",
            "code_size": code_size,
            "bb_offsets": list(range(blocks)),
            "purpose_names": purpose_names,
            "purpose_bytes": purpose_bytes,
            "refcount_family_names": refcount_family_names,
            "refcount_family_bytes": refcount_family_bytes,
            "unattributed_bytes": unattributed_bytes,
            "block_role_names": block_role_names,
            "block_role_attributed_bytes": block_role_attributed_bytes,
            "block_role_unattributed_bytes": block_role_unattributed_bytes,
            "block_role_purpose_bytes": block_role_purpose_bytes,
        }
        for (
            process_id,
            function_id,
            qualname,
            code_size,
            blocks,
            purpose_names,
            purpose_bytes,
            refcount_family_names,
            refcount_family_bytes,
            unattributed_bytes,
            block_role_names,
            block_role_attributed_bytes,
            block_role_unattributed_bytes,
            block_role_purpose_bytes,
        ) in [
            (10, "1:6", "pystones", 100, 1, [], [], [], [], 0, [], [], [], []),
            (20, "1:6", "pystones", 200, 2, [], [], [], [], 0, [], [], [], []),
            (30, "1:6", "pystones", 300, 3, [], [], [], [], 0, [], [], [], []),
            (40, "1:6", "pystones", 400, 4, [], [], [], [], 0, [], [], [], []),
            (
                50,
                "1:6",
                "pystones",
                500,
                5,
                ["refcount", "deopt"],
                [320, 12],
                ["exit_sweep", "local_load_clone"],
                [120, 40],
                168,
                ["cleanup", "ordinary", "refcount_support", "unknown"],
                [140, 152, 40, 0],
                [90, 40, 20, 18],
                [[120, 0], [160, 12], [40, 0], [0, 0]],
            ),
            (60, "1:7", "_dp_listcomp_3", 12, 1, [], [], [], [], 0, [], [], [], []),
        ]
    ]
    jit_bb_map.write_text("\n".join(json.dumps(row) for row in rows) + "\n")

    code_size = summary.parse_jit_code_size(
        jit_bb_map,
        {
            "apply_loops_per_s_runs": [1, 2, 3],
        },
    )

    assert code_size["selected_process_id"] == 50
    assert code_size["process_selection"] == "last_refcounts_enabled_apply"
    assert code_size["total_code_size_bytes"] == 500
    assert code_size["purpose_bytes"] == {"deopt": 12, "refcount": 320}
    assert code_size["refcount_family_bytes"] == {
        "exit_sweep": 120,
        "local_load_clone": 40,
    }
    assert code_size["refcount_family_group_bytes"] == {
        "release": 120,
        "acquire": 40,
    }
    assert code_size["unattributed_bytes"] == 168
    assert code_size["block_role_attributed_bytes"] == {
        "cleanup": 140,
        "ordinary": 152,
        "refcount_support": 40,
        "unknown": 0,
    }
    assert code_size["block_role_unattributed_bytes"] == {
        "cleanup": 90,
        "ordinary": 40,
        "refcount_support": 20,
        "unknown": 18,
    }
    assert code_size["block_role_total_bytes"] == {
        "cleanup": 230,
        "ordinary": 192,
        "refcount_support": 60,
        "unknown": 18,
    }
    assert code_size["block_role_purpose_bytes"] == {
        "cleanup": {"deopt": 0, "refcount": 120},
        "ordinary": {"deopt": 12, "refcount": 160},
        "refcount_support": {"deopt": 0, "refcount": 40},
        "unknown": {"deopt": 0, "refcount": 0},
    }


def test_code_size_can_select_last_successful_no_refcount_apply_process(tmp_path: Path) -> None:
    summary = _load_summary_module()
    jit_bb_map = tmp_path / "jit-bb-map.jsonl"
    rows = [
        {
            "process_id": process_id,
            "function_id": "1:6",
            "function_qualname": "pystones",
            "entry_kind": "direct_function_body",
            "code_size": code_size,
            "bb_offsets": [0],
            "purpose_names": ["refcount"],
            "purpose_bytes": [refcount_bytes],
            "unattributed_bytes": code_size - refcount_bytes,
            "block_role_names": ["ordinary", "unknown"],
            "block_role_attributed_bytes": [refcount_bytes, 0],
            "block_role_unattributed_bytes": [code_size - refcount_bytes, 0],
            "block_role_purpose_bytes": [[refcount_bytes], [0]],
        }
        for process_id, code_size, refcount_bytes in [
            (10, 100, 10),
            (20, 200, 20),
            (30, 300, 30),
            (40, 400, 40),
            (50, 500, 50),
            (60, 240, 0),
            (70, 220, 0),
        ]
    ]
    jit_bb_map.write_text("\n".join(json.dumps(row) for row in rows) + "\n")

    code_size = summary.parse_refcounts_disabled_jit_code_size(
        jit_bb_map,
        {
            "apply_loops_per_s_runs": [1, 2, 3],
            "apply_refcounts_disabled_loops_per_s_runs": [4, 5],
        },
    )

    assert code_size["selected_process_id"] == 70
    assert code_size["process_selection"] == "last_refcounts_disabled_apply"
    assert code_size["total_code_size_bytes"] == 220


def test_compact_code_summary_uses_machine_block_count(tmp_path: Path) -> None:
    summary = _load_summary_module()
    code_summary = tmp_path / "jit-code-summary.jsonl"
    code_summary.write_text(
        json.dumps(
            {
                "process_id": 20,
                "function_id": "1:6",
                "function_qualname": "pystones",
                "entry_kind": "direct_function_body",
                "code_size": 321,
                "machine_block_count": 17,
            }
        )
        + "\n"
    )

    code_size = summary.parse_jit_code_size(
        code_summary,
        {
            "apply_loops_per_s_runs": [1],
        },
    )

    assert code_size["total_code_size_bytes"] == 321
    assert code_size["total_machine_block_count"] == 17
