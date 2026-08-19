---
title: "Inline safe eager comprehensions"
---

# Inline safe eager comprehensions

- Status: REOPENED / ATTEMPT 2 IN PROGRESS; Attempt 1 remains REJECTED;
  real CPython watcher/finalizer failures and structured
  lowering/ownership regressions were established, and the candidate passed
  Profile mode, but Apply/Verify produced unsafe repeated iterator
  destruction. All experimental production code and regression tests were
  reverted; this strategy record is the only retained artifact.
- Pacific date: 2026-08-18 PDT.
- Baseline revision: integrated captured-builtins change `zolltvqv`, commit
  `827fa7f9`; new working change `uopnoqlm`.
- Outcome: REJECTED. Stock CPython already inlines eager list, set, and dictionary
  comprehensions, but SOAC currently creates and executes a new synthetic
  closure/function for each comprehension evaluation. Prior zero-loss
  `comprehensions` profiling attributes **28.30% inclusive CPU** to closure
  creation and **6.52%** to vectorcall registration. Lazy generator-factory
  work is a separate **21.13% inclusive** path and is not an eager-inlining
  speedup claim. Seven synthetic eager-comprehension native bodies consume
  **107,628 bytes / 7,003 machine blocks**. A conservative statement-local
  lowering could remove this redundant work while restoring CPython's
  no-comprehension-function observer behavior; semantic validation and real
  benchmark impact was never safe to measure. The completed fixed-eight baseline
  takes **515.48 seconds** and has stock-relative geometric ratio
  **0.2811670766x**, far below the full-suite **1.10x** target. A genuine
  unchanged-production regression fails in **1.23 seconds**: stock creates
  **zero** eager-comprehension functions while SOAC creates **five**; it
  also exposes independently preexisting iterator/item/local finalizer-order
  divergence. Renamed target visibility to `locals()`, `vars()`, dynamic
  evaluation, frames, and tracing requires conservative rejection or an
  explicitly enforced compatibility policy. Structured lowering now produces
  zero synthetic helper definitions for direct/nested list, dict, and set
  comprehensions (**one test passed; 368 filtered**), and Profile achieved
  zero watcher events with correct values/finalizers. However, one ordinary
  Apply/Verify invocation called its iterator finalizer approximately
  **204 times**, demonstrating use-after-free/double destruction; broader
  cleanup-root promotion also captured **five** existing typed generator-
  state owners. Location-global release filtering and exception-dispatch
  drop hypotheses were disproved. No candidate benchmark or full gate was
  run, and all production/test experiments were reverted.

## Hypothesis and evidence

PEP 709 changed stock CPython to inline dictionary, list, and set
comprehensions instead of creating a new one-shot Python function each time.
The vendored `vendor/cpython/Doc/whatsnew/3.12.rst` explicitly records that
there is no longer a separate comprehension frame or tracing/profiling
function call, while iteration targets remain isolated from the outer scope.
The running project targets CPython 3.15, so an eager comprehension's
additional synthetic Python function is not only an avoidable runtime cost:
it creates user-observable function-watcher and call-frame behavior that
modern CPython does not produce.

The vendored `vendor/cpython/Python/codegen.c::codegen_comprehension`
evaluates the outer iterable exactly once, then uses
`push_inlined_comprehension_state`, emits the result container and iteration
directly, and restores local state with
`pop_inlined_comprehension_state`. Its exception cleanup discards a partially
built result and restores displaced comprehension locals. Generator
expressions intentionally retain a separate callable because their execution
and iteration are lazy; they are outside this eager-inlining strategy.

SOAC's current
`crates/soac_lowering/src/passes/ast_to_ast/rewrite_expr/comprehension.rs`
routes all list/set/dict expressions through `lower_function`. Even when the
expression is synchronous and immediately assigned to a genuine local or
returned directly, that path synthesizes `def _dp_listcomp_*`,
`_dp_setcomp_*`, or `_dp_dictcomp_*`, materializes a closure and Python
function, initializes the result inside the synthetic function, and
immediately calls it. The existing `InlineCompKind` name does not mean the
current production path actually inlines a comprehension.

The latest zero-loss post-captured-builtins native profile is
`work/logs/comprehensions-captured-builtins_callgraph.txt` with companion
`work/logs/comprehensions-captured-builtins_speedscope.json`: **199 Hz /
30,000 loops**, **zero lost samples**, and **877 Speedscope sampled stacks /
100,229 total weights**. Overlapping inclusive costs are:

- Synthetic closure creation: **28.30%**.
- Shared nested-function instantiation: approximately **26.86%**;
  `instantiate_bb_function_inner`: **24.79%**.
- Per-function vectorcall registration: **6.52%**.
- Synthetic nested `WidgetTray` eager-comprehension frames: approximately
  **50.13%** for one list comprehension, **20.27%** for a dictionary
  comprehension, and **10.31%** for its nested list comprehension. These
  describe enclosing hot regions, not removable costs; their shares overlap.
- Lazy generator-factory vectorcall: **21.13%**, a separate non-eager
  hotspot; this strategy must not assume it disappears.
- Existing seven compiled eager-comprehension bodies: **107,628 emitted
  native bytes / 7,003 machine blocks**.

A source-AST opportunity census of the fixed eight workloads finds **seven
eager comprehensions in `comprehensions`**, including **five** directly
assigned/returned eligible shapes and **one** potentially recoverable nested
shape; **six in `chaos`**, including **two** direct shapes; and **one direct
shape in `spectral_norm`**. `fannkuch`, `float`, `nbody`, `deltablue`, and
`richards` contain no matching eager comprehension. This is an opportunity
census, never a benchmark-name admission rule or claim that every direct
shape is semantically safe.

The falsifiable hypothesis is that eligible eager expressions can execute
directly in their containing function, with **zero synthetic Python function
CREATE watcher events**, fewer nested function/code bodies, preserved CPython
cleanup and scoping behavior, and improved ordinary Apply-mode throughput.

The unchanged-production focused regression
`tests/test_inlined_eager_comprehensions.py::test_eager_comprehensions_use_the_enclosing_frame_without_changing_lifetimes`
is genuinely RED: **one failed test in 1.23 seconds**. Its Profile assertion
`result["created"] == []` fails because a real CPython function watcher
observes **zero** eager-comprehension function CREATE events on stock CPython
but **five** in SOAC:

```text
direct_return.<locals>._dp_listcomp_6
eager_shapes.<locals>._dp_dictcomp_11
eager_shapes.<locals>._dp_setcomp_14
eager_shapes.<locals>._dp_dictcomp_19.<locals>._dp_listcomp_22
eager_shapes.<locals>._dp_dictcomp_19.<locals>._dp_listcomp_22
```

The same fixture independently exposes a preexisting user-visible lifetime
bug. For successful iteration, stock CPython records
`[iter, drop-iter, drop-old-y, drop-item, after]`, while unchanged SOAC
records `[iter, drop-item, drop-iter, drop-old-y, after]`. It also captures
failure cases in iterator construction, `next`, and the comprehension body.
For `__iter__` / `__next__` failures, stock traceback activation frames can
retain the iterator differently from untouched SOAC because existing JIT
execution omits those frames. That is an existing approved activation-
introspection boundary: preserve prior SOAC behavior rather than claiming
unsupported CPython traceback-retention equivalence. The new optimization
must still fix its own normal/body-path cleanup and preserve observable
callbacks that prior SOAC already delivered.
The revised fixture therefore explicitly preserves previous SOAC
`__iter__` failure events `[iter, drop-iter, caught:outer]` and `__next__`
failure events `[iter, drop-result, drop-item, drop-iter, caught:outer]`,
while retaining strict stock-compatible successful/body cleanup and
partial-result destruction.
This divergence exists before candidate code; the optimization must either
correct optimization-owned tested ownership/finalizer ordering or retain an
explicitly safe fallback. The watcher check is intentionally first so the
baseline RED cleanly demonstrates excess function creation without masking
the separate finalizer issue.

