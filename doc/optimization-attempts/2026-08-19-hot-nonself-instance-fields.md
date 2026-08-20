---
title: "Hot Non-Self Instance Field Specialization"
---

# Hot non-self instance field specialization

- Status: **ATTEMPT 2 RETAIN / VALIDATED LANDING CANDIDATE; ORIGINAL
  ATTEMPT 1 REMAINS
  LANDED / RETAINED WITH ALL HISTORICAL GAINS, DISCLOSED COMPREHENSIONS
  REGRESSION, NATIVE-CODE GROWTH, AND VERIFIED FULL GATE PRESERVED;
  ALL THREE GENUINE LAYOUT-UNIFORM TRANSFORMED / WHOLE-PRODUCTION
  PLANNER / REAL EMITTED-CFG REDS TURN GREEN; CLEAN REPEATED RICHARDS
  1.088106X / STOCK-PAIRED 1.070336X; TWO-FILE IMPLEMENTATION AND
  AUTHORITATIVE FULL CORRECTNESS GATE GREEN**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`nnyqlvvy`**, commit
  **`70012945`**.
- Candidate revision: change **`mztqqkor`**, commit **`0fa6ba9e`**; genuine
  unchanged-production integration RED is established and the approved
  one-file production owner is beginning structured work.
- Outcome: determine whether existing exact-owner split-dictionary
  constructor cells can safely specialize hot non-`self` instance reads and
  writes with unique profile-backed owner provenance, without adding cells,
  weakening guards, or disrupting retained scalar/self/inherited behavior.

## Hypothesis and evidence

- General-purpose opportunity: Python methods often inspect other local
  objects besides their first `self` parameter. Existing same-module
  constructor cells already prove some exact receiver owners and attribute
  layouts, but current owner-field planning principally admits `self`
  accesses or already-selected guarded scalar regions. Hot ordinary
  non-`self` fields can therefore remain generic despite reusable exact
  owner/attribute/index evidence.
- Integrated exact-positional normal fixed-eight stock geometric score is
  **0.5482172650503208x**. The authoritative full-suite **1.10x stock**
  goal remains unmet. Existing generated Apply code totals **24,353,560
  native bytes / 1,608,670 machine blocks**, optimized typed coverage
  **3,069 blocks / 218 functions**, and serialized pre-optimization BlockPy
  **14,398,752 bytes**.
- Fresh zero-loss post-binder native profiles contain **365 `deltablue`
  samples** and **526 `richards` samples**. Inclusive generic instance
  lookup ancestry is approximately **17.534% / 18.632%**, respectively.
  These inclusive stack shares overlap and are not additive speedup or proof
  that any specific access can be eliminated.
- Delta candidate: `Variable.value` has a unique same-module exact observed
  owner and existing split-dictionary constructor cell at index **1**.
  `EqualityConstraint.execute` generic subtree occupies approximately
  **5.205% inclusive / 1.644% self**. Only the properly guarded field
  portion is plausibly removable; unrelated subtree work remains.
- Richards candidates include exact owner-backed `Packet.datum` /
  `Packet.data`, `HandlerTaskRec`, `TaskWorkArea`, and `WorkerTaskRec`
  fields. Chaos may expose unique `GVector.x` / `.y` / `.z` constructor
  cells. These are source/profile opportunities, not established candidate
  selections or measured improvements.
- Source plus decoded-profile census finds **10 globally unique profiled
  and already-anchored delta attributes / 64 non-self occurrences**,
  including `choose_method` **10**, `Scale.execute` **8**,
  `Scale.recalculate` **7**, and `Equality.execute` **2**. Richards has
  **12 unique anchored attributes / 32 occurrences**, including
  `Handler.fn` **9** and `Device.fn` / `Idle.fn` / `Work.fn` **4 each**.
  Chaos has **16 unique anchored attributes / 42 occurrences**, including
  `GVector.x` / `.y` / `.z`.
- Comprehensions has **2 globally unique existing anchors but only 1
  non-self occurrence**; its dataclass `Widget` still lacks the desired
  eligible anchor. Do not predict a meaningful comprehensions improvement.
  Global per-attribute profile evidence does not encode the source-specific
  owner; relying only on an attribute name would confuse unrelated same-name
  classes.
- No user-visible existing correctness bug or candidate speedup is claimed.
  The optimization must derive a unique exact same-module owner and existing
  anchor; otherwise preserve the original generic operation.
- Genuine unchanged-production integration
  `tests/test_late_owner_nonself_fields.py` **fails 1 / 4.43 seconds** at the
  intended final assertion on line **551** after real separate Profile,
  Verify, and Apply execution. Profile proves unique exact `Box.payload`
  split index **1**. Expected source indexed hits are `read_other` **64**,
  `Consumer.consume` **32**, compound access **32**, nested eager access
  **64**, and store **32**; all currently record **0**.
- Existing specialization controls remain GREEN in the same failing run:
  `Box.__init__` constructor anchor sources **#2 / #7 / #16** each record
  **37 indexed hits**, `Box.read_self` records **32**, and prior
  unequal-index inherited owners record **64**. Ambiguous owners, cold
  sites, generated/unanchored objects, slots, mutation/descriptor behavior,
  and finalizers also pass. Fixture setup explicitly warms `Box.__init__`;
  omitting that call left constructor profile rows at only **1**, so that
  rejected setup did not establish a valid hot anchor.
- Production behavior was unchanged before this genuine RED. Exactly one
  production file is authorized for the separate implementation owner;
  the independent structured RED also now genuinely fails, while candidate
  subsequent implementation, performance comparisons, and the full
  correctness gate all complete.
- A second genuine unchanged-production optimizer regression exercises the
  real lowering/production planning path with `Record`, `Left`, `Right`,
  and `Slotted` classes plus an actual encoded `CounterDump` profile. Its
  exact failure is **`hot non-self Load at #0 should reuse existing
  Record.payload cell`**; the focused `soac_opt` test **fails 1** before
  any production behavior is implemented. Coverage also includes hot load
  and store, **10 candidate accesses** for the eight-entry cap, scalar
  precedence, ambiguous owners, cold observations, slotted objects, and
  unanchored fields.
- Reviewer identified a test-fixture-only correctness issue: a source-count
  `HashMap` keyed only by `InstrId` collides across functions because
  instruction identity is function-relative. The implementation owner is
  corrected fixture keys to **function identity plus source instruction**
  before production work.
- The same genuine production-path structured optimizer regression now
  turns **RED-to-GREEN, 1 passed / 1 selected**. Actual lowered module and
  encoded profile plans prove a globally unique exact owner reuses the
  minimum existing `SplitDict` constructor cell, with exact source and
  field index for both load and store. Ambiguous, cold, slotted, and
  unanchored cases remain generic; existing scalar and self plans retain
  precedence; exactly the **eight hottest deterministic** candidates are
  selected; and no catalog cells are added.
- The sole production implementation is an approximately **75-line private
  helper in `crates/soac_opt/src/pipeline_v3.rs`**, appended **after
  existing scalar planning**. It adds no public API, runtime path, or owner
  cells. The frozen real transformed Profile→Verify→Apply integration is
  subsequently turns genuinely RED-to-GREEN; broad suites, candidate
  benchmark and full correctness gate subsequently pass.
- The independently frozen real transformed integration
  `tests/test_late_owner_nonself_fields.py` now **passes 1 / 4.50 seconds**
  after its genuine unchanged-production **1 failed / 4.43 seconds**.
  Separate Profile→Verify→Apply processes record indexed hits for
  top-level `read_other`, unrelated `Consumer.consume`, compound receivers,
  nested eager comprehensions, and non-self stores. Ambiguous/cold/
  generated/unanchored/slotted cases remain generic. Exact subclass hooks,
  property/MRO/class replacement, promoted/deleted/growing dictionaries,
  finalizer ordering, and existing self/inherited specializations all pass.
  The first fast-pytest debug-extension rebuild takes approximately
  **24 seconds** once; this is environmental workflow cost, not benchmark
  evidence.
- The strengthened structured optimizer regression now also rejects a
  profiled **foreign-module owner** that shares an attribute with an
  otherwise valid same-module anchor: globally ambiguous ownership remains
  ineligible even when one local candidate appears valid. Full
  `cargo test -p soac_opt --lib` passes **211 / 211** tests, including
  existing virtual-object, constructor, scalar, and inherited-specialization
  coverage. The full JIT library also passes **563 / 563** tests.
- Focused transformed-runtime guardrails pass **26 / 26 in 34.30 seconds**,
  including the new real non-self regression, previous late scalar and
  inherited fields, fused floats, source-function and generator watchers,
  import-time constructors, all seven optimization Verify counter cases,
  actual virtual-constructor escape/materialization, cross-module
  attributes, method getters/setters, and function/default/code mutation.
  Aligned combined `cargo check -p soac_opt -p soac_jit --tests` and
  package-scoped `fmt-rust-check soac_opt` both pass. Production is frozen
  to exactly `crates/soac_opt/src/pipeline_v3.rs`; candidate performance and
  full correctness gate subsequently pass.
- Release debug-single smoke **061626** passes **8 / 8 workloads with zero
  worker errors** and unchanged **3,069 typed blocks / 218 functions**.
  Mode-matched total native code grows
  **2,377,824 → 2,426,104 bytes (+2.030%)**, with machine blocks
  **157,417 → 160,598 (+2.021%)**. `chaos` grows
  **695,920 → 712,432 bytes (+2.373%; 12 functions)**; `deltablue`
  **459,688 → 481,284 bytes (+4.698%; 19 functions)**; `richards`
  **358,240 → 367,664 bytes (+2.631%; 7 functions)**; and
  `comprehensions` **301,328 → 302,076 bytes (+0.248%)**. `fannkuch`,
  `float`, `nbody`, and `spectral_norm` remain byte-identical. No functions
  are lost or shrunk, and constructor bodies remain unchanged.
