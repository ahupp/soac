---
title: "Exact Positional Argument Binding"
---

# Exact positional argument binding

- Status: **ATTEMPT 1 LANDED / RETAINED; ATTEMPT 2 LANDED CANDIDATE /
  RETAIN, FULL CORRECTNESS GATE GREEN**. Attempt 1's normal fixed-eight,
  targeted three-round,
  matched zero-loss profiles, identical native code, and full correctness
  gate remain verified. Attempt 2 has genuine unchanged-production
  transformed-integration and independent structured Rust RED-to-GREEN
  regressions, a complete **570 / 570** JIT Rust library, and expanded
  transformed coverage **35 passed / 1 known expected xfail**; final
  scoped formatting/checks, post-format regressions, release smoke,
  normally sampled fixed-eight and repeated target comparisons, and matched
  zero-loss profiles are complete; the authoritative full correctness gate
  passes **1,230 Python nodeids / 93 isolated batches** and every Rust
  suite.
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

## Attempt 2: generated exact-positional trampoline binding

- Status: **LANDED CANDIDATE / RETAIN; genuine unchanged-production transformed
  specialization integration AND independent production-path structured
  Rust RED-to-GREEN verified; full JIT library 570 / 570 GREEN; expanded
  transformed suite 35 PASS / 1 expected xfail; final scoped
  formatting/checks, post-format regressions, and release smoke GREEN;
  normally sampled and repeated target comparisons plus matched zero-loss
  profiles complete; authoritative full correctness gate GREEN**. Attempt 1
  above remains landed and retained;
  this is a subsequent iteration of the same strategy, not a replacement
  for its historical architecture, measurements, or verdict.
- Integrated baseline: change **`lnxvnnml`**, commit **`89cf1193`**.
  Candidate change **`rukzksko`** was initially observed at mutable working
  commit **`ad1a4479`**; that commit identifier will change as the working
  revision is snapshotted.
- Authoritative retained fixed-eight baseline
  **`comparison-20260819-131748-1b79JF`** has stock geometric score
  **0.6326613107877241x**, **23,188,640 native bytes / 1,527,950 machine
  blocks**, and optimized typed coverage **2,866 blocks / 204 functions**.
  Retained targeted three-round baseline
  **`comparison-20260819-132104-LvP5XE`** has stock score
  **0.44758856139159614x** and per-round generated coverage **18,255,240
  native bytes / 1,201,600 machine blocks / 2,265 typed blocks / 183
  functions**. The corresponding retained release smoke is comparison
  **131641**. The full-suite stock **1.10x** objective remains unmet.

### Current source and zero-loss profile evidence

- The retained immutable exact-positional binder from Attempt 1 removes
  generic binding work after entry, but the actual generated vectorcall
  trampoline still calls the Rust binder and then enters a default adapter.
  The existing process-wide trampoline cache is keyed only by arity, so
  eligible positional functions currently share their generated trampoline
  with same-arity keyword-only or variadic functions that are ineligible.
- Current zero-loss richards profile attributes a **7.922%** whole-workload
  binding union to direct-wrapper self **3.873%**, nested binder **3.169%**,
  and refresh **0.880%**. These describe that union; inclusive parent frames
  must not be added again, and required ownership/default checks are not
  automatically removable. A separate default-adapter **self** share of
  **2.112%** gives a source-backed, disjoint **10.034 percentage-point gross
  ceiling**, not a predicted speedup. An earlier profile's binder share was
  **4.628%**, illustrating sampling/revision sensitivity rather than a
  separately additive gain.
- Existing delta profiles attribute binder **4.658–7.065%** plus disjoint
  default-adapter self **1.370–1.694%**, or an approximately
  **6.028–8.759 percentage-point gross range**. Chaos attributes binder
  **2.318%** plus adapter self **0.435%**, or **2.753 percentage points
  gross**. Comprehensions is approximately zero for this opportunity and
  must be treated as an unchanged guardrail, not a promised target win.
- Source census finds **39 / 39 richards** and **23 / 23 chaos** functions
  eligible for an immutable fully supplied exact-positional plan; the hot
  wrapper arities are **1 / 2 / 3**, with chaos also exercising arity **4**.
  Static eligibility does not prove every dynamic call has matching
  arguments, current defaults, or an unchanged code object.
- Existing generated benchmark-body summaries exclude vectorcall
  trampolines. Direct measured-worker `jitdump` inspection gives hidden
  retained trampoline totals **richards 5,824 bytes**, **deltablue 5,236
  bytes**, **chaos 5,236 bytes**, and **comprehensions 3,276 bytes**;
  observed individual arity sizes are **0: 588**, **1: 756**, **2: 884**,
  **3: 1,048**, **4: 1,128**, and **6: 1,420 bytes**. Candidate trampoline
  growth must be audited independently even if normal native-body summaries
  remain unchanged.