No candidate performance, code-size reduction, watcher GREEN, corrected
finalizer ordering, or full-suite result is claimed before the corresponding
regressions and benchmark actually pass.

## Implementation and compatibility

- Select only synchronous eager list/set/dict comprehensions used directly
  as the entire expression of a real local assignment or direct return.
  Do not broaden to arbitrary expression contexts until source evaluation
  order and result ownership are independently proven.
- Perform the decision and transformation in explicit lowering/AST or IR,
  before mechanical code generation. Do not recognize benchmark names,
  source fingerprints, exact expected outputs, or incidental rendered IR.
- Give each comprehension iteration target an independently fresh internal
  storage slot. Keep nested target slots disjoint, prevent outward leakage,
  and reject overlap with existing outer locals or other unsafe scope shapes
  unless CPython-equivalent save/restore has been proven.
- Renaming an iteration target must not change observable activation state:
  `locals()`, `vars()`, `eval`, `exec`, `sys._getframe` / frame locals,
  tracing, and profiling can inspect or depend on the source-visible binding.
  `OPT_GOAL.md` already defines an **approved activation-introspection
  relaxation**, but requires unsupported observation to fail explicitly,
  decline optimization, or use a compatible fallback; silently returning
  wrong/incomplete locals or dropping entitled callbacks is forbidden.
  Existing integration cases mark frame-sensitive locals/frame behavior as
  unsupported, but an xfail or policy statement alone does not prove this
  candidate's dynamic enforcement. Preserve correct source-name visibility
  or demonstrate the required explicit failure/fallback. The current
  lowerer conservatively rejects recognizable `locals`, `vars`, `dir`,
  `globals`, `eval`, `exec`, and `_getframe` introspection, while
  alias/dynamic-call boundaries and end-to-end enforcement still require
  semantic validation.
- Evaluate the outer iterable exactly once and in CPython order. Preserve
  nested iterable/filter ordering, `__iter__` / `__next__` callbacks, dict
  key-before-value evaluation, container initialization, mutation side
  effects, and all successful/exceptional control-flow edges.
- Preserve iterator and value finalizer timing, including partial-result
  destruction, target cleanup, custom `__del__` callbacks, and cleanup
  ordering when iterables, filters, elements, or container updates fail.
  Finalizer behavior must be tested, not inferred from equal final values.
  For preexisting `__iter__` / `__next__` traceback-frame retention
  differences covered by the approved activation relaxation, preserve prior
  SOAC semantics rather than claim unsupported stock-frame equivalence.
- Transfer the internal `_dp_tmp` result into the local assignment or return
  exactly once. Preserve owned references, previous-local decref timing,
  return transfer, exceptions, and absence of leaked compiler temporaries.
- Fall back unchanged for asynchronous comprehensions, generator
  expressions, assignment expressions/walrus operators, captured lambdas or
  nested escaping closures, `global`/`nonlocal` destinations, overlapping
  iteration targets, class/module-sensitive bindings, and nondirect
  expression contexts. Unsupported shapes must retain the existing tested
  synthetic-function lowering.
- Preserve user-authored functions and function-watcher events, tracing,
  original code objects, closure cells, recursion, shadowing, captured
  builtins, runtime module lifecycle, profile/apply evidence, and the prior
  compiler-owned `StopIteration` binding fix.
- A focused CPython `PyFunction` watcher establishes a genuine baseline
  RED (**one failure in 1.23 seconds**): stock eager comprehensions create
  **zero** synthetic comprehension functions while SOAC creates **five**.
  The same fixture captures independently preexisting successful/error-path
  iterator/item/outer-local finalizer-order mismatches.
- A separate structured lowerer regression,
  `passes::test::direct_eager_comprehensions_stay_in_the_enclosing_function`,
  first genuinely fails against unchanged production with synthetic
  `choose.<locals>._dp_listcomp_3`, `_dp_dictcomp_8`, its nested
  `_dp_listcomp_11`, and `_dp_setcomp_14`. It now passes genuine RED→GREEN:
  **one passed, 368 filtered**. Direct list, nested dict/list, and set
  comprehensions produce **zero synthetic helper definitions or
  `MakeFunction` operations**, while resolved IR retains compiler-trusted
  iteration/next operations and quiet deletion cleanup. The implementation
  also accounts for Ruff's empty-set literal shape. Production changes span
  `ast_to_ast/rewrite_expr/comprehension.rs`, `rewrite_expr/mod.rs`, and
  `driver.rs`, with the structured regression in `passes/test.rs`. The full
  Profile→Apply watcher, ownership, and finalizer regression remains pending;
  a structural GREEN does not establish runtime semantics or throughput.
- The first candidate end-to-end run exposes a generated-template issue:
  using `{{}}` for an empty dictionary in `py_stmt!` creates a set containing
  a dictionary and fails because dictionaries are unhashable. The template
  is corrected to literal `{}`; rerun and semantic GREEN remain pending.
- The next runtime iteration removes all five watcher-visible synthetic
  CREATE events and produces correct list/dict/set/nested result values, but
  still fails ownership/finalizer semantics. Its normal event sequence is
  `[iter, drop-iter, drop-old-y, after]`, missing CPython's required
  `drop-item` entirely; exceptional iterator/body cases also omit expected
  destruction callbacks. Zero synthetic functions plus equal return values
  do not establish correctness. The end-to-end regression remains RED until
  cleanup is repaired across successful and exceptional exits. The concrete
  blocker is that the JIT cleanup-root predicate broadly excludes **all**
  compiler-generated `_dp_*` locals, including newly inlined owned
  comprehension values. Any correction in `jit/planning.rs` and its
  structured `jit/test.rs` regression must use a **general validated
  ownership criterion**: root a `LocalLocation` with an actual **owned
  `Rebind` in one basic block and explicit `Delete` in another block**,
  regardless of its name. The existing `MaybeOwned` lattice already handles
  optional/unbound finally cleanup, and `ownership_effects` already models
  `Del` as both read and kill, so no optimizer-crate change or special
  `_dp_*` prefix exception is warranted. The structured planning regression
  is independently genuine RED:
  `jit::planning::tests::owned_internal_locals_with_explicit_cross_edge_cleanup_have_roots`
  fails (**one failed; 547 filtered**) because owned
  `RefcountLocal { location: LocalLocation(7), name: "_dp_tmp_1" }` is absent
  from the computed cleanup roots, which contain only `values`. An earlier
  proposed `Delete` old-state intersection cannot work because a maybe-bound
  merge reports `Unbound`; the validated cross-block owned-`Rebind` plus
  `Delete` criterion addresses that actual CFG shape. The structured
  regression now passes genuine RED→GREEN (**one passed; 547 filtered**):
  owned cross-block cleanup receives a root independent of local spelling,
  an immediate same-block temporary remains unrooted, and existing
  try-carrier exclusions remain unchanged. Runtime Profile→Apply cleanup
  improves substantially: zero watcher CREATE events and result values pass,
  successful finalizers now exactly match stock CPython, and body-exception
  cleanup also exactly matches stock. The sole remaining mismatch is
  non-`StopIteration` `__next__` exception ordering versus prior SOAC.
  The approved narrow correction places partial-result deletion and
  reverse-order target cleanup in a `next`-only bare exception handler before
  reraising, while the outer `finally` releases the iterator. That
  correction now makes the entire **Profile-mode** watcher/value/lifetime
  matrix GREEN, including zero synthetic CREATE events, exact stock
  successful/body finalizers, and preserved prior-SOAC `__iter__` /
  `__next__` exception ordering. **Apply still fails**: an ignored
  `Payload.__del__` first raises `StopIteration`, then raises
  `TypeError` while concatenating a string with `NoneType`, and the
  overlap-fallback assertion fails. This suggests, but does not yet prove,
  an optimized ownership/double-decref error. A narrower, still
  name-independent candidate roots only a location with an actual owned
  `Rebind` in one block and `Delete` in a different block whose planner
  `old_state == Unbound`, identifying an optional/maybe-bound cleanup join.
  Existing must-bound or transported inline temporaries remain unrooted when
  their real ownership states differ. The narrowed criterion keeps the
  structured planning test GREEN (**one passed; 548 filtered**) and the
  complete Profile matrix GREEN, but **does not fix Apply destructor
  corruption**. This disproves the initial hypothesis that ordinary
  must-bound typed-inline roots caused the Apply failure. Bounded per-
  function diagnostics further prove `Payload` / `Values` methods acquire
  **zero** new internal roots, while `local_assignment` gains only its
  iterator, result, next-value, and target roots. Nevertheless, its first
  Apply/Verify normal call emits up to **204 repeated `drop-iter`
  finalizers** plus ignored `Payload` errors, while Profile finalizes exactly
  once. This proves use-after-free/double destruction in optimized rooted
  iteration, not a changed destructor method. Separate `escaped_capture`
  also picks up **five** existing `typed_gen_state` roots,
  demonstrating a potentially broad structural blast radius. One attempted
  refinement requires **owned `Rebind` in block A,
  maybe-unbound `Delete` in a different block B, and no independently
  planned nonterminal owned `ReleaseLocal` for the same `LocalLocation`**.
  The run fails on a **Profile-mode** exceptional iterator leak before its
  sequential fixture reaches Apply; therefore no claim that this filter
  fixes or worsens Apply is supported. Further inspection also disproves its
  premise: the actual `local_assignment` iterator and next-value temporaries
  have only **terminal `ReleaseLocal` actions with reason `Raise`**, not
  independently planned nonterminal owned releases. A nonterminal-release
  exclusion could not remove those locations. The exception leak is the
  only directly observed outcome for that attempt, and the actual Apply
  use-after-free mechanism remains unresolved.

  A subsequent source-level hypothesis proposed that a cleanup-root
  `StackMirror` might forward a borrowed alias into
  `drop_forwarded_local_names` and receive an invalid dispatch decref.
  **Runtime instrumentation disproves this explanation**: every actual
  `local_assignment` exception-dispatch plan was inspected with roots
  restored, and **zero rooted names** appeared in its forwarded-drop list.
  The source shape may warrant independent review, but it is not the
  observed Apply failure. No retained-root fourth-sink repair or dispatch
  production change is justified by this evidence. The actual optimized
  use-after-free/double-release path remains unresolved; runtime semantic
  GREEN is still pending.
