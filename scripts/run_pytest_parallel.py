#!/usr/bin/env python3
from __future__ import annotations

import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(os.environ["REPO_ROOT"])
VENV_PYTHON = Path(os.environ["VENV_DIR"]) / "bin" / "python"


@dataclass
class RunResult:
    selector: str
    returncode: int
    elapsed_s: float
    output: str


def parse_jobs(raw: str, max_jobs: int) -> int:
    if raw == "auto":
        jobs = os.cpu_count() or 1
    else:
        jobs = int(raw)
    if jobs <= 0:
        return 0
    return min(jobs, max_jobs)


def is_simple_selector(arg: str) -> bool:
    return not arg.startswith("-") and "::" not in arg


def collect_test_files(args: list[str]) -> tuple[int, list[str], str]:
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

    files: list[str] = []
    seen: set[str] = set()
    for raw_line in proc.stdout.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("="):
            continue
        file_path = line.split("::", 1)[0]
        if file_path in seen:
            continue
        if not (REPO_ROOT / file_path).exists():
            continue
        seen.add(file_path)
        files.append(file_path)
    return 0, files, output


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

    collect_code, files, collect_output = collect_test_files([f"--tb={tb}", *argv])
    if collect_code != 0:
        sys.stdout.write(collect_output)
        return collect_code

    jobs = parse_jobs(jobs_env, max(1, len(files)))
    if jobs <= 1 or len(files) <= 1:
        return subprocess.run(
            pytest_cmd([f"--tb={tb}", *argv]),
            cwd=REPO_ROOT,
        ).returncode

    print(
        f"[diet-python pytest] running {len(files)} test files across {jobs} workers"
    )

    results: list[RunResult] = []
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = {
            pool.submit(run_pytest, [f"--tb={tb}", file_path], file_path): file_path
            for file_path in files
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
        f"[diet-python pytest] file summary: {passed} passed, {len(failed)} failed"
    )
    if failed:
        print("[diet-python pytest] failed files:")
        for result in failed:
            print(f"  - {result.selector}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
