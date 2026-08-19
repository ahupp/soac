---
title: "Prefer guarded StopIteration runtime fast path"
---

# Prefer guarded StopIteration runtime fast path

- Status: rejected; attempted optimizer/JIT changes reverted
- Pacific date: 2026-08-18 PDT
- Change: `ourqpywu`, based on retained all-module shutdown flushing.
- Outcome: **REJECTED**. The attempted fast-path expansion skips mutable
  runtime helper/validator globals and therefore hides Python-visible user
  callbacks. CPython dictionary watchers cannot repair the guard because
  SOAC's indexed global stores explicitly bypass watchers and versions.
  Genuine baseline integration was **3 failed / 1 passed in 3.41 seconds**;
  the attempted implementation remained **3 failed / 3 passed in 4.17
  seconds** despite a temporarily GREEN structured optimizer test. All
  candidate optimizer/helper/test changes are discarded, no candidate
  throughput was measured, and no performance gain is claimed. The
  independently discovered synthetic exception-shadow correctness fix is
  tracked separately in
  `2026-08-18-bind-synthetic-iteration-exceptions-to-runtime.md`.

## Hypothesis and evidence

Python loops, comprehensions, generators, and iterator protocols frequently
check whether a caught exception is `StopIteration`. SOAC already has a
narrow guarded native implementation for the compiler-owned
`soac.runtime.exception_matches(exc, StopIteration)` call inside
`crates/soac_jit/src/jit/specialized_helpers.rs`. However, when
`call_hot_targets` identifies the runtime helper, optimizer v3 can select a
profiled guarded/direct call straight into the Python helper, bypassing the
existing generic vectorcall hook and invoking `_validate_exception_type`.

Native zero-loss profiling identifies the opportunity in distinct workloads:

- On `chaos`, generated-JIT-symbol-aware profiling attributes **15.02%
  inclusive** to `exception_matches` and **10.17% inclusive** to nested
  `_validate_exception_type`.
- On restored `comprehensions`, the authoritative 659-sample, zero-loss
  Speedscope profile attributes **6.83% inclusive** to `exception_matches`
  and **5.01% inclusive** to its nested validator. The generated JIT frame
  names are visible in Speedscope even when plain `perf report` text omits
  them.

These are overlapping inclusive call-tree percentages, not additive exclusive
CPU shares. The previous valid comprehension capture is
`work/logs/comprehensions-shutdown-flush-steady_speedscope.json`; prior
post-closure `chaos` profiling is under
`work/logs/chaos-synthetic-closure-cache_speedscope.json`.

The source-backed baseline path is:

1. `soac_opt::pipeline_v3::direct_call_requests_from_evidence_v3` reads
   profiled `call_hot_targets`, resolves the cross-module runtime helper,
   and requests a direct call when ordinary argument binding succeeds.
2. The resulting direct call skips `py_vectorcall_hook` and executes
   `soac.runtime.exception_matches`, which then invokes
   `_validate_exception_type` for the built-in exact `StopIteration` case.
3. Generic JIT calls already reach `py_vectorcall_hook`, then
   `fast_runtime_stop_iteration_match`, which accepts exactly two
   positional/no-keyword arguments, checks the canonical helper identity and
   exact `PyExc_StopIteration`, calls `PyErr_GivenExceptionMatches`, and
   returns an owned Python bool.
4. Existing helper caching must additionally reject user monkeypatches and
   stale code mutation before that native arm can safely replace a direct
   call. The strategy should reuse the existing arm, not add a second runtime
   primitive.

The new focused regression genuinely fails against unchanged production:
`just pytest-fast tests/test_stop_iteration_runtime_fast_path.py -q` reports
**3 failed / 1 passed in 3.41 seconds**. Its compiler-generated nested
`collect` list-comprehension Apply event reports `clif_direct_edges=1`, where
the guarded runtime path requires zero. Two independent semantic failures
also prove the existing fast path is unsafe before the planner is changed.
Monkeypatching
`soac.runtime.exception_matches` before the first cache lookup suppresses the
replacement wrapper's expected side effects (`observed=[]`). Mutating the
already cached canonical function's `__code__` likewise skips the replacement
code's expected side effects (`observed=[]`). Replacing the entire helper
after a canonical warmup already falls back correctly. An initial fixture
also exposed an unrelated preexisting global `StopIteration`-shadow lowering
issue; fixture ordering was adjusted rather than claiming that issue was
fixed by this strategy. The baseline also already preserves handler subclass,
tuple, `ValueError`, invalid-handler, `RecursionError`, and isolated
user-global-shadow semantics.

The independent structured optimizer regression
`pipeline_v3::tests::direct_call_requests_preserve_exact_stop_iteration_runtime_fast_path`
also fails before production changes. Actual selected direct-call source IDs
are `[10, 11, 12, 13, 14]`, while only `[12, 13, 14]` should remain direct:
ID 10 is the explicit-runtime-name match and ID 11 the equivalent
constant-pool runtime alias. The other three cases respectively exercise an
ordinary `ValueError`, a local handler, and a local callback; all three must
keep their existing direct plans.

After the source-proven planner correction, the same structured regression
passes (**1 passed / 202 filtered**). Both runtime-name matches receive an
explanatory `PlanDiagnostic`, while `ValueError`, the local handler, and the
local callback retain their previous direct edges.

The earlier `comprehensions` smoke provides independent real-workload
baseline evidence: `_any_knobby.<locals>.<genexpr>` and each
`_add_widgets` nested list/dict-comprehension body emit
`clif_direct_edges=1`, `function_env_indirect_edges=1`, and
`generic_fallback_edges=2`. The runtime `exception_matches` body itself has
an additional direct edge to its validator. Candidate measurement should
confirm that only the inappropriate nested runtime-match edges disappear;
ordinary constructor, method, and unrelated direct edges must remain.

Critical additional soundness blocker: `soac.runtime.exception_matches`
dynamically resolves `_validate_exception_type`, `isinstance`, and
`RecursionError`; the validator itself resolves mutable `tuple`, `type`,
`BaseException`, `issubclass`, and `isinstance`. Replacing the validator or an
applicable module/builtins global can introduce visible side effects or
change the answer even when the canonical helper and its `__code__` remain
unchanged. The existing fast arm already skips these dependencies, and the
proposed planner change would expose that unsoundness to more Apply calls.
Vendored CPython 3.15 has no usable `ma_version_tag`, while `dk_version`
ignores replacement of existing dictionary values. More decisively,
`crates/soac_jit_runtime/src/lib.rs::store_global_indexed_body` performs a
raw indexed slot replacement and explicitly documents that **watchers and
versions are skipped**. Therefore even CPython dictionary watcher
registration cannot reliably invalidate transformed module-global
dependencies. This rules out the proposed cheap guard and rejects the
optimization without changing default behavior.

