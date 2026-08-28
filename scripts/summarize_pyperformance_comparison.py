from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import signal
import statistics
import subprocess
from contextlib import suppress
from pathlib import Path
from typing import Any

import pyperf
import pyperformance
from pyperf._collect_metadata import collect_python_metadata

_DRIVER_METADATA = "soac_pyperformance_driver"
_LANGUAGE_METADATA = "soac_pyperformance_language"
_STOCK_SOURCE_METADATA = "soac_pyperformance_stock_source_fingerprint"
_STRICT_SOURCE_METADATA = "soac_pyperformance_strict_source_fingerprint"
_LOCAL_PACKAGES_METADATA = "soac_pyperformance_local_packages_fingerprint"
_STRICT_POLICY_METADATA = (
    "soac_pyperformance_selection_policy",
    "soac_pyperformance_harness_policy",
)


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
    parser.add_argument("--run-rounds", type=int)
    parser.add_argument("--benchmarks", default="all")
    parser.add_argument("--pyperformance-args", nargs=argparse.REMAINDER, default=[])
    return parser.parse_args()


def _write_json(path: Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def _requested_drivers(comparison_dir: Path) -> list[str]:
    path = comparison_dir / "requested-benchmarks.txt"
    names = [
        line.removeprefix("- ").strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.startswith("- ")
    ]
    if not names or any(not name for name in names):
        raise ValueError(f"requested benchmark list is empty: {path}")
    if len(names) != len(set(names)):
        raise ValueError(f"requested benchmark list contains duplicate drivers: {path}")
    return sorted(names)


def _comparison_plan(
    rounds: int,
    drivers: list[str],
    *,
    selector: str,
    extra_args: list[str],
    baseline: str | None,
) -> dict[str, Any]:
    if type(rounds) is not int or rounds < 1:
        raise ValueError("requested comparison rounds must be a positive integer")
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
        "extra_args": extra_args,
        "baseline": baseline,
        "runs": runs,
    }


def _load_comparison_plan(comparison_dir: Path) -> dict[str, Any] | None:
    path = comparison_dir / "comparison-plan.json"
    if not path.is_file():
        return None
    plan = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(plan, dict) or plan.get("schema") != 1:
        raise ValueError(f"invalid comparison plan: {path}")
    drivers = plan.get("requested_drivers")
    extra_args = plan.get("extra_args")
    if (
        not isinstance(drivers, list)
        or not drivers
        or any(not isinstance(name, str) or not name for name in drivers)
        or len(drivers) != len(set(drivers))
        or not isinstance(extra_args, list)
        or any(not isinstance(argument, str) for argument in extra_args)
        or not isinstance(plan.get("benchmark_selector"), str)
        or not plan["benchmark_selector"]
        or (plan.get("baseline") is not None and not isinstance(plan["baseline"], str))
    ):
        raise ValueError(f"invalid requested comparison inputs: {path}")
    expected = _comparison_plan(
        plan.get("requested_rounds"),
        drivers,
        selector=plan["benchmark_selector"],
        extra_args=extra_args,
        baseline=plan.get("baseline"),
    )
    if plan != expected:
        raise ValueError(
            f"comparison plan has inconsistent rounds/order/outputs: {path}"
        )
    if _requested_drivers(comparison_dir) != drivers:
        raise ValueError("requested benchmark list changed after the comparison plan")
    return plan


def _run_logged(command: list[str], environment: dict[str, str], log_path: Path) -> int:
    with log_path.open("w", encoding="utf-8") as log:
        process = subprocess.Popen(
            command,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            start_new_session=True,
        )
        assert process.stdout is not None
        try:
            for line in process.stdout:
                log.write(line)
                log.flush()
                print(line, end="", flush=True)
            return process.wait()
        except BaseException:
            with suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                with suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            raise
        finally:
            process.stdout.close()


