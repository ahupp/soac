---
title: "Immutable Singleton Truthiness"
---

# Immutable singleton truthiness

- Status: **LANDED CANDIDATE / RETAIN; current retained lossless hotspot, pinned CPython
  identity semantics, and genuine unchanged-production transformed
  compatibility GREEN 1 / 2.12 seconds verified; genuine pinned-runtime
  production-path structured optimization-decision RED-to-GREEN
  1 passed / 572 filtered; one-file implementation saved / formatted /
  independently source-reviewed; final transformed singleton integration,
  JIT 573 / optimizer 213 / typed 54 / broad transformed 16 / scoped
  checks AND release smoke 8 / 8 GREEN with exact native invariance;
  clean repeated richards 1.055991x raw / 1.041139x adjusted, exact native
  invariance and matched lossless profile confirmed; full authoritative
  gate GREEN 1,233 Python nodeids / 96 passing batches / zero failures**.
- Pacific date: **2026-08-19 PDT**.
- Integrated baseline: retained `main` change **`zkwnlurq`**, commit
  **`048bf8b8`**.
- Candidate change: **`vosvuxuw`**, initially observed at mutable working
  commit **`0093ff25`**; future snapshots can change that commit ID.
- Outcome: determine whether the existing truthiness hook can directly
  classify immutable `True`, `False`, and `None` identities while preserving
  the exact existing generic CPython path for every other object. There
  is **no claimed existing user-visible CPython behavior mismatch**.

## Hypothesis and evidence

- General-purpose opportunity: Python programs repeatedly branch on stored
  booleans and `None`. The existing exported `dp_jit_is_true` delegates
  through private `is_true_hook` to `PyObject_IsTrue` even when the value is
  one of CPython's three immutable singleton identities. A production-used
  local identity classifier could avoid the existing C-call / PLT boundary
  without weakening arbitrary-object truthiness.
- Current retained richards profile is **lossless, 280 samples**. Its
  disjoint truthiness leaves are `PyObject_IsTrue` **4.644%** plus PLT
  **1.428%**, a measured **6.072-percentage-point gross leaf union**;
  existing `dp_jit_is_true` also contributes **0.714%**. The hook remains
  necessary, and inclusive/self attribution must not be added or treated
  as a promised end-to-end speedup.
- Source-attributed owners include **`Richards.run` approximately 2.5
  percentage points**,
  **`TaskState.isTaskHoldingOrWaiting` approximately 1.429 points**, and
  **`TaskState.isWaitingWithPacket` approximately 1.071 points**. Those
  code paths initialize and mutate actual boolean attributes with `True`
  and `False`; they are not proof that every profiled truthiness value is
  a singleton, and dynamic proportions must be verified independently.
- Pinned `vendor/cpython/Objects/object.c:2052` already checks
  `v == Py_True`, `v == Py_False`, and `v == Py_None` in that order before
  inspecting number/mapping/sequence truthiness slots. The proposed
  optimization mirrors only these existing immutable identity facts;
  it does not infer truth from object type, attribute shape, cached
  mutable metadata, or profile frequency.
- A superficially larger guarded-iterator hotspot was **rejected before
  implementation**: apparent `GetIter` ancestry of **10.96%** contains
  approximately **9.59 percentage points of GC / pending work**, and a
  broad iterator shortcut would have unsafe visible protocol semantics.
  Inclusive ancestry is not removable productive work.
- Immediate retained artifacts are release smoke
  **`comparison-20260819-165908-yAxVUq`**, fixed-eight normal
  **`comparison-20260819-170030-xMdA75`**, and clean repeated four-workload
  **`comparison-20260819-170351-FabHzT`**. The normal official stock score
  **0.6273571181431998x** and previous score **0.9683515036210124x** are
  explicitly **host-contaminated / outlier-sensitive**, not reliable
  evidence of a prior aggregate regression. The clean repeated stock and
  previous scores are **0.49747399350945193x** and
  **1.0194276621869476x**, respectively.
