import importlib.util
import json
import os
import signal
from types import SimpleNamespace
from pathlib import Path


def test_pinned_nqueens_source_matches_installed_pyperformance_benchmark():
    spec = importlib.util.find_spec("benchmarks.bm_nqueens.run_benchmark")
    assert spec is not None and spec.origin is not None
    installed = Path(spec.origin)
    pinned = (
        Path(__file__).resolve().parents[1]
        / "crates"
        / "soac_jit"
        / "src"
        / "jit"
        / "fixtures"
        / "opaque_fused_pyperformance_nqueens_v1.py"
    )

    assert installed.read_bytes() == pinned.read_bytes()


def load_pyperformance_sitecustomize(monkeypatch):
    monkeypatch.delenv("SOAC_PYPERFORMANCE_ENABLE", raising=False)
    path = (
        Path(__file__).resolve().parents[1]
        / "scripts"
        / "pyperformance_soac_sitecustomize"
        / "sitecustomize.py"
    )
    spec = importlib.util.spec_from_file_location("_soac_pyperformance_sitecustomize", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_pyperformance_work_dir_includes_benchmark_variant(monkeypatch, tmp_path):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    script = tmp_path / "pyperformance" / "data-files" / "benchmarks" / "bm_async_tree" / "run_benchmark.py"
    script.parent.mkdir(parents=True)
    script.write_text("# benchmark placeholder\n")
    monkeypatch.setenv("SOAC_WORK_DIR", str(tmp_path / "work"))

    monkeypatch.setattr(
        sitecustomize.sys,
        "argv",
        [
            str(script),
            "none",
            "--task-groups",
            "--debug-single-value",
            "--inherit-environ",
            "SOAC_WORK_DIR,SOAC_OPT_MODE",
            "--output",
            "/tmp/profile-output",
        ],
    )
    none_tg_dir = Path(sitecustomize._benchmark_work_dir()).name

    monkeypatch.setattr(
        sitecustomize.sys,
        "argv",
        [
            str(script),
            "cpu_io_mixed",
            "--debug-single-value",
            "--inherit-environ",
            "SOAC_WORK_DIR,SOAC_OPT_MODE",
            "--output",
            "/tmp/apply-output",
        ],
    )
    cpu_io_dir = Path(sitecustomize._benchmark_work_dir()).name

    assert none_tg_dir != cpu_io_dir
    assert "bm_async_tree-none-task-groups" in none_tg_dir
    assert "bm_async_tree-cpu_io_mixed" in cpu_io_dir


def test_pyperformance_work_dir_keeps_worker_task(monkeypatch):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)

    assert sitecustomize._stable_benchmark_args(
        [
            "none",
            "--task-groups",
            "--worker",
            "--pipe",
            "4",
            "--worker-task=0",
            "--values",
            "1",
            "--min-time",
            "1e-09",
            "--loops",
            "1",
            "--warmups",
            "0",
        ]
    ) == ["none", "--task-groups", "--worker-task=0"]


def test_pyperformance_work_dir_keeps_separate_worker_task_values(monkeypatch, tmp_path):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    script = (
        tmp_path
        / "pyperformance"
        / "data-files"
        / "benchmarks"
        / "bm_deepcopy"
        / "run_benchmark.py"
    )
    script.parent.mkdir(parents=True)
    script.write_text("# benchmark placeholder\n")
    monkeypatch.setenv("SOAC_WORK_DIR", str(tmp_path / "work"))

    monkeypatch.setattr(sitecustomize.sys, "argv", [str(script), "--worker-task=0"])
    first_task_dir = Path(sitecustomize._benchmark_work_dir()).name

    monkeypatch.setattr(sitecustomize.sys, "argv", [str(script), "--worker-task=1"])
    second_task_dir = Path(sitecustomize._benchmark_work_dir()).name

    assert first_task_dir != second_task_dir
    assert "bm_deepcopy-worker-task_0" in first_task_dir
    assert "bm_deepcopy-worker-task_1" in second_task_dir


def test_pyperformance_worker_manifest_uses_root_work_dir(monkeypatch, tmp_path):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    script = (
        tmp_path
        / "pyperformance"
        / "data-files"
        / "benchmarks"
        / "bm_nqueens"
        / "run_benchmark.py"
    )
    script.parent.mkdir(parents=True)
    script.write_text("# benchmark placeholder\n")
    work_root = tmp_path / "work"
    work_dir = work_root / "benchmarks" / "bm_nqueens-worker-task_0-example"
    monkeypatch.setenv("SOAC_OPT_MODE", "profile")
    monkeypatch.setattr(
        sitecustomize.sys,
        "argv",
        [
            str(script),
            "--worker",
            "--worker-task=0",
            "--pipe",
            "4",
            "--loops",
            "10",
        ],
    )
    monkeypatch.setattr(sitecustomize.sys, "executable", "/tmp/worker-python")

    sitecustomize._append_benchmark_manifest(str(work_root), str(work_dir))

    rows = [
        json.loads(line)
        for line in (work_root / "worker_manifest.jsonl").read_text().splitlines()
    ]
    assert rows == [
        {
            "benchmark_name": "bm_nqueens",
            "benchmark_script": str(script.resolve()),
            "opt_mode": "profile",
            "python_executable": "/tmp/worker-python",
            "stable_args": ["--worker-task=0"],
            "work_dir": str(work_dir),
        }
    ]


