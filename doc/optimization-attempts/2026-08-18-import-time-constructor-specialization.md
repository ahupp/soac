---
title: "Import-time constructor specialization"
---

# Import-time constructor specialization

- Status: landed
- Pacific date: 2026-08-18 PDT
- Previous baseline revision: `f49d7d64` on `main`; subsequent hot-loop
  profile-coverage and N-Queens benchmark-integrity repairs are the current
  comparison prerequisites.
- Outcome: retained. The focused integration proves immediate safe constructor
  registration, a real profile target, and an eager apply-mode direct edge;
  actual `chaos` apply artifacts also prove new hot `GVector` constructor
  edges. A robust three-workload median comparison improves by **1.01763x**
  overall, with a small `richards` slowdown and **9.73%** native-code growth.
  The candidate remains only **0.35673x** as fast as paired stock CPython;
  the full correctness gate passes, but full-suite acceptance remains pending.

## Hypothesis and evidence

Ordinary Python classes are commonly defined and then instantiated while their
own module is still initializing. SOAC already has source-independent,
guarded specialization for safe heap-type constructors, but its final
module-wide owner-type registration happens only after module initialization
returns. A class instantiated before that sweep therefore has no synthetic
constructor target identity when the profile call executes; profile evidence
cannot select the existing safe constructor specialization during apply.

Registering eligible classes immediately after their creation should expose
their existing synthetic constructor entry to all subsequent module-level
calls without recognizing benchmark names, class names, source bytes, or
expected output. This is a general registration-order fix, not a benchmark
substitution. A possible benefit is fewer generic Python class-call and
initializer dispatches in allocation-heavy real workloads.

An earlier decoded single-worker `chaos` profile, recorded in
`doc/optimization-attempts/2026-08-18-hot-loop-profile-counter-coverage.md`,
observed **137,970** executions of the `GVector.linear_combination` path.
The installed benchmark source constructs exactly one `GVector` in each
`linear_combination` invocation. The same recovered profile records
**454,830 call observations** and **337,880 generic attribute observations**
across `Spline.__call__`; these are aggregates across multiple source sites,
not constructor counts. Earlier operator profiling separately observed
**701,730 floating-point operations** in `GVector.linear_combination`;
arithmetic observations are likewise not allocation counts. The rejected
exact-float strategy showed that a large operation count by itself does not
guarantee a useful speedup.

An earlier symbol-only native profile captured **1,123 `cpu-clock` samples**
with zero lost samples. Its overlapping inclusive call paths attributed
**87.62%** to `py_vectorcall_hook`, **77.20%** to
`Chaosgame.transform_point`, **62.60%** to generic
`_PyObject_MakeTpCall`, and **61.62%** to `Spline.__call__`.
`soac_jit_make_function_with_closure` accounted for **15.05%** on a nested
path. A subsequent post-coverage-repair capture recorded **218 samples** with
zero lost samples and attributed **11.47% inclusive / 5.50% self** to
`_PyObject_GenericGetAttrWithDict`, **9.17% inclusive / 4.13% self** to the
SOAC vectorcall hook, and **17.89% inclusive** to closure creation. Inclusive
call-tree percentages overlap and must not be added. Generic type dispatch,
attribute access, and closure work are all credible competing costs; the
candidate may be positive, neutral, or negative.

The concrete baseline ordering is:

1. Before this change, `soac.runtime.create_class` prepares and executes the
   source class namespace, invokes its metaclass, validates the class cell,
   and calls `_soac_ext.profile_watch_type_key_layout(cls)` before returning
   the realized class.
2. Transformed module initialization can immediately invoke that class and
   run additional module-level Python code.
3. Only after `call_module_init` returns does
   `crates/soac_pyo3/src/jit_runtime.rs` invoke
   `register_function_owner_types_for_module_keys_with_constructor_entries`.
4. `crates/soac_jit/src/lib.rs` already maps an eligible class's transformed
   `__init__` to a distinct synthetic constructor-entry function ID and
   rejects unsupported allocation/metaclass shapes.
5. Until that existing registration runs, an earlier class call cannot
   contribute its synthetic target identity to `call_hot_targets`.

The hypothesis is falsifiable at four independent boundaries: a safe class
has a nonzero synthetic target before the module's first call; profile records
that target; apply emits a real direct constructor edge; and an ordinary
profile-to-apply pyperformance comparison improves previous SOAC throughput
without material semantic or generated-code regressions.

