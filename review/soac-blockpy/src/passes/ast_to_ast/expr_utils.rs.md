# soac-blockpy/src/passes/ast_to_ast/expr_utils.rs

## File Responsibilities
Provides tiny Ruff AST expression constructors shared by rewrite passes.

## Datatypes
- None.

## Functions
- `make_tuple`: builds a Ruff tuple expression from existing element expressions, using default metadata.

## Context Read
- `ruff_python_ast::ExprTuple` construction APIs.
