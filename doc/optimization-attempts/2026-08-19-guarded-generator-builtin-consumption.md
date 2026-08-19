---
title: "Guarded Generator Builtin Consumption"
---

# Guarded generator builtin consumption

- Status: **IN PROGRESS; TWO INDEPENDENT GENUINE UNCHANGED-PRODUCTION
  STOCK-VS-SOAC SEMANTIC AND EXPORTED-VECTORCALL STRUCTURED REDS
  CONFIRMED; EXACT TWO-FILE IMPLEMENTATION COMPILES; BOTH ACTUAL
  STOCK-PARITY INTEGRATION AND EXPORTED-VECTORCALL REGRESSION
  RED-TO-GREEN; HASH-FRAGILE EXISTING DICTIONARY TEST PASSES ISOLATED;
  ALL THREE REENTRANT-OWNER, LIVE-RUNTIME-GLOBALS, AND SHORT-CAPSULE
  SAFETY FIXES IMPLEMENTED; EXPANDED REAL ADVERSARIAL INTEGRATION GREEN
  1 / 1 IN 5.80 SECONDS; FRESH FINAL POST-FIX JIT LIBRARY GREEN
  569 / 569; BROAD TRANSFORMED COMPATIBILITY GREEN 71 / 71 IN
  35.48 SECONDS; SCOPED FORMAT / TEST-TARGET CHECK GREEN; FINAL
  POST-FORMAT EXPANDED INTEGRATION GREEN 1 / 1 IN 5.57 SECONDS;
  FIXED-EIGHT RELEASE SMOKE GREEN WITH EXACT NATIVE INVARIANCE;
  CLEAN TARGETED COMPREHENSIONS IMPROVES 1.112864X BUT RICHARDS
  REGRESSES BEFORE SAME-STRATEGY ARGUMENT-SHAPE REFINEMENT;
  REFINEMENT FOCUSED RUST 1 / 1 AND TRANSFORMED 6 / 6 PASS;
  POST-REFINEMENT FIXED-EIGHT SMOKE PASSES 8 / 8 WITH EXACT NATIVE
  INVARIANCE; FINAL CLEAN REPEAT IMPROVES COMPREHENSIONS 1.112631X
  AND RECOVERS RICHARDS; MATCHED ZERO-LOSS PROFILES COMPLETE;
  LANDED CANDIDATE / RETAIN; AUTHORITATIVE FULL GATE PASSES
  1,229 PYTHON NODEIDS / 92 BATCHES AND ALL RUST SUITES**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`wnlnpkrp`**, commit
  **`2bb19f4f`**.
- Candidate revision: change **`lnxvnnml`**; most recently recorded
  pre-document-snapshot commit **`52888f30`**. The commit identity changes
  when the completed documentation is resnapshotted.
- Outcome: determine whether canonical builtin `any` / `all` can consume
  a trusted SOAC source-generator through its existing compiled resume
  entry without creating a new runtime API, changing native code shape,
  or losing CPython-visible iteration, generator, and mutation semantics.

## Hypothesis and evidence

- General-purpose opportunity: source generator expressions consumed by
  canonical `any` / `all` currently pass through interpreted
  `ClosureGenerator.__next__` / `send`, even when the actual source-owned
  resume body is already compiled. Runtime source helpers intentionally
  keep standard CPython vectorcall, so generic iteration repeatedly pays
  the Python helper bridge rather than using the existing compiled child.
- Fresh retained comprehensions profile contains **692 raw samples with
  zero loss**. Builtin `any` is **18.492% inclusive** and
  `slot_tp_iternext` **17.047% inclusive**; excluding sampled GC / kernel
  ancestry gives approximately **17.480%** and **16.034%** productive
  ancestry. The actual generator-expression body remains necessary work:
  **7.799% inclusive / 4.616% self**. These are nested, overlapping
  inclusive stack shares, not independent savings or a promised speedup.
  Existing generator execution, result truth testing, ownership, and
  source-body work must remain; only removable bridge work is in scope.
- The current retained normal fixed-eight stock/SOAC score is
  **0.5896760656259606x**; its matched targeted fixed-four stock score is
  **0.43858930692865516x**. These scores do not satisfy the authoritative
  full-suite **1.10x stock** goal.
- Retained Apply coverage is **23,188,640 native bytes / 1,527,950 machine
  blocks** and **2,866 optimized typed blocks / 204 functions**. The
  intended existing-helper fast path should preserve compiled body
  inventories and native shapes, but candidate invariance has **not** yet
  been measured.
- The actual patched guest interpreter exports existing
  **`PyType_GetSoacMetadata`**, independently verified by `nm`; no custom
  interpreter rebuild or newly exported symbol is proposed.
- An artificial SOAC `_reraise_control_flow` path incorrectly invokes a
  fake `asyncio.CancelledError.__instancecheck__` while canonical builtin
  iteration consumes exact `StopIteration`. A genuine unchanged-production
  stock/SOAC regression now confirms this is an existing **CPython-visible
  correctness bug**; the initial candidate fixes the focused stock-parity
  regression, and the strengthened post-safety-fix transformed
  integration now passes.
- A reviewer initially saved a **632-line transformed integration
  fixture**, verifying its Python AST on the host; it has since grown to
  a frozen, host-AST-clean **705-line adversarial fixture**. Its first
  unchanged-production
  focused attempt reaches the real fake-`asyncio` comparison but also
  discovers an **unrelated preexisting SOAC yielded-item lifetime
  difference**: a false yielded item is destroyed exactly once, while the
  short-circuit terminal true item may remain retained even after GC,
  contrary to the fixture's initial stock-like lifetime assumption. The
  fixture is being narrowed to preserve actual prior SOAC terminal
  ownership and require at-most-once destruction before asserting the
  intended compatibility difference. This first attempt is **not yet a
  clean targeted RED** at that historical stage; independent architectural
  review finds the wider Profile / Verify / Apply controls otherwise
  sound. The corrected later run below provides the genuine target RED.
- A second fixture precondition independently exposes another **unrelated
  preexisting baseline incompatibility**: rebinding `builtins.any` or
  `builtins.all` after transformed-module load does not invoke the
  replacement callback because lowered runtime builtins are already
  statically resolved. The reviewer replaces that invalid assumption
  with **`consume_dynamic(consumer, values)`**, which receives the real
  replacement callable and therefore exercises the required unchanged
  generic fallback; module-level shadows remain a conservative fallback
  case. Neither this existing rebinding difference nor terminal-item
  retention is the proposed optimization's target; both controls pass in
  the final focused target RED.
- A third unrelated preexisting mismatch affects **code-local
  `sys.monitoring`**: enabling `PY_START` on the transformed source
  parent's `consume_any.__code__` currently emits no callback under the
  existing JIT. The reviewer therefore keeps parent-local monitoring as
  a nonasserting compatibility control and instead requires observable
  local callbacks on the actual runtime generator method / helper code.
  This preserves the meaningful monitoring guard without asserting a
  callback the unchanged production baseline never provided. This control
  passes in the initial focused target RED before candidate production
  changes.
- The corrected real transformed regression
  **`just pytest-fast tests/test_guarded_generator_builtin_consumption.py
  -q`** now produces a genuine unchanged-production **0 passed / 1 failed
  in 5.84 seconds**, failing **only** at its final stock/SOAC comparison.
  Stock produces **`checks=[]`**, `any=False`, and `all=True`; SOAC
  instead produces **`checks=['StopIteration', 'StopIteration']`** and
  two **`RuntimeError('unexpected asyncio cancellation check')`** errors.
  Every Profile / Verify / Apply subprocess succeeds beforehand: real
  source-generator watcher creation remains **2 / 2 in each mode**,
  **40 profiling counters** and compiled parent/child bodies are proven,
  and captured values, short-circuiting, single iterator acquisition,
  finalizers, dynamic replacement callbacks, runtime closed/resume/
  reraise mutations, local/global monitoring, tracing/profiling,
  interpreted fallback, class iterator/next/send and `__bool__` mutation,
  subclasses, and `send.__code__` controls all pass. The error is therefore
  an isolated genuine user-visible compatibility defect rather than an
  invalid ownership, builtin rebinding, or parent-monitor assumption.
- A second independent structured Rust regression executes the **actual
  existing exported `dp_jit_py_vectorcall`** with real builtin `any` /
  `all` and real Python generator expressions. Existing results are
  correct, and unrelated `len` / wrong-arity fallback controls pass;
  the only failure is the intentional final production-selector mismatch
  **`(None, None) != (Some(Any), Some(All))`**. Cargo genuinely reports
  **`running 1 test`**, one failure, and exit **101**; the shared mutex
  is released before the assertion. An earlier invalid combination of a
  substring filter with `--exact` ran **zero tests** and was explicitly
  **not counted** as RED; an invalid Lima `--workdir` invocation was
  discarded. The rerun is the actual unchanged-behavior structured RED.

