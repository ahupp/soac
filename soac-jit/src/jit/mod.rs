use self::precompiled_object::{
    ElfSymbolBinding, ElfSymbolKind, ObjectDataDefinition, ObjectDataRelocation,
    ObjectFunctionDefinition, R_X86_64_64, write_precompiled_object,
};
use crate::SOAC_RUNTIME_CLIF;
#[cfg(test)]
use crate::config::SOAC_JIT_EMIT_REFCOUNTS_ENV;
use crate::config::{
    CraneliftTargetConfig, SpecializationMode, behavior_change_indexed_stores_enabled,
    counter_dump_input_path_from_env, jit_refcount_emission_enabled, profiled_cold_blocks_enabled,
    soac_work_dir_from_env, specialization_mode_from_env, specialization_mode_is_profile,
};
use crate::counter::TopValueCounter;
use crate::counter_dump::{
    CollectedTypeKeyLayout, CounterDumpFile, CounterDumpTypeKey, collect_type_key_layouts,
    collect_type_table, read_block_entry_counts_from_file, read_branch_preferences_from_file,
    read_call_target_specializations_from_file, read_getitem_specializations_from_file,
    read_operator_specializations_from_file, read_setitem_specializations_from_file,
};
use crate::module_constants::{ModuleCodegenConstants, ModuleConstantId};
use crate::module_type::{CounterRuntimeSlot, SharedModuleState, build_counter_storage_layout};
use cranelift_codegen::cfg_printer::CFGPrinter;
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::inline::{Inline, InlineCommand};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::isa::TargetIsa;
#[cfg(test)]
use cranelift_codegen::settings::Configurable;
use cranelift_control::ControlPlane;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module, ModuleReloc};
use cranelift_reader::parse_functions;
use pyo3::ffi;
use soac_blockpy::block_py::{
    AbruptKind, BlockArg, BlockEdge, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule,
    BlockTerm, CallArgKeyword, CallArgPositional, CallableScopeKind, CellLocation, ChildVisitable,
    CodegenBlock, CounterDef, CounterId, CounterScope, CounterSite, Del, DeoptEntrySource,
    FunctionId, FunctionKind, HasMeta, HasSemanticInstrId, InstrCodegen, InstrId, InstrKey,
    Literal, LocalLocation, NameLocation, ParamKind, ResolvedName, StorageLayout, Store, Visit,
    WithMeta, operation as blockpy_intrinsics,
};
use soac_blockpy::passes::{
    CodegenModuleShape, FactStore, FunctionRefcountPlan, InstrResolved, InstrTyped,
    LocalEnvResumeBinding, LocalEnvResumeBindingState, LocalEnvResumePoint,
    LocalEnvResumeStatePrecision, LocalEnvResumeValueSource, LocalRefState, PyExactType,
    PyObjFacts, RefcountActionKind, RefcountReleaseReason, RefcountSite, RuntimeHelperId,
    TypedCodegenModuleShape, ValueFacts, infer_module_value_facts, lower_codegen_function_to_typed,
    lower_typed_function_if_tests_to_truthy, try_lower_typed_instr_to_codegen_legacy,
    try_lower_typed_term_to_codegen_legacy,
};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, CString, c_void};
use std::fs;
use std::mem::{MaybeUninit, offset_of};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use tracing::info;

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

mod deopt_interpreter;
#[allow(unused_imports)]
pub(crate) use deopt_interpreter::run_blockpy_function_from_entry;
mod direct_abi;
mod intrinsics;
mod jitdump;
mod operation_specializations;
mod planning;
mod precompiled_object;
mod runtime_context;
mod specialized_helpers;
mod typed_value;

use direct_abi::{
    ArgOwnership, DirectCallableDesc, DirectEntry, DirectTargetId, ErrorAbi, HiddenArgAbi,
    ParamAbi, PyLongI64Coercion, ResultAbi,
};
pub use planning::{
    BlockExcDispatchPlan, BlockParamFacts, CurrentJitRefcountPlanCheck, EdgeTransportPlan,
    FunctionLocalPlan, LocalRefKind, ParamBindingFacts, ParamProvenance, PlannedJitDeoptPoint,
    PlannedJitDeoptPointId, PlannedJitDeoptResumeFunction, PlannedJitDeoptResumeModule,
    PlannedJitFunctionLocals, PlannedJitModuleLocals, PlannedLocalEnvEntryMaterialization,
    PlannedLocalEnvEntrySource, PlannedLocalStorage, PlannedStackSlotEntrySeed,
    RuntimeBlockParamPlan, check_refcount_plan_against_current_jit, exc_dispatch_plan,
    local_ref_kind_for_stack_mirror, plan_function_locals, plan_function_refcount_ownership,
    plan_jit_deopt_resume_module, plan_jit_deopt_resume_module_from_passes,
    plan_jit_function_locals, plan_jit_module_locals,
    planned_implicit_target_transports_for_function, planned_jit_params_for_function,
    planned_jump_edge_transports_for_function,
    planned_local_env_entry_materializations_for_function,
    planned_stack_slot_entry_seeds_for_function, render_jit_deopt_resume_function,
    render_jit_deopt_resume_module, render_jit_function_locals, render_jit_module_locals,
};
use runtime_context::{
    FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET, FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET,
    FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET, FUNCTION_ENV_GLOBALS_OBJ_OFFSET,
    FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET, PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
    PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET,
};
pub use runtime_context::{ModuleJitContext, ModuleRuntimeContext};
pub use specialized_helpers::ObjPtr;
use specialized_helpers::register_specialized_jit_symbols;
pub use typed_value::{
    EmitResult, IntFacts, IntRange, IntWidth, ResultDemand, SoacRepr, SoacValue, ValueOwnership,
};

static RUNTIME_SUPPORT_LIBRARY: OnceLock<Result<RuntimeSupportLibrary, String>> = OnceLock::new();
static PRECOMPILED_LIBRARY: OnceLock<Result<Option<PrecompiledLibrary>, String>> = OnceLock::new();
static NEXT_IMPORT_SPEC_ID: AtomicUsize = AtomicUsize::new(0);
static JIT_DATA_SYMBOLS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
static TYPE_KEY_RUNTIME_REGISTRY: OnceLock<Mutex<HashMap<CounterDumpTypeKey, usize>>> =
    OnceLock::new();
const JIT_ARENA_BYTES: usize = 256 * 1024 * 1024;
const MISSING_PYTHON_EXCEPTION_TRAP: ir::TrapCode = ir::TrapCode::unwrap_user(1);
const COLD_BLOCK_ENTRY_RATE_DENOMINATOR: u64 = 100;
thread_local! {
    static PROCESS_JIT_COMPILE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

unsafe extern "C" {
    fn _Py_Dealloc(obj: *mut ffi::PyObject);
}

fn py_dealloc_symbol() -> *const u8 {
    _Py_Dealloc as *const u8
}

fn jit_data_symbols() -> &'static Mutex<HashMap<String, usize>> {
    JIT_DATA_SYMBOLS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn type_key_runtime_registry() -> &'static Mutex<HashMap<CounterDumpTypeKey, usize>> {
    TYPE_KEY_RUNTIME_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_jit_data_symbol(symbol: &str, ptr: *const u8) {
    let mut symbols = jit_data_symbols()
        .lock()
        .expect("JIT data symbol registry lock poisoned");
    symbols.insert(symbol.to_string(), ptr as usize);
}

fn lookup_registered_jit_data_symbol(symbol: &str) -> Option<*const u8> {
    let symbols = jit_data_symbols()
        .lock()
        .expect("JIT data symbol registry lock poisoned");
    symbols.get(symbol).copied().map(|ptr| ptr as *const u8)
}

fn cpython_type_symbol_name(symbol: CpythonTypeSymbol) -> &'static str {
    match symbol {
        CpythonTypeSymbol::Function => "PyFunction_Type",
        CpythonTypeSymbol::Method => "PyMethod_Type",
        CpythonTypeSymbol::Type => "PyType_Type",
        CpythonTypeSymbol::Long => "PyLong_Type",
        CpythonTypeSymbol::List => "PyList_Type",
    }
}

fn push_symbol_component_hex(out: &mut String, component: &str) {
    for byte in component.as_bytes() {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("upper hex digit should exist"));
        out.push(
            char::from_digit(u32::from(byte & 0x0f), 16).expect("lower hex digit should exist"),
        );
    }
}

fn reloc_type_ref_symbol_name(type_ref: &RelocTypeRef) -> Cow<'static, str> {
    match type_ref {
        RelocTypeRef::CpythonTypeSymbol(symbol) => Cow::Borrowed(cpython_type_symbol_name(*symbol)),
        RelocTypeRef::TypeKey(type_key) => {
            let mut symbol = String::from("__soac_typekey_");
            push_symbol_component_hex(&mut symbol, type_key.module_name.as_str());
            symbol.push('_');
            push_symbol_component_hex(&mut symbol, type_key.qualname.as_str());
            Cow::Owned(symbol)
        }
    }
}

fn reloc_callable_ref_symbol_name(callable_ref: &RelocCallableRef) -> String {
    match callable_ref {
        RelocCallableRef::OwnerAttr {
            owner_type_ref,
            attr_name,
        } => {
            let mut symbol = String::from("__soac_callable_owner_attr_");
            let owner_symbol = reloc_type_ref_symbol_name(owner_type_ref);
            push_symbol_component_hex(&mut symbol, owner_symbol.as_ref());
            symbol.push('_');
            push_symbol_component_hex(&mut symbol, attr_name.as_str());
            symbol
        }
    }
}

fn module_constant_symbol_prefix(module: &BlockPyModule<CodegenModuleShape>) -> String {
    format!(
        "__soac_module_constant_{}",
        module.module_name_gen.module_id()
    )
}

fn module_constant_symbol_prefix_for_instance(
    module: &BlockPyModule<CodegenModuleShape>,
    instance_key: usize,
) -> String {
    format!("{}_{}", module_constant_symbol_prefix(module), instance_key)
}

fn scalar_counter_storage_symbol(module: &BlockPyModule<CodegenModuleShape>) -> String {
    format!(
        "__soac_scalar_counters_{}",
        module.module_name_gen.module_id()
    )
}

fn scalar_counter_storage_symbol_for_instance(
    module: &BlockPyModule<CodegenModuleShape>,
    instance_key: usize,
) -> String {
    format!("{}_{}", scalar_counter_storage_symbol(module), instance_key)
}

fn top_value_counter_storage_symbol(module: &BlockPyModule<CodegenModuleShape>) -> String {
    format!(
        "__soac_top_value_counters_{}",
        module.module_name_gen.module_id()
    )
}

fn top_value_counter_storage_symbol_for_instance(
    module: &BlockPyModule<CodegenModuleShape>,
    instance_key: usize,
) -> String {
    format!(
        "{}_{}",
        top_value_counter_storage_symbol(module),
        instance_key
    )
}

fn push_shared_module_symbol_identity(
    out: &mut String,
    module_name: &str,
    source_hash: u64,
    fallback_instance_key: Option<usize>,
) {
    push_symbol_component_hex(out, module_name);
    out.push('_');
    out.push_str(format!("{source_hash:016x}").as_str());
    if source_hash == 0 {
        if let Some(instance_key) = fallback_instance_key {
            out.push_str("_inst_");
            out.push_str(instance_key.to_string().as_str());
        }
    }
}

fn push_shared_module_symbol_identity_for_shared_state(
    out: &mut String,
    shared_state: &SharedModuleState,
) {
    push_shared_module_symbol_identity(
        out,
        shared_state.module_name.as_str(),
        shared_state.source_hash(),
        Some(shared_state.storage_instance_key()),
    );
}

fn module_constant_symbol_prefix_for_shared_state(shared_state: &SharedModuleState) -> String {
    let mut symbol = String::from("__soac_module_constant_shared_");
    push_shared_module_symbol_identity_for_shared_state(&mut symbol, shared_state);
    symbol
}

fn module_constant_symbol_prefix_for_module_identity(
    module_name: &str,
    source_hash: u64,
) -> String {
    let mut symbol = String::from("__soac_module_constant_shared_");
    push_shared_module_symbol_identity(&mut symbol, module_name, source_hash, None);
    symbol
}

fn scalar_counter_storage_symbol_for_shared_state(shared_state: &SharedModuleState) -> String {
    let mut symbol = String::from("__soac_scalar_counters_shared_");
    push_shared_module_symbol_identity_for_shared_state(&mut symbol, shared_state);
    symbol
}

fn top_value_counter_storage_symbol_for_shared_state(shared_state: &SharedModuleState) -> String {
    let mut symbol = String::from("__soac_top_value_counters_shared_");
    push_shared_module_symbol_identity_for_shared_state(&mut symbol, shared_state);
    symbol
}

fn direct_function_symbol_scope_for_shared_state(
    shared_state: &SharedModuleState,
    function_id: FunctionId,
) -> String {
    let mut scope = String::from("shared_");
    push_shared_module_symbol_identity_for_shared_state(&mut scope, shared_state);
    scope.push_str("_fn_");
    scope.push_str(function_id.packed().to_string().as_str());
    scope
}

fn precompiled_direct_function_symbol_scope_for_shared_state(
    shared_state: &SharedModuleState,
    function_id: FunctionId,
) -> String {
    precompiled_direct_function_symbol_scope_for_module_identity(
        shared_state.module_name.as_str(),
        shared_state.source_hash(),
        function_id,
    )
}

fn precompiled_direct_function_symbol_scope_for_module_identity(
    module_name: &str,
    source_hash: u64,
    function_id: FunctionId,
) -> String {
    let mut scope = String::from("shared_");
    push_shared_module_symbol_identity(&mut scope, module_name, source_hash, None);
    scope.push_str("_fn_");
    if source_hash == 0 {
        scope.push_str(function_id.packed().to_string().as_str());
    } else {
        scope.push_str(function_id.function_id().to_string().as_str());
    }
    scope
}

fn module_constant_object_symbol(symbol_prefix: &str, constant_id: ModuleConstantId) -> String {
    format!("{symbol_prefix}_object_{}", constant_id.0)
}

fn declare_module_constant_object_data_for_symbol(
    jit_module: &mut JITModule,
    symbol_prefix: &str,
    constant_id: ModuleConstantId,
    module_constant_ptr: *mut ffi::PyObject,
) -> Result<DataId, String> {
    let symbol = module_constant_object_symbol(symbol_prefix, constant_id);
    register_jit_data_symbol(symbol.as_str(), module_constant_ptr.cast::<u8>());
    jit_module
        .declare_data(symbol.as_str(), Linkage::Import, true, false)
        .map_err(|err| format!("failed to declare module constant object {symbol}: {err}"))
}

fn declare_module_constant_object_data(
    jit_module: &mut JITModule,
    module: &BlockPyModule<CodegenModuleShape>,
    module_constant_ptrs: &[*mut ffi::PyObject],
) -> Result<Vec<DataId>, String> {
    let instance_key = module as *const BlockPyModule<CodegenModuleShape> as usize;
    let symbol_prefix = module_constant_symbol_prefix_for_instance(module, instance_key);
    declare_module_constant_object_data_for_prefix(
        jit_module,
        symbol_prefix.as_str(),
        module_constant_ptrs,
    )
}

fn declare_module_constant_object_data_for_prefix(
    jit_module: &mut JITModule,
    symbol_prefix: &str,
    module_constant_ptrs: &[*mut ffi::PyObject],
) -> Result<Vec<DataId>, String> {
    module_constant_ptrs
        .iter()
        .copied()
        .enumerate()
        .map(|(index, ptr)| {
            declare_module_constant_object_data_for_symbol(
                jit_module,
                symbol_prefix,
                ModuleConstantId(index),
                ptr,
            )
        })
        .collect()
}

fn define_scalar_counter_storage_data_for_symbol(
    jit_module: &mut JITModule,
    symbol: &str,
    scalar_counter_count: usize,
) -> Result<DataId, String> {
    let data_id = jit_module
        .declare_data(symbol, Linkage::Local, true, false)
        .map_err(|err| format!("failed to declare scalar counter storage {symbol}: {err}"))?;
    let mut data = DataDescription::new();
    data.define_zeroinit(
        scalar_counter_count
            .checked_mul(std::mem::size_of::<u64>())
            .ok_or_else(|| {
                format!("scalar counter storage size overflow for {symbol}: {scalar_counter_count}")
            })?,
    );
    data.set_align(std::mem::align_of::<u64>() as u64);
    jit_module
        .define_data(data_id, &data)
        .map_err(|err| format!("failed to define scalar counter storage {symbol}: {err}"))?;
    Ok(data_id)
}

fn define_scalar_counter_storage_data(
    jit_module: &mut JITModule,
    module: &BlockPyModule<CodegenModuleShape>,
    scalar_counter_count: usize,
) -> Result<DataId, String> {
    define_scalar_counter_storage_data_for_symbol(
        jit_module,
        scalar_counter_storage_symbol(module).as_str(),
        scalar_counter_count,
    )
}

fn declare_scalar_counter_storage_import(
    jit_module: &mut JITModule,
    symbol: &str,
) -> Result<DataId, String> {
    jit_module
        .declare_data(symbol, Linkage::Import, true, false)
        .map_err(|err| format!("failed to declare imported scalar counter storage {symbol}: {err}"))
}

fn define_top_value_counter_storage_data_for_symbol(
    jit_module: &mut JITModule,
    symbol: &str,
    top_value_counter_count: usize,
) -> Result<DataId, String> {
    let data_id = jit_module
        .declare_data(symbol, Linkage::Local, true, false)
        .map_err(|err| format!("failed to declare top-value counter storage {symbol}: {err}"))?;
    let mut data = DataDescription::new();
    data.define_zeroinit(
        top_value_counter_count
            .checked_mul(std::mem::size_of::<TopValueCounter>())
            .ok_or_else(|| {
                format!(
                    "top-value counter storage size overflow for {symbol}: {top_value_counter_count}"
                )
            })?,
    );
    data.set_align(std::mem::align_of::<TopValueCounter>() as u64);
    jit_module
        .define_data(data_id, &data)
        .map_err(|err| format!("failed to define top-value counter storage {symbol}: {err}"))?;
    Ok(data_id)
}

fn define_top_value_counter_storage_data(
    jit_module: &mut JITModule,
    module: &BlockPyModule<CodegenModuleShape>,
    top_value_counter_count: usize,
) -> Result<DataId, String> {
    define_top_value_counter_storage_data_for_symbol(
        jit_module,
        top_value_counter_storage_symbol(module).as_str(),
        top_value_counter_count,
    )
}

fn declare_top_value_counter_storage_import(
    jit_module: &mut JITModule,
    symbol: &str,
) -> Result<DataId, String> {
    jit_module
        .declare_data(symbol, Linkage::Import, true, false)
        .map_err(|err| {
            format!("failed to declare imported top-value counter storage {symbol}: {err}")
        })
}

fn declare_type_ptr_import(jit_module: &mut JITModule, symbol: &str) -> Result<DataId, String> {
    jit_module
        .declare_data(symbol, Linkage::Import, true, false)
        .map_err(|err| format!("failed to declare imported type symbol {symbol}: {err}"))
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
    let Some(path) = crate::config::precompiled_library_path_from_env()? else {
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

#[derive(Clone, Copy, Debug)]
enum SigType {
    Pointer,
    I64,
    I32,
}

#[derive(Clone, Copy, Debug)]
struct StaticSignature {
    params: &'static [SigType],
    returns: &'static [SigType],
}

impl StaticSignature {
    const fn new(params: &'static [SigType], returns: &'static [SigType]) -> Self {
        Self { params, returns }
    }
}

#[derive(Debug)]
struct ImportSpec {
    symbol: &'static str,
    signature: StaticSignature,
    linkage: Linkage,
    internal_id: OnceLock<usize>,
}

impl ImportSpec {
    const fn new(
        symbol: &'static str,
        params: &'static [SigType],
        returns: &'static [SigType],
    ) -> Self {
        Self {
            symbol,
            signature: StaticSignature::new(params, returns),
            linkage: Linkage::Import,
            internal_id: OnceLock::new(),
        }
    }

    const fn local(
        symbol: &'static str,
        params: &'static [SigType],
        returns: &'static [SigType],
    ) -> Self {
        Self {
            symbol,
            signature: StaticSignature::new(params, returns),
            linkage: Linkage::Local,
            internal_id: OnceLock::new(),
        }
    }

    fn internal_id(&'static self) -> usize {
        *self
            .internal_id
            .get_or_init(|| NEXT_IMPORT_SPEC_ID.fetch_add(1, Ordering::Relaxed))
    }
}

static DP_JIT_INCREF_IMPORT: ImportSpec =
    ImportSpec::local(SOAC_RUNTIME_INCREF_SYMBOL, &[SigType::Pointer], &[]);
static DP_JIT_DECREF_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_DECREF_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[],
);
static SOAC_RUNTIME_INCREF_APPLIED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_INCREF_APPLIED_SYMBOL,
    &[SigType::Pointer],
    &[SigType::I32],
);
static SOAC_RUNTIME_DECREF_APPLIED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_DECREF_APPLIED_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);
static SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_SET_RAISED_EXCEPTION_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[],
);
static SOAC_RUNTIME_LOAD_GLOBAL_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL,
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
static SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL,
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
static SOAC_RUNTIME_LOAD_GLOBAL_SLOW_IMPORT: ImportSpec = ImportSpec::new(
    "soac_runtime_load_global_slow",
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
static SOAC_RUNTIME_STORE_GLOBAL_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_STORE_GLOBAL_SYMBOL,
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
static SOAC_RUNTIME_STORE_GLOBAL_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL,
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
static SOAC_RUNTIME_PROBE_FIELD_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_PROBE_FIELD_INDEXED_SYMBOL,
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
static SOAC_RUNTIME_STORE_FIELD_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL,
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
    ],
    &[SigType::I32],
);
static SOAC_RUNTIME_BUILTIN_ORD_I64_IMPORT: ImportSpec = ImportSpec::local(
    direct_abi::SOAC_RUNTIME_BUILTIN_ORD_I64_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I64],
);
static SOAC_RUNTIME_BUILTIN_CHR_I64_IMPORT: ImportSpec = ImportSpec::local(
    direct_abi::SOAC_RUNTIME_BUILTIN_CHR_I64_SYMBOL,
    &[SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
static SOAC_RUNTIME_BUILTIN_LEN_I64_IMPORT: ImportSpec = ImportSpec::local(
    direct_abi::SOAC_RUNTIME_BUILTIN_LEN_I64_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I64],
);
static SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I64],
);
static DP_JIT_RAISE_I64_OVERFLOW_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_raise_i64_overflow", &[], &[]);
static DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_py_call_positional_three",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
static DP_JIT_PY_CALL_OBJECT_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_py_call_object",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_PY_VECTORCALL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_py_vectorcall",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
static DP_JIT_NEXT_OR_SENTINEL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_next_or_sentinel",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_ENTER_RECURSIVE_CALL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_enter_recursive_call",
    &[SigType::Pointer],
    &[SigType::I32],
);
static PY_THREAD_STATE_GET_UNCHECKED_IMPORT: ImportSpec =
    ImportSpec::new("PyThreadState_GetUnchecked", &[], &[SigType::Pointer]);
static DP_JIT_PY_CALL_WITH_KW_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_py_call_with_kw",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_PYTYPE_GENERIC_ALLOC_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pytype_generic_alloc",
    &[SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
static DP_JIT_FINISH_CONSTRUCTOR_INIT_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_finish_constructor_init",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_LOAD_RUNTIME_OBJ_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_load_runtime_obj",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_PYOBJECT_GETATTR_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_getattr",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_PYOBJECT_SETATTR_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_setattr",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_PYOBJECT_GETITEM_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_getitem",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_PYOBJECT_SETITEM_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_setitem",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_PYOBJECT_TO_I64_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_to_i64",
    &[SigType::Pointer],
    &[SigType::I64],
);
static PYLONG_FROM_LONGLONG_IMPORT: ImportSpec =
    ImportSpec::new("PyLong_FromLongLong", &[SigType::I64], &[SigType::Pointer]);
static DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_record_top_value_sample",
    &[SigType::Pointer, SigType::I64],
    &[],
);
static DP_JIT_RAISE_DELETED_NAME_ERROR_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_raise_deleted_name_error", &[SigType::Pointer], &[]);
static DP_JIT_RAISE_MISSING_REQUIRED_ARGUMENT_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_raise_missing_required_argument", &[], &[]);
static DP_JIT_RAISE_SUPER_ARG_DELETED_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_raise_super_arg_deleted", &[], &[]);
static DP_JIT_MAKE_CELL_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_make_cell", &[SigType::Pointer], &[SigType::Pointer]);
static DP_JIT_LOAD_CELL_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_load_cell", &[SigType::Pointer], &[SigType::Pointer]);
static DP_JIT_STORE_CELL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_store_cell",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static SOAC_RUNTIME_TUPLE_NEW_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_TUPLE_NEW_SYMBOL,
    &[SigType::I64],
    &[SigType::Pointer],
);
static SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_SYMBOL,
    &[SigType::Pointer, SigType::I64, SigType::Pointer],
    &[],
);
static DP_JIT_IS_TRUE_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_is_true", &[SigType::Pointer], &[SigType::I32]);
static DP_JIT_RAISE_FROM_EXC_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_raise_from_exc",
    &[SigType::Pointer],
    &[SigType::I32],
);
static DP_JIT_PUSH_HANDLED_EXCEPTION_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_push_handled_exception",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_POP_HANDLED_EXCEPTION_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_pop_handled_exception", &[SigType::Pointer], &[]);
static DP_JIT_DEOPT_RESUME_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_deopt_resume",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
        SigType::I64,
    ],
    &[SigType::Pointer],
);
static DP_JIT_VECTORCALL_BIND_DIRECT_ARGS_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_vectorcall_bind_direct_args",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
    ],
    &[SigType::I32],
);
static DP_JIT_VECTORCALL_COMPILE_FUNCTION_ENV_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_vectorcall_compile_function_env",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
struct ModuleFuncImports {
    func_ids_by_internal_id: Vec<Option<FuncId>>,
    import_id_to_symbol: HashMap<u32, &'static str>,
    #[cfg(test)]
    func_id_to_symbol: HashMap<u32, &'static str>,
}

impl ModuleFuncImports {
    fn new() -> Self {
        Self {
            func_ids_by_internal_id: Vec::new(),
            import_id_to_symbol: HashMap::new(),
            #[cfg(test)]
            func_id_to_symbol: HashMap::new(),
        }
    }

