---
title: "Polymorphic Inherited Owner-Field Specialization"
---

# Polymorphic inherited owner-field specialization

- Status: **LANDED CANDIDATE / RETAIN; NORMAL FIXED-EIGHT, TARGETED
  THREE-ROUND GAINS, ZERO-LOSS PROFILES, AND FULL CORRECTNESS GATE VERIFIED;
  MATERIAL NATIVE-CODE GROWTH DISCLOSED**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`okqlrmxm`**, commit
  **`ccef62b6`**.
- Candidate change: **`nvvlrumm`**; all six production files compile and the
  focused transformed integration, two optimizer regressions, and genuine
  typed-validator regression pass.
- Outcome: determine whether inherited methods can safely specialize hot
  instance reads against several independently guarded exact concrete
  receiver types, without assuming subclass layouts match their lexical
  defining class.

## Hypothesis and evidence

- General-purpose opportunity: inherited Python methods often read the same
  receiver fields across multiple concrete subclasses. Existing owner-field
  specialization rejects these inherited sites because the method's lexical
  owner does not establish the actual instance type or split-dictionary key
  layout. Reusing an inherited offset indiscriminately would be incorrect;
  separately validating each hot exact concrete owner could remove repeated
  generic attribute lookup while preserving Python dispatch.
- The integrated fixed-eight paired-stock geometric score is
  **0.5099697650277614x**. The authoritative full-pyperformance goal remains
  **1.10x** against the same stock CPython and is **not met**. Existing
  Apply-mode generated code totals **23,359,400 native bytes / 1,549,290
  machine blocks**, with **3,069 optimized typed-IR blocks / 218
  functions**.
- Current integrated `deltablue` native evidence is
  `work/logs/integrated-generator-baseline-deltablue_record.txt`: **434
  CPU-clock samples**, **27.225 MB**, and no lost-sample warning. Current
  integrated `richards` evidence is
  `work/logs/integrated-generator-baseline-richards_record.txt`: **599
  samples**, **37.514 MB**, and no lost-sample warning. These are accepted
  zero-loss captures; stack percentages below are inclusive and overlap.
- Generic `PyObject_GetAttr` ancestry occupies **20.736%** of `deltablue`
  and **27.714%** of `richards`. Only selected inherited self-field reads
  are plausibly avoidable: `BinaryConstraint.input` / `output` account for
  **406,784 / 1,529,855 = 26.59%** of observed generic reads, while the two
  inherited `TaskState` predicates account for
  **795,472 / 2,968,392 = 26.80%**. These fractions are workload-specific
  attribution, not additive speedup or proof of candidate selection.
- Concrete split-dictionary layouts differ materially. In `deltablue`,
  `EqualityConstraint` uses `direction=3`, `v1=1`, and `v2=2`, whereas
  `ScaleConstraint` uses `direction=0`, `v1=4`, and `v2=5`. In `richards`,
  `IdleTask`, `DeviceTask`, `HandlerTask`, and `WorkTask` have
  `packet_pending=4`, `task_waiting=5`, and `task_holding=6`, unlike lexical
  `TaskState` offsets `packet_pending=0`, `task_waiting=1`, and
  `task_holding=2`. A subclass-relaxed lexical-owner guard or shared
  offset would therefore silently access the wrong field.
- Exclude class attribute `Direction.FORWARD` at source instruction **#4**
  and unavoidable downstream initialization work. A defensible initial
  expectation is approximately **3–7%** improvement in an affected workload
  if independently guarded hot reads are actually selected. No **20%**
  workload improvement, full-suite gain, or actual candidate speedup is
  established.
- Final strengthened **610-line** unchanged-production transformed
  integration `tests/test_inherited_owner_fields.py` **fails 1 / 1.93
  seconds** for exactly the intended polymorphic specialization gap. The
  initial **1 / 2.10-second** generic-only fixture is superseded by this
  stronger run, which additionally profiles the exact lexical `StateBase`
  owner and exposes the existing fast-path-versus-fallback boundary.
- Profile evidence proves exact `StateBase` layout **(0, 1, 2)** and four
  concrete descendant layouts **(4, 5, 6)**. Existing Verify per-source
  counters record **32 indexed hits** for the exact base and **128 indexed
  fallbacks** for the four descendants. A correct candidate must preserve
  those existing base hits while reaching **160 indexed hits across all
  five exact owner variants**, rather than replacing the base specialization
  with a subclass-relaxed or wrong-index guard.