- Actual specialized-target native sizes change for
  `EqualityConstraint.execute` **1,456 → 2,280 bytes**,
  `ScaleConstraint.execute` **13,768 → 16,508 bytes**, `HandlerTask.fn`
  **10,576 → 13,412 bytes**, and `Spline.__call__`
  **92,792 → 97,216 bytes**. These are generated-code coverage and cost
  evidence, not throughput. Cold single-loop smoke timings are invalid for
  performance conclusions. Normal fixed-eight comparison **061808**
  subsequently completes; robust analysis confirms benefits and a retained
  comprehensions regression.
- Normally sampled fixed-eight comparison **061808** completes **8 / 8**.
  Official paired-stock geometric score improves
  **0.5482172650503208x → 0.5594598880789836x**, and arithmetic
  previous-SOAC geometric improvement is **1.0148678728309706x**.
  Previous-SOAC workload **mean** ratios are `chaos` **1.066432x**,
  `comprehensions` **0.973369x**, `deltablue` **1.105441x**, `fannkuch`
  **0.956711x**, `float` **0.967598x**, `nbody` **1.000895x**, `richards`
  **1.048716x**, and `spectral_norm` **1.009271x**. Unaffected code may
  remain invariant and paired-stock timing may drift; do not classify
  apparent regressions or gains as causal without normal-mode code
  attribution and repeated paired rounds. Later matched rounds and the full
  correctness gate establish the retained outcome.
- Independent fixed-eight robust previous-SOAC geometric improvement is
  only **1.00314x (+0.31%)**, or **1.01321x** after paired-stock
  adjustment. Robust `chaos` ratio is **1.03340x**, clustered interval
  **1.0127–1.1062x**; `deltablue` **1.06625x**, interval
  **1.0017–1.1643x**, although an alternate reviewer interval includes
  neutral and uncertainty must be disclosed; `richards` **1.05546x**,
  interval **1.0195–1.0719x**; and `comprehensions` **0.9688x**, interval
  **0.945–1.005x**, which remains inconclusive. `float` falls to roughly
  **0.954x** despite exactly unchanged emitted code, indicating possible
  paired-stock/VM noise rather than a demonstrated code-caused regression.
- Exact normal-mode generated native code grows
  **24,353,560 → 25,033,800 bytes (+2.7932%)**. Workload bodies grow for
  `deltablue` **463,672 → 487,496 bytes (+5.138%)**, `richards`
  **399,900 → 415,908 bytes (+4.003%)**, `chaos`
  **704,988 → 731,652 bytes (+3.782%)**, and `comprehensions`
  **304,148 → 305,676 bytes (+0.502%)**; the four unaffected workloads
  remain byte-identical. Typed coverage stays **3,069 blocks / 218
  functions**, with no lost functions or worker errors.
- Targeted three-round comparison **062131** against prior exact-positional
  comparison **054212** completes with **60-versus-60 samples**. Robust
  four-workload geometric improvement is **1.03730284x**, or **1.04940010x**
  after paired-stock adjustment; official arithmetic previous-SOAC
  improvement is **1.0245948746x**.
- Repeated `chaos` improves **55.903815 → 52.915306 ms (1.056477x)**,
  clustered **95% interval 1.02991–1.08743x**, or **1.07391x**
  stock-adjusted; `deltablue` improves
  **3.529319 → 3.338118 ms (1.057278x)**, interval
  **1.01003–1.11517x**, or **1.05735x** adjusted; and `richards` improves
  **31.815431 → 29.670153 ms (1.072304x)**, interval
  **1.03668–1.12246x**, or **1.10669x** adjusted.
- Crucial negative result: repeated `comprehensions` worsens
  **60.14894 → 62.22617 us (0.966618x)**, approximately **3.34%** lower
  throughput, with interval **0.94899–0.99361x** entirely below one and
  stock-adjusted ratio **0.96505x**. Every round regresses, and three
  candidate comprehension bodies actually change, including nested
  captured-owner and benchmark bodies. Do not dismiss this as noise.
  Matched profiles confirm affected generic-lookup reductions but do not
  establish the comprehension-regression cause. Retain the candidate while
  transparently reporting that every-round regression and **2.7932%** normal
  native-code growth; the authoritative full correctness gate passes.
- Matched zero-loss `deltablue` profiles use **400 replay loops** and
  contain **365 integrated-binder → 354 candidate samples**. Inclusive /
  self `GenericGetAttr` ancestry falls
  **17.534% / 4.658% → 9.603% / 2.260%**, and inclusive
  `PyObject_GetAttr` falls **22.466% → 14.970%**. Lazy compiler ancestry
  remains substantial, increasing **13.425% → 14.683%**.
- Matched zero-loss `richards` profiles use **70 replay loops** and contain
  **526 baseline → 469 candidate samples**. Inclusive / self generic lookup
  falls **18.632% / 6.466% → 15.989% / 3.839%**; lazy compiler ancestry
  increases **9.314% → 10.230%**. Inclusive stacks overlap, and profiles
  are mechanism diagnostics rather than headline performance evidence.
- Available `comprehensions` profiling compares **844 → 782 samples against
  the older integrated direct-generator revision**, not the exact
  positional-binder parent. Diagnostic replay improves
  **71.1568 → 67.8875 us**, but generic lookup rises **6.045% → 6.394%**,
  lazy compiler ancestry changes **7.218% → 5.500%**, and GC changes
  **16.604% → 14.322%** with different unwind stacks. This non-matched
  replay may reflect compilation/GC and **does not prove the candidate
  guards help, are benign, or explain the repeated 0.966618x regression**.
  The matched 60-versus-60 benchmark remains authoritative; causal root is
  unproven.

## Implementation and compatibility

- Exactly one production file is authorized for the implementation owner:
  `crates/soac_opt/src/pipeline_v3.rs`. The separate new transformed-runtime
  regression `tests/test_late_owner_nonself_fields.py` genuinely fails
  **1 / 4.43 seconds** with existing constructor/self/inherited controls
  passing. An independent production-path structured optimizer regression
  initially genuinely fails, then turns **GREEN 1 / 1** after fixing its
  function-plus-source fixture identity and implementing one private
  production helper. The real frozen transformed integration also turns
  **GREEN 1 / 4.50 seconds**.
- Proposed decision: admit only an existing hot non-`self` field source with
  at least **8 generic observations**, **globally exactly one** profiled
  exact owner for the attribute, an exact same-module owner, and an
  already-published `SplitDict` constructor anchor with the same exact
  owner, attribute name, and index. Per-attribute owner evidence is not
  source-specific; globally ambiguous owners are rejected even if one
  source appears likely. Reuse the existing minimum matching cell index
  mechanically while preserving original source identity, access kind, and
  field index. Do not add owner cells, modify publication, extend
  process-global registries, or introduce a public API.
- Append new non-self decisions **after** existing scalar, `self`, and
  inherited plans, preserving their precedence. Add at most **8 new
  non-self decisions per function**, sorted hottest first with a
  deterministic source/access/owner tie-break; never widen or replace an
  existing validated decision.
- Preserve existing ordinary indexed-field, selected exact-int scalar,
  `self` owner, and polymorphic inherited-owner precedence. The new
  candidate must not overwrite or weaken existing sidecars, conflate
  distinct exact owners, or convert a polymorphic group into a trusted
  single-owner scalar guard.
- Existing exact guard lifetime remains per access: the cell's weak owner
  must still resolve to the receiver's exact live class, captured nonzero
  type version must match, generic hooks/descriptors must remain safe, and
  the current live split-key table/name/index/inline capacity/value must
  still validate. Owner death/rebinding, subclass overrides, descriptor or
  hook mutation, materialized/promoted/deleted dictionaries, missing keys,
  and ambiguous or absent provenance take the original generic fallback.
- Preserve one receiver evaluation, original getter/setter semantics,
  callback/refcount/finalizer ordering, Verify source counters, exact
  exceptions, and any existing inlining/remapping behavior. Do not specialize
  slots, dynamic or foreign-module owners, classes without an existing
  anchor, or ambiguous attributes observed on more than one concrete owner.
- Focused transformed Profile→Verify→Apply regression is genuinely RED
  **1 / 4.43 seconds**, with owner ambiguity, hotness, missing anchors,
  mutation, and prior self/inherited controls passing. Independent
  structured decision regression also initially fails on the missing
  `Record.payload` anchor, then turns **GREEN 1 / 1**, proving existing
  owner-cell reuse, scalar/self precedence, and the eight-entry cap. The
  independently frozen real transformed integration also genuinely turns
  **GREEN 1 / 4.50 seconds**.

## Benchmark protocol and coverage

