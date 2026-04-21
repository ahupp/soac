# crates/soac_jit/src/jit/mod.rs

## File Responsibilities

This is the main SOAC JIT backend. It owns Cranelift module setup, import/local symbol declaration, process-global JIT state,
runtime support loading/inlining, module constants and counters, direct-call and constructor specialization, typed-value and
demand-aware lowering, deopt metadata, offline object emission, CLIF rendering, JIT dump recording, and compiled-handle
lifetime management.

## Datatypes

- Process/global state: `RUNTIME_SUPPORT_LIBRARY`, `PRECOMPILED_LIBRARY`, `NEXT_IMPORT_SPEC_ID`, `JIT_DATA_SYMBOLS`,
  `TYPE_KEY_RUNTIME_REGISTRY`, `JIT_ARENA_BYTES`, `MISSING_PYTHON_EXCEPTION_TRAP`, and
  `COLD_BLOCK_ENTRY_RATE_DENOMINATOR` hold singleton runtime libraries, symbol registries, arena sizing, trap codes, and
  branch-layout thresholds.
- Symbol/import descriptors: `SigType`, `StaticSignature`, `ImportSpec`, and many static `*_IMPORT` values describe helper
  signatures and whether helpers are imported from CPython/Rust or local runtime CLIF.
- Import state: `ModuleFuncImports` and `FuncBuildImports` map import specs and function ids to Cranelift references during
  module/function construction.
- Rendering/build artifacts: `RenderedSpecializedClif`, `ClifBlockDisplayAnnotation`, `BuiltSpecializedFunction`,
  `DeclaredJitFunction`, `DefinedJitFunction`, `DefinedFunctionArtifact`, `CompiledFunctionBytes`,
  `CompiledFunctionArtifact`, `CompiledSpecializedRunner`, `CompiledFunctionHandle`, `DirectFunctionCompileResult`,
  `JitCodegenStats`, `CompiledRunnerEntry`, and `BuildSpecializedFunctionOptions` carry generated CLIF, machine code,
  direct/vectorcall entrypoints, deopt table pointers, and compile metrics.
- Process JIT structures: `ProcessJitEngine`, `ProcessJitState`, `ProcessJitBatchFunction`,
  `ProcessJitBatchFunctionSource`, `ProcessJitFunctionEntry`, `ProcessJitFunctionShape`, `ProcessJitCompileGuard`,
  `ModuleConstantObjectBinding`, `ScalarCounterStorageBinding`, and `TopValueCounterStorageBinding` manage the
  process-global JITModule and shared bindings.
- Runtime/precompiled structures: `PrecompiledLibrary`, `PrecompiledModuleRuntime`, `RuntimeSupportLibrary`,
  `ParsedRuntimeClifFunction`, `ObjectFunctionDefinition`, `ObjectDataDefinition`, `PrecompileObjectSummary`,
  `PrecompiledObjectBytes`, and ELF builder datatypes model shared-library/object-file loading and offline object emission.
- Deopt structures: `RuntimeJitDeoptTable`, `RuntimeJitDeoptRecord`, `RuntimeJitDeoptInvocation`, `RuntimeJitDeoptLocal`,
  `RuntimeJitDeoptLocals`, `RuntimeJitDeoptCursor`, `RuntimeJitDeoptContinuation`, `RuntimeJitDeoptSupportCtx`, and
  `JitDeoptExitRef` describe deopt points, live values, resume cursors, and runtime materialization.
- Codegen structures: `FunctionRuntimeDataLayout`, `JitEmitConsts`, `ModuleConstantAccess`, `ResultDemandPlan`,
  `JitEmitCtx`, `JitGuardMissTarget`, `JitGuardMissDispatch`, `DirectMethodSpecialization`,
  `DirectConstructorSpecialization`, `DirectFunctionSpecialization`, `DirectCallArgPlan`, `DirectCallArgSource`,
  `DirectCallIncompatibility`, `DirectCallEntryKind`, `DirectEdgeStats`, `FieldIndexSpecialization`,
  `CpythonTypeSymbol`, `RelocTypeRef`, `RelocCallableRef`, `LocalEnvCodegenIntrinsicEmitState`, `LocalEnvEntry`,
  `LocalEnvStorage`, `LocalEnv`, `LocalFailureCleanupValue`, `LocalFailureCleanupValueKey`, `StackSlots`,
  `ExceptionStateSlots`, `PendingLocalFailureCleanup`, `PendingLocalFailureContinuation`, and `LocalFailureCleanupKey`
  hold codegen context, local storage, cleanup, specialization, and relocation state.
