# soac-blockpy/src/codegen_cache.rs

## File Responsibilities

Filesystem serialization cache for pre-optimization/codegen BlockPy modules. It builds keyed cache
paths, writes/loads rkyv archives with a versioned header, rehydrates skipped name-generator state,
and remaps cached function ids to the current module id before reuse.

## Datatypes

- `CODEGEN_MODULE_CACHE_MAGIC`, `CODEGEN_MODULE_CACHE_FORMAT_VERSION`: cache file header and format
  version.
- `PythonModuleCacheSource`: cache namespace for project modules versus Python stdlib modules.
- `FunctionIdRemapper`: visitor state that rewrites function ids to a new module id.

## Functions

- `PythonModuleCacheSource::subtree`: returns the cache subtree name.
- `codegen_module_cache_path`: builds a safe cache file path under the selected source subtree.
- `codegen_module_cache_key`: combines source hash and build identity hash.
- `store_codegen_module_cache`: serializes a module to a temp file and renames it into place.
- `load_codegen_module_cache`: reads, validates, deserializes, and rehydrates a cached module.
- `rehydrate_codegen_module_generators`: recovers module/function name generators after rkyv load.
- `remap_codegen_module_function_ids`: rewrites functions, direct calls, make-function constants,
  and counter sites to a fresh module id.
- `FunctionIdRemapper::remap`: maps non-global ids into the new module id.
- `FunctionIdRemapper::visit_instr_mut`: rewrites ids inside `CallDirect` and `MakeFunction`.
- `collect_make_function_constant_slots`: finds module constant slots that encode function ids for
  runtime `make_function` calls.
- `codegen_expr_is_runtime_symbol`, `resolved_expr_is_runtime_symbol`,
  `codegen_expr_constant_index`, `resolved_function_id_constant`, `function_id_constant_expr`:
  identify and update function-id constants referenced through module constant slots.
- `archive_bytes_from_cache_file`, `aligned_archive_bytes`: validate header/version and prepare
  aligned bytes for rkyv.
- `cache_file_stem`: validates cache keys as safe file stems.
- `stable_hash`: FNV-1a hash used for build identity.
- `recovered_module_name_gen`, `recovered_function_name_gen`: reconstruct allocator state from
  loaded module contents.
- `non_empty_parent`, `temp_cache_path`: filesystem helpers for cache writes.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/block_py/visit.rs`
- `soac-blockpy/src/passes/mod.rs`
