# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/augassign_stmt/mod.rs

## File Responsibilities

Lowers augmented assignment while preserving Python evaluation order for name, attribute, and
subscript targets. It binds target object/index/current value temps before evaluating the RHS and
then emits a store/set operation with the in-place binary operation value.

## Datatypes

- None.

## Functions

- `lower_augassign_instr_into`: lowers augmented assignment for name, attribute, and subscript
  targets; rejects unsupported targets with a formatted assignment/delete diagnostic.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/assign_stmt/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`

