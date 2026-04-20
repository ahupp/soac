# soac-blockpy/src/transformer.rs

## File Responsibilities

Mutable Ruff AST visitor/walker in Python evaluation order. Rewriter passes implement
`Transformer` and override targeted hooks while the default walkers recursively visit all child
nodes that can contain evaluated expressions, annotations, parameters, patterns, and strings.

## Datatypes

- `Transformer`: visitor trait with overridable hooks for statements, expressions, annotations,
  decorators, operators, comprehensions, exception handlers, arguments, parameters, patterns,
  string fragments, and literal nodes.

## Functions

- `Transformer` default methods: delegate each hook to the corresponding `walk_*` function.
- `walk_body`, `walk_elif_else_clause`: visit statement suites and if/elif/else clauses.
- `walk_stmt`: visits each Ruff statement variant in evaluation order, including defs/classes,
  returns, deletes, assignments, loops, with, match, raise, try, assert, imports, and expression
  statements.
- `walk_annotation`, `walk_decorator`: visit annotation/decorator expressions.
- `walk_expr`: visits each Ruff expression variant in evaluation order, including calls,
  comprehensions, f/t-strings, literals, attributes, subscripts, starred/name/list/tuple/slice
  forms, and async/yield expressions.
- `walk_comprehension`: visits comprehension iterator, target, and filters.
- `walk_except_handler`: visits exception type and body.
- `walk_arguments`: visits call positional args before keywords, matching Python evaluation order.
- `walk_parameters`: visits defaults before annotations, then parameters in declaration order.
- `walk_parameter`, `walk_keyword`, `walk_with_item`: visit child expressions inside parameters,
  keywords, and with-items.
- `walk_type_params`, `walk_type_param`: visit type parameter bounds/defaults.
- `walk_match_case`, `walk_pattern`, `walk_pattern_arguments`, `walk_pattern_keyword`: visit match
  patterns and guards/bodies.
- `walk_f_string`, `walk_interpolated_string_element`, `walk_t_string`: visit interpolated string
  expressions and nested format specs.
- Leaf walkers `walk_expr_context`, `walk_bool_op`, `walk_operator`, `walk_unary_op`,
  `walk_cmp_op`, `walk_alias`, `walk_string_literal`, `walk_bytes_literal`: no-op hooks for leaf
  metadata or literals.

## Context Read

- `soac-blockpy/src/template/mod.rs`
- `soac-blockpy/src/passes/ast_to_ast/body.rs`