## Implementation and compatibility

- Production implementation scope: exactly two existing files,
  `crates/soac_jit/src/lib.rs` and
  `crates/soac_jit/src/jit/specialized_helpers.rs`. Reuse the existing
  generic vectorcall path, existing generator resume entry, existing
  constructor-template ownership, and the pinned interpreter metadata
  accessor; add **no public API, global state, runtime-helper inventory,
  IR concept, or new generated-native shape**.
- The complete two-file source implementation now **compiles**, and its
  genuine structured existing-exported-vectorcall regression turns
  **RED-to-GREEN: 1 focused test passed**. The build type-compiles all
  **569 JIT library tests**, but does not run the complete suite. The
  implementation uses a private owner-template cache and the actual
  running interpreter's existing `PyType_GetSoacMetadata`, a cheap exact
  builtin selector, live canonical class / method / helper / builtin
  guards, one iterator acquisition and one captured `tp_iternext`, the
  existing direct compiled resume, and an exact `StopIteration` boundary
  restoring stock cancellation-check parity. Canonical builtin
  `any` / `all` are selected, while unrelated `len` and wrong-arity calls
  retain their existing fallback. The frozen real transformed
  stock/SOAC fake-`asyncio` regression also turns genuine
  **RED-to-GREEN: 1 passed in 8.63 seconds**, across actual Profile,
  Verify, and Apply. Candidate `checks=[]`, `any=False`, and `all=True`
  now exactly match stock, whereas unchanged production checked
  `StopIteration` twice and raised two `RuntimeError`s. Real source
  generator watcher events remain intact; every frozen dynamic-callback,
  helper/class/code and mid-iteration mutation, monitoring, tracing,
  profiling, force-interpreter, iterator, and finalizer control passes.
  These two focused GREEN results do not establish whole-suite safety:
  the first complete JIT run encounters one existing GENERAL-dictionary
  collision observer expecting **`[false]`** but receiving
  **`[false, false]`**. Its assertion panics while holding the shared
  Python test mutex, poisoning that process and producing approximately
  **109 secondary failures**; these are not independent bugs. The same
  unchanged collision test now passes **1 / 1 in a fresh isolated
  process**. Pinned CPython `dictobject.c` uses randomized open-address
  probing that may legally call a colliding key's `__eq__` twice;
  demanding exactly one callback is process/hash fragile. The test does
  not dispatch through `any` / `all`, so the earlier failure is not
  evidence of a generator fast-path observer regression.
  During the later first authoritative full-gate attempt, the same
  preexisting embedded regression
  **`runtime_module_lookup_preserves_general_dict_collision_identity_and_error_suppression`**
  again receives legitimate **`[false, false]`** instead of its brittle
  exact **`[false]`** expectation; the shared Python mutex is poisoned,
  yielding **113 failures total, 112 secondary**. No production runtime
  behavior changes. Root durably fixes only the existing `#[cfg(test)]`
  body in `crates/soac_jit/src/function_instantiation.rs`: both GENERAL-
  dictionary and dictionary-subclass cases must observe a **nonempty
  sequence whose every identity is false**, allowing repeated CPython
  open-address probes while preserving strict fresh-key identity and
  exception-suppression checks. Independent source review is clean; the
  exact previously failing focused Rust test now passes **1 / 1**, and
  the package is formatted. This adds a third existing Rust file only
  for test code; the same **two runtime production files remain
  unchanged**. The corrected authoritative full-gate retry subsequently
  passes with **1,229 Python nodeids / 92 isolated batches** and all
  workspace Rust suites.
  Independently, reentrant Python callbacks can replace/delete
  `_resume_function`, `_preserved_values`, or mutable runtime
  `NO_DEFAULT` during direct resume, risking freed `FunctionEnv`, capsule,
  or sentinel. The owner has now implemented **strong INCREF pinning for
  all three original Python call arguments across the entire direct native
  resume, with balanced ordered cleanup**, matching the existing
  interpreted `send` lifetime. A fresh complete
  **`cargo test -p soac_jit --lib` genuinely passes 569 / 569 tests**
  after these three ownership pins, and the unrelated collision test also
  passes **1 / 1** in isolation. Independent review nevertheless finds a
  **second object-lifetime concern on a non-`StopIteration` body error**:
  the generator body or a reentrant finalizer may mutate/promote
  runtime globals, freeing the cached **`prepared.runtime_values`** before
  the exception path dereferences its cached `_reraise_control_flow`
  helper slot. The owner has now implemented safe existing **live
  globals / builtins lookup** to reload `_reraise_control_flow` after
  possible promotion, without dereferencing stale cached values or
  invoking generator resume twice. A **third independent preserved-state
  length concern** exists because
  public generator **`_preserved_values`** can be replaced with a
  different genuine `soac.PreservedState` capsule that has fewer slots.
  Validating the expected source layout does not validate the replacement
  capsule's actual slot count, so raw
  **`preserved_values_ptr + closed_index`** can read past the available
  capsule slots. The
  approved two-file implementation now uses the existing
  length-checked **`preserved_state::load_preserved_state_owned`** and
  normal truthiness behavior; no third production file is required.
  All three safety corrections are implemented in the same approved
  two files. The initial **569 / 569** JIT run predates the latter two
  fixes; a separate fresh final complete run now genuinely passes
  **569 / 569 after all three corrections**. The reviewer has frozen the
  **705-line host-AST-clean
  integration** with three narrow genuine adversarial additions: a real
  **zero-length, same-name `soac.PreservedState` capsule** must raise the
  existing bounded `RuntimeError`; an active generator callback replaces
  `_resume_function`, `_preserved_values`, and mutable runtime
  `NO_DEFAULT` while still yielding true; and a `ValueError` callback
  promotes runtime globals and replaces `_reraise_control_flow`, proving
  the newly installed live helper is invoked. All modified globals are
  restored. The full frozen **705-line transformed integration genuinely
  passes 1 / 1 in 5.80 seconds across Profile / Verify / Apply**,
  including every new concrete safety control, original source-generator
  watcher events, stock fake-`asyncio` parity, class/method mutation,
  and monitoring. The distinct fresh final
  **`cargo test -p soac_jit --lib`** also passes **569 / 569 after all
  fixes**. The broader existing transformed generator/runtime
  compatibility matrix genuinely passes **71 / 71 in 35.48 seconds**.
  Root subsequently runs package-scoped **`just fmt-rust soac_jit`** and
  **`just fmt-rust-check soac_jit`**, both successfully; aligned
  **`cargo check -p soac_jit --tests`** passes in **10.05 seconds**.
  The final post-format expanded Profile / Verify / Apply integration
  passes **1 / 1 in 5.57 seconds** (**5.832 seconds total pytest time**)
  after a **26.46-second debug-extension restage**, which is workflow
  overhead, not runtime performance. Release debug-single fixed-eight
  smoke candidate comparison **125205** against retained comparison
  **112443** passes **8 / 8**, with independently verified exact
  generated-code/function coverage invariance and zero worker errors.
  Cold smoke timings do not measure throughput. The normally sampled
  fixed-eight comparison subsequently completes with a supported
  comprehensions improvement and a possible deltablue regression;
  Repeated comparisons, causal profiles, and the corrected authoritative
  full gate subsequently complete successfully. Potentially expensive
  per-yield guards require the disclosed repeated guardrail evidence;
  the final candidate is retained.
- A constructor-template-owned canonical-owner guard may recognize only
  trusted live canonical builtin `any` / `all` and the exact compiler-owned
  source-generator class / currently valid compiled resume metadata.
  Dispatch must remain a real ordinary CPython callable / generator
  operation; it must not replace a generator with a capsule or bypass
  Python-visible function or generator creation.
- Preserve the real source-backed generator-expression `PyFunction`, its
  watcher CREATE event, closure cells, distinct generator object, source
  identity, laziness, `send` / `throw` / `close`, and finalizers. The prior
  eager-comprehension callable-elision optimization explicitly does **not**
  apply to lazy source generator expressions.