- Fixed benchmark selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`, compared
  against the same vendored stock CPython and integrated exact-positional
  SOAC revision. Independently profile each revision and inspect actual
  transformed hot-function coverage and emitted owner guards.
- Baseline normal comparison:
  `work/pyperformance/comparison-20260819-053859-89QDJ8/summary.json`.
  Existing completion is **8 / 8**; candidate completion, targeted repeated
  affected/control rounds, paired stock, confidence intervals, native-code
  cost, cold setup impact, and full-suite acceptance are **pending**.
- Current optimized typed coverage is **3,069 blocks / 218 functions**;
  generated native code is **24,353,560 bytes / 1,608,670 machine blocks**;
  serialized pre-optimization BlockPy is **14,398,752 bytes**. No candidate
  transformation/native sizes have been measured.
- Baseline post-binder profiles are zero-loss delta **365 samples** and
  richards **526 samples**. Generic lookup shares are overlapping
  attribution; no candidate native profile or per-source hit counter exists.

## Measurements

| Metric | Integrated exact-positional baseline | Candidate | Change |
| --- | --- | --- | --- |
| Fixed-eight paired stock / SOAC geometric score | 0.5482172650503208x | 0.5594598880789836x | fixed-eight improvement; full-suite stock 1.10x goal unmet |
| Previous-SOAC arithmetic / robust improvement | integrated `nnyqlvvy/70012945` | arithmetic 1.0148678728309706x; robust 1.00314x | stock-adjusted robust 1.01321x |
| Robust targeted-workload median ratios | integrated binder baseline | chaos 1.03340x; delta 1.06625x; richards 1.05546x; comprehensions 0.9688x | delta alternate interval and comprehensions include neutral |
| Matched targeted three-round robust / stock-adjusted geometric improvement | previous binder comparison 054212 | 1.03730284x / 1.04940010x | arithmetic 1.0245948746x; 60 versus 60 samples |
| Three-round `chaos` / `deltablue` / `richards` median ratios | 55.903815 ms / 3.529319 ms / 31.815431 ms | 52.915306 ms / 3.338118 ms / 29.670153 ms | 1.056477x / 1.057278x / 1.072304x; intervals exclude one |
| Three-round `comprehensions` regression | 60.14894 us | 62.22617 us | 0.966618x; interval 0.94899–0.99361x; every round slower |
| Fixed-eight previous-SOAC workload means | integrated binder baseline | chaos 1.066432x; comprehensions 0.973369x; delta 1.105441x; fann 0.956711x; float 0.967598x; nbody 1.000895x; richards 1.048716x; spectral 1.009271x | preliminary means; no causal regression claim |
| Optimized typed-IR blocks / functions | 3,069 / 218 | 3,069 / 218 | unchanged |
| Serialized pre-optimization BlockPy bytes | 14,398,752 | pending | pending |
| Apply-mode native code bytes | 24,353,560 | 25,033,800 | +2.7932%; material code growth |
| Normal affected-workload generated bytes | delta 463,672; richards 399,900; chaos 704,988; comprehensions 304,148 | delta 487,496; richards 415,908; chaos 731,652; comprehensions 305,676 | +5.138% / +4.003% / +3.782% / +0.502% |
| Apply-mode machine blocks | 1,608,670 | pending | pending |
| Mode-matched release debug-single native bytes / machine blocks | 2,377,824 / 157,417 | 2,426,104 / 160,598 | +2.030% bytes / +2.021% blocks; 8 / 8 pass |
| Debug-single workload native growth | mode-matched integrated binder baseline | chaos +2.373%; delta +4.698%; richards +2.631%; comprehensions +0.248% | remaining four workloads unchanged |
| Matched post-binder delta / richards zero-loss samples | 365 / 526 | 354 / 469 | 400 / 70 replay loops; mechanism diagnostic only |
| Inclusive / self generic instance lookup ancestry | delta 17.534% / 4.658%; richards 18.632% / 6.466% | delta 9.603% / 2.260%; richards 15.989% / 3.839% | overlapping shares; cold compiler contamination |
| Non-matched `comprehensions` diagnostic profile | older direct-generator baseline 844 samples | candidate 782 samples | replay faster but compile/GC differ; does not explain repeated regression |
| Delta `EqualityConstraint.execute` generic subtree | 5.205% inclusive / 1.644% self | pending | only guarded field portion potentially removable |
| Unique existing `Variable.value` constructor anchor | owner `Variable`, split index 1 | pending | no candidate hit claim |
| Globally unique existing-anchor source census | delta 10 attrs / 64 occurrences; richards 12 / 32; chaos 16 / 42; comprehensions 2 / 1 | pending | static/profile opportunity only; no comprehensions gain predicted |
| Genuine transformed non-self integration | 1 failed / 4.43 s; five intended consumer hit counts all zero | 1 passed / 4.50 s; all top-level/method/compound/nested/store hits | genuine RED-to-GREEN; all ambiguity/mutation/self/inherited controls pass |
| Genuine structured optimizer decision regression | 1 focused test failed; hot non-self Load #0 misses Record.payload cell | passes 1 / 1; unique existing min cell, exact load/store source/index | genuine RED-to-GREEN; eight hottest and ambiguity/cold/slot/scalar controls |
| Complete affected optimizer Rust library / foreign-module ambiguity | integrated binder optimizer baseline | 211 / 211 passed; foreign owner invalidates otherwise valid same-module anchor | GREEN; virtual/constructor/scalar/inherited controls preserved |
| Complete affected JIT Rust library | integrated binder JIT baseline | 563 / 563 passed | GREEN |
| Focused transformed prior-specialization guardrails | existing retained runtime behavior | 26 / 26 passed in 34.30 s | GREEN; scalar/inherited/virtual/counter/mutation controls |
| Aligned combined optimizer / JIT test-target check and scoped format check | existing baseline | `cargo check -p soac_opt -p soac_jit --tests` and `fmt-rust-check soac_opt` pass | GREEN |
| Full `just test-all` correctness gate | integrated baseline passed | 1,222 nodeids; 89 / 89 isolated file batches; 8 workers | GREEN; zero failed |

The authoritative complete gate is recorded in
`work/logs/hot-nonself-fields-test-all.log`. `just test-all` passes
**1,222 Python nodeids across 89 / 89 isolated file batches and eight
workers**, with **zero failed batches**. Workspace Rust suites pass JIT
**563**, optimizer **211**, typed IR **54**, lowering **371**, and PyO3
**8**. Cargo tests take **66.743 seconds**, inner / outer pytest
**94.592 / 94.607 seconds**, and the complete test phase **161.366
seconds**; the known counter-dump batch takes **93.80 seconds**.

## Attempt history

### Attempt 1: identify unique owner-backed non-self fields

- Change: correlate fresh post-binder delta/richards zero-loss lookup
  attribution with existing exact-owner split constructor anchors and
  retained owner/scalar specializations, then capture a genuine
  unchanged-production transformed integration RED. One optimizer file is
  authorized only after this RED.
- Measurements and coverage: delta **365 samples / 17.534%** inclusive
  generic lookup; richards **526 samples / 18.632%**. Delta
  `Variable.value` has unique index **1**; decoded globally unique existing
  anchors cover delta **10 attributes / 64 occurrences**, richards
  **12 / 32**, chaos **16 / 42**, and comprehensions only **2 / 1**.
  `Widget` lacks its desired eligible anchor, so no comprehensions gain is
  predicted. Existing stock
  score is **0.5482172650503208x** with unchanged baseline native totals.
- Compatibility and tests: unchanged-production transformed integration
  genuinely **fails 1 / 4.43 seconds**, because intended unique-owner
  non-self read/consume/compound/nested/store indexed hits are all zero.
  Existing `Box.__init__` anchors remain **37 hits each**, `read_self`
  **32**, and inherited cases **64**; ambiguous/cold/unanchored/slots/
  mutation/descriptor/finalizer controls pass. Explicit constructor warming
  fixes the initially insufficient **1-observation** setup. An independent
  production-path lowered-module / encoded-CounterDump optimizer regression
  also fails exactly because hot non-self load **#0** does not reuse the
  existing `Record.payload` constructor cell. Its fixture covers ten
  candidate sites, scalar/ambiguity/cold/slotted/unanchored controls, and
  requires correcting `InstrId`-only source-count keys to function plus
  source before implementation. The exact structured regression now passes
  **1 / 1** after an approximately **75-line private helper** in the sole
  approved optimizer file. It reuses the minimum exact existing split cell,
  selects load/store by exact source/index, rejects unsupported owners,
  preserves prior scalar/self decisions, and deterministically caps at
  eight hottest candidates without adding cells or APIs. The frozen real
  integration now genuinely turns **RED 1 / 4.43 seconds → GREEN
  1 / 4.50 seconds**, proving all expected non-self source hits while all
  ambiguity/mutation/descriptor/dictionary/finalizer/self/inherited
  controls pass across Profile→Verify→Apply. The structured regression also
  rejects a profiled foreign owner even when a valid same-module anchor
  exists. Complete optimizer Rust tests pass **211 / 211**, preserving
  virtual-object/constructor/scalar/inherited cases. The first
  debug-extension rebuild costs approximately **24 seconds** once. Full JIT
  Rust tests pass **563 / 563**, and focused transformed guardrails pass
  **26 / 26 in 34.30 seconds**, including real virtualization,
  cross-module attributes, all seven Verify counter cases, and existing
  optimization/mutation safeguards. Combined aligned optimizer/JIT
  `--tests` checking and package-scoped optimizer formatting check pass.
  The sole optimizer production file is frozen. Release debug-single smoke
  **061626** passes **8 / 8** with unchanged typed coverage and zero errors,
  but mode-matched native code grows **2.030%** and machine blocks
  **2.021%**; actual delta/richards/chaos hot bodies expand while four
  unaffected workloads remain identical. Cold smoke times are not valid
  throughput. Normal fixed-eight comparison **061808** completes **8 / 8**:
  official stock score **0.5594598880789836x**, arithmetic previous-SOAC
  **1.0148678728309706x**, but robust previous-SOAC improvement only
  **1.00314x** / paired-stock adjusted **1.01321x**. Normal native code
  grows **2.7932%**, with four affected workloads expanded and four
  byte-identical. Matched three-round comparison **062131** confirms robust
  subset **1.03730284x**, with chaos **1.056477x**, delta **1.057278x**,
  and richards **1.072304x**, but also a significant every-round
  comprehensions regression **0.966618x**, interval
  **0.94899–0.99361x**, and three changed bodies. Matched delta/richards
  zero-loss profiles confirm reduced generic lookup, with cold compiler
  contamination. Comprehensions profiling uses an older direct-generator
  comparator and cannot explain its reproducible regression. The candidate
  is retained despite that real negative result and **2.7932%** code growth;
  full correctness gate passes **1,222 nodeids / 89 isolated batches** plus
  every Rust suite.
- Result: **IN PROGRESS; genuine production-path optimizer and independent
  transformed integration RED-to-GREEN verified, one private one-file helper
  implemented; optimizer Rust 211 / 211, JIT Rust 563 / 563, and
  transformed Python 26 / 26 and scoped aligned/format checks GREEN; release
  smoke 8 / 8; normal robust previous 1.00314x with +2.7932% native bytes;
  target workloads improve in repeated rounds but comprehensions regresses;
  matched profiles completed, full correctness gate PASSED, LANDED /
  RETAIN**.
- Reason: existing constructor anchors can establish unique exact receiver
  layouts, but name-only inference, added cells, and relaxed subclass guards
  would violate soundness.

## Verdict and next action

- Verdict: **LANDED / RETAIN; FULL CORRECTNESS GATE PASSED**. Genuine
  unchanged-production non-self integration
  RED is established, and the real production-path structured optimizer
  regression turns **GREEN 1 / 1** after the bounded one-file private
  helper. The independent transformed integration also passes
  **1 / 4.50 seconds** after its genuine RED; full optimizer library passes
  **211 / 211**, including foreign-module ambiguity and virtual/scalar
  controls; JIT Rust passes **563 / 563**, and focused transformed Python
  passes **26 / 26 in 34.30 seconds**. Combined optimizer/JIT test-target
  checking and scoped optimizer format check pass. Release fixed-eight
  debug-single smoke passes **8 / 8** but native code grows **2.030%**;
  normal fixed-eight comparison completes with stock score
  **0.5594598880789836x** and arithmetic previous-SOAC
  **1.0148678728309706x**, but robust previous improvement only
  **1.00314x**, stock-adjusted **1.01321x**. Actual normal generated code
  grows **2.7932%**. Matched three-round comparison **062131** confirms
  delta/chaos/richards improvements but also a statistically supported
  **0.966618x comprehensions regression every round**, with three changed
  bodies. Unchanged-code float shows separate environmental noise. Matched
  profiles confirm substantial delta/richards generic lookup reductions but
  include cold compiler work. Available comprehension replay uses an older
  direct-generator comparator and different GC/compiler ancestry; its
  apparent improvement does not disprove the actual repeated regression or
  identify causality. Retain the overall robust **1.03730284x** subset gain
  while disclosing **0.966618x comprehensions** and **2.7932%** native-code
  growth; the full correctness gate passes all **1,222 nodeids / 89
  isolated batches** plus workspace Rust suites. No public API,
  runtime path, or new owner cell is added.
- Transferable lesson: a hot attribute name alone is not owner provenance.
  Reuse only a unique exact-owner existing split anchor, preserve all live
  guards and prior specialization precedence, and keep absent/ambiguous
  cases on the original Python operation. Deterministically cap new
  additions at eight per function after all existing specialization
  decisions.
- Next action: integrate the validated retained change; future profitability
  work should investigate the unexplained repeated comprehensions
  regression, material native-code growth, and unmet stock **1.10x** goal.

## Attempt 2: specialize layout-uniform polymorphic non-self instance fields

- Current status: **RETAIN; fully validated landing candidate;
  authoritative full correctness gate GREEN**. This is a chronological
  reopening of
  the already-retained strategy; **Attempt 1 and its complete historical
  verdict, regressions, generated-code growth, measurements, and full gate
  remain preserved above**.
- Pacific date: **2026-08-19 PDT**.
- Current integrated baseline: retained `main` change **`vosvuxuw`**,
  commit **`9ad7d7dc`**.
- Candidate: current change **`wrzzyrtx`**, initially observed at mutable
  commit **`ef824f3b`**, described as **`Specialize layout-uniform
  polymorphic non-self instance fields`**; snapshots can change the
  commit ID.
- No existing user-visible CPython behavior bug is claimed. Actual
  unchanged-production stock/transformed **Profile → Verify → Apply**
  semantic controls already pass; the genuine new transformed **RED** is
  solely missing source-specific indexed hits. Three independent
  unchanged-production transformed, whole-optimizer, and emitted-CFG
  **REDs** were established before implementation. An optimizer
  **RED → GREEN** was independently reported before the interruption,
  and its actual post-interruption focused whole-production rerun is now
  independently verified **GREEN: 1 passed / 213 filtered / 0.08
  seconds**. The real emitted-CFG regression is also independently
  verified **GREEN: 1 passed / 573 filtered**; the frozen genuine
  stock/Profile → Verify → Apply transformed regression is now
  independently verified **GREEN: 1 passed in 2.95 seconds**.

### Current hypothesis and source-grounded evidence

- Attempt 1 deliberately rejects globally ambiguous attribute ownership.
  Some real workloads nevertheless visit a small, closed set of
  same-module exact owners whose existing constructor anchors all prove
  the **same attribute index**. Reusing those complete owner sets with
  their existing exact runtime guards may safely convert currently generic
  polymorphic loads without treating an attribute name alone as proof.
- Current retained richards zero-loss profile contains **255 samples**.
  The disjoint generic-attribute **leaf self** total is **9.803255%**;
  source partitions include **`Richards.run` 4.706082%** and
  **`Task.runTask` 1.568361%**. These partitions concern only the four
  selected generic-lookup leaves; they are not whole-stack ancestry, a
  speedup prediction, or proof that every access qualifies.
- Actual decoded profile records approximately **81,634 generic schedule
  `t.link` loads**. All five observed same-module concrete owners
  **`DeviceTask`, `HandlerTask`, `IdleTask`, `Packet`, and `WorkTask`**
  locate `.link` at exact split index **0**. The unrelated `Packet`
  owner is part of the real observed set and must **not** be omitted for
  convenience. All five similarly locate `.ident` at exact index **1**.
- The four task descendants, excluding unrelated `Packet`, share matching
  `.priority`, `.input`, and `.handle` indices **2 / 3 / 7**. Deltablue
  exposes only `.my_output` / `.satisfied` across two same-index owners;
  current chaos and comprehensions show **no corresponding opportunities**.
  Do not promise their improvement.
- Existing **`AmbiguousLeft` / `AmbiguousRight`** place the same attribute
  at unequal indices **0 / 1**; this case remains genuinely ambiguous and
  must stay on the original generic Python access. Foreign-module,
  unanchored, slotted, descriptor-backed, hook-observed, missing, cold,
  or incompletely observed owners must likewise remain generic.

### Current implementation and compatibility boundaries

- The current production implementation is present in **exactly two
  existing files**:
  **`crates/soac_opt/src/pipeline_v3.rs`** and
  **`crates/soac_jit/src/jit/mod.rs`**. Root additionally authorizes only
  a narrow **`#[cfg(test)]`-only** assertion in existing
  **`crates/soac_jit/src/jit/test.rs`** to access its existing private
  real Cranelift harness and prove one shared emitted live split probe.
  This is a third Rust **test-only** path, not a third runtime production
  file.