- Independently, the abstract lexical `DeltaBase` owner remains absent from
  profiles, while its two concrete descendants have different observed
  split-dictionary indices. Their inherited reads and writes still execute
  the generic operation with **zero indexed hits**. Profile, Verify, and
  Apply execute in separate processes; all processes and every compatibility
  assertion succeed, with only the final specialization counters RED.
- The complete transformed Verify and Apply behavioral matrix already
  passes: user hooks, property descriptors, base/MRO rebinding, promoted
  dictionaries, deleted fields, key growth, finalizers, and unsupported
  slots all retain CPython behavior. Only the expected inherited-field
  specialization assertion is RED. Exactly six production surfaces are now
  authorized for the implementation owner; implementation begins only after
  all three genuine unchanged-production REDs have been captured.
- Independent genuine unchanged-behavior structured optimizer regression
  `inherited_split_owner_catalog_reuses_one_anchor_per_concrete_owner_and_field`
  now **fails** because the actual inherited-owner catalog is exactly empty
  **`[]`**, versus **six expected transitive `Left` / `Right` /
  `Grandchild` × `value` / `direction` shared anchors**. The regression
  includes dense-index and unsupported-slot controls.
- A second independent genuine structured typed-plan validator regression
  initially **fails** because the existing optimization-plan validator
  rejects distinct exact `Left` and `Right` owners at the same source as an
  invalid duplicate. Together with the transformed integration and catalog
  regression, this establishes **three separate genuine REDs before
  six-file production implementation begins**.
- Both structured regressions now turn **RED-to-GREEN**.
  `inherited_split_owner_catalog_reuses_one_anchor_per_concrete_owner_and_field`
  passes with exactly **six transitive `Left` / `Right` / `Grandchild` ×
  `value` / `direction` bounded shared anchors**, including dense-index and
  unsupported-slot controls.
  `validates_distinct_exact_split_owners_at_one_inherited_field_source`
  passes distinct exact `Left` / `Right` owners at one source and rejects
  duplicate owners and mixed invalid variants. The unchanged 610-line
  transformed integration subsequently also turns genuinely RED-to-GREEN.
- Decisive complete transformed integration
  `tests/test_inherited_owner_fields.py` now **passes 1 / 2.03 seconds**,
  after its genuine unchanged-production **1 failed / 1.93 seconds**.
  Separate Profile, Verify, and Apply processes prove all **five exact
  StateBase owners**, preserving **32 original lexical-owner indexed hits**
  and converting **128 descendant fallbacks** into hits for **160 total
  hits per source**. Both unequal-index Delta descendants specialize, for
  inherited reads **and writes**. Generic hooks, properties, base/MRO
  mutation, promoted/deleted/growing dictionaries, reference-count
  finalizers, and unsupported slots preserve CPython behavior.
- Independently, aligned `cargo check -p soac_jit --tests` passes in
  **6.28 seconds**. All six approved production files compile; subsequent
  full package/runtime suites, candidate benchmarks, and authoritative full
  correctness gate also pass.
- First validated implementation milestone: **three of six authorized
  production files pass the focused structured regressions**. The optimizer
  now expresses
  deterministic, literal, same-module transitive split inheritance; reuses
  self-written concrete-owner / attribute anchors; requires hot evidence
  **at least 8 observations**; and caps polymorphic chains at **8 variants**.
  The typed-plan validator now admits distinct exact split owners for one
  source while rejecting duplicate same-owner variants and mixed attribute
  or storage shapes. The existing public `TypedAttrAccessPlan` enum now has
  a **`PolymorphicLateBoundOwnerFields`** variant; this is a public API
  addition that must be reported explicitly.
- The eight-variant-cap issue is fixed by filtering **actual owner-specific
  profiled layouts before applying the cap**, then explicitly reserving the
  profiled exact lexical owner: descendants are ordered first, with at most
  **seven descendants plus the lexical owner**. The JIT inherited full-MRO
  publisher, grouped typed-sidecar annotation, and exact-owner polymorphic
  guarded get/set emitter now compile and pass the real transformed
  integration.
- Stronger real production-path optimizer regression
  `inherited_split_owner_plans_cap_profiled_descendants_and_preserve_lexical_owner`
  now **passes**. Its fixture contains **10 profiled concrete descendants**,
  an alphabetically first but **unprofiled descendant**, and an existing
  profiled lexical `Root` owner at the same source. Production selects
  exactly **seven genuinely profiled descendants with distinct expected
  concrete indices plus `Root`**, preserving the **8-variant cap** and
  excluding the unprofiled descendant. Both focused optimizer regressions
  now pass **2 / 2**; the independent typed-validator RED-to-GREEN and
  five-owner transformed integration remain green.
