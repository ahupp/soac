# soac-blockpy/src/passes/ast_to_ast/string_templates.rs

## File Responsibilities
Lowers f-strings, t-strings, and string literals with active surrogate escapes into explicit runtime expressions that preserve Python semantics through later lowering.

## Datatypes
- `StringTemplateLowerer`: transformer that rewrites string-template expressions in place.
- `SurrogateStringLiteralLowerer`: transformer that rewrites string literals requiring source-based evaluation.

## Functions
- `join_parts`: joins rewritten string/template expression parts, optionally forcing a `''.join(...)` call.
- `strip_debug_comment`: removes debug-expression comment text from f-string conversion text.
- `rewrite_interpolation`: rewrites one f-string interpolation into formatted-value expression parts.
- `rewrite_elements`: rewrites all f-string elements and reports whether a join is needed.
- `rewrite_tstring_interpolation`: rewrites one t-string interpolation into runtime interpolation object construction.
- `rewrite_fstring`: lowers a Ruff f-string expression.
- `rewrite_tstring`: lowers a Ruff template-string expression.
- `StringTemplateLowerer::visit_expr`: replaces f-string/t-string nodes with lowered expressions.
- `lower_string_templates_in_expr`: runs string-template lowering for a single expression.
- Source/escape helpers `source_slice`, `hex_value`, `parse_hex_escape`, `has_active_surrogate_escape`, and `string_literal_needs_source_eval` detect literals that must be evaluated from source text.
- `SurrogateStringLiteralLowerer::visit_expr`: rewrites affected string literals to `__soac__.eval_string_literal(...)`.
- `rewrite_surrogate_escape_string_literals`: entry point for source-sensitive string literal rewrites.
- `lower_string_templates_in_instr_ruff`: applies template lowering to an `InstrRuff` expression.
- `rewrite_string_literal` and `rewrite_fstring_literal`: construct literal expressions for rewritten string segments.

## Context Read
- `soac_py/src/soac/runtime.py` for `eval_string_literal`, template, and interpolation runtime helpers.
- `blockpy_expr_simplify/mod.rs` for later expression conversion.