The candidate's first expanded six-case integration confirms the blocker:
**3 failed / 3 passed in 4.17 seconds**. Pre-first helper replacement,
post-warm replacement, and in-place canonical `__code__` mutation now pass,
but monkeypatches of `_validate_exception_type` and runtime-global
`isinstance` still have their visible side effects suppressed. Moreover,
the actual generated nested comprehension continues to emit
`clif_direct_edges=1` despite the structured optimizer policy turning GREEN.
The source-confirmed cause is that both
`crates/soac_lowering/src/passes/ast_to_ast/rewrite_expr/mod.rs` and
`crates/soac_lowering/src/passes/ruff_to_blockpy/stmt_sequences.rs` synthesize
lexical `except StopIteration:` / `except StopAsyncIteration:` across all
four sync/async comprehension and ordinary loop cases. Thus compiler-owned
loop exhaustion
incorrectly resolves a user-shadowable global instead of the exact runtime
exception, and the planner correctly sees `Global` rather than
`RuntimeName::StopIteration`. This independently violates CPython behavior
when a module binds `StopIteration = ValueError`, and explains the apparent
structured-unit-versus-production mismatch. The new independent lowerer
regression `synthetic_iteration_handlers_use_compiler_owned_runtime_exceptions`
genuinely fails because the handler is an AST bare `Name` rather than the
private runtime attribute; it covers both sync and async handlers. Replace
only synthetic handlers with `except __soac__.StopIteration:` or
`except __soac__.StopAsyncIteration:` as appropriate; user-authored handlers
remain lexically shadowable. Restoring the previously avoided global-shadow
case from module initialization must prove ordinary-loop and comprehension
correctness plus real sync planner eligibility. Mutable runtime/builtins
dependency soundness remains a separate unresolved blocker.

The expected throughput improvement was never safely measurable: user-visible
mutation correctness failed first. The synthetic-handler correctness issue
has a separate retained strategy and must not be represented as acceptance of
this rejected fast path.

## Attempted implementation and compatibility: reverted

- In both `soac_lowering` synthetic-loop rewrite paths, bind all four
  compiler-generated sync/async comprehension and ordinary-loop exhaustion
  handlers to `__soac__.StopIteration` or `__soac__.StopAsyncIteration`,
  never the user's shadowable globals. Preserve user-authored
  `except StopIteration:` / `except StopAsyncIteration:` lexical semantics.
  Validate a module with `StopIteration = ValueError` from import, both
  ordinary loops and nested comprehensions, both structured sync/async
  handlers, and the resulting explicit sync runtime-name planner provenance.
- In `crates/soac_opt/src/pipeline_v3.rs`, identify the narrow call shape at
  v3 planning time only when `codegen_runtime_name_value_v3` proves the
  callable is `RuntimeName::ExceptionMatches` and argument two is
  `RuntimeName::StopIteration`. Accept explicit runtime locations and module
  constants containing `ConstantExpr::RuntimeName`; require exactly two
  plain positional arguments and no keywords.
- For only that proven source shape, decline the profiled direct-call plan
  with an explanatory diagnostic and retain ordinary generic vectorcall
  lowering, allowing the existing guarded native hook to run. Continue
  recording profile-mode counter evidence; do not alter other direct calls.
- Preserve shadowable globals, local handler aliases, custom `StopIteration`
  bindings, tuple exception handlers, `ValueError` or other expected types,
  keyword/starred argument shapes, and monkeypatched runtime helper calls.
  Their original direct/generic planning and Python behavior must remain
  unchanged.
- Harden `cached_runtime_exception_matches` /
  `fast_runtime_stop_iteration_match` in
  `crates/soac_jit/src/jit/specialized_helpers.rs` using existing
  `PyFunction_GetSoacMetadata` / `PyFunctionJitExtra` provenance. Admit only
  a JIT-registered function whose owning `SharedModuleState` is exactly
  `soac.runtime`, whose lowered function identity is `exception_matches`, and
  whose current Python code object still equals `registered_code`.
- Validate canonical helper provenance before first cache admission and
  recheck only cached-callable / registered-code identity on every fast-path
  use. This avoids repeating owner/name or function-map checks in the hot
  matching arm. A helper wrapper
  installed before the first call, a later replacement, mutation of the
  cached callable's `__code__`, missing SOAC metadata, or changed handler
  falls back to ordinary CPython vectorcall and executes user code.
- Guard every dynamically resolved helper/validator dependency as well.
  Replacement in runtime globals or relevant builtins must invalidate the
  fast arm before user callbacks or changed answers can be skipped.
  Structural-only `dk_version` and nonexistent `ma_version_tag` are not
  sound guards; any supported watcher or explicit dependency check must be
  measured for hot-path cost.
- Preserve `StopIteration` subclasses and nonmatching exceptions via
  `PyErr_GivenExceptionMatches`, owned bool refcounts, exception propagation,
  handler evaluation order, original profile counters, no new global state,
  no new helper/public API, and no benchmark/source recognition.
- Focused tests must prove a real baseline semantic/optimization RED, a
  source-proven v3 direct-plan decline, expected generic/native call shape,
  correct iteration/exhaustion, user handler shadowing, pre/post helper
  monkeypatching, and post-cache `__code__` mutation. The baseline is already
  genuine **3 failed / 1 passed**: the generated nested iteration body
  incorrectly has one direct edge, while pre-first-call monkeypatch and
  post-cache in-place code mutation both incorrectly produce `observed=[]`;
  the post-warm full-function replacement correctly retains fallback
  behavior. The structured optimizer RED independently proves that both
  runtime-name and constant-pool alias matches were incorrectly direct while
  three non-runtime/shadowable cases must remain direct; the corrected
  structured case temporarily passed (**1 passed / 202 filtered**).
  End-to-end semantic candidate remained **3 failed / 3 passed**; both
  attempted production changes and the temporary focused tests were reverted.

## Benchmark protocol and coverage

- Fixed now-working exploratory selection:
  `chaos,richards,deltablue,comprehensions`. Full pyperformance acceptance
  remains the complete suite at **1.10x** stock CPython, not this subset.
- Previous four-workload normal-sampling baseline:
  `work/pyperformance/comparison-20260818-160837-V7kb6V/summary.json`; log
  `work/logs/profile-shutdown-flush-expanded.log`.