### Bounded candidate production architecture

- Final candidate implementation is **FROZEN** in exactly three authorized
  existing production files:
  `crates/soac_jit/src/lib.rs`,
  `crates/soac_jit/src/jit/process.rs`, and
  `crates/soac_jit/src/jit/vectorcall.rs`. It was written only after both
  unchanged-production REDs, compiles, and passes both focused production
  regressions. Independent host source review reports no blocker; no
  user-visible CPython mismatch is claimed.
- Reuse the existing immutable
  `binds_exact_positional(requested_arity, NULL)` decision rather than
  introducing another binding concept; cap generated exact-positional
  trampolines at **eight arguments**. Partition the existing process cache
  by **`(arity, exact_positional_eligible)`**, so same-arity keyword-only,
  variadic, over-cap, or otherwise ineligible functions retain the original
  shared generic trampoline. A default-capable exact function remains
  eligible and installs the exact trampoline; a call omitting its defaults
  takes that same trampoline's embedded original generic-binder /
  default-adapter fallback arm. Preserve the existing two-argument generic
  engine method because other current production paths and tests still
  call it; the new exact-shape engine path uses distinct generated symbols
  and the partitioned existing cache.
- In the eligible trampoline, guard the vectorcall argument count after
  masking its offset flag, exact requested arity, null keyword names,
  current function/default snapshots, null keyword defaults, and valid
  non-null positional argument pointers. Preserve the original `nargsf`,
  zero-arity handling, null-buffer/error precedence, recursion behavior,
  current-code/default mutation detection, per-prefix cleanup, and the
  original Rust binder on every guard miss.
- After every argument is validated and supplied, use existing pinned
  `RefcountLowering` for inline immortal-aware owned-reference increments.
  The all-arguments/no-omitted-default proof also permits entering existing
  **`FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET`** core code directly instead of
  its default adapter; both existing entries share the same ABI. The
  current direct-call planner already chooses `Core` for fully supplied
  arguments. Every generic/mutated/partial call must continue through
  **`FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET`** and the unchanged
  original binder; core entry must never observe an omitted default.
- Preserve exact mortal/immortal increments and decrements, malformed
  partial-argument cleanup, exception ordering, recursion and monitoring,
  source/code/default mutations, keyword/default/keyword-only/variadic
  calls, generator behavior, and forced-interpreter fallback. Reuse
  existing private ABI offsets and process state; add no public API,
  runtime helper, global mutable state, IR node, or benchmark direct-body
  shape. Trampoline code itself is expected to change and requires the
  explicit `jitdump` size audit described above.

### Required regressions, measurements, and verdict

- Independent genuine unchanged-production structured Rust **RED**:
  `cargo test -p soac_jit --lib
  exact_positional_vectorcall_trampolines_partition_actual_template_shapes`
  runs **1 failed / 0 passed / 569 filtered** through real lowered
  function templates, `CompileSession`, and the existing process cache.
  Eligible exact and ineligible generic targets incorrectly share actual
  installed pointer **`267842752615856`**, failing only the final required
  partition assertion. Same-shape eligible/default sharing, keyword-only /
  `*args` / `**kwargs` generic sharing, and the **nine-argument cap**
  fallback all pass before that intended failure.
- Reviewer-owned actual transformed integration
  **`tests/test_exact_positional_vectorcall_trampoline.py`** now establishes
  a genuine unchanged-production **RED: 1 failed in 2.36 seconds**. Real
  **Profile → Verify → Apply** subprocesses all execute successfully, and
  exported **`PyVectorcall_Function`** reports both eligible `exact_a` and
  ineligible `keyword_only` installed at the same actual pointer
  **`261253012398080`**. The sole final failing assertion requires those
  same-arity trampoline/cache identities to be partitioned.
- Before that intended structural failure, all existing controls pass:
  same-shape trampoline reuse, current defaults/keyword-default/code
  mutations, keyword and variadic fallback, vectorcall offset handling,
  owned references/finalizers/exceptions, actual `call_hot_targets`
  execution, and compiled native direct-body evidence. This is a genuine
  missed-specialization RED, not a claimed user-visible CPython mismatch;
  production was unchanged when the failure was captured.
- The unchanged structured Rust regression now verifies genuine production
  **RED → GREEN: 1 passed / 569 filtered**. Its real lowered templates,
  compile session, and process cache prove eligible exact targets share the
  exact trampoline, keyword-only / `*args` / `**kwargs` targets share the
  separate generic trampoline, defaults preserve their intended shape, and
  arity **nine** retains the existing capped generic fallback.
