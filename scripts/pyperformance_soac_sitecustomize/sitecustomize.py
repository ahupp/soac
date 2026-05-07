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
    "--track-memory",
    "--values",
    "--warmups",
}

_PYPERF_STABLE_FLAGS_WITH_VALUE = {
    "--worker-task",
}

_PYPERF_FLAGS_WITH_OPTIONAL_VALUE = set()

_PYPERF_VALUE_PREFIXES = tuple(
    f"{flag}=" for flag in (_PYPERF_FLAGS_WITH_VALUE | _PYPERF_FLAGS_WITH_OPTIONAL_VALUE)
)

_PYPERF_FLAGS = {
    "--debug-single-value",
    "--worker",
}


def _enabled(name: str) -> bool:
    return os.environ.get(name, "").lower() in {"1", "true", "yes", "on"}


def _is_benchmark_worker() -> bool:
    if "PYPERFORMANCE_RUNID" in os.environ:
        return True
    argv0 = sys.argv[0].replace(os.sep, "/")
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
        elif any(arg.startswith(f"{flag}=") for flag in _PYPERF_STABLE_FLAGS_WITH_VALUE):
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
        char if char.isalnum() or char in {"-", "_", "."} else "_"
        for char in value
    )
    return safe.strip("._-") or "default"


def _benchmark_work_dir() -> str | None:
    work_root = os.environ.get("SOAC_WORK_DIR")
    if not work_root:
        return None

    benchmark_path = Path(sys.argv[0]).resolve()
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
    benchmark_path = Path(sys.argv[0]).resolve()
    return {
        "benchmark_name": benchmark_path.parent.name or "unknown",
        "benchmark_script": str(benchmark_path),
        "opt_mode": os.environ.get("SOAC_OPT_MODE", "none"),
        "python_executable": sys.executable,
        "stable_args": _stable_benchmark_args(sys.argv[1:]),
        "work_dir": work_dir,
    }


def _append_benchmark_manifest(work_root: str, work_dir: str) -> None:
    manifest_path = Path(work_root) / "worker_manifest.jsonl"
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(
        _benchmark_manifest_record(work_dir),
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8") + b"\n"
    fd = os.open(manifest_path, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o644)
    try:
        os.write(fd, payload)
    finally:
        os.close(fd)


def _pause_before_measured_values(ready_file: str) -> None:
    ready_path = Path(ready_file)
    ready_path.parent.mkdir(parents=True, exist_ok=True)
    ready_path.write_text("ready\n")
    os.kill(os.getpid(), signal.SIGSTOP)


def _install_measured_value_pause_hook(worker_task_cls=None) -> None:
    ready_file = os.environ.get("SOAC_PYPERFORMANCE_MEASURE_READY_FILE")
    if not ready_file:
        return

    if worker_task_cls is None:
        from pyperf._worker import WorkerTask

        worker_task_cls = WorkerTask

    original_compute_values = worker_task_cls._compute_values
    paused = False

    def compute_values_with_pause(
        self,
        values,
        nvalue,
        is_warmup=False,
        calibrate_loops=False,
        start=0,
    ):
        nonlocal paused
        if not paused and not is_warmup and not calibrate_loops:
            paused = True
            _pause_before_measured_values(ready_file)
        return original_compute_values(
            self,
            values,
            nvalue,
            is_warmup=is_warmup,
            calibrate_loops=calibrate_loops,
            start=start,
        )

    worker_task_cls._compute_values = compute_values_with_pause


_install_measured_value_pause_hook()


if (
    _enabled("SOAC_PYPERFORMANCE_ENABLE")
    and not _enabled("SOAC_PYPERFORMANCE_DRIVER")
    and not _enabled("SOAC_PYPERFORMANCE_EXEC_WRAPPED")
    and _is_benchmark_worker()
):
    os.environ["SOAC_PYPERFORMANCE_EXEC_WRAPPED"] = "1"
    work_root = os.environ.get("SOAC_WORK_DIR")
    if benchmark_work_dir := _benchmark_work_dir():
        os.makedirs(benchmark_work_dir, exist_ok=True)
        if work_root:
            _append_benchmark_manifest(work_root, benchmark_work_dir)
        os.environ["SOAC_WORK_DIR"] = benchmark_work_dir
    os.execv(
        sys.executable,
        [
            sys.executable,
            "-m",
            "soac.import_hook",
            sys.argv[0],
            *sys.argv[1:],
        ],
    )
