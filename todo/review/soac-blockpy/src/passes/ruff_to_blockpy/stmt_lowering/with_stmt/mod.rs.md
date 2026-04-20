# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/with_stmt/mod.rs

## File Responsibilities

Desugars sync and async `with` statements into explicit context-manager helper calls, try/except/
finally cleanup, optional target assignment lowering, and statement-sequence integration.

## Datatypes

- None.

## Functions

- `maybe_placeholder`: binds non-simple context expressions to a temp so enter/exit paths reuse the
  same object and can clear it after the block.
- `desugar_structured_with_stmt_for_blockpy`: rewrites nested with-items from right to left into
  explicit enter, body, exception-exit, normal-exit, and cleanup statements.
- `desugar_structured_with_instr_for_blockpy`: converts an `InstrRuff` with-statement through raw
  AST desugaring and back.
- `lower_with_stmt_sequence`: expands a with-statement in the current sequence, preserving any
  pending linear prefix and delegating the expanded body back to sequence lowering.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/assign_stmt/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_sequences.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`

