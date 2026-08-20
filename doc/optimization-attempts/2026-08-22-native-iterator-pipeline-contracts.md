---
title: "Native-iterator map/filter pipelines"
---

# Native-iterator map/filter pipelines

- Status: in progress
- Pacific date: 2026-08-22 PDT
- Change or revision: shared strict-runtime working change; integration pending
- Outcome: selection gap measured; first implementation written, after/performance evidence pending

## Hypothesis and evidence

- General-purpose opportunity: remove closed `map`/`filter` wrapper dispatch and
  compiler-helper admission dependencies while leaving an arbitrary input
  iterator and ordinary callbacks on their native Python boundaries.
- Supporting evidence: a genuine offline strict publication on extension
  `b0080fcf0bb000dab2971eab3a132b6e1fa41c4aeb26bd4b580df9d08ce3859c`
  produced zero selected map/filter stage plans in both apply and verify. The
  source used a native, compiler-created genexpr with its actual preserved-state
  capsule. All callback/result/provenance controls passed before the selection
  assertion failed. This is selection-unavailable evidence, not proof that
  source-activation exclusion is a defect.
- The existing selector requires a generator-instance plan before helper lookup.
  Generic range and argument-backed iterables therefore need a distinct native
  input plan. Ordinary `soac.runtime` membership does not admit helper bodies.
- Expected effect: completed compiler-owned pipeline bundles with no residual
  unadmitted helper calls, no source-activation elimination, and lower per-item
  dispatch overhead. No speedup has been measured.

## Implementation and compatibility

- Implementation: a validated typed sidecar selects a fixed native CFG
  template for a closed stage and materializer. Map/filter stages and list/tuple
  sinks are independent choices, not benchmark-specific replacements. Codegen
  emits the selected ownership and error phases mechanically.
- Input iterators remain native objects, including strict source genexprs. Their
  construction, source-frame lifetime owners, handled state, and public resume
  boundary remain intact. Ordinary callbacks are not granted optimization
  authority or called through an unchecked ABI.
- The complete bundle must prove `MustEliminate` for its wrapper, guard the
  actual evaluated source callee identities before dependent effects, and leave
  the original operation available on guard failure without reevaluating inputs.
  Unsupported uses, incomplete templates, or budget failures decline atomically.
- Internal operations bind canonical native semantics, not mutable
  `soac.runtime` attributes or ambient builtins. Profile observations prioritize
  plans; they do not establish callable identity or authority.
- Required semantic coverage includes one iterator acquisition, construction
  errors versus iteration exhaustion, callback/truth failures, no eager
  callability check, surviving-object ownership and finalizer order, list
  capacity/shrink behavior, and partial list/tuple cleanup. The approved
  eliminated-internal-object relaxation does not relax surviving user lifetimes.
- Selection happens before expression linearization; a complete nested call
  remains atomic until its fixed CFG is emitted. Selection reserves expansion
  cost, validates current consumer/wrapper origins and both guards, and commits
  all requested plans together. Final typed rewrites and emission revalidate the
  current tree. Source-function activation exclusions remain unchanged.
- Raw materializer primitives keep tuple's eight-item stack buffer, its native
  list promotion before a ninth request, and native consuming tuple completion.
  The surviving list uses `list_vectorcall`'s generic allocator and native
  preallocation/shrink policy. Map uses vectorcall without the offset flag;
  filter uses `CallOneArg` and refreshes its native next slot per delivered item,
  not per rejected input. These are written paths, not verified-after claims.

## Benchmark protocol and coverage

- Fixed correctness workload: ordinary callbacks in closed map/list and
  filter/tuple pipelines, plus retained native source-genexpr controls, using
  genuine offline strict publication and compiled/entry-interpreter modes.
- Performance benchmark selection: independently runnable `chaos`, then the
  fixed full pyperformance target set for any acceptance claim. Not run yet.
- Comparison command and rounds: `just pyperformance-compare chaos 3`, followed
  by the full-suite comparison before a retained performance claim; pending.