- Match CPython builtin iteration precisely: evaluate and fetch the
  iterator once, capture the effective `tp_iternext` once where CPython
  does, test truthiness with the same callbacks and exception propagation,
  preserve actual existing ownership and DECREF behavior, short-circuit
  at the identical element, and stop only at the canonical iterator-end
  condition. In particular, do not assume the prior SOAC terminal true
  item is destroyed immediately: preserve its existing retained lifetime
  and require at-most-once finalization. Never remove necessary
  generator-body or refcount work. The candidate now holds owned strong
  references to all
  three live borrowed arguments, `_resume_function`, `_preserved_values`,
  and mutable runtime `NO_DEFAULT`, for the complete direct-resume call:
  callbacks can replace/delete generator fields or the runtime sentinel
  before the compiled `FunctionEnv` / state capsule returns, then
  releases all three with balanced ordered cleanup. The complete JIT
  library passes **569 / 569** after this first correction but before
  the next two. The distinct non-`StopIteration` error path now safely
  reloads `_reraise_control_flow` through existing current live globals /
  builtins lookup after possible reentrant dictionary promotion, never
  dereferencing stale cached `prepared.runtime_values` or resuming the
  generator twice. This second source correction is implemented but not
  yet revalidated.
  Separately, never infer actual preserved-state capacity from expected
  source layout: public `_preserved_values` may reference another valid,
  shorter SOAC capsule. Read its closed-state slot only through existing
  **`preserved_state::load_preserved_state_owned`** with real bounds and
  truthiness checks, never unchecked pointer arithmetic; this third
  preserved-state bounds correction is also implemented within the same
  two
  files. The strengthened real transformed adversarial integration passes
  **1 / 1 in 5.80 seconds**, and the separate fresh post-fix complete
  JIT library passes **569 / 569**. Broad existing transformed
  generator/runtime compatibility also passes **71 / 71 in 35.48
  seconds**; scoped formatting and its check pass, aligned JIT test-
  target checking passes in **10.05 seconds**, and the final post-format
  expanded transformed integration passes **1 / 1 in 5.57 seconds**.
- Revalidate all mutable assumptions at use: exact owner/type/version,
  current generator class and method/code/helper identity, canonical live
  builtin identity, source/child tracing, profiling, local/global
  monitoring, interpreted-force switches, reentry, and constructor / state
  mutation. Class replacement, custom iterator or next behavior,
  noncanonical builtins, changed helpers, changed source code, observers,
  or unverified ownership must use the untouched original slow path.
  Because the existing lowerer statically resolves canonical builtin
  names, prove replacement-callable fallback through explicit dynamic
  **`consume_dynamic(consumer, values)`** rather than incorrectly
  assuming post-load `builtins.any` / `builtins.all` rebinding is already
  observed; treat module shadows conservatively. Similarly, retain
  transformed-parent code-local monitoring as a nonasserting existing-
  behavior control; assert actual local-monitor callbacks on runtime
  generator method / helper code instead.
- An exact normal `StopIteration` fast boundary may avoid SOAC's
  artificial `_reraise_control_flow`; the genuine stock/SOAC RED now
  proves the compatibility difference. Other exceptions and all uncertain
  conditions retain the live existing helper and its observable behavior;
  no speculative subclass, hook, or cancellation shortcut is acceptable.
- Focused unchanged-production semantic integration RED is confirmed
  **0 passed / 1 failed in 5.84 seconds**, with all other real transformed
  controls passing. An independent actual exported-vectorcall Rust
  eligibility regression also genuinely runs **one test / one failure /
  exit 101**, while ordinary builtin/generator behavior and unrelated
  controls pass. The exact two-file candidate was saved only after both
  genuine unchanged-production REDs, now compiles, and turns both the
  actual exported-vectorcall structured regression **GREEN 1 / 1** and
  the independent frozen stock/SOAC transformed semantic integration
  **GREEN 1 / 1 in 8.63 seconds**. The first complete JIT run then hits
  a hash-fragile unrelated existing GENERAL-dictionary collision-count
  assertion and approximately 109 secondary shared-mutex poison
  failures; the same isolated test now passes **1 / 1**. The owner has
  implemented balanced strong pinning of all three direct-resume
  arguments to address the first independently identified reentrant
  object-lifetime concern. The fresh complete JIT library now passes
  **569 / 569**, but review discovers a second stale runtime-values
  concern on non-
  `StopIteration` errors; safe existing live globals/builtins lookup is
  now implemented. A third independently identified shorter replacement
  `soac.PreservedState` capsule permits reading a closed-state index
  beyond its available slots; the existing length-checked owned state
  accessor now
  replaces unchecked pointer arithmetic. The earlier **569 / 569** suite
  predates those two additional fixes; a **fresh final post-fix JIT
  library separately passes 569 / 569**. The frozen expanded
  **705-line** transformed adversarial integration now genuinely passes
  **1 / 1 in 5.80 seconds**, including all three safety controls and
  prior source watcher / stock fake-asyncio / monitor behavior. Broad
  existing transformed runtime/generator compatibility now passes
  **71 / 71 in 35.48 seconds**. Package-scoped formatting / format check,
  the aligned **10.05-second** JIT test-target check, and final post-
  format expanded transformed integration **1 / 1 in 5.57 seconds** all
  pass. Candidate fixed-eight release smoke passes **8 / 8** with exact
  native/function invariance, and the normal fixed-eight comparison
  completes with unchanged native code; repeated guardrail comparisons,
  matched causal profiles, and the corrected authoritative full gate
  subsequently pass.

## Benchmark protocol and coverage

- Fixed normal benchmark selection: `chaos`, `comprehensions`,
  `deltablue`, `fannkuch`, `float`, `nbody`, `richards`, and
  `spectral_norm`, against the same vendored stock CPython and the
  integrated eager-comprehension callable-elision revision.
- Initial baseline: retained fixed-eight stock score
  **0.5896760656259606x**; retained matched targeted fixed-four stock
  score **0.43858930692865516x**. Candidate normal fixed-eight
  comparison **125415-rBAF9T** is complete; matched three-round
  targeted comparison **125840-kMbAuX** was discarded in full after
  concurrent-worker contamination, and wholly restarted clean comparison
  **130328-CpSpU4** is valid pre-partition historical evidence. Final
  post-partition comparison **132104** is the authoritative clean
  repeated candidate result.
- Initial fixed-eight release **debug-single smoke comparison 125205**
  versus retained smoke **112443** passes **8 / 8**. Independent auditing
  of every measured Apply worker PID and complete function/adapter row
  confirms identical `(function id, qualname, entry kind, native bytes,
  machine blocks)` across all workloads; only process-specific PID /
  `code_id` differs. Native code remains exactly **2,242,168 bytes /
  148,116 machine blocks**, with unchanged **2,866 optimized typed
  blocks / 204 functions**, zero `ERROR` / `CRITICAL` events, and source
  bodies chaos **32**, comprehensions **24**, deltablue **76**,
  fannkuch **1**, float **7**, nbody **6**, richards **51**, and
  spectral_norm **7**. The apparent cold smoke arithmetic **1.11x** is
  invalid as throughput evidence; no speedup is established.
- After the bounded same-strategy selector partition, a **new
  post-refinement release debug-single comparison 131641** completes
  **8 / 8**, matching both retained baseline **112443** and pre-partition
  candidate **125205**. Independent measured Apply worker-PID auditing
  confirms every full function/adapter row has the same function ID,
  qualified name, native bytes, and machine-block count; zero errors are
  reported. Native code remains exactly **2,242,168 bytes / 148,116
  machine blocks**, typed coverage **2,866 blocks / 204 functions**, and
  all eight source-body inventories remain unchanged. This confirms
  structural invariance only; cold smoke timings are invalid. A
  subsequent normally sampled post-partition comparison **131748** is
  complete; final repeated comparison **132104** and matched causal
  profiles are also complete, and the corrected authoritative full gate
  passes all **92 isolated Python batches** and Rust suites.
- The **post-partition normally sampled fixed-eight comparison 131748**
  completes **8 / 8**, with official stock score
  **0.6326613107877241x** and official previous-SOAC arithmetic
  **1.0612781659923773x**. Arithmetic means remain vulnerable to old
  baseline outliers; robust full-eight geometry against retained
  **112949** is **0.999207x raw / 1.008631x stock-adjusted**, while the
  direct comparison with immediate unpartitioned candidate **125415**
  is **0.987636x raw / 1.017527x stock-adjusted**. These noisy complete-
  suite results are not proof of a uniform improvement.
- Against retained production, comprehensions improves
  **51.095554 -> 45.587036 us**, raw **1.120835x
  [1.082428, 1.168494]** and stock-adjusted **1.074564x
  [1.035752, 1.165127]**. Retained Richards changes
  **30.302235 -> 30.027457 ms (1.009151x raw / 0.987162x paired)**;
  its raw confidence interval crosses one. Deltablue remains a possible
  regression at **0.972967x [0.917417, 0.988302] / 0.963141x paired
  [0.890059, 0.996543]** and requires final repeated guardrails.
- Against immediate pre-partition candidate **125415**, Richards
  recovers **31.108640 -> 30.027457 ms**, raw **1.036006x
  [1.013553, 1.178287]** and paired **1.048397x
  [1.018914, 1.195408]**, supporting removal of unrelated-call selector
  overhead. Comprehensions versus that candidate is **0.981261x raw /
  0.999125x paired**, with confidence intervals crossing one; deltablue
  is neutral at **0.995438x**. Every one of **80 measured Apply worker
  PIDs** retains every function/adapter row, exactly
  **23,188,640 native bytes / 1,527,950 machine blocks**, and
  **2,866 typed blocks / 204 functions**, with zero errors. Final
  post-partition three-round comparison **132104** independently
  confirms the target improvement and Richards recovery.