- The implemented optimizer groups profile-derived indexed layouts by
  original instruction source and access kind. Its new polymorphic case
  admits only genuinely hot **LOAD** sites with **two through five exact
  same-module owners**, one complete existing split-field constructor
  anchor per distinct owner, one attribute name, and **identical expected
  field indices**. Any foreign owner, repeated owner, missing anchor,
  mixed index, polymorphic store, or owner count above five rejects the
  complete candidate. The five-owner case includes unrelated `Packet`;
  no partial owner group is retained.
- The existing minimum **eight** profile observations remains in force.
  Existing unique-owner candidates retain priority; the original cap of
  **eight distinct additional non-self source sites per function** now
  counts a complete polymorphic owner group as one source. Ties and
  selected owners are ordered deterministically. Existing scalar, self,
  inherited, unique-owner, store, instrumentation, and optimization-plan
  precedence remains unchanged; no owner cell or layout fact is invented.
- The implemented mechanical JIT first checks that all selected
  polymorphic split owners have the same expected index. Every owner
  retains its existing independent weak-owner identity and live
  type-version guard. Successful exact-owner branches then converge into
  **one matched-owner block parameter (phi)**; the selected owner alone
  feeds **one shared live split-key guard and one shared inline-values
  field probe**. Existing hook/descriptor safety, key/value/layout
  validation, and untouched generic fallback remain in place. The
  existing structured CFG regression counts real Cranelift loads of
  `PyHeapTypeObject.ht_cached_keys`; its independently verified
  post-interruption result is **1 passed / 573 filtered**, **0.10 seconds
  test runtime / 0.49 seconds total**, with exactly **one** live shared
  probe for all five independently guarded owners.
- Preserve live class/MRO mutation, subclass/custom `__getattribute__`,
  data descriptors, dictionary promotion / key changes, missing fields,
  finalization, side effects, error propagation, and weak-owner lifetime.
  Every unsupported or changed state takes the existing generic operation;
  do not add a public API, exported runtime helper, mutable global, or
  public IR operation.
- New unchanged-production integration
  **`tests/test_uniform_polymorphic_nonself_fields.py`** establishes a
  genuine optimization-only transformed **RED: 1 failed in 2.79 seconds**
  (**3.162 seconds outer pytest**). Actual stock and transformed
  **Profile → Verify → Apply** observable semantics all pass. Profile
  proves exactly five owners **`Left`, `Right`, `Third`, `Fourth`, and
  `Packet`**, every `.link` at split index **0**. Existing
  unique/inherited hits, mixed-index **`Left` / `Right` 0 / 1**,
  six-owner rejection, cold sites, slots, unanchored owners, generic
  stores, hooks/properties, MRO/class/dictionary mutations, finalizers,
  and actual native direct bodies all pass.
