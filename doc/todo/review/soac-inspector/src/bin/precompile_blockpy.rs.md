# crates/soac_inspector/src/bin/precompile_blockpy.rs

## File Responsibilities

Offline precompile CLI. It reads specialization counters, resolves pre-optimization `BlockPyModule` cache entries for referenced modules, compiles those modules to object files, and links them into a shared library.

## Datatypes

- `Args`: CLI options for counter dump, module cache dir, object output dir, shared-library output, linker command, and extra linker args.
- `CounterModuleRef`: deduplicated module reference from counters, including source hash, module id, name, package, and optional source path.
- `CompiledModuleObject`: object-file path plus label produced for a compiled module.
- `SOAC_BUILD_IDENTITY`: embedded build identity used to resolve compatible module-cache files.

## Functions

- `main`: exits with a readable error from `run`.
- `run`: invokes `run_with_args` on process arguments.
- `run_with_args`: parses options, reads counters, resolves module caches, compiles each referenced module, and links a shared object.
- `counter_modules_from_records`: extracts and deduplicates modules referenced by counter records.
- `module_id_for_record`: validates and extracts the module id attached to rows in one counter record.
- `row_function_ids`: yields function ids present in a counter row.
- `resolve_module_cache_path`: picks a matching serialized module cache file for a counter module.
- `module_cache_path_for_identity`: constructs the exact cache path for a known source hash/build identity/runtime-name mode.
- `matching_cache_paths_for_source_hash`: finds candidate cache files when exact identity matching is ambiguous or absent.
- `link_shared_library`: invokes the linker to create the final shared library.
- `default_module_cache_dir`: returns the cache path derived from `SOAC_WORK_DIR/modules`.
- `repo_root`: finds the repository root from the Cargo manifest directory.
- `default_object_dir`: derives the object-output directory from the shared-library output path.
- `object_file_name`: creates a stable object filename for a module reference.
- `sanitize_path_component`: converts arbitrary module/path text into a filesystem-safe component.
- `parse_args`: parses CLI flags.
- `next_path`, `next_string`, `next_os_string`: typed parsers for flag values.
- `print_usage`: writes CLI usage.

## Context Read

- `soac_jit` offline compile/module cache APIs
- `crates/soac_pyo3/src/jit_runtime.rs`
- `crates/build_support/src/lib.rs`