- Baseline artifact: the b008 extension above, selected native v19, normal
  checker0020/v19. The actual before receipt is
  `work/strict-v19-intrinsic-pipeline-before-b008-retry/result.json`.
- Candidate artifact: native20b/checker0020/v20, extension
  `d6c3467682c832044831c9c13adf7d1aa78adcdb916c1f85c59022b719a44417`.
  This candidate fails the reserved-import integration boundary below; it is
  not an accepted or benchmarked artifact.
- Profile evidence: independently generated profile/apply/verify processes for
  the measured fixture, using the same publication and captured dependencies.
- Module selection: only the fixture's selected strict module. Identical
  ordinary source and callback helpers stayed on stock Python.
- Completed/failed benchmarks: none run; no benchmark or suite-wide claim.
- Transformed benchmark/dependency/standard-library coverage: unavailable.
- Correctness coverage: ordinary two cases and strict entry control passed;
  compiled behavior passed, then all four apply/verify map/filter selection
  checks failed. Real source capsules matched their native generator owners.

## Measurements

| Metric | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| Stock CPython benchmark elapsed | Pending | Pending | n/a |
| SOAC apply benchmark elapsed | Pending | Pending | Pending |
| Stock / SOAC speedup | Pending | Pending | Pending |
| Previous SOAC / candidate SOAC | n/a | Pending | Pending |
| Selected map/filter stages, apply | 0 / 0 | Pending | Pending |
| Selected map/filter stages, verify | 0 / 0 | Pending | Pending |
| Optimized typed-IR blocks/instructions | Unavailable | Pending | Pending |
| Pre-optimization BlockPy bytes | Unavailable | Pending | Pending |
| Apply-mode native code bytes/machine blocks | Unavailable | Pending | Pending |

## Attempt history

### Attempt 1: genuine strict selection discriminator

- The first run passed two ordinary controls but both strict cases failed to
  import the repository test helper under the isolated validator process.
  The 19.55-second gate is fixture/tooling evidence, not an optimization result.
  Its unchanged-input receipt remains at
  `work/strict-v19-intrinsic-pipeline-before-b008/result.json`.
- The fixture-only repair supplied the repository path to the validator, not to
  strict source execution or authority. The rerun completed at
  2026-08-22 22:35:43 PDT: 3 passed, 1 failed; 38.48 seconds of tests and
  42.02 seconds for the gate. Runtime and test snapshots were unchanged.
- Apply and verify each reported zero selected map stages and zero selected
  filter stages. All ordinary/strict result, callback, exception, and actual
  source-creation provenance checks passed before the final selection assertion.
- Historical fragment-attempt telemetry was not completed-rewrite evidence.
  A separate correctness repair now commits inline sidecars/events only after
  every target in that rewrite succeeds; its focused regression and full
  optimizer suite (237 tests) passed. The older b008 runtime did not contain
  that telemetry repair, so event absence is not used as independent evidence.
- Result: feature opportunity confirmed, implementation pending. Refine the
  positive to opaque native iterator support; keep unsupported source-activation
  composition as an explicit negative rather than weakening its guard.

### Attempt 2: separate native-input selection from source-activation retention

- Refined the maintained fixture into range-backed map and argument-backed
  filter positives, plus separate source-genexpr native-owner controls. Added
  list `__sizeof__` comparisons, lengths 0/1/2/3/4/7/8/9/17, and noncallable
  callbacks on empty inputs. All ordinary callbacks remain unadmitted.
- The new before used extension
  `88718261c5b3b3e64b937732f4c024a4c3e3ac4dc20322eb6d6f6dc5c0a6e32e`
  with the same selected native19/checker/support. It completed at
  2026-08-22 22:58:40 PDT: 7 passed, 1 failed; 71.69 seconds of tests and
  75.14 seconds for the gate. Runtime and test snapshots were unchanged.
