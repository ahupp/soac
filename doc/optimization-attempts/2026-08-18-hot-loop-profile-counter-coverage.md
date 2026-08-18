---
title: "Hot-loop profile counter coverage"
---

# Hot-loop profile counter coverage

- Status: landed
- Pacific date: 2026-08-18 PDT
- Baseline revision: `f49d7d64` on `main`
- Outcome: retain the original-instruction-ID and typed-liveness repairs as a
  correctness/profile-evidence prerequisite, not a throughput optimization.
  Three-benchmark robust medians are approximately 1–2% slower, generated
  code grows, and severe VM outliers make mean-based comparisons unreliable;
  the corrected full `just test-all` gate passes all 1,209 Python cases in
  72 batches and the full Rust workspace after repairing two initially
  exposed preserved-cell profile-mode failures.

## Hypothesis and evidence

Profile-guided specialization can only optimize source operations for which
the profile pass records representative evidence. The baseline SOAC revision
assigns profile counters to original semantic instruction IDs before typed JIT
planning, but then applies typed optimization rewrites during the profile pass.
Even when profile mode has no eligible inline targets, an idle rewrite
iteration can clone a hot continuation and renumber every instruction in the
clone. The frequently executed clone then has no matching source-keyed
counters, while the countered original continuation remains cold.

The observed zero-counter problem affects distinct workloads and operation
families, rather than one benchmark-specific shape:

| Benchmark | Function | Zero candidate sites / candidate sites |
| --- | --- | --- |
| `chaos` | `Chaosgame.create_image_chaos` | 25 / 41 |
| `chaos` | `Spline.__call__` | 25 / 73 |
| `chaos` | `Chaosgame.get_random_trafo` | 25 / 40 |
| `chaos` | `Spline.GetIndex` | 17 / 26 |
| `deltablue` | `projection_test` | 30 / 58 |
| `deltablue` | `Planner.remove_propagate_from` | 21 / 42 |
| `deltablue` | `chain_test` | 19 / 42 |
| `deltablue` | `Planner.extract_plan_from_constraints` | 12 / 18 |
| `richards` | `Richards.run` | 49 / 53 |
| `richards` | `WorkTask.fn` | 15 / 34 |

Zero counts alone do not prove a bug: some source branches and functions are
genuinely cold. The affected hot-loop cases are corroborated by nonzero parent
entry/header sites, hot calls into nested functions, original cold instruction
IDs, and evidence that the clone remapped the executed continuation. An
independent structured Rust regression confirms that profile planning expands a
joined-loop fixture from 24 original basic blocks to 37, adding 13 cloned
blocks while the counters remain attached to the original instruction IDs.

For `Chaosgame.create_image_chaos`, pre-loop counter sites fire twice and
nested `transform_point` counters fire 10,000 times, but all candidate sites
in the parent hot loop remain zero. Its 25 zero sites include eight operators,
seven attribute accesses, five calls, one item load, one item store, and three
branches. `Spline.__call__` is not wholly unprofiled: its outer/header sites
record tens of thousands of observations and the nested
`GVector.linear_combination` path fires 137,970 times, while its inner-loop
counter sites remain zero. `Richards.run` has 34 missing call sites and ten
missing attribute sites among its 49 zero candidates.

The exact baseline source path is:

1. `crates/soac_driver/src/lib.rs` defines typed profile counter definitions
   from the original lowered module in `finish_pre_optimization_module`.
2. `crates/soac_jit/src/jit/typed_pipeline.rs` deliberately disables static
   direct-call targets in profile mode, but the baseline still calls
   `apply_typed_v3_module_rewrites` because a blank
   `SpecializationProfile` is present.
3. The typed rewrite fixpoint calls
   `split_typed_post_inline_hot_continuations` and
   `split_typed_post_inline_cleanup_hot_continuations` even when its inline
   target set is empty. Baseline events show two inline passes, nonzero idle
   splitting, and zero actual inline-rewrite time for affected functions.
