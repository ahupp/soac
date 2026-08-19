---
title: "Direct Trusted Generator-Instance Preserved State"
---

# Direct generator-instance preserved state

- Status: **LANDED / RETAIN; NORMAL FIXED-EIGHT, THREE-ROUND GUARDRAILS,
  ZERO-LOSS NATIVE PROFILE, AND FULL CORRECTNESS GATE ALL PASS**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`xtnupnyk`**, commit
  **`b0238a27`**.
- Candidate revision: **`okqlrmxm`**.
- Outcome: investigate whether a canonical original-code generator factory
  can directly create its trusted preserved-state capsule and initialize a
  real `ClosureGenerator` without reentering the generic Python interpreter,
  while restoring exact CPython generator name/qualified-name identity.

## Hypothesis and evidence

- General-purpose opportunity: ordinary transformed generator expressions
  and nested generator factories repeatedly create a new generator instance
  through a Python runtime helper, interpreted state-preservation bridge,
  and generic helper dispatch. A narrowly validated direct state path could
  remove repeated setup work without recognizing benchmark names or changing
  generator execution semantics.
- The integrated full-eight paired stock score is only
  **0.5132971537493283x**; the full-suite **1.10x stock** performance target
  remains unmet. Current normally sampled `comprehensions` robust SOAC
  latency is approximately **66.395 us**, versus paired stock approximately
  **7.569 us**, about **0.114x** stock throughput. This single-workload or
  eight-workload evidence does not establish full-suite acceptance.
- The current matched zero-loss `comprehensions` native profile contains
  **849 CPU-clock samples**. Inclusive `generator_factory_vectorcall`
  accounts for **18.133%**, its Python runtime helper approximately
  **15.43%**, interpreter execution **14.02%**, the state-preservation
  bridge **4.003%**, preserved-state handling **3.414%**, and garbage
  collection **6.006%**. Stack shares overlap and must not be summed.
- Even removing the entire **18.133%** factory stack would imply an absolute
  theoretical workload ceiling around **1.2215x**, not 18.133% plus its
  nested stack shares. A realistic candidate target is approximately
  **5–12% whole-workload improvement**, subject to actual independent normal
  profiling and measured compatibility overhead; no speedup is established.
- Genuine unchanged-production CPython compatibility regression:
  `tests/test_generator_instance_templates.py` **fails one test in 0.63
  seconds**. Stock plain and captured generator `__name__` /
  `__qualname__` identities are **all TRUE**, whereas transformed SOAC
  returns **FALSE for all four checks**. This confirms that the current
  source-backed path loses canonical original Python function Unicode-object
  identities, rather than merely changing equal string contents.
- Every prior semantic guard already passes in that same unchanged-production
  run: independent captured values/cells; lazy body startup; `send`,
  `throw`, `close`, and finalizer behavior; twice-cached runtime helper
  replacement **before first call**; live helper-global and generator-class
  changes; code-local `sys.monitoring` before global monitoring; global
  monitoring; profiling; and tracing. The regression's flattened JSON event
  contract was corrected so these checks observe real generated behavior.
  The failure is specifically canonical generator-name object identity.
- A second independent genuine unchanged-behavior structured Rust
  regression,
  `compiler_owned_preserved_state_initializes_raw_slots_without_python_tuples`,
  reports **0 passed / 1 failed / 559 filtered**. Its exact assertion
  requires compiler-owned preserved **object and scalar slots** to create
  the trusted state capsule **without Python tuples**; unchanged production
  cannot satisfy that direct raw-slot path. The four-file implementation
  begins only after both this structured RED and the real generator
  identity RED are established.
- The exact same private structured preserved-state regression now turns
  **RED-to-GREEN: 1 passed / 559 filtered**. It proves direct capsule
  construction from a raw **owned Python-object slot plus unboxed `i64`
  scalar slot**, correct capsule destruction, and cleanup of an abandoned
  partially initialized builder. This validates state ownership, not the
  still-unfinished generator factory/helper integration.