- Every behavior/capacity check passed in profile, apply, and verify. Both
  entries retained real source-genexpr capsules matching their native generator
  owners in every mode. Only the compiled native-input positive failed: map
  and filter each had zero committed bundles in apply and verify.
- Receipt: `work/strict-v19-native-iterator-pipeline-before-887/result.json`,
  SHA256 `0622a4b15cc30cbd94f1193fc85f91e4a8c26293e592c6b9f5b3692505936ad7`.
- At this checkpoint only the data-only typed plan/provenance/guard types and
  tests had changed; the selector and emitter were unchanged. No performance or
  implementation-after claim follows from this baseline.

### Attempt 3: atomic fixed-template selection and mechanical emission

- The tests-first structured BEFORE gate recorded one pass and one failure in
  0.08 seconds. All four Map/Filter × List/Tuple combinations had zero plans;
  source-activation/observed-wrapper negative controls passed. Receipt:
  `work/strict-native-iterator-plan-before/result.json`; log SHA256
  `cce5278fa560d7127e7ee0bb513d7b029f98037a453f50ed5fe353e81800e8d4`.
- Added the bounded selector, current-IR validator, transactional replacement,
  inline-budget reservation, and mechanical native CFG. The cold fallback
  consumes the evaluated callee/callback/iterable operands exactly once. The
  implementation invokes no unadmitted Python helper body and creates no new
  source execution authority.
- Added structured late-invalid-proposal rollback, zero-budget, stale-current-
  operand, and real declared-global-load controls. Global source-name spelling
  is candidate membership only: both actual loaded callee objects must pass
  their native identity guards. Local/cell rebinding remains outside this slice.
- Added primitive kernels for capacity/buffer thresholds, partial-result
  cleanup with the exact pending error, and independent callee guard failures.
  Static review corrected list allocation from `PyList_New` to the actual
  `list_vectorcall` allocator, and map invocation from `CallOneArg` to its actual
  no-offset vectorcall shape before execution.
- Expanded maintained behavior controls with independently replaced source
  globals, callable-class construction raising `StopIteration`, and a real
  next-slot change via supported `__class__` assignment between two
  `itertools.count` subclasses. The slot test checks distinct native C slots;
  merely replacing a Python `__next__` method would not distinguish the cache
  rule. These controls await matching native20 execution; they were not run
  against the mixed source/selected-runtime window.
- Current test fixture SHA256:
  `5ed20bf32c8a02b91d718f2bbd617c826f7bfcbb44a4e38231e7deb8da73745f`.
  Changed-range Ruff parsing/formatting passed; full-file Ruff still reports
  three pre-existing diagnostics outside this attempt's edits. Rust checks,
  actual selection/codegen, runtime-after, code sizes, and performance remain
  pending at this checkpoint.

### Attempt 4: matching native20 runtime integration

- The native20b/checker0020/v20 checkpoint passes eight native iterator
  selection/kernel tests and its related full compiler-library checks. Its
  matching debug extension, `d6c3467682c8…`, builds from unchanged inputs in
  **36.52 seconds** and verifies the actual loaded interpreter, library,
  checker, support code, and extension. This is build evidence, not timing of
  an optimized workload.
- The actual profile/apply/verify cohort finishes **9 passed, 4 failed** in
  **70.01 seconds** (73.88 seconds including provenance). The ordinary controls
  pass. Every strict case reaches a missing `dp_jit_native_iterator_guard`
  declaration in the immutable JIT worker snapshot; no completed optimized
  runtime claim follows. Receipt:
  `work/strict-v20-native-iterator-first/result.json`; log SHA256
  `5f2892a10c0863c6872e0dca5b1ef1f0fc4547de63c3d51cce71a4fea96a4671`.
- A maintained one-function strict repro independently passes its profile
  invocation and fails at the same apply import boundary in **15.73 seconds**
  (19.13 seconds including provenance). All runtime and test inputs are
  unchanged. Receipt:
  `work/strict-v20-native-iterator-import-repro-before/result.json`; log SHA256
  `9db924269200b8fbd6c319de2c882b57e9b38f480113af40dbc3109115e65f22`.
