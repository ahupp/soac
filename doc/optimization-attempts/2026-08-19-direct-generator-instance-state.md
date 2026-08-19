---
title: "Direct Trusted Generator-Instance Preserved State"
---

# Direct generator-instance preserved state

- Status: **ATTEMPT 1 LANDED / RETAINED; ATTEMPT 2 LANDED CANDIDATE /
  RETAIN FOR PROVEN CPYTHON GC PARITY WITH NEUTRAL REPEATED
  PERFORMANCE; FULL GATE GREEN**. Attempt 1's normal fixed-eight,
  three-round guardrails,
  zero-loss native profile, and full correctness gate remain verified.
  Attempt 2's unchanged-production stock-versus-transformed GC-cycle
  mismatch now has genuine stock-parity AND independent pinned-CPython
  structured RED-to-GREEN results; its frozen two-file implementation
  passes complete JIT **571 / 571**, expanded transformed **52 / 52**, and
  scoped formatting/checks. Release smoke, normal and repeated
  measurements, and profile comparison are complete; its authoritative full
  correctness gate passes **1,231 Python nodeids / 94 isolated batches**
  and all workspace Rust suites.
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

## Attempt 2: GC-visible packed state and guarded direct generator allocation

- Status: **LANDED CANDIDATE / RETAIN FOR REAL CPYTHON GC / FINALIZER
  PARITY; real
  unchanged-production GC-cycle
  mismatch CONFIRMED by focused diagnostic and genuine formal
  stock-parity correctness AND independent actual pinned-CPython
  structured RED-to-GREEN; two-file implementation saves real cycle;
  full JIT 571 / 571, expanded transformed 52 / 52, and scoped checks
  GREEN; release smoke and normal / repeated measurements complete;
  three-round target PERFORMANCE NEUTRAL; full correctness gate GREEN**.
  Attempt 1 above remains landed and
  retained, with its original architecture, measurements, verdict, and
  compatibility history preserved.
- Integrated baseline: retained change **`rukzksko`**, commit
  **`2a9e3ca8`**. New candidate change **`mzvpmvzo`** is initially observed
  at mutable working commit **`58d8e600`**; future snapshots change that
  commit identifier.
- Current normally sampled fixed-eight baseline
  **`comparison-20260819-141233`** has stock score
  **0.6146084338507914x**, ordinary generated coverage **23,188,640 native
  bytes / 1,527,950 machine blocks**, and optimized typed coverage
  **2,866 blocks / 204 functions**. Current targeted three-round baseline
  **`comparison-20260819-141538`** has stock score
  **0.4625866444625596x** and per-round coverage **18,255,240 native
  bytes / 1,201,600 machine blocks / 2,265 typed blocks / 183 functions**.
  Neither result satisfies the full-suite stock **1.10x** objective.

### Current profile evidence and confirmed compatibility mismatch

- Historical comprehensions profile from
  **`comparison-20260819-131748`** contains **570 zero-loss samples**.
  It predates the intervening retained exact-positional-trampoline change
  and therefore is **not a matched deep profile of the immediate
  `141233` baseline**; comprehensions was neutral across that intervening
  change, so the profile provides qualitative source-hotspot evidence only.
  Generator factory ancestry is **20.179% inclusive**, or
  approximately **14.739%** after excluding overlapping periodic GC.
  Nested canonical class `type_call` contributes approximately
  **9.65–11.58%**, Python `__init__` **9.477%**, interpreted evaluation
  **6.494%**, preserved-state builder **4.213%**, and capsule work
  **1.755%**. These call-stack shares overlap; even sequential builder /
  capsule operations can appear under the same ancestors, so they must not
  be summed or represented as an expected speedup.
- **Confirmed user-visible CPython compatibility mismatch:** the actual
  retained transformed runtime stores Python references inside a
  GC-untracked preserved-state `PyCapsule`. The first focused same-process
  diagnostic **PASSES in 0.38 seconds** by intentionally asserting the
  existing difference: stock reports **`weakref_collected=True`**, no
  preserved-state capsule, and **`finalizers=['released']`**, whereas SOAC
  reports **`weakref_collected=False`**,
  **`gc.is_tracked(preserved_capsule)=False`**, and **`finalizers=[]`**.
  The test explicitly breaks its temporary iterator → generator → capsule
  → iterator cycle afterward. This proves a real existing GC/finalizer
  mismatch on the actual runtime, not merely a source-level hypothesis.
  Pinned CPython `capsule.c` identifies the traversal boundary and the
  existing **`_PyCapsule_SetTraverse`** export was host-verified. The
  expanded real stock-parity integration subsequently verifies genuine
  unchanged-production **RED: 1 failed in 6.10 seconds**, then candidate
  **GREEN: 1 passed in 5.93 seconds** after the bounded two-file fix.
