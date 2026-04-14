# scripts/summarize_module_load_log.py

## File Responsibilities

Reads SOAC tracing JSONL module-load/codegen events and reports aggregate timing, status counts, slowest modules/functions, and JIT counter totals.

## Datatypes

- `TimingStats`: count/total/mean/max timing aggregate plus max label.
- `CounterStats`: aggregate count/total/max for codegen counters.
- `JitMax`: identifies the module/function with the maximum value for one JIT counter.
- `LogSummary`: complete parsed summary for one log file.
- Module constants: default log path, event names, timing keys, and JIT codegen counter names.

## Functions

- `parse_args`: accepts an optional JSONL log path.
- `load_jsonl`: reads tracing JSONL entries, skipping blank lines.
- `numeric_timings`: extracts numeric `*_us` timing fields from one entry.
- `module_name`: returns a stable module label from an event.
- `function_name`: returns module/function/function-id labels for codegen events.
- `status`: extracts or defaults event status.
- `include_in_jit_counter_summary`: filters internal/runtime modules out of JIT counter totals.
- `timing_stats`: aggregates timing values and tracks the max label.
- `jit_counter_stats`: aggregates one JIT counter and tracks the max function.
- `summarize_entries`: classifies events, builds all timing/counter/status summaries, and records slowest modules/functions.
- `summarize_log`: loads a file and summarizes it.
- `fmt_ms`: formats microsecond totals as milliseconds.
- `print_status_counts`: prints status counter rows.
- `print_summary`: renders the summary report.
- `main`: CLI entrypoint.

## Context Read

- `soac-pyo3/src/jit_runtime.rs`
- `soac-jit` tracing targets for module-load and codegen events

