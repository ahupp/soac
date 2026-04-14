# soac-jit/src/module_type.rs

## File Responsibilities

Defines SOAC's transformed Python module type and per-module shared runtime state. It owns lowered module data, codegen
constants, original code objects, counter storage, precompiled runtime state, indexed module dictionaries, counter dump
emission, JIT-codegen logging, and type/key layout profiling hooks.

## Datatypes

- `SoacExtModuleDataRef<'a>`: borrowed access wrapper exposing a module's `SharedModuleState`.
- `ModuleInfo`: Python-visible transformed-module metadata: source hash and indexed module keys.
- `SharedModuleState`: long-lived per-module state retained by `CompileSession`; holds lowered IR, names, source hash,
  constant table/objects, function lookup tables, original code objects, runtime counter storage, and precompiled runtime.
- `CounterStorageKey`: deduplication key for scalar or top-value counter storage.
- `CounterRuntimeSlot`: runtime storage location for a counter id, either scalar `u64` or top-value counter index.
- `SoacExtModuleState`: CPython module state payload containing an initialized `Arc<SharedModuleState>`.
- `ProfileTypeRegistry`: process registry assigning stable numeric ids to profiled owner types.
- `SOAC_EXT_MODULE_SLOTS`, `SOAC_EXT_MODULE_DEF`: CPython module definition for transformed modules.
- `MODULE_DICT_METADATA_NAMES`: standard module metadata names forced into indexed module dictionaries.
- `SOAC_INDEXED_MODULE_TYPE`: cached heap subtype of `module` with an extra `ModuleInfo*` slot.
- `SoacExtModule`: Rust API for creating transformed module objects and retrieving their state.

## Functions

- `hash_module_source`: computes FNV-1a source hash used in module metadata and counter dumps.
- `SharedModuleState` lookup/accessors: `storage_instance_key`, `module_id`, `source_hash`, `lookup_function`,
  `lookup_direct_call_target_function`, `lookup_original_code`, `module_constant_ptrs`, `module_constant_obj`,
  `counter_slots_by_id`, `scalar_counter_values_ptr`, `top_value_counter_values_ptr`, `counter_values`, `counter_value`.
- `SharedModuleState::top_values_counter_snapshot`: snapshots a top-value counter for a logical counter id.
- `SharedModuleState::lookup_or_compile_direct_function_handle`: resolves cross-module direct-call targets, finds
  precompiled handles, or compiles direct function bodies through the process JIT.
- `SharedModuleState::append_jit_codegen_log`: method wrapper around structured JIT-codegen tracing.
- `SharedModuleState::append_specialization_runtime_log`: emits selected indexed global/field counters to tracing in apply
  mode.
- `SharedModuleState::counter_dump_record`: builds a complete module counter dump record, including top-value rows and key
  layouts.
- `SharedModuleState::counter_dump_module_keys`, `counter_dump_type_key_layouts`: collect module and type key-layout dump data.
- `SharedModuleState::append_counter_dump_file`: appends an encoded record to the configured dump path.
- Counter helpers: `allocate_shared_module_state_storage_key`, `record_top_value_sample_counter_ptr`, `counter_scope_name`,
  `counter_storage_key`, `counter_uses_call_target_storage`, `build_counter_storage_layout`, `build_counter_storage`.
- Shared-state builders: `build_shared_state_for_inspection`, `build_shared_state_for_testing`, `build_function_index_by_id`.
- `SoacExtModuleState::init`: constructs and retains `SharedModuleState` for a transformed module.
- `SoacExtModuleState::clear`: emits runtime logs/counter dumps and drops the retained state.
- `SoacExtModuleState::data`, `clone_shared_state`: return borrowed or cloned access to initialized state.
- `key_layout_counter_enabled`, `specialization_mode_records_counters`: decide whether profile/verify modes should collect key
  layout data.
- `append_jit_codegen_log`: emits structured tracing fields for direct/vectorcall JIT codegen.
- `watch_split_keys_for_type`: asks vendored CPython to watch split-key layout events for a heap type.
- `ProfileTypeRegistry::id_for_type`, `entries_for_ids`: assign and query profile type ids.
- `profile_type_registry`, `snapshot_type_key_layout_events`, `snapshot_type_key_layout_events_bound`: read CPython key-layout
  events and normalize them for counter dumps.
- `counter_dump_file_from_env`: resolves and prepares the counter dump output path.
- CPython module callbacks: `soac_ext_module_clear`, `soac_ext_module_traverse`, `soac_ext_module_free`,
  `soac_ext_module_create`.
- Indexed-module helpers: `soac_indexed_module_dict`, `soac_module_dict_slot`, `soac_indexed_module_info_offset`,
  `soac_indexed_module_info_slot`, `indexed_module_info`, `soac_replace_module_dict`, `soac_new_indexed_module_object`,
  `soac_init_indexed_module_object`, `soac_indexed_module_new`, `soac_indexed_module_dealloc`.
- Module definition/type helpers: `soac_ext_module_def`, `soac_ext_module_state`, `create_soac_indexed_module_type`,
  `soac_indexed_module_type`, `indexed_module_type_for_python`, `ensure_module_dict_metadata_names`,
  `tuple_from_global_names`.
- `SoacExtModule::new`: creates a transformed module, initializes indexed globals and `SharedModuleState`, and returns the
  Python module object.
- `SoacExtModule::with_data`: runs a closure with borrowed module data.
- `SoacExtModule::clone_shared_state`: clones the module's retained shared state.

## Context Read

- `soac-jit/src/session.rs`
- `soac-jit/src/counter.rs`
- `soac-jit/src/counter_dump.rs`
- `soac-jit/src/module_constants.rs`
- `soac-jit/src/jit/mod.rs`