- Cranelift/object utilities: `TrivialJumpBlock`, `TrivialJumpPredecessor`, `TrivialJumpNormalizationStats`,
  `RuntimeSupportInliner`, `ElfObjectBuilder`, `ElfSectionIndex`, `ElfSymbolKind`, `ElfSymbolBinding`, `ElfSymbol`,
  `ElfRelocation`, and `ElfSectionHeaderInput` support postopt cleanup, runtime inlining, and ELF object writing.
- Constants: `JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT`, `SOAC_RUNTIME_*_SYMBOL`, ELF constants, and runtime support inline limits
  define symbol names and binary-format values.

## Functions

- Symbol/name helpers: `py_dealloc_symbol`, `jit_data_symbols`, `type_key_runtime_registry`, `register_jit_data_symbol`,
  `lookup_registered_jit_data_symbol`, `cpython_type_symbol_name`, `push_symbol_component_hex`,
  `reloc_type_ref_symbol_name`, `reloc_callable_ref_symbol_name`, module/counter/direct symbol-scope helpers, and
  `jit_python_perf_symbol_name` construct stable names and maintain symbol registries.
- Module constants/counters: `declare_module_constant_object_data*`, `define_scalar_counter_storage_data*`,
  `declare_scalar_counter_storage_import`, `define_top_value_counter_storage_data*`,
  `declare_top_value_counter_storage_import`, `declare_type_ptr_import`, `placeholder_module_constant_ptrs`,
  `scalar_counter_slot_for_id`, `scalar_counter_byte_offset`, `scalar_counter_addr`, `emit_increment_counter`,
  `top_value_counter_byte_offset`, `lookup_counter_id`, `lookup_runtime_counter_id`,
  `build_counted_runtime_refcount_helper`, and `build_counted_runtime_refcount_helpers` declare data objects and emit
  counter/refcount instrumentation.
- Precompiled library/runtime: `runtime_support_library`, `precompiled_library`, `load_precompiled_library_from_env`,
  `promote_current_soac_extension_symbols_for_precompiled_library`, `take_dlerror`,
  `lookup_precompiled_direct_function_handle`, `precompiled_module_runtime`, `build_precompiled_module_runtime`, and
  `patch_precompiled_module_constant_slots` load and bind optional precompiled code.
- Process JIT: `new_jit_builder`, `register_jit_builder_symbols`, `new_jit_module`,
  `process_jit_is_currently_compiling`, `ProcessJitCompileGuard::{new,drop}`, `collect_process_jit_batch_functions`,
  `resolve_process_jit_batch_function`, and `ProcessJitEngine` methods declare, batch, compile, finalize, and publish
  functions in the process-global JIT.
- Deopt metadata/runtime: `RuntimeJitDeoptCursor`/`RuntimeJitDeoptContinuation`/`RuntimeJitDeoptRecord`/
  `RuntimeJitDeoptTable`/`RuntimeJitDeoptInvocation`/`RuntimeJitDeoptLocal` methods, `runtime_jit_deopt_continuation_for_point`,
  `runtime_jit_deopt_*_supported`, `RuntimeJitDeoptSupportCtx`, `emit_deopt_resume_call`, `emit_deopt_live_value_buffer`,
  `emit_deopt_live_value_for_binding`, and `deopt_binding_stack_slot_for_location` validate and emit deopt support.
- Runtime layout/direct metadata: `new_compiled_direct_runner_handle`, `compiled_direct_code_ptr`,
  `compiled_default_direct_code_ptr`, `compiled_direct_deopt_table_ptr`, `compiled_direct_deopt_table`,
  `FunctionRuntimeDataLayout` methods, `max_referenced_function_closure_slot`, `load_function_env_obj`,
  `load_py_function_soac_metadata_obj`, `emit_direct_function_env_load`, and current-exception helpers emit/load function
  runtime data.