- No new public crate API, environment variable, runtime helper, global
  mutable state, benchmark-specific admission, or fallback-compatibility
  abstraction is proposed.

## Benchmark protocol and coverage

- Acceptance criterion: the complete pyperformance suite, fixed before
  comparison, with paired stock/SOAC geometric speedup of at least **1.10x**.
  A selected subset is diagnostic and cannot satisfy the full-suite target.
- Fixed exploratory benchmark set: **eight workloads**:
  `chaos,comprehensions,deltablue,fannkuch,float,nbody,richards,spectral_norm`.
  Fresh normal-sampling baseline artifact:
  `work/pyperformance/comparison-20260818-194002-hXpGuV/summary.json`;
  all **8 / 8** complete in **515.48 seconds** with paired-stock geometric
  ratio **0.281167076589324x**. Because each full comparison is expensive,
  use the fixed `chaos,comprehensions,spectral_norm` subset for intermediate
  iteration, then validate the complete fixed eight before any broad claim.
- Hardware: **8 CPUs / 12 GiB**, Linux guest kernel **6.8.0-137**. Generate
  independent profile evidence for baseline and candidate on the same
  resources/kernel; prior pre-reboot results are not directly comparable.
- Prior same-hardware fixed-four context:
  `work/pyperformance/comparison-20260818-185154-TACah3/summary.json`;
  paired-stock geometric ratio **0.2735422874x**. This provides existing
  workload/hotspot context but does not replace the pending fixed-eight
  baseline.
- The completed eight-workload baseline transforms `__main__` and
  `soac.runtime` for each benchmark, with **35 `chaos` / 21
  `comprehensions` / 79 `deltablue` / 1 `fannkuch` / 9 `float` /
  9 `nbody` / 53 `richards` / 10 `spectral_norm`** compiled functions and
  **no transformed standard-library module**. Benchmark completion remains
  distinct from meaningful transformed hot-path coverage.
- Record paired stock, previous SOAC, candidate SOAC, robust medians,
  pyperf significance, source/worker coverage, serialized BlockPy, optimized
  typed IR, emitted native bytes, machine blocks, and setup time.
- Debug-single-value smoke establishes completion/coverage only; attached
  native profiles establish attribution only. Headline performance must use
  unattached, normally sampled Apply-mode results.
- Candidate artifact, native post-change profile, and full correctness gate:
  **not run**, because the candidate never passed Profile→Apply safety.
  No finalized performance-log entry is appropriate for a rejected change.

## Measurements

| Benchmark | Prior fixed-four SOAC median | Fixed-eight paired stock mean | Fixed-eight SOAC mean | Fixed-eight stock / SOAC | Candidate |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 72.0015015 ms | 31.7881268 ms | mean 64.5569748 ms; median 63.9865815 ms | 0.492404x | pending |
| `comprehensions` | 85.5721665 microseconds | 8.1164533 microseconds | mean 77.5630500 microseconds; median 77.1133052 microseconds | 0.104643x | pending |
| `deltablue` | 4.3779513 ms | 1.6114451 ms | mean 3.9715783 ms; median 3.9610381 ms | 0.405744x | pending |
| `fannkuch` | unavailable | 199.1649016 ms | mean 240.2776810 ms; median 238.5309770 ms | 0.828895x | pending |
| `float` | unavailable | 34.9068753 ms | mean 52.2848398 ms; median 51.1992745 ms | 0.667629x | pending |
| `nbody` | unavailable | 51.4768049 ms | mean 885.5229094 ms; median 885.2678085 ms | 0.058132x | pending |
| `richards` | 40.2460413 ms | 22.4372471 ms | mean 41.3079500 ms; median 40.9855660 ms | 0.543170x | pending |
| `spectral_norm` | unavailable | 51.7913849 ms | mean 484.4061489 ms; median 476.9289910 ms | 0.106917x | pending |

The prior fixed-four paired-stock geometric ratio is **0.2735422874x**;
its `chaos` mean contains a severe **355.788 ms** outlier, so robust medians
must accompany means. The completed fixed-eight stock ratio is
**0.281167076589324x**; candidate comparison, significance, and complete-
suite acceptance remain pending.

| Generated-code / hotspot metric | Prior fixed-four result | Fixed-eight baseline | Candidate |
| --- | --- | --- | --- |
| Optimized typed-IR final basic blocks | 2,541 | 3,389 | pending |
| Optimized typed-IR function instances | 193 | 222 | pending |
| Pre-optimization serialized BlockPy bytes | 8,171,456 | 14,378,912 | pending |
| Apply-mode emitted native bytes | 18,876,080 | 26,752,920 | pending |
| Apply-mode native machine blocks | 1,257,800 | 1,782,930 | pending |
| Compiled eager-comprehension bodies | 7; 107,628 bytes / 7,003 blocks | pending | pending |
| Closure creation, inclusive sampled CPU | 28.30% | pending | pending |
| Vectorcall registration, inclusive sampled CPU | 6.52% | pending | pending |
| Lazy generator factory, separate inclusive hotspot | 21.13% | pending | pending |

## Attempt history

### Attempt 1: Inline provably safe direct synchronous eager comprehensions

- Change: proposed conservative lowering of direct local-assignment/return
  list/set/dict comprehensions into their existing parent function, using
  fresh disjoint iteration slots and explicit result ownership; preserve the
  untouched synthetic-function fallback for unsupported scopes/expressions.
- Evidence: vendored modern CPython/PEP 709 inlines eager comprehensions;
  existing SOAC synthesizes and registers a function for every evaluation;
  zero-loss native profiling attributes **28.30%** inclusive to closure
  creation and **6.52%** to vectorcall registration. Seven redundant
  synthetic bodies consume **107,628 bytes / 7,003 machine blocks**.
