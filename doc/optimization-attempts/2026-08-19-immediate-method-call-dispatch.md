---
title: "Immediate Zero-Argument and Positional Method Call Dispatch"
---

# Immediate zero-argument and positional method call dispatch

- Current status: **FULLY VALIDATED / RETAIN LANDING CANDIDATE / ATTEMPT 4; retained Attempts 1–3
  and their negative evidence remain unchanged; owner-independent,
  profile-selected direct dispatch after the existing CPython method lookup
  was first implemented in four bounded production files; independent
  whole-production Rust typed-decision RED → GREEN includes the original
  and captured continuation clone; genuine actual transformed direct-hit
  RED → GREEN plus stock-parity / live-vectorcall / method-code controls
  both pass 2 / 2 in 2.61 seconds; first-candidate scoped formatting
  completed before pytest; first-candidate full serial JIT 578 / 578,
  optimizer 214 / 214, typed IR 54 / 54, broad transformed compatibility
  41 / 41 across 25 files in 43.34 seconds, combined Cargo check, and
  scoped formatting check are GREEN;
  fixed-eight release DEBUG-SINGLE smoke passes with unchanged coverage
  and hidden trampolines but ordinary native code grows 4.28% and blocks
  grow 3.95%; normally sampled fixed-eight `richards` improves 1.055383x
  while `deltablue` remains uncertain and candidate native bytes grow
  4.147%; clean repeated target improves raw `deltablue` 1.078213x and
  `comprehensions` 1.024145x but stock-paired `chaos` regresses 0.962989x
  and ordinary native code grows 5.09%; four zero-loss causal profiles
  expose replacement direct-call recursion and remaining code-growth /
  `chaos` risks; FIRST DIRECT CANDIDATE INCONCLUSIVE / REFINE GUARD CFG
  AND NATIVE DIRECT RECURSION; refined real-production Cranelift
  RED → GREEN now proves one native frame-pointer read, one original
  recursion helper only on its cold path, and one expected target-ID
  comparison; refinement touches two existing production paths / five
  production files total; original trampoline structured parity,
  frozen transformed integration 2 / 2, retained focused controls 10 /
  10, fresh serial JIT 579 / 579, optimizer 214 / 214, typed IR 54 / 54,
  broad transformed compatibility 51 / 51 across 27 files in 63.47
  seconds, combined Cargo check, and scoped formatting check are GREEN;
  scoped formatting preceded refined pytest; refined fixed-eight release
  DEBUG-SINGLE smoke preserves all coverage / hidden trampoline bytes and
  reduces ordinary native code 16,580 bytes / 1,334 blocks versus the
  rejected first candidate; normally sampled refined fixed-eight
  `deltablue` improves 1.07343x raw / 1.14257x stock-paired with noisy
  retained outliers, and stock-paired `chaos` is neutral; definitive
  refined three-round `deltablue` improves 1.10975889x raw / 1.13970302x
  stock-paired and `richards` 1.04242557x raw / 1.06015760x stock-paired,
  while `chaos` and `comprehensions` are stock-paired neutral; all four
  matched zero-loss refined causal profiles confirm descriptor-specific
  hot public recursion is eliminated without claiming unrelated
  recursion disappeared; authoritative full correctness gate is GREEN
  with 1,239 Python nodeids / 100 isolated batches / eight workers / zero
  failures; full-suite stock 1.10x remains unmet; not yet landed**.
- Historical Attempts 1–3 status: **LANDED CANDIDATE / RETAIN; real transformed CPython bound-method-visibility
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
- Historical Attempts 1–3 integrated baseline: retained `main` change **`mzvpmvzo`**, commit
  **`684842b9`**.
- Historical Attempts 1–3 candidate change: **`zkwnlurq`**, initially observed at mutable working
  commit **`a94f0ca3`**; subsequent snapshots change that commit ID.
- Current Attempt 4 integrated baseline: retained `main` change
  **`wmyqzzsr`**, commit **`149ad6c7`**.
- Current Attempt 4 candidate: change **`srxzvruu`**, initially observed
  at mutable working commit **`b23c1312`**.
- Historical Attempts 1–3 outcome: investigate whether the existing typed method-call operation
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

### Attempt 4: direct-call the profiled descriptor after unchanged CPython lookup

- Current state: **PROPOSED / IN PROGRESS** against integrated
  **`wmyqzzsr 149ad6c7`**, in new candidate **`srxzvruu b23c1312`**.
  Attempts 1–3 above are already retained history, including their genuine
  CPython-visibility corrections, the adverse **0.93660x** positional
  comprehensions result, subsequent continuation-clone correction, and
  final **1.118447x** repeated `deltablue` improvement. No current
  user-visible CPython mismatch is claimed for this new optimization.
- Hypothesis: the existing v3 **`call_hot_targets`** evidence already
  identifies a hot transformed method's exact function ID and validated
  receiver-plus-positional-argument plan, even when its class does not yet
  exist during eager compilation. Keep the original pinned-CPython
  **`_PyObject_GetMethod`** lookup and its actual owned returned
  descriptor. Only if that lookup reports a real method and its live
  descriptor independently matches the selected exact transformed Python
  function, execute the existing JIT direct-function body instead of
  routing the same descriptor and receiver through the generic vectorcall
  hook plus exact-positional trampoline. Unsupported or changed values
  retain the original vectorcall branch.
- Saved implementation scope is exactly **four existing production files**:
  `crates/soac_ir_typed/src/typed.rs`,
  `crates/soac_jit/src/jit/typed_pipeline.rs`,
  `crates/soac_jit/src/jit/function_targets.rs`, and
  `crates/soac_jit/src/jit/mod.rs`. Preserve the existing typed immediate
  method operation and source-grounded getter/call provenance; existing
  **`TypedInstrExtra`** now carries same-source, same-module,
  owner-independent **`TypedDirectFunctionCallGuard`** decisions for at
  most **two** selected transformed Python functions. Only existing
  **`Provided` / `DefaultSentinel`** argument plans qualify; packed-rest
  arguments, cross-module targets, generators, unsupported calls, and
  unprofiled targets stay excluded. The original anchored source pair
  carries the selected decision through verified continuation clones;
  existing function-target collection predeclares each direct body before
  codegen. Mechanical emission retains the unconditional pinned
  **`_PyObject_GetMethod`**, checks the actually returned exact Python
  descriptor's existing function ID / code / defaults snapshot and its
  current registered live vectorcall pointer, records existing direct-hit
  or guarded-fallback counters, and preserves owned descriptor / receiver
  / argument cleanup on success and recursion/error failure. No
  preexisting owner type, owner relocation, class-name guess, global
  state, or new runtime helper is required. Package-scoped formatting was
  completed **before** the first candidate transformed pytest; both the
  focused whole-production Rust typed decision and real transformed
  semantic / direct-hit tests now pass. Full serial affected Rust suites,
  broad transformed compatibility, combined crate/test-target checking,
  and scoped formatting verification also pass; root-owned release smoke
  confirms actual recovered direct/fallback edges but exposes material
  ordinary-native-code growth. Valid normal / repeated benchmarks, causal
  profiles, and the authoritative full correctness gate were **PENDING**
  at this earlier first-candidate checkpoint.