- Normally sampled candidate fixed-eight comparison
  **`comparison-20260819-125415-rBAF9T`** versus retained
  **`comparison-20260819-112949-9UMVhs`** completes **8 / 8**. Its
  official stock/SOAC score is **0.601574599529184x**, versus retained
  **0.5896760656259606x**. Official previous-SOAC arithmetic
  **1.0702651354606592x** is skewed by baseline outliers and is not proof
  of a full-suite improvement. Robust fixed-eight previous-SOAC geometry
  is **1.011716x raw / 0.991257x stock-adjusted**.
- Primary target comprehensions median improves
  **51.095554 -> 44.732796 us**, raw **1.142239x** with worker-cluster
  **95% interval [1.099774, 1.191959]**, and **1.075505x stock-adjusted
  [1.027680, 1.164908]**. Matched stock itself improves
  **8.414 -> 7.923 us**, so adjustment is material. Chaos remains
  neutral at **1.001140x [0.968151, 1.051244] / 1.005750x paired**.
  Deltablue is a **possible regression**, raw **0.977426x
  [0.959656, 0.995174]** and paired **0.962409x
  [0.923810, 0.999478]**. Richards is noisy at **0.974078x
  [0.858533, 1.144212] / 0.941592x paired**; do not infer a causal
  change from this single round.
- Independent normal-mode auditing verifies every function/adapter row
  across all **80 measured Apply worker PIDs** is unchanged, with zero
  `ERROR` / `CRITICAL` events. Total generated native code remains
  **23,188,640 bytes / 1,527,950 machine blocks**, optimized typed
  coverage **2,866 blocks / 204 functions**, and all benchmark/body
  inventories are retained. Repeated targeted guardrails and matched
  profiles are required before deciding whether the possible deltablue
  decline reproduces.
- **Targeted measurement workflow incident:** the initial candidate
  artifact **`comparison-20260819-125840-kMbAuX`** is **discarded in its
  entirety**. At **2026-08-19 13:01:36.648 PDT**, a newly delegated
  supposedly host-only subagent misread inherited context and briefly
  launched overlapping stock-only comparison
  **`comparison-20260819-130136-VxkFeh`**. Its stock workers overlap the
  initial candidate's round-03 Apply workers; the concurrently written
  shared `tee` log also contains approximately **41,285 NUL bytes**.
  Separate benchmark JSON exists but cannot make the contaminated
  three-round comparison valid. Root stops the additional job and
  restarts all candidate rounds from scratch as
  **`comparison-20260819-130328-CpSpU4`** against retained targeted
  comparison **113536**, under exclusive guest ownership. No results
  from the discarded run are eligible for a headline, guardrail verdict,
  or retain decision. The wholly clean replacement, matched causal
  profiles, and corrected full correctness gate all subsequently
  complete; the contaminated artifact remains discarded.
- Wholly clean targeted comparison
  **`comparison-20260819-130328-CpSpU4`** versus retained comparison
  **113536** completes **three rounds / 30 workers / 60 measurements per
  workload**, with **10,000 round-stratified worker-cluster bootstrap
  draws**. Primary target comprehensions median improves
  **49.926194 -> 44.862793 us**, raw **1.112864x
  [1.104525, 1.148854]** and stock-adjusted **1.123740x
  [1.111642, 1.166810]**. Chaos remains neutral at **1.001430x
  [0.983392, 1.010731] / 0.991476x paired
  [0.973044, 1.006651]**.
- Deltablue has a mild raw decline **0.984845x
  [0.965023, 0.998362]**, but paired **0.986022x
  [0.962534, 1.006112]** crosses neutrality. Richards has a **genuine
  reproduced regression**, raw **0.978051x [0.968505, 0.988798]** and
  stock-adjusted **0.962661x [0.945508, 0.976625]**, approximately
  **2.2% to 3.7% slower**; it must not be dismissed as noise and requires
  dispatch-path investigation. Four-workload robust geometry remains
  favorable at **1.017884x raw / 1.014091x stock-adjusted**. Official
  subset stock geometry is **0.44513685009055015x**, versus retained
  **0.43858930692865516x**; official previous-SOAC geometry is
  **1.0090672927925823x** and does not replace robust worker estimates.
- Every one of the **120 measured Apply worker PIDs** preserves its
  complete function/adapter rows with zero errors. Each targeted round
  retains exactly **18,255,240 native bytes / 1,201,600 machine blocks**
  and **2,265 typed blocks / 183 functions**. The fixed-eight baseline and
  candidate independently retain **23,188,640 bytes / 1,527,950 blocks**
  and **2,866 typed blocks / 204 functions**; subset and full-suite
  coverage counts must not be conflated. Genuine CPython-visible
  correctness improvement, significant target gains, favorable subset
  geometry, and unchanged code support **RETAIN CANDIDATE** status, but
  Richards' reproduced decline motivates the same-strategy argument-
  shape refinement described below. All normal and repeated results
  above are valid **pre-refinement historical evidence**, not measurements
  of the subsequently changed candidate. The final post-refinement
  comparisons, profiles, and corrected authoritative full gate below
  are complete.
- **Final post-partition comparison 132104 versus retained 113536**
  completes **three uncontended rounds / 30 workers / 60 values per
  workload**, with **10,000 round-stratified worker-cluster bootstrap
  draws**. Comprehensions improves **49.926194 -> 44.872185 us**,
  **1.112631x [1.096347, 1.139781]**, or **1.123761x stock-adjusted
  [1.105023, 1.153696]**. Chaos is **1.016804x
  [1.002464, 1.025268] / 1.014166x paired**, with the paired interval
  crossing one. Deltablue is **0.990545x [0.976398, 1.004386] /
  1.013556x paired**, with both intervals crossing one. Richards is
  raw-neutral **0.997661x [0.984818, 1.003284]**, but retains a
  disclosed paired decline **0.986296x [0.967541, 0.993720]**,
  approximately **1.37%**.
- Versus the immediate unpartitioned candidate, Richards improves
  **1.020050x [1.004693, 1.028789] / 1.024551x paired
  [1.005389, 1.036650]**; target comprehensions is neutral at
  **0.999791x**, and deltablue is **1.005788x / 1.027925x paired**.
  Final subset robust geometry is **1.028280x / 1.033141x paired**
  against retained production, or **1.010214x / 1.018785x paired**
  against the unpartitioned candidate. Official subset stock geometry is
  **0.44758856139159614x**, versus retained **0.43858930692865516x**;
  official previous **1.0112507283090535x** is affected by giant
  outliers and does not replace robust estimates. All **120 final Apply
  PIDs** retain every function/adapter row and exactly **18,255,240
  native bytes / 1,201,600 blocks / 2,265 typed blocks / 183 functions
  per round**, with zero errors.
- Final matched zero-loss comprehensions profiles contain
  **692 retained -> 547 unpartitioned -> 570 partitioned samples**.
  Builtin `any` ancestry is **18.492% -> 0% -> 0%**, and iterator-slot
  ancestry **17.047% -> 0% -> 0%**. The real source-generator body
  remains **7.799% / 4.616% inclusive/self -> 8.408% / 5.116% ->
  7.897% / 3.864%**. Final guarded-consumer ancestry is **13.159%**;
  canonical-guard inclusive/self falls from **3.656% / 1.462%** to
  **1.579% / 0.527%**. Existing vectorcall self changes
  **1.878% -> 2.741% -> 2.107%**. Inclusive frames are nested,
  normalized, and noisy; never add their shares.
- The valid immediate same-strategy Richards zero-loss profile contains
  **432 unpartitioned -> 568 partitioned samples**, with existing
  generic-vectorcall inclusive/self **13.89% / 5.09% -> 10.56% /
  3.35%**. Inlining and sampling variability limit attribution, but the
  mechanism supports the significant measured Richards recovery. This
  matched immediate comparison is not the earlier nonmatching historical
  **`ccef62b6`** reference.
- Matched pre-refinement comprehensions profiles are both zero-loss,
  with **692 retained -> 547 candidate raw samples**. Builtin
  `any` ancestry decreases **18.492% -> 0%** and `slot_tp_iternext`
  **17.047% -> 0%**. Replacement guarded-consumer ancestry is
  **13.528% inclusive / 0.548% self**; its canonical guard is
  **3.656% inclusive / 1.462% self**. These frames are nested and must
  **not** be added. The preserved source generator-expression body
  remains **7.799% / 4.616% inclusive/self -> 8.408% / 5.116%**.
  Function-factory shares change **24.129% -> 15.905%**, but profiles
  are normalized and kernel page clearing independently changes
  **12.703% -> 4.934%**; do not attribute every share difference to the
  optimization.
