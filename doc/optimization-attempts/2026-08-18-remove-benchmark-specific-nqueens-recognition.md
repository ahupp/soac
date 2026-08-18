---
title: "Remove benchmark-specific N-Queens recognition"
---

# Remove benchmark-specific N-Queens recognition

- Status: landed
- Pacific date: 2026-08-18 PDT
- Change or revision: post-hot-loop working revision; not yet assigned
- Outcome: reject and remove exact-source N-Queens substitution as a
  benchmark-integrity and CPython-observability violation. Structured Rust and
  actual-source generator-tracing integration checks pass, and an honest
  one-value N-Queens smoke completes near stock parity without source
  recognition. The full correctness gate passes; near parity comes from
  faithful CPython generator fallback, not optimized generator hot code.

## Hypothesis and evidence

The optimization goal explicitly prohibits production eligibility based on a
benchmark filename, function name, precomputed result, or exact source bytes.
The preexisting opaque N-Queens path violates that policy: it compares a
module's complete source against two pinned N-Queens benchmark fixtures,
recognizes a fixed producer graph, then replaces actual permutation
enumeration, Python generator execution, and collection construction with a
hand-specialized bit-mask search.

The historical 2026-07-28 entry in `doc/PERF_LOG.md` reports stock CPython at
**118 ms**, SOAC apply at **34.8 microseconds**, and an exact-source speedup
of **3393.97x**. Those numbers are historically recorded; they are not a
valid reusable-optimization result and must not appear in a current
full-suite acceptance score.

For an equally weighted geometric mean, a single benchmark ratio contributes
`ratio ** (1 / benchmark_count)` even when every other benchmark ratio is
unchanged. The historical **3393.97x** recognition therefore multiplies a
97-benchmark suite score by **1.0874243x**, or **8.7424%**, and an
80-benchmark suite score by **1.1069649x**, or **10.6965%**. At 80
benchmarks, this one invalid substitution exceeds the entire 10%-faster-than-
CPython goal even if no other workload improves.

The same code silently breaks observable Python behavior. The old N-Queens
integration cases install `sys.settrace`, confirm that directly consuming
`n_queens` produces generator call/line events, then assert that consuming
the same source-backed generator through the optimized benchmark produces no
events. Active tracing, profiling, debugger, and monitoring observers cannot
see the erased source-backed generator activations. The optimization also
overrides the normal module-dictionary layout for its two recognized source
files and bypasses original generator/frame, allocation, and safepoint
behavior without an approved production compatibility exception.

Historical evidence confirms that an honest source-backed-generator path
already existed. The 2026-05-01 entry in `doc/PERF_LOG.md` records SOAC
N-Queens at **104 ms** versus stock CPython at **94.2 ms** while preserving
original named-generator CPython vectorcall. A separate 2026-07-26 entry
reports repeated native medians of **8.431 runs/s** for SOAC versus
**8.468 runs/s** for CPython, approximately **0.44% slower**, specifically by
retaining source-backed generator execution. These are prior historical
snapshots, not measurements of the present candidate; the honest current
single-value N-Queens result is reported separately below.

## Implementation and compatibility

- Remove the exact-source production admission and both pinned N-Queens source
  fixtures. Production optimization eligibility must not inspect benchmark
  source equality or carry an N-Queens fingerprint in shared module state.
- The concrete JIT change deletes the 2,484-line
  `crates/soac_jit/src/jit/opaque_fused_iteration.rs` implementation and both
  `opaque_fused_nqueens_v1.py` /
  `opaque_fused_pyperformance_nqueens_v1.py` pinned fixtures. Shared-state,
  module-layout, typed-admission, codegen, import, and JIT-symbol cleanup is
  limited to `crates/soac_jit/src/module_type.rs` and
  `crates/soac_jit/src/jit/{typed_pipeline,mod,imports,symbols,runtime_context}.rs`;
  `runtime_context.rs` also drops the now-unused `PY_FUNCTION_VERSION_OFFSET`
  and `PY_CODE_VERSION_OFFSET` constants previously needed by the opaque
  guard sequence.
- Remove the apply-mode opaque root replacement, its benchmark-specific typed
  codegen path, and JIT registration of the specialized affine/bit-mask helper.
  Do not introduce a replacement environment switch or another default-on
  benchmark-recognition route.
- Remove the recognized-source exception that forces an indexed globals
  dictionary. Every source-backed named-generator module follows the ordinary
  apply-mode Unicode globals-dictionary policy and keeps its original CPython
  generator vectorcall. Existing profile/verify module policy remains
  unchanged.
