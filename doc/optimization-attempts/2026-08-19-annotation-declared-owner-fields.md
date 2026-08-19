---
title: "Annotation-Declared Owner Fields"
---

# Annotation-declared owner fields

- Status: **REJECTED; GENUINE STRUCTURED AND TRANSFORMED RED-TO-GREEN,
  BUT REPEATED TARGET THROUGHPUT SIGNIFICANTLY REGRESSES AND MATCHED
  ZERO-LOSS PROFILE CONFIRMS GUARD COST; PRODUCTION / SPECIALIZATION /
  TEST CHANGES VERIFIED RESTORED; NO FULL GATE OR PERF LOG**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`puozyplw`**, commit
  **`4046c825`**, a **documentation-only** child of production
  guarded-runtime change **`zwkrytkq/443b2e42`**; both revisions run the
  same production code.
- Candidate revision: change **`yywuowlk`**, commit **`0a1d31fb`**;
  no production implementation, passing candidate, or measured gain.
- Outcome: determine whether literal compiler-generated annotation metadata
  can safely anchor profiled dataclass/annotated split-dictionary fields,
  without stale decorator-era type guards or observable annotation effects.

## Hypothesis and evidence

- General-purpose opportunity: annotation-declared classes, including
  dataclasses with generated initializers, may expose stable field names
  through an existing compiler-generated annotation thunk even when no
  transformed source constructor writes those fields. Existing exact-owner
  split-dictionary field plans could reuse bounded annotation-derived
  anchors without a new profile schema, typed-plan concept, helper, or
  runtime ABI.
- Current guarded-runtime comprehensions zero-loss profile
  `work/logs/guarded-indexed-runtime-factory-candidate-comprehensions_*`
  contains **618 raw samples**. Generic attribute access is approximately
  **8.577%**, but the hot `_dp_listcomp_20` `dict.get` subtree alone is
  approximately **2.9–3.1%** and is **not a `Widget` field**; it must not
  be included in the proposed optimization ceiling. Actual `Widget`
  dataclass-field work has a conservative whole-workload gross opportunity
  of only approximately **2–3 percentage points**.
- An existing single-field owner guard previously increased generated code
  by approximately **748 native bytes / 67 machine blocks**. Annotation
  anchors may introduce additional guard/code overhead, so a throughput
  gain is not promised; reject if matched workload improvement does not
  outweigh native size and maintenance costs.
- Actual `Widget` compiler-generated `__annotate_func__`, function
  **`1:9`**, retains **six static tuple-pair field names plus metadata**
  even when the thunk is interpreted. This existing literal evidence, not
  execution of annotations or dynamic namespace inference, is the proposed
  bounded catalog source.
- Normal guarded-runtime fixed-eight comparison **090414** has stock
  geometric score **0.5883463026285985x**; targeted fixed-four comparison
  **090720** has stock geometry **0.4290269750586277x**. Current Apply
  coverage is **23,293,040 native bytes / 1,533,550 machine blocks** and
  **2,866 optimized typed blocks / 204 functions**. The authoritative
  full-pyperformance **1.10x stock** target remains unmet.
- No existing user-visible CPython correctness defect is claimed. The
  missing capability is profiled owner specialization for genuine
  annotation-declared fields without constructor-written anchors.

## Implementation and compatibility

- Proposed production scope: exactly two existing files,
  `crates/soac_opt/src/pipeline_v3.rs` and `crates/soac_jit/src/lib.rs`.
  Reviewer-created integration coverage is the new approximately
  **600-line**, host-`ast.parse`-clean
  `tests/test_annotated_owner_fields.py`; the existing
  `tests/test_late_owner_nonself_fields.py` unanchored annotation case may
  need an **intentional negative-to-positive** migration once production
  behavior genuinely changes. The new fixture exercises transformed
  `Profile -> Verify -> Apply` and adversarial semantics; its first
  first unchanged-production focused run genuinely fails **1 failed in
  2.53 seconds** at only the intended missing specializations. No production
  behavior has been changed.
- Extend the existing deterministic owner-field catalog only with capped
  same-owner anchors proven by literal compiler-generated annotation thunk
  **static tuple pairs**. Preserve exact source/access/index evidence,
  existing hot-profile admission, immutable typed plans, and existing JIT
  guarded indexed loads/stores. Do not execute annotations, resolve
  arbitrary dynamic expressions, or create a parallel field specialization
  framework.
