from __future__ import annotations

import os
import shlex
import signal
import subprocess
import sys
import time
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from dataclasses import dataclass
from datetime import UTC, datetime
from math import ceil
from pathlib import Path
from threading import Lock

REPO_ROOT = Path(os.environ["REPO_ROOT"])
VENV_PYTHON = Path(os.environ["VENV_DIR"]) / "bin" / "python"
LOGS_DIR = REPO_ROOT / "work" / "logs"
MAX_BATCH_NODEIDS = 4
SCENARIO_NODE_GROUP = "tests/test_strict_scenarios.py::test_strict_scenario"
# These reviewed compatibility tests run several subprocess phases or validate
# multiple authenticated modules per node. Isolate their batch deadlines
# from neighboring tests. Source scenarios additionally compose the existing
# per-block deadline across their independent runtime invocations below.
SINGLETON_NODE_GROUPS = frozenset(
    {
        "tests/test_counter_dump_file.py::"
        "test_profiled_full_nqueens_slice_preserves_results_mutations_and_ordinary_tracing",
        "tests/test_counter_dump_file.py::"
        "test_profiled_pyperformance_nqueens_preserves_rebinding_and_ordinary_tracing",
        "tests/test_closed_iterator_pipeline.py::"
        "test_reviewed_closed_pipelines_use_authenticated_entries",
        SCENARIO_NODE_GROUP,
    }
)


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
    timeout_s: float


class BatchMonitor:
    def __init__(self) -> None:
        self._active: dict[int, ActiveBatch] = {}
        self._processes: dict[int, subprocess.Popen[str]] = {}
        self._lock = Lock()
        self._process_lock = Lock()
        self.cancelled = False
        self.interrupted_by: int | None = None

    def request_cancel(self, signum: int | None = None) -> None:
        # The signal handler must not wait for a worker's in-flight Popen.
        self.cancelled = True
        if signum is not None and self.interrupted_by is None:
            self.interrupted_by = signum

    def launch(
        self, cmd: list[str], batch: PytestBatch, batch_id: int, start_s: float,
        timeout_s: float,
    ) -> subprocess.Popen[str] | None:
        # Serialize the cancellation boundary with process publication. A launch
        # already in progress must publish its group before stop() can finish.
        with self._process_lock:
            if self.cancelled:
                return None
            proc = subprocess.Popen(
                cmd,
                cwd=REPO_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                start_new_session=True,
            )
            self._processes[batch_id] = proc
            with self._lock:
                self._active[batch_id] = ActiveBatch(
                    batch_id, batch.label, len(batch.selectors), proc.pid, start_s,
                    timeout_s,
                )
            return proc

    def finish(self, batch_id: int) -> None:
        with self._process_lock:
            proc = self._processes.pop(batch_id, None)
        try:
            if proc is not None:
                terminate_process_group(proc)
        finally:
            with self._lock:
                self._active.pop(batch_id, None)

    def stop(self) -> None:
        self.request_cancel()
        with self._process_lock:
            processes = list(self._processes.values())
            self._processes.clear()
        terminate_process_groups(processes)

    def snapshot(self) -> list[ActiveBatch]:
        # Progress reporting must not block on a worker launching a process.
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
        check=False,
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
        if node_group_key(unit.selectors[0]) in SINGLETON_NODE_GROUPS:
            flush_current_batch()
            batches.extend(PytestBatch(nodeid, [nodeid]) for nodeid in unit.selectors)
            continue
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
    # A suite-wide proportional batch grows without bound as tests are added.
    # Keep the timeout's unit bounded independently of the collected suite;
    # small selections may still use smaller batches to occupy the workers.
    target_batch_size = min(MAX_BATCH_NODEIDS, max(1, ceil(len(nodeids) / jobs)))
    file_nodeids: dict[str, list[str]] = {}
    for nodeid in nodeids:
        file_nodeids.setdefault(node_file_path(nodeid), []).append(nodeid)

    # Keep batches file-local so import-hook and sys.modules state cannot leak
    # across unrelated test files.
    batches: list[PytestBatch] = []
    for file_path, file_group in file_nodeids.items():
        batches.extend(make_file_batches(file_path, file_group, target_batch_size))
    return sorted(batches, key=lambda batch: len(batch.selectors), reverse=True)


def scenario_extra_timeouts(nodeids: list[str]) -> dict[str, float]:
    """Compose existing runtime limits only for exact parsed enrollments.

    The parser and strict helper have no native/checker work at import time.
    Import them only when the actual collection includes source scenarios;
    ordinary runner invocations retain their existing dependencies and budgets.
    """
    selected = {
        node for node in nodeids if node_group_key(node) == SCENARIO_NODE_GROUP
    }
    if not selected:
        return {}
    source_root = Path(__file__).resolve().parents[1]
    previous_path = sys.path[:]
    try:
        sys.path.insert(0, str(source_root))
        from tests import _strict_scenarios as scenarios
    finally:
        sys.path[:] = previous_path
    expected_parser = source_root / "tests" / "_strict_scenarios.py"
    if Path(scenarios.__file__).resolve() != expected_parser:
        raise ValueError("scenario timeout parser was imported from another checkout")
    root = (REPO_ROOT / "tests/strict_scenarios").resolve()
    enrolled = {
        (
            f"{SCENARIO_NODE_GROUP}["
            f"{scenarios.scenario_pytest_id(scenario, mode, root)}]"
        ): (len(scenario.blocks) - 1) * scenarios.STRICT_RUNTIME_TIMEOUT
        for scenario in scenarios.discover_strict_scenarios(root)
        for mode in scenario.modes
    }
    unknown = selected - enrolled.keys()
    if unknown:
        raise ValueError(f"scenario timeout enrollment is missing: {sorted(unknown)}")
    return {node: enrolled[node] for node in selected}


