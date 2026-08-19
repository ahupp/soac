---
title: "Interned Runtime Lookup Keys"
---

# Interned runtime lookup keys

- Status: **LANDED CANDIDATE / RETAIN; THREE-ROUND COMPREHENSIONS GAIN,
  MATCHED ZERO-LOSS PROFILE, AND FULL CORRECTNESS GATE VERIFIED; INITIAL
  FIXED-EIGHT ARITHMETIC REGRESSION DISCLOSED**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`mztqqkor`**, commit
  **`5124483c`**.
- Candidate revision: change **`wtyxsxpv`**, commit **`8a36c759`**; exactly
  two private production files are implemented, and both focused JIT
  regressions pass.
- Outcome: determine whether immutable per-template interned Unicode lookup
  keys can remove repeated temporary-string allocation in trusted live
  runtime-module lookups while retaining every mutable binding, reentrancy,
  exception, ownership, and module-hook behavior.

## Hypothesis and evidence

- General-purpose opportunity: source-backed function/generator
  instantiation repeatedly resolves trusted runtime imports, module
  bootstrap state, and factory bindings. The pinned CPython
  `PyDict_GetItemString` helper creates a temporary Unicode key for each
  C-string lookup. Reusing existing-function-template-owned interned Unicode
  keys with exception-equivalent `PyDict_GetItem` may remove repeated
  conversion without caching mutable modules or factories.
- Integrated normal fixed-eight stock geometric score is
  **0.5594598880789836x**. The authoritative full-suite **1.10x stock**
  goal remains unmet. Current generated Apply code totals **25,033,800
  native bytes / 1,652,600 machine blocks**, optimized typed coverage
  **3,069 blocks / 218 functions**, and serialized pre-optimization BlockPy
  **14,398,752 bytes**. The machine-block count is verified from
  `work/pyperformance/comparison-20260819-061808-DYXZjR/summary.json`.
- A current zero-loss `comprehensions` profile contains **782 records**.
  `PyDict_GetItemString` ancestry has **4.860% union coverage**: source
  function import lookup **3.453%**, canonical bootstrap lookup **1.151%**,
  and unrelated lookup **0.256%**. Only the first two trusted runtime sites
  are candidates. Union / inclusive attribution is not an additive or
  established workload speedup.
- An earlier `chaos` profile contains a potential **2.029%** target
  (**1.353% + 0.676%**), but it belongs to an earlier integrated revision
  and must be labeled as such. Other current sampled workloads have **zero
  observed target samples**; absence of samples is not proof the path never
  executes, and no broad workload gain is predicted.
- No existing user-visible mismatch or CPython correctness bug has been
  established. No candidate speedup, function-allocation reduction, native
  code effect, setup cost, or full-suite benefit is claimed.
- Mandatory CPython-observable safety hazard: a **general-key dictionary**
  may invoke a colliding custom key's `__eq__(lookup_key)`. That callback can
  distinguish a freshly allocated Unicode lookup key from a reused
  interned key by identity or side effects, so unconditionally replacing
  `PyDict_GetItemString` would change user-visible behavior even when text
  contents match. This is an implementation hazard, not an observed
  existing-production bug.
- Genuine unchanged-production structured JIT regression constructs an
  actual lowered `FunctionInstantiationTemplate`, performs real production
  runtime imports across successive `sys.modules` runtime-module
  replacements, and restores prior module state. The focused test
  **fails 1** at the exact assertion **`production runtime lookup must
  prepare reusable interned Unicode keys`**. Both live module-replacement
  assertions pass before the missing-cache failure, proving the RED is the
  absent reusable-key preparation rather than broken existing mutation
  behavior. Planned post-implementation checks include all three CPython
  interned-key object identities and weak references proving no strong
  runtime-module retention.
