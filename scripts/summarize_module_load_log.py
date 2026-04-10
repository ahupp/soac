#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import statistics
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any


DEFAULT_LOG = Path("logs/last_benchmark_counters/module_loads.jsonl")
MODULE_LOAD_EVENT = "soac.module_load"
MODULE_LOAD_PHASE_EVENT = "soac.module_load.phase"
JIT_CODEGEN_EVENT = "soac.jit_codegen"
JIT_CODEGEN_TIMING = "jit_codegen_total"
MODULE_LOAD_TIMING = "module_load_total"


@dataclass(frozen=True)
class TimingStats:
    count: int
    cumulative_ms: float
    median_ms: float
    max_ms: float
    max_owner: str


@dataclass(frozen=True)
class JitMax:
    elapsed_ms: float
    module_name: str
    qualname: str
    entry_kind: str


@dataclass(frozen=True)
class LogSummary:
    path: Path
    module_event_count: int
    module_status_counts: Counter[str]
    cumulative_module_load_ms: float
    module_timing_stats: dict[str, TimingStats]
    jit_event_count: int
    jit_status_counts: Counter[str]
    cumulative_jit_codegen_ms: float
    max_jit_codegen: JitMax | None


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Summarize SOAC module-load and JIT-codegen timing JSONL."
    )
    parser.add_argument(
        "log",
        nargs="?",
        default=DEFAULT_LOG,
        type=Path,
        help=f"module_loads.jsonl path (default: {DEFAULT_LOG})",
    )
    return parser.parse_args()


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    with path.open(encoding="utf-8") as file:
        for line_number, line in enumerate(file, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"{path}:{line_number}: invalid JSON: {exc}") from exc
            if isinstance(entry, dict):
                entries.append(entry)
    return entries


def numeric_timings(entry: dict[str, Any]) -> dict[str, float]:
    out: dict[str, float] = {}
    for name, value in entry.items():
        if isinstance(name, str) and name.endswith("_us") and isinstance(value, int | float):
            out[name.removesuffix("_us")] = float(value) / 1000.0

    raw_timings = entry.get("timings_ms", {})
    if not isinstance(raw_timings, dict):
        return out
    for name, value in raw_timings.items():
        if isinstance(name, str) and isinstance(value, int | float):
            out[name] = float(value)
    return out


def module_name(entry: dict[str, Any]) -> str:
    raw_flat_name = entry.get("module_name")
    if isinstance(raw_flat_name, str) and raw_flat_name:
        return raw_flat_name
    raw_flat_path = entry.get("path")
    if isinstance(raw_flat_path, str) and raw_flat_path:
        return raw_flat_path

    raw_module = entry.get("module", {})
    if not isinstance(raw_module, dict):
        return "<unknown>"
    raw_name = raw_module.get("module_name")
    if isinstance(raw_name, str) and raw_name:
        return raw_name
    raw_path = raw_module.get("path")
    if isinstance(raw_path, str) and raw_path:
        return raw_path
    return "<unknown>"


def function_name(entry: dict[str, Any]) -> tuple[str, str, str]:
    raw_flat_qualname = entry.get("function_qualname")
    if isinstance(raw_flat_qualname, str) and raw_flat_qualname:
        return (
            module_name(entry),
            raw_flat_qualname,
            str(entry.get("function_entry_kind", "<unknown>")),
        )

    raw_module = entry.get("module", {})
    module = raw_module.get("module_name", "<unknown>") if isinstance(raw_module, dict) else "<unknown>"
    raw_function = entry.get("function", {})
    if not isinstance(raw_function, dict):
        return str(module), "<unknown>", "<unknown>"
    qualname = raw_function.get("qualname", "<unknown>")
    entry_kind = raw_function.get("entry_kind", "<unknown>")
    return str(module), str(qualname), str(entry_kind)


def status(entry: dict[str, Any]) -> str:
    raw_status = entry.get("status")
    return raw_status if isinstance(raw_status, str) and raw_status else "<missing>"


def timing_stats(
    timing_values: dict[str, list[tuple[float, str]]],
) -> dict[str, TimingStats]:
    stats: dict[str, TimingStats] = {}
    for timing_name, samples in timing_values.items():
        values = [value for value, _owner in samples]
        max_value, max_owner = max(samples, key=lambda sample: sample[0])
        stats[timing_name] = TimingStats(
            count=len(values),
            cumulative_ms=sum(values),
            median_ms=statistics.median(values),
            max_ms=max_value,
            max_owner=max_owner,
        )
    return stats