- Candidate completion-only smoke, not run because semantics failed:
  `just pyperformance-compare chaos,richards,deltablue,comprehensions 1 '' --debug-single-value`.
  Use it for completion, code shape, runtime frame presence, and coverage;
  single-value cold timings are not headline throughput.
- Candidate normal-sampling comparison, not run because semantics failed:
  `just pyperformance-compare chaos,richards,deltablue,comprehensions 1 work/pyperformance/comparison-20260818-160837-V7kb6V`.
  Report paired stock, direct previous SOAC, robust medians, pyperf
  significance, and unchanged benchmark selection.
- Existing coverage is `__main__` plus `soac.runtime`, with no transformed
  standard-library modules; the four benchmark compiled-function counts are
  **35 `chaos`**, **21 `comprehensions`**, **79 `deltablue`**, and **53
  `richards`**. Confirm actual warm-loop matching paths, not only benchmark
  completion or compilation of the runtime helper.
- The existing `deltablue` mean is contaminated by **24.93 ms** and
  **8.12 ms** outliers. Compare robust medians and significance before
  attributing any cross-revision gain or regression.

## Measurements

| Benchmark | Baseline paired stock mean | Baseline SOAC mean | Baseline SOAC median | Candidate SOAC | Previous / candidate |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 30.0223895 ms | 80.2183813 ms | 79.6964280 ms | not run | not applicable |
| `comprehensions` | 7.9201417 microseconds | 89.4829336 microseconds | 89.3961992 microseconds | not run | not applicable |
| `deltablue` | 1.4686720 ms | 5.9147526 ms | 4.7241264 ms | not run | not applicable |
| `richards` | 23.7982538 ms | 44.1826106 ms | 42.9617265 ms | not run | not applicable |

Baseline four-workload stock-relative geometric speedup is
**0.2579951305x**, far below the full-suite **1.10x** target.

| Native/codegen guardrail | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| `chaos` `exception_matches` inclusive CPU | 15.02% | not run | not applicable |
| `chaos` `_validate_exception_type` inclusive CPU | 10.17% | not run | not applicable |
| `comprehensions` `exception_matches` inclusive CPU | 6.83% | not run | not applicable |
| `comprehensions` `_validate_exception_type` inclusive CPU | 5.01% | not run | not applicable |
| Optimized typed-IR final basic blocks | 2,541 | not run | not applicable |
| Optimized typed-IR function instances | 193 | not run | not applicable |
| Pre-optimization serialized BlockPy bytes | 8,171,776 | not run | not applicable |
| Apply-mode native emitted bytes | 18,674,240 | not run | not applicable |
| Apply-mode native machine blocks | 1,236,140 | not run | not applicable |

Guarded generic dispatch may reduce direct edges, inlining, generated code, or
Python helper activations. Measure those outcomes independently before
claiming any hot-path or throughput improvement.

## Attempt history

### Attempt 1: Prefer the existing guarded runtime vectorcall arm

- Change: let the v3 planner retain generic vectorcall only for explicitly
  proven `exception_matches(exc, StopIteration)` runtime-origin calls, while
  hardening the existing native hook's helper provenance/code-mutation guard.
- Measurements and coverage: zero-loss native profiles show **15.02% /
  10.17%** inclusive runtime match/validation in `chaos` and **6.83% / 5.01%**
  in `comprehensions`; current four-case stock score is **0.2579951x**.
  No candidate generated-code measurements, native profile, or throughput
  run was performed because semantic regressions remained RED.
- Compatibility and tests: require a genuine baseline regression before
  production edits and preserve all handler shadowing, monkeypatch, code
  mutation, tuple/other-exception, subclass, and tracing semantics.
  Genuine untouched-production integration: **3 failed / 1 passed in 3.41
  seconds**. The generated list-comprehension body has one unwanted Apply
  direct edge; independent semantic REDs prove both prewarm helper
  monkeypatch and in-place canonical `__code__` mutation suppress user
  callbacks (`observed=[]`); post-warm helper replacement already falls back
  correctly. A structured optimizer RED independently selects direct IDs
  `[10, 11, 12, 13, 14]` rather than `[12, 13, 14]`, then turns GREEN after
  the narrow fix (**1 passed / 202 filtered**). Review then found mutable
  `_validate_exception_type`, `isinstance`, and other transitive
  runtime/builtins globals remain unguarded even when helper/code identity
  matches. Expanded validation confirms **3 failed / 3 passed in 4.17
  seconds**: helper rebinding/code mutation are fixed, but validator/global
  callbacks remain skipped and the real nested direct edge still exists.
  Source tracing then proves that both lowering paths wrongly generate
  lexically shadowable synthetic sync/async handlers; the structured lowerer
  regression fails with `synthetic StopIteration handler must use a runtime
  attribute`. The real sync handler is therefore `Global`, not the optimizer
  unit test's explicit `RuntimeName`.
  Existing runtime indexed stores explicitly bypass dictionary watchers, so
  watcher invalidation cannot repair these failures. The separate synthetic
  handler-shadow correctness fix is tracked independently.
- Result: **REJECTED**. Planner/helper candidate and temporary tests reverted;
  no candidate benchmark or claimed speedup.

## Verdict and next action

- Verdict: **REJECTED; candidate implementation reverted.** Canonical
  callable/code guards are insufficient, runtime helper and validator globals
  remain mutable, and SOAC indexed stores skip dictionary watchers. Expanded
  validation remained **3 failed / 3 passed**. There was no safe candidate
  benchmark, throughput claim, or retained optimizer/JIT change.
- Transferable lesson: a profiled direct Python call can be slower than an
  already implemented guarded generic C hook; dispatch policy and mutable
  callable provenance both matter.
- Next action: retain only the independent synthetic-handler correctness
  repair under
  `2026-08-18-bind-synthetic-iteration-exceptions-to-runtime.md`. Revisit
  helper bypass only if every dynamic dependency can be guarded across all
  mutation paths without suppressing user-visible callbacks.

## Attempt 2: Guard canonical StopIteration matching with live dependency slots

- Status: **LANDED / RETAIN; GENUINE PLANNER / INTEGRATION RED-TO-GREEN,
  FIXED-EIGHT AND MATCHED THREE-ROUND GAINS, REDUCED NATIVE CODE,
  ZERO-LOSS CHAOS PROFILE, AND FULL CORRECTNESS GATE VERIFIED**.
- Pacific date: **2026-08-19 PDT**.
- Baseline revision: integrated `main` change **`wtyxsxpv`**, commit
  **`01d784ec`**.
