# soac-blockpy/src/passes/ast_to_ast/util.rs

## File Responsibilities
Provides small predicate and name-normalization helpers shared by AST rewrite passes.

## Datatypes
- None.

## Functions
- `is_noarg_call`: recognizes calls to a bare name with no positional or keyword arguments.
- `is_dp_helper_lookup_expr`: recognizes `__soac__.<helper>` attribute lookup expressions.
- `strip_synthetic_module_init_qualname`: removes the synthetic module-init qualname prefix.
- `strip_synthetic_class_namespace_qualname`: removes synthetic class namespace frames from qualnames.

## Context Read
- Class and method rewrite modules that inspect helper lookups and qualnames.
