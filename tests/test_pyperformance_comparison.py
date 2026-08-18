import importlib.util
import json
from pathlib import Path

import pyperf
import pytest


def _load_comparison_module():
    script = (
        Path(__file__).resolve().parents[1]
        / "scripts"
        / "summarize_pyperformance_comparison.py"
    )
    spec = importlib.util.spec_from_file_location(
        "summarize_pyperformance_comparison", script
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _write_suite(
    path: Path,
    benchmarks: dict[str, float],
    *,
    metadata: dict[str, object] | None = None,
) -> None:
    suite = pyperf.BenchmarkSuite(
        [
            pyperf.Benchmark(
                [
                    pyperf.Run(
                        [elapsed, elapsed * 1.01],
                        metadata={
                            **(metadata or {}),
                            "name": name,
                            "unit": "second",
                            "loops": 1,
                        },
                        collect_metadata=False,
                    )
                ]
            )
            for name, elapsed in benchmarks.items()
        ]
    )
    suite.dump(str(path))


def _write_round(
    directory: Path,
    index: int,
    *,
    stock: dict[str, float],
    soac: dict[str, float],
    metadata: dict[str, object] | None = None,
) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    _write_suite(directory / f"round-{index:02d}-stock.json", stock, metadata=metadata)
    _write_suite(directory / f"round-{index:02d}-soac.json", soac, metadata=metadata)


def test_comparison_reports_paired_speedups_and_geometric_mean(tmp_path: Path) -> None:
    comparison = _load_comparison_module()
    _write_round(
        tmp_path,
        1,
        stock={"mixed_a": 2.0, "mixed_b": 4.0},
        soac={"mixed_a": 1.0, "mixed_b": 2.0},
    )
    _write_round(
        tmp_path,
        2,
        stock={"mixed_a": 2.2, "mixed_b": 4.4},
        soac={"mixed_a": 1.1, "mixed_b": 2.2},
    )

    summary = comparison.summarize_comparison(tmp_path)

    assert summary["round_count"] == 2
    assert summary["benchmark_count"] == 2
    assert summary["complete"] is True
    assert summary["geometric_mean_speedup_vs_stock"] == pytest.approx(2.0)
    assert summary["benchmarks"]["mixed_a"]["speedup_vs_stock"] == pytest.approx(2.0)
    assert (tmp_path / "stock.json").is_file()
    assert (tmp_path / "soac.json").is_file()
    assert (
        len(
            pyperf.BenchmarkSuite.load(str(tmp_path / "stock.json"))
            .get_benchmark("mixed_a")
            .get_runs()
        )
        == 2
    )


def test_comparison_rejects_missing_benchmark_results(tmp_path: Path) -> None:
    comparison = _load_comparison_module()
    _write_round(
        tmp_path,
        1,
        stock={"mixed_a": 2.0, "mixed_b": 4.0},
        soac={"mixed_a": 1.0},
    )

    with pytest.raises(ValueError, match="missing benchmarks: mixed_b"):
        comparison.summarize_comparison(tmp_path)


def test_comparison_rejects_benchmarks_missing_from_the_fixed_target_set(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    _write_round(tmp_path, 1, stock={"mixed_a": 2.0}, soac={"mixed_a": 1.0})
    (tmp_path / "requested-benchmarks.txt").write_text(
        "Selected benchmarks:\n- mixed_a\n- mixed_b\n", encoding="utf-8"
    )

    with pytest.raises(ValueError, match="missing benchmarks: mixed_b"):
        comparison.summarize_comparison(tmp_path)


def test_comparison_reports_speedup_against_prior_soac(tmp_path: Path) -> None:
    comparison = _load_comparison_module()
    baseline = tmp_path / "baseline"
    candidate = tmp_path / "candidate"
    _write_round(baseline, 1, stock={"mixed": 4.0}, soac={"mixed": 2.0})
    _write_round(candidate, 1, stock={"mixed": 4.0}, soac={"mixed": 1.0})

    summary = comparison.summarize_comparison(candidate, baseline=baseline)

    assert summary["geometric_mean_speedup_vs_baseline_soac"] == pytest.approx(2.0)
    assert summary["benchmarks"]["mixed"]["speedup_vs_baseline_soac"] == pytest.approx(
        2.0
    )


def test_comparison_uses_prior_round_median_instead_of_pooled_mean(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    baseline = tmp_path / "baseline"
    candidate = tmp_path / "candidate"
    for index, elapsed in enumerate((1.0, 2.0, 100.0), start=1):
        _write_round(baseline, index, stock={"mixed": 4.0}, soac={"mixed": elapsed})
    comparison.summarize_comparison(baseline)
    _write_round(candidate, 1, stock={"mixed": 4.0}, soac={"mixed": 1.0})

    summary = comparison.summarize_comparison(candidate, baseline=baseline)

    assert summary["geometric_mean_speedup_vs_baseline_soac"] == pytest.approx(2.0)


def test_comparison_rejects_a_prior_soac_result_from_another_python(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    baseline = tmp_path / "baseline"
    candidate = tmp_path / "candidate"
    _write_round(
        baseline,
        1,
        stock={"mixed": 4.0},
        soac={"mixed": 2.0},
        metadata={"python_version": "3.14.0"},
    )
    _write_round(
        candidate,
        1,
        stock={"mixed": 4.0},
        soac={"mixed": 1.0},
        metadata={"python_version": "3.15.0"},
    )

    with pytest.raises(ValueError, match="incompatible python_version"):
        comparison.summarize_comparison(candidate, baseline=baseline)


def test_comparison_reports_transformed_stdlib_and_native_code(tmp_path: Path) -> None:
    comparison = _load_comparison_module()
    _write_round(tmp_path, 1, stock={"mixed": 2.0}, soac={"mixed": 1.0})
    worker = tmp_path / "round-01-soac.soac-work" / "benchmarks" / "mixed-worker"
    stdlib_module = (
        worker / "modules" / "python-stdlib" / "json" / "decoder" / "mod.blockpy"
    )
    project_module = worker / "modules" / "project" / "benchmark_impl" / "mod.blockpy"
    for module in (stdlib_module, project_module):
        module.parent.mkdir(parents=True, exist_ok=True)
        module.write_bytes(b"cache")
    (worker / "pyperformance-worker-timing.jsonl").write_text(
        json.dumps(
            {
                "pid": 44,
                "opt_mode": "apply",
                "pyperf_benchmark_name": "mixed",
                "record_type": "pyperformance_worker_timing_v1",
            }
        )
        + "\n"
    )
    (worker / "jit-code-summary.jsonl").write_text(
        "\n".join(
            [
                json.dumps(
                    {
                        "process_id": 12,
                        "function_id": "1:8",
                        "function_qualname": "profile_only",
                        "code_size": 999,
                        "machine_block_count": 99,
                    }
                ),
                json.dumps(
                    {
                        "process_id": 44,
                        "function_id": "1:4",
                        "function_qualname": "execute",
                        "code_size": 120,
                        "machine_block_count": 3,
                    }
                ),
            ]
        )
        + "\n"
    )
    (worker / "events.jsonl").write_text(
        "\n".join(
            [
                json.dumps(
                    {
                        "event": "soac.typed_v3_function_rewrite",
                        "function_id": "1:8",
                        "final_block_count": 90,
                    }
                ),
                json.dumps(
                    {
                        "event": "soac.typed_v3_function_rewrite",
                        "function_id": "1:4",
                        "final_block_count": 12,
                    }
                ),
                json.dumps(
                    {
                        "event": "soac.typed_v3_function_rewrite",
                        "function_id": "1:4",
                        "final_block_count": 5,
                    }
                ),
            ]
        )
        + "\n"
    )

    summary = comparison.summarize_comparison(tmp_path)

    assert summary["transformation"]["stdlib_modules"] == ["json.decoder"]
    assert summary["transformation"]["project_modules"] == ["benchmark_impl"]
    assert summary["transformation"]["compiled_functions"] == ["execute"]
    assert summary["transformation"]["benchmark_coverage"] == {
        "mixed": {
            "project_modules": ["benchmark_impl"],
            "stdlib_modules": ["json.decoder"],
            "compiled_functions": ["execute"],
            "worker_count": 1,
        }
    }
    assert summary["transformation"]["preoptimization_blockpy_bytes"] == 10
    assert summary["transformation"]["optimized_typed_ir_block_count"] == 5
    assert summary["transformation"]["optimized_typed_ir_function_count"] == 1
    assert summary["transformation"]["native_code_bytes"] == 120
    assert summary["transformation"]["native_machine_blocks"] == 3


def test_comparison_attributes_benchmark_variants_to_their_own_worker(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    _write_round(
        tmp_path,
        1,
        stock={"pickle": 2.0, "pickle_dict": 4.0},
        soac={"pickle": 1.0, "pickle_dict": 2.0},
    )

    for index, benchmark in enumerate(("pickle", "pickle_dict"), start=1):
        worker = (
            tmp_path
            / "round-01-soac.soac-work"
            / "benchmarks"
            / f"bm_pickle-variant-{index}"
        )
        module_path = worker / "modules" / "project" / "__main__" / "mod.blockpy"
        module_path.parent.mkdir(parents=True, exist_ok=True)
        module_path.write_bytes(b"cache")
        (worker / "pyperformance-worker-timing.jsonl").write_text(
            json.dumps(
                {
                    "pid": index,
                    "opt_mode": "apply",
                    "pyperf_benchmark_name": benchmark,
                    "record_type": "pyperformance_worker_timing_v1",
                }
            )
            + "\n"
        )
        (worker / "jit-code-summary.jsonl").write_text(
            json.dumps(
                {
                    "process_id": index,
                    "function_id": "1:4",
                    "function_qualname": f"run_{benchmark}",
                    "code_size": index * 100,
                    "machine_block_count": index,
                }
            )
            + "\n"
        )
        (worker / "events.jsonl").write_text(
            json.dumps(
                {
                    "event": "soac.typed_v3_function_rewrite",
                    "function_id": "1:4",
                    "final_block_count": index * 3,
                }
            )
            + "\n"
        )

    summary = comparison.summarize_comparison(tmp_path)
    coverage = summary["transformation"]["benchmark_coverage"]

    assert coverage["pickle"]["compiled_functions"] == ["run_pickle"]
    assert coverage["pickle_dict"]["compiled_functions"] == ["run_pickle_dict"]
    assert summary["transformation"]["optimized_typed_ir_block_count"] == 9
    assert summary["transformation"]["native_code_bytes"] == 300
