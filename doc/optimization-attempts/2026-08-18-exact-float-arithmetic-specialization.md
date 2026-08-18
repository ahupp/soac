---
title: "Exact-Float Arithmetic Specialization"
---

# Exact-float arithmetic specialization

- Status: rejected
- Pacific date: 2026-08-18 PDT
- Change or revision: baseline `main` revision `f49d7d64`; rejected working
  candidate measured against the same previous SOAC revision
- Outcome: guarded exact-float arithmetic passed focused compatibility checks
  but produced no `chaos` improvement, regressed the three-benchmark
  previous-SOAC geometric mean, and increased native code size. Reject the
  implementation and retain this negative strategy record.

## Hypothesis and evidence

General-purpose numeric Python workloads repeatedly operate on ordinary exact
`float` objects. Replacing eligible boxed Python-number dispatch with guarded
native floating-point arithmetic should improve realistic numeric code without
recognizing benchmark names, function names, source bytes, or expected output.

The baseline binary-operator profiling and planning path recognized exact
`int` and exact `str`, but not exact `float`. Its runtime shape recorder
therefore classified floating operands as unsupported shape zero, preventing
the baseline profile-guided typed planner from selecting a float-specific
operation.

Baseline profiling observed approximately 1.17 million operator samples,
including 851,723 unsupported shape-zero samples and 701,730 hot floating-point
operations. The mixed `chaos` workload includes vector and spline arithmetic;
its compiled functions include `GVector.__add__`, `GVector.__sub__`,
`GVector.__mul__`, and `GVector.linear_combination`. These findings identify a
reusable exact-type specialization opportunity rather than a benchmark-specific
substitution.

A subsequent symbol-only native `cpu-clock` capture, with detailed basic-block
maps disabled, recorded 1,123 samples with zero lost samples. Its overlapping
inclusive call paths attribute 87.62% to `py_vectorcall_hook`, 77.20% to
`Chaosgame.transform_point`, 62.60% to generic `_PyObject_MakeTpCall`, 61.62%
to `Spline.__call__`, and 10.24% to `GVector.linear_combination`.
`soac_jit_make_function_with_closure` accounts for 15.05% on a nested path;
within `GVector.linear_combination`, `PyNumber_Multiply` accounts for 1.25%
and `PyNumber_Add` for 0.53%. These are overlapping call-tree percentages, not
additive exclusive shares. Call dispatch, closure creation, and generic
attribute access may therefore limit or outweigh the attainable benefit of
arithmetic specialization despite the large float sample count.

The separate exclusive-self report attributes 5.97% to
`_PyObject_GenericGetAttrWithDict`, 3.83% to
`_PyObject_TryGetInstanceAttribute`, 3.03% to `py_vectorcall_hook`, and 2.94%
to SOAC argument binding. Exclusive `PyNumber_Subtract`, `PyNumber_Add`, and
`PyNumber_Multiply` account for only 0.71%, 0.62%, and 0.53% respectively;
`float_mul` accounts for 0.62% and `float_add` for 0.36%. These measurements
make a modest, neutral, or negative arithmetic-only result plausible and point
to attribute lookup and call binding as separate follow-up strategies.

Expected effect: improve measured `chaos` apply throughput while preserving or
improving the fixed three-benchmark geometric mean, avoiding material
`deltablue`/`richards` regressions, and controlling optimized typed-IR and
generated native-code growth.

## Implementation and compatibility

- Implementation shape: add `ExactTypeTag::Float = 3`, recording an exact
  float/float pair as packed shape `771`. Derive source-keyed v3
  `ExactFloatBinarySpecializationPlan` decisions only for profiled `Add`,
  `Sub`, and `Mul`; mechanically emit those validated decisions and attach a
  `TypedExactFloatBinaryPlan` to the matching typed operation.
- Evaluate both original operands once, then guard their actual object types
  against `PyFloat_Type`. On the hot path, load `PyFloatObject.ob_fval`, emit
  native `f64` add/sub/mul, and box the owned result with
  `PyFloat_FromDouble`. A type-guard miss runs the original
  `PyNumber_Add`, `PyNumber_Subtract`, or `PyNumber_Multiply` operation using
  the already evaluated operands.
