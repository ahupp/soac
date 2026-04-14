# soac-blockpy/src/passes/blockpy_expr_simplify/mod.rs

## File Responsibilities
Simplifies Ruff-shaped `InstrRuff` expressions into core BlockPy expressions with explicit operations, runtime helper calls, literals, await/yield nodes, and collection construction. It is the bridge from source-like AST operations to SOAC's core operation vocabulary.

## Datatypes
- No standalone public datatypes; the file primarily transforms `InstrRuff` into `InstrWithAwaitAndYield`.

## Functions
- Literal/runtime constructors such as `core_builtin_name`, `number_literal_expr_with_meta`, `complex_literal_expr_with_meta`, `core_operation_expr`, `core_operation_expr_with_meta`, and `core_runtime...` call helpers build core expressions with metadata.
- Operation lowering helpers such as `unary_op_expr_with_meta`, `binop_expr_with_meta`, `getattr_expr_with_meta`, `getitem_expr_with_meta`, `*_from_ast_with_meta`, and `add_op_expr_with_meta` map Ruff operators to core operation nodes and special runtime calls.
- Collection lowering helpers including `reduce_core_blockpy_dict`, tuple/list/set/dict handling, and splat lowering translate Python collection syntax into runtime-friendly operations.
- Call and make-function helpers parse literal arguments, function ids, function kinds, and string arguments for SOAC helper calls.
- `non_operator_operation_from_helper_call` recognizes helper calls that can become first-class core operations.
- `lower_core_call_expr_with_meta`: lowers call expressions into core call operations, preserving args/keywords and known runtime helpers.
- `InstrWithAwaitAndYield` conversion implementation lowers each `InstrRuff` expression/statement variant to the next pipeline IR.

## Context Read
- `soac-blockpy/src/passes/mod.rs` for `InstrRuff` and `InstrWithAwaitAndYield` variants.
- `soac-blockpy/src/block_py/operation.rs` for core operation payloads.
- `soac-blockpy/src/passes/ast_to_ast/string_templates.rs` for string-template lowering interaction.
