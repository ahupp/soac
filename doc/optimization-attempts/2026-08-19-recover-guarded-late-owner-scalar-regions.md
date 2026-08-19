---
title: "Recover Guarded Late-Owner Scalar Regions"
---

# Recover guarded late-owner scalar regions

- Status: **LANDED / RETAIN; REPEATED AFFECTED-WORKLOAD BENEFIT VERIFIED,
  FULL CORRECTNESS GATE PASSED**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`zssttuox`**, commit
  **`fa4c3c77`**.
- Candidate revision: change **`nzlwkyzw`**.
- Outcome: the first proposed sidecar-recognition fix is **insufficient**:
  actual invalidated scalar regions access non-`self`/top-level receivers and
  therefore have no existing late-owner sidecars to recognize. The revised
  hypothesis reuses already-published, same-owner/same-attribute split-dict
  constructor/method anchor cells for validated arbitrary-local region inputs.

## Hypothesis and evidence

- General-purpose opportunity: preserve selected integer/scalar regions when
  their attribute inputs can reuse an already-published, sound late-bound
  exact-owner split-dictionary guard from the same owner and attribute.
  This addresses ordinary mutable Python object/attribute code rather than
  recognizing benchmark-specific control-flow shapes.
- The retained baseline emits
  `typed_scalar_regions_invalidated_without_live_indexed_field_guards` for
  actual transformed hot functions. `deltablue` `chain_test` and
  `projection_test` each lose **two branch regions** per observed function
  event; `richards` `HandlerTask.fn` and `WorkTask.fn` each lose **one**.
  Matching only the **10 measured Apply worker PIDs** gives **10 invalidation
  events per affected function**, or **20 events per workload**. Each delta
  event invalidates two regions; each richards event invalidates one. Raw
  logs contain **11 events per function** because they also include a later
  native-profile replay PID; that replay and all Profile workers must be
  excluded. The exact invalidations survived the preceding owner-field change
  unchanged.
- Crucial source-backed correction: `chain_test` / `projection_test` are
  top-level functions, and the richards missing inputs are non-`self`
  `pkt.kind` and `w.destination` / `w.count`. The existing late-owner catalog
  admits only the first receiver parameter of class methods; **none of these
  invalidated input sites has a `LateBoundOwnerField` sidecar**. Teaching the
  validator or emitter to accept existing late sidecars alone therefore
  recovers **zero** target regions.
- Existing deterministic constructor/method anchors do identify the required
  owner/attribute cells: `Variable.__init__.value`, `Packet.__init__.kind`,
  and `WorkerTaskRec.__init__.destination` / `count`. The revised proposal
  reuses these exact same-owner/same-attribute `SplitDict` anchor cells for
  independently profiled arbitrary-local `RegionInputSource` accesses. It
  does not expand the catalog, publication protocol, or owner-cell table.
- A genuine unchanged-production integration RED now independently proves
  the proposed anchor/consumer boundary:
  `tests/test_late_owner_scalar_regions.py` **fails one test in 1.65
  seconds**, at line 316. Profile already records at least **32 generic
  constructor stores and non-`self` reads**, owner-specific `type_keys`, and
  exact-int pair shape **`0x0101`**. Existing constructor anchors already
  specialize: `Record.__init__` records **33 indexed hits**,
  `Packet.__init__` **33**, and `WorkState.__init__` **66**. Nevertheless,
  `record_branch`, `Handler.packet_branch`, and `Handler.state_branch` each
  record **zero indexed hits**; both Verify and Apply each log **one
  discarded branch per target**, with missing field source **`InstrId(1)`**
  and **no return invalidations**. Existing CPython-visible guard/semantic
  assertions pass in **both** modes; the failure is actual missing scalar
  applicability, not an existing incorrect Python result.
- A second genuine unchanged-production structured optimizer RED isolates
  the missing decision:
  `late_bound_scalar_regions_reuse_existing_split_owner_constructor_cells`
  **fails one test / 207 filtered** under `cargo test -p soac_opt ... --lib`;
  `work/logs/late-owner-scalar-opt-red.log` records that the already-selected
  borrowed region input and matching `Record.value` constructor owner cell
  both exist, yet the selected consumer plans are exactly **`[]`**.
- The same genuine structured optimizer regression now **passes one test**:
  `late_bound_scalar_regions_reuse_existing_split_owner_constructor_cells`
  proves on an actual lowered fixture that **both top-level and non-`self`**
  already-selected borrowed scalar inputs reuse the exact existing
  same-owner/same-attribute constructor cell. This establishes plan
  selection, not complete guarded JIT execution or end-to-end GREEN.
- The focused JIT guard family
  `cargo test -p soac_jit exact_int_indexed_field_sidecars_ --lib` also
  **passes 5 / 5**. Matching new late-bound `SplitDict` owner guards preserve
  both branch and return selections; mismatched owner module/class,
  attribute, index, `ObjectSlot` storage, and foreign runtime module are
  rejected. Existing legacy/missing-guard behavior remains covered. These
  structured checks do not yet establish end-to-end candidate execution.
- A broader focused JIT family,
  `cargo test -p soac_jit late_bound --lib`, now **passes 6 / 6**. Structured
  coverage proves selected/remapped same-module inline sidecars attach,
  stale continuation clones reconcile, foreign runtime-module/index
  mismatches reject, `Generic`/ordinary `IndexedField` controls preserve
  existing behavior, original ABI/source precedence remains intact, and the
  entire selected subtree stays atomic. A real transformed forced-inline
  Python fixture now **passes 1 / 1.78 seconds** across Profile, Verify, and
  Apply.
- The saved forced-inline fixture trains a transformed same-module `StoreTo`
  wrapper **32 times**, requires at least **32 Profile `call_hot_targets`**,
  checks structured DEBUG inline-rewrite evidence in both Verify and Apply,
  and asserts exactly-once subclass/property behavior. Its full enhanced
  runtime execution now **passes 1 / 1.78 seconds**, proving actual `StoreTo`
  inline rewrite and function/instruction source remapping in **both**
  Verify and Apply; direct and genuinely inlined subclass/property hooks
  each execute exactly once.
- The production implementation subsequently **removes approximately 120
  lines of duplicated artifact-specific annotation**. One selective,
  exact-matching seeded/remapped-plan helper now handles original
  instructions and inlined/stale continuation clones at the start of every
  fixpoint iteration and after remap/clone/late attachment. Structured JIT
  `late_bound` tests remain **6 / 6 GREEN** after consolidation, and the full
  enhanced genuinely inlined Profile/Verify/Apply integration is freshly
  revalidated **1 passed / 1.90 seconds**; its earlier **1.78-second** pass
  predates the cleanup.
