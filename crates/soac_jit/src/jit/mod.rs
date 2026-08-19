use crate::PY_FUNCTION_JIT_EXTRA_REGISTERED_CODE_OFFSET;
use crate::function_instantiation::make_function_kind_abi_tag;
use crate::module_constants::{ModuleCodegenConstants, ModuleConstantId};
use crate::module_type::{CounterRuntimeSlot, SharedModuleState, build_counter_storage_layout};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_jit::JITModule;
use cranelift_module::{DataId, FuncId};
use pyo3::{Python, ffi};
use soac_config::SoacEnvConfig;
use soac_core::block_py as blockpy_intrinsics;
use soac_core::block_py::{
    AbruptKind, Block, BlockArg, BlockEdge, BlockLabel, BlockParamRole, BlockPyFunction,
    BlockPyModule, BlockTerm, CallArgKeyword, CallArgPositional, CellLocation, ChildVisitable,
    CounterDef, CounterId, Del, FunctionExecutionMode, FunctionKind, HasSemanticInstrId, InstrId,
    InstrKey, InstrLocationMap, LocalLocation, ModuleShape, NameLocation, ParamKind,
    PreservedLocation, PreservedSlotStorage, ResolvedName, RuntimeFunctionId, RuntimeName,
    StorageLayout, Store, Visit, current_instr_locations,
};
use soac_instrument::RUNTIME_DECREF_LOCATION_COUNTER_KIND;
use soac_ir_blockpy::{
    BlockPyModuleShape, InstrBlockPy, constructor_init_function_id_for_entry_function,
    is_constructor_entry_function,
};
use soac_ir_typed::emit_v3::{
    MechanicalCodegenConversion, MechanicalCodegenOperation, MechanicalCodegenStep,
    MechanicalExitKind, MechanicalIndexedFieldReceiverSource, MechanicalRegionEmission,
    MechanicalRegionInputSource, MechanicalStepOp,
    mechanical_codegen_step as opt_v3_mechanical_codegen_step,
    mechanical_region_inputs as opt_v3_mechanical_region_inputs,
};
use soac_ir_typed::plan_v3::{
    ConversionKind, IndexedFieldAccessKind as PlanV3IndexedFieldAccessKind, IndexedFieldOwnerType,
    IndexedGlobalAccessKind as PlanV3IndexedGlobalAccessKind, MaterializeKind, PlanNodeId,
    PlanNodeKind, PlanValue, RegionId, RegionPlan, Rep, RichCompareOp,
};
use soac_ir_typed::{
    FactStore, InstrTyped, PyExactType, PyObjFacts, RuntimeHelperId, TypedAttrAccessPlan,
    TypedBlock, TypedBlockLayoutHint, TypedBlockPyModuleShape, TypedCall, TypedCallAccessPlan,
    TypedConstructorInitPlanSource, TypedDirectCallGuardTest, TypedDirectCallGuardTestKind,
    TypedDirectCallableCall, TypedDirectCallableCallGuard, TypedDirectMethodCall,
    TypedExactIntBranchPlan, TypedExactIntReturnPlan, TypedGetAttr, TypedGuardedCallableCall,
    TypedGuardedMethodCall, TypedIndexedFieldCounterSource, TypedIndexedFieldGuard,
    TypedIndexedFieldPlanSource, TypedPlannedResult, TypedPyObjectOwnershipPlan, TypedSetAttr,
    ValueFacts, lower_blockpy_function_to_typed,
};
#[cfg(test)]
use soac_opt::passes::infer_module_value_facts;
use soac_opt::passes::{
    FunctionRefcountPlan, LocalEnvResumeBinding, LocalEnvResumePoint, LocalEnvResumeValueSource,
    LocalRefState, REFCOUNT_STACK_SLOT_CLEAR_PREVIOUS, REFCOUNT_STACK_SLOT_EXIT_SWEEP,
    REFCOUNT_STACK_SLOT_REPLACE_CLONED_PREVIOUS, REFCOUNT_STACK_SLOT_REPLACE_MOVED_PREVIOUS,
    REFCOUNT_STACK_SLOT_REPLACE_TRANSFERRED_PREVIOUS, RefcountActionKind, RefcountLocal,
    RefcountReleaseReason, RefcountSite, annotate_typed_function_planned_results,
    annotate_typed_function_result_demands, annotate_typed_function_value_facts,
    lower_typed_function_call_access_plan_instrs, refcount_release_location_branch_name,
    refcount_stack_slot_location_branch_name, refresh_typed_function_value_facts,
    try_lower_typed_instr_to_codegen_legacy, validate_typed_function_call_access_plans,
    validate_typed_function_value_facts,
};
use soac_opt::region_emission_v3::{
    ExactIntBranchSelection as OptV3ExactIntBranchSelection,
    ExactIntReturnSelection as OptV3ExactIntReturnSelection,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::mem::offset_of;
use std::sync::Arc;

type BlockPyBlock = Block<InstrBlockPy>;

unsafe extern "C" {
    static mut PyFunction_Type: ffi::PyTypeObject;
    static mut PyMethod_Type: ffi::PyTypeObject;
    static mut PyType_Type: ffi::PyTypeObject;
    static mut PyLong_Type: ffi::PyTypeObject;
    static mut PyFloat_Type: ffi::PyTypeObject;
    static mut PyList_Type: ffi::PyTypeObject;
    static mut PyTuple_Type: ffi::PyTypeObject;
    static mut PyUnicode_Type: ffi::PyTypeObject;
    static mut _PyDict_IndexedValueTombstone: i8;
    fn PyThreadState_GetUnchecked() -> *mut ffi::PyThreadState;
}

mod backend;
mod codegen_env;
mod compiled;
mod counters;
mod deopt;
mod deopt_interpreter;
#[allow(unused_imports)]
pub(crate) use deopt_interpreter::{
    BlockPyEntryRuntimeContext, run_blockpy_function_from_entry,
    run_blockpy_function_from_vectorcall_entry,
};
mod direct_abi;
mod direct_function;
mod function_targets;
mod imports;
mod inspection;
mod intrinsics;
mod jitdump;
mod module_data;
mod operation_specializations;
mod planning;
mod precompile;
mod precompiled_library;
mod precompiled_object;
mod process;
mod refcount_lowering;
mod runtime_context;
mod runtime_support;
mod signal_diagnostics;
mod specialization_profile;
mod specialized_helpers;
mod symbols;
mod typed_pipeline;
mod typed_value;
mod vectorcall;

#[cfg(test)]
use backend::define_prepared_function;
#[cfg(test)]
use backend::new_jit_module;
#[cfg(test)]
use backend::normalize_postopt_clif_for_inspection;
use backend::{
    CompiledFunctionBytes, new_jit_builder, new_jit_module_with_runtime_support_symbols,
};
#[cfg(test)]
use backend::{stable_cranelift_function_hash, stable_cranelift_function_name};
#[cfg(test)]
use codegen_env::declare_local_fn;
use codegen_env::{FuncBuildImports, JitCodegenEnv};
pub(crate) use compiled::{
    CompiledFunctionHandle, DirectFunctionCompileResult, JitCodegenStats, VectorcallEntryFn,
};
pub(crate) use counters::CounterRef;
#[cfg(test)]
use counters::build_counted_runtime_refcount_helper;
use counters::{
    CountedRefcountHelpers, build_counted_runtime_refcount_helpers,
    collect_deopt_entry_counter_ids_by_kind, collect_runtime_branch_counter_refs_by_kind,
    collect_runtime_counter_ids_by_kind, collect_runtime_counter_refs_by_kind_branch,
    collect_runtime_counter_refs_by_kind_branch_source, emit_increment_counter_slot,
    emit_record_top_value_counter_slot, scalar_counter_slot_for_id, scalar_counter_slot_for_ref,
    top_value_counter_slot_for_id,
};
pub(crate) use deopt::RuntimeFunctionEntryPlan;
#[cfg(test)]
use deopt::{RuntimeJitDeoptContinuation, RuntimeJitDeoptRecord, RuntimeJitDeoptTable};
use deopt::{
    RuntimeJitDeoptCursor, RuntimeJitDeoptInvocation, RuntimeJitDeoptLocals,
    RuntimeJitDeoptUnsupportedReason, runtime_jit_deopt_guard_operand_replay_safe,
    runtime_jit_typed_deopt_continuation_for_point,
    runtime_jit_typed_deopt_guard_operand_replay_safe,
    typed_nested_guard_misses_can_resume_before_instr,
};
use direct_abi::{
    ArgOwnership, DirectCallableDesc, DirectEntry, DirectTargetId, ErrorAbi, HiddenArgAbi,
    ParamAbi, PyLongI64Coercion, ResultAbi,
};
use direct_function::{
    DirectCallArgPlan, DirectCallArgSource, DirectCallEntryKind, DirectEdgeStats,
    DirectFunctionSpecialization, DirectMethodSpecialization, declare_direct_function,
    declare_imported_direct_function, direct_function_specializations_from_typed_guards,
    direct_method_specialization_from_typed_call, direct_method_specializations_from_typed_guards,
    make_direct_function_signature, record_profiled_direct_call_incompatibility,
    validate_direct_call_compatibility,
};
#[cfg(test)]
use direct_function::{DirectCallIncompatibility, plan_direct_call_args_for_target};
use function_targets::collect_typed_call_direct_targets;
#[cfg(all(
    test,
    target_os = "linux",
    target_arch = "x86_64",
    target_endian = "little"
))]
use module_data::module_constant_symbol_prefix_for_module_identity;
use module_data::{
    ModuleConstantAccess, ModuleConstantAccessTable, declare_module_constant_object_data,
    declare_top_value_counter_storage_import, declare_type_ptr_import,
    define_scalar_counter_storage_data, define_top_value_counter_storage_data,
    top_value_counter_storage_symbol_for_shared_state,
};
#[cfg(test)]
use module_data::{
    declare_scalar_counter_storage_import, define_scalar_counter_storage_data_for_symbol,
    direct_function_symbol_scope_for_shared_state, module_constant_object_symbol,
    module_constant_symbol_prefix_for_instance, module_constant_symbol_prefix_for_shared_state,
    persistent_function_id_for_module_function,
    precompiled_direct_function_symbol_scope_for_persistent, push_shared_module_symbol_identity,
    scalar_counter_storage_symbol_for_instance, top_value_counter_storage_symbol_for_instance,
};
use operation_specializations::IndexedFieldLoweringPlan;
#[cfg(test)]
use planning::plan_typed_v3_jit_module_for_test;
pub use planning::{
    BlockExcDispatchPlan, BlockParamFacts, CleanupRootSlotState, EdgeTransportPlan,
    FunctionLocalPlan, LocalRefKind, ParamBindingFacts, ParamProvenance,
    PlannedCleanupRootSlotStates, PlannedJitDeoptPoint, PlannedJitDeoptPointId,
    PlannedJitDeoptResumeFunction, PlannedJitDeoptResumeModule, PlannedJitFunctionLocals,
    PlannedJitModuleLocals, PlannedLocalEnvEntryMaterialization, PlannedLocalEnvEntrySource,
    PlannedLocalStorage, PlannedStackSlotEntrySeed, PreparedJitTypedModulePlan,
    RuntimeBlockArgPlan, RuntimeBlockParamPlan, RuntimeBlockParamRepr,
    local_ref_kind_for_stack_mirror, plan_jit_typed_module,
    planned_implicit_target_transports_for_typed_function, planned_jit_params_for_typed_function,
    planned_jump_edge_transports_for_typed_function,
    planned_local_env_entry_materializations_for_function,
    planned_stack_slot_entry_seeds_for_typed_function, render_jit_deopt_resume_function,
    render_jit_deopt_resume_module, render_jit_function_locals, render_jit_module_locals,
    typed_exc_dispatch_plan,
};
#[cfg(all(
    test,
    target_os = "linux",
    target_arch = "x86_64",
    target_endian = "little"
))]
use precompile::precompile_codegen_module_to_object_bytes;
pub use precompile::{
    PrecompileModuleIndex, PrecompileModuleIndexEntry, PrecompileObjectSummary,
    precompile_codegen_module_to_object_file,
};
pub(crate) use precompiled_library::{
    PrecompiledModuleRuntime, lookup_precompiled_direct_function_handle,
    lookup_precompiled_static_module_constant,
};
pub(crate) use process::{ProcessJitEngine, process_jit_is_currently_compiling};
use refcount_lowering::RefcountLowering;
pub(crate) use runtime_context::{
    FIRST_VALID_CPYTHON_FUNCTION_VERSION, FunctionRuntimeDataLayout,
    invalidate_py_function_soac_function_id, raw_py_code_version,
};
use runtime_context::{
    FUNCTION_ENV_BUILTINS_OBJ_OFFSET, FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET,
    FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET, FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET,
    FUNCTION_ENV_GLOBALS_OBJ_OFFSET, FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET, PY_FUNCTION_CODE_OFFSET,
    PY_FUNCTION_DEFAULTS_OFFSET, PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
    PY_FUNCTION_KWDEFAULTS_OFFSET, PY_FUNCTION_SOAC_FUNCTION_ID_OFFSET,
    PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET, load_function_env_obj,
    load_py_function_soac_metadata_obj,
};
pub use runtime_context::{ModuleJitContext, ModuleRuntimeContext};
#[cfg(test)]
use runtime_support::inline_runtime_support_calls;
#[cfg(test)]
use runtime_support::{ParsedRuntimeClifFunction, parse_runtime_clif_functions};
pub(crate) use specialization_profile::PlannedOptimizationInputs;
use specialization_profile::{
    SpecializationProfile, load_planned_optimization_inputs_for_runtime_state,
};
pub use specialized_helpers::ObjPtr;
use symbols::ensure_reloc_type_symbol_registered;
use symbols::{
    CpythonTypeSymbol, RelocCallableRef, RelocTypeRef, lookup_registered_jit_data_symbol,
    reloc_callable_ref_symbol_name, reloc_type_ref_from_typed_attr_owner_ref,
    reloc_type_ref_symbol_name,
};
#[cfg(test)]
use symbols::{
    SOAC_RUNTIME_DECREF_SYMBOL, SOAC_RUNTIME_INCREF_SYMBOL,
    SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL, SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_SYMBOL,
    SOAC_RUNTIME_PYLONG_AS_I64_SYMBOL, SOAC_RUNTIME_STORE_GLOBAL_INDEXED_STOLEN_SYMBOL,
    SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL, push_direct_function_module_identity,
    register_runtime_type_for_key,
};
pub(crate) use typed_pipeline::JitModulePlan;
#[cfg(test)]
use typed_pipeline::{
    apply_profile_typed_block_metadata_to_typed_function,
    apply_profile_typed_guard_miss_policy_to_typed_function,
    apply_profile_typed_plans_to_typed_function,
};
use typed_pipeline::{
    collect_codegen_constants_for_module_name, optimize_blockpy, optimize_blockpy_for_shared_state,
};
pub use typed_value::{
    EmitResult, IntFacts, IntRange, IntWidth, ResultDemand, SoacRepr, SoacValue, ValueOwnership,
};

pub fn install_sigill_diagnostics() -> Result<(), String> {
    signal_diagnostics::install_sigill_diagnostics()
}

const MISSING_PYTHON_EXCEPTION_TRAP: ir::TrapCode = ir::TrapCode::unwrap_user(1);
const DEOPT_SUPPRESSED_FALLBACK_TRAP: ir::TrapCode = ir::TrapCode::unwrap_user(2);
thread_local! {
    static PROCESS_JIT_COMPILE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
use imports::predeclare_typed_direct_call_imports;
use imports::{
    DP_JIT_DECREF_DEALLOC_PRESERVING_ERROR_IMPORT, DP_JIT_DECREF_IMPORT,
    DP_JIT_DEL_PRESERVED_IMPORT, DP_JIT_DEL_PRESERVED_QUIETLY_IMPORT, DP_JIT_DEOPT_RESUME_IMPORT,
    DP_JIT_DIRECT_COMPILE_FUNCTION_ENV_IMPORT, DP_JIT_ENTER_RECURSIVE_CALL_IMPORT,
    DP_JIT_FINISH_CONSTRUCTOR_INIT_IMPORT, DP_JIT_INCREF_IMPORT, DP_JIT_IS_TRUE_IMPORT,
    DP_JIT_LOAD_CELL_IMPORT, DP_JIT_LOAD_RUNTIME_OBJ_BY_ID_IMPORT, DP_JIT_MAKE_CELL_IMPORT,
    DP_JIT_MAKE_GENERATOR_INSTANCE_FROM_VECTORCALL_IMPORT, DP_JIT_POP_HANDLED_EXCEPTION_IMPORT,
    DP_JIT_PRESERVED_VALUES_PTR_IMPORT, DP_JIT_PROTOCOL_ITER_FUNCTION_ID_IMPORT,
    DP_JIT_PROTOCOL_NEXT_FUNCTION_ID_IMPORT, DP_JIT_PUSH_HANDLED_EXCEPTION_IMPORT,
    DP_JIT_PY_CALL_OBJECT_IMPORT, DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT,
    DP_JIT_PY_CALL_WITH_KW_IMPORT, DP_JIT_PY_VECTORCALL_IMPORT, DP_JIT_PYOBJECT_GETATTR_IMPORT,
    DP_JIT_PYOBJECT_GETITEM_IMPORT, DP_JIT_PYOBJECT_SETATTR_IMPORT, DP_JIT_PYOBJECT_SETITEM_IMPORT,
    DP_JIT_PYOBJECT_TO_I64_IMPORT, DP_JIT_PYTYPE_GENERIC_ALLOC_IMPORT,
    DP_JIT_RAISE_FROM_EXC_IMPORT, DP_JIT_RAISE_I64_OVERFLOW_IMPORT,
    DP_JIT_RAISE_SUPER_ARG_DELETED_IMPORT, DP_JIT_RAISE_UNBOUND_LOCAL_ERROR_IMPORT,
    DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT, DP_JIT_STORE_CELL_IMPORT, ImportSpec, ModuleFuncImports,
    PY_HANDLE_PENDING_IMPORT, PYLONG_FROM_LONGLONG_IMPORT, PYNUMBER_ADD_IMPORT,
    PYNUMBER_AND_IMPORT, PYNUMBER_MULTIPLY_IMPORT, PYNUMBER_OR_IMPORT, PYNUMBER_SUBTRACT_IMPORT,
    PYNUMBER_XOR_IMPORT, PYOBJECT_RICHCOMPARE_IMPORT, PYUNICODE_COMPARE_IMPORT,
    SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_IMPORT, SOAC_JIT_RESUME_GENERATOR_IMPORT,
    SOAC_RUNTIME_BUILTIN_CHR_I64_IMPORT, SOAC_RUNTIME_BUILTIN_ITER_OBJECT_IMPORT,
    SOAC_RUNTIME_BUILTIN_LEN_I64_IMPORT, SOAC_RUNTIME_BUILTIN_ORD_I64_IMPORT,
    SOAC_RUNTIME_COMPARE_COMPACT_ASCII_UNICODE_IMPORT, SOAC_RUNTIME_LOAD_GLOBAL_IMPORT,
    SOAC_RUNTIME_LOAD_GLOBAL_SLOW_IMPORT, SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_IMPORT,
    SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_IMPORT, SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT,
    SOAC_RUNTIME_STORE_GLOBAL_IMPORT, SOAC_RUNTIME_STORE_GLOBAL_INDEXED_IMPORT,
    SOAC_RUNTIME_STORE_GLOBAL_INDEXED_STOLEN_IMPORT, SOAC_RUNTIME_TUPLE_NEW_IMPORT,
    SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_IMPORT, SOAC_RUNTIME_UNPACK_FIXED_IMPORT, SigType,
    predeclare_specialization_type_imports,
};
#[cfg(test)]
use imports::{SOAC_RUNTIME_DECREF_APPLIED_IMPORT, SOAC_RUNTIME_INCREF_APPLIED_IMPORT};
use inspection::{
    ClifBlockDisplayAnnotations, ClifBlockRole, ClifBlockRoles, ClifFunctionDisplayAlias,
    ClifFunctionDisplayAliases, RefcountFamily, refcount_family_source_loc_bits,
    register_block_display_annotation, register_block_role, render_compiled_clif_and_vcode_disasm,
    render_instr_typed_metadata_index, render_instr_typed_program,
    render_pre_inline_clif_for_inspection,
};
pub use inspection::{RenderedSpecializedClif, run_cranelift_smoke};
#[cfg(test)]
use inspection::{
    annotate_clif_instruction_purposes, nest_clif_blocks_by_nearest_dominator,
    rewrite_clif_function_aliases,
};

struct BuiltSpecializedFunction {
    ctx: cranelift_codegen::Context,
    main_id: cranelift_module::FuncId,
    main_symbol: String,
    default_adapter_id: Option<cranelift_module::FuncId>,
    default_adapter_symbol: Option<String>,
    import_id_to_symbol: HashMap<u32, &'static str>,
    local_func_id_to_symbol: HashMap<u32, &'static str>,
    direct_func_id_to_qualname: HashMap<u32, String>,
    #[cfg(test)]
    func_id_to_symbol: HashMap<u32, &'static str>,
    block_annotations: ClifBlockDisplayAnnotations,
    block_roles: ClifBlockRoles,
}

#[derive(Clone)]
struct DeclaredJitFunction {
    func_id: FuncId,
    default_func_id: Option<FuncId>,
    symbol: String,
    default_symbol: Option<String>,
}

fn add_declared_direct_function_alias(
    aliases: &mut HashMap<u32, String>,
    declared: &DeclaredJitFunction,
    qualname: &str,
) {
    aliases.insert(declared.func_id.as_u32(), qualname.to_string());
    if let Some(default_func_id) = declared.default_func_id {
        aliases.insert(default_func_id.as_u32(), format!("{qualname}:defaults"));
    }
}

fn clif_function_display_aliases(
    import_id_to_symbol: &HashMap<u32, &'static str>,
    local_func_id_to_symbol: &HashMap<u32, &'static str>,
    runtime_support_symbols: &HashMap<u32, String>,
    direct_func_id_to_qualname: &HashMap<u32, String>,
) -> ClifFunctionDisplayAliases {
    let mut aliases = ClifFunctionDisplayAliases::new();
    for (func_id, symbol) in import_id_to_symbol {
        aliases.insert(
            *func_id,
            ClifFunctionDisplayAlias::runtime_helper((*symbol).to_string()),
        );
    }
    for (func_id, symbol) in local_func_id_to_symbol {
        aliases.insert(
            *func_id,
            ClifFunctionDisplayAlias::runtime_helper((*symbol).to_string()),
        );
    }
    for (func_id, symbol) in runtime_support_symbols {
        aliases.insert(
            *func_id,
            ClifFunctionDisplayAlias::runtime_helper(symbol.clone()),
        );
    }
    for (func_id, qualname) in direct_func_id_to_qualname {
        aliases.insert(
            *func_id,
            ClifFunctionDisplayAlias::direct_python(qualname.clone()),
        );
    }
    aliases
}

fn codegen_expr_is_borrowable_from_local_env(
    expr: &InstrBlockPy,
    local_env: &LocalEnv,
    stack_slots: &StackSlots,
    storage_layout: Option<&StorageLayout>,
) -> bool {
    match expr {
        InstrBlockPy::Load(op) => {
            let Some(location) = op.name.local_location() else {
                return false;
            };
            if let Some(index) = local_env.entry_index_for_location(location) {
                return local_env.entries[index].is_pyobject_binding();
            }
            storage_layout
                .and_then(|layout| layout.stack_slots().get(location.slot() as usize))
                .is_some_and(|name| match local_env.entry_index_for_name(name) {
                    Some(index) => local_env.entries[index].is_pyobject_binding(),
                    None => stack_slots.has_name(name),
                })
        }
        _ => false,
    }
}

fn codegen_expr_pyobject_input_is_borrowed_from_local_env(
    expr: &InstrBlockPy,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> bool {
    codegen_expr_is_borrowable_from_local_env(
        expr,
        local_env,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    )
}

fn typed_expr_is_borrowable_from_local_env(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    stack_slots: &StackSlots,
    storage_layout: Option<&StorageLayout>,
) -> bool {
    match expr {
        InstrTyped::Load(op) => {
            let Some(location) = op.name.local_location() else {
                return false;
            };
            if let Some(index) = local_env.entry_index_for_location(location) {
                return local_env.entries[index].is_pyobject_binding();
            }
            storage_layout
                .and_then(|layout| layout.stack_slots().get(location.slot() as usize))
                .is_some_and(|name| match local_env.entry_index_for_name(name) {
                    Some(index) => local_env.entries[index].is_pyobject_binding(),
                    None => stack_slots.has_name(name),
                })
        }
        _ => false,
    }
}

fn typed_expr_pyobject_input_is_borrowed_from_local_env(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> bool {
    if let Some(is_borrowed) = typed_expr_planned_pyobject_input_is_borrowed_from_local_env(
        expr,
        local_env,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    ) {
        return is_borrowed;
    }
    if !expr
        .result_demand()
        .map(ResultDemand::borrowed_ok)
        .unwrap_or(true)
    {
        return false;
    }
    typed_expr_is_borrowable_from_local_env(
        expr,
        local_env,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    )
}

fn typed_expr_planned_pyobject_input_is_borrowed_from_local_env(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    stack_slots: &StackSlots,
    storage_layout: Option<&StorageLayout>,
) -> Option<bool> {
    let TypedPlannedResult::PyObject { ownership } = expr.planned_result()? else {
        return Some(false);
    };
    Some(match ownership {
        TypedPyObjectOwnershipPlan::BorrowedLocal { location } => {
            typed_expr_local_load_location(expr) == Some(location)
                && typed_expr_is_borrowable_from_local_env(
                    expr,
                    local_env,
                    stack_slots,
                    storage_layout,
                )
        }
        TypedPyObjectOwnershipPlan::Immortal => true,
        TypedPyObjectOwnershipPlan::Owned => false,
    })
}

fn typed_expr_local_load_location(expr: &InstrTyped) -> Option<LocalLocation> {
    match expr {
        InstrTyped::Load(op) => op.name.local_location(),
        _ => None,
    }
}

fn local_name_for_location<'a>(
    storage_layout: &'a StorageLayout,
    location: LocalLocation,
) -> &'a str {
    storage_layout
        .stack_slots()
        .get(location.slot() as usize)
        .map(String::as_str)
        .unwrap_or_else(|| panic!("missing stack slot for local location {}", location.slot()))
}

fn emit_preserved_state_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    local_env
        .load_name(fb, "_dp_state", ctx, true)
        .unwrap_or_else(|| {
            let qualname = ctx
                .module
                .callable_defs
                .iter()
                .find(|function| function.function_id == ctx.function_id)
                .map(|function| function.names.qualname.as_str())
                .unwrap_or("<unknown>");
            panic!(
                "preserved slots require the generator resume state local _dp_state [function={qualname} id={}]",
                ctx.function_id
            )
        })
}

fn preserved_slot_storage_for_location(
    ctx: &JitEmitCtx<'_>,
    location: PreservedLocation,
) -> PreservedSlotStorage {
    ctx.storage_layout
        .as_ref()
        .and_then(|layout| layout.preserved_slot(location.slot()))
        .map(|slot| slot.storage)
        .unwrap_or_else(|| {
            panic!(
                "missing preserved slot {} in storage layout for function {}",
                location.slot(),
                ctx.function_id
            )
        })
}

fn preserved_values_base_value(ctx: &JitEmitCtx<'_>) -> ir::Value {
    ctx.consts.preserved_values_base_value.unwrap_or_else(|| {
        panic!(
            "missing preserved values base for function {}",
            ctx.function_id
        )
    })
}

fn preserved_values_slot_offset(slot: u32) -> Result<i32, String> {
    let slot_offset = i64::from(slot)
        * i64::try_from(std::mem::size_of::<u64>())
            .map_err(|_| "preserved slot word size does not fit i64".to_string())?;
    i32::try_from(slot_offset)
        .map_err(|_| format!("preserved slot offset {slot_offset} does not fit i32"))
}

fn emit_codegen_non_local_name_load(
    fb: &mut FunctionBuilder<'_>,
    name: &ResolvedName,
    _load_instr_id: Option<InstrId>,
    _local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
    _borrowed: bool,
) -> Option<ir::Value> {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    match name.location {
        NameLocation::Constant(index) => Some(emit_owned_module_constant(
            fb,
            ModuleConstantId(index as usize),
            ctx,
        )),
        NameLocation::GlobalName => {
            panic!("symbolic global name reached JIT codegen without the global_index pass");
        }
        NameLocation::Global(slot) => {
            let globals_obj = ctx.consts.block_const;
            let builtins_obj = load_function_env_obj(
                fb,
                ptr_ty,
                ctx.consts.function_env_value,
                FUNCTION_ENV_BUILTINS_OBJ_OFFSET,
            );
            let name_obj = emit_owned_module_constant(
                fb,
                ctx.module_constants
                    .require_unicode_constant_id(name.id.as_str()),
                ctx,
            );
            let slot_index = fb.ins().iconst(ir::types::I64, i64::from(slot.slot()));
            let value_inst = fb.ins().call(
                ctx.load_global_fast_ref,
                &[globals_obj, builtins_obj, name_obj, slot_index],
            );
            let value = fb.inst_results(value_inst)[0];
            let value_ok_block = fb.create_block();
            fb.append_block_param(value_ok_block, ptr_ty);
            let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
            fb.ins().brif(
                value_is_null,
                ctx.consts.step_null_block,
                &step_null_block_args(ctx),
                value_ok_block,
                &[ir::BlockArg::Value(value)],
            );

            fb.switch_to_block(value_ok_block);
            Some(fb.block_params(value_ok_block)[0])
        }
        NameLocation::RuntimeName(runtime_name) => {
            if let Some(constant_id) = ctx.module_constants.runtime_name_constant_id(runtime_name) {
                return Some(emit_owned_module_constant(fb, constant_id, ctx));
            }
            let runtime_name_id = runtime_name_id_value(fb, Some(runtime_name));
            let value_inst = fb
                .ins()
                .call(ctx.load_runtime_obj_by_id_ref, &[runtime_name_id]);
            let value = fb.inst_results(value_inst)[0];
            let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
            let value_ok_block = fb.create_block();
            fb.append_block_param(value_ok_block, ptr_ty);
            fb.ins().brif(
                value_is_null,
                ctx.consts.step_null_block,
                &step_null_block_args(ctx),
                value_ok_block,
                &[ir::BlockArg::Value(value)],
            );
            fb.switch_to_block(value_ok_block);
            Some(fb.block_params(value_ok_block)[0])
        }
        NameLocation::Preserved(location) => {
            let values = preserved_values_base_value(ctx);
            let slot_offset = preserved_values_slot_offset(location.slot()).unwrap_or_else(|err| {
                panic!(
                    "invalid preserved load offset for function {} slot {}: {err}",
                    ctx.function_id,
                    location.slot()
                )
            });
            match preserved_slot_storage_for_location(ctx, location) {
                PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::PyCellObject => {
                    let value = fb
                        .ins()
                        .load(ptr_ty, ir::MemFlags::trusted(), values, slot_offset);
                    Some(emit_checked_local_value_or_unbound(
                        fb,
                        name.id.as_str(),
                        value,
                        LocalRefKind::Borrowed,
                        ctx,
                        false,
                    ))
                }
                PreservedSlotStorage::I64 => {
                    let value = fb.ins().load(
                        ctx.consts.i64_ty,
                        ir::MemFlags::trusted(),
                        values,
                        slot_offset,
                    );
                    let value_inst = fb.ins().call(ctx.py_long_from_i64_ref, &[value]);
                    Some(emit_checked_owned_pyobject_result(
                        fb,
                        fb.inst_results(value_inst)[0],
                        ctx,
                    ))
                }
            }
        }
        NameLocation::Local(_) | NameLocation::Cell(_) => None,
    }
}

fn runtime_name_id_value(
    fb: &mut FunctionBuilder<'_>,
    runtime_name: Option<RuntimeName>,
) -> ir::Value {
    let runtime_name = runtime_name.expect("runtime-name load should carry a RuntimeName id");
    fb.ins()
        .iconst(ir::types::I64, i64::from(runtime_name.id()))
}

fn emit_cell_value_load_from_raw_cell(
    fb: &mut FunctionBuilder<'_>,
    cell_obj: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let value_inst = fb.ins().call(ctx.load_cell_ref, &[cell_obj]);
    let value = fb.inst_results(value_inst)[0];
    ctx.refcount_emitter()
        .with_family(RefcountFamily::OwnedTemporary)
        .emit_decref(fb, cell_obj, None);
    let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
    let value_ok_block = fb.create_block();
    fb.append_block_param(value_ok_block, ptr_ty);
    fb.ins().brif(
        value_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        value_ok_block,
        &[ir::BlockArg::Value(value)],
    );
    fb.switch_to_block(value_ok_block);
    fb.block_params(value_ok_block)[0]
}

fn emit_optional_counter_increment_for_kind(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    counters: &HashMap<InstrId, CounterRef>,
    instr_id: InstrId,
) {
    if let Some(counter_ref) = counters.get(&instr_id).copied() {
        let counter_slot = scalar_counter_slot_for_ref(ctx.counter_slots_by_id, counter_ref)
            .unwrap_or_else(|err| panic!("{err}"));
        let scalar_counter_base_value = ctx.consts.scalar_counter_base_value.unwrap_or_else(|| {
            panic!(
                "missing scalar counter base for counter id {}",
                counter_ref.counter_id.0
            )
        });
        emit_increment_counter_slot(fb, scalar_counter_base_value, counter_slot);
    }
}

fn emit_codegen_indexed_global_load(
    fb: &mut FunctionBuilder<'_>,
    globals_obj: ir::Value,
    name_obj: ir::Value,
    slot_index: ir::Value,
    instr_id: InstrId,
    guard_miss_resume_point: LocalEnvResumePoint,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let result_block = fb.create_block();
    fb.append_block_param(result_block, ptr_ty);
    let fallback_block = fb.create_block();
    fb.set_cold_block(fallback_block);
    let guard_miss_dispatch = prepare_optional_guard_miss_dispatch(
        ctx.guard_miss_target_for_resume_point(guard_miss_resume_point, fallback_block),
        fallback_block,
        ctx.guard_miss_deopt_ref_for_instr_id(instr_id),
    );
    let direct_block = fb.create_block();
    fb.append_block_param(direct_block, ptr_ty);

    let direct_inst = fb.ins().call(
        ctx.probe_global_indexed_ref,
        &[globals_obj, name_obj, slot_index],
    );
    let direct_value = fb.inst_results(direct_inst)[0];
    let direct_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, direct_value, null_ptr);
    fb.ins().brif(
        direct_is_null,
        guard_miss_dispatch.branch_block(),
        &[],
        direct_block,
        &[ir::BlockArg::Value(direct_value)],
    );

    fb.switch_to_block(direct_block);
    let direct_value = fb.block_params(direct_block)[0];
    ctx.emit_incref_for_family(
        fb,
        direct_value,
        Some(PyObjFacts::unknown().with_non_null_ref()),
        RefcountFamily::BorrowedResultClone,
    );
    emit_optional_counter_increment_for_kind(fb, ctx, ctx.global_indexed_hit_counter_ids, instr_id);
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(direct_value)]);

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(fallback_block) => {
            fb.switch_to_block(fallback_block);
            emit_optional_counter_increment_for_kind(
                fb,
                ctx,
                ctx.global_indexed_fallback_counter_ids,
                instr_id,
            );
            let builtins_obj = load_function_env_obj(
                fb,
                ptr_ty,
                ctx.consts.function_env_value,
                FUNCTION_ENV_BUILTINS_OBJ_OFFSET,
            );
            let fallback_inst = fb.ins().call(
                ctx.load_global_slow_ref,
                &[globals_obj, builtins_obj, name_obj, slot_index],
            );
            let fallback_value = fb.inst_results(fallback_inst)[0];
            let fallback_is_null =
                fb.ins()
                    .icmp(ir::condcodes::IntCC::Equal, fallback_value, null_ptr);
            fb.ins().brif(
                fallback_is_null,
                ctx.consts.step_null_block,
                &step_null_block_args(ctx),
                result_block,
                &[ir::BlockArg::Value(fallback_value)],
            );
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            fb.switch_to_block(block);
            fb.set_cold_block(block);
            emit_optional_counter_increment_for_kind(
                fb,
                ctx,
                ctx.global_indexed_fallback_counter_ids,
                instr_id,
            );
            let deopt_result = emit_deopt_resume_call_with_local_env(
                fb,
                target,
                deopt_resume_ref,
                globals_obj,
                ctx,
                local_env,
            );
            emit_deopt_result_return_or_step_null(fb, ctx, deopt_result);
        }
    }

    fb.switch_to_block(result_block);
    fb.block_params(result_block)[0]
}

fn emit_planned_indexed_global_load(
    fb: &mut FunctionBuilder<'_>,
    globals_obj: ir::Value,
    name: &str,
    expected_index: u32,
    instr_id: InstrId,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let name_obj = emit_owned_module_constant(
        fb,
        ctx.module_constants.require_unicode_constant_id(name),
        ctx,
    );
    let slot_index = fb.ins().iconst(ir::types::I64, i64::from(expected_index));
    let guard_miss_resume_point =
        ctx.guard_miss_resume_point
            .unwrap_or(LocalEnvResumePoint::BeforeInstr {
                key: InstrKey::new(ctx.function_id, instr_id),
            });
    emit_codegen_indexed_global_load(
        fb,
        globals_obj,
        name_obj,
        slot_index,
        instr_id,
        guard_miss_resume_point,
        local_env,
        ctx,
    )
}

fn emit_borrowed_planned_indexed_global_load(
    fb: &mut FunctionBuilder<'_>,
    globals_obj: ir::Value,
    name: &str,
    expected_index: u32,
    instr_id: InstrId,
    fallback_block: ir::Block,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let name_obj = emit_owned_module_constant(
        fb,
        ctx.module_constants.require_unicode_constant_id(name),
        ctx,
    );
    let slot_index = fb.ins().iconst(ir::types::I64, i64::from(expected_index));
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let result_block = fb.create_block();
    fb.append_block_param(result_block, ptr_ty);
    let miss_block = fb.create_block();
    fb.set_cold_block(miss_block);

    let direct_inst = fb.ins().call(
        ctx.probe_global_indexed_ref,
        &[globals_obj, name_obj, slot_index],
    );
    let direct_value = fb.inst_results(direct_inst)[0];
    let direct_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, direct_value, null_ptr);
    fb.ins().brif(
        direct_is_null,
        miss_block,
        &[],
        result_block,
        &[ir::BlockArg::Value(direct_value)],
    );

    fb.switch_to_block(miss_block);
    emit_optional_counter_increment_for_kind(
        fb,
        ctx,
        ctx.global_indexed_fallback_counter_ids,
        instr_id,
    );
    fb.ins().jump(fallback_block, &[]);

    fb.switch_to_block(result_block);
    emit_optional_counter_increment_for_kind(fb, ctx, ctx.global_indexed_hit_counter_ids, instr_id);
    fb.block_params(result_block)[0]
}

fn codegen_expr_helper_name<'a>(
    expr: &'a InstrBlockPy,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a str> {
    match expr {
        InstrBlockPy::Load(op)
            if op.name.location.is_global() || op.name.location.is_runtime_name() =>
        {
            Some(op.name.id.as_str())
        }
        InstrBlockPy::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_runtime_name_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn codegen_expr_static_runtime_name<'a>(
    expr: &'a InstrBlockPy,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a str> {
    match expr {
        InstrBlockPy::Load(op) if op.name.location.is_runtime_name() => Some(op.name.id.as_str()),
        InstrBlockPy::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_runtime_name_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn typed_expr_static_runtime_name<'a>(
    expr: &'a InstrTyped,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a str> {
    match expr {
        InstrTyped::Load(op) if op.name.location.is_runtime_name() => Some(op.name.id.as_str()),
        InstrTyped::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_runtime_name_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn codegen_expr_runtime_helper(
    expr: &InstrBlockPy,
    ctx: &JitEmitCtx<'_>,
) -> Option<RuntimeHelperId> {
    ctx.value_facts_for_expr(expr)
        .and_then(ValueFacts::runtime_helper)
        .or_else(|| {
            codegen_expr_helper_name(expr, ctx.module_constants)
                .and_then(RuntimeHelperId::from_runtime_symbol)
        })
}

fn typed_expr_helper_name<'a>(
    expr: &'a InstrTyped,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a str> {
    match expr {
        InstrTyped::Load(op)
            if op.name.location.is_global() || op.name.location.is_runtime_name() =>
        {
            Some(op.name.id.as_str())
        }
        InstrTyped::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_runtime_name_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn typed_expr_runtime_helper(expr: &InstrTyped, ctx: &JitEmitCtx<'_>) -> Option<RuntimeHelperId> {
    expr.result_facts()
        .and_then(ValueFacts::runtime_helper)
        .or_else(|| {
            typed_expr_helper_name(expr, ctx.module_constants)
                .and_then(RuntimeHelperId::from_runtime_symbol)
        })
}

struct SuperInstanceArg {
    value: ir::Value,
    is_borrowed: bool,
    is_deleted: Option<ir::Value>,
}

fn emit_local_value_for_super_deleted_name_arg(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrBlockPy,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Option<(ir::Value, ir::Value)> {
    let InstrBlockPy::Load(op) = expr else {
        return None;
    };
    let location = op.name.local_location()?;
    let layout = ctx
        .storage_layout
        .as_ref()
        .expect("Load local slot should have storage layout during codegen");
    let name = local_name_for_location(layout, location);
    let value = if let Some(index) = local_env
        .entry_index_for_location(location)
        .or_else(|| local_env.entry_index_for_name(name))
    {
        local_env.entries[index].value()
    } else {
        let slot = ctx.stack_slots.slot_for_block_arg_name(name)?;
        fb.ins().stack_load(ctx.consts.ptr_ty, slot, 0)
    };
    let null_ptr = fb.ins().iconst(ctx.consts.ptr_ty, 0);
    let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
    Some((value, value_is_null))
}

fn emit_super_instance_arg_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    instance_expr: &InstrBlockPy,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> SuperInstanceArg {
    if let Some((value, is_deleted)) =
        emit_local_value_for_super_deleted_name_arg(fb, instance_expr, local_env, ctx)
    {
        return SuperInstanceArg {
            value,
            is_borrowed: true,
            is_deleted: Some(is_deleted),
        };
    }
    let instance_is_borrowed =
        codegen_expr_pyobject_input_is_borrowed_from_local_env(instance_expr, local_env, ctx);
    let instance = emit_codegen_expr_with_local_env(
        fb,
        instance_expr,
        local_env,
        ctx,
        instance_is_borrowed,
        codegen_env,
        func_imports,
    );
    SuperInstanceArg {
        value: instance,
        is_borrowed: instance_is_borrowed,
        is_deleted: None,
    }
}

fn emit_codegen_super_helper_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable_expr: &InstrBlockPy,
    super_fn_expr: &InstrBlockPy,
    cls_expr: &InstrBlockPy,
    instance_expr: &InstrBlockPy,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let callable_is_borrowed =
        codegen_expr_pyobject_input_is_borrowed_from_local_env(callable_expr, local_env, ctx);
    let callable = emit_codegen_expr_with_local_env(
        fb,
        callable_expr,
        local_env,
        ctx,
        callable_is_borrowed,
        codegen_env,
        func_imports,
    );

    let super_fn_is_borrowed =
        codegen_expr_pyobject_input_is_borrowed_from_local_env(super_fn_expr, local_env, ctx);
    let super_fn = emit_codegen_expr_with_local_env(
        fb,
        super_fn_expr,
        local_env,
        ctx,
        super_fn_is_borrowed,
        codegen_env,
        func_imports,
    );

    let cls_is_borrowed =
        codegen_expr_pyobject_input_is_borrowed_from_local_env(cls_expr, local_env, ctx);
    let cls = emit_codegen_expr_with_local_env(
        fb,
        cls_expr,
        local_env,
        ctx,
        cls_is_borrowed,
        codegen_env,
        func_imports,
    );

    let mut instance_arg = emit_super_instance_arg_with_local_env(
        fb,
        instance_expr,
        local_env,
        ctx,
        codegen_env,
        func_imports,
    );
    if let Some(instance_is_deleted) = instance_arg.is_deleted {
        let instance_deleted_block = fb.create_block();
        let instance_ok_block = fb.create_block();
        fb.append_block_param(instance_ok_block, ptr_ty);
        fb.ins().brif(
            instance_is_deleted,
            instance_deleted_block,
            &[],
            instance_ok_block,
            &[ir::BlockArg::Value(instance_arg.value)],
        );

        fb.switch_to_block(instance_deleted_block);
        let raise_super_arg_deleted_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_RAISE_SUPER_ARG_DELETED_IMPORT,
        );
        fb.ins().call(raise_super_arg_deleted_ref, &[]);
        if !cls_is_borrowed {
            ctx.refcount_emitter()
                .with_family(RefcountFamily::OwnedTemporary)
                .emit_decref(fb, cls, None);
        }
        if !super_fn_is_borrowed {
            ctx.refcount_emitter()
                .with_family(RefcountFamily::OwnedTemporary)
                .emit_decref(fb, super_fn, None);
        }
        if !callable_is_borrowed {
            ctx.refcount_emitter()
                .with_family(RefcountFamily::OwnedTemporary)
                .emit_decref(fb, callable, None);
        }
        fb.ins()
            .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

        fb.switch_to_block(instance_ok_block);
        instance_arg.value = fb.block_params(instance_ok_block)[0];
    }

    let call_inst = fb.ins().call(
        ctx.py_call_positional_three_ref,
        &[
            ctx.consts.thread_state_value,
            callable,
            super_fn,
            cls,
            instance_arg.value,
            null_ptr,
        ],
    );
    if !instance_arg.is_borrowed {
        ctx.emit_decref_for_family(fb, instance_arg.value, None, RefcountFamily::OwnedTemporary);
    }
    if !cls_is_borrowed {
        ctx.refcount_emitter()
            .with_family(RefcountFamily::OwnedTemporary)
            .emit_decref(fb, cls, None);
    }
    if !super_fn_is_borrowed {
        ctx.refcount_emitter()
            .with_family(RefcountFamily::OwnedTemporary)
            .emit_decref(fb, super_fn, None);
    }
    if !callable_is_borrowed {
        ctx.refcount_emitter()
            .with_family(RefcountFamily::OwnedTemporary)
            .emit_decref(fb, callable, None);
    }

    let call_value = fb.inst_results(call_inst)[0];
    let call_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
    let call_ok_block = fb.create_block();
    fb.append_block_param(call_ok_block, ptr_ty);
    fb.ins().brif(
        call_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        call_ok_block,
        &[ir::BlockArg::Value(call_value)],
    );
    fb.switch_to_block(call_ok_block);
    fb.block_params(call_ok_block)[0]
}

fn emit_resolved_direct_function_metadata_and_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    target_function: &BlockPyFunction<impl ModuleShape>,
    ctx: &JitEmitCtx<'_>,
) -> (ir::Value, ir::Value) {
    #[repr(C)]
    struct PyHeapTypeObjectSoacPrefix {
        ht_type: ffi::PyTypeObject,
        as_async: ffi::PyAsyncMethods,
        as_number: ffi::PyNumberMethods,
        as_mapping: ffi::PyMappingMethods,
        as_sequence: ffi::PySequenceMethods,
        as_buffer: ffi::PyBufferProcs,
        ht_name: *mut ffi::PyObject,
        ht_slots: *mut ffi::PyObject,
        ht_qualname: *mut ffi::PyObject,
        ht_cached_keys: *mut std::ffi::c_void,
        ht_module: *mut ffi::PyObject,
        ht_tpname: *mut i8,
        ht_token: *mut std::ffi::c_void,
        ht_soac_metadata: *mut std::ffi::c_void,
        ht_soac_metadata_destructor: *mut std::ffi::c_void,
        ht_soac_function_id: u64,
    }

    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    let metadata = if is_constructor_entry_function(target_function) {
        fb.ins().load(
            ptr_ty,
            ir::MemFlags::trusted(),
            callable,
            offset_of!(PyHeapTypeObjectSoacPrefix, ht_soac_metadata) as i32,
        )
    } else {
        load_py_function_soac_metadata_obj(fb, ptr_ty, callable)
    };
    let metadata_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, metadata, null_ptr);
    let load_env_block = fb.create_block();
    let compile_block = fb.create_block();
    fb.set_cold_block(compile_block);
    let done_block = fb.create_block();
    fb.append_block_param(done_block, ptr_ty);
    fb.append_block_param(done_block, ptr_ty);

    fb.ins()
        .brif(metadata_is_null, compile_block, &[], load_env_block, &[]);

    fb.switch_to_block(load_env_block);
    let env = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        metadata,
        PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
    );
    let env_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, env, null_ptr);
    fb.ins().brif(
        env_is_null,
        compile_block,
        &[],
        done_block,
        &[ir::BlockArg::Value(metadata), ir::BlockArg::Value(env)],
    );

    fb.switch_to_block(compile_block);
    let compiled_env_inst = fb
        .ins()
        .call(ctx.direct_compile_function_env_ref, &[callable, metadata]);
    let compiled_env = fb.inst_results(compiled_env_inst)[0];
    let compiled_env_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, compiled_env, null_ptr);
    fb.ins().brif(
        compiled_env_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        done_block,
        &[
            ir::BlockArg::Value(metadata),
            ir::BlockArg::Value(compiled_env),
        ],
    );

    fb.switch_to_block(done_block);
    (
        fb.block_params(done_block)[0],
        fb.block_params(done_block)[1],
    )
}

fn emit_resolved_direct_entry_ptr(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    metadata: ir::Value,
    function_env: ir::Value,
    entry_kind: DirectCallEntryKind,
    ctx: &JitEmitCtx<'_>,
) -> (ir::Value, ir::Value) {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let offset = match entry_kind {
        DirectCallEntryKind::Core => FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET,
        DirectCallEntryKind::DefaultResolving => FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET,
    };
    let initial_callee_ptr = load_function_env_obj(fb, ptr_ty, function_env, offset);
    let initial_callee_is_null =
        fb.ins()
            .icmp(ir::condcodes::IntCC::Equal, initial_callee_ptr, null_ptr);
    let compile_block = fb.create_block();
    fb.set_cold_block(compile_block);
    let check_deopt_block = fb.create_block();
    fb.append_block_param(check_deopt_block, ptr_ty);
    fb.append_block_param(check_deopt_block, ptr_ty);
    let ready_block = fb.create_block();
    fb.append_block_param(ready_block, ptr_ty);
    fb.append_block_param(ready_block, ptr_ty);
    fb.ins().brif(
        initial_callee_is_null,
        compile_block,
        &[],
        check_deopt_block,
        &[
            ir::BlockArg::Value(function_env),
            ir::BlockArg::Value(initial_callee_ptr),
        ],
    );

    fb.switch_to_block(check_deopt_block);
    let ready_env = fb.block_params(check_deopt_block)[0];
    let ready_callee = fb.block_params(check_deopt_block)[1];
    let deopt_table =
        load_function_env_obj(fb, ptr_ty, ready_env, FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET);
    let deopt_table_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, deopt_table, null_ptr);
    fb.ins().brif(
        deopt_table_is_null,
        compile_block,
        &[],
        ready_block,
        &[
            ir::BlockArg::Value(ready_env),
            ir::BlockArg::Value(ready_callee),
        ],
    );

    fb.switch_to_block(compile_block);
    let compiled_env_inst = fb
        .ins()
        .call(ctx.direct_compile_function_env_ref, &[callable, metadata]);
    let compiled_env = fb.inst_results(compiled_env_inst)[0];
    let compiled_env_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, compiled_env, null_ptr);
    let compiled_env_ok = fb.create_block();
    fb.ins().brif(
        compiled_env_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        compiled_env_ok,
        &[],
    );

    fb.switch_to_block(compiled_env_ok);
    let compiled_callee_ptr = load_function_env_obj(fb, ptr_ty, compiled_env, offset);
    let compiled_callee_is_null =
        fb.ins()
            .icmp(ir::condcodes::IntCC::Equal, compiled_callee_ptr, null_ptr);
    let compiled_check_deopt_block = fb.create_block();
    fb.append_block_param(compiled_check_deopt_block, ptr_ty);
    fb.ins().brif(
        compiled_callee_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        compiled_check_deopt_block,
        &[ir::BlockArg::Value(compiled_callee_ptr)],
    );

    fb.switch_to_block(compiled_check_deopt_block);
    let compiled_callee_ptr = fb.block_params(compiled_check_deopt_block)[0];
    let compiled_deopt_table = load_function_env_obj(
        fb,
        ptr_ty,
        compiled_env,
        FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET,
    );
    let compiled_deopt_table_is_null =
        fb.ins()
            .icmp(ir::condcodes::IntCC::Equal, compiled_deopt_table, null_ptr);
    fb.ins().brif(
        compiled_deopt_table_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        ready_block,
        &[
            ir::BlockArg::Value(compiled_env),
            ir::BlockArg::Value(compiled_callee_ptr),
        ],
    );

    fb.switch_to_block(ready_block);
    (
        fb.block_params(ready_block)[0],
        fb.block_params(ready_block)[1],
    )
}

fn emit_constructor_entry_type_metadata(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    cls_value: ir::Value,
) -> ir::Value {
    #[repr(C)]
    struct PyHeapTypeObjectSoacPrefix {
        ht_type: ffi::PyTypeObject,
        as_async: ffi::PyAsyncMethods,
        as_number: ffi::PyNumberMethods,
        as_mapping: ffi::PyMappingMethods,
        as_sequence: ffi::PySequenceMethods,
        as_buffer: ffi::PyBufferProcs,
        ht_name: *mut ffi::PyObject,
        ht_slots: *mut ffi::PyObject,
        ht_qualname: *mut ffi::PyObject,
        ht_cached_keys: *mut std::ffi::c_void,
        ht_module: *mut ffi::PyObject,
        ht_tpname: *mut i8,
        ht_token: *mut std::ffi::c_void,
        ht_soac_metadata: *mut std::ffi::c_void,
        ht_soac_metadata_destructor: *mut std::ffi::c_void,
        ht_soac_function_id: u64,
    }

    fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        cls_value,
        offset_of!(PyHeapTypeObjectSoacPrefix, ht_soac_metadata) as i32,
    )
}

fn emit_ready_constructor_entry_function_env(
    fb: &mut FunctionBuilder<'_>,
    cls_value: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let metadata = emit_constructor_entry_type_metadata(fb, ptr_ty, cls_value);
    let metadata_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, metadata, null_ptr);
    let load_env_block = fb.create_block();
    let compile_block = fb.create_block();
    fb.set_cold_block(compile_block);
    let check_deopt_block = fb.create_block();
    fb.append_block_param(check_deopt_block, ptr_ty);
    let done_block = fb.create_block();
    fb.append_block_param(done_block, ptr_ty);

    fb.ins().brif(
        metadata_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        load_env_block,
        &[],
    );

    fb.switch_to_block(load_env_block);
    let function_env = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        metadata,
        PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
    );
    let function_env_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, function_env, null_ptr);
    fb.ins().brif(
        function_env_is_null,
        compile_block,
        &[],
        check_deopt_block,
        &[ir::BlockArg::Value(function_env)],
    );

    fb.switch_to_block(check_deopt_block);
    let ready_env = fb.block_params(check_deopt_block)[0];
    let deopt_table =
        load_function_env_obj(fb, ptr_ty, ready_env, FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET);
    let deopt_table_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, deopt_table, null_ptr);
    fb.ins().brif(
        deopt_table_is_null,
        compile_block,
        &[],
        done_block,
        &[ir::BlockArg::Value(ready_env)],
    );

    fb.switch_to_block(compile_block);
    let compiled_env_inst = fb
        .ins()
        .call(ctx.direct_compile_function_env_ref, &[cls_value, metadata]);
    let compiled_env = fb.inst_results(compiled_env_inst)[0];
    let compiled_env_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, compiled_env, null_ptr);
    fb.ins().brif(
        compiled_env_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        done_block,
        &[ir::BlockArg::Value(compiled_env)],
    );

    fb.switch_to_block(done_block);
    fb.block_params(done_block)[0]
}

fn emit_take_current_raised_exception(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
) -> ir::Value {
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let raised_exc = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        thread_state_value,
        PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET,
    );
    fb.ins().store(
        ir::MemFlags::trusted(),
        null_ptr,
        thread_state_value,
        PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET,
    );
    raised_exc
}

fn emit_current_raised_exception(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
) -> ir::Value {
    fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        thread_state_value,
        PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET,
    )
}

fn emit_take_current_raised_exception_or_trap(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
) -> ir::Value {
    let raised_exc = emit_take_current_raised_exception(fb, ptr_ty, thread_state_value);
    fb.ins().trapz(raised_exc, MISSING_PYTHON_EXCEPTION_TRAP);
    raised_exc
}

#[derive(Clone)]
struct JitEmitConsts {
    step_null_block: ir::Block,
    step_null_args: Vec<ir::Value>,
    ptr_ty: ir::Type,
    i64_ty: ir::Type,
    i32_ty: ir::Type,
    function_env_value: ir::Value,
    function_data_value: ir::Value,
    module_constant_object_globals: Vec<ir::GlobalValue>,
    scalar_counter_base_value: Option<ir::Value>,
    top_value_counter_base_value: Option<ir::Value>,
    thread_state_value: ir::Value,
    preserved_values_base_value: Option<ir::Value>,
    none_constant_id: ModuleConstantId,
    true_constant_id: ModuleConstantId,
    false_constant_id: ModuleConstantId,
    empty_tuple_constant_id: ModuleConstantId,
    block_const: ir::Value,
    module_constant_accesses: ModuleConstantAccessTable,
}

#[derive(Clone)]
struct JitEmitCtx<'mc> {
    module: &'mc BlockPyModule<TypedBlockPyModuleShape>,
    function_id: RuntimeFunctionId,
    function_kind: FunctionKind,
    indexed_field_guards_by_instr: &'mc HashMap<InstrId, Vec<TypedIndexedFieldGuard>>,
    module_constants: &'mc ModuleCodegenConstants,
    value_facts: &'mc FactStore,
    deopt_resume_plan: &'mc PlannedJitDeoptResumeFunction,
    runtime_supported_deopt_resume_points: Option<&'mc [LocalEnvResumePoint]>,
    refcount_plan: &'mc FunctionRefcountPlan,
    cleanup_root_slot_states: &'mc PlannedCleanupRootSlotStates,
    truthiness_only_local_locations: &'mc HashSet<LocalLocation>,
    return_cleanup_blocks_by_label: &'mc HashMap<BlockLabel, ir::Block>,
    instr_locations: &'mc InstrLocationMap,
    counter_slots_by_id: &'mc [CounterRuntimeSlot],
    storage_layout: Option<StorageLayout>,
    function_runtime_data_layout: &'mc FunctionRuntimeDataLayout,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
    refcount_lowering: RefcountLowering,
    py_call_positional_three_ref: ir::FuncRef,
    py_vectorcall_ref: ir::FuncRef,
    py_handle_pending_ref: Option<ir::FuncRef>,
    handle_pending_checks_enabled: bool,
    refcount_emission_enabled: bool,
    consts: JitEmitConsts,
    load_global_fast_ref: ir::FuncRef,
    probe_global_indexed_ref: ir::FuncRef,
    load_global_slow_ref: ir::FuncRef,
    guard_miss_deopt_stub_ref: Option<ir::FuncRef>,
    guard_miss_deopt_instr_ids: &'mc HashSet<InstrId>,
    guard_miss_deopt_without_refcounts_instr_ids: &'mc HashSet<InstrId>,
    guard_miss_resume_point: Option<LocalEnvResumePoint>,
    load_runtime_obj_by_id_ref: ir::FuncRef,
    enter_recursive_ref: ir::FuncRef,
    direct_compile_function_env_ref: ir::FuncRef,
    pytype_generic_alloc_ref: ir::FuncRef,
    finish_constructor_init_ref: ir::FuncRef,
    pyobject_getattr_ref: ir::FuncRef,
    pyobject_setattr_ref: ir::FuncRef,
    pyobject_getitem_ref: ir::FuncRef,
    pyobject_setitem_ref: ir::FuncRef,
    del_preserved_ref: ir::FuncRef,
    del_preserved_quietly_ref: ir::FuncRef,
    pyobject_to_i64_ref: ir::FuncRef,
    py_long_from_i64_ref: ir::FuncRef,
    raise_unbound_local_error_ref: ir::FuncRef,
    make_function_with_closure_ref: ir::FuncRef,
    make_cell_ref: ir::FuncRef,
    load_cell_ref: ir::FuncRef,
    store_cell_ref: ir::FuncRef,
    py_call_object_ref: ir::FuncRef,
    py_call_with_kw_ref: ir::FuncRef,
    record_top_value_sample_ref: Option<ir::FuncRef>,
    tuple_new_ref: ir::FuncRef,
    tuple_set_item_ref: ir::FuncRef,
    stack_slots: StackSlots,
    exception_state_slots: ExceptionStateSlots,
    pop_handled_exception_ref: ir::FuncRef,
    direct_edge_stats: &'mc DirectEdgeStats,
    direct_call_target_functions:
        &'mc HashMap<RuntimeFunctionId, BlockPyFunction<TypedBlockPyModuleShape>>,
    direct_call_functions: &'mc HashMap<RuntimeFunctionId, DeclaredJitFunction>,
    call_target_counter_ids: &'mc HashMap<InstrId, CounterId>,
    call_direct_target_counter_ids: &'mc HashMap<InstrId, CounterId>,
    call_direct_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    call_direct_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    operator_shape_counter_ids: &'mc HashMap<InstrId, CounterId>,
    getitem_shape_counter_ids: &'mc HashMap<InstrId, CounterId>,
    getitem_specialized_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    getitem_specialized_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    getitem_specialized_hit_counter_ids_by_source:
        &'mc HashMap<(RuntimeFunctionId, InstrId), CounterRef>,
    getitem_specialized_fallback_counter_ids_by_source:
        &'mc HashMap<(RuntimeFunctionId, InstrId), CounterRef>,
    setitem_shape_counter_ids: &'mc HashMap<InstrId, CounterId>,
    setitem_specialized_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    setitem_specialized_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    setitem_specialized_hit_counter_ids_by_source:
        &'mc HashMap<(RuntimeFunctionId, InstrId), CounterRef>,
    setitem_specialized_fallback_counter_ids_by_source:
        &'mc HashMap<(RuntimeFunctionId, InstrId), CounterRef>,
    branch_outcome_counter_ids: &'mc HashMap<InstrId, CounterId>,
    global_indexed_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    global_indexed_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    field_indexed_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    field_indexed_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    field_indexed_hit_counter_ids_by_source: &'mc HashMap<(RuntimeFunctionId, InstrId), CounterRef>,
    field_indexed_fallback_counter_ids_by_source:
        &'mc HashMap<(RuntimeFunctionId, InstrId), CounterRef>,
    field_generic_getattr_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    field_generic_setattr_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    deopt_entry_guard_miss_counter_ids: &'mc HashMap<usize, CounterId>,
    refcount_decref_location_counter_refs: &'mc HashMap<String, CounterRef>,
    allow_local_only_slot_backed_stores: bool,
    exception_forwarded_local_names: Option<&'mc [String]>,
    type_ptr_data_ids: RefCell<HashMap<RelocTypeRef, DataId>>,
    callable_ptr_data_ids: RefCell<HashMap<RelocCallableRef, DataId>>,
}

#[derive(Clone, Copy)]
struct RefcountEmitter {
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    lowering: RefcountLowering,
    family: Option<RefcountFamily>,
}

impl RefcountEmitter {
    fn with_family(self, family: RefcountFamily) -> Self {
        Self {
            family: Some(family),
            ..self
        }
    }

    fn emit_incref(
        self,
        fb: &mut FunctionBuilder<'_>,
        value: ir::Value,
        facts: Option<PyObjFacts>,
    ) {
        with_refcount_family(fb, self.family, |fb| {
            self.lowering.emit_incref(fb, self.ptr_ty, value, facts);
        });
    }

    fn emit_decref(
        self,
        fb: &mut FunctionBuilder<'_>,
        value: ir::Value,
        facts: Option<PyObjFacts>,
    ) {
        with_refcount_family(fb, self.family, |fb| {
            self.lowering
                .emit_decref(fb, self.ptr_ty, self.thread_state_value, value, facts);
        });
    }
}

fn with_refcount_family<T>(
    fb: &mut FunctionBuilder<'_>,
    family: Option<RefcountFamily>,
    emit: impl FnOnce(&mut FunctionBuilder<'_>) -> T,
) -> T {
    let Some(family) = family else {
        return emit(fb);
    };
    let previous_srcloc = fb.srcloc();
    fb.set_srcloc(ir::SourceLoc::new(refcount_family_source_loc_bits(family)));
    let result = emit(fb);
    fb.set_srcloc(previous_srcloc);
    result
}

#[derive(Clone, Copy)]
struct RefcountDecrefLocationCounterParts<'a> {
    counter_refs: &'a HashMap<String, CounterRef>,
    counter_slots_by_id: &'a [CounterRuntimeSlot],
    scalar_counter_base_value: Option<ir::Value>,
}

fn refcount_decref_location_counter_parts<'mc>(
    ctx: &JitEmitCtx<'mc>,
) -> RefcountDecrefLocationCounterParts<'mc> {
    RefcountDecrefLocationCounterParts {
        counter_refs: ctx.refcount_decref_location_counter_refs,
        counter_slots_by_id: ctx.counter_slots_by_id,
        scalar_counter_base_value: ctx.consts.scalar_counter_base_value,
    }
}

#[derive(Clone, Copy)]
struct JitDeoptExitRef {
    function_env_value: ir::Value,
    record_ordinal: i64,
}

#[derive(Clone, Copy)]
struct JitGuardMissTarget {
    fallback_block: ir::Block,
    deopt_exit: JitDeoptExitRef,
}

impl JitGuardMissTarget {
    fn fallback_block(self) -> ir::Block {
        self.fallback_block
    }

    fn deopt_exit(self) -> JitDeoptExitRef {
        self.deopt_exit
    }
}

#[derive(Clone, Copy)]
enum JitGuardMissDispatch {
    FallbackBlock(ir::Block),
    DeoptResume {
        block: ir::Block,
        target: JitDeoptExitRef,
        deopt_resume_ref: ir::FuncRef,
    },
}

impl JitGuardMissDispatch {
    fn branch_block(self) -> ir::Block {
        match self {
            Self::FallbackBlock(block) | Self::DeoptResume { block, .. } => block,
        }
    }
}

fn prepare_guard_miss_dispatch(
    target: JitGuardMissTarget,
    deopt_resume_ref: Option<ir::FuncRef>,
) -> JitGuardMissDispatch {
    match deopt_resume_ref {
        Some(deopt_resume_ref) => JitGuardMissDispatch::DeoptResume {
            block: target.fallback_block(),
            target: target.deopt_exit(),
            deopt_resume_ref,
        },
        None => JitGuardMissDispatch::FallbackBlock(target.fallback_block()),
    }
}

fn prepare_optional_guard_miss_dispatch(
    target: Result<JitGuardMissTarget, RuntimeJitDeoptUnsupportedReason>,
    fallback_block: ir::Block,
    deopt_resume_ref: Option<ir::FuncRef>,
) -> JitGuardMissDispatch {
    let Some(deopt_resume_ref) = deopt_resume_ref else {
        return JitGuardMissDispatch::FallbackBlock(fallback_block);
    };
    let Ok(target) = target else {
        return JitGuardMissDispatch::FallbackBlock(fallback_block);
    };
    prepare_guard_miss_dispatch(target, Some(deopt_resume_ref))
}

fn collect_typed_guard_miss_deopt_instr_ids(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashSet<InstrId> {
    struct Collector {
        instr_ids: HashSet<InstrId>,
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            let exact_int_deopt = typed_expr_has_exact_int_guard_miss_deopt(expr);
            if (expr.guard_miss_deopt_enabled() || exact_int_deopt)
                && let Some(instr_id) = expr.try_semantic_instr_id()
            {
                self.instr_ids.insert(instr_id);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        instr_ids: HashSet::new(),
    };
    collector.visit_fn(function);
    collector.instr_ids
}

fn collect_typed_exact_int_guard_miss_deopt_instr_ids(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashSet<InstrId> {
    struct Collector {
        instr_ids: HashSet<InstrId>,
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if typed_expr_has_exact_int_guard_miss_deopt(expr)
                && let Some(instr_id) = expr.try_semantic_instr_id()
            {
                self.instr_ids.insert(instr_id);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        instr_ids: HashSet::new(),
    };
    collector.visit_fn(function);
    collector.instr_ids
}

fn typed_expr_has_exact_int_guard_miss_deopt(expr: &InstrTyped) -> bool {
    expr.typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
        .is_some_and(|plan| {
            planning::exact_int_return_plan_i64_result(plan).is_some()
                || planning::exact_int_return_plan_i32_bool01_result(plan).is_some()
                || planning::exact_int_return_plan_immortal_pyobject_result(plan).is_some()
        })
}

fn emit_deopt_resume_call(
    fb: &mut FunctionBuilder<'_>,
    target: JitDeoptExitRef,
    deopt_resume_ref: ir::FuncRef,
    globals_obj: ir::Value,
    live_values_base: ir::Value,
    live_value_count: usize,
    ptr_ty: ir::Type,
    i64_ty: ir::Type,
) -> ir::Value {
    let deopt_table = load_function_env_obj(
        fb,
        ptr_ty,
        target.function_env_value,
        FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET,
    );
    let builtins_obj = load_function_env_obj(
        fb,
        ptr_ty,
        target.function_env_value,
        FUNCTION_ENV_BUILTINS_OBJ_OFFSET,
    );
    let function_data = fb.ins().iadd_imm(
        target.function_env_value,
        i64::from(FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET),
    );
    let record_ordinal = fb.ins().iconst(i64_ty, target.record_ordinal);
    let live_value_count = i64::try_from(live_value_count)
        .unwrap_or_else(|_| panic!("deopt live value count does not fit i64"));
    let live_value_count = fb.ins().iconst(i64_ty, live_value_count);
    let call_inst = fb.ins().call(
        deopt_resume_ref,
        &[
            deopt_table,
            globals_obj,
            builtins_obj,
            function_data,
            record_ordinal,
            live_values_base,
            live_value_count,
        ],
    );
    fb.inst_results(call_inst)[0]
}

fn emit_deopt_resume_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    target: JitDeoptExitRef,
    deopt_resume_ref: ir::FuncRef,
    globals_obj: ir::Value,
    ctx: &JitEmitCtx<'_>,
    local_env: &LocalEnv,
) -> ir::Value {
    emit_deopt_entry_guard_miss_counter(fb, target, ctx);
    let (live_values_base, live_value_count) =
        emit_deopt_live_value_buffer(fb, target, ctx, local_env)
            .unwrap_or_else(|err| panic!("{err}"));
    emit_deopt_resume_call(
        fb,
        target,
        deopt_resume_ref,
        globals_obj,
        live_values_base,
        live_value_count,
        ctx.consts.ptr_ty,
        ctx.consts.i64_ty,
    )
}

fn emit_deopt_entry_guard_miss_counter(
    fb: &mut FunctionBuilder<'_>,
    target: JitDeoptExitRef,
    ctx: &JitEmitCtx<'_>,
) {
    let Ok(ordinal) = usize::try_from(target.record_ordinal) else {
        return;
    };
    let Some(counter_id) = ctx.deopt_entry_guard_miss_counter_ids.get(&ordinal) else {
        return;
    };
    let Ok(counter_slot) = scalar_counter_slot_for_id(ctx.counter_slots_by_id, *counter_id) else {
        return;
    };
    let scalar_counter_base_value = ctx.consts.scalar_counter_base_value.unwrap_or_else(|| {
        panic!(
            "missing scalar counter base for deopt-entry counter id {}",
            counter_id.0
        )
    });
    emit_increment_counter_slot(fb, scalar_counter_base_value, counter_slot);
}

fn emit_deopt_result_return_or_step_null(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    deopt_result: ir::Value,
) {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let deopt_result_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, deopt_result, null_ptr);
    let deopt_success_block = fb.create_block();
    fb.append_block_param(deopt_success_block, ptr_ty);
    fb.set_cold_block(deopt_success_block);
    fb.ins().brif(
        deopt_result_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        deopt_success_block,
        &[ir::BlockArg::Value(deopt_result)],
    );

    fb.switch_to_block(deopt_success_block);
    let resumed_result = fb.block_params(deopt_success_block)[0];
    fb.ins().return_(&[resumed_result]);
}

fn emit_deopt_live_value_buffer(
    fb: &mut FunctionBuilder<'_>,
    target: JitDeoptExitRef,
    ctx: &JitEmitCtx<'_>,
    local_env: &LocalEnv,
) -> Result<(ir::Value, usize), String> {
    let point_id = PlannedJitDeoptPointId {
        function_id: ctx.function_id,
        ordinal: usize::try_from(target.record_ordinal).map_err(|_| {
            format!(
                "deopt target ordinal {} is negative or does not fit usize",
                target.record_ordinal
            )
        })?,
    };
    let deopt_point = ctx
        .deopt_resume_plan
        .deopt_point_by_id(point_id)
        .ok_or_else(|| format!("missing planned JIT deopt point {:?}", point_id))?;
    let entry = ctx
        .deopt_resume_plan
        .entry(deopt_point.resume_point)
        .ok_or_else(|| {
            format!(
                "planned JIT deopt point {:?} has no resume entry {:?}",
                point_id, deopt_point.resume_point
            )
        })?;
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    if entry.locals.is_empty() {
        return Ok((null_ptr, 0));
    }

    let mut values = Vec::with_capacity(entry.locals.len());
    for binding in &entry.locals {
        values.push(emit_deopt_live_value_for_binding(
            fb, binding, ctx, local_env, null_ptr,
        )?);
    }

    let slot_size = (values.len() * std::mem::size_of::<u64>()) as u32;
    let stack_slot = fb.create_sized_stack_slot(ir::StackSlotData::new(
        ir::StackSlotKind::ExplicitSlot,
        slot_size,
        0,
    ));
    for (index, value) in values.iter().copied().enumerate() {
        fb.ins().stack_store(
            value,
            stack_slot,
            (index * std::mem::size_of::<u64>()) as i32,
        );
    }
    Ok((fb.ins().stack_addr(ptr_ty, stack_slot, 0), values.len()))
}

fn emit_deopt_live_value_for_binding(
    fb: &mut FunctionBuilder<'_>,
    binding: &LocalEnvResumeBinding,
    ctx: &JitEmitCtx<'_>,
    local_env: &LocalEnv,
    null_ptr: ir::Value,
) -> Result<ir::Value, String> {
    match binding.source {
        LocalEnvResumeValueSource::Unbound => return Ok(null_ptr),
        LocalEnvResumeValueSource::StackSlot(location) => {
            let Some(slot) = deopt_binding_stack_slot_for_location(ctx, location) else {
                return Err(format!(
                    "cannot materialize stack-slot deopt value for local {} at location {:?}",
                    binding.name, binding.location
                ));
            };
            return Ok(fb.ins().stack_load(ctx.consts.ptr_ty, slot, 0));
        }
        LocalEnvResumeValueSource::BlockParam(_)
        | LocalEnvResumeValueSource::StoredValue(_)
        | LocalEnvResumeValueSource::Unknown => {}
    }
    if let Some(index) = local_env
        .entry_index_for_location(binding.location)
        .or_else(|| local_env.entry_index_for_name(binding.name.as_str()))
    {
        let entry = &local_env.entries[index];
        if let Some(facts) = entry.i64_facts() {
            let result = emit_soac_value_result_for_demand(
                fb,
                SoacValue::i64(entry.value(), facts),
                ctx,
                ResultDemand::PYOBJECT_OWNED,
                None,
            );
            let (value, ownership, _) = result.expect_pyobject("scalar deopt live value");
            debug_assert!(ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal));
            return Ok(value);
        }
        return Ok(entry.value());
    }
    if let Some(slot) = ctx
        .stack_slots
        .slot_for_block_arg_name(binding.name.as_str())
        .or_else(|| deopt_binding_stack_slot_for_location(ctx, binding.location))
    {
        return Ok(fb.ins().stack_load(ctx.consts.ptr_ty, slot, 0));
    }
    Err(format!(
        "cannot materialize live deopt value for local {} at location {:?} from source {:?}",
        binding.name, binding.location, binding.source
    ))
}

fn deopt_binding_stack_slot_for_location(
    ctx: &JitEmitCtx<'_>,
    location: LocalLocation,
) -> Option<ir::StackSlot> {
    let layout = ctx.storage_layout.as_ref()?;
    let name = layout
        .stack_slots()
        .get(location.slot() as usize)
        .map(String::as_str)?;
    ctx.stack_slots.slot_for_block_arg_name(name)
}

impl JitEmitCtx<'_> {
    fn refcount_emitter(&self) -> RefcountEmitter {
        RefcountEmitter {
            ptr_ty: self.consts.ptr_ty,
            thread_state_value: self.consts.thread_state_value,
            lowering: self.refcount_lowering,
            family: None,
        }
    }

    fn emit_incref_for_family(
        &self,
        fb: &mut FunctionBuilder<'_>,
        value: ir::Value,
        facts: Option<PyObjFacts>,
        family: RefcountFamily,
    ) {
        self.refcount_emitter()
            .with_family(family)
            .emit_incref(fb, value, facts);
    }

    fn emit_decref_for_family(
        &self,
        fb: &mut FunctionBuilder<'_>,
        value: ir::Value,
        facts: Option<PyObjFacts>,
        family: RefcountFamily,
    ) {
        self.refcount_emitter()
            .with_family(family)
            .emit_decref(fb, value, facts);
    }

    fn value_facts_for_instr_id(&self, instr_id: InstrId) -> Option<ValueFacts> {
        self.value_facts
            .fact_for(InstrKey::new(self.function_id, instr_id))
    }

    fn value_facts_for_expr(&self, expr: &InstrBlockPy) -> Option<ValueFacts> {
        let instr_id = expr.try_semantic_instr_id()?;
        self.value_facts_for_instr_id(instr_id)
    }

    fn require_deopt_point(
        &self,
        point: LocalEnvResumePoint,
    ) -> Result<&PlannedJitDeoptPoint, String> {
        self.deopt_resume_plan.deopt_point(point).ok_or_else(|| {
            format!(
                "missing planned JIT deopt point {:?} for function {}",
                point, self.function_id
            )
        })
    }

    fn require_deopt_record_ref(
        &self,
        point: LocalEnvResumePoint,
    ) -> Result<JitDeoptExitRef, String> {
        let deopt_point = self.require_deopt_point(point)?;
        let ordinal = i64::try_from(deopt_point.id.ordinal).map_err(|_| {
            format!(
                "planned JIT deopt point {:?} for function {} has an ordinal that does not fit i64",
                point, self.function_id
            )
        })?;
        Ok(JitDeoptExitRef {
            function_env_value: self.consts.function_env_value,
            record_ordinal: ordinal,
        })
    }

    fn require_deopt_point_at_block_entry(
        &self,
        block: BlockLabel,
    ) -> Result<JitDeoptExitRef, String> {
        self.require_deopt_record_ref(LocalEnvResumePoint::BlockEntry {
            function_id: self.function_id,
            block,
        })
    }

    fn require_deopt_point_before_instr_id(
        &self,
        instr_id: InstrId,
    ) -> Result<JitDeoptExitRef, String> {
        self.require_deopt_record_ref(LocalEnvResumePoint::BeforeInstr {
            key: InstrKey::new(self.function_id, instr_id),
        })
    }

    fn require_deopt_point_before_term(
        &self,
        block: BlockLabel,
    ) -> Result<JitDeoptExitRef, String> {
        self.require_deopt_record_ref(LocalEnvResumePoint::BeforeTerm {
            function_id: self.function_id,
            block,
        })
    }

    fn guard_miss_target_for_resume_point(
        &self,
        point: LocalEnvResumePoint,
        fallback_block: ir::Block,
    ) -> Result<JitGuardMissTarget, RuntimeJitDeoptUnsupportedReason> {
        if self
            .runtime_supported_deopt_resume_points
            .is_some_and(|supported| !supported.contains(&point))
        {
            return Err(RuntimeJitDeoptUnsupportedReason::MissingInstruction);
        }
        let function = self
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == self.function_id)
            .ok_or(RuntimeJitDeoptUnsupportedReason::MissingFunction)?;
        if let Some(reason) =
            runtime_jit_typed_deopt_continuation_for_point(function, self.instr_locations, point)
                .unsupported_reason()
        {
            return Err(reason);
        }
        let _entry = self
            .deopt_resume_plan
            .entry(point)
            .ok_or(RuntimeJitDeoptUnsupportedReason::MissingPlanRecord)?;
        let deopt_exit = self
            .require_deopt_record_ref(point)
            .map_err(|_| RuntimeJitDeoptUnsupportedReason::MissingPlanRecord)?;
        Ok(JitGuardMissTarget {
            fallback_block,
            deopt_exit,
        })
    }

    fn guard_miss_target_for_codegen_resume_point(
        &self,
        point: LocalEnvResumePoint,
        pre_guard_operands: &[&InstrBlockPy],
        fallback_block: ir::Block,
    ) -> Result<JitGuardMissTarget, RuntimeJitDeoptUnsupportedReason> {
        if pre_guard_operands
            .iter()
            .any(|expr| !runtime_jit_deopt_guard_operand_replay_safe(expr))
        {
            return Err(RuntimeJitDeoptUnsupportedReason::ReplayUnsafeGuardOperand);
        }
        self.guard_miss_target_for_resume_point(point, fallback_block)
    }

    fn guard_miss_target_for_typed_resume_point(
        &self,
        point: LocalEnvResumePoint,
        pre_guard_operands: &[&InstrTyped],
        fallback_block: ir::Block,
    ) -> Result<JitGuardMissTarget, RuntimeJitDeoptUnsupportedReason> {
        if pre_guard_operands
            .iter()
            .any(|expr| !runtime_jit_typed_deopt_guard_operand_replay_safe(expr))
        {
            return Err(RuntimeJitDeoptUnsupportedReason::ReplayUnsafeGuardOperand);
        }
        self.guard_miss_target_for_resume_point(point, fallback_block)
    }

    fn guard_miss_deopt_ref_for_instr_id(&self, instr_id: InstrId) -> Option<ir::FuncRef> {
        if !self.refcount_emission_enabled
            && !self
                .guard_miss_deopt_without_refcounts_instr_ids
                .contains(&instr_id)
        {
            return None;
        }
        self.guard_miss_deopt_instr_ids
            .contains(&instr_id)
            .then_some(self.guard_miss_deopt_stub_ref)
            .flatten()
    }

    fn with_guard_miss_resume_point(&self, point: LocalEnvResumePoint) -> Self {
        let mut ctx = self.clone();
        ctx.guard_miss_resume_point = Some(point);
        ctx
    }

    fn with_step_null_target(
        &self,
        step_null_block: ir::Block,
        step_null_args: Vec<ir::Value>,
    ) -> Self {
        let mut ctx = self.clone();
        ctx.consts.step_null_block = step_null_block;
        ctx.consts.step_null_args = step_null_args;
        ctx
    }
}

#[cfg(test)]
fn infer_jit_value_facts(module: &BlockPyModule<BlockPyModuleShape>) -> FactStore {
    infer_module_value_facts(module)
}

fn direct_call_target_function<'a>(
    ctx: &'a JitEmitCtx<'_>,
    function_id: RuntimeFunctionId,
) -> Option<&'a BlockPyFunction<TypedBlockPyModuleShape>> {
    ctx.module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .or_else(|| ctx.direct_call_target_functions.get(&function_id))
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
}

struct LocalEnvCodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd, Env: JitCodegenEnv> {
    fb: &'a mut FunctionBuilder<'b>,
    local_env: &'c mut LocalEnv,
    ctx: &'c JitEmitCtx<'mc>,
    codegen_env: &'a mut Env,
    func_imports: &'a mut FuncBuildImports<'d>,
    owned_transfer_temp_load: Option<LocalLocation>,
}

#[derive(Clone)]
struct LocalEnvEntry {
    location: Option<LocalLocation>,
    name: String,
    aliases: Vec<String>,
    binding: LocalBindingValue,
    storage: LocalEnvStorage,
    binding_facts: ParamBindingFacts,
}

#[derive(Clone, Copy)]
enum LocalBindingValue {
    PyObject {
        value: ir::Value,
        ref_kind: LocalRefKind,
        py_facts: Option<PyObjFacts>,
    },
    ExactI64 {
        value: ir::Value,
        facts: IntFacts,
    },
    I32Bool01 {
        value: ir::Value,
    },
    Unbound {
        value: ir::Value,
    },
}

impl LocalBindingValue {
    fn pyobject(value: ir::Value, ref_kind: LocalRefKind, py_facts: Option<PyObjFacts>) -> Self {
        if matches!(ref_kind, LocalRefKind::Unbound) {
            return Self::unbound(value);
        }
        Self::PyObject {
            value,
            ref_kind,
            py_facts,
        }
    }

    const fn unbound(value: ir::Value) -> Self {
        Self::Unbound { value }
    }

    fn exact_i64(value: ir::Value, facts: IntFacts) -> Self {
        debug_assert_eq!(facts.width, IntWidth::I64);
        Self::ExactI64 { value, facts }
    }

    const fn i32_bool01(value: ir::Value) -> Self {
        Self::I32Bool01 { value }
    }

    const fn value(self) -> ir::Value {
        match self {
            Self::PyObject { value, .. }
            | Self::ExactI64 { value, .. }
            | Self::I32Bool01 { value }
            | Self::Unbound { value } => value,
        }
    }

    const fn ref_kind(self) -> LocalRefKind {
        match self {
            Self::PyObject { ref_kind, .. } => ref_kind,
            Self::ExactI64 { .. } | Self::I32Bool01 { .. } => LocalRefKind::Immortal,
            Self::Unbound { .. } => LocalRefKind::Unbound,
        }
    }

    const fn py_facts(self) -> Option<PyObjFacts> {
        match self {
            Self::PyObject { py_facts, .. } => py_facts,
            Self::ExactI64 { .. } | Self::I32Bool01 { .. } => None,
            Self::Unbound { .. } => None,
        }
    }

    const fn i64_facts(self) -> Option<IntFacts> {
        match self {
            Self::ExactI64 { facts, .. } => Some(facts),
            Self::PyObject { .. } | Self::I32Bool01 { .. } | Self::Unbound { .. } => None,
        }
    }

    const fn i32_bool01_facts(self) -> Option<IntFacts> {
        match self {
            Self::I32Bool01 { .. } => Some(IntFacts::i32_bool01()),
            Self::PyObject { .. } | Self::ExactI64 { .. } | Self::Unbound { .. } => None,
        }
    }

    const fn is_pyobject(self) -> bool {
        matches!(self, Self::PyObject { .. })
    }
}

impl LocalEnvEntry {
    fn new(
        location: Option<LocalLocation>,
        name: String,
        aliases: Vec<String>,
        binding: LocalBindingValue,
        storage: LocalEnvStorage,
        binding_facts: ParamBindingFacts,
    ) -> Self {
        Self {
            location,
            name,
            aliases,
            binding,
            storage,
            binding_facts,
        }
    }

    fn pyobject(
        location: Option<LocalLocation>,
        name: String,
        aliases: Vec<String>,
        value: ir::Value,
        ref_kind: LocalRefKind,
        storage: LocalEnvStorage,
        binding_facts: ParamBindingFacts,
        py_facts: Option<PyObjFacts>,
    ) -> Self {
        Self::new(
            location,
            name,
            aliases,
            LocalBindingValue::pyobject(value, ref_kind, py_facts),
            storage,
            binding_facts,
        )
    }

    fn exact_i64(
        location: Option<LocalLocation>,
        name: String,
        aliases: Vec<String>,
        value: ir::Value,
        facts: IntFacts,
        storage: LocalEnvStorage,
    ) -> Self {
        Self::new(
            location,
            name,
            aliases,
            LocalBindingValue::exact_i64(value, facts),
            storage,
            ParamBindingFacts::DefinitelyBound,
        )
    }

    fn i32_bool01(
        location: Option<LocalLocation>,
        name: String,
        aliases: Vec<String>,
        value: ir::Value,
        storage: LocalEnvStorage,
    ) -> Self {
        Self::new(
            location,
            name,
            aliases,
            LocalBindingValue::i32_bool01(value),
            storage,
            ParamBindingFacts::DefinitelyBound,
        )
    }

    const fn value(&self) -> ir::Value {
        self.binding.value()
    }

    const fn ref_kind(&self) -> LocalRefKind {
        self.binding.ref_kind()
    }

    const fn py_facts(&self) -> Option<PyObjFacts> {
        self.binding.py_facts()
    }

    const fn i64_facts(&self) -> Option<IntFacts> {
        self.binding.i64_facts()
    }

    const fn i32_bool01_facts(&self) -> Option<IntFacts> {
        self.binding.i32_bool01_facts()
    }

    const fn is_pyobject_binding(&self) -> bool {
        self.binding.is_pyobject()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalEnvStorage {
    LocalOnly,
    StackMirror,
}

#[derive(Clone, Default)]
struct LocalEnv {
    entries: Vec<LocalEnvEntry>,
}

#[derive(Clone)]
struct LocalFailureCleanupValue {
    key: LocalFailureCleanupValueKey,
    value: ir::Value,
    name: String,
    ref_kind: LocalRefKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum LocalFailureCleanupValueKey {
    Location(LocalLocation),
    Name(String),
}

impl LocalFailureCleanupValue {
    fn from_local_env_entry(entry: &LocalEnvEntry) -> Self {
        let key = entry
            .location
            .map(LocalFailureCleanupValueKey::Location)
            .unwrap_or_else(|| LocalFailureCleanupValueKey::Name(entry.name.clone()));
        Self {
            key,
            value: entry.value(),
            name: entry.name.clone(),
            ref_kind: entry.ref_kind(),
        }
    }
}

impl LocalEnv {
    fn bind_entry_location_with_aliases(
        &mut self,
        location: LocalLocation,
        name: &str,
        aliases: Vec<String>,
        value: ir::Value,
        ref_kind: LocalRefKind,
        storage: LocalEnvStorage,
        binding_facts: ParamBindingFacts,
        py_facts: Option<PyObjFacts>,
    ) {
        debug_assert!(
            self.entry_index_for_location(location).is_none(),
            "block-entry LocalEnv location should be bound once"
        );
        self.entries.push(LocalEnvEntry::pyobject(
            Some(location),
            name.to_string(),
            aliases,
            value,
            ref_kind,
            storage,
            binding_facts,
            py_facts,
        ));
    }

    fn bind_entry_location_i64_with_aliases(
        &mut self,
        location: LocalLocation,
        name: &str,
        aliases: Vec<String>,
        value: ir::Value,
        facts: IntFacts,
        storage: LocalEnvStorage,
    ) {
        debug_assert!(
            self.entry_index_for_location(location).is_none(),
            "block-entry LocalEnv location should be bound once"
        );
        self.entries.push(LocalEnvEntry::exact_i64(
            Some(location),
            name.to_string(),
            aliases,
            value,
            facts,
            storage,
        ));
    }

    fn bind_entry_location_i32_bool01_with_aliases(
        &mut self,
        location: LocalLocation,
        name: &str,
        aliases: Vec<String>,
        value: ir::Value,
        storage: LocalEnvStorage,
    ) {
        debug_assert!(
            self.entry_index_for_location(location).is_none(),
            "block-entry LocalEnv location should be bound once"
        );
        self.entries.push(LocalEnvEntry::i32_bool01(
            Some(location),
            name.to_string(),
            aliases,
            value,
            storage,
        ));
    }

    fn entry_index_for_location(&self, location: LocalLocation) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.location == Some(location))
    }

    fn entry_index_for_name(&self, name: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.name == name || entry.aliases.iter().any(|alias| alias == name))
    }

    fn entry_index_for_block_arg_name(&self, name: &str) -> Option<usize> {
        self.entry_index_for_name(name).or_else(|| {
            if !is_try_exception_alias_name(name) {
                return None;
            }
            let mut matches = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| is_try_exception_alias_name(entry.name.as_str()));
            let first = matches.next().map(|(index, _)| index);
            debug_assert!(
                matches.next().is_none(),
                "expected at most one current-exception LocalEnv entry"
            );
            first
        })
    }

    fn py_facts_for_load(&self, name: &ResolvedName) -> Option<PyObjFacts> {
        name.local_location()
            .and_then(|location| {
                self.entry_index_for_location(location)
                    .or_else(|| self.entry_index_for_name(name.id.as_str()))
            })
            .or_else(|| self.entry_index_for_name(name.id.as_str()))
            .and_then(|index| self.entries[index].py_facts())
    }

    fn i64_facts_for_load(&self, name: &ResolvedName) -> Option<IntFacts> {
        name.local_location()
            .and_then(|location| {
                self.entry_index_for_location(location)
                    .or_else(|| self.entry_index_for_name(name.id.as_str()))
            })
            .or_else(|| self.entry_index_for_name(name.id.as_str()))
            .and_then(|index| self.entries[index].i64_facts())
    }

    fn scalar_i64_value_for_load(&self, name: &ResolvedName) -> Option<(ir::Value, IntFacts)> {
        name.local_location()
            .and_then(|location| {
                self.entry_index_for_location(location)
                    .or_else(|| self.entry_index_for_name(name.id.as_str()))
            })
            .or_else(|| self.entry_index_for_name(name.id.as_str()))
            .and_then(|index| {
                self.entries[index]
                    .i64_facts()
                    .map(|facts| (self.entries[index].value(), facts))
            })
    }

    fn scalar_i32_bool01_value_for_load(
        &self,
        name: &ResolvedName,
    ) -> Option<(ir::Value, IntFacts)> {
        name.local_location()
            .and_then(|location| {
                self.entry_index_for_location(location)
                    .or_else(|| self.entry_index_for_name(name.id.as_str()))
            })
            .or_else(|| self.entry_index_for_name(name.id.as_str()))
            .and_then(|index| {
                self.entries[index]
                    .i32_bool01_facts()
                    .map(|facts| (self.entries[index].value(), facts))
            })
    }

    fn scalar_i64_value_for_name(&self, name: &str) -> Option<(ir::Value, IntFacts)> {
        self.entry_index_for_name(name).and_then(|index| {
            self.entries[index]
                .i64_facts()
                .map(|facts| (self.entries[index].value(), facts))
        })
    }

    fn load_location(
        &self,
        fb: &mut FunctionBuilder<'_>,
        location: LocalLocation,
        name: &str,
        ctx: &JitEmitCtx<'_>,
        borrowed: bool,
    ) -> Option<ir::Value> {
        if let Some(index) = self
            .entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
        {
            let entry = &self.entries[index];
            if let Some(facts) = entry.i64_facts() {
                debug_assert!(
                    !borrowed,
                    "scalar local cannot be loaded as a borrowed PyObject"
                );
                let result = emit_soac_value_result_for_demand(
                    fb,
                    SoacValue::i64(entry.value(), facts),
                    ctx,
                    ResultDemand::PYOBJECT_OWNED,
                    None,
                );
                let (value, ownership, _) = result.expect_pyobject("scalar local materialization");
                debug_assert!(
                    ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal)
                );
                return Some(value);
            }
            if let Some(facts) = entry.i32_bool01_facts() {
                let result = emit_soac_value_result_for_demand(
                    fb,
                    SoacValue::i32(entry.value(), facts),
                    ctx,
                    if borrowed {
                        ResultDemand::PYOBJECT_BORROWED_OK
                    } else {
                        ResultDemand::PYOBJECT_OWNED
                    },
                    None,
                );
                let (value, ownership, _) = result.expect_pyobject("scalar bool materialization");
                debug_assert!(
                    ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal)
                );
                return Some(value);
            }
            let value = entry.value();
            if entry.binding_facts.requires_checked_local_load()
                || entry.ref_kind() == LocalRefKind::Unbound
            {
                return Some(emit_checked_local_value_or_unbound(
                    fb,
                    name,
                    value,
                    entry.ref_kind(),
                    ctx,
                    borrowed,
                ));
            }
            if local_ref_kind_needs_incref_for_load(entry.ref_kind(), borrowed) {
                ctx.emit_incref_for_family(
                    fb,
                    value,
                    entry.py_facts(),
                    RefcountFamily::LocalLoadClone,
                );
            }
            return Some(value);
        }
        None
    }

    fn load_name(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        ctx: &JitEmitCtx<'_>,
        borrowed: bool,
    ) -> Option<ir::Value> {
        if let Some(index) = self.entry_index_for_name(name) {
            let entry = &self.entries[index];
            if let Some(facts) = entry.i64_facts() {
                debug_assert!(
                    !borrowed,
                    "scalar local cannot be loaded as a borrowed PyObject"
                );
                let result = emit_soac_value_result_for_demand(
                    fb,
                    SoacValue::i64(entry.value(), facts),
                    ctx,
                    ResultDemand::PYOBJECT_OWNED,
                    None,
                );
                let (value, ownership, _) = result.expect_pyobject("scalar local materialization");
                debug_assert!(
                    ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal)
                );
                return Some(value);
            }
            if let Some(facts) = entry.i32_bool01_facts() {
                let result = emit_soac_value_result_for_demand(
                    fb,
                    SoacValue::i32(entry.value(), facts),
                    ctx,
                    if borrowed {
                        ResultDemand::PYOBJECT_BORROWED_OK
                    } else {
                        ResultDemand::PYOBJECT_OWNED
                    },
                    None,
                );
                let (value, ownership, _) = result.expect_pyobject("scalar bool materialization");
                debug_assert!(
                    ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal)
                );
                return Some(value);
            }
            let value = entry.value();
            if entry.binding_facts.requires_checked_local_load()
                || entry.ref_kind() == LocalRefKind::Unbound
            {
                return Some(emit_checked_local_value_or_unbound(
                    fb,
                    name,
                    value,
                    entry.ref_kind(),
                    ctx,
                    borrowed,
                ));
            }
            if local_ref_kind_needs_incref_for_load(entry.ref_kind(), borrowed) {
                ctx.emit_incref_for_family(
                    fb,
                    value,
                    entry.py_facts(),
                    RefcountFamily::LocalLoadClone,
                );
            }
            return Some(value);
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn store_i64_location(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        location: LocalLocation,
        name: &str,
        value: ir::Value,
        facts: IntFacts,
        cleanup_root_previous_state: CleanupRootSlotState,
        cleanup_root_previous_facts: Option<PyObjFacts>,
        stack_slots: &StackSlots,
        refcount_location_counters: Option<RefcountDecrefLocationCounterParts<'_>>,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
    ) {
        let previous_entry = if let Some(existing_index) = self
            .entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
        {
            Some(self.entries.remove(existing_index))
        } else {
            None
        };
        let previous_had_stack_mirror = previous_entry
            .as_ref()
            .is_some_and(|entry| entry.storage == LocalEnvStorage::StackMirror);
        if stack_slots.has_name(name) && (previous_entry.is_none() || previous_had_stack_mirror) {
            let previous_state = if stack_slots.has_cleanup_root_name(name) {
                cleanup_root_previous_state
            } else {
                CleanupRootSlotState::MaybeOwnedReference
            };
            if previous_state.may_hold_owned_reference() {
                stack_slots
                    .clear_value_with_previous_state_counted(
                        fb,
                        name,
                        previous_state,
                        ptr_ty,
                        thread_state_value,
                        decref_ref,
                        refcounts.with_family(RefcountFamily::LocalOverwrite),
                        previous_entry
                            .as_ref()
                            .and_then(LocalEnvEntry::py_facts)
                            .or(cleanup_root_previous_facts),
                        refcount_location_counters,
                    )
                    .expect("slot-backed scalar local missing from stack slots");
            }
        }
        self.entries.push(LocalEnvEntry::exact_i64(
            Some(location),
            name.to_string(),
            previous_entry
                .as_ref()
                .map(|entry| entry.aliases.clone())
                .unwrap_or_default(),
            value,
            facts,
            LocalEnvStorage::LocalOnly,
        ));
        if let Some(previous) = previous_entry
            && previous.storage == LocalEnvStorage::LocalOnly
            && transient_local_needs_decref(previous.ref_kind())
        {
            emit_decref_via_lowering(
                fb,
                refcounts.with_family(RefcountFamily::LocalOverwrite),
                previous.value(),
                previous.py_facts(),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn store_i32_bool01_location(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        location: LocalLocation,
        name: &str,
        value: ir::Value,
        cleanup_root_previous_state: CleanupRootSlotState,
        cleanup_root_previous_facts: Option<PyObjFacts>,
        stack_slots: &StackSlots,
        refcount_location_counters: Option<RefcountDecrefLocationCounterParts<'_>>,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
    ) {
        let previous_entry = if let Some(existing_index) = self
            .entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
        {
            Some(self.entries.remove(existing_index))
        } else {
            None
        };
        let previous_had_stack_mirror = previous_entry
            .as_ref()
            .is_some_and(|entry| entry.storage == LocalEnvStorage::StackMirror);
        if stack_slots.has_name(name) && (previous_entry.is_none() || previous_had_stack_mirror) {
            let previous_state = if stack_slots.has_cleanup_root_name(name) {
                cleanup_root_previous_state
            } else {
                CleanupRootSlotState::MaybeOwnedReference
            };
            if previous_state.may_hold_owned_reference() {
                stack_slots
                    .clear_value_with_previous_state_counted(
                        fb,
                        name,
                        previous_state,
                        ptr_ty,
                        thread_state_value,
                        decref_ref,
                        refcounts.with_family(RefcountFamily::LocalOverwrite),
                        previous_entry
                            .as_ref()
                            .and_then(LocalEnvEntry::py_facts)
                            .or(cleanup_root_previous_facts),
                        refcount_location_counters,
                    )
                    .expect("slot-backed scalar bool local missing from stack slots");
            }
        }
        self.entries.push(LocalEnvEntry::i32_bool01(
            Some(location),
            name.to_string(),
            previous_entry
                .as_ref()
                .map(|entry| entry.aliases.clone())
                .unwrap_or_default(),
            value,
            LocalEnvStorage::LocalOnly,
        ));
        if let Some(previous) = previous_entry
            && previous.storage == LocalEnvStorage::LocalOnly
            && transient_local_needs_decref(previous.ref_kind())
        {
            emit_decref_via_lowering(
                fb,
                refcounts.with_family(RefcountFamily::LocalOverwrite),
                previous.value(),
                previous.py_facts(),
            );
        }
    }

    fn store_location(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        location: LocalLocation,
        name: &str,
        value: ir::Value,
        value_ref_kind: LocalRefKind,
        py_facts: Option<PyObjFacts>,
        allow_local_only_slot_backed_store: bool,
        cleanup_root_previous_state: CleanupRootSlotState,
        cleanup_root_previous_facts: Option<PyObjFacts>,
        stack_slots: &StackSlots,
        refcount_location_counters: Option<RefcountDecrefLocationCounterParts<'_>>,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
    ) {
        let previous_entry = if let Some(existing_index) = self
            .entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
        {
            Some(self.entries.remove(existing_index))
        } else {
            None
        };
        let is_cleanup_root = stack_slots.has_cleanup_root_name(name);
        let should_mirror_stack_slot = stack_slots.has_name(name)
            && if is_cleanup_root {
                transient_local_needs_decref(value_ref_kind)
            } else {
                match previous_entry.as_ref().map(|entry| entry.storage) {
                    Some(LocalEnvStorage::LocalOnly) => false,
                    Some(LocalEnvStorage::StackMirror) => true,
                    None => !allow_local_only_slot_backed_store,
                }
            };
        if should_mirror_stack_slot {
            if is_cleanup_root {
                stack_slots
                    .replace_transferred_value_with_previous_state_counted(
                        fb,
                        name,
                        value,
                        value_ref_kind,
                        cleanup_root_previous_state,
                        ptr_ty,
                        thread_state_value,
                        incref_ref,
                        decref_ref,
                        refcounts.with_family(RefcountFamily::LocalOverwrite),
                        previous_entry
                            .as_ref()
                            .and_then(LocalEnvEntry::py_facts)
                            .or(cleanup_root_previous_facts),
                        refcount_location_counters,
                    )
                    .expect("cleanup-root local missing from stack slots");
            } else {
                stack_slots
                    .replace_cloned_value_counted(
                        fb,
                        name,
                        value,
                        value_ref_kind,
                        ptr_ty,
                        thread_state_value,
                        incref_ref,
                        decref_ref,
                        refcounts.with_family(RefcountFamily::LocalOverwrite),
                        previous_entry.as_ref().and_then(LocalEnvEntry::py_facts),
                        refcount_location_counters,
                    )
                    .expect("slot-backed local missing from stack slots");
                if local_ref_kind_needs_refcount_call(value_ref_kind) {
                    emit_decref_via_lowering(
                        fb,
                        refcounts.with_family(RefcountFamily::LocalOverwrite),
                        value,
                        py_facts,
                    );
                }
            }
            self.entries.push(LocalEnvEntry::pyobject(
                Some(
                    previous_entry
                        .as_ref()
                        .and_then(|entry| entry.location)
                        .unwrap_or(location),
                ),
                name.to_string(),
                previous_entry
                    .as_ref()
                    .map(|entry| entry.aliases.clone())
                    .unwrap_or_default(),
                value,
                local_ref_kind_for_stack_mirror(value_ref_kind),
                LocalEnvStorage::StackMirror,
                local_binding_facts_for_stored_value(value_ref_kind),
                py_facts,
            ));
        } else {
            if stack_slots.has_name(name) {
                let previous_state = if is_cleanup_root {
                    cleanup_root_previous_state
                } else {
                    CleanupRootSlotState::MaybeOwnedReference
                };
                let should_clear_stack_slot = if is_cleanup_root {
                    previous_state.may_hold_owned_reference()
                } else {
                    previous_entry.is_none()
                };
                if should_clear_stack_slot {
                    stack_slots
                        .clear_value_with_previous_state_counted(
                            fb,
                            name,
                            previous_state,
                            ptr_ty,
                            thread_state_value,
                            decref_ref,
                            refcounts.with_family(RefcountFamily::LocalOverwrite),
                            previous_entry
                                .as_ref()
                                .and_then(LocalEnvEntry::py_facts)
                                .or(cleanup_root_previous_facts),
                            refcount_location_counters,
                        )
                        .expect("slot-backed local missing from stack slots");
                }
            }
            self.entries.push(LocalEnvEntry::pyobject(
                Some(location),
                name.to_string(),
                previous_entry
                    .as_ref()
                    .map(|entry| entry.aliases.clone())
                    .unwrap_or_default(),
                value,
                value_ref_kind,
                LocalEnvStorage::LocalOnly,
                local_binding_facts_for_stored_value(value_ref_kind),
                py_facts,
            ));
        }
        if let Some(previous) = previous_entry {
            if transient_local_needs_decref(previous.ref_kind()) {
                emit_decref_via_lowering(
                    fb,
                    refcounts.with_family(RefcountFamily::LocalOverwrite),
                    previous.value(),
                    previous.py_facts(),
                );
            }
        }
    }

    fn store_name(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        value_ref_kind: LocalRefKind,
        py_facts: Option<PyObjFacts>,
        _ptr_ty: ir::Type,
        _thread_state_value: ir::Value,
        _decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
    ) {
        let previous_entry = self
            .entry_index_for_name(name)
            .map(|existing_index| self.entries.remove(existing_index));
        self.entries.push(LocalEnvEntry::pyobject(
            None,
            name.to_string(),
            Vec::new(),
            value,
            value_ref_kind,
            LocalEnvStorage::LocalOnly,
            local_binding_facts_for_stored_value(value_ref_kind),
            py_facts,
        ));
        if let Some(previous) = previous_entry {
            if transient_local_needs_decref(previous.ref_kind()) {
                emit_decref_via_lowering(
                    fb,
                    refcounts.with_family(RefcountFamily::LocalOverwrite),
                    previous.value(),
                    previous.py_facts(),
                );
            }
        }
    }

    fn delete_location(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        location: LocalLocation,
        name: &str,
        stack_slots: &StackSlots,
        cleanup_root_previous_state: CleanupRootSlotState,
        cleanup_root_previous_facts: Option<PyObjFacts>,
        refcount_location_counters: Option<RefcountDecrefLocationCounterParts<'_>>,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
    ) -> Result<(), String> {
        let had_stack_slot = stack_slots.has_name(name);
        let removed_entry = if let Some(index) = self
            .entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
        {
            let previous = self.entries.remove(index);
            if transient_local_needs_decref(previous.ref_kind()) {
                emit_decref_via_lowering(
                    fb,
                    refcounts.with_family(RefcountFamily::ExplicitDelete),
                    previous.value(),
                    previous.py_facts(),
                );
            }
            Some(previous)
        } else {
            None
        };
        let should_clear_stack_slot = removed_entry
            .as_ref()
            .map(|entry| entry.storage == LocalEnvStorage::StackMirror)
            .unwrap_or(had_stack_slot);
        if should_clear_stack_slot {
            stack_slots
                .clear_value_with_previous_state_counted(
                    fb,
                    name,
                    if stack_slots.has_cleanup_root_name(name) {
                        cleanup_root_previous_state
                    } else {
                        CleanupRootSlotState::MaybeOwnedReference
                    },
                    ptr_ty,
                    thread_state_value,
                    decref_ref,
                    refcounts.with_family(RefcountFamily::ExplicitDelete),
                    removed_entry
                        .as_ref()
                        .and_then(LocalEnvEntry::py_facts)
                        .or(cleanup_root_previous_facts),
                    refcount_location_counters,
                )
                .expect("slot-backed delete target missing from stack slots");
        }
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let unbound_storage = if should_clear_stack_slot {
            LocalEnvStorage::StackMirror
        } else {
            LocalEnvStorage::LocalOnly
        };
        self.entries.push(LocalEnvEntry::new(
            removed_entry
                .as_ref()
                .and_then(|entry| entry.location)
                .or(Some(location)),
            name.to_string(),
            removed_entry
                .as_ref()
                .map(|entry| entry.aliases.clone())
                .unwrap_or_default(),
            LocalBindingValue::unbound(null_ptr),
            unbound_storage,
            local_binding_facts_for_stored_value(LocalRefKind::Unbound),
        ));
        Ok(())
    }

    fn move_location_to_location(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        source_location: LocalLocation,
        source_name: &str,
        target_location: LocalLocation,
        target_name: &str,
        py_facts: Option<PyObjFacts>,
        allow_local_only_slot_backed_store: bool,
        source_cleanup_root_previous_state: CleanupRootSlotState,
        target_cleanup_root_previous_state: CleanupRootSlotState,
        target_cleanup_root_previous_facts: Option<PyObjFacts>,
        stack_slots: &StackSlots,
        refcount_location_counters: Option<RefcountDecrefLocationCounterParts<'_>>,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
    ) -> bool {
        if source_location == target_location || source_name == target_name {
            return false;
        }
        let Some(source_index) = self
            .entry_index_for_location(source_location)
            .or_else(|| self.entry_index_for_name(source_name))
        else {
            return false;
        };
        if self
            .entry_index_for_location(target_location)
            .or_else(|| self.entry_index_for_name(target_name))
            == Some(source_index)
        {
            return false;
        }
        let source_entry = &self.entries[source_index];
        if source_entry.binding_facts.requires_checked_local_load() {
            return false;
        }
        let is_cleanup_root = stack_slots.has_cleanup_root_name(target_name);
        let target_index = self
            .entry_index_for_location(target_location)
            .or_else(|| self.entry_index_for_name(target_name));
        let target_storage = target_index.map(|index| self.entries[index].storage);
        let should_mirror_stack_slot = stack_slots.has_name(target_name)
            && (is_cleanup_root
                || match target_storage {
                    Some(LocalEnvStorage::LocalOnly) => false,
                    Some(LocalEnvStorage::StackMirror) => true,
                    None => !allow_local_only_slot_backed_store,
                });
        let should_clear_source_stack_slot = match source_entry.storage {
            LocalEnvStorage::LocalOnly => {
                if !transient_local_needs_decref(source_entry.ref_kind()) {
                    return false;
                }
                false
            }
            LocalEnvStorage::StackMirror => {
                if !should_mirror_stack_slot
                    || source_entry.ref_kind() != LocalRefKind::Borrowed
                    || !source_cleanup_root_previous_state.may_hold_owned_reference()
                    || !stack_slots.has_name(source_name)
                {
                    return false;
                }
                true
            }
        };

        let source_entry = self.entries.remove(source_index);
        let previous_entry = self
            .entry_index_for_location(target_location)
            .or_else(|| self.entry_index_for_name(target_name))
            .map(|index| self.entries.remove(index));
        let target_storage = if should_mirror_stack_slot {
            stack_slots
                .replace_moved_owned_value_with_previous_state_counted(
                    fb,
                    target_name,
                    source_entry.value(),
                    target_cleanup_root_previous_state,
                    ptr_ty,
                    thread_state_value,
                    decref_ref,
                    refcounts.with_family(RefcountFamily::LocalOverwrite),
                    previous_entry
                        .as_ref()
                        .and_then(LocalEnvEntry::py_facts)
                        .or(target_cleanup_root_previous_facts),
                    refcount_location_counters,
                )
                .expect("moved generated-temp target missing from stack slots");
            LocalEnvStorage::StackMirror
        } else {
            LocalEnvStorage::LocalOnly
        };
        if should_clear_source_stack_slot {
            stack_slots
                .clear_moved_value(fb, source_name, ptr_ty)
                .expect("moved generated-temp source missing from stack slots");
        }
        let target_ref_kind = match target_storage {
            LocalEnvStorage::LocalOnly => source_entry.ref_kind(),
            LocalEnvStorage::StackMirror => local_ref_kind_for_stack_mirror(LocalRefKind::Owned),
        };
        let target_binding_ref_kind = match target_storage {
            LocalEnvStorage::LocalOnly => source_entry.ref_kind(),
            LocalEnvStorage::StackMirror => LocalRefKind::Owned,
        };
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        self.entries.push(LocalEnvEntry::pyobject(
            Some(target_location),
            target_name.to_string(),
            previous_entry
                .as_ref()
                .map(|entry| entry.aliases.clone())
                .unwrap_or_default(),
            source_entry.value(),
            target_ref_kind,
            target_storage,
            local_binding_facts_for_stored_value(target_binding_ref_kind),
            py_facts,
        ));
        self.entries.push(LocalEnvEntry::new(
            Some(source_location),
            source_name.to_string(),
            source_entry.aliases,
            LocalBindingValue::unbound(null_ptr),
            source_entry.storage,
            local_binding_facts_for_stored_value(LocalRefKind::Unbound),
        ));
        if let Some(previous) = previous_entry {
            if transient_local_needs_decref(previous.ref_kind()) {
                emit_decref_via_lowering(
                    fb,
                    refcounts.with_family(RefcountFamily::LocalOverwrite),
                    previous.value(),
                    previous.py_facts(),
                );
            }
        }
        true
    }

    fn remove_location_or_name(
        &mut self,
        location: LocalLocation,
        name: &str,
    ) -> Option<LocalEnvEntry> {
        self.entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
            .map(|index| self.entries.remove(index))
    }

    fn can_transfer_owned_local_only_location(&self, location: LocalLocation, name: &str) -> bool {
        self.entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
            .map(|index| &self.entries[index])
            .is_some_and(|entry| {
                entry.storage == LocalEnvStorage::LocalOnly
                    && entry.ref_kind() == LocalRefKind::Owned
                    && !entry.binding_facts.requires_checked_local_load()
            })
    }

    fn mark_owned_local_only_location_transferred(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        location: LocalLocation,
        name: &str,
        ptr_ty: ir::Type,
    ) -> bool {
        if !self.can_transfer_owned_local_only_location(location, name) {
            return false;
        }
        let Some(entry) = self.remove_location_or_name(location, name) else {
            return false;
        };
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        self.entries.push(LocalEnvEntry::new(
            Some(location),
            name.to_string(),
            entry.aliases,
            LocalBindingValue::unbound(null_ptr),
            LocalEnvStorage::LocalOnly,
            local_binding_facts_for_stored_value(LocalRefKind::Unbound),
        ));
        true
    }

    #[cfg(test)]
    fn local_only_cleanup_values(&self) -> Vec<ir::Value> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.storage == LocalEnvStorage::LocalOnly
                    && transient_local_needs_decref(entry.ref_kind())
            })
            .map(|entry| entry.value())
            .collect()
    }

    fn local_only_cleanup_entries_excluding(
        &self,
        forwarded_locations: &HashSet<LocalLocation>,
    ) -> Vec<LocalFailureCleanupValue> {
        self.entries
            .iter()
            .filter(|entry| {
                !entry
                    .location
                    .is_some_and(|location| forwarded_locations.contains(&location))
                    && entry.storage == LocalEnvStorage::LocalOnly
                    && transient_local_needs_decref(entry.ref_kind())
            })
            .map(LocalFailureCleanupValue::from_local_env_entry)
            .collect()
    }

    #[cfg(debug_assertions)]
    fn transient_semantic_cleanup_names_excluding(
        &self,
        forwarded_locations: &HashSet<LocalLocation>,
        preserved_values: &[ir::Value],
    ) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.location.is_some()
                    && !entry
                        .location
                        .is_some_and(|location| forwarded_locations.contains(&location))
                    && !preserved_values.contains(&entry.value())
                    && transient_local_needs_decref(entry.ref_kind())
            })
            .map(|entry| entry.name.clone())
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn bind_planned_local_env_at_block_entry(
    fb: &mut FunctionBuilder<'_>,
    jit_local_plan: &PlannedJitFunctionLocals,
    block_index: usize,
    block_param_values: &[ir::Value],
    local_env: &mut LocalEnv,
    stack_slots: &StackSlots,
    refcount_location_counters: Option<RefcountDecrefLocationCounterParts<'_>>,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
    refcounts: RefcountEmitter,
    propagate_entry_py_facts: bool,
) -> Result<(), String> {
    for entry in &jit_local_plan.entry_materializations[block_index] {
        let binding = &entry.binding;
        let entry_py_facts = local_env_entry_py_facts_for_materialization(
            entry,
            stack_slots.has_cleanup_root_name(binding.name.as_str()),
            propagate_entry_py_facts,
        );
        match entry.source {
            PlannedLocalEnvEntrySource::BlockParam { param_index } => {
                let param_value =
                    block_param_values
                        .get(param_index)
                        .copied()
                        .ok_or_else(|| {
                            format!(
                                "planned LocalEnv block param {} for {} is missing runtime value",
                                param_index, binding.name
                            )
                        })?;
                match entry.repr {
                    RuntimeBlockParamRepr::ExactI64 => {
                        debug_assert_eq!(binding.storage, PlannedLocalStorage::BlockParam);
                        local_env.bind_entry_location_i64_with_aliases(
                            binding.location,
                            binding.name.as_str(),
                            entry.entry_aliases.clone(),
                            param_value,
                            IntFacts::i64_unknown(),
                            LocalEnvStorage::LocalOnly,
                        );
                        continue;
                    }
                    RuntimeBlockParamRepr::I32Bool01 => {
                        debug_assert_eq!(binding.storage, PlannedLocalStorage::BlockParam);
                        local_env.bind_entry_location_i32_bool01_with_aliases(
                            binding.location,
                            binding.name.as_str(),
                            entry.entry_aliases.clone(),
                            param_value,
                            LocalEnvStorage::LocalOnly,
                        );
                        continue;
                    }
                    RuntimeBlockParamRepr::PyObject => {}
                }
                let entry_storage = if binding.storage == PlannedLocalStorage::StackSlot
                    || stack_slots.has_cleanup_root_name(binding.name.as_str())
                {
                    LocalEnvStorage::StackMirror
                } else {
                    LocalEnvStorage::LocalOnly
                };
                local_env.bind_entry_location_with_aliases(
                    binding.location,
                    binding.name.as_str(),
                    entry.entry_aliases.clone(),
                    param_value,
                    entry.entry_ref_kind,
                    entry_storage,
                    binding.param_facts.binding,
                    entry_py_facts,
                );
                if binding.storage == PlannedLocalStorage::StackSlot {
                    stack_slots
                        .replace_cloned_value_counted(
                            fb,
                            binding.name.as_str(),
                            param_value,
                            entry.entry_ref_kind,
                            ptr_ty,
                            thread_state_value,
                            incref_ref,
                            decref_ref,
                            refcounts,
                            None,
                            refcount_location_counters,
                        )
                        .expect("runtime block param missing from stack slots");
                    if local_ref_kind_needs_refcount_call(entry.entry_ref_kind) {
                        emit_decref_if_not_null(
                            fb,
                            ptr_ty,
                            decref_ref,
                            thread_state_value,
                            param_value,
                        );
                    }
                }
            }
            PlannedLocalEnvEntrySource::StackSlotLoad => {
                if local_env
                    .entry_index_for_location(binding.location)
                    .or_else(|| local_env.entry_index_for_name(binding.name.as_str()))
                    .is_some()
                {
                    continue;
                }
                let slot = stack_slots
                    .slot_for_block_arg_name(binding.name.as_str())
                    .ok_or_else(|| {
                        format!(
                            "planned stack-slot entry binding for {} is missing stack storage",
                            binding.name
                        )
                    })?;
                let value = fb.ins().stack_load(ptr_ty, slot, 0);
                local_env.bind_entry_location_with_aliases(
                    binding.location,
                    binding.name.as_str(),
                    entry.entry_aliases.clone(),
                    value,
                    entry.entry_ref_kind,
                    LocalEnvStorage::StackMirror,
                    binding.param_facts.binding,
                    entry_py_facts,
                );
            }
        }
    }
    Ok(())
}

fn local_env_entry_py_facts_for_materialization(
    entry: &PlannedLocalEnvEntryMaterialization,
    is_cleanup_root: bool,
    propagate_entry_py_facts: bool,
) -> Option<PyObjFacts> {
    let is_cleanup_root_block_param =
        matches!(entry.source, PlannedLocalEnvEntrySource::BlockParam { .. }) && is_cleanup_root;
    if propagate_entry_py_facts && !is_cleanup_root_block_param {
        entry.binding.param_facts.value.map(|facts| {
            if entry
                .binding
                .param_facts
                .binding
                .requires_checked_local_load()
            {
                facts.without_non_null_ref()
            } else {
                facts
            }
        })
    } else {
        None
    }
}

fn transient_local_needs_decref(ref_kind: LocalRefKind) -> bool {
    match ref_kind {
        LocalRefKind::Owned | LocalRefKind::Unknown => true,
        LocalRefKind::Borrowed | LocalRefKind::Immortal | LocalRefKind::Unbound => false,
    }
}

fn local_ref_kind_needs_incref_for_load(ref_kind: LocalRefKind, borrowed: bool) -> bool {
    !borrowed && local_ref_kind_needs_refcount_call(ref_kind)
}

fn local_ref_kind_needs_refcount_call(ref_kind: LocalRefKind) -> bool {
    !matches!(ref_kind, LocalRefKind::Immortal)
}

fn local_ref_kind_needs_incref_for_stack_slot_transfer(ref_kind: LocalRefKind) -> bool {
    matches!(ref_kind, LocalRefKind::Borrowed)
}

fn local_ref_kind_needs_incref_for_forward(ref_kind: LocalRefKind, forwarded_count: usize) -> bool {
    match ref_kind {
        LocalRefKind::Owned | LocalRefKind::Unknown => forwarded_count > 0,
        LocalRefKind::Borrowed | LocalRefKind::Unbound => true,
        LocalRefKind::Immortal => false,
    }
}

fn local_env_entry_needs_incref_for_forward(
    entry: &LocalEnvEntry,
    forwarded_count: usize,
    stack_slots: &StackSlots,
) -> bool {
    if !entry.is_pyobject_binding() {
        return false;
    }
    if entry.storage == LocalEnvStorage::StackMirror
        && stack_slots.has_cleanup_root_name(entry.name.as_str())
    {
        return false;
    }
    local_ref_kind_needs_incref_for_forward(entry.ref_kind(), forwarded_count)
}

fn emit_local_env_entry_pyobject_for_forward(
    fb: &mut FunctionBuilder<'_>,
    entry: &LocalEnvEntry,
    ctx: &JitEmitCtx<'_>,
    forwarded_count: usize,
) -> ir::Value {
    if let Some(facts) = entry.i64_facts() {
        let result = emit_soac_value_result_for_demand(
            fb,
            SoacValue::i64(entry.value(), facts),
            ctx,
            ResultDemand::PYOBJECT_OWNED,
            None,
        );
        let (value, ownership, _) = result.expect_pyobject("forwarded scalar local");
        debug_assert!(ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal));
        return value;
    }
    if let Some(facts) = entry.i32_bool01_facts() {
        let result = emit_soac_value_result_for_demand(
            fb,
            SoacValue::i32(entry.value(), facts),
            ctx,
            ResultDemand::PYOBJECT_OWNED,
            None,
        );
        let (value, ownership, _) = result.expect_pyobject("forwarded scalar bool local");
        debug_assert!(ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal));
        return value;
    }
    let value = entry.value();
    if local_env_entry_needs_incref_for_forward(entry, forwarded_count, &ctx.stack_slots) {
        ctx.emit_incref_for_family(
            fb,
            value,
            entry.py_facts(),
            RefcountFamily::ForwardedValueClone,
        );
    }
    value
}

fn emit_local_env_entry_pyobject_for_frame_root_transfer(
    fb: &mut FunctionBuilder<'_>,
    entry: &LocalEnvEntry,
    ctx: &JitEmitCtx<'_>,
) -> (ir::Value, LocalRefKind) {
    if let Some(facts) = entry.i64_facts() {
        let result = emit_soac_value_result_for_demand(
            fb,
            SoacValue::i64(entry.value(), facts),
            ctx,
            ResultDemand::PYOBJECT_OWNED,
            None,
        );
        let (value, ownership, _) = result.expect_pyobject("scalar cleanup-root materialization");
        debug_assert!(ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal));
        let ref_kind = if matches!(ownership, ValueOwnership::Immortal) {
            LocalRefKind::Immortal
        } else {
            LocalRefKind::Owned
        };
        return (value, ref_kind);
    }
    if let Some(facts) = entry.i32_bool01_facts() {
        let result = emit_soac_value_result_for_demand(
            fb,
            SoacValue::i32(entry.value(), facts),
            ctx,
            ResultDemand::PYOBJECT_OWNED,
            None,
        );
        let (value, ownership, _) =
            result.expect_pyobject("scalar bool cleanup-root materialization");
        debug_assert!(ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal));
        let ref_kind = if matches!(ownership, ValueOwnership::Immortal) {
            LocalRefKind::Immortal
        } else {
            LocalRefKind::Owned
        };
        return (value, ref_kind);
    }
    (entry.value(), entry.ref_kind())
}

enum PlannedLocalStoreEffect {
    Rebind(LocalRefKind),
    Delete,
}

fn planned_cleanup_root_previous_state_for_key(
    instr_key: InstrKey,
    name: &str,
    ctx: &JitEmitCtx<'_>,
) -> CleanupRootSlotState {
    if ctx.stack_slots.has_cleanup_root_name(name) {
        ctx.cleanup_root_slot_states
            .previous_state_for_instr(instr_key, name)
    } else {
        CleanupRootSlotState::MaybeOwnedReference
    }
}

fn planned_cleanup_root_previous_facts_for_key(
    instr_key: InstrKey,
    name: &str,
    ctx: &JitEmitCtx<'_>,
) -> Option<PyObjFacts> {
    ctx.stack_slots
        .has_cleanup_root_name(name)
        .then(|| {
            ctx.cleanup_root_slot_states
                .previous_facts_for_instr(instr_key, name)
        })
        .flatten()
}

fn cleanup_root_state_key(states: &HashMap<String, CleanupRootSlotState>) -> Vec<String> {
    let mut key = states
        .iter()
        .filter_map(|(name, state)| state.may_hold_owned_reference().then_some(name.clone()))
        .collect::<Vec<_>>();
    key.sort();
    key
}

fn local_ref_kind_for_planned_local_state(state: LocalRefState) -> LocalRefKind {
    match state {
        LocalRefState::Unbound => LocalRefKind::Unbound,
        LocalRefState::Borrowed => LocalRefKind::Borrowed,
        LocalRefState::Owned => LocalRefKind::Owned,
        LocalRefState::Immortal => LocalRefKind::Immortal,
    }
}

fn planned_local_store_effect(
    expr: &InstrBlockPy,
    location: LocalLocation,
    ctx: &JitEmitCtx<'_>,
) -> Option<PlannedLocalStoreEffect> {
    planned_local_store_effect_for_key(expr.semantic_instr_key(ctx.function_id), location, ctx)
}

fn planned_typed_local_store_effect(
    expr: &InstrTyped,
    location: LocalLocation,
    ctx: &JitEmitCtx<'_>,
) -> Option<PlannedLocalStoreEffect> {
    planned_local_store_effect_for_key(expr.semantic_instr_key(ctx.function_id), location, ctx)
}

fn planned_local_store_effect_for_key(
    instr_key: InstrKey,
    location: LocalLocation,
    ctx: &JitEmitCtx<'_>,
) -> Option<PlannedLocalStoreEffect> {
    let block_label = ctx
        .instr_locations
        .get(&instr_key.instr_id)
        .map(|location| location.block_label())?;
    let block_plan = ctx.refcount_plan.block(block_label)?;
    for action in &block_plan.actions {
        let RefcountSite::Instr(site_key) = action.site else {
            continue;
        };
        if site_key != instr_key {
            continue;
        }
        match &action.kind {
            RefcountActionKind::RebindLocal {
                local, new_state, ..
            } if local.location == location => {
                return Some(PlannedLocalStoreEffect::Rebind(
                    local_ref_kind_for_planned_local_state(*new_state),
                ));
            }
            RefcountActionKind::DeleteLocal { local, .. } if local.location == location => {
                return Some(PlannedLocalStoreEffect::Delete);
            }
            _ => {}
        }
    }
    None
}

fn local_ref_kind_for_stored_value(value: &InstrBlockPy, ctx: &JitEmitCtx<'_>) -> LocalRefKind {
    match ctx
        .value_facts_for_expr(value)
        .and_then(ValueFacts::as_pyobj)
    {
        Some(facts) if facts.is_immortal() => LocalRefKind::Immortal,
        _ => LocalRefKind::Owned,
    }
}

fn py_facts_for_codegen_expr_with_local_env(
    expr: &InstrBlockPy,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Option<PyObjFacts> {
    if let InstrBlockPy::Load(op) = expr {
        if let Some(py_facts) = local_env.py_facts_for_load(&op.name) {
            return Some(py_facts);
        }
        if op.name.location.as_constant().is_some_and(|index| {
            ctx.module_constants
                .constant_is_int(ModuleConstantId(index as usize))
        }) {
            return Some(PyObjFacts::exact_type(PyExactType::Int));
        }
    }
    ctx.value_facts_for_expr(expr)
        .and_then(ValueFacts::as_pyobj)
}

fn py_facts_for_typed_expr_with_local_env(
    expr: &InstrTyped,
    local_env: &LocalEnv,
) -> Option<PyObjFacts> {
    if let InstrTyped::Load(op) = expr {
        if let Some(py_facts) = local_env.py_facts_for_load(&op.name) {
            return Some(py_facts);
        }
        return op.extra().result_facts().and_then(ValueFacts::as_pyobj);
    }
    expr.result_facts().and_then(ValueFacts::as_pyobj)
}

fn planned_owned_pyobject_result_for_typed_expr(
    expr: &InstrTyped,
    local_env: &LocalEnv,
) -> (ValueOwnership, PyObjFacts) {
    let facts =
        py_facts_for_typed_expr_with_local_env(expr, local_env).unwrap_or_else(PyObjFacts::unknown);
    let ownership = match expr.planned_result() {
        Some(TypedPlannedResult::PyObject {
            ownership: TypedPyObjectOwnershipPlan::Immortal,
        }) => ValueOwnership::Immortal,
        _ if facts.is_immortal() => ValueOwnership::Immortal,
        _ => ValueOwnership::Owned,
    };
    (ownership, facts)
}

fn typed_local_load_direct_result_plan(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    stack_slots: &StackSlots,
    storage_layout: Option<&StorageLayout>,
    demand: ResultDemand,
) -> Option<(ValueOwnership, PyObjFacts)> {
    if !matches!(expr, InstrTyped::Load(op) if op.name.local_location().is_some()) {
        return None;
    }
    if !typed_expr_is_borrowable_from_local_env(expr, local_env, stack_slots, storage_layout) {
        return None;
    }

    let facts =
        py_facts_for_typed_expr_with_local_env(expr, local_env).unwrap_or_else(PyObjFacts::unknown);
    match demand {
        ResultDemand::EffectOnly => Some((ValueOwnership::Borrowed, facts)),
        ResultDemand::PyObject { borrowed_ok } => {
            let ownership = match expr.planned_result() {
                Some(TypedPlannedResult::PyObject {
                    ownership: TypedPyObjectOwnershipPlan::Immortal,
                }) => ValueOwnership::Immortal,
                _ if facts.is_immortal() => ValueOwnership::Immortal,
                Some(TypedPlannedResult::PyObject {
                    ownership: TypedPyObjectOwnershipPlan::BorrowedLocal { location },
                }) if borrowed_ok && typed_expr_local_load_location(expr) == Some(location) => {
                    ValueOwnership::Borrowed
                }
                _ => return None,
            };
            Some((ownership, facts))
        }
        ResultDemand::I32Bool01 | ResultDemand::I64 | ResultDemand::I64Index => None,
    }
}

fn local_ref_kind_for_typed_stored_value(
    value: &InstrTyped,
    ownership: ValueOwnership,
) -> LocalRefKind {
    if matches!(ownership, ValueOwnership::Immortal) {
        return LocalRefKind::Immortal;
    }
    match value.result_facts().and_then(ValueFacts::as_pyobj) {
        Some(facts) if facts.is_immortal() => LocalRefKind::Immortal,
        _ => LocalRefKind::Owned,
    }
}

fn owned_cell_backing_local(
    storage_layout: &StorageLayout,
    slot: u32,
) -> Option<(LocalLocation, &str)> {
    let closure_slot = storage_layout.owned_slot(slot)?;
    let location = storage_layout
        .stack_slots()
        .iter()
        .position(|name| name == &closure_slot.storage_name)
        .map(|index| {
            LocalLocation(
                u32::try_from(index).expect("owned cell backing local index should fit in u32"),
            )
        })?;
    Some((location, closure_slot.storage_name.as_str()))
}

fn local_locations_for_names(
    storage_layout: &StorageLayout,
    names: &[String],
) -> HashSet<LocalLocation> {
    names
        .iter()
        .filter_map(|name| {
            storage_layout
                .stack_slots()
                .iter()
                .position(|candidate| candidate == name)
                .map(|index| {
                    LocalLocation(u32::try_from(index).expect("local slot index should fit in u32"))
                })
        })
        .collect()
}

fn emit_local_store_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrBlockPy,
    op: &Store<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<ir::Value> {
    let result = emit_local_store_result_with_local_env(
        fb,
        expr,
        op,
        local_env,
        emit_ctx,
        ResultDemand::PYOBJECT_OWNED,
        codegen_env,
        func_imports,
    )?;
    let (value, ownership, _) = result.expect_pyobject("legacy local store result");
    assert!(
        ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED),
        "legacy local store result should produce a PyObject that satisfies owned demand"
    );
    Some(value)
}

fn emit_none_for_demand(
    fb: &mut FunctionBuilder<'_>,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    match demand {
        ResultDemand::EffectOnly => EmitResult::no_value(),
        ResultDemand::PyObject { .. } => {
            let none_const = emit_none_const(fb, emit_ctx);
            EmitResult::immortal_pyobject(none_const, PyObjFacts::none_singleton())
        }
        ResultDemand::I32Bool01 => {
            panic!("owned None materialization cannot satisfy I32Bool01 demand")
        }
        ResultDemand::I64 => {
            panic!("owned None materialization cannot satisfy I64 demand")
        }
        ResultDemand::I64Index => {
            panic!("owned None materialization cannot satisfy I64Index demand")
        }
    }
}

fn emit_local_store_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrBlockPy,
    op: &Store<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    if let Some(location) = op.name.local_location() {
        let layout = emit_ctx
            .storage_layout
            .as_ref()
            .expect("Store local slot should have storage layout during codegen");
        let name = local_name_for_location(layout, location);
        if matches!(
            planned_local_store_effect(expr, location, emit_ctx),
            Some(PlannedLocalStoreEffect::Delete)
        ) {
            local_env
                .delete_location(
                    fb,
                    location,
                    name,
                    &emit_ctx.stack_slots,
                    planned_cleanup_root_previous_state_for_key(
                        expr.semantic_instr_key(emit_ctx.function_id),
                        name,
                        emit_ctx,
                    ),
                    planned_cleanup_root_previous_facts_for_key(
                        expr.semantic_instr_key(emit_ctx.function_id),
                        name,
                        emit_ctx,
                    ),
                    Some(refcount_decref_location_counter_parts(emit_ctx)),
                    emit_ctx.consts.ptr_ty,
                    emit_ctx.consts.thread_state_value,
                    emit_ctx.decref_ref,
                    emit_ctx.refcount_emitter(),
                )
                .unwrap_or_else(|error| panic!("{error}"));
            return Some(emit_none_for_demand(fb, emit_ctx, demand));
        }
        let value_py_facts = if matches!(emit_ctx.function_kind, FunctionKind::Function) {
            py_facts_for_codegen_expr_with_local_env(&op.value, local_env, emit_ctx)
        } else {
            None
        };
        let value = emit_codegen_expr_with_local_env(
            fb,
            &op.value,
            local_env,
            emit_ctx,
            false,
            codegen_env,
            func_imports,
        );
        let value_ref_kind = match planned_local_store_effect(expr, location, emit_ctx) {
            Some(PlannedLocalStoreEffect::Rebind(ref_kind)) => ref_kind,
            Some(PlannedLocalStoreEffect::Delete) => unreachable!(),
            None => local_ref_kind_for_stored_value(&op.value, emit_ctx),
        };
        local_env.store_location(
            fb,
            location,
            name,
            value,
            value_ref_kind,
            value_py_facts,
            emit_ctx.allow_local_only_slot_backed_stores,
            planned_cleanup_root_previous_state_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            planned_cleanup_root_previous_facts_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            &emit_ctx.stack_slots,
            Some(refcount_decref_location_counter_parts(emit_ctx)),
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.incref_ref,
            emit_ctx.decref_ref,
            emit_ctx.refcount_emitter(),
        );
        return Some(emit_none_for_demand(fb, emit_ctx, demand));
    }

    let location = op.name.cell_location()?;
    if !(location.is_owned() && matches!(op.value.as_ref(), InstrBlockPy::MakeCell(_))) {
        return None;
    }
    let layout = emit_ctx
        .storage_layout
        .as_ref()
        .expect("Store owned cell slot should have storage layout during codegen");
    let backing = owned_cell_backing_local(layout, location.slot());
    let backing_name = backing
        .as_ref()
        .map(|(_, name)| *name)
        .or_else(|| {
            layout
                .owned_slot(location.slot())
                .map(|slot| slot.storage_name.as_str())
        })
        .unwrap_or_else(|| {
            panic!(
                "missing owned cell slot mapping for owned cell location {}",
                location.slot()
            )
        });
    let value_py_facts = if matches!(emit_ctx.function_kind, FunctionKind::Function) {
        py_facts_for_codegen_expr_with_local_env(&op.value, local_env, emit_ctx)
    } else {
        None
    };
    let value = emit_codegen_expr_with_local_env(
        fb,
        &op.value,
        local_env,
        emit_ctx,
        false,
        codegen_env,
        func_imports,
    );
    let default_ref_kind = local_ref_kind_for_stored_value(&op.value, emit_ctx);
    if let Some((backing_location, _)) = backing {
        let value_ref_kind = match planned_local_store_effect(expr, backing_location, emit_ctx) {
            Some(PlannedLocalStoreEffect::Rebind(ref_kind)) => ref_kind,
            Some(PlannedLocalStoreEffect::Delete) => unreachable!(),
            None => default_ref_kind,
        };
        local_env.store_location(
            fb,
            backing_location,
            backing_name,
            value,
            value_ref_kind,
            value_py_facts,
            emit_ctx.allow_local_only_slot_backed_stores,
            planned_cleanup_root_previous_state_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                backing_name,
                emit_ctx,
            ),
            planned_cleanup_root_previous_facts_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                backing_name,
                emit_ctx,
            ),
            &emit_ctx.stack_slots,
            Some(refcount_decref_location_counter_parts(emit_ctx)),
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.incref_ref,
            emit_ctx.decref_ref,
            emit_ctx.refcount_emitter(),
        );
    } else {
        local_env.store_name(
            fb,
            backing_name,
            value,
            default_ref_kind,
            value_py_facts,
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
            emit_ctx.refcount_emitter(),
        );
    }
    Some(emit_none_for_demand(fb, emit_ctx, demand))
}

fn emit_typed_local_store_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    op: &Store<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let Some(location) = op.name.local_location() else {
        return Ok(None);
    };
    let layout = emit_ctx
        .storage_layout
        .as_ref()
        .expect("Store local slot should have storage layout during typed codegen");
    let name = local_name_for_location(layout, location);
    let planned_store_effect = planned_typed_local_store_effect(expr, location, emit_ctx);
    if matches!(planned_store_effect, Some(PlannedLocalStoreEffect::Delete)) {
        local_env
            .delete_location(
                fb,
                location,
                name,
                &emit_ctx.stack_slots,
                planned_cleanup_root_previous_state_for_key(
                    expr.semantic_instr_key(emit_ctx.function_id),
                    name,
                    emit_ctx,
                ),
                planned_cleanup_root_previous_facts_for_key(
                    expr.semantic_instr_key(emit_ctx.function_id),
                    name,
                    emit_ctx,
                ),
                Some(refcount_decref_location_counter_parts(emit_ctx)),
                emit_ctx.consts.ptr_ty,
                emit_ctx.consts.thread_state_value,
                emit_ctx.decref_ref,
                emit_ctx.refcount_emitter(),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        return Ok(Some(emit_none_for_demand(fb, emit_ctx, demand)));
    }

    let planned_borrowed_store = matches!(
        planned_store_effect.as_ref(),
        Some(PlannedLocalStoreEffect::Rebind(LocalRefKind::Borrowed))
    );
    let store_value_demand = if planned_borrowed_store {
        ResultDemand::PYOBJECT_BORROWED_OK
    } else {
        ResultDemand::PYOBJECT_OWNED
    };
    let value_demand = if planned_borrowed_store {
        store_value_demand
    } else {
        op.value.result_demand().unwrap_or(store_value_demand)
    };
    let planned_truthiness_only_store =
        emit_ctx.truthiness_only_local_locations.contains(&location)
            && planning::typed_expr_can_satisfy_pyobject_truthiness_repr(op.value.as_ref());
    if (!planned_borrowed_store
        && typed_expr_i32_bool01_demand_facts(op.value.as_ref(), local_env, emit_ctx).is_some())
        || planned_truthiness_only_store
    {
        let value_result = emit_typed_codegen_stmt_result_with_local_env(
            fb,
            &op.value,
            local_env,
            emit_ctx,
            ResultDemand::I32_BOOL01,
            codegen_env,
            func_imports,
        )?;
        let value = value_result.expect_i32_bool01("typed local scalar bool store RHS");
        local_env.store_i32_bool01_location(
            fb,
            location,
            name,
            value,
            planned_cleanup_root_previous_state_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            planned_cleanup_root_previous_facts_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            &emit_ctx.stack_slots,
            Some(refcount_decref_location_counter_parts(emit_ctx)),
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
            emit_ctx.refcount_emitter(),
        );
        return Ok(Some(emit_none_for_demand(fb, emit_ctx, demand)));
    }
    if !planned_borrowed_store
        && typed_expr_i64_demand_facts(op.value.as_ref(), local_env, emit_ctx).is_some()
    {
        let value_result = emit_typed_codegen_stmt_result_with_local_env(
            fb,
            &op.value,
            local_env,
            emit_ctx,
            ResultDemand::I64_VALUE,
            codegen_env,
            func_imports,
        )?;
        let (value, i64_facts) = value_result.expect_i64("typed local scalar store RHS");
        local_env.store_i64_location(
            fb,
            location,
            name,
            value,
            i64_facts,
            planned_cleanup_root_previous_state_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            planned_cleanup_root_previous_facts_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            &emit_ctx.stack_slots,
            Some(refcount_decref_location_counter_parts(emit_ctx)),
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
            emit_ctx.refcount_emitter(),
        );
        return Ok(Some(emit_none_for_demand(fb, emit_ctx, demand)));
    }
    let value_result = match value_demand {
        ResultDemand::PyObject { .. } => emit_typed_codegen_stmt_result_with_local_env(
            fb,
            &op.value,
            local_env,
            emit_ctx,
            value_demand,
            codegen_env,
            func_imports,
        )?,
        other => {
            return Err(format!(
                "typed local store RHS requires PyObject demand, got {other:?}"
            ));
        }
    };
    let (value, ownership, value_py_facts) = value_result.expect_pyobject("typed local store RHS");
    let value_py_facts = if matches!(emit_ctx.function_kind, FunctionKind::Function) {
        py_facts_for_typed_expr_with_local_env(&op.value, local_env).unwrap_or(value_py_facts)
    } else {
        value_py_facts
    };
    if !ownership.can_satisfy_pyobject_demand(store_value_demand) {
        return Err(format!(
            "typed local store RHS produced {ownership:?}, but store requires {store_value_demand:?}"
        ));
    }
    let value_ref_kind = match planned_store_effect {
        Some(PlannedLocalStoreEffect::Rebind(ref_kind)) => ref_kind,
        Some(PlannedLocalStoreEffect::Delete) => unreachable!(),
        None => local_ref_kind_for_typed_stored_value(&op.value, ownership),
    };
    local_env.store_location(
        fb,
        location,
        name,
        value,
        value_ref_kind,
        Some(value_py_facts),
        emit_ctx.allow_local_only_slot_backed_stores,
        planned_cleanup_root_previous_state_for_key(
            expr.semantic_instr_key(emit_ctx.function_id),
            name,
            emit_ctx,
        ),
        planned_cleanup_root_previous_facts_for_key(
            expr.semantic_instr_key(emit_ctx.function_id),
            name,
            emit_ctx,
        ),
        &emit_ctx.stack_slots,
        Some(refcount_decref_location_counter_parts(emit_ctx)),
        emit_ctx.consts.ptr_ty,
        emit_ctx.consts.thread_state_value,
        emit_ctx.incref_ref,
        emit_ctx.decref_ref,
        emit_ctx.refcount_emitter(),
    );
    Ok(Some(emit_none_for_demand(fb, emit_ctx, demand)))
}

fn emit_preserved_store_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Store<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    let location = op.name.preserved_location()?;
    let value = emit_codegen_expr_with_local_env(
        fb,
        &op.value,
        local_env,
        emit_ctx,
        false,
        codegen_env,
        func_imports,
    );
    Some(emit_preserved_store_value_result(
        fb, location, value, true, emit_ctx, demand,
    ))
}

fn emit_typed_preserved_store_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Store<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let Some(location) = op.name.preserved_location() else {
        return Ok(None);
    };
    if preserved_slot_storage_for_location(emit_ctx, location) == PreservedSlotStorage::I64
        && typed_expr_i64_demand_facts(op.value.as_ref(), local_env, emit_ctx).is_some()
    {
        let value_result = emit_typed_codegen_stmt_result_with_local_env(
            fb,
            &op.value,
            local_env,
            emit_ctx,
            ResultDemand::I64_VALUE,
            codegen_env,
            func_imports,
        )?;
        let (raw_value, _) = value_result.expect_i64("typed preserved scalar store value");
        return Ok(Some(emit_preserved_store_i64_result(
            fb, location, raw_value, emit_ctx, demand,
        )));
    }
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        &op.value,
        local_env,
        emit_ctx,
        typed_expr_pyobject_input_is_borrowed_from_local_env(&op.value, local_env, emit_ctx),
        codegen_env,
        func_imports,
    )?;
    let (value, ownership, _) = value.expect_pyobject("typed preserved store value");
    Ok(Some(emit_preserved_store_value_result(
        fb,
        location,
        value,
        ownership.is_owned(),
        emit_ctx,
        demand,
    )))
}

fn emit_preserved_store_value_result(
    fb: &mut FunctionBuilder<'_>,
    location: PreservedLocation,
    value: ir::Value,
    value_is_owned: bool,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let values = preserved_values_base_value(emit_ctx);
    let slot_offset = preserved_values_slot_offset(location.slot()).unwrap_or_else(|err| {
        panic!(
            "invalid preserved store offset for function {} slot {}: {err}",
            emit_ctx.function_id,
            location.slot()
        )
    });
    match preserved_slot_storage_for_location(emit_ctx, location) {
        PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::PyCellObject => {
            emit_ctx.emit_incref_for_family(
                fb,
                value,
                Some(PyObjFacts::unknown().with_non_null_ref()),
                RefcountFamily::ContainerStoreClone,
            );
            let old_value = fb.ins().load(
                emit_ctx.consts.ptr_ty,
                ir::MemFlags::trusted(),
                values,
                slot_offset,
            );
            fb.ins()
                .store(ir::MemFlags::trusted(), value, values, slot_offset);
            let null_ptr = fb.ins().iconst(emit_ctx.consts.ptr_ty, 0);
            let old_is_null = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, old_value, null_ptr);
            let release_old_block = fb.create_block();
            let done_block = fb.create_block();
            fb.append_block_param(release_old_block, emit_ctx.consts.ptr_ty);
            fb.ins().brif(
                old_is_null,
                done_block,
                &[],
                release_old_block,
                &[ir::BlockArg::Value(old_value)],
            );

            fb.switch_to_block(release_old_block);
            let old_value = fb.block_params(release_old_block)[0];
            emit_ctx.emit_decref_for_family(
                fb,
                old_value,
                Some(PyObjFacts::unknown().with_non_null_ref()),
                RefcountFamily::ContainerOverwriteRelease,
            );
            fb.ins().jump(done_block, &[]);

            fb.switch_to_block(done_block);
            if value_is_owned {
                emit_ctx.emit_decref_for_family(fb, value, None, RefcountFamily::OwnedTemporary);
            }
        }
        PreservedSlotStorage::I64 => {
            let value_inst = fb.ins().call(emit_ctx.pyobject_to_i64_ref, &[value]);
            let raw_value = fb.inst_results(value_inst)[0];
            let owned_inputs_storage = [value];
            let owned_inputs = if value_is_owned {
                &owned_inputs_storage[..]
            } else {
                &[][..]
            };
            let raw_value = emit_scalar_result_after_current_exception_check_with_cleanup(
                fb,
                raw_value,
                emit_ctx.consts.i64_ty,
                owned_inputs,
                emit_ctx,
            );
            return emit_preserved_store_i64_result(fb, location, raw_value, emit_ctx, demand);
        }
    }
    emit_none_for_demand(fb, emit_ctx, demand)
}

fn emit_preserved_store_i64_result(
    fb: &mut FunctionBuilder<'_>,
    location: PreservedLocation,
    raw_value: ir::Value,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let values = preserved_values_base_value(emit_ctx);
    let slot_offset = preserved_values_slot_offset(location.slot()).unwrap_or_else(|err| {
        panic!(
            "invalid preserved scalar store offset for function {} slot {}: {err}",
            emit_ctx.function_id,
            location.slot()
        )
    });
    fb.ins()
        .store(ir::MemFlags::trusted(), raw_value, values, slot_offset);
    emit_none_for_demand(fb, emit_ctx, demand)
}

fn typed_local_store_prefers_scalar_repr(
    expr: &InstrTyped,
    store: &Store<InstrTyped>,
    target_location: LocalLocation,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    let planned_store_effect = planned_typed_local_store_effect(expr, target_location, emit_ctx);
    let planned_borrowed_store = matches!(
        planned_store_effect.as_ref(),
        Some(PlannedLocalStoreEffect::Rebind(LocalRefKind::Borrowed))
    );
    ((!planned_borrowed_store
        && typed_expr_i32_bool01_demand_facts(store.value.as_ref(), local_env, emit_ctx).is_some())
        || (emit_ctx
            .truthiness_only_local_locations
            .contains(&target_location)
            && planning::typed_expr_can_satisfy_pyobject_truthiness_repr(store.value.as_ref())))
        || (!planned_borrowed_store
            && typed_expr_i64_demand_facts(store.value.as_ref(), local_env, emit_ctx).is_some())
}

fn emit_typed_owned_cell_makecell_store_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    op: &Store<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let Some(location) = op.name.cell_location() else {
        return Ok(None);
    };
    if !(location.is_owned() && matches!(op.value.as_ref(), InstrTyped::MakeCell(_))) {
        return Ok(None);
    }
    let layout = emit_ctx
        .storage_layout
        .as_ref()
        .expect("Store owned cell slot should have storage layout during typed codegen");
    let backing = owned_cell_backing_local(layout, location.slot());
    let backing_name = backing
        .as_ref()
        .map(|(_, name)| *name)
        .or_else(|| {
            layout
                .owned_slot(location.slot())
                .map(|slot| slot.storage_name.as_str())
        })
        .unwrap_or_else(|| {
            panic!(
                "missing owned cell slot mapping for owned cell location {}",
                location.slot()
            )
        });
    let value_result = emit_typed_codegen_stmt_result_with_local_env(
        fb,
        &op.value,
        local_env,
        emit_ctx,
        ResultDemand::PYOBJECT_OWNED,
        codegen_env,
        func_imports,
    )?;
    let (value, ownership, value_py_facts) =
        value_result.expect_pyobject("typed owned cell MakeCell store RHS");
    if !ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED) {
        return Err(format!(
            "typed owned cell MakeCell store RHS produced {ownership:?}, but store requires owned PyObject"
        ));
    }
    let value_py_facts = if matches!(emit_ctx.function_kind, FunctionKind::Function) {
        py_facts_for_typed_expr_with_local_env(&op.value, local_env).unwrap_or(value_py_facts)
    } else {
        value_py_facts
    };
    let default_ref_kind = local_ref_kind_for_typed_stored_value(&op.value, ownership);
    if let Some((backing_location, _)) = backing {
        let value_ref_kind =
            match planned_typed_local_store_effect(expr, backing_location, emit_ctx) {
                Some(PlannedLocalStoreEffect::Rebind(ref_kind)) => ref_kind,
                Some(PlannedLocalStoreEffect::Delete) => unreachable!(),
                None => default_ref_kind,
            };
        local_env.store_location(
            fb,
            backing_location,
            backing_name,
            value,
            value_ref_kind,
            Some(value_py_facts),
            emit_ctx.allow_local_only_slot_backed_stores,
            planned_cleanup_root_previous_state_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                backing_name,
                emit_ctx,
            ),
            planned_cleanup_root_previous_facts_for_key(
                expr.semantic_instr_key(emit_ctx.function_id),
                backing_name,
                emit_ctx,
            ),
            &emit_ctx.stack_slots,
            Some(refcount_decref_location_counter_parts(emit_ctx)),
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.incref_ref,
            emit_ctx.decref_ref,
            emit_ctx.refcount_emitter(),
        );
    } else {
        local_env.store_name(
            fb,
            backing_name,
            value,
            default_ref_kind,
            Some(value_py_facts),
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
            emit_ctx.refcount_emitter(),
        );
    }
    Ok(Some(emit_none_for_demand(fb, emit_ctx, demand)))
}

fn emit_typed_cell_store_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Store<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let Some(location) = op.name.cell_location() else {
        return Ok(None);
    };
    let raw_cell = emit_raw_cell_object_for_location_with_local_env(
        fb,
        location,
        op.name.id.as_str(),
        local_env,
        emit_ctx,
    );
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        &op.value,
        local_env,
        emit_ctx,
        typed_expr_pyobject_input_is_borrowed_from_local_env(&op.value, local_env, emit_ctx),
        codegen_env,
        func_imports,
    )?;
    let (value, ownership, _) = value.expect_pyobject("typed cell store value");
    let call_inst = fb.ins().call(emit_ctx.store_cell_ref, &[raw_cell, value]);
    emit_ctx.emit_decref_for_family(fb, raw_cell, None, RefcountFamily::OwnedTemporary);
    if ownership.is_owned() {
        emit_ctx.emit_decref_for_family(fb, value, None, RefcountFamily::OwnedTemporary);
    }
    let call_value = fb.inst_results(call_inst)[0];
    let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
        fb,
        local_env,
        ctx: emit_ctx,
        codegen_env,
        func_imports,
        owned_transfer_temp_load: None,
    };
    let value = intrinsics::OperationEmitState::<InstrTyped>::finish_owned_result(
        &mut intrinsic_state,
        call_value,
    );
    Ok(Some(emit_owned_pyobject_result_for_demand(
        intrinsic_state.fb,
        value,
        PyObjFacts::none_singleton(),
        intrinsic_state.ctx,
        demand,
    )))
}

fn emit_typed_local_delete_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Del<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> Option<EmitResult> {
    let location = op.name.local_location()?;
    let layout = emit_ctx
        .storage_layout
        .as_ref()
        .expect("Del local slot should have storage layout during typed codegen");
    let name = local_name_for_location(layout, location);
    local_env
        .delete_location(
            fb,
            location,
            name,
            &emit_ctx.stack_slots,
            planned_cleanup_root_previous_state_for_key(
                op.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            planned_cleanup_root_previous_facts_for_key(
                op.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            Some(refcount_decref_location_counter_parts(emit_ctx)),
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
            emit_ctx.refcount_emitter(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    Some(emit_none_for_demand(fb, emit_ctx, demand))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_cell_delete_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Del<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    let location = op.name.cell_location()?;
    let raw_cell = emit_raw_cell_object_for_location_with_local_env(
        fb,
        location,
        op.name.id.as_str(),
        local_env,
        emit_ctx,
    );
    let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
        fb,
        local_env,
        ctx: emit_ctx,
        codegen_env,
        func_imports,
        owned_transfer_temp_load: None,
    };
    let value = intrinsics::emit_del_deref_raw_cell::<InstrTyped>(
        raw_cell,
        op.quietly,
        &mut intrinsic_state,
    );
    Some(emit_owned_pyobject_result_for_demand(
        intrinsic_state.fb,
        value,
        PyObjFacts::none_singleton(),
        intrinsic_state.ctx,
        demand,
    ))
}

fn emit_preserved_delete_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Del<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> Option<EmitResult> {
    let location = op.name.preserved_location()?;
    Some(emit_preserved_delete_result(
        fb,
        op.name.id.as_str(),
        location,
        op.quietly,
        local_env,
        emit_ctx,
        demand,
    ))
}

fn emit_typed_preserved_delete_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Del<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> Option<EmitResult> {
    let location = op.name.preserved_location()?;
    Some(emit_preserved_delete_result(
        fb,
        op.name.id.as_str(),
        location,
        op.quietly,
        local_env,
        emit_ctx,
        demand,
    ))
}

fn emit_preserved_delete_result(
    fb: &mut FunctionBuilder<'_>,
    name: &str,
    location: PreservedLocation,
    quietly: bool,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    match preserved_slot_storage_for_location(emit_ctx, location) {
        PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::PyCellObject => {
            let values = preserved_values_base_value(emit_ctx);
            let slot_offset = preserved_values_slot_offset(location.slot()).unwrap_or_else(|err| {
                panic!(
                    "invalid preserved delete offset for function {} slot {}: {err}",
                    emit_ctx.function_id,
                    location.slot()
                )
            });
            let null_ptr = fb.ins().iconst(emit_ctx.consts.ptr_ty, 0);
            let old_value = fb.ins().load(
                emit_ctx.consts.ptr_ty,
                ir::MemFlags::trusted(),
                values,
                slot_offset,
            );
            let old_is_null = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, old_value, null_ptr);
            let release_block = fb.create_block();
            let done_block = fb.create_block();
            fb.append_block_param(release_block, emit_ctx.consts.ptr_ty);
            if quietly {
                fb.ins().brif(
                    old_is_null,
                    done_block,
                    &[],
                    release_block,
                    &[ir::BlockArg::Value(old_value)],
                );
            } else {
                let unbound_block = fb.create_block();
                fb.ins().brif(
                    old_is_null,
                    unbound_block,
                    &[],
                    release_block,
                    &[ir::BlockArg::Value(old_value)],
                );

                fb.switch_to_block(unbound_block);
                let name_obj = emit_owned_module_constant(
                    fb,
                    emit_ctx.module_constants.require_unicode_constant_id(name),
                    emit_ctx,
                );
                tracing::info!(
                    target: "soac_unbound_local_codegen",
                    function_id = ?emit_ctx.function_id,
                    name,
                    location = ?location,
                    "emit_preserved_delete_unbound_path",
                );
                fb.ins()
                    .call(emit_ctx.raise_unbound_local_error_ref, &[name_obj]);
                emit_release_owned_inputs(fb, emit_ctx, &[name_obj]);
                fb.ins().jump(
                    emit_ctx.consts.step_null_block,
                    &step_null_block_args(emit_ctx),
                );
            }

            fb.switch_to_block(release_block);
            let old_value = fb.block_params(release_block)[0];
            fb.ins()
                .store(ir::MemFlags::trusted(), null_ptr, values, slot_offset);
            emit_ctx.emit_decref_for_family(
                fb,
                old_value,
                Some(PyObjFacts::unknown().with_non_null_ref()),
                RefcountFamily::ExplicitDelete,
            );
            fb.ins().jump(done_block, &[]);

            fb.switch_to_block(done_block);
            emit_none_for_demand(fb, emit_ctx, demand)
        }
        PreservedSlotStorage::I64 => {
            let state = emit_preserved_state_with_local_env(fb, local_env, emit_ctx);
            let slot = fb.ins().iconst(ir::types::I64, i64::from(location.slot()));
            let name_obj = emit_owned_module_constant(
                fb,
                emit_ctx.module_constants.require_unicode_constant_id(name),
                emit_ctx,
            );
            let func_ref = if quietly {
                emit_ctx.del_preserved_quietly_ref
            } else {
                emit_ctx.del_preserved_ref
            };
            let call_inst = fb.ins().call(func_ref, &[state, slot, name_obj]);
            emit_ctx.emit_decref_for_family(
                fb,
                name_obj,
                Some(PyObjFacts::exact_type(PyExactType::Str)),
                RefcountFamily::OwnedTemporary,
            );
            let result =
                emit_checked_owned_pyobject_result(fb, fb.inst_results(call_inst)[0], emit_ctx);
            emit_owned_pyobject_result_for_demand(
                fb,
                result,
                PyObjFacts::none_singleton(),
                emit_ctx,
                demand,
            )
        }
    }
}

fn emit_local_delete_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Del<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> Option<ir::Value> {
    let result = emit_local_delete_result_with_local_env(
        fb,
        op,
        local_env,
        emit_ctx,
        ResultDemand::PYOBJECT_OWNED,
    )?;
    let (value, ownership, _) = result.expect_pyobject("legacy local delete result");
    assert!(
        ownership.is_owned(),
        "legacy local delete result should produce an owned PyObject"
    );
    Some(value)
}

fn emit_local_delete_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Del<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> Option<EmitResult> {
    let location = op.name.local_location()?;
    let layout = emit_ctx
        .storage_layout
        .as_ref()
        .expect("Del local slot should have storage layout during codegen");
    let name = local_name_for_location(layout, location);
    local_env
        .delete_location(
            fb,
            location,
            name,
            &emit_ctx.stack_slots,
            planned_cleanup_root_previous_state_for_key(
                op.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            planned_cleanup_root_previous_facts_for_key(
                op.semantic_instr_key(emit_ctx.function_id),
                name,
                emit_ctx,
            ),
            Some(refcount_decref_location_counter_parts(emit_ctx)),
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
            emit_ctx.refcount_emitter(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    Some(emit_none_for_demand(fb, emit_ctx, demand))
}

#[derive(Clone)]
struct StackSlots {
    names: Vec<String>,
    storage_layout_indices: Vec<usize>,
    slots: Vec<ir::StackSlot>,
    cleanup_root_names: HashSet<String>,
}

impl StackSlots {
    fn new(
        fb: &mut FunctionBuilder<'_>,
        slot_names: &[String],
        cleanup_root_names: &HashSet<String>,
        storage_layout: Option<&StorageLayout>,
    ) -> Self {
        let mut slots = Vec::with_capacity(slot_names.len());
        for _ in slot_names {
            slots.push(fb.create_sized_stack_slot(ir::StackSlotData::new(
                ir::StackSlotKind::ExplicitSlot,
                std::mem::size_of::<u64>() as u32,
                0,
            )));
        }
        let slot_name_set = slot_names
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let storage_layout_index_by_name = storage_layout
            .map(|layout| {
                layout
                    .stack_slots()
                    .iter()
                    .enumerate()
                    .map(|(index, name)| (name.as_str(), index))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let storage_layout_indices = slot_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                storage_layout_index_by_name
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(index)
            })
            .collect();
        Self {
            names: slot_names.to_vec(),
            storage_layout_indices,
            slots,
            cleanup_root_names: cleanup_root_names
                .iter()
                .filter(|name| slot_name_set.contains(name.as_str()))
                .cloned()
                .collect(),
        }
    }

    fn slot_for_name(&self, name: &str) -> Option<ir::StackSlot> {
        self.slot_index_for_name(name)
            .map(|index| self.slots[index])
    }

    fn slot_index_for_name(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|candidate| candidate == name)
    }

    fn storage_layout_index_for_slot_index(&self, slot_index: usize) -> usize {
        self.storage_layout_indices[slot_index]
    }

    fn slot_for_block_arg_name(&self, name: &str) -> Option<ir::StackSlot> {
        self.slot_for_name(name).or_else(|| {
            if !is_try_exception_alias_name(name) {
                return None;
            }
            let mut matches = self
                .names
                .iter()
                .enumerate()
                .filter(|(_, candidate)| is_try_exception_alias_name(candidate));
            let first = matches.next().map(|(index, _)| self.slots[index]);
            debug_assert!(
                matches.next().is_none(),
                "expected at most one current-exception stack slot"
            );
            first
        })
    }

    fn has_name(&self, name: &str) -> bool {
        self.slot_for_name(name).is_some()
    }

    fn has_cleanup_root_name(&self, name: &str) -> bool {
        self.cleanup_root_names.contains(name)
    }

    fn initialize_all(
        &self,
        fb: &mut FunctionBuilder<'_>,
        ptr_ty: ir::Type,
        fallthrough_abrupt_kind_const: Option<ir::Value>,
    ) {
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        for (name, slot) in self.names.iter().zip(self.slots.iter()) {
            let value = if is_try_abrupt_kind_name(name) {
                fallthrough_abrupt_kind_const
                    .expect("try abrupt-kind stack slots require a fallthrough constant")
            } else {
                null_ptr
            };
            fb.ins().stack_store(value, *slot, 0);
        }
    }

    fn has_try_abrupt_kind_name(&self) -> bool {
        self.names
            .iter()
            .any(|name| is_try_abrupt_kind_name(name.as_str()))
    }

    fn replace_cloned_value_counted(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        value_ref_kind: LocalRefKind,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        previous_facts: Option<PyObjFacts>,
        counter_parts: Option<RefcountDecrefLocationCounterParts<'_>>,
    ) -> Option<()> {
        self.replace_cloned_value_with_previous_state_counted(
            fb,
            name,
            value,
            value_ref_kind,
            CleanupRootSlotState::MaybeOwnedReference,
            ptr_ty,
            thread_state_value,
            incref_ref,
            decref_ref,
            refcounts,
            previous_facts,
            counter_parts,
        )
    }

    fn replace_cloned_value_with_previous_state_counted(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        value_ref_kind: LocalRefKind,
        previous_state: CleanupRootSlotState,
        ptr_ty: ir::Type,
        _thread_state_value: ir::Value,
        _incref_ref: ir::FuncRef,
        _decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        previous_facts: Option<PyObjFacts>,
        counter_parts: Option<RefcountDecrefLocationCounterParts<'_>>,
    ) -> Option<()> {
        let slot_index = self.slot_index_for_name(name)?;
        let slot = self.slots[slot_index];
        let previous = previous_state
            .may_hold_owned_reference()
            .then(|| fb.ins().stack_load(ptr_ty, slot, 0));
        if local_ref_kind_needs_refcount_call(value_ref_kind) {
            emit_incref_via_lowering(
                fb,
                refcounts.with_family(RefcountFamily::StackSlotClone),
                value,
                None,
            );
        }
        fb.ins().stack_store(value, slot, 0);
        if let Some(previous) = previous {
            if let Some(counter_parts) = counter_parts {
                emit_refcount_stack_slot_decref_location_counter(
                    fb,
                    REFCOUNT_STACK_SLOT_REPLACE_CLONED_PREVIOUS,
                    self.storage_layout_index_for_slot_index(slot_index),
                    name,
                    counter_parts,
                );
            }
            emit_decref_via_lowering(
                fb,
                refcounts,
                previous,
                nullable_stack_slot_decref_facts(previous_facts),
            );
        }
        Some(())
    }

    fn replace_transferred_value(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        value_ref_kind: LocalRefKind,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        previous_facts: Option<PyObjFacts>,
    ) -> Option<()> {
        self.replace_transferred_value_counted(
            fb,
            name,
            value,
            value_ref_kind,
            ptr_ty,
            thread_state_value,
            incref_ref,
            decref_ref,
            refcounts,
            previous_facts,
            None,
        )
    }

    fn replace_transferred_value_counted(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        value_ref_kind: LocalRefKind,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        previous_facts: Option<PyObjFacts>,
        counter_parts: Option<RefcountDecrefLocationCounterParts<'_>>,
    ) -> Option<()> {
        self.replace_transferred_value_with_previous_state_counted(
            fb,
            name,
            value,
            value_ref_kind,
            CleanupRootSlotState::MaybeOwnedReference,
            ptr_ty,
            thread_state_value,
            incref_ref,
            decref_ref,
            refcounts,
            previous_facts,
            counter_parts,
        )
    }

    fn replace_transferred_value_with_previous_state_counted(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        value_ref_kind: LocalRefKind,
        previous_state: CleanupRootSlotState,
        ptr_ty: ir::Type,
        _thread_state_value: ir::Value,
        _incref_ref: ir::FuncRef,
        _decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        previous_facts: Option<PyObjFacts>,
        counter_parts: Option<RefcountDecrefLocationCounterParts<'_>>,
    ) -> Option<()> {
        let slot_index = self.slot_index_for_name(name)?;
        let slot = self.slots[slot_index];
        let previous = previous_state
            .may_hold_owned_reference()
            .then(|| fb.ins().stack_load(ptr_ty, slot, 0));
        if local_ref_kind_needs_incref_for_stack_slot_transfer(value_ref_kind) {
            emit_incref_via_lowering(
                fb,
                refcounts.with_family(RefcountFamily::StackSlotClone),
                value,
                None,
            );
        }
        fb.ins().stack_store(value, slot, 0);
        if let Some(previous) = previous {
            if let Some(counter_parts) = counter_parts {
                emit_refcount_stack_slot_decref_location_counter(
                    fb,
                    REFCOUNT_STACK_SLOT_REPLACE_TRANSFERRED_PREVIOUS,
                    self.storage_layout_index_for_slot_index(slot_index),
                    name,
                    counter_parts,
                );
            }
            emit_decref_via_lowering(
                fb,
                refcounts,
                previous,
                nullable_stack_slot_decref_facts(previous_facts),
            );
        }
        Some(())
    }

    fn replace_moved_owned_value_with_previous_state_counted(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        previous_state: CleanupRootSlotState,
        ptr_ty: ir::Type,
        _thread_state_value: ir::Value,
        _decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        previous_facts: Option<PyObjFacts>,
        counter_parts: Option<RefcountDecrefLocationCounterParts<'_>>,
    ) -> Option<()> {
        let slot_index = self.slot_index_for_name(name)?;
        let slot = self.slots[slot_index];
        let previous = previous_state
            .may_hold_owned_reference()
            .then(|| fb.ins().stack_load(ptr_ty, slot, 0));
        fb.ins().stack_store(value, slot, 0);
        if let Some(previous) = previous {
            if let Some(counter_parts) = counter_parts {
                emit_refcount_stack_slot_decref_location_counter(
                    fb,
                    REFCOUNT_STACK_SLOT_REPLACE_MOVED_PREVIOUS,
                    self.storage_layout_index_for_slot_index(slot_index),
                    name,
                    counter_parts,
                );
            }
            emit_decref_via_lowering(
                fb,
                refcounts,
                previous,
                nullable_stack_slot_decref_facts(previous_facts),
            );
        }
        Some(())
    }

    fn clear_value(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        previous_facts: Option<PyObjFacts>,
    ) -> Option<()> {
        self.clear_value_counted(
            fb,
            name,
            ptr_ty,
            thread_state_value,
            decref_ref,
            refcounts,
            previous_facts,
            None,
        )
    }

    fn clear_value_counted(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        previous_facts: Option<PyObjFacts>,
        counter_parts: Option<RefcountDecrefLocationCounterParts<'_>>,
    ) -> Option<()> {
        self.clear_value_with_previous_state_counted(
            fb,
            name,
            CleanupRootSlotState::MaybeOwnedReference,
            ptr_ty,
            thread_state_value,
            decref_ref,
            refcounts,
            previous_facts,
            counter_parts,
        )
    }

    fn clear_value_with_previous_state_counted(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        previous_state: CleanupRootSlotState,
        ptr_ty: ir::Type,
        _thread_state_value: ir::Value,
        _decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        previous_facts: Option<PyObjFacts>,
        counter_parts: Option<RefcountDecrefLocationCounterParts<'_>>,
    ) -> Option<()> {
        let slot_index = self.slot_index_for_name(name)?;
        let slot = self.slots[slot_index];
        let previous = previous_state
            .may_hold_owned_reference()
            .then(|| fb.ins().stack_load(ptr_ty, slot, 0));
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        fb.ins().stack_store(null_ptr, slot, 0);
        if let Some(previous) = previous {
            if let Some(counter_parts) = counter_parts {
                emit_refcount_stack_slot_decref_location_counter(
                    fb,
                    REFCOUNT_STACK_SLOT_CLEAR_PREVIOUS,
                    self.storage_layout_index_for_slot_index(slot_index),
                    name,
                    counter_parts,
                );
            }
            emit_decref_via_lowering(
                fb,
                refcounts,
                previous,
                nullable_stack_slot_decref_facts(previous_facts),
            );
        }
        Some(())
    }

    fn clear_moved_value(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        ptr_ty: ir::Type,
    ) -> Option<()> {
        let slot = self.slot_for_name(name)?;
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        fb.ins().stack_store(null_ptr, slot, 0);
        Some(())
    }

    fn decref_all_with_cleanup_root_states_counted(
        &self,
        fb: &mut FunctionBuilder<'_>,
        ptr_ty: ir::Type,
        _thread_state_value: ir::Value,
        _decref_ref: ir::FuncRef,
        refcounts: RefcountEmitter,
        cleanup_root_states: &HashMap<String, CleanupRootSlotState>,
        cleanup_root_facts: &HashMap<String, PyObjFacts>,
        counter_parts: Option<RefcountDecrefLocationCounterParts<'_>>,
    ) {
        for (slot_index, (name, slot)) in self.names.iter().zip(self.slots.iter()).enumerate() {
            if !cleanup_root_states
                .get(name)
                .copied()
                .unwrap_or(CleanupRootSlotState::MaybeOwnedReference)
                .may_hold_owned_reference()
            {
                continue;
            }
            let value = fb.ins().stack_load(ptr_ty, *slot, 0);
            if let Some(counter_parts) = counter_parts {
                emit_refcount_stack_slot_decref_location_counter(
                    fb,
                    REFCOUNT_STACK_SLOT_EXIT_SWEEP,
                    self.storage_layout_index_for_slot_index(slot_index),
                    name,
                    counter_parts,
                );
            }
            emit_decref_via_lowering(
                fb,
                refcounts.with_family(RefcountFamily::ExitSweep),
                value,
                nullable_stack_slot_decref_facts(cleanup_root_facts.get(name).copied()),
            );
        }
    }
}

fn nullable_stack_slot_decref_facts(facts: Option<PyObjFacts>) -> Option<PyObjFacts> {
    // Maybe-owned frame roots can physically hold null on paths where no object
    // has been stored. Preserve object facts, but never use them to skip the
    // raw null guard for a stack-slot decref.
    facts.map(PyObjFacts::without_non_null_ref)
}

fn emit_incref_if_not_null(
    fb: &mut FunctionBuilder<'_>,
    _ptr_ty: ir::Type,
    incref_ref: ir::FuncRef,
    value: ir::Value,
) {
    // The runtime refcount helpers own the null and immortal checks. Emitting
    // a caller-side null branch duplicates those checks after runtime inlining.
    fb.ins().call(incref_ref, &[value]);
}

fn emit_decref_if_not_null(
    fb: &mut FunctionBuilder<'_>,
    _ptr_ty: ir::Type,
    decref_ref: ir::FuncRef,
    thread_state_value: ir::Value,
    value: ir::Value,
) {
    fb.ins().call(decref_ref, &[thread_state_value, value]);
}

fn emit_incref_via_lowering(
    fb: &mut FunctionBuilder<'_>,
    refcounts: RefcountEmitter,
    value: ir::Value,
    facts: Option<PyObjFacts>,
) {
    refcounts.emit_incref(fb, value, facts);
}

fn emit_decref_via_lowering(
    fb: &mut FunctionBuilder<'_>,
    refcounts: RefcountEmitter,
    value: ir::Value,
    facts: Option<PyObjFacts>,
) {
    refcounts.emit_decref(fb, value, facts);
}

fn refcount_family_for_release_reason(reason: &RefcountReleaseReason) -> RefcountFamily {
    match reason {
        RefcountReleaseReason::Return => RefcountFamily::ReturnRelease,
        RefcountReleaseReason::Raise => RefcountFamily::RaiseRelease,
        RefcountReleaseReason::Jump { .. }
        | RefcountReleaseReason::IfThen { .. }
        | RefcountReleaseReason::IfElse { .. }
        | RefcountReleaseReason::BranchCase { .. }
        | RefcountReleaseReason::BranchDefault { .. }
        | RefcountReleaseReason::ExceptionEdge { .. } => RefcountFamily::EdgeRelease,
    }
}

#[derive(Clone)]
struct ExceptionStateSlots {
    previous_handled_by_name: HashMap<String, ir::StackSlot>,
    previous_handled_is_pushed_by_name: HashMap<String, ir::StackSlot>,
}

impl ExceptionStateSlots {
    fn new(fb: &mut FunctionBuilder<'_>, function: &BlockPyFunction<impl ModuleShape>) -> Self {
        let mut previous_handled_by_name = HashMap::new();
        let mut previous_handled_is_pushed_by_name = HashMap::new();
        for block in &function.blocks {
            let Some(name) = block.exception_param() else {
                continue;
            };
            previous_handled_by_name
                .entry(name.to_string())
                .or_insert_with(|| {
                    fb.create_sized_stack_slot(ir::StackSlotData::new(
                        ir::StackSlotKind::ExplicitSlot,
                        std::mem::size_of::<u64>() as u32,
                        0,
                    ))
                });
            previous_handled_is_pushed_by_name
                .entry(name.to_string())
                .or_insert_with(|| {
                    fb.create_sized_stack_slot(ir::StackSlotData::new(
                        ir::StackSlotKind::ExplicitSlot,
                        std::mem::size_of::<u64>() as u32,
                        0,
                    ))
                });
        }
        Self {
            previous_handled_by_name,
            previous_handled_is_pushed_by_name,
        }
    }

    fn initialize_all_to_null(&self, fb: &mut FunctionBuilder<'_>, ptr_ty: ir::Type) {
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        for slot in self.previous_handled_by_name.values() {
            fb.ins().stack_store(null_ptr, *slot, 0);
        }
        let not_pushed = fb.ins().iconst(ir::types::I64, 0);
        for slot in self.previous_handled_is_pushed_by_name.values() {
            fb.ins().stack_store(not_pushed, *slot, 0);
        }
    }

    fn slots_for_exception(&self, name: &str) -> Option<(ir::StackSlot, ir::StackSlot)> {
        Some((
            self.previous_handled_by_name.get(name).copied()?,
            self.previous_handled_is_pushed_by_name.get(name).copied()?,
        ))
    }
}

impl<'a, 'b, 'mc, 'c, 'd, Env: JitCodegenEnv> intrinsics::OperationEmitState<'b, InstrBlockPy>
    for LocalEnvCodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd, Env>
{
    fn ctx(&self) -> &JitEmitCtx<'mc> {
        self.ctx
    }

    fn fb(&mut self) -> &mut FunctionBuilder<'b> {
        self.fb
    }

    fn import_func(&mut self, spec: &'static ImportSpec) -> ir::FuncRef {
        self.func_imports
            .get_or_panic(self.codegen_env, &mut self.fb.func, spec)
    }

    fn emit_arg_values(&mut self, args: &[&InstrBlockPy]) -> Vec<(ir::Value, bool)> {
        let mut arg_values = Vec::with_capacity(args.len());
        for arg in args {
            let borrowed_arg = codegen_expr_pyobject_input_is_borrowed_from_local_env(
                arg,
                &*self.local_env,
                self.ctx,
            );
            let value = emit_codegen_expr_with_local_env(
                self.fb,
                arg,
                &mut *self.local_env,
                self.ctx,
                borrowed_arg,
                self.codegen_env,
                self.func_imports,
            );
            arg_values.push((value, borrowed_arg));
        }
        arg_values
    }

    fn emit_owned_bool_from_i32_result(&mut self, result: ir::Value) -> ir::Value {
        emit_owned_bool_from_i32_result(self.fb, result, self.ctx)
    }

    fn emit_owned_bool_from_cond(&mut self, cond: ir::Value) -> ir::Value {
        emit_owned_bool_from_cond(self.fb, cond, self.ctx)
    }

    fn emit_i32_bool01_from_pyobject_truthiness(
        &mut self,
        value: ir::Value,
        facts: PyObjFacts,
        borrowed: bool,
        invert: bool,
    ) -> ir::Value {
        let is_true_ref = self.func_imports.get_or_panic(
            self.codegen_env,
            &mut self.fb.func,
            &DP_JIT_IS_TRUE_IMPORT,
        );
        let mut truth = emit_truthy_from_pyobject_value(
            self.fb,
            value,
            facts,
            is_true_ref,
            self.ctx,
            !borrowed,
        );
        if invert {
            truth = emit_i32_bool01_not(self.fb, truth, self.ctx);
        }
        truth.expect_i32_bool01("PyObject truthiness")
    }

    fn emit_owned_bool_from_pyobject_truthiness(
        &mut self,
        value: ir::Value,
        facts: PyObjFacts,
        borrowed: bool,
        invert: bool,
    ) -> ir::Value {
        let is_true_ref = self.func_imports.get_or_panic(
            self.codegen_env,
            &mut self.fb.func,
            &DP_JIT_IS_TRUE_IMPORT,
        );
        emit_owned_bool_from_pyobject_truthiness(
            self.fb,
            value,
            facts,
            borrowed,
            invert,
            is_true_ref,
            self.ctx,
        )
    }

    fn emit_type_ptr_value(&mut self, owner_type_ref: &RelocTypeRef) -> Option<ir::Value> {
        emit_type_ptr_value_for_ref(self.fb, self.codegen_env, self.ctx, owner_type_ref)
            .unwrap_or_else(|err| {
                panic!("failed to bind type symbol during JIT codegen: {err}");
            })
    }

    fn py_facts_for_arg(&self, arg: &InstrBlockPy) -> PyObjFacts {
        py_facts_for_codegen_expr_with_local_env(arg, self.local_env, self.ctx)
            .unwrap_or_else(PyObjFacts::unknown)
    }

    fn prepare_guard_miss_dispatch_for_instr(
        &mut self,
        instr_id: InstrId,
        pre_guard_operands: &[&InstrBlockPy],
        fallback_block: ir::Block,
    ) -> JitGuardMissDispatch {
        let guard_miss_resume_point =
            self.ctx
                .guard_miss_resume_point
                .unwrap_or(LocalEnvResumePoint::BeforeInstr {
                    key: InstrKey::new(self.ctx.function_id, instr_id),
                });
        prepare_optional_guard_miss_dispatch(
            self.ctx.guard_miss_target_for_codegen_resume_point(
                guard_miss_resume_point,
                pre_guard_operands,
                fallback_block,
            ),
            fallback_block,
            self.ctx.guard_miss_deopt_ref_for_instr_id(instr_id),
        )
    }

    fn emit_deopt_resume_result(
        &mut self,
        target: JitDeoptExitRef,
        deopt_resume_ref: ir::FuncRef,
    ) -> ir::Value {
        let (live_values_base, live_value_count) =
            emit_deopt_live_value_buffer(self.fb, target, self.ctx, self.local_env)
                .unwrap_or_else(|err| panic!("{err}"));
        emit_deopt_resume_call(
            self.fb,
            target,
            deopt_resume_ref,
            self.ctx.consts.block_const,
            live_values_base,
            live_value_count,
            self.ctx.consts.ptr_ty,
            self.ctx.consts.i64_ty,
        )
    }
}

impl<'a, 'b, 'mc, 'c, 'd, Env: JitCodegenEnv> intrinsics::OperationEmitState<'b, InstrTyped>
    for LocalEnvCodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd, Env>
{
    fn ctx(&self) -> &JitEmitCtx<'mc> {
        self.ctx
    }

    fn fb(&mut self) -> &mut FunctionBuilder<'b> {
        self.fb
    }

    fn import_func(&mut self, spec: &'static ImportSpec) -> ir::FuncRef {
        self.func_imports
            .get_or_panic(self.codegen_env, &mut self.fb.func, spec)
    }

    fn emit_arg_values(&mut self, args: &[&InstrTyped]) -> Vec<(ir::Value, bool)> {
        let mut arg_values = Vec::with_capacity(args.len());
        for arg in args {
            let borrowed_arg = typed_expr_pyobject_input_is_borrowed_from_local_env(
                arg,
                &*self.local_env,
                self.ctx,
            );
            let (value, ownership, _) = emit_typed_pyobject_value_with_local_env(
                self.fb,
                arg,
                &mut *self.local_env,
                self.ctx,
                borrowed_arg,
                self.codegen_env,
                self.func_imports,
                "typed intrinsic PyObject argument",
            )
            .unwrap_or_else(|err| panic!("{err}"));
            let transfers_owned_temp = self
                .owned_transfer_temp_load
                .is_some_and(|location| typed_expr_local_load_location(arg) == Some(location));
            arg_values.push((value, !transfers_owned_temp && !ownership.is_owned()));
        }
        arg_values
    }

    fn can_emit_guarded_i64_index_arg(&self, arg: &InstrTyped) -> bool {
        typed_expr_can_emit_guarded_i64_index(arg, self.local_env, self.ctx)
    }

    fn emit_guarded_i64_index_arg(
        &mut self,
        arg: &InstrTyped,
        guard_miss_block: ir::Block,
    ) -> Option<ir::Value> {
        emit_typed_guarded_i64_index_with_local_env(
            self.fb,
            arg,
            self.local_env,
            self.ctx,
            self.codegen_env,
            self.func_imports,
            guard_miss_block,
        )
        .unwrap_or_else(|err| panic!("{err}"))
    }

    fn emit_owned_bool_from_i32_result(&mut self, result: ir::Value) -> ir::Value {
        emit_owned_bool_from_i32_result(self.fb, result, self.ctx)
    }

    fn emit_owned_bool_from_cond(&mut self, cond: ir::Value) -> ir::Value {
        emit_owned_bool_from_cond(self.fb, cond, self.ctx)
    }

    fn emit_i32_bool01_from_pyobject_truthiness(
        &mut self,
        value: ir::Value,
        facts: PyObjFacts,
        borrowed: bool,
        invert: bool,
    ) -> ir::Value {
        let is_true_ref = self.func_imports.get_or_panic(
            self.codegen_env,
            &mut self.fb.func,
            &DP_JIT_IS_TRUE_IMPORT,
        );
        let mut truth = emit_truthy_from_pyobject_value(
            self.fb,
            value,
            facts,
            is_true_ref,
            self.ctx,
            !borrowed,
        );
        if invert {
            truth = emit_i32_bool01_not(self.fb, truth, self.ctx);
        }
        truth.expect_i32_bool01("PyObject truthiness")
    }

    fn emit_owned_bool_from_pyobject_truthiness(
        &mut self,
        value: ir::Value,
        facts: PyObjFacts,
        borrowed: bool,
        invert: bool,
    ) -> ir::Value {
        let is_true_ref = self.func_imports.get_or_panic(
            self.codegen_env,
            &mut self.fb.func,
            &DP_JIT_IS_TRUE_IMPORT,
        );
        emit_owned_bool_from_pyobject_truthiness(
            self.fb,
            value,
            facts,
            borrowed,
            invert,
            is_true_ref,
            self.ctx,
        )
    }

    fn emit_type_ptr_value(&mut self, owner_type_ref: &RelocTypeRef) -> Option<ir::Value> {
        emit_type_ptr_value_for_ref(self.fb, self.codegen_env, self.ctx, owner_type_ref)
            .unwrap_or_else(|err| {
                panic!("failed to bind type symbol during JIT codegen: {err}");
            })
    }

    fn py_facts_for_arg(&self, arg: &InstrTyped) -> PyObjFacts {
        py_facts_for_typed_expr_with_local_env(arg, self.local_env)
            .unwrap_or_else(PyObjFacts::unknown)
    }

    fn prepare_guard_miss_dispatch_for_instr(
        &mut self,
        instr_id: InstrId,
        pre_guard_operands: &[&InstrTyped],
        fallback_block: ir::Block,
    ) -> JitGuardMissDispatch {
        let guard_miss_resume_point =
            self.ctx
                .guard_miss_resume_point
                .unwrap_or(LocalEnvResumePoint::BeforeInstr {
                    key: InstrKey::new(self.ctx.function_id, instr_id),
                });
        prepare_optional_guard_miss_dispatch(
            self.ctx.guard_miss_target_for_typed_resume_point(
                guard_miss_resume_point,
                pre_guard_operands,
                fallback_block,
            ),
            fallback_block,
            self.ctx.guard_miss_deopt_ref_for_instr_id(instr_id),
        )
    }

    fn emit_deopt_resume_result(
        &mut self,
        target: JitDeoptExitRef,
        deopt_resume_ref: ir::FuncRef,
    ) -> ir::Value {
        let (live_values_base, live_value_count) =
            emit_deopt_live_value_buffer(self.fb, target, self.ctx, self.local_env)
                .unwrap_or_else(|err| panic!("{err}"));
        emit_deopt_resume_call(
            self.fb,
            target,
            deopt_resume_ref,
            self.ctx.consts.block_const,
            live_values_base,
            live_value_count,
            self.ctx.consts.ptr_ty,
            self.ctx.consts.i64_ty,
        )
    }
}

fn local_binding_facts_for_stored_value(ref_kind: LocalRefKind) -> ParamBindingFacts {
    if ref_kind == LocalRefKind::Unbound {
        return ParamBindingFacts::MaybeUnbound;
    }
    ParamBindingFacts::DefinitelyBound
}

#[derive(Debug, Clone)]
enum LocalEnvEdgePrepError {
    MissingSourceBinding {
        source_name: String,
    },
    ExpectedScalarSource {
        source_name: String,
        target_name: String,
        repr: RuntimeBlockParamRepr,
    },
    UnsupportedScalarConstantArg {
        target_name: String,
        source: BlockArg,
    },
    UnsupportedCurrentExceptionArg,
}

impl std::fmt::Display for LocalEnvEdgePrepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSourceBinding { source_name } => {
                write!(
                    f,
                    "missing LocalEnv binding for block-arg source {source_name}"
                )
            }
            Self::ExpectedScalarSource {
                source_name,
                target_name,
                repr,
            } => {
                write!(
                    f,
                    "block arg {target_name} expected scalar {repr:?} source {source_name}"
                )
            }
            Self::UnsupportedScalarConstantArg {
                target_name,
                source,
            } => {
                write!(
                    f,
                    "block arg {target_name} expected scalar value but source {source:?} is not scalar"
                )
            }
            Self::UnsupportedCurrentExceptionArg => {
                write!(
                    f,
                    "unexpected current-exception block arg in LocalEnv edge prep"
                )
            }
        }
    }
}

fn emit_forwarded_block_arg_source_value(
    fb: &mut FunctionBuilder<'_>,
    source_name: &str,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
    forwarded_local_counts: &mut HashMap<usize, usize>,
) -> Result<(ir::Value, Option<usize>), LocalEnvEdgePrepError> {
    if let Some(value_index) = local_env.entry_index_for_block_arg_name(source_name) {
        let entry = &local_env.entries[value_index];
        let forwarded_count = forwarded_local_counts.entry(value_index).or_insert(0usize);
        let value = emit_local_env_entry_pyobject_for_forward(fb, entry, ctx, *forwarded_count);
        *forwarded_count += 1;
        return Ok((value, Some(value_index)));
    }
    if let Some(slot) = ctx.stack_slots.slot_for_block_arg_name(source_name) {
        let value = fb.ins().stack_load(ctx.consts.ptr_ty, slot, 0);
        if !ctx.stack_slots.has_cleanup_root_name(source_name) {
            ctx.emit_incref_for_family(fb, value, None, RefcountFamily::ForwardedValueClone);
        }
        return Ok((value, None));
    }
    Err(LocalEnvEdgePrepError::MissingSourceBinding {
        source_name: source_name.to_string(),
    })
}

fn emit_forwarded_block_arg_source_i64_value(
    source_name: &str,
    target_name: &str,
    local_env: &LocalEnv,
    forwarded_locations: &mut HashSet<LocalLocation>,
) -> Result<ir::Value, LocalEnvEdgePrepError> {
    let Some(value_index) = local_env.entry_index_for_block_arg_name(source_name) else {
        return Err(LocalEnvEdgePrepError::ExpectedScalarSource {
            source_name: source_name.to_string(),
            target_name: target_name.to_string(),
            repr: RuntimeBlockParamRepr::ExactI64,
        });
    };
    let entry = &local_env.entries[value_index];
    if entry.i64_facts().is_none() {
        return Err(LocalEnvEdgePrepError::ExpectedScalarSource {
            source_name: source_name.to_string(),
            target_name: target_name.to_string(),
            repr: RuntimeBlockParamRepr::ExactI64,
        });
    }
    if let Some(location) = entry.location {
        forwarded_locations.insert(location);
    }
    Ok(entry.value())
}

fn emit_forwarded_block_arg_source_i32_bool01_value(
    source_name: &str,
    target_name: &str,
    local_env: &LocalEnv,
    forwarded_locations: &mut HashSet<LocalLocation>,
) -> Result<ir::Value, LocalEnvEdgePrepError> {
    let Some(value_index) = local_env.entry_index_for_block_arg_name(source_name) else {
        return Err(LocalEnvEdgePrepError::ExpectedScalarSource {
            source_name: source_name.to_string(),
            target_name: target_name.to_string(),
            repr: RuntimeBlockParamRepr::I32Bool01,
        });
    };
    let entry = &local_env.entries[value_index];
    if entry.i32_bool01_facts().is_none() {
        return Err(LocalEnvEdgePrepError::ExpectedScalarSource {
            source_name: source_name.to_string(),
            target_name: target_name.to_string(),
            repr: RuntimeBlockParamRepr::I32Bool01,
        });
    }
    if let Some(location) = entry.location {
        forwarded_locations.insert(location);
    }
    Ok(entry.value())
}

fn emit_checked_local_value_or_unbound(
    fb: &mut FunctionBuilder<'_>,
    name: &str,
    value: ir::Value,
    ref_kind: LocalRefKind,
    ctx: &JitEmitCtx<'_>,
    borrowed: bool,
) -> ir::Value {
    if is_try_abrupt_kind_name(name) {
        let null_ptr = fb.ins().iconst(ctx.consts.ptr_ty, 0);
        let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
        let fallthrough_block = fb.create_block();
        let value_ok_block = fb.create_block();
        let done_block = fb.create_block();
        fb.append_block_param(done_block, ctx.consts.ptr_ty);
        fb.ins()
            .brif(value_is_null, fallthrough_block, &[], value_ok_block, &[]);

        fb.switch_to_block(fallthrough_block);
        let fallthrough_tag = abrupt_kind_tag(AbruptKind::Fallthrough);
        let fallthrough_i64 = fb.ins().iconst(ctx.consts.i64_ty, fallthrough_tag);
        let fallthrough_value = emit_to_python_long(
            fb,
            SoacValue::i64(fallthrough_i64, IntFacts::i64_known(fallthrough_tag)),
            ctx.py_long_from_i64_ref,
            ctx,
        )
        .expect_pyobject("abrupt kind fallthrough materialize")
        .0;
        if !borrowed {
            ctx.emit_incref_for_family(
                fb,
                fallthrough_value,
                Some(PyObjFacts::unknown().with_non_null_ref()),
                RefcountFamily::LocalLoadClone,
            );
        }
        fb.ins()
            .jump(done_block, &[ir::BlockArg::Value(fallthrough_value)]);

        fb.switch_to_block(value_ok_block);
        if local_ref_kind_needs_incref_for_load(ref_kind, borrowed) {
            ctx.emit_incref_for_family(
                fb,
                value,
                Some(PyObjFacts::unknown().with_non_null_ref()),
                RefcountFamily::LocalLoadClone,
            );
        }
        fb.ins().jump(done_block, &[ir::BlockArg::Value(value)]);

        fb.switch_to_block(done_block);
        return fb.block_params(done_block)[0];
    }
    let null_ptr = fb.ins().iconst(ctx.consts.ptr_ty, 0);
    let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
    let unbound_block = fb.create_block();
    let value_ok_block = fb.create_block();
    fb.append_block_param(value_ok_block, ctx.consts.ptr_ty);
    fb.ins().brif(
        value_is_null,
        unbound_block,
        &[],
        value_ok_block,
        &[ir::BlockArg::Value(value)],
    );

    fb.switch_to_block(unbound_block);
    let name_obj = emit_owned_module_constant(
        fb,
        ctx.module_constants.require_unicode_constant_id(name),
        ctx,
    );
    tracing::info!(
        target: "soac_unbound_local_codegen",
        function_id = ?ctx.function_id,
        name,
        borrowed,
        ref_kind = ?ref_kind,
        "emit_local_load_unbound_path",
    );
    fb.ins()
        .call(ctx.raise_unbound_local_error_ref, &[name_obj]);
    emit_release_owned_inputs(fb, ctx, &[name_obj]);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(value_ok_block);
    let value = fb.block_params(value_ok_block)[0];
    if local_ref_kind_needs_incref_for_load(ref_kind, borrowed) {
        ctx.emit_incref_for_family(
            fb,
            value,
            Some(PyObjFacts::unknown().with_non_null_ref()),
            RefcountFamily::LocalLoadClone,
        );
    }
    value
}

fn is_try_exception_alias_name(name: &str) -> bool {
    name.starts_with("_dp_try_exc_")
}

fn is_try_abrupt_kind_name(name: &str) -> bool {
    name.starts_with("_dp_try_abrupt_kind_")
}

fn is_try_abrupt_payload_name(name: &str) -> bool {
    name.starts_with("_dp_try_abrupt_payload_")
}

fn can_release_via_stack_slot_fallback(name: &str) -> bool {
    is_try_exception_alias_name(name)
        || is_try_abrupt_kind_name(name)
        || is_try_abrupt_payload_name(name)
}

fn block_arg_values(values: &[ir::Value]) -> Vec<ir::BlockArg> {
    values.iter().copied().map(ir::BlockArg::Value).collect()
}

struct PendingLocalFailureCleanup {
    block: ir::Block,
    cleanup_arg_count: usize,
    cleanup_actions: Vec<PendingLocalFailureCleanupAction>,
    continuation: PendingLocalFailureContinuation,
}

#[derive(Clone)]
enum PendingLocalFailureCleanupAction {
    Decref,
    RetireFrameRoot {
        name: String,
        ref_kind: LocalRefKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PendingLocalFailureCleanupActionKey {
    Decref,
    RetireFrameRoot { name: String },
}

impl PendingLocalFailureCleanupAction {
    fn key(&self) -> PendingLocalFailureCleanupActionKey {
        match self {
            Self::Decref => PendingLocalFailureCleanupActionKey::Decref,
            Self::RetireFrameRoot { name, .. } => {
                PendingLocalFailureCleanupActionKey::RetireFrameRoot { name: name.clone() }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PendingLocalFailureContinuation {
    CleanupNull(ir::Block),
    ExceptionDispatch(ir::Block),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum LocalFailureCleanupKey {
    Exact {
        cleanup_values: Vec<ir::Value>,
        cleanup_actions: Vec<PendingLocalFailureCleanupActionKey>,
        forwarded_values: Vec<ir::Value>,
        continuation: PendingLocalFailureContinuation,
    },
    CleanupNullLocals {
        cleanup_keys: Vec<LocalFailureCleanupValueKey>,
        cleanup_null_block: ir::Block,
    },
}

impl LocalFailureCleanupKey {
    fn new(
        cleanup_values: &[LocalFailureCleanupValue],
        cleanup_actions: &[PendingLocalFailureCleanupAction],
        forwarded_values: &[ir::Value],
        continuation: PendingLocalFailureContinuation,
    ) -> LocalFailureCleanupKey {
        match continuation {
            PendingLocalFailureContinuation::CleanupNull(cleanup_null_block)
                if forwarded_values.is_empty() =>
            {
                LocalFailureCleanupKey::CleanupNullLocals {
                    cleanup_keys: cleanup_values
                        .iter()
                        .map(|cleanup_value| cleanup_value.key.clone())
                        .collect(),
                    cleanup_null_block,
                }
            }
            _ => LocalFailureCleanupKey::Exact {
                cleanup_values: cleanup_values
                    .iter()
                    .map(|cleanup_value| cleanup_value.value)
                    .collect(),
                cleanup_actions: cleanup_actions
                    .iter()
                    .map(PendingLocalFailureCleanupAction::key)
                    .collect(),
                forwarded_values: forwarded_values.to_vec(),
                continuation,
            },
        }
    }
}

fn step_null_block_args(ctx: &JitEmitCtx<'_>) -> Vec<ir::BlockArg> {
    block_arg_values(&ctx.consts.step_null_args)
}

fn emit_release_owned_inputs(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    owned_inputs: &[ir::Value],
) {
    // `ctx.decref_ref` lowers to the runtime decref helper, which already preserves the
    // currently raised exception across any object deallocation it triggers. Error paths can
    // therefore release owned temporaries directly before jumping to `step_null`.
    for owned_input in owned_inputs {
        with_refcount_family(fb, Some(RefcountFamily::OwnedTemporary), |fb| {
            fb.ins().call(
                ctx.decref_ref,
                &[ctx.consts.thread_state_value, *owned_input],
            );
        });
    }
}

fn emit_decref_owned_inputs_after_nullable_result(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    result: ir::Value,
    owned_inputs: &[ir::Value],
) -> ir::Value {
    emit_release_owned_inputs(fb, ctx, owned_inputs);
    result
}

fn emit_nullable_pyobject_call_with_cleanup(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    func_ref: ir::FuncRef,
    args: &[ir::Value],
    owned_inputs: &[ir::Value],
) -> ir::Value {
    let call_inst = fb.ins().call(func_ref, args);
    emit_decref_owned_inputs_after_nullable_result(
        fb,
        ctx,
        fb.inst_results(call_inst)[0],
        owned_inputs,
    )
}

fn emit_checked_owned_pyobject_result_for_demand(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    facts: PyObjFacts,
    ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let facts = facts.with_non_null_ref();
    match demand {
        ResultDemand::EffectOnly => {
            let null_ptr = fb.ins().iconst(ctx.consts.ptr_ty, 0);
            let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
            let value_ok_block = fb.create_block();
            fb.ins().brif(
                value_is_null,
                ctx.consts.step_null_block,
                &step_null_block_args(ctx),
                value_ok_block,
                &[],
            );
            fb.switch_to_block(value_ok_block);
            if !facts.is_immortal() {
                ctx.refcount_emitter()
                    .with_family(RefcountFamily::OwnedTemporary)
                    .emit_decref(fb, value, Some(facts));
            }
            EmitResult::no_value()
        }
        ResultDemand::PyObject { .. } => {
            let value = emit_checked_owned_pyobject_result(fb, value, ctx);
            EmitResult::owned_pyobject(value, facts)
        }
        ResultDemand::I32Bool01 => {
            panic!("owned PyObject result helper cannot satisfy I32Bool01 demand")
        }
        ResultDemand::I64 => {
            panic!("owned PyObject result helper cannot satisfy I64 demand")
        }
        ResultDemand::I64Index => {
            panic!("owned PyObject result helper cannot satisfy I64Index demand")
        }
    }
}

fn emit_checked_owned_pyobject_call_result_with_cleanup(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    func_ref: ir::FuncRef,
    args: &[ir::Value],
    owned_inputs: &[ir::Value],
    demand: ResultDemand,
    facts: PyObjFacts,
) -> EmitResult {
    let value = emit_nullable_pyobject_call_with_cleanup(fb, ctx, func_ref, args, owned_inputs);
    emit_checked_owned_pyobject_result_for_demand(fb, value, facts, ctx, demand)
}

fn emit_checked_owned_pyobject_call_with_cleanup(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    func_ref: ir::FuncRef,
    args: &[ir::Value],
    owned_inputs: &[ir::Value],
) -> ir::Value {
    let result = emit_checked_owned_pyobject_call_result_with_cleanup(
        fb,
        ctx,
        func_ref,
        args,
        owned_inputs,
        ResultDemand::PYOBJECT_OWNED,
        PyObjFacts::unknown(),
    );
    let (value, ownership, _) = result.expect_pyobject("checked owned PyObject call");
    debug_assert!(ownership.is_owned());
    value
}

fn emit_checked_owned_pyobject_call_value_with_cleanup(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    func_ref: ir::FuncRef,
    args: &[ir::Value],
    owned_inputs: &[ir::Value],
    facts: PyObjFacts,
) -> SoacValue {
    let value =
        emit_checked_owned_pyobject_call_with_cleanup(fb, ctx, func_ref, args, owned_inputs);
    SoacValue::pyobject(value, facts)
}

fn emit_owned_module_constant_from_parts(
    fb: &mut FunctionBuilder<'_>,
    constant_id: ModuleConstantId,
    module_constant_object_globals: &[ir::GlobalValue],
    ptr_ty: ir::Type,
    access_table: &ModuleConstantAccessTable,
) -> ir::Value {
    let object_global = module_constant_object_globals
        .get(constant_id.0)
        .copied()
        .unwrap_or_else(|| panic!("missing module constant object {}", constant_id.0));
    let symbol_value = fb.ins().global_value(ptr_ty, object_global);
    match access_table.access(constant_id) {
        ModuleConstantAccess::SymbolAddress => symbol_value,
        ModuleConstantAccess::PointerSlot => {
            fb.ins()
                .load(ptr_ty, ir::MemFlags::trusted(), symbol_value, 0)
        }
    }
}

fn emit_owned_module_constant(
    fb: &mut FunctionBuilder<'_>,
    constant_id: ModuleConstantId,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    emit_owned_module_constant_from_parts(
        fb,
        constant_id,
        &ctx.consts.module_constant_object_globals,
        ctx.consts.ptr_ty,
        &ctx.consts.module_constant_accesses,
    )
}

fn emit_none_const(fb: &mut FunctionBuilder<'_>, ctx: &JitEmitCtx<'_>) -> ir::Value {
    emit_owned_module_constant(fb, ctx.consts.none_constant_id, ctx)
}

fn emit_true_const(fb: &mut FunctionBuilder<'_>, ctx: &JitEmitCtx<'_>) -> ir::Value {
    emit_owned_module_constant(fb, ctx.consts.true_constant_id, ctx)
}

fn emit_false_const(fb: &mut FunctionBuilder<'_>, ctx: &JitEmitCtx<'_>) -> ir::Value {
    emit_owned_module_constant(fb, ctx.consts.false_constant_id, ctx)
}

fn emit_empty_tuple_const(fb: &mut FunctionBuilder<'_>, ctx: &JitEmitCtx<'_>) -> ir::Value {
    emit_owned_module_constant(fb, ctx.consts.empty_tuple_constant_id, ctx)
}

fn placeholder_module_constant_ptrs(count: usize) -> Vec<*mut ffi::PyObject> {
    (0..count)
        .map(|index| (0x1000usize + index * 0x10) as *mut ffi::PyObject)
        .collect()
}

fn emit_increment_counter(
    fb: &mut FunctionBuilder<'_>,
    counter_id: CounterId,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let counter_slot = scalar_counter_slot_for_id(ctx.counter_slots_by_id, counter_id)
        .unwrap_or_else(|err| panic!("{err}"));
    let scalar_counter_base_value = ctx.consts.scalar_counter_base_value.unwrap_or_else(|| {
        panic!(
            "missing scalar counter base for counter id {}",
            counter_id.0
        )
    });
    emit_increment_counter_slot(fb, scalar_counter_base_value, counter_slot);
    // TODO: Split codegen instructions into value-producing vs non-value-producing ops
    // and elide retain/release work when a statement result is not consumed.
    let none_const = emit_none_const(fb, ctx);
    ctx.emit_incref_for_family(
        fb,
        none_const,
        Some(PyObjFacts::none_singleton()),
        RefcountFamily::ConstantClone,
    );
    none_const
}

fn emit_increment_counter_ref(
    fb: &mut FunctionBuilder<'_>,
    counter_ref: CounterRef,
    ctx: &JitEmitCtx<'_>,
) {
    let counter_slot = scalar_counter_slot_for_ref(ctx.counter_slots_by_id, counter_ref)
        .unwrap_or_else(|err| panic!("{err}"));
    let scalar_counter_base_value = ctx.consts.scalar_counter_base_value.unwrap_or_else(|| {
        panic!(
            "missing scalar counter base for counter id {}",
            counter_ref.counter_id.0
        )
    });
    emit_increment_counter_slot(fb, scalar_counter_base_value, counter_slot);
}

fn emit_increment_counter_ref_from_parts(
    fb: &mut FunctionBuilder<'_>,
    counter_ref: CounterRef,
    counter_slots_by_id: &[CounterRuntimeSlot],
    scalar_counter_base_value: Option<ir::Value>,
) {
    let counter_slot = scalar_counter_slot_for_ref(counter_slots_by_id, counter_ref)
        .unwrap_or_else(|err| panic!("{err}"));
    let scalar_counter_base_value = scalar_counter_base_value.unwrap_or_else(|| {
        panic!(
            "missing scalar counter base for counter id {}",
            counter_ref.counter_id.0
        )
    });
    emit_increment_counter_slot(fb, scalar_counter_base_value, counter_slot);
}

fn emit_refcount_decref_location_counter_from_parts(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    local: &RefcountLocal,
    reason: &RefcountReleaseReason,
    counter_refs: &HashMap<String, CounterRef>,
    counter_slots_by_id: &[CounterRuntimeSlot],
    scalar_counter_base_value: Option<ir::Value>,
) {
    let branch_name = refcount_release_location_branch_name(source_label, local, reason);
    if let Some(counter_ref) = counter_refs.get(branch_name.as_str()).copied() {
        emit_increment_counter_ref_from_parts(
            fb,
            counter_ref,
            counter_slots_by_id,
            scalar_counter_base_value,
        );
    }
}

fn emit_refcount_decref_location_counter_branch_from_parts(
    fb: &mut FunctionBuilder<'_>,
    branch_name: &str,
    parts: RefcountDecrefLocationCounterParts<'_>,
) {
    if let Some(counter_ref) = parts.counter_refs.get(branch_name).copied() {
        emit_increment_counter_ref_from_parts(
            fb,
            counter_ref,
            parts.counter_slots_by_id,
            parts.scalar_counter_base_value,
        );
    }
}

fn emit_refcount_stack_slot_decref_location_counter(
    fb: &mut FunctionBuilder<'_>,
    purpose: &str,
    slot_index: usize,
    name: &str,
    parts: RefcountDecrefLocationCounterParts<'_>,
) {
    let branch_name = refcount_stack_slot_location_branch_name(purpose, slot_index, name);
    emit_refcount_decref_location_counter_branch_from_parts(fb, branch_name.as_str(), parts);
}

fn emit_refcount_decref_location_counter(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    local: &RefcountLocal,
    reason: &RefcountReleaseReason,
    ctx: &JitEmitCtx<'_>,
) {
    emit_refcount_decref_location_counter_from_parts(
        fb,
        source_label,
        local,
        reason,
        ctx.refcount_decref_location_counter_refs,
        ctx.counter_slots_by_id,
        ctx.consts.scalar_counter_base_value,
    );
}

fn emit_raw_cell_object_for_name_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    name: &ResolvedName,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let Some(location) = name.cell_location() else {
        panic!(
            "raw cell access should target a cell-backed name, got {} at {:?}",
            name.id, name.location
        );
    };
    emit_raw_cell_object_for_location_with_local_env(fb, location, name.id.as_str(), local_env, ctx)
}

fn emit_raw_closure_cell_object_for_slot(
    fb: &mut FunctionBuilder<'_>,
    slot: u32,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let data_slot = ctx
        .function_runtime_data_layout
        .closure_cell_slot(slot as usize);
    let raw_cell_value =
        emit_function_data_slot_borrowed(fb, ctx.consts.function_data_value, data_slot, ptr_ty);
    let raw_cell_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, raw_cell_value, null_ptr);
    let raw_cell_ok_block = fb.create_block();
    fb.append_block_param(raw_cell_ok_block, ptr_ty);
    fb.ins().brif(
        raw_cell_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        raw_cell_ok_block,
        &[ir::BlockArg::Value(raw_cell_value)],
    );
    fb.switch_to_block(raw_cell_ok_block);
    let raw_cell_value = fb.block_params(raw_cell_ok_block)[0];
    ctx.emit_incref_for_family(
        fb,
        raw_cell_value,
        None,
        RefcountFamily::BorrowedResultClone,
    );
    raw_cell_value
}

fn emit_raw_cell_object_for_location_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    location: CellLocation,
    debug_name: &str,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    match location {
        CellLocation::Owned(slot) => {
            let closure_slot = ctx
                .storage_layout
                .as_ref()
                .and_then(|layout| layout.owned_slot(slot))
                .unwrap_or_else(|| {
                    panic!(
                        "missing owned cell slot mapping for {} at local cell slot {}",
                        debug_name, slot
                    )
                });
            let mut candidate_names = vec![closure_slot.storage_name.as_str()];
            if closure_slot.logical_name != closure_slot.storage_name {
                candidate_names.push(closure_slot.logical_name.as_str());
            }
            for candidate_name in &candidate_names {
                if let Some(slot_value) = local_env.load_name(fb, candidate_name, ctx, false) {
                    return slot_value;
                }
            }
            panic!(
                "missing owned cell {} in direct JIT state via names {:?} (slot {slot})",
                debug_name, candidate_names
            );
        }
        CellLocation::Preserved(slot) => {
            let location = PreservedLocation(slot);
            let values = preserved_values_base_value(ctx);
            let slot_offset = preserved_values_slot_offset(slot).unwrap_or_else(|err| {
                panic!(
                    "invalid preserved cell load offset for function {} slot {}: {err}",
                    ctx.function_id, slot
                )
            });
            let value = fb.ins().load(
                ctx.consts.ptr_ty,
                ir::MemFlags::trusted(),
                values,
                slot_offset,
            );
            match preserved_slot_storage_for_location(ctx, location) {
                PreservedSlotStorage::PyCellObject | PreservedSlotStorage::PyObjectOrNull => {
                    let logical_name = ctx
                        .storage_layout
                        .as_ref()
                        .and_then(|layout| layout.preserved_slot(slot))
                        .map(|preserved_slot| preserved_slot.logical_name.as_str())
                        .expect("preserved raw cell should have a storage-layout slot");
                    emit_checked_local_value_or_unbound(
                        fb,
                        logical_name,
                        value,
                        LocalRefKind::Borrowed,
                        ctx,
                        false,
                    )
                }
                PreservedSlotStorage::I64 => panic!(
                    "preserved raw cell slot {} in function {} used scalar storage",
                    slot, ctx.function_id
                ),
            }
        }
        CellLocation::Closure(slot) | CellLocation::CapturedSource(slot) => {
            emit_raw_closure_cell_object_for_slot(fb, slot, ctx)
        }
    }
}

fn emit_function_data_slot_borrowed(
    fb: &mut FunctionBuilder<'_>,
    function_data: ir::Value,
    slot: usize,
    ptr_ty: ir::Type,
) -> ir::Value {
    let offset = slot
        .checked_mul(std::mem::size_of::<usize>())
        .and_then(|offset| i32::try_from(offset).ok())
        .expect("function runtime object slot offset should fit in i32");
    fb.ins()
        .load(ptr_ty, ir::MemFlags::trusted(), function_data, offset)
}

fn emit_pack_current_values_tuple(
    fb: &mut FunctionBuilder<'_>,
    values: &[(ir::Value, bool)],
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    if values.is_empty() {
        let empty_tuple_const = emit_empty_tuple_const(fb, ctx);
        ctx.emit_incref_for_family(
            fb,
            empty_tuple_const,
            Some(PyObjFacts::unknown().with_non_null_ref()),
            RefcountFamily::ConstantClone,
        );
        return empty_tuple_const;
    }

    let ptr_ty = ctx.consts.ptr_ty;
    let i64_ty = ctx.consts.i64_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let tuple_len = fb.ins().iconst(i64_ty, values.len() as i64);
    let tuple_inst = fb.ins().call(ctx.tuple_new_ref, &[tuple_len]);
    let tuple_obj = fb.inst_results(tuple_inst)[0];
    let tuple_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, tuple_obj, null_ptr);
    let tuple_ok_block = fb.create_block();
    fb.append_block_param(tuple_ok_block, ptr_ty);
    fb.ins().brif(
        tuple_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        tuple_ok_block,
        &[ir::BlockArg::Value(tuple_obj)],
    );
    fb.switch_to_block(tuple_ok_block);
    let tuple_obj = fb.block_params(tuple_ok_block)[0];
    let all_borrowed = values.iter().all(|(_, borrowed)| *borrowed);
    let all_owned = values.iter().all(|(_, borrowed)| !*borrowed);

    let slot_size = (values.len() * std::mem::size_of::<u64>()) as u32;
    let stack_slot = fb.create_sized_stack_slot(ir::StackSlotData::new(
        ir::StackSlotKind::ExplicitSlot,
        slot_size,
        0,
    ));
    for (index, (value, _)) in values.iter().copied().enumerate() {
        fb.ins().stack_store(
            value,
            stack_slot,
            (index * std::mem::size_of::<u64>()) as i32,
        );
    }
    let values_base = fb.ins().stack_addr(ptr_ty, stack_slot, 0);
    let borrowed_base = if all_borrowed || all_owned {
        None
    } else {
        let borrowed_stack_slot = fb.create_sized_stack_slot(ir::StackSlotData::new(
            ir::StackSlotKind::ExplicitSlot,
            values.len() as u32,
            0,
        ));
        for (index, (_, borrowed)) in values.iter().copied().enumerate() {
            let borrowed_value = fb.ins().iconst(ir::types::I8, i64::from(borrowed));
            fb.ins()
                .stack_store(borrowed_value, borrowed_stack_slot, index as i32);
        }
        Some(fb.ins().stack_addr(ptr_ty, borrowed_stack_slot, 0))
    };

    let loop_block = fb.create_block();
    fb.append_block_param(loop_block, i64_ty);
    fb.append_block_param(loop_block, ptr_ty);
    let done_block = fb.create_block();
    fb.append_block_param(done_block, ptr_ty);
    let body_block = fb.create_block();
    fb.append_block_param(body_block, i64_ty);
    fb.append_block_param(body_block, ptr_ty);

    let zero_i64 = fb.ins().iconst(i64_ty, 0);
    fb.ins().jump(
        loop_block,
        &[
            ir::BlockArg::Value(zero_i64),
            ir::BlockArg::Value(tuple_obj),
        ],
    );

    fb.switch_to_block(loop_block);
    let loop_index = fb.block_params(loop_block)[0];
    let loop_tuple = fb.block_params(loop_block)[1];
    let at_end = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, loop_index, tuple_len);
    fb.ins().brif(
        at_end,
        done_block,
        &[ir::BlockArg::Value(loop_tuple)],
        body_block,
        &[
            ir::BlockArg::Value(loop_index),
            ir::BlockArg::Value(loop_tuple),
        ],
    );

    fb.switch_to_block(body_block);
    let body_index = fb.block_params(body_block)[0];
    let body_tuple = fb.block_params(body_block)[1];
    let value_offset = fb.ins().ishl_imm(body_index, 3);
    let value_addr = fb.ins().iadd(values_base, value_offset);
    let value = fb.ins().load(ptr_ty, ir::MemFlags::new(), value_addr, 0);
    if all_borrowed {
        ctx.emit_incref_for_family(fb, value, None, RefcountFamily::ForwardedValueClone);
    } else if let Some(borrowed_base) = borrowed_base {
        let borrowed_addr = fb.ins().iadd(borrowed_base, body_index);
        let borrowed_value = fb
            .ins()
            .load(ir::types::I8, ir::MemFlags::new(), borrowed_addr, 0);
        let value_is_borrowed =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::NotEqual, borrowed_value, 0);
        let owned_block = fb.create_block();
        let borrowed_block = fb.create_block();
        fb.ins()
            .brif(value_is_borrowed, borrowed_block, &[], owned_block, &[]);

        fb.switch_to_block(borrowed_block);
        ctx.emit_incref_for_family(fb, value, None, RefcountFamily::ForwardedValueClone);
        fb.ins().jump(owned_block, &[]);

        fb.switch_to_block(owned_block);
    }
    fb.ins()
        .call(ctx.tuple_set_item_ref, &[body_tuple, body_index, value]);
    let next_index = fb.ins().iadd_imm(body_index, 1);
    fb.ins().jump(
        loop_block,
        &[
            ir::BlockArg::Value(next_index),
            ir::BlockArg::Value(body_tuple),
        ],
    );

    fb.switch_to_block(done_block);
    fb.block_params(done_block)[0]
}

fn emit_codegen_tuple_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    tuple: &blockpy_intrinsics::Tuple<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let mut arg_values: Vec<ir::Value> = Vec::with_capacity(tuple.values.len());
    let mut borrowed_args: Vec<bool> = Vec::with_capacity(tuple.values.len());
    for arg in &tuple.values {
        let borrowed_arg =
            codegen_expr_pyobject_input_is_borrowed_from_local_env(arg, local_env, emit_ctx);
        let value = emit_codegen_expr_with_local_env(
            fb,
            arg,
            local_env,
            emit_ctx,
            borrowed_arg,
            codegen_env,
            func_imports,
        );
        arg_values.push(value);
        borrowed_args.push(borrowed_arg);
    }
    let tuple_items = arg_values
        .into_iter()
        .zip(borrowed_args)
        .collect::<Vec<_>>();
    let tuple_value = emit_pack_current_values_tuple(fb, tuple_items.as_slice(), emit_ctx);
    tuple_value
}

fn emit_typed_tuple_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    tuple: &blockpy_intrinsics::Tuple<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let mut arg_values: Vec<ir::Value> = Vec::with_capacity(tuple.values.len());
    let mut borrowed_args: Vec<bool> = Vec::with_capacity(tuple.values.len());
    for arg in &tuple.values {
        let (value, borrowed_arg) = emit_typed_pyobject_input_with_local_env(
            fb,
            arg,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            "typed tuple element",
        )?;
        arg_values.push(value);
        borrowed_args.push(borrowed_arg);
    }
    let tuple_items = arg_values
        .into_iter()
        .zip(borrowed_args)
        .collect::<Vec<_>>();
    let tuple_value = emit_pack_current_values_tuple(fb, tuple_items.as_slice(), emit_ctx);
    Ok(tuple_value)
}

fn emit_call_args_tuple_from_values(
    fb: &mut FunctionBuilder<'_>,
    arg_values: &[(ir::Value, bool)],
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let i64_ty = ctx.consts.i64_ty;
    let tuple_len = fb.ins().iconst(i64_ty, arg_values.len() as i64);
    let tuple_inst = fb.ins().call(ctx.tuple_new_ref, &[tuple_len]);
    let call_args_tuple =
        emit_checked_owned_pyobject_result(fb, fb.inst_results(tuple_inst)[0], ctx);

    for (index, (value, borrowed_arg)) in arg_values.iter().enumerate() {
        if *borrowed_arg {
            ctx.emit_incref_for_family(fb, *value, None, RefcountFamily::ForwardedValueClone);
        }
        let item_index = fb.ins().iconst(i64_ty, index as i64);
        fb.ins().call(
            ctx.tuple_set_item_ref,
            &[call_args_tuple, item_index, *value],
        );
    }

    call_args_tuple
}

fn emit_positional_vectorcall_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrBlockPy],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let (arg_values, arg_borrowed) =
        emit_positional_arg_values(fb, args, local_env, ctx, codegen_env, func_imports);
    let result = emit_positional_vectorcall_result_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        ctx,
        ResultDemand::PYOBJECT_OWNED,
    );
    let (value, ownership, _) = result.expect_pyobject("positional vectorcall result");
    debug_assert!(ownership.is_owned());
    value
}

fn emit_positional_vectorcall_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrBlockPy],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let (arg_values, arg_borrowed) =
        emit_positional_arg_values(fb, args, local_env, ctx, codegen_env, func_imports);
    emit_positional_vectorcall_result_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        ctx,
        demand,
    )
}

fn emit_positional_vectorcall_result_with_arg_values(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    arg_values: Vec<ir::Value>,
    arg_borrowed: Vec<bool>,
    ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    debug_assert_eq!(arg_values.len(), arg_borrowed.len());
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let args_ptr = if arg_values.is_empty() {
        null_ptr
    } else {
        let args_slot = fb.create_sized_stack_slot(ir::StackSlotData::new(
            ir::StackSlotKind::ExplicitSlot,
            (arg_values.len() * std::mem::size_of::<u64>()) as u32,
            0,
        ));
        for (index, value) in arg_values.iter().copied().enumerate() {
            fb.ins().stack_store(
                value,
                args_slot,
                (index * std::mem::size_of::<u64>()) as i32,
            );
        }
        fb.ins().stack_addr(ptr_ty, args_slot, 0)
    };
    let nargsf = fb.ins().iconst(ptr_ty, arg_values.len() as i64);
    let call_inst = fb.ins().call(
        ctx.py_vectorcall_ref,
        &[
            ctx.consts.thread_state_value,
            callable,
            args_ptr,
            nargsf,
            null_ptr,
        ],
    );
    let call_value = fb.inst_results(call_inst)[0];
    emit_checked_positional_call_result_for_demand(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        call_value,
        ctx,
        demand,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_generator_instance_result_with_arg_values(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    arg_values: Vec<ir::Value>,
    arg_borrowed: Vec<bool>,
    ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    debug_assert_eq!(arg_values.len(), arg_borrowed.len());
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let args_ptr = if arg_values.is_empty() {
        null_ptr
    } else {
        let args_slot = fb.create_sized_stack_slot(ir::StackSlotData::new(
            ir::StackSlotKind::ExplicitSlot,
            (arg_values.len() * std::mem::size_of::<u64>()) as u32,
            0,
        ));
        for (index, value) in arg_values.iter().copied().enumerate() {
            fb.ins().stack_store(
                value,
                args_slot,
                (index * std::mem::size_of::<u64>()) as i32,
            );
        }
        fb.ins().stack_addr(ptr_ty, args_slot, 0)
    };
    let nargsf = fb.ins().iconst(ptr_ty, arg_values.len() as i64);
    let make_generator_instance_ref = func_imports.get(
        codegen_env,
        &mut fb.func,
        &DP_JIT_MAKE_GENERATOR_INSTANCE_FROM_VECTORCALL_IMPORT,
    )?;
    let call_inst = fb.ins().call(
        make_generator_instance_ref,
        &[callable, args_ptr, nargsf, null_ptr],
    );
    let call_value = fb.inst_results(call_inst)[0];
    Ok(emit_checked_positional_call_result_for_demand(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        call_value,
        ctx,
        demand,
    ))
}

fn emit_positional_call_three_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrBlockPy],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    debug_assert!(args.len() <= 3);
    let (arg_values, arg_borrowed) =
        emit_positional_arg_values(fb, args, local_env, ctx, codegen_env, func_imports);
    let result = emit_positional_call_three_result_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        ctx,
        ResultDemand::PYOBJECT_OWNED,
    );
    let (value, ownership, _) = result.expect_pyobject("positional call-three result");
    debug_assert!(ownership.is_owned());
    value
}

fn emit_positional_call_three_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrBlockPy],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    demand: ResultDemand,
) -> EmitResult {
    debug_assert!(args.len() <= 3);
    let (arg_values, arg_borrowed) =
        emit_positional_arg_values(fb, args, local_env, ctx, codegen_env, func_imports);
    emit_positional_call_three_result_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        ctx,
        demand,
    )
}

fn emit_positional_arg_values(
    fb: &mut FunctionBuilder<'_>,
    args: &[&InstrBlockPy],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> (Vec<ir::Value>, Vec<bool>) {
    let mut arg_values: Vec<ir::Value> = Vec::with_capacity(args.len());
    let mut arg_borrowed: Vec<bool> = Vec::with_capacity(args.len());
    for arg in args {
        let borrowed_arg =
            codegen_expr_pyobject_input_is_borrowed_from_local_env(arg, local_env, ctx);
        arg_borrowed.push(borrowed_arg);
        arg_values.push(emit_codegen_expr_with_local_env(
            fb,
            arg,
            local_env,
            ctx,
            borrowed_arg,
            codegen_env,
            func_imports,
        ));
    }
    (arg_values, arg_borrowed)
}

fn emit_positional_call_three_result_with_arg_values(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    arg_values: Vec<ir::Value>,
    arg_borrowed: Vec<bool>,
    ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    debug_assert_eq!(arg_values.len(), arg_borrowed.len());
    debug_assert!(arg_values.len() <= 3);
    let null_ptr = fb.ins().iconst(ctx.consts.ptr_ty, 0);
    let arg1 = arg_values.first().copied().unwrap_or(null_ptr);
    let arg2 = arg_values.get(1).copied().unwrap_or(null_ptr);
    let arg3 = arg_values.get(2).copied().unwrap_or(null_ptr);
    let call_inst = fb.ins().call(
        ctx.py_call_positional_three_ref,
        &[
            ctx.consts.thread_state_value,
            callable,
            arg1,
            arg2,
            arg3,
            null_ptr,
        ],
    );
    let call_value = fb.inst_results(call_inst)[0];
    emit_checked_positional_call_result_for_demand(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        call_value,
        ctx,
        demand,
    )
}

fn emit_checked_positional_call_result_for_demand(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    arg_values: Vec<ir::Value>,
    arg_borrowed: Vec<bool>,
    call_value: ir::Value,
    ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let mut owned_inputs =
        Vec::with_capacity(arg_values.len() + usize::from(!callable_is_borrowed));
    for (value, borrowed_arg) in arg_values.into_iter().zip(arg_borrowed.into_iter()) {
        if !borrowed_arg {
            owned_inputs.push(value);
        }
    }
    if !callable_is_borrowed {
        owned_inputs.push(callable);
    }
    let call_value =
        emit_decref_owned_inputs_after_nullable_result(fb, ctx, call_value, &owned_inputs);
    emit_checked_owned_pyobject_result_for_demand(
        fb,
        call_value,
        PyObjFacts::unknown(),
        ctx,
        demand,
    )
}

fn emit_object_call_with_tuple_args_result(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    call_args_tuple: ir::Value,
    kwargs_obj: Option<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let mut owned_inputs = Vec::with_capacity(3);
    if let Some(kwargs_obj) = kwargs_obj {
        owned_inputs.push(kwargs_obj);
    }
    owned_inputs.push(call_args_tuple);
    if !callable_is_borrowed {
        owned_inputs.push(callable);
    }
    let (func_ref, args): (ir::FuncRef, Vec<ir::Value>) = if let Some(kwargs_obj) = kwargs_obj {
        (
            ctx.py_call_with_kw_ref,
            vec![callable, call_args_tuple, kwargs_obj],
        )
    } else {
        (ctx.py_call_object_ref, vec![callable, call_args_tuple])
    };
    emit_checked_owned_pyobject_call_result_with_cleanup(
        fb,
        ctx,
        func_ref,
        args.as_slice(),
        owned_inputs.as_slice(),
        demand,
        PyObjFacts::unknown(),
    )
}

fn emit_checked_runtime_name_object(
    fb: &mut FunctionBuilder<'_>,
    runtime_name: RuntimeName,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let runtime_name_id = runtime_name_id_value(fb, Some(runtime_name));
    emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.load_runtime_obj_by_id_ref,
        &[runtime_name_id],
        &[],
    )
}

fn emit_empty_dict_with_args_tuple(
    fb: &mut FunctionBuilder<'_>,
    empty_args_tuple: ir::Value,
    empty_args_tuple_is_borrowed: bool,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let dict_callable = emit_checked_runtime_name_object(fb, RuntimeName::Dict, ctx);
    let mut owned_inputs = Vec::with_capacity(2);
    if !empty_args_tuple_is_borrowed {
        owned_inputs.push(empty_args_tuple);
    }
    owned_inputs.push(dict_callable);
    emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.py_call_object_ref,
        &[dict_callable, empty_args_tuple],
        owned_inputs.as_slice(),
    )
}

fn emit_one_arg_method_call_and_discard(
    fb: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    method_name: &[u8],
    value_obj: ir::Value,
    value_borrowed: bool,
    ctx: &JitEmitCtx<'_>,
) {
    let null_ptr = fb.ins().iconst(ctx.consts.ptr_ty, 0);
    let method_name_obj = emit_owned_module_constant(
        fb,
        ctx.module_constants
            .require_unicode_constant_id_for_bytes(method_name),
        ctx,
    );
    let method_obj = emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.pyobject_getattr_ref,
        &[receiver, method_name_obj],
        &[method_name_obj],
    );
    let mut owned_inputs = Vec::with_capacity(2);
    if !value_borrowed {
        owned_inputs.push(value_obj);
    }
    owned_inputs.push(method_obj);
    let _ = emit_checked_owned_pyobject_call_result_with_cleanup(
        fb,
        ctx,
        ctx.py_call_positional_three_ref,
        &[
            ctx.consts.thread_state_value,
            method_obj,
            value_obj,
            null_ptr,
            null_ptr,
            null_ptr,
        ],
        owned_inputs.as_slice(),
        ResultDemand::EffectOnly,
        PyObjFacts::unknown(),
    );
}

fn emit_kwargs_setitem_or_cleanup(
    fb: &mut FunctionBuilder<'_>,
    kwargs_obj: ir::Value,
    key_obj: ir::Value,
    value_obj: ir::Value,
    value_borrowed: bool,
    cleanup_on_error: &[ir::Value],
    ctx: &JitEmitCtx<'_>,
) {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let set_inst = fb
        .ins()
        .call(ctx.pyobject_setitem_ref, &[kwargs_obj, key_obj, value_obj]);
    ctx.refcount_emitter()
        .with_family(RefcountFamily::OwnedTemporary)
        .emit_decref(fb, key_obj, None);
    if !value_borrowed {
        ctx.refcount_emitter()
            .with_family(RefcountFamily::OwnedTemporary)
            .emit_decref(fb, value_obj, None);
    }
    let set_value = fb.inst_results(set_inst)[0];
    let set_failed = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, set_value, null_ptr);
    let set_ok = fb.create_block();
    let set_fail = fb.create_block();
    fb.append_block_param(set_fail, ptr_ty);
    fb.ins().brif(
        set_failed,
        set_fail,
        &[ir::BlockArg::Value(kwargs_obj)],
        set_ok,
        &[],
    );
    fb.switch_to_block(set_fail);
    let failed_kwargs = fb.block_params(set_fail)[0];
    emit_release_owned_inputs(fb, ctx, &[failed_kwargs]);
    emit_release_owned_inputs(fb, ctx, cleanup_on_error);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));
    fb.switch_to_block(set_ok);
    ctx.refcount_emitter()
        .with_family(RefcountFamily::OwnedTemporary)
        .emit_decref(fb, set_value, None);
}

fn emit_keyword_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrBlockPy],
    keywords: &[(&str, &InstrBlockPy)],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let result = emit_keyword_call_result_with_local_env(
        fb,
        callable,
        callable_is_borrowed,
        args,
        keywords,
        local_env,
        ctx,
        codegen_env,
        func_imports,
        ResultDemand::PYOBJECT_OWNED,
    );
    let (value, ownership, _) = result.expect_pyobject("keyword call result");
    debug_assert!(ownership.is_owned());
    value
}

#[allow(clippy::too_many_arguments)]
fn emit_keyword_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrBlockPy],
    keywords: &[(&str, &InstrBlockPy)],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let mut tuple_items: Vec<(ir::Value, bool)> = Vec::with_capacity(args.len());
    for arg in args {
        let borrowed_arg =
            codegen_expr_pyobject_input_is_borrowed_from_local_env(arg, local_env, ctx);
        let value = emit_codegen_expr_with_local_env(
            fb,
            arg,
            local_env,
            ctx,
            borrowed_arg,
            codegen_env,
            func_imports,
        );
        tuple_items.push((value, borrowed_arg));
    }
    let call_args_tuple = emit_call_args_tuple_from_values(fb, tuple_items.as_slice(), ctx);

    let empty_tuple_len = fb.ins().iconst(ctx.consts.i64_ty, 0);
    let empty_tuple_inst = fb.ins().call(ctx.tuple_new_ref, &[empty_tuple_len]);
    let empty_tuple =
        emit_checked_owned_pyobject_result(fb, fb.inst_results(empty_tuple_inst)[0], ctx);
    let kwargs_obj = emit_empty_dict_with_args_tuple(fb, empty_tuple, false, ctx);

    for (name, value_expr) in keywords {
        let key_obj = emit_owned_module_constant(
            fb,
            ctx.module_constants.require_unicode_constant_id(name),
            ctx,
        );
        let value_borrowed =
            codegen_expr_pyobject_input_is_borrowed_from_local_env(value_expr, local_env, ctx);
        let value_obj = emit_codegen_expr_with_local_env(
            fb,
            value_expr,
            local_env,
            ctx,
            value_borrowed,
            codegen_env,
            func_imports,
        );
        let mut cleanup_on_error = Vec::with_capacity(2);
        cleanup_on_error.push(call_args_tuple);
        if !callable_is_borrowed {
            cleanup_on_error.push(callable);
        }
        emit_kwargs_setitem_or_cleanup(
            fb,
            kwargs_obj,
            key_obj,
            value_obj,
            value_borrowed,
            cleanup_on_error.as_slice(),
            ctx,
        );
    }

    emit_object_call_with_tuple_args_result(
        fb,
        callable,
        callable_is_borrowed,
        call_args_tuple,
        Some(kwargs_obj),
        ctx,
        demand,
    )
}

fn emit_typed_pyobject_arg_value_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(ir::Value, bool), String> {
    let borrowed = typed_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, ctx);
    let (value, ownership, _) = emit_typed_pyobject_value_with_local_env(
        fb,
        expr,
        local_env,
        ctx,
        borrowed,
        codegen_env,
        func_imports,
        "typed PyObject call argument",
    )?;
    Ok((value, !ownership.is_owned()))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_pyobject_value_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    site: &str,
) -> Result<(ir::Value, ValueOwnership, PyObjFacts), String> {
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        expr,
        local_env,
        ctx,
        borrowed,
        codegen_env,
        func_imports,
    )?;
    let demand = if borrowed {
        ResultDemand::PYOBJECT_BORROWED_OK
    } else {
        ResultDemand::PYOBJECT_OWNED
    };
    Ok(emit_soac_value_result_for_demand(fb, value, ctx, demand, None).expect_pyobject(site))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_keyword_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrTyped],
    keywords: &[(&str, &InstrTyped)],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    demand: ResultDemand,
) -> Result<EmitResult, String> {
    let mut tuple_items: Vec<(ir::Value, bool)> = Vec::with_capacity(args.len());
    for arg in args {
        tuple_items.push(emit_typed_pyobject_arg_value_with_local_env(
            fb,
            arg,
            local_env,
            ctx,
            codegen_env,
            func_imports,
        )?);
    }
    let call_args_tuple = emit_call_args_tuple_from_values(fb, tuple_items.as_slice(), ctx);

    let empty_tuple_len = fb.ins().iconst(ctx.consts.i64_ty, 0);
    let empty_tuple_inst = fb.ins().call(ctx.tuple_new_ref, &[empty_tuple_len]);
    let empty_tuple =
        emit_checked_owned_pyobject_result(fb, fb.inst_results(empty_tuple_inst)[0], ctx);
    let kwargs_obj = emit_empty_dict_with_args_tuple(fb, empty_tuple, false, ctx);

    for (name, value_expr) in keywords {
        let key_obj = emit_owned_module_constant(
            fb,
            ctx.module_constants.require_unicode_constant_id(name),
            ctx,
        );
        let (value_obj, value_borrowed) = emit_typed_pyobject_arg_value_with_local_env(
            fb,
            value_expr,
            local_env,
            ctx,
            codegen_env,
            func_imports,
        )?;
        let mut cleanup_on_error = Vec::with_capacity(2);
        cleanup_on_error.push(call_args_tuple);
        if !callable_is_borrowed {
            cleanup_on_error.push(callable);
        }
        emit_kwargs_setitem_or_cleanup(
            fb,
            kwargs_obj,
            key_obj,
            value_obj,
            value_borrowed,
            cleanup_on_error.as_slice(),
            ctx,
        );
    }

    Ok(emit_object_call_with_tuple_args_result(
        fb,
        callable,
        callable_is_borrowed,
        call_args_tuple,
        Some(kwargs_obj),
        ctx,
        demand,
    ))
}

fn emit_unpack_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[CallArgPositional<InstrBlockPy>],
    keywords: &[CallArgKeyword<InstrBlockPy>],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let result = emit_unpack_call_result_with_local_env(
        fb,
        callable,
        callable_is_borrowed,
        args,
        keywords,
        local_env,
        ctx,
        codegen_env,
        func_imports,
        ResultDemand::PYOBJECT_OWNED,
    );
    let (value, ownership, _) = result.expect_pyobject("unpack call result");
    debug_assert!(ownership.is_owned());
    value
}

#[allow(clippy::too_many_arguments)]
fn emit_unpack_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[CallArgPositional<InstrBlockPy>],
    keywords: &[CallArgKeyword<InstrBlockPy>],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    let list_callable = emit_checked_runtime_name_object(fb, RuntimeName::List, ctx);
    let empty_tuple_const = emit_empty_tuple_const(fb, ctx);
    let args_list = emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.py_call_object_ref,
        &[list_callable, empty_tuple_const],
        &[list_callable],
    );

    let kwargs_obj = if keywords.is_empty() {
        None
    } else {
        let empty_tuple_const = emit_empty_tuple_const(fb, ctx);
        Some(emit_empty_dict_with_args_tuple(
            fb,
            empty_tuple_const,
            true,
            ctx,
        ))
    };

    for arg in args {
        let (value_expr, method_name) = match arg {
            CallArgPositional::Positional(value_expr) => (value_expr, b"append".as_slice()),
            CallArgPositional::Starred(value_expr) => (value_expr, b"extend".as_slice()),
        };
        let value_borrowed =
            codegen_expr_pyobject_input_is_borrowed_from_local_env(value_expr, local_env, ctx);
        let value_obj = emit_codegen_expr_with_local_env(
            fb,
            value_expr,
            local_env,
            ctx,
            value_borrowed,
            codegen_env,
            func_imports,
        );
        emit_one_arg_method_call_and_discard(
            fb,
            args_list,
            method_name,
            value_obj,
            value_borrowed,
            ctx,
        );
    }

    for keyword in keywords {
        match keyword {
            CallArgKeyword::Named { arg, value } => {
                let kwargs_obj = kwargs_obj.expect("kwargs object must exist for named kw part");
                let key_obj = emit_owned_module_constant(
                    fb,
                    ctx.module_constants
                        .require_unicode_constant_id(arg.as_str()),
                    ctx,
                );
                let value_borrowed =
                    codegen_expr_pyobject_input_is_borrowed_from_local_env(value, local_env, ctx);
                let value_obj = emit_codegen_expr_with_local_env(
                    fb,
                    value,
                    local_env,
                    ctx,
                    value_borrowed,
                    codegen_env,
                    func_imports,
                );
                let mut cleanup_on_error = Vec::with_capacity(2);
                cleanup_on_error.push(args_list);
                if !callable_is_borrowed {
                    cleanup_on_error.push(callable);
                }
                emit_kwargs_setitem_or_cleanup(
                    fb,
                    kwargs_obj,
                    key_obj,
                    value_obj,
                    value_borrowed,
                    cleanup_on_error.as_slice(),
                    ctx,
                );
            }
            CallArgKeyword::Starred(value_expr) => {
                let kwargs_obj = kwargs_obj.expect("kwargs object must exist for kwstar part");
                let value_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
                    value_expr, local_env, ctx,
                );
                let value_obj = emit_codegen_expr_with_local_env(
                    fb,
                    value_expr,
                    local_env,
                    ctx,
                    value_borrowed,
                    codegen_env,
                    func_imports,
                );
                emit_one_arg_method_call_and_discard(
                    fb,
                    kwargs_obj,
                    b"update",
                    value_obj,
                    value_borrowed,
                    ctx,
                );
            }
        }
    }

    let tuple_callable = emit_checked_runtime_name_object(fb, RuntimeName::TupleFromIter, ctx);
    let call_args_tuple = emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.py_call_positional_three_ref,
        &[
            ctx.consts.thread_state_value,
            tuple_callable,
            args_list,
            null_ptr,
            null_ptr,
            null_ptr,
        ],
        &[tuple_callable, args_list],
    );

    emit_object_call_with_tuple_args_result(
        fb,
        callable,
        callable_is_borrowed,
        call_args_tuple,
        kwargs_obj,
        ctx,
        demand,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_unpack_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[CallArgPositional<InstrTyped>],
    keywords: &[CallArgKeyword<InstrTyped>],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    demand: ResultDemand,
) -> Result<EmitResult, String> {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    let list_callable = emit_checked_runtime_name_object(fb, RuntimeName::List, ctx);
    let empty_tuple_const = emit_empty_tuple_const(fb, ctx);
    let args_list = emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.py_call_object_ref,
        &[list_callable, empty_tuple_const],
        &[list_callable],
    );

    let kwargs_obj = if keywords.is_empty() {
        None
    } else {
        let empty_tuple_const = emit_empty_tuple_const(fb, ctx);
        Some(emit_empty_dict_with_args_tuple(
            fb,
            empty_tuple_const,
            true,
            ctx,
        ))
    };

    for arg in args {
        let (value_expr, method_name) = match arg {
            CallArgPositional::Positional(value_expr) => (value_expr, b"append".as_slice()),
            CallArgPositional::Starred(value_expr) => (value_expr, b"extend".as_slice()),
        };
        let (value_obj, value_borrowed) = emit_typed_pyobject_arg_value_with_local_env(
            fb,
            value_expr,
            local_env,
            ctx,
            codegen_env,
            func_imports,
        )?;
        emit_one_arg_method_call_and_discard(
            fb,
            args_list,
            method_name,
            value_obj,
            value_borrowed,
            ctx,
        );
    }

    for keyword in keywords {
        match keyword {
            CallArgKeyword::Named { arg, value } => {
                let kwargs_obj = kwargs_obj.expect("kwargs object must exist for named kw part");
                let key_obj = emit_owned_module_constant(
                    fb,
                    ctx.module_constants
                        .require_unicode_constant_id(arg.as_str()),
                    ctx,
                );
                let (value_obj, value_borrowed) = emit_typed_pyobject_arg_value_with_local_env(
                    fb,
                    value,
                    local_env,
                    ctx,
                    codegen_env,
                    func_imports,
                )?;
                let mut cleanup_on_error = Vec::with_capacity(2);
                cleanup_on_error.push(args_list);
                if !callable_is_borrowed {
                    cleanup_on_error.push(callable);
                }
                emit_kwargs_setitem_or_cleanup(
                    fb,
                    kwargs_obj,
                    key_obj,
                    value_obj,
                    value_borrowed,
                    cleanup_on_error.as_slice(),
                    ctx,
                );
            }
            CallArgKeyword::Starred(value_expr) => {
                let kwargs_obj = kwargs_obj.expect("kwargs object must exist for kwstar part");
                let (value_obj, value_borrowed) = emit_typed_pyobject_arg_value_with_local_env(
                    fb,
                    value_expr,
                    local_env,
                    ctx,
                    codegen_env,
                    func_imports,
                )?;
                emit_one_arg_method_call_and_discard(
                    fb,
                    kwargs_obj,
                    b"update",
                    value_obj,
                    value_borrowed,
                    ctx,
                );
            }
        }
    }

    let tuple_callable = emit_checked_runtime_name_object(fb, RuntimeName::TupleFromIter, ctx);
    let call_args_tuple = emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.py_call_positional_three_ref,
        &[
            ctx.consts.thread_state_value,
            tuple_callable,
            args_list,
            null_ptr,
            null_ptr,
            null_ptr,
        ],
        &[tuple_callable, args_list],
    );

    Ok(emit_object_call_with_tuple_args_result(
        fb,
        callable,
        callable_is_borrowed,
        call_args_tuple,
        kwargs_obj,
        ctx,
        demand,
    ))
}

fn emit_owned_bool_from_cond(
    fb: &mut FunctionBuilder<'_>,
    cond: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let truth = emit_i32_bool01_from_cond(fb, cond, ctx);
    let (bool_value, _, _) =
        emit_to_python_bool(fb, truth, ctx).expect_pyobject("bool materialize");
    bool_value
}

fn emit_i32_bool01_from_cond(
    fb: &mut FunctionBuilder<'_>,
    cond: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> SoacValue {
    let zero_i32 = fb.ins().iconst(ctx.consts.i32_ty, 0);
    let one_i32 = fb.ins().iconst(ctx.consts.i32_ty, 1);
    let truth_i32 = fb.ins().select(cond, one_i32, zero_i32);
    SoacValue::i32(truth_i32, IntFacts::i32_bool01())
}

fn emit_i32_bool01_const(
    fb: &mut FunctionBuilder<'_>,
    value: bool,
    ctx: &JitEmitCtx<'_>,
) -> SoacValue {
    let raw = i32::from(value);
    let truth_i32 = fb.ins().iconst(ctx.consts.i32_ty, i64::from(raw));
    SoacValue::i32(truth_i32, IntFacts::i32_known(raw))
}

fn emit_i32_bool01_from_i32_result(
    fb: &mut FunctionBuilder<'_>,
    result: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> SoacValue {
    let is_true = fb.ins().icmp_imm(ir::condcodes::IntCC::NotEqual, result, 0);
    emit_i32_bool01_from_cond(fb, is_true, ctx)
}

fn emit_to_python_bool(
    fb: &mut FunctionBuilder<'_>,
    value: SoacValue,
    ctx: &JitEmitCtx<'_>,
) -> SoacValue {
    let truth_i32 = value.expect_i32_bool01("emit_to_python_bool");
    let is_true = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::NotEqual, truth_i32, 0);
    let true_const = emit_true_const(fb, ctx);
    let false_const = emit_false_const(fb, ctx);
    let bool_value = fb.ins().select(is_true, true_const, false_const);
    SoacValue::immortal_pyobject(bool_value, PyObjFacts::bool_object())
}

fn emit_checked_owned_pyobject_result(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let null_ptr = fb.ins().iconst(ctx.consts.ptr_ty, 0);
    let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
    let value_ok_block = fb.create_block();
    fb.append_block_param(value_ok_block, ctx.consts.ptr_ty);
    fb.ins().brif(
        value_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        value_ok_block,
        &[ir::BlockArg::Value(value)],
    );
    fb.switch_to_block(value_ok_block);
    fb.block_params(value_ok_block)[0]
}

fn emit_to_python_long(
    fb: &mut FunctionBuilder<'_>,
    value: SoacValue,
    py_long_from_i64_ref: ir::FuncRef,
    ctx: &JitEmitCtx<'_>,
) -> SoacValue {
    match value {
        pyobject @ SoacValue::PyObject { .. } => pyobject,
        SoacValue::I64 { value, .. } => emit_checked_owned_pyobject_call_value_with_cleanup(
            fb,
            ctx,
            py_long_from_i64_ref,
            &[value],
            &[],
            PyObjFacts::exact_type(PyExactType::Int),
        ),
        SoacValue::I32 { value, .. } => {
            let value_i64 = fb.ins().sextend(ctx.consts.i64_ty, value);
            emit_checked_owned_pyobject_call_value_with_cleanup(
                fb,
                ctx,
                py_long_from_i64_ref,
                &[value_i64],
                &[],
                PyObjFacts::exact_type(PyExactType::Int),
            )
        }
    }
}

fn emit_i32_bool01_not(
    fb: &mut FunctionBuilder<'_>,
    value: SoacValue,
    ctx: &JitEmitCtx<'_>,
) -> SoacValue {
    let truth_i32 = value.expect_i32_bool01("emit_i32_bool01_not");
    let is_false = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, truth_i32, 0);
    emit_i32_bool01_from_cond(fb, is_false, ctx)
}

fn emit_release_owned_pyobject(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    facts: Option<PyObjFacts>,
    ctx: &JitEmitCtx<'_>,
) {
    if facts.is_some_and(PyObjFacts::is_immortal) {
        return;
    }
    if let Some(facts) = facts {
        ctx.refcount_emitter()
            .with_family(RefcountFamily::OwnedTemporary)
            .emit_decref(fb, value, Some(facts));
    } else {
        ctx.refcount_emitter()
            .with_family(RefcountFamily::OwnedTemporary)
            .emit_decref(fb, value, None);
    }
}

fn emit_release_pyobject_if_owned(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    facts: PyObjFacts,
    owned: bool,
    ctx: &JitEmitCtx<'_>,
) {
    if owned {
        emit_release_owned_pyobject(fb, value, Some(facts), ctx);
    }
}

fn emit_owned_bool_from_i32_result(
    fb: &mut FunctionBuilder<'_>,
    result: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let truth = emit_i32_bool01_from_i32_result_or_error(fb, result, ctx);
    let (bool_value, _, _) =
        emit_to_python_bool(fb, SoacValue::i32(truth, IntFacts::i32_bool01()), ctx)
            .expect_pyobject("bool materialize");
    bool_value
}

fn emit_i32_bool01_from_i32_result_or_error(
    fb: &mut FunctionBuilder<'_>,
    result: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let is_error = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, result, -1);
    let ok_block = fb.create_block();
    fb.ins().brif(
        is_error,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        ok_block,
        &[],
    );
    fb.switch_to_block(ok_block);
    let truth = emit_i32_bool01_from_i32_result(fb, result, ctx);
    truth.expect_i32_bool01("i32 result truthiness")
}

fn emit_owned_bool_from_pyobject_truthiness(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    facts: PyObjFacts,
    borrowed: bool,
    invert: bool,
    is_true_ref: ir::FuncRef,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let mut truth = emit_truthy_from_pyobject_value(fb, value, facts, is_true_ref, ctx, !borrowed);
    if invert {
        truth = emit_i32_bool01_not(fb, truth, ctx);
    }
    let (bool_value, _, _) =
        emit_to_python_bool(fb, truth, ctx).expect_pyobject("truthiness bool materialize");
    bool_value
}

fn call_site_profiled_targets<'a>(
    call: &blockpy_intrinsics::Call<InstrBlockPy>,
    profiled_targets: Option<&'a [RuntimeFunctionId]>,
) -> Option<&'a [RuntimeFunctionId]> {
    let _ = call.try_semantic_instr_id()?;
    profiled_targets.filter(|targets| !targets.is_empty())
}

fn codegen_expr_const_i64(
    expr: &InstrBlockPy,
    module_constants: &ModuleCodegenConstants,
) -> Option<i64> {
    match expr {
        InstrBlockPy::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_i64_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn typed_expr_const_i64(
    expr: &InstrTyped,
    module_constants: &ModuleCodegenConstants,
) -> Option<i64> {
    match expr {
        InstrTyped::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_i64_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn type_ptr_data_id_for_ref(
    codegen_env: &mut impl JitCodegenEnv,
    ctx: &JitEmitCtx<'_>,
    owner_type_ref: &RelocTypeRef,
) -> Result<Option<DataId>, String> {
    if let Some(data_id) = ctx.type_ptr_data_ids.borrow().get(owner_type_ref).copied() {
        return Ok(Some(data_id));
    }
    if !ensure_reloc_type_symbol_registered(owner_type_ref)? {
        return Ok(None);
    }
    let symbol = reloc_type_ref_symbol_name(owner_type_ref);
    let data_id = declare_type_ptr_import(codegen_env, symbol.as_ref())?;
    ctx.type_ptr_data_ids
        .borrow_mut()
        .insert(owner_type_ref.clone(), data_id);
    Ok(Some(data_id))
}

fn emit_type_ptr_value_for_ref(
    fb: &mut FunctionBuilder<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    ctx: &JitEmitCtx<'_>,
    owner_type_ref: &RelocTypeRef,
) -> Result<Option<ir::Value>, String> {
    let Some(data_id) = type_ptr_data_id_for_ref(codegen_env, ctx, owner_type_ref)? else {
        return Ok(None);
    };
    let type_data = codegen_env.codegen_declare_data_in_func(data_id, &mut fb.func)?;
    Ok(Some(fb.ins().global_value(ctx.consts.ptr_ty, type_data)))
}

fn callable_ptr_data_id_for_ref(
    codegen_env: &mut impl JitCodegenEnv,
    ctx: &JitEmitCtx<'_>,
    callable_ref: &RelocCallableRef,
) -> Result<Option<DataId>, String> {
    if let Some(data_id) = ctx
        .callable_ptr_data_ids
        .borrow()
        .get(callable_ref)
        .copied()
    {
        return Ok(Some(data_id));
    }
    let symbol = reloc_callable_ref_symbol_name(callable_ref);
    if lookup_registered_jit_data_symbol(symbol.as_str()).is_none() {
        return Ok(None);
    }
    let data_id = declare_type_ptr_import(codegen_env, symbol.as_str())?;
    ctx.callable_ptr_data_ids
        .borrow_mut()
        .insert(callable_ref.clone(), data_id);
    Ok(Some(data_id))
}

fn emit_callable_ptr_value_for_ref(
    fb: &mut FunctionBuilder<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    ctx: &JitEmitCtx<'_>,
    callable_ref: &RelocCallableRef,
) -> Result<Option<ir::Value>, String> {
    let Some(data_id) = callable_ptr_data_id_for_ref(codegen_env, ctx, callable_ref)? else {
        return Ok(None);
    };
    let callable_data = codegen_env.codegen_declare_data_in_func(data_id, &mut fb.func)?;
    Ok(Some(
        fb.ins().global_value(ctx.consts.ptr_ty, callable_data),
    ))
}

pub(super) fn emit_exact_type_version_match(
    fb: &mut FunctionBuilder<'_>,
    obj: ir::Value,
    expected_type: ir::Value,
    expected_version: u32,
) -> ir::Value {
    let ptr_ty = fb.func.dfg.value_type(obj);
    let actual_type = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyObject, ob_type) as i32,
    );
    let type_matches = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, actual_type, expected_type);
    let actual_version = fb.ins().load(
        ir::types::I32,
        ir::MemFlags::trusted(),
        actual_type,
        offset_of!(ffi::PyTypeObject, tp_version_tag) as i32,
    );
    let version_matches = fb.ins().icmp_imm(
        ir::condcodes::IntCC::Equal,
        actual_version,
        i64::from(expected_version),
    );
    fb.ins().band(type_matches, version_matches)
}

#[repr(C)]
struct RawPyDictSplitValuesForJit {
    capacity: u8,
    size: u8,
    embedded: u8,
    valid: u8,
    values: [*mut ffi::PyObject; 1],
}

#[repr(C)]
struct RawPyASCIIObjectForJit {
    ob_base: ffi::PyObject,
    length: ffi::Py_ssize_t,
    hash: ffi::Py_hash_t,
    state: u32,
}

const MANAGED_DICT_OFFSET: i32 = -3 * (std::mem::size_of::<*mut ffi::PyObject>() as i32);
const SPLIT_VALUES_CAPACITY_OFFSET: i32 = offset_of!(RawPyDictSplitValuesForJit, capacity) as i32;
const SPLIT_VALUES_SIZE_OFFSET: i32 = offset_of!(RawPyDictSplitValuesForJit, size) as i32;
const SPLIT_VALUES_VALID_OFFSET: i32 = offset_of!(RawPyDictSplitValuesForJit, valid) as i32;
const SPLIT_VALUES_VALUES_OFFSET: i32 = offset_of!(RawPyDictSplitValuesForJit, values) as i32;
const PYTYPE_TP_BASICSIZE_OFFSET: i32 = offset_of!(ffi::PyTypeObject, tp_basicsize) as i32;

fn split_values_slot_offset(expected_index: u32) -> Result<i32, String> {
    let slot_offset = i64::from(SPLIT_VALUES_VALUES_OFFSET)
        + i64::from(expected_index)
            * i64::try_from(std::mem::size_of::<*mut ffi::PyObject>())
                .map_err(|_| "pointer size does not fit i64".to_string())?;
    i32::try_from(slot_offset)
        .map_err(|_| format!("indexed field offset {slot_offset} does not fit i32"))
}

#[allow(clippy::too_many_arguments)]
fn emit_trusted_inline_values_field_values(
    fb: &mut FunctionBuilder<'_>,
    obj: ir::Value,
    owner_type: ir::Value,
    expected_index: u32,
    hit_block: ir::Block,
    miss_block: ir::Block,
    emit_ctx: &JitEmitCtx<'_>,
) {
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let i64_ty = emit_ctx.consts.i64_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let values_block = fb.create_block();
    let valid_block = fb.create_block();
    fb.append_block_param(values_block, ptr_ty);
    fb.append_block_param(valid_block, ptr_ty);

    let materialized_dict =
        fb.ins()
            .load(ptr_ty, ir::MemFlags::trusted(), obj, MANAGED_DICT_OFFSET);
    let dict_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, materialized_dict, null_ptr);
    fb.ins().brif(
        dict_is_null,
        values_block,
        &[ir::BlockArg::Value(obj)],
        miss_block,
        &[],
    );

    fb.switch_to_block(values_block);
    let obj = fb.block_params(values_block)[0];
    let basicsize = fb.ins().load(
        i64_ty,
        ir::MemFlags::trusted(),
        owner_type,
        PYTYPE_TP_BASICSIZE_OFFSET,
    );
    let values = fb.ins().iadd(obj, basicsize);
    let valid = fb.ins().load(
        ir::types::I8,
        ir::MemFlags::trusted(),
        values,
        SPLIT_VALUES_VALID_OFFSET,
    );
    let values_are_valid = fb.ins().icmp_imm(ir::condcodes::IntCC::NotEqual, valid, 0);
    fb.ins().brif(
        values_are_valid,
        valid_block,
        &[ir::BlockArg::Value(values)],
        miss_block,
        &[],
    );

    fb.switch_to_block(valid_block);
    let values = fb.block_params(valid_block)[0];
    let capacity = fb.ins().load(
        ir::types::I8,
        ir::MemFlags::trusted(),
        values,
        SPLIT_VALUES_CAPACITY_OFFSET,
    );
    let capacity = fb.ins().uextend(i64_ty, capacity);
    let expected_index_value = fb.ins().iconst(i64_ty, i64::from(expected_index));
    let index_in_capacity = fb.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThan,
        expected_index_value,
        capacity,
    );
    fb.ins().brif(
        index_in_capacity,
        hit_block,
        &[ir::BlockArg::Value(values)],
        miss_block,
        &[],
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_trusted_inline_values_field_probe(
    fb: &mut FunctionBuilder<'_>,
    obj: ir::Value,
    owner_type: ir::Value,
    expected_index: u32,
    hit_block: ir::Block,
    miss_block: ir::Block,
    emit_ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let slot_block = fb.create_block();
    let value_block = fb.create_block();
    fb.append_block_param(slot_block, ptr_ty);
    fb.append_block_param(value_block, ptr_ty);

    emit_trusted_inline_values_field_values(
        fb,
        obj,
        owner_type,
        expected_index,
        slot_block,
        miss_block,
        emit_ctx,
    );

    fb.switch_to_block(slot_block);
    let values = fb.block_params(slot_block)[0];
    let slot_offset = split_values_slot_offset(expected_index)?;
    let value = fb
        .ins()
        .load(ptr_ty, ir::MemFlags::trusted(), values, slot_offset);
    let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
    fb.ins().brif(
        value_is_null,
        miss_block,
        &[],
        value_block,
        &[ir::BlockArg::Value(value)],
    );

    fb.switch_to_block(value_block);
    let value = fb.block_params(value_block)[0];
    fb.ins().jump(hit_block, &[ir::BlockArg::Value(value)]);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_trusted_inline_values_field_store(
    fb: &mut FunctionBuilder<'_>,
    obj: ir::Value,
    owner_type: ir::Value,
    expected_index: u32,
    replacement: ir::Value,
    replacement_is_borrowed: bool,
    hit_block: ir::Block,
    miss_block: ir::Block,
    emit_ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let i64_ty = emit_ctx.consts.i64_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let values_block = fb.create_block();
    let first_insert_block = fb.create_block();
    let first_insert_store_block = fb.create_block();
    let overwrite_block = fb.create_block();
    fb.append_block_param(values_block, ptr_ty);
    fb.append_block_param(first_insert_block, ptr_ty);
    fb.append_block_param(first_insert_store_block, ptr_ty);
    fb.append_block_param(first_insert_store_block, ir::types::I8);
    fb.append_block_param(first_insert_store_block, ir::types::I8);
    fb.append_block_param(overwrite_block, ptr_ty);
    fb.append_block_param(overwrite_block, ptr_ty);

    emit_trusted_inline_values_field_values(
        fb,
        obj,
        owner_type,
        expected_index,
        values_block,
        miss_block,
        emit_ctx,
    );

    fb.switch_to_block(values_block);
    let values = fb.block_params(values_block)[0];
    let slot_offset = split_values_slot_offset(expected_index)?;
    let old_value = fb
        .ins()
        .load(ptr_ty, ir::MemFlags::trusted(), values, slot_offset);
    let old_value_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, old_value, null_ptr);
    fb.ins().brif(
        old_value_is_null,
        first_insert_block,
        &[ir::BlockArg::Value(values)],
        overwrite_block,
        &[ir::BlockArg::Value(values), ir::BlockArg::Value(old_value)],
    );

    fb.switch_to_block(first_insert_block);
    let values = fb.block_params(first_insert_block)[0];
    let size = fb.ins().load(
        ir::types::I8,
        ir::MemFlags::trusted(),
        values,
        SPLIT_VALUES_SIZE_OFFSET,
    );
    let capacity = fb.ins().load(
        ir::types::I8,
        ir::MemFlags::trusted(),
        values,
        SPLIT_VALUES_CAPACITY_OFFSET,
    );
    let has_capacity = fb
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedLessThan, size, capacity);
    fb.ins().brif(
        has_capacity,
        first_insert_store_block,
        &[
            ir::BlockArg::Value(values),
            ir::BlockArg::Value(size),
            ir::BlockArg::Value(capacity),
        ],
        miss_block,
        &[],
    );

    fb.switch_to_block(first_insert_store_block);
    let values = fb.block_params(first_insert_store_block)[0];
    let size = fb.block_params(first_insert_store_block)[1];
    let capacity = fb.block_params(first_insert_store_block)[2];
    if replacement_is_borrowed {
        emit_ctx.emit_incref_for_family(
            fb,
            replacement,
            Some(PyObjFacts::unknown().with_non_null_ref()),
            RefcountFamily::ContainerStoreClone,
        );
    }
    fb.ins()
        .store(ir::MemFlags::trusted(), replacement, values, slot_offset);
    let capacity = fb.ins().uextend(i64_ty, capacity);
    let order_offset = fb.ins().imul_imm(
        capacity,
        i64::try_from(std::mem::size_of::<*mut ffi::PyObject>())
            .map_err(|_| "pointer size does not fit i64".to_string())?,
    );
    let order_base = fb.ins().iadd(values, order_offset);
    let order_base = fb
        .ins()
        .iadd_imm(order_base, i64::from(SPLIT_VALUES_VALUES_OFFSET));
    let order_index = fb.ins().uextend(i64_ty, size);
    let order_addr = fb.ins().iadd(order_base, order_index);
    let expected_index_u8 = fb.ins().iconst(ir::types::I8, i64::from(expected_index));
    fb.ins()
        .store(ir::MemFlags::trusted(), expected_index_u8, order_addr, 0);
    let next_size = fb.ins().iadd_imm(size, 1);
    fb.ins().store(
        ir::MemFlags::trusted(),
        next_size,
        values,
        SPLIT_VALUES_SIZE_OFFSET,
    );
    fb.ins().jump(hit_block, &[]);

    fb.switch_to_block(overwrite_block);
    let values = fb.block_params(overwrite_block)[0];
    let old_value = fb.block_params(overwrite_block)[1];
    if replacement_is_borrowed {
        emit_ctx.emit_incref_for_family(
            fb,
            replacement,
            Some(PyObjFacts::unknown().with_non_null_ref()),
            RefcountFamily::ContainerStoreClone,
        );
    }
    fb.ins()
        .store(ir::MemFlags::trusted(), replacement, values, slot_offset);
    emit_ctx.emit_decref_for_family(
        fb,
        old_value,
        Some(PyObjFacts::unknown().with_non_null_ref()),
        RefcountFamily::ContainerOverwriteRelease,
    );
    fb.ins().jump(hit_block, &[]);
    Ok(())
}

fn emit_exact_cpython_type_guard(
    fb: &mut FunctionBuilder<'_>,
    obj: ir::Value,
    expected_type_ref: RelocTypeRef,
    fallback_block: ir::Block,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
) -> Result<(), String> {
    let ptr_ty = fb.func.dfg.value_type(obj);
    let expected_type = emit_type_ptr_value_for_ref(fb, codegen_env, ctx, &expected_type_ref)?
        .ok_or_else(|| format!("missing type symbol for {expected_type_ref:?}"))?;
    let actual_type = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyObject, ob_type) as i32,
    );
    let type_matches = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, actual_type, expected_type);
    let ok_block = fb.create_block();
    fb.ins()
        .brif(type_matches, ok_block, &[], fallback_block, &[]);
    fb.switch_to_block(ok_block);
    Ok(())
}
fn emit_exact_function_id_match_bool01(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    expected_function_id: RuntimeFunctionId,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
) -> Result<SoacValue, String> {
    let i32_ty = ctx.consts.i32_ty;
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let zero_i32 = fb.ins().iconst(i32_ty, 0);
    let done_block = fb.create_block();
    let non_null_block = fb.create_block();
    fb.append_block_param(done_block, i32_ty);

    let is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, callable, null_ptr);
    fb.ins().brif(
        is_null,
        done_block,
        &[ir::BlockArg::Value(zero_i32)],
        non_null_block,
        &[],
    );

    fb.switch_to_block(non_null_block);
    let actual_id = emit_callee_function_id_checked(fb, callable, ctx, codegen_env);
    let id_matches = fb.ins().icmp_imm(
        ir::condcodes::IntCC::Equal,
        actual_id,
        expected_function_id.to_packed_runtime_u64() as i64,
    );
    let truth = emit_i32_bool01_from_cond(fb, id_matches, ctx).expect_i32_bool01("function guard");
    fb.ins().jump(done_block, &[ir::BlockArg::Value(truth)]);

    fb.switch_to_block(done_block);
    Ok(SoacValue::i32(
        fb.block_params(done_block)[0],
        IntFacts::i32_bool01(),
    ))
}

fn emit_callee_function_id_checked(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
) -> ir::Value {
    #[repr(C)]
    struct PyMethodObjectPrefix {
        ob_refcnt: isize,
        ob_type: *mut ffi::PyTypeObject,
        im_func: *mut ffi::PyObject,
    }

    #[repr(C)]
    struct PyHeapTypeObjectSoacPrefix {
        ht_type: ffi::PyTypeObject,
        as_async: ffi::PyAsyncMethods,
        as_number: ffi::PyNumberMethods,
        as_mapping: ffi::PyMappingMethods,
        as_sequence: ffi::PySequenceMethods,
        as_buffer: ffi::PyBufferProcs,
        ht_name: *mut ffi::PyObject,
        ht_slots: *mut ffi::PyObject,
        ht_qualname: *mut ffi::PyObject,
        ht_cached_keys: *mut std::ffi::c_void,
        ht_module: *mut ffi::PyObject,
        ht_tpname: *mut i8,
        ht_token: *mut std::ffi::c_void,
        ht_soac_metadata: *mut std::ffi::c_void,
        ht_soac_metadata_destructor: *mut std::ffi::c_void,
        ht_soac_function_id: u64,
    }

    const PYOBJECT_OB_TYPE_OFFSET: i32 = offset_of!(ffi::PyObject, ob_type) as i32;
    const PYMETHOD_IM_FUNC_OFFSET: i32 = offset_of!(PyMethodObjectPrefix, im_func) as i32;
    const PYTYPE_TP_FLAGS_OFFSET: i32 = offset_of!(ffi::PyTypeObject, tp_flags) as i32;
    const PYHEAPTYPE_SOAC_METADATA_OFFSET: i32 =
        offset_of!(PyHeapTypeObjectSoacPrefix, ht_soac_metadata) as i32;
    const PYHEAPTYPE_SOAC_FUNCTION_ID_OFFSET: i32 =
        offset_of!(PyHeapTypeObjectSoacPrefix, ht_soac_function_id) as i32;

    let ptr_ty = ctx.consts.ptr_ty;
    let i64_ty = ctx.consts.i64_ty;
    let null_block = fb.create_block();
    let not_null_block = fb.create_block();
    let function_block = fb.create_block();
    let maybe_method_block = fb.create_block();
    let method_block = fb.create_block();
    let maybe_type_block = fb.create_block();
    let type_block = fb.create_block();
    let miss_block = fb.create_block();
    let function_value_block = fb.create_block();
    let done_block = fb.create_block();
    let nonzero_id_block = fb.create_block();
    let nonzero_type_id_block = fb.create_block();
    fb.append_block_param(function_value_block, ptr_ty);
    fb.append_block_param(done_block, i64_ty);

    let callable_is_null = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, callable, 0);
    fb.ins()
        .brif(callable_is_null, null_block, &[], not_null_block, &[]);

    fb.switch_to_block(null_block);
    let err_const = fb.ins().iconst(i64_ty, i64::MIN);
    fb.ins().jump(done_block, &[ir::BlockArg::Value(err_const)]);

    fb.switch_to_block(not_null_block);
    let callable_type = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        callable,
        PYOBJECT_OB_TYPE_OFFSET,
    );
    let py_function_type = emit_type_ptr_value_for_ref(
        fb,
        codegen_env,
        ctx,
        &RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Function),
    )
    .unwrap_or_else(|err| panic!("failed to bind PyFunction_Type symbol: {err}"))
    .expect("PyFunction_Type symbol should be available");
    let is_function = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, callable_type, py_function_type);
    fb.ins()
        .brif(is_function, function_block, &[], maybe_method_block, &[]);

    fb.switch_to_block(function_block);
    fb.ins()
        .jump(function_value_block, &[ir::BlockArg::Value(callable)]);

    fb.switch_to_block(maybe_method_block);
    let py_method_type = emit_type_ptr_value_for_ref(
        fb,
        codegen_env,
        ctx,
        &RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Method),
    )
    .unwrap_or_else(|err| panic!("failed to bind PyMethod_Type symbol: {err}"))
    .expect("PyMethod_Type symbol should be available");
    let is_method = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, callable_type, py_method_type);
    fb.ins()
        .brif(is_method, method_block, &[], maybe_type_block, &[]);

    fb.switch_to_block(method_block);
    let method_function = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        callable,
        PYMETHOD_IM_FUNC_OFFSET,
    );
    fb.ins().jump(
        function_value_block,
        &[ir::BlockArg::Value(method_function)],
    );

    fb.switch_to_block(maybe_type_block);
    let py_type_type = emit_type_ptr_value_for_ref(
        fb,
        codegen_env,
        ctx,
        &RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Type),
    )
    .unwrap_or_else(|err| panic!("failed to bind PyType_Type symbol: {err}"))
    .expect("PyType_Type symbol should be available");
    let is_type_object = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, callable_type, py_type_type);
    fb.ins()
        .brif(is_type_object, type_block, &[], miss_block, &[]);

    fb.switch_to_block(type_block);
    let type_flags = fb.ins().load(
        i64_ty,
        ir::MemFlags::trusted(),
        callable,
        PYTYPE_TP_FLAGS_OFFSET,
    );
    let heaptype_mask = fb.ins().iconst(i64_ty, ffi::Py_TPFLAGS_HEAPTYPE as i64);
    let heaptype_bits = fb.ins().band(type_flags, heaptype_mask);
    let is_heap_type = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::NotEqual, heaptype_bits, 0);
    fb.ins()
        .brif(is_heap_type, nonzero_type_id_block, &[], miss_block, &[]);

    fb.switch_to_block(nonzero_type_id_block);
    let metadata = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        callable,
        PYHEAPTYPE_SOAC_METADATA_OFFSET,
    );
    let metadata_is_null = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, metadata, 0);
    let type_metadata_done_block = fb.create_block();
    fb.ins().brif(
        metadata_is_null,
        miss_block,
        &[],
        type_metadata_done_block,
        &[],
    );

    fb.switch_to_block(type_metadata_done_block);
    let packed = fb.ins().load(
        i64_ty,
        ir::MemFlags::trusted(),
        callable,
        PYHEAPTYPE_SOAC_FUNCTION_ID_OFFSET,
    );
    let id_is_zero = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, packed, 0);
    let type_id_done_block = fb.create_block();
    fb.ins()
        .brif(id_is_zero, miss_block, &[], type_id_done_block, &[]);

    fb.switch_to_block(type_id_done_block);
    fb.ins().jump(done_block, &[ir::BlockArg::Value(packed)]);

    fb.switch_to_block(function_value_block);
    let function_value = fb.block_params(function_value_block)[0];
    let function_is_null = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::Equal, function_value, 0);
    fb.ins()
        .brif(function_is_null, null_block, &[], nonzero_id_block, &[]);

    fb.switch_to_block(nonzero_id_block);
    let packed = fb.ins().load(
        i64_ty,
        ir::MemFlags::trusted(),
        function_value,
        PY_FUNCTION_SOAC_FUNCTION_ID_OFFSET,
    );
    let id_is_zero = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, packed, 0);
    let id_done_block = fb.create_block();
    fb.ins()
        .brif(id_is_zero, miss_block, &[], id_done_block, &[]);

    fb.switch_to_block(id_done_block);
    let metadata = load_py_function_soac_metadata_obj(fb, ptr_ty, function_value);
    let metadata_is_null = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, metadata, 0);
    let code_snapshot_block = fb.create_block();
    fb.ins()
        .brif(metadata_is_null, miss_block, &[], code_snapshot_block, &[]);

    fb.switch_to_block(code_snapshot_block);
    let current_code = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        function_value,
        PY_FUNCTION_CODE_OFFSET,
    );
    let registered_code = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        metadata,
        PY_FUNCTION_JIT_EXTRA_REGISTERED_CODE_OFFSET,
    );
    let code_matches = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, current_code, registered_code);
    let current_defaults = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        function_value,
        PY_FUNCTION_DEFAULTS_OFFSET,
    );
    let registered_defaults = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        metadata,
        crate::PY_FUNCTION_JIT_EXTRA_REGISTERED_DEFAULTS_OFFSET,
    );
    let defaults_match = fb.ins().icmp(
        ir::condcodes::IntCC::Equal,
        current_defaults,
        registered_defaults,
    );
    let current_kwdefaults = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        function_value,
        PY_FUNCTION_KWDEFAULTS_OFFSET,
    );
    let registered_kwdefaults = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        metadata,
        crate::PY_FUNCTION_JIT_EXTRA_REGISTERED_KWDEFAULTS_OFFSET,
    );
    let kwdefaults_match = fb.ins().icmp(
        ir::condcodes::IntCC::Equal,
        current_kwdefaults,
        registered_kwdefaults,
    );
    // Unlike positional defaults, keyword-only defaults live in a mutable
    // dictionary. Pointer identity cannot prove that the copied runtime slots
    // still contain the current values after an in-place update or deletion.
    // Keep those functions on the vectorcall path, which rereads their
    // keyword-only defaults before binding.
    let kwdefaults_are_immutable =
        fb.ins()
            .icmp_imm(ir::condcodes::IntCC::Equal, current_kwdefaults, 0);
    let code_and_defaults_match = fb.ins().band(code_matches, defaults_match);
    let kwdefaults_are_safe = fb.ins().band(kwdefaults_match, kwdefaults_are_immutable);
    let snapshots_match = fb.ins().band(code_and_defaults_match, kwdefaults_are_safe);
    let current_code_block = fb.create_block();
    fb.ins()
        .brif(snapshots_match, current_code_block, &[], miss_block, &[]);

    fb.switch_to_block(current_code_block);
    fb.ins().jump(done_block, &[ir::BlockArg::Value(packed)]);

    fb.switch_to_block(miss_block);
    let zero_const = fb.ins().iconst(i64_ty, 0);
    fb.ins()
        .jump(done_block, &[ir::BlockArg::Value(zero_const)]);

    fb.switch_to_block(done_block);
    let callee_id = fb.block_params(done_block)[0];
    let errored = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::SignedLessThan, callee_id, 0);
    let ok_block = fb.create_block();
    fb.append_block_param(ok_block, ctx.consts.i64_ty);
    fb.ins().brif(
        errored,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        ok_block,
        &[ir::BlockArg::Value(callee_id)],
    );
    fb.switch_to_block(ok_block);
    fb.block_params(ok_block)[0]
}

fn emit_record_top_value_sample(
    fb: &mut FunctionBuilder<'_>,
    counter_id: CounterId,
    observed_value: ir::Value,
    ctx: &JitEmitCtx<'_>,
) {
    let counter_slot = top_value_counter_slot_for_id(ctx.counter_slots_by_id, counter_id)
        .unwrap_or_else(|err| panic!("{err}"));
    let top_value_counter_base_value =
        ctx.consts.top_value_counter_base_value.unwrap_or_else(|| {
            panic!(
                "missing top-value counter base for counter id {}",
                counter_id.0
            )
        });
    let record_top_value_sample_ref = ctx.record_top_value_sample_ref.unwrap_or_else(|| {
        panic!(
            "missing top-value counter helper import for counter id {}",
            counter_id.0
        )
    });
    emit_record_top_value_counter_slot(
        fb,
        top_value_counter_base_value,
        counter_slot,
        observed_value,
        record_top_value_sample_ref,
    );
}

fn emit_record_call_target_sample(
    fb: &mut FunctionBuilder<'_>,
    counter_id: CounterId,
    callee_id: ir::Value,
    ctx: &JitEmitCtx<'_>,
) {
    emit_record_top_value_sample(fb, counter_id, callee_id, ctx);
}

fn emit_record_protocol_method_target_sample(
    fb: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    counter_id: CounterId,
    helper_import: &'static ImportSpec,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) {
    let helper = func_imports.get_or_panic(codegen_env, &mut fb.func, helper_import);
    let call = fb.ins().call(helper, &[receiver]);
    let callee_id = fb.inst_results(call)[0];
    emit_record_call_target_sample(fb, counter_id, callee_id, ctx);
}

fn protocol_method_target_helper_import(runtime_name: &str) -> Option<&'static ImportSpec> {
    match runtime_name {
        "iter" => Some(&DP_JIT_PROTOCOL_ITER_FUNCTION_ID_IMPORT),
        "next" => Some(&DP_JIT_PROTOCOL_NEXT_FUNCTION_ID_IMPORT),
        _ => None,
    }
}

fn call_access_allows_protocol_target_sample(access: Option<&TypedCallAccessPlan>) -> bool {
    match access {
        None | Some(TypedCallAccessPlan::Generic) => true,
        Some(TypedCallAccessPlan::GuardedRuntimeProtocolMethod { method_guards, .. }) => {
            method_guards.is_empty()
        }
        Some(TypedCallAccessPlan::GuardedCallable { .. })
        | Some(TypedCallAccessPlan::GuardedMethod { .. }) => false,
    }
}

fn emit_record_direct_call_target_sample(
    fb: &mut FunctionBuilder<'_>,
    site_instr_id: Option<InstrId>,
    function_id: RuntimeFunctionId,
    ctx: &JitEmitCtx<'_>,
) {
    if let Some(counter_id) = site_instr_id
        .and_then(|site_instr_id| ctx.call_direct_target_counter_ids.get(&site_instr_id))
        .copied()
    {
        let callee_id = fb.ins().iconst(
            ctx.consts.i64_ty,
            function_id.to_packed_runtime_u64() as i64,
        );
        emit_record_call_target_sample(fb, counter_id, callee_id, ctx);
    }
}

fn emit_record_branch_outcome_sample(
    fb: &mut FunctionBuilder<'_>,
    counter_id: CounterId,
    truth_i32: ir::Value,
    ctx: &JitEmitCtx<'_>,
) {
    let observed_value = fb.ins().uextend(ctx.consts.i64_ty, truth_i32);
    emit_record_top_value_sample(fb, counter_id, observed_value, ctx);
}

fn emit_direct_call_resolved_raw_with_arg_values(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    arg_values: Vec<ir::Value>,
    arg_borrowed: Vec<bool>,
    entry_kind: DirectCallEntryKind,
    target_function: &BlockPyFunction<impl ModuleShape>,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
) -> ir::Value {
    debug_assert_eq!(arg_values.len(), target_function.params.len());
    ctx.direct_edge_stats.record_resolved_direct_edge();

    let (function_metadata, function_env) =
        emit_resolved_direct_function_metadata_and_env(fb, callable, target_function, ctx);

    let enter_inst = fb
        .ins()
        .call(ctx.enter_recursive_ref, &[ctx.consts.thread_state_value]);
    let enter_status = fb.inst_results(enter_inst)[0];
    let enter_failed = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::NotEqual, enter_status, 0);
    let entered_block = fb.create_block();
    fb.ins().brif(
        enter_failed,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        entered_block,
        &[],
    );
    fb.switch_to_block(entered_block);

    let mut call_args = Vec::with_capacity(arg_values.len() + 2);
    call_args.push(function_env);
    call_args.push(ctx.consts.thread_state_value);
    call_args.extend(arg_values.iter().copied());
    let call_inst = if let Some(direct_func_id) = ctx
        .direct_call_functions
        .get(&target_function.function_id)
        .and_then(|function| match entry_kind {
            DirectCallEntryKind::Core => Some(function.func_id),
            DirectCallEntryKind::DefaultResolving => function.default_func_id,
        }) {
        let (function_env, _callee_ptr) = emit_resolved_direct_entry_ptr(
            fb,
            callable,
            function_metadata,
            function_env,
            entry_kind,
            ctx,
        );
        call_args[0] = function_env;
        let func_ref = codegen_env
            .codegen_declare_func_in_func(direct_func_id, &mut fb.func)
            .expect("reserved direct function should be declared in codegen env");
        fb.ins().call(func_ref, &call_args)
    } else {
        ctx.direct_edge_stats.record_function_env_indirect_edge();
        let (function_env, callee_ptr) = emit_resolved_direct_entry_ptr(
            fb,
            callable,
            function_metadata,
            function_env,
            entry_kind,
            ctx,
        );
        call_args[0] = function_env;
        let direct_sig =
            fb.import_signature(make_direct_function_signature(codegen_env, target_function));
        fb.ins().call_indirect(direct_sig, callee_ptr, &call_args)
    };
    let call_value = fb.inst_results(call_inst)[0];
    let mut owned_inputs =
        Vec::with_capacity(arg_values.len() + usize::from(!callable_is_borrowed));
    for (value, borrowed_arg) in arg_values.into_iter().zip(arg_borrowed.into_iter()) {
        if !borrowed_arg {
            owned_inputs.push(value);
        }
    }
    if !callable_is_borrowed {
        owned_inputs.push(callable);
    }
    emit_decref_owned_inputs_after_nullable_result(fb, ctx, call_value, &owned_inputs)
}

fn emit_direct_call_resolved_with_arg_values(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    arg_values: Vec<ir::Value>,
    arg_borrowed: Vec<bool>,
    entry_kind: DirectCallEntryKind,
    target_function: &BlockPyFunction<impl ModuleShape>,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let call_value = emit_direct_call_resolved_raw_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        entry_kind,
        target_function,
        ctx,
        codegen_env,
    );
    let call_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
    let call_ok_block = fb.create_block();
    fb.append_block_param(call_ok_block, ptr_ty);
    fb.ins().brif(
        call_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        call_ok_block,
        &[ir::BlockArg::Value(call_value)],
    );
    fb.switch_to_block(call_ok_block);
    fb.block_params(call_ok_block)[0]
}

fn emit_direct_call_args_from_plan(
    fb: &mut FunctionBuilder<'_>,
    arg_plan: &DirectCallArgPlan,
    provided_arg_values: Vec<ir::Value>,
    provided_arg_borrowed: Vec<bool>,
    ctx: &JitEmitCtx<'_>,
) -> (Vec<ir::Value>, Vec<bool>) {
    debug_assert_eq!(provided_arg_values.len(), provided_arg_borrowed.len());
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let mut arg_values = Vec::with_capacity(arg_plan.len());
    let mut arg_borrowed = Vec::with_capacity(arg_plan.len());
    let mut used_provided_args = 0usize;
    for source in &arg_plan.sources {
        match *source {
            DirectCallArgSource::Provided(index) => {
                debug_assert_eq!(
                    index, used_provided_args,
                    "direct-call arg plans should consume provided args in order"
                );
                arg_values.push(provided_arg_values[index]);
                arg_borrowed.push(provided_arg_borrowed[index]);
                used_provided_args += 1;
            }
            DirectCallArgSource::PackedRest { start } => {
                debug_assert_eq!(
                    start, used_provided_args,
                    "direct-call arg plans should pack the next provided arg"
                );
                let tuple_items = provided_arg_values[start..]
                    .iter()
                    .copied()
                    .zip(provided_arg_borrowed[start..].iter().copied())
                    .collect::<Vec<_>>();
                let tuple = emit_pack_current_values_tuple(fb, tuple_items.as_slice(), ctx);
                arg_values.push(tuple);
                arg_borrowed.push(false);
                used_provided_args = provided_arg_values.len();
            }
            DirectCallArgSource::DefaultSentinel => {
                arg_values.push(null_ptr);
                arg_borrowed.push(true);
            }
        }
    }
    debug_assert_eq!(used_provided_args, provided_arg_values.len());
    (arg_values, arg_borrowed)
}

fn emit_direct_call_resolved_with_arg_plan_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrBlockPy],
    arg_plan: &DirectCallArgPlan,
    target_function: &BlockPyFunction<impl ModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let implicit_callable_arg = is_constructor_entry_function(target_function);
    let mut provided_arg_values: Vec<ir::Value> =
        Vec::with_capacity(args.len() + usize::from(implicit_callable_arg));
    let mut provided_arg_borrowed: Vec<bool> =
        Vec::with_capacity(args.len() + usize::from(implicit_callable_arg));
    if implicit_callable_arg {
        provided_arg_values.push(callable);
        provided_arg_borrowed.push(true);
    }
    for arg in args {
        let borrowed_arg =
            codegen_expr_pyobject_input_is_borrowed_from_local_env(arg, local_env, ctx);
        provided_arg_borrowed.push(borrowed_arg);
        provided_arg_values.push(emit_codegen_expr_with_local_env(
            fb,
            arg,
            local_env,
            ctx,
            borrowed_arg,
            codegen_env,
            func_imports,
        ));
    }
    let (arg_values, arg_borrowed) = emit_direct_call_args_from_plan(
        fb,
        arg_plan,
        provided_arg_values,
        provided_arg_borrowed,
        ctx,
    );
    emit_direct_call_resolved_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        if arg_plan.requires_default_resolving_entry() {
            DirectCallEntryKind::DefaultResolving
        } else {
            DirectCallEntryKind::Core
        },
        target_function,
        ctx,
        codegen_env,
    )
}

fn emit_typed_pyobject_input_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    site: &str,
) -> Result<(ir::Value, bool), String> {
    let borrowed = typed_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, ctx);
    let (value, ownership, _) = emit_typed_pyobject_value_with_local_env(
        fb,
        expr,
        local_env,
        ctx,
        borrowed,
        codegen_env,
        func_imports,
        site,
    )?;
    Ok((value, !ownership.is_owned()))
}

fn push_owned_typed_input_cleanup(
    owned_inputs: &mut Vec<ir::Value>,
    value: ir::Value,
    input_needs_no_cleanup: bool,
) {
    if !input_needs_no_cleanup {
        owned_inputs.push(value);
    }
}

fn emit_typed_direct_call_resolved_with_arg_plan_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrTyped],
    arg_plan: &DirectCallArgPlan,
    target_function: &BlockPyFunction<impl ModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let implicit_callable_arg = is_constructor_entry_function(target_function);
    let mut provided_arg_values: Vec<ir::Value> =
        Vec::with_capacity(args.len() + usize::from(implicit_callable_arg));
    let mut provided_arg_borrowed: Vec<bool> =
        Vec::with_capacity(args.len() + usize::from(implicit_callable_arg));
    if implicit_callable_arg {
        provided_arg_values.push(callable);
        provided_arg_borrowed.push(true);
    }
    for arg in args {
        let (value, borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            arg,
            local_env,
            ctx,
            codegen_env,
            func_imports,
            "typed direct-call arg",
        )?;
        provided_arg_values.push(value);
        provided_arg_borrowed.push(borrowed);
    }
    let (arg_values, arg_borrowed) = emit_direct_call_args_from_plan(
        fb,
        arg_plan,
        provided_arg_values,
        provided_arg_borrowed,
        ctx,
    );
    Ok(emit_direct_call_resolved_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        if arg_plan.requires_default_resolving_entry() {
            DirectCallEntryKind::DefaultResolving
        } else {
            DirectCallEntryKind::Core
        },
        target_function,
        ctx,
        codegen_env,
    ))
}

fn emit_direct_method_resolved_with_args_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    receiver_is_borrowed: bool,
    args: &[&InstrBlockPy],
    specialization: &DirectMethodSpecialization,
    target_function: &BlockPyFunction<impl ModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let mut provided_arg_values = Vec::with_capacity(args.len() + 1);
    let mut provided_arg_borrowed = Vec::with_capacity(args.len() + 1);
    provided_arg_values.push(receiver);
    provided_arg_borrowed.push(receiver_is_borrowed);
    for arg in args {
        let borrowed_arg =
            codegen_expr_pyobject_input_is_borrowed_from_local_env(arg, local_env, ctx);
        provided_arg_borrowed.push(borrowed_arg);
        provided_arg_values.push(emit_codegen_expr_with_local_env(
            fb,
            arg,
            local_env,
            ctx,
            borrowed_arg,
            codegen_env,
            func_imports,
        ));
    }
    let (arg_values, arg_borrowed) = emit_direct_call_args_from_plan(
        fb,
        &specialization.arg_plan,
        provided_arg_values,
        provided_arg_borrowed,
        ctx,
    );
    let callable = emit_callable_ptr_value_for_ref(
        fb,
        codegen_env,
        ctx,
        &specialization.descriptor_function_ref,
    )
    .unwrap_or_else(|err| panic!("failed to bind direct method callable symbol: {err}"))
    .expect("direct method callable symbol should be available");
    emit_direct_call_resolved_with_arg_values(
        fb,
        callable,
        true,
        arg_values,
        arg_borrowed,
        if specialization.arg_plan.requires_default_resolving_entry() {
            DirectCallEntryKind::DefaultResolving
        } else {
            DirectCallEntryKind::Core
        },
        target_function,
        ctx,
        codegen_env,
    )
}

fn emit_typed_direct_method_resolved_with_args_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    receiver_is_borrowed: bool,
    args: &[&InstrTyped],
    specialization: &DirectMethodSpecialization,
    target_function: &BlockPyFunction<impl ModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let mut provided_arg_values = Vec::with_capacity(args.len() + 1);
    let mut provided_arg_borrowed = Vec::with_capacity(args.len() + 1);
    provided_arg_values.push(receiver);
    provided_arg_borrowed.push(receiver_is_borrowed);
    for arg in args {
        let (value, borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            arg,
            local_env,
            ctx,
            codegen_env,
            func_imports,
            "typed direct-method arg",
        )?;
        provided_arg_values.push(value);
        provided_arg_borrowed.push(borrowed);
    }
    let (arg_values, arg_borrowed) = emit_direct_call_args_from_plan(
        fb,
        &specialization.arg_plan,
        provided_arg_values,
        provided_arg_borrowed,
        ctx,
    );
    let callable = emit_callable_ptr_value_for_ref(
        fb,
        codegen_env,
        ctx,
        &specialization.descriptor_function_ref,
    )
    .unwrap_or_else(|err| panic!("failed to bind direct method callable symbol: {err}"))
    .expect("direct method callable symbol should be available");
    Ok(emit_direct_call_resolved_with_arg_values(
        fb,
        callable,
        true,
        arg_values,
        arg_borrowed,
        if specialization.arg_plan.requires_default_resolving_entry() {
            DirectCallEntryKind::DefaultResolving
        } else {
            DirectCallEntryKind::Core
        },
        target_function,
        ctx,
        codegen_env,
    ))
}

fn abrupt_kind_tag(kind: AbruptKind) -> i64 {
    match kind {
        AbruptKind::Fallthrough => 0,
        AbruptKind::Return => 1,
        AbruptKind::Exception => 2,
        AbruptKind::Break => 3,
        AbruptKind::Continue => 4,
    }
}

fn emit_planned_target_args_codegen_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    target_args: &[RuntimeBlockArgPlan],
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
    _codegen_env: &mut impl JitCodegenEnv,
    _func_imports: &mut FuncBuildImports<'_>,
) -> Result<(Vec<ir::BlockArg>, HashSet<LocalLocation>), LocalEnvEdgePrepError> {
    let mut args = Vec::with_capacity(target_args.len());
    let mut forwarded_locations = HashSet::new();
    let mut forwarded_local_counts = HashMap::new();
    for target_arg in target_args {
        let value = match (&target_arg.source, target_arg.repr) {
            (BlockArg::Name(source_name), RuntimeBlockParamRepr::ExactI64) => {
                emit_forwarded_block_arg_source_i64_value(
                    source_name,
                    target_arg.target_name.as_str(),
                    local_env,
                    &mut forwarded_locations,
                )?
            }
            (BlockArg::Name(source_name), RuntimeBlockParamRepr::I32Bool01) => {
                emit_forwarded_block_arg_source_i32_bool01_value(
                    source_name,
                    target_arg.target_name.as_str(),
                    local_env,
                    &mut forwarded_locations,
                )?
            }
            (BlockArg::Name(source_name), RuntimeBlockParamRepr::PyObject) => {
                let (value, maybe_index) = emit_forwarded_block_arg_source_value(
                    fb,
                    source_name,
                    local_env,
                    ctx,
                    &mut forwarded_local_counts,
                )?;
                if let Some(index) = maybe_index
                    && let Some(location) = local_env.entries[index].location
                {
                    forwarded_locations.insert(location);
                }
                value
            }
            (BlockArg::None, RuntimeBlockParamRepr::PyObject) => {
                let none_const = emit_none_const(fb, ctx);
                ctx.emit_incref_for_family(
                    fb,
                    none_const,
                    Some(PyObjFacts::none_singleton()),
                    RefcountFamily::ConstantClone,
                );
                none_const
            }
            (BlockArg::CurrentException, RuntimeBlockParamRepr::PyObject) => {
                return Err(LocalEnvEdgePrepError::UnsupportedCurrentExceptionArg);
            }
            (BlockArg::AbruptKind(kind), RuntimeBlockParamRepr::PyObject) => {
                emit_owned_module_constant(
                    fb,
                    ctx.module_constants
                        .require_int_constant_id(abrupt_kind_tag(*kind)),
                    ctx,
                )
            }
            (source, RuntimeBlockParamRepr::ExactI64 | RuntimeBlockParamRepr::I32Bool01) => {
                return Err(LocalEnvEdgePrepError::UnsupportedScalarConstantArg {
                    target_name: target_arg.target_name.clone(),
                    source: source.clone(),
                });
            }
        };
        args.push(ir::BlockArg::Value(value));
    }
    Ok((args, forwarded_locations))
}

fn emit_decref_unforwarded_local_env(
    fb: &mut FunctionBuilder<'_>,
    local_env: &LocalEnv,
    forwarded_locations: &HashSet<LocalLocation>,
    preserved_values: &[ir::Value],
    thread_state_value: ir::Value,
    decref_ref: ir::FuncRef,
    family: RefcountFamily,
) {
    #[cfg(debug_assertions)]
    {
        let residual_semantic = local_env
            .transient_semantic_cleanup_names_excluding(forwarded_locations, preserved_values);
        debug_assert!(
            residual_semantic.is_empty(),
            "planned edge cleanup left semantic locals for generic LocalEnv cleanup: {:?}",
            residual_semantic
        );
    }
    for entry in &local_env.entries {
        if entry
            .location
            .is_some_and(|location| forwarded_locations.contains(&location))
        {
            continue;
        }
        if preserved_values.contains(&entry.value()) {
            continue;
        }
        if transient_local_needs_decref(entry.ref_kind()) {
            with_refcount_family(fb, Some(family), |fb| {
                fb.ins()
                    .call(decref_ref, &[thread_state_value, entry.value()]);
            });
        }
    }
}

fn emit_forward_named_values_from_local_env_with_refcount<'a, I>(
    fb: &mut FunctionBuilder<'_>,
    source_names: I,
    local_env: &LocalEnv,
    ptr_ty: ir::Type,
    incref_ref: ir::FuncRef,
    ctx: &JitEmitCtx<'_>,
) -> Result<(Vec<ir::Value>, HashSet<LocalLocation>), LocalEnvEdgePrepError>
where
    I: IntoIterator<Item = &'a str>,
{
    let source_names = source_names.into_iter().collect::<Vec<_>>();
    let mut values = Vec::with_capacity(source_names.len());
    let mut forwarded_local_locations = HashSet::new();
    let mut forwarded_local_counts = HashMap::new();
    for source_name in source_names {
        if let Some(value_index) = local_env.entry_index_for_block_arg_name(source_name) {
            let entry = &local_env.entries[value_index];
            let forwarded_count = forwarded_local_counts.entry(value_index).or_insert(0usize);
            let value = emit_local_env_entry_pyobject_for_forward(fb, entry, ctx, *forwarded_count);
            *forwarded_count += 1;
            if let Some(location) = entry.location {
                forwarded_local_locations.insert(location);
            }
            values.push(value);
            continue;
        }
        if is_try_abrupt_kind_name(source_name) {
            values.push(emit_owned_module_constant(
                fb,
                ctx.module_constants
                    .require_int_constant_id(abrupt_kind_tag(AbruptKind::Fallthrough)),
                ctx,
            ));
            continue;
        }
        if is_try_abrupt_payload_name(source_name) {
            let none_const = emit_none_const(fb, ctx);
            with_refcount_family(fb, Some(RefcountFamily::ConstantClone), |fb| {
                fb.ins().call(incref_ref, &[none_const]);
            });
            values.push(none_const);
            continue;
        }
        values.push(fb.ins().iconst(ptr_ty, 0));
    }
    Ok((values, forwarded_local_locations))
}

fn emit_forward_named_values_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    source_names: &[String],
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Result<(Vec<ir::Value>, HashSet<LocalLocation>), LocalEnvEdgePrepError> {
    emit_forward_named_values_from_local_env_with_refcount(
        fb,
        source_names.iter().map(String::as_str),
        local_env,
        ctx.consts.ptr_ty,
        ctx.incref_ref,
        ctx,
    )
}

fn emit_exception_dispatch_slot_writes(
    fb: &mut FunctionBuilder<'_>,
    slot_writes: &[(String, BlockArg)],
    forwarded_local_names: &[String],
    forwarded_local_values: &[ir::Value],
    dispatch_exc: ir::Value,
    stack_slots: &StackSlots,
    cleanup_root_previous_states: &HashMap<String, CleanupRootSlotState>,
    cleanup_root_previous_facts: &HashMap<String, PyObjFacts>,
    refcount_location_counters: Option<RefcountDecrefLocationCounterParts<'_>>,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    none_const: ir::Value,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
    refcounts: RefcountEmitter,
) -> Result<(), String> {
    let forwarded_locals_by_name = forwarded_local_names
        .iter()
        .zip(forwarded_local_values.iter().copied())
        .map(|(name, value)| (name.as_str(), value))
        .collect::<HashMap<_, _>>();
    for (target_name, source) in slot_writes {
        let (value, value_ref_kind) = match source {
            BlockArg::Name(source_name) => forwarded_locals_by_name
                .get(source_name.as_str())
                .copied()
                .map(|value| (value, LocalRefKind::Owned))
                .ok_or_else(|| {
                    format!(
                        "missing forwarded exception dispatch slot source {source_name} for target {target_name}"
                    )
                })?,
            BlockArg::CurrentException => (dispatch_exc, LocalRefKind::Owned),
            BlockArg::None => (none_const, LocalRefKind::Immortal),
            BlockArg::AbruptKind(_) => {
                unreachable!("validated exception edges should not use abrupt-kind args")
            }
        };
        stack_slots
            .replace_cloned_value_with_previous_state_counted(
                fb,
                target_name,
                value,
                value_ref_kind,
                cleanup_root_previous_states
                    .get(target_name)
                    .copied()
                    .unwrap_or(CleanupRootSlotState::MaybeOwnedReference),
                ptr_ty,
                thread_state_value,
                incref_ref,
                decref_ref,
                refcounts,
                cleanup_root_previous_facts.get(target_name).copied(),
                refcount_location_counters,
            )
            .expect("exception dispatch slot target missing from stack slots");
    }
    Ok(())
}

fn emit_exception_dispatch_forwarded_decrefs(
    fb: &mut FunctionBuilder<'_>,
    forwarded_local_names: &[String],
    forwarded_local_values: &[ir::Value],
    decref_local_names: &[String],
    reason: &str,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    decref_ref: ir::FuncRef,
) -> Result<(), String> {
    let forwarded_locals_by_name = forwarded_local_names
        .iter()
        .zip(forwarded_local_values.iter().copied())
        .map(|(name, value)| (name.as_str(), value))
        .collect::<HashMap<_, _>>();
    for name in decref_local_names {
        let value = forwarded_locals_by_name
            .get(name.as_str())
            .copied()
            .ok_or_else(|| format!("missing forwarded exception {reason} local {name}"))?;
        emit_decref_if_not_null(fb, ptr_ty, decref_ref, thread_state_value, value);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_exception_dispatch_target_args(
    fb: &mut FunctionBuilder<'_>,
    target_args: &[RuntimeBlockArgPlan],
    forwarded_local_names: &[String],
    forwarded_local_values: &[ir::Value],
    dispatch_exc: ir::Value,
    module_constants: &ModuleCodegenConstants,
    module_constant_object_globals: &[ir::GlobalValue],
    ptr_ty: ir::Type,
    module_constant_accesses: &ModuleConstantAccessTable,
    thread_state_value: ir::Value,
    none_const: ir::Value,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
) -> Result<Vec<ir::BlockArg>, String> {
    let mut dispatch_exc_forward_count = 0usize;
    let forwarded_locals_by_name = forwarded_local_names
        .iter()
        .zip(forwarded_local_values.iter().copied())
        .map(|(name, value)| (name.as_str(), value))
        .collect::<HashMap<_, _>>();
    let mut forwarded_local_counts = HashMap::new();
    let mut args = Vec::with_capacity(target_args.len());
    for target_arg in target_args {
        if target_arg.repr != RuntimeBlockParamRepr::PyObject {
            return Err(format!(
                "exception dispatch target arg {} unexpectedly uses {:?}",
                target_arg.target_name, target_arg.repr
            ));
        }
        let value = match &target_arg.source {
            BlockArg::Name(source_name) => {
                let value = forwarded_locals_by_name
                    .get(source_name.as_str())
                    .copied()
                    .ok_or_else(|| {
                        format!(
                            "missing forwarded exception dispatch block-param source {source_name} for target {}",
                            target_arg.target_name
                        )
                    })?;
                let forwarded_count = forwarded_local_counts
                    .entry(source_name.as_str())
                    .or_insert(0usize);
                if *forwarded_count > 0 {
                    with_refcount_family(fb, Some(RefcountFamily::ForwardedValueClone), |fb| {
                        emit_incref_if_not_null(fb, ptr_ty, incref_ref, value);
                    });
                }
                *forwarded_count += 1;
                value
            }
            BlockArg::CurrentException => {
                if dispatch_exc_forward_count > 0 {
                    with_refcount_family(fb, Some(RefcountFamily::ForwardedValueClone), |fb| {
                        fb.ins().call(incref_ref, &[dispatch_exc]);
                    });
                }
                dispatch_exc_forward_count += 1;
                dispatch_exc
            }
            BlockArg::None => {
                with_refcount_family(fb, Some(RefcountFamily::ConstantClone), |fb| {
                    fb.ins().call(incref_ref, &[none_const]);
                });
                none_const
            }
            BlockArg::AbruptKind(kind) => emit_owned_module_constant_from_parts(
                fb,
                module_constants.require_int_constant_id(abrupt_kind_tag(*kind)),
                module_constant_object_globals,
                ptr_ty,
                module_constant_accesses,
            ),
        };
        args.push(ir::BlockArg::Value(value));
    }
    if dispatch_exc_forward_count == 0 {
        fb.ins()
            .call(decref_ref, &[thread_state_value, dispatch_exc]);
    }
    Ok(args)
}

fn emit_pop_handled_exception(
    fb: &mut FunctionBuilder<'_>,
    exception_name: &str,
    ctx: &JitEmitCtx<'_>,
) {
    let Some((previous_slot, is_pushed_slot)) = ctx
        .exception_state_slots
        .slots_for_exception(exception_name)
    else {
        return;
    };
    let is_pushed = fb.ins().stack_load(ir::types::I64, is_pushed_slot, 0);
    let should_pop = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::NotEqual, is_pushed, 0);
    let pop_block = fb.create_block();
    let done_block = fb.create_block();
    fb.ins().brif(should_pop, pop_block, &[], done_block, &[]);

    fb.switch_to_block(pop_block);
    let previous = fb.ins().stack_load(ctx.consts.ptr_ty, previous_slot, 0);
    fb.ins().call(ctx.pop_handled_exception_ref, &[previous]);
    let _ = ctx.stack_slots.clear_value_counted(
        fb,
        exception_name,
        ctx.consts.ptr_ty,
        ctx.consts.thread_state_value,
        ctx.decref_ref,
        ctx.refcount_emitter(),
        None,
        Some(refcount_decref_location_counter_parts(ctx)),
    );
    let null_ptr = fb.ins().iconst(ctx.consts.ptr_ty, 0);
    fb.ins().stack_store(null_ptr, previous_slot, 0);
    let not_pushed = fb.ins().iconst(ir::types::I64, 0);
    fb.ins().stack_store(not_pushed, is_pushed_slot, 0);
    fb.ins().jump(done_block, &[]);

    fb.switch_to_block(done_block);
}

fn emit_pop_handled_exception_if_leaving(
    fb: &mut FunctionBuilder<'_>,
    current_exception_name: Option<&str>,
    target_exception_name: Option<&str>,
    ctx: &JitEmitCtx<'_>,
) {
    let Some(exception_name) = current_exception_name else {
        return;
    };
    if target_exception_name == Some(exception_name) {
        return;
    }
    emit_pop_handled_exception(fb, exception_name, ctx);
}

fn emit_pop_handled_exception_if_not_forwarded<'a, I>(
    fb: &mut FunctionBuilder<'_>,
    current_exception_name: Option<&str>,
    target_params: I,
    ctx: &JitEmitCtx<'_>,
) where
    I: IntoIterator<Item = &'a str>,
{
    let Some(exception_name) = current_exception_name else {
        return;
    };
    if target_params.into_iter().any(|name| name == exception_name) {
        return;
    }
    emit_pop_handled_exception(fb, exception_name, ctx);
}

fn block_exception_name(
    function: &BlockPyFunction<impl ModuleShape>,
    label: BlockLabel,
) -> Option<&str> {
    function
        .blocks
        .iter()
        .find(|block| block.label == label)
        .unwrap_or_else(|| {
            panic!(
                "function {} ({}) references unknown block label {}",
                function.function_id, function.names.qualname, label
            )
        })
        .exception_param()
}

fn codegen_block_indices_by_label(
    function: &BlockPyFunction<impl ModuleShape>,
) -> HashMap<BlockLabel, usize> {
    function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect()
}

fn codegen_block_index_for_label(
    function: &BlockPyFunction<impl ModuleShape>,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    label: BlockLabel,
) -> Result<usize, String> {
    block_indices_by_label.get(&label).copied().ok_or_else(|| {
        format!(
            "function {} ({}) references unknown block label {}",
            function.function_id, function.names.qualname, label
        )
    })
}

fn emit_planned_local_releases_for_reason_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    reason: &RefcountReleaseReason,
    local_env: &mut LocalEnv,
    forwarded_locations: &HashSet<LocalLocation>,
    emit_ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    emit_planned_local_releases_for_reason_with_local_env_excluding(
        fb,
        source_label,
        reason,
        local_env,
        forwarded_locations,
        &HashSet::new(),
        emit_ctx,
    )
}

fn emit_planned_local_releases_for_reason_with_local_env_excluding(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    reason: &RefcountReleaseReason,
    local_env: &mut LocalEnv,
    forwarded_locations: &HashSet<LocalLocation>,
    unmaterialized_locations: &HashSet<LocalLocation>,
    emit_ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    let Some(block_plan) = emit_ctx.refcount_plan.block(source_label) else {
        return Ok(());
    };
    let is_non_exit_release_reason = matches!(
        reason,
        RefcountReleaseReason::Jump { .. }
            | RefcountReleaseReason::IfThen { .. }
            | RefcountReleaseReason::IfElse { .. }
            | RefcountReleaseReason::BranchCase { .. }
            | RefcountReleaseReason::BranchDefault { .. }
            | RefcountReleaseReason::ExceptionEdge { .. }
    );
    for action in &block_plan.actions {
        let RefcountActionKind::ReleaseLocal {
            local,
            reason: action_reason,
            ..
        } = &action.kind
        else {
            continue;
        };
        if action_reason != reason {
            continue;
        }
        if matches!(reason, RefcountReleaseReason::Raise)
            && emit_ctx
                .exception_forwarded_local_names
                .is_some_and(|names| {
                    names.iter().any(|name| {
                        name == &local.name
                            || local_env
                                .entry_index_for_block_arg_name(name)
                                .and_then(|index| local_env.entries[index].location)
                                == Some(local.location)
                    })
                })
        {
            continue;
        }
        if matches!(reason, RefcountReleaseReason::Raise)
            && local_env
                .entry_index_for_location(local.location)
                .or_else(|| local_env.entry_index_for_name(&local.name))
                .is_some_and(|index| {
                    emit_ctx
                        .consts
                        .step_null_args
                        .contains(&local_env.entries[index].value())
                })
        {
            continue;
        }
        if forwarded_locations.contains(&local.location) {
            // Cleanup-only locals may be forwarded as block params even when the semantic
            // ownership plan releases them on this edge. In that representation the target
            // block owns the cleanup obligation instead of a source-side stack slot.
            continue;
        }
        if unmaterialized_locations.contains(&local.location) {
            continue;
        }
        let removed = local_env.remove_location_or_name(local.location, &local.name);
        let is_cleanup_root = emit_ctx
            .stack_slots
            .has_cleanup_root_name(local.name.as_str());
        let retire_to_frame_root = is_non_exit_release_reason && is_cleanup_root;
        if retire_to_frame_root {
            match removed.as_ref().map(|entry| entry.storage) {
                Some(LocalEnvStorage::LocalOnly) => {
                    let previous = removed.as_ref().expect("checked above");
                    if previous.i64_facts().is_some() {
                        continue;
                    }
                    let (root_value, root_ref_kind) =
                        emit_local_env_entry_pyobject_for_frame_root_transfer(
                            fb, previous, emit_ctx,
                        );
                    emit_refcount_decref_location_counter(
                        fb,
                        source_label,
                        local,
                        reason,
                        emit_ctx,
                    );
                    emit_ctx
                        .stack_slots
                        .replace_transferred_value(
                            fb,
                            local.name.as_str(),
                            root_value,
                            root_ref_kind,
                            emit_ctx.consts.ptr_ty,
                            emit_ctx.consts.thread_state_value,
                            emit_ctx.incref_ref,
                            emit_ctx.decref_ref,
                            emit_ctx
                                .refcount_emitter()
                                .with_family(refcount_family_for_release_reason(reason)),
                            None,
                        )
                        .ok_or_else(|| {
                            format!(
                                "refcount plan release for block {source_label} references missing stack slot {:?}",
                                local.name
                            )
                        })?;
                }
                Some(LocalEnvStorage::StackMirror) | None => {}
            }
            continue;
        }
        if let Some(previous) = removed.as_ref()
            && transient_local_needs_decref(previous.ref_kind())
        {
            emit_refcount_decref_location_counter(fb, source_label, local, reason, emit_ctx);
            with_refcount_family(fb, Some(refcount_family_for_release_reason(reason)), |fb| {
                emit_decref_if_not_null(
                    fb,
                    emit_ctx.consts.ptr_ty,
                    emit_ctx.decref_ref,
                    emit_ctx.consts.thread_state_value,
                    previous.value(),
                );
            });
        }
        let should_clear_stack_slot = !is_cleanup_root
            && (removed
                .as_ref()
                .is_some_and(|entry| entry.storage == LocalEnvStorage::StackMirror)
                || removed.is_none());
        if should_clear_stack_slot {
            emit_refcount_decref_location_counter(fb, source_label, local, reason, emit_ctx);
            emit_ctx
                .stack_slots
                .clear_value(
                    fb,
                    local.name.as_str(),
                    emit_ctx.consts.ptr_ty,
                    emit_ctx.consts.thread_state_value,
                    emit_ctx.decref_ref,
                    emit_ctx
                        .refcount_emitter()
                        .with_family(refcount_family_for_release_reason(reason)),
                    removed
                        .as_ref()
                        .and_then(LocalEnvEntry::py_facts),
                )
                .ok_or_else(|| {
                    format!(
                        "refcount plan release for block {source_label} references missing stack slot {:?}",
                        local.name
                    )
                })?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_planned_stack_slot_releases_for_reason_from_parts(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    reason: &RefcountReleaseReason,
    forwarded_locations: &HashSet<LocalLocation>,
    refcount_plan: &FunctionRefcountPlan,
    stack_slots: &StackSlots,
    refcount_decref_location_counter_refs: &HashMap<String, CounterRef>,
    counter_slots_by_id: &[CounterRuntimeSlot],
    scalar_counter_base_value: Option<ir::Value>,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    decref_ref: ir::FuncRef,
    refcounts: RefcountEmitter,
) -> Result<(), String> {
    if !matches!(
        reason,
        RefcountReleaseReason::Return
            | RefcountReleaseReason::Raise
            | RefcountReleaseReason::Jump { .. }
            | RefcountReleaseReason::IfThen { .. }
            | RefcountReleaseReason::IfElse { .. }
            | RefcountReleaseReason::BranchCase { .. }
            | RefcountReleaseReason::BranchDefault { .. }
            | RefcountReleaseReason::ExceptionEdge { .. }
    ) {
        return Ok(());
    }
    let Some(block_plan) = refcount_plan.block(source_label) else {
        return Ok(());
    };
    for action in &block_plan.actions {
        let RefcountActionKind::ReleaseLocal {
            local,
            reason: action_reason,
            ..
        } = &action.kind
        else {
            continue;
        };
        if action_reason != reason {
            continue;
        }
        if forwarded_locations.contains(&local.location) {
            // The value is carried to the exception target as a block param, so the target
            // block owns the corresponding cleanup obligation.
            continue;
        }
        if !can_release_via_stack_slot_fallback(local.name.as_str()) {
            continue;
        }
        emit_refcount_decref_location_counter_from_parts(
            fb,
            source_label,
            local,
            reason,
            refcount_decref_location_counter_refs,
            counter_slots_by_id,
            scalar_counter_base_value,
        );
        stack_slots
            .clear_value(
                fb,
                local.name.as_str(),
                ptr_ty,
                thread_state_value,
                decref_ref,
                refcounts.with_family(refcount_family_for_release_reason(reason)),
                None,
            )
            .ok_or_else(|| {
                format!(
                    "refcount plan release for block {source_label} references missing stack slot {:?}",
                    local.name
                )
            })?;
    }
    Ok(())
}

fn emit_truthy_from_value(
    fb: &mut FunctionBuilder<'_>,
    input_value: SoacValue,
    is_true_ref: ir::FuncRef,
    ctx: &JitEmitCtx<'_>,
) -> SoacValue {
    match input_value {
        SoacValue::I32 { value, facts } if facts.is_i32_bool01() => SoacValue::i32(value, facts),
        SoacValue::I32 { value, .. } => emit_i32_bool01_from_i32_result(fb, value, ctx),
        SoacValue::I64 { value, .. } => {
            let is_true = fb.ins().icmp_imm(ir::condcodes::IntCC::NotEqual, value, 0);
            emit_i32_bool01_from_cond(fb, is_true, ctx)
        }
        SoacValue::PyObject {
            value: owned_value,
            ownership,
            facts: py_facts,
        } => emit_truthy_from_pyobject_value(
            fb,
            owned_value,
            py_facts,
            is_true_ref,
            ctx,
            ownership.is_owned(),
        ),
    }
}

fn emit_truthy_from_pyobject_value(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    py_facts: PyObjFacts,
    is_true_ref: ir::FuncRef,
    ctx: &JitEmitCtx<'_>,
    owned: bool,
) -> SoacValue {
    if py_facts.is_none() || py_facts.is_false_singleton() {
        emit_release_pyobject_if_owned(fb, value, py_facts, owned, ctx);
        return emit_i32_bool01_const(fb, false, ctx);
    }
    if py_facts.is_true_singleton() {
        emit_release_pyobject_if_owned(fb, value, py_facts, owned, ctx);
        return emit_i32_bool01_const(fb, true, ctx);
    }
    if py_facts.is_exact_type(PyExactType::Bool) {
        let true_const = emit_true_const(fb, ctx);
        let is_true = fb
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, value, true_const);
        emit_release_pyobject_if_owned(fb, value, py_facts, owned, ctx);
        return emit_i32_bool01_from_cond(fb, is_true, ctx);
    }

    let truth_inst = fb.ins().call(is_true_ref, &[value]);
    let truth_value = fb.inst_results(truth_inst)[0];
    let truth_error = fb.ins().iconst(ctx.consts.i32_ty, -1);
    let is_error = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, truth_value, truth_error);
    let truth_error_block = fb.create_block();
    let truth_ok_block = fb.create_block();
    fb.append_block_param(truth_ok_block, ctx.consts.i32_ty);
    fb.ins().brif(
        is_error,
        truth_error_block,
        &[],
        truth_ok_block,
        &[ir::BlockArg::Value(truth_value)],
    );

    fb.switch_to_block(truth_error_block);
    emit_release_pyobject_if_owned(fb, value, py_facts, owned, ctx);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(truth_ok_block);
    let truth_ok_value = fb.block_params(truth_ok_block)[0];
    emit_release_pyobject_if_owned(fb, value, py_facts, owned, ctx);
    emit_i32_bool01_from_i32_result(fb, truth_ok_value, ctx)
}

fn emit_codegen_expr_value_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrBlockPy,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> SoacValue {
    let facts = py_facts_for_codegen_expr_with_local_env(expr, local_env, emit_ctx)
        .unwrap_or_else(PyObjFacts::unknown);
    let value = emit_codegen_expr_with_local_env(
        fb,
        expr,
        local_env,
        emit_ctx,
        borrowed,
        codegen_env,
        func_imports,
    );
    let ownership = if facts.is_immortal() {
        ValueOwnership::Immortal
    } else if borrowed {
        ValueOwnership::Borrowed
    } else {
        ValueOwnership::Owned
    };
    SoacValue::pyobject_with_ownership(value, ownership, facts)
}

fn prepare_typed_guard_miss_dispatch_for_instr(
    emit_ctx: &JitEmitCtx<'_>,
    instr_id: InstrId,
    pre_guard_operands: &[&InstrTyped],
    fallback_block: ir::Block,
) -> JitGuardMissDispatch {
    let guard_miss_resume_point =
        emit_ctx
            .guard_miss_resume_point
            .unwrap_or(LocalEnvResumePoint::BeforeInstr {
                key: InstrKey::new(emit_ctx.function_id, instr_id),
            });
    prepare_optional_guard_miss_dispatch(
        emit_ctx.guard_miss_target_for_typed_resume_point(
            guard_miss_resume_point,
            pre_guard_operands,
            fallback_block,
        ),
        fallback_block,
        emit_ctx.guard_miss_deopt_ref_for_instr_id(instr_id),
    )
}

fn emit_typed_guard_miss_deopt_resume_return(
    fb: &mut FunctionBuilder<'_>,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    block: ir::Block,
    fallback_counter_id: Option<CounterRef>,
    owned_inputs: &[ir::Value],
    target: JitDeoptExitRef,
    deopt_resume_ref: ir::FuncRef,
) {
    fb.switch_to_block(block);
    fb.set_cold_block(block);
    if let Some(counter_id) = fallback_counter_id {
        emit_increment_counter_ref(fb, counter_id, emit_ctx);
    }
    emit_release_owned_inputs(fb, emit_ctx, owned_inputs);
    let deopt_result = emit_deopt_resume_call_with_local_env(
        fb,
        target,
        deopt_resume_ref,
        emit_ctx.consts.block_const,
        emit_ctx,
        local_env,
    );
    emit_deopt_result_return_or_step_null(fb, emit_ctx, deopt_result);
}

fn emit_typed_getattr_fallback(
    fb: &mut FunctionBuilder<'_>,
    op: &TypedGetAttr<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<SoacValue, String> {
    let (value, value_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.value.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed getattr receiver",
    )?;
    let (attr, attr_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.attr.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed getattr attr",
    )?;
    let mut owned_inputs = Vec::with_capacity(2);
    push_owned_typed_input_cleanup(&mut owned_inputs, value, value_is_borrowed);
    push_owned_typed_input_cleanup(&mut owned_inputs, attr, attr_is_borrowed);
    if let Some(counter_id) = emit_ctx
        .field_generic_getattr_counter_ids
        .get(&op.semantic_instr_id())
        .copied()
    {
        emit_increment_counter_ref(fb, counter_id, emit_ctx);
    }
    let getattr_inst = fb.ins().call(emit_ctx.pyobject_getattr_ref, &[value, attr]);
    let result = emit_decref_owned_inputs_after_nullable_result(
        fb,
        emit_ctx,
        fb.inst_results(getattr_inst)[0],
        owned_inputs.as_slice(),
    );
    let result = emit_checked_owned_pyobject_result(fb, result, emit_ctx);
    Ok(SoacValue::pyobject(result, PyObjFacts::unknown()))
}

fn emit_typed_setattr_fallback(
    fb: &mut FunctionBuilder<'_>,
    op: &TypedSetAttr<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let (value, value_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.value.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed setattr receiver",
    )?;
    let (attr, attr_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.attr.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed setattr attr",
    )?;
    let (replacement, replacement_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.replacement.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed setattr replacement",
    )?;
    let mut owned_inputs = Vec::with_capacity(3);
    push_owned_typed_input_cleanup(&mut owned_inputs, value, value_is_borrowed);
    push_owned_typed_input_cleanup(&mut owned_inputs, attr, attr_is_borrowed);
    push_owned_typed_input_cleanup(&mut owned_inputs, replacement, replacement_is_borrowed);
    if let Some(counter_id) = emit_ctx
        .field_generic_setattr_counter_ids
        .get(&op.semantic_instr_id())
        .copied()
    {
        emit_increment_counter_ref(fb, counter_id, emit_ctx);
    }
    let setattr_inst = fb
        .ins()
        .call(emit_ctx.pyobject_setattr_ref, &[value, attr, replacement]);
    let result = emit_decref_owned_inputs_after_nullable_result(
        fb,
        emit_ctx,
        fb.inst_results(setattr_inst)[0],
        owned_inputs.as_slice(),
    );
    Ok(emit_checked_owned_pyobject_result_for_demand(
        fb,
        result,
        PyObjFacts::none_singleton(),
        emit_ctx,
        demand,
    ))
}

fn typed_indexed_field_counter_ref(
    instr_id: InstrId,
    counter_source: Option<TypedIndexedFieldCounterSource>,
    local_counters: &HashMap<InstrId, CounterRef>,
    source_counters: &HashMap<(RuntimeFunctionId, InstrId), CounterRef>,
) -> Option<CounterRef> {
    counter_source
        .and_then(|source| {
            source_counters
                .get(&(source.function_id, source.instr_id))
                .copied()
        })
        .or_else(|| local_counters.get(&instr_id).copied())
}

fn emit_typed_indexed_getattr(
    fb: &mut FunctionBuilder<'_>,
    op: &TypedGetAttr<InstrTyped>,
    source: TypedIndexedFieldPlanSource,
    counter_source: Option<TypedIndexedFieldCounterSource>,
    guards: &[TypedIndexedFieldGuard],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<SoacValue>, String> {
    let instr_id = op.semantic_instr_id();
    let Some(plan) = IndexedFieldLoweringPlan::for_access(
        instr_id,
        source,
        guards,
        PlanV3IndexedFieldAccessKind::Load,
    )?
    else {
        return Ok(None);
    };
    let (value, value_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.value.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed indexed getattr receiver",
    )?;
    let (attr, attr_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.attr.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed indexed getattr attr",
    )?;
    let mut owned_inputs = Vec::with_capacity(2);
    push_owned_typed_input_cleanup(&mut owned_inputs, value, value_is_borrowed);
    push_owned_typed_input_cleanup(&mut owned_inputs, attr, attr_is_borrowed);
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let hit_counter_id = typed_indexed_field_counter_ref(
        instr_id,
        counter_source,
        emit_ctx.field_indexed_hit_counter_ids,
        emit_ctx.field_indexed_hit_counter_ids_by_source,
    );
    let fallback_counter_id = typed_indexed_field_counter_ref(
        instr_id,
        counter_source,
        emit_ctx.field_indexed_fallback_counter_ids,
        emit_ctx.field_indexed_fallback_counter_ids_by_source,
    );

    let result_block = fb.create_block();
    fb.append_block_param(result_block, ptr_ty);
    let fallback_block = fb.create_block();
    fb.set_cold_block(fallback_block);
    let pre_guard_operands = [op.value.as_ref(), op.attr.as_ref()];
    let guard_miss_dispatch = prepare_typed_guard_miss_dispatch_for_instr(
        emit_ctx,
        instr_id,
        &pre_guard_operands,
        fallback_block,
    );

    for (index, specialization) in plan.specializations.iter().enumerate() {
        let Some(owner_type) = plan.require_type_ptr(
            instr_id,
            specialization,
            emit_type_ptr_value_for_ref(fb, codegen_env, emit_ctx, &specialization.owner_type_ref)
                .map_err(|err| {
                    format!(
                        "failed to bind field-indexed get owner type for {}: {err}",
                        instr_id
                    )
                })?,
        )?
        else {
            continue;
        };
        let maybe_direct_block = fb.create_block();
        let direct_block = fb.create_block();
        fb.append_block_param(direct_block, ptr_ty);
        let next_guard_block = if index + 1 == plan.specializations.len() {
            fallback_block
        } else {
            fb.create_block()
        };
        let type_matches =
            emit_exact_type_version_match(fb, value, owner_type, specialization.type_version);
        fb.ins()
            .brif(type_matches, maybe_direct_block, &[], next_guard_block, &[]);

        fb.switch_to_block(maybe_direct_block);
        emit_trusted_inline_values_field_probe(
            fb,
            value,
            owner_type,
            specialization.expected_index,
            direct_block,
            guard_miss_dispatch.branch_block(),
            emit_ctx,
        )?;

        fb.switch_to_block(direct_block);
        let direct_value = fb.block_params(direct_block)[0];
        emit_ctx.emit_incref_for_family(
            fb,
            direct_value,
            Some(PyObjFacts::unknown().with_non_null_ref()),
            RefcountFamily::BorrowedResultClone,
        );
        if let Some(counter_id) = hit_counter_id {
            emit_increment_counter_ref(fb, counter_id, emit_ctx);
        }
        emit_release_owned_inputs(fb, emit_ctx, owned_inputs.as_slice());
        fb.ins()
            .jump(result_block, &[ir::BlockArg::Value(direct_value)]);

        if index + 1 != plan.specializations.len() {
            fb.switch_to_block(next_guard_block);
        }
    }

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(fallback_block) => {
            fb.switch_to_block(fallback_block);
            if let Some(counter_id) = fallback_counter_id {
                emit_increment_counter_ref(fb, counter_id, emit_ctx);
            }
            let getattr_inst = fb.ins().call(emit_ctx.pyobject_getattr_ref, &[value, attr]);
            let fallback_value = emit_decref_owned_inputs_after_nullable_result(
                fb,
                emit_ctx,
                fb.inst_results(getattr_inst)[0],
                owned_inputs.as_slice(),
            );
            let fallback_value = emit_checked_owned_pyobject_result(fb, fallback_value, emit_ctx);
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            emit_typed_guard_miss_deopt_resume_return(
                fb,
                local_env,
                emit_ctx,
                block,
                fallback_counter_id,
                owned_inputs.as_slice(),
                target,
                deopt_resume_ref,
            );
        }
    }

    fb.switch_to_block(result_block);
    let result = fb.block_params(result_block)[0];
    Ok(Some(SoacValue::pyobject(
        result,
        PyObjFacts::unknown().with_non_null_ref(),
    )))
}

fn emit_typed_indexed_setattr(
    fb: &mut FunctionBuilder<'_>,
    op: &TypedSetAttr<InstrTyped>,
    source: TypedIndexedFieldPlanSource,
    counter_source: Option<TypedIndexedFieldCounterSource>,
    guards: &[TypedIndexedFieldGuard],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let result_needs_pyobject = match demand {
        ResultDemand::EffectOnly => false,
        ResultDemand::PyObject { .. } => true,
        ResultDemand::I32Bool01 | ResultDemand::I64 | ResultDemand::I64Index => {
            panic!("typed setattr cannot satisfy non-PyObject demand {demand:?}")
        }
    };
    let instr_id = op.semantic_instr_id();
    let Some(plan) = IndexedFieldLoweringPlan::for_access(
        instr_id,
        source,
        guards,
        PlanV3IndexedFieldAccessKind::Store,
    )?
    else {
        return Ok(None);
    };
    let (value, value_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.value.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed indexed setattr receiver",
    )?;
    let (attr, attr_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.attr.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed indexed setattr attr",
    )?;
    let (replacement, replacement_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        op.replacement.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed indexed setattr replacement",
    )?;
    let mut direct_owned_inputs = Vec::with_capacity(2);
    push_owned_typed_input_cleanup(&mut direct_owned_inputs, value, value_is_borrowed);
    push_owned_typed_input_cleanup(&mut direct_owned_inputs, attr, attr_is_borrowed);
    let mut fallback_owned_inputs = direct_owned_inputs.clone();
    push_owned_typed_input_cleanup(
        &mut fallback_owned_inputs,
        replacement,
        replacement_is_borrowed,
    );
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let hit_counter_id = typed_indexed_field_counter_ref(
        instr_id,
        counter_source,
        emit_ctx.field_indexed_hit_counter_ids,
        emit_ctx.field_indexed_hit_counter_ids_by_source,
    );
    let fallback_counter_id = typed_indexed_field_counter_ref(
        instr_id,
        counter_source,
        emit_ctx.field_indexed_fallback_counter_ids,
        emit_ctx.field_indexed_fallback_counter_ids_by_source,
    );

    let result_block = fb.create_block();
    if result_needs_pyobject {
        fb.append_block_param(result_block, ptr_ty);
    }
    let fallback_block = fb.create_block();
    fb.set_cold_block(fallback_block);
    let pre_guard_operands = [op.value.as_ref(), op.attr.as_ref(), op.replacement.as_ref()];
    let guard_miss_dispatch = prepare_typed_guard_miss_dispatch_for_instr(
        emit_ctx,
        instr_id,
        &pre_guard_operands,
        fallback_block,
    );

    for (index, specialization) in plan.specializations.iter().enumerate() {
        let Some(owner_type) = plan.require_type_ptr(
            instr_id,
            specialization,
            emit_type_ptr_value_for_ref(fb, codegen_env, emit_ctx, &specialization.owner_type_ref)
                .map_err(|err| {
                    format!(
                        "failed to bind field-indexed set owner type for {}: {err}",
                        instr_id
                    )
                })?,
        )?
        else {
            continue;
        };
        let maybe_direct_block = fb.create_block();
        let direct_block = fb.create_block();
        let next_guard_block = if index + 1 == plan.specializations.len() {
            fallback_block
        } else {
            fb.create_block()
        };
        let type_matches =
            emit_exact_type_version_match(fb, value, owner_type, specialization.type_version);
        fb.ins()
            .brif(type_matches, maybe_direct_block, &[], next_guard_block, &[]);

        fb.switch_to_block(maybe_direct_block);
        emit_trusted_inline_values_field_store(
            fb,
            value,
            owner_type,
            specialization.expected_index,
            replacement,
            replacement_is_borrowed,
            direct_block,
            guard_miss_dispatch.branch_block(),
            emit_ctx,
        )?;

        fb.switch_to_block(direct_block);
        if let Some(counter_id) = hit_counter_id {
            emit_increment_counter_ref(fb, counter_id, emit_ctx);
        }
        if result_needs_pyobject {
            let none_const = emit_none_const(fb, emit_ctx);
            emit_ctx.emit_incref_for_family(
                fb,
                none_const,
                Some(PyObjFacts::none_singleton()),
                RefcountFamily::ConstantClone,
            );
            emit_release_owned_inputs(fb, emit_ctx, direct_owned_inputs.as_slice());
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(none_const)]);
        } else {
            emit_release_owned_inputs(fb, emit_ctx, direct_owned_inputs.as_slice());
            fb.ins().jump(result_block, &[]);
        }

        if index + 1 != plan.specializations.len() {
            fb.switch_to_block(next_guard_block);
        }
    }

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(fallback_block) => {
            fb.switch_to_block(fallback_block);
            if let Some(counter_id) = fallback_counter_id {
                emit_increment_counter_ref(fb, counter_id, emit_ctx);
            }
            let setattr_inst = fb
                .ins()
                .call(emit_ctx.pyobject_setattr_ref, &[value, attr, replacement]);
            let fallback_value = emit_decref_owned_inputs_after_nullable_result(
                fb,
                emit_ctx,
                fb.inst_results(setattr_inst)[0],
                fallback_owned_inputs.as_slice(),
            );
            let fallback_result = emit_checked_owned_pyobject_result_for_demand(
                fb,
                fallback_value,
                PyObjFacts::none_singleton(),
                emit_ctx,
                demand,
            );
            if result_needs_pyobject {
                let (fallback_value, ownership, _) =
                    fallback_result.expect_pyobject("typed indexed setattr fallback result");
                debug_assert!(ownership.is_owned());
                fb.ins()
                    .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);
            } else {
                debug_assert!(matches!(fallback_result, EmitResult::NoValue));
                fb.ins().jump(result_block, &[]);
            }
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            emit_typed_guard_miss_deopt_resume_return(
                fb,
                local_env,
                emit_ctx,
                block,
                fallback_counter_id,
                fallback_owned_inputs.as_slice(),
                target,
                deopt_resume_ref,
            );
        }
    }

    fb.switch_to_block(result_block);
    Ok(Some(if result_needs_pyobject {
        let result = fb.block_params(result_block)[0];
        EmitResult::owned_pyobject(result, PyObjFacts::none_singleton())
    } else {
        EmitResult::no_value()
    }))
}

#[allow(dead_code)]
fn emit_typed_codegen_expr_value_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<SoacValue, String> {
    if let InstrTyped::Load(op) = expr {
        if let Some((value, facts)) = local_env.scalar_i64_value_for_load(&op.name) {
            return Ok(SoacValue::i64(value, facts));
        }
        if let Some((value, facts)) = local_env.scalar_i32_bool01_value_for_load(&op.name) {
            return Ok(SoacValue::i32(value, facts));
        }
        let facts = op
            .extra()
            .result_facts()
            .and_then(ValueFacts::as_pyobj)
            .unwrap_or_else(PyObjFacts::unknown);
        let value = if let Some(plan) = op.extra().indexed_global_access_plan() {
            if plan.access != PlanV3IndexedGlobalAccessKind::Load {
                return Err(format!(
                    "typed indexed-global plan for {} expected {:?}, but typed load requires Load",
                    plan.instr_id, plan.access
                ));
            }
            emit_planned_indexed_global_load(
                fb,
                emit_ctx.consts.block_const,
                plan.name.as_str(),
                plan.expected_index,
                plan.instr_id,
                local_env,
                emit_ctx,
            )
        } else {
            emit_resolved_name_load_with_local_env(
                fb,
                &op.name,
                op.try_semantic_instr_id(),
                local_env,
                emit_ctx,
                borrowed,
            )
        };
        let ownership = if facts.is_immortal() {
            ValueOwnership::Immortal
        } else if borrowed {
            ValueOwnership::Borrowed
        } else {
            ValueOwnership::Owned
        };
        return Ok(SoacValue::pyobject_with_ownership(value, ownership, facts));
    }

    if let InstrTyped::IncrementCounter(op) = expr {
        assert!(
            !borrowed,
            "typed increment_counter must not request a borrowed result"
        );
        let value = emit_increment_counter(fb, op.counter_id, emit_ctx);
        return Ok(SoacValue::pyobject(value, PyObjFacts::none_singleton()));
    }

    if let InstrTyped::MakeFunctionWithClosure(op) = expr {
        assert!(
            !borrowed,
            "typed MakeFunctionWithClosure must not request a borrowed result"
        );
        let value = emit_typed_codegen_make_function_with_closure_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?;
        return Ok(SoacValue::pyobject(value, PyObjFacts::unknown()));
    }

    if let InstrTyped::CellRef(op) = expr {
        assert!(
            !borrowed,
            "typed CellRef must not request a borrowed result"
        );
        let value = emit_raw_cell_object_for_location_with_local_env(
            fb,
            op.location,
            "typed cell_ref",
            local_env,
            emit_ctx,
        );
        return Ok(SoacValue::pyobject(value, PyObjFacts::unknown()));
    }

    if let InstrTyped::Store(op) = expr {
        assert!(!borrowed, "typed Store must not request a borrowed result");
        if let Some(result) = emit_typed_preserved_store_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )? {
            let (value, ownership, facts) = result.expect_pyobject("typed preserved store result");
            assert!(
                ownership.is_owned(),
                "typed preserved store expression should produce an owned PyObject"
            );
            return Ok(SoacValue::pyobject(value, facts));
        }
        if let Some(result) = emit_typed_local_store_result_with_local_env(
            fb,
            expr,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )? {
            let (value, ownership, facts) = result.expect_pyobject("typed local store result");
            assert!(
                ownership.is_owned(),
                "typed local store expression should produce an owned PyObject"
            );
            return Ok(SoacValue::pyobject(value, facts));
        }
        if let Some(result) = emit_typed_owned_cell_makecell_store_result_with_local_env(
            fb,
            expr,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )? {
            let (value, ownership, facts) =
                result.expect_pyobject("typed owned cell MakeCell store result");
            assert!(
                ownership.is_owned(),
                "typed owned cell MakeCell store expression should produce an owned PyObject"
            );
            return Ok(SoacValue::pyobject(value, facts));
        }
        if let Some(result) = emit_typed_cell_store_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )? {
            let (value, ownership, facts) = result.expect_pyobject("typed cell store result");
            assert!(
                ownership.is_owned(),
                "typed cell store expression should produce an owned PyObject"
            );
            return Ok(SoacValue::pyobject(value, facts));
        }
    }

    if let InstrTyped::Del(op) = expr {
        assert!(!borrowed, "typed Del must not request a borrowed result");
        if let Some(result) = emit_typed_preserved_delete_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
        ) {
            let (value, ownership, facts) = result.expect_pyobject("typed preserved delete result");
            assert!(
                ownership.is_owned(),
                "typed preserved delete expression should produce an owned PyObject"
            );
            return Ok(SoacValue::pyobject(value, facts));
        }
        if let Some(result) = emit_typed_local_delete_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
        ) {
            let (value, ownership, facts) = result.expect_pyobject("typed local delete result");
            assert!(
                ownership.is_owned(),
                "typed local delete expression should produce an owned PyObject"
            );
            return Ok(SoacValue::pyobject(value, facts));
        }
        if let Some(result) = emit_typed_cell_delete_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        ) {
            let (value, ownership, facts) = result.expect_pyobject("typed cell delete result");
            assert!(
                ownership.is_owned(),
                "typed cell delete expression should produce an owned PyObject"
            );
            return Ok(SoacValue::pyobject(value, facts));
        }
    }

    if let InstrTyped::Truthy(op) = expr {
        let borrowed =
            typed_expr_pyobject_input_is_borrowed_from_local_env(op.value(), local_env, emit_ctx);
        let value = emit_typed_codegen_expr_value_with_local_env(
            fb,
            op.value(),
            local_env,
            emit_ctx,
            borrowed,
            codegen_env,
            func_imports,
        )?;
        let is_true_ref = func_imports.get(codegen_env, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?;
        return Ok(emit_truthy_from_value(fb, value, is_true_ref, emit_ctx));
    }

    if matches!(expr, InstrTyped::BinOp(_) | InstrTyped::UnaryOp(_)) {
        assert!(
            !borrowed,
            "typed operation expression must not use borrowed result"
        );
        let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
            fb,
            local_env,
            ctx: emit_ctx,
            codegen_env,
            func_imports,
            owned_transfer_temp_load: None,
        };
        if let Some(value) = intrinsics::emit_typed_operation(expr, &mut intrinsic_state) {
            let (ownership, facts) = planned_owned_pyobject_result_for_typed_expr(expr, local_env);
            return Ok(SoacValue::pyobject_with_ownership(value, ownership, facts));
        }
    }

    if let InstrTyped::Tuple(op) = expr {
        assert!(
            !borrowed,
            "typed tuple expression must not request a borrowed result"
        );
        let value = emit_typed_tuple_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?;
        return Ok(SoacValue::pyobject(value, PyObjFacts::unknown()));
    }

    if typed_intrinsic_operation_may_emit_pyobject(expr) {
        assert!(
            !borrowed,
            "typed intrinsic operation expression must not request a borrowed result"
        );
        let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
            fb,
            local_env,
            ctx: emit_ctx,
            codegen_env,
            func_imports,
            owned_transfer_temp_load: None,
        };
        if let Some(value) = intrinsics::emit_typed_operation(expr, &mut intrinsic_state) {
            let facts = expr
                .result_facts()
                .and_then(ValueFacts::as_pyobj)
                .unwrap_or_else(PyObjFacts::unknown);
            return Ok(SoacValue::pyobject_with_ownership(
                value,
                ValueOwnership::Owned,
                facts,
            ));
        }
    }

    if let InstrTyped::CallTyped(op) = expr {
        if let Some(result) = emit_typed_codegen_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )? {
            let (value, ownership, facts) = result.expect_pyobject("typed call expression result");
            assert!(
                ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED),
                "typed call expression result should satisfy owned PyObject demand"
            );
            return Ok(SoacValue::pyobject_with_ownership(value, ownership, facts));
        }
        let legacy_expr = lower_typed_instr_to_codegen_legacy_for_fallback(expr.clone())?;
        return Ok(emit_codegen_expr_value_with_local_env(
            fb,
            &legacy_expr,
            local_env,
            emit_ctx,
            borrowed,
            codegen_env,
            func_imports,
        ));
    }

    if let InstrTyped::GuardedCallableCallTyped(op) = expr {
        let result = emit_typed_codegen_guarded_callable_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )?;
        let (value, ownership, facts) =
            result.expect_pyobject("typed guarded callable call expression result");
        assert!(
            ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED),
            "typed guarded callable call expression result should satisfy owned PyObject demand"
        );
        return Ok(SoacValue::pyobject_with_ownership(value, ownership, facts));
    }

    if let InstrTyped::GuardedMethodCallTyped(op) = expr {
        let result = emit_typed_codegen_guarded_method_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )?;
        let (value, ownership, facts) =
            result.expect_pyobject("typed guarded method call expression result");
        assert!(
            ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED),
            "typed guarded method call expression result should satisfy owned PyObject demand"
        );
        return Ok(SoacValue::pyobject_with_ownership(value, ownership, facts));
    }

    if let InstrTyped::DirectCallableCallTyped(op) = expr {
        let result = emit_typed_codegen_direct_callable_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )?;
        let (value, ownership, facts) =
            result.expect_pyobject("typed direct callable call expression result");
        assert!(
            ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED),
            "typed direct callable call expression result should satisfy owned PyObject demand"
        );
        return Ok(SoacValue::pyobject_with_ownership(value, ownership, facts));
    }

    if let InstrTyped::DirectMethodCallTyped(op) = expr {
        let result = emit_typed_codegen_direct_method_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )?;
        let (value, ownership, facts) =
            result.expect_pyobject("typed direct method call expression result");
        assert!(
            ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED),
            "typed direct method call expression result should satisfy owned PyObject demand"
        );
        return Ok(SoacValue::pyobject_with_ownership(value, ownership, facts));
    }

    if let InstrTyped::DirectCallGuardTest(op) = expr {
        return emit_typed_direct_call_guard_test_value_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        );
    }

    if let InstrTyped::GetAttrTyped(op) = expr
        && let TypedAttrAccessPlan::IndexedField {
            source,
            counter_source,
            guards,
        } = &op.access
    {
        let maybe_value = emit_typed_indexed_getattr(
            fb,
            op,
            *source,
            *counter_source,
            guards,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?;
        if let Some(value) = maybe_value {
            return Ok(value);
        }
    }

    if let InstrTyped::SetAttrTyped(op) = expr
        && let TypedAttrAccessPlan::IndexedField {
            source,
            counter_source,
            guards,
        } = &op.access
    {
        let maybe_value = emit_typed_indexed_setattr(
            fb,
            op,
            *source,
            *counter_source,
            guards,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )?;
        if let Some(value) = maybe_value {
            let (value, ownership, facts) =
                value.expect_pyobject("typed indexed setattr expression result");
            assert!(
                ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED),
                "typed indexed setattr expression result should satisfy owned PyObject demand"
            );
            return Ok(SoacValue::pyobject_with_ownership(value, ownership, facts));
        }
    }

    if let InstrTyped::GetAttrTyped(op) = expr {
        return emit_typed_getattr_fallback(fb, op, local_env, emit_ctx, codegen_env, func_imports);
    }

    if let InstrTyped::SetAttrTyped(op) = expr {
        let result = emit_typed_setattr_fallback(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )?;
        let (value, ownership, facts) = result.expect_pyobject("typed setattr expression result");
        assert!(
            ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED),
            "typed setattr expression result should satisfy owned PyObject demand"
        );
        return Ok(SoacValue::pyobject_with_ownership(value, ownership, facts));
    }

    let legacy_expr = lower_typed_instr_to_codegen_legacy_for_fallback(expr.clone())?;
    Ok(emit_codegen_expr_value_with_local_env(
        fb,
        &legacy_expr,
        local_env,
        emit_ctx,
        borrowed,
        codegen_env,
        func_imports,
    ))
}

struct SimpleCallParts<'a> {
    simple_args: Vec<&'a InstrBlockPy>,
    simple_keywords: Vec<(&'a str, &'a InstrBlockPy)>,
    has_unpack: bool,
}

struct TypedSimpleCallParts<'a> {
    simple_args: Vec<&'a InstrTyped>,
    simple_keywords: Vec<(&'a str, &'a InstrTyped)>,
    has_unpack: bool,
}

fn simple_call_parts(call: &soac_core::block_py::Call<InstrBlockPy>) -> SimpleCallParts<'_> {
    let mut simple_args: Vec<&InstrBlockPy> = Vec::new();
    let mut simple_keywords: Vec<(&str, &InstrBlockPy)> = Vec::new();
    let mut has_unpack = false;
    for arg in &call.args {
        match arg {
            CallArgPositional::Positional(value) => simple_args.push(value),
            CallArgPositional::Starred(_) => has_unpack = true,
        }
    }
    for keyword in &call.keywords {
        match keyword {
            CallArgKeyword::Named { arg, value } => simple_keywords.push((arg.as_str(), value)),
            CallArgKeyword::Starred(_) => has_unpack = true,
        }
    }
    SimpleCallParts {
        simple_args,
        simple_keywords,
        has_unpack,
    }
}

fn typed_simple_call_parts(call: &TypedCall<InstrTyped>) -> TypedSimpleCallParts<'_> {
    let mut simple_args: Vec<&InstrTyped> = Vec::new();
    let mut simple_keywords: Vec<(&str, &InstrTyped)> = Vec::new();
    let mut has_unpack = false;
    for arg in &call.args {
        match arg {
            CallArgPositional::Positional(value) => simple_args.push(value),
            CallArgPositional::Starred(_) => has_unpack = true,
        }
    }
    for keyword in &call.keywords {
        match keyword {
            CallArgKeyword::Named { arg, value } => simple_keywords.push((arg.as_str(), value)),
            CallArgKeyword::Starred(_) => has_unpack = true,
        }
    }
    TypedSimpleCallParts {
        simple_args,
        simple_keywords,
        has_unpack,
    }
}

fn typed_call_can_emit_simple_positional_with_typed_inputs(
    call: &TypedCall<InstrTyped>,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    let TypedSimpleCallParts {
        simple_args,
        simple_keywords,
        has_unpack,
    } = typed_simple_call_parts(call);
    if has_unpack || !simple_keywords.is_empty() {
        return false;
    }
    // If the specialized path declined this call, the generic fallback still has to emit
    // typed-only argument expressions instead of forcing the whole call through legacy BlockPy.
    // Keep the cell_ref helper on the legacy path because it is ABI-shaped, not a normal call.
    if matches!(
        typed_expr_runtime_helper(call.func.as_ref(), emit_ctx),
        Some(RuntimeHelperId::CellRef)
    ) {
        return false;
    }
    if simple_args.len() == 3
        && matches!(
            typed_expr_helper_name(call.func.as_ref(), emit_ctx.module_constants),
            Some("call_super")
        )
    {
        return false;
    }

    true
}

fn typed_simple_positional_arg_refs<'a>(
    args: &'a [CallArgPositional<InstrTyped>],
    keywords: &[CallArgKeyword<InstrTyped>],
    context: &str,
) -> Result<Vec<&'a InstrTyped>, String> {
    if !keywords.is_empty() {
        return Err(format!("{context} with keyword args is not supported"));
    }
    let mut positional = Vec::with_capacity(args.len());
    for arg in args {
        let CallArgPositional::Positional(arg) = arg else {
            return Err(format!("{context} with starred args is not supported"));
        };
        positional.push(arg);
    }
    Ok(positional)
}

fn typed_simple_positional_args(call: &TypedCall<InstrTyped>) -> Result<Vec<&InstrTyped>, String> {
    typed_simple_positional_arg_refs(
        call.args.as_slice(),
        call.keywords.as_slice(),
        "typed call with typed-only children",
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_positional_arg_values(
    fb: &mut FunctionBuilder<'_>,
    args: &[&InstrTyped],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(Vec<ir::Value>, Vec<bool>), String> {
    let mut arg_values = Vec::with_capacity(args.len());
    let mut arg_borrowed = Vec::with_capacity(args.len());
    for arg in args {
        let (arg_value, arg_is_borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            arg,
            local_env,
            ctx,
            codegen_env,
            func_imports,
            "typed positional call arg",
        )?;
        arg_values.push(arg_value);
        arg_borrowed.push(arg_is_borrowed);
    }
    Ok((arg_values, arg_borrowed))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_positional_call_result_with_arg_refs(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrTyped],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let (arg_values, arg_borrowed) =
        emit_typed_positional_arg_values(fb, args, local_env, emit_ctx, codegen_env, func_imports)?;
    let call_demand = if demand == ResultDemand::I32Bool01 {
        ResultDemand::PYOBJECT_OWNED
    } else {
        demand
    };
    let result = if call_demand == ResultDemand::EffectOnly && arg_values.len() <= 3 {
        emit_positional_call_three_result_with_arg_values(
            fb,
            callable,
            callable_is_borrowed,
            arg_values,
            arg_borrowed,
            emit_ctx,
            call_demand,
        )
    } else {
        emit_positional_vectorcall_result_with_arg_values(
            fb,
            callable,
            callable_is_borrowed,
            arg_values,
            arg_borrowed,
            emit_ctx,
            call_demand,
        )
    };
    if demand == ResultDemand::I32Bool01 {
        let (value, ownership, facts) = result.expect_pyobject("typed positional call bool result");
        let is_true_ref = func_imports.get(codegen_env, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?;
        return Ok(emit_soac_value_result_for_demand(
            fb,
            SoacValue::pyobject_with_ownership(value, ownership, facts),
            emit_ctx,
            demand,
            Some(is_true_ref),
        ));
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_generator_instance_call_result_with_arg_refs(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrTyped],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let (arg_values, arg_borrowed) =
        emit_typed_positional_arg_values(fb, args, local_env, emit_ctx, codegen_env, func_imports)?;
    let call_demand = if demand == ResultDemand::I32Bool01 {
        ResultDemand::PYOBJECT_OWNED
    } else {
        demand
    };
    let result = emit_generator_instance_result_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        emit_ctx,
        call_demand,
        codegen_env,
        func_imports,
    )?;
    if demand == ResultDemand::I32Bool01 {
        let (value, ownership, facts) =
            result.expect_pyobject("typed generator instance call bool result");
        let is_true_ref = func_imports.get(codegen_env, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?;
        return Ok(emit_soac_value_result_for_demand(
            fb,
            SoacValue::pyobject_with_ownership(value, ownership, facts),
            emit_ctx,
            demand,
            Some(is_true_ref),
        ));
    }
    Ok(result)
}

fn current_constructor_entry_init_function<'a>(
    emit_ctx: &'a JitEmitCtx<'_>,
) -> Option<&'a BlockPyFunction<TypedBlockPyModuleShape>> {
    let current_function = emit_ctx
        .module
        .callable_defs
        .iter()
        .find(|function| function.function_id == emit_ctx.function_id)?;
    let init_function_id = constructor_init_function_id_for_entry_function(current_function)?;
    direct_call_target_function(emit_ctx, init_function_id)
}

fn blockpy_expr_is_constructor_call(expr: &InstrBlockPy, emit_ctx: &JitEmitCtx<'_>) -> bool {
    codegen_expr_static_runtime_name(expr, emit_ctx.module_constants)
        == Some(RuntimeName::ConstructorCall.name())
        || codegen_expr_helper_name(expr, emit_ctx.module_constants)
            == Some(RuntimeName::ConstructorCall.name())
}

fn typed_expr_is_constructor_call(expr: &InstrTyped, emit_ctx: &JitEmitCtx<'_>) -> bool {
    typed_expr_static_runtime_name(expr, emit_ctx.module_constants)
        == Some(RuntimeName::ConstructorCall.name())
        || typed_expr_helper_name(expr, emit_ctx.module_constants)
            == Some(RuntimeName::ConstructorCall.name())
}

#[derive(Clone, Copy)]
enum ConstructorInitUserArgSource {
    Provided { index: usize },
    PackedRest { start: usize },
}

fn constructor_init_user_arg_sources(
    init_function: &BlockPyFunction<TypedBlockPyModuleShape>,
    user_arg_count: usize,
) -> Option<Vec<ConstructorInitUserArgSource>> {
    let first_param = init_function.params.iter().next()?;
    if !matches!(first_param.kind, ParamKind::PosOnly | ParamKind::Any) {
        return None;
    }

    let mut sources = Vec::with_capacity(init_function.params.len().saturating_sub(1));
    let mut next_user_arg = 0usize;
    let mut packed_rest = false;
    for param in init_function.params.iter().skip(1) {
        match param.kind {
            ParamKind::PosOnly | ParamKind::Any => {
                if packed_rest || next_user_arg >= user_arg_count {
                    return None;
                }
                sources.push(ConstructorInitUserArgSource::Provided {
                    index: next_user_arg,
                });
                next_user_arg += 1;
            }
            ParamKind::VarArg => {
                if packed_rest {
                    return None;
                }
                sources.push(ConstructorInitUserArgSource::PackedRest {
                    start: next_user_arg,
                });
                next_user_arg = user_arg_count;
                packed_rest = true;
            }
            ParamKind::KwOnly | ParamKind::KwArg => return None,
        }
    }
    (next_user_arg == user_arg_count).then_some(sources)
}

fn emit_constructor_init_user_arg_values(
    fb: &mut FunctionBuilder<'_>,
    sources: &[ConstructorInitUserArgSource],
    user_arg_values: &[ir::Value],
    user_arg_borrowed: &[bool],
    emit_ctx: &JitEmitCtx<'_>,
) -> (Vec<ir::Value>, Vec<bool>) {
    debug_assert_eq!(user_arg_values.len(), user_arg_borrowed.len());
    let mut arg_values = Vec::with_capacity(sources.len());
    let mut arg_borrowed = Vec::with_capacity(sources.len());
    for source in sources {
        match *source {
            ConstructorInitUserArgSource::Provided { index } => {
                arg_values.push(user_arg_values[index]);
                arg_borrowed.push(user_arg_borrowed[index]);
            }
            ConstructorInitUserArgSource::PackedRest { start } => {
                let tuple_items = user_arg_values[start..]
                    .iter()
                    .copied()
                    .zip(user_arg_borrowed[start..].iter().copied())
                    .collect::<Vec<_>>();
                arg_values.push(emit_pack_current_values_tuple(
                    fb,
                    tuple_items.as_slice(),
                    emit_ctx,
                ));
                arg_borrowed.push(false);
            }
        }
    }
    (arg_values, arg_borrowed)
}

fn emit_constructor_entry_allocation_only(
    fb: &mut FunctionBuilder<'_>,
    cls_value: ir::Value,
    cls_is_borrowed: bool,
    emit_ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let nitems = fb.ins().iconst(emit_ctx.consts.i64_ty, 0);
    let alloc_inst = fb
        .ins()
        .call(emit_ctx.pytype_generic_alloc_ref, &[cls_value, nitems]);
    let allocated = fb.inst_results(alloc_inst)[0];
    let allocated_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, allocated, null_ptr);
    let alloc_failed_block = fb.create_block();
    fb.set_cold_block(alloc_failed_block);
    let alloc_ok_block = fb.create_block();
    fb.append_block_param(alloc_ok_block, ptr_ty);
    fb.ins().brif(
        allocated_is_null,
        alloc_failed_block,
        &[],
        alloc_ok_block,
        &[ir::BlockArg::Value(allocated)],
    );

    fb.switch_to_block(alloc_failed_block);
    if !cls_is_borrowed {
        emit_ctx.emit_decref_for_family(fb, cls_value, None, RefcountFamily::OwnedTemporary);
    }
    fb.ins().jump(
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
    );

    fb.switch_to_block(alloc_ok_block);
    let allocated = fb.block_params(alloc_ok_block)[0];
    if !cls_is_borrowed {
        emit_ctx.emit_decref_for_family(fb, cls_value, None, RefcountFamily::OwnedTemporary);
    }
    allocated
}

#[allow(clippy::too_many_arguments)]
fn emit_constructor_entry_direct_init_call_with_arg_values(
    fb: &mut FunctionBuilder<'_>,
    init_function: &BlockPyFunction<TypedBlockPyModuleShape>,
    cls_value: ir::Value,
    cls_is_borrowed: bool,
    user_arg_values: Vec<ir::Value>,
    user_arg_borrowed: Vec<bool>,
    user_arg_sources: &[ConstructorInitUserArgSource],
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
) -> ir::Value {
    debug_assert_eq!(user_arg_values.len(), user_arg_borrowed.len());
    let (init_user_arg_values, init_user_arg_borrowed) = emit_constructor_init_user_arg_values(
        fb,
        user_arg_sources,
        user_arg_values.as_slice(),
        user_arg_borrowed.as_slice(),
        emit_ctx,
    );
    let mut owned_constructor_inputs = init_user_arg_values
        .iter()
        .copied()
        .zip(init_user_arg_borrowed.iter().copied())
        .filter_map(|(value, borrowed)| (!borrowed).then_some(value))
        .collect::<Vec<_>>();
    if !cls_is_borrowed {
        owned_constructor_inputs.push(cls_value);
    }
    let direct_func_id = emit_ctx
        .direct_call_functions
        .get(&init_function.function_id)
        .expect("constructor init direct target should be predeclared")
        .func_id;
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let nitems = fb.ins().iconst(emit_ctx.consts.i64_ty, 0);
    let alloc_inst = fb
        .ins()
        .call(emit_ctx.pytype_generic_alloc_ref, &[cls_value, nitems]);
    let allocated = fb.inst_results(alloc_inst)[0];
    let allocated_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, allocated, null_ptr);
    let alloc_failed_block = fb.create_block();
    fb.set_cold_block(alloc_failed_block);
    let enter_block = fb.create_block();
    fb.ins()
        .brif(allocated_is_null, alloc_failed_block, &[], enter_block, &[]);

    fb.switch_to_block(alloc_failed_block);
    emit_release_owned_inputs(fb, emit_ctx, &owned_constructor_inputs);
    fb.ins().jump(
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
    );

    fb.switch_to_block(enter_block);
    emit_ctx.direct_edge_stats.record_resolved_direct_edge();
    let enter_inst = fb.ins().call(
        emit_ctx.enter_recursive_ref,
        &[emit_ctx.consts.thread_state_value],
    );
    let enter_status = fb.inst_results(enter_inst)[0];
    let enter_failed = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::NotEqual, enter_status, 0);
    let init_call_block = fb.create_block();
    let enter_failed_block = fb.create_block();
    fb.set_cold_block(enter_failed_block);
    fb.ins()
        .brif(enter_failed, enter_failed_block, &[], init_call_block, &[]);

    fb.switch_to_block(enter_failed_block);
    let mut enter_failed_owned_inputs = owned_constructor_inputs.clone();
    enter_failed_owned_inputs.push(allocated);
    emit_release_owned_inputs(fb, emit_ctx, &enter_failed_owned_inputs);
    fb.ins().jump(
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
    );

    fb.switch_to_block(init_call_block);
    let func_ref = codegen_env
        .codegen_declare_func_in_func(direct_func_id, &mut fb.func)
        .expect("reserved constructor init function should be declared in codegen env");
    let init_function_env = current_constructor_entry_init_function(emit_ctx)
        .filter(|current_init_function| {
            current_init_function.function_id == init_function.function_id
        })
        .map(|_| emit_ctx.consts.function_env_value)
        .unwrap_or_else(|| emit_ready_constructor_entry_function_env(fb, cls_value, emit_ctx));
    let mut init_call_args = Vec::with_capacity(init_user_arg_values.len() + 3);
    init_call_args.push(init_function_env);
    init_call_args.push(emit_ctx.consts.thread_state_value);
    init_call_args.push(allocated);
    init_call_args.extend(init_user_arg_values.iter().copied());
    let init_call_inst = fb.ins().call(func_ref, &init_call_args);
    let init_result = fb.inst_results(init_call_inst)[0];
    emit_release_owned_inputs(fb, emit_ctx, &owned_constructor_inputs);
    let init_result_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, init_result, null_ptr);
    let init_ok_block = fb.create_block();
    fb.append_block_param(init_ok_block, ptr_ty);
    let init_failed_block = fb.create_block();
    fb.set_cold_block(init_failed_block);
    fb.ins().brif(
        init_result_is_null,
        init_failed_block,
        &[],
        init_ok_block,
        &[ir::BlockArg::Value(init_result)],
    );

    fb.switch_to_block(init_failed_block);
    emit_release_owned_inputs(fb, emit_ctx, &[allocated]);
    fb.ins().jump(
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
    );

    fb.switch_to_block(init_ok_block);
    let init_result = fb.block_params(init_ok_block)[0];
    let finish_inst = fb.ins().call(
        emit_ctx.finish_constructor_init_ref,
        &[allocated, init_result],
    );
    let fast_result = fb.inst_results(finish_inst)[0];
    let fast_result_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, fast_result, null_ptr);
    let fast_ok_block = fb.create_block();
    fb.append_block_param(fast_ok_block, ptr_ty);
    fb.ins().brif(
        fast_result_is_null,
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
        fast_ok_block,
        &[ir::BlockArg::Value(fast_result)],
    );
    fb.switch_to_block(fast_ok_block);
    fb.block_params(fast_ok_block)[0]
}

#[allow(clippy::too_many_arguments)]
fn emit_codegen_constructor_entry_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_core::block_py::Call<InstrBlockPy>,
    simple_args: &[&InstrBlockPy],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<ir::Value> {
    if !blockpy_expr_is_constructor_call(call.func.as_ref(), emit_ctx) {
        return None;
    }
    let init_function = current_constructor_entry_init_function(emit_ctx)?;
    if simple_args.is_empty() {
        return None;
    }
    let user_arg_sources =
        constructor_init_user_arg_sources(init_function, simple_args.len().checked_sub(1)?)?;
    if user_arg_sources.len() + 1 != init_function.params.len() {
        return None;
    }
    if !emit_ctx
        .direct_call_functions
        .contains_key(&init_function.function_id)
    {
        return None;
    }
    if !simple_args
        .iter()
        .all(|arg| codegen_expr_pyobject_input_is_borrowed_from_local_env(arg, local_env, emit_ctx))
    {
        return None;
    }
    let (arg_values, arg_borrowed) = emit_positional_arg_values(
        fb,
        simple_args,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    );
    debug_assert!(arg_borrowed.iter().all(|is_borrowed| *is_borrowed));
    let cls_value = arg_values[0];
    let cls_is_borrowed = arg_borrowed[0];
    Some(emit_constructor_entry_direct_init_call_with_arg_values(
        fb,
        init_function,
        cls_value,
        cls_is_borrowed,
        arg_values[1..].to_vec(),
        arg_borrowed[1..].to_vec(),
        user_arg_sources.as_slice(),
        emit_ctx,
        codegen_env,
    ))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_constructor_entry_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedCall<InstrTyped>,
    arg_refs: &[&InstrTyped],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let constructor_init_plan = call.extra.constructor_init_plan();
    if constructor_init_plan.is_none()
        && !typed_expr_is_constructor_call(call.func.as_ref(), emit_ctx)
    {
        return Ok(None);
    }
    let Some(init_function) = call
        .extra
        .constructor_init_plan()
        .and_then(|plan| direct_call_target_function(emit_ctx, plan.init_function_id))
        .or_else(|| current_constructor_entry_init_function(emit_ctx))
    else {
        return Ok(None);
    };
    if arg_refs.is_empty() {
        return Ok(None);
    }
    if constructor_init_plan.is_some_and(|plan| {
        plan.source == TypedConstructorInitPlanSource::InlinedConstructorEntryWithInlinedInitBody
    }) {
        let (cls_value, cls_is_borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            arg_refs[0],
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            "typed constructor allocation class arg",
        )?;
        let result =
            emit_constructor_entry_allocation_only(fb, cls_value, cls_is_borrowed, emit_ctx);
        return Ok(Some(emit_owned_pyobject_result_for_demand(
            fb,
            result,
            PyObjFacts::unknown(),
            emit_ctx,
            demand,
        )));
    }
    let Some(user_arg_sources) =
        constructor_init_user_arg_sources(init_function, arg_refs.len() - 1)
    else {
        return Ok(None);
    };
    if user_arg_sources.len() + 1 != init_function.params.len() {
        return Ok(None);
    }
    if !emit_ctx
        .direct_call_functions
        .contains_key(&init_function.function_id)
    {
        return Ok(None);
    }
    let (arg_values, arg_borrowed) = emit_typed_positional_arg_values(
        fb,
        arg_refs,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let cls_value = arg_values[0];
    let cls_is_borrowed = arg_borrowed[0];
    let result = emit_constructor_entry_direct_init_call_with_arg_values(
        fb,
        init_function,
        cls_value,
        cls_is_borrowed,
        arg_values[1..].to_vec(),
        arg_borrowed[1..].to_vec(),
        user_arg_sources.as_slice(),
        emit_ctx,
        codegen_env,
    );
    Ok(Some(
        emit_owned_pyobject_result_for_demand_with_codegen_imports(
            fb,
            result,
            PyObjFacts::unknown(),
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?,
    ))
}

#[allow(clippy::too_many_arguments)]
fn emit_codegen_simple_call_effect_only_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_core::block_py::Call<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    let SimpleCallParts {
        simple_args,
        simple_keywords,
        has_unpack,
    } = simple_call_parts(call);

    if has_unpack {
        let callable_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
            call.func.as_ref(),
            local_env,
            emit_ctx,
        );
        let callable = emit_codegen_expr_with_local_env(
            fb,
            call.func.as_ref(),
            local_env,
            emit_ctx,
            callable_is_borrowed,
            codegen_env,
            func_imports,
        );
        return Some(emit_unpack_call_result_with_local_env(
            fb,
            callable,
            callable_is_borrowed,
            call.args.as_slice(),
            call.keywords.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            ResultDemand::EffectOnly,
        ));
    }

    if !simple_keywords.is_empty() {
        let callable_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
            call.func.as_ref(),
            local_env,
            emit_ctx,
        );
        let callable = emit_codegen_expr_with_local_env(
            fb,
            call.func.as_ref(),
            local_env,
            emit_ctx,
            callable_is_borrowed,
            codegen_env,
            func_imports,
        );
        return Some(emit_keyword_call_result_with_local_env(
            fb,
            callable,
            callable_is_borrowed,
            simple_args.as_slice(),
            simple_keywords.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            ResultDemand::EffectOnly,
        ));
    }

    if codegen_expr_runtime_helper(call.func.as_ref(), emit_ctx).is_some() {
        return None;
    }
    if simple_args.len() == 3
        && matches!(
            codegen_expr_helper_name(call.func.as_ref(), emit_ctx.module_constants),
            Some("call_super")
        )
    {
        return None;
    }

    let site_instr_id = call.try_semantic_instr_id();

    let callable_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
        call.func.as_ref(),
        local_env,
        emit_ctx,
    );
    let callable = emit_codegen_expr_with_local_env(
        fb,
        call.func.as_ref(),
        local_env,
        emit_ctx,
        callable_is_borrowed,
        codegen_env,
        func_imports,
    );
    if let Some(counter_id) = site_instr_id
        .and_then(|site_instr_id| emit_ctx.call_target_counter_ids.get(&site_instr_id))
        .copied()
    {
        let callee_id = emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env);
        emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
    }
    Some(if simple_args.len() <= 3 {
        emit_positional_call_three_result_with_local_env(
            fb,
            callable,
            callable_is_borrowed,
            simple_args.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            ResultDemand::EffectOnly,
        )
    } else {
        emit_positional_vectorcall_result_with_local_env(
            fb,
            callable,
            callable_is_borrowed,
            simple_args.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            ResultDemand::EffectOnly,
        )
    })
}

fn emit_codegen_simple_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_core::block_py::Call<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    profiled_targets: Option<&[RuntimeFunctionId]>,
    typed_access: Option<&TypedCallAccessPlan>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<ir::Value> {
    let SimpleCallParts {
        simple_args,
        simple_keywords,
        has_unpack,
    } = simple_call_parts(call);

    if !has_unpack
        && simple_keywords.is_empty()
        && simple_args.is_empty()
        && codegen_expr_runtime_helper(call.func.as_ref(), emit_ctx)
            == Some(RuntimeHelperId::Globals)
    {
        emit_ctx.emit_incref_for_family(
            fb,
            emit_ctx.consts.block_const,
            None,
            RefcountFamily::ConstantClone,
        );
        return Some(emit_ctx.consts.block_const);
    }

    if !has_unpack
        && simple_keywords.is_empty()
        && let Some(value) = emit_codegen_constructor_entry_call_with_local_env(
            fb,
            call,
            simple_args.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )
    {
        return Some(value);
    }

    if !has_unpack
        && simple_keywords.is_empty()
        && simple_args.len() == 3
        && matches!(
            codegen_expr_helper_name(call.func.as_ref(), emit_ctx.module_constants),
            Some("call_super")
        )
    {
        return Some(emit_codegen_super_helper_call_with_local_env(
            fb,
            call.func.as_ref(),
            simple_args[0],
            simple_args[1],
            simple_args[2],
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        ));
    }

    if has_unpack {
        let callable_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
            call.func.as_ref(),
            local_env,
            emit_ctx,
        );
        let callable = emit_codegen_expr_with_local_env(
            fb,
            call.func.as_ref(),
            local_env,
            emit_ctx,
            callable_is_borrowed,
            codegen_env,
            func_imports,
        );
        return Some(emit_unpack_call_with_local_env(
            fb,
            callable,
            callable_is_borrowed,
            call.args.as_slice(),
            call.keywords.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        ));
    }

    if !has_unpack
        && simple_keywords.is_empty()
        && let Some(helper_id) = codegen_expr_runtime_helper(call.func.as_ref(), emit_ctx)
    {
        if helper_id == RuntimeHelperId::CellRef && simple_args.len() == 1 {
            let InstrBlockPy::Load(cell_name) = simple_args[0] else {
                panic!(
                    "cell_ref should lower to a located load arg, got {:?}",
                    simple_args[0]
                );
            };
            if cell_name.name.cell_location().is_some() {
                return Some(emit_raw_cell_object_for_name_with_local_env(
                    fb,
                    &cell_name.name,
                    local_env,
                    emit_ctx,
                ));
            }
            panic!(
                "cell_ref should target a cell-backed name, got {} at {:?}",
                cell_name.name.id, cell_name.name.location
            );
        }
    }

    if !has_unpack && simple_keywords.is_empty() {
        let ptr_ty = emit_ctx.consts.ptr_ty;
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let site_instr_id = call.try_semantic_instr_id();
        let call_target_counter = site_instr_id
            .and_then(|site_instr_id| emit_ctx.call_target_counter_ids.get(&site_instr_id))
            .copied();
        let direct_hit_counter_id = site_instr_id
            .and_then(|site_instr_id| emit_ctx.call_direct_hit_counter_ids.get(&site_instr_id))
            .copied();
        let direct_fallback_counter_id = site_instr_id
            .and_then(|site_instr_id| {
                emit_ctx
                    .call_direct_fallback_counter_ids
                    .get(&site_instr_id)
            })
            .copied();
        let mut sampled_protocol_target = false;
        if call_access_allows_protocol_target_sample(typed_access)
            && let Some(counter_id) = call_target_counter
            && simple_args.len() == 1
            && let Some(runtime_name) =
                codegen_expr_static_runtime_name(call.func.as_ref(), emit_ctx.module_constants)
            && let Some(helper_import) = protocol_method_target_helper_import(runtime_name)
        {
            let receiver_expr = simple_args[0];
            if codegen_expr_pyobject_input_is_borrowed_from_local_env(
                receiver_expr,
                local_env,
                emit_ctx,
            ) {
                let receiver = emit_codegen_expr_with_local_env(
                    fb,
                    receiver_expr,
                    local_env,
                    emit_ctx,
                    true,
                    codegen_env,
                    func_imports,
                );
                emit_record_protocol_method_target_sample(
                    fb,
                    receiver,
                    counter_id,
                    helper_import,
                    emit_ctx,
                    codegen_env,
                    func_imports,
                );
                sampled_protocol_target = true;
            }
        }
        if let Some(TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
            runtime_name,
            method_name,
            method_guards,
        }) = typed_access
            && matches!(*runtime_name, RuntimeName::Iter | RuntimeName::Next)
            && simple_args.len() == 1
        {
            let direct_method_specializations =
                direct_method_specializations_from_typed_guards(method_guards, method_name);
            if !direct_method_specializations.is_empty() {
                let receiver_expr = simple_args[0];
                let receiver_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
                    receiver_expr,
                    local_env,
                    emit_ctx,
                );
                let receiver = emit_codegen_expr_with_local_env(
                    fb,
                    receiver_expr,
                    local_env,
                    emit_ctx,
                    receiver_is_borrowed,
                    codegen_env,
                    func_imports,
                );
                let result_block = fb.create_block();
                fb.append_block_param(result_block, ptr_ty);
                let generic_block = fb.create_block();
                fb.set_cold_block(generic_block);

                for (index, specialization) in direct_method_specializations.iter().enumerate() {
                    let expected_type = emit_type_ptr_value_for_ref(
                        fb,
                        codegen_env,
                        emit_ctx,
                        &specialization.owner_type_ref,
                    )
                    .unwrap_or_else(|err| {
                        panic!("failed to bind runtime protocol method type symbol: {err}");
                    })
                    .expect("runtime protocol method type symbol should be available");
                    let direct_block = fb.create_block();
                    let miss_block = if index + 1 == direct_method_specializations.len() {
                        generic_block
                    } else {
                        fb.create_block()
                    };
                    let is_match = emit_exact_type_version_match(
                        fb,
                        receiver,
                        expected_type,
                        specialization.type_version,
                    );
                    fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

                    fb.switch_to_block(direct_block);
                    let target_function =
                        direct_call_target_function(emit_ctx, specialization.function_id)
                            .expect("runtime protocol method specialization target should exist");
                    if let Some(counter_id) = call_target_counter {
                        let callee_id = fb.ins().iconst(
                            emit_ctx.consts.i64_ty,
                            specialization.function_id.to_packed_runtime_u64() as i64,
                        );
                        emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
                    }
                    if let Some(counter_id) = direct_hit_counter_id {
                        emit_increment_counter_ref(fb, counter_id, emit_ctx);
                    }
                    let direct_result = emit_direct_method_resolved_with_args_from_local_env(
                        fb,
                        receiver,
                        receiver_is_borrowed,
                        &[],
                        specialization,
                        target_function,
                        local_env,
                        emit_ctx,
                        codegen_env,
                        func_imports,
                    );
                    fb.ins()
                        .jump(result_block, &[ir::BlockArg::Value(direct_result)]);
                    if index + 1 != direct_method_specializations.len() {
                        fb.switch_to_block(miss_block);
                    }
                }

                fb.switch_to_block(generic_block);
                emit_ctx
                    .direct_edge_stats
                    .record_guarded_generic_fallback_block();
                let callable = emit_checked_runtime_name_object(fb, *runtime_name, emit_ctx);
                if let Some(counter_id) = call_target_counter {
                    let callee_id =
                        emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env);
                    emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
                }
                if let Some(counter_id) = direct_fallback_counter_id {
                    emit_increment_counter_ref(fb, counter_id, emit_ctx);
                }
                let (generic_result, _, _) = emit_positional_call_three_result_with_arg_values(
                    fb,
                    callable,
                    false,
                    vec![receiver],
                    vec![receiver_is_borrowed],
                    emit_ctx,
                    ResultDemand::PYOBJECT_OWNED,
                )
                .expect_pyobject("guarded runtime protocol method fallback");
                fb.ins()
                    .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
                fb.switch_to_block(result_block);
                return Some(fb.block_params(result_block)[0]);
            }
        }
        let direct_specializations = match typed_access {
            Some(TypedCallAccessPlan::GuardedCallable { function_guards }) => {
                direct_function_specializations_from_typed_guards(function_guards)
            }
            Some(_) => Vec::new(),
            _ => call_site_profiled_targets(call, profiled_targets)
                .map(|targets| {
                    targets
                        .iter()
                        .copied()
                        .filter_map(|function_id| {
                            let Some(target_function) =
                                direct_call_target_function(emit_ctx, function_id)
                            else {
                                emit_ctx
                                    .direct_edge_stats
                                    .record_profiled_missing_target_candidate();
                                return None;
                            };
                            if target_function.names.fn_name == "__init__" {
                                return None;
                            }
                            let implicit_positional_arg_count =
                                usize::from(is_constructor_entry_function(target_function));
                            let arg_plan = match validate_direct_call_compatibility(
                                target_function,
                                emit_ctx.direct_call_functions,
                                simple_args.len(),
                                implicit_positional_arg_count,
                                false,
                                false,
                            ) {
                                Ok(arg_plan) => arg_plan,
                                Err(incompatibility) => {
                                    record_profiled_direct_call_incompatibility(
                                        emit_ctx.direct_edge_stats,
                                        incompatibility,
                                    );
                                    return None;
                                }
                            };
                            if is_constructor_entry_function(target_function)
                                && arg_plan.requires_default_resolving_entry()
                            {
                                return None;
                            }
                            Some(DirectFunctionSpecialization {
                                function_id,
                                arg_plan,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        };
        let direct_method_specializations = match typed_access {
            Some(TypedCallAccessPlan::GuardedMethod {
                method_name,
                method_guards,
            }) => direct_method_specializations_from_typed_guards(method_guards, method_name),
            Some(_) => Vec::new(),
            _ => Vec::new(),
        };
        if !direct_method_specializations.is_empty() {
            let InstrBlockPy::GetAttr(getattr) = call.func.as_ref() else {
                unreachable!("direct method specializations require GetAttr call target");
            };
            let receiver_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
                getattr.value.as_ref(),
                local_env,
                emit_ctx,
            );
            let receiver = emit_codegen_expr_with_local_env(
                fb,
                getattr.value.as_ref(),
                local_env,
                emit_ctx,
                receiver_is_borrowed,
                codegen_env,
                func_imports,
            );
            let result_block = fb.create_block();
            fb.append_block_param(result_block, ptr_ty);
            let generic_block = fb.create_block();
            fb.set_cold_block(generic_block);
            let method_guard_miss_resume_point = emit_ctx.guard_miss_resume_point.or_else(|| {
                site_instr_id.map(|site_instr_id| LocalEnvResumePoint::BeforeInstr {
                    key: InstrKey::new(emit_ctx.function_id, site_instr_id),
                })
            });
            let method_guard_miss_dispatch = method_guard_miss_resume_point
                .map(|guard_miss_resume_point| {
                    prepare_optional_guard_miss_dispatch(
                        emit_ctx.guard_miss_target_for_codegen_resume_point(
                            guard_miss_resume_point,
                            &[getattr.value.as_ref()],
                            generic_block,
                        ),
                        generic_block,
                        site_instr_id.and_then(|instr_id| {
                            emit_ctx.guard_miss_deopt_ref_for_instr_id(instr_id)
                        }),
                    )
                })
                .unwrap_or(JitGuardMissDispatch::FallbackBlock(generic_block));
            for (index, specialization) in direct_method_specializations.iter().enumerate() {
                let Some(expected_type) = emit_type_ptr_value_for_ref(
                    fb,
                    codegen_env,
                    emit_ctx,
                    &specialization.owner_type_ref,
                )
                .unwrap_or_else(|err| {
                    panic!("failed to bind direct method type symbol: {err}");
                }) else {
                    continue;
                };
                let direct_block = fb.create_block();
                let miss_block = if index + 1 == direct_method_specializations.len() {
                    method_guard_miss_dispatch.branch_block()
                } else {
                    fb.create_block()
                };
                let is_match = emit_exact_type_version_match(
                    fb,
                    receiver,
                    expected_type,
                    specialization.type_version,
                );
                fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

                fb.switch_to_block(direct_block);
                let target_function =
                    direct_call_target_function(emit_ctx, specialization.function_id)
                        .expect("direct method specialization target should exist");
                if let Some(counter_id) = call_target_counter {
                    let callee_id = fb.ins().iconst(
                        emit_ctx.consts.i64_ty,
                        specialization.function_id.to_packed_runtime_u64() as i64,
                    );
                    emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
                }
                if let Some(counter_id) = direct_hit_counter_id {
                    emit_increment_counter_ref(fb, counter_id, emit_ctx);
                }
                let direct_result = emit_direct_method_resolved_with_args_from_local_env(
                    fb,
                    receiver,
                    receiver_is_borrowed,
                    simple_args.as_slice(),
                    specialization,
                    target_function,
                    local_env,
                    emit_ctx,
                    codegen_env,
                    func_imports,
                );
                fb.ins()
                    .jump(result_block, &[ir::BlockArg::Value(direct_result)]);
                if index + 1 != direct_method_specializations.len() {
                    fb.switch_to_block(miss_block);
                }
            }

            match method_guard_miss_dispatch {
                JitGuardMissDispatch::FallbackBlock(generic_block) => {
                    fb.switch_to_block(generic_block);
                    let attr_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
                        getattr.attr.as_ref(),
                        local_env,
                        emit_ctx,
                    );
                    let attr = emit_codegen_expr_with_local_env(
                        fb,
                        getattr.attr.as_ref(),
                        local_env,
                        emit_ctx,
                        attr_is_borrowed,
                        codegen_env,
                        func_imports,
                    );
                    let getattr_inst = fb
                        .ins()
                        .call(emit_ctx.pyobject_getattr_ref, &[receiver, attr]);
                    let mut owned_inputs = Vec::with_capacity(2);
                    if !attr_is_borrowed {
                        owned_inputs.push(attr);
                    }
                    if !receiver_is_borrowed {
                        owned_inputs.push(receiver);
                    }
                    let callable = emit_decref_owned_inputs_after_nullable_result(
                        fb,
                        emit_ctx,
                        fb.inst_results(getattr_inst)[0],
                        &owned_inputs,
                    );
                    let callable_is_null =
                        fb.ins()
                            .icmp(ir::condcodes::IntCC::Equal, callable, null_ptr);
                    let callable_ok_block = fb.create_block();
                    fb.append_block_param(callable_ok_block, ptr_ty);
                    fb.ins().brif(
                        callable_is_null,
                        emit_ctx.consts.step_null_block,
                        &step_null_block_args(emit_ctx),
                        callable_ok_block,
                        &[ir::BlockArg::Value(callable)],
                    );
                    fb.switch_to_block(callable_ok_block);
                    let callable = fb.block_params(callable_ok_block)[0];
                    if let Some(counter_id) = call_target_counter {
                        let callee_id =
                            emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env);
                        emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
                    }
                    if let Some(counter_id) = direct_fallback_counter_id {
                        emit_increment_counter_ref(fb, counter_id, emit_ctx);
                    }
                    let generic_result = if simple_args.len() <= 3 {
                        emit_positional_call_three_with_local_env(
                            fb,
                            callable,
                            false,
                            simple_args.as_slice(),
                            local_env,
                            emit_ctx,
                            codegen_env,
                            func_imports,
                        )
                    } else {
                        emit_positional_vectorcall_with_local_env(
                            fb,
                            callable,
                            false,
                            simple_args.as_slice(),
                            local_env,
                            emit_ctx,
                            codegen_env,
                            func_imports,
                        )
                    };
                    fb.ins()
                        .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
                }
                JitGuardMissDispatch::DeoptResume {
                    block,
                    target,
                    deopt_resume_ref,
                } => {
                    fb.switch_to_block(block);
                    fb.set_cold_block(block);
                    if let Some(counter_id) = direct_fallback_counter_id {
                        emit_increment_counter_ref(fb, counter_id, emit_ctx);
                    }
                    if !receiver_is_borrowed {
                        emit_release_owned_inputs(fb, emit_ctx, &[receiver]);
                    }
                    let deopt_result = emit_deopt_resume_call_with_local_env(
                        fb,
                        target,
                        deopt_resume_ref,
                        emit_ctx.consts.block_const,
                        emit_ctx,
                        local_env,
                    );
                    emit_deopt_result_return_or_step_null(fb, emit_ctx, deopt_result);
                }
            }
            fb.switch_to_block(result_block);
            return Some(fb.block_params(result_block)[0]);
        }
        let callable_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
            call.func.as_ref(),
            local_env,
            emit_ctx,
        );
        let callable = emit_codegen_expr_with_local_env(
            fb,
            call.func.as_ref(),
            local_env,
            emit_ctx,
            callable_is_borrowed,
            codegen_env,
            func_imports,
        );
        let should_emit_callee_id =
            call_target_counter.is_some() || !direct_specializations.is_empty();
        let callee_id = should_emit_callee_id
            .then(|| emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env));
        if !sampled_protocol_target && let Some(counter_id) = call_target_counter {
            let callee_id = callee_id.expect("callee id should exist for call target counter");
            emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
        }
        if !direct_specializations.is_empty() {
            let result_block = fb.create_block();
            fb.append_block_param(result_block, ptr_ty);
            let generic_block = fb.create_block();
            fb.set_cold_block(generic_block);
            let direct_guard_miss_dispatch = if let Some(site_instr_id) = site_instr_id {
                let guard_miss_resume_point =
                    emit_ctx
                        .guard_miss_resume_point
                        .unwrap_or(LocalEnvResumePoint::BeforeInstr {
                            key: InstrKey::new(emit_ctx.function_id, site_instr_id),
                        });
                prepare_optional_guard_miss_dispatch(
                    emit_ctx.guard_miss_target_for_codegen_resume_point(
                        guard_miss_resume_point,
                        &[call.func.as_ref()],
                        generic_block,
                    ),
                    generic_block,
                    emit_ctx.guard_miss_deopt_ref_for_instr_id(site_instr_id),
                )
            } else {
                JitGuardMissDispatch::FallbackBlock(generic_block)
            };
            {
                let callee_id = callee_id.expect("callee id should exist for direct call guards");
                let callable_type = fb.ins().load(
                    ptr_ty,
                    ir::MemFlags::trusted(),
                    callable,
                    offset_of!(ffi::PyObject, ob_type) as i32,
                );
                let py_function_type = emit_type_ptr_value_for_ref(
                    fb,
                    codegen_env,
                    emit_ctx,
                    &RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Function),
                )
                .unwrap_or_else(|err| panic!("failed to bind PyFunction_Type symbol: {err}"))
                .expect("PyFunction_Type symbol should be available");
                let callable_is_exact_function =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, callable_type, py_function_type);
                let py_type_type = emit_type_ptr_value_for_ref(
                    fb,
                    codegen_env,
                    emit_ctx,
                    &RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Type),
                )
                .unwrap_or_else(|err| panic!("failed to bind PyType_Type symbol: {err}"))
                .expect("PyType_Type symbol should be available");
                let callable_is_exact_type =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, callable_type, py_type_type);
                for (index, specialization) in direct_specializations.iter().enumerate() {
                    let target_function =
                        direct_call_target_function(emit_ctx, specialization.function_id)
                            .expect("direct specialization target should exist");
                    let direct_block = fb.create_block();
                    let miss_block = if index + 1 == direct_specializations.len() {
                        direct_guard_miss_dispatch.branch_block()
                    } else {
                        fb.create_block()
                    };
                    let is_match = fb.ins().icmp_imm(
                        ir::condcodes::IntCC::Equal,
                        callee_id,
                        specialization.function_id.to_packed_runtime_u64() as i64,
                    );
                    let callable_shape_matches = if is_constructor_entry_function(target_function) {
                        callable_is_exact_type
                    } else {
                        callable_is_exact_function
                    };
                    let is_match = fb.ins().band(is_match, callable_shape_matches);
                    fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

                    fb.switch_to_block(direct_block);
                    if let Some(counter_id) = direct_hit_counter_id {
                        emit_increment_counter_ref(fb, counter_id, emit_ctx);
                    }
                    let direct_result = emit_direct_call_resolved_with_arg_plan_from_local_env(
                        fb,
                        callable,
                        callable_is_borrowed,
                        simple_args.as_slice(),
                        &specialization.arg_plan,
                        target_function,
                        local_env,
                        emit_ctx,
                        codegen_env,
                        func_imports,
                    );
                    fb.ins()
                        .jump(result_block, &[ir::BlockArg::Value(direct_result)]);
                    if index + 1 != direct_specializations.len() {
                        fb.switch_to_block(miss_block);
                    }
                }
            }

            match direct_guard_miss_dispatch {
                JitGuardMissDispatch::FallbackBlock(generic_block) => {
                    fb.switch_to_block(generic_block);
                    emit_ctx
                        .direct_edge_stats
                        .record_guarded_generic_fallback_block();
                    if let Some(counter_id) = direct_fallback_counter_id {
                        emit_increment_counter_ref(fb, counter_id, emit_ctx);
                    }
                    let generic_result = if simple_args.len() <= 3 {
                        emit_positional_call_three_with_local_env(
                            fb,
                            callable,
                            callable_is_borrowed,
                            simple_args.as_slice(),
                            local_env,
                            emit_ctx,
                            codegen_env,
                            func_imports,
                        )
                    } else {
                        emit_positional_vectorcall_with_local_env(
                            fb,
                            callable,
                            callable_is_borrowed,
                            simple_args.as_slice(),
                            local_env,
                            emit_ctx,
                            codegen_env,
                            func_imports,
                        )
                    };
                    fb.ins()
                        .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
                }
                JitGuardMissDispatch::DeoptResume {
                    block,
                    target,
                    deopt_resume_ref,
                } => {
                    fb.switch_to_block(block);
                    fb.set_cold_block(block);
                    emit_ctx
                        .direct_edge_stats
                        .record_guarded_generic_fallback_block();
                    if let Some(counter_id) = direct_fallback_counter_id {
                        emit_increment_counter_ref(fb, counter_id, emit_ctx);
                    }
                    if !callable_is_borrowed {
                        emit_release_owned_inputs(fb, emit_ctx, &[callable]);
                    }
                    let deopt_result = emit_deopt_resume_call_with_local_env(
                        fb,
                        target,
                        deopt_resume_ref,
                        emit_ctx.consts.block_const,
                        emit_ctx,
                        local_env,
                    );
                    emit_deopt_result_return_or_step_null(fb, emit_ctx, deopt_result);
                }
            }
            fb.switch_to_block(result_block);
            return Some(fb.block_params(result_block)[0]);
        }
        if let Some(counter_id) = call_target_counter {
            let callee_id = callee_id.expect("callee id should exist for call target counter");
            emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
        }
        return Some(emit_positional_vectorcall_with_local_env(
            fb,
            callable,
            callable_is_borrowed,
            simple_args.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        ));
    }
    if !has_unpack && !simple_keywords.is_empty() {
        let callable_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
            call.func.as_ref(),
            local_env,
            emit_ctx,
        );
        let callable = emit_codegen_expr_with_local_env(
            fb,
            call.func.as_ref(),
            local_env,
            emit_ctx,
            callable_is_borrowed,
            codegen_env,
            func_imports,
        );
        return Some(emit_keyword_call_with_local_env(
            fb,
            callable,
            callable_is_borrowed,
            simple_args.as_slice(),
            simple_keywords.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        ));
    }

    None
}

fn emit_codegen_make_function_with_closure_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    make_function: &soac_core::block_py::MakeFunctionWithClosure<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let function_id = fb.ins().iconst(
        emit_ctx.consts.i64_ty,
        make_function.function_id().to_packed_runtime_u64() as i64,
    );
    let kind = fb.ins().iconst(
        emit_ctx.consts.i64_ty,
        make_function_kind_abi_tag(make_function.kind),
    );
    let captures_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
        make_function.captures.as_ref(),
        local_env,
        emit_ctx,
    );
    let captures = emit_codegen_expr_with_local_env(
        fb,
        make_function.captures.as_ref(),
        local_env,
        emit_ctx,
        captures_is_borrowed,
        codegen_env,
        func_imports,
    );
    let param_defaults_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
        make_function.param_defaults.as_ref(),
        local_env,
        emit_ctx,
    );
    let param_defaults = emit_codegen_expr_with_local_env(
        fb,
        make_function.param_defaults.as_ref(),
        local_env,
        emit_ctx,
        param_defaults_is_borrowed,
        codegen_env,
        func_imports,
    );
    let annotate_fn_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
        make_function.annotate_fn.as_ref(),
        local_env,
        emit_ctx,
    );
    let annotate_fn = emit_codegen_expr_with_local_env(
        fb,
        make_function.annotate_fn.as_ref(),
        local_env,
        emit_ctx,
        annotate_fn_is_borrowed,
        codegen_env,
        func_imports,
    );
    let globals = emit_ctx.consts.block_const;
    let call_inst = fb.ins().call(
        emit_ctx.make_function_with_closure_ref,
        &[
            function_id,
            kind,
            captures,
            param_defaults,
            annotate_fn,
            globals,
        ],
    );
    let mut owned_inputs = Vec::new();
    if !captures_is_borrowed {
        owned_inputs.push(captures);
    }
    if !param_defaults_is_borrowed {
        owned_inputs.push(param_defaults);
    }
    if !annotate_fn_is_borrowed {
        owned_inputs.push(annotate_fn);
    }
    let value = emit_decref_owned_inputs_after_nullable_result(
        fb,
        emit_ctx,
        fb.inst_results(call_inst)[0],
        owned_inputs.as_slice(),
    );
    let result = emit_checked_owned_pyobject_result_for_demand(
        fb,
        value,
        PyObjFacts::unknown(),
        emit_ctx,
        ResultDemand::PYOBJECT_OWNED,
    );
    let (value, ownership, _) = result.expect_pyobject("make-function-with-closure result");
    debug_assert!(ownership.is_owned());
    value
}

fn emit_typed_codegen_make_function_with_closure_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    make_function: &soac_core::block_py::MakeFunctionWithClosure<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let function_id = fb.ins().iconst(
        emit_ctx.consts.i64_ty,
        make_function.function_id().to_packed_runtime_u64() as i64,
    );
    let kind = fb.ins().iconst(
        emit_ctx.consts.i64_ty,
        make_function_kind_abi_tag(make_function.kind),
    );
    let captures = emit_typed_codegen_expr_value_with_local_env(
        fb,
        make_function.captures.as_ref(),
        local_env,
        emit_ctx,
        typed_expr_pyobject_input_is_borrowed_from_local_env(
            make_function.captures.as_ref(),
            local_env,
            emit_ctx,
        ),
        codegen_env,
        func_imports,
    )?;
    let (captures, captures_ownership, _) =
        captures.expect_pyobject("typed make-function captures");
    let param_defaults = emit_typed_codegen_expr_value_with_local_env(
        fb,
        make_function.param_defaults.as_ref(),
        local_env,
        emit_ctx,
        typed_expr_pyobject_input_is_borrowed_from_local_env(
            make_function.param_defaults.as_ref(),
            local_env,
            emit_ctx,
        ),
        codegen_env,
        func_imports,
    )?;
    let (param_defaults, param_defaults_ownership, _) =
        param_defaults.expect_pyobject("typed make-function param defaults");
    let annotate_fn = emit_typed_codegen_expr_value_with_local_env(
        fb,
        make_function.annotate_fn.as_ref(),
        local_env,
        emit_ctx,
        typed_expr_pyobject_input_is_borrowed_from_local_env(
            make_function.annotate_fn.as_ref(),
            local_env,
            emit_ctx,
        ),
        codegen_env,
        func_imports,
    )?;
    let (annotate_fn, annotate_fn_ownership, _) =
        annotate_fn.expect_pyobject("typed make-function annotation function");
    let globals = emit_ctx.consts.block_const;
    let call_inst = fb.ins().call(
        emit_ctx.make_function_with_closure_ref,
        &[
            function_id,
            kind,
            captures,
            param_defaults,
            annotate_fn,
            globals,
        ],
    );
    let mut owned_inputs = Vec::new();
    if captures_ownership.is_owned() {
        owned_inputs.push(captures);
    }
    if param_defaults_ownership.is_owned() {
        owned_inputs.push(param_defaults);
    }
    if annotate_fn_ownership.is_owned() {
        owned_inputs.push(annotate_fn);
    }
    let value = emit_decref_owned_inputs_after_nullable_result(
        fb,
        emit_ctx,
        fb.inst_results(call_inst)[0],
        owned_inputs.as_slice(),
    );
    let result = emit_checked_owned_pyobject_result_for_demand(
        fb,
        value,
        PyObjFacts::unknown(),
        emit_ctx,
        ResultDemand::PYOBJECT_OWNED,
    );
    let (value, ownership, _) = result.expect_pyobject("typed make-function-with-closure result");
    debug_assert!(ownership.is_owned());
    Ok(value)
}

fn emit_codegen_expr_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrBlockPy,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    if let InstrBlockPy::Load(op) = expr {
        return emit_resolved_name_load_with_local_env(
            fb,
            &op.name,
            op.try_semantic_instr_id(),
            local_env,
            emit_ctx,
            borrowed,
        );
    }
    if let InstrBlockPy::IncrementCounter(op) = expr {
        assert!(
            !borrowed,
            "increment_counter must not request a borrowed result"
        );
        return emit_increment_counter(fb, op.counter_id, emit_ctx);
    }
    if let InstrBlockPy::MakeFunctionWithClosure(op) = expr {
        assert!(
            !borrowed,
            "MakeFunctionWithClosure must not request a borrowed result"
        );
        return emit_codegen_make_function_with_closure_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        );
    }
    if let InstrBlockPy::Tuple(op) = expr {
        assert!(
            !borrowed,
            "tuple expression must not request a borrowed result"
        );
        return emit_codegen_tuple_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        );
    }
    if let InstrBlockPy::CellRef(op) = expr {
        assert!(
            !borrowed,
            "codegen operation expression must not use borrowed result"
        );
        return emit_raw_cell_object_for_location_with_local_env(
            fb,
            op.location,
            "cell_ref",
            local_env,
            emit_ctx,
        );
    }
    if matches!(
        expr,
        InstrBlockPy::BinOp(_)
            | InstrBlockPy::UnaryOp(_)
            | InstrBlockPy::GetAttr(_)
            | InstrBlockPy::SetAttr(_)
            | InstrBlockPy::GetItem(_)
            | InstrBlockPy::SetItem(_)
            | InstrBlockPy::DelItem(_)
            | InstrBlockPy::Store(_)
            | InstrBlockPy::Del(_)
            | InstrBlockPy::MakeCell(_)
    ) {
        assert!(
            !borrowed,
            "codegen operation expression must not use borrowed result"
        );
        let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
            fb,
            local_env,
            ctx: emit_ctx,
            codegen_env,
            func_imports,
            owned_transfer_temp_load: None,
        };
        if let Some(value) = intrinsics::emit_operation(expr, &mut intrinsic_state) {
            return value;
        }
    }
    if let InstrBlockPy::Store(op) = expr {
        if let Some(value) = emit_local_store_with_local_env(
            fb,
            expr,
            op,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        ) {
            return value;
        }
        let Some(location) = op.name.cell_location() else {
            panic!("Store should be resolved before codegen: {op:?}");
        };
        let raw_cell = emit_raw_cell_object_for_location_with_local_env(
            fb, location, "Store", local_env, emit_ctx,
        );
        let value_borrowed = codegen_expr_is_borrowable_from_local_env(
            &op.value,
            local_env,
            &emit_ctx.stack_slots,
            emit_ctx.storage_layout.as_ref(),
        );
        let value = emit_codegen_expr_with_local_env(
            fb,
            &op.value,
            local_env,
            emit_ctx,
            value_borrowed,
            codegen_env,
            func_imports,
        );
        let call_inst = fb.ins().call(emit_ctx.store_cell_ref, &[raw_cell, value]);
        emit_ctx.emit_decref_for_family(fb, raw_cell, None, RefcountFamily::OwnedTemporary);
        if !value_borrowed {
            emit_ctx.emit_decref_for_family(fb, value, None, RefcountFamily::OwnedTemporary);
        }
        let call_value = fb.inst_results(call_inst)[0];
        let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
            fb,
            local_env,
            ctx: emit_ctx,
            codegen_env,
            func_imports,
            owned_transfer_temp_load: None,
        };
        return intrinsics::OperationEmitState::<InstrBlockPy>::finish_owned_result(
            &mut intrinsic_state,
            call_value,
        );
    }
    if let InstrBlockPy::Del(op) = expr {
        if let Some(result) = emit_preserved_delete_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
        ) {
            let (value, ownership, _) = result.expect_pyobject("legacy preserved delete");
            assert!(
                ownership.is_owned(),
                "legacy preserved delete should produce an owned PyObject"
            );
            return value;
        }
        if let Some(value) = emit_local_delete_with_local_env(fb, op, local_env, emit_ctx) {
            return value;
        }
        let Some(location) = op.name.cell_location() else {
            panic!("Del should be resolved before codegen: {op:?}");
        };
        let raw_cell = emit_raw_cell_object_for_location_with_local_env(
            fb, location, "Del", local_env, emit_ctx,
        );
        let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
            fb,
            local_env,
            ctx: emit_ctx,
            codegen_env,
            func_imports,
            owned_transfer_temp_load: None,
        };
        return intrinsics::emit_del_deref_raw_cell::<InstrBlockPy>(
            raw_cell,
            op.quietly,
            &mut intrinsic_state,
        );
    }
    if let InstrBlockPy::Call(call) = expr {
        assert!(
            !borrowed,
            "codegen call expression must not use borrowed result"
        );
        if let Some(result) = emit_runtime_builtin_primitive_call_result_with_local_env(
            fb,
            call,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        ) {
            let (value, ownership, _) = result.expect_pyobject("runtime builtin expression result");
            assert!(
                ownership.is_owned(),
                "runtime builtin expression result should be an owned PyObject"
            );
            return value;
        }
        if let Some(value) = emit_codegen_simple_call_with_local_env(
            fb,
            call,
            local_env,
            emit_ctx,
            None,
            None,
            codegen_env,
            func_imports,
        ) {
            return value;
        }
    }
    panic!("operation {expr:?} should have been handled by LocalEnv direct emitter")
}

fn discard_emit_result(
    fb: &mut FunctionBuilder<'_>,
    result: EmitResult,
    emit_ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    if let Some(value) = result.value() {
        emit_discard_soac_value(fb, value, emit_ctx);
    }
    Ok(())
}

fn emit_discard_soac_value(
    fb: &mut FunctionBuilder<'_>,
    value: SoacValue,
    emit_ctx: &JitEmitCtx<'_>,
) {
    if let SoacValue::PyObject {
        value,
        ownership,
        facts,
    } = value
        && ownership.is_owned()
        && !facts.is_immortal()
    {
        emit_ctx.emit_decref_for_family(fb, value, Some(facts), RefcountFamily::OwnedTemporary);
    }
}

fn emit_soac_value_as_pyobject_for_demand(
    fb: &mut FunctionBuilder<'_>,
    value: SoacValue,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let ResultDemand::PyObject { borrowed_ok } = demand else {
        panic!("PyObject materialization requested for non-PyObject demand {demand:?}");
    };
    let pyobject = match value {
        SoacValue::PyObject {
            value,
            ownership,
            facts,
        } => {
            let (value, ownership) = if borrowed_ok {
                (value, ownership)
            } else {
                emit_promote_pyobject_to_owned_boundary(fb, value, ownership, facts, emit_ctx)
            };
            SoacValue::pyobject_with_ownership(value, ownership, facts)
        }
        SoacValue::I32 { value, facts } if facts.is_i32_bool01() => {
            emit_to_python_bool(fb, SoacValue::i32(value, facts), emit_ctx)
        }
        value @ (SoacValue::I32 { .. } | SoacValue::I64 { .. }) => {
            emit_to_python_long(fb, value, emit_ctx.py_long_from_i64_ref, emit_ctx)
        }
    };
    let (value, ownership, facts) = pyobject.expect_pyobject("PyObject demand");
    if !ownership.can_satisfy_pyobject_demand(demand) {
        panic!("PyObject demand {demand:?} produced incompatible ownership {ownership:?}");
    }
    EmitResult::pyobject(value, ownership, facts)
}

fn emit_soac_value_as_i32_bool01(
    fb: &mut FunctionBuilder<'_>,
    value: SoacValue,
    is_true_ref: Option<ir::FuncRef>,
    emit_ctx: &JitEmitCtx<'_>,
) -> EmitResult {
    let truth = match value {
        SoacValue::I32 { value, facts } if facts.is_i32_bool01() => SoacValue::i32(value, facts),
        SoacValue::I32 { value, .. } => emit_i32_bool01_from_i32_result(fb, value, emit_ctx),
        SoacValue::I64 { value, .. } => {
            let is_true = fb.ins().icmp_imm(ir::condcodes::IntCC::NotEqual, value, 0);
            emit_i32_bool01_from_cond(fb, is_true, emit_ctx)
        }
        SoacValue::PyObject {
            value,
            ownership,
            facts,
        } => {
            let is_true_ref = is_true_ref
                .expect("PyObject truthiness demand requires an imported is-true helper");
            emit_truthy_from_pyobject_value(
                fb,
                value,
                facts,
                is_true_ref,
                emit_ctx,
                ownership.is_owned(),
            )
        }
    };
    let (value, facts) = truth.expect_i32("I32Bool01 demand");
    EmitResult::i32(value, facts)
}

fn emit_soac_value_result_for_demand(
    fb: &mut FunctionBuilder<'_>,
    value: SoacValue,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    is_true_ref: Option<ir::FuncRef>,
) -> EmitResult {
    match demand {
        ResultDemand::EffectOnly => {
            emit_discard_soac_value(fb, value, emit_ctx);
            EmitResult::no_value()
        }
        ResultDemand::PyObject { .. } => {
            emit_soac_value_as_pyobject_for_demand(fb, value, emit_ctx, demand)
        }
        ResultDemand::I32Bool01 => emit_soac_value_as_i32_bool01(fb, value, is_true_ref, emit_ctx),
        ResultDemand::I64 | ResultDemand::I64Index => {
            let (value, facts) = value.expect_i64("I64 demand");
            EmitResult::i64(value, facts)
        }
    }
}

fn emit_owned_pyobject_result_for_demand(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    facts: PyObjFacts,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    emit_owned_pyobject_result_for_demand_with_truthiness(fb, value, facts, emit_ctx, demand, None)
}

fn emit_owned_pyobject_result_for_demand_with_truthiness(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    facts: PyObjFacts,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    is_true_ref: Option<ir::FuncRef>,
) -> EmitResult {
    let ownership = if facts.is_immortal() {
        ValueOwnership::Immortal
    } else {
        ValueOwnership::Owned
    };
    emit_soac_value_result_for_demand(
        fb,
        SoacValue::pyobject_with_ownership(value, ownership, facts),
        emit_ctx,
        demand,
        is_true_ref,
    )
}

fn emit_owned_pyobject_result_for_demand_with_codegen_imports(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    facts: PyObjFacts,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let is_true_ref = if demand == ResultDemand::I32Bool01 {
        Some(func_imports.get(codegen_env, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?)
    } else {
        None
    };
    Ok(emit_owned_pyobject_result_for_demand_with_truthiness(
        fb,
        value,
        facts,
        emit_ctx,
        demand,
        is_true_ref,
    ))
}

fn emit_promote_pyobject_to_owned_boundary(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    ownership: ValueOwnership,
    facts: PyObjFacts,
    emit_ctx: &JitEmitCtx<'_>,
) -> (ir::Value, ValueOwnership) {
    if matches!(ownership, ValueOwnership::Borrowed) {
        emit_ctx.emit_incref_for_family(
            fb,
            value,
            Some(facts),
            RefcountFamily::BorrowedResultClone,
        );
        return (value, ValueOwnership::Owned);
    }
    (value, ownership)
}

fn direct_positional_call_args(
    call: &soac_core::block_py::Call<InstrBlockPy>,
    param_count: usize,
) -> Option<Vec<&InstrBlockPy>> {
    if !call.keywords.is_empty() || call.args.len() != param_count {
        return None;
    }
    call.args
        .iter()
        .map(|arg| match arg {
            CallArgPositional::Positional(value) => Some(value),
            CallArgPositional::Starred(_) => None,
        })
        .collect()
}

fn direct_simple_positional_call_args(
    call: &soac_core::block_py::Call<InstrBlockPy>,
) -> Option<Vec<&InstrBlockPy>> {
    if !call.keywords.is_empty() {
        return None;
    }
    call.args
        .iter()
        .map(|arg| match arg {
            CallArgPositional::Positional(value) => Some(value),
            CallArgPositional::Starred(_) => None,
        })
        .collect()
}

fn typed_direct_positional_call_args(
    call: &TypedCall<InstrTyped>,
    param_count: usize,
) -> Option<Vec<&InstrTyped>> {
    if !call.keywords.is_empty() || call.args.len() != param_count {
        return None;
    }
    call.args
        .iter()
        .map(|arg| match arg {
            CallArgPositional::Positional(value) => Some(value),
            CallArgPositional::Starred(_) => None,
        })
        .collect()
}

fn typed_simple_positional_call_args(call: &TypedCall<InstrTyped>) -> Option<Vec<&InstrTyped>> {
    if !call.keywords.is_empty() {
        return None;
    }
    call.args
        .iter()
        .map(|arg| match arg {
            CallArgPositional::Positional(value) => Some(value),
            CallArgPositional::Starred(_) => None,
        })
        .collect()
}

#[cfg(test)]
fn static_runtime_primitive_for_call(
    call: &soac_core::block_py::Call<InstrBlockPy>,
    module_constants: &ModuleCodegenConstants,
) -> Option<direct_abi::RuntimePrimitiveId> {
    let desc = static_runtime_primitive_desc_for_call(call, module_constants)?;
    let DirectTargetId::RuntimePrimitive(primitive) = desc.target else {
        return None;
    };
    Some(primitive)
}

fn static_runtime_primitive_desc_for_call(
    call: &soac_core::block_py::Call<InstrBlockPy>,
    module_constants: &ModuleCodegenConstants,
) -> Option<&'static DirectCallableDesc> {
    let name = codegen_expr_static_runtime_name(call.func.as_ref(), module_constants)?;
    let args = direct_simple_positional_call_args(call)?;
    let primitive = direct_abi::runtime_primitive_for_builtin_name_and_arity(name, args.len())?;
    let desc = direct_abi::runtime_primitive_desc(primitive);
    debug_assert_eq!(args.len(), desc.abi.params.len());
    Some(desc)
}

fn static_runtime_primitive_desc_for_typed_call(
    call: &TypedCall<InstrTyped>,
    module_constants: &ModuleCodegenConstants,
) -> Option<&'static DirectCallableDesc> {
    let name = typed_expr_static_runtime_name(call.func.as_ref(), module_constants)?;
    let args = typed_simple_positional_call_args(call)?;
    let primitive = direct_abi::runtime_primitive_for_builtin_name_and_arity(name, args.len())?;
    let desc = direct_abi::runtime_primitive_desc(primitive);
    debug_assert_eq!(args.len(), desc.abi.params.len());
    Some(desc)
}

fn runtime_primitive_import_spec(desc: &DirectCallableDesc) -> &'static ImportSpec {
    match desc.entry {
        DirectEntry::RuntimeSymbol(direct_abi::SOAC_RUNTIME_BUILTIN_ORD_I64_SYMBOL) => {
            &SOAC_RUNTIME_BUILTIN_ORD_I64_IMPORT
        }
        DirectEntry::RuntimeSymbol(direct_abi::SOAC_RUNTIME_BUILTIN_CHR_I64_SYMBOL) => {
            &SOAC_RUNTIME_BUILTIN_CHR_I64_IMPORT
        }
        DirectEntry::RuntimeSymbol(direct_abi::SOAC_RUNTIME_BUILTIN_LEN_I64_SYMBOL) => {
            &SOAC_RUNTIME_BUILTIN_LEN_I64_IMPORT
        }
        DirectEntry::RuntimeSymbol(direct_abi::SOAC_RUNTIME_BUILTIN_ITER_OBJECT_SYMBOL) => {
            &SOAC_RUNTIME_BUILTIN_ITER_OBJECT_IMPORT
        }
        DirectEntry::RuntimeSymbol(direct_abi::SOAC_RUNTIME_UNPACK_FIXED_SYMBOL) => {
            &SOAC_RUNTIME_UNPACK_FIXED_IMPORT
        }
        DirectEntry::RuntimeSymbol(direct_abi::SOAC_JIT_RESUME_GENERATOR_SYMBOL) => {
            &SOAC_JIT_RESUME_GENERATOR_IMPORT
        }
        DirectEntry::RuntimeSymbol(symbol) => {
            panic!("missing ImportSpec for runtime primitive symbol {symbol}")
        }
        DirectEntry::ProcessJitPythonFunction => {
            panic!("runtime primitive descriptor unexpectedly used process-JIT entry")
        }
    }
}

fn runtime_primitive_i64_result_facts(desc: &DirectCallableDesc) -> IntFacts {
    match desc.target {
        DirectTargetId::RuntimePrimitive(direct_abi::RuntimePrimitiveId::BuiltinOrdI64) => {
            IntFacts::i64_range(IntRange {
                min: 0,
                max: 0x10ffff,
            })
        }
        DirectTargetId::RuntimePrimitive(direct_abi::RuntimePrimitiveId::BuiltinLenI64) => {
            IntFacts::i64_range(IntRange {
                min: 0,
                max: i64::MAX as i128,
            })
        }
        DirectTargetId::RuntimePrimitive(_) | DirectTargetId::PythonFunction(_) => {
            IntFacts::i64_unknown()
        }
    }
}

fn i64_binop_result_facts(
    kind: blockpy_intrinsics::BinOpKind,
    lhs_facts: IntFacts,
    rhs_facts: IntFacts,
) -> Option<IntFacts> {
    if lhs_facts.width != IntWidth::I64 || rhs_facts.width != IntWidth::I64 {
        return None;
    }
    if !matches!(
        kind,
        blockpy_intrinsics::BinOpKind::Add
            | blockpy_intrinsics::BinOpKind::Sub
            | blockpy_intrinsics::BinOpKind::Mul
    ) {
        return None;
    }
    let result_range = match (lhs_facts.range, rhs_facts.range) {
        (Some(lhs_range), Some(rhs_range)) => match kind {
            blockpy_intrinsics::BinOpKind::Add => lhs_range.checked_add(rhs_range),
            blockpy_intrinsics::BinOpKind::Sub => lhs_range.checked_sub(rhs_range),
            blockpy_intrinsics::BinOpKind::Mul => lhs_range.checked_mul(rhs_range),
            _ => unreachable!("I64 BinOp kind checked above"),
        }
        .filter(|range| range.is_within(IntRange::I64)),
        _ => None,
    };
    let known_value = match (lhs_facts.known_value, rhs_facts.known_value) {
        (Some(lhs), Some(rhs)) => match kind {
            blockpy_intrinsics::BinOpKind::Add => lhs
                .checked_add(rhs)
                .filter(|value| IntRange::exact(*value).is_within(IntRange::I64)),
            blockpy_intrinsics::BinOpKind::Sub => lhs
                .checked_sub(rhs)
                .filter(|value| IntRange::exact(*value).is_within(IntRange::I64)),
            blockpy_intrinsics::BinOpKind::Mul => lhs
                .checked_mul(rhs)
                .filter(|value| IntRange::exact(*value).is_within(IntRange::I64)),
            _ => None,
        },
        _ => None,
    };
    let result_range = result_range.or_else(|| known_value.map(IntRange::exact));
    Some(IntFacts {
        width: IntWidth::I64,
        known_value,
        range: result_range,
    })
}

fn codegen_expr_i64_demand_facts(
    expr: &InstrBlockPy,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> Option<IntFacts> {
    if let Some(value) = codegen_expr_const_i64(expr, emit_ctx.module_constants) {
        return Some(IntFacts::i64_known(value));
    }
    match expr {
        InstrBlockPy::Call(call) => {
            let Some(desc) =
                static_runtime_primitive_desc_for_call(call, emit_ctx.module_constants)
            else {
                return None;
            };
            if !matches!(desc.abi.result, ResultAbi::I64)
                || !runtime_primitive_call_params_can_satisfy_abi(call, desc, local_env, emit_ctx)
            {
                return None;
            }
            Some(runtime_primitive_i64_result_facts(desc))
        }
        InstrBlockPy::BinOp(op) => {
            let lhs_facts = codegen_expr_i64_demand_facts(op.left.as_ref(), local_env, emit_ctx)?;
            let rhs_facts = codegen_expr_i64_demand_facts(op.right.as_ref(), local_env, emit_ctx)?;
            i64_binop_result_facts(op.kind, lhs_facts, rhs_facts)
        }
        _ => None,
    }
}

fn typed_expr_i64_demand_facts(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> Option<IntFacts> {
    if let Some(value) = typed_expr_const_i64(expr, emit_ctx.module_constants) {
        return Some(IntFacts::i64_known(value));
    }
    if let InstrTyped::Load(op) = expr
        && let Some(facts) = local_env.i64_facts_for_load(&op.name)
    {
        return Some(facts);
    }
    if let Some(plan) = expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
        && planning::exact_int_return_plan_i64_result(plan).is_some()
        && opt_v3_exact_int_deopt_miss_target_available(plan.instr_id, emit_ctx)
    {
        return Some(IntFacts::i64_unknown());
    }
    match expr {
        InstrTyped::CallTyped(call) => {
            let Some(desc) =
                static_runtime_primitive_desc_for_typed_call(call, emit_ctx.module_constants)
            else {
                return None;
            };
            if !matches!(desc.abi.result, ResultAbi::I64)
                || !runtime_primitive_typed_call_params_can_satisfy_abi(
                    call, desc, local_env, emit_ctx,
                )
            {
                return None;
            }
            Some(runtime_primitive_i64_result_facts(desc))
        }
        InstrTyped::BinOp(op) => {
            let lhs_facts = typed_expr_i64_demand_facts(op.left.as_ref(), local_env, emit_ctx)?;
            let rhs_facts = typed_expr_i64_demand_facts(op.right.as_ref(), local_env, emit_ctx)?;
            i64_binop_result_facts(op.kind, lhs_facts, rhs_facts)
        }
        _ => match expr.result_facts() {
            Some(ValueFacts::I64(_)) => Some(IntFacts::i64_unknown()),
            _ => None,
        },
    }
}

fn typed_expr_i32_bool01_demand_facts(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    _emit_ctx: &JitEmitCtx<'_>,
) -> Option<IntFacts> {
    if let InstrTyped::Load(op) = expr
        && let Some(facts) = local_env
            .scalar_i32_bool01_value_for_load(&op.name)
            .map(|(_, facts)| facts)
    {
        return Some(facts);
    }
    if let Some(plan) = expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
        && planning::exact_int_return_plan_i32_bool01_result(plan).is_some()
    {
        return Some(IntFacts::i32_bool01());
    }
    match expr {
        InstrTyped::Truthy(_) | InstrTyped::DirectCallGuardTest(_) => Some(IntFacts::i32_bool01()),
        _ => match expr.result_facts() {
            Some(ValueFacts::Bool(_)) => Some(IntFacts::i32_bool01()),
            _ => None,
        },
    }
}

fn codegen_expr_can_satisfy_i64_demand(
    expr: &InstrBlockPy,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    codegen_expr_i64_demand_facts(expr, local_env, emit_ctx).is_some()
}

fn typed_expr_can_satisfy_i64_demand(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    typed_expr_i64_demand_facts(expr, local_env, emit_ctx).is_some()
}

fn typed_expr_can_emit_guarded_i64_index(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    if typed_expr_const_i64(expr, emit_ctx.module_constants).is_some() {
        return true;
    }
    match expr {
        InstrTyped::Load(op) => {
            if local_env.scalar_i64_value_for_load(&op.name).is_some() {
                return true;
            }
            (op.name.location.as_local().is_some() || op.name.location.as_constant().is_some())
                && typed_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, emit_ctx)
        }
        InstrTyped::BinOp(op)
            if matches!(
                op.kind,
                blockpy_intrinsics::BinOpKind::Add
                    | blockpy_intrinsics::BinOpKind::Sub
                    | blockpy_intrinsics::BinOpKind::Mul
            ) =>
        {
            typed_expr_can_emit_guarded_i64_index(op.left.as_ref(), local_env, emit_ctx)
                && typed_expr_can_emit_guarded_i64_index(op.right.as_ref(), local_env, emit_ctx)
        }
        _ => false,
    }
}

fn codegen_expr_has_exact_int_pyobject_facts(
    expr: &InstrBlockPy,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    if !matches!(emit_ctx.function_kind, FunctionKind::Function) {
        return false;
    }
    if let InstrBlockPy::Load(op) = expr {
        if local_env
            .py_facts_for_load(&op.name)
            .is_some_and(|py_facts| py_facts.is_exact_type(PyExactType::Int))
        {
            return true;
        }
        if op.name.location.as_constant().is_some_and(|index| {
            emit_ctx
                .module_constants
                .constant_is_int(ModuleConstantId(index as usize))
        }) {
            return true;
        }
    }
    emit_ctx
        .value_facts_for_expr(expr)
        .and_then(ValueFacts::as_pyobj)
        .is_some_and(|py_facts| py_facts.is_exact_type(PyExactType::Int))
}

fn typed_expr_has_exact_int_pyobject_facts(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    if !matches!(emit_ctx.function_kind, FunctionKind::Function) {
        return false;
    }
    if let InstrTyped::Load(op) = expr {
        if local_env
            .py_facts_for_load(&op.name)
            .is_some_and(|py_facts| py_facts.is_exact_type(PyExactType::Int))
        {
            return true;
        }
        if op.name.location.as_constant().is_some_and(|index| {
            emit_ctx
                .module_constants
                .constant_is_int(ModuleConstantId(index as usize))
        }) {
            return true;
        }
    }
    expr.result_facts()
        .and_then(ValueFacts::as_pyobj)
        .is_some_and(|py_facts| py_facts.is_exact_type(PyExactType::Int))
}

fn codegen_expr_can_satisfy_param_abi(
    expr: &InstrBlockPy,
    param: ParamAbi,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    match param {
        ParamAbi::PyObject { .. } => true,
        ParamAbi::I64 { py_long_coercion } => {
            codegen_expr_can_satisfy_i64_demand(expr, local_env, emit_ctx)
                || (py_long_coercion.is_some()
                    && codegen_expr_has_exact_int_pyobject_facts(expr, local_env, emit_ctx))
        }
        ParamAbi::I32 => false,
    }
}

fn typed_expr_can_satisfy_param_abi(
    expr: &InstrTyped,
    param: ParamAbi,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    match param {
        ParamAbi::PyObject { .. } => true,
        ParamAbi::I64 { py_long_coercion } => {
            typed_expr_can_satisfy_i64_demand(expr, local_env, emit_ctx)
                || (py_long_coercion.is_some()
                    && typed_expr_has_exact_int_pyobject_facts(expr, local_env, emit_ctx))
        }
        ParamAbi::I32 => false,
    }
}

fn runtime_primitive_call_params_can_satisfy_abi(
    call: &soac_core::block_py::Call<InstrBlockPy>,
    desc: &DirectCallableDesc,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    let DirectTargetId::RuntimePrimitive(_) = desc.target else {
        return false;
    };
    let Some(args) = direct_positional_call_args(call, desc.abi.params.len()) else {
        return false;
    };
    args.into_iter()
        .zip(desc.abi.params.iter().copied())
        .all(|(arg, param)| codegen_expr_can_satisfy_param_abi(arg, param, local_env, emit_ctx))
}

fn runtime_primitive_typed_call_params_can_satisfy_abi(
    call: &TypedCall<InstrTyped>,
    desc: &DirectCallableDesc,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    let DirectTargetId::RuntimePrimitive(_) = desc.target else {
        return false;
    };
    let Some(args) = typed_direct_positional_call_args(call, desc.abi.params.len()) else {
        return false;
    };
    args.into_iter()
        .zip(desc.abi.params.iter().copied())
        .all(|(arg, param)| typed_expr_can_satisfy_param_abi(arg, param, local_env, emit_ctx))
}

#[cfg(test)]
fn codegen_expr_static_can_satisfy_i64_demand(
    expr: &InstrBlockPy,
    module_constants: &ModuleCodegenConstants,
) -> bool {
    codegen_expr_static_i64_demand_facts(expr, module_constants).is_some()
}

#[cfg(test)]
fn codegen_expr_static_i64_demand_facts(
    expr: &InstrBlockPy,
    module_constants: &ModuleCodegenConstants,
) -> Option<IntFacts> {
    if let Some(value) = codegen_expr_const_i64(expr, module_constants) {
        return Some(IntFacts::i64_known(value));
    }
    match expr {
        InstrBlockPy::Call(call) => {
            let Some(desc) = static_runtime_primitive_desc_for_call(call, module_constants) else {
                return None;
            };
            if !matches!(desc.abi.result, ResultAbi::I64)
                || !runtime_primitive_call_static_params_can_satisfy_abi(
                    call,
                    desc,
                    module_constants,
                )
            {
                return None;
            }
            Some(runtime_primitive_i64_result_facts(desc))
        }
        InstrBlockPy::BinOp(op) => {
            let lhs_facts =
                codegen_expr_static_i64_demand_facts(op.left.as_ref(), module_constants)?;
            let rhs_facts =
                codegen_expr_static_i64_demand_facts(op.right.as_ref(), module_constants)?;
            i64_binop_result_facts(op.kind, lhs_facts, rhs_facts)
        }
        _ => None,
    }
}

#[cfg(test)]
fn runtime_primitive_call_static_params_can_satisfy_abi(
    call: &soac_core::block_py::Call<InstrBlockPy>,
    desc: &DirectCallableDesc,
    module_constants: &ModuleCodegenConstants,
) -> bool {
    let Some(args) = direct_positional_call_args(call, desc.abi.params.len()) else {
        return false;
    };
    args.into_iter()
        .zip(desc.abi.params.iter().copied())
        .all(|(arg, param)| match param {
            ParamAbi::PyObject { .. } => true,
            ParamAbi::I64 { .. } => {
                codegen_expr_static_can_satisfy_i64_demand(arg, module_constants)
            }
            ParamAbi::I32 => false,
        })
}

fn emit_scalar_result_after_current_exception_check_with_cleanup(
    fb: &mut FunctionBuilder<'_>,
    result: ir::Value,
    result_ty: ir::Type,
    owned_inputs: &[ir::Value],
    emit_ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let null_ptr = fb.ins().iconst(emit_ctx.consts.ptr_ty, 0);
    let raised_exc = emit_current_raised_exception(
        fb,
        emit_ctx.consts.ptr_ty,
        emit_ctx.consts.thread_state_value,
    );
    let has_error = fb
        .ins()
        .icmp(ir::condcodes::IntCC::NotEqual, raised_exc, null_ptr);
    let error_block = fb.create_block();
    let ok_block = fb.create_block();
    fb.append_block_param(ok_block, result_ty);
    fb.ins().brif(
        has_error,
        error_block,
        &[],
        ok_block,
        &[ir::BlockArg::Value(result)],
    );

    fb.switch_to_block(error_block);
    emit_release_owned_inputs(fb, emit_ctx, owned_inputs);
    fb.ins().jump(
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
    );

    fb.switch_to_block(ok_block);
    emit_release_owned_inputs(fb, emit_ctx, owned_inputs);
    fb.block_params(ok_block)[0]
}

fn emit_i64_result_for_demand(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    facts: IntFacts,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    emit_soac_value_result_for_demand(fb, SoacValue::i64(value, facts), emit_ctx, demand, None)
}

fn emit_checked_i64_overflow_result(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    overflow: ir::Value,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let overflow_block = fb.create_block();
    let value_ok_block = fb.create_block();
    fb.append_block_param(value_ok_block, emit_ctx.consts.i64_ty);
    fb.ins().brif(
        overflow,
        overflow_block,
        &[],
        value_ok_block,
        &[ir::BlockArg::Value(value)],
    );

    fb.switch_to_block(overflow_block);
    let raise_overflow_ref =
        func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_RAISE_I64_OVERFLOW_IMPORT);
    fb.ins().call(raise_overflow_ref, &[]);
    fb.ins().jump(
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
    );

    fb.switch_to_block(value_ok_block);
    fb.block_params(value_ok_block)[0]
}

fn emit_i64_binop_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &blockpy_intrinsics::BinOp<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    if !matches!(demand, ResultDemand::I64 | ResultDemand::I64Index) {
        return None;
    }
    let lhs_facts = codegen_expr_i64_demand_facts(op.left.as_ref(), local_env, emit_ctx)?;
    let rhs_facts = codegen_expr_i64_demand_facts(op.right.as_ref(), local_env, emit_ctx)?;
    let result_facts = i64_binop_result_facts(op.kind, lhs_facts, rhs_facts)?;
    let lhs = emit_codegen_stmt_result_with_local_env(
        fb,
        op.left.as_ref(),
        local_env,
        emit_ctx,
        ResultDemand::I64_VALUE,
        codegen_env,
        func_imports,
    )
    .expect("I64-capable BinOp left operand should emit");
    let (lhs, _) = lhs.expect_i64("I64 BinOp left operand");
    let rhs = emit_codegen_stmt_result_with_local_env(
        fb,
        op.right.as_ref(),
        local_env,
        emit_ctx,
        ResultDemand::I64_VALUE,
        codegen_env,
        func_imports,
    )
    .expect("I64-capable BinOp right operand should emit");
    let (rhs, _) = rhs.expect_i64("I64 BinOp right operand");
    let (raw_value, overflow) = match op.kind {
        blockpy_intrinsics::BinOpKind::Add => fb.ins().sadd_overflow(lhs, rhs),
        blockpy_intrinsics::BinOpKind::Sub => fb.ins().ssub_overflow(lhs, rhs),
        blockpy_intrinsics::BinOpKind::Mul => fb.ins().smul_overflow(lhs, rhs),
        _ => unreachable!("unsupported I64 BinOp should not pass demand analysis"),
    };
    let value = emit_checked_i64_overflow_result(
        fb,
        raw_value,
        overflow,
        emit_ctx,
        codegen_env,
        func_imports,
    );
    Some(emit_i64_result_for_demand(
        fb,
        value,
        result_facts,
        emit_ctx,
        demand,
    ))
}

fn emit_typed_i64_binop_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &blockpy_intrinsics::BinOp<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    if !matches!(demand, ResultDemand::I64 | ResultDemand::I64Index) {
        return Ok(None);
    }
    let lhs_facts = typed_expr_i64_demand_facts(op.left.as_ref(), local_env, emit_ctx);
    let Some(lhs_facts) = lhs_facts else {
        return Ok(None);
    };
    let Some(rhs_facts) = typed_expr_i64_demand_facts(op.right.as_ref(), local_env, emit_ctx)
    else {
        return Ok(None);
    };
    let Some(result_facts) = i64_binop_result_facts(op.kind, lhs_facts, rhs_facts) else {
        return Ok(None);
    };
    let lhs = emit_typed_codegen_stmt_result_with_local_env(
        fb,
        op.left.as_ref(),
        local_env,
        emit_ctx,
        ResultDemand::I64_VALUE,
        codegen_env,
        func_imports,
    )?;
    let (lhs, _) = lhs.expect_i64("typed I64 BinOp left operand");
    let rhs = emit_typed_codegen_stmt_result_with_local_env(
        fb,
        op.right.as_ref(),
        local_env,
        emit_ctx,
        ResultDemand::I64_VALUE,
        codegen_env,
        func_imports,
    )?;
    let (rhs, _) = rhs.expect_i64("typed I64 BinOp right operand");
    let (raw_value, overflow) = match op.kind {
        blockpy_intrinsics::BinOpKind::Add => fb.ins().sadd_overflow(lhs, rhs),
        blockpy_intrinsics::BinOpKind::Sub => fb.ins().ssub_overflow(lhs, rhs),
        blockpy_intrinsics::BinOpKind::Mul => fb.ins().smul_overflow(lhs, rhs),
        _ => unreachable!("unsupported typed I64 BinOp should not pass demand analysis"),
    };
    let value = emit_checked_i64_overflow_result(
        fb,
        raw_value,
        overflow,
        emit_ctx,
        codegen_env,
        func_imports,
    );
    Ok(Some(emit_i64_result_for_demand(
        fb,
        value,
        result_facts,
        emit_ctx,
        demand,
    )))
}

fn emit_overflow_guarded_i64_binop(
    fb: &mut FunctionBuilder<'_>,
    kind: blockpy_intrinsics::BinOpKind,
    lhs: ir::Value,
    rhs: ir::Value,
    emit_ctx: &JitEmitCtx<'_>,
    guard_miss_block: ir::Block,
) -> Option<ir::Value> {
    let (raw_value, overflow) = match kind {
        blockpy_intrinsics::BinOpKind::Add => fb.ins().sadd_overflow(lhs, rhs),
        blockpy_intrinsics::BinOpKind::Sub => fb.ins().ssub_overflow(lhs, rhs),
        blockpy_intrinsics::BinOpKind::Mul => fb.ins().smul_overflow(lhs, rhs),
        _ => return None,
    };
    let value_ok_block = fb.create_block();
    fb.append_block_param(value_ok_block, emit_ctx.consts.i64_ty);
    fb.ins().brif(
        overflow,
        guard_miss_block,
        &[],
        value_ok_block,
        &[ir::BlockArg::Value(raw_value)],
    );

    fb.switch_to_block(value_ok_block);
    Some(fb.block_params(value_ok_block)[0])
}

fn emit_typed_guarded_i64_index_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    guard_miss_block: ir::Block,
) -> Result<Option<ir::Value>, String> {
    if let Some(const_value) = typed_expr_const_i64(expr, emit_ctx.module_constants) {
        return Ok(Some(fb.ins().iconst(emit_ctx.consts.i64_ty, const_value)));
    }

    match expr {
        InstrTyped::Load(op)
            if op.name.location.as_local().is_some()
                || op.name.location.as_constant().is_some() =>
        {
            if let Some((value, _)) = local_env.scalar_i64_value_for_load(&op.name) {
                return Ok(Some(value));
            }
            if !typed_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, emit_ctx) {
                return Ok(None);
            }
            let value = emit_typed_codegen_expr_value_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                true,
                codegen_env,
                func_imports,
            )?;
            let (value, ownership, _) = value.expect_pyobject("typed guarded item index load");
            if ownership.is_owned() {
                return Ok(None);
            }
            let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
                fb,
                local_env,
                ctx: emit_ctx,
                codegen_env,
                func_imports,
                owned_transfer_temp_load: None,
            };
            Ok(Some(intrinsics::emit_v3_guarded_compact_long_i64(
                &mut intrinsic_state,
                value,
                guard_miss_block,
            )))
        }
        InstrTyped::BinOp(op)
            if matches!(
                op.kind,
                blockpy_intrinsics::BinOpKind::Add
                    | blockpy_intrinsics::BinOpKind::Sub
                    | blockpy_intrinsics::BinOpKind::Mul
            ) =>
        {
            if !typed_expr_can_emit_guarded_i64_index(op.left.as_ref(), local_env, emit_ctx)
                || !typed_expr_can_emit_guarded_i64_index(op.right.as_ref(), local_env, emit_ctx)
            {
                return Ok(None);
            }
            let lhs = emit_typed_guarded_i64_index_with_local_env(
                fb,
                op.left.as_ref(),
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
                guard_miss_block,
            )?;
            let Some(lhs) = lhs else {
                return Ok(None);
            };
            let rhs = emit_typed_guarded_i64_index_with_local_env(
                fb,
                op.right.as_ref(),
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
                guard_miss_block,
            )?;
            let Some(rhs) = rhs else {
                return Ok(None);
            };
            Ok(emit_overflow_guarded_i64_binop(
                fb,
                op.kind,
                lhs,
                rhs,
                emit_ctx,
                guard_miss_block,
            ))
        }
        _ => Ok(None),
    }
}

fn emit_exact_pylong_as_i64_saturating_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrBlockPy,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> EmitResult {
    let value_is_borrowed =
        codegen_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, emit_ctx);
    let value = emit_codegen_expr_with_local_env(
        fb,
        expr,
        local_env,
        emit_ctx,
        value_is_borrowed,
        codegen_env,
        func_imports,
    );
    let pylong_as_i64_saturating_ref = func_imports.get_or_panic(
        codegen_env,
        &mut fb.func,
        &SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_IMPORT,
    );
    let as_i64_inst = fb.ins().call(
        pylong_as_i64_saturating_ref,
        &[emit_ctx.consts.thread_state_value, value],
    );
    let raw_i64 = fb.inst_results(as_i64_inst)[0];
    let owned_inputs = if value_is_borrowed {
        Vec::new()
    } else {
        vec![value]
    };
    let value_i64 = emit_scalar_result_after_current_exception_check_with_cleanup(
        fb,
        raw_i64,
        emit_ctx.consts.i64_ty,
        owned_inputs.as_slice(),
        emit_ctx,
    );
    emit_i64_result_for_demand(fb, value_i64, IntFacts::i64_unknown(), emit_ctx, demand)
}

fn emit_typed_exact_pylong_as_i64_saturating_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        expr,
        local_env,
        emit_ctx,
        typed_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, emit_ctx),
        codegen_env,
        func_imports,
    )?;
    if let Some((value, facts)) = value.as_i64() {
        return Ok(emit_i64_result_for_demand(
            fb, value, facts, emit_ctx, demand,
        ));
    }
    let (value, ownership, _) = value.expect_pyobject("typed runtime primitive exact-int param");
    let pylong_as_i64_saturating_ref = func_imports.get_or_panic(
        codegen_env,
        &mut fb.func,
        &SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_IMPORT,
    );
    let as_i64_inst = fb.ins().call(
        pylong_as_i64_saturating_ref,
        &[emit_ctx.consts.thread_state_value, value],
    );
    let raw_i64 = fb.inst_results(as_i64_inst)[0];
    let owned_inputs = if ownership.is_owned() {
        vec![value]
    } else {
        Vec::new()
    };
    let value_i64 = emit_scalar_result_after_current_exception_check_with_cleanup(
        fb,
        raw_i64,
        emit_ctx.consts.i64_ty,
        owned_inputs.as_slice(),
        emit_ctx,
    );
    Ok(emit_i64_result_for_demand(
        fb,
        value_i64,
        IntFacts::i64_unknown(),
        emit_ctx,
        demand,
    ))
}

fn emit_runtime_primitive_hidden_args(
    desc: &DirectCallableDesc,
    emit_ctx: &JitEmitCtx<'_>,
) -> Vec<ir::Value> {
    let mut args = Vec::with_capacity(desc.abi.hidden_args.len());
    for hidden_arg in desc.abi.hidden_args {
        match hidden_arg {
            HiddenArgAbi::ThreadState => args.push(emit_ctx.consts.thread_state_value),
            HiddenArgAbi::FunctionEnv => {
                panic!("runtime primitive descriptor cannot use a function-env hidden argument")
            }
        }
    }
    args
}

fn emit_runtime_primitive_param_value_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrBlockPy,
    param: ParamAbi,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> (ir::Value, Option<ir::Value>) {
    match param {
        ParamAbi::PyObject {
            ownership: ArgOwnership::BorrowedOk,
        } => {
            let expr_is_borrowed =
                codegen_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, emit_ctx);
            let value = emit_codegen_expr_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                expr_is_borrowed,
                codegen_env,
                func_imports,
            );
            let owned_after_call = if expr_is_borrowed { None } else { Some(value) };
            (value, owned_after_call)
        }
        ParamAbi::PyObject { ownership } => {
            panic!("runtime primitive PyObject param ownership {ownership:?} is not implemented")
        }
        ParamAbi::I64 {
            py_long_coercion: Some(PyLongI64Coercion::Saturating),
        } if codegen_expr_const_i64(expr, emit_ctx.module_constants).is_none()
            && !codegen_expr_can_satisfy_i64_demand(expr, local_env, emit_ctx)
            && codegen_expr_has_exact_int_pyobject_facts(expr, local_env, emit_ctx) =>
        {
            let coerced = emit_exact_pylong_as_i64_saturating_result_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                ResultDemand::I64_VALUE,
                codegen_env,
                func_imports,
            );
            let (value, _) = coerced.expect_i64("runtime primitive PyLong-to-I64 param");
            (value, None)
        }
        ParamAbi::I64 { .. } => {
            let arg_result = emit_codegen_stmt_result_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                ResultDemand::I64_VALUE,
                codegen_env,
                func_imports,
            )
            .expect("I64-capable runtime builtin argument should emit");
            let (value, _) = arg_result.expect_i64("runtime primitive I64 param");
            (value, None)
        }
        ParamAbi::I32 => panic!("runtime primitive I32 params are not implemented"),
    }
}

fn emit_runtime_primitive_typed_param_value_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    param: ParamAbi,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(ir::Value, Option<ir::Value>), String> {
    Ok(match param {
        ParamAbi::PyObject {
            ownership: ArgOwnership::BorrowedOk,
        } => {
            let borrowed =
                typed_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, emit_ctx);
            let (value, ownership, _) = emit_typed_pyobject_value_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                borrowed,
                codegen_env,
                func_imports,
                "typed runtime primitive PyObject param",
            )?;
            let owned_after_call = if ownership.is_owned() {
                Some(value)
            } else {
                None
            };
            (value, owned_after_call)
        }
        ParamAbi::PyObject { ownership } => {
            panic!("runtime primitive PyObject param ownership {ownership:?} is not implemented")
        }
        ParamAbi::I64 {
            py_long_coercion: Some(PyLongI64Coercion::Saturating),
        } if typed_expr_const_i64(expr, emit_ctx.module_constants).is_none()
            && !typed_expr_can_satisfy_i64_demand(expr, local_env, emit_ctx)
            && typed_expr_has_exact_int_pyobject_facts(expr, local_env, emit_ctx) =>
        {
            let coerced = emit_typed_exact_pylong_as_i64_saturating_result_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                ResultDemand::I64_VALUE,
                codegen_env,
                func_imports,
            )?;
            let (value, _) = coerced.expect_i64("typed runtime primitive PyLong-to-I64 param");
            (value, None)
        }
        ParamAbi::I64 { .. } => {
            let arg_result = emit_typed_codegen_stmt_result_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                ResultDemand::I64_VALUE,
                codegen_env,
                func_imports,
            )?;
            let (value, _) = arg_result.expect_i64("typed runtime primitive I64 param");
            (value, None)
        }
        ParamAbi::I32 => panic!("runtime primitive I32 params are not implemented"),
    })
}

fn emit_runtime_primitive_result_for_demand(
    fb: &mut FunctionBuilder<'_>,
    desc: &DirectCallableDesc,
    raw_result: Option<ir::Value>,
    owned_inputs: &[ir::Value],
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> EmitResult {
    match desc.abi.result {
        ResultAbi::I64 => {
            let raw_result = raw_result.expect("I64 runtime primitive should return a value");
            let value = match desc.abi.error {
                ErrorAbi::CurrentException => {
                    emit_scalar_result_after_current_exception_check_with_cleanup(
                        fb,
                        raw_result,
                        emit_ctx.consts.i64_ty,
                        owned_inputs,
                        emit_ctx,
                    )
                }
                ErrorAbi::CannotRaise => {
                    emit_release_owned_inputs(fb, emit_ctx, owned_inputs);
                    raw_result
                }
            };
            emit_i64_result_for_demand(
                fb,
                value,
                runtime_primitive_i64_result_facts(desc),
                emit_ctx,
                demand,
            )
        }
        ResultAbi::PyObject {
            ownership: ValueOwnership::Owned,
            exact_type,
        } => {
            let raw_result = raw_result.expect("PyObject runtime primitive should return a value");
            let value = match desc.abi.error {
                ErrorAbi::CurrentException => {
                    let value = emit_decref_owned_inputs_after_nullable_result(
                        fb,
                        emit_ctx,
                        raw_result,
                        owned_inputs,
                    );
                    emit_checked_owned_pyobject_result(fb, value, emit_ctx)
                }
                ErrorAbi::CannotRaise => {
                    emit_release_owned_inputs(fb, emit_ctx, owned_inputs);
                    raw_result
                }
            };
            let facts = exact_type
                .map(PyObjFacts::exact_type)
                .unwrap_or_else(PyObjFacts::unknown);
            emit_owned_pyobject_result_for_demand(fb, value, facts, emit_ctx, demand)
        }
        ResultAbi::PyObject { ownership, .. } => {
            panic!("runtime primitive PyObject result ownership {ownership:?} is not implemented")
        }
        ResultAbi::I32 => panic!("runtime primitive I32 results are not implemented"),
        ResultAbi::NoValue => {
            emit_release_owned_inputs(fb, emit_ctx, owned_inputs);
            EmitResult::no_value()
        }
    }
}

fn emit_runtime_builtin_primitive_desc_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_core::block_py::Call<InstrBlockPy>,
    desc: &DirectCallableDesc,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> EmitResult {
    let args = direct_positional_call_args(call, desc.abi.params.len())
        .expect("runtime primitive call arity should match descriptor");
    let mut call_args = emit_runtime_primitive_hidden_args(desc, emit_ctx);
    let mut owned_inputs = Vec::new();
    for (arg, param) in args.into_iter().zip(desc.abi.params.iter().copied()) {
        let (value, owned_after_call) = emit_runtime_primitive_param_value_with_local_env(
            fb,
            arg,
            param,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        );
        call_args.push(value);
        if let Some(owned_after_call) = owned_after_call {
            owned_inputs.push(owned_after_call);
        }
    }
    emit_runtime_primitive_protocol_target_sample(
        fb,
        desc,
        call_args.as_slice(),
        call.try_semantic_instr_id(),
        emit_ctx,
        codegen_env,
        func_imports,
    );
    let func_ref = func_imports.get_or_panic(
        codegen_env,
        &mut fb.func,
        runtime_primitive_import_spec(desc),
    );
    let call_inst = fb.ins().call(func_ref, call_args.as_slice());
    let raw_result = fb.inst_results(call_inst).first().copied();
    emit_runtime_primitive_result_for_demand(
        fb,
        desc,
        raw_result,
        owned_inputs.as_slice(),
        emit_ctx,
        demand,
    )
}

fn emit_runtime_builtin_primitive_typed_desc_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedCall<InstrTyped>,
    desc: &DirectCallableDesc,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let args = typed_direct_positional_call_args(call, desc.abi.params.len())
        .expect("typed runtime primitive call arity should match descriptor");
    let mut call_args = emit_runtime_primitive_hidden_args(desc, emit_ctx);
    let mut owned_inputs = Vec::new();
    for (arg, param) in args.into_iter().zip(desc.abi.params.iter().copied()) {
        let (value, owned_after_call) = emit_runtime_primitive_typed_param_value_with_local_env(
            fb,
            arg,
            param,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?;
        call_args.push(value);
        if let Some(owned_after_call) = owned_after_call {
            owned_inputs.push(owned_after_call);
        }
    }
    emit_runtime_primitive_protocol_target_sample(
        fb,
        desc,
        call_args.as_slice(),
        call.try_semantic_instr_id(),
        emit_ctx,
        codegen_env,
        func_imports,
    );
    let func_ref = func_imports.get_or_panic(
        codegen_env,
        &mut fb.func,
        runtime_primitive_import_spec(desc),
    );
    let call_inst = fb.ins().call(func_ref, call_args.as_slice());
    let raw_result = fb.inst_results(call_inst).first().copied();
    Ok(emit_runtime_primitive_result_for_demand(
        fb,
        desc,
        raw_result,
        owned_inputs.as_slice(),
        emit_ctx,
        demand,
    ))
}

fn emit_runtime_primitive_protocol_target_sample(
    fb: &mut FunctionBuilder<'_>,
    desc: &DirectCallableDesc,
    call_args: &[ir::Value],
    site_instr_id: Option<InstrId>,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) {
    let DirectTargetId::RuntimePrimitive(direct_abi::RuntimePrimitiveId::BuiltinIterObject) =
        desc.target
    else {
        return;
    };
    let Some(counter_id) = site_instr_id
        .and_then(|site_instr_id| emit_ctx.call_target_counter_ids.get(&site_instr_id))
    else {
        return;
    };
    let Some(receiver) = call_args.get(desc.abi.hidden_args.len()).copied() else {
        return;
    };
    emit_record_protocol_method_target_sample(
        fb,
        receiver,
        *counter_id,
        &DP_JIT_PROTOCOL_ITER_FUNCTION_ID_IMPORT,
        emit_ctx,
        codegen_env,
        func_imports,
    );
}

fn emit_runtime_builtin_primitive_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_core::block_py::Call<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    let desc = static_runtime_primitive_desc_for_call(call, emit_ctx.module_constants)?;
    if !runtime_primitive_call_params_can_satisfy_abi(call, desc, local_env, emit_ctx) {
        return None;
    }
    let DirectTargetId::RuntimePrimitive(_) = desc.target else {
        return None;
    };
    Some(
        emit_runtime_builtin_primitive_desc_call_result_with_local_env(
            fb,
            call,
            desc,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        ),
    )
}

fn emit_runtime_builtin_primitive_typed_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedCall<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let Some(desc) = static_runtime_primitive_desc_for_typed_call(call, emit_ctx.module_constants)
    else {
        return Ok(None);
    };
    if !runtime_primitive_typed_call_params_can_satisfy_abi(call, desc, local_env, emit_ctx) {
        return Ok(None);
    }
    let DirectTargetId::RuntimePrimitive(_) = desc.target else {
        return Ok(None);
    };
    Ok(Some(
        emit_runtime_builtin_primitive_typed_desc_call_result_with_local_env(
            fb,
            call,
            desc,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?,
    ))
}

fn emit_codegen_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_core::block_py::Call<InstrBlockPy>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    if let Some(result) = emit_runtime_builtin_primitive_call_result_with_local_env(
        fb,
        call,
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    ) {
        return Some(result);
    }
    if demand == ResultDemand::EffectOnly
        && let Some(result) = emit_codegen_simple_call_effect_only_with_local_env(
            fb,
            call,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )
    {
        return Some(result);
    }
    emit_codegen_simple_call_with_local_env(
        fb,
        call,
        local_env,
        emit_ctx,
        None,
        None,
        codegen_env,
        func_imports,
    )
    .map(|value| {
        emit_owned_pyobject_result_for_demand(fb, value, PyObjFacts::unknown(), emit_ctx, demand)
    })
}

fn emit_typed_codegen_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedCall<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    if let Some(result) = emit_typed_codegen_direct_callable_specialization_result_with_local_env(
        fb,
        call,
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )? {
        return Ok(Some(result));
    }

    if let Some(result) = emit_runtime_builtin_primitive_typed_call_result_with_local_env(
        fb,
        call,
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )? {
        return Ok(Some(result));
    }
    let TypedSimpleCallParts {
        simple_args,
        simple_keywords,
        has_unpack,
    } = typed_simple_call_parts(call);
    if !has_unpack
        && simple_keywords.is_empty()
        && simple_args.is_empty()
        && typed_expr_runtime_helper(call.func.as_ref(), emit_ctx) == Some(RuntimeHelperId::Globals)
    {
        emit_ctx.emit_incref_for_family(
            fb,
            emit_ctx.consts.block_const,
            None,
            RefcountFamily::ConstantClone,
        );
        return Ok(Some(emit_owned_pyobject_result_for_demand(
            fb,
            emit_ctx.consts.block_const,
            PyObjFacts::known_not_none(),
            emit_ctx,
            demand,
        )));
    }
    if has_unpack {
        let (callable, callable_is_borrowed) = emit_typed_pyobject_arg_value_with_local_env(
            fb,
            call.func.as_ref(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?;
        return emit_typed_unpack_call_result_with_local_env(
            fb,
            callable,
            callable_is_borrowed,
            call.args.as_slice(),
            call.keywords.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            demand,
        )
        .map(Some);
    }
    if !simple_keywords.is_empty() {
        let (callable, callable_is_borrowed) = emit_typed_pyobject_arg_value_with_local_env(
            fb,
            call.func.as_ref(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?;
        return emit_typed_keyword_call_result_with_local_env(
            fb,
            callable,
            callable_is_borrowed,
            simple_args.as_slice(),
            simple_keywords.as_slice(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            demand,
        )
        .map(Some);
    }
    if typed_call_can_emit_simple_positional_with_typed_inputs(call, emit_ctx) {
        return emit_typed_codegen_simple_positional_call_result_with_local_env(
            fb,
            call,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        );
    }
    Ok(None)
}

fn emit_typed_codegen_simple_positional_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedCall<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let arg_refs = typed_simple_positional_args(call)?;
    if let Some(result) = emit_typed_constructor_entry_call_with_local_env(
        fb,
        call,
        arg_refs.as_slice(),
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )? {
        return Ok(Some(result));
    }
    let (callable, callable_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        call.func.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed call callable",
    )?;

    let call_target_counter = call
        .try_semantic_instr_id()
        .and_then(|site_instr_id| emit_ctx.call_target_counter_ids.get(&site_instr_id))
        .copied();
    if let Some(plan) = call.extra.generator_instance_plan() {
        tracing::info!(
            target: "soac_generator_instance_codegen",
            function = ?emit_ctx.function_id,
            instr_id = ?call.try_semantic_instr_id(),
            generator = ?plan.function_id,
            lane = "call_typed_simple_positional",
            "typed_generator_instance_codegen_lane",
        );
    }

    let mut sampled_protocol_target = false;
    if call_access_allows_protocol_target_sample(Some(&call.access))
        && let Some(counter_id) = call_target_counter
        && arg_refs.len() == 1
        && let Some(runtime_name) =
            typed_expr_static_runtime_name(call.func.as_ref(), emit_ctx.module_constants)
        && let Some(helper_import) = protocol_method_target_helper_import(runtime_name)
        && typed_expr_pyobject_input_is_borrowed_from_local_env(arg_refs[0], local_env, emit_ctx)
    {
        let (receiver, receiver_is_borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            arg_refs[0],
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            "typed protocol next receiver",
        )?;
        debug_assert!(
            receiver_is_borrowed,
            "protocol next profiling only emits borrowed local receivers"
        );
        emit_record_protocol_method_target_sample(
            fb,
            receiver,
            counter_id,
            helper_import,
            emit_ctx,
            codegen_env,
            func_imports,
        );
        sampled_protocol_target = true;
    }

    if !sampled_protocol_target && let Some(counter_id) = call_target_counter {
        let callee_id = emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env);
        emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
    }

    let result = if call.extra.generator_instance_plan().is_some() {
        emit_typed_generator_instance_call_result_with_arg_refs(
            fb,
            callable,
            callable_is_borrowed,
            arg_refs.as_slice(),
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?
    } else {
        emit_typed_positional_call_result_with_arg_refs(
            fb,
            callable,
            callable_is_borrowed,
            arg_refs.as_slice(),
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?
    };
    Ok(Some(result))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_codegen_direct_callable_specialization_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedCall<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let TypedSimpleCallParts {
        simple_args,
        simple_keywords,
        has_unpack,
    } = typed_simple_call_parts(call);
    if has_unpack || !simple_keywords.is_empty() {
        return Ok(None);
    }
    if typed_expr_runtime_helper(call.func.as_ref(), emit_ctx).is_some() {
        return Ok(None);
    }
    if simple_args.len() == 3
        && matches!(
            typed_expr_helper_name(call.func.as_ref(), emit_ctx.module_constants),
            Some("call_super")
        )
    {
        return Ok(None);
    }

    let arg_refs = typed_simple_positional_args(call)?;
    debug_assert_eq!(simple_args.len(), arg_refs.len());

    let site_instr_id = call.try_semantic_instr_id();
    let direct_specializations = match &call.access {
        TypedCallAccessPlan::GuardedCallable { function_guards } => {
            direct_function_specializations_from_typed_guards(function_guards)
        }
        TypedCallAccessPlan::Generic => Vec::new(),
        TypedCallAccessPlan::GuardedMethod { .. }
        | TypedCallAccessPlan::GuardedRuntimeProtocolMethod { .. } => return Ok(None),
    };
    if direct_specializations.is_empty() {
        return Ok(None);
    }

    emit_typed_prepared_direct_callable_specialization_result_with_local_env(
        fb,
        call.func.as_ref(),
        arg_refs.as_slice(),
        site_instr_id,
        direct_specializations.as_slice(),
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_prepared_direct_callable_specialization_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    func: &InstrTyped,
    arg_refs: &[&InstrTyped],
    site_instr_id: Option<InstrId>,
    direct_specializations: &[DirectFunctionSpecialization],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    if direct_specializations.is_empty() {
        return Ok(None);
    }

    let call_target_counter = site_instr_id
        .and_then(|site_instr_id| emit_ctx.call_target_counter_ids.get(&site_instr_id))
        .copied();
    let direct_hit_counter_id = site_instr_id
        .and_then(|site_instr_id| emit_ctx.call_direct_hit_counter_ids.get(&site_instr_id))
        .copied();
    let direct_fallback_counter_id = site_instr_id
        .and_then(|site_instr_id| {
            emit_ctx
                .call_direct_fallback_counter_ids
                .get(&site_instr_id)
        })
        .copied();

    let (callable, callable_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        func,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed direct-call callable",
    )?;
    let should_emit_callee_id = call_target_counter.is_some() || !direct_specializations.is_empty();
    let callee_id = should_emit_callee_id
        .then(|| emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env));
    if let Some(counter_id) = call_target_counter {
        let callee_id = callee_id.expect("callee id should exist for call target counter");
        emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
    }

    let ptr_ty = emit_ctx.consts.ptr_ty;
    let result_block = fb.create_block();
    fb.append_block_param(result_block, ptr_ty);
    let generic_block = fb.create_block();
    fb.set_cold_block(generic_block);
    let direct_guard_miss_dispatch = if let Some(site_instr_id) = site_instr_id {
        prepare_typed_guard_miss_dispatch_for_instr(emit_ctx, site_instr_id, &[func], generic_block)
    } else {
        JitGuardMissDispatch::FallbackBlock(generic_block)
    };

    {
        let callee_id = callee_id.expect("callee id should exist for direct call guards");
        let callable_type = fb.ins().load(
            ptr_ty,
            ir::MemFlags::trusted(),
            callable,
            offset_of!(ffi::PyObject, ob_type) as i32,
        );
        let py_function_type = emit_type_ptr_value_for_ref(
            fb,
            codegen_env,
            emit_ctx,
            &RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Function),
        )
        .unwrap_or_else(|err| panic!("failed to bind PyFunction_Type symbol: {err}"))
        .expect("PyFunction_Type symbol should be available");
        let callable_is_exact_function =
            fb.ins()
                .icmp(ir::condcodes::IntCC::Equal, callable_type, py_function_type);
        let py_type_type = emit_type_ptr_value_for_ref(
            fb,
            codegen_env,
            emit_ctx,
            &RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Type),
        )
        .unwrap_or_else(|err| panic!("failed to bind PyType_Type symbol: {err}"))
        .expect("PyType_Type symbol should be available");
        let callable_is_exact_type =
            fb.ins()
                .icmp(ir::condcodes::IntCC::Equal, callable_type, py_type_type);
        for (index, specialization) in direct_specializations.iter().enumerate() {
            let target_function = direct_call_target_function(emit_ctx, specialization.function_id)
                .expect("direct specialization target should exist");
            let direct_block = fb.create_block();
            let miss_block = if index + 1 == direct_specializations.len() {
                direct_guard_miss_dispatch.branch_block()
            } else {
                fb.create_block()
            };
            let is_match = fb.ins().icmp_imm(
                ir::condcodes::IntCC::Equal,
                callee_id,
                specialization.function_id.to_packed_runtime_u64() as i64,
            );
            let callable_shape_matches = if is_constructor_entry_function(target_function) {
                callable_is_exact_type
            } else {
                callable_is_exact_function
            };
            let is_match = fb.ins().band(is_match, callable_shape_matches);
            fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

            fb.switch_to_block(direct_block);
            emit_record_direct_call_target_sample(
                fb,
                site_instr_id,
                specialization.function_id,
                emit_ctx,
            );
            if let Some(counter_id) = direct_hit_counter_id {
                emit_increment_counter_ref(fb, counter_id, emit_ctx);
            }
            let direct_result = emit_typed_direct_call_resolved_with_arg_plan_from_local_env(
                fb,
                callable,
                callable_is_borrowed,
                arg_refs,
                &specialization.arg_plan,
                target_function,
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
            )?;
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(direct_result)]);
            if index + 1 != direct_specializations.len() {
                fb.switch_to_block(miss_block);
            }
        }
    }

    match direct_guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(generic_block) => {
            fb.switch_to_block(generic_block);
            emit_ctx
                .direct_edge_stats
                .record_guarded_generic_fallback_block();
            if let Some(counter_id) = direct_fallback_counter_id {
                emit_increment_counter_ref(fb, counter_id, emit_ctx);
            }
            let generic_result = emit_typed_positional_call_result_with_arg_refs(
                fb,
                callable,
                callable_is_borrowed,
                arg_refs,
                local_env,
                emit_ctx,
                ResultDemand::PYOBJECT_OWNED,
                codegen_env,
                func_imports,
            )?;
            let (generic_result, ownership, _) =
                generic_result.expect_pyobject("typed direct-call fallback result");
            debug_assert!(ownership.is_owned());
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            fb.switch_to_block(block);
            fb.set_cold_block(block);
            emit_ctx
                .direct_edge_stats
                .record_guarded_generic_fallback_block();
            if let Some(counter_id) = direct_fallback_counter_id {
                emit_increment_counter_ref(fb, counter_id, emit_ctx);
            }
            if !callable_is_borrowed {
                emit_release_owned_inputs(fb, emit_ctx, &[callable]);
            }
            let deopt_result = emit_deopt_resume_call_with_local_env(
                fb,
                target,
                deopt_resume_ref,
                emit_ctx.consts.block_const,
                emit_ctx,
                local_env,
            );
            emit_deopt_result_return_or_step_null(fb, emit_ctx, deopt_result);
        }
    }

    fb.switch_to_block(result_block);
    let result = fb.block_params(result_block)[0];
    Ok(Some(
        emit_owned_pyobject_result_for_demand_with_codegen_imports(
            fb,
            result,
            PyObjFacts::unknown(),
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?,
    ))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_generic_positional_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    func: &InstrTyped,
    arg_refs: &[&InstrTyped],
    site_instr_id: Option<InstrId>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let (callable, callable_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        func,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed guarded-call callable fallback",
    )?;
    if let Some(counter_id) = site_instr_id
        .and_then(|site_instr_id| emit_ctx.call_target_counter_ids.get(&site_instr_id))
        .copied()
    {
        let callee_id = emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env);
        emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
    }
    emit_typed_positional_call_result_with_arg_refs(
        fb,
        callable,
        callable_is_borrowed,
        arg_refs,
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )
}

fn typed_getattr_parts(expr: &InstrTyped) -> Option<(&InstrTyped, &InstrTyped)> {
    match expr {
        InstrTyped::GetAttrTyped(getattr) => Some((getattr.value.as_ref(), getattr.attr.as_ref())),
        _ => None,
    }
}

fn emit_typed_codegen_guarded_callable_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedGuardedCallableCall<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    if let Some(plan) = call.extra.generator_instance_plan() {
        tracing::info!(
            target: "soac_generator_instance_codegen",
            function = ?emit_ctx.function_id,
            instr_id = ?call.try_semantic_instr_id(),
            generator = ?plan.function_id,
            lane = "guarded_callable",
            "typed_generator_instance_codegen_lane",
        );
    }
    let arg_refs = typed_simple_positional_arg_refs(
        call.args.as_slice(),
        call.keywords.as_slice(),
        "typed guarded callable call",
    )?;
    if call.extra.generator_instance_plan().is_some() {
        let (callable, callable_is_borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            call.func.as_ref(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            "typed guarded generator callable",
        )?;
        return emit_typed_generator_instance_call_result_with_arg_refs(
            fb,
            callable,
            callable_is_borrowed,
            arg_refs.as_slice(),
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        );
    }
    let direct_specializations =
        direct_function_specializations_from_typed_guards(call.function_guards.as_slice());
    if direct_specializations.is_empty() {
        let result = emit_typed_generic_positional_call_result_with_local_env(
            fb,
            call.func.as_ref(),
            arg_refs.as_slice(),
            call.try_semantic_instr_id(),
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?;
        return Ok(result);
    }

    emit_typed_prepared_direct_callable_specialization_result_with_local_env(
        fb,
        call.func.as_ref(),
        arg_refs.as_slice(),
        call.try_semantic_instr_id(),
        direct_specializations.as_slice(),
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )
    .and_then(|maybe_result| {
        maybe_result.ok_or_else(|| {
            "typed guarded callable call has no generic or direct emission path".to_string()
        })
    })
}

fn emit_typed_codegen_guarded_method_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedGuardedMethodCall<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let arg_refs = typed_simple_positional_arg_refs(
        call.args.as_slice(),
        call.keywords.as_slice(),
        "typed guarded method call",
    )?;
    let direct_method_specializations = direct_method_specializations_from_typed_guards(
        call.method_guards.as_slice(),
        call.method_name.as_str(),
    );
    if direct_method_specializations.is_empty() {
        let result = emit_typed_generic_positional_call_result_with_local_env(
            fb,
            call.func.as_ref(),
            arg_refs.as_slice(),
            call.try_semantic_instr_id(),
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?;
        return Ok(result);
    }

    let Some((receiver_expr, attr_expr)) = typed_getattr_parts(call.func.as_ref()) else {
        return Err("typed guarded method call requires a GetAttr call target".to_string());
    };
    let site_instr_id = call.try_semantic_instr_id();
    let call_target_counter = site_instr_id
        .and_then(|site_instr_id| emit_ctx.call_target_counter_ids.get(&site_instr_id))
        .copied();
    let direct_hit_counter_id = site_instr_id
        .and_then(|site_instr_id| emit_ctx.call_direct_hit_counter_ids.get(&site_instr_id))
        .copied();
    let direct_fallback_counter_id = site_instr_id
        .and_then(|site_instr_id| {
            emit_ctx
                .call_direct_fallback_counter_ids
                .get(&site_instr_id)
        })
        .copied();
    let (receiver, receiver_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        receiver_expr,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed guarded-method receiver",
    )?;

    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let result_block = fb.create_block();
    fb.append_block_param(result_block, ptr_ty);
    let generic_block = fb.create_block();
    fb.set_cold_block(generic_block);
    let method_guard_miss_dispatch = site_instr_id
        .map(|site_instr_id| {
            prepare_typed_guard_miss_dispatch_for_instr(
                emit_ctx,
                site_instr_id,
                &[receiver_expr],
                generic_block,
            )
        })
        .unwrap_or(JitGuardMissDispatch::FallbackBlock(generic_block));
    for (index, specialization) in direct_method_specializations.iter().enumerate() {
        let Some(expected_type) =
            emit_type_ptr_value_for_ref(fb, codegen_env, emit_ctx, &specialization.owner_type_ref)
                .unwrap_or_else(|err| {
                    panic!("failed to bind direct method type symbol: {err}");
                })
        else {
            continue;
        };
        let direct_block = fb.create_block();
        let miss_block = if index + 1 == direct_method_specializations.len() {
            method_guard_miss_dispatch.branch_block()
        } else {
            fb.create_block()
        };
        let is_match =
            emit_exact_type_version_match(fb, receiver, expected_type, specialization.type_version);
        fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

        fb.switch_to_block(direct_block);
        let target_function = direct_call_target_function(emit_ctx, specialization.function_id)
            .expect("direct method specialization target should exist");
        if let Some(counter_id) = call_target_counter {
            let callee_id = fb.ins().iconst(
                emit_ctx.consts.i64_ty,
                specialization.function_id.to_packed_runtime_u64() as i64,
            );
            emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
        }
        emit_record_direct_call_target_sample(
            fb,
            site_instr_id,
            specialization.function_id,
            emit_ctx,
        );
        if let Some(counter_id) = direct_hit_counter_id {
            emit_increment_counter_ref(fb, counter_id, emit_ctx);
        }
        let direct_result = emit_typed_direct_method_resolved_with_args_from_local_env(
            fb,
            receiver,
            receiver_is_borrowed,
            arg_refs.as_slice(),
            specialization,
            target_function,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?;
        fb.ins()
            .jump(result_block, &[ir::BlockArg::Value(direct_result)]);
        if index + 1 != direct_method_specializations.len() {
            fb.switch_to_block(miss_block);
        }
    }

    match method_guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(generic_block) => {
            fb.switch_to_block(generic_block);
            let (attr, attr_is_borrowed) = emit_typed_pyobject_input_with_local_env(
                fb,
                attr_expr,
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
                "typed guarded-method attr fallback",
            )?;
            let getattr_inst = fb
                .ins()
                .call(emit_ctx.pyobject_getattr_ref, &[receiver, attr]);
            let mut owned_inputs = Vec::with_capacity(2);
            if !attr_is_borrowed {
                owned_inputs.push(attr);
            }
            if !receiver_is_borrowed {
                owned_inputs.push(receiver);
            }
            let callable = emit_decref_owned_inputs_after_nullable_result(
                fb,
                emit_ctx,
                fb.inst_results(getattr_inst)[0],
                &owned_inputs,
            );
            let callable_is_null = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, callable, null_ptr);
            let callable_ok_block = fb.create_block();
            fb.append_block_param(callable_ok_block, ptr_ty);
            fb.ins().brif(
                callable_is_null,
                emit_ctx.consts.step_null_block,
                &step_null_block_args(emit_ctx),
                callable_ok_block,
                &[ir::BlockArg::Value(callable)],
            );
            fb.switch_to_block(callable_ok_block);
            let callable = fb.block_params(callable_ok_block)[0];
            if let Some(counter_id) = call_target_counter {
                let callee_id =
                    emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env);
                emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
            }
            if let Some(counter_id) = direct_fallback_counter_id {
                emit_increment_counter_ref(fb, counter_id, emit_ctx);
            }
            let generic_result = emit_typed_positional_call_result_with_arg_refs(
                fb,
                callable,
                false,
                arg_refs.as_slice(),
                local_env,
                emit_ctx,
                ResultDemand::PYOBJECT_OWNED,
                codegen_env,
                func_imports,
            )?;
            let (generic_result, ownership, _) =
                generic_result.expect_pyobject("typed guarded-method fallback result");
            debug_assert!(ownership.is_owned());
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            fb.switch_to_block(block);
            fb.set_cold_block(block);
            if let Some(counter_id) = direct_fallback_counter_id {
                emit_increment_counter_ref(fb, counter_id, emit_ctx);
            }
            if !receiver_is_borrowed {
                emit_release_owned_inputs(fb, emit_ctx, &[receiver]);
            }
            let deopt_result = emit_deopt_resume_call_with_local_env(
                fb,
                target,
                deopt_resume_ref,
                emit_ctx.consts.block_const,
                emit_ctx,
                local_env,
            );
            emit_deopt_result_return_or_step_null(fb, emit_ctx, deopt_result);
        }
    }

    fb.switch_to_block(result_block);
    let result = fb.block_params(result_block)[0];
    emit_owned_pyobject_result_for_demand_with_codegen_imports(
        fb,
        result,
        PyObjFacts::unknown(),
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )
}

fn emit_typed_codegen_direct_callable_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedDirectCallableCall<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    if let Some(plan) = call.extra.generator_instance_plan() {
        tracing::info!(
            target: "soac_generator_instance_codegen",
            function = ?emit_ctx.function_id,
            instr_id = ?call.try_semantic_instr_id(),
            generator = ?plan.function_id,
            lane = "direct_callable",
            "typed_generator_instance_codegen_lane",
        );
    }
    let mut arg_refs = Vec::with_capacity(call.args.len());
    for arg in &call.args {
        let CallArgPositional::Positional(arg) = arg else {
            return Err("typed direct callable call does not support starred args".to_string());
        };
        arg_refs.push(arg);
    }
    if call.extra.generator_instance_plan().is_some() {
        let (callable, callable_is_borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            call.func.as_ref(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            "typed direct generator callable",
        )?;
        let result = emit_typed_generator_instance_call_result_with_arg_refs(
            fb,
            callable,
            callable_is_borrowed,
            arg_refs.as_slice(),
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?;
        return Ok(result);
    }
    let TypedDirectCallableCallGuard::Function(guard) = &call.guard;
    let direct_specializations =
        direct_function_specializations_from_typed_guards(std::slice::from_ref(guard));
    emit_typed_prepared_direct_callable_specialization_result_with_local_env(
        fb,
        call.func.as_ref(),
        arg_refs.as_slice(),
        call.try_semantic_instr_id(),
        direct_specializations.as_slice(),
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )
    .and_then(|result| {
        result.ok_or_else(|| {
            "typed direct callable call has no guarded direct or generic emission path".to_string()
        })
    })
}

fn emit_typed_codegen_direct_method_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedDirectMethodCall<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let specialization = direct_method_specialization_from_typed_call(call)
        .ok_or_else(|| "typed direct method call has invalid owner type ref".to_string())?;
    let target_function = direct_call_target_function(emit_ctx, specialization.function_id)
        .ok_or_else(|| {
            format!(
                "typed direct method call target {:?} is unavailable",
                specialization.function_id
            )
        })?;
    emit_record_direct_call_target_sample(
        fb,
        call.try_semantic_instr_id(),
        specialization.function_id,
        emit_ctx,
    );
    let (receiver, receiver_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        call.receiver.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed direct-method receiver",
    )?;
    let mut arg_refs = Vec::with_capacity(call.args.len());
    for arg in &call.args {
        let CallArgPositional::Positional(arg) = arg else {
            return Err("typed direct method call does not support starred args".to_string());
        };
        arg_refs.push(arg);
    }
    let result = emit_typed_direct_method_resolved_with_args_from_local_env(
        fb,
        receiver,
        receiver_is_borrowed,
        arg_refs.as_slice(),
        &specialization,
        target_function,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    emit_owned_pyobject_result_for_demand_with_codegen_imports(
        fb,
        result,
        PyObjFacts::unknown(),
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )
}

fn emit_codegen_stmt_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrBlockPy,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    if matches!(demand, ResultDemand::I64 | ResultDemand::I64Index)
        && let Some(const_value) = codegen_expr_const_i64(expr, emit_ctx.module_constants)
    {
        let value = fb.ins().iconst(emit_ctx.consts.i64_ty, const_value);
        return Ok(emit_i64_result_for_demand(
            fb,
            value,
            IntFacts::i64_known(const_value),
            emit_ctx,
            demand,
        ));
    }
    match expr {
        InstrBlockPy::Store(op) => {
            if let Some(result) = emit_preserved_store_result_with_local_env(
                fb,
                op,
                local_env,
                emit_ctx,
                demand,
                codegen_env,
                func_imports,
            ) {
                return Ok(result);
            }
            if let Some(result) = emit_local_store_result_with_local_env(
                fb,
                expr,
                op,
                local_env,
                emit_ctx,
                demand,
                codegen_env,
                func_imports,
            ) {
                return Ok(result);
            }
        }
        InstrBlockPy::Del(op) => {
            if let Some(result) =
                emit_local_delete_result_with_local_env(fb, op, local_env, emit_ctx, demand)
            {
                return Ok(result);
            }
        }
        InstrBlockPy::BinOp(op) => {
            if let Some(result) = emit_i64_binop_result_with_local_env(
                fb,
                op,
                local_env,
                emit_ctx,
                demand,
                codegen_env,
                func_imports,
            ) {
                return Ok(result);
            }
        }
        InstrBlockPy::Call(call) => {
            if let Some(result) = emit_codegen_call_result_with_local_env(
                fb,
                call,
                local_env,
                emit_ctx,
                demand,
                codegen_env,
                func_imports,
            ) {
                return Ok(result);
            }
        }
        _ => {}
    }
    let value =
        emit_codegen_stmt_with_local_env(fb, expr, local_env, emit_ctx, codegen_env, func_imports);
    let facts = py_facts_for_codegen_expr_with_local_env(expr, local_env, emit_ctx)
        .unwrap_or_else(PyObjFacts::unknown);
    emit_owned_pyobject_result_for_demand_with_codegen_imports(
        fb,
        value,
        facts,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )
}

fn emit_resolved_name_load_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    name: &ResolvedName,
    instr_id: Option<InstrId>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    borrowed: bool,
) -> ir::Value {
    if let Some(value) =
        emit_codegen_non_local_name_load(fb, name, instr_id, local_env, emit_ctx, borrowed)
    {
        return value;
    }
    if let Some(location) = name.local_location() {
        let layout = emit_ctx
            .storage_layout
            .as_ref()
            .expect("Load local slot should have storage layout during codegen");
        let name = local_name_for_location(layout, location);
        if let Some(value) = local_env.load_location(fb, location, name, emit_ctx, borrowed) {
            return value;
        }
        panic!("missing local {name} in direct JIT state");
    }
    if name.cell_location().is_some() {
        assert!(
            !borrowed,
            "cell-backed name loads must produce owned references"
        );
        let cell_obj = emit_raw_cell_object_for_name_with_local_env(fb, name, local_env, emit_ctx);
        return emit_cell_value_load_from_raw_cell(fb, cell_obj, emit_ctx);
    }
    panic!("Load should be resolved before codegen: {name:?}");
}

fn emit_typed_direct_call_guard_test_value_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &TypedDirectCallGuardTest<InstrTyped>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<SoacValue, String> {
    let value_is_borrowed = typed_expr_pyobject_input_is_borrowed_from_local_env(
        op.value.as_ref(),
        local_env,
        emit_ctx,
    );
    let (raw_value, ownership, facts) = emit_typed_pyobject_value_with_local_env(
        fb,
        op.value.as_ref(),
        local_env,
        emit_ctx,
        value_is_borrowed,
        codegen_env,
        func_imports,
        "typed direct-call guard input",
    )?;

    let guard = match &op.kind {
        TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id } => {
            emit_exact_function_id_match_bool01(fb, raw_value, *function_id, emit_ctx, codegen_env)?
        }
        TypedDirectCallGuardTestKind::ExactTypeVersion {
            owner_type_ref,
            type_version,
            ..
        } => {
            let Some(owner_type_ref) = reloc_type_ref_from_typed_attr_owner_ref(owner_type_ref)
            else {
                return Err(format!(
                    "typed direct method guard references unknown owner type {owner_type_ref:?}"
                ));
            };
            let Some(expected_type) =
                emit_type_ptr_value_for_ref(fb, codegen_env, emit_ctx, &owner_type_ref)?
            else {
                return Err(format!(
                    "typed direct method guard owner type is not registered: {owner_type_ref:?}"
                ));
            };
            SoacValue::i32(
                emit_exact_type_version_match(fb, raw_value, expected_type, *type_version),
                IntFacts::i32_bool01(),
            )
        }
    };

    if ownership.is_owned() {
        emit_release_owned_pyobject(fb, raw_value, Some(facts), emit_ctx);
    }
    Ok(guard)
}

fn typed_intrinsic_operation_may_emit_pyobject(expr: &InstrTyped) -> bool {
    match expr {
        InstrTyped::GetItem(_) | InstrTyped::SetItem(_) | InstrTyped::DelItem(_) => true,
        InstrTyped::MakeCell(_) => true,
        InstrTyped::Store(op) => op.name.location.is_global(),
        InstrTyped::Del(op) => op.name.location.is_global(),
        _ => false,
    }
}

fn typed_instr_kind(expr: &InstrTyped) -> &'static str {
    match expr {
        InstrTyped::Truthy(_) => "Truthy",
        InstrTyped::Load(_) => "Load",
        InstrTyped::BinOp(_) => "BinOp",
        InstrTyped::Tuple(_) => "Tuple",
        InstrTyped::UnaryOp(_) => "UnaryOp",
        InstrTyped::CalleeFunctionId(_) => "CalleeFunctionId",
        InstrTyped::CallTyped(_) => "CallTyped",
        InstrTyped::GuardedCallableCallTyped(_) => "GuardedCallableCallTyped",
        InstrTyped::GuardedMethodCallTyped(_) => "GuardedMethodCallTyped",
        InstrTyped::DirectCallableCallTyped(_) => "DirectCallableCallTyped",
        InstrTyped::DirectMethodCallTyped(_) => "DirectMethodCallTyped",
        InstrTyped::DirectCallGuardTest(_) => "DirectCallGuardTest",
        InstrTyped::CallDirect(_) => "CallDirect",
        InstrTyped::GetAttrTyped(_) => "GetAttrTyped",
        InstrTyped::SetAttrTyped(_) => "SetAttrTyped",
        InstrTyped::GetItem(_) => "GetItem",
        InstrTyped::SetItem(_) => "SetItem",
        InstrTyped::DelItem(_) => "DelItem",
        InstrTyped::Store(_) => "Store",
        InstrTyped::Del(_) => "Del",
        InstrTyped::MakeCell(_) => "MakeCell",
        InstrTyped::IncrementCounter(_) => "IncrementCounter",
        InstrTyped::CellRef(_) => "CellRef",
        InstrTyped::MakeFunctionWithClosure(_) => "MakeFunctionWithClosure",
    }
}

fn lower_typed_instr_to_codegen_legacy_for_fallback(
    expr: InstrTyped,
) -> Result<InstrBlockPy, String> {
    let context = format!(
        "{}{}",
        typed_instr_kind(&expr),
        expr.try_semantic_instr_id()
            .map(|instr_id| format!(" #{instr_id}"))
            .unwrap_or_default()
    );
    try_lower_typed_instr_to_codegen_legacy(expr)
        .map_err(|err| format!("{err} [typed_legacy_fallback={context}]"))
}

#[allow(dead_code)]
fn emit_typed_codegen_expr_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        expr,
        local_env,
        emit_ctx,
        borrowed,
        codegen_env,
        func_imports,
    )?;
    Ok(match value {
        SoacValue::PyObject {
            value,
            ownership,
            facts,
        } => {
            if !borrowed && matches!(ownership, ValueOwnership::Borrowed) {
                emit_ctx.emit_incref_for_family(
                    fb,
                    value,
                    Some(facts),
                    RefcountFamily::BorrowedResultClone,
                );
            }
            debug_assert!(
                !borrowed || !ownership.is_owned(),
                "borrowed PyObject request unexpectedly produced an owned value"
            );
            debug_assert!(
                !matches!(ownership, ValueOwnership::Immortal) || facts.is_immortal(),
                "immortal PyObject ownership should carry immortal facts"
            );
            value
        }
        SoacValue::I32 {
            value: truth_i32,
            facts,
        } if facts.is_i32_bool01() => {
            if borrowed {
                return Err(
                    "typed scalar bool materialization cannot satisfy a borrowed PyObject load"
                        .to_string(),
                );
            }
            let is_true = fb
                .ins()
                .icmp_imm(ir::condcodes::IntCC::NotEqual, truth_i32, 0);
            let true_const = emit_true_const(fb, emit_ctx);
            let false_const = emit_false_const(fb, emit_ctx);
            let bool_value = fb.ins().select(is_true, true_const, false_const);
            if !borrowed {
                emit_ctx.emit_incref_for_family(
                    fb,
                    bool_value,
                    None,
                    RefcountFamily::ConstantClone,
                );
            }
            bool_value
        }
        SoacValue::I32 { .. } | SoacValue::I64 { .. } => {
            if borrowed {
                return Err(format!(
                    "typed scalar {:?} materialization cannot satisfy a borrowed PyObject load",
                    value.repr()
                ));
            }
            let result = emit_soac_value_as_pyobject_for_demand(
                fb,
                value,
                emit_ctx,
                ResultDemand::PYOBJECT_OWNED,
            );
            let (value, ownership, _) =
                result.expect_pyobject("typed scalar expression materialization");
            debug_assert!(ownership.is_owned() || matches!(ownership, ValueOwnership::Immortal));
            value
        }
    })
}

fn emit_codegen_stmt_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrBlockPy,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    match expr {
        InstrBlockPy::Store(op) => {
            if let Some(result) = emit_preserved_store_result_with_local_env(
                fb,
                op,
                local_env,
                emit_ctx,
                ResultDemand::PYOBJECT_OWNED,
                codegen_env,
                func_imports,
            ) {
                let (value, ownership, _) = result.expect_pyobject("legacy preserved store");
                assert!(
                    ownership.is_owned(),
                    "legacy preserved store should produce an owned PyObject"
                );
                return value;
            }
            if let Some(value) = emit_local_store_with_local_env(
                fb,
                expr,
                op,
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
            ) {
                return value;
            }
        }
        InstrBlockPy::Del(op) => {
            if let Some(result) = emit_preserved_delete_result_with_local_env(
                fb,
                op,
                local_env,
                emit_ctx,
                ResultDemand::PYOBJECT_OWNED,
            ) {
                let (value, ownership, _) = result.expect_pyobject("legacy preserved delete");
                assert!(
                    ownership.is_owned(),
                    "legacy preserved delete should produce an owned PyObject"
                );
                return value;
            }
            if let Some(value) = emit_local_delete_with_local_env(fb, op, local_env, emit_ctx) {
                return value;
            }
        }
        _ => {}
    }
    emit_codegen_expr_with_local_env(
        fb,
        expr,
        local_env,
        emit_ctx,
        false,
        codegen_env,
        func_imports,
    )
}

#[allow(dead_code)]
fn emit_typed_codegen_stmt_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    if let InstrTyped::Store(op) = expr {
        if let Some(result) = emit_typed_preserved_store_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )? {
            let (value, ownership, _) = result.expect_pyobject("typed statement preserved store");
            assert!(
                ownership.is_owned(),
                "typed statement preserved store should produce an owned PyObject"
            );
            return Ok(value);
        }
        if let Some(result) = emit_typed_local_store_result_with_local_env(
            fb,
            expr,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )? {
            let (value, ownership, _) = result.expect_pyobject("typed statement local store");
            assert!(
                ownership.is_owned(),
                "typed statement local store should produce an owned PyObject"
            );
            return Ok(value);
        }
        if let Some(result) = emit_typed_owned_cell_makecell_store_result_with_local_env(
            fb,
            expr,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )? {
            let (value, ownership, _) =
                result.expect_pyobject("typed statement owned cell MakeCell store");
            assert!(
                ownership.is_owned(),
                "typed statement owned cell MakeCell store should produce an owned PyObject"
            );
            return Ok(value);
        }
        if let Some(result) = emit_typed_cell_store_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        )? {
            let (value, ownership, _) = result.expect_pyobject("typed statement cell store");
            assert!(
                ownership.is_owned(),
                "typed statement cell store should produce an owned PyObject"
            );
            return Ok(value);
        }
    }
    if let InstrTyped::Del(op) = expr {
        if let Some(result) = emit_typed_preserved_delete_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
        ) {
            let (value, ownership, _) = result.expect_pyobject("typed statement preserved delete");
            assert!(
                ownership.is_owned(),
                "typed statement preserved delete should produce an owned PyObject"
            );
            return Ok(value);
        }
        if let Some(result) = emit_typed_local_delete_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
        ) {
            let (value, ownership, _) = result.expect_pyobject("typed statement local delete");
            assert!(
                ownership.is_owned(),
                "typed statement local delete should produce an owned PyObject"
            );
            return Ok(value);
        }
        if let Some(result) = emit_typed_cell_delete_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            ResultDemand::PYOBJECT_OWNED,
            codegen_env,
            func_imports,
        ) {
            let (value, ownership, _) = result.expect_pyobject("typed statement cell delete");
            assert!(
                ownership.is_owned(),
                "typed statement cell delete should produce an owned PyObject"
            );
            return Ok(value);
        }
    }
    if typed_intrinsic_operation_may_emit_pyobject(expr) {
        let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
            fb,
            local_env,
            ctx: emit_ctx,
            codegen_env,
            func_imports,
            owned_transfer_temp_load: None,
        };
        if let Some(value) = intrinsics::emit_typed_operation(expr, &mut intrinsic_state) {
            return Ok(value);
        }
    }

    if matches!(
        expr,
        InstrTyped::Truthy(_)
            | InstrTyped::Load(_)
            | InstrTyped::BinOp(_)
            | InstrTyped::Tuple(_)
            | InstrTyped::UnaryOp(_)
            | InstrTyped::IncrementCounter(_)
            | InstrTyped::CellRef(_)
            | InstrTyped::MakeFunctionWithClosure(_)
            | InstrTyped::CallTyped(_)
            | InstrTyped::GuardedCallableCallTyped(_)
            | InstrTyped::GuardedMethodCallTyped(_)
            | InstrTyped::DirectCallableCallTyped(_)
            | InstrTyped::DirectMethodCallTyped(_)
            | InstrTyped::DirectCallGuardTest(_)
    ) {
        return emit_typed_codegen_expr_with_local_env(
            fb,
            expr,
            local_env,
            emit_ctx,
            false,
            codegen_env,
            func_imports,
        );
    }

    let legacy_expr = lower_typed_instr_to_codegen_legacy_for_fallback(expr.clone())?;
    Ok(emit_codegen_stmt_with_local_env(
        fb,
        &legacy_expr,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    ))
}

fn emit_typed_codegen_stmt_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    if let Some(result) =
        emit_typed_local_load_result_with_local_env(fb, expr, local_env, emit_ctx, demand)
    {
        return Ok(result);
    }
    if matches!(demand, ResultDemand::I64 | ResultDemand::I64Index)
        && let Some(const_value) = typed_expr_const_i64(expr, emit_ctx.module_constants)
    {
        let value = fb.ins().iconst(emit_ctx.consts.i64_ty, const_value);
        return Ok(emit_i64_result_for_demand(
            fb,
            value,
            IntFacts::i64_known(const_value),
            emit_ctx,
            demand,
        ));
    }
    if matches!(demand, ResultDemand::I64 | ResultDemand::I64Index)
        && let Some(result) = emit_typed_exact_int_expr_i64_result(
            fb,
            expr,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?
    {
        return Ok(result);
    }
    if matches!(demand, ResultDemand::I32Bool01)
        && let Some(result) = emit_typed_exact_int_expr_i32_bool01_result(
            fb,
            expr,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?
    {
        return Ok(result);
    }
    if let InstrTyped::Tuple(_) = expr {
        let result = emit_typed_codegen_expr_value_with_local_env(
            fb,
            expr,
            local_env,
            emit_ctx,
            false,
            codegen_env,
            func_imports,
        )?;
        let (value, ownership, facts) = result.expect_pyobject("typed tuple statement result");
        return Ok(match demand {
            ResultDemand::EffectOnly => {
                if ownership.is_owned() && !facts.is_immortal() {
                    emit_ctx.emit_decref_for_family(
                        fb,
                        value,
                        Some(facts),
                        RefcountFamily::OwnedTemporary,
                    );
                }
                EmitResult::no_value()
            }
            ResultDemand::PyObject { .. } => {
                if !ownership.can_satisfy_pyobject_demand(demand) {
                    return Err(format!(
                        "typed tuple statement result produced {ownership:?}, but demand is {demand:?}"
                    ));
                }
                EmitResult::pyobject(value, ownership, facts)
            }
            ResultDemand::I32Bool01 | ResultDemand::I64 | ResultDemand::I64Index => {
                panic!("typed tuple cannot satisfy non-PyObject demand {demand:?}")
            }
        });
    }

    if let InstrTyped::CallTyped(op) = expr {
        if let Some(result) = emit_typed_codegen_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )? {
            return Ok(result);
        }
    }
    if let InstrTyped::BinOp(op) = expr
        && let Some(result) = emit_typed_i64_binop_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )?
    {
        return Ok(result);
    }
    if let InstrTyped::GuardedCallableCallTyped(op) = expr {
        return emit_typed_codegen_guarded_callable_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        );
    }
    if let InstrTyped::GuardedMethodCallTyped(op) = expr {
        return emit_typed_codegen_guarded_method_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        );
    }
    if let InstrTyped::DirectCallableCallTyped(op) = expr {
        return emit_typed_codegen_direct_callable_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        );
    }
    if let InstrTyped::DirectMethodCallTyped(op) = expr {
        return emit_typed_codegen_direct_method_call_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        );
    }
    if let InstrTyped::GetAttrTyped(op) = expr {
        let result = if let TypedAttrAccessPlan::IndexedField {
            source,
            counter_source,
            guards,
        } = &op.access
        {
            emit_typed_indexed_getattr(
                fb,
                op,
                *source,
                *counter_source,
                guards,
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
            )?
        } else {
            None
        };
        let result = match result {
            Some(result) => result,
            None => {
                emit_typed_getattr_fallback(fb, op, local_env, emit_ctx, codegen_env, func_imports)?
            }
        };
        let (value, ownership, facts) = result.expect_pyobject("typed getattr statement result");
        return Ok(match demand {
            ResultDemand::EffectOnly => {
                if ownership.is_owned() && !facts.is_immortal() {
                    emit_ctx.emit_decref_for_family(
                        fb,
                        value,
                        Some(facts),
                        RefcountFamily::OwnedTemporary,
                    );
                }
                EmitResult::no_value()
            }
            ResultDemand::PyObject { .. } => {
                if !ownership.can_satisfy_pyobject_demand(demand) {
                    return Err(format!(
                        "typed getattr statement result produced {ownership:?}, but demand is {demand:?}"
                    ));
                }
                EmitResult::pyobject(value, ownership, facts)
            }
            ResultDemand::I32Bool01 | ResultDemand::I64 | ResultDemand::I64Index => {
                panic!("typed getattr cannot satisfy non-PyObject demand {demand:?}")
            }
        });
    }
    if let InstrTyped::SetAttrTyped(op) = expr {
        if let TypedAttrAccessPlan::IndexedField {
            source,
            counter_source,
            guards,
        } = &op.access
        {
            if let Some(result) = emit_typed_indexed_setattr(
                fb,
                op,
                *source,
                *counter_source,
                guards,
                local_env,
                emit_ctx,
                demand,
                codegen_env,
                func_imports,
            )? {
                return Ok(result);
            }
        }
        return emit_typed_setattr_fallback(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        );
    }
    if let InstrTyped::Store(op) = expr {
        if let Some(result) = emit_typed_preserved_store_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )? {
            return Ok(result);
        }
        if let Some(result) = emit_typed_local_store_result_with_local_env(
            fb,
            expr,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )? {
            return Ok(result);
        }
        if let Some(result) = emit_typed_owned_cell_makecell_store_result_with_local_env(
            fb,
            expr,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )? {
            return Ok(result);
        }
        if let Some(result) = emit_typed_cell_store_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        )? {
            return Ok(result);
        }
        if op.name.location.is_global() {
            let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
                fb,
                local_env,
                ctx: emit_ctx,
                codegen_env,
                func_imports,
                owned_transfer_temp_load: None,
            };
            if let Some(value) = intrinsics::emit_typed_operation(expr, &mut intrinsic_state) {
                let facts = expr
                    .result_facts()
                    .and_then(ValueFacts::as_pyobj)
                    .unwrap_or_else(PyObjFacts::unknown);
                return Ok(emit_owned_pyobject_result_for_demand(
                    fb, value, facts, emit_ctx, demand,
                ));
            }
        }
    }
    if let InstrTyped::Del(op) = expr {
        if let Some(result) =
            emit_typed_preserved_delete_result_with_local_env(fb, op, local_env, emit_ctx, demand)
        {
            return Ok(result);
        }
        if let Some(result) =
            emit_typed_local_delete_result_with_local_env(fb, op, local_env, emit_ctx, demand)
        {
            return Ok(result);
        }
        if let Some(result) = emit_typed_cell_delete_result_with_local_env(
            fb,
            op,
            local_env,
            emit_ctx,
            demand,
            codegen_env,
            func_imports,
        ) {
            return Ok(result);
        }
    }
    if typed_intrinsic_operation_may_emit_pyobject(expr) {
        let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
            fb,
            local_env,
            ctx: emit_ctx,
            codegen_env,
            func_imports,
            owned_transfer_temp_load: None,
        };
        if let Some(value) = intrinsics::emit_typed_operation(expr, &mut intrinsic_state) {
            let facts = expr
                .result_facts()
                .and_then(ValueFacts::as_pyobj)
                .unwrap_or_else(PyObjFacts::unknown);
            return Ok(emit_owned_pyobject_result_for_demand(
                fb, value, facts, emit_ctx, demand,
            ));
        }
    }

    if let Some(result) = emit_typed_exact_int_expr_pyobject_result(
        fb,
        expr,
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )? {
        return Ok(result);
    }

    if matches!(
        expr,
        InstrTyped::Truthy(_)
            | InstrTyped::Load(_)
            | InstrTyped::BinOp(_)
            | InstrTyped::UnaryOp(_)
            | InstrTyped::CallTyped(_)
            | InstrTyped::GuardedCallableCallTyped(_)
            | InstrTyped::GuardedMethodCallTyped(_)
            | InstrTyped::DirectCallableCallTyped(_)
            | InstrTyped::DirectMethodCallTyped(_)
            | InstrTyped::DirectCallGuardTest(_)
            | InstrTyped::IncrementCounter(_)
            | InstrTyped::CellRef(_)
            | InstrTyped::MakeFunctionWithClosure(_)
    ) {
        if demand == ResultDemand::I32_BOOL01 {
            return emit_typed_codegen_i32_bool01_result_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
            );
        }
        let value = emit_typed_codegen_stmt_with_local_env(
            fb,
            expr,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?;
        let (ownership, facts) = planned_owned_pyobject_result_for_typed_expr(expr, local_env);
        return Ok(emit_soac_value_result_for_demand(
            fb,
            SoacValue::pyobject_with_ownership(value, ownership, facts),
            emit_ctx,
            demand,
            None,
        ));
    }

    let legacy_expr = lower_typed_instr_to_codegen_legacy_for_fallback(expr.clone())?;
    emit_codegen_stmt_result_with_local_env(
        fb,
        &legacy_expr,
        local_env,
        emit_ctx,
        demand,
        codegen_env,
        func_imports,
    )
}

fn emit_typed_local_load_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
) -> Option<EmitResult> {
    if let InstrTyped::Load(op) = expr
        && let Some((value, facts)) = local_env.scalar_i64_value_for_load(&op.name)
    {
        return Some(emit_soac_value_result_for_demand(
            fb,
            SoacValue::i64(value, facts),
            emit_ctx,
            demand,
            None,
        ));
    }
    if let InstrTyped::Load(op) = expr
        && let Some((value, facts)) = local_env.scalar_i32_bool01_value_for_load(&op.name)
    {
        return Some(emit_soac_value_result_for_demand(
            fb,
            SoacValue::i32(value, facts),
            emit_ctx,
            demand,
            None,
        ));
    }
    let (ownership, facts) = typed_local_load_direct_result_plan(
        expr,
        local_env,
        &emit_ctx.stack_slots,
        emit_ctx.storage_layout.as_ref(),
        demand,
    )?;
    let InstrTyped::Load(op) = expr else {
        unreachable!("typed local load result plan only accepts loads");
    };
    let value = emit_resolved_name_load_with_local_env(
        fb,
        &op.name,
        op.try_semantic_instr_id(),
        local_env,
        emit_ctx,
        true,
    );
    Some(emit_soac_value_result_for_demand(
        fb,
        SoacValue::pyobject_with_ownership(value, ownership, facts),
        emit_ctx,
        demand,
        None,
    ))
}

#[derive(Clone, Copy, Debug)]
enum OptV3MechanicalValue {
    PyObject { value: ir::Value, owned: bool },
    I64(ir::Value),
    I32Bool01(ir::Value),
}

struct OptV3RegionInputValues {
    values: HashMap<PlanValue, OptV3MechanicalValue>,
    preseeded_scalars: HashSet<PlanValue>,
    preseeded_convert_inputs: HashSet<PlanValue>,
}

struct OptV3ExactIntDeoptMissTarget {
    block: ir::Block,
    target: JitDeoptExitRef,
    deopt_resume_ref: ir::FuncRef,
}

#[derive(Clone, Copy, Debug)]
struct OptV3PyObjectResult {
    value: ir::Value,
    ownership: ValueOwnership,
}

#[derive(Clone, Copy)]
struct ExactIntBranchEmissionSelection<'a> {
    hot_plan: &'a RegionPlan,
    hot_region: &'a MechanicalRegionEmission,
    fallback_plan: &'a RegionPlan,
    fallback_region: &'a MechanicalRegionEmission,
}

#[derive(Clone, Copy)]
struct ExactIntReturnEmissionSelection<'a> {
    hot_plan: &'a RegionPlan,
    hot_region: &'a MechanicalRegionEmission,
    fallback_plan: &'a RegionPlan,
    fallback_region: &'a MechanicalRegionEmission,
}

impl<'a> From<OptV3ExactIntBranchSelection<'a>> for ExactIntBranchEmissionSelection<'a> {
    fn from(selection: OptV3ExactIntBranchSelection<'a>) -> Self {
        Self {
            hot_plan: selection.hot_plan,
            hot_region: selection.hot_region,
            fallback_plan: selection.fallback_plan,
            fallback_region: selection.fallback_region,
        }
    }
}

impl<'a> From<OptV3ExactIntReturnSelection<'a>> for ExactIntReturnEmissionSelection<'a> {
    fn from(selection: OptV3ExactIntReturnSelection<'a>) -> Self {
        Self {
            hot_plan: selection.hot_plan,
            hot_region: selection.hot_region,
            fallback_plan: selection.fallback_plan,
            fallback_region: selection.fallback_region,
        }
    }
}

impl<'a> From<&'a TypedExactIntBranchPlan> for ExactIntBranchEmissionSelection<'a> {
    fn from(plan: &'a TypedExactIntBranchPlan) -> Self {
        Self {
            hot_plan: &plan.hot_plan,
            hot_region: &plan.hot_region,
            fallback_plan: &plan.fallback_plan,
            fallback_region: &plan.fallback_region,
        }
    }
}

impl<'a> From<&'a TypedExactIntReturnPlan> for ExactIntReturnEmissionSelection<'a> {
    fn from(plan: &'a TypedExactIntReturnPlan) -> Self {
        Self {
            hot_plan: &plan.hot_plan,
            hot_region: &plan.hot_region,
            fallback_plan: &plan.fallback_plan,
            fallback_region: &plan.fallback_region,
        }
    }
}

impl OptV3MechanicalValue {
    fn matches_rep(self, rep: Rep) -> bool {
        matches!(
            (self, rep),
            (
                Self::PyObject { owned: true, .. },
                Rep::PyObjectOwned | Rep::PyObjectBorrowed | Rep::PyObjectImmortal
            ) | (
                Self::PyObject { owned: false, .. },
                Rep::PyObjectBorrowed | Rep::PyObjectImmortal
            ) | (Self::I64(_), Rep::I64)
                | (Self::I32Bool01(_), Rep::I32Bool01)
        )
    }
}

fn typed_indexed_field_guards_by_instr(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<InstrId, Vec<TypedIndexedFieldGuard>> {
    struct Collector {
        guards_by_instr: HashMap<InstrId, Vec<TypedIndexedFieldGuard>>,
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::GetAttrTyped(op) = expr
                && let TypedAttrAccessPlan::IndexedField { guards, .. } = &op.access
            {
                self.guards_by_instr
                    .entry(op.semantic_instr_id())
                    .or_default()
                    .extend(guards.iter().cloned());
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        guards_by_instr: HashMap::new(),
    };
    collector.visit_fn(function);
    collector.guards_by_instr
}

fn emit_typed_exact_int_branch_truth_i32(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<ir::Value>, String> {
    let Some(plan) = expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_branch_plan())
    else {
        return Ok(None);
    };
    emit_opt_v3_exact_int_branch_selection(
        fb,
        plan.instr_id,
        plan.into(),
        emit_ctx.indexed_field_guards_by_instr,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
    .map(Some)
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_exact_int_return_pyobject(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<ir::Value>, String> {
    let Some(plan) = expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
    else {
        return Ok(None);
    };
    emit_opt_v3_exact_int_return_selection(
        fb,
        plan.instr_id,
        plan.into(),
        emit_ctx.indexed_field_guards_by_instr,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
    .map(|result| Some(result.value))
}

fn exact_int_deopt_resume_point(
    emit_ctx: &JitEmitCtx<'_>,
    instr_id: InstrId,
) -> LocalEnvResumePoint {
    emit_ctx
        .guard_miss_resume_point
        .unwrap_or(LocalEnvResumePoint::BeforeInstr {
            key: InstrKey::new(emit_ctx.function_id, instr_id),
        })
}

fn opt_v3_exact_int_deopt_miss_target_available(
    instr_id: InstrId,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    if emit_ctx
        .guard_miss_deopt_ref_for_instr_id(instr_id)
        .is_none()
    {
        return false;
    }
    let point = exact_int_deopt_resume_point(emit_ctx, instr_id);
    if emit_ctx
        .runtime_supported_deopt_resume_points
        .is_some_and(|supported| !supported.contains(&point))
    {
        return false;
    }
    let Some(function) = emit_ctx
        .module
        .callable_defs
        .iter()
        .find(|function| function.function_id == emit_ctx.function_id)
    else {
        return false;
    };
    if runtime_jit_typed_deopt_continuation_for_point(function, emit_ctx.instr_locations, point)
        .unsupported_reason()
        .is_some()
    {
        return false;
    }
    emit_ctx.deopt_resume_plan.entry(point).is_some()
        && emit_ctx.require_deopt_record_ref(point).is_ok()
}

fn prepare_opt_v3_exact_int_deopt_miss_target(
    fb: &mut FunctionBuilder<'_>,
    instr_id: InstrId,
    emit_ctx: &JitEmitCtx<'_>,
) -> Option<OptV3ExactIntDeoptMissTarget> {
    let deopt_resume_ref = emit_ctx.guard_miss_deopt_ref_for_instr_id(instr_id)?;
    let block = fb.create_block();
    fb.set_cold_block(block);
    let resume_point = exact_int_deopt_resume_point(emit_ctx, instr_id);
    let target = emit_ctx
        .guard_miss_target_for_resume_point(resume_point, block)
        .ok()?;
    Some(OptV3ExactIntDeoptMissTarget {
        block,
        target: target.deopt_exit(),
        deopt_resume_ref,
    })
}

fn emit_opt_v3_exact_int_deopt_miss_block(
    fb: &mut FunctionBuilder<'_>,
    deopt_miss: OptV3ExactIntDeoptMissTarget,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) {
    fb.switch_to_block(deopt_miss.block);
    fb.set_cold_block(deopt_miss.block);
    let deopt_result = emit_deopt_resume_call_with_local_env(
        fb,
        deopt_miss.target,
        deopt_miss.deopt_resume_ref,
        emit_ctx.consts.block_const,
        emit_ctx,
        local_env,
    );
    emit_deopt_result_return_or_step_null(fb, emit_ctx, deopt_result);
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_exact_int_expr_i64_result(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    if !matches!(demand, ResultDemand::I64 | ResultDemand::I64Index) {
        return Ok(None);
    }
    let Some(plan) = expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
    else {
        return Ok(None);
    };
    let Some(result_value) = planning::exact_int_return_plan_i64_result(plan) else {
        return Ok(None);
    };
    let Some(deopt_miss) = prepare_opt_v3_exact_int_deopt_miss_target(fb, plan.instr_id, emit_ctx)
    else {
        return Ok(None);
    };
    let result = emit_opt_v3_exact_int_return_deopt_i64_selection(
        fb,
        plan,
        result_value,
        deopt_miss,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    Ok(Some(emit_i64_result_for_demand(
        fb,
        result,
        IntFacts::i64_unknown(),
        emit_ctx,
        demand,
    )))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_exact_int_expr_i32_bool01_result(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    if !matches!(demand, ResultDemand::I32Bool01) {
        return Ok(None);
    }
    let Some(plan) = expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
    else {
        return Ok(None);
    };
    let Some(result_value) = planning::exact_int_return_plan_i32_bool01_result(plan) else {
        return Ok(None);
    };
    let result = if let Some(deopt_miss) =
        prepare_opt_v3_exact_int_deopt_miss_target(fb, plan.instr_id, emit_ctx)
    {
        emit_opt_v3_exact_int_return_deopt_i32_bool01_selection(
            fb,
            plan,
            result_value,
            deopt_miss,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?
    } else {
        emit_opt_v3_exact_int_return_i32_bool01_selection(
            fb,
            plan.instr_id,
            plan.into(),
            emit_ctx.indexed_field_guards_by_instr,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?
    };
    Ok(Some(EmitResult::i32(result, IntFacts::i32_bool01())))
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_exact_int_return_deopt_i64_selection(
    fb: &mut FunctionBuilder<'_>,
    plan: &TypedExactIntReturnPlan,
    result_value: PlanValue,
    deopt_miss: OptV3ExactIntDeoptMissTarget,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let result_block = fb.create_block();
    fb.append_block_param(result_block, emit_ctx.consts.i64_ty);
    let mut skipped_outputs = HashSet::new();
    if let Some(exit) = plan.hot_region.exits.first()
        && let MechanicalExitKind::Return { value } = exit.kind
    {
        skipped_outputs.insert(value);
    }

    let mut hot_values = opt_v3_region_input_values(
        fb,
        &plan.hot_plan,
        local_env,
        emit_ctx,
        codegen_env,
        emit_ctx.indexed_field_guards_by_instr,
        Some(deopt_miss.block),
        "exact-int scalar expression hot region",
    )?;
    emit_opt_v3_mechanical_region_steps_with_skipped_outputs(
        fb,
        &plan.hot_region,
        &mut hot_values.values,
        &hot_values.preseeded_scalars,
        &hot_values.preseeded_convert_inputs,
        Some(deopt_miss.block),
        &skipped_outputs,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let result = opt_v3_i64_value(&hot_values.values, result_value)?;
    fb.ins().jump(result_block, &[ir::BlockArg::Value(result)]);
    emit_opt_v3_exact_int_deopt_miss_block(fb, deopt_miss, local_env, emit_ctx);

    fb.switch_to_block(result_block);
    Ok(fb.block_params(result_block)[0])
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_exact_int_return_deopt_i32_bool01_selection(
    fb: &mut FunctionBuilder<'_>,
    plan: &TypedExactIntReturnPlan,
    result_value: PlanValue,
    deopt_miss: OptV3ExactIntDeoptMissTarget,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let result_block = fb.create_block();
    fb.append_block_param(result_block, emit_ctx.consts.i32_ty);
    let mut skipped_outputs = HashSet::new();
    if let Some(exit) = plan.hot_region.exits.first()
        && let MechanicalExitKind::Return { value } = exit.kind
    {
        skipped_outputs.insert(value);
    }

    let mut hot_values = opt_v3_region_input_values(
        fb,
        &plan.hot_plan,
        local_env,
        emit_ctx,
        codegen_env,
        emit_ctx.indexed_field_guards_by_instr,
        Some(deopt_miss.block),
        "exact-int bool expression hot region",
    )?;
    emit_opt_v3_mechanical_region_steps_with_skipped_outputs(
        fb,
        &plan.hot_region,
        &mut hot_values.values,
        &hot_values.preseeded_scalars,
        &hot_values.preseeded_convert_inputs,
        Some(deopt_miss.block),
        &skipped_outputs,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let result = opt_v3_i32_bool01_value(&hot_values.values, result_value)?;
    fb.ins().jump(result_block, &[ir::BlockArg::Value(result)]);
    emit_opt_v3_exact_int_deopt_miss_block(fb, deopt_miss, local_env, emit_ctx);

    fb.switch_to_block(result_block);
    Ok(fb.block_params(result_block)[0])
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_exact_int_return_deopt_immortal_pyobject_selection(
    fb: &mut FunctionBuilder<'_>,
    value_instr_id: InstrId,
    selection: ExactIntReturnEmissionSelection<'_>,
    indexed_field_guards_by_instr: &HashMap<InstrId, Vec<TypedIndexedFieldGuard>>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<OptV3PyObjectResult, String> {
    let Some(deopt_miss) = prepare_opt_v3_exact_int_deopt_miss_target(fb, value_instr_id, emit_ctx)
    else {
        return Err(format!(
            "exact-int immortal PyObject result for {value_instr_id} requires a guard-miss deopt target"
        ));
    };

    let result_block = fb.create_block();
    fb.append_block_param(result_block, emit_ctx.consts.ptr_ty);

    let mut hot_values = opt_v3_region_input_values(
        fb,
        selection.hot_plan,
        local_env,
        emit_ctx,
        codegen_env,
        indexed_field_guards_by_instr,
        Some(deopt_miss.block),
        "exact-int immortal PyObject hot region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.hot_region,
        &mut hot_values.values,
        &hot_values.preseeded_scalars,
        &hot_values.preseeded_convert_inputs,
        Some(deopt_miss.block),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let hot_result =
        opt_v3_region_return_pyobject(selection.hot_region, &hot_values.values, value_instr_id)?;
    if hot_result.ownership != ValueOwnership::Immortal {
        return Err(format!(
            "exact-int immortal PyObject result for {value_instr_id} produced {:?}",
            hot_result.ownership
        ));
    }
    emit_opt_v3_release_owned_values_except(
        fb,
        &hot_values.values,
        Some(hot_result.value),
        emit_ctx,
    );
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(hot_result.value)]);
    emit_opt_v3_exact_int_deopt_miss_block(fb, deopt_miss, local_env, emit_ctx);

    fb.switch_to_block(result_block);
    Ok(OptV3PyObjectResult {
        value: fb.block_params(result_block)[0],
        ownership: ValueOwnership::Immortal,
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_exact_int_expr_pyobject_result(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    if !matches!(expr, InstrTyped::BinOp(_)) {
        return Ok(None);
    }
    let ResultDemand::PyObject { borrowed_ok: false } = demand else {
        return Ok(None);
    };
    let Some(plan) = expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
    else {
        return Ok(None);
    };
    let result = if planning::exact_int_return_plan_immortal_pyobject_result(plan).is_some()
        && opt_v3_exact_int_deopt_miss_target_available(plan.instr_id, emit_ctx)
    {
        emit_opt_v3_exact_int_return_deopt_immortal_pyobject_selection(
            fb,
            plan.instr_id,
            plan.into(),
            emit_ctx.indexed_field_guards_by_instr,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?
    } else {
        emit_opt_v3_exact_int_return_selection(
            fb,
            plan.instr_id,
            plan.into(),
            emit_ctx.indexed_field_guards_by_instr,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )?
    };
    if !result.ownership.can_satisfy_pyobject_demand(demand) {
        return Err(format!(
            "optimizer v3 expression result for {} produced {:?}, but demand is {demand:?}",
            plan.instr_id, result.ownership
        ));
    }
    let facts =
        py_facts_for_typed_expr_with_local_env(expr, local_env).unwrap_or_else(PyObjFacts::unknown);
    Ok(Some(EmitResult::pyobject(
        result.value,
        result.ownership,
        facts,
    )))
}

fn emit_opt_v3_exact_int_branch_selection(
    fb: &mut FunctionBuilder<'_>,
    test_instr_id: InstrId,
    selection: ExactIntBranchEmissionSelection<'_>,
    indexed_field_guards_by_instr: &HashMap<InstrId, Vec<TypedIndexedFieldGuard>>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let result_block = fb.create_block();
    fb.append_block_param(result_block, emit_ctx.consts.i32_ty);
    let fallback_block = fb.create_block();
    fb.set_cold_block(fallback_block);

    let mut hot_values = opt_v3_region_input_values(
        fb,
        selection.hot_plan,
        local_env,
        emit_ctx,
        codegen_env,
        indexed_field_guards_by_instr,
        Some(fallback_block),
        "exact-int branch hot region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.hot_region,
        &mut hot_values.values,
        &hot_values.preseeded_scalars,
        &hot_values.preseeded_convert_inputs,
        Some(fallback_block),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let hot_condition =
        opt_v3_region_branch_condition(selection.hot_region, &hot_values.values, test_instr_id)?;
    emit_opt_v3_release_owned_values_except(fb, &hot_values.values, None, emit_ctx);
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(hot_condition)]);

    fb.switch_to_block(fallback_block);
    let mut fallback_values = opt_v3_region_input_values(
        fb,
        selection.fallback_plan,
        local_env,
        emit_ctx,
        codegen_env,
        indexed_field_guards_by_instr,
        None,
        "exact-int branch fallback region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.fallback_region,
        &mut fallback_values.values,
        &fallback_values.preseeded_scalars,
        &fallback_values.preseeded_convert_inputs,
        None,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let fallback_condition = opt_v3_region_branch_condition(
        selection.fallback_region,
        &fallback_values.values,
        test_instr_id,
    )?;
    emit_opt_v3_release_owned_values_except(fb, &fallback_values.values, None, emit_ctx);
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_condition)]);

    fb.switch_to_block(result_block);
    Ok(fb.block_params(result_block)[0])
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_exact_int_return_i32_bool01_selection(
    fb: &mut FunctionBuilder<'_>,
    value_instr_id: InstrId,
    selection: ExactIntReturnEmissionSelection<'_>,
    indexed_field_guards_by_instr: &HashMap<InstrId, Vec<TypedIndexedFieldGuard>>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let result_block = fb.create_block();
    fb.append_block_param(result_block, emit_ctx.consts.i32_ty);
    let fallback_block = fb.create_block();
    fb.set_cold_block(fallback_block);

    let mut hot_skipped_outputs = HashSet::new();
    hot_skipped_outputs.insert(opt_v3_region_return_value(
        selection.hot_region,
        value_instr_id,
    )?);
    let mut hot_values = opt_v3_region_input_values(
        fb,
        selection.hot_plan,
        local_env,
        emit_ctx,
        codegen_env,
        indexed_field_guards_by_instr,
        Some(fallback_block),
        "exact-int bool expression hot region",
    )?;
    emit_opt_v3_mechanical_region_steps_with_skipped_outputs(
        fb,
        selection.hot_region,
        &mut hot_values.values,
        &hot_values.preseeded_scalars,
        &hot_values.preseeded_convert_inputs,
        Some(fallback_block),
        &hot_skipped_outputs,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let hot_result = opt_v3_region_return_materialized_i32_bool01(
        selection.hot_region,
        &hot_values.values,
        value_instr_id,
    )?;
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(hot_result)]);

    fb.switch_to_block(fallback_block);
    let mut fallback_values = opt_v3_region_input_values(
        fb,
        selection.fallback_plan,
        local_env,
        emit_ctx,
        codegen_env,
        indexed_field_guards_by_instr,
        None,
        "exact-int bool expression fallback region",
    )?;
    emit_opt_v3_mechanical_region_steps_with_skipped_outputs(
        fb,
        selection.fallback_region,
        &mut fallback_values.values,
        &fallback_values.preseeded_scalars,
        &fallback_values.preseeded_convert_inputs,
        None,
        &HashSet::new(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let fallback_result = opt_v3_region_return_pyobject(
        selection.fallback_region,
        &fallback_values.values,
        value_instr_id,
    )?;
    let is_true_ref = func_imports.get(codegen_env, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?;
    let fallback_result = emit_truthy_from_pyobject_value(
        fb,
        fallback_result.value,
        PyObjFacts::unknown(),
        is_true_ref,
        emit_ctx,
        fallback_result.ownership.is_owned(),
    )
    .expect_i32_bool01("exact-int bool fallback truthiness");
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_result)]);

    fb.switch_to_block(result_block);
    Ok(fb.block_params(result_block)[0])
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_exact_int_return_selection(
    fb: &mut FunctionBuilder<'_>,
    value_instr_id: InstrId,
    selection: ExactIntReturnEmissionSelection<'_>,
    indexed_field_guards_by_instr: &HashMap<InstrId, Vec<TypedIndexedFieldGuard>>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<OptV3PyObjectResult, String> {
    let result_block = fb.create_block();
    fb.append_block_param(result_block, emit_ctx.consts.ptr_ty);
    let fallback_block = fb.create_block();
    fb.set_cold_block(fallback_block);

    let mut hot_values = opt_v3_region_input_values(
        fb,
        selection.hot_plan,
        local_env,
        emit_ctx,
        codegen_env,
        indexed_field_guards_by_instr,
        Some(fallback_block),
        "exact-int return hot region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.hot_region,
        &mut hot_values.values,
        &hot_values.preseeded_scalars,
        &hot_values.preseeded_convert_inputs,
        Some(fallback_block),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let hot_result =
        opt_v3_region_return_pyobject(selection.hot_region, &hot_values.values, value_instr_id)?;
    emit_opt_v3_release_owned_values_except(
        fb,
        &hot_values.values,
        Some(hot_result.value),
        emit_ctx,
    );
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(hot_result.value)]);

    fb.switch_to_block(fallback_block);
    let mut fallback_values = opt_v3_region_input_values(
        fb,
        selection.fallback_plan,
        local_env,
        emit_ctx,
        codegen_env,
        indexed_field_guards_by_instr,
        None,
        "exact-int return fallback region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.fallback_region,
        &mut fallback_values.values,
        &fallback_values.preseeded_scalars,
        &fallback_values.preseeded_convert_inputs,
        None,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let fallback_result = opt_v3_region_return_pyobject(
        selection.fallback_region,
        &fallback_values.values,
        value_instr_id,
    )?;
    emit_opt_v3_release_owned_values_except(
        fb,
        &fallback_values.values,
        Some(fallback_result.value),
        emit_ctx,
    );
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_result.value)]);

    fb.switch_to_block(result_block);
    Ok(OptV3PyObjectResult {
        value: fb.block_params(result_block)[0],
        ownership: merge_opt_v3_pyobject_ownership(hot_result.ownership, fallback_result.ownership),
    })
}

fn emit_opt_v3_release_owned_values_except(
    fb: &mut FunctionBuilder<'_>,
    values: &HashMap<PlanValue, OptV3MechanicalValue>,
    keep_value: Option<ir::Value>,
    emit_ctx: &JitEmitCtx<'_>,
) {
    let mut released = HashSet::new();
    for value in values.values().copied() {
        let OptV3MechanicalValue::PyObject { value, owned: true } = value else {
            continue;
        };
        if keep_value == Some(value) || !released.insert(value) {
            continue;
        }
        emit_release_owned_pyobject(fb, value, None, emit_ctx);
    }
}

fn opt_v3_indexed_field_receiver_value(
    fb: &mut FunctionBuilder<'_>,
    receiver: MechanicalIndexedFieldReceiverSource<'_>,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    context: &str,
) -> Result<ir::Value, String> {
    match receiver {
        MechanicalIndexedFieldReceiverSource::LocalName { name } => local_env
            .load_name(fb, name, emit_ctx, true)
            .ok_or_else(|| {
                format!(
                    "optimizer v3 {context} indexed-field receiver references unavailable local {name:?}"
                )
            }),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_borrowed_indexed_field_input(
    fb: &mut FunctionBuilder<'_>,
    source: InstrId,
    receiver: MechanicalIndexedFieldReceiverSource<'_>,
    owner_type: &IndexedFieldOwnerType,
    attr_name: &str,
    expected_index: u32,
    fallback_block: ir::Block,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    indexed_field_guards_by_instr: &HashMap<InstrId, Vec<TypedIndexedFieldGuard>>,
    context: &str,
) -> Result<ir::Value, String> {
    let specializations = if let Some(guards) = indexed_field_guards_by_instr
        .get(&source)
        .map(Vec::as_slice)
    {
        let Some(plan) = IndexedFieldLoweringPlan::for_access(
            source,
            TypedIndexedFieldPlanSource::OptimizationPlanV3,
            guards,
            PlanV3IndexedFieldAccessKind::Load,
        )?
        else {
            return Ok(emit_opt_v3_indexed_field_input_fallback_value(
                fb,
                source,
                fallback_block,
                emit_ctx,
            ));
        };
        plan.specializations
    } else {
        return Err(format!(
            "optimizer v3 {context} borrowed indexed-field input {source} for {}.{}[{}] in function {} reached codegen without typed guards",
            owner_type.qualname, attr_name, expected_index, emit_ctx.function_id
        ));
    };

    let receiver = opt_v3_indexed_field_receiver_value(fb, receiver, local_env, emit_ctx, context)?;
    let mut emittable_specializations = Vec::with_capacity(specializations.len());
    for specialization in specializations {
        let Some(owner_type_ptr) =
            emit_type_ptr_value_for_ref(fb, codegen_env, emit_ctx, &specialization.owner_type_ref)
                .map_err(|err| {
                    format!(
                        "failed to bind opt-v3 indexed-field input owner type for {source}: {err}"
                    )
                })?
        else {
            continue;
        };
        emittable_specializations.push((specialization, owner_type_ptr));
    }
    if emittable_specializations.is_empty() {
        return Ok(emit_opt_v3_indexed_field_input_fallback_value(
            fb,
            source,
            fallback_block,
            emit_ctx,
        ));
    };

    let result_block = fb.create_block();
    fb.append_block_param(result_block, emit_ctx.consts.ptr_ty);
    let miss_block = fb.create_block();
    fb.set_cold_block(miss_block);

    let specialization_count = emittable_specializations.len();
    for (index, (specialization, owner_type_ptr)) in
        emittable_specializations.into_iter().enumerate()
    {
        let maybe_direct_block = fb.create_block();
        let direct_block = fb.create_block();
        fb.append_block_param(direct_block, emit_ctx.consts.ptr_ty);
        let next_guard_block = if index + 1 == specialization_count {
            miss_block
        } else {
            fb.create_block()
        };
        let type_matches = emit_exact_type_version_match(
            fb,
            receiver,
            owner_type_ptr,
            specialization.type_version,
        );
        fb.ins()
            .brif(type_matches, maybe_direct_block, &[], next_guard_block, &[]);

        fb.switch_to_block(maybe_direct_block);
        emit_trusted_inline_values_field_probe(
            fb,
            receiver,
            owner_type_ptr,
            specialization.expected_index,
            direct_block,
            miss_block,
            emit_ctx,
        )?;

        fb.switch_to_block(direct_block);
        let direct_value = fb.block_params(direct_block)[0];
        emit_optional_counter_increment_for_kind(
            fb,
            emit_ctx,
            emit_ctx.field_indexed_hit_counter_ids,
            source,
        );
        fb.ins()
            .jump(result_block, &[ir::BlockArg::Value(direct_value)]);

        if index + 1 != specialization_count {
            fb.switch_to_block(next_guard_block);
        }
    }

    fb.switch_to_block(miss_block);
    emit_optional_counter_increment_for_kind(
        fb,
        emit_ctx,
        emit_ctx.field_indexed_fallback_counter_ids,
        source,
    );
    fb.ins().jump(fallback_block, &[]);

    fb.switch_to_block(result_block);
    Ok(fb.block_params(result_block)[0])
}

fn emit_opt_v3_indexed_field_input_fallback_value(
    fb: &mut FunctionBuilder<'_>,
    source: InstrId,
    fallback_block: ir::Block,
    emit_ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    emit_optional_counter_increment_for_kind(
        fb,
        emit_ctx,
        emit_ctx.field_indexed_fallback_counter_ids,
        source,
    );
    fb.ins().jump(fallback_block, &[]);
    let dead_block = fb.create_block();
    fb.switch_to_block(dead_block);
    fb.ins().iconst(emit_ctx.consts.ptr_ty, 0)
}

fn emit_opt_v3_owned_indexed_field_input(
    fb: &mut FunctionBuilder<'_>,
    receiver: MechanicalIndexedFieldReceiverSource<'_>,
    attr_name: &str,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    context: &str,
) -> Result<ir::Value, String> {
    let receiver = opt_v3_indexed_field_receiver_value(fb, receiver, local_env, emit_ctx, context)?;
    let attr = emit_owned_module_constant(
        fb,
        emit_ctx
            .module_constants
            .require_unicode_constant_id(attr_name),
        emit_ctx,
    );
    let getattr_inst = fb
        .ins()
        .call(emit_ctx.pyobject_getattr_ref, &[receiver, attr]);
    let value = fb.inst_results(getattr_inst)[0];
    Ok(emit_checked_owned_pyobject_result(fb, value, emit_ctx))
}

fn opt_v3_region_input_values(
    fb: &mut FunctionBuilder<'_>,
    region: &RegionPlan,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    indexed_field_guards_by_instr: &HashMap<InstrId, Vec<TypedIndexedFieldGuard>>,
    local_fallback_block: Option<ir::Block>,
    context: &str,
) -> Result<OptV3RegionInputValues, String> {
    let mut values = HashMap::new();
    let mut preseeded_scalars = HashSet::new();
    let mut preseeded_convert_inputs = HashSet::new();
    for input in opt_v3_mechanical_region_inputs(region, context)? {
        let value = match input.source {
            MechanicalRegionInputSource::FunctionParam { name } if input.value.rep == Rep::I64 => {
                let (value, _) = local_env.scalar_i64_value_for_name(name).ok_or_else(|| {
                    format!(
                        "optimizer v3 {context} input {:?} references unavailable scalar local {:?}",
                        input.value, name
                    )
                })?;
                OptV3MechanicalValue::I64(value)
            }
            MechanicalRegionInputSource::FunctionParam { name }
                if input.value.rep == Rep::PyObjectBorrowed =>
            {
                if let Some((value, facts)) = local_env.scalar_i64_value_for_name(name) {
                    let outputs = opt_v3_i64_convert_outputs_for_input(region, input.value);
                    if !outputs.is_empty() {
                        preseeded_convert_inputs.insert(input.value);
                        for output in outputs {
                            opt_v3_store_mechanical_value(
                                &mut values,
                                output,
                                OptV3MechanicalValue::I64(value),
                            )?;
                            preseeded_scalars.insert(output);
                        }
                        continue;
                    }
                    let result = emit_soac_value_result_for_demand(
                        fb,
                        SoacValue::i64(value, facts),
                        emit_ctx,
                        ResultDemand::PYOBJECT_OWNED,
                        None,
                    );
                    let (value, ownership, _) =
                        result.expect_pyobject("scalar opt-v3 PyObject input materialization");
                    OptV3MechanicalValue::PyObject {
                        value,
                        owned: ownership.is_owned(),
                    }
                } else {
                    let value = local_env
                        .load_name(fb, name, emit_ctx, true)
                        .ok_or_else(|| {
                            format!(
                                "optimizer v3 {context} input {:?} references unavailable local {:?}",
                                input.value, name
                            )
                        })?;
                    OptV3MechanicalValue::PyObject {
                        value,
                        owned: false,
                    }
                }
            }
            MechanicalRegionInputSource::FunctionParam { name } => {
                return Err(format!(
                    "optimizer v3 {context} function-param input {:?} for {name:?} has unsupported rep {:?}",
                    input.value, input.value.rep
                ));
            }
            MechanicalRegionInputSource::ModuleConstant { index } => {
                OptV3MechanicalValue::PyObject {
                    value: emit_owned_module_constant(
                        fb,
                        ModuleConstantId(index as usize),
                        emit_ctx,
                    ),
                    owned: input.value.rep == Rep::PyObjectOwned,
                }
            }
            MechanicalRegionInputSource::IndexedGlobal {
                source,
                module_name: _,
                name,
                expected_index,
            } if input.value.rep == Rep::PyObjectBorrowed => {
                let fallback_block = local_fallback_block.ok_or_else(|| {
                    format!(
                        "optimizer v3 {context} borrowed indexed-global input {:?} needs a local fallback block",
                        input.value
                    )
                })?;
                OptV3MechanicalValue::PyObject {
                    value: emit_borrowed_planned_indexed_global_load(
                        fb,
                        emit_ctx.consts.block_const,
                        name,
                        expected_index,
                        source,
                        fallback_block,
                        emit_ctx,
                    ),
                    owned: false,
                }
            }
            MechanicalRegionInputSource::IndexedGlobal {
                source,
                module_name: _,
                name,
                expected_index,
            } if input.value.rep == Rep::PyObjectOwned => OptV3MechanicalValue::PyObject {
                value: emit_planned_indexed_global_load(
                    fb,
                    emit_ctx.consts.block_const,
                    name,
                    expected_index,
                    source,
                    local_env,
                    emit_ctx,
                ),
                owned: true,
            },
            MechanicalRegionInputSource::IndexedField {
                source,
                receiver,
                owner_type,
                attr_name,
                expected_index,
            } if input.value.rep == Rep::PyObjectBorrowed => {
                let fallback_block = local_fallback_block.ok_or_else(|| {
                    format!(
                        "optimizer v3 {context} borrowed indexed-field input {:?} needs a local fallback block",
                        input.value
                    )
                })?;
                OptV3MechanicalValue::PyObject {
                    value: emit_opt_v3_borrowed_indexed_field_input(
                        fb,
                        source,
                        receiver,
                        owner_type,
                        attr_name,
                        expected_index,
                        fallback_block,
                        local_env,
                        emit_ctx,
                        codegen_env,
                        indexed_field_guards_by_instr,
                        context,
                    )?,
                    owned: false,
                }
            }
            MechanicalRegionInputSource::IndexedField {
                receiver,
                attr_name,
                ..
            } if input.value.rep == Rep::PyObjectOwned => OptV3MechanicalValue::PyObject {
                value: emit_opt_v3_owned_indexed_field_input(
                    fb, receiver, attr_name, local_env, emit_ctx, context,
                )?,
                owned: true,
            },
            MechanicalRegionInputSource::IndexedField { .. } => {
                return Err(format!(
                    "optimizer v3 {context} indexed-field input {:?} has unsupported rep {:?}",
                    input.value, input.value.rep
                ));
            }
            MechanicalRegionInputSource::IndexedGlobal { .. } => {
                return Err(format!(
                    "optimizer v3 {context} indexed-global input {:?} has unsupported rep {:?}",
                    input.value, input.value.rep
                ));
            }
        };
        opt_v3_store_mechanical_value(&mut values, input.value, value)?;
    }
    Ok(OptV3RegionInputValues {
        values,
        preseeded_scalars,
        preseeded_convert_inputs,
    })
}

fn opt_v3_i64_convert_outputs_for_input(region: &RegionPlan, input: PlanValue) -> Vec<PlanValue> {
    region
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            PlanNodeKind::Convert(convert)
                if convert.kind == ConversionKind::FromPythonLongCompactToI64
                    && convert.input == input
                    && convert.output.rep == Rep::I64 =>
            {
                Some(convert.output)
            }
            _ => None,
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_mechanical_region_steps(
    fb: &mut FunctionBuilder<'_>,
    region: &MechanicalRegionEmission,
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    preseeded_scalars: &HashSet<PlanValue>,
    preseeded_convert_inputs: &HashSet<PlanValue>,
    local_fallback_block: Option<ir::Block>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    let skipped_outputs = HashSet::new();
    emit_opt_v3_mechanical_region_steps_with_skipped_outputs(
        fb,
        region,
        values,
        preseeded_scalars,
        preseeded_convert_inputs,
        local_fallback_block,
        &skipped_outputs,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_mechanical_region_steps_with_skipped_outputs(
    fb: &mut FunctionBuilder<'_>,
    region: &MechanicalRegionEmission,
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    preseeded_scalars: &HashSet<PlanValue>,
    preseeded_convert_inputs: &HashSet<PlanValue>,
    local_fallback_block: Option<ir::Block>,
    skipped_outputs: &HashSet<PlanValue>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    for step in &region.steps {
        match opt_v3_mechanical_codegen_step(
            region.region,
            step,
            local_fallback_block.is_some(),
            preseeded_scalars,
            preseeded_convert_inputs,
        )? {
            MechanicalCodegenStep::Input { output } => {
                if preseeded_convert_inputs.contains(&output) {
                    continue;
                }
                if !values.contains_key(&output) {
                    return Err(format!(
                        "optimizer v3 region {:?} input node {:?} references missing value {:?}",
                        region.region, step.node, output
                    ));
                }
            }
            MechanicalCodegenStep::ConstantI64 { output, value } => {
                let value =
                    OptV3MechanicalValue::I64(fb.ins().iconst(emit_ctx.consts.i64_ty, value));
                opt_v3_store_mechanical_value(values, output, value)?;
            }
            MechanicalCodegenStep::SpecializationGuard { .. } => {}
            MechanicalCodegenStep::PreseededConvert { output } => {
                if !values.contains_key(&output) {
                    return Err(format!(
                        "optimizer v3 region {:?} preseeded conversion node {:?} references missing value {:?}",
                        region.region, step.node, output
                    ));
                }
            }
            MechanicalCodegenStep::Convert {
                kind,
                input,
                output,
            } => {
                emit_opt_v3_mechanical_convert(
                    fb,
                    region.region,
                    step.node,
                    kind,
                    input,
                    output,
                    values,
                    local_fallback_block,
                    local_env,
                    emit_ctx,
                    codegen_env,
                    func_imports,
                )?;
            }
            MechanicalCodegenStep::Operation { op, inputs, output } => {
                emit_opt_v3_mechanical_operation(
                    fb,
                    region.region,
                    step.node,
                    op,
                    inputs,
                    output,
                    values,
                    local_fallback_block,
                    emit_ctx,
                    codegen_env,
                    func_imports,
                )?;
            }
            MechanicalCodegenStep::Materialize {
                kind,
                input,
                output,
            } => {
                if skipped_outputs.contains(&output) {
                    continue;
                }
                emit_opt_v3_mechanical_materialize(
                    fb,
                    region.region,
                    step.node,
                    kind,
                    input,
                    output,
                    values,
                    emit_ctx,
                )?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_mechanical_convert(
    fb: &mut FunctionBuilder<'_>,
    region: RegionId,
    node: PlanNodeId,
    kind: MechanicalCodegenConversion,
    input: PlanValue,
    output: PlanValue,
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    local_fallback_block: Option<ir::Block>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    match kind {
        MechanicalCodegenConversion::FromPythonLongCompactToI64 => {
            let fallback_block = local_fallback_block.ok_or_else(|| {
                format!(
                    "optimizer v3 region {region:?} conversion node {node:?} needs a local fallback block"
                )
            })?;
            let (value, owned) = opt_v3_pyobject_value(values, input)?;
            if owned {
                return Err(format!(
                    "optimizer v3 region {region:?} conversion node {node:?} expected borrowed PyObject input"
                ));
            }
            let converted = {
                let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
                    fb,
                    local_env,
                    ctx: emit_ctx,
                    codegen_env,
                    func_imports,
                    owned_transfer_temp_load: None,
                };
                intrinsics::emit_v3_guarded_compact_long_i64(
                    &mut intrinsic_state,
                    value,
                    fallback_block,
                )
            };
            opt_v3_store_mechanical_value(values, output, OptV3MechanicalValue::I64(converted))
        }
        MechanicalCodegenConversion::TruthinessToI32Bool01 => {
            let (value, owned) = opt_v3_take_pyobject_value(values, input)?;
            let is_true_ref =
                func_imports.get(codegen_env, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?;
            let truth = emit_truthy_from_pyobject_value(
                fb,
                value,
                PyObjFacts::unknown(),
                is_true_ref,
                emit_ctx,
                owned,
            );
            let truth_i32 = truth.expect_i32_bool01("optimizer v3 truthiness conversion");
            opt_v3_store_mechanical_value(
                values,
                output,
                OptV3MechanicalValue::I32Bool01(truth_i32),
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_mechanical_operation(
    fb: &mut FunctionBuilder<'_>,
    region: RegionId,
    node: PlanNodeId,
    op: MechanicalCodegenOperation,
    inputs: [PlanValue; 2],
    output: PlanValue,
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    local_fallback_block: Option<ir::Block>,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    match op {
        MechanicalCodegenOperation::PyNumberAdd
        | MechanicalCodegenOperation::PyNumberSubtract
        | MechanicalCodegenOperation::PyNumberMultiply
        | MechanicalCodegenOperation::PyNumberBitAnd
        | MechanicalCodegenOperation::PyNumberBitOr
        | MechanicalCodegenOperation::PyNumberBitXor => {
            let import = match op {
                MechanicalCodegenOperation::PyNumberAdd => &PYNUMBER_ADD_IMPORT,
                MechanicalCodegenOperation::PyNumberSubtract => &PYNUMBER_SUBTRACT_IMPORT,
                MechanicalCodegenOperation::PyNumberMultiply => &PYNUMBER_MULTIPLY_IMPORT,
                MechanicalCodegenOperation::PyNumberBitAnd => &PYNUMBER_AND_IMPORT,
                MechanicalCodegenOperation::PyNumberBitOr => &PYNUMBER_OR_IMPORT,
                MechanicalCodegenOperation::PyNumberBitXor => &PYNUMBER_XOR_IMPORT,
                _ => unreachable!("matched PyNumber binary operation"),
            };
            let args = opt_v3_take_pyobject_call_args(values, inputs.as_slice())?;
            let arg_values = args.iter().map(|(value, _)| *value).collect::<Vec<_>>();
            let owned_inputs = args
                .iter()
                .filter_map(|(value, owned)| (*owned).then_some(*value))
                .collect::<Vec<_>>();
            let operation_ref = func_imports.get(codegen_env, &mut fb.func, import)?;
            let result = emit_checked_owned_pyobject_call_with_cleanup(
                fb,
                emit_ctx,
                operation_ref,
                arg_values.as_slice(),
                owned_inputs.as_slice(),
            );
            opt_v3_store_mechanical_value(
                values,
                output,
                OptV3MechanicalValue::PyObject {
                    value: result,
                    owned: true,
                },
            )
        }
        MechanicalCodegenOperation::PyObjectRichCompare { op } => {
            let args = opt_v3_take_pyobject_call_args(values, inputs.as_slice())?;
            let mut arg_values = args.iter().map(|(value, _)| *value).collect::<Vec<_>>();
            let owned_inputs = args
                .iter()
                .filter_map(|(value, owned)| (*owned).then_some(*value))
                .collect::<Vec<_>>();
            let compare_op = fb.ins().iconst(
                emit_ctx.consts.i32_ty,
                i64::from(opt_v3_rich_compare_op_code(op)),
            );
            arg_values.push(compare_op);
            let compare_ref =
                func_imports.get(codegen_env, &mut fb.func, &PYOBJECT_RICHCOMPARE_IMPORT)?;
            let result = emit_checked_owned_pyobject_call_with_cleanup(
                fb,
                emit_ctx,
                compare_ref,
                arg_values.as_slice(),
                owned_inputs.as_slice(),
            );
            opt_v3_store_mechanical_value(
                values,
                output,
                OptV3MechanicalValue::PyObject {
                    value: result,
                    owned: true,
                },
            )
        }
        MechanicalCodegenOperation::PyObjectRichCompareBool { op } => {
            let fallback_block = local_fallback_block.ok_or_else(|| {
                format!(
                    "optimizer v3 region {region:?} node {node:?} exact unicode compare needs a local fallback block"
                )
            })?;
            let (lhs, lhs_owned) = opt_v3_pyobject_value(values, inputs[0])?;
            let (rhs, rhs_owned) = opt_v3_pyobject_value(values, inputs[1])?;
            if lhs_owned || rhs_owned {
                return Err(format!(
                    "optimizer v3 region {region:?} node {node:?} exact unicode compare expected borrowed inputs"
                ));
            }
            emit_exact_cpython_type_guard(
                fb,
                lhs,
                RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Unicode),
                fallback_block,
                emit_ctx,
                codegen_env,
            )?;
            emit_exact_cpython_type_guard(
                fb,
                rhs,
                RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Unicode),
                fallback_block,
                emit_ctx,
                codegen_env,
            )?;
            let result = emit_opt_v3_exact_unicode_compare_bool(
                fb,
                emit_ctx,
                codegen_env,
                func_imports,
                lhs,
                rhs,
                op,
            )?;
            opt_v3_store_mechanical_value(values, output, OptV3MechanicalValue::I32Bool01(result))
        }
        MechanicalCodegenOperation::CheckedI64Add
        | MechanicalCodegenOperation::CheckedI64Sub
        | MechanicalCodegenOperation::CheckedI64Mul => {
            let op_name = match op {
                MechanicalCodegenOperation::CheckedI64Add => "CheckedI64Add",
                MechanicalCodegenOperation::CheckedI64Sub => "CheckedI64Sub",
                MechanicalCodegenOperation::CheckedI64Mul => "CheckedI64Mul",
                _ => unreachable!("matched checked i64 arithmetic operation"),
            };
            let fallback_block = local_fallback_block.ok_or_else(|| {
                format!(
                    "optimizer v3 region {region:?} node {node:?} {op_name} needs a local fallback block"
                )
            })?;
            let lhs = opt_v3_i64_value(values, inputs[0])?;
            let rhs = opt_v3_i64_value(values, inputs[1])?;
            let (result, overflow) = match op {
                MechanicalCodegenOperation::CheckedI64Add => fb.ins().sadd_overflow(lhs, rhs),
                MechanicalCodegenOperation::CheckedI64Sub => fb.ins().ssub_overflow(lhs, rhs),
                MechanicalCodegenOperation::CheckedI64Mul => fb.ins().smul_overflow(lhs, rhs),
                _ => unreachable!("matched checked i64 arithmetic operation"),
            };
            let ok_block = fb.create_block();
            fb.append_block_param(ok_block, emit_ctx.consts.i64_ty);
            fb.ins().brif(
                overflow,
                fallback_block,
                &[],
                ok_block,
                &[ir::BlockArg::Value(result)],
            );
            fb.switch_to_block(ok_block);
            let result = fb.block_params(ok_block)[0];
            opt_v3_store_mechanical_value(values, output, OptV3MechanicalValue::I64(result))
        }
        MechanicalCodegenOperation::I64BitAnd
        | MechanicalCodegenOperation::I64BitOr
        | MechanicalCodegenOperation::I64BitXor => {
            let lhs = opt_v3_i64_value(values, inputs[0])?;
            let rhs = opt_v3_i64_value(values, inputs[1])?;
            let result = match op {
                MechanicalCodegenOperation::I64BitAnd => fb.ins().band(lhs, rhs),
                MechanicalCodegenOperation::I64BitOr => fb.ins().bor(lhs, rhs),
                MechanicalCodegenOperation::I64BitXor => fb.ins().bxor(lhs, rhs),
                _ => unreachable!("matched i64 bitwise operation"),
            };
            opt_v3_store_mechanical_value(values, output, OptV3MechanicalValue::I64(result))
        }
        MechanicalCodegenOperation::I64CompareToBool01 { op } => {
            let lhs = opt_v3_i64_value(values, inputs[0])?;
            let rhs = opt_v3_i64_value(values, inputs[1])?;
            let cond = fb.ins().icmp(opt_v3_rich_compare_intcc(op), lhs, rhs);
            let zero = fb.ins().iconst(emit_ctx.consts.i32_ty, 0);
            let one = fb.ins().iconst(emit_ctx.consts.i32_ty, 1);
            let value = fb.ins().select(cond, one, zero);
            opt_v3_store_mechanical_value(values, output, OptV3MechanicalValue::I32Bool01(value))
        }
    }
}

fn emit_opt_v3_mechanical_materialize(
    fb: &mut FunctionBuilder<'_>,
    region: RegionId,
    node: PlanNodeId,
    kind: MaterializeKind,
    input: PlanValue,
    output: PlanValue,
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    emit_ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    match kind {
        MaterializeKind::PythonLong => {
            let value = opt_v3_i64_value(values, input)?;
            let result = emit_checked_owned_pyobject_call_with_cleanup(
                fb,
                emit_ctx,
                emit_ctx.py_long_from_i64_ref,
                &[value],
                &[],
            );
            opt_v3_store_mechanical_value(
                values,
                output,
                OptV3MechanicalValue::PyObject {
                    value: result,
                    owned: true,
                },
            )
        }
        MaterializeKind::PythonBool => {
            let value = opt_v3_i32_bool01_value(values, input)?;
            let (result, ownership, facts) =
                emit_to_python_bool(fb, SoacValue::i32(value, IntFacts::i32_bool01()), emit_ctx)
                    .expect_pyobject("optimizer v3 bool materialize");
            if !matches!(ownership, ValueOwnership::Immortal) || !facts.is_immortal() {
                return Err(format!(
                    "optimizer v3 region {region:?} materialize node {node:?} expected immortal bool materialization"
                ));
            }
            opt_v3_store_mechanical_value(
                values,
                output,
                OptV3MechanicalValue::PyObject {
                    value: result,
                    owned: false,
                },
            )
        }
    }
}

fn opt_v3_store_mechanical_value(
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    key: PlanValue,
    value: OptV3MechanicalValue,
) -> Result<(), String> {
    if !value.matches_rep(key.rep) {
        return Err(format!(
            "optimizer v3 mechanical value for {:?} does not match expected rep {:?}",
            key.id, key.rep
        ));
    }
    if values.insert(key, value).is_some() {
        return Err(format!(
            "optimizer v3 mechanical value {:?} was produced more than once",
            key.id
        ));
    }
    Ok(())
}

fn opt_v3_pyobject_value(
    values: &HashMap<PlanValue, OptV3MechanicalValue>,
    key: PlanValue,
) -> Result<(ir::Value, bool), String> {
    match values.get(&key).copied() {
        Some(OptV3MechanicalValue::PyObject { value, owned }) => Ok((value, owned)),
        other => Err(format!(
            "optimizer v3 expected PyObject value {:?}, got {other:?}",
            key.id
        )),
    }
}

fn opt_v3_take_pyobject_value(
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    key: PlanValue,
) -> Result<(ir::Value, bool), String> {
    let (value, owned) = opt_v3_pyobject_value(values, key)?;
    if owned {
        values.remove(&key);
    }
    Ok((value, owned))
}

fn opt_v3_take_pyobject_call_args(
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    inputs: &[PlanValue],
) -> Result<Vec<(ir::Value, bool)>, String> {
    inputs
        .iter()
        .map(|input| opt_v3_take_pyobject_value(values, *input))
        .collect()
}

fn opt_v3_i64_value(
    values: &HashMap<PlanValue, OptV3MechanicalValue>,
    key: PlanValue,
) -> Result<ir::Value, String> {
    match values.get(&key).copied() {
        Some(OptV3MechanicalValue::I64(value)) => Ok(value),
        other => Err(format!(
            "optimizer v3 expected i64 value {:?}, got {other:?}",
            key.id
        )),
    }
}

fn opt_v3_i32_bool01_value(
    values: &HashMap<PlanValue, OptV3MechanicalValue>,
    key: PlanValue,
) -> Result<ir::Value, String> {
    match values.get(&key).copied() {
        Some(OptV3MechanicalValue::I32Bool01(value)) => Ok(value),
        other => Err(format!(
            "optimizer v3 expected i32 bool01 value {:?}, got {other:?}",
            key.id
        )),
    }
}

fn opt_v3_region_branch_condition(
    region: &MechanicalRegionEmission,
    values: &HashMap<PlanValue, OptV3MechanicalValue>,
    source: InstrId,
) -> Result<ir::Value, String> {
    let exit = region.exits.first().ok_or_else(|| {
        format!(
            "optimizer v3 region {:?} for source {source} has no exit",
            region.region
        )
    })?;
    let MechanicalExitKind::Branch { condition, .. } = &exit.kind else {
        return Err(format!(
            "optimizer v3 region {:?} for source {source} does not end in a branch",
            region.region
        ));
    };
    opt_v3_i32_bool01_value(values, *condition)
}

fn opt_v3_region_return_value(
    region: &MechanicalRegionEmission,
    source: InstrId,
) -> Result<PlanValue, String> {
    let exit = region.exits.first().ok_or_else(|| {
        format!(
            "optimizer v3 region {:?} for source {source} has no exit",
            region.region
        )
    })?;
    let MechanicalExitKind::Return { value } = exit.kind else {
        return Err(format!(
            "optimizer v3 region {:?} for source {source} does not end in a return",
            region.region
        ));
    };
    Ok(value)
}

fn opt_v3_region_return_materialized_i32_bool01(
    region: &MechanicalRegionEmission,
    values: &HashMap<PlanValue, OptV3MechanicalValue>,
    source: InstrId,
) -> Result<ir::Value, String> {
    let return_value = opt_v3_region_return_value(region, source)?;
    let input = region.steps.iter().find_map(|step| match step.op {
        MechanicalStepOp::Materialize {
            kind: MaterializeKind::PythonBool,
            input,
            output,
        } if output == return_value && input.rep == Rep::I32Bool01 => Some(input),
        _ => None,
    });
    let Some(input) = input else {
        return Err(format!(
            "optimizer v3 region {:?} for source {source} does not return a materialized i32 bool",
            region.region
        ));
    };
    opt_v3_i32_bool01_value(values, input)
}

fn opt_v3_region_return_pyobject(
    region: &MechanicalRegionEmission,
    values: &HashMap<PlanValue, OptV3MechanicalValue>,
    source: InstrId,
) -> Result<OptV3PyObjectResult, String> {
    let exit = region.exits.first().ok_or_else(|| {
        format!(
            "optimizer v3 region {:?} for source {source} has no exit",
            region.region
        )
    })?;
    let MechanicalExitKind::Return { value } = &exit.kind else {
        return Err(format!(
            "optimizer v3 region {:?} for source {source} does not end in a return",
            region.region
        ));
    };
    let (raw_value, owned) = opt_v3_pyobject_value(values, *value)?;
    if !owned && value.rep != Rep::PyObjectImmortal {
        return Err(format!(
            "optimizer v3 region {:?} for source {source} produced borrowed PyObject for return",
            region.region
        ));
    }
    Ok(OptV3PyObjectResult {
        value: raw_value,
        ownership: opt_v3_pyobject_ownership(value.rep, owned)?,
    })
}

fn opt_v3_pyobject_ownership(rep: Rep, owned: bool) -> Result<ValueOwnership, String> {
    match (rep, owned) {
        (Rep::PyObjectOwned, true) => Ok(ValueOwnership::Owned),
        (Rep::PyObjectImmortal, false) => Ok(ValueOwnership::Immortal),
        (Rep::PyObjectImmortal, true) => Ok(ValueOwnership::Owned),
        (Rep::PyObjectBorrowed, false) => Ok(ValueOwnership::Borrowed),
        (Rep::PyObjectBorrowed, true) => {
            Err("optimizer v3 produced an owned value for a borrowed PyObject rep".to_string())
        }
        (Rep::PyObjectOwned, false) => {
            Err("optimizer v3 produced a borrowed value for an owned PyObject rep".to_string())
        }
        (other, _) => Err(format!(
            "optimizer v3 return value has non-PyObject representation {other:?}"
        )),
    }
}

fn merge_opt_v3_pyobject_ownership(
    hot: ValueOwnership,
    fallback: ValueOwnership,
) -> ValueOwnership {
    match (hot, fallback) {
        (ValueOwnership::Immortal, ValueOwnership::Immortal) => ValueOwnership::Immortal,
        (ValueOwnership::Borrowed, ValueOwnership::Borrowed) => ValueOwnership::Borrowed,
        _ => ValueOwnership::Owned,
    }
}

fn opt_v3_rich_compare_intcc(op: RichCompareOp) -> ir::condcodes::IntCC {
    match op {
        RichCompareOp::Eq => ir::condcodes::IntCC::Equal,
        RichCompareOp::Ne => ir::condcodes::IntCC::NotEqual,
        RichCompareOp::Lt => ir::condcodes::IntCC::SignedLessThan,
        RichCompareOp::Le => ir::condcodes::IntCC::SignedLessThanOrEqual,
        RichCompareOp::Gt => ir::condcodes::IntCC::SignedGreaterThan,
        RichCompareOp::Ge => ir::condcodes::IntCC::SignedGreaterThanOrEqual,
    }
}

fn emit_i32_bool01_from_condition(
    fb: &mut FunctionBuilder<'_>,
    emit_ctx: &JitEmitCtx<'_>,
    condition: ir::Value,
) -> ir::Value {
    let zero = fb.ins().iconst(emit_ctx.consts.i32_ty, 0);
    let one = fb.ins().iconst(emit_ctx.consts.i32_ty, 1);
    fb.ins().select(condition, one, zero)
}

fn emit_opt_v3_rich_compare_bool01_from_i32_compare_result(
    fb: &mut FunctionBuilder<'_>,
    emit_ctx: &JitEmitCtx<'_>,
    op: RichCompareOp,
    compare_result: ir::Value,
) -> ir::Value {
    let compare_zero = fb.ins().iconst(emit_ctx.consts.i32_ty, 0);
    let condition = fb
        .ins()
        .icmp(opt_v3_rich_compare_intcc(op), compare_result, compare_zero);
    emit_i32_bool01_from_condition(fb, emit_ctx, condition)
}

fn emit_opt_v3_exact_unicode_compare_bool(
    fb: &mut FunctionBuilder<'_>,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
    op: RichCompareOp,
) -> Result<ir::Value, String> {
    const PY_UNICODE_STATE_COMPACT_MASK: i64 = 1 << 5;
    const PY_UNICODE_STATE_ASCII_MASK: i64 = 1 << 6;
    const PY_UNICODE_COMPACT_ASCII_MASK: i64 =
        PY_UNICODE_STATE_COMPACT_MASK | PY_UNICODE_STATE_ASCII_MASK;
    const PYASCII_LENGTH_OFFSET: i32 = offset_of!(RawPyASCIIObjectForJit, length) as i32;
    const PYASCII_STATE_OFFSET: i32 = offset_of!(RawPyASCIIObjectForJit, state) as i32;
    const PYASCII_DATA_OFFSET: i32 = std::mem::size_of::<RawPyASCIIObjectForJit>() as i32;

    let done_block = fb.create_block();
    let same_object_block = fb.create_block();
    let ascii_probe_block = fb.create_block();
    let ascii_dispatch_block = fb.create_block();
    let ascii_char_compare_block = fb.create_block();
    let ascii_helper_compare_block = fb.create_block();
    let unicode_compare_block = fb.create_block();
    fb.append_block_param(done_block, emit_ctx.consts.i32_ty);

    let same_object = fb.ins().icmp(ir::condcodes::IntCC::Equal, lhs, rhs);
    fb.ins()
        .brif(same_object, same_object_block, &[], ascii_probe_block, &[]);

    fb.switch_to_block(same_object_block);
    let equal_compare_result = fb.ins().iconst(emit_ctx.consts.i32_ty, 0);
    let same_result = emit_opt_v3_rich_compare_bool01_from_i32_compare_result(
        fb,
        emit_ctx,
        op,
        equal_compare_result,
    );
    fb.ins()
        .jump(done_block, &[ir::BlockArg::Value(same_result)]);

    fb.switch_to_block(ascii_probe_block);
    let lhs_len = fb.ins().load(
        emit_ctx.consts.ptr_ty,
        ir::MemFlags::trusted(),
        lhs,
        PYASCII_LENGTH_OFFSET,
    );
    let rhs_len = fb.ins().load(
        emit_ctx.consts.ptr_ty,
        ir::MemFlags::trusted(),
        rhs,
        PYASCII_LENGTH_OFFSET,
    );
    let lhs_len_one = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, lhs_len, 1);
    let rhs_len_one = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, rhs_len, 1);
    let both_len_one = fb.ins().band(lhs_len_one, rhs_len_one);

    let lhs_state = fb.ins().load(
        emit_ctx.consts.i32_ty,
        ir::MemFlags::trusted(),
        lhs,
        PYASCII_STATE_OFFSET,
    );
    let rhs_state = fb.ins().load(
        emit_ctx.consts.i32_ty,
        ir::MemFlags::trusted(),
        rhs,
        PYASCII_STATE_OFFSET,
    );
    let compact_ascii_mask = fb
        .ins()
        .iconst(emit_ctx.consts.i32_ty, PY_UNICODE_COMPACT_ASCII_MASK);
    let lhs_ascii_bits = fb.ins().band(lhs_state, compact_ascii_mask);
    let rhs_ascii_bits = fb.ins().band(rhs_state, compact_ascii_mask);
    let lhs_is_compact_ascii = fb.ins().icmp(
        ir::condcodes::IntCC::Equal,
        lhs_ascii_bits,
        compact_ascii_mask,
    );
    let rhs_is_compact_ascii = fb.ins().icmp(
        ir::condcodes::IntCC::Equal,
        rhs_ascii_bits,
        compact_ascii_mask,
    );
    let both_compact_ascii = fb.ins().band(lhs_is_compact_ascii, rhs_is_compact_ascii);
    fb.ins().brif(
        both_compact_ascii,
        ascii_dispatch_block,
        &[],
        unicode_compare_block,
        &[],
    );

    fb.switch_to_block(ascii_dispatch_block);
    fb.ins().brif(
        both_len_one,
        ascii_char_compare_block,
        &[],
        ascii_helper_compare_block,
        &[],
    );

    fb.switch_to_block(ascii_char_compare_block);
    let lhs_char = fb.ins().load(
        ir::types::I8,
        ir::MemFlags::trusted(),
        lhs,
        PYASCII_DATA_OFFSET,
    );
    let rhs_char = fb.ins().load(
        ir::types::I8,
        ir::MemFlags::trusted(),
        rhs,
        PYASCII_DATA_OFFSET,
    );
    let char_condition = fb
        .ins()
        .icmp(opt_v3_rich_compare_intcc(op), lhs_char, rhs_char);
    let char_result = emit_i32_bool01_from_condition(fb, emit_ctx, char_condition);
    fb.ins()
        .jump(done_block, &[ir::BlockArg::Value(char_result)]);

    fb.switch_to_block(ascii_helper_compare_block);
    let ascii_compare_ref = func_imports.get(
        codegen_env,
        &mut fb.func,
        &SOAC_RUNTIME_COMPARE_COMPACT_ASCII_UNICODE_IMPORT,
    )?;
    let ascii_call = fb.ins().call(ascii_compare_ref, &[lhs, rhs]);
    let ascii_compare_result = fb.inst_results(ascii_call)[0];
    let ascii_result = emit_opt_v3_rich_compare_bool01_from_i32_compare_result(
        fb,
        emit_ctx,
        op,
        ascii_compare_result,
    );
    fb.ins()
        .jump(done_block, &[ir::BlockArg::Value(ascii_result)]);

    fb.switch_to_block(unicode_compare_block);
    let compare_ref = func_imports.get(codegen_env, &mut fb.func, &PYUNICODE_COMPARE_IMPORT)?;
    let call = fb.ins().call(compare_ref, &[lhs, rhs]);
    let compare_result = fb.inst_results(call)[0];
    let unicode_result =
        emit_opt_v3_rich_compare_bool01_from_i32_compare_result(fb, emit_ctx, op, compare_result);
    fb.ins()
        .jump(done_block, &[ir::BlockArg::Value(unicode_result)]);

    fb.switch_to_block(done_block);
    Ok(fb.block_params(done_block)[0])
}

fn opt_v3_rich_compare_op_code(op: RichCompareOp) -> i32 {
    match op {
        RichCompareOp::Eq => ffi::Py_EQ,
        RichCompareOp::Ne => ffi::Py_NE,
        RichCompareOp::Lt => ffi::Py_LT,
        RichCompareOp::Le => ffi::Py_LE,
        RichCompareOp::Gt => ffi::Py_GT,
        RichCompareOp::Ge => ffi::Py_GE,
    }
}

fn emit_typed_codegen_i32_bool01_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let direct_bool_expr = match expr {
        InstrTyped::BinOp(_) => Some(expr),
        InstrTyped::Truthy(op) if matches!(op.value(), InstrTyped::BinOp(_)) => Some(op.value()),
        _ => None,
    };
    if let Some(direct_bool_expr) = direct_bool_expr {
        let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
            fb,
            local_env,
            ctx: emit_ctx,
            codegen_env,
            func_imports,
            owned_transfer_temp_load: None,
        };
        if let Some(truth_i32) =
            intrinsics::emit_typed_i32_bool01_operation(direct_bool_expr, &mut intrinsic_state)
        {
            return Ok(EmitResult::i32(truth_i32, IntFacts::i32_bool01()));
        }
    }

    let is_true_ref = func_imports.get(codegen_env, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?;
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        expr,
        local_env,
        emit_ctx,
        false,
        codegen_env,
        func_imports,
    )?;
    Ok(emit_soac_value_result_for_demand(
        fb,
        value,
        emit_ctx,
        ResultDemand::I32_BOOL01,
        Some(is_true_ref),
    ))
}

fn emit_typed_codegen_i64_index_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    pyobject_to_i64_ref: ir::FuncRef,
) -> Result<EmitResult, String> {
    if let Some(const_value) = typed_expr_const_i64(expr, emit_ctx.module_constants) {
        let value = fb.ins().iconst(emit_ctx.consts.i64_ty, const_value);
        return Ok(EmitResult::i64(value, IntFacts::i64_known(const_value)));
    }

    if let InstrTyped::CalleeFunctionId(op) = expr {
        let (callable, callable_is_borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            op.value.as_ref(),
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            "typed callee_function_id",
        )?;
        let callee_id = emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env);
        if !callable_is_borrowed {
            emit_ctx.emit_decref_for_family(fb, callable, None, RefcountFamily::OwnedTemporary);
        }
        return Ok(EmitResult::i64(callee_id, IntFacts::i64_unknown()));
    }

    let index_value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        expr,
        local_env,
        emit_ctx,
        false,
        codegen_env,
        func_imports,
    )?;
    if let Some((index_i64, facts)) = index_value.as_i64() {
        return Ok(EmitResult::i64(index_i64, facts));
    }
    let (index_obj, ownership, facts) = index_value.expect_pyobject("typed branch-table index");
    let index_i64_inst = fb.ins().call(pyobject_to_i64_ref, &[index_obj]);
    let index_i64 = fb.inst_results(index_i64_inst)[0];
    if ownership.is_owned() {
        emit_release_owned_pyobject(fb, index_obj, Some(facts), emit_ctx);
    }
    Ok(EmitResult::i64(index_i64, IntFacts::i64_unknown()))
}

fn local_failure_cleanup_emit_ctx<'mc>(
    fb: &mut FunctionBuilder<'_>,
    emit_ctx: &JitEmitCtx<'mc>,
    local_env: &LocalEnv,
    cleanup_null_block: ir::Block,
    pending_local_failure_cleanups: &mut Vec<PendingLocalFailureCleanup>,
    local_failure_cleanup_blocks: &mut HashMap<LocalFailureCleanupKey, ir::Block>,
    block_roles: &mut ClifBlockRoles,
) -> Result<Option<JitEmitCtx<'mc>>, String> {
    if !emit_ctx.consts.step_null_args.is_empty() {
        return Ok(None);
    }
    let (forwarded_values, forwarded_local_indices, continuation) = if let Some(forwarded_names) =
        emit_ctx.exception_forwarded_local_names
    {
        let forwarding_emit_ctx = forwarding_materialization_failure_emit_ctx(
            fb,
            emit_ctx,
            local_env,
            forwarded_names,
            cleanup_null_block,
            pending_local_failure_cleanups,
            local_failure_cleanup_blocks,
            block_roles,
        );
        let (forwarded_values, forwarded_local_indices) = emit_forward_named_values_from_local_env(
            fb,
            forwarded_names,
            local_env,
            forwarding_emit_ctx.as_ref().unwrap_or(emit_ctx),
        )
        .map_err(|err| format!("missing local mapping for failure cleanup forwarding: {err}"))?;
        (
            forwarded_values,
            forwarded_local_indices,
            PendingLocalFailureContinuation::ExceptionDispatch(emit_ctx.consts.step_null_block),
        )
    } else {
        (
            Vec::new(),
            HashSet::new(),
            PendingLocalFailureContinuation::CleanupNull(cleanup_null_block),
        )
    };
    let cleanup_entries = local_env.local_only_cleanup_entries_excluding(&forwarded_local_indices);
    if cleanup_entries.is_empty() && forwarded_values.is_empty() {
        return Ok(None);
    }
    let cleanup_actions = cleanup_entries
        .iter()
        .map(|entry| {
            if matches!(
                continuation,
                PendingLocalFailureContinuation::ExceptionDispatch(_)
            ) && emit_ctx.stack_slots.has_name(entry.name.as_str())
            {
                PendingLocalFailureCleanupAction::RetireFrameRoot {
                    name: entry.name.clone(),
                    ref_kind: entry.ref_kind,
                }
            } else {
                PendingLocalFailureCleanupAction::Decref
            }
        })
        .collect::<Vec<_>>();
    if cleanup_entries.is_empty() {
        return Ok(Some(emit_ctx.with_step_null_target(
            emit_ctx.consts.step_null_block,
            forwarded_values,
        )));
    }

    let cleanup_arg_count = cleanup_entries.len();
    let forwarded_arg_count = forwarded_values.len();
    let key = LocalFailureCleanupKey::new(
        cleanup_entries.as_slice(),
        cleanup_actions.as_slice(),
        forwarded_values.as_slice(),
        continuation,
    );
    let cleanup_block = if let Some(cleanup_block) = local_failure_cleanup_blocks.get(&key).copied()
    {
        cleanup_block
    } else {
        let cleanup_block = fb.create_block();
        for _ in 0..cleanup_arg_count {
            fb.append_block_param(cleanup_block, emit_ctx.consts.ptr_ty);
        }
        for _ in 0..forwarded_arg_count {
            fb.append_block_param(cleanup_block, emit_ctx.consts.ptr_ty);
        }
        pending_local_failure_cleanups.push(PendingLocalFailureCleanup {
            block: cleanup_block,
            cleanup_arg_count,
            cleanup_actions,
            continuation,
        });
        register_block_role(block_roles, cleanup_block, ClifBlockRole::Cleanup);
        local_failure_cleanup_blocks.insert(key, cleanup_block);
        cleanup_block
    };
    let mut step_null_args: Vec<_> = cleanup_entries.iter().map(|entry| entry.value).collect();
    step_null_args.extend(forwarded_values);
    Ok(Some(
        emit_ctx.with_step_null_target(cleanup_block, step_null_args),
    ))
}

fn forwarding_materialization_failure_emit_ctx<'mc>(
    fb: &mut FunctionBuilder<'_>,
    emit_ctx: &JitEmitCtx<'mc>,
    local_env: &LocalEnv,
    forwarded_names: &[String],
    cleanup_null_block: ir::Block,
    pending_local_failure_cleanups: &mut Vec<PendingLocalFailureCleanup>,
    local_failure_cleanup_blocks: &mut HashMap<LocalFailureCleanupKey, ir::Block>,
    block_roles: &mut ClifBlockRoles,
) -> Option<JitEmitCtx<'mc>> {
    let needs_fallible_materialization = forwarded_names.iter().any(|name| {
        local_env
            .entry_index_for_block_arg_name(name)
            .is_some_and(|index| local_env.entries[index].i64_facts().is_some())
    });
    if !needs_fallible_materialization {
        return None;
    }

    let cleanup_entries = local_env.local_only_cleanup_entries_excluding(&HashSet::new());
    let cleanup_actions = cleanup_entries
        .iter()
        .map(|_| PendingLocalFailureCleanupAction::Decref)
        .collect::<Vec<_>>();
    let continuation = PendingLocalFailureContinuation::CleanupNull(cleanup_null_block);
    let key = LocalFailureCleanupKey::new(
        cleanup_entries.as_slice(),
        cleanup_actions.as_slice(),
        &[],
        continuation,
    );
    let cleanup_block = if let Some(cleanup_block) = local_failure_cleanup_blocks.get(&key).copied()
    {
        cleanup_block
    } else {
        let cleanup_block = fb.create_block();
        for _ in &cleanup_entries {
            fb.append_block_param(cleanup_block, emit_ctx.consts.ptr_ty);
        }
        pending_local_failure_cleanups.push(PendingLocalFailureCleanup {
            block: cleanup_block,
            cleanup_arg_count: cleanup_entries.len(),
            cleanup_actions,
            continuation,
        });
        register_block_role(block_roles, cleanup_block, ClifBlockRole::Cleanup);
        local_failure_cleanup_blocks.insert(key, cleanup_block);
        cleanup_block
    };
    let step_null_args = cleanup_entries
        .iter()
        .map(|entry| entry.value)
        .collect::<Vec<_>>();
    Some(emit_ctx.with_step_null_target(cleanup_block, step_null_args))
}

fn emit_typed_codegen_ops(
    fb: &mut FunctionBuilder<'_>,
    ops: &[InstrTyped],
    local_env: &mut LocalEnv,
    stack_slots: &StackSlots,
    emit_ctx: &JitEmitCtx<'_>,
    cleanup_null_block: ir::Block,
    pending_local_failure_cleanups: &mut Vec<PendingLocalFailureCleanup>,
    local_failure_cleanup_blocks: &mut HashMap<LocalFailureCleanupKey, ir::Block>,
    block_roles: &mut ClifBlockRoles,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    let mut index = 0;
    while index < ops.len() {
        let expr = &ops[index];
        let instr_id = expr.try_semantic_instr_id();
        if let Some(instr_id) = instr_id {
            emit_ctx.require_deopt_point_before_instr_id(instr_id)?;
        }
        if let (InstrTyped::Store(store), Some(InstrTyped::Del(delete)), InstrTyped::Load(source)) =
            (expr, ops.get(index + 1), store_value_expr(expr))
            && let (Some(target_location), Some(source_location), Some(delete_location)) = (
                store.name.local_location(),
                source.name.local_location(),
                delete.name.local_location(),
            )
            && source_location == delete_location
            && source.name.id.as_str() == delete.name.id.as_str()
            && is_generated_transfer_temp_name(source.name.id.as_str())
            && !typed_local_store_prefers_scalar_repr(
                expr,
                store,
                target_location,
                local_env,
                emit_ctx,
            )
        {
            let value_py_facts = if matches!(emit_ctx.function_kind, FunctionKind::Function) {
                py_facts_for_typed_expr_with_local_env(store.value.as_ref(), local_env)
            } else {
                None
            };
            let store_key = expr.semantic_instr_key(emit_ctx.function_id);
            if local_env.move_location_to_location(
                fb,
                source_location,
                source.name.id.as_str(),
                target_location,
                store.name.id.as_str(),
                value_py_facts,
                emit_ctx.allow_local_only_slot_backed_stores,
                planned_cleanup_root_previous_state_for_key(
                    store_key,
                    source.name.id.as_str(),
                    emit_ctx,
                ),
                planned_cleanup_root_previous_state_for_key(
                    store_key,
                    store.name.id.as_str(),
                    emit_ctx,
                ),
                planned_cleanup_root_previous_facts_for_key(
                    store_key,
                    store.name.id.as_str(),
                    emit_ctx,
                ),
                stack_slots,
                Some(refcount_decref_location_counter_parts(emit_ctx)),
                emit_ctx.consts.ptr_ty,
                emit_ctx.consts.thread_state_value,
                emit_ctx.decref_ref,
                emit_ctx.refcount_emitter(),
            ) {
                index += 2;
                continue;
            }
        }
        let stmt_emit_ctx = local_failure_cleanup_emit_ctx(
            fb,
            emit_ctx,
            local_env,
            cleanup_null_block,
            pending_local_failure_cleanups,
            local_failure_cleanup_blocks,
            block_roles,
        )?;
        let stmt_emit_ctx = stmt_emit_ctx.as_ref().unwrap_or(emit_ctx);
        let guard_miss_emit_ctx = instr_id
            .filter(|_| typed_nested_guard_misses_can_resume_before_instr(expr))
            .map(|instr_id| {
                stmt_emit_ctx.with_guard_miss_resume_point(LocalEnvResumePoint::BeforeInstr {
                    key: InstrKey::new(stmt_emit_ctx.function_id, instr_id),
                })
            });
        let stmt_emit_ctx = guard_miss_emit_ctx.as_ref().unwrap_or(stmt_emit_ctx);
        if let Some((transfer_location, transfer_name)) =
            typed_setitem_owned_transfer_temp(expr, ops.get(index + 1))
            && local_env.can_transfer_owned_local_only_location(transfer_location, transfer_name)
            && let Some(result) = emit_typed_intrinsic_statement_with_owned_transfer_temp(
                fb,
                expr,
                local_env,
                stmt_emit_ctx,
                expr.result_demand().unwrap_or(ResultDemand::EffectOnly),
                transfer_location,
                codegen_env,
                func_imports,
            )
        {
            let transferred = local_env.mark_owned_local_only_location_transferred(
                fb,
                transfer_location,
                transfer_name,
                emit_ctx.consts.ptr_ty,
            );
            debug_assert!(transferred);
            discard_emit_result(fb, result, emit_ctx)?;
            index += 2;
            continue;
        }
        let result = emit_typed_codegen_stmt_result_with_local_env(
            fb,
            expr,
            local_env,
            stmt_emit_ctx,
            expr.result_demand().unwrap_or(ResultDemand::EffectOnly),
            codegen_env,
            func_imports,
        )?;
        discard_emit_result(fb, result, emit_ctx)?;
        index += 1;
    }
    Ok(())
}

fn store_value_expr(expr: &InstrTyped) -> &InstrTyped {
    let InstrTyped::Store(store) = expr else {
        return expr;
    };
    store.value.as_ref()
}

fn is_generated_transfer_temp_name(name: &str) -> bool {
    name.starts_with("_dp_tmp_")
        || name.starts_with("_dp_typed_inline_")
        || name.starts_with("_dp_typed_linearized_expr_")
}

fn typed_setitem_owned_transfer_temp<'a>(
    expr: &'a InstrTyped,
    next_expr: Option<&'a InstrTyped>,
) -> Option<(LocalLocation, &'a str)> {
    let InstrTyped::SetItem(setitem) = expr else {
        return None;
    };
    let InstrTyped::Load(replacement) = setitem.replacement.as_ref() else {
        return None;
    };
    let InstrTyped::Del(delete) = next_expr? else {
        return None;
    };
    let replacement_location = replacement.name.local_location()?;
    let delete_location = delete.name.local_location()?;
    (replacement_location == delete_location
        && replacement.name.id.as_str() == delete.name.id.as_str()
        && is_generated_transfer_temp_name(replacement.name.id.as_str()))
    .then_some((replacement_location, replacement.name.id.as_str()))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_intrinsic_statement_with_owned_transfer_temp(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    transfer_location: LocalLocation,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
        fb,
        local_env,
        ctx: emit_ctx,
        codegen_env,
        func_imports,
        owned_transfer_temp_load: Some(transfer_location),
    };
    let value = intrinsics::emit_typed_operation(expr, &mut intrinsic_state)?;
    let facts = expr
        .result_facts()
        .and_then(ValueFacts::as_pyobj)
        .unwrap_or_else(PyObjFacts::unknown);
    Some(emit_owned_pyobject_result_for_demand(
        intrinsic_state.fb,
        value,
        facts,
        emit_ctx,
        demand,
    ))
}

#[derive(Clone, Copy, Default)]
struct IfTruthInstrumentation {
    true_counter_ref: Option<CounterRef>,
    true_direct_call_target: Option<(InstrId, RuntimeFunctionId)>,
}

struct IfGuardMissDeopt<'a> {
    instr_id: InstrId,
    resume_point: LocalEnvResumePoint,
    guard_operand: &'a InstrTyped,
    fallback_counter_ref: Option<CounterRef>,
}

fn direct_call_guard_if_truth_instrumentation(
    expr: &InstrTyped,
    emit_ctx: &JitEmitCtx<'_>,
) -> IfTruthInstrumentation {
    let InstrTyped::DirectCallGuardTest(op) = expr else {
        return IfTruthInstrumentation::default();
    };
    let Some(instr_id) = expr.try_semantic_instr_id() else {
        return IfTruthInstrumentation::default();
    };
    let true_counter_ref = emit_ctx.call_direct_hit_counter_ids.get(&instr_id).copied();
    let true_direct_call_target = match &op.kind {
        TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id } => {
            Some((instr_id, *function_id))
        }
        TypedDirectCallGuardTestKind::ExactTypeVersion { function_id, .. } => {
            Some((instr_id, *function_id))
        }
    };
    IfTruthInstrumentation {
        true_counter_ref,
        true_direct_call_target,
    }
}

fn direct_call_guard_if_miss_deopt<'a>(
    expr: &'a InstrTyped,
    default_resume_point: LocalEnvResumePoint,
    emit_ctx: &JitEmitCtx<'_>,
) -> Option<IfGuardMissDeopt<'a>> {
    if !expr.guard_miss_deopt_enabled() {
        return None;
    }
    let InstrTyped::DirectCallGuardTest(op) = expr else {
        return None;
    };
    let instr_id = expr.try_semantic_instr_id()?;
    Some(IfGuardMissDeopt {
        instr_id,
        resume_point: emit_ctx
            .guard_miss_resume_point
            .unwrap_or(default_resume_point),
        guard_operand: op.value.as_ref(),
        fallback_counter_ref: emit_ctx
            .call_direct_fallback_counter_ids
            .get(&instr_id)
            .copied(),
    })
}

fn typed_term_successor_labels(term: &BlockTerm<InstrTyped>) -> Vec<BlockLabel> {
    match term {
        BlockTerm::Jump(edge) => vec![edge.target],
        BlockTerm::IfTerm(if_term) => vec![if_term.then_label, if_term.else_label],
        BlockTerm::BranchTable(branch) => {
            let mut labels = branch.targets.clone();
            labels.push(branch.default_label);
            labels
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => Vec::new(),
    }
}

fn typed_direct_call_guard_deopt_else_label(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    instr_locations: &InstrLocationMap,
    guard_miss_deopt_instr_ids: &HashSet<InstrId>,
    guard_miss_deopt_stub_available: bool,
    deopt_resume_plan: &PlannedJitDeoptResumeFunction,
    runtime_supported_deopt_resume_points: Option<&[LocalEnvResumePoint]>,
) -> Option<BlockLabel> {
    if !guard_miss_deopt_stub_available {
        return None;
    }
    let BlockTerm::IfTerm(if_term) = &block.term else {
        return None;
    };
    if if_term.then_label == if_term.else_label {
        return None;
    }
    let InstrTyped::DirectCallGuardTest(op) = &if_term.test else {
        return None;
    };
    if !if_term.test.guard_miss_deopt_enabled() {
        return None;
    }
    let instr_id = if_term.test.try_semantic_instr_id()?;
    if !guard_miss_deopt_instr_ids.contains(&instr_id)
        || !runtime_jit_typed_deopt_guard_operand_replay_safe(op.value.as_ref())
    {
        return None;
    }
    let resume_point = LocalEnvResumePoint::BeforeTerm {
        function_id: function.function_id,
        block: block.label,
    };
    if runtime_supported_deopt_resume_points
        .is_some_and(|supported| !supported.contains(&resume_point))
    {
        return None;
    }
    if runtime_jit_typed_deopt_continuation_for_point(function, instr_locations, resume_point)
        .unsupported_reason()
        .is_some()
        || deopt_resume_plan.entry(resume_point).is_none()
    {
        return None;
    }
    Some(if_term.else_label)
}

fn typed_machine_deopt_suppressed_blocks(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    instr_locations: &InstrLocationMap,
    guard_miss_deopt_instr_ids: &HashSet<InstrId>,
    guard_miss_deopt_stub_available: bool,
    deopt_resume_plan: &PlannedJitDeoptResumeFunction,
    runtime_supported_deopt_resume_points: Option<&[LocalEnvResumePoint]>,
) -> HashSet<BlockLabel> {
    let mut predecessors = HashMap::<BlockLabel, HashSet<BlockLabel>>::new();
    let mut deopt_edges = HashSet::<(BlockLabel, BlockLabel)>::new();
    for block in &function.blocks {
        for successor in typed_term_successor_labels(&block.term) {
            predecessors
                .entry(successor)
                .or_default()
                .insert(block.label);
        }
        if let Some(edge) = &block.exc_edge {
            predecessors
                .entry(edge.target)
                .or_default()
                .insert(block.label);
        }
        if let Some(else_label) = typed_direct_call_guard_deopt_else_label(
            function,
            block,
            instr_locations,
            guard_miss_deopt_instr_ids,
            guard_miss_deopt_stub_available,
            deopt_resume_plan,
            runtime_supported_deopt_resume_points,
        ) {
            deopt_edges.insert((block.label, else_label));
        }
    }
    deopt_edges
        .iter()
        .filter_map(|(_, target)| {
            let preds = predecessors.get(target)?;
            (!preds.is_empty()
                && preds
                    .iter()
                    .all(|pred| deopt_edges.contains(&(*pred, *target))))
            .then_some(*target)
        })
        .collect()
}

fn emit_codegen_if_target_arm(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    arm_name: &str,
    branch_block: ir::Block,
    target_label: BlockLabel,
    target_exception_name: Option<&str>,
    release_reason: RefcountReleaseReason,
    entry_counter_ref: Option<CounterRef>,
    selected_direct_call_target: Option<(InstrId, RuntimeFunctionId)>,
    current_exception_name: Option<&str>,
    function: &BlockPyFunction<impl ModuleShape>,
    exec_blocks: &[ir::Block],
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    fb.switch_to_block(branch_block);
    emit_handle_pending_if_backedge(
        fb,
        source_label,
        target_label,
        block_indices_by_label,
        emit_ctx,
    )?;
    if let Some(counter_ref) = entry_counter_ref {
        emit_increment_counter_ref(fb, counter_ref, emit_ctx);
    }
    if let Some((instr_id, function_id)) = selected_direct_call_target {
        emit_record_direct_call_target_sample(fb, Some(instr_id), function_id, emit_ctx);
    }
    let target_index =
        codegen_block_index_for_label(function, block_indices_by_label, target_label)?;
    let edge_transport = &implicit_target_transports[target_index];
    let mut jump_args = Vec::with_capacity(edge_transport.target_args.len());
    let (prepared_args, forwarded_locations) = emit_planned_target_args_codegen_from_local_env(
        fb,
        &edge_transport.target_args,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
    .map_err(|err| {
        format!(
            "missing local mapping for {arm_name}-branch block params in block {source_label}: {err}"
        )
    })?;
    jump_args.extend(prepared_args);
    emit_planned_local_releases_for_reason_with_local_env(
        fb,
        source_label,
        &release_reason,
        local_env,
        &forwarded_locations,
        emit_ctx,
    )?;
    emit_decref_unforwarded_local_env(
        fb,
        local_env,
        &forwarded_locations,
        &[],
        emit_ctx.consts.thread_state_value,
        emit_ctx.decref_ref,
        refcount_family_for_release_reason(&release_reason),
    );
    emit_pop_handled_exception_if_leaving(
        fb,
        current_exception_name,
        target_exception_name,
        emit_ctx,
    );
    fb.ins().jump(exec_blocks[target_index], &jump_args);
    Ok(())
}

fn is_codegen_backedge(
    source_label: BlockLabel,
    target_label: BlockLabel,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
) -> Result<bool, String> {
    let source_index = *block_indices_by_label
        .get(&source_label)
        .ok_or_else(|| format!("missing codegen block index for source label {source_label}"))?;
    let target_index = *block_indices_by_label
        .get(&target_label)
        .ok_or_else(|| format!("missing codegen block index for target label {target_label}"))?;
    Ok(target_index <= source_index)
}

fn emit_handle_pending_if_backedge(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    target_label: BlockLabel,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    emit_ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    if !emit_ctx.handle_pending_checks_enabled {
        return Ok(());
    }
    if !is_codegen_backedge(source_label, target_label, block_indices_by_label)? {
        return Ok(());
    }
    let Some(py_handle_pending_ref) = emit_ctx.py_handle_pending_ref else {
        return Ok(());
    };

    let pending_inst = fb
        .ins()
        .call(py_handle_pending_ref, &[emit_ctx.consts.thread_state_value]);
    let pending_rc = fb.inst_results(pending_inst)[0];
    let pending_ok = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::Equal, pending_rc, 0);
    let continue_block = fb.create_block();
    fb.ins().brif(
        pending_ok,
        continue_block,
        &[],
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
    );
    fb.switch_to_block(continue_block);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_codegen_if_truth_i32(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    test_instr_id: Option<InstrId>,
    truth_i32: ir::Value,
    then_label: BlockLabel,
    else_label: BlockLabel,
    instrumentation: IfTruthInstrumentation,
    guard_miss_deopt: Option<IfGuardMissDeopt<'_>>,
    current_exception_name: Option<&str>,
    function: &BlockPyFunction<impl ModuleShape>,
    exec_blocks: &[ir::Block],
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    if let Some(test_instr_id) = test_instr_id {
        if let Some(counter_id) = emit_ctx
            .branch_outcome_counter_ids
            .get(&test_instr_id)
            .copied()
        {
            emit_record_branch_outcome_sample(fb, counter_id, truth_i32, emit_ctx);
        }
    }

    let prefer_true = true;
    let hot_cond = if prefer_true {
        fb.ins()
            .icmp_imm(ir::condcodes::IntCC::NotEqual, truth_i32, 0)
    } else {
        fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, truth_i32, 0)
    };
    let hot_branch = fb.create_block();
    let cold_branch = fb.create_block();
    let cold_guard_miss_dispatch = guard_miss_deopt.as_ref().and_then(|guard_miss_deopt| {
        prefer_true.then(|| {
            prepare_optional_guard_miss_dispatch(
                emit_ctx.guard_miss_target_for_typed_resume_point(
                    guard_miss_deopt.resume_point,
                    &[guard_miss_deopt.guard_operand],
                    cold_branch,
                ),
                cold_branch,
                emit_ctx.guard_miss_deopt_ref_for_instr_id(guard_miss_deopt.instr_id),
            )
        })
    });
    fb.ins().brif(hot_cond, hot_branch, &[], cold_branch, &[]);

    let (hot_name, hot_label, cold_name, cold_label) = if prefer_true {
        ("then", then_label, "else", else_label)
    } else {
        ("else", else_label, "then", then_label)
    };
    let hot_is_true = hot_label == then_label;
    let hot_counter_ref = hot_is_true
        .then_some(instrumentation.true_counter_ref)
        .flatten();
    let hot_direct_call_target = hot_is_true
        .then_some(instrumentation.true_direct_call_target)
        .flatten();
    let mut hot_local_env = local_env.clone();
    emit_codegen_if_target_arm(
        fb,
        source_label,
        hot_name,
        hot_branch,
        hot_label,
        block_exception_name(function, hot_label),
        if hot_label == then_label {
            RefcountReleaseReason::IfThen { target: hot_label }
        } else {
            RefcountReleaseReason::IfElse { target: hot_label }
        },
        hot_counter_ref,
        hot_direct_call_target,
        current_exception_name,
        function,
        exec_blocks,
        block_indices_by_label,
        implicit_target_transports,
        &mut hot_local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let cold_is_true = cold_label == then_label;
    let cold_counter_ref = cold_is_true
        .then_some(instrumentation.true_counter_ref)
        .flatten();
    let cold_direct_call_target = cold_is_true
        .then_some(instrumentation.true_direct_call_target)
        .flatten();
    let mut cold_local_env = local_env.clone();
    if let (
        Some(IfGuardMissDeopt {
            fallback_counter_ref,
            ..
        }),
        Some(JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        }),
    ) = (guard_miss_deopt.as_ref(), cold_guard_miss_dispatch)
    {
        emit_ctx
            .direct_edge_stats
            .record_guarded_generic_fallback_block();
        emit_typed_guard_miss_deopt_resume_return(
            fb,
            &cold_local_env,
            emit_ctx,
            block,
            *fallback_counter_ref,
            &[],
            target,
            deopt_resume_ref,
        );
        return Ok(());
    }
    emit_codegen_if_target_arm(
        fb,
        source_label,
        cold_name,
        cold_branch,
        cold_label,
        block_exception_name(function, cold_label),
        if cold_label == then_label {
            RefcountReleaseReason::IfThen { target: cold_label }
        } else {
            RefcountReleaseReason::IfElse { target: cold_label }
        },
        cold_counter_ref,
        cold_direct_call_target,
        current_exception_name,
        function,
        exec_blocks,
        block_indices_by_label,
        implicit_target_transports,
        &mut cold_local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
}

fn emit_codegen_return_pyobject(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    ret_value: ir::Value,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    current_exception_name: Option<&str>,
) -> Result<(), String> {
    emit_codegen_return_pyobject_with_unmaterialized_locals(
        fb,
        source_label,
        ret_value,
        local_env,
        emit_ctx,
        current_exception_name,
        &HashSet::new(),
    )
}

fn emit_codegen_return_pyobject_with_unmaterialized_locals(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    ret_value: ir::Value,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    current_exception_name: Option<&str>,
    unmaterialized_locations: &HashSet<LocalLocation>,
) -> Result<(), String> {
    let forwarded_locations = HashSet::new();
    let release_reason = RefcountReleaseReason::Return;
    emit_planned_local_releases_for_reason_with_local_env_excluding(
        fb,
        source_label,
        &release_reason,
        local_env,
        &forwarded_locations,
        unmaterialized_locations,
        emit_ctx,
    )?;
    emit_decref_unforwarded_local_env(
        fb,
        local_env,
        &forwarded_locations,
        &[],
        emit_ctx.consts.thread_state_value,
        emit_ctx.decref_ref,
        refcount_family_for_release_reason(&release_reason),
    );
    emit_pop_handled_exception_if_leaving(fb, current_exception_name, None, emit_ctx);
    let cleanup_block = emit_ctx
        .return_cleanup_blocks_by_label
        .get(&source_label)
        .copied()
        .unwrap_or_else(|| panic!("missing return cleanup block for {source_label}"));
    fb.ins()
        .jump(cleanup_block, &[ir::BlockArg::Value(ret_value)]);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_codegen_branch_table_from_i64(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    targets: &[BlockLabel],
    default_label: BlockLabel,
    index_i64: ir::Value,
    function: &BlockPyFunction<impl ModuleShape>,
    exec_blocks: &[ir::Block],
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    current_exception_name: Option<&str>,
) -> Result<(), String> {
    let i64_ty = emit_ctx.consts.i64_ty;
    let index_error = fb.ins().iconst(i64_ty, i64::MIN);
    let is_error = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, index_i64, index_error);
    let dispatch_block = fb.create_block();
    fb.append_block_param(dispatch_block, i64_ty);
    fb.ins().brif(
        is_error,
        emit_ctx.consts.step_null_block,
        &block_arg_values(&emit_ctx.consts.step_null_args),
        dispatch_block,
        &[ir::BlockArg::Value(index_i64)],
    );

    let default_block = fb.create_block();
    let mut switch = Switch::new();
    let mut case_blocks = Vec::with_capacity(targets.len());
    for (case_index, _) in targets.iter().enumerate() {
        let case_block = fb.create_block();
        switch.set_entry(case_index as u128, case_block);
        case_blocks.push(case_block);
    }

    fb.switch_to_block(dispatch_block);
    let dispatch_value = fb.block_params(dispatch_block)[0];
    switch.emit(fb, dispatch_value, default_block);

    for (target_label, case_block) in targets.iter().zip(case_blocks.iter()) {
        fb.switch_to_block(*case_block);
        let target_index =
            codegen_block_index_for_label(function, block_indices_by_label, *target_label)?;
        emit_handle_pending_if_backedge(
            fb,
            source_label,
            *target_label,
            block_indices_by_label,
            emit_ctx,
        )?;
        let edge_transport = &implicit_target_transports[target_index];
        let mut case_local_env = local_env.clone();
        let mut case_jump_args = Vec::with_capacity(edge_transport.target_args.len());
        let (prepared_args, forwarded_locations) =
            emit_planned_target_args_codegen_from_local_env(
                fb,
                &edge_transport.target_args,
                &case_local_env,
                emit_ctx,
                codegen_env,
                func_imports,
            )
            .map_err(|err| {
                format!(
                    "missing local mapping for br_table case block params in block {source_label}: {err}"
                )
            })?;
        case_jump_args.extend(prepared_args);
        let release_reason = RefcountReleaseReason::BranchCase {
            target: *target_label,
        };
        emit_planned_local_releases_for_reason_with_local_env(
            fb,
            source_label,
            &release_reason,
            &mut case_local_env,
            &forwarded_locations,
            emit_ctx,
        )?;
        emit_decref_unforwarded_local_env(
            fb,
            &case_local_env,
            &forwarded_locations,
            &[],
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
            refcount_family_for_release_reason(&release_reason),
        );
        emit_pop_handled_exception_if_leaving(
            fb,
            current_exception_name,
            block_exception_name(function, *target_label),
            emit_ctx,
        );
        fb.ins().jump(exec_blocks[target_index], &case_jump_args);
    }

    fb.switch_to_block(default_block);
    let default_index =
        codegen_block_index_for_label(function, block_indices_by_label, default_label)?;
    emit_handle_pending_if_backedge(
        fb,
        source_label,
        default_label,
        block_indices_by_label,
        emit_ctx,
    )?;
    let edge_transport = &implicit_target_transports[default_index];
    let mut default_local_env = local_env.clone();
    let mut default_jump_args = Vec::with_capacity(edge_transport.target_args.len());
    let (prepared_args, forwarded_locations) = emit_planned_target_args_codegen_from_local_env(
        fb,
        &edge_transport.target_args,
        &default_local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
    .map_err(|err| {
        format!(
            "missing local mapping for br_table default block params in block {source_label}: {err}"
        )
    })?;
    default_jump_args.extend(prepared_args);
    let release_reason = RefcountReleaseReason::BranchDefault {
        target: default_label,
    };
    emit_planned_local_releases_for_reason_with_local_env(
        fb,
        source_label,
        &release_reason,
        &mut default_local_env,
        &forwarded_locations,
        emit_ctx,
    )?;
    emit_decref_unforwarded_local_env(
        fb,
        &default_local_env,
        &forwarded_locations,
        &[],
        emit_ctx.consts.thread_state_value,
        emit_ctx.decref_ref,
        refcount_family_for_release_reason(&release_reason),
    );
    emit_pop_handled_exception_if_leaving(
        fb,
        current_exception_name,
        block_exception_name(function, default_label),
        emit_ctx,
    );
    fb.ins()
        .jump(exec_blocks[default_index], &default_jump_args);
    Ok(())
}

fn emit_load_raise_from_function(
    fb: &mut FunctionBuilder<'_>,
    emit_ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let runtime_name_id = runtime_name_id_value(fb, Some(RuntimeName::RaiseFrom));
    let raise_fn_inst = fb
        .ins()
        .call(emit_ctx.load_runtime_obj_by_id_ref, &[runtime_name_id]);
    let raise_fn = fb.inst_results(raise_fn_inst)[0];
    let raise_fn_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, raise_fn, null_ptr);
    let raise_fn_ok = fb.create_block();
    fb.append_block_param(raise_fn_ok, ptr_ty);
    fb.ins().brif(
        raise_fn_null,
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
        raise_fn_ok,
        &[ir::BlockArg::Value(raise_fn)],
    );

    fb.switch_to_block(raise_fn_ok);
    fb.block_params(raise_fn_ok)[0]
}

#[allow(clippy::too_many_arguments)]
fn emit_codegen_raise_exception_from_function(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    raise_fn: ir::Value,
    exc_value: ir::Value,
    exc_ownership: ValueOwnership,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    raise_exc_ref: ir::FuncRef,
    current_exception_name: Option<&str>,
) -> Result<(), String> {
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let thread_state_value = emit_ctx.consts.thread_state_value;
    let decref_ref = emit_ctx.decref_ref;

    // A bare `raise exc` normalizes the exception but must not overwrite an
    // explicit cause already attached by a nested `raise_from(exc, cause)`
    // expression. `NO_DEFAULT` distinguishes that operation from source
    // `raise exc from None`, whose nested helper call passes the real `None`.
    let cause_value = emit_checked_runtime_name_object(fb, RuntimeName::NoDefault, emit_ctx);
    let raise_call_inst = fb.ins().call(
        emit_ctx.py_call_positional_three_ref,
        &[
            thread_state_value,
            raise_fn,
            exc_value,
            cause_value,
            null_ptr,
            null_ptr,
        ],
    );
    let raise_exc_obj = fb.inst_results(raise_call_inst)[0];
    fb.ins()
        .call(decref_ref, &[thread_state_value, cause_value]);
    if exc_ownership.is_owned() {
        fb.ins().call(decref_ref, &[thread_state_value, exc_value]);
    }
    fb.ins().call(decref_ref, &[thread_state_value, raise_fn]);
    let raise_exc_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, raise_exc_obj, null_ptr);
    let raise_exc_ok = fb.create_block();
    fb.append_block_param(raise_exc_ok, ptr_ty);
    fb.ins().brif(
        raise_exc_null,
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
        raise_exc_ok,
        &[ir::BlockArg::Value(raise_exc_obj)],
    );

    fb.switch_to_block(raise_exc_ok);
    let reo_exc_obj = fb.block_params(raise_exc_ok)[0];
    let raise_inst = fb.ins().call(raise_exc_ref, &[reo_exc_obj]);
    let raise_rc = fb.inst_results(raise_inst)[0];
    fb.ins()
        .call(decref_ref, &[thread_state_value, reo_exc_obj]);
    let raise_rc_fail = fb.create_block();
    let raise_rc_ok = fb.create_block();
    let raise_ok = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, raise_rc, 0);
    fb.ins()
        .brif(raise_ok, raise_rc_ok, &[], raise_rc_fail, &[]);
    let exception_forwarded_names = emit_ctx.exception_forwarded_local_names.unwrap_or(&[]);

    fb.switch_to_block(raise_rc_fail);
    emit_pop_handled_exception_if_not_forwarded(
        fb,
        current_exception_name,
        exception_forwarded_names.iter().map(String::as_str),
        emit_ctx,
    );
    fb.ins().jump(
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
    );

    fb.switch_to_block(raise_rc_ok);
    let forwarded_locations = HashSet::new();
    let release_reason = RefcountReleaseReason::Raise;
    emit_planned_local_releases_for_reason_with_local_env(
        fb,
        source_label,
        &release_reason,
        local_env,
        &forwarded_locations,
        emit_ctx,
    )?;
    emit_decref_unforwarded_local_env(
        fb,
        local_env,
        &forwarded_locations,
        &emit_ctx.consts.step_null_args,
        emit_ctx.consts.thread_state_value,
        decref_ref,
        refcount_family_for_release_reason(&release_reason),
    );
    emit_pop_handled_exception_if_not_forwarded(
        fb,
        current_exception_name,
        exception_forwarded_names.iter().map(String::as_str),
        emit_ctx,
    );
    fb.ins().jump(
        emit_ctx.consts.step_null_block,
        &step_null_block_args(emit_ctx),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_codegen_term(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    term: &BlockTerm<InstrTyped>,
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    exec_blocks: &[ir::Block],
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    jump_edge_transports: &[Option<EdgeTransportPlan>],
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    pyobject_to_i64_ref: ir::FuncRef,
    raise_exc_ref: ir::FuncRef,
    current_exception_name: Option<&str>,
) -> Result<(), String> {
    let term_guard_miss_resume_point = LocalEnvResumePoint::BeforeTerm {
        function_id: emit_ctx.function_id,
        block: source_label,
    };

    if let BlockTerm::IfTerm(if_term) = term {
        let term_emit_ctx = typed_nested_guard_misses_can_resume_before_instr(&if_term.test)
            .then(|| emit_ctx.with_guard_miss_resume_point(term_guard_miss_resume_point));
        let emit_ctx = term_emit_ctx.as_ref().unwrap_or(emit_ctx);
        let test_instr_id = if_term.test.try_semantic_instr_id();
        let demand = if_term
            .test
            .result_demand()
            .unwrap_or(ResultDemand::I32_BOOL01);
        let truth = match demand {
            ResultDemand::I32Bool01 => {
                if let Some(truth_i32) = emit_typed_exact_int_branch_truth_i32(
                    fb,
                    &if_term.test,
                    local_env,
                    emit_ctx,
                    codegen_env,
                    func_imports,
                )? {
                    EmitResult::i32(truth_i32, IntFacts::i32_bool01())
                } else {
                    emit_typed_codegen_i32_bool01_result_with_local_env(
                        fb,
                        &if_term.test,
                        local_env,
                        emit_ctx,
                        codegen_env,
                        func_imports,
                    )?
                }
            }
            other => {
                return Err(format!(
                    "typed if condition requires I32Bool01 demand, got {other:?}"
                ));
            }
        };
        let truth_i32 = truth.expect_i32_bool01("typed if condition truthiness");
        let instrumentation = direct_call_guard_if_truth_instrumentation(&if_term.test, emit_ctx);
        let guard_miss_deopt =
            direct_call_guard_if_miss_deopt(&if_term.test, term_guard_miss_resume_point, emit_ctx);
        return emit_codegen_if_truth_i32(
            fb,
            source_label,
            test_instr_id,
            truth_i32,
            if_term.then_label,
            if_term.else_label,
            instrumentation,
            guard_miss_deopt,
            current_exception_name,
            function,
            exec_blocks,
            block_indices_by_label,
            implicit_target_transports,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        );
    }

    if let BlockTerm::Return(value) = term {
        let term_emit_ctx = typed_nested_guard_misses_can_resume_before_instr(value)
            .then(|| emit_ctx.with_guard_miss_resume_point(term_guard_miss_resume_point));
        let emit_ctx = term_emit_ctx.as_ref().unwrap_or(emit_ctx);
        let demand = value
            .result_demand()
            .unwrap_or(ResultDemand::PYOBJECT_BORROWED_OK);
        let result = match demand {
            ResultDemand::PyObject { .. } => {
                if let Some(ret_value) = emit_typed_exact_int_return_pyobject(
                    fb,
                    value,
                    local_env,
                    emit_ctx,
                    codegen_env,
                    func_imports,
                )? {
                    return emit_codegen_return_pyobject(
                        fb,
                        source_label,
                        ret_value,
                        local_env,
                        emit_ctx,
                        current_exception_name,
                    );
                }
                emit_typed_codegen_stmt_result_with_local_env(
                    fb,
                    value,
                    local_env,
                    emit_ctx,
                    demand,
                    codegen_env,
                    func_imports,
                )?
            }
            other => {
                return Err(format!(
                    "typed return value requires PyObject demand, got {other:?}"
                ));
            }
        };
        let (ret_value, ownership, facts) = result.expect_pyobject("typed return value");
        let (ret_value, ownership) =
            emit_promote_pyobject_to_owned_boundary(fb, ret_value, ownership, facts, emit_ctx);
        if !ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED) {
            return Err(format!(
                "typed return value produced {ownership:?}, but return requires owned PyObject"
            ));
        }
        return emit_codegen_return_pyobject(
            fb,
            source_label,
            ret_value,
            local_env,
            emit_ctx,
            current_exception_name,
        );
    }

    if let BlockTerm::Raise(raise_stmt) = term {
        let raise_fn = emit_load_raise_from_function(fb, emit_ctx);
        let (exc_value, exc_ownership) = if let Some(exc_expr) = raise_stmt.exc.as_ref() {
            // Do not propagate BeforeTerm to the exception expression yet:
            // emit_load_raise_from_function has already run, so resuming before
            // the term would replay that prework.
            let demand = exc_expr
                .result_demand()
                .unwrap_or(ResultDemand::PYOBJECT_BORROWED_OK);
            let result = match demand {
                ResultDemand::PyObject { .. } => emit_typed_codegen_stmt_result_with_local_env(
                    fb,
                    exc_expr,
                    local_env,
                    emit_ctx,
                    demand,
                    codegen_env,
                    func_imports,
                )?,
                other => {
                    return Err(format!(
                        "typed raise exception requires PyObject demand, got {other:?}"
                    ));
                }
            };
            let (exc_value, ownership, facts) = result.expect_pyobject("typed raise exception");
            let (exc_value, ownership) =
                emit_promote_pyobject_to_owned_boundary(fb, exc_value, ownership, facts, emit_ctx);
            if !ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED) {
                return Err(format!(
                    "typed raise exception produced {ownership:?}, but raise requires owned PyObject"
                ));
            }
            (exc_value, ownership)
        } else {
            let none_const = emit_none_const(fb, emit_ctx);
            emit_ctx.emit_incref_for_family(
                fb,
                none_const,
                Some(PyObjFacts::none_singleton()),
                RefcountFamily::ConstantClone,
            );
            (none_const, ValueOwnership::Owned)
        };
        return emit_codegen_raise_exception_from_function(
            fb,
            source_label,
            raise_fn,
            exc_value,
            exc_ownership,
            local_env,
            emit_ctx,
            raise_exc_ref,
            current_exception_name,
        );
    }

    if let BlockTerm::BranchTable(branch) = term {
        let term_emit_ctx = typed_nested_guard_misses_can_resume_before_instr(&branch.index)
            .then(|| emit_ctx.with_guard_miss_resume_point(term_guard_miss_resume_point));
        let emit_ctx = term_emit_ctx.as_ref().unwrap_or(emit_ctx);
        let demand = branch
            .index
            .result_demand()
            .unwrap_or(ResultDemand::I64_INDEX);
        let index = match demand {
            ResultDemand::I64Index => emit_typed_codegen_i64_index_result_with_local_env(
                fb,
                &branch.index,
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
                pyobject_to_i64_ref,
            )?,
            other => {
                return Err(format!(
                    "typed branch-table index requires I64Index demand, got {other:?}"
                ));
            }
        };
        let (index_i64, _) = index.expect_i64("typed branch-table index");
        return emit_codegen_branch_table_from_i64(
            fb,
            source_label,
            &branch.targets,
            branch.default_label,
            index_i64,
            function,
            exec_blocks,
            block_indices_by_label,
            implicit_target_transports,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
            current_exception_name,
        );
    }

    if let BlockTerm::Jump(edge) = term {
        let target_index =
            codegen_block_index_for_label(function, block_indices_by_label, edge.target)?;
        let source_index =
            codegen_block_index_for_label(function, block_indices_by_label, source_label)?;
        emit_handle_pending_if_backedge(
            fb,
            source_label,
            edge.target,
            block_indices_by_label,
            emit_ctx,
        )?;
        let edge_transport = jump_edge_transports[source_index]
            .as_ref()
            .expect("jump term should have a planned edge transport");
        let mut jump_args = Vec::with_capacity(edge_transport.target_args.len());
        let (prepared_args, forwarded_locations) = emit_planned_target_args_codegen_from_local_env(
            fb,
            &edge_transport.target_args,
            local_env,
            emit_ctx,
            codegen_env,
            func_imports,
        )
        .map_err(|err| {
            format!(
                "missing local mapping for typed jump block params in block {source_label}: {err}"
            )
        })?;
        jump_args.extend(prepared_args);
        let release_reason = RefcountReleaseReason::Jump {
            target: edge.target,
        };
        emit_planned_local_releases_for_reason_with_local_env(
            fb,
            source_label,
            &release_reason,
            local_env,
            &forwarded_locations,
            emit_ctx,
        )?;
        emit_decref_unforwarded_local_env(
            fb,
            local_env,
            &forwarded_locations,
            &[],
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
            refcount_family_for_release_reason(&release_reason),
        );
        emit_pop_handled_exception_if_leaving(
            fb,
            current_exception_name,
            block_exception_name(function, edge.target),
            emit_ctx,
        );
        fb.ins().jump(exec_blocks[target_index], &jump_args);
        return Ok(());
    }

    unreachable!("all typed block terminators should be handled before legacy lowering")
}

#[derive(Clone, Debug, Default)]
struct BuildSpecializedFunctionOptions {
    module_constant_accesses: ModuleConstantAccessTable,
    counted_refcount_helpers: Option<CountedRefcountHelpers>,
    planned_typed_function: Option<BlockPyFunction<TypedBlockPyModuleShape>>,
    runtime_supported_deopt_resume_points: Option<Vec<LocalEnvResumePoint>>,
    external_direct_call_target_functions:
        HashMap<RuntimeFunctionId, BlockPyFunction<TypedBlockPyModuleShape>>,
}

struct PreparedSpecializedTypedFunction {
    typed_function: BlockPyFunction<TypedBlockPyModuleShape>,
}

fn block_edge_shape(edge: Option<&BlockEdge>) -> Option<(BlockLabel, usize)> {
    edge.map(|edge| (edge.target.clone(), edge.args.len()))
}

fn block_term_shape<I: soac_core::block_py::Instr>(
    term: &BlockTerm<I>,
) -> (&'static str, Vec<(BlockLabel, usize)>) {
    match term {
        BlockTerm::Jump(edge) => ("jump", vec![(edge.target.clone(), edge.args.len())]),
        BlockTerm::IfTerm(if_term) => (
            "if",
            vec![
                (if_term.then_label.clone(), 0),
                (if_term.else_label.clone(), 0),
            ],
        ),
        BlockTerm::BranchTable(branch) => {
            let mut edges = branch
                .targets
                .iter()
                .cloned()
                .map(|target| (target, 0))
                .collect::<Vec<_>>();
            edges.push((branch.default_label.clone(), 0));
            ("branch_table", edges)
        }
        BlockTerm::Raise(_) => ("raise", Vec::new()),
        BlockTerm::Return(_) => ("return", Vec::new()),
    }
}

fn validate_typed_function_preserves_codegen_cfg(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    typed_function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> Result<(), String> {
    if typed_function.blocks.len() != function.blocks.len() {
        return Err(format!(
            "typed specialized JIT function block count mismatch: {} != {}",
            typed_function.blocks.len(),
            function.blocks.len()
        ));
    }
    for (index, (codegen_block, typed_block)) in function
        .blocks
        .iter()
        .zip(typed_function.blocks.iter())
        .enumerate()
    {
        if typed_block.label != codegen_block.label {
            return Err(format!(
                "typed specialized JIT block {index} label mismatch: {} != {}",
                typed_block.label, codegen_block.label
            ));
        }
        if typed_block.params != codegen_block.params {
            return Err(format!(
                "typed specialized JIT block {} param mismatch",
                codegen_block.label
            ));
        }
        if block_edge_shape(typed_block.exc_edge.as_ref())
            != block_edge_shape(codegen_block.exc_edge.as_ref())
        {
            return Err(format!(
                "typed specialized JIT block {} exception edge mismatch",
                codegen_block.label
            ));
        }
        if block_term_shape(&typed_block.term) != block_term_shape(&codegen_block.term) {
            return Err(format!(
                "typed specialized JIT block {} terminator CFG mismatch",
                codegen_block.label
            ));
        }
    }
    Ok(())
}

fn annotate_typed_profiled_cold_blocks(
    typed_function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let cold_block_labels = profile.cold_block_labels(typed_function)?;
    if cold_block_labels.is_empty() {
        return Ok(());
    }
    for block in &mut typed_function.blocks {
        if cold_block_labels.contains(&block.label) {
            block.extra.layout = TypedBlockLayoutHint::Cold;
        }
    }
    Ok(())
}

fn prepare_specialized_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    planned_typed_function: Option<&BlockPyFunction<TypedBlockPyModuleShape>>,
    value_facts: &FactStore,
) -> Result<PreparedSpecializedTypedFunction, String> {
    let mut typed_function = planned_typed_function
        .cloned()
        .unwrap_or_else(|| function.clone());
    annotate_typed_function_value_facts(&mut typed_function, value_facts);
    validate_typed_function_value_facts(&typed_function)?;
    lower_typed_function_call_access_plan_instrs(&mut typed_function);
    refresh_typed_function_value_facts(&mut typed_function);
    annotate_typed_function_result_demands(&mut typed_function);
    annotate_typed_function_planned_results(&mut typed_function);
    validate_typed_function_call_access_plans(&typed_function)?;
    validate_typed_function_value_facts(&typed_function)?;
    validate_typed_function_preserves_codegen_cfg(function, &typed_function)?;
    Ok(PreparedSpecializedTypedFunction { typed_function })
}

fn build_cranelift_run_bb_specialized_function(
    codegen_env: &mut impl JitCodegenEnv,
    blocks: &[ObjPtr],
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    value_facts: &FactStore,
    jit_local_plan: &PlannedJitFunctionLocals,
    jit_deopt_resume_plan: &PlannedJitDeoptResumeFunction,
    module_constants: &ModuleCodegenConstants,
    counter_defs: &[CounterDef],
    module_constant_object_data_ids: &[DataId],
    counter_slots_by_id: &[CounterRuntimeSlot],
    scalar_counter_data_id: Option<DataId>,
    top_value_counter_data_id: Option<DataId>,
    compile_session: &crate::session::CompileSession,
    direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
    symbol_scope: Option<&str>,
    predeclared_direct_functions: Option<&HashMap<RuntimeFunctionId, DeclaredJitFunction>>,
    options: BuildSpecializedFunctionOptions,
) -> Result<BuiltSpecializedFunction, String> {
    let env_config = compile_session.env_config()?;
    let block_count = function.blocks.len();
    if block_count == 0 {
        return Err(format!("specialized JIT run_bb plan has no blocks"));
    }
    if !blocks.is_empty() && blocks.len() != block_count {
        return Err(format!(
            "specialized JIT block table length mismatch: {} != {}",
            blocks.len(),
            block_count
        ));
    }
    for block in &function.blocks {
        for expr in &block.body {
            if let InstrTyped::IncrementCounter(op) = expr {
                if scalar_counter_slot_for_id(counter_slots_by_id, op.counter_id).is_err() {
                    return Err(format!(
                        "specialized JIT scalar counter layout is missing counter id {} for function {}",
                        op.counter_id.0, function.names.qualname
                    ));
                }
            }
        }
    }
    jit_deopt_resume_plan.validate_for_typed_function(function)?;
    let call_target_counter_ids =
        collect_runtime_counter_ids_by_kind(counter_defs, function.function_id, "call_hot_targets");
    let call_direct_target_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "call_direct_targets",
    );
    let call_direct_hit_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "call_direct",
        "hit",
    );
    let call_direct_fallback_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "call_direct",
        "fallback",
    );
    let operator_shape_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "operator_hot_shapes",
    );
    let getitem_shape_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "getitem_hot_shapes",
    );
    let getitem_specialized_hit_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "getitem_specialized",
        "hit",
    );
    let getitem_specialized_fallback_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "getitem_specialized",
        "fallback",
    );
    let getitem_specialized_hit_counter_ids_by_source =
        collect_runtime_counter_refs_by_kind_branch_source(
            counter_defs,
            "getitem_specialized",
            "hit",
        );
    let getitem_specialized_fallback_counter_ids_by_source =
        collect_runtime_counter_refs_by_kind_branch_source(
            counter_defs,
            "getitem_specialized",
            "fallback",
        );
    let setitem_shape_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "setitem_hot_shapes",
    );
    let setitem_specialized_hit_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "setitem_specialized",
        "hit",
    );
    let setitem_specialized_fallback_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "setitem_specialized",
        "fallback",
    );
    let setitem_specialized_hit_counter_ids_by_source =
        collect_runtime_counter_refs_by_kind_branch_source(
            counter_defs,
            "setitem_specialized",
            "hit",
        );
    let setitem_specialized_fallback_counter_ids_by_source =
        collect_runtime_counter_refs_by_kind_branch_source(
            counter_defs,
            "setitem_specialized",
            "fallback",
        );
    let global_indexed_hit_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "global_indexed",
        "hit",
    );
    let global_indexed_fallback_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "global_indexed",
        "fallback",
    );
    let field_indexed_hit_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "field_access",
        "indexed_hit",
    );
    let field_indexed_fallback_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "field_access",
        "indexed_fallback",
    );
    let field_indexed_hit_counter_ids_by_source =
        collect_runtime_counter_refs_by_kind_branch_source(
            counter_defs,
            "field_access",
            "indexed_hit",
        );
    let field_indexed_fallback_counter_ids_by_source =
        collect_runtime_counter_refs_by_kind_branch_source(
            counter_defs,
            "field_access",
            "indexed_fallback",
        );
    let field_generic_getattr_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "field_access",
        "generic_getattr",
    );
    let field_generic_setattr_counter_ids = collect_runtime_counter_refs_by_kind_branch(
        counter_defs,
        function.function_id,
        "field_access",
        "generic_setattr",
    );
    let deopt_entry_guard_miss_counter_ids = collect_deopt_entry_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "deopt_entry_guard_miss",
        jit_deopt_resume_plan,
    );
    let branch_outcome_counter_ids =
        collect_runtime_counter_ids_by_kind(counter_defs, function.function_id, "branch_outcomes");
    let refcount_decref_location_counter_refs = collect_runtime_branch_counter_refs_by_kind(
        counter_defs,
        function.function_id,
        RUNTIME_DECREF_LOCATION_COUNTER_KIND,
    );
    for counter_id in call_target_counter_ids
        .values()
        .chain(call_direct_target_counter_ids.values())
        .chain(operator_shape_counter_ids.values())
        .chain(getitem_shape_counter_ids.values())
        .chain(setitem_shape_counter_ids.values())
        .chain(branch_outcome_counter_ids.values())
    {
        top_value_counter_slot_for_id(counter_slots_by_id, *counter_id).map_err(|_| {
            format!(
                "specialized JIT top-value counter layout is missing counter id {} for function {}",
                counter_id.0, function.names.qualname
            )
        })?;
    }
    for counter_ref in call_direct_hit_counter_ids
        .values()
        .chain(call_direct_fallback_counter_ids.values())
        .chain(getitem_specialized_hit_counter_ids.values())
        .chain(getitem_specialized_fallback_counter_ids.values())
        .chain(getitem_specialized_hit_counter_ids_by_source.values())
        .chain(getitem_specialized_fallback_counter_ids_by_source.values())
        .chain(setitem_specialized_hit_counter_ids.values())
        .chain(setitem_specialized_fallback_counter_ids.values())
        .chain(setitem_specialized_hit_counter_ids_by_source.values())
        .chain(setitem_specialized_fallback_counter_ids_by_source.values())
        .chain(global_indexed_hit_counter_ids.values())
        .chain(global_indexed_fallback_counter_ids.values())
        .chain(field_indexed_hit_counter_ids.values())
        .chain(field_indexed_fallback_counter_ids.values())
        .chain(field_indexed_hit_counter_ids_by_source.values())
        .chain(field_indexed_fallback_counter_ids_by_source.values())
        .chain(field_generic_getattr_counter_ids.values())
        .chain(field_generic_setattr_counter_ids.values())
        .chain(refcount_decref_location_counter_refs.values())
    {
        scalar_counter_slot_for_ref(counter_slots_by_id, *counter_ref).map_err(|err| {
            format!(
                "{err} for function {} ({})",
                function.names.qualname, function.function_id
            )
        })?;
    }
    let requires_top_value_counters = !call_target_counter_ids.is_empty()
        || !call_direct_target_counter_ids.is_empty()
        || !operator_shape_counter_ids.is_empty()
        || !getitem_shape_counter_ids.is_empty()
        || !setitem_shape_counter_ids.is_empty()
        || !branch_outcome_counter_ids.is_empty();
    if requires_top_value_counters && top_value_counter_data_id.is_none() {
        return Err(format!(
            "missing top-value counter storage for function {}",
            function.names.qualname
        ));
    }
    let function_runtime_data_layout = FunctionRuntimeDataLayout::from_typed_function(function);
    let true_constant_id = module_constants.require_runtime_name_constant_id("TRUE");
    let false_constant_id = module_constants.require_runtime_name_constant_id("FALSE");
    let none_constant_id = module_constants.require_runtime_name_constant_id("NONE");
    let empty_tuple_constant_id = module_constants.require_runtime_name_constant_id("EMPTY_TUPLE");

    let direct_edge_stats = DirectEdgeStats::default();
    let PreparedSpecializedTypedFunction { typed_function } = prepare_specialized_typed_function(
        function,
        options.planned_typed_function.as_ref(),
        value_facts,
    )?;
    let indexed_field_guards_by_instr = typed_indexed_field_guards_by_instr(&typed_function);
    let guard_miss_deopt_instr_ids = collect_typed_guard_miss_deopt_instr_ids(&typed_function);
    let guard_miss_deopt_without_refcounts_instr_ids =
        collect_typed_exact_int_guard_miss_deopt_instr_ids(&typed_function);
    let guard_miss_deopt_stub = !guard_miss_deopt_instr_ids.is_empty();
    let direct_call_targets = collect_typed_call_direct_targets(&typed_function);
    let empty_direct_functions = HashMap::new();
    let direct_call_functions = predeclared_direct_functions.unwrap_or(&empty_direct_functions);
    let mut direct_call_target_functions = options.external_direct_call_target_functions.clone();
    for function_id in direct_call_targets {
        if module
            .callable_defs
            .iter()
            .any(|function| function.function_id == function_id)
            || direct_call_target_functions.contains_key(&function_id)
        {
            continue;
        }
        let Some(target_function) = direct_call_resolver
            .map(|shared_state| {
                shared_state.lookup_direct_call_target_function(compile_session, function_id)
            })
            .transpose()?
            .flatten()
        else {
            continue;
        };
        direct_call_target_functions.insert(
            function_id,
            lower_blockpy_function_to_typed(target_function),
        );
    }
    let ptr_ty = codegen_env.codegen_target_config().pointer_type();
    let i64_ty = ir::types::I64;
    let mut module_imports = ModuleFuncImports::new();

    let (main_sig, main_id, main_symbol, default_adapter_id, default_adapter_symbol) =
        match predeclared_direct_functions
            .and_then(|functions| functions.get(&function.function_id))
        {
            Some(declared) => (
                make_direct_function_signature(codegen_env, function),
                declared.func_id,
                declared.symbol.clone(),
                declared.default_func_id,
                declared.default_symbol.clone(),
            ),
            None => {
                let (sig, declared) = declare_direct_function(codegen_env, function, symbol_scope)?;
                (
                    sig,
                    declared.func_id,
                    declared.symbol,
                    declared.default_func_id,
                    declared.default_symbol,
                )
            }
        };
    let counted_refcount_helpers = if let Some(counted_refcount_helpers) =
        options.counted_refcount_helpers
    {
        counted_refcount_helpers
    } else {
        let jit_module = codegen_env.codegen_jit_module_mut().ok_or_else(|| {
            "counted runtime refcount helpers must be reserved before detached codegen".to_string()
        })?;
        build_counted_runtime_refcount_helpers(
            jit_module,
            compile_session.env_config()?,
            function,
            counter_defs,
            counter_slots_by_id,
            scalar_counter_data_id,
            symbol_scope,
        )?
    };

    let mut ctx = codegen_env.codegen_make_context();
    ctx.func.signature = main_sig;
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut block_annotations = ClifBlockDisplayAnnotations::new();
    let mut block_roles = ClifBlockRoles::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = fb.create_block();
        let mut exec_blocks = Vec::with_capacity(block_count);
        let block_indices_by_label = codegen_block_indices_by_label(function);
        let runtime_block_params = &jit_local_plan.runtime_block_params;
        let implicit_target_transports = &jit_local_plan.implicit_target_transports;
        let jump_edge_transports = &jit_local_plan.jump_edge_transports;
        let entry_materializations = &jit_local_plan.entry_materializations;
        let exc_dispatches = &jit_local_plan.exc_dispatches;
        let refcount_plan = &jit_local_plan.refcount_plan;
        let instr_locations = current_instr_locations(function);
        let full_block_param_names = function
            .blocks
            .iter()
            .map(Block::param_name_vec)
            .collect::<Vec<_>>();
        let shared_null_cleanup = function
            .blocks
            .iter()
            .any(|block| block.exception_param().is_none())
            .then(|| (fb.create_block(), fb.create_block()));
        let mut per_exception_null_cleanup_blocks = Vec::new();
        let mut pre_cleanup_null_blocks = Vec::with_capacity(block_count);
        let mut cleanup_null_blocks = Vec::with_capacity(block_count);
        for (index, block) in function.blocks.iter().enumerate() {
            exec_blocks.push(fb.create_block());
            if block.exception_param().is_none() {
                let (pre_cleanup, cleanup) =
                    shared_null_cleanup.expect("shared null cleanup should exist");
                pre_cleanup_null_blocks.push(pre_cleanup);
                cleanup_null_blocks.push(cleanup);
            } else {
                let pre_cleanup = fb.create_block();
                let cleanup = fb.create_block();
                pre_cleanup_null_blocks.push(pre_cleanup);
                cleanup_null_blocks.push(cleanup);
                per_exception_null_cleanup_blocks.push((index, pre_cleanup, cleanup));
            }
        }
        for (index, block) in exec_blocks.iter().enumerate() {
            if typed_function.blocks[index].extra.layout == TypedBlockLayoutHint::Cold {
                fb.set_cold_block(*block);
            }
        }
        if let Some((pre_cleanup, cleanup)) = shared_null_cleanup {
            fb.set_cold_block(pre_cleanup);
            fb.set_cold_block(cleanup);
        }
        for (_, pre_cleanup, cleanup) in &per_exception_null_cleanup_blocks {
            fb.set_cold_block(*pre_cleanup);
            fb.set_cold_block(*cleanup);
        }
        let step_null_block = fb.create_block();
        let raise_exc_direct_block = fb.create_block();
        let exception_exit_sweep_block = fb.create_block();
        let exception_exit_post_sweep_decref_block = fb.create_block();
        let exception_exit_restore_block = fb.create_block();
        let exception_exit_return_block = fb.create_block();
        fb.set_cold_block(step_null_block);
        fb.set_cold_block(raise_exc_direct_block);
        fb.set_cold_block(exception_exit_sweep_block);
        fb.set_cold_block(exception_exit_post_sweep_decref_block);
        fb.set_cold_block(exception_exit_restore_block);
        fb.set_cold_block(exception_exit_return_block);
        let mut return_cleanup_blocks_by_key = HashMap::<Vec<String>, ir::Block>::new();
        let mut return_cleanup_blocks_by_label = HashMap::<BlockLabel, ir::Block>::new();
        let mut return_cleanup_block_states = Vec::<(
            ir::Block,
            Vec<String>,
            HashMap<String, CleanupRootSlotState>,
            HashMap<String, PyObjFacts>,
        )>::new();
        for block in &function.blocks {
            if !matches!(block.term, BlockTerm::Return(_)) {
                continue;
            }
            let states = jit_local_plan
                .cleanup_root_slot_states
                .exit_state_for_block(block.label);
            let facts = jit_local_plan
                .cleanup_root_slot_states
                .exit_facts_for_block(block.label);
            let key = cleanup_root_state_key(&states);
            let cleanup_block = if let Some(cleanup_block) = return_cleanup_blocks_by_key.get(&key)
            {
                *cleanup_block
            } else {
                let cleanup_block = fb.create_block();
                return_cleanup_blocks_by_key.insert(key.clone(), cleanup_block);
                return_cleanup_block_states.push((cleanup_block, key.clone(), states, facts));
                cleanup_block
            };
            return_cleanup_blocks_by_label.insert(block.label, cleanup_block);
        }
        let required_stack_slot_names =
            jit_local_plan.required_stack_slot_names_for_function(function);
        let stack_slots = StackSlots::new(
            &mut fb,
            &required_stack_slot_names,
            &jit_local_plan.cleanup_root_names,
            function.storage_layout().as_ref(),
        );
        let exception_state_slots = ExceptionStateSlots::new(&mut fb, function);

        register_block_display_annotation(
            &mut block_annotations,
            entry_block,
            "jit_entry",
            vec![
                "fn_env".into(),
                "tstate".into(),
                "entry_args".into(),
                "ambient_args".into(),
            ],
        );
        for (index, block) in exec_blocks.iter().enumerate() {
            let param_names = if runtime_block_params[index].is_empty() {
                full_block_param_names[index].clone()
            } else {
                runtime_block_params[index]
                    .iter()
                    .map(|param| param.arg_name.clone())
                    .collect()
            };
            register_block_display_annotation(
                &mut block_annotations,
                *block,
                function.blocks[index].label.to_string(),
                param_names,
            );
        }
        if let Some((pre_cleanup, cleanup)) = shared_null_cleanup {
            register_block_display_annotation(
                &mut block_annotations,
                pre_cleanup,
                "pre_cleanup_null::shared",
                Vec::new(),
            );
            register_block_role(&mut block_roles, pre_cleanup, ClifBlockRole::Cleanup);
            register_block_display_annotation(
                &mut block_annotations,
                cleanup,
                "cleanup_null::shared",
                vec!["error".into()],
            );
            register_block_role(&mut block_roles, cleanup, ClifBlockRole::Cleanup);
        }
        for (index, pre_cleanup, cleanup) in &per_exception_null_cleanup_blocks {
            register_block_display_annotation(
                &mut block_annotations,
                *pre_cleanup,
                format!("pre_cleanup_null::{}", function.blocks[*index].label),
                Vec::new(),
            );
            register_block_role(&mut block_roles, *pre_cleanup, ClifBlockRole::Cleanup);
            register_block_display_annotation(
                &mut block_annotations,
                *cleanup,
                format!("cleanup_null::{}", function.blocks[*index].label),
                vec!["error".into()],
            );
            register_block_role(&mut block_roles, *cleanup, ClifBlockRole::Cleanup);
        }
        register_block_display_annotation(
            &mut block_annotations,
            step_null_block,
            "step_null",
            vec!["args".into()],
        );
        register_block_display_annotation(
            &mut block_annotations,
            raise_exc_direct_block,
            "raise_exc_direct",
            vec!["args".into(), "exc".into()],
        );
        register_block_display_annotation(
            &mut block_annotations,
            exception_exit_sweep_block,
            "cleanup_exception_sweep",
            vec!["kind".into(), "error".into(), "post_sweep_decref".into()],
        );
        register_block_role(
            &mut block_roles,
            exception_exit_sweep_block,
            ClifBlockRole::Cleanup,
        );
        register_block_display_annotation(
            &mut block_annotations,
            exception_exit_post_sweep_decref_block,
            "cleanup_exception_post_sweep_decref",
            vec!["error".into(), "value".into()],
        );
        register_block_role(
            &mut block_roles,
            exception_exit_post_sweep_decref_block,
            ClifBlockRole::Cleanup,
        );
        register_block_display_annotation(
            &mut block_annotations,
            exception_exit_restore_block,
            "cleanup_exception_restore",
            vec!["error".into()],
        );
        register_block_role(
            &mut block_roles,
            exception_exit_restore_block,
            ClifBlockRole::Cleanup,
        );
        register_block_display_annotation(
            &mut block_annotations,
            exception_exit_return_block,
            "cleanup_exception_return",
            Vec::new(),
        );
        register_block_role(
            &mut block_roles,
            exception_exit_return_block,
            ClifBlockRole::Cleanup,
        );
        for (cleanup_block, key, _, _) in &return_cleanup_block_states {
            let label = if key.is_empty() {
                "cleanup_return::no_roots".to_string()
            } else {
                format!("cleanup_return::{}", key.join(","))
            };
            register_block_display_annotation(
                &mut block_annotations,
                *cleanup_block,
                label,
                vec!["ret".into()],
            );
            register_block_role(&mut block_roles, *cleanup_block, ClifBlockRole::Cleanup);
        }

        fb.append_block_params_for_function_params(entry_block);
        for (index, block) in exec_blocks.iter().enumerate() {
            for param in &runtime_block_params[index] {
                let param_ty = match param.repr {
                    RuntimeBlockParamRepr::PyObject => ptr_ty,
                    RuntimeBlockParamRepr::ExactI64 => i64_ty,
                    RuntimeBlockParamRepr::I32Bool01 => ir::types::I32,
                };
                fb.append_block_param(*block, param_ty);
            }
        }
        fb.append_block_param(step_null_block, ptr_ty); // args
        fb.append_block_param(raise_exc_direct_block, ptr_ty); // args
        fb.append_block_param(raise_exc_direct_block, ptr_ty); // exc
        fb.append_block_param(exception_exit_sweep_block, ir::types::I64); // kind
        fb.append_block_param(exception_exit_sweep_block, ptr_ty); // error
        fb.append_block_param(exception_exit_sweep_block, ptr_ty); // post_sweep_decref
        fb.append_block_param(exception_exit_post_sweep_decref_block, ptr_ty); // error
        fb.append_block_param(exception_exit_post_sweep_decref_block, ptr_ty); // value
        fb.append_block_param(exception_exit_restore_block, ptr_ty); // error
        for (cleanup_block, _, _, _) in &return_cleanup_block_states {
            fb.append_block_param(*cleanup_block, ptr_ty); // ret
        }
        if let Some((_, cleanup)) = shared_null_cleanup {
            fb.append_block_param(cleanup, ptr_ty); // error
        }
        for (_, _, cleanup) in &per_exception_null_cleanup_blocks {
            fb.append_block_param(*cleanup, ptr_ty); // error
        }

        fb.switch_to_block(entry_block);
        let entry_block_params = fb.block_params(entry_block).to_vec();
        let fn_env_value = entry_block_params[0];
        let thread_state_value = entry_block_params[1];
        let globals_value = load_function_env_obj(
            &mut fb,
            ptr_ty,
            fn_env_value,
            FUNCTION_ENV_GLOBALS_OBJ_OFFSET,
        );
        let function_data_value = fb
            .ins()
            .iadd_imm(fn_env_value, i64::from(FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET));
        let direct_entry_args = entry_block_params[2..].to_vec();
        let mut func_imports = FuncBuildImports::new(&mut module_imports);
        let incref_ref = if let Some(incref_func_id) = counted_refcount_helpers.incref_func_id {
            codegen_env.codegen_declare_func_in_func(incref_func_id, &mut fb.func)?
        } else {
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_INCREF_IMPORT)
        };
        let decref_ref = if let Some(decref_func_id) = counted_refcount_helpers.decref_func_id {
            codegen_env.codegen_declare_func_in_func(decref_func_id, &mut fb.func)?
        } else {
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_DECREF_IMPORT)
        };
        let refcount_lowering = if !env_config.jit_refcount_emission_enabled() {
            RefcountLowering::Disabled
        } else if counted_refcount_helpers.incref_func_id.is_some()
            || counted_refcount_helpers.decref_func_id.is_some()
        {
            RefcountLowering::HelperCalls {
                incref_ref,
                decref_ref,
            }
        } else {
            let dealloc_preserving_error_ref = func_imports.get_or_panic(
                codegen_env,
                &mut fb.func,
                &DP_JIT_DECREF_DEALLOC_PRESERVING_ERROR_IMPORT,
            );
            RefcountLowering::Explicit {
                dealloc_preserving_error_ref,
            }
        };
        let refcounts = RefcountEmitter {
            ptr_ty,
            thread_state_value,
            lowering: refcount_lowering,
            family: None,
        };
        let py_call_positional_three_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT,
        );
        let py_call_object_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PY_CALL_OBJECT_IMPORT);
        let py_vectorcall_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PY_VECTORCALL_IMPORT);
        let handle_pending_checks_enabled = env_config.jit_handle_pending_checks_enabled();
        let py_handle_pending_ref = handle_pending_checks_enabled.then(|| {
            func_imports.get_or_panic(codegen_env, &mut fb.func, &PY_HANDLE_PENDING_IMPORT)
        });
        let py_call_with_kw_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PY_CALL_WITH_KW_IMPORT);
        let enter_recursive_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_ENTER_RECURSIVE_CALL_IMPORT,
        );
        let direct_compile_function_env_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_DIRECT_COMPILE_FUNCTION_ENV_IMPORT,
        );
        let pytype_generic_alloc_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_PYTYPE_GENERIC_ALLOC_IMPORT,
        );
        let finish_constructor_init_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_FINISH_CONSTRUCTOR_INIT_IMPORT,
        );
        let load_global_fast_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &SOAC_RUNTIME_LOAD_GLOBAL_IMPORT);
        let probe_global_indexed_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_IMPORT,
        );
        let load_global_slow_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &SOAC_RUNTIME_LOAD_GLOBAL_SLOW_IMPORT,
        );
        let guard_miss_deopt_stub_ref = (guard_miss_deopt_stub
            && (env_config.jit_refcount_emission_enabled()
                || !guard_miss_deopt_without_refcounts_instr_ids.is_empty()))
        .then(|| func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_DEOPT_RESUME_IMPORT));
        let machine_deopt_suppressed_blocks = typed_machine_deopt_suppressed_blocks(
            &typed_function,
            &instr_locations,
            &guard_miss_deopt_instr_ids,
            guard_miss_deopt_stub_ref.is_some(),
            jit_deopt_resume_plan,
            options.runtime_supported_deopt_resume_points.as_deref(),
        );
        let load_runtime_obj_by_id_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_LOAD_RUNTIME_OBJ_BY_ID_IMPORT,
        );
        let raise_exc_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_RAISE_FROM_EXC_IMPORT);
        let push_handled_exception_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_PUSH_HANDLED_EXCEPTION_IMPORT,
        );
        let pop_handled_exception_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_POP_HANDLED_EXCEPTION_IMPORT,
        );
        let pyobject_getattr_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PYOBJECT_GETATTR_IMPORT);
        let pyobject_setattr_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PYOBJECT_SETATTR_IMPORT);
        let pyobject_getitem_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PYOBJECT_GETITEM_IMPORT);
        let pyobject_setitem_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PYOBJECT_SETITEM_IMPORT);
        let preserved_values_ptr_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_PRESERVED_VALUES_PTR_IMPORT,
        );
        let del_preserved_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_DEL_PRESERVED_IMPORT);
        let del_preserved_quietly_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_DEL_PRESERVED_QUIETLY_IMPORT,
        );
        let pyobject_to_i64_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PYOBJECT_TO_I64_IMPORT);
        let py_long_from_i64_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &PYLONG_FROM_LONGLONG_IMPORT);
        let record_top_value_sample_ref = requires_top_value_counters.then(|| {
            func_imports.get_or_panic(
                codegen_env,
                &mut fb.func,
                &DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT,
            )
        });
        let raise_unbound_local_error_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_RAISE_UNBOUND_LOCAL_ERROR_IMPORT,
        );
        let make_function_with_closure_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_IMPORT,
        );
        let make_cell_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_MAKE_CELL_IMPORT);
        let load_cell_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_LOAD_CELL_IMPORT);
        let store_cell_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_STORE_CELL_IMPORT);
        let tuple_new_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &SOAC_RUNTIME_TUPLE_NEW_IMPORT);
        let tuple_set_item_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_IMPORT,
        );
        let set_raised_exception_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT,
        );
        let module_constant_object_globals = module_constant_object_data_ids
            .iter()
            .map(|data_id| codegen_env.codegen_declare_data_in_func(*data_id, &mut fb.func))
            .collect::<Result<Vec<_>, _>>()?;
        let scalar_counter_base_value = scalar_counter_data_id.map(|data_id| {
            let counter_data = codegen_env
                .codegen_declare_data_in_func(data_id, &mut fb.func)
                .expect("scalar counter storage should be declared before JIT codegen");
            fb.ins().global_value(ptr_ty, counter_data)
        });
        let top_value_counter_base_value = top_value_counter_data_id.map(|data_id| {
            let counter_data = codegen_env
                .codegen_declare_data_in_func(data_id, &mut fb.func)
                .expect("top-value counter storage should be declared before JIT codegen");
            fb.ins().global_value(ptr_ty, counter_data)
        });
        let fallthrough_abrupt_kind_const = stack_slots.has_try_abrupt_kind_name().then(|| {
            emit_owned_module_constant_from_parts(
                &mut fb,
                module_constants.require_int_constant_id(abrupt_kind_tag(AbruptKind::Fallthrough)),
                &module_constant_object_globals,
                ptr_ty,
                &options.module_constant_accesses,
            )
        });
        stack_slots.initialize_all(&mut fb, ptr_ty, fallthrough_abrupt_kind_const);
        exception_state_slots.initialize_all_to_null(&mut fb, ptr_ty);

        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let entry_failure_block = pre_cleanup_null_blocks[0];
        let entry_failure_args = Vec::new();
        assert_eq!(
            direct_entry_args.len(),
            function.body_params().len(),
            "direct JIT entry arity does not match entry params",
        );
        let entry_runtime_param_names = runtime_block_params[0]
            .iter()
            .map(|param| param.binding.name.as_str())
            .collect::<HashSet<_>>();
        let entry_stack_seed_param_names = entry_materializations[0]
            .iter()
            .filter_map(|entry| {
                matches!(entry.source, PlannedLocalEnvEntrySource::StackSlotLoad)
                    .then_some(entry.binding.name.as_str())
            })
            .collect::<HashSet<_>>();
        let entry_runtime_param_ref_kinds = entry_materializations[0]
            .iter()
            .filter_map(|entry| {
                matches!(entry.source, PlannedLocalEnvEntrySource::BlockParam { .. })
                    .then_some((entry.binding.name.clone(), entry.entry_ref_kind))
            })
            .collect::<HashMap<_, _>>();
        let entry_cleanup_root_states = jit_local_plan
            .cleanup_root_slot_states
            .entry_state_for_block(function.entry_block().label);
        let mut entry_param_values = HashMap::new();
        for (param, value) in function.body_params().iter().zip(direct_entry_args.iter()) {
            let needs_runtime_arg = entry_runtime_param_names.contains(param.name.as_str());
            let needs_stack_seed = entry_stack_seed_param_names.contains(param.name.as_str());
            let needs_cleanup_root = stack_slots.has_cleanup_root_name(param.name.as_str());
            let entry_ref_kind = entry_runtime_param_ref_kinds
                .get(&param.name)
                .copied()
                .unwrap_or(LocalRefKind::Borrowed);
            let should_materialize_cleanup_root = needs_cleanup_root
                && entry_cleanup_root_states
                    .get(&param.name)
                    .copied()
                    .unwrap_or(CleanupRootSlotState::NoOwnedReference)
                    .may_hold_owned_reference();

            if should_materialize_cleanup_root {
                stack_slots
                    .replace_cloned_value_with_previous_state_counted(
                        &mut fb,
                        param.name.as_str(),
                        *value,
                        LocalRefKind::Borrowed,
                        CleanupRootSlotState::NoOwnedReference,
                        ptr_ty,
                        thread_state_value,
                        incref_ref,
                        decref_ref,
                        refcounts,
                        None,
                        Some(RefcountDecrefLocationCounterParts {
                            counter_refs: &refcount_decref_location_counter_refs,
                            counter_slots_by_id,
                            scalar_counter_base_value,
                        }),
                    )
                    .expect("entry cleanup-root slot missing from stack slots");
            }

            if needs_stack_seed && !needs_runtime_arg && !needs_cleanup_root {
                with_refcount_family(&mut fb, Some(RefcountFamily::EntryArgClone), |fb| {
                    emit_incref_if_not_null(fb, ptr_ty, incref_ref, *value);
                });
                stack_slots
                    .replace_cloned_value_counted(
                        &mut fb,
                        param.name.as_str(),
                        *value,
                        LocalRefKind::Owned,
                        ptr_ty,
                        thread_state_value,
                        incref_ref,
                        decref_ref,
                        refcounts,
                        None,
                        Some(RefcountDecrefLocationCounterParts {
                            counter_refs: &refcount_decref_location_counter_refs,
                            counter_slots_by_id,
                            scalar_counter_base_value,
                        }),
                    )
                    .expect("entry slot missing from stack slots");
                fb.ins().call(decref_ref, &[thread_state_value, *value]);
            }

            if needs_runtime_arg {
                if !should_materialize_cleanup_root && transient_local_needs_decref(entry_ref_kind)
                {
                    with_refcount_family(&mut fb, Some(RefcountFamily::EntryArgClone), |fb| {
                        emit_incref_if_not_null(fb, ptr_ty, incref_ref, *value);
                    });
                }
                entry_param_values.insert(param.name.as_str(), *value);
            }
        }
        for block_param in function.blocks[0].bb_params() {
            if !entry_runtime_param_names.contains(block_param.name.as_str())
                || entry_param_values.contains_key(block_param.name.as_str())
            {
                continue;
            }
            let value = match block_param.role {
                BlockParamRole::AbruptKind => {
                    let fallthrough_tag = abrupt_kind_tag(AbruptKind::Fallthrough);
                    let fallthrough_i64 = fb.ins().iconst(ir::types::I64, fallthrough_tag);
                    let value_inst = fb.ins().call(py_long_from_i64_ref, &[fallthrough_i64]);
                    let value = fb.inst_results(value_inst)[0];
                    let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
                    let value_ok_block = fb.create_block();
                    fb.append_block_param(value_ok_block, ptr_ty);
                    fb.ins().brif(
                        value_is_null,
                        entry_failure_block,
                        &block_arg_values(&entry_failure_args),
                        value_ok_block,
                        &[ir::BlockArg::Value(value)],
                    );
                    fb.switch_to_block(value_ok_block);
                    fb.block_params(value_ok_block)[0]
                }
                BlockParamRole::AbruptPayload => emit_owned_module_constant_from_parts(
                    &mut fb,
                    none_constant_id,
                    &module_constant_object_globals,
                    ptr_ty,
                    &options.module_constant_accesses,
                ),
                BlockParamRole::Value => null_ptr,
                BlockParamRole::Exception => null_ptr,
            };
            entry_param_values.insert(block_param.name.as_str(), value);
        }
        for param in &runtime_block_params[0] {
            if entry_param_values.contains_key(param.binding.name.as_str()) {
                continue;
            }
            if param.binding.param_facts.binding == ParamBindingFacts::MaybeUnbound
                && param.binding.param_facts.ownership == LocalRefKind::Unbound
            {
                entry_param_values.insert(param.binding.name.as_str(), null_ptr);
            }
        }
        let preserved_values_base_value =
            if function
                .storage_layout()
                .as_ref()
                .is_some_and(|layout| !layout.preserved_slots.is_empty())
            {
                let preserved_state = entry_param_values.get("_dp_state").copied().ok_or_else(
                    || {
                        format!(
                            "preserved-state function {} ({}) is missing direct entry _dp_state",
                            function.function_id, function.names.qualname
                        )
                    },
                )?;
                let values_inst = fb.ins().call(preserved_values_ptr_ref, &[preserved_state]);
                let values = fb.inst_results(values_inst)[0];
                let values_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, values, null_ptr);
                let values_ok_block = fb.create_block();
                fb.append_block_param(values_ok_block, ptr_ty);
                fb.ins().brif(
                    values_is_null,
                    entry_failure_block,
                    &block_arg_values(&entry_failure_args),
                    values_ok_block,
                    &[ir::BlockArg::Value(values)],
                );
                fb.switch_to_block(values_ok_block);
                Some(fb.block_params(values_ok_block)[0])
            } else {
                None
            };
        if let Some(layout) = function.storage_layout().as_ref()
            && !layout.preserved_slots.is_empty()
        {
            tracing::info!(
                target: "soac_generator_preserved_layout",
                function_id = ?function.function_id,
                qualname = function.names.qualname.as_str(),
                preserved_slots = ?layout
                    .preserved_slots
                    .iter()
                    .map(|slot| (
                        slot.logical_name.as_str(),
                        slot.storage_name.as_str(),
                        slot.storage,
                        slot.init.clone(),
                    ))
                    .collect::<Vec<_>>(),
                "jit_resume_body_preserved_layout",
            );
        }
        let entry_jump_args = runtime_block_params[0]
            .iter()
            .map(|param| {
                if param.repr != RuntimeBlockParamRepr::PyObject {
                    return Err(format!(
                        "entry runtime block param {} ({}) unexpectedly uses {:?}",
                        param.arg_name, param.binding.name, param.repr
                    ));
                }
                entry_param_values
                    .get(param.binding.name.as_str())
                    .copied()
                    .map(ir::BlockArg::Value)
                    .ok_or_else(|| {
                        format!(
                            "missing direct entry value for runtime block param {} ({})",
                            param.arg_name, param.binding.name
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        fb.ins().jump(exec_blocks[0], &entry_jump_args);

        let mut exception_dispatch_blocks: Vec<Option<ir::Block>> = vec![None; exec_blocks.len()];
        let mut pending_local_failure_cleanups = Vec::new();
        let mut local_failure_cleanup_blocks = HashMap::new();
        let empty_cleanup_root_state = HashMap::new();
        let cleanup_root_union_exit_state =
            jit_local_plan.cleanup_root_slot_states.union_exit_states();
        let empty_cleanup_root_facts = HashMap::new();
        let cleanup_root_union_exit_facts =
            jit_local_plan.cleanup_root_slot_states.union_exit_facts();
        for (index, maybe_dispatch) in exc_dispatches.iter().enumerate() {
            if let Some(dispatch_plan) = maybe_dispatch {
                let dispatch_block = fb.create_block();
                for _ in &dispatch_plan.forwarded_local_names {
                    fb.append_block_param(dispatch_block, ptr_ty);
                }
                register_block_display_annotation(
                    &mut block_annotations,
                    dispatch_block,
                    format!("exc_dispatch::{}", function.blocks[index].label),
                    dispatch_plan.forwarded_local_names.clone(),
                );
                exception_dispatch_blocks[index] = Some(dispatch_block);
            }
        }

        for (index, block) in exec_blocks.iter().enumerate() {
            fb.switch_to_block(*block);
            let codegen_block = &function.blocks[index];
            if machine_deopt_suppressed_blocks.contains(&codegen_block.label) {
                fb.ins().trap(DEOPT_SUPPRESSED_FALLBACK_TRAP);
                continue;
            }
            let mut local_env = LocalEnv::default();
            let block_param_values = fb.block_params(*block).to_vec();
            bind_planned_local_env_at_block_entry(
                &mut fb,
                jit_local_plan,
                index,
                &block_param_values,
                &mut local_env,
                &stack_slots,
                Some(RefcountDecrefLocationCounterParts {
                    counter_refs: &refcount_decref_location_counter_refs,
                    counter_slots_by_id,
                    scalar_counter_base_value,
                }),
                ptr_ty,
                thread_state_value,
                incref_ref,
                decref_ref,
                refcounts,
                matches!(function.kind, FunctionKind::Function),
            )?;
            let block_const = globals_value;
            let fast_step_null_block =
                exception_dispatch_blocks[index].unwrap_or(pre_cleanup_null_blocks[index]);
            let fast_step_null_args = Vec::new();
            let emit_ctx = JitEmitCtx {
                module,
                function_id: function.function_id,
                function_kind: function.kind,
                indexed_field_guards_by_instr: &indexed_field_guards_by_instr,
                module_constants,
                value_facts,
                deopt_resume_plan: jit_deopt_resume_plan,
                runtime_supported_deopt_resume_points: options
                    .runtime_supported_deopt_resume_points
                    .as_deref(),
                refcount_plan,
                cleanup_root_slot_states: &jit_local_plan.cleanup_root_slot_states,
                truthiness_only_local_locations: &jit_local_plan.truthiness_only_local_locations,
                return_cleanup_blocks_by_label: &return_cleanup_blocks_by_label,
                instr_locations: &instr_locations,
                counter_slots_by_id,
                storage_layout: function.storage_layout().clone(),
                function_runtime_data_layout: &function_runtime_data_layout,
                incref_ref,
                decref_ref,
                refcount_lowering,
                py_call_positional_three_ref,
                py_vectorcall_ref,
                py_handle_pending_ref,
                handle_pending_checks_enabled,
                refcount_emission_enabled: env_config.jit_refcount_emission_enabled(),
                consts: JitEmitConsts {
                    step_null_block: fast_step_null_block,
                    step_null_args: fast_step_null_args,
                    ptr_ty,
                    i64_ty,
                    i32_ty: ir::types::I32,
                    function_env_value: fn_env_value,
                    function_data_value,
                    module_constant_object_globals: module_constant_object_globals.clone(),
                    scalar_counter_base_value,
                    top_value_counter_base_value,
                    thread_state_value,
                    preserved_values_base_value,
                    none_constant_id,
                    true_constant_id,
                    false_constant_id,
                    empty_tuple_constant_id,
                    block_const,
                    module_constant_accesses: options.module_constant_accesses.clone(),
                },
                load_global_fast_ref,
                probe_global_indexed_ref,
                load_global_slow_ref,
                guard_miss_deopt_stub_ref,
                guard_miss_deopt_instr_ids: &guard_miss_deopt_instr_ids,
                guard_miss_deopt_without_refcounts_instr_ids:
                    &guard_miss_deopt_without_refcounts_instr_ids,
                guard_miss_resume_point: None,
                load_runtime_obj_by_id_ref,
                enter_recursive_ref,
                direct_compile_function_env_ref,
                pytype_generic_alloc_ref,
                finish_constructor_init_ref,
                pyobject_getattr_ref,
                pyobject_setattr_ref,
                pyobject_getitem_ref,
                pyobject_setitem_ref,
                del_preserved_ref,
                del_preserved_quietly_ref,
                pyobject_to_i64_ref,
                py_long_from_i64_ref,
                raise_unbound_local_error_ref,
                make_function_with_closure_ref,
                make_cell_ref,
                load_cell_ref,
                store_cell_ref,
                py_call_object_ref,
                py_call_with_kw_ref,
                record_top_value_sample_ref,
                tuple_new_ref,
                tuple_set_item_ref,
                stack_slots: stack_slots.clone(),
                exception_state_slots: exception_state_slots.clone(),
                pop_handled_exception_ref,
                direct_edge_stats: &direct_edge_stats,
                direct_call_target_functions: &direct_call_target_functions,
                direct_call_functions,
                call_target_counter_ids: &call_target_counter_ids,
                call_direct_target_counter_ids: &call_direct_target_counter_ids,
                call_direct_hit_counter_ids: &call_direct_hit_counter_ids,
                call_direct_fallback_counter_ids: &call_direct_fallback_counter_ids,
                operator_shape_counter_ids: &operator_shape_counter_ids,
                getitem_shape_counter_ids: &getitem_shape_counter_ids,
                getitem_specialized_hit_counter_ids: &getitem_specialized_hit_counter_ids,
                getitem_specialized_fallback_counter_ids: &getitem_specialized_fallback_counter_ids,
                getitem_specialized_hit_counter_ids_by_source:
                    &getitem_specialized_hit_counter_ids_by_source,
                getitem_specialized_fallback_counter_ids_by_source:
                    &getitem_specialized_fallback_counter_ids_by_source,
                setitem_shape_counter_ids: &setitem_shape_counter_ids,
                setitem_specialized_hit_counter_ids: &setitem_specialized_hit_counter_ids,
                setitem_specialized_fallback_counter_ids: &setitem_specialized_fallback_counter_ids,
                setitem_specialized_hit_counter_ids_by_source:
                    &setitem_specialized_hit_counter_ids_by_source,
                setitem_specialized_fallback_counter_ids_by_source:
                    &setitem_specialized_fallback_counter_ids_by_source,
                global_indexed_hit_counter_ids: &global_indexed_hit_counter_ids,
                global_indexed_fallback_counter_ids: &global_indexed_fallback_counter_ids,
                field_indexed_hit_counter_ids: &field_indexed_hit_counter_ids,
                field_indexed_fallback_counter_ids: &field_indexed_fallback_counter_ids,
                field_indexed_hit_counter_ids_by_source: &field_indexed_hit_counter_ids_by_source,
                field_indexed_fallback_counter_ids_by_source:
                    &field_indexed_fallback_counter_ids_by_source,
                field_generic_getattr_counter_ids: &field_generic_getattr_counter_ids,
                field_generic_setattr_counter_ids: &field_generic_setattr_counter_ids,
                deopt_entry_guard_miss_counter_ids: &deopt_entry_guard_miss_counter_ids,
                branch_outcome_counter_ids: &branch_outcome_counter_ids,
                refcount_decref_location_counter_refs: &refcount_decref_location_counter_refs,
                allow_local_only_slot_backed_stores: true,
                exception_forwarded_local_names: exc_dispatches[index]
                    .as_ref()
                    .map(|dispatch| dispatch.forwarded_local_names.as_slice()),
                type_ptr_data_ids: RefCell::new(HashMap::new()),
                callable_ptr_data_ids: RefCell::new(HashMap::new()),
            };
            debug_assert!(
                emit_ctx
                    .deopt_resume_plan
                    .deopt_points_for_block(codegen_block.label, &instr_locations)
                    .all(|point| point.id.function_id == function.function_id)
            );
            emit_ctx.require_deopt_point_at_block_entry(codegen_block.label)?;
            let _block_refcount_plan = emit_ctx.refcount_plan.block(codegen_block.label);

            emit_typed_codegen_ops(
                &mut fb,
                &typed_function.blocks[index].body,
                &mut local_env,
                &stack_slots,
                &emit_ctx,
                cleanup_null_blocks[index],
                &mut pending_local_failure_cleanups,
                &mut local_failure_cleanup_blocks,
                &mut block_roles,
                codegen_env,
                &mut func_imports,
            )?;
            emit_ctx.require_deopt_point_before_term(codegen_block.label)?;

            let term_emit_ctx = local_failure_cleanup_emit_ctx(
                &mut fb,
                &emit_ctx,
                &local_env,
                cleanup_null_blocks[index],
                &mut pending_local_failure_cleanups,
                &mut local_failure_cleanup_blocks,
                &mut block_roles,
            )?;
            let term_emit_ctx = term_emit_ctx.as_ref().unwrap_or(&emit_ctx);
            emit_typed_codegen_term(
                &mut fb,
                codegen_block.label,
                &typed_function.blocks[index].term,
                function,
                &exec_blocks,
                &block_indices_by_label,
                jump_edge_transports,
                implicit_target_transports,
                &mut local_env,
                term_emit_ctx,
                codegen_env,
                &mut func_imports,
                pyobject_to_i64_ref,
                raise_exc_ref,
                codegen_block.exception_param(),
            )?;
            continue;
        }

        for (index, maybe_dispatch_block) in exception_dispatch_blocks.iter().enumerate() {
            let Some(dispatch_block) = *maybe_dispatch_block else {
                continue;
            };
            let Some(dispatch_plan) = exc_dispatches[index].as_ref() else {
                continue;
            };

            fb.switch_to_block(dispatch_block);
            let forwarded_local_values = fb.block_params(dispatch_block).to_vec();
            let null_ptr = fb.ins().iconst(ptr_ty, 0);
            let dispatch_step_null_args = Vec::new();

            let raised_exc =
                emit_take_current_raised_exception(&mut fb, ptr_ty, thread_state_value);
            let raised_exc_null = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, raised_exc, null_ptr);
            let raised_exc_ok = fb.create_block();
            fb.append_block_param(raised_exc_ok, ptr_ty);
            fb.ins().brif(
                raised_exc_null,
                pre_cleanup_null_blocks[index],
                &dispatch_step_null_args,
                raised_exc_ok,
                &[ir::BlockArg::Value(raised_exc)],
            );

            fb.switch_to_block(raised_exc_ok);
            let dispatch_exc = fb.block_params(raised_exc_ok)[0];
            if let Some(exception_name) =
                function.blocks[dispatch_plan.target_index].exception_param()
            {
                if let Some((previous_slot, is_pushed_slot)) =
                    exception_state_slots.slots_for_exception(exception_name)
                {
                    let previous_inst = fb.ins().call(push_handled_exception_ref, &[dispatch_exc]);
                    let previous = fb.inst_results(previous_inst)[0];
                    fb.ins().stack_store(previous, previous_slot, 0);
                    let is_pushed = fb.ins().iconst(ir::types::I64, 1);
                    fb.ins().stack_store(is_pushed, is_pushed_slot, 0);
                }
            }
            let slot_write_none_const = emit_owned_module_constant_from_parts(
                &mut fb,
                none_constant_id,
                &module_constant_object_globals,
                ptr_ty,
                &options.module_constant_accesses,
            );
            emit_exception_dispatch_slot_writes(
                &mut fb,
                &dispatch_plan.slot_writes,
                &dispatch_plan.forwarded_local_names,
                &forwarded_local_values,
                dispatch_exc,
                &stack_slots,
                jit_local_plan
                    .cleanup_root_slot_states
                    .block_exit_states
                    .get(&function.blocks[index].label)
                    .unwrap_or(&empty_cleanup_root_state),
                jit_local_plan
                    .cleanup_root_slot_states
                    .block_exit_facts
                    .get(&function.blocks[index].label)
                    .unwrap_or(&empty_cleanup_root_facts),
                Some(RefcountDecrefLocationCounterParts {
                    counter_refs: &refcount_decref_location_counter_refs,
                    counter_slots_by_id,
                    scalar_counter_base_value,
                }),
                ptr_ty,
                thread_state_value,
                slot_write_none_const,
                incref_ref,
                decref_ref,
                refcounts,
            )?;
            emit_exception_dispatch_forwarded_decrefs(
                &mut fb,
                &dispatch_plan.forwarded_local_names,
                &forwarded_local_values,
                &dispatch_plan.release_local_names,
                "release",
                ptr_ty,
                thread_state_value,
                decref_ref,
            )?;
            let source_label = function.blocks[index].label;
            let release_reason = RefcountReleaseReason::ExceptionEdge {
                target: function.blocks[dispatch_plan.target_index].label,
            };
            let forwarded_locations = function
                .storage_layout()
                .as_ref()
                .map(|layout| {
                    local_locations_for_names(layout, &dispatch_plan.forwarded_local_names)
                })
                .unwrap_or_default();
            emit_planned_stack_slot_releases_for_reason_from_parts(
                &mut fb,
                source_label,
                &release_reason,
                &forwarded_locations,
                refcount_plan,
                &stack_slots,
                &refcount_decref_location_counter_refs,
                counter_slots_by_id,
                scalar_counter_base_value,
                ptr_ty,
                thread_state_value,
                decref_ref,
                refcounts,
            )?;
            let target_arg_none_const = emit_owned_module_constant_from_parts(
                &mut fb,
                none_constant_id,
                &module_constant_object_globals,
                ptr_ty,
                &options.module_constant_accesses,
            );
            let target_jump_args = emit_exception_dispatch_target_args(
                &mut fb,
                &dispatch_plan.target_args,
                &dispatch_plan.forwarded_local_names,
                &forwarded_local_values,
                dispatch_exc,
                module_constants,
                &module_constant_object_globals,
                ptr_ty,
                &options.module_constant_accesses,
                thread_state_value,
                target_arg_none_const,
                incref_ref,
                decref_ref,
            )?;
            emit_exception_dispatch_forwarded_decrefs(
                &mut fb,
                &dispatch_plan.forwarded_local_names,
                &forwarded_local_values,
                &dispatch_plan.drop_forwarded_local_names,
                "drop",
                ptr_ty,
                thread_state_value,
                decref_ref,
            )?;
            fb.ins()
                .jump(exec_blocks[dispatch_plan.target_index], &target_jump_args);
        }

        for (cleanup_block, _, cleanup_root_states, cleanup_root_facts) in
            &return_cleanup_block_states
        {
            fb.switch_to_block(*cleanup_block);
            let ret_value = fb.block_params(*cleanup_block)[0];
            let mut popped_exception_names = HashSet::new();
            for block in &function.blocks {
                let Some(exception_name) = block.exception_param() else {
                    continue;
                };
                if !popped_exception_names.insert(exception_name) {
                    continue;
                }
                let Some((previous_slot, is_pushed_slot)) =
                    exception_state_slots.slots_for_exception(exception_name)
                else {
                    continue;
                };
                let is_pushed = fb.ins().stack_load(ir::types::I64, is_pushed_slot, 0);
                let should_pop = fb
                    .ins()
                    .icmp_imm(ir::condcodes::IntCC::NotEqual, is_pushed, 0);
                let pop_block = fb.create_block();
                let done_block = fb.create_block();
                fb.ins().brif(should_pop, pop_block, &[], done_block, &[]);

                fb.switch_to_block(pop_block);
                let previous = fb.ins().stack_load(ptr_ty, previous_slot, 0);
                fb.ins().call(pop_handled_exception_ref, &[previous]);
                let null_ptr = fb.ins().iconst(ptr_ty, 0);
                fb.ins().stack_store(null_ptr, previous_slot, 0);
                let not_pushed = fb.ins().iconst(ir::types::I64, 0);
                fb.ins().stack_store(not_pushed, is_pushed_slot, 0);
                fb.ins().jump(done_block, &[]);

                fb.switch_to_block(done_block);
            }
            stack_slots.decref_all_with_cleanup_root_states_counted(
                &mut fb,
                ptr_ty,
                thread_state_value,
                decref_ref,
                refcounts,
                cleanup_root_states,
                cleanup_root_facts,
                Some(RefcountDecrefLocationCounterParts {
                    counter_refs: &refcount_decref_location_counter_refs,
                    counter_slots_by_id,
                    scalar_counter_base_value,
                }),
            );
            fb.ins().return_(&[ret_value]);
        }

        for cleanup in &pending_local_failure_cleanups {
            fb.switch_to_block(cleanup.block);
            let cleanup_params = fb.block_params(cleanup.block).to_vec();
            let cleanup_values = &cleanup_params[..cleanup.cleanup_arg_count];
            for (&value, action) in cleanup_values.iter().zip(&cleanup.cleanup_actions) {
                match action {
                    PendingLocalFailureCleanupAction::Decref => {
                        emit_decref_if_not_null(
                            &mut fb,
                            ptr_ty,
                            decref_ref,
                            thread_state_value,
                            value,
                        );
                    }
                    PendingLocalFailureCleanupAction::RetireFrameRoot { name, ref_kind } => {
                        stack_slots
                            .replace_transferred_value_counted(
                                &mut fb,
                                name.as_str(),
                                value,
                                *ref_kind,
                                ptr_ty,
                                thread_state_value,
                                incref_ref,
                                decref_ref,
                                refcounts,
                                None,
                                Some(RefcountDecrefLocationCounterParts {
                                    counter_refs: &refcount_decref_location_counter_refs,
                                    counter_slots_by_id,
                                    scalar_counter_base_value,
                                }),
                            )
                            .ok_or_else(|| {
                                format!(
                                    "failure cleanup references missing frame-root slot {name:?}"
                                )
                            })?;
                    }
                }
            }
            match cleanup.continuation {
                PendingLocalFailureContinuation::CleanupNull(cleanup_null_block) => {
                    let error_value = emit_take_current_raised_exception_or_trap(
                        &mut fb,
                        ptr_ty,
                        thread_state_value,
                    );
                    fb.ins()
                        .jump(cleanup_null_block, &[ir::BlockArg::Value(error_value)]);
                }
                PendingLocalFailureContinuation::ExceptionDispatch(dispatch_block) => {
                    let forwarded_args =
                        block_arg_values(&cleanup_params[cleanup.cleanup_arg_count..]);
                    fb.ins().jump(dispatch_block, &forwarded_args);
                }
            }
        }

        if let Some((pre_cleanup, cleanup)) = shared_null_cleanup {
            fb.switch_to_block(pre_cleanup);
            let error_value =
                emit_take_current_raised_exception_or_trap(&mut fb, ptr_ty, thread_state_value);
            fb.ins().jump(cleanup, &[ir::BlockArg::Value(error_value)]);
        }
        for (_, pre_cleanup, cleanup) in &per_exception_null_cleanup_blocks {
            fb.switch_to_block(*pre_cleanup);
            let error_value =
                emit_take_current_raised_exception_or_trap(&mut fb, ptr_ty, thread_state_value);
            fb.ins().jump(*cleanup, &[ir::BlockArg::Value(error_value)]);
        }

        if let Some((_, cleanup)) = shared_null_cleanup {
            fb.switch_to_block(cleanup);
            let error_value = fb.block_params(cleanup)[0];
            let restore_error = fb.ins().iconst(ir::types::I64, 1);
            let null_ptr = fb.ins().iconst(ptr_ty, 0);
            fb.ins().jump(
                exception_exit_sweep_block,
                &[
                    ir::BlockArg::Value(restore_error),
                    ir::BlockArg::Value(error_value),
                    ir::BlockArg::Value(null_ptr),
                ],
            );
        }

        for (index, _, cleanup) in &per_exception_null_cleanup_blocks {
            fb.switch_to_block(*cleanup);
            let error_value = fb.block_params(*cleanup)[0];
            let cleanup_args = fb.block_params(*cleanup)[1..].to_vec();
            for value in cleanup_args {
                emit_decref_if_not_null(&mut fb, ptr_ty, decref_ref, thread_state_value, value);
            }
            if let Some(exception_name) = function.blocks[*index].exception_param() {
                if let Some((previous_slot, is_pushed_slot)) =
                    exception_state_slots.slots_for_exception(exception_name)
                {
                    let is_pushed = fb.ins().stack_load(ir::types::I64, is_pushed_slot, 0);
                    let should_pop =
                        fb.ins()
                            .icmp_imm(ir::condcodes::IntCC::NotEqual, is_pushed, 0);
                    let pop_block = fb.create_block();
                    let done_block = fb.create_block();
                    register_block_role(&mut block_roles, pop_block, ClifBlockRole::Cleanup);
                    register_block_role(&mut block_roles, done_block, ClifBlockRole::Cleanup);
                    fb.ins().brif(should_pop, pop_block, &[], done_block, &[]);

                    fb.switch_to_block(pop_block);
                    let previous = fb.ins().stack_load(ptr_ty, previous_slot, 0);
                    fb.ins().call(pop_handled_exception_ref, &[previous]);
                    let null_ptr = fb.ins().iconst(ptr_ty, 0);
                    fb.ins().stack_store(null_ptr, previous_slot, 0);
                    let not_pushed = fb.ins().iconst(ir::types::I64, 0);
                    fb.ins().stack_store(not_pushed, is_pushed_slot, 0);
                    fb.ins().jump(done_block, &[]);

                    fb.switch_to_block(done_block);
                }
            }
            let restore_error = fb.ins().iconst(ir::types::I64, 1);
            let null_ptr = fb.ins().iconst(ptr_ty, 0);
            fb.ins().jump(
                exception_exit_sweep_block,
                &[
                    ir::BlockArg::Value(restore_error),
                    ir::BlockArg::Value(error_value),
                    ir::BlockArg::Value(null_ptr),
                ],
            );
        }

        fb.switch_to_block(step_null_block);
        let step_null_args = fb.block_params(step_null_block)[0];
        let error_value =
            emit_take_current_raised_exception_or_trap(&mut fb, ptr_ty, thread_state_value);
        let restore_error_after_decref = fb.ins().iconst(ir::types::I64, 2);
        fb.ins().jump(
            exception_exit_sweep_block,
            &[
                ir::BlockArg::Value(restore_error_after_decref),
                ir::BlockArg::Value(error_value),
                ir::BlockArg::Value(step_null_args),
            ],
        );

        fb.switch_to_block(raise_exc_direct_block);
        let red_args = fb.block_params(raise_exc_direct_block)[0];
        let red_exc = fb.block_params(raise_exc_direct_block)[1];
        let red_null = fb.ins().iconst(ptr_ty, 0);
        let red_exc_null = fb
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, red_exc, red_null);
        let red_set_block = fb.create_block();
        fb.append_block_param(red_set_block, ptr_ty);
        let red_done_block = fb.create_block();
        fb.ins().brif(
            red_exc_null,
            red_done_block,
            &[],
            red_set_block,
            &[ir::BlockArg::Value(red_exc)],
        );
        fb.switch_to_block(red_set_block);
        let red_set_exc = fb.block_params(red_set_block)[0];
        let _ = fb.ins().call(raise_exc_ref, &[red_set_exc]);
        fb.ins()
            .call(decref_ref, &[thread_state_value, red_set_exc]);
        fb.ins().jump(red_done_block, &[]);
        fb.switch_to_block(red_done_block);
        fb.ins().call(decref_ref, &[thread_state_value, red_args]);
        let keep_current_error = fb.ins().iconst(ir::types::I64, 0);
        fb.ins().jump(
            exception_exit_sweep_block,
            &[
                ir::BlockArg::Value(keep_current_error),
                ir::BlockArg::Value(red_null),
                ir::BlockArg::Value(red_null),
            ],
        );

        fb.switch_to_block(exception_exit_sweep_block);
        let kind = fb.block_params(exception_exit_sweep_block)[0];
        let error_value = fb.block_params(exception_exit_sweep_block)[1];
        let post_sweep_decref = fb.block_params(exception_exit_sweep_block)[2];
        stack_slots.decref_all_with_cleanup_root_states_counted(
            &mut fb,
            ptr_ty,
            thread_state_value,
            decref_ref,
            refcounts,
            &cleanup_root_union_exit_state,
            &cleanup_root_union_exit_facts,
            Some(RefcountDecrefLocationCounterParts {
                counter_refs: &refcount_decref_location_counter_refs,
                counter_slots_by_id,
                scalar_counter_base_value,
            }),
        );
        let should_keep_current_error = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, kind, 0);
        let restore_or_post_decref_block = fb.create_block();
        register_block_display_annotation(
            &mut block_annotations,
            restore_or_post_decref_block,
            "cleanup_exception_dispatch",
            Vec::new(),
        );
        register_block_role(
            &mut block_roles,
            restore_or_post_decref_block,
            ClifBlockRole::Cleanup,
        );
        fb.ins().brif(
            should_keep_current_error,
            exception_exit_return_block,
            &[],
            restore_or_post_decref_block,
            &[],
        );

        fb.switch_to_block(restore_or_post_decref_block);
        let should_decref_after_sweep = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, kind, 2);
        fb.ins().brif(
            should_decref_after_sweep,
            exception_exit_post_sweep_decref_block,
            &[
                ir::BlockArg::Value(error_value),
                ir::BlockArg::Value(post_sweep_decref),
            ],
            exception_exit_restore_block,
            &[ir::BlockArg::Value(error_value)],
        );

        fb.switch_to_block(exception_exit_post_sweep_decref_block);
        let error_value = fb.block_params(exception_exit_post_sweep_decref_block)[0];
        let post_sweep_decref = fb.block_params(exception_exit_post_sweep_decref_block)[1];
        fb.ins()
            .call(decref_ref, &[thread_state_value, post_sweep_decref]);
        fb.ins().jump(
            exception_exit_restore_block,
            &[ir::BlockArg::Value(error_value)],
        );

        fb.switch_to_block(exception_exit_restore_block);
        let error_value = fb.block_params(exception_exit_restore_block)[0];
        fb.ins()
            .call(set_raised_exception_ref, &[thread_state_value, error_value]);
        fb.ins().jump(exception_exit_return_block, &[]);

        fb.switch_to_block(exception_exit_return_block);
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        fb.ins().return_(&[null_ptr]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    direct_edge_stats.emit_trace(
        direct_call_resolver
            .map(|shared_state| shared_state.module_name.as_str())
            .unwrap_or("<standalone>"),
        function,
    );

    let mut direct_func_id_to_qualname = HashMap::new();
    direct_func_id_to_qualname.insert(main_id.as_u32(), function.names.qualname.clone());
    if let Some(default_adapter_id) = default_adapter_id {
        direct_func_id_to_qualname.insert(
            default_adapter_id.as_u32(),
            format!("{}:defaults", function.names.qualname),
        );
    }
    for (function_id, declared) in direct_call_functions {
        let qualname = if *function_id == function.function_id {
            Some(function.names.qualname.as_str())
        } else {
            module
                .callable_defs
                .iter()
                .find(|candidate| candidate.function_id == *function_id)
                .map(|candidate| candidate.names.qualname.as_str())
                .or_else(|| {
                    direct_call_target_functions
                        .get(function_id)
                        .map(|candidate| candidate.names.qualname.as_str())
                })
        };
        if let Some(qualname) = qualname {
            add_declared_direct_function_alias(&mut direct_func_id_to_qualname, declared, qualname);
        }
    }

    Ok(BuiltSpecializedFunction {
        ctx,
        main_id,
        main_symbol,
        default_adapter_id,
        default_adapter_symbol,
        import_id_to_symbol: module_imports.debug_symbols().clone(),
        local_func_id_to_symbol: module_imports.debug_declared_symbols().clone(),
        direct_func_id_to_qualname,
        #[cfg(test)]
        func_id_to_symbol: module_imports.debug_declared_symbols().clone(),
        block_annotations,
        block_roles,
    })
}

pub unsafe fn render_cranelift_run_bb_specialized_with_cfg(
    blocks: &[ObjPtr],
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>,
    module_constants: &ModuleCodegenConstants,
) -> Result<RenderedSpecializedClif, String> {
    unsafe {
        // Standalone debug rendering must not observe or mutate the process JIT session.
        let compile_session = crate::session::CompileSession::new();
        render_cranelift_run_bb_specialized_with_runtime_state_and_cfg(
            &compile_session,
            blocks,
            module,
            function,
            module_constants,
            None,
        )
    }
}

pub unsafe fn render_instr_typed_for_codegen_with_runtime_state(
    compile_session: &crate::session::CompileSession,
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>,
    runtime_state: Option<&SharedModuleState>,
) -> Result<String, String> {
    let builder = new_jit_builder(compile_session.env_config()?)?;
    let mut jit_module = JITModule::new(builder);
    let specialization_profile = SpecializationProfile::from_runtime_state_with_session(
        runtime_state,
        Some(compile_session),
    )?;
    predeclare_specialization_type_imports(&mut jit_module, &specialization_profile)?;
    let jit_module_plan = if let Some(shared_state) = runtime_state {
        optimize_blockpy_for_shared_state(
            shared_state,
            Some(compile_session),
            Some(&specialization_profile),
            compile_session.env_config()?,
        )?
    } else {
        optimize_blockpy(
            module,
            Some(&specialization_profile),
            compile_session.env_config()?,
        )?
    };
    let render_module = jit_module_plan.module.as_ref();
    let render_function = render_module
        .callable_defs
        .iter()
        .find(|candidate| candidate.function_id == function.function_id)
        .ok_or_else(|| {
            format!(
                "planned specialized JIT module is missing function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
    let PreparedSpecializedTypedFunction { typed_function } =
        prepare_specialized_typed_function(render_function, None, &jit_module_plan.value_facts)?;
    let direct_call_targets = collect_typed_call_direct_targets(&typed_function);
    let mut direct_call_target_functions = HashMap::new();
    for function_id in direct_call_targets {
        if render_module
            .callable_defs
            .iter()
            .any(|function| function.function_id == function_id)
            || direct_call_target_functions.contains_key(&function_id)
        {
            continue;
        }
        let Some(target_function) = runtime_state
            .map(|shared_state| {
                shared_state.lookup_direct_call_target_function(compile_session, function_id)
            })
            .transpose()?
            .flatten()
        else {
            continue;
        };
        direct_call_target_functions.insert(
            function_id,
            lower_blockpy_function_to_typed(target_function),
        );
    }

    let mut out = String::new();
    out.push_str("; ---- InstrTyped input to specialized codegen ----\n");
    out.push_str(&format!(
        "; module: {}\n",
        runtime_state
            .map(|shared_state| shared_state.module_name.as_str())
            .unwrap_or("<standalone>")
    ));
    out.push_str(&format!("; function: {}\n", render_function.names.qualname));
    out.push_str(&format!(
        "; function_id: {}\n\n",
        render_function.function_id
    ));
    out.push_str("; ---- InstrTyped program ----\n");
    out.push_str(&render_instr_typed_program(&typed_function));
    out.push('\n');
    out.push_str("; ---- typed metadata index, preorder ----\n");
    out.push_str(&render_instr_typed_metadata_index(&typed_function));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

pub unsafe fn render_cranelift_run_bb_specialized_with_runtime_state_and_cfg(
    compile_session: &crate::session::CompileSession,
    blocks: &[ObjPtr],
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>,
    module_constants: &ModuleCodegenConstants,
    runtime_state: Option<&SharedModuleState>,
) -> Result<RenderedSpecializedClif, String> {
    if blocks.is_empty() {
        return Err("specialized JIT run_bb requires at least one block".to_string());
    }

    let (mut jit_module, runtime_support_symbols) =
        new_jit_module_with_runtime_support_symbols(compile_session)?;
    let specialization_profile = SpecializationProfile::from_runtime_state_with_session(
        runtime_state,
        Some(compile_session),
    )?;
    predeclare_specialization_type_imports(&mut jit_module, &specialization_profile)?;
    let jit_module_plan = if let Some(shared_state) = runtime_state {
        optimize_blockpy_for_shared_state(
            shared_state,
            Some(compile_session),
            Some(&specialization_profile),
            compile_session.env_config()?,
        )?
    } else {
        optimize_blockpy(
            module,
            Some(&specialization_profile),
            compile_session.env_config()?,
        )?
    };
    let render_module = jit_module_plan.module.as_ref();
    let render_function = render_module
        .callable_defs
        .iter()
        .find(|candidate| candidate.function_id == function.function_id)
        .ok_or_else(|| {
            format!(
                "planned specialized JIT module is missing function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
    let placeholder_blocks;
    let render_blocks = if blocks.len() == render_function.blocks.len() {
        blocks
    } else {
        placeholder_blocks =
            vec![std::ptr::null_mut::<std::ffi::c_void>(); render_function.blocks.len()];
        placeholder_blocks.as_slice()
    };
    let planned_module_constants;
    let render_module_constants = if let Some(shared_state) = runtime_state {
        planned_module_constants = collect_codegen_constants_for_module_name(
            shared_state.module_name.as_str(),
            render_module,
        );
        &planned_module_constants
    } else {
        module_constants
    };
    let module_constant_owners = runtime_state
        .map(|shared_state| {
            Python::attach(|py| {
                crate::module_type::build_module_constant_objects(
                    py,
                    render_module_constants,
                    shared_state.module_name.as_str(),
                    shared_state.source_hash(),
                )
                .map_err(|err| err.to_string())
            })
        })
        .transpose()?;
    let module_constant_ptrs = module_constant_owners
        .as_ref()
        .map(|owners| owners.iter().map(|obj| obj.as_ptr()).collect::<Vec<_>>())
        .unwrap_or_else(|| placeholder_module_constant_ptrs(render_module_constants.len()));
    let counter_defs = runtime_state
        .map(|_| render_module.counter_defs.as_slice())
        .unwrap_or(module.counter_defs.as_slice());
    let (counter_slots_by_id, scalar_counter_count, top_value_counter_count) =
        build_counter_storage_layout(counter_defs)?;
    let module_constant_object_data_ids =
        declare_module_constant_object_data(&mut jit_module, render_module, &module_constant_ptrs)?;
    let scalar_counter_data_id = if scalar_counter_count == 0 {
        None
    } else {
        Some(define_scalar_counter_storage_data(
            &mut jit_module,
            render_module,
            scalar_counter_count,
        )?)
    };
    let top_value_counter_data_id = if top_value_counter_count == 0 {
        None
    } else if let Some(shared_state) = runtime_state {
        Some(declare_top_value_counter_storage_import(
            &mut jit_module,
            top_value_counter_storage_symbol_for_shared_state(shared_state).as_str(),
        )?)
    } else {
        Some(define_top_value_counter_storage_data(
            &mut jit_module,
            render_module,
            top_value_counter_count,
        )?)
    };
    let jit_local_plan = jit_module_plan
        .locals
        .function(render_function.function_id)
        .ok_or_else(|| {
            format!(
                "missing JIT local plan for function {} ({})",
                render_function.function_id, render_function.names.qualname
            )
        })?;
    let jit_deopt_resume_plan = jit_module_plan
        .deopt_resume
        .function(render_function.function_id)
        .ok_or_else(|| {
            format!(
                "missing JIT deopt resume plan for function {} ({})",
                render_function.function_id, render_function.names.qualname
            )
        })?;
    let PreparedSpecializedTypedFunction {
        typed_function: render_typed_function,
    } = prepare_specialized_typed_function(render_function, None, &jit_module_plan.value_facts)?;
    let mut predeclared_direct_functions = HashMap::new();
    let mut external_direct_call_target_functions = HashMap::new();
    for function_id in collect_typed_call_direct_targets(&render_typed_function) {
        if function_id == render_function.function_id
            || predeclared_direct_functions.contains_key(&function_id)
        {
            continue;
        }
        if let Some(target_function) = render_module
            .callable_defs
            .iter()
            .find(|function| function.function_id == function_id)
            .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
        {
            let declared =
                declare_imported_direct_function(&mut jit_module, target_function, "render")?;
            predeclared_direct_functions.insert(function_id, declared);
            continue;
        }
        let Some(target_function) = runtime_state
            .map(|shared_state| {
                shared_state.lookup_direct_call_target_function(compile_session, function_id)
            })
            .transpose()?
            .flatten()
            .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
        else {
            continue;
        };
        let target_function = lower_blockpy_function_to_typed(target_function);
        let declared =
            declare_imported_direct_function(&mut jit_module, &target_function, "render")?;
        predeclared_direct_functions.insert(function_id, declared);
        external_direct_call_target_functions.insert(function_id, target_function);
    }
    let built = build_cranelift_run_bb_specialized_function(
        &mut jit_module,
        render_blocks,
        render_module,
        render_function,
        &jit_module_plan.value_facts,
        jit_local_plan,
        jit_deopt_resume_plan,
        render_module_constants,
        counter_defs,
        module_constant_object_data_ids.as_slice(),
        counter_slots_by_id.as_ref(),
        scalar_counter_data_id,
        top_value_counter_data_id,
        compile_session,
        runtime_state,
        None,
        Some(&predeclared_direct_functions),
        BuildSpecializedFunctionOptions {
            external_direct_call_target_functions,
            ..BuildSpecializedFunctionOptions::default()
        },
    )?;
    let mut out = String::new();
    let function_aliases = clif_function_display_aliases(
        &built.import_id_to_symbol,
        &built.local_func_id_to_symbol,
        &runtime_support_symbols,
        &built.direct_func_id_to_qualname,
    );
    out.push_str("; function aliases (Cranelift display id -> readable name)\n");
    let mut symbols: Vec<String> = function_aliases
        .values()
        .map(|alias| alias.display_name.clone())
        .collect();
    symbols.sort_unstable();
    symbols.dedup();
    for symbol in symbols {
        out.push_str("; ");
        out.push_str(&symbol);
        out.push('\n');
    }
    out.push('\n');
    let pre_inline_clif = render_pre_inline_clif_for_inspection(
        &built.ctx.func,
        &function_aliases,
        &built.block_annotations,
    );
    let (compiled_clif, cfg_dot, vcode_disasm) = match render_compiled_clif_and_vcode_disasm(
        &mut jit_module,
        compile_session.env_config()?,
        built.ctx,
        &function_aliases,
        &built.block_annotations,
    ) {
        Ok(rendered) => rendered,
        Err(err) => return Err(err),
    };
    out.push_str(&compiled_clif);
    Ok(RenderedSpecializedClif {
        pre_inline_clif,
        clif: out,
        cfg_dot,
        vcode_disasm,
    })
}

pub(crate) unsafe fn compile_cranelift_run_bb_specialized_cached(
    compile_session: &Arc<crate::session::CompileSession>,
    blocks: &[ObjPtr],
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>,
    module_constants: &ModuleCodegenConstants,
    counter_defs: &[CounterDef],
    module_constant_ptrs: &[*mut ffi::PyObject],
    direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
) -> Result<DirectFunctionCompileResult, String> {
    unsafe {
        compile_session.process_jit()?.compile_direct_function(
            compile_session,
            blocks,
            module,
            function,
            module_constants,
            counter_defs,
            module_constant_ptrs,
            direct_call_resolver,
        )
    }
}

#[cfg(test)]
mod test;