    fn debug_symbols(&self) -> &HashMap<u32, &'static str> {
        &self.import_id_to_symbol
    }

    #[cfg(test)]
    fn debug_declared_symbols(&self) -> &HashMap<u32, &'static str> {
        &self.func_id_to_symbol
    }

    fn ensure_declared(
        &mut self,
        jit_module: &mut JITModule,
        spec: &'static ImportSpec,
    ) -> Result<FuncId, String> {
        let internal_id = spec.internal_id();
        if internal_id >= self.func_ids_by_internal_id.len() {
            self.func_ids_by_internal_id.resize(internal_id + 1, None);
        }
        if let Some(func_id) = self.func_ids_by_internal_id[internal_id] {
            return Ok(func_id);
        }
        let sig = lower_static_signature(jit_module, spec.signature);
        let func_id = match spec.linkage {
            Linkage::Import => declare_import_fn(jit_module, spec.symbol, &sig)?,
            Linkage::Local => declare_local_fn(jit_module, spec.symbol, &sig)?,
            linkage => {
                return Err(format!(
                    "unsupported linkage {linkage:?} for jit call spec {}",
                    spec.symbol
                ));
            }
        };
        self.func_ids_by_internal_id[internal_id] = Some(func_id);
        #[cfg(test)]
        self.func_id_to_symbol.insert(func_id.as_u32(), spec.symbol);
        if matches!(spec.linkage, Linkage::Import) {
            self.import_id_to_symbol
                .insert(func_id.as_u32(), spec.symbol);
        }
        Ok(func_id)
    }
}

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
        jit_module: &mut JITModule,
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
        let func_id = self.module_imports.ensure_declared(jit_module, spec)?;
        let func_ref = jit_module.declare_func_in_func(func_id, func);
        self.func_refs_by_internal_id[internal_id] = Some(func_ref);
        Ok(func_ref)
    }

    fn get_or_panic(
        &mut self,
        jit_module: &mut JITModule,
        func: &mut ir::Function,
        spec: &'static ImportSpec,
    ) -> ir::FuncRef {
        self.get(jit_module, func, spec).unwrap_or_else(|err| {
            panic!(
                "failed to bind import {} during JIT codegen: {}",
                spec.symbol, err
            )
        })
    }
}

#[derive(Debug, Clone)]
pub struct RenderedSpecializedClif {
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

struct DefinedJitFunction {
    function_id: FunctionId,
    function_qualname: String,
    param_count: usize,
    main_id: FuncId,
    main_symbol: String,
    default_adapter_id: Option<FuncId>,
    default_adapter_symbol: Option<String>,
    stats: JitCodegenStats,
    artifact: DefinedFunctionArtifact,
    default_adapter_artifact: Option<DefinedFunctionArtifact>,
    deopt_table: Arc<RuntimeJitDeoptTable>,
}

struct ProcessJitBatchFunction<'a> {
    function: BlockPyFunction<CodegenModuleShape>,
    source: ProcessJitBatchFunctionSource<'a>,
}

#[derive(Clone)]
enum ProcessJitBatchFunctionSource<'a> {
    ExplicitInputs,
    BorrowedSharedState(&'a crate::module_type::SharedModuleState),
    OwnedSharedState(Arc<crate::module_type::SharedModuleState>),
}

impl ProcessJitBatchFunctionSource<'_> {
    fn shared_state(&self) -> Option<&crate::module_type::SharedModuleState> {
        match self {
            Self::ExplicitInputs => None,
            Self::BorrowedSharedState(shared_state) => Some(shared_state),
            Self::OwnedSharedState(shared_state) => Some(shared_state.as_ref()),
        }
    }
}

pub(crate) struct ProcessJitEngine {
    state: Mutex<ProcessJitState>,
    vectorcall_trampolines: Mutex<HashMap<usize, VectorcallEntryFn>>,
}

struct ProcessJitState {
    jit_module: JITModule,
    direct_functions: HashMap<FunctionId, ProcessJitFunctionEntry>,
    module_constant_objects: HashMap<usize, ModuleConstantObjectBinding>,
    scalar_counter_storage: HashMap<usize, ScalarCounterStorageBinding>,
    top_value_counter_storage: HashMap<usize, TopValueCounterStorageBinding>,
    next_direct_symbol_id: u64,
}

#[derive(Clone)]
struct ModuleConstantObjectBinding {
    data_ids: Vec<DataId>,
}

#[derive(Clone, Copy)]
struct ScalarCounterStorageBinding {
    data_id: DataId,
    scalar_count: usize,
}

#[derive(Clone, Copy)]
struct TopValueCounterStorageBinding {
    data_id: DataId,
    top_value_count: usize,
}

#[derive(Clone)]
enum ProcessJitFunctionEntry {
    Declared {
        declared: DeclaredJitFunction,
        shape: ProcessJitFunctionShape,
    },
    Ready {
        declared: DeclaredJitFunction,
        shape: ProcessJitFunctionShape,
        compiled_handle: Arc<CompiledFunctionHandle>,
    },
}

#[derive(Clone, Eq, PartialEq)]
struct ProcessJitFunctionShape {
    qualname: String,
    param_count: usize,
}

impl ProcessJitFunctionShape {
    fn for_function(function: &BlockPyFunction<CodegenModuleShape>) -> Self {
        Self {
            qualname: function.names.qualname.clone(),
            param_count: function.params.len(),
        }
    }
}

impl ProcessJitFunctionEntry {
    fn declared(&self) -> DeclaredJitFunction {
        match self {
            Self::Declared { declared, .. } => declared.clone(),
            Self::Ready { declared, .. } => declared.clone(),
        }
    }

    fn shape(&self) -> &ProcessJitFunctionShape {
        match self {
            Self::Declared { shape, .. } | Self::Ready { shape, .. } => shape,
        }
    }

    fn ready_entry(&self) -> Option<(DeclaredJitFunction, Arc<CompiledFunctionHandle>)> {
        match self {
            Self::Ready {
                declared,
                compiled_handle,
                ..
            } => Some((declared.clone(), Arc::clone(compiled_handle))),
            Self::Declared { .. } => None,
        }
    }
}

impl ProcessJitState {
    fn new(compile_session: &crate::session::CompileSession) -> Result<Self, String> {
        Ok(Self {
            jit_module: new_jit_module(compile_session)?,
            direct_functions: HashMap::new(),
            module_constant_objects: HashMap::new(),
            scalar_counter_storage: HashMap::new(),
            top_value_counter_storage: HashMap::new(),
            next_direct_symbol_id: 0,
        })
    }

    fn ensure_module_constant_objects(
        &mut self,
        module_constant_ptrs: &[*mut ffi::PyObject],
        binding_key: usize,
        symbol_prefix: &str,
    ) -> Result<Vec<DataId>, String> {
        if let Some(binding) = self.module_constant_objects.get(&binding_key) {
            if binding.data_ids.len() != module_constant_ptrs.len() {
                return Err(format!(
                    "module constant object count mismatch for module instance {}: {} != {}",
                    binding_key,
                    binding.data_ids.len(),
                    module_constant_ptrs.len()
                ));
            }
            return Ok(binding.data_ids.clone());
        }
        let data_ids = declare_module_constant_object_data_for_prefix(
            &mut self.jit_module,
            symbol_prefix,
            module_constant_ptrs,
        )?;
        self.module_constant_objects.insert(
            binding_key,
            ModuleConstantObjectBinding {
                data_ids: data_ids.clone(),
            },
        );
        Ok(data_ids)
    }

    fn ensure_local_scalar_counter_storage(
        &mut self,
        module: &BlockPyModule<CodegenModuleShape>,
        scalar_counter_count: usize,
        instance_key: usize,
    ) -> Result<Option<DataId>, String> {
        if scalar_counter_count == 0 {
            return Ok(None);
        }
        if let Some(binding) = self.scalar_counter_storage.get(&instance_key).copied() {
            if binding.scalar_count != scalar_counter_count {
                return Err(format!(
                    "scalar counter storage length mismatch for module instance {}: {} != {}",
                    instance_key, binding.scalar_count, scalar_counter_count
                ));
            }
            return Ok(Some(binding.data_id));
        }
        let symbol = scalar_counter_storage_symbol_for_instance(module, instance_key);
        let data_id = define_scalar_counter_storage_data_for_symbol(
            &mut self.jit_module,
            symbol.as_str(),
            scalar_counter_count,
        )?;
        self.scalar_counter_storage.insert(
            instance_key,
            ScalarCounterStorageBinding {
                data_id,
                scalar_count: scalar_counter_count,
            },
        );
        Ok(Some(data_id))
    }

    fn ensure_local_top_value_counter_storage(
        &mut self,
        module: &BlockPyModule<CodegenModuleShape>,
        top_value_counter_count: usize,
        instance_key: usize,
    ) -> Result<Option<DataId>, String> {
        if top_value_counter_count == 0 {
            return Ok(None);
        }
        if let Some(binding) = self.top_value_counter_storage.get(&instance_key).copied() {
            if binding.top_value_count != top_value_counter_count {
                return Err(format!(
                    "top-value counter storage length mismatch for module instance {}: {} != {}",
                    instance_key, binding.top_value_count, top_value_counter_count
                ));
            }
            return Ok(Some(binding.data_id));
        }
        let symbol = top_value_counter_storage_symbol_for_instance(module, instance_key);
        let data_id = define_top_value_counter_storage_data_for_symbol(
            &mut self.jit_module,
            symbol.as_str(),
            top_value_counter_count,
        )?;
        self.top_value_counter_storage.insert(
            instance_key,
            TopValueCounterStorageBinding {
                data_id,
                top_value_count: top_value_counter_count,
            },
        );
        Ok(Some(data_id))
    }

    fn declare_direct_function(
        &mut self,
        function: &BlockPyFunction<CodegenModuleShape>,
        symbol_scope: Option<&str>,
    ) -> Result<DeclaredJitFunction, String> {
        let shape = ProcessJitFunctionShape::for_function(function);
        if let Some(entry) = self.direct_functions.get(&function.function_id) {
            if entry.shape() == &shape {
                return Ok(entry.declared());
            }
        }
        let owned_symbol_scope;
        let symbol_scope = if let Some(symbol_scope) = symbol_scope {
            symbol_scope
        } else {
            owned_symbol_scope =
                direct_function_symbol_scope(function.function_id, self.next_direct_symbol_id);
            self.next_direct_symbol_id = self.next_direct_symbol_id.wrapping_add(1);
            owned_symbol_scope.as_str()
        };
        let (_sig, declared) =
            declare_direct_function(&mut self.jit_module, function, Some(symbol_scope))?;
        self.direct_functions.insert(
            function.function_id,
            ProcessJitFunctionEntry::Declared {
                declared: declared.clone(),
                shape,
            },
        );
        Ok(declared)
    }

    fn is_direct_function_ready(&self, function_id: FunctionId) -> bool {
        self.direct_functions
            .get(&function_id)
            .is_some_and(|entry| entry.ready_entry().is_some())
    }

    fn ready_direct_function(
        &self,
        function: &BlockPyFunction<CodegenModuleShape>,
    ) -> Option<Arc<CompiledFunctionHandle>> {
        let entry = self.direct_functions.get(&function.function_id)?;
        (entry.shape() == &ProcessJitFunctionShape::for_function(function))
            .then(|| entry.ready_entry().map(|(_, handle)| handle))
            .flatten()
    }

    fn mark_direct_function_ready(
        &mut self,
        session: &Arc<crate::session::CompileSession>,
        function_id: FunctionId,
        code_ptr: *const u8,
        default_code_ptr: *const u8,
        param_count: usize,
        deopt_table: Arc<RuntimeJitDeoptTable>,
    ) -> Result<Arc<CompiledFunctionHandle>, String> {
        let Some(entry) = self.direct_functions.get(&function_id) else {
            return Err(format!(
                "process JIT function {function_id} was defined before declaration"
            ));
        };
        debug_assert_eq!(deopt_table.function_id(), function_id);
        let declared = entry.declared();
        let shape = entry.shape().clone();
        let compiled_handle = Arc::new(CompiledFunctionHandle::from_direct_entry(
            session,
            code_ptr,
            default_code_ptr,
            param_count,
            deopt_table,
        ));
        self.direct_functions.insert(
            function_id,
            ProcessJitFunctionEntry::Ready {
                declared,
                shape,
                compiled_handle: Arc::clone(&compiled_handle),
            },
        );
        Ok(compiled_handle)
    }
}

struct ProcessJitCompileGuard;

struct CompiledSpecializedRunner {
    _session: Arc<crate::session::CompileSession>,
    entry: Option<CompiledRunnerEntry>,
    direct_deopt_table: Option<Arc<RuntimeJitDeoptTable>>,
}

pub(crate) struct CompiledFunctionHandle {
    handle: ObjPtr,
}

pub(crate) struct DirectFunctionCompileResult {
    pub(crate) handle: Arc<CompiledFunctionHandle>,
    pub(crate) compiled: bool,
    pub(crate) stats: Option<JitCodegenStats>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct JitCodegenStats {
    pub(crate) clif_block_count: usize,
    pub(crate) clif_inst_count: usize,
    pub(crate) machine_code_size_bytes: usize,
    pub(crate) machine_code_block_count: usize,
    pub(crate) machine_code_edge_count: usize,
}

// The handle points to an immutable compiled runner after construction. The code memory is kept
// alive by the runner owner, and the raw handle is freed only when the final Arc drops this wrapper.
unsafe impl Send for CompiledFunctionHandle {}
unsafe impl Sync for CompiledFunctionHandle {}

impl CompiledFunctionHandle {
    fn from_direct_entry(
        session: &Arc<crate::session::CompileSession>,
        code_ptr: *const u8,
        default_code_ptr: *const u8,
        param_count: usize,
        deopt_table: Arc<RuntimeJitDeoptTable>,
    ) -> Self {
        Self {
            handle: new_compiled_direct_runner_handle(
                session,
                code_ptr,
                default_code_ptr,
                param_count,
                deopt_table,
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn direct_runner_info(&self) -> Result<(*const u8, *const u8, usize), String> {
        compiled_direct_runner_info(self.handle)
    }

    pub(crate) fn direct_code_ptr(&self) -> Result<ObjPtr, String> {
        compiled_direct_code_ptr(self.handle)
    }

    pub(crate) fn default_direct_code_ptr(&self) -> Result<ObjPtr, String> {
        compiled_default_direct_code_ptr(self.handle)
    }

    pub(crate) fn direct_deopt_table_ptr(&self) -> Result<ObjPtr, String> {
        compiled_direct_deopt_table_ptr(self.handle)
    }

    #[cfg(test)]
    pub(crate) fn direct_deopt_table(&self) -> Result<Arc<RuntimeJitDeoptTable>, String> {
        compiled_direct_deopt_table(self.handle)
    }

    #[cfg(test)]
    fn raw_handle(&self) -> ObjPtr {
        self.handle
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
        function_id = function.function_id.function_id(),
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
        plan_jit_deopt_resume_module(&shared_state.lowered_module, &value_facts)?;
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

impl Drop for CompiledFunctionHandle {
    fn drop(&mut self) {
        unsafe { free_cranelift_run_bb_specialized_cached(self.handle) };
        self.handle = std::ptr::null_mut();
    }
}

pub type VectorcallEntryFn = unsafe extern "C" fn(ObjPtr, *const ObjPtr, usize, ObjPtr) -> ObjPtr;

#[derive(Clone, Copy)]
enum CompiledRunnerEntry {
    Direct {
        code_ptr: *const u8,
        default_code_ptr: *const u8,
        param_count: usize,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeJitDeoptTable {
    function_id: FunctionId,
    function: Box<BlockPyFunction<CodegenModuleShape>>,
    module_constant_ptrs: Vec<ObjPtr>,
    points: Vec<RuntimeJitDeoptRecord>,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeJitDeoptRecord {
    id: PlannedJitDeoptPointId,
    resume_point: LocalEnvResumePoint,
    precision: LocalEnvResumeStatePrecision,
    locals: Vec<LocalEnvResumeBinding>,
    continuation: RuntimeJitDeoptContinuation,
}

pub(crate) struct RuntimeJitDeoptInvocation<'a> {
    table: &'a RuntimeJitDeoptTable,
    record: &'a RuntimeJitDeoptRecord,
    globals_obj: ObjPtr,
    function_data_obj: ObjPtr,
    live_values: &'a [ObjPtr],
}

pub(crate) struct RuntimeJitDeoptLocal<'a> {
    binding: &'a LocalEnvResumeBinding,
    value: ObjPtr,
    release_on_frame_exit: bool,
}

pub(crate) struct RuntimeJitDeoptLocals<'a> {
    locals: Vec<RuntimeJitDeoptLocal<'a>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeJitDeoptCursor {
    block: BlockLabel,
    body_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeJitDeoptUnsupportedReason {
    WrongFunction,
    MissingFunction,
    MissingBlock,
    MissingInstruction,
    MissingPlanRecord,
    UnsupportedBlockTail,
    ReplayUnsafeGuardOperand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeJitDeoptContinuation {
    Unsupported {
        reason: RuntimeJitDeoptUnsupportedReason,
    },
    ResumeBlockTail {
        cursor: RuntimeJitDeoptCursor,
    },
}

impl RuntimeJitDeoptCursor {
    pub(crate) fn new(block: BlockLabel, body_index: usize) -> Self {
        Self { block, body_index }
    }

    pub(crate) fn block(self) -> BlockLabel {
        self.block
    }

    pub(crate) fn body_index(self) -> usize {
        self.body_index
    }

    pub(crate) fn at_block_entry(block: BlockLabel) -> Self {
        Self::new(block, 0)
    }
}

impl RuntimeJitDeoptContinuation {
    pub(crate) fn unsupported(reason: RuntimeJitDeoptUnsupportedReason) -> Self {
        Self::Unsupported { reason }
    }

    pub(crate) fn initial_cursor(&self) -> Option<RuntimeJitDeoptCursor> {
        match self {
            RuntimeJitDeoptContinuation::ResumeBlockTail { cursor } => Some(*cursor),
            RuntimeJitDeoptContinuation::Unsupported { .. } => None,
        }
    }

    pub(crate) fn unsupported_reason(&self) -> Option<RuntimeJitDeoptUnsupportedReason> {
        match self {
            RuntimeJitDeoptContinuation::Unsupported { reason } => Some(*reason),
            RuntimeJitDeoptContinuation::ResumeBlockTail { .. } => None,
        }
    }
}

impl RuntimeJitDeoptRecord {
    #[cfg(test)]
    pub(crate) fn id(&self) -> PlannedJitDeoptPointId {
        self.id
    }

    pub(crate) fn ordinal(&self) -> usize {
        self.id.ordinal
    }

    #[cfg(test)]
    pub(crate) fn resume_point(&self) -> LocalEnvResumePoint {
        self.resume_point
    }

    #[cfg(test)]
    pub(crate) fn precision(&self) -> LocalEnvResumeStatePrecision {
        self.precision
    }

    pub(crate) fn locals(&self) -> &[LocalEnvResumeBinding] {
        &self.locals
    }

    pub(crate) fn initial_cursor(&self) -> Option<RuntimeJitDeoptCursor> {
        self.continuation.initial_cursor()
    }

    #[cfg(test)]
    pub(crate) fn continuation(&self) -> &RuntimeJitDeoptContinuation {
        &self.continuation
    }

    pub(crate) fn validate_live_value_buffer(
        &self,
        live_values: ObjPtr,
        live_value_count: i64,
    ) -> Result<(), String> {
        let count = usize::try_from(live_value_count).map_err(|_| {
            format!("live value count {live_value_count} is negative or does not fit usize")
        })?;
        if count != self.locals.len() {
            return Err(format!(
                "deopt record {} expected {} live values but got {}",
                self.ordinal(),
                self.locals.len(),
                count
            ));
        }
        if count != 0 && live_values.is_null() {
            return Err(format!(
                "deopt record {} expected a non-null live value buffer for {} values",
                self.ordinal(),
                count
            ));
        }
        Ok(())
    }

    pub(crate) fn describe(&self, function_id: FunctionId) -> String {
        format!(
            "function {}, record {}, resume_point {:?}, precision {:?}, locals {}, continuation {:?}",
            function_id,
            self.ordinal(),
            self.resume_point,
            self.precision,
            self.locals.len(),
            self.continuation
        )
    }
}

impl RuntimeJitDeoptTable {
    fn from_plan(
        function: &BlockPyFunction<CodegenModuleShape>,
        plan: &PlannedJitDeoptResumeFunction,
        module_constant_ptrs: &[*mut ffi::PyObject],
    ) -> Result<Self, String> {
        let mut points = Vec::with_capacity(plan.deopt_points.len());
        for deopt_point in &plan.deopt_points {
            let entry = plan.entry(deopt_point.resume_point).ok_or_else(|| {
                format!(
                    "planned deopt point {:?} for function {} has no resume entry",
                    deopt_point.point, function.function_id
                )
            })?;
            points.push(RuntimeJitDeoptRecord {
                id: deopt_point.id,
                resume_point: deopt_point.resume_point,
                precision: deopt_point.precision,
                locals: entry.locals.clone(),
                continuation: runtime_jit_deopt_continuation_for_point(
                    function,
                    deopt_point.resume_point,
                ),
            });
        }
        let table = Self {
            function_id: function.function_id,
            function: Box::new(function.clone()),
            module_constant_ptrs: module_constant_ptrs
                .iter()
                .map(|ptr| ptr.cast::<c_void>())
                .collect(),
            points,
        };
        table.validate_against_plan(plan)?;
        Ok(table)
    }

    fn validate_against_plan(&self, plan: &PlannedJitDeoptResumeFunction) -> Result<(), String> {
        if self.points.len() != plan.deopt_points.len() {
            return Err(format!(
                "runtime JIT deopt table for function {} has {} points, expected {}",
                self.function_id,
                self.points.len(),
                plan.deopt_points.len()
            ));
        }
        for (record, planned) in self.points.iter().zip(plan.deopt_points.iter()) {
            if record.id != planned.id
                || record.resume_point != planned.resume_point
                || record.precision != planned.precision
            {
                return Err(format!(
                    "runtime JIT deopt table record {:?} does not match planned point {:?}",
                    record.id, planned.id
                ));
            }
            let Some(entry) = plan.entry(planned.resume_point) else {
                return Err(format!(
                    "runtime JIT deopt table record {:?} references missing resume point {:?}",
                    record.id, planned.resume_point
                ));
            };
            if record.locals != entry.locals {
                return Err(format!(
                    "runtime JIT deopt table record {:?} has stale local materialization",
                    record.id
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn function_id(&self) -> FunctionId {
        self.function_id
    }

    pub(crate) fn function(&self) -> &BlockPyFunction<CodegenModuleShape> {
        self.function.as_ref()
    }

    pub(crate) fn module_constant_ptr(
        &self,
        constant_id: ModuleConstantId,
    ) -> Result<ObjPtr, String> {
        self.module_constant_ptrs
            .get(constant_id.0)
            .copied()
            .ok_or_else(|| {
                format!(
                    "deopt table for function {} is missing module constant {}",
                    self.function_id, constant_id.0
                )
            })
    }

    pub(crate) fn record_for_ordinal(
        &self,
        record_ordinal: i64,
    ) -> Result<&RuntimeJitDeoptRecord, String> {
        let ordinal = usize::try_from(record_ordinal).map_err(|_| {
            format!(
                "deopt record ordinal {record_ordinal} is negative or does not fit usize for function {}",
                self.function_id
            )
        })?;
        let record = self.points.get(ordinal).ok_or_else(|| {
            format!(
                "deopt record ordinal {ordinal} is outside table for function {} with {} records",
                self.function_id,
                self.points.len()
            )
        })?;
        if record.id.ordinal != ordinal {
            return Err(format!(
                "deopt record ordinal {ordinal} resolves to stale record {:?}",
                record.id
            ));
        }
        Ok(record)
    }

    #[cfg(test)]
    pub(crate) fn describe_record_ordinal(&self, record_ordinal: i64) -> Result<String, String> {
        self.record_for_ordinal(record_ordinal)
            .map(|record| record.describe(self.function_id))
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.points.len()
    }

    #[cfg(test)]
    fn record_for_point(&self, point: LocalEnvResumePoint) -> Option<&RuntimeJitDeoptRecord> {
        self.points
            .iter()
            .find(|record| record.resume_point == point)
    }
}

fn runtime_jit_deopt_continuation_for_point(
    function: &BlockPyFunction<CodegenModuleShape>,
    point: LocalEnvResumePoint,
) -> RuntimeJitDeoptContinuation {
    match point {
        LocalEnvResumePoint::BeforeTerm { function_id, block } => {
            if function_id != function.function_id {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::WrongFunction,
                );
            }
            let Some(block) = function
                .blocks
                .iter()
                .find(|candidate| candidate.label == block)
            else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingBlock,
                );
            };
            if runtime_jit_deopt_block_tail_supported(function, block, block.body.len()) {
                RuntimeJitDeoptContinuation::ResumeBlockTail {
                    cursor: RuntimeJitDeoptCursor::new(block.label, block.body.len()),
                }
            } else {
                RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
                )
            }
        }
        LocalEnvResumePoint::BeforeInstr { key } => {
            if key.function_id != function.function_id {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::WrongFunction,
                );
            }
            let block_label = key.instr_id.block_label();
            let Some(block) = function
                .blocks
                .iter()
                .find(|candidate| candidate.label == block_label)
            else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingBlock,
                );
            };
            let Some(start_body_index) = block
                .body
                .iter()
                .position(|instr| instr.try_semantic_instr_id() == Some(key.instr_id))
            else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingInstruction,
                );
            };
            if runtime_jit_deopt_block_tail_supported(function, block, start_body_index) {
                RuntimeJitDeoptContinuation::ResumeBlockTail {
                    cursor: RuntimeJitDeoptCursor::new(block_label, start_body_index),
                }
            } else {
                RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
                )
            }
        }
        LocalEnvResumePoint::BlockEntry { function_id, block } => {
            if function_id != function.function_id {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::WrongFunction,
                );
            }
            let Some(block) = function
                .blocks
                .iter()
                .find(|candidate| candidate.label == block)
            else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingBlock,
                );
            };
            if runtime_jit_deopt_block_tail_supported(function, block, 0) {
                RuntimeJitDeoptContinuation::ResumeBlockTail {
                    cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
                }
            } else {
                RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
                )
            }
        }
    }
}

