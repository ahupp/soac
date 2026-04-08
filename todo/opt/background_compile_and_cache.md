# Background Compile And Cache Warming

## Article Spark

V8 reduces startup cliffs by deferring work, tiering gradually, and using hints/background work for
functions expected to run soon.

## SOAC Question

Can SOAC use counter profiles and module-load metadata to warm the file-backed cache or compile hot
functions off the critical path?

## Concrete Experiment

- After module creation, enqueue only profiled-hot functions for background BlockPy-to-BB/codegen/JIT
  preparation.
- Keep first-call behavior correct: if a function is called before background compile finishes, use
  the current synchronous lazy compile and cancel/reuse the background job.
- Use the named counters directory to decide which function ids get precompiled.
- Persist per-function lowered artifacts so a later process can skip pure lowering for unchanged
  source/runtime metadata.

## Success Signal

- For benchmark/app server style workloads, hot functions are compiled before first hot-loop
  invocation.
- Cold modules/functions do not pay whole-module optimization cost at import.
- Cache hit/miss logs can explain exactly what was reused.

## Risks

- Background compilation must not touch Python objects without the GIL or keep mutable module/type
  state snapshots that can become stale.