- Local environment and cleanup: `LocalEnv` methods, `bind_planned_local_env_at_block_entry`, local ref-kind helpers,
  `planned_*_store_effect*`, `emit_local_store*`, `emit_local_delete*`, `StackSlots` methods, `ExceptionStateSlots` methods,
  forwarded-value helpers, pending cleanup helpers, `emit_release_owned_inputs`, nullable-call cleanup helpers, local release
  helpers, exception dispatch helpers, and handled-exception pop helpers maintain CPython-compatible local/refcount behavior.
- Constants/boxing/truthiness: `emit_owned_module_constant*`, singleton constant helpers, `emit_owned_bool_from_cond`,
  `emit_i32_bool01_*`, `emit_to_python_bool`, `emit_checked_owned_pyobject_result`, `emit_to_python_long`,
  `emit_release_*`, `emit_owned_bool_from_*`, `emit_truthy_from_*`, and `emit_branch_index_i64` produce typed and boxed
  values with Python error checks.
- Calls/direct calls: `direct_method_specializations_for_call_site`, `direct_constructor_specializations_for_call_site`,
  `collect_call_direct_targets`, call-target specialization loaders, direct-call compatibility/planning helpers,
  direct-call emitters, vectorcall/tuple/kwargs/unpack emitters, runtime primitive ABI helpers, and scalar result emitters
  implement direct Python calls, constructor fast paths, and builtin/runtime primitive calls.
- Expression/statement/term codegen: `emit_codegen_expr*_with_local_env`, `emit_typed_codegen_expr*_with_local_env`,
  `emit_codegen_simple_call*`, `emit_codegen_call*`, `emit_codegen_stmt*`, `emit_typed_codegen_stmt*`,
  `emit_typed_codegen_ops`, `emit_codegen_if_*`, `emit_codegen_return_pyobject`, `emit_codegen_branch_table_from_i64`,
  `emit_load_raise_from_function`, `emit_codegen_raise_exception_from_function`, `emit_codegen_term`, and
  `emit_typed_codegen_term` lower BlockPy/typed BlockPy into Cranelift.
- Specialization profile/type relocation: `SpecializationProfile`, `load_call_target_specializations`,
  `resolve_type_key_to_type`, type-key registry helpers, reloc type/callable resolution helpers,
  `field_index_specialization_for_type`, `load_field_index_specializations*`, `prime_field_index_layout`,
  `collect_cold_block_labels_from_path`, and runtime counter collection helpers apply profile-guided specialization.
- Cranelift backend/object support: `define_prepared_function`, `compile_prepared_function_bytes*`,
  `prepare_cranelift_function_for_backend`, trivial-jump normalization helpers, `record_jit_bb_map`,
  runtime support CLIF parse/remap/load/inline helpers, import declaration helpers, direct function symbol/signature helpers,
  default direct adapter builder, object precompile helpers, ELF writer helpers, and import alias/block-annotation rewriters
  bridge generated CLIF to machine code, object files, inspection text, and perf maps.
- Public entrypoints: `run_cranelift_smoke`, `precompile_codegen_module_to_object_file`,
  `render_cranelift_run_bb_specialized_with_cfg`,
  `render_cranelift_run_bb_specialized_with_runtime_state_and_cfg`,
  `compile_cranelift_run_bb_specialized_cached`, and `free_cranelift_run_bb_specialized_cached` are the main external JIT
  APIs exposed to tests/tools/runtime.

## Context Read

- `crates/soac_jit/src/jit/planning.rs`: supplies local/refcount/deopt plans consumed by codegen.
- `crates/soac_jit/src/jit/intrinsics.rs`: emits intrinsic operations through `OperationEmitState`.
- `crates/soac_jit/src/jit/specialized_helpers.rs`: registers and implements imported helper symbols.
- `crates/soac_jit/src/jit/runtime_context.rs`: supplies ABI offsets and runtime context structs.
- `crates/soac_jit/src/jit/direct_abi.rs` and `crates/soac_jit/src/jit/typed_value.rs`: supply direct-call descriptors and typed-demand
  value/result models.
- `crate::module_constants`, `crate::module_type`, `crate::session`, `crate::config`, and `crate::operator_specialization`:
  provide constants, shared module state, process session/config, and specialization metadata.
- `soac_blockpy::block_py` and `soac_blockpy::passes`: primary IR, facts, plans, and typed instruction inputs.
