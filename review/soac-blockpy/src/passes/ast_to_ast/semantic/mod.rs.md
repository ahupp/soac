# soac-blockpy/src/passes/ast_to_ast/semantic/mod.rs

## File Responsibilities
Builds and stores a semantic-scope snapshot for transformed Ruff AST. It assigns stable node indices, approximates Ruff semantic binding/scope data after rewrites, tracks globals/nonlocals/freevars/cells, handles implicit `__class__` cell use, and exposes scope lookup APIs for later AST rewrites.

## Datatypes
- `SemanticScopeId`: local identifier for a semantic scope in the snapshot.
- `SemanticScopeData`: stored data for one scope, including kind, parent, qualname, bindings, globals/nonlocals, freevars, cells, and type-param/capture metadata.
- `SemanticSnapshot`: collection of scope data plus node-to-scope maps.
- `SemanticProvenance`: assigns/fixes node indices and maps functions/lambdas to semantic scopes.
- `MaxNodeIndexCollector` / `MissingNodeIndexAssigner`: transformers for node-index normalization.
- `SemanticStateInner`: shared inner state for semantic queries.
- `SemanticScope`: query wrapper for a specific scope in a `SemanticAstState`.
- `RuffScopeBindingCollector`: current-scope binding/load collector for semantic construction.
- `ImplicitClassCellUseDetector`: detects zero-arg `super()` or `__class__` references.
- `ScopePreparation`: temporary scope-building record.
- `RuffSemanticSnapshotBuilder`: transformer that constructs the full semantic snapshot.
- Public `SemanticAstState` is implemented here and represents immutable semantic state attached to the rewritten AST.

## Functions
- Node-index helpers: `next_node_index_for_suite`, `ensure_node_indices_for_suite`, `SemanticProvenance::ensure_node_index`, and function/lambda scope lookup methods keep rewritten AST nodes addressable.
- `SemanticScope` methods expose parent/child relationships, scope kind, qualnames, bindings, cells, freevars, globals, nonlocals, and storage names.
- `child_qualname`: creates nested qualnames.
- Implicit class-cell helpers: `is_super_call`, `uses_implicit_class_cell`, and `expr_uses_implicit_class_cell` recognize CPython class-cell cases.
- Binding collectors: `RuffScopeBindingCollector` visitor methods, `collect_scope_bindings`, `collect_scope_expr_bindings`, `merge_semantic_binding`, and `set_semantic_binding` gather and merge scope-local bindings.
- `RuffSemanticSnapshotBuilder::build`: entry point for semantic snapshot construction.
- Builder methods `prepare_current_scope`, `prepare_current_expr_scope`, `visit_function_definition_exprs`, `prepare_scope_from_collector`, `push_snapshot_scope`, `propagate_nonlocal_roots`, `compute_local_cell_bindings`, and related helpers create scope records and propagate closure information.
- `parameter_refs`: extracts parameter names/ranges for scope binding.
- `SemanticAstState` methods construct state, query scopes for functions/lambdas/nodes, and expose scope wrappers.

## Context Read
- `ast_symbol_analysis/mod.rs` for current-scope collection rules.
- `context.rs` and `scope_helpers.rs` for scope kinds and generated storage names.
- `rewrite_class_def/*` for semantic consumers.