- Exact method-result guard must preserve the existing precise function
  identity plus current **`__code__`**, positional **`__defaults__`**, and
  **`__kwdefaults__`** snapshots; mutable keyword-only default dictionaries
  cannot be justified by pointer identity alone. The current Python
  function's actual **vectorcall pointer** must still match its registered
  compiled entry: vendored **`PyFunction_SetVectorcall`** can replace or
  clear it independently, and the repository already contains an actual
  `ctypes` null-vectorcall compatibility control. Changed pointer, code,
  defaults, identity, non-exact callable, alternate descriptor, or
  unavailable target must use the unchanged generic fallback.
- Since **`_PyObject_GetMethod` remains unconditional and unchanged**,
  CPython still resolves inherited descriptors, instance-dictionary
  shadows, dynamic class / MRO replacements, data descriptors, properties,
  custom **`__getattribute__`** / **`__getattr__`**, `staticmethod`, and
  `classmethod` before considering direct dispatch. Both motivating
  `deltablue` and `richards` hierarchies have ordinary mutable instance
  dictionaries, not proven slot-only instances; bypassing lookup or
  inferring shadow absence from an owner type/version is unsound.
- Preserve receiver evaluation and descriptor lookup before argument
  evaluation, original getter / call counters, receiver and actual
  descriptor ownership, positional default binding, raised exceptions,
  recursion overflow, profiler / tracer / monitoring observation, and
  exact reverse-order finalizer cleanup. The existing direct-entry path
  still invokes the **public recursion-check helper unconditionally**;
  unlike the retained vectorcall trampoline's hot inline stack guard,
  that replacement work can materially offset or exceed avoided
  dispatch. In particular, recursion-overflow failure must release the
  owned lookup descriptor and receiver / argument prefixes before
  propagating the original error. Performance and cleanup behavior need
  actual evidence; do not assume the existing trampoline guard can be
  copied directly because its cold failure currently returns immediately.
- Current retained release-smoke baseline is
  **`comparison-20260819-212319-5JbYy6`**: all **8 Apply workers / 397
  total JIT source rows including adapters / 204 direct-function-body
  rows**, **2,238,412 ordinary native bytes / 147,769 machine blocks /
  38,108 hidden trampoline bytes**, with **2,866 optimized typed blocks /
  204 functions**. Debug-single smoke timings are not valid throughput.
- Current retained normally sampled fixed-eight baseline is
  **`comparison-20260819-212444-EOYNr0`**: official stock
  **0.6555584208465822x**, previous-SOAC
  **0.9850631879265838x**, **80 Apply workers / 3,970 total source rows
  including adapters / 2,040 direct-function-body rows**, **23,159,960
  ordinary native bytes / 1,524,970 machine blocks / 381,080 hidden
  trampoline bytes / 2,866 typed blocks / 204 functions**. Three
  unusually slow `deltablue` workers contaminated that historical
  official previous-SOAC aggregate; do not present it as a clean
  regression or candidate gain.
- Current retained clean three-round / four-workload baseline is
  **`comparison-20260819-212748-3uvMT3`**: official stock
  **0.5358039397819471x**, previous-SOAC
  **1.0132710404047143x**, `chaos` approximately **38.9389 ms**,
  `comprehensions` approximately **42.8007 µs**, `deltablue`
  approximately **2.30527 ms**, and `richards` approximately
  **21.8213 ms**. All **120 Apply workers / 10,650 total JIT source rows
  including adapters / 5,490 direct-function-body rows** contain
  **54,686,760 ordinary native bytes / 3,596,430 machine blocks /
  777,240 hidden trampoline bytes / 2,265 typed blocks / 183 functions**.
- Current lossless native profiles contain **160 `deltablue` samples /
  228 `richards` samples**. The source-attributed, strictly disjoint
  removable dispatch / trampoline leaf ceiling, with benchmark shutdown
  excluded from the denominator, is at most approximately **13.888889%
  for `deltablue`** and **6.167% for `richards`**, explicitly excluding
  the retained **`_PyObject_GetMethod`** lookup. Using that **same
  shutdown-excluded denominator** at the demonstrated
  `EqualityConstraint.execute` source parent, disjoint vectorcall-hook
  self **2.083333 percentage points**, exact-trampoline self **2.083333
  points**, and live-thread-state acquisition **2.083333 points** total
  approximately **6.25 points**; its separate method-lookup self
  **2.083333 points** remains. For `Task.runTask`, the separately labeled
  **whole-profile denominator, including shutdown**, gives disjoint hook
  self **1.315553 points**, trampoline self **1.754404 points**, and
  CPython vectorcall dispatch **0.438851 points**, totaling **3.508808
  whole-profile points**; separate method-lookup self **1.315553
  whole-profile points** remains. These `richards` whole-profile source
  pieces must not be added to or directly compared with its
  shutdown-excluded **6.167%** ceiling. These are
  sample-limited ceilings, not predicted throughput gains; guard costs,
  direct recursion checks, nested-site eligibility, and code growth can
  eliminate the apparent opportunity. Do not add inclusive ancestors or
  unrelated nested source parents.
- Required focused unchanged-production evidence includes the now-verified
  **whole-production Rust structured optimization RED** proving that a
  source-selected inherited method
  already has a hot v3 target ID / argument plan but its owner-independent
  direct descriptor path is absent; a real transformed
  **stock → Profile → Verify → Apply** integration covering inherited
  classes, per-instance shadows, class / MRO mutation, properties,
  descriptors, custom getters, method **`__code__` / `__defaults__` /
  `__kwdefaults__` / vectorcall** mutation, tracing and monitoring,
  finite recursion / `RecursionError`, finalizers, original counters, and
  actual JIT native bodies. Existing wrapper-visibility,
  continuation-clone, captured-builtin, generator, keyword/starred, and
  `super` controls must remain intact.
