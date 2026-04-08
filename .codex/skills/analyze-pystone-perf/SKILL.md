---
name: analyze-pystone-perf
description: End-to-end workflow for SOAC pystone performance analysis. Use when Codex needs to generate benchmark specialization counters, run the specialized pystone benchmark under perf, render specialized JIT CLIF, correlate perf hotspots with generated CLIF / source, or produce ranked optimization suggestions from benchmark, perf, counters, and CLIF evidence.
---

# Analyze Pystone Perf

Profile the default `just benchmark` flow, then connect the resulting perf
hotspots to the specialized CLIF that produced them.

## Workflow

Run steps from the repo root. Keep logs and rendered CLIF in `logs/`.

1. **Generate benchmark counters and baseline throughput**

```bash
set -o pipefail
BENCHMARK_CONSTANT_CLOCKS=0 just benchmark 2>&1 | tee logs/benchmark_specialization_for_perf.log
```

Use the specialized pass, not the profiling pass, as the transformed
benchmark headline. The profile counter dump is written to
`logs/last_benchmark_counters/profile.bin`.

2. **Sanity-check that expected specializations are still reached**

Run the verify-mode pass before perf analysis. It applies
`profile.bin`, records the specialization-input counters again, and writes
`verify.bin`.

```bash
set -o pipefail
BENCHMARK_CONSTANT_CLOCKS=0 just benchmark-verify 100000 \
  2>&1 | tee logs/benchmark_specialization_verify.log

cargo run -p soac-inspector --bin inspect_counters -- \
  --specializations logs/last_benchmark_counters/profile.bin \
  > logs/profile_call_target_specializations.txt

cargo run -p soac-inspector --bin inspect_counters -- \
  --specializations logs/last_benchmark_counters/verify.bin \
  > logs/verify_call_target_specializations.txt
```

Compare profile and verify at the **site / target level**, not by raw counter
counts or dump size. Expected hot call/operator sites from the profile should
either appear in the verify dump or have an understood reason for disappearing
such as a deliberately bypassed operation or a too-small verify loop count.

Do this before trusting perf conclusions:

- Check that `logs/last_benchmark_counters/verify.bin` exists and is non-empty.
- Check that expected hot call targets are present in the verify specialization summary.
- Render specialized CLIF and grep for the expected fast-path shape.
- Investigate missing expected sites as “specialization may not be running” before ranking deeper codegen issues.

3. **Record perf using the exact benchmark counter dump**

```bash
DIET_PYTHON_COUNTERS_DIR="$PWD/logs/last_benchmark_counters" \
DIET_PYTHON_SPECIALIZATION_MODE=apply \
BENCHMARK_CONSTANT_CLOCKS=0 \
PERF_PERCENT_LIMIT=0.2 \
just perf-pystone-jit-warm 500000 logs/pystone_jit_perf_specialized_from_benchmark \
  > logs/pystone_jit_perf_specialized_from_benchmark.just.log 2>&1
```

Prefer the paired `just run-and-view-speedscope` / `just
perf-pystone-jit-warm` recipes over ad-hoc `perf` commands, because they set up
the warmed / stopped process protocol and write the standard report set.

4. **Read the perf artifacts**

For prefix `logs/pystone_jit_perf_specialized_from_benchmark`, inspect:

- `<prefix>.log`: measured loops/sec during the profiled run.
- `<prefix>_by_dso.txt`: split between CPython, SOAC runtime/extension, and `[JIT]`.
- `<prefix>_by_dso_symbol.txt`: top self-time symbols.
- `<prefix>_callgraph.txt`: cumulative stacks and helper-call parents.
- `<prefix>_report.txt`: full report if the condensed reports are ambiguous.

Look for SOAC-specific boundaries first: specialized runtime helpers, generic
CPython hooks reached from helpers, fallback vectorcall / eval-frame stacks,
dictionary lookup from global helpers, generic attr access, exact-long helpers,
metadata lookup for direct calls, PyNumber fallbacks, rich-compare fallbacks,
and refcount / deallocation clusters.

5. **Dump and translate the specializations**

```bash
cargo run -p soac-inspector --bin inspect_counters -- \
  --dump logs/last_benchmark_counters/profile.bin \
  > logs/last_benchmark_counters_dump.txt

cargo run -p soac-inspector --bin inspect_counters -- \
  --specializations logs/last_benchmark_counters/profile.bin \
  > logs/last_benchmark_specializations.txt
```

The benchmark counter dump may use packed runtime `FunctionId` values. The
standalone `render_jit_clif` tool reparses the module and usually uses a
different small module id. If `render_jit_clif --specialized` says it has no
specialized plan for a function that should be hot, translate the
specialization env vars from benchmark ids to the function ids reported by the
renderer / counter dump before rendering.

6. **Render specialized CLIF for hot functions**

```bash
cargo run -p soac-inspector --bin render_jit_clif -- \
  --specialized scripts/pystone.py <function_id> \
  > logs/pystone_fn<function_id>_specialized.clif
```

Start with functions that appear as `[JIT] py:d:<name>` in perf, then add callees
whose helpers dominate the callgraph. For pystone this is usually `Proc0`,
`Proc1`, `Proc8`, `Func2`, and the procs/functions reached from `Proc0`.

7. **Correlate perf to CLIF and source**

Use `rg` over the rendered CLIF for the helper or import names seen in perf.
Count helper calls when the scale matters:

```bash
rg -n 'dp_jit_py_vectorcall|call_indirect|dp_jit_direct_code_ptr|dp_jit_pyobject_getattr|dp_jit_pyobject_setattr|dp_jit_exact_long_binary_op|call PyNumber_|PyObject_RichCompare' \
  logs/pystone_fn*_specialized.clif
```

When reporting a candidate, cite the full chain:

- Perf self/cumulative cost and the owning helper stack.
- The CLIF helper calls / fallback blocks that explain that cost.
- The Python source shape if it matters.
- The missing specialization or codegen decision that would remove the cost.

## Ranking Guidance

Rank opportunities by expected pystone impact and confidence, not by self-time
alone.

Prefer candidates where perf and CLIF agree. Example: perf shows
`clif_vectorcall_data`, and CLIF shows repeated
`dp_jit_direct_code_ptr` / `dp_jit_direct_vmctx` calls on direct-call fast paths.

Treat generic CPython symbols as actionable only when the parent stack is a
SOAC helper or generated fallback. Example: `unicodekeys_lookup_unicode` under
`soac_runtime_load_global_slow` points at global-load specialization; the same
symbol under unrelated CPython import code is noise.

Do not stop at “X is hot.” State the likely fix shape: inline guarded fast path,
outline miss path, add counter input, add type/key guard, hoist constant metadata,
remove a generic fallback island, or prune redundant ownership cleanup.

## Final Report

Include:

- Specialized transformed loops/sec, stock loops/sec, and profiled-run loops/sec.
- DSO split from perf.
- Log paths for benchmark, perf callgraph, perf symbols, and rendered hot CLIF.
- Top 5 ranked optimization candidates.

For each candidate, include: hotspot, evidence, interpretation, and the next
implementation step.
