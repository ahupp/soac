# soac-blockpy/src/passes/ast_to_ast/rewrite_stmt/decorator/mod.rs

## File Responsibilities
Provides decorator-list utilities that convert decorators into ordinary call expressions applied in Python decorator order.

## Datatypes
- None.

## Functions
- `collect_exprs`: clones decorator expressions from borrowed decorator nodes.
- `into_exprs`: consumes decorators into their expressions.
- `rewrite_exprs`: wraps a target expression with decorator calls in reverse application order.
- `rewrite`: convenience function that consumes decorators and decorates the target expression.

## Context Read
- Ruff AST decorator and call-expression structures.
- Class/function rewrite modules that apply decorators after constructing helper objects.