- Existing pinned CPython thread/code layout prefixes are extended to read
  tracing, profiling, global monitoring, and code-local monitoring state.
  Depending on these verified pinned internals is allowed, but production
  helper/factory wiring is now saved in all four approved production files.
  The complete four-file implementation now compiles, and the real
  stock-versus-SOAC generator-name identity integration subsequently
  **passes 1 / 0.67 seconds**.
- Source-level implementation now includes exact canonical runtime-helper
  metadata, original code, and compile-session guards; interned cached-key
  helper-global/class checks without allocations; trace/profile plus global
  **and code-local** monitoring observer checks; direct raw-slot preserved-
  state capsule construction and generator-class call; and actual original
  `PyFunction` name/qualified-name pointer identities. An independent
  source review has not found a concrete defect. The complete production
  path now compiles and the genuine transformed integration turns
  **RED-to-GREEN, 1 passed in 0.67 seconds**; subsequent focused and full
  correctness gates plus representative benchmarks also pass.
- Real transformed generator RED-to-GREEN: all **four** plain/captured
  generator name/qualified-name object identities now match stock CPython;
  the existing zero-temporary-tuple preserved-state path is exercised. The
  frozen integration also preserves cached helper replacement **before first
  call**, current helper globals/generator class, local plus global
  monitoring, profiling, tracing, lazy execution, `send`/`throw`/`close`,
  independent captures, and finalizers. If the live function name no longer
  matches its original code metadata, the implementation conservatively
  retains the prior compiler-name fallback path rather than silently
  changing renamed-function behavior.
- The complete affected JIT Rust library now **passes 560 / 560** tests.
  A grouped transformed-runtime semantic suite also reports **35 passed and
  1 expected existing xfail**, covering generator identity/local-global
  monitoring, prior source-function watcher behavior, original code/default
  mutations, synthetic factories, generator resume/default/throw/cleanup,
  async behavior, and retained late-owner/fused/indexed optimizations.
  The complete Cargo test-target run also **passes 560 / 560**, and the
  interpreter-aligned Cargo test-target check plus package-scoped formatting
  and format checks all **pass**. Production is frozen in exactly the four
  approved files; no public API was added. Representative normal and
  three-round benchmarks are complete; the authoritative full correctness
  gate also passes **87 / 87 isolated Python batches**.
- Release debug-single smoke comparison **040613** completes all **8 / 8
  workloads** without errors and preserves **3,069 optimized typed blocks /
  218 functions**. Mode-matched comparison against preceding
  source-materialization smoke **031917** confirms exactly unchanged
  generated Apply code across all eight workloads:
  **2,314,724 native bytes / 153,612 machine blocks**. Default smoke logs do
  not contain DEBUG-level direct-generator events; focused transformed
  integration, not absent default logs, establishes direct-path behavior.
  Single-loop smoke timings are cold/unusable as throughput evidence. The
  normally sampled fixed-eight comparison **040730** subsequently completes
  **8 / 8**; its results are below.
- Normally sampled fixed-eight comparison **040730** improves
  `comprehensions` robust median **66.3955 → 63.9919 us (1.03756x)**, with
  approximate **95% interval 1.015–1.083x**; its prior-SOAC mean ratio is
  **1.03984x**. The full-eight arithmetic stock score is
  **0.5099697650277614x**, versus integrated baseline
  **0.5132971537493283x**; arithmetic previous-SOAC ratio is
  **0.9912586741386916x**, while robust previous-SOAC geometric ratio is
  **1.006422x**, paired-stock robust score **0.520038x**, and
  stock-adjusted previous robust ratio **1.018791x**. The full-suite
  **1.10x** stock target remains unmet.
- Other robust prior/candidate ratios are `chaos` **0.9886x**,
  `deltablue` **1.0542x**, `fannkuch` **1.0394x**, `float` **0.9992x**,
  `nbody` **0.9766x**, `richards` **0.9980x**, and `spectral_norm`
  **0.9616x**. Paired stock `spectral_norm` also changes approximately
  **0.9635x**, so shared environmental drift is possible but not proven.
  Native generated code is unchanged across all eight workloads.