- Full-suite stock **1.10x** remains unmet, and the complete pyperformance
  suite has not been measured. The fixed subset has been independently
  measured; do not equate its result or the gross profile ceiling with
  full-suite acceptance.

## Implementation and compatibility

- Proposed production scope is **exactly one existing file**:
  **`crates/soac_jit/src/jit/specialized_helpers.rs`**. Modify only the
  existing production-used private `is_true_hook` path, optionally with a
  tiny private classifier used directly by that production path.
- After preserving the existing null check and exact
  `RuntimeError("invalid null value for dp_jit_is_true")`, return **1**
  for pointer-identical `True`, **0** for pointer-identical `False`, and
  **0** for pointer-identical `None`. These immortal singleton identities
  are immutable within the pinned interpreter and require no mutable
  type guard, cache, global state, allocation, or ownership change.
- Every non-singleton object must take the **unchanged original**
  `PyObject_IsTrue` fallback. Preserve number-slot `__bool__` precedence,
  mapping/sequence `__len__`, side effects, class mutation, subclass
  behavior, raised errors, invalid truth-result errors, reentrancy,
  finalization, and `-1` error propagation. Do not treat integer `0` / `1`
  or merely bool-like custom objects as singleton identities.
- Keep the existing exported `dp_jit_is_true` symbol, signature, caller,
  instrumentation, generated native bodies, and interpreter-visible
  behavior unchanged. Add **no new exported runtime helper, public API,
  mutable global, public IR operation, or second production file**.
- New real transformed integration
  **`tests/test_immutable_singleton_truthiness.py`** is genuinely
  **GREEN on unchanged production: 1 passed in 2.12 seconds**. Actual
  same-process stock and transformed **Profile → Verify → Apply** cover
  singleton identities, dynamically mutated boolean attributes, integer /
  list subclasses, rejected bool subclassing, custom `__bool__` /
  `__len__`, raised and invalid returns, descriptors, class / callback
  mutation, reference counts, weakrefs, finalizers, actual branch-profile
  outcomes **0 / 1**, and emitted source native bodies. There is **no
  existing CPython correctness mismatch**, and this test was never a
  production behavior RED.
- The first disposable fixture-only run mismatched the stock module
  `__name__`, which appears in pinned CPython 3.15's `InvalidBool`
  `TypeError`; aligning the stock namespace fixed the test. This was a
  **test-fixture error**, not a production semantic failure.
- A genuine actual pinned-runtime **production-path structured
  optimization-decision RED** is now established by
  **`immutable_singleton_truthiness_preserves_the_exported_python_protocol`**.
  A conservative classifier is wired directly through the existing
  production/exported **`dp_jit_is_true`** path; all existing singleton
  outputs and refcounts, custom `__bool__` called exactly once,
  `__len__`, raised `ValueError`, and malformed-null `RuntimeError` first
  **PASS**. Its sole final optimization-decision assertion finds actual
  **`None`** instead of required **`Some(1)`**, yielding genuine
  **0 passed / 1 failed / 572 filtered**. This is an optimization-path
  decision RED, **not** a CPython behavior failure.
- The architect's exact one-file implementation in existing
  **`specialized_helpers.rs`** is **SAVED**, package-formatted, and
  independently host-source-reviewed **GREEN**. Its production-used pure
  private classifier checks only exact singleton pointers: **`True → 1`**,
  **`False → 0`**, and **`None → 0`**. The existing malformed-null error
  check runs first, every non-singleton still uses the untouched
  `PyObject_IsTrue`, and ownership / exported API / helper inventory /
  global state / typed IR remain unchanged.
- The same actual pinned-runtime production-path decision regression
  genuinely turns **RED-to-GREEN: 1 passed / 572 filtered**, after the
  previous **`None` versus `Some(1)`** decision failure; all existing
  singleton, callback, refcount, and error controls continue to pass.