4. `crates/soac_opt/src/typed/mod.rs` redirects execution to a cloned hot
   continuation in `clone_typed_hot_continuation` and uses
   `TypedContinuationCloneInstrIdRemapper` to allocate fresh instruction IDs.
5. Profile counter lookup still uses the original `(function_id, InstrId)`
   pairs, so the cloned hot path executes without its intended evidence.

Preserving the original source instruction identities during the profile pass
should restore evidence for operators, calls, indexed fields, list item
operations, and branches. The later apply pass can then make better
general-purpose specialization decisions. Avoiding rewrite work in the
profiling pass may also reduce profile setup time, but that secondary effect
must be measured separately from apply throughput.

## Implementation and compatibility

- Implementation: invoke typed v3 optimization rewrites only when
  `env_config.specialization_mode()` is not
  `Some(SpecializationMode::Profile)`. Verify and apply retain their existing
  replay-driven rewrites; profile preserves the original call graph, control
  flow, and semantic instruction IDs. The gate runs after ordinary counter
  instrumentation and initial generator-instance annotation.
- This is an explicit mode check, not an inference from whether a profile file
  or counter-dump path happens to exist. Missing apply evidence must not
  accidentally disable apply-mode planning.
- The normal typed preparation pipeline still lowers the module, defines
  counters, maintains generator/closure state, computes ownership/value facts,
  and constructs a JIT plan. Existing production paths already invoke typed
  preparation with a no-op rewrite callback or `profile=None`, so typed
  rewrites are not a prerequisite for generic execution.
- Preserve CPython-visible results, evaluation order, exceptions, reference
  ownership, generator/coroutine behavior, closure bindings, and generic
  dispatch. Removing profile-only speculative rewrites must not silently alter
  profile execution semantics.
- No new mutable type assumptions, guard lifetime, benchmark-specific
  admissions, environment variables, or production-visible behavior changes
  are introduced. Apply/verify retain their existing guards and fallback
  behavior.
- Newly recovered evidence can expose preexisting optimizer assumptions that
  were hidden by missing counter coverage. The first release smoke selects an
  exact-int branch region whose borrowed local `h` is unavailable at the
  eventual source site because typed liveness ignores local reads represented
  inside selected exact-int sidecars. Extend
  `crates/soac_opt/src/passes/ownership_effects.rs` so
  `collect_typed_local_reads` includes `RegionInputSource::FunctionParam`
  named locals and `IndexedField` `LocalName` receivers from both hot and
  fallback regions of exact-int branch and return plans. Normal existing
  liveness then transports `h` and preserves the selected optimization;
  pruning valid sidecars or changing generic Python semantics is unnecessary.
  A second structured RED regression confirms that `h` is a real formal
  parameter with declared storage, but its nonentry branch block omits `h`
  from live-ins because the sidecar's named-local read is invisible.
- Focused regression coverage: the new four-iteration subprocess integration
  reproducer initializes an alias to `None`, rejoins two alias-assignment
  branches, and exercises a Python call, exact-int operators, exact-list
  get/set operations, generic instance fields, and both branch outcomes in
  its parent loop. It failed against the baseline because its parent-loop
  call and generic field counters are zero despite correct execution. An
  independent structured Rust regression initially failed because baseline
  profile planning changed the original joined-loop CFG from 24 to 37 basic
  blocks. After the profile-only gate, that regression passes: profile retains
  all 24 original blocks and all six counter instruction IDs, while apply can
  still perform its existing hot split. The post-fix integration regression
  also passes, restoring all six parent-loop counter families and preserving
  the profile-to-apply result. Focused original-code-object coverage for named
  generators, generator expressions, coroutines, and async generators also
  passes, as do five additional generic-counter, named-generator,
  generator-expression, and nested-generator regressions.

## Benchmark protocol and coverage