## Implementation and compatibility

- Implemented change: extend the existing class-creation callback so
  `soac.runtime.create_class` passes both the realized class and its trusted
  transformed namespace function to `_soac_ext.profile_watch_type_key_layout`.
  The new **public** `soac_jit::register_created_owner_type_from_namespace`
  API reads SOAC-owned namespace-function metadata, recovers the owning module
  context, and invokes the existing `register_owner_types_from_type` path
  before returning the class. Non-type inputs and namespace functions without
  trusted SOAC metadata remain no-ops.
- Reuse the existing synthetic lowered constructor entry, owner-type watcher,
  type metadata, argument planner, exact-callee guard, original Python call
  fallback, and final post-import registration sweep. Do not add a separate
  global registry, thread-local ownership, temporary Python metadata
  attributes, benchmark-specific recognition, or source fingerprints.
- Safe eligibility remains restricted to a same-module nonabstract heap type
  with exact built-in `type` metaclass, `PyType_GenericAlloc`, ordinary
  `object.__new__`, and an existing transformed initializer. Preserve current
  explicit-argument/default-refresh restrictions.
- Unsupported custom `__new__`, custom metaclass `__call__`, non-generic
  allocation, abstract classes, unsupported arguments, or missing trusted
  metadata retain zero constructor identity and the original generic Python
  call path. Preserve evaluation order, once-only argument evaluation,
  initializer and metaclass side effects, result identity, exceptions,
  reference ownership, tracing/monitoring behavior, and class-cell errors.
- Early registration itself must not introduce new user-visible class
  attribute lookups. A custom metaclass can intercept
  `owner.__module__` through `__getattribute__`; invoking that lookup before
  the module assigns the class name observably changes Python execution even
  when the unsupported constructor ultimately falls back. Check exact
  built-in metaclass and direct-allocation eligibility through raw CPython
  slots before any Python-visible lookup, and use raw exact-Unicode type-dict
  and globals checks for same-module proof without invoking descriptors or
  custom equality. Preserve the existing post-assignment late-sweep lookup.
- Existing owner registration recursively visits nested class objects in an
  eligible outer class's dictionary. A raw-safe outer class can still contain
  an unsafe custom-metaclass inner class; merely checking the root before
  recursion does not prevent an observable nested `__module__` callback.
  Preflight the entire recursively reachable type/dictionary graph with
  cycle-safe raw checks, and defer the whole outer registration to the
  unchanged late sweep whenever any nested owner is unsafe.
- Exact class identity and existing owner/type invalidation define the guard
  lifetime. Rebinding, mutation of `__init__`, custom metaclass behavior, or
  changed class allocation semantics must not leave a stale direct-call
  assumption active. Keep ordinary owner watchers and late registration.
- Eager compilation currently starts before module initialization. The lowered
  synthetic constructor ID exists then, but the runtime class object does not.
  Apply can only specialize if v3 target selection can resolve the persistent
  lowered ID before class creation and generated guards read the realized
  heap type's metadata later. If eager planning cannot do that, record the
  result honestly as a profile-evidence-only improvement rather than claiming
  a direct edge or speedup.
- Authored focused integration:
  `tests/test_import_time_constructor_registration.py::test_import_time_constructor_registration_reaches_profile_and_apply`.
  It checks a distinct nonzero constructor ID immediately after class
  creation, at least four matching profile call-target observations, an actual
  apply-mode `soac_jit_direct_edges` event, and zero-ID generic fallbacks
  with ordered side effects for custom `__new__` and metaclass `__call__`.
  Its genuine baseline RED execution fails in **0.45 seconds** at
  `assert profile["box_id"] != 0`: the safe `Box` constructor still has ID
  zero during class/module initialization. The custom `__new__` and custom
  metaclass IDs correctly remain zero, and their ordered side effects and
  results pass. After the three-file production change, the same focused
  integration passes: **one test passed in 2.21 seconds**, proving a nonzero
  early synthetic constructor ID distinct from `__init__`, at least four
  matching profile targets, an actual eager-mode apply direct edge in `run`,
  and unchanged safe fallback behavior. Warning-free
  `cargo check -p soac_jit -p soac_pyo3 --tests` also passes in
  **5.71 seconds**. Additional focused constructor, exception/custom-`__new__`,
  direct-fallback, and hot-loop profile coverage passes: **seven tests passed,
  two deselected**. The two explicitly named runtime-bootstrap tests carry a
  slow marker and were silently deselected by the default runner. Their
  explicit `--run-slow` rerun passes the runtime profile-bootstrap case but
  exposes one preexisting vendored-CPython/test mismatch: the other test
  expects `_testinternalcapi.has_indexed_values`, which this CPython does not
  expose (`has_inline_values` exists). That failure occurs after successful
  transformed-runtime import and class bootstrap; it is not evidence that the
  constructor change breaks bootstrap. The default full gate excludes these
  slow cases and passes. The targeted runtime
  profile-bootstrap test passes when explicitly selected with `--run-slow`:
  **one test passed**.
