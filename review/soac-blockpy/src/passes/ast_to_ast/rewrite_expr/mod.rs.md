# soac-blockpy/src/passes/ast_to_ast/rewrite_expr/mod.rs

## File Responsibilities
Provides expression-level AST rewrites that require helper statements or helper functions, especially generators/comprehensions, named expressions, async detection, and scoped helper expression lowering.

## Datatypes
- `NamedExprTargetCollector`: transformer that collects assignment-expression target names.
- `NamedExprRewriter`: transformer that rewrites assignment expressions according to class/comprehension leakage rules.
- `ScopedHelperExprPass`: concrete `ExprRewritePass` used by the AST rewrite loop.

## Functions
- `lower_generator_expr`: lowers generator expressions to helper functions with correct async and scope behavior.
- `genexpr_requires_async`, `comp_is_async`, and `expr_requires_async`: determine whether expression helper code must be async.
- `wrap_ifs`: constructs nested `if` statements for comprehension filters.
- `collect_named_expr_targets`: finds walrus targets in comprehension expressions.
- `NamedExprTargetCollector::visit_expr`: records named-expression targets without crossing nested scopes.
- `NamedExprRewriter::new` / `visit_expr`: rewrites named expressions, including special handling for class-scope targets.
- `ScopedHelperExprPass::lower_expr`: entry point that lowers comprehensions, generator expressions, and named-expression patterns that need helper scope.

## Context Read
- `comprehension.rs` for detailed comprehension lowering.
- `ast_rewrite/mod.rs` for the `ExprRewritePass` contract.
- `context.rs` and `scope_helpers.rs` for scope-sensitive behavior.