- Complete affected Rust libraries now pass: `soac_ir_typed --lib`
  **54 / 54**, `soac_opt --lib` **210 / 210**, and `soac_jit --lib`
  **561 / 561**, for **825 / 825 tests total**. These include the
  distinct-exact-owner validator, six transitive inherited owner/field
  anchors, and actual profile-backed eight-variant cap preserving lexical
  `Root`. The new JIT structured regression also verifies polymorphic owner
  groups cannot be reused as exact-single-owner scalar guards. Package-scoped
  formatting and format checks pass for all three changed packages.
  Post-format full `cargo test -p soac_jit --tests` also passes
  **561 / 561**, and the aligned JIT test-target Cargo check passes. The
  grouped transformed-runtime suite passes **78 / 78 selected tests across
  10 files in 29.41 seconds**, with **7 deselected**. It covers inherited
  owners, prior late-owner/scalar behavior, source-function watchers,
  direct-generator monitoring, original-code mutations, fused floats,
  indexed fields, and broad imports. Production is frozen to exactly six
  files; normal fixed-eight, targeted three-round, matched zero-loss native
  profiles, and the full correctness gate are complete and passing.
- Release fixed-eight debug-single smoke
  `work/pyperformance/comparison-20260819-050518-2KsHNq` completes
  **8 / 8 workloads with zero worker errors** and unchanged
  **3,069 typed blocks / 218 functions**. Compared with mode-matched
  integrated-generator smoke **040613**, all six unaffected workloads retain
  identical generated bytes and machine blocks. Total generated native code
  grows **2,314,724 → 2,377,824 bytes (+2.73%)**.
- `deltablue` generated native code grows
  **430,556 → 459,688 bytes (+6.77%)**, with machine blocks
  **28,231 → 30,033**. Both `BinaryConstraint.input` / `output` methods grow
  **1,884 → 3,308 bytes**, and `choose_method` grows
  **30,320 → 39,984 bytes**. `richards` grows
  **324,272 → 358,240 bytes (+10.48%)**, with machine blocks
  **22,067 → 24,070**; the two inherited `TaskState` predicates grow
  **2,840 → 5,672 bytes** and **2,240 → 5,080 bytes**, while
  `Task.runTask` grows **5,532 → 11,160 bytes**. This confirms real
  guarded-path coverage but exposes substantial code-size cost.
- Debug-single cold one-loop timings and geometric means are **not valid
  throughput evidence**. Independently profiled, normally sampled fixed-eight
  comparison `work/pyperformance/comparison-20260819-050635-0mVSmo` now
  completes **8 / 8 workloads**. The paired stock geometric score improves
  **0.5099697650277614x → 0.520917130452074x**; arithmetic previous-SOAC
  geometric improvement is **1.0185607035898507x**; robust previous-SOAC
  geometric improvement is **1.015700x**, or **1.012746x** after
  paired-stock adjustment. `deltablue` means
  improve **4.2485349 → 3.85283076 ms (1.10270478x)**, and `richards` means
  improve **39.913370 → 35.7430085 ms (1.11667629x)**. This fixed-eight
  score remains far below the full-suite **1.10x** stock target.
- Other previous-SOAC mean ratios are `chaos` **0.97575x**,
  `comprehensions` **0.95981x**, `fannkuch` **0.97328x**, `float`
  **0.96172x**, `nbody` **1.00722x**, and `spectral_norm` **1.06554x**.
  All six unaffected workloads have exactly identical generated native
  bodies, so their apparent movements are not evidence of changed machine
  code. Statistical interpretation still requires repeated paired controls.
- Independent robust normal analysis confirms `deltablue` median
  **4.171529 → 3.821542 ms (1.091583x)**, clustered bootstrap **95%
  interval 1.04091–1.13356x**, or **1.093966x** paired-stock-adjusted.
  `richards` median improves **39.759100 → 35.600975 ms (1.116798x)**,
  interval **1.06980–1.17540x**, or **1.113471x** paired-stock-adjusted.