- The first broad interpreter-aligned Cargo gate now **passes 261 tests**:
  typed-IR **53 / 53** plus optimizer **208 / 208**, including existing
  fused atomic-expression, virtual-object, ownership, and inlining coverage
  as well as the new split-owner constructor-anchor regression. Its
  **14.18-second build** recompiles only changed SOAC crates, with **no
  PyO3 rebuild**; later broad JIT/Python, benchmark, and full-gate outcomes
  are recorded below.
- The complete `soac_jit` library subsequently **passes all 557 / 557 tests
  in 5.54 seconds**, bringing validated Rust library totals to **818 / 818**:
  typed-IR **53**, optimizer **208**, and JIT **557**. This covers existing
  exact-int, late-owner, fused-expression, precompiled-code, inlining,
  virtual-object, deoptimization, and ownership regressions. Interpreter
  alignment continues to avoid PyO3 rebuilds. The combined
  `cargo check -p soac_ir_typed -p soac_opt -p soac_jit --tests`
  subsequently **passes in 8.01 seconds**, and package-scoped formatting plus
  format checks **pass for all three crates**. The initial broad
  single-process `just pytest-fast -q tests/` run reports
  **1,187 passed, 2 skipped, 7 deselected, 26 xfailed, and 3 failed in
  277.82 seconds**. Its three existing-coverage failures are
  `tests/test_opt_cases.py::test_opt_case_verify_counters[indexed_field_branch_compare]`,
  `test_soac_function_can_evaluate_multiple_generator_expressions`, and
  `test_soac_except_body_sets_cpython_handled_exception`. The indexed-field
  case requires `branch_fields` **`indexed_hit >= 2` but observes `0`**.
  The generator and handled-exception cases both observed the **same leaked
  `asyncio.TimeoutError` in `sys.exception()`** during that single-process
  run, but both **pass in a fresh focused process containing the same three
  tests**. This establishes broad-process exception-state contamination for
  those two cases, not independent reproducible candidate regressions. The
  indexed-field case initially still failed in that fresh process, then
  genuinely passes after the private source-aware counter repair described
  below. After that repair, the exact existing indexed-field counter case,
  genuinely inlined integration, and both isolated exception-state cases
  together **pass 4 / 4**. All affected Rust libraries are also rerun GREEN:
  typed-IR **53**, optimizer **208**, and JIT **557**, totaling **818 / 818**;
  scoped formatting is complete. The post-repair aligned combined Cargo
  test-target check subsequently **passes in 3.75 seconds**. A grouped
  transformed-runtime suite subsequently **passes all 67 selected tests in
  35.04 seconds, with 7 deselected**, covering all optimization counter
  cases, the new scalar strategy, prior late-owner/fused strategies, broad
  imports, synthetic metadata, and closure cache without a PyO3 rebuild.
  Scoped three-package formatting and format checks also pass. Production is
  frozen. Release debug-single smoke completes **8 / 8**; representative
  normal fixed-eight candidate completes **8 / 8**; a subsequent three-round
  affected-workload repeat resolves its initial deltablue concern and
  supports **RETAIN**. The authoritative full correctness gate also passes.
- Release debug-single smoke
  `work/pyperformance/comparison-20260819-022546-zY88oc` completes all
  **eight workloads**, preserving **3,069 optimized typed blocks / 218
  functions**. Actual measured-worker code/event evidence recovers exactly
  **three of four** affected functions: `deltablue.chain_test` and richards
  `HandlerTask.fn` / `WorkTask.fn` no longer emit their prior scalar-region
  invalidations. `deltablue.projection_test` still invalidates **two**
  missing sources, **#126 / #365**, and its generated body remains exactly
  **84,980 bytes / 5,507 machine blocks**. The unresolved projection gap is
  caused by insufficient debug-smoke profiling evidence, as the subsequent
  normal comparison confirms.
- PID-matched integrated-baseline→**debug-smoke** generated code changes are
  `chain_test` **44,184 bytes / 2,918 blocks → 44,572 / 2,939**,
  `HandlerTask.fn` **10,352 / 656 → 10,576 / 671**, and `WorkTask.fn`
  **28,676 / 1,900 → 26,072 / 1,719**. Direct benchmark bodies total
  **426,396 → 423,560 bytes (-0.665%)** for delta and
  **331,288 → 319,008 bytes (-3.707%)** for richards. `Packet.__init__`
  remains unchanged; `WorkerTaskRec.__init__` shrinks **1,812 → 856
  bytes**. Debug-single elapsed values contain lazy-JIT cold compilation
  and are **not steady-state throughput evidence**.
- The representative normal fixed-eight comparison
  `work/pyperformance/comparison-20260819-022850-9kkx1m` subsequently completes
  **8 / 8** and recovers **all four target functions (six branch regions)**:
  neither deltablue
  nor richards emits any remaining scalar-region invalidation. The smoke-only
  `projection_test` gap arose because original sources **#114 and #126**
  each had just **1 observation**, below the required hot threshold **8**;
  the normal profile supplies **8 observations each**. Source **#365** is
  an unprofiled continuation-cloned instruction; its specific original
  source has not been established.
  Thus debug-single coverage is insufficient to infer normal profile
  eligibility.
- Exact normal arithmetic-geometric scores are
  **0.48444263615875466x versus stock** and
  **1.0291882636176903x versus previous SOAC**; the outlier-resistant
  previous-SOAC median geometric ratio is only **1.00948x**, and the stock
  median geometric ratio is approximately **0.4892x**. Deltablue median
  changes **4.23111 → 4.54011 ms**, or **0.93194x previous/current**, a
  apparent single-round **6.81% throughput slowdown** that is statistically
  neutral in the subsequent three-round repeat; its trimmed ratio is
  **0.93768x**, with bootstrap **95% interval 0.8606–0.9836x** in this
  single-round observation. Richards
  median changes **40.6679 → 39.4548 ms**, or **1.03075x** (**3.08%**),
  not the misleading ~31% improvement suggested by the means: prior values
  include **78 / 97 / 108 ms** outliers. Full native code changes
  **23,417,280 → 23,359,400 bytes (-0.24717%)**, machine blocks
  **1,553,260 → 1,549,290 (-0.25559%)**, and typed IR stays at
  **3,069 blocks / 218 functions**.