- Compatibility: CPython's lack of synthetic watcher CREATE events,
  iterable/filter/key/value ordering, target isolation, nested fresh slots,
  source-visible `locals()` / `vars()` / `eval` / `exec` / frame / tracing
  behavior, iterator/partial-container finalizers, `_dp_tmp` transfer,
  exceptions, closures, walrus/global/nonlocal/async fallback, and lazy
  generators all require direct regression evidence. Activation
  introspection during fresh-target renaming remains explicitly unresolved.
- Tests/measurements: the unchanged-production watcher regression genuinely
  fails in **1.23 seconds**: stock creates **zero** eager-comprehension
  functions, while SOAC creates **five**. It also captures a preexisting
  successful-iteration finalizer order mismatch: stock
  `[iter, drop-iter, drop-old-y, drop-item, after]` versus SOAC
  `[iter, drop-item, drop-iter, drop-old-y, after]`, plus iterator/next/body
  exceptional paths. The fresh fixed-eight same-resource baseline completes
  in **515.48 seconds**, with stock ratio **0.2811670766x**, **3,389 typed
  blocks / 222 functions**, **26,752,920 native bytes**, and **1,782,930
  machine blocks**. A new structured lowerer test passes **1 / 1**
  (**368 filtered**) with zero synthetic helpers for direct/nested
  list/dict/set, trusted iter/next, quiet deletes, conservative direct
  introspection rejection, and Ruff empty-set handling. Full watcher /
  finalizer Profile→Apply initially fails on a `{{}}` empty-dictionary
  template (an unhashable set containing a dict); correcting it to `{}`
  exposes a distinct ownership failure. Although watcher events become
  `created=[]` and result values match, the JIT cleanup-root predicate
  excludes compiler-generated owned locals, omitting `drop-item` and other
  required callbacks. A general planning-only root criterion is authorized:
  validate an actual owned `Rebind` plus an explicit `Delete` of the same
  `LocalLocation` in different basic blocks; existing `MaybeOwned` handles
  unbound finally paths. Its independent structured
  `owned_internal_locals_with_explicit_cross_edge_cleanup_have_roots`
  regression genuinely fails with missing `_dp_tmp_1` root, then passes
  (**one passed; 547 filtered**) after the general cross-block ownership
  correction; same-block temporaries and try-carrier exclusions remain
  unchanged. A name-prefix workaround and changes to
  `ownership_effects` are prohibited. Existing stock/SOAC
  traceback-retained iterator differences on `__iter__` / `__next__` are
  covered by explicit prior-SOAC event assertions. The corrected general
  root criterion and next-only exception cleanup make the complete Profile
  watcher/value/finalizer matrix GREEN. Apply remains RED with ignored
  `Payload.__del__` `StopIteration` / `str + NoneType` `TypeError` and an
  overlap-fallback assertion failure, suggesting a still-unproven
  specialization-owned lifetime error. The next candidate further narrows
  the general cross-block criterion to a `Delete` with planner
  `old_state == Unbound`, preserving existing must-bound/transported
  temporaries without using names. The narrowed structural check still passes
  (**one passed; 548 filtered**), and Profile remains GREEN, but Apply
  corruption persists; the must-bound-root hypothesis is therefore rejected.
  Function-specific diagnostics prove `Payload` / `Values` gain no internal
  roots; Apply `local_assignment` gains four expected comprehension roots but
  emits up to **204 repeated iterator finalizers** in Apply/Verify,
  unlike exactly-once Profile. `escaped_capture` gains **five** preexisting
  typed generator-state roots, exposing broad blast-radius risk. Full semantic GREEN,
  candidate comparison, post-change native profiling, and full gate remain
  pending. Excluding every local with an existing nonterminal owned release
  causes an observed **Profile-mode** iterator leak; the sequential fixture
  stops there and never executes Apply, so the earlier supposed Apply fix
  was not verified. Actual iterator/next locals have only terminal
  `ReleaseLocal(reason=Raise)` actions, disproving the proposed nonterminal-
  release explanation. A later borrowed-root exception-dispatch hypothesis is also
  **disproved**: instrumentation of every actual `local_assignment`
  exception-dispatch plan finds **zero rooted forwarded-drop names**. Do not
  attribute the use-after-free to that plausible source shape or add an
  unsupported fourth-sink/dispatch change. The actual Apply ownership root
  cause, full semantic GREEN, and performance remain unproven.
- Result: **REJECTED**. The lowerer and general cleanup-root structured
  regressions each passed after genuine REDs, and Profile eliminated all
  watcher-created synthetic functions while preserving required values and
  lifetimes. Nevertheless, Apply/Verify exhibited reproducible
  use-after-free/double destruction, with approximately **204 iterator
  `__del__` calls from one normal invocation**. Broad root promotion also
  affected **five** unrelated existing typed generator-state locals.
  Actual iterator/next actions were only terminal
  `ReleaseLocal(reason=Raise)`, disproving the supposed nonterminal-release
  explanation. A release-filter variant failed in **Profile** before Apply
  executed, so no Apply success/failure claim can be drawn from that run.
  Instrumenting every actual exception dispatch found **zero rooted names**
  in `drop_forwarded_local_names`, disproving the proposed borrowed-root
  dispatch cause. No sound general path-sensitive owner/root reconciliation
  was established. All candidate lowerer/JIT changes and newly added tests
  were reverted; no performance comparison, post-change profile, or full
  candidate gate was justified.

## Verdict and next action

- Verdict: **REJECTED**. Although PEP 709 semantics, zero-loss profiling,
  the genuine five-versus-zero watcher RED, and structured lowerer/ownership
  RED→GREEN tests establish a legitimate general-purpose opportunity, the
  attempted implementation is unsafe in optimized Apply and Verify modes.
  Its **204 repeated iterator finalizers** prove a severe ownership failure,
  and five unrelated existing generator-state roots show broad blast radius.
  Full Profile correctness does not compensate for unsafe optimized
  execution. The fixed-eight baseline remains **0.2811670766x** stock, but
  there is **no candidate measurement, speedup, full gate, or retained
  production/test change**. The complete-suite **1.10x** target is unmet.
- Transferable lesson: modern CPython inlines eager comprehensions but still
  preserves isolated targets, exact destruction, and separate lazy-generator
  behavior. Equal values, zero watcher events, structural tests, and even
  complete Profile-mode correctness do not establish Apply/Verify ownership
  safety. Cleanup roots and terminal raises require path-sensitive ownership
  reasoning; inspect actual executed plans before attributing failures, and
  never claim Apply success when a sequential test stopped in Profile.
- Next action: pursue a different independently documented optimization;
  revisit eager inlining only after a separate, generally sound
  path-sensitive ownership model can prove exactly-once cleanup on all
  normal and exceptional edges.

## Attempt 2: callable eager child without parent ownership changes

- Current status: **GENUINE FULL-PRODUCTION SYNTHETIC-EAGER DIRECT-PLAN
  OPTIMIZER AND ACTUAL STOCK-VS-SOAC WATCHER / AUDIT INTEGRATION
  RED-TO-GREEN; COMPLETE THREE-FILE CALLABLE IMPLEMENTATION COMPILES;
  TWO LEGITIMATE LEGACY ARTIFACT ORACLES MIGRATED; COMBINED TRANSFORMED
  FIXTURES 3 / 3, OPTIMIZER 213 / 213, JIT 568 / 568 GREEN; BROAD
  TRANSFORMED 83 PASSED / 7 DESELECTED; SCOPED FORMAT / COMBINED TEST
  CHECKS PASS; FINAL POST-FORMAT INTEGRATIONS 3 / 3; REPEATED TARGET
  IMPROVEMENT VERIFIED; FULL AUTHORITATIVE GATE GREEN; LANDED
  CANDIDATE / RETAIN**.
  Attempt 1 and its severe
  **204-finalizer** use-after-
  free verdict remain explicitly **REJECTED** and are preserved verbatim
  above. The complete three-file implementation compiles and the actual
  stock/SOAC watcher/audit regression, repeated candidate benchmarks, and
  full authoritative correctness gate all pass.
