---
title: "Template-Aware Function Registration"
---

# Template-aware function registration

- Status: **LANDED / RETAIN; GENUINE REAL-CLOSURE REGRESSION RED-TO-GREEN,
  JIT LIBRARY AND ALL TARGETS 567 / 567, TRANSFORMED RUNTIME 50 / 50,
  NORMAL AND THREE-ROUND BENCHMARKS AND MATCHED ZERO-LOSS PROFILE
  COMPLETE; FULL CORRECTNESS GATE PASSED**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`rllxmowx`**, commit
  **`d2776d87`**.
- Candidate revision: change **`kvpzmtlp`**, commit **`cb55f41b`**; exactly
  two private production files implement the genuine structured
  RED-to-GREEN candidate.
- Outcome: determine whether private function-registration and metadata
  paths can reuse an already-owned instantiation template and a narrowly
  guarded positive shared trampoline without changing public registration,
  first-call failures, function mutation, or the existing function ABI.

## Hypothesis and evidence

- General-purpose opportunity: repeated source-backed function and
  generator instantiation already owns an `Arc` instantiation template, but
  downstream private registration and metadata preparation may rediscover
  that same template, repeat runtime-id hashing, clone module names, or
  reacquire a shared vectorcall trampoline. Propagating existing ownership
  through one coherent private path may reduce duplicate registration work
  without introducing a competing optimization concept.
- Integrated normal fixed-eight comparison **075301** has stock geometric
  score **0.5782047994439117x**; targeted fixed-four comparison **075611**
  has stock score **0.4154093171730844x**. Neither subset establishes the
  full-suite target, and authoritative full-pyperformance **1.10x stock**
  remains unmet.
- Current generated Apply code is **23,293,040 native bytes / 1,533,550
  machine blocks**, optimized typed coverage **2,866 blocks / 204
  functions**, and serialized pre-optimization BlockPy **14,398,752
  bytes**. The reduced function count reflects prior intentional removal of
  compiler-owned matcher/validator helpers, not missing user functions.
- Fresh current zero-loss chaos profile
  `work/logs/live-guarded-stop-iteration-candidate-chaos_*` contains **639
  raw recorded samples across 70 replay loops**. Inclusive eager factory
  ancestry is **10.010%**, registration **3.754%**, `register_clif`
  **3.598%**, metadata construction **2.189%**, trampoline locking
  **1.253%**, duplicate template lookup **0.782%**, and runtime-id hashing
  **1.408%**. These are overlapping stack ancestries; they must not be
  summed or equated with expected workload speedup.
- Fresh **current post-StopIteration** zero-loss comprehensions profile
  `work/logs/template-aware-registration-baseline-comprehensions_*` uses
  **50,000 loops / 199 Hz** and contains **743 raw recorded samples**,
  **478 distinct aggregated Speedscope stacks**, and total weight
  **100,144**. Function-factory inclusive ancestry is **21.5470%**,
  composed by source contexts nested dict comprehension **11.7131%**,
  `_add_widgets` **5.6598%**, and `_any_knobby` **4.1740%**.
- Current comprehensions `instantiate_shared` ancestry is **19.9293%**,
  inner factory **16.6980%**, `register_jit` **4.8490%**, `register_clif`
  **3.9054%**, metadata construction **3.0975%**, and duplicate
  `SharedModuleState.lookup_function_template` **1.3471%**. Runtime
  function-ID hashing is **2.6931% inclusive / 1.3471% self**, including
  **2.5583%** within factory ancestry; `SipHash` self is **1.7505%**.
  Shared vectorcall trampoline is only **0.2696%**, runtime objects
  **0.9426%**, synthetic code **4.4416%**, and malloc **4.7102%
  whole-workload / 1.3471% within the factory**. No matcher frames remain.
- All profile shares overlap and must not be summed. The primary measured
  opportunity is eliminating duplicate template / runtime-ID hash lookup,
  with borrowed module-name allocation as a possible bonus; trampoline
  reuse is only a **0.2696% sampled** opportunity, not a guaranteed
  material gain. Attached replay approximately **67.25 us** is diagnostic
  only. The earlier previous-revision **738-sample** comprehension figures
  are **superseded** and must not be used as current baseline.
- No current user-visible correctness defect, measured candidate speedup,
  compilation saving, or generated-code reduction has been established.
  First-call failure/reentrancy and mutable-function refresh are material
  constraints, not optional follow-up checks.
