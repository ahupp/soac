from __future__ import annotations

import argparse
import json
import platform
import statistics
from pathlib import Path
from typing import Any

import pyperf
from pyperf._collect_metadata import collect_python_metadata
import pyperformance


_DRIVER_METADATA = "soac_pyperformance_driver"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Merge independent stock/SOAC pyperformance runs, validate benchmark "
            "coverage, and report paired speedups plus transformed-module coverage."
        )
    )
    parser.add_argument("comparison_dir", type=Path, nargs="?")
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--preflight-baseline", type=Path)
    parser.add_argument("--json-out", type=Path)
    return parser.parse_args()


def _load_suite(path: Path) -> pyperf.BenchmarkSuite:
    if not path.is_file():
        raise ValueError(f"pyperformance result does not exist: {path}")
    return pyperf.BenchmarkSuite.load(str(path))


def _benchmark_elapsed(suite: pyperf.BenchmarkSuite) -> dict[str, float]:
    return {
        benchmark.get_name(): benchmark.mean() for benchmark in suite.get_benchmarks()
    }


def _benchmark_drivers(suite: pyperf.BenchmarkSuite) -> dict[str, str]:
    drivers = {}
    for benchmark in suite.get_benchmarks():
        name = benchmark.get_name()
        driver = benchmark.get_metadata().get(_DRIVER_METADATA, name)
        if not isinstance(driver, str) or not driver:
            raise ValueError(f"benchmark {name} has invalid driver attribution")
        drivers[name] = driver
    return drivers


def _require_benchmarks(
    actual: dict[str, float], expected: set[str], *, label: str
) -> None:
    missing = sorted(expected.difference(actual))
    unexpected = sorted(set(actual).difference(expected))
    problems = []
    if missing:
        problems.append(f"missing benchmarks: {', '.join(missing)}")
    if unexpected:
        problems.append(f"unexpected benchmarks: {', '.join(unexpected)}")
    if problems:
        raise ValueError(
            f"{label} has incomplete comparison coverage: {'; '.join(problems)}"
        )


def _require_comparable_metadata(
    suite: pyperf.BenchmarkSuite | pyperf.Benchmark,
    expected_metadata: dict[str, Any],
    *,
    label: str,
) -> None:
    metadata = suite.get_metadata()
    for key in (
        "python_version",
        "python_implementation",
        "performance_version",
        "platform",
        "cpu_affinity",
    ):
        actual = metadata.get(key)
        expected = expected_metadata.get(key)
        if actual is not None and expected is not None and actual != expected:
            raise ValueError(
                f"{label} has incompatible {key}: {actual!r} != {expected!r}"
            )


def _require_driver_attribution(
    actual: dict[str, str], expected: dict[str, str], *, label: str
) -> None:
    mismatches = sorted(
        name for name, driver in actual.items() if expected.get(name) != driver
    )
    if mismatches:
        raise ValueError(
            f"{label} has driver attribution mismatch: {', '.join(mismatches)}"
        )


def _require_comparable_benchmark_metadata(
    suite: pyperf.BenchmarkSuite,
    expected: dict[str, dict[str, Any]],
    *,
    label: str,
) -> None:
    for benchmark in suite.get_benchmarks():
        metadata = expected.get(benchmark.get_name())
        if metadata is not None:
            _require_comparable_metadata(
                benchmark,
                metadata,
                label=f"{label} benchmark {benchmark.get_name()}",
            )


def preflight_baseline(baseline: Path) -> None:
    current = {
        "platform": platform.platform(True, False),
        "performance_version": pyperformance.__version__,
    }
    collect_python_metadata(current)
    suite = _load_suite(baseline)
    _require_comparable_metadata(suite, current, label=f"baseline {baseline}")
    for benchmark in suite.get_benchmarks():
        _require_comparable_metadata(
            benchmark,
            current,
            label=f"baseline {baseline} benchmark {benchmark.get_name()}",
        )


def _merge_suites(paths: list[Path], output: Path) -> pyperf.BenchmarkSuite:
    merged = _load_suite(paths[0])
    for path in paths[1:]:
        merged.add_runs(_load_suite(path))
    merged.dump(str(output), replace=True)
    return merged