- The candidate Richards profile has **432 zero-loss samples**, with
  existing `py_vectorcall` **13.89% inclusive / 5.09% self**. An older
  profile has **599 samples / 8.35% inclusive / 2.50% self**, but it
  belongs to historical commit **`ccef62b6`**, not current retained
  commit **`2bb19f4f`**. The comparison is therefore **suggestive only,
  not matched causal evidence**; it cannot establish that dispatch is
  responsible for the measured Richards decline.
- Target and guardrails: comprehensions canonical `any` / `all` source
  generators, with independent `chaos`, `deltablue`, and `richards`
  controls. Require robust worker-cluster medians / uncertainty, paired
  stock adjustment, and matched source-backed zero-loss profiles before
  attributing a throughput effect.
- Baseline profile: retained comprehensions **692 raw zero-loss
  samples**, unpartitioned candidate **547**, and final candidate
  **570**. Immediate Richards profiles contain **432 unpartitioned /
  568 final samples**; the unrelated **599-sample** older `ccef62b6`
  profile is a different revision and not causal. Final smoke,
  normal/repeated comparisons, and matched profiles are complete; the
  authoritative full gate also passes after the narrow existing-test
  correction.
- Module selection, benchmark/dependency and standard-library transform
  coverage, measured worker PID/body inventories, source-generator resume
  reachability, startup/compilation costs, and candidate per-function
  native evidence must be audited independently; completion alone does
  not establish hot-path JIT coverage.

## Measurements

| Metric | Integrated retained baseline | Candidate | Status |
| --- | --- | --- | --- |
| Pre-refinement fixed-eight stock / SOAC geometry | 0.5896760656259606x | 0.601574599529184x | historical candidate before argument-shape refinement; full-suite 1.10x stock goal unmet |
| Post-partition fixed-eight stock / official previous-SOAC geometry | retained 0.5896760656259606x | 0.6326613107877241x / 1.0612781659923773x | official means vulnerable to earlier baseline outliers; full-suite 1.10x stock goal unmet |
| Post-partition fixed-eight robust / stock-adjusted geometry versus retained / prior candidate | retained 112949; unpartitioned 125415 | retained 0.999207x / 1.008631x; immediate 0.987636x / 1.017527x | complete-suite means affected by outliers/noise; repeated comparison required |
| Post-partition normal comprehensions median / raw / stock-adjusted versus retained | 51.095554 us | 45.587036 us; 1.120835x / 1.074564x | raw CI [1.082428,1.168494]; paired CI [1.035752,1.165127] |
| Post-partition normal richards versus immediate unpartitioned candidate | 31.108640 ms | 30.027457 ms; 1.036006x / 1.048397x paired | raw CI [1.013553,1.178287]; paired CI [1.018914,1.195408]; selector-partition recovery |
| Post-partition normal deltablue raw / stock-adjusted versus retained | retained 112949 | 0.972967x / 0.963141x | raw CI [0.917417,0.988302]; paired CI [0.890059,0.996543]; possible regression needs repeated guardrails |
| Final post-partition targeted stock / official previous-SOAC geometry | retained targeted 0.43858930692865516x | 0.44758856139159614x / 1.0112507283090535x | official mean skewed by giant outliers; robust estimates authoritative |
| Final post-partition targeted comprehensions median / raw / stock-adjusted | 49.926194 us | 44.872185 us; 1.112631x / 1.123761x | 10,000 round-stratified cluster draws; raw CI [1.096347,1.139781]; paired CI [1.105023,1.153696] |
| Final post-partition targeted richards versus retained / unpartitioned | retained 113536; unpartitioned 130328 | retained 0.997661x / 0.986296x paired; unpartitioned 1.020050x / 1.024551x paired | raw retained neutral; paired residual -1.37%; immediate recovery CI [1.004693,1.028789], paired [1.005389,1.036650] |
| Final post-partition targeted chaos / deltablue raw / stock-adjusted | retained targeted 113536 | chaos 1.016804x / 1.014166x; delta 0.990545x / 1.013556x | chaos paired and deltablue confidence intervals include neutrality |
| Final post-partition subset robust / stock-adjusted versus retained / unpartitioned | retained 113536; unpartitioned 130328 | retained 1.028280x / 1.033141x; unpartitioned 1.010214x / 1.018785x | target preserved; Richards significantly recovered |
| Pre-refinement targeted fixed-four stock / SOAC geometry | 0.43858930692865516x | 0.44513685009055015x | historical three clean rounds before argument-shape refinement; subset only |
| Fixed-eight previous-SOAC official / robust / stock-adjusted geometry | retained `wnlnpkrp` | 1.0702651354606592x / 1.011716x / 0.991257x | historical pre-partition official arithmetic skewed by outliers; final repeated targeted confirmation completed |
| Normal comprehensions median / raw / stock-adjusted previous-SOAC | 51.095554 us | 44.732796 us; 1.142239x / 1.075505x | raw CI [1.099774,1.191959]; paired CI [1.027680,1.164908]; stock itself 8.414->7.923 us |
| Normal deltablue raw / stock-adjusted previous-SOAC | retained prior revision | 0.977426x / 0.962409x | raw CI [0.959656,0.995174]; paired CI [0.923810,0.999478]; possible regression requires repeated guardrails |
| First candidate targeted comparison 125840 | retained targeted 113536 | DISCARDED IN FULL | overlapping 130136 stock workers contaminate round-03 Apply; shared tee log contains approximately 41,285 NUL bytes |
| Restarted root-exclusive targeted comparison 130328 | retained targeted 113536 | COMPLETE; three rounds / 30 workers / 60 values per workload | entirely fresh uncontended comparison; no discarded-run evidence accepted |
| Clean targeted comprehensions median / raw / stock-adjusted improvement | 49.926194 us | 44.862793 us; 1.112864x / 1.123740x | 10,000 round-stratified cluster draws; raw CI [1.104525,1.148854]; paired CI [1.111642,1.166810] |
| Clean targeted deltablue raw / stock-adjusted | retained targeted prior revision | 0.984845x / 0.986022x | raw CI [0.965023,0.998362]; paired CI [0.962534,1.006112] crosses neutrality |
| Clean targeted richards raw / stock-adjusted | retained targeted prior revision | 0.978051x / 0.962661x | genuine 2.2-3.7% regression; raw CI [0.968505,0.988798]; paired CI [0.945508,0.976625] |
| Clean targeted subset robust / stock-adjusted / official previous-SOAC | retained targeted 113536 | 1.017884x / 1.014091x / 1.0090672927925823x | aggregate favorable but does not erase reproduced richards slowdown |
| Optimized typed-IR blocks / functions | 2,866 / 204 | 2,866 / 204 | all 80 normal-mode measured workers/body inventories unchanged |
| Pre-optimization BlockPy bytes | unavailable for this strategy | unavailable | no audited candidate BlockPy-size claim |
| Apply-mode native bytes / machine blocks | 23,188,640 / 1,527,950 | 23,188,640 / 1,527,950 | all 80 normal measured Apply PIDs/full function-adapter rows unchanged; zero ERROR/CRITICAL |
| Targeted per-round native bytes / machine blocks / typed blocks / functions | 18,255,240 / 1,201,600 / 2,265 / 183 | 18,255,240 / 1,201,600 / 2,265 / 183 | all 120 three-round Apply PIDs/full function-adapter rows exact; zero errors |
| Pre-refinement release debug-single fixed-eight native bytes / machine blocks | 2,242,168 / 148,116 | 2,242,168 / 148,116 | comparison 125205; all eight measured Apply PIDs/functions/adapters exact; zero ERROR/CRITICAL; cold timing invalid |
| Post-refinement release debug-single fixed-eight native bytes / machine blocks | 2,242,168 / 148,116 | 2,242,168 / 148,116 | comparison 131641 matches retained 112443 and prior 125205; every measured Apply PID/function/adapter row exact; cold timing invalid |
| Release debug-single direct source bodies chaos / comps / delta / fann / float / nbody / rich / spectral | 32 / 24 / 76 / 1 / 7 / 6 / 51 / 7 | 32 / 24 / 76 / 1 / 7 / 6 / 51 / 7 | every id/qualname/entry/bytes/blocks unchanged; only PID/code_id differs |
| Matched retained / unpartitioned / final comprehensions zero-loss samples | 692 | 547 / 570 | all three zero-loss profiles; real source generator preserved |
| Pre-refinement builtin `any` / `slot_tp_iternext` inclusive ancestry | 18.492% / 17.047% | 0% / 0% | existing interpreted bridge removed; nested profiles not additive |
| Pre-refinement guarded consumer inclusive / self | absent | 13.528% / 0.548% | includes source body and nested canonical guards; not independent overhead |
| Pre-refinement canonical consumer guard inclusive / self | absent | 3.656% / 1.462% | nested in guarded consumer; do not add inclusive shares |
| Final guarded consumer / canonical guard inclusive / self | unpartitioned guard 3.656% / 1.462% | consumer 13.159%; guard 1.579% / 0.527% | noisy nested shares; never add inclusive frames |
| Final preserved generator body inclusive / self | retained 7.799% / 4.616%; unpartitioned 8.408% / 5.116% | 7.897% / 3.864% | required real source generator body remains |
| Matched immediate Richards generic vectorcall inclusive / self | unpartitioned 13.89% / 5.09%; 432 zero-loss samples | final 10.56% / 3.35%; 568 zero-loss samples | valid immediate comparison; inlining/sampling limit attribution |
| Retained source genexpr body inclusive / self | 7.799% / 4.616% | 8.408% / 5.116% | required compiled source body remains; normalized sample shares vary |
| Pre-refinement Richards existing vectorcall inclusive / self | 8.35% / 2.50% on DIFFERENT old commit ccef62b6 | 13.89% / 5.09% over 432 zero-loss samples | old 599-sample reference is not current retained commit 2bb19f4f; suggestive only, never causal |
| Approximate productive builtin / iterator ancestry excluding GC and kernel | 17.480% / 16.034% | original builtin / iterator frames absent | nested inclusive ancestry included required work; final guarded consumer remains |
| Actual source generator-expression body inclusive / self | 7.799% / 4.616% | 7.897% / 3.864% | required source computation remains in the final zero-loss profile |
| Genuine initial stock/SOAC semantic integration RED-to-GREEN | 0 passed / 1 failed in 5.84 s | 1 passed in 8.63 s before later adversarial additions | actual Profile/Verify/Apply checks=[] / any=False / all=True matches stock; source watchers and prior mutation/monitor/iterator/finalizer controls pass |
| Genuine structured production-path Rust RED-to-GREEN | 1 real test failed; exit 101 | 1 focused test passed | actual exported dp_jit_py_vectorcall selects canonical any/all; len/wrong-arity fallback preserved |
| First complete JIT run / collision observer | old exact callback assertion assumes [false] | first full run gets [false,false]; isolated fresh rerun passes 1 / 1 | CPython open-address collision probing may legally compare twice; hash-fragile test unrelated to any/all; first panic causes approximately 109 secondary mutex-poison failures |
| First full-gate collision test / durable test-only correction | preexisting exact GENERAL [false] assertion | first gate fails 113 total / 112 secondary; corrected exact test passes 1 / 1 | both GENERAL and dict-subclass require nonempty all-false identities; strict fresh-key/error checks retained; third Rust file test-only |
| Initial complete JIT Rust library after three-argument ownership fix | retained main JIT suite passes | 569 / 569 passed | fresh process after unrelated poisoned first run; historical initial pass predates live-globals and bounded-capsule fixes |
| Final complete JIT Rust library after all three lifetime and bounds corrections | retained main JIT suite passes | 569 / 569 passed | separate genuinely fresh post-fix full library run; includes owner pins, live runtime-global reload, and bounded capsule access |
| Reentrant direct-resume owner lifetime | interpreted Python send strongly owns resume function, preserved state, and NO_DEFAULT | source fix pins all three with balanced cleanup | prevents callback freeing FunctionEnv/capsule/sentinel; full JIT library subsequently passes 569 / 569 |
| Non-StopIteration runtime-globals error-path lifetime | original interpreted send safely consults live runtime helpers | implemented safe live globals / builtins helper reload; adversarial integration passes | actual ValueError promotes globals and invokes replaced current helper; no stale prepared.runtime_values or double resume |
| Replacement preserved-state capsule actual bounds | expected source layout does not prove replacement capsule length | implemented existing length-checked preserved_state::load_preserved_state_owned; adversarial integration passes | actual zero-length same-name valid capsule raises existing bounded RuntimeError; no third production file |
| Strengthened real transformed adversarial integration | initial fixture 632 lines | frozen 705-line fixture passes 1 / 1 in 5.80 s | actual Profile/Verify/Apply validates zero-length capsule, triple owner mutation while yielding, promoted globals/new live helper, original watchers/parity/monitor controls |
| Broad transformed existing generator / runtime compatibility | retained main previously passed | 71 / 71 passed in 35.48 s | source generators, runtime helpers, mutation, callbacks, monitors, and ownership controls |
| Package-scoped JIT formatting / formatting check / test-target check | retained main previously passed | all passed; cargo test-target check 10.05 s | just fmt-rust soac_jit; just fmt-rust-check soac_jit; cargo check -p soac_jit --tests |
| Final post-format expanded transformed integration | initial focused semantic RED 1 failed | 1 passed in 5.57 s; pytest total 5.832 s | actual Profile/Verify/Apply, stock fake-asyncio parity, all three ownership/state/global controls; 26.46 s debug restage is workflow-only |
| Same-strategy argument-shape refinement focused validation | pre-refinement all selectors run before shape exclusion | structured Rust 1 / 1; transformed new parity + five retained StopIteration tests 6 / 6 in 11.34 s | exact existing next 1/2, StopIteration 2, any/all 1; preserve original nargsf and priority |
| Final post-refinement scoped format check / JIT test-target check | earlier full JIT library 569 / 569 before shape refinement | both pass; cargo check -p soac_jit --tests 2.76 s | just fmt-rust-check soac_jit passes; earlier complete 569-test execution predates final selector refinement |
| Post-refinement final repeated candidate / matched profiles | retained 113536; normal 131748 | final 132104 COMPLETE; comprehensions 692/547/570; Richards 432/568 samples | target 1.112631x; Richards recovers versus unpartitioned; paired retained residual -1.37% disclosed |
| Full `just test-all` correctness gate | retained main previously passed | PASS; 1,229 Python nodeids / 92 batches / 8 workers; zero failures | first run had one brittle test + 112 secondary failures; corrected retry passes JIT 569, opt 213, lower 371, typed 54, PyO3 8; full phase 161.678 s |