- Fixed exploratory selection: `chaos,richards,deltablue`. This subset is a
  fast diagnostic, not a replacement for full-suite pyperformance acceptance.
- Previous baseline command: `just pyperformance-compare
  chaos,richards,deltablue 1`, using ordinary pyperf sampling and no attached
  native profiler.
- Previous SOAC baseline revision: `f49d7d64`.
- Previous SOAC baseline artifact:
  `work/pyperformance/comparison-20260818-102635-eV9bjf/summary.json`.
- More credible same-day stock reference:
  `work/pyperformance/comparison-20260818-120758-cw6Nzc/summary.json`.
  That stock run was generated while evaluating a separate rejected
  optimization; stock execution does not use the rejected SOAC implementation.
- First release smoke directory:
  `work/pyperformance/comparison-20260818-130600-XFWCkg`; the apply pass
  failed, so this is not a completed comparison or a headline result.
- Successful post-liveness release smoke artifact:
  `work/pyperformance/comparison-20260818-131945-hmVeOQ/summary.json`.
  This used `--debug-single-value`, so it verifies completion and coverage,
  not representative steady-state performance.
- Successful three-workload cold/single-value smoke artifact:
  `work/pyperformance/comparison-20260818-132418-tbIyps/summary.json`.
  Its timings are heavily contaminated by cold setup; use it only to verify
  completion and transformed coverage.
- Completed normal-sampling candidate comparison artifact:
  `work/pyperformance/comparison-20260818-132520-fxTXzI/summary.json`;
  full output is in `work/logs/hot-loop-candidate-representative.log`.
  The run used one stock/profile/apply round, fresh candidate profile
  evidence, and 20 measured stock and apply values per benchmark. It completed
  in 206.11 seconds and suffered severe VM scheduling/outlier interference.
- Restoring the previous rejected experiment required a 44.98-second debug
  extension rebuild before the focused RED check. This is a one-time build
  cost, not benchmark runtime, profile setup time, or an optimization result.
- Initial full-gate log: `work/logs/hot-loop-test-all.log`. All Rust crates
  passed, including 548 JIT, 367 lowering, and 202 optimizer tests, but two
  N-Queens profile-mode Python integration cases failed on missing codegen
  Unicode constants. Cargo tests took 88.267 seconds, pytest took 45.839
  seconds, and the failing test phase took 134.124 seconds.
- Corrected full-gate log: `work/logs/hot-loop-test-all-fixed.log`.
  `just test-all` passes all 1,209 Python test node IDs across 72 batches and
  the complete Rust workspace, including 549 `soac_jit`, 367 `soac_lowering`,
  202 `soac_opt`, and eight PyO3-extension tests. The successful test phase
  took 198.632 seconds; the counter-dump batch took 118.51 seconds.
- Baseline and normal-sampling candidate completed benchmarks: `chaos`,
  `deltablue`, and `richards`; no failures in the final fixed subset.
  Full-suite completion and the 1.10x stock-CPython acceptance target have
  not been evaluated for this strategy.
- Each baseline benchmark transforms its own `__main__` benchmark module and
  `soac.runtime`. No standard-library or third-party dependency module is
  transformed in this subset.
- Baseline apply compiles 35 `chaos` functions, 79 `deltablue` functions, and
  53 `richards` functions; 167 reported function instances in total.

## Measurements

| Benchmark | Previous SOAC mean | Prior credible stock reference | Candidate stock mean | Candidate SOAC mean | Previous / candidate SOAC mean |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 99.2156843 ms | 30.0613919 ms | 30.2397584 ms | 102.6065045 ms | 0.9669532x |
| `deltablue` | 4.6591915 ms | 1.5202782 ms | 1.5436580 ms | 5.0575092 ms | 0.9212423x |
| `richards` | 45.3569023 ms | 22.0921291 ms | 44.2515538 ms | 83.3514952 ms | 0.5441642x |

