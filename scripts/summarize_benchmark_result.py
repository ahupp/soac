#!/usr/bin/env python3
from __future__ import annotations

import argparse
import importlib
import json
import re
from collections import defaultdict
from pathlib import Path
from statistics import mean, median
from typing import Any


RAW_LOOPS_PER_S_RE = re.compile(r"^\s*(\d+)\s+(\d+)\s+loops/s\s*$")
RUNTIME_FALLBACK_KINDS = {
    "global_indexed_fallback",
    "field_indexed_fallback",
    "operator_specialized_fallback",
    "getitem_specialized_fallback",
    "setitem_specialized_fallback",
    "call_direct_fallback",
}
DEOPT_CALL_KIND = "deopt_entry_guard_miss"
REFCOUNT_LOCATION_KIND = "runtime_decref_location"
RELEASE_REFCOUNT_FAMILIES = {
    "local_overwrite",
    "explicit_delete",
    "edge_release",
    "return_release",
    "raise_release",
    "owned_temporary",
    "container_overwrite_release",
    "exit_sweep",
}
ACQUIRE_REFCOUNT_FAMILIES = {
    "local_load_clone",
    "forwarded_value_clone",
    "stack_slot_clone",
    "borrowed_result_clone",
    "container_store_clone",
    "constant_clone",
    "entry_arg_clone",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Summarize a SOAC benchmark result directory using benchmark.txt "
            "throughput and counters/jit-code-summary.jsonl code size."
        )
    )
    parser.add_argument("result_dir", type=Path)
    parser.add_argument("--json-out", type=Path)
    return parser.parse_args()


def parse_benchmark_report(report_path: Path) -> dict[str, Any]:
    if not report_path.is_file():
        raise FileNotFoundError(f"benchmark report not found at {report_path}")

    result: dict[str, Any] = {
        "change_id": None,
        "commit_id": None,
        "description": None,
        "date": None,
        "profile_loops": None,
        "benchmark_loops": None,
        "verify_loops": None,
        "result_mode": None,
        "specialized_runs": None,
        "warmup_loops": None,
        "benchmark_cpu": None,
        "benchmark_constant_clocks": None,
        "cranelift_opt_level": None,
        "profile_loops_per_s": None,
        "verify_loops_per_s": None,
        "apply_loops_per_s_runs": [],
        "apply_refcounts_disabled_loops_per_s_runs": [],
    }

    pending_section: str | None = None
    current_apply_section = "apply"
    for raw_line in report_path.read_text().splitlines():
        line = raw_line.rstrip("\n")
        stripped = line.strip()

        if line.startswith("change_id "):
            result["change_id"] = line.removeprefix("change_id ").strip()
            continue
        if line.startswith("commit_id "):
            result["commit_id"] = line.removeprefix("commit_id ").strip()
            continue
        if line.startswith("description "):
            result["description"] = line.removeprefix("description ").strip()
            continue

        key_map = {
            "date:": "date",
            "profile loops:": "profile_loops",
            "benchmark loops:": "benchmark_loops",
            "verify loops:": "verify_loops",
            "result mode:": "result_mode",
            "specialized runs:": "specialized_runs",
            "warmup loops:": "warmup_loops",
            "benchmark cpu:": "benchmark_cpu",
            "benchmark constant clocks:": "benchmark_constant_clocks",
            "cranelift opt level:": "cranelift_opt_level",
        }
        matched_key = False
        for prefix, key in key_map.items():
            if line.startswith(prefix):
                value = line.removeprefix(prefix).strip()
                if key in {"profile_loops", "benchmark_loops", "verify_loops", "specialized_runs", "warmup_loops"}:
                    result[key] = int(value)
                else:
                    result[key] = value
                matched_key = True
                break
        if matched_key:
            continue

        if stripped == "jit transformed profile pass":
            pending_section = "profile"
            continue
        if stripped == "jit transformed verify pass":
            pending_section = "verify"
            continue
        if stripped.startswith("jit transformed specialized apply pass"):
            if "refcounts disabled" in stripped or "without refcounts" in stripped:
                current_apply_section = "apply_refcounts_disabled"
            else:
                current_apply_section = "apply"
            continue
        if stripped.startswith("specialized run "):
            pending_section = current_apply_section
            continue

        match = RAW_LOOPS_PER_S_RE.match(stripped)
        if match and pending_section is not None:
            loops_per_s = int(match.group(2))
            if pending_section == "profile":
                result["profile_loops_per_s"] = loops_per_s
            elif pending_section == "verify":
                result["verify_loops_per_s"] = loops_per_s
            elif pending_section == "apply":
                result["apply_loops_per_s_runs"].append(loops_per_s)
            elif pending_section == "apply_refcounts_disabled":
                result["apply_refcounts_disabled_loops_per_s_runs"].append(loops_per_s)
            pending_section = None

    add_run_stats(result, "apply")
    add_run_stats(result, "apply_refcounts_disabled")
    return result


