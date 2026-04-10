---
name: benchmark-compare
description: Compare two SOAC pystone benchmark directories. Use when Codex needs to create missing bench/{change_id}_{commit_id} results for jj revisions, compare specialized benchmark throughput, verify specialization counters, and inspect paired perf profiles.
---

# Benchmark Compare

Use `just benchmark` to create one complete benchmark/perf artifact
directory for a revision, then compare two such directories.

## Result Layout

Results live under the ignored repo-root `bench/` directory. Each result
directory is named:

```text
bench/{change_id}_{commit_id}/
```

Important files:

- `benchmark.txt`: textual report with profile, verify, and specialized
  apply-pass timings.
- `counters/profile.bin`: counters collected by the profile pass.
- `counters/verify.bin`: verify-mode counters collected while applying
  profile specializations.
- `profile_counters.txt` and `verify_counters.txt`: textual counter dumps.
- `profile_specializations.txt` and `verify_specializations.txt`:
  specialization summaries.
- `perf.log`: loops/sec during the perf run.
- `perf_by_dso.txt`, `perf_by_dso_symbol.txt`, `perf_callgraph.txt`,
  `perf_report.txt`, and `perf_speedscope.json`: perf outputs for the
  specialized apply-mode run.
- `perf_cranelift_blocks.tsv`: perf samples attributed to generated Cranelift
  basic blocks using the JIT block-offset sidecar.
- `clif/functions.tsv`, `clif/fn_<function_id>_<qualname>.clif`, and
  `clif/fn_<function_id>_<qualname>.vcode`: lowered pystone functions plus the
  rendered post-opt CLIF and VCode for each one.
- `clif/fn_<function_id>_<qualname>.annotated.vcode`: VCode with perf sample
  counts inserted before sampled block labels.

## Create The Current Result

From a checkout containing the target code:

```bash
just benchmark
```

Useful shorter run while iterating:

```bash
BENCHMARK_SPECIALIZED_RUNS=3 \
just benchmark 1000000 100000 10000000
```

The recipe does not run the stock CPython benchmark.

## Find An Existing Result

When asked to compare jj revs, first resolve each requested rev to its short
change id and short commit id. Reuse only an exact
`bench/{change_id}_{commit_id}` directory for that resolved revision:

```bash
change_id="$(jj --ignore-working-copy log -r "<jj-rev>" --no-graph -T 'change_id.short()')"
commit_id="$(jj --ignore-working-copy log -r "<jj-rev>" --no-graph -T 'commit_id.short()')"
result_dir="bench/${change_id}_${commit_id}"
test -d "$result_dir" && printf '%s\n' "$result_dir"
```

Do not reuse a directory that only matches the change id. Rebased jj changes can
keep the same change id while changing code, dependencies, or benchmark recipe
behavior, so prefix reuse can silently compare against stale artifacts. If the
exact `{change_id}_{commit_id}` directory is absent, re-run `just benchmark` for
that revision and create the exact directory for the current commit id.

A result is complete enough for comparison when it has `benchmark.txt`,
`counters/profile.bin`, `counters/verify.bin`, `verify_counters.txt`,
`profile_specializations.txt`, `verify_specializations.txt`, `perf.log`, and
`perf_callgraph.txt`. Prefer results that also have
`perf_cranelift_blocks.tsv`, `clif/functions.tsv`, `clif/*.clif`, and
`clif/*.vcode`; prefer `clif/*.annotated.vcode` when comparing JIT block-level
changes. If the exact result exists but lacks these generated inspection
artifacts, say so and re-run `just benchmark` for that side only if the missing
detail is needed.

## Create A Missing Result For Another Rev

If no complete exact `bench/{change_id}_{commit_id}` result exists for a
requested jj rev, create a temporary side workspace and run the result producer
there. Always write results back into the original repo's `bench/` directory:

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
  just benchmark 1000000 100000 10000000 "$original_repo/bench" @-
)

jj workspace forget "$workspace_name"
rm -rf "$ws"
```

The temporary workspace's `@` is an empty child of the requested revision, so
pass `@-` as the final `just benchmark` revision argument.

## Compare

Compare in this order:

1. Confirm the directory names and `benchmark.txt` revision headers match the
   intended revisions.
2. Extract specialized apply-pass loops/sec from `benchmark.txt`; use the
   median as the headline result.
3. Compare profile-pass loops/sec only as overhead context.
4. Confirm `profile_specializations.txt` and `verify_specializations.txt`
   match inside each result or explain any difference.
5. Compare A vs B `profile_specializations.txt`; changed specialization sets can
   dominate benchmark throughput.
6. Compare `verify_counters.txt` hit/fallback totals for direct calls, globals,
   fields, operators, and other expected fast paths.
7. Compare `perf.log` loops/sec as a shorter perf-context run.
8. Compare `perf_by_dso.txt`, `perf_by_dso_symbol.txt`, and `perf_callgraph.txt`
   to explain where time moved.
9. Compare `perf_cranelift_blocks.tsv` to identify the specific JIT blocks that
   gained or lost samples.
10. Compare the relevant `clif/fn_*_*.clif`, `clif/fn_*_*.vcode`, and
   `clif/fn_*_*.annotated.vcode` files for functions whose perf
   stacks moved materially.

When reporting, include the two result directories, median specialized
throughput, relative delta, specialization-set delta, verify hit/fallback
summary, rendered-CLIF paths inspected, and the perf hotspot movement that best
explains the delta.
