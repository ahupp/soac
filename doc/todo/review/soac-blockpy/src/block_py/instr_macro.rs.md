# soac-blockpy/src/block_py/instr_macro.rs

## File Responsibilities

Macro implementation for defining BlockPy operation payload structs. The macros generate storage,
constructors, debug rendering, metadata access, child visitation, and child mapping consistently for
both generic lowered operations and Ruff-shaped operations.

## Datatypes

- `define_instr!`: macro for operations that either have child instruction fields or are
  value-only payloads.
- `define_ruff_instr!`: thin wrapper macro for Ruff-shaped operation payloads.

## Functions

- `define_instr!` top-level arms: distinguish generic `struct Name<E>` operations from
  value-only `struct Name` operations.
- `@collect_fields` and `@collect_value_fields`: parse field lists and generate struct fields,
  constructor arguments, and constructor initialization.
- Generated `new`: constructs operations with default metadata.
- Generated `Debug`: tuple-style debug output over declared fields.
- Generated `HasMeta` / `WithMeta`: store and replace operation metadata.
- Generated `ChildVisitable`: visits only fields typed as `Box<E>`, `Vec<E>`, or
  `Option<Box<E>>`.
- Generated `Mappable`: maps only child instruction fields and preserves non-instruction payloads.
- `@visit_expr_fields`, `@visit_expr_fields_mut`: recursive macro arms for immutable/mutable child
  traversal.
- `@debug_tuple_fields`: recursive macro arms for debug rendering.
- `@build_mapped`, `@build_try_mapped`: recursive macro arms for infallible/fallible child mapping.
- `define_ruff_instr!`: forwards to `define_instr!` while preserving attrs/visibility.

## Context Read

- `soac-blockpy/src/block_py/instr.rs`
- `soac-blockpy/src/block_py/meta.rs`
- `soac-blockpy/src/block_py/map.rs`
- `soac-blockpy/src/block_py/visit.rs`
