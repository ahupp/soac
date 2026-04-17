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
- If Python reaches a function whose compilation is queued but not yet running, remove that job from
  the queue and compile it synchronously on the calling thread. If a worker is already compiling it,
  block until that worker publishes the compiled result.
- Use the named counters directory to decide which function ids get precompiled.
- Persist per-function lowered artifacts so a later process can skip pure lowering for unchanged
  source/runtime metadata.

## Attached Reservation And Wait Follow-Up

The current deadlock-avoidance shape can detach Python only around the reservation/wait path while
keeping actual foreground compilation attached. That is a pragmatic split, but it is not the desired
final model: the reservation and "someone else is compiling this function" wait should also be
expressible without temporarily leaving the attached Python state.

Target shape:

- Model compile reservations so they never require running Python callbacks or touching Python object
  state while holding the process JIT state lock.
- Make waiters block without depending on `py.detach(...)`; the wait path should be a normal Rust
  synchronization point with explicit ownership of any data needed after wake-up.
- Keep foreground first-call compilation attached for paths that need Python-visible state, but make
  the reservation/commit phases purely Rust-side and short.
- Add a regression that exercises "foreground call waits for an in-flight background compile" without
  requiring a detached Python wait.

## Success Signal

- For benchmark/app server style workloads, hot functions are compiled before first hot-loop
  invocation.
- Cold modules/functions do not pay whole-module optimization cost at import.
- Cache hit/miss logs can explain exactly what was reused.

## Risks

- Background compilation must not touch Python objects without the GIL or keep mutable module/type
  state snapshots that can become stale.
