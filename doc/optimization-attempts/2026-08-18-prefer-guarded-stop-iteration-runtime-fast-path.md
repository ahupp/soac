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