- Exact-type guards reject subclasses and mixed operand types before any
  optimization-dependent visible effect. Unsupported operators, including
  division, remain generic; stale or mismatched source/operator decisions are
  rejected before code generation.
- Preserve CPython evaluation order, operand ownership/reference counts,
  exceptions, special-method dispatch, subclass and reflected-operator
  behavior, and result allocation/materialization. Validate NaN, infinities,
  signed zero, overflow behavior, and divide-by-zero semantics for every
  operation that is actually admitted; do not assume that hardware arithmetic
  alone establishes Python equivalence.
- Mutable assumptions apply only to guarded exact built-in float objects and
  the supported operation. Do not cache subclass dispatch, user methods, or
  stale object/type identity without an explicit validated guard lifetime.
- Source changes include structured exact-float shape round-trip, nested
  operation selection, mixed/unsupported rejection, plan validation,
  mechanical emission, typed-sidecar, and result-fact regressions.
  `cargo check -p soac_ir_typed -p soac_opt --tests` passed, and
  `cargo test -p soac_ir_typed -p soac_opt --lib` passed all 258 tests
  (52 `soac_ir_typed` plus 206 `soac_opt`), including nine focused exact-float
  regressions. `cargo check -p soac_jit --tests` also passed. The focused
  structured JIT regression
  `specialized_jit_opt_v3_exact_float_arithmetic_emits_machine_path_and_fallback`
  passed, proving one selected native `Fadd`, `Fsub`, or `Fmul` operation,
  `PyFloat_FromDouble` boxing, and the original generic fallback for each
  operator; the optimizer-wrapper regression
  `single_function_planning_preserves_profiled_exact_float_binary_operations`
  also passed. Both `just pytest-fast tests/test_exact_float_specialization.py
  -q` and the combined focused check
  `just pytest-fast tests/test_select_pyperformance_worker.py
  tests/test_exact_float_specialization.py -q` passed; the latter ran four
  tests. They validate end-to-end profile-to-apply execution, observed
  float/float shape 771, nested operations, subclass/reflected/mixed/string/int
  fallback, signed zero, NaN, infinity, unchanged division and
  `ZeroDivisionError`, exactly-once left-to-right operand evaluation, and
  exclusion of actual `--calibrate-loops` workers from measured-worker
  selection. These passing checks establish compatibility, not a performance
  improvement; the normal-sampling candidate was rejected below.

## Benchmark protocol and coverage

- Fixed exploratory selection: `chaos,richards,deltablue`; this representative
  subset is not the full-suite acceptance criterion.
- Baseline command: `just pyperformance-compare chaos,richards,deltablue 1`;
  one independently started stock/SOAC round using the normal default sampling
  mode, not `--debug-single-value` and not native `perf`.
- Previous SOAC baseline revision: `f49d7d64`.
- Previous SOAC baseline artifact:
  `work/pyperformance/comparison-20260818-102635-eV9bjf/summary.json`.
- Candidate artifact:
  `work/pyperformance/comparison-20260818-120758-cw6Nzc/summary.json`.
  Both baseline and candidate used one normal-sampling comparison round.
  `pyperf` did not consider the before/after differences statistically
  significant; a three-round confirmation was not justified after the
  candidate showed no improvement and added native code.
- Detailed basic-block native profile: unavailable. Enabling `SOAC_JIT_BB_MAP=1`
  spent 123.04 seconds in `jit_commit_bb_map_us` across only 30 emitted
  functions, including 21.83 seconds for one function, and the replay timed
  out before measured-value readiness. The writable VM mount makes detailed
  per-basic-block map emission prohibitively slow. Recorded profile samples
  remain usable, although a separate hot-loop counter-coverage gap needs
  follow-up.
