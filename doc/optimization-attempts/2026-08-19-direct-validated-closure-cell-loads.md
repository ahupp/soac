---
title: "Direct Validated Closure-Cell Loads"
---

# Direct validated closure-cell loads

- Status: **REJECTED; GENUINE EXPORTED-HELPER RED-TO-GREEN AND ZERO-LOSS
  MECHANISM CONFIRMED, BUT REPEATED TARGETED THROUGHPUT IS NEUTRAL;
  PRODUCTION / SPECIALIZATION RESTORED TO MAIN; NO FULL GATE OR PERF LOG**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`zwkrytkq`**, commit
  **`443b2e42`**.
- Candidate revision: change **`puozyplw`**, commit **`c098168e`**;
  one-file experimental implementation passes its focused exported-helper
  regression but is **rejected** for no measurable end-to-end benefit.
- Outcome: determine whether an existing already-validated closure-cell
  helper can directly read pinned CPython cell fields while retaining its
  exported ABI, all user-visible behavior, and generated-code coverage.

## Hypothesis and evidence

- General-purpose opportunity: closure and generator loads repeatedly call
  a validated helper that still dispatches through exported `PyCell_Get`
  and a separate redundant `ffi::Py_TYPE` / PLT indirection. The existing
  public `ffi::PyObject.ob_type` plus a small explicit pinned-CPython
  cell-layout mirror can expose the cell's `ob_ref`, so direct field reads
  may remove indirection after the same nonnull/exact-cell checks.
- Fresh current integrated chaos zero-loss profile prefix
  `work/logs/direct-cell-read-baseline-chaos_*` exists with **70 replay
  loops / 199 Hz**, **599 raw recorded samples**, **305 distinct aggregated
  Speedscope stacks**, and zero sample loss. Existing `dp_jit_load_cell`
  has **3.50616% inclusive / 0.83480% self**. Its nested exported
  `PyCell_Get` subtree contributes **2.00352 percentage points**, but that
  includes **1.00176 points of required immortal-aware `Py_INCREF` work**
  that the optimization must retain. Truly removable getter wrappers are
  `PyCell_Get` **0.50088**, PLT **0.33392**, and `GetRef` **0.16696**;
  separate outer exact-type/PLT wrappers are **0.50088 + 0.16696**. The
  honest disjoint removable upper bound is therefore only
  **1.66960 percentage points**, not the misleading **2.67136-point**
  ancestry sum that includes required ownership. The helper's **0.83480%**
  self work also remains. JIT owners are `Spline.__call__` **2.67136%**
  and its nested list comprehension **0.83480%**. Attached replay
  **42.8094 ms** is diagnostic only.
- Current comprehensions baseline has **618 raw zero-loss samples**, with
  approximate helper **0.971%**, nested `PyCell_Get` **0.648%**, and
  `Py_TYPE` **0.162%**. The estimated removable ancestry there is only
  approximately **0.8095 percentage points**, with required owned-reference
  work still retained. Any comprehensions improvement is expected to be
  small; no candidate workload gain is established.
- Integrated normal fixed-eight comparison **090414** has stock geometric
  score **0.5883463026285985x**, and targeted fixed-four comparison
  **090720** has exact stock score **0.4290269750586277x**. Subsets do not
  establish full-suite acceptance, and the **1.10x full-suite stock**
  target remains unmet.
- Existing Apply coverage is **23,293,040 native bytes / 1,533,550 machine
  blocks** and **2,866 optimized typed blocks / 204 functions**. Existing
  compiler helper ABI, generated code, and callable coverage should remain
  unchanged, but candidate invariance is not yet verified.
- No existing CPython-visible correctness defect is asserted. The proposed
  change removes redundant indirection only when all old validations,
  exception behavior, and ownership rules remain identical.
