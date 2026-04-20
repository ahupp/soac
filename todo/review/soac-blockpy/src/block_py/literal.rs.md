# soac-blockpy/src/block_py/literal.rs

## File Responsibilities

Owns BlockPy literal payload types independent of operation kind, plus helpers for wrapping those
payloads into literal instructions with metadata.

## Datatypes

- `Literal`: union of string, bytes, and number literal payloads.
- `LiteralValue`: IR operation wrapping a `Literal`.
- `StringLiteral`: string literal payload.
- `BytesLiteral`: bytes literal payload.
- `NumberLiteral`: numeric literal payload.
- `NumberLiteralValue`: integer or floating-point numeric payload.
- `IntLiteral`: decimal-preserving integer literal representation, avoiding early truncation.

## Functions

- `LiteralValue::as_literal`: borrows the contained literal payload.
- `LiteralValue::into_literal`: consumes the operation and returns the payload.
- `literal_value`: builds a `LiteralValue` with explicit metadata.
- `literal_expr`: converts a literal payload into any instruction enum that accepts `LiteralValue`.
- `IntLiteral::from_decimal`: constructs an integer literal from decimal text.
- `IntLiteral::from_i64`: constructs from a machine integer.
- `IntLiteral::as_decimal`: returns the stored decimal text.
- `IntLiteral::as_i64`: parses the literal as `i64` when possible.
- Formatting impls: render literals in debug/display forms used by BlockPy output and diagnostics.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/block_py/instr_macro.rs`
- `soac-blockpy/src/block_py/meta.rs`
