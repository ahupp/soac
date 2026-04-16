# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/assign_stmt/mod.rs

## File Responsibilities

Lowers assignment targets and assignment statements, including name stores, attribute/subscript
stores, tuple/list unpacking, target-object lowering, and temporary binding/cleanup for
multi-target evaluation order.

## Datatypes

- `UnpackTargetKind`: identifies whether an unpack target was tuple-shaped or list-shaped; currently
  both use the same list conversion for starred targets.

## Functions

- `rhs_temp_name`: creates a synthetic name for assignment temporaries.
- `temp_load_expr`: builds a load expression for a temp.
- `bind_temp`: stores a value into a temp and returns a load of that temp.
- `delete_temp`: emits a delete of a temp.
- `lower_target_object_with_setup`: lowers an assignment/delete target object with setup statements.
- `lower_assignment_target_into`: recursively lowers assignment to names, attributes, subscripts,
  tuple targets, and list targets.
- `lower_unpack_target_into`: calls `__soac__.unpack`, binds the result, assigns each unpacked item,
  converts starred targets to lists, and deletes the unpack temp.
- `should_bind_assignment_value`: decides whether RHS must be bound to a temp before target stores.
- `lower_assign_instr_into`: lowers RHS once, binds it if needed, applies all targets, then cleans up
  the temp.
- `build_for_target_assign_body`: builds the small assignment/delete sequence used to bind for-loop
  targets from the loop temporary.
- `with_target_object_expr`: AST-level helper for assignment target object rewrites.
- `rewrite_assignment_target`: AST-level assignment target rewrite used by structured with-stmt
  desugaring.
- `rewrite_unpack_target`: AST-level unpack assignment rewrite used before BlockPy lowering.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/mod.rs`
- `soac-blockpy/src/passes/ast_to_ast/expr_utils.rs`
