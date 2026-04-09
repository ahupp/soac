---
name: run-pystone-benchmark
description: Run this repo's pystone benchmark recipe and summarize its bench/{change_id}_{commit_id} result directory. Use when Codex needs to benchmark transformed/JIT pystone, capture profile/verify/specialized counters, run perf for the specialized apply run, or summarize the artifact-producing benchmark output.
---

# Run Pystone Benchmark

Use the artifact-producing `Justfile` benchmark recipe from the repo root.
It writes one result directory under the ignored `bench/` tree.

## Run

```bash
just benchmark
```

The first positional argument is the specialized apply-pass loop count:

```bash
just benchmark 1000000
```

The benchmark recipe uses 10M loops for the perf-recorded specialized apply
run by default, so perf reports have enough samples for hotspot attribution.

`just benchmark` runs:

- transformed profile pass
- transformed verify pass
- transformed specialized apply pass
- perf capture of the specialized apply run
- counter / specialization text dumps
- rendered post-opt CLIF for every lowered pystone function

## Summarize

`just benchmark` prints `benchmark result: <dir>`. Read artifacts from that
directory, especially:

- `benchmark.txt`
- `profile_counters.txt`
- `verify_counters.txt`
- `profile_specializations.txt`
- `verify_specializations.txt`
- `perf.log`
- `perf_by_dso_symbol.txt`
- `perf_callgraph.txt`
- `clif/functions.tsv`
- `clif/fn_<function_id>_<qualname>.clif`

For a default benchmark request, report:

- result directory
- specialized apply-pass loops per second from `benchmark.txt`
- verify-mode loops per second
- perf-run loops per second from `perf.log`
- any specialization-summary or verify-counter surprises

If helpful, you may also mention the profiling-pass throughput, but do not use it
as the headline result.

If the user asks for a warmed unspecialized comparison, use `just benchmark-warm`
and label it clearly as the warm baseline rather than the artifact-producing
benchmark.

## Notes

- Build output may appear before the benchmark numbers when the release extension is stale.
- Benchmark result directories are intentionally untracked under `bench/`.
