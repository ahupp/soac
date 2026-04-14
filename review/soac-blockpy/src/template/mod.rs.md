# soac-blockpy/src/template/mod.rs

## File Responsibilities

Macro-backed Python syntax templating for compiler rewrites. It parses Python snippets containing
typed placeholders, instantiates them with Ruff AST fragments, identifiers, literals, temporary
names, or dict entries, and provides typed statement extraction helpers.

## Datatypes

- `py_stmt_internal!`, `py_stmts_internal!`, `py_expr!`, `py_stmt!`, `py_stmts!`,
  `py_stmt_typed!`: exported macros for parsing and instantiating templates.
- `StmtTryFrom`: trait for extracting a specific Ruff statement variant from a generated `Stmt`.
- `PlaceholderValue`: placeholder payload, either expression or statement list.
- `DictEntries<I>`: wrapper for converting key/expression pairs into a dict expression.
- `IntoPlaceholder`: trait converting Rust values into either AST placeholders or JSON scalar ids.
- `PlaceholderType`: supported placeholder kinds: expression, statement, identifier, literal,
  temporary name, and dict.
- `SyntaxTemplate`: parsed template statement list.
- `PlaceholderReplacer`: AST transformer state for replacing placeholder sentinels.

## Functions

- Template macros: collect placeholder values/ids, cache parsed templates in `LazyLock`, and return
  expression/statement/suite/typed-statement outputs.
- `StmtTryFrom::try_from_stmt` and generated impls: downcast generated statements to expected Ruff
  variants.
- `expect_stmt`: panics with template context if a generated statement has the wrong variant.
- `body_from_stmts`: converts a statement vector to `Suite`.
- `is_simple`: recognizes simple expression forms.
- `expand_body_stmt`: currently returns one statement, used as future expansion hook.
- `IntoPlaceholder` impls: support expressions, statements, suites, strings, integers, booleans,
  dict entries, and statement variants as template arguments.
- `var_for_placeholder`: converts placeholder name/type to the sentinel identifier used in parsed
  templates.
- `SyntaxTemplate::parse`: rewrites `{name:type}` markers to sentinels and parses the template.
- `SyntaxTemplate::instantiate_suite`: replaces sentinels, checks unused placeholders, flattens
  placeholder-if wrappers, and returns statements.
- `SyntaxTemplate::instantiate_one`: requires exactly one generated statement and reports expanded
  source on failure.
- `PlaceholderReplacer::new`, `parse_placeholder`, `take_value`, `get_id`, `get_tmpname`,
  `replace_identifier`, `replace_optional_identifier`, `replace_name`, `finish`: manage
  placeholder lookup, generated temp names, replacements, and diagnostics.
- `Transformer for PlaceholderReplacer`: replaces placeholders in expressions, statements,
  parameters, keywords, and aliases while walking the rest of the AST.
- `placeholder_regex`, `placeholder_template_regex`, `placeholder_text_regex`: compiled regexes for
  sentinel/template recognition.
- `dict_expr_from_entries`: builds a Ruff dict expression from string keys and expression values.
- `identifier_string`, `identifier_expr`, `literal_expr`, `parse_constant_expr`,
  `parse_dynamic_expr`: convert JSON ids/literals into Ruff AST expressions or identifiers.

## Context Read

- `soac-blockpy/src/transformer.rs`
- `soac-blockpy/src/namegen.rs`
- `soac-blockpy/src/passes/ast_to_ast/simplify/mod.rs`