- Existing production dictionary lookup behavior remains unchanged before
  this RED. The implementation owner is now adding exact-dictionary
  `dk_kind` admission, original fresh-identity GENERAL-key fallback,
  adversarial equality / swallowed-exception controls, and unchanged fresh
  module attribute lookup. No structured GREEN, runtime compatibility
  result, candidate benchmark, or full correctness gate was claimed at that
  initial stage; later sections record their verified outcomes.
- The genuine real-template / production-runtime-lookup regression now
  turns **RED-to-GREEN**. It performs successive real `sys.modules`
  runtime-module replacements, verifies all **three exact CPython interned
  Unicode identities**, preserves live bootstrap/factory and module
  replacement behavior, and proves through weak references that cached keys
  do **not** strongly retain runtime modules.
- A second adversarial structured regression verifies both exact
  **GENERAL-key dictionaries and dictionary subclasses** retain the
  original `PyDict_GetItemString` path: a colliding custom `__eq__` observes
  the original **fresh lookup-key identity**. A raised `RuntimeError` is
  suppressed exactly as before, reported through the unraisable hook, and
  leaves no pending exception. Focused aligned JIT tests report
  **2 PASSED**.
- The completed fast path admits only exact **UNICODE/SPLIT** dictionaries,
  using existing PyO3 `ffi::PyDictObject` and a private **four-field**
  `#[repr(C)]` keys prefix. Fallible Unicode interning occurs outside the
  lock; module `dp.getattr("code_with_freevars")` remains unchanged. One
  trivial typed-pointer compile mismatch was corrected. Exactly two
  existing production files change, with no public API, runtime helper,
  global cache, or stronger session/subinterpreter guarantee. Complete JIT
  library and all Cargo test targets each pass **565 / 565**, and grouped
  transformed runtime tests pass **21 tests across 12 files in 17.48
  seconds**. Aligned JIT test-target checking and package-scoped formatting
  check also pass. Exactly two existing private production files are frozen;
  candidate benchmarks and full correctness gate subsequently complete.
- Release debug-single smoke **065734** passes **8 / 8 workloads with zero
  worker errors**. Every PID-matched emitted function row, native-byte
  count, and machine-block count is exactly identical to prior mode-matched
  hot-nonself smoke **061626**: aggregate **2,426,104 native bytes /
  160,598 machine blocks**, with unchanged **3,069 typed blocks / 218
  functions**.
- Individual smoke generated totals remain `chaos`
  **712,432 bytes / 47,119 blocks**, `comprehensions`
  **302,076 / 20,022**, `deltablue` **481,284 / 31,399**, and `richards`
  **367,664 / 24,658**; the other four workloads are likewise identical.
  Cold single-loop smoke values are **not valid throughput measurements**.
  Normal fixed-eight comparison **065917** against prior normal
  hot-nonself comparison **061808** subsequently completes with mixed
  results; retention and full correctness gate are established below.
- Normally sampled comparison **065917** completes **8 / 8 workloads**.
  Official paired-stock geometric score **declines** from
  **0.5594598880789836x to 0.5558386711560767x**; previous-SOAC arithmetic
  geometric ratio is **0.9899057912132601x**, an overall headline
  **regression** that must not be concealed. Mean prior/candidate ratios are
  `chaos` **0.9699889x**, `comprehensions` **1.0384746x**, `deltablue`
  **1.0250211x**, `fannkuch` **1.0083301x**, `float` **0.9757068x**,
  `nbody` **0.9926666x**, `richards` **0.9805468x**, and `spectral_norm`
  **0.9325275x**.
- Independent fixed-eight robust previous-SOAC geometric ratio is
  **1.003392x**, or **1.003027x** after paired-stock adjustment, despite the
  official **0.9899057912132601x** arithmetic regression. `comprehensions`
  robust median improves **63.2901 → 60.6805 us (1.043005x)**, or
  **1.055533x** stock-adjusted; its clustered **95% interval
  0.9983–1.0808x includes neutral**, so a statistically established gain
  cannot yet be claimed. Other robust ratios include `chaos`
  **0.990362x**, `deltablue` **1.017364x**, `richards` **0.990769x**, and
  `spectral_norm` **0.96547x**.