fn runtime_jit_deopt_guard_miss_supported(
    function: &BlockPyFunction<CodegenModuleShape>,
    point: LocalEnvResumePoint,
    pre_guard_operands: &[&InstrCodegen],
) -> Result<(), RuntimeJitDeoptUnsupportedReason> {
    if let Some(reason) =
        runtime_jit_deopt_continuation_for_point(function, point).unsupported_reason()
    {
        return Err(reason);
    }
    if pre_guard_operands
        .iter()
        .any(|expr| !runtime_jit_deopt_guard_operand_replay_safe(expr))
    {
        return Err(RuntimeJitDeoptUnsupportedReason::ReplayUnsafeGuardOperand);
    }
    Ok(())
}

fn runtime_jit_deopt_guard_operand_replay_safe(expr: &InstrCodegen) -> bool {
    matches!(
        expr,
        InstrCodegen::Load(load)
            if matches!(
                load.name.location,
                NameLocation::Local(_) | NameLocation::Cell(_) | NameLocation::Constant(_)
            )
    )
}

fn typed_nested_guard_misses_can_resume_before_instr(expr: &InstrTyped) -> bool {
    let mut saw_replay_unsafe_effect = false;
    typed_nested_guard_scan_expr(expr, &mut saw_replay_unsafe_effect)
}

fn nested_guard_candidate_seen_before_replay_unsafe_effect(
    has_guard_candidate: bool,
    saw_replay_unsafe_effect: bool,
) -> bool {
    !has_guard_candidate || !saw_replay_unsafe_effect
}

