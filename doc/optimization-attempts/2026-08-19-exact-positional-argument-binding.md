---
title: "Exact Positional Argument Binding"
---

# Exact positional argument binding

- Status: **LANDED / RETAIN; NORMAL FIXED-EIGHT, TARGETED THREE-ROUND,
  MATCHED ZERO-LOSS PROFILES, IDENTICAL NATIVE CODE, AND FULL CORRECTNESS
  GATE ALL VERIFIED**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`nvvlrumm`**, commit
  **`7684c2fa`**.
- Candidate change: **`nnyqlvvy`**; one production file is implemented and
  both focused real lowering/FFI regressions pass.
- Outcome: evaluate whether reusing the existing precomputed direct-argument
  binding plan can bypass generic bookkeeping for fully supplied, exact
  positional calls while preserving every CPython ownership, error, and
  function-mutation guarantee.

## Hypothesis and evidence

- General-purpose opportunity: hot Python methods and direct calls commonly
  supply exactly the positional arguments expected by an already-compiled
  target. The current binder still enters general-purpose binding logic,
  including zeroing/bookkeeping needed for defaults and keyword paths.
  Existing `DirectArgBindingPlan` metadata should be sufficient to admit a
  narrow exact-arity positional fast path without duplicating semantic
  decisions or adding new global mutable state.
- Integrated normal fixed-eight comparison **050635** has stock geometric
  score **0.520917130452074x**. The authoritative full-suite **1.10x stock**
  goal remains unmet. Current generated Apply code is **24,353,560 native
  bytes / 1,608,670 machine blocks**, optimized typed coverage **3,069
  blocks / 218 functions**, and serialized pre-optimization BlockPy
  **14,398,752 bytes**.
- Latest post-inherited matched zero-loss profiles contain **390
  `deltablue` samples** and **522 `richards` samples**. Argument binding
  occupies **7.181% inclusive / 6.925% self** for `deltablue` and
  **8.424% inclusive / 7.466% self** for `richards`; approximately
  **0.958%** of richards samples are nested `memset`. Inclusive shares
  overlap and are not additive predicted speedup.
- An earlier `chaos` capture reports binder **3.046% inclusive / 2.708%
  self**. Its workload code is unchanged across the relevant integrated
  direct-generator/inherited-owner revisions, but this capture must not be
  mislabeled as a fresh post-inherited candidate profile.
- Source-level census finds **66 / 66** `deltablue` and **39 / 39**
  `richards` benchmark functions positional; observed call syntax has
  **176 / 176** delta and **84 / 85** richards calls without keywords.
  These source counts do not establish dynamic candidate eligibility,
  per-call hit rates, or a measured whole-workload benefit.
- A retained precedent in `doc/PERF_LOG.md`, **2026-04-13 — Optimize
  vectorcall argument binding**, records existing precomputed
  `DirectArgBindingPlan` support and **+12.54% pystone** at that time. That
  historical pystone result is evidence for reusing an existing concept,
  not a prediction or baseline for the current eight-workload comparison.
- No user-visible current correctness bug has been established. This is a
  narrowly guarded performance hypothesis only; candidate speedup,
  generated-code effects, startup cost, and pyperformance acceptance remain
  **pending**.
- Genuine unchanged-binding structured JIT regression
  `exact_positional_binding_selects_only_fully_supplied_ordered_parameters`
  reports **0 passed / 1 FAILED / 561 filtered**. Its precise failure is the
  existing selector returning false for
  `plan_for("zero").binds_exact_positional(0, no_keywords)`; no production
  binding behavior had changed before this RED. The real lowering fixture
  covers nine callable shapes: zero-arity, ordinary, positional-only,
  fully supplied defaults, closure, generator, keyword-only, `*args`, and
  `**kwargs`. Controls include the vectorcall offset bit, non-null keywords,
  omitted arguments, and excess arguments. The one-file implementation
  starts only after this genuine failure.
- The exact structured decision regression now turns **RED-to-GREEN**.
  Lowered zero-arity, ordinary, positional-only, fully supplied default,
  closure, and generator functions are accepted, including the vectorcall
  offset bit; missing/excess arguments, non-null keywords, keyword-only
  parameters, `*args`, and `**kwargs` are rejected. The original generic
  binder remains authoritative for every rejected shape.
