# soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/named_expr/mod.rs

## File Responsibilities

Lowers walrus/named expressions by evaluating the value, storing it into the target name, and
returning a load expression for that same name.

## Datatypes

- None.

## Functions

- `into_store_name`: preserves a Ruff name as the store target name.
- `into_load_name`: rebuilds a Ruff name expression with load context.
- `lower_named_expr_into`: validates that the target is a name, lowers the value, emits a store,
  and returns the load-context target expression.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`