- All **80 measured normal worker PIDs** have exactly identical
  per-function emitted native bytes and machine blocks: aggregate
  **25,033,800 bytes / 1,652,600 blocks**, unchanged
  **3,069 typed blocks / 218 functions**, and zero errors. In particular,
  the apparent spectral slowdown has unchanged generated code; causality
  remains unproven. Candidate three-round comparison **070242** against
  prior hot-nonself comparison **062131** subsequently confirms the targeted
  comprehensions gain without changing generated code; matched zero-loss
  profiling subsequently confirms the lookup mechanism, and the full
  correctness gate passes.
- Targeted **60-versus-60** sample comparison **070242** shows robust
  `comprehensions` improvement **1.067068x**, clustered **95% interval
  1.035739–1.094314x**; paired-stock adjusted **1.049671x**, interval
  **1.012618–1.096761x**. Individual raw rounds are
  **1.046683x / 1.068997x / 1.066195x**, while stock-adjusted rounds are
  **1.021455x / 0.995131x / 1.133882x**; one adjusted round is neutral or
  slightly negative, and must not be concealed.
- Targeted `chaos` is **1.008220x**, interval **0.981002–1.039541x**,
  adjusted **1.005929x**, consistent with neutral. `deltablue` is
  **1.039965x**, interval **1.004617–1.084943x**, adjusted **1.042675x**;
  no source-backed causal mechanism for that movement is established.
  `richards` is **0.983662x**, interval **0.952731–1.027502x**, with
  stock-adjusted **0.957491x** and interval **0.917952–1.000002x**,
  explicitly borderline negative rather than a proven improvement.
- Robust targeted-subset geometric improvement is **1.024243x**, or
  **1.013272x** stock-adjusted; official arithmetic is **1.036707x**.
  All **120 measured candidate worker PIDs** retain identical per-function
  native/body shapes; targeted totals are **19,407,320 bytes / 1,278,600
  machine blocks**, with zero errors. These targeted results support
  **RETAIN CANDIDATE**, but do not erase the fixed-eight arithmetic
  **0.9899057912132601x regression**, stock decline
  **0.5594598880789836x → 0.5558386711560767x**, or only
  **1.003392x** robust full-eight improvement. A matched zero-loss
  comprehensions profile confirms the intended lookup mechanism; the full
  correctness gate also passes.
- Matched zero-loss `comprehensions` profiles compare the previous
  hot-nonself candidate to
  `work/logs/interned-runtime-keys-candidate-comprehensions_*` using the
  same **50,000 loops / 199 Hz**, with **782 → 738 recorded samples**.
  Exact `PyDict_GetItemString` ancestry falls **4.860160% → 0%**, including
  runtime import **3.453272% → 0%** and canonical bootstrap
  **1.151091% → 0%**. The previously sampled unrelated **0.255798%** site
  is absent from this candidate sample but its source is unchanged; do not
  claim it was optimized.
- Unicode allocation/decode ancestry within function instantiation falls
  **2.1743% → 0.4071%**, and descendant deallocation ancestry
  **2.6859% → 0.5429%**. These shares overlap and are not additive.
  Remaining `PyDict_GetItem` import lookup rises **0.1279% → 0.2714%** and
  bootstrap lookup **0.2558% → 0.5429%**. Cold first-call compilation
  increases **5.2439% → 5.5643%**, and GC increases
  **14.3217% → 16.3884%**. Profiler-attached replay
  **67.8875 → 62.9493 us (1.078446x)** is **diagnostic only**, not a
  benchmark headline. Matched 60-versus-60 medians and intervals remain
  authoritative.

## Implementation and compatibility

- Frozen bounded production scope: exactly
  `crates/soac_jit/src/lib.rs` and
  `crates/soac_jit/src/function_instantiation.rs`, reusing the existing
  immutable per-function instantiation template. These are the only two
  production files changed for this strategy.
