---
title: "Reuse Source-Backed Function Materialization Metadata"
---

# Source-backed function materialization

- Status: **LANDED / RETAIN; REPEATED SOURCE-BACKED PERFORMANCE,
  WATCHER CORRECTNESS, AND FULL CORRECTNESS GATE VERIFIED**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`nzlwkyzw`**, commit
  **`a3e1960f`**.
- Candidate revision: change **`xtnupnyk`**.
- Outcome: determine whether immutable source-backed code/function-template
  metadata can remove repeated materialization work while preserving every
  CPython-visible fresh-function, closure, default, watcher, and mutation
  behavior.

## Hypothesis and evidence

- General-purpose opportunity: ordinary Python nested functions and
  generator-expression factories repeatedly materialize fresh function
  objects. Reusing only already-immutable compiler-owned template/code facts
  could reduce repeated metadata lookups and registration overhead without
  treating benchmarks specially or sharing mutable function objects.
- Current integrated-baseline `comprehensions` takes approximately
  **84.9 us in SOAC versus 7.92 us in paired stock CPython**. The current
  fixed-eight stock geometric score is only **0.48444263615875466x**, far
  below the full-suite **1.10x** acceptance target; neither this workload
  nor that subset establishes full-suite progress.
- Latest verified zero-loss `comprehensions` native profile contains
  **916 CPU-clock samples**. Function/closure creation accounts for
  **31.213% inclusive**, the original source-backed
  `WidgetTray._any_knobby.<locals>.<genexpr>` family approximately
  **17.57%**, immutable original-code free-variable scanning **2.401%**, and
  function/JIT registration **7.532%**. Inclusive shares overlap and must
  not be added or treated as independently removable work.
- Prior synthetic-only prepared-function metadata reuse was retained for
  **CPython correctness/watcher semantics only**, with no measured throughput
  improvement. It neither proves source-backed speedup nor justifies
  extrapolating the 31% inclusive creation share.
- The genuine unchanged-production regression
  `tests/test_source_function_templates.py` now **fails one test in 0.48
  seconds** (**0.45 seconds actual test time**). Within the same interpreter,
  the real CPython function watcher records stock original genexpr,
  captured nested, and defaulted function creation as **`[0, 0, 0]`** for
  each family. Transformed SOAC instead repeats **`[0, 3, 4, 5]` three
  times** for each family: spurious `MODIFY_DEFAULTS`,
  `MODIFY_KWDEFAULTS`, and `MODIFY_QUALNAME` accompany every CREATE,
  including functions with nonempty defaults. SOAC source-backed name and
  qualified-name identity are also **false** where stock identity is
  **true**.
- All earlier watcher/behavior assertions already pass: CREATE sees
  defaults/keyword-defaults/closure as **`None`**, three function objects
  are distinct but share original code, captured cells stay independent,
  generator evaluation remains lazy, nonempty positional/keyword defaults
  work, subsequent actual user modifications emit the real event sequence
  **`[3, 4, 5]`**, and replaced runtime factories remain observable. The
  RED isolates unnecessary source-backed initialization mutations rather
  than establishing a throughput gain.
- A second independent unchanged-behavior structured JIT regression,
  `source_backed_zero_freevar_function_preserves_code_metadata_identity`,
  genuinely **fails one test / 557 filtered in 0.03 seconds**. It compares
  actual Python object pointers and proves the freshly materialized
  source-backed function's `__name__` is a distinct object from its
  immutable code object's name. The first interpreter-aligned build took
  **22.98 seconds**; that build time is workflow overhead, not benchmark
  evidence. Production implementation began only after both genuine REDs.
- Source-observed implementation milestone, **not yet compiled or
  validated**: the existing private `RawPyCodeVersionPrefix` gains a
  `co_nfreevars` accessor/private re-export; each existing
  `FunctionInstantiationTemplate` obtains Python-reentry-safe `OnceLock`
  storage for immutable original-code facts; zero-freevar source-backed
  functions can skip unnecessary closure tuple materialization; and freshly
  validated positional/keyword defaults are written to direct CPython
  function slots with the required **INCREF**, avoiding spurious watcher
  MODIFY events. Canonical source metadata/module-guard wiring is still
  pending. These are observed source changes, not a successful compile,
  structured GREEN, runtime GREEN, performance result, or new public API.
