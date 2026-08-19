---
title: "Immediate Zero-Argument and Positional Method Call Dispatch"
---

# Immediate zero-argument and positional method call dispatch

- Status: **LANDED CANDIDATE / RETAIN; real transformed CPython bound-method-visibility
  correctness AND independent full-production typed-pipeline structured
  RED-to-GREEN verified for zero and one/two positional arguments;
  previous zero-argument full Rust / transformed suites and smoke GREEN;
  first normal timing INVALID under external contention; clean repeated
  zero-argument target and lossless residual profiles measured;
  source-reviewed two-file positional extension also turns both genuine
  REDs GREEN; first post-extension full JIT run RED on preexisting brittle
  collision test plus shared-mutex cascade; test-only repair and fresh
  full JIT 572 / 572, optimizer 213 / 213, typed 54 / 54, transformed
  15 / 15, scoped checks, and post-positional release smoke GREEN;
  positional normal target shows significant comprehensions REGRESSION;
  exact Apply cloned-instruction-ID root cause confirmed by genuine nested
  Apply-only correctness RED AND independent whole-production nested Verify
  structured RED both turn GREEN after audited one-file clone recovery;
  fresh post-clone JIT 572 / optimizer 213 / typed 54 / transformed 15 /
  scoped checks AND release smoke 8 / 8 GREEN; actual hot benchmark Apply
  selection confirmed; clean three-round deltablue 1.118447x and
  comprehensions neutral 0.998325x; matched lossless profiles complete;
  authoritative full correctness gate GREEN: 1,232 Python nodeids /
  95 passing batches / zero failures**.
- Pacific date: **2026-08-19 PDT**.
- Integrated baseline: retained `main` change **`mzvpmvzo`**, commit
  **`684842b9`**.
- Candidate change: **`zkwnlurq`**, initially observed at mutable working
  commit **`a94f0ca3`**; subsequent snapshots change that commit ID.
- Outcome: investigate whether the existing typed method-call operation
  can use pinned CPython's actual immediate-call lookup protocol for proven
  zero-argument calls, avoiding unnecessary bound-method wrapper creation
  while preserving every descriptor, ownership, mutation, and evaluation
  guarantee. A genuine unchanged-production regression now proves the
  current visible-wrapper difference from pinned stock CPython.

## Hypothesis and evidence

- General-purpose opportunity: Python workloads frequently evaluate an
  attribute and immediately invoke it as a method. Pinned stock CPython's
  existing **`_PyObject_GetMethod`** distinguishes a method-descriptor call
  from an ordinary attribute lookup and can invoke the method without
  materializing a separate GC-tracked bound-method wrapper. Current SOAC
  generic attribute lowering instead constructs that wrapper before its
  immediate zero-argument call.
- **Confirmed user-visible CPython compatibility difference:** genuine
  unchanged-production transformed regression
  **`tests/test_immediate_method_call_dispatch.py`** fails **1 / 3.86
  seconds** solely because pinned stock immediate inherited
  **`Child.observe()`** reports **no visible bound `MethodType`
  (`False`)**, while transformed Profile execution sees an actual
  GC-referrer bound-method wrapper **(`True`)**. Real
  **Profile → Verify → Apply** execution, hot counters, and native-body
  evidence accompany the failing stock-parity assertion; this is an actual
  existing runtime difference, not merely a source-level hypothesis.
- Every other stock/transformed compatibility control in that genuine RED
  passes: intentionally stored bound methods remain visible; `super`
  returns its correct value; dynamic MRO mutation, instance shadowing,
  data descriptors, properties, custom attribute hooks, static/class
  methods, missing methods, raising descriptors/methods, receiver
  finalization, and exceptional ownership cleanup all match. Actual
  `call_hot_targets` records at least **32** observations, and native code
  includes **`Base.observe`**, immediate/stored paths, and hot callers.
- An earlier host-interpreter assumption that `super` must expose a bound
  wrapper is **refuted by pinned CPython 3.15**: pinned stock immediate
  `super` also avoids the wrapper. The focused regression therefore checks
  `super`'s value only, and candidate source-shape recovery must exclude
  `super` paths rather than changing their existing semantics.
- Current lossless richards profile attributes a **7.593%** whole-workload
  union to bound-method wrapper creation plus destruction; deltablue
  attributes **6.158%**. This union includes method sites outside the
  proposed narrow zero-argument recovery and does not predict an equal
  throughput gain.
- Source census finds **18 / 37** richards method-call sites and
  **47 / 100** deltablue method-call sites have zero explicit arguments.
  Direct richards zero-argument wrapper creation has a source-backed
  approximate **2.278-percentage-point floor**; creation plus matching
  destruction might suggest roughly **4.6 points gross**, but dynamic
  frequency, guards, alternate descriptors, surviving lookup work, and
  sample overlap prevent a reliable prediction. Do not promise a target
  gain above **5%**.
- Generic method-lookup ancestry is **19.238%** inclusive in richards and
  **21.740%** inclusive in deltablue. Wrapper allocation is only a subset
  of that ancestry; parent and child shares overlap and must not be added.
  An older, different-revision chaos profile suggests approximately
  **1.015%** wrapper work and is only historical context, not matched
  current-baseline evidence.
- The immediate retained normally sampled fixed-eight baseline is
  comparison **150451**, with stock score **0.6249286764762751x** and
  generated coverage **23,188,640 native bytes / 1,527,950 machine
  blocks / 2,866 typed blocks / 204 functions**. Immediate retained
  three-round targeted comparison **150805** has stock score
  **0.4619152255075415x**, with per-round **18,255,240 native bytes /
  1,201,600 machine blocks / 2,265 typed blocks / 183 functions**.
  Corresponding retained release smoke is **150319**. The authoritative
  full-suite stock **1.10x** objective remains unmet; the full suite itself
  has not been measured for this strategy.

## Implementation and compatibility

- Approved production scope is exactly three existing files:
  `crates/soac_jit/src/jit/typed_pipeline.rs`,
  `crates/soac_jit/src/jit/imports.rs`, and
  `crates/soac_jit/src/jit/mod.rs`. All three candidate production files
  are **FROZEN**, including the existing pinned CPython import and
  mechanical final JIT emission; both focused actual regressions now pass.
- Before typed linearization, record a private, source-grounded sidecar
  connecting original attribute-access and immediate-call instruction IDs.
  At the final typed stage, recover only a proven **zero-explicit-argument
  call** in the same block with a unique temporary definition, one call
  use, and its corresponding deletion. Reject escaped, reused, aliased,
  cross-block, multi-use, keyword, starred, `super`, or otherwise
  uncertain values;
  preserve all prior scalar, owner, direct-call, generator, and other typed
  optimizations.