- Keep real source-independent compiler mechanisms: validated direct calls,
  exact-int regions, typed inlining, closure and generator state,
  ordinary closed `list`/`tuple` iterator pipelines, and existing generic
  optimization-plan/IR/runtime ABI representations. Do not discard reusable
  infrastructure merely because a benchmark-specific admission was removed.
- Preserve actual Python generator activations, materialized solution tuples
  in encounter order, `None` returns for discarded results, rebinding,
  defaults, exceptions, and native generator-call/version semantics.
  `sys.settrace` callbacks for source-backed producer calls must remain
  observable; supported observers must not silently miss those activations.
- Replace pinned-fixture behavioral checks with tests against the installed
  pyperformance source or source-independent generic fixtures. Add explicit
  assertions for ordinary module globals, real generator tracing, and correct
  profile-to-apply results. Exact-source admission tests should be removed
  with the removed production path, not kept as compatibility scaffolding.
- The rewritten integration cases are
  `test_profiled_full_nqueens_slice_preserves_results_tracing_and_mutations`
  and `test_profiled_pyperformance_nqueens_preserves_generator_tracing_and_rebinding`.
  Both now require genuine `call` and `line` tracing callbacks from both
  `n_queens` and `permutations`, rather than asserting an empty event list.
  They preserve the complete width-0-through-8 count sequence, width 9,
  mutated defaults, replaced vectorcall, rebinding, and original-result
  checks. The actual installed pyperformance source replaces the pinned
  fixture; only the traced pyperformance test width is reduced from 8 to 4
  to keep focused diagnostics practical. Both real runtime cases pass along
  with all eight pyperformance sitecustomize regressions: **10 tests passed
  in 64.67 seconds**. The two N-Queens cases take 41.51 and 23.07 seconds,
  respectively.
- A new structured Rust regression,
  `source_backed_generator_consumers_do_not_receive_opaque_substitutions`,
  checks equivalent source/comment variants and rejects any source-backed
  opaque substitution. The initial `cargo check -p soac_jit --tests` passes
  after production recognition and fixtures are removed, and the structured
  regression passes one test with 542 filtered. The subsequent warning-free
  `cargo check -p soac_jit --tests`, retained
  `profile_mode_preserves_countered_hot_loop_ids_while_apply_can_split`
  regression, and package-scoped formatting/checks also pass.
- `doc/PERF_LOG.md` remains an unchanged historical record. Its 2026-07-28
  measurements describe a removed implementation rather than current
  production behavior.
- Statistically representative N-Queens throughput and full-suite performance
  remain pending. Warning-free JIT test-target compilation, the new structured
  no-substitution regression, the existing hot-loop profile-instruction-ID
  regression, package formatting checks, both source-backed N-Queens behavior
  cases, all eight sitecustomize regressions, and an honest release-mode
  single-value N-Queens smoke have passed. The complete `just test-all`
  correctness gate also passes.

## Benchmark protocol and coverage

- Baseline fixed exploratory selection: `chaos,richards,deltablue`; this
  subset intentionally excludes the benchmark-specific N-Queens shortcut.
- Baseline normal-sampling comparison artifact:
  `work/pyperformance/comparison-20260818-132520-fxTXzI/summary.json`.
  The comparison used one independently started stock/profile/apply round
  and 20 measured values per benchmark.
- Baseline transformed modules for each workload: its `__main__` benchmark
  module and `soac.runtime`; no standard-library or third-party dependency
  module was transformed.
- Baseline compiled apply functions: 35 for `chaos`, 79 for `deltablue`, and
  53 for `richards`; 167 across the fixed subset.
- Candidate selection should add an independently runnable actual
  pyperformance `nqueens` check without changing the full-suite acceptance
  definition. A final score must exclude source-identity substitutions and
  report stock, prior SOAC, changed SOAC, transformed modules, and actual
  source-backed generator behavior.
- Historical N-Queens 3393.97x and 104/94.2 ms figures come from prior
  checked-in `doc/PERF_LOG.md` entries. They are not paired with the current
  interpreter, hardware, VM, code revision, or candidate.
- Honest actual-source N-Queens release smoke artifact:
  `work/pyperformance/comparison-20260818-140910-BtgC9F/summary.json`.
  This used `--debug-single-value`; it verifies source-independent completion
  and approximate parity, not statistical significance or the final
  full-suite performance target.
