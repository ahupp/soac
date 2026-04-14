# soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/mod.rs

## File Responsibilities

Defines the expression-lowering interface from Ruff-shaped BlockPy instructions into core BlockPy
instructions, including helper-call construction, attribute/item operations, augmented assignment
value construction, and special recognition of core `__soac__` helper calls.

## Datatypes

- `RuffToBlockPyExpr`: trait implemented by target instruction types that can accept lowered Ruff
  expressions plus core operations such as store/delete, helper calls, attribute/item access, and
  augmented assignment values.
- `BlockPySetupExprLowerer`: trait that owns recursive expression lowering into a setup-statement
  builder.
- `AstSetupExprLowerer`: default lowerer implementation that uses recursive AST-shaped lowering.

## Functions

- `string_literal_expr`: creates an `InstrWithAwaitAndYield` string literal with metadata.
- `RuffToBlockPyExpr::from_lowered_expr`: converts lowered Ruff expression into target expression.
- `RuffToBlockPyExpr::helper_call`: builds a runtime helper call in the target expression type.
- `RuffToBlockPyExpr::lower_augassign_value`: builds the value operation for augmented assignment.
- `RuffToBlockPyExpr::load_deleted_name`: wraps a load so deleted-name errors can be preserved.
- `RuffToBlockPyExpr::{get_attr,set_attr,get_item,set_item,del_item}`: build core attribute and
  item operations.
- `inplace_kind`: maps Ruff augmented operators to BlockPy binary operation kinds.
- `InstrWithAwaitAndYield` implementation of `RuffToBlockPyExpr`: converts recognized runtime
  helper calls to direct core operations and otherwise wraps Ruff expressions.
- `BlockPySetupExprLowerer::lower_expr_instr_into`: lowers string templates, then recursively
  lowers an expression into setup statements.
- `BlockPySetupExprLowerer::lower_expr_into`: lowers and converts directly to the target
  expression type.
- `lower_expr_head_ast_for_blockpy`: currently identity hook for expression heads before statement
  planning.
- `lower_expr_into_with_setup`: convenience wrapper using `AstSetupExprLowerer`.
- `make_function_kind_from_literal`: parses a function-kind string literal for direct
  `MakeFunction` lowering.
- `make_function_id_from_literal`: parses a packed function-id numeric literal.
- `string_literal_value`: extracts a Rust string from a Ruff string literal instruction.
- `lowered_helper_call`: recognizes a call to `__soac__.<helper>` with fixed arity and no keywords.
- `lower_direct_core_helper_expr`: converts selected runtime helper calls (`make_function`,
  `store_global`, `cell_ref`) to core BlockPy operations.
- `fresh_setup_name`: creates a fresh synthetic setup temp name.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/recursive/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/boolop_compare/mod.rs`
- `soac-blockpy/src/block_py/operation.rs`
- `soac-blockpy/src/block_py/mod.rs`

