# soac-blockpy/src/passes/ast_to_ast/rewrite_expr/comprehension.rs

## File Responsibilities
Lowers list/set/dict/generator comprehensions into explicit helper functions and loop bodies. It manages comprehension-local target renaming, named-expression leakage, async requirements, nested generator order, and construction of the final container or generator function.

## Datatypes
- `LoadRenamer`: transformer that rewrites loads according to a rename map.
- `TargetRenamer`: transformer that creates fresh storage names for comprehension iteration targets and rewrites their uses.
- `LoweredGenerator`: intermediate representation of one lowered comprehension generator clause and its setup/body fragments.

## Functions
- `LoadRenamer::visit_expr`: rewrites load expressions using a supplied name map.
- `TargetRenamer::new` / `ensure_binding`: configure target renaming and allocate fresh names.
- `TargetRenamer::visit_expr`: rewrites store/load occurrences for comprehension targets.
- `rename_loads`: applies load renaming to an expression.
- `collect_store_names`: extracts names bound by a comprehension target.
- `wrap_ifs`: wraps a statement body with nested `if` guards from comprehension filters.
- `lower_function`: builds the helper function body for a comprehension, including loops, awaits, appends/adds/sets/yields, and return value.
- `collect_named_expr_targets`: finds assignment-expression targets that must be treated specially for comprehension scoping.
- `comp_is_async` / `expr_requires_async`: determine whether a comprehension helper must be async.
- Private lowering helpers in this file build iterator setup, nested generator bodies, collection writes, and result expressions for each comprehension kind.

## Context Read
- `soac-blockpy/src/passes/ast_to_ast/rewrite_expr/mod.rs` for caller logic.
- `soac-blockpy/src/passes/ast_symbol_analysis/mod.rs` for scope-local name collection patterns.
- `soac-blockpy/src/passes/ast_to_ast/context.rs` for fresh names and scope state.
