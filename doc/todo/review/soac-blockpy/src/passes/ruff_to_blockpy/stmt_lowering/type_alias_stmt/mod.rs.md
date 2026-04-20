# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/type_alias_stmt/mod.rs

## File Responsibilities

Rewrites PEP 695 type-alias statements into runtime calls that construct `TypeAliasType` and any
needed type parameters, then cleans temporary type parameter names.

## Datatypes

- `TypeParamInfo`: generated binding statements, temp parameter names, and optional tuple of type
  parameter objects.

## Functions

- `make_type_param_info`: lowers `TypeVar`, `TypeVarTuple`, and `ParamSpec` declarations into
  runtime helper binding statements and a type-params tuple.
- `rewrite_type_alias_stmt`: rewrites a name-target type alias into helper calls, preserving
  type-parameter setup and cleanup; leaves unsupported alias targets unmodified.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`
- `soac-blockpy/src/passes/ast_to_ast/expr_utils.rs`

