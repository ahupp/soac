# crates/soac_jit/src/operator_specialization.rs

## File Responsibilities

Defines compact encodings for observed exact-type operator shapes and exact-int operator helper selectors. These values are
recorded in counters and later decoded by specialized codegen to choose exact-int unary/binary fast paths.

## Datatypes

- `ExactTypeTag`: compact tag for exact operand types currently supported by operator specialization; only `Int` exists today.
- `UNARY_TAG_SHIFT`, `BINARY_LHS_TAG_SHIFT`, `BINARY_RHS_TAG_SHIFT`: bit positions for shape packing.
- `ExactIntBinaryOpKind`: stable integer ABI tags for exact-int binary and comparison operations, including inplace variants.
- `ExactIntUnaryOpKind`: stable integer ABI tags for exact-int unary/truth operations.

## Functions

- `ExactTypeTag::packed`: converts a tag to its `u64` counter encoding.
- `ExactTypeTag::from_packed`: decodes a packed tag, rejecting unsupported values.
- `pack_unary_shape`: packs a unary operand type tag.
- `pack_binary_shape`: packs left/right binary operand type tags into one `u64`.
- `unpack_unary_shape`: decodes a unary shape.
- `unpack_binary_shape`: decodes a binary shape.
- `ExactIntBinaryOpKind::from_binop_kind`: maps BlockPy binary op kinds to exact-int runtime helper ABI tags, returning `None`
  for unsupported matrix multiply, contains, and identity operations.
- `ExactIntUnaryOpKind::from_unary_op_kind`: maps BlockPy unary op kinds to exact-int runtime helper ABI tags.

## Context Read

- `crates/soac_jit/src/jit/mod.rs`
- `crates/soac_jit/src/jit/specialized_helpers.rs`
- `soac-blockpy/src/block_py.rs`