def test_pyperformance_default_dependency_packages_extend_allow_list(
    monkeypatch,
    tmp_path,
):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    benchmark_root = tmp_path / "pyperformance" / "data-files" / "benchmarks"
    script = benchmark_root / "bm_tomli_loads" / "run_benchmark.py"
    script.parent.mkdir(parents=True)
    script.write_text("# benchmark placeholder\n")
    package_root = tmp_path / "venv" / "site-packages" / "tomli"
    package_root.mkdir(parents=True)
    monkeypatch.setenv("SOAC_MODULE_ENABLED", f"path:{benchmark_root}")
    monkeypatch.setattr(sitecustomize.sys, "argv", [str(script)])
    monkeypatch.setattr(
        sitecustomize.importlib.util,
        "find_spec",
        lambda package_name: SimpleNamespace(
            submodule_search_locations=[str(package_root)],
            origin=str(package_root / "__init__.py"),
        )
        if package_name == "tomli"
        else None,
    )

    sitecustomize._enable_default_dependency_packages()

    assert os.environ["SOAC_MODULE_ENABLED"] == (
        f"path:{benchmark_root},path:{package_root.resolve()}"
    )


def test_pyperformance_default_dependency_packages_can_be_skipped(
    monkeypatch,
    tmp_path,
):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    benchmark_root = tmp_path / "pyperformance" / "data-files" / "benchmarks"
    script = benchmark_root / "bm_tomli_loads" / "run_benchmark.py"
    script.parent.mkdir(parents=True)
    script.write_text("# benchmark placeholder\n")
    explicit_root = tmp_path / "explicit"
    monkeypatch.setenv("SOAC_MODULE_ENABLED", f"path:{explicit_root}")
    monkeypatch.setattr(sitecustomize.sys, "argv", [str(script)])

    if sitecustomize._using_default_module_allowlist():
        sitecustomize._enable_default_dependency_packages()

    assert os.environ["SOAC_MODULE_ENABLED"] == f"path:{explicit_root}"


def test_pyperformance_measured_value_hook_pauses_once_before_values(
    monkeypatch,
    tmp_path,
):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    script = (
        tmp_path
        / "pyperformance"
        / "data-files"
        / "benchmarks"
        / "bm_nqueens"
        / "run_benchmark.py"
    )
    script.parent.mkdir(parents=True)
    script.write_text("# benchmark placeholder\n")
    ready_file = tmp_path / "ready"
    calls = []

    class FakeWorkerTask:
        def _compute_values(
            self,
            values,
            nvalue,
            is_warmup=False,
            calibrate_loops=False,
            start=0,
        ):
            calls.append(("compute", is_warmup, calibrate_loops, start))

    monkeypatch.setattr(sitecustomize.sys, "argv", [str(script), "--worker"])
    monkeypatch.setenv("SOAC_PYPERFORMANCE_MEASURE_READY_FILE", str(ready_file))
    monkeypatch.setattr(
        sitecustomize.os,
        "kill",
        lambda pid, signum: calls.append(("kill", pid, signum)),
    )

    sitecustomize._install_measured_value_pause_hook(FakeWorkerTask)
    task = FakeWorkerTask()
    task._compute_values([], 1, is_warmup=True)
    task._compute_values([], 1)
    task._compute_values([], 1)

    assert ready_file.read_text() == "ready\n"
    assert calls == [
        ("compute", True, False, 0),
        ("kill", os.getpid(), signal.SIGSTOP),
        ("compute", False, False, 0),
        ("compute", False, False, 0),
    ]


def test_pyperformance_worker_timing_records_exact_pyperf_benchmark_name(
    monkeypatch,
    tmp_path,
):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    script = (
        tmp_path
        / "pyperformance"
        / "data-files"
        / "benchmarks"
        / "bm_async_tree"
        / "run_benchmark.py"
    )
    script.parent.mkdir(parents=True)
    script.write_text("# benchmark placeholder\n")
    work_dir = tmp_path / "work"
    flush_callbacks = []

    class FakeWorkerTask:
        name = "async_tree_cpu_io_mixed_tg"

        def _compute_values(
            self,
            values,
            nvalue,
            is_warmup=False,
            calibrate_loops=False,
            start=0,
        ):
            return values

    monkeypatch.setattr(sitecustomize.sys, "argv", [str(script), "--worker"])
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "1")
    monkeypatch.setenv("SOAC_OPT_MODE", "apply")
    monkeypatch.setenv("SOAC_WORK_DIR", str(work_dir))
    monkeypatch.setenv(sitecustomize._WORKER_START_ENV, "60")
    monkeypatch.setattr(sitecustomize.atexit, "register", flush_callbacks.append)

    sitecustomize._install_measured_value_pause_hook(FakeWorkerTask)
    FakeWorkerTask()._compute_values([], 1)

    assert len(flush_callbacks) == 1
    flush_callbacks[0]()
    timing_path = work_dir / "pyperformance-worker-timing.jsonl"
    records = [json.loads(line) for line in timing_path.read_text().splitlines()]

    assert len(records) == 1
    assert records[0]["record_type"] == "pyperformance_worker_timing_v1"
    assert records[0]["benchmark_name"] == "bm_async_tree"
    assert records[0]["pyperf_benchmark_name"] == "async_tree_cpu_io_mixed_tg"
    assert records[0]["opt_mode"] == "apply"