- The sole final specialization-counter assertion sees
  **`Consumer.read '#0' = 0` instead of at least `160`** and
  **`read_uniform '#0' = 0` instead of at least `320`**. This is a
  genuine production-path indexed-hit RED, **not** a user-visible
  CPython behavior mismatch.
- The frozen real transformed integration has now turned that genuine
  optimization-only counter failure **GREEN: 1 passed in 2.95 seconds**
  (**2.92 seconds inner runtime**). Actual stock and transformed
  Profile → Verify → Apply execution agrees, all five exact owners
  including unrelated `Packet` retain the same proven index, and real
  Verify counters record indexed hits at both original source sites.
  Existing unique and inherited specializations, mixed indices, more
  than five owners, foreign owners, slots, dynamic hooks, MRO/property
  changes, dictionary mutations, and finalizers all preserve CPython
  behavior. No baseline user-visible CPython bug is claimed.
- The transformed check first required a one-time debug-extension build
  of **26.92 seconds**. This is workflow/build setup overhead, not the
  reported test runtime, an optimized-workload measurement, or a
  performance result.
- A second genuine unchanged-production whole-planner structured **RED**,
  **`hot_nonself_uniform_split_fields_reuse_every_exact_owner_with_bounded_sources`**,
  exercises real lowered Python plus an encoded counter profile and the
  complete production optimization path. Existing unique / inherited
  controls, mixed split indices **0 / 1**, foreign owners, missing anchors,
  more than five owners, cold observations, and unchanged generic stores
  all pass first. The sole intended final decision failure finds actual
  **0** same-index owner plans instead of the required **5**.
- An earlier tiny `HashSet::collect` type-inference mismatch was confined
  to the draft test fixture and corrected **before** this genuine
  assertion RED; it was not a production optimization failure.
- The focused whole-production optimizer regression has now independently
  turned its genuine **0 → 5** exact-owner decision failure **GREEN:
  1 passed / 213 filtered / 0.08 seconds**. The current rerun verifies
  all five existing exact owner anchors, the cap of eight distinct hot
  source sites, retained unique/inherited decisions, and mixed-index,
  foreign-owner, missing-anchor, over-cap, cold, and generic-store
  negative controls. The 213 filtered tests are not a full optimizer
  suite.
- A third independent genuine unchanged-production structured emitted-CFG
  **RED**,
  **`uniform_polymorphic_late_owner_loads_share_one_live_split_key_probe`**,
  now executes the actual specialized typed JIT builder with **five real
  `TypedAttrAccessPlan` exact-owner guards**. In the existing
  **`#[cfg(test)]`-only `crates/soac_jit/src/jit/test.rs`** harness, it
  inspects actual Cranelift **`ir::Function` layout / DFG** and counts
  **`Opcode::Load`** at
  **`offset_of(PyHeapTypeObject, ht_cached_keys)`**. The real emitted
  function has **5** live split-key probes instead of the required shared
  **1**. This is structured production codegen evidence—not renderer
  text, instrumentation, or a fabricated production stub.
- The actual candidate emitted-CFG regression has now independently
  turned that genuine **5 → 1** probe failure **GREEN: 1 passed / 573
  filtered**, with **0.10 seconds** test runtime and **0.49 seconds**
  total. The real five-owner typed plan emits exactly **one** live
  `ht_cached_keys` probe; no broad JIT-suite result is implied by the
  573 filtered tests.
- Package-scoped formatting of **`soac_opt` and `soac_jit`** has
  completed and its separate package-scoped format check is **GREEN**.
  After formatting, the complete optimizer library passes **214 / 214
  tests**, the complete JIT library passes **574 / 574 tests**, and the
  complete typed-IR library passes **54 / 54 tests**; JIT test execution
  takes **5.60 seconds**. Broad transformed compatibility passes
  **16 / 16 in 37.28 seconds**, and combined optimizer/JIT test-target
  checking passes in **3.69 seconds**. The optimizer's **13.81-second
  rebuild**, JIT's **26.00-second compile**, and second debug-extension
  rebuild of **21.57 seconds** are workflow/build setup costs, not
  workload measurements or optimization performance evidence.
- The new dedicated polymorphic fixture explicitly proves same-index
  **0**. It does not yet contain a distinct dedicated nonzero-index
  polymorphic regression. Real candidate richards direct bodies for
  `Task.addPacket` / `Task.release` contain source-grounded `priority`
  opportunities at index **2**, and `Task.qpkt` contains an `ident`
  opportunity at index **1**. These are actual changed source-body
  evidence, **not per-site specialization counters**; dedicated
  nonzero-index fixture coverage and confirmation for indices **3 / 7**
  remain **PENDING**.
- Both approved production implementations and the existing
  **`#[cfg(test)]`-only** CFG assertion are now visible in the working tree.
  The post-interruption focused whole-production optimizer and actual JIT
  CFG regressions and frozen real transformed Profile → Verify → Apply
  regression are independently verified **GREEN**. Scoped formatting
  and format checking, all optimizer/JIT/typed-IR libraries, broad
  transformed compatibility, and combined Rust/test-target checking are
  complete. Eight-workload mode-matched release smoke also passes. A
  dedicated nonzero-index polymorphic fixture / complete actual richards
  nonzero-index audit remains future follow-up; normally sampled and
  clean repeated candidate performance are verified, and the
  authoritative full correctness gate is **GREEN**.

### Current benchmark protocol and retained baseline

- Retained release smoke **174435** completes all eight benchmarks with
  **2,238,468 native bytes / 147,712 blocks / 36,500 hidden trampoline
  bytes**.
- The completed candidate release debug-single smoke
  **`comparison-20260819-185033-swtmUh`** passes all **eight** actual
  Apply worker PIDs against that mode-matched retained **174435** result.
  All **397 total JIT source rows, including adapters**, contain exactly
  **204 direct-function-body rows**; source identities and optimized
  typed coverage remain identical at **2,866 typed blocks / 204
  functions**. No worker reports an error. Hidden trampoline bytes remain
  exactly **36,500**. Total emitted native code changes
  **2,238,468 → 2,238,412 bytes (-56 bytes)** and
  **147,712 → 147,769 machine blocks (+57)**.
- `chaos`, `comprehensions`, `fannkuch`, `float`, `nbody`, and
  `spectral_norm` have byte-for-byte and block-for-block identical direct
  bodies. `deltablue` changes **456,944 → 455,284 native bytes (-1,660)**
  across five shrinking bodies. `richards` changes **347,408 → 349,012
  bytes (+1,604)** and adds **154 machine blocks** across exactly ten
  changed actual direct bodies: `schedule` **6,804 / 459 → 7,364 / 490
  bytes / blocks**; `Richards.run` **180,128 / 12,595 → 180,888 /
  12,641**; `Task.runTask` **10,308 / 615 → 8,880 / 547**;
  `Packet.append_to` **3,332 → 4,268 bytes**; `Task.addPacket`
  **8,864 → 7,788**; `Task.hold` **5,072 → 4,424**; `Task.release`
  **2,632 → 2,608**; `Task.qpkt` **4,588 → 4,636**; `IdleTask.fn`
  **11,848 → 12,416**; and `WorkTask.fn` **26,812 → 28,720**.
- Cold debug-single richards values **34.16 versus 26.23 milliseconds**
  are **explicitly invalid performance evidence**: this smoke verifies
  release execution and generated-code coverage, not representative
  speed, regression, or improvement. No candidate performance claim is
  made before independent normally sampled and repeated comparisons.
- Retained normally sampled fixed-eight comparison **174639** reports
  stock **0.6345791409139968x** and previous SOAC
  **1.0532525776372081x**. Actual Apply coverage is **23,163,480 native
  bytes / 1,524,480 blocks / 365,000 hidden trampoline bytes**. The
  earlier comparator carried noise; report worker-level confidence and
  stock drift rather than treating its mean as a guarantee.
- Candidate normally sampled fixed-eight comparison
  **`comparison-20260819-185353-AwqE0f`**, against retained **174639**,
  completes all eight workloads and all **80 actual Apply worker PIDs**.
  Its official stock score is **0.6672361371916246x**, versus retained
  **0.6345791409139968x**; official changed/previous SOAC improvement is
  **1.076213366589749x**. Its **3,970 total JIT source rows, including
  adapters**, contain exactly **2,040 direct-function-body rows**; every
  worker preserves exact source identities/counts, **2,866 typed blocks /
  204 functions**, and zero errors. Hidden trampolines remain exactly
  **365,000 bytes**; emitted native code changes
  **23,163,480 → 23,159,960 bytes (-3,520)** and
  **1,524,480 → 1,524,970 machine blocks (+490)**.
- All direct bodies in six unaffected fixed-eight workloads remain
  byte-for-byte and block-for-block identical. Existing `deltablue`
  native code changes **4,627,960 → 4,608,000 bytes (-19,960)** across
  six existing direct bodies that shrink through the shared probe.
  `richards` changes **3,954,720 → 3,971,160 bytes (+16,440)** across
  ten actual direct bodies; representative changed functions include
  `schedule` **7,364 → 7,820 bytes**, `Task.runTask`
  **10,308 → 8,880 bytes**, and `Richards.run`
  **184,212 → 185,116 bytes**. No generated function disappears and no
  untransformed hot-path claim is inferred from completion alone.
- Worker-robust `richards` latency changes **25.4559 → 23.5100 ms**,
  raw **1.08277x** with **95% confidence interval 1.04273–1.11631x**;
  however, the candidate's paired stock CPython is approximately **6%**
  faster too, leaving stock-adjusted **1.01869x** with interval
  **0.97808–1.06599x**. Because that interval includes parity, this is
  **not a definitive richards improvement**. `deltablue` changes
  **2.5413 → 2.3253 ms**, raw **1.09288x**
  (**1.06488–1.17315x**), and stock-adjusted **1.15613x**
  (**1.09567–1.22546x**). Raw apparent `chaos` and `comprehensions`
  improvements occur despite byte/block-identical generated bodies and
  are explicitly attributed to environmental drift, not the candidate.
