import importlib.util
import json
from pathlib import Path
import platform

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
    drivers: dict[str, str] | None = None,
) -> None:
    suite = pyperf.BenchmarkSuite(
        [
            pyperf.Benchmark(
                [
                    pyperf.Run(
                        [elapsed, elapsed * 1.01],
                        metadata={
                            **(metadata or {}),
                            **(
                                {"soac_pyperformance_driver": drivers[name]}
                                if drivers is not None
                                else {}
                            ),
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
    stock_drivers: dict[str, str] | None = None,
    soac_drivers: dict[str, str] | None = None,
) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    _write_suite(
        directory / f"round-{index:02d}-stock.json",
        stock,
        metadata=metadata,
        drivers=stock_drivers,
    )
    _write_suite(
        directory / f"round-{index:02d}-soac.json",
        soac,
        metadata=metadata,
        drivers=soac_drivers,
    )


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


def test_comparison_distinguishes_requested_drivers_from_emitted_results(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    drivers = {
        "base64_small": "base64",
        "base64_large": "base64",
        "fastapi_http": "fastapi",
    }
    _write_round(
        tmp_path,
        1,
        stock={"base64_small": 2.0, "base64_large": 4.0, "fastapi_http": 6.0},
        soac={"base64_small": 1.0, "base64_large": 2.0, "fastapi_http": 3.0},
        stock_drivers=drivers,
        soac_drivers=drivers,
    )
    (tmp_path / "requested-benchmarks.txt").write_text(
        "Selected benchmarks:\n- base64\n- fastapi\n", encoding="utf-8"
    )

    summary = comparison.summarize_comparison(tmp_path)

    assert summary["requested_driver_count"] == 2
    assert summary["benchmark_count"] == 3
    assert set(summary["benchmarks"]) == set(drivers)
    assert summary["complete"] is True


def test_comparison_rejects_requested_driver_without_emitted_results(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    drivers = {"base64_small": "base64", "base64_large": "base64"}
    _write_round(
        tmp_path,
        1,
        stock={"base64_small": 2.0, "base64_large": 4.0},
        soac={"base64_small": 1.0, "base64_large": 2.0},
        stock_drivers=drivers,
        soac_drivers=drivers,
    )
    (tmp_path / "requested-benchmarks.txt").write_text(
        "Selected benchmarks:\n- base64\n- fastapi\n", encoding="utf-8"
    )

    with pytest.raises(ValueError, match="missing benchmarks: fastapi"):
        comparison.summarize_comparison(tmp_path)


def test_comparison_rejects_missing_result_from_multi_result_driver(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    stock_drivers = {
        "base64_small": "base64",
        "base64_large": "base64",
        "fastapi_http": "fastapi",
    }
    soac_drivers = {"base64_small": "base64", "fastapi_http": "fastapi"}
    _write_round(
        tmp_path,
        1,
        stock={"base64_small": 2.0, "base64_large": 4.0, "fastapi_http": 6.0},
        soac={"base64_small": 1.0, "fastapi_http": 3.0},
        stock_drivers=stock_drivers,
        soac_drivers=soac_drivers,
    )
    (tmp_path / "requested-benchmarks.txt").write_text(
        "Selected benchmarks:\n- base64\n- fastapi\n", encoding="utf-8"
    )

    with pytest.raises(ValueError, match="missing benchmarks: base64_large"):
        comparison.summarize_comparison(tmp_path)


@pytest.mark.parametrize("drift_mode", ["stock", "soac"])
def test_comparison_rejects_result_driver_mapping_drift_across_rounds(
    tmp_path: Path,
    drift_mode: str,
) -> None:
    comparison = _load_comparison_module()
    drivers = {
        "base64_small": "base64",
        "base64_large": "base64",
        "fastapi_http": "fastapi",
    }
    drifted_drivers = {
        "base64_small": "fastapi",
        "base64_large": "base64",
        "fastapi_http": "base64",
    }
    results = {"base64_small": 2.0, "base64_large": 4.0, "fastapi_http": 6.0}
    _write_round(
        tmp_path,
        1,
        stock=results,
        soac=results,
        stock_drivers=drivers,
        soac_drivers=drivers,
    )
    _write_round(
        tmp_path,
        2,
        stock=results,
        soac=results,
        stock_drivers=drifted_drivers if drift_mode == "stock" else drivers,
        soac_drivers=drifted_drivers if drift_mode == "soac" else drivers,
    )
    (tmp_path / "requested-benchmarks.txt").write_text(
        "Selected benchmarks:\n- base64\n- fastapi\n", encoding="utf-8"
    )

    with pytest.raises(ValueError, match="driver attribution mismatch"):
        comparison.summarize_comparison(tmp_path)


def test_baseline_preflight_rejects_incompatible_platform(tmp_path: Path) -> None:
    comparison = _load_comparison_module()
    baseline = tmp_path / "previous-soac.json"
    _write_suite(
        baseline,
        {"mixed": 1.0},
        metadata={"platform": "incompatible-prior-platform"},
    )

    with pytest.raises(ValueError, match="incompatible platform"):
        comparison.preflight_baseline(baseline)


def test_baseline_preflight_checks_individual_benchmark_python_metadata(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    baseline = tmp_path / "previous-soac.json"
    _write_suite(
        baseline,
        {"startup": 1.0, "normal": 2.0},
        metadata={"platform": platform.platform(True, False)},
    )
    suite = pyperf.BenchmarkSuite.load(str(baseline))
    suite.get_benchmark("normal").update_metadata(
        {"python_version": "incompatible-prior-python"}
    )
    suite.dump(str(baseline), replace=True)

    assert "python_version" not in suite.get_metadata()
    with pytest.raises(ValueError, match="incompatible python_version"):
        comparison.preflight_baseline(baseline)


@pytest.mark.parametrize(
    "drifted_result",
    ["round-01-soac.json", "round-02-stock.json", "round-02-soac.json"],
)
def test_comparison_rejects_per_result_python_drift_across_rounds(
    tmp_path: Path,
    drifted_result: str,
) -> None:
    comparison = _load_comparison_module()
    for index in (1, 2):
        _write_round(
            tmp_path,
            index,
            stock={"startup": 2.0, "normal": 4.0},
            soac={"startup": 1.0, "normal": 2.0},
        )
    for path in tmp_path.glob("round-*.json"):
        suite = pyperf.BenchmarkSuite.load(str(path))
        suite.get_benchmark("normal").update_metadata(
            {
                "python_version": (
                    "incompatible-python"
                    if path.name == drifted_result
                    else "shared-python"
                )
            }
        )
        suite.dump(str(path), replace=True)
        assert "python_version" not in suite.get_metadata()

    with pytest.raises(ValueError, match="incompatible python_version"):
        comparison.summarize_comparison(tmp_path)


def test_comparison_rejects_baseline_per_result_python_drift(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    candidate = tmp_path / "candidate"
    baseline = tmp_path / "baseline"
    for directory in (candidate, baseline):
        _write_round(
            directory,
            1,
            stock={"startup": 2.0, "normal": 4.0},
            soac={"startup": 1.0, "normal": 2.0},
        )
        for path in directory.glob("round-*.json"):
            suite = pyperf.BenchmarkSuite.load(str(path))
            suite.get_benchmark("normal").update_metadata(
                {
                    "python_version": (
                        "incompatible-python"
                        if directory == baseline and "-soac" in path.name
                        else "shared-python"
                    )
                }
            )
            suite.dump(str(path), replace=True)
            assert "python_version" not in suite.get_metadata()

    with pytest.raises(ValueError, match="incompatible python_version"):
        comparison.summarize_comparison(candidate, baseline=baseline)


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


def test_comparison_counts_distinct_apply_worker_processes(tmp_path: Path) -> None:
    comparison = _load_comparison_module()
    _write_round(tmp_path, 1, stock={"mixed": 2.0}, soac={"mixed": 1.0})
    worker = tmp_path / "round-01-soac.soac-work" / "benchmarks" / "mixed-worker"
    module_path = worker / "modules" / "project" / "benchmark_impl" / "mod.blockpy"
    module_path.parent.mkdir(parents=True)
    module_path.write_bytes(b"cache")
    (worker / "pyperformance-worker-timing.jsonl").write_text(
        "\n".join(
            json.dumps(
                {
                    "pid": pid,
                    "opt_mode": mode,
                    "pyperf_benchmark_name": "mixed",
                    "record_type": "pyperformance_worker_timing_v1",
                    "measured_batches": measured_batches,
                }
            )
            for pid, mode, measured_batches in (
                (44, "apply", 1),
                (44, "apply", 1),
                (45, "apply", 1),
                (77, "apply", 0),
                (99, "profile", 1),
            )
        )
        + "\n"
    )
    (worker / "jit-code-summary.jsonl").write_text(
        "\n".join(
            json.dumps(
                {
                    "process_id": pid,
                    "function_id": f"{pid}:1",
                    "function_qualname": f"execute_{pid}",
                    "code_size": size,
                    "machine_block_count": 1,
                }
            )
            for pid, size in ((44, 10), (45, 20), (77, 777), (99, 999))
        )
        + "\n"
    )

    summary = comparison.summarize_comparison(tmp_path)

    assert summary["transformation"]["benchmark_coverage"]["mixed"]["worker_count"] == 2
    assert summary["transformation"]["native_code_bytes"] == 30


def test_comparison_distinguishes_reused_worker_pids_across_worker_directories(
    tmp_path: Path,
) -> None:
    comparison = _load_comparison_module()
    _write_round(tmp_path, 1, stock={"mixed": 2.0}, soac={"mixed": 1.0})
    work_root = tmp_path / "round-01-soac.soac-work" / "benchmarks"
    for index in (1, 2):
        worker = work_root / f"mixed-worker-{index}"
        module_path = worker / "modules" / "project" / "benchmark_impl" / "mod.blockpy"
        module_path.parent.mkdir(parents=True)
        module_path.write_bytes(b"cache")
        (worker / "pyperformance-worker-timing.jsonl").write_text(
            json.dumps(
                {
                    "pid": 44,
                    "opt_mode": "apply",
                    "pyperf_benchmark_name": "mixed",
                    "record_type": "pyperformance_worker_timing_v1",
                    "measured_batches": 1,
                }
            )
            + "\n"
        )
        (worker / "jit-code-summary.jsonl").write_text(
            json.dumps(
                {
                    "process_id": 44,
                    "function_id": f"{index}:1",
                    "function_qualname": f"execute_{index}",
                    "code_size": index * 10,
                    "machine_block_count": 1,
                }
            )
            + "\n"
        )

    summary = comparison.summarize_comparison(tmp_path)

    assert summary["transformation"]["benchmark_coverage"]["mixed"]["worker_count"] == 2
    assert summary["transformation"]["native_code_bytes"] == 30