fn typed_nested_guard_scan_expr(expr: &InstrTyped, saw_replay_unsafe_effect: &mut bool) -> bool {
    match expr {
        InstrTyped::Truthy(op) => {
            typed_nested_guard_scan_expr(op.value(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::Load(op) => nested_guard_candidate_seen_before_replay_unsafe_effect(
            matches!(op.name.location, NameLocation::Global(_)),
            *saw_replay_unsafe_effect,
        ),
        InstrTyped::BinOp(op) => {
            typed_nested_guard_scan_expr(op.left.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.right.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyUnaryOp(op) => {
            typed_nested_guard_scan_expr(op.operand.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyTuple(op) => op
            .values
            .iter()
            .all(|value| typed_nested_guard_scan_expr(value, saw_replay_unsafe_effect)),
        InstrTyped::LegacyCalleeFunctionId(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyCall(op) => {
            if op.args.is_empty()
                && op.keywords.is_empty()
                && let InstrTyped::LegacyGetAttr(getattr) = op.func.as_ref()
            {
                // Direct-method guard code evaluates only the receiver before the guard.
                // Keep this no-arg only until argument guard points carry their own
                // precise resume state.
                return typed_nested_guard_scan_expr(
                    getattr.value.as_ref(),
                    saw_replay_unsafe_effect,
                ) && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                ) && mark_replay_unsafe_effect(saw_replay_unsafe_effect);
            }
            typed_nested_guard_scan_expr(op.func.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_positional_args(
                    op.args.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && typed_nested_guard_scan_keyword_args(
                    op.keywords.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyCallDirect(op) => {
            typed_nested_guard_scan_expr(op.callable.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_positional_args(
                    op.args.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && typed_nested_guard_scan_keyword_args(
                    op.keywords.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyGetAttr(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.attr.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacySetAttr(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.attr.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.replacement.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyGetItem(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.index.as_ref(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacySetItem(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.index.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.replacement.as_ref(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyDelItem(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.index.as_ref(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyStore(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    matches!(op.name.location, NameLocation::Global(_)),
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyDel(_) => mark_replay_unsafe_effect(saw_replay_unsafe_effect),
        InstrTyped::LegacyMakeCell(op) => {
            op.initial_value.as_ref().map_or(true, |initial_value| {
                typed_nested_guard_scan_expr(initial_value.as_ref(), saw_replay_unsafe_effect)
            }) && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyIncrementCounter(_) => {
            mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::LegacyCellRef(_) => true,
        InstrTyped::LegacyMakeFunctionWithClosure(op) => {
            typed_nested_guard_scan_expr(op.captures.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(
                    op.param_defaults.as_ref(),
                    saw_replay_unsafe_effect,
                )
                && typed_nested_guard_scan_expr(op.annotate_fn.as_ref(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
    }
}

fn typed_nested_guard_scan_positional_args(
    args: &[CallArgPositional<InstrTyped>],
    saw_replay_unsafe_effect: &mut bool,
) -> bool {
    args.iter().all(|arg| match arg {
        CallArgPositional::Positional(expr) | CallArgPositional::Starred(expr) => {
            typed_nested_guard_scan_expr(expr, saw_replay_unsafe_effect)
        }
    })
}

fn typed_nested_guard_scan_keyword_args(
    keywords: &[CallArgKeyword<InstrTyped>],
    saw_replay_unsafe_effect: &mut bool,
) -> bool {
    keywords.iter().all(|keyword| match keyword {
        CallArgKeyword::Named { value, .. } | CallArgKeyword::Starred(value) => {
            typed_nested_guard_scan_expr(value, saw_replay_unsafe_effect)
        }
    })
}

fn mark_replay_unsafe_effect(saw_replay_unsafe_effect: &mut bool) -> bool {
    *saw_replay_unsafe_effect = true;
    true
}

fn runtime_jit_deopt_block_tail_supported(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &CodegenBlock,
    start_body_index: usize,
) -> bool {
    let Some(body_tail) = block.body.get(start_body_index..) else {
        return false;
    };
    let support = RuntimeJitDeoptSupportCtx::new(function);
    body_tail
        .iter()
        .all(|expr| runtime_jit_deopt_expr_supported(expr, &support))
        && runtime_jit_deopt_term_supported(&block.term, &support)
        && block
            .exc_edge
            .as_ref()
            .is_none_or(runtime_jit_deopt_exception_edge_supported)
}

struct RuntimeJitDeoptSupportCtx<'a> {
    storage_layout: Option<&'a StorageLayout>,
    runtime_layout: FunctionRuntimeDataLayout,
}

impl<'a> RuntimeJitDeoptSupportCtx<'a> {
    fn new(function: &'a BlockPyFunction<CodegenModuleShape>) -> Self {
        RuntimeJitDeoptSupportCtx {
            storage_layout: function.storage_layout.as_ref(),
            runtime_layout: FunctionRuntimeDataLayout::from_function(function),
        }
    }

    fn owned_cell_supported(&self, slot: u32) -> bool {
        self.storage_layout
            .and_then(|layout| layout.local_cell_slot(slot))
            .is_some()
    }

    fn closure_cell_supported(&self, slot: u32) -> bool {
        (slot as usize) < self.runtime_layout.closure_len()
    }
}

fn runtime_jit_deopt_expr_supported(
    expr: &InstrCodegen,
    support: &RuntimeJitDeoptSupportCtx<'_>,
) -> bool {
    match expr {
        InstrCodegen::Load(load) => match load.name.location {
            NameLocation::Cell(CellLocation::Owned(slot)) => support.owned_cell_supported(slot),
            NameLocation::Cell(CellLocation::Closure(slot))
            | NameLocation::Cell(CellLocation::CapturedSource(slot)) => {
                support.closure_cell_supported(slot)
            }
            _ => true,
        },
        InstrCodegen::BinOp(binop) => {
            runtime_jit_deopt_binop_supported(binop.kind)
                && runtime_jit_deopt_expr_supported(&binop.left, support)
                && runtime_jit_deopt_expr_supported(&binop.right, support)
        }
        InstrCodegen::UnaryOp(unary) => runtime_jit_deopt_expr_supported(&unary.operand, support),
        InstrCodegen::Tuple(tuple) => tuple
            .values
            .iter()
            .all(|value| runtime_jit_deopt_expr_supported(value, support)),
        InstrCodegen::GetAttr(getattr) => {
            runtime_jit_deopt_expr_supported(&getattr.value, support)
                && runtime_jit_deopt_expr_supported(&getattr.attr, support)
        }
        InstrCodegen::GetItem(getitem) => {
            runtime_jit_deopt_expr_supported(&getitem.value, support)
                && runtime_jit_deopt_expr_supported(&getitem.index, support)
        }
        InstrCodegen::SetAttr(setattr) => {
            runtime_jit_deopt_expr_supported(&setattr.value, support)
                && runtime_jit_deopt_expr_supported(&setattr.attr, support)
                && runtime_jit_deopt_expr_supported(&setattr.replacement, support)
        }
        InstrCodegen::SetItem(setitem) => {
            runtime_jit_deopt_expr_supported(&setitem.value, support)
                && runtime_jit_deopt_expr_supported(&setitem.index, support)
                && runtime_jit_deopt_expr_supported(&setitem.replacement, support)
        }
        InstrCodegen::DelItem(delitem) => {
            runtime_jit_deopt_expr_supported(&delitem.value, support)
                && runtime_jit_deopt_expr_supported(&delitem.index, support)
        }
        InstrCodegen::CalleeFunctionId(callee) => {
            runtime_jit_deopt_expr_supported(&callee.value, support)
        }
        InstrCodegen::Call(call) => {
            runtime_jit_deopt_call_parts_supported(&call.func, &call.args, &call.keywords, support)
        }
        InstrCodegen::CallDirect(call) => runtime_jit_deopt_call_parts_supported(
            &call.callable,
            &call.args,
            &call.keywords,
            support,
        ),
        InstrCodegen::Store(store) => {
            runtime_jit_deopt_name_location_supported(store.name.location, support)
                && runtime_jit_deopt_expr_supported(&store.value, support)
        }
        InstrCodegen::Del(del) => {
            runtime_jit_deopt_name_location_supported(del.name.location, support)
        }
        InstrCodegen::IncrementCounter(_) => true,
        InstrCodegen::MakeCell(make_cell) => make_cell
            .initial_value
            .as_ref()
            .map_or(true, |initial_value| {
                runtime_jit_deopt_expr_supported(initial_value, support)
            }),
        InstrCodegen::MakeFunctionWithClosure(make_function) => {
            runtime_jit_deopt_expr_supported(&make_function.captures, support)
                && runtime_jit_deopt_expr_supported(&make_function.param_defaults, support)
                && runtime_jit_deopt_expr_supported(&make_function.annotate_fn, support)
        }
        InstrCodegen::CellRef(cell_ref) => match cell_ref.location {
            CellLocation::Owned(slot) => support.owned_cell_supported(slot),
            CellLocation::Closure(slot) | CellLocation::CapturedSource(slot) => {
                support.closure_cell_supported(slot)
            }
        },
    }
}

fn runtime_jit_deopt_name_location_supported(
    location: NameLocation,
    support: &RuntimeJitDeoptSupportCtx<'_>,
) -> bool {
    match location {
        NameLocation::Cell(CellLocation::Owned(slot)) => support.owned_cell_supported(slot),
        NameLocation::Cell(CellLocation::Closure(slot))
        | NameLocation::Cell(CellLocation::CapturedSource(slot)) => {
            support.closure_cell_supported(slot)
        }
        _ => true,
    }
}

fn runtime_jit_deopt_call_parts_supported(
    callable: &InstrCodegen,
    args: &[CallArgPositional<InstrCodegen>],
    keywords: &[CallArgKeyword<InstrCodegen>],
    support: &RuntimeJitDeoptSupportCtx<'_>,
) -> bool {
    runtime_jit_deopt_expr_supported(callable, support)
        && args.iter().all(|arg| match arg {
            CallArgPositional::Positional(expr) => runtime_jit_deopt_expr_supported(expr, support),
            CallArgPositional::Starred(expr) => runtime_jit_deopt_expr_supported(expr, support),
        })
        && keywords.iter().all(|keyword| match keyword {
            CallArgKeyword::Named { value, .. } => runtime_jit_deopt_expr_supported(value, support),
            CallArgKeyword::Starred(value) => runtime_jit_deopt_expr_supported(value, support),
        })
}

fn runtime_jit_deopt_binop_supported(kind: blockpy_intrinsics::BinOpKind) -> bool {
    matches!(
        kind,
        blockpy_intrinsics::BinOpKind::Add
            | blockpy_intrinsics::BinOpKind::Sub
            | blockpy_intrinsics::BinOpKind::Mul
            | blockpy_intrinsics::BinOpKind::MatMul
            | blockpy_intrinsics::BinOpKind::TrueDiv
            | blockpy_intrinsics::BinOpKind::FloorDiv
            | blockpy_intrinsics::BinOpKind::Mod
            | blockpy_intrinsics::BinOpKind::Pow
            | blockpy_intrinsics::BinOpKind::LShift
            | blockpy_intrinsics::BinOpKind::RShift
            | blockpy_intrinsics::BinOpKind::Or
            | blockpy_intrinsics::BinOpKind::Xor
            | blockpy_intrinsics::BinOpKind::And
            | blockpy_intrinsics::BinOpKind::Eq
            | blockpy_intrinsics::BinOpKind::Ne
            | blockpy_intrinsics::BinOpKind::Lt
            | blockpy_intrinsics::BinOpKind::Le
            | blockpy_intrinsics::BinOpKind::Gt
            | blockpy_intrinsics::BinOpKind::Ge
            | blockpy_intrinsics::BinOpKind::Contains
            | blockpy_intrinsics::BinOpKind::Is
            | blockpy_intrinsics::BinOpKind::InplaceAdd
            | blockpy_intrinsics::BinOpKind::InplaceSub
            | blockpy_intrinsics::BinOpKind::InplaceMul
            | blockpy_intrinsics::BinOpKind::InplaceMatMul
            | blockpy_intrinsics::BinOpKind::InplaceTrueDiv
            | blockpy_intrinsics::BinOpKind::InplaceFloorDiv
            | blockpy_intrinsics::BinOpKind::InplaceMod
            | blockpy_intrinsics::BinOpKind::InplacePow
            | blockpy_intrinsics::BinOpKind::InplaceLShift
            | blockpy_intrinsics::BinOpKind::InplaceRShift
            | blockpy_intrinsics::BinOpKind::InplaceOr
            | blockpy_intrinsics::BinOpKind::InplaceXor
            | blockpy_intrinsics::BinOpKind::InplaceAnd
    )
}

fn runtime_jit_deopt_term_supported(
    term: &BlockTerm<InstrCodegen>,
    support: &RuntimeJitDeoptSupportCtx<'_>,
) -> bool {
    match term {
        BlockTerm::Return(value) => runtime_jit_deopt_expr_supported(value, support),
        BlockTerm::Jump(edge) => edge.args.iter().all(|arg| {
            matches!(
                arg,
                BlockArg::Name(_)
                    | BlockArg::None
                    | BlockArg::CurrentException
                    | BlockArg::AbruptKind(_)
            )
        }),
        BlockTerm::IfTerm(if_term) => runtime_jit_deopt_expr_supported(&if_term.test, support),
        BlockTerm::BranchTable(branch) => runtime_jit_deopt_expr_supported(&branch.index, support),
        BlockTerm::Raise(raise) => raise
            .exc
            .as_ref()
            .is_none_or(|exc| runtime_jit_deopt_expr_supported(exc, support)),
    }
}

fn runtime_jit_deopt_exception_edge_supported(edge: &BlockEdge) -> bool {
    edge.args.iter().all(|arg| {
        matches!(
            arg,
            BlockArg::Name(_) | BlockArg::None | BlockArg::CurrentException
        )
    })
}

impl RuntimeJitDeoptInvocation<'_> {
    pub(crate) unsafe fn from_raw<'a>(
        deopt_table: ObjPtr,
        globals_obj: ObjPtr,
        function_data_obj: ObjPtr,
        record_ordinal: i64,
        live_values: ObjPtr,
        live_value_count: i64,
    ) -> Result<RuntimeJitDeoptInvocation<'a>, String> {
        if deopt_table.is_null() {
            return Err(format!(
                "null deopt table pointer, ordinal {record_ordinal}, live values {live_value_count}"
            ));
        }
        let table = unsafe { &*(deopt_table.cast::<RuntimeJitDeoptTable>()) };
        let record = table.record_for_ordinal(record_ordinal)?;
        record.validate_live_value_buffer(live_values, live_value_count)?;
        let live_value_count = usize::try_from(live_value_count).map_err(|_| {
            format!("live value count {live_value_count} is negative or does not fit usize")
        })?;
        let live_values = if live_value_count == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(live_values.cast::<ObjPtr>(), live_value_count) }
        };
        Ok(RuntimeJitDeoptInvocation {
            table,
            record,
            globals_obj,
            function_data_obj,
            live_values,
        })
    }

    pub(crate) fn record(&self) -> &'_ RuntimeJitDeoptRecord {
        self.record
    }

    pub(crate) fn function(&self) -> &BlockPyFunction<CodegenModuleShape> {
        self.table.function()
    }

    pub(crate) fn globals_obj(&self) -> ObjPtr {
        self.globals_obj
    }

    pub(crate) fn function_data_obj(&self) -> ObjPtr {
        self.function_data_obj
    }

    pub(crate) fn module_constant_ptr(&self, constant_index: u32) -> Result<ObjPtr, String> {
        self.table
            .module_constant_ptr(ModuleConstantId(constant_index as usize))
    }

    pub(crate) fn live_bindings(
        &self,
    ) -> impl ExactSizeIterator<Item = (&'_ LocalEnvResumeBinding, ObjPtr)> + '_ {
        self.record
            .locals()
            .iter()
            .zip(self.live_values.iter().copied())
    }

    pub(crate) fn materialize_locals(&self) -> Result<RuntimeJitDeoptLocals<'_>, String> {
        RuntimeJitDeoptLocals::from_live_bindings(self.live_bindings())
    }

    pub(crate) fn describe(&self) -> String {
        format!(
            "{}, live values {}",
            self.record().describe(self.table.function_id()),
            self.live_bindings().len()
        )
    }
}

impl<'a> RuntimeJitDeoptLocals<'a> {
    pub(crate) fn from_live_bindings(
        live_bindings: impl IntoIterator<Item = (&'a LocalEnvResumeBinding, ObjPtr)>,
    ) -> Result<Self, String> {
        let mut names = HashSet::new();
        let mut locations = HashSet::new();
        let mut locals = Vec::new();
        for (binding, value) in live_bindings {
            if !names.insert(binding.name.as_str()) {
                return Err(format!(
                    "duplicate deopt local name {} while reconstructing runtime locals",
                    binding.name
                ));
            }
            if !locations.insert(binding.location) {
                return Err(format!(
                    "duplicate deopt local location {:?} while reconstructing runtime locals",
                    binding.location
                ));
            }
            match binding.binding {
                LocalEnvResumeBindingState::Bound if value.is_null() => {
                    return Err(format!(
                        "deopt local {} at {:?} is definitely bound but has a null value",
                        binding.name, binding.location
                    ));
                }
                LocalEnvResumeBindingState::Unbound if !value.is_null() => {
                    return Err(format!(
                        "deopt local {} at {:?} is unbound but has a non-null value",
                        binding.name, binding.location
                    ));
                }
                _ => {}
            }
            locals.push(RuntimeJitDeoptLocal {
                binding,
                value,
                release_on_frame_exit: transient_local_needs_decref(binding.ownership),
            });
        }
        Ok(Self { locals })
    }

    pub(crate) fn len(&self) -> usize {
        self.locals.len()
    }

    pub(crate) fn describe(&self) -> String {
        let names = self
            .locals
            .iter()
            .map(|local| format!("{}={:p}", local.binding.name, local.value))
            .collect::<Vec<_>>()
            .join(", ");
        format!("reconstructed locals {} [{}]", self.len(), names)
    }

    pub(crate) fn get_by_name(&self, name: &str) -> Option<&RuntimeJitDeoptLocal<'a>> {
        self.locals.iter().find(|local| local.binding.name == name)
    }

    pub(crate) fn get_by_name_mut(&mut self, name: &str) -> Option<&mut RuntimeJitDeoptLocal<'a>> {
        self.locals
            .iter_mut()
            .find(|local| local.binding.name == name)
    }

    pub(crate) fn get_by_location(
        &self,
        location: LocalLocation,
    ) -> Option<&RuntimeJitDeoptLocal<'a>> {
        self.locals
            .iter()
            .find(|local| local.binding.location == location)
    }

    pub(crate) fn get_by_location_mut(
        &mut self,
        location: LocalLocation,
    ) -> Option<&mut RuntimeJitDeoptLocal<'a>> {
        self.locals
            .iter_mut()
            .find(|local| local.binding.location == location)
    }

    pub(crate) unsafe fn release_frame_owned_values(&mut self) {
        for local in &mut self.locals {
            unsafe {
                local.release_frame_owned_value();
            }
        }
    }
}

impl RuntimeJitDeoptLocal<'_> {
    pub(crate) fn binding(&self) -> &'_ LocalEnvResumeBinding {
        self.binding
    }

    pub(crate) fn value(&self) -> ObjPtr {
        self.value
    }

    pub(crate) unsafe fn replace_with_owned_value(&mut self, value: ObjPtr) {
        unsafe {
            self.release_frame_owned_value();
        }
        self.value = value;
        self.release_on_frame_exit = true;
    }

    pub(crate) unsafe fn delete_value(&mut self) {
        unsafe {
            self.release_frame_owned_value();
        }
    }

    unsafe fn release_frame_owned_value(&mut self) {
        if self.release_on_frame_exit && !self.value.is_null() {
            unsafe {
                ffi::Py_DECREF(self.value.cast::<ffi::PyObject>());
            }
        }
        self.value = std::ptr::null_mut();
        self.release_on_frame_exit = false;
    }
}

fn new_compiled_direct_runner_handle(
    session: &Arc<crate::session::CompileSession>,
    code_ptr: *const u8,
    default_code_ptr: *const u8,
    param_count: usize,
    deopt_table: Arc<RuntimeJitDeoptTable>,
) -> ObjPtr {
    Box::into_raw(Box::new(CompiledSpecializedRunner {
        _session: Arc::clone(session),
        entry: Some(CompiledRunnerEntry::Direct {
            code_ptr,
            default_code_ptr,
            param_count,
        }),
        direct_deopt_table: Some(deopt_table),
    })) as ObjPtr
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

fn planned_pyobject_input_borrowed_ok_for_codegen_expr(
    result_demand_plan: &ResultDemandPlan,
    expr: &InstrCodegen,
) -> Option<bool> {
    let instr_id = expr.try_semantic_instr_id()?;
    result_demand_plan
        .demand_for_instr_id(instr_id)
        .map(ResultDemand::borrowed_ok)
}

fn codegen_expr_pyobject_input_is_borrowed_from_local_env(
    expr: &InstrCodegen,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> bool {
    let planned_borrowed_ok =
        planned_pyobject_input_borrowed_ok_for_codegen_expr(ctx.result_demand_plan, expr);
    if !planned_borrowed_ok.unwrap_or(true) {
        return false;
    }
    codegen_expr_is_borrowable_from_local_env(
        expr,
        local_env,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    )
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
    load_instr_id: Option<InstrId>,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
    borrowed: bool,
) -> Option<ir::Value> {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    match name.location {
        NameLocation::Constant(index) => {
            assert!(
                !borrowed,
                "constant-backed name loads must produce owned references"
            );
            Some(emit_owned_module_constant(
                fb,
                ModuleConstantId(index as usize),
                ctx,
            ))
        }
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
            let value = if let Some(load_instr_id) = load_instr_id.filter(|load_instr_id| {
                ctx.global_indexed_hit_counter_ids
                    .contains_key(load_instr_id)
            }) {
                let guard_miss_resume_point =
                    ctx.guard_miss_resume_point
                        .unwrap_or(LocalEnvResumePoint::BeforeInstr {
                            key: InstrKey::new(ctx.function_id, load_instr_id),
                        });
                emit_codegen_indexed_global_load(
                    fb,
                    globals_obj,
                    name_obj,
                    slot_index,
                    load_instr_id,
                    guard_miss_resume_point,
                    local_env,
                    ctx,
                )
            } else {
                let value_inst = fb.ins().call(
                    ctx.load_global_fast_ref,
                    &[globals_obj, name_obj, slot_index],
                );
                let value = fb.inst_results(value_inst)[0];
                emit_decref_owned_input_after_nullable_result(fb, ctx, value, name_obj)
            };
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
        NameLocation::RuntimeName => {
            let name_obj = emit_owned_module_constant(
                fb,
                ctx.module_constants
                    .require_unicode_constant_id(name.id.as_str()),
                ctx,
            );
            let value_inst = fb.ins().call(ctx.load_runtime_obj_ref, &[name_obj]);
            let value = fb.inst_results(value_inst)[0];
            let value = emit_decref_owned_input_after_nullable_result(fb, ctx, value, name_obj);
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
    counters: &HashMap<InstrId, CounterId>,
    instr_id: InstrId,
) {
    if let Some(counter_id) = counters.get(&instr_id).copied() {
        let counter_slot = scalar_counter_slot_for_id(ctx.counter_slots_by_id, counter_id)
            .unwrap_or_else(|err| panic!("{err}"));
        let scalar_counter_base_value = ctx.consts.scalar_counter_base_value.unwrap_or_else(|| {
            panic!(
                "missing scalar counter base for counter id {}",
                counter_id.0
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
        ctx.guard_miss_target_for_resume_point(guard_miss_resume_point, &[], fallback_block),
        fallback_block,
        ctx.guard_miss_deopt_stub_ref,
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

fn codegen_expr_const_string(
    expr: &InstrCodegen,
    module_constants: &ModuleCodegenConstants,
) -> Option<String> {
    match expr {
        InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_string_value(ModuleConstantId(index as usize))
        }),
        InstrCodegen::Call(call) => {
            if codegen_expr_helper_name(call.func.as_ref(), module_constants) != Some("str")
                || call.args.len() != 1
                || !call.keywords.is_empty()
            {
                return None;
            }
            let CallArgPositional::Positional(arg) = &call.args[0] else {
                return None;
            };
            codegen_expr_const_string(arg, module_constants)
        }
        _ => None,
    }
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

fn load_deleted_name_arg<'a>(
    expr: &'a InstrCodegen,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a InstrCodegen> {
    let InstrCodegen::Call(call) = expr else {
        return None;
    };
    if !call.keywords.is_empty() || call.args.len() != 2 {
        return None;
    }
    if codegen_expr_helper_name(call.func.as_ref(), module_constants) != Some("load_deleted_name") {
        return None;
    }
    let CallArgPositional::Positional(value) = &call.args[1] else {
        return None;
    };
    Some(value)
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
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> SuperInstanceArg {
    if let Some(value_expr) = load_deleted_name_arg(instance_expr, ctx.module_constants) {
        if let Some((value, is_deleted)) =
            emit_local_value_for_super_deleted_name_arg(fb, value_expr, local_env, ctx)
        {
            return SuperInstanceArg {
                value,
                is_borrowed: true,
                is_deleted: Some(is_deleted),
            };
        }
        let value = emit_codegen_expr_with_local_env(
            fb,
            value_expr,
            local_env,
            ctx,
            false,
            jit_module,
            func_imports,
        );
        return SuperInstanceArg {
            value,
            is_borrowed: false,
            is_deleted: None,
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
        jit_module,
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
    jit_module: &mut JITModule,
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
        jit_module,
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
        jit_module,
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
        jit_module,
        func_imports,
    );

    let mut instance_arg = emit_super_instance_arg_with_local_env(
        fb,
        instance_expr,
        local_env,
        ctx,
        jit_module,
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
            jit_module,
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

fn emit_direct_function_env_load(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    let metadata = load_py_function_soac_metadata_obj(fb, ptr_ty, callable);
    let metadata_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, metadata, null_ptr);
    let load_env_block = fb.create_block();
    let env_ok_block = fb.create_block();
    let done_block = fb.create_block();
    fb.append_block_param(done_block, ptr_ty);

    fb.ins().brif(
        metadata_is_null,
        done_block,
        &[ir::BlockArg::Value(null_ptr)],
        load_env_block,
        &[],
    );

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
        done_block,
        &[ir::BlockArg::Value(null_ptr)],
        env_ok_block,
        &[],
    );

    fb.switch_to_block(env_ok_block);
    fb.ins().jump(done_block, &[ir::BlockArg::Value(env)]);

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
        let closure_len =
            storage_layout_closure_len.max(max_referenced_function_closure_slot(function));
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

#[derive(Clone, Debug, Default)]
struct ResultDemandPlan {
    demands_by_instr_id: HashMap<InstrId, ResultDemand>,
}

impl ResultDemandPlan {
    fn insert_instr(&mut self, expr: &InstrTyped, demand: ResultDemand) {
        if let Some(instr_id) = expr.try_semantic_instr_id() {
            self.demands_by_instr_id.insert(instr_id, demand);
        }
    }

    fn demand_for_instr_id(&self, instr_id: InstrId) -> Option<ResultDemand> {
        self.demands_by_instr_id.get(&instr_id).copied()
    }

    fn demand_for_typed_stmt(&self, expr: &InstrTyped) -> ResultDemand {
        expr.try_semantic_instr_id()
            .and_then(|instr_id| self.demand_for_instr_id(instr_id))
            .unwrap_or(ResultDemand::EffectOnly)
    }
}

fn insert_call_arg_input_demands(
    plan: &mut ResultDemandPlan,
    args: &[CallArgPositional<InstrTyped>],
    keywords: &[CallArgKeyword<InstrTyped>],
) {
    for arg in args {
        let value = arg.expr();
        insert_pyobject_borrowed_input_demand(plan, value);
    }
    for keyword in keywords {
        let value = keyword.expr();
        insert_pyobject_borrowed_input_demand(plan, value);
    }
}

fn insert_pyobject_borrowed_input_demand(plan: &mut ResultDemandPlan, expr: &InstrTyped) {
    plan.insert_instr(expr, ResultDemand::PYOBJECT_BORROWED_OK);
    insert_typed_child_demands(plan, expr);
}

fn insert_typed_child_demands(plan: &mut ResultDemandPlan, expr: &InstrTyped) {
    match expr {
        InstrTyped::BinOp(op) => {
            insert_pyobject_borrowed_input_demand(plan, op.left.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.right.as_ref());
        }
        InstrTyped::LegacyUnaryOp(op) => {
            insert_pyobject_borrowed_input_demand(plan, op.operand.as_ref());
        }
        InstrTyped::LegacyTuple(op) => {
            for value in &op.values {
                insert_pyobject_borrowed_input_demand(plan, value);
            }
        }
        InstrTyped::LegacyCalleeFunctionId(op) => {
            insert_pyobject_borrowed_input_demand(plan, op.value.as_ref());
        }
        InstrTyped::LegacyStore(store) => {
            plan.insert_instr(store.value.as_ref(), ResultDemand::PYOBJECT_OWNED);
            insert_typed_child_demands(plan, store.value.as_ref());
        }
        InstrTyped::LegacyCall(call) => {
            plan.insert_instr(call.func.as_ref(), ResultDemand::PYOBJECT_BORROWED_OK);
            insert_typed_child_demands(plan, call.func.as_ref());
            insert_call_arg_input_demands(plan, call.args.as_slice(), call.keywords.as_slice());
        }
        InstrTyped::LegacyCallDirect(call) => {
            plan.insert_instr(call.callable.as_ref(), ResultDemand::PYOBJECT_BORROWED_OK);
            insert_typed_child_demands(plan, call.callable.as_ref());
            insert_call_arg_input_demands(plan, call.args.as_slice(), call.keywords.as_slice());
        }
        InstrTyped::LegacyGetAttr(op) => {
            insert_pyobject_borrowed_input_demand(plan, op.value.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.attr.as_ref());
        }
        InstrTyped::LegacySetAttr(op) => {
            insert_pyobject_borrowed_input_demand(plan, op.value.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.attr.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.replacement.as_ref());
        }
        InstrTyped::LegacyGetItem(op) => {
            insert_pyobject_borrowed_input_demand(plan, op.value.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.index.as_ref());
        }
        InstrTyped::LegacySetItem(op) => {
            insert_pyobject_borrowed_input_demand(plan, op.value.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.index.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.replacement.as_ref());
        }
        InstrTyped::LegacyDelItem(op) => {
            insert_pyobject_borrowed_input_demand(plan, op.value.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.index.as_ref());
        }
        InstrTyped::LegacyMakeFunctionWithClosure(op) => {
            insert_pyobject_borrowed_input_demand(plan, op.captures.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.param_defaults.as_ref());
            insert_pyobject_borrowed_input_demand(plan, op.annotate_fn.as_ref());
        }
        _ => {}
    }
}

fn plan_typed_result_demands(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> ResultDemandPlan {
    let mut plan = ResultDemandPlan::default();
    for block in &function.blocks {
        for expr in &block.body {
            plan.insert_instr(expr, ResultDemand::EffectOnly);
            insert_typed_child_demands(&mut plan, expr);
        }
        if let BlockTerm::IfTerm(if_term) = &block.term {
            plan.insert_instr(&if_term.test, ResultDemand::I32_BOOL01);
        }
        if let BlockTerm::BranchTable(branch) = &block.term {
            plan.insert_instr(&branch.index, ResultDemand::I64_INDEX);
        }
        if let BlockTerm::Return(value) = &block.term {
            plan.insert_instr(value, ResultDemand::PYOBJECT_OWNED);
        }
        if let BlockTerm::Raise(raise_stmt) = &block.term
            && let Some(exc) = raise_stmt.exc.as_ref()
        {
            plan.insert_instr(exc, ResultDemand::PYOBJECT_OWNED);
            insert_typed_child_demands(&mut plan, exc);
        }
        match &block.term {
            BlockTerm::IfTerm(if_term) => insert_typed_child_demands(&mut plan, &if_term.test),
            BlockTerm::BranchTable(branch) => insert_typed_child_demands(&mut plan, &branch.index),
            BlockTerm::Return(value) => insert_typed_child_demands(&mut plan, value),
            BlockTerm::Raise(_) | BlockTerm::Jump(_) => {}
        }
    }
    plan
}

#[derive(Clone)]
struct JitEmitCtx<'mc> {
    module: &'mc BlockPyModule<CodegenModuleShape>,
    function_id: FunctionId,
    function_kind: FunctionKind,
    shared_state: Option<&'mc crate::module_type::SharedModuleState>,
    module_constants: &'mc ModuleCodegenConstants,
    value_facts: &'mc FactStore,
    result_demand_plan: &'mc ResultDemandPlan,
    deopt_resume_plan: &'mc PlannedJitDeoptResumeFunction,
    refcount_plan: &'mc FunctionRefcountPlan,
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
    guard_miss_resume_point: Option<LocalEnvResumePoint>,
    store_global_indexed_ref: ir::FuncRef,
    probe_field_indexed_ref: ir::FuncRef,
    store_field_indexed_ref: ir::FuncRef,
    load_runtime_obj_ref: ir::FuncRef,
    enter_recursive_ref: ir::FuncRef,
    pyobject_getattr_ref: ir::FuncRef,
    pyobject_setattr_ref: ir::FuncRef,
    pyobject_getitem_ref: ir::FuncRef,
    pyobject_setitem_ref: ir::FuncRef,
    py_long_from_i64_ref: ir::FuncRef,
    raise_deleted_name_error_ref: ir::FuncRef,
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
    direct_call_target_functions: &'mc HashMap<FunctionId, BlockPyFunction<CodegenModuleShape>>,
    direct_call_functions: &'mc HashMap<FunctionId, DeclaredJitFunction>,
    call_target_counter_ids: &'mc HashMap<InstrId, CounterId>,
    call_target_specializations: &'mc HashMap<InstrId, Vec<FunctionId>>,
    call_direct_hit_counter_ids: &'mc HashMap<InstrId, CounterId>,
    call_direct_fallback_counter_ids: &'mc HashMap<InstrId, CounterId>,
    operator_shape_counter_ids: &'mc HashMap<InstrId, CounterId>,
    operator_specializations: &'mc HashMap<InstrId, Vec<u64>>,
    operator_specialized_hit_counter_ids: &'mc HashMap<InstrId, CounterId>,
    operator_specialized_fallback_counter_ids: &'mc HashMap<InstrId, CounterId>,
    getitem_shape_counter_ids: &'mc HashMap<InstrId, CounterId>,
    getitem_specializations: &'mc HashMap<InstrId, Vec<u64>>,
    getitem_specialized_hit_counter_ids: &'mc HashMap<InstrId, CounterId>,
    getitem_specialized_fallback_counter_ids: &'mc HashMap<InstrId, CounterId>,
    setitem_shape_counter_ids: &'mc HashMap<InstrId, CounterId>,
    setitem_specializations: &'mc HashMap<InstrId, Vec<u64>>,
    setitem_specialized_hit_counter_ids: &'mc HashMap<InstrId, CounterId>,
    setitem_specialized_fallback_counter_ids: &'mc HashMap<InstrId, CounterId>,
    branch_outcome_counter_ids: &'mc HashMap<InstrId, CounterId>,
    branch_prefer_true: &'mc HashMap<InstrId, bool>,
    global_indexed_hit_counter_ids: &'mc HashMap<InstrId, CounterId>,
    global_indexed_fallback_counter_ids: &'mc HashMap<InstrId, CounterId>,
    field_indexed_hit_counter_ids: &'mc HashMap<InstrId, CounterId>,
    field_indexed_fallback_counter_ids: &'mc HashMap<InstrId, CounterId>,
    deopt_entry_guard_miss_counter_ids: &'mc HashMap<usize, CounterId>,
    field_index_specializations: &'mc HashMap<String, Vec<FieldIndexSpecialization>>,
    behavior_change_indexed_stores: bool,
    allow_local_only_slot_backed_stores: bool,
    exception_forwarded_local_names: Option<&'mc [String]>,
    type_ptr_data_ids: RefCell<HashMap<RelocTypeRef, DataId>>,
    callable_ptr_data_ids: RefCell<HashMap<RelocCallableRef, DataId>>,
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
    let counter_slot = scalar_counter_slot_for_id(ctx.counter_slots_by_id, *counter_id)
        .unwrap_or_else(|err| panic!("{err}"));
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
        pre_guard_operands: &[&InstrCodegen],
        fallback_block: ir::Block,
    ) -> Result<JitGuardMissTarget, RuntimeJitDeoptUnsupportedReason> {
        let function = self
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == self.function_id)
            .ok_or(RuntimeJitDeoptUnsupportedReason::MissingFunction)?;
        runtime_jit_deopt_guard_miss_supported(function, point, pre_guard_operands)?;
        let deopt_exit = self
            .require_deopt_record_ref(point)
            .map_err(|_| RuntimeJitDeoptUnsupportedReason::MissingPlanRecord)?;
        Ok(JitGuardMissTarget {
            fallback_block,
            deopt_exit,
        })
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
    function_id: FunctionId,
    descriptor_function_ref: RelocCallableRef,
    owner_type_ref: RelocTypeRef,
    type_version: u32,
    arg_plan: DirectCallArgPlan,
}

#[derive(Clone)]
struct DirectConstructorSpecialization {
    function_id: FunctionId,
    init_function_ref: RelocCallableRef,
    owner_type_ref: RelocTypeRef,
    type_version: u32,
    arg_plan: DirectCallArgPlan,
}

#[derive(Clone)]
struct DirectFunctionSpecialization {
    function_id: FunctionId,
    arg_plan: DirectCallArgPlan,
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
    call_direct_missing_target_fallbacks: Cell<usize>,
    call_direct_unsupported_shape_fallbacks: Cell<usize>,
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

    fn record_call_direct_missing_target_fallback(&self) {
        Self::increment(&self.call_direct_missing_target_fallbacks);
    }

    fn record_call_direct_unsupported_shape_fallback(&self) {
        Self::increment(&self.call_direct_unsupported_shape_fallbacks);
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
            + self.call_direct_missing_target_fallbacks.get()
            + self.call_direct_unsupported_shape_fallbacks.get()
            + self.guarded_generic_fallback_blocks.get()
            + self.profiled_missing_target_candidates.get()
            + self.profiled_arity_mismatch_candidates.get()
            + self.profiled_unsupported_shape_candidates.get()
    }

    fn emit_trace(&self, module_name: &str, function: &BlockPyFunction<CodegenModuleShape>) {
        if self.total() == 0 {
            return;
        }
        let clif_direct_edges = self.clif_direct_edges.get();
        let function_env_indirect_edges = self.function_env_indirect_edges.get();
        let call_direct_missing_target_fallbacks = self.call_direct_missing_target_fallbacks.get();
        let call_direct_unsupported_shape_fallbacks =
            self.call_direct_unsupported_shape_fallbacks.get();
        let guarded_generic_fallback_blocks = self.guarded_generic_fallback_blocks.get();
        let profiled_missing_target_candidates = self.profiled_missing_target_candidates.get();
        let profiled_arity_mismatch_candidates = self.profiled_arity_mismatch_candidates.get();
        let profiled_unsupported_shape_candidates =
            self.profiled_unsupported_shape_candidates.get();
        let generic_fallback_edges = function_env_indirect_edges
            + call_direct_missing_target_fallbacks
            + call_direct_unsupported_shape_fallbacks
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
            call_direct_missing_target_fallbacks,
            call_direct_unsupported_shape_fallbacks,
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
    function_id: FunctionId,
) -> Option<&'a BlockPyFunction<CodegenModuleShape>> {
    ctx.module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .or_else(|| ctx.direct_call_target_functions.get(&function_id))
}

fn direct_call_positional_arg_count(args: &[CallArgPositional<InstrCodegen>]) -> usize {
    args.iter()
        .filter(|arg| matches!(arg, CallArgPositional::Positional(_)))
        .count()
}

fn direct_call_has_starred_arguments(
    args: &[CallArgPositional<InstrCodegen>],
    keywords: &[CallArgKeyword<InstrCodegen>],
) -> bool {
    args.iter()
        .any(|arg| matches!(arg, CallArgPositional::Starred(_)))
        || keywords
            .iter()
            .any(|keyword| matches!(keyword, CallArgKeyword::Starred(_)))
}

fn plan_direct_call_args_for_target(
    target_function: &BlockPyFunction<CodegenModuleShape>,
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
    function: &BlockPyFunction<CodegenModuleShape>,
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
    param: &soac_blockpy::block_py::Param,
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
    target_function: &BlockPyFunction<CodegenModuleShape>,
    _direct_call_functions: &HashMap<FunctionId, DeclaredJitFunction>,
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

#[derive(Clone)]
struct FieldIndexSpecialization {
    expected_index: u32,
    owner_type_ref: RelocTypeRef,
    type_version: u32,
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RelocCallableRef {
    OwnerAttr {
        owner_type_ref: RelocTypeRef,
        attr_name: String,
    },
}

struct LocalEnvCodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd> {
    fb: &'a mut FunctionBuilder<'b>,
    local_env: &'c mut LocalEnv,
    ctx: &'c JitEmitCtx<'mc>,
    jit_module: &'a mut JITModule,
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
                return Some(emit_checked_local_value_or_deleted(
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
                return Some(emit_checked_local_value_or_deleted(
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
    let block_plan = ctx.refcount_plan.block(instr_key.instr_id.block_label())?;
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
    _local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Option<PyObjFacts> {
    ctx.value_facts_for_expr(expr)
        .and_then(ValueFacts::as_pyobj)
}

fn py_facts_for_typed_expr_with_local_env(
    expr: &InstrTyped,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Option<PyObjFacts> {
    if let InstrTyped::Load(op) = expr {
        if let Some(py_facts) = local_env.py_facts_for_load(&op.name) {
            return Some(py_facts);
        }
        if op.name.location.as_constant().is_some_and(|index| {
            ctx.module_constants
                .constant_is_int(ModuleConstantId(index as usize))
        }) {
            return Some(PyObjFacts::exact_type(PyExactType::Int));
        }
        return op
            .try_semantic_instr_id()
            .and_then(|instr_id| ctx.value_facts_for_instr_id(instr_id))
            .and_then(ValueFacts::as_pyobj);
    }
    expr.try_semantic_instr_id()
        .and_then(|instr_id| ctx.value_facts_for_instr_id(instr_id))
        .and_then(ValueFacts::as_pyobj)
}

fn local_ref_kind_for_typed_stored_value(
    value: &InstrTyped,
    ownership: ValueOwnership,
    ctx: &JitEmitCtx<'_>,
) -> LocalRefKind {
    if matches!(ownership, ValueOwnership::Immortal) {
        return LocalRefKind::Immortal;
    }
    match value
        .try_semantic_instr_id()
        .and_then(|instr_id| ctx.value_facts_for_instr_id(instr_id))
        .and_then(ValueFacts::as_pyobj)
    {
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
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<ir::Value> {
    let result = emit_local_store_result_with_local_env(
        fb,
        expr,
        op,
        local_env,
        emit_ctx,
        ResultDemand::PYOBJECT_OWNED,
        jit_module,
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
    jit_module: &mut JITModule,
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
            jit_module,
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
        jit_module,
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
    jit_module: &mut JITModule,
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
        .try_semantic_instr_id()
        .and_then(|instr_id| emit_ctx.result_demand_plan.demand_for_instr_id(instr_id))
        .unwrap_or(ResultDemand::PYOBJECT_OWNED);
    let value_result = match value_demand {
        ResultDemand::PyObject { borrowed_ok: false } => {
            emit_typed_codegen_stmt_result_with_local_env(
                fb,
                &op.value,
                local_env,
                emit_ctx,
                value_demand,
                jit_module,
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
        py_facts_for_typed_expr_with_local_env(&op.value, local_env, emit_ctx)
            .unwrap_or(value_py_facts)
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
        None => local_ref_kind_for_typed_stored_value(&op.value, ownership, emit_ctx),
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
    fn new(fb: &mut FunctionBuilder<'_>, function: &BlockPyFunction<CodegenModuleShape>) -> Self {
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

impl<'a, 'b, 'mc, 'c, 'd> intrinsics::OperationEmitState<'b, InstrCodegen>
    for LocalEnvCodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd>
{
    fn ctx(&self) -> &JitEmitCtx<'mc> {
        self.ctx
    }

    fn fb(&mut self) -> &mut FunctionBuilder<'b> {
        self.fb
    }

    fn import_func(&mut self, spec: &'static ImportSpec) -> ir::FuncRef {
        self.func_imports
            .get_or_panic(self.jit_module, &mut self.fb.func, spec)
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
                self.jit_module,
                self.func_imports,
            );
            arg_values.push((value, borrowed_arg));
        }
        arg_values
    }

    fn release_arg_values(&mut self, arg_values: &[(ir::Value, bool)]) {
        for (value, borrowed_arg) in arg_values {
            if !borrowed_arg {
                self.fb.ins().call(
                    self.ctx.decref_ref,
                    &[self.ctx.consts.thread_state_value, *value],
                );
            }
        }
    }

    fn finish_owned_result(&mut self, value: ir::Value) -> ir::Value {
        let null_ptr = self.fb.ins().iconst(self.ctx.consts.ptr_ty, 0);
        let value_is_null = self
            .fb
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
        let value_ok_block = self.fb.create_block();
        self.fb
            .append_block_param(value_ok_block, self.ctx.consts.ptr_ty);
        self.fb.ins().brif(
            value_is_null,
            self.ctx.consts.step_null_block,
            &step_null_block_args(self.ctx),
            value_ok_block,
            &[ir::BlockArg::Value(value)],
        );
        self.fb.switch_to_block(value_ok_block);
        self.fb.block_params(value_ok_block)[0]
    }

    fn emit_owned_bool_from_i32_result(&mut self, result: ir::Value) -> ir::Value {
        emit_owned_bool_from_i32_result(self.fb, result, self.ctx)
    }

    fn emit_owned_bool_from_cond(&mut self, cond: ir::Value) -> ir::Value {
        emit_owned_bool_from_cond(self.fb, cond, self.ctx)
    }

    fn emit_owned_bool_from_pyobject_truthiness(
        &mut self,
        value: ir::Value,
        facts: PyObjFacts,
        borrowed: bool,
        invert: bool,
    ) -> ir::Value {
        let is_true_ref = self.func_imports.get_or_panic(
            self.jit_module,
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
        emit_type_ptr_value_for_ref(self.fb, self.jit_module, self.ctx, owner_type_ref)
            .unwrap_or_else(|err| {
                panic!("failed to bind type symbol during JIT codegen: {err}");
            })
    }

    fn py_facts_for_arg(&self, arg: &InstrCodegen) -> PyObjFacts {
        self.ctx
            .value_facts_for_expr(arg)
            .and_then(ValueFacts::as_pyobj)
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
            self.ctx.guard_miss_target_for_resume_point(
                guard_miss_resume_point,
                pre_guard_operands,
                fallback_block,
            ),
            fallback_block,
            self.ctx.guard_miss_deopt_stub_ref,
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

fn emit_checked_local_value_or_deleted(
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
    let deleted_block = fb.create_block();
    let value_ok_block = fb.create_block();
    fb.append_block_param(value_ok_block, ctx.consts.ptr_ty);
    fb.ins().brif(
        value_is_null,
        deleted_block,
        &[],
        value_ok_block,
        &[ir::BlockArg::Value(value)],
    );

    fb.switch_to_block(deleted_block);
    let name_obj = emit_owned_module_constant(
        fb,
        ctx.module_constants.require_unicode_constant_id(name),
        ctx,
    );
    fb.ins().call(ctx.raise_deleted_name_error_ref, &[name_obj]);
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
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, value]);
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
    function_id: FunctionId,
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
    symbol_name: &str,
    function_name: &str,
    wrapper_import: &'static ImportSpec,
    applied_import: &'static ImportSpec,
    scalar_counter_data_id: DataId,
    counter_slot: usize,
) -> Result<FuncId, String> {
    let ptr_ty = jit_module.target_config().pointer_type();
    let sig = lower_static_signature(jit_module, wrapper_import.signature);
    let helper_id = declare_local_fn(jit_module, symbol_name, &sig)?;

    let mut ctx = jit_module.make_context();
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
        let counter_data = jit_module.declare_data_in_func(scalar_counter_data_id, &mut fb.func);
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
        helper_id,
        &mut ctx,
        function_name,
        "failed to define counted runtime refcount helper",
    )?;
    jit_module.clear_context(&mut ctx);
    Ok(helper_id)
}

fn build_counted_runtime_refcount_helpers(
    jit_module: &mut JITModule,
    function: &BlockPyFunction<CodegenModuleShape>,
    counter_defs: &[CounterDef],
    counter_slots_by_id: &[CounterRuntimeSlot],
    scalar_counter_data_id: Option<DataId>,
    symbol_scope: Option<&str>,
) -> Result<CountedRefcountHelpers, String> {
    if !jit_refcount_emission_enabled()? {
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
    jit_module: &mut JITModule,
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
            jit_module,
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
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let (arg_values, arg_borrowed) =
        emit_positional_arg_values(fb, args, local_env, ctx, jit_module, func_imports);
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
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let (arg_values, arg_borrowed) =
        emit_positional_arg_values(fb, args, local_env, ctx, jit_module, func_imports);
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
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    debug_assert!(args.len() <= 3);
    let (arg_values, arg_borrowed) =
        emit_positional_arg_values(fb, args, local_env, ctx, jit_module, func_imports);
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
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
    demand: ResultDemand,
) -> EmitResult {
    debug_assert!(args.len() <= 3);
    let (arg_values, arg_borrowed) =
        emit_positional_arg_values(fb, args, local_env, ctx, jit_module, func_imports);
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
    jit_module: &mut JITModule,
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
            jit_module,
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
    name: &str,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let name_obj = emit_owned_module_constant(
        fb,
        ctx.module_constants.require_unicode_constant_id(name),
        ctx,
    );
    emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.load_runtime_obj_ref,
        &[name_obj],
        &[name_obj],
    )
}

fn emit_soac_ext_make_function_callable(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ext_module = emit_owned_module_constant(
        fb,
        ctx.module_constants
            .require_runtime_name_constant_id("_soac_ext"),
        ctx,
    );
    let attr_name = emit_owned_module_constant(
        fb,
        ctx.module_constants
            .require_unicode_constant_id("make_function"),
        ctx,
    );
    emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.pyobject_getattr_ref,
        &[ext_module, attr_name],
        &[ext_module, attr_name],
    )
}

fn emit_empty_dict_with_args_tuple(
    fb: &mut FunctionBuilder<'_>,
    empty_args_tuple: ir::Value,
    empty_args_tuple_is_borrowed: bool,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let dict_callable = emit_checked_runtime_name_object(fb, "dict", ctx);
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
    jit_module: &mut JITModule,
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
        jit_module,
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
    jit_module: &mut JITModule,
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
            jit_module,
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
            jit_module,
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

fn emit_unpack_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[CallArgPositional<InstrCodegen>],
    keywords: &[CallArgKeyword<InstrCodegen>],
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
        jit_module,
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
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
    demand: ResultDemand,
) -> EmitResult {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    let list_callable = emit_checked_runtime_name_object(fb, "list", ctx);
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
            jit_module,
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
                    jit_module,
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
                    jit_module,
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

    let tuple_callable = emit_checked_runtime_name_object(fb, "tuple_from_iter", ctx);
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

fn emit_owned_bool_from_cond(
    fb: &mut FunctionBuilder<'_>,
    cond: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let truth = emit_i32_bool01_from_cond(fb, cond, ctx);
    let (bool_value, _) = emit_to_python_bool(fb, truth, ctx).expect_pyobject("bool materialize");
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
    fb.ins().call(ctx.incref_ref, &[bool_value]);
    SoacValue::pyobject(bool_value, PyObjFacts::bool_object())
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
    let (bool_value, _) = emit_to_python_bool(fb, truth, ctx).expect_pyobject("bool materialize");
    bool_value
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
    let (bool_value, _) =
        emit_to_python_bool(fb, truth, ctx).expect_pyobject("truthiness bool materialize");
    bool_value
}

fn emit_branch_index_i64(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrCodegen,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
    pyobject_to_i64_ref: ir::FuncRef,
) -> ir::Value {
    match expr {
        InstrCodegen::CalleeFunctionId(op) => {
            let callable_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
                op.value.as_ref(),
                local_env,
                ctx,
            );
            let callable = emit_codegen_expr_with_local_env(
                fb,
                op.value.as_ref(),
                local_env,
                ctx,
                callable_is_borrowed,
                jit_module,
                func_imports,
            );
            let callee_id = emit_callee_function_id_checked(fb, callable, ctx, jit_module);
            if !callable_is_borrowed {
                fb.ins()
                    .call(ctx.decref_ref, &[ctx.consts.thread_state_value, callable]);
            }
            callee_id
        }
        _ => {
            let index_obj = emit_codegen_expr_with_local_env(
                fb,
                expr,
                local_env,
                ctx,
                false,
                jit_module,
                func_imports,
            );
            let index_i64_inst = fb.ins().call(pyobject_to_i64_ref, &[index_obj]);
            let index_i64 = fb.inst_results(index_i64_inst)[0];
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, index_obj]);
            index_i64
        }
    }
}

fn module_constant_string_value<'a>(
    module: &'a BlockPyModule<CodegenModuleShape>,
    constant_index: u32,
) -> Option<&'a str> {
    let InstrResolved::Literal(literal) = module.module_constants.get(constant_index as usize)?
    else {
        return None;
    };
    let Literal::StringLiteral(literal) = literal.as_literal() else {
        return None;
    };
    Some(literal.value.as_str())
}

pub(super) fn codegen_constant_string_value<'a>(
    module: &'a BlockPyModule<CodegenModuleShape>,
    expr: &InstrCodegen,
) -> Option<&'a str> {
    let InstrCodegen::Load(load) = expr else {
        return None;
    };
    let NameLocation::Constant(constant_index) = load.name.location else {
        return None;
    };
    module_constant_string_value(module, constant_index)
}

fn direct_method_specializations_for_call_site(
    call: &blockpy_intrinsics::Call<InstrCodegen>,
    ctx: &JitEmitCtx<'_>,
) -> Vec<DirectMethodSpecialization> {
    if ctx.shared_state.is_none() {
        return Vec::new();
    }
    let InstrCodegen::GetAttr(getattr) = call.func.as_ref() else {
        return Vec::new();
    };
    let Some(method_name) = codegen_constant_string_value(ctx.module, getattr.attr.as_ref()) else {
        return Vec::new();
    };
    let Some(instr_id) = call.try_semantic_instr_id() else {
        return Vec::new();
    };
    let Some(targets) = ctx.call_target_specializations.get(&instr_id) else {
        return Vec::new();
    };
    let explicit_positional_arg_count = direct_call_positional_arg_count(&call.args);
    let has_starred_arguments = direct_call_has_starred_arguments(&call.args, &call.keywords);
    let has_keywords = !call.keywords.is_empty();
    let mut out = Vec::new();
    for function_id in targets.iter().copied() {
        let Some(target_function) = direct_call_target_function(ctx, function_id) else {
            ctx.direct_edge_stats
                .record_profiled_missing_target_candidate();
            continue;
        };
        let arg_plan = match validate_direct_call_compatibility(
            target_function,
            ctx.direct_call_functions,
            explicit_positional_arg_count,
            1,
            has_starred_arguments,
            has_keywords,
        ) {
            Ok(arg_plan) => arg_plan,
            Err(incompatibility) => {
                record_profiled_direct_call_incompatibility(ctx.direct_edge_stats, incompatibility);
                continue;
            }
        };
        let Ok(owner_types) =
            (unsafe { crate::lookup_exact_owner_types_for_method(function_id, method_name) })
        else {
            continue;
        };
        for owner in owner_types {
            let Ok(Some(owner_type_ref)) = reloc_type_ref_for_type(owner.owner_type) else {
                continue;
            };
            out.push(DirectMethodSpecialization {
                function_id,
                descriptor_function_ref: RelocCallableRef::OwnerAttr {
                    owner_type_ref: owner_type_ref.clone(),
                    attr_name: method_name.to_string(),
                },
                owner_type_ref,
                type_version: owner.type_version,
                arg_plan: arg_plan.clone(),
            });
        }
    }
    out
}

fn direct_constructor_specializations_for_call_site(
    call: &blockpy_intrinsics::Call<InstrCodegen>,
    ctx: &JitEmitCtx<'_>,
) -> Vec<DirectConstructorSpecialization> {
    if ctx.shared_state.is_none() {
        return Vec::new();
    }
    let Some(instr_id) = call.try_semantic_instr_id() else {
        return Vec::new();
    };
    let Some(targets) = ctx.call_target_specializations.get(&instr_id) else {
        return Vec::new();
    };
    let explicit_positional_arg_count = direct_call_positional_arg_count(&call.args);
    let has_starred_arguments = direct_call_has_starred_arguments(&call.args, &call.keywords);
    let has_keywords = !call.keywords.is_empty();
    let mut out = Vec::new();
    for function_id in targets.iter().copied() {
        let Some(target_function) = direct_call_target_function(ctx, function_id) else {
            ctx.direct_edge_stats
                .record_profiled_missing_target_candidate();
            continue;
        };
        let arg_plan = match validate_direct_call_compatibility(
            target_function,
            ctx.direct_call_functions,
            explicit_positional_arg_count,
            1,
            has_starred_arguments,
            has_keywords,
        ) {
            Ok(arg_plan) => arg_plan,
            Err(incompatibility) => {
                record_profiled_direct_call_incompatibility(ctx.direct_edge_stats, incompatibility);
                continue;
            }
        };
        let Ok(owner_types) =
            (unsafe { crate::lookup_exact_owner_types_for_constructor(function_id) })
        else {
            continue;
        };
        for owner in owner_types {
            let Ok(Some(owner_type_ref)) = reloc_type_ref_for_type(owner.owner_type) else {
                continue;
            };
            out.push(DirectConstructorSpecialization {
                function_id,
                init_function_ref: RelocCallableRef::OwnerAttr {
                    owner_type_ref: owner_type_ref.clone(),
                    attr_name: "__init__".to_string(),
                },
                owner_type_ref,
                type_version: owner.type_version,
                arg_plan: arg_plan.clone(),
            });
        }
    }
    out
}

fn collect_call_direct_targets(
    function: &BlockPyFunction<CodegenModuleShape>,
) -> HashSet<FunctionId> {
    struct CallDirectTargetCollector<'a> {
        out: &'a mut HashSet<FunctionId>,
    }

    impl Visit<InstrCodegen> for CallDirectTargetCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            if let InstrCodegen::CallDirect(call) = expr {
                self.out.insert(call.function_id);
            }
            expr.visit_children(self);
        }
    }

    let mut out = HashSet::new();
    let mut collector = CallDirectTargetCollector { out: &mut out };
    collector.visit_fn(function);
    out
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

fn collect_make_function_targets(
    function: &BlockPyFunction<CodegenModuleShape>,
) -> HashSet<FunctionId> {
    struct MakeFunctionTargetCollector<'a> {
        out: &'a mut HashSet<FunctionId>,
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

fn is_synthetic_class_helper_function(function: &BlockPyFunction<CodegenModuleShape>) -> bool {
    function.names.bind_name.starts_with("_dp_class_ns_")
        || function.names.bind_name.starts_with("_dp_define_class_")
}

fn collect_runtime_counter_ids_by_kind(
    counter_defs: &[CounterDef],
    function_id: FunctionId,
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
    function_id: FunctionId,
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
    behavior_change_indexed_stores: bool,
    profiled_cold_blocks: bool,
    guard_miss_deopt: bool,
}

impl<'a> SpecializationProfile<'a> {
    fn from_runtime_state(shared_state: Option<&'a SharedModuleState>) -> Result<Self, String> {
        let specialization_mode = specialization_mode_from_env()?;
        let counter_dump_path = if shared_state.is_some()
            && specialization_mode != Some(crate::config::SpecializationMode::Profile)
        {
            counter_dump_input_path_from_env()?
        } else {
            None
        };
        Ok(Self {
            module_name: shared_state.map(|shared_state| shared_state.module_name.as_str()),
            counter_dump_path: counter_dump_path.map(Cow::Owned),
            behavior_change_indexed_stores: behavior_change_indexed_stores_enabled()?,
            profiled_cold_blocks: profiled_cold_blocks_enabled()?,
            guard_miss_deopt: matches!(
                specialization_mode,
                Some(SpecializationMode::Verify | SpecializationMode::Apply)
            ),
        })
    }

    fn from_precompile(
        module_name: &'a str,
        counter_dump_path: Option<&'a Path>,
    ) -> Result<Self, String> {
        Ok(Self {
            module_name: Some(module_name),
            counter_dump_path: counter_dump_path.map(Cow::Borrowed),
            behavior_change_indexed_stores: true,
            profiled_cold_blocks: profiled_cold_blocks_enabled()?,
            guard_miss_deopt: true,
        })
    }

    fn call_target_specializations(
        &self,
        function_id: FunctionId,
    ) -> Result<HashMap<InstrId, Vec<FunctionId>>, String> {
        let Some(module_name) = self.module_name else {
            return Ok(HashMap::new());
        };
        let Some(path) = existing_counter_dump_path(self.counter_dump_path.as_deref()) else {
            return Ok(HashMap::new());
        };
        read_call_target_specializations_from_file(path, module_name, function_id)
    }

    fn operator_specializations(
        &self,
        function_id: FunctionId,
    ) -> Result<HashMap<InstrId, Vec<u64>>, String> {
        let Some(module_name) = self.module_name else {
            return Ok(HashMap::new());
        };
        let Some(path) = existing_counter_dump_path(self.counter_dump_path.as_deref()) else {
            return Ok(HashMap::new());
        };
        read_operator_specializations_from_file(path, module_name, function_id)
    }

    fn getitem_specializations(
        &self,
        function_id: FunctionId,
    ) -> Result<HashMap<InstrId, Vec<u64>>, String> {
        let Some(module_name) = self.module_name else {
            return Ok(HashMap::new());
        };
        let Some(path) = existing_counter_dump_path(self.counter_dump_path.as_deref()) else {
            return Ok(HashMap::new());
        };
        read_getitem_specializations_from_file(path, module_name, function_id)
    }

    fn setitem_specializations(
        &self,
        function_id: FunctionId,
    ) -> Result<HashMap<InstrId, Vec<u64>>, String> {
        let Some(module_name) = self.module_name else {
            return Ok(HashMap::new());
        };
        let Some(path) = existing_counter_dump_path(self.counter_dump_path.as_deref()) else {
            return Ok(HashMap::new());
        };
        read_setitem_specializations_from_file(path, module_name, function_id)
    }

    fn branch_preferences(
        &self,
        function_id: FunctionId,
    ) -> Result<HashMap<InstrId, bool>, String> {
        let Some(module_name) = self.module_name else {
            return Ok(HashMap::new());
        };
        let Some(path) = existing_counter_dump_path(self.counter_dump_path.as_deref()) else {
            return Ok(HashMap::new());
        };
        read_branch_preferences_from_file(path, module_name, function_id)
    }

    fn field_index_specializations(
        &self,
    ) -> Result<HashMap<String, Vec<FieldIndexSpecialization>>, String> {
        let Some(path) = existing_counter_dump_path(self.counter_dump_path.as_deref()) else {
            return Ok(HashMap::new());
        };
        load_field_index_specializations_from_path(path)
    }

    fn cold_block_labels(
        &self,
        function: &BlockPyFunction<CodegenModuleShape>,
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

fn load_call_target_specializations(
    module_name: &str,
    function_id: FunctionId,
) -> Result<HashMap<InstrId, Vec<FunctionId>>, String> {
    if specialization_mode_is_profile()? {
        return Ok(HashMap::new());
    }
    let Some(path) = counter_dump_input_path_from_env()? else {
        return Ok(HashMap::new());
    };
    let path = path.as_path();
    if !path.exists() {
        return Ok(HashMap::new());
    }
    read_call_target_specializations_from_file(path, module_name, function_id)
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
            let Some(owner_type) = resolve_reloc_type_ref_to_type(owner_type_ref)? else {
                return Ok(false);
            };
            let symbol = reloc_type_ref_symbol_name(owner_type_ref);
            register_jit_data_symbol(symbol.as_ref(), owner_type.cast::<u8>());
            Ok(true)
        }
    }
}

fn type_ptr_data_id_for_ref(
    jit_module: &mut JITModule,
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
    let data_id = declare_type_ptr_import(jit_module, symbol.as_ref())?;
    ctx.type_ptr_data_ids
        .borrow_mut()
        .insert(owner_type_ref.clone(), data_id);
    Ok(Some(data_id))
}

fn emit_type_ptr_value_for_ref(
    fb: &mut FunctionBuilder<'_>,
    jit_module: &mut JITModule,
    ctx: &JitEmitCtx<'_>,
    owner_type_ref: &RelocTypeRef,
) -> Result<Option<ir::Value>, String> {
    let Some(data_id) = type_ptr_data_id_for_ref(jit_module, ctx, owner_type_ref)? else {
        return Ok(None);
    };
    let type_data = jit_module.declare_data_in_func(data_id, &mut fb.func);
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
    let Some(callable) = resolve_reloc_callable_ref_to_object(callable_ref)? else {
        return Ok(false);
    };
    let symbol = reloc_callable_ref_symbol_name(callable_ref);
    register_jit_data_symbol(symbol.as_str(), callable.cast::<u8>());
    Ok(true)
}

fn callable_ptr_data_id_for_ref(
    jit_module: &mut JITModule,
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
    if !ensure_reloc_callable_symbol_registered(callable_ref)? {
        return Ok(None);
    }
    let symbol = reloc_callable_ref_symbol_name(callable_ref);
    let data_id = declare_type_ptr_import(jit_module, symbol.as_str())?;
    ctx.callable_ptr_data_ids
        .borrow_mut()
        .insert(callable_ref.clone(), data_id);
    Ok(Some(data_id))
}

fn emit_callable_ptr_value_for_ref(
    fb: &mut FunctionBuilder<'_>,
    jit_module: &mut JITModule,
    ctx: &JitEmitCtx<'_>,
    callable_ref: &RelocCallableRef,
) -> Result<Option<ir::Value>, String> {
    let Some(data_id) = callable_ptr_data_id_for_ref(jit_module, ctx, callable_ref)? else {
        return Ok(None);
    };
    let callable_data = jit_module.declare_data_in_func(data_id, &mut fb.func);
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
        let key = CString::new(layout.key.as_str())
            .map_err(|_| format!("field specialization attr contains NUL: {:?}", layout.key))?;
        if unsafe { ffi::PyObject_SetAttrString(temp_instance, key.as_ptr(), none) } != 0 {
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

fn load_field_index_specializations_from_path(
    path: &Path,
) -> Result<HashMap<String, Vec<FieldIndexSpecialization>>, String> {
    let dump = CounterDumpFile::open(path)?;
    let records = dump.records()?;
    let type_table = collect_type_table(records.as_slice())?;
    let type_key_layouts = collect_type_key_layouts(records.as_slice())?;
    let mut out = HashMap::<String, Vec<FieldIndexSpecialization>>::new();
    for (type_id, layouts) in type_key_layouts {
        let Some(type_key) = type_table.get(&type_id) else {
            continue;
        };
        let Some(owner_type) = resolve_type_key_to_type(type_key)? else {
            continue;
        };
        prime_field_index_layout(owner_type, layouts.as_slice())?;
        for layout in layouts {
            if let Some(specialization) =
                field_index_specialization_for_type(owner_type, layout.key.as_str(), layout.index)?
            {
                out.entry(layout.key).or_default().push(specialization);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
fn load_field_index_specializations()
-> Result<HashMap<String, Vec<FieldIndexSpecialization>>, String> {
    if specialization_mode_is_profile()? {
        return Ok(HashMap::new());
    }
    let Some(path) = counter_dump_input_path_from_env()? else {
        return Ok(HashMap::new());
    };
    let path = path.as_path();
    if !path.exists() {
        return Ok(HashMap::new());
    }
    load_field_index_specializations_from_path(path)
}

fn collect_cold_block_labels_from_path(
    path: &Path,
    function: &BlockPyFunction<CodegenModuleShape>,
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

fn emit_callee_function_id_checked(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
        jit_module,
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
        jit_module,
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
        jit_module,
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
    target_function: &BlockPyFunction<CodegenModuleShape>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
) -> ir::Value {
    debug_assert_eq!(arg_values.len(), target_function.params.len());
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    ctx.direct_edge_stats.record_resolved_direct_edge();

    let function_env = emit_direct_function_env_load(fb, callable, ctx);
    let function_env_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, function_env, null_ptr);
    let function_env_ok_block = fb.create_block();
    fb.ins().brif(
        function_env_is_null,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        function_env_ok_block,
        &[],
    );
    fb.switch_to_block(function_env_ok_block);

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
        let func_ref = jit_module.declare_func_in_func(direct_func_id, &mut fb.func);
        fb.ins().call(func_ref, &call_args)
    } else {
        ctx.direct_edge_stats.record_function_env_indirect_edge();
        let offset = match entry_kind {
            DirectCallEntryKind::Core => FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET,
            DirectCallEntryKind::DefaultResolving => FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET,
        };
        let callee_ptr = load_function_env_obj(fb, ptr_ty, function_env, offset);
        let direct_sig =
            fb.import_signature(make_direct_function_signature(jit_module, target_function));
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
    target_function: &BlockPyFunction<CodegenModuleShape>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
        jit_module,
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
    target_function: &BlockPyFunction<CodegenModuleShape>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
        emit_callable_ptr_value_for_ref(fb, jit_module, ctx, &specialization.init_function_ref)
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
        jit_module,
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
    target_function: &BlockPyFunction<CodegenModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
            jit_module,
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
        jit_module,
    )
}

fn emit_direct_constructor_resolved_with_args_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrCodegen],
    specialization: &DirectConstructorSpecialization,
    target_function: &BlockPyFunction<CodegenModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
            jit_module,
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
        jit_module,
    )
}

fn emit_direct_method_resolved_with_args_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    receiver_is_borrowed: bool,
    args: &[&InstrCodegen],
    specialization: &DirectMethodSpecialization,
    target_function: &BlockPyFunction<CodegenModuleShape>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
            jit_module,
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
        jit_module,
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
        jit_module,
    )
}

fn emit_call_direct_expr_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_blockpy::block_py::CallDirect<InstrCodegen>,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let fallback_call = || {
        InstrCodegen::Call(
            soac_blockpy::block_py::Call::new(
                (*call.callable).clone(),
                call.args.clone(),
                call.keywords.clone(),
            )
            .with_meta(call.meta()),
        )
    };

    let Some(target_function) = direct_call_target_function(ctx, call.function_id) else {
        ctx.direct_edge_stats
            .record_call_direct_missing_target_fallback();
        let fallback = fallback_call();
        return emit_codegen_expr_with_local_env(
            fb,
            &fallback,
            local_env,
            ctx,
            false,
            jit_module,
            func_imports,
        );
    };

    let arg_plan = match validate_direct_call_compatibility(
        target_function,
        ctx.direct_call_functions,
        direct_call_positional_arg_count(&call.args),
        0,
        direct_call_has_starred_arguments(&call.args, &call.keywords),
        !call.keywords.is_empty(),
    ) {
        Ok(arg_plan) => arg_plan,
        Err(_) => {
            ctx.direct_edge_stats
                .record_call_direct_unsupported_shape_fallback();
            let fallback = fallback_call();
            return emit_codegen_expr_with_local_env(
                fb,
                &fallback,
                local_env,
                ctx,
                false,
                jit_module,
                func_imports,
            );
        }
    };

    let callable_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
        call.callable.as_ref(),
        local_env,
        ctx,
    );
    let callable = emit_codegen_expr_with_local_env(
        fb,
        call.callable.as_ref(),
        local_env,
        ctx,
        callable_is_borrowed,
        jit_module,
        func_imports,
    );
    let args = call
        .args
        .iter()
        .map(|arg| match arg {
            CallArgPositional::Positional(expr) => expr,
            CallArgPositional::Starred(_) => {
                unreachable!("starred direct args should have used generic fallback")
            }
        })
        .collect::<Vec<_>>();
    emit_direct_call_resolved_with_arg_plan_from_local_env(
        fb,
        callable,
        callable_is_borrowed,
        args.as_slice(),
        &arg_plan,
        target_function,
        local_env,
        ctx,
        jit_module,
        func_imports,
    )
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
    _jit_module: &mut JITModule,
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
    function: &BlockPyFunction<CodegenModuleShape>,
    label: BlockLabel,
) -> Option<&str> {
    function.blocks[label.index()].exception_param()
}

fn emit_planned_local_releases_for_reason_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    reason: &RefcountReleaseReason,
    local_env: &mut LocalEnv,
    forwarded_locations: &HashSet<LocalLocation>,
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
        if removed.is_none() && !can_release_via_stack_slot_fallback(local.name.as_str()) {
            return Err(format!(
                "refcount plan release for block {source_label} references local {:?} \
                 without a LocalEnv binding",
                local.name
            ));
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
            facts: py_facts,
        } => emit_truthy_from_pyobject_value(fb, owned_value, py_facts, is_true_ref, ctx, true),
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
    jit_module: &mut JITModule,
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
        jit_module,
        func_imports,
    );
    SoacValue::pyobject(value, facts)
}

#[allow(dead_code)]
fn emit_typed_codegen_expr_value_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<SoacValue, String> {
    if let InstrTyped::Load(op) = expr {
        let facts = op
            .try_semantic_instr_id()
            .and_then(|instr_id| emit_ctx.value_facts_for_instr_id(instr_id))
            .and_then(ValueFacts::as_pyobj)
            .unwrap_or_else(PyObjFacts::unknown);
        let value = emit_resolved_name_load_with_local_env(
            fb,
            &op.name,
            op.try_semantic_instr_id(),
            local_env,
            emit_ctx,
            borrowed,
        );
        return Ok(SoacValue::pyobject(value, facts));
    }

    if let InstrTyped::Truthy(op) = expr {
        let value = emit_typed_codegen_expr_value_with_local_env(
            fb,
            op.value(),
            local_env,
            emit_ctx,
            false,
            jit_module,
            func_imports,
        )?;
        let is_true_ref = func_imports.get(jit_module, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?;
        return Ok(emit_truthy_from_owned_value(
            fb,
            value,
            is_true_ref,
            emit_ctx,
        ));
    }

    if let InstrTyped::BinOp(_) = expr {
        let legacy_expr = try_lower_typed_instr_to_codegen_legacy(expr.clone())?;
        return Ok(emit_codegen_expr_value_with_local_env(
            fb,
            &legacy_expr,
            local_env,
            emit_ctx,
            borrowed,
            jit_module,
            func_imports,
        ));
    }

    let legacy_expr = try_lower_typed_instr_to_codegen_legacy(expr.clone())?;
    Ok(emit_codegen_expr_value_with_local_env(
        fb,
        &legacy_expr,
        local_env,
        emit_ctx,
        borrowed,
        jit_module,
        func_imports,
    ))
}

struct SimpleCallParts<'a> {
    simple_args: Vec<&'a InstrCodegen>,
    simple_keywords: Vec<(&'a str, &'a InstrCodegen)>,
    has_unpack: bool,
}

fn simple_call_parts(call: &soac_blockpy::block_py::Call<InstrCodegen>) -> SimpleCallParts<'_> {
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

#[allow(clippy::too_many_arguments)]
fn emit_codegen_simple_call_effect_only_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
            jit_module,
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
            jit_module,
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
            jit_module,
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
            jit_module,
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
    if !direct_constructor_specializations_for_call_site(call, emit_ctx).is_empty()
        || !direct_method_specializations_for_call_site(call, emit_ctx).is_empty()
        || site_instr_id
            .and_then(|site_instr_id| emit_ctx.call_target_specializations.get(&site_instr_id))
            .is_some_and(|targets| !targets.is_empty())
    {
        return None;
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
        jit_module,
        func_imports,
    );
    if let Some(counter_id) = site_instr_id
        .and_then(|site_instr_id| emit_ctx.call_target_counter_ids.get(&site_instr_id))
        .copied()
    {
        let callee_id = emit_callee_function_id_checked(fb, callable, emit_ctx, jit_module);
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
            jit_module,
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
            jit_module,
            func_imports,
            ResultDemand::EffectOnly,
        )
    })
}

fn emit_codegen_simple_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<ir::Value> {
    let SimpleCallParts {
        simple_args,
        simple_keywords,
        has_unpack,
    } = simple_call_parts(call);

    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

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
            jit_module,
            func_imports,
        ));
    }

    if !has_unpack
        && simple_keywords.is_empty()
        && codegen_expr_runtime_helper(call.func.as_ref(), emit_ctx) == Some(RuntimeHelperId::Str)
        && simple_args.len() == 1
    {
        if let Some(value) = codegen_expr_const_string(simple_args[0], emit_ctx.module_constants) {
            return Some(emit_owned_module_constant(
                fb,
                emit_ctx
                    .module_constants
                    .require_unicode_constant_id(value.as_str()),
                emit_ctx,
            ));
        }
    }

    if !has_unpack
        && simple_keywords.is_empty()
        && simple_args.len() == 1
        && codegen_expr_runtime_helper(call.func.as_ref(), emit_ctx)
            == Some(RuntimeHelperId::NextOrSentinel)
    {
        let iterator_expr = simple_args[0];
        let iterator_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
            iterator_expr,
            local_env,
            emit_ctx,
        );
        let iterator = emit_codegen_expr_with_local_env(
            fb,
            iterator_expr,
            local_env,
            emit_ctx,
            iterator_is_borrowed,
            jit_module,
            func_imports,
        );
        let sentinel = emit_owned_module_constant(
            fb,
            emit_ctx
                .module_constants
                .require_runtime_name_constant_id("ITER_COMPLETE"),
            emit_ctx,
        );
        let next_or_sentinel_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_NEXT_OR_SENTINEL_IMPORT);
        let next_inst = fb.ins().call(next_or_sentinel_ref, &[iterator, sentinel]);
        let mut owned_inputs = Vec::with_capacity(2);
        if !iterator_is_borrowed {
            owned_inputs.push(iterator);
        }
        owned_inputs.push(sentinel);
        let next_value = emit_decref_owned_inputs_after_nullable_result(
            fb,
            emit_ctx,
            fb.inst_results(next_inst)[0],
            &owned_inputs,
        );
        let value_is_null = fb
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, next_value, null_ptr);
        let value_ok_block = fb.create_block();
        fb.append_block_param(value_ok_block, ptr_ty);
        fb.ins().brif(
            value_is_null,
            emit_ctx.consts.step_null_block,
            &step_null_block_args(emit_ctx),
            value_ok_block,
            &[ir::BlockArg::Value(next_value)],
        );
        fb.switch_to_block(value_ok_block);
        return Some(fb.block_params(value_ok_block)[0]);
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
            jit_module,
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
            jit_module,
            func_imports,
        ));
    }

    if !has_unpack
        && simple_keywords.is_empty()
        && let Some(helper_id) = codegen_expr_runtime_helper(call.func.as_ref(), emit_ctx)
    {
        if helper_id == RuntimeHelperId::RaiseDeletedName
            && simple_args.len() == 1
            && let Some(name) = codegen_expr_const_string(simple_args[0], emit_ctx.module_constants)
        {
            let name_obj = emit_owned_module_constant(
                fb,
                emit_ctx
                    .module_constants
                    .require_unicode_constant_id(name.as_str()),
                emit_ctx,
            );
            fb.ins()
                .call(emit_ctx.raise_deleted_name_error_ref, &[name_obj]);
            emit_release_owned_inputs(fb, emit_ctx, &[name_obj]);
            fb.ins().jump(
                emit_ctx.consts.step_null_block,
                &step_null_block_args(emit_ctx),
            );
            return Some(null_ptr);
        }
        if helper_id == RuntimeHelperId::LoadDeletedName
            && simple_args.len() == 2
            && let Some(name) = codegen_expr_const_string(simple_args[0], emit_ctx.module_constants)
        {
            let name_obj = emit_owned_module_constant(
                fb,
                emit_ctx
                    .module_constants
                    .require_unicode_constant_id(name.as_str()),
                emit_ctx,
            );
            let value_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
                simple_args[1],
                local_env,
                emit_ctx,
            );
            let value_obj = emit_codegen_expr_with_local_env(
                fb,
                simple_args[1],
                local_env,
                emit_ctx,
                value_borrowed,
                jit_module,
                func_imports,
            );
            let value_is_null = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, value_obj, null_ptr);
            let deleted_block = fb.create_block();
            let value_ok_block = fb.create_block();
            fb.append_block_param(value_ok_block, ptr_ty);
            fb.ins().brif(
                value_is_null,
                deleted_block,
                &[],
                value_ok_block,
                &[ir::BlockArg::Value(value_obj)],
            );

            fb.switch_to_block(deleted_block);
            fb.ins()
                .call(emit_ctx.raise_deleted_name_error_ref, &[name_obj]);
            emit_release_owned_inputs(fb, emit_ctx, &[name_obj]);
            if !value_borrowed {
                emit_decref_if_not_null(
                    fb,
                    emit_ctx.consts.ptr_ty,
                    emit_ctx.decref_ref,
                    emit_ctx.consts.thread_state_value,
                    value_obj,
                );
            }
            fb.ins().jump(
                emit_ctx.consts.step_null_block,
                &step_null_block_args(emit_ctx),
            );

            fb.switch_to_block(value_ok_block);
            let value_obj = fb.block_params(value_ok_block)[0];
            fb.ins().call(
                emit_ctx.decref_ref,
                &[emit_ctx.consts.thread_state_value, name_obj],
            );
            if value_borrowed {
                fb.ins().call(emit_ctx.incref_ref, &[value_obj]);
            }
            return Some(value_obj);
        }
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
        let constructor_specializations =
            direct_constructor_specializations_for_call_site(call, emit_ctx);
        let direct_specializations = site_instr_id
            .and_then(|site_instr_id| emit_ctx.call_target_specializations.get(&site_instr_id))
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
        let direct_method_specializations =
            direct_method_specializations_for_call_site(call, emit_ctx);
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
                jit_module,
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
                        emit_ctx.guard_miss_target_for_resume_point(
                            guard_miss_resume_point,
                            &[getattr.value.as_ref()],
                            generic_block,
                        ),
                        generic_block,
                        emit_ctx.guard_miss_deopt_stub_ref,
                    )
                })
                .unwrap_or(JitGuardMissDispatch::FallbackBlock(generic_block));
            for (index, specialization) in direct_method_specializations.iter().enumerate() {
                let Some(expected_type) = emit_type_ptr_value_for_ref(
                    fb,
                    jit_module,
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
                        specialization.function_id.packed() as i64,
                    );
                    emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
                }
                if let Some(counter_id) = direct_hit_counter_id {
                    let _ = emit_increment_counter(fb, counter_id, emit_ctx);
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
                    jit_module,
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
                        jit_module,
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
                            emit_callee_function_id_checked(fb, callable, emit_ctx, jit_module);
                        emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
                    }
                    if let Some(counter_id) = direct_fallback_counter_id {
                        let _ = emit_increment_counter(fb, counter_id, emit_ctx);
                    }
                    let generic_result = if simple_args.len() <= 3 {
                        emit_positional_call_three_with_local_env(
                            fb,
                            callable,
                            false,
                            simple_args.as_slice(),
                            local_env,
                            emit_ctx,
                            jit_module,
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
                            jit_module,
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
                        let _ = emit_increment_counter(fb, counter_id, emit_ctx);
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
            jit_module,
            func_imports,
        );
        let should_emit_callee_id = call_target_counter.is_some()
            || !constructor_specializations.is_empty()
            || !direct_specializations.is_empty();
        let callee_id = should_emit_callee_id
            .then(|| emit_callee_function_id_checked(fb, callable, emit_ctx, jit_module));
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
                    emit_ctx.guard_miss_target_for_resume_point(
                        guard_miss_resume_point,
                        &[call.func.as_ref()],
                        generic_block,
                    ),
                    generic_block,
                    emit_ctx.guard_miss_deopt_stub_ref,
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
                        jit_module,
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
                        let _ = emit_increment_counter(fb, counter_id, emit_ctx);
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
                        jit_module,
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
                    jit_module,
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
                        specialization.function_id.packed() as i64,
                    );
                    let is_match = fb.ins().band(is_match, callable_is_exact_function);
                    fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

                    fb.switch_to_block(direct_block);
                    let target_function =
                        direct_call_target_function(emit_ctx, specialization.function_id)
                            .expect("direct specialization target should exist");
                    if let Some(counter_id) = direct_hit_counter_id {
                        let _ = emit_increment_counter(fb, counter_id, emit_ctx);
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
                        jit_module,
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
                        let _ = emit_increment_counter(fb, counter_id, emit_ctx);
                    }
                    let generic_result = if simple_args.len() <= 3 {
                        emit_positional_call_three_with_local_env(
                            fb,
                            callable,
                            callable_is_borrowed,
                            simple_args.as_slice(),
                            local_env,
                            emit_ctx,
                            jit_module,
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
                            jit_module,
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
                        let _ = emit_increment_counter(fb, counter_id, emit_ctx);
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
            jit_module,
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
            jit_module,
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
            jit_module,
            func_imports,
        ));
    }

    None
}

fn emit_codegen_make_function_with_closure_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    make_function: &soac_blockpy::block_py::MakeFunctionWithClosure<InstrCodegen>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let callable = emit_soac_ext_make_function_callable(fb, emit_ctx);
    let function_id = emit_owned_module_constant(
        fb,
        emit_ctx
            .module_constants
            .require_u64_constant_id(make_function.function_id().packed()),
        emit_ctx,
    );
    let kind = emit_owned_module_constant(
        fb,
        emit_ctx
            .module_constants
            .require_unicode_constant_id(make_function.kind.make_function_kind_name()),
        emit_ctx,
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
        jit_module,
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
        jit_module,
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
        jit_module,
        func_imports,
    );
    let globals = emit_ctx.consts.block_const;
    let result = emit_positional_vectorcall_result_with_arg_values(
        fb,
        callable,
        false,
        vec![
            function_id,
            kind,
            captures,
            param_defaults,
            annotate_fn,
            globals,
        ],
        vec![
            false,
            false,
            captures_is_borrowed,
            param_defaults_is_borrowed,
            annotate_fn_is_borrowed,
            true,
        ],
        emit_ctx,
        ResultDemand::PYOBJECT_OWNED,
    );
    let (value, ownership, _) = result.expect_pyobject("make-function-with-closure result");
    debug_assert!(ownership.is_owned());
    value
}

fn emit_codegen_expr_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrCodegen,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    jit_module: &mut JITModule,
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
            jit_module,
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
            jit_module,
            func_imports,
        );
    }
    if let InstrCodegen::CalleeFunctionId(op) = expr {
        assert!(
            !borrowed,
            "callee_function_id must not request a borrowed result"
        );
        let callable_is_borrowed = codegen_expr_pyobject_input_is_borrowed_from_local_env(
            op.value.as_ref(),
            local_env,
            emit_ctx,
        );
        let callable = emit_codegen_expr_with_local_env(
            fb,
            op.value.as_ref(),
            local_env,
            emit_ctx,
            callable_is_borrowed,
            jit_module,
            func_imports,
        );
        let callee_id = emit_callee_function_id_checked(fb, callable, emit_ctx, jit_module);
        if !callable_is_borrowed {
            fb.ins().call(
                emit_ctx.decref_ref,
                &[emit_ctx.consts.thread_state_value, callable],
            );
        }
        return callee_id;
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
            jit_module,
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
            jit_module,
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
            jit_module,
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
            jit_module,
            func_imports,
        };
        return intrinsics::OperationEmitState::finish_owned_result(
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
            jit_module,
            func_imports,
        };
        return intrinsics::emit_del_deref_raw_cell(raw_cell, op.quietly, &mut intrinsic_state);
    }
    if let InstrCodegen::CallDirect(call) = expr {
        assert!(
            !borrowed,
            "codegen direct-call expression must not use borrowed result"
        );
        return emit_call_direct_expr_with_local_env(
            fb,
            call,
            local_env,
            emit_ctx,
            jit_module,
            func_imports,
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
            jit_module,
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
            jit_module,
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
            fb.ins().call(
                emit_ctx.decref_ref,
                &[emit_ctx.consts.thread_state_value, value],
            );
            EmitResult::no_value()
        }
        ResultDemand::PyObject { .. } => EmitResult::owned_pyobject(value, facts),
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
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
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

#[cfg(test)]
fn static_runtime_primitive_for_call(
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
    module_constants: &ModuleCodegenConstants,
) -> Option<direct_abi::RuntimePrimitiveId> {
    let desc = static_runtime_primitive_desc_for_call(call, module_constants)?;
    let DirectTargetId::RuntimePrimitive(primitive) = desc.target else {
        return None;
    };
    Some(primitive)
}

fn static_runtime_primitive_desc_for_call(
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
    module_constants: &ModuleCodegenConstants,
) -> Option<&'static DirectCallableDesc> {
    let name = codegen_expr_static_runtime_name(call.func.as_ref(), module_constants)?;
    let primitive = direct_abi::runtime_primitive_for_builtin_name(name)?;
    let desc = direct_abi::runtime_primitive_desc(primitive);
    let _ = direct_positional_call_args(call, desc.abi.params.len())?;
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

fn codegen_expr_can_satisfy_i64_demand(
    expr: &InstrCodegen,
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
) -> bool {
    codegen_expr_i64_demand_facts(expr, local_env, emit_ctx).is_some()
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

fn runtime_primitive_call_params_can_satisfy_abi(
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
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
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
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
            let (boxed, boxed_facts) = boxed.expect_pyobject("I64 Python object demand");
            EmitResult::owned_pyobject(boxed, boxed_facts)
        }
    }
}

fn emit_checked_i64_overflow_result(
    fb: &mut FunctionBuilder<'_>,
    value: ir::Value,
    overflow: ir::Value,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
        func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_RAISE_I64_OVERFLOW_IMPORT);
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
    jit_module: &mut JITModule,
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
        jit_module,
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
        jit_module,
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
        jit_module,
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

fn emit_exact_pylong_as_i64_saturating_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrCodegen,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    jit_module: &mut JITModule,
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
        jit_module,
        func_imports,
    );
    let pylong_as_i64_saturating_ref = func_imports.get_or_panic(
        jit_module,
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
    jit_module: &mut JITModule,
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
                jit_module,
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
                jit_module,
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
                jit_module,
                func_imports,
            )
            .expect("I64-capable runtime builtin argument should emit");
            let (value, _) = arg_result.expect_i64("runtime primitive I64 param");
            (value, None)
        }
        ParamAbi::I32 => panic!("runtime primitive I32 params are not implemented"),
    }
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
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
    desc: &DirectCallableDesc,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    jit_module: &mut JITModule,
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
            jit_module,
            func_imports,
        );
        call_args.push(value);
        if let Some(owned_after_call) = owned_after_call {
            owned_inputs.push(owned_after_call);
        }
    }
    let func_ref = func_imports.get_or_panic(
        jit_module,
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

fn emit_runtime_builtin_primitive_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    jit_module: &mut JITModule,
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
            jit_module,
            func_imports,
        ),
    )
}

