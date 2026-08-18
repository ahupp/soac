---
title: "Bind synthetic iteration exceptions to the compiler runtime"
---

# Bind synthetic iteration exceptions to the compiler runtime

- Status: LANDED / RETAIN; general CPython-correctness fix, not a throughput
  optimization.
- Pacific date: 2026-08-18 PDT.
- Change: `ourqpywu`, based on retained all-module shutdown flushing.
- Outcome: an independently genuine lowerer regression proves that SOAC
  incorrectly synthesizes user-shadowable synchronous and asynchronous
  iteration-exhaustion handlers. All four compiler-generated handler sites
  should use trusted runtime exception bindings while user-authored handlers
  retain ordinary lexical shadowing. The full lowerer suite (**368 passed**),
  dedicated Profile→Apply regression, and combined iterator/original-code/
  asyncgen/shutdown checks (**21 passed in 13.01 seconds**) are GREEN.
  Four-workload normal sampling retains identical native code, typed IR, and
  coverage; all four previous-SOAC comparisons are statistically
  nonsignificant. This is a correctness repair, not a throughput win. The
  The complete project gate passes: **1,212 Python nodeids / 76 batches**,
  alongside all Rust crate suites.

## Hypothesis and evidence

CPython iteration exhausts with its actual builtin `StopIteration` or
`StopAsyncIteration`, regardless of whether the surrounding user module binds
those names to another object. SOAC's compiler rewrites currently synthesize
lexical `except StopIteration:` / `except StopAsyncIteration:` instead. A
module binding `StopIteration = ValueError` can therefore change the behavior
of an ordinary `for` loop, list comprehension, or generator expression; the
same flaw exists for synthetic async iteration. That is a CPython-visible
correctness bug, not a benchmark-specific optimization opportunity.

The affected production sites are:

- `crates/soac_lowering/src/passes/ast_to_ast/rewrite_expr/mod.rs`:
  compiler-generated synchronous and asynchronous comprehension/genexpr
  handlers.
- `crates/soac_lowering/src/passes/ruff_to_blockpy/stmt_sequences.rs`:
  compiler-generated ordinary `for` and `async for` handlers.

The structured regression
`synthetic_iteration_handlers_use_compiler_owned_runtime_exceptions`
genuinely fails on unchanged production with
`synthetic StopIteration handler must use a runtime attribute`. The existing
handler is an AST bare `Name`, not an attribute on the compiler-owned
runtime object. After replacing all four synthetic handlers, the same
structured Rust regression passes (**1 passed**). The dedicated
`tests/test_synthetic_iteration_exception_shadowing.py` regression also
passes (**1 passed in 4.84 seconds**) across both Profile and Apply. It binds
`StopIteration = ValueError` and `StopAsyncIteration = RuntimeError` from
module import and verifies list comprehensions, ordinary `for`, generator
expressions, `async for`, and async comprehensions still terminate correctly,
while explicit user sync/async handlers retain their lexical shadowing.
The complete lowerer crate passes **368 tests**; a combined focused set
covering the new integration, iterator/original-code behavior, async
generators, and shutdown flush passes **21 tests in 13.01 seconds**.
`cargo check -p soac_lowering -p soac_jit --tests` is warning-free, and
package-scoped lowerer formatting/checks pass.

The final `just test-all` gate passes; complete output is in
`work/logs/synthetic-iteration-exception-shadowing-test-all.log`. It
validates **1,212 Python nodeids across 76 passing batches**, with
**545 `soac_jit`**, **368 `soac_lowering`**, **202 `soac_opt`**, and
**8 PyO3** Rust tests. Cargo takes **87.752 seconds**, pytest
**104.218 seconds**, the combined test phase **191.987 seconds**, and total
wall time **221.87 seconds**; the slow counter-dump batch takes
**102.15 seconds**.

This bug was discovered during the separate rejected fast-path investigation
recorded in
`2026-08-18-prefer-guarded-stop-iteration-runtime-fast-path.md`. That strategy
was rejected because runtime helper dependencies are mutable and SOAC's raw
indexed global stores bypass dictionary watchers. This correctness fix does
not retain its planner policy, guarded helper expansion, mutation bypass, or
any unsupported throughput claim.

