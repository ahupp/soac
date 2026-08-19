---
title: "Guarded Indexed Runtime Factory Lookup"
---

# Guarded indexed runtime factory lookup

- Status: **LANDED CANDIDATE / RETAIN; GENUINE PRODUCTION-PATH RED-TO-GREEN,
  JIT LIBRARY AND ALL TEST TARGETS 568 / 568, TRANSFORMED RUNTIME 33 / 33,
  REPEATED TARGETED BENCHMARKS AND ZERO-LOSS PROFILING COMPLETE; FULL
  CORRECTNESS GATE PASSED**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`kvpzmtlp`**, commit
  **`1ff679ff`**.
- Candidate revision: change **`zwkrytkq`**, commit **`40bf1623`**;
  one-file implementation begins only after the observed genuine RED and
  now passes focused validation, repeated targeted benchmarks, and the full
  correctness gate.
- Outcome: determine whether the actual mutable indexed runtime module's
  synthetic-code factory can be recovered through a narrowly guarded live
  dictionary slot without changing observable CPython module lookup.

## Hypothesis and evidence

- General-purpose opportunity: repeated function instantiation already owns
  `PreparedSyntheticCode` and the interned factory name, but the ordinary
  module attribute path can repeatedly allocate a fresh Python name and
  perform generic module lookup. Reusing the existing key only when live
  module/type/dictionary assumptions are proven may reduce factory overhead
  without introducing another runtime abstraction.
- Current zero-loss comprehensions baseline
  `work/logs/template-aware-registration-candidate-comprehensions_*`
  contains **707 raw recorded samples / 436 distinct aggregated weighted
  stacks**. Whole function-factory ancestry is **17.529%**, and
  `synthetic_code_for_template` is **3.2516%**. Source-backed disjoint fresh
  Python attribute-name Unicode allocation contributes approximately
  **1.415%** and generic module attribute lookup **1.131%**, for only a
  **2.546-percentage-point gross upper bound**; replacement guards reduce
  that ceiling, so a realistic whole-workload improvement may be roughly
  **1–2%**, not an established gain. Other inclusive shares overlap.
- The actual `soac.runtime` module is the mutable heap
  **`_soac_ext.IndexedModuleType`**, a direct `PyModule` subclass using a
  custom exact dictionary with pinned CPython **`dk_kind == 3`**. A
  hypothetical exact `PyModule_Type`-only shortcut would miss **100%** of
  measured hot runtime calls and is not an acceptable implementation.
- Integrated normally sampled fixed-eight comparison **082430** has stock
  geometric score **0.6028454470492562x**; targeted fixed-four comparison
  **082751** has stock score **0.4169339309251314x**. Neither subset proves
  full-suite acceptance, and the **1.10x full-suite stock** target remains
  unmet.
- Existing Apply coverage is **23,293,040 native bytes / 1,533,550 machine
  blocks**, optimized typed coverage **2,866 blocks / 204 functions**, and
  serialized pre-optimization BlockPy **14,398,752 bytes**. The proposed
  runtime lookup should preserve generated-code shape, but candidate
  invariance is not yet measured.
- No current CPython-visible correctness defect is asserted. The aim is
  safely reducing repeated work without hiding user-observable hooks,
  descriptor behavior, replacement, errors, or lookup-name identity.
- Release fixed-eight debug-single comparison **090221** completes
  **8 / 8** with zero errors; independent PID matching confirms every
  function/adapter remains unchanged, totaling **2,253,100 native bytes /
  148,734 machine blocks** and **2,866 typed blocks / 204 functions**.
  Cold single-iteration timings are not throughput evidence.
- Normally sampled fixed-eight comparison **090414** reports stock score
  **0.5883463026285985x**, below prior **0.6028454470492562x**, and
  official previous-SOAC geometry **0.9969491581827803x**. Robust previous
  geometry is approximately **0.99735x / 0.98057x stock-adjusted**. The
  apparent float control decline cannot execute this synthetic-factory
  optimization and must not be attributed to it; platform/stock drift
  remains material. All **80 measured workers** retain exactly
  **23,293,040 native bytes / 1,533,550 machine blocks** and
  **2,866 typed blocks / 204 functions**, with zero errors.