def summarize_entries(path: Path, entries: list[dict[str, Any]]) -> LogSummary:
    module_events = [entry for entry in entries if entry.get("event") == MODULE_LOAD_EVENT]
    module_phase_events = [entry for entry in entries if entry.get("event") == MODULE_LOAD_PHASE_EVENT]
    jit_events = [entry for entry in entries if entry.get("event") == JIT_CODEGEN_EVENT]

    module_timing_values: dict[str, list[tuple[float, str]]] = defaultdict(list)
    cumulative_module_load_ms = 0.0
    for entry in module_events:
        owner = module_name(entry)
        timings = numeric_timings(entry)
        cumulative_module_load_ms += timings.get(MODULE_LOAD_TIMING, 0.0)
        for timing_name, value in timings.items():
            module_timing_values[timing_name].append((value, owner))

    for entry in module_phase_events:
        phase = entry.get("phase")
        elapsed = entry.get("elapsed_us")
        if isinstance(phase, str) and isinstance(elapsed, int | float):
            module_timing_values[phase].append((float(elapsed) / 1000.0, module_name(entry)))

    cumulative_jit_codegen_ms = 0.0
    max_jit_codegen: JitMax | None = None
    for entry in jit_events:
        elapsed = numeric_timings(entry).get(JIT_CODEGEN_TIMING)
        if elapsed is None:
            continue
        cumulative_jit_codegen_ms += elapsed
        module, qualname, entry_kind = function_name(entry)
        if max_jit_codegen is None or elapsed > max_jit_codegen.elapsed_ms:
            max_jit_codegen = JitMax(elapsed, module, qualname, entry_kind)

    return LogSummary(
        path=path,
        module_event_count=len(module_events),
        module_status_counts=Counter(status(entry) for entry in module_events),
        cumulative_module_load_ms=cumulative_module_load_ms,
        module_timing_stats=timing_stats(module_timing_values),
        jit_event_count=len(jit_events),
        jit_status_counts=Counter(status(entry) for entry in jit_events),
        cumulative_jit_codegen_ms=cumulative_jit_codegen_ms,
        max_jit_codegen=max_jit_codegen,
    )


def summarize_log(path: Path) -> LogSummary:
    return summarize_entries(path, load_jsonl(path))


def fmt_ms(value: float) -> str:
    return f"{value:10.3f}"


def print_status_counts(label: str, counts: Counter[str]) -> None:
    if not counts:
        return
    rendered = ", ".join(f"{name}={counts[name]}" for name in sorted(counts))
    print(f"{label}: {rendered}")


def print_summary(summary: LogSummary) -> None:
    print(f"log: {summary.path}")
    print(f"module-load events: {summary.module_event_count}")
    print_status_counts("module-load status", summary.module_status_counts)
    print(f"cumulative {MODULE_LOAD_TIMING}: {fmt_ms(summary.cumulative_module_load_ms)} ms")
    print()
    print("module timing medians/maxima:")
    print(f"{'timing':56} {'n':>5} {'total_ms':>10} {'median_ms':>10} {'max_ms':>10} max_module")
    for timing_name in sorted(summary.module_timing_stats):
        stats = summary.module_timing_stats[timing_name]
        print(
            f"{timing_name:56} {stats.count:5d} "
            f"{fmt_ms(stats.cumulative_ms)} {fmt_ms(stats.median_ms)} "
            f"{fmt_ms(stats.max_ms)} {stats.max_owner}"
        )

    print()
    print(f"jit-codegen events: {summary.jit_event_count}")
    print_status_counts("jit-codegen status", summary.jit_status_counts)
    print(f"cumulative {JIT_CODEGEN_TIMING}: {fmt_ms(summary.cumulative_jit_codegen_ms)} ms")
    if summary.max_jit_codegen is None:
        print("max jit-codegen: <none>")
    else:
        max_jit = summary.max_jit_codegen
        print(
            "max jit-codegen: "
            f"{fmt_ms(max_jit.elapsed_ms)} ms "
            f"{max_jit.module_name}.{max_jit.qualname} "
            f"({max_jit.entry_kind})"
        )


def main() -> None:
    args = parse_args()
    print_summary(summarize_log(args.log))


if __name__ == "__main__":
    main()
