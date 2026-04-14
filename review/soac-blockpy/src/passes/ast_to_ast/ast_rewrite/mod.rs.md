# soac-blockpy/src/passes/ast_to_ast/ast_rewrite/mod.rs

## File Responsibilities
Implements the reusable Ruff-AST rewrite driver. It repeatedly applies statement and expression rewrite passes, supports expression lowering that emits preceding statements, tracks lexical scope frames while recursing, and preserves source ranges after expression rewrites.

## Datatypes
- `Rewrite`: statement-pass result, either preserving one statement or replacing it with a walked statement list.
- `LoweredExpr`: expression-pass result containing generated statements, replacement expression, and a modified flag.
- `RewriteLoop`: internal transformer that owns buffered generated statements, pass references, scope context, and modification state.
- `StmtRewritePass`: trait for statement-level lowering passes.
- `BBRewritePass`: statement-lowering trait alias point for BlockPy-oriented rewrites; implemented as a `StmtRewritePass` adapter.
- `ExprRewritePass`: trait for expression-level lowering passes that may emit statements.

## Functions
- `LoweredExpr::modified` / `unmodified`: construct expression rewrite results.
- `rewrite_once_with_pass`: runs one traversal pass and reports whether it changed the tree.
- `rewrite_with_pass`: repeats `rewrite_once_with_pass` to fixed point.
- `RewriteLoop::flush_buffered`: walks rewritten statements, descends into nested function/class scopes with correct `ScopeFrame`s, and splices buffered helper statements before the current statement.
- `RewriteLoop::process_statements`: applies the configured statement pass to a statement list and routes rewritten statements through `flush_buffered`.
- `RewriteLoop::visit_body` / `visit_stmt`: replace statement lists or single statements through the rewrite pipeline.
- `RewriteLoop::visit_expr`: repeatedly applies the expression pass, buffers emitted statements, preserves original source range, and only walks children after no direct rewrite occurs.
- `collect_declared_bindings`: collects `global` and `nonlocal` declarations inside a function body without crossing nested scopes.
- `apply_expr_range`: rewrites the range field for every Ruff expression variant.

## Context Read
- `soac-blockpy/src/passes/ast_to_ast/context.rs` for scope stack state.
- `soac-blockpy/src/passes/ast_to_ast/rewrite_expr/mod.rs` and `rewrite_stmt/annotation.rs` for concrete pass implementations.