- Targeted, three-round comparison **041126** covers `chaos`,
  `comprehensions`, `nbody`, and `spectral_norm`. Across **60 candidate
  values**, comprehensions median improves **66.3955 → 62.1283 us**, or
  **1.068684x**; cluster-bootstrap **95% interval 1.032661–1.107134x** and
  paired-stock-adjusted improvement **1.121045x**. Guardrail ratios are
  `nbody` **0.996094x**, `chaos` **0.981125x** with an interval including
  one, and `spectral_norm` **0.979251x** raw / **0.980119x**
  stock-adjusted, with interval **0.9367–1.0031x**. Neither `nbody` nor
  `spectral_norm` contains an eligible generator expression, and their
  generated native code is unchanged. Robust subset geometric improvement
  is **1.005639x**, stock-adjusted **1.021786x**; arithmetic subset ratio
  is **0.9951655705x**. These results support retaining the targeted
  improvement, not claiming a substantial full-suite gain.
- Matched **50,000-loop**, zero-loss native profiles contain **849 baseline
  versus 844 candidate samples**. Inclusive Python helper-bridge share
  falls **15.425% → 0%**, preserved-state PyO3 bridge **4.003% → 0%**, and
  generator-factory share **18.133% → 16.357%**. The remaining direct-class
  ancestry includes `_PyObject_MakeTpCall` **12.09%** and slot
  initialization **10.432%**, but the latter already includes **7.826%**
  overlapping periodic garbage collection: actual non-GC slot-init share is
  approximately **2.606%**, and non-GC evaluation approximately **0.592%**.
  The prior factory includes **6.006%** GC; subtracting overlapping GC
  yields productive factory shares **12.127% → 8.531%**, a reduction of
  **3.596 percentage points**. Shutdown GC **8.124% → 8.779%** is separate.
  All stack percentages are inclusive/overlapping and cannot be summed.
  The old initializer may compile in Profile mode, but no old initializer
  handle executes in measured Apply workers. The full correctness gate
  subsequently passes.

## Implementation and compatibility

- Proposed shape: for a canonical original-code generator factory only,
  validate trusted helper metadata, the original Python code object, and the
  active compile/session identity before any first-call wrapper executes;
  directly initialize the existing trusted preserved-state capsule and pass
  the actual original `PyFunction` name/qualified-name pointers into a real
  `ClosureGenerator`. Reuse existing compiler/runtime ownership structures;
  do not add process-global mutable caches or share generator instances.
- Exactly four production surfaces are approved if the remaining structured
  RED justifies implementation:
  `crates/soac_jit/src/lib.rs`,
  `crates/soac_jit/src/preserved_state.rs`,
  `crates/soac_jit/src/jit/runtime_context.rs`, and
  `crates/soac_jit/src/jit/mod.rs`. A dedicated transformed-runtime
  integration test now exists and genuinely fails as described above. No
  production implementation is now underway only after both genuine REDs.
  The private tuple-free state regression now **passes 1 / 559 filtered**;
  all four production files compile with the proposed helper/factory wiring,
  and the real candidate integration now **passes 1 / 0.67 seconds**.
- Guard the exact original source-backed generator kind, current live
  `soac.runtime` helper globals, trusted registered factory/helper identity,
  current actual original code pointer, and matching compilation/session
  state. Revalidate assumptions for each eligible instance; replacement,
  rebinding, metadata mismatch, unsupported code kind, stale sessions, or an
  unavailable ready direct entry must use the unchanged complete Python
  factory/helper path.
- Preserve full fallback when `sys.settrace`, `sys.setprofile`, global
  `sys.monitoring` events, or code-local `sys.monitoring` events make the
  Python helper/interpreter execution observable. Local monitoring must be
  checked explicitly, not inferred from global event state; callbacks,
  ordering, and exception propagation remain CPython-visible.
- Preserve a **fresh Python function, fresh closure cells, and fresh generator
  object for every evaluation**; exact name/qualified-name object identity;
  lazy body startup; argument binding; current globals/builtins;
  independent captured values; `send`, `throw`, `close`, and finalization;
  preserved-state ownership, traversal, cancellation, reference cleanup, and
  reentrant destruction. Constructor/factory mutation before first call
  must remain visible and must not be bypassed by cached trusted metadata.
