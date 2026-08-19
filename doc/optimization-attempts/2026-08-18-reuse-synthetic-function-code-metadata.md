---
title: "Reuse immutable synthetic-function code metadata"
---

# Reuse immutable synthetic-function code metadata

- Status: LANDED / RETAIN for CPython function-metadata/watcher correctness
  only; no measurable throughput win.
- Pacific date: 2026-08-18 PDT.
- Baseline: main change `ourqpywu`, commit `faf146fd`, after the retained
  compiler-owned sync/async iteration-exception correctness repair.
- Outcome: zero-loss native profiles identify repeated synthetic-function
  name/qualname allocations and metadata setters after canonical code has
  already been prepared. Vendored CPython can initialize a new function
  directly from immutable `co_name` / `co_qualname`. A genuine standalone
  watcher regression fails on unchanged production (**1 failed in 0.44
  seconds**), with all three captured synthetic functions reporting
  `name_identity=[False, False, False]`. The canonical-only one-file
  candidate now passes the same watcher regression (**1 passed in 0.44
  seconds**), including shared name/qualname identity and zero redundant
  watcher modification events. Expanded semantic guardrails pass **14 tests
  in 10.66 seconds**; release smoke completes all four workloads with
  unchanged native code. Normal-sampling differences are nonsignificant,
  noisy medians lean slightly negative, and an independent zero-loss native
  profile does not show reduced closure/string CPU. Retained only for
  correct CPython string identity, removal of spurious watcher events, and a
  simpler canonical construction path; the complete gate passes **1,213
  Python nodeids / 77 batches** plus all Rust suites. No `doc/PERF_LOG.md`
  entry is warranted.

## Hypothesis and evidence

The retained synthetic closure-code cache already prepares one immutable
synthetic `CodeType` with the correct `co_name` and `co_qualname` for each
eligible `FunctionInstantiationTemplate`. Nevertheless,
`crates/soac_jit/src/function_instantiation.rs` still performs redundant work
for every newly instantiated closure:

1. `build_closure_shaped_entry_from_ordered_captures` allocates a fresh
   `PyString::new(py, qualname)` and passes it to
   `PyFunction_NewWithQualName` even when the prepared code already owns the
   exact immutable qualname.
2. `update_function_metadata` then unconditionally writes both
   `func.__qualname__` and `func.__name__`, even though CPython initialized
   these fields from the correctly named prepared code.

Zero-loss native profiling demonstrates general closure-construction cost:

- `comprehensions`: synthetic closure creation accounts for **26.25%**
  inclusive CPU. Python Unicode/string construction accounts for **3.49%**
  overall and **2.58%** within closure construction; redundant function
  metadata/qualname setters account for approximately **1.82%**.
- `chaos`: the already-optimized closure-instantiation subtree remains
  approximately **7.85%** inclusive, with approximately **0.87%** in
  Python Unicode/string creation.
- These are overlapping inclusive shares, not additive CPU percentages.
  Provenance: `work/logs/comprehensions-shutdown-flush-steady_speedscope.json`
  and `work/logs/chaos-synthetic-closure-cache_speedscope.json`.

The vendored CPython source provides a narrow sound implementation boundary:
`vendor/cpython/Objects/funcobject.c::PyFunction_NewWithQualName` always
initializes `func_name` from `code_obj->co_name`; when its `qualname`
argument is null, it reuses `code_obj->co_qualname` rather than constructing
a fresh Python string. `PyFunction_New` delegates to that same constructor
with a null qualname. Therefore a correctly prepared immutable synthetic
code object already contains the complete metadata needed for a fresh
function, without post-creation setters.

CPython's qualname setter emits `PyFunction_EVENT_MODIFY_QUALNAME`. Merely
removing a setter is observer-sensitive: a focused function-watcher
regression must establish the actual create/modify event sequence, final
function metadata, and compatibility with ordinary CPython function creation
before treating redundant event removal as sound. Do not suppress actual
user-driven or fallback-path metadata mutation events.