- Represent the admitted decision with the **existing typed
  `GuardedMethodCall` operation and an empty guard list**, not a new public
  enum/IR concept, runtime patch, or benchmark-specific recognizer. JIT
  emission mechanically consumes that existing validated shape and imports
  the already-exported pinned CPython **`_PyObject_GetMethod`** symbol.
- Let pinned CPython implement method-descriptor lookup and all observable
  custom attribute behavior: data/non-data descriptors, custom
  `__getattribute__` / `__getattr__`, dynamic class/MRO changes, instance
  shadowing, `staticmethod`, `classmethod`, properties, replacement
  methods, missing attributes, and exceptions. Guard or reject any shape
  whose original lookup/call behavior cannot be preserved.
- Evaluate the receiver exactly once, keep the correct owned callable and
  receiver references in both method and non-method cases, preserve
  callable evaluation order / error state / cleanup, and avoid introducing
  a visible temporary wrapper. Standalone bound-method lookup or storage
  must remain unchanged; subclasses, mutation/reentry, tracing, profiling,
  monitoring, finalizers, and GC behavior require actual regression
  evidence.
- Retain the existing generic attribute-then-call path on an unsupported
  source shape, uncertain ownership, descriptor error, non-immediate use,
  nonzero arguments, keyword arguments, or incompatible method protocol.
  Add no public API, runtime helper, global mutable state, or public IR
  operation. Candidate emitted-body/native growth is unknown and must be
  measured, not assumed invariant.
- Focused unchanged-production transformed CPython-parity regression:
  genuine **RED, 1 failed in 3.86 seconds**, with all unrelated
  descriptor/ownership/native-profile controls passing.
- Independent genuine unchanged-production whole-production structured
  Rust **RED** in approved `typed_pipeline.rs`:
  **`immediate_zero_arg_method_calls_retain_resolved_dispatch_after_typed_rewrites`**
  exercises actual source lowering, real field/call instrumentation
  counters, and complete
  **`optimize_blockpy_with_external_inline_callees`** across
  **Profile / Verify / Apply**. The first Profile immediate call contains
  **zero** existing typed method-dispatch nodes instead of required
  **one**; the focused test reports **1 failed / 571 filtered**.
  Standalone temporary / escaped method, nonzero-argument, keyword, and
  `super` controls pass on that unchanged-production RED.
- The same full-production typed-pipeline regression now independently
  verifies genuine **RED → GREEN: 1 passed / 571 filtered** across real
  **Profile / Verify / Apply**. A private prelinear source/getattr/call
  sidecar and final validated recovery produce the existing empty-guard
  typed method-dispatch operation for proven immediate/single-use
  temporary cases, exclude aliases/nonzero arguments/keywords/`super`, and
  preserve the original field and call instrumentation-counter IDs.
  Observable standalone stored bound methods remain generic.
- The unchanged frozen actual transformed stock-parity regression now
  independently verifies genuine **RED → GREEN: 1 passed in 3.47
  seconds**. Real **Profile → Verify → Apply** observes the same absence
  of an immediate bound-method wrapper as pinned stock CPython while
  retaining stored wrappers, correct `super` values, inherited dispatch,
  dynamic MRO/instance-shadow mutation, descriptors/properties/custom
  attribute hooks, static/class methods, missing/raising cases,
  finalizer/error ownership, original getter/call counters, and real native
  body evidence. The implementation imports existing
  **`_PyObject_GetMethod`** and reuses the existing empty-guard typed
  operation; it adds no public API, runtime helper, or global.
- Package-scoped Rust formatting now completes, and the fresh complete JIT
  Rust library genuinely passes **572 / 572**, including the new
  production-path typed recovery regression. Independent host source review
  of all three approved files verifies the existing pinned CPython method
  resolver ABI, null-output/error propagation, original field/getter and
  call counter identities, receiver ownership, and failure cleanup.
- Complete affected optimizer Rust suite passes **213 / 213** and typed-IR
  Rust suite passes **54 / 54**. Grouped actual transformed compatibility
  tests pass **15 / 15**, including preserved real getter/call counter
  behavior. Scoped `just fmt-rust soac_jit`,
  `just fmt-rust-check soac_jit`, and
  `cargo check -p soac_jit --tests` all pass.
- Mode-matched release debug-single smoke **155015** against retained
  **150319** completes **8 / 8**, with **zero ERROR / CRITICAL events**.
  Every measured Apply PID preserves the same transformed modules,
  source-function/adapter identities, and optimized typed coverage
  **2,866 blocks / 204 functions**; pre-optimization BlockPy remains
  **7,199,376 bytes** and hidden exact trampolines remain **36,500 bytes**.
- Ordinary smoke native code falls **2,242,168 → 2,238,276 bytes
  (-3,892)** and machine blocks **148,116 → 147,862 (-254)**. Per-workload
  byte changes are chaos **-3,324**, comprehensions **-372**, deltablue
  **+928**, nbody **+104**, richards **-844**, spectral norm **-384**, and
  fannkuch / float **0**. Cold debug-single timings and arithmetic are
  **invalid throughput evidence**; code-size changes do not establish
  benchmark speedup.
- Normally sampled fixed-eight comparison **155216** completes all **80**
  measured Apply workers with exactly unchanged source-function/module
  coverage, **2,866 typed blocks / 204 functions**, **365,000 hidden
  trampoline bytes**, and **zero errors**. Ordinary native code improves
  **23,188,640 → 23,158,320 bytes (-30,320)**; machine blocks improve
  **1,527,950 → 1,525,740 (-2,210)**. These structural/coverage
  measurements are valid independently of worker wall-time contention.
- **All `155216` performance results are INVALID and discarded for
  inference or retention** because unrelated broad execution contention
  distorts even byte-identical controls. Unchanged float workers report
  approximately **94 / 95 / 137 / 155 ms** instead of approximately
  **40 ms**; chaos reaches **70.7 / 59.9 ms** versus approximately
  **43 ms**, deltablue **15.25 ms** versus approximately **2.7 ms**,
  nbody **138 ms** versus approximately **61 ms**, and richards
  **44.7 ms** versus approximately **27 ms**. Official stock score
  approximately **0.532048x** and previous-SOAC ratio approximately
  **0.853392x** are meaningless and are not performance headlines.
- Exploratory contaminated delta **1.06834x [1.04722, 1.09894]** /
  paired **1.07215x**, and richards **1.01742x**, are also explicitly
  **INVALID / unreliable** despite apparently favorable statistics; those
  contaminated normal results remain permanently excluded from inference.