- A genuine unchanged-production structured JIT regression now invokes the
  **actual exported closure-cell helper** and fails **0 passed / 1 failed /
  568 filtered** at the exact final assertion: **"the actual exported
  cell-load helper must select the direct owned cell contents path"**.
  All existing behavior prerequisites pass first: a mortal result acquires
  one owned reference and releases it, live cell contents can be replaced,
  immortal values remain valid, an empty cell raises the exact
  `UnboundLocalError`, malformed list and null inputs raise the exact
  existing `RuntimeError`s, and a real Python `__del__` runs only after the
  cell is destroyed and the final owned result is released. The shared test
  mutex is dropped before the intentional RED, preventing unrelated lock
  poisoning. The first **23.04-second** build is workflow overhead only,
  not candidate runtime or performance evidence.
- The first production implementation then fails to compile with Rust
  **E0425**: macOS host cache `pyo3-ffi-0.29.0` exports
  `ffi::PyCellObject`, but the actual guest Cargo workspace uses patched
  PyO3 git checkout **`9072f6c`**, which does **not** re-export that type.
  The host crate cache is therefore not evidence for the real build API.
  The approved same-file correction is a private, explicit pinned-layout
  **`#[repr(C)] RawPyCellObject { ob_base: ffi::PyObject,
  ob_ref: *mut ffi::PyObject }`**, following the project `RawPy*` naming
  convention instead of pointer arithmetic. This dependency mismatch is
  resolved without changing the public API or adding a second file.
- The same actual exported-hook regression now turns **RED-to-GREEN:
  1 passed / 568 filtered in 0.01 seconds**. The corrected one-file path
  directly reads public `ffi::PyObject.ob_type` and private
  `RawPyCellObject.ob_ref`, retaining the existing immortal-aware
  **`Py_XINCREF`**, including its null-safe behavior. The obsolete private
  `PyCell_Get` extern and unreachable fallback are removed. Real mortal
  ownership **+1 / release**, live cell replacement, immortal values,
  exact empty-cell `UnboundLocalError`, malformed list/null
  `RuntimeError`s, and delayed Python finalizers all pass. The complete
  complete JIT library and all Cargo test targets each pass **569 / 569**.
  Broad transformed compatibility passes **37 tests**, with **1 existing
  documented eval-closure expected failure**, across empty/mutated/deleted
  cells, wrong/null input, owned/free cells, generators, class cells,
  finalizers, monitoring, StopIteration, and scalar paths. Package-scoped
  formatting and the JIT all-target Cargo check pass. Candidate performance
  Candidate production is rejected before running the full correctness
  gate; focused Rust, transformed, and scoped validation remain genuine.
- Release debug-single fixed-eight smoke comparison **093518-KU8DLt**
  against integrated guarded-runtime baseline **090221** completes
  **8 / 8**. Two independent audits match every measured worker PID and
  every function's `(entry_kind, id, qualname, bytes, blocks)` exactly:
  aggregate native code remains **2,253,100 bytes / 148,734 machine
  blocks**, optimized coverage **2,866 typed blocks / 204 functions**, and
  error count **zero**. Cold one-iteration timings and smoke geometric
  means are not throughput evidence. Normally sampled fixed-eight and
  targeted three-round comparisons plus the full correctness gate remain
  **pending** at the smoke stage; no throughput conclusion can come from
  cold one-iteration timings.
- Normally sampled fixed-eight comparison **093702** against integrated
  guarded-runtime baseline **090414** completes with stock score
  **0.5898185869862203x** versus **0.5883463026285985x**, and official
  previous-SOAC geometry **1.0078220782447407x**. Independent fixed-eight
  robust previous geometry is **1.021857x / 1.015815x stock-adjusted**.
  Primary target chaos remains **inconclusive**: raw **0.987502x
  [0.90365, 1.03526]**, matched stock drift **0.961724x**, and paired
  **1.026804x [0.93217, 1.08252]**. Comprehensions shows raw **1.051210x
  [1.00395, 1.13014]** and paired **1.071682x**, but that gain is far
  larger than its source-backed approximately **0.8-point** removable
  profile ancestry and may reflect platform noise; it is not causal proof.
  Deltablue/richards are paired-neutral and other controls drift. Every
  measured function across all **80 worker PIDs** preserves exactly
  **23,293,040 native bytes / 1,533,550 machine blocks**, with unchanged
  **2,866 typed blocks / 204 functions** and zero errors. Targeted
  three-round comparison and matched candidate zero-loss profiling are now
  complete; the strategy is rejected for neutral target throughput.
