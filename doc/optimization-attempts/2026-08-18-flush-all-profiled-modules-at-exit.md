---
title: "Flush all profiled modules at exit"
---

# Flush all profiled modules at exit

- Status: landed
- Pacific date: 2026-08-18 PDT
- Change: `ltuyuupm`, based on retained closure optimization `otwslxwo`.
- Outcome: retained correctness/coverage improvement. Baseline
  `comprehensions` fails strict apply because its completed
  profile contains only the benchmark main module and omits live
  `soac.runtime`. A deterministic shutdown observer independently fails
  because no counter file exists after late user work while modules remain
  live. Candidate exact-once profile/verify flushing, late counters, and
  strict apply now pass focused regressions; the previously failing
  `comprehensions` benchmark now completes normal profile-to-apply sampling.
  The four-workload stock-relative score is only **0.2579951x**; the full
  correctness gate passes, but full-suite acceptance remains incomplete.

## Hypothesis and evidence

SOAC records independent binary counter frames for each transformed module.
Cross-module apply plans correctly require a source-matched frame for every
selected direct target. The current implementation writes a module's frame
only from that transformed module's CPython `m_clear` callback. However, the
process-global `CompileSession` retains `SharedModuleState` objects, runtime
helpers, nested functions, and live generator references; there is no
guarantee that every profiled module clears before interpreter exit.

The standalone `comprehensions` baseline,
`work/pyperformance/comparison-20260818-152600-dmzNUX`, demonstrates the
actual failure before this change. Stock completes at approximately
**19.3 microseconds** and SOAC profile completes at **188 milliseconds**, but
SOAC apply fails:

```text
RuntimeError: counter dump does not contain module soac.runtime [direct_target=exception_matches id=2:27]
```

Its `profile.bin` contains exactly one **71,096-byte `__main__` frame** and
no runtime frame. Successful `chaos`, `deltablue`, and `richards` workers each
contain two frames, including a **508,936-byte `soac.runtime` frame**.
Profile events confirm `exception_matches` was JIT-compiled; the missing
runtime frame prevents proving its counter activity and makes apply reject
the otherwise valid selected cross-module target. Baseline diagnostic output
is `work/logs/synthetic-closure-comprehensions-baseline-smoke.log`.

This is a general counter-lifecycle/correctness bug, not a `comprehensions`
exception or a reason to weaken source-hash/target validation. Flush every
live profiled module deterministically after final user work and before
shutdown observers inspect the output. Preserve exact-once serialization when
ordinary `m_clear` already wrote the same module/path or when type-layout
serialization reenters Python.

## Implementation and compatibility

- Add one explicit public Rust API,
  `CompileSession::flush_counter_dump_outputs`, which snapshots retained
  `SharedModuleState` objects under the existing registry mutex, releases the
  registry lock, then flushes eligible states using the normal configured
  profile/verify output path.
- Register a private `_soac_ext` shutdown callable with Python `atexit` during
  extension initialization. Do not export it through `module.add_function`,
  add a new user-visible Python API, introduce thread-local/global cache
  state, or special-case module/benchmark/source names.
- Preserve LIFO ordering: an observer registered **before any SOAC import**
  runs after SOAC's flush, while a user callback registered after SOAC import
  runs first and can still execute transformed code. Snapshot counters only
  after that late callback, so its final observations are included.
- Keep existing `m_clear` behavior and immediate per-module flush when the
  module really clears earlier. A state-local tracker records each output
  `PathBuf` as `InProgress` or `Complete` and whether the underlying module
  has already cleared. Repeated shutdown/clear/reentrant attempts must emit
  exactly one frame per `(module state, output path)`; profile and verify
  paths remain independent. This is a per-process invariant, not a promise
  that a file shared by independent worker processes contains only two total
  frames: ten processes each writing main plus runtime correctly yield twenty
  frames.
- Reserve or inspect state under its private mutex, then release that mutex
  before `counter_dump_record` snapshots type/layout metadata or executes any
  Python-visible callbacks. Mark success `Complete`; remove a failed
  reservation so a later attempt can retry. Never serialize while holding the
  process-registry lock or the per-state deduplication lock.