def run_comparison_rounds(
    comparison_dir: Path,
    *,
    rounds: int,
    benchmarks: str,
    extra_args: list[str],
    baseline: Path | None = None,
    run=_run_logged,
) -> dict[str, Any]:
    """Run the fixed request to completion; a failed phase never narrows it."""
    comparison_dir = comparison_dir.absolute()
    plan_path = comparison_dir / "comparison-plan.json"
    status_path = comparison_dir / "run-status.json"
    if (
        plan_path.exists()
        or status_path.exists()
        or any(comparison_dir.glob("round-*.json"))
    ):
        raise ValueError(f"comparison plan or results already exists: {comparison_dir}")
    for argument in extra_args:
        option = argument.partition("=")[0]
        owned_options = {"--benchmarks", "--output", "--append", "--python"}
        if any(
            owned.startswith(option)
            for owned in owned_options
            if option.startswith("--")
        ) or (option.startswith(("-b", "-o", "-p")) and not option.startswith("--")):
            raise ValueError(
                f"comparison owns driver selection, interpreter, and output: {argument}"
            )
    plan = _comparison_plan(
        rounds,
        _requested_drivers(comparison_dir),
        selector=benchmarks,
        extra_args=extra_args,
        baseline=str(baseline.absolute()) if baseline is not None else None,
    )
    plan_bytes = (json.dumps(plan, indent=2) + "\n").encode()
    with plan_path.open("xb") as handle:
        handle.write(plan_bytes)
    status = {
        "schema": 1,
        "plan_sha256": hashlib.sha256(plan_bytes).hexdigest(),
        "complete": False,
        "runs": [
            {
                **entry,
                "status": "not_run",
                "exit_code": None,
                "log": str(comparison_dir / (Path(entry["output"]).stem + ".log")),
            }
            for entry in plan["runs"]
        ],
    }
    order = ["round\torder\tmode\toutput"]
    order.extend(
        f"{entry['round']}\t{entry['order']}\t{entry['mode']}\t{entry['output']}"
        for entry in plan["runs"]
    )
    (comparison_dir / "run-order.tsv").write_text("\n".join(order) + "\n")
    _write_json(status_path, status)
    environment = os.environ.copy()
    environment.pop("SOAC_WORK_DIR", None)
    environment.pop("SOAC_OPT_MODE", None)
    selector = "" if benchmarks in {"all", "full"} else benchmarks
    for entry in status["runs"]:
        if plan_path.read_bytes() != plan_bytes:
            raise ValueError("comparison plan changed during execution")
        entry["status"] = "running"
        _write_json(status_path, status)
        command = [
            "just",
            "pyperformance",
            entry["mode"],
            str(comparison_dir / entry["output"]),
            selector,
            *extra_args,
        ]
        try:
            entry["exit_code"] = run(command, environment.copy(), Path(entry["log"]))
        except OSError as error:
            entry["exit_code"] = 1
            entry["error"] = f"{type(error).__name__}: {error}"
        except KeyboardInterrupt:
            entry.update(status="interrupted", exit_code=130)
            status["interrupted"] = True
            _write_json(status_path, status)
            return status
        entry["status"] = "succeeded" if entry["exit_code"] == 0 else "failed"
        _write_json(status_path, status)
        if plan_path.read_bytes() != plan_bytes:
            raise ValueError("comparison plan changed during execution")
    status["complete"] = all(entry["status"] == "succeeded" for entry in status["runs"])
    _write_json(status_path, status)
    return status


def _load_suite(path: Path) -> pyperf.BenchmarkSuite:
    if not path.is_file():
        raise ValueError(f"pyperformance result does not exist: {path}")
    try:
        return pyperf.BenchmarkSuite.load(str(path))
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError(f"invalid pyperformance result {path}: {error}") from error


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
            actual = benchmark.get_metadata()
            keys = [_STOCK_SOURCE_METADATA, _LOCAL_PACKAGES_METADATA]
            if (
                metadata.get(_LANGUAGE_METADATA)
                == actual.get(_LANGUAGE_METADATA)
                == "strict"
            ):
                keys.extend([_STRICT_SOURCE_METADATA, *_STRICT_POLICY_METADATA])
            for key in keys:
                if actual.get(key) != metadata.get(key):
                    raise ValueError(
                        f"{label} benchmark {benchmark.get_name()} has incompatible {key}"
                    )


def _require_language_metadata(suite, language: str, *, label: str) -> None:
    for benchmark in suite.get_benchmarks():
        metadata = benchmark.get_metadata()
        name = benchmark.get_name()
        if metadata.get(_LANGUAGE_METADATA) != language:
            raise ValueError(
                f"{label} benchmark {name} does not record {language} Python execution"
            )
        digests = [_STOCK_SOURCE_METADATA]
        if _LOCAL_PACKAGES_METADATA in metadata:
            digests.append(_LOCAL_PACKAGES_METADATA)
        if language == "strict":
            digests.append(_STRICT_SOURCE_METADATA)
            for key in _STRICT_POLICY_METADATA:
                if not isinstance(metadata.get(key), str) or not metadata[key]:
                    raise ValueError(f"{label} benchmark {name} has no {key}")
        for key in digests:
            if not isinstance(metadata.get(key), str) or not re.fullmatch(
                r"[0-9a-f]{64}", metadata[key]
            ):
                raise ValueError(f"{label} benchmark {name} has no valid {key}")


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
    _require_language_metadata(suite, "strict", label=f"baseline {baseline}")


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


