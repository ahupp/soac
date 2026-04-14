# soac-blockpy/src/passes/ast_to_ast/rewrite_future_annotations/mod.rs

## File Responsibilities
Strips `from __future__ import ...` statements, validates known future features, and implements future annotations by converting annotation expressions to source-like strings.

## Datatypes
- `FutureAnnotationsRewriter`: transformer with a Ruff code generator used to render annotations as strings.

## Functions
- `rewrite`: module entry point; strips future imports, applies annotation stringification when `annotations` is enabled, and returns enabled features.
- `FutureAnnotationsRewriter::new`: constructs the annotation renderer.
- `FutureAnnotationsRewriter::strip_future_imports`: removes future imports from the body and validates feature names.
- `FutureAnnotationsRewriter::annotation_string`: renders a Ruff annotation expression.
- `FutureAnnotationsRewriter::visit_annotation`: replaces annotation expressions with string literals.
- `is_known_future_feature`: validates supported CPython future-feature names.
- `is_future_import`: recognizes `from __future__ import ...`.

## Context Read
- `ruff_python_codegen` generator behavior.
- `soac-blockpy/src/passes/ast_to_ast/rewrite_stmt/annotation.rs` for later annotation handling.