- Normally measured native code grows
  **23,359,400 → 24,353,560 bytes (+4.256%)**, while optimized typed
  coverage remains **3,069 blocks / 218 functions**. This normal-mode code
  cost differs from the earlier mode-matched debug smoke and must not be
  hidden. Mode-matched normal `deltablue` native code grows
  **429,984 → 463,672 bytes (+7.835%)**, and `richards` grows
  **334,172 → 399,900 bytes (+19.669%)**. Normal richards contains **17
  changed inherited bodies**; `Task.__init__` alone grows
  **4,160 → 17,596 bytes**, compared with fewer changed bodies in debug
  smoke. This code growth is real and profile-dependent. All workers report
  zero errors and existing scalar-region invalidations remain zero. Targeted
  three-round comparison
  `work/pyperformance/comparison-20260819-051003-t5P0GP` now confirms
  robust affected-workload improvements and supports **retaining** the
  candidate despite the material native-code growth.
- Across **60 targeted candidate samples**, `deltablue` robust median
  improves **4.171529 → 3.750207 ms (1.112346x)**, clustered bootstrap
  **95% interval 1.097582–1.152915x**, or **1.137479x** paired-stock
  adjusted. `richards` median improves
  **39.759100 → 33.958922 ms (1.170800x)**, interval
  **1.135034–1.219269x**, or **1.169866x** paired-stock adjusted.
  Unaffected `chaos` **0.993178x** and `comprehensions` **1.002310x** are
  neutral; robust subset geometric improvement is **1.067058x**,
  paired-stock adjusted **1.071040x**, and arithmetic subset improvement
  **1.056756x**. Matched zero-loss candidate native profiles subsequently
  confirm the expected generic-lookup reductions; the authoritative full
  correctness gate subsequently **passes**.
  The fixed-eight **0.520917x** stock score does not meet the full-suite
  **1.10x** objective.
- Matched zero-loss `deltablue` profiles use **400 replay loops** at sampling
  frequency **199**, with **434 baseline → 390 candidate samples**. Inclusive
  generic `PyObject_GetAttr` decreases **20.736% → 16.664%** and
  `GenericGetAttrWithDict` **14.288% → 9.999%**. Generic inherited
  `BinaryConstraint.input` ancestry falls **2.766% → 0%**, `output`
  **0.922% → 0%**, and `choose_method` **1.614% → 0.256%**. Attached replay
  **5.421225 → 4.869577 ms (1.1133x)** is diagnostic, not a benchmark
  headline; cold Cranelift ancestry remains approximately **13.1% → 13.3%**.
- Matched zero-loss `richards` profiles use **70 replay loops**, with
  **599 baseline → 522 candidate samples**. Inclusive generic
  `PyObject_GetAttr` falls **27.714% → 18.392%** and
  `GenericGetAttrWithDict` **23.707% → 16.857%**. Inherited generic
  `TaskState` holding ancestry falls **4.341% → 0%** and waiting
  **1.002% → 0%**. Attached replay
  **43.952764 → 38.476686 ms (1.14232x)** is also diagnostic only. All
  sampled ancestry percentages overlap and must not be added; independently
  repeated robust benchmark medians remain the performance authority.
- Reviewer correction: inherited publication must **not** expand the global
  function-owner watcher or weakref registry. The declaring base function is
  already registered, and pinned CPython `PyType_Modified` recursively
  invalidates every subclass type version. The completed implementation
  publishes only descendant-owned cells without expanding the existing
  global registry. Independent source review confirms original-function
  identity across the complete MRO, exact weak owner/type-version and live
  split-key guards, single receiver evaluation, original fallback, and
  preserved reference-count/counter behavior.

## Implementation and compatibility

- Proposed architecture: build a deterministic catalog of supported
  inherited self-field source sites, then use existing profile evidence to
  identify each exact concrete receiver owner and its own observed
  split-dictionary attribute index. Emit an explicit validated polymorphic
  plan / typed sidecar rather than rediscovering owner or layout semantics
  during codegen. A small exact-owner guard chain may mechanically select
  the matching concrete layout; every miss must retain the complete
  original generic Python attribute operation.
- Exactly six production surfaces are authorized for the separate
  implementation owner only:
  `pipeline_v3`, `plan_v3`, `typed.rs`, crate `lib.rs`,
  `typed_pipeline.rs`, and `jit/mod.rs`. The existing public
  `TypedAttrAccessPlan` enum now adds
  **`PolymorphicLateBoundOwnerFields`**; report this public variant
  explicitly. No new runtime helper or process-global mutable cache is
  selected. All six approved optimizer/typed/JIT files now compile; JIT
  full-MRO publication, grouped typed annotation, and guarded polymorphic
  reads/writes pass the transformed integration.
- Each concrete-owner case must prove a live weak owner reference, exact
  receiver-type identity, unchanged captured nonzero type version, supported
  inheritance/MRO, unchanged generic hooks, and the absence of an overriding
  descriptor for that field. Revalidate the live split-key table, expected
  index, interned attribute-name identity, inline-values availability and
  capacity, and non-null value before taking a direct load.