- Candidate revision: change **`rllxmowx`**, commit **`aa0ec27c`**.
- Historical boundary: **Attempt 1 remains REJECTED exactly as recorded
  above**. Its lexical synthetic iteration-exception bug was subsequently
  fixed in the separate dedicated strategy; that correction does not prove
  mutable runtime-helper or validator dependencies safe to bypass.

### Revised hypothesis and source evidence

- Existing profiled direct calls to `soac.runtime.exception_matches` still
  bypass the already-existing generic vectorcall hook. For canonical
  compiler-owned matcher plus compiler-owned `StopIteration`, a direct
  decline could route to the guarded existing hook, but only if every
  canonical helper/validator identity, mutable transitive dependency, and
  callback/monitoring assumption is checked live at each invocation.
- Fresh current zero-loss `comprehensions` profile contains **738 samples**:
  `exception_matches` is **3.392% inclusive**, with nested validator
  **2.035% inclusive**. Current prior profiles attribute approximately
  **6.495%** matcher ancestry to `deltablue` and **2.131%** to `richards`.
  Older `chaos` attribution of **12.353%** is explicitly stale and is now
  superseded by a fresh integrated-revision zero-loss capture. These are
  overlapping ancestry shares, not additive speedups or candidate
  performance results.
- Fresh current post-interned `chaos` profile is
  `work/logs/live-guarded-stop-iteration-baseline-chaos_*`, captured with
  **70 loops / 199 Hz**: **806 recorded samples / 50.406 MB / zero losses**,
  **99,960 weighted total across 415 distinct stacks**. Runtime matcher
  ancestry is **10.42117% inclusive**, with overlapping nested validator
  **5.33413%**. Exact direct parents are `Spline.__call__` **7.56803%** and
  nested `Spline.__call__.<locals>._dp_listcomp_3` **2.85314%**; no sampled
  compilation is present. `object_isinstance` is **1.86074%** and
  `GetOptionalAttr` **1.48860%**. Overlapping eager-factory ancestry
  **9.42777%** is a separate future opportunity, not this strategy.
- Genuine unchanged-production focused integration
  `just pytest-fast tests/test_stop_iteration_runtime_fast_path.py -q`
  reports a final clean **5 FAILED / 0 passed in 3.78 seconds**, with no
  production edits.
  Four failures expose true existing CPython-user-visible bugs: replacing
  the matcher helper **before first call**, mutating cached helper
  `__code__`, mutating validator `__code__`, and replacing the live
  validator all produce **`observed=[]`** instead of invoking the expected
  user callback. These are compatibility bugs, not merely missed
  specialization counters.
- The fifth corrected real production Profile→Verify regression now trains
  **40 hot nested matcher calls and 40 unrelated calls**. User shadowing
  controls pass, and the ordinary non-exact `explicit_shadow` direct edge
  remains correctly retained. The intended nested Verify event instead
  reports actual **`clif_direct_edges=1`**,
  **`function_env_indirect_edges=1`**, and
  **`generic_fallback_edges=2`**, where canonical matcher direct edges must
  be **0**. This isolates the production planner mismatch without relying on
  an inlined unrelated-target assumption.
- The corrected fixture additionally contains balanced raw indexed-global
  mutation/restoration for both an existing runtime `isinstance` slot and a
  previously absent `issubclass` insertion, each bypassing watchers and
  version invalidation. No production behavior changed before this final
  clean RED. Root has authorized exactly three production files for the
  separate implementation owner after exclusive guest transfer.
- Independent genuine unchanged-production optimizer regression runs the
  real `soac_opt::pipeline_v3::direct_call_requests_from_evidence_v3`
  production path. Actual selected source IDs are **`[10, 11, 12, 13,
  14]`**, but only **`[12, 13, 14]`** are valid: source **#10** is the
  explicit canonical runtime matcher and **#11** its constant-pool runtime
  alias, both wrongly admitted as direct calls. `ValueError`, the local
  handler, and the local callback must remain valid direct targets. The
  focused optimizer test reports **1 FAILED / 211 filtered**; initial
  compilation takes **14.42 seconds** as workflow context only.
- The exact same genuine production-path structured optimizer regression
  now turns **RED-to-GREEN, 1 PASSED / 211 filtered**. Explicit canonical
  runtime matcher **#10** and constant-pool alias **#11** are declined,
  while `ValueError`, the local handler, and local callback retain valid
  direct edges **#12 / #13 / #14**. This verifies planner selection only,
  not transformed runtime correctness.
- The first implementation is saved in exactly the three authorized
  production files. It removes the old unsound process-global
  `AtomicUsize` matcher cache and reuses the existing helper-template
  `OnceLock` to store only pointer/index/session metadata. The guard reads
  **seven live custom-indexed runtime slots plus four live combined-builtin
  slots**, validates exact original registered helper and validator code,
  compile session and globals pointers, admits only exact `StopIteration`,
  and checks both code objects for tracing and code-local/global observation.
  It adds no process-global cache, Python key, public API, or runtime
  helper.
- First transformed integration rerun now passes **4 / 5 previously failing
  scenarios**. All four genuine CPython-user-visible bugs are fixed:
  pre-first matcher replacement, helper `__code__` mutation, validator
  `__code__` mutation, and live validator replacement now invoke their
  expected callbacks. Raw present/absent indexed globals, monitoring, and
  mandatory all-subclass fallback controls also pass.
- The remaining fifth failure is a fixture-oracle mismatch. The optimizer
  correctly removes every canonical matcher direct edge, but existing
  `crates/soac_jit/src/jit/direct_function.rs:194` returns without emitting
  `soac_jit_direct_edges` when the total is zero. The fixture incorrectly
  expected a zero-valued event. Its separate owner is correcting only the
  test oracle to prove the nested function compiled from existing JIT code
  summary, assert **absence** of the nested direct-edge event, and preserve
  the unrelated direct edge. No fourth production file will change, and
  **5 / 5 GREEN is not claimed until the corrected rerun below**.
- The corrected genuine end-to-end regression now turns from unchanged
  production **5 FAILED / 3.78 seconds** to candidate
  **5 PASSED / 4.87 seconds**. All four actual preexisting user-visible
  callback bugs are fixed. Runtime and builtin replacement controls include
  watcher/version-free writes to an existing `isinstance` slot and a
  previously absent `issubclass` slot. Exact `StopIteration` uses the
  guarded path; every subtype, custom/raising `__class__`, class object,
  and nonmatching exception retains original fallback. Helper and
  validator code-local/global monitoring, tracing, and profiling remain
  observable.
