"""Run pyperformance benchmark scripts through SOAC's import-hook entrypoint.

This directory is prepended to PYTHONPATH only by the `just pyperformance soac`
recipe. Pyperformance also uses the inherited environment while creating and
repairing its benchmark virtual environments, so the hook only activates for
actual benchmark worker processes.

Each benchmark worker receives its own SOAC work subdirectory. Pyperformance
executes every benchmark script as ``__main__``, and some pyperformance
entries share one benchmark script with different positional arguments, so
sharing one counter dump across the full suite would mix different source
hashes and class/type observations under one module name.
"""

import json
import os
import signal
import sys
import atexit
import importlib.util
import time
from functools import lru_cache
from hashlib import sha256
from pathlib import Path


_PYPERF_FLAGS_WITH_VALUE = {
    "--affinity",
    "--inherit-environ",
    "--loops",
    "--metadata",
    "--min-time",
    "--output",
    "--pipe",
    "--timeout",
    "--track-memory",
    "--values",
    "--warmups",
}

_PYPERF_STABLE_FLAGS_WITH_VALUE = {
    "--worker-task",
}

_PYPERF_FLAGS_WITH_OPTIONAL_VALUE = set()

_PYPERF_VALUE_PREFIXES = tuple(
    f"{flag}="
    for flag in (_PYPERF_FLAGS_WITH_VALUE | _PYPERF_FLAGS_WITH_OPTIONAL_VALUE)
)

_PYPERF_FLAGS = {
    "--debug-single-value",
    "--worker",
}

_WORKER_START_ENV = "SOAC_PYPERFORMANCE_WORKER_START_NS"
_WORKER_TIMING_FILENAME = "pyperformance-worker-timing.jsonl"


def _enabled(name: str) -> bool:
    return os.environ.get(name, "").lower() in {"1", "true", "yes", "on"}


def _is_benchmark_worker() -> bool:
    argv0 = sys.argv[0].replace(os.sep, "/")
    if argv0.endswith("/pyperf/_process_time.py"):
        return False
    if "PYPERFORMANCE_RUNID" in os.environ:
        return True
    return (
        argv0.endswith("/run_benchmark.py")
        and "/pyperformance/data-files/benchmarks/" in argv0
    )


def _stable_benchmark_args(argv: list[str]) -> list[str]:
    stable_args = []
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in _PYPERF_STABLE_FLAGS_WITH_VALUE:
            if index + 1 < len(argv):
                stable_args.append(f"{arg}={argv[index + 1]}")
                index += 2
            else:
                stable_args.append(arg)
                index += 1
        elif any(
            arg.startswith(f"{flag}=") for flag in _PYPERF_STABLE_FLAGS_WITH_VALUE
        ):
            stable_args.append(arg)
            index += 1
        elif arg in _PYPERF_FLAGS_WITH_VALUE:
            index += 2
        elif arg in _PYPERF_FLAGS_WITH_OPTIONAL_VALUE:
            index += 1
            if index < len(argv) and not argv[index].startswith("-"):
                index += 1
        elif arg.startswith(_PYPERF_VALUE_PREFIXES):
            index += 1
        elif arg in _PYPERF_FLAGS:
            index += 1
        else:
            stable_args.append(arg)
            index += 1
    return stable_args


def _safe_path_component(value: str) -> str:
    safe = "".join(
        char if char.isalnum() or char in {"-", "_", "."} else "_" for char in value
    )
    return safe.strip("._-") or "default"