- Root cause: native addresses were registered with the JIT builder, but the
  separate serial reservation phase had not declared the new imports before
  copying its read-only worker snapshot. The correction uses one native
  primitive inventory for both phases, also reserves the new consuming
  collection-insertion helper, and adds a structured test that requests each
  complete signature through the actual immutable codegen environment. Its
  compile/runtime after-gates are recorded below.

- The structured frozen-import test passes through the real immutable codegen
  environment, and the matching extension
  `9c8b411dd2c11b3bc2fb2bc477627822cb90f93f9379f0f7828bf67d5f6d3e0b`
  builds from unchanged source. Its actual runtime after-cohort completes at
  **2026-08-23 01:07:30 PDT** with **14 passed** in **148.09 seconds**
  (**151.64 seconds** including provenance). This includes the minimal strict
  repro, compiled and forced-entry profile/apply/verify cases, and ordinary
  callback, guard, capacity, callable-class, and next-slot controls. Apply and
  verify require committed bundle events for the five exercised functions;
  separate source-genexpr cases retain their actual native activation owners.
  Receipt: `work/strict-v20-native-iterator-imports-after/result.json`; log
  SHA256 `d077d1cc0455841fc579aa85806e25c89cb7ed17962f73c87dc45819015d941d`.
  The native, checker, support, extension, and fixture inputs remain unchanged.
  These are behavior and structured-selection results, not performance timings.

### Borrowed Step ownership and rejected Python-method specialization

The shared Operand/IteratorStep repair has genuine runtime evidence: the fixed
shared39 replay passes the receiver-ownership regression and all nine loop-exit
shapes through both compiled and entry modes. It preserves the original
borrowed native `tp_iternext` call and consuming cleanup; it does not depend on
inlining an iterator implementation. Separate retained-exception controls
exposed excess source traceback attachment on implicit exhaustion, motivating
the original-source error-site policy and its compiled/entry consumers.

A subsequent attempt to inline Python `IterRange.__next__` or
`ClosureGenerator.__next__` from an IteratorStep was rejected and withdrawn.
The structured fixtures supplied canonical owner/method facts, but the current
authenticated strict runtime has no legitimate producer for those helper
bodies: ordinary `soac.runtime` remains native and unowned, native `range` does
not construct `IterRange`, and the old method/helper catalog requires a
transformed runtime module. Passing synthetic selector tests therefore did not
establish an optimization applicable to real strict programs. No module
admission exception, source-activation relaxation, or benchmark-source change
was introduced to make the tests select.

The withdrawal preserves raw native Step, shared Take ownership/dataflow,
observable receiver use, alias/liveness blockers, and generic SourceErrorSite
propagation/cache validation. Replacement structured tests inline ordinary
guarded helper bodies while retaining their raw Step and original caller
attribution, including a separate source-activation decline. The existing
canonical native map/filter materializer plans and their actual runtime paths
are unchanged. A native for-region extension with a complete resolved
constructor/Step/retirement MustEliminate proof remains an ignored design,
pending a separate hot-path/measurement decision. No performance claim is made
for either the withdrawn selector or that future design. The failed and
corrected fixture receipts and the source-only withdrawal record are retained
under `work/iterator-step-source-site-draft/` and the associated
`work/strict-v20-source-site40-*` directories.

## Verdict and next action

- Verdict: genuine runtime selection and compatibility pass in the focused
  cohort; full-gate, code-size, and performance acceptance remain pending.
- Transferable lesson: compiler operation provenance, source callable
  authority, iterator ownership, and complete transformation commitment are
  different facts. A raw candidate or correct ordinary fallback proves none
  of the others.
- Next action: integrate with the remaining ownership/class changes, run the
  full gate, and collect fixed benchmark evidence. Comparisons must use the same native epoch for stock,
  previous SOAC, and candidate; the old native19 correctness artifact alone is
  not a comparable performance baseline.
