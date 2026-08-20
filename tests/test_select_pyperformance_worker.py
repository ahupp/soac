import importlib.util
import json
import subprocess
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
    stock = work_dir / "stock"
    stock.mkdir()
    script = stock / "run_benchmark.py"
    script.write_text(
        'import pyperf\ndef workload():\n    return 42\nif __name__ == "__main__":\n    runner = pyperf.Runner()\n    runner.bench_func("sample", workload)\n'
    )

    def checker(command, **kwargs):
        # A publication-shaped unit fixture only; never passed to native
        # startup or treated as genuine runtime authority by these tests.
        Path(command[command.index("--deployment") + 1]).write_text(
            '{"fixture": true}\n'
        )
        return subprocess.CompletedProcess(
            command, 0, '{"modules": 1, "generation": "fixture"}', ""
        )

    execution = (
        load_select_pyperformance_worker()
        ._source_tools()
        .prepare_strict_benchmark(
            script,
            Path("/tmp/python"),
            work_dir / "bundle",
            Path("/tmp/checker"),
            {},
            run=checker,
        )
    )
    source = execution["source"]
    return {
        "benchmark_name": "bm_nqueens",
        "benchmark_script": str(script),
        "opt_mode": opt_mode,
        "python_executable": "/tmp/python",
        "stable_args": stable_args,
        "work_dir": str(work_dir),
        "language": "strict",
        "strict_bundle": execution["manifest_path"],
        "strict_deployment": execution["deployment"],
        "strict_script": source["strict_script"],
        "strict_harness": source["harness_script"],
        "strict_project": source["project"],
        "strict_modules": source["modules"],
        "strict_source_fingerprint": source["source_fingerprint"],
        "stock_source_fingerprint": source["stock_source_fingerprint"],
        "selection_policy": source["selection_policy"],
        "harness_policy": source["harness_projection"]["policy"],
        "artifact_generation": execution["publication"]["generation"],
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


@pytest.mark.parametrize("changed", ["ordinary", "authority", "source", "harness"])
def test_replay_rejects_changed_or_ordinary_worker_provenance(tmp_path, changed):
    module = load_select_pyperformance_worker()
    record = worker_record(tmp_path, "selected", stable_args=["--worker-task=0"])
    if changed == "ordinary":
        record["language"] = "ordinary"
    elif changed == "authority":
        record["strict_deployment"] = "/other/deployment.json"
    elif changed == "source":
        Path(record["benchmark_script"]).write_text("changed = True\n")
    else:
        Path(record["strict_harness"]).write_text("changed = True\n")
    manifest = tmp_path / "worker_manifest.jsonl"
    write_manifest(manifest, [record])
    with pytest.raises(ValueError):
        module.select_worker(manifest, "nqueens")