- Existing early class registration executes inside `create_class`,
  **before** decorators run or the class is bound in module globals.
  Publishing annotation-only cells there would capture stale pre-decorator
  type tags. The safer source-proven design is to **defer** these cells
  until the existing indexed module globals actually bind the exact same
  owner, then let the existing module-end scan publish them **once**, after
  decoration. No cell rearming, weak-reference refresh, mutable republish,
  or additional watcher state is permitted. Retain immutable annotation
  helper identity, current packed function identity and registered code
  provenance, exact final owner/type version, unchanged hooks, absent
  competing descriptors, and valid split-dictionary storage.
- Genuine unchanged-production integration invokes real transformed
  `Profile -> Verify -> Apply` subprocesses, all of which execute
  successfully before the intended final assertion. Profile records exact
  `DecoratedRecord.record_payload` index **0**, `DecoratedRecord.count`
  index **1**, plus `PlainRecord` and `PseudoRecord` owner type keys.
  Existing lexical/non-self controls and descriptor/default/ClassVar/slots/
  frozen/hooks/spoiled-helper/lazy-annotation/finalizer/dictionary/MRO/
  class/subclass semantic controls all pass. The sole regression is that
  all eight expected annotation-derived positives—`read_plain`,
  `read_record_payload`, `count`, `Consumer.consume`, nested access,
  `read_pseudo`, `write_plain`, and `write_record_payload`—have actual
  Verify **`indexed_hit == 0`**. Result: **1 failed in 2.53 seconds**.
  The one-time **21.47-second** debug-extension rebuild follows the
  previous release-to-debug/source-restore transition and is workflow
  overhead only, not candidate performance.
- A second independent genuine unchanged-production structured optimizer
  regression lowers actual decorated, plain, slotted, unreferenced, and
  **10-field** classes, supplies real counter evidence, and invokes full
  production `plan_and_emit_module_v3_from_raw_evidence`. Existing
  `read_legacy` specialization still passes. Its sole intended failure at
  `pipeline_v3.rs:3558` is **"the real class annotation thunk must publish
  Record.payload's owner cell"**; focused result is **1 failed / 212
  filtered**. Both integration and optimizer REDs precede any two-file
  behavior change; implementation begins only after observing both.
- The exact full-production structured optimizer regression now turns
  **RED-to-GREEN: 1 passed / 1 focused test**. The two approved production
  files append deterministic literal annotation-thunk anchors capped at
  **eight**, preserve every preexisting dense cell index, and select actual
  `Record` / `Plain` hot load/store plans with original source and concrete
  index. Existing `read_legacy` specialization remains valid; cold,
  slotted, and unreferenced classes are excluded.
- The implemented publisher validates immutable annotation-helper
  identity, its current packed function ID and code provenance, plus the
  exact registered numeric indexed-globals slot and final class owner with
  correct owned-reference release. It **defers before decoration / module
  binding** and publishes **once** during existing final registration;
  no cell rearming, republishing, or weak-reference refresh is added.
  Independent source review reports no issue. The original frozen actual
  transformed integration now turns **RED-to-GREEN on its first
  implementation run: 1 passed in 2.33 seconds**, after the unchanged-
  production **1 failed in 2.53 seconds**. Real `Profile -> Verify ->
  Apply` now records original-source indexed hot loads/stores and nested
  access for plain, decorated, and `default_factory` annotation owners.
  Frozen/slots/default/ClassVar/InitVar/KW_ONLY/helper-code/replaced-owner /
  hooks/descriptors/MRO/promoted-dictionary/subclass/finalizer/lazy-
  annotation controls all pass. Existing non-self compatibility then fails
  solely at its obsolete generated-dataclass negative: `read_generated`
  records **32 Verify indexed hits** where the old fixture expected
  zero/generic behavior. The reviewer narrowly migrates **only**
  `read_generated: 32` into positive expectations while preserving all
  Profile evidence and ambiguous/cold/unanchored/slot/inherited controls.
  The new annotation integration and intentionally migrated existing
  non-self fixture now jointly pass **2 / 2 in 4.34 seconds**, retaining
  all positive and negative controls. Complete optimizer library tests
  pass **213 / 213**, and complete JIT library tests pass **568 / 568**.
  Broader transformed coverage passes **49 / 49 in 19.07 seconds**.
  `just fmt-rust-check soac_opt soac_jit` and
  `cargo check -p soac_jit --tests` pass. The frozen production change is
  limited to the two approved files; benchmarks and the full gate remain
  pending.
