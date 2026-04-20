# soac-blockpy/src/env_config.rs

## File Responsibilities

Centralizes SOAC environment-variable parsing for BlockPy/JIT-facing behavior: specialization
mode, work directory, Cranelift opt level, counters, module cache, precompiled library path, compile
mode, perf helper frames, tracing/logging, and exec tracing.

## Datatypes

- `SOAC_*_ENV` constants: canonical environment variable names.
- `DEFAULT_SOAC_JSON_LOG_FILTER`: default tracing filter when `SOAC_WORK_DIR` implies JSON logs.
- `SpecializationMode`: profile/verify/apply optimization mode, excluding explicit `none`.
- `CompileMode`: lazy or eager compilation request.
- `SoacLogConfig`: tracing filter plus optional JSONL destination path.
- `SoacEnvConfig`: parsed, validated process configuration snapshot.

## Functions

- `SpecializationMode::from_str`, `records_counters`,
  `behavior_change_indexed_stores_enabled`, `output_counter_dump_filename`,
  `input_counter_dump_filename`: parse and classify specialization modes.
- `CompileMode::from_str`: parse compile mode.
- `SoacEnvConfig::from_env`: reads and validates all supported env vars once into a config value.
- `SoacEnvConfig` accessors: expose Cranelift opt level, specialization mode, work dir, counter
  paths, profiling flags, refcount emission, module cache root, compile mode, perf-frame flag, exec
  trace, log config, and specialization-runtime logging state.
- `invalid_env_value`: standardizes parse error messages.
- `env_string`, `env_path`, `parse_bool_env_value`, `env_bool`: low-level env parsing helpers.
- `parse_cranelift_opt_level`, `parse_optional_cranelift_opt_level`,
  `parse_optional_specialization_mode`, `parse_optional_compile_mode`: typed parsers.
- Compatibility accessor functions ending in `_from_env`: construct `SoacEnvConfig` and return one
  specific field for older call sites.
- `precompiled_library_path_from_env`: parses `SOAC_PRECOMPILED_LIBRARY`.
- `pre_optimization_module_cache_identity`, `pre_optimization_module_cache_path`: build module
  cache identity/path inputs.
- `parse_soac_log_config`: parses `SOAC_LOG` filter segments and `json=...`, defaulting to
  `$SOAC_WORK_DIR/events.jsonl` when appropriate.

## Context Read

- `soac-blockpy/src/codegen_cache.rs`
- `README.md`
- `AGENTS.md`