- The frozen stock/transformed **Profile → Verify → Apply** singleton
  integration is genuinely **GREEN on the final candidate**, preserving
  its baseline-green custom truthiness, mutations, errors, finalizers,
  branch outcomes, and source-native controls. Complete final Rust suites
  pass JIT **573 / 573**, optimizer **213 / 213**, and typed IR
  **54 / 54**; broad transformed compatibility passes **16 / 16**.
  Package-scoped formatting / format check and `cargo check -p soac_jit
  --tests` also pass. Final release smoke passes **8 / 8** with exact
  measured native coverage; normally sampled fixed-eight comparison is
  complete but its retained comparator is noisy. Clean repeated target
  performance, matched lossless profile, and the authoritative full
  `just test-all` gate are all complete and **GREEN**.
- The full authoritative
  **`work/logs/immutable-singleton-truthiness-test-all.log`** exits zero:
  **1,233 transformed Python nodeids / 96 isolated batches / 8 workers /
  96 passed / 0 failed**. Rust JIT passes **573**, optimizer **213**,
  typed IR **54**, lowering **371**, and PyO3 **8**. Runtime test build
  takes **1.679 seconds**, Cargo **64.701 seconds**, pytest **80.356
  seconds inner / 80.374 seconds outer**, and total test phase **145.087
  seconds**. The new real transformed truthiness integration passes in
  **2.85 seconds**; the known **28-node** counter-dump batch takes
  **79.97 seconds** and remains the critical path.

## Benchmark protocol and coverage

- Fixed benchmark selection: existing eight workloads **chaos,
  comprehensions, deltablue, fannkuch, float, nbody, richards, and
  spectral norm**; repeated four-workload subset **chaos,
  comprehensions, deltablue, and richards**, with richards the primary
  source-backed target and the others guardrails.
- Use fresh independent profile evidence, mode-matched release smoke,
  normally sampled fixed-eight comparison, and at least **three**
  independently started targeted comparison rounds. Report stock scores,
  previous SOAC ratios, worker-clustered confidence intervals, paired-stock
  drift, severe host contention, and any actual source-function coverage
  changes. Cold debug-single timings and attached-profiler replay are not
  throughput evidence.
- Baseline artifacts: retained smoke **165908-yAxVUq**, noisy normal
  **170030-xMdA75**, and clean repeated target **170351-FabHzT**.
  Candidate release smoke is
  **`comparison-20260819-174435-fIWJY1`**, and completed candidate normal
  is **`comparison-20260819-174639-jdIc19`**. Clean final three-round
  candidate comparison against retained target **170351** is
  **`comparison-20260819-175000-LYnAbi`**; matched source/worker causal
  profiles have zero lost samples.
- Keep the existing benchmark module-selection policy and explicitly
  inspect measured Apply PID source-function rows, hot richards bodies,
  stock controls, errors, ordinary native sizes, and hidden exact
  trampolines. Benchmark completion alone does not prove singleton
  coverage.
- Baseline fixed-eight coverage is **2,866 optimized typed blocks / 204
  functions / 23,163,480 ordinary native bytes / 1,524,480 machine
  blocks**, with **365,000 hidden trampoline bytes** across measured
  workers. Baseline repeated four-workload coverage is **2,265 typed
  blocks / 183 functions** per round and **54,697,320 native bytes /
  3,594,960 machine blocks** across all three rounds, with **746,520
  hidden trampoline bytes**. Release-smoke pre-optimization BlockPy is
  **7,199,376 bytes**; ordinary smoke native is **2,238,468 bytes /
  147,712 machine blocks**, and hidden smoke trampolines are **36,500
  bytes**.
- New production path should not alter generated source-function bodies,
  typed coverage, or hidden trampoline size. Candidate smoke directly
  verifies all **8 measured Apply PIDs / 397 source rows / 204 direct
  bodies**, exact source IDs and module coverage, every body size / block
  count, hidden trampoline names / bytes, and **zero ERROR** against
  retained smoke **165908**. Native totals remain exactly **2,238,468
  bytes / 147,712 blocks**, with **36,500 hidden trampoline bytes**.
  Per-workload rows / direct bodies / native bytes / blocks / hidden bytes:

  | Workload | Rows | Direct bodies | Native bytes | Blocks | Hidden bytes |
  | --- | --- | --- | --- | --- | --- |
  | chaos | 64 | 32 | 673,548 | 44,571 | 6,692 |
  | comprehensions | 38 | 24 | 267,464 | 17,764 | 4,088 |
  | deltablue | 152 | 76 | 456,944 | 29,570 | 6,692 |
  | fannkuch | 2 | 1 | 15,856 | 956 | 952 |
  | float | 14 | 7 | 59,916 | 3,850 | 2,076 |
  | nbody | 12 | 6 | 248,284 | 16,505 | 6,512 |
  | richards | 101 | 51 | 347,408 | 23,191 | 7,412 |
  | spectral norm | 14 | 7 | 169,048 | 11,305 | 2,076 |