- Matched three-round comparison **094009** against integrated targeted
  baseline **090720** pools **60 samples** with round-stratified worker
  intervals. Primary target chaos is **0.9989209x [0.974108, 1.023993]**,
  or **1.0035838x paired [0.970724, 1.040379]**; comprehensions is
  **1.0106112x [0.980419, 1.034464]**, or **0.9995720x paired
  [0.968234, 1.032001]**. Deltablue **0.990944x / 0.985960x paired** and
  richards **0.998526x / 0.991161x paired** are likewise neutral.
  Four-workload robust geometry is **0.9997258x raw / 0.9950451x
  stock-adjusted**. The official arithmetic approximately **1.017x** is
  misleading due to outlier sensitivity and must not be reported as a
  meaningful gain. All **120 candidate measured workers/functions** retain
  exactly **18,352,680 native bytes / 1,206,840 machine blocks per
  round**, with zero errors. There is **no measurable target throughput
  benefit**, so the strategy is **REJECTED** despite technically successful
  targeted mechanism removal.
- Matched **70-loop / 199-Hz** chaos profiles are both zero-loss, with
  **599 -> 604 raw samples**. Existing `dp_jit_load_cell` inclusive
  ancestry decreases **3.50616% -> 0.49648%**; the outer cell
  `Py_TYPE` / PLT subtree **0.66784% -> 0%** and nested `PyCell_Get` /
  `GetRef` subtree **2.00352% -> 0%** disappear as standalone frames.
  Critically, the baseline subtree includes **1.00176% mandatory
  immortal-aware owned-reference `Py_INCREF`**; that reference work is
  semantically retained and inlined, **not eliminated**. GC is effectively
  unchanged at **0.3339% -> 0.3317%**. Attached replay changes
  **42.8094 -> 43.2203 ms (approximately 0.99049x)**, slightly slower,
  but remains diagnostic only. Mechanism elimination without a supported
  workload gain does not justify the added private CPython layout mirror
  or its maintenance burden.

## Implementation and compatibility

- Proposed production scope: exactly one existing file,
  `crates/soac_jit/src/jit/specialized_helpers.rs`. Preserve the existing
  exported helper name, signature, call sites, and native entry shape; add
  no public API, runtime helper, global state, IR shape, or integration
  file.
- After the existing exact nonnull cell validation, compare the public
  `ffi::PyObject.ob_type` field directly and read the pinned vendored cell
  reference through a private explicit C-layout
  `RawPyCellObject { ob_base, ob_ref }`; the actual workspace-patched PyO3
  does not expose `ffi::PyCellObject`. Preserve the existing
  immortal-aware `Py_INCREF` / `Py_XINCREF` owned-reference behavior and
  do not count required reference work as removable. Keep existing null,
  non-cell, and empty-cell exception paths and exception types unchanged.
- The crate already rejects free-threaded CPython builds, so the pinned
  GIL-enabled layout and direct field access are the narrow precondition;
  do not imply support for unsupported builds or alternate cell layouts.
- Preserve mortal and immortal reference handling, cell mutation visibility,
  generator/coroutine closures, finalizer ordering, owned-result lifetime,
  and every previously exported helper ABI offset. Never cache a cell value
  or suppress the original exception for a null/wrong/empty input. The
  passing implementation removes the unreachable fallback and obsolete
  private `PyCell_Get` extern rather than retaining dead compatibility
  paths.
