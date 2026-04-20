# soac-blockpy/src/passes/ruff_to_blockpy/module_plan/callable_scope.rs

## File Responsibilities

Builds `CallableScopeInfo` for lowered BlockPy functions from the semantic AST scope model. It
normalizes generated function names/qualnames, converts semantic binding categories to BlockPy
binding kinds, and repairs class-cell/capture metadata.

## Datatypes

- None.

## Functions

- `is_module_init_name`: recognizes synthetic module-init function names.
- `display_name_for_function`: maps synthetic lambdas/comprehensions to CPython-style display
  names.
- `normalize_qualname`: strips synthetic wrapper qualnames and replaces selected synthetic tails.
- `blockpy_binding_kind_for_name`: converts semantic local/nonlocal/global binding information to
  BlockPy local/global/cell binding kinds.
- `parameters_contain_name`: checks whether a function parameter list contains a specific name.
- `callable_owns_synthetic_classcell`: detects whether a function owns the synthetic classcell
  argument.
- `callable_scope_info`: builds complete `CallableScopeInfo`, including effective load/store
  bindings, cell storage/capture names, type-param names, function names, and synthetic classcell
  ownership.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/module_plan/mod.rs`
- `soac-blockpy/src/block_py/scope.rs`
- `soac-blockpy/src/passes/ast_to_ast/semantic/mod.rs`