def _has_sealed_worker_evidence(row, metadata) -> bool:
    """Check measurement provenance, without treating it as runtime authority."""
    if row.get("language") != "strict" or metadata is None:
        return False
    for row_key, metadata_key in (
        ("stock_source_fingerprint", _STOCK_SOURCE_METADATA),
        ("strict_source_fingerprint", _STRICT_SOURCE_METADATA),
        ("selection_policy", _STRICT_POLICY_METADATA[0]),
        ("harness_policy", _STRICT_POLICY_METADATA[1]),
    ):
        if row.get(row_key) != metadata.get(metadata_key):
            return False
    states = row.get("sealed_strict_modules")
    if not isinstance(states, list) or not states:
        return False
    names = set()
    for state in states:
        if (
            not isinstance(state, dict)
            or state.get("schema") != 2
            or state.get("ready") is not True
            or state.get("strict_assign") is not True
            or state.get("checked_attr") is not True
            or state.get("sealed") is not True
            or not isinstance(state.get("module_name"), str)
            or state["module_name"] in names
            or state.get("artifact_generation") != row.get("artifact_generation")
            or state.get("source_kind") not in {"project", "python-stdlib"}
            or not isinstance(state.get("source_path"), str)
            or not isinstance(state.get("interpreter_id"), int)
        ):
            return False
        for key in ("source_sha256", "artifact_generation", "startup_identity"):
            if not isinstance(state.get(key), str) or not re.fullmatch(
                r"[0-9a-f]{64}", state[key]
            ):
                return False
        names.add(state["module_name"])
    return "__main__" in names


def _transformation_coverage(soac_paths: list[Path]) -> dict[str, Any]:
    project_modules: set[str] = set()
    stdlib_modules: set[str] = set()
    compiled_functions: set[str] = set()
    per_benchmark: dict[str, dict[str, Any]] = {}
    round_metrics: list[dict[str, int]] = []
    sealed_round_benchmarks: list[set[str]] = []

    for soac_path in soac_paths:
        this_round: set[str] = set()
        sealed_round_benchmarks.append(this_round)
        work_root = Path(f"{soac_path.with_suffix('')}.soac-work")
        if not work_root.is_dir():
            continue
        expected_metadata = {
            benchmark.get_name(): benchmark.get_metadata()
            for benchmark in _load_suite(soac_path).get_benchmarks()
        }

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
                metrics["preoptimization_stdlib_blockpy_bytes"] += (
                    module_path.stat().st_size
                )
            elif source_kind == "project":
                metrics["preoptimization_project_blockpy_bytes"] += (
                    module_path.stat().st_size
                )

        for timing_path in work_root.rglob("pyperformance-worker-timing.jsonl"):
            worker_dir = timing_path.parent
            summary_path = worker_dir / "jit-code-summary.jsonl"
            apply_rows = [
                row
                for row in _read_jsonl(timing_path)
                if row.get("record_type") == "pyperformance_worker_timing_v1"
                and row.get("opt_mode") == "apply"
                and isinstance(row.get("pid"), int)
                and isinstance(row.get("measured_batches"), int)
                and row["measured_batches"] > 0
                and _has_sealed_worker_evidence(
                    row, expected_metadata.get(row.get("pyperf_benchmark_name"))
                )
            ]
            apply_pids = {row["pid"] for row in apply_rows}
            if not apply_pids:
                continue

            apply_function_ids: set[str] = set()
            worker_functions: set[str] = set()
            for row in _read_jsonl(summary_path) if summary_path.is_file() else ():
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

            worker_project_modules = {
                state["module_name"]
                for row in apply_rows
                for state in row["sealed_strict_modules"]
                if state["source_kind"] == "project"
            }
            worker_stdlib_modules = {
                state["module_name"]
                for row in apply_rows
                for state in row["sealed_strict_modules"]
                if state["source_kind"] == "python-stdlib"
            }
            project_modules.update(worker_project_modules)
            stdlib_modules.update(worker_stdlib_modules)
            worker_benchmarks = {
                benchmark_name
                for row in apply_rows
                if isinstance((benchmark_name := row.get("pyperf_benchmark_name")), str)
                and benchmark_name
            }
            this_round.update(worker_benchmarks)
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
                    and event.get("process_id") in apply_pids
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
        "sealed_round_benchmarks": [sorted(names) for names in sealed_round_benchmarks],
        "module_coverage_evidence": "native seal snapshots from measured strict workers",
        "compiled_functions_are_execution_proof": False,
        "hot_path_execution": "not established by compilation or seal snapshots; inspect representative measured-worker profiles",
        "preoptimization_size_available": any(
            metrics["preoptimization_project_blockpy_bytes"]
            or metrics["preoptimization_stdlib_blockpy_bytes"]
            for metrics in round_metrics
        ),
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
    *,
    selected_results: bool = False,
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
        if selected_results:
            # Only partial diagnostics project individual results. Complete
            # comparisons retain the exact full-set validation below.
            missing = expected.difference(suite.get_benchmark_names())
            if missing:
                raise ValueError(
                    f"baseline {path} is missing results: {sorted(missing)}"
                )
            suite = pyperf.BenchmarkSuite(
                [suite.get_benchmark(name) for name in sorted(expected)]
            )
        _require_language_metadata(suite, "strict", label=f"baseline {path}")
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
        _require_driver_attribution(
            _benchmark_drivers(suite),
            {
                name: expected_benchmark_metadata[name].get(_DRIVER_METADATA, name)
                for name in expected
            },
            label=f"baseline {path}",
        )
        for name, value in elapsed.items():
            per_benchmark[name].append(value)
    return {name: statistics.median(values) for name, values in per_benchmark.items()}


