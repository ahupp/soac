# crates/soac_opt/src/operator_specialization.rs

## File Responsibilities

Defines compact encodings for observed exact-type binary operator shapes. These values are recorded in counters and later
decoded by v3 planning/codegen to select supported exact-int binary fast paths.

## Datatypes

- `ExactTypeTag`: compact tag for exact operand types currently supported by operator specialization; only `Int` exists today.
- `BINARY_LHS_TAG_SHIFT`, `BINARY_RHS_TAG_SHIFT`: bit positions for binary shape packing.

## Functions

- `ExactTypeTag::packed`: converts a tag to its `u64` counter encoding.
- `ExactTypeTag::from_packed`: decodes a packed tag, rejecting unsupported values.
- `pack_binary_shape`: packs left/right binary operand type tags into one `u64`.
- `unpack_binary_shape`: decodes a binary shape.

## Context Read

- `crates/soac_jit/src/jit/mod.rs`
- `soac-blockpy/src/block_py.rs`