- Retained clean repeated four-workload comparison **175000** reports
  stock **0.4865323207896451x** and previous SOAC
  **1.023626052523357x**. Three-round Apply coverage is **54,697,320
  native bytes / 3,594,960 blocks / 746,520 hidden trampoline bytes**.
  The fixed subset remains **chaos, comprehensions, deltablue, and
  richards**; richards is the primary source-backed target.
- Completed candidate clean three-round fixed-four comparison
  **`comparison-20260819-185725-iJQ74K`**, against retained **175000**,
  reports official stock **0.5139251222980681x** and previous SOAC
  **1.0654218950545014x**. All **120 actual Apply worker PIDs / 10,650
  total JIT source rows, including adapters**, contain exactly **5,490
  direct-function-body rows**, preserve exact source identities, report
  zero errors, and retain **2,265 typed blocks / 183 functions**. Hidden
  trampolines remain exactly **746,520 bytes**; native code changes
  **54,697,320 → 54,686,760 bytes (-10,560)** and machine blocks change
  **3,594,960 → 3,596,430 (+1,470)**. Six existing `deltablue` bodies
  shrink by **59,880 bytes** in total while ten `richards` bodies grow by
  **49,320 bytes**; every `chaos` and `comprehensions` body remains
  exactly unchanged.
- Definitive clean repeated `richards` latency improves
  **25.707173 → 23.625606 ms**, raw **1.088106x** with **95% interval
  1.069411–1.117355x** and paired-stock-adjusted **1.070336x** with
  **95% interval 1.043181–1.107330x**. Every independently started
  round improves: raw **1.108967x / 1.068209x / 1.081729x** and paired
  **1.103835x / 1.039361x / 1.076220x**. This clean repeated result,
  unlike the noisy normally sampled fixed-eight paired interval,
  supports a real richards improvement.
- Clean repeated `deltablue` changes **2.487616 → 2.468877 ms**, raw
  **1.007590x (0.989368–1.033818x)** and paired-stock-adjusted
  **0.974161x (0.946963–1.002748x)**. Both intervals include parity:
  `deltablue` is **NEUTRAL**, and its stronger single-round normal
  result must not be claimed as a reproducible improvement. Unchanged
  `chaos` code nevertheless reports raw **1.038179x** / paired
  **1.046299x**, an environmental artifact; `comprehensions` raw
  **1.016239x** / paired **1.002620x** is **NEUTRAL**.
- Baseline optimized coverage remains **2,866 typed blocks / 204
  functions** for the fixed eight and **2,265 blocks / 183 functions** per
  targeted round. Candidate smoke preserves fixed-eight typed coverage,
  direct-source identities/counts, and hidden bytes; mode-matched native
  bytes fall **56** while blocks rise **57**. Normally sampled fixed-eight
  native bytes fall **3,520** while blocks rise **490**, with identical
  typed/direct/hidden coverage. Clean three-round native bytes fall
  **10,560** while blocks rise **1,470**, with all source identities,
  typed coverage, and hidden trampolines unchanged. Workload-site
  indexed counters and complete nonzero-index auditing remain pending.
- Completed matched lossless richards causal profiling compares **255
  retained / 244 candidate samples** from the same measured worker,
  **100 replay loops / 99 Hz / `SOAC_JIT_BB_MAP=0`**, with **zero lost
  samples**. The primary **disjoint four-symbol generic-attribute leaf
  self** total falls **9.803255% → 4.099016% (-5.704239 percentage
  points)**. Retained components are `_PyObject_TryGetInstanceAttribute`
  **5.098173%**, `_PyObject_GenericGetAttrWithDict` **3.528812%**,
  `PyObject_GetAttr` **0.784180%**, and its PLT **0.392090%**; candidate
  components are respectively **1.229705% / 2.869311% / 0% / 0%**.
- Correct disjoint source partitions of those same four leaves change
  `Richards.run` **4.706082% → 1.229705%**, `Task.runTask`
  **1.568361% → 0.819803%**, `Task.release` **0.784180% → 0%**,
  `Task.addPacket` **0.392090% → 0%**, and `Task.qpkt`
  **0.392090% → 1.229705%**. Lookup guard work moves into JIT direct
  bodies; a rising individual source partition does not erase the net
  disjoint reduction.
- As a **separate overlapping whole-stack metric**, generic
  `PyObject_GetAttr` ancestry excluding `_PyObject_GetMethod` changes
  **14.900427% → 9.017836% (-5.882591 percentage points)**. Distinct
  `_PyObject_GetMethod` inclusive ancestry changes **7.841804% →
  9.016836%** and remains a different bottleneck. Do not add nested
  inclusive ancestry to disjoint leaves or interpret the two measures as
  independent gains.
- The authoritative full `just test-all` gate is **GREEN**; evidence is
  recorded in **`work/logs/uniform-polymorphic-nonself-test-all.log`**.
  It passes **1,234 transformed Python nodeids / 97 isolated file
  batches / 8 workers**, with **97 PASS / 0 failures**. Workspace Rust
  libraries pass JIT **574**, optimizer **214**, typed IR **54**,
  lowering **371**, and PyO3 **8**. Cargo compile takes **51.34
  seconds**, the Cargo test phase **68.796 seconds**, pytest inner /
  outer **74.030 / 74.043 seconds**, and total test phase **142.853
  seconds**. The new transformed regression passes in **2.52 seconds**;
  the preexisting **28-node counter-dump batch takes 73.32 seconds** and
  dominates Python elapsed time. The implementation is a validated
  **RETAIN / LANDING CANDIDATE**. The full pyperformance suite is
  unmeasured and its stock **1.10x** objective remains unmet.

| Attempt 2 metric | Current retained baseline | Candidate | Interpretation |
| --- | --- | --- | --- |
| Fixed-eight stock / previous score | 0.6345791409139968x / 1.0532525776372081x | 0.6672361371916246x stock / 1.076213366589749x previous SOAC | raw aggregate reflects material paired-stock/environment drift |
| Clean repeated fixed-four stock / previous score | 0.4865323207896451x / 1.023626052523357x | 0.5139251222980681x stock / 1.0654218950545014x previous SOAC | richards improves in all three raw and paired rounds |
| Mode-matched actual Apply release smoke / source coverage | 2,238,468 bytes / 147,712 blocks / 36,500 hidden bytes | GREEN 8 / 8; 2,238,412 bytes / 147,769 blocks / 36,500 hidden bytes | identical direct-source IDs/counts and 2,866 typed blocks / 204 functions; cold timings are invalid performance evidence |
| Fixed-eight native bytes / blocks / hidden bytes | 23,163,480 / 1,524,480 / 365,000 | 23,159,960 / 1,524,970 / 365,000 | all 80 Apply PIDs preserve exact source IDs/counts and typed coverage |
| Fixed-eight robust richards previous / stock-adjusted | 25.4559 ms retained SOAC | 23.5100 ms; raw 1.08277x [1.04273, 1.11631]; paired 1.01869x [0.97808, 1.06599] | paired interval includes parity; no definitive richards improvement |
| Fixed-eight robust deltablue previous / stock-adjusted | 2.5413 ms retained SOAC | 2.3253 ms; raw 1.09288x [1.06488, 1.17315]; paired 1.15613x [1.09567, 1.22546] | single-round result is not reproduced in clean three-round evidence |
| Clean repeated richards previous / stock-adjusted | 25.707173 ms retained SOAC | 23.625606 ms; raw 1.088106x [1.069411, 1.117355]; paired 1.070336x [1.043181, 1.107330] | all three independent raw and paired rounds improve |
| Clean repeated deltablue previous / stock-adjusted | 2.487616 ms retained SOAC | 2.468877 ms; raw 1.007590x [0.989368, 1.033818]; paired 0.974161x [0.946963, 1.002748] | NEUTRAL; both intervals include parity |
| Repeated native bytes / blocks / hidden bytes | 54,697,320 / 3,594,960 / 746,520 | 54,686,760 / 3,596,430 / 746,520 | all 120 Apply PIDs / 10,650 JIT source rows, including adapters / 5,490 direct bodies preserve typed coverage |
| Matched lossless richards disjoint generic-attribute leaf self | 255 samples / 9.803255% | 244 samples / 4.099016%; -5.704239 percentage points | same worker / 100 loops / 99 Hz / no block maps / zero lost; do not add separate ancestry |
| Matched separate non-GetMethod whole-stack generic ancestry | 14.900427% | 9.017836%; -5.882591 percentage points | overlapping inclusive metric; GetMethod ancestry 7.841804% → 9.016836% is distinct |
| Actual schedule `t.link` generic loads | approximately 81,634; five exact owners all index 0 | pending | Packet must remain in complete owner set |
| Baseline stock/transformed observable behavior | actual Profile/Verify/Apply semantics GREEN; no existing CPython mismatch | independently verified stock/Profile/Verify/Apply GREEN | transformed failure is specialization-only |
| Real transformed polymorphic indexed-hit decision | genuine 1 failed / 2.79 s; Consumer.read 0 vs 160 and read_uniform 0 vs 320 | independently verified GREEN; 1 passed / 2.95 s, 2.92 s inner | five owners including Packet all index 0; actual source hits and all semantic controls pass |
| Whole-production polymorphic optimizer decision | genuine structured RED; actual same-index owner plans 0 versus 5 | independently verified GREEN; 1 passed / 213 filtered / 0.08 s; all five owners and eight-site cap | real lowered Python/profile; unique/inherited/mixed/foreign/missing/>5/cold/store controls pass |
| Actual shared-emitted-CFG decision | genuine real Cranelift structured RED; actual ht_cached_keys loads 5 versus 1 | independently verified GREEN; 1 passed / 573 filtered; 0.10 s runtime / 0.49 s total; exactly one live probe | existing cfg(test)-only JIT harness / five real typed owner plans; exactly two runtime production paths |
| Post-format full optimizer / JIT / typed-IR libraries | retained baseline passed | GREEN 214 / 214 optimizer; 574 / 574 JIT, 5.60 s JIT execution; 54 / 54 typed IR | 13.81 s optimizer rebuild / 26.00 s JIT compile are workflow-only overhead |
| Broad transformed compatibility / combined test-target check | retained baseline passed | GREEN 16 / 16 in 37.28 s; combined cargo check --tests GREEN 3.69 s | second 21.57 s debug-extension rebuild is workflow-only |
| Scoped optimizer / JIT formatting and format check | retained baseline passed | both packages formatted and scoped format-check GREEN | no full-workspace formatting claim |
| Dedicated nonzero-index polymorphic fixture / actual richards index audit | known actual richards indices 1 / 2 / 3 / 7 | changed direct bodies expose actual ident index 1 and priority index 2; dedicated fixture and indices 3 / 7 pending | source-body evidence is not per-site specialization-counter proof |
| Full `just test-all` correctness gate | retained baseline passed | GREEN 1,234 nodeids / 97 PASS / 0 failed / 8 workers; JIT 574, optimizer 214, typed 54, lowering 371, PyO3 8 | total 142.853 s; new regression 2.52 s; existing 28-node batch 73.32 s |