- Normal-worker generated bodies differ materially from debug-smoke shape:
  `projection_test` shrinks **84,980 → 81,184 bytes (-4.47%)**, `chain_test`
  grows **388 bytes**, `HandlerTask.fn` grows **224 bytes**, and
  `WorkTask.fn` shrinks **28,676 → 26,072 bytes**. Direct per-worker code
  totals are delta **426,396 → 422,988 bytes (-0.799%)** and richards
  **331,288 → 328,908 bytes (-0.718%)**. Unlike debug-single,
  `WorkerTaskRec.__init__` is **unchanged in the normal run**.
- The targeted independently ordered **three-round** comparison
  `work/pyperformance/comparison-20260819-023725-AuXBa1` yields pooled robust
  median ratios `chaos` **1.06100x**, `deltablue` **1.01251x**, and
  `richards` **1.03725x**; their robust geometric mean is **1.03673x**.
  Deltablue is **statistically neutral**, with an approximate confidence
  interval **0.97–1.046x**, so its initial one-round slowdown does not
  reproduce. All four target functions (six branch regions) recover in
  **every round**, covering **30 measured Apply workers per workload**. The
  subset paired stock geometric score remains only **0.462392x**. Arithmetic
  previous-SOAC **1.1111428x** and richards approximately **1.296x** are
  distorted by prior **78 / 97 / 108 ms** outliers and are not headlines.
  Full-eight stock scores **0.5127524704981717x →
  0.48444263615875466x** come from **different paired stock cohorts** and do
  not establish a real stock regression, stock parity, or the full-suite
  **1.10x** acceptance target.
- Matched delta native profiles both report **zero lost samples**:
  baseline **553 CPU-clock samples** versus candidate **429** over **400
  replay loops**. Attached replay changes **6.744 → 5.317 ms**, but this is
  diagnostic only: roughly **13.75%** of one profile is first-call compiler
  activity, replay uses **`warmups=0`**, and stack-unwind behavior also
  affects sample attribution. Matched richards native profiles are also
  verified zero-loss: baseline **632 CPU-clock samples** versus candidate
  **592** over **70 replay loops**. Attached replay changes **46.2268 →
  43.5983 ms (1.06029x)**, but remains diagnostic rather than headline
  throughput. Inclusive `GenericGetAttrWithDict` falls **23.73% → 21.11%**,
  `TryGetInstanceAttribute` **9.81% → 6.59%**, and `bind_function_args`
  **9.81% → 6.76%**; these shares overlap. Candidate first-call compilation
  contributes **7.94%** versus baseline **0.16%** because replay uses
  `warmups=0`. Primary performance evidence remains the **1.03673x**
  three-round robust subset result. The full correctness gate passes.
- The first real candidate Verify execution subsequently exposes a genuine
  **user-visible compatibility regression**: a subclass `__getattribute__`
  hook runs **twice**, producing `['subclass:get', 'subclass:get']`, whereas
  unchanged CPython/SOAC behavior invokes it **once**. The working hypothesis
  cause is now confirmed: the typed expression linearizer hoists the original
  late-owner `GetAttr` (and potentially its comparison) into an earlier
  temporary. The original subclass hook therefore executes once before the
  scalar-region guard; fallback reevaluates that expression and executes the
  observable hook a second time. Structured plan/guard success does not imply
  correct evaluation order or a safe runtime optimization.
- The separately reproduced legacy `indexed_field_branch_compare` counter
  regression remains **unexplained**, and prior caller-inline/virtualization
  explanations are **decisively retracted**. A direct actual Profile-to-Verify
  probe shows original `branch_fields` field sources **#6 and #9** each have
  Profile **`generic_getattr = 80`**, but Verify has
  **`indexed_hit = 0`, `indexed_fallback = 0`, and `generic = 0` for both**.
  The existing `Record.__init__` self stores nevertheless record **80 indexed
  hits each**. Structured events place constructor-store inlining **inside
  `branch_fields`** itself, with **one rewritten store / two mapped
  instruction IDs**; `exercise_branch_fields` has **one inline pass but no
  rewrite**. Therefore the regression is not caller inlining, field-guard
  misses, or generic fallback. Two proposed
  **Verify-only** repairs both fail: preserving regular remapped-indexed
  counter sources leaves **zero indexed hits**, and separately preserving
  live late-owner sidecars also leaves **zero indexed hits**. The earlier
  claim that `Record` was fully virtualized is **retracted**: the trusted
  fully-virtual path applies only to `range` / `IterRange`, so it cannot
  establish that mechanism for `Record`. Temporary structured
  instrumentation is filtering actual surviving late/regular plans,
  instruction/counter maps, and in-function rewrites before diagnosing a
  sound repair; the true cause is not yet established.
- Filtered actual code-generation tracing now proves the final
  `branch_fields` function contains ordinary `IndexedField` guards at **four
  instruction IDs: #6, #9, #40, and #43**. Verify counter definitions exist
  only for the original **#6 / #9**, and **no `LateBoundOwnerField`
  sidecars remain**. Constructor continuation cloning introduced **#40 /
  #43**. Inspecting actual selected scalar regions now **conclusively proves
  the counter-loss mechanism**: live ordinary indexed sidecars map clone
  **#40 to original function `1:4` source #6**, and clone **#43 to original
  function `1:4` source #9**. Counter definitions exist only at those
  originals, but the existing regular scalar emitter ignores each typed
  `counter_source` and increments by clone ID, producing zero reported hits.
  The approved minimal repair adds a **private regular source-aware map** for
  indexed hit/miss/fallback using the existing counter helper, with **no new
  public API**. The genuine existing
  `indexed_field_branch_compare` regression now **passes**: the private map
  credits indexed hits, misses, no-specialization, and fallback to the
  original callee/source. The original virtualization predicate is restored,
  both ineffective Verify clauses and temporary diagnostics are removed, and
  the enhanced genuinely inlined Profile/Verify/Apply integration **also
  passes in the same run**. Subsequent isolated exception-state checks,
  grouped gates, representative comparisons, and the full correctness gate
  also pass as documented below.
- Approved narrow repair: attach **only** an already-selected,
  validated late-owner exact-int scalar plan before expression linearization,
  then preserve that entire selected subtree atomically in
  `soac_opt/src/typed/linearize.rs`, analogous to the existing fused-float
  atomic-subtree rule. Generic and unselected expressions must retain their
  existing linearization. A third independent genuine structured RED,
  `late_bound_exact_int_scalar_linearization_preserves_selected_region_atomically`,
  now fails with actual nested getter sources **`[]`** instead of the
  required original source **`InstrId(1)`**, proving the selected getter was
  hoisted before its guard. The same regression requires unrelated generic
  and existing indexed exact-int trees to retain their ordinary hoisting.
  That exact structured linearizer regression now **passes**: the selected
  entire `Truthy` / comparison / getter subtree remains atomic, while
  unrelated `Generic` and ordinary `IndexedField` exact-int controls retain
  their existing hoisting. Attachment is selective for an already-selected
  borrowed input with exact source, owner, module, attribute, and index,
  and occurs per function **after the callee-module snapshot**. Foreign
  runtime-module/index negative controls pass within the **6 / 6** structured
  JIT family, which also validates same-module inline attachment and stale
  continuation reconciliation. The direct focused real transformed
  integration **passes one test in 1.54 seconds** across Profile, Verify,
  and Apply; the enhanced real forced-inline Python regression subsequently
  passes **1 / 1.78 seconds before consolidation** and **1 / 1.90 seconds
  after consolidation** across all three modes.