- The genuine structured production JIT regression subsequently turns
  **RED-to-GREEN**:
  `source_backed_zero_freevar_function_preserves_code_metadata_identity`
  **passes 1 / 1, with 557 tests filtered**, after the immutable
  source-template cache, existing raw zero-freevar prefix, direct fresh
  defaults/keyword-default slots, NULL-qualified-name original constructor,
  and exact-Unicode-guarded module setter are implemented. Independent
  host-side review found no additional issue. The separate real stock-versus-
  SOAC watcher integration subsequently also **passes 1 / 1 in 0.42
  seconds**.
- Genuine standalone watcher RED-to-GREEN: within the same interpreter,
  stock and transformed SOAC both emit **CREATE-only** sequences for original
  zero-freevar genexpr, captured nested, and nonempty positional/keyword
  defaulted functions. Canonical original code/name/qualified-name identity,
  CREATE-before-slot-initialization snapshots of `None`, distinct fresh
  function objects/cells, later genuine MODIFY events and dispatch, and
  replaced synthetic factory behavior all pass. The focused regression is
  **1 / 1 GREEN in 0.42 seconds**. Its first debug-extension rebuild takes
  **22.74 seconds** of workflow/setup overhead, not benchmark time.
- An optional tightly guarded, positive-only, per-template ready-handle
  cache is now **implemented**, bounded to approximately **35 private lines**
  using the existing template `OnceLock`. Its key must match the exact
  compile-session identity and immutable registered code pointer/version;
  interpreted/test-force settings must still be rechecked on **every call**.
  Cache only positive `Some` results, require the live
  `PyFunction.func_code` to equal the registered immutable code snapshot
  before both cache hit and insertion, clone the ready handle for each fresh
  `FunctionEnv`, and preserve the complete normal miss path. The private
  structured `PreparedDirectEntryKey` regression now **passes**, verifying
  same-session/code/version key equality and independent different-session,
  different-code, and different-version key inequality. It does **not**
  exercise actual cache retrieval, insertion, force-mode checks, or the
  current-code guard; those conditions are source-reviewed, and mutation
  behavior is covered separately by the **31 / 31** Python suite. Existing
  profile evidence places
  `lookup_ready_direct_function` at approximately **2.401%** inclusive;
  this is motivation, not measured cache benefit or a new public API.
  The post-cache grouped transformed-runtime suite subsequently **passes
  all 31 / 31 selected tests in 15.81 seconds**.
- Post-cache full Rust validation now **passes all 559 / 559** tests in
  `cargo test -p soac_jit --lib`, including both new structured original
  code/name-identity and ready-cache session/code/version-key equality
  regressions plus existing JIT guardrails. Current-code guards are
  source-reviewed and actual code mutation is exercised by the separate
  Python suite. The test run
  takes **5.50 seconds** after one interpreter-aligned **22.19-second**
  rebuild; rebuild time is workflow overhead only. The grouped transformed
  Python suite subsequently **passes 31 / 31 in 15.81 seconds**, including
  same-interpreter watcher stock parity, source genexpr/original captured/
  defaulted functions, synthetic factory mutation/reentry/module
  replacement, actual code/default/keyword-default/method/generator/
  interpreted-entry mutation, and retained late-owner scalar/fused/fixed
  unpack/shutdown/indexed-counter behavior. The combined
  interpreter-aligned Cargo test-target check also **passes in 5.04
  seconds**, and scoped Rust formatting/format checks pass. Production is
  frozen; representative fixed-eight and independently repeated comparisons
  are complete, and the full correctness gate passes.
- Release debug-single smoke candidate **031917** completes all **8 / 8
  workloads**, preserving **3,069 optimized typed blocks / 218 functions**
  and total smoke-worker generated code **2,314,724 bytes / 153,612 machine
  blocks**. Mode-matched `comprehensions` code for **21 benchmark functions**
  is exactly unchanged, including the hot
  original genexpr **24,452 bytes / 1,692 blocks**,
  `_any_knobby` **1,376 / 90**, and `_add_widgets` **18,876 / 1,284**.