- Representative N-Queens sampling, a new fixed-subset comparison, and
  full-suite completion: pending.
- Completed full-gate artifact: `work/logs/nqueens-integrity-test-all.log`.
  `just test-all` passes all 1,208 Python node IDs across 72 batches and the
  complete Rust workspace, including 543 `soac_jit`, 367 `soac_lowering`,
  202 `soac_opt`, and eight PyO3-extension tests. The successful test phase
  takes 188.938 seconds, including a 107.39-second counter-dump batch.

## Measurements

| Historical N-Queens mode | Stock CPython | SOAC apply | Interpretation |
| --- | --- | --- | --- |
| 2026-07-28 pinned-source bit-mask substitution | 118 ms | 34.8 microseconds | reported 3393.97x; invalid benchmark-specific score |
| 2026-05-01 ordinary source-backed generators | 94.2 ms | 104 ms | honest historical generator path; not current |
| 2026-07-26 ordinary source-backed generators | 8.468 runs/s | 8.431 runs/s | 0.44% below stock; not current |
| Current post-removal ordinary generator path | 42.4562010 ms | 40.8079650 ms | 1.0403901x; one-value smoke, not statistically significant |

The current result comes from the actual installed pyperformance source,
without a pinned-source match, replacement search algorithm, or special
module dictionary. It exercises ordinary source-backed named generators and
their real Python tracing behavior. The single observed 1.0403901x ratio is
not a measured optimization win, is not directly comparable with historical
hardware/runs, and does not establish full-suite acceptance; it does show that
removing the artificial 3393.97x shortcut does not force the current
N-Queens workload into catastrophic runtime. It is faithful CPython-fallback
parity, not evidence that SOAC optimized the generator hot path.

| Fixed baseline benchmark | Stock mean | SOAC apply mean | Stock / SOAC | Previous-SOAC interpretation |
| --- | --- | --- | --- | --- |
| `chaos` | 30.2397584 ms | 102.6065045 ms | 0.2947158x | baseline; candidate pending |
| `deltablue` | 1.5436580 ms | 5.0575092 ms | 0.3052210x | baseline; candidate pending |
| `richards` | 44.2515538 ms | 83.3514952 ms | 0.5309029x | baseline; candidate pending |

The historical baseline summary's fixed-subset stock-relative geometric mean
is **0.3628087x**. Severe VM scheduling outliers distort its arithmetic
means: the corresponding SOAC medians are 100.6145265 ms, 4.6892690 ms,
and 43.2001030 ms for `chaos`, `deltablue`, and `richards`. In particular,
`richards` stock samples reached 147.0885750 ms and SOAC samples reached
444.0347900 ms. Use robust paired distributions and fresh same-revision
profiles when assessing changes; do not present noisy means as exact effects.

| Generated-code metric | Fixed-subset baseline | Post-removal candidate | Change |
| --- | --- | --- | --- |
| Optimized typed-IR final basic blocks | 1,897 | pending | pending |
| Optimized typed-IR function instances | 167 | pending | pending |
| Pre-optimization serialized BlockPy bytes | 6,311,140 | pending | pending |
| Apply-mode native emitted bytes | 14,288,640 | pending | pending |
| Apply-mode native machine blocks | 949,320 | pending | pending |

These baseline totals include repeated worker compilation. The full-suite
1.10x target is not met or measured by this fixed subset.

The honest single-value N-Queens candidate transforms `__main__` and
`soac.runtime`, with no transformed standard-library module. It reports three
distinct compiled function names: `bench_n_queens`,
`n_queens.<locals>.<genexpr>`, and
`permutations.<locals>.<genexpr>`. The source-backed named `n_queens` and
`permutations` generators themselves remain on their ordinary CPython
vectorcall path, so the benchmark completes without transformed hot-generator
coverage. Aggregate apply generation contains five optimized typed-function
instances, 117 final typed-IR basic blocks, 868,294 serialized BlockPy bytes,
99,496 native emitted bytes, and 6,874 native machine blocks.

The same smoke exposes a separate profiling workflow bottleneck: its profile
measured value takes approximately **17.4 seconds**, while the honest apply
value is only **40.8079650 ms**. Worker setup is approximately **362.9 ms**
for profile and **544.6 ms** for apply; the release rebuild costs another
**20.70 seconds**. Per-operation specialization counters make profiling much
more expensive than optimized execution. This cost must be considered before
running broad N-Queens-heavy comparisons, but profile time must never be
misreported as apply throughput.

