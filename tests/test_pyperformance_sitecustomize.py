import importlib.util
import json
import os
import signal
import subprocess
import sys
from types import SimpleNamespace
from pathlib import Path

import pytest


def load_pyperformance_sitecustomize(monkeypatch):
    monkeypatch.delenv("SOAC_PYPERFORMANCE_ENABLE", raising=False)
    monkeypatch.delenv("SOAC_PYPERFORMANCE_STRICT_BUNDLE", raising=False)
    monkeypatch.delenv("SOAC_PYPERFORMANCE_WORK_ROOT", raising=False)
    path = (
        Path(__file__).resolve().parents[1]
        / "scripts"
        / "pyperformance_soac_sitecustomize"
        / "sitecustomize.py"
    )
    spec = importlib.util.spec_from_file_location(
        "_soac_pyperformance_sitecustomize", path
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_pyperformance_process_time_helper_is_not_a_benchmark_worker(
    monkeypatch,
    tmp_path,
):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    helper = tmp_path / "site-packages" / "pyperf" / "_process_time.py"
    monkeypatch.setattr(sitecustomize.sys, "argv", [str(helper), "1", "/fake/python"])
    monkeypatch.setenv("PYPERFORMANCE_RUNID", "cpython3.15-bm_2to3")
    monkeypatch.setenv("SOAC_PYPERFORMANCE_ENABLE", "1")
    monkeypatch.setenv("SOAC_WORK_DIR", str(tmp_path / "work"))

    assert sitecustomize._is_benchmark_worker() is False
    assert sitecustomize._worker_timing_path() is None


def test_pyperformance_wrapped_module_worker_keeps_inherited_runid(monkeypatch):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    monkeypatch.setattr(sitecustomize.sys, "argv", ["-m", "soac.import_hook"])
    monkeypatch.setenv("PYPERFORMANCE_RUNID", "cpython3.15-bm_2to3")
    monkeypatch.setenv("SOAC_PYPERFORMANCE_EXEC_WRAPPED", "1")

    assert sitecustomize._is_benchmark_worker() is True


@pytest.mark.parametrize("timeout_args", [["--timeout", "30"], ["--timeout=120"]])
def test_pyperformance_work_dir_ignores_timeout(monkeypatch, tmp_path, timeout_args):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    script = (
        tmp_path
        / "pyperformance"
        / "data-files"
        / "benchmarks"
        / "bm_2to3"
        / "run_benchmark.py"
    )
    script.parent.mkdir(parents=True)
    script.write_text("# benchmark placeholder\n")
    monkeypatch.setenv("SOAC_WORK_DIR", str(tmp_path / "work"))

    monkeypatch.setattr(sitecustomize.sys, "argv", [str(script), "--worker-task=0"])
    expected_work_dir = sitecustomize._benchmark_work_dir()

    monkeypatch.setattr(
        sitecustomize.sys,
        "argv",
        [str(script), "--worker-task=0", *timeout_args],
    )

    assert sitecustomize._benchmark_work_dir() == expected_work_dir


def test_pyperformance_work_dir_includes_benchmark_variant(monkeypatch, tmp_path):
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


def test_pyperformance_work_dir_keeps_separate_worker_task_values(
    monkeypatch, tmp_path
):
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


def test_pyperformance_strict_worker_preserves_flags_and_selects_startup_authority(
    monkeypatch, tmp_path
):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    script = tmp_path / "stock" / "run_benchmark.py"
    script.parent.mkdir(parents=True)
    script.write_text("result = 1\n")
    strict_script = tmp_path / "strict" / "project" / "run_benchmark.py"
    deployment = str(tmp_path / "strict" / "authority" / "deployment.json")
    execution = {
        "deployment": deployment,
        "source": {"stock_script": str(script), "strict_script": str(strict_script)},
    }
    monkeypatch.setattr(
        sitecustomize.sys, "argv", [str(script), "--worker", "--loops=3"]
    )
    monkeypatch.setattr(
        sitecustomize.sys,
        "orig_argv",
        [sys.executable, "-B", "-X", "utf8", str(script), "--worker", "--loops=3"],
    )
    expected = [
        sys.executable,
        "-B",
        "-X",
        "utf8",
        "-X",
        f"soac_strict_config={deployment}",
        "-m",
        "soac_strict_worker",
        str(strict_script),
        "--worker",
        "--loops=3",
    ]
    assert sitecustomize._strict_worker_command(execution) == expected
    monkeypatch.setitem(sitecustomize.sys._xoptions, "soac_strict_config", deployment)
    monkeypatch.setattr(
        sitecustomize.sys,
        "orig_argv",
        [
            sys.executable,
            "-B",
            "-X",
            "utf8",
            "-X",
            f"soac_strict_config={deployment}",
            str(script),
            "--worker",
            "--loops=3",
        ],
    )
    assert sitecustomize._strict_worker_command(execution) == expected
    monkeypatch.setitem(
        sitecustomize.sys._xoptions, "soac_strict_config", "/different/config"
    )
    with pytest.raises(ValueError, match="different strict startup"):
        sitecustomize._strict_worker_command(execution)


def test_pyperformance_strict_worker_directory_stays_stable_after_reexec(
    monkeypatch, tmp_path
):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    stock = tmp_path / "bm_example" / "run_benchmark.py"
    strict = tmp_path / "overlay" / "project" / "run_benchmark.py"
    monkeypatch.setenv("SOAC_PYPERFORMANCE_WORK_ROOT", str(tmp_path / "work"))
    monkeypatch.setenv("SOAC_WORK_DIR", str(tmp_path / "work"))
    monkeypatch.setattr(
        sitecustomize,
        "_strict_execution",
        lambda: {"source": {"stock_script": str(stock)}},
    )
    monkeypatch.setattr(sitecustomize.sys, "argv", [str(stock), "--worker-task=0"])
    directory = sitecustomize._benchmark_work_dir()
    monkeypatch.setenv("SOAC_WORK_DIR", directory)
    monkeypatch.setattr(sitecustomize.sys, "argv", [str(strict), "--worker-task=0"])
    assert sitecustomize._benchmark_work_dir() == directory


def test_missing_strict_worker_authority_is_fatal_before_original_code(tmp_path):
    script = tmp_path / "run_benchmark.py"
    executed = tmp_path / "ordinary-code-ran"
    script.write_text(f"from pathlib import Path\nPath({str(executed)!r}).touch()\n")
    hook = (
        Path(__file__).resolve().parents[1]
        / "scripts"
        / "pyperformance_soac_sitecustomize"
    )
    environment = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("SOAC_PYPERFORMANCE_")
    }
    environment.update(
        SOAC_PYPERFORMANCE_ENABLE="1",
        PYPERFORMANCE_RUNID="strict-worker-negative",
        PYTHONPATH=str(hook),
    )
    result = subprocess.run(
        [sys.executable, str(script)],
        env=environment,
        text=True,
        capture_output=True,
        timeout=20,
    )
    assert result.returncode == 78
    assert "startup failed" in result.stderr
    assert not executed.exists()


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
    monkeypatch.setattr(
        sitecustomize,
        "_strict_seal_evidence",
        lambda: [{"module_name": "__main__", "sealed": True}],
    )

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
    assert records[0]["sealed_strict_modules"] == [
        {"module_name": "__main__", "sealed": True}
    ]


def test_measured_value_hook_rejects_unsealed_producer_before_timing(
    monkeypatch, tmp_path
):
    sitecustomize = load_pyperformance_sitecustomize(monkeypatch)
    calls = []

    class FakeWorkerTask:
        def _compute_values(self, *args, **kwargs):
            calls.append("measured")

    def unsealed():
        raise RuntimeError("producer not sealed")

    monkeypatch.setenv("SOAC_PYPERFORMANCE_MEASURE_READY_FILE", str(tmp_path / "ready"))
    monkeypatch.setattr(sitecustomize, "_strict_seal_evidence", unsealed)
    sitecustomize._install_measured_value_pause_hook(FakeWorkerTask)
    with pytest.raises(RuntimeError, match="producer not sealed"):
        FakeWorkerTask()._compute_values([], 1)
    assert not calls
    assert not (tmp_path / "ready").exists()