def _read_object(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise TypeError(f"expected an object in {path}")
    return value


def _phase_evidence(
    comparison_dir: Path,
    entry: dict[str, Any],
    phase: dict[str, Any],
    requested: list[str],
) -> tuple[dict[str, Any], list[str], list[dict[str, Any]]]:
    output = comparison_dir / phase["output"]
    status_path = Path(str(output) + ".status.json")
    identity = {"round": entry["round"], "mode": entry["mode"], "phase": phase["name"]}
    evidence: dict[str, Any] = {
        **identity,
        "output": str(output),
        "status_path": str(status_path),
        "results": [],
        "exit_code": None,
        "complete": False,
    }
    failures = []
    records = {}
    try:
        status = _read_object(status_path)
        evidence["exit_code"] = status.get("exit_code")
        language = "ordinary" if phase["name"] == "stock" else "strict"
        mode = None if language == "ordinary" else phase["name"]
        if (
            status.get("schema") != 1
            or status.get("output") != str(output)
            or status.get("language") != language
            or status.get("optimization_mode") != mode
            or status.get("requested_drivers") != requested
        ):
            raise ValueError(
                f"phase request/identity differs from comparison plan: {status_path}"
            )
        rows = status.get("records")
        if not isinstance(rows, list):
            raise TypeError(f"missing driver outcomes: {status_path}")
        for row in rows:
            if (
                not isinstance(row, dict)
                or row.get("benchmark") not in requested
                or row["benchmark"] in records
            ):
                raise ValueError(f"invalid or repeated driver outcome: {status_path}")
            records[row["benchmark"]] = row
        if status.get("exit_code") != 0 or status.get("complete") is not True:
            failures.append(f"{phase['output']} phase did not complete successfully")
        if status.get("error"):
            evidence["error"] = status["error"]
    except (OSError, ValueError, TypeError) as error:
        failures.append(str(error))

    emitted: dict[str, list[str]] = {}
    try:
        suite = _load_suite(output)
        language = "ordinary" if phase["name"] == "stock" else "strict"
        _require_language_metadata(suite, language, label=str(output))
        attribution = _benchmark_drivers(suite)
        for name, driver in attribution.items():
            emitted.setdefault(driver, []).append(name)
        evidence["results"] = [
            {"name": name, "driver": attribution[name], "elapsed_s": elapsed}
            for name, elapsed in sorted(_benchmark_elapsed(suite).items())
        ]
        _require_benchmarks(
            dict.fromkeys(emitted, 0.0), set(requested), label=str(output)
        )
    except (OSError, ValueError, TypeError) as error:
        failures.append(str(error))

    driver_failures = []
    outcomes = []
    for name in requested:
        row = records.get(name)
        if row is None:
            row = {
                "benchmark": name,
                "status": "not_run",
                "stage": "unknown",
                "error": "no durable driver outcome",
                "emitted_results": [],
            }
        else:
            row = dict(row)
        names = row.get("emitted_results")
        valid_results = (
            isinstance(names, list)
            and bool(names)
            and all(isinstance(value, str) for value in names)
            and len(names) == len(set(names))
            and set(names) == set(emitted.get(name, []))
        )
        if (
            row.get("status") != "succeeded"
            or row.get("stage") != "complete"
            or not valid_results
        ):
            if row.get("status") == "succeeded":
                row.update(
                    status="failed",
                    error="successful driver outcome does not match measured results",
                )
            driver_failures.append({**identity, **row})
        outcomes.append(row)
    evidence["driver_outcomes"] = outcomes
    if driver_failures:
        failures.append(
            f"{phase['output']} has {len(driver_failures)}/{len(requested)} failed or missing drivers"
        )
    evidence["complete"] = not failures
    return evidence, failures, driver_failures


def _execution_evidence(comparison_dir: Path, plan: dict[str, Any]) -> dict[str, Any]:
    failures = []
    expected_paths = {
        phase["output"] for entry in plan["runs"] for phase in entry["phases"]
    }
    actual_paths = {
        path.name
        for path in comparison_dir.glob("round-*.json")
        if not path.name.endswith(".status.json")
    }
    missing = sorted(expected_paths - actual_paths)
    unexpected = sorted(actual_paths - expected_paths)
    if missing:
        failures.append("requested round/phase results missing: " + ", ".join(missing))
    if unexpected:
        failures.append("unexpected round results: " + ", ".join(unexpected))
    runs = []
    execution_status_valid = False
    try:
        status_path = comparison_dir / "run-status.json"
        status = _read_object(status_path)
        digest = hashlib.sha256(
            (comparison_dir / "comparison-plan.json").read_bytes()
        ).hexdigest()
        if status.get("schema") != 1 or status.get("plan_sha256") != digest:
            raise ValueError(
                "comparison execution status does not match the frozen plan"
            )
        if status.get("complete") is not True:
            failures.append("comparison execution has no successful terminal status")
        runs = status.get("runs")
        if not isinstance(runs, list) or len(runs) != len(plan["runs"]):
            raise ValueError("comparison execution status has missing requested runs")
        for expected, actual in zip(plan["runs"], runs, strict=True):
            if not isinstance(actual, dict) or any(
                actual.get(key) != value for key, value in expected.items()
            ):
                raise ValueError(
                    "comparison execution status changed round order or identity"
                )
            if actual.get("status") != "succeeded" or actual.get("exit_code") != 0:
                failures.append(
                    f"round {expected['round']} {expected['mode']} status="
                    f"{actual.get('status')} exit={actual.get('exit_code')}"
                )
        execution_status_valid = all(
            type(row.get("exit_code")) is int
            and row.get("status") in {"succeeded", "failed", "interrupted"}
            for row in runs
        )
    except (OSError, ValueError, TypeError) as error:
        failures.append(str(error))
    phases = []
    driver_failures = []
    for entry in plan["runs"]:
        for phase in entry["phases"]:
            evidence, problems, drivers = _phase_evidence(
                comparison_dir, entry, phase, plan["requested_drivers"]
            )
            phases.append(evidence)
            failures.extend(problems)
            driver_failures.extend(drivers)
    return {
        "runs": runs,
        "phases": phases,
        "failures": failures,
        "driver_failures": driver_failures,
        "execution_status_valid": execution_status_valid,
    }


def _paired_benchmark_results(
    stock_samples, soac_samples, speedup_samples, baseline_elapsed=None
) -> dict[str, dict[str, float]]:
    benchmark_results: dict[str, dict[str, float]] = {}
    for name in sorted(stock_samples):
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
    return benchmark_results


def _partial_result_evidence(
    comparison_dir: Path, plan: dict[str, Any], execution: dict[str, Any]
) -> dict[str, Any]:
    """Select full-round pairs, then reuse the complete result/coverage checks."""
    partial: dict[str, Any] = {
        "scope": "partial diagnostics only; no suite-wide aggregate or acceptance claim",
        "requested_rounds": plan["requested_rounds"],
        "benchmarks": {},
        "available_apply_rounds": [],
        "transformation": None,
        "issues": [],
    }
    loaded = {}
    outcomes = {}
    available_apply_paths = []
    for phase in execution["phases"]:
        key = (phase["round"], phase["phase"])
        outcomes[key] = (
            {row["benchmark"]: row for row in phase["driver_outcomes"]}
            if type(phase["exit_code"]) is int
            else {}
        )
        try:
            path = Path(phase["output"])
            loaded[key] = _load_suite(path)
            if phase["phase"] == "apply":
                partial["available_apply_rounds"].append(phase["round"])
                available_apply_paths.append(path)
        except (OSError, ValueError, TypeError) as error:
            partial["issues"].append(str(error))

    if available_apply_paths:
        try:
            partial["transformation"] = _transformation_coverage(available_apply_paths)
        except (OSError, ValueError, TypeError) as error:
            partial["issues"].append(f"partial transformation evidence: {error}")
    if not execution.get("execution_status_valid"):
        partial["issues"].append(
            "paired results require a matching terminal execution journal"
        )
        return partial
    keys = [
        (number, phase)
        for number in range(1, plan["requested_rounds"] + 1)
        for phase in ("stock", "apply")
    ]
    if any(key not in loaded for key in keys):
        partial["issues"].append("no result can be paired across every requested round")
        return partial
    names = set.intersection(*(set(loaded[key].get_benchmark_names()) for key in keys))
    for name in sorted(names):
        stock_reference = loaded[(1, "stock")].get_benchmark(name).get_metadata()
        strict_reference = loaded[(1, "apply")].get_benchmark(name).get_metadata()
        driver = stock_reference.get(_DRIVER_METADATA, name)
        stock_values = []
        apply_values = []
        profile_complete = True
        try:
            if driver not in plan["requested_drivers"]:
                raise ValueError(f"{name} belongs to an unrequested driver: {driver}")
            for number in range(1, plan["requested_rounds"] + 1):
                for phase in ("stock", "profile", "apply"):
                    key = (number, phase)
                    row = outcomes.get(key, {}).get(driver, {})
                    succeeded = row.get("status") == "succeeded" and name in row.get(
                        "emitted_results", []
                    )
                    suite = loaded.get(key)
                    if phase == "profile":
                        profile_complete = profile_complete and succeeded
                        if suite is None or name not in suite.get_benchmark_names():
                            continue
                    elif not succeeded:
                        raise ValueError(
                            f"{name}: round {number} {phase} has no successful driver outcome"
                        )
                    measured = suite.get_benchmark(name)
                    selected = pyperf.BenchmarkSuite([measured])
                    label = f"{name} round {number} {phase}"
                    _require_language_metadata(
                        selected,
                        "ordinary" if phase == "stock" else "strict",
                        label=label,
                    )
                    _require_driver_attribution(
                        _benchmark_drivers(selected), {name: driver}, label=label
                    )
                    _require_comparable_metadata(measured, stock_reference, label=label)
                    _require_comparable_benchmark_metadata(
                        selected, {name: stock_reference}, label=label
                    )
                    if phase != "stock":
                        _require_comparable_benchmark_metadata(
                            selected, {name: strict_reference}, label=label
                        )
                    if phase == "stock":
                        stock_values.append(measured.mean())
                    elif phase == "apply":
                        apply_values.append(measured.mean())
        except (OSError, ValueError, TypeError) as error:
            partial["issues"].append(str(error))
            continue
        baseline_values = None
        baseline_error = None
        if plan["baseline"] is not None:
            try:
                baseline_values = _baseline_elapsed(
                    Path(plan["baseline"]),
                    {name},
                    stock_reference,
                    {name: strict_reference},
                    selected_results=True,
                )
            except (OSError, ValueError, TypeError) as error:
                baseline_error = str(error)
        result = _paired_benchmark_results(
            {name: stock_values},
            {name: apply_values},
            {
                name: [
                    stock / apply
                    for stock, apply in zip(stock_values, apply_values, strict=True)
                ]
            },
            baseline_values,
        )[name]
        result.update(
            driver=driver,
            paired_round_count=plan["requested_rounds"],
            profile_complete=profile_complete,
            source_selection={
                "stock_source_fingerprint": strict_reference[_STOCK_SOURCE_METADATA],
                "strict_source_fingerprint": strict_reference[_STRICT_SOURCE_METADATA],
                "selection_policy": strict_reference[_STRICT_POLICY_METADATA[0]],
                "harness_policy": strict_reference[_STRICT_POLICY_METADATA[1]],
            },
        )
        if baseline_error is not None:
            result["baseline_error"] = baseline_error
        partial["benchmarks"][name] = result
    return partial


def build_comparison_report(
    comparison_dir: Path,
    *,
    baseline: Path | None = None,
    execution_error: str | None = None,
) -> dict[str, Any]:
    """Persist negative evidence too; never average an intersection of the request."""
    comparison_dir = comparison_dir.absolute()
    report: dict[str, Any] = {
        "comparison_dir": str(comparison_dir),
        "complete": False,
        "requested_drivers": [],
        "requested_driver_count": 0,
        "requested_rounds": None,
        "failures": [],
        "driver_failures": [],
        "runs": [],
        "phases": [],
    }
    plan = None
    try:
        report["requested_drivers"] = _requested_drivers(comparison_dir)
        report["requested_driver_count"] = len(report["requested_drivers"])
        plan = _load_comparison_plan(comparison_dir)
        if plan is None:
            raise ValueError(
                "comparison plan is missing; the original requested round count is unknown"
            )
        report["requested_rounds"] = plan["requested_rounds"]
        report.update(_execution_evidence(comparison_dir, plan))
        if not report["failures"] and execution_error is None:
            report.update(summarize_comparison(comparison_dir, baseline=baseline))
    except (OSError, ValueError, TypeError) as error:
        report["failures"].append(str(error))
    if execution_error is not None:
        report["failures"].append(execution_error)
    if not report["complete"] and plan is not None:
        report["partial_evidence"] = _partial_result_evidence(
            comparison_dir, plan, report
        )
    return report


def summarize_comparison(
    comparison_dir: Path, *, baseline: Path | None = None
) -> dict[str, Any]:
    comparison_dir = comparison_dir.absolute()
    plan = _load_comparison_plan(comparison_dir)
    if plan is not None:
        evidence = _execution_evidence(comparison_dir, plan)
        if evidence["failures"]:
            raise ValueError("; ".join(evidence["failures"]))
        planned_baseline = plan["baseline"]
        if baseline is not None and str(baseline.absolute()) != planned_baseline:
            raise ValueError("baseline differs from the frozen comparison plan")
        baseline = Path(planned_baseline) if planned_baseline is not None else None
    stock_paths = sorted(comparison_dir.glob("round-*-stock.json"))
    if not stock_paths:
        raise ValueError(f"no stock pyperformance rounds found in {comparison_dir}")

    soac_paths = []
    stock_samples: dict[str, list[float]] = {}
    soac_samples: dict[str, list[float]] = {}
    speedup_samples: dict[str, list[float]] = {}
    reference_metadata: dict[str, Any] | None = None
    reference_benchmark_metadata: dict[str, dict[str, Any]] | None = None
    reference_strict_metadata: dict[str, dict[str, Any]] | None = None
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
        _require_language_metadata(stock_suite, "ordinary", label=str(stock_path))
        _require_language_metadata(soac_suite, "strict", label=str(soac_path))
        if reference_metadata is None:
            reference_metadata = stock_suite.get_metadata()
            reference_benchmark_metadata = {
                benchmark.get_name(): benchmark.get_metadata()
                for benchmark in stock_suite.get_benchmarks()
            }
            reference_strict_metadata = {
                benchmark.get_name(): benchmark.get_metadata()
                for benchmark in soac_suite.get_benchmarks()
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
        assert reference_benchmark_metadata is not None
        assert reference_strict_metadata is not None
        _require_comparable_benchmark_metadata(
            stock_suite, reference_benchmark_metadata, label=str(stock_path)
        )
        _require_comparable_benchmark_metadata(
            soac_suite, reference_benchmark_metadata, label=str(soac_path)
        )
        _require_comparable_benchmark_metadata(
            soac_suite, reference_strict_metadata, label=str(soac_path)
        )
        if plan is not None:
            profile_path = soac_path.with_name(soac_path.stem + ".profile.json")
            profile_suite = _load_suite(profile_path)
            _require_benchmarks(
                _benchmark_elapsed(profile_suite), expected, label=str(profile_path)
            )
            _require_driver_attribution(
                _benchmark_drivers(profile_suite),
                expected_drivers,
                label=str(profile_path),
            )
            _require_comparable_metadata(
                profile_suite, reference_metadata, label=str(profile_path)
            )
            _require_comparable_benchmark_metadata(
                profile_suite, reference_strict_metadata, label=str(profile_path)
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
    assert reference_strict_metadata is not None
    baseline_elapsed = (
        _baseline_elapsed(
            baseline,
            expected,
            reference_metadata,
            reference_strict_metadata,
        )
        if baseline is not None
        else None
    )

    benchmark_results = _paired_benchmark_results(
        stock_samples, soac_samples, speedup_samples, baseline_elapsed
    )

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
        "stock_language": "ordinary",
        "candidate_language": "strict",
        "source_selection": {
            name: {
                "stock_source_fingerprint": metadata[_STOCK_SOURCE_METADATA],
                "strict_source_fingerprint": metadata[_STRICT_SOURCE_METADATA],
                "selection_policy": metadata[_STRICT_POLICY_METADATA[0]],
                "harness_policy": metadata[_STRICT_POLICY_METADATA[1]],
            }
            for name, metadata in sorted(reference_strict_metadata.items())
        },
        "benchmarks": benchmark_results,
        "geometric_mean_speedup_vs_stock": statistics.geometric_mean(
            benchmark["speedup_vs_stock"] for benchmark in benchmark_results.values()
        ),
        "transformation": _transformation_coverage(soac_paths),
    }
    summary["sealed_strict_execution_evidence_complete"] = all(
        expected <= set(names)
        for names in summary["transformation"]["sealed_round_benchmarks"]
    )
    if baseline_elapsed is not None:
        summary["baseline"] = str(baseline)
        summary["geometric_mean_speedup_vs_baseline_soac"] = statistics.geometric_mean(
            benchmark["speedup_vs_baseline_soac"]
            for benchmark in benchmark_results.values()
        )
    _merge_suites(stock_paths, comparison_dir / "stock.json")
    _merge_suites(soac_paths, comparison_dir / "soac.json")
    return summary


def render_summary(summary: dict[str, Any]) -> str:
    if not summary["complete"]:
        lines = [
            f"pyperformance comparison: {summary['comparison_dir']}",
            "INCOMPLETE: no full-suite speedup or acceptance claim",
            f"requested drivers: {summary['requested_driver_count']}",
            f"requested paired rounds: {summary['requested_rounds']}",
            "successful phase results remain in their original per-round JSON files",
        ]
        partial = summary.get("partial_evidence")
        if partial is not None:
            lines.append(
                f"partial per-result diagnostics: {len(partial['benchmarks'])} results "
                f"with all {partial['requested_rounds']} requested stock/apply pairs"
            )
            for name, result in partial["benchmarks"].items():
                line = (
                    f"  {name}: stock={result['stock_elapsed_s']:.6g}s "
                    f"soac={result['soac_elapsed_s']:.6g}s "
                    f"paired stock/soac={result['speedup_vs_stock']:.3f}x"
                )
                if "speedup_vs_baseline_soac" in result:
                    line += f" previous-soac/current-soac={result['speedup_vs_baseline_soac']:.3f}x"
                if not result["profile_complete"]:
                    line += " (profile did not complete in every round)"
                lines.append(line)
            coverage = partial["transformation"]
            if coverage is not None:
                lines.extend(
                    [
                        "partial apply coverage rounds: "
                        + ", ".join(
                            str(number) for number in partial["available_apply_rounds"]
                        ),
                        (
                            f"partial compiled function inventory: {len(coverage['compiled_functions'])}; "
                            f"native bytes: {coverage['native_code_bytes']} (median over available work roots)"
                        ),
                        "partial sealed project modules: "
                        + (", ".join(coverage["project_modules"]) or "none"),
                        "partial sealed standard-library modules: "
                        + (", ".join(coverage["stdlib_modules"]) or "none"),
                        "compilation and seal snapshots are not hot-path execution proof",
                    ]
                )
        lines.extend(f"  {failure}" for failure in summary["failures"])
        for row in summary["driver_failures"]:
            lines.append(
                f"  round {row['round']} {row['phase']} {row['benchmark']}: "
                f"{row.get('stage')} / {row.get('error', row.get('status'))}"
            )
        return "\n".join(lines)
    lines = [
        f"pyperformance comparison: {summary['comparison_dir']}",
        f"independent paired rounds: {summary['round_count']}",
        "languages: original ordinary CPython / explicitly opted-in strict SOAC",
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
                "    sealed strict modules / compiled code (not hot-path execution proof): "
                f"{len(coverage['compiled_functions'])} functions; "
                f"project={', '.join(coverage['project_modules']) or 'none'}; "
                f"stdlib={', '.join(coverage['stdlib_modules']) or 'none'}"
            )
        else:
            lines.append(
                "    strict execution coverage: no matched native-seal measurement evidence"
            )
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
                + (
                    f"{coverage['preoptimization_blockpy_bytes']} bytes (median per round)"
                    if coverage["preoptimization_size_available"]
                    else "unavailable; strict source lowering does not reuse unsigned cache artifacts"
                )
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
        except (OSError, ValueError, TypeError) as error:
            raise SystemExit(
                f"pyperformance baseline preflight failed: {error}"
            ) from error
        return 0
    if args.comparison_dir is None:
        raise SystemExit("pyperformance comparison directory is required")
    execution_error = None
    interrupted = False
    if args.run_rounds is not None:
        try:
            status = run_comparison_rounds(
                args.comparison_dir,
                rounds=args.run_rounds,
                benchmarks=args.benchmarks,
                extra_args=args.pyperformance_args,
                baseline=args.baseline,
            )
            interrupted = status.get("interrupted", False)
        except (OSError, ValueError, TypeError) as error:
            execution_error = str(error)
        except KeyboardInterrupt:
            interrupted = True
            execution_error = "comparison interrupted before terminal status"
    summary = build_comparison_report(
        args.comparison_dir, baseline=args.baseline, execution_error=execution_error
    )
    json_out = args.json_out or args.comparison_dir / "summary.json"
    _write_json(json_out, summary)
    rendered = render_summary(summary)
    (args.comparison_dir / "summary.txt").write_text(rendered + "\n", encoding="utf-8")
    print(rendered)
    return 130 if interrupted else int(not summary["complete"])


if __name__ == "__main__":
    raise SystemExit(main())