- Debug-single smoke timings are **INVALID** throughput evidence and do
  not support any speedup claim. Recurring full-gate → venv redundant
  package installation and a **21.71-second** release rebuild are
  **workflow/setup costs only**, not measured candidate performance.
- Completed candidate normal **174639** versus noisy retained **170030**
  shows provisional target richards **27.683731 → 25.455922 ms**,
  **1.08752x** raw / **1.05370x** stock-adjusted, with zero candidate
  richards outliers. The official richards previous ratio **1.1164x** is
  inflated by a retained outlier and is not a reliable headline. Final
  worker-clustered normal richards is **1.087516x [1.052810, 1.110215]**
  raw / **1.053701x [1.011770, 1.089200]** stock-adjusted; its comparator
  remains noisy, so the clean independent three-round comparison is the
  authoritative headline. All **80** measured normal Apply workers keep
  exactly **23,163,480 native bytes / 1,524,480 blocks / 365,000 hidden
  trampoline bytes**, identical source rows, and zero errors.
- Provisional controls, raw / stock-adjusted, are chaos
  **1.0336x / 1.0060x**, comprehensions **1.0437x / 1.0046x**, deltablue
  **1.0587x / 1.0105x**, float **1.0072x / 1.0005x**, nbody
  **1.0278x / 1.0205x**, fannkuch **1.0475x / 0.9670x**, and spectral
  norm **0.9934x / 0.9585x**. Fannkuch and spectral adjusted ratios are
  affected by stock drift; avoid unsupported causal control claims.
  Official fixed-eight stock is exactly **0.6345791409139968x**, with
  previous **1.0532525776372081x**; the retained comparator remains
  host-contaminated.
- Definitive clean three-round comparison **175000** versus retained
  **170351** improves richards **27.146548 → 25.707173 ms**,
  **1.055991x [1.026761, 1.076974]**, stock-adjusted
  **1.041139x [1.004715, 1.062791]**. Raw per-round ratios are
  **1.05297 / 1.06793 / 1.05861**, with all paired rounds also above
  neutral. Five candidate richards worker outliers depress its official
  mean-based previous ratio to **1.0244962581850656x**, versus robust
  **1.055991x**; clustered intervals and independent round consistency
  still support the target improvement.
- Repeated control raw / stock-adjusted results are chaos
  **1.032905x [1.024492, 1.056316] /
  0.974171x [0.958818, 1.002254]**, comprehensions
  **1.020752x [1.005712, 1.033404] /
  0.954158x [0.935538, 0.974371]**, and deltablue
  **1.052594x [1.038294, 1.076919] /
  1.015103x [0.983506, 1.050410]**. Comprehensions improves in raw time
  **45.076 → 44.160 µs**, but its paired ratio is significantly lower
  because candidate stock is approximately **6.5% faster**; report this
  candidly and claim **no comprehensions improvement**. Chaos / deltablue
  stock-adjusted intervals cross neutral.
- Exact final targeted official scores are stock
  **0.4865323207896451x** / previous **1.023626052523357x**. All **120**
  measured Apply workers retain identical source IDs, every body byte /
  block, and zero errors: **54,697,320 native bytes / 3,594,960 blocks /
  746,520 hidden trampoline bytes**, exactly unchanged.