- Review then identified an additional compatibility edge in the initial
  early-registration implementation: unsupported custom-metaclass
  `__getattribute__("__module__")` could run before class-global assignment.
  The focused integration is being strengthened to record interception
  timing, require the ordinary existing late lookup, and reject any lookup
  before `CustomMeta` is assigned. The strengthened regression is genuinely
  RED against the first candidate: its observed assignment states are
  `[False, True]`, proving an extra pre-assignment interception followed by
  the ordinary expected late lookup. After raw eligibility and exact-Unicode
  checks are moved before any Python-visible operation, the strengthened
  regression passes: **one test passed in 2.09 seconds** and reports only
  `[True]`, retaining the ordinary late post-assignment lookup. Safe profile
  constructor evidence and the actual eager apply direct edge still pass.
  A further strengthened nested-class regression is independently RED: a safe
  `Outer` containing a custom-metaclass `Inner` still produces nested lookup
  ordering `[False, True]` before `Outer` is assigned. Cycle-safe recursive
  raw `tp_dict` preflight fixes the third RED by deferring the whole unsafe
  outer graph to the unchanged late sweep; only the expected post-assignment
  nested lookup remains. Early safe-class IDs, profile constructor targets,
  actual apply direct edges, and both top-level/nested custom-metaclass
  observation invariants all pass. Focused guardrails report **seven tests
  passed in 12.21 seconds**. Package-scoped Rust formatting and formatting
  checks for `soac_jit` and `soac_pyo3` pass. The full `just test-all` gate
  also passes all **1,209 Python node IDs across 73 batches**, including the
  new constructor integration, and all Rust crates.

## Benchmark protocol and coverage

- Fast sanity benchmark: `chaos`; exploratory mixed subset:
  `chaos,richards,deltablue`. This subset does not replace the complete
  pyperformance suite or its **1.10x** stock-CPython acceptance target.
- Initial completion-only smoke:
  `just pyperformance-compare chaos 1 '' --debug-single-value`.
  Single-value timing is not a statistically valid headline.
- Normal-sampling mixed comparison:
  `just pyperformance-compare chaos,richards,deltablue 1 work/pyperformance/comparison-20260818-132520-fxTXzI`.
  Use fresh profile evidence for the candidate; increase rounds when VM
  stability and runtime permit.
- Existing ordinary-sampling baseline artifact:
  `work/pyperformance/comparison-20260818-132520-fxTXzI/summary.json`.
  It includes the retained hot-loop coverage/liveness repair but predates
  removal of the N-Queens exact-source shortcut. The selected three workloads
  do not include that invalid N-Queens path; candidate benchmarking must use
  the current integrity-fixed source-independent production implementation.
- Completed ordinary-sampling candidate:
  `work/pyperformance/comparison-20260818-144613-sAgzx3/summary.json`;
  full output is in `work/logs/import-time-constructor-representative.log`.
  The profile-to-apply comparison takes **192.73 seconds** and collects
  **20 measured stock and apply values per benchmark**.
- Completed full correctness gate:
  `work/logs/import-time-constructor-test-all.log`. `just test-all` passes
  **1,209 Python node IDs across 73 batches** and all Rust crate suites,
  including **543 `soac_jit`**, **367 `soac_lowering`**, **202 `soac_opt`**,
  and **eight PyO3-extension** tests. The test phase takes **171.603 seconds**;
  its slowest counter-dump batch takes **99.20 seconds**.
- The concise retained-performance summary is recorded separately in
  `doc/PERF_LOG.md` under change ID `ntkwxnmn`.