- Genuine unchanged-production private structured regression lowers real
  `outer(offset) -> inner(value)` source, constructs **two distinct CPython
  closures**, and successfully registers both through the existing public
  production vectorcall path. Metadata `Arc` pointer identity already equals
  the known existing instantiation template for both functions. The exact
  intended subsequent assertion fails because
  **`prepared_vectorcall_trampoline` is `None`** at `lib.rs:4198`.
  Focused result is **0 passed / 1 failed / 566 filtered in 0.16 seconds**;
  one **30.5-second** compilation is workflow cost only.
- Planned fixture follow-ups require session mismatch and arity mismatch to
  reject cached reuse while preserving both distinct closure values and
  function environments. Production behavior is unchanged before the RED;
  the bounded two-existing-file implementation starts only afterward.
- The exact genuine lowered nested-closure regression now turns
  **RED-to-GREEN: 1 PASSED / 566 filtered in 0.08 seconds**. Private
  known-template registration proves identical supplied `Arc` pointer
  identity and reuses the same session-plus-arity positive trampoline;
  mismatched session and arity are rejected. Two actual `FunctionEnv` ABI
  pointers and closure cells remain distinct, with captured values **3
  versus 9**. No function object, environment, or closure state is shared.
- The strengthened structured regression additionally invokes the unchanged
  **public registration path** and compiles genuinely distinct
  alternate-session and alternate-arity trampolines. Both incompatible
  entries are rejected while the original compatible cache remains intact;
  distinct ABI environments and captured references remain independent.
  Complete `cargo test -p soac_jit --lib` passes **567 / 567 in 5.31
  seconds**.
- Exactly two private production files now propagate the already-owned
  template `Arc`, removing duplicate shared/module/function/template/eager
  `HashMap` lookups. Initialized immutable original-code presence,
  including **`Some(None)`**, is reused; the `Arc`-owned module name is
  borrowed rather than cloning a per-function `String`; positive-only
  session/arity trampoline initialization occurs **outside the `OnceLock`**.
  Public `register(None)`, generator, interpreter, and live-force fallbacks
  remain intact. `#[repr(C)]` / boxed `FunctionEnv` is unchanged; generated
  code is invariant in release debug-single smoke and the normally sampled
  fixed-eight comparison. Independent source review
  is clean; no public API, runtime helper, global, or IR change is added.
  Complete JIT library and all-target suites each pass **567 / 567**;
  transformed-runtime guardrails pass **50 / 50 in 35.45 seconds**,
  covering all five StopIteration regressions, source/synthetic watchers,
  factory/cache mutations, generator monitoring, defaults/code, captured
  builtins, forced interpreter, fixed unpack, inherited/non-self/scalar
  fields, closed pipelines, and constructor virtualization.
  `just fmt-rust-check soac_jit` and
  `cargo check -p soac_jit --all-targets` both pass. The independent normal
  fixed-eight analysis is complete; targeted repeated performance validation
  and the full correctness gate now passes.
- Release debug-single fixed-eight comparison **082252-j02JgI** completes
  **8 / 8** against mode-matched guarded-StopIteration smoke **075039**.
  Aggregate emitted code remains exactly **2,253,100 native bytes /
  148,734 machine blocks**, with unchanged optimized typed coverage
  **2,866 blocks / 204 functions**. Independent per-PID review confirms
  every function and adapter has identical bytes/blocks across all eight
  workloads, with zero errors. Cold single-iteration timings and their
  geometric mean are not steady-state performance evidence.
- Normally sampled fixed-eight comparison **082430-bLoO7z** completes
  **8 / 8** against integrated comparison **075301**. Paired stock score
  changes **0.5782047994439117x -> 0.6028454470492562x**; official
  arithmetic previous-SOAC geometric ratio is **1.0267422803863242x**.
  Individual arithmetic previous-SOAC ratios are chaos **1.0266345x**,
  comprehensions **1.0705593x**, deltablue **0.9591279x**, fannkuch
  **0.9738374x**, float **1.0570756x**, nbody **1.0021573x**, richards
  **1.0465226x**, and spectral_norm **1.0852042x**.
