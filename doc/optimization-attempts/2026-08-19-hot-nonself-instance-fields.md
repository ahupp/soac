---
title: "Hot Non-Self Instance Field Specialization"
---

# Hot non-self instance field specialization

- Status: **LANDED / RETAIN; THREE-ROUND GAINS, MATCHED ZERO-LOSS PROFILES,
  AND FULL CORRECTNESS GATE VERIFIED; REPRODUCIBLE COMPREHENSIONS
  REGRESSION AND NATIVE-CODE GROWTH DISCLOSED**.
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
