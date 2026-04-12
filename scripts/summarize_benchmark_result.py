#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from collections import defaultdict
from pathlib import Path
from statistics import mean, median
from typing import Any


RAW_LOOPS_PER_S_RE = re.compile(r"^\s*(\d+)\s+(\d+)\s+loops/s\s*$")


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
    }

    pending_section: str | None = None
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
        if stripped.startswith("specialized run "):
            pending_section = "apply"
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
            pending_section = None

    apply_runs = result["apply_loops_per_s_runs"]
    result["apply_loops_per_s_median"] = int(median(apply_runs)) if apply_runs else None
    result["apply_loops_per_s_mean"] = int(round(mean(apply_runs))) if apply_runs else None
    result["apply_loops_per_s_min"] = min(apply_runs) if apply_runs else None
    result["apply_loops_per_s_max"] = max(apply_runs) if apply_runs else None
    return result


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
    by_name = {
        str(row["function_qualname"]): int(row["code_size"])
        for row in rows
    }
    total = sum(by_name.values())
    non_dp_total = sum(size for name, size in by_name.items() if not name.startswith("_dp_"))
    core_total = sum(
        size
        for name, size in by_name.items()
        if not name.startswith("_dp_") and name not in {"main", "pystones"}
    )
    top_functions = [
        {"function_qualname": name, "code_size_bytes": size}
        for name, size in sorted(by_name.items(), key=lambda item: item[1], reverse=True)[:10]
    ]
    return {
        "latest_process_id": process_id,
        "function_count": len(by_name),
        "total_code_size_bytes": total,
        "non_dp_code_size_bytes": non_dp_total,
        "core_code_size_bytes": core_total,
        "functions_by_name": by_name,
        "top_functions": top_functions,
    }


def format_summary(summary: dict[str, Any]) -> str:
    benchmark = summary["benchmark"]
    code_size = summary.get("jit_code_size")

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
        "apply loops/s runs: "
        + (", ".join(str(value) for value in benchmark["apply_loops_per_s_runs"]) or "n/a"),
        f"apply median loops/s: {maybe(benchmark['apply_loops_per_s_median'])}",
        f"apply mean loops/s: {maybe(benchmark['apply_loops_per_s_mean'])}",
        f"apply min loops/s: {maybe(benchmark['apply_loops_per_s_min'])}",
        f"apply max loops/s: {maybe(benchmark['apply_loops_per_s_max'])}",
    ]

    if code_size is None:
        lines.append("latest pystone jit code size: n/a")
        return "\n".join(lines) + "\n"

    lines.extend(
        [
            f"latest pystone jit process id: {code_size['latest_process_id']}",
            f"pystone total code size bytes: {code_size['total_code_size_bytes']}",
            f"pystone non-_dp_ code size bytes: {code_size['non_dp_code_size_bytes']}",
            f"pystone core code size bytes: {code_size['core_code_size_bytes']}",
            "largest pystone functions by code size:",
        ]
    )
    for entry in code_size["top_functions"]:
        lines.append(f"  {entry['function_qualname']}: {entry['code_size_bytes']}")
    return "\n".join(lines) + "\n"


def main() -> int:
    args = parse_args()
    result_dir = args.result_dir.resolve()
    summary = {
        "result_dir": str(result_dir),
        "benchmark": parse_benchmark_report(result_dir / "benchmark.txt"),
        "jit_code_size": parse_jit_code_size(result_dir / "counters" / "jit-bb-map.jsonl"),
    }

    if args.json_out is not None:
        args.json_out.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")

    print(format_summary(summary), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
