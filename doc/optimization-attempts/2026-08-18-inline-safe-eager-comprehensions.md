---
title: "Inline safe eager comprehensions"
---

# Inline safe eager comprehensions

- Status: REJECTED; real CPython watcher/finalizer failures and structured
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