Profile and apply coverage differ materially. The profile process JITs **13
function names**, including both source-backed named generators,
`ClosureGenerator`, and runtime code, and emits **242,020 bytes** of native
code. The apply process JITs only the **three** benchmark/inner-genexpr names
listed above and emits **99,496 bytes**; its named generator hot bodies run
as original CPython functions. This asymmetric execution explains why the
17.4-second profiled path cannot be interpreted as the behavior of the
40.8-millisecond apply path, and why the exploratory 1.04x result does not
demonstrate JIT optimization of source-backed generators.

## Attempt history

### Attempt 1: Identify and reject benchmark-identity substitution

- Change: inspected the pinned-source recognizer, opaque N-Queens admission,
  forced indexed module-dictionary exception, tracing assertions, historical
  performance entries, and current optimization policy.
- Measurements and coverage: historical 3393.97x artificial speedup inflates
  97-suite and 80-suite geometric means by 8.7424% and 10.6965%,
  respectively. Historical ordinary-generator paths are 104 ms versus
  94.2 ms and 8.431 versus 8.468 runs/s.
- Compatibility and tests: existing benchmark tests explicitly assert that an
  active tracer receives no source-backed producer events along the optimized
  path, demonstrating silent observer loss.
- Result: **reject** exact-source N-Queens specialization as invalid
  production benchmark recognition and unapproved semantic divergence.
- Reason: benchmark-specific eligibility, algorithm substitution, module
  layout exception, and missing callbacks contradict `OPT_GOAL.md`.

### Attempt 2: Restore the source-independent production generator path

- Change: remove pinned source/fixture recognition, shared-state fingerprint,
  benchmark-specific module layout, opaque apply admission, scalar codegen,
  and JIT helper registration while retaining generic plans and ABI support.
- Measurements and coverage: paired current N-Queens throughput, ordinary
  generator/frame execution, generic subset deltas, transformed coverage,
  and generated-code impact are now available from the single-value actual
  pyperformance smoke: 42.4562010 ms stock versus 40.8079650 ms SOAC, with
  real source-backed generator execution, three distinct compiled names, 117
  typed blocks, and 99,496 native emitted bytes. The 1.0403901x result is
  exploratory and statistically inconclusive; a representative subset
  comparison remains pending.
- Compatibility and tests: update source-independent behavior tests to
  verify original generator tracing, normal globals, complete result tuples,
  rebinding/default behavior, and profile-to-apply execution. The two renamed
  integration tests now require real call and line callbacks from both source
  generators and read the installed pyperformance benchmark; host syntax
  validation passes. The structured
  `source_backed_generator_consumers_do_not_receive_opaque_substitutions`
  regression checks source/comment variants. `cargo check -p soac_jit
  --tests` passes without warnings, the structured Rust regression passes one
  test with 542 filtered, the retained hot-loop profile-instruction-ID
  regression passes, and package formatting checks pass. Both renamed
  N-Queens behavior tests and all eight sitecustomize regressions pass:
  **10 tests in 64.67 seconds**. Actual `call` and `line` callbacks fire for
  both `n_queens` and `permutations`; counts for widths 0 through 9, mutated
  defaults, vectorcall changes, global rebinding, and profile-to-apply
  execution remain correct.
- Result: benchmark recognition is removed, genuine generator tracing is
  restored, and an honest one-value N-Queens run completes near stock parity.
  Do not claim a throughput win from statistically insufficient data; the
  full `just test-all` gate passes 1,208 Python node IDs across 72 batches
  together with all Rust workspace tests.

## Verdict and next action

- Verdict: exact-source N-Queens recognition is **REJECTED** as a production
  strategy. Its removal, structured no-substitution test, actual-source
  tracing regressions, honest fallback-parity smoke, and full correctness
  gate are verified. This is a landed benchmark-validity/correctness fix, not
  a demonstrated generator-hot-path optimization or completion of the 1.10x
  full-suite performance goal.
- Transferable lesson: one extreme benchmark-specific outlier can dominate a
  geometric-mean acceptance score while simultaneously hiding broken
  tracing, generator, and module-dictionary semantics. Benchmark completion
  and a huge ratio are not evidence of a general compiler optimization.
- Next action: obtain representative statistically valid stock/SOAC
  measurements while managing the observed 17.4-second profiling overhead.
  Distinguish faithful CPython generator fallback from transformed hot-path
  coverage, and evaluate full-suite progress only after benchmark-specific
  substitutions are absent.
