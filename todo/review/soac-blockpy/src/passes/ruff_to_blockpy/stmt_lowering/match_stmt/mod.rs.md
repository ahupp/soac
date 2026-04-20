# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/match_stmt/mod.rs

## File Responsibilities

Desugars Python `match` statements into ordinary assignments, tests, nested if/else chains, and
runtime helper calls before BlockPy lowering. The rewrite preserves subject single-evaluation and
handles value, singleton, OR, sequence, mapping, star, class, and as-patterns.

## Datatypes

- `PatternTest`: intermediate pattern result, either a test expression plus assignments to run on
  match, or a wildcard plus assignments.

## Functions

- `body_to_vec`: consumes a Ruff suite as a vector.
- `fold_exprs`: folds a non-empty expression list into a boolop expression.
- `integer_expr`: parses a small integer literal expression.
- `trace_expr`: emits tracing output for generated expressions when trace logging is enabled.
- `test_for_pattern`: recursively converts one match pattern and subject expression into a
  `PatternTest`, including generated tests and binding assignments.
- `assigned_names`: finds names assigned by generated assignment statements, used for guard-failure
  cleanup.
- `rewrite_match_stmt`: rewrites a full match statement into a subject-temp assignment plus a
  reversed if/else chain for cases, guards, bindings, cleanup, and fallthrough.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`
- `soac-blockpy/src/passes/ast_to_ast/expr_utils.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/boolop_compare/mod.rs`

