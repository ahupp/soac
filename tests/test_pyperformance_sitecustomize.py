import importlib.util
from pathlib import Path


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