- Pacific date: **2026-08-19 PDT**.
- Current baseline: integrated `main` change **`yywuowlk`**, commit
  **`30b58df5`**, a **documentation-only** child of the latest actual
  production change **`zwkrytkq/443b2e42`**. Existing fixed-eight
  comparison **090414** and targeted comparison **090720** therefore remain
  valid code baselines. Working candidate: change **`wnlnpkrp`**, commit
  **`e79ab200`**.
- Current fixed-eight stock geometry is **0.5883463026285985x**; targeted
  four-workload stock geometry is **0.4290269750586277x**. Existing Apply
  coverage is **23,293,040 native bytes / 1,533,550 machine blocks** and
  **2,866 optimized typed blocks / 204 functions**. The complete-suite
  **1.10x stock** goal remains unmet.

### Current source and profile evidence

- Fresh retained zero-loss profiles include **618 comprehensions samples**
  and **599 chaos samples**. Existing eager synthetic-function factory
  ancestry contributes approximately **13.273%** of comprehensions,
  comprising **5.018% parent** and **8.255% nested** eager contexts, and
  approximately **7.012%** of chaos. These are measured whole-workload
  ancestry regions, not promised or mechanically removable gains.
- Separate **7.445%** lazy generator-expression factory ancestry must be
  **excluded** from this eager strategy; generator laziness, independent
  code, and observer behavior retain their original path. Avoid adding
  overlapping profile parents, children, registration, or call costs.
- Modern stock CPython / PEP 709 creates **zero** eager-comprehension
  `PyFunction` objects and emits no synthetic function/code creation audit
  events. The newly frozen same-interpreter regression observes **six SOAC
  eager CREATE events**—list, set, dict, nested dict, and the nested inner
  list twice—plus `code.__new__` audit events; both stock watcher and code
  audit lists are empty. The historical Attempt 1 fixture observed five
  creations under its different source shape. This is a genuine current
  user-visible compatibility defect, not an optimization-only benchmark
  assertion.
- The new `tests/test_eager_comprehension_function_elision.py` initially
  exposed a separate preexisting failing-body finalizer-order mismatch:
  stock emits **`(iter, read, caught, drop-item, drop-iterator)`**, while
  unchanged SOAC emits **`(iter, read, drop-item, caught, drop-iterator)`**.
  The fixture now explicitly preserves existing SOAC ordering and verifies
  exactly-once destruction. A second unrelated preexisting nested Verify
  exact-int scalar bug cannot load captured `offset`; the fixture avoids it
  by using captured tuple pairs without masking the target behavior.
- Final unchanged-production focused integration genuinely fails **1
  test**, solely at intended stock/SOAC eager watcher/audit parity, after
  **all** Profile, Verify, and Apply subprocesses succeed. Real profile
  evidence, native parent and separate synthetic-child bodies, **five
  factory callbacks**, **one factory-code mutation**, both runtime aliases,
  cache reentry and runtime-module replacement, trace/profile/local/global
  monitoring each once, forced interpretation, source/generator/spoof
  controls, GC, and finalizers all pass. This frozen integration therefore
  establishes an actual CPython-visible defect while preserving existing
  SOAC lifetime behavior.
- A second independent genuine unchanged-production structured regression,
  `direct_call_plans_preserve_eager_comprehension_callable_elision`, uses
  real lowering, `ProfileEvidenceStore`, and complete production
  `plan_and_emit_module_v3_from_raw_evidence`. Generated eager list/set/
  dict direct-call instruction IDs **`[12, 24, 36]`** are incorrectly
  selected; ordinary and source-spoof direct calls **`[60, 63]`** remain
  correctly selected, and lazy generator behavior is represented. Focused
  optimizer result is **0 passed / 1 failed**, solely at the intended final
  eager-direct-plan assertion. Both actual integration and optimizer REDs
  precede any production behavior change; the exact three-file
  implementation begins only afterward.
- The same actual full-production optimizer regression now turns
  **RED-to-GREEN: 1 focused test passed**. Its semantic decline applies
  only to a same-module exact compiler-generated list/set/dict display with
  a numeric generated bind, a child JIT function whose sole input is the
  compiler-owned **`_dp_iter_*`**, and the actual matching parent-local
  `Store(MakeFunctionWithClosure target)` followed by the matching `Call`.
  Eager generated direct IDs **`[12, 24, 36]`** are declined; ordinary and
  source-authored prefix-spoof IDs **`[60, 63]`** remain directly planned,
  and lazy behavior remains represented. This planner GREEN alone does not
  establish runtime semantics; the separate frozen real watcher/audit
  integration independently turns GREEN only after the JIT callable path.
- The definitive frozen
  `tests/test_eager_comprehension_function_elision.py` integration now
  turns **genuine RED-to-GREEN: 1 passed in 12.61 seconds**. In the same
  process, stock watcher CREATE and `code.__new__` lists are empty, and the
  transformed candidate likewise records **zero / zero** across actual
  Profile, Verify, and Apply subprocesses. Nested list/set/dict shapes and
  captured cells, **five live factory callbacks**, factory `__code__`
  mutation, both aliases, cache subclass/reentry, runtime-module swap,
  source/lazy/spoof fallbacks, trace/profile/local/global monitoring,
  forced interpretation, existing finalizer orders **exactly once**, GC
  cycles, mixed synthetic direct-edge safety, and unrelated profiled/native
  execution all pass.
- One focused-test-only oracle originally assumed an unrelated function
  must emit a separate direct-edge event, but the existing optimizer
  legitimately inlines that call. The oracle now proves the actual
  unrelated profiled target and compiled native body plus another ordinary
  direct edge; it does not alter production behavior or weaken the
  preserved unrelated-direct-call requirement. Prior Attempt 1's unsafe
  parent ownership changes are not repeated.
- Two existing legacy tests then genuinely fail because they intentionally
  asserted the now-eliminated synthetic artifacts:
  `tests/test_synthetic_closure_code_cache.py` expects canonical
  **`code.__new__ == 1`** but observes the stock-correct **0**, and
  `tests/test_synthetic_function_metadata.py` expects canonical watcher
  **CREATE == 3** but likewise observes **0**. These are expected old-test
  oracle failures after the real CPython parity correction, not silently
  ignored regressions.
- Exactly those two test oracles are narrowly migrated and frozen with clean
  host ASTs. Their canonical path now requires **zero synthetic watcher
  CREATE and zero code audit**. Metadata coverage additionally forces the
  existing real fallback using a benign subclass of the **original**
  bootstrap cache, then still verifies **three genuinely distinct
  `PyFunction` objects**, independent closure cells, and correct
  name/qualname metadata. All original factory callbacks, cache reentry,
  and source-function checks remain intact. The new stock-parity
  integration plus both deliberately migrated synthetic metadata/cache
  suites now jointly pass **3 / 3 in 7.06 seconds**. Canonical watcher and
  code-audit counts are zero, while benign original-cache-subclass fallback
  still yields three genuinely distinct Python functions, closure cells,
  and correct name/qualname metadata; all live mutation controls pass.
- Attempt 1 demonstrates why changing the parent CFG is prohibited: moving
  child operations into the parent's ownership/control graph produced
  approximately **204 iterator finalizers from one Apply/Verify call**.
  Attempt 2 must preserve the existing parent `Store`, `Delete`, control
  flow, cleanup roots, iteration, and current parent/child ownership; the
  eager child remains separately compiled and callable.

### Compiling three-file source-backed architecture; real semantic integration passes

- Prepare **one shared per-template `PyCapsule`** carrying an `Arc` to the
  ready, separately compiled eager child. This capsule owns no Python
  objects or hidden GC roots; it is shared compiler execution metadata,
  not a callable substitute or a Python closure.