- Full focused end-to-end GREEN: all three top-level/non-`self` scalar
  consumers record **at least 16 indexed hits**, and their prior scalar
  invalidations disappear. Existing observable semantics pass in both
  specialized modes, including exactly-once subclass/descriptor callbacks,
  deleted/materialized/promoted dictionaries, owner rebinding, and unseeded
  shared-key first insertion. This proves the focused integration, not
  broad compatibility or representative performance. Same-module inlining
  and continuation clones have structured coverage; the enhanced real
  transformed forced-inline Python fixture additionally **passes 1 / 1.90
  seconds after consolidation** with actual inline/source-remap evidence in
  Verify and Apply.
- Baseline zero-loss native profiles show why these workloads remain useful,
  while also exposing the danger of assuming causality from sample shares:
  candidate `deltablue` generic-getter share was **12.296%** and generic
  setter **1.988%**; `richards` generic getter remained **23.735%** and
  generic setter **5.061%**. These inclusive stack percentages overlap.
- A fresh integrated-main `comprehensions` profile,
  `work/logs/guarded-scalar-baseline-comprehensions_record.txt` and
  accompanying Speedscope artifacts, records **916 CPU-clock samples, zero
  lost, and 57.450 MB**. Inclusive synthetic closure creation is **31.22%**,
  JIT/vectorcall registration **7.53%**, generator creation **19.00%**, and
  Python evaluation **22.82%**; these stack shares overlap and must not be
  summed. A semantic function-watcher/defaults/keyword-defaults investigation
  is a possible **separate future strategy**, not part of scalar recovery.
- Source-confirmed structural boundaries:
  `jit/mod.rs::typed_indexed_field_guards_by_instr` collects only
  `TypedAttrAccessPlan::IndexedField`; the
  `typed_pipeline.rs::invalidate_unguarded_exact_int_selections` validator
  consequently accepts only the older `TypeKey`/expected-index guard map;
  mechanical borrowed indexed-field emission consumes that same map. Existing
  `virtual_objects.rs` indexing assumptions remain a separate soundness
  boundary: this strategy must **not** broaden virtual-field trust or erase
  the real runtime guard. Simply weakening or deleting any validator would
  be unsound and still would not create the missing non-`self` provenance.
- Expected falsifiable outcome: selected source-keyed late-owner regions
  survive validation and produce native scalar operations behind complete
  owner/storage guards; unsupported or mutated owners still use the original
  Python path. A real before/after benchmark must establish whether any
  recovered regions improve overall performance without worsening existing
  `comprehensions`/`richards` regressions or native-code growth.

## Implementation and compatibility

- Proposed implementation shape: a post-planner consumes only
  already-selected, borrowed `RegionInputSource::IndexedField` scalar inputs
  with existing hot profile evidence of at least **8** observations, reusing
  **existing deterministic same-owner/same-attribute `SplitDict` constructor
  anchor cells** only where the owner, attribute, and index match uniquely.
  Carry source-keyed owner type, attribute name, expected index, and existing
  dense owner-cell identity in a validated typed sidecar. Preserve the old
  indexed-field guard map unchanged; add a separate owner-aware typed guard
  map consumed by scalar-region validation and mechanical guarded input
  emission. Do **not** add catalog entries, publish new cells, support
  slots, modify `virtual_objects`, or weaken any existing guard boundary.
- Validate original function/instruction identity, arbitrary receiver local,
  same module/owner, exact same attribute, `SplitDict` storage, expected
  index, and anchor-cell identity. Preserve source remapping through
  supported same-module inlining and portable FunctionEnv-relative access.
  Unsupported slots, cross-module, dynamic, inherited, mismatched, missing,
  or ambiguous anchors must retain the full generic scalar-region fallback.
- Guard lifetime: a late-owner capability is usable only while its weak owner
  still denotes the receiver's **exact live class** and the captured nonzero
  `tp_version_tag` still matches. Supported split fields additionally require
  unmaterialized valid inline values, capacity,
  split-kind cached keys, expected index below `dk_nentries`, and exact
  interned shared-key identity. Recheck these facts on each executed fast
  path; there is no global cache, permanent assumption, or expiration timer.
- CPython-visible behavior: preserve subclass and descriptor dispatch, custom
  hooks, dictionary materialization and
  promotion, missing shared-key first stores, owner replacement/collection,
  user callbacks, exception timing, evaluation order, and borrowed/owned
  reference lifetimes. Stores retain INCREF/swap/DECREF ordering. A guard
  miss must execute the complete original generic/deoptimization path.
  The first candidate violates this requirement by duplicating a subclass
  callback on guard miss; preserving exactly-once evaluation is a mandatory
  unresolved correctness prerequisite.
- Depending on verified details of the exact pinned CPython, or modifying the
  vendored CPython where genuinely appropriate, is permitted; Rust
  `repr(Rust)` declarations are not C-layout proofs. Use explicit ABI/layout
  representations and preserve all user-visible CPython behavior.
- Focused regression coverage: the genuine unchanged-production integration
  **fails one test in 1.65 seconds**, proving three existing constructor
  anchors but three missing borrowed scalar-region consumers in both Verify
  and Apply. The independent structured optimizer regression also **fails
  one test / 207 filtered**, proving that both `Record.value` anchor and
  borrowed scalar input exist but no consumer plan is selected; its actual
  lowered top-level/non-`self` fixture now **passes one test** after
  implementation. Four approved production files have changed. The focused
  JIT sidecar/guard family now **passes 5 / 5**, including owner/module,
  attribute, index, slot, foreign-module, and legacy/missing rejection;
  first end-to-end candidate Verify remains **RED** because pre-guard
  linearization plus scalar fallback executes subclass `__getattribute__`
  twice instead of once. Its independent structured atomic-linearizer
  regression genuinely failed with getter sources `[]` instead of original
  `InstrId(1)`, then **passes** after preserving the entire selected
  `Truthy`/comparison/getter subtree. Generic and ordinary indexed controls
  remain hoisted. The full transformed Profile/Verify/Apply integration now
  **passes 1 / 1.54 seconds**. Structured same-module inlining and stale
  continuation reconciliation now pass within the JIT **6 / 6** family;
  end-to-end forced-inline Python coverage now also **passes 1 / 1.90
  seconds after consolidation** with actual same-module rewrite/source-remap evidence in both
  Verify and Apply.
  Eventual checks
  include arbitrary-local owner/attribute anchor matching, preserved selected
  region, emitted complete split-owner guard and native operations,
  weak-owner/type-version/key mutation fallback, ambiguous/mismatched/slot
  rejection, old indexed-field compatibility, and unchanged precompiled
  behavior.
