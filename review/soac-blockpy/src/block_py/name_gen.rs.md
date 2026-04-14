# soac-blockpy/src/block_py/name_gen.rs

## File Responsibilities

Generates stable module/function/block/temp identities for BlockPy. Function ids pack module and
function-local ids; cloned functions share allocator state to avoid duplicate generated names.

## Datatypes

- `FunctionId`: packed `u64` function identity with module id in the high bits and function id in
  the low bits; `GLOBAL` is a sentinel.
- `BlockLabel`: block label wrapper with `u32::MAX` reserved for synthetic fallthrough.
- `FunctionNameGen`: shared state for one function's block labels and temp names.
- `FunctionNameGenState`: atomic counters and owning function id.
- `ModuleNameGen`: shared module id plus next-function counter.

## Functions

- `FunctionId::new`, `from_packed`, `global`, `packed`, `module_id`, `function_id`: construct and
  inspect packed ids.
- `BlockLabel::from_index`, `fallthrough`, `is_fallthrough`, `as_u32`, `index`: construct and
  inspect labels.
- `FunctionNameGen::new`, `recovered`, `share`, `function_id`, `next_block_name`,
  `next_tmp_name`: allocate or recover function-local names.
- `ModuleNameGen::new`, `recovered`, `module_id`, `next_function_name_gen`: allocate or recover
  module-scoped function ids.
- `Clone`/`Default` impls: preserve shared state and provide global/default generators.
- Debug/display impls: render ids and labels for diagnostics and snapshots.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/codegen_cache.rs`
