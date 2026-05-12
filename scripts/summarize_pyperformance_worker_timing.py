#!/usr/bin/env python3
"""Summarize SOAC pyperformance worker setup and measured-value wall time."""

from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path
from typing import Iterable


TIMING_FILENAME = "pyperformance-worker-timing.jsonl"
TIMING_RECORD_TYPE = "pyperformance_worker_timing_v1"


def _format_ns(value: int) -> str:
    seconds = value / 1_000_000_000
    if seconds >= 10:
        return f"{seconds:.1f} s"
    if seconds >= 1:
        return f"{seconds:.2f} s"
    milliseconds = value / 1_000_000
    if milliseconds >= 10:
        return f"{milliseconds:.1f} ms"
    return f"{milliseconds:.2f} ms"


def _timing_records(work_root: Path) -> Iterable[dict[str, object]]:
    for path in sorted(work_root.rglob(TIMING_FILENAME)):
        with path.open(encoding="utf-8") as handle:
            for line in handle:
                try:
                    record = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if record.get("record_type") == TIMING_RECORD_TYPE:
                    yield record


def _int_values(records: list[dict[str, object]], key: str) -> list[int]:
    values = []
    for record in records:
        value = record.get(key)
        if isinstance(value, int):
            values.append(value)
    return values


def _summary_line(label: str, values: list[int]) -> str:
    total = sum(values)
    median = int(statistics.median(values))
    maximum = max(values)
    return (
        f"  {label:<29} total {_format_ns(total):>9}  "
        f"median {_format_ns(median):>9}  max {_format_ns(maximum):>9}"
    )


def main(argv: list[str]) -> int:
    if len(argv) not in {2, 3}:
        print(
            "usage: summarize_pyperformance_worker_timing.py <work-root> [pass-label]",
            file=sys.stderr,
        )
        return 2

    work_root = Path(argv[1])
    pass_label = argv[2] if len(argv) == 3 and argv[2] else "run"
    records = list(_timing_records(work_root))
    if not records:
        print(f"pyperformance worker timing ({pass_label}): no measured workers recorded")
        return 0

    setup_values = _int_values(records, "setup_wall_ns")
    measured_values = _int_values(records, "measured_wall_ns")
    total_values = _int_values(records, "worker_total_wall_ns")
    batch_count = sum(
        value
        for record in records
        if isinstance((value := record.get("measured_batches")), int)
    )

    print(
        f"pyperformance worker timing ({pass_label}): "
        f"{len(records)} worker(s), {batch_count} measured batch(es)"
    )
    if setup_values:
        print(_summary_line("setup before measured values", setup_values))
    if measured_values:
        print(_summary_line("measured-value collection", measured_values))
    if total_values:
        print(_summary_line("worker lifetime", total_values))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