These are the arithmetic means written to the normal-sampling result summary,
not robust estimates. Its stock-relative geometric mean is **0.3628087x**,
and its previous-SOAC geometric mean is **0.7855426x**. Severe VM outliers,
especially for `richards`, make the apparent previous-SOAC geometric
regression unreliable.

The 20 raw measured values per benchmark give the following more robust
medians:

| Benchmark | Previous SOAC median | Candidate SOAC median | Median elapsed change | Candidate stock median | Paired stock / SOAC median |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 99.4998870 ms | 100.6145265 ms | +1.120% | 30.0425653 ms | 0.2985907x |
| `deltablue` | 4.6088112 ms | 4.6892690 ms | +1.746% | 1.5271759 ms | 0.3256746x |
| `richards` | 42.3901580 ms | 43.2001030 ms | +1.911% | 23.9640667 ms | 0.5547224x |

The median-based previous-SOAC geometric mean is **0.9843328x** and the
median-based paired stock geometric mean is **0.3778435x**. These exploratory
one-round medians suggest a small 1–2% slowdown, not a demonstrated
optimization. They are more informative than the distorted means but do not
establish a statistically reliable effect or meet the full-suite target.

Candidate `richards` stock values reached **147.0885750 ms**, raising their
mean to 44.2515538 ms despite a 23.9640667 ms median. Two candidate apply
values reached **437.9120660 ms** and **444.0347900 ms**, raising their mean
to 83.3514952 ms despite a 43.2001030 ms median and yielding a
**122.3158039 ms** sample standard deviation. The baseline `richards` apply
median was 42.3901580 ms. Treat mean-based previous-SOAC ratios and the
reported 0.7855426x geometric mean as noise dominated; do not describe them as
a reliable 46% `richards` regression or a 27% overall regression.

The original baseline measured stock `chaos` at 58.1008614 ms, but its 20
values included severe outliers of 43.765, 53.822, 70.173, 71.580, 95.309,
100.145, 129.642, 132.309, and 133.069 ms among otherwise ordinary 29–31 ms
values. The later stock run stayed between 29.249 and 30.680 ms, making its
30.0613919 ms result the more credible historical reference. These stock and
previous-SOAC numbers come from different runs, so they are not a paired
speedup. Report a newly paired stock-relative result and a separate direct
previous-SOAC comparison for the candidate; do not treat stock drift as an
optimization gain.

| Generated-code metric | Previous SOAC baseline | Candidate | Change |
| --- | --- | --- | --- |
| Optimized typed-IR final basic blocks | 1,862 | 1,897 | +35 / +1.880% |
| Optimized typed-IR function instances | 167 | 167 | unchanged |
| Pre-optimization serialized BlockPy bytes | 6,311,140 | 6,311,140 | unchanged |
| Apply-mode native emitted bytes | 13,977,840 | 14,288,640 | +310,800 / +2.224% |
| Apply-mode native machine blocks | 929,520 | 949,320 | +19,800 / +2.130% |

Generated-code totals aggregate repeated worker compilation and are not a
deduplicated resident-code footprint. A candidate that improves counter
coverage may legitimately produce different apply-mode plans and code size;
the retained repair does increase generated code and must not be misreported
as a code-size or throughput improvement.

The normal-sampling run spent 206.11 seconds overall. Its profile pass used
30 worker processes with 70.5 seconds total pre-measurement setup versus
3.20 seconds collecting measured values; apply used 30 worker processes with
90.5 seconds total setup versus 5.34 seconds collecting measured values.
Median per-worker setup was 2.41 seconds in profile and 2.87 seconds in apply.
These workflow/setup costs are distinct from headline pyperformance values;
outlier-resistant per-benchmark median/min/max summaries and worker reuse
would make future optimization decisions less expensive and less vulnerable
to VM interference.