- Matched three-round comparison **090720** against integrated targeted
  baseline **082751** uses worker/round-stratified samples.
  Comprehensions improve **1.0528748x [1.022367, 1.074919]**, or
  **1.0438235x paired [1.012884, 1.068542]**, with all three raw rounds
  improving **1.04507x / 1.02135x / 1.04854x**. Chaos improves
  **1.0482367x [1.026554, 1.065812]**, or **1.0390366x paired
  [1.010680, 1.064081]**. Deltablue is paired **1.01737x** and richards
  **1.02315x**, with both confidence intervals crossing one. Four-workload
  robust geometry is **1.0403375x raw / 1.0307875x stock-adjusted**.
  All **120 candidate measured functions/workers** preserve per-round
  **18,352,680 native bytes / 1,206,840 blocks**, or
  **55,058,040 bytes / 3,620,520 blocks** across all three rounds.
- Matched zero-loss comprehensions profiles contain **707 -> 618 raw
  samples** and **436 -> 404 distinct aggregated stacks**. Synthetic-code
  factory ancestry decreases **3.2516% -> 1.7809%**; fresh Unicode-name
  allocation **1.415% -> 0%** and generic module attribute lookup
  **1.1306% -> 0%** disappear from their respective source paths. These
  descendant shares overlap their parent and must not be added to the
  overall factory reduction. Residual live-dictionary work is **0.6476%**,
  Unicode keys **0.3238%**, and helper self **0.4857%**. Garbage collection
  also declines **17.97% -> 14.40%**, confounding any direct replay
  comparison; attached replay is diagnostic only. Repeated matched
  workload medians, not replay or inclusive shares, justify retention.
- The genuine unchanged-production structured JIT regression now reports
  **0 passed / 1 failed / 567 filtered in 0.03 seconds**. Its actual
  lowered comprehension uses the exact canonical
  **`_soac_ext.IndexedModuleType`** and a real custom
  **`dk_kind == 3`** dictionary, installs and restores a fake bootstrap,
  and invokes production `synthetic_code_for_template` twice. Existing
  synthetic-code reuse, single factory invocation, and restored state all
  pass. Only the intended new cached-owner assertion fails:
  **actual owner pointer `0` versus the nonzero canonical type pointer**.
  Production behavior was unchanged before the RED; the preceding
  **27.64-second** one-time build is workflow cost, not test runtime or
  candidate performance evidence.
- The first candidate guard attempted to inspect every MRO ancestor via
  `ancestor.tp_dict`; the next focused regression still fails with cached
  owner pointer **0**. In pinned CPython, static `PyModule_Type` and
  `object` keep interpreter-owned dictionaries outside `tp_dict`, as the
  existing JIT source already documents. The attempted proof therefore
  silently rejects the real hot module and cannot establish a fast path.
  The corrected proof inspects the exact canonical **heap** owner
  dictionary only after validating its direct immutable `PyModule_Type`
  base and inherited getter; static-base dictionaries are not dereferenced
  through their null `tp_dict`.
- The original genuine production-path regression now turns
  **RED-to-GREEN: 1 passed / 567 filtered**. The actual lowered
  comprehension, canonical heap `IndexedModuleType`, custom
  **`dk_kind == 3`** dictionary, production synthetic-code cache, exact
  cached owner identity, nonzero live owner-type version, code reuse, and
  single factory invocation all pass together. This establishes the real
  indexed-module path, not an exact-base-module substitute. The same
  strengthened production-path test additionally performs a real
  watcher/version-free raw indexed factory replacement with balanced
  references; validates module-missing `__getattr__` and user-module-
  subclass `__getattribute__` with their original fresh name identities;
  confirms a GENERAL collision observes the original fresh name and that a
  raised exception is **propagated, not swallowed**; mutates the canonical
  global heap class with a data property / type-version invalidation and
  `__getattribute__`; and restores the class before final assertions.
  The entire focused structured/adversarial case passes **1 / 1**.
  Post-format complete JIT library and all Cargo test targets each pass
  **568 / 568**, including this strengthened production-path regression.
  Broad transformed-runtime compatibility passes **33 / 33 in 34.20
  seconds**, covering synthetic closure cache/audit/reentrant module swaps,
  pre/post factory mutation, synthetic/source watchers, generators, live
  function code/default mutation, forced interpreter, all five guarded
  StopIteration cases, inherited/non-self/scalar fields, fused floats, real
  `IndexedModuleType` / module `__getattr__`, direct exception cleanup, and
  constructor virtualization. Package-scoped
  `just fmt-rust-check soac_jit` and
  `cargo check -p soac_jit --all-targets` both pass. Candidate performance
  and the full correctness gate now passes.

