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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Summarize a SOAC benchmark result directory using benchmark.txt "
            "throughput and counters/jit-bb-map.jsonl code size."
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


def parse_jit_code_size(jit_bb_map_path: Path) -> dict[str, Any] | None:
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

    process_id = max(by_process)
    rows = by_process[process_id]
    by_name = {}
    for row in rows:
        qualname = str(row["function_qualname"])
        entry_kind = str(row.get("entry_kind") or "")
        if not entry_kind:
            symbol = str(row.get("symbol") or "")
            entry_kind = "default_direct_adapter" if ":defaults" in symbol else "direct_function_body"
        name = qualname if entry_kind == "direct_function_body" else f"{qualname} [{entry_kind}]"
        by_name[name] = {
            "code_size_bytes": int(row["code_size"]),
            "machine_block_count": len(row.get("bb_offsets") or []),
        }
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
        "latest_process_id": process_id,
        "function_count": len(by_name),
        "total_code_size_bytes": total,
        "total_machine_block_count": total_blocks,
        "non_dp_code_size_bytes": non_dp_total,
        "non_dp_machine_block_count": non_dp_blocks,
        "core_code_size_bytes": core_total,
        "core_machine_block_count": core_blocks,
        "functions_by_name": by_name,
        "top_functions": top_functions,
    }


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
                        refcount_locations[branch] += branch_count

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
            f"latest pystone jit process id: {code_size['latest_process_id']}",
            f"pystone total code size bytes: {code_size['total_code_size_bytes']}",
            f"pystone total machine blocks: {code_size['total_machine_block_count']}",
            f"pystone non-_dp_ code size bytes: {code_size['non_dp_code_size_bytes']}",
            f"pystone non-_dp_ machine blocks: {code_size['non_dp_machine_block_count']}",
            f"pystone core code size bytes: {code_size['core_code_size_bytes']}",
            f"pystone core machine blocks: {code_size['core_machine_block_count']}",
            "largest pystone functions by code size:",
        ]
    )
    for entry in code_size["top_functions"]:
        lines.append(
            f"  {entry['function_qualname']}: "
            f"{entry['code_size_bytes']} bytes, {entry['machine_block_count']} blocks"
        )
    return "\n".join(lines) + "\n"


def main() -> int:
    args = parse_args()
    result_dir = args.result_dir.resolve()
    summary = {
        "result_dir": str(result_dir),
        "benchmark": parse_benchmark_report(result_dir / "benchmark.txt"),
        "jit_code_size": parse_jit_code_size(result_dir / "counters" / "jit-bb-map.jsonl"),
        "specialization_runtime": parse_specialization_stats(result_dir),
    }

    if args.json_out is not None:
        args.json_out.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")

    print(format_summary(summary), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
