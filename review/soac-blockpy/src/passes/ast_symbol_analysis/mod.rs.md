# soac-blockpy/src/passes/ast_symbol_analysis/mod.rs

## File Responsibilities
Provides shallow, current-scope symbol collection over Ruff AST statements and expressions. It records names bound or loaded by the current syntactic scope while deliberately not descending into nested function, class, lambda, and comprehension scopes where those names belong to a different scope.

## Datatypes
- `CurrentScopeNameTraversal`: transformer extension trait that owns the common current-scope traversal rules for bound and loaded names.
- `CurrentScopeNameCollector`: concrete collector for loaded and bound names.
- `ExplicitGlobalOrNonlocalCollector`: test-only collector for explicit `global`/`nonlocal` declarations.

## Functions
- `CurrentScopeNameTraversal::record_bound_name` / `record_loaded_name`: insert a name into the appropriate set.
- `CurrentScopeNameTraversal::visit_current_scope_stmt_impl`: handles statement-specific binding forms such as assignment targets, imports, `for`/`with` targets, exception aliases, function/class definitions, and declaration boundaries.
- `CurrentScopeNameTraversal::visit_current_scope_expr_impl`: handles expression loads, named-expression stores, and skips nested expression scopes.
- `CurrentScopeNameCollector::visit_stmt` / `visit_expr`: route transformer traversal through the trait implementation.
- `collect_loaded_names`: returns names loaded in the current statement slice.
- `collect_bound_names`: returns names bound in the current statement slice.
- `collect_explicit_global_or_nonlocal_names`: test-only helper for explicit declaration names.
- `collect_assigned_names`: recursively extracts assignment target names from names, tuples, lists, and starred targets.
- `import_binding_name`: returns the actual local binding introduced by an import alias.

## Context Read
- `soac-blockpy/src/transformer.rs` for traversal hooks.
- `soac-blockpy/src/passes/ast_to_ast/semantic/mod.rs` for consumers that extend `CurrentScopeNameTraversal`.