- Implementation, candidate Profile/Verify/Apply execution, and fixed-eight /
  three-round targeted benchmark results are verified; the authoritative
  full correctness gate also **passes**.

## Benchmark protocol and coverage

- Fixed benchmark selection: `chaos`, `comprehensions`, `deltablue`,
  `fannkuch`, `float`, `nbody`, `richards`, and `spectral_norm`. The current
  historical fixed-eight comparison is exploratory **one round**; any final
  broad performance claim requires the protocol in `OPT_GOAL.md`, including
  independently profiled revisions and at least three alternating rounds.
- Authoritative integrated-main baseline:
  `work/pyperformance/comparison-20260819-003845-YpCssx/summary.json`.
- Previous strategy's independent two-round affected-workload evidence:
  `work/pyperformance/comparison-20260819-004559-eJ14ia/summary.json`.
  These are **baseline history**, not measurements of the new strategy.
- Profile evidence: independently regenerate profile evidence for each
  candidate revision; never reuse a prior compiler revision's instruction
  identities as if they were validated current evidence.
- Module selection: benchmark `__main__` plus compiler-owned `soac.runtime`;
  no transformed standard-library modules in the recorded baseline.
- Baseline completion: all eight benchmarks complete. Actual per-benchmark
  compiled-function coverage is **34 / 21 / 78 / 1 / 9 / 8 / 53 / 9**,
  respectively. The candidate also completes **8 / 8**, recovers all four
  target functions (six branch regions), and has no failed benchmarks.
- Precise generated-code baselines below match each workload's **10 measured
  Apply worker PIDs** from worker-timing evidence; they exclude Profile and
  subsequent native-perf replay processes. Per-worker **all** rows include
  nested/auxiliary generated bodies; **direct** rows match the benchmark's
  reported transformed function coverage.

| PID-matched Apply workload / function | Baseline native bytes | Baseline machine blocks | Scope |
| --- | --- | --- | --- |
| `deltablue.chain_test` | 44,184 | 2,918 | one measured Apply worker |
| `deltablue.projection_test` | 84,980 | 5,507 | one measured Apply worker |
| `deltablue.Variable.__init__` | 5,928 | 373 | one measured Apply worker |
| `deltablue`, all generated rows | 433,392 | 28,423 | 156 rows / worker |
| `deltablue`, direct transformed bodies | 426,396 | 27,973 | 78 functions / worker |
| `richards.HandlerTask.fn` | 10,352 | 656 | one measured Apply worker |
| `richards.WorkTask.fn` | 28,676 | 1,900 | one measured Apply worker |
| `richards.Packet.__init__` | 3,852 | 239 | one measured Apply worker |
| `richards.WorkerTaskRec.__init__` | 1,812 | 119 | one measured Apply worker |
| `richards`, all generated rows | 336,552 | 22,847 | 105 rows / worker |
| `richards`, direct transformed bodies | 331,288 | 22,487 | 53 functions / worker |

| Baseline workload | Paired stock mean | Integrated SOAC mean | Prior SOAC / integrated SOAC median | Compiled functions | New candidate |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 30.0260 ms | 61.0314 ms | 1.11374x | 34 | pending |
| `comprehensions` | 7.7853 us | 84.9822 us | 0.96539x, regression | 21 | pending |
| `deltablue` | 1.4651 ms | 4.2357 ms | 1.05241x | 78 | pending |
| `fannkuch` | 181.9451 ms | 256.5647 ms | 1.00765x | 1 | pending |
| `float` | 65.4273 ms, contaminated | 40.9300 ms | 1.27966x | 9 | pending |
| `nbody` | 48.6301 ms | 66.1440 ms | 0.98324x | 8 | pending |
| `richards` | 22.2625 ms | 52.2815 ms | 0.96820x, regression | 53 | pending |
| `spectral_norm` | 48.9935 ms | 56.7293 ms | 0.98763x | 9 | pending |

The current baseline's paired stock geometric score is
**0.5127524704981717x**. Its previous-SOAC mean geometric ratio was
**1.1168014647654891x**, but baseline `chaos`/`spectral_norm` outliers make
the robust median geometric ratio **1.0403088969105239x** more informative.
The paired stock `float` run is contaminated; its apparent stock-beating
ratio is not evidence of stock parity. The previous five-workload two-round
comparison has robust geometric ratio **1.0662174681x**, with `float`
**1.2692356x**, `chaos` **1.1122241x**, and `deltablue` **1.0440702x**,
but reproducible `comprehensions` **0.9624948x** and `richards`
**0.9713289x** regressions. Neither the eight-workload subset nor the
five-workload repeat establishes the full-suite **1.10x stock** goal.

## Measurements

