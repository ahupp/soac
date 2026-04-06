---
name: run-pystone-benchmark
description: Run this repo's pystone benchmark recipes and capture the output in logs/. Use when Codex needs to benchmark transformed or JIT execution against stock CPython, compare loops per second, or summarize the two-pass specialized benchmark or the warmed unspecialized baseline.
---

# Run Pystone Benchmark

Use the `Justfile` benchmark recipes from the repo root and capture the output to a file in `logs/`.

## Run

Use `set -o pipefail` so the benchmark exit status is preserved when logging:

```bash
set -o pipefail
just benchmark 2>&1 | tee logs/benchmark_run.log
```

If the user requests a different loop count, pass it as the first argument to the `just` recipe and keep the log in `logs/`.

By default, a plain benchmark request means `just benchmark`, which is the
two-pass flow:

- transformed profiling pass
- transformed specialized pass
- stock CPython baseline

Report the specialized second-pass throughput as the transformed result unless
the user explicitly asks for the warmed unspecialized baseline, in which case
use `just benchmark-warm`.

## Summarize

`just benchmark` prints three sections:

- `jit transformed profile pass`
- `jit transformed specialized pass`
- `stock cpython`

For a default benchmark request, report:

- specialized transformed/JIT loops per second
- stock loops per second
- relative slowdown or speedup factor

If helpful, you may also mention the profiling-pass throughput, but do not use it
as the headline transformed result.

If the user asks for a warmed unspecialized comparison, use `just benchmark-warm`
and label it clearly as the warm baseline rather than the default benchmark.

## Notes

- Build output may appear before the benchmark numbers when the release extension is stale.
- Keep benchmark artifacts in `logs/` and refer to the log path in the final summary.