- Independent fixed-eight median-based previous-SOAC geometry is
  **1.0267941x**, or **1.0410265x** after paired-stock adjustment.
  Comprehensions improve **1.0715506x**, with clustered interval
  **[1.03108, 1.11377]**; paired-stock adjustment is **1.1223179x**,
  affected by substantial stock drift and not interchangeable with raw
  throughput. Chaos is neutral at **1.01164x**; the apparent deltablue
  mean regression resolves to robust **0.98733x / 1.00339x paired**,
  consistent with neutral. Richards is **1.04350x**, with confidence
  uncertainty that does not establish a strong independent effect.
  Independent PID matching across all **80 measured workers** confirms
  every function's native bytes and blocks remain unchanged: exactly
  **23,293,040 bytes / 1,533,550 machine blocks**, with identical
  **2,866 typed blocks / 204 functions** and zero errors. Targeted
  three-round comparison **082751-u5MFfc** and matched zero-loss candidate
  profiling and the full correctness gate are complete.
- Targeted comparison **082751-u5MFfc** against prior guarded-stop
  comparison **075611** contains **60 candidate versus 60 baseline
  samples**. Comprehensions improve **1.03462367x**, with worker-cluster
  interval **[1.01672, 1.06397]**, or **1.05486663x stock-adjusted**
  with interval **[1.03275, 1.08669]**; all raw rounds improve
  **1.02754x / 1.03021x / 1.05493x**. Chaos is raw-neutral at
  **1.02215583x [0.99808, 1.05693]**, or **1.05169649x paired
  [1.01150, 1.09102]**.
- The targeted controls also exhibit genuine **raw-sample slowdowns**:
  deltablue **0.96396622x [0.94099, 0.98619]** and richards
  **0.97942974x [0.95978, 0.99654]**. Deltablue's matched stock also
  slows by factor **0.9606669**, yielding neutral paired adjustment
  **1.00343442x [0.97270, 1.04241]**; richards is likewise paired-neutral
  at **0.98241412x [0.95923, 1.00885]**, with a round-two outlier.
  These stock-drift results are evidence against confidently attributing
  the raw regressions to the candidate, not permission to omit them.
  Raw four-workload geometric improvement is **0.999617x** (neutral),
  stock-adjusted geometry **1.0226285x**, and official arithmetic result
  approximately **0.99511x**. Targeted native code remains exactly
  **18,352,680 bytes / 1,206,840 blocks**. The candidate merits
  retention after the full correctness gate passes; the full-suite stock
  target remains unmet.
- Matched zero-loss comprehensions profiles use the same **50,000 loops /
  199 Hz**: raw recorded samples decrease **743 -> 707**, with distinct
  weighted stacks **478 -> 436**. Inclusive function-factory ancestry
  decreases **21.547% -> 17.529%**, and shared instantiation
  **19.929% -> 16.397%**. Duplicate template lookup specifically inside
  metadata decreases **0.8078% -> 0%**, and metadata `hash_one`
  **0.6730% -> 0%**; total template lookup remains
  **1.3471% -> 0.8492%**, because unrelated valid lookup paths remain.
  All `RuntimeFunctionId` hashing decreases **3.3672% -> 1.1316%**, and
  `SipHash` write **1.7505% -> 0.8492%**. Registration decreases
  **4.849% -> 4.522%**, while remaining allocation means metadata
  increases **3.0975% -> 3.3919%**; engine ancestry decreases
  **0.4044% -> 0.1412%**. GC is comparable at
  **17.892% -> 17.973%**, and neither profile samples compilation.
  Stack shares overlap and cannot be added. Attached replay
  **67.2500 -> 63.7527 us (1.05488x)** is diagnostic only; repeated
  workload medians, including candid raw delta/richards slowdowns and
  paired-neutral controls, remain the acceptance evidence.
- Conservative source-backed **upper estimates**, not promised gains:
  current comprehensions have approximately **0.808 percentage points** of
  disjoint duplicate-template subtree, **0.673** original-code hashing,
  **0.538** eager static-lookup hashing, and **0.270** trampoline work, or
  roughly **2.29 percentage points** before any optional module-name
  allocation savings. Chaos has approximately **0.626 + 0.156 + 1.253 =
  2.04 percentage points**. Nested hash frames are already included in
  their disjoint parent estimate and must not be summed again. These are
  recoverable ceilings, not predicted candidate speedups.

## Implementation and compatibility

- Implemented bounded production scope: exactly
  `crates/soac_jit/src/lib.rs` and
  `crates/soac_jit/src/function_instantiation.rs`. Both are existing files;
  both now contain the private template-aware registration changes.
- Propagate the already-owned `Arc<FunctionInstantiationTemplate>` through
  private function registration and metadata creation rather than resolving
  it again. Borrow the existing module-name string instead of introducing a
  redundant clone. Public registration callers, exported signatures, and
  external behavior remain unchanged.