- The apparent debug-single `comprehensions` value of approximately
  **2.44 ms versus 374 us** is **not** steady-state throughput: its measured
  worker spends **345.7 ms**, correlated by timing with **338.3 ms**
  compiling `exception_matches` (prior comparable cold compilation
  **243.1 ms**); those compile events do not carry worker PIDs,
  with first-use template/cache setup also unamortized. One-loop cold debug
  observations cannot establish a candidate win or regression; normally
  sampled fixed-eight and three-round results below provide the actual
  throughput evidence.
- The normally sampled fixed-eight comparison
  `work/pyperformance/comparison-20260819-032221-30yRwR/summary.json`
  completes **8 / 8**. Paired stock geometric score is
  **0.5132971537493283x** versus the prior baseline cohort's
  **0.48444263615875466x**; previous-SOAC arithmetic geometric speedup is
  **1.0548731200914065x**, robust median geometric speedup is
  **1.0426394914876491x**, and paired-stock-adjusted robust speedup is
  **1.0435055437958298x**. `comprehensions` median improves
  **83.212579 → 66.395 us (1.253287x)** and mean improves
  **84.882500147 → 66.456778858 us (1.277258718x)**. Candidate values
  **62.355–70.656 us** do not overlap baseline **79.165–93.288 us**.
- Other full-eight robust median ratios are `chaos` **1.035237x**,
  `deltablue` **1.032370x**, `fannkuch` **0.982661x**, `float`
  **1.049268x**, `nbody` **1.013516x**, `richards` **0.994366x**, and
  `spectral_norm` **1.003416x**. There is no reproduced material guardrail
  regression. Across **80 Apply workers**, native code is exactly unchanged
  at **23,359,400 bytes / 1,549,290 machine blocks** and typed IR remains
  **3,069 blocks / 218 functions**; normal `comprehensions` has unchanged
  **26 generated bodies / 302,812 bytes / 20,042 blocks**.
- The independent three-round targeted comparison
  `work/pyperformance/comparison-20260819-032719-IMdv93/summary.json`
  selects `chaos`, `comprehensions`, `deltablue`, and `richards`. Pooled
  robust previous-SOAC geometric speedup is **1.095507476732x**, paired-
  stock-adjusted robust speedup is **1.092128843x**, and arithmetic
  previous-SOAC speedup is **1.097022084142x**. Repeated comprehensions
  median improves **83.21258 → 66.6000 us (1.249438x)**, or
  **1.240319x** after paired-stock adjustment, with approximate **95%
  interval 1.214–1.353x**. All **60 candidate values (62.1118–75.5767
  us)** lie below all **20 baseline values (79.1651–93.2882 us)**. This
  separate three-round cohort's candidate mean is **66.695371094 us**
  versus prior **84.882500147 us (1.272689525x)** and paired stock
  **7.856961670 us**; do not conflate it with the full-eight one-round mean.
- Other configured-baseline targeted ratios are `chaos` **1.023602x**,
  `deltablue` **1.109611x**, and `richards` **1.01495x**. Alternative
  repeated baseline guardrails are approximately neutral, so do not present
  those secondary movements as independently established optimization
  effects. Targeted paired stock score is only **0.329315198339x**; neither
  subset reaches the full-suite **1.10x stock** acceptance goal.
- Matched **zero-loss** 50,000-loop comprehensions native profiles contain
  **916 baseline → 849 candidate CPU-clock samples**. Inclusive closure
  creation falls **31.213% → 20.971%**, original source-backed creation
  **17.573% → 7.067%**, immutable `co_freevars` scanning **2.401% → 0%**,
  positional/keyword defaults setters **0.218% / 0.436% → 0%**,
  attribute lookup **10.695% → 6.835%**, attribute setting
  **2.729% → 0.118%**, ready-handle lookup **2.401% → 0.118%**, and
  registration **7.532% → 4.832%**. GC remains **14.420% → 14.130%**;
  candidate first-call compile ancestry is **6.246% versus 4.146%**.
  Shares overlap and must not be added. Function-watcher notification is
  **not eliminated**: valid candidate notifications remain approximately
  **0.354%**; only spurious source-initialization MODIFY events disappear.
  Attached replay **80.63 → 72.62 us** is diagnostic only, not the reported
  throughput headline.
