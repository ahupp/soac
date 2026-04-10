---
name: analyze-pystone-perf
description: End-to-end workflow for SOAC pystone performance analysis. Use when Codex needs to generate benchmark specialization counters, run the specialized pystone benchmark under perf, render specialized JIT CLIF, correlate perf hotspots with generated CLIF / source, or produce ranked optimization suggestions from benchmark, perf, counters, and CLIF evidence.
---

# Analyze Pystone Perf

Run the artifact-producing `$soac-profile-benchmark` flow, then connect the
resulting perf hotspots to the specialized CLIF that produced them.

## Workflow

Run steps from the repo root. One-off benchmark, counter, perf, and rendered
CLIF artifacts are created under `bench/{change_id}_{commit_id}/`; finalized
benchmarks for changes being merged to `main` use `bench/{change_id}/`.

1. **Generate benchmark/counter/perf artifacts**

```bash
BENCHMARK_CONSTANT_CLOCKS=0 just benchmark
```

Use the specialized apply pass in `<result-dir>/benchmark.txt`, not the profile
or verify pass, as the transformed benchmark headline. The recipe prints
`benchmark result: <result-dir>`. Use that directory for the rest of the
workflow.

2. **Sanity-check that expected specializations are still reached**

`just benchmark` already runs a verify-mode pass and writes
`<result-dir>/counters/verify.bin`,
`<result-dir>/profile_specializations.txt`, and
`<result-dir>/verify_specializations.txt`.

Compare profile and verify at the **site / target level**, not by raw counter
counts or dump size. Expected hot call/operator sites from the profile should
either appear in the verify dump or have an understood reason for disappearing
such as a deliberately bypassed operation or a too-small verify loop count.

Do this before trusting perf conclusions:

- Check that `<result-dir>/counters/verify.bin` exists and is non-empty.
- Check that expected hot call targets are present in the verify specialization summary.
- Render specialized CLIF and grep for the expected fast-path shape.
- Investigate missing expected sites as “specialization may not be running” before ranking deeper codegen issues.

3. **Read the perf artifacts**

For prefix `<result-dir>/perf`, inspect:

- `<prefix>.log`: measured loops/sec during the profiled run.
- `<prefix>_by_dso.txt`: split between CPython, SOAC runtime/extension, and `[JIT]`.
- `<prefix>_by_dso_symbol.txt`: top self-time symbols.
- `<prefix>_callgraph.txt`: cumulative stacks and helper-call parents.
- `<prefix>_report.txt`: full report if the condensed reports are ambiguous.
- `<result-dir>/perf_cranelift_blocks.tsv`: JIT samples attributed to generated
  Cranelift basic blocks.

Look for SOAC-specific boundaries first: specialized runtime helpers, generic
CPython hooks reached from helpers, fallback vectorcall / eval-frame stacks,
dictionary lookup from global helpers, generic attr access, exact-long helpers,
metadata lookup for direct calls, PyNumber fallbacks, rich-compare fallbacks,
and refcount / deallocation clusters.

4. **Read the counters and specializations**

`just benchmark` writes textual dumps beside the binary counter files:

- `<result-dir>/profile_counters.txt`
- `<result-dir>/verify_counters.txt`
- `<result-dir>/profile_specializations.txt`
- `<result-dir>/verify_specializations.txt`
- `<result-dir>/clif/functions.tsv`
- `<result-dir>/clif/fn_<function_id>_<qualname>.clif`
- `<result-dir>/clif/fn_<function_id>_<qualname>.vcode`
- `<result-dir>/clif/fn_<function_id>_<qualname>.annotated.vcode`

5. **Read rendered specialized CLIF for hot functions**

```bash
rg -n 'Proc0|Proc1|Proc8|Func2' <result-dir>/clif/functions.tsv
rg -n 'soac_runtime_|dp_jit_|PyObject_|PyNumber_|call_indirect' \
  <result-dir>/clif/fn_*_*.clif
```

`just benchmark` renders one post-opt CLIF file per lowered pystone function,
plus raw and perf-annotated VCode. Start with functions that appear as `[JIT]
py:d:<name>` in perf, then add callees whose helpers dominate the callgraph.
For pystone this is usually `Proc0`, `Proc1`, `Proc8`, `Func2`, and the
procs/functions reached from `Proc0`.

6. **Correlate perf to CLIF and source**

Use `rg` over the rendered CLIF for the helper or import names seen in perf.
Count helper calls when the scale matters:

```bash
rg -n 'dp_jit_py_vectorcall|call_indirect|dp_jit_direct_code_ptr|dp_jit_pyobject_getattr|dp_jit_pyobject_setattr|dp_jit_exact_long_binary_op|call PyNumber_|PyObject_RichCompare' \
  <result-dir>/clif/fn_*_*.clif
```

When reporting a candidate, cite the full chain:

- Perf self/cumulative cost and the owning helper stack.
- Cranelift block sample count from `perf_cranelift_blocks.tsv`, when the cost
  is in generated JIT code.
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

- Result directory.
- Specialized transformed loops/sec, verify loops/sec, perf-run loops/sec, and profiled-run loops/sec.
- DSO split from perf.
- Paths for benchmark.txt, perf callgraph, perf symbols, and rendered hot CLIF.
- Top 5 ranked optimization candidates.

For each candidate, include: hotspot, evidence, interpretation, and the next
implementation step.