- Matched same-worker/source richards profiles use **99 Hz / 100 loops /
  `BBMAP=0`**, with **280 → 255 samples** and **zero losses**. Disjoint
  leaf `PyObject_IsTrue` plus PLT falls **6.072336% → 0**, while the
  necessary existing `dp_jit_is_true` leaf rises
  **0.714157% → 4.314992%**. Thus the true disjoint combined truth leaf
  declines **6.786493% → 4.314992%**, or **2.471501 percentage points
  net**, not six points; actual `Richards.run` / `TaskState` source
  callers lose their generic C-call frames. Do not add overlapping
  inclusive ancestry or claim the full old C-call share as a net gain.

## Measurements

| Metric | Retained baseline | Candidate | Change |
| --- | --- | --- | --- |
| Fixed-eight stock / SOAC geometric score | 0.6273571181431998x; host-contaminated | 0.6345791409139968x | previous comparator noisy; not final repeated evidence |
| Fixed-eight official previous SOAC score | 0.9683515036210124x; outlier-contaminated | 1.0532525776372081x | noisy retained baseline; clean repeated target is authoritative |
| Normally sampled richards | 27.683731 ms; noisy comparator | 25.455922 ms | 1.087516x [1.052810, 1.110215]; adjusted 1.053701x [1.011770, 1.089200] |
| Clean repeated richards | 27.146548 ms | 25.707173 ms | 1.055991x [1.026761, 1.076974]; adjusted 1.041139x [1.004715, 1.062791] |
| Clean repeated four-workload stock / SOAC score | 0.49747399350945193x | 0.4865323207896451x | control stock drift; full-suite 1.10x unmet |
| Clean repeated four-workload previous SOAC score | 1.0194276621869476x | 1.023626052523357x | repeated richards is the source-backed target |
| Lossless current richards C truth-call / PLT leaf | 280 samples; 6.072336% | 255 samples; 0% | C-call leaves removed; retained hook remains |
| Combined disjoint richards truth leaves | 6.786493% including 0.714157% existing hook | 4.314992% existing hook | 2.471501 percentage points net; not 6.072336 points |
| Fixed-eight optimized typed coverage | 2,866 blocks / 204 functions | 2,866 blocks / 204 functions | unchanged across all 80 workers |
| Repeated subset typed coverage per round | 2,265 blocks / 183 functions | 2,265 blocks / 183 functions | unchanged across all 120 workers |
| Pre-optimization BlockPy release-smoke bytes | 7,199,376 | pending | expected unchanged; unverified |
| Release-smoke Apply native bytes / blocks / hidden bytes | 2,238,468 / 147,712 / 36,500 | 2,238,468 / 147,712 / 36,500 | every measured PID / source row / body / hidden trampoline exactly unchanged |
| Fixed-eight Apply native bytes / blocks | 23,163,480 bytes / 1,524,480 blocks | 23,163,480 bytes / 1,524,480 blocks | every measured source row unchanged |
| Repeated three-round Apply native bytes / blocks | 54,697,320 bytes / 3,594,960 blocks | 54,697,320 bytes / 3,594,960 blocks | every measured source row unchanged |
| Fixed-eight / repeated hidden trampoline bytes | 365,000 / 746,520 | 365,000 / 746,520 | unchanged |
| Real transformed singleton/custom-object semantics | actual stock/Profile/Verify/Apply GREEN 1 passed / 2.12 s | final candidate GREEN | no CPython mismatch; semantics preserved |
| Actual production-path structured singleton decision | genuine RED 0 passed / 1 failed / 572 filtered; actual None vs Some(1) | 1 passed / 572 filtered | actual exported-hook optimization decision RED-to-GREEN |
| Full `just test-all` correctness gate | retained immediate-method gate passed | 1,233 Python nodeids / 96 batches / 8 workers / 0 failures | GREEN; JIT 573, optimizer 213, typed 54, lowering 371, PyO3 8 |

## Attempt history

### Attempt 1: classify three immutable singleton identities in the existing hook

- Change: one existing-file local identity fast path in `is_true_hook`
  is **saved / package-formatted / independently reviewed**; exact
  singleton pointers use a pure private classifier, with the original
  null error first, generic `PyObject_IsTrue` fallback, public symbol,
  ownership, and every custom truthiness callback preserved.