- The fifth real Profile→Verify→Apply regression trains both nested matcher
  and ordinary direct targets. In **both Verify and Apply**, newly appended
  `jit-code-summary` direct-body records prove the nested function was
  compiled; its direct-edge event is correctly **absent** because existing
  tracing omits events for total zero, while the ordinary `explicit_shadow`
  direct-edge event remains present. The independent structured optimizer
  regression remains GREEN.
- A dedicated actual private CPython FFI structured production-guard
  regression now passes **1 / 565 filtered**; the JIT test inventory grows
  to **566**. It builds a real custom indexed runtime dictionary and proves
  both **present-slot replacement** and **previously absent-slot insertion**
  leave `dk_version` and `ma_used` unchanged while the live guard correctly
  invalidates and then restores. A real combined-Unicode builtins dictionary
  value changes in place with identical `ma_keys`, and the guard still
  invalidates. All replacement/restoration paths preserve balanced
  `INCREF` / `DECREF` ownership. This establishes actual live-slot safety,
  not a watcher/version approximation.
- The optimizer structured regression and full five-case transformed
  integration remain GREEN. The complete optimizer library also passes
  **212 / 212**. Complete JIT/all-target suites, broad transformed
  guardrails, scoped formatting/checks, candidate performance, and full
  correctness gate all subsequently pass.
- Workflow-only first complete JIT-suite failure: the new FFI regression
  incorrectly assumes `PyLong_FromLong(1000)` /
  `PyLong_FromLong(2000)` return mortal objects, but pinned CPython returns
  an immortal raw reference count **`0x50000c0000000`**. Its exact
  `refcount == 2` assertion panics while holding a shared test mutex,
  cascading process-local `PoisonError` into unrelated tests. This is a
  test-fixture assumption, not changed production matcher semantics. The
  owner then replaces those values with definitely mortal freshly allocated
  `PyList` objects and verifies exact baseline / `+1` / restored reference
  counts. A fresh complete all-target
  `cargo test -p soac_jit --tests --quiet` run subsequently passes
  **566 / 566**, and full `soac_opt --lib` passes **212 / 212**. The failed
  process remains documented as a pinned-CPython fixture / mutex-poisoning
  issue, not a production correctness regression.
- First broad transformed runtime matrix reports **36 passing cases and 1
  failure**, isolated to existing
  `tests/test_regression_direct_exception_cleanup.py::test_apply_mode_runtime_unpack_path_preserves_exception_cleanup`.
  Actual user-visible result remains correct, **`[1, 2]`**. Its stale
  assertion demanded **any** emitted direct-edge event, but the only former
  edge was precisely the canonical matcher intentionally removed by this
  optimization; existing zero-edge tracing suppresses the event.
- Root authorizes the separate reviewer to migrate **only that existing
  test oracle**: preserve the exact `[1, 2]` result and assert the real
  emitted native `direct_function_body` for `run` through existing JIT code
  summary instead. All genuine direct-call and constructor regressions remain
  untouched. Production scope stays exactly three files; validation test
  inventory is the new focused StopIteration integration plus this one
  existing fixture migration. Do not distort production or invent synthetic
  direct edges merely to satisfy an obsolete observability assumption.
  The reviewer completes that test-only migration, preserving all genuine
  direct-call and constructor assertions unchanged.
- The broad transformed runtime rerun now passes **37 / 37 tests across 16
  files in 26.92 seconds**. It includes all five new StopIteration semantic /
  Profile→Verify→Apply cases, synthetic exception-shadow handling,
  iterator/hash/collision behavior, all **five** direct exception-cleanup
  cases, generator/source/synthetic watcher and factory mutation paths,
  inherited/non-self/scalar owner fields, default/code mutation,
  constructors, and fused-float behavior. The migrated existing cleanup
  test verifies both real native `run` direct-function-body evidence and
  exact result **`[1, 2]`**. Full optimizer **212 / 212** and JIT
  **566 / 566** remain green. Package-scoped
  `just fmt-rust-check soac_opt soac_jit` and combined
  `cargo check --all-targets -p soac_opt -p soac_jit` also pass. Production
  is frozen to exactly the three authorized files; the validation inventory
  contains one new focused StopIteration file plus one migrated existing
  cleanup oracle. Candidate benchmarks and the full correctness gate also
  subsequently pass.
- Release fixed-eight debug-single smoke
  `work/pyperformance/comparison-20260819-075039-BbR6Za` completes
  **8 / 8 workloads with zero errors**, compared with mode-matched interned
  baseline smoke **065734**. Aggregate generated native code decreases
  **2,426,104 → 2,253,100 bytes (-7.13094%)**; optimized typed coverage
  changes **3,069 → 2,866 blocks** and **218 → 204 functions**.
- This typed-function reduction is **intentional**, not lost benchmark
  coverage: each of seven affected workloads drops only the compiler-owned
  `exception_matches` / `_validate_exception_type` direct helper bodies and
  associated adapters. **No user benchmark function is lost**; `fannkuch`
  is invariant. PID-matched workload totals are `chaos`
  **712,432 → 681,500 bytes (-4.34%)**, `comprehensions`
  **302,076 → 274,348 bytes (-9.18%)**, `deltablue`
  **481,284 → 454,868 bytes (-5.49%)**, and `richards`
  **367,664 → 348,348 bytes (-5.25%)**; `float` shrinks **25.89%**,
  `nbody` **9.08%**, and `spectral_norm` **11.88%**.
- Actual retained-source direct-edge evidence matches the plan:
  `Spline.__call__` changes **3 → 1** edges, its nested list comprehension
  changes **1 → no zero-edge event**, a `Planner` source changes
  **1 → none**, and all `Widget` list/dict comprehension matcher edges
  change **1 → none**. Unrelated direct edges remain intact. Cold one-loop
  smoke timings and the reported **4.259x** previous score are invalid as
  throughput evidence. Normally sampled fixed-eight comparison subsequently
  completes; later repeated validation and full correctness gate establish
  retention.
- Normally sampled fixed-eight candidate
  `work/pyperformance/comparison-20260819-075301-LBFDfW` completes
  **8 / 8** against prior interned baseline **065917**. Official stock
  geometric score improves **0.5558386711560767x → 0.5782047994439117x**;
  official arithmetic previous-SOAC improvement is
  **1.0350348551699229x**. Robust previous-SOAC geometric improvement is
  **1.02855x**, or **1.03376x** stock-adjusted.
- Robust `chaos` median ratio is **1.15902x**, worker-clustered interval
  **1.1278–1.2281x**, or **1.14381x** stock-adjusted. `deltablue` is
  **1.06525x**, interval **1.0243–1.0999x**, or **1.05494x** adjusted,
  but its adjusted confidence interval includes neutral. `nbody` is
  **1.07298x**, interval **1.0497–1.1078x**, or **1.09780x** adjusted;
  attribution to this strategy is not established. `comprehensions` is
  **1.03599x**, interval **0.9862–1.0737x**, adjusted **1.00219x**,
  inconclusive.