| Metric | Integrated baseline | New candidate | Change |
| --- | --- | --- | --- |
| Completed fixed-eight benchmarks | 8 / 8 | 8 / 8 | complete |
| Fixed-eight paired stock / SOAC geometric ratio | 0.5127524704981717x | 0.48444263615875466x; robust approximately 0.4892x | below baseline and stock |
| Previous historical SOAC / baseline robust ratio | 1.0403088969105239x | arithmetic 1.0291882636176903x; robust 1.00948x | mixed; delta regresses |
| Targeted three-round affected-workload robust ratio | same integrated baseline | 1.03673x; delta neutral 1.01251x | retained |
| Targeted three-workload paired stock geometric ratio | not comparable to full-eight cohort | 0.462392x | below stock; not full-suite acceptance |
| Optimized typed-IR blocks / functions | 3,069 / 218 | 3,069 / 218 | unchanged |
| Pre-optimization serialized BlockPy bytes | 14,398,752 | pending | pending |
| Apply-mode native code bytes | 23,417,280 | 23,359,400 | -0.24717% |
| Apply-mode machine blocks | 1,553,260 | 1,549,290 | -0.25559% |
| `deltablue` missing guarded scalar regions | two each in `chain_test` / `projection_test` | both functions recovered in normal profile; no invalidations | two of two functions recovered |
| `richards` missing guarded scalar regions | one each in `HandlerTask.fn` / `WorkTask.fn` | both functions recovered | two of two functions recovered |
| Release debug-single fixed-eight smoke | not applicable | 8 / 8 passed; timings cold-contaminated | correctness/code evidence only |
| New-strategy focused integration baseline / candidate | 1 failed / 1.65 s; first candidate duplicate callback | Profile/Verify/Apply 1 passed / 1.54 s | genuine RED-to-GREEN |
| New-strategy structured optimizer RED / GREEN | 1 failed / 207 filtered | 1 passed | genuine RED-to-GREEN |
| Complete typed-IR / optimizer Rust libraries | existing suites | 53 / 53 + 208 / 208 | 261 passed |
| Complete JIT Rust library | existing suite | 557 / 557 in 5.54 s | all Rust libraries 818 / 818 |
| Combined three-package Cargo test-target check | not applicable | post-repair passed in 3.75 s; earlier 8.01 s | GREEN |
| Grouped transformed-runtime regression suite | not applicable | 67 / 67 passed in 35.04 s; 7 deselected | GREEN |
| Three-package scoped Rust formatting / check | not applicable | passed | GREEN |
| Complete transformed Python tests | existing suite | full gate 1,218 nodeids / 85 file-local batches / 8 workers | all 85 batches passed |
| New-strategy JIT owner/mismatch guard regressions | not applicable | 5 / 5 passed | structured GREEN |
| Broader JIT late-owner inline/continuation regressions | not applicable | 6 / 6 passed | structured GREEN |
| New-strategy selected-subtree linearizer RED / GREEN | failed: getter sources [] instead of InstrId(1) | selected subtree retained; controls hoisted | genuine RED-to-GREEN |
| Same-module inlining / continuation-clone regressions | no direct Python case | structured 6 / 6; forced-inline runtime 1 passed / 1.90 s after consolidation | structured and runtime GREEN |
| New-strategy full `just test-all` correctness gate | prior integrated gate | 85 / 85 Python batches; all Rust suites | passed in 171.606 s test phase |

## Attempt history

### Attempt 1: accept existing late-owner sidecars

- Change: documentation and host source/artifact analysis only; **no
  production code or regression test changed** for this strategy.
- Measurements and coverage: integrated-main fixed-eight and prior targeted
  results above are real historical baselines. New-strategy performance,
  generated code, region recovery, and statistical comparisons are
  **pending**.
- Compatibility and tests: inherited weak-owner, exact-type/version, live
  split-key/slot, fallback, and refcount requirements identified. The
  subsequent genuine baseline regression is recorded in Attempt 2;
  candidate compatibility validation remains **pending**.
- Result: **REJECTED AS INSUFFICIENT**; no implementation was attempted.
- Reason: the relevant top-level/non-`self` source instructions have no
  late-owner sidecars because the existing catalog only admits class-method
  first-receiver accesses. Accepting existing sidecars therefore recovers
  **zero** observed target regions.

### Attempt 2: reuse validated existing split-dictionary owner anchors

- Change: identify existing `Variable.value`, `Packet.kind`, and
  `WorkerTaskRec.destination` / `count` constructor/method anchor cells for
  exact same-owner/same-attribute arbitrary-local region inputs. A minimal
  actual integration uses `Record.__init__`, `Packet.__init__`, and
  `WorkState.__init__`; the production proposal remains restricted to
  selected borrowed scalar inputs with **no new catalog entries,
  publication, cells, slots, or `virtual_objects` changes**.
- Measurements and coverage: retained integrated-main baseline and fresh
  zero-loss `comprehensions` profile above; candidate code, recovered scalar
  regions, before/after throughput, and generated-code deltas are **pending**.