- Independently clean zero-argument three-round comparison **155732**
  against retained **150805** confirms deltablue
  **2.928596 → 2.716862 ms**, **1.077933x [1.057344, 1.095900]** /
  paired-stock **1.109685x [1.071159, 1.143742]**, with raw rounds
  **1.0753x / 1.1012x / 1.0522x** and no delta worker outliers. Richards
  is **1.010292x** with its interval crossing neutral; paired
  **1.035240x** has an interval above one, but **5 / 30** severe
  host-contention outliers make that richards result unreliable rather
  than a clean headline. Chaos and comprehensions controls are neutral.
- Clean repeated four-workload robust geometry is **1.021368x** /
  paired-stock **1.038450x**; official targeted stock score is
  **0.4762066426894141x** and previous-SOAC arithmetic
  **1.0121414134188496x**. All **120** measured Apply workers retain the
  same source-function IDs and **746,520** hidden trampoline bytes, with
  zero errors; aggregate ordinary native bytes decrease
  **54,765,720 → 54,683,160 (-82,560)** and machine blocks decrease
  **3,604,800 → 3,598,140 (-6,660)**.
- Same zero-argument candidate's **lossless 757-sample** deltablue profile
  retains Python bound-method creation **1.189%** plus deallocation
  **1.585%**, a disjoint **2.774%** residual wrapper union; separate
  builtin-wrapper work is approximately **0.396%**. Its lossless
  **269-sample richards** retry retains creation **2.232%** plus
  destruction **4.460%**, or **6.692%** residual, with a material
  small-sample caveat. The initial **374-sample** richards capture lost
  **one sample chunk**, was correctly rejected, and is never performance
  evidence; the retry at **99 Hz** records zero loss.
- Candidate's lossless **292-sample comprehensions** profile retains
  builtin-method creation **1.713%** plus **`meth_dealloc` 2.054%**, or
  approximately **3.766%** combined after source rounding. The observed
  parent is captured-cell **`id_to_widget.get(dwid)`**, not an interpreted
  Python helper. These are current zero-argument implementation residuals
  and an investigation baseline, not proof an extension will eliminate
  them or improve throughput.
- First atomic positional implementation is **SAVED** inside this **same
  strategy** and touches only **two** of its already-approved production
  files, `jit/typed_pipeline.rs` and `jit/mod.rs`; package formatting is
  completed first and the existing CPython import is reused unchanged.
  Admit at most **two simple `Load` positional
  arguments**, with receivers proven local, closure-cell, or preserved;
  exclude global/class/super receivers, aliases, keywords, starred args,
  and effectful argument expressions. Preserve exact same-block
  **method lookup before argument evaluation**, original counters, and
  exception cleanup in reverse order **arguments → receiver → method
  descriptor**.
- The existing focused
  **`tests/test_immediate_method_call_dispatch.py`** now independently
  proves a second genuine unchanged-zero-argument-candidate CPython
  correctness **RED: 1 failed in 4.48 seconds**. Its sole final mismatch is
  pinned stock **`{'python_positional': False, 'python_two_positional':
  False, 'builtin_positional': False, 'captured_builtin_positional':
  False}`** versus transformed **all four `True`**: unnecessary visible
  bound wrappers remain for ordinary one-/two-argument Python calls,
  builtin methods, and captured builtin receivers.
- Before that final positional parity failure, real
  **Profile → Verify → Apply** passes all zero-argument and stored-wrapper
  controls, descriptor lookup **before** `UnboundLocalError`, argument
  evaluation order, keyword/starred/`super` fallback, owned receiver
  finalizer behavior, MRO/shadow/custom-descriptor semantics, original
  counters, and emitted native coverage. The valid existing zero-argument
  **1.077933x** repeated delta result and source profiles remain evidence
  for that earlier iteration only.
- A second independent same-strategy whole-production positional Rust
  **RED** expands the existing
  **`immediate_zero_arg_method_calls_retain_resolved_dispatch_after_typed_rewrites`**
  regression. Real lowered **Profile / Verify / Apply** cases cover
  one/two positional arguments, builtin **`dict.get`**, captured **`Cell`**
  receivers, and negative above-two/effectful/global/class/keyword/star /
  `super`/alias controls. The first actual Profile positional call has
  **0** existing typed method-dispatch nodes instead of required **1**;
  the focused test reports **1 failed / 571 filtered**, with all prior
  zero-argument controls passing. Its single serial compile unexpectedly
  takes approximately **60 seconds** as workflow overhead, not throughput
  or a candidate benchmark.
- Independent complete host source review of the saved, package-formatted
  two-file positional extension is **GREEN**: eligibility is capped at
  **two `Load` arguments** with only `Local` / `Cell` / `Preserved`
  receivers and preserved original source/counter IDs; global, class,
  `super`, keyword, starred, effectful, and aliased cases stay generic.
  Existing CPython method lookup precedes argument evaluation; the
  ordinary-descriptor path releases its owned receiver before loading
  arguments. On each argument-prefix failure, cleanup runs in exact
  reverse order **previous owned arguments → conditionally owned self →
  owned callable**, forwarding the original pending exception.
- The expanded actual whole-production typed-pipeline structured
  regression now verifies positional **RED → GREEN: 1 passed / 571
  filtered** across **Profile / Verify / Apply**. Real source lowering
  selects proven one-/two-argument Python calls, builtin **`dict.get`**,
  and captured `Cell` receivers; aliases, above-cap/effectful/global /
  class/keyword/star/`super` paths remain unchanged.
- The frozen real transformed four-way positional stock-parity regression
  independently verifies **RED → GREEN: 1 passed in 4.82 seconds**. All
  four previously incorrect visible Python/builtin positional wrappers now
  match pinned stock across **Profile → Verify → Apply**, while every
  prior zero-argument/stored-wrapper, descriptor-before-unbound-error,
  evaluation-order, kwargs/star/`super`, MRO/shadow/property/custom-hook,
  missing/error/finalizer, original-counter, and native-body control
  remains correct.
- The earlier complete JIT **572**, optimizer **213**, typed IR **54**,
  grouped transformed matrix, scoped formatting/checks, release smoke,
  and repeated delta measurement apply to the **previous zero-argument
  iteration**.
