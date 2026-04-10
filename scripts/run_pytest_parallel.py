#!/usr/bin/env python3
from __future__ import annotations

import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from math import ceil
from pathlib import Path


REPO_ROOT = Path(os.environ["REPO_ROOT"])
VENV_PYTHON = Path(os.environ["VENV_DIR"]) / "bin" / "python"
MIN_BATCH_NODEIDS = 16


@dataclass
class RunResult:
    selector: str
    returncode: int
    elapsed_s: float
    output: str


@dataclass
class PytestBatch:
    label: str
    selectors: list[str]


def parse_jobs(raw: str, max_jobs: int) -> int:
    if raw == "auto":
        jobs = os.cpu_count() or 1
    else:
        jobs = int(raw)
    if jobs <= 0:
        return 0
    return min(jobs, max_jobs)


def is_simple_selector(arg: str) -> bool:
    return not arg.startswith("-")


def collect_test_nodeids(args: list[str]) -> tuple[int, list[str], str]:
    cmd = [
        str(VENV_PYTHON),
        "-m",
        "pytest",
        "--collect-only",
        "-q",
        *args,
    ]
    proc = subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    output = proc.stdout + proc.stderr
    if proc.returncode != 0:
        return proc.returncode, [], output

    nodeids: list[str] = []
    seen: set[str] = set()
    for raw_line in proc.stdout.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("="):
            continue
        file_path = line.split("::", 1)[0]
        if not (REPO_ROOT / file_path).exists():
            continue
        if "::" not in line or line in seen:
            continue
        seen.add(line)
        nodeids.append(line)
    return 0, nodeids, output


def node_group_key(nodeid: str) -> str:
    # Batch parametrized cases from the same test function together. This keeps
    # integration-module cases from becoming one subprocess per fixture.
    last_sep = nodeid.rfind("::")
    last_param = nodeid.rfind("[")
    if last_param > last_sep:
        return nodeid[:last_param]
    return nodeid


def node_file_path(nodeid: str) -> str:
    return nodeid.split("::", 1)[0]


def make_file_batches(
    file_path: str, nodeids: list[str], target_batch_size: int
) -> list[PytestBatch]:
    grouped_nodeids: dict[str, list[str]] = {}
    for nodeid in nodeids:
        grouped_nodeids.setdefault(node_group_key(nodeid), []).append(nodeid)

    units: list[PytestBatch] = []
    for group_key, group in grouped_nodeids.items():
        if len(group) <= target_batch_size:
            units.append(PytestBatch(group_key, group))
            continue

        chunk_count = ceil(len(group) / target_batch_size)
        for chunk_index in range(chunk_count):
            start = chunk_index * target_batch_size
            chunk = group[start : start + target_batch_size]
            units.append(
                PytestBatch(
                    f"{group_key} chunk {chunk_index + 1}/{chunk_count}",
                    chunk,
                )
            )

    batches: list[PytestBatch] = []
    current_units: list[PytestBatch] = []
    current_selectors: list[str] = []

    def flush_current_batch() -> None:
        if not current_selectors:
            return
        if len(current_units) == 1:
            label = current_units[0].label
        else:
            label = (
                f"{file_path} batch {len(batches) + 1} "
                f"({len(current_selectors)} nodeids)"
            )
        batches.append(PytestBatch(label, list(current_selectors)))
        current_units.clear()
        current_selectors.clear()

    for unit in units:
        if (
            current_selectors
            and len(current_selectors) + len(unit.selectors) > target_batch_size
        ):
            flush_current_batch()
        current_units.append(unit)
        current_selectors.extend(unit.selectors)

    flush_current_batch()
    return batches


def make_nodeid_batches(nodeids: list[str], jobs: int) -> list[PytestBatch]:
    target_batch_size = max(MIN_BATCH_NODEIDS, ceil(len(nodeids) / jobs))
    file_nodeids: dict[str, list[str]] = {}
    for nodeid in nodeids:
        file_nodeids.setdefault(node_file_path(nodeid), []).append(nodeid)

    # Keep batches file-local so import-hook and sys.modules state cannot leak
    # across unrelated test files.
    batches: list[PytestBatch] = []
    for file_path, file_group in file_nodeids.items():
        batches.extend(make_file_batches(file_path, file_group, target_batch_size))
    return sorted(batches, key=lambda batch: len(batch.selectors), reverse=True)


def pytest_cmd(args: list[str]) -> list[str]:
    return [
        str(VENV_PYTHON),
        "-m",
        "pytest",
        "-vv",
        "--durations=0",
        *args,
    ]


def run_pytest(args: list[str], selector: str) -> RunResult:
    cmd = pytest_cmd(args)
    start = time.monotonic()
    proc = subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    elapsed_s = time.monotonic() - start
    return RunResult(
        selector=selector,
        returncode=proc.returncode,
        elapsed_s=elapsed_s,
        output=proc.stdout + proc.stderr,
    )


def print_failure(result: RunResult) -> None:
    print(f"\n=== FAIL {result.selector} ({result.elapsed_s:.2f}s) ===")
    sys.stdout.write(result.output)
    if result.output and not result.output.endswith("\n"):
        print()
    print(f"=== END FAIL {result.selector} ===")


def main(argv: list[str]) -> int:
    if not argv:
        cmd = [str(VENV_PYTHON), "-m", "pytest", "--help"]
        return subprocess.run(cmd, cwd=REPO_ROOT).returncode

    tb = os.environ.get("PYTEST_TB", "native")
    jobs_env = os.environ.get("PYTEST_NUMPROCS", "auto")

    if any(not is_simple_selector(arg) for arg in argv):
        return subprocess.run(
            pytest_cmd([f"--tb={tb}", *argv]),
            cwd=REPO_ROOT,
        ).returncode

    collect_code, nodeids, collect_output = collect_test_nodeids([f"--tb={tb}", *argv])
    if collect_code != 0:
        sys.stdout.write(collect_output)
        return collect_code
    if not nodeids:
        return subprocess.run(
            pytest_cmd([f"--tb={tb}", *argv]),
            cwd=REPO_ROOT,
        ).returncode

    jobs = parse_jobs(jobs_env, max(1, len(nodeids)))
    if jobs <= 1 or len(nodeids) <= 1:
        return subprocess.run(
            pytest_cmd([f"--tb={tb}", *argv]),
            cwd=REPO_ROOT,
        ).returncode

    batches = make_nodeid_batches(nodeids, jobs)
    jobs = min(jobs, len(batches))
    print(
        f"[diet-python pytest] running {len(nodeids)} test nodeids "
        f"in {len(batches)} batches across {jobs} workers"
    )

    results: list[RunResult] = []
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = {
            pool.submit(
                run_pytest,
                [f"--tb={tb}", *batch.selectors],
                batch.label,
            ): batch
            for batch in batches
        }
        for future in as_completed(futures):
            result = future.result()
            results.append(result)
            status = "PASS" if result.returncode == 0 else "FAIL"
            print(f"[{status}] {result.selector} ({result.elapsed_s:.2f}s)")
            if result.returncode != 0:
                print_failure(result)

    failed = [result for result in results if result.returncode != 0]
    passed = len(results) - len(failed)
    print(
        f"[diet-python pytest] batch summary: {passed} passed, {len(failed)} failed"
    )
    if failed:
        print("[diet-python pytest] failed batches:")
        for result in failed:
            print(f"  - {result.selector}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