- Optionally reuse a **positive-only shared vectorcall trampoline** from the
  existing template, keyed by the exact compile-session identity and
  callable arity. Cache only a previously successful compatible trampoline;
  failed initialization or incompatible/missing entries must run the
  original path and preserve first-call engine errors, retry ordering, and
  callback-visible reentrancy.
- Preserve every existing admission/invalidation check: force-interpreter
  modes, ordinary vs generator call convention, compile mode/session,
  current function/code identity, `__code__`, positional/keyword defaults,
  current runtime helper/module state, and profile/trace/local/global
  monitoring. Never reuse a trampoline across different callable arity,
  session, generator mode, or original code.
- Keep each function's `FunctionEnv` **Box allocation, ownership, ABI
  header, layout, pointer offsets, closure bindings, traversal, cleanup,
  and lifetime exactly unchanged**. A shared trampoline is not shared
  function state. Preserve callback/finalizer ordering, exceptions,
  registration failures, shutdown, weak module behavior, and current-code
  refresh on every invocation.
- Add no new public API, exported register behavior, process-global mutable
  cache, runtime helper, IR node, optimization plan, profile schema, or
  function ABI. Existing per-template owned state is the only possible
  caching home.
- Genuine structured unchanged-production production-registration RED
  initially failed at the missing prepared trampoline after successful real
  closure registration and existing-template `Arc` identity checks. The
  candidate structured test validates positive reuse, actual session/arity
  mismatch, unchanged public registration, distinct closures, and function
  environments. Broad transformed guardrails cover forced interpreter,
  generator, and code/default mutation; engine initialization remains
  outside `OnceLock` to preserve first-call failure/reentry behavior.

## Benchmark protocol and coverage

