# soac-blockpy/src/block_py/meta.rs

## File Responsibilities

Defines source and semantic identity metadata for BlockPy instructions. It bridges Ruff node/range
metadata, assigned semantic instruction ids, and stable `(function, instruction)` keys used by
profiling and instrumentation.

## Datatypes

- `InstrId`: semantic instruction id within a block, stored as `(block_label, index_in_block)`.
- `InstrKey`: globally meaningful instruction key combining `FunctionId` and `InstrId`.
- `IdentifiedInstr<I>`: wrapper that pairs an operation with an assigned `InstrId`.
- `Meta`: optional node/range metadata plus optional semantic instruction id.
- `HasMeta`: trait for retrieving metadata.
- `WithMeta`: trait for assigning/replacing metadata.
- `HasSemanticInstrId`: extension trait for requiring or deriving semantic instruction ids.

## Functions

- `InstrId::new`, `block_label`, `instr_index_in_block`: construct and access instruction ids.
- `InstrKey::new`: constructs a function-scoped instruction key.
- `IdentifiedInstr::new`, `instr_id`, `op`, `into_op`: manage an identified operation wrapper.
- `IdentifiedInstr::meta`: returns wrapped operation metadata with the wrapper id applied.
- `IdentifiedInstr::with_meta`: updates wrapped metadata while preserving an existing id unless a
  new one is supplied.
- `Meta::new`: constructs metadata from a Ruff node index and text range.
- `Meta::synthetic`: creates empty metadata for synthesized IR.
- `WithMeta::with_source`: copies metadata from another item.
- `HasSemanticInstrId::try_semantic_instr_id`, `semantic_instr_id`, `semantic_instr_key`: access
  assigned instruction ids and build profiling keys.
- Blanket `HasMeta` impl for Ruff nodes: extracts node index and range.

## Context Read

- `soac-blockpy/src/block_py/name_gen.rs`
- `soac-blockpy/src/passes/instr_id.rs`
