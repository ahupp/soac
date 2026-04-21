# crates/soac_macros/src/lib.rs

## File Responsibilities

Procedural macro crate for SOAC IR enums. It generates delegation and mapping boilerplate for enum-shaped instructions and provides a `match_default!` macro for concise enum-pattern defaulting.

## Datatypes

- `EnumBroadcastTarget`: selected generated trait implementation family for `enum_broadcast`.
- `MatchDefaultArm`: parsed arm for the `match_default!` macro, distinguishing explicit variant arms from the default arm.
- `MatchDefaultInput`: parsed input to `match_default!`, including target expression and arms.

## Functions and Macros

- `enum_variants`: validates a derive input is an enum and returns variants.
- `item_enum_variants`: validates an item input is an enum and returns variants.
- `EnumBroadcastTarget::parse`: parses the requested generated implementation target from an attribute path.
- `EnumBroadcastTarget::impl_tokens`: emits implementation code for `HasMeta`, `WithMeta`, `ChildVisitable`, `Mappable`, and `Debug` over enum variants.
- `enum_broadcast`: attribute macro that appends generated impls to an enum item.
- `derive_delegate_match_default`: derive macro that generates variant-list and delegated `match_default` helper macros.
- Generated `*_variants` macro: emits code over all enum variants for downstream macros.
- Generated delegate macro: expands matches that share a default arm across variants.
- `variant_ident_from_pat`: extracts a variant identifier from a pattern when possible.
- `enum_ident_from_type`: extracts an enum identifier from an impl self type.
- `Parse for MatchDefaultArm::parse`: parses one explicit or default match arm.
- `Parse for MatchDefaultInput::parse`: parses the whole `match_default!` invocation.
- `expand_match_default`: expands a defaulting match expression after validating arms.
- `match_default`: public proc macro entrypoint for default-arm match expansion.

## Context Read

- SOAC IR enum usage in `soac-blockpy`
- Generated trait names referenced by `soac_macros`