- The genuine unchanged-production structured RED already proves actual
  exported-helper execution and validates null/wrong/empty inputs, mortal
  and immortal ownership, live mutation, and real Python finalization before
  failing solely on the missing direct owned-cell path. The candidate now
  turns that same actual exported-hook regression GREEN **1 / 568 filtered
  in 0.01 seconds**. Full JIT library and all test targets each pass
  **569 / 569**; transformed compatibility passes **37 tests / 1 existing
  documented eval-closure XFAIL**, and scoped formatting/all-target checks
  pass. Candidate benchmarks are complete and neutral; the full gate is
  **not run because the candidate is rejected**.

## Benchmark protocol and coverage

- Fixed normal selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`, against
  the same vendored stock CPython and independently profiled integrated
  guarded-runtime baseline.
- Baseline artifacts: normal comparison **090414**, targeted three-round
  comparison **090720**, and the fresh current chaos zero-loss profile
  `work/logs/direct-cell-read-baseline-chaos_*`, independently verified at
  **599 raw samples / 305 aggregated stacks / zero loss**. Distinguish
  required owned-reference work from genuinely removable wrapper ancestry
  before assigning a candidate target or speedup.
- Require preserved transformed closure/generator, cell mutation, watcher,
  StopIteration, inherited/non-self/scalar, factory, and constructor
  semantics. Compare robust matched medians and stock adjustment; attached
  native replay is diagnostic and cannot substitute for throughput.
- Candidate smoke, normal and targeted comparisons, matched native
  profiles, emitted-code invariance, and focused compatibility are
  complete. The full correctness gate is **not run because the production
  change is rejected and has been restored to the integrated baseline**.

## Measurements

| Metric | Integrated guarded-runtime baseline | Candidate | Change |
| --- | --- | --- | --- |
| Normal fixed-eight paired stock / SOAC geometry | 0.5883463026285985x | 0.5898185869862203x | full-suite stock 1.10x goal unmet |
| Normal fixed-eight official previous-SOAC geometry | integrated comparison 090414 | 1.0078220782447407x | single round; primary chaos inconclusive |
| Targeted fixed-four paired stock / SOAC geometry | 0.4290269750586277x | pending | subset only; not full-suite acceptance |
| Previous-SOAC robust / stock-adjusted improvement | integrated `zwkrytkq/443b2e42` | 1.021857x / 1.015815x | full fixed-eight; target confidence inconclusive |
| Targeted three-round raw / stock-adjusted geometry | integrated comparison 090720 | 0.9997258x / 0.9950451x | statistically neutral; arithmetic approximately 1.017x distorted by outliers |
| Targeted chaos raw / stock-adjusted improvement | integrated comparison 090720 | 0.9989209x / 1.0035838x | raw CI [0.974108, 1.023993]; paired CI [0.970724, 1.040379]; no measurable gain |
| Targeted comprehensions raw / stock-adjusted improvement | integrated comparison 090720 | 1.0106112x / 0.9995720x | raw CI [0.980419, 1.034464]; paired CI [0.968234, 1.032001]; no measurable gain |
| Normal chaos raw / stock-adjusted improvement | integrated comparison 090414 | 0.987502x / 1.026804x | raw CI [0.90365, 1.03526], paired CI [0.93217, 1.08252]; inconclusive |
| Normal comprehensions raw / stock-adjusted improvement | integrated comparison 090414 | 1.051210x / 1.071682x | raw CI [1.00395, 1.13014]; exceeds plausible approximately 0.8-point source opportunity |
| Optimized typed-IR blocks / functions | 2,866 / 204 | 2,866 / 204 | unchanged |
| Apply-mode native code bytes / machine blocks | 23,293,040 / 1,533,550 | 23,293,040 / 1,533,550 | all 80 measured worker PIDs/functions identical; zero errors |
| Targeted per-round native code bytes / machine blocks | 18,352,680 / 1,206,840 | 18,352,680 / 1,206,840 | all 120 measured worker PIDs/functions identical; zero errors |
| Release debug-single fixed-eight smoke | guarded-runtime comparison 090221 | 8 / 8; 2,253,100 bytes / 148,734 blocks; typed 2,866 / 204 | every PID/function tuple identical; zero errors; cold timings invalid |
| Fresh chaos zero-loss samples / aggregated stacks | 599 raw / 305 aggregated stacks | 604 raw samples | matched 70 loops / 199 Hz; both zero-loss |
| Current chaos helper inclusive / self | 3.50616% / 0.83480% | 0.49648% inclusive | mechanism ancestry decreases; workload throughput remains neutral |
| Current chaos nested getter / required Py_INCREF ancestry | 2.00352% / 1.00176% | no standalone getter frame; required INCREF inlined and retained | owned-reference work not eliminated |
| Current chaos truly removable getter / outer type wrappers | 1.00176% / 0.66784% | getter/type wrapper frames disappear | honest disjoint removable ceiling 1.66960 percentage points |
| Matched attached chaos replay | 42.8094 ms | 43.2203 ms | approximately 0.99049x; diagnostic only, not acceptance evidence |
| Current comprehensions helper / nested PyCell_Get / Py_TYPE | approximately 0.971% / 0.648% / 0.162% | pending | 618-sample baseline; likely small |
| Genuine production-path structured regression | 0 passed / 1 failed / 568 filtered; actual exported helper lacks direct owned-cell path | 1 passed / 568 filtered in 0.01 s | genuine RED-to-GREEN; mortal/immortal/refcount/mutation/error/finalizer controls pass |
| First candidate compilation against actual workspace PyO3 | host cache exposes `ffi::PyCellObject` | initial E0425 resolved using private explicit `#[repr(C)] RawPyCellObject` | patched PyO3 checkout `9072f6c`; no public API or pointer arithmetic |
| Complete JIT Rust library | integrated guarded-runtime baseline 568 tests | 569 / 569 passed | GREEN; includes actual exported-helper ownership/error/finalizer regression |
| Complete JIT Cargo test targets | integrated guarded-runtime baseline 568 tests | 569 / 569 passed | GREEN |
| Broad transformed closure / generator compatibility | integrated guarded-runtime baseline | 37 passed / 1 existing documented eval-closure XFAIL | GREEN; empty/mutated/deleted cells, refs, generators, class cells, finalizers, monitors, StopIteration, scalars |
| Scoped JIT formatting / all-target Cargo check | integrated guarded-runtime baseline | both passed | GREEN |
| Existing helper ABI / native function coverage | unchanged integrated baseline | pending | no public API/runtime helper/global planned |
| Full `just test-all` correctness gate | integrated baseline previously passed | not run | optimization rejected; production code is not retained |