- A second actual pinned-CPython FFI regression,
  `exact_positional_binding_preserves_owned_references_and_cleans_only_written_prefix`,
  constructs a real lowered module, `FunctionEnv`, and
  `PyFunctionJitExtra`. It proves zero-argument calls accept null buffers
  plus the offset bit; three actual list arguments receive matching
  `INCREF` / `DECREF`; malformed **`[object, NULL, object]`** cleans only
  the acquired prefix, restores object reference counts, and leaves the
  uninitialized sentinel tail untouched; and null output takes precedence
  over null argument buffer with exact existing errors. Both focused Rust
  regressions pass **2 / 561 filtered**.
- The single approved `crates/soac_jit/src/lib.rs` implementation is saved,
  independent source review reports no issue, and no public API was added.
  Post-format complete `cargo test -p soac_jit --lib` passes
  **563 / 563** tests,
  including both new structured selector and actual pinned-CPython FFI
  ownership cases. Complete JIT Cargo test targets also pass
  **563 / 563**, aligned `cargo check -p soac_jit --tests` passes, and
  package-scoped formatting passes. A subsequent test build reused artifacts
  in **0.34 seconds**; this is workflow evidence, not performance data.
  The package-scoped formatting **and formatting check** pass. A grouped
  suite covering **16 transformed Python regression files / families** now
  passes **95 tests in 30.42 seconds**, with **2 existing expected xfails**
  and **7 deselected**; this includes defaults, function mutation, watchers,
  direct generators, inherited owners, scalar regions, closures, and
  keywords. Production is frozen to exactly
  `crates/soac_jit/src/lib.rs`; subsequent candidate performance and the
  authoritative full correctness gate both pass.
- Release debug-single smoke
  `work/pyperformance/comparison-20260819-053742-4ppthM` completes
  **8 / 8 workloads with zero worker errors**. Exact PID-matched comparison
  against mode-matched inherited-owner smoke **050518** finds every emitted
  function row, native byte count, and machine-block count identical across
  all eight workloads: aggregate **2,377,824 native bytes / 157,417 machine
  blocks**, with unchanged **3,069 typed blocks / 218 functions**.
  `deltablue` remains **459,688 bytes / 30,033 blocks / 156 entries**, and
  `richards` remains **358,240 bytes / 24,070 blocks / 105 entries**. This
  confirms a runtime-only binder change with no emitted-code growth; cold
  one-loop smoke timings are **not throughput evidence**.
- Normally sampled fixed-eight comparison
  `work/pyperformance/comparison-20260819-053859-89QDJ8` now completes
  **8 / 8 workloads**. Paired stock geometric score improves
  **0.520917130452074x → 0.5482172650503208x**, and previous-SOAC
  arithmetic geometric improvement is **1.05714472883199x**. Individual
  previous-SOAC mean ratios are `chaos` **1.06042506x**,
  `comprehensions` **1.07927796x**, `deltablue` **1.03720175x**,
  `fannkuch` **1.05823783x**, `float` **1.07543302x**, `nbody`
  **1.01666740x**, `richards` **1.13154904x**, and `spectral_norm`
  **1.00364501x**.
- Actual normally measured Apply native code is exactly unchanged at
  **24,353,560 bytes / 1,608,670 machine blocks across all 80 measured
  workers**, with zero errors; optimized typed coverage remains
  **3,069 blocks / 218 functions**. Independent full-eight robust previous
  SOAC geometric improvement is **1.059214x**, or **1.052378x** after
  paired-stock adjustment.
- Robust previous-SOAC median ratios are `richards` **1.124859x**,
  clustered **95% interval 1.07988–1.16805x**; `deltablue` **1.080547x**,
  interval **0.99915–1.15093x**, which includes neutral; `chaos`
  **1.072778x**, interval **1.00150–1.13923x**; and `comprehensions`
  **1.067907x**, interval **1.02840–1.10559x**. Targeted three-round
  comparison **054212**, against prior three-round inherited comparison
  **051003**, subsequently completes and supports retaining the candidate.