- For each actual eager comprehension invocation, allocate a normal
  GC-traversed tuple containing the **shared capsule, current globals,
  current builtins, and existing capture objects**, and bind that tuple as
  the self-state of a genuine **`METH_O` `PyCFunction`**. The resulting
  object remains a real Python callable through vectorcall, `tp_call`,
  generic fallback, and deoptimization. It is neither a bare noncallable
  capsule nor an untraversed structure hiding Python-owned roots.
- The C callback builds a stack `FunctionEnvAbiHeader` from borrowed exact
  tuple slots, with ABI slot zero **NULL**, validated exact capture cells,
  late-owner/deoptimization compatibility, matching CPython recursion and
  `tp_call`, and at most **eight captures**. It invokes the already-ready
  compiled child without changing its body or ownership. Immutable shared
  `Arc<CompiledFunctionHandle>` metadata must not retain Python roots;
  tuple traversal preserves globals/builtins/captures, cyclic GC, mutation,
  and finalization. No parent CFG inline, fresh root promotion, temporary
  Python attribute, thread-local ownership, or hidden refcount transfer is
  permitted.
- Capture the authoritative pristine bootstrap factory code and its cache
  snapshot during runtime **`_dp_module_init` registration before the
  module body runs**, then require original exact live factory/cache
  identity, current factory code, and runtime module type/version on each
  eligible use. This rejects later factory/cache replacement, cache
  subclassing/reentry, pre-first-call wrapping, and runtime-module
  replacement rather than trusting mutable state captured too late.
- Explicit bounded cache compatibility limitation: arbitrary **in-place**
  mutation of the original `_DP_CODE_WITH_FREEVARS_CACHE` contents is not
  checked. Original-cache replacement, subclassing, and reentry are
  rejected/fallback, but ordinary in-place mutation under the same exact
  cache object remains outside this approved private-cache boundary.
- Existing hot parent direct-call plans currently assume the exact
  synthetic target is a `PyFunction`; a `PyCFunction` has runtime function
  identity **0**, and mixed Profile evidence may otherwise deopt. A narrow
  semantic planner decline for compiler-owned **synthetic eager** direct
  targets in `crates/soac_opt/src/pipeline_v3.rs` is planned alongside
  private implementation in `crates/soac_jit/src/lib.rs` and
  `crates/soac_jit/src/function_instantiation.rs`: exactly **three
  existing production files**. Preserve unrelated real-function direct
  calls and existing parent ownership. The real production-path structured
  planner independently turns GREEN **1 / 1** using the semantic
  source/parent/child evidence above. Full three-file
  `cargo check -p soac_jit --tests` now **passes** against the actual
  workspace-patched guest PyO3, which exposes `METH_O`, `PyMethodDef`,
  `PyCapsule`, and `PyCFunction` APIs. The frozen watcher/audit transformed
  integration independently passes **1 / 1 in 12.61 seconds**, proving
  real CPython-visible watcher/audit parity and callable ownership.
- A cheap immutable child display/kind/single-parameter eligibility guard
  now runs **before** the original-code `HashMap` lookup, so unrelated
  ordinary function registrations do not pay the new synthetic-only lookup
  cost. This is a production guard-order property, not a measured
  performance claim.
- Preserve canonical live runtime module/factory identity, existing code
  and ready-entry cache/session checks, exact captured builtins, module /
  helper mutation, tracing/profile/global+local monitoring hooks,
  force-interpreter fallback, source-backed real functions, and every lazy
  generator path. Require strict compiler-child source provenance with
  **no original code object**, exact captured builtins, and parent source
  trace/profile/global+local monitoring checks. Preserve original eager
  iterable/next/body ordering,
  `Store`/`Delete` behavior, dictionary key/value order, exceptions,
  iterators/items/outer-local finalizers, class/module behavior, and stock
  watcher/audit parity without executing annotations or user callbacks
  earlier.
- Pinned CPython implementation details and vendored CPython changes are
  permitted by the user when semantics are preserved. However, the running
  interpreter exposes custom APIs absent from the checkout; rebuilding
  vendored CPython from the visible source could silently remove those
  extensions and is **unsafe without verified build provenance**. Avoid
  interpreter rebuilds for this attempt unless that discrepancy is resolved.
- Prefer no new public API, runtime helper, global mutable state, typed-IR
  concept, or runtime-helper inventory change. Both unchanged-production
  REDs are confirmed, the optimizer RED turns GREEN, and the full three-
  file callable implementation compiles, and the real watcher/audit
  integration passes **1 / 1 in 12.61 seconds**. Two genuinely obsolete
  legacy watcher/code-audit expectations are narrowly migrated, retaining
  real fallback metadata, three distinct Python functions/cells, callback,
  reentry, and source checks. Their combined three-fixture rerun passes
  **3 / 3 in 7.06 seconds**. Complete affected Rust libraries then pass
  serially: `cargo test -p soac_opt --lib` **213 / 213** and
  `cargo test -p soac_jit --lib` **568 / 568**. Broad real transformed
  compatibility then passes **83 tests / 7 conventionally deselected in
  44.74 seconds**, covering eager stock watcher/audit across
  Profile/Verify/Apply, both migrated metadata/cache paths, source
  closures, lazy generators, captured custom mappings/module builtins,
  function/code/default mutation, live StopIteration guards/monitoring,
  constructor/direct-call cleanup, inherited/non-self/scalar paths, broad
  import, class cells, and coroutines. Scoped
  `just fmt-rust-check soac_opt soac_jit` and combined
  `cargo check -p soac_opt -p soac_jit --tests` both pass; the structured
  optimizer regression also passes again after formatting. The final
  post-format three-integration rerun passes **3 / 3 in 7.55 seconds**.
  Subsequent repeated benchmarks and the complete correctness gate both
  confirm the candidate.
- Release debug-single fixed-eight smoke comparison **112443** against the
  retained guarded-runtime baseline **090221** passes **8 / 8** with zero
  worker errors. Two independent measured-Apply-PID JIT-code-summary
  audits confirm all **204 direct compiled bodies / 397 total entries**
  remain present, every `(entry_kind, qualname, function_id)` is unchanged,
  every separate synthetic child remains compiled, and optimized typed
  coverage stays **2,866 blocks / 204 functions**. Aggregate emitted code
  decreases **2,253,100 -> 2,242,168 bytes (-10,932 / -0.4852%)** and
  **148,734 -> 148,116 machine blocks (-618)**.
- Comprehensions smoke code decreases
  **274,348 -> 267,704 bytes (-6,644 / -2.42%)**, exclusively in parent
  `_add_widgets` **18,876 -> 15,456 (-3,420)**, nested dict comprehension
  17 **16,572 -> 14,968 (-1,604)**, and `make_some_widgets`
  **92,444 -> 90,824 (-1,620)**. Chaos decreases
  **681,500 -> 677,908 bytes (-3,592)** across parent `Spline` **-612**,
  `create_image` **-568**, and `Chaosgame.__init__` **-2,412**. Spectral
  decreases **170,128 -> 169,432 bytes (-696)** in `eval_times_u`.
  Every deltablue/fannkuch/float/nbody/richards function remains byte-
  identical; cold smoke timings and arithmetic ratios are not throughput
  evidence.
- Benchmark-summary unique-name coverage is unchanged for every workload:
  chaos **32 -> 32**, comprehensions **19 -> 19**, deltablue **76 -> 76**,
  fannkuch **1 -> 1**, float **7 -> 7**, nbody **6 -> 6**, richards
  **51 -> 51**, and spectral **7 -> 7**. For comprehensions, the
  independently authoritative native JIT summary also confirms **24 -> 24
  distinct compiled bodies / 38 -> 38 total entries**. Unique summary
  names and JIT compiled-body counts are different inventories; there is
  **no coverage loss or 24-to-19 change**.
- Normally sampled fixed-eight comparison **112949** against retained
  comparison **090414** completes **8 / 8**. Candidate stock geometry is
  **0.5896760656259606x** versus baseline **0.5883463026285985x**;
  official previous-SOAC arithmetic geometry is **0.955006696914243x**,
  contaminated by extreme candidate outliers including deltablue
  **13.9 ms**, richards **97.7 ms**, and nbody **91.6 ms**. This
  unfavorable official result must not be omitted. Independent worker-
  bootstrap robust fixed-eight geometry is **1.011265x raw / 1.033794x
  stock-adjusted**, separating median behavior from arithmetic outliers.
