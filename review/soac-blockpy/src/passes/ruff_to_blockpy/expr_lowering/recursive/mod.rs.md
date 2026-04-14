# soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/recursive/mod.rs

## File Responsibilities

Performs recursive expression lowering for Ruff-shaped BlockPy expressions. It delegates special
forms with Python evaluation-order behavior to focused lowerers, lowers dict items explicitly, and
maps ordinary expression children recursively.

## Datatypes

- None.

## Functions

- `lower_raw_ast_expr`: converts a raw Ruff AST expression to `InstrRuff`, lowers it recursively,
  and converts it back to a raw AST expression.
- `lower_expr_ast_recursive`: dispatches boolops, comparisons, if-expressions, named expressions,
  and dicts to specialized logic; rejects statement-shaped instructions; recursively maps ordinary
  expression children.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/boolop_compare/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/if_expr/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/named_expr/mod.rs`