- Robust control ratios are `richards` **0.97277x / 0.99050x adjusted**,
  `float` **0.96860x / 0.99152x adjusted**, `fannkuch`
  **0.98251x / 1.00258x adjusted**, and `spectral_norm`
  **0.98589x / 0.99777x adjusted**. No control movement is assigned causal
  meaning without repeated paired evidence.
- Normally sampled generated native code decreases
  **25,033,800 → 23,293,040 bytes (-6.9536%)**, machine blocks
  **1,652,600 → 1,533,550**, and optimized typed coverage
  **3,069 → 2,866 blocks / 218 → 204 functions**. This again removes only
  compiler-owned matcher/validator helper bodies; **zero user benchmark
  callable functions are lost**. Targeted three-round comparison and full
  correctness gate subsequently pass.
- Targeted same-selector three-round candidate
  `work/pyperformance/comparison-20260819-075611-aGuv3b` completes
  **60-versus-60 samples** against prior interned comparison **070242**.
  Robust `chaos` ratio is **1.120774x**, worker-bootstrap **95% interval
  1.07971–1.14830x**; paired-stock adjusted **1.121950x**, interval
  **1.07692–1.16931x**. Individual raw rounds are
  **1.1445x / 1.1646x / 1.0573x**.
- `deltablue` improves **1.038626x**, interval **1.01790–1.06682x**, or
  **1.047388x** stock-adjusted, interval **1.01652–1.08442x**; all rounds
  improve. `comprehensions` raw **1.021244x**, interval
  **0.99938–1.04532x**, crosses neutral; stock-adjusted **1.051409x**,
  interval **1.00728–1.08261x**, depends on stock factor **0.97131x**.
  `richards` raw **1.006805x**, interval **0.98074–1.03963x**, also
  crosses neutral; adjusted **1.034581x**, interval
  **1.00195–1.07905x**, likewise requires stock-drift caution.
- Robust targeted-subset geometric improvement is **1.045955x**, or
  **1.063296x** stock-adjusted; official arithmetic previous-SOAC ratio is
  **1.0568639734384457x**, and paired-stock subset score
  **0.4154093171730844x**. Targeted native bytes decrease
  **19,407,320 → 18,352,680 (-5.434%)**, removing only the two
  compiler-owned matcher/validator helper bodies per affected worker; user
  callable coverage remains unchanged. This supports **RETAIN CANDIDATE**.
  Matched zero-loss chaos profiling subsequently confirms matcher and
  validator elimination; authoritative full correctness gate also passes,
  while subset results do not satisfy the full-suite stock goal.
- Matched current-revision zero-loss chaos candidate profiles
  `work/logs/live-guarded-stop-iteration-candidate-chaos_*` use the same
  **70 loops / 199 Hz** as baseline: raw recorded samples fall
  **806 → 639**, and distinct aggregated Speedscope stacks **415 → 349**.
  Runtime matcher ancestry falls **10.421% → 0%**, nested validator
  **5.334% → 0%**, `builtin_isinstance` **2.357% → 0%**,
  `object_isinstance` **1.861% → 0%**, and runtime-global slow lookup
  **3.225% → 0%**. These ancestor/descendant stack shares overlap and must
  never be added.
- The replacement live-dependency guard occupies **2.035% inclusive**,
  including overlapping slot validation **0.939%** and monitoring checks
  **0.312%**. `py_vectorcall` self changes **3.350% → 3.603%**, TLS
  **4.962% → 4.223%**, and GC **0.248% → 0.469%**; no compiler frames are
  sampled. Normalized eager-generator-factory ancestry remains
  **9.428% → 10.010%** and is a separate future opportunity. Attached
  replay **57.6803 → 45.7623 ms (1.26043x)** is **diagnostic only**; the
  repeated chaos median **1.120774x**, interval **1.07971–1.14830x**, is
  the performance headline.
- Pinned SOAC runtime global dictionaries use a custom indexed
  `dk_kind = 3` representation. Existing raw indexed stores skip normal
  CPython dictionary watchers and version invalidation. Therefore a
  watcher/version-only cache, immutable function pointer, or code-identity
  guard repeats the rejected Attempt 1 bug. Current values must be read
  through live dependency keys and validated slots on **every use**.
- Integrated fixed-eight stock score is **0.5558386711560767x**, and the
  corresponding fixed-four stock score is **0.3874458135x**. Current
  generated Apply code is **25,033,800 bytes / 1,652,600 machine blocks**,
  optimized typed coverage **3,069 blocks / 218 functions**, and
  pre-optimization BlockPy **14,398,752 bytes**. The authoritative
  full-suite **1.10x stock** goal remains unmet; no new previous-SOAC or
  candidate speedup exists.

### Proposed implementation and compatibility

- Root-authorized bounded production scope: exactly
  `crates/soac_opt/src/pipeline_v3.rs`,
  `crates/soac_jit/src/lib.rs`, and
  `crates/soac_jit/src/jit/specialized_helpers.rs`, plus one new focused
  transformed integration. No production implementation, public API,
  process-global cache, runtime helper, or runtime Python helper change is
  currently established.
- Cache **zero newly created Python keys or strong runtime-module
  references**. Existing lowered runtime `global_names` already provides a
  preallocated raw indexed slot per dependency; verify its exact Unicode
  name and cache only current globals-dictionary / `ma_keys` / `ma_values`
  pointers, existing index, and expected current value pointers. Re-read
  the live slot every call, including null/tombstone absence. For builtins,
  scan existing exact combined Unicode entries once, cache only original
  entry/key pointers, and re-check `ma_keys`, key kind, original key
  identity, and live value every use. Canonical helper/validator provenance
  and current code identity remain guarded; no lock, allocation, callback,
  public API, extra module reference, or new process-global state is added.
- Validate the helper/validator's currently visible globals
  `isinstance`, `tuple`, `type`, and `_validate_exception_type`. For names
  absent from globals, validate their absence **and** the live builtins
  values `issubclass`, `BaseException`, and `RecursionError`. Any helper or
  validator rebinding, `__code__` mutation, globals insertion/replacement,
  builtins mutation, custom indexed raw store, mapping/slot mismatch,
  recursion mismatch, or unavailable canonical provenance must retain the
  full original Python execution path.