def _read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows = []
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict):
                rows.append(row)
    return rows


def _transformation_coverage(soac_paths: list[Path]) -> dict[str, Any]:
    project_modules: set[str] = set()
    stdlib_modules: set[str] = set()
    compiled_functions: set[str] = set()
    per_benchmark: dict[str, dict[str, Any]] = {}
    round_metrics: list[dict[str, int]] = []

    for soac_path in soac_paths:
        work_root = Path(f"{soac_path.with_suffix('')}.soac-work")
        if not work_root.is_dir():
            continue

        metrics = {
            "preoptimization_project_blockpy_bytes": 0,
            "preoptimization_stdlib_blockpy_bytes": 0,
            "optimized_typed_ir_block_count": 0,
            "optimized_typed_ir_function_count": 0,
            "native_code_bytes": 0,
            "native_machine_blocks": 0,
        }

        for module_path in work_root.rglob("mod.blockpy"):
            parts = module_path.parts
            try:
                modules_index = parts.index("modules")
                source_kind = parts[modules_index + 1]
            except (ValueError, IndexError):
                continue
            module_name = ".".join(parts[modules_index + 2 : -1])
            if not module_name:
                continue
            if source_kind == "python-stdlib":
                stdlib_modules.add(module_name)
                metrics["preoptimization_stdlib_blockpy_bytes"] += (
                    module_path.stat().st_size
                )
            elif source_kind == "project":
                project_modules.add(module_name)
                metrics["preoptimization_project_blockpy_bytes"] += (
                    module_path.stat().st_size
                )

        for summary_path in work_root.rglob("jit-code-summary.jsonl"):
            worker_dir = summary_path.parent
            timing_path = worker_dir / "pyperformance-worker-timing.jsonl"
            if not timing_path.is_file():
                continue
            apply_rows = [
                row
                for row in _read_jsonl(timing_path)
                if row.get("record_type") == "pyperformance_worker_timing_v1"
                and row.get("opt_mode") == "apply"
                and isinstance(row.get("pid"), int)
                and row.get("measured_batches") != 0
            ]
            apply_pids = {row["pid"] for row in apply_rows}
            if not apply_pids:
                continue

            apply_function_ids: set[str] = set()
            worker_functions: set[str] = set()
            for row in _read_jsonl(summary_path):
                if row.get("process_id") not in apply_pids:
                    continue
                qualname = row.get("function_qualname")
                if isinstance(qualname, str):
                    compiled_functions.add(qualname)
                    worker_functions.add(qualname)
                if isinstance((function_id := row.get("function_id")), str):
                    apply_function_ids.add(function_id)
                if isinstance((code_size := row.get("code_size")), int):
                    metrics["native_code_bytes"] += code_size
                if isinstance((block_count := row.get("machine_block_count")), int):
                    metrics["native_machine_blocks"] += block_count

            worker_module_root = worker_dir / "modules"
            worker_project_modules = {
                ".".join(path.relative_to(worker_module_root / "project").parts[:-1])
                for path in (worker_module_root / "project").rglob("mod.blockpy")
            }
            worker_stdlib_modules = {
                ".".join(
                    path.relative_to(worker_module_root / "python-stdlib").parts[:-1]
                )
                for path in (worker_module_root / "python-stdlib").rglob("mod.blockpy")
            }
            worker_benchmarks = {
                benchmark_name
                for row in apply_rows
                if isinstance((benchmark_name := row.get("pyperf_benchmark_name")), str)
                and benchmark_name
            }
            for benchmark_name in worker_benchmarks:
                coverage = per_benchmark.setdefault(
                    benchmark_name,
                    {
                        "project_modules": set(),
                        "stdlib_modules": set(),
                        "compiled_functions": set(),
                        "worker_pids": set(),
                    },
                )
                coverage["project_modules"].update(worker_project_modules)
                coverage["stdlib_modules"].update(worker_stdlib_modules)
                coverage["compiled_functions"].update(worker_functions)
                coverage["worker_pids"].update(
                    (worker_dir.resolve(), row["pid"])
                    for row in apply_rows
                    if row.get("pyperf_benchmark_name") == benchmark_name
                )

            events_path = worker_dir / "events.jsonl"
            if not events_path.is_file():
                continue
            final_typed_block_counts: dict[str, int] = {}
            for event in _read_jsonl(events_path):
                function_id = event.get("function_id")
                if (
                    event.get("event") == "soac.typed_v3_function_rewrite"
                    and isinstance(function_id, str)
                    and function_id in apply_function_ids
                    and isinstance((block_count := event.get("final_block_count")), int)
                ):
                    final_typed_block_counts[function_id] = block_count
            metrics["optimized_typed_ir_block_count"] += sum(
                final_typed_block_counts.values()
            )
            metrics["optimized_typed_ir_function_count"] += len(
                final_typed_block_counts
            )

        round_metrics.append(metrics)

    metric_names = (
        "preoptimization_project_blockpy_bytes",
        "preoptimization_stdlib_blockpy_bytes",
        "optimized_typed_ir_block_count",
        "optimized_typed_ir_function_count",
        "native_code_bytes",
        "native_machine_blocks",
    )
    median_metrics = {
        name: int(statistics.median(metrics[name] for metrics in round_metrics))
        if round_metrics
        else 0
        for name in metric_names
    }

    return {
        "project_modules": sorted(project_modules),
        "stdlib_modules": sorted(stdlib_modules),
        "compiled_functions": sorted(compiled_functions),
        "benchmark_coverage": {
            name: {
                "project_modules": sorted(coverage["project_modules"]),
                "stdlib_modules": sorted(coverage["stdlib_modules"]),
                "compiled_functions": sorted(coverage["compiled_functions"]),
                "worker_count": len(coverage["worker_pids"]),
            }
            for name, coverage in sorted(per_benchmark.items())
        },
        "metric_aggregation": "median_per_round",
        **median_metrics,
        "preoptimization_blockpy_bytes": (
            median_metrics["preoptimization_project_blockpy_bytes"]
            + median_metrics["preoptimization_stdlib_blockpy_bytes"]
        ),
    }