- Each existing template may own exactly **three interned immutable Unicode
  lookup keys**. Store only the keys, not strong references to `sys.modules`,
  runtime modules, bootstrap objects, factories, or module dictionaries.
  Replace admitted `PyDict_GetItemString` calls only when the live object is
  an **exact dictionary** and the existing PyO3 `ffi::PyDictObject.ma_keys`
  plus pinned CPython keys
  metadata proves **`dk_kind != DICT_KEYS_GENERAL`**. Use the same
  exception-suppressing `PyDict_GetItem` semantics on that still-live exact
  Unicode-key dictionary and immutable key.
- For **`DICT_KEYS_GENERAL`**, non-exact dictionaries, ambiguous key kinds,
  or unavailable metadata, retain the **original `PyDict_GetItemString`
  call**. This preserves fresh Unicode lookup identity, collision-key
  `__eq__` callbacks, original reentrancy, and swallowed exceptions.
  Reference `vendor/cpython/Include/internal/pycore_dict.h` for the pinned
  layout. Reuse existing public PyO3 `ffi::PyDictObject` rather than
  duplicating the full dictionary object; add only a small private
  `#[repr(C)]` `RawPyDictKeysPrefix` containing the pinned `dk_kind`, inside
  existing `crates/soac_jit/src/function_instantiation.rs`. Add no
  public API or extra file.
- Optimize only the **three existing `PyDict_GetItemString` call sites**.
  Keep PyO3 `dp.getattr("code_with_freevars")` completely unchanged so
  custom module `__getattribute__` / `__getattr__` hooks continue to observe
  their original lookup-string identity, dispatch, and exceptions.
- Revalidate `sys.modules`, module identity/dictionary, bootstrap contents,
  and the factory binding on **every call**. Existing factory replacement,
  module deletion/replacement, mutable runtime globals, subclass/module
  hooks, interpreted fallback, local/global monitoring, and session guards
  must remain authoritative; cached keys must not imply cached values.
- Follow only the existing template-owned Python-object lifetime model;
  `CompileSession` itself is an existing process-global static `OnceLock`.
  This strategy does **not** establish cross-interpreter isolation,
  subinterpreter safety, or session-reset guarantees. Weak-module tests
  must prove no new strong runtime-module retention without overstating
  existing session behavior.
- Preserve Python reentry, recursion, `None`, missing keys,
  out-of-memory/error propagation or suppression, module import hooks,
  dictionary mutation, object ownership, and finalizer ordering. Failed
  initialization or unsafe lookup must retain the exact original generic
  behavior. Do not use temporary module attributes, thread-local state,
  public APIs, runtime helpers, or global mutable caches.
- Focused private real-template/production-lookup regression genuinely
  **fails 1**, then passes after key preparation, proving all three interned
  identities, successive live module replacement, and no strong module
  retention. The adversarial **colliding custom-key identity regression**
  also passes for GENERAL dictionaries and dict subclasses, preserving
  fresh lookup identity, unraisable reporting, and swallowed exceptions;
  focused JIT tests pass **2**. Transformed factory/module
  replacement, reentry, monitoring, source-backed watcher, `None`,
  allocation-failure, and shutdown guardrails; grouped transformed
  compatibility checks now pass **21 / 21 across 12 files in 17.48
  seconds**.

## Benchmark protocol and coverage

