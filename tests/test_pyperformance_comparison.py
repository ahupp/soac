import hashlib
import importlib.util
import json
import os
import platform
import subprocess
import sys
import textwrap
from pathlib import Path

import pyperf
import pytest


def test_comparison_result_root_survives_nested_just_recipes(tmp_path):
    root = Path(__file__).resolve().parents[1]
    result_root = tmp_path / "task-owned-results"
    result = subprocess.run(
        [
            "just", "--set", "pyperformance_results_dir", str(result_root),
            "--command", "just", "--command", "printenv", "PYPERFORMANCE_RESULTS_DIR",
        ],
        cwd=root,
        env=dict(os.environ),
        text=True,
        capture_output=True,
        check=True,
    )
    assert result.stdout.strip() == str(result_root)


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
    language: str = "strict",
) -> None:
    suite = pyperf.BenchmarkSuite(
        [
            pyperf.Benchmark(
                [
                    pyperf.Run(
                        [elapsed, elapsed * 1.01],
                        metadata={
                            "soac_pyperformance_language": language,
                            "soac_pyperformance_stock_source_fingerprint": hashlib.sha256(
                                (drivers.get(name, name) if drivers else name).encode()
                            ).hexdigest(),
                            **(
                                {
                                    "soac_pyperformance_strict_source_fingerprint": hashlib.sha256(
                                        (
                                            "strict:"
                                            + (
                                                drivers.get(name, name)
                                                if drivers
                                                else name
                                            )
                                        ).encode()
                                    ).hexdigest(),
                                    "soac_pyperformance_selection_policy": "fixture-selection-v1",
                                    "soac_pyperformance_harness_policy": "fixture-harness-v1",
                                }
                                if language == "strict"
                                else {}
                            ),
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
        language="ordinary",
    )
    _write_suite(
        directory / f"round-{index:02d}-soac.json",
        soac,
        metadata=metadata,
        drivers=soac_drivers,
        language="strict",
    )


def _timing_evidence(benchmark, modules=None):
    modules = modules or {"__main__": "project", "benchmark_impl": "project"}
    return {
        "language": "strict",
        "measured_batches": 1,
        "stock_source_fingerprint": hashlib.sha256(benchmark.encode()).hexdigest(),
        "strict_source_fingerprint": hashlib.sha256(
            ("strict:" + benchmark).encode()
        ).hexdigest(),
        "selection_policy": "fixture-selection-v1",
        "harness_policy": "fixture-harness-v1",
        "artifact_generation": "1" * 64,
        "sealed_strict_modules": [
            {
                "schema": 2,
                "ready": True,
                "strict_assign": True,
                "checked_attr": True,
                "module_name": name,
                "source_kind": kind,
                "source_path": f"/fixture/strict/{name}.py",
                "source_sha256": "2" * 64,
                "artifact_generation": "1" * 64,
                "startup_identity": "3" * 64,
                "interpreter_id": 0,
                "sealed": True,
            }
            for name, kind in modules.items()
        ],
    }


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
    assert summary["sealed_strict_execution_evidence_complete"] is False
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


def test_baseline_preflight_rejects_retired_ordinary_soac_lane(tmp_path):
    comparison = _load_comparison_module()
    baseline = tmp_path / "previous-ordinary-soac.json"
    _write_suite(baseline, {"mixed": 1.0}, language="ordinary")
    with pytest.raises(ValueError, match="does not record strict Python"):
        comparison.preflight_baseline(baseline)


@pytest.mark.parametrize(
    "key, value",
    [
        ("soac_pyperformance_language", "ordinary"),
        ("soac_pyperformance_stock_source_fingerprint", "f" * 64),
        ("soac_pyperformance_local_packages_fingerprint", "f" * 64),
        ("soac_pyperformance_strict_source_fingerprint", "f" * 64),
        ("soac_pyperformance_selection_policy", "other-policy"),
        ("soac_pyperformance_harness_policy", "other-harness"),
    ],
)
def test_comparison_rejects_changed_language_source_or_policy(tmp_path, key, value):
    comparison = _load_comparison_module()
    for index in (1, 2):
        _write_round(tmp_path, index, stock={"mixed": 2.0}, soac={"mixed": 1.0})
    changed = tmp_path / "round-02-soac.json"
    suite = pyperf.BenchmarkSuite.load(str(changed))
    suite.get_benchmark("mixed").update_metadata({key: value})
    suite.dump(str(changed), replace=True)
    with pytest.raises(
        ValueError,
        match="does not record strict Python|incompatible soac_pyperformance",
    ):
        comparison.summarize_comparison(tmp_path)


def test_comparison_rejects_previous_strict_source_selection_drift(tmp_path):
    comparison = _load_comparison_module()
    candidate = tmp_path / "candidate"
    baseline = tmp_path / "prior-strict.json"
    _write_round(candidate, 1, stock={"mixed": 2.0}, soac={"mixed": 1.0})
    _write_suite(
        baseline,
        {"mixed": 1.5},
        metadata={"soac_pyperformance_strict_source_fingerprint": "f" * 64},
    )
    with pytest.raises(
        ValueError, match="incompatible soac_pyperformance_strict_source_fingerprint"
    ):
        comparison.summarize_comparison(candidate, baseline=baseline)


@pytest.mark.parametrize("stock_fingerprint", [None, "b" * 64])
def test_comparison_rejects_local_dependency_preparation_mismatch(
    tmp_path, stock_fingerprint
):
    comparison = _load_comparison_module()
    _write_round(tmp_path, 1, stock={"mixed": 2.0}, soac={"mixed": 1.0})
    for mode, fingerprint in (("stock", stock_fingerprint), ("soac", "a" * 64)):
        if fingerprint is None:
            continue
        path = tmp_path / f"round-01-{mode}.json"
        suite = pyperf.BenchmarkSuite.load(str(path))
        suite.get_benchmark("mixed").update_metadata(
            {
                "soac_pyperformance_local_packages_fingerprint": fingerprint,
            }
        )
        suite.dump(str(path), replace=True)
    with pytest.raises(
        ValueError, match="incompatible soac_pyperformance_local_packages_fingerprint"
    ):
        comparison.summarize_comparison(tmp_path)


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
                **_timing_evidence(
                    "mixed",
                    {
                        "__main__": "project",
                        "benchmark_impl": "project",
                        "json.decoder": "python-stdlib",
                    },
                ),
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
                        "process_id": 12,
                        "function_id": "1:8",
                        "final_block_count": 90,
                    }
                ),
                json.dumps(
                    {
                        "event": "soac.typed_v3_function_rewrite",
                        "process_id": 44,
                        "function_id": "1:4",
                        "final_block_count": 12,
                    }
                ),
                json.dumps(
                    {
                        "event": "soac.typed_v3_function_rewrite",
                        "process_id": 44,
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
    assert summary["transformation"]["project_modules"] == [
        "__main__",
        "benchmark_impl",
    ]
    assert summary["transformation"]["compiled_functions"] == ["execute"]
    assert summary["transformation"]["benchmark_coverage"] == {
        "mixed": {
            "project_modules": ["__main__", "benchmark_impl"],
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
    assert summary["sealed_strict_execution_evidence_complete"] is True
    assert summary["transformation"]["compiled_functions_are_execution_proof"] is False


def test_cache_files_do_not_prove_strict_admission_or_sealing(tmp_path):
    comparison = _load_comparison_module()
    _write_round(tmp_path, 1, stock={"mixed": 2.0}, soac={"mixed": 1.0})
    cache = (
        tmp_path
        / "round-01-soac.soac-work"
        / "benchmarks"
        / "worker"
        / "modules"
        / "project"
        / "fake"
        / "mod.blockpy"
    )
    cache.parent.mkdir(parents=True)
    cache.write_bytes(b"not admission or execution evidence")
    summary = comparison.summarize_comparison(tmp_path)
    assert summary["transformation"]["project_modules"] == []
    assert summary["transformation"]["benchmark_coverage"] == {}
    assert summary["sealed_strict_execution_evidence_complete"] is False


@pytest.mark.parametrize(
    "changed",
    [
        None,
        "schema",
        "ready",
        "strict_assign",
        "checked_attr",
        "seal",
        "source",
        "generation",
        "ordinary",
    ],
)
def test_native_seal_evidence_is_independent_of_compilation_and_checked_per_round(
    tmp_path, changed
):
    comparison = _load_comparison_module()
    for index in (1, 2):
        _write_round(tmp_path, index, stock={"mixed": 2.0}, soac={"mixed": 1.0})
    worker = tmp_path / "round-01-soac.soac-work" / "benchmarks" / "worker"
    worker.mkdir(parents=True)
    row = {
        **_timing_evidence("mixed", {"__main__": "project"}),
        "pid": 44,
        "opt_mode": "apply",
        "pyperf_benchmark_name": "mixed",
        "record_type": "pyperformance_worker_timing_v1",
    }
    if changed == "seal":
        row["sealed_strict_modules"][0]["sealed"] = False
    elif changed in {"schema", "ready", "strict_assign", "checked_attr"}:
        row["sealed_strict_modules"][0][changed] = 1 if changed == "schema" else False
    elif changed == "source":
        row["strict_source_fingerprint"] = "f" * 64
    elif changed == "generation":
        row["sealed_strict_modules"][0]["artifact_generation"] = "f" * 64
    elif changed == "ordinary":
        row["language"] = "ordinary"
    (worker / "pyperformance-worker-timing.jsonl").write_text(json.dumps(row) + "\n")
    summary = comparison.summarize_comparison(tmp_path)
    coverage = summary["transformation"]["benchmark_coverage"]
    if changed is None:
        assert coverage["mixed"]["project_modules"] == ["__main__"]
        assert coverage["mixed"]["worker_count"] == 1
        assert coverage["mixed"]["compiled_functions"] == []
    else:
        assert coverage == {}
    # Round two never supplied evidence, even when round one did.
    assert summary["sealed_strict_execution_evidence_complete"] is False


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
                    **_timing_evidence(benchmark, {"__main__": "project"}),
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
                    "process_id": index,
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
                    **_timing_evidence("mixed"),
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
                    **_timing_evidence("mixed"),
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


def _comparison_plan(rounds, drivers, *, selector="all", extra_args=()):
    runs = []
    for number in range(1, rounds + 1):
        modes = ("stock", "soac") if number % 2 else ("soac", "stock")
        for order, mode in enumerate(modes, 1):
            output = f"round-{number:02d}-{mode}.json"
            phases = (
                [{"name": "stock", "output": output}]
                if mode == "stock"
                else [
                    {
                        "name": "profile",
                        "output": f"round-{number:02d}-soac.profile.json",
                    },
                    {"name": "apply", "output": output},
                ]
            )
            runs.append(
                {
                    "round": number,
                    "order": order,
                    "mode": mode,
                    "output": output,
                    "phases": phases,
                }
            )
    return {
        "schema": 1,
        "requested_drivers": sorted(drivers),
        "requested_rounds": rounds,
        "benchmark_selector": selector,
        "extra_args": list(extra_args),
        "baseline": None,
        "runs": runs,
    }


@pytest.mark.parametrize("any_results", [False, True])
def test_incomplete_comparison_cli_preserves_a_machine_readable_negative_report(
    tmp_path, any_results
):
    (tmp_path / "requested-benchmarks.txt").write_text("- accepted\n- rejected\n")
    if any_results:
        _write_round(
            tmp_path,
            1,
            stock={"accepted": 2.0, "rejected": 4.0},
            soac={"accepted": 1.0},
        )
    report = tmp_path / "summary.json"
    script = (
        Path(__file__).resolve().parents[1]
        / "scripts/summarize_pyperformance_comparison.py"
    )
    result = subprocess.run(
        [sys.executable, str(script), str(tmp_path), "--json-out", str(report)],
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0
    assert report.is_file(), result.stderr
    summary = json.loads(report.read_text())
    assert summary["complete"] is False
    assert summary["requested_drivers"] == ["accepted", "rejected"]
    assert summary["requested_driver_count"] == 2
    assert summary["failures"]
    assert "geometric_mean_speedup_vs_stock" not in summary
    assert "geometric_mean_speedup_vs_baseline_soac" not in summary
    assert not (tmp_path / "stock.json").exists()
    assert not (tmp_path / "soac.json").exists()


def test_comparison_cannot_relabel_one_pair_as_a_complete_three_round_request(tmp_path):
    comparison = _load_comparison_module()
    (tmp_path / "requested-benchmarks.txt").write_text("- mixed\n")
    (tmp_path / "comparison-plan.json").write_text(
        json.dumps(_comparison_plan(3, ["mixed"])) + "\n"
    )
    _write_round(tmp_path, 1, stock={"mixed": 2.0}, soac={"mixed": 1.0})
    with pytest.raises(ValueError, match="requested.*round|round.*missing"):
        comparison.summarize_comparison(tmp_path)
    assert not (tmp_path / "stock.json").exists()
    assert not (tmp_path / "soac.json").exists()


def _write_driver_phase_report(output, phase, drivers, *, rejected=()):
    records = [
        {
            "benchmark": name,
            "status": "failed" if name in rejected else "succeeded",
            "stage": "strict_preparation" if name in rejected else "complete",
            "emitted_results": [] if name in rejected else [name],
            **(
                {"error": "RuntimeError: synthetic checker rejection"}
                if name in rejected
                else {}
            ),
        }
        for name in sorted(drivers)
    ]
    Path(str(output) + ".status.json").write_text(
        json.dumps(
            {
                "schema": 1,
                "output": str(output),
                "language": "ordinary" if phase == "stock" else "strict",
                "optimization_mode": None if phase == "stock" else phase,
                "requested_drivers": sorted(drivers),
                "records": records,
                "exit_code": int(bool(rejected)),
                "complete": not rejected,
            }
        )
        + "\n"
    )


def test_comparison_rounds_continue_after_partial_strict_failures_without_shrinking(
    tmp_path, monkeypatch
):
    comparison = _load_comparison_module()
    (tmp_path / "requested-benchmarks.txt").write_text("- accepted\n- rejected\n")
    monkeypatch.setenv("SOAC_WORK_DIR", "/must/not/reuse")
    monkeypatch.setenv("SOAC_OPT_MODE", "verify")
    calls = []

    def run(command, environment, log_path):
        assert "SOAC_WORK_DIR" not in environment
        assert "SOAC_OPT_MODE" not in environment
        assert command[:2] == ["just", "pyperformance"]
        assert command[4:] == ["", "--debug-single-value"]
        mode, output = command[2], Path(command[3])
        calls.append((mode, output.name))
        log_path.write_text("synthetic orchestration: no benchmark worker\n")
        if mode == "stock":
            _write_suite(
                output, {"accepted": 2.0, "rejected": 4.0}, language="ordinary"
            )
            _write_driver_phase_report(output, "stock", ["accepted", "rejected"])
            return 0
        profile = output.with_name(output.stem + ".profile.json")
        for phase, path in (("profile", profile), ("apply", output)):
            _write_suite(path, {"accepted": 1.0})
            _write_driver_phase_report(
                path, phase, ["accepted", "rejected"], rejected=["rejected"]
            )
        return 1

    status = comparison.run_comparison_rounds(
        tmp_path,
        rounds=3,
        benchmarks="all",
        extra_args=["--debug-single-value"],
        run=run,
    )
    assert [mode for mode, _ in calls] == [
        "stock",
        "soac",
        "soac",
        "stock",
        "stock",
        "soac",
    ]
    plan = json.loads((tmp_path / "comparison-plan.json").read_text())
    assert plan == _comparison_plan(
        3, ["accepted", "rejected"], extra_args=["--debug-single-value"]
    )
    assert [row["exit_code"] for row in status["runs"]] == [0, 1, 1, 0, 0, 1]
    assert all(row["status"] in {"succeeded", "failed"} for row in status["runs"])
    report = comparison.build_comparison_report(tmp_path)
    assert report["complete"] is False
    assert report["requested_driver_count"] == 2
    assert report["requested_rounds"] == 3
    assert len(report["driver_failures"]) == 6
    assert {row["benchmark"] for row in report["driver_failures"]} == {"rejected"}
    assert {row["phase"] for row in report["driver_failures"]} == {"profile", "apply"}
    assert "geometric_mean_speedup_vs_stock" not in report
    assert not (tmp_path / "stock.json").exists()
    assert not (tmp_path / "soac.json").exists()
    with pytest.raises(ValueError, match="already exists"):
        comparison.run_comparison_rounds(
            tmp_path, rounds=3, benchmarks="all", extra_args=[], run=run
        )
    assert len(calls) == 6


def _run_synthetic_comparison(
    directory,
    *,
    rounds=1,
    drivers=("mixed",),
    profile_rejected=(),
    apply_rejected=(),
    baseline=None,
):
    """Exercise orchestration with structured results, never benchmark workers."""
    comparison = _load_comparison_module()
    (directory / "requested-benchmarks.txt").write_text(
        "".join(f"- {name}\n" for name in drivers)
    )

    def run(command, _environment, log_path):
        mode, output = command[2], Path(command[3])
        log_path.write_text("synthetic phase results; no benchmark worker\n")
        phases = (
            [("stock", output)]
            if mode == "stock"
            else [
                ("profile", output.with_name(output.stem + ".profile.json")),
                ("apply", output),
            ]
        )
        for phase, path in phases:
            rejected = (
                profile_rejected
                if phase == "profile"
                else apply_rejected
                if phase == "apply"
                else ()
            )
            _write_suite(
                path,
                {
                    name: 2.0 if phase == "stock" else 1.0
                    for name in drivers
                    if name not in rejected
                },
                language="ordinary" if phase == "stock" else "strict",
                metadata={"python_version": "3.15.0"},
            )
            _write_driver_phase_report(path, phase, drivers, rejected=rejected)
        return int(mode == "soac" and bool(profile_rejected or apply_rejected))

    status = comparison.run_comparison_rounds(
        directory,
        rounds=rounds,
        benchmarks="all",
        extra_args=[],
        baseline=baseline,
        run=run,
    )
    return comparison, status


def test_complete_planned_comparison_keeps_measurement_and_native_coverage_separate(
    tmp_path,
):
    comparison, status = _run_synthetic_comparison(tmp_path, rounds=3)
    assert status["complete"] is True
    summary = comparison.build_comparison_report(tmp_path)
    assert summary["complete"] is True, summary["failures"]
    assert summary["round_count"] == summary["requested_rounds"] == 3
    assert summary["requested_drivers"] == ["mixed"]
    assert summary["geometric_mean_speedup_vs_stock"] == pytest.approx(2.0)
    assert len(summary["phases"]) == 9
    assert summary["sealed_strict_execution_evidence_complete"] is False
    assert (tmp_path / "stock.json").is_file()
    assert (tmp_path / "soac.json").is_file()


def test_successful_apply_cannot_erase_failed_profile_drivers(tmp_path):
    comparison, _ = _run_synthetic_comparison(
        tmp_path, drivers=("accepted", "rejected"), profile_rejected=("rejected",)
    )
    summary = comparison.build_comparison_report(tmp_path)
    assert summary["complete"] is False
    assert [(row["phase"], row["benchmark"]) for row in summary["driver_failures"]] == [
        ("profile", "rejected")
    ]
    apply = next(row for row in summary["phases"] if row["phase"] == "apply")
    assert apply["complete"] is True
    assert {row["driver"] for row in apply["results"]} == {"accepted", "rejected"}
    assert "geometric_mean_speedup_vs_stock" not in summary
    assert not (tmp_path / "stock.json").exists()


@pytest.mark.parametrize(
    "corruption", ["plan_hash", "phase_missing", "phase_nonterminal"]
)
def test_planned_comparison_requires_matching_terminal_phase_evidence(
    tmp_path, corruption
):
    comparison, _ = _run_synthetic_comparison(tmp_path)
    if corruption == "plan_hash":
        path = tmp_path / "run-status.json"
        data = json.loads(path.read_text())
        data["plan_sha256"] = "0" * 64
        path.write_text(json.dumps(data))
    else:
        path = tmp_path / "round-01-soac.profile.json.status.json"
        if corruption == "phase_missing":
            path.unlink()
        else:
            data = json.loads(path.read_text())
            data["exit_code"] = None
            data["complete"] = False
            path.write_text(json.dumps(data))
    summary = comparison.build_comparison_report(tmp_path)
    assert summary["complete"] is False
    assert summary["failures"]
    assert "geometric_mean_speedup_vs_stock" not in summary
    assert not (tmp_path / "stock.json").exists()


def test_interrupted_comparison_preserves_the_unattempted_request(tmp_path):
    comparison = _load_comparison_module()
    (tmp_path / "requested-benchmarks.txt").write_text("- mixed\n")
    calls = []

    def interrupted(command, _environment, log_path):
        calls.append(command)
        log_path.write_text("synthetic interruption\n")
        raise KeyboardInterrupt

    status = comparison.run_comparison_rounds(
        tmp_path, rounds=3, benchmarks="all", extra_args=[], run=interrupted
    )
    assert len(calls) == 1
    assert status["interrupted"] is True
    assert status["runs"][0]["exit_code"] == 130
    assert [row["status"] for row in status["runs"]] == ["interrupted"] + [
        "not_run"
    ] * 5
    summary = comparison.build_comparison_report(tmp_path)
    assert summary["complete"] is False
    assert summary["requested_rounds"] == 3
    assert len(summary["driver_failures"]) == 9
    assert "geometric_mean_speedup_vs_stock" not in summary


@pytest.mark.parametrize(
    ("profile_exit", "profile_written", "apply_exit", "expected_exit"),
    [(23, True, 0, 1), (0, False, 0, 1), (0, True, 31, 1), (0, True, 0, 0)],
)
def test_actual_soac_recipe_attempts_apply_and_preserves_phase_failure(
    tmp_path, profile_exit, profile_written, apply_exit, expected_exit
):
    repo = Path(__file__).resolve().parents[1]
    lines = (repo / "Justfile").read_text().splitlines()
    start = next(
        i for i, line in enumerate(lines) if line.startswith("pyperformance mode=")
    )
    end = next(
        (
            i
            for i in range(start + 1, len(lines))
            if lines[i] and not lines[i].startswith((" ", "#"))
        ),
        len(lines),
    )
    body = "\n".join(line.removeprefix("  ") for line in lines[start + 1 : end])
    output = tmp_path / "result.json"
    body = (
        body.replace("{{mode}}", "soac")
        .replace("{{output}}", str(output))
        .replace("{{benchmarks}}", "driver")
        .replace("{{args}}", "--debug-single-value")
    )
    # Run the actual recipe's control flow. Only external setup/benchmark
    # commands are stubs; neither an analyzer nor a timed worker is launched.
    executable = tmp_path / "venv" / "bin" / "python"
    executable.parent.mkdir(parents=True)
    executable.write_text(
        f"#!{sys.executable}\n"
        + textwrap.dedent(
            """
            import json
            import os
            from pathlib import Path
            import sys

            if sys.argv[1] == "-c":
                print(Path(os.environ["REPO_ROOT"]) / "site-packages")
                raise SystemExit(0)
            script = Path(sys.argv[1]).name
            if script != "run_pyperformance_cached.py":
                raise SystemExit(0)
            arguments = sys.argv[2:]
            output = Path(arguments[arguments.index("-o") + 1])
            phase = os.environ["SOAC_OPT_MODE"]
            with Path(os.environ["FAKE_PHASE_LOG"]).open("a") as log:
                log.write(json.dumps({"phase": phase, "arguments": arguments}) + "\\n")
            output.write_text("{}\\n")
            if phase == "profile" and os.environ["FAKE_PROFILE_WRITTEN"] == "1":
                (Path(os.environ["SOAC_WORK_DIR"]) / "profile.bin").write_bytes(b"profile")
            raise SystemExit(int(os.environ[f"FAKE_{phase.upper()}_EXIT"]))
            """
        )
    )
    executable.chmod(0o755)
    script = tmp_path / "actual-recipe.sh"
    script.write_text("just() { :; }\n" + body + "\n")
    phases = tmp_path / "phase-calls.jsonl"
    environment = {
        **os.environ,
        "CPYTHON_LIB_DIR": str(tmp_path),
        "CPYTHON_BIN": str(executable),
        "REPO_ROOT": str(tmp_path),
        "VENV_DIR": str(executable.parents[1]),
        "PYPERFORMANCE_RESULTS_DIR": str(tmp_path / "results"),
        "XDG_CACHE_HOME": str(tmp_path / "cache"),
        "SOAC_WORK_DIR": str(tmp_path / "worker"),
        "FAKE_PHASE_LOG": str(phases),
        "FAKE_PROFILE_EXIT": str(profile_exit),
        "FAKE_PROFILE_WRITTEN": str(int(profile_written)),
        "FAKE_APPLY_EXIT": str(apply_exit),
    }
    result = subprocess.run(
        ["bash", str(script)],
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == expected_exit, result.stdout + result.stderr
    calls = [json.loads(line) for line in phases.read_text().splitlines()]
    assert [row["phase"] for row in calls] == ["profile", "apply"]
    assert all("--benchmarks=driver" in row["arguments"] for row in calls)


@pytest.mark.parametrize(
    "argument",
    [
        "--benchmarks=smaller",
        "-bsmaller",
        "--bench=smaller",
        "--output=other",
        "-oother",
        "--append=old.json",
        "--python=/other",
        "-p/other",
    ],
)
def test_comparison_rejects_extra_arguments_that_replace_the_frozen_request(
    tmp_path, argument
):
    comparison = _load_comparison_module()
    (tmp_path / "requested-benchmarks.txt").write_text("- mixed\n")
    calls = []
    with pytest.raises(ValueError, match="comparison owns"):
        comparison.run_comparison_rounds(
            tmp_path,
            rounds=3,
            benchmarks="all",
            extra_args=[argument],
            run=lambda *args: calls.append(args),
        )
    assert calls == []
    assert not (tmp_path / "comparison-plan.json").exists()


@pytest.mark.parametrize("rejected", [False, True])
def test_comparison_cli_runs_the_frozen_rounds_and_always_writes_a_report(
    tmp_path, rejected
):
    (tmp_path / "requested-benchmarks.txt").write_text("- accepted\n- rejected\n")
    helpers = Path(__file__).resolve()
    executable = tmp_path / "bin" / "just"
    executable.parent.mkdir()
    executable.write_text(
        f"#!{sys.executable}\n"
        + textwrap.dedent(
            f"""
            import importlib.util
            import json
            import os
            from pathlib import Path
            import sys
            spec = importlib.util.spec_from_file_location("synthetic_results", {str(helpers)!r})
            fixtures = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(fixtures)
            assert sys.argv[1] == "pyperformance"
            assert sys.argv[4:] == ["", "--debug-single-value"]
            mode, output = sys.argv[2], Path(sys.argv[3])
            with Path(os.environ["SYNTHETIC_CALLS"]).open("a") as log:
                log.write(json.dumps({{"mode": mode, "output": str(output)}}) + "\\n")
            rejected = ["rejected"] if mode == "soac" and {rejected!r} else []
            phases = (
                [("stock", output)] if mode == "stock" else
                [("profile", output.with_name(output.stem + ".profile.json")), ("apply", output)]
            )
            for phase, path in phases:
                fixtures._write_suite(
                    path,
                    {{name: 2.0 if mode == "stock" else 1.0
                     for name in ["accepted", "rejected"] if name not in rejected}},
                    language="ordinary" if mode == "stock" else "strict",
                )
                fixtures._write_driver_phase_report(
                    path, phase, ["accepted", "rejected"], rejected=rejected
                )
            print("synthetic orchestration subprocess; no benchmark worker")
            raise SystemExit(int(bool(rejected)))
            """
        )
    )
    executable.chmod(0o755)
    script = helpers.parents[1] / "scripts/summarize_pyperformance_comparison.py"
    calls = tmp_path / "calls.jsonl"
    result = subprocess.run(
        [
            sys.executable,
            str(script),
            str(tmp_path),
            "--run-rounds",
            "2",
            "--benchmarks",
            "all",
            "--pyperformance-args",
            "--debug-single-value",
        ],
        env={
            **os.environ,
            "PATH": str(executable.parent) + os.pathsep + os.environ["PATH"],
            "SYNTHETIC_CALLS": str(calls),
        },
        text=True,
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert result.returncode == int(rejected), result.stdout + result.stderr
    assert [json.loads(line)["mode"] for line in calls.read_text().splitlines()] == [
        "stock",
        "soac",
        "soac",
        "stock",
    ]
    summary = json.loads((tmp_path / "summary.json").read_text())
    assert summary["complete"] is not rejected
    assert summary["requested_drivers"] == ["accepted", "rejected"]
    assert summary["requested_rounds"] == 2
    assert (tmp_path / "summary.txt").is_file()
    if rejected:
        assert len(summary["driver_failures"]) == 4
        assert "geometric_mean_speedup_vs_stock" not in summary
        assert not (tmp_path / "stock.json").exists()
    else:
        assert summary["geometric_mean_speedup_vs_stock"] == pytest.approx(2.0)
        assert summary["sealed_strict_execution_evidence_complete"] is False


def test_incomplete_comparison_preserves_full_round_ratios_and_partial_native_coverage(
    tmp_path,
):
    comparison, _ = _run_synthetic_comparison(
        tmp_path,
        rounds=3,
        drivers=("accepted", "rejected"),
        profile_rejected=("rejected",),
        apply_rejected=("rejected",),
    )
    worker = tmp_path / "round-01-soac.soac-work" / "benchmarks" / "accepted-worker"
    worker.mkdir(parents=True)
    (worker / "pyperformance-worker-timing.jsonl").write_text(
        json.dumps(
            {
                **_timing_evidence("accepted"),
                "pid": 44,
                "opt_mode": "apply",
                "pyperf_benchmark_name": "accepted",
                "record_type": "pyperformance_worker_timing_v1",
            }
        )
        + "\n"
    )
    (worker / "jit-code-summary.jsonl").write_text(
        json.dumps(
            {
                "process_id": 44,
                "function_id": "1:4",
                "function_qualname": "execute",
                "code_size": 120,
                "machine_block_count": 3,
            }
        )
        + "\n"
    )
    summary = comparison.build_comparison_report(tmp_path)
    assert summary["complete"] is False
    assert summary["requested_driver_count"] == 2
    partial = summary["partial_evidence"]
    assert partial["requested_rounds"] == 3
    assert list(partial["benchmarks"]) == ["accepted"]
    assert partial["benchmarks"]["accepted"]["paired_round_count"] == 3
    assert partial["benchmarks"]["accepted"]["speedup_vs_stock"] == pytest.approx(2.0)
    assert partial["available_apply_rounds"] == [1, 2, 3]
    coverage = partial["transformation"]
    assert coverage["compiled_functions"] == ["execute"]
    assert coverage["native_code_bytes"] == 120
    assert coverage["benchmark_coverage"]["accepted"]["worker_count"] == 1
    assert coverage["sealed_round_benchmarks"] == [["accepted"], [], []]
    assert "geometric_mean_speedup_vs_stock" not in partial
    assert "geometric_mean_speedup_vs_stock" not in summary
    assert not (tmp_path / "stock.json").exists()


@pytest.mark.parametrize(
    "problem",
    ["missing_pair", "driver_drift", "source_drift", "python_drift", "profile_drift"],
)
def test_partial_comparison_never_averages_incomplete_or_incompatible_pairs(
    tmp_path, problem
):
    comparison, _ = _run_synthetic_comparison(
        tmp_path,
        rounds=3,
        drivers=("accepted", "rejected"),
        profile_rejected=("rejected",),
        apply_rejected=("rejected",),
    )
    path = tmp_path / (
        "round-02-soac.profile.json"
        if problem == "profile_drift"
        else "round-02-soac.json"
    )
    if problem == "missing_pair":
        path.unlink()
    else:
        suite = pyperf.BenchmarkSuite.load(str(path))
        key, value = {
            "driver_drift": ("soac_pyperformance_driver", "another-driver"),
            "source_drift": ("soac_pyperformance_strict_source_fingerprint", "f" * 64),
            "python_drift": ("python_version", "incompatible-python"),
            "profile_drift": ("soac_pyperformance_selection_policy", "other-selection"),
        }[problem]
        suite.get_benchmark("accepted").update_metadata({key: value})
        suite.dump(str(path), replace=True)
    summary = comparison.build_comparison_report(tmp_path)
    assert summary["complete"] is False
    partial = summary["partial_evidence"]
    assert partial["benchmarks"] == {}
    assert partial["issues"]
    assert "geometric_mean_speedup_vs_stock" not in summary
    assert not (tmp_path / "stock.json").exists()


@pytest.mark.parametrize("changed_baseline", [False, True])
def test_partial_previous_soac_ratios_keep_per_result_source_checks(
    tmp_path, changed_baseline
):
    previous = tmp_path / "previous.json"
    metadata = {"python_version": "3.15.0"}
    if changed_baseline:
        metadata["soac_pyperformance_strict_source_fingerprint"] = "f" * 64
    _write_suite(previous, {"accepted": 3.0, "rejected": 6.0}, metadata=metadata)
    comparison, _ = _run_synthetic_comparison(
        tmp_path,
        rounds=3,
        drivers=("accepted", "rejected"),
        profile_rejected=("rejected",),
        apply_rejected=("rejected",),
        baseline=previous,
    )
    summary = comparison.build_comparison_report(tmp_path)
    assert summary["complete"] is False
    benchmark = summary["partial_evidence"]["benchmarks"]["accepted"]
    assert benchmark["speedup_vs_stock"] == pytest.approx(2.0)
    if changed_baseline:
        assert "speedup_vs_baseline_soac" not in benchmark
        assert benchmark["baseline_error"]
    else:
        assert benchmark["speedup_vs_baseline_soac"] == pytest.approx(3.0)
    assert "geometric_mean_speedup_vs_baseline_soac" not in summary