def pytest_cmd(args: list[str]) -> list[str]:
    return [
        str(VENV_PYTHON),
        "-m",
        "pytest",
        "-vv",
        "--durations=0",
        *args,
    ]


def signal_process_group(proc: subprocess.Popen[str], signum: int) -> None:
    try:
        os.killpg(proc.pid, signum)
    except ProcessLookupError:
        pass
    except PermissionError:
        proc.send_signal(signum)


def process_group_exists(proc: subprocess.Popen[str]) -> bool:
    try:
        os.killpg(proc.pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        pass
    return True


def terminate_process_groups(processes: list[subprocess.Popen[str]]) -> None:
    # A leader may already have exited while descendants still own the pipes.
    # Signal every group before waiting, so cancellation has one grace period,
    # not one grace period per worker.
    for proc in processes:
        signal_process_group(proc, signal.SIGTERM)
    remaining = processes
    deadline = time.monotonic() + 5
    while remaining:
        for proc in remaining:
            proc.poll()
        remaining = [proc for proc in remaining if process_group_exists(proc)]
        delay = deadline - time.monotonic()
        if not remaining or delay <= 0:
            break
        time.sleep(min(0.05, delay))
    for proc in remaining:
        signal_process_group(proc, signal.SIGKILL)
    for proc in processes:
        proc.wait()


def terminate_process_group(proc: subprocess.Popen[str]) -> None:
    terminate_process_groups([proc])


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

    python_gdb = (
        REPO_ROOT
        / os.environ.get("CPYTHON_SOURCE_DIR", "vendor/cpython")
        / "Tools" / "gdb" / "libpython.py"
    )
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
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        # TimeoutExpired retains partial bytes even when text=True. A timeout
        # can also split an encoded character, so diagnostics must be tolerant.
        output = "".join(
            part.decode("utf-8", errors="replace") if isinstance(part, bytes) else part or ""
            for part in (exc.stdout, exc.stderr)
        )
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
) -> RunResult | None:
    cmd = pytest_cmd(args)
    start = time.monotonic()
    proc = monitor.launch(cmd, batch, batch_id, start, timeout_s)
    if proc is None:
        return None
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
        monitor.finish(batch_id)
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
        deadline = f"{batch.timeout_s:.1f}s" if batch.timeout_s > 0 else "disabled"
        print(
            "  - "
            f"pid={batch.pid} elapsed={elapsed_s:.1f}s "
            f"nodeids={batch.selector_count} "
            f"deadline={deadline} {batch.label}"
        )