- Initial expected benefit was conservatively **3–7% whole-workload
  improvement**, not the full overlapping 31% stack share. Actual repeated
  `comprehensions` achieves approximately **1.25x throughput**, while the
  full eight-workload robust geometric improvement is approximately
  **1.043x**. The optimization is retained for both measured speed and
  CPython-visible watcher correctness; the full correctness gate also passes.

## Implementation and compatibility

- Proposed shape: reuse the existing immutable
  `FunctionInstantiationTemplate` and original source-backed code object;
  derive closure arity from the existing `RawPyCodeVersionPrefix.co_nfreevars`
  instead of repeatedly scanning immutable original free-variable metadata.
  Preserve canonical `__name__` / `__qualname__` code-string identity and
  avoid redundant no-op defaults/keyword-defaults mutations that generate
  observable CPython function-watcher events.
- Current implementation uses only existing/private structures:
  `RawPyCodeVersionPrefix.co_nfreevars`, per-template reentry-safe
  `OnceLock` immutable facts, zero-freevar tuple avoidance, and owned direct
  default-slot initialization with INCREF. Canonical source metadata uses a
  NULL-qualified-name original constructor and exact-Unicode-guarded module
  setter. The genuine structured JIT regression now passes **1 / 1**; no
  new public API is added. The independent same-interpreter watcher
  integration subsequently **passes 1 / 1 in 0.42 seconds**. The bounded
  positive-only session/code/version-keyed ready-handle cache is now
  implemented, with a private structured key-match/mismatch test GREEN.
  The full post-cache JIT library passes **559 / 559**, and grouped
  transformed watcher/source/mutation/runtime revalidation also passes
  **31 / 31 in 15.81 seconds**.
- If justified by actual structure and profile evidence, investigate an
  optional explicitly guarded ready-handle/cache owned by the existing
  compiler/runtime state. Its key, ownership, mutation invalidation, and
  fallback must be explicit; do not add hidden process-global state or
  retain mutable Python function objects as reusable instances.
- The implemented optional private cache is constrained to the
  existing-template `OnceLock`, exact compile-session plus immutable code
  pointer/version identity, positive-only cached readiness, per-call
  interpreted/test-force revalidation, and cloning each compiled handle
  into a **fresh** function environment. The private structural test checks
  `PreparedDirectEntryKey` equality and session/code/version inequality
  only; it does not itself execute retrieval, insertion, interpreted-force,
  or current-code paths. Existing mutation behavior passes in the grouped
  **31 / 31** runtime suite, and the live-code/force guards are
  source-reviewed. No public API is added.
- Preserve CPython's **fresh function-object identity per evaluation**, fresh
  closure cells/captures, current globals/builtins, original code identity,
  actual defaults/keyword-defaults semantics, annotations, source/module
  metadata, finalizer ordering, and every required create/modify watcher
  event. User mutation of an earlier function, its defaults, closure, code,
  annotations, or registration hooks must never change a later fresh
  function unexpectedly.
- Runtime factory replacement/monkeypatching must remain observable wherever
  it is currently part of Python-visible behavior. Noncanonical objects,
  non-source-backed cases, dynamic defaults, mutable code metadata, unsafe
  handles, or invalidated assumptions must execute the existing full path.
  No optimization may silently remove an actual required watcher event.
- Depending on verified implementation details of the exact pinned CPython
  is explicitly allowed, including the existing C-layout prefix; vendored
  CPython changes are also allowed if genuinely necessary, though none are
  anticipated. Distinguish explicit `#[repr(C)]` layouts from `repr(Rust)`
  FFI declarations, and preserve all user-visible CPython semantics.
- Guard lifetime: immutable code/template facts remain valid only while the
  exact source-backed code identity and selected compiler-owned template are
  still the ones used for this fresh materialization. Any optional handle or
  cache must revalidate live owner/factory/default/closure assumptions on
  every use and retain an explicit full fallback.