## Attempt history

### Attempt 1: identify redundant validated-cell indirection

- Change: identify the existing validated cell-helper call chain and public
  pinned CPython fields before approving any code change. Restrict potential
  production edits to the single existing specialized-helpers file.
- Measurements and coverage: a fresh integrated **70-loop / 199-Hz** chaos
  profile contains **599 raw samples / 305 aggregated stacks / zero loss**.
  Helper inclusive **3.50616%** contains **1.00176% required owned
  reference work** and **0.83480% self work**, neither removable. The
  genuine disjoint getter/type-wrapper ceiling is only **1.66960
  percentage points**; current comprehensions is smaller.
  Existing normal and targeted stock scores are
  **0.5883463026285985x / 0.4290269750586277x**.
- Compatibility and tests: preserve null/wrong/empty exceptions, mortal and
  immortal ownership, live cells, generator/finalizer behavior, existing
  exported ABI, and GIL-only build boundary. The genuine actual exported-
  helper structured RED fails **0 passed / 1 failed / 568 filtered** only
  at the required direct-owned-cell-path assertion after all real
  refcount/replacement/immortal/empty/wrong/null/finalizer controls pass.
  The mutex is released before the intentional failure; the one-time
  **23.04-second** build is workflow-only. First sole-file implementation
  fails with **E0425** because host cached PyO3 exports `PyCellObject`
  while the actual workspace-patched git checkout **`9072f6c`** does not.
  The private explicit `#[repr(C)] RawPyCellObject` correction then turns
  the actual exported-helper regression **GREEN 1 / 568 filtered in 0.01
  seconds**, preserving immortal-aware `Py_XINCREF`, all exact errors and
  ownership/finalizer controls, and the existing public ABI. The obsolete
  private extern and unreachable fallback are removed. The complete JIT
  library and all Cargo test targets each pass **569 / 569**; transformed
  compatibility passes **37 tests / 1 existing documented eval-closure
  XFAIL**, and scoped formatting/all-target checks pass. Release
  debug-single fixed-eight smoke passes **8 / 8**, with independent
  per-PID/function native identity and zero errors; cold smoke times are
  invalid. The normal fixed-eight comparison reports stock
  **0.5898185869862203x**, official previous **1.0078220782447407x**,
  robust **1.021857x**, and unchanged generated code, but primary chaos is
  statistically inconclusive and the apparent comprehension gain exceeds
  its plausible profile opportunity. The matched three-round repeat then
  shows neutral chaos **0.9989209x**, neutral comprehensions
  **1.0106112x**, and neutral subset **0.9997258x**. Matched zero-loss
  chaos profiles confirm helper ancestry **3.50616% -> 0.49648%** and
  removal of wrapper frames, while mandatory **1.00176%** owned-reference
  work remains semantically intact and inlined; GC is stable, and attached
  replay is slightly slower. The lack of measurable end-to-end benefit
  outweighs adding a private CPython layout mirror. The full gate is not
  run because production and specialization changes are rejected.