- Target comprehensions median improves **54.2567 -> 50.4265 us**, raw
  **1.0759568x [1.04479, 1.15648]**, or **1.1446674x stock-adjusted
  [1.07696, 1.22948]**; matched stock drift **0.93997x** materially
  inflates the paired estimate and must be disclosed. Chaos is neutral at
  **0.976784x [0.94254, 1.02452] / 0.975345x paired**. Spectral is raw-
  neutral **1.02774x**, or **1.06743x paired [1.008, 1.107]**; richards
  is raw **0.96203x [0.897, 0.988]** but paired-neutral **0.98133x**
  amid environment outliers; deltablue is approximately **1.007x raw /
  1.012x paired**, also neutral.
- Normal fixed-eight native code decreases
  **23,293,040 -> 23,188,640 bytes (-104,400)** and
  **1,533,550 -> 1,527,950 machine blocks (-5,600)**, while optimized
  typed coverage remains **2,866 blocks / 204 functions** and summary
  coverage sets are unchanged. Independent auditing confirms all **80
  actual measured worker PIDs**, unchanged complete **204-body** coverage,
  and zero errors. Normal per-worker shrinkage is chaos **-3,808 bytes**,
  comprehensions **-5,936**, and spectral **-696**; the other five
  workloads remain byte-identical. Overall byte reduction is **0.4482%**.
- Targeted three-round comparison **113536** against retained comparison
  **090720** measures **60 candidate / 60 baseline values**.
  Comprehensions median improves **52.4194 -> 49.9408 us**, raw
  **1.0496318x [1.027465, 1.076117]**, or **1.0520710x stock-adjusted
  [1.027128, 1.080952]**; per-round raw ratios are
  **1.08915x / 1.09505x / 0.99357x**. Chaos is neutral at
  **0.997188x raw / 0.99898x paired**, deltablue
  **0.98517x / 0.98291x paired** with intervals crossing one, and richards
  **1.00107x / 1.00134x paired**. Four-workload robust geometry is
  **1.00797x raw / 1.00850x stock-adjusted**; official arithmetic geometry
  is **1.0272793682532497x** and is distinct from robust medians.
- Independent targeted auditing confirms every one of **120 measured
  Apply PIDs** retains the full compiled-body inventory, with zero errors.
  Aggregate native code decreases
  **55,058,040 -> 54,765,720 bytes (-292,320 / -0.5309%)**, and machine
  blocks decrease **3,620,520 -> 3,604,800**. The real stock watcher/audit
  compatibility fix, significant repeated target improvement, and smaller
  code support **LANDED CANDIDATE / RETAIN** status. Matched zero-loss
  profiles below independently confirm the mechanism; the authoritative
  full correctness gate passes, and the complete-suite stock goal remains
  unmet.

### Attempt 2 matched zero-loss profiles

- The comprehensions profiles use **50,000 loops at 199 Hz**, with
  **618 -> 692 raw samples and zero losses**. Whole make-function ancestry
  decreases **20.718% -> 11.999%**, shared instantiation
  **19.262% -> 10.265%**, and the old Python-function instantiator
  **16.674% -> 3.614%**. Eager-only parent ancestry falls
  **13.273% -> 6.074%**: `_add_widgets` decreases **5.018% -> 2.169%**
  and its nested dictionary comprehension **8.255% -> 3.905%**. The
  distinct lazy generator-expression source remains on its original path;
  its normalized share changes **7.445% -> 5.925%**, not to zero.
- Registration decreases **4.532% -> 1.446%**, metadata
  **3.723% -> 0.869%**, synthetic-code creation **1.781% -> 0%**, and
  Python-function allocation **1.619% -> 0.434%**. Replacement capsule
  ancestry changes **0.324% -> 1.013%**, tuple ancestry
  **0.324% -> 0.579%**, and candidate original-code lookup is **0.433%**.
  The callback's **81.215% inclusive** share contains the actual compiled
  child execution; callback self cost is only **0.145%**. Profile shares
  overlap and must not be added or mistaken for independent costs.
- GC remains nearly unchanged at **14.398% -> 14.446%**, but kernel
  page-clearing rises **4.204% -> 12.703%** during the attached run.
  Consequently the attached replay **57.689 -> 64.188 us** is a
  confounded diagnostic, not a throughput result; the authoritative
  repeated median remains **52.4194 -> 49.9408 us (1.0496318x)**.
- Chaos profiles use **70 loops at 199 Hz**, with
  **599 -> 690 raw samples and zero losses**. `Spline` eager ancestry
  decreases **7.179% -> 4.494%**, while its old instantiator decreases
  **5.677% -> 0%**, registration **1.336% -> 0%**, and synthetic-code
  creation **1.002% -> 0%**; replacement tuple/capsule work remains.
  Despite the confirmed mechanism, paired repeated chaos throughput is
  **0.99898x**, statistically neutral.

### Attempt 2 authoritative full correctness gate

The completed `just test-all` run recorded in
`work/logs/eager-comprehension-callable-test-all.log` **passes all 1,228
Python nodeids across 91 / 91 isolated file batches on eight workers**,
with **zero failed batches**. The Rust suites pass **568 JIT**, **213
optimizer**, **371 lowering**, **54 typed-IR**, and **8 PyO3** tests. The
Cargo phase takes **91.634 seconds**, pytest **81.696 seconds inner /
81.722 seconds outer**, and the complete test phase **173.377 seconds**.
The new stock-CPython watcher/audit parity integration passes in
**9.10 seconds**; the existing 28-test counter-dump batch takes
**80.81 seconds**. Attempt 2 is **LANDED CANDIDATE / RETAIN**; main
integration remains a separate root-owned action. Historical Attempt 1
remains **REJECTED**, and the full-suite **1.10x stock** target is unmet.

### Attempt 2 measurements and validation state

