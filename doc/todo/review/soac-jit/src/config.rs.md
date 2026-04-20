# soac-jit/src/config.rs

## File Responsibilities

Thin `soac-jit` facade over centralized environment parsing in `soac_config`, plus construction of Cranelift target ISA
settings for runtime JIT and offline object generation. This keeps call sites from reading environment variables directly
while preserving crate-local visibility choices. BlockPy module-cache path and metadata helpers are forwarded from
`soac_blockpy::codegen_cache`.

## Datatypes

- `CraneliftTargetConfig`: normalized Cranelift target options: optimization level, PIC mode, frame-pointer preservation, and
  machine-code CFG metadata emission.
- Re-exported `SpecializationMode`: profile/verify/apply specialization mode parsed by `soac_config`.
- Test-only re-export `SOAC_JIT_EMIT_REFCOUNTS_ENV`: environment variable name used by tests.

## Functions

- `CraneliftTargetConfig::runtime_from_env`: builds a non-PIC target config for normal process JIT code.
- `CraneliftTargetConfig::object_from_env`: builds a PIC target config for offline object/shared-library compilation.
- `CraneliftTargetConfig::from_env_with_pic`: shared constructor that reads the Cranelift optimization level and applies a PIC
  choice.
- `CraneliftTargetConfig::build_isa`: creates the native Cranelift `TargetIsa` using the stored flags.
- `CraneliftTargetConfig::set_bool_flag` / `set_flag`: typed wrappers around Cranelift settings mutation with string errors.
- `soac_work_dir_from_env`: returns configured SOAC work directory.
- `counter_dump_input_path_from_env`: returns profile counter input path for verify/apply modes.
- `counter_dump_output_path_from_env`: returns profile/verify counter output path.
- `profiled_cold_blocks_enabled`: returns whether profiled cold blocks should affect codegen.
- `jit_refcount_emission_enabled`: returns whether JIT refcount emission is enabled.
- `module_cache_root_from_env`: resolves the pre-optimization module cache root from `SOAC_WORK_DIR`.
- `pre_optimization_module_cache_identity`: builds the build-identity string used in module-cache keys.
- `pre_optimization_module_cache_path`: resolves a module-cache path for a source hash and identity.
- `precompiled_library_path_from_env`: returns an optional precompiled shared-library path.
- `eager_clif_compile_requested`: returns whether compile mode requests eager CLIF compilation.
- `jit_perf_helper_frames_enabled`: returns whether helper frame wrappers should be used for perf profiling.
- `specialization_mode_is_profile`: convenience predicate for profile mode.
- `behavior_change_indexed_stores_enabled`: convenience predicate for verify/apply indexed-store behavior changes.
- `specialization_mode_from_env`: returns the parsed specialization mode.

## Context Read

- `soac-config/src/lib.rs`
- `soac-blockpy/src/codegen_cache.rs`
- `soac-jit/src/jit/mod.rs`
- `soac-jit/src/module_type.rs`
