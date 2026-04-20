# soac-blockpy/src/block_py/map.rs

## File Responsibilities

Provides generic ownership-consuming mapping traits for BlockPy instruction trees, terms, blocks,
functions, and modules. Passes use these traits to change instruction shape or rewrite instruction
contents without hand-copying structural fields.

## Datatypes

- `Mappable`: trait implemented by operation payloads that know how to map their child
  instructions.
- `MapInstr`: infallible mapper for instructions and names.
- `TryMapInstr`: fallible mapper for instructions and names.
- `MapTerm`, `MapBlock`, `MapFunction`, `MapModule`: structural mapping traits over progressively
  larger BlockPy containers.
- `TryMapTerm`, `TryMapBlock`, `TryMapFunction`, `TryMapModule`: fallible variants.

## Functions

- `Mappable::map_children`, `try_map_children`: operation-specific child mapping hooks.
- `Mappable::map_same_children`, `try_map_same_children`: convenience helpers for same-type
  rewrites.
- Closure impls for `MapInstr` and `TryMapInstr`: let `FnMut(I) -> I` and
  `FnMut(I) -> Result<I, E>` act as same-type instruction mappers while preserving names.
- `MapTerm::map_term` / `TryMapTerm::try_map_term`: map expressions inside return, raise, if, and
  branch-table terminators while preserving edges.
- `MapBlock::map_block` / `TryMapBlock::try_map_block`: map statements and terminator in a block.
- `MapFunction::map_fn` / `TryMapFunction::try_map_fn`: map all blocks in a function while
  preserving function metadata.
- `MapModule::map_module` / `TryMapModule::try_map_module`: map all functions in a module while
  preserving globals, constants, counters, and module name generator.
- `map_function_blocks`: helper for rewriting only a function's block list.
- `map_module_functions`: helper for rewriting only a module's function list.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/block_py/operation.rs`