def _baseline_elapsed(
    baseline: Path,
    expected: set[str],
    expected_metadata: dict[str, Any],
    expected_benchmark_metadata: dict[str, dict[str, Any]],
) -> dict[str, float]:
    if baseline.is_dir():
        paths = sorted(baseline.glob("round-*-soac.json"))
        merged = baseline / "soac.json"
        if not paths and merged.is_file():
            paths = [merged]
        if not paths:
            raise ValueError(f"baseline comparison has no SOAC result: {baseline}")
    else:
        paths = [baseline]

    per_benchmark: dict[str, list[float]] = {name: [] for name in expected}
    for path in paths:
        suite = _load_suite(path)
        _require_comparable_metadata(
            suite,
            expected_metadata,
            label=f"baseline {path}",
        )
        _require_comparable_benchmark_metadata(
            suite,
            expected_benchmark_metadata,
            label=f"baseline {path}",
        )
        elapsed = _benchmark_elapsed(suite)
        _require_benchmarks(elapsed, expected, label=f"baseline {path}")
        for name, value in elapsed.items():
            per_benchmark[name].append(value)
    return {name: statistics.median(values) for name, values in per_benchmark.items()}


def summarize_comparison(
    comparison_dir: Path, *, baseline: Path | None = None
) -> dict[str, Any]:
    stock_paths = sorted(comparison_dir.glob("round-*-stock.json"))
    if not stock_paths:
        raise ValueError(f"no stock pyperformance rounds found in {comparison_dir}")

    soac_paths = []
    stock_samples: dict[str, list[float]] = {}
    soac_samples: dict[str, list[float]] = {}
    speedup_samples: dict[str, list[float]] = {}
    reference_metadata: dict[str, Any] | None = None
    reference_benchmark_metadata: dict[str, dict[str, Any]] | None = None
    expected: set[str] | None = None
    expected_drivers: dict[str, str] | None = None
    requested_path = comparison_dir / "requested-benchmarks.txt"
    if requested_path.is_file():
        requested_drivers: set[str] | None = {
            line.removeprefix("- ").strip()
            for line in requested_path.read_text(encoding="utf-8").splitlines()
            if line.startswith("- ")
        }
        if not requested_drivers:
            raise ValueError(f"requested benchmark list is empty: {requested_path}")
    else:
        requested_drivers = None

    for stock_path in stock_paths:
        soac_path = stock_path.with_name(
            stock_path.name.replace("-stock.json", "-soac.json")
        )
        soac_paths.append(soac_path)
        stock_suite = _load_suite(stock_path)
        soac_suite = _load_suite(soac_path)
        if reference_metadata is None:
            reference_metadata = stock_suite.get_metadata()
            reference_benchmark_metadata = {
                benchmark.get_name(): benchmark.get_metadata()
                for benchmark in stock_suite.get_benchmarks()
            }
        _require_comparable_metadata(
            stock_suite,
            reference_metadata,
            label=str(stock_path),
        )
        _require_comparable_metadata(
            soac_suite,
            reference_metadata,
            label=str(soac_path),
        )
        assert reference_benchmark_metadata is not None
        _require_comparable_benchmark_metadata(
            stock_suite,
            reference_benchmark_metadata,
            label=str(stock_path),
        )
        _require_comparable_benchmark_metadata(
            soac_suite,
            reference_benchmark_metadata,
            label=str(soac_path),
        )
        stock_elapsed = _benchmark_elapsed(stock_suite)
        soac_elapsed = _benchmark_elapsed(soac_suite)
        stock_drivers = _benchmark_drivers(stock_suite)
        soac_drivers = _benchmark_drivers(soac_suite)
        if requested_drivers is not None:
            _require_benchmarks(
                dict.fromkeys(stock_drivers.values(), 0.0),
                requested_drivers,
                label=str(stock_path),
            )
            _require_benchmarks(
                dict.fromkeys(soac_drivers.values(), 0.0),
                requested_drivers,
                label=str(soac_path),
            )
        if expected is None:
            expected = set(stock_elapsed)
            if not expected:
                raise ValueError(f"stock result contains no benchmarks: {stock_path}")
            expected_drivers = stock_drivers
        _require_benchmarks(stock_elapsed, expected, label=str(stock_path))
        _require_benchmarks(soac_elapsed, expected, label=str(soac_path))
        assert expected_drivers is not None
        _require_driver_attribution(
            stock_drivers, expected_drivers, label=str(stock_path)
        )
        _require_driver_attribution(
            soac_drivers, expected_drivers, label=str(soac_path)
        )
        for name in expected:
            stock_samples.setdefault(name, []).append(stock_elapsed[name])
            soac_samples.setdefault(name, []).append(soac_elapsed[name])
            speedup_samples.setdefault(name, []).append(
                stock_elapsed[name] / soac_elapsed[name]
            )

    assert expected is not None
    assert expected_drivers is not None
    assert reference_metadata is not None
    assert reference_benchmark_metadata is not None
    _merge_suites(stock_paths, comparison_dir / "stock.json")
    _merge_suites(soac_paths, comparison_dir / "soac.json")
    baseline_elapsed = (
        _baseline_elapsed(
            baseline,
            expected,
            reference_metadata,
            reference_benchmark_metadata,
        )
        if baseline is not None
        else None
    )

    benchmark_results: dict[str, dict[str, float]] = {}
    for name in sorted(expected):
        candidate_elapsed = statistics.median(soac_samples[name])
        benchmark = {
            "stock_elapsed_s": statistics.median(stock_samples[name]),
            "soac_elapsed_s": candidate_elapsed,
            "speedup_vs_stock": statistics.median(speedup_samples[name]),
        }
        if baseline_elapsed is not None:
            benchmark["baseline_soac_elapsed_s"] = baseline_elapsed[name]
            benchmark["speedup_vs_baseline_soac"] = (
                baseline_elapsed[name] / candidate_elapsed
            )
        benchmark_results[name] = benchmark

    summary: dict[str, Any] = {
        "comparison_dir": str(comparison_dir),
        "round_count": len(stock_paths),
        "requested_driver_count": len(
            requested_drivers
            if requested_drivers is not None
            else set(expected_drivers.values())
        ),
        "benchmark_count": len(expected),
        "complete": True,
        "benchmarks": benchmark_results,
        "geometric_mean_speedup_vs_stock": statistics.geometric_mean(
            benchmark["speedup_vs_stock"] for benchmark in benchmark_results.values()
        ),
        "transformation": _transformation_coverage(soac_paths),
    }
    if baseline_elapsed is not None:
        summary["baseline"] = str(baseline)
        summary["geometric_mean_speedup_vs_baseline_soac"] = statistics.geometric_mean(
            benchmark["speedup_vs_baseline_soac"]
            for benchmark in benchmark_results.values()
        )
    return summary