- Targeted **60-versus-60 sample**, three-round comparison **054212** shows
  previous-SOAC robust four-workload geometric improvement
  **1.05567184x**, or **1.02805729x** after adjusting for paired-stock drift
  **0.9738417x**. Official arithmetic previous-SOAC improvement is
  **1.0526481874321825x**; paired-stock subset score is
  **0.36518696809819273x**. Robust `deltablue` median improves
  **3.750207 → 3.529319 ms (1.06258668x)**, worker-bootstrap **95%
  interval 1.01126–1.08508x**, or **1.037105x** stock-adjusted.
- Robust `richards` median improves
  **33.958922 → 31.815431 ms (1.06737267x)**, interval
  **1.01524–1.09177x**, or **1.045383x** stock-adjusted. Robust
  `comprehensions` median improves **63.84438 → 60.14894 us
  (1.06143821x)**, interval **1.03246–1.07916x**, or **1.037920x**
  stock-adjusted. `chaos` improves **57.67426 → 55.903815 ms
  (1.03166949x)** unadjusted, but its interval **0.99896–1.05705x**
  includes one and stock-adjusted ratio **0.992675x** is neutral.
- All **120 candidate measured-worker PIDs** preserve exact emitted native
  code and report zero errors. These repeated results support **RETAIN
  CANDIDATE**, not full-suite acceptance. Matched delta/richards zero-loss
  native profiles confirm the intended binder reduction; the authoritative
  full correctness gate also passes, and the stock **1.10x** goal
  remains unmet.
- Matched zero-loss `deltablue` profiles across **400 replay loops** contain
  **390 baseline → 365 candidate samples**. Binder inclusive / self ancestry
  falls **7.181% / 6.925% → 2.740% / 2.740%**, the containing binder-call
  ancestry falls **9.489% → 4.658%**, and binder-associated `memset` falls
  **0.513% → 0%**. Cold Cranelift/compiler ancestry remains approximately
  **13.33% → 13.4%**; profile replay is diagnostic rather than a steady
  benchmark.
- Matched zero-loss `richards` profiles across **70 replay loops** contain
  **522 baseline → 526 candidate samples**. Binder inclusive / self ancestry
  falls **8.424% / 7.466% → 3.231% / 3.231%**, containing binder-call
  ancestry falls **10.150% → 5.892%**, and binder-associated `memset` falls
  **0.958% → 0%**. Cold compiler ancestry remains **8.822% → 9.314%**.
  Inclusive stack ancestry overlaps; do not add nested shares or use
  attached replay as the headline. Independent repeated benchmark medians
  remain the authority.

## Implementation and compatibility

- Proposed scope: modify exactly one production file,
  `crates/soac_jit/src/lib.rs`, reusing its existing
  `DirectArgBindingPlan`. Admit a call only when the target supports the
  existing direct entry, its exact positional arity is fully supplied, and
  no keyword/default/keyword-only/variadic handling is required. Keep all
  existing generic binding and fallback behavior unchanged.
- Preserve CPython ownership: successful binding must apply exactly the
  existing `INCREF` / `DECREF` sequence to positional arguments. For a
  malformed vectorcall buffer containing a null after a valid prefix,
  decrement **only the successfully acquired prefix**; never inspect or
  decrement uninitialized trailing slots. Zero-arity calls must tolerate
  their existing null-argument-buffer representation.
- Preserve vectorcall ABI/error behavior: normalize the existing vectorcall
  offset bit, retain argument-buffer null checks, exact argument-count and
  error ordering, and original exception messages. Calls with keywords,
  missing/extra arguments, default insertion, keyword-only arguments,
  `*args`, or `**kwargs` must use the unchanged generic path.
- Preserve mutable target semantics: retain existing current-code/default
  refresh and guard behavior for `__code__`, `__defaults__`,
  `__kwdefaults__`, wrappers, interpreted entry, and compiled-handle
  replacement. No cached function assumption may remain valid after an
  existing guard would reject it. The existing method/self binding and
  reference-lifetime rules remain authoritative.
- Focused structured exact-positional binder evidence genuinely changes
  from **0 passed / 1 failed / 561 filtered** to **GREEN**. The independent
  real pinned-CPython FFI refcount/malformed-prefix regression also passes;
  focused Rust totals **2 passed / 561 filtered**. Existing transformed
  code/default/keyword/entry mutation integration is included in the
  passing **95-test** transformed guardrail suite.