def add_run_stats(result: dict[str, Any], key_prefix: str) -> None:
    runs = result[f"{key_prefix}_loops_per_s_runs"]
    result[f"{key_prefix}_loops_per_s_median"] = int(median(runs)) if runs else None
    result[f"{key_prefix}_loops_per_s_mean"] = int(round(mean(runs))) if runs else None
    result[f"{key_prefix}_loops_per_s_min"] = min(runs) if runs else None
    result[f"{key_prefix}_loops_per_s_max"] = max(runs) if runs else None


def select_jit_code_size_process(
    process_ids: list[int], benchmark: dict[str, Any] | None
) -> tuple[int, str]:
    if benchmark is not None:
        apply_run_count = len(benchmark.get("apply_loops_per_s_runs") or [])
        # benchmark.txt records process-backed passes in this order:
        # profile, verify, then each successful refcounts-enabled apply run.
        if apply_run_count > 0:
            apply_process_index = 2 + apply_run_count - 1
            if apply_process_index < len(process_ids):
                return process_ids[apply_process_index], "last_refcounts_enabled_apply"
    return process_ids[-1], "latest_process"


def select_refcounts_disabled_jit_code_size_process(
    process_ids: list[int], benchmark: dict[str, Any] | None
) -> tuple[int, str] | None:
    if benchmark is None:
        return None
    apply_run_count = len(benchmark.get("apply_loops_per_s_runs") or [])
    disabled_run_count = len(benchmark.get("apply_refcounts_disabled_loops_per_s_runs") or [])
    if disabled_run_count <= 0:
        return None
    disabled_process_index = 2 + apply_run_count + disabled_run_count - 1
    if disabled_process_index >= len(process_ids):
        return None
    return process_ids[disabled_process_index], "last_refcounts_disabled_apply"


def parse_jit_code_size(
    jit_bb_map_path: Path, benchmark: dict[str, Any] | None = None
) -> dict[str, Any] | None:
    if not jit_bb_map_path.is_file():
        return None

    by_process: dict[int, list[dict[str, Any]]] = defaultdict(list)
    for line in jit_bb_map_path.read_text().splitlines():
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        function_id = str(record.get("function_id", ""))
        if not function_id.startswith("1:"):
            continue
        process_id = int(record["process_id"])
        by_process[process_id].append(record)

    if not by_process:
        return None

    process_ids = sorted(by_process)
    process_id, selection = select_jit_code_size_process(process_ids, benchmark)
    return summarize_jit_code_size_rows(by_process[process_id], process_ids, process_id, selection)


def parse_refcounts_disabled_jit_code_size(
    jit_bb_map_path: Path, benchmark: dict[str, Any] | None = None
) -> dict[str, Any] | None:
    if not jit_bb_map_path.is_file():
        return None

    by_process: dict[int, list[dict[str, Any]]] = defaultdict(list)
    for line in jit_bb_map_path.read_text().splitlines():
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        function_id = str(record.get("function_id", ""))
        if not function_id.startswith("1:"):
            continue
        process_id = int(record["process_id"])
        by_process[process_id].append(record)

    if not by_process:
        return None

    process_ids = sorted(by_process)
    selected = select_refcounts_disabled_jit_code_size_process(process_ids, benchmark)
    if selected is None:
        return None
    process_id, selection = selected
    return summarize_jit_code_size_rows(by_process[process_id], process_ids, process_id, selection)