- Release debug-single fixed-eight comparison **101131** against the
  actually retained guarded-runtime smoke **090221** passes **8 / 8**
  with zero errors. Every function/body byte and machine block is exactly
  unchanged for all **seven non-comprehensions workloads**, and compiled
  coverage remains **2,866 typed blocks / 204 functions**. Exactly six
  intended `Widget` consumers grow: generated expression **+420 bytes**,
  `_is_big_spinny` **+524**, dict comprehension 12 **+612**, dict
  comprehension 17 **+1,128**, list comprehension 23 **+2,132**, and
  `make_some_widgets` list comprehension 29 **+636**. The dominant
  unrelated `_dp_listcomp_20` `dict.get` body remains byte-for-byte
  identical. Comprehensions generated code increases
  **274,348 -> 279,800 bytes (+5,452 / +1.987%)** and
  **18,153 -> 18,513 machine blocks**; aggregate smoke code increases
  **2,253,100 -> 2,258,552 bytes (+0.242%)** and
  **148,734 -> 149,094 blocks**. Cold single-iteration timings are not
  throughput evidence.
- Normally sampled fixed-eight comparison **101328** against actual
  retained guarded-runtime comparison **090414** reports stock geometry
  **0.5777893609272814x**, below prior **0.5883463026285985x**, and
  official previous-SOAC geometry **0.976412288391244x**, a regression.
  Independent robust fixed-eight geometry is **0.980147x raw / 0.983264x
  stock-adjusted**. Targeted comprehensions remains negative but
  statistically inconclusive: **0.968605x [0.944921, 1.043362]**, or
  **0.958675x paired [0.936464, 1.036833]**. Chaos drops to
  **0.934016x [0.920450, 0.978905] / 0.913857x paired** despite every
  affected control function/body being byte-identical, demonstrating
  substantial environment drift rather than a proven candidate code path;
  richards is **0.961006x raw / 0.973683x paired**, consistent with
  neutral/drift. Every function across seven controls remains unchanged;
  comprehensions alone adds **5,452 bytes for each of 10 measured
  workers**. Normal aggregate native code increases
  **23,293,040 -> 23,347,560 bytes (+0.234%)**, and machine blocks grow
  **1,533,550 -> 1,537,150 (+3,600)**; error count is zero. A targeted
  repeated comparison is now complete and decisively negative; matched
  causal profiling remains pending.
- Targeted three-round comparison **101713** against retained comparison
  **090720** contains **60 candidate / 60 baseline samples**. The actual
  intended comprehensions target significantly **regresses** to
  **0.955866x [0.935330, 0.977960]**, a **4.413%** throughput loss;
  paired-stock geometry is also strictly below neutral at
  **0.957629x [0.935177, 0.982552]**. Raw rounds are
  **0.981255x / 0.999933x / 0.920205x**, and robust median latency grows
  **52.4194 -> 54.8397 us**. Candidate outliers reach **501.48 us** versus
  baseline maximum **61.89 us**, with **8 candidate samples above 160 us**;
  although these outliers severely distort official mean geometry
  approximately **0.88117x**, the robust median itself still establishes a
  significant genuine slowdown. Matched stock drift is only **0.99816x**.
- Targeted chaos is **0.972772x / 0.994187x paired**, deltablue
  **0.98930x / 1.00270x paired**, and richards
  **1.00003x / 0.99494x paired**, consistent with neutral controls.
  Four-workload robust geometry is **0.979348x raw / 0.987207x paired**.
  All **90 control worker PIDs** remain function/body byte-identical;
  each of **30 comprehensions workers** adds **5,452 bytes / 360 blocks**,
  increasing targeted aggregate native bytes
  **55,058,040 -> 55,221,600 (+0.297%)**, with zero worker errors or
  deoptimizations. The specialization is structurally correct but
  reproducibly slower; the strategy is **REJECTED**. No full gate or
  retained PERF_LOG entry is warranted.