- The first complete **post-positional JIT suite is RED** for one actual
  root cause: existing embedded Rust regression
  **`synthetic_code_caches_the_exact_indexed_runtime_module_attribute_guard`**
  at **`crates/soac_jit/src/function_instantiation.rs:2769`** assumes a
  GENERAL-dictionary collision observer sees exactly **`[false, false]`**,
  while pinned CPython's legal randomized hash/open-address probing
  compares **five** times, all **`false`**. The first assertion panic
  poisons the shared Python test mutex, creating approximately **111
  secondary failures**; these are not separate method-dispatch defects.
- Root's narrow existing **`#[cfg(test)]`-only** correction in that fourth
  Rust path is now **SAVED, package-formatted, and independently source
  reviewed**. It extracts collision identities and requires **at least two
  observations, all `false`**, preserving fresh-name identity plus every
  existing success / raising-error check; earlier robust GENERAL-dictionary
  and dict-subclass sibling assertions remain intact. The exact previously
  failing isolated test now **PASSES**. This fourth Rust file is
  test-only, not a fourth production path; runtime optimization remains
  exactly the three previously approved production files.
- After that durable test-only repair, a fresh complete **post-positional
  JIT suite genuinely passes 572 / 572**; fresh optimizer **213 / 213** and
  typed IR **54 / 54** suites also pass. Grouped actual transformed
  compatibility passes **15 / 15**. Scoped
  `just fmt-rust soac_jit`, `just fmt-rust-check soac_jit`, and
  `cargo check -p soac_jit --tests` all pass. Exactly three runtime
  production files are now **FROZEN**; the additional existing
  `function_instantiation.rs` change is strictly **`#[cfg(test)]`-only**.
- Post-positional release debug-single comparison **162613** completes
  **8 / 8**, preserving all **397** measured Apply source-function /
  adapter identities, module coverage, **36,500** hidden trampoline
  bytes, and zero errors. Ordinary native coverage changes from retained
  **2,242,168 bytes / 148,116 blocks**, to zero-argument-only
  **2,238,276 / 147,862**, to final positional
  **2,239,376 / 147,928**. The extension adds **1,100 bytes / 66 blocks**
  over zero-argument dispatch but remains **2,792 bytes / 188 blocks**
  below retained production.
- Zero-argument → positional per-workload smoke changes are chaos
  **+128 bytes / +8 blocks**, comprehensions **+140 / +10**, deltablue
  **+1,044 / +67**, richards **-212 / -19**, and all other workloads
  unchanged. Cold smoke timing remains **INVALID** as throughput evidence.
- Critical negative coverage finding: the motivating hot comprehensions
  captured-cell **`id_to_widget.get(dwid)`** list-comprehension body
  remains exactly **12,552 bytes / 816 blocks across all three
  revisions**; its surrounding dictionary comprehension also remains
  exactly **14,968 bytes / 991 blocks**. Only cold
  **`WidgetTray.__init__`** grows **1,068 → 1,208 bytes**. Therefore the
  synthetic captured-`Cell` regression does **not** prove selection of the
  hot benchmark call site; no comprehensions wrapper elimination or speed
  improvement may be claimed.
- Normally sampled positional comparison **162850** against clean
  retained **150451** shows a real robust comprehensions **REGRESSION**:
  **43.628 → 46.582 us**, ratio **0.93660x [0.91529, 0.95561]** /
  paired-stock **0.94364x [0.91652, 0.97374]**. Both intervals exclude
  neutral, and there are **no comprehensions outliers**; do not dismiss
  or invert this approximately **6.3%** target slowdown.
- Actual hot captured-cell benchmark function **`1:31`** changes its
  **Profile-mode** native body **9,608 → 9,968 bytes**, demonstrating
  initial source selection, but its **Apply-mode** body remains exactly
  **12,552 bytes / 816 blocks**. The actual source is nested
  **`[id_to_widget.get(dwid) for dwid in ...]`**.
  `clone_typed_hot_continuation` creates **11** Apply blocks that retain
  the same compiler-generated `ResolvedName` but remap each original
  getter/call **`InstrId`**. The existing private source-pair sidecar is
  not remapped; final recovery's global temporary-use counts and edge
  restrictions reject both the original and cloned pairs. Thus initial
  source selection is genuinely lost in profile-driven rewrites before
  final Apply emission.
- The existing synthetic captured-`Cell` regression tests only a simple
  return and does **not** exercise the real cloned nested comprehension;
  it cannot establish hot benchmark recovery. Root halted repeated
  measurements pending an actual nested transformed **Apply** regression
  and bounded source-grounded refinement.
- Deltablue separately improves **2.894 → 2.662 ms**,
  **1.08725x [1.04774, 1.12140]** / paired **1.08739x**. Richards
  **27.966 → 26.723 ms** is **1.04650x** with raw interval above neutral,
  but its paired interval crosses one; chaos is **1.02258x**. Severe
  unrelated worker outliers on fannkuch (**3.81x**) and spectral norm
  (**1.4x**) make the official previous geometric approximately
  **0.96786x** unreliable for suite-wide inference. No retain decision or
  comprehensions benefit is established.
- Independent audit confirms all **80** positional normal Apply workers
  retain exactly the same source-function IDs and **365,000** hidden
  trampoline bytes, with zero errors; ordinary native bytes decrease
  **23,188,640 → 23,170,000**. This valid coverage evidence does not
  mitigate the actual **0.93660x** comprehensions regression.
- The same-strategy clone-aware refinement is now **SAVED in exactly one
  existing production file**,
  **`crates/soac_jit/src/jit/typed_pipeline.rs`**, and independently
  host-audited **GREEN**. Groups use the exact full compiler-private
  `ResolvedName`, reject any CFG `BlockArg` transport, and require every
  global store/load/delete for that temporary to belong to an isolated,
  adjacent same-block getter/call pair. Exactly **one** pair must retain
  the original source getter **and** call IDs; every clone must match its
  receiver, constant attribute, ordered simple arguments, and `Generic`
  access. Each validated group is recovered all-or-none, removing only
  its proven per-block store/delete; aliases and prior receiver,
  ownership, evaluation-order, `super`, keyword, and argument-count
  protections remain excluded. Final structured and transformed
  correctness checks and fresh complete affected suites are green;
  benchmark-path recovery has not yet been measured.
- The existing frozen transformed integration now establishes a genuine
  same-strategy nested **Apply-only CPython correctness RED: 1 failed in
  5.04 seconds**. Actual source
  **`[mapping.get(value) for value in (key,)][0]`** uses a captured
  closure `Cell` and **`ProbeKey.__hash__`**: pinned stock observes no bound
  method wrapper (**`False`**), transformed **Profile also correctly
  reports `False`**, but transformed **Apply incorrectly reports `True`**.
  This proves the source-pair continuation-clone gap on a real nested
  compiled body, not merely a hypothetical benchmark explanation.