The first candidate release smoke is incomplete and must not be reported as a
headline comparison: stock `chaos` measured approximately 30.9 ms, the SOAC
profile pass completed at approximately 96.5 ms with 1.39 seconds of setup,
and apply failed before producing an elapsed result. Its failure log is
`work/logs/hot-loop-candidate-chaos-smoke.log`:

```text
optimizer v3 exact-int branch hot region input PlanValue { id: PlanValueId(1), rep: PyObjectBorrowed } references unavailable local "h" [function=Chaosgame.create_image_chaos id=1:31] [background_function=GVector.__init__ id=1:1]
```

Recovered profile evidence now admits an existing exact-int specialization
whose selected borrowed input is invisible to the existing typed local-read
collector and therefore unavailable in the final typed function. The initial
candidate failed explicitly rather than producing an incorrect Python result.
The subsequent liveness repair preserves that real semantic read and the
selected optimized region; its follow-up release smoke completes successfully.

The successful follow-up release smoke used `--debug-single-value` and is
likewise not a headline timing result: stock `chaos` measured 30.6686650 ms,
SOAC profile approximately 119 ms, and SOAC apply 238.1151990 ms
(`0.1287976x` stock/SOAC). It transformed `__main__` and `soac.runtime`,
compiled 35 functions, retained 846 optimized typed-IR blocks, serialized
991,150 pre-optimization BlockPy bytes, and emitted 727,336 native bytes in
48,293 machine blocks. These measurements prove release-mode profile/apply
completion and generated-code coverage, not a representative performance
regression or improvement. The later normal-sampling comparison and its
outlier-resistant interpretation are recorded above.

A subsequent cold `--debug-single-value` smoke completes all three exploratory
workloads but is also unusable as headline performance evidence: `chaos`
reports 30.1519060 ms stock versus 1.5622597130 seconds SOAC, `deltablue`
reports 1.4772310 ms stock versus 876.9950760 ms SOAC, and `richards`
reports 22.8810580 ms stock versus 446.1387270 ms SOAC. These inflated apply
values include cold/setup effects and must not be compared with the previous
normally sampled SOAC baseline or presented as a strategy regression. The
run does establish that all three apply passes transform `__main__` and
`soac.runtime`, complete with 35, 79, and 53 compiled functions respectively,
and report 1,897 optimized typed-IR blocks, 3,155,570 serialized BlockPy
bytes, 1,428,864 native emitted bytes, and 94,932 machine blocks. Worker
counts differ from the normal-sampling baseline, so these aggregate smoke
code-size totals must not be compared directly with the baseline totals.

The failed smoke still produced a complete `chaos` profile, independently
confirming that the profile-only gate restores the missing hot-loop evidence:

| Profiled function | Counter family | Nonzero rows / rows | Recorded observations |
| --- | --- | --- | --- |
| `Chaosgame.create_image_chaos` | Operators | 10 / 10 | 40,002 |
| `Chaosgame.create_image_chaos` | Calls | 11 / 12 | 20,008 |
| `Chaosgame.create_image_chaos` | Generic attribute reads | 12 / 12 | 35,005 |
| `Chaosgame.create_image_chaos` | Branch outcomes | 6 / 6 | 15,003 |
| `Chaosgame.create_image_chaos` | Exact-list loads | 1 / 1 | 5,000 |
| `Chaosgame.create_image_chaos` | Exact-list stores | 1 / 1 | 5,000 |
| `Spline.__call__` | Operators | 25 / 25 | 1,169,567 |
| `Spline.__call__` | Calls | 13 / 14 | 454,830 |
| `Spline.__call__` | Generic attribute reads | 11 / 11 | 337,880 |
| `Spline.__call__` | Branch outcomes | 10 / 10 | 272,912 |
| `Spline.__call__` | Item loads | 11 / 12 | 376,877 |
| `Spline.__call__` | Item stores | 1 / 1 | 77,970 |

