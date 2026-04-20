# soac-blockpy/src/passes/trace/mod.rs

## File Responsibilities
Injects profiling and verification counters into codegen-stage BlockPy modules. Counters cover block entries, refcount actions, global loads, call targets, indexed field/global hits/fallbacks, top values, and branch locality.

## Datatypes
- `PreparedTraceNameLocator`: helper that maps known runtime/global names to resolved name loads for injected trace calls.

## Functions
- `specialization_runtime_logging_enabled`: checks config for trace/runtime logging behavior.
- `specialization_mode_instruments_top_values`: decides whether top-value counters should be emitted.
- `instrument_bb_module_with_block_entry_counters`: inserts block-entry counters.
- `instrument_bb_module_with_refcount_counters`: inserts refcount counters for verify mode.
- `instrument_bb_module_with_global_load_counters`: instruments global load observations.
- `instrument_bb_module_with_call_target_counters`: instruments call/direct-call/operator/global/field target observations and hit/fallback counters.
- `instrument_bb_module_with_locality_counters`: instruments branch direction/locality observations before terminators.
- Local predicate helpers classify operator specialization, global-index, and field-index candidates.
- `define_indexed_hit_fallback_counters`: defines paired hit/fallback counters for indexed access sites.
- `PreparedTraceNameLocator::new`: prepares name lookup context for injected trace expressions.
- `PreparedTraceNameLocator::load_name`: constructs resolved-name loads for helpers/globals.
- `helper_call_expr`, `string_literal_expr`, `tuple_expr`, and `param_pairs_expr`: build injected counter/helper call expressions.

## Context Read
- `instrument.rs` for generic counter definitions.
- `soac-blockpy/src/block_py/counters.rs` for counter schemas.
- `soac-blockpy/src/env_config.rs` for specialization mode configuration.
