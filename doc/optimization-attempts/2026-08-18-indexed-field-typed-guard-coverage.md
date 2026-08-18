---
title: "Indexed-Field Typed-Guard Coverage"
---

# Indexed-field typed-guard coverage

- Status: landed
- Pacific date: 2026-08-18 PDT
- Change or revision: `tmtwnonn`
- Outcome: restored `richards` and `deltablue` apply-mode coverage without
  increasing the `chaos` typed IR or generated native code; both newly runnable
  benchmarks remain substantially slower than stock CPython.

## Hypothesis and evidence

Profile-guided indexed-field specialization should support representative
class-heavy Python workloads without allowing an unguarded direct field load.
Before this change, the `richards` and `deltablue` pyperformance benchmarks
completed their profile passes but failed during specialized apply: an exact-int
comparison region attempted to borrow `Packet.kind` or `Variable.value` from an
indexed field that had no corresponding typed owner/layout guard.

The ordinary attribute specialization and the enclosing exact-int/exact-string
region were selected independently. Eager whole-module compilation can occur
before the profiled owner class is resolvable, so the attribute site correctly
falls back to generic access. Later typed rewrites can also remove or replace a
guarded attribute instruction. Previously, neither case invalidated the
dependent scalar-region sidecar, and code generation correctly rejected the
resulting unsafe plan.

`chaos` already completed stock, profile, and apply passes before the change,
making it the numeric sanity baseline. Because `richards` and `deltablue`
previously failed, no pre-change SOAC elapsed time exists for either benchmark.

## Implementation and compatibility

- After the final typed rewrites, validate every borrowed indexed-field input
  used by an exact-int/exact-string branch or return region against an actual
  surviving typed indexed-field guard.
- Require the source instruction, owner module, owner qualified name, and
  expected field index to match. Preserve valid specializations and discard
  only dependent branch/return sidecars whose required guard is absent or
  mismatched.
- Retain the original generic attribute/comparison operation when a region is
  discarded; do not invent guards in code generation or dereference an
  unguarded field.
- Preserve CPython-visible attribute lookup and mutation, owner/layout
  assumptions, evaluation order, exceptions, refcounts, descriptors, and
  monkeypatch behavior.
- Structured regression coverage:
  `exact_int_indexed_field_sidecars_preserve_live_typed_guards`,
  `exact_int_indexed_field_sidecars_reject_missing_typed_guards`, and
  `exact_int_indexed_field_sidecars_reject_mismatched_typed_guards`. The
  missing-guard regression failed before the fix; all three and the complete
  547-test `soac_jit` package passed afterward.

## Benchmark protocol and coverage

- Previous-SOAC baseline:
  `work/pyperformance/comparison-20260818-082800-OVOfOR/summary.json`.
- Candidate `chaos` comparison:
  `work/pyperformance/comparison-20260818-093216-EiPVAa/summary.json`.
- Candidate `richards,deltablue` comparison:
  `work/pyperformance/comparison-20260818-092945-1kFqqF/summary.json`.
- `chaos` candidate: three independently started, order-alternated paired
  stock/SOAC rounds using `--debug-single-value`; the previous-SOAC baseline
  has one exploratory value, so its apparent improvement is descriptive rather
  than a statistically established before/after win.
- `richards,deltablue`: one exploratory paired round using
  `--debug-single-value`; the slowdown ratios are diagnostic, not rigorous
  final performance claims.
- Each SOAC candidate run creates fresh profile evidence before the measured
  apply pass.
- All three benchmarks transform their `__main__` benchmark code and
  `soac.runtime`; no standard-library module is transformed.
- Compiled apply functions: `chaos` 35, `richards` 53, `deltablue` 79.
- Before the fix: `richards` and `deltablue` profile succeeded but apply
  failed. Afterward: both profile and apply complete.
- Full-suite completion, coverage, and geometric mean: not measured.

## Measurements

| `chaos` metric | Previous SOAC baseline | Candidate | Change |
| --- | --- | --- | --- |
| Stock CPython elapsed | 30.31 ms | 29.54 ms | n/a |
| SOAC apply elapsed | 215.22 ms | 192.72 ms | -10.46% elapsed |
| Stock / SOAC speedup | 0.141x | 0.153x | +8.86% relative |
| Previous SOAC / candidate SOAC | n/a | 1.117x | +11.68% throughput |
| Optimized typed-IR final blocks | 846 | 846 | unchanged |
| Optimized typed-IR functions | 35 | 35 | unchanged |
| Pre-optimization BlockPy bytes | 991,150 | 991,150 | unchanged |
| Apply-mode native code bytes | 717,688 | 717,688 | unchanged |
| Apply-mode machine blocks | 47,728 | 47,728 | unchanged |

| Newly runnable benchmark | Before | Stock CPython | SOAC apply | SOAC versus stock | Compiled functions |
| --- | --- | --- | --- | --- | --- |
| `richards` | apply failed | 22.10 ms | 381.46 ms | 17.26x slower | 53 |
| `deltablue` | apply failed | 1.47 ms | 484.26 ms | 328.69x slower | 79 |

The combined `richards,deltablue` run emitted 1,016 optimized typed-IR blocks
across 132 compiled function instances, 680,096 native-code bytes, and 45,224
machine blocks; its serialized pre-optimization BlockPy totaled 2,164,420
bytes.

## Attempt history

### Attempt 1: Establish the baseline and reproduce both failed apply passes

- `chaos`: SOAC apply 215.22 ms versus stock CPython 30.31 ms, or 0.141x
  stock/SOAC.
- `richards`: apply fails on the borrowed `Packet.kind` indexed-field input.
- `deltablue`: apply fails on the borrowed `Variable.value` indexed-field input.
- Initial native output: 717,688 bytes and 47,728 machine blocks; optimized
  typed IR: 846 blocks across 35 functions.
- Result: coverage failure reproduced; no throughput can be attributed to the
  two workloads that failed before completing apply.

### Attempt 2: Reject scalar-region sidecars without matching live typed guards

- Added a failing structured regression for the absent-guard shape and
  validated the existing guarded control.
- Added final dependency-aware sidecar validation, preserving guarded plans
  while rejecting missing guards, wrong owner identity, and wrong field index.
- `richards` now completes at 381.46 ms versus stock's 22.10 ms; the planner
  rejects unguarded branch inputs in `HandlerTask.fn` and `WorkTask.fn`.
- `deltablue` now completes at 484.26 ms versus stock's 1.47 ms; the planner
  rejects two unguarded branch inputs in `projection_test`.
- Three-round candidate `chaos` median: 192.72 ms versus 29.54 ms stock;
  optimized IR, generated code, and machine-block counts are unchanged.
- Result: retained for semantic safety and benchmark coverage. The
  previous-SOAC 1.117x ratio is exploratory because the baseline has only one
  sample; the severe `richards` and `deltablue` slowdowns remain unresolved.

## Verdict and next action

- Verdict: landed as a correctness and benchmark-coverage fix, not proof of a
  full-suite pyperformance improvement or the 10%-faster-than-stock goal.
- Transferable lesson: profile-driven scalar-region inputs depend on the final
  surviving typed attribute guards; profile success or a nominally selected
  indexed-field optimization cannot prove that dependency remains valid.
- Next action: profile `deltablue` and `richards`, identify why their transformed
  execution remains approximately 329x and 17x slower than stock, and record
  each subsequent optimization strategy in its own tracked file.
