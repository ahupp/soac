---
name: benchmark-compare
description: Compare two SOAC pystone benchmark directories. Use when Codex needs to create missing one-off work/bench/{change_id}_{commit_id} results for jj revisions, compare specialized benchmark throughput, verify specialization counters, and inspect rendered CLIF for changed fast paths.
---

# Benchmark Compare

Use `just benchmark` to create one complete benchmark artifact directory for a
revision, then compare two such directories.

## Result Layout

Results live under the ignored repo-root `work/bench/` directory. Each result
directory used for comparison is named:

```text
work/bench/{change_id}_{commit_id}/
```

This commit-qualified layout is for one-off comparison runs. Finalized
benchmarks for changes being merged to `main` use `work/bench/{change_id}` instead.

An ordinary `just benchmark` result contains:

- `benchmark.txt`: textual report with profile, verify, and specialized
  apply-pass timings.
- `summary.txt` and `summary.json`: specialized throughput and emitted-code
  size summaries.
- `counters/profile.bin`: counters collected by the profile pass.
- `counters/verify.bin`: verify-mode counters collected while applying
  planned specializations.
- `counters/events.jsonl` and `counters/jit-code-summary.jsonl`: runtime events
  and compact generated-code summaries.
- `counters/modules/**/mod.blockpy`: cached pre-optimization BlockPy modules;
  apply and verify consume these together with `profile.bin` to make typed v3
  decisions during JIT planning, without a serialized optimization plan.

Only `just benchmark-deep-profile` or
`just benchmark-deep-profile-from-profile <result-dir>` adds:

- `profile_counters.txt` and `verify_counters.txt`: textual counter dumps.
- `profile_specializations.txt` and `verify_specializations.txt`:
  specialization summaries.
- `clif/functions.tsv`, `clif/fn_<function_id>_<qualname>.clif`, and
  `clif/fn_<function_id>_<qualname>.vcode`: lowered pystone functions plus the
  rendered post-opt CLIF and VCode for each one.
- `perf.data`, `perf.injected.data`, and
  `clif/fn_<function_id>_<qualname>.annotated.vcode`: native profiling and
  JIT-block attribution.

## Create The Current Result

From a checkout containing the target code:

```bash
just benchmark
```

Useful shorter run while iterating:

```bash
BENCHMARK_SPECIALIZED_RUNS=3 \
just benchmark 1000000 100000
```

The recipe does not run the stock CPython benchmark.

## Find An Existing Result

When asked to compare jj revs, first resolve each requested rev to its short
change id and short commit id. Reuse only an exact
`work/bench/{change_id}_{commit_id}` directory for that resolved revision:

```bash
change_id="$(jj --ignore-working-copy log -r "<jj-rev>" --no-graph -T 'change_id.short()')"
commit_id="$(jj --ignore-working-copy log -r "<jj-rev>" --no-graph -T 'commit_id.short()')"
result_dir="work/bench/${change_id}_${commit_id}"
test -d "$result_dir" && printf '%s\n' "$result_dir"
```

Do not reuse a directory that only matches the change id. Rebased jj changes can
keep the same change id while changing code, dependencies, or benchmark recipe
behavior, so prefix reuse can silently compare against stale artifacts. If the
exact `{change_id}_{commit_id}` directory is absent, re-run `just benchmark` for
that revision and create the exact directory for the current commit id. The
benchmark recipe always records the current `@` revision, so switch the
workspace to the revision you want before running it.

A result is complete enough for throughput and generated-code-size comparison
when it has `benchmark.txt`, `summary.json`, `counters/profile.bin`, and
`counters/verify.bin`. Textual counter summaries, specialization summaries,
CLIF/VCode, and perf artifacts are optional deep-profile diagnostics, not
ordinary benchmark outputs. When those details are needed for an existing
result, run `just benchmark-deep-profile-from-profile <result-dir>` to reuse
its existing profile evidence; do not rerun `just benchmark` expecting it to
create deep-profile artifacts.

## Create A Missing Result For Another Rev

If no complete exact `work/bench/{change_id}_{commit_id}` result exists for a
requested jj rev, create a temporary side workspace and run the result producer
there. Always write results back into the original repo's `work/bench/` directory:

```bash
original_repo="$PWD"
rev="<jj-rev>"
ws="$(mktemp -d "${TMPDIR:-/tmp}/soac-benchmark-compare.XXXXXX")"
workspace_name="benchmark-compare-$$"
jj workspace add --revision "$rev" --name "$workspace_name" "$ws"

if [[ ! -x "$ws/vendor/cpython/python" ]]; then
  rm -rf "$ws/vendor/cpython"
  mkdir -p "$ws/vendor"
  ln -s "$original_repo/vendor/cpython" "$ws/vendor/cpython"
fi

(
  cd "$ws"
  jj edit "$rev"
  just benchmark 1000000 100000 "$original_repo/work/bench"
)

jj workspace forget "$workspace_name"
rm -rf "$ws"
```

## Compare

Compare in this order:

1. Confirm the directory names and `benchmark.txt` revision headers match the
   intended revisions.
2. Extract specialized apply-pass loops/sec from `benchmark.txt`; use the
   median as the headline result.
3. Compare profile-pass loops/sec only as overhead context.
4. Compare generated-code bytes and machine-block counts in each `summary.json`.
5. When deep-profile diagnostics are requested, generate them for the relevant
   side(s), then compare `profile_specializations.txt` with
   `verify_specializations.txt` inside each result and across revisions.
6. If available, compare `verify_counters.txt` hit/fallback totals for direct
   calls, globals, fields, operators, and other expected fast paths.
7. If available, compare the relevant `clif/fn_*_*.clif` and
   `clif/fn_*_*.vcode` files for functions whose specialization sets or verify
   hits moved materially.
8. If the benchmark delta still needs explanation, inspect the separately
   collected perf artifacts and switch to the `analyze-pystone-perf` workflow.

When reporting, include the two result directories, median specialized
throughput, relative delta, and emitted-code-size delta. Include
specialization-set deltas, verify hit/fallback summaries, and rendered-CLIF
paths only when their deep-profile diagnostics were actually generated.