- Expanded real **Profile → Verify → Apply** investigation establishes an
  existing mode-specific constructor-monitoring nuance: code-local
  monitoring of **`ClosureGenerator.__init__`** emits **no `PY_START` in
  Profile mode**, but does emit the observable callback in **Apply mode**.
  The final expanded regression correctly requires the existing Apply
  behavior only rather than asserting that callback in every mode. Thus an
  unconditional init-local-monitoring bug claim is **refuted**. Candidate
  direct allocation must fall back when Apply-mode
  initializer monitoring, global monitoring, tracing, profiling,
  replacement, or another observable hook requires the original path.

### Bounded two-file candidate architecture

- Approved production scope, authorized only after the genuine
  unchanged-production correctness RED, is exactly two existing files:
  `crates/soac_jit/src/preserved_state.rs` and
  `crates/soac_jit/src/lib.rs`. Final implementations in both files are
  **FROZEN** after the two genuine unchanged-production REDs. No new public
  API, global state, runtime helper, IR node, or separate strategy record
  is added. The existing successful generator event receives one **new**
  **`constructor_path`** field with **`direct_slots`** or
  **`python_class`**; no separate event or helper is introduced.
- In existing private preserved-state ownership, use one checked contiguous
  allocation for raw **`u64`** value slots plus a packed object-ownership
  bitset. The private **24-byte** state uses one checked `Vec` allocation
  for values plus multiword bitmap through both existing public/direct
  construction paths. Preserve scalar representation, slot order, exact
  bounds and reserve/overflow checks, object-reference ownership,
  partial-construction RAII, idempotent destruction, and every existing
  state-load error.
- Make Python object roots visible to CPython cycle collection through
  the actual live existing **`_PyCapsule_SetTraverse`** export: track only
  owned Python/cell slots, traverse the exact marked objects, and exclude
  scalar values that resemble object pointers. Clear the **object payload
  slot before its decrement**, preserving its immutable kind bitmap, using
  **`Py_CLEAR`** ordering without holding a Rust borrow across a reentrant
  finalizer; keep repeated
  clear/destructor paths idempotent. Independent source review reports the
  preserved-state implementation clean; the actual stock-parity
  integration independently verifies cycle collection and finalization.
- In the existing generator-factory entry, admit only the canonical exact
  generator class and perform existing `GenericAlloc` plus **eight checked
  slot initializations**. Guard the original live `__init__` function/code,
  class/type version, initializer vectorcall, `__new__`, allocator,
  owner/session identity, descriptors/hooks, recursion, and all
  local/global monitoring / tracing / profiling observers. Initialize a
  real fresh generator instance with source
  function/watchers, exact names, existing captures, preserved state, and
  normal finalizer/lifetime visibility; never fabricate a non-generator or
  bypass source-Python function creation.
- On every mismatch, mutation, custom class/new/allocator/descriptor hook,
  monitor, reentrant action, forced-interpreter condition, or unsafe slot
  shape, retain the entire original factory/class-call behavior. Preserve
  laziness, `send` / `throw` / `close`, independent closures, subclass
  behavior, GC cycles, exceptional partial initialization, and exact
  decref/finalizer sequencing. Existing previous retained generator
  optimizations and normal emitted native-body coverage remain guardrails,
  not already-verified candidate results.

### Required regressions, measurements, and verdict

- Reviewer-owned new minimal integration
  **`tests/test_generator_state_gc_and_direct_initialization.py`** is saved.
  It uses real same-process stock-versus-transformed weak references to
  discriminate an iterator → generator → preserved-state capsule → iterator
  cycle. Its initial unchanged-production diagnostic **PASSES in 0.38
  seconds** because it asserts the confirmed existing mismatch. The
  expanded unchanged-production stock-parity test then produces a genuine
  **correctness RED: 1 failed in 6.10 seconds**. Its sole final reached
  failure is stock **`{'collected': True, 'tracked': None, 'finalizers':
  ['released']}`** versus transformed **`{'collected': False, 'tracked':
  False, 'finalizers': []}`**.