## Attempt history

### Attempt 1: guarded canonical builtin consumption of a real source generator

- Change: proposed two-file reuse of existing canonical type metadata and
  compiled generator resume inside the existing generic vectorcall path;
  the first complete private implementation is saved only after the two
  independently genuine unchanged-production semantic and structured
  REDs. Initial compilation succeeds, type-compiling all 569 JIT library
  tests, and the focused exported-vectorcall regression passes 1 / 1;
  the first complete JIT run later hits a hash-fragile unrelated existing
  dictionary-observer assertion and secondary same-process lock
  poisoning; the isolated observer rerun passes 1 / 1 and a fresh
  complete JIT library subsequently passes **569 / 569** after three
  direct-resume argument pins. Two additional exceptional-path and
  shorter-capsule fixes are implemented afterward; a distinct fresh
  final complete JIT library also passes **569 / 569 after all three
  corrections**, and the expanded adversarial integration passes.
- Measurements and coverage: retained stock scores, native/typed coverage,
  and the **692-sample zero-loss** comprehensions hotspot are available;
  final candidate benchmarks, exact transformed-body evidence, matched
  profile deltas, and the authoritative full correctness gate all
  complete.
- Compatibility and tests: preserve the source `PyFunction` watcher and
  actual generator object, CPython iteration/truthiness/short-circuit
  semantics, dynamic owners and hooks, exception ownership, monitors,
  force-interpreter behavior, and existing fallback. The initially saved
  **632-line** fixture is later strengthened to a frozen host-AST-clean
  **705 lines**; its first focused run reaches the fake
  `asyncio` comparison but exposes the prior terminal true-item retained
  lifetime rather than a clean target failure. A second unrelated
  precondition finds post-load `builtins.any` / `builtins.all` rebinding
  already ignored by existing statically resolved lowering. The reviewer
  preserves prior ownership / at-most-once finalization and replaces the
  invalid rebinding oracle with explicit
  **`consume_dynamic(consumer, values)`** generic fallback, treating
  module shadows conservatively. A third preexisting baseline mismatch
  is absent source-parent code-local `PY_START`; the fixture retains the
  parent as a nonasserting control and requires real generator-method /
  helper local callbacks instead. The corrected unchanged-production
  integration then fails **0 passed / 1 failed in 5.84 seconds** solely
  on stock **`checks=[]` / any=False / all=True** versus SOAC
  **`checks=['StopIteration', 'StopIteration']`** and two unexpected
  cancellation-check `RuntimeError`s; source watchers **2 / 2**, **40**
  counters, native parent/child bodies, and all mutation/monitor/fallback
  controls pass. A separate focused Rust run then executes one real test
  through existing exported `dp_jit_py_vectorcall`, correctly consuming
  real `any` / `all` source generators and preserving `len` /
  wrong-arity controls before failing only on selector
  **`(None, None) != (Some(Any), Some(All))`**; exit **101** confirms
  the genuine structured RED. An earlier zero-test `--exact` invocation
  and invalid Lima `--workdir` command are not counted as validation.
  The same actual production-path structured regression now genuinely
  passes **1 / 1**, selecting `any` / `all` and preserving unrelated
  fallback. The independent real stock/SOAC transformed semantic
  regression now also passes **1 / 1 in 8.63 seconds**; candidate
  `checks=[]`, `any=False`, and `all=True` exactly match stock while
  source-generator watchers and every frozen compatibility control remain
  intact. The first complete JIT run then encounters a hash-fragile
  existing GENERAL-dict collision callback expectation
  **`[false]` versus `[false, false]`**; its panic poisons the shared
  Python mutex and causes approximately **109 secondary failures**, not
  independent defects. The isolated unchanged observer passes **1 / 1**;
  randomized pinned-CPython open-address probing permits two equality
  calls, and this test never enters the `any` / `all` path. Independent
  source inspection also identifies a real direct-resume borrowed-
  argument lifetime issue when callbacks replace/delete `_resume_function`,
  `_preserved_values`, or mutable runtime `NO_DEFAULT`; the owner now
  implements INCREF pins for all three across native resume with balanced
  ordered cleanup, and the fresh complete JIT library passes
  **569 / 569**. Review then discovers a second reference-lifetime issue:
  generator-body error/finalizer code can mutate or promote runtime
  globals, freeing cached `prepared.runtime_values` before the non-
  `StopIteration` path dereferences `_reraise_control_flow`; the owner
  implements safe existing live globals / builtins helper lookup without
  resuming the generator again. A third independently discovered
  preserved-state length issue is user replacement of `_preserved_values`
  with another genuine
  shorter `soac.PreservedState` capsule: expected source layout cannot
  validate actual slot capacity, so raw
  `preserved_values_ptr + closed_index` may exceed its available slots.
  The implementation instead uses existing length-checked
  `preserved_state::load_preserved_state_owned` and truthiness without
  changing an out-of-scope file. All three source corrections are now
  implemented in exactly the approved two files; the prior **569 / 569**
  run predates the latter two fixes. Root-authorized narrow host-only
  integration additions are now frozen at **705 AST-valid lines** and
  cover a real zero-length same-name `PreservedState` capsule / bounded
  existing `RuntimeError`, an active yielding callback replacing all
  three owner arguments, and `ValueError`-triggered globals promotion /
  live replacement `_reraise_control_flow` invocation with restoration.
  The complete strengthened transformed integration genuinely passes
  **1 / 1 in 5.80 seconds** across Profile / Verify / Apply, retaining
  source-generator watchers, exact stock fake-asyncio behavior, and
  earlier class/helper/monitor controls. A separate genuinely fresh
  complete post-fix **`cargo test -p soac_jit --lib` passes 569 / 569**;
  this is distinct from the earlier pre-final-fix 569-test run. The
  broader existing transformed generator/runtime compatibility matrix
  also genuinely passes **71 / 71 in 35.48 seconds**. Root's package-
  scoped **`just fmt-rust soac_jit`** and
  **`just fmt-rust-check soac_jit`** pass, as does
  **`cargo check -p soac_jit --tests` in 10.05 seconds**. The final
  post-format expanded transformed integration genuinely passes
  **1 / 1 in 5.57 seconds** (**5.832 seconds total pytest time**); the
  required **26.46-second debug restage** is workflow-only setup, not a
  benchmark. Fixed-eight release debug-single smoke **125205** versus
  retained **112443** then passes **8 / 8** with exactly
  **2,242,168 native bytes / 148,116 blocks**, unchanged
  **2,866 typed blocks / 204 functions**, all per-PID function/adapter
  rows unchanged, and zero errors. Source-body counts remain chaos **32**,
  comprehensions **24**, deltablue **76**, fannkuch **1**, float **7**,
  nbody **6**, richards **51**, and spectral_norm **7**. Smoke's cold
  arithmetic **1.11x** is not throughput evidence. Normally sampled
  fixed-eight comparison **125415** then completes **8 / 8** with stock
  **0.601574599529184x** versus **0.5896760656259606x**; official
  previous **1.0702651354606592x** is outlier-skewed, while robust full-
  eight previous is **1.011716x raw / 0.991257x stock-adjusted**.
  Comprehensions improves **51.095554 -> 44.732796 us
  (1.142239x raw / 1.075505x paired)** with both worker-cluster intervals
  above one, but deltablue declines **0.977426x raw / 0.962409x paired**
  with intervals below one and requires repeated investigation. Chaos is
  neutral and richards noisy. All **80 measured normal Apply PIDs**
  retain exactly **23,188,640 native bytes / 1,527,950 blocks**,
  **2,866 typed blocks / 204 functions**, and zero errors. The two
  production files are frozen. The first targeted comparison
  **125840-kMbAuX** is discarded in full after an unauthorized overlapping
  stock-only benchmark starts at **13:01:36.648 PDT**, contaminates its
  round-03 Apply workers, and introduces approximately **41,285 NUL
  bytes** into a shared log; its separate JSON is not valid inference.
  Root stops the extra job and restarts a fully clean comparison
  **130328-CpSpU4** against retained **113536**. This wholly clean
  three-round / 60-value comparison proves comprehensions
  **49.926194 -> 44.862793 us (1.112864x raw / 1.123740x paired)** with
  both cluster intervals strictly above one. Chaos is neutral and
  deltablue paired **0.986022x** crosses neutrality; richards genuinely
  regresses **0.978051x raw / 0.962661x paired**, with both confidence
  intervals below one and a required dispatch investigation. Four-
  workload robust geometry is **1.017884x raw / 1.014091x paired**;
  targeted stock is **0.44513685009055015x** and official previous
  **1.0090672927925823x**. All **120 targeted Apply PIDs** retain exact
  per-round **18,255,240 native bytes / 1,201,600 machine blocks** and
  **2,265 typed blocks / 183 functions**, with zero errors. The candidate
  is favorable overall and fixes genuine CPython-visible behavior, but
  the Richards decline and causal profiles require investigation.
