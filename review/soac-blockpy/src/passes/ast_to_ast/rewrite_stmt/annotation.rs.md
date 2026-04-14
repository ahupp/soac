# soac-blockpy/src/passes/ast_to_ast/rewrite_stmt/annotation.rs

## File Responsibilities
Rewrites annotations into explicit runtime-visible metadata operations. It strips annotations from assignments/functions where needed, populates `__annotations__`, and handles type-parameter capture data for deferred annotation helpers.

## Datatypes
- `AnnotationStripper`: transformer that removes annotation syntax while collecting annotation entries and generated helper statements.

## Functions
- `rewrite_ann_assign_to_dunder_annotate`: entry point that rewrites annotation statements in a suite.
- `AnnotationStripper::strip` / `strip_with_captures`: run annotation stripping and return collected annotation entries.
- `AnnotationStripper::annotation_string`: render an annotation expression for metadata storage.
- `AnnotationStripper::parameter_annotation_entry`: create an annotation entry for one parameter.
- `AnnotationStripper::function_signature_entries`: collect parameter and return annotation entries for a function.
- `AnnotationStripper::function_annotation_helper`: build helper statements for function annotations when captures are needed.
- `AnnotationStripper::visit_body`: rewrite a body and insert generated annotation metadata statements.
- `AnnotationStripper::visit_stmt`: handles annotated assignments, function definitions, class definitions, and nested bodies.
- `type_param_names`: extracts type-parameter names from Ruff type-param syntax.
- `capture_name_values` / `type_param_capture_values`: build capture pairs for annotation helper functions.
- Private helpers in this file construct `__annotations__` writes, helper call expressions, and capture payloads.

## Context Read
- `rewrite_future_annotations/mod.rs` for future-annotation stringification.
- `soac_py/src/soac/runtime.py` annotation/runtime helper conventions.
- Ruff AST function, parameter, and type-param nodes.
