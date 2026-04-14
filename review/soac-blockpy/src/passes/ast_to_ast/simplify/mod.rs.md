# soac-blockpy/src/passes/ast_to_ast/simplify/mod.rs

## File Responsibilities
Performs simple Ruff AST cleanup after rewrite passes: flattening nested placeholder statement lists and removing synthetic pass placeholders.

## Datatypes
- `Flattener`: transformer that rewrites bodies and statements into flatter statement sequences.

## Functions
- `Flattener::visit_body`: recursively flattens body statements.
- `remove_placeholder_pass`: removes synthetic placeholder pass statements from a body.
- `Flattener::visit_stmt`: flattens nested statement containers and descends into child bodies.
- `flatten`: public entry point for running the flattener over a suite.

## Context Read
- AST rewrite passes that produce placeholder or nested statement output.
- `transformer.rs` traversal utilities.