## Implementation and compatibility

- Candidate production scope: exactly one existing file,
  `crates/soac_jit/src/function_instantiation.rs`. Reuse the existing
  `PreparedSyntheticCode` and already-interned lookup key. Production
  implementation starts only after the genuine unchanged-behavior RED and
  now passes its focused structured owner/version regression.
- Establish the cached assumption **only after** the existing initial
  factory lookup, auditing, and reentry behavior complete successfully.
  Cache the exact canonical indexed runtime module-type pointer and its
  captured **nonzero type version** only after proving that its effective
  getter inherits `PyModule.tp_getattro`, its direct base is the immutable
  static `PyModule_Type`, and the canonical heap owner's real dictionary
  supplies no factory-name descriptor or competing hook. Do not inspect
  static-base dictionaries through `tp_dict`: pinned CPython stores those
  interpreter-owned dictionaries elsewhere.
- On **every invocation**, revalidate the exact module identity and module
  type, the same nonzero type version, effective getter, and the current
  exact non-GENERAL module dictionary; require the existing factory key to
  remain **present** at its live indexed slot. Convert the borrowed live
  value to an owned reference using the correct CPython `INCREF` before
  returning; do not retain runtime modules, factories, or mutable values.
- Any missing key, GENERAL dictionary, custom subclass, class/MRO
  descriptor/property, changed `__getattribute__`, changed type/version,
  module-level `__getattr__`, replacement runtime module/factory, or changed
  raw indexed slot must fall back to the **original fresh**
  `dp.getattr(...)` path. Preserve raised exceptions, module hooks,
  user-observable lookup-name identity, finalization, refcounts, audit, and
  reentry. A raw live value change must not depend on dictionary watcher
  notifications or version increments.
- Avoid a new public API, global mutable state, runtime helper, exported
  concept, profile schema, typed-IR sidecar, or generated-code change.
  Keep all existing generator, interpreter, function mutation, and prior
  runtime specialization behavior unchanged.
- The genuine production-path structured unchanged-behavior RED now turns
  GREEN on the actual indexed runtime module and pinned custom dictionary.
  Its strengthened single test also proves raw indexed replacement with
  balanced refs, missing-module and custom-subclass fresh-name hooks,
  GENERAL collision/fresh identity and propagated exceptions, and mutable
  canonical heap-class property/getter invalidation. Complete JIT library
  and all test targets each pass **568 / 568**, and broad transformed
  coverage passes **33 / 33 in 34.20 seconds**. Scoped formatting and
  all-target Cargo checks and the full correctness gate pass.

## Benchmark protocol and coverage