- The new unchanged-production transformed compatibility baseline,
  **`tests/test_resolved_method_descriptor_direct_calls.py`**, is
  independently **GREEN: 1 passed in 1.70 seconds**; outer focused
  workflow elapsed **2.003 seconds**. It executes actual pinned stock
  alongside eager **Profile → Verify → Apply** and verifies two
  polymorphic inherited targets, zero / one / two explicit positional
  arguments, an effect-only call, per-instance method shadowing, data
  descriptors, custom **`__getattribute__`**, inherited class-method
  replacement, changed and restored positional defaults, and an
  unaffected builtin method. The actual profile contains at least
  **48 `call_hot_targets` observations at each of four source sites**,
  and generated native evidence contains **eight expected
  direct-function bodies**. This is an unchanged-production semantic
  **GREEN**, not an optimization RED and not a current CPython bug.
- The subsequently expanded **same unchanged-production** integration is
  independently **GREEN: 1 passed in 1.57 seconds**, now exercising real
  inherited **`Base.value`** vectorcall and method-code mutation after
  hot target training. Actual **`PyFunction_SetVectorcall(method, NULL)`**
  produces the identical **`TypeError`** in pinned stock and in each
  eager **Profile / Verify / Apply** subprocess; restoring the live
  pointer again returns **11**. Replacing that same method's
  **`__code__`** with a same-arity implementation returns **53** in
  stock and all three transformed modes; restoring the code again
  returns **11**. Existing inherited-polymorphism, shadows, descriptors,
  defaults, four hot target counters, and eight generated native-body
  controls remain GREEN. Production source was unchanged at this
  historical baseline, and the successful compatibility baseline
  demonstrates no CPython mismatch.
  Additional tracing / monitoring, recursion-failure, and finalizer
  controls remain **PENDING**.
- A second focused test now independently proves a **genuine
  unchanged-production transformed optimization RED** using actual
  recorded specialization counters:
  **`test_profiled_inherited_method_descriptors_use_direct_calls`** fails
  in **1.10 seconds** after executing real eager **Profile → Verify**.
  The inherited `immediate` call site first establishes at least **one
  nonzero profiled transformed target** and at least **48
  `call_hot_targets` observations**. Its actual Verify `call_direct`
  counter is **`branches={'fallback': 0, 'hit': 0}`**, so the real
  direct-hit assertion fails exactly **`0 > 0`**: the existing inherited
  method resolves correctly but never reaches a profiled direct native
  body. The complete unchanged-production focused file collects **two
  tests**: the comprehensive stock-parity / vectorcall / code-mutation
  integration **PASSES in 1.54 seconds**, and the direct-hit
  specialization regression **FAILS in 1.10 seconds**; total elapsed is
  **2.68 seconds**. An earlier draft mistakenly assumed a different
  branch-counter schema and two independently nonzero target rows; that
  fixture-only mistake was corrected before the authentic runtime RED and
  is not a production failure. An initial separate Rust fixture also
  failed during test-only name mangling **before reaching its intended
  production assertion**; that setup failure is likewise **not** a
  structured production RED. Production code was unchanged at the time
  of these genuine baseline REDs, and no user-visible CPython mismatch is
  asserted.
- An independent corrected Rust test now separately proves a **genuine
  unchanged-production whole-production typed-pipeline optimization
  RED**:
  **`profiled_inherited_method_descriptors_retain_ownerless_direct_targets`**
  executes the actual source lowering, instrumentation, existing v3
  selected method target / receiver-plus-argument plan, full
  **`optimize_blockpy_with_external_inline_callees`**, and real typed
  direct-target collector. Its **Profile control passes**, preserving
  original target observation; **Verify fails exactly**
  **`Verify must retain the profiled inherited descriptor target even
  when its class does not exist during eager planning`**, because the
  owner-independent selected target is absent. Focused Rust result is
  **0 passed / 1 failed / 577 filtered**. A **21.66-second** Rust build
  is workflow-only compilation time, never benchmark or runtime
  throughput evidence. The earlier double-underscore/name-mangling
  fixture failure and a transient iterator compile error are excluded
  from this genuine production RED.
- The same frozen production-path Rust regression now independently
  verifies genuine **RED → GREEN: 1 passed / 577 filtered** after the
  bounded four-file implementation. It still checks actual source
  lowering, instrumentation, real v3 target selection / argument
  binding, full typed production rewrites, and direct-target collection.
  The strengthened real test now covers both the original inherited
  method site **and a captured comprehension's original-plus-hot-cloned
  continuation**. **Profile** retains no ownerless direct targets or
  descriptor guards, preserving the original observation graph;
  **Verify and Apply** retain the selected inherited target, direct-body
  predeclaration, and descriptor guard on each original / cloned source,
  while every old owner-guard list remains empty. This proves the
  owner-independent typed decision remains valid even though its class
  does not exist during eager planning.
- After package-scoped formatting, the exact frozen actual transformed
  focused file independently verifies genuine optimization **RED →
  GREEN: 2 passed in 2.61 seconds**. The previously failing
  **`test_profiled_inherited_method_descriptors_use_direct_calls`** now
  observes an actual positive **Verify `call_direct.hit`** count for the
  profiled inherited method. Its companion
  **`test_resolved_method_descriptor_direct_calls_preserve_cpython_dispatch`**
  simultaneously preserves pinned stock parity across eager **Profile →
  Verify → Apply**, two inherited targets, zero / one / two positional
  arguments, effect-only calls, instance shadowing, data descriptors,
  custom getters, inherited class replacement, mutable defaults, a
  builtin control, real **NULL vectorcall → `TypeError` → restoration**,
  and same-arity **`__code__` 53 → restoration 11**. Four real hot target
  counters and eight actual native function bodies remain present. A
  one-time approximately **22-second** debug-extension rebuild is
  workflow-only build overhead, never throughput evidence.
- Fresh complete, serial affected Rust libraries are **GREEN: JIT 578 /
  578, optimizer 214 / 214, and typed IR 54 / 54**. The broad actual
  transformed compatibility matrix is **GREEN: 41 / 41 tests across 25
  files in 43.34 seconds**. Combined
  **`cargo check -p soac_ir_typed -p soac_jit --tests --quiet`** and
  scoped **`just fmt-rust-check soac_ir_typed soac_jit`** both pass.
  Package-scoped formatting preceded the first candidate pytest. The
  four production paths are frozen; these correctness/check results are
  neither throughput measurements nor evidence of a completed full gate.