- No broad named-generator, coroutine, async-generator, or async-comprehension
  exceptions are justified by the current evidence. Unsupported classes,
  custom generator factories, dynamic helpers, monitoring, or uncertain
  preserved-state layouts retain their existing fallback.
- Depending on verified internals of the exact pinned CPython or adjusting
  vendored CPython is allowed when user-visible behavior is preserved;
  explicit C-layout ownership and proven ABI facts remain mandatory.
- Guard lifetime: all trusted factory/helper/code/session/name identities and
  global/local tracing state must still match **at each generator creation**.
  No positive guard is permanent, and no strong cache may retain user
  functions/generators beyond their existing lifetime.
- Focused regressions: unchanged-production generator
  name/qualified-name identity plus monitoring/factory-mutation integration
  genuinely **fails 1 / 0.63 seconds**, with all existing mutation,
  monitoring, tracing, and generator semantics otherwise passing. The
  independent structured preserved-state raw-slot/capsule regression
  genuinely failed **1 / 559 filtered**, then **passes 1 / 559 filtered**
  after direct owned-object/unboxed-`i64` construction and destruction/
  abandoned-builder cleanup. Approved four-file production helper/factory
  wiring compiles, and the genuine real generator identity/monitoring/
  helper-mutation integration now **passes 1 / 0.67 seconds**. Broad Rust
  Rust library and complete Cargo test targets each pass **560 / 560**, and
  grouped transformed Python reports **35 passed / 1 expected existing
  xfail**. The aligned Cargo `--tests` check and scoped formatting/checks
  pass. Normal and three-round performance evidence supports retention;
  authoritative full correctness validation also **passes**.

## Benchmark protocol and coverage

