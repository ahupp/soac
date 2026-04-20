# soac-blockpy/src/passes/ast_to_instr.rs

## File Responsibilities
Converts between Ruff AST nodes and SOAC's `InstrRuff` IR. It wraps AST expressions/statements into instruction variants, normalizes and denormalizes `if`/`elif` structures, and can reconstruct Ruff AST from `InstrRuff` for passes that still operate on Ruff-shaped trees.

## Datatypes
- `InstrRuffAstExt`: extension trait defining bidirectional conversion between Ruff AST suites/expressions/statements and `InstrRuff`.

## Functions
- `InstrRuffAstExt::wrap_ast_expr` / `wrap_ast_stmt`: attach metadata and wrap a typed operation as an `InstrRuff` expression or statement.
- `none_expr_with_meta`: constructs a `None` expression with explicit metadata.
- `from_ast_suite` / `into_ast_suite`: convert whole statement bodies between Ruff and `InstrRuff` form.
- `normalize_if_orelse` / `denormalize_if_orelse`: convert Ruff `elif` clauses to and from nested `StmtIf` shape.
- `from_ast_expr` / `into_ast_expr`: large variant-by-variant conversion between Ruff expressions and `InstrRuff` expression variants.
- `from_ast_stmt` / `into_ast_stmt`: large variant-by-variant conversion between Ruff statements and `InstrRuff` statement variants.

## Context Read
- `soac-blockpy/src/passes/mod.rs` for all `InstrRuff` variants.
- `soac-blockpy/src/block_py/operation.rs` and related IR payload modules for constructed operation types.