fn emit_codegen_call_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    if let Some(result) = emit_runtime_builtin_primitive_call_result_with_local_env(
        fb,
        call,
        local_env,
        emit_ctx,
        demand,
        jit_module,
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
            jit_module,
            func_imports,
        )
    {
        return Some(result);
    }
    emit_codegen_simple_call_with_local_env(fb, call, local_env, emit_ctx, jit_module, func_imports)
        .map(|value| {
            emit_owned_pyobject_result_for_demand(
                fb,
                value,
                PyObjFacts::unknown(),
                emit_ctx,
                demand,
            )
        })
}

fn emit_codegen_call_direct_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_blockpy::block_py::CallDirect<InstrCodegen>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> EmitResult {
    let value = emit_call_direct_expr_with_local_env(
        fb,
        call,
        local_env,
        emit_ctx,
        jit_module,
        func_imports,
    );
    emit_owned_pyobject_result_for_demand(fb, value, PyObjFacts::unknown(), emit_ctx, demand)
}

fn emit_codegen_stmt_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrCodegen,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    jit_module: &mut JITModule,
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
                jit_module,
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
                jit_module,
                func_imports,
            ) {
                return Ok(result);
            }
        }
        InstrCodegen::CallDirect(call) => {
            return Ok(emit_codegen_call_direct_result_with_local_env(
                fb,
                call,
                local_env,
                emit_ctx,
                demand,
                jit_module,
                func_imports,
            ));
        }
        InstrCodegen::Call(call) => {
            if let Some(result) = emit_codegen_call_result_with_local_env(
                fb,
                call,
                local_env,
                emit_ctx,
                demand,
                jit_module,
                func_imports,
            ) {
                return Ok(result);
            }
        }
        _ => {}
    }
    let value =
        emit_codegen_stmt_with_local_env(fb, expr, local_env, emit_ctx, jit_module, func_imports);
    Ok(emit_owned_pyobject_result_for_demand(
        fb,
        value,
        PyObjFacts::unknown(),
        emit_ctx,
        demand,
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

#[allow(dead_code)]
fn emit_typed_codegen_expr_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        expr,
        local_env,
        emit_ctx,
        borrowed,
        jit_module,
        func_imports,
    )?;
    Ok(match value {
        SoacValue::PyObject { value, .. } => value,
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
    jit_module: &mut JITModule,
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
                jit_module,
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
        jit_module,
        func_imports,
    )
}

#[allow(dead_code)]
fn emit_typed_codegen_stmt_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<ir::Value, String> {
    if matches!(
        expr,
        InstrTyped::Truthy(_) | InstrTyped::Load(_) | InstrTyped::BinOp(_)
    ) {
        return emit_typed_codegen_expr_with_local_env(
            fb,
            expr,
            local_env,
            emit_ctx,
            false,
            jit_module,
            func_imports,
        );
    }

    let legacy_expr = try_lower_typed_instr_to_codegen_legacy(expr.clone())?;
    Ok(emit_codegen_stmt_with_local_env(
        fb,
        &legacy_expr,
        local_env,
        emit_ctx,
        jit_module,
        func_imports,
    ))
}

fn emit_typed_codegen_stmt_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    if let InstrTyped::LegacyStore(op) = expr {
        if let Some(result) = emit_typed_local_store_result_with_local_env(
            fb,
            expr,
            op,
            local_env,
            emit_ctx,
            demand,
            jit_module,
            func_imports,
        )? {
            return Ok(result);
        }
    }

    if matches!(
        expr,
        InstrTyped::Truthy(_) | InstrTyped::Load(_) | InstrTyped::BinOp(_)
    ) {
        if demand == ResultDemand::I32_BOOL01 {
            return emit_typed_codegen_i32_bool01_result_with_local_env(
                fb,
                expr,
                local_env,
                emit_ctx,
                jit_module,
                func_imports,
            );
        }
        let value = emit_typed_codegen_stmt_with_local_env(
            fb,
            expr,
            local_env,
            emit_ctx,
            jit_module,
            func_imports,
        )?;
        return Ok(match demand {
            ResultDemand::EffectOnly => {
                fb.ins().call(
                    emit_ctx.decref_ref,
                    &[emit_ctx.consts.thread_state_value, value],
                );
                EmitResult::no_value()
            }
            ResultDemand::PyObject { .. } => {
                EmitResult::owned_pyobject(value, PyObjFacts::unknown())
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
        jit_module,
        func_imports,
    )
}

fn emit_typed_codegen_i32_bool01_result_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let is_true_ref = func_imports.get(jit_module, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT)?;
    let value = emit_typed_codegen_expr_value_with_local_env(
        fb,
        expr,
        local_env,
        emit_ctx,
        false,
        jit_module,
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
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
    pyobject_to_i64_ref: ir::FuncRef,
) -> Result<EmitResult, String> {
    let legacy_expr = try_lower_typed_instr_to_codegen_legacy(expr.clone())?;
    let index_i64 = emit_branch_index_i64(
        fb,
        &legacy_expr,
        local_env,
        emit_ctx,
        jit_module,
        func_imports,
        pyobject_to_i64_ref,
    );
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
    jit_module: &mut JITModule,
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
            stmt_emit_ctx.result_demand_plan.demand_for_typed_stmt(expr),
            jit_module,
            func_imports,
        )?;
        discard_emit_result(fb, result, emit_ctx)?;
    }
    Ok(())
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
    exec_blocks: &[ir::Block],
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    fb.switch_to_block(branch_block);
    let target_index = target_label.index();
    let edge_transport = &implicit_target_transports[target_index];
    let mut jump_args = Vec::with_capacity(edge_transport.target_args.len());
    let (prepared_args, forwarded_locations) = emit_planned_target_args_codegen_from_local_env(
        fb,
        &edge_transport.target_args,
        local_env,
        emit_ctx,
        jit_module,
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
    function: &BlockPyFunction<CodegenModuleShape>,
    exec_blocks: &[ir::Block],
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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

    let prefer_true = test_instr_id
        .and_then(|test_instr_id| emit_ctx.branch_prefer_true.get(&test_instr_id).copied())
        .unwrap_or(true);
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
        exec_blocks,
        implicit_target_transports,
        &mut hot_local_env,
        emit_ctx,
        jit_module,
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
        exec_blocks,
        implicit_target_transports,
        &mut cold_local_env,
        emit_ctx,
        jit_module,
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
    let forwarded_locations = HashSet::new();
    let release_reason = RefcountReleaseReason::Return;
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
    function: &BlockPyFunction<CodegenModuleShape>,
    exec_blocks: &[ir::Block],
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
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
        let target_index = target_label.index();
        let edge_transport = &implicit_target_transports[target_index];
        let mut case_local_env = local_env.clone();
        let mut case_jump_args = Vec::with_capacity(edge_transport.target_args.len());
        let (prepared_args, forwarded_locations) =
            emit_planned_target_args_codegen_from_local_env(
                fb,
                &edge_transport.target_args,
                &case_local_env,
                emit_ctx,
                jit_module,
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
    let default_index = default_label.index();
    let edge_transport = &implicit_target_transports[default_index];
    let mut default_local_env = local_env.clone();
    let mut default_jump_args = Vec::with_capacity(edge_transport.target_args.len());
    let (prepared_args, forwarded_locations) = emit_planned_target_args_codegen_from_local_env(
        fb,
        &edge_transport.target_args,
        &default_local_env,
        emit_ctx,
        jit_module,
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
    let raise_name_obj = emit_owned_module_constant(
        fb,
        emit_ctx
            .module_constants
            .require_unicode_constant_id("raise_from"),
        emit_ctx,
    );
    let raise_fn_inst = fb
        .ins()
        .call(emit_ctx.load_runtime_obj_ref, &[raise_name_obj]);
    fb.ins().call(
        emit_ctx.decref_ref,
        &[emit_ctx.consts.thread_state_value, raise_name_obj],
    );
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

fn emit_codegen_term(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    term: &BlockTerm<InstrCodegen>,
    function: &BlockPyFunction<CodegenModuleShape>,
    exec_blocks: &[ir::Block],
    jump_edge_transports: &[Option<EdgeTransportPlan>],
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
    is_true_ref: ir::FuncRef,
    pyobject_to_i64_ref: ir::FuncRef,
    raise_exc_ref: ir::FuncRef,
    current_exception_name: Option<&str>,
) -> Result<(), String> {
    let decref_ref = emit_ctx.decref_ref;

    match term {
        BlockTerm::Jump(target_label) => {
            let target_index = target_label.target.index();
            let edge_transport = jump_edge_transports[source_label.index()]
                .as_ref()
                .expect("jump term should have a planned edge transport");
            let mut jump_args = Vec::with_capacity(edge_transport.target_args.len());
            let (prepared_args, forwarded_locations) =
                emit_planned_target_args_codegen_from_local_env(
                    fb,
                    &edge_transport.target_args,
                    local_env,
                    emit_ctx,
                    jit_module,
                    func_imports,
                )
                .map_err(|err| {
                    format!(
                        "missing local mapping for jump block params in block {source_label}: {err}"
                    )
                })?;
            jump_args.extend(prepared_args);
            let release_reason = RefcountReleaseReason::Jump {
                target: target_label.target,
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
                decref_ref,
            );
            emit_pop_handled_exception_if_leaving(
                fb,
                current_exception_name,
                block_exception_name(function, target_label.target),
                emit_ctx,
            );
            fb.ins().jump(exec_blocks[target_index], &jump_args);
        }
        BlockTerm::IfTerm(if_term) => {
            let test_instr_id = if_term.test.try_semantic_instr_id();
            let test_value = emit_codegen_expr_value_with_local_env(
                fb,
                &if_term.test,
                local_env,
                emit_ctx,
                false,
                jit_module,
                func_imports,
            );
            let truth = emit_truthy_from_owned_value(fb, test_value, is_true_ref, emit_ctx);
            let truth_i32 = truth.expect_i32_bool01("if condition truthiness");
            emit_codegen_if_truth_i32(
                fb,
                source_label,
                test_instr_id,
                truth_i32,
                if_term.then_label,
                if_term.else_label,
                current_exception_name,
                function,
                exec_blocks,
                implicit_target_transports,
                local_env,
                emit_ctx,
                jit_module,
                func_imports,
            )?;
        }
        BlockTerm::BranchTable(branch) => {
            let index_i64 = emit_branch_index_i64(
                fb,
                &branch.index,
                local_env,
                emit_ctx,
                jit_module,
                func_imports,
                pyobject_to_i64_ref,
            );
            emit_codegen_branch_table_from_i64(
                fb,
                source_label,
                &branch.targets,
                branch.default_label,
                index_i64,
                function,
                exec_blocks,
                implicit_target_transports,
                local_env,
                emit_ctx,
                jit_module,
                func_imports,
                current_exception_name,
            )?;
        }
        BlockTerm::Return(value) => {
            let ret_value = emit_codegen_expr_with_local_env(
                fb,
                value,
                local_env,
                emit_ctx,
                false,
                jit_module,
                func_imports,
            );
            emit_codegen_return_pyobject(
                fb,
                source_label,
                ret_value,
                local_env,
                emit_ctx,
                current_exception_name,
            )?;
        }
        BlockTerm::Raise(raise_stmt) => {
            let raise_fn = emit_load_raise_from_function(fb, emit_ctx);
            let exc_value = if let Some(exc_expr) = raise_stmt.exc.as_ref() {
                emit_codegen_expr_with_local_env(
                    fb,
                    exc_expr,
                    local_env,
                    emit_ctx,
                    false,
                    jit_module,
                    func_imports,
                )
            } else {
                let none_const = emit_none_const(fb, emit_ctx);
                fb.ins().call(emit_ctx.incref_ref, &[none_const]);
                none_const
            };
            emit_codegen_raise_exception_from_function(
                fb,
                source_label,
                raise_fn,
                exc_value,
                ValueOwnership::Owned,
                local_env,
                emit_ctx,
                raise_exc_ref,
                current_exception_name,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_codegen_term(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    term: &BlockTerm<InstrTyped>,
    function: &BlockPyFunction<CodegenModuleShape>,
    exec_blocks: &[ir::Block],
    jump_edge_transports: &[Option<EdgeTransportPlan>],
    implicit_target_transports: &[EdgeTransportPlan],
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
    is_true_ref: ir::FuncRef,
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
        let demand = test_instr_id
            .and_then(|instr_id| emit_ctx.result_demand_plan.demand_for_instr_id(instr_id))
            .unwrap_or(ResultDemand::I32_BOOL01);
        let truth = match demand {
            ResultDemand::I32Bool01 => emit_typed_codegen_i32_bool01_result_with_local_env(
                fb,
                &if_term.test,
                local_env,
                emit_ctx,
                jit_module,
                func_imports,
            )?,
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
            implicit_target_transports,
            local_env,
            emit_ctx,
            jit_module,
            func_imports,
        );
    }

    if let BlockTerm::Return(value) = term {
        let term_emit_ctx = typed_nested_guard_misses_can_resume_before_instr(value)
            .then(|| emit_ctx.with_guard_miss_resume_point(term_guard_miss_resume_point));
        let emit_ctx = term_emit_ctx.as_ref().unwrap_or(emit_ctx);
        let value_instr_id = value.try_semantic_instr_id();
        let demand = value_instr_id
            .and_then(|instr_id| emit_ctx.result_demand_plan.demand_for_instr_id(instr_id))
            .unwrap_or(ResultDemand::PYOBJECT_OWNED);
        let result = match demand {
            ResultDemand::PyObject { borrowed_ok: false } => {
                emit_typed_codegen_stmt_result_with_local_env(
                    fb,
                    value,
                    local_env,
                    emit_ctx,
                    demand,
                    jit_module,
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
            let exc_instr_id = exc_expr.try_semantic_instr_id();
            let demand = exc_instr_id
                .and_then(|instr_id| emit_ctx.result_demand_plan.demand_for_instr_id(instr_id))
                .unwrap_or(ResultDemand::PYOBJECT_OWNED);
            let result = match demand {
                ResultDemand::PyObject { borrowed_ok: false } => {
                    emit_typed_codegen_stmt_result_with_local_env(
                        fb,
                        exc_expr,
                        local_env,
                        emit_ctx,
                        demand,
                        jit_module,
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
        let index_instr_id = branch.index.try_semantic_instr_id();
        let demand = index_instr_id
            .and_then(|instr_id| emit_ctx.result_demand_plan.demand_for_instr_id(instr_id))
            .unwrap_or(ResultDemand::I64_INDEX);
        let index = match demand {
            ResultDemand::I64Index => emit_typed_codegen_i64_index_result_with_local_env(
                fb,
                &branch.index,
                local_env,
                emit_ctx,
                jit_module,
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
            implicit_target_transports,
            local_env,
            emit_ctx,
            jit_module,
            func_imports,
            current_exception_name,
        );
    }

    let legacy_term = try_lower_typed_term_to_codegen_legacy(term.clone())?;
    emit_codegen_term(
        fb,
        source_label,
        &legacy_term,
        function,
        exec_blocks,
        jump_edge_transports,
        implicit_target_transports,
        local_env,
        emit_ctx,
        jit_module,
        func_imports,
        is_true_ref,
        pyobject_to_i64_ref,
        raise_exc_ref,
        current_exception_name,
    )
}

fn new_jit_builder() -> Result<JITBuilder, String> {
    let isa = CraneliftTargetConfig::runtime_from_env()?.build_isa()?;
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
    builder.symbol_lookup_fn(Box::new(lookup_registered_jit_data_symbol));
    register_specialized_jit_symbols(builder);
}

fn new_jit_module(_compile_session: &crate::session::CompileSession) -> Result<JITModule, String> {
    let mut jit_module = JITModule::new(new_jit_builder()?);
    load_runtime_support_clif(&mut jit_module)?;
    Ok(jit_module)
}

pub(crate) fn process_jit_is_currently_compiling() -> bool {
    PROCESS_JIT_COMPILE_DEPTH.with(|depth| depth.get() > 0)
}

impl ProcessJitCompileGuard {
    fn enter() -> Self {
        PROCESS_JIT_COMPILE_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}

impl Drop for ProcessJitCompileGuard {
    fn drop(&mut self) {
        PROCESS_JIT_COMPILE_DEPTH.with(|depth| {
            let current = depth.get();
            debug_assert!(current > 0);
            depth.set(current.saturating_sub(1));
        });
    }
}

fn collect_process_jit_batch_functions<'a>(
    session: &Arc<crate::session::CompileSession>,
    root: &BlockPyFunction<CodegenModuleShape>,
    direct_call_resolver: Option<&'a crate::module_type::SharedModuleState>,
) -> Result<Vec<ProcessJitBatchFunction<'a>>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    let root_module_id = root.function_id.module_id();
    seen.insert(root.function_id);
    queue.push_back(ProcessJitBatchFunction {
        function: root.clone(),
        source: direct_call_resolver
            .map(ProcessJitBatchFunctionSource::BorrowedSharedState)
            .unwrap_or(ProcessJitBatchFunctionSource::ExplicitInputs),
    });
    while let Some(batch_function) = queue.pop_front() {
        let mut direct_targets = collect_call_direct_targets(&batch_function.function);
        if let Some(shared_state) = batch_function.source.shared_state() {
            for targets in load_call_target_specializations(
                shared_state.module_name.as_str(),
                batch_function.function.function_id,
            )?
            .values()
            {
                direct_targets.extend(targets.iter().copied());
            }
        }
        for function_id in direct_targets {
            if !seen.insert(function_id) {
                continue;
            }
            if let Some(function) =
                resolve_process_jit_batch_function(session, direct_call_resolver, function_id)?
            {
                if function.function.function_id.module_id() != root_module_id {
                    continue;
                }
                queue.push_back(function);
            }
        }
        for function_id in collect_make_function_targets(&batch_function.function) {
            if seen.contains(&function_id) {
                continue;
            }
            if let Some(function) =
                resolve_process_jit_batch_function(session, direct_call_resolver, function_id)?
            {
                if function.function.function_id.module_id() != root_module_id {
                    continue;
                }
                if is_synthetic_class_helper_function(&function.function) {
                    seen.insert(function_id);
                    queue.push_back(function);
                }
            }
        }
        out.push(batch_function);
    }
    Ok(out)
}

fn resolve_process_jit_batch_function<'a>(
    session: &Arc<crate::session::CompileSession>,
    direct_call_resolver: Option<&'a crate::module_type::SharedModuleState>,
    function_id: FunctionId,
) -> Result<Option<ProcessJitBatchFunction<'a>>, String> {
    if function_id == FunctionId::global() {
        return Ok(None);
    }
    if let Some(shared_state) = direct_call_resolver
        && let Some(function) = shared_state.lookup_function(function_id).cloned()
    {
        return Ok(Some(ProcessJitBatchFunction {
            function,
            source: ProcessJitBatchFunctionSource::BorrowedSharedState(shared_state),
        }));
    }
    Ok(session
        .lookup_shared_function(function_id)?
        .map(|(shared_state, function)| ProcessJitBatchFunction {
            function,
            source: ProcessJitBatchFunctionSource::OwnedSharedState(shared_state),
        }))
}

impl ProcessJitEngine {
    pub(crate) fn new(compile_session: &crate::session::CompileSession) -> Result<Self, String> {
        Ok(Self {
            state: Mutex::new(ProcessJitState::new(compile_session)?),
            vectorcall_trampolines: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn vectorcall_trampoline(
        &self,
        _compile_session: &crate::session::CompileSession,
        param_count: usize,
    ) -> Result<VectorcallEntryFn, String> {
        let mut trampolines = self
            .vectorcall_trampolines
            .lock()
            .map_err(|_| "process JIT vectorcall trampoline cache lock poisoned".to_string())?;
        if let Some(entry) = trampolines.get(&param_count).copied() {
            return Ok(entry);
        }

        let mut state = self
            .state
            .lock()
            .map_err(|_| "process JIT module lock poisoned".to_string())?;
        let symbol = format!("__soac_vectorcall_arity_{param_count}");
        let entry =
            define_shared_vectorcall_trampoline(&mut state.jit_module, param_count, &symbol)?;
        trampolines.insert(param_count, entry);
        Ok(entry)
    }

    pub(crate) unsafe fn compile_direct_function(
        &self,
        session: &Arc<crate::session::CompileSession>,
        blocks: &[ObjPtr],
        module: &BlockPyModule<CodegenModuleShape>,
        function: &BlockPyFunction<CodegenModuleShape>,
        module_constants: &ModuleCodegenConstants,
        counter_defs: &[CounterDef],
        module_constant_ptrs: &[*mut ffi::PyObject],
        direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
    ) -> Result<DirectFunctionCompileResult, String> {
        let batch_functions =
            collect_process_jit_batch_functions(session, function, direct_call_resolver)?;
        let root_function_id = function.function_id;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "process JIT module lock poisoned".to_string())?;
        if let Some(compiled_handle) = state.ready_direct_function(function) {
            return Ok(DirectFunctionCompileResult {
                handle: compiled_handle,
                compiled: false,
                stats: None,
            });
        }
        let _guard = ProcessJitCompileGuard::enter();
        let mut predeclared = HashMap::new();
        let mut functions_to_define = Vec::new();
        for batch_function in &batch_functions {
            let function = &batch_function.function;
            let direct_symbol_scope = batch_function.source.shared_state().map(|shared_state| {
                direct_function_symbol_scope_for_shared_state(shared_state, function.function_id)
            });
            let declared =
                state.declare_direct_function(function, direct_symbol_scope.as_deref())?;
            if !state.is_direct_function_ready(function.function_id) {
                functions_to_define.push(batch_function);
            }
            predeclared.insert(function.function_id, declared);
        }

        let mut defined_functions = Vec::with_capacity(functions_to_define.len());
        let mut jit_plan_cache: HashMap<
            usize,
            (
                FactStore,
                PlannedJitModuleLocals,
                PlannedJitDeoptResumeModule,
            ),
        > = HashMap::new();
        for batch_function in functions_to_define {
            let function = &batch_function.function;
            let placeholder_blocks;
            let function_blocks = if function.function_id == root_function_id {
                blocks
            } else {
                placeholder_blocks =
                    vec![std::ptr::null_mut::<std::ffi::c_void>(); function.blocks.len()];
                placeholder_blocks.as_slice()
            };
            let owned_module_constant_ptrs;
            let owned_counter_slots_by_id;
            let (
                function_module,
                function_module_constants,
                function_counter_defs,
                function_module_constant_ptrs,
                function_counter_slots_by_id,
                function_scalar_counter_data_id,
                function_top_value_counter_data_id,
                function_direct_call_resolver,
                function_module_constant_binding_key,
                function_module_constant_symbol_prefix,
                function_symbol_scope,
            ) = if let Some(shared_state) = batch_function.source.shared_state() {
                owned_module_constant_ptrs = shared_state.module_constant_ptrs();
                let instance_key = shared_state.storage_instance_key();
                let scalar_counter_symbol =
                    scalar_counter_storage_symbol_for_shared_state(shared_state);
                let scalar_counter_base_ptr = shared_state.scalar_counter_values_ptr();
                let scalar_counter_data_id = if scalar_counter_base_ptr.is_null() {
                    None
                } else {
                    register_jit_data_symbol(
                        scalar_counter_symbol.as_str(),
                        scalar_counter_base_ptr.cast::<u8>(),
                    );
                    Some(declare_scalar_counter_storage_import(
                        &mut state.jit_module,
                        scalar_counter_symbol.as_str(),
                    )?)
                };
                let top_value_counter_symbol =
                    top_value_counter_storage_symbol_for_shared_state(shared_state);
                let top_value_counter_base_ptr = shared_state.top_value_counter_values_ptr();
                let top_value_counter_data_id = if top_value_counter_base_ptr.is_null() {
                    None
                } else {
                    register_jit_data_symbol(
                        top_value_counter_symbol.as_str(),
                        top_value_counter_base_ptr.cast::<u8>(),
                    );
                    Some(declare_top_value_counter_storage_import(
                        &mut state.jit_module,
                        top_value_counter_symbol.as_str(),
                    )?)
                };
                (
                    &shared_state.lowered_module,
                    &shared_state.codegen_constants,
                    shared_state.lowered_module.counter_defs.as_slice(),
                    owned_module_constant_ptrs.as_slice(),
                    shared_state.counter_slots_by_id(),
                    scalar_counter_data_id,
                    top_value_counter_data_id,
                    Some(shared_state),
                    instance_key,
                    module_constant_symbol_prefix_for_shared_state(shared_state),
                    Some(direct_function_symbol_scope_for_shared_state(
                        shared_state,
                        function.function_id,
                    )),
                )
            } else {
                let (counter_slots_by_id, scalar_counter_count, top_value_count) =
                    build_counter_storage_layout(counter_defs)?;
                let instance_key = module as *const BlockPyModule<CodegenModuleShape> as usize;
                let scalar_counter_data_id = state.ensure_local_scalar_counter_storage(
                    module,
                    scalar_counter_count,
                    instance_key,
                )?;
                let top_value_counter_data_id = state.ensure_local_top_value_counter_storage(
                    module,
                    top_value_count,
                    instance_key,
                )?;
                owned_counter_slots_by_id = counter_slots_by_id;
                (
                    module,
                    module_constants,
                    counter_defs,
                    module_constant_ptrs,
                    owned_counter_slots_by_id.as_ref(),
                    scalar_counter_data_id,
                    top_value_counter_data_id,
                    None,
                    instance_key,
                    module_constant_symbol_prefix_for_instance(module, instance_key),
                    None,
                )
            };
            let function_module_constant_object_data_ids = state.ensure_module_constant_objects(
                function_module_constant_ptrs,
                function_module_constant_binding_key,
                function_module_constant_symbol_prefix.as_str(),
            )?;
            if !jit_plan_cache.contains_key(&function_module_constant_binding_key) {
                let value_facts = infer_jit_value_facts(function_module);
                let jit_module_local_plan = plan_jit_module_locals(function_module, &value_facts)?;
                let jit_module_deopt_resume_plan =
                    plan_jit_deopt_resume_module(function_module, &value_facts)?;
                jit_plan_cache.insert(
                    function_module_constant_binding_key,
                    (
                        value_facts,
                        jit_module_local_plan,
                        jit_module_deopt_resume_plan,
                    ),
                );
            }
            let (
                function_value_facts,
                function_jit_module_local_plan,
                function_jit_module_deopt_resume_plan,
            ) = jit_plan_cache
                .get(&function_module_constant_binding_key)
                .expect("JIT plan cache entry should exist after insertion");
            let function_jit_local_plan = function_jit_module_local_plan
                .function(function.function_id)
                .ok_or_else(|| {
                    format!(
                        "missing JIT local plan for function {} ({})",
                        function.function_id, function.names.qualname
                    )
                })?;
            let function_jit_deopt_resume_plan = function_jit_module_deopt_resume_plan
                .function(function.function_id)
                .ok_or_else(|| {
                    format!(
                        "missing JIT deopt resume plan for function {} ({})",
                        function.function_id, function.names.qualname
                    )
                })?;
            let function_deopt_table = Arc::new(RuntimeJitDeoptTable::from_plan(
                function,
                function_jit_deopt_resume_plan,
                function_module_constant_ptrs,
            )?);
            let built = build_cranelift_run_bb_specialized_function(
                &mut state.jit_module,
                function_blocks,
                function_module,
                function,
                function_value_facts,
                function_jit_local_plan,
                function_jit_deopt_resume_plan,
                function_module_constants,
                function_counter_defs,
                function_module_constant_object_data_ids.as_slice(),
                function_counter_slots_by_id,
                function_scalar_counter_data_id,
                function_top_value_counter_data_id,
                session.as_ref(),
                function_direct_call_resolver,
                &SpecializationProfile::from_runtime_state(function_direct_call_resolver)?,
                function_symbol_scope.as_deref(),
                Some(&predeclared),
                BuildSpecializedFunctionOptions::default(),
            )
            .map_err(|err| {
                format!(
                    "{err} [function={} id={}]",
                    function.names.qualname, function.function_id
                )
            })?;
            let mut ctx = built.ctx;
            let main_id = built.main_id;
            let main_symbol = built.main_symbol;
            let default_adapter_id = built.default_adapter_id;
            let default_adapter_symbol = built.default_adapter_symbol;
            let clif_block_count = ctx.func.layout.blocks().count();
            let clif_inst_count = ctx.func.dfg.num_insts();
            let function_name =
                direct_function_backend_name(function, batch_function.source.shared_state());
            let artifact = define_prepared_function(
                &mut state.jit_module,
                main_id,
                &mut ctx,
                function_name.as_str(),
                "failed to define specialized jit run_bb function",
            )
            .map_err(|err| {
                format!(
                    "{err} [function={} id={}]",
                    function.names.qualname, function.function_id
                )
            })?;
            state.jit_module.clear_context(&mut ctx);
            let default_adapter_artifact = match (
                default_adapter_id,
                default_adapter_symbol.as_ref(),
            ) {
                (Some(default_adapter_id), Some(default_adapter_symbol)) => {
                    let mut default_ctx = build_default_resolving_direct_adapter(
                        &mut state.jit_module,
                        function,
                        main_id,
                        default_adapter_id,
                    )
                    .map_err(|err| {
                        format!(
                            "{err} [default-adapter function={} id={}]",
                            function.names.qualname, function.function_id
                        )
                    })?;
                    let artifact = define_prepared_function(
                        &mut state.jit_module,
                        default_adapter_id,
                        &mut default_ctx,
                        default_adapter_symbol.as_str(),
                        "failed to define default-resolving direct adapter",
                    )
                    .map_err(|err| {
                        format!(
                            "{err} [default-adapter function={} id={}]",
                            function.names.qualname, function.function_id
                        )
                    })?;
                    state.jit_module.clear_context(&mut default_ctx);
                    Some(artifact)
                }
                (None, None) => None,
                _ => {
                    return Err(format!(
                        "default direct adapter declaration is inconsistent for function {} id={}",
                        function.names.qualname, function.function_id
                    ));
                }
            };
            defined_functions.push(DefinedJitFunction {
                function_id: function.function_id,
                function_qualname: function.names.qualname.clone(),
                param_count: function.params.len(),
                main_id,
                main_symbol,
                default_adapter_id,
                default_adapter_symbol,
                stats: JitCodegenStats {
                    clif_block_count,
                    clif_inst_count,
                    machine_code_size_bytes: artifact.code_size,
                    machine_code_block_count: artifact.code_bb_offsets.len(),
                    machine_code_edge_count: artifact.code_bb_edges.len(),
                },
                artifact,
                default_adapter_artifact,
                deopt_table: function_deopt_table,
            });
        }

        state
            .jit_module
            .finalize_definitions()
            .map_err(|err| format!("failed to finalize specialized jit run_bb function: {err}"))?;
        let mut root_handle = None;
        let mut root_stats = None;
        for defined in defined_functions {
            let code_ptr = state.jit_module.get_finalized_function(defined.main_id);
            let default_code_ptr = defined
                .default_adapter_id
                .map(|default_adapter_id| {
                    state.jit_module.get_finalized_function(default_adapter_id)
                })
                .unwrap_or(code_ptr);
            let compiled_handle = state.mark_direct_function_ready(
                session,
                defined.function_id,
                code_ptr,
                default_code_ptr,
                defined.param_count,
                Arc::clone(&defined.deopt_table),
            )?;
            let code_id = jitdump::record_code_load(
                &defined.main_symbol,
                code_ptr.cast::<u8>(),
                defined.artifact.code_size,
                state.jit_module.isa(),
                defined.artifact.systemv_unwind_info.as_ref(),
            )?;
            record_jit_bb_map(
                &defined.main_symbol,
                code_id,
                &defined.artifact,
                defined.function_id,
                &defined.function_qualname,
                "direct_function_body",
            );
            if let (
                Some(default_adapter_id),
                Some(default_adapter_symbol),
                Some(default_adapter_artifact),
            ) = (
                defined.default_adapter_id,
                defined.default_adapter_symbol.as_ref(),
                defined.default_adapter_artifact.as_ref(),
            ) {
                let default_code_ptr = state.jit_module.get_finalized_function(default_adapter_id);
                let code_id = jitdump::record_code_load(
                    default_adapter_symbol,
                    default_code_ptr.cast::<u8>(),
                    default_adapter_artifact.code_size,
                    state.jit_module.isa(),
                    default_adapter_artifact.systemv_unwind_info.as_ref(),
                )?;
                record_jit_bb_map(
                    default_adapter_symbol,
                    code_id,
                    default_adapter_artifact,
                    defined.function_id,
                    &defined.function_qualname,
                    "default_direct_adapter",
                );
            }
            if defined.function_id == function.function_id {
                root_handle = Some(compiled_handle);
                root_stats = Some(defined.stats);
            }
        }
        let handle = root_handle.ok_or_else(|| {
            format!(
                "process JIT batch did not define root function {} id={}",
                function.names.qualname, function.function_id
            )
        })?;
        Ok(DirectFunctionCompileResult {
            handle,
            compiled: true,
            stats: root_stats,
        })
    }
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
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
) -> Result<DefinedFunctionArtifact, String> {
    let compiled =
        compile_prepared_function_bytes(jit_module, func_id, ctx, function_name, err_prefix)?;
    jit_module
        .define_function_bytes(
            func_id,
            compiled.bytes.alignment,
            compiled.bytes.code.as_slice(),
            compiled.bytes.relocs.as_slice(),
        )
        .map_err(|err| format!("{err_prefix}: {err}"))?;
    Ok(compiled.artifact)
}

fn compile_prepared_function_bytes(
    jit_module: &mut JITModule,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
) -> Result<CompiledFunctionArtifact, String> {
    let function_name = if jit_refcount_emission_enabled()? {
        Cow::Borrowed(function_name)
    } else {
        Cow::Owned(format!("{function_name}:refcounts=off"))
    };
    ctx.func.name = stable_cranelift_function_name(function_name.as_ref());
    prepare_cranelift_function_for_backend(jit_module, None, ctx, err_prefix)?;
    compile_backend_prepared_function_bytes(jit_module.isa(), func_id, ctx, err_prefix)
}

fn compile_prepared_function_bytes_with_isa(
    jit_module: &mut JITModule,
    isa: &dyn TargetIsa,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
) -> Result<CompiledFunctionArtifact, String> {
    let function_name = if jit_refcount_emission_enabled()? {
        Cow::Borrowed(function_name)
    } else {
        Cow::Owned(format!("{function_name}:refcounts=off"))
    };
    ctx.func.name = stable_cranelift_function_name(function_name.as_ref());
    prepare_cranelift_function_for_backend(jit_module, Some(isa), ctx, err_prefix)?;
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
    jit_module: &mut JITModule,
    isa: Option<&dyn TargetIsa>,
    ctx: &mut cranelift_codegen::Context,
    err_prefix: &str,
) -> Result<(), String> {
    inline_runtime_support_calls(jit_module, ctx, err_prefix)?;
    let isa = isa.unwrap_or_else(|| jit_module.isa());
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
    symbol: &str,
    code_id: u64,
    artifact: &DefinedFunctionArtifact,
    function_id: FunctionId,
    function_qualname: &str,
    entry_kind: &str,
) {
    let dir = match soac_work_dir_from_env() {
        Ok(Some(dir)) => dir,
        Ok(None) => return,
        Err(err) => {
            eprintln!("[soac jitdump] invalid SOAC_WORK_DIR: {err}");
            return;
        }
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

const RUNTIME_SUPPORT_INLINE_MAX_INSTS: usize = 128;
const SOAC_RUNTIME_EXAMPLE_SYMBOL_PREFIX: &str = "soac_runtime_example_";

#[derive(Debug)]
struct RuntimeSupportInliner {
    inlineable: HashMap<ir::UserExternalName, ir::Function>,
}

impl RuntimeSupportInliner {
    fn for_module(jit_module: &mut JITModule) -> Result<Self, String> {
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
                jit_module,
                &mut local_func_ids,
                &parsed.symbol,
                &parsed.function.signature,
                "inlineable runtime CLIF function",
            )?;
            let mut function = if should_inline_refcount_as_noop(parsed.symbol.as_str())? {
                build_noop_runtime_support_function(func_id, &parsed.function.signature)
            } else {
                parsed.function.clone()
            };
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
            if function.dfg.num_insts() > RUNTIME_SUPPORT_INLINE_MAX_INSTS {
                continue;
            }
            inlineable.insert(ir::UserExternalName::new(0, func_id.as_u32()), function);
        }
        Ok(Self { inlineable })
    }
}

fn should_inline_refcount_as_noop(symbol: &str) -> Result<bool, String> {
    Ok(!jit_refcount_emission_enabled()?
        && matches!(
            symbol,
            SOAC_RUNTIME_INCREF_SYMBOL | SOAC_RUNTIME_DECREF_SYMBOL
        ))
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
    jit_module: &mut JITModule,
    ctx: &mut cranelift_codegen::Context,
    err_prefix: &str,
) -> Result<bool, String> {
    let mut inliner = RuntimeSupportInliner::for_module(jit_module)?;
    ctx.inline(&mut inliner)
        .map_err(|err| format!("{err_prefix}: failed to inline runtime support calls: {err:?}"))
}

fn lower_static_signature(jit_module: &mut JITModule, signature: StaticSignature) -> ir::Signature {
    let mut lowered = jit_module.make_signature();
    let lower_sig_type = |sig_type| match sig_type {
        SigType::Pointer => jit_module.target_config().pointer_type(),
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
    jit_module: &mut JITModule,
    symbol: &str,
    sig: &ir::Signature,
) -> Result<FuncId, String> {
    jit_module
        .declare_function(symbol, Linkage::Import, sig)
        .map_err(|err| format!("failed to declare imported {symbol} symbol: {err}"))
}

fn declare_local_fn(
    jit_module: &mut JITModule,
    symbol: &str,
    sig: &ir::Signature,
) -> Result<FuncId, String> {
    jit_module
        .declare_function(symbol, Linkage::Local, sig)
        .map_err(|err| format!("failed to declare local {symbol} function: {err}"))
}

fn make_direct_function_signature(
    jit_module: &JITModule,
    function: &BlockPyFunction<CodegenModuleShape>,
) -> ir::Signature {
    let ptr_ty = jit_module.target_config().pointer_type();
    let mut sig = jit_module.make_signature();
    sig.params.push(ir::AbiParam::new(ptr_ty));
    sig.params.push(ir::AbiParam::new(ptr_ty));
    for _ in function.params.iter() {
        sig.params.push(ir::AbiParam::new(ptr_ty));
    }
    sig.returns.push(ir::AbiParam::new(ptr_ty));
    sig
}

fn direct_function_symbol(
    function: &BlockPyFunction<CodegenModuleShape>,
    symbol_scope: Option<&str>,
) -> String {
    let base =
        jit_python_perf_symbol_name(JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT, &function.names.qualname);
    scoped_jit_symbol(&base, symbol_scope)
}

fn default_direct_function_symbol(
    function: &BlockPyFunction<CodegenModuleShape>,
    symbol_scope: Option<&str>,
) -> String {
    let base = format!(
        "{}:defaults",
        jit_python_perf_symbol_name(JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT, &function.names.qualname)
    );
    scoped_jit_symbol(&base, symbol_scope)
}

fn direct_function_symbol_scope(function_id: FunctionId, symbol_id: u64) -> String {
    format!("fn_{}_{}", function_id.packed(), symbol_id)
}

fn direct_function_backend_name(
    function: &BlockPyFunction<CodegenModuleShape>,
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
            name.push_str(function.function_id.module_id().to_string().as_str());
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
    jit_module: &mut JITModule,
    function: &BlockPyFunction<CodegenModuleShape>,
    symbol_scope: Option<&str>,
) -> Result<(ir::Signature, DeclaredJitFunction), String> {
    let sig = make_direct_function_signature(jit_module, function);
    let symbol = direct_function_symbol(function, symbol_scope);
    let func_id = declare_local_fn(jit_module, &symbol, &sig)?;
    let (default_func_id, default_symbol) = if function_has_default_resolving_direct_entry(function)
    {
        let default_symbol = default_direct_function_symbol(function, symbol_scope);
        (
            Some(declare_local_fn(jit_module, &default_symbol, &sig)?),
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

fn build_default_resolving_direct_adapter(
    jit_module: &mut JITModule,
    function: &BlockPyFunction<CodegenModuleShape>,
    core_func_id: FuncId,
    adapter_func_id: FuncId,
) -> Result<cranelift_codegen::Context, String> {
    let ptr_ty = jit_module.target_config().pointer_type();
    let runtime_layout = FunctionRuntimeDataLayout::from_function(function);
    let mut module_imports = ModuleFuncImports::new();
    let mut ctx = jit_module.make_context();
    ctx.func.signature = make_direct_function_signature(jit_module, function);
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
            jit_module,
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
        let core_func_ref = jit_module.declare_func_in_func(core_func_id, &mut fb.func);
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
    for (symbol, clif_text) in SOAC_RUNTIME_CLIF {
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
    jit_module: &mut JITModule,
    local_func_ids: &mut HashMap<String, FuncId>,
    symbol: &str,
    signature: &ir::Signature,
    description: &str,
) -> Result<FuncId, String> {
    if let Some(func_id) = local_func_ids.get(symbol) {
        return Ok(*func_id);
    }
    let func_id = jit_module
        .declare_function(symbol, Linkage::Local, signature)
        .map_err(|err| format!("failed to declare {description} {symbol}: {err}"))?;
    local_func_ids.insert(symbol.to_string(), func_id);
    Ok(func_id)
}

fn remap_runtime_clif_extern_user_names(
    jit_module: &mut JITModule,
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
                jit_module,
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
                let import_id = jit_module
                    .declare_function(symbol, Linkage::Import, &sig)
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
            let import_id = jit_module
                .declare_data(symbol, Linkage::Import, false, false)
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

fn load_runtime_support_clif(jit_module: &mut JITModule) -> Result<(), String> {
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
        let mut ctx = jit_module.make_context();
        ctx.func = function;
        let _ = define_prepared_function(
            jit_module,
            func_id,
            &mut ctx,
            &parsed.symbol,
            &format!("failed to define runtime CLIF function {}", parsed.symbol),
        )?;
        jit_module.clear_context(&mut ctx);
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

fn compile_runtime_support_clif_for_object(
    jit_module: &mut JITModule,
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
        let mut ctx = jit_module.make_context();
        ctx.func = function;
        let compiled = compile_prepared_function_bytes_with_isa(
            jit_module,
            object_isa,
            func_id,
            &mut ctx,
            &parsed.symbol,
            &format!(
                "failed to compile runtime CLIF function {} to object",
                parsed.symbol
            ),
        )?;
        jit_module.clear_context(&mut ctx);
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
    output_path: &Path,
) -> Result<PrecompileObjectSummary, String> {
    let bytes = precompile_codegen_module_to_object_bytes(
        module_name,
        source_hash,
        module,
        counter_dump_path,
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
) -> Result<PrecompiledObjectBytes, String> {
    let compile_session = crate::session::CompileSession::new();
    let object_isa = CraneliftTargetConfig::object_from_env()?.build_isa()?;
    let builder = new_jit_builder()?;
    let mut jit_module = JITModule::new(builder);
    let mut function_definitions =
        compile_runtime_support_clif_for_object(&mut jit_module, object_isa.as_ref())?;

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

    let value_facts = infer_jit_value_facts(module);
    let jit_module_local_plan = plan_jit_module_locals(module, &value_facts)?;
    let jit_module_deopt_resume_plan = plan_jit_deopt_resume_module(module, &value_facts)?;
    let mut predeclared = HashMap::new();
    let mut symbol_scopes = HashMap::new();
    for function in &module.callable_defs {
        let symbol_scope = precompiled_direct_function_symbol_scope_for_module_identity(
            module_name,
            source_hash,
            function.function_id,
        );
        let (_sig, declared) =
            declare_direct_function(&mut jit_module, function, Some(symbol_scope.as_str()))?;
        predeclared.insert(function.function_id, declared);
        symbol_scopes.insert(function.function_id, symbol_scope);
    }
    let specialization_profile =
        SpecializationProfile::from_precompile(module_name, counter_dump_path)?;
    for function in &module.callable_defs {
        let placeholder_blocks =
            vec![std::ptr::null_mut::<std::ffi::c_void>(); function.blocks.len()];
        let jit_local_plan = jit_module_local_plan
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT local plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let jit_deopt_resume_plan = jit_module_deopt_resume_plan
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
            module,
            function,
            &value_facts,
            jit_local_plan,
            jit_deopt_resume_plan,
            &module_constants,
            module.counter_defs.as_slice(),
            module_constant_object_data_ids.as_slice(),
            counter_slots_by_id.as_ref(),
            scalar_counter_data_id,
            top_value_counter_data_id,
            &compile_session,
            None,
            &specialization_profile,
            symbol_scopes.get(&function.function_id).map(String::as_str),
            Some(&predeclared),
            BuildSpecializedFunctionOptions {
                module_constant_accesses: module_constant_access_table.clone(),
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
        jit_module.clear_context(&mut ctx);
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
                jit_module.clear_context(&mut default_ctx);
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
    let mut ctx = jit_module.make_context();
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
        function_id,
        &mut ctx,
        "jit-smoke",
        "failed to define Cranelift function",
    )?;
    jit_module.clear_context(&mut ctx);
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
    guard_miss_deopt_stub: bool,
    module_constant_accesses: ModuleConstantAccessTable,
}

fn build_cranelift_run_bb_specialized_function(
    jit_module: &mut JITModule,
    blocks: &[ObjPtr],
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
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
    specialization_profile: &SpecializationProfile<'_>,
    symbol_scope: Option<&str>,
    predeclared_direct_functions: Option<&HashMap<FunctionId, DeclaredJitFunction>>,
    options: BuildSpecializedFunctionOptions,
) -> Result<BuiltSpecializedFunction, String> {
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
            if let InstrCodegen::IncrementCounter(op) = expr {
                if scalar_counter_slot_for_id(counter_slots_by_id, op.counter_id).is_err() {
                    return Err(format!(
                        "specialized JIT scalar counter layout is missing counter id {} for function {}",
                        op.counter_id.0, function.names.qualname
                    ));
                }
            }
        }
    }
    jit_deopt_resume_plan.validate_for_function(function)?;
    let typed_function =
        lower_typed_function_if_tests_to_truthy(lower_codegen_function_to_typed(function.clone()));
    if typed_function.blocks.len() != function.blocks.len() {
        return Err(format!(
            "typed specialized JIT function block count mismatch: {} != {}",
            typed_function.blocks.len(),
            function.blocks.len()
        ));
    }
    let result_demand_plan = plan_typed_result_demands(&typed_function);

    let call_target_counter_ids =
        collect_runtime_counter_ids_by_kind(counter_defs, function.function_id, "call_hot_targets");
    let call_direct_hit_counter_ids =
        collect_runtime_counter_ids_by_kind(counter_defs, function.function_id, "call_direct_hit");
    let call_direct_fallback_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "call_direct_fallback",
    );
    let operator_shape_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "operator_hot_shapes",
    );
    let operator_specialized_hit_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "operator_specialized_hit",
    );
    let operator_specialized_fallback_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "operator_specialized_fallback",
    );
    let getitem_shape_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "getitem_hot_shapes",
    );
    let getitem_specialized_hit_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "getitem_specialized_hit",
    );
    let getitem_specialized_fallback_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "getitem_specialized_fallback",
    );
    let setitem_shape_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "setitem_hot_shapes",
    );
    let setitem_specialized_hit_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "setitem_specialized_hit",
    );
    let setitem_specialized_fallback_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "setitem_specialized_fallback",
    );
    let global_indexed_hit_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "global_indexed_hit",
    );
    let global_indexed_fallback_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "global_indexed_fallback",
    );
    let field_indexed_hit_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "field_indexed_hit",
    );
    let field_indexed_fallback_counter_ids = collect_runtime_counter_ids_by_kind(
        counter_defs,
        function.function_id,
        "field_indexed_fallback",
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
    let call_target_specializations =
        specialization_profile.call_target_specializations(function.function_id)?;
    let operator_specializations =
        specialization_profile.operator_specializations(function.function_id)?;
    let getitem_specializations =
        specialization_profile.getitem_specializations(function.function_id)?;
    let setitem_specializations =
        specialization_profile.setitem_specializations(function.function_id)?;
    let field_index_specializations = specialization_profile.field_index_specializations()?;
    let branch_prefer_true = specialization_profile.branch_preferences(function.function_id)?;
    let cold_block_labels = specialization_profile.cold_block_labels(function)?;
    let behavior_change_indexed_stores = specialization_profile.behavior_change_indexed_stores
        && function.scope.scope_kind != CallableScopeKind::Module;
    let guard_miss_deopt_stub = specialization_profile.guard_miss_deopt;
    let function_runtime_data_layout = FunctionRuntimeDataLayout::from_function(function);
    let true_constant_id = module_constants.require_runtime_name_constant_id("TRUE");
    let false_constant_id = module_constants.require_runtime_name_constant_id("FALSE");
    let none_constant_id = module_constants.require_runtime_name_constant_id("NONE");
    let empty_tuple_constant_id = module_constants.require_runtime_name_constant_id("EMPTY_TUPLE");

    let mut direct_call_targets = collect_call_direct_targets(function);
    for targets in call_target_specializations.values() {
        direct_call_targets.extend(targets.iter().copied());
    }
    let empty_direct_functions = HashMap::new();
    let direct_call_functions = predeclared_direct_functions.unwrap_or(&empty_direct_functions);
    let mut direct_call_target_functions = HashMap::new();
    for function_id in direct_call_targets {
        if module
            .callable_defs
            .iter()
            .any(|function| function.function_id == function_id)
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
        direct_call_target_functions.insert(function_id, target_function);
    }
    let ptr_ty = jit_module.target_config().pointer_type();
    let i64_ty = ir::types::I64;
    let mut module_imports = ModuleFuncImports::new();

    let (main_sig, main_id, main_symbol, default_adapter_id, default_adapter_symbol) =
        match predeclared_direct_functions
            .and_then(|functions| functions.get(&function.function_id))
        {
            Some(declared) => (
                make_direct_function_signature(jit_module, function),
                declared.func_id,
                declared.symbol.clone(),
                declared.default_func_id,
                declared.default_symbol.clone(),
            ),
            None => {
                let (sig, declared) = declare_direct_function(jit_module, function, symbol_scope)?;
                (
                    sig,
                    declared.func_id,
                    declared.symbol,
                    declared.default_func_id,
                    declared.default_symbol,
                )
            }
        };
    let counted_refcount_helpers = build_counted_runtime_refcount_helpers(
        jit_module,
        function,
        counter_defs,
        counter_slots_by_id,
        scalar_counter_data_id,
        symbol_scope,
    )?;

    let mut ctx = jit_module.make_context();
    ctx.func.signature = main_sig;
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut block_annotations = ClifBlockDisplayAnnotations::new();
    let direct_edge_stats = DirectEdgeStats::default();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = fb.create_block();
        let mut exec_blocks = Vec::with_capacity(block_count);
        let runtime_block_params = &jit_local_plan.runtime_block_params;
        let implicit_target_transports = &jit_local_plan.implicit_target_transports;
        let jump_edge_transports = &jit_local_plan.jump_edge_transports;
        let entry_materializations = &jit_local_plan.entry_materializations;
        let exc_dispatches = &jit_local_plan.exc_dispatches;
        let refcount_plan = &jit_local_plan.refcount_plan;
        let full_block_param_names = function
            .blocks
            .iter()
            .map(CodegenBlock::param_name_vec)
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
            if cold_block_labels.contains(&function.blocks[index].label) {
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
            jit_module.declare_func_in_func(incref_func_id, &mut fb.func)
        } else {
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_INCREF_IMPORT)
        };
        let decref_ref = if let Some(decref_func_id) = counted_refcount_helpers.decref_func_id {
            jit_module.declare_func_in_func(decref_func_id, &mut fb.func)
        } else {
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_DECREF_IMPORT)
        };
        let py_call_positional_three_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT,
        );
        let py_call_object_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_PY_CALL_OBJECT_IMPORT);
        let py_vectorcall_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_PY_VECTORCALL_IMPORT);
        let py_call_with_kw_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_PY_CALL_WITH_KW_IMPORT);
        let enter_recursive_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_ENTER_RECURSIVE_CALL_IMPORT,
        );
        let pytype_generic_alloc_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_PYTYPE_GENERIC_ALLOC_IMPORT,
        );
        let finish_constructor_init_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_FINISH_CONSTRUCTOR_INIT_IMPORT,
        );
        let load_global_fast_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &SOAC_RUNTIME_LOAD_GLOBAL_IMPORT);
        let probe_global_indexed_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_IMPORT,
        );
        let load_global_slow_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_LOAD_GLOBAL_SLOW_IMPORT,
        );
        let guard_miss_deopt_stub_ref = (options.guard_miss_deopt_stub || guard_miss_deopt_stub)
            .then(|| {
                func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_DEOPT_RESUME_IMPORT)
            });
        let store_global_indexed_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_STORE_GLOBAL_INDEXED_IMPORT,
        );
        let probe_field_indexed_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_PROBE_FIELD_INDEXED_IMPORT,
        );
        let store_field_indexed_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_STORE_FIELD_INDEXED_IMPORT,
        );
        let load_runtime_obj_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_LOAD_RUNTIME_OBJ_IMPORT);
        let is_true_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT);
        let raise_exc_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_RAISE_FROM_EXC_IMPORT);
        let push_handled_exception_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_PUSH_HANDLED_EXCEPTION_IMPORT,
        );
        let pop_handled_exception_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_POP_HANDLED_EXCEPTION_IMPORT,
        );
        let pyobject_getattr_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_PYOBJECT_GETATTR_IMPORT);
        let pyobject_setattr_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_PYOBJECT_SETATTR_IMPORT);
        let pyobject_getitem_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_PYOBJECT_GETITEM_IMPORT);
        let pyobject_setitem_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_PYOBJECT_SETITEM_IMPORT);
        let pyobject_to_i64_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_PYOBJECT_TO_I64_IMPORT);
        let py_long_from_i64_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &PYLONG_FROM_LONGLONG_IMPORT);
        let record_top_value_sample_ref = requires_top_value_counters.then(|| {
            func_imports.get_or_panic(
                jit_module,
                &mut fb.func,
                &DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT,
            )
        });
        let raise_deleted_name_error_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_RAISE_DELETED_NAME_ERROR_IMPORT,
        );
        let make_cell_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_MAKE_CELL_IMPORT);
        let load_cell_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_LOAD_CELL_IMPORT);
        let store_cell_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_STORE_CELL_IMPORT);
        let tuple_new_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &SOAC_RUNTIME_TUPLE_NEW_IMPORT);
        let tuple_set_item_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_IMPORT,
        );
        let set_raised_exception_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT,
        );
        let module_constant_object_globals = module_constant_object_data_ids
            .iter()
            .map(|data_id| jit_module.declare_data_in_func(*data_id, &mut fb.func))
            .collect::<Vec<_>>();
        let scalar_counter_base_value = scalar_counter_data_id.map(|data_id| {
            let counter_data = jit_module.declare_data_in_func(data_id, &mut fb.func);
            fb.ins().global_value(ptr_ty, counter_data)
        });
        let top_value_counter_base_value = top_value_counter_data_id.map(|data_id| {
            let counter_data = jit_module.declare_data_in_func(data_id, &mut fb.func);
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

        for (index, block) in exec_blocks.iter().enumerate() {
            fb.switch_to_block(*block);
            let codegen_block = &function.blocks[index];
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
                shared_state: direct_call_resolver,
                module_constants,
                value_facts,
                result_demand_plan: &result_demand_plan,
                deopt_resume_plan: jit_deopt_resume_plan,
                refcount_plan,
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
                guard_miss_resume_point: None,
                store_global_indexed_ref,
                probe_field_indexed_ref,
                store_field_indexed_ref,
                load_runtime_obj_ref,
                enter_recursive_ref,
                pyobject_getattr_ref,
                pyobject_setattr_ref,
                pyobject_getitem_ref,
                pyobject_setitem_ref,
                py_long_from_i64_ref,
                raise_deleted_name_error_ref,
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
                call_target_specializations: &call_target_specializations,
                call_direct_hit_counter_ids: &call_direct_hit_counter_ids,
                call_direct_fallback_counter_ids: &call_direct_fallback_counter_ids,
                operator_shape_counter_ids: &operator_shape_counter_ids,
                operator_specializations: &operator_specializations,
                operator_specialized_hit_counter_ids: &operator_specialized_hit_counter_ids,
                operator_specialized_fallback_counter_ids:
                    &operator_specialized_fallback_counter_ids,
                getitem_shape_counter_ids: &getitem_shape_counter_ids,
                getitem_specializations: &getitem_specializations,
                getitem_specialized_hit_counter_ids: &getitem_specialized_hit_counter_ids,
                getitem_specialized_fallback_counter_ids: &getitem_specialized_fallback_counter_ids,
                setitem_shape_counter_ids: &setitem_shape_counter_ids,
                setitem_specializations: &setitem_specializations,
                setitem_specialized_hit_counter_ids: &setitem_specialized_hit_counter_ids,
                setitem_specialized_fallback_counter_ids: &setitem_specialized_fallback_counter_ids,
                global_indexed_hit_counter_ids: &global_indexed_hit_counter_ids,
                global_indexed_fallback_counter_ids: &global_indexed_fallback_counter_ids,
                field_indexed_hit_counter_ids: &field_indexed_hit_counter_ids,
                field_indexed_fallback_counter_ids: &field_indexed_fallback_counter_ids,
                deopt_entry_guard_miss_counter_ids: &deopt_entry_guard_miss_counter_ids,
                branch_outcome_counter_ids: &branch_outcome_counter_ids,
                branch_prefer_true: &branch_prefer_true,
                field_index_specializations: &field_index_specializations,
                behavior_change_indexed_stores,
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
                    .deopt_points_for_block(codegen_block.label)
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
                jit_module,
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
                jump_edge_transports,
                implicit_target_transports,
                &mut local_env,
                term_emit_ctx,
                jit_module,
                &mut func_imports,
                is_true_ref,
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
    function: &soac_blockpy::block_py::BlockPyFunction<CodegenModuleShape>,
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

pub unsafe fn render_cranelift_run_bb_specialized_with_runtime_state_and_cfg(
    compile_session: &crate::session::CompileSession,
    blocks: &[ObjPtr],
    module: &BlockPyModule<CodegenModuleShape>,
    function: &soac_blockpy::block_py::BlockPyFunction<CodegenModuleShape>,
    module_constants: &ModuleCodegenConstants,
    runtime_state: Option<&SharedModuleState>,
) -> Result<RenderedSpecializedClif, String> {
    if blocks.is_empty() {
        return Err("specialized JIT run_bb requires at least one block".to_string());
    }

    let builder = new_jit_builder()?;
    let mut jit_module = JITModule::new(builder);
    let module_constant_ptrs = runtime_state
        .map(SharedModuleState::module_constant_ptrs)
        .unwrap_or_else(|| placeholder_module_constant_ptrs(module_constants.len()));
    let counter_defs = runtime_state
        .map(|state| state.lowered_module.counter_defs.as_slice())
        .unwrap_or(module.counter_defs.as_slice());
    let (counter_slots_by_id, scalar_counter_count, top_value_counter_count) =
        build_counter_storage_layout(counter_defs)?;
    let module_constant_object_data_ids =
        declare_module_constant_object_data(&mut jit_module, module, &module_constant_ptrs)?;
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
    } else if let Some(shared_state) = runtime_state {
        Some(declare_top_value_counter_storage_import(
            &mut jit_module,
            top_value_counter_storage_symbol_for_shared_state(shared_state).as_str(),
        )?)
    } else {
        Some(define_top_value_counter_storage_data(
            &mut jit_module,
            module,
            top_value_counter_count,
        )?)
    };
    let value_facts = infer_jit_value_facts(module);
    let jit_module_local_plan = plan_jit_module_locals(module, &value_facts)?;
    let jit_module_deopt_resume_plan = plan_jit_deopt_resume_module(module, &value_facts)?;
    let jit_local_plan = jit_module_local_plan
        .function(function.function_id)
        .ok_or_else(|| {
            format!(
                "missing JIT local plan for function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
    let jit_deopt_resume_plan = jit_module_deopt_resume_plan
        .function(function.function_id)
        .ok_or_else(|| {
            format!(
                "missing JIT deopt resume plan for function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
    let built = build_cranelift_run_bb_specialized_function(
        &mut jit_module,
        blocks,
        module,
        function,
        &value_facts,
        jit_local_plan,
        jit_deopt_resume_plan,
        module_constants,
        counter_defs,
        module_constant_object_data_ids.as_slice(),
        counter_slots_by_id.as_ref(),
        scalar_counter_data_id,
        top_value_counter_data_id,
        compile_session,
        runtime_state,
        &SpecializationProfile::from_runtime_state(runtime_state)?,
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
    let (compiled_clif, cfg_dot, vcode_disasm) = render_compiled_clif_and_vcode_disasm(
        &mut jit_module,
        built.ctx,
        &built.import_id_to_symbol,
        &built.block_annotations,
    )?;
    out.push_str(&compiled_clif);
    Ok(RenderedSpecializedClif {
        clif: out,
        cfg_dot,
        vcode_disasm,
    })
}

fn render_compiled_clif_and_vcode_disasm(
    jit_module: &mut JITModule,
    mut ctx: cranelift_codegen::Context,
    import_id_to_symbol: &HashMap<u32, &'static str>,
    block_annotations: &ClifBlockDisplayAnnotations,
) -> Result<(String, String, String), String> {
    prepare_cranelift_function_for_backend(
        jit_module,
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
        .isa()
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
    function: &soac_blockpy::block_py::BlockPyFunction<CodegenModuleShape>,
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

fn compiled_direct_runner_info(
    compiled_handle: ObjPtr,
) -> Result<(*const u8, *const u8, usize), String> {
    if compiled_handle.is_null() {
        return Err("invalid null compiled handle for direct vectorcall trampoline".to_string());
    }
    let compiled = unsafe { &*(compiled_handle as *const CompiledSpecializedRunner) };
    debug_assert!(
        compiled.direct_deopt_table.is_some(),
        "compiled direct handle should carry a deopt table"
    );
    match compiled.entry {
        Some(CompiledRunnerEntry::Direct {
            code_ptr,
            default_code_ptr,
            param_count,
        }) => Ok((code_ptr, default_code_ptr, param_count)),
        None => Err("invalid compiled handle without entrypoint".to_string()),
    }
}

pub(crate) fn compiled_direct_code_ptr(compiled_handle: ObjPtr) -> Result<ObjPtr, String> {
    compiled_direct_runner_info(compiled_handle).map(|(code_ptr, _, _)| code_ptr as ObjPtr)
}

pub(crate) fn compiled_default_direct_code_ptr(compiled_handle: ObjPtr) -> Result<ObjPtr, String> {
    compiled_direct_runner_info(compiled_handle)
        .map(|(_, default_code_ptr, _)| default_code_ptr as ObjPtr)
}

pub(crate) fn compiled_direct_deopt_table_ptr(compiled_handle: ObjPtr) -> Result<ObjPtr, String> {
    if compiled_handle.is_null() {
        return Err("invalid null compiled handle for direct deopt table pointer".to_string());
    }
    let compiled = unsafe { &*(compiled_handle as *const CompiledSpecializedRunner) };
    compiled
        .direct_deopt_table
        .as_ref()
        .map(|table| Arc::as_ptr(table) as ObjPtr)
        .ok_or_else(|| "compiled direct handle does not carry a deopt table".to_string())
}

#[cfg(test)]
fn compiled_direct_deopt_table(
    compiled_handle: ObjPtr,
) -> Result<Arc<RuntimeJitDeoptTable>, String> {
    if compiled_handle.is_null() {
        return Err("invalid null compiled handle for direct deopt table".to_string());
    }
    let compiled = unsafe { &*(compiled_handle as *const CompiledSpecializedRunner) };
    compiled
        .direct_deopt_table
        .as_ref()
        .cloned()
        .ok_or_else(|| "compiled direct handle does not carry a deopt table".to_string())
}

fn define_shared_vectorcall_trampoline(
    jit_module: &mut JITModule,
    param_count: usize,
    symbol_name: &str,
) -> Result<VectorcallEntryFn, String> {
    let ptr_ty = jit_module.target_config().pointer_type();
    let i64_ty = ir::types::I64;
    let mut module_imports = ModuleFuncImports::new();

    let mut main_sig = jit_module.make_signature();
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.returns.push(ir::AbiParam::new(ptr_ty));

    let main_id = declare_local_fn(jit_module, symbol_name, &main_sig)?;

    let mut direct_sig = jit_module.make_signature();
    direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    for _ in 0..param_count {
        direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    }
    direct_sig.returns.push(ir::AbiParam::new(ptr_ty));

    let mut ctx = jit_module.make_context();
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
        main_id,
        &mut ctx,
        &format!("direct-vectorcall-trampoline:{param_count}"),
        "failed to define direct vectorcall trampoline",
    )?;
    jit_module.clear_context(&mut ctx);
    jit_module
        .finalize_definitions()
        .map_err(|err| format!("failed to finalize direct vectorcall trampoline: {err}"))?;

    let code_ptr = jit_module.get_finalized_function(main_id);
    jitdump::record_code_load(
        symbol_name,
        code_ptr.cast::<u8>(),
        main_artifact.code_size,
        jit_module.isa(),
        main_artifact.systemv_unwind_info.as_ref(),
    )?;
    let entry: VectorcallEntryFn = unsafe { std::mem::transmute(code_ptr) };
    Ok(entry)
}

pub unsafe fn free_cranelift_run_bb_specialized_cached(compiled_handle: ObjPtr) {
    if compiled_handle.is_null() {
        return;
    }
    let _ = Box::from_raw(compiled_handle as *mut CompiledSpecializedRunner);
}

#[cfg(test)]
mod test;