def summarize_jit_code_size_rows(
    rows: list[dict[str, Any]],
    process_ids: list[int],
    process_id: int,
    selection: str,
) -> dict[str, Any]:
    by_name = {}
    purpose_bytes: dict[str, int] = defaultdict(int)
    refcount_family_bytes: dict[str, int] = defaultdict(int)
    block_role_attributed_bytes: dict[str, int] = defaultdict(int)
    block_role_unattributed_bytes: dict[str, int] = defaultdict(int)
    block_role_purpose_bytes: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    unattributed_bytes = 0
    for row in rows:
        qualname = str(row["function_qualname"])
        entry_kind = str(row.get("entry_kind") or "")
        if not entry_kind:
            symbol = str(row.get("symbol") or "")
            entry_kind = "default_direct_adapter" if ":defaults" in symbol else "direct_function_body"
        name = qualname if entry_kind == "direct_function_body" else f"{qualname} [{entry_kind}]"
        by_name[name] = {
            "code_size_bytes": int(row["code_size"]),
            "machine_block_count": int(
                row.get("machine_block_count", len(row.get("bb_offsets") or []))
            ),
        }
        purpose_names = [str(name) for name in row.get("purpose_names") or []]
        purpose_values = [int(value) for value in row.get("purpose_bytes") or []]
        if len(purpose_names) == len(purpose_values):
            for purpose, value in zip(purpose_names, purpose_values, strict=True):
                purpose_bytes[purpose] += value
        refcount_family_names = [str(name) for name in row.get("refcount_family_names") or []]
        refcount_family_values = [
            int(value) for value in row.get("refcount_family_bytes") or []
        ]
        if len(refcount_family_names) == len(refcount_family_values):
            for family, value in zip(
                refcount_family_names, refcount_family_values, strict=True
            ):
                refcount_family_bytes[family] += value
        block_role_names = [str(name) for name in row.get("block_role_names") or []]
        block_role_attributed_values = [
            int(value) for value in row.get("block_role_attributed_bytes") or []
        ]
        block_role_unattributed_values = [
            int(value) for value in row.get("block_role_unattributed_bytes") or []
        ]
        if len(block_role_names) == len(block_role_attributed_values):
            for block_role, value in zip(
                block_role_names, block_role_attributed_values, strict=True
            ):
                block_role_attributed_bytes[block_role] += value
        if len(block_role_names) == len(block_role_unattributed_values):
            for block_role, value in zip(
                block_role_names, block_role_unattributed_values, strict=True
            ):
                block_role_unattributed_bytes[block_role] += value
        block_role_purpose_values = row.get("block_role_purpose_bytes") or []
        if len(block_role_names) == len(block_role_purpose_values):
            for block_role, values in zip(
                block_role_names, block_role_purpose_values, strict=True
            ):
                if len(purpose_names) != len(values):
                    continue
                for purpose, value in zip(purpose_names, values, strict=True):
                    block_role_purpose_bytes[block_role][purpose] += int(value)
        unattributed_bytes += int(row.get("unattributed_bytes") or 0)
    total = sum(row["code_size_bytes"] for row in by_name.values())
    total_blocks = sum(row["machine_block_count"] for row in by_name.values())
    non_dp_total = sum(
        row["code_size_bytes"]
        for name, row in by_name.items()
        if not name.startswith("_dp_")
    )
    non_dp_blocks = sum(
        row["machine_block_count"]
        for name, row in by_name.items()
        if not name.startswith("_dp_")
    )
    core_total = sum(
        row["code_size_bytes"]
        for name, row in by_name.items()
        if not name.startswith("_dp_") and name not in {"main", "pystones"}
    )
    core_blocks = sum(
        row["machine_block_count"]
        for name, row in by_name.items()
        if not name.startswith("_dp_") and name not in {"main", "pystones"}
    )
    top_functions = [
        {
            "function_qualname": name,
            "code_size_bytes": row["code_size_bytes"],
            "machine_block_count": row["machine_block_count"],
        }
        for name, row in sorted(
            by_name.items(), key=lambda item: item[1]["code_size_bytes"], reverse=True
        )[:10]
    ]
    return {
        "selected_process_id": process_id,
        "process_selection": selection,
        "available_process_ids": process_ids,
        "function_count": len(by_name),
        "total_code_size_bytes": total,
        "total_machine_block_count": total_blocks,
        "non_dp_code_size_bytes": non_dp_total,
        "non_dp_machine_block_count": non_dp_blocks,
        "core_code_size_bytes": core_total,
        "core_machine_block_count": core_blocks,
        "purpose_bytes": dict(sorted(purpose_bytes.items())),
        "refcount_family_bytes": dict(sorted(refcount_family_bytes.items())),
        "refcount_family_group_bytes": {
            "release": sum(
                refcount_family_bytes[family]
                for family in RELEASE_REFCOUNT_FAMILIES
                if family in refcount_family_bytes
            ),
            "acquire": sum(
                refcount_family_bytes[family]
                for family in ACQUIRE_REFCOUNT_FAMILIES
                if family in refcount_family_bytes
            ),
        },
        "unattributed_bytes": unattributed_bytes,
        "block_role_attributed_bytes": dict(sorted(block_role_attributed_bytes.items())),
        "block_role_unattributed_bytes": dict(sorted(block_role_unattributed_bytes.items())),
        "block_role_total_bytes": {
            block_role: block_role_attributed_bytes[block_role]
            + block_role_unattributed_bytes[block_role]
            for block_role in sorted(
                set(block_role_attributed_bytes) | set(block_role_unattributed_bytes)
            )
        },
        "block_role_purpose_bytes": {
            block_role: dict(sorted(purpose_map.items()))
            for block_role, purpose_map in sorted(block_role_purpose_bytes.items())
        },
        "functions_by_name": by_name,
        "top_functions": top_functions,
    }