- The frozen transformed integration independently verifies genuine
  **RED → GREEN: 1 passed in 1.50 seconds**. Actual
  **Profile → Verify → Apply** subprocesses now expose distinct installed
  exact/generic `PyVectorcall_Function` pointers while retaining every
  defaults/keyword-default/code-mutation, variadic/keyword, offset,
  reference/finalizer/exception, counter, and compiled native body/adapter
  control. Separate debug-extension staging/build took **30.29 seconds** as
  workflow overhead, not transformed test runtime or benchmark evidence.
- Initial complete `cargo test -p soac_jit --lib` passes **570 / 570**
  tests in **6.05 seconds**, including the new structured production-path
  regression; the retained baseline previously had **569** JIT tests.
- Expanded actual transformed compatibility matrix passes **35 tests / 1
  known preexisting expected xfail**, across **13 files / 36 cases in
  21.87 seconds**. Coverage includes real **Profile → Verify → Apply**,
  current defaults/keyword-default/code mutation, omitted arguments and
  arity/keywords, function watchers, closures, generators, previously
  retained guarded `any` / `all`, exception behavior, owner guards, and
  captured builtins.
- Final post-format complete JIT Rust library and all Cargo test targets
  each pass **570 / 570**, with the final test run taking **6.99 seconds**.
  The unchanged frozen real transformed **Profile → Verify → Apply**
  integration passes again **1 / 1 in 1.58 seconds**. Package-scoped
  `just fmt-rust soac_jit`, `just fmt-rust-check soac_jit`, and
  `cargo check -p soac_jit --tests` all pass; the aligned test-target check
  completes in **2.94 seconds**.
- Final implementation is frozen in the **three authorized production
  files**, with the immutable capped exact-positional decision, unchanged
  generic engine method plus exact method / `(arity, shape)` cache, unique
  generated symbols, existing inline immortal-aware reference increments,
  full current-code/default/keyword-default/argument guards, direct `Core`
  entry, original cleanup, and untouched generic binder/default-adapter
  fallback. Both focused genuine RED-to-GREEN regressions, the complete
  **570 / 570** JIT Rust library, and expanded **35 passed / 1 expected
  xfail** transformed matrix now pass, as do scoped formatting, formatting
  check, aligned Cargo test-target check, and final post-format library /
  integration reruns. The authoritative full `just test-all` gate also
  **PASSES**; complete counts and timings are recorded below.
- Release debug-single fixed-eight smoke comparison **141038** against
  retained mode-matched **131641** completes **8 / 8**, with **zero
  errors**. All **397** actual measured Apply direct-function and adapter
  rows match exactly, retaining **2,242,168 native bytes / 148,116 machine
  blocks**, optimized coverage **2,866 typed blocks / 204 functions**, and
  **7,199,376 pre-optimization BlockPy bytes**.
- Separately audited `jitdump` exposes real hidden trampoline growth absent
  from ordinary direct-body summaries: aggregate **28,720 → 36,500 bytes**,
  or **+7,780 bytes / +27.09%** (**+0.347%** relative to the smoke's
  **2,242,168** ordinary direct-body bytes). Each workload adds only the
  intended **`_exact_positional`** trampoline shape, with no duplicated
  generic trampoline: chaos **5,236 → 6,692**, comprehensions
  **3,276 → 4,088**, deltablue **5,236 → 6,692**, fannkuch
  **756 → 952**, float **1,640 → 2,076**, nbody **5,112 → 6,512**,
  richards **5,824 → 7,412**, and spectral norm **1,640 → 2,076 bytes**.
  Retained per-arity sizes are documented above; this hidden code growth
  is a real compatibility/maintenance tradeoff even though all regular
  emitted-body rows are unchanged.
- Aggregated measured-worker setup changes **3,450.663 → 2,911.542 ms**,
  but is nonheadline workflow context. All cold debug-single timing,
  arithmetic, and setup comparisons are invalid as throughput evidence;
  no candidate speedup is claimed.
- Normally sampled fixed-eight comparison **141233** against retained
  **131748** completes **8 / 8**. Official candidate stock geometric score
  is **0.6146084338507914x**, down from retained
  **0.6326613107877241x**; official previous-SOAC arithmetic is
  **1.0388446426221598x**. Robust fixed-eight previous geometry is
  **1.036288x**, but stock-adjusted geometry is only **0.995015x**;
  unusually large paired-stock float drift **40.951 → 34.269** and
  benchmark outliers prevent either official aggregate from establishing a
  causal suite-wide gain.
- Actual target richards improves median **30.027457 → 27.912782 ms**,
  **1.075760x [1.04830, 1.08815]**, or stock-adjusted
  **1.064496x [1.03199, 1.08259]**. Deltablue improves
  **3.212536 → 2.832788 ms**, **1.134054x [1.10732, 1.20505]**, or
  stock-adjusted **1.109951x [1.08253, 1.19208]**. Chaos is **1.034747x**
  raw but stock-adjusted **0.993167x**, consistent with neutral;
  comprehensions is **1.006726x** raw / **0.972032x** adjusted and does
  not establish a source-backed target gain.