- The frozen candidate's fixed-eight release **DEBUG-SINGLE coverage
  smoke `comparison-20260819-223233-tpmBoI`** versus mode-matched
  retained **`comparison-20260819-212319-5JbYy6`** passes all **eight
  actual Apply PIDs**, preserves the identical **397 JIT source rows
  including adapters / 204 direct bodies**, and reports **2,866 typed
  blocks / 204 typed functions** with zero errors. Hidden vectorcall
  trampolines remain exactly **38,108 bytes**, including every arity.
  Ordinary native code instead grows **2,238,412 → 2,334,180 bytes
  (+95,768 / +4.28%)** and **147,769 → 153,608 blocks (+5,839 /
  +3.95%)**. `deltablue` grows **60,732 bytes / 13.34% across 23
  changed bodies**, including `EqualityConstraint.execute` **2,320 →
  3,868 bytes**, `Plan.execute` **9,244 → 11,832**,
  `Planner.add_propagate` **5,156 → 8,720**, and `make_plan` **7,484 →
  13,676**. `richards` grows **18,856 bytes / 5.40% across nine changed
  bodies**, including `Task.runTask` **8,880 → 11,240** and
  `Handler.fn` **13,716 → 18,484**. `chaos` adds **10,252 bytes**,
  `comprehensions` **2,676**, and `float` **3,252**; `fannkuch`,
  `nbody`, and `spectral_norm` are unchanged. Actual source-edge events
  confirm `EqualityConstraint.execute` **2 direct / 2 fallback**,
  `Plan.execute` **4 direct / 2 fallback**, `Task.runTask` **3 direct /
  3 fallback**, and `Handler.fn` **6 direct / 6 fallback**, without
  missing-target, arity, or unsupported-edge errors. This proves real
  direct-edge recovery and exposes a serious code-growth risk;
  **DEBUG-SINGLE cold timings are not valid throughput measurements**.
  At this smoke checkpoint, valid fixed-eight normal / clean repeated
  comparisons, causal profiles, and the full gate remained pending.
- The subsequent normally sampled fixed-eight candidate comparison
  **`comparison-20260819-223548-gYDTS2`** versus retained
  **`comparison-20260819-212444-EOYNr0`** verifies all **80 actual Apply
  PIDs**, the same **3,970 total JIT source rows including adapters /
  2,040 direct bodies**, **2,866 typed blocks / 204 typed functions**,
  exactly **381,080 hidden-trampoline bytes**, and zero errors. Ordinary
  native code grows **23,159,960 → 24,120,400 bytes (+960,440 /
  +4.147%)** and **1,524,970 → 1,583,640 blocks (+58,670 / +3.847%)**.
  The official fixed-eight geometric means are **0.6729416640142044x
  versus stock** and **1.035396831312697x versus previous SOAC**.
  Robust `deltablue` worker medians improve **2.265631 → 2.056944 ms**,
  raw **1.101455x, 95% CI [0.98774, 1.43141]**, and stock-paired
  **1.095627x**; the raw interval crosses neutral and severe historical
  retained-baseline outliers prevent a definitive claim. `richards`
  improves **22.299028 → 21.128856 ms**, raw **1.055383x, 95% CI
  [1.02392, 1.07844]**, and stock-paired **1.056675x, 95% CI [1.02343,
  1.08413]**. `chaos` and `comprehensions` remain neutral. `nbody` has
  **byte-identical generated source bodies** yet an apparently
  significant paired **0.91687x**, demonstrating external stock/worker
  drift rather than evidence of a generated-code regression. This
  normal run remained provisional before the subsequent clean repeated
  comparison; causal profiles and the authoritative full gate were
  **PENDING** at that checkpoint.
- The definitive three-round targeted comparison
  **`comparison-20260819-224008-shxzPs`** versus retained
  **`comparison-20260819-212748-3uvMT3`** preserves all **120 actual
  Apply PIDs**, the identical **10,650 total JIT source rows including
  adapters / 5,490 direct bodies**, **2,265 typed blocks / 183 typed
  functions**, exactly **777,240 hidden-trampoline bytes**, and zero
  errors. Ordinary native code grows **54,686,760 → 57,470,520 bytes
  (+2,783,760 / +5.09%)** and **3,596,430 → 3,766,440 blocks (+170,010
  / +4.73%)**. Official targeted geometric means are
  **0.5286221076693118x versus stock** and **1.0116250410937884x versus
  previous SOAC**. Robust `deltablue` medians improve **2.305270891 →
  2.138047430 ms**, raw **1.078213x, 95% CI [1.042818, 1.110329]**; its
  stock-paired **1.038803x, 95% CI [0.999962, 1.074140]** is marginal
  and crosses neutral. `richards` improves **21.821327875 →
  21.141180437 ms**, raw **1.032172x, 95% CI [1.017447, 1.048155]**,
  but stock-paired **0.999101x, 95% CI [0.980374, 1.017818]** is
  neutral. `comprehensions` improves **42.800702 → 41.791633 μs**, raw
  **1.024145x, 95% CI [1.012192, 1.040317]**, and stock-paired
  **1.017522x, 95% CI [1.004959, 1.034576]**. The broad mixed guardrail
  `chaos` instead worsens **38.938904 → 39.494066 ms**, raw
  **0.985943x, 95% CI [0.970202, 1.011858]**; although that raw interval
  is neutral, stock-paired **0.962989x, 95% CI [0.949799, 0.995557]**
  is a significant slowdown. Round estimates **0.9677x / 1.0654x /
  0.9660x** and a maximum **52.63 ms** expose substantial variability
  without erasing the paired guardrail failure. Verdict is
  **PROVISIONAL / INVESTIGATE CHAOS AND PUBLIC RECURSION**: inspect
  generated-code growth and the retained direct-call public recursion
  helper before any retention decision; matched lossless causal profiles
  and the authoritative full gate were **PENDING** at this comparison
  checkpoint.
- The subsequent matched **zero-loss `deltablue` causal profiles** have
  **160 retained / 156 candidate samples**; the percentages exclude
  interpreter shutdown only. Generic vectorcall-hook self falls
  **6.944444% → 1.459854% (−5.484590 percentage points)** and
  thread-state/TLS self falls **5.555556% → 3.649635% (−1.905921
  percentage points)**. However, direct calls introduce a previously
  absent public recursion-check cost **0% → 3.649635%**, disjointly
  attributed to `Plan.execute` **2.189781%**,
  `EqualityConstraint.execute` **0.729927%**, and `Planner.make_plan`
  **0.729927%**. Authoritative `_PyObject_GetMethod` remains intentionally
  unchanged, **14.583333% → 15.328467%**. The replacement recursion cost
  explains why eliminating generic dispatch does not translate directly
  into equivalent stock-paired throughput; these separate groups are not
  summed or represented as disjoint unless stated.