- Fixed benchmark selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`, compared
  against the same vendored stock CPython and integrated guarded
  StopIteration SOAC revision. Independently profile each revision and
  validate actual source-backed factory/registration stacks plus prior
  StopIteration, owner/scalar, watcher, generator, mutation, and shutdown
  guardrails.
- Baseline fixed-eight artifact is comparison **075301**; baseline targeted
  fixed-four artifact is comparison **075611**. Existing coverage is
  **2,866 typed blocks / 204 functions** and
  **23,293,040 native bytes / 1,533,550 machine blocks**.
- Fresh current chaos profile has **639 raw recorded samples** and fresh
  current comprehensions **743 raw samples / 478 distinct stacks / 100,144
  total weight**. The earlier **738-sample** comprehension profile belongs
  to a superseded previous revision. The candidate normal fixed-eight run
  preserves every measured worker/function's emitted code. Independent
  normal robust medians, paired-stock adjustment, and PID audit are
  complete. The targeted three-round repeat is complete, with raw control
  regressions and matched stock drift disclosed; matched zero-loss
  candidate profiling and the authoritative full gate are complete. Separate
  startup-only measurements were not required and are unavailable.

## Measurements

| Metric | Integrated guarded-StopIteration baseline | Candidate | Change |
| --- | --- | --- | --- |
| Normal fixed-eight paired stock / SOAC geometry | 0.5782047994439117x | 0.6028454470492562x | full-suite stock 1.10x goal unmet |
| Normal fixed-eight arithmetic previous-SOAC geometry | comparison 075301 | 1.0267422803863242x | robust and paired-stock analysis separately reported |
| Targeted fixed-four paired stock / SOAC geometry | 0.4154093171730844x | pending | subset only; not full-suite acceptance |
| Previous-SOAC robust / stock-adjusted improvement | integrated `rllxmowx/d2776d87` | 1.0267941x / 1.0410265x | paired stock drift materially affects some workloads |
| Normal comprehensions robust / paired-stock improvement | integrated comparison 075301 | 1.0715506x / 1.1223179x | clustered interval [1.03108, 1.11377]; substantial stock drift |
| Targeted 60-vs-60 four-workload raw / stock-adjusted geometry | comparison 075611 | 0.999617x / 1.0226285x | raw neutral; official arithmetic approximately 0.99511x |
| Targeted comprehensions raw / stock-adjusted improvement | comparison 075611 | 1.03462367x / 1.05486663x | raw interval [1.01672, 1.06397]; all three rounds improve |
| Targeted deltablue / richards raw control ratios | comparison 075611 | 0.96396622x / 0.97942974x | both raw intervals below one; stock-adjusted 1.00343442x / 0.98241412x neutral |
| Targeted Apply native bytes / machine blocks | 18,352,680 / 1,206,840 | 18,352,680 / 1,206,840 | unchanged |
| Optimized typed-IR blocks / functions | 2,866 / 204 | 2,866 / 204 | unchanged |
| Serialized pre-optimization BlockPy bytes | 14,398,752 | pending | pending |
| Apply-mode native code bytes / machine blocks | 23,293,040 / 1,533,550 | 23,293,040 / 1,533,550 | all 80 PIDs/functions byte-identical; zero errors |
| Release debug-single fixed-eight smoke | guarded-StopIteration 075039 | 8 / 8; 2,253,100 bytes / 148,734 blocks; typed 2,866 / 204 | every PID/function/adapter identical; zero errors; cold timings invalid |
| Current chaos zero-loss samples / loops | 639 / 70 | pending | pending |
| Current chaos inclusive factory / registration / metadata ancestry | 10.010% / 3.754% / 2.189% | pending | overlapping; not additive speedup |
| Current chaos trampoline lock / duplicate lookup / runtime hash | 1.253% / 0.782% / 1.408% | pending | overlapping; not additive speedup |
| Current post-StopIteration comprehensions raw samples / stacks / weight | 743 / 478 / 100,144 | 707 raw samples / 436 distinct stacks | matched zero-loss; 50,000 loops / 199 Hz |
| Current comprehensions factory / registration / metadata ancestry | 21.5470% / 4.8490% / 3.0975% | 17.529% / 4.522% / 3.3919% | overlapping; metadata retains allocation |
| Current comprehensions total template lookup / all runtime-ID hashing | 1.3471% / 3.3672% | 0.8492% / 1.1316% | overlapping; other valid template lookups remain |
| Metadata duplicate template lookup / hash ancestry | 0.8078% / 0.6730% | 0% / 0% | targeted duplicated lookup and hash disappear |
| Matched attached profiling replay | 67.2500 us | 63.7527 us | 1.05488x diagnostic only; not throughput headline |
| Prior-revision comprehensions profile | 738 samples; factory 20.198% | not applicable | superseded; not current baseline |
| Genuine structured production registration / existing-template regression | 0 passed / 1 failed / 566 filtered in 0.16 s; prepared trampoline None | 1 passed / 566 filtered in 0.08 s | genuine RED-to-GREEN; real lowered closures and identical supplied Arc |
| Positive-only trampoline session / arity / independent closure guardrails | no cached trampoline | shared compatible session/arity only; mismatches rejected; distinct env/cells capture 3 and 9 | GREEN focused structured case |
| Strengthened public registration / incompatible actual trampoline test | unchanged public registration | real alternate-session and alternate-arity trampolines rejected; original cached entry retained | GREEN; ABI environments / captures independent |
| Complete JIT Rust library | integrated guarded-stop baseline | 567 / 567 passed in 5.31 s | GREEN |
| Complete JIT Cargo test targets | integrated guarded-stop baseline | 567 / 567 passed | GREEN |
| Broad transformed-runtime compatibility | integrated guarded-stop baseline | 50 / 50 passed in 35.45 s | GREEN; watchers, mutation, generators, owner fields, and constructor virtualization |
| Scoped formatting / all-target Cargo check | integrated guarded-stop baseline | both passed | GREEN |
| Disjoint source-backed recoverable upper estimate | comprehensions ~2.29 points; chaos ~2.04 points | unmeasured | conservative ceiling; no promised candidate gain |
| Full `just test-all` correctness gate | integrated baseline previously passed | 1,227 nodeids; 90 / 90 batches passed; 567 JIT / 212 optimizer / 54 typed / 371 lowering / 8 PyO3 | GREEN; cargo 68.197 s, pytest 78.846 s, total 147.071 s |

## Attempt history

### Attempt 1: identify duplicated template-aware registration work

- Change: inspect fresh integrated chaos **and current post-StopIteration
  comprehensions** native call stacks against existing instantiation-template
  registration, metadata, runtime-id lookup, and trampoline initialization
  paths. The earlier comprehension profile is superseded. A genuine
  unchanged-production structured regression then fails after successful
  real closure registration; production behavior has not changed before its
  RED.
- Measurements and coverage: current chaos **639 raw samples / 70 loops**,
  factory **10.010%**, registration **3.754%**, metadata **2.189%**,
  trampoline lock **1.253%**, duplicate lookup **0.782%**, runtime hash
  **1.408%**, all overlapping. Current comprehensions have **743 samples /
  478 distinct stacks / 100,144 weight**, factory **21.5470%**,
  registration **4.8490%**, duplicate template lookup **1.3471%**, runtime
  hash **2.6931% inclusive / 1.3471% self**, and shared trampoline only
  **0.2696%**; older **738-sample** figures are superseded. Existing
  fixed-eight stock score is **0.5782047994439117x**.
- Compatibility and tests: existing `Arc` template and a positive
  session/arity-specific trampoline must preserve first-call failures,
  reentry, interpreted/generator modes, code/default refresh, public
  registration behavior, and the exact per-function `FunctionEnv` Box/ABI.
  The genuine structured regression reports
  **0 passed / 1 failed / 566 filtered in 0.16 seconds**, failing exactly
  when existing-template `prepared_vectorcall_trampoline` is absent after
  two successful public vectorcall registrations and `Arc` identity proof.
  Its initial **30.5-second** compile is workflow-only. The same real
  production-path regression then turns **GREEN 1 / 566 filtered in
  0.08 seconds**, proving same `Arc`/positive trampoline reuse, session and
  arity mismatch rejection, distinct `FunctionEnv` pointers, and independent
  closure captures **3 versus 9**. The strengthened fixture also invokes
  unchanged public registration and builds truly distinct session/arity
  trampolines while preserving the valid original cache. Full JIT library
  and all test targets each pass **567 / 567**, while broad transformed
  compatibility passes **50 / 50 in 35.45 seconds**; scoped formatting and
  the all-target Cargo check pass. The two-file implementation
  eliminates duplicate lookup/hashing, borrows module names, preserves
  `Some(None)` and public fallback behavior. Source-backed disjoint ceilings
  are roughly **2.29 percentage points** for comprehensions and **2.04**
  for chaos, not promised gains. Matched measurements and the authoritative
  full correctness gate both pass; raw delta/richards controls and paired
  stock drift remain disclosed.
- Result: **LANDED / RETAIN; genuine real-closure structured RED-to-GREEN,
  bounded two-file private implementation, JIT library and all targets
  567 / 567, transformed runtime 50 / 50; matched comprehensions gain and
  full correctness gate 90 / 90 batches passed**.
- Reason: immutable successful trampoline metadata can potentially be shared,
  but per-function state, mutable target behavior, failure ordering, and
  public registration semantics cannot be collapsed.

## Verdict and next action

- Verdict: **LANDED / RETAIN; FULL CORRECTNESS GATE PASSED**;
  genuine unchanged-production real-closure structured
  regression turns **GREEN 1 / 566 filtered**, and the exact two-file
  private implementation preserves distinct environments and real
  session/arity mismatch fallback; unchanged public registration remains
  valid. Complete JIT library and all targets each pass **567 / 567**,
  transformed guardrails pass **50 / 50 in 35.45 seconds**, and scoped
  formatting/all-target checks pass. Release fixed-eight debug-single smoke
  passes **8 / 8** with every PID/function/adapter unchanged and no errors.
  The first normal fixed-eight run reports official previous-SOAC ratio
  **1.0267422803863242x**, robust **1.0267941x / 1.0410265x
  stock-adjusted**, and unchanged native code across all **80 measured
  workers**. Targeted comprehensions improve **1.03462367x raw /
  1.05486663x paired**, but delta/richards show raw regressions that become
  neutral after matched stock adjustment; raw subset geometry is
  **0.999617x**. Matched zero-loss profiling confirms targeted duplicated
  metadata lookup **0.8078% -> 0%** and hashing **0.6730% -> 0%**;
  the authoritative full correctness gate passes. No public API change
  exists, and the full-suite stock goal remains unmet.
- Authoritative `just test-all` log
  `work/logs/template-aware-registration-test-all.log` proves **1,227
  Python nodeids / 90 isolated file-local batches / 8 workers**, with
  **90 passed / 0 failed**. Workspace Rust suites include **567 JIT**,
  **212 optimizer**, **54 typed-IR**, **371 lowering**, and **8 PyO3**
  passing tests. Cargo takes **68.197 seconds**, pytest
  **78.846 seconds inner / 78.862 seconds outer**, and the complete test
  phase **147.071 seconds**; the known counter-dump batch takes
  **78.76 seconds**.
- Transferable lesson: stack ancestry is overlapping. Reuse already-owned
  compiler concepts before adding caches, and never trade first-call error
  semantics or `FunctionEnv` ABI stability for registration throughput.
- Next action: retain the validated change; future optimization should use
  fresh source-backed profiles without overstating the small **0.2696%**
  baseline trampoline opportunity or the still-unmet full-suite stock goal.
