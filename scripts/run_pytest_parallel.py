#!/usr/bin/env python3
from __future__ import annotations

import os
import shlex
import signal
import subprocess
import sys
import time
from datetime import UTC, datetime
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from dataclasses import dataclass
from math import ceil
from pathlib import Path
from threading import Lock


REPO_ROOT = Path(os.environ["REPO_ROOT"])
VENV_PYTHON = Path(os.environ["VENV_DIR"]) / "bin" / "python"
LOGS_DIR = REPO_ROOT / "work" / "logs"
MIN_BATCH_NODEIDS = 16


@dataclass
class RunResult:
    selector: str
    returncode: int
    elapsed_s: float
    output: str
    timed_out: bool = False


@dataclass
class PytestBatch:
    label: str
    selectors: list[str]


@dataclass
class ActiveBatch:
    batch_id: int
    label: str
    selector_count: int
    pid: int
    start_s: float


class BatchMonitor:
    def __init__(self) -> None:
        self._active: dict[int, ActiveBatch] = {}
        self._lock = Lock()

    def start(self, batch: ActiveBatch) -> None:
        with self._lock:
            self._active[batch.batch_id] = batch

    def finish(self, batch_id: int) -> None:
        with self._lock:
            self._active.pop(batch_id, None)

    def snapshot(self) -> list[ActiveBatch]:
        with self._lock:
            return sorted(self._active.values(), key=lambda batch: batch.start_s)


def parse_jobs(raw: str, max_jobs: int) -> int:
    if raw == "auto":
        jobs = os.cpu_count() or 1
    else:
        jobs = int(raw)
    if jobs <= 0:
        return 0
    return min(jobs, max_jobs)


def parse_nonnegative_float_env(name: str, default: float) -> float:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    try:
        value = float(raw)
    except ValueError:
        print(
            f"[diet-python pytest] invalid {name}={raw!r}; expected seconds",
            file=sys.stderr,
        )
        return default
    if value < 0:
        print(
            f"[diet-python pytest] invalid {name}={raw!r}; expected nonnegative seconds",
            file=sys.stderr,
        )
        return default
    return value


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


def terminate_process_group(proc: subprocess.Popen[str]) -> None:
    if proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    except PermissionError:
        proc.terminate()
    try:
        proc.wait(timeout=5)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        return
    except PermissionError:
        proc.kill()


def process_exists(pid: int) -> bool:
    return (Path("/proc") / str(pid)).exists()


def proc_children_by_parent() -> dict[int, list[int]]:
    children: dict[int, list[int]] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        stat_path = entry / "stat"
        try:
            stat = stat_path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        # /proc/<pid>/stat uses "pid (comm) state ppid ..."; comm may contain spaces.
        try:
            after_comm = stat.rsplit(")", 1)[1].strip()
            parts = after_comm.split()
            ppid = int(parts[1])
            pid = int(entry.name)
        except (IndexError, ValueError):
            continue
        children.setdefault(ppid, []).append(pid)
    return children


def collect_descendant_pids(root_pid: int) -> list[int]:
    children = proc_children_by_parent()
    pids: list[int] = []
    stack = [root_pid]
    seen: set[int] = set()
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        pids.append(pid)
        stack.extend(reversed(children.get(pid, [])))
    return pids


def proc_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None


def proc_cmdline(pid: int) -> str:
    path = Path("/proc") / str(pid) / "cmdline"
    try:
        raw = path.read_bytes()
    except OSError:
        return "<unavailable>"
    text = raw.replace(b"\0", b" ").decode("utf-8", errors="replace").strip()
    return text or "<empty>"


def proc_cwd(pid: int) -> str:
    path = Path("/proc") / str(pid) / "cwd"
    try:
        return str(path.resolve(strict=True))
    except OSError:
        return "<unavailable>"


