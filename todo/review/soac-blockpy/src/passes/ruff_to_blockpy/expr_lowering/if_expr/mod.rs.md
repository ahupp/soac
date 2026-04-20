# soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/if_expr/mod.rs

## File Responsibilities

Lowers Python conditional expressions (`a if test else b`) into inline BlockPy fragments with a
synthetic result temp and explicit branch blocks.

## Datatypes

- None.

## Functions

- `store_name`: builds a store-context synthetic name.
- `load_name`: builds a load of a synthetic name.
- `assign_name`: emits a store instruction for the synthetic result value.
- `try_lower_if_expr_direct`: lowers test, then expression, and else expression into separate
  inline fragments, connects them with an if terminator, and returns a load of the result temp.
- `lower_if_expr_into`: appends the direct fragment to an existing statement builder and returns
  the expression result temp load.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/mod.rs`