## Implementation and compatibility

- Replace only compiler-synthesized synchronous handlers with
  `except __soac__.StopIteration:`.
- Replace only compiler-synthesized asynchronous handlers with
  `except __soac__.StopAsyncIteration:`.
- Cover both lowering families: comprehension/genexpr rewrites and ordinary
  `for` / `async for` statement lowering, for four total synthetic sites.
- Preserve user-authored lexical `except StopIteration:` and
  `except StopAsyncIteration:` handlers, module-global mutation, evaluation
  order, exception propagation, iteration result values, and generator/async
  semantics.
- Preserve existing `profile`, `verify`, and `apply` behavior, direct-call
  planning, runtime helper dispatch, and mutable-runtime-global semantics.
- Do not introduce source fingerprints, benchmark-specific recognition, new
  environment knobs, public APIs, runtime helpers, or dictionary watchers.
- Keep structured lowerer tests plus a small Profile→Apply integration
  covering ordinary loops, comprehensions/genexprs, async variants, and
  explicit user-handler shadowing.

## Benchmark protocol and coverage

- This is a general semantic correctness repair, not a claimed optimization.
  Use fixed `chaos,richards,deltablue,comprehensions` selection to detect
  generated-code or throughput regressions after semantic validation.
- Previous normal-sampling baseline:
  `work/pyperformance/comparison-20260818-160837-V7kb6V/summary.json`;
  `work/logs/profile-shutdown-flush-expanded.log`.
- Completion-only candidate smoke:
  `just pyperformance-compare chaos,richards,deltablue,comprehensions 1 '' --debug-single-value`.
  Single-value/cold timings are not a throughput headline.
- Actual smoke artifact:
  `work/pyperformance/comparison-20260818-165326-UDwXFX/summary.json`;
  `work/logs/synthetic-iteration-exception-shadowing-smoke.log`.
  All four workloads complete in **40.20 seconds** including a release
  rebuild. Cold single-value Apply readings are approximately 139 ms
  `chaos`, 412 microseconds `comprehensions`, 353 ms `deltablue`, and 322 ms
  `richards`; they are completion evidence only, not throughput results.
  Coverage retains 193 typed functions / 2,541 final typed blocks,
  **1,867,424 native bytes** / **123,614 machine blocks** across one worker,
  matching the previous normal baseline per-worker values exactly.
  Serialized BlockPy totals 4,085,728 bytes in this smoke artifact.
  All eight `comprehensions` nested direct-edge tuples are also unchanged,
  independently confirming that the rejected optimizer policy was reverted.
  The cold smoke score must not be compared with the previous normal-sampling
  0.257995x stock-relative baseline.
- Completed candidate normal-sampling comparison:
  `just pyperformance-compare chaos,richards,deltablue,comprehensions 1 work/pyperformance/comparison-20260818-160837-V7kb6V`.
- Candidate result:
  `work/pyperformance/comparison-20260818-165506-0dnfty/summary.json`;
  `work/logs/synthetic-iteration-exception-shadowing-normal.log`;
  **86.84 seconds** elapsed. Pyperf marks **all four previous-SOAC
  comparisons nonsignificant**.
- Existing compiled-function coverage is 35 `chaos`, 21 `comprehensions`,
  79 `deltablue`, and 53 `richards`; transformed modules are `__main__`
  plus `soac.runtime`, with no transformed standard library.
- The previous `deltablue` mean contains 24.93 ms / 8.12 ms outliers; use
  robust medians and significance when interpreting any regression.
- Acceptance remains the full pyperformance suite at **1.10x stock
  CPython**. Neither fixing this correctness bug nor completing four
  workloads satisfies that performance goal.

## Measurements