- Independent actual Apply-PID audit confirms all **80** normal workers /
  **3,970** direct-function and adapter rows match exactly, with **zero
  errors**, **23,188,640 native bytes / 1,527,950 machine blocks**, and
  unchanged **2,866 typed blocks / 204 functions**. Separate measured-worker
  `jitdump` audit exposes hidden trampolines **287,200 → 365,000 bytes**,
  **+77,800 / +27.09%**, approximately **0.335%** of ordinary native
  code; only the exact shape grows, with no duplicated generic trampoline.
  Median setup does not regress: richards **671.0 → 669.9 ms**, deltablue
  **733.3 → 591.7 ms**, chaos **557.7 → 545.1 ms**, and comprehensions
  **461.7 → 453 ms**. Setup remains supporting context, not throughput.
- Final clean three-round targeted comparison **141538** against immediate
  retained **132104** confirms richards **29.844901 → 28.280179 ms**, or
  **1.055329x [1.046396, 1.074325]** / paired-stock
  **1.060877x [1.050755, 1.082730]**; the three raw rounds are
  **1.084518x / 1.071323x / 1.044516x**. Deltablue improves
  **3.176625 → 2.928699 ms**, or **1.084654x [1.069272, 1.099800]** /
  paired-stock **1.084153x [1.062347, 1.100416]**, with rounds
  **1.085989x / 1.055059x / 1.098654x**. Both target confidence intervals
  exclude neutral before and after paired-stock adjustment.
- Three-round chaos is **1.008679x** raw / **0.999914x** paired and its
  interval includes neutral; comprehensions is **0.998851x** raw /
  **0.980417x** paired, with an interval including neutral and mild stock
  sensitivity. Four-workload robust geometry is **1.036295x** /
  stock-adjusted **1.030463x**. Official targeted previous-SOAC arithmetic
  is **1.0410906398909108x** and stock score
  **0.4625866444625596x**, compared with retained
  **0.44758856139159614x**; that subset does not satisfy or establish the
  full-suite stock goal.
- All **120** targeted Apply PIDs preserve **10,650** direct-function /
  adapter rows, zero errors, and aggregate **54,765,720 native bytes /
  3,604,800 machine blocks** across three rounds; per round this is exactly
  **18,255,240 bytes / 1,201,600 blocks / 2,265 typed blocks / 183
  functions**. Separate all-worker hidden trampolines grow
  **587,160 → 746,520 bytes (+159,360)**, entirely the intended exact
  shape with no duplicated generic trampoline.
- Matched **same-source**, zero-loss richards profiles contain **568 → 395
  samples across 70 loops**. Entire old binder ancestry falls
  **7.92182% → 0%**, including binder-wrapper self **3.87289% → 0%**,
  nested binder **3.16873% → 0%**, and refresh **0.88020% → 0%**;
  separately disjoint default-adapter **self** falls
  **2.11249% → 0%**. The **10.03431-percentage-point** gross opportunity
  is not a speedup prediction; direct-wrapper self increases
  **6.51350% → 8.86146%** from inlined validation/reference work, and
  inclusive ancestry overlaps. Candidate delta has a zero-loss **276-sample
  / 400-loop** profile with residual binder **0.72435%** and adapter self
  **0%** on rare generic calls; the available older **365-sample** delta
  baseline is from a different revision and is not a valid causal match.
- Authoritative full `just test-all` **exits zero**; see
  **`work/logs/exact-positional-trampoline-test-all.log`**. All **1,230
  Python nodeids / 93 isolated batches / 8 workers** pass (**93 passed / 0
  failed**). Rust suites pass JIT **570**, lowering **371**, optimizer
  **213**, typed IR **54**, and PyO3 **8**. Cargo takes **67.263
  seconds**, inner / outer parallel pytest **77.522 / 77.537 seconds**,
  and the complete test phase **144.812 seconds**. The new actual
  transformed integration passes in **2.03 seconds**; the existing
  **28-node** counter-dump batch takes **77.38 seconds**.
- Current verdict: **ATTEMPT 2 LANDED CANDIDATE / RETAIN; Attempt 1
  remains RETAINED; FULL CORRECTNESS GATE GREEN**. Independent repeated
  richards and delta target improvements,
  matched-source mechanism elimination, unchanged ordinary native bodies,
  and genuine structured/transformed RED-to-GREEN support retention while
  explicitly accepting real hidden trampoline growth and lower fixed-eight
  stock score under drift. The authoritative full `just test-all` gate
  passes all **1,230 Python nodeids / 93 batches** and every workspace
  Rust suite; full-suite stock **1.10x** remains unmet.