### Attempt 2 verdict and next action

- Verdict: **RETAIN / VALIDATED LANDING CANDIDATE; original Attempt 1 remains
  LANDED / RETAINED;
  genuine unchanged-production transformed same-index polymorphic
  optimization-only RED 1 / 2.79 seconds with all CPython semantics
  passing; independent genuine whole-production optimizer RED 0 versus 5
  owner plans independently verified GREEN 1 passed / 213 filtered /
  0.08 seconds; independent genuine actual emitted-CFG RED 5 versus 1
  live split-key probes independently verified GREEN 1 passed / 573
  filtered, 0.10 seconds runtime / 0.49 seconds total; two-file runtime
  implementation present; genuine frozen transformed optimization
  RED → GREEN independently verified 1 passed / 2.95 seconds;
  scoped formatting / format-check GREEN; full post-format optimizer /
  JIT / typed-IR libraries GREEN 214 / 214, 574 / 574, and 54 / 54;
  broad transformed compatibility GREEN 16 / 16 in 37.28 seconds;
  combined test-target check GREEN 3.69 seconds; mode-matched release
  smoke GREEN 8 / 8 with identical source identities and -56 native
  bytes / +57 blocks; normally sampled fixed-eight GREEN 80 / 80 actual
  Apply PIDs, stock 0.6672361371916246x and previous SOAC
  1.076213366589749x, -3,520 native bytes / +490 blocks; deltablue
  single-round stock-adjusted 1.15613x is NOT reproduced; noisy
  single-round richards paired 1.01869x includes parity; definitive
  targeted richards 25.707173 → 23.625606 ms, raw 1.088106x
  [1.069411, 1.117355], paired 1.070336x [1.043181, 1.107330]; all
  three raw and paired rounds improve; clean repeated deltablue /
  comprehensions NEUTRAL; unchanged-code chaos movement is
  environmental; fixed-four stock 0.5139251222980681x / previous SOAC
  1.0654218950545014x, -10,560 native bytes / +1,470 blocks across
  120 actual Apply PIDs; source-body evidence for nonzero indices
  1 / 2; matched zero-loss causal richards 255 / 244 samples reduces
  disjoint generic-attribute leaves 9.803255% → 4.099016%
  (-5.704239 percentage points); authoritative full correctness gate
  GREEN 1,234 nodeids / 97 isolated batches / 8 workers / 0 failures,
  JIT 574 / optimizer 214 / typed 54 / lowering 371 / PyO3 8;
  dedicated nonzero-index coverage / complete index audit remain future
  follow-up; full-suite stock 1.10x remains unmet**.
- Transferable lesson: owner ambiguity is not automatically unsafe when a
  complete bounded set of same-module anchored exact owners proves one
  identical layout index, but every owner—including unrelated concrete
  classes—and all existing mutable-runtime guards remain mandatory.
- Next action: integrate the fully validated retained change, then
  continue toward the unmeasured full-suite stock **1.10x** objective.

## Attempt 3: recover polymorphic non-self fields with distinct owner indices

- Status: **TWO GENUINE WHOLE-PRODUCTION STRUCTURED AND TWO REAL
  TRANSFORMED MIXED-INDEX REDS GREEN; THREE FOCUSED TRANSFORMED
  GUARDRAILS, FOUR COMPLETE RUST SUITES, AND BROAD TRANSFORMED SELECTION
  PASS; CLEAN THREE-ROUND PERFORMANCE RECORDED; AUTHORITATIVE FULL GATE
  GREEN; VALIDATED CANDIDATE RETAINED**.
  Attempts 1 and 2, their retained
  decisions, genuine RED-to-GREEN evidence, historical adverse results, and
  full correctness gates remain unchanged above.
- Pacific selection date: **2026-08-20 PDT**.
- Current compatibility baseline: the CPython-correct class-layout candidate
  `lwyqsqsm`, fresh same-kernel fixed-four artifact
  `work/pyperformance/comparison-20260820-091112-iaT71z/summary.json`.
  That candidate is a required user-visible correctness repair, not a license
  to restore the earlier incorrect class layouts. Its authoritative full
  correctness gate remains pending.
- General-purpose hypothesis: independently guarded exact concrete Python
  classes may store one attribute at distinct shared-dictionary indices. A
  complete, bounded, Profile-proven same-module owner set remains safe to
  specialize when each existing exact-owner cell proves its **own** live
  split index. Uniform indices are a profitable shared-probe case, not a
  semantic requirement for guarded polymorphic reads. This extends the
  existing non-self field strategy; it is not a new strategy or a
  benchmark-specific rule.
- Causal real-profile evidence: corrected CPython lexical static-attribute
  sorting preserves every existing **11 concrete owners / 51 type keys** and
  all **662 `__main__` Profile counter rows**. It changes
  `IdleTaskRec.count` / `WorkerTaskRec.count` from **`1 / 1`** to
  **`1 / 0`**, task/packet `.ident` from five owners all **`1`** to four
  exact task owners **`1`** plus `Packet` **`2`**, and task/packet `.link`
  from five owners all **`0`** to four exact task owners **`0`** plus
  `Packet` **`4`**. Do not omit the unrelated but actually observed
  `Packet` owner. The preexisting owner evidence is split across
  `soac.runtime` **`4 owners / 32 keys`** and `__main__`
  **`7 owners / 19 keys`**, which the normal production evidence store
  already merges.
- Important unchanged observations include `WorkTask.fn`
  **13 field sites / 121,718 generic observations**,
  `Packet.append_to` **4 / 81,416**, `Task.qpkt` **8 / 371,936**, and
  `Task.runTask` **11 / 627,240**. The existing
  `late_bound_split_owner_nonself_field_plans` helper in
  `crates/soac_opt/src/pipeline_v3.rs` rejects a complete owner group when
  any `candidate.expected_index != field.expected_index`. Existing
  `emit_typed_late_bound_owner_getattr` in
  `crates/soac_jit/src/jit/mod.rs` already distinguishes the optimized
  same-index shared-probe case from the safe per-owner-index fallback branch;
  each branch retains its exact weak-owner, live type-version, current
  split-key identity/name/index, valid inline-values, and original generic
  fallback guards. No new JIT runtime path, owner cell, hidden trampoline,
  mutable global, ABI field, helper, or public API is expected.
- The real class-correct baseline runs **three order-alternating
  same-kernel rounds / four workloads / 120 actual measured Apply workers**.
  Its stock geometric score is **`0.5401772590486644x`** and its previous
  pre-correction SOAC score **`0.9744429326747311x`**. Repeated `richards`
  is **`21.672169 -> 22.405269 ms`**, raw **`0.958229x`**
  (**95% `0.939852-0.979582`**) and stock-paired **`0.940916x`**
  (**95% `0.918484-0.965860`**); all three paired rounds regress.
  Candidate recovery comparisons must use fresh profile evidence from this
  same-kernel class-correct baseline rather than the pre-correction
  revision. Fixed-four typed IR is **`2,265 blocks / 183 functions`**;
  all three baseline rounds emit **`56,797,080 native bytes /
  3,716,400 machine blocks / 777,240 hidden trampoline bytes`**.
- Predicted profitability risk: admitting mixed owners can add a separate
  exact-owner/key/value guard chain for each class. `deltablue`
  `Strength.stronger` / `Strength.weaker` repeatedly inspect
  `s1.strength` / `s2.strength`; if the global profiled `.strength` owner
  group becomes newly eligible, extra non-self guard code can harm that hot
  path despite recovering `richards`. `comprehensions` was an unchanged-code
  negative control during the class correction and must not be cited as a
  causal regression without new emitted-body evidence. Inspect actual
  source-level selected plans, typed sidecars, per-function emitted bytes,
  and clean stock-paired `chaos`, `comprehensions`, `deltablue`, and
  `richards` before retaining the extension.
- Actual production shape: the sole semantic change removes the
  `candidate.expected_index != field.expected_index` rejection from the
  existing optimizer helper. Stale uniform-only local, candidate-cap, and
  plan-reason names are renamed truthfully to polymorphic terminology;
  they do not change eligibility. Preserve its minimum **eight** Profile
  observations, complete **two-to-five** distinct exact same-module owners,
  matching attribute/access, one existing constructor anchor per owner,
  deterministic ordering, maximum **eight** additional non-self source sites
  per function, scalar/self/inherited/unique-owner precedence, and rejection
  of foreign owners, missing anchors, polymorphic stores, slots, cold sites,
  and unsupported owner groups. Continue to use the existing shared probe
  when all selected indices match; otherwise mechanically reuse the already
  implemented independently guarded per-owner-index branch.