- Before that single intended final failure, actual
  **Profile → Verify → Apply** runs pass all existing zero-argument and
  four positional parity checks, stored wrappers, descriptor/MRO/shadow
  behavior, errors/finalizers, original counters, and emitted nested
  direct-function body evidence.
- A second independent genuine whole-production structured Rust **RED**
  expands existing
  **`immediate_zero_arg_method_calls_retain_resolved_dispatch_after_typed_rewrites`**
  with actual lowered nested source
  **`[mapping.get(value) for value in (key,)][0]`**. **Profile** passes
  every existing target, but the first **Verify** nested function
  **`captured_comprehension.<locals>._dp_listcomp_3`** has **0** resolved
  method-dispatch nodes instead of the required **1**. The focused run
  genuinely reports **1 failed / 571 filtered**. This structured
  continuation-clone failure independently matches the transformed
  Apply-only stock-parity **1 failed / 5.04 seconds**.
- The first candidate structured Verify run correctly resolved **two**
  nodes—the anchored original and its continuation clone—but initially
  failed because the old fixture expected **one**. Once that stale
  production-shaped oracle was corrected, the genuine whole-production
  structured regression turned **GREEN**, proving exactly **Profile 1 /
  Verify 2 / Apply 2** selected method nodes.
- The frozen real transformed nested stock-parity integration also turned
  **GREEN: 1 passed / 7.78 seconds**. Pinned stock and transformed
  Profile / Verify / Apply now all preserve captured-cell nested-listcomp
  bound-method visibility, with every previous zero/one/two-argument,
  descriptor/MRO/shadow, evaluation-order, finalizer, counter, and native
  coverage control still passing.
- Fresh **post-clone** complete Rust JIT **572 / 572**, optimizer
  **213 / 213**, and typed IR **54 / 54** suites, broad transformed
  compatibility **15 / 15**, package-scoped formatting check, and Cargo
  `--tests` check are all genuinely **GREEN**.
- Decisive final post-clone release debug-single smoke **165908** passes
  **8 / 8**, with every **397** actual measured Apply source-function row
  and module coverage preserved, no errors, identical typed counts
  **2,866 / 204**, and unchanged **36,500 bytes** of hidden trampolines.
  Crucially, the real hot nested comprehensions function **`1:31` /
  `_dp_listcomp_20`** finally changes in **Apply** from **12,552 bytes /
  816 blocks** to **12,644 bytes / 823 blocks**; retained, zero-argument,
  and pre-clone positional revisions all remained at **12,552 / 816**.
  This proves the previously missed actual benchmark source is selected,
  but does not establish throughput improvement.
- Aggregate native lineage is retained **2,242,168 bytes / 148,116
  blocks**, zero-argument **2,238,276 / 147,862**, pre-clone positional
  **2,239,376 / 147,928**, and final post-clone **2,238,468 / 147,712**:
  **908 fewer bytes / 216 fewer blocks** than pre-clone and **3,700 fewer
  bytes / 404 fewer blocks** than retained. Cold smoke timings are
  **INVALID** throughput evidence.
- Completed post-clone normal comparison **170030** versus clean retained
  **150451** still shows a real adverse comprehensions result:
  **43.628 → 46.737 µs**, raw **0.93349x [0.88504, 0.96516]** and
  stock-adjusted **0.96113x [0.90671, 0.99558]**, including one **1.98x**
  worker outlier. Against pre-clone positional comparison **162850**, the
  same workload is **0.99668x** raw / **1.01854x** stock-adjusted, with
  both intervals crossing neutral. Actual hot Apply selection is proven,
  but there is **no credible one-round comprehensions speedup**.
- Deltablue is **1.07568x** raw / **1.10340x** stock-adjusted, but two
  outliers make both confidence intervals cross neutral. Richards is
  **1.01019x** raw, also neutral, and **1.05898x** stock-adjusted barely
  exceeds one amid stock drift. Widespread host contamination makes the
  official previous geometric approximately **0.968x INVALID** for
  whole-suite inference; these results do not support retention.
- Independent audit confirms all **80** measured normal Apply workers
  retain identical source-function / PID coverage, **365,000** hidden
  trampoline bytes, and zero errors. Ordinary native code changes from
  retained **23,188,640 bytes / 1,527,950 blocks** to final
  **23,163,480 / 1,524,480**, a decrease of **25,160 bytes / 3,470
  blocks**; against pre-clone **23,170,000 / 1,526,540**, the reduction
  is **6,520 bytes / 2,060 blocks**. All **10** comprehensions workers
  confirm hot function **`1:31`** changes **12,552 / 816 → 12,644 / 823**.
- The final clean three-round targeted comparison **170351** versus
  retained **150805** shows comprehensions **45.001 → 45.076 µs**,
  **0.998325x [0.987093, 1.019031]**: clearly **NEUTRAL**, despite actual
  nested hot-site selection. Its paired-stock **1.081145x** coincides
  with an **8.30% slower stock control** and must **not** be attributed
  to the optimization or presented as a comprehensions improvement.
- Deltablue improves **2.92860 → 2.61845 ms**,
  **1.118447x [1.09207, 1.13869]** / paired **1.15754x**. Compared
  independently against the already-improved zero-argument candidate
  **155732**, the final positional / clone-aware implementation still
  improves **1.037585x [1.01464, 1.05759]** / paired **1.05365x**.
  Richards is **1.019530x [1.00573, 1.03815]**, with its paired result
  subject to stock drift. Chaos is **0.98202x** raw but **1.01114x**
  paired with an interval crossing neutral because stock itself drifts
  **2.96%**; treat chaos as neutral after adjustment.
- The official clean targeted four-workload stock score is exactly
  **0.49747399350945193x**, and official previous-SOAC score is
  **1.0194276621869476x**. The fixed-eight normal **170030** reports
  stock **0.6273571181431998x** / previous **0.9683515036210124x**, but
  host-load **7.37 / 9.21 / 10.42**, worker outliers, and control drift
  make that normal aggregate unreliable; it does not prove an actual
  suite-wide regression or benefit.
- All **120** final targeted Apply workers retain exactly **10,650**
  source-function rows, unchanged **746,520 bytes** of hidden
  trampolines, and zero errors. Every **30** comprehensions workers
  confirms the actual hot nested body changes **12,552 bytes / 816
  blocks → 12,644 / 823**. Three-round ordinary native lineage is
  retained **54,765,720 bytes / 3,604,800 blocks**, zero-argument
  **54,683,160 / 3,598,140**, and final positional / clone-aware
  **54,697,320 / 3,594,960**.
