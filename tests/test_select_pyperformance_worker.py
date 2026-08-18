import importlib.util
import json
from pathlib import Path

import pytest


def load_select_pyperformance_worker():
    path = (
        Path(__file__).resolve().parents[1]
        / "scripts"
        / "select_pyperformance_worker.py"
    )
    spec = importlib.util.spec_from_file_location("select_pyperformance_worker", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def write_manifest(path: Path, records):
    path.write_text("".join(f"{json.dumps(record)}\n" for record in records))


def worker_record(tmp_path: Path, name: str, *, stable_args, opt_mode="profile"):
    work_dir = tmp_path / name
    work_dir.mkdir()
    (work_dir / "profile.bin").write_bytes(b"profile")
    return {
        "benchmark_name": "bm_nqueens",
        "benchmark_script": "/tmp/run_benchmark.py",
        "opt_mode": opt_mode,
        "python_executable": "/tmp/python",
        "stable_args": stable_args,
        "work_dir": str(work_dir),
    }


@pytest.mark.parametrize("calibration_flag", ["calibrate_loops", "--calibrate-loops"])
def test_select_worker_prefers_measured_profile_record(
    monkeypatch, tmp_path, calibration_flag
):
    module = load_select_pyperformance_worker()
    measured = worker_record(tmp_path, "measured", stable_args=["--worker-task=0"])
    calibration = worker_record(
        tmp_path,
        "calibration",
        stable_args=[calibration_flag, "--worker-task=0"],
    )
    apply = worker_record(
        tmp_path,
        "apply",
        stable_args=["--worker-task=0"],
        opt_mode="apply",
    )
    manifest = tmp_path / "worker_manifest.jsonl"
    write_manifest(manifest, [calibration, apply, measured])

    assert module.select_worker(manifest, "nqueens") == measured
    assert module.select_worker(manifest, "nqueens", worker="apply") == apply


def test_select_worker_rejects_ambiguous_measured_workers(tmp_path):
    module = load_select_pyperformance_worker()
    first = worker_record(tmp_path, "first", stable_args=["--worker-task=0"])
    second = worker_record(tmp_path, "second", stable_args=["--worker-task=1"])
    manifest = tmp_path / "worker_manifest.jsonl"
    write_manifest(manifest, [first, second])

    with pytest.raises(ValueError, match="multiple measured workers"):
        module.select_worker(manifest, "nqueens")

    assert module.select_worker(manifest, "nqueens", worker="second") == second
