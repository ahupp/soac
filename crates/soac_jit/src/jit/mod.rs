use self::precompiled_object::{
    ElfSymbolBinding, ElfSymbolKind, ObjectDataDefinition, ObjectDataRelocation,
    ObjectFunctionDefinition, R_X86_64_64, write_precompiled_object,
};
use crate::SOAC_JIT_RUNTIME_CLIF;
use crate::config::{
    CraneliftTargetConfig, PythonModuleCacheSource, SpecializationMode,
    module_optimization_plan_v3_path, pre_optimization_module_cache_identity,
};
use crate::counter::TopValueCounter;
use crate::function_instantiation::{
    SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL, make_function_kind_abi_tag,
    soac_jit_make_function_with_closure,
};
use crate::module_constants::{ModuleCodegenConstants, ModuleConstantId};
use crate::module_type::{CounterRuntimeSlot, SharedModuleState, build_counter_storage_layout};
use cranelift_codegen::cfg_printer::CFGPrinter;
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::inline::{Inline, InlineCommand};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::isa::{OwnedTargetIsa, TargetFrontendConfig, TargetIsa};
#[cfg(test)]
use cranelift_codegen::settings::Configurable;
use cranelift_control::ControlPlane;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module, ModuleReloc};
use cranelift_reader::parse_functions;
use pyo3::{Py, PyAny, Python, ffi};
use soac_config::{RuntimeOptimizationPipeline, SoacEnvConfig};
use soac_core::block_py as blockpy_intrinsics;
use soac_core::block_py::{
    AbruptKind, Block, BlockArg, BlockEdge, BlockLabel, BlockParamRole, BlockPyFunction,
    BlockPyModule, BlockTerm, CallArgKeyword, CallArgPositional, CallableScopeKind, CellLocation,
    ChildVisitable, CounterBranchId, CounterDef, CounterId, CounterScope, CounterSite, Del,
    DeoptEntrySource, FunctionExecutionMode, FunctionKind, HasSemanticInstrId, InstrId, InstrKey,
    InstrLocationMap, LocalFunctionId, LocalLocation, ModuleContentId, ModuleShape, NameLocation,
    ParamKind, PersistentFunctionId, ResolvedName, RuntimeFunctionId, RuntimeModuleId, RuntimeName,
    SerializedFunctionId, StorageLayout, Store, Visit, VisitMut, current_instr_locations,
};
use soac_core::profile::{
    CollectedTypeKeyLayout, CounterDumpTypeKey, read_block_entry_counts_from_file,
};
use soac_instrument::{InstrumentationConfig, instrument_typed_module};
use soac_opt::access_emission_v3::{
    ExactListItemAccessPlan as OptV3ExactListItemAccessPlan,
    IndexedFieldAccessPlan as OptV3IndexedFieldAccessPlan,
    IndexedFieldLayoutGroup as OptV3IndexedFieldLayoutGroup,
    IndexedFieldRuntimeAccessRequest as OptV3IndexedFieldRuntimeAccessRequest,
    IndexedGlobalAccessPlan as OptV3IndexedGlobalAccessPlan,
    ResolvedIndexedFieldAccess as OptV3ResolvedIndexedFieldAccessFromOpt,
    exact_list_items_for_function_from_artifacts as opt_v3_emitted_exact_list_items_for_function,
    indexed_field_layout_groups as opt_v3_indexed_field_layout_groups,
    indexed_field_runtime_access_request as opt_v3_indexed_field_runtime_access_request,
    indexed_fields_for_function_from_artifacts as opt_v3_emitted_indexed_fields_for_function,
    indexed_globals_for_function_from_artifacts as opt_v3_emitted_indexed_globals_for_function,
    prepare_indexed_field_accesses_for_codegen as opt_v3_prepare_indexed_field_accesses_for_codegen,
};
use soac_opt::alternatives_v3::AlternativeCatalog;
use soac_opt::artifacts_v3::{
    ExactIntBranchV3Artifacts, load_optimization_artifacts_v3,
    single_function_optimization_artifacts_v3, validate_optimization_artifacts_v3_for_module,
};
use soac_opt::call_emission_v3::{
    ResolvedV3DirectCallPlan, direct_call_body_plans as opt_v3_direct_call_body_plans,
    direct_call_targets as opt_v3_direct_call_targets,
    direct_calls_for_function_from_artifacts as opt_v3_emitted_direct_calls_for_function,
    typed_call_emission_plans_from_v3,
};
use soac_opt::emit_v3::{
    MechanicalCodegenConversion, MechanicalCodegenOperation, MechanicalCodegenStep,
    MechanicalExitKind, MechanicalRegionEmission,
    mechanical_codegen_step as opt_v3_mechanical_codegen_step,
    mechanical_convert_inputs_for_output as opt_v3_mechanical_convert_inputs_for_output,
    mechanical_region_function_param_inputs as opt_v3_mechanical_region_function_param_inputs,
};
use soac_opt::passes::{
    CodegenModuleShape, FactStore, FunctionRefcountPlan, InstrCodegen, InstrTyped,
    LocalEnvResumeBinding, LocalEnvResumeBindingState, LocalEnvResumePoint,
    LocalEnvResumeStatePrecision, LocalEnvResumeValueSource, LocalRefState, PyExactType,
    PyObjFacts, RefcountActionKind, RefcountReleaseReason, RefcountSite, RuntimeHelperId,
    TypedAttrAccessPlan, TypedAttrOwnerRef, TypedBlock, TypedBlockLayoutHint, TypedCall,
    TypedCallAccessPlan, TypedCallEmissionPlans, TypedCodegenModuleShape, TypedDirectCallArgPlan,
    TypedDirectCallArgSource, TypedDirectCallGuardTest, TypedDirectCallGuardTestKind,
    TypedDirectCallableCall, TypedDirectCallableCallGuard, TypedDirectConstructorCallGuard,
    TypedDirectFunctionCallGuard, TypedDirectMethodCall, TypedDirectMethodCallGuard,
    TypedExactIntBranchPlan, TypedExactIntPlanSource, TypedExactIntReturnPlan,
    TypedExactIntScalarThreadPlan, TypedExactListItemAccessPlan, TypedExactListItemPlanSource,
    TypedGetAttr, TypedGuardedCallableCall, TypedGuardedMethodCall, TypedIndexedFieldGuard,
    TypedIndexedFieldPlanSource, TypedIndexedGlobalAccessPlan, TypedIndexedGlobalPlanSource,
    TypedPlannedResult, TypedPyObjectOwnershipPlan, TypedSetAttr, ValueFacts,
    annotate_typed_function_planned_results, annotate_typed_function_result_demands,
    annotate_typed_function_value_facts, annotate_typed_module_value_facts,
    assign_missing_typed_function_instr_ids, infer_module_value_facts,
    inline_typed_function_direct_call_stores, lower_codegen_function_to_typed,
    lower_codegen_module_to_typed, lower_typed_function_call_access_plan_instrs,
    lower_typed_function_call_emission_plans, lower_typed_if_tests_to_truthy,
    refresh_typed_function_value_facts, try_lower_typed_instr_to_codegen_legacy,
    validate_typed_function_call_access_plans, validate_typed_function_value_facts,
};
use soac_opt::pipeline_v3::plan_and_emit_module_v3_from_raw_evidence;
use soac_opt::plan::ProfileEvidenceStore;
use soac_opt::plan_v3::{
    CallBodyKind, IndexedFieldAccessKind as PlanV3IndexedFieldAccessKind,
    IndexedGlobalAccessKind as PlanV3IndexedGlobalAccessKind, MaterializeKind, ModulePlanIdentity,
    PlanNodeId, PlanValue, RegionId, RegionPlan, Rep, RichCompareOp,
};
use soac_opt::region_emission_v3::{
    ExactIntBranchSelection as OptV3ExactIntBranchSelection,
    ExactIntReturnSelection as OptV3ExactIntReturnSelection,
    ScalarThreadInlineReturnTargets as OptV3ScalarThreadInlineReturnTargets,
    ScalarThreadSelection as OptV3ScalarThreadSelection,
    exact_int_branch_selection_for_source as opt_v3_exact_int_branch_selection_for_source,
    exact_int_return_selection_for_source as opt_v3_exact_int_return_selection_for_source,
    scalar_thread_inline_return_targets as opt_v3_scalar_thread_inline_return_targets,
    scalar_thread_selection_for_store_branch as opt_v3_scalar_thread_selection_for_store_branch,
    scalar_thread_unmaterialized_local_location as opt_v3_scalar_thread_unmaterialized_local_location,
};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, CString, c_void};
use std::fs;
use std::mem::{MaybeUninit, offset_of};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

type CodegenBlock = Block<InstrCodegen>;
use tracing::{info, warn};

unsafe extern "C" {
    static mut PyFunction_Type: ffi::PyTypeObject;
    static mut PyMethod_Type: ffi::PyTypeObject;
    static mut PyType_Type: ffi::PyTypeObject;
    static mut PyLong_Type: ffi::PyTypeObject;
    static mut PyList_Type: ffi::PyTypeObject;
    static mut _PyDict_IndexedValueTombstone: i8;
    fn PyThreadState_GetUnchecked() -> *mut ffi::PyThreadState;
    fn PyUnstable_Type_AssignVersionTag(type_obj: *mut ffi::PyTypeObject) -> i32;
    fn _PyType_LookupRef(
        type_obj: *mut ffi::PyTypeObject,
        name: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
}

mod compiled;
mod deopt;
mod deopt_interpreter;
#[allow(unused_imports)]
pub(crate) use deopt_interpreter::{
    BlockPyEntryRuntimeContext, run_blockpy_function_from_entry,
    run_blockpy_function_from_vectorcall_entry,
};
mod direct_abi;
mod imports;
mod intrinsics;
mod jitdump;
mod module_data;
mod operation_specializations;
mod planning;
mod precompiled_object;
mod process;
mod runtime_context;
mod signal_diagnostics;
mod specialized_helpers;
mod symbols;
mod typed_pipeline;
mod typed_value;

pub(crate) use compiled::{
    CompiledFunctionHandle, DirectFunctionCompileResult, JitCodegenStats, VectorcallEntryFn,
};
pub(crate) use deopt::RuntimeFunctionEntryPlan;
#[cfg(test)]
use deopt::{RuntimeJitDeoptContinuation, RuntimeJitDeoptRecord};
use deopt::{
    RuntimeJitDeoptCursor, RuntimeJitDeoptInvocation, RuntimeJitDeoptLocals, RuntimeJitDeoptTable,
    RuntimeJitDeoptUnsupportedReason, runtime_jit_deopt_guard_operand_replay_safe,
    runtime_jit_typed_deopt_continuation_for_point,
    runtime_jit_typed_deopt_guard_operand_replay_safe,
    typed_nested_guard_misses_can_resume_before_instr,
};
use direct_abi::{
    ArgOwnership, DirectCallableDesc, DirectEntry, DirectTargetId, ErrorAbi, HiddenArgAbi,
    ParamAbi, PyLongI64Coercion, ResultAbi,
};
#[cfg(test)]
use module_data::push_shared_module_symbol_identity;
use module_data::{
    declare_module_constant_object_data, declare_module_constant_object_data_for_prefix,
    declare_scalar_counter_storage_import, declare_top_value_counter_storage_import,
    declare_type_ptr_import, define_scalar_counter_storage_data,
    define_scalar_counter_storage_data_for_symbol, define_top_value_counter_storage_data,
    define_top_value_counter_storage_data_for_symbol,
    direct_function_symbol_scope_for_shared_state, module_constant_object_symbol,
    module_constant_symbol_prefix_for_instance, module_constant_symbol_prefix_for_module_identity,
    module_constant_symbol_prefix_for_shared_state, persistent_function_id_for_module_function,
    precompiled_direct_function_symbol_scope_for_persistent,
    precompiled_direct_function_symbol_scope_for_shared_state, scalar_counter_storage_symbol,
    scalar_counter_storage_symbol_for_instance, scalar_counter_storage_symbol_for_shared_state,
    top_value_counter_storage_symbol, top_value_counter_storage_symbol_for_instance,
    top_value_counter_storage_symbol_for_shared_state,
};
pub use planning::{
    BlockExcDispatchPlan, BlockParamFacts, EdgeTransportPlan, FunctionLocalPlan, LocalRefKind,
    ParamBindingFacts, ParamProvenance, PlannedJitDeoptPoint, PlannedJitDeoptPointId,
    PlannedJitDeoptResumeFunction, PlannedJitDeoptResumeModule, PlannedJitFunctionLocals,
    PlannedJitModuleLocals, PlannedLocalEnvEntryMaterialization, PlannedLocalEnvEntrySource,
    PlannedLocalStorage, PlannedStackSlotEntrySeed, PreparedJitTypedModulePlan,
    RuntimeBlockParamPlan, local_ref_kind_for_stack_mirror, plan_jit_module_from_codegen,
    plan_jit_typed_module, planned_implicit_target_transports_for_typed_function,
    planned_jit_params_for_typed_function, planned_jump_edge_transports_for_typed_function,
    planned_local_env_entry_materializations_for_function,
    planned_stack_slot_entry_seeds_for_typed_function, render_jit_deopt_resume_function,
    render_jit_deopt_resume_module, render_jit_function_locals, render_jit_module_locals,
    typed_exc_dispatch_plan,
};
pub(crate) use process::{ProcessJitEngine, process_jit_is_currently_compiling};
use runtime_context::{
    FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET, FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET,
    FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET, FUNCTION_ENV_GLOBALS_OBJ_OFFSET,
    FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET, PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
    PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET,
};
pub use runtime_context::{ModuleJitContext, ModuleRuntimeContext};
pub use specialized_helpers::ObjPtr;
use specialized_helpers::register_specialized_jit_symbols;
use symbols::{
    cpython_type_symbol_from_name, cpython_type_symbol_name, lookup_registered_jit_data_symbol,
    push_symbol_component_hex, py_dealloc_symbol, register_jit_data_symbol,
    reloc_callable_ref_symbol_name, reloc_type_ref_symbol_name, type_key_runtime_registry,
};
pub(crate) use typed_pipeline::JitModulePlan;
use typed_pipeline::{
    apply_profile_call_emission_plans_to_typed_function, build_jit_module_plan,
    build_typed_v3_jit_module_plan, collect_codegen_constants_for_module_name,
    predeclare_planned_typed_function_imports_for_reservation,
};
#[cfg(test)]
use typed_pipeline::{
    apply_profile_typed_block_metadata_to_typed_function,
    apply_profile_typed_guard_miss_policy_to_typed_function,
    apply_profile_typed_plans_to_typed_function,
};
pub use typed_value::{
    EmitResult, IntFacts, IntRange, IntWidth, ResultDemand, SoacRepr, SoacValue, ValueOwnership,
};

pub fn install_sigill_diagnostics() -> Result<(), String> {
    signal_diagnostics::install_sigill_diagnostics()
}

static RUNTIME_SUPPORT_LIBRARY: OnceLock<Result<RuntimeSupportLibrary, String>> = OnceLock::new();
static PRECOMPILED_LIBRARY: OnceLock<Result<Option<PrecompiledLibrary>, String>> = OnceLock::new();
const JIT_ARENA_BYTES: usize = 256 * 1024 * 1024;
const MISSING_PYTHON_EXCEPTION_TRAP: ir::TrapCode = ir::TrapCode::unwrap_user(1);
#[allow(dead_code)]
const OPT_V3_FUSED_CONSUMER_TRAP: ir::TrapCode = ir::TrapCode::unwrap_user(2);
const COLD_BLOCK_ENTRY_RATE_DENOMINATOR: u64 = 100;
thread_local! {
    static PROCESS_JIT_COMPILE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

fn runtime_support_library() -> Result<&'static RuntimeSupportLibrary, String> {
    match RUNTIME_SUPPORT_LIBRARY.get_or_init(|| {
        if let Some(error) = runtime_support_clif_compatibility_error() {
            return Err(error.to_string());
        }
        parse_runtime_clif_functions().map(|functions| RuntimeSupportLibrary { functions })
    }) {
        Ok(library) => Ok(library),
        Err(error) => Err(error.clone()),
    }
}

fn precompiled_library() -> Result<Option<&'static PrecompiledLibrary>, String> {
    match PRECOMPILED_LIBRARY.get_or_init(load_precompiled_library_from_env) {
        Ok(Some(library)) => Ok(Some(library)),
        Ok(None) => Ok(None),
        Err(error) => Err(error.clone()),
    }
}

fn load_precompiled_library_from_env() -> Result<Option<PrecompiledLibrary>, String> {
    let Some(path) = crate::config::precompiled_library_path()? else {
        return Ok(None);
    };
    promote_current_soac_extension_symbols_for_precompiled_library()?;
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        format!(
            "SOAC_PRECOMPILED_LIBRARY contains an interior NUL byte: {}",
            path.display()
        )
    })?;
    let handle = unsafe {
        libc::dlerror();
        libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL)
    };
    if handle.is_null() {
        return Err(format!(
            "failed to load SOAC_PRECOMPILED_LIBRARY {}: {}",
            path.display(),
            take_dlerror()
        ));
    }
    info!(
        target: "soac_jit_precompiled",
        event = "soac.precompiled_library_load",
        path = %path.display(),
        "soac_precompiled_library_load",
    );
    Ok(Some(PrecompiledLibrary {
        handle: handle as usize,
        path,
    }))
}

fn promote_current_soac_extension_symbols_for_precompiled_library() -> Result<(), String> {
    let mut info = MaybeUninit::<libc::Dl_info>::zeroed();
    let ok = unsafe {
        libc::dladdr(
            specialized_helpers::dp_jit_load_runtime_obj as *const c_void,
            info.as_mut_ptr(),
        )
    };
    if ok == 0 {
        return Err(
            "failed to locate the loaded _soac_ext shared object for SOAC_PRECOMPILED_LIBRARY"
                .to_string(),
        );
    }
    let info = unsafe { info.assume_init() };
    if info.dli_fname.is_null() {
        return Err(
            "dynamic loader did not report a path for the loaded _soac_ext shared object"
                .to_string(),
        );
    }

    let handle = unsafe {
        libc::dlerror();
        libc::dlopen(
            info.dli_fname,
            libc::RTLD_NOW | libc::RTLD_GLOBAL | libc::RTLD_NOLOAD,
        )
    };
    if !handle.is_null() {
        return Ok(());
    }
    let no_load_error = take_dlerror();

    let handle = unsafe {
        libc::dlerror();
        libc::dlopen(info.dli_fname, libc::RTLD_NOW | libc::RTLD_GLOBAL)
    };
    if handle.is_null() {
        return Err(format!(
            "failed to promote _soac_ext symbols for SOAC_PRECOMPILED_LIBRARY (RTLD_NOLOAD error: {}; reopen error: {})",
            no_load_error,
            take_dlerror()
        ));
    }
    Ok(())
}

fn take_dlerror() -> String {
    let error = unsafe { libc::dlerror() };
    if error.is_null() {
        return "unknown dynamic loader error".to_string();
    }
    unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug)]
struct PrecompiledLibrary {
    handle: usize,
    path: PathBuf,
}

pub(crate) struct PrecompiledModuleRuntime {
    deopt_resume_plan: PlannedJitDeoptResumeModule,
    module_constant_ptrs: Vec<usize>,
}

impl PrecompiledModuleRuntime {
    fn module_constant_ptrs(&self) -> Vec<*mut ffi::PyObject> {
        self.module_constant_ptrs
            .iter()
            .map(|ptr| *ptr as *mut ffi::PyObject)
            .collect()
    }
}

impl PrecompiledLibrary {
    fn lookup_code_symbol(&self, symbol: &str) -> Result<Option<*const u8>, String> {
        let c_symbol = CString::new(symbol)
            .map_err(|_| format!("precompiled symbol contains an interior NUL byte: {symbol:?}"))?;
        let ptr = unsafe {
            libc::dlerror();
            libc::dlsym(self.handle as *mut c_void, c_symbol.as_ptr())
        };
        if ptr.is_null() {
            let error = unsafe { libc::dlerror() };
            if error.is_null() {
                return Ok(None);
            }
            let error = unsafe { CStr::from_ptr(error) }.to_string_lossy();
            if error.contains("undefined symbol:") {
                return Ok(None);
            }
            return Err(format!(
                "failed to look up precompiled symbol {symbol:?} in {}: {}",
                self.path.display(),
                error
            ));
        }
        Ok(Some(ptr.cast::<u8>() as *const u8))
    }

    fn lookup_module_constant_slot(
        &self,
        symbol: &str,
    ) -> Result<Option<*mut *mut ffi::PyObject>, String> {
        self.lookup_symbol(symbol)
            .map(|ptr| ptr.map(|ptr| ptr.cast::<*mut ffi::PyObject>()))
    }

    fn lookup_symbol(&self, symbol: &str) -> Result<Option<*mut c_void>, String> {
        let c_symbol = CString::new(symbol)
            .map_err(|_| format!("precompiled symbol contains an interior NUL byte: {symbol:?}"))?;
        let ptr = unsafe {
            libc::dlerror();
            libc::dlsym(self.handle as *mut c_void, c_symbol.as_ptr())
        };
        if ptr.is_null() {
            let error = unsafe { libc::dlerror() };
            if error.is_null() {
                return Ok(None);
            }
            return Err(format!(
                "failed to look up precompiled symbol {symbol:?} in {}: {}",
                self.path.display(),
                unsafe { CStr::from_ptr(error) }.to_string_lossy()
            ));
        }
        Ok(Some(ptr))
    }
}

use imports::{
    DP_JIT_DECREF_IMPORT, DP_JIT_DEOPT_RESUME_IMPORT, DP_JIT_DIRECT_COMPILE_FUNCTION_ENV_IMPORT,
    DP_JIT_ENTER_RECURSIVE_CALL_IMPORT, DP_JIT_FINISH_CONSTRUCTOR_INIT_IMPORT,
    DP_JIT_INCREF_IMPORT, DP_JIT_IS_TRUE_IMPORT, DP_JIT_LOAD_CELL_IMPORT,
    DP_JIT_LOAD_RUNTIME_OBJ_BY_ID_IMPORT, DP_JIT_MAKE_CELL_IMPORT,
    DP_JIT_POP_HANDLED_EXCEPTION_IMPORT, DP_JIT_PUSH_HANDLED_EXCEPTION_IMPORT,
    DP_JIT_PY_CALL_OBJECT_IMPORT, DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT,
    DP_JIT_PY_CALL_WITH_KW_IMPORT, DP_JIT_PY_VECTORCALL_IMPORT, DP_JIT_PYOBJECT_GETATTR_IMPORT,
    DP_JIT_PYOBJECT_GETITEM_IMPORT, DP_JIT_PYOBJECT_SETATTR_IMPORT, DP_JIT_PYOBJECT_SETITEM_IMPORT,
    DP_JIT_PYOBJECT_TO_I64_IMPORT, DP_JIT_PYTYPE_GENERIC_ALLOC_IMPORT,
    DP_JIT_RAISE_FROM_EXC_IMPORT, DP_JIT_RAISE_I64_OVERFLOW_IMPORT,
    DP_JIT_RAISE_MISSING_REQUIRED_ARGUMENT_IMPORT, DP_JIT_RAISE_SUPER_ARG_DELETED_IMPORT,
    DP_JIT_RAISE_UNBOUND_LOCAL_ERROR_IMPORT, DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT,
    DP_JIT_STORE_CELL_IMPORT, DP_JIT_VECTORCALL_BIND_DIRECT_ARGS_IMPORT,
    DP_JIT_VECTORCALL_COMPILE_FUNCTION_ENV_IMPORT, ImportSpec, ModuleFuncImports,
    PY_THREAD_STATE_GET_UNCHECKED_IMPORT, PYLONG_FROM_LONGLONG_IMPORT, PYNUMBER_ADD_IMPORT,
    PYNUMBER_AND_IMPORT, PYNUMBER_MULTIPLY_IMPORT, PYNUMBER_OR_IMPORT, PYNUMBER_SUBTRACT_IMPORT,
    PYNUMBER_XOR_IMPORT, PYOBJECT_RICHCOMPARE_IMPORT, SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_IMPORT,
    SOAC_RUNTIME_BUILTIN_CHR_I64_IMPORT, SOAC_RUNTIME_BUILTIN_LEN_I64_IMPORT,
    SOAC_RUNTIME_BUILTIN_ORD_I64_IMPORT, SOAC_RUNTIME_DECREF_APPLIED_IMPORT,
    SOAC_RUNTIME_INCREF_APPLIED_IMPORT, SOAC_RUNTIME_LOAD_GLOBAL_IMPORT,
    SOAC_RUNTIME_LOAD_GLOBAL_SLOW_IMPORT, SOAC_RUNTIME_PROBE_FIELD_INDEXED_IMPORT,
    SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_IMPORT, SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_IMPORT,
    SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT, SOAC_RUNTIME_STORE_FIELD_INDEXED_IMPORT,
    SOAC_RUNTIME_STORE_GLOBAL_IMPORT, SOAC_RUNTIME_STORE_GLOBAL_INDEXED_IMPORT,
    SOAC_RUNTIME_TUPLE_NEW_IMPORT, SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_IMPORT, SigType,
    StaticSignature, predeclare_jit_runtime_imports, predeclare_specialization_type_imports,
    predeclare_typed_direct_call_imports,
};

struct FuncBuildImports<'a> {
    module_imports: &'a mut ModuleFuncImports,
    func_refs_by_internal_id: Vec<Option<ir::FuncRef>>,
}

impl<'a> FuncBuildImports<'a> {
    fn new(module_imports: &'a mut ModuleFuncImports) -> Self {
        Self {
            module_imports,
            func_refs_by_internal_id: Vec::new(),
        }
    }

    fn get(
        &mut self,
        codegen_env: &mut impl JitCodegenEnv,
        func: &mut ir::Function,
        spec: &'static ImportSpec,
    ) -> Result<ir::FuncRef, String> {
        let internal_id = spec.internal_id();
        if internal_id >= self.func_refs_by_internal_id.len() {
            self.func_refs_by_internal_id.resize(internal_id + 1, None);
        }
        if let Some(func_ref) = self.func_refs_by_internal_id[internal_id] {
            return Ok(func_ref);
        }
        let func_id = self.module_imports.ensure_declared(codegen_env, spec)?;
        let func_ref = codegen_env.codegen_declare_func_in_func(func_id, func)?;
        self.func_refs_by_internal_id[internal_id] = Some(func_ref);
        Ok(func_ref)
    }

    fn get_or_panic(
        &mut self,
        codegen_env: &mut impl JitCodegenEnv,
        func: &mut ir::Function,
        spec: &'static ImportSpec,
    ) -> ir::FuncRef {
        self.get(codegen_env, func, spec).unwrap_or_else(|err| {
            panic!(
                "failed to bind import {} during JIT codegen: {}",
                spec.symbol, err
            )
        })
    }
}

#[derive(Debug, Clone)]
pub struct RenderedSpecializedClif {
    pub pre_inline_clif: String,
    pub clif: String,
    pub cfg_dot: String,
    pub vcode_disasm: String,
}

#[derive(Debug, Clone)]
struct ClifBlockDisplayAnnotation {
    semantic_name: String,
    param_names: Vec<String>,
}

type ClifBlockDisplayAnnotations = HashMap<String, ClifBlockDisplayAnnotation>;

struct BuiltSpecializedFunction {
    ctx: cranelift_codegen::Context,
    main_id: cranelift_module::FuncId,
    main_symbol: String,
    default_adapter_id: Option<cranelift_module::FuncId>,
    default_adapter_symbol: Option<String>,
    import_id_to_symbol: HashMap<u32, &'static str>,
    #[cfg(test)]
    func_id_to_symbol: HashMap<u32, &'static str>,
    block_annotations: ClifBlockDisplayAnnotations,
}

#[derive(Clone)]
struct DeclaredJitFunction {
    func_id: FuncId,
    default_func_id: Option<FuncId>,
    symbol: String,
    default_symbol: Option<String>,
}

fn env_config_for_session(
    compile_session: Option<&crate::session::CompileSession>,
) -> Result<Cow<'_, SoacEnvConfig>, String> {
    match compile_session {
        Some(session) => Ok(Cow::Borrowed(session.env_config()?)),
        None => Ok(Cow::Owned(SoacEnvConfig::from_env()?)),
    }
}

trait JitCodegenEnv {
    fn codegen_isa(&self) -> &dyn TargetIsa;

    fn codegen_jit_module_mut(&mut self) -> Option<&mut JITModule> {
        None
    }

    fn function_declaration(&self, id: FuncId) -> Result<(&ir::Signature, Linkage), String>;

    fn data_declaration(&self, id: DataId) -> Result<(Linkage, bool), String>;

    fn codegen_declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        signature: &ir::Signature,
    ) -> Result<FuncId, String>;

    fn codegen_declare_data(
        &mut self,
        name: &str,
        linkage: Linkage,
        writable: bool,
        tls: bool,
    ) -> Result<DataId, String>;

    fn codegen_target_config(&self) -> TargetFrontendConfig {
        self.codegen_isa().frontend_config()
    }

    fn codegen_make_context(&self) -> cranelift_codegen::Context {
        let mut ctx = cranelift_codegen::Context::new();
        ctx.func.signature.call_conv = self.codegen_isa().default_call_conv();
        ctx
    }

    fn codegen_clear_context(&self, ctx: &mut cranelift_codegen::Context) {
        ctx.clear();
        ctx.func.signature.call_conv = self.codegen_isa().default_call_conv();
    }

    fn codegen_make_signature(&self) -> ir::Signature {
        ir::Signature::new(self.codegen_isa().default_call_conv())
    }

    fn codegen_declare_func_in_func(
        &mut self,
        func_id: FuncId,
        func: &mut ir::Function,
    ) -> Result<ir::FuncRef, String> {
        let (signature, linkage) = self.function_declaration(func_id)?;
        let signature = func.import_signature(signature.clone());
        let user_name_ref = func.declare_imported_user_function(ir::UserExternalName {
            namespace: 0,
            index: func_id.as_u32(),
        });
        Ok(func.import_function(ir::ExtFuncData {
            name: ir::ExternalName::user(user_name_ref),
            signature,
            colocated: linkage.is_final(),
            patchable: false,
        }))
    }

    fn codegen_declare_data_in_func(
        &mut self,
        data_id: DataId,
        func: &mut ir::Function,
    ) -> Result<ir::GlobalValue, String> {
        let (linkage, tls) = self.data_declaration(data_id)?;
        let user_name_ref = func.declare_imported_user_function(ir::UserExternalName {
            namespace: 1,
            index: data_id.as_u32(),
        });
        Ok(func.create_global_value(ir::GlobalValueData::Symbol {
            name: ir::ExternalName::user(user_name_ref),
            offset: ir::immediates::Imm64::new(0),
            colocated: linkage.is_final(),
            tls,
        }))
    }
}

impl JitCodegenEnv for JITModule {
    fn codegen_isa(&self) -> &dyn TargetIsa {
        Module::isa(self)
    }

    fn codegen_jit_module_mut(&mut self) -> Option<&mut JITModule> {
        Some(self)
    }

    fn function_declaration(&self, id: FuncId) -> Result<(&ir::Signature, Linkage), String> {
        let declaration = self.declarations().get_function_decl(id);
        Ok((&declaration.signature, declaration.linkage))
    }

    fn data_declaration(&self, id: DataId) -> Result<(Linkage, bool), String> {
        let declaration = self.declarations().get_data_decl(id);
        Ok((declaration.linkage, declaration.tls))
    }

    fn codegen_declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        signature: &ir::Signature,
    ) -> Result<FuncId, String> {
        Module::declare_function(self, name, linkage, signature)
            .map_err(|err| format!("failed to declare JIT function {name}: {err}"))
    }

    fn codegen_declare_data(
        &mut self,
        name: &str,
        linkage: Linkage,
        writable: bool,
        tls: bool,
    ) -> Result<DataId, String> {
        Module::declare_data(self, name, linkage, writable, tls)
            .map_err(|err| format!("failed to declare JIT data {name}: {err}"))
    }
}

pub(crate) fn lookup_precompiled_direct_function_handle(
    session: &Arc<crate::session::CompileSession>,
    shared_state: &SharedModuleState,
    function: &BlockPyFunction<CodegenModuleShape>,
) -> Result<Option<Arc<CompiledFunctionHandle>>, String> {
    let Some(library) = precompiled_library()? else {
        return Ok(None);
    };
    if shared_state.source_hash() == 0 {
        return Ok(None);
    }

    let symbol_scope = precompiled_direct_function_symbol_scope_for_shared_state(
        shared_state,
        function.function_id,
    );
    let symbol = direct_function_symbol(function, Some(symbol_scope.as_str()));
    let Some(code_ptr) = library.lookup_code_symbol(symbol.as_str())? else {
        return Ok(None);
    };

    let default_code_ptr = if function_has_default_resolving_direct_entry(function) {
        let default_symbol = default_direct_function_symbol(function, Some(symbol_scope.as_str()));
        library
            .lookup_code_symbol(default_symbol.as_str())?
            .ok_or_else(|| {
                format!(
                    "precompiled library {} has direct entry {symbol:?} but is missing default entry {default_symbol:?}",
                    library.path.display()
                )
            })?
    } else {
        code_ptr
    };

    let runtime = precompiled_module_runtime(library, shared_state)?;
    let function_deopt_resume_plan = runtime
        .deopt_resume_plan
        .function(function.function_id)
        .ok_or_else(|| {
            format!(
                "missing JIT deopt resume plan for precompiled function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
    let module_constant_ptrs = runtime.module_constant_ptrs();
    let deopt_table = Arc::new(RuntimeJitDeoptTable::from_plan(
        function,
        function_deopt_resume_plan,
        module_constant_ptrs.as_slice(),
    )?);
    info!(
        target: "soac_jit_precompiled",
        event = "soac.precompiled_direct_function_hit",
        module = shared_state.module_name.as_str(),
        source_hash = format_args!("0x{:016x}", shared_state.source_hash()),
        function_id = function.function_id.local_function_id().as_u32(),
        qualname = function.names.qualname.as_str(),
        symbol = symbol.as_str(),
        "soac_precompiled_direct_function_hit",
    );
    Ok(Some(Arc::new(CompiledFunctionHandle::from_direct_entry(
        session,
        code_ptr,
        default_code_ptr,
        function.params.len(),
        deopt_table,
        None,
    ))))
}

pub(crate) fn lookup_precompiled_static_module_constant(
    module_name: &str,
    source_hash: u64,
    constant_id: ModuleConstantId,
) -> Result<Option<*mut ffi::PyObject>, String> {
    let Some(library) = precompiled_library()? else {
        return Ok(None);
    };
    if source_hash == 0 {
        return Ok(None);
    }
    let symbol_prefix = module_constant_symbol_prefix_for_module_identity(module_name, source_hash);
    let symbol = module_constant_object_symbol(symbol_prefix.as_str(), constant_id);
    library
        .lookup_code_symbol(symbol.as_str())
        .map(|ptr| ptr.map(|ptr| ptr.cast_mut().cast::<ffi::PyObject>()))
}

fn precompiled_module_runtime(
    library: &PrecompiledLibrary,
    shared_state: &SharedModuleState,
) -> Result<Arc<PrecompiledModuleRuntime>, String> {
    match shared_state
        .precompiled_module_runtime
        .get_or_init(|| build_precompiled_module_runtime(library, shared_state))
    {
        Ok(runtime) => Ok(Arc::clone(runtime)),
        Err(error) => Err(error.clone()),
    }
}

fn build_precompiled_module_runtime(
    library: &PrecompiledLibrary,
    shared_state: &SharedModuleState,
) -> Result<Arc<PrecompiledModuleRuntime>, String> {
    patch_precompiled_module_constant_slots(library, shared_state)?;
    let value_facts = infer_jit_value_facts(&shared_state.lowered_module);
    let deopt_resume_plan =
        plan_jit_module_from_codegen(&shared_state.lowered_module, value_facts)?.deopt_resume;
    let module_constant_ptrs = shared_state
        .module_constant_ptrs()
        .into_iter()
        .map(|ptr| ptr as usize)
        .collect();
    Ok(Arc::new(PrecompiledModuleRuntime {
        deopt_resume_plan,
        module_constant_ptrs,
    }))
}

fn patch_precompiled_module_constant_slots(
    library: &PrecompiledLibrary,
    shared_state: &SharedModuleState,
) -> Result<(), String> {
    let symbol_prefix = module_constant_symbol_prefix_for_shared_state(shared_state);
    for (index, ptr) in shared_state.module_constant_ptrs().into_iter().enumerate() {
        let constant_id = ModuleConstantId(index);
        if shared_state
            .codegen_constants
            .static_pyobject_image(constant_id)
            .is_some()
        {
            continue;
        }
        let symbol = module_constant_object_symbol(symbol_prefix.as_str(), constant_id);
        let Some(slot) = library.lookup_module_constant_slot(symbol.as_str())? else {
            return Err(format!(
                "precompiled library {} is missing module constant slot {symbol:?}",
                library.path.display()
            ));
        };
        unsafe { *slot = ptr };
    }
    Ok(())
}

fn codegen_expr_is_borrowable_from_local_env(
    expr: &InstrCodegen,
    local_env: &LocalEnv,
    stack_slots: &StackSlots,
    storage_layout: Option<&StorageLayout>,
) -> bool {
    match expr {
        InstrCodegen::Load(op) => {
            let Some(location) = op.name.local_location() else {
                return false;
            };
            if local_env.entry_index_for_location(location).is_some() {
                return true;
            }
            storage_layout
                .and_then(|layout| layout.stack_slots().get(location.slot() as usize))
                .is_some_and(|name| {
                    local_env.entry_index_for_name(name).is_some() || stack_slots.has_name(name)
                })
        }
        _ => false,
    }
}

fn codegen_expr_pyobject_input_is_borrowed_from_local_env(
    expr: &InstrCodegen,
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
            if local_env.entry_index_for_location(location).is_some() {
                return true;
            }
            storage_layout
                .and_then(|layout| layout.stack_slots().get(location.slot() as usize))
                .is_some_and(|name| {
                    local_env.entry_index_for_name(name).is_some() || stack_slots.has_name(name)
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
        TypedPyObjectOwnershipPlan::BorrowedLocal => {
            typed_expr_is_borrowable_from_local_env(expr, local_env, stack_slots, storage_layout)
        }
        TypedPyObjectOwnershipPlan::Immortal => true,
        TypedPyObjectOwnershipPlan::Owned => false,
    })
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
            let name_obj = emit_owned_module_constant(
                fb,
                ctx.module_constants
                    .require_unicode_constant_id(name.id.as_str()),
                ctx,
            );
            let slot_index = fb.ins().iconst(ir::types::I64, i64::from(slot.slot()));
            let value_inst = fb.ins().call(
                ctx.load_global_fast_ref,
                &[globals_obj, name_obj, slot_index],
            );
            let value = fb.inst_results(value_inst)[0];
            let value = emit_decref_owned_input_after_nullable_result(fb, ctx, value, name_obj);
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
        NameLocation::RuntimeName(_) => {
            let runtime_name_id = runtime_name_id_value(fb, name.runtime_name_id());
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
    fb.ins()
        .call(ctx.decref_ref, &[ctx.consts.thread_state_value, cell_obj]);
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
    fb.ins().call(ctx.incref_ref, &[direct_value]);
    emit_optional_counter_increment_for_kind(fb, ctx, ctx.global_indexed_hit_counter_ids, instr_id);
    fb.ins()
        .call(ctx.decref_ref, &[ctx.consts.thread_state_value, name_obj]);
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
            let fallback_inst = fb.ins().call(
                ctx.load_global_slow_ref,
                &[globals_obj, name_obj, slot_index],
            );
            let fallback_value = fb.inst_results(fallback_inst)[0];
            let fallback_value =
                emit_decref_owned_input_after_nullable_result(fb, ctx, fallback_value, name_obj);
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);
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
            emit_release_owned_inputs(fb, ctx, &[name_obj]);
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

fn codegen_expr_helper_name<'a>(
    expr: &'a InstrCodegen,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a str> {
    match expr {
        InstrCodegen::Load(op)
            if op.name.location.is_global() || op.name.location.is_runtime_name() =>
        {
            Some(op.name.id.as_str())
        }
        InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_runtime_name_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn codegen_expr_static_runtime_name<'a>(
    expr: &'a InstrCodegen,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a str> {
    match expr {
        InstrCodegen::Load(op) if op.name.location.is_runtime_name() => Some(op.name.id.as_str()),
        InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
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
    expr: &InstrCodegen,
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
    expr: &InstrCodegen,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Option<(ir::Value, ir::Value)> {
    let InstrCodegen::Load(op) = expr else {
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
        local_env.entries[index].value
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
    instance_expr: &InstrCodegen,
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
    callable_expr: &InstrCodegen,
    super_fn_expr: &InstrCodegen,
    cls_expr: &InstrCodegen,
    instance_expr: &InstrCodegen,
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
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, cls]);
        }
        if !super_fn_is_borrowed {
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, super_fn]);
        }
        if !callable_is_borrowed {
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, callable]);
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
        fb.ins().call(
            ctx.decref_ref,
            &[ctx.consts.thread_state_value, instance_arg.value],
        );
    }
    if !cls_is_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, cls]);
    }
    if !super_fn_is_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, super_fn]);
    }
    if !callable_is_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, callable]);
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

fn load_function_env_obj(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    function_env_value: ir::Value,
    offset: i32,
) -> ir::Value {
    fb.ins()
        .load(ptr_ty, ir::MemFlags::trusted(), function_env_value, offset)
}

fn load_py_function_soac_metadata_obj(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    function_obj: ir::Value,
) -> ir::Value {
    #[repr(C)]
    struct PyFunctionObjectSoacMetadataPrefix {
        ob_refcnt: isize,
        ob_type: *mut ffi::PyTypeObject,
        func_globals: *mut ffi::PyObject,
        func_builtins: *mut ffi::PyObject,
        func_name: *mut ffi::PyObject,
        func_qualname: *mut ffi::PyObject,
        func_code: *mut ffi::PyObject,
        func_defaults: *mut ffi::PyObject,
        func_kwdefaults: *mut ffi::PyObject,
        func_closure: *mut ffi::PyObject,
        func_doc: *mut ffi::PyObject,
        func_dict: *mut ffi::PyObject,
        func_weakreflist: *mut ffi::PyObject,
        func_module: *mut ffi::PyObject,
        func_annotations: *mut ffi::PyObject,
        func_annotate: *mut ffi::PyObject,
        func_typeparams: *mut ffi::PyObject,
        vectorcall: ffi::vectorcallfunc,
        func_soac_metadata: *mut std::ffi::c_void,
    }

    fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        function_obj,
        offset_of!(PyFunctionObjectSoacMetadataPrefix, func_soac_metadata) as i32,
    )
}

fn emit_resolved_direct_function_metadata_and_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> (ir::Value, ir::Value) {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    let metadata = load_py_function_soac_metadata_obj(fb, ptr_ty, callable);
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

#[derive(Clone, Debug)]
pub(crate) struct FunctionRuntimeDataLayout {
    positional_default_count: usize,
    positional_default_slots_by_param_index: HashMap<usize, usize>,
    kwonly_default_slots: HashMap<String, usize>,
    closure_start: usize,
    closure_len: usize,
    total_len: usize,
}

impl FunctionRuntimeDataLayout {
    pub(crate) fn from_function(function: &BlockPyFunction<CodegenModuleShape>) -> Self {
        Self::from_parts(function, max_referenced_function_closure_slot(function))
    }

    pub(crate) fn from_typed_function(function: &BlockPyFunction<TypedCodegenModuleShape>) -> Self {
        Self::from_parts(
            function,
            max_referenced_typed_function_closure_slot(function),
        )
    }

    fn from_parts<P: ModuleShape>(function: &BlockPyFunction<P>, max_closure_slot: usize) -> Self {
        let positional_param_indices = function
            .params
            .params
            .iter()
            .enumerate()
            .filter_map(|(index, param)| {
                matches!(param.kind, ParamKind::PosOnly | ParamKind::Any).then_some(index)
            })
            .collect::<Vec<_>>();
        let positional_default_count = positional_param_indices.len();
        let positional_default_slots_by_param_index = positional_param_indices
            .into_iter()
            .enumerate()
            .map(|(slot, param_index)| (param_index, slot))
            .collect::<HashMap<_, _>>();
        let mut kwonly_default_slots = HashMap::new();
        for param in function.params.iter() {
            if param.kind == ParamKind::KwOnly {
                let slot = positional_default_count + kwonly_default_slots.len();
                kwonly_default_slots.insert(param.name.to_string(), slot);
            }
        }
        let closure_start = positional_default_count + kwonly_default_slots.len();
        let storage_layout_closure_len = function
            .storage_layout()
            .as_ref()
            .map(|layout| layout.freevars.len())
            .unwrap_or(0);
        let closure_len = storage_layout_closure_len.max(max_closure_slot);
        let total_len = closure_start + closure_len;
        Self {
            positional_default_count,
            positional_default_slots_by_param_index,
            kwonly_default_slots,
            closure_start,
            closure_len,
            total_len,
        }
    }

    pub(crate) fn positional_default_count(&self) -> usize {
        self.positional_default_count
    }

    pub(crate) fn positional_default_slot(&self, default_index: usize) -> usize {
        debug_assert!(default_index < self.positional_default_count);
        default_index
    }

    pub(crate) fn positional_default_slot_for_param_index(
        &self,
        param_index: usize,
    ) -> Option<usize> {
        self.positional_default_slots_by_param_index
            .get(&param_index)
            .copied()
    }

    pub(crate) fn kwonly_default_slot(&self, name: &str) -> Option<usize> {
        self.kwonly_default_slots.get(name).copied()
    }

    pub(crate) fn kwonly_default_slots(&self) -> impl Iterator<Item = (&str, usize)> {
        self.kwonly_default_slots
            .iter()
            .map(|(name, slot)| (name.as_str(), *slot))
    }

    pub(crate) fn closure_len(&self) -> usize {
        self.closure_len
    }

    pub(crate) fn closure_cell_slot(&self, closure_slot: usize) -> usize {
        debug_assert!(closure_slot < self.closure_len);
        self.closure_start + closure_slot
    }

    pub(crate) fn total_len(&self) -> usize {
        self.total_len
    }
}

fn max_referenced_function_closure_slot(function: &BlockPyFunction<CodegenModuleShape>) -> usize {
    #[derive(Default)]
    struct Collector {
        max_slot_plus_one: usize,
    }

    impl Collector {
        fn visit_cell_location(&mut self, location: CellLocation) {
            match location {
                CellLocation::Closure(slot) | CellLocation::CapturedSource(slot) => {
                    self.max_slot_plus_one = self.max_slot_plus_one.max(slot as usize + 1);
                }
                CellLocation::Owned(_) => {}
            }
        }

        fn visit_name(&mut self, name: &ResolvedName) {
            if let Some(location) = name.cell_location() {
                self.visit_cell_location(location);
            }
        }
    }

    impl Visit<InstrCodegen> for Collector {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            match expr {
                InstrCodegen::Load(op) => self.visit_name(&op.name),
                InstrCodegen::Store(op) => self.visit_name(&op.name),
                InstrCodegen::Del(op) => self.visit_name(&op.name),
                InstrCodegen::CellRef(op) => self.visit_cell_location(op.location),
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector::default();
    collector.visit_fn(function);
    collector.max_slot_plus_one
}

fn max_referenced_typed_function_closure_slot(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> usize {
    #[derive(Default)]
    struct Collector {
        max_slot_plus_one: usize,
    }

    impl Collector {
        fn visit_cell_location(&mut self, location: CellLocation) {
            match location {
                CellLocation::Closure(slot) | CellLocation::CapturedSource(slot) => {
                    self.max_slot_plus_one = self.max_slot_plus_one.max(slot as usize + 1);
                }
                CellLocation::Owned(_) => {}
            }
        }

        fn visit_name(&mut self, name: &ResolvedName) {
            if let Some(location) = name.cell_location() {
                self.visit_cell_location(location);
            }
        }
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            match expr {
                InstrTyped::Load(op) => self.visit_name(&op.name),
                InstrTyped::Store(op) => self.visit_name(&op.name),
                InstrTyped::Del(op) => self.visit_name(&op.name),
                InstrTyped::CellRef(op) => self.visit_cell_location(op.location),
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector::default();
    collector.visit_fn(function);
    collector.max_slot_plus_one
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
    none_constant_id: ModuleConstantId,
    true_constant_id: ModuleConstantId,
    false_constant_id: ModuleConstantId,
    empty_tuple_constant_id: ModuleConstantId,
    block_const: ir::Value,
    module_constant_accesses: ModuleConstantAccessTable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ModuleConstantAccess {
    #[default]
    SymbolAddress,
    PointerSlot,
}

#[derive(Clone, Debug, Default)]
struct ModuleConstantAccessTable {
    entries: Option<Arc<[ModuleConstantAccess]>>,
}

impl ModuleConstantAccessTable {
    fn from_entries(entries: Vec<ModuleConstantAccess>) -> Self {
        Self {
            entries: Some(Arc::from(entries)),
        }
    }

    fn access(&self, constant_id: ModuleConstantId) -> ModuleConstantAccess {
        self.entries
            .as_ref()
            .and_then(|entries| entries.get(constant_id.0).copied())
            .unwrap_or_default()
    }
}

#[derive(Clone)]
struct JitEmitCtx<'mc> {
    module: &'mc BlockPyModule<TypedCodegenModuleShape>,
    function_id: RuntimeFunctionId,
    function_kind: FunctionKind,
    module_constants: &'mc ModuleCodegenConstants,
    value_facts: &'mc FactStore,
    deopt_resume_plan: &'mc PlannedJitDeoptResumeFunction,
    refcount_plan: &'mc FunctionRefcountPlan,
    instr_locations: &'mc InstrLocationMap,
    counter_slots_by_id: &'mc [CounterRuntimeSlot],
    storage_layout: Option<StorageLayout>,
    function_runtime_data_layout: &'mc FunctionRuntimeDataLayout,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
    py_call_positional_three_ref: ir::FuncRef,
    py_vectorcall_ref: ir::FuncRef,
    pytype_generic_alloc_ref: ir::FuncRef,
    finish_constructor_init_ref: ir::FuncRef,
    consts: JitEmitConsts,
    load_global_fast_ref: ir::FuncRef,
    probe_global_indexed_ref: ir::FuncRef,
    load_global_slow_ref: ir::FuncRef,
    guard_miss_deopt_stub_ref: Option<ir::FuncRef>,
    guard_miss_deopt_instr_ids: &'mc HashSet<InstrId>,
    guard_miss_resume_point: Option<LocalEnvResumePoint>,
    store_global_indexed_ref: ir::FuncRef,
    probe_field_indexed_ref: ir::FuncRef,
    store_field_indexed_ref: ir::FuncRef,
    load_runtime_obj_by_id_ref: ir::FuncRef,
    enter_recursive_ref: ir::FuncRef,
    direct_compile_function_env_ref: ir::FuncRef,
    pyobject_getattr_ref: ir::FuncRef,
    pyobject_setattr_ref: ir::FuncRef,
    pyobject_getitem_ref: ir::FuncRef,
    pyobject_setitem_ref: ir::FuncRef,
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
        &'mc HashMap<RuntimeFunctionId, BlockPyFunction<TypedCodegenModuleShape>>,
    direct_call_functions: &'mc HashMap<RuntimeFunctionId, DeclaredJitFunction>,
    call_target_counter_ids: &'mc HashMap<InstrId, CounterId>,
    call_direct_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    call_direct_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    operator_shape_counter_ids: &'mc HashMap<InstrId, CounterId>,
    getitem_shape_counter_ids: &'mc HashMap<InstrId, CounterId>,
    getitem_specialized_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    getitem_specialized_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    setitem_shape_counter_ids: &'mc HashMap<InstrId, CounterId>,
    setitem_specialized_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    setitem_specialized_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    branch_outcome_counter_ids: &'mc HashMap<InstrId, CounterId>,
    global_indexed_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    global_indexed_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    field_indexed_hit_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    field_indexed_fallback_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    field_generic_getattr_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    field_generic_setattr_counter_ids: &'mc HashMap<InstrId, CounterRef>,
    deopt_entry_guard_miss_counter_ids: &'mc HashMap<usize, CounterId>,
    allow_local_only_slot_backed_stores: bool,
    exception_forwarded_local_names: Option<&'mc [String]>,
    type_ptr_data_ids: RefCell<HashMap<RelocTypeRef, DataId>>,
    callable_ptr_data_ids: RefCell<HashMap<RelocCallableRef, DataId>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CounterRef {
    pub(super) counter_id: CounterId,
    branch_id: Option<CounterBranchId>,
}

impl CounterRef {
    const fn branch(counter_id: CounterId, branch_id: CounterBranchId) -> Self {
        Self {
            counter_id,
            branch_id: Some(branch_id),
        }
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> HashSet<InstrId> {
    struct Collector {
        instr_ids: HashSet<InstrId>,
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if expr.guard_miss_deopt_enabled()
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
    if matches!(binding.source, LocalEnvResumeValueSource::Unbound) {
        return Ok(null_ptr);
    }
    if let Some(index) = local_env
        .entry_index_for_location(binding.location)
        .or_else(|| local_env.entry_index_for_name(binding.name.as_str()))
    {
        return Ok(local_env.entries[index].value);
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
    fn value_facts_for_instr_id(&self, instr_id: InstrId) -> Option<ValueFacts> {
        self.value_facts
            .fact_for(InstrKey::new(self.function_id, instr_id))
    }

    fn value_facts_for_expr(&self, expr: &InstrCodegen) -> Option<ValueFacts> {
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
        pre_guard_operands: &[&InstrCodegen],
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

fn infer_jit_value_facts(module: &BlockPyModule<CodegenModuleShape>) -> FactStore {
    infer_module_value_facts(module)
}

#[derive(Clone)]
struct DirectMethodSpecialization {
    function_id: RuntimeFunctionId,
    descriptor_function_ref: RelocCallableRef,
    owner_type_ref: RelocTypeRef,
    type_version: u32,
    arg_plan: DirectCallArgPlan,
}

#[derive(Clone)]
struct DirectConstructorSpecialization {
    function_id: RuntimeFunctionId,
    init_function_ref: RelocCallableRef,
    owner_type_ref: RelocTypeRef,
    type_version: u32,
    arg_plan: DirectCallArgPlan,
}

#[derive(Clone)]
struct DirectFunctionSpecialization {
    function_id: RuntimeFunctionId,
    arg_plan: DirectCallArgPlan,
}

fn direct_call_arg_plan_from_typed(plan: &TypedDirectCallArgPlan) -> DirectCallArgPlan {
    DirectCallArgPlan {
        sources: plan
            .sources
            .iter()
            .map(|source| match source {
                TypedDirectCallArgSource::Provided(index) => DirectCallArgSource::Provided(*index),
                TypedDirectCallArgSource::DefaultSentinel => DirectCallArgSource::DefaultSentinel,
            })
            .collect(),
    }
}

fn direct_function_specializations_from_typed_guards(
    guards: &[TypedDirectFunctionCallGuard],
) -> Vec<DirectFunctionSpecialization> {
    guards
        .iter()
        .map(|guard| DirectFunctionSpecialization {
            function_id: guard.function_id,
            arg_plan: direct_call_arg_plan_from_typed(&guard.arg_plan),
        })
        .collect()
}

fn direct_constructor_specializations_from_typed_guards(
    guards: &[TypedDirectConstructorCallGuard],
) -> Vec<DirectConstructorSpecialization> {
    guards
        .iter()
        .filter_map(direct_constructor_specialization_from_typed_guard)
        .collect()
}

fn direct_constructor_specialization_from_typed_guard(
    guard: &TypedDirectConstructorCallGuard,
) -> Option<DirectConstructorSpecialization> {
    let owner_type_ref = reloc_type_ref_from_typed_attr_owner_ref(&guard.owner_type_ref)?;
    Some(DirectConstructorSpecialization {
        function_id: guard.function_id,
        init_function_ref: RelocCallableRef::OwnerAttr {
            owner_type_ref: owner_type_ref.clone(),
            attr_name: "__init__".to_string(),
        },
        owner_type_ref,
        type_version: guard.type_version,
        arg_plan: direct_call_arg_plan_from_typed(&guard.arg_plan),
    })
}

fn direct_method_specializations_from_typed_guards(
    guards: &[TypedDirectMethodCallGuard],
    method_name: &str,
) -> Vec<DirectMethodSpecialization> {
    guards
        .iter()
        .filter_map(|guard| {
            let owner_type_ref = reloc_type_ref_from_typed_attr_owner_ref(&guard.owner_type_ref)?;
            Some(DirectMethodSpecialization {
                function_id: guard.function_id,
                descriptor_function_ref: RelocCallableRef::OwnerAttr {
                    owner_type_ref: owner_type_ref.clone(),
                    attr_name: method_name.to_string(),
                },
                owner_type_ref,
                type_version: guard.type_version,
                arg_plan: direct_call_arg_plan_from_typed(&guard.arg_plan),
            })
        })
        .collect()
}

fn direct_method_specialization_from_typed_call(
    call: &TypedDirectMethodCall<InstrTyped>,
) -> Option<DirectMethodSpecialization> {
    let owner_type_ref = reloc_type_ref_from_typed_attr_owner_ref(&call.guard.owner_type_ref)?;
    Some(DirectMethodSpecialization {
        function_id: call.guard.function_id,
        descriptor_function_ref: RelocCallableRef::OwnerAttr {
            owner_type_ref: owner_type_ref.clone(),
            attr_name: call.method_name.clone(),
        },
        owner_type_ref,
        type_version: call.guard.type_version,
        arg_plan: direct_call_arg_plan_from_typed(&call.guard.arg_plan),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectCallArgPlan {
    sources: Vec<DirectCallArgSource>,
}

impl DirectCallArgPlan {
    fn len(&self) -> usize {
        self.sources.len()
    }

    fn requires_default_resolving_entry(&self) -> bool {
        self.sources
            .iter()
            .any(|source| matches!(source, DirectCallArgSource::DefaultSentinel))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectCallArgSource {
    Provided(usize),
    DefaultSentinel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectCallIncompatibility {
    StarredArguments,
    Keywords,
    UnsupportedParameterKind { kind: ParamKind },
    MissingRequiredArgument,
    TooManyPositionalArguments { provided: usize, accepted: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirectCallEntryKind {
    Core,
    DefaultResolving,
}

#[derive(Default)]
struct DirectEdgeStats {
    clif_direct_edges: Cell<usize>,
    function_env_indirect_edges: Cell<usize>,
    guarded_generic_fallback_blocks: Cell<usize>,
    profiled_missing_target_candidates: Cell<usize>,
    profiled_arity_mismatch_candidates: Cell<usize>,
    profiled_unsupported_shape_candidates: Cell<usize>,
}

impl DirectEdgeStats {
    fn increment(cell: &Cell<usize>) {
        cell.set(cell.get() + 1);
    }

    fn record_resolved_direct_edge(&self) {
        Self::increment(&self.clif_direct_edges);
    }

    fn record_function_env_indirect_edge(&self) {
        Self::increment(&self.function_env_indirect_edges);
    }

    fn record_guarded_generic_fallback_block(&self) {
        Self::increment(&self.guarded_generic_fallback_blocks);
    }

    fn record_profiled_missing_target_candidate(&self) {
        Self::increment(&self.profiled_missing_target_candidates);
    }

    fn record_profiled_arity_mismatch_candidate(&self) {
        Self::increment(&self.profiled_arity_mismatch_candidates);
    }

    fn record_profiled_unsupported_shape_candidate(&self) {
        Self::increment(&self.profiled_unsupported_shape_candidates);
    }

    fn total(&self) -> usize {
        self.clif_direct_edges.get()
            + self.function_env_indirect_edges.get()
            + self.guarded_generic_fallback_blocks.get()
            + self.profiled_missing_target_candidates.get()
            + self.profiled_arity_mismatch_candidates.get()
            + self.profiled_unsupported_shape_candidates.get()
    }

    fn emit_trace(&self, module_name: &str, function: &BlockPyFunction<impl ModuleShape>) {
        if self.total() == 0 {
            return;
        }
        let clif_direct_edges = self.clif_direct_edges.get();
        let function_env_indirect_edges = self.function_env_indirect_edges.get();
        let guarded_generic_fallback_blocks = self.guarded_generic_fallback_blocks.get();
        let profiled_missing_target_candidates = self.profiled_missing_target_candidates.get();
        let profiled_arity_mismatch_candidates = self.profiled_arity_mismatch_candidates.get();
        let profiled_unsupported_shape_candidates =
            self.profiled_unsupported_shape_candidates.get();
        let generic_fallback_edges = function_env_indirect_edges
            + guarded_generic_fallback_blocks
            + profiled_missing_target_candidates
            + profiled_arity_mismatch_candidates
            + profiled_unsupported_shape_candidates;
        info!(
            target: "soac_jit_direct_edges",
            module = module_name,
            function_id = %function.function_id,
            qualname = %function.names.qualname,
            clif_direct_edges,
            function_env_indirect_edges,
            generic_fallback_edges,
            guarded_generic_fallback_blocks,
            profiled_missing_target_candidates,
            profiled_arity_mismatch_candidates,
            profiled_unsupported_shape_candidates,
            "soac_jit_direct_edges"
        );
    }
}

fn direct_call_target_function<'a>(
    ctx: &'a JitEmitCtx<'_>,
    function_id: RuntimeFunctionId,
) -> Option<&'a BlockPyFunction<TypedCodegenModuleShape>> {
    ctx.module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .or_else(|| ctx.direct_call_target_functions.get(&function_id))
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
}

fn plan_direct_call_args_for_target<P: ModuleShape>(
    target_function: &BlockPyFunction<P>,
    explicit_positional_arg_count: usize,
    implicit_positional_arg_count: usize,
    has_starred_arguments: bool,
    has_keywords: bool,
) -> Result<DirectCallArgPlan, DirectCallIncompatibility> {
    if has_starred_arguments {
        return Err(DirectCallIncompatibility::StarredArguments);
    }
    if has_keywords {
        return Err(DirectCallIncompatibility::Keywords);
    }

    for param in target_function.params.iter() {
        if matches!(param.kind, ParamKind::VarArg | ParamKind::KwArg) {
            return Err(DirectCallIncompatibility::UnsupportedParameterKind { kind: param.kind });
        }
    }

    let provided_positional_arg_count =
        implicit_positional_arg_count + explicit_positional_arg_count;
    let accepted_positional_arg_count = target_function
        .params
        .iter()
        .filter(|param| matches!(param.kind, ParamKind::PosOnly | ParamKind::Any))
        .count();
    if provided_positional_arg_count > accepted_positional_arg_count {
        return Err(DirectCallIncompatibility::TooManyPositionalArguments {
            provided: provided_positional_arg_count,
            accepted: accepted_positional_arg_count,
        });
    }

    let mut sources = Vec::with_capacity(target_function.params.len());
    let mut next_provided_arg = 0usize;
    for param in target_function.params.iter() {
        match param.kind {
            ParamKind::PosOnly | ParamKind::Any => {
                if next_provided_arg < provided_positional_arg_count {
                    sources.push(DirectCallArgSource::Provided(next_provided_arg));
                    next_provided_arg += 1;
                } else if param.has_default {
                    sources.push(DirectCallArgSource::DefaultSentinel);
                } else {
                    return Err(DirectCallIncompatibility::MissingRequiredArgument);
                }
            }
            ParamKind::KwOnly => {
                if param.has_default {
                    sources.push(DirectCallArgSource::DefaultSentinel);
                } else {
                    return Err(DirectCallIncompatibility::MissingRequiredArgument);
                }
            }
            ParamKind::VarArg | ParamKind::KwArg => unreachable!(
                "unsupported variadic params should be rejected before planning direct-call args"
            ),
        }
    }
    debug_assert_eq!(next_provided_arg, provided_positional_arg_count);
    Ok(DirectCallArgPlan { sources })
}

fn function_has_default_resolving_direct_entry(
    function: &BlockPyFunction<impl ModuleShape>,
) -> bool {
    // The adapter is also needed for parameters without source defaults:
    // __defaults__ / __kwdefaults__ can be assigned after function creation.
    function.params.iter().any(|param| {
        matches!(
            param.kind,
            ParamKind::PosOnly | ParamKind::Any | ParamKind::KwOnly
        )
    })
}

fn param_runtime_default_slot(
    layout: &FunctionRuntimeDataLayout,
    param: &soac_core::block_py::Param,
    param_index: usize,
) -> Option<usize> {
    match param.kind {
        ParamKind::PosOnly | ParamKind::Any => {
            layout.positional_default_slot_for_param_index(param_index)
        }
        ParamKind::KwOnly => layout.kwonly_default_slot(&param.name),
        ParamKind::VarArg | ParamKind::KwArg => None,
    }
}

fn validate_direct_call_compatibility(
    target_function: &BlockPyFunction<impl ModuleShape>,
    _direct_call_functions: &HashMap<RuntimeFunctionId, DeclaredJitFunction>,
    explicit_positional_arg_count: usize,
    implicit_positional_arg_count: usize,
    has_starred_arguments: bool,
    has_keywords: bool,
) -> Result<DirectCallArgPlan, DirectCallIncompatibility> {
    plan_direct_call_args_for_target(
        target_function,
        explicit_positional_arg_count,
        implicit_positional_arg_count,
        has_starred_arguments,
        has_keywords,
    )
}

fn record_profiled_direct_call_incompatibility(
    stats: &DirectEdgeStats,
    incompatibility: DirectCallIncompatibility,
) {
    match incompatibility {
        DirectCallIncompatibility::MissingRequiredArgument
        | DirectCallIncompatibility::TooManyPositionalArguments { .. } => {
            stats.record_profiled_arity_mismatch_candidate();
        }
        DirectCallIncompatibility::StarredArguments
        | DirectCallIncompatibility::Keywords
        | DirectCallIncompatibility::UnsupportedParameterKind { .. } => {
            stats.record_profiled_unsupported_shape_candidate();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FieldIndexSpecialization {
    expected_index: u32,
    owner_type_ref: RelocTypeRef,
    type_version: u32,
}

impl FieldIndexSpecialization {
    fn to_typed_guard(&self) -> TypedIndexedFieldGuard {
        TypedIndexedFieldGuard {
            expected_index: self.expected_index,
            owner_type_ref: typed_attr_owner_ref_from_reloc_type_ref(&self.owner_type_ref),
            type_version: self.type_version,
        }
    }
}

type OptV3ResolvedIndexedFieldAccess =
    OptV3ResolvedIndexedFieldAccessFromOpt<FieldIndexSpecialization>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexedFieldLoweringPlan {
    source: TypedIndexedFieldPlanSource,
    access: PlanV3IndexedFieldAccessKind,
    specializations: Vec<FieldIndexSpecialization>,
}

impl IndexedFieldLoweringPlan {
    fn for_access(
        instr_id: InstrId,
        source: TypedIndexedFieldPlanSource,
        guards: &[TypedIndexedFieldGuard],
        expected_access: PlanV3IndexedFieldAccessKind,
    ) -> Result<Option<Self>, String> {
        match source {
            TypedIndexedFieldPlanSource::OptimizationPlanV3 => {
                Self::from_typed_guards(instr_id, source, guards, expected_access)
            }
        }
    }

    fn from_typed_guards(
        instr_id: InstrId,
        source: TypedIndexedFieldPlanSource,
        guards: &[TypedIndexedFieldGuard],
        expected_access: PlanV3IndexedFieldAccessKind,
    ) -> Result<Option<Self>, String> {
        if guards.is_empty() {
            if source == TypedIndexedFieldPlanSource::OptimizationPlanV3 {
                return Err(format!(
                    "optimizer v3 indexed-field {:?} for {instr_id} lost all typed codegen guards",
                    expected_access
                ));
            }
            return Ok(None);
        }

        let mut specializations = Vec::with_capacity(guards.len());
        for guard in guards {
            let Some(specialization) = field_index_specialization_from_typed_guard(guard) else {
                continue;
            };
            push_unique_specialization(&mut specializations, specialization);
        }

        if specializations.is_empty() {
            if source == TypedIndexedFieldPlanSource::OptimizationPlanV3 {
                return Err(format!(
                    "optimizer v3 indexed-field {:?} for {instr_id} has no resolvable typed codegen guards",
                    expected_access
                ));
            }
            return Ok(None);
        }

        Ok(Some(Self {
            source,
            access: expected_access,
            specializations,
        }))
    }

    fn require_type_ptr(
        &self,
        instr_id: InstrId,
        specialization: &FieldIndexSpecialization,
        owner_type: Option<ir::Value>,
    ) -> Result<Option<ir::Value>, String> {
        match owner_type {
            Some(owner_type) => Ok(Some(owner_type)),
            None if self.source == TypedIndexedFieldPlanSource::OptimizationPlanV3 => Err(format!(
                "prevalidated optimizer v3 indexed-field {:?} for {instr_id} could not bind runtime owner type reference {:?}",
                self.access, specialization.owner_type_ref
            )),
            None => Ok(None),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CpythonTypeSymbol {
    Function,
    Method,
    Type,
    Long,
    List,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RelocTypeRef {
    CpythonTypeSymbol(CpythonTypeSymbol),
    TypeKey(CounterDumpTypeKey),
}

fn typed_attr_owner_ref_from_reloc_type_ref(owner_type_ref: &RelocTypeRef) -> TypedAttrOwnerRef {
    match owner_type_ref {
        RelocTypeRef::CpythonTypeSymbol(symbol) => {
            TypedAttrOwnerRef::CpythonTypeSymbol(cpython_type_symbol_name(*symbol).to_string())
        }
        RelocTypeRef::TypeKey(type_key) => TypedAttrOwnerRef::TypeKey {
            module_name: type_key.module_name.clone(),
            qualname: type_key.qualname.clone(),
        },
    }
}

fn reloc_type_ref_from_typed_attr_owner_ref(
    owner_type_ref: &TypedAttrOwnerRef,
) -> Option<RelocTypeRef> {
    match owner_type_ref {
        TypedAttrOwnerRef::CpythonTypeSymbol(symbol_name) => {
            cpython_type_symbol_from_name(symbol_name).map(RelocTypeRef::CpythonTypeSymbol)
        }
        TypedAttrOwnerRef::TypeKey {
            module_name,
            qualname,
        } => Some(RelocTypeRef::TypeKey(CounterDumpTypeKey {
            module_name: module_name.clone(),
            qualname: qualname.clone(),
        })),
    }
}

fn field_index_specialization_from_typed_guard(
    guard: &TypedIndexedFieldGuard,
) -> Option<FieldIndexSpecialization> {
    Some(FieldIndexSpecialization {
        expected_index: guard.expected_index,
        owner_type_ref: reloc_type_ref_from_typed_attr_owner_ref(&guard.owner_type_ref)?,
        type_version: guard.type_version,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RelocCallableRef {
    OwnerAttr {
        owner_type_ref: RelocTypeRef,
        attr_name: String,
    },
}

struct LocalEnvCodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd, Env: JitCodegenEnv> {
    fb: &'a mut FunctionBuilder<'b>,
    local_env: &'c mut LocalEnv,
    ctx: &'c JitEmitCtx<'mc>,
    codegen_env: &'a mut Env,
    func_imports: &'a mut FuncBuildImports<'d>,
}

#[derive(Clone)]
struct LocalEnvEntry {
    location: Option<LocalLocation>,
    name: String,
    aliases: Vec<String>,
    value: ir::Value,
    ref_kind: LocalRefKind,
    storage: LocalEnvStorage,
    binding_facts: ParamBindingFacts,
    py_facts: Option<PyObjFacts>,
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
            value: entry.value,
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
        self.entries.push(LocalEnvEntry {
            location: Some(location),
            name: name.to_string(),
            aliases,
            value,
            ref_kind,
            storage,
            binding_facts,
            py_facts,
        });
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
            .and_then(|index| self.entries[index].py_facts)
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
            let value = entry.value;
            if entry.binding_facts.requires_checked_local_load()
                || entry.ref_kind == LocalRefKind::Unbound
            {
                return Some(emit_checked_local_value_or_unbound(
                    fb, name, value, ctx, borrowed,
                ));
            }
            if !borrowed {
                fb.ins().call(ctx.incref_ref, &[value]);
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
            let value = entry.value;
            if entry.binding_facts.requires_checked_local_load()
                || entry.ref_kind == LocalRefKind::Unbound
            {
                return Some(emit_checked_local_value_or_unbound(
                    fb, name, value, ctx, borrowed,
                ));
            }
            if !borrowed {
                fb.ins().call(ctx.incref_ref, &[value]);
            }
            return Some(value);
        }
        None
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
        stack_slots: &StackSlots,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
    ) {
        let previous_entry = if let Some(existing_index) = self
            .entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
        {
            Some(self.entries.remove(existing_index))
        } else {
            None
        };
        let should_mirror_stack_slot = stack_slots.has_name(name)
            && match previous_entry.as_ref().map(|entry| entry.storage) {
                Some(LocalEnvStorage::LocalOnly) => false,
                Some(LocalEnvStorage::StackMirror) => true,
                None => !allow_local_only_slot_backed_store,
            };
        if should_mirror_stack_slot {
            stack_slots
                .replace_cloned_value(
                    fb,
                    name,
                    value,
                    ptr_ty,
                    thread_state_value,
                    incref_ref,
                    decref_ref,
                )
                .expect("slot-backed local missing from stack slots");
            fb.ins().call(decref_ref, &[thread_state_value, value]);
            self.entries.push(LocalEnvEntry {
                location: Some(
                    previous_entry
                        .as_ref()
                        .and_then(|entry| entry.location)
                        .unwrap_or(location),
                ),
                name: name.to_string(),
                aliases: previous_entry
                    .as_ref()
                    .map(|entry| entry.aliases.clone())
                    .unwrap_or_default(),
                value,
                ref_kind: local_ref_kind_for_stack_mirror(value_ref_kind),
                storage: LocalEnvStorage::StackMirror,
                binding_facts: local_binding_facts_for_stored_value(value_ref_kind),
                py_facts,
            });
        } else {
            self.entries.push(LocalEnvEntry {
                location: Some(location),
                name: name.to_string(),
                aliases: previous_entry
                    .as_ref()
                    .map(|entry| entry.aliases.clone())
                    .unwrap_or_default(),
                value,
                ref_kind: value_ref_kind,
                storage: LocalEnvStorage::LocalOnly,
                binding_facts: local_binding_facts_for_stored_value(value_ref_kind),
                py_facts,
            });
        }
        if let Some(previous) = previous_entry {
            if transient_local_needs_decref(previous.ref_kind) {
                emit_decref_if_not_null(fb, ptr_ty, decref_ref, thread_state_value, previous.value);
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
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
    ) {
        let previous_entry = self
            .entry_index_for_name(name)
            .map(|existing_index| self.entries.remove(existing_index));
        self.entries.push(LocalEnvEntry {
            location: None,
            name: name.to_string(),
            aliases: Vec::new(),
            value,
            ref_kind: value_ref_kind,
            storage: LocalEnvStorage::LocalOnly,
            binding_facts: local_binding_facts_for_stored_value(value_ref_kind),
            py_facts,
        });
        if let Some(previous) = previous_entry {
            if transient_local_needs_decref(previous.ref_kind) {
                emit_decref_if_not_null(fb, ptr_ty, decref_ref, thread_state_value, previous.value);
            }
        }
    }

    fn delete_location(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        location: LocalLocation,
        name: &str,
        stack_slots: &StackSlots,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
    ) -> Result<(), String> {
        let had_stack_slot = stack_slots.has_name(name);
        let removed_entry = if let Some(index) = self
            .entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
        {
            let previous = self.entries.remove(index);
            if transient_local_needs_decref(previous.ref_kind) {
                emit_decref_if_not_null(fb, ptr_ty, decref_ref, thread_state_value, previous.value);
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
                .clear_value(fb, name, ptr_ty, thread_state_value, decref_ref)
                .expect("slot-backed delete target missing from stack slots");
        }
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let unbound_storage = if should_clear_stack_slot {
            LocalEnvStorage::StackMirror
        } else {
            LocalEnvStorage::LocalOnly
        };
        self.entries.push(LocalEnvEntry {
            location: removed_entry
                .as_ref()
                .and_then(|entry| entry.location)
                .or(Some(location)),
            name: name.to_string(),
            aliases: removed_entry
                .as_ref()
                .map(|entry| entry.aliases.clone())
                .unwrap_or_default(),
            value: null_ptr,
            ref_kind: LocalRefKind::Unbound,
            storage: unbound_storage,
            binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Unbound),
            py_facts: None,
        });
        Ok(())
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

    #[cfg(test)]
    fn local_only_cleanup_values(&self) -> Vec<ir::Value> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.storage == LocalEnvStorage::LocalOnly
                    && transient_local_needs_decref(entry.ref_kind)
            })
            .map(|entry| entry.value)
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
                    && transient_local_needs_decref(entry.ref_kind)
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
                    && !preserved_values.contains(&entry.value)
                    && transient_local_needs_decref(entry.ref_kind)
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
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
    propagate_entry_py_facts: bool,
) -> Result<(), String> {
    for entry in &jit_local_plan.entry_materializations[block_index] {
        let binding = &entry.binding;
        let entry_py_facts = if propagate_entry_py_facts {
            binding.param_facts.value
        } else {
            None
        };
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
                let entry_storage = match binding.storage {
                    PlannedLocalStorage::BlockParam => LocalEnvStorage::LocalOnly,
                    PlannedLocalStorage::StackSlot => LocalEnvStorage::StackMirror,
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
                if entry_storage == LocalEnvStorage::StackMirror {
                    stack_slots
                        .replace_cloned_value(
                            fb,
                            binding.name.as_str(),
                            param_value,
                            ptr_ty,
                            thread_state_value,
                            incref_ref,
                            decref_ref,
                        )
                        .expect("runtime block param missing from stack slots");
                    emit_decref_if_not_null(
                        fb,
                        ptr_ty,
                        decref_ref,
                        thread_state_value,
                        param_value,
                    );
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

fn transient_local_needs_decref(ref_kind: LocalRefKind) -> bool {
    match ref_kind {
        LocalRefKind::Owned | LocalRefKind::Unknown => true,
        LocalRefKind::Borrowed | LocalRefKind::Immortal | LocalRefKind::Unbound => false,
    }
}

fn local_ref_kind_needs_incref_for_forward(ref_kind: LocalRefKind, forwarded_count: usize) -> bool {
    match ref_kind {
        LocalRefKind::Owned | LocalRefKind::Unknown => forwarded_count > 0,
        LocalRefKind::Borrowed | LocalRefKind::Unbound => true,
        LocalRefKind::Immortal => false,
    }
}

enum PlannedLocalStoreEffect {
    Rebind(LocalRefKind),
    Delete,
}

fn local_ref_kind_for_planned_local_state(state: LocalRefState) -> LocalRefKind {
    match state {
        LocalRefState::Unbound => LocalRefKind::Unbound,
        LocalRefState::Owned => LocalRefKind::Owned,
        LocalRefState::Immortal => LocalRefKind::Immortal,
    }
}

fn planned_local_store_effect(
    expr: &InstrCodegen,
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

fn local_ref_kind_for_stored_value(value: &InstrCodegen, ctx: &JitEmitCtx<'_>) -> LocalRefKind {
    match ctx
        .value_facts_for_expr(value)
        .and_then(ValueFacts::as_pyobj)
    {
        Some(facts) if facts.is_immortal() => LocalRefKind::Immortal,
        _ => LocalRefKind::Owned,
    }
}

fn py_facts_for_codegen_expr_with_local_env(
    expr: &InstrCodegen,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Option<PyObjFacts> {
    if let InstrCodegen::Load(op) = expr {
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
                    ownership: TypedPyObjectOwnershipPlan::BorrowedLocal,
                }) if borrowed_ok => ValueOwnership::Borrowed,
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
    let closure_slot = storage_layout.local_cell_slot(slot)?;
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
    expr: &InstrCodegen,
    op: &Store<InstrCodegen>,
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
        ownership.is_owned(),
        "legacy local store result should produce an owned PyObject"
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
            fb.ins().call(emit_ctx.incref_ref, &[none_const]);
            EmitResult::owned_pyobject(none_const, PyObjFacts::none_singleton())
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
    expr: &InstrCodegen,
    op: &Store<InstrCodegen>,
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
                    emit_ctx.consts.ptr_ty,
                    emit_ctx.consts.thread_state_value,
                    emit_ctx.decref_ref,
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
            &emit_ctx.stack_slots,
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.incref_ref,
            emit_ctx.decref_ref,
        );
        return Some(emit_none_for_demand(fb, emit_ctx, demand));
    }

    let location = op.name.cell_location()?;
    if !(location.is_owned() && matches!(op.value.as_ref(), InstrCodegen::MakeCell(_))) {
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
                .local_cell_slot(location.slot())
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
            &emit_ctx.stack_slots,
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.incref_ref,
            emit_ctx.decref_ref,
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
    if matches!(
        planned_typed_local_store_effect(expr, location, emit_ctx),
        Some(PlannedLocalStoreEffect::Delete)
    ) {
        local_env
            .delete_location(
                fb,
                location,
                name,
                &emit_ctx.stack_slots,
                emit_ctx.consts.ptr_ty,
                emit_ctx.consts.thread_state_value,
                emit_ctx.decref_ref,
            )
            .unwrap_or_else(|error| panic!("{error}"));
        return Ok(Some(emit_none_for_demand(fb, emit_ctx, demand)));
    }

    let value_demand = op
        .value
        .result_demand()
        .unwrap_or(ResultDemand::PYOBJECT_OWNED);
    let value_result = match value_demand {
        ResultDemand::PyObject { borrowed_ok: false } => {
            emit_typed_codegen_stmt_result_with_local_env(
                fb,
                &op.value,
                local_env,
                emit_ctx,
                value_demand,
                codegen_env,
                func_imports,
            )?
        }
        other => {
            return Err(format!(
                "typed local store RHS requires owned PyObject demand, got {other:?}"
            ));
        }
    };
    let (value, ownership, value_py_facts) = value_result.expect_pyobject("typed local store RHS");
    let value_py_facts = if matches!(emit_ctx.function_kind, FunctionKind::Function) {
        py_facts_for_typed_expr_with_local_env(&op.value, local_env).unwrap_or(value_py_facts)
    } else {
        value_py_facts
    };
    if !ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED) {
        return Err(format!(
            "typed local store RHS produced {ownership:?}, but store requires owned PyObject"
        ));
    }
    let value_ref_kind = match planned_typed_local_store_effect(expr, location, emit_ctx) {
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
        &emit_ctx.stack_slots,
        emit_ctx.consts.ptr_ty,
        emit_ctx.consts.thread_state_value,
        emit_ctx.incref_ref,
        emit_ctx.decref_ref,
    );
    Ok(Some(emit_none_for_demand(fb, emit_ctx, demand)))
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
                .local_cell_slot(location.slot())
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
            &emit_ctx.stack_slots,
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.incref_ref,
            emit_ctx.decref_ref,
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
    fb.ins().call(
        emit_ctx.decref_ref,
        &[emit_ctx.consts.thread_state_value, raw_cell],
    );
    if ownership.is_owned() {
        fb.ins().call(
            emit_ctx.decref_ref,
            &[emit_ctx.consts.thread_state_value, value],
        );
    }
    let call_value = fb.inst_results(call_inst)[0];
    let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
        fb,
        local_env,
        ctx: emit_ctx,
        codegen_env,
        func_imports,
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
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
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

fn emit_local_delete_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    op: &Del<InstrCodegen>,
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
    op: &Del<InstrCodegen>,
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
            emit_ctx.consts.ptr_ty,
            emit_ctx.consts.thread_state_value,
            emit_ctx.decref_ref,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    Some(emit_none_for_demand(fb, emit_ctx, demand))
}

#[derive(Clone)]
struct StackSlots {
    names: Vec<String>,
    slots: Vec<ir::StackSlot>,
}

impl StackSlots {
    fn new(fb: &mut FunctionBuilder<'_>, slot_names: &[String]) -> Self {
        let mut slots = Vec::with_capacity(slot_names.len());
        for _ in slot_names {
            slots.push(fb.create_sized_stack_slot(ir::StackSlotData::new(
                ir::StackSlotKind::ExplicitSlot,
                std::mem::size_of::<u64>() as u32,
                0,
            )));
        }
        Self {
            names: slot_names.to_vec(),
            slots,
        }
    }

    fn slot_for_name(&self, name: &str) -> Option<ir::StackSlot> {
        self.names
            .iter()
            .position(|candidate| candidate == name)
            .map(|index| self.slots[index])
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

    fn replace_cloned_value(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
    ) -> Option<()> {
        let slot = self.slot_for_name(name)?;
        let previous = fb.ins().stack_load(ptr_ty, slot, 0);
        emit_incref_if_not_null(fb, ptr_ty, incref_ref, value);
        fb.ins().stack_store(value, slot, 0);
        emit_decref_if_not_null(fb, ptr_ty, decref_ref, thread_state_value, previous);
        Some(())
    }

    fn clear_value(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
    ) -> Option<()> {
        let slot = self.slot_for_name(name)?;
        let previous = fb.ins().stack_load(ptr_ty, slot, 0);
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        fb.ins().stack_store(null_ptr, slot, 0);
        emit_decref_if_not_null(fb, ptr_ty, decref_ref, thread_state_value, previous);
        Some(())
    }

    fn decref_all(
        &self,
        fb: &mut FunctionBuilder<'_>,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        decref_ref: ir::FuncRef,
    ) {
        for slot in &self.slots {
            let value = fb.ins().stack_load(ptr_ty, *slot, 0);
            emit_decref_if_not_null(fb, ptr_ty, decref_ref, thread_state_value, value);
        }
    }
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

impl<'a, 'b, 'mc, 'c, 'd, Env: JitCodegenEnv> intrinsics::OperationEmitState<'b, InstrCodegen>
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

    fn emit_arg_values(&mut self, args: &[&InstrCodegen]) -> Vec<(ir::Value, bool)> {
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

    fn py_facts_for_arg(&self, arg: &InstrCodegen) -> PyObjFacts {
        py_facts_for_codegen_expr_with_local_env(arg, self.local_env, self.ctx)
            .unwrap_or_else(PyObjFacts::unknown)
    }

    fn prepare_guard_miss_dispatch_for_instr(
        &mut self,
        instr_id: InstrId,
        pre_guard_operands: &[&InstrCodegen],
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
            let value = emit_typed_codegen_expr_value_with_local_env(
                self.fb,
                arg,
                &mut *self.local_env,
                self.ctx,
                borrowed_arg,
                self.codegen_env,
                self.func_imports,
            )
            .unwrap_or_else(|err| panic!("{err}"));
            let (value, ownership, _) = value.expect_pyobject("typed intrinsic PyObject argument");
            arg_values.push((value, borrowed_arg || !ownership.is_owned()));
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalEnvEdgePrepError {
    MissingSourceBinding { source_name: String },
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
        let value = entry.value;
        let forwarded_count = forwarded_local_counts.entry(value_index).or_insert(0usize);
        if local_ref_kind_needs_incref_for_forward(entry.ref_kind, *forwarded_count) {
            emit_incref_if_not_null(fb, ctx.consts.ptr_ty, ctx.incref_ref, value);
        }
        *forwarded_count += 1;
        return Ok((value, Some(value_index)));
    }
    if let Some(slot) = ctx.stack_slots.slot_for_block_arg_name(source_name) {
        let value = fb.ins().stack_load(ctx.consts.ptr_ty, slot, 0);
        emit_incref_if_not_null(fb, ctx.consts.ptr_ty, ctx.incref_ref, value);
        return Ok((value, None));
    }
    Err(LocalEnvEdgePrepError::MissingSourceBinding {
        source_name: source_name.to_string(),
    })
}

fn emit_checked_local_value_or_unbound(
    fb: &mut FunctionBuilder<'_>,
    name: &str,
    value: ir::Value,
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
        fb.ins()
            .jump(done_block, &[ir::BlockArg::Value(fallthrough_value)]);

        fb.switch_to_block(value_ok_block);
        fb.ins().jump(done_block, &[ir::BlockArg::Value(value)]);

        fb.switch_to_block(done_block);
        let value = fb.block_params(done_block)[0];
        if !borrowed {
            emit_incref_if_not_null(fb, ctx.consts.ptr_ty, ctx.incref_ref, value);
        }
        return value;
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
    fb.ins()
        .call(ctx.raise_unbound_local_error_ref, &[name_obj]);
    emit_release_owned_inputs(fb, ctx, &[name_obj]);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(value_ok_block);
    let value = fb.block_params(value_ok_block)[0];
    if !borrowed {
        fb.ins().call(ctx.incref_ref, &[value]);
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
    continuation: PendingLocalFailureContinuation,
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
        fb.ins().call(
            ctx.decref_ref,
            &[ctx.consts.thread_state_value, *owned_input],
        );
    }
}

fn emit_decref_owned_input_after_nullable_result(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    result: ir::Value,
    owned_input: ir::Value,
) -> ir::Value {
    emit_decref_owned_inputs_after_nullable_result(fb, ctx, result, &[owned_input])
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
                fb.ins()
                    .call(ctx.decref_ref, &[ctx.consts.thread_state_value, value]);
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

fn scalar_counter_slot_for_id(
    counter_slots_by_id: &[CounterRuntimeSlot],
    counter_id: CounterId,
) -> Result<usize, String> {
    match counter_slots_by_id.get(counter_id.0).copied() {
        Some(CounterRuntimeSlot::Scalar(slot)) => Ok(slot),
        Some(CounterRuntimeSlot::Branches { .. }) => Err(format!(
            "counter id {} uses branch storage where a scalar counter was required",
            counter_id.0
        )),
        Some(CounterRuntimeSlot::TopValues(_)) => Err(format!(
            "counter id {} uses top-value storage where a scalar counter was required",
            counter_id.0
        )),
        None => Err(format!(
            "missing scalar counter slot for counter id {}",
            counter_id.0
        )),
    }
}

pub(super) fn scalar_counter_slot_for_ref(
    counter_slots_by_id: &[CounterRuntimeSlot],
    counter_ref: CounterRef,
) -> Result<usize, String> {
    match (
        counter_slots_by_id.get(counter_ref.counter_id.0).copied(),
        counter_ref.branch_id,
    ) {
        (Some(CounterRuntimeSlot::Scalar(slot)), None) => Ok(slot),
        (Some(CounterRuntimeSlot::Branches { start, len }), Some(branch_id))
            if branch_id.0 < len =>
        {
            Ok(start + branch_id.0)
        }
        (Some(CounterRuntimeSlot::Branches { .. }), None) => Err(format!(
            "counter id {} uses branch storage but no branch was selected",
            counter_ref.counter_id.0
        )),
        (Some(CounterRuntimeSlot::Scalar(_)), Some(branch_id)) => Err(format!(
            "counter id {} uses scalar storage but branch {} was selected",
            counter_ref.counter_id.0, branch_id.0
        )),
        (Some(CounterRuntimeSlot::TopValues(_)), _) => Err(format!(
            "counter id {} uses top-value storage where a scalar counter was required",
            counter_ref.counter_id.0
        )),
        (Some(CounterRuntimeSlot::Branches { len, .. }), Some(branch_id)) => Err(format!(
            "counter id {} branch {} is out of range for {} branches",
            counter_ref.counter_id.0, branch_id.0, len
        )),
        (None, _) => Err(format!(
            "missing scalar counter slot for counter id {}",
            counter_ref.counter_id.0
        )),
    }
}

pub(super) fn top_value_counter_slot_for_id(
    counter_slots_by_id: &[CounterRuntimeSlot],
    counter_id: CounterId,
) -> Result<usize, String> {
    match counter_slots_by_id.get(counter_id.0).copied() {
        Some(CounterRuntimeSlot::TopValues(slot)) => Ok(slot),
        Some(CounterRuntimeSlot::Scalar(_)) => Err(format!(
            "counter id {} uses scalar storage where a top-value counter was required",
            counter_id.0
        )),
        Some(CounterRuntimeSlot::Branches { .. }) => Err(format!(
            "counter id {} uses branch storage where a top-value counter was required",
            counter_id.0
        )),
        None => Err(format!(
            "missing top-value counter slot for counter id {}",
            counter_id.0
        )),
    }
}

fn scalar_counter_byte_offset(counter_slot: usize) -> i64 {
    counter_slot
        .checked_mul(std::mem::size_of::<u64>())
        .and_then(|offset| i64::try_from(offset).ok())
        .unwrap_or_else(|| panic!("scalar counter byte offset overflow for slot {counter_slot}"))
}

fn scalar_counter_addr(
    fb: &mut FunctionBuilder<'_>,
    scalar_counter_base_value: ir::Value,
    counter_slot: usize,
) -> (ir::Value, i32) {
    let byte_offset = scalar_counter_byte_offset(counter_slot);
    if let Ok(offset) = i32::try_from(byte_offset) {
        (scalar_counter_base_value, offset)
    } else {
        (fb.ins().iadd_imm(scalar_counter_base_value, byte_offset), 0)
    }
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
    let (counter_addr, counter_offset) =
        scalar_counter_addr(fb, scalar_counter_base_value, counter_slot);
    let old_value = fb.ins().load(
        ir::types::I64,
        ir::MemFlags::trusted(),
        counter_addr,
        counter_offset,
    );
    let new_value = fb.ins().iadd_imm(old_value, 1);
    fb.ins().store(
        ir::MemFlags::trusted(),
        new_value,
        counter_addr,
        counter_offset,
    );
    // TODO: Split codegen instructions into value-producing vs non-value-producing ops
    // and elide retain/release work when a statement result is not consumed.
    let none_const = emit_none_const(fb, ctx);
    fb.ins().call(ctx.incref_ref, &[none_const]);
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

pub(super) fn emit_increment_counter_slot(
    fb: &mut FunctionBuilder<'_>,
    scalar_counter_base_value: ir::Value,
    counter_slot: usize,
) {
    let (counter_addr, counter_offset) =
        scalar_counter_addr(fb, scalar_counter_base_value, counter_slot);
    let old_value = fb.ins().load(
        ir::types::I64,
        ir::MemFlags::trusted(),
        counter_addr,
        counter_offset,
    );
    let new_value = fb.ins().iadd_imm(old_value, 1);
    fb.ins().store(
        ir::MemFlags::trusted(),
        new_value,
        counter_addr,
        counter_offset,
    );
}

fn top_value_counter_byte_offset(counter_slot: usize) -> i64 {
    counter_slot
        .checked_mul(std::mem::size_of::<TopValueCounter>())
        .and_then(|offset| i64::try_from(offset).ok())
        .unwrap_or_else(|| panic!("top-value counter byte offset overflow for slot {counter_slot}"))
}

pub(super) fn emit_record_top_value_counter_slot(
    fb: &mut FunctionBuilder<'_>,
    top_value_counter_base_value: ir::Value,
    counter_slot: usize,
    observed_value: ir::Value,
    record_top_value_sample_ref: ir::FuncRef,
) {
    let counter_addr = fb.ins().iadd_imm(
        top_value_counter_base_value,
        top_value_counter_byte_offset(counter_slot),
    );
    fb.ins()
        .call(record_top_value_sample_ref, &[counter_addr, observed_value]);
}

#[derive(Clone, Copy, Debug, Default)]
struct CountedRefcountHelpers {
    incref_func_id: Option<FuncId>,
    decref_func_id: Option<FuncId>,
}

fn lookup_counter_id(
    counter_defs: &[CounterDef],
    scope: CounterScope,
    kind: &str,
    site: &CounterSite,
) -> Option<CounterId> {
    counter_defs.iter().find_map(|counter| {
        (counter.scope == scope && counter.kind == kind && &counter.site == site)
            .then_some(counter.id)
    })
}

fn lookup_runtime_counter_id(
    counter_defs: &[CounterDef],
    function_id: RuntimeFunctionId,
    kind: &str,
) -> Option<CounterId> {
    lookup_counter_id(
        counter_defs,
        CounterScope::Function,
        kind,
        &CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: None,
        },
    )
    .or_else(|| {
        lookup_counter_id(
            counter_defs,
            CounterScope::Global,
            kind,
            &CounterSite::Runtime {
                function_id: None,
                instr_id: None,
            },
        )
    })
}

fn build_counted_runtime_refcount_helper(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    symbol_name: &str,
    function_name: &str,
    wrapper_import: &'static ImportSpec,
    applied_import: &'static ImportSpec,
    scalar_counter_data_id: DataId,
    counter_slot: usize,
) -> Result<FuncId, String> {
    let ptr_ty = jit_module.codegen_target_config().pointer_type();
    let sig = lower_static_signature(jit_module, wrapper_import.signature);
    let helper_id = declare_local_fn(jit_module, symbol_name, &sig)?;

    let mut ctx = jit_module.codegen_make_context();
    ctx.func.signature = sig;
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = fb.create_block();
        fb.append_block_params_for_function_params(entry_block);
        fb.switch_to_block(entry_block);
        let call_args = fb.block_params(entry_block).to_vec();
        let mut module_imports = ModuleFuncImports::new();
        let mut func_imports = FuncBuildImports::new(&mut module_imports);
        let runtime_ref = func_imports.get_or_panic(jit_module, &mut fb.func, applied_import);
        let runtime_call = fb.ins().call(runtime_ref, &call_args);
        let applied = fb.inst_results(runtime_call)[0];
        let counter_data =
            jit_module.codegen_declare_data_in_func(scalar_counter_data_id, &mut fb.func)?;
        let scalar_counter_base_value = fb.ins().global_value(ptr_ty, counter_data);
        let (counter_addr, counter_offset) =
            scalar_counter_addr(&mut fb, scalar_counter_base_value, counter_slot);
        let old_value = fb.ins().load(
            ir::types::I64,
            ir::MemFlags::trusted(),
            counter_addr,
            counter_offset,
        );
        let applied_i64 = fb.ins().uextend(ir::types::I64, applied);
        let new_value = fb.ins().iadd(old_value, applied_i64);
        fb.ins().store(
            ir::MemFlags::trusted(),
            new_value,
            counter_addr,
            counter_offset,
        );
        fb.ins().return_(&[]);
        fb.seal_all_blocks();
        fb.finalize();
    }

    let _ = define_prepared_function(
        jit_module,
        env_config,
        helper_id,
        &mut ctx,
        function_name,
        "failed to define counted runtime refcount helper",
    )?;
    jit_module.codegen_clear_context(&mut ctx);
    Ok(helper_id)
}

fn build_counted_runtime_refcount_helpers(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    function: &BlockPyFunction<impl ModuleShape>,
    counter_defs: &[CounterDef],
    counter_slots_by_id: &[CounterRuntimeSlot],
    scalar_counter_data_id: Option<DataId>,
    symbol_scope: Option<&str>,
) -> Result<CountedRefcountHelpers, String> {
    if !env_config.jit_refcount_emission_enabled() {
        return Ok(CountedRefcountHelpers::default());
    }

    let incref_func_id =
        lookup_runtime_counter_id(counter_defs, function.function_id, "runtime_incref")
            .map(|counter_id| {
                let counter_slot = scalar_counter_slot_for_id(counter_slots_by_id, counter_id)?;
                let scalar_counter_data_id = scalar_counter_data_id.ok_or_else(|| {
                    format!(
                        "missing scalar counter storage for runtime incref counter {}",
                        counter_id.0
                    )
                })?;
                let helper_name = scoped_jit_symbol(
                    format!("py:rc:incref:{}", function.names.qualname).as_str(),
                    symbol_scope,
                );
                build_counted_runtime_refcount_helper(
                    jit_module,
                    env_config,
                    &helper_name,
                    &helper_name,
                    &DP_JIT_INCREF_IMPORT,
                    &SOAC_RUNTIME_INCREF_APPLIED_IMPORT,
                    scalar_counter_data_id,
                    counter_slot,
                )
            })
            .transpose()?;

    let decref_func_id =
        lookup_runtime_counter_id(counter_defs, function.function_id, "runtime_decref")
            .map(|counter_id| {
                let counter_slot = scalar_counter_slot_for_id(counter_slots_by_id, counter_id)?;
                let scalar_counter_data_id = scalar_counter_data_id.ok_or_else(|| {
                    format!(
                        "missing scalar counter storage for runtime decref counter {}",
                        counter_id.0
                    )
                })?;
                let helper_name = scoped_jit_symbol(
                    format!("py:rc:decref:{}", function.names.qualname).as_str(),
                    symbol_scope,
                );
                build_counted_runtime_refcount_helper(
                    jit_module,
                    env_config,
                    &helper_name,
                    &helper_name,
                    &DP_JIT_DECREF_IMPORT,
                    &SOAC_RUNTIME_DECREF_APPLIED_IMPORT,
                    scalar_counter_data_id,
                    counter_slot,
                )
            })
            .transpose()?;

    Ok(CountedRefcountHelpers {
        incref_func_id,
        decref_func_id,
    })
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
    fb.ins().call(ctx.incref_ref, &[raw_cell_value]);
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
                .and_then(|layout| layout.local_cell_slot(slot))
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
    values: &[ir::Value],
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    if values.is_empty() {
        let empty_tuple_const = emit_empty_tuple_const(fb, ctx);
        fb.ins().call(ctx.incref_ref, &[empty_tuple_const]);
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
    let values_base = fb.ins().stack_addr(ptr_ty, stack_slot, 0);

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
    fb.ins().call(ctx.incref_ref, &[value]);
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
    tuple: &blockpy_intrinsics::Tuple<InstrCodegen>,
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
    let tuple_value = emit_pack_current_values_tuple(fb, arg_values.as_slice(), emit_ctx);
    for (value, borrowed_arg) in arg_values.into_iter().zip(borrowed_args.into_iter()) {
        if !borrowed_arg {
            fb.ins().call(
                emit_ctx.decref_ref,
                &[emit_ctx.consts.thread_state_value, value],
            );
        }
    }
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
    let tuple_value = emit_pack_current_values_tuple(fb, arg_values.as_slice(), emit_ctx);
    for (value, borrowed_arg) in arg_values.into_iter().zip(borrowed_args.into_iter()) {
        if !borrowed_arg {
            fb.ins().call(
                emit_ctx.decref_ref,
                &[emit_ctx.consts.thread_state_value, value],
            );
        }
    }
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
            fb.ins().call(ctx.incref_ref, &[*value]);
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
    args: &[&InstrCodegen],
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
    args: &[&InstrCodegen],
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

fn emit_positional_call_three_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrCodegen],
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
    args: &[&InstrCodegen],
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
    args: &[&InstrCodegen],
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
    fb.ins()
        .call(ctx.decref_ref, &[ctx.consts.thread_state_value, key_obj]);
    if !value_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, value_obj]);
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
    fb.ins()
        .call(ctx.decref_ref, &[ctx.consts.thread_state_value, set_value]);
}

fn emit_keyword_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrCodegen],
    keywords: &[(&str, &InstrCodegen)],
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
    args: &[&InstrCodegen],
    keywords: &[(&str, &InstrCodegen)],
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
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        expr,
        local_env,
        ctx,
        typed_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, ctx),
        codegen_env,
        func_imports,
    )?;
    let (value, ownership, _) = value.expect_pyobject("typed PyObject call argument");
    Ok((value, !ownership.is_owned()))
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
    args: &[CallArgPositional<InstrCodegen>],
    keywords: &[CallArgKeyword<InstrCodegen>],
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
    args: &[CallArgPositional<InstrCodegen>],
    keywords: &[CallArgKeyword<InstrCodegen>],
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
    fb.ins()
        .call(ctx.decref_ref, &[ctx.consts.thread_state_value, value]);
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

fn annotate_typed_attr_accesses(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    opt_v3_indexed_fields_by_instr: &HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    specialize_stores: bool,
) -> Result<usize, String> {
    struct Annotator<'a> {
        opt_v3_indexed_fields_by_instr: &'a HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
        specialize_stores: bool,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn opt_v3_guards_for_attr(&mut self, instr_id: InstrId) -> Option<TypedAttrAccessPlan> {
            let accesses = self.opt_v3_indexed_fields_by_instr.get(&instr_id)?;
            let mut guards = Vec::with_capacity(accesses.len());
            for access in accesses {
                guards.push(access.specialization.to_typed_guard());
            }
            Some(TypedAttrAccessPlan::IndexedField {
                source: TypedIndexedFieldPlanSource::OptimizationPlanV3,
                guards,
            })
        }

        fn annotate_attr(
            &mut self,
            instr_id: InstrId,
            expected_access: PlanV3IndexedFieldAccessKind,
        ) -> Option<TypedAttrAccessPlan> {
            if self.opt_v3_indexed_fields_by_instr.contains_key(&instr_id) {
                for access in self.opt_v3_indexed_fields_by_instr.get(&instr_id)? {
                    if access.access != expected_access {
                        self.error = Some(format!(
                            "optimizer v3 indexed-field for {instr_id} was prevalidated as {:?}, but typed node requires {:?}",
                            access.access, expected_access
                        ));
                        return None;
                    }
                }
                return self.opt_v3_guards_for_attr(instr_id);
            }
            None
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrTyped::GetAttrTyped(op) => {
                    if let Some(access) = self
                        .annotate_attr(op.semantic_instr_id(), PlanV3IndexedFieldAccessKind::Load)
                    {
                        op.access = access;
                        self.count += 1;
                    }
                }
                InstrTyped::SetAttrTyped(op) if self.specialize_stores => {
                    if let Some(access) = self
                        .annotate_attr(op.semantic_instr_id(), PlanV3IndexedFieldAccessKind::Store)
                    {
                        op.access = access;
                        self.count += 1;
                    }
                }
                InstrTyped::SetAttrTyped(op)
                    if self
                        .opt_v3_indexed_fields_by_instr
                        .contains_key(&op.semantic_instr_id()) =>
                {
                    self.error = Some(format!(
                        "optimizer v3 indexed-field store emission for {} cannot be consumed because indexed stores are disabled",
                        op.semantic_instr_id()
                    ));
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        opt_v3_indexed_fields_by_instr,
        specialize_stores,
        count: 0,
        error: None,
    };
    for block in &mut function.blocks {
        for instr in &mut block.body {
            annotator.visit_instr_mut(instr);
        }
        annotator.visit_term_mut(&mut block.term);
        if let Some(error) = annotator.error.take() {
            return Err(error);
        }
    }
    Ok(annotator.count)
}

fn annotate_typed_indexed_field_accesses_from_profile(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let (_, _, opt_v3_indexed_fields_by_instr) =
        profile.field_index_specialization_maps(function.function_id)?;
    if opt_v3_indexed_fields_by_instr.is_empty() {
        return Ok(());
    }
    let specialize_field_stores = profile.typed_specializations_embedded()
        || (profile.behavior_change_indexed_stores
            && function.scope.scope_kind != CallableScopeKind::Module);
    annotate_typed_attr_accesses(
        function,
        &opt_v3_indexed_fields_by_instr,
        specialize_field_stores,
    )?;
    Ok(())
}

fn typed_indexed_global_access_plan_from_opt_v3(
    plan: &OptV3IndexedGlobalAccessPlan,
) -> TypedIndexedGlobalAccessPlan {
    TypedIndexedGlobalAccessPlan {
        source: TypedIndexedGlobalPlanSource::OptimizationPlanV3,
        instr_id: plan.source,
        access: plan.access,
        module_name: plan.module_name.clone(),
        name: plan.name.clone(),
        expected_index: plan.expected_index,
        guard: plan.guard,
        fallback: plan.fallback,
    }
}

fn annotate_typed_indexed_global_accesses(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    indexed_globals_by_instr: &HashMap<InstrId, OptV3IndexedGlobalAccessPlan>,
) -> Result<usize, String> {
    struct Annotator<'a> {
        indexed_globals_by_instr: &'a HashMap<InstrId, OptV3IndexedGlobalAccessPlan>,
        used: HashSet<InstrId>,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn plan_for_instr(
            &mut self,
            instr_id: InstrId,
            expected_access: PlanV3IndexedGlobalAccessKind,
            location_is_global: bool,
        ) -> Option<TypedIndexedGlobalAccessPlan> {
            let plan = self.indexed_globals_by_instr.get(&instr_id)?;
            if plan.access != expected_access {
                self.error = Some(format!(
                    "optimizer v3 indexed-global plan for {instr_id} expected {:?}, but typed node requires {:?}",
                    plan.access, expected_access
                ));
                return None;
            }
            if !location_is_global {
                self.error = Some(format!(
                    "optimizer v3 indexed-global plan for {instr_id} reached a non-global typed node"
                ));
                return None;
            }
            self.used.insert(instr_id);
            Some(typed_indexed_global_access_plan_from_opt_v3(plan))
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrTyped::Load(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) = self.plan_for_instr(
                            instr_id,
                            PlanV3IndexedGlobalAccessKind::Load,
                            op.name.location.is_global(),
                        )
                    {
                        op.extra_mut().set_indexed_global_access_plan(plan);
                        self.count += 1;
                    }
                }
                InstrTyped::Store(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) = self.plan_for_instr(
                            instr_id,
                            PlanV3IndexedGlobalAccessKind::Store,
                            op.name.location.is_global(),
                        )
                    {
                        op.extra_mut().set_indexed_global_access_plan(plan);
                        self.count += 1;
                    }
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        indexed_globals_by_instr,
        used: HashSet::new(),
        count: 0,
        error: None,
    };
    annotator.visit_fn_mut(function);
    if let Some(error) = annotator.error {
        return Err(error);
    }
    if annotator.used.len() != indexed_globals_by_instr.len() {
        let missing = indexed_globals_by_instr
            .keys()
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "optimizer v3 indexed-global plans were not attached to typed nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

fn annotate_typed_indexed_global_accesses_from_profile(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let Some(indexed_globals_by_instr) = profile
        .opt_v3_emitted_indexed_globals
        .get(&function.function_id)
    else {
        return Ok(());
    };
    annotate_typed_indexed_global_accesses(function, indexed_globals_by_instr)?;
    Ok(())
}

fn typed_exact_list_item_access_plan_from_opt_v3(
    plan: &OptV3ExactListItemAccessPlan,
) -> TypedExactListItemAccessPlan {
    TypedExactListItemAccessPlan {
        source: TypedExactListItemPlanSource::OptimizationPlanV3,
        instr_id: plan.source,
        access: plan.access,
        shape: plan.shape,
        guard: plan.guard,
        fallback: plan.fallback,
    }
}

fn annotate_typed_exact_list_item_accesses(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    exact_list_items_by_instr: &HashMap<InstrId, OptV3ExactListItemAccessPlan>,
) -> Result<usize, String> {
    struct Annotator<'a> {
        exact_list_items_by_instr: &'a HashMap<InstrId, OptV3ExactListItemAccessPlan>,
        used: HashSet<InstrId>,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn plan_for_instr(
            &mut self,
            instr_id: InstrId,
            expected_access: soac_opt::plan_v3::ExactListItemAccessKind,
        ) -> Option<TypedExactListItemAccessPlan> {
            let plan = self.exact_list_items_by_instr.get(&instr_id)?;
            if plan.access != expected_access {
                self.error = Some(format!(
                    "optimizer v3 exact-list item plan for {instr_id} expected {:?}, but typed node requires {:?}",
                    plan.access, expected_access
                ));
                return None;
            }
            self.used.insert(instr_id);
            Some(typed_exact_list_item_access_plan_from_opt_v3(plan))
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrTyped::GetItem(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) = self.plan_for_instr(
                            instr_id,
                            soac_opt::plan_v3::ExactListItemAccessKind::Get,
                        )
                    {
                        op.extra_mut().set_exact_list_item_access_plan(plan);
                        self.count += 1;
                    }
                }
                InstrTyped::SetItem(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) = self.plan_for_instr(
                            instr_id,
                            soac_opt::plan_v3::ExactListItemAccessKind::Set,
                        )
                    {
                        op.extra_mut().set_exact_list_item_access_plan(plan);
                        self.count += 1;
                    }
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        exact_list_items_by_instr,
        used: HashSet::new(),
        count: 0,
        error: None,
    };
    annotator.visit_fn_mut(function);
    if let Some(error) = annotator.error {
        return Err(error);
    }
    if annotator.used.len() != exact_list_items_by_instr.len() {
        let missing = exact_list_items_by_instr
            .keys()
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "optimizer v3 exact-list item plans were not attached to typed nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

fn annotate_typed_exact_list_item_accesses_from_profile(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let Some(exact_list_items_by_instr) = profile
        .opt_v3_emitted_exact_list_items
        .get(&function.function_id)
    else {
        return Ok(());
    };
    annotate_typed_exact_list_item_accesses(function, exact_list_items_by_instr)?;
    Ok(())
}

fn typed_exact_int_branch_plan_from_opt_v3(
    instr_id: InstrId,
    selection: OptV3ExactIntBranchSelection<'_>,
) -> TypedExactIntBranchPlan {
    TypedExactIntBranchPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: selection.hot_plan.clone(),
        hot_region: selection.hot_region.clone(),
        fallback_plan: selection.fallback_plan.clone(),
        fallback_region: selection.fallback_region.clone(),
    }
}

fn typed_exact_int_return_plan_from_opt_v3(
    instr_id: InstrId,
    selection: OptV3ExactIntReturnSelection<'_>,
) -> TypedExactIntReturnPlan {
    TypedExactIntReturnPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: selection.hot_plan.clone(),
        hot_region: selection.hot_region.clone(),
        fallback_plan: selection.fallback_plan.clone(),
        fallback_region: selection.fallback_region.clone(),
    }
}

fn typed_exact_int_scalar_thread_plan_from_opt_v3(
    store_instr_id: InstrId,
    producer_instr_id: InstrId,
    consumer_instr_id: InstrId,
    selection: OptV3ScalarThreadSelection<'_>,
) -> TypedExactIntScalarThreadPlan {
    TypedExactIntScalarThreadPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        store_instr_id,
        producer_instr_id,
        consumer_instr_id,
        thread: selection.thread.clone(),
        producer_hot_plan: selection.producer.hot_plan.clone(),
        producer_hot_region: selection.producer.hot_region.clone(),
        producer_fallback_plan: selection.producer.fallback_plan.clone(),
        producer_fallback_region: selection.producer.fallback_region.clone(),
        consumer_hot_plan: selection.consumer.hot_plan.clone(),
        consumer_hot_region: selection.consumer.hot_region.clone(),
        consumer_fallback_plan: selection.consumer.fallback_plan.clone(),
        consumer_fallback_region: selection.consumer.fallback_region.clone(),
    }
}

fn annotate_typed_exact_int_selections(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    artifacts: &ExactIntBranchV3Artifacts,
) -> Result<usize, String> {
    struct Annotator<'a> {
        artifacts: &'a ExactIntBranchV3Artifacts,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn attach_branch_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let selection =
                match opt_v3_exact_int_branch_selection_for_source(self.artifacts, instr_id) {
                    Ok(selection) => selection,
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                };
            let Some(selection) = selection else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "optimizer v3 exact-int branch plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            let plan = typed_exact_int_branch_plan_from_opt_v3(instr_id, selection);
            self.count += usize::from(extra.set_exact_int_branch_plan(plan));
        }

        fn attach_return_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let selection =
                match opt_v3_exact_int_return_selection_for_source(self.artifacts, instr_id) {
                    Ok(selection) => selection,
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                };
            let Some(selection) = selection else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "optimizer v3 exact-int return plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            let plan = typed_exact_int_return_plan_from_opt_v3(instr_id, selection);
            self.count += usize::from(extra.set_exact_int_return_plan(plan));
        }

        fn attach_scalar_thread_plan(
            &mut self,
            store_expr: &mut InstrTyped,
            consumer_test: &InstrTyped,
        ) {
            let Some(store_instr_id) = store_expr.try_semantic_instr_id() else {
                return;
            };
            let InstrTyped::Store(store) = store_expr else {
                return;
            };
            let Some(producer_instr_id) = store.value.try_semantic_instr_id() else {
                return;
            };
            let Some(consumer_instr_id) = consumer_test.try_semantic_instr_id() else {
                return;
            };
            let selection = match opt_v3_scalar_thread_selection_for_store_branch(
                self.artifacts,
                producer_instr_id,
                consumer_instr_id,
                &store.name,
            ) {
                Ok(selection) => selection,
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            };
            let Some(selection) = selection else {
                return;
            };
            let plan = typed_exact_int_scalar_thread_plan_from_opt_v3(
                store_instr_id,
                producer_instr_id,
                consumer_instr_id,
                selection,
            );
            self.count += usize::from(store.extra_mut().set_exact_int_scalar_thread_plan(plan));
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            self.attach_return_plan(expr);
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        artifacts,
        count: 0,
        error: None,
    };
    let empty_if_tests_by_label = function
        .blocks
        .iter()
        .filter_map(|block| {
            if !block.body.is_empty() {
                return None;
            }
            let BlockTerm::IfTerm(if_term) = &block.term else {
                return None;
            };
            Some((block.label, if_term.test.clone()))
        })
        .collect::<HashMap<_, _>>();
    for block in &mut function.blocks {
        if let [store_expr] = block.body.as_mut_slice()
            && let BlockTerm::Jump(edge) = &block.term
            && edge.args.is_empty()
            && let Some(consumer_test) = empty_if_tests_by_label.get(&edge.target)
        {
            annotator.attach_scalar_thread_plan(store_expr, consumer_test);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        if let BlockTerm::IfTerm(if_term) = &mut block.term {
            annotator.attach_branch_plan(&mut if_term.test);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        for instr in &mut block.body {
            annotator.visit_instr_mut(instr);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        annotator.visit_term_mut(&mut block.term);
        if let Some(error) = annotator.error.take() {
            return Err(error);
        }
    }
    Ok(annotator.count)
}

fn annotate_typed_exact_int_selections_from_profile(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let Some(artifacts) = profile
        .opt_v3_exact_int_branch_artifacts
        .get(&function.function_id)
    else {
        return Ok(());
    };
    annotate_typed_exact_int_selections(function, artifacts)?;
    Ok(())
}

fn call_site_profiled_targets<'a>(
    call: &blockpy_intrinsics::Call<InstrCodegen>,
    profiled_targets: Option<&'a [RuntimeFunctionId]>,
) -> Option<&'a [RuntimeFunctionId]> {
    let _ = call.try_semantic_instr_id()?;
    profiled_targets.filter(|targets| !targets.is_empty())
}

fn collect_call_direct_targets(
    _function: &BlockPyFunction<CodegenModuleShape>,
) -> HashSet<RuntimeFunctionId> {
    HashSet::new()
}

fn collect_typed_call_direct_targets(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> HashSet<RuntimeFunctionId> {
    struct CallDirectTargetCollector<'a> {
        out: &'a mut HashSet<RuntimeFunctionId>,
    }

    impl Visit<InstrTyped> for CallDirectTargetCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallDirect(call) = expr {
                self.out.insert(call.function_id);
            }
            if let InstrTyped::GuardedCallableCallTyped(call) = expr {
                self.out.extend(
                    call.function_guards
                        .iter()
                        .map(|guard| guard.function_id)
                        .chain(
                            call.constructor_guards
                                .iter()
                                .map(|guard| guard.function_id),
                        ),
                );
            }
            if let InstrTyped::GuardedMethodCallTyped(call) = expr {
                self.out
                    .extend(call.method_guards.iter().map(|guard| guard.function_id));
            }
            if let InstrTyped::DirectCallableCallTyped(call) = expr {
                match &call.guard {
                    TypedDirectCallableCallGuard::Function(guard) => {
                        self.out.insert(guard.function_id);
                    }
                    TypedDirectCallableCallGuard::Constructor(guard) => {
                        self.out.insert(guard.function_id);
                    }
                }
            }
            if let InstrTyped::DirectMethodCallTyped(call) = expr {
                self.out.insert(call.guard.function_id);
            }
            expr.visit_children(self);
        }
    }

    let mut out = HashSet::new();
    let mut collector = CallDirectTargetCollector { out: &mut out };
    collector.visit_fn(function);
    out
}

fn collect_planned_typed_call_direct_targets(
    module_plan: &JitModulePlan,
    function_id: RuntimeFunctionId,
) -> Result<HashSet<RuntimeFunctionId>, String> {
    let planned_function = module_plan
        .module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .ok_or_else(|| {
            format!("planned JIT module is missing function {function_id} for direct-call targets")
        })?;
    Ok(collect_typed_call_direct_targets(planned_function))
}

fn codegen_expr_const_i64(
    expr: &InstrCodegen,
    module_constants: &ModuleCodegenConstants,
) -> Option<i64> {
    match expr {
        InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
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

fn collect_make_function_targets(
    function: &BlockPyFunction<CodegenModuleShape>,
) -> HashSet<RuntimeFunctionId> {
    struct MakeFunctionTargetCollector<'a> {
        out: &'a mut HashSet<RuntimeFunctionId>,
    }

    impl Visit<InstrCodegen> for MakeFunctionTargetCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            if let InstrCodegen::MakeFunctionWithClosure(op) = expr {
                self.out.insert(op.function_id());
            }
            expr.visit_children(self);
        }
    }

    let mut out = HashSet::new();
    let mut collector = MakeFunctionTargetCollector { out: &mut out };
    collector.visit_fn(function);
    out
}

pub(crate) fn is_synthetic_class_helper_function(
    function: &BlockPyFunction<CodegenModuleShape>,
) -> bool {
    function.names.bind_name.starts_with("_dp_class_ns_")
        || function.names.bind_name.starts_with("_dp_define_class_")
}

fn collect_runtime_counter_ids_by_kind(
    counter_defs: &[CounterDef],
    function_id: RuntimeFunctionId,
    kind: &str,
) -> HashMap<InstrId, CounterId> {
    counter_defs
        .iter()
        .filter_map(|counter| match &counter.site {
            CounterSite::Runtime {
                function_id: Some(counter_function_id),
                instr_id: Some(instr_id),
            } if counter.kind == kind && *counter_function_id == function_id => {
                Some((*instr_id, counter.id))
            }
            _ => None,
        })
        .collect()
}

fn collect_runtime_counter_refs_by_kind_branch(
    counter_defs: &[CounterDef],
    function_id: RuntimeFunctionId,
    kind: &str,
    branch: &str,
) -> HashMap<InstrId, CounterRef> {
    counter_defs
        .iter()
        .filter_map(|counter| match &counter.site {
            CounterSite::Runtime {
                function_id: Some(counter_function_id),
                instr_id: Some(instr_id),
            } if counter.kind == kind && *counter_function_id == function_id => {
                let branch_id = counter.branch_id(branch)?;
                Some((*instr_id, CounterRef::branch(counter.id, branch_id)))
            }
            _ => None,
        })
        .collect()
}

fn deopt_entry_source_for_resume_point(point: LocalEnvResumePoint) -> DeoptEntrySource {
    match point {
        LocalEnvResumePoint::BlockEntry { block, .. } => {
            DeoptEntrySource::BlockEntry { block_label: block }
        }
        LocalEnvResumePoint::BeforeInstr { key } => DeoptEntrySource::BeforeInstr {
            instr_id: key.instr_id,
        },
        LocalEnvResumePoint::BeforeTerm { block, .. } => {
            DeoptEntrySource::BeforeTerm { block_label: block }
        }
    }
}

fn collect_deopt_entry_counter_ids_by_kind(
    counter_defs: &[CounterDef],
    function_id: RuntimeFunctionId,
    kind: &str,
    deopt_resume_plan: &PlannedJitDeoptResumeFunction,
) -> HashMap<usize, CounterId> {
    counter_defs
        .iter()
        .filter_map(|counter| match &counter.site {
            CounterSite::DeoptEntry {
                function_id: counter_function_id,
                source,
            } if counter.kind == kind && *counter_function_id == function_id => {
                let ordinal = deopt_resume_plan
                    .deopt_points
                    .iter()
                    .find(|point| deopt_entry_source_for_resume_point(point.point) == *source)?
                    .id
                    .ordinal;
                Some((ordinal, counter.id))
            }
            _ => None,
        })
        .collect()
}

#[derive(Clone)]
struct SpecializationProfile<'a> {
    module_name: Option<&'a str>,
    counter_dump_path: Option<Cow<'a, Path>>,
    direct_call_emission_scope: DirectCallEmissionScope,
    opt_v3_emitted_direct_calls:
        HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
    opt_v3_emitted_exact_list_items:
        HashMap<RuntimeFunctionId, HashMap<InstrId, OptV3ExactListItemAccessPlan>>,
    opt_v3_emitted_indexed_fields:
        HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<OptV3IndexedFieldAccessPlan>>>,
    opt_v3_emitted_indexed_globals:
        HashMap<RuntimeFunctionId, HashMap<InstrId, OptV3IndexedGlobalAccessPlan>>,
    opt_v3_exact_int_branch_artifacts: HashMap<RuntimeFunctionId, Arc<ExactIntBranchV3Artifacts>>,
    behavior_change_indexed_stores: bool,
    profiled_cold_blocks: bool,
    guard_miss_deopt: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirectCallEmissionScope {
    DirectCallBodiesOnly,
    AllDirectCallCandidates,
}

#[derive(Clone, Default)]
struct PlannedOptimizationInputs {
    opt_v3_emitted_direct_calls:
        HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
    opt_v3_emitted_exact_list_items:
        HashMap<RuntimeFunctionId, HashMap<InstrId, OptV3ExactListItemAccessPlan>>,
    opt_v3_emitted_indexed_fields:
        HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<OptV3IndexedFieldAccessPlan>>>,
    opt_v3_emitted_indexed_globals:
        HashMap<RuntimeFunctionId, HashMap<InstrId, OptV3IndexedGlobalAccessPlan>>,
    opt_v3_exact_int_branch_artifacts: HashMap<RuntimeFunctionId, Arc<ExactIntBranchV3Artifacts>>,
}

impl PlannedOptimizationInputs {
    fn has_v3_optimization_inputs(&self) -> bool {
        !self.opt_v3_emitted_direct_calls.is_empty()
            || !self.opt_v3_emitted_exact_list_items.is_empty()
            || !self.opt_v3_emitted_indexed_fields.is_empty()
            || !self.opt_v3_emitted_indexed_globals.is_empty()
            || !self.opt_v3_exact_int_branch_artifacts.is_empty()
    }

    fn v3_direct_function_call_targets(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
        self.opt_v3_emitted_direct_calls
            .get(&function_id)
            .map(opt_v3_direct_call_targets)
            .unwrap_or_default()
    }

    fn direct_call_targets_for_batch(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
        self.v3_direct_function_call_targets(function_id)
    }
}

fn load_planned_optimization_inputs_for_runtime_state(
    shared_state: Option<&SharedModuleState>,
    compile_session: Option<&crate::session::CompileSession>,
    env_config: &SoacEnvConfig,
    specialization_mode: Option<SpecializationMode>,
) -> Result<PlannedOptimizationInputs, String> {
    if !matches!(
        specialization_mode,
        Some(SpecializationMode::Verify | SpecializationMode::Apply)
    ) {
        return Ok(PlannedOptimizationInputs::default());
    }
    if env_config
        .runtime_optimization_pipeline()
        .uses_typed_v3_runtime()
    {
        let Some(shared_state) = shared_state else {
            return Ok(PlannedOptimizationInputs::default());
        };
        return planned_typed_v3_runtime_inputs_from_raw_evidence(
            shared_state,
            compile_session,
            env_config,
        );
    }
    let Some(shared_state) = shared_state else {
        return Ok(PlannedOptimizationInputs::default());
    };
    let Some(cache_root) = env_config.module_cache_root() else {
        return Err(format!(
            "SOAC_OPT_MODE=verify/apply requires SOAC_WORK_DIR/module cache to load mod.optv3 for module {}",
            shared_state.module_name
        ));
    };
    let cache_identity = pre_optimization_module_cache_identity(
        env!("SOAC_BUILD_IDENTITY"),
        shared_state.module_name == "soac.runtime",
    );
    let candidate_sources = match shared_state.module_cache_source {
        Some(source) => vec![source],
        None => vec![
            PythonModuleCacheSource::Project,
            PythonModuleCacheSource::PythonStdlib,
        ],
    };

    for source in candidate_sources.iter().copied() {
        let path = module_optimization_plan_v3_path(
            cache_root.as_path(),
            source,
            shared_state.module_name.as_str(),
        )?;
        if !path.exists() {
            continue;
        }
        let artifacts =
            load_optimization_artifacts_v3(path.as_path()).map_err(|err| err.to_string())?;
        validate_optimization_artifacts_v3_for_module(
            &artifacts,
            shared_state.module_name.as_str(),
            shared_state.source_hash,
            cache_identity.as_str(),
        )
        .map_err(|err| err.to_string())?;
        let inputs = planned_optimization_inputs_from_v3_artifacts(
            &artifacts,
            shared_state,
            compile_session,
        )?;
        return Ok(inputs);
    }
    Err(format!(
        "SOAC_OPT_MODE=verify/apply requires mod.optv3 for module {} under {}",
        shared_state.module_name,
        cache_root.display()
    ))
}

fn planned_typed_v3_runtime_inputs_from_raw_evidence(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    env_config: &SoacEnvConfig,
) -> Result<PlannedOptimizationInputs, String> {
    let Some(counter_dump_path) = env_config.counter_dump_input_path() else {
        return Ok(PlannedOptimizationInputs::default());
    };
    if !counter_dump_path.exists() {
        return Ok(PlannedOptimizationInputs::default());
    }
    let evidence_store = ProfileEvidenceStore::from_counter_dump(counter_dump_path.as_path())
        .map_err(|err| err.to_string())?;
    let artifacts = plan_and_emit_module_v3_from_raw_evidence(
        &AlternativeCatalog::default_v3(),
        ModulePlanIdentity {
            module_name: shared_state.module_name.clone(),
            source_hash: shared_state.source_hash,
            cache_identity: pre_optimization_module_cache_identity(
                env!("SOAC_BUILD_IDENTITY"),
                shared_state.module_name == "soac.runtime",
            ),
        },
        &shared_state.lowered_module,
        &evidence_store,
    )
    .map_err(|err| err.to_string())?;
    planned_optimization_inputs_from_v3_artifacts(&artifacts, shared_state, compile_session)
}

fn planned_optimization_inputs_from_v3_artifacts(
    artifacts: &ExactIntBranchV3Artifacts,
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
) -> Result<PlannedOptimizationInputs, String> {
    let mut inputs = PlannedOptimizationInputs::default();
    for planned_function in &artifacts.plan.functions {
        let local_function_id = planned_function.function.function.local_function_id();
        let current_function_id = RuntimeFunctionId::new(
            RuntimeModuleId::new(shared_state.module_id()),
            local_function_id,
        );
        shared_state
            .lookup_function(current_function_id)
            .ok_or_else(|| {
                format!(
                    "optimization plan v3 for module {} references missing function id {} ({})",
                    artifacts.plan.module.module_name,
                    local_function_id,
                    planned_function
                        .function
                        .debug_name
                        .as_deref()
                        .unwrap_or("<unknown>")
                )
            })?;
        let Some(function_artifacts) =
            opt_v3_single_function_artifacts(artifacts, planned_function.function.function)?
        else {
            continue;
        };
        if let Some(direct_calls) =
            opt_v3_emitted_direct_calls_for_function(&function_artifacts, |target| {
                resolve_opt_v3_runtime_function_target(shared_state, compile_session, target)
            })?
        {
            inputs
                .opt_v3_emitted_direct_calls
                .insert(current_function_id, direct_calls);
        }
        if let Some(items) = opt_v3_emitted_exact_list_items_for_function(&function_artifacts)? {
            inputs
                .opt_v3_emitted_exact_list_items
                .insert(current_function_id, items);
        }
        if let Some(indexed_fields) =
            opt_v3_emitted_indexed_fields_for_function(&function_artifacts)?
        {
            inputs
                .opt_v3_emitted_indexed_fields
                .insert(current_function_id, indexed_fields);
        }
        if let Some(indexed_globals) =
            opt_v3_emitted_indexed_globals_for_function(&function_artifacts)?
        {
            inputs
                .opt_v3_emitted_indexed_globals
                .insert(current_function_id, indexed_globals);
        }
        inputs
            .opt_v3_exact_int_branch_artifacts
            .insert(current_function_id, Arc::new(function_artifacts));
    }
    Ok(inputs)
}

fn planned_optimization_inputs_from_v3_artifacts_for_codegen_module(
    artifacts: &ExactIntBranchV3Artifacts,
    module: &BlockPyModule<CodegenModuleShape>,
    module_name: &str,
    source_hash: u64,
    module_index: Option<&PrecompileModuleIndex>,
) -> Result<PlannedOptimizationInputs, String> {
    let mut inputs = PlannedOptimizationInputs::default();
    let module_id = RuntimeModuleId::new(module.module_name_gen.module_id());
    for planned_function in &artifacts.plan.functions {
        let local_function_id = planned_function.function.function.local_function_id();
        let current_function_id = RuntimeFunctionId::new(module_id, local_function_id);
        module
            .callable_defs
            .iter()
            .find(|function| function.function_id == current_function_id)
            .ok_or_else(|| {
                format!(
                    "optimization plan v3 for module {} references missing function id {} ({})",
                    artifacts.plan.module.module_name,
                    local_function_id,
                    planned_function
                        .function
                        .debug_name
                        .as_deref()
                        .unwrap_or("<unknown>")
                )
            })?;
        let Some(function_artifacts) =
            opt_v3_single_function_artifacts(artifacts, planned_function.function.function)?
        else {
            continue;
        };
        if let Some(direct_calls) =
            opt_v3_emitted_direct_calls_for_function(&function_artifacts, |target| {
                resolve_opt_v3_codegen_module_function_target(
                    module_name,
                    source_hash,
                    module_id,
                    module,
                    module_index,
                    target,
                )
            })?
        {
            inputs
                .opt_v3_emitted_direct_calls
                .insert(current_function_id, direct_calls);
        }
        if let Some(items) = opt_v3_emitted_exact_list_items_for_function(&function_artifacts)? {
            inputs
                .opt_v3_emitted_exact_list_items
                .insert(current_function_id, items);
        }
        if let Some(indexed_fields) =
            opt_v3_emitted_indexed_fields_for_function(&function_artifacts)?
        {
            inputs
                .opt_v3_emitted_indexed_fields
                .insert(current_function_id, indexed_fields);
        }
        if let Some(indexed_globals) =
            opt_v3_emitted_indexed_globals_for_function(&function_artifacts)?
        {
            inputs
                .opt_v3_emitted_indexed_globals
                .insert(current_function_id, indexed_globals);
        }
        inputs
            .opt_v3_exact_int_branch_artifacts
            .insert(current_function_id, Arc::new(function_artifacts));
    }
    Ok(inputs)
}

fn resolve_opt_v3_runtime_function_target(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    target: PersistentFunctionId,
) -> Result<Option<RuntimeFunctionId>, String> {
    let target_shared_state_owner;
    let target_shared_state = if target.module.module_name == shared_state.module_name
        && target.module.source_hash == shared_state.source_hash
    {
        shared_state
    } else {
        let Some(compile_session) = compile_session else {
            return Ok(None);
        };
        let Some(target_shared_state) = compile_session.shared_module_state_for_identity(
            &target.module.module_name,
            target.module.source_hash,
        )?
        else {
            return Ok(None);
        };
        target_shared_state_owner = target_shared_state;
        target_shared_state_owner.as_ref()
    };
    let function_id = RuntimeFunctionId::new(
        RuntimeModuleId::new(target_shared_state.module_id()),
        target.local,
    );
    Ok(target_shared_state
        .lookup_function(function_id)
        .map(|function| function.function_id))
}

fn resolve_opt_v3_codegen_module_function_target(
    module_name: &str,
    source_hash: u64,
    module_id: RuntimeModuleId,
    module: &BlockPyModule<CodegenModuleShape>,
    module_index: Option<&PrecompileModuleIndex>,
    target: PersistentFunctionId,
) -> Result<Option<RuntimeFunctionId>, String> {
    if target.module.module_name != module_name || target.module.source_hash != source_hash {
        return Ok(
            module_index.and_then(|module_index| module_index.function_id_for_target(&target))
        );
    }
    let target_function_id = RuntimeFunctionId::new(module_id, target.local);
    Ok(module
        .callable_defs
        .iter()
        .any(|function| function.function_id == target_function_id)
        .then_some(target_function_id))
}

fn opt_v3_single_function_artifacts(
    artifacts: &ExactIntBranchV3Artifacts,
    function: SerializedFunctionId,
) -> Result<Option<ExactIntBranchV3Artifacts>, String> {
    single_function_optimization_artifacts_v3(artifacts, function).map_err(|err| err.to_string())
}

fn planned_optimization_inputs_for_precompile(
    plan_input: Option<PrecompileOptimizationPlanInput<'_>>,
    module_index: Option<&PrecompileModuleIndex>,
    module_name: &str,
    source_hash: u64,
    module: &BlockPyModule<CodegenModuleShape>,
) -> Result<PlannedOptimizationInputs, String> {
    let Some(plan_input) = plan_input else {
        return Ok(PlannedOptimizationInputs::default());
    };
    let Some(path) = plan_input.v3_path.filter(|path| path.exists()) else {
        return Err(format!(
            "precompile requires mod.optv3 for module {module_name}"
        ));
    };
    let artifacts = load_optimization_artifacts_v3(path).map_err(|err| err.to_string())?;
    validate_optimization_artifacts_v3_for_module(
        &artifacts,
        module_name,
        source_hash,
        plan_input.cache_identity,
    )
    .map_err(|err| err.to_string())?;
    planned_optimization_inputs_from_v3_artifacts_for_codegen_module(
        &artifacts,
        module,
        module_name,
        source_hash,
        module_index,
    )
}

impl<'a> SpecializationProfile<'a> {
    fn has_v3_optimization_inputs(&self) -> bool {
        !self.opt_v3_emitted_direct_calls.is_empty()
            || !self.opt_v3_emitted_exact_list_items.is_empty()
            || !self.opt_v3_emitted_indexed_fields.is_empty()
            || !self.opt_v3_emitted_indexed_globals.is_empty()
            || !self.opt_v3_exact_int_branch_artifacts.is_empty()
    }

    fn typed_specializations_embedded(&self) -> bool {
        self.direct_call_emission_scope == DirectCallEmissionScope::AllDirectCallCandidates
    }

    fn from_runtime_state_with_session(
        shared_state: Option<&'a SharedModuleState>,
        compile_session: Option<&crate::session::CompileSession>,
    ) -> Result<Self, String> {
        let env_config = env_config_for_session(compile_session)?;
        let specialization_mode = env_config.specialization_mode();
        let runtime_pipeline = env_config.runtime_optimization_pipeline();
        let typed_v3_runtime = runtime_pipeline.uses_typed_v3_runtime();
        let legacy_plan_artifacts_runtime = runtime_pipeline.uses_legacy_plan_artifacts_runtime();
        let counter_dump_path = if shared_state.is_some()
            && legacy_plan_artifacts_runtime
            && specialization_mode != Some(crate::config::SpecializationMode::Profile)
        {
            env_config.counter_dump_input_path()
        } else {
            None
        };
        let planned_inputs = load_planned_optimization_inputs_for_runtime_state(
            shared_state,
            compile_session,
            &env_config,
            specialization_mode,
        )?;
        Ok(Self {
            module_name: shared_state.map(|shared_state| shared_state.module_name.as_str()),
            counter_dump_path: counter_dump_path.map(Cow::Owned),
            direct_call_emission_scope: if typed_v3_runtime
                || planned_inputs.has_v3_optimization_inputs()
            {
                DirectCallEmissionScope::AllDirectCallCandidates
            } else {
                DirectCallEmissionScope::DirectCallBodiesOnly
            },
            opt_v3_emitted_direct_calls: planned_inputs.opt_v3_emitted_direct_calls,
            opt_v3_emitted_exact_list_items: planned_inputs.opt_v3_emitted_exact_list_items,
            opt_v3_emitted_indexed_fields: planned_inputs.opt_v3_emitted_indexed_fields,
            opt_v3_emitted_indexed_globals: planned_inputs.opt_v3_emitted_indexed_globals,
            opt_v3_exact_int_branch_artifacts: planned_inputs.opt_v3_exact_int_branch_artifacts,
            behavior_change_indexed_stores: !typed_v3_runtime
                && specialization_mode
                    .is_some_and(SpecializationMode::behavior_change_indexed_stores_enabled),
            profiled_cold_blocks: !typed_v3_runtime && env_config.profiled_cold_blocks_enabled(),
            guard_miss_deopt: !typed_v3_runtime
                && matches!(
                    specialization_mode,
                    Some(SpecializationMode::Verify | SpecializationMode::Apply)
                ),
        })
    }

    fn from_precompile(
        env_config: &SoacEnvConfig,
        module_name: &'a str,
        counter_dump_path: Option<&'a Path>,
        planned_inputs: PlannedOptimizationInputs,
    ) -> Result<Self, String> {
        Ok(Self {
            module_name: Some(module_name),
            counter_dump_path: counter_dump_path.map(Cow::Borrowed),
            direct_call_emission_scope: if planned_inputs.has_v3_optimization_inputs() {
                DirectCallEmissionScope::AllDirectCallCandidates
            } else {
                DirectCallEmissionScope::DirectCallBodiesOnly
            },
            opt_v3_emitted_direct_calls: planned_inputs.opt_v3_emitted_direct_calls,
            opt_v3_emitted_exact_list_items: planned_inputs.opt_v3_emitted_exact_list_items,
            opt_v3_emitted_indexed_fields: planned_inputs.opt_v3_emitted_indexed_fields,
            opt_v3_emitted_indexed_globals: planned_inputs.opt_v3_emitted_indexed_globals,
            opt_v3_exact_int_branch_artifacts: planned_inputs.opt_v3_exact_int_branch_artifacts,
            behavior_change_indexed_stores: true,
            profiled_cold_blocks: env_config.profiled_cold_blocks_enabled(),
            guard_miss_deopt: true,
        })
    }

    fn opt_v3_indexed_field_access_plans(&self) -> Vec<&OptV3IndexedFieldAccessPlan> {
        self.opt_v3_emitted_indexed_fields
            .values()
            .flat_map(|accesses_by_instr| accesses_by_instr.values())
            .flat_map(|accesses| accesses.iter())
            .collect()
    }

    fn codegen_opt_v3_direct_calls(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>> {
        self.opt_v3_emitted_direct_calls
            .get(&function_id)
            .map(opt_v3_direct_call_body_plans)
            .unwrap_or_default()
    }

    fn typed_call_emission_direct_calls(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>> {
        match self.direct_call_emission_scope {
            DirectCallEmissionScope::DirectCallBodiesOnly => {
                self.codegen_opt_v3_direct_calls(function_id)
            }
            DirectCallEmissionScope::AllDirectCallCandidates => self
                .opt_v3_emitted_direct_calls
                .get(&function_id)
                .cloned()
                .unwrap_or_default(),
        }
    }

    fn typed_inline_direct_calls(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>> {
        self.typed_inline_resolved_direct_calls(function_id)
            .into_iter()
            .map(|(source, plans)| {
                (
                    source,
                    plans
                        .into_iter()
                        .map(|plan| (plan.target, plan.arg_plan))
                        .collect(),
                )
            })
            .collect()
    }

    fn typed_inline_resolved_direct_calls(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>> {
        if self.direct_call_emission_scope != DirectCallEmissionScope::AllDirectCallCandidates {
            return HashMap::new();
        }
        self.opt_v3_emitted_direct_calls
            .get(&function_id)
            .map(|direct_calls| {
                direct_calls
                    .iter()
                    .filter_map(|(source, plans)| {
                        let inline_plans = plans
                            .iter()
                            .filter(|plan| plan.body.kind == CallBodyKind::Inline)
                            .cloned()
                            .collect::<Vec<_>>();
                        (!inline_plans.is_empty()).then_some((*source, inline_plans))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn field_index_specialization_maps(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Result<
        (
            HashMap<String, Vec<FieldIndexSpecialization>>,
            HashMap<InstrId, Vec<FieldIndexSpecialization>>,
            HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
        ),
        String,
    > {
        let opt_v3_planned_fields = self.opt_v3_indexed_field_access_plans();
        let opt_v3_layout_groups =
            opt_v3_indexed_field_layout_groups(opt_v3_planned_fields.iter().copied());
        prime_opt_v3_field_index_layouts(opt_v3_layout_groups.iter())?;
        let opt_v3_by_instr = opt_v3_prepare_indexed_field_accesses_for_codegen(
            self.opt_v3_emitted_indexed_fields.get(&function_id),
            |request| field_index_specialization_from_opt_v3_for_function(function_id, request),
        )?;
        Ok((HashMap::new(), HashMap::new(), opt_v3_by_instr))
    }

    fn cold_block_labels(
        &self,
        function: &BlockPyFunction<impl ModuleShape>,
    ) -> Result<HashSet<BlockLabel>, String> {
        if !self.profiled_cold_blocks {
            return Ok(HashSet::new());
        }
        let Some(module_name) = self.module_name else {
            return Ok(HashSet::new());
        };
        let Some(path) = existing_counter_dump_path(self.counter_dump_path.as_deref()) else {
            return Ok(HashSet::new());
        };
        collect_cold_block_labels_from_path(path, function, module_name)
    }
}

fn existing_counter_dump_path(path: Option<&Path>) -> Option<&Path> {
    path.filter(|path| path.exists())
}

fn resolve_type_key_to_type(
    type_key: &CounterDumpTypeKey,
) -> Result<Option<*mut ffi::PyTypeObject>, String> {
    if type_key.module_name.is_empty()
        || type_key.qualname.is_empty()
        || type_key.qualname.split('.').any(|part| part == "<locals>")
    {
        return Ok(None);
    }
    if unsafe { PyThreadState_GetUnchecked() }.is_null() {
        return Ok(None);
    }

    let module_name = CString::new(type_key.module_name.as_str())
        .map_err(|_| format!("type key module contains NUL: {:?}", type_key.module_name))?;
    let modules = unsafe { ffi::PyImport_GetModuleDict() };
    if modules.is_null() {
        if unsafe { !ffi::PyErr_Occurred().is_null() } {
            return Err("failed to read sys.modules while resolving type key".to_string());
        }
        return Ok(None);
    }
    let mut current = unsafe { ffi::PyDict_GetItemString(modules, module_name.as_ptr()) };
    if current.is_null() {
        return Ok(None);
    }
    unsafe { ffi::Py_INCREF(current) };

    for part in type_key.qualname.split('.') {
        if part.is_empty() {
            unsafe { ffi::Py_DECREF(current) };
            return Ok(None);
        }
        let part = CString::new(part)
            .map_err(|_| format!("type key qualname contains NUL: {:?}", type_key.qualname))?;
        let next = unsafe { ffi::PyObject_GetAttrString(current, part.as_ptr()) };
        unsafe { ffi::Py_DECREF(current) };
        if next.is_null() {
            unsafe { ffi::PyErr_Clear() };
            return Ok(None);
        }
        current = next;
    }

    if unsafe { ffi::PyType_Check(current) } == 0 {
        unsafe { ffi::Py_DECREF(current) };
        return Ok(None);
    }
    let owner_type = current as *mut ffi::PyTypeObject;
    unsafe { ffi::Py_DECREF(current) };
    Ok(Some(owner_type))
}

fn cpython_type_symbol_for_type(owner_type: *mut ffi::PyTypeObject) -> Option<CpythonTypeSymbol> {
    match owner_type {
        ptr if ptr == std::ptr::addr_of_mut!(PyFunction_Type) => Some(CpythonTypeSymbol::Function),
        ptr if ptr == std::ptr::addr_of_mut!(PyMethod_Type) => Some(CpythonTypeSymbol::Method),
        ptr if ptr == std::ptr::addr_of_mut!(PyType_Type) => Some(CpythonTypeSymbol::Type),
        ptr if ptr == std::ptr::addr_of_mut!(PyLong_Type) => Some(CpythonTypeSymbol::Long),
        ptr if ptr == std::ptr::addr_of_mut!(PyList_Type) => Some(CpythonTypeSymbol::List),
        _ => None,
    }
}

fn resolve_cpython_type_symbol(symbol: CpythonTypeSymbol) -> *mut ffi::PyTypeObject {
    match symbol {
        CpythonTypeSymbol::Function => std::ptr::addr_of_mut!(PyFunction_Type),
        CpythonTypeSymbol::Method => std::ptr::addr_of_mut!(PyMethod_Type),
        CpythonTypeSymbol::Type => std::ptr::addr_of_mut!(PyType_Type),
        CpythonTypeSymbol::Long => std::ptr::addr_of_mut!(PyLong_Type),
        CpythonTypeSymbol::List => std::ptr::addr_of_mut!(PyList_Type),
    }
}

fn py_string_attr_owned(
    obj: *mut ffi::PyObject,
    attr_name: &CStr,
) -> Result<Option<String>, String> {
    let attr = unsafe { ffi::PyObject_GetAttrString(obj, attr_name.as_ptr()) };
    if attr.is_null() {
        unsafe { ffi::PyErr_Clear() };
        return Ok(None);
    }
    if unsafe { ffi::PyUnicode_Check(attr) } == 0 {
        unsafe { ffi::Py_DECREF(attr) };
        return Ok(None);
    }
    let mut size = 0isize;
    let data = unsafe { ffi::PyUnicode_AsUTF8AndSize(attr, &mut size) };
    if data.is_null() {
        unsafe { ffi::Py_DECREF(attr) };
        return Err(format!(
            "failed to read Python string attribute {} as UTF-8",
            attr_name.to_string_lossy()
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) };
    let value = match std::str::from_utf8(bytes) {
        Ok(value) => value.to_owned(),
        Err(err) => {
            unsafe { ffi::Py_DECREF(attr) };
            return Err(format!(
                "Python string attribute {} was not valid UTF-8: {err}",
                attr_name.to_string_lossy()
            ));
        }
    };
    unsafe { ffi::Py_DECREF(attr) };
    Ok(Some(value))
}

fn type_key_for_type(
    owner_type: *mut ffi::PyTypeObject,
) -> Result<Option<CounterDumpTypeKey>, String> {
    if owner_type.is_null() {
        return Ok(None);
    }
    let owner_obj = owner_type.cast::<ffi::PyObject>();
    let Some(module_name) = py_string_attr_owned(owner_obj, c"__module__")? else {
        return Ok(None);
    };
    let Some(qualname) = py_string_attr_owned(owner_obj, c"__qualname__")? else {
        return Ok(None);
    };
    if module_name.is_empty()
        || qualname.is_empty()
        || qualname.split('.').any(|part| part == "<locals>")
    {
        return Ok(None);
    }
    Ok(Some(CounterDumpTypeKey {
        module_name,
        qualname,
    }))
}

fn register_runtime_type_for_key(
    type_key: &CounterDumpTypeKey,
    owner_type: *mut ffi::PyTypeObject,
) {
    let mut registry = type_key_runtime_registry()
        .lock()
        .expect("type key runtime registry lock poisoned");
    registry.insert(type_key.clone(), owner_type as usize);
}

fn lookup_runtime_type_for_key(type_key: &CounterDumpTypeKey) -> Option<*mut ffi::PyTypeObject> {
    let registry = type_key_runtime_registry()
        .lock()
        .expect("type key runtime registry lock poisoned");
    registry
        .get(type_key)
        .copied()
        .map(|ptr| ptr as *mut ffi::PyTypeObject)
}

fn reloc_type_ref_for_type(
    owner_type: *mut ffi::PyTypeObject,
) -> Result<Option<RelocTypeRef>, String> {
    if let Some(symbol) = cpython_type_symbol_for_type(owner_type) {
        return Ok(Some(RelocTypeRef::CpythonTypeSymbol(symbol)));
    }
    let Some(type_key) = type_key_for_type(owner_type)? else {
        return Ok(None);
    };
    register_runtime_type_for_key(&type_key, owner_type);
    Ok(Some(RelocTypeRef::TypeKey(type_key)))
}

fn resolve_reloc_type_ref_to_type(
    owner_type_ref: &RelocTypeRef,
) -> Result<Option<*mut ffi::PyTypeObject>, String> {
    match owner_type_ref {
        RelocTypeRef::CpythonTypeSymbol(symbol) => Ok(Some(resolve_cpython_type_symbol(*symbol))),
        RelocTypeRef::TypeKey(type_key) => {
            if let Some(owner_type) = lookup_runtime_type_for_key(type_key) {
                return Ok(Some(owner_type));
            }
            resolve_type_key_to_type(type_key)
        }
    }
}

fn ensure_reloc_type_symbol_registered(owner_type_ref: &RelocTypeRef) -> Result<bool, String> {
    match owner_type_ref {
        RelocTypeRef::CpythonTypeSymbol(_) => Ok(true),
        RelocTypeRef::TypeKey(_) => {
            let symbol = reloc_type_ref_symbol_name(owner_type_ref);
            if lookup_registered_jit_data_symbol(symbol.as_ref()).is_some() {
                return Ok(true);
            }
            let Some(owner_type) = resolve_reloc_type_ref_to_type(owner_type_ref)? else {
                return Ok(false);
            };
            register_jit_data_symbol(symbol.as_ref(), owner_type.cast::<u8>());
            Ok(true)
        }
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

fn resolve_reloc_callable_ref_to_object(
    callable_ref: &RelocCallableRef,
) -> Result<Option<ObjPtr>, String> {
    match callable_ref {
        RelocCallableRef::OwnerAttr {
            owner_type_ref,
            attr_name,
        } => {
            let Some(owner_type) = resolve_reloc_type_ref_to_type(owner_type_ref)? else {
                return Ok(None);
            };
            let attr_name = CString::new(attr_name.as_str()).map_err(|_| {
                format!("callable attr contains NUL and cannot be resolved: {attr_name:?}")
            })?;
            let dict = unsafe { (*owner_type).tp_dict };
            if dict.is_null() {
                return Ok(None);
            }
            let value = unsafe { ffi::PyDict_GetItemString(dict, attr_name.as_ptr()) };
            if value.is_null() || unsafe { ffi::PyFunction_Check(value) } == 0 {
                return Ok(None);
            }
            Ok(Some(value as ObjPtr))
        }
    }
}

fn ensure_reloc_callable_symbol_registered(
    callable_ref: &RelocCallableRef,
) -> Result<bool, String> {
    let symbol = reloc_callable_ref_symbol_name(callable_ref);
    if lookup_registered_jit_data_symbol(symbol.as_str()).is_some() {
        return Ok(true);
    }
    let Some(callable) = resolve_reloc_callable_ref_to_object(callable_ref)? else {
        return Ok(false);
    };
    register_jit_data_symbol(symbol.as_str(), callable.cast::<u8>());
    Ok(true)
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

fn owner_type_has_class_binding_for_attr(
    owner_type: *mut ffi::PyTypeObject,
    attr_name: &str,
) -> Result<bool, String> {
    let attr_name = CString::new(attr_name)
        .map_err(|_| format!("field specialization attr contains NUL: {attr_name:?}"))?;
    let attr_obj = unsafe { ffi::PyUnicode_FromString(attr_name.as_ptr()) };
    if attr_obj.is_null() {
        return Err("failed to allocate field specialization attr name".to_string());
    }
    let descriptor = unsafe { _PyType_LookupRef(owner_type, attr_obj) };
    unsafe { ffi::Py_DECREF(attr_obj) };
    if descriptor.is_null() {
        if unsafe { !ffi::PyErr_Occurred().is_null() } {
            return Err("failed while checking owner type class binding".to_string());
        }
        Ok(false)
    } else {
        unsafe { ffi::Py_DECREF(descriptor) };
        Ok(true)
    }
}

unsafe fn owner_type_supports_field_layout_priming(owner_type: *mut ffi::PyTypeObject) -> bool {
    const PY_TPFLAGS_MANAGED_DICT_SOAC: u64 = 1 << 4;
    const PY_TPFLAGS_INLINE_VALUES_SOAC: u64 = 1 << 2;

    if owner_type.is_null() {
        return false;
    }
    if ((*owner_type).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE) == 0
        || ((*owner_type).tp_flags & PY_TPFLAGS_INLINE_VALUES_SOAC) == 0
        || ((*owner_type).tp_flags & PY_TPFLAGS_MANAGED_DICT_SOAC) == 0
    {
        return false;
    }
    if ffi::Py_TYPE(owner_type as *mut ffi::PyObject) != std::ptr::addr_of_mut!(ffi::PyType_Type) {
        return false;
    }
    let Some(owner_tp_alloc) = (*owner_type).tp_alloc else {
        return false;
    };
    let generic_alloc: unsafe extern "C" fn(
        *mut ffi::PyTypeObject,
        ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject = ffi::PyType_GenericAlloc;
    std::ptr::fn_addr_eq(owner_tp_alloc, generic_alloc)
}

unsafe fn owner_type_has_safe_zero_arg_priming_constructor(
    owner_type: *mut ffi::PyTypeObject,
) -> bool {
    if !owner_type_supports_field_layout_priming(owner_type)
        || ((*owner_type).tp_flags & ffi::Py_TPFLAGS_IS_ABSTRACT) != 0
    {
        return false;
    }
    let class_dict = (*owner_type).tp_dict;
    if class_dict.is_null() {
        return false;
    }
    unsafe { ffi::PyDict_GetItemString(class_dict, c"__init__".as_ptr()) }.is_null()
        && unsafe { ffi::PyDict_GetItemString(class_dict, c"__new__".as_ptr()) }.is_null()
}

fn prime_field_index_layout(
    owner_type: *mut ffi::PyTypeObject,
    layouts: &[CollectedTypeKeyLayout],
) -> Result<(), String> {
    if layouts.is_empty() || !unsafe { owner_type_supports_field_layout_priming(owner_type) } {
        return Ok(());
    }
    let Some(owner_tp_alloc) = (unsafe { (*owner_type).tp_alloc }) else {
        return Ok(());
    };
    let mut temp_instance =
        if unsafe { owner_type_has_safe_zero_arg_priming_constructor(owner_type) } {
            unsafe { ffi::PyObject_CallNoArgs(owner_type.cast()) }
        } else {
            std::ptr::null_mut()
        };
    if temp_instance.is_null() {
        unsafe { ffi::PyErr_Clear() };
        temp_instance = unsafe { owner_tp_alloc(owner_type, 0) };
    }
    if temp_instance.is_null() {
        unsafe { ffi::PyErr_Clear() };
        return Ok(());
    }
    let none = unsafe { ffi::Py_None() };
    for layout in layouts {
        let key_name = CString::new(layout.key.as_str())
            .map_err(|_| format!("field specialization attr contains NUL: {:?}", layout.key))?;
        let key = unsafe { ffi::PyUnicode_InternFromString(key_name.as_ptr()) };
        if key.is_null() {
            unsafe {
                ffi::Py_DECREF(temp_instance);
                ffi::PyErr_Clear();
            }
            return Ok(());
        }
        let set_result = unsafe { ffi::PyObject_SetAttr(temp_instance, key, none) };
        unsafe { ffi::Py_DECREF(key) };
        if set_result != 0 {
            unsafe {
                ffi::Py_DECREF(temp_instance);
                ffi::PyErr_Clear();
            }
            return Ok(());
        }
    }
    unsafe { ffi::Py_DECREF(temp_instance) };
    Ok(())
}

fn field_index_specialization_for_type(
    owner_type: *mut ffi::PyTypeObject,
    attr_name: &str,
    expected_index: u32,
) -> Result<Option<FieldIndexSpecialization>, String> {
    if owner_type.is_null() {
        return Ok(None);
    }
    if unsafe { ((*owner_type).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE) == 0 } {
        return Ok(None);
    }
    let has_generic_getattr = unsafe { (*owner_type).tp_getattro }.is_some_and(|getattr| {
        std::ptr::fn_addr_eq(
            getattr,
            ffi::PyObject_GenericGetAttr
                as unsafe extern "C" fn(
                    *mut ffi::PyObject,
                    *mut ffi::PyObject,
                ) -> *mut ffi::PyObject,
        )
    });
    let has_generic_setattr = unsafe { (*owner_type).tp_setattro }.is_some_and(|setattr| {
        std::ptr::fn_addr_eq(
            setattr,
            ffi::PyObject_GenericSetAttr
                as unsafe extern "C" fn(
                    *mut ffi::PyObject,
                    *mut ffi::PyObject,
                    *mut ffi::PyObject,
                ) -> i32,
        )
    });
    if !has_generic_getattr
        || !has_generic_setattr
        || owner_type_has_class_binding_for_attr(owner_type, attr_name)?
    {
        return Ok(None);
    }

    if unsafe { (*owner_type).tp_version_tag } == 0 {
        let _ = unsafe { PyUnstable_Type_AssignVersionTag(owner_type) };
    }
    let type_version = unsafe { (*owner_type).tp_version_tag };
    if type_version == 0 {
        return Ok(None);
    }
    let Some(owner_type_ref) = reloc_type_ref_for_type(owner_type)? else {
        return Ok(None);
    };

    Ok(Some(FieldIndexSpecialization {
        expected_index,
        owner_type_ref,
        type_version,
    }))
}

fn prime_opt_v3_field_index_layouts<'a>(
    layout_groups: impl IntoIterator<Item = &'a OptV3IndexedFieldLayoutGroup>,
) -> Result<(), String> {
    for group in layout_groups {
        let Some(owner_type) = resolve_type_key_to_type(&group.type_key)? else {
            continue;
        };
        prime_field_index_layout(owner_type, group.layouts.as_slice())?;
    }
    Ok(())
}

fn field_index_specialization_from_primed_opt_v3(
    request: &OptV3IndexedFieldRuntimeAccessRequest,
) -> Result<Option<FieldIndexSpecialization>, String> {
    let Some(owner_type) = resolve_type_key_to_type(&request.type_key)? else {
        return Ok(None);
    };
    field_index_specialization_for_type(
        owner_type,
        request.attr_name.as_str(),
        request.expected_index,
    )
}

fn constructor_owner_type_for_type_key(
    function_id: RuntimeFunctionId,
    type_key: &CounterDumpTypeKey,
) -> Result<Option<*mut ffi::PyTypeObject>, String> {
    let owner_types = unsafe { crate::lookup_exact_owner_types_for_constructor(function_id) }
        .map_err(|_| format!("failed to resolve owner types for constructor {function_id}"))?;
    for owner in owner_types {
        if type_key_for_type(owner.owner_type)?.as_ref() == Some(type_key) {
            register_runtime_type_for_key(type_key, owner.owner_type);
            return Ok(Some(owner.owner_type));
        }
    }
    Ok(None)
}

fn indexed_field_owner_type_for_function(
    function_id: RuntimeFunctionId,
    type_key: &CounterDumpTypeKey,
) -> Result<Option<*mut ffi::PyTypeObject>, String> {
    if let Some(owner_type) = resolve_type_key_to_type(type_key)? {
        return Ok(Some(owner_type));
    }
    constructor_owner_type_for_type_key(function_id, type_key)
}

fn field_index_specialization_from_opt_v3_for_function(
    function_id: RuntimeFunctionId,
    request: &OptV3IndexedFieldRuntimeAccessRequest,
) -> Result<Option<FieldIndexSpecialization>, String> {
    let type_key = &request.type_key;
    let Some(owner_type) = indexed_field_owner_type_for_function(function_id, type_key)? else {
        return Ok(None);
    };
    prime_field_index_layout(
        owner_type,
        &[CollectedTypeKeyLayout {
            owner_type_id: 0,
            key: request.attr_name.clone(),
            index: request.expected_index,
        }],
    )?;
    field_index_specialization_for_type(
        owner_type,
        request.attr_name.as_str(),
        request.expected_index,
    )
}

fn push_unique_specialization(
    specializations: &mut Vec<FieldIndexSpecialization>,
    specialization: FieldIndexSpecialization,
) {
    if !specializations.contains(&specialization) {
        specializations.push(specialization);
    }
}

fn collect_cold_block_labels_from_path(
    path: &Path,
    function: &BlockPyFunction<impl ModuleShape>,
    module_name: &str,
) -> Result<HashSet<BlockLabel>, String> {
    let block_entry_counts =
        read_block_entry_counts_from_file(path, module_name, function.function_id)?;
    let entry_label = function.entry_block().label;
    let Some(entry_count) = block_entry_counts.get(&entry_label).copied() else {
        return Ok(HashSet::new());
    };
    if entry_count == 0 {
        return Ok(HashSet::new());
    }

    Ok(function
        .blocks
        .iter()
        .filter_map(|block| {
            if block.label == entry_label {
                return None;
            }
            let block_count = block_entry_counts.get(&block.label).copied()?;
            (block_count.saturating_mul(COLD_BLOCK_ENTRY_RATE_DENOMINATOR) <= entry_count)
                .then_some(block.label)
        })
        .collect())
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
    struct PyFunctionObjectSoacPrefix {
        ob_refcnt: isize,
        ob_type: *mut ffi::PyTypeObject,
        func_globals: *mut ffi::PyObject,
        func_builtins: *mut ffi::PyObject,
        func_name: *mut ffi::PyObject,
        func_qualname: *mut ffi::PyObject,
        func_code: *mut ffi::PyObject,
        func_defaults: *mut ffi::PyObject,
        func_kwdefaults: *mut ffi::PyObject,
        func_closure: *mut ffi::PyObject,
        func_doc: *mut ffi::PyObject,
        func_dict: *mut ffi::PyObject,
        func_weakreflist: *mut ffi::PyObject,
        func_module: *mut ffi::PyObject,
        func_annotations: *mut ffi::PyObject,
        func_annotate: *mut ffi::PyObject,
        func_typeparams: *mut ffi::PyObject,
        vectorcall: ffi::vectorcallfunc,
        func_soac_metadata: *mut std::ffi::c_void,
        func_soac_metadata_destructor: *mut std::ffi::c_void,
        func_soac_function_id: u64,
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
    const PYFUNCTION_SOAC_FUNCTION_ID_OFFSET: i32 =
        offset_of!(PyFunctionObjectSoacPrefix, func_soac_function_id) as i32;
    const PYTYPE_TP_FLAGS_OFFSET: i32 = offset_of!(ffi::PyTypeObject, tp_flags) as i32;
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
        PYFUNCTION_SOAC_FUNCTION_ID_OFFSET,
    );
    let id_is_zero = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, packed, 0);
    let id_done_block = fb.create_block();
    fb.ins()
        .brif(id_is_zero, miss_block, &[], id_done_block, &[]);

    fb.switch_to_block(id_done_block);
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
        emit_resolved_direct_function_metadata_and_env(fb, callable, ctx);

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

fn emit_direct_constructor_resolved_with_arg_values(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    arg_values: Vec<ir::Value>,
    arg_borrowed: Vec<bool>,
    specialization: &DirectConstructorSpecialization,
    target_function: &BlockPyFunction<impl ModuleShape>,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let zero = fb.ins().iconst(ctx.consts.i64_ty, 0);
    let alloc_inst = fb
        .ins()
        .call(ctx.pytype_generic_alloc_ref, &[callable, zero]);
    let allocated = fb.inst_results(alloc_inst)[0];
    if !callable_is_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, callable]);
    }
    let alloc_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, allocated, null_ptr);
    let alloc_failed = fb.create_block();
    let alloc_ok = fb.create_block();
    fb.append_block_param(alloc_ok, ptr_ty);
    fb.ins().brif(
        alloc_is_null,
        alloc_failed,
        &[],
        alloc_ok,
        &[ir::BlockArg::Value(allocated)],
    );

    fb.switch_to_block(alloc_failed);
    let mut owned_inputs = Vec::with_capacity(arg_values.len());
    for (value, borrowed_arg) in arg_values.iter().copied().zip(arg_borrowed.iter().copied()) {
        if !borrowed_arg {
            owned_inputs.push(value);
        }
    }
    emit_release_owned_inputs(fb, ctx, &owned_inputs);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(alloc_ok);
    let allocated = fb.block_params(alloc_ok)[0];
    let mut provided_arg_values = Vec::with_capacity(arg_values.len() + 1);
    let mut provided_arg_borrowed = Vec::with_capacity(arg_borrowed.len() + 1);
    provided_arg_values.push(allocated);
    provided_arg_borrowed.push(true);
    provided_arg_values.extend(arg_values);
    provided_arg_borrowed.extend(arg_borrowed);
    let (init_arg_values, init_arg_borrowed) = emit_direct_call_args_from_plan(
        fb,
        &specialization.arg_plan,
        provided_arg_values,
        provided_arg_borrowed,
        ptr_ty,
    );
    let init_callable =
        emit_callable_ptr_value_for_ref(fb, codegen_env, ctx, &specialization.init_function_ref)
            .unwrap_or_else(|err| panic!("failed to bind constructor callable symbol: {err}"))
            .expect("constructor callable symbol should be available");
    let init_result = emit_direct_call_resolved_raw_with_arg_values(
        fb,
        init_callable,
        true,
        init_arg_values,
        init_arg_borrowed,
        if specialization.arg_plan.requires_default_resolving_entry() {
            DirectCallEntryKind::DefaultResolving
        } else {
            DirectCallEntryKind::Core
        },
        target_function,
        ctx,
        codegen_env,
    );
    let init_failed = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, init_result, null_ptr);
    let init_fail_block = fb.create_block();
    let init_ok_block = fb.create_block();
    fb.append_block_param(init_ok_block, ptr_ty);
    fb.ins().brif(
        init_failed,
        init_fail_block,
        &[],
        init_ok_block,
        &[ir::BlockArg::Value(init_result)],
    );

    fb.switch_to_block(init_fail_block);
    emit_release_owned_inputs(fb, ctx, &[allocated]);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(init_ok_block);
    let init_result = fb.block_params(init_ok_block)[0];
    let finish_inst = fb
        .ins()
        .call(ctx.finish_constructor_init_ref, &[allocated, init_result]);
    let result = fb.inst_results(finish_inst)[0];
    let result_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, result, null_ptr);
    let result_ok_block = fb.create_block();
    fb.append_block_param(result_ok_block, ptr_ty);
    fb.ins().brif(
        result_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        result_ok_block,
        &[ir::BlockArg::Value(result)],
    );
    fb.switch_to_block(result_ok_block);
    fb.block_params(result_ok_block)[0]
}

fn emit_direct_call_args_from_plan(
    fb: &mut FunctionBuilder<'_>,
    arg_plan: &DirectCallArgPlan,
    provided_arg_values: Vec<ir::Value>,
    provided_arg_borrowed: Vec<bool>,
    ptr_ty: ir::Type,
) -> (Vec<ir::Value>, Vec<bool>) {
    debug_assert_eq!(provided_arg_values.len(), provided_arg_borrowed.len());
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
    args: &[&InstrCodegen],
    arg_plan: &DirectCallArgPlan,
    target_function: &BlockPyFunction<impl ModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let mut provided_arg_values: Vec<ir::Value> = Vec::with_capacity(args.len());
    let mut provided_arg_borrowed: Vec<bool> = Vec::with_capacity(args.len());
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
        ptr_ty,
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
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        expr,
        local_env,
        ctx,
        borrowed,
        codegen_env,
        func_imports,
    )?;
    let (value, ownership, _) = value.expect_pyobject(site);
    Ok((value, borrowed || !ownership.is_owned()))
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
    let ptr_ty = ctx.consts.ptr_ty;
    let mut provided_arg_values: Vec<ir::Value> = Vec::with_capacity(args.len());
    let mut provided_arg_borrowed: Vec<bool> = Vec::with_capacity(args.len());
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
        ptr_ty,
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

fn emit_direct_constructor_resolved_with_args_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrCodegen],
    specialization: &DirectConstructorSpecialization,
    target_function: &BlockPyFunction<impl ModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let mut arg_values = Vec::with_capacity(args.len());
    let mut arg_borrowed = Vec::with_capacity(args.len());
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
    emit_direct_constructor_resolved_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        specialization,
        target_function,
        ctx,
        codegen_env,
    )
}

fn emit_typed_direct_constructor_resolved_with_args_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrTyped],
    specialization: &DirectConstructorSpecialization,
    target_function: &BlockPyFunction<impl ModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let mut arg_values = Vec::with_capacity(args.len());
    let mut arg_borrowed = Vec::with_capacity(args.len());
    for arg in args {
        let (value, borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            arg,
            local_env,
            ctx,
            codegen_env,
            func_imports,
            "typed direct-constructor arg",
        )?;
        arg_values.push(value);
        arg_borrowed.push(borrowed);
    }
    Ok(emit_direct_constructor_resolved_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        specialization,
        target_function,
        ctx,
        codegen_env,
    ))
}

fn emit_direct_method_resolved_with_args_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    receiver_is_borrowed: bool,
    args: &[&InstrCodegen],
    specialization: &DirectMethodSpecialization,
    target_function: &BlockPyFunction<impl ModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
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
        ptr_ty,
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
    let ptr_ty = ctx.consts.ptr_ty;
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
        ptr_ty,
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
    target_args: &[(String, BlockArg)],
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
    _codegen_env: &mut impl JitCodegenEnv,
    _func_imports: &mut FuncBuildImports<'_>,
) -> Result<(Vec<ir::BlockArg>, HashSet<LocalLocation>), LocalEnvEdgePrepError> {
    let mut args = Vec::with_capacity(target_args.len());
    let mut forwarded_locations = HashSet::new();
    let mut forwarded_local_counts = HashMap::new();
    for (_, explicit_arg) in target_args {
        let value = match explicit_arg {
            BlockArg::Name(source_name) => {
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
            BlockArg::None => {
                let none_const = emit_none_const(fb, ctx);
                fb.ins().call(ctx.incref_ref, &[none_const]);
                none_const
            }
            BlockArg::CurrentException => {
                return Err(LocalEnvEdgePrepError::UnsupportedCurrentExceptionArg);
            }
            BlockArg::AbruptKind(kind) => emit_owned_module_constant(
                fb,
                ctx.module_constants
                    .require_int_constant_id(abrupt_kind_tag(*kind)),
                ctx,
            ),
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
        if preserved_values.contains(&entry.value) {
            continue;
        }
        if transient_local_needs_decref(entry.ref_kind) {
            fb.ins()
                .call(decref_ref, &[thread_state_value, entry.value]);
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
            let value = entry.value;
            let forwarded_count = forwarded_local_counts.entry(value_index).or_insert(0usize);
            if local_ref_kind_needs_incref_for_forward(entry.ref_kind, *forwarded_count) {
                emit_incref_if_not_null(fb, ptr_ty, incref_ref, value);
            }
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
            fb.ins().call(incref_ref, &[none_const]);
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
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    none_const: ir::Value,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
) -> Result<(), String> {
    let forwarded_locals_by_name = forwarded_local_names
        .iter()
        .zip(forwarded_local_values.iter().copied())
        .map(|(name, value)| (name.as_str(), value))
        .collect::<HashMap<_, _>>();
    for (target_name, source) in slot_writes {
        let value = match source {
            BlockArg::Name(source_name) => forwarded_locals_by_name
                .get(source_name.as_str())
                .copied()
                .ok_or_else(|| {
                    format!(
                        "missing forwarded exception dispatch slot source {source_name} for target {target_name}"
                    )
                })?,
            BlockArg::CurrentException => dispatch_exc,
            BlockArg::None => none_const,
            BlockArg::AbruptKind(_) => {
                unreachable!("validated exception edges should not use abrupt-kind args")
            }
        };
        stack_slots
            .replace_cloned_value(
                fb,
                target_name,
                value,
                ptr_ty,
                thread_state_value,
                incref_ref,
                decref_ref,
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
    target_args: &[(String, BlockArg)],
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
    for (target_name, source) in target_args {
        let value = match source {
            BlockArg::Name(source_name) => {
                let value = forwarded_locals_by_name
                    .get(source_name.as_str())
                    .copied()
                    .ok_or_else(|| {
                        format!(
                            "missing forwarded exception dispatch block-param source {source_name} for target {target_name}"
                        )
                    })?;
                let forwarded_count = forwarded_local_counts
                    .entry(source_name.as_str())
                    .or_insert(0usize);
                if *forwarded_count > 0 {
                    emit_incref_if_not_null(fb, ptr_ty, incref_ref, value);
                }
                *forwarded_count += 1;
                value
            }
            BlockArg::CurrentException => {
                if dispatch_exc_forward_count > 0 {
                    fb.ins().call(incref_ref, &[dispatch_exc]);
                }
                dispatch_exc_forward_count += 1;
                dispatch_exc
            }
            BlockArg::None => {
                fb.ins().call(incref_ref, &[none_const]);
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
    let _ = ctx.stack_slots.clear_value(
        fb,
        exception_name,
        ctx.consts.ptr_ty,
        ctx.consts.thread_state_value,
        ctx.decref_ref,
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
                        .contains(&local_env.entries[index].value)
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
        if let Some(previous) = removed.as_ref()
            && transient_local_needs_decref(previous.ref_kind)
        {
            emit_decref_if_not_null(
                fb,
                emit_ctx.consts.ptr_ty,
                emit_ctx.decref_ref,
                emit_ctx.consts.thread_state_value,
                previous.value,
            );
        }
        if removed
            .as_ref()
            .is_some_and(|entry| entry.storage == LocalEnvStorage::StackMirror)
            || removed.is_none()
        {
            emit_ctx
                .stack_slots
                .clear_value(
                    fb,
                    local.name.as_str(),
                    emit_ctx.consts.ptr_ty,
                    emit_ctx.consts.thread_state_value,
                    emit_ctx.decref_ref,
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
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    decref_ref: ir::FuncRef,
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
        stack_slots
            .clear_value(fb, local.name.as_str(), ptr_ty, thread_state_value, decref_ref)
            .ok_or_else(|| {
                format!(
                    "refcount plan release for block {source_label} references missing stack slot {:?}",
                    local.name
                )
            })?;
    }
    Ok(())
}

fn emit_truthy_from_owned_value(
    fb: &mut FunctionBuilder<'_>,
    owned_value: SoacValue,
    is_true_ref: ir::FuncRef,
    ctx: &JitEmitCtx<'_>,
) -> SoacValue {
    match owned_value {
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
    expr: &InstrCodegen,
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

fn emit_typed_indexed_getattr(
    fb: &mut FunctionBuilder<'_>,
    op: &TypedGetAttr<InstrTyped>,
    source: TypedIndexedFieldPlanSource,
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
    let i64_ty = emit_ctx.consts.i64_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let hit_counter_id = emit_ctx
        .field_indexed_hit_counter_ids
        .get(&instr_id)
        .copied();
    let fallback_counter_id = emit_ctx
        .field_indexed_fallback_counter_ids
        .get(&instr_id)
        .copied();

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
        let expected_index = fb
            .ins()
            .iconst(i64_ty, i64::from(specialization.expected_index));
        let type_matches =
            emit_exact_type_version_match(fb, value, owner_type, specialization.type_version);
        fb.ins()
            .brif(type_matches, maybe_direct_block, &[], next_guard_block, &[]);

        fb.switch_to_block(maybe_direct_block);
        let direct_inst = fb.ins().call(
            emit_ctx.probe_field_indexed_ref,
            &[value, attr, expected_index],
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
        fb.ins().call(emit_ctx.incref_ref, &[direct_value]);
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
    Ok(Some(SoacValue::pyobject(result, PyObjFacts::unknown())))
}

fn emit_typed_indexed_setattr(
    fb: &mut FunctionBuilder<'_>,
    op: &TypedSetAttr<InstrTyped>,
    source: TypedIndexedFieldPlanSource,
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
    let mut owned_inputs = Vec::with_capacity(3);
    push_owned_typed_input_cleanup(&mut owned_inputs, value, value_is_borrowed);
    push_owned_typed_input_cleanup(&mut owned_inputs, attr, attr_is_borrowed);
    push_owned_typed_input_cleanup(&mut owned_inputs, replacement, replacement_is_borrowed);
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let i64_ty = emit_ctx.consts.i64_ty;
    let zero_i32 = fb.ins().iconst(emit_ctx.consts.i32_ty, 0);
    let hit_counter_id = emit_ctx
        .field_indexed_hit_counter_ids
        .get(&instr_id)
        .copied();
    let fallback_counter_id = emit_ctx
        .field_indexed_fallback_counter_ids
        .get(&instr_id)
        .copied();

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
        let expected_index = fb
            .ins()
            .iconst(i64_ty, i64::from(specialization.expected_index));
        let type_matches =
            emit_exact_type_version_match(fb, value, owner_type, specialization.type_version);
        fb.ins()
            .brif(type_matches, maybe_direct_block, &[], next_guard_block, &[]);

        fb.switch_to_block(maybe_direct_block);
        let direct_inst = fb.ins().call(
            emit_ctx.store_field_indexed_ref,
            &[
                emit_ctx.consts.thread_state_value,
                value,
                attr,
                expected_index,
                replacement,
            ],
        );
        let direct_result = fb.inst_results(direct_inst)[0];
        let direct_missed = fb
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, direct_result, zero_i32);
        fb.ins().brif(
            direct_missed,
            guard_miss_dispatch.branch_block(),
            &[],
            direct_block,
            &[],
        );

        fb.switch_to_block(direct_block);
        if let Some(counter_id) = hit_counter_id {
            emit_increment_counter_ref(fb, counter_id, emit_ctx);
        }
        if result_needs_pyobject {
            let none_const = emit_none_const(fb, emit_ctx);
            fb.ins().call(emit_ctx.incref_ref, &[none_const]);
            emit_release_owned_inputs(fb, emit_ctx, owned_inputs.as_slice());
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(none_const)]);
        } else {
            emit_release_owned_inputs(fb, emit_ctx, owned_inputs.as_slice());
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
                owned_inputs.as_slice(),
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
                owned_inputs.as_slice(),
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
        let value = emit_typed_codegen_expr_value_with_local_env(
            fb,
            op.value(),
            local_env,
            emit_ctx,
            false,
            codegen_env,
            func_imports,
        )?;
        let is_true_ref = func_imports.get(codegen_env, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?;
        return Ok(emit_truthy_from_owned_value(
            fb,
            value,
            is_true_ref,
            emit_ctx,
        ));
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
        let legacy_expr = try_lower_typed_instr_to_codegen_legacy(expr.clone())?;
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
        && let TypedAttrAccessPlan::IndexedField { source, guards } = &op.access
    {
        let maybe_value = emit_typed_indexed_getattr(
            fb,
            op,
            *source,
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
        && let TypedAttrAccessPlan::IndexedField { source, guards } = &op.access
    {
        let maybe_value = emit_typed_indexed_setattr(
            fb,
            op,
            *source,
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

    let legacy_expr = try_lower_typed_instr_to_codegen_legacy(expr.clone())?;
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
    simple_args: Vec<&'a InstrCodegen>,
    simple_keywords: Vec<(&'a str, &'a InstrCodegen)>,
    has_unpack: bool,
}

struct TypedSimpleCallParts<'a> {
    simple_args: Vec<&'a InstrTyped>,
    simple_keywords: Vec<(&'a str, &'a InstrTyped)>,
    has_unpack: bool,
}

fn simple_call_parts(call: &soac_core::block_py::Call<InstrCodegen>) -> SimpleCallParts<'_> {
    let mut simple_args: Vec<&InstrCodegen> = Vec::new();
    let mut simple_keywords: Vec<(&str, &InstrCodegen)> = Vec::new();
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
    if !matches!(call.access, TypedCallAccessPlan::Generic) {
        return false;
    }
    let TypedSimpleCallParts {
        simple_args,
        simple_keywords,
        has_unpack,
    } = typed_simple_call_parts(call);
    if has_unpack || !simple_keywords.is_empty() {
        return false;
    }
    if typed_expr_runtime_helper(call.func.as_ref(), emit_ctx).is_some() {
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
    Ok(
        if demand == ResultDemand::EffectOnly && arg_values.len() <= 3 {
            emit_positional_call_three_result_with_arg_values(
                fb,
                callable,
                callable_is_borrowed,
                arg_values,
                arg_borrowed,
                emit_ctx,
                demand,
            )
        } else {
            emit_positional_vectorcall_result_with_arg_values(
                fb,
                callable,
                callable_is_borrowed,
                arg_values,
                arg_borrowed,
                emit_ctx,
                demand,
            )
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_codegen_simple_call_effect_only_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_core::block_py::Call<InstrCodegen>,
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
    call: &soac_core::block_py::Call<InstrCodegen>,
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
        fb.ins()
            .call(emit_ctx.incref_ref, &[emit_ctx.consts.block_const]);
        return Some(emit_ctx.consts.block_const);
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
            let InstrCodegen::Load(cell_name) = simple_args[0] else {
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
        if let Some(TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
            runtime_name,
            method_name,
            method_guards,
        }) = typed_access
            && *runtime_name == RuntimeName::Iter
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
        let (constructor_specializations, direct_specializations) = match typed_access {
            Some(TypedCallAccessPlan::GuardedCallable {
                function_guards,
                constructor_guards,
            }) => (
                direct_constructor_specializations_from_typed_guards(constructor_guards),
                direct_function_specializations_from_typed_guards(function_guards),
            ),
            Some(_) => (Vec::new(), Vec::new()),
            _ => {
                let constructor_specializations = Vec::new();
                let direct_specializations = call_site_profiled_targets(call, profiled_targets)
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
                                let arg_plan = match validate_direct_call_compatibility(
                                    target_function,
                                    emit_ctx.direct_call_functions,
                                    simple_args.len(),
                                    0,
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
                                Some(DirectFunctionSpecialization {
                                    function_id,
                                    arg_plan,
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                (constructor_specializations, direct_specializations)
            }
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
            let InstrCodegen::GetAttr(getattr) = call.func.as_ref() else {
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
        let should_emit_callee_id = call_target_counter.is_some()
            || !constructor_specializations.is_empty()
            || !direct_specializations.is_empty();
        let callee_id = should_emit_callee_id
            .then(|| emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env));
        if let Some(counter_id) = call_target_counter {
            let callee_id = callee_id.expect("callee id should exist for call target counter");
            emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
        }
        if !constructor_specializations.is_empty() || !direct_specializations.is_empty() {
            let result_block = fb.create_block();
            fb.append_block_param(result_block, ptr_ty);
            let generic_block = fb.create_block();
            fb.set_cold_block(generic_block);
            let direct_guard_miss_dispatch = if !constructor_specializations.is_empty() {
                JitGuardMissDispatch::FallbackBlock(generic_block)
            } else if let Some(site_instr_id) = site_instr_id {
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
            let mut direct_chain_start = None;
            if !constructor_specializations.is_empty() {
                let mut next_miss_block = fb.create_block();
                for (index, specialization) in constructor_specializations.iter().enumerate() {
                    let Some(expected_type) = emit_type_ptr_value_for_ref(
                        fb,
                        codegen_env,
                        emit_ctx,
                        &specialization.owner_type_ref,
                    )
                    .unwrap_or_else(|err| {
                        panic!("failed to bind constructor type symbol: {err}");
                    }) else {
                        continue;
                    };
                    let type_match_block = fb.create_block();
                    let direct_block = fb.create_block();
                    let miss_block = if index + 1 == constructor_specializations.len() {
                        if direct_specializations.is_empty() {
                            generic_block
                        } else {
                            fb.create_block()
                        }
                    } else {
                        fb.create_block()
                    };
                    let is_exact_type =
                        fb.ins()
                            .icmp(ir::condcodes::IntCC::Equal, callable, expected_type);
                    fb.ins()
                        .brif(is_exact_type, type_match_block, &[], miss_block, &[]);

                    fb.switch_to_block(type_match_block);
                    let type_version = fb.ins().load(
                        ir::types::I32,
                        ir::MemFlags::trusted(),
                        callable,
                        offset_of!(ffi::PyTypeObject, tp_version_tag) as i32,
                    );
                    let version_matches = fb.ins().icmp_imm(
                        ir::condcodes::IntCC::Equal,
                        type_version,
                        specialization.type_version as i64,
                    );
                    fb.ins()
                        .brif(version_matches, direct_block, &[], miss_block, &[]);

                    fb.switch_to_block(direct_block);
                    let target_function =
                        direct_call_target_function(emit_ctx, specialization.function_id)
                            .expect("direct constructor specialization target should exist");
                    if let Some(counter_id) = direct_hit_counter_id {
                        emit_increment_counter_ref(fb, counter_id, emit_ctx);
                    }
                    let direct_result = emit_direct_constructor_resolved_with_args_from_local_env(
                        fb,
                        callable,
                        callable_is_borrowed,
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
                    if index + 1 != constructor_specializations.len() {
                        fb.switch_to_block(miss_block);
                    } else {
                        next_miss_block = miss_block;
                    }
                }
                direct_chain_start = Some(next_miss_block);
            }

            if !direct_specializations.is_empty() {
                if let Some(start_block) = direct_chain_start {
                    fb.switch_to_block(start_block);
                }
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
                for (index, specialization) in direct_specializations.iter().enumerate() {
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
                    let is_match = fb.ins().band(is_match, callable_is_exact_function);
                    fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

                    fb.switch_to_block(direct_block);
                    let target_function =
                        direct_call_target_function(emit_ctx, specialization.function_id)
                            .expect("direct specialization target should exist");
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
    make_function: &soac_core::block_py::MakeFunctionWithClosure<InstrCodegen>,
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
    expr: &InstrCodegen,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    if let InstrCodegen::Load(op) = expr {
        return emit_resolved_name_load_with_local_env(
            fb,
            &op.name,
            op.try_semantic_instr_id(),
            local_env,
            emit_ctx,
            borrowed,
        );
    }
    if let InstrCodegen::IncrementCounter(op) = expr {
        assert!(
            !borrowed,
            "increment_counter must not request a borrowed result"
        );
        return emit_increment_counter(fb, op.counter_id, emit_ctx);
    }
    if let InstrCodegen::MakeFunctionWithClosure(op) = expr {
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
    if let InstrCodegen::Tuple(op) = expr {
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
    if let InstrCodegen::CellRef(op) = expr {
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
        InstrCodegen::BinOp(_)
            | InstrCodegen::UnaryOp(_)
            | InstrCodegen::GetAttr(_)
            | InstrCodegen::SetAttr(_)
            | InstrCodegen::GetItem(_)
            | InstrCodegen::SetItem(_)
            | InstrCodegen::DelItem(_)
            | InstrCodegen::Store(_)
            | InstrCodegen::Del(_)
            | InstrCodegen::MakeCell(_)
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
        };
        if let Some(value) = intrinsics::emit_operation(expr, &mut intrinsic_state) {
            return value;
        }
    }
    if let InstrCodegen::Store(op) = expr {
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
        fb.ins().call(
            emit_ctx.decref_ref,
            &[emit_ctx.consts.thread_state_value, raw_cell],
        );
        if !value_borrowed {
            fb.ins().call(
                emit_ctx.decref_ref,
                &[emit_ctx.consts.thread_state_value, value],
            );
        }
        let call_value = fb.inst_results(call_inst)[0];
        let mut intrinsic_state = LocalEnvCodegenIntrinsicEmitState {
            fb,
            local_env,
            ctx: emit_ctx,
            codegen_env,
            func_imports,
        };
        return intrinsics::OperationEmitState::<InstrCodegen>::finish_owned_result(
            &mut intrinsic_state,
            call_value,
        );
    }
    if let InstrCodegen::Del(op) = expr {
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
        };
        return intrinsics::emit_del_deref_raw_cell::<InstrCodegen>(
            raw_cell,
            op.quietly,
            &mut intrinsic_state,
        );
    }
    if let InstrCodegen::Call(call) = expr {
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
    match result {
        EmitResult::NoValue | EmitResult::I32 { .. } | EmitResult::I64 { .. } => Ok(()),
        EmitResult::PyObject {
            value, ownership, ..
        } => {
            if ownership.is_owned() {
                fb.ins().call(
                    emit_ctx.decref_ref,
                    &[emit_ctx.consts.thread_state_value, value],
                );
            }
            Ok(())
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
    match demand {
        ResultDemand::EffectOnly => {
            if !facts.is_immortal() {
                fb.ins().call(
                    emit_ctx.decref_ref,
                    &[emit_ctx.consts.thread_state_value, value],
                );
            }
            EmitResult::no_value()
        }
        ResultDemand::PyObject { .. } => {
            let ownership = if facts.is_immortal() {
                ValueOwnership::Immortal
            } else {
                ValueOwnership::Owned
            };
            EmitResult::PyObject {
                value,
                ownership,
                facts,
            }
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

fn direct_positional_call_args(
    call: &soac_core::block_py::Call<InstrCodegen>,
    param_count: usize,
) -> Option<Vec<&InstrCodegen>> {
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

#[cfg(test)]
fn static_runtime_primitive_for_call(
    call: &soac_core::block_py::Call<InstrCodegen>,
    module_constants: &ModuleCodegenConstants,
) -> Option<direct_abi::RuntimePrimitiveId> {
    let desc = static_runtime_primitive_desc_for_call(call, module_constants)?;
    let DirectTargetId::RuntimePrimitive(primitive) = desc.target else {
        return None;
    };
    Some(primitive)
}

fn static_runtime_primitive_desc_for_call(
    call: &soac_core::block_py::Call<InstrCodegen>,
    module_constants: &ModuleCodegenConstants,
) -> Option<&'static DirectCallableDesc> {
    let name = codegen_expr_static_runtime_name(call.func.as_ref(), module_constants)?;
    let primitive = direct_abi::runtime_primitive_for_builtin_name(name)?;
    let desc = direct_abi::runtime_primitive_desc(primitive);
    let _ = direct_positional_call_args(call, desc.abi.params.len())?;
    Some(desc)
}

fn static_runtime_primitive_desc_for_typed_call(
    call: &TypedCall<InstrTyped>,
    module_constants: &ModuleCodegenConstants,
) -> Option<&'static DirectCallableDesc> {
    let name = typed_expr_static_runtime_name(call.func.as_ref(), module_constants)?;
    let primitive = direct_abi::runtime_primitive_for_builtin_name(name)?;
    let desc = direct_abi::runtime_primitive_desc(primitive);
    let _ = typed_direct_positional_call_args(call, desc.abi.params.len())?;
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
    expr: &InstrCodegen,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> Option<IntFacts> {
    if let Some(value) = codegen_expr_const_i64(expr, emit_ctx.module_constants) {
        return Some(IntFacts::i64_known(value));
    }
    match expr {
        InstrCodegen::Call(call) => {
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
        InstrCodegen::BinOp(op) => {
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

fn codegen_expr_can_satisfy_i64_demand(
    expr: &InstrCodegen,
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

fn codegen_expr_has_exact_int_pyobject_facts(
    expr: &InstrCodegen,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    if !matches!(emit_ctx.function_kind, FunctionKind::Function) {
        return false;
    }
    if let InstrCodegen::Load(op) = expr {
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
    expr: &InstrCodegen,
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
    call: &soac_core::block_py::Call<InstrCodegen>,
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
    expr: &InstrCodegen,
    module_constants: &ModuleCodegenConstants,
) -> bool {
    codegen_expr_static_i64_demand_facts(expr, module_constants).is_some()
}

#[cfg(test)]
fn codegen_expr_static_i64_demand_facts(
    expr: &InstrCodegen,
    module_constants: &ModuleCodegenConstants,
) -> Option<IntFacts> {
    if let Some(value) = codegen_expr_const_i64(expr, module_constants) {
        return Some(IntFacts::i64_known(value));
    }
    match expr {
        InstrCodegen::Call(call) => {
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
        InstrCodegen::BinOp(op) => {
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
    call: &soac_core::block_py::Call<InstrCodegen>,
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
    match demand {
        ResultDemand::EffectOnly => EmitResult::no_value(),
        ResultDemand::I64 | ResultDemand::I64Index => EmitResult::i64(value, facts),
        ResultDemand::I32Bool01 => {
            let is_true = fb.ins().icmp_imm(ir::condcodes::IntCC::NotEqual, value, 0);
            let truth = emit_i32_bool01_from_cond(fb, is_true, emit_ctx);
            let (truth_i32, truth_facts) = truth.expect_i32("I64 truthiness demand");
            EmitResult::i32(truth_i32, truth_facts)
        }
        ResultDemand::PyObject { .. } => {
            let boxed = emit_to_python_long(
                fb,
                SoacValue::i64(value, facts),
                emit_ctx.py_long_from_i64_ref,
                emit_ctx,
            );
            let (boxed, ownership, boxed_facts) = boxed.expect_pyobject("I64 Python object demand");
            EmitResult::pyobject(boxed, ownership, boxed_facts)
        }
    }
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
    op: &blockpy_intrinsics::BinOp<InstrCodegen>,
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

fn emit_exact_pylong_as_i64_saturating_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrCodegen,
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
    expr: &InstrCodegen,
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
            let value = emit_typed_codegen_expr_value_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                typed_expr_pyobject_input_is_borrowed_from_local_env(expr, local_env, emit_ctx),
                codegen_env,
                func_imports,
            )?;
            let (value, ownership, _) =
                value.expect_pyobject("typed runtime primitive PyObject param");
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
    call: &soac_core::block_py::Call<InstrCodegen>,
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

fn emit_runtime_builtin_primitive_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_core::block_py::Call<InstrCodegen>,
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
    call: &soac_core::block_py::Call<InstrCodegen>,
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
    let (callable, callable_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        call.func.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed call callable",
    )?;

    if let Some(counter_id) = call
        .try_semantic_instr_id()
        .and_then(|site_instr_id| emit_ctx.call_target_counter_ids.get(&site_instr_id))
        .copied()
    {
        let callee_id = emit_callee_function_id_checked(fb, callable, emit_ctx, codegen_env);
        emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
    }

    let result = emit_typed_positional_call_result_with_arg_refs(
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
    let (constructor_specializations, direct_specializations) = match &call.access {
        TypedCallAccessPlan::GuardedCallable {
            function_guards,
            constructor_guards,
        } => (
            direct_constructor_specializations_from_typed_guards(constructor_guards),
            direct_function_specializations_from_typed_guards(function_guards),
        ),
        TypedCallAccessPlan::ProfiledCallableTargets { targets } => {
            let direct_specializations = targets
                .iter()
                .copied()
                .filter_map(|function_id| {
                    let Some(target_function) = direct_call_target_function(emit_ctx, function_id)
                    else {
                        emit_ctx
                            .direct_edge_stats
                            .record_profiled_missing_target_candidate();
                        return None;
                    };
                    if target_function.names.fn_name == "__init__" {
                        return None;
                    }
                    let arg_plan = match validate_direct_call_compatibility(
                        target_function,
                        emit_ctx.direct_call_functions,
                        simple_args.len(),
                        0,
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
                    Some(DirectFunctionSpecialization {
                        function_id,
                        arg_plan,
                    })
                })
                .collect::<Vec<_>>();
            (Vec::new(), direct_specializations)
        }
        TypedCallAccessPlan::Generic => (Vec::new(), Vec::new()),
        TypedCallAccessPlan::ProfiledMethodTargets { .. }
        | TypedCallAccessPlan::GuardedMethod { .. }
        | TypedCallAccessPlan::GuardedRuntimeProtocolMethod { .. } => return Ok(None),
    };
    if constructor_specializations.is_empty() && direct_specializations.is_empty() {
        return Ok(None);
    }

    emit_typed_prepared_direct_callable_specialization_result_with_local_env(
        fb,
        call.func.as_ref(),
        arg_refs.as_slice(),
        site_instr_id,
        constructor_specializations.as_slice(),
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
    constructor_specializations: &[DirectConstructorSpecialization],
    direct_specializations: &[DirectFunctionSpecialization],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    if constructor_specializations.is_empty() && direct_specializations.is_empty() {
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
    let should_emit_callee_id = call_target_counter.is_some()
        || !constructor_specializations.is_empty()
        || !direct_specializations.is_empty();
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
    let direct_guard_miss_dispatch = if !constructor_specializations.is_empty() {
        JitGuardMissDispatch::FallbackBlock(generic_block)
    } else if let Some(site_instr_id) = site_instr_id {
        prepare_typed_guard_miss_dispatch_for_instr(emit_ctx, site_instr_id, &[func], generic_block)
    } else {
        JitGuardMissDispatch::FallbackBlock(generic_block)
    };

    let mut direct_chain_start = None;
    if !constructor_specializations.is_empty() {
        let mut next_miss_block = fb.create_block();
        for (index, specialization) in constructor_specializations.iter().enumerate() {
            let Some(expected_type) = emit_type_ptr_value_for_ref(
                fb,
                codegen_env,
                emit_ctx,
                &specialization.owner_type_ref,
            )
            .unwrap_or_else(|err| {
                panic!("failed to bind constructor type symbol: {err}");
            }) else {
                continue;
            };
            let type_match_block = fb.create_block();
            let direct_block = fb.create_block();
            let miss_block = if index + 1 == constructor_specializations.len() {
                if direct_specializations.is_empty() {
                    generic_block
                } else {
                    fb.create_block()
                }
            } else {
                fb.create_block()
            };
            let is_exact_type = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, callable, expected_type);
            fb.ins()
                .brif(is_exact_type, type_match_block, &[], miss_block, &[]);

            fb.switch_to_block(type_match_block);
            let type_version = fb.ins().load(
                ir::types::I32,
                ir::MemFlags::trusted(),
                callable,
                offset_of!(ffi::PyTypeObject, tp_version_tag) as i32,
            );
            let version_matches = fb.ins().icmp_imm(
                ir::condcodes::IntCC::Equal,
                type_version,
                specialization.type_version as i64,
            );
            fb.ins()
                .brif(version_matches, direct_block, &[], miss_block, &[]);

            fb.switch_to_block(direct_block);
            let target_function = direct_call_target_function(emit_ctx, specialization.function_id)
                .expect("direct constructor specialization target should exist");
            if let Some(counter_id) = direct_hit_counter_id {
                emit_increment_counter_ref(fb, counter_id, emit_ctx);
            }
            let direct_result = emit_typed_direct_constructor_resolved_with_args_from_local_env(
                fb,
                callable,
                callable_is_borrowed,
                arg_refs,
                specialization,
                target_function,
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
            )?;
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(direct_result)]);
            if index + 1 != constructor_specializations.len() {
                fb.switch_to_block(miss_block);
            } else {
                next_miss_block = miss_block;
            }
        }
        direct_chain_start = Some(next_miss_block);
    }

    if !direct_specializations.is_empty() {
        if let Some(start_block) = direct_chain_start {
            fb.switch_to_block(start_block);
        }
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
        for (index, specialization) in direct_specializations.iter().enumerate() {
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
            let is_match = fb.ins().band(is_match, callable_is_exact_function);
            fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

            fb.switch_to_block(direct_block);
            let target_function = direct_call_target_function(emit_ctx, specialization.function_id)
                .expect("direct specialization target should exist");
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
    Ok(Some(emit_owned_pyobject_result_for_demand(
        fb,
        result,
        PyObjFacts::unknown(),
        emit_ctx,
        demand,
    )))
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
    let arg_refs = typed_simple_positional_arg_refs(
        call.args.as_slice(),
        call.keywords.as_slice(),
        "typed guarded callable call",
    )?;
    let constructor_specializations =
        direct_constructor_specializations_from_typed_guards(call.constructor_guards.as_slice());
    let direct_specializations =
        direct_function_specializations_from_typed_guards(call.function_guards.as_slice());
    if constructor_specializations.is_empty() && direct_specializations.is_empty() {
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
        constructor_specializations.as_slice(),
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
    Ok(emit_owned_pyobject_result_for_demand(
        fb,
        result,
        PyObjFacts::unknown(),
        emit_ctx,
        demand,
    ))
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
    let (callable, callable_is_borrowed) = emit_typed_pyobject_input_with_local_env(
        fb,
        call.func.as_ref(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        "typed direct callable",
    )?;
    let mut arg_refs = Vec::with_capacity(call.args.len());
    for arg in &call.args {
        let CallArgPositional::Positional(arg) = arg else {
            return Err("typed direct callable call does not support starred args".to_string());
        };
        arg_refs.push(arg);
    }
    let result = match &call.guard {
        TypedDirectCallableCallGuard::Function(guard) => {
            let target_function = direct_call_target_function(emit_ctx, guard.function_id)
                .ok_or_else(|| {
                    format!(
                        "typed direct callable call target {:?} is unavailable",
                        guard.function_id
                    )
                })?;
            let arg_plan = direct_call_arg_plan_from_typed(&guard.arg_plan);
            emit_typed_direct_call_resolved_with_arg_plan_from_local_env(
                fb,
                callable,
                callable_is_borrowed,
                arg_refs.as_slice(),
                &arg_plan,
                target_function,
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
            )?
        }
        TypedDirectCallableCallGuard::Constructor(guard) => {
            let specialization = direct_constructor_specialization_from_typed_guard(guard)
                .ok_or_else(|| {
                    "typed direct constructor call has invalid owner type ref".to_string()
                })?;
            let target_function = direct_call_target_function(emit_ctx, specialization.function_id)
                .ok_or_else(|| {
                    format!(
                        "typed direct constructor call target {:?} is unavailable",
                        specialization.function_id
                    )
                })?;
            emit_typed_direct_constructor_resolved_with_args_from_local_env(
                fb,
                callable,
                callable_is_borrowed,
                arg_refs.as_slice(),
                &specialization,
                target_function,
                local_env,
                emit_ctx,
                codegen_env,
                func_imports,
            )?
        }
    };
    Ok(emit_owned_pyobject_result_for_demand(
        fb,
        result,
        PyObjFacts::unknown(),
        emit_ctx,
        demand,
    ))
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
    Ok(emit_owned_pyobject_result_for_demand(
        fb,
        result,
        PyObjFacts::unknown(),
        emit_ctx,
        demand,
    ))
}

fn emit_codegen_stmt_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrCodegen,
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
        InstrCodegen::Store(op) => {
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
        InstrCodegen::Del(op) => {
            if let Some(result) =
                emit_local_delete_result_with_local_env(fb, op, local_env, emit_ctx, demand)
            {
                return Ok(result);
            }
        }
        InstrCodegen::BinOp(op) => {
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
        InstrCodegen::Call(call) => {
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
    Ok(emit_owned_pyobject_result_for_demand(
        fb, value, facts, emit_ctx, demand,
    ))
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
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        op.value.as_ref(),
        local_env,
        emit_ctx,
        value_is_borrowed,
        codegen_env,
        func_imports,
    )?;
    let (raw_value, ownership, facts) = value.expect_pyobject("typed direct-call guard input");

    let guard = match &op.kind {
        TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id } => {
            emit_exact_function_id_match_bool01(fb, raw_value, *function_id, emit_ctx, codegen_env)?
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
                fb.ins().call(emit_ctx.incref_ref, &[value]);
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
            let is_true = fb
                .ins()
                .icmp_imm(ir::condcodes::IntCC::NotEqual, truth_i32, 0);
            let true_const = emit_true_const(fb, emit_ctx);
            let false_const = emit_false_const(fb, emit_ctx);
            let bool_value = fb.ins().select(is_true, true_const, false_const);
            if !borrowed {
                fb.ins().call(emit_ctx.incref_ref, &[bool_value]);
            }
            bool_value
        }
        SoacValue::I32 { .. } | SoacValue::I64 { .. } => {
            return Err(format!(
                "typed expression produced {:?} without a PyObject materializer",
                value.repr()
            ));
        }
    })
}

fn emit_codegen_stmt_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrCodegen,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    match expr {
        InstrCodegen::Store(op) => {
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
        InstrCodegen::Del(op) => {
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

    let legacy_expr = try_lower_typed_instr_to_codegen_legacy(expr.clone())?;
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
                    fb.ins().call(
                        emit_ctx.decref_ref,
                        &[emit_ctx.consts.thread_state_value, value],
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
                EmitResult::PyObject {
                    value,
                    ownership,
                    facts,
                }
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
        let result = if let TypedAttrAccessPlan::IndexedField { source, guards } = &op.access {
            emit_typed_indexed_getattr(
                fb,
                op,
                *source,
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
                    fb.ins().call(
                        emit_ctx.decref_ref,
                        &[emit_ctx.consts.thread_state_value, value],
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
                EmitResult::PyObject {
                    value,
                    ownership,
                    facts,
                }
            }
            ResultDemand::I32Bool01 | ResultDemand::I64 | ResultDemand::I64Index => {
                panic!("typed getattr cannot satisfy non-PyObject demand {demand:?}")
            }
        });
    }
    if let InstrTyped::SetAttrTyped(op) = expr {
        if let TypedAttrAccessPlan::IndexedField { source, guards } = &op.access {
            if let Some(result) = emit_typed_indexed_setattr(
                fb,
                op,
                *source,
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
        return Ok(match demand {
            ResultDemand::EffectOnly => {
                let (_, facts) = planned_owned_pyobject_result_for_typed_expr(expr, local_env);
                if !facts.is_immortal() {
                    fb.ins().call(
                        emit_ctx.decref_ref,
                        &[emit_ctx.consts.thread_state_value, value],
                    );
                }
                EmitResult::no_value()
            }
            ResultDemand::PyObject { .. } => {
                let (ownership, facts) =
                    planned_owned_pyobject_result_for_typed_expr(expr, local_env);
                EmitResult::PyObject {
                    value,
                    ownership,
                    facts,
                }
            }
            ResultDemand::I32Bool01 => unreachable!("I32Bool01 handled before PyObject emission"),
            ResultDemand::I64 => unreachable!("I64 is not a generic PyObject statement demand"),
            ResultDemand::I64Index => unreachable!("I64Index is not a statement demand"),
        });
    }

    let legacy_expr = try_lower_typed_instr_to_codegen_legacy(expr.clone())?;
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
    Some(match demand {
        ResultDemand::EffectOnly => EmitResult::no_value(),
        ResultDemand::PyObject { .. } => EmitResult::PyObject {
            value,
            ownership,
            facts,
        },
        ResultDemand::I32Bool01 | ResultDemand::I64 | ResultDemand::I64Index => unreachable!(
            "typed local load borrowed result plan does not satisfy non-PyObject demands"
        ),
    })
}

#[derive(Clone, Copy, Debug)]
enum OptV3MechanicalValue {
    PyObject { value: ir::Value, owned: bool },
    I64(ir::Value),
    I32Bool01(ir::Value),
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
                Rep::PyObjectOwned | Rep::PyObjectImmortal
            ) | (
                Self::PyObject { owned: false, .. },
                Rep::PyObjectBorrowed | Rep::PyObjectImmortal
            ) | (Self::I64(_), Rep::I64)
                | (Self::I32Bool01(_), Rep::I32Bool01)
        )
    }
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
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
    .map(|result| Some(result.value))
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
    let result = emit_opt_v3_exact_int_return_selection(
        fb,
        plan.instr_id,
        plan.into(),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    if !result.ownership.can_satisfy_pyobject_demand(demand) {
        return Err(format!(
            "optimizer v3 expression result for {} produced {:?}, but demand is {demand:?}",
            plan.instr_id, result.ownership
        ));
    }
    let facts =
        py_facts_for_typed_expr_with_local_env(expr, local_env).unwrap_or_else(PyObjFacts::unknown);
    Ok(Some(EmitResult::PyObject {
        value: result.value,
        ownership: result.ownership,
        facts,
    }))
}

fn emit_opt_v3_exact_int_branch_selection(
    fb: &mut FunctionBuilder<'_>,
    test_instr_id: InstrId,
    selection: ExactIntBranchEmissionSelection<'_>,
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
        "exact-int branch hot region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.hot_region,
        &mut hot_values,
        Some(fallback_block),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let hot_condition =
        opt_v3_region_branch_condition(selection.hot_region, &hot_values, test_instr_id)?;
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(hot_condition)]);

    fb.switch_to_block(fallback_block);
    let mut fallback_values = opt_v3_region_input_values(
        fb,
        selection.fallback_plan,
        local_env,
        emit_ctx,
        "exact-int branch fallback region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.fallback_region,
        &mut fallback_values,
        None,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let fallback_condition =
        opt_v3_region_branch_condition(selection.fallback_region, &fallback_values, test_instr_id)?;
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_condition)]);

    fb.switch_to_block(result_block);
    Ok(fb.block_params(result_block)[0])
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_exact_int_return_selection(
    fb: &mut FunctionBuilder<'_>,
    value_instr_id: InstrId,
    selection: ExactIntReturnEmissionSelection<'_>,
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
        "exact-int return hot region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.hot_region,
        &mut hot_values,
        Some(fallback_block),
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let hot_result =
        opt_v3_region_return_pyobject(selection.hot_region, &hot_values, value_instr_id)?;
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(hot_result.value)]);

    fb.switch_to_block(fallback_block);
    let mut fallback_values = opt_v3_region_input_values(
        fb,
        selection.fallback_plan,
        local_env,
        emit_ctx,
        "exact-int return fallback region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.fallback_region,
        &mut fallback_values,
        None,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let fallback_result =
        opt_v3_region_return_pyobject(selection.fallback_region, &fallback_values, value_instr_id)?;
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_result.value)]);

    fb.switch_to_block(result_block);
    Ok(OptV3PyObjectResult {
        value: fb.block_params(result_block)[0],
        ownership: merge_opt_v3_pyobject_ownership(hot_result.ownership, fallback_result.ownership),
    })
}

fn opt_v3_region_input_values(
    fb: &mut FunctionBuilder<'_>,
    region: &RegionPlan,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    context: &str,
) -> Result<HashMap<PlanValue, OptV3MechanicalValue>, String> {
    let mut values = HashMap::new();
    for input in opt_v3_mechanical_region_function_param_inputs(region, context)? {
        let value = local_env
            .load_name(fb, input.name, emit_ctx, true)
            .ok_or_else(|| {
                format!(
                    "optimizer v3 {context} input {:?} references unavailable local {:?}",
                    input.value, input.name
                )
            })?;
        opt_v3_store_mechanical_value(
            &mut values,
            input.value,
            OptV3MechanicalValue::PyObject {
                value,
                owned: false,
            },
        )?;
    }
    Ok(values)
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_mechanical_region_steps(
    fb: &mut FunctionBuilder<'_>,
    region: &MechanicalRegionEmission,
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    local_fallback_block: Option<ir::Block>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    emit_opt_v3_mechanical_region_steps_controlled(
        fb,
        region,
        values,
        local_fallback_block,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn emit_opt_v3_mechanical_region_steps_until_value(
    fb: &mut FunctionBuilder<'_>,
    region: &MechanicalRegionEmission,
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    local_fallback_block: Option<ir::Block>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    stop_after_value: PlanValue,
) -> Result<(), String> {
    emit_opt_v3_mechanical_region_steps_controlled(
        fb,
        region,
        values,
        local_fallback_block,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        Some(stop_after_value),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn emit_opt_v3_mechanical_region_steps_with_preseeded_scalar(
    fb: &mut FunctionBuilder<'_>,
    region: &MechanicalRegionEmission,
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    local_fallback_block: Option<ir::Block>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    preseeded_scalar: PlanValue,
) -> Result<(), String> {
    emit_opt_v3_mechanical_region_steps_controlled(
        fb,
        region,
        values,
        local_fallback_block,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
        None,
        Some(preseeded_scalar),
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_v3_mechanical_region_steps_controlled(
    fb: &mut FunctionBuilder<'_>,
    region: &MechanicalRegionEmission,
    values: &mut HashMap<PlanValue, OptV3MechanicalValue>,
    local_fallback_block: Option<ir::Block>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    stop_after_value: Option<PlanValue>,
    preseeded_scalar: Option<PlanValue>,
) -> Result<(), String> {
    if let Some(value) = stop_after_value
        && values.contains_key(&value)
    {
        return Ok(());
    }
    let preseeded_convert_inputs = if let Some(preseeded_scalar) = preseeded_scalar {
        opt_v3_mechanical_convert_inputs_for_output(region, preseeded_scalar)
    } else {
        HashSet::new()
    };
    for step in &region.steps {
        match opt_v3_mechanical_codegen_step(
            region.region,
            step,
            local_fallback_block.is_some(),
            preseeded_scalar,
            &preseeded_convert_inputs,
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
        if let Some(value) = stop_after_value
            && values.contains_key(&value)
        {
            return Ok(());
        }
    }
    if let Some(value) = stop_after_value {
        return Err(format!(
            "optimizer v3 region {:?} did not produce requested stop value {:?}",
            region.region, value.id
        ));
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
    let truth = emit_truthy_from_owned_value(fb, value, is_true_ref, emit_ctx);
    let truth_i32 = truth.expect_i32_bool01("typed I32Bool01 demand");
    Ok(EmitResult::i32(truth_i32, IntFacts::i32_bool01()))
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
            fb.ins().call(
                emit_ctx.decref_ref,
                &[emit_ctx.consts.thread_state_value, callable],
            );
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
) -> Result<Option<JitEmitCtx<'mc>>, String> {
    if !emit_ctx.consts.step_null_args.is_empty() {
        return Ok(None);
    }
    let (forwarded_values, forwarded_local_indices, continuation) =
        if let Some(forwarded_names) = emit_ctx.exception_forwarded_local_names {
            let (forwarded_values, forwarded_local_indices) =
                emit_forward_named_values_from_local_env(fb, forwarded_names, local_env, emit_ctx)
                    .map_err(|err| {
                        format!("missing local mapping for failure cleanup forwarding: {err}")
                    })?;
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
            continuation,
        });
        local_failure_cleanup_blocks.insert(key, cleanup_block);
        cleanup_block
    };
    let mut step_null_args: Vec<_> = cleanup_entries.iter().map(|entry| entry.value).collect();
    step_null_args.extend(forwarded_values);
    Ok(Some(
        emit_ctx.with_step_null_target(cleanup_block, step_null_args),
    ))
}

fn emit_typed_codegen_ops(
    fb: &mut FunctionBuilder<'_>,
    ops: &[InstrTyped],
    local_env: &mut LocalEnv,
    _stack_slots: &StackSlots,
    emit_ctx: &JitEmitCtx<'_>,
    cleanup_null_block: ir::Block,
    pending_local_failure_cleanups: &mut Vec<PendingLocalFailureCleanup>,
    local_failure_cleanup_blocks: &mut HashMap<LocalFailureCleanupKey, ir::Block>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    for expr in ops {
        let instr_id = expr.try_semantic_instr_id();
        if let Some(instr_id) = instr_id {
            emit_ctx.require_deopt_point_before_instr_id(instr_id)?;
        }
        let stmt_emit_ctx = local_failure_cleanup_emit_ctx(
            fb,
            emit_ctx,
            local_env,
            cleanup_null_block,
            pending_local_failure_cleanups,
            local_failure_cleanup_blocks,
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
    }
    Ok(())
}

#[allow(dead_code)]
fn codegen_block_has_predecessor(
    function: &BlockPyFunction<CodegenModuleShape>,
    target: BlockLabel,
) -> usize {
    function
        .blocks
        .iter()
        .filter(|block| match &block.term {
            BlockTerm::Jump(edge) => edge.target == target,
            BlockTerm::IfTerm(if_term) => {
                if_term.then_label == target || if_term.else_label == target
            }
            BlockTerm::BranchTable(branch) => {
                branch.default_label == target || branch.targets.contains(&target)
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => false,
        })
        .count()
}

#[allow(dead_code)]
fn cranelift_value_args(args: &[ir::BlockArg]) -> Result<Vec<ir::Value>, String> {
    args.iter()
        .map(|arg| match arg {
            ir::BlockArg::Value(value) => Ok(*value),
            other => Err(format!(
                "optimizer v3 scalar-thread jump expected value block arg, got {other:?}"
            )),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn emit_opt_v3_scalar_thread_inline_return_branch(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    test_instr_id: Option<InstrId>,
    truth_i32: ir::Value,
    unmaterialized_location: Option<LocalLocation>,
    targets: OptV3ScalarThreadInlineReturnTargets<'_>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    if let Some(test_instr_id) = test_instr_id
        && let Some(counter_id) = emit_ctx
            .branch_outcome_counter_ids
            .get(&test_instr_id)
            .copied()
    {
        emit_record_branch_outcome_sample(fb, counter_id, truth_i32, emit_ctx);
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
    fb.ins().brif(hot_cond, hot_branch, &[], cold_branch, &[]);

    let (hot_label, hot_term, cold_label, cold_term) = if prefer_true {
        (
            targets.then_label,
            targets.then_term,
            targets.else_label,
            targets.else_term,
        )
    } else {
        (
            targets.else_label,
            targets.else_term,
            targets.then_label,
            targets.then_term,
        )
    };

    let mut hot_local_env = local_env.clone();
    emit_opt_v3_scalar_thread_inline_return_arm(
        fb,
        hot_branch,
        hot_label,
        hot_term,
        unmaterialized_location,
        &mut hot_local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
    .map_err(|err| {
        format!("optimizer v3 scalar-thread inline return from block {source_label}: {err}")
    })?;

    let mut cold_local_env = local_env.clone();
    emit_opt_v3_scalar_thread_inline_return_arm(
        fb,
        cold_branch,
        cold_label,
        cold_term,
        unmaterialized_location,
        &mut cold_local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
    .map_err(|err| {
        format!("optimizer v3 scalar-thread inline return from block {source_label}: {err}")
    })
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn emit_opt_v3_scalar_thread_inline_return_arm(
    fb: &mut FunctionBuilder<'_>,
    branch_block: ir::Block,
    target_label: BlockLabel,
    target_term: &BlockTerm<InstrCodegen>,
    unmaterialized_location: Option<LocalLocation>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    fb.switch_to_block(branch_block);
    let BlockTerm::Return(value) = target_term else {
        return Err(format!(
            "target block {target_label} is no longer a return block"
        ));
    };
    let ret_value = emit_codegen_expr_with_local_env(
        fb,
        value,
        local_env,
        emit_ctx,
        false,
        codegen_env,
        func_imports,
    );
    let unmaterialized_locations = unmaterialized_location.into_iter().collect::<HashSet<_>>();
    emit_codegen_return_pyobject_with_unmaterialized_locals(
        fb,
        target_label,
        ret_value,
        local_env,
        emit_ctx,
        None,
        &unmaterialized_locations,
    )
}

fn typed_exact_int_scalar_thread_selection(
    plan: &TypedExactIntScalarThreadPlan,
    producer_source: InstrId,
    consumer_source: InstrId,
) -> Result<Option<OptV3ScalarThreadSelection<'_>>, String> {
    if plan.producer_instr_id != producer_source || plan.consumer_instr_id != consumer_source {
        return Ok(None);
    }
    if !matches!(
        plan.thread.materialization,
        soac_opt::plan_v3::ScalarThreadMaterialization::DeferredUntilPythonObjectUse { .. }
    ) {
        return Err(format!(
            "optimizer v3 scalar thread for local {} has materialization unsupported by current mechanical lowering: {:?}",
            plan.thread.local.name, plan.thread.materialization
        ));
    }
    Ok(Some(OptV3ScalarThreadSelection {
        thread: &plan.thread,
        producer: OptV3ExactIntReturnSelection {
            hot_plan: &plan.producer_hot_plan,
            hot_region: &plan.producer_hot_region,
            fallback_plan: &plan.producer_fallback_plan,
            fallback_region: &plan.producer_fallback_region,
        },
        consumer: OptV3ExactIntBranchSelection {
            hot_plan: &plan.consumer_hot_plan,
            hot_region: &plan.consumer_hot_region,
            fallback_plan: &plan.consumer_fallback_plan,
            fallback_region: &plan.consumer_fallback_region,
        },
    }))
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn emit_opt_v3_scalar_threaded_store_branch(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    typed_block: &TypedBlock,
    typed_function: &BlockPyFunction<TypedCodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
    jit_local_plan: &PlannedJitFunctionLocals,
    exec_blocks: &[ir::Block],
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    jump_edge_transports: &[Option<EdgeTransportPlan>],
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    cleanup_null_block: ir::Block,
    pending_local_failure_cleanups: &mut Vec<PendingLocalFailureCleanup>,
    local_failure_cleanup_blocks: &mut HashMap<LocalFailureCleanupKey, ir::Block>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
    current_exception_name: Option<&str>,
) -> Result<Option<Vec<BlockLabel>>, String> {
    let [store_expr] = typed_block.body.as_slice() else {
        return Ok(None);
    };
    let InstrTyped::Store(store) = store_expr else {
        return Ok(None);
    };
    let Some(location) = store.name.local_location() else {
        return Ok(None);
    };
    let BlockTerm::Jump(edge) = &typed_block.term else {
        return Ok(None);
    };
    if !edge.args.is_empty() {
        return Ok(None);
    }
    if codegen_block_has_predecessor(function, edge.target) != 1 {
        return Ok(None);
    }
    if current_exception_name.is_some() || block_exception_name(function, edge.target).is_some() {
        return Ok(None);
    }

    let consumer_index =
        codegen_block_index_for_label(function, block_indices_by_label, edge.target)?;
    let consumer_block = &typed_function.blocks[consumer_index];
    if !consumer_block.body.is_empty() {
        return Ok(None);
    }
    let BlockTerm::IfTerm(if_term) = &consumer_block.term else {
        return Ok(None);
    };
    let Some(producer_source) = store.value.try_semantic_instr_id() else {
        return Ok(None);
    };
    let Some(consumer_source) = if_term.test.try_semantic_instr_id() else {
        return Ok(None);
    };
    let Some(plan) = store.extra().exact_int_scalar_thread_plan() else {
        return Ok(None);
    };
    let Some(selection) =
        typed_exact_int_scalar_thread_selection(plan, producer_source, consumer_source)?
    else {
        return Ok(None);
    };

    if let Some(store_instr_id) = store_expr.try_semantic_instr_id() {
        emit_ctx.require_deopt_point_before_instr_id(store_instr_id)?;
    }
    emit_ctx.require_deopt_point_before_term(source_label)?;
    emit_ctx.require_deopt_point_at_block_entry(edge.target)?;
    emit_ctx.require_deopt_point_before_term(edge.target)?;

    let source_index =
        codegen_block_index_for_label(function, block_indices_by_label, source_label)?;
    let source_jump_transport = jump_edge_transports[source_index]
        .as_ref()
        .expect("jump term should have a planned edge transport");
    let layout = emit_ctx
        .storage_layout
        .as_ref()
        .expect("Store local slot should have storage layout during typed codegen");
    let local_name = local_name_for_location(layout, location);
    let local_already_bound = local_env
        .entry_index_for_location(location)
        .or_else(|| local_env.entry_index_for_name(local_name))
        .is_some();
    let inline_return_targets = if !local_already_bound && emit_ctx.stack_slots.has_name(local_name)
    {
        opt_v3_scalar_thread_inline_return_targets(
            function,
            block_indices_by_label,
            if_term,
            &store.name,
        )?
    } else {
        None
    };
    let result_block = if inline_return_targets.is_none() {
        let block = fb.create_block();
        fb.append_block_param(block, emit_ctx.consts.i32_ty);
        fb.append_block_param(block, emit_ctx.consts.ptr_ty);
        Some(block)
    } else {
        None
    };
    let producer_fallback_block = fb.create_block();
    fb.set_cold_block(producer_fallback_block);

    let stmt_emit_ctx = local_failure_cleanup_emit_ctx(
        fb,
        emit_ctx,
        local_env,
        cleanup_null_block,
        pending_local_failure_cleanups,
        local_failure_cleanup_blocks,
    )?;
    let stmt_emit_ctx = stmt_emit_ctx.as_ref().unwrap_or(emit_ctx);

    let mut hot_values = opt_v3_region_input_values(
        fb,
        selection.producer.hot_plan,
        local_env,
        stmt_emit_ctx,
        "scalar-thread producer hot region",
    )?;
    emit_opt_v3_mechanical_region_steps_until_value(
        fb,
        selection.producer.hot_region,
        &mut hot_values,
        Some(producer_fallback_block),
        local_env,
        stmt_emit_ctx,
        codegen_env,
        func_imports,
        selection.thread.producer.value,
    )?;
    let threaded_i64 = opt_v3_i64_value(&hot_values, selection.thread.producer.value)?;
    let mut consumer_hot_values = HashMap::new();
    opt_v3_store_mechanical_value(
        &mut consumer_hot_values,
        selection.thread.consumer.value,
        OptV3MechanicalValue::I64(threaded_i64),
    )?;
    emit_opt_v3_mechanical_region_steps_with_preseeded_scalar(
        fb,
        selection.consumer.hot_region,
        &mut consumer_hot_values,
        None,
        local_env,
        stmt_emit_ctx,
        codegen_env,
        func_imports,
        selection.thread.consumer.value,
    )?;
    let hot_condition = opt_v3_region_branch_condition(
        selection.consumer.hot_region,
        &consumer_hot_values,
        consumer_source,
    )?;
    if let Some(inline_return_targets) = inline_return_targets {
        let mut hot_return_env = local_env.clone();
        emit_opt_v3_scalar_thread_inline_return_branch(
            fb,
            edge.target,
            Some(consumer_source),
            hot_condition,
            opt_v3_scalar_thread_unmaterialized_local_location(selection.thread)?,
            inline_return_targets,
            &mut hot_return_env,
            stmt_emit_ctx,
            codegen_env,
            func_imports,
        )?;
    } else {
        let result_block =
            result_block.expect("non-inline scalar thread path should have a result block");
        let hot_c = emit_checked_owned_pyobject_call_with_cleanup(
            fb,
            stmt_emit_ctx,
            stmt_emit_ctx.py_long_from_i64_ref,
            &[threaded_i64],
            &[],
        );
        fb.ins().jump(
            result_block,
            &[
                ir::BlockArg::Value(hot_condition),
                ir::BlockArg::Value(hot_c),
            ],
        );
    }

    fb.switch_to_block(producer_fallback_block);
    let mut fallback_env = local_env.clone();
    let producer_fallback_emit_ctx = local_failure_cleanup_emit_ctx(
        fb,
        emit_ctx,
        &fallback_env,
        cleanup_null_block,
        pending_local_failure_cleanups,
        local_failure_cleanup_blocks,
    )?;
    let producer_fallback_emit_ctx = producer_fallback_emit_ctx.as_ref().unwrap_or(emit_ctx);
    let mut producer_fallback_values = opt_v3_region_input_values(
        fb,
        selection.producer.fallback_plan,
        &mut fallback_env,
        producer_fallback_emit_ctx,
        "scalar-thread producer fallback region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.producer.fallback_region,
        &mut producer_fallback_values,
        None,
        &mut fallback_env,
        producer_fallback_emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let fallback_c_result = opt_v3_region_return_pyobject(
        selection.producer.fallback_region,
        &producer_fallback_values,
        producer_source,
    )?;
    if !fallback_c_result
        .ownership
        .can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED)
    {
        return Err(format!(
            "optimizer v3 scalar-thread fallback producer for {producer_source} produced {:?}, expected owned PyObject",
            fallback_c_result.ownership
        ));
    }
    fallback_env.store_location(
        fb,
        location,
        local_name,
        fallback_c_result.value,
        LocalRefKind::Owned,
        Some(PyObjFacts::unknown()),
        emit_ctx.allow_local_only_slot_backed_stores,
        &emit_ctx.stack_slots,
        emit_ctx.consts.ptr_ty,
        emit_ctx.consts.thread_state_value,
        emit_ctx.incref_ref,
        emit_ctx.decref_ref,
    );
    let consumer_fallback_emit_ctx = local_failure_cleanup_emit_ctx(
        fb,
        emit_ctx,
        &fallback_env,
        cleanup_null_block,
        pending_local_failure_cleanups,
        local_failure_cleanup_blocks,
    )?;
    let consumer_fallback_emit_ctx = consumer_fallback_emit_ctx.as_ref().unwrap_or(emit_ctx);
    let mut consumer_fallback_values = opt_v3_region_input_values(
        fb,
        selection.consumer.fallback_plan,
        &mut fallback_env,
        consumer_fallback_emit_ctx,
        "scalar-thread consumer fallback region",
    )?;
    emit_opt_v3_mechanical_region_steps(
        fb,
        selection.consumer.fallback_region,
        &mut consumer_fallback_values,
        None,
        &mut fallback_env,
        consumer_fallback_emit_ctx,
        codegen_env,
        func_imports,
    )?;
    let fallback_condition = opt_v3_region_branch_condition(
        selection.consumer.fallback_region,
        &consumer_fallback_values,
        consumer_source,
    )?;
    if let Some(inline_return_targets) = inline_return_targets {
        emit_opt_v3_scalar_thread_inline_return_branch(
            fb,
            edge.target,
            Some(consumer_source),
            fallback_condition,
            None,
            inline_return_targets,
            &mut fallback_env,
            consumer_fallback_emit_ctx,
            codegen_env,
            func_imports,
        )?;
        return Ok(Some(vec![
            edge.target,
            if_term.then_label,
            if_term.else_label,
        ]));
    }
    let fallback_c = fallback_env
        .load_location(fb, location, local_name, consumer_fallback_emit_ctx, true)
        .ok_or_else(|| {
            format!("optimizer v3 scalar-thread fallback lost materialized local {local_name}")
        })?;
    let result_block =
        result_block.expect("non-inline scalar thread path should have result block");
    fb.ins().jump(
        result_block,
        &[
            ir::BlockArg::Value(fallback_condition),
            ir::BlockArg::Value(fallback_c),
        ],
    );

    fb.switch_to_block(result_block);
    let result_params = fb.block_params(result_block).to_vec();
    let condition = result_params[0];
    let c_value = result_params[1];
    local_env.store_location(
        fb,
        location,
        local_name,
        c_value,
        LocalRefKind::Owned,
        Some(PyObjFacts::unknown()),
        emit_ctx.allow_local_only_slot_backed_stores,
        &emit_ctx.stack_slots,
        emit_ctx.consts.ptr_ty,
        emit_ctx.consts.thread_state_value,
        emit_ctx.incref_ref,
        emit_ctx.decref_ref,
    );
    let (prepared_args, forwarded_locations) = emit_planned_target_args_codegen_from_local_env(
        fb,
        &source_jump_transport.target_args,
        local_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )
    .map_err(|err| {
        format!(
            "missing local mapping for fused optimizer v3 scalar-thread jump from block {source_label}: {err}"
        )
    })?;
    emit_planned_local_releases_for_reason_with_local_env(
        fb,
        source_label,
        &RefcountReleaseReason::Jump {
            target: edge.target,
        },
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
    );
    let consumer_arg_values = cranelift_value_args(&prepared_args)?;
    let mut consumer_env = LocalEnv::default();
    bind_planned_local_env_at_block_entry(
        fb,
        jit_local_plan,
        consumer_index,
        &consumer_arg_values,
        &mut consumer_env,
        &emit_ctx.stack_slots,
        emit_ctx.consts.ptr_ty,
        emit_ctx.consts.thread_state_value,
        emit_ctx.incref_ref,
        emit_ctx.decref_ref,
        matches!(function.kind, FunctionKind::Function),
    )?;
    emit_codegen_if_truth_i32(
        fb,
        edge.target,
        Some(consumer_source),
        condition,
        if_term.then_label,
        if_term.else_label,
        None,
        function,
        exec_blocks,
        block_indices_by_label,
        implicit_target_transports,
        &mut consumer_env,
        emit_ctx,
        codegen_env,
        func_imports,
    )?;
    Ok(Some(vec![edge.target]))
}

fn emit_codegen_if_target_arm(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    arm_name: &str,
    branch_block: ir::Block,
    target_label: BlockLabel,
    target_exception_name: Option<&str>,
    release_reason: RefcountReleaseReason,
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

#[allow(clippy::too_many_arguments)]
fn emit_codegen_if_truth_i32(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    test_instr_id: Option<InstrId>,
    truth_i32: ir::Value,
    then_label: BlockLabel,
    else_label: BlockLabel,
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
    fb.ins().brif(hot_cond, hot_branch, &[], cold_branch, &[]);

    let (hot_name, hot_label, cold_name, cold_label) = if prefer_true {
        ("then", then_label, "else", else_label)
    } else {
        ("else", else_label, "then", then_label)
    };
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
    let mut cold_local_env = local_env.clone();
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
    );
    emit_pop_handled_exception_if_leaving(fb, current_exception_name, None, emit_ctx);
    fb.ins().return_(&[ret_value]);
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

    let cause_value = emit_none_const(fb, emit_ctx);
    fb.ins().call(emit_ctx.incref_ref, &[cause_value]);
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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
        return emit_codegen_if_truth_i32(
            fb,
            source_label,
            test_instr_id,
            truth_i32,
            if_term.then_label,
            if_term.else_label,
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
            .unwrap_or(ResultDemand::PYOBJECT_OWNED);
        let result = match demand {
            ResultDemand::PyObject { borrowed_ok: false } => {
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
                    "typed return value requires owned PyObject demand, got {other:?}"
                ));
            }
        };
        let (ret_value, ownership, _) = result.expect_pyobject("typed return value");
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
                .unwrap_or(ResultDemand::PYOBJECT_OWNED);
            let result = match demand {
                ResultDemand::PyObject { borrowed_ok: false } => {
                    emit_typed_codegen_stmt_result_with_local_env(
                        fb,
                        exc_expr,
                        local_env,
                        emit_ctx,
                        demand,
                        codegen_env,
                        func_imports,
                    )?
                }
                other => {
                    return Err(format!(
                        "typed raise exception requires owned PyObject demand, got {other:?}"
                    ));
                }
            };
            let (exc_value, ownership, _) = result.expect_pyobject("typed raise exception");
            if !ownership.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED) {
                return Err(format!(
                    "typed raise exception produced {ownership:?}, but raise requires owned PyObject"
                ));
            }
            (exc_value, ownership)
        } else {
            let none_const = emit_none_const(fb, emit_ctx);
            fb.ins().call(emit_ctx.incref_ref, &[none_const]);
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

fn new_jit_builder(env_config: &SoacEnvConfig) -> Result<JITBuilder, String> {
    let isa = CraneliftTargetConfig::runtime(env_config).build_isa()?;
    let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    if let Ok(provider) = ArenaMemoryProvider::new_with_size(JIT_ARENA_BYTES) {
        builder.memory_provider(Box::new(provider));
    }
    register_jit_builder_symbols(&mut builder);
    Ok(builder)
}

fn register_jit_builder_symbols(builder: &mut JITBuilder) {
    builder.symbol("_Py_Dealloc", py_dealloc_symbol());
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Function),
        std::ptr::addr_of_mut!(PyFunction_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Method),
        std::ptr::addr_of_mut!(PyMethod_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Type),
        std::ptr::addr_of_mut!(PyType_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Long),
        std::ptr::addr_of_mut!(PyLong_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::List),
        std::ptr::addr_of_mut!(PyList_Type).cast::<u8>(),
    );
    builder.symbol(
        "_PyDict_IndexedValueTombstone",
        std::ptr::addr_of_mut!(_PyDict_IndexedValueTombstone).cast::<u8>(),
    );
    builder.symbol(
        SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL,
        soac_jit_make_function_with_closure as *const u8,
    );
    builder.symbol_lookup_fn(Box::new(lookup_registered_jit_data_symbol));
    register_specialized_jit_symbols(builder);
}

fn new_jit_module(compile_session: &crate::session::CompileSession) -> Result<JITModule, String> {
    let env_config = compile_session.env_config()?;
    let mut jit_module = JITModule::new(new_jit_builder(env_config)?);
    load_runtime_support_clif(&mut jit_module, env_config)?;
    Ok(jit_module)
}

#[derive(Debug)]
struct DefinedFunctionArtifact {
    code_size: usize,
    code_bb_offsets: Vec<usize>,
    code_bb_edges: Vec<(usize, usize)>,
    systemv_unwind_info: Option<cranelift_codegen::isa::unwind::systemv::UnwindInfo>,
}

#[derive(Clone)]
pub(super) struct CompiledFunctionBytes {
    code: Vec<u8>,
    alignment: u64,
    relocs: Vec<ModuleReloc>,
}

struct CompiledFunctionArtifact {
    bytes: CompiledFunctionBytes,
    artifact: DefinedFunctionArtifact,
}

#[derive(Debug)]
struct TrivialJumpBlock {
    block: ir::Block,
    target: ir::Block,
    params: Vec<ir::Value>,
    jump_args: Vec<ir::BlockArg>,
    predecessors: Vec<TrivialJumpPredecessor>,
    remove_if_unreferenced: bool,
}

#[derive(Debug, Clone, Copy)]
struct TrivialJumpPredecessor {
    block: ir::Block,
    inst: ir::Inst,
}

#[derive(Debug, Default, Clone, Copy)]
struct TrivialJumpNormalizationStats {
    removed_blocks: usize,
    redirected_edges: usize,
}

fn define_prepared_function(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
) -> Result<DefinedFunctionArtifact, String> {
    let compiled = compile_prepared_function_bytes(
        jit_module,
        env_config,
        func_id,
        ctx,
        function_name,
        err_prefix,
    )?;
    define_compiled_function_bytes(jit_module, func_id, &compiled, err_prefix)?;
    Ok(compiled.artifact)
}

fn define_compiled_function_bytes(
    jit_module: &mut JITModule,
    func_id: FuncId,
    compiled: &CompiledFunctionArtifact,
    err_prefix: &str,
) -> Result<(), String> {
    jit_module
        .define_function_bytes(
            func_id,
            compiled.bytes.alignment,
            compiled.bytes.code.as_slice(),
            compiled.bytes.relocs.as_slice(),
        )
        .map_err(|err| format!("{err_prefix}: {err}"))?;
    Ok(())
}

fn compile_prepared_function_bytes(
    codegen_env: &mut impl JitCodegenEnv,
    env_config: &SoacEnvConfig,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
) -> Result<CompiledFunctionArtifact, String> {
    let function_name = if env_config.jit_refcount_emission_enabled() {
        Cow::Borrowed(function_name)
    } else {
        Cow::Owned(format!("{function_name}:refcounts=off"))
    };
    ctx.func.name = stable_cranelift_function_name(function_name.as_ref());
    prepare_cranelift_function_for_backend(codegen_env, env_config, None, ctx, err_prefix)?;
    compile_backend_prepared_function_bytes(codegen_env.codegen_isa(), func_id, ctx, err_prefix)
}

fn compile_prepared_function_bytes_with_isa(
    codegen_env: &mut impl JitCodegenEnv,
    env_config: &SoacEnvConfig,
    isa: &dyn TargetIsa,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
) -> Result<CompiledFunctionArtifact, String> {
    let function_name = if env_config.jit_refcount_emission_enabled() {
        Cow::Borrowed(function_name)
    } else {
        Cow::Owned(format!("{function_name}:refcounts=off"))
    };
    ctx.func.name = stable_cranelift_function_name(function_name.as_ref());
    prepare_cranelift_function_for_backend(codegen_env, env_config, Some(isa), ctx, err_prefix)?;
    compile_backend_prepared_function_bytes(isa, func_id, ctx, err_prefix)
}

fn compile_backend_prepared_function_bytes(
    isa: &dyn TargetIsa,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    err_prefix: &str,
) -> Result<CompiledFunctionArtifact, String> {
    let func_for_relocs = ctx.func.clone();
    let mut ctrl_plane = ControlPlane::default();
    let compiled_stencil = isa
        .compile_function(&ctx.func, &ctx.domtree, false, &mut ctrl_plane)
        .map_err(|err| format!("{err_prefix}: {err:?}"))?;
    let compiled = compiled_stencil.apply_params(&ctx.func.params);
    let (code_bb_offsets, code_bb_edges) = compiled.get_code_bb_layout();
    let alignment = compiled.buffer.alignment as u64;
    let relocs = compiled
        .buffer
        .relocs()
        .iter()
        .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &func_for_relocs, func_id))
        .collect::<Vec<_>>();
    let systemv_unwind_info = compiled
        .create_unwind_info(isa)
        .map_err(|err| format!("{err_prefix}: failed to create unwind info: {err:?}"))?
        .and_then(|unwind_info| match unwind_info {
            cranelift_codegen::isa::unwind::UnwindInfo::SystemV(info) => Some(info),
            _ => None,
        });
    let code = compiled.code_buffer().to_vec();
    Ok(CompiledFunctionArtifact {
        bytes: CompiledFunctionBytes {
            code,
            alignment,
            relocs,
        },
        artifact: DefinedFunctionArtifact {
            code_size: compiled.code_buffer().len(),
            code_bb_offsets,
            code_bb_edges,
            systemv_unwind_info,
        },
    })
}

fn prepare_cranelift_function_for_backend(
    codegen_env: &mut impl JitCodegenEnv,
    env_config: &SoacEnvConfig,
    isa: Option<&dyn TargetIsa>,
    ctx: &mut cranelift_codegen::Context,
    err_prefix: &str,
) -> Result<(), String> {
    inline_runtime_support_calls(codegen_env, env_config, ctx, err_prefix)?;
    let isa = isa.unwrap_or_else(|| codegen_env.codegen_isa());
    let mut ctrl_plane = ControlPlane::default();
    ctx.optimize(isa, &mut ctrl_plane)
        .map_err(|err| format!("{err_prefix}: {err:?}"))?;
    ctx.compute_cfg();
    ctx.compute_domtree();
    ctx.verify_if(isa)
        .map_err(|err| format!("{err_prefix}: post-opt verifier failed: {err:?}"))?;
    Ok(())
}

fn normalize_postopt_clif_for_inspection(func: &mut ir::Function) -> TrivialJumpNormalizationStats {
    let mut stats = TrivialJumpNormalizationStats::default();
    loop {
        let cfg = ControlFlowGraph::with_function(func);
        let value_uses = cranelift_value_use_insts(func);
        let blocks = collect_noncritical_trivial_jump_block_rewrites(func, &cfg, &value_uses);
        if blocks.is_empty() {
            break;
        }
        let redirected_edges = redirect_trivial_jump_block_predecessors(func, &blocks);
        if redirected_edges == 0 {
            break;
        }
        stats.redirected_edges += redirected_edges;
        let cfg = ControlFlowGraph::with_function(func);
        let entry_block = func.layout.blocks().next();
        for block in blocks {
            if !block.remove_if_unreferenced {
                continue;
            }
            if Some(block.block) == entry_block {
                continue;
            }
            if cfg.pred_iter(block.block).next().is_none() {
                stats.removed_blocks += 1;
                remove_block_from_layout(func, block.block);
            }
        }
    }
    stats
}

fn collect_noncritical_trivial_jump_block_rewrites(
    func: &ir::Function,
    cfg: &ControlFlowGraph,
    value_uses: &HashMap<ir::Value, Vec<ir::Inst>>,
) -> Vec<TrivialJumpBlock> {
    let mut rewrites = Vec::new();
    let mut occupied_blocks = HashSet::new();
    for block in func.layout.blocks() {
        let Some((jump_inst, target, jump_args)) = trivial_jump_block_target(func, block) else {
            continue;
        };
        if target == block {
            continue;
        }
        let predecessors = cfg
            .pred_iter(block)
            .map(|pred| TrivialJumpPredecessor {
                block: pred.block,
                inst: pred.inst,
            })
            .collect::<Vec<_>>();
        if predecessors.is_empty() {
            continue;
        }
        let params = func.dfg.block_params(block).to_vec();
        if !trivial_jump_args_are_param_forwards(&jump_args, &params) {
            continue;
        }
        if !trivial_jump_block_params_only_feed_jump(jump_inst, &params, value_uses) {
            continue;
        }
        if func.dfg.block_params(target).len() != jump_args.len() {
            continue;
        }

        if predecessors.len() == 1 && predecessors[0].block != target {
            if !trivial_jump_block_edges_are_noncritical(cfg, block, target, &predecessors) {
                continue;
            }
            if predecessors.iter().any(|pred| {
                predecessor_forward_rewrites(func, pred.inst, block, target, &params, &jump_args)
                    .is_none()
            }) {
                continue;
            }
            let involved_blocks = std::iter::once(block)
                .chain(std::iter::once(target))
                .chain(predecessors.iter().map(|pred| pred.block))
                .collect::<Vec<_>>();
            if involved_blocks
                .iter()
                .any(|block| occupied_blocks.contains(block))
            {
                continue;
            }
            occupied_blocks.extend(involved_blocks);
            rewrites.push(TrivialJumpBlock {
                block,
                target,
                params,
                jump_args,
                predecessors,
                remove_if_unreferenced: true,
            });
            continue;
        }

        let final_target_pred_count =
            trivial_jump_final_target_pred_count(cfg, block, target, &predecessors);
        let rewritable_predecessors = predecessors
            .iter()
            .filter(|pred| pred.block != target)
            .filter(|pred| func.dfg.insts[pred.inst].opcode() == ir::Opcode::Jump)
            .filter(|pred| trivial_jump_block_target(func, pred.block).is_some())
            .filter(|pred| {
                trivial_jump_predecessor_edge_is_noncritical(
                    cfg,
                    block,
                    target,
                    pred,
                    final_target_pred_count,
                )
            })
            .filter(|pred| {
                predecessor_forward_rewrites(func, pred.inst, block, target, &params, &jump_args)
                    .is_some()
            })
            .copied()
            .collect::<Vec<_>>();
        if !rewritable_predecessors.is_empty() && rewritable_predecessors.len() < predecessors.len()
        {
            rewrites.push(TrivialJumpBlock {
                block,
                target,
                params,
                jump_args,
                predecessors: rewritable_predecessors,
                remove_if_unreferenced: false,
            });
        }
    }
    rewrites
}

fn trivial_jump_args_are_param_forwards(jump_args: &[ir::BlockArg], params: &[ir::Value]) -> bool {
    let params = params.iter().copied().collect::<HashSet<_>>();
    jump_args.iter().all(|arg| match arg {
        ir::BlockArg::Value(value) => params.contains(value),
        ir::BlockArg::TryCallRet(_) | ir::BlockArg::TryCallExn(_) => false,
    })
}

fn trivial_jump_block_target(
    func: &ir::Function,
    block: ir::Block,
) -> Option<(ir::Inst, ir::Block, Vec<ir::BlockArg>)> {
    let insts = func.layout.block_insts(block).collect::<Vec<_>>();
    let (last, prefix) = insts.split_last()?;
    if prefix
        .iter()
        .any(|inst| func.dfg.insts[*inst].opcode() != ir::Opcode::Nop)
    {
        return None;
    }
    if func.dfg.insts[*last].opcode() != ir::Opcode::Jump {
        return None;
    }
    let destinations =
        func.dfg.insts[*last].branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables);
    let destination = destinations.first()?;
    if destinations.len() != 1 {
        return None;
    }
    Some((
        *last,
        destination.block(&func.dfg.value_lists),
        destination.args(&func.dfg.value_lists).collect(),
    ))
}

fn cranelift_value_use_insts(func: &ir::Function) -> HashMap<ir::Value, Vec<ir::Inst>> {
    let mut uses: HashMap<ir::Value, Vec<ir::Inst>> = HashMap::new();
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            let mut inst_values = Vec::new();
            for value in func.dfg.inst_args(inst) {
                if !inst_values.contains(value) {
                    inst_values.push(*value);
                }
            }
            let destinations = func.dfg.insts[inst]
                .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables);
            for destination in destinations {
                for arg in destination.args(&func.dfg.value_lists) {
                    let ir::BlockArg::Value(value) = arg else {
                        continue;
                    };
                    if !inst_values.contains(&value) {
                        inst_values.push(value);
                    }
                }
            }
            for value in inst_values {
                uses.entry(value).or_default().push(inst);
            }
        }
    }
    uses
}

fn trivial_jump_block_params_only_feed_jump(
    jump_inst: ir::Inst,
    params: &[ir::Value],
    value_uses: &HashMap<ir::Value, Vec<ir::Inst>>,
) -> bool {
    params.iter().all(|param| {
        value_uses
            .get(param)
            .is_none_or(|uses| uses.iter().all(|inst| *inst == jump_inst))
    })
}

fn trivial_jump_block_edges_are_noncritical(
    cfg: &ControlFlowGraph,
    block: ir::Block,
    target: ir::Block,
    predecessors: &[TrivialJumpPredecessor],
) -> bool {
    let final_target_pred_count =
        trivial_jump_final_target_pred_count(cfg, block, target, predecessors);
    predecessors.iter().all(|pred| {
        trivial_jump_predecessor_edge_is_noncritical(
            cfg,
            block,
            target,
            pred,
            final_target_pred_count,
        )
    })
}

fn trivial_jump_final_target_pred_count(
    cfg: &ControlFlowGraph,
    block: ir::Block,
    target: ir::Block,
    predecessors: &[TrivialJumpPredecessor],
) -> usize {
    cfg.pred_iter(target)
        .map(|pred| pred.block)
        .filter(|pred| *pred != block)
        .chain(predecessors.iter().map(|pred| pred.block))
        .collect::<HashSet<_>>()
        .len()
}

fn trivial_jump_predecessor_edge_is_noncritical(
    cfg: &ControlFlowGraph,
    block: ir::Block,
    target: ir::Block,
    predecessor: &TrivialJumpPredecessor,
    final_target_pred_count: usize,
) -> bool {
    let mut final_pred_successors = cfg.succ_iter(predecessor.block).collect::<HashSet<_>>();
    final_pred_successors.remove(&block);
    final_pred_successors.insert(target);
    final_pred_successors.len() <= 1 || final_target_pred_count <= 1
}

fn predecessor_forward_rewrites(
    func: &ir::Function,
    pred_inst: ir::Inst,
    block: ir::Block,
    target: ir::Block,
    params: &[ir::Value],
    jump_args: &[ir::BlockArg],
) -> Option<Vec<(usize, Vec<ir::BlockArg>)>> {
    let mut rewrites = Vec::new();
    let destinations = func.dfg.insts[pred_inst]
        .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables);
    for (index, destination) in destinations.iter().enumerate() {
        if destination.block(&func.dfg.value_lists) == block {
            let incoming_args = destination.args(&func.dfg.value_lists).collect::<Vec<_>>();
            let forwarded = compose_forwarded_block_args(&incoming_args, params, jump_args)?;
            if func.dfg.block_params(target).len() != forwarded.len() {
                return None;
            }
            rewrites.push((index, forwarded));
        }
    }
    (!rewrites.is_empty()).then_some(rewrites)
}

fn compose_forwarded_block_args(
    incoming_args: &[ir::BlockArg],
    params: &[ir::Value],
    jump_args: &[ir::BlockArg],
) -> Option<Vec<ir::BlockArg>> {
    if incoming_args.len() != params.len() {
        return None;
    }
    let param_args = params
        .iter()
        .copied()
        .zip(incoming_args.iter().copied())
        .collect::<HashMap<_, _>>();
    Some(
        jump_args
            .iter()
            .map(|arg| match arg {
                ir::BlockArg::Value(value) => param_args.get(value).copied().unwrap_or(*arg),
                ir::BlockArg::TryCallRet(_) | ir::BlockArg::TryCallExn(_) => *arg,
            })
            .collect(),
    )
}

fn redirect_trivial_jump_block_predecessors(
    func: &mut ir::Function,
    blocks: &[TrivialJumpBlock],
) -> usize {
    let mut changed = 0;
    for block in blocks {
        for predecessor in &block.predecessors {
            let Some(rewrites) = predecessor_forward_rewrites(
                func,
                predecessor.inst,
                block.block,
                block.target,
                &block.params,
                &block.jump_args,
            ) else {
                continue;
            };
            let new_calls = rewrites
                .into_iter()
                .map(|(index, args)| {
                    (
                        index,
                        ir::BlockCall::new(block.target, args, &mut func.dfg.value_lists),
                    )
                })
                .collect::<Vec<_>>();
            let dfg = &mut func.dfg;
            let destinations = dfg.insts[predecessor.inst]
                .branch_destination_mut(&mut dfg.jump_tables, &mut dfg.exception_tables);
            for (index, destination) in new_calls {
                if destinations[index].block(&dfg.value_lists) == block.block {
                    destinations[index] = destination;
                    changed += 1;
                }
            }
        }
    }
    changed
}

fn remove_block_from_layout(func: &mut ir::Function, block: ir::Block) {
    let insts = func.layout.block_insts(block).collect::<Vec<_>>();
    for inst in insts {
        func.layout.remove_inst(inst);
    }
    func.layout.remove_block(block);
}

fn stable_cranelift_function_name(function_name: &str) -> ir::UserFuncName {
    let hash = stable_cranelift_function_hash(function_name.as_bytes());
    ir::UserFuncName::user((hash >> 32) as u32, hash as u32)
}

fn stable_cranelift_function_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn record_jit_bb_map(
    env_config: &SoacEnvConfig,
    symbol: &str,
    code_id: u64,
    artifact: &DefinedFunctionArtifact,
    function_id: RuntimeFunctionId,
    function_qualname: &str,
    entry_kind: &str,
) {
    let Some(dir) = env_config.soac_work_dir() else {
        return;
    };
    let path = dir.join("jit-bb-map.jsonl");
    let record = serde_json::json!({
        "process_id": std::process::id(),
        "code_id": code_id,
        "symbol": symbol,
        "code_size": artifact.code_size,
        "function_id": format!("{function_id}"),
        "function_qualname": function_qualname,
        "entry_kind": entry_kind,
        "bb_offsets": &artifact.code_bb_offsets,
        "bb_edges": &artifact.code_bb_edges,
    });
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| format!("failed to open {}: {err}", path.display()))?;
        use std::io::Write;
        serde_json::to_writer(&mut file, &record)
            .map_err(|err| format!("failed to serialize {}: {err}", path.display()))?;
        file.write_all(b"\n")
            .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
        Ok(())
    })();
    if let Err(err) = result {
        eprintln!("[soac jit bb map] {err}");
    }
}

fn register_jit_signal_diagnostics(
    symbol: &str,
    code_ptr: *const u8,
    artifact: &DefinedFunctionArtifact,
    function_id: RuntimeFunctionId,
    function_qualname: &str,
    entry_kind: &str,
) {
    signal_diagnostics::register_jit_code_range(
        symbol,
        code_ptr,
        artifact.code_size,
        function_id,
        function_qualname,
        entry_kind,
        &artifact.code_bb_offsets,
    );
}

const RUNTIME_SUPPORT_INLINE_MAX_INSTS: usize = 128;
const SOAC_RUNTIME_EXAMPLE_SYMBOL_PREFIX: &str = "soac_runtime_example_";

#[derive(Debug)]
struct RuntimeSupportInliner {
    inlineable: HashMap<ir::UserExternalName, ir::Function>,
}

impl RuntimeSupportInliner {
    fn for_module(
        codegen_env: &mut impl JitCodegenEnv,
        env_config: &SoacEnvConfig,
    ) -> Result<Self, String> {
        let library = runtime_support_library()?;
        let local_runtime_symbols = runtime_support_local_symbols(&library);
        let mut import_func_ids = HashMap::new();
        let mut import_data_ids = HashMap::new();
        let mut local_func_ids = HashMap::new();
        let mut inlineable = HashMap::new();
        for parsed in &library.functions {
            if !matches!(
                parsed.symbol.as_str(),
                SOAC_RUNTIME_INCREF_SYMBOL
                    | SOAC_RUNTIME_DECREF_SYMBOL
                    | SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL
                    | SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL
                    | SOAC_RUNTIME_STORE_GLOBAL_SYMBOL
                    | SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL
                    | SOAC_RUNTIME_PROBE_FIELD_INDEXED_SYMBOL
                    | SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL
                    | SOAC_RUNTIME_TUPLE_NEW_SYMBOL
                    | SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_SYMBOL
            ) {
                continue;
            }
            let func_id = declare_runtime_clif_local_function(
                codegen_env,
                &mut local_func_ids,
                &parsed.symbol,
                &parsed.function.signature,
                "inlineable runtime CLIF function",
            )?;
            let mut function = if should_inline_refcount_as_noop(env_config, parsed.symbol.as_str())
            {
                build_noop_runtime_support_function(func_id, &parsed.function.signature)
            } else {
                parsed.function.clone()
            };
            remap_runtime_clif_extern_user_names(
                codegen_env,
                &mut function,
                &parsed.extern_symbols,
                &parsed.runtime_function_symbols,
                &local_runtime_symbols,
                &parsed.global_extern_symbols,
                &mut import_func_ids,
                &mut local_func_ids,
                &mut import_data_ids,
            )?;
            if function.dfg.num_insts() > RUNTIME_SUPPORT_INLINE_MAX_INSTS {
                continue;
            }
            inlineable.insert(ir::UserExternalName::new(0, func_id.as_u32()), function);
        }
        Ok(Self { inlineable })
    }
}

fn should_inline_refcount_as_noop(env_config: &SoacEnvConfig, symbol: &str) -> bool {
    !env_config.jit_refcount_emission_enabled()
        && matches!(
            symbol,
            SOAC_RUNTIME_INCREF_SYMBOL | SOAC_RUNTIME_DECREF_SYMBOL
        )
}

fn build_noop_runtime_support_function(func_id: FuncId, signature: &ir::Signature) -> ir::Function {
    let mut function = ir::Function::with_name_signature(
        ir::UserFuncName::user(0, func_id.as_u32()),
        signature.clone(),
    );
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut function, &mut builder_ctx);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);
        fb.ins().return_(&[]);
        fb.finalize();
    }
    function
}

impl Inline for RuntimeSupportInliner {
    fn inline(
        &mut self,
        caller: &ir::Function,
        _call_inst: ir::Inst,
        _call_opcode: ir::Opcode,
        callee: ir::FuncRef,
        _call_args: &[ir::Value],
    ) -> InlineCommand<'_> {
        let ext_func = &caller.dfg.ext_funcs[callee];
        let ir::ExternalName::User(name_ref) = &ext_func.name else {
            return InlineCommand::KeepCall;
        };
        let user_name = caller.params.user_named_funcs()[*name_ref].clone();
        let Some(callee_func) = self.inlineable.get(&user_name) else {
            return InlineCommand::KeepCall;
        };
        InlineCommand::Inline {
            callee: Cow::Borrowed(callee_func),
            // We only want to splice these tiny refcount helpers into the caller.
            visit_callee: false,
        }
    }
}

fn inline_runtime_support_calls(
    codegen_env: &mut impl JitCodegenEnv,
    env_config: &SoacEnvConfig,
    ctx: &mut cranelift_codegen::Context,
    err_prefix: &str,
) -> Result<bool, String> {
    let mut inliner = RuntimeSupportInliner::for_module(codegen_env, env_config)?;
    ctx.inline(&mut inliner)
        .map_err(|err| format!("{err_prefix}: failed to inline runtime support calls: {err:?}"))
}

fn lower_static_signature(
    codegen_env: &impl JitCodegenEnv,
    signature: StaticSignature,
) -> ir::Signature {
    let mut lowered = codegen_env.codegen_make_signature();
    let lower_sig_type = |sig_type| match sig_type {
        SigType::Pointer => codegen_env.codegen_target_config().pointer_type(),
        SigType::I64 => ir::types::I64,
        SigType::I32 => ir::types::I32,
    };
    for param in signature.params {
        lowered
            .params
            .push(ir::AbiParam::new(lower_sig_type(*param)));
    }
    for ret in signature.returns {
        lowered
            .returns
            .push(ir::AbiParam::new(lower_sig_type(*ret)));
    }
    lowered
}

fn declare_import_fn(
    codegen_env: &mut impl JitCodegenEnv,
    symbol: &str,
    sig: &ir::Signature,
) -> Result<FuncId, String> {
    codegen_env
        .codegen_declare_function(symbol, Linkage::Import, sig)
        .map_err(|err| format!("failed to declare imported {symbol} symbol: {err}"))
}

fn declare_local_fn(
    codegen_env: &mut impl JitCodegenEnv,
    symbol: &str,
    sig: &ir::Signature,
) -> Result<FuncId, String> {
    codegen_env
        .codegen_declare_function(symbol, Linkage::Local, sig)
        .map_err(|err| format!("failed to declare local {symbol} function: {err}"))
}

fn make_direct_function_signature(
    codegen_env: &impl JitCodegenEnv,
    function: &BlockPyFunction<impl ModuleShape>,
) -> ir::Signature {
    let ptr_ty = codegen_env.codegen_target_config().pointer_type();
    let mut sig = codegen_env.codegen_make_signature();
    sig.params.push(ir::AbiParam::new(ptr_ty));
    sig.params.push(ir::AbiParam::new(ptr_ty));
    for _ in function.params.iter() {
        sig.params.push(ir::AbiParam::new(ptr_ty));
    }
    sig.returns.push(ir::AbiParam::new(ptr_ty));
    sig
}

fn direct_function_symbol(
    function: &BlockPyFunction<impl ModuleShape>,
    symbol_scope: Option<&str>,
) -> String {
    let base =
        jit_python_perf_symbol_name(JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT, &function.names.qualname);
    scoped_jit_symbol(&base, symbol_scope)
}

fn default_direct_function_symbol(
    function: &BlockPyFunction<impl ModuleShape>,
    symbol_scope: Option<&str>,
) -> String {
    let base = format!(
        "{}:defaults",
        jit_python_perf_symbol_name(JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT, &function.names.qualname)
    );
    scoped_jit_symbol(&base, symbol_scope)
}

fn direct_function_symbol_scope(function_id: RuntimeFunctionId, symbol_id: u64) -> String {
    format!("fn_{}_{}", function_id.to_packed_runtime_u64(), symbol_id)
}

fn direct_function_backend_name(
    function: &BlockPyFunction<impl ModuleShape>,
    shared_state: Option<&SharedModuleState>,
) -> String {
    let mut name = String::from("direct:");
    match shared_state {
        Some(shared_state) => push_direct_function_module_identity(
            &mut name,
            shared_state.module_name.as_str(),
            shared_state.source_hash(),
        ),
        None => {
            name.push_str("module_id:");
            name.push_str(
                function
                    .function_id
                    .runtime_module_id()
                    .as_u32()
                    .to_string()
                    .as_str(),
            );
        }
    }
    name.push(':');
    name.push_str(function.names.qualname.as_str());
    name.push(':');
    name.push_str(function.params.len().to_string().as_str());
    name
}

fn push_direct_function_module_identity(out: &mut String, module_name: &str, source_hash: u64) {
    push_symbol_component_hex(out, module_name);
    out.push(':');
    out.push_str(format!("{source_hash:016x}").as_str());
}

fn declare_direct_function(
    codegen_env: &mut impl JitCodegenEnv,
    function: &BlockPyFunction<impl ModuleShape>,
    symbol_scope: Option<&str>,
) -> Result<(ir::Signature, DeclaredJitFunction), String> {
    let sig = make_direct_function_signature(codegen_env, function);
    let symbol = direct_function_symbol(function, symbol_scope);
    let func_id = declare_local_fn(codegen_env, &symbol, &sig)?;
    let (default_func_id, default_symbol) = if function_has_default_resolving_direct_entry(function)
    {
        let default_symbol = default_direct_function_symbol(function, symbol_scope);
        (
            Some(declare_local_fn(codegen_env, &default_symbol, &sig)?),
            Some(default_symbol),
        )
    } else {
        (None, None)
    };
    Ok((
        sig,
        DeclaredJitFunction {
            func_id,
            default_func_id,
            symbol,
            default_symbol,
        },
    ))
}

fn declare_imported_direct_function(
    codegen_env: &mut impl JitCodegenEnv,
    function: &BlockPyFunction<impl ModuleShape>,
    symbol_scope: &str,
) -> Result<DeclaredJitFunction, String> {
    let sig = make_direct_function_signature(codegen_env, function);
    let symbol = direct_function_symbol(function, Some(symbol_scope));
    let func_id = declare_import_fn(codegen_env, &symbol, &sig)?;
    let (default_func_id, default_symbol) = if function_has_default_resolving_direct_entry(function)
    {
        let default_symbol = default_direct_function_symbol(function, Some(symbol_scope));
        (
            Some(declare_import_fn(codegen_env, &default_symbol, &sig)?),
            Some(default_symbol),
        )
    } else {
        (None, None)
    };
    Ok(DeclaredJitFunction {
        func_id,
        default_func_id,
        symbol,
        default_symbol,
    })
}

fn build_default_resolving_direct_adapter(
    codegen_env: &mut impl JitCodegenEnv,
    function: &BlockPyFunction<impl ModuleShape>,
    core_func_id: FuncId,
    adapter_func_id: FuncId,
) -> Result<cranelift_codegen::Context, String> {
    let ptr_ty = codegen_env.codegen_target_config().pointer_type();
    let runtime_layout = FunctionRuntimeDataLayout::from_parts(function, 0);
    let mut module_imports = ModuleFuncImports::new();
    let mut ctx = codegen_env.codegen_make_context();
    ctx.func.signature = make_direct_function_signature(codegen_env, function);
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = fb.create_block();
        fb.append_block_params_for_function_params(entry_block);
        fb.switch_to_block(entry_block);
        fb.seal_block(entry_block);

        let entry_params = fb.block_params(entry_block).to_vec();
        let function_env_value = entry_params[0];
        let thread_state_value = entry_params[1];
        let direct_entry_args = &entry_params[2..];
        let function_data_value = fb.ins().iadd_imm(
            function_env_value,
            i64::from(FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET),
        );
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let raise_missing_ref = FuncBuildImports::new(&mut module_imports).get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_RAISE_MISSING_REQUIRED_ARGUMENT_IMPORT,
        );
        let missing_block = fb.create_block();
        let call_core_block = fb.create_block();
        for _ in function.params.iter() {
            fb.append_block_param(call_core_block, ptr_ty);
        }

        let mut selected_args = Vec::with_capacity(function.params.len());
        for (param_index, (param, arg_value)) in function
            .params
            .iter()
            .zip(direct_entry_args.iter().copied())
            .enumerate()
        {
            let Some(default_slot) =
                param_runtime_default_slot(&runtime_layout, param, param_index)
            else {
                let is_missing = fb
                    .ins()
                    .icmp(ir::condcodes::IntCC::Equal, arg_value, null_ptr);
                let present_block = fb.create_block();
                fb.ins()
                    .brif(is_missing, missing_block, &[], present_block, &[]);
                fb.switch_to_block(present_block);
                selected_args.push(arg_value);
                continue;
            };

            let is_missing = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, arg_value, null_ptr);
            let use_default_block = fb.create_block();
            let use_arg_block = fb.create_block();
            let after_block = fb.create_block();
            fb.append_block_param(after_block, ptr_ty);
            fb.ins()
                .brif(is_missing, use_default_block, &[], use_arg_block, &[]);

            fb.switch_to_block(use_default_block);
            let default_value = emit_function_data_slot_borrowed(
                &mut fb,
                function_data_value,
                default_slot,
                ptr_ty,
            );
            let default_is_missing =
                fb.ins()
                    .icmp(ir::condcodes::IntCC::Equal, default_value, null_ptr);
            let default_ok_block = fb.create_block();
            fb.ins().brif(
                default_is_missing,
                missing_block,
                &[],
                default_ok_block,
                &[],
            );
            fb.switch_to_block(default_ok_block);
            fb.ins()
                .jump(after_block, &[ir::BlockArg::Value(default_value)]);

            fb.switch_to_block(use_arg_block);
            fb.ins()
                .jump(after_block, &[ir::BlockArg::Value(arg_value)]);

            fb.switch_to_block(after_block);
            selected_args.push(fb.block_params(after_block)[0]);
        }
        fb.ins()
            .jump(call_core_block, &block_arg_values(&selected_args));
        fb.seal_block(call_core_block);

        fb.switch_to_block(call_core_block);
        let mut call_args = Vec::with_capacity(function.params.len() + 2);
        call_args.push(function_env_value);
        call_args.push(thread_state_value);
        call_args.extend(fb.block_params(call_core_block).iter().copied());
        let core_func_ref = codegen_env.codegen_declare_func_in_func(core_func_id, &mut fb.func)?;
        let call_inst = fb.ins().call(core_func_ref, &call_args);
        let result = fb.inst_results(call_inst)[0];
        fb.ins().return_(&[result]);

        fb.seal_block(missing_block);
        fb.switch_to_block(missing_block);
        fb.ins().call(raise_missing_ref, &[]);
        fb.ins().return_(&[null_ptr]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    let _ = adapter_func_id;
    Ok(ctx)
}

fn scoped_jit_symbol(base: &str, symbol_scope: Option<&str>) -> String {
    match symbol_scope {
        Some(scope) => format!("{base}:{scope}"),
        None => base.to_string(),
    }
}

fn is_clif_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(crate) const JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT: &str = "d";
pub(crate) const SOAC_RUNTIME_INCREF_SYMBOL: &str = "soac_runtime_incref";
pub(crate) const SOAC_RUNTIME_DECREF_SYMBOL: &str = "soac_runtime_decref";
pub(crate) const SOAC_RUNTIME_INCREF_APPLIED_SYMBOL: &str = "soac_runtime_incref_applied";
pub(crate) const SOAC_RUNTIME_DECREF_APPLIED_SYMBOL: &str = "soac_runtime_decref_applied";
pub(crate) const SOAC_RUNTIME_SET_RAISED_EXCEPTION_SYMBOL: &str =
    "soac_runtime_set_raised_exception";
pub(crate) const SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL: &str = "soac_runtime_load_global";
pub(crate) const SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL: &str =
    "soac_runtime_probe_global_indexed";
pub(crate) const SOAC_RUNTIME_STORE_GLOBAL_SYMBOL: &str = "soac_runtime_store_global";
pub(crate) const SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL: &str =
    "soac_runtime_store_global_indexed";
pub(crate) const SOAC_RUNTIME_PROBE_FIELD_INDEXED_SYMBOL: &str = "soac_runtime_probe_field_indexed";
pub(crate) const SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL: &str = "soac_runtime_store_field_indexed";
pub(crate) const SOAC_RUNTIME_TUPLE_NEW_SYMBOL: &str = "soac_runtime_tuple_new";
pub(crate) const SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_SYMBOL: &str =
    "soac_runtime_tuple_set_item_stolen";
#[cfg(test)]
pub(crate) const SOAC_RUNTIME_PYLONG_AS_I64_SYMBOL: &str = "soac_runtime_pylong_as_i64";
pub(crate) const SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_SYMBOL: &str =
    "soac_runtime_pylong_as_i64_saturating";

pub(crate) fn jit_python_perf_symbol_name(kind: &str, qualname: &str) -> String {
    format!("py:{kind}:{qualname}")
}

fn runtime_support_clif_compatibility_error() -> Option<&'static str> {
    if cfg!(Py_GIL_DISABLED) {
        return Some("runtime CLIF support does not support free-threaded CPython builds");
    }
    if cfg!(py_sys_config = "Py_REF_DEBUG") {
        return Some("runtime CLIF support does not support Py_REF_DEBUG CPython builds");
    }
    if cfg!(py_sys_config = "Py_TRACE_REFS") {
        return Some("runtime CLIF support does not support Py_TRACE_REFS CPython builds");
    }
    None
}

#[derive(Debug)]
struct RuntimeSupportLibrary {
    functions: Vec<ParsedRuntimeClifFunction>,
}

#[derive(Clone, Debug)]
struct ParsedRuntimeClifFunction {
    symbol: String,
    function: ir::Function,
    extern_symbols: HashMap<ir::UserExternalName, String>,
    runtime_function_symbols: HashMap<ir::UserExternalName, String>,
    global_extern_symbols: HashMap<u32, String>,
}

fn parse_runtime_clif_functions() -> Result<Vec<ParsedRuntimeClifFunction>, String> {
    let mut parsed_functions = Vec::new();
    for (symbol, clif_text) in SOAC_JIT_RUNTIME_CLIF {
        let mut functions = parse_functions(clif_text)
            .map_err(|err| format!("failed to parse runtime CLIF for {symbol}: {err}"))?;
        if functions.len() != 1 {
            return Err(format!(
                "expected exactly one runtime CLIF function for {symbol}, found {}",
                functions.len()
            ));
        }
        let function = functions
            .pop()
            .ok_or_else(|| format!("missing parsed runtime CLIF function for {symbol}"))?;
        parsed_functions.push(ParsedRuntimeClifFunction {
            symbol: (*symbol).to_string(),
            function,
            extern_symbols: parse_runtime_clif_extern_symbols(clif_text)?,
            runtime_function_symbols: parse_runtime_clif_runtime_function_symbols(clif_text)?,
            global_extern_symbols: parse_runtime_clif_global_extern_symbols(clif_text)?,
        });
    }
    Ok(parsed_functions)
}

fn parse_runtime_clif_extern_symbols(
    clif_text: &str,
) -> Result<HashMap<ir::UserExternalName, String>, String> {
    let mut extern_symbols = HashMap::new();
    for line in clif_text.lines() {
        if line.trim_start().starts_with(';') {
            continue;
        }
        if !line.contains("::{extern#") {
            continue;
        }
        if !line.contains("Instance {") {
            continue;
        }
        let Some(user_name) = parse_runtime_clif_user_name(line) else {
            return Err(format!(
                "failed to parse user function name from runtime CLIF line: {line}"
            ));
        };
        let Some(symbol) = parse_runtime_clif_extern_symbol(line) else {
            return Err(format!(
                "failed to parse extern symbol from runtime CLIF line: {line}"
            ));
        };
        extern_symbols.insert(user_name, symbol);
    }
    Ok(extern_symbols)
}

fn parse_runtime_clif_runtime_function_symbols(
    clif_text: &str,
) -> Result<HashMap<ir::UserExternalName, String>, String> {
    let mut runtime_symbols = HashMap::new();
    for line in clif_text.lines() {
        if line.trim_start().starts_with(';') {
            continue;
        }
        if !line.contains("Instance {") {
            continue;
        }
        let Some(user_name) = parse_runtime_clif_user_name(line) else {
            return Err(format!(
                "failed to parse user function name from runtime CLIF line: {line}"
            ));
        };
        let Some(symbol) = parse_runtime_clif_instance_symbol(line) else {
            continue;
        };
        if symbol.starts_with("soac_runtime_") {
            runtime_symbols.insert(user_name, symbol);
        }
    }
    Ok(runtime_symbols)
}

fn parse_runtime_clif_global_extern_symbols(
    clif_text: &str,
) -> Result<HashMap<u32, String>, String> {
    let mut extern_symbols = HashMap::new();
    for line in clif_text.lines() {
        if !line.contains("::{extern#") || !line.contains(" = symbol userextname") {
            continue;
        }
        let Some(alias_pos) = line.find("userextname") else {
            return Err(format!(
                "failed to parse user global name from runtime CLIF line: {line}"
            ));
        };
        let alias = &line[(alias_pos + "userextname".len())..];
        let alias_end = alias
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(alias.len());
        let Some(alias) = alias.get(..alias_end) else {
            return Err(format!(
                "failed to parse user global name from runtime CLIF line: {line}"
            ));
        };
        let Ok(alias) = alias.parse::<u32>() else {
            return Err(format!(
                "failed to parse user global name from runtime CLIF line: {line}"
            ));
        };
        let Some(symbol) = parse_runtime_clif_extern_symbol(line) else {
            return Err(format!(
                "failed to parse extern symbol from runtime CLIF line: {line}"
            ));
        };
        extern_symbols.insert(alias, symbol);
    }
    Ok(extern_symbols)
}

fn parse_runtime_clif_user_name(line: &str) -> Option<ir::UserExternalName> {
    let token = line
        .split_whitespace()
        .find(|token| token.starts_with('u') && token.contains(':'))?;
    let rest = token.strip_prefix('u')?;
    let colon = rest.find(':')?;
    let namespace = rest.get(..colon)?.parse().ok()?;
    let rest = rest.get(colon + 1..)?;
    let index_end = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len());
    let index = rest.get(..index_end)?.parse().ok()?;
    Some(ir::UserExternalName::new(namespace, index))
}

fn parse_runtime_clif_extern_symbol(line: &str) -> Option<String> {
    let extern_pos = line.find("::{extern#")?;
    let rest = line.get(extern_pos..)?;
    parse_runtime_clif_instance_symbol(rest)
}

fn parse_runtime_clif_instance_symbol(line: &str) -> Option<String> {
    let symbol = line.rsplit("::").next()?;
    let symbol_end = symbol
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .unwrap_or(symbol.len());
    let symbol = symbol.get(..symbol_end)?;
    if symbol.is_empty() {
        return None;
    }
    Some(symbol.to_string())
}

fn runtime_support_local_symbols(library: &RuntimeSupportLibrary) -> HashSet<String> {
    library
        .functions
        .iter()
        .filter(|parsed| {
            !parsed
                .symbol
                .starts_with(SOAC_RUNTIME_EXAMPLE_SYMBOL_PREFIX)
        })
        .map(|parsed| parsed.symbol.clone())
        .collect()
}

fn declare_runtime_clif_local_function(
    codegen_env: &mut impl JitCodegenEnv,
    local_func_ids: &mut HashMap<String, FuncId>,
    symbol: &str,
    signature: &ir::Signature,
    description: &str,
) -> Result<FuncId, String> {
    if let Some(func_id) = local_func_ids.get(symbol) {
        return Ok(*func_id);
    }
    let func_id = codegen_env
        .codegen_declare_function(symbol, Linkage::Local, signature)
        .map_err(|err| format!("failed to declare {description} {symbol}: {err}"))?;
    local_func_ids.insert(symbol.to_string(), func_id);
    Ok(func_id)
}

fn remap_runtime_clif_extern_user_names(
    codegen_env: &mut impl JitCodegenEnv,
    function: &mut ir::Function,
    extern_symbols: &HashMap<ir::UserExternalName, String>,
    runtime_function_symbols: &HashMap<ir::UserExternalName, String>,
    local_runtime_symbols: &HashSet<String>,
    global_extern_symbols: &HashMap<u32, String>,
    import_func_ids: &mut HashMap<String, FuncId>,
    local_func_ids: &mut HashMap<String, FuncId>,
    import_data_ids: &mut HashMap<String, cranelift_module::DataId>,
) -> Result<(), String> {
    let remaps = function
        .dfg
        .ext_funcs
        .iter()
        .filter_map(|(_, ext_func)| {
            let ir::ExternalName::User(name_ref) = ext_func.name else {
                return None;
            };
            let original_name = function.params.user_named_funcs()[name_ref].clone();
            Some((name_ref, original_name, ext_func.signature))
        })
        .collect::<Vec<_>>();

    for (name_ref, original_name, sig_ref) in remaps {
        let mapped_name = if let Some(symbol) = runtime_function_symbols
            .get(&original_name)
            .filter(|symbol| local_runtime_symbols.contains(*symbol))
        {
            let sig = function.dfg.signatures[sig_ref].clone();
            let local_id = declare_runtime_clif_local_function(
                codegen_env,
                local_func_ids,
                symbol,
                &sig,
                "runtime CLIF local symbol",
            )?;
            ir::UserExternalName::new(0, local_id.as_u32())
        } else if let Some(symbol) = extern_symbols.get(&original_name) {
            let import_id = if let Some(import_id) = import_func_ids.get(symbol) {
                *import_id
            } else {
                let sig = function.dfg.signatures[sig_ref].clone();
                let import_id = codegen_env
                    .codegen_declare_function(symbol, Linkage::Import, &sig)
                    .map_err(|err| {
                        format!("failed to declare runtime CLIF extern symbol {symbol}: {err}")
                    })?;
                import_func_ids.insert(symbol.clone(), import_id);
                import_id
            };
            ir::UserExternalName::new(0, import_id.as_u32())
        } else {
            return Err(format!(
                "unresolved non-extern runtime CLIF user function name {} while loading {}",
                original_name, function.name
            ));
        };
        function.params.reset_user_func_name(name_ref, mapped_name);
    }

    let global_symbol_remaps = function
        .global_values
        .iter()
        .filter_map(|(gv, data)| {
            let ir::GlobalValueData::Symbol {
                name: ir::ExternalName::User(name_ref),
                ..
            } = data
            else {
                return None;
            };
            Some((gv, *name_ref))
        })
        .collect::<Vec<_>>();
    for (gv, name_ref) in global_symbol_remaps {
        let Some(symbol) = global_extern_symbols.get(&name_ref.as_u32()) else {
            continue;
        };
        let import_id = if let Some(import_id) = import_data_ids.get(symbol) {
            *import_id
        } else {
            let import_id = codegen_env
                .codegen_declare_data(symbol, Linkage::Import, false, false)
                .map_err(|err| {
                    format!("failed to declare runtime CLIF extern data symbol {symbol}: {err}")
                })?;
            import_data_ids.insert(symbol.clone(), import_id);
            import_id
        };
        let mapped_name_ref = function
            .declare_imported_user_function(ir::UserExternalName::new(1, import_id.as_u32()));
        if let ir::GlobalValueData::Symbol { name, .. } = &mut function.global_values[gv] {
            *name = ir::ExternalName::User(mapped_name_ref);
        }
    }
    Ok(())
}

fn load_runtime_support_clif(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
) -> Result<(), String> {
    let library = runtime_support_library()?;
    let local_runtime_symbols = runtime_support_local_symbols(&library);
    let mut import_func_ids = HashMap::new();
    let mut import_data_ids = HashMap::new();
    let mut local_func_ids = HashMap::new();
    for parsed in library.functions.iter().cloned() {
        if parsed
            .symbol
            .starts_with(SOAC_RUNTIME_EXAMPLE_SYMBOL_PREFIX)
        {
            continue;
        }
        let func_id = declare_runtime_clif_local_function(
            jit_module,
            &mut local_func_ids,
            &parsed.symbol,
            &parsed.function.signature,
            "runtime CLIF function",
        )?;
        let mut function = parsed.function;
        remap_runtime_clif_extern_user_names(
            jit_module,
            &mut function,
            &parsed.extern_symbols,
            &parsed.runtime_function_symbols,
            &local_runtime_symbols,
            &parsed.global_extern_symbols,
            &mut import_func_ids,
            &mut local_func_ids,
            &mut import_data_ids,
        )?;
        let mut ctx = jit_module.codegen_make_context();
        ctx.func = function;
        let _ = define_prepared_function(
            jit_module,
            env_config,
            func_id,
            &mut ctx,
            &parsed.symbol,
            &format!("failed to define runtime CLIF function {}", parsed.symbol),
        )?;
        jit_module.codegen_clear_context(&mut ctx);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct PrecompileObjectSummary {
    pub output_path: PathBuf,
    pub function_count: usize,
    pub data_object_count: usize,
    pub object_size_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct PrecompileModuleIndexEntry<'a> {
    pub module_name: &'a str,
    pub source_hash: u64,
    pub module: &'a BlockPyModule<CodegenModuleShape>,
}

#[derive(Debug, Clone)]
struct PrecompileIndexedFunction {
    module_name: String,
    source_hash: u64,
    persistent_id: PersistentFunctionId,
    function: BlockPyFunction<CodegenModuleShape>,
}

#[derive(Debug, Clone)]
struct PrecompileIndexedModule {
    module_id: RuntimeModuleId,
}

#[derive(Debug, Clone, Default)]
pub struct PrecompileModuleIndex {
    modules_by_identity: HashMap<(String, u64), PrecompileIndexedModule>,
    functions_by_id: HashMap<RuntimeFunctionId, PrecompileIndexedFunction>,
    ambiguous_function_ids: HashSet<RuntimeFunctionId>,
}

impl PrecompileModuleIndex {
    pub fn from_entries<'a>(
        entries: impl IntoIterator<Item = PrecompileModuleIndexEntry<'a>>,
    ) -> Result<Self, String> {
        let mut index = Self::default();
        for entry in entries {
            index.insert(entry)?;
        }
        Ok(index)
    }

    fn insert(&mut self, entry: PrecompileModuleIndexEntry<'_>) -> Result<(), String> {
        let identity = (entry.module_name.to_string(), entry.source_hash);
        let module_id = entry.module.module_name_gen.runtime_module_id();
        if self
            .modules_by_identity
            .insert(identity.clone(), PrecompileIndexedModule { module_id })
            .is_some()
        {
            return Err(format!(
                "duplicate precompile module identity: module={} source_hash=0x{:016x}",
                entry.module_name, entry.source_hash
            ));
        }
        for function in &entry.module.callable_defs {
            let indexed = PrecompileIndexedFunction {
                module_name: entry.module_name.to_string(),
                source_hash: entry.source_hash,
                persistent_id: persistent_function_id_for_module_function(
                    entry.module_name,
                    entry.source_hash,
                    function.function_id.local_function_id(),
                ),
                function: function.clone(),
            };
            if let Some(previous) = self.functions_by_id.insert(function.function_id, indexed)
                && (previous.module_name != entry.module_name
                    || previous.source_hash != entry.source_hash)
            {
                self.functions_by_id.remove(&function.function_id);
                self.ambiguous_function_ids.insert(function.function_id);
            }
        }
        Ok(())
    }

    fn function_id_for_target(&self, target: &PersistentFunctionId) -> Option<RuntimeFunctionId> {
        let module = self
            .modules_by_identity
            .get(&(target.module.module_name.clone(), target.module.source_hash))?;
        let function_id = RuntimeFunctionId::new(module.module_id, target.local);
        self.function(function_id).map(|_| function_id)
    }

    fn function(&self, function_id: RuntimeFunctionId) -> Option<&PrecompileIndexedFunction> {
        if self.ambiguous_function_ids.contains(&function_id) {
            return None;
        }
        self.functions_by_id.get(&function_id)
    }

    fn precompiled_symbol_scope_for_function(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Option<String> {
        let function = self.function(function_id)?;
        Some(precompiled_direct_function_symbol_scope_for_persistent(
            &function.persistent_id,
        ))
    }
}

fn precompile_external_direct_call_target_functions(
    module: &BlockPyModule<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
    module_index: Option<&PrecompileModuleIndex>,
) -> Result<HashMap<RuntimeFunctionId, BlockPyFunction<TypedCodegenModuleShape>>, String> {
    let Some(module_index) = module_index else {
        return Ok(HashMap::new());
    };
    let current_module_id = module.module_name_gen.module_id();
    let mut target_ids = HashSet::new();
    for function in &module.callable_defs {
        let mut typed_function = function.clone();
        apply_profile_call_emission_plans_to_typed_function(&mut typed_function, profile)?;
        lower_typed_function_call_access_plan_instrs(&mut typed_function);
        target_ids.extend(collect_typed_call_direct_targets(&typed_function));
    }
    Ok(target_ids
        .into_iter()
        .filter(|function_id| function_id.runtime_module_id().as_u32() != current_module_id)
        .filter_map(|function_id| {
            module_index.function(function_id).map(|target| {
                (
                    function_id,
                    lower_codegen_function_to_typed(target.function.clone()),
                )
            })
        })
        .collect())
}

#[derive(Debug, Clone, Copy)]
pub struct PrecompileOptimizationPlanInput<'a> {
    pub v3_path: Option<&'a Path>,
    pub cache_identity: &'a str,
}

fn compile_runtime_support_clif_for_object(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    object_isa: &dyn TargetIsa,
) -> Result<Vec<ObjectFunctionDefinition>, String> {
    let library = runtime_support_library()?;
    let local_runtime_symbols = runtime_support_local_symbols(&library);
    let mut import_func_ids = HashMap::new();
    let mut import_data_ids = HashMap::new();
    let mut local_func_ids = HashMap::new();
    let mut out = Vec::new();
    for parsed in library.functions.iter().cloned() {
        if parsed
            .symbol
            .starts_with(SOAC_RUNTIME_EXAMPLE_SYMBOL_PREFIX)
        {
            continue;
        }
        let func_id = declare_runtime_clif_local_function(
            jit_module,
            &mut local_func_ids,
            &parsed.symbol,
            &parsed.function.signature,
            "runtime CLIF function",
        )?;
        let mut function = parsed.function;
        remap_runtime_clif_extern_user_names(
            jit_module,
            &mut function,
            &parsed.extern_symbols,
            &parsed.runtime_function_symbols,
            &local_runtime_symbols,
            &parsed.global_extern_symbols,
            &mut import_func_ids,
            &mut local_func_ids,
            &mut import_data_ids,
        )?;
        let mut ctx = jit_module.codegen_make_context();
        ctx.func = function;
        let compiled = compile_prepared_function_bytes_with_isa(
            jit_module,
            env_config,
            object_isa,
            func_id,
            &mut ctx,
            &parsed.symbol,
            &format!(
                "failed to compile runtime CLIF function {} to object",
                parsed.symbol
            ),
        )?;
        jit_module.codegen_clear_context(&mut ctx);
        out.push(ObjectFunctionDefinition {
            func_id,
            symbol: parsed.symbol,
            binding: ElfSymbolBinding::Local,
            bytes: compiled.bytes,
            systemv_unwind_info: compiled.artifact.systemv_unwind_info,
        });
    }
    Ok(out)
}

pub fn precompile_codegen_module_to_object_file(
    module_name: &str,
    source_hash: u64,
    module: &BlockPyModule<CodegenModuleShape>,
    counter_dump_path: Option<&Path>,
    optimization_plan: Option<PrecompileOptimizationPlanInput<'_>>,
    module_index: Option<&PrecompileModuleIndex>,
    output_path: &Path,
) -> Result<PrecompileObjectSummary, String> {
    let bytes = precompile_codegen_module_to_object_bytes(
        module_name,
        source_hash,
        module,
        counter_dump_path,
        optimization_plan,
        module_index,
    )?;
    if let Some(parent) = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create object output dir {}: {err}",
                parent.display()
            )
        })?;
    }
    fs::write(output_path, bytes.object.as_slice()).map_err(|err| {
        format!(
            "failed to write object file {}: {err}",
            output_path.display()
        )
    })?;
    Ok(PrecompileObjectSummary {
        output_path: output_path.to_path_buf(),
        function_count: bytes.function_count,
        data_object_count: bytes.data_object_count,
        object_size_bytes: bytes.object.len(),
    })
}

struct PrecompiledObjectBytes {
    object: Vec<u8>,
    function_count: usize,
    data_object_count: usize,
    #[cfg(test)]
    function_symbols: Vec<String>,
    #[cfg(test)]
    data_symbols: Vec<String>,
    #[cfg(test)]
    data_symbol_writable: Vec<(String, bool)>,
}

fn precompile_codegen_module_to_object_bytes(
    module_name: &str,
    source_hash: u64,
    module: &BlockPyModule<CodegenModuleShape>,
    counter_dump_path: Option<&Path>,
    optimization_plan: Option<PrecompileOptimizationPlanInput<'_>>,
    module_index: Option<&PrecompileModuleIndex>,
) -> Result<PrecompiledObjectBytes, String> {
    let compile_session = crate::session::CompileSession::new();
    let env_config = compile_session.env_config()?;
    let object_isa = CraneliftTargetConfig::object(env_config).build_isa()?;
    let builder = new_jit_builder(env_config)?;
    let mut jit_module = JITModule::new(builder);
    let mut function_definitions =
        compile_runtime_support_clif_for_object(&mut jit_module, env_config, object_isa.as_ref())?;

    let module_constants = ModuleCodegenConstants::collect_from_module(module);
    let module_constant_ptrs = placeholder_module_constant_ptrs(module_constants.len());
    let module_constant_symbol_prefix =
        module_constant_symbol_prefix_for_module_identity(module_name, source_hash);
    let module_constant_object_data_ids = declare_module_constant_object_data_for_prefix(
        &mut jit_module,
        module_constant_symbol_prefix.as_str(),
        module_constant_ptrs.as_slice(),
    )?;

    let (counter_slots_by_id, scalar_counter_count, top_value_counter_count) =
        build_counter_storage_layout(module.counter_defs.as_slice())?;
    let scalar_counter_data_id = if scalar_counter_count == 0 {
        None
    } else {
        Some(define_scalar_counter_storage_data(
            &mut jit_module,
            module,
            scalar_counter_count,
        )?)
    };
    let top_value_counter_data_id = if top_value_counter_count == 0 {
        None
    } else {
        Some(define_top_value_counter_storage_data(
            &mut jit_module,
            module,
            top_value_counter_count,
        )?)
    };

    let mut data_definitions = Vec::new();
    let mut module_constant_accesses = Vec::with_capacity(module_constants.len());
    for (index, data_id) in module_constant_object_data_ids.iter().copied().enumerate() {
        let constant_id = ModuleConstantId(index);
        let symbol =
            module_constant_object_symbol(module_constant_symbol_prefix.as_str(), constant_id);
        if let Some(image) = module_constants.static_pyobject_image(constant_id) {
            module_constant_accesses.push(ModuleConstantAccess::SymbolAddress);
            data_definitions.push(ObjectDataDefinition {
                data_id,
                symbol,
                binding: ElfSymbolBinding::Global,
                bytes: image.bytes,
                align: image.align,
                writable: image.writable,
                relocations: image
                    .relocations
                    .into_iter()
                    .map(|relocation| ObjectDataRelocation {
                        offset: relocation.offset,
                        symbol: relocation.symbol.to_string(),
                        kind: ElfSymbolKind::Object,
                        reloc_type: R_X86_64_64,
                        addend: 0,
                    })
                    .collect(),
            });
        } else {
            module_constant_accesses.push(ModuleConstantAccess::PointerSlot);
            data_definitions.push(ObjectDataDefinition {
                data_id,
                symbol,
                binding: ElfSymbolBinding::Global,
                bytes: vec![0; std::mem::size_of::<usize>()],
                align: std::mem::align_of::<usize>() as u64,
                writable: true,
                relocations: Vec::new(),
            });
        }
    }
    let module_constant_access_table =
        ModuleConstantAccessTable::from_entries(module_constant_accesses);
    if let Some(data_id) = scalar_counter_data_id {
        data_definitions.push(ObjectDataDefinition {
            data_id,
            symbol: scalar_counter_storage_symbol(module),
            binding: ElfSymbolBinding::Global,
            bytes: vec![
                0;
                scalar_counter_count
                    .checked_mul(std::mem::size_of::<u64>())
                    .ok_or_else(|| format!(
                        "scalar counter storage size overflow: {scalar_counter_count}"
                    ))?
            ],
            align: std::mem::align_of::<u64>() as u64,
            writable: true,
            relocations: Vec::new(),
        });
    }
    if let Some(data_id) = top_value_counter_data_id {
        data_definitions.push(ObjectDataDefinition {
            data_id,
            symbol: top_value_counter_storage_symbol(module),
            binding: ElfSymbolBinding::Global,
            bytes: vec![
                0;
                top_value_counter_count
                    .checked_mul(std::mem::size_of::<TopValueCounter>())
                    .ok_or_else(|| format!(
                        "top-value counter storage size overflow: {top_value_counter_count}"
                    ))?
            ],
            align: std::mem::align_of::<TopValueCounter>() as u64,
            writable: true,
            relocations: Vec::new(),
        });
    }

    let planned_inputs = planned_optimization_inputs_for_precompile(
        optimization_plan,
        module_index,
        module_name,
        source_hash,
        module,
    )?;
    let specialization_profile = SpecializationProfile::from_precompile(
        env_config,
        module_name,
        counter_dump_path,
        planned_inputs,
    )?;
    let jit_module_plan = if specialization_profile.has_v3_optimization_inputs() {
        build_typed_v3_jit_module_plan(module, Some(&specialization_profile), env_config)?
    } else {
        build_jit_module_plan(module)?
    };
    let planned_module = jit_module_plan.module.as_ref();
    let external_direct_call_target_functions = precompile_external_direct_call_target_functions(
        planned_module,
        &specialization_profile,
        module_index,
    )?;
    let mut predeclared = HashMap::new();
    let mut symbol_scopes = HashMap::new();
    for function in &planned_module.callable_defs {
        let persistent_id = persistent_function_id_for_module_function(
            module_name,
            source_hash,
            function.function_id.local_function_id(),
        );
        let symbol_scope = precompiled_direct_function_symbol_scope_for_persistent(&persistent_id);
        let (_sig, declared) =
            declare_direct_function(&mut jit_module, function, Some(symbol_scope.as_str()))?;
        predeclared.insert(function.function_id, declared);
        symbol_scopes.insert(function.function_id, symbol_scope);
    }
    if let Some(module_index) = module_index {
        for (function_id, function) in &external_direct_call_target_functions {
            if predeclared.contains_key(function_id) {
                continue;
            }
            let Some(symbol_scope) =
                module_index.precompiled_symbol_scope_for_function(*function_id)
            else {
                continue;
            };
            let declared =
                declare_imported_direct_function(&mut jit_module, function, symbol_scope.as_str())?;
            predeclared.insert(*function_id, declared);
        }
    }
    for function in &planned_module.callable_defs {
        let placeholder_blocks =
            vec![std::ptr::null_mut::<std::ffi::c_void>(); function.blocks.len()];
        let jit_local_plan = jit_module_plan
            .locals
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT local plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let jit_deopt_resume_plan = jit_module_plan
            .deopt_resume
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT deopt resume plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let built = build_cranelift_run_bb_specialized_function(
            &mut jit_module,
            placeholder_blocks.as_slice(),
            planned_module,
            function,
            module
                .callable_defs
                .iter()
                .find(|candidate| candidate.function_id == function.function_id),
            &jit_module_plan.value_facts,
            jit_local_plan,
            jit_deopt_resume_plan,
            &module_constants,
            planned_module.counter_defs.as_slice(),
            module_constant_object_data_ids.as_slice(),
            counter_slots_by_id.as_ref(),
            scalar_counter_data_id,
            top_value_counter_data_id,
            &compile_session,
            None,
            symbol_scopes.get(&function.function_id).map(String::as_str),
            Some(&predeclared),
            BuildSpecializedFunctionOptions {
                module_constant_accesses: module_constant_access_table.clone(),
                external_direct_call_target_functions: external_direct_call_target_functions
                    .clone(),
                ..BuildSpecializedFunctionOptions::default()
            },
        )
        .map_err(|err| {
            format!(
                "{err} [function={} id={}]",
                function.names.qualname, function.function_id
            )
        })?;
        let mut ctx = built.ctx;
        let compiled = compile_prepared_function_bytes_with_isa(
            &mut jit_module,
            env_config,
            object_isa.as_ref(),
            built.main_id,
            &mut ctx,
            direct_function_backend_name(function, None).as_str(),
            "failed to compile specialized jit run_bb function to object",
        )
        .map_err(|err| {
            format!(
                "{err} [function={} id={}]",
                function.names.qualname, function.function_id
            )
        })?;
        jit_module.codegen_clear_context(&mut ctx);
        function_definitions.push(ObjectFunctionDefinition {
            func_id: built.main_id,
            symbol: built.main_symbol,
            binding: ElfSymbolBinding::Global,
            bytes: compiled.bytes,
            systemv_unwind_info: compiled.artifact.systemv_unwind_info,
        });
        match (
            built.default_adapter_id,
            built.default_adapter_symbol.as_ref(),
        ) {
            (Some(default_adapter_id), Some(default_adapter_symbol)) => {
                let mut default_ctx = build_default_resolving_direct_adapter(
                    &mut jit_module,
                    function,
                    built.main_id,
                    default_adapter_id,
                )
                .map_err(|err| {
                    format!(
                        "{err} [default-adapter function={} id={}]",
                        function.names.qualname, function.function_id
                    )
                })?;
                let compiled = compile_prepared_function_bytes_with_isa(
                    &mut jit_module,
                    env_config,
                    object_isa.as_ref(),
                    default_adapter_id,
                    &mut default_ctx,
                    default_adapter_symbol.as_str(),
                    "failed to compile default-resolving direct adapter to object",
                )
                .map_err(|err| {
                    format!(
                        "{err} [default-adapter function={} id={}]",
                        function.names.qualname, function.function_id
                    )
                })?;
                jit_module.codegen_clear_context(&mut default_ctx);
                function_definitions.push(ObjectFunctionDefinition {
                    func_id: default_adapter_id,
                    symbol: default_adapter_symbol.clone(),
                    binding: ElfSymbolBinding::Global,
                    bytes: compiled.bytes,
                    systemv_unwind_info: compiled.artifact.systemv_unwind_info,
                });
            }
            (None, None) => {}
            _ => {
                return Err(format!(
                    "default direct adapter declaration is inconsistent for function {} id={}",
                    function.names.qualname, function.function_id
                ));
            }
        }
    }

    let object = write_precompiled_object(
        &jit_module,
        object_isa.as_ref(),
        &function_definitions,
        &data_definitions,
    )?;
    Ok(PrecompiledObjectBytes {
        object,
        function_count: function_definitions.len(),
        data_object_count: data_definitions.len(),
        #[cfg(test)]
        function_symbols: function_definitions
            .iter()
            .map(|definition| definition.symbol.clone())
            .collect(),
        #[cfg(test)]
        data_symbols: data_definitions
            .iter()
            .map(|definition| definition.symbol.clone())
            .collect(),
        #[cfg(test)]
        data_symbol_writable: data_definitions
            .iter()
            .map(|definition| (definition.symbol.clone(), definition.writable))
            .collect(),
    })
}

fn rewrite_import_fn_aliases(
    clif: &str,
    import_id_to_symbol: &HashMap<u32, &'static str>,
) -> String {
    let mut import_aliases: HashMap<String, String> = HashMap::new();
    for raw_line in clif.lines() {
        let line = raw_line.trim_start();
        let Some(eq_pos) = line.find(" = ") else {
            continue;
        };
        let alias = &line[..eq_pos];
        if alias.is_empty() {
            continue;
        }
        let rest = &line[(eq_pos + 3)..];
        let rest = rest.strip_prefix("colocated ").unwrap_or(rest);
        let Some(first_token) = rest.split_whitespace().next() else {
            continue;
        };
        let Some(colon_pos) = first_token.find(':') else {
            continue;
        };
        let import_id = &first_token[(colon_pos + 1)..];
        if import_id.is_empty() || !import_id.as_bytes().iter().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(import_id) = import_id.parse::<u32>() else {
            continue;
        };
        let Some(symbol) = import_id_to_symbol.get(&import_id) else {
            continue;
        };
        import_aliases.insert(alias.to_string(), (*symbol).to_string());
    }

    let bytes = clif.as_bytes();
    let mut out = String::with_capacity(clif.len() + 128);
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'f' && index + 2 < bytes.len() && bytes[index + 1] == b'n' {
            let start = index;
            let mut end = index + 2;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            let has_digits = end > start + 2;
            let left_boundary = start == 0 || !is_clif_ident_byte(bytes[start - 1]);
            let right_boundary = end >= bytes.len() || !is_clif_ident_byte(bytes[end]);
            if has_digits && left_boundary && right_boundary {
                let token = &clif[start..end];
                if let Some(alias) = import_aliases.get(token) {
                    out.push_str(alias);
                    index = end;
                    continue;
                }
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

fn register_block_display_annotation(
    annotations: &mut ClifBlockDisplayAnnotations,
    block: ir::Block,
    semantic_name: impl Into<String>,
    param_names: Vec<String>,
) {
    annotations.insert(
        block.to_string(),
        ClifBlockDisplayAnnotation {
            semantic_name: semantic_name.into(),
            param_names,
        },
    );
}

fn parse_block_header_for_display(line: &str) -> Option<(&str, Vec<&str>)> {
    if line.trim_start().len() != line.len() || !line.starts_with("block") {
        return None;
    }
    let bytes = line.as_bytes();
    let mut token_end = "block".len();
    while token_end < bytes.len() && bytes[token_end].is_ascii_digit() {
        token_end += 1;
    }
    if token_end == "block".len() {
        return None;
    }
    let token = &line[..token_end];
    let mut cursor = token_end;
    let mut param_types = Vec::new();
    if cursor < bytes.len() && bytes[cursor] == b'(' {
        let params_start = cursor + 1;
        let params_end = params_start + line[params_start..].find(')')?;
        let params_text = &line[params_start..params_end];
        if !params_text.trim().is_empty() {
            for param in params_text.split(", ") {
                let (_, ty) = param.split_once(':')?;
                param_types.push(ty.trim());
            }
        }
        cursor = params_end + 1;
    }
    if !line[cursor..].trim_end().ends_with(':') {
        return None;
    }
    Some((token, param_types))
}

fn rewrite_block_header_annotations(
    clif: &str,
    block_annotations: &ClifBlockDisplayAnnotations,
) -> String {
    let mut out = String::with_capacity(clif.len() + (block_annotations.len() * 48));
    for chunk in clif.split_inclusive('\n') {
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        out.push_str(line);
        if let Some((token, param_types)) = parse_block_header_for_display(line) {
            let annotation = block_annotations.get(token);
            let semantic_name = annotation
                .map(|annotation| annotation.semantic_name.as_str())
                .unwrap_or(token);
            let param_names = annotation.map(|annotation| annotation.param_names.as_slice());
            out.push_str(" ; block ");
            out.push_str(semantic_name);
            out.push('(');
            for (index, ty) in param_types.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                let fallback_name = format!("param{index}");
                let param_name = param_names
                    .and_then(|names| names.get(index))
                    .map(String::as_str)
                    .unwrap_or(fallback_name.as_str());
                out.push_str(param_name);
                out.push_str(": ");
                out.push_str(ty);
            }
            out.push(')');
        }
        if chunk.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

pub fn run_cranelift_smoke(module: &BlockPyModule<CodegenModuleShape>) -> Result<(), String> {
    let function_count = module.callable_defs.len() as i64;
    let block_count = module
        .callable_defs
        .iter()
        .map(|f| f.blocks.len() as i64)
        .sum::<i64>();
    let sentinel = (function_count << 32) ^ block_count;

    let compile_session = crate::session::CompileSession::new();
    let mut jit_module = new_jit_module(&compile_session)?;
    let env_config = compile_session.env_config()?;
    let mut ctx = jit_module.codegen_make_context();
    ctx.func
        .signature
        .returns
        .push(ir::AbiParam::new(ir::types::I64));
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let value = builder.ins().iconst(ir::types::I64, sentinel);
        builder.ins().return_(&[value]);
        builder.finalize();
    }

    let function_id = declare_local_fn(&mut jit_module, "dp_jit_smoke", &ctx.func.signature)?;
    let _ = define_prepared_function(
        &mut jit_module,
        env_config,
        function_id,
        &mut ctx,
        "jit-smoke",
        "failed to define Cranelift function",
    )?;
    jit_module.codegen_clear_context(&mut ctx);
    jit_module
        .finalize_definitions()
        .map_err(|err| format!("failed to finalize Cranelift definitions: {err}"))?;

    let code_ptr = jit_module.get_finalized_function(function_id);
    let compiled: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code_ptr) };
    let got = compiled();
    if got != sentinel {
        return Err(format!(
            "Cranelift JIT smoke mismatch: expected {sentinel}, got {got}"
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct BuildSpecializedFunctionOptions {
    module_constant_accesses: ModuleConstantAccessTable,
    counted_refcount_helpers: Option<CountedRefcountHelpers>,
    planned_typed_function: Option<BlockPyFunction<TypedCodegenModuleShape>>,
    external_direct_call_target_functions:
        HashMap<RuntimeFunctionId, BlockPyFunction<TypedCodegenModuleShape>>,
}

struct PreparedSpecializedTypedFunction {
    typed_function: BlockPyFunction<TypedCodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    typed_function: &BlockPyFunction<TypedCodegenModuleShape>,
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
    typed_function: &mut BlockPyFunction<TypedCodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    planned_typed_function: Option<&BlockPyFunction<TypedCodegenModuleShape>>,
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

fn instr_typed_variant_name(expr: &InstrTyped) -> &'static str {
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

fn render_instr_typed_preorder_extras(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> String {
    struct ExtraRenderer<'a> {
        out: &'a mut String,
        block_label: Option<BlockLabel>,
        ordinal: usize,
    }

    impl Visit<InstrTyped> for ExtraRenderer<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            let block_label = self
                .block_label
                .map(|label| label.to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            let instr_id = expr
                .try_semantic_instr_id()
                .map(|instr_id| instr_id.to_string())
                .unwrap_or_else(|| "<synthetic>".to_string());
            match expr.typed_extra() {
                Some(extra) => {
                    self.out.push_str(&format!(
                        "; typed_expr[{}] block={} instr_id={} kind={} extra={:?}\n",
                        self.ordinal,
                        block_label,
                        instr_id,
                        instr_typed_variant_name(expr),
                        extra
                    ));
                }
                None => {
                    self.out.push_str(&format!(
                        "; typed_expr[{}] block={} instr_id={} kind={} extra=<none>\n",
                        self.ordinal,
                        block_label,
                        instr_id,
                        instr_typed_variant_name(expr)
                    ));
                }
            }
            self.ordinal += 1;
            expr.visit_children(self);
        }
    }

    let mut out = String::new();
    let mut renderer = ExtraRenderer {
        out: &mut out,
        block_label: None,
        ordinal: 0,
    };
    for block in &function.blocks {
        renderer.block_label = Some(block.label);
        renderer.visit_block(block);
    }
    out
}

fn build_cranelift_run_bb_specialized_function(
    codegen_env: &mut impl JitCodegenEnv,
    blocks: &[ObjPtr],
    module: &BlockPyModule<TypedCodegenModuleShape>,
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    legacy_scalar_thread_function: Option<&BlockPyFunction<CodegenModuleShape>>,
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
    for counter_id in call_target_counter_ids
        .values()
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
        .chain(setitem_specialized_hit_counter_ids.values())
        .chain(setitem_specialized_fallback_counter_ids.values())
        .chain(global_indexed_hit_counter_ids.values())
        .chain(global_indexed_fallback_counter_ids.values())
        .chain(field_indexed_hit_counter_ids.values())
        .chain(field_indexed_fallback_counter_ids.values())
        .chain(field_generic_getattr_counter_ids.values())
        .chain(field_generic_setattr_counter_ids.values())
    {
        scalar_counter_slot_for_ref(counter_slots_by_id, *counter_ref).map_err(|err| {
            format!(
                "{err} for function {} ({})",
                function.names.qualname, function.function_id
            )
        })?;
    }
    let requires_top_value_counters = !call_target_counter_ids.is_empty()
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
    let guard_miss_deopt_instr_ids = collect_typed_guard_miss_deopt_instr_ids(&typed_function);
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
            lower_codegen_function_to_typed(target_function),
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
        fb.set_cold_block(step_null_block);
        fb.set_cold_block(raise_exc_direct_block);
        let required_stack_slot_names =
            jit_local_plan.required_stack_slot_names_for_function(function);
        let stack_slots = StackSlots::new(&mut fb, &required_stack_slot_names);
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
            register_block_display_annotation(
                &mut block_annotations,
                cleanup,
                "cleanup_null::shared",
                vec!["error".into()],
            );
        }
        for (index, pre_cleanup, cleanup) in &per_exception_null_cleanup_blocks {
            register_block_display_annotation(
                &mut block_annotations,
                *pre_cleanup,
                format!("pre_cleanup_null::{}", function.blocks[*index].label),
                Vec::new(),
            );
            register_block_display_annotation(
                &mut block_annotations,
                *cleanup,
                format!("cleanup_null::{}", function.blocks[*index].label),
                vec!["error".into()],
            );
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

        fb.append_block_params_for_function_params(entry_block);
        for (index, block) in exec_blocks.iter().enumerate() {
            for _ in &runtime_block_params[index] {
                fb.append_block_param(*block, ptr_ty);
            }
        }
        fb.append_block_param(step_null_block, ptr_ty); // args
        fb.append_block_param(raise_exc_direct_block, ptr_ty); // args
        fb.append_block_param(raise_exc_direct_block, ptr_ty); // exc
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
        let py_call_positional_three_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT,
        );
        let py_call_object_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PY_CALL_OBJECT_IMPORT);
        let py_vectorcall_ref =
            func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_PY_VECTORCALL_IMPORT);
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
        let guard_miss_deopt_stub_ref =
            (env_config.jit_refcount_emission_enabled() && guard_miss_deopt_stub).then(|| {
                func_imports.get_or_panic(codegen_env, &mut fb.func, &DP_JIT_DEOPT_RESUME_IMPORT)
            });
        let store_global_indexed_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &SOAC_RUNTIME_STORE_GLOBAL_INDEXED_IMPORT,
        );
        let probe_field_indexed_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &SOAC_RUNTIME_PROBE_FIELD_INDEXED_IMPORT,
        );
        let store_field_indexed_ref = func_imports.get_or_panic(
            codegen_env,
            &mut fb.func,
            &SOAC_RUNTIME_STORE_FIELD_INDEXED_IMPORT,
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
            function.params.len(),
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
        let mut entry_param_values = HashMap::new();
        for (param, value) in function.params.iter().zip(direct_entry_args.iter()) {
            let needs_runtime_arg = entry_runtime_param_names.contains(param.name.as_str());
            let needs_stack_seed = entry_stack_seed_param_names.contains(param.name.as_str());
            let needs_owned_value = needs_runtime_arg || needs_stack_seed;
            let selected_value = if needs_owned_value {
                emit_incref_if_not_null(&mut fb, ptr_ty, incref_ref, *value);
                Some(*value)
            } else {
                None
            };

            if let Some(selected_value) = selected_value {
                if needs_stack_seed && !needs_runtime_arg {
                    stack_slots
                        .replace_cloned_value(
                            &mut fb,
                            param.name.as_str(),
                            selected_value,
                            ptr_ty,
                            thread_state_value,
                            incref_ref,
                            decref_ref,
                        )
                        .expect("entry slot missing from stack slots");
                    fb.ins()
                        .call(decref_ref, &[thread_state_value, selected_value]);
                }
                if needs_runtime_arg {
                    entry_param_values.insert(param.name.as_str(), selected_value);
                }
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
        let entry_jump_args = runtime_block_params[0]
            .iter()
            .map(|param| {
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

        let mut opt_v3_fused_scalar_thread_consumers = HashSet::<BlockLabel>::new();
        for (index, block) in exec_blocks.iter().enumerate() {
            fb.switch_to_block(*block);
            let codegen_block = &function.blocks[index];
            if opt_v3_fused_scalar_thread_consumers.contains(&codegen_block.label) {
                fb.ins().trap(OPT_V3_FUSED_CONSUMER_TRAP);
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
                ptr_ty,
                thread_state_value,
                incref_ref,
                decref_ref,
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
                module_constants,
                value_facts,
                deopt_resume_plan: jit_deopt_resume_plan,
                refcount_plan,
                instr_locations: &instr_locations,
                counter_slots_by_id,
                storage_layout: function.storage_layout().clone(),
                function_runtime_data_layout: &function_runtime_data_layout,
                incref_ref,
                decref_ref,
                py_call_positional_three_ref,
                py_vectorcall_ref,
                pytype_generic_alloc_ref,
                finish_constructor_init_ref,
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
                guard_miss_resume_point: None,
                store_global_indexed_ref,
                probe_field_indexed_ref,
                store_field_indexed_ref,
                load_runtime_obj_by_id_ref,
                enter_recursive_ref,
                direct_compile_function_env_ref,
                pyobject_getattr_ref,
                pyobject_setattr_ref,
                pyobject_getitem_ref,
                pyobject_setitem_ref,
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
                call_direct_hit_counter_ids: &call_direct_hit_counter_ids,
                call_direct_fallback_counter_ids: &call_direct_fallback_counter_ids,
                operator_shape_counter_ids: &operator_shape_counter_ids,
                getitem_shape_counter_ids: &getitem_shape_counter_ids,
                getitem_specialized_hit_counter_ids: &getitem_specialized_hit_counter_ids,
                getitem_specialized_fallback_counter_ids: &getitem_specialized_fallback_counter_ids,
                setitem_shape_counter_ids: &setitem_shape_counter_ids,
                setitem_specialized_hit_counter_ids: &setitem_specialized_hit_counter_ids,
                setitem_specialized_fallback_counter_ids: &setitem_specialized_fallback_counter_ids,
                global_indexed_hit_counter_ids: &global_indexed_hit_counter_ids,
                global_indexed_fallback_counter_ids: &global_indexed_fallback_counter_ids,
                field_indexed_hit_counter_ids: &field_indexed_hit_counter_ids,
                field_indexed_fallback_counter_ids: &field_indexed_fallback_counter_ids,
                field_generic_getattr_counter_ids: &field_generic_getattr_counter_ids,
                field_generic_setattr_counter_ids: &field_generic_setattr_counter_ids,
                deopt_entry_guard_miss_counter_ids: &deopt_entry_guard_miss_counter_ids,
                branch_outcome_counter_ids: &branch_outcome_counter_ids,
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

            if let Some(legacy_scalar_thread_function) = legacy_scalar_thread_function {
                if let Some(fused_labels) = emit_opt_v3_scalar_threaded_store_branch(
                    &mut fb,
                    codegen_block.label,
                    &typed_function.blocks[index],
                    &typed_function,
                    legacy_scalar_thread_function,
                    jit_local_plan,
                    &exec_blocks,
                    &block_indices_by_label,
                    jump_edge_transports,
                    implicit_target_transports,
                    &mut local_env,
                    &emit_ctx,
                    cleanup_null_blocks[index],
                    &mut pending_local_failure_cleanups,
                    &mut local_failure_cleanup_blocks,
                    codegen_env,
                    &mut func_imports,
                    codegen_block.exception_param(),
                )? {
                    opt_v3_fused_scalar_thread_consumers.extend(fused_labels);
                    continue;
                }
            }

            emit_typed_codegen_ops(
                &mut fb,
                &typed_function.blocks[index].body,
                &mut local_env,
                &stack_slots,
                &emit_ctx,
                cleanup_null_blocks[index],
                &mut pending_local_failure_cleanups,
                &mut local_failure_cleanup_blocks,
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
                ptr_ty,
                thread_state_value,
                slot_write_none_const,
                incref_ref,
                decref_ref,
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
                ptr_ty,
                thread_state_value,
                decref_ref,
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

        for cleanup in &pending_local_failure_cleanups {
            fb.switch_to_block(cleanup.block);
            let cleanup_params = fb.block_params(cleanup.block).to_vec();
            let cleanup_values = &cleanup_params[..cleanup.cleanup_arg_count];
            for &value in cleanup_values {
                emit_decref_if_not_null(&mut fb, ptr_ty, decref_ref, thread_state_value, value);
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
            stack_slots.decref_all(&mut fb, ptr_ty, thread_state_value, decref_ref);
            fb.ins()
                .call(set_raised_exception_ref, &[thread_state_value, error_value]);
            let null_ptr = fb.ins().iconst(ptr_ty, 0);
            fb.ins().return_(&[null_ptr]);
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
            stack_slots.decref_all(&mut fb, ptr_ty, thread_state_value, decref_ref);
            fb.ins()
                .call(set_raised_exception_ref, &[thread_state_value, error_value]);
            let null_ptr = fb.ins().iconst(ptr_ty, 0);
            fb.ins().return_(&[null_ptr]);
        }

        fb.switch_to_block(step_null_block);
        let step_null_args = fb.block_params(step_null_block)[0];
        let error_value =
            emit_take_current_raised_exception_or_trap(&mut fb, ptr_ty, thread_state_value);
        stack_slots.decref_all(&mut fb, ptr_ty, thread_state_value, decref_ref);
        fb.ins()
            .call(decref_ref, &[thread_state_value, step_null_args]);
        fb.ins()
            .call(set_raised_exception_ref, &[thread_state_value, error_value]);
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        fb.ins().return_(&[null_ptr]);

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
        stack_slots.decref_all(&mut fb, ptr_ty, thread_state_value, decref_ref);
        fb.ins().return_(&[red_null]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    direct_edge_stats.emit_trace(
        direct_call_resolver
            .map(|shared_state| shared_state.module_name.as_str())
            .unwrap_or("<standalone>"),
        function,
    );

    Ok(BuiltSpecializedFunction {
        ctx,
        main_id,
        main_symbol,
        default_adapter_id,
        default_adapter_symbol,
        import_id_to_symbol: module_imports.debug_symbols().clone(),
        #[cfg(test)]
        func_id_to_symbol: module_imports.debug_declared_symbols().clone(),
        block_annotations,
    })
}

pub unsafe fn render_cranelift_run_bb_specialized_with_cfg(
    blocks: &[ObjPtr],
    module: &BlockPyModule<CodegenModuleShape>,
    function: &soac_core::block_py::BlockPyFunction<CodegenModuleShape>,
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
    module: &BlockPyModule<CodegenModuleShape>,
    function: &soac_core::block_py::BlockPyFunction<CodegenModuleShape>,
    runtime_state: Option<&SharedModuleState>,
) -> Result<String, String> {
    let builder = new_jit_builder(compile_session.env_config()?)?;
    let mut jit_module = JITModule::new(builder);
    let specialization_profile = SpecializationProfile::from_runtime_state_with_session(
        runtime_state,
        Some(compile_session),
    )?;
    predeclare_specialization_type_imports(&mut jit_module, &specialization_profile)?;
    let jit_module_plan =
        if runtime_state.is_some() && specialization_profile.has_v3_optimization_inputs() {
            build_typed_v3_jit_module_plan(
                module,
                Some(&specialization_profile),
                compile_session.env_config()?,
            )?
        } else {
            build_jit_module_plan(module)?
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
            lower_codegen_function_to_typed(target_function),
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
    out.push_str("; ---- typed nodes and embedded extras, preorder ----\n");
    out.push_str(&render_instr_typed_preorder_extras(&typed_function));
    out.push('\n');
    out.push_str("; ---- BlockPyFunction<TypedCodegenModuleShape> debug ----\n");
    out.push_str(&format!("{typed_function:#?}"));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

pub unsafe fn render_cranelift_run_bb_specialized_with_runtime_state_and_cfg(
    compile_session: &crate::session::CompileSession,
    blocks: &[ObjPtr],
    module: &BlockPyModule<CodegenModuleShape>,
    function: &soac_core::block_py::BlockPyFunction<CodegenModuleShape>,
    module_constants: &ModuleCodegenConstants,
    runtime_state: Option<&SharedModuleState>,
) -> Result<RenderedSpecializedClif, String> {
    if blocks.is_empty() {
        return Err("specialized JIT run_bb requires at least one block".to_string());
    }

    let builder = new_jit_builder(compile_session.env_config()?)?;
    let mut jit_module = JITModule::new(builder);
    let specialization_profile = SpecializationProfile::from_runtime_state_with_session(
        runtime_state,
        Some(compile_session),
    )?;
    predeclare_specialization_type_imports(&mut jit_module, &specialization_profile)?;
    let jit_module_plan =
        if runtime_state.is_some() && specialization_profile.has_v3_optimization_inputs() {
            build_typed_v3_jit_module_plan(
                module,
                Some(&specialization_profile),
                compile_session.env_config()?,
            )?
        } else {
            build_jit_module_plan(module)?
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
    let built = build_cranelift_run_bb_specialized_function(
        &mut jit_module,
        render_blocks,
        render_module,
        render_function,
        Some(function),
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
        None,
        BuildSpecializedFunctionOptions::default(),
    )?;
    let mut out = String::new();
    out.push_str("; import fn aliases (Cranelift display id -> symbol)\n");
    let mut symbols: Vec<&'static str> = built.import_id_to_symbol.values().copied().collect();
    symbols.sort_unstable();
    symbols.dedup();
    for symbol in symbols {
        out.push_str("; ");
        out.push_str(symbol);
        out.push('\n');
    }
    out.push('\n');
    let pre_inline_clif = render_pre_inline_clif_for_inspection(
        &built.ctx.func,
        &built.import_id_to_symbol,
        &built.block_annotations,
    );
    let (compiled_clif, cfg_dot, vcode_disasm) = render_compiled_clif_and_vcode_disasm(
        &mut jit_module,
        compile_session.env_config()?,
        built.ctx,
        &built.import_id_to_symbol,
        &built.block_annotations,
    )?;
    out.push_str(&compiled_clif);
    Ok(RenderedSpecializedClif {
        pre_inline_clif,
        clif: out,
        cfg_dot,
        vcode_disasm,
    })
}

fn render_pre_inline_clif_for_inspection(
    func: &ir::Function,
    import_id_to_symbol: &HashMap<u32, &'static str>,
    block_annotations: &ClifBlockDisplayAnnotations,
) -> String {
    let mut clif = String::new();
    clif.push_str("; ---- pre-inlining CLIF for inspection ----\n");
    clif.push_str(
        "; emitted after SOAC typed codegen and before runtime support CLIF inlining and Cranelift optimization\n",
    );
    let clif_display =
        rewrite_import_fn_aliases(func.display().to_string().as_str(), import_id_to_symbol);
    clif.push_str(&rewrite_block_header_annotations(
        &clif_display,
        block_annotations,
    ));
    clif
}

fn render_compiled_clif_and_vcode_disasm(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    mut ctx: cranelift_codegen::Context,
    import_id_to_symbol: &HashMap<u32, &'static str>,
    block_annotations: &ClifBlockDisplayAnnotations,
) -> Result<(String, String, String), String> {
    prepare_cranelift_function_for_backend(
        jit_module,
        env_config,
        None,
        &mut ctx,
        "failed to render specialized jit run_bb function",
    )?;

    let mut display_func = ctx.func.clone();
    let normalize_stats = normalize_postopt_clif_for_inspection(&mut display_func);
    let cfg_dot = CFGPrinter::new(&display_func).to_string();

    let mut clif = String::new();
    clif.push_str("; ---- normalized post-opt CLIF for inspection ----\n");
    clif.push_str(
        "; trivial jump-only blocks are collapsed here for readability; production codegen uses the unnormalized post-opt CLIF\n",
    );
    clif.push_str(&format!(
        "; normalized trivial jumps: redirected_edges={}, removed_blocks={}\n",
        normalize_stats.redirected_edges, normalize_stats.removed_blocks
    ));
    let clif_display = rewrite_import_fn_aliases(
        display_func.display().to_string().as_str(),
        import_id_to_symbol,
    );
    clif.push_str(&rewrite_block_header_annotations(
        &clif_display,
        block_annotations,
    ));

    let mut ctrl_plane = ControlPlane::default();
    let compiled = jit_module
        .codegen_isa()
        .compile_function(&ctx.func, &ctx.domtree, true, &mut ctrl_plane)
        .map_err(|err| format!("failed to compile specialized jit run_bb function: {err:?}"))?;

    let mut vcode_disasm = String::new();
    vcode_disasm.push_str("; ---- emitted VCode disassembly ----\n");
    match compiled.vcode {
        Some(disasm) if !disasm.trim().is_empty() => vcode_disasm.push_str(&disasm),
        _ => vcode_disasm.push_str("; emitted disassembly unavailable for this backend\n"),
    }

    Ok((clif, cfg_dot, vcode_disasm))
}

pub(crate) unsafe fn compile_cranelift_run_bb_specialized_cached(
    compile_session: &Arc<crate::session::CompileSession>,
    blocks: &[ObjPtr],
    module: &BlockPyModule<CodegenModuleShape>,
    function: &soac_core::block_py::BlockPyFunction<CodegenModuleShape>,
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

fn define_shared_vectorcall_trampoline(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    param_count: usize,
    symbol_name: &str,
) -> Result<VectorcallEntryFn, String> {
    let ptr_ty = jit_module.codegen_target_config().pointer_type();
    let i64_ty = ir::types::I64;
    let mut module_imports = ModuleFuncImports::new();

    let mut main_sig = jit_module.codegen_make_signature();
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.returns.push(ir::AbiParam::new(ptr_ty));

    let main_id = declare_local_fn(jit_module, symbol_name, &main_sig)?;

    let mut direct_sig = jit_module.codegen_make_signature();
    direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    for _ in 0..param_count {
        direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    }
    direct_sig.returns.push(ir::AbiParam::new(ptr_ty));

    let mut ctx = jit_module.codegen_make_context();
    ctx.func.signature = main_sig;
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);

        let callable_val = fb.block_params(entry)[0];
        let args_val = fb.block_params(entry)[1];
        let nargsf_val = fb.block_params(entry)[2];
        let kwnames_val = fb.block_params(entry)[3];

        let mut func_imports = FuncBuildImports::new(&mut module_imports);
        let bind_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_VECTORCALL_BIND_DIRECT_ARGS_IMPORT,
        );
        let compile_env_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_VECTORCALL_COMPILE_FUNCTION_ENV_IMPORT,
        );
        let enter_recursive_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_ENTER_RECURSIVE_CALL_IMPORT,
        );
        let decref_ref = func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_DECREF_IMPORT);
        let thread_state_get_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &PY_THREAD_STATE_GET_UNCHECKED_IMPORT,
        );
        let set_raised_exception_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT,
        );

        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let function_extra_val = load_py_function_soac_metadata_obj(&mut fb, ptr_ty, callable_val);
        let function_extra_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, function_extra_val, 0);
        let function_extra_ok = fb.create_block();
        let early_fail_block = fb.create_block();
        fb.ins().brif(
            function_extra_missing,
            early_fail_block,
            &[],
            function_extra_ok,
            &[],
        );
        fb.seal_block(early_fail_block);
        fb.seal_block(function_extra_ok);

        fb.switch_to_block(early_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(function_extra_ok);
        let function_env_val = fb.ins().load(
            ptr_ty,
            ir::MemFlags::trusted(),
            function_extra_val,
            PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
        );
        let function_env_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, function_env_val, 0);
        let function_env_ok = fb.create_block();
        let context_fail_block = fb.create_block();
        fb.ins().brif(
            function_env_missing,
            context_fail_block,
            &[],
            function_env_ok,
            &[],
        );
        fb.seal_block(context_fail_block);
        fb.seal_block(function_env_ok);

        fb.switch_to_block(context_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(function_env_ok);
        let initial_callee_ptr = load_function_env_obj(
            &mut fb,
            ptr_ty,
            function_env_val,
            FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET,
        );
        let initial_callee_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, initial_callee_ptr, 0);
        let compile_env_block = fb.create_block();
        let function_env_ready = fb.create_block();
        fb.append_block_param(function_env_ready, ptr_ty);
        fb.append_block_param(function_env_ready, ptr_ty);
        fb.ins().brif(
            initial_callee_missing,
            compile_env_block,
            &[],
            function_env_ready,
            &[
                ir::BlockArg::Value(function_env_val),
                ir::BlockArg::Value(initial_callee_ptr),
            ],
        );
        fb.seal_block(compile_env_block);

        fb.switch_to_block(compile_env_block);
        let compile_inst = fb
            .ins()
            .call(compile_env_ref, &[callable_val, function_extra_val]);
        let compiled_function_env_val = fb.inst_results(compile_inst)[0];
        let compiled_function_env_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, compiled_function_env_val, 0);
        let compile_fail_block = fb.create_block();
        let compiled_function_env_ok = fb.create_block();
        fb.ins().brif(
            compiled_function_env_missing,
            compile_fail_block,
            &[],
            compiled_function_env_ok,
            &[],
        );
        fb.seal_block(compile_fail_block);
        fb.seal_block(compiled_function_env_ok);

        fb.switch_to_block(compile_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(compiled_function_env_ok);
        let compiled_callee_ptr = load_function_env_obj(
            &mut fb,
            ptr_ty,
            compiled_function_env_val,
            FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET,
        );
        let compiled_callee_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, compiled_callee_ptr, 0);
        let compiled_callee_fail_block = fb.create_block();
        fb.ins().brif(
            compiled_callee_missing,
            compiled_callee_fail_block,
            &[],
            function_env_ready,
            &[
                ir::BlockArg::Value(compiled_function_env_val),
                ir::BlockArg::Value(compiled_callee_ptr),
            ],
        );
        fb.seal_block(compiled_callee_fail_block);
        fb.seal_block(function_env_ready);

        fb.switch_to_block(compiled_callee_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(function_env_ready);
        let function_env_val = fb.block_params(function_env_ready)[0];
        let callee_ptr = fb.block_params(function_env_ready)[1];
        let thread_state_inst = fb.ins().call(thread_state_get_ref, &[]);
        let thread_state_val = fb.inst_results(thread_state_inst)[0];
        let enter_inst = fb.ins().call(enter_recursive_ref, &[thread_state_val]);
        let enter_status = fb.inst_results(enter_inst)[0];
        let enter_failed = fb
            .ins()
            .icmp_imm(ir::condcodes::IntCC::NotEqual, enter_status, 0);
        let recursion_fail_block = fb.create_block();
        let bind_block = fb.create_block();
        fb.ins()
            .brif(enter_failed, recursion_fail_block, &[], bind_block, &[]);
        fb.seal_block(recursion_fail_block);
        fb.seal_block(bind_block);

        fb.switch_to_block(recursion_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(bind_block);
        let bound_args_slot = if param_count == 0 {
            None
        } else {
            Some(fb.create_sized_stack_slot(ir::StackSlotData::new(
                ir::StackSlotKind::ExplicitSlot,
                (param_count * std::mem::size_of::<u64>()) as u32,
                0,
            )))
        };
        let bound_args_ptr = if let Some(slot) = bound_args_slot {
            fb.ins().stack_addr(ptr_ty, slot, 0)
        } else {
            null_ptr
        };
        let out_len = fb.ins().iconst(i64_ty, param_count as i64);
        let bind_inst = fb.ins().call(
            bind_ref,
            &[
                callable_val,
                args_val,
                nargsf_val,
                kwnames_val,
                function_extra_val,
                bound_args_ptr,
                out_len,
            ],
        );
        let bind_ok = fb.inst_results(bind_inst)[0];
        let bind_failed = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, bind_ok, 0);
        let fail_block = fb.create_block();
        let ok_block = fb.create_block();
        fb.ins().brif(bind_failed, fail_block, &[], ok_block, &[]);
        fb.seal_block(fail_block);
        fb.seal_block(ok_block);

        fb.switch_to_block(fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(ok_block);
        let direct_sig_ref = fb.import_signature(direct_sig);
        let mut call_args = Vec::with_capacity(param_count + 2);
        call_args.push(function_env_val);
        call_args.push(thread_state_val);
        let mut owned_args = Vec::with_capacity(param_count);
        if let Some(slot) = bound_args_slot {
            for index in 0..param_count {
                let value =
                    fb.ins()
                        .stack_load(ptr_ty, slot, (index * std::mem::size_of::<u64>()) as i32);
                owned_args.push(value);
                call_args.push(value);
            }
        }
        let call_inst = fb
            .ins()
            .call_indirect(direct_sig_ref, callee_ptr, &call_args);
        let result = fb.inst_results(call_inst)[0];
        let result_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, result, null_ptr);
        let direct_null_block = fb.create_block();
        let direct_ok_block = fb.create_block();
        fb.ins()
            .brif(result_is_null, direct_null_block, &[], direct_ok_block, &[]);
        fb.seal_block(direct_null_block);
        fb.seal_block(direct_ok_block);

        fb.switch_to_block(direct_null_block);
        let error_value =
            emit_take_current_raised_exception_or_trap(&mut fb, ptr_ty, thread_state_val);
        for value in owned_args.iter().copied() {
            fb.ins().call(decref_ref, &[thread_state_val, value]);
        }
        fb.ins()
            .call(set_raised_exception_ref, &[thread_state_val, error_value]);
        fb.ins().return_(&[result]);

        fb.switch_to_block(direct_ok_block);
        for value in owned_args {
            fb.ins().call(decref_ref, &[thread_state_val, value]);
        }
        fb.ins().return_(&[result]);
        fb.seal_all_blocks();
        fb.finalize();
    }

    let main_artifact = define_prepared_function(
        jit_module,
        env_config,
        main_id,
        &mut ctx,
        &format!("direct-vectorcall-trampoline:{param_count}"),
        "failed to define direct vectorcall trampoline",
    )?;
    jit_module.codegen_clear_context(&mut ctx);
    jit_module
        .finalize_definitions()
        .map_err(|err| format!("failed to finalize direct vectorcall trampoline: {err}"))?;

    let code_ptr = jit_module.get_finalized_function(main_id);
    jitdump::record_code_load(
        symbol_name,
        code_ptr.cast::<u8>(),
        main_artifact.code_size,
        jit_module.codegen_isa(),
        main_artifact.systemv_unwind_info.as_ref(),
    )?;
    register_jit_signal_diagnostics(
        symbol_name,
        code_ptr.cast::<u8>(),
        &main_artifact,
        RuntimeFunctionId::global(),
        symbol_name,
        "direct_vectorcall_trampoline",
    );
    let entry: VectorcallEntryFn = unsafe { std::mem::transmute(code_ptr) };
    Ok(entry)
}

#[cfg(test)]
mod test;