- Fixed normal selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`, using the
  same vendored stock CPython and independently profiled integrated SOAC
  baseline. Use a same-selector repeated targeted comparison before
  attributing a small comprehensions change.
- Baseline artifacts: normal comparison **082430**, targeted comparison
  **082751**, and the current **707-sample / 436-stack** zero-loss
  comprehensions profile. Candidate normal comparison **090414**, targeted
  three-round comparison **090720**, and matched **618-sample / 404-stack**
  profile and the authoritative full correctness gate are complete.
- Confirm actual `soac.runtime` indexed-module specialization and retained
  source/synthetic function watchers, factory/module mutation, monitoring,
  StopIteration, generator, inherited/non-self/scalar, and constructor
  semantics before treating benchmark completion as meaningful coverage.

## Measurements

| Metric | Integrated template-aware baseline | Candidate | Change |
| --- | --- | --- | --- |
| Normal fixed-eight paired stock / SOAC geometry | 0.6028454470492562x | 0.5883463026285985x | full-workload stock score regresses; stock 1.10x goal unmet |
| Normal fixed-eight official / robust previous-SOAC geometry | integrated comparison 082430 | 0.9969491581827803x / approximately 0.99735x | stock-adjusted approximately 0.98057x; float control cannot execute source path |
| Targeted fixed-four paired stock / SOAC geometry | 0.4169339309251314x | pending | subset only; not full-suite acceptance |
| Previous-SOAC targeted robust / stock-adjusted improvement | integrated `kvpzmtlp/1ff679ff` | 1.0403375x / 1.0307875x | three-round matched subset; not full-suite acceptance |
| Targeted comprehensions raw / stock-adjusted improvement | targeted comparison 082751 | 1.0528748x / 1.0438235x | raw CI [1.022367, 1.074919]; all three rounds improve |
| Targeted chaos raw / stock-adjusted improvement | targeted comparison 082751 | 1.0482367x / 1.0390366x | raw CI [1.026554, 1.065812] |
| Optimized typed-IR blocks / functions | 2,866 / 204 | 2,866 / 204 | unchanged |
| Serialized pre-optimization BlockPy bytes | 14,398,752 | pending | pending |
| Apply-mode native code bytes / machine blocks | 23,293,040 / 1,533,550 | 23,293,040 / 1,533,550 | all 80 measured workers/functions unchanged |
| Targeted per-round native bytes / machine blocks | 18,352,680 / 1,206,840 | 18,352,680 / 1,206,840 | all 120 measured workers/functions unchanged |
| Current comprehensions zero-loss samples / distinct stacks | 707 / 436 | 618 / 404 | zero-loss; weighted stacks are not raw sample count |
| Synthetic-code factory inclusive ancestry | 3.2516% | 1.7809% | nested shares overlap; GC 17.97% -> 14.40% confounds replay |
| Fresh Unicode name allocation / generic attribute lookup | 1.415% / 1.1306% | 0% / 0% | eliminated source paths; do not add descendants to parent |
| Actual hot runtime module type / dictionary kind | `_soac_ext.IndexedModuleType` / `dk_kind == 3` | pending | exact `PyModule_Type` alone misses all hot calls |
| Genuine production-path structured regression | 0 passed / 1 failed / 567 filtered in 0.03 s; cached owner 0 instead of nonzero canonical type | 1 passed / 567 filtered | genuine RED-to-GREEN; actual indexed owner / live nonzero version / custom dictionary |
| First candidate MRO dictionary proof | static bases expose no `tp_dict` | failed intermediate cached-owner assertion; corrected heap-owner-only proof passes | preserve failed iteration; direct immutable module base and inherited getter validated |
| Adversarial class hooks / GENERAL errors / raw indexed mutation | existing generic semantics | 1 / 1 strengthened actual production-path test | raw-slot refs, fresh-name hooks, GENERAL exception propagation, mutable canonical class/version all GREEN |
| Complete post-format JIT library / all Cargo test targets | integrated template-aware baseline 567 tests | 568 / 568 and 568 / 568 passed | GREEN; includes strongest production-path / adversarial regression |
| Broad transformed-runtime compatibility | integrated template-aware baseline | 33 / 33 passed in 34.20 s | GREEN; factory mutation, watchers, generators, live code/defaults, owners, module hooks, and virtualization |
| Scoped formatting / all-target Cargo check | integrated template-aware baseline | both passed | GREEN; package-scoped JIT checks |
| Full `just test-all` correctness gate | integrated baseline previously passed | 1,227 nodeids; 90 / 90 batches; 568 JIT / 212 optimizer / 54 typed / 371 lowering / 8 PyO3 | GREEN; cargo 61.431 s, pytest 76.810 s, total 138.270 s |

## Attempt history

### Attempt 1: identify the real indexed runtime module and guarded slot

- Change: inspect the current post-registration comprehensions profile and
  the actual runtime module/type/dictionary representation before choosing
  any implementation. Limit potential production edits to the single
  existing function-instantiation file.
- Measurements and coverage: **707 raw samples / 436 aggregated stacks**;
  factory **17.529%**, synthetic code **3.2516%**, source-backed gross
  fresh-name plus generic-lookup ceiling approximately **2.546 percentage
  points**. Existing normal stock geometry is **0.6028454470492562x**;
  candidate normal stock declines to **0.5883463026285985x**, while
  repeated comprehensions and chaos improve significantly.
- Compatibility and tests: the real module is a mutable heap
  `_soac_ext.IndexedModuleType`, not exact `PyModule_Type`. Exact module /
  type/version/getter/MRO/dictionary/present-slot revalidation and the
  original fresh-name fallback are mandatory. The genuine unchanged-
  production structured RED fails exactly on missing cached owner
  **0 versus the real nonzero canonical type**, with **1 failed / 567
  filtered in 0.03 seconds**; actual lowered comprehension, custom indexed
  runtime module/dictionary, twice-called production factory, single factory
  invocation, synthetic-code reuse, and fake-bootstrap restoration all pass.
  The one-time **27.64-second** build is workflow overhead only.
  The first production guard incorrectly traverses static-base `tp_dict`;
  pinned CPython stores these dictionaries outside the static type object,
  and a second focused run still fails with cached owner **0**. Restricting
  dictionary inspection to the canonical heap owner after proving the
  direct immutable module base and inherited getter then turns the genuine
  structured regression **GREEN 1 / 567 filtered**, including exact owner,
  nonzero type version, reused code, and one factory call. The strengthened
  same production-path test additionally passes watcher/version-free raw
  factory replacement with balanced references, original fresh-name module
  hooks, GENERAL collision with propagated exceptions, canonical heap-class
  property/version/getter mutation, and class restoration. Post-format full
  JIT library and all Cargo test targets each pass **568 / 568**, and broad
  transformed compatibility passes **33 / 33 in 34.20 seconds**, and
  scoped formatting / all-target checks pass. Fixed-eight and three-round
  comparisons, matched zero-loss profiling, and the full correctness gate
  are complete.
- Result: **LANDED CANDIDATE / RETAIN; GENUINE STRUCTURED RED-TO-GREEN, SIGNIFICANT
  REPEATED COMPREHENSIONS / CHAOS IMPROVEMENT, UNCHANGED NATIVE CODE;
  FULL GATE PASSED**.
- Reason: an apparently simple exact-base-module shortcut would never
  execute on the real hotspot, while a relaxed subclass/dictionary shortcut
  could suppress observable CPython module hooks or descriptors.

## Verdict and next action

- Verdict: **LANDED CANDIDATE / RETAIN; FULL CORRECTNESS GATE PASSED**. The normal
  fixed-eight stock and previous-SOAC geometries regress to
  **0.5883463026285985x** and **0.9969491581827803x**, respectively;
  matched three-round comprehensions **1.0528748x** and chaos
  **1.0482367x** improve significantly, with native code unchanged and
  target source paths removed in zero-loss profiles. Delta/richards are
  paired-neutral, and float cannot execute the optimized source path. Full
  JIT **568 / 568**, transformed runtime **33 / 33**, and scoped checks
  pass, as does the full gate; the full-suite **1.10x stock** target
  remains unmet.
- Authoritative `just test-all` log
  `work/logs/guarded-indexed-runtime-factory-test-all.log` records
  **1,227 Python nodeids / 90 isolated file-local batches / 8 workers**,
  with **90 passed / 0 failed**. Workspace Rust suites include **568 JIT**,
  **212 optimizer**, **54 typed-IR**, **371 lowering**, and **8 PyO3**
  passing tests. Cargo takes **61.431 seconds**, pytest
  **76.810 seconds inner / 76.825 seconds outer**, and the complete test
  phase **138.270 seconds**; the known counter-dump batch reports
  **77.08 seconds**.
- Transferable lesson: identify the exact live module and pinned dictionary
  representation before designing a CPython fast path. Existing object and
  key ownership can be reused, but mutable type semantics and fresh fallback
  name identity require live guards.
- Next action: retain the fully validated change while preserving the
  candid fixed-eight regression, significant matched target gains, unchanged
  generated code, and production-path hook/raw-slot/GENERAL-error controls.