- No new public API, runtime helper, optimization-plan type, enum variant,
  profile schema, or specialization concept is proposed. Pinned CPython
  ABI/layout details may be used where explicitly justified.

## Benchmark protocol and coverage

- Fixed benchmark selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`, compared
  against the same vendored stock CPython and integrated inherited-owner
  SOAC revision. Independently profile each revision and preserve existing
  source-backed functions, direct generators, inherited owners, scalar
  regions, fused floats, fixed unpacking, watcher, and shutdown guardrails.
- Baseline artifact:
  `work/pyperformance/comparison-20260819-050635-0mVSmo/summary.json`.
  Candidate artifact, targeted comparison rounds, robust previous-SOAC and
  paired-stock medians, confidence intervals, and actual binder fast-path
  coverage are **pending**.
- Existing fixed-eight completion is **8 / 8**, typed coverage **3,069
  blocks / 218 functions**, native code **24,353,560 bytes / 1,608,670
  machine blocks**, and pre-optimization BlockPy **14,398,752 bytes**.
  Candidate generated-code equality or change has not been measured.
- Matched zero-loss baseline/candidate native profiles provide delta
  **390 → 365** and richards **522 → 526** samples, with sharply reduced
  binder ancestry. The earlier chaos binder evidence has unchanged generated
  workload code but remains separately labeled.

## Measurements

| Metric | Integrated inherited-owner baseline | Candidate | Change |
| --- | --- | --- | --- |
| Fixed-eight paired stock / SOAC geometric score | 0.520917130452074x | 0.5482172650503208x | fixed eight improves; full-suite stock 1.10x goal unmet |
| Previous-SOAC arithmetic / robust improvement | integrated `nvvlrumm/7684c2fa` | arithmetic 1.05714472883199x; robust 1.059214x | paired-stock-adjusted robust 1.052378x |
| Robust `richards` / `deltablue` previous medians | integrated inherited-owner baseline | 1.124859x / 1.080547x | 95% rich 1.07988–1.16805x; delta 0.99915–1.15093x |
| Robust `chaos` / `comprehensions` previous medians | integrated inherited-owner baseline | 1.072778x / 1.067907x | 95% chaos 1.00150–1.13923x; comprehensions 1.02840–1.10559x |
| Targeted three-round robust / stock-adjusted geometric improvement | prior inherited three-round comparison 051003 | 1.05567184x / 1.02805729x | 60 versus 60 samples; stock drift 0.9738417x |
| Targeted three-round `deltablue` / `richards` medians | 3.750207 / 33.958922 ms | 3.529319 / 31.815431 ms | 1.06258668x / 1.06737267x; both worker intervals exclude one |
| Targeted three-round `comprehensions` / `chaos` medians | 63.84438 us / 57.67426 ms | 60.14894 us / 55.903815 ms | 1.06143821x / 1.03166949x; chaos stock-adjusted 0.992675x neutral |
| Fixed-eight previous-SOAC workload means | integrated inherited-owner baseline | chaos 1.06043x; comprehensions 1.07928x; delta 1.03720x; fann 1.05824x; float 1.07543x; nbody 1.01667x; richards 1.13155x; spectral 1.00365x | mean only; robust independent review pending |
| Optimized typed-IR blocks / functions | 3,069 / 218 | 3,069 / 218 | unchanged |
| Serialized pre-optimization BlockPy bytes | 14,398,752 | pending | pending |
| Apply-mode native code bytes | 24,353,560 | 24,353,560 | unchanged |
| Apply-mode machine blocks | 1,608,670 | 1,608,670 | unchanged |
| Mode-matched debug-single generated native bytes / machine blocks | 2,377,824 / 157,417 | 2,377,824 / 157,417 | identical function rows across all 8 workloads |
| Release debug-single completion / typed coverage | inherited smoke 8 / 8; 3,069 blocks / 218 functions | 8 / 8; zero worker errors; 3,069 / 218 | unchanged; cold timings invalid |
| Matched `deltablue` zero-loss native samples / loops | 390 | 365 / 400 loops | binder-inclusive 7.181% → 2.740% |
| Matched `richards` zero-loss native samples / loops | 522 | 526 / 70 loops | binder-inclusive 8.424% → 3.231% |
| Binder inclusive / self stack share | delta 7.181% / 6.925%; richards 8.424% / 7.466% | delta 2.740% / 2.740%; richards 3.231% / 3.231% | inclusive overlapping attribution; not a speedup |
| Outer binder-call ancestry / binder-associated memset | delta 9.489% / 0.513%; richards 10.150% / 0.958% | delta 4.658% / 0%; richards 5.892% / 0% | nested shares overlap; replay diagnostic only |
| Earlier unchanged-code `chaos` binder share | 3.046% inclusive / 2.708% self | pending | earlier capture, not a new candidate profile |
| Positional function / keyword-free source census | delta 66 / 66 functions, 176 / 176 calls; richards 39 / 39, 84 / 85 | pending | static source only |
| Genuine structured binder regression | 0 passed / 1 failed / 561 filtered; zero-arity selector incorrectly false | passes; nine lowered callable shapes and offset/negative controls | genuine RED-to-GREEN |
| Actual pinned-CPython FFI refcount / malformed-prefix regression | existing generic ownership behavior | passes real FunctionEnv/PyFunctionJitExtra; exact list references and NULL-prefix cleanup | GREEN; focused Rust 2 passed / 561 filtered |
| Complete JIT Rust library / package formatting | integrated inherited-owner JIT baseline | 563 / 563 passed; package formatting passes | GREEN |
| Post-format complete JIT Cargo test targets / aligned check | integrated inherited-owner baseline | 563 / 563 passed; `cargo check -p soac_jit --tests` passes | GREEN |
| Grouped transformed Python semantic families | existing guardrails | 95 passed; 2 expected xfails; 7 deselected; 16 files; 30.42 s | GREEN |
| Transformed default / code / keyword mutation guardrails | existing baseline | covered by 95 passing transformed tests | GREEN |
| Full `just test-all` correctness gate | integrated baseline previously passed | 1,221 nodeids; 88 / 88 isolated batches; 8 workers | GREEN; zero failed |

The final authoritative correctness log is
`work/logs/exact-positional-binder-test-all.log`. `just test-all` passes
**1,221 Python nodeids across 88 / 88 isolated file batches and eight
workers**, with **zero failures**. Workspace Rust suites pass JIT **563**,
typed IR **54**, optimizer **210**, lowering **371**, and PyO3 **8**. The
Cargo test phase takes **58.807 seconds**, inner / outer pytest
**92.972 / 92.986 seconds**, and the complete test phase **151.807
seconds**; the known counter-dump batch accounts for **92.18 seconds**.

## Attempt history

### Attempt 1: quantify existing exact-positional binder overhead

- Change: compare current post-inherited zero-loss delta/richards binder
  profiles with existing direct-binding-plan source and historical retained
  binding optimization, then capture a genuine unchanged-production
  structured RED. One-file implementation begins only after the RED.
- Measurements and coverage: delta binder **7.181% inclusive / 6.925%
  self** across **390 samples**; richards **8.424% / 7.466%** across
  **522 samples**; earlier unchanged-code chaos **3.046% / 2.708%**.
  Existing fixed-eight stock score is **0.520917130452074x** and native
  code totals **24,353,560 bytes / 1,608,670 blocks**.
- Compatibility and tests: structured
  `exact_positional_binding_selects_only_fully_supplied_ordered_parameters`
  genuinely **fails 1 / 561 filtered**, precisely because a valid
  zero-arity/no-keyword selector is false. The unchanged regression now
  turns GREEN for nine lowered callable shapes plus offset, keyword,
  omitted, and excess controls. The actual pinned-CPython FFI ownership
  regression also passes zero-arity/null, three-list owned references,
  malformed **`[object, NULL, object]`** prefix-only cleanup, untouched
  sentinel tail, output-null precedence, and exact errors; focused tests
  pass **2 / 561 filtered**. Complete JIT Rust library tests then pass
  **563 / 563** post-format, complete JIT test targets also pass
  **563 / 563**, aligned Cargo test-target checking passes, and
  package-scoped formatting passes. A later test build reuses artifacts in
  **0.34 seconds**. Scoped formatting and its format check both pass. The
  grouped **16-file/family** transformed Python suite passes **95 tests in
  30.42 seconds**, with **2 existing expected xfails / 7 deselected**.
  The single production file is frozen. Release debug-single smoke **053742**
  passes **8 / 8** with zero worker errors and exactly unchanged
  **2,377,824 native bytes / 157,417 machine blocks** versus mode-matched
  inherited smoke; every emitted function row is identical. Cold one-loop
  timings are not throughput evidence. Normal fixed-eight comparison
  **053859** then completes **8 / 8**, with paired stock score
  **0.520917130452074x → 0.5482172650503208x**, arithmetic previous-SOAC
  improvement **1.05714472883199x**, robust geometric improvement
  **1.059214x** / stock-adjusted **1.052378x**, improved means for all
  eight workloads, and exactly unchanged native code / typed coverage.
  Robust richards, chaos, and comprehensions intervals exclude one; delta
  **0.99915–1.15093x** includes one. Targeted three-round comparison
  **054212** against prior repeated **051003** subsequently confirms robust
  subset **1.05567184x** / stock-adjusted **1.02805729x**, with delta
  **1.06258668x**, richards **1.06737267x**, comprehensions
  **1.06143821x**, and chaos neutral after stock adjustment. All **120
  candidate measured-worker PIDs** retain identical generated code and zero
  errors. Matched zero-loss profiles confirm delta binder ancestry
  **7.181% → 2.740%** and richards **8.424% → 3.231%**, with nested
  `memset` eliminated and cold compiler ancestry still present. Retention is
  supported; the authoritative full correctness gate passes **1,221 Python
  nodeids / 88 isolated batches** plus every workspace Rust suite.
- Result: **IN PROGRESS; genuine structured decision RED-to-GREEN, real
  CPython FFI ownership GREEN, complete JIT Rust library 563 / 563 GREEN;
  transformed Python 95 passed / 2 expected xfails; release smoke 8 / 8
  with unchanged native code; normal robust previous improvement 1.059214x
  and targeted three-round robust 1.05567184x; matched zero-loss binder
  profiles confirm mechanism; full correctness gate PASSED, LANDED /
  RETAIN**.
- Reason: reuse the existing validated binding-plan concept; exact
  positional calls may avoid generic initialization, but error/refcount
  ownership must remain identical.

## Verdict and next action

- Verdict: **LANDED / RETAIN; MATCHED ZERO-LOSS PROFILES VERIFIED AND FULL
  CORRECTNESS GATE PASSED**. Genuine structured selector RED-to-GREEN and real
  pinned-CPython FFI ownership regression pass, with exactly one production
  file and no public API. Post-format complete JIT Rust library and all test
  targets each pass **563 / 563**, aligned JIT test-target Cargo check
  passes, and scoped formatting plus formatting check pass. Grouped
  **16-file** transformed suites pass **95 tests / 2 expected xfails / 7
  deselected in 30.42 seconds**. Production is frozen to one existing file;
  mode-matched release smoke passes **8 / 8** with exactly unchanged
  generated native code. Normal fixed-eight comparison completes with
  stock score **0.5482172650503208x**, arithmetic previous-SOAC improvement
  **1.05714472883199x**, robust previous improvement **1.059214x** /
  stock-adjusted **1.052378x**, and identical generated code across all
  workloads. Robust richards/chaos/comprehensions intervals exclude one;
  delta's interval includes neutral. Targeted three-round repeat **054212**
  then confirms robust delta **1.06258668x**, richards **1.06737267x**,
  comprehensions **1.06143821x**, neutral stock-adjusted chaos, and robust
  subset **1.05567184x** / stock-adjusted **1.02805729x**. Retain the
  candidate with unchanged generated code. Matched zero-loss delta
  **390 → 365** and richards **522 → 526** profiles confirm binder ancestry
  and zero-fill reductions; cold compiler shares and overlapping stacks are
  explicitly caveated. The authoritative full correctness gate passes all
  **1,221 Python nodeids / 88 batches** and Rust suites; stock **1.10x**
  remains unmet.
- Transferable lesson: binder self time and source-level positional counts
  indicate an optimization opportunity but do not prove eligible dynamic
  calls or workload speedup. Preserve prefix-only ownership cleanup and
  current mutable-target refresh before adding a fast path.
- Next action: integrate the validated retained one-file change; subsequent
  work must continue toward the unmet full-suite stock **1.10x** objective.