In `create_image_chaos`, previously zero loop instruction IDs `#62`, `#63`,
`#74`, and `#83` now each record 5,000 exact-int operator observations with
packed shape `257`; the exact-list load at `#58` and store at `#67` each
record 5,000 shape-`1` observations. Seven formerly unobserved loop
attribute sites each record 5,000 generic reads, and call site `#96` records
5,000 observations of a transformed target. One call site remains genuinely
cold; nonzero coverage is not expected for branches that do not execute.

An independent, successful native profile can be reproduced with the
project-native recipe:

```bash
SOAC_JIT_BB_MAP=0 PERF_FREQUENCY=199 just pyperformance-deep-profile-from-profile work/pyperformance/comparison-20260818-132520-fxTXzI/round-01-soac.json chaos loops=10 output_prefix=work/logs/hot-loop-fixed-symbols
```

The recipe automatically skips `--calibrate-loops` workers, selects a measured
worker, reuses the release environment, and captures **218 `cpu-clock`
samples**, **13.662 MB**, and **zero lost samples** without the VM mount's
prohibitively expensive basic-block map. The Speedscope output is
`work/logs/hot-loop-fixed-symbols_speedscope.json`.
`_PyObject_GenericGetAttrWithDict` accounts for **11.47% inclusive** and
**5.50% self** CPU; the SOAC vectorcall hook accounts for **9.17% inclusive**
and **4.13% self**; closure creation accounts for **17.89% inclusive**.
Inclusive shares overlap and must not be added. Native profiling is separate
diagnostic evidence, not the throughput benchmark.

## Attempt history

### Attempt 1: Locate missing source-keyed evidence in executed hot loops

- Change: inspected existing baseline profiles, typed planning/rewrite code,
  and hot-continuation cloning without modifying production behavior.
- Measurements and coverage: documented the cross-benchmark zero-site census,
  nested-callee observations, baseline SOAC elapsed times, stock outlier drift,
  transformed modules, compiled functions, and generated-code totals above.
- Compatibility and tests: `just pytest-fast
  tests/test_profile_hot_loop_counters.py -q` produced a genuine pre-fix RED
  result at `assert calls`: the transformed four-iteration loop completed
  correctly, but parent-loop call-target and generic field-access counters
  remained zero. The independent structured Rust regression
  `profile_mode_preserves_countered_hot_loop_ids_while_apply_can_split` is
  also RED: the profile-mode joined-loop CFG grows from 24 to 37 blocks and
  fails its original-block-preservation assertion. Existing no-rewrite typed
  preparation and generic generator planning provide additional structural
  support. The structured and focused runtime regressions were subsequently
  made green in Attempt 2.
- Result: source-keyed profile counters are assigned before profile-mode
  continuation cloning renumbers the hot path.
- Reason: profile mode suppresses static call targets but does not yet
  suppress the general typed rewrite fixpoint and its idle continuation split.

### Attempt 2: Preserve original profile-mode instruction identities

- Change: added an explicit profile-only gate around typed optimization
  rewrites after normal instrumentation and initial generator annotation;
  apply/verify optimization behavior remains enabled.
- Measurements and coverage: the initial release smoke restores every
  formerly missing hot `create_image_chaos` operator, field, list, and branch
  family and records 1,169,567 `Spline.__call__` operator observations, but
  fails during apply on an unavailable borrowed local `h`. Final paired stock
  and SOAC results, robust previous-SOAC medians, transformed apply coverage,
  code size, and measured setup costs were collected after Attempt 3.