- Existing source-independent N-Queens smoke:
  `work/pyperformance/comparison-20260818-140910-BtgC9F/summary.json`.
  Stock **42.4562010 ms** versus SOAC apply **40.8079650 ms** is one value
  only. Its **1.0403901x** ratio reflects ordinary named-generator CPython
  fallback, not JIT optimization of the hot generators.
- Candidate completion-only `chaos` smoke:
  `work/pyperformance/comparison-20260818-143546-ZhctOi/summary.json`.
  Its cold `--debug-single-value` apply timing includes startup effects and
  is not a representative throughput measurement. Its code summaries do
  independently establish real `GVector` constructor direct edges; ordinary
  throughput is evaluated separately in the completed normal-sampling run.
- Candidate completion-only mixed-subset smoke:
  `work/pyperformance/comparison-20260818-143734-Yxp7Z4/summary.json`.
  All three workloads complete, but substantial cold setup contaminates each
  single value. Treat it as coverage/setup/code-size evidence only.
- Baseline completed benchmarks: `chaos`, `deltablue`, and `richards`.
  Both candidate single-value smokes and ordinary sampling complete the
  same three workloads; full-suite completion remains pending.
- Each current subset benchmark transforms its `__main__` benchmark module
  and `soac.runtime`; none transforms a standard-library or external
  dependency module. Baseline apply compiles 35 `chaos`, 79 `deltablue`, and
  53 `richards` function instances, totaling 167.
- Explicitly distinguish compilation of an existing synthetic constructor
  entry from a recorded profile constructor target and a measured apply-mode
  direct edge at an actual caller. Both baseline and candidate compile
  `GVector.__soac_constructor_entry__`, but only the candidate emits a new
  direct edge in each of `GVector.linear_combination`, `GVector.__mul__`, and
  `GVector.__add__`. The synthetic target is function `1:37`, packed runtime
  ID `4294967333`.
- Keep native profiling separate from the ordinary trained apply-pass
  throughput result. The project-native `perf` recipe requires
  `SOAC_JIT_BB_MAP=0` on this VM because detailed per-basic-block mapping
  previously added more than two minutes of mount overhead.

## Measurements

| Benchmark | Candidate paired stock mean | Previous SOAC mean | Candidate SOAC mean | Previous SOAC median | Candidate SOAC median | Previous / candidate median |
| --- | --- | --- | --- | --- | --- | --- |
| `chaos` | 30.3324081 ms | 102.6065045 ms | 96.9252632 ms | 100.6145265 ms | 95.8330440 ms | 1.0498939x |
| `deltablue` | 1.4555377 ms | 5.0575092 ms | 4.6228215 ms | 4.6892690 ms | 4.5808291 ms | 1.0236726x |
| `richards` | 22.2228124 ms | 83.3514952 ms | 48.2372796 ms | 43.2001030 ms | 44.0575340 ms | 0.9805384x |

The current baseline's stock-relative geometric mean is **0.3628087x** when
computed from its arithmetic means, but its `richards` samples have severe VM
outliers: stock reaches **147.0885750 ms** and two apply values reach
**437.9120660 ms** and **444.0347900 ms**, despite a stock median of
**23.9640667 ms** and SOAC median of **43.2001030 ms**. Its apply sample
standard deviation is **122.3158039 ms**. Do not treat mean-based
cross-revision deltas as reliable in the presence of these outliers. Compare
raw distributions and robust medians, report the candidate's paired stock
results, and distinguish noisy inference from measured facts.

The candidate's robust previous-SOAC geometric mean across the three median
ratios is **1.0176311x**, approximately **1.76%** higher throughput. `chaos`
improves by **4.99%** on the previous/candidate median ratio, `deltablue`
improves by **2.37%**, and `richards` declines by **1.95%**. The normal
pyperf comparison reports the mean-based `chaos` **1.06x** and `deltablue`
**1.09x** differences as significant, while hiding `richards` because its
difference is not significant. These are one-round results, not a full-suite
acceptance claim.

The raw summary's previous-SOAC geometric mean of **1.2601803x** is
misleading: the old `richards` mean includes **437.9120660 ms** and
**444.0347900 ms** VM outliers, while the current `richards` maximum is
**93.4962875 ms**. Do not report the apparent **26.0%** gain or the
mean-based **1.72795x** `richards` improvement as an optimization result.
The candidate's current paired stock-relative geometric mean is
**0.3567255x**; SOAC remains approximately **2.80x slower** than stock for
this subset and far below the required full-suite **1.10x** target.