def append_block_role_summary(
    lines: list[str], heading_prefix: str, code_size: dict[str, Any]
) -> None:
    if not code_size["block_role_total_bytes"]:
        return
    lines.append(f"{heading_prefix} emitted code bytes by block role:")
    for block_role, size in sorted(
        code_size["block_role_total_bytes"].items(), key=lambda item: (-item[1], item[0])
    ):
        attributed = code_size["block_role_attributed_bytes"].get(block_role, 0)
        unattributed = code_size["block_role_unattributed_bytes"].get(block_role, 0)
        lines.append(
            f"  {block_role}: {size} total "
            f"({attributed} attributed, {unattributed} unattributed)"
        )


def append_refcount_family_summary(
    lines: list[str], heading_prefix: str, code_size: dict[str, Any]
) -> None:
    if not code_size["refcount_family_bytes"]:
        return
    lines.append(f"{heading_prefix} emitted refcount bytes by semantic family:")
    for family, size in sorted(
        code_size["refcount_family_bytes"].items(), key=lambda item: (-item[1], item[0])
    ):
        lines.append(f"  {family}: {size}")
    refcount_bytes = code_size["purpose_bytes"].get("refcount", 0)
    tagged_bytes = sum(code_size["refcount_family_bytes"].values())
    family_group_bytes = code_size["refcount_family_group_bytes"]
    lines.append(f"  subtotal_release: {family_group_bytes['release']}")
    lines.append(f"  subtotal_acquire: {family_group_bytes['acquire']}")
    if refcount_bytes > tagged_bytes:
        lines.append(f"  unclassified_refcount: {refcount_bytes - tagged_bytes}")


def parse_specialization_runtime_stats(events_path: Path) -> dict[str, Any]:
    by_kind: dict[str, int] = defaultdict(int)
    if events_path.is_file():
        for line in events_path.read_text().splitlines():
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            if record.get("event") != "soac.specialization_runtime":
                continue
            kind = str(record.get("kind") or "")
            try:
                value = int(record.get("value") or 0)
            except (TypeError, ValueError):
                continue
            if value <= 0:
                continue
            by_kind[kind] += value

    guard_failures = by_kind.get(DEOPT_CALL_KIND, 0) + sum(
        by_kind.get(kind, 0) for kind in RUNTIME_FALLBACK_KINDS
    )
    return {
        "deopt_calls": by_kind.get(DEOPT_CALL_KIND, 0),
        "guard_failures": guard_failures,
        "by_kind": dict(sorted(by_kind.items())),
        "source": str(events_path),
    }


