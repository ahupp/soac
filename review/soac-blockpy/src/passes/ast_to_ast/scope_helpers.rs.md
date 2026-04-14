# soac-blockpy/src/passes/ast_to_ast/scope_helpers.rs

## File Responsibilities
Defines common scope classifications and naming helpers used by AST rewrites.

## Datatypes
- `ScopeKind`: lexical scope category: function, class, or module.

## Functions
- `is_internal_symbol`: identifies SOAC-generated/internal names that should receive special treatment.
- `cell_name`: returns the synthetic cell-storage name for a logical source name.

## Context Read
- `context.rs` for `ScopeFrame` use of `ScopeKind`.
- Class and semantic rewrite modules for synthetic cell naming.
