# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/direct/mod.rs

## File Responsibilities

Contains direct statement rewrites for statement shapes that can be normalized before BlockPy
lowering. Currently only handles `raise ... from ...`.

## Datatypes

- None.

## Functions

- `rewrite_raise_stmt`: rewrites `raise exc from cause` to `raise __soac__.raise_from(exc, cause)`;
  leaves ordinary raise unchanged and rejects impossible cause-only raises.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`

