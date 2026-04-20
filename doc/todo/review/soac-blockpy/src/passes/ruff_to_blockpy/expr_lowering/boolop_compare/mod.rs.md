# soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/boolop_compare/mod.rs

## File Responsibilities

Lowers short-circuiting boolean expressions and chained comparisons into inline BlockPy fragments.
Single comparisons are left as ordinary compare expressions; multi-step forms get explicit temp
stores and conditional blocks so evaluation order and short-circuit behavior are preserved.

## Datatypes

- None.

## Functions

- `store_name`: builds a store-context Ruff name for synthetic temps.
- `load_name`: builds a load expression for a synthetic temp.
- `assign_name`: emits a BlockPy store instruction to a synthetic temp.
- `try_lower_branching_expr_direct`: dispatches boolop and multi-comparison expressions to direct
  fragment lowering.
- `lower_boolop_direct`: lowers `and`/`or` by storing each candidate result in a temp and branching
  to the next value only when Python short-circuit semantics require it.
- `lower_compare_chain_direct`: lowers chained comparisons by preserving intermediate comparators,
  emitting each comparison into a temp, and branching to the next comparison only if the previous
  one was true.
- `lower_boolop_into`: appends the direct boolop fragment into an existing statement builder and
  returns the temp load that represents the expression value.
- `lower_compare_into`: lowers a single comparison directly or appends a chained-comparison
  fragment and returns its result temp.
- `compare_expr`: builds a one-step `ExprCompare` instruction from an operator and two operands.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/mod.rs`
- `soac-blockpy/src/block_py/instr.rs`