- Compatibility and tests: genuine unchanged-production integration RED
  **one failed / 1.65 seconds**; Profile evidence and constructor anchors
  pass, both Verify/Apply Python semantic guards pass, but each mode drops
  exactly one selected branch for each of three target consumers. Rebuilding
  **six crates in 23.35 seconds** and a benign Rosetta warning were workflow
  overhead, not benchmark or application-performance measurements. A second
  genuine structured optimizer RED **fails one test / 207 filtered**:
  `Record.value` constructor cell and borrowed input are present, but
  consumer plans are `[]`; see `work/logs/late-owner-scalar-opt-red.log`.
  The exact same structured optimizer regression now **passes one test**,
  proving both actual lowered top-level and non-`self` borrowed consumers
  reuse the existing exact same-owner/attribute constructor cell. Four
  production files have changed; the focused JIT owner-guard family now
  **passes 5 / 5**, retaining matching branch/return regions while rejecting
  mismatched owner/module/attribute/index, slots, foreign modules, and
  missing guards. The first real candidate Verify execution instead reveals
  an unsafe subclass callback sequence, `['subclass:get', 'subclass:get']`,
  where the prior sequence has exactly one callback. Source tracing confirms
  that the typed linearizer hoists the original late-owner getter before the
  selected scalar guard, then fallback reevaluates it. A narrowly approved
  **fifth** production file, `soac_opt/src/typed/linearize.rs`, may preserve
  only an already-selected validated late-owner scalar subtree atomically,
  without changing generic expression lowering. The third independent
  structured regression
  `late_bound_exact_int_scalar_linearization_preserves_selected_region_atomically`
  genuinely failed because original nested getter source `InstrId(1)` was
  absent (`[]`), then **passes** with the complete selected
  `Truthy`/comparison/getter subtree kept atomic. Unrelated generic and
  ordinary indexed exact-int trees still hoist normally; selective matching
  checks borrowed representation, source, owner, module, attribute, and
  index after the callee-module snapshot. The broader focused JIT
  `late_bound` family **passes 6 / 6**, including remapped same-module inline
  attachment, stale continuation-clone reconciliation, foreign runtime
  module/index rejection, generic/indexed controls, original ABI/source
  precedence, and atomic selected-tree preservation. The repaired focused
  transformed integration now **passes 1 / 1.54 seconds** across Profile,
  Verify, and Apply: all three non-`self` consumers record at least **16
  indexed hits**, no scalar regions are invalidated, and subclass/descriptor
  hooks run exactly once. Deleted/materialized/promoted dictionaries, owner
  rebinding, and unseeded first stores also retain correct behavior.
  The enhanced forced-inline Python fixture trains a
  same-module `StoreTo` wrapper **32 times**, requires at least **32
  `call_hot_targets`**, checks Verify/Apply DEBUG inline-rewrite evidence,
  and asserts exactly-once subclass/property behavior. The enhanced real
  transformed Profile/Verify/Apply execution **passes 1 / 1.78 seconds**
  before cleanup and **passes again in 1.90 seconds after consolidation**,
  proving genuine same-module inline rewrite and original
  function/instruction-source remapping in both specialized modes. Direct
  and inlined hooks each run exactly once; all three consumers retain indexed
  hits with no invalidations and all mutation/unseeded-first-store guards
  pass. Approximately **120 duplicate annotator lines are removed** in favor
  of one exact-match seeded/remapped helper covering original instructions,
  same-module inlining, and stale continuation clones throughout fixpoint,
  remap, clone, and late-attachment phases. The post-consolidation JIT family
  remains **6 / 6 GREEN**. The first broad grouped interpreter-aligned
  Cargo gate now **passes all 261 tests**, typed-IR **53 / 53** and
  optimizer **208 / 208**; it includes prior fused/virtual/ownership/inline
  guardrails and the new owner-anchor selection. The complete JIT library
  subsequently **passes 557 / 557 in 5.54 seconds**, for **818 / 818**
  combined Rust library tests. The combined three-crate Cargo test-target
  check also **passes in 8.01 seconds**, and all three package-scoped
  formatting/check gates pass. The initial complete single-process
  `just pytest-fast -q tests/` run finishes with
  **1,187 passed, 2 skipped, 7 deselected, 26 xfailed, and 3 failed in
  277.82 seconds**. Its failures are
  `test_opt_case_verify_counters[indexed_field_branch_compare]` and
  `test_soac_function_can_evaluate_multiple_generator_expressions`, and
  `test_soac_except_body_sets_cpython_handled_exception`. The indexed-field
  assertion specifically requires `branch_fields` **`indexed_hit >= 2`**
  but gets **0**. Both other cases observe the same leaked
  `asyncio.TimeoutError` in `sys.exception()`. A fresh focused pytest
  process running these same three cases confirms **both exception-state
  cases PASS**, while the legacy indexed-field case still **FAILS**. The
  two extra broad failures are therefore verified single-process
  contamination, whereas the indexed-field loss is a real reproducible
  compatibility regression. Both proposed Verify-only preservation attempts
  fail: the regular remapped-indexed source guard and the live late-sidecar
  guard each leave **0 indexed hits**. Direct Profile/Verify probing proves
  `branch_fields` original sources **#6 / #9** each transition from
  **80 generic Profile hits** to **0 indexed hits / 0 indexed fallbacks / 0
  generic Verify hits**, while `Record.__init__` stores each retain **80
  indexed hits**. Actual structured events show **one constructor-store
  rewrite inside `branch_fields`**, not caller inlining;
  `exercise_branch_fields` has a pass but **no rewrite**. Earlier caller-inline
  and `Record` fully-virtualization explanations are retracted. Filtered
  actual codegen tracing now finds ordinary indexed guards at **#6 / #9 /
  #40 / #43**, Verify counter definitions only at original **#6 / #9**, no
  surviving late-owner sidecars, and constructor continuation clones **#40 /
  #43**. Actual selected scalar regions conclusively map clone **#40 →
  original function `1:4` source #6** and **#43 → original `1:4` source
  #9**. The regular scalar emitter discards this existing typed original
  `counter_source` and increments undefined clone IDs. The minimal private
  source-aware hit/miss/no-specialization/fallback map now reuses the
  existing helper and restores original callee/source attribution. The
  genuine legacy counter regression **passes**, as does the enhanced actual
  same-module-inline Profile/Verify/Apply integration in the same run.
  Both ineffective Verify clauses and temporary tracing are removed, the
  original virtualization predicate is restored, and no new public API is
  added. The final focused rerun now **passes 4 / 4**, covering the repaired
  exact legacy counter regression, enhanced genuinely inlined transformed
  integration, and both isolated exception-state cases. All affected Rust
  libraries are freshly rerun GREEN: typed-IR **53 / 53**, optimizer
  **208 / 208**, and JIT **557 / 557**, or **818 / 818** total; scoped
  formatting is done. The aligned post-repair combined three-package Cargo
  `--tests` check now **passes in 3.75 seconds**. The grouped transformed
  Python regression run **passes 67 / 67 in 35.04 seconds, with 7
  deselected**, spanning all optimization counters, new scalar, existing
  late-owner/fused, broad import, synthetic metadata, and closure-cache
  cases without a PyO3 rebuild. Scoped three-package formatting/checks also
  pass. Release debug-single smoke subsequently completes **8 / 8** and
  recovers `chain_test`, `HandlerTask.fn`, and `WorkTask.fn`; it does **not**
  recover `projection_test` sources **#126 / #365** because original
  **#114 / #126** each have only **1** smoke observation below the hot
  threshold **8**. The subsequent normal fixed-eight comparison completes
  **8 / 8**, provides **8 observations each**, and recovers **all four target
  functions (six branch regions)** with **zero** delta/richards
  invalidations. **#365 is an
  unprofiled continuation-cloned instruction**; its specific original source
  is not established. Full
  native code shrinks **0.24717%** and machine blocks **0.25559%**, with
  unchanged typed blocks/functions. Exact arithmetic scores are
  **0.48444263615875466x stock / 1.0291882636176903x previous SOAC**, but
  robust previous-SOAC geometric speedup is only **1.00948x**. Deltablue
  median throughput regresses **0.93194x**, with trimmed **0.93768x** and
  bootstrap 95% interval **0.8606–0.9836x**; richards improves a modest
  **1.03075x**, not the outlier-distorted ~31% suggested by means. Normal
  `projection_test` shrinks **84,980 → 81,184 bytes**, and direct
  delta/richards generated code shrinks **0.799% / 0.718%**; normal
  `WorkerTaskRec.__init__` remains unchanged despite its debug-smoke
  difference. The targeted **three-round** repeat subsequently establishes
  robust pooled `chaos` **1.06100x**, delta **1.01251x** (statistically
  neutral, approximate interval **0.97–1.046x**), and richards
  **1.03725x**; robust affected-subset geometric improvement is
  **1.03673x**, with all four target functions (six branch regions)
  recovered in all **30 Apply workers per affected workload**. Baseline
  richards outliers contaminate
  arithmetic ratios, and subset stock remains **0.462392x**. The decision is
  **RETAIN**; the full correctness gate also **passes**.