- Measurements and coverage: current retained richards **280** lossless
  samples show **6.072 percentage points gross** in disjoint truth-call /
  PLT leaves. Candidate release smoke **174435** passes **8 / 8** with
  exact **397** source rows / **204** bodies / **2,238,468 bytes /
  147,712 blocks / 36,500 hidden bytes** and zero errors; cold timing is
  invalid. Normal richards is **1.087516x [1.052810, 1.110215]** /
  adjusted **1.053701x**, but its retained comparator is noisy. Clean
  independent three-round richards improves **1.055991x [1.026761,
  1.076974]** / adjusted **1.041139x [1.004715, 1.062791]**; all three
  rounds win despite five candidate outliers. Comprehensions is
  **0.954158x adjusted** under approximately **6.5%** faster stock; make
  no comprehensions gain claim. All **80 / 120** source/native rows /
  hidden trampolines are invariant; matched zero-loss profile shows
  **2.471501 percentage points net** truth-leaf reduction.
- Compatibility and tests: pinned CPython already checks the exact same
  three singleton identities. Real unchanged-production stock /
  transformed **Profile → Verify → Apply** singleton/custom truthiness
  integration is **GREEN: 1 passed / 2.12 seconds**, with branch outcomes
  and source bodies confirmed. An earlier disposable mismatch came only
  from incorrect stock fixture `__name__` in an `InvalidBool` error and
  was fixed in the fixture; there is no existing production correctness
  failure. Actual pinned-runtime production/exported-hook structured
  decision regression is genuinely **RED: 0 passed / 1 failed / 572
  filtered**, solely because singleton classification is **`None`**
  instead of **`Some(1)`**; singleton/refcount/custom callback/error/null
  behavior already passes. The same actual exported-hook structured
  decision now genuinely turns **GREEN: 1 passed / 572 filtered**.
  Final candidate stock/Profile/Verify/Apply integration, JIT
  **573 / 573**, optimizer **213 / 213**, typed IR **54 / 54**, broad
  transformed **16 / 16**, package formatting / format check, and Cargo
  `--tests` check are all **GREEN**; release smoke is **8 / 8 GREEN**.
  Normal and clean repeated benchmarks / matched profiles are complete;
  the authoritative full gate is **GREEN: 1,233 nodeids / 96 batches /
  8 workers / zero failures**.
- Result: **LANDED CANDIDATE / RETAIN; genuine repeated richards improvement,
  unchanged native code / semantics, disclosed stock-drift controls; no
  user-visible behavior bug asserted; full authoritative gate GREEN**.
- Reason: direct immutable identity checks may remove avoidable C-call
  boundaries from real boolean-heavy loops, but the retained hook,
  non-singleton fallback, finite sampling, and whole-workload overhead
  prevent turning a gross profile share into a promised improvement.

## Verdict and next action

- Verdict: **LANDED CANDIDATE / RETAIN; source-grounded one-file optimization;
  no CPython behavior mismatch; genuine transformed baseline GREEN
  1 / 2.12 seconds; genuine production-path structured decision
  RED-to-GREEN 1 passed / 572 filtered; sole implementation saved /
  formatted / reviewed; final transformed integration / JIT 573 /
  optimizer 213 / typed 54 / broad transformed 16 / scoped checks GREEN;
  exact-native release smoke GREEN 8 / 8; clean repeated richards
  1.055991x raw / 1.041139x adjusted with all rounds above neutral;
  matched net truth leaf -2.471501 points, invariant native code,
  comprehensions adjusted regression / stock drift candidly disclosed;
  authoritative full gate GREEN 1,233 Python nodeids / 96 passing batches
  / 8 workers / zero failures**.
- Transferable lesson: immutable singleton identities are stronger
  semantics than observed bool-like types; profile ancestry and GC /
  pending work are not automatically removable costs.
- Next action: integrate the fully validated retained candidate.
  Full-suite stock **1.10x** remains
  unmet and unmeasured.