- Root then performs a bounded **same-strategy refinement** in only the
  already approved
  `crates/soac_jit/src/jit/specialized_helpers.rs`: classify existing
  null keyword arguments / nonnull argument buffer and `nargs` once;
  attempt the existing `next` selector only for **1 / 2 arguments**,
  existing `StopIteration` selector only for **2**, and new exact
  `any` / `all` selector only for **1**. The original `nargsf` value,
  selector priority, CPython fallback, public API, and compiled/native
  shape are preserved; unrelated Richards calls no longer pay the new
  universal selector. Independent source review finds no issue. Scoped
  formatting and final **`just fmt-rust-check soac_jit`** both pass,
  as does **`cargo check -p soac_jit --tests` in 2.76 seconds**. The
  actual exported-vectorcall structured Rust
  regression passes **1 / 1**, and the focused new transformed stock-
  parity integration plus **five existing StopIteration regressions**
  pass **6 / 6 in 11.34 seconds**. The initial Rust compile takes
  approximately **60 seconds** as workflow overhead only. One mistyped
  test filename is interrupted before collection, then corrected to the
  existing test file; it is not counted as a test failure or passing
  run. The earlier complete **569 / 569** JIT library execution predates
  this final selector refinement; focused Rust **1 / 1**, transformed
  **6 / 6**, scoped format check, and the **2.76-second** test-target
  check are the verified post-refinement results. Earlier normal and
  clean repeated comparisons remain accurate
  **historical pre-refinement results**. A new post-partition fixed-eight
  release debug-single comparison **131641** now passes **8 / 8** and
  exactly matches both retained **112443** and pre-partition **125205**:
  every measured Apply PID/full function-adapter row is unchanged,
  native code remains **2,242,168 bytes / 148,116 blocks**, optimized
  coverage remains **2,866 typed blocks / 204 functions**, and zero
  errors occur. Cold smoke timings do not establish throughput. The new
  normally sampled post-partition comparison **131748** now completes
  **8 / 8** with stock **0.6326613107877241x** and outlier-sensitive
  official previous **1.0612781659923773x**. Against retained **112949**,
  comprehensions improves **51.095554 -> 45.587036 us
  (1.120835x / 1.074564x paired)**; richards is neutral at
  **1.009151x raw / 0.987162x paired**, while deltablue is a possible
  decline **0.972967x / 0.963141x paired**. Against immediate
  unpartitioned candidate **125415**, richards recovers
  **31.108640 -> 30.027457 ms (1.036006x raw / 1.048397x paired)**
  with both intervals above one, while the target and deltablue are
  neutral. Robust full-eight retained geometry is
  **0.999207x / 1.008631x paired**; immediate candidate geometry is
  **0.987636x / 1.017527x paired**, reflecting run noise. All **80
  measured Apply PIDs** retain exact **23,188,640 native bytes /
  1,527,950 blocks / 2,866 typed blocks / 204 functions**, with zero
  errors. Final clean post-partition repeated comparison **132104**
  confirms comprehensions **49.926194 -> 44.872185 us
  (1.112631x [1.096347, 1.139781] / 1.123761x paired
  [1.105023, 1.153696])**. Chaos and deltablue are paired-neutral;
  Richards is raw-neutral **0.997661x**, with a residual paired
  **0.986296x [0.967541, 0.993720]** decline, but significantly recovers
  **1.020050x / 1.024551x paired** versus the unpartitioned candidate.
  Final subset robust geometry is **1.028280x / 1.033141x paired**;
  targeted stock **0.44758856139159614x**, official previous
  **1.0112507283090535x** is outlier-sensitive. Matched zero-loss
  comprehensions profiles **692 -> 547 -> 570** preserve the real
  generator and eliminate old builtin/iterator frames, while matched
  immediate Richards profiles **432 -> 568** reduce vectorcall
  inclusive/self **13.89% / 5.09% -> 10.56% / 3.35%**. The candidate is
  **LANDED CANDIDATE / RETAIN**, but its first authoritative full
  correctness gate fails on
  one preexisting GENERAL-dictionary collision-count assertion, followed
  by **112 shared-mutex secondary failures**. Root corrects only the
  existing Rust test in `function_instantiation.rs`, requiring nonempty
  all-false fresh-key identities for both GENERAL and dictionary-
  subclass cases while retaining original exception checks; focused
  repro **1 / 1** and package formatting pass. Exactly two runtime
  production files remain unchanged, with a third existing file touched
  only in `#[cfg(test)]`. The corrected authoritative full-gate retry
  then **exits zero**: **1,229 Python nodeids / 92 isolated batches /
  eight workers / 92 passed / zero failed**, plus **569 JIT**, **213
  optimizer**, **371 lowering**, **54 typed-IR**, and **8 PyO3** Rust
  tests. Build-test-runtime takes **32.538 seconds**, Cargo tests
  **72.456 seconds**, pytest **89.188 seconds inner / 89.206 seconds
  outer**, and the complete test phase **161.678 seconds**. The new
  generator regression passes in **7.28 seconds**, while the known
  28-node counter-dump batch takes **88.28 seconds**; see
  `work/logs/guarded-generator-builtin-consumption-test-all.log`.