- Compatibility and tests: both focused regressions were confirmed RED before
  the production gate. The structured Rust regression
  `profile_mode_preserves_countered_hot_loop_ids_while_apply_can_split` now
  passes: one test passed with 547 filtered; profile preserves the original
  24-block CFG and all six counted instruction IDs, while apply still expands
  beyond 24 blocks using its existing hot-continuation split. The focused
  integration command `just pytest-fast tests/test_regression_original_code_object.py
  tests/test_profile_hot_loop_counters.py -q` passes all four tests: profile
  records calls, exact-int operators, exact-list gets, exact-list sets,
  generic field access, and both branch outcomes inside the parent loop;
  apply returns the correct value; named generators, generator expressions,
  coroutines, and async generators preserve their original code objects.
  A focused selection from `tests/test_counter_dump_file.py` also passes five
  additional generic-counter, named-generator, generator-expression, and
  nested-generator checks with 23 tests deselected. Package-scoped Rust
  formatting and compilation checks pass. These focused tests do not exercise
  the newly admitted benchmark-specific exact-int input failure.
- Result: focused correctness improved, but the release smoke failed during
  apply. No steady-state performance improvement is claimed.

### Attempt 3: Include selected scalar-region reads in typed local liveness

- Change: extend `collect_typed_local_reads` in
  `crates/soac_opt/src/passes/ownership_effects.rs` to visit both hot and
  fallback regions of selected exact-int branch and return sidecars. Record
  each `RegionInputSource::FunctionParam` named-local read and each
  `IndexedField` `LocalName` receiver as an ordinary local use, so existing
  typed liveness transports the required values and preserves valid selected
  optimizations.
- Measurements and coverage: recovered `chaos` parent-loop and
  `Spline.__call__` counter observations, the completed normal-sampling
  three-benchmark comparison, paired stock and previous-SOAC values, severe
  VM outliers, 1–2% robust median regressions, generated-code growth, native
  profile, and 206.11-second workflow cost are recorded above.
- Compatibility and tests: add a focused structured regression for an
  otherwise eligible exact-int region whose named local or indexed-field
  receiver exists only in its hot/fallback sidecar. Prove the local remains
  live and its selected optimized region survives while preserving ownership,
  evaluation order, fallback behavior, and profile-to-apply execution.
- Pre-fix structured result: the new `soac_opt` regression
  `typed_exact_int_region_inputs_keep_hidden_locals_live_at_their_block` is
  independently RED with `exact-int branch hot named local input must keep
  declared parameter h live in its actual block`. The test establishes that
  `h` exists in the function's formal parameters and local storage but is
  missing from the selected nonentry block's live-ins. Four cases exercise
  exact-int branch hot inputs, branch fallback inputs, return hot inputs, and
  indexed-field receiver fallback inputs.
- Post-fix structured validation: `cargo test -p soac_opt
  typed_exact_int_region_inputs_keep_hidden_locals_live_at_their_block --lib`
  passes one test with 201 filtered, covering all four scenarios in the test
  body. The complete `soac_opt` Rust test suite subsequently passes all 202
  tests, all three indexed-field exact-int sidecar regressions pass, the
  profile-mode original-instruction-ID regression passes, and
  `just fmt-rust-check soac_opt soac_jit` passes. The follow-up
  `--debug-single-value` release `chaos` comparison now completes both
  profile and apply with 35 compiled functions, 846 typed blocks, and
  727,336 native emitted bytes; see the non-headline smoke measurements
  above. A second cold/single-value smoke also completes `chaos`,
  `deltablue`, and `richards`; its setup-contaminated elapsed values are not
  representative performance measurements.
- Result: typed liveness recognizes selected scalar-region named-local reads,
  the structured regression is green, and all three release exploratory
  workloads now complete without pruning valid exact-int optimizations.
  Normal-sampling results show no throughput win: robust medians are slightly
  negative, arithmetic means are noise dominated, and native code grows.
  Retain this as a correctness/profile-evidence prerequisite only.

### Attempt 4: Preserve codegen constants for unoptimized generator cell refs

- Change: the first full `just test-all` gate ran after the focused Rust,
  generator, profile-counter, and three-workload checks had passed.