- Before that intended parity failure, all real same-process
  **Profile → Verify → Apply** controls pass: source-function watcher;
  **70 alternating object / `i64`** preserved slots and GC referents;
  closure/name/code identity; laziness and `send` / `throw` / `close`;
  actual init code-local monitoring in **Apply** but not **Profile**;
  initializer-function / `__init__.__code__` / `setattr` / class / `new`
  mutation guards; forced mode; resurrection and exactly-once finalizers;
  profile counters and generated native coverage. Restoring the canonical
  class's `__new__` would mutate its pinned type, so the mutation control
  uses a replacement subclass instead. An additional existing-event
  `direct_slots` structural assertion follows parity and is therefore **not
  reached** on the unchanged baseline. Production was unchanged when this
  genuine RED was recorded.
- Independent genuine unchanged-production structured Rust **RED**:
  **`compact_preserved_state_tracks_owned_objects_and_cells_across_bitmap_words`**
  constructs a real pinned-CPython preserved-state capsule containing
  **130 mixed object / cell / scalar slots**. Actual
  **`PyObject_GC_IsTracked`** returns **`0`** instead of required **`1`**;
  the focused test reports **0 passed / 1 failed / 570 filtered**.
  Compilation took **24.33 seconds** as one-time workflow overhead, not
  regression runtime or performance evidence.
- Its first draft incorrectly assumed host-cached PyO3 exposed
  **`ffi::PyCell_New`**, but the actual pinned guest git PyO3 does not.
  The architect corrected the test to reuse existing private
  **`crate::PyCell_New`** without adding any helper, file, or public API;
  the discarded compilation error was not counted as a genuine RED.
- The same actual pinned-runtime structured regression now verifies
  genuine **RED → GREEN: 1 passed / 570 filtered**. Its **130** mixed
  slots prove live capsule GC tracking, exact object/cell traversal with
  scalar pointer-lookalikes excluded, multiword bitmap boundaries
  **63 / 64 / 127 / 128**, visitor early-stop at **37**, contiguous raw
  value pointers, exact strong-reference handling, clear-before-decrement
  without a Rust borrow across finalization, and idempotent double clear.
- The frozen actual transformed stock-parity integration also verifies
  genuine user-visible **RED → GREEN: 1 passed in 5.93 seconds** across
  real **Profile → Verify → Apply** execution. Stock and SOAC both collect
  the iterator/generator cycle and finalize once; the transformed capsule
  is GC-tracked with exact **70-slot object / cell / scalar** traversal.
  Source-function watchers, constructor init code-local monitoring in
  Apply, live class / `new` / `setattr` / initializer-code changes,
  laziness and `send` / `throw` / `close`, resurrection, counters, emitted
  native coverage, and the new `constructor_path=direct_slots` field on
  the existing successful generator event all pass.
- Both authorized production implementations are saved, including
  canonical generator `GenericAlloc` / **eight checked slots** in
  `lib.rs`; both real structured and stock-parity regressions turn
  **RED → GREEN**. Fresh post-format complete JIT library passes
  **571 / 571 in 5.75 seconds**, and all Cargo test targets independently
  pass **571 / 571 in 5.63 seconds**. The expanded transformed generator /
  watcher / monitoring / mutation / finalizer matrix passes **52 / 52 tests
  across 20 files in 30.07 seconds**, with **no xfails**. Scoped
  `just fmt-rust soac_jit`, `just fmt-rust-check soac_jit`, and
  `cargo check -p soac_jit --tests` all pass; the aligned test-target check
  takes **2.53 seconds**. The authoritative full `just test-all` gate also
  **PASSES**; complete counts and timings are recorded below.
- Release debug-single smoke comparison **150319** completes **8 / 8**,
  with **zero errors** and all **397** measured Apply direct-function /
  adapter rows unchanged. Ordinary native code remains exactly
  **2,242,168 bytes / 148,116 machine blocks**, optimized typed coverage
  **2,866 blocks / 204 functions**, pre-optimization BlockPy
  **7,199,376 bytes**, and existing hidden trampolines **36,500 bytes**.
  The direct event is DEBUG-only and absent from INFO smoke logs; the
  focused structural integration independently proves the actual path.
  Cold smoke timings are not throughput evidence.