- Safe late-owner publication must scan the **entire exact concrete
  receiver MRO** for the originally registered inherited base function.
  Concrete subclasses often override or delegate `__init__`; accepting
  only the first effective descriptor identity would falsely reject every
  relevant `richards` case and the strengthened integration subclasses.
  Scanning for the original base function does not relax exact receiver
  identity, owner/type version, target descriptor or hook safety, or live
  split-key validation.
- Guard lifetime is exactly one guarded field operation: the owner weakref
  must still resolve to the same exact concrete class and its captured
  version must remain current at every execution. Class rebinding/death,
  descriptor or `__getattribute__` changes, incompatible MRO changes,
  dictionary materialization/promotion/deletion, missing keys, unexpected
  subclasses, and unknown receiver types must immediately use the unchanged
  original generic operation. Do not invoke user code while establishing a
  fast-path guard or prime/mutate class dictionaries merely to specialize.
- Preserve exact evaluation order, descriptor invocation, exception text,
  callback count, object ownership/reference counts, tracing/monitoring
  behavior, and same-module provenance. Explicitly reject object slots,
  class/static attributes, dynamic or cross-module owners, incomplete
  profile evidence, and any broad subclass-relaxed guard. Pinned CPython
  internal layout dependence is permitted when explicit and sound for the
  vendored interpreter.
- Focused transformed-runtime integration genuinely improves from
  **1 failed / 1.93 seconds** to **1 passed / 2.03 seconds**, preserving all
  compatibility controls; the independent structured optimizer catalog and
  distinct-owner typed validator regressions likewise turn
  **RED-to-GREEN**. Required cases
  include independently indexed
  subclasses, exact owner guards, descriptor/hook mutation, dynamic
  subclasses, missing/deleted/materialized/promoted attributes, class
  weakref lifetime, polymorphic ordering, no cross-owner confusion, and
  unchanged original fallback.

## Benchmark protocol and coverage

