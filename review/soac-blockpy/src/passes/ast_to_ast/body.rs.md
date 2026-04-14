# soac-blockpy/src/passes/ast_to_ast/body.rs

## File Responsibilities
Defines the AST body alias used by AST-to-AST passes and provides small body utilities.

## Datatypes
- `Suite`: alias for Ruff `ast::Suite`, the vector of statements that forms a Python body.

## Functions
- `empty_suite`: returns an empty statement body.
- `split_docstring`: separates a leading string-literal docstring from the remaining body when present.

## Context Read
- `ruff_python_ast` statement and string literal variants.