def parse_specialization_counter_dump_stats(counter_dump_path: Path) -> dict[str, Any] | None:
    if not counter_dump_path.is_file():
        return None

    try:
        soac = importlib.import_module("soac")
        ext = soac._soac_ext
    except Exception:
        return None

    try:
        records = json.loads(ext.inspect_counter_dump_json(str(counter_dump_path)))["records"]
    except Exception:
        return None

    by_kind: dict[str, int] = defaultdict(int)
    refcount_locations: dict[str, int] = defaultdict(int)
    for record in records:
        for row in record.get("rows", []):
            kind = str(row.get("kind") or "")
            try:
                value = int(row.get("value") or 0)
            except (TypeError, ValueError):
                continue
            if value <= 0:
                continue
            by_kind[kind] += value
            if kind == REFCOUNT_LOCATION_KIND:
                function_qualname = str(row.get("function_qualname") or "").strip()
                function_id = str(row.get("function_id") or "").strip()
                module_name = str(record.get("module_name") or "").strip()
                if function_qualname:
                    function_label = (
                        f"{module_name}.{function_qualname}"
                        if module_name and not function_qualname.startswith(f"{module_name}.")
                        else function_qualname
                    )
                elif function_id:
                    function_label = f"function={function_id}"
                else:
                    function_label = "function=<unknown>"
                branches = row.get("branches")
                if isinstance(branches, dict):
                    branch_items = branches.items()
                else:
                    branch_items = (
                        (branch_value.get("branch"), branch_value.get("value"))
                        for branch_value in row.get("branch_values", [])
                    )
                for branch, branch_count in branch_items:
                    branch = str(branch or "")
                    if not branch:
                        continue
                    try:
                        branch_count = int(branch_count or 0)
                    except (TypeError, ValueError):
                        continue
                    if branch_count > 0:
                        refcount_locations[f"{function_label}: {branch}"] += branch_count

    guard_failures = by_kind.get(DEOPT_CALL_KIND, 0) + sum(
        by_kind.get(kind, 0) for kind in RUNTIME_FALLBACK_KINDS
    )
    return {
        "deopt_calls": by_kind.get(DEOPT_CALL_KIND, 0),
        "guard_failures": guard_failures,
        "by_kind": dict(sorted(by_kind.items())),
        "top_refcount_decref_locations": [
            {"location": branch, "value": value}
            for branch, value in sorted(
                refcount_locations.items(), key=lambda item: (-item[1], item[0])
            )[:20]
        ],
        "source": str(counter_dump_path),
    }


def parse_specialization_stats(result_dir: Path) -> dict[str, Any]:
    return parse_specialization_counter_dump_stats(result_dir / "counters" / "verify.bin") or (
        parse_specialization_runtime_stats(result_dir / "counters" / "events.jsonl")
    )