- Fixed benchmark selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm` against the
  same vendored stock CPython and integrated direct-generator SOAC baseline.
  A final broad performance claim requires the policy-defined full suite;
  targeted repeated rounds should include `deltablue`, `richards`, and
  unaffected guardrails.
- Profile each revision independently and require transformed benchmark
  project-module and actual hot-method JIT evidence. Do not treat benchmark
  completion alone as proof of inherited-field specialization or assume
  standard-library/dependency transformation. Candidate coverage is verified
  through unchanged typed totals, inherited per-source hits, and zero-loss
  native hot-method profiles.
- Baseline fixed-eight stock score: **0.5099697650277614x**; optimized typed
  coverage **3,069 blocks / 218 functions**; generated native code
  **23,359,400 bytes / 1,549,290 machine blocks**; serialized
  pre-optimization BlockPy **14,398,752 bytes**, verified from integrated
  `work/pyperformance/comparison-20260819-040730-wzYML7/summary.json`.
- Candidate benchmark results, generated-code growth, statistically robust
  medians, exact per-site hits/fallbacks, transformed hot functions, and the
  authoritative full correctness gate are recorded below. Independent
  compilation/setup-overhead attribution is unavailable.

## Measurements

| Metric | Integrated direct-generator baseline | Candidate | Change |
| --- | --- | --- | --- |
| Fixed-eight paired stock / SOAC geometric ratio | 0.5099697650277614x | 0.520917130452074x | fixed eight improves; full-suite stock 1.10x goal unmet |
| Fixed-eight previous-SOAC arithmetic / robust improvement | integrated `okqlrmxm/ccef62b6` | arithmetic 1.0185607035898507x; robust 1.015700x | stock-adjusted robust 1.012746x |
| `deltablue` normally sampled mean / median | 4.2485349 / 4.171529 ms | 3.85283076 / 3.821542 ms | mean 1.10270478x; median 1.091583x; 95% 1.04091–1.13356x |
| `richards` normally sampled mean / median | 39.913370 / 39.759100 ms | 35.7430085 / 35.600975 ms | mean 1.11667629x; median 1.116798x; 95% 1.06980–1.17540x |
| Targeted three-round `deltablue` median | 4.171529 ms | 3.750207 ms | 1.112346x; 95% 1.097582–1.152915x; stock-adjusted 1.137479x |
| Targeted three-round `richards` median | 39.759100 ms | 33.958922 ms | 1.170800x; 95% 1.135034–1.219269x; stock-adjusted 1.169866x |
| Targeted three-round robust / stock-adjusted geometric ratio | integrated baseline | 1.067058x / 1.071040x | arithmetic 1.056756x; chaos/comprehensions neutral |
| Optimized typed-IR blocks / functions | 3,069 / 218 | 3,069 / 218 | unchanged |
| Pre-optimization BlockPy bytes | 14,398,752 | pending | pending |
| Apply-mode native code bytes | 23,359,400 | 24,353,560 | +4.256%; material code growth |
| Normal `deltablue` generated native bytes | 429,984 | 463,672 | +7.835% |
| Normal `richards` generated native bytes | 334,172 | 399,900 | +19.669%; 17 changed inherited bodies |
| Apply-mode machine blocks | 1,549,290 | pending | pending |
| Mode-matched fixed-eight release debug-single smoke | 2,314,724 bytes | 2,377,824 bytes; 8 / 8 complete, zero worker errors | +2.73%; cold timings invalid |
| Debug-single `deltablue` native bytes / blocks | 430,556 / 28,231 | 459,688 / 30,033 | native +6.77% |
| Debug-single `richards` native bytes / blocks | 324,272 / 22,067 | 358,240 / 24,070 | native +10.48% |
| `deltablue` zero-loss native samples | 434; 27.225 MB | 390; 400 replay loops; frequency 199 | generic GetAttr 20.736% → 16.664%; replay diagnostic only |
| `richards` zero-loss native samples | 599; 37.514 MB | 522; 70 replay loops | generic GetAttr 27.714% → 18.392%; replay diagnostic only |
| `deltablue` inherited generic hot sites | input 2.766%; output 0.922%; choose_method 1.614% | input/output 0%; choose_method 0.256% | overlapping inclusive shares |
| `richards` inherited generic predicate sites | holding 4.341%; waiting 1.002% | each 0% | overlapping inclusive shares |
| Inclusive generic `PyObject_GetAttr` ancestry | delta 20.736%; richards 27.714% | pending | overlapping shares; not a speedup |
| Eligible inherited generic reads | delta 406,784 / 1,529,855; richards 795,472 / 2,968,392 | pending | 26.59% / 26.80%; not additive |
| Genuine transformed-runtime integration | 1 failed / 1.93 s; StateBase 32 existing hits + 128 descendant fallbacks | 1 passed / 2.03 s; five owners and 160 hits per source | genuine RED-to-GREEN; Delta unequal-index reads/writes and full semantic matrix pass |
| Aligned JIT Cargo test-target check | previous baseline | passes in 6.28 s | six production files compile |
| Genuine structured optimizer catalog regression | actual inherited catalog []; six transitive owner/field anchors expected | passes six anchors, dense/slot controls | genuine RED-to-GREEN |
| Structured production-path polymorphic cap | 10 profiled descendants, 1 unprofiled descendant, profiled Root | exactly 7 distinct-index profiled descendants + Root; 8 variants | GREEN; unprofiled descendant excluded |
| Genuine distinct-owner typed-validator regression | distinct exact Left / Right owners rejected as duplicate | distinct owners pass; duplicate/mixed variants rejected | genuine RED-to-GREEN |
| Complete affected typed-IR Rust library | integrated baseline | 54 / 54 passed | GREEN |
| Complete affected optimizer Rust library | integrated baseline | 210 / 210 passed | GREEN |
| Complete affected JIT Rust library / scalar-group exclusion | integrated baseline | 561 / 561 passed; polymorphic groups excluded from single-owner scalar guards | GREEN |
| Three affected Rust libraries / scoped formatting | integrated baseline | 54 + 210 + 561 = 825 passed; package-scoped formatting/check pass | GREEN |
| Post-format full JIT test targets / aligned Cargo check | integrated baseline | 561 / 561 passed; aligned check passes | GREEN |
| Grouped transformed semantic runtime suite | existing guardrails | 78 passed; 7 deselected; 10 files; 29.41 s | GREEN |
| Full `just test-all` correctness gate | current baseline previously passed | 1,221 nodeids; 88 / 88 isolated batches; 8 workers | GREEN; zero failed |

The authoritative full-gate log is
`work/logs/inherited-owner-test-all.log`. `just test-all` passes **1,221
Python nodeids across 88 / 88 isolated file batches and eight workers**,
with **zero failed batches**. Workspace Rust suites pass: JIT **561**, typed
IR **54**, optimizer **210**, lowering **371**, and PyO3 **8**. Cargo tests
take **72.359 seconds**, inner / outer pytest **93.990 / 94.003 seconds**,
and the complete test phase **166.374 seconds**. The known single
counter-dump batch accounts for **93.06 seconds**.

## Attempt history

### Attempt 1: quantify inherited concrete-owner layout mismatch

- Change: capture current integrated zero-loss profiles, correlate hot
  inherited receiver reads with exact concrete subclass split-dictionary
  layouts, and run a genuine unchanged-production transformed integration
  regression. Exactly six production surfaces are authorized for the
  separate implementation owner; implementation starts only after the
  integration, catalog, and validator REDs have all been observed.
- Measurements and coverage: `deltablue` **434 samples**, `richards` **599
  samples**, no lost-sample warnings; respective generic attribute ancestry
  **20.736% / 27.714%** and eligible inherited-read fractions
  **26.59% / 26.80%**. The existing fixed-eight stock score is
  **0.5099697650277614x** and generated code is
  **23,359,400 bytes / 1,549,290 blocks**.
- Compatibility and tests: the genuine unchanged-production integration
  **fails 1 / 1.93 seconds** after strengthening the earlier **1 / 2.10
  second** regression. Exact `StateBase` layout **(0, 1, 2)** already
  records **32 indexed hits per source**; four descendants with layout
  **(4, 5, 6)** collectively record **128 indexed fallbacks**. The target
  is **160 indexed hits across five variants** while preserving existing
  base behavior. Two differently indexed Delta descendants have zero hits
  and no observed abstract owner. Separate Profile/Verify/Apply processes
  preserve the complete semantic matrix. An independent structured optimizer
  RED proves the inherited catalog is empty instead of selecting six
  transitive concrete-owner / field anchors, with dense/slot controls.
  A second structured RED proves the existing typed validator rejects
  distinct exact `Left` / `Right` owners at the same source as duplicates.
  Three of six production files now contain deterministic transitive owner
  planning, shared anchors, **>=8** hot selection capped at eight variants,
  distinct-owner validation, and public
  `TypedAttrAccessPlan::PolymorphicLateBoundOwnerFields`. Both independent
  structured optimizer and typed-validator regressions now pass. Actual
  owner-specific profile layouts are filtered before the cap, and the exact
  profiled lexical owner is reserved alongside at most seven descendants.
  JIT inherited full-MRO publication, grouped typed annotation, and
  polymorphic guarded get/set emission are now complete. Publication does
  not expand the global owner registry: the declaring base is already
  registered and pinned `PyType_Modified` invalidates descendant versions,
  so only descendant cells are published. The genuine transformed
  integration now **passes 1 / 2.03 seconds**, producing all **160 indexed
  hits per source** and preserving the complete compatibility matrix;
  aligned JIT Cargo test-target checking passes in **6.28 seconds**.
  A stronger production-path cap regression also passes: **10 profiled
  descendants plus an alphabetically first unprofiled descendant and
  lexical Root** select exactly **seven profiled distinct-index descendants
  plus Root**, excluding the unprofiled case. Both optimizer focused tests
  pass **2 / 2**. Complete typed-IR and optimizer Rust libraries pass
  **54 / 54** and **210 / 210**, respectively, and the complete JIT library
  passes **561 / 561**, for **825 passing Rust tests total**. The new JIT
  regression prevents polymorphic groups from masquerading as
  exact-single-owner scalar guards. Scoped formatting and format checks pass
  for all three changed packages. Post-format full JIT test targets again
  pass **561 / 561**, and the aligned JIT test-target Cargo check passes.
  The grouped transformed suite passes **78 / 78 tests in 29.41 seconds**,
  with **7 deselected across 10 files** covering inherited owner fields,
  earlier late-owner/scalar behavior, source-function watchers,
  direct-generator monitoring, original-code mutations, fused floats,
  indexed fields, and broad imports. All six production files are frozen.
  Release debug-single smoke **050518** completes **8 / 8**, with unchanged
  typed coverage and no worker errors. Mode-matched generated native code
  grows **2,314,724 → 2,377,824 bytes (+2.73%)** overall,
  **+6.77%** in `deltablue`, and **+10.48%** in `richards`; all other
  workload code stays unchanged. Cold one-loop timings are not throughput
  evidence. Normal fixed-eight comparison **050635** subsequently completes
  **8 / 8**: stock score **0.520917130452074x**, arithmetic prior-SOAC
  **1.0185607035898507x**, `deltablue` mean **1.10270478x**, and `richards`
  mean **1.11667629x**. Robust normal medians improve **1.091583x** and
  **1.116798x**, with bootstrap intervals excluding one. Normally measured
  native code grows **4.256%** overall, **7.835%** for delta, and
  **19.669%** for richards; six unaffected workloads have unchanged native
  code and scalar invalidations remain zero. Targeted three-round comparison
  **051003** confirms `deltablue` median **1.112346x**, interval
  **1.097582–1.152915x**; `richards` median **1.170800x**, interval
  **1.135034–1.219269x**; and robust affected/control subset **1.067058x**.
  Controls are neutral and the optimization is retained despite substantial
  code growth. Matched zero-loss delta **434 → 390** and richards
  **599 → 522** profiles confirm delta input/output and richards predicate
  generic ancestry drops to zero, while delta `choose_method` falls to
  **0.256%** and overall generic attribute ancestry declines; attached
  replays and overlapping shares are diagnostic only. The authoritative
  full correctness gate also passes **1,221 nodeids / 88 isolated
  batches**, plus every affected Rust suite.
- Result: **IN PROGRESS; all three genuine integration/catalog/validator
  regressions RED-to-GREEN, optimizer cap/catalog tests 2 / 2, six
  production files compile, five-owner transformed Profile/Verify/Apply
  integration passes; typed/optimizer/JIT Rust 54 + 210 + 561 = 825 / 825
  and grouped transformed Python 78 / 78 GREEN; fixed-eight release smoke
  passes with material code growth; normal and three-round target medians
  significantly improve; matched zero-loss profiles confirm targeted lookup
  elimination or reduction; full correctness gate PASSED, LANDED
  CANDIDATE / RETAIN**.
- Reason: lexical inheritance does not imply identical dictionary layout;
  only an exact-owner, descriptor-safe, versioned, live-key-validated
  polymorphic decision can preserve CPython behavior.

## Verdict and next action

- Verdict: **LANDED CANDIDATE / RETAIN; FULL CORRECTNESS GATE PASSED**.
  Genuine unchanged-production integration,
  inherited-owner catalog, and distinct-owner typed-validator REDs are all
  established; the optimizer and validator now pass their genuine
  RED-to-GREEN regressions, and the lexical-owner cap is preserved. The
  complete transformed integration now also passes **1 / 2.03 seconds**,
  all six production files compile, and registry-safe full-MRO publication,
  grouped exact-owner guards, and inherited reads/writes preserve semantics.
  The production-path cap regression proves exactly seven profiled
  descendants plus the lexical owner while rejecting an unprofiled
  descendant; both optimizer tests pass **2 / 2**. Complete typed-IR and
  optimizer Rust libraries pass **54 / 54** and **210 / 210**; the full JIT
  library passes **561 / 561**, including the exact-single-owner scalar-group
  control. All three packages pass scoped formatting/checks. Post-format
  full JIT all-target tests again pass **561 / 561** and the aligned check
  passes. The transformed semantic suite passes **78 / 78 tests across
  10 files in 29.41 seconds**, with **7 deselected**. Six production files
  are frozen. Fixed-eight release debug-single smoke passes **8 / 8** but
  shows **+2.73%** total native-code growth, **+6.77%** for `deltablue`,
  and **+10.48%** for `richards`; cold timings are not meaningful. Normal
  fixed-eight throughput comparison completes with `deltablue` mean
  **1.10270478x**, `richards` mean **1.11667629x**, robust medians
  **1.091583x / 1.116798x**, and **4.256%** overall normal native-code
  growth (**+7.835% delta / +19.669% richards**). Six controls retain
  identical generated code. Targeted three-round **051003** confirms
  `deltablue` **1.112346x**, `richards` **1.170800x**, neutral controls,
  and robust subset improvement **1.067058x**. Retain this measured benefit
  while disclosing **4.256%** native-code growth. Matched zero-loss profiles
  confirm reduced generic attribute ancestry, elimination of targeted
  input/output and predicate sites, and a remaining **0.256%**
  `choose_method` ancestry. The full correctness gate passes all **1,221
  Python nodeids / 88 isolated batches** plus workspace Rust suites; the
  stock **1.10x** goal remains unmet.
- Transferable lesson: an inherited method's lexical owner is not a
  concrete receiver layout. Guard every exact profiled concrete owner
  independently; generic attribute ancestry is overlapping evidence, not a
  prediction of workload speedup.
- Next action: integrate the fully validated retained candidate; subsequent
  optimizations must account for its material native-code growth and the
  unmet full-suite stock **1.10x** objective.