The older `f49d7d64` SOAC means were `chaos` **99.2156843 ms**,
`deltablue` **4.6591915 ms**, and `richards` **45.3569023 ms**. The paired
older stock `chaos` mean of **58.1008614 ms** was contaminated by large VM
outliers; a separate stable stock run measured **30.0613919 ms**. These are
historical references, not current paired candidate ratios.

| Generated-code metric | Current three-workload baseline | Candidate | Change |
| --- | --- | --- | --- |
| Optimized typed-IR final basic blocks | 1,897 | 2,055 | +158 / +8.33% |
| Optimized typed-IR function instances | 167 | 167 | unchanged |
| Pre-optimization serialized BlockPy bytes | 6,311,140 | 6,311,524 | +384 / +0.006% |
| Apply-mode native emitted bytes | 14,288,640 | 15,679,400 | +1,390,760 / +9.73% |
| Apply-mode native machine blocks | 949,320 | 1,038,180 | +88,860 / +9.36% |

These generated-code totals include repeated benchmark-worker compilation;
they are not a deduplicated resident-code footprint. Constructor direct-call
inlining can increase native code and machine blocks, so measure both the
aggregate and hot-function deltas rather than assuming lower dispatch cost
outweighs code growth.

The initial cold, single-value `chaos` smoke measures stock **30.800190 ms**
and SOAC apply **350.993068 ms**, an exploratory **0.08775x** ratio. This
single cold value is contaminated by setup and must not be compared with the
normally sampled baseline or reported as a performance regression/win.
Profile measures **86.0 ms**; profile/apply worker setup takes **1.29 / 1.80
seconds** respectively, and the release extension rebuild takes **18.34
seconds**. It does establish that the transformed benchmark completes and
compiles **35 functions**.

Compared with the previous comparable one-worker `chaos` smoke at
`work/pyperformance/comparison-20260818-131945-hmVeOQ/summary.json`, the
candidate's optimized typed blocks increase from **846 to 892**
(**+46 / +5.44%**), native emitted bytes increase from **727,336 to 758,184**
(**+30,848 / +4.24%**), native machine blocks increase from **48,293 to
50,229** (**+1,936 / +4.01%**), and serialized BlockPy grows from
**991,150 to 991,214 bytes**. This code-size increase is verified, but its
steady-state throughput effect cannot be inferred from this cold smoke.

A second cold, single-value mixed-subset smoke completes `chaos` at
**31.696 ms stock / 529.695 ms SOAC**, `deltablue` at **1.520 ms stock /
2.157 seconds SOAC**, and `richards` at **22.759 ms stock / 395.972 ms
SOAC**. Profile values are **85.3 ms**, **4.48 ms**, and **46.4 ms**
respectively. Apply worker setup totals **28.6 seconds**, with one worker
taking approximately **20.5 seconds**, versus **3.08 seconds** of measured
values. These extreme cold/startup effects invalidate the single-value
throughput ratios; they do establish a concerning startup/code-growth
cost that must be considered separately from warm steady-state timing.

The extreme cold `deltablue` setup is mostly shared-mount diagnostic artifact
I/O rather than actual linking: **152 JIT commit events** total **22.412
seconds**, of which `jit_commit_code_summary` alone consumes **21.383
seconds (95.4%)** and jitdump consumes **0.975 seconds**; code definition
and finalization take only **0.014 / 0.016 seconds**. Corresponding
code-summary writing accounts for **4.923 / 5.220 seconds** in `richards`
and **2.685 / 2.803 seconds** in `chaos`. The ordinary-sampling candidate has
**92.3 seconds** aggregate apply worker setup with a **4.74-second** maximum
individual setup, rather than a reproducible 20-second worker. Improving
summary/jitdump artifact writing on this VM is separate workflow work: in
`crates/soac_jit/src/jit/backend.rs`, `append_jit_artifact_record` streams
`serde_json::to_writer` directly to an unbuffered `File`, producing many small
writes to the Lima shared mount for each JSONL record. A separate proposed
strategy is to serialize one complete record plus its newline and issue one
`write_all` while preserving immediate JSONL visibility. This writer change
has not been implemented; do not confuse artifact serialization latency with
measured Python throughput.