def format_summary(summary: dict[str, Any]) -> str:
    benchmark = summary["benchmark"]
    code_size = summary.get("jit_code_size")
    disabled_code_size = summary.get("jit_code_size_refcounts_disabled")
    runtime_stats = summary.get("specialization_runtime") or {}

    def maybe(value: Any) -> str:
        return "n/a" if value is None or value == "" else str(value)

    lines = [
        "benchmark summary",
        f"change_id: {maybe(benchmark['change_id'])}",
        f"commit_id: {maybe(benchmark['commit_id'])}",
        f"description: {maybe(benchmark['description'])}",
        f"cranelift opt level: {maybe(benchmark['cranelift_opt_level'])}",
        f"profile loops/s: {maybe(benchmark['profile_loops_per_s'])}",
        f"verify loops/s: {maybe(benchmark['verify_loops_per_s'])}",
        "apply loops/s runs (refcounts enabled): "
        + (", ".join(str(value) for value in benchmark["apply_loops_per_s_runs"]) or "n/a"),
        f"apply median loops/s (refcounts enabled): {maybe(benchmark['apply_loops_per_s_median'])}",
        f"apply mean loops/s (refcounts enabled): {maybe(benchmark['apply_loops_per_s_mean'])}",
        f"apply min loops/s (refcounts enabled): {maybe(benchmark['apply_loops_per_s_min'])}",
        f"apply max loops/s (refcounts enabled): {maybe(benchmark['apply_loops_per_s_max'])}",
        "apply loops/s runs (refcounts disabled): "
        + (
            ", ".join(str(value) for value in benchmark["apply_refcounts_disabled_loops_per_s_runs"])
            or "n/a"
        ),
        "apply median loops/s (refcounts disabled): "
        f"{maybe(benchmark['apply_refcounts_disabled_loops_per_s_median'])}",
        "apply mean loops/s (refcounts disabled): "
        f"{maybe(benchmark['apply_refcounts_disabled_loops_per_s_mean'])}",
        "apply min loops/s (refcounts disabled): "
        f"{maybe(benchmark['apply_refcounts_disabled_loops_per_s_min'])}",
        "apply max loops/s (refcounts disabled): "
        f"{maybe(benchmark['apply_refcounts_disabled_loops_per_s_max'])}",
        f"specialization stats source: {maybe(runtime_stats.get('source'))}",
        f"specialization deopt calls: {maybe(runtime_stats.get('deopt_calls'))}",
        f"specialization guard failures: {maybe(runtime_stats.get('guard_failures'))}",
    ]
    by_kind = runtime_stats.get("by_kind") or {}
    if by_kind:
        lines.append("specialization counters by kind:")
        for kind, value in by_kind.items():
            lines.append(f"  {kind}: {value}")
    top_refcount_locations = runtime_stats.get("top_refcount_decref_locations") or []
    if top_refcount_locations:
        lines.append("top runtime_decref_location branches:")
        for entry in top_refcount_locations:
            lines.append(f"  {entry['value']}: {entry['location']}")

    if code_size is None:
        lines.append("latest pystone jit code size: n/a")
        return "\n".join(lines) + "\n"

    lines.extend(
        [
            f"pystone jit process id: {code_size['selected_process_id']}",
            f"pystone jit process selection: {code_size['process_selection']}",
            f"pystone total code size bytes: {code_size['total_code_size_bytes']}",
            f"pystone total machine blocks: {code_size['total_machine_block_count']}",
            f"pystone non-_dp_ code size bytes: {code_size['non_dp_code_size_bytes']}",
            f"pystone non-_dp_ machine blocks: {code_size['non_dp_machine_block_count']}",
            f"pystone core code size bytes: {code_size['core_code_size_bytes']}",
            f"pystone core machine blocks: {code_size['core_machine_block_count']}",
        ]
    )
    if disabled_code_size is not None:
        lines.extend(
            [
                f"pystone no-refcount jit process id: {disabled_code_size['selected_process_id']}",
                f"pystone no-refcount total code size bytes: {disabled_code_size['total_code_size_bytes']}",
                "pystone no-refcount code size delta bytes: "
                f"{disabled_code_size['total_code_size_bytes'] - code_size['total_code_size_bytes']}",
            ]
        )
        append_block_role_summary(lines, "pystone no-refcount", disabled_code_size)
        append_refcount_family_summary(lines, "pystone no-refcount", disabled_code_size)
    if code_size["purpose_bytes"]:
        lines.append("pystone emitted code bytes by purpose:")
        for purpose, size in sorted(
            code_size["purpose_bytes"].items(), key=lambda item: (-item[1], item[0])
        ):
            lines.append(f"  {purpose}: {size}")
        lines.append(f"pystone unattributed emitted code bytes: {code_size['unattributed_bytes']}")
    append_refcount_family_summary(lines, "pystone", code_size)
    if code_size["block_role_total_bytes"]:
        append_block_role_summary(lines, "pystone", code_size)
        cleanup_purpose_bytes = code_size["block_role_purpose_bytes"].get("cleanup")
        if cleanup_purpose_bytes:
            lines.append("pystone cleanup-block emitted code bytes by purpose:")
            for purpose, size in sorted(
                cleanup_purpose_bytes.items(), key=lambda item: (-item[1], item[0])
            ):
                lines.append(f"  {purpose}: {size}")
        refcount_support_purpose_bytes = code_size["block_role_purpose_bytes"].get(
            "refcount_support"
        )
        if refcount_support_purpose_bytes:
            lines.append("pystone refcount-support emitted code bytes by purpose:")
            for purpose, size in sorted(
                refcount_support_purpose_bytes.items(), key=lambda item: (-item[1], item[0])
            ):
                lines.append(f"  {purpose}: {size}")
    lines.append("largest pystone functions by code size:")
    for entry in code_size["top_functions"]:
        lines.append(
            f"  {entry['function_qualname']}: "
            f"{entry['code_size_bytes']} bytes, {entry['machine_block_count']} blocks"
        )
    return "\n".join(lines) + "\n"


def main() -> int:
    args = parse_args()
    result_dir = args.result_dir.resolve()
    benchmark = parse_benchmark_report(result_dir / "benchmark.txt")
    jit_code_summary_path = result_dir / "counters" / "jit-code-summary.jsonl"
    if not jit_code_summary_path.is_file():
        # Older result directories predate the compact code summary artifact.
        jit_code_summary_path = result_dir / "counters" / "jit-bb-map.jsonl"
    summary = {
        "result_dir": str(result_dir),
        "benchmark": benchmark,
        "jit_code_size": parse_jit_code_size(jit_code_summary_path, benchmark),
        "jit_code_size_refcounts_disabled": parse_refcounts_disabled_jit_code_size(
            jit_code_summary_path, benchmark
        ),
        "specialization_runtime": parse_specialization_stats(result_dir),
    }

    if args.json_out is not None:
        args.json_out.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")

    print(format_summary(summary), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