- Fixed benchmark selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`, compared
  against the same vendored stock CPython and integrated hot-nonself SOAC
  revision. Independently profile each revision and separate project
  coverage from transformed standard-library coverage.
- Baseline normal comparison:
  `work/pyperformance/comparison-20260819-061808-DYXZjR/summary.json`.
  Existing completion is **8 / 8**, optimized typed coverage
  **3,069 blocks / 218 functions**, native code
  **25,033,800 bytes / 1,652,600 machine blocks**, and pre-optimization
  BlockPy **14,398,752 bytes**.
- Current zero-loss comprehension attribution uses **782 samples**;
  separately labeled chaos evidence is from an earlier revision. Candidate
  implementation, debug smoke, normal/targeted repeated comparisons, exact
  generated-code effects, independent zero-loss profiles, and full gate are
  **pending**.

## Measurements

| Metric | Integrated hot-nonself baseline | Candidate | Change |
| --- | --- | --- | --- |
| Fixed-eight paired stock / SOAC geometric score | 0.5594598880789836x | 0.5558386711560767x | headline decline; full-suite stock 1.10x goal unmet |
| Previous-SOAC arithmetic / robust geometric ratio | integrated `mztqqkor/5124483c` | arithmetic 0.9899057912132601x; robust 1.003392x | stock-adjusted robust 1.003027x; mean regression preserved |
| Fixed-eight robust `comprehensions` median | 63.2901 us | 60.6805 us | 1.043005x; clustered 95% 0.9983–1.0808x includes neutral |
| Targeted three-round robust / stock-adjusted subset geometry | prior hot-nonself repeated comparison 062131 | 1.024243x / 1.013272x | official arithmetic 1.036707x; 60 versus 60 samples |
| Three-round `comprehensions` raw / stock-adjusted improvement | prior repeated hot-nonself baseline | 1.067068x / 1.049671x | 95% 1.035739–1.094314x / 1.012618–1.096761x |
| Three-round `richards` raw / stock-adjusted ratio | prior repeated hot-nonself baseline | 0.983662x / 0.957491x | adjusted 95% 0.917952–1.000002x borderline negative |
| Targeted measured-worker native bytes / blocks | 19,407,320 / 1,278,600 | 19,407,320 / 1,278,600 | all 120 measured worker function rows identical |
| Fixed-eight workload prior/candidate mean ratios | integrated hot-nonself baseline | chaos 0.9699889x; comprehensions 1.0384746x; delta 1.0250211x; fann 1.0083301x; float 0.9757068x; nbody 0.9926666x; richards 0.9805468x; spectral 0.9325275x | mixed means; causality and confidence intervals pending |
| Optimized typed-IR blocks / functions | 3,069 / 218 | 3,069 / 218 | unchanged |
| Serialized pre-optimization BlockPy bytes | 14,398,752 | pending | pending |
| Apply-mode native code bytes | 25,033,800 | 25,033,800 | all 80 worker function rows identical |
| Apply-mode machine blocks | 1,652,600 | 1,652,600 | unchanged |
| Mode-matched fixed-eight debug-single native bytes / blocks | 2,426,104 / 160,598 | 2,426,104 / 160,598 | every emitted row identical across 8 workloads |
| Release debug-single completion / typed coverage | prior smoke 8 / 8; 3,069 blocks / 218 functions | 8 / 8; zero worker errors; 3,069 / 218 | unchanged; cold timings invalid |
| Matched comprehensions zero-loss samples / replay setup | 782 | 738; 50,000 loops / 199 Hz | zero-loss; attached replay diagnostic only |
| `PyDict_GetItemString` union / target ancestry | union 4.860160%; import 3.453272%; bootstrap 1.151091% | 0% at sampled target frames | unrelated previous 0.255798% frame unchanged in source |
| Function-instantiation Unicode allocation / deallocation ancestry | 2.1743% / 2.6859% | 0.4071% / 0.5429% | overlapping; not additive |
| Remaining dict lookup / compile / GC ancestry | import 0.1279%; bootstrap 0.2558%; compile 5.2439%; GC 14.3217% | import 0.2714%; bootstrap 0.5429%; compile 5.5643%; GC 16.3884% | diagnostic tradeoffs; profile not benchmark headline |
| Earlier-revision chaos target ancestry | 2.029% = 1.353% + 0.676% | pending | earlier baseline, not current candidate evidence |
| Genuine private structured template/key regression | 1 failed; real lowered template/import missing reusable interned Unicode keys | passes three exact intern identities, successive module swaps, weak-module release | genuine RED-to-GREEN |
| Adversarial GENERAL-key collision identity regression | custom `__eq__(lookup_key)` distinguishes fresh versus interned Unicode | passes GENERAL dict + dict subclass; RuntimeError swallowed/unraisable reported | focused aligned JIT 2 PASSED |
| Complete JIT Rust library / all Cargo test targets | integrated prior JIT baseline | 565 / 565 passed for each | GREEN |
| Factory/module/reentry/monitoring transformed guardrails | existing integrated behavior | 21 passed across 12 files in 17.48 s | GREEN |
| Aligned JIT test-target Cargo check / scoped format check | existing baseline | both passed | GREEN |
| Full `just test-all` correctness gate | integrated baseline passed | 1,222 nodeids; 89 / 89 isolated file batches; 8 workers | GREEN; zero failures |

The authoritative full-gate log is
`work/logs/interned-runtime-keys-test-all.log`. `just test-all` passes
**1,222 Python nodeids across 89 / 89 file-local batches and eight
workers**, with **zero failed batches**. Workspace Rust suites pass JIT
**565**, optimizer **211**, typed IR **54**, lowering **371**, and PyO3
**8**. Cargo tests take **58.487 seconds**, inner / outer pytest
**95.296 / 95.311 seconds**, and the complete test phase
**153.809 seconds**; the known counter-dump batch takes **94.53 seconds**.

## Attempt history

### Attempt 1: attribute repeated trusted runtime lookup strings

- Change: inspect current zero-loss comprehension call stacks, identify
  pinned CPython temporary Unicode allocation within `PyDict_GetItemString`,
  compare an earlier separately labeled chaos profile, and capture a genuine
  unchanged-production structured JIT RED using real lowered template,
  production import, and successive runtime-module replacement.
- Measurements and coverage: current comprehensions **782 samples**,
  lookup union **4.860%**, target function-import **3.453%**, canonical
  bootstrap **1.151%**, unrelated **0.256%**. Earlier-revision chaos target
  is **2.029%**. Existing fixed-eight stock score is
  **0.5594598880789836x** with **25,033,800 bytes / 1,652,600 blocks**.
- Compatibility and tests: immutable interned keys must not retain mutable
  modules or factories; current dictionary lookup, exception suppression,
  import/module hooks, reentry, OOM, finalizers, and source watcher/monitor
  behavior must remain equivalent. General-key dictionary collision
  callbacks can observe fresh-versus-interned lookup identity, so only exact
  non-GENERAL dictionaries may take the new path; GENERAL dictionaries must
  retain original `PyDict_GetItemString`. The genuine structured regression
  **fails 1** exactly at missing production reusable-key preparation while
  both runtime-module replacement assertions pass. Adversarial collision
  checks initially remain pending, then both focused aligned JIT tests pass:
  real production import / three interned identities / live module swaps /
  weak-module release, plus GENERAL/subclass collision fresh-key identity /
  unraisable error suppression. Fast path admits exact UNICODE/SPLIT keys
  only; fallible interning occurs outside the lock, and module `getattr`
  stays unchanged. One typed-pointer compile mismatch is corrected. Full
  JIT library and all Cargo test targets each pass **565 / 565**; grouped
  transformed runtime checks pass **21 / 21 across 12 files in 17.48
  seconds**; aligned JIT test-target checking and scoped formatting check
  pass. Exactly two private production files are frozen. Release fixed-eight
  debug-single smoke **065734** passes **8 / 8**, with every emitted
  function row and **2,426,104 native bytes / 160,598 blocks** identical to
  prior smoke; cold timings are invalid. Normal fixed-eight comparison
  **065917** then completes **8 / 8** with lower stock score
  **0.5558386711560767x** versus **0.5594598880789836x**, arithmetic prior
  ratio **0.9899057912132601x**, and mixed workload means. Independent
  robust prior geometry is **1.003392x / 1.003027x stock-adjusted**;
  comprehensions improves **1.043005x** but its clustered interval includes
  neutral. All **80 measured workers** have identical generated code and no
  errors. Three-round comparison **070242** confirms comprehensions
  **1.067068x**, interval **1.035739–1.094314x**, targeted robust subset
  **1.024243x**, and unchanged native code; adjusted richards remains
  borderline negative and fixed-eight arithmetic remains regressed.
  Matched zero-loss profiles **782 → 738 samples** confirm target
  `PyDict_GetItemString` ancestry **4.860160% → 0%** and reduced Unicode
  allocation/deallocation; remaining lookup, compile, and GC shares plus
  unchanged unrelated-site source are explicitly disclosed. Candidate is
  retained, and the authoritative full correctness gate passes **1,222
  Python nodeids / 89 file-local batches** plus all workspace Rust suites.
- Result: **IN PROGRESS; genuine structured production lookup RED-to-GREEN
  and adversarial GENERAL-key/subclass regression GREEN; focused JIT
  2 PASSED, exactly two private production files; broad validation,
  JIT Rust 565 / 565, transformed Python 21 / 21, and scoped checks GREEN;
  release smoke 8 / 8 with identical generated code; normal benchmark
  fixed-eight arithmetic regression 0.9899057912132601x versus robust
  1.003392x and unchanged native code; targeted comprehensions 1.067068x;
  matched zero-loss mechanism profile complete, full correctness gate
  PASSED; LANDED CANDIDATE / RETAIN**.
- Reason: immutable key reuse is potentially safe, while caching mutable
  runtime values or changing exception-suppression semantics is not.

## Verdict and next action

- Verdict: **LANDED CANDIDATE / RETAIN; MATCHED ZERO-LOSS PROFILE VERIFIED
  AND FULL CORRECTNESS GATE PASSED**. Genuine unchanged-production
  structured lookup
  turns RED-to-GREEN, and adversarial GENERAL-dictionary/subclass identity
  and exception compatibility pass, for **2 focused JIT tests**. Complete
  JIT library and all test targets each pass **565 / 565**, transformed
  checks pass **21 / 21 across 12 files in 17.48 seconds**, and aligned
  Cargo / scoped format checks pass. Release smoke passes **8 / 8** with
  exactly unchanged native code and typed coverage. Normal fixed-eight
  comparison **065917** completes **8 / 8**, but stock score drops to
  **0.5558386711560767x** and arithmetic previous-SOAC ratio is
  **0.9899057912132601x**. Robust fixed-eight previous ratio is
  **1.003392x**, and comprehensions **1.043005x** has interval
  **0.9983–1.0808x** including neutral. All **80 measured workers** retain
  identical generated code. Three-round comparison **070242** confirms
  comprehensions **1.067068x**, adjusted **1.049671x**, and robust subset
  **1.024243x**; richards adjusted **0.957491x** remains borderline
  negative. Retain while preserving the official fixed-eight arithmetic
  regression. Matched zero-loss **782 → 738-sample** profiles eliminate
  sampled target string lookup while preserving generic fallback; allocation
  and deallocation shares overlap, and cold compiler/GC remain present.
  The full correctness gate passes all **1,222 nodeids / 89 isolated
  batches** and workspace Rust suites. No public API change exists.
- Transferable lesson: immutable interned lookup keys may be shared, but
  runtime module/factory bindings and import behavior must be freshly
  observed on every call. General-key collision equality can observe lookup
  object identity, so exact pinned dictionary-key kind must be guarded and
  the original fresh-key fallback preserved. Profile union share bounds
  opportunity; it is not measured speedup.
- Next action: integrate the fully validated retained change while
  preserving the fixed-eight arithmetic regression, adjusted richards
  borderline result, and unmet stock **1.10x** goal in the final record.