- Matched zero-loss zero-argument → final profiles: comprehensions
  **292 → 292**, richards **269 → 280**, deltablue **757 at 199 Hz →
  246 at 99 Hz**; the frequency change limits deltablue precision. The
  earlier richards **374-sample** run lost one chunk and was rejected.
  Comprehensions' disjoint builtin-wrapper union declines
  **3.7663% → 2.7402%** (creation **1.7125% → 1.0278%**, destruction
  **2.0538% → 1.7124%**); hot `_dp_listcomp_20` creation falls
  **1.3702% → 0.6856%**. New `_PyObject_GetMethod` is **3.0825%**
  inclusive / **1.0278%** self, consistent with neutral throughput;
  overlapping inclusive samples must not be added. Deltablue wrapper
  union falls **3.1698% → 0.4067%**, with `method_dealloc`
  **1.5849% → 0**. Small richards samples show wrapper union
  **6.6917% → 6.7855%**, not elimination, while overlapping generic
  `GetAttr` ancestry falls **18.2185% → 11.7846%**.
- **LANDED CANDIDATE / RETAIN** is supported by multiple genuine stock-CPython-visible
  correctness fixes, significant repeated deltablue improvement including
  incremental gain over zero-argument dispatch, neutral repeated
  comprehensions, and reduced ordinary native code. Final lossless profiles
  are recorded, and the authoritative full **`just test-all` gate exits
  zero**; see **`work/logs/immediate-method-call-dispatch-test-all.log`**.
  It passes **1,232 Python nodeids / 95 isolated batches / 8 workers /
  0 failures**, including the actual transformed method-parity regression
  in **6.03 seconds**. Rust suites pass JIT **572**, optimizer **213**,
  lowering **371**, typed IR **54**, and PyO3 **8**. Runtime build takes
  **1.951 seconds**, Cargo **89.061 seconds**, pytest **80.428 seconds
  inner / 80.445 seconds outer**, and total test phase **169.519
  seconds**; the known **28-node** counter batch takes **80.52 seconds**.
  Production remains exactly three runtime paths plus one existing
  **`#[cfg(test)]`-only** collision-probe assertion repair. Earlier
  zero-argument repeated deltablue **1.077933x** and discarded
  contention-corrupted fixed-eight **155216** remain historical results,
  not post-refinement measurements.

## Benchmark protocol and coverage

- Fixed benchmark selection: current eight-workload set **chaos,
  comprehensions, deltablue, fannkuch, float, nbody, richards, and spectral
  norm**. Use the existing repeated four-workload set for richards /
  deltablue targets and chaos / comprehensions guardrails.
- Comparison protocol: mode-matched debug-single release smoke followed by
  normally sampled fixed-eight comparison and independently ordered
  **three-round** targeted comparison; report worker-clustered confidence
  intervals and paired-stock drift. Final candidate runs: smoke **165908**,
  normal **170030** with host-contention caveat, and clean targeted
  **170351**.
- Retained baseline artifacts: smoke comparison **150319**, normal
  **150451**, and repeated targeted **150805**. Final clean targeted
  stock / previous scores are **0.49747399350945193x /
  1.0194276621869476x**; fixed-eight **0.6273571181431998x /
  0.9683515036210124x** is contaminated by host outliers.
- Benchmark module/dependency/standard-library transformation and actual
  source call-site coverage: unchanged allow-list, worker manifests,
  source counters, and JIT summaries verify all **80 / 120** measured
  workers and actual nested function **`1:31`** Apply recovery in every
  comprehensions worker; source-function coverage and hidden trampolines
  remain unchanged.
- Lossless zero-argument → final positional profiles contain deltablue
  **757 → 246**, richards **269 → 280**, and comprehensions **292 → 292**
  samples; deltablue's **199 Hz → 99 Hz** mismatch limits precision. An
  initial **374-sample** richards capture with one lost chunk was
  discarded. The older chaos profile is not a matched causal comparison.
- Retained compiled/native coverage: normal **2,866 typed blocks / 204
  functions / 23,188,640 native bytes / 1,527,950 machine blocks**;
  targeted per round **2,265 typed blocks / 183 functions / 18,255,240
  native bytes / 1,201,600 machine blocks**. Final candidate normal is
  **23,163,480 bytes / 1,524,480 blocks**; targeted three-round aggregate
  **54,697,320 bytes / 3,594,960 blocks**, with all source rows / hidden
  trampolines unchanged and zero errors.

## Measurements

