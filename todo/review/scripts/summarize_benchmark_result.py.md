# scripts/summarize_benchmark_result.py

## File Responsibilities

Summarizes a SOAC benchmark result directory. It parses benchmark throughput sections, specialization counters, optional JIT code-size maps, and prints a compact human-readable summary.

## Datatypes

- `RAW_LOOPS_PER_S_RE`: regex for raw benchmark loop-rate lines.
- No classes are defined.

## Functions

- `parse_args`: accepts the benchmark result directory path.
- `parse_benchmark_report`: parses `benchmark.txt` into structured timing/throughput fields for profile, verify, apply, and stock runs.
- `add_run_stats`: computes median/min/max and derived fields for one parsed run family.
- `parse_jit_code_size`: summarizes JIT function/body size from `jit_bb_map.jsonl` when present.
- `format_summary`: formats headline throughput, deltas, code-size totals, and counter summaries; nested `maybe` renders absent values.
- `main`: loads available result artifacts and prints the formatted summary.

## Context Read

- `scripts/pystone.py`
- `scripts/summarize_module_load_log.py`
- benchmark result layout produced by `just benchmark`