- Focused regressions: a **323-line** stock-versus-SOAC actual CPython
  `ctypes` function-watcher integration is now saved and host `ast.parse`
  validates its syntax. It exercises original zero-free-variable genexprs,
  captured original nested functions, nonempty positional and keyword
  defaults, pre-initialization CREATE snapshots showing defaults **`None`**,
  no initialization MODIFY event **3 / 4 / 5**, three distinct fresh
  functions/cells, original code/name identity, later user metadata/default
  mutations, and runtime factory replacement. Its unchanged-production
  transformed execution genuinely **fails 1 / 0.48 seconds** on extra
  initialization watcher events and source-name identity. Production scope
  is limited to **four explicitly approved files**. The independent private
  structured zero-freevar/code-name-identity JIT regression also genuinely
  **fails 1 / 557 filtered in 0.03 seconds**. Four-file production
  implementation started only after both REDs. The exact private structured
  JIT regression now **passes 1 / 1, with 557 filtered**; the independent
  real stock-versus-SOAC watcher integration also **passes 1 / 1 in 0.42
  seconds**. Grouped transformed-runtime validation and fixed-eight /
  three-round benchmark comparisons pass; the authoritative full
  correctness gate also **passes**.

## Benchmark protocol and coverage

- Fixed benchmark selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`; prioritize
  transformed source-backed comprehensions/genexpr factories while checking
  all retained optimization guardrails.
- Comparison protocol: same vendored stock CPython, independently regenerated
  revision-specific profiles, normal Apply sampling, identical resources and
  module policy. Any final broad claim requires at least three independently
  started, order-alternating rounds as specified by `OPT_GOAL.md`.
- Baseline artifact:
  `work/pyperformance/comparison-20260819-022850-9kkx1m/summary.json`.
- Candidate artifacts:
  `work/pyperformance/comparison-20260819-032221-30yRwR/summary.json`
  (fixed eight, one round) and
  `work/pyperformance/comparison-20260819-032719-IMdv93/summary.json`
  (four affected/guardrail workloads, three rounds). Their stock, previous-
  SOAC, robust median, and confidence evidence are recorded above.
- Existing transformed project modules: benchmark `__main__` plus
  compiler-owned `soac.runtime`; transformed standard-library modules:
  **none** in the baseline. Baseline per-benchmark compiled-function
  coverage is **34 / 21 / 78 / 1 / 9 / 8 / 53 / 9**, respectively.
- Baseline benchmark completion: **8 / 8**. Candidate release debug-single
  smoke also completes **8 / 8** with unchanged source-backed/genexpr
  generated code. Normal fixed-eight sampling also completes **8 / 8**;
  targeted three-round sampling completes all four selected workloads.
- Profile provenance: prior zero-loss current integrated comprehensions
  profile under `work/logs/guarded-scalar-baseline-comprehensions_*`; the
  sample distribution motivates investigation but is not candidate outcome
  or normally sampled throughput.

## Measurements

| Metric | Integrated scalar baseline | Candidate | Change |
| --- | --- | --- | --- |
| Completed fixed-eight benchmarks | 8 / 8 | 8 / 8 | complete |
| Fixed-eight paired stock / SOAC geometric ratio | 0.48444263615875466x | 0.5132971537493283x | full-suite 1.10x goal unmet |
| Fixed-eight previous-SOAC robust geometric ratio | same source baseline | 1.0426394914876491x | improved |
| Three-round targeted previous-SOAC robust geometric ratio | same source baseline | 1.095507476732x; paired-stock adjusted 1.092128843x | improved |
| `comprehensions` full-eight SOAC median | 83.212579 us | 66.395 us | 1.253287x |
| `comprehensions` three-round SOAC median | 83.21258 us | 66.6000 us | 1.249438x |
| Optimized typed-IR blocks / functions | 3,069 / 218 | 3,069 / 218 | unchanged |
| Pre-optimization serialized BlockPy bytes | 14,398,752 | pending | pending |
| Apply-mode native code bytes | 23,359,400 | 23,359,400 | unchanged |
| Apply-mode machine blocks | 1,549,290 | 1,549,290 | unchanged |
| Native profiling CPU-clock samples lost | 0 of 916 | 0 of 849 | both zero loss |
| Inclusive function/closure creation | 31.213%; overlapping | 20.971%; overlapping | directional native evidence |
| Original source-backed genexpr family | 17.573%; overlapping | 7.067%; overlapping | directional native evidence |
| Original free-variable metadata scan | 2.401%; overlapping | 0%; overlapping | directional native evidence |
| Ready direct-entry lookup | 2.401%; overlapping | 0.118%; overlapping | directional native evidence |
| Function/JIT registration | 7.532%; overlapping | 4.832%; overlapping | directional native evidence |
| Focused watcher integration RED / GREEN | 1 failed / 0.48 s; stock [0,0,0] versus SOAC [0,3,4,5] × 3 | 1 passed / 0.42 s; CREATE-only | genuine RED-to-GREEN |
| Focused source-backed structural RED / GREEN | 1 failed / 557 filtered in 0.03 s | 1 passed / 557 filtered | genuine RED-to-GREEN |
| Private ready-handle cache session/code/version key test | no cache | equal matching key; unequal session/code/version keys | structured key comparison only; runtime suite 31 / 31 |
| Complete post-cache JIT Rust library | 557 previous tests | 559 / 559 in 5.50 s | both new structured cases pass |
| Grouped post-cache transformed runtime regressions | existing guardrails | 31 / 31 in 15.81 s | watcher, mutation, and retained optimizations GREEN |
| Combined aligned Cargo test-target check | not applicable | passed in 5.04 s | GREEN |
| Package-scoped Rust formatting / check | not applicable | passed | GREEN |
| Release debug-single total native bytes / machine blocks | 2,314,724 / 153,612 | 2,314,724 / 153,612 | unchanged |
| Mode-matched comprehensions benchmark bodies | 21 functions | 21 functions; hot genexpr 24,452 / 1,692 | unchanged |
| Full `just test-all` correctness gate | prior baseline gate | 1,219 nodeids / 86 file-local batches; all Rust suites | 86 / 86 batches passed |

## Attempt history

### Attempt 1: establish source-backed immutable materialization boundaries

- Change: documentation plus a separately owned **323-line** stock/SOAC
  CPython `ctypes` watcher integration genuinely run against unchanged
  production; a second private structured zero-freevar/code-name-pointer
  regression also genuinely fails. Implementation in the **four explicitly
  approved production files** began only after both REDs. Existing private
  prefix/template/default-slot and canonical original-constructor/module
  changes now drive the structured JIT test **GREEN 1 / 1** and the real
  same-interpreter watcher integration **GREEN 1 / 0.42 seconds**. Initial
  debug-extension setup cost **22.74 seconds** and is not throughput data.
- Measurements and coverage: integrated scalar baseline and prior
  `comprehensions` zero-loss profile are recorded above. New-strategy
  watcher execution genuinely **fails 1 / 0.48 seconds**; the independent
  structured JIT regression genuinely **fails 1 / 557 filtered in 0.03
  seconds**. Its initial **22.98-second aligned build** is workflow
  overhead only. Subsequent normally sampled code/throughput and native-
  profile evidence are recorded above; the authoritative gate is running.
- Compatibility and tests: fresh objects/cells, exact defaults and watcher
  events, user mutation/factory replacement, existing synthetic-only
  fallback, original code identity, and guarded handle lifetime are explicit
  prerequisites. Same-interpreter stock watcher events are only CREATE
  **`[0, 0, 0]`**, whereas all three transformed function families repeat
  CREATE plus spurious MODIFY events **`[0, 3, 4, 5] × 3`**; transformed
  name/qualified-name identity also differs. All other fresh-cell,
  lazy-generator, default, mutation-event, and factory-guard checks pass.
  The independent structured test also confirms distinct function-name/code
  object pointers. Four-file implementation has started, while both
  the structured JIT regression now **passes 1 / 1**, independent host
  review found no issue, and the real stock-versus-SOAC watcher integration
  **passes 1 / 0.42 seconds** with all original/defaulted/captured families
  and mutation/factory guards intact **before cache insertion**. The
  optional approximately **35-line private** positive-only template
  `OnceLock` cache is now implemented; it reuses an immutable ready compiled
  handle only for the exact compile session/code pointer/version and current
  live `PyFunction.func_code`, rechecks force mode on each attachment, and
  clones into a fresh `FunctionEnv`. Its private structured regression
  **passes** same-key equality plus independent session/code/version
  inequality; it does not directly execute cache retrieval, insertion,
  force-mode, or live-code guards. Those conditions are source-reviewed and
  existing mutation behavior is validated by the **31 / 31** Python suite.
  The entire post-cache JIT library also **passes 559 / 559 in
  5.50 seconds**, including both new structured tests; its single aligned
  rebuild costs **22.19 seconds**. The grouped transformed-runtime watcher,
  original code/default/keyword/generator/method mutation, synthetic
  factory/reentry/module replacement, and prior late-scalar/fused/fixed-
  unpack/shutdown/indexed-counter regressions now **pass 31 / 31 in 15.81
  seconds**. Combined interpreter-aligned Cargo `--tests` **passes in 5.04
  seconds**, and scoped Rust formatting/checks pass. Release debug-single
  smoke completes **8 / 8** with unchanged typed/native code and identical
  PID-matched source-backed genexpr bodies; its apparent 2.44 ms
  comprehensions value is contaminated by **338.3 ms** of cold
  `exception_matches` JIT plus unamortized setup. Normal fixed-eight robust
  geometric improvement is **1.0426394914876491x**, and independent three-
  round affected-workload robust improvement is **1.095507476732x**;
  `comprehensions` improves approximately **1.25x** with unchanged generated
  code. The change adds no new public API. The authoritative full
  correctness gate **passes**.
- Authoritative full `just test-all` gate **PASSES**:
  `work/logs/source-function-templates-test-all.log` records **1,219 Python
  nodeids across 86 file-local batches, with 86 / 86 batches passing and
  zero failures**. Rust suites also pass: **soac_jit 559**,
  **soac_ir_typed 53**, **soac_opt 208**, **soac_lowering 371**, and
  **PyO3 8**. Cargo tests take **62.957 seconds**, pytest
  **94.462 seconds internally / 94.477 seconds externally**, and the full
  test phase **157.448 seconds**; the slow existing counter batch takes
  **94.08 seconds**. This verifies full correctness, not the unmet
  full-suite stock-performance goal.
- Result: **LANDED / RETAIN; genuine structured JIT and same-interpreter watcher
  RED-to-GREEN, full JIT 559 / 559, grouped Python 31 / 31, scoped checks,
  and repeated representative performance verified; full correctness gate
  **86 / 86 Python batches plus all Rust suites passed**.
- Reason: source-backed creation and free-variable metadata are measurable
  costs, but most inclusive closure-stack time is not necessarily removable;
  correctness-only synthetic metadata work provides a negative precedent.

## Verdict and next action

- Verdict: **LANDED / RETAIN; FULL CORRECTNESS GATE PASSED**; genuine
  unchanged-production CPython watcher and
  structured code-name-identity REDs both turn GREEN; all 559 JIT tests,
  31 grouped transformed-runtime regressions, and scoped Cargo/format
  checks pass. Normally sampled fixed-eight robust improvement is
  **1.0426394914876491x** and targeted three-round robust improvement is
  **1.095507476732x**; `comprehensions` median improves approximately
  **1.25x** across all 60 candidate values with unchanged native/typed code.
  Stock score remains **0.5132971537493283x**, well below the full-suite
  **1.10x** goal. The full correctness gate passes **1,219 Python nodeids /
  86 file-local batches**, plus **559 JIT / 53 typed-IR / 208 optimizer /
  371 lowering / 8 PyO3** Rust tests; see
  `work/logs/source-function-templates-test-all.log`.
- Transferable lesson: immutable original-code/template facts may be shared,
  but Python function objects, closure cells, defaults, watcher-visible
  mutations, and dynamically replaced factories cannot be silently reused.
- Next action: integrate the validated retained optimization and continue
  reporting the full-suite stock-performance target as unmet.