- Matched zero-loss comprehensions profiles use **50,000 loops / 199 Hz**,
  with **618 -> 662 raw samples**. Generic `PyObject_GetAttr` inclusive
  ancestry decreases **8.4148% -> 6.7972% (-1.6176 percentage points)**,
  and `PyObject_GenericGetAttr` **6.4730% -> 4.5315%**. Exact intended
  `Widget` attribute ancestry decreases for `_is_big_spinny`
  **1.1333% -> approximately 0.755%** and list comprehension 23
  **2.1027% -> 1.5105%**, confirming correct selection and real generic
  lookup elimination.
- However, generated `_is_big_spinny` self work increases **0% ->
  1.3604%**, list comprehension 23 self **0.8085% -> 1.0573%**, and TLS
  ancestry **0.9714% -> 2.2657%**; reference-count and deallocation work
  also rise. Garbage collection changes **14.398% -> 15.407%**, providing
  an additional confound. All inclusive/self ancestries require their stated
  scopes and overlap; do **not** sum them into a purported causal total.
  Attached replay worsens **57.6886 -> 60.8879 us (0.94748x)** but is
  diagnostic only. The acceptance evidence remains the matched
  **0.955866x raw / 0.957629x paired** target with both confidence
  intervals strictly below one, plus **5,452 extra bytes per worker**.
  Correctly selected guards cost more than the generic attribute operations
  they replace.
- Preserve existing exact weak owner/version, concrete class binding, live
  split-key/index, descriptor/hook precedence, and generic fallback.
  Explicitly reject or preserve `ClassVar`, `InitVar`, `KW_ONLY`, slots,
  frozen dataclasses, properties, custom attribute hooks, class/MRO
  mutations, helper function/code mutation, subclasses, deleted/promoted
  dictionaries, finalizers, and user-visible annotation side effects.
- Add no new public API, runtime helper, process-global mutable state,
  profile format, public typed plan, or broad mutable-class guard. Existing
  scalar/self/inherited/non-self specializations retain their precedence
  and compatibility behavior.
- Required regressions are a genuine unchanged-production transformed
  `Profile -> Verify -> Apply` annotation-owner RED, a production-path
  structured catalog/plan RED, deferred once-only post-decoration
  publication timing controls,
  migration of the existing now-obsolete negative expectation, and a broad
  dataclass/class mutation semantic matrix. The new host-parse-clean
  transformed fixture is frozen and its first unchanged-production run is
  a genuine **1 failed / 2.53-second** RED. Independent production-path
  structured optimizer coverage was likewise genuinely RED, **1 failed /
  212 filtered**, and now turns GREEN **1 / 1** after the bounded two-file
  implementation; existing legacy specialization is preserved. The frozen
  transformed integration independently turns **GREEN 1 / 2.33 seconds**
  on its first implementation run. The obsolete existing non-self
  `read_generated` negative genuinely fails with **32 indexed hits** and
  is intentionally migrated without weakening other controls; the combined
  transformed fixtures pass **2 / 2 in 4.34 seconds**. Full optimizer and
  JIT libraries pass **213 / 213** and **568 / 568**, respectively; broad
  transformed coverage passes **49 / 49 in 19.07 seconds**, and
  package-scoped formatting plus JIT test-target checks pass. Benchmarking
  and the full correctness gate remain **pending**.

## Benchmark protocol and coverage

