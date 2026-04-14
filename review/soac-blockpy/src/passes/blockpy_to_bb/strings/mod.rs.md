# soac-blockpy/src/passes/blockpy_to_bb/strings/mod.rs

## File Responsibilities
Normalizes string/bytes/runtime literal values in resolved BlockPy modules by moving them into the module constant table and rewriting expressions to module-constant loads for codegen.

## Datatypes
- `CodegenExprNormalizer`: mapper that owns the module constant vector while converting `InstrResolved` to `InstrCodegen`.

## Functions
- `normalize_bb_module_strings`: module entry point that maps all functions and populates `module_constants`.
- `CodegenExprNormalizer::push_module_constant`: appends a literal value and returns its constant slot.
- `CodegenExprNormalizer::map_instr`: rewrites literal/string-like instructions to module constant loads and recursively maps other instructions.
- `CodegenExprNormalizer::map_name`: preserves resolved names during mapping.

## Context Read
- `soac-blockpy/src/passes/mod.rs` for `InstrResolved` and `InstrCodegen` shapes.
- `soac-blockpy/src/block_py/literal.rs` for module constant payloads.