The dedicated standalone regression retains exactly **three** transient
captured-list-comprehension function objects with an actual CPython function
watcher. Unchanged production genuinely fails in **0.44 seconds** because all
three have `name_identity=[False, False, False]`. Watcher callbacks report
no errors; independent captures/function objects, the noncanonical factory's
**two** calls, and source-backed nested-function identity already pass. The
first failing baseline assertion prevented independent observation of the
later baseline qualname-identity/event-5 assertions. After the one-file
canonical-only fix, the unchanged regression
`tests/test_synthetic_function_metadata.py::test_synthetic_functions_reuse_prepared_code_metadata_without_extra_events`
passes (**1 passed in 0.44 seconds**): every synthetic function's `__name__`
and `__qualname__` is the identical immutable code-object string, and there
are **zero `PyFunction_EVENT_MODIFY_QUALNAME` events**. Distinct closure
cells/function identities, the noncanonical factory's two calls, original
source code identity, and real post-creation user metadata mutations all
continue to pass. The initial focused baseline invocation also paid a
**27.72-second release-to-debug extension rebuild** before the short actual
test, a workflow issue rather than a candidate performance measurement.

Expanded focused validation now passes **14 tests in 10.66 seconds**,
covering the existing closure cache, factory reentry, runtime-module
replacement, function-code/default mutation, original code identity,
deterministic shutdown flushing, and synthetic sync/async exception
shadowing. `cargo check -p soac_jit --tests` is warning-free; both
`just fmt-rust soac_jit` and `just fmt-rust-check soac_jit` pass. A later
release-to-debug extension switch cost **37.3 seconds**, again a workflow
rebuild cost rather than measured candidate throughput.

The final `just test-all` passes;
`work/logs/synthetic-function-metadata-test-all.log` records **1,213 Python
nodeids across 77 passing batches**, **545 `soac_jit`**, **368
`soac_lowering`**, **202 `soac_opt`**, and **eight PyO3** Rust tests. Cargo
takes **95.882 seconds**, pytest **111.806 seconds**, the combined test
phase **207.718 seconds**, and total wall time **242.14 seconds**; the
counter-dump batch takes **107.60 seconds**.

## Implementation and compatibility

- Restrict the optimization to `InstantiatedEntry` values whose existing
  `has_prepared_code_metadata` flag proves canonical
  `PreparedSyntheticCode` eligibility. The cache already verifies the
  runtime module and canonical factory, preserves non-owning runtime-module
  lifetime, and safely handles reentry.
- On that narrow branch, invoke `PyFunction_NewWithQualName` with a null
  qualname pointer so the new Python function shares the prepared immutable
  `co_qualname` and automatically receives `co_name`.
- Skip redundant `__qualname__` / `__name__` setters only for the canonical
  prepared-code path. Continue setting docstrings, annotations, defaults,
  keyword defaults, closures, globals, function metadata/vectorcall state,
  and other user-visible attributes exactly as before.
- Retain the original supplied-qualname construction and metadata setters
  for source-backed original code, mismatched capture layouts, monkeypatched
  runtime factories, replacement runtime modules, and every uncached or
  noncanonical fallback.
- Every invocation must still allocate a fresh function and fresh applicable
  capture cells, preserve distinct closure values and function identities,
  honor factory monkeypatching/runtime replacement/reentry, and leave
  original source function code and metadata unchanged.
- Preserve CPython function-watcher creation observations and real mutable
  watcher callbacks. Any change in redundant compiler-owned qualname
  modification events must be explicitly demonstrated to match the ordinary
  CPython construction boundary; do not silently hide genuine user-visible
  behavior.
- Preserve code audit behavior, profile/runtime shutdown frame emission,
  compiler synthetic exception correctness, owner-type invalidation, and
  source-independent eligibility. Introduce no new public API, environment
  variable, runtime helper, global mutable cache, source fingerprint, or
  benchmark-specific recognition.
- The genuine real-function-watcher regression is RED→GREEN: **1 failed in
  0.44 seconds** before the candidate, then **1 passed in 0.44 seconds**
  after it. Existing closure-cache/reentry/module-replacement,
  function-code/default-mutation, original-code, shutdown, and synthetic
  sync/async guardrails pass **14 tests in 10.66 seconds**.

## Benchmark protocol and coverage

- Fixed now-working general-purpose workload set:
  `chaos,richards,deltablue,comprehensions`. Full acceptance remains the
  complete pyperformance suite at **1.10x stock CPython**; a subset result
  is not acceptance.
- Previous normally sampled baseline:
  `work/pyperformance/comparison-20260818-165506-0dnfty/summary.json`;
  `work/logs/synthetic-iteration-exception-shadowing-normal.log`.