- Authoritative full `just test-all` gate **PASSES**:
  `work/logs/late-owner-scalar-test-all.log` records **1,218 Python nodeids
  across 85 file-local batches / 8 workers**, with **85 / 85 batches
  passing**. Rust suites also pass: **soac_jit 557**, **soac_ir_typed 53**,
  **soac_lowering 371**, **soac_opt 208**, and **PyO3 8**. Test-runtime
  preparation takes **1.320 seconds**, Cargo tests **77.286 seconds**,
  pytest **94.294 seconds internally / 94.308 seconds externally**, and the
  complete test phase **171.606 seconds**; the existing slow counter batch
  takes **93.95 seconds**. This is full correctness validation, not
  evidence that the full-suite stock-performance goal has been met.
- Workflow observation: raw Cargo previously lacked the Justfile's
  `PYO3_PYTHON` / `PYO3_PYTHON_REAL` settings, causing avoidable **25–30
  second PyO3 recompilations** when alternating with transformed pytest.
  The first grouped gate aligns those interpreter variables and compiles
  only changed SOAC crates in **14.18 seconds**, with **no PyO3 rebuild**.
  Build time is workflow overhead, not application-performance evidence.
- Workflow observation: the monolithic `just pytest-fast -q tests/` command
  shares one Python process across files and can leak handled
  `asyncio.TimeoutError` state into unrelated tests. The repository's
  `scripts/run_pytest_parallel.py` intentionally batches **per file** to
  isolate imports and `sys.modules`; authoritative `just test-all` uses that
  batched isolation. Single-process cross-file contamination must not be
  misreported as an independently reproducible compiler regression.
- Benchmark workflow observations: report robust median ratios alongside
  outlier-sensitive arithmetic means; filter subset baselines to the exact
  selected benchmark names before summarization; and avoid treating
  `warmups=0` native-profile replay or first-call compiler/unwind samples as
  steady-state throughput. Pair stock and SOAC within each comparison rather
  than treating stock results from different runs as identical.
- Additional workflow observation: a multiline `just py-fast ... -c` probe
  encountered argument-quoting issues; preserve multiline source as an exact
  argument or use an inspectable script so shell/recipe re-tokenization does
  not obscure transformed-runtime evidence.
- Result: **RETAIN; optimizer, JIT owner guards, selected-subtree
  linearizer, and focused Profile/Verify/Apply integration GREEN;
  structured inlining/continuation **6 / 6**; enhanced real forced-inline
  Profile/Verify/Apply **1 / 1.90 seconds after consolidation**; full
  Rust libraries **818 / 818**, post-repair combined test-target check
  **3.75 seconds**, grouped transformed Python **67 / 67 in 35.04 seconds**,
  and three-crate formatting GREEN; full-eight previous robust **1.00948x**,
  repeated targeted robust **1.03673x**, full correctness gate **85 / 85
  Python batches plus all Rust suites passed**.
- Reason: existing published cells provide a potential provenance bridge to
  arbitrary-local scalar inputs without adding owner lifetime roots or
  bypassing user-visible Python semantics.

## Verdict and next action

- Verdict: **LANDED / RETAIN; FULL CORRECTNESS GATE PASSED**;
  unchanged-production integration RED, structured optimizer RED-to-GREEN,
  focused JIT guards **5 / 5**, and the selected-subtree linearizer
  RED-to-GREEN are verified. The first real candidate Verify duplicated an
  observable subclass callback; the repaired focused Profile/Verify/Apply
  integration now **passes 1 / 1.54 seconds** with exactly-once callbacks.
  Same-module inlining and continuation reconciliation have structured JIT
  coverage **6 / 6**, and the enhanced real forced-inline integration passes
  **1 / 1.90 seconds after consolidating duplicate annotation**. Complete
  typed-IR/optimizer/JIT libraries pass **818 / 818**; the combined
  test-target check and scoped formatting also pass. The running complete
  Python suite has exposed existing indexed-field-counter,
  multiple-generator-expression, and handled-exception failures; exact
  assertions include a definite lost legacy indexed-field hit
  (**required >=2, actual 0**) that still fails in a fresh process. Both
  generator/handled-exception cases sharing `asyncio.TimeoutError` pass in
  that same fresh focused three-test process, establishing broad-process
  contamination. The reproducible counter regression survives both
  regular-remapped-source and live-late-sidecar Verify-only preservation
  attempts. Direct counter/event evidence disproves both caller-inlining and
  `Record` fully-virtualization explanations: constructor-store rewriting is
  inside `branch_fields`, whose two Profile-hot field sources disappear from
  all Verify counter categories. Filtered surviving plan/counter maps are
  now show four ordinary indexed sites **#6 / #9 / #40 / #43** versus two
  original Verify counters **#6 / #9**, with no late-owner sidecars.
  Constructor continuation cloning maps **#40 → original function `1:4`
  source #6** and **#43 → original `1:4` source #9**; the regular scalar
  emitter had ignored this typed original counter provenance. A private
  source-aware map now correctly credits hit/miss/no-specialization/fallback
  to the original source; the genuine legacy counter regression and enhanced
  forced-inline integration both **pass**. Existing virtualization is
  restored and temporary patches/diagnostics are removed without new public
  API. The final focused rerun passes **4 / 4**, including both isolated
  state cases, and all Rust libraries pass **818 / 818** again. Final
  combined test-target check passes in **3.75 seconds**. The grouped
  transformed-runtime regression run passes **67 / 67 in 35.04 seconds**,
  with **7 deselected**, and scoped formatting/checks pass. Release
  debug-single smoke recovers **three of four** target functions, whereas the
  normally profiled fixed-eight run completes **8 / 8** and recovers **all
  four**, with slightly smaller generated code. Full-eight robust previous
  geometric improvement is **1.00948x**; its one-round deltablue slowdown
  does not reproduce in the targeted three-round result, where delta is
  statistically neutral **1.01251x**, chaos is **1.06100x**, richards is
  **1.03725x**, and pooled robust geometric improvement is **1.03673x**.
  All affected regions recover across all **30 measured Apply workers** per
  workload. The full-eight candidate stock score is
  **0.48444263615875466x** and the affected-subset stock score is
  **0.462392x**; different paired stock cohorts are not direct before/after
  claims, and the full-suite **1.10x** goal remains unmet. The decision is
  **RETAIN**, and the authoritative full correctness gate passes **1,218
  Python nodeids / 85 file-local batches / 8 workers**, plus Rust
  **557 JIT / 53 typed-IR / 371 lowering / 208 optimizer / 8 PyO3**; see
  `work/logs/late-owner-scalar-test-all.log`.
- Transferable lesson: optimizing class-method `self` accesses does not
  establish a guard at unrelated top-level/non-`self` scalar inputs. Recovery
  requires validated same-owner/same-attribute provenance plus the complete
  existing runtime guard, not merely accepting another sidecar variant.
- Next action: preserve the existing no-new-cells,
  no-virtual-object-guard-erasure soundness boundaries.