def run_gdb_stack_capture(pid: int) -> tuple[int, str]:
    gdb = shutil_which("gdb")
    if gdb is None:
        return 127, "gdb not found; install gdb before capturing native stacks\n"

    python_gdb = REPO_ROOT / "vendor" / "cpython" / "python-gdb.py"
    commands = [
        "set pagination off",
        "set confirm off",
    ]
    if python_gdb.exists():
        commands.append(f"source {python_gdb}")
    commands.extend(
        [
            "info threads",
            "thread apply all bt full",
        ]
    )
    if python_gdb.exists():
        commands.append("thread apply all py-bt")

    cmd = [gdb, "-q", "-batch", "-p", str(pid)]
    for command in commands:
        cmd.extend(["-ex", command])
    try:
        proc = subprocess.run(
            cmd,
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            timeout=20,
        )
    except subprocess.TimeoutExpired as exc:
        output = (exc.stdout or "") + (exc.stderr or "")
        output += "\ngdb stack capture timed out after 20s\n"
        return 124, output
    return proc.returncode, proc.stdout + proc.stderr


def shutil_which(command: str) -> str | None:
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        if not directory:
            continue
        candidate = Path(directory) / command
        if os.access(candidate, os.X_OK):
            return str(candidate)
    return None


def capture_timeout_stacks(root_pid: int, label: str, timeout_s: float) -> Path:
    timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    safe_label = "".join(ch if ch.isalnum() else "-" for ch in label)[:80].strip("-")
    out = LOGS_DIR / f"pytest-timeout-stacks-{timestamp}-pid{root_pid}-{safe_label}.log"
    out.parent.mkdir(parents=True, exist_ok=True)

    pids = collect_descendant_pids(root_pid)
    with out.open("w", encoding="utf-8") as file:
        file.write("SOAC pytest timeout stack capture\n")
        file.write(f"timestamp_utc={datetime.now(UTC).isoformat()}\n")
        file.write(f"root_pid={root_pid}\n")
        file.write(f"timeout_s={timeout_s:.1f}\n")
        file.write(f"batch={label}\n")
        file.write(f"pids={' '.join(str(pid) for pid in pids)}\n\n")

        for pid in pids:
            file.write(f"\n===== pid {pid} =====\n")
            file.write(f"cmdline: {proc_cmdline(pid)}\n")
            status = proc_text(Path("/proc") / str(pid) / "status")
            if status is not None:
                file.write(status.splitlines()[0] + "\n")
                for line in status.splitlines()[1:12]:
                    file.write(line + "\n")
            file.write(f"cwd: {proc_cwd(pid)}\n")
            file.write("\n--- gdb: native and Python stacks ---\n")
            if not process_exists(pid):
                file.write("process exited before capture\n")
                continue
            status_code, output = run_gdb_stack_capture(pid)
            file.write(output)
            if output and not output.endswith("\n"):
                file.write("\n")
            if status_code != 0:
                file.write(f"\ngdb capture exited with status {status_code} for pid {pid}\n")
    return out