- Preserve `SOAC_OPT_MODE=profile` -> `profile.bin`, `verify` -> `verify.bin`,
  and no dump in `apply` or `none`. Preserve source identity, source-hash
  validation, module-key/type-key snapshots, strict missing-target errors,
  reference lifetime, shutdown ordering, user callbacks, and callback
  exceptions. Do not fabricate an empty runtime frame or relax validation.
- Regression fixture should register its final observer before importing any
  SOAC module, hold runtime modules/generators strongly enough that ordinary
  `m_clear` does not run, register late user work afterward, and validate
  complete runtime/main frames with final counter values before interpreter
  exit. Reenter serialization through custom type-layout callbacks and assert
  exactly one frame per module/path. The genuine baseline RED,
  `tests/test_counter_dump_shutdown_flush.py::test_counter_dump_flushes_live_modules_once_after_user_exit_callbacks`,
  fails in **0.42 seconds**: the observer sees the late callback's expected
  `seen=[10, 20]`, but `profile.bin` is absent because both transformed
  modules remain strongly live and neither `m_clear` has run. Candidate
  end-to-end GREEN now passes with existing mode/lifecycle guardrails:
  **four integration tests passed in 5.90 seconds**.
- The structured Rust regression
  `counter_dump_flush_is_path_aware_reentrant_and_retries_failed_writes`
  passes: **one test passed, 545 filtered** in the final suite. It verifies
  failed-write retry,
  exactly one successful frame per module/output path, independent verify
  output, `InProgress` reentry suppression, and cleared-state skipping. The
  combined `cargo check -p soac_jit -p soac_pyo3 --tests` passes without
  warnings, and all **eight PyO3 crate tests** pass.
- The final end-to-end candidate restores runtime and target profile frames
  exactly once before `m_clear`, records the late user callback as
  `seen=[10, 20]` and **two `call_hot_targets` observations**, and passes
  strict apply replay. Verify mode independently exposes both exact-once
  frames in `verify.bin` and final user work while correctly omitting the
  profile-only call-target counter. Existing immediate `m_clear`, no-dump
  `none`, and replay regressions also pass: **four tests in 5.90 seconds**.
  Package-scoped formatting/Rust checks pass. A direct attribute assertion
  confirms the shutdown callback is not exposed on `_soac_ext`. The full
  `just test-all` correctness gate passes **1,211 Python node IDs across 75
  batches** and all Rust crate suites.
- Public API surface: exactly one new public Rust method on the already
  exported `CompileSession`; the extension shutdown callable remains private.

## Benchmark protocol and coverage

- Existing stable fixed subset: `chaos,richards,deltablue`, with normal
  baseline
  `work/pyperformance/comparison-20260818-153938-uayM6V/summary.json`.
  Baseline means are `chaos` **84.2816742 ms**, `deltablue` **4.5131975 ms**,
  and `richards` **43.2257645 ms**. Its stock-relative geometric mean is
  **0.4013803170x**, still far below the full-suite **1.10x** goal.
- The actual newly enabled benchmark is `comprehensions`; baseline stock and
  profile succeed but baseline apply fails. Its first candidate comparison
  must be reported as restored coverage, not as a previous-SOAC speedup
  against a nonexistent baseline apply result.
- First completion-only smoke:
  `just pyperformance-compare comprehensions 1 '' --debug-single-value`.
  Single-value results are useful for restored stock/profile/apply completion
  and frame inspection; they are not a representative throughput result.