Compared with the prior one-worker-per-benchmark mixed smoke at
`work/pyperformance/comparison-20260818-132418-tbIyps/summary.json`, the
new mixed smoke increases optimized typed blocks from **1,897 to 2,055**
(**+158 / +8.33%**), native emitted bytes from **1,428,864 to 1,567,940**
(**+139,076 / +9.73%**), and native machine blocks from **94,932 to 103,818**
(**+8,886 / +9.36%**). Benchmark transformed-module coverage and compiled
function counts remain unchanged; actual constructor direct edges in those
benchmark hot functions are confirmed separately in the `chaos` artifact.

Per-function apply code summaries from the previous ordinary-sampling baseline
and the one-worker candidate independently establish that constructor entry
compilation and caller usage are distinct:

| `chaos` apply function | Previous native bytes | Candidate native bytes | Previous / candidate typed blocks | Direct-edge observation |
| --- | --- | --- | --- | --- |
| `GVector.linear_combination` | 14,548 | 12,432 (-14.55%) | 4 / 9 | new constructor edge: 0 to 1 |
| `GVector.__mul__` | 2,772 | 4,404 (+58.87%) | 1 / 6 | new constructor edge: 0 to 1 |
| `GVector.__add__` | 9,396 | 9,348 (-0.51%) | 4 / 9 | new constructor edge: 0 to 1 |
| `Chaosgame.create_image_chaos` | 40,408 | 56,376 (+39.52%) | 45 / 71 | all direct edges: 4 to 6 |
| `Spline.__call__` | 92,792 | 92,792 (unchanged) | unchanged | all direct edges: 3, unchanged |

`GVector.linear_combination` machine blocks fall from **1,041 to 842** even
as its typed blocks increase. `Chaosgame.create_image_chaos` machine blocks
grow from **2,610 to 3,732**. The hot constructor transformation is therefore
real, but mixed local code-size effects and large caller growth make measured
steady-state throughput essential before retaining the strategy.

| Constructor evidence | Baseline | Candidate |
| --- | --- | --- |
| `GVector.linear_combination` executions in prior decoded profile | 137,970 | pending |
| Safe constructor ID before module body call | 0; focused RED confirmed | nonzero in focused GREEN |
| Matching profile `call_hot_targets` observations | unavailable / zero | at least 4 in focused GREEN |
| Apply-mode actual caller direct edge | no `GVector` constructor caller edges | focused `run` edge; three real `GVector` caller edges |
| Custom `__new__` / metaclass generic fallback | required | zero IDs and ordered effects verified |

## Attempt history

### Attempt 1: Expose safe constructor identity during class creation

- Change: extend the existing transformed class-creation callback to register
  eligible owner types and their synthetic constructor entries before the
  class returns, retaining existing late registration and guards.
- Measurements and coverage: a decoded single-worker baseline records
  137,970 `GVector.linear_combination` executions, significant generic
  type-call/vectorcall overhead, and an existing synthetic constructor entry.
  The focused candidate records at least four synthetic constructor profile
  targets and an actual apply-mode direct edge. The actual `chaos` candidate
  also emits new constructor direct edges in three hot `GVector` callers.
  Cold smokes establish transformed coverage, setup costs, and material
  generated-code growth. A normally sampled comparison yields a robust
  **1.0176311x** three-benchmark median improvement, but with **9.73%** more
  native code and a **0.3567255x** stock-relative geometric mean.
- Compatibility and tests: a dedicated behavior/profile/apply integration is
  authored, including custom-allocation/metaclass fallbacks and side-effect
  ordering. The baseline is genuinely RED: one test fails in **0.45 seconds**
  because its safe constructor ID is zero during initialization, while the
  unsafe constructor IDs and ordered fallback effects remain correct. The
  candidate then passes the same integration in **2.21 seconds**, including
  early ID, profiled synthetic target, actual eager-mode apply direct edge,
  and unchanged unsafe-shape fallbacks. A warning-free focused Rust test-target
  check passes in **5.71 seconds**. Focused constructor, exception/custom-new,
  direct-fallback, and profile-hot-loop checks report **seven passed, two
  deselected**. Those two explicitly selected runtime-bootstrap tests were
  silently excluded by their slow marker. An explicit `--run-slow` rerun
  passes the profile-bootstrap case and reveals one preexisting CPython API
  mismatch after successful runtime import/bootstrap:
  `_testinternalcapi.has_indexed_values` is unavailable in this vendored build.
  Do not describe the complete slow bootstrap suite as passing. The targeted
  slow runtime-profile bootstrap passes, warning-free Rust checks pass, and
  package-scoped formatting/formatting checks pass. The ordinary full
  `just test-all` gate also passes **1,209 Python node IDs / 73 batches** and
  all Rust crate suites.