def main(argv: list[str]) -> int:
    require_batches = "--require-batch-runner" in argv
    argv = [arg for arg in argv if arg != "--require-batch-runner"]
    jobs_env = os.environ.get("PYTEST_NUMPROCS", "auto")
    if require_batches and (
        not argv
        or any(not is_simple_selector(arg) for arg in argv)
        or parse_jobs(jobs_env, 1) == 0
    ):
        print(
            "[diet-python pytest] --require-batch-runner requires the batch runner: "
            "pass file/node selectors without pytest options and enable PYTEST_NUMPROCS",
            file=sys.stderr,
        )
        return 2
    if not argv:
        cmd = [str(VENV_PYTHON), "-m", "pytest", "--help"]
        return subprocess.run(cmd, cwd=REPO_ROOT, check=False).returncode

    tb = os.environ.get("PYTEST_TB", "native")
    batch_timeout_s = parse_nonnegative_float_env("SOAC_PYTEST_BATCH_TIMEOUT", 300.0)
    progress_interval_s = parse_nonnegative_float_env(
        "SOAC_PYTEST_PROGRESS_INTERVAL", 10.0
    )

    if any(not is_simple_selector(arg) for arg in argv):
        return subprocess.run(
            pytest_cmd([f"--tb={tb}", *argv]),
            cwd=REPO_ROOT,
            check=False,
        ).returncode

    collect_code, nodeids, collect_output = collect_test_nodeids([f"--tb={tb}", *argv])
    if collect_code != 0:
        sys.stdout.write(collect_output)
        return collect_code
    if not nodeids:
        if require_batches:
            sys.stdout.write(collect_output)
            print(
                "[diet-python pytest] --require-batch-runner requires the batch runner, "
                "but collection produced no test nodeids",
                file=sys.stderr,
            )
            return 5
        return subprocess.run(
            pytest_cmd([f"--tb={tb}", *argv]),
            cwd=REPO_ROOT,
            check=False,
        ).returncode

    jobs = parse_jobs(jobs_env, max(1, len(nodeids)))
    if jobs <= 0:
        return subprocess.run(
            pytest_cmd([f"--tb={tb}", *argv]),
            cwd=REPO_ROOT,
            check=False,
        ).returncode

    batches = make_nodeid_batches(nodeids, jobs)
    try:
        extra_timeouts = scenario_extra_timeouts(nodeids)
    except (OSError, SyntaxError, ValueError) as error:
        print(f"[diet-python pytest] {error}", file=sys.stderr)
        return 2
    jobs = min(jobs, len(batches))
    print(
        f"[diet-python pytest] running {len(nodeids)} test nodeids "
        f"in {len(batches)} batches across {jobs} workers"
    )
    if batch_timeout_s > 0:
        print(f"[diet-python pytest] base per-batch timeout: {batch_timeout_s:.1f}s")
        if any(extra_timeouts.values()):
            print("[diet-python pytest] source scenarios use parsed aggregate deadlines")
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
    pool: ThreadPoolExecutor | None = None
    futures: dict[Future[RunResult | None], PytestBatch] = {}
    reported: set[Future[RunResult | None]] = set()
    completed = False

    def request_interruption(signum: int, _frame: object) -> None:
        monitor.request_cancel(signum)

    def report_result(future: Future[RunResult | None], *, cancelled: bool = False) -> None:
        reported.add(future)
        result = future.result()
        if result is None:
            return
        results.append(result)
        status = (
            "CANCELLED"
            if cancelled
            else "TIMEOUT"
            if result.timed_out
            else "PASS"
            if result.returncode == 0
            else "FAIL"
        )
        print(f"[{status}] {result.selector} ({result.elapsed_s:.2f}s)")
        if cancelled:
            sys.stdout.write(result.output)
            if result.output and not result.output.endswith("\n"):
                print()
        elif result.returncode != 0:
            print_failure(result)

    previous_handlers = {}
    try:
        for signum in (signal.SIGINT, signal.SIGTERM):
            previous_handlers[signum] = signal.signal(signum, request_interruption)
        pool = ThreadPoolExecutor(max_workers=jobs)
        for batch_id, batch in enumerate(batches):
            if monitor.cancelled:
                break
            effective_timeout_s = batch_timeout_s
            if effective_timeout_s > 0:
                effective_timeout_s += sum(
                    extra_timeouts.get(node, 0) for node in batch.selectors
                )
            future = pool.submit(
                run_pytest,
                [f"--tb={tb}", *batch.selectors],
                batch,
                batch_id,
                effective_timeout_s,
                monitor,
            )
            futures[future] = batch
        pending = set(futures)
        next_progress_s = time.monotonic() + progress_interval_s
        while pending and not monitor.cancelled:
            # Signals only latch cancellation; keep this wait bounded even when
            # progress output is disabled so main can perform orderly shutdown.
            wait_timeout = 0.1
            if progress_interval_s > 0:
                wait_timeout = min(
                    wait_timeout, max(0.0, next_progress_s - time.monotonic())
                )
            done, pending = wait(
                pending,
                timeout=wait_timeout,
                return_when=FIRST_COMPLETED,
            )
            if monitor.cancelled:
                break
            for future in done:
                report_result(future)
            if progress_interval_s <= 0 or not pending:
                continue
            now_s = time.monotonic()
            if now_s >= next_progress_s:
                print_running_batches(monitor.snapshot(), now_s)
                while next_progress_s <= now_s:
                    next_progress_s += progress_interval_s
        completed = not monitor.cancelled
    finally:
        cancelled = not completed or monitor.cancelled
        # Close the launch gate before cancelling queued futures: a thread may
        # already have taken a future out of the executor's queue.
        monitor.request_cancel()
        try:
            if cancelled:
                try:
                    print("[diet-python pytest] stopping worker groups", flush=True)
                except (OSError, ValueError):
                    # A broken output pipe must not bypass process cleanup.
                    pass
            try:
                if pool is not None:
                    pool.shutdown(wait=False, cancel_futures=True)
            finally:
                try:
                    monitor.stop()
                finally:
                    if pool is not None:
                        pool.shutdown(wait=True, cancel_futures=True)
            for future in futures:
                if future in reported or future.cancelled():
                    continue
                try:
                    report_result(future, cancelled=cancelled)
                except Exception as exc:
                    if not cancelled:
                        raise
                    # Keep the original main-thread exception or signal status,
                    # while retaining any additional worker diagnostic.
                    try:
                        print(
                            f"[diet-python pytest] worker error: {type(exc).__name__}: {exc}",
                            file=sys.stderr,
                        )
                    except (OSError, ValueError):
                        pass
        finally:
            for signum, handler in previous_handlers.items():
                signal.signal(signum, handler)

    if monitor.interrupted_by is not None:
        return 128 + monitor.interrupted_by

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