- Completed previously failing `comprehensions` candidate smoke:
  `work/pyperformance/comparison-20260818-160717-N8T737/summary.json`;
  full log `work/logs/profile-shutdown-flush-comprehensions-smoke.log`.
  The release smoke completes in **23.01 seconds** and restores the runtime
  profile frame plus strict apply. Its **580,112-byte `profile.bin`** contains
  exactly two frames in compile-session registry order: a **509,256-byte
  `soac.runtime` payload**, then a **70,776-byte `__main__` payload**. The
  older failing main-only profile was **71,096 bytes**; previously successful
  other workloads had **508,936-byte** runtime payloads, so candidate frame
  sizes must not be misreported as unchanged. Stock **20.251 microseconds** versus SOAC
  **483.236 microseconds** is a cold single value, approximately **23.86x
  slower**; report this as newly restored coverage, not a statistically valid
  headline. It transforms `__main__` and `soac.runtime`, lists **21 compiled
  JIT function names**, and reports **486 typed basic blocks / 26 function
  instances**, **299,484 native bytes**, **19,796 machine blocks**, and
  **930,126 pre-optimization BlockPy bytes**. Apply setup is approximately
  **339 milliseconds**. Actual measured-loop JIT coverage includes
  `bench_comprehensions` (**26,012 bytes**), `WidgetTray._add_widgets`
  (**18,876 bytes**), five nested list/dict comprehensions (**11–25 KB**),
  `_any_knobby`/its generator expression (**24,452 bytes**), and
  `soac.runtime.exception_matches` (**3,440 bytes**). The
  **95,140-byte** `make_some_widgets` function executes before measurement
  starts and is setup coverage, not measured hot-path coverage.
- Expanded candidate set, only after `comprehensions` completes:
  `chaos,richards,deltablue,comprehensions`. Compare previous SOAC throughput
  only on the original three shared workloads; do not compare a
  four-benchmark candidate geometric mean or generated-code total against a
  three-benchmark baseline.
- Candidate normal comparison command:
  `just pyperformance-compare chaos,richards,deltablue,comprehensions 1`.
  Independently compare the three common workloads against
  `work/pyperformance/comparison-20260818-153938-uayM6V` and describe the
  newly enabled fourth benchmark separately.
- Completed expanded normal-sampling comparison:
  `work/pyperformance/comparison-20260818-160837-V7kb6V/summary.json`;
  output `work/logs/profile-shutdown-flush-expanded.log`. All **four**
  workloads complete in **98.19 seconds**, with **20 measured values per
  benchmark**. Across **40 apply workers**, setup totals **39.1 seconds**
  while measured values account for **7.10 seconds**. Ten independent
  measured worker subprocesses append one main and one runtime frame each,
  producing **20 correct frames** in the shared worker profile; a single
  calibration worker produces two. Exact-once applies to each
  module/state/path within each process.
- Completed full correctness gate:
  `work/logs/profile-shutdown-flush-test-all.log`. `just test-all` passes
  **1,211 Python node IDs across 75 batches**, **545 `soac_jit`**, **367
  `soac_lowering`**, **202 `soac_opt`**, and **eight PyO3-extension** tests.
  Cargo tests take **66.897 seconds**, pytest takes **102.920 seconds**, the
  full test phase takes **169.841 seconds**, and total elapsed time is
  **171.49 seconds**. The slowest counter-dump batch takes **101.31
  seconds**.
- First `comprehensions` native-profile attempt:
  `work/logs/profile-shutdown-flush-comprehensions-perf.log`. Replaying
  **20,000 loops** creates a **139.362 MB** capture with **2,375 samples**,
  **6.19% sample loss**, and **38 lost chunks**. The project loss guard
  correctly rejects this capture; it is not valid hotspot evidence.
- A bounded **5,000-loop** retry avoids sample loss and records **802 samples
  / 50.217 MB**, but **30.42%** of its inclusive profile is cold/late JIT
  compilation. Reject it as a representative steady-state hotspot profile;
  zero lost samples alone do not prove the measured phase is clean.
- The valid steady-state profile replays **30,000 loops** with
  `PERF_FREQUENCY=199`, recording **659 samples / 41.331 MB / zero lost
  samples** in **9.92 seconds**. Its call graph is
  `work/logs/comprehensions-shutdown-flush-steady_callgraph.txt`. Synthetic
  closure creation accounts for **26.25% inclusive**, including **23.23%**
  in function instantiation, **4.71%** in JIT vectorcall registration, and
  **3.49%** in synthetic code lookup. `exception_matches` accounts for
  **6.83% inclusive** and `_validate_exception_type` for **5.01%**; aggregate
  late compilation contributes **6.07%**, and first/second-half profile
  composition still shifts. These overlapping inclusive shares identify
  future workload-specific strategies without claiming the flush itself
  reduced execution cost.