- Result: focused RED-to-GREEN and real benchmark hot-caller direct edges are
  verified. Representative subset medians show a modest improvement, one
  small benchmark regression, and material code growth. The full correctness
  gate passes; the full-suite performance target remains unverified.
- Reason: eager planning precedes class realization, so direct-edge selection
  must be verified independently and may require a separate timing fix.

### Attempt 2: Keep early registration invisible to custom metaclasses

- Change: review the first passing constructor implementation for
  Python-visible side effects before class-global assignment. A custom
  metaclass can intercept its class's `__module__` lookup even though its
  constructor is unsupported and receives zero metadata.
- Measurements and coverage: this is a semantic ordering issue, not a
  throughput measurement. Extend the integration to record whether the class
  is already bound whenever `__module__` is intercepted. The strengthened
  integration is genuinely RED: it records `[False, True]`, exposing the
  invalid early interception and then the existing valid late lookup.
- Compatibility and tests: require the existing late registration lookup to
  remain after assignment; early same-module checks must first reject unsafe
  metaclasses/allocation slots and avoid descriptor or equality callbacks.
  The raw-eligibility and exact-Unicode repair restores `[True]` without
  losing early safe-constructor registration, profile target evidence, or the
  apply direct edge; the strengthened test passes in **2.09 seconds**.
- Result: the second genuine semantic RED is GREEN. Review nested custom
  metaclass recursion separately before benchmarking.

### Attempt 3: Defer unsafe nested class graphs to the late sweep

- Change: extend the already-passing top-level custom-metaclass integration
  with a raw-safe `Outer` containing an `Inner` that uses an intercepting
  custom metaclass.
- Measurements and coverage: the strengthened integration is a third genuine
  semantic RED. Existing recursive owner registration intercepts the inner
  `__module__` lookup with assignment states `[False, True]`: the first
  callback happens before `Outer` is globally bound, while the second is the
  valid unchanged late-sweep observation.
- Compatibility and tests: inspect the complete nested owner graph using
  cycle-safe raw CPython type/dictionary eligibility checks. If any reachable
  nested owner is unsafe, skip early registration of the entire outer graph;
  the existing module-wide sweep must retain its normal post-assignment
  behavior. The private `owner_type_supports_early_registration` preflight
  checks raw heap status, exact metaclass, allocation/new slots, and exact
  Unicode module identity while tracking visited types in a `HashSet`.
- Result: genuine third RED-to-GREEN. Cycle-safe raw `tp_dict` preflight
  rejects the entire unsafe nested graph, preserves only the existing
  post-assignment inner lookup, and retains the safe constructor's actual
  apply direct edge. The combined focused constructor/custom-shape/direct
  fallback/hot-loop guardrails pass **seven tests in 12.21 seconds**.

## Verdict and next action

- Verdict: **LANDED / RETAIN**. Focused and actual `GVector` hot-caller constructor
  apply direct edges are verified, both top-level/nested custom-metaclass
  visibility regressions pass, and robust mixed-subset medians improve by
  **1.0176311x**. This modest result comes with **9.73%** more native code,
  a small `richards` median regression, substantial diagnostic-artifact I/O,
  and a **0.3567255x** paired-stock geometric mean. The complete correctness
  gate passes; the complete pyperformance suite and **1.10x** acceptance
  target remain pending.
- Transferable lesson: synthetic targets, profile evidence, runtime type
  metadata, planner decisions, actual caller edges, and benchmark throughput
  are distinct boundaries; validate each instead of treating one as proof of
  the others.
- Next action: retain the validated general constructor-coverage improvement,
  monitor its generated-code growth, and investigate shared-mount JSONL
  artifact serialization as a separate optimization/workflow strategy. The
  full-suite **1.10x** objective remains outstanding.