- Normal fixed-eight comparison **150451** reports stock score
  **0.6249286764762751x** versus retained **0.6146084338507914x** and
  previous-SOAC arithmetic **1.0003747535524583x**; robust previous
  geometry is **1.004336x / 1.012863x paired-stock**. Comprehensions
  initially improves **45.282469 → 43.628377 us (1.037913x; 95%
  1.004673–1.071342)** / paired **1.045783x [1.015075, 1.082059]**, but
  that single-round result **does not reproduce** in the authoritative
  repeated comparison. All **80** normal PIDs retain **3,970** exact body /
  adapter rows, **23,188,640 bytes / 1,527,950 machine blocks / 2,866
  typed blocks / 204 functions**, unchanged hidden trampolines
  **365,000 bytes**, and zero errors.
- Final clean three-round comparison **150805** against retained **141538**
  proves target performance **NEUTRAL**, not improved: comprehensions
  **44.923821 → 45.000817 us**, **0.998289x [0.979545, 1.019565]** /
  stock-adjusted **1.001340x [0.982678, 1.026006]**, with individual raw
  rounds **0.98224x / 1.034504x / 0.98554x**. Chaos is **1.011097x** /
  paired **1.017143x**; deltablue **1.000035x** / paired **0.983673x**;
  richards **1.021804x [1.003291, 1.033660]** but paired
  **1.006983x** with its interval crossing neutral. Robust four-workload
  geometry is **1.007762x / 1.002211x paired**. Official targeted stock
  score is **0.4619152255075415x** versus retained
  **0.4625866444625596x**, with previous-SOAC arithmetic
  **1.0117878326581697x**; none establishes a target speedup.
- All **120** repeated Apply PIDs retain exactly **54,765,720 ordinary
  native bytes / 3,604,800 machine blocks** across three rounds, or
  **18,255,240 bytes / 1,201,600 machine blocks / 2,265 typed blocks / 183
  functions** per round; all hidden trampolines remain exactly
  **746,520 bytes**, with no errors or emitted-body changes.
- Qualitative zero-loss comparison uses **557 candidate samples** against
  **570 historical** samples from earlier **131748**, with the retained
  exact-positional revision between them; this is **not a matched
  immediate-baseline causal comparison**. Within generator-factory ancestry,
  old `_PyObject_MakeTpCall` **10.178% → 0%**, `type_call`
  **9.652% → 0%**, initializer **9.477% → 0%**, and interpreted init eval
  **6.493% → 0%**; direct-factory ancestry falls
  **18.776% → 8.259%**. Correctness introduces GC-visible capsule
  traversal **0% → 6.820% inclusive / 3.049% self**, while whole GC
  ancestry changes **14.040% → 23.868%**. Shares are nested, overlapping,
  and from different revisions; do not sum them or infer a speedup.
  Source function instantiation remains visible
  **10.873% → 12.210%**. The repeated target remains neutral despite
  removing interpreted initialization.
- Authoritative full `just test-all` **exits zero**; see
  **`work/logs/direct-generator-instance-state-compact-test-all.log`**.
  All **1,231 Python nodeids / 94 isolated batches / 8 workers** pass
  (**94 passed / 0 failed**). Workspace Rust suites pass JIT **571**,
  lowering **371**, optimizer **213**, typed IR **54**, and PyO3 **8**.
  Runtime build takes **1.592 seconds**, Cargo tests **65.950 seconds**,
  inner / outer parallel pytest **79.423 / 79.439 seconds**, and the
  complete test phase **145.402 seconds**. The new genuine GC-cycle
  integration passes in **8.31 seconds**; the existing **28-node**
  counter-dump batch takes **79.41 seconds**.
- Current verdict: **ATTEMPT 2 LANDED CANDIDATE / RETAIN; Attempt 1
  remains RETAINED; FULL CORRECTNESS GATE GREEN**. Retain because a
  genuine stock-visible
  GC/finalizer bug is fixed, both real regressions turn RED-to-GREEN, and
  three-round throughput / ordinary native bodies / hidden trampolines
  remain neutral or unchanged. Do **not** claim an optimization speedup;
  the authoritative full `just test-all` gate passes all **1,231 Python
  nodeids / 94 batches** and every workspace Rust suite. Full-suite stock
  **1.10x** remains unmet.