def run_pytest(
    args: list[str],
    batch: PytestBatch,
    batch_id: int,
    timeout_s: float,
    monitor: BatchMonitor,
) -> RunResult:
    cmd = pytest_cmd(args)
    start = time.monotonic()
    proc = subprocess.Popen(
        cmd,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    monitor.start(
        ActiveBatch(
            batch_id=batch_id,
            label=batch.label,
            selector_count=len(batch.selectors),
            pid=proc.pid,
            start_s=start,
        )
    )
    timed_out = False
    try:
        stdout, stderr = proc.communicate(timeout=timeout_s if timeout_s > 0 else None)
    except subprocess.TimeoutExpired:
        timed_out = True
        stack_path: Path | None = None
        stack_error: str | None = None
        try:
            stack_path = capture_timeout_stacks(proc.pid, batch.label, timeout_s)
        except Exception as exc:  # noqa: BLE001 - timeout diagnostics must be best-effort.
            stack_error = f"{type(exc).__name__}: {exc}"
        terminate_process_group(proc)
        stdout, stderr = proc.communicate()
    finally:
        elapsed_s = time.monotonic() - start
        monitor.finish(batch_id)
    output = stdout + stderr
    if timed_out:
        if stack_path is not None:
            output += f"\n[diet-python pytest] captured timeout stacks: {stack_path}\n"
        if stack_error is not None:
            output += (
                "\n[diet-python pytest] failed to capture timeout stacks: "
                f"{stack_error}\n"
            )
        output += (
            "\n[diet-python pytest] batch timed out after "
            f"{timeout_s:.1f}s: {shlex.join(cmd)}\n"
        )
    elapsed_s = time.monotonic() - start
    return RunResult(
        selector=batch.label,
        returncode=124 if timed_out else int(proc.returncode or 0),
        elapsed_s=elapsed_s,
        output=output,
        timed_out=timed_out,
    )


def print_failure(result: RunResult) -> None:
    print(f"\n=== FAIL {result.selector} ({result.elapsed_s:.2f}s) ===")
    sys.stdout.write(result.output)
    if result.output and not result.output.endswith("\n"):
        print()
    print(f"=== END FAIL {result.selector} ===")


def print_running_batches(active: list[ActiveBatch], now_s: float) -> None:
    if not active:
        print("[diet-python pytest] no active pytest batches; waiting for workers")
        return
    print(f"[diet-python pytest] still running {len(active)} batch(es):")
    for batch in active:
        elapsed_s = now_s - batch.start_s
        print(
            "  - "
            f"pid={batch.pid} elapsed={elapsed_s:.1f}s "
            f"nodeids={batch.selector_count} {batch.label}"
        )


def main(argv: list[str]) -> int:
    if not argv:
        cmd = [str(VENV_PYTHON), "-m", "pytest", "--help"]
        return subprocess.run(cmd, cwd=REPO_ROOT).returncode

    tb = os.environ.get("PYTEST_TB", "native")
    jobs_env = os.environ.get("PYTEST_NUMPROCS", "auto")
    batch_timeout_s = parse_nonnegative_float_env("SOAC_PYTEST_BATCH_TIMEOUT", 300.0)
    progress_interval_s = parse_nonnegative_float_env(
        "SOAC_PYTEST_PROGRESS_INTERVAL", 10.0
    )

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
    if jobs <= 0:
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
    if batch_timeout_s > 0:
        print(f"[diet-python pytest] per-batch timeout: {batch_timeout_s:.1f}s")
    else:
        print("[diet-python pytest] per-batch timeout: disabled")
    if progress_interval_s > 0:
        print(
            f"[diet-python pytest] live batch report interval: {progress_interval_s:.1f}s"
        )
    else:
        print("[diet-python pytest] live batch reports: disabled")

    results: list[RunResult] = []
    monitor = BatchMonitor()
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures: dict[Future[RunResult], PytestBatch] = {
            pool.submit(
                run_pytest,
                [f"--tb={tb}", *batch.selectors],
                batch,
                batch_id,
                batch_timeout_s,
                monitor,
            ): batch
            for batch_id, batch in enumerate(batches)
        }
        pending = set(futures)
        next_progress_s = time.monotonic() + progress_interval_s
        while pending:
            wait_timeout = None
            if progress_interval_s > 0:
                wait_timeout = max(0.0, next_progress_s - time.monotonic())
            done, pending = wait(
                pending,
                timeout=wait_timeout,
                return_when=FIRST_COMPLETED,
            )
            for future in done:
                result = future.result()
                results.append(result)
                status = (
                    "TIMEOUT"
                    if result.timed_out
                    else "PASS"
                    if result.returncode == 0
                    else "FAIL"
                )
                print(f"[{status}] {result.selector} ({result.elapsed_s:.2f}s)")
                if result.returncode != 0:
                    print_failure(result)
            if progress_interval_s <= 0 or not pending:
                continue
            now_s = time.monotonic()
            if now_s >= next_progress_s:
                print_running_batches(monitor.snapshot(), now_s)
                while next_progress_s <= now_s:
                    next_progress_s += progress_interval_s

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