- Full-gate Rust result: all Rust crates passed, including 548 `soac_jit`,
  367 `soac_lowering`, and 202 `soac_opt` tests. The selected pytest batch
  then reported 26 passing and two failing cases:
  `test_specialized_full_nqueens_slice_preserves_results_and_guarded_fallbacks`
  and `test_specialized_pyperformance_nqueens_discard_executes_hot_and_fallback_paths`.
- Both failures occur while importing the source-backed benchmark during
  `SOAC_OPT_MODE=profile`, before apply-mode opaque-fusion admission.
  Background codegen for `permutations.<locals>.<genexpr>` panics because its
  module constant pool lacks Unicode names for `_dp_cell_pool` and
  `typed cell_ref`.
- Root cause: the profile rewrite gate now correctly preserves generator
  `CellRef` and preserved-cell operations that previously disappeared during
  profile-time optimization, but `ModuleConstantCollector` skips `CellRef`
  payloads and does not account for all preserved-slot/error-path names.
  `emit_checked_local_value_or_unbound` still requires those names to exist in
  the module's prebuilt codegen constant pool.
- Pre-fix structured result:
  `module_constants::tests::closure_and_preserved_storage_names_are_available_to_unbound_codegen`
  is genuinely RED because BlockPy constant collection omits the real closure
  name `captured_value`; 548 unrelated JIT tests were filtered. The fixture
  covers typed and untyped collection of closure/freevar, owned-cell, and
  preserved-slot names.
- Post-fix structured result:
  `closure_and_preserved_storage_names_are_available_to_unbound_codegen` now
  passes one test with 548 filtered. Both typed and untyped constant
  collection retain logical and storage names for owned cells, closure
  freevars, and preserved slots. Preserved-cell unbound diagnostics now use
  the actual Python variable name instead of the internal `typed cell_ref`
  label.
- Post-fix integration result: both original N-Queens regressions now pass
  in 68.07 seconds, including source-backed named-generator and nested
  generator-expression profiling plus profile-to-verify/apply execution and
  guarded fallback behavior.
- Disambiguation: removing exact-source N-Queens recognition or its special
  indexed-module-dictionary policy cannot independently prevent this failure:
  ordinary source-named-generator globals are only selected in apply mode,
  while both panics occur earlier in profile mode. The fix must preserve the
  generic generator/cell codegen invariant for arbitrary source programs.
- Corrected full-gate result: `just test-all` passes with 1,209 Python node
  IDs across 72 batches; Rust includes 549 JIT, 367 lowering, 202 optimizer,
  and eight PyO3-extension tests, plus the other workspace crates. The full
  test phase took 198.632 seconds, including a 118.51-second counter-dump
  batch. Output: `work/logs/hot-loop-test-all-fixed.log`.
- Workflow lesson: include source-backed named-generator plus nested
  generator-expression profiling in the focused profile-mode regression
  selection before spending roughly 134 seconds on another full test phase.

## Verdict and next action

- Verdict: **RETAIN as a correctness and profile-evidence prerequisite, not
  as a throughput optimization.** The fixed subset now collects valid hot-loop
  evidence and runs correctly; robust candidate medians are approximately
  1–2% slower, generated code grows, and VM outliers make the much larger
  mean-based regression inconclusive. The initially exposed preserved-cell
  profile failures are fixed, and the corrected full `just test-all` gate
  passes all 1,209 Python node IDs and the full Rust workspace. The full
  pyperformance suite and 1.10x stock-CPython target remain pending.
- Transferable lesson: profile counters and later replay plans are keyed to
  original semantic source instructions. Any profile-time rewrite that clones
  or renumbers executed instructions can erase optimization evidence even
  when the benchmark completes and nested callees appear well profiled.
- Next action: use the restored profile evidence and independent native
  hotspot capture to investigate generic attribute dispatch, vectorcall
  overhead, and closure creation as separately logged optimization
  strategies. Require robust same-run stock and previous-SOAC comparisons
  before claiming a performance gain.
