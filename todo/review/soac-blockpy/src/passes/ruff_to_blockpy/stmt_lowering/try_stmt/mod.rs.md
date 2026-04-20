# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/try_stmt/mod.rs

## File Responsibilities

Rewrites complex exception-handler forms and delegates structured try/except/else/finally lowering
to the try-region lowering module. It handles `except*` desugaring, typed except chains, handler
name cleanup, and final integration into statement-sequence lowering.

## Datatypes

- None.

## Functions

- `body_to_vec`: consumes a Ruff suite as a vector.
- `quiet_delete_marker`: creates helper cleanup for deleting handler-bound exception names.
- `wrap_handler_body_with_cleanup`: wraps a handler body in a `try/finally` cleanup for the bound
  exception name.
- `has_non_default_handler`: detects typed except handlers.
- `has_default_handler`: detects bare/default except handlers.
- `rewrite_try_stmt`: rewrites `except*` and typed except chains into simpler try/except forms with
  explicit helper calls and cleanup.
- `rewrite_try_instr`: converts an `InstrRuff` try statement to raw AST, applies `rewrite_try_stmt`,
  and converts the result back.
- `lower_star_try_stmt_sequence`: lowers `except*` by expanding its rewritten statements into the
  current statement sequence.
- `lower_try_stmt_sequence`: builds a `TryPlan`, lowers try/else/except/finally regions, and emits
  the final entry/jump blocks for a structured try.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/try_regions.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_sequences.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`