- Result: **REJECTED; MECHANISM ELIMINATED BUT MATCHED TARGET WORKLOADS
  REMAIN NEUTRAL; ONLY THE NEGATIVE STRATEGY RECORD IS RETAINED**.
- Reason: direct pinned layout access is potentially valid only after the
  same existing validation and owned-result semantics; replacing generic
  checks or weakening empty-cell behavior would be user-visible.

## Verdict and next action

- Verdict: **REJECTED; NO MEASURABLE MATCHED TARGET THROUGHPUT DESPITE
  CONFIRMED ZERO-LOSS MECHANISM ELIMINATION**. Genuine exported-helper
  RED-to-GREEN, JIT library/all targets **569 / 569**, transformed
  **37 pass / 1 existing XFAIL**, and scoped checks remain green. Release
  debug-single smoke passes **8 / 8**, with identical
  per-PID/function native code and zero errors; its cold timings are
  invalid. Normal fixed-eight stock is **0.5898185869862203x**, previous
  **1.0078220782447407x**, and all 80 measured workers preserve native
  code, but primary chaos remains statistically inconclusive and controls
  drift. Targeted primary chaos and comprehensions confidence intervals
  both cross neutral, and raw/paired subset geometry is
  **0.9997258x / 0.9950451x**. No attributable candidate throughput gain
  exists. Matched profiles prove helper ancestry **3.50616% -> 0.49648%**
  and removed getter/type wrappers, but the required owned-reference
  increment remains; attached replay is slightly slower and diagnostic
  only. Adding a private pinned-ABI layout mirror is not justified without
  reproducible workload benefit. Production and specialization changes are
  have been restored by the root owner to the integrated baseline; **no full
  `just test-all` is run and
  no `doc/PERF_LOG.md`
  retained-change entry is created**. Only this negative strategy record is
  preserved. The full-suite **1.10x stock** goal remains unmet.
- Transferable lesson: separate helper-inclusive profile ancestry from
  disjoint child indirection, and preserve existing ownership/error
  contracts when replacing a CPython C-API convenience function.
- Next action: retain only this negative strategy record; experimental
  production and specialization changes are already restored. Pursue a
  source-backed optimization with a larger reproducible workload impact.