| Metric | Retained baseline | Candidate | Change |
| --- | --- | --- | --- |
| Fixed-eight stock / SOAC geometric score | 0.6249286764762751x | 0.6273571181431998x; previous 0.9683515036210124x | official previous contaminated by host outliers; no whole-suite claim |
| Targeted three-round stock / SOAC geometric score | 0.4619152255075415x | 0.49747399350945193x | clean three-round final candidate |
| Targeted previous SOAC / candidate SOAC | n/a | 1.0194276621869476x official | significant deltablue; comprehensions neutral |
| Repeated deltablue vs retained SOAC | 2.92860 ms | 2.61845 ms | 1.118447x [1.09207, 1.13869]; paired 1.15754x |
| Repeated deltablue vs zero-argument candidate | zero-argument comparison 155732 | final clone-aware comparison 170351 | incremental 1.037585x [1.01464, 1.05759] |
| Repeated comprehensions vs retained SOAC | 45.001 µs | 45.076 µs | 0.998325x [0.987093, 1.019031], neutral; paired 1.081145x reflects 8.30% stock drift |
| Fixed-eight optimized typed coverage | 2,866 blocks / 204 functions | 2,866 blocks / 204 functions | unchanged |
| Targeted optimized typed coverage per round | 2,265 blocks / 183 functions | 2,265 blocks / 183 functions | unchanged |
| Fixed-eight Apply native bytes / machine blocks | 23,188,640 bytes / 1,527,950 blocks | 23,163,480 bytes / 1,524,480 blocks | -25,160 bytes / -3,470 blocks |
| Post-positional fixed-eight Apply native bytes | 23,188,640 bytes | 23,170,000 bytes across 80 workers | -18,640 bytes; hot comprehensions still unspecialized |
| Targeted Apply native bytes / machine blocks across three rounds | 54,765,720 bytes / 3,604,800 blocks | 54,697,320 bytes / 3,594,960 blocks | -68,400 bytes / -9,840 blocks |
| Serialized pre-optimization BlockPy bytes, release smoke | 7,199,376 | 7,199,376 | unchanged |
| Mode-matched final clone-aware smoke native bytes / machine blocks | 2,242,168 bytes / 148,116 blocks | 2,238,468 bytes / 147,712 blocks | -3,700 bytes / -404 blocks |
| Post-positional mode-matched smoke native bytes / machine blocks | 2,238,276 bytes / 147,862 blocks, zero-argument candidate | 2,239,376 bytes / 147,928 blocks | +1,100 bytes / +66 blocks versus zero-argument; -2,792 bytes / -188 blocks versus retained |
| Mode-matched smoke hidden exact trampoline bytes | 36,500 | 36,500 | unchanged |
| Lossless richards wrapper create + destroy union | 7.593% | 6.692% residual / 269 samples | small sample; no extension gain implied |
| Lossless deltablue wrapper create + destroy union | 6.158% | 2.774% residual / 757 samples | zero-argument candidate; extension unmeasured |
| Lossless comprehensions builtin wrapper create + destroy | not independently refreshed | approximately 3.766% / 292 samples | captured-cell dict.get; extension unmeasured |
| Richards zero-argument creation floor | approximately 2.278% | pending | static/source-bounded |
| Static zero-argument method sites | richards 18 / 37; deltablue 47 / 100 | pending | dynamic eligibility unknown |
| Genuine transformed stock-parity regression | 1 failed / 3.86 s; stock wrapper False, SOAC Profile wrapper True | 1 passed / 3.47 s across Profile / Verify / Apply | genuine CPython correctness RED-to-GREEN |
| Genuine same-strategy positional stock-parity regression | zero-argument candidate 1 failed / 4.48 s; stock four False versus SOAC four True | 1 passed / 4.82 s across Profile / Verify / Apply | positional CPython correctness RED-to-GREEN |
| Genuine same-strategy positional structured regression | 1 failed / 571 filtered; actual Profile dispatch nodes 0 versus 1 | 1 passed / 571 filtered across Profile / Verify / Apply | whole-production positional typed-pipeline RED-to-GREEN |
| Genuine structured method-plan regression | 1 failed / 571 filtered; actual Profile dispatch nodes 0 versus 1 | 1 passed / 571 filtered across Profile / Verify / Apply | genuine whole-production typed-pipeline RED-to-GREEN |
| Full `just test-all` correctness gate | retained baseline previously passed | 1,232 Python nodeids / 95 batches / 8 workers / 0 failures | GREEN; JIT 572, optimizer 213, lowering 371, typed 54, PyO3 8 |

## Attempt history

### Attempt 1: recover existing typed immediate zero-argument method calls

- Change: proposed private source call/getattr provenance sidecar,
  conservative final typed recovery, and mechanical emission of existing
  `GuardedMethodCall` through pinned `_PyObject_GetMethod`; exactly three
  approved existing production files. All three implementations are saved,
  with the existing pinned CPython import and no new API/helper/global.
- Measurements and coverage: retained normal stock
  **0.6249286764762751x**, targeted stock **0.4619152255075415x**, actual
  existing native/typed coverage above, and zero-loss wrapper allocations;
  all candidate evidence **PENDING**.
- Compatibility and tests: standalone bound methods, descriptors,
  attribute hooks, instance shadows, live class/MRO mutation, lookup/call
  evaluation order, owned references, exceptions, GC, and finalizers must
  match stock. The unchanged transformed stock-parity integration now
  genuinely **fails 1 / 3.86 seconds** solely on visible wrapper
  **`False` versus `True`**, while all descriptor/super/value/ownership /
  hot-counter/native controls pass. Independent full-production typed
  pipeline likewise genuinely **fails 1 / 571 filtered**, with actual
  source lowering/counters and zero existing dispatch nodes versus one,
  then passes **1 / 571 filtered** after private source-proven recovery.
  The same frozen transformed CPython correctness regression also turns
  **GREEN 1 / 3.47 seconds**, preserving all real descriptor, hook,
  finalizer, monitoring, pinned `super`, counter, and native controls.
- Result: **IN PROGRESS; genuine transformed CPython correctness AND
  independent whole-production structured RED-to-GREEN; full JIT
  572 / 572, optimizer 213 / 213, typed 54 / 54, grouped transformed
  15 / 15, package formatting/checks, and release smoke 8 / 8 GREEN;
  completed normal coverage / code-size valid but throughput INVALID;
  clean repeated deltablue 1.077933x; richards inconclusive/outlier-
  affected; same-strategy positional extension under consideration**.
- Reason: pinned stock avoids the actually observable immediate-call
  wrapper that SOAC currently creates; recovering the existing typed shape
  must preserve standalone wrappers and pinned `super` semantics. Measured
  benefit may still fail to justify code size or compatibility risk.

### Attempt 2: cautiously admit one or two simple positional loads

- Change: first saved and package-formatted continuation of the exact
  **same strategy**, limited to existing `jit/typed_pipeline.rs` and
  `jit/mod.rs`; reuse the existing method import, source-pair provenance,
  typed operation, and counters. Independent complete host source review
  reports no blocker.
- Eligibility: maximum **two** side-effect-free simple `Load` positional
  arguments; local / captured-cell / preserved receivers only; no global,
  class, `super`, alias, keyword, starred argument, or effectful argument.
  Preserve same-block lookup-before-argument ordering and reverse
  exceptional cleanup **arguments → receiver → descriptor**.
- Supporting measurements: valid zero-argument repeated deltablue
  **1.077933x** plus lossless candidate residual wrapper shares
  deltablue **2.774% / 757 samples**, richards **6.692% / 269 samples**,
  and comprehensions **3.766% / 292 samples**. Reject the initial lossy
  richards capture; none of these is a positional-extension benchmark.
- Compatibility and tests: genuine unchanged-production positional
  stock-wrapper **RED, 1 failed / 4.48 seconds**, compares pinned stock
  four `False` values against transformed four `True` values; all actual
  **Profile → Verify → Apply**, prior zero-argument, descriptor-before-
  error, evaluation-order, kwargs/star/`super`, ownership/finalizer,
  MRO/shadow, counter, and native controls pass. Independent expanded
  actual typed-pipeline regression also genuinely **fails 1 / 571
  filtered**, proving missing Profile positional dispatch while
  zero-argument/builtin/captured/negative controls are exercised; its
  roughly **60-second** compile is workflow-only. First positional
  implementation is saved/formatted and independently source-reviewed;
  actual expanded structured positional regression turns **GREEN 1 / 571
  filtered**, and frozen transformed four-way stock parity turns **GREEN
  1 / 4.82 seconds** across all three runtime modes.