- Existing transformed project modules are `__main__` and `soac.runtime`;
  no standard-library modules are transformed in the prior three workloads.
  Their baseline compiled function counts are **35 / 79 / 53**. Inspect
  candidate `comprehensions` project/runtime coverage and final profile frame
  counts before claiming the new benchmark exercises the intended path.
- All-module flushing is expected to improve correctness and suite completion,
  not steady-state Python throughput. Measure exit/worker setup separately;
  avoid claiming a pyperformance speedup from added or removed benchmarks.

## Measurements

| Profile completeness / correctness metric | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| `comprehensions` main profile frame | 1 / 71,096 bytes | 1 / 70,776-byte payload | retained |
| `comprehensions` runtime profile frame | missing | 1 / 509,256-byte payload | restored |
| Total `comprehensions` profile | main only / 71,096 bytes | 2 frames / 580,112 bytes | complete |
| Successful other-workload runtime frame reference | 508,936-byte payload | 509,256-byte comprehensions payload | differs by workload |
| `comprehensions` apply completion | fails missing `exception_matches` target | completes strict apply | restored |
| Early observer sees late callback work and complete dump | `seen=[10, 20]`; profile.bin absent | both frames; late profile calls = 2 | fixed |
| Duplicate module/path frames | exact-once required | exactly one runtime + target per path | fixed |
| Verify/apply/none mode preservation | required | separate verify, strict apply, none no-dump pass | fixed |

The expanded normal-sampling benchmark results are:

| Benchmark | Candidate paired stock mean | Candidate SOAC mean | Stock / SOAC | Existing previous SOAC median | Candidate SOAC median |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 30.0223895 ms | 80.2183813 ms | 0.3742582x | 83.2072625 ms | 79.6964280 ms |
| `comprehensions` | 7.9201417 microseconds | 89.4829336 microseconds | 0.0885101x | no completed previous SOAC result | 89.3961992 microseconds |
| `deltablue` | 1.4686720 ms | 5.9147526 ms | 0.2483066x | 4.5196838 ms | 4.7241264 ms |
| `richards` | 23.7982538 ms | 44.1826106 ms | 0.5386339x | 42.9130165 ms | 42.9617265 ms |

`comprehensions` is now successfully transformed and measured, but remains
approximately **11.30x slower than stock**; completing it is a correctness
and coverage improvement, not a performance win. Its four-workload
stock-relative geometric ratio is **0.2579951305x**. That four-benchmark
score must not be compared with the previous three-benchmark
**0.4013803170x** score because the benchmark sets differ.

The three shared-workload previous/candidate median ratios are **1.0440526x**
for `chaos`, **0.9567237x** for `deltablue`, and **0.9988662x** for
`richards`; their geometric mean is **0.9992452x**, effectively unchanged.
The `deltablue` mean is distorted by **24.9327259 ms** and **8.1246 ms**
outliers despite a **4.7241264 ms** median. Do not attribute noisy positive
or negative throughput changes to shutdown flushing; this strategy repairs
evidence completeness.

| Existing three-workload performance / code guardrail | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| `chaos` SOAC mean | 84.2816742 ms | 80.2183813 ms | noisy; use median |
| `deltablue` SOAC mean | 4.5131975 ms | 5.9147526 ms | distorted by outliers |
| `richards` SOAC mean | 43.2257645 ms | 44.1826106 ms | noisy; use median |
| Stock-relative geometric speedup | 0.4013803170x / three cases | 0.2579951305x / four cases | different sets; not comparable |
| Optimized typed-IR final basic blocks | 2,055 / three cases | 2,541 / four cases | added benchmark |
| Optimized typed-IR function instances | 167 / three cases | 193 / four cases | added benchmark |
| Pre-optimization serialized BlockPy bytes | 6,311,524 / three cases | 8,171,776 / four cases | added benchmark |
| Apply-mode native emitted bytes | 15,679,400 / three cases | 18,674,240 / four cases | added benchmark |
| Apply-mode native machine blocks | 1,038,180 / three cases | 1,236,140 / four cases | added benchmark |