- Fixed normal selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`, compared
  against the same vendored stock CPython and independently profiled
  guarded-runtime production code. Use a same-selector repeated targeted
  comparison before accepting small dataclass-field changes.
- Baseline artifacts: guarded-runtime fixed-eight comparison **090414**,
  targeted three-round comparison **090720**, and the current
  **618-sample zero-loss** comprehensions profile. `main` is a docs-only
  descendant of the benchmarked production revision, so these artifacts
  are valid code baselines; do not pretend a separate production change.
- Independently distinguish actual `Widget` field loads from unrelated
  `dict.get`, inspect publication type versions after decorators, and
  compare exact per-function generated bytes/blocks. Require retained JIT
  and transformed-module coverage plus preserved source/synthetic factory,
  cell, watcher, StopIteration, owner/scalar, and generator guardrails.
- Candidate release debug-single smoke passes **8 / 8**; seven workloads
  retain identical code and exactly six intended comprehensions consumers
  grow. The normal fixed-eight result regresses, and the decisive targeted
  repeat shows statistically significant **4.413%** comprehensions
  slowdown despite neutral controls; native growth is confirmed. Matched
  causal profiling confirms correct generic lookup elimination outweighed
  by guarded generated work; the candidate is rejected. A full gate is not
  run for a rejected production change.

## Measurements

| Metric | Integrated guarded-runtime production baseline | Candidate | Change |
| --- | --- | --- | --- |
| Normal fixed-eight paired stock / SOAC geometry | 0.5883463026285985x | 0.5777893609272814x | regression; full-suite stock 1.10x goal unmet |
| Normal fixed-eight official / robust / paired previous-SOAC geometry | retained comparison 090414 | 0.976412288391244x / 0.980147x / 0.983264x | all below one; unaffected controls show environment drift |
| Normal target comprehensions raw / stock-adjusted geometry | retained comparison 090414 | 0.968605x / 0.958675x | raw CI [0.944921, 1.043362]; paired CI [0.936464, 1.036833]; inconclusive |
| Targeted three-round comprehensions raw / stock-adjusted geometry | comparison 090720; median 52.4194 us | 0.955866x / 0.957629x; median 54.8397 us | raw CI [0.935330, 0.977960]; paired CI [0.935177, 0.982552]; significant 4.413% slowdown |
| Targeted four-workload robust / stock-adjusted geometry | retained comparison 090720 | 0.979348x / 0.987207x | official approximately 0.88117x distorted by candidate outliers |
| Targeted fixed-four paired stock / SOAC geometry | 0.4290269750586277x | pending | subset only; not full-suite acceptance |
| Previous-SOAC robust / stock-adjusted improvement | docs-only `puozyplw/4046c825`; production `zwkrytkq/443b2e42` | pending | same production code |
| Optimized typed-IR blocks / functions | 2,866 / 204 | 2,866 / 204 | unchanged |
| Apply-mode native code bytes / machine blocks | 23,293,040 / 1,533,550 | 23,347,560 / 1,537,150 | +0.234% bytes / +3,600 blocks; seven controls identical |
| Targeted three-round aggregate native code bytes | 55,058,040 | 55,221,600 | +0.297%; all 90 controls unchanged; 30 comprehensions workers +5,452 bytes each |
| Release debug-single fixed-eight native bytes / blocks | 2,253,100 / 148,734 | 2,258,552 / 149,094 | +5,452 bytes / +0.242%; seven controls unchanged; zero errors |
| Release debug-single comprehensions native bytes / blocks | 274,348 / 18,153 | 279,800 / 18,513 | +1.987%; exactly six Widget consumers; unrelated dict.get unchanged |
| Current comprehensions zero-loss raw samples | 618 | 662 | matched 50,000 loops / 199 Hz; both zero-loss |
| Matched generic PyObject_GetAttr / PyObject_GenericGetAttr ancestry | 8.4148% / 6.4730% | 6.7972% / 4.5315% | correct generic lookup elimination; scopes overlap |
| Matched guarded generated self / TLS ancestry | big_spinny 0%; listcomp23 0.8085%; TLS 0.9714% | 1.3604% / 1.0573% / 2.2657% | replacement guard/code overhead rises; do not sum overlapping shares |
| Matched diagnostic profiling replay | 57.6886 us | 60.8879 us | 0.94748x; diagnostic only; GC 14.398% -> 15.407% confound |
| Total generic attributes / unrelated `_dp_listcomp_20` dict.get | approximately 8.577% / 2.9–3.1% | pending | dict.get is not a Widget field |
| Genuine Widget dataclass-field opportunity | approximately 2–3 gross percentage points | pending | not a promised throughput gain |
| Prior single-owner field guard growth | approximately 748 bytes / 67 machine blocks | pending | guard/code overhead may erase small gains |
| Existing compiler-generated annotation thunk | `Widget.__annotate_func__` function 1:9; six static tuple pairs | pending | literal metadata only; do not execute annotations |
| Genuine transformed integration regression | unchanged-production integration 1 failed in 2.53 s; all eight Verify indexed hits zero | 1 passed in 2.33 s | genuine RED-to-GREEN; real Profile/Verify/Apply plain/decorated/default_factory loads/stores/nested access and full mutation semantics |
| Genuine production-path structured optimizer regression | 1 failed / 212 filtered; real annotation thunk missing Record.payload owner cell | 1 passed / 1 focused | genuine RED-to-GREEN; capped deterministic Record/Plain source/index anchors; legacy/cold/slot/unreferenced controls pass |
| Existing generated-dataclass non-self expectation | prior read_generated expected zero/generic hits | actual 32 Verify indexed hits; only that access migrated to positives | genuine old-oracle failure; Profile / ambiguous / cold / unanchored / slot / inherited controls preserved |
| Combined new annotation / migrated existing non-self transformed fixtures | new fixture RED; existing stale negative fails at 32 hits | 2 / 2 passed in 4.34 s | GREEN; both complete transformed semantic families preserved |
| Complete optimizer / JIT Rust libraries | integrated baseline 212 optimizer / 568 JIT tests | 213 / 213 and 568 / 568 passed | GREEN |
| Broad transformed-runtime compatibility | integrated guarded-runtime baseline | 49 / 49 passed in 19.07 s | GREEN; annotation, non-self, decorator, mutation, and prior semantic families |
| Scoped two-package formatting / JIT test-target check | integrated guarded-runtime baseline | both passed | GREEN |
| Full `just test-all` correctness gate | integrated baseline previously passed | not run | candidate rejected; experimental production is not retained |

## Attempt history

### Attempt 1: identify literal annotation-owner anchors

- Change: inspect the current true comprehensions profile and existing
  compiler-generated `Widget.__annotate_func__` metadata before approving
  production edits. Restrict possible implementation to the existing
  optimizer catalog and JIT class publication.
- Measurements and coverage: **618 zero-loss samples**; total generic
  attributes approximately **8.577%**, but **2.9–3.1%** is unrelated
  `_dp_listcomp_20` `dict.get`. Actual annotation-owner ceiling is only
  approximately **2–3 gross percentage points**, and an existing
  single-field guard added approximately **748 bytes / 67 blocks**.
- Compatibility and tests: static tuple-pair names can survive interpreted
  annotation thunks, but early class registration inside `create_class`
  precedes decorators and module binding. Defer annotation-only cells until
  actual same-owner indexed-global binding, then publish once through the
  existing module-end scan; never rearm/refresh/republish. Preserve
  descriptor/type version, hooks, frozen / slots/ClassVar/InitVar/KW_ONLY,
  subclass, finalizer, helper mutation, and annotation side effects. The
  approximately 600-line transformed/adversarial fixture is host-AST clean
  and its first unchanged-production run genuinely fails **1 / 2.53
  seconds** only because all eight annotation-derived access targets have
  zero Verify indexed hits. Real Profile/Verify/Apply, exact owner type
  keys, and the complete compatibility matrix already pass. The one-time
  **21.47-second** debug rebuild is workflow-only. Independent unchanged-
  production structured optimizer coverage then fails **1 / 212 filtered**
  exactly because the real annotation thunk cannot publish `Record.payload`,
  while real decorated/plain/slotted/unreferenced/10-field classes and the
  existing `read_legacy` specialization use full production planning.
  Two-file implementation begins only after both genuine REDs, appends at
  most eight deterministic literal anchors without shifting existing dense
  indices, and turns the full-production structured optimizer test GREEN
  **1 / 1**, retaining real `Record` / `Plain` source/index evidence,
  existing legacy specialization, and negative controls. Final publisher
  validates immutable helper identity/current ID/code and exact numeric
  module-global owner, defers pre-decoration, and publishes once with
  owned-reference release and no rearm. The unchanged frozen actual
  transformed integration then turns **GREEN 1 / 2.33 seconds on its first
  implementation run**, proving original-source annotation-owner indexed
  loads/stores/nested access and full frozen/slots/default/ClassVar/InitVar/
  KW_ONLY/helper mutation/class hooks/descriptors/MRO/promoted dictionary/
  subclass/finalizer/lazy controls. Existing non-self compatibility then
  genuinely fails solely because `read_generated` has **32 indexed hits**
  instead of its old generic-zero expectation. The reviewer migrates only
  that access to positive expectations while preserving all Profile,
  ambiguity, cold, unanchored, slot, and inherited controls. The saved file
  is frozen and host-AST clean. The combined new annotation and migrated
  existing non-self transformed fixtures then pass **2 / 2 in 4.34
  seconds**. Full optimizer and JIT libraries pass **213 / 213** and
  **568 / 568**, respectively. Broader transformed compatibility passes
  **49 / 49 in 19.07 seconds**; two-package scoped formatting and the JIT
  test-target check pass. Release fixed-eight debug-single smoke passes
  **8 / 8**, with seven unaffected workloads byte-identical and exactly six
  intended Widget consumers adding **5,452 bytes / 360 blocks**; cold
  timings are invalid. Normal fixed-eight official previous-SOAC geometry
  regresses to **0.976412288391244x**; target comprehensions is
  **0.968605x**, statistically inconclusive, while unaffected controls
  show environment drift. Normal native bytes increase **0.234%**.
  Targeted repeated throughput decisively confirms comprehensions
  **0.955866x [0.935330, 0.977960] / 0.957629x paired**, despite neutral
  controls; aggregate targeted native bytes grow **0.297%**. The official
  mean is additionally distorted by severe candidate outliers, but the
  robust median independently regresses. Matched zero-loss profiles
  confirm `PyObject_GetAttr` **8.4148% -> 6.7972%** and exact Widget
  generic lookup reductions, but guarded generated self/TLS/refcount work
  rises, GC differs, and diagnostic replay slows. The structurally correct
  specialization is rejected because guard cost exceeds the cheap generic
  attribute work while native code grows. No full gate is run.
- Result: **REJECTED; CORRECT ANNOTATION SPECIALIZATION SIGNIFICANTLY
  SLOWS ITS TARGET BY 4.413% AND INCREASES NATIVE CODE; ONLY NEGATIVE
  STRATEGY HISTORY IS RETAINED**.
- Reason: interpreting aggregate generic attribute ancestry as dataclass
  work would overstate the opportunity; capturing pre-decorator class
  versions would silently invalidate every intended fast path.

## Verdict and next action

- Verdict: **REJECTED; TARGET COMPREHENSIONS SIGNIFICANTLY SLOWER DESPITE
  CONFIRMED GENERIC ATTRIBUTE ELIMINATION**. The bounded two-
  file deferred-once publisher passes source review, structured planning,
  and actual transformed semantics. The existing generated-dataclass
  negative is intentionally migrated after its real **32-hit** failure,
  and combined new/existing transformed fixtures pass **2 / 2 in 4.34
  seconds**. Full optimizer/JIT libraries pass **213 / 213** and
  **568 / 568**; broader transformed coverage passes **49 / 49**, and
  scoped formatting/test-target checks pass. Release smoke confirms all
  seven unaffected workloads remain byte-identical, exactly six Widget
  consumers grow by **5,452 total bytes**, and the unrelated dict.get body
  is unchanged; cold smoke timings do not establish throughput. Normal
  fixed-eight previous-SOAC geometry **0.976412288391244x** regresses,
  normal native code grows **0.234%**, and seven control workloads remain
  byte-identical. The decisive repeated target is **0.955866x raw
  [0.935330, 0.977960] / 0.957629x paired [0.935177, 0.982552]**,
  confirming a significant **4.413%** throughput loss while targeted native
  bytes grow **0.297%**. Mean outliers worsen the arithmetic result but are
  not needed to establish the robust regression. Matched zero-loss profiles
  confirm generic lookup drops **8.4148% -> 6.7972%**, while guarded
  generated/TLS/refcount work rises; attached replay worsens but is
  diagnostic only. Root has verified restoration of both production files,
  the specialization documentation, and the existing fixture migration,
  and removed the new integration. **No full gate is run and
  no retained `doc/PERF_LOG.md` entry is created**; only this negative
  strategy record remains. Full-suite **1.10x stock** remains unmet.
- Transferable lesson: use immutable literal annotation evidence without
  executing annotations, and anchor mutable classes only after their
  decorator-final owner/version is established.
- Next action: retain only this negative strategy history; experimental
  production, specialization, and test changes are already restored.
  Seek cheaper owner-field guard shapes before revisiting annotation-
  derived anchors.
