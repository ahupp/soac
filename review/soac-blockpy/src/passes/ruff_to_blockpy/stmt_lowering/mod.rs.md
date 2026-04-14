# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs

## File Responsibilities

Coordinates statement lowering from `InstrRuff` and raw Ruff AST statements into BlockPy inline
builders. It simplifies statement heads, delegates specialized statement forms to submodules,
plans sequence-control boundaries, and lowers direct statement forms into statements or block
terminators.

## Datatypes

- `BlockPyStmtBuilder<E>`: alias for `InlineBlockBuilder<E>` used while lowering statements.
- `StructuredLoweringBridge`: small adapter for running structured lowering into a fresh inline
  builder.

## Functions

- `StructuredLoweringBridge::new`: constructs the bridge.
- `StructuredLoweringBridge::try_lower_inline_value`: runs a lowering closure in a new inline
  builder and returns the builder plus produced value.
- `try_lower_inline_value_from_structured`: implementation of bridge lowering.
- `stmts_from_rewrite`: converts a rewrite result to raw statements.
- `instrs_from_rewrite`: converts a rewrite result to `InstrRuff` statements.
- `single_stmt`: wraps one statement in a vector.
- `lower_nested_stmt_into_with_expr`: simplifies nested raw AST statements when needed and lowers
  them into an inline builder.
- `should_simplify_nested_stmt_head`: selects statement kinds that should pass through one
  simplification step before nested lowering.
- `simplify_stmt_ast_once_for_blockpy`: applies one AST-level normalization/desugaring step for
  each statement kind.
- `simplify_stmt_head_ast_for_blockpy`: applies one simplification step plus final head cleanup.
- `simplify_instr_head_for_blockpy`: applies equivalent head simplification for `InstrRuff`
  statements.
- `finish_stmt_head_ast_for_blockpy`: adjusts simplified raw if-test expression heads.
- `plan_simplified_instr_head_for_blockpy`: maps a simplified statement to `StmtSequenceHeadPlan`.
- `plan_instr_head_for_blockpy`: public planning wrapper for one instruction statement.
- `lower_instr_for_test`: test-only wrapper around production statement lowering.
- `lower_instr_into_with_expr`: lowers one statement into the builder, setting terminators for
  return/raise/break/continue and rejecting statement forms that should have been normalized
  earlier.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/assign_stmt/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/if_stmt/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/try_stmt/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/mod.rs`

