# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/delete_stmt/mod.rs

## File Responsibilities

Lowers delete statements for names, attributes, and subscripts into BlockPy delete/helper/item
operations with correct target-object setup.

## Datatypes

- None.

## Functions

- `lower_delete_target_into`: lowers one delete target, including temp-binding object and index
  expressions for subscript deletes and using `delattr` for attribute deletes.
- `lower_delete_instr_into`: lowers all targets in a delete statement in order.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/assign_stmt/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/mod.rs`