| Metric | Retained production baseline | Attempt 2 candidate | Status |
| --- | --- | --- | --- |
| Fixed-eight stock / SOAC geometry | 0.5883463026285985x | 0.5896760656259606x | full-suite stock 1.10x target unmet |
| Normal fixed-eight official / robust / stock-adjusted previous-SOAC geometry | retained comparison 090414 | 0.955006696914243x / 1.011265x / 1.033794x | official mean contaminated by extreme delta/rich/nbody outliers; report honestly |
| Normal comprehensions median / robust / stock-adjusted improvement | 54.2567 us | 50.4265 us; 1.0759568x / 1.1446674x | raw CI [1.04479, 1.15648]; paired CI [1.07696, 1.22948]; stock drift 0.93997 |
| Targeted three-round comprehensions median / robust / stock-adjusted improvement | 52.4194 us | 49.9408 us; 1.0496318x / 1.0520710x | raw CI [1.027465, 1.076117]; paired CI [1.027128, 1.080952] |
| Targeted four-workload robust / stock-adjusted / official geometry | retained comparison 090720 | 1.00797x / 1.00850x / 1.0272793682532497x | controls neutral; official arithmetic separate from robust medians |
| Targeted fixed-four stock / SOAC geometry | 0.4290269750586277x | 0.43858930692865516x | targeted subset only; full-suite stock goal remains unmet |
| Optimized typed-IR blocks / functions | 2,866 / 204 | 2,866 / 204 | unchanged; all parent/child bodies retained |
| Apply native bytes / machine blocks | 23,293,040 / 1,533,550 | 23,188,640 / 1,527,950 | -104,400 bytes / -5,600 blocks; all 80 measured PIDs/body inventories verified; zero errors |
| Targeted three-round native bytes / machine blocks | 55,058,040 / 3,620,520 | 54,765,720 / 3,604,800 | -292,320 bytes / -0.5309%; all 120 measured PIDs/body inventories unchanged; zero errors |
| Release debug-single fixed-eight native bytes / blocks | 2,253,100 / 148,734 | 2,242,168 / 148,116 | -10,932 bytes / -618 blocks; all 204 direct bodies / 397 entries retained |
| Release debug-single comprehensions native bytes / unique names / compiled bodies | 274,348 / 19 / 24 | 267,704 / 19 / 24 | -6,644 bytes / -2.42%; unique summary names and native body inventories each unchanged |
| Release debug-single chaos / spectral native bytes | 681,500 / 170,128 | 677,908 / 169,432 | -3,592 / -696 bytes; delta/fann/float/nbody/rich unchanged |
| Matched comprehensions / chaos zero-loss raw samples | 618 / 599 | 692 / 690 | same 50,000 / 70 loops at 199 Hz; zero sample losses |
| Eager synthetic factory comprehensions / Spline ancestry | 13.273% / 7.179% | 6.074% / 4.494% | eager-only matched parent ancestry; chaos throughput remains neutral |
| Separate lazy generator-expression factory | 7.445% | 5.925% | original lazy path retained; normalized profile shares vary |
| Existing eager-function watcher CREATE / audit parity | stock CREATE 0 / code.__new__ 0; old SOAC six eager CREATE plus code.__new__ | candidate CREATE 0 / code.__new__ 0 | genuine current user-visible defect fixed; historical Attempt 1 fixture independently observed five |
| Parent ownership / finalizer safety | prior Attempt 1 up to 204 iterator finalizers | PASS; current finalizers run exactly once | parent CFG / Store / Delete / roots unchanged; no repeated finalizers |
| Genuine unchanged-production transformed watcher / audit regression | 1 failed only on stock/SOAC eager CREATE / code audit mismatch | 1 passed in 12.61 s | genuine RED-to-GREEN; Profile/Verify/Apply, factories, aliases, monitoring, GC, exact-once finalizers, and mixed calls pass |
| Genuine production-path synthetic-eager direct planner regression | 0 passed / 1 failed; generated direct IDs [12, 24, 36] incorrectly selected | 1 focused test passed | genuine RED-to-GREEN; exact generated semantic evidence; ordinary/spoof [60, 63] and lazy behavior preserved |
| Complete three-file JIT test-target type check | retained production prior build | `cargo check -p soac_jit --tests` passed | actual patched guest PyO3 exposes METH_O / PyMethodDef / PyCapsule / PyCFunction |
| Actual production callable / capsule / captured-root ownership | old eager path creates six Python functions | frozen real transformed integration passes | nested captures, GC cycles, finalizers, generic call/deopt, and monitoring controls pass |
| Existing synthetic cache / function metadata artifact oracles | prior tests expect code audit 1 / watcher CREATE 3 | canonical expectations migrated to zero; forced original-cache-subclass fallback retains three real functions | both genuine old-oracle failures preserved; metadata/cells/live mutation remain tested |
| Combined stock-parity / migrated metadata / migrated cache transformed suites | watcher/audit parity RED; two stale legacy oracles fail | 3 / 3 passed in 7.06 s | GREEN; zero canonical CREATE/audit and fallback three distinct Python functions/cells |
| Complete affected optimizer / JIT Rust libraries | retained production baseline | 213 / 213 optimizer; 568 / 568 JIT | GREEN; executed serially |
| Broad real transformed compatibility matrix | retained production semantic suites | 83 passed / 7 conventionally deselected in 44.74 s | GREEN; watchers, caches, lazy/source, mutations, StopIteration, cleanup, owner fields, imports, coroutine |
| Scoped optimizer/JIT formatting / combined Cargo test-target check | retained production prior build | both passed; structured optimizer rerun passes | GREEN; final post-format transformed integrations 3 / 3 in 7.55 s |
| Full `just test-all` correctness gate | retained production previously passed | PASS; 1,228 Python nodeids; 91 / 91 batches; 8 workers | zero failures; JIT 568, optimizer 213, lowering 371, typed 54, PyO3 8; full phase 173.377 s |

### Current Attempt 2 verdict and next action

- Current verdict: **LANDED CANDIDATE / RETAIN; GENUINE PRODUCTION-PATH STRUCTURED OPTIMIZER AND
  STOCK-VS-SOAC PROFILE / VERIFY / APPLY WATCHER / AUDIT INTEGRATION
  RED-TO-GREEN; PREVIOUS ATTEMPT 1 REMAINS REJECTED**. Stock emits zero
  CREATE/audit events while actual SOAC emits six eager CREATE plus code
  audit; all transformed mutation, monitoring, alias, parent/child,
  ownership, and existing finalizer-order controls pass. The three-file
  callable tuple/capsule/compiled-child and synthetic-eager direct-plan
  decline now compile with the real patched guest APIs. The actual full optimizer
  path now passes **1 / 1**, semantically declining eager generated direct
  IDs **[12, 24, 36]** while preserving ordinary/spoof **[60, 63]** and
  lazy paths. Runtime `_dp_module_init` captures pristine factory/cache,
  existing-template Arc-only capsules and GC-visible tuples preserve roots,
  and bounded stack ABI headers retain the separately compiled child.
  Arbitrary in-place original-cache contents are explicitly not guarded.
  The frozen real watcher/audit integration passes **1 / 1 in 12.61
  seconds**, restoring stock **zero eager CREATE / zero code audit** with
  full factory/mutation/monitoring/GC/finalizer controls. Two existing
  synthetic cache/metadata tests genuinely failed on old canonical
  **audit 1 / CREATE 3** assumptions and are narrowly migrated to stock-
  correct zero; a benign original-cache subclass preserves fallback
  coverage of three real distinct Python functions/cells and names. The
  combined three-test rerun now passes **3 / 3 in 7.06 seconds**, keeping
  canonical watcher/audit counts zero and fallback metadata/closures/live
  mutation coverage intact. Cheap immutable synthetic-child guards precede
  the original-code `HashMap` lookup to avoid charging unrelated
  registrations. Complete optimizer and JIT Rust libraries now pass
  **213 / 213** and **568 / 568**, respectively. Broad transformed
  compatibility passes **83 tests / 7 conventionally deselected in 44.74
  seconds**, retaining eager/source/lazy/watcher/cache/mutation/monitor /
  direct-call/import/owner/coroutine coverage. Two-package scoped
  formatting, combined Cargo test-target checks, and post-format structured
  optimizer rerun pass. The final post-format three transformed
  integrations pass **3 / 3 in 7.55 seconds**. Fixed-eight release
  debug-single smoke passes **8 / 8** and preserves all **204 direct
  bodies / 397 entries**, every benchmark-summary unique-name set, and
  comprehensions **24 native bodies / 19 unique names**; native smoke code
  decreases **0.4852%**. Cold smoke timings are invalid. The normal fixed-
  eight report has stock **0.5896760656259606x**, outlier-contaminated
  official previous **0.955006696914243x**, robust previous
  **1.011265x / 1.033794x paired**, and comprehensions raw **1.0759568x**
  with a confidence interval above one; strong stock drift inflates paired
  target speedup. Normal generated code decreases **104,400 bytes / 5,600
  blocks**. Final PID/error audit and targeted comparisons independently
  reproduce comprehensions
  **1.0496318x raw / 1.0520710x paired**, with both confidence intervals
  above one; controls are neutral and all **120 Apply PIDs** retain their
  compiled bodies while native bytes decrease **0.5309%**. The candidate
  has matched zero-loss profiles confirming comprehensions eager parent
  ancestry **13.273% -> 6.074%** and chaos **7.179% -> 4.494%**, while
  lazy generators remain on their original path and chaos throughput is
  neutral. The attached comprehensions replay is confounded by increased
  kernel page clearing and is not the throughput headline. Performance
  supports retention, and the authoritative full correctness gate passes
  **1,228 Python nodeids / 91 isolated batches / zero failures**, together
  with **568 JIT / 213 optimizer / 371 lowering / 54 typed / 8 PyO3**
  Rust tests.
- Next action: root-owned integration of the fully validated retained
  candidate; the complete-suite **1.10x stock** goal remains unmet.