- Symbol-only native profile: `work/logs/chaos-float-baseline-symbols_record.txt`
  and `work/logs/chaos-float-baseline-symbols_by_dso_symbol.txt`; exclusive
  self samples are in `work/logs/chaos-float-baseline-symbols-self.txt`. The
  capture contains 1,123
  `cpu-clock` samples, zero lost samples, and a 70.220 MB recording. Disabling
  detailed basic-block map emission avoids the writable-mount bottleneck while
  retaining native and JIT symbol attribution. The capture is diagnostic
  evidence only and does not replace unprofiled headline timing.
- Each candidate SOAC revision must generate fresh profile evidence and use
  the same stock interpreter, inputs, module-selection policy, and sampling
  configuration as its comparison baseline.
- Completed baseline benchmarks: `chaos`, `deltablue`, and `richards`;
  baseline failures: none within the selected subset. Full-suite completion
  and aggregate: not measured.
- All three benchmarks transform their `__main__` benchmark module and
  `soac.runtime`. No standard-library modules or third-party dependencies are
  transformed for this selection.
- Compiled apply functions: `chaos` 35, `richards` 53, and `deltablue` 79;
  167 across the selected benchmark-specific coverage records.
- Separate integrity blocker: a default-enabled exact-source N-Queens
  substitution violates the benchmark-specificity policy. `nqueens` is not in
  this exploratory selection, and that unrelated shortcut must not be counted
  as progress for this strategy or any full-suite acceptance claim.

## Measurements

| Benchmark | Baseline stock | Previous SOAC apply | Candidate stock | Candidate SOAC apply | Previous / candidate SOAC | Candidate elapsed change |
| --- | --- | --- | --- | --- | --- | --- |
| `chaos` | 58.1008610 ms | 99.2156843 ms | 30.0613919 ms | 99.2387103 ms | 0.9997680x | +0.023% |
| `deltablue` | 1.5724309 ms | 4.6591915 ms | 1.5202782 ms | 4.7115277 ms | 0.9888919x | +1.12% |
| `richards` | 22.5002146 ms | 45.3569023 ms | 22.0921291 ms | 48.8091671 ms | 0.9292702x | +7.61% |

The previous-SOAC geometric mean is **0.9721427x**, equivalent to approximately
**2.87% slower elapsed time** across the fixed subset. The candidate's
same-run stock-relative geometric mean is **0.3536784x**. Do not compare it
directly with the baseline's 0.4611075x stock-relative mean: stock `chaos`
shifted from 58.1009 ms to 30.0614 ms between runs, a 48.3% timing drift.
Compare candidate and previous SOAC elapsed values directly instead. `pyperf`
reports that the individual differences are not statistically significant;
the rejection reflects the absence of a demonstrated win together with code
growth, not a claim of a statistically established slowdown.

| Generated-code metric | Previous SOAC baseline | Candidate | Change |
| --- | --- | --- | --- |
| Optimized typed-IR final basic blocks | 1,862 | 1,862 | unchanged |
| Optimized typed-IR function instances | 167 | 167 | unchanged |
| Pre-optimization serialized BlockPy bytes | 6,311,140 | 6,311,140 | unchanged |
| Apply-mode native emitted bytes | 13,977,840 | 14,022,160 | +0.317% |
| Apply-mode native machine blocks | 929,520 | 931,630 | +0.227% |

| Deduplicated `chaos` apply function | Previous native bytes | Previous machine blocks | Candidate native bytes (smoke) | Candidate machine blocks (smoke) |
| --- | --- | --- | --- | --- |
| `GVector.linear_combination` | 14,548 | 1,041 | 15,700 (+7.92%) | 1,098 (+5.48%) |
| `GVector.__mul__` | 2,772 | 192 | 2,772 (unchanged) | 192 (unchanged) |
| `GVector.__add__` | 9,396 | 631 | 9,808 (+4.38%) | 647 (+2.54%) |
| `Spline.__call__` | 90,428 | 6,035 | 90,428 (unchanged) | 6,035 (unchanged) |
| `Chaosgame.transform_point` | 45,200 | 3,000 | 46,608 (+3.12%) | 3,063 (+2.10%) |