- Matched **zero-loss `richards` causal profiles** have **228 retained /
  213 candidate samples**. Exact-trampoline share falls **11.452382% →
  1.886773%**, but generic-hook share rises **4.404840% → 6.132768%**
  and thread-state/TLS share rises **2.203926% → 6.130757%**. These
  groups may be nested or otherwise overlap; their percentages are not
  additive. The shifted remaining costs are consistent with the
  repeated stock-paired throughput result being neutral.
- Matched **zero-loss `comprehensions` causal profiles** have **545
  retained / 504 candidate samples**. Generic-hook share falls
  **4.897931% → 3.220494%**, and public-recursion share falls
  **0.212499% → 0%**; the separate reductions align with the positive
  repeated stock-paired throughput result without summing potentially
  nested groups.
- The **zero-loss candidate-only `chaos` causal profile** has **213
  samples**; there is **no matched retained `chaos` baseline**. Direct
  public recursion accounts for only **0.471480%**, attributed solely
  to `transform_point`, whose generated body grows **4,616 bytes**.
  Thus recursion alone does not explain the significant paired mixed
  guardrail slowdown; changed guard CFG / ordinary-native-code growth
  remain plausible costs requiring refinement. Profiling requires the
  measured worker directory **basename**, not its absolute path; this
  is a workflow-only invocation constraint, not benchmark evidence.
- Causal verdict: **FIRST DIRECT CANDIDATE INCONCLUSIVE / REFINE GUARD
  CFG AND NATIVE DIRECT RECURSION**. Preserve unchanged authoritative
  method lookup, live-vectorcall / descriptor guards, explicit ownership,
  and recursion correctness while investigating a smaller emitted guard
  CFG and the replacement public direct-call recursion overhead. At this
  first-candidate checkpoint, no refined implementation, retained
  performance claim, or completed authoritative full gate existed.
- A new **genuine first-candidate production-path structured refinement
  RED** now lowers a real inherited `Base.value` immediate call through
  its actual selected typed descriptor guard and declared direct native
  body, then inspects emitted Cranelift instructions/CFG rather than
  rendered text. Unchanged first-candidate production has **zero native
  frame-pointer reads**, **one original public recursion-helper call /
  zero cold-only helper calls**, and **two comparisons of the same packed
  direct target**. Independently verified saved refinement turns this
  authentic real-production Cranelift optimization **RED → GREEN**:
  actual lowered inherited `Base.value` now emits **exactly one
  `GetFramePointer`**, **exactly one original public recursion helper
  exclusively on its marked cold path**, and **exactly one comparison
  with the expected packed target ID**. This contrasts with the genuine
  first-candidate **frame-pointer 0 / helper total 1 and cold 0 / target
  comparisons 2** RED; it is not a fixture/parser error, throughput
  result, or CPython-visible behavior mismatch.
- Saved same-strategy refinement changes only the existing production
  **`crates/soac_jit/src/jit/mod.rs`** and
  **`crates/soac_jit/src/jit/vectorcall.rs`**, plus an existing
  **`#[cfg(test)]`-only `crates/soac_jit/src/jit/test.rs`** regression.
  Since `vectorcall.rs` is new to this strategy while `mod.rs` was already
  included, Attempt 4 now changes **five existing production files in
  total**; the original typed selection / declaration architecture
  remains unchanged. After unchanged authoritative
  **`_PyObject_GetMethod`**, only an **exact Python function** can match
  at most **two** profiled IDs through a single ordered ID pass. A null
  runtime-metadata guard precedes the same shared current registered
  `__code__` / positional-default / keyword-default snapshot checks;
  mutable keyword defaults still require **`__kwdefaults__ == NULL`**,
  and the nonnull current vectorcall pointer must exactly equal its
  registered compiled entry. Original Profile / Verify classification,
  generic and constructor direct-call paths, fallback counters, and all
  ownership/error semantics remain intact. The existing shared
  trampoline's recursion failure retains **`ReturnNull`**, while the new
  resolved-descriptor native recursion guard uses **`JumpTo` its existing
  scoped owned-input cleanup**, preserving descriptor / receiver /
  argument release on overflow. Unsupported architectures retain their
  original public CPython recursion helper. The preexisting four generic
  / constructor direct-call paths are untouched; only the new resolved
  descriptor direct edge opts into the native guard and scoped-cleanup
  policy. The existing shared trampoline's genuine structured regression
  independently remains **GREEN**. Refined package-scoped formatting
  completed **before** candidate transformed pytest; the frozen real
  stock / Profile → Verify → Apply semantic-and-direct-hit integration
  passes **2 / 2**, while retained focused method / recursion /
  compatibility controls pass **10 / 10**. Fresh complete serial Rust
  suites pass **JIT 579 / 579, optimizer 214 / 214, and typed IR 54 /
  54**. The expanded real transformed compatibility matrix passes
  **51 / 51 across 27 files in 63.47 seconds**; combined
  **`cargo check -p soac_opt -p soac_ir_typed -p soac_jit --tests
  --quiet`** and the package-scoped formatting check pass. The subsequent
  refined release smoke confirms unchanged hidden-trampoline bytes and
  reduced ordinary native code versus the first candidate. Refined valid
  fixed-eight normal sampling completed with all coverage preserved; the
  definitive clean three-round benchmark was **RUNNING**, and new causal
  profiles / the authoritative full gate remained **PENDING**, at this
  earlier refined-validation checkpoint. At that time the preceding
  three-round / causal-profile data applied only to the
  adverse/inconclusive first candidate.