def render_summary(summary: dict[str, Any]) -> str:
    lines = [
        f"pyperformance comparison: {summary['comparison_dir']}",
        f"independent paired rounds: {summary['round_count']}",
        (
            "benchmark driver coverage: "
            f"{summary['requested_driver_count']}/{summary['requested_driver_count']} complete"
        ),
        f"benchmark coverage: {summary['benchmark_count']}/{summary['benchmark_count']} complete",
    ]
    for name, benchmark in summary["benchmarks"].items():
        line = (
            f"  {name}: stock={benchmark['stock_elapsed_s']:.6g}s "
            f"soac={benchmark['soac_elapsed_s']:.6g}s "
            f"paired stock/soac={benchmark['speedup_vs_stock']:.3f}x"
        )
        if "speedup_vs_baseline_soac" in benchmark:
            line += f" previous-soac/current-soac={benchmark['speedup_vs_baseline_soac']:.3f}x"
        lines.append(line)
        coverage = summary["transformation"]["benchmark_coverage"].get(name)
        if coverage is not None:
            lines.append(
                "    JIT coverage: "
                f"{len(coverage['compiled_functions'])} functions; "
                f"project={', '.join(coverage['project_modules']) or 'none'}; "
                f"stdlib={', '.join(coverage['stdlib_modules']) or 'none'}"
            )
        else:
            lines.append("    JIT coverage: no exact benchmark attribution recorded")
    lines.append(
        f"geometric mean stock/SOAC: {summary['geometric_mean_speedup_vs_stock']:.3f}x"
    )
    if "geometric_mean_speedup_vs_baseline_soac" in summary:
        lines.append(
            "geometric mean previous-SOAC/current-SOAC: "
            f"{summary['geometric_mean_speedup_vs_baseline_soac']:.3f}x"
        )

    coverage = summary["transformation"]
    lines.extend(
        [
            "transformed project/dependency modules: "
            + (", ".join(coverage["project_modules"]) or "none recorded"),
            "transformed standard-library modules: "
            + (", ".join(coverage["stdlib_modules"]) or "none recorded"),
            f"compiled function coverage: {len(coverage['compiled_functions'])}",
            (
                "pre-optimization BlockPy cache: "
                f"{coverage['preoptimization_blockpy_bytes']} bytes "
                "(median per round)"
            ),
            (
                "optimized typed IR: "
                f"{coverage['optimized_typed_ir_block_count']} final basic blocks "
                f"across {coverage['optimized_typed_ir_function_count']} functions "
                "(median per round)"
            ),
            (
                f"emitted native code: {coverage['native_code_bytes']} bytes "
                "(median per round)"
            ),
            (
                f"emitted machine blocks: {coverage['native_machine_blocks']} "
                "(median per round)"
            ),
        ]
    )
    return "\n".join(lines)


def main() -> int:
    args = parse_args()
    if args.preflight_baseline is not None:
        try:
            preflight_baseline(args.preflight_baseline)
        except (OSError, ValueError) as error:
            raise SystemExit(f"pyperformance baseline preflight failed: {error}") from error
        return 0
    if args.comparison_dir is None:
        raise SystemExit("pyperformance comparison directory is required")
    try:
        summary = summarize_comparison(args.comparison_dir, baseline=args.baseline)
    except (OSError, ValueError) as error:
        raise SystemExit(f"pyperformance comparison failed: {error}") from error
    if args.json_out is not None:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(render_summary(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
