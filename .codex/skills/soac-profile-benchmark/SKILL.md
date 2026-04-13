---
name: soac-profile-benchmark
description: Run SOAC's pystone benchmark and summarize its result directory. Use when Codex needs to benchmark transformed/JIT pystone, capture profile/verify/apply results, or extend an existing profile run with CLIF/perf artifacts.
---

# SOAC Profile Benchmark

Use the repo `Justfile` benchmark recipes from the repo root. They write one
result directory under the ignored shared `bench/` tree.

## Run

For one-off test benchmarks while iterating, run:

```bash
just benchmark
```

This writes `bench/{change_id}_{commit_id}` so rebased or amended jj changes do
not accidentally reuse stale results. The recipe records and prints the actual
current `@` revision that it executed.

When a change is finalized for merge to `main`, rebase the finished change onto
`main` first, switch the workspace to the exact revision that will be merged,
then run the benchmark in finalized mode:

```bash
jj edit <jj-rev>
just benchmark 1000000 100000 bench finalized
```

This writes `bench/{change_id}`. If you intentionally want a fresh child
revision instead of the existing change id, you may switch with `jj new <jj-rev>`
before running `just benchmark`, but the artifact name will then reflect that
new child revision because the recipe always uses the current `@`.

The first positional argument is the specialized apply-pass loop count:

```bash
just benchmark 1000000
```

`just benchmark` runs:

- transformed profile pass
- transformed verify pass
- transformed specialized apply pass with `SOAC_JIT_EMIT_REFCOUNTS=0`
- transformed specialized apply pass with refcount emission enabled

If the user explicitly wants the heavy follow-on artifacts, use one of:

```bash
just benchmark-deep-profile
just benchmark-deep-profile-from-profile <result-dir>
```

These add:

- counter / specialization text dumps
- rendered post-opt CLIF for every lowered pystone function
- rendered Cranelift VCode / CFG for every lowered pystone function
- perf capture and perf-annotated VCode

## Summarize

`just benchmark` prints `benchmark result: <dir>`. Read artifacts from that
directory, especially:

- `benchmark.txt`
- `counters/profile.bin`
- `counters/verify.bin`
- `counters/events.jsonl`

If the deep-profile recipe was used, also read:

- `deep_profile.txt`
- `profile_counters.txt`
- `verify_counters.txt`
- `profile_specializations.txt`
- `verify_specializations.txt`
- `perf.data`
- `perf.injected.data`
- `clif/functions.tsv`
- `clif/fn_<function_id>_<qualname>.clif`
- `clif/fn_<function_id>_<qualname>.vcode`
- `clif/fn_<function_id>_<qualname>.annotated.vcode`

Every benchmark result should also have:

- `summary.txt`
- `summary.json`

For a default benchmark request, report:

- result directory
- specialized apply-pass median loops per second with refcounts enabled from `summary.txt` or `summary.json`
- unsound no-refcounts apply-pass median loops per second as a diagnostic comparison
- verify-mode loops per second
- Cranelift opt level used for the run
- latest pystone JIT code size totals from `summary.txt` or `summary.json`
- any obvious benchmark/runtime surprises

If helpful, you may also mention the profiling-pass throughput, but do not use it
as the headline result.

If the user asks for a warmed unspecialized comparison, use `just benchmark-warm`
and label it clearly as the warm baseline rather than the artifact-producing
benchmark.

## Notes

- Build output may appear before the benchmark numbers when the release extension is stale.
- Benchmark result directories are intentionally untracked under `bench/`.