| Benchmark | Baseline stock mean | Baseline SOAC mean | Baseline SOAC median | Candidate SOAC | Previous / candidate |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 30.0223895 ms | 80.2183813 ms | 79.6964280 ms | 81.8153701 ms; median 80.2891995 ms | 0.980481x mean; 0.992617x median |
| `comprehensions` | 7.9201417 microseconds | 89.4829336 microseconds | 89.3961992 microseconds | 89.4563098 microseconds; median 87.9882798 microseconds | 1.000298x mean; 1.016001x median |
| `deltablue` | 1.4686720 ms | 5.9147526 ms | 4.7241264 ms | 4.5462772 ms; median 4.4998589 ms | 1.301010x mean; 1.049839x median |
| `richards` | 23.7982538 ms | 44.1826106 ms | 42.9617265 ms | 43.4689396 ms; median 43.0256230 ms | 1.016418x mean; 0.998515x median |

Baseline four-workload stock-relative geometric speedup is
**0.2579951305x**; candidate paired-stock score is **0.2780522558x**, still
far below the full-suite **1.10x** target. The apparent previous-SOAC mean
geometric ratio **1.0671618912x** is inflated by the baseline `deltablue`
outliers and is not a performance claim. The outlier-robust median
geometric ratio is **1.0140007635x**, and pyperf finds every individual
previous comparison nonsignificant; conclude throughput is unchanged within
measurement noise. Candidate `richards` stock sampling also has a high
11.9 ms standard deviation, so cross-run stock-score shifts are not causal.

| Codegen/coverage guardrail | Baseline | Candidate |
| --- | --- | --- |
| Compiled benchmark functions | 35 / 21 / 79 / 53 | 35 / 21 / 79 / 53; unchanged |
| Optimized typed-IR final basic blocks | 2,541 | 2,541; unchanged |
| Optimized typed-IR function instances | 193 | 193; unchanged |
| Pre-optimization serialized BlockPy bytes | 8,171,776 | 8,171,456; 320 bytes smaller |
| Apply-mode native emitted bytes | 18,674,240 | 18,674,240; unchanged |
| Apply-mode native machine blocks | 1,236,140 | 1,236,140; unchanged |

## Attempt history

### Attempt 1: Give all compiler-generated stop handlers trusted provenance

- Change: bind four synthetic sync/async stop handlers through the private
  `__soac__` runtime object while leaving user-authored handlers lexical.
- Evidence: structured lowerer regression genuinely fails because existing
  synthetic handlers contain a bare, user-shadowable AST `Name`.
- Compatibility: module-global `StopIteration = ValueError` must not affect
  ordinary iteration or comprehensions, but explicit user handlers must
  continue catching `ValueError`; async variants require equivalent trusted
  exception binding.
- Tests: structured lowerer RED→GREEN (**1 passed**) and full lowerer suite
  **368 passed**; dedicated
  `tests/test_synthetic_iteration_exception_shadowing.py` Profile→Apply
  integration **1 passed in 4.84 seconds** across all sync/async synthetic
  and explicit-handler cases; combined iterator/original-code/asyncgen/
  shutdown regressions **21 passed in 13.01 seconds**; combined lowerer/JIT
  Cargo check and scoped lowerer formatting/checks pass. Four-workload
  debug-single release smoke and full normal-sampling comparison complete;
  typed IR, native bytes/machine blocks, and coverage are unchanged, and all
  four previous-SOAC comparisons are nonsignificant. Full `just test-all`
  passes **1,212 Python nodeids / 76 batches**, plus all relevant Rust suites.
- Result: **LANDED / RETAIN as correctness only**. This fix is independent of
  the rejected runtime-helper fast-path strategy and does not claim a
  throughput win.

## Verdict and next action

- Verdict: **LANDED / RETAIN as a general CPython-correctness fix.** All
  focused sync/async shadowing tests and the full correctness gate pass;
  normal sampling shows no statistically established throughput change and
  generated native code is identical. The **0.2780522558x** four-workload
  stock ratio remains far below the full-suite **1.10x** goal.
- Transferable lesson: compiler-synthesized implicit language semantics must
  resolve trusted runtime bindings, while user-authored syntax must retain
  normal lexical name resolution.
- Next action: continue with a separate semantics-safe general-purpose
  optimization strategy; do not reintroduce the rejected mutable-global
  runtime-helper bypass.