- Inspect both helper and validator for active tracing, profiling, and
  code-local/global `sys.monitoring`; any observer must retain full
  execution and callbacks. Preserve user-visible runtime wrappers, descriptor
  effects, exception state, recursion behavior, and original fallback.
- The optimized arm may match **only an actual exact instance satisfying
  `Py_TYPE(exc) == PyExc_StopIteration`**. **Every `StopIteration`
  subclass must fall back**, even a real exception subtype, because the
  original helper first evaluates `isinstance(exc, RecursionError)` and a
  subclass may override `__class__` with observable or raising behavior.
  Preserve ordinary subclass results through the unchanged original helper.
  Nonmatching exceptions, exception class objects, spoofed or raising
  `__class__`, custom metaclasses, and ambiguous receiver shapes likewise
  require full fallback; no subclass-relaxed fast arm is sound.
- Planner admission must be statically proven as the canonical
  compiler-owned runtime matcher and compiler-owned runtime `StopIteration`.
  Only that exact direct edge should decline into the existing hook;
  unrelated runtime direct calls and existing optimization plans remain
  unchanged. No string-name heuristic, broad generator exclusion, watcher
  shortcut, new public API, or additional global state is permitted.
- New focused integration and structured source/provenance tests must first
  fail against unchanged production, then cover all runtime-global raw
  stores, globals insertion/deletion, live builtin changes, helper and
  validator code swaps, trace/profile/local/global monitoring,
  forced fallback for every `StopIteration` subtype, including subclasses
  overriding/raising `__class__`, class objects, spoofed receivers,
  unrelated direct-call preservation, and synthetic lexical shadowing.
  Genuine planner and transformed integration regressions both turn
  **RED-to-GREEN**, and the real pinned-CPython FFI guard regression passes;
  candidate performance and the full correctness gate also pass.

### Attempt 2 benchmark protocol and measurements

- Repeat the fixed eight `chaos`, `comprehensions`, `deltablue`, `fannkuch`,
  `float`, `nbody`, `richards`, and `spectral_norm` against the same stock
  CPython and immediately prior integrated interned-runtime SOAC revision.
  Independently profile each revision; separate transformed runtime-helper
  execution and typed/native coverage from benchmark completion.
- The previous integrated fixed-eight score is **0.5558386711560767x** and
  prior fixed-four score **0.3874458135x**. Candidate fixed-eight and
  previous-SOAC comparisons, robust medians/intervals, repeated rounds,
  guarded direct-edge counts, zero-loss native attribution, generated-code
  effects, and full correctness gate are recorded below.

| Attempt 2 metric | Integrated interned-runtime baseline | Candidate | Change |
| --- | --- | --- | --- |
| Fixed-eight paired stock / SOAC geometric score | 0.5558386711560767x | 0.5782047994439117x | improves; full-suite stock 1.10x goal unmet |
| Fixed-eight previous-SOAC arithmetic / robust / stock-adjusted geometry | integrated interned baseline | 1.0350348551699229x / 1.02855x / 1.03376x | targeted repeat also completed |
| Fixed-eight robust chaos / delta medians | integrated interned baseline | chaos 1.15902x; delta 1.06525x | chaos 95% 1.1278–1.2281x; delta adjusted interval includes neutral |
| Targeted three-round robust / stock-adjusted subset geometry | prior interned repeated comparison 070242 | 1.045955x / 1.063296x | arithmetic 1.0568639734384457x; 60 versus 60 samples |
| Targeted three-round chaos / delta medians | prior interned repeated baseline | chaos 1.120774x; delta 1.038626x | 95% chaos 1.07971–1.14830x; delta 1.01790–1.06682x |
| Targeted three-round comprehensions / richards medians | prior interned repeated baseline | 1.021244x / 1.006805x raw | both raw confidence intervals include neutral; adjusted gains stock-sensitive |
| Targeted measured native bytes | 19,407,320 | 18,352,680 | -5.434%; only compiler-owned helper bodies removed |
| Fixed-four paired stock / SOAC geometric score | 0.3874458135x | pending | subset only; no full-suite claim |
| Optimized typed-IR blocks / functions, debug-single smoke | 3,069 / 218 | 2,866 / 204 | intended compiler-owned matcher/validator helper removal; no user function lost |
| Serialized pre-optimization BlockPy bytes | 14,398,752 | pending | pending |
| Normal Apply-mode native code bytes / machine blocks | 25,033,800 / 1,652,600 | 23,293,040 / 1,533,550 | native -6.9536%; compiler-owned helper removal only |
| Mode-matched debug-single generated native bytes | 2,426,104 | 2,253,100 | -7.13094%; smoke timing invalid |
| Current comprehensions zero-loss samples / matcher / validator | 738 / 3.392% / 2.035% | pending | overlapping inclusive ancestry |
| Current delta / richards matcher ancestry | 6.495% / 2.131% | pending | overlapping inclusive ancestry |
| Matched chaos zero-loss samples / distinct stacks | 806 samples / 415 stacks | 639 samples / 349 stacks | same 70 loops / 199 Hz; zero losses |
| Matched chaos matcher / validator ancestry | 10.421% / 5.334% | 0% / 0% | overlapping; no additive speed claim |
| Matched chaos replacement live guard / slot / monitoring | no replacement guard | 2.035% inclusive / 0.939% slot / 0.312% monitoring | overlapping nested shares; no sampled compiler |
| Current chaos direct matcher parents | Spline.__call__ 7.56803%; nested _dp_listcomp_3 2.85314% | pending | no sampled compilation |
| Older chaos matcher ancestry | 12.353% | not applicable | stale earlier revision; superseded by fresh 806-sample capture |
| Genuine new transformed integration regression | 5 failed / 0 passed / 3.78 s; four observed=[] callback bugs; nested direct edge 1 | 5 passed / 4.87 s; no nested edge, ordinary direct retained | genuine RED-to-GREEN; live raw globals/builtins/monitoring/subclass controls |
| Genuine new structured canonical planner regression | 1 failed / 211 filtered; actual source IDs [10, 11, 12, 13, 14] versus expected [12, 13, 14] | 1 passed / 211 filtered; only [12, 13, 14] retained | genuine RED-to-GREEN; canonical matcher + alias declined |
| Actual CPython FFI live-slot guard regression | indexed stores leave dk_version / ma_used unchanged; builtin in-place change preserves ma_keys | 1 passed / 565 filtered; all mutations invalidate and restore | GREEN; balanced INCREF / DECREF; JIT inventory 566 |
| Complete optimizer Rust library | integrated prior optimizer baseline | 212 / 212 passed | GREEN |
| First full JIT-suite attempt | existing JIT baseline | failed: fixture assumed pinned PyLong values mortal; shared mutex poisoned | workflow-only; fixed using fresh mortal lists |
| Fresh complete all-target JIT Rust suite | JIT inventory 566 | 566 / 566 passed | GREEN; exact baseline/+1/restored mortal-list reference counts |
| First broad transformed-runtime matrix | retained prior regression inventory | 36 passed; 1 stale zero-edge observability assertion failed | actual `[1, 2]` correct; obsolete event oracle migrated |
| Corrected broad transformed-runtime guardrails | first run 36 passed / 1 stale oracle failed | 37 / 37 passed across 16 files in 26.92 s | GREEN; genuine direct-call/constructor assertions preserved |
| Scoped optimizer/JIT formatting and combined all-target check | integrated baseline | `fmt-rust-check soac_opt soac_jit` and combined all-target check pass | GREEN |
| Full `just test-all` correctness gate | integrated baseline previously passed | 1,227 nodeids; 90 / 90 isolated file batches; 8 workers | GREEN; zero failed |