- Candidate completion-only smoke, after semantic GREEN:
  `just pyperformance-compare chaos,richards,deltablue,comprehensions 1 '' --debug-single-value`.
  Cold single-value timings show only completion, code metrics, and coverage.
- Completed release smoke:
  `work/pyperformance/comparison-20260818-173241-jyzyRO/summary.json`;
  `work/logs/synthetic-function-metadata-smoke.log`. All four workloads
  complete in **35.09 seconds**, including a release-extension rebuild, with
  unchanged coverage **35 / 21 / 79 / 53** and unchanged one-worker
  generated code: **2,541 optimized typed blocks / 193 functions**,
  **1,867,424 native bytes**, **123,614 machine blocks**, and **4,085,728
  serialized BlockPy bytes**. Approximate cold single-value Apply readings
  **155 ms / 483 microseconds / 397 ms / 332 ms** are not headline
  throughput and must not be compared with normal samples.
- Candidate ordinary-sampling comparison:
  `just pyperformance-compare chaos,richards,deltablue,comprehensions 1 work/pyperformance/comparison-20260818-165506-0dnfty`.
  Compare paired current stock, directly previous SOAC, robust medians, and
  pyperf significance. Keep the selected workloads and sample mode fixed.
- First completed normal-sampling candidate:
  `work/pyperformance/comparison-20260818-173820-fI0KHb/summary.json`;
  elapsed **109.86 seconds**. All four pyperf previous-SOAC comparisons are
  explicitly **not statistically significant**. The candidate `chaos` mean
  includes a 141 ms outlier; `richards` contains approximately 106 / 143 /
  211 ms outliers. Do not interpret noisy means or paired-stock drift as a
  real slowdown or speedup.
- Independent post-change native profile:
  `work/logs/synthetic-function-metadata-perf.log`; **199 Hz** and
  **30,000 loops**, completed in **10.66 seconds**. Authoritative perf
  records contain **659 baseline / 677 candidate CPU-clock samples**, with
  **zero lost samples**; the candidate capture is **42.456 MB**.
  Separately, exported baseline/candidate Speedscope data contains
  **499 / 495 sampled records**, with candidate weighted total **100,097**;
  the inclusive percentages below use that Speedscope basis, not the raw
  perf-record sample count. Shares overlap and modest changes are
  sampling-sensitive. The profile does not establish a performance win.
- Existing coverage transforms `__main__` plus `soac.runtime` only, with no
  transformed standard-library/dependency modules. Compiled function counts
  are **35 `chaos`**, **21 `comprehensions`**, **79 `deltablue`**, and
  **53 `richards`**.
- Confirm actual hot synthetic-closure metadata reuse through focused
  watcher/allocation evidence and follow-up native profiles, not benchmark
  completion alone.

## Measurements

| Benchmark | Baseline paired stock mean | Baseline SOAC mean | Baseline SOAC median | Candidate SOAC | Previous / candidate |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 30.2793541 ms | 81.8153701 ms | 80.2891995 ms | approximately 86.029 ms; median 80.9584 ms | 0.991735x median; not significant |
| `comprehensions` | 7.8948889 microseconds | 89.4563098 microseconds | 87.9882798 microseconds | approximately 89.228 microseconds; median 88.8435 microseconds | 0.990374x median; not significant |
| `deltablue` | 1.4238250 ms | 4.5462772 ms | 4.4998589 ms | approximately 4.640 ms; median 4.54970 ms | 0.989046x median; not significant |
| `richards` | 25.4002089 ms | 43.4689396 ms | 43.0256230 ms | approximately 62.072 ms; median 44.5541 ms | 0.965693x median; not significant |

The baseline fixed-four paired-stock geometric ratio is
**0.2780522558x**, far below the full-suite **1.10x** acceptance target.
Its `richards` stock mean has a high 11.9 ms standard deviation, so prefer
direct previous-SOAC comparisons and robust medians over cross-run stock
drift.

The first candidate fixed-four paired-stock ratio is approximately
**0.24754976x**, with heavy transient VM outliers; its robust
previous-SOAC median geometric ratio is approximately **0.984153x**.
All four individual previous comparisons are nonsignificant, so neither a
throughput win nor a proven regression is established. The target remains
the full-suite **1.10x** stock ratio, not a noisy subset score.

