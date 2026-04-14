# soac-blockpy/src/passes/ast_to_ast/rewrite_class_def/class_body.rs

## File Responsibilities
Rewrites class-body statements so class namespace execution can be represented as helper functions while preserving selected nested scopes and hoisted definitions.

## Datatypes
- `ClassBodyScopeRewriter`: transformer that tracks the current class metadata, semantic state, and hoisted statements while rewriting class-body scopes.

## Functions
- `rewrite_class_body_scopes`: entry point for rewriting a class body with semantic and scope context.
- `ClassBodyScopeRewriter::new`: constructs the scoped rewriter.
- `ClassBodyScopeRewriter::take_hoisted`: drains statements that must be emitted outside the current position.
- `ClassBodyScopeRewriter::visit_body` / `visit_stmt`: transform nested bodies and statements while preserving correct class-body semantics.
- `ClassBodyScopeRewriter::rewrite_stmt_list`: expands or hoists individual statements that need class-body treatment.

## Context Read
- `soac-blockpy/src/passes/ast_to_ast/rewrite_class_def/mod.rs` for the larger class rewrite.
- `soac-blockpy/src/passes/ast_to_ast/semantic/mod.rs` for semantic scope data used by the rewriter.
