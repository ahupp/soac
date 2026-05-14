import importlib.util
from pathlib import Path


def load_bench_module(module_name):
    path = Path(__file__).resolve().parents[1] / "bench" / f"{module_name}.py"
    spec = importlib.util.spec_from_file_location(module_name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_dispatcher_preserves_compile_only_forwarding():
    module = load_bench_module("nqueens_slices")

    assert module.parse_args(
        [
            "nqueens_slices.py",
            "diagonal_set_consumers",
            "8",
            "1",
            "--compile-only",
        ]
    ) == (
        "nqueens_slice_diagonal_set_consumers",
        ["8", "1", "--compile-only"],
    )


def test_compile_only_runner_uses_tiny_seed_workload(capsys):
    module = load_bench_module("nqueens_slice_support")
    calls = []

    def record_call(queen_count):
        calls.append(queen_count)
        return 7

    assert (
        module.run_slice(
            "diagonal_set_consumers",
            record_call,
            lambda _queen_count: None,
            [
                "nqueens_slice_diagonal_set_consumers.py",
                "8",
                "1",
                "--compile-only",
            ]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert calls == [1]
    assert "compile_only = true" in output
    assert "compile_queen_count = 1" in output
    assert "compile_result = 7" in output