- Fixed benchmark selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`. The
  candidate must preserve all retained source-backed watcher/default/cache,
  late-owner scalar, fused-float, fixed-unpack, and shutdown guardrails.
- Compare normally sampled Apply runs against the same vendored stock
  CPython and the immediately prior integrated SOAC revision. Independently
  regenerate source-keyed profile evidence; use identical VM/resources,
  benchmark selection, module policy, and stock pairing. A final broad
  claim requires at least three independently started, order-alternating
  rounds according to `OPT_GOAL.md`.
- Baseline artifact:
  `work/pyperformance/comparison-20260819-032221-30yRwR/summary.json`.
- Previous focused three-round source-materialization evidence:
  `work/pyperformance/comparison-20260819-032719-IMdv93/summary.json`.
  Those results are integrated-baseline history, not a direct-generator
  candidate outcome.
- Baseline project coverage: benchmark `__main__` and compiler-owned
  `soac.runtime`; no transformed standard-library modules. Fixed-eight
  compiled-function counts are **34 / 21 / 78 / 1 / 9 / 8 / 53 / 9**,
  respectively. Baseline completion is **8 / 8**; candidate release
  debug-single smoke also completes **8 / 8** with unchanged generated code.
  The normally sampled fixed-eight candidate also completes **8 / 8**; its
  full aggregate is approximately neutral; three-round guardrails do not
  establish regressions in workloads without eligible generator expressions.
- Current integrated generated code is **23,359,400 native bytes /
  1,549,290 machine blocks**, with **3,069 optimized typed blocks / 218
  functions**. Candidate normal Apply generated bytes, machine blocks, and
  typed coverage remain exactly unchanged across all eight workloads.

## Measurements

| Metric | Integrated source-materialization baseline | Candidate | Change |
| --- | --- | --- | --- |
| Completed fixed-eight benchmarks | 8 / 8 | normally sampled 8 / 8 | complete |
| Fixed-eight paired stock / SOAC geometric ratio | 0.5132971537493283x | 0.5099697650277614x | stock 1.10x goal unmet |
| Fixed-eight `comprehensions` SOAC median | 66.3955 us | 63.9919 us | 1.037561x |
| Three-round `comprehensions` SOAC median | 66.3955 us | 62.1283 us | 1.068684x; clustered 95% interval 1.032661–1.107134x |
| Fixed-eight previous-SOAC robust / arithmetic ratio | integrated baseline | 1.006422x / 0.9912586741386916x | near neutral overall |
| Three-round subset robust / paired-stock-adjusted ratio | integrated baseline | 1.005639x / 1.021786x | guardrails statistically neutral or inconclusive |
| Optimized typed-IR blocks / functions | 3,069 / 218 | 3,069 / 218 | unchanged |
| Pre-optimization serialized BlockPy bytes | 14,398,752 | not independently recomputed | unavailable |
| Apply-mode native code bytes | 23,359,400 | 23,359,400 | unchanged |
| Apply-mode machine blocks | 1,549,290 | 1,549,290 | unchanged |
| Matched native CPU-clock samples / lost | 849 / 0 lost | 844 / 0 lost | zero-loss, 50,000 loops |
| Inclusive generator-factory vectorcall | 18.133%; overlapping | 16.357%; overlapping | productive non-GC 12.127% → 8.531% |
| Inclusive Python runtime helper bridge | 15.425%; overlapping | 0% | measured Apply bridge eliminated |
| Inclusive state-preservation PyO3 bridge | 4.003%; overlapping | 0% | measured Apply bridge eliminated |
| Inclusive periodic factory GC | 6.006%; overlapping | 7.826%; overlapping | included in candidate slot-init 10.432% |
| Genuine generator identity / monitoring integration RED | 1 failed / 0.63 s; stock four TRUE versus SOAC four FALSE | 1 passed / 0.67 s; all four TRUE | genuine RED-to-GREEN |
| Structured trusted-state / guard regression | 0 passed / 1 failed / 559 filtered | 1 passed / 559 filtered; raw owned object + i64, destruction/abandon cleanup | genuine RED-to-GREEN |
| Complete affected JIT Rust library | 559 previous tests | 560 / 560 passed | GREEN |
| Complete affected Cargo JIT test targets | 559 previous tests | 560 / 560 passed | GREEN |
| Grouped transformed semantic runtime suite | existing guardrails | 35 passed; 1 expected existing xfail | GREEN |
| Aligned Cargo test-target check / scoped format checks | existing gates | all passed | GREEN |
| Mode-matched release debug-single native bytes / machine blocks | 2,314,724 / 153,612 | 2,314,724 / 153,612 | unchanged |
| Full `just test-all` correctness gate | existing integrated gate | 1,220 nodeids; 87 / 87 batches passed | GREEN; eight workers |

The final authoritative log is
`work/logs/direct-generator-state-test-all.log`. `just test-all` passes all
**1,220 Python nodeids across 87 / 87 file-isolated batches and eight
workers**, with zero failed batches. Rust suites pass: JIT **560**, typed IR
**53**, lowering **371**, optimizer **208**, and PyO3 **8**. Runtime test
build takes **20.539 seconds**, Cargo tests **60.073 seconds**, inner pytest
**93.912 seconds**, outer pytest **93.926 seconds**, and the complete test
phase **154.011 seconds**. The known counter-dump batch accounts for
**93.25 seconds**.

## Attempt history

### Attempt 1: identify trusted original-generator state boundary

- Change: existing profile/source analysis, strategy documentation, and a
  genuinely failing new transformed-runtime generator identity regression;
  a second independent structured preserved-state test also genuinely
  fails; implementation in the four approved production files is now
  underway.
- Measurements and coverage: current integrated fixed-eight and matched
  **849-sample zero-loss** comprehensions profile only. No new candidate
  performance, generated-code, candidate structured GREEN, or full-suite result
  exists.
- Compatibility and tests: exact original generator code/name identity,
  trusted helper/session, fresh state/cells, lazy/send/throw/close behavior,
  preserved-state cleanup, helper monkeypatching, and global plus local
  monitoring fallback already pass. The genuine baseline identity regression
  **fails 1 / 0.63 seconds** because all four plain/captured name and
  qualified-name identity checks are false versus stock true. The flattened
  JSON event contract is corrected. Independent structured
  `compiler_owned_preserved_state_initializes_raw_slots_without_python_tuples`
  also genuinely **fails 1 / 559 filtered** because object/scalar state
  could not construct a tuple-free capsule. The exact structured regression
  now **passes 1 / 559 filtered**, proving raw owned-object/unboxed-`i64`
  state plus capsule and abandoned-builder cleanup. Pinned thread/code
  prefixes now expose trace/profile/global/local monitoring checks. Exact
  helper/original-code/session, interned cached-key globals/class, direct
  capsule/class call, and original name/qualified-name pointer wiring now
  exist in all four approved files; independent source audit reports no
  concrete defect. The complete four-file production path now compiles and
  the genuine unchanged-production generator identity integration turns
  **GREEN 1 / 0.67 seconds**, preserving pre-first-call helper replacement,
  live globals/class, local/global monitoring, trace/profile, lazy
  send/throw/close, independent cells, and finalizers. Renamed functions
  conservatively retain the old compiler-name fallback. The full JIT Rust
  library now **passes 560 / 560**, and grouped transformed runtime checks
  report **35 passed / 1 expected existing xfail** across generator,
  monitoring, mutation, async, and prior optimization guardrails. Complete
  Cargo test targets also pass **560 / 560**; interpreter-aligned Cargo
  `--tests`, scoped Rust formatting, and scoped format checks all pass.
  Production is frozen to the four approved files and adds no public API.
  Release debug-single smoke **040613** subsequently completes **8 / 8**,
  with unchanged **2,314,724 native bytes / 153,612 blocks** versus the
  mode-matched **031917** source-backed baseline. Smoke timings are
  cold-contaminated; default logs omit direct-path DEBUG events, while the
  focused integration proves the path. Normally sampled comparison
  **040730** subsequently completes **8 / 8**: comprehensions robust median
  improves **1.03756x**, but full-eight robust previous-SOAC ratio is only
  **1.00642x** and the arithmetic aggregate approximately **0.99x**. Nbody
  and spectral guardrails appear slower, with simultaneous stock spectral
  movement. Targeted three-round comparison **041126** confirms
  comprehensions **1.068684x** improvement, 95% interval
  **1.032661–1.107134x**, while unrelated controls are neutral or
  inconclusive; robust subset improvement is **1.005639x**, or
  **1.021786x** stock-adjusted. Matched zero-loss **849 → 844-sample**
  profiles remove the old helper and preserved-state bridges; generated
  native bytes, machine blocks, and typed coverage remain unchanged.
  Productive factory attribution explicitly excludes overlapping periodic
  GC. The authoritative full correctness gate passes **1,220 Python
  nodeids / 87 isolated batches**, plus every Rust suite.
- Result: **RETAIN; genuine generator/state RED-to-GREEN, complete JIT
  library/test targets 560 / 560, grouped transformed Python 35 passed / 1
  expected xfail, fixed-eight 8 / 8, targeted three-round comprehensions
  1.068684x, unchanged generated code; full correctness gate GREEN**.
- Reason: the interpreted factory bridge is substantial but its nested
  samples overlap; the theoretical **1.2215x** ceiling and realistic
  **5–12%** target must not be confused with measured candidate benefit.

## Verdict and next action

- Verdict: **LANDED / RETAIN; FULL CORRECTNESS GATE PASSED**. Genuine generator identity and
  independent tuple-free preserved-state regressions both turn GREEN, but
  the complete JIT library passes **560 / 560** and grouped Python passes
  **35 cases plus 1 expected xfail**. Complete JIT test targets and aligned
  Cargo/format checks also pass. Release debug-single smoke completes
  **8 / 8** with unchanged generated code; normal fixed-eight sampling shows
  **1.037561x** comprehensions improvement and **1.006422x** full-eight
  robust improvement. Three independent affected-workload rounds confirm
  **1.068684x** comprehensions improvement, interval
  **1.032661–1.107134x**, with neutral/inconclusive unrelated guardrails
  and unchanged generated code. Zero-loss profiles confirm elimination of
  the interpreted helper bridge while exposing overlapping GC in the
  remaining direct-class initialization. The stock **1.10x** goal is not
  met; the authoritative full correctness gate passes **87 / 87 batches**.
- Transferable lesson: eliminating an interpreter bridge is safe only when
  it preserves every observable tracing/monitoring callback, dynamic helper
  binding, original-code identity, generator lifetime, and fresh state.
- Next action: integrate the validated retained change. Any future
  class-construction optimization must separate productive initialization
  from overlapping periodic GC; the stock **1.10x** objective remains unmet.