| Native/codegen guardrail | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| `comprehensions` synthetic closure creation inclusive CPU | 26.265% | 27.482% | no reduction; sample noise |
| `comprehensions` Python string creation inclusive CPU | 3.492% overall / 2.58% under closures | 5.318% overall | no reduction; sample noise |
| `comprehensions` `PyObject_SetAttr` inclusive CPU | 3.189% | 2.364% | overlapping inclusive samples |
| `comprehensions` `handle_func_event` inclusive CPU | 0.911% | 0.738% | overlapping inclusive samples |
| Owner function watcher under closure creation | 0.152% | 0% observed | limited sample; regression confirms no spurious event |
| Synthetic code preparation inclusive CPU | 3.493% | 3.104% | overlapping inclusive samples |
| `chaos` synthetic closure instantiation inclusive CPU | approximately 7.85% | not reprofiled | no claim |
| `chaos` Python string creation inclusive CPU | approximately 0.87% | not reprofiled | no claim |
| Optimized typed-IR final basic blocks | 2,541 | 2,541 | unchanged |
| Optimized typed-IR function instances | 193 | 193 | unchanged |
| Pre-optimization serialized BlockPy bytes | 8,171,456 | 8,171,456 | unchanged |
| Apply-mode native emitted bytes | 18,674,240 | 18,674,240 | unchanged |
| Apply-mode native machine blocks | 1,236,140 | 1,236,140 | unchanged |

## Attempt history

### Attempt 1: Initialize canonical synthetic functions from prepared code

- Change: reuse existing prepared immutable `co_name` / `co_qualname` at
  Python-function creation and avoid redundant metadata setters only for
  canonical synthetic closures.
- Baseline evidence: closure creation accounts for **26.25%** of sampled
  `comprehensions`; Python strings and metadata setters form smaller nested
  hot subtrees. Vendored CPython proves a null qualname reuses code metadata.
- Compatibility: function watcher event sequencing, original source
  functions, factory monkeypatches, `sys.modules` replacement, reentry,
  independent captures, annotations, defaults, and fallbacks must all remain
  valid.
- Tests: genuine focused RED (**1 failed in 0.44 seconds**), exactly three
  created synthetic functions, three false name identities, no callback
  errors, and two expected noncanonical factory calls. Canonical-only
  one-file production change turns the same regression GREEN (**1 passed in
  0.44 seconds**), proving shared name/qualname identity, zero redundant
  qualname watcher events, distinct captures, fallback calls, original code,
  and actual user mutation. Expanded semantic regressions pass **14 tests in
  10.66 seconds**, warning-free JIT Cargo check and scoped formatting/checks
  pass. Four-workload release smoke and normal sampling complete with
  identical coverage/generated code. All normal previous-SOAC differences
  are nonsignificant, with substantial VM outliers; robust median geometric
  ratio is approximately 0.984153x. Independent zero-loss native profiling
  shows closure/string shares 26.265%→27.482% and 3.492%→5.318%, with no
  demonstrated throughput benefit. Full `just test-all` passes **1,213 Python
  nodeids / 77 batches** and the entire Rust workspace.
- Result: **LANDED / RETAIN for correctness only**; real CPython string
  identity and watcher events improve, but no optimization or throughput
  improvement is established and no `doc/PERF_LOG.md` entry is added.

## Verdict and next action

- Verdict: **LANDED / RETAIN for CPython correctness and observer semantics
  only.** A genuine real-watcher RED→GREEN and **14 passing**
  expanded semantic guardrails verify canonical name/qualname identity,
  observer-safe metadata creation, and fallback behavior. First normal
  sampling is inconclusive/slightly negative on noisy medians, all four
  comparisons are nonsignificant, and a zero-loss native profile finds no
  CPU improvement. Retain only for CPython-correct shared name/qualname
  identity, removal of synthetic watcher mutation events, and simpler code.
  The full correctness gate passes **1,213 Python tests / 77 batches**;
  candidate stock score remains approximately **0.24755x**, far below the
  full-suite **1.10x** target. No performance-log entry is warranted.
- Transferable lesson: immutable code objects can already supply canonical
  Python-function names; repeated wrapper allocation/setter calls should not
  be removed without checking CPython observer behavior.
- Next action: investigate a separate source-independent optimization with
  actual statistically supported throughput evidence; keep this change
  classified solely as a CPython correctness/observer fix.