- Refined release fixed-eight **DEBUG-SINGLE smoke
  `comparison-20260819-230959-2ICnbd`** versus retained
  **`comparison-20260819-212319-5JbYy6`** and adverse first candidate
  **`comparison-20260819-223233-tpmBoI`** passes all **eight actual
  measured Apply PIDs**, preserving the identical **397 total JIT source
  rows including adapters / 204 direct bodies**, **2,866 typed blocks /
  204 typed functions**, and every hidden-trampoline arity / exact total
  **38,108 bytes**; there are zero **ERROR / CRITICAL** events. Ordinary
  retained → first → refined native code is **2,238,412 → 2,334,180 →
  2,317,600 bytes** and **147,769 → 153,608 → 152,274 blocks**. Refined
  code remains **+79,188 bytes / +4,505 blocks** versus retained, but
  removes **16,580 bytes / 1,334 blocks** versus the rejected first
  candidate. First → refined per-workload byte reductions are
  `deltablue` **10,064**, `richards` **3,604**, `chaos` **2,036**,
  `comprehensions` **372**, and `float` **504**; `fannkuch`, `nbody`,
  and `spectral_norm` remain byte-identical. Named first → refined
  source-body **bytes / blocks** are `EqualityConstraint.execute`
  **3,868 / 238 → 3,500 / 210**, `Plan.execute` **11,832 / 738 →
  11,712 / 730**, `Planner.make_plan` **13,676 / 865 → 13,056 / 812**,
  `Task.runTask` **11,240 / 694 → 10,784 / 654**, `HandlerTask.fn`
  **18,484 / 1,168 → 17,432 / 1,076**, and
  `Chaosgame.transform_point` **50,892 / 3,351 → 49,988 / 3,274**.
  Actual direct / fallback edges remain present.
  **DEBUG-SINGLE cold timings are invalid throughput evidence**. At this
  coverage-smoke checkpoint, the normally sampled comparison was
  running and clean repeated benchmarks, causal profiles, and full gate
  were pending.
- Refined normally sampled fixed-eight
  **`comparison-20260819-231117-FXavZ9`** records exact official
  geometric means **0.6865833897338185x versus stock** and
  **0.9970497902989457x versus previous SOAC**; the official previous
  mean is noisy. All **80 actual Apply PIDs** retain the identical
  **3,970 total JIT source rows including adapters / 2,040 direct
  bodies**, **2,866 typed blocks / 204 typed functions**, exactly
  **381,080 hidden-trampoline bytes**, and zero errors; `fannkuch`,
  `nbody`, and `spectral_norm` source bodies remain byte-identical.
  Ordinary retained → first → refined native code is **23,159,960 →
  24,120,400 → 23,952,600 bytes** and **1,524,970 → 1,583,640 →
  1,570,300 blocks**: refined remains **+792,640 bytes / +45,330
  blocks** versus retained but eliminates **167,800 bytes / 13,340
  blocks** versus the first candidate. Relative to retained, robust
  `deltablue` worker medians improve **2.26563 → 2.11065 ms**, raw
  **1.07343x, 95% CI [1.06809, 1.39325]**, and stock-paired
  **1.14257x, 95% CI [1.11695, 1.47461]**; substantial retained-baseline
  outliers distort the upper interval and prevent treating this normal
  pass as definitive. `richards` changes **22.2990 → 21.6909 ms**, raw
  **1.02804x, 95% CI [0.99765, 1.05536]** / stock-paired
  **1.04519x, 95% CI [1.01371, 1.09836]**. `chaos` raw **0.97784x** is
  significantly adverse but stock-paired **0.99248x, 95% CI [0.97563,
  1.03286]** is neutral; `comprehensions` paired **0.99820x** is also
  neutral. Against the rejected first candidate, paired `deltablue`
  improves **1.04284x, 95% CI [1.02533, 1.16514]**, while `richards`
  **0.98913x**, `chaos` **1.00657x**, and `comprehensions` **0.99991x**
  are neutral. Significant raw `fannkuch` / `spectral_norm` changes
  despite **byte-identical generated source bodies** confirm external
  timing drift. The definitive refined clean three-round comparison was
  **RUNNING** at this normal-pass checkpoint; refined causal profiles and
  authoritative full gate were then **PENDING**.
- Definitive refined clean three-round targeted comparison
  **`comparison-20260819-231434-GW1s2w`** records exact official
  geometric means **0.5613133486246105x versus stock** and
  **1.027017074961002x versus previous SOAC**. All **120 actual measured
  Apply PIDs** preserve **10,650 identical total JIT source rows
  including adapters / 5,490 direct bodies**, **2,265 typed blocks / 183
  typed functions**, every hidden-trampoline arity and exact total
  **777,240 bytes**, and zero errors. Ordinary retained → first →
  refined native code is **54,686,760 → 57,470,520 → 56,982,240 bytes**
  and **3,596,430 → 3,766,440 → 3,727,500 blocks**: refined still adds
  **2,295,480 bytes / 131,070 blocks** versus retained, but removes
  **488,280 bytes / 38,940 blocks** versus the adverse first candidate.
  `deltablue` improves **2.305270891 → 2.077271844 ms**, raw
  **1.10975889x, 95% CI [1.09359, 1.12969]** and stock-paired
  **1.13970302x, 95% CI [1.10924, 1.16692]**. `richards` improves
  **21.821327875 → 20.933223937 ms**, raw **1.04242557x, 95% CI
  [1.02812, 1.05426]** and stock-paired **1.06015760x, 95% CI [1.04276,
  1.08066]**. `chaos` raw **38.938904 → 40.615719 ms / 0.958715x,
  approximately 95% CI [0.947, 0.986]**, and `comprehensions` raw
  **42.800702 → 43.924292 μs / 0.97442x, 95% CI [0.9603, 0.9936]**, are
  both adversely significant; however, their respective stock-paired
  **0.98850885x [0.97645, 1.02004]** and **0.99510391x [0.97957,
  1.01549]** are neutral. Against the adverse first candidate,
  stock-paired `deltablue` improves **1.09713079x**, `richards`
  **1.06111111x**, and `chaos` **1.02650074x [1.0093, 1.0437]**,
  confirming recovery of its prior significant mixed-guardrail
  regression. `comprehensions` is candidly slower than the first
  candidate, **0.97796807x [0.9614, 0.9962]**, despite being neutral
  versus retained. At this benchmark checkpoint, status was **RETAIN
  LANDING CANDIDATE / AUTHORITATIVE FULL GATE PENDING**, not landed:
  both motivating workloads improve
  with raw and paired intervals above neutral; mixed controls are
  stock-paired neutral versus retained, but adverse raw controls and
  remaining ordinary-native-code growth are preserved. Refined matched
  lossless causal profiles were **PENDING** at this comparison
  checkpoint; the full-suite stock **1.10x** goal remains unmet /
  unmeasured.
- All **four refined causal profiles now complete with `Total Lost
  Samples: 0`**, matched measured worker / loop count / sampling
  frequency within each comparison, and **`SOAC_JIT_BB_MAP=0`**. Every
  denominator excludes **interpreter shutdown only**; measured in-loop
  garbage-collection work remains included. The following separate leaf
  and source-parent shares are not additive across potentially nested
  groups, and their sparse samples do not independently prove throughput
  causation.