These deduplicated per-function baselines make type-check and native-code
bloat visible independently of repeated worker compilation. Candidate values
come from a preliminary `--debug-single-value` smoke run, not a representative
timing comparison; they establish emitted code growth but do not establish
steady-state performance. Do not mistake larger guarded-operation emission
for an improvement merely because profile evidence identified floats.

The generated-code totals aggregate repeated compilation/reporting of the same
modules. Worker timing files record ten measured apply-worker processes for
each benchmark, or 30 measured worker invocations across the selection; the
reported sizes must not be interpreted as deduplicated per-benchmark code size
or a unique resident native-code footprint. The result summary uses
`median_per_round` aggregation. Its reported worker count of one per benchmark
identifies one stable worker directory, not one operating-system worker
process.

Measured-worker setup is substantial but is separate from the pyperformance
headline value: median setup is 2.196 seconds for `chaos`, 2.693 seconds for
`richards`, and 3.251 seconds for `deltablue`. Median measured-worker spans
are 0.200, 0.171, and 0.148 seconds respectively. Those setup durations must
not be silently included in or confused with the steady-state elapsed results.

## Attempt history

### Attempt 1: Establish normal-sampling baseline and identify missing float shapes

- Measured baseline `f49d7d64` using one normal-sampling comparison of
  `chaos,richards,deltablue`; all three stock, profile, and apply passes
  completed.
- `chaos`: 58.100861 ms stock, 99.215684 ms SOAC apply, 0.5856016x stock/SOAC.
- `deltablue`: 1.5724309 ms stock, 4.6591915 ms SOAC apply, 0.3374901x.
- `richards`: 22.5002146 ms stock, 45.3569023 ms SOAC apply, 0.4960704x.
- Exploratory fixed-subset geometric mean: 0.4611075x; the 1.10x full-suite
  acceptance target is neither met nor measured.
- Approximately 1.17 million operator samples include 851,723 shape-zero
  samples and 701,730 hot float operations. Baseline exact-type tags
  recognized only integer and string operands.
- Detailed native profiling could not complete: enabling the basic-block map
  cost 123.04 seconds across 30 emitted functions, with one function taking
  21.83 seconds, before the measured-value readiness timeout.
- A symbol-only native `cpu-clock` capture subsequently completed with 1,123
  samples and zero lost samples after disabling the detailed basic-block map.
  The overlapping inclusive call tree is dominated by Python call dispatch and
  `Spline.__call__`; float-number dispatch within `GVector.linear_combination`
  is observable but comparatively smaller.
- Result: numeric specialization hypothesis supported by baseline profile and
  native-symbol evidence; no candidate result existed for this baseline pass.

### Attempt 2: Add guarded exact-float evidence and arithmetic lowering

- Change: implemented exact-float tag `3`/packed shape `771`, explicit
  source-keyed v3 add/sub/mul decisions, mechanical emission, typed sidecars,
  exact `PyFloat_Type` guards, direct `ob_fval` `f64` arithmetic,
  `PyFloat_FromDouble` boxing, and the original numeric-operation fallback.
- Measurements and coverage: the representative candidate and previous-SOAC
  comparison are recorded in Attempt 4; preliminary generated-code growth is
  recorded in Attempt 3.
- Compatibility and tests: the experimental source represented exact-type
  guards and the original generic fallback. The typed-IR and optimizer test-target check
  passed, and their 258 library tests passed, including nine focused exact-float
  shape, planner, typed-sidecar, emission, nested-operation, mixed-shape, and
  value-fact regressions. The JIT test-target check, structured native-emission
  and generic-fallback regression, optimizer-wrapper regression, and four
  focused Python integration tests also passed. End-to-end runtime coverage
  includes exceptional float values, subclasses, reflected operators, mixed
  operands, unchanged division errors, and exactly-once evaluation order.
- Result: implementation and focused compatibility validation completed;
  performance acceptance required the normal-sampling comparison in Attempt 4.

### Attempt 3: Preliminary single-value release smoke