The authoritative full-gate log is
`work/logs/live-guarded-stop-iteration-test-all.log`. `just test-all`
passes **1,227 Python nodeids across 90 / 90 isolated file batches and
eight workers**, with **zero failed batches**. Workspace Rust suites pass
JIT **566**, optimizer **212**, typed IR **54**, lowering **371**, and PyO3
**8**. Cargo tests take **70.160 seconds**, inner / outer pytest
**74.533 / 74.546 seconds**, and the complete test phase
**144.721 seconds**; the known counter-dump batch takes **74.13 seconds**.

### Attempt 2 verdict and next action

- Verdict: **LANDED / RETAIN; Attempt 1 remains REJECTED exactly as
  recorded above**. Attempt 2 genuine
  unchanged-production focused integration cleanly fails
  **5 / 3.78 seconds**, including **four true CPython-user-visible
  helper/validator mutation bugs** and one exact nested matcher Verify
  direct-edge count **1 instead of 0**. The unrelated non-exact direct edge,
  user shadowing, and balanced raw existing/absent runtime-global mutation
  controls are valid. Independent genuine production-path optimizer RED
  initially fails **1 / 211 filtered**, selecting source IDs
  **[10, 11, 12, 13, 14]** rather than expected **[12, 13, 14]**, then
  genuinely turns **GREEN 1 / 211 filtered**: explicit matcher and alias are
  declined while ValueError/local direct calls remain. The initial exactly
  three-file implementation removes the old global atomic cache, stores
  only pointer/index/session metadata in the existing template `OnceLock`,
  and validates seven live runtime plus four builtin slots, exact canonical
  code/session/globals, and observation guards. First transformed rerun
  initially passes **4 / 5**, fixing all genuine callback bugs; the fifth
  test's zero-event oracle is then corrected without production expansion.
  Full transformed integration turns **5 FAILED / 3.78 seconds → 5 PASSED /
  4.87 seconds**, with Profile→Verify→Apply nested compiled-body proof,
  absent canonical matcher edge, retained unrelated direct edge, exhaustive
  live raw globals/builtins mutation, exact-only matching, and helper /
  validator observation guards. The dedicated actual private CPython FFI
  live-slot regression passes **1 / 565 filtered**, proving unchanged
  indexed `dk_version` / `ma_used` and unchanged combined builtin `ma_keys`
  do not bypass invalidation, with balanced references. The full optimizer
  library passes **212 / 212**. First full JIT run exposes a fixture-only
  immortal-`PyLong` refcount assumption and poisons its shared test mutex,
  causing cascading unrelated `PoisonError`s. Production behavior is
  unaffected; the corrected fixture now uses definitely mortal fresh lists
  and exact baseline / `+1` / restored reference counts. A fresh complete
  all-target JIT process passes **566 / 566**, alongside optimizer
  **212 / 212**. First broad transformed suite passes **36** and exposes
  **1 stale existing exception-cleanup test oracle**: correct `[1, 2]`
  execution no longer emits a matcher direct-edge event after intentional
  removal. Root authorizes only that test to assert existing emitted native
  `run` body evidence instead; all genuine direct/constructor regressions
  and exactly three production files remain unchanged. Corrected broad
  transformed rerun now passes **37 / 37 across 16 files in 26.92 seconds**,
  including all five direct exception-cleanup cases and all five new
  StopIteration scenarios. Scoped optimizer/JIT format checking and
  combined all-target Cargo checking also pass. Production is frozen to
  exactly three files, with one new focused test plus one migrated existing
  cleanup oracle. Release fixed-eight debug-single smoke **075039** passes
  **8 / 8**, shrinking mode-matched native bytes **7.13094%** and typed
  functions **218 → 204** solely by removing compiler-owned matcher /
  validator helper bodies; no user benchmark functions are lost and
  unrelated direct edges remain. Cold smoke timing and **4.259x** summary
  are invalid throughput evidence. Normally sampled fixed-eight **075301**
  subsequently completes **8 / 8** with stock score
  **0.5782047994439117x**, arithmetic prior improvement
  **1.0350348551699229x**, robust **1.02855x / 1.03376x stock-adjusted**,
  chaos **1.15902x**, and **6.9536%** less normal native code. Only
  compiler-owned matcher/validator bodies disappear; no user callable is
  lost. Delta/comprehensions adjusted intervals and unrelated controls
  require repeated validation. Targeted three-round comparison **075611**
  subsequently confirms robust chaos **1.120774x**, delta **1.038626x**,
  and subset **1.045955x / 1.063296x stock-adjusted**; raw
  comprehensions/richards confidence intervals cross neutral and adjusted
  gains are stock-sensitive. Targeted native code shrinks **5.434%**, with
  no user callable loss. Matched zero-loss chaos profiles reduce raw samples
  **806 → 639**, eliminate matcher **10.421% → 0%** and validator
  **5.334% → 0%**, and replace them with an overlapping **2.035%** live
  guard; replay is diagnostic only. The authoritative full correctness gate
  passes all **1,227 Python nodeids / 90 isolated batches** plus workspace
  Rust suites, validating the retained change.
- Transferable lesson: bypassing a Python matcher is sound only when every
  helper/validator dependency remains observable and live even under SOAC's
  watcher-bypassing custom indexed stores. Exception type checks must not
  erase `isinstance`-visible `__class__` callbacks.
- Next action: integrate the fully validated retained Attempt 2 while
  preserving Attempt 1's rejected history. Keep repeated benchmark medians,
  not profiler-attached replay or overlapping stack shares, as performance
  headlines; the full-suite stock **1.10x** target remains unmet.