Generated-code metrics above apply to the original three-benchmark set. A
candidate run including `comprehensions` naturally has additional
modules/functions. Its transformed function counts are **35 `chaos`**, **21
`comprehensions`**, **79 `deltablue`**, and **53 `richards`**; all transform
`__main__` plus `soac.runtime`, with no transformed standard-library module.
Three-versus-four-workload totals cannot be compared directly without
per-benchmark normalization.

## Attempt history

### Attempt 1: Add exact-once shutdown flushing for retained module states

- Change: snapshot all process-retained transformed states in a private
  extension `atexit` callback and emit each eligible module/output path at
  most once, while keeping the existing module-clear behavior.
- Measurements and coverage: failed baseline `comprehensions` has only the
  **71,096-byte main frame**; successful baseline workloads include a
  **508,936-byte runtime frame**. The candidate restores the runtime frame,
  producing exactly two frames totaling **580,112 bytes**: runtime payload
  **509,256 bytes** and main payload **70,776 bytes**. It preserves strict
  apply and completes actual `comprehensions` with
  **21 compiled names**, **486 typed blocks**, and **299,484 native bytes**.
  Its single-value **20.251 / 483.236 microsecond** stock/SOAC result is cold
  completion evidence, not a throughput headline. Expanded normal sampling
  completes all four workloads; `comprehensions` measures **7.920 / 89.483
  microseconds** stock/SOAC and remains **11.30x slower**. The shared-three
  median geometric ratio is **0.9992452x**. A valid **659-sample**, zero-loss
  steady-state native profile attributes **26.25% inclusive** to closure
  creation, after rejecting one lossy and one cold-compiler-contaminated
  capture. The full correctness gate passes **1,211 Python cases / 75
  batches** and all Rust crate suites.
- Compatibility and tests: planned deterministic early-observer / late-user
  callback subprocess, strongly retained runtime modules, reentrant callbacks,
  exact-once records, and separate profile/verify paths. The genuine baseline
  subprocess RED fails in **0.42 seconds**: final user work `seen=[10, 20]`
  runs, but no profile file exists while both transformed modules are still
  live. The structured path-aware/reentrant/retry Rust regression passes
  **one test / 545 filtered** in the final expanded suite, the combined Rust
  test-target check passes, and all **eight PyO3 tests** pass;
  candidate profile now exposes both frames exactly once and two late-call
  samples. Verify also exposes both frames, but its call-target counter is
  correctly zero. The corrected full shutdown fixture plus existing
  immediate-clear, none-mode, and strict replay guardrails pass **four tests
  in 5.90 seconds**; scoped formatting/checks pass.
- Result: retained after strict `comprehensions` profile/apply completion,
  actual four-workload normal sampling, exact-once semantics, and the passing
  full correctness gate. The cold smoke is not a throughput claim; full-suite
  acceptance remains unmeasured.

## Verdict and next action

- Verdict: **LANDED / RETAIN** as a correctness and benchmark-coverage fix.
  Genuine shutdown RED-to-GREEN, exact-once
  profile/verify flushing, final user counters, no-dump none mode, and strict
  apply are verified. Actual previously failing `comprehensions` completes
  with restored runtime evidence, and all four benchmarks complete normal
  sampling. The new workload remains **11.30x slower** than stock, the
  four-case paired-stock score is only **0.2579951x**, and the existing three
  workloads have no meaningful throughput shift. The full correctness gate
  passes; the complete-suite **1.10x** target remains pending. Valid native profiling
  points to **26.25% inclusive** closure creation as a future separate
  strategy.
- Transferable lesson: a successful profile process does not guarantee a
  complete multi-module evidence set; counter durability must follow explicit
  process/session lifecycle rather than accidental module-clear ordering.
- Next action: retain the validated completeness fix and investigate
  `comprehensions` closure
  instantiation, exception matching, and broader full-suite blockers as
  separate strategies.