- Artifact:
  `work/pyperformance/comparison-20260818-120151-jx3jLJ/summary.json`.
- This `chaos` run used `--debug-single-value` and was affected by cold-start
  costs. Its 30.7738 ms stock and 253.394 ms SOAC apply values (0.1214x) are
  smoke evidence only, not headline timing and not comparable to the
  normal-sampling baseline.
- Coverage: 35 transformed apply functions, 991,150 serialized BlockPy bytes,
  and 846 optimized typed-IR blocks. Candidate profiling recorded 841,723
  actual exact-float/float shape-771 samples: 701,730 in
  `GVector.linear_combination`, 60,000 in `Chaosgame.transform_point`, 30,000
  in `GVector.Mag`, 20,000 in `Chaosgame.truncate`, 15,000 in
  `GVector.__add__`, 14,985 in `GVector.dist`, six in `GVector.__init__`, and
  two in `Chaosgame.create_image_chaos`.
- Apply-mode native code: 722,120 bytes versus 717,688 bytes in the previous
  comparable quick baseline (+0.62%); 47,939 machine blocks versus 47,728
  (+0.44%). The deduplicated function table above shows concentrated growth in
  `GVector.linear_combination` (+7.92% bytes, +5.48% blocks),
  `GVector.__add__` (+4.38%, +2.54%), and
  `Chaosgame.transform_point` (+3.12%, +2.10%). `Spline.__call__` and
  `GVector.__mul__` did not change in this smoke run.
- Result: release execution and coverage were confirmed, and emitted-code
  growth was quantified; this cold single-value run established no
  steady-state throughput conclusion.

### Attempt 4: Normal-sampling comparison and rejection

- Artifact:
  `work/pyperformance/comparison-20260818-120758-cw6Nzc/summary.json`.
- `chaos`: 30.0613919 ms current stock and 99.2387103 ms candidate SOAC;
  previous SOAC was 99.2156843 ms, for 0.9997680x previous/candidate
  throughput and 0.023% slower elapsed time.
- `deltablue`: 1.5202782 ms current stock and 4.7115277 ms candidate SOAC;
  previous SOAC was 4.6591915 ms, for 0.9888919x previous/candidate and
  approximately 1.12% slower elapsed time.
- `richards`: 22.0921291 ms current stock and 48.8091671 ms candidate SOAC;
  previous SOAC was 45.3569023 ms, for 0.9292702x previous/candidate and
  approximately 7.61% slower elapsed time.
- Fixed-subset geometric mean versus previous SOAC: 0.9721427x, or
  approximately 2.87% slower elapsed. Current-run geometric mean versus stock:
  0.3536784x. Baseline-to-candidate stock comparisons are invalid because the
  stock `chaos` measurement shifted by 48.3%; `pyperf` does not report the
  individual differences as statistically significant.
- Coverage stayed at 167 compiled function instances and 1,862 optimized
  typed-IR blocks. Native code increased from 13,977,840 to 14,022,160 bytes
  (+0.317%), and machine blocks increased from 929,520 to 931,630 (+0.227%).
- Result: **rejected**. Exact float arithmetic added guards and generated code
  without improving its intended `chaos` workload; the broader subset did not
  improve. Revert the implementation while retaining this strategy history.

## Verdict and next action

- Verdict: **rejected**. The fixed-subset previous-SOAC geometric mean was
  0.9721427x and native-code size increased by 0.317%, while the target numeric
  workload was effectively unchanged. Remove the implementation; retain the
  negative measurement and compatibility history.
- Transferable lesson: an unsupported profiling shape can hide a large
  general-purpose numeric hotspot even when the operation is already counted.
  Benchmark completion, profile sample volume, selected typed fast paths, and
  passing semantic tests do not prove a throughput improvement. Compare
  previous and candidate SOAC directly when stock timings drift, and monitor
  both aggregate and deduplicated native-code growth.
- Next action: pursue hot-loop profiling-counter coverage as a separate
  strategy; address the unrelated N-Queens benchmark-integrity blocker before
  claiming full-suite progress.