- Matched zero-loss **`deltablue` retained → first → refined samples
  160 → 156 → 162** verify descriptor direct public recursion
  **0% → 3.649635% → 0%**, exactly removing the first candidate's new
  hot helper. Generic-hook share is **6.944444% → 1.459854% →
  2.174679%**, and thread-state/TLS share **5.555556% → 3.649635% →
  2.898006%**. Authoritative `_PyObject_GetMethod` intentionally remains
  **14.583333% → 15.328467% → 21.014067%**; separate refined source
  parents include `EqualityConstraint.execute` **8.697541 percentage
  points**, `Planner.add_propagate` **4.347009 points**, and
  `Plan.execute` **2.173505 points**. Do not characterize required
  lookup as removable or sum its ancestry with other leaf groups.
- Matched zero-loss **`richards` retained → first → refined samples
  228 → 213 → 215** show public recursion **0.440785% → 0.471442% →
  0%**, generic hook **4.404840% → 6.132768% → 3.270666%**, exact
  trampoline **11.452382% → 1.886773% → 3.738909%**, TLS
  **2.203926% → 6.130757% → 6.541333%**, and retained method lookup
  **5.727195% → 5.659315% → 7.008571%**. TLS rises despite the measured
  workload gain; finite sampling and overlapping stack ancestry rule out
  causal sums.
- Matched zero-loss **`comprehensions` retained → first → refined samples
  545 → 504 → 601** show generic hook **4.897931% → 3.220494% →
  2.726336%**, TLS **5.528461% → 4.133180% → 4.276216%**, trampoline
  **1.277317% → 0% → 0.194320%**, and method lookup
  **2.340974% → 1.378886% → 1.555732%**. Total public recursion changes
  **0.212499% → 0% → 0.389811%** entirely through a separate unchanged
  direct-call site; it must not be attributed to the refined descriptor
  edge or described as globally eliminated.
- Zero-loss **`chaos` first → refined samples 213 → 205** have **no
  retained baseline profile**. Descriptor-specific
  `Chaosgame.transform_point` public recursion drops **0.471480% → 0%**,
  but **total** public recursion rises **0.471480% → 0.975766%** through
  unrelated unchanged `GVector.__add__` / `linear_combination` direct
  sites. Generic hook changes **2.830890% → 3.902064%** and TLS
  **6.603736% → 6.830361%**. Sparse unmatched-to-retained observations
  do not prove why stock-paired throughput is neutral or justify
  claiming all recursion vanished. The four causal captures are
  complete; at this causal-profile checkpoint, only the authoritative
  full correctness gate remained **PENDING** before landing.
- The final authoritative **`just test-all` gate exits zero / GREEN**;
  its retained log is
  **`work/logs/resolved-method-descriptor-test-all.log`**. Transformed
  Python covers exactly **1,239 pytest nodeids / 100 isolated batches /
  eight workers / 100 passed / zero failures**. Rust suites pass **JIT
  579 / 579 in 15.86 seconds**, including the new production-consumed
  cold descriptor-direct guard regression; **optimizer 214 / 214 in
  0.59 seconds**, **typed IR 54 / 54 in 0.01 seconds**, **lowering 371 /
  371 in 0.55 seconds**, and **PyO3 extension 8 / 8 in 0.11 seconds**,
  along with the remaining workspace tests. Runtime build takes **1.598
  seconds**; the complete Cargo phase takes **82.978 seconds**,
  including approximately **1 minute 04 seconds** compiling test
  targets; transformed pytest takes **78.708 inner / 78.725 outer
  seconds**, for total test phase **161.717 seconds**. The new real
  transformed **two-node integration passes in 3.37 seconds**; the known
  serial **28-node counter-dump shard takes 78.42 seconds**. Final
  status is **FULLY VALIDATED / RETAIN LANDING CANDIDATE**, not already
  landed; the full-suite stock **1.10x** goal remains unmet /
  unmeasured.
- Measurements and verdict: unchanged-production transformed stock
  parity, inherited live-vectorcall mutation, and same-arity method-code
  mutation **GREEN 1 / 1.57 seconds**, and again **GREEN 1 / 1.54
  seconds** in the two-test focused run; genuine actual transformed
  direct-hit optimization **RED 1 / 1.10 seconds → GREEN together with
  unchanged stock-parity semantics, 2 passed / 2.61 seconds**; independent
  whole-production Rust typed-decision optimization **RED 0 passed / 1
  failed / 577 filtered → GREEN 1 passed / 577 filtered**; bounded
  four-production-file implementation saved and package-formatted before
  pytest; full serial Rust **JIT 578 / 578, optimizer 214 / 214, typed
  IR 54 / 54**, broad transformed compatibility **41 / 41 across 25 files
  in 43.34 seconds**, combined Cargo test-target check, and scoped
  formatting check **GREEN**; mode-matched fixed-eight release coverage
  smoke **GREEN 8 / 8**, but ordinary native bytes **+4.28%** and blocks
  **+3.95%** with unchanged hidden trampolines; normally sampled
  fixed-eight **80 Apply PIDs**, official stock **0.6729416640142044x**
  / previous SOAC **1.035396831312697x**, `richards` **1.055383x** with
  raw and paired intervals above neutral, `deltablue` **1.101455x** with
  raw interval crossing neutral, and ordinary native bytes **+4.147%**;
  clean three-round target **stock 0.5286221076693118x / previous SOAC
  1.0116250410937884x**, raw `deltablue` **1.078213x** but marginal
  stock-paired **1.038803x**, stock-paired `richards` neutral
  **0.999101x**, `comprehensions` raw **1.024145x** / stock-paired
  **1.017522x**, and significant stock-paired `chaos` regression
  **0.962989x** with ordinary native bytes **+5.09%**; zero-loss causal
  `deltablue` **160 / 156**, `richards` **228 / 213**,
  `comprehensions` **545 / 504**, and candidate-only `chaos` **213**
  complete, exposing new `deltablue` direct public recursion
  **0% → 3.649635%**; new real-production Cranelift refinement
  **RED frame-pointer 0 / helper total 1 and cold 0 / target comparisons
  2 → GREEN frame-pointer 1 / original helper only cold / target
  comparison 1**; saved refinement adds `vectorcall.rs` for **five total
  production files**; refined original-trampoline structured parity,
  frozen transformed integration **2 / 2**, retained controls **10 /
  10**, fresh serial **JIT 579 / 579, optimizer 214 / 214, typed IR 54 /
  54**, broad transformed **51 / 51 across 27 files in 63.47 seconds**,
  combined Cargo check, and scoped formatting check **GREEN**; refined
  fixed-eight DEBUG-SINGLE smoke **GREEN 8 / 8**, hidden trampolines
  **38,108 bytes unchanged**, ordinary first → refined native
  **2,334,180 → 2,317,600 bytes / 153,608 → 152,274 blocks**; refined
  fixed-eight normal **80 Apply PIDs**, official stock
  **0.6865833897338185x / previous SOAC 0.9970497902989457x**, robust
  `deltablue` **1.07343x raw / 1.14257x stock-paired** with retained
  outliers, stock-paired `chaos` neutral **0.99248x**, and
  first → refined ordinary native **24,120,400 → 23,952,600 bytes /
  1,583,640 → 1,570,300 blocks**; definitive refined clean repeated
  official stock **0.5613133486246105x / previous SOAC
  1.027017074961002x**, `deltablue` **1.10975889x raw / 1.13970302x
  stock-paired**, `richards` **1.04242557x raw / 1.06015760x
  stock-paired**, `chaos` / `comprehensions` raw adverse but paired
  neutral **0.98850885x / 0.99510391x**, and unchanged hidden
  trampolines **777,240 bytes**; four refined lossless causal profiles
  **deltablue 160 / 156 / 162, richards 228 / 213 / 215,
  comprehensions 545 / 504 / 601, chaos first / refined 213 / 205**
  complete, with descriptor direct public recursion eliminated while
  unrelated `chaos` / `comprehensions` recursion remains; authoritative
  full **`just test-all`** gate **GREEN: 1,239 Python nodeids / 100
  isolated batches / eight workers / zero failures**, JIT **579**,
  optimizer **214**, typed IR **54**, lowering **371**, and PyO3 **8**.
  The stock full-suite **1.10x** target remains unmet. No full-suite
  stock gain, new CPython correctness fix, or already-landed candidate
  is claimed; the final verdict is **FULLY VALIDATED / RETAIN LANDING
  CANDIDATE**.