@lru_cache(maxsize=1)
def _strict_source_tools():
    path = Path(__file__).resolve().parent.parent / "strict_pyperformance_sources.py"
    spec = importlib.util.spec_from_file_location("_soac_worker_sources", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@lru_cache(maxsize=1)
def _verified_execution(path: str, python: str):
    return _strict_source_tools().verify_strict_benchmark(Path(path), Path(python))


def _strict_execution():
    path = os.environ.get("SOAC_PYPERFORMANCE_STRICT_BUNDLE")
    return _verified_execution(path, sys.executable) if path else None


def _benchmark_source_path() -> Path:
    execution = _strict_execution()
    if execution is not None:
        return Path(execution["source"]["stock_script"])
    return Path(sys.argv[0]).resolve()


def _benchmark_work_dir() -> str | None:
    work_root = os.environ.get("SOAC_PYPERFORMANCE_WORK_ROOT") or os.environ.get(
        "SOAC_WORK_DIR"
    )
    if not work_root:
        return None

    benchmark_path = _benchmark_source_path()
    benchmark_name = benchmark_path.parent.name or "unknown"
    variant_args = _stable_benchmark_args(sys.argv[1:])
    variant_slug = _safe_path_component(
        "-".join(arg.lstrip("-") or "dash" for arg in variant_args)
    )[:64]
    key = "\0".join([str(benchmark_path), *variant_args])
    path_hash = sha256(key.encode("utf-8")).hexdigest()[:12]
    return str(
        Path(work_root)
        / "benchmarks"
        / f"{_safe_path_component(benchmark_name)}-{variant_slug}-{path_hash}"
    )


def _benchmark_manifest_record(work_dir: str) -> dict[str, object]:
    benchmark_path = _benchmark_source_path()
    record = {
        "benchmark_name": benchmark_path.parent.name or "unknown",
        "benchmark_script": str(benchmark_path),
        "opt_mode": os.environ.get("SOAC_OPT_MODE", "none"),
        "python_executable": sys.executable,
        "stable_args": _stable_benchmark_args(sys.argv[1:]),
        "work_dir": work_dir,
    }
    if (execution := _strict_execution()) is not None:
        record.update(
            {
                "language": "strict",
                "strict_bundle": execution["manifest_path"],
                "strict_deployment": execution["deployment"],
                "strict_script": execution["source"]["strict_script"],
                "strict_harness": execution["source"]["harness_script"],
                "harness_policy": execution["source"]["harness_projection"]["policy"],
                "strict_source_fingerprint": execution["source_fingerprint"],
                "stock_source_fingerprint": execution["source"][
                    "stock_source_fingerprint"
                ],
                "selection_policy": execution["selection_policy"],
                "strict_modules": execution["source"]["modules"],
                "strict_project": execution["source"]["project"],
                "artifact_generation": execution["publication"]["generation"],
            }
        )
    return record


def _strict_worker_command(execution):
    script = Path(sys.argv[0]).resolve()
    source = execution["source"]
    if str(script) not in {source["stock_script"], source["strict_script"]}:
        raise ValueError("strict bundle does not select the actual benchmark script")
    # Retain the interpreter flags selected by pyperformance. Add startup
    # authority before -m, never after importing the extension.
    try:
        script_index = sys.orig_argv.index(sys.argv[0], 1)
    except ValueError as error:
        raise ValueError("cannot preserve benchmark interpreter arguments") from error
    options = sys.orig_argv[1:script_index]
    configured = sys._xoptions.get("soac_strict_config")
    if configured is not None and configured != execution["deployment"]:
        raise ValueError("benchmark already has a different strict startup authority")
    retained = []
    index = 0
    while index < len(options):
        if (
            options[index] == "-X"
            and index + 1 < len(options)
            and options[index + 1].startswith("soac_strict_config=")
        ):
            index += 2
        elif options[index].startswith("-Xsoac_strict_config="):
            index += 1
        else:
            retained.append(options[index])
            index += 1
    return [
        sys.executable,
        *retained,
        "-X",
        f"soac_strict_config={execution['deployment']}",
        "-m",
        "soac_strict_worker",
        source["strict_script"],
        *sys.argv[1:],
    ]


def _append_jsonl(path: Path, record: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = (
        json.dumps(
            record,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
        + b"\n"
    )
    fd = os.open(path, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o644)
    try:
        os.write(fd, payload)
    finally:
        os.close(fd)


def _append_benchmark_manifest(work_root: str, work_dir: str) -> None:
    _append_jsonl(
        Path(work_root) / "worker_manifest.jsonl",
        _benchmark_manifest_record(work_dir),
    )


def _worker_timing_enabled() -> bool:
    return _enabled("SOAC_PYPERFORMANCE_ENABLE") and _is_benchmark_worker()


def _ensure_worker_timing_start() -> None:
    if _worker_timing_enabled():
        os.environ.setdefault(_WORKER_START_ENV, str(time.perf_counter_ns()))


def _worker_timing_path() -> Path | None:
    if not _worker_timing_enabled():
        return None
    work_dir = os.environ.get("SOAC_WORK_DIR")
    if not work_dir:
        return None
    return Path(work_dir) / _WORKER_TIMING_FILENAME


def _pause_before_measured_values(ready_file: str) -> None:
    ready_path = Path(ready_file)
    ready_path.parent.mkdir(parents=True, exist_ok=True)
    ready_path.write_text("ready\n")
    os.kill(os.getpid(), signal.SIGSTOP)


def _strict_seal_evidence():
    if not _enabled("SOAC_PYPERFORMANCE_ENABLE"):
        return None
    execution = _strict_execution()
    if execution is None:
        raise RuntimeError("cannot measure an ordinary worker as strict SOAC")
    from soac_strict_worker import sealed_module_evidence

    return sealed_module_evidence(execution)


def _install_measured_value_pause_hook(worker_task_cls=None) -> None:
    ready_file = os.environ.get("SOAC_PYPERFORMANCE_MEASURE_READY_FILE")
    timing_path = _worker_timing_path()
    if not ready_file and timing_path is None:
        return

    if worker_task_cls is None:
        from pyperf._worker import WorkerTask

        worker_task_cls = WorkerTask

    original_compute_values = worker_task_cls._compute_values
    paused = False
    first_measured_start_ns: int | None = None
    last_measured_end_ns: int | None = None
    pyperf_benchmark_name: str | None = None
    measured_batches = 0
    measured_wall_ns = 0
    strict_seals = None

    def compute_values_with_pause(
        self,
        values,
        nvalue,
        is_warmup=False,
        calibrate_loops=False,
        start=0,
    ):
        nonlocal paused
        nonlocal first_measured_start_ns
        nonlocal last_measured_end_ns
        nonlocal pyperf_benchmark_name
        nonlocal measured_batches
        nonlocal measured_wall_ns
        nonlocal strict_seals

        measured = not is_warmup and not calibrate_loops
        if measured:
            # Validate before the timer/pause. A completed import or a source
            # cache file is not evidence that this actual producer is sealed.
            strict_seals = _strict_seal_evidence()
        if measured and pyperf_benchmark_name is None:
            pyperf_benchmark_name = getattr(self, "name", None)
        if not paused and measured and ready_file:
            paused = True
            _pause_before_measured_values(ready_file)
        started_ns = time.perf_counter_ns() if measured else None
        if measured and first_measured_start_ns is None:
            first_measured_start_ns = started_ns
        try:
            return original_compute_values(
                self,
                values,
                nvalue,
                is_warmup=is_warmup,
                calibrate_loops=calibrate_loops,
                start=start,
            )
        finally:
            if measured and started_ns is not None:
                ended_ns = time.perf_counter_ns()
                last_measured_end_ns = ended_ns
                measured_batches += 1
                measured_wall_ns += ended_ns - started_ns

    def flush_worker_timing() -> None:
        if (
            timing_path is None
            or first_measured_start_ns is None
            or last_measured_end_ns is None
            or measured_batches == 0
        ):
            return

        try:
            worker_start_ns = int(
                os.environ.get(_WORKER_START_ENV, str(first_measured_start_ns))
            )
        except ValueError:
            worker_start_ns = first_measured_start_ns

        work_dir = os.environ.get("SOAC_WORK_DIR", "")
        record = _benchmark_manifest_record(work_dir)
        record.update(
            {
                "record_type": "pyperformance_worker_timing_v1",
                "pyperf_benchmark_name": pyperf_benchmark_name,
                "pid": os.getpid(),
                "setup_wall_ns": max(0, first_measured_start_ns - worker_start_ns),
                "measured_batches": measured_batches,
                "measured_wall_ns": measured_wall_ns,
                "measured_span_wall_ns": max(
                    0,
                    last_measured_end_ns - first_measured_start_ns,
                ),
                "worker_total_wall_ns": max(
                    0,
                    time.perf_counter_ns() - worker_start_ns,
                ),
                "sealed_strict_modules": strict_seals,
            }
        )
        _append_jsonl(timing_path, record)

    worker_task_cls._compute_values = compute_values_with_pause
    if timing_path is not None:
        atexit.register(flush_worker_timing)


def _activate_strict_worker():
    execution = _strict_execution()
    if execution is None:
        raise RuntimeError(
            "strict benchmark worker has no offline-authenticated source bundle"
        )
    if _enabled("SOAC_PYPERFORMANCE_EXEC_WRAPPED"):
        if sys._xoptions.get("soac_strict_config") != execution["deployment"]:
            raise RuntimeError(
                "wrapped strict benchmark has no matching native startup authority"
            )
        return
    command = _strict_worker_command(execution)
    os.environ["SOAC_PYPERFORMANCE_EXEC_WRAPPED"] = "1"
    work_root = os.environ.get("SOAC_PYPERFORMANCE_WORK_ROOT") or os.environ.get(
        "SOAC_WORK_DIR"
    )
    if benchmark_work_dir := _benchmark_work_dir():
        os.makedirs(benchmark_work_dir, exist_ok=True)
        if work_root:
            _append_benchmark_manifest(work_root, benchmark_work_dir)
        os.environ["SOAC_WORK_DIR"] = benchmark_work_dir
    project = Path(execution["source"]["project"])
    os.environ["SOAC_MODULE_ENABLED"] = ",".join(
        f"path:{project / relative}"
        for relative in execution["source"]["modules"].values()
    )
    os.execv(sys.executable, command)


if (
    _enabled("SOAC_PYPERFORMANCE_ENABLE")
    and not _enabled("SOAC_PYPERFORMANCE_DRIVER")
    and _is_benchmark_worker()
):
    try:
        _ensure_worker_timing_start()
        _install_measured_value_pause_hook()
        _activate_strict_worker()
    except Exception as error:
        # CPython normally prints and ignores errors from sitecustomize. That
        # would run the original ordinary workload and mislabel it as strict.
        # A benchmark startup failure must instead prevent every user opcode.
        print(
            f"strict pyperformance startup failed: {error}", file=sys.stderr, flush=True
        )
        os._exit(78)