- Genuine unchanged-production transformed RED: two existing real
  Profile -> Verify -> Apply integrations are first strengthened to require
  safe mixed-owner source hits. The layout-uniform regression's
  `read_mixed` records actual **`0` indexed hits instead of at least `64`**;
  the older non-self regression's `read_ambiguous` independently records
  actual **`0` instead of at least `64`**. Both expected concrete owners,
  their different split-key indices, and prior semantic controls are real;
  the frozen production planner still rejects the groups. The focused
  unchanged-production run genuinely reports **`2 failed in 4.45 seconds`**
  at the two intended specialization assertions, not a setup error or
  CPython-visible behavior mismatch. Restaging from the prior release
  benchmark unexpectedly rebuilds the unchanged debug extension for
  **`21.50 seconds`**; this is workflow-only overhead, not benchmark or
  test-runtime evidence.
- Independent genuine unchanged-production structured RED: focused
  `cargo test -p soac_opt hot_nonself_ -- --nocapture` reports exactly
  **`2 failed / 212 filtered / 0.04 seconds`**. The actual complete
  production lowering/Profile/optimizer path selects no mixed owner plans:
  actual **`[]`** versus expected
  **`[(MixedLeft, 0), (MixedRight, 1)]`** for the existing polymorphic
  regression, and independently actual **`[]`** versus expected
  **`[(Left, 0), (Right, 1)]`** for the older non-self regression. These
  are genuine typed whole-production decision failures before implementation,
  independent of the two real transformed
  **`2 failed / 4.45 seconds`** indexed-hit REDs. Preserve genuine
  same-index shared-probe, foreign/incomplete/>5 owner, cold/store,
  descriptor/hook/class-mutation, deleted/promoted dictionary, finalizer,
  single-evaluation, and untouched-fallback controls. After the one-line
  semantic eligibility correction, the same exact complete-production
  optimizer selection independently changes from
  **`2 failed / 212 filtered / 0.04 seconds`** to
  **`2 passed / 212 filtered / 0.04 seconds`**, with both actual mixed-index
  owner sets selected. The preceding **`11.21-second`** Rust rebuild is
  workflow-only overhead, not test execution or benchmark evidence.
- Independent real transformed RED-to-GREEN: the same frozen
  `read_mixed` and `read_ambiguous` cases first genuinely fail with **`0`
  indexed hits instead of at least `64`** each, then both pass with at least
  **`64` actual source-specific indexed hits** after the optimizer-only
  change. The existing inherited unequal-index / custom hook / descriptor /
  MRO guardrail also passes unchanged. The combined real
  Profile -> Verify -> Apply selection is **GREEN `3 / 3 in 6.16
  seconds`**, independently complementing structured optimizer
  **`2 passed / 212 filtered / 0.04 seconds`**. A preceding
  **`21.91-second`** unchanged debug-extension rebuild is workflow-only
  overhead, not benchmark or test-runtime evidence.
- Broader independently verified correctness: complete serial Rust suites
  pass **typed IR `54 / 54`**, **JIT `580 / 580`**,
  **lowering `372 / 372`**, and **optimizer `214 / 214`**. A real
  transformed **`34 / 34`** regression selection across **13 files** passes
  in **`19.24 seconds`**, including class static-attribute semantics,
  owner cells, profiled method dispatch, descriptors, object slots,
  inherited owners, and both newly restored mixed-index source hits.
  Package-scoped Rust formatting and its check pass; combined
  `cargo check -p soac_opt -p soac_jit --tests` passes in
  **`9.22 seconds`**. The authoritative full transformed and Rust
  correctness gate subsequently passes.
- Release smoke `comparison-20260820-102008-vOpSf8` completes **4 / 4**,
  with the expected **`2,265 typed blocks / 183 functions`**. It uses only
  one measured worker/value per workload; comparing it with the normally
  trained class baseline produces misleading `comprehensions`
  **`0.279586x`**, `deltablue` **`0.650070x`**, and geometric
  **`0.663412x`**. Reject all smoke throughput claims: different profile
  training also changes unrelated emitted bodies. Its useful structural
  observation is that both `Strength.stronger` and `Strength.weaker` grow
  from **`716 bytes / 45 blocks`** to **`3,256 bytes / 228 blocks`**;
  the anticipated separate-owner guard cost is real.
- Normally sampled `comparison-20260820-102120-8vVISD` completes **4 / 4**
  and reports stock **`0.5688287175126115x`** / previous class-correct
  SOAC **`1.046069203512862x`**. Actual same-mode `chaos` bodies are
  byte-identical; `comprehensions` changes only an unrelated annotation
  lambda by **`384 bytes`**. `deltablue` grows
  `Strength.stronger` **`2,540 bytes`**, `Strength.weaker`
  **`2,540 bytes`**, and `Planner.incremental_remove`
  **`2,168 bytes`**. The previously lost mixed-owner guards return to
  `WorkTask.fn` **`+2,664 bytes`**, `Richards.run` **`+2,624`**,
  `Packet.append_to` **`+2,188`**, `Task.runTask` **`+1,272`**,
  `Task.qpkt` **`+1,184`**, `schedule` **`+1,064`**, and
  `IdleTask.fn` **`+960`**. Single-round timings are not definitive.
- Clean same-kernel `comparison-20260820-102351-dmaMsN` completes
  **4 / 4** across **three order-alternating rounds / 120 measured Apply
  PIDs**. Stock geometric score improves from class-only
  **`0.5401772590486644x`** to **`0.5596865226885351x`**; official previous
  SOAC is **`1.0572879104903203x`**. Per-benchmark previous means are
  `chaos` **`1.053337x`**, `comprehensions` **`1.094981x`**,
  `deltablue` **`1.038808x`**, and `richards` **`1.042951x`**. The
  30-worker round-stratified `deltablue` comparison is raw
  **`1.0270x`** (**95% `1.0129-1.0544`**) but stock-paired only
  **`0.9948x`** (**95% `0.9747-1.0258`**); `richards` is raw
  **`1.0573x`** (**95% `1.0253-1.0950`**) but stock-paired only
  **`1.0098x`** (**95% `0.9815-1.0523`**). Both stock-adjusted affected
  workloads are therefore **NEUTRAL**, not established causal speedups.
  Unchanged-code `chaos` and `comprehensions`, including an apparent
  stock-paired comprehension **`1.0553x`**, expose ambient timing drift;
  do not credit them to mixed-index guards.
- The combined class-correctness plus mixed-index change is also compared
  with fresh integrated `main` artifact
  `comparison-20260820-090642-20GRdt`: stock score
  **`0.5564929785348224x -> 0.5596865226885351x`**, raw median SOAC
  geometric ratio **`1.0302667322x`**. Stock-paired `deltablue`
  **`1.0015x`** is neutral, while stock-paired `richards`
  **`0.9502x`** (**95% `0.9241-0.9896`**) remains adversely affected
  despite a raw **`1.0131x`** interval crossing parity. Disclose this
  unresolved paired regression rather than claiming complete recovery or
  treating a slightly better stock geometric score as full-suite progress.
- Median-per-round generated native code grows from the class-correct
  baseline **`18,932,360 -> 19,124,400 bytes`** (**`+1.0143%`**), and
  machine blocks grow **`1,238,800 -> 1,250,920`** (**`+0.978%`**).
  Across three rounds, ordinary native emission changes
  **`56,797,080 -> 57,373,200 bytes`** (**`+576,120`**) while all
  **120 actual Apply PIDs / 5,490 direct bodies / 5,160 default-direct
  adapters** preserve their source identities. The **`483,840 visible
  adapter bytes`** are already included in native totals. Binary parsing
  separately proves **`777,240 true hidden vectorcall trampoline bytes`**
  remain identical across parent, class-only, and mixed-index candidates;
  ordinary-plus-hidden totals change **`57,574,320 -> 58,150,440` bytes**.
  Final repeated `chaos` and `comprehensions` bodies are byte-identical.
  Typed coverage remains **`2,265 blocks / 183 functions`**,
  pre-optimization BlockPy **`8,285,072 bytes`**, and project-module
  coverage only `__main__` plus `soac.runtime`; no standard-library or
  third-party dependency hot path is transformed.
- Authoritative final combined `just test-all` gate: **GREEN**, recorded
  in `work/logs/class-static-mixed-owner-test-all.log`: **1,259 transformed
  Python nodeids / 101 isolated batches / 8 workers / 101 PASS / 0
  failures**, including all **20 class-static cases** and both mixed-index
  integrations. Rust passes **54 typed IR**, **580 JIT in 15.02 seconds**,
  **372 lowering in 0.51 seconds**, **214 optimizer in 0.57 seconds**,
  and **8 PyO3 in 0.10 seconds**. Debug runtime setup takes
  **24.716 seconds**, Cargo tests **81.230 seconds**, inner / outer pytest
  **78.940 / 78.954 seconds**, and total test phase **160.193 seconds**;
  the existing 28-node counter shard takes **78.22 seconds**. Retain the
  validated correctness and safe mixed-index guards, while preserving the
  disclosed parent-relative paired `richards` regression; do not claim the
  change is landed.
- Full-suite completion: only **8 of 97** acceptance benchmark variants
  have ever been compared, and no transformed standard-library or
  third-party hot module is demonstrated. The full-suite **`1.10x`** goal,
  full benchmark inventory and acceptance verdict remain **UNMET**.
