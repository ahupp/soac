---
title: "Branch Predicate Lowering"
---

# Branch Predicate Lowering

## Goal

Lower branch-context comparisons to predicate values instead of always
lowering them as owned Python bool objects followed by `dp_jit_is_true`.

This is intentionally larger than another helper reshuffle. The pystone
experiments showed that moving truth conversion into either generated CLIF,
`dp_jit_is_true`, or a combined richcompare/truth helper is benchmark-negative
when it still produces an owned compare result or adds many branch blocks.

## Current shape

For `if left OP right`, codegen currently emits the expression path:

1. emit owned `left`
2. emit owned `right`
3. call `PyObject_RichCompare` or a profiled exact-long richcompare slot
4. decref `left` and `right`
5. call `dp_jit_is_true(compare_result)`
6. decref `compare_result`
7. branch on the `i32` truth

This is correct, but it preserves the object-result contract even when the
result is immediately consumed by a branch.

## Proposed architecture

Add an explicit predicate-lowering path used only by branch terms and other
truth-only contexts.

1. Add a codegen function such as `emit_codegen_predicate(...) -> PredicateValue`.
   It should be separate from `emit_codegen_expr` so expression contexts keep
   returning owned `PyObject *`.

2. Teach `BlockTerm::IfTerm` to call predicate lowering first.
   The generic fallback remains: emit an owned object expression, call
   `dp_jit_is_true`, decref the owned object, and branch.

3. Add comparison predicate lowering.
   For a comparison in predicate context, emit owned operands and produce an
   `i32` / CLIF boolean predicate plus an explicit error path.

4. Start with exact-builtin fast paths whose predicate semantics are the same
   as Python expression semantics:
   - exact compact `int` comparisons
   - exact `str` equality / ordering if a CPython API or SOAC helper can return
     bool without allocating a Python bool
   - identity comparisons, which are already predicate-shaped in expression
     lowering but still produce an owned bool object

5. Keep the generic richcompare fallback sound.
   Do not blindly replace Python branch comparisons with
   `PyObject_RichCompareBool`: that API has identity shortcuts for `==` / `!=`
   that are not equivalent to arbitrary Python `__eq__` results. It may be
   valid behind exact-builtin guards.

6. Outline uncommon miss/error paths if the predicate fast path would otherwise
   add many blocks to the hot path.

## Validation

- Render specialized pystone CLIF before/after for `Proc0`.
- The hot comparison branches should not contain the sequence
  `PyObject_RichCompare` -> `dp_jit_is_true`.
- The replacement should either be straight CLIF comparison or one small
  exact-builtin bool helper call plus a compact error branch.
- Run `just benchmark-verify 100000`.
- Run `just benchmark`; report all three specialized runs.
- Collect specialized perf and confirm the `dp_jit_is_true` / richcompare
  consumer stack falls without moving the same cost into a new helper.
