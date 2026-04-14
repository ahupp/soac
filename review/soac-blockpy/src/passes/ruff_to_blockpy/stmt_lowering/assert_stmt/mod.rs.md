# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/assert_stmt/mod.rs

## File Responsibilities

Desugars Python `assert` statements into ordinary conditional raise statements before direct
BlockPy lowering.

## Datatypes

- None.

## Functions

- `rewrite_assert_stmt`: rewrites `assert test` and `assert test, msg` to `if __debug__:` guarded
  raises of `__soac__.AssertionError`.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`