## Historical Attempts 1–3 verdict and next action

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

## Current Attempt 4 verdict and next action

- Verdict: **FULLY VALIDATED / RETAIN LANDING CANDIDATE; profiled, owner-independent direct
  dispatch after unchanged CPython method lookup is package-formatted
  and implemented in four existing production files; whole-production
  Rust original / captured-clone decision turns RED → GREEN 1 passed /
  577 filtered; actual transformed inherited direct-hit optimization and
  full stock-parity / vectorcall / method-code compatibility both pass
  2 / 2 in 2.61 seconds; complete serial JIT 578 / 578, optimizer 214 /
  214, typed IR 54 / 54, broad transformed 41 / 41 across 25 files in
  43.34 seconds, combined Cargo check, and scoped formatting check are
  GREEN; mode-matched fixed-eight release DEBUG-SINGLE smoke passes
  8 / 8 and recovers real direct/fallback edges, but ordinary native
  bytes grow 4.28% and blocks 3.95% while hidden trampolines remain
  unchanged; normally sampled fixed-eight `richards` improves 1.055383x
  with raw and paired intervals above neutral, `deltablue` 1.101455x
  remains uncertain, and ordinary native bytes grow 4.147%; clean
  repeated targeted `deltablue` improves 1.078213x raw / marginal
  1.038803x paired, `richards` paired 0.999101x is neutral,
  `comprehensions` improves 1.024145x raw / 1.017522x paired, but
  `chaos` stock-paired 0.962989x significantly regresses and ordinary
  native bytes grow 5.09%; four zero-loss causal profiles expose new
  `deltablue` public direct recursion 0% → 3.649635%, mixed remaining
  wrapper/TLS costs, and unmatched `chaos` direct recursion only
  0.471480%; FIRST DIRECT CANDIDATE INCONCLUSIVE / REFINE GUARD CFG AND
  NATIVE DIRECT RECURSION; genuine actual emitted-production Cranelift
  refinement turns RED → GREEN with exactly one frame-pointer read, one
  original public recursion helper only on the cold path, and one target
  comparison; bounded refinement saves two existing production paths /
  five Attempt 4 production files total; refined original-trampoline
  structured parity, frozen transformed integration 2 / 2, retained
  focused controls 10 / 10, complete serial JIT 579 / 579, optimizer 214
  / 214, typed IR 54 / 54, expanded transformed 51 / 51 across 27 files
  in 63.47 seconds, combined Cargo check, and scoped formatting check
  are GREEN; refined fixed-eight DEBUG-SINGLE smoke passes 8 / 8,
  preserves 38,108 hidden bytes and all real direct/fallback edges, and
  reduces first-candidate ordinary native code 16,580 bytes / 1,334
  blocks while retaining 79,188 additional bytes versus retained;
  refined normal covers all 80 Apply workers, improves `deltablue`
  1.07343x raw / 1.14257x paired with retained outliers, leaves paired
  `chaos` neutral 0.99248x, and removes 167,800 ordinary native bytes
  versus the first candidate; definitive clean three-round `deltablue`
  improves 1.10975889x raw / 1.13970302x paired, `richards`
  1.04242557x raw / 1.06015760x paired, and `chaos` /
  `comprehensions` remain paired-neutral versus retained despite adverse
  raw controls; versus the first candidate, paired `chaos` recovers
  1.02650074x while `comprehensions` regresses 0.97796807x; hidden
  trampolines remain exactly 777,240 bytes, ordinary native code still
  grows 2,295,480 bytes versus retained; all four matched refined
  zero-loss causal profiles complete and prove descriptor-specific public
  recursion elimination without removing authoritative method lookup or
  unrelated direct recursion; authoritative full correctness gate exits
  zero with 1,239 Python nodeids / 100 isolated batches / eight workers
  / zero failures and JIT 579, optimizer 214, typed IR 54, lowering 371,
  PyO3 8; full-suite stock 1.10x remains unmet and the candidate is not
  yet landed**.
- Next action: integrate the fully validated retained landing candidate;
  preserve the adverse raw controls and ordinary-native-code growth.
  Keep the genuine historical
  transformed direct-hit / whole-production Rust REDs and vectorcall /
  code-mutation controls frozen and preserve all Attempts 1–3 and their
  negative outcomes;
  reject any candidate that weakens method lookup, mutable vectorcall,
  recursion / ownership semantics, or repeated benchmark performance.