- Result: **IN PROGRESS; genuine same-strategy positional CPython
  correctness AND whole-production structured RED-to-GREEN; no new
  strategy file; first post-extension full JIT RED on one preexisting
  hash-probe assertion plus approximately 111 mutex-poison secondary
  failures; narrowed existing test-only fix / fresh JIT 572 / 572,
  optimizer 213 / 213, typed 54 / 54, transformed 15 / 15, and scoped
  checks / positional release smoke 8 / 8 GREEN; actual hot comprehensions
  method selected in Profile but lost in Apply; normal comprehensions
  significant 0.93660x regression; repeated run HALTED pending
  diagnosis/refinement; full gate PENDING**.
- Reason: residual wrappers include actual positional Python and builtin
  method calls, but only an atomic, source-proven extension can preserve
  CPython lookup order and ownership.

### Attempt 3: recover source-anchored calls across hot-continuation clones

- Change: same-strategy refinement **saved and independently host-audited**
  in existing **`jit/typed_pipeline.rs`** only; no new production file,
  import, public API, runtime helper, or relaxation of prior
  eligibility/ownership rules.
- Root cause: actual nested benchmark function **`1:31`** is admitted in
  Profile (**9,608 → 9,968 bytes**) but remains unchanged in Apply
  (**12,552 bytes / 816 blocks**) because
  `clone_typed_hot_continuation` produces **11** blocks that share the
  original compiler `ResolvedName` but remap getter/call `InstrId`s. The
  private source sidecar still knows only the original IDs, and global
  temporary counts plus edge rules consequently reject all cloned pairs.
- Implemented proof: group exact compiler-private full `ResolvedName`s,
  reject CFG `BlockArg` transport, and require every global store/load/del
  for that name to belong to an isolated adjacent same-block pair.
  Exactly one pair retains the original source getter **and** call IDs;
  all clones match receiver, constant attribute, ordered arguments, and
  `Generic` access. Apply each group all-or-none and remove only its
  proven per-block store/delete; preserve all prior alias, descriptor,
  ownership, and evaluation controls.
- Evidence: actual positional normal comprehension is **0.93660x
  [0.91529, 0.95561]** / paired **0.94364x**, despite all **80** worker
  source-function sets / hidden trampolines remaining unchanged; existing
  synthetic Cell-only return does not cover the cloned hot nested body.
- Compatibility and tests: new real nested transformed **Apply**
  correctness regression now genuinely **fails 1 / 5.04 seconds**:
  captured-cell nested comprehension / `ProbeKey.__hash__` wrapper
  visibility is stock **`False`**, Profile **`False`**, Apply **`True`**;
  all zero/positional parity, descriptors, MRO, errors, finalizers,
  counters, and actual compiled nested body controls pass first.
  Independent genuine whole-production structured nested clone regression
  also **fails 1 / 571 filtered**: Profile preserves all existing targets,
  but Verify **`captured_comprehension.<locals>._dp_listcomp_3`** has
  **0** resolved method nodes versus the required **1**. The first
  candidate Verify run resolves **2** original-plus-clone nodes; after
  correcting the stale expected-one fixture, the full-pipeline regression
  turns **GREEN** with exactly **Profile 1 / Verify 2 / Apply 2** selected
  nodes. The frozen real transformed nested parity regression also turns
  **GREEN: 1 passed / 7.78 seconds**, preserving every previous
  zero/one/two-argument, descriptor, MRO, shadow, evaluation, finalizer,
  counter, and native-body control.
- Result: **LANDED CANDIDATE / RETAIN; both genuine nested stock-parity and full-pipeline
  clone REDs turn GREEN; independently audited one-file recovery;
  post-clone full JIT 572 / optimizer 213 / typed 54 / transformed 15 /
  scoped checks GREEN; post-clone release smoke 8 / 8 confirms actual hot
  `_dp_listcomp_20` Apply 12,552 / 816 → 12,644 / 823; adverse
  one-round comprehensions preserved but clean repeated comprehensions
  neutral 0.998325x; repeated deltablue 1.118447x retained /
  1.037585x over zero-argument; full gate GREEN 1,232 nodeids /
  95 batches / zero failures**.

## Verdict and next action

- Verdict: **LANDED CANDIDATE / RETAIN; genuine unchanged-production transformed
  stock-parity correctness AND independent whole-production
  typed-pipeline structured regression turn RED-to-GREEN; scoped package
  formatting/checks and complete JIT 572 / 572, optimizer 213 / 213,
  typed 54 / 54, grouped transformed 15 / 15, and release smoke 8 / 8
  pass; first normal throughput INVALID under contention; clean repeated
  zero-argument deltablue improves 1.077933x, richards inconclusive;
  lossless zero-argument residual profiles captured; genuine positional
  stock-parity AND structured RED-to-GREEN; first two-file extension saved
  / formatted / source-reviewed; first post-extension JIT suite RED on
  one brittle preexisting collision-probe assertion plus shared-lock
  secondary failures; test-only correction and fresh full JIT 572 / 572,
  optimizer 213 / 213, typed 54 / 54, transformed 15 / 15, and scoped
  checks GREEN; post-positional smoke 8 / 8 GREEN but motivating hot
  comprehensions method lost between Profile and Apply through cloned
  `InstrId`s; normal comprehensions 0.93660x with interval strictly below
  neutral; genuine nested transformed Apply-only correctness RED AND
  independent whole-production nested Verify structured RED confirm the
  clone gap; audited one-file recovery turns both GREEN, proving Profile
  1 / Verify 2 / Apply 2 nodes and transformed parity 1 / 7.78 seconds;
  fresh post-clone JIT 572 / optimizer 213 / typed 54 / transformed 15 /
  scoped checks GREEN; post-clone smoke 8 / 8 proves real hot benchmark
  Apply selection with unchanged coverage and smaller aggregate native
  code; normal 170030 adverse comprehensions and host outliers preserved;
  clean targeted 170351 proves comprehensions neutral 0.998325x,
  deltablue 1.118447x versus retained / 1.037585x versus zero-argument;
  lossless profiles complete; authoritative full gate GREEN with 1,232
  Python nodeids / 95 batches / 8 workers / zero failures**.
- Transferable lesson: inclusive generic lookup ancestry substantially
  exceeds the specific removable wrapper subset; preserve stock's exact
  method protocol instead of weakening descriptor or receiver semantics.
- Next action: integrate the fully validated retained candidate; its
  full-suite performance acceptance target has not been measured.
  Full-suite stock **1.10x** remains unmet.
