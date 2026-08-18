#!/usr/bin/env python3
"""Select one replayable pyperformance worker from a SOAC manifest."""

from __future__ import annotations

import argparse
import json
import shlex
import sys
from pathlib import Path
from typing import Any


def _load_records(path: Path) -> list[dict[str, Any]]:
    records = []
    for lineno, line in enumerate(path.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as exc:
            raise ValueError(f"{path}:{lineno}: invalid JSON: {exc}") from exc
        if not isinstance(record, dict):
            raise ValueError(f"{path}:{lineno}: expected JSON object")
        records.append(record)
    return records


def _is_calibration_record(record: dict[str, Any]) -> bool:
    stable_args = record.get("stable_args", [])
    return any(
        isinstance(arg, str)
        and arg.lstrip("-").replace("-", "_").startswith("calibrate_")
        for arg in stable_args
    )


def _measurement_records(
    records: list[dict[str, Any]],
    benchmark: str,
    worker: str | None,
) -> list[dict[str, Any]]:
    benchmark_name = benchmark if benchmark.startswith("bm_") else f"bm_{benchmark}"
    matching = [
        record
        for record in records
        if record.get("benchmark_name") == benchmark_name
        and not _is_calibration_record(record)
    ]

    if worker is not None:
        matching = [
            record
            for record in matching
            if Path(str(record.get("work_dir", ""))).name == worker
        ]
    else:
        profile_records = [
            record for record in matching if record.get("opt_mode") == "profile"
        ]
        if profile_records:
            matching = profile_records

    deduped_by_work_dir: dict[str, dict[str, Any]] = {}
    for record in matching:
        work_dir = record.get("work_dir")
        if isinstance(work_dir, str):
            deduped_by_work_dir[work_dir] = record
    matching = list(deduped_by_work_dir.values())

    return matching


def _validate_record(record: dict[str, Any]) -> None:
    for key in ("benchmark_name", "benchmark_script", "python_executable", "work_dir"):
        if not isinstance(record.get(key), str):
            raise ValueError(f"selected worker is missing string field {key!r}")
    stable_args = record.get("stable_args")
    if not isinstance(stable_args, list) or not all(
        isinstance(arg, str) for arg in stable_args
    ):
        raise ValueError("selected worker is missing string-list field 'stable_args'")


def select_worker(
    manifest_path: Path,
    benchmark: str,
    worker: str | None = None,
) -> dict[str, Any]:
    records = _load_records(manifest_path)
    matching = _measurement_records(records, benchmark, worker)
    if not matching:
        worker_hint = f" and worker {worker!r}" if worker is not None else ""
        raise ValueError(f"no measured worker found for benchmark {benchmark!r}{worker_hint}")
    if len(matching) > 1:
        candidates = ", ".join(
            Path(str(record["work_dir"])).name for record in sorted(
                matching,
                key=lambda item: str(item["work_dir"]),
            )
        )
        raise ValueError(
            f"benchmark {benchmark!r} has multiple measured workers: {candidates}; "
            "pass --worker <worker-dir-name>"
        )
    record = matching[0]
    _validate_record(record)
    profile_path = Path(str(record["work_dir"])) / "profile.bin"
    if not profile_path.is_file():
        raise ValueError(f"selected worker has no profile at {profile_path}")
    return record


def _print_bash(record: dict[str, Any]) -> None:
    stable_args = record["stable_args"]
    print(f"WORKER_BENCHMARK_NAME={shlex.quote(record['benchmark_name'])}")
    print(f"WORKER_BENCHMARK_SCRIPT={shlex.quote(record['benchmark_script'])}")
    print(f"WORKER_PYTHON={shlex.quote(record['python_executable'])}")
    print(f"WORKER_WORK_DIR={shlex.quote(record['work_dir'])}")
    print("WORKER_STABLE_ARGS=(")
    for arg in stable_args:
        print(f"  {shlex.quote(arg)}")
    print(")")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("benchmark")
    parser.add_argument("--worker")
    parser.add_argument("--format", choices=("bash", "json"), default="json")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        record = select_worker(args.manifest, args.benchmark, args.worker)
    except (OSError, ValueError) as exc:
        print(exc, file=sys.stderr)
        return 2
    if args.format == "bash":
        _print_bash(record)
    else:
        print(json.dumps(record, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