- Result: **LANDED CANDIDATE / RETAIN; EXACT TWO-FILE IMPLEMENTATION COMPILES;
  STRUCTURED AND REAL STOCK-PARITY INTEGRATION EACH PASS 1 / 1;
  HASH-FRAGILE EXISTING OBSERVER PASSES ISOLATED; THREE-ARGUMENT
  REENTRANT OWNER, LIVE-GLOBAL ERROR PATH, AND BOUNDED CAPSULE FIXES
  IMPLEMENTED; STRENGTHENED REAL ADVERSARIAL INTEGRATION PASSES 1 / 1
  IN 5.80 SECONDS; DISTINCT FRESH FINAL POST-FIX JIT LIBRARY PASSES
  569 / 569; BROAD TRANSFORMED COMPATIBILITY PASSES 71 / 71 IN
  35.48 SECONDS; SCOPED FORMATTING / TEST-TARGET CHECK GREEN; FINAL
  POST-FORMAT INTEGRATION PASSES 1 / 1 IN 5.57 SECONDS; FIXED-EIGHT
  RELEASE SMOKE AND NORMAL COMPARISON EACH PASS 8 / 8 WITH EXACT NATIVE
  INVARIANCE; CLEAN TARGETED COMPREHENSIONS IMPROVES 1.112864X RAW /
  1.123740X PAIRED BUT RICHARDS REGRESSES 2.2-3.7%; SAME-STRATEGY
  ARGUMENT-SHAPE REFINEMENT STRUCTURED 1 / 1 AND TRANSFORMED 6 / 6;
  POST-REFINEMENT FIXED-EIGHT SMOKE PASSES 8 / 8 WITH EXACT NATIVE
  INVARIANCE; FINAL CLEAN TARGET IMPROVES 1.112631X AND RICHARDS
  RECOVERS 1.020050X VERSUS UNPARTITIONED; MATCHED ZERO-LOSS PROFILES
  CONFIRM MECHANISM; CORRECTED AUTHORITATIVE FULL GATE PASSES
  1,229 PYTHON NODEIDS / 92 BATCHES AND ALL RUST SUITES**.
- Reason: hot interpreted generator bridging exists despite an already
  compiled source body, but nested profile ancestry contains substantial
  required execution and cannot establish a recoverable speedup.

## Verdict and next action

- Verdict: **LANDED CANDIDATE / RETAIN; AUTHORITATIVE FULL GATE PASSED;
  TWO INDEPENDENT
  GENUINE STOCK-VS-SOAC
  USER-VISIBLE SEMANTIC AND EXPORTED-VECTORCALL STRUCTURED REDS
  CONFIRMED; TWO-FILE IMPLEMENTATION COMPILES AND BOTH REGRESSIONS TURN
  GENUINE RED-TO-GREEN; UNRELATED HASH-FRAGILE DICTIONARY OBSERVER
  PASSES ISOLATED; THREE-OWNER REFERENCE HANDLING, LIVE RUNTIME-GLOBAL
  ERROR PATH, AND SHORT-CAPSULE LENGTH CHECKS IMPLEMENTED; STRENGTHENED
  TRANSFORMED ADVERSARIAL INTEGRATION PASSES 1 / 1 IN 5.80 SECONDS;
  FRESH FINAL POST-FIX JIT LIBRARY PASSES 569 / 569; BROAD TRANSFORMED
  COMPATIBILITY PASSES 71 / 71 IN 35.48 SECONDS; PACKAGE FORMATTING /
  TEST-TARGET CHECK PASS; FINAL POST-FORMAT EXPANDED INTEGRATION PASSES
  1 / 1 IN 5.57 SECONDS; FIXED-EIGHT RELEASE SMOKE PASSES 8 / 8 WITH
  EXACT FUNCTION / NATIVE INVARIANCE; NORMAL FIXED-EIGHT TARGET
  IMPROVES 1.142239X RAW / 1.075505X PAIRED; WHOLE CLEAN TARGETED
  REPEAT IMPROVES COMPREHENSIONS 1.112864X RAW / 1.123740X PAIRED,
  BUT RICHARDS GENUINELY REGRESSES 0.978051X / 0.962661X;
  SAME-STRATEGY ARGUMENT-SHAPE REFINEMENT STRUCTURED 1 / 1 AND
  TRANSFORMED 6 / 6 PASS; POST-REFINEMENT FIXED-EIGHT SMOKE PASSES
  8 / 8 WITH EXACT NATIVE INVARIANCE; POST-PARTITION NORMAL TARGET
  IMPROVES 1.120835X AND RICHARDS RECOVERS 1.036006X VERSUS
  UNPARTITIONED CANDIDATE; FINAL REPEATED TARGET IMPROVES 1.112631X,
  RICHARDS RECOVERS 1.020050X VERSUS UNPARTITIONED, ROBUST SUBSET
  IMPROVES 1.028280X, AND MATCHED ZERO-LOSS PROFILES SUPPORT THE
  MECHANISM; CORRECTED AUTHORITATIVE FULL CORRECTNESS GATE PASSES
  1,229 PYTHON NODEIDS / 92 ISOLATED BATCHES / ZERO FAILURES**.
  Unexpected
  fake cancellation checks are a
  proven existing CPython-visible defect and the frozen transformed
  regression confirms candidate stock parity. Normal comprehensions
  improves in repeated uncontended pre-refinement rounds, deltablue is
  paired-neutral, and the original aggregate is favorable, but Richards'
  genuine repeated regression prompted the bounded shape refinement.
  Final repeated comparisons preserve the target gain, show paired-
  neutral deltablue, and substantially recover Richards versus the
  immediate unpartitioned candidate, though a **1.37% paired Richards
  decline versus retained** remains. The matched immediate Richards
  profile is distinct from the unrelated old revision. The full-suite
  **1.10x stock** target remains unmet.
- Transferable lesson: preserve the genuine source generator and CPython
  iteration protocol; optimize only a proven canonical compiled-resume
  bridge, and never treat inclusive builtin/iterator shares or retained
  source-body work as guaranteed removable savings.
- Next action: root-owned integration of the fully validated retained
  candidate; never use discarded
  **125840-kMbAuX** for inference or compare old Richards revision
  **ccef62b6** as though it were current baseline **2bb19f4f**. Decide
  preserve the historical first-gate failure and durable existing-test
  correction. The complete-suite **1.10x stock** target remains unmet.
