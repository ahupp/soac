use crate::SOAC_RUNTIME_CLIF;
use crate::counter_dump::{
    CollectedTypeKeyLayout, CounterDumpFile, CounterDumpTypeKey, collect_type_key_layouts,
    collect_type_table, read_branch_preferences_from_file,
    read_call_target_specializations_from_file, read_operator_specializations_from_file,
};
use crate::module_constants::{ModuleCodegenConstants, ModuleConstantId};
use crate::module_type::SharedModuleState;
use cranelift_codegen::cfg_printer::CFGPrinter;
use cranelift_codegen::inline::{Inline, InlineCommand};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_control::ControlPlane;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use cranelift_module::{DataDescription, FuncId, Linkage, Module, ModuleReloc};
use cranelift_reader::parse_functions;
use pyo3::ffi;
use soac_blockpy::block_py::{
    AbruptKind, BlockArg, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, CallArgKeyword,
    CallArgPositional, CallableScopeKind, CellLocation, ChildVisitable, CodegenBlock, CounterDef,
    CounterId, CounterScope, CounterSite, FunctionId, HasMeta, HasSemanticInstrId, InstrCodegen,
    InstrId, InstrKey, Literal, LocalLocation, NameLocation, ParamKind, ResolvedName,
    StorageLayout, Visit, WithMeta, operation as blockpy_intrinsics,
};
use soac_blockpy::passes::{
    CodegenModuleShape, FactStore, FunctionRefcountPlan, InstrResolved, PyExactType, PyObjFacts,
    RefcountActionKind, RefcountReleaseReason, RuntimeHelperId, ValueFacts,
    infer_module_value_facts,
};
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::ffi::CString;
use std::mem::offset_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use tracing::info;

unsafe extern "C" {
    static mut PyFunction_Type: ffi::PyTypeObject;
    static mut PyMethod_Type: ffi::PyTypeObject;
    static mut PyType_Type: ffi::PyTypeObject;
    static mut PyLong_Type: ffi::PyTypeObject;
    static mut _PyDict_IndexedValueTombstone: i8;
    fn PyUnstable_Type_AssignVersionTag(type_obj: *mut ffi::PyTypeObject) -> i32;
    fn _PyType_LookupRef(
        type_obj: *mut ffi::PyTypeObject,
        name: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
}

mod intrinsics;
mod jitdump;
mod planning;
mod runtime_context;
mod specialized_helpers;
mod typed_value;

pub use planning::{
    BlockExcDispatchPlan, BlockLocalPlan, CurrentJitRefcountPlanCheck, FunctionLocalPlan,
    LocalRefKind, PlannedLocalBinding, check_refcount_plan_against_current_jit, exc_dispatch_plan,
    jit_param_names_for_block, plan_function_locals, plan_function_refcount_ownership,
};
use runtime_context::{
    FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET, FUNCTION_ENV_GLOBALS_OBJ_OFFSET,
    FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET, PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
    PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET,
};
pub use runtime_context::{ModuleJitContext, ModuleRuntimeContext};
pub use specialized_helpers::ObjPtr;
use specialized_helpers::register_specialized_jit_symbols;
pub use typed_value::{IntFacts, IntRange, IntWidth, SoacRepr, SoacValue};

static RUNTIME_SUPPORT_LIBRARY: OnceLock<Result<RuntimeSupportLibrary, String>> = OnceLock::new();
static NEXT_IMPORT_SPEC_ID: AtomicUsize = AtomicUsize::new(0);
static NEXT_IMPORT_TRAMPOLINE_ID: AtomicUsize = AtomicUsize::new(0);
const JIT_ARENA_BYTES: usize = 256 * 1024 * 1024;
const MISSING_PYTHON_EXCEPTION_TRAP: ir::TrapCode = ir::TrapCode::unwrap_user(1);

thread_local! {
    static PROCESS_JIT_COMPILE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

unsafe extern "C" {
    fn _Py_Dealloc(obj: *mut ffi::PyObject);
}

fn py_dealloc_symbol() -> *const u8 {
    _Py_Dealloc as *const u8
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
static SOAC_RUNTIME_LOAD_GLOBAL_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_LOAD_GLOBAL_INDEXED_SYMBOL,
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
static SOAC_RUNTIME_LOAD_FIELD_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_LOAD_FIELD_INDEXED_SYMBOL,
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
static DP_JIT_LEAVE_RECURSIVE_CALL_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_leave_recursive_call", &[SigType::Pointer], &[]);
static DP_JIT_PY_THREAD_STATE_GET_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_py_thread_state_get", &[], &[SigType::Pointer]);
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
static DP_JIT_DIRECT_FUNCTION_CONTEXT_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_direct_function_context",
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
static SOAC_RUNTIME_GUARD_TYPE_VERSION_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_GUARD_TYPE_VERSION_SYMBOL,
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::I32],
);
static DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_record_top_value_sample",
    &[SigType::Pointer, SigType::I64],
    &[],
);
static DP_JIT_RAISE_DELETED_NAME_ERROR_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_raise_deleted_name_error", &[SigType::Pointer], &[]);
static DP_JIT_MAKE_CELL_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_make_cell", &[SigType::Pointer], &[SigType::Pointer]);
static DP_JIT_LOAD_CELL_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_load_cell", &[SigType::Pointer], &[SigType::Pointer]);
static DP_JIT_STORE_CELL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_store_cell",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_TUPLE_NEW_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_tuple_new", &[SigType::I64], &[SigType::Pointer]);
static DP_JIT_TUPLE_SET_ITEM_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_tuple_set_item",
    &[SigType::Pointer, SigType::I64, SigType::Pointer],
    &[SigType::I32],
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
static DP_JIT_VECTORCALL_FUNCTION_EXTRA_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_vectorcall_function_extra",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_VECTORCALL_FUNCTION_ENV_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_vectorcall_function_env",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
struct ModuleFuncImports {
    func_ids_by_internal_id: Vec<Option<FuncId>>,
    import_id_to_symbol: HashMap<u32, &'static str>,
}

impl ModuleFuncImports {
    fn new() -> Self {
        Self {
            func_ids_by_internal_id: Vec::new(),
            import_id_to_symbol: HashMap::new(),
        }
    }

    fn debug_symbols(&self) -> &HashMap<u32, &'static str> {
        &self.import_id_to_symbol
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
            Linkage::Import => define_import_trampoline_fn(jit_module, spec.symbol, &sig)?,
            Linkage::Local => declare_local_fn(jit_module, spec.symbol, &sig)?,
            linkage => {
                return Err(format!(
                    "unsupported linkage {linkage:?} for jit call spec {}",
                    spec.symbol
                ));
            }
        };
        self.func_ids_by_internal_id[internal_id] = Some(func_id);
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
    import_id_to_symbol: HashMap<u32, &'static str>,
    block_annotations: ClifBlockDisplayAnnotations,
}

#[derive(Clone)]
struct DeclaredJitFunction {
    func_id: FuncId,
    symbol: String,
}

struct DefinedJitFunction {
    function_id: FunctionId,
    function_qualname: String,
    param_count: usize,
    main_id: FuncId,
    main_symbol: String,
    artifact: DefinedFunctionArtifact,
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
    next_direct_symbol_id: u64,
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
            next_direct_symbol_id: 0,
        })
    }

    fn declare_direct_function(
        &mut self,
        function: &BlockPyFunction<CodegenModuleShape>,
    ) -> Result<DeclaredJitFunction, String> {
        let shape = ProcessJitFunctionShape::for_function(function);
        if let Some(entry) = self.direct_functions.get(&function.function_id) {
            if entry.shape() == &shape {
                return Ok(entry.declared());
            }
        }
        let symbol_scope =
            direct_function_symbol_scope(function.function_id, self.next_direct_symbol_id);
        self.next_direct_symbol_id = self.next_direct_symbol_id.wrapping_add(1);
        let (_sig, declared) =
            declare_direct_function(&mut self.jit_module, function, Some(symbol_scope.as_str()))?;
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
        param_count: usize,
    ) -> Result<Arc<CompiledFunctionHandle>, String> {
        let Some(entry) = self.direct_functions.get(&function_id) else {
            return Err(format!(
                "process JIT function {function_id} was defined before declaration"
            ));
        };
        let declared = entry.declared();
        let shape = entry.shape().clone();
        let compiled_handle = Arc::new(CompiledFunctionHandle::from_direct_entry(
            session,
            code_ptr,
            param_count,
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
}

pub(crate) struct CompiledFunctionHandle {
    handle: ObjPtr,
}

pub(crate) struct DirectFunctionCompileResult {
    pub(crate) handle: Arc<CompiledFunctionHandle>,
    pub(crate) compiled: bool,
}

// The handle points to an immutable compiled runner after construction. The code memory is kept
// alive by the runner owner, and the raw handle is freed only when the final Arc drops this wrapper.
unsafe impl Send for CompiledFunctionHandle {}
unsafe impl Sync for CompiledFunctionHandle {}

impl CompiledFunctionHandle {
    fn from_direct_entry(
        session: &Arc<crate::session::CompileSession>,
        code_ptr: *const u8,
        param_count: usize,
    ) -> Self {
        Self {
            handle: new_compiled_direct_runner_handle(session, code_ptr, param_count),
        }
    }

    #[cfg(test)]
    pub(crate) fn direct_runner_info(&self) -> Result<(*const u8, usize), String> {
        compiled_direct_runner_info(self.handle)
    }

    pub(crate) fn direct_code_ptr(&self) -> Result<ObjPtr, String> {
        compiled_direct_code_ptr(self.handle)
    }
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
        param_count: usize,
    },
}

fn new_compiled_direct_runner_handle(
    session: &Arc<crate::session::CompileSession>,
    code_ptr: *const u8,
    param_count: usize,
) -> ObjPtr {
    Box::into_raw(Box::new(CompiledSpecializedRunner {
        _session: Arc::clone(session),
        entry: Some(CompiledRunnerEntry::Direct {
            code_ptr,
            param_count,
        }),
    })) as ObjPtr
}

fn codegen_expr_is_borrowable(
    expr: &InstrCodegen,
    local_names: &[String],
    stack_slots: &StackSlots,
    storage_layout: Option<&StorageLayout>,
) -> bool {
    match expr {
        InstrCodegen::Load(op) => op
            .name
            .local_location()
            .and_then(|location| storage_layout?.stack_slots().get(location.slot() as usize))
            .is_some_and(|name| {
                local_names.iter().any(|candidate| candidate == name) || stack_slots.has_name(name)
            }),
        _ => false,
    }
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

fn emit_codegen_local_name_load(
    fb: &mut FunctionBuilder,
    location: LocalLocation,
    local_names: &[String],
    local_values: &[ir::Value],
    ctx: &JitEmitCtx<'_>,
    borrowed: bool,
) -> ir::Value {
    let layout = ctx
        .storage_layout
        .as_ref()
        .expect("Load local slot should have storage layout during codegen");
    let name = local_name_for_location(layout, location);
    if let Some(slot_index) = local_names.iter().position(|candidate| candidate == name) {
        let slot_value = local_values[slot_index];
        if !borrowed {
            fb.ins().call(ctx.incref_ref, &[slot_value]);
        }
        return slot_value;
    }
    if let Some(slot_value) =
        emit_checked_stack_slot_value(fb, &ctx.stack_slots, name, ctx, borrowed)
    {
        return slot_value;
    }
    panic!("missing local {name} in direct JIT state");
}

fn emit_codegen_non_local_name_load(
    fb: &mut FunctionBuilder<'_>,
    name: &ResolvedName,
    load_instr_id: InstrId,
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
            let value = if ctx
                .global_indexed_hit_counter_ids
                .contains_key(&load_instr_id)
            {
                emit_codegen_indexed_global_load(
                    fb,
                    globals_obj,
                    name_obj,
                    slot_index,
                    load_instr_id,
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

fn emit_codegen_located_name_load(
    fb: &mut FunctionBuilder<'_>,
    name: &ResolvedName,
    load_instr_id: InstrId,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    borrowed: bool,
) -> ir::Value {
    match name.location {
        NameLocation::Local(location) => {
            emit_codegen_local_name_load(fb, location, local_names, local_values, ctx, borrowed)
        }
        NameLocation::Cell(location)
            if location.is_owned() || location.is_closure() || location.is_captured_source() =>
        {
            assert!(
                !borrowed,
                "cell-backed name loads must produce owned references"
            );
            let cell_obj = emit_raw_cell_object_for_name(fb, name, local_names, local_values, ctx);
            emit_cell_value_load_from_raw_cell(fb, cell_obj, ctx)
        }
        NameLocation::Constant(_)
        | NameLocation::GlobalName
        | NameLocation::Global(_)
        | NameLocation::RuntimeName => {
            emit_codegen_non_local_name_load(fb, name, load_instr_id, ctx, borrowed)
                .expect("non-local load helper should handle non-local name locations")
        }
        NameLocation::Cell(_) => {
            unreachable!("all cell location cases should be handled above");
        }
    }
}

fn emit_optional_counter_increment_for_kind(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    counters: &HashMap<InstrId, CounterId>,
    instr_id: InstrId,
) {
    if let Some(counter_id) = counters.get(&instr_id).copied() {
        let counter_ptr = ctx.counter_ptrs[counter_id.0];
        emit_increment_counter_ptr(fb, ctx.consts.ptr_ty, counter_ptr);
    }
}

fn emit_codegen_indexed_global_load(
    fb: &mut FunctionBuilder<'_>,
    globals_obj: ir::Value,
    name_obj: ir::Value,
    slot_index: ir::Value,
    instr_id: InstrId,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let result_block = fb.create_block();
    fb.append_block_param(result_block, ptr_ty);
    let fallback_block = fb.create_block();
    let direct_block = fb.create_block();
    fb.append_block_param(direct_block, ptr_ty);

    let direct_inst = fb.ins().call(
        ctx.load_global_indexed_ref,
        &[globals_obj, name_obj, slot_index],
    );
    let direct_value = fb.inst_results(direct_inst)[0];
    let direct_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, direct_value, null_ptr);
    fb.ins().brif(
        direct_is_null,
        fallback_block,
        &[],
        direct_block,
        &[ir::BlockArg::Value(direct_value)],
    );

    fb.switch_to_block(direct_block);
    let direct_value = fb.block_params(direct_block)[0];
    emit_optional_counter_increment_for_kind(fb, ctx, ctx.global_indexed_hit_counter_ids, instr_id);
    fb.ins()
        .call(ctx.decref_ref, &[ctx.consts.thread_state_value, name_obj]);
    fb.ins()
        .jump(result_block, &[ir::BlockArg::Value(direct_value)]);

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

fn super_instance_arg_without_deleted_guard<'a>(
    expr: &'a InstrCodegen,
    module_constants: &'a ModuleCodegenConstants,
) -> &'a InstrCodegen {
    let InstrCodegen::Call(call) = expr else {
        return expr;
    };
    if !call.keywords.is_empty() || call.args.len() != 2 {
        return expr;
    }
    if codegen_expr_helper_name(call.func.as_ref(), module_constants) != Some("load_deleted_name") {
        return expr;
    }
    let CallArgPositional::Positional(value) = &call.args[1] else {
        return expr;
    };
    value
}

fn emit_codegen_super_helper_call(
    fb: &mut FunctionBuilder<'_>,
    callable_expr: &InstrCodegen,
    super_fn_expr: &InstrCodegen,
    cls_expr: &InstrCodegen,
    instance_expr: &InstrCodegen,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let callable_is_borrowed = codegen_expr_is_borrowable(
        callable_expr,
        local_names,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    );
    let callable = emit_codegen_expr(
        fb,
        callable_expr,
        local_names,
        local_values,
        ctx,
        callable_is_borrowed,
        jit_module,
        func_imports,
    );

    let super_fn_is_borrowed = codegen_expr_is_borrowable(
        super_fn_expr,
        local_names,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    );
    let super_fn = emit_codegen_expr(
        fb,
        super_fn_expr,
        local_names,
        local_values,
        ctx,
        super_fn_is_borrowed,
        jit_module,
        func_imports,
    );

    let cls_is_borrowed = codegen_expr_is_borrowable(
        cls_expr,
        local_names,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    );
    let cls = emit_codegen_expr(
        fb,
        cls_expr,
        local_names,
        local_values,
        ctx,
        cls_is_borrowed,
        jit_module,
        func_imports,
    );

    let instance_expr =
        super_instance_arg_without_deleted_guard(instance_expr, ctx.module_constants);
    let instance_is_borrowed = codegen_expr_is_borrowable(
        instance_expr,
        local_names,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    );
    let instance = emit_codegen_expr(
        fb,
        instance_expr,
        local_names,
        local_values,
        ctx,
        instance_is_borrowed,
        jit_module,
        func_imports,
    );

    let call_inst = fb.ins().call(
        ctx.py_call_positional_three_ref,
        &[
            ctx.consts.thread_state_value,
            callable,
            super_fn,
            cls,
            instance,
            null_ptr,
        ],
    );
    if !instance_is_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, instance]);
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
    let callable_is_borrowed = codegen_expr_is_borrowable_from_local_env(
        callable_expr,
        local_env,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    );
    let callable = emit_codegen_expr_with_local_env(
        fb,
        callable_expr,
        local_env,
        ctx,
        callable_is_borrowed,
        jit_module,
        func_imports,
    );

    let super_fn_is_borrowed = codegen_expr_is_borrowable_from_local_env(
        super_fn_expr,
        local_env,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    );
    let super_fn = emit_codegen_expr_with_local_env(
        fb,
        super_fn_expr,
        local_env,
        ctx,
        super_fn_is_borrowed,
        jit_module,
        func_imports,
    );

    let cls_is_borrowed = codegen_expr_is_borrowable_from_local_env(
        cls_expr,
        local_env,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    );
    let cls = emit_codegen_expr_with_local_env(
        fb,
        cls_expr,
        local_env,
        ctx,
        cls_is_borrowed,
        jit_module,
        func_imports,
    );

    let instance_expr =
        super_instance_arg_without_deleted_guard(instance_expr, ctx.module_constants);
    let instance_is_borrowed = codegen_expr_is_borrowable_from_local_env(
        instance_expr,
        local_env,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    );
    let instance = emit_codegen_expr_with_local_env(
        fb,
        instance_expr,
        local_env,
        ctx,
        instance_is_borrowed,
        jit_module,
        func_imports,
    );

    let call_inst = fb.ins().call(
        ctx.py_call_positional_three_ref,
        &[
            ctx.consts.thread_state_value,
            callable,
            super_fn,
            cls,
            instance,
            null_ptr,
        ],
    );
    if !instance_is_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, instance]);
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

fn emit_direct_function_env_load_or_slow_path(
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
    let slow_block = fb.create_block();
    let load_env_block = fb.create_block();
    let env_ok_block = fb.create_block();
    let done_block = fb.create_block();
    fb.append_block_param(done_block, ptr_ty);

    fb.ins()
        .brif(metadata_is_null, slow_block, &[], load_env_block, &[]);

    fb.switch_to_block(load_env_block);
    let env = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        metadata,
        PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
    );
    let env_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, env, null_ptr);
    fb.ins()
        .brif(env_is_null, slow_block, &[], env_ok_block, &[]);

    fb.switch_to_block(env_ok_block);
    fb.ins().jump(done_block, &[ir::BlockArg::Value(env)]);

    fb.switch_to_block(slow_block);
    let slow_inst = fb.ins().call(ctx.direct_function_context_ref, &[callable]);
    let slow_env = fb.inst_results(slow_inst)[0];
    let slow_env_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, slow_env, null_ptr);
    let slow_env_ok_block = fb.create_block();
    fb.ins().brif(
        slow_env_is_null,
        done_block,
        &[ir::BlockArg::Value(null_ptr)],
        slow_env_ok_block,
        &[],
    );

    fb.switch_to_block(slow_env_ok_block);
    fb.ins().jump(done_block, &[ir::BlockArg::Value(slow_env)]);

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
    function_data_value: ir::Value,
    thread_state_value: ir::Value,
    none_const: ir::Value,
    true_const: ir::Value,
    false_const: ir::Value,
    deleted_const: ir::Value,
    empty_tuple_const: ir::Value,
    block_const: ir::Value,
    py_function_type_ptr: *mut ffi::PyTypeObject,
    py_method_type_ptr: *mut ffi::PyTypeObject,
    py_type_type_ptr: *mut ffi::PyTypeObject,
    py_long_type_ptr: *mut ffi::PyTypeObject,
}

#[derive(Clone)]
struct JitEmitCtx<'mc> {
    module: &'mc BlockPyModule<CodegenModuleShape>,
    function_id: FunctionId,
    shared_state: Option<&'mc crate::module_type::SharedModuleState>,
    module_constants: &'mc ModuleCodegenConstants,
    module_constant_ptrs: &'mc [*mut ffi::PyObject],
    value_facts: &'mc FactStore,
    refcount_plan: &'mc FunctionRefcountPlan,
    counter_ptrs: &'mc [*mut u64],
    top_value_counter_ptrs: &'mc [ObjPtr],
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
    load_global_indexed_ref: ir::FuncRef,
    load_global_slow_ref: ir::FuncRef,
    store_global_indexed_ref: ir::FuncRef,
    load_field_indexed_ref: ir::FuncRef,
    store_field_indexed_ref: ir::FuncRef,
    load_runtime_obj_ref: ir::FuncRef,
    direct_function_context_ref: ir::FuncRef,
    enter_recursive_ref: ir::FuncRef,
    leave_recursive_ref: ir::FuncRef,
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
    guard_method_type_version_ref: ir::FuncRef,
    record_top_value_sample_ref: ir::FuncRef,
    tuple_new_ref: ir::FuncRef,
    tuple_set_item_ref: ir::FuncRef,
    set_raised_exception_ref: ir::FuncRef,
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
    branch_outcome_counter_ids: &'mc HashMap<InstrId, CounterId>,
    branch_prefer_true: &'mc HashMap<InstrId, bool>,
    global_indexed_hit_counter_ids: &'mc HashMap<InstrId, CounterId>,
    global_indexed_fallback_counter_ids: &'mc HashMap<InstrId, CounterId>,
    field_indexed_hit_counter_ids: &'mc HashMap<InstrId, CounterId>,
    field_indexed_fallback_counter_ids: &'mc HashMap<InstrId, CounterId>,
    field_index_specializations: &'mc HashMap<String, Vec<FieldIndexSpecialization>>,
    behavior_change_indexed_stores: bool,
}

impl JitEmitCtx<'_> {
    fn value_facts_for_expr(&self, expr: &InstrCodegen) -> Option<ValueFacts> {
        let instr_id = expr.try_semantic_instr_id()?;
        self.value_facts
            .fact_for(InstrKey::new(self.function_id, instr_id))
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
    descriptor_function: ObjPtr,
    owner_type: *mut ffi::PyTypeObject,
    type_version: u32,
    arg_plan: DirectCallArgPlan,
}

#[derive(Clone)]
struct DirectConstructorSpecialization {
    function_id: FunctionId,
    init_function: ObjPtr,
    owner_type: *mut ffi::PyTypeObject,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectCallArgSource {
    Provided(usize),
    DefaultSentinel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectCallIncompatibility {
    MissingPredeclared,
    StarredArguments,
    Keywords,
    UnsupportedParameterKind { kind: ParamKind },
    MissingRequiredArgument,
    TooManyPositionalArguments { provided: usize, accepted: usize },
}

#[derive(Default)]
struct DirectEdgeStats {
    clif_direct_edges: Cell<usize>,
    call_direct_missing_target_fallbacks: Cell<usize>,
    call_direct_unsupported_shape_fallbacks: Cell<usize>,
    call_direct_missing_predeclared_fallbacks: Cell<usize>,
    guarded_generic_fallback_blocks: Cell<usize>,
    profiled_missing_target_candidates: Cell<usize>,
    profiled_arity_mismatch_candidates: Cell<usize>,
    profiled_unsupported_shape_candidates: Cell<usize>,
    profiled_missing_predeclared_candidates: Cell<usize>,
}

impl DirectEdgeStats {
    fn increment(cell: &Cell<usize>) {
        cell.set(cell.get() + 1);
    }

    fn record_resolved_direct_edge(&self) {
        Self::increment(&self.clif_direct_edges);
    }

    fn record_call_direct_missing_target_fallback(&self) {
        Self::increment(&self.call_direct_missing_target_fallbacks);
    }

    fn record_call_direct_unsupported_shape_fallback(&self) {
        Self::increment(&self.call_direct_unsupported_shape_fallbacks);
    }

    fn record_call_direct_missing_predeclared_fallback(&self) {
        Self::increment(&self.call_direct_missing_predeclared_fallbacks);
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

    fn record_profiled_missing_predeclared_candidate(&self) {
        Self::increment(&self.profiled_missing_predeclared_candidates);
    }

    fn total(&self) -> usize {
        self.clif_direct_edges.get()
            + self.call_direct_missing_target_fallbacks.get()
            + self.call_direct_unsupported_shape_fallbacks.get()
            + self.call_direct_missing_predeclared_fallbacks.get()
            + self.guarded_generic_fallback_blocks.get()
            + self.profiled_missing_target_candidates.get()
            + self.profiled_arity_mismatch_candidates.get()
            + self.profiled_unsupported_shape_candidates.get()
            + self.profiled_missing_predeclared_candidates.get()
    }

    fn emit_trace(&self, module_name: &str, function: &BlockPyFunction<CodegenModuleShape>) {
        if self.total() == 0 {
            return;
        }
        let clif_direct_edges = self.clif_direct_edges.get();
        let function_env_indirect_edges = 0usize;
        let call_direct_missing_target_fallbacks = self.call_direct_missing_target_fallbacks.get();
        let call_direct_unsupported_shape_fallbacks =
            self.call_direct_unsupported_shape_fallbacks.get();
        let call_direct_missing_predeclared_fallbacks =
            self.call_direct_missing_predeclared_fallbacks.get();
        let guarded_generic_fallback_blocks = self.guarded_generic_fallback_blocks.get();
        let profiled_missing_target_candidates = self.profiled_missing_target_candidates.get();
        let profiled_arity_mismatch_candidates = self.profiled_arity_mismatch_candidates.get();
        let profiled_unsupported_shape_candidates =
            self.profiled_unsupported_shape_candidates.get();
        let profiled_missing_predeclared_candidates =
            self.profiled_missing_predeclared_candidates.get();
        let generic_fallback_edges = call_direct_missing_target_fallbacks
            + call_direct_unsupported_shape_fallbacks
            + call_direct_missing_predeclared_fallbacks
            + guarded_generic_fallback_blocks
            + profiled_missing_target_candidates
            + profiled_arity_mismatch_candidates
            + profiled_unsupported_shape_candidates
            + profiled_missing_predeclared_candidates;
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
            call_direct_missing_predeclared_fallbacks,
            guarded_generic_fallback_blocks,
            profiled_missing_target_candidates,
            profiled_arity_mismatch_candidates,
            profiled_unsupported_shape_candidates,
            profiled_missing_predeclared_candidates,
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

fn validate_direct_call_compatibility(
    target_function: &BlockPyFunction<CodegenModuleShape>,
    direct_call_functions: &HashMap<FunctionId, DeclaredJitFunction>,
    explicit_positional_arg_count: usize,
    implicit_positional_arg_count: usize,
    has_starred_arguments: bool,
    has_keywords: bool,
) -> Result<DirectCallArgPlan, DirectCallIncompatibility> {
    let arg_plan = plan_direct_call_args_for_target(
        target_function,
        explicit_positional_arg_count,
        implicit_positional_arg_count,
        has_starred_arguments,
        has_keywords,
    )?;
    if !direct_call_functions.contains_key(&target_function.function_id) {
        return Err(DirectCallIncompatibility::MissingPredeclared);
    }
    Ok(arg_plan)
}

fn record_profiled_direct_call_incompatibility(
    stats: &DirectEdgeStats,
    incompatibility: DirectCallIncompatibility,
) {
    match incompatibility {
        DirectCallIncompatibility::MissingPredeclared => {
            stats.record_profiled_missing_predeclared_candidate();
        }
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

#[derive(Clone, Copy)]
struct FieldIndexSpecialization {
    expected_index: u32,
    owner_type: *mut ffi::PyTypeObject,
    type_version: u32,
}

struct CodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd> {
    fb: &'a mut FunctionBuilder<'b>,
    local_names: &'c mut Vec<String>,
    local_values: &'c mut Vec<ir::Value>,
    ctx: &'c JitEmitCtx<'mc>,
    jit_module: &'a mut JITModule,
    func_imports: &'a mut FuncBuildImports<'d>,
}

struct LocalEnvCodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd> {
    fb: &'a mut FunctionBuilder<'b>,
    local_env: &'c mut LocalEnv,
    ctx: &'c JitEmitCtx<'mc>,
    jit_module: &'a mut JITModule,
    func_imports: &'a mut FuncBuildImports<'d>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum LocalEnvKey {
    Location(LocalLocation),
    Name(String),
}

impl LocalEnvKey {
    fn legacy_name(name: &str) -> Self {
        Self::Name(name.to_string())
    }
}

#[derive(Clone)]
struct LocalEnvEntry {
    key: LocalEnvKey,
    name: String,
    value: ir::Value,
    ref_kind: LocalRefKind,
    storage: LocalEnvStorage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalEnvStorage {
    LocalOnly,
    StackMirror,
}

#[derive(Default)]
struct LocalEnv {
    entries: Vec<LocalEnvEntry>,
}

struct LocalEnvLegacyParts {
    names: Vec<String>,
    values: Vec<ir::Value>,
}

impl LocalEnv {
    fn bind_entry_location(
        &mut self,
        location: LocalLocation,
        name: &str,
        value: ir::Value,
        ref_kind: LocalRefKind,
        storage: LocalEnvStorage,
    ) {
        debug_assert!(
            self.entry_index_for_location(location).is_none(),
            "block-entry LocalEnv location should be bound once"
        );
        self.entries.push(LocalEnvEntry {
            key: LocalEnvKey::Location(location),
            name: name.to_string(),
            value,
            ref_kind,
            storage,
        });
    }

    fn entry_index_for_location(&self, location: LocalLocation) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.key == LocalEnvKey::Location(location))
    }

    fn entry_index_for_name(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.name == name)
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
            let value = self.entries[index].value;
            if !borrowed {
                fb.ins().call(ctx.incref_ref, &[value]);
            }
            return Some(value);
        }
        emit_checked_stack_slot_value(fb, &ctx.stack_slots, name, ctx, borrowed)
    }

    fn load_name(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        ctx: &JitEmitCtx<'_>,
        borrowed: bool,
    ) -> Option<ir::Value> {
        if let Some(index) = self.entry_index_for_name(name) {
            let value = self.entries[index].value;
            if !borrowed {
                fb.ins().call(ctx.incref_ref, &[value]);
            }
            return Some(value);
        }
        emit_checked_stack_slot_value(fb, &ctx.stack_slots, name, ctx, borrowed)
    }

    fn store_location(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        location: LocalLocation,
        name: &str,
        value: ir::Value,
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
        if stack_slots.has_name(name) {
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
        } else {
            self.entries.push(LocalEnvEntry {
                key: LocalEnvKey::Location(location),
                name: name.to_string(),
                value,
                ref_kind: LocalRefKind::Owned,
                storage: LocalEnvStorage::LocalOnly,
            });
        }
        if let Some(previous) = previous_entry {
            if transient_local_needs_decref(previous.ref_kind) {
                fb.ins()
                    .call(decref_ref, &[thread_state_value, previous.value]);
            }
        }
    }

    fn store_name(
        &mut self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        stack_slots: &StackSlots,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
    ) {
        let previous_entry = self
            .entry_index_for_name(name)
            .map(|existing_index| self.entries.remove(existing_index));
        if stack_slots.has_name(name) {
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
        } else {
            self.entries.push(LocalEnvEntry {
                key: LocalEnvKey::legacy_name(name),
                name: name.to_string(),
                value,
                ref_kind: LocalRefKind::Owned,
                storage: LocalEnvStorage::LocalOnly,
            });
        }
        if let Some(previous) = previous_entry {
            if transient_local_needs_decref(previous.ref_kind) {
                fb.ins()
                    .call(decref_ref, &[thread_state_value, previous.value]);
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
        if let Some(index) = self
            .entry_index_for_location(location)
            .or_else(|| self.entry_index_for_name(name))
        {
            let previous = self.entries.remove(index);
            if transient_local_needs_decref(previous.ref_kind) {
                fb.ins()
                    .call(decref_ref, &[thread_state_value, previous.value]);
            }
        } else if !stack_slots.has_name(name) {
            return Err(format!("missing local binding for delete target: {name}"));
        }
        if stack_slots.has_name(name) {
            stack_slots
                .clear_value(fb, name, ptr_ty, thread_state_value, decref_ref)
                .expect("slot-backed delete target missing from stack slots");
        }
        Ok(())
    }

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

    fn unchanged_entry_for_legacy_part(
        &self,
        name: &str,
        value: ir::Value,
    ) -> Option<&LocalEnvEntry> {
        self.entries
            .iter()
            .find(|entry| entry.name == name && entry.value == value)
    }

    fn to_legacy_parts(&self) -> LocalEnvLegacyParts {
        LocalEnvLegacyParts {
            names: self
                .entries
                .iter()
                .map(|entry| entry.name.clone())
                .collect(),
            values: self.entries.iter().map(|entry| entry.value).collect(),
        }
    }

    fn replace_from_legacy_parts(&mut self, parts: LocalEnvLegacyParts) {
        debug_assert_eq!(
            parts.names.len(),
            parts.values.len(),
            "JIT transient local names and values must stay parallel"
        );
        let LocalEnvLegacyParts { names, values } = parts;
        let entries = names
            .into_iter()
            .zip(values)
            .map(|(name, value)| {
                let existing_entry = self.entries.iter().find(|entry| entry.name == name);
                let unchanged_entry = self.unchanged_entry_for_legacy_part(name.as_str(), value);
                let ref_kind = unchanged_entry
                    .map(|entry| entry.ref_kind)
                    .unwrap_or(LocalRefKind::Owned);
                let storage = unchanged_entry
                    .map(|entry| entry.storage)
                    .unwrap_or(LocalEnvStorage::LocalOnly);
                LocalEnvEntry {
                    key: existing_entry
                        .map(|entry| entry.key.clone())
                        .unwrap_or_else(|| LocalEnvKey::legacy_name(name.as_str())),
                    name,
                    value,
                    ref_kind,
                    storage,
                }
            })
            .collect();
        self.entries = entries;
    }

    #[cfg(test)]
    fn with_legacy_parts_mut<R>(
        &mut self,
        emit: impl FnOnce(&mut Vec<String>, &mut Vec<ir::Value>) -> R,
    ) -> R {
        let mut local_parts = self.to_legacy_parts();
        let result = emit(&mut local_parts.names, &mut local_parts.values);
        self.replace_from_legacy_parts(local_parts);
        result
    }
}

fn planned_entry_binding_for_block_arg_name<'a>(
    block_plan: Option<&'a BlockLocalPlan>,
    name: &str,
) -> Option<&'a PlannedLocalBinding> {
    let entry_locals = &block_plan?.entry_locals;
    entry_locals
        .iter()
        .find(|binding| binding.name == name)
        .or_else(|| {
            if !is_try_exception_alias_name(name) {
                return None;
            }
            entry_locals
                .iter()
                .find(|binding| is_try_exception_alias_name(binding.name.as_str()))
        })
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

fn local_ref_kind_for_stack_mirror(ref_kind: LocalRefKind) -> LocalRefKind {
    match ref_kind {
        LocalRefKind::Immortal => LocalRefKind::Immortal,
        LocalRefKind::Unbound => LocalRefKind::Unbound,
        LocalRefKind::Owned | LocalRefKind::Borrowed | LocalRefKind::Unknown => {
            LocalRefKind::Borrowed
        }
    }
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

fn emit_call_if_not_null(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    func_ref: ir::FuncRef,
    value: ir::Value,
) {
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
    let call_block = fb.create_block();
    let done_block = fb.create_block();
    fb.ins()
        .brif(value_is_null, done_block, &[], call_block, &[]);

    fb.switch_to_block(call_block);
    fb.ins().call(func_ref, &[value]);
    fb.ins().jump(done_block, &[]);

    fb.switch_to_block(done_block);
}

fn emit_incref_if_not_null(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    incref_ref: ir::FuncRef,
    value: ir::Value,
) {
    emit_call_if_not_null(fb, ptr_ty, incref_ref, value);
}

fn emit_decref_if_not_null(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    decref_ref: ir::FuncRef,
    thread_state_value: ir::Value,
    value: ir::Value,
) {
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
    let call_block = fb.create_block();
    let done_block = fb.create_block();
    fb.ins()
        .brif(value_is_null, done_block, &[], call_block, &[]);

    fb.switch_to_block(call_block);
    fb.ins().call(decref_ref, &[thread_state_value, value]);
    fb.ins().jump(done_block, &[]);

    fb.switch_to_block(done_block);
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

fn bind_local_value(
    fb: &mut FunctionBuilder<'_>,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    name: &str,
    value: ir::Value,
    stack_slots: &StackSlots,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
) {
    if let Some(existing_index) = local_names.iter().position(|candidate| candidate == name) {
        let previous = local_values.remove(existing_index);
        local_names.remove(existing_index);
        fb.ins().call(decref_ref, &[thread_state_value, previous]);
    }
    if stack_slots.has_name(name) {
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
    } else {
        local_names.push(name.to_string());
        local_values.push(value);
    }
}

fn delete_local_value(
    fb: &mut FunctionBuilder<'_>,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    name: &str,
    stack_slots: &StackSlots,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    decref_ref: ir::FuncRef,
) -> Result<(), String> {
    if let Some(index) = local_names.iter().position(|candidate| candidate == name) {
        let previous = local_values.remove(index);
        local_names.remove(index);
        fb.ins().call(decref_ref, &[thread_state_value, previous]);
    } else if !stack_slots.has_name(name) {
        return Err(format!("missing local binding for delete target: {name}"));
    }
    if stack_slots.has_name(name) {
        stack_slots
            .clear_value(fb, name, ptr_ty, thread_state_value, decref_ref)
            .expect("slot-backed delete target missing from stack slots");
    }
    Ok(())
}

impl<'a, 'b, 'mc, 'c, 'd> intrinsics::OperationEmitState<'b, InstrCodegen>
    for CodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd>
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
            let borrowed_arg = codegen_expr_is_borrowable(
                arg,
                &*self.local_names,
                &self.ctx.stack_slots,
                self.ctx.storage_layout.as_ref(),
            );
            let value = emit_codegen_expr(
                self.fb,
                arg,
                &mut *self.local_names,
                &mut *self.local_values,
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

    fn py_facts_for_arg(&self, arg: &InstrCodegen) -> PyObjFacts {
        self.ctx
            .value_facts_for_expr(arg)
            .and_then(ValueFacts::as_pyobj)
            .unwrap_or_else(PyObjFacts::unknown)
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
            let borrowed_arg = codegen_expr_is_borrowable_from_local_env(
                arg,
                &*self.local_env,
                &self.ctx.stack_slots,
                self.ctx.storage_layout.as_ref(),
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

    fn py_facts_for_arg(&self, arg: &InstrCodegen) -> PyObjFacts {
        self.ctx
            .value_facts_for_expr(arg)
            .and_then(ValueFacts::as_pyobj)
            .unwrap_or_else(PyObjFacts::unknown)
    }
}

fn load_stack_slot_value(
    fb: &mut FunctionBuilder<'_>,
    stack_slots: &StackSlots,
    name: &str,
    ptr_ty: ir::Type,
    borrowed: bool,
    incref_ref: ir::FuncRef,
) -> Option<ir::Value> {
    let slot = stack_slots.slot_for_block_arg_name(name)?;
    let value = fb.ins().stack_load(ptr_ty, slot, 0);
    if !borrowed {
        emit_incref_if_not_null(fb, ptr_ty, incref_ref, value);
    }
    Some(value)
}

fn emit_checked_stack_slot_value(
    fb: &mut FunctionBuilder<'_>,
    stack_slots: &StackSlots,
    name: &str,
    ctx: &JitEmitCtx<'_>,
    borrowed: bool,
) -> Option<ir::Value> {
    let slot = stack_slots.slot_for_block_arg_name(name)?;
    let value = fb.ins().stack_load(ctx.consts.ptr_ty, slot, 0);
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
        return Some(value);
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
    let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
    fb.ins()
        .call(ctx.decref_ref, &[ctx.consts.thread_state_value, name_obj]);
    emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(value_ok_block);
    let value = fb.block_params(value_ok_block)[0];
    if !borrowed {
        fb.ins().call(ctx.incref_ref, &[value]);
    }
    Some(value)
}

fn is_try_exception_alias_name(name: &str) -> bool {
    name.starts_with("_dp_try_exc_")
}

fn is_try_abrupt_kind_name(name: &str) -> bool {
    name.starts_with("_dp_try_abrupt_kind_")
}

fn block_arg_values(values: &[ir::Value]) -> Vec<ir::BlockArg> {
    values.iter().copied().map(ir::BlockArg::Value).collect()
}

struct PendingLocalFailureCleanup {
    block: ir::Block,
    cleanup_null_block: ir::Block,
}

fn step_null_block_args(ctx: &JitEmitCtx<'_>) -> Vec<ir::BlockArg> {
    block_arg_values(&ctx.consts.step_null_args)
}

fn emit_take_error_before_local_null_cleanup(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    emit_take_current_raised_exception_or_trap(fb, ctx.consts.ptr_ty, ctx.consts.thread_state_value)
}

fn emit_restore_error_after_local_null_cleanup(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    error_value: ir::Value,
) {
    fb.ins().call(
        ctx.set_raised_exception_ref,
        &[ctx.consts.thread_state_value, error_value],
    );
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
    let null_ptr = fb.ins().iconst(ctx.consts.ptr_ty, 0);
    let result_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, result, null_ptr);
    let null_block = fb.create_block();
    let ok_block = fb.create_block();
    let done_block = fb.create_block();
    fb.append_block_param(done_block, ctx.consts.ptr_ty);
    fb.ins()
        .brif(result_is_null, null_block, &[], ok_block, &[]);

    fb.switch_to_block(null_block);
    let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
    for owned_input in owned_inputs {
        fb.ins().call(
            ctx.decref_ref,
            &[ctx.consts.thread_state_value, *owned_input],
        );
    }
    emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
    fb.ins().jump(done_block, &[ir::BlockArg::Value(result)]);

    fb.switch_to_block(ok_block);
    for owned_input in owned_inputs {
        fb.ins().call(
            ctx.decref_ref,
            &[ctx.consts.thread_state_value, *owned_input],
        );
    }
    fb.ins().jump(done_block, &[ir::BlockArg::Value(result)]);

    fb.switch_to_block(done_block);
    fb.block_params(done_block)[0]
}

fn emit_checked_owned_pyobject_call_with_cleanup(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    func_ref: ir::FuncRef,
    args: &[ir::Value],
    owned_inputs: &[ir::Value],
) -> ir::Value {
    let call_inst = fb.ins().call(func_ref, args);
    let value = emit_decref_owned_inputs_after_nullable_result(
        fb,
        ctx,
        fb.inst_results(call_inst)[0],
        owned_inputs,
    );
    emit_checked_owned_pyobject_result(fb, value, ctx)
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
    module_constant_ptrs: &[*mut ffi::PyObject],
    ptr_ty: ir::Type,
) -> ir::Value {
    let constant_ptr = module_constant_ptrs
        .get(constant_id.0)
        .copied()
        .unwrap_or_else(|| {
            panic!(
                "missing module constant pointer for constant id {}",
                constant_id.0
            )
        });
    fb.ins().iconst(ptr_ty, constant_ptr as i64)
}

fn emit_owned_module_constant(
    fb: &mut FunctionBuilder<'_>,
    constant_id: ModuleConstantId,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    emit_owned_module_constant_from_parts(
        fb,
        constant_id,
        ctx.module_constant_ptrs,
        ctx.consts.ptr_ty,
    )
}

fn placeholder_module_constant_ptrs(count: usize) -> Vec<*mut ffi::PyObject> {
    (0..count)
        .map(|index| (0x1000usize + index * 0x10) as *mut ffi::PyObject)
        .collect()
}

fn placeholder_counter_ptrs(count: usize) -> Vec<*mut u64> {
    (0..count)
        .map(|index| (0x2000usize + index * 0x10) as *mut u64)
        .collect()
}

fn placeholder_top_value_counter_ptrs(count: usize) -> Vec<ObjPtr> {
    vec![std::ptr::null_mut(); count]
}

fn emit_increment_counter(
    fb: &mut FunctionBuilder<'_>,
    counter_id: CounterId,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let counter_ptr = ctx
        .counter_ptrs
        .get(counter_id.0)
        .copied()
        .unwrap_or_else(|| panic!("missing counter pointer for counter id {}", counter_id.0));
    let counter_addr = fb.ins().iconst(ctx.consts.ptr_ty, counter_ptr as i64);
    let old_value = fb
        .ins()
        .load(ir::types::I64, ir::MemFlags::trusted(), counter_addr, 0);
    let new_value = fb.ins().iadd_imm(old_value, 1);
    fb.ins()
        .store(ir::MemFlags::trusted(), new_value, counter_addr, 0);
    // TODO: Split codegen instructions into value-producing vs non-value-producing ops
    // and elide retain/release work when a statement result is not consumed.
    fb.ins().call(ctx.incref_ref, &[ctx.consts.none_const]);
    ctx.consts.none_const
}

pub(super) fn emit_increment_counter_ptr(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    counter_ptr: *mut u64,
) {
    let counter_addr = fb.ins().iconst(ptr_ty, counter_ptr as i64);
    let old_value = fb
        .ins()
        .load(ir::types::I64, ir::MemFlags::trusted(), counter_addr, 0);
    let new_value = fb.ins().iadd_imm(old_value, 1);
    fb.ins()
        .store(ir::MemFlags::trusted(), new_value, counter_addr, 0);
}

#[derive(Clone, Copy, Debug)]
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

fn counter_ptr_for_id(
    counter_ptrs: &[*mut u64],
    counter_id: CounterId,
) -> Result<*mut u64, String> {
    counter_ptrs
        .get(counter_id.0)
        .copied()
        .ok_or_else(|| format!("missing counter pointer for counter id {}", counter_id.0))
}

fn build_counted_runtime_refcount_helper(
    compile_session: &crate::session::CompileSession,
    jit_module: &mut JITModule,
    symbol_name: &str,
    cache_name: &str,
    runtime_import: &'static ImportSpec,
    counter_ptr: *mut u64,
) -> Result<FuncId, String> {
    let ptr_ty = jit_module.target_config().pointer_type();
    let mut sig = jit_module.make_signature();
    for _ in runtime_import.signature.params {
        sig.params.push(ir::AbiParam::new(ptr_ty));
    }
    let helper_id = declare_local_fn(jit_module, symbol_name, &sig)?;

    let mut ctx = jit_module.make_context();
    ctx.func.signature = sig;
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = fb.create_block();
        fb.append_block_params_for_function_params(entry_block);
        fb.switch_to_block(entry_block);
        let args = fb.block_params(entry_block).to_vec();
        let counter_addr = fb.ins().iconst(ptr_ty, counter_ptr as i64);
        let old_value = fb
            .ins()
            .load(ir::types::I64, ir::MemFlags::trusted(), counter_addr, 0);
        let new_value = fb.ins().iadd_imm(old_value, 1);
        fb.ins()
            .store(ir::MemFlags::trusted(), new_value, counter_addr, 0);

        let mut module_imports = ModuleFuncImports::new();
        let mut func_imports = FuncBuildImports::new(&mut module_imports);
        let runtime_ref = func_imports.get_or_panic(jit_module, &mut fb.func, runtime_import);
        fb.ins().call(runtime_ref, &args);
        fb.ins().return_(&[]);
        fb.seal_all_blocks();
        fb.finalize();
    }

    let _ = define_function_with_incremental_cache(
        compile_session,
        jit_module,
        helper_id,
        &mut ctx,
        cache_name,
        CraneliftCompileCachePolicy::Disabled {
            reason: "counted refcount helper embeds a per-run counter pointer",
        },
        "failed to define counted runtime refcount helper",
    )?;
    jit_module.clear_context(&mut ctx);
    Ok(helper_id)
}

fn build_counted_runtime_refcount_helpers(
    compile_session: &crate::session::CompileSession,
    jit_module: &mut JITModule,
    function: &BlockPyFunction<CodegenModuleShape>,
    counter_defs: &[CounterDef],
    counter_ptrs: &[*mut u64],
    symbol_scope: Option<&str>,
) -> Result<CountedRefcountHelpers, String> {
    let incref_func_id =
        lookup_runtime_counter_id(counter_defs, function.function_id, "runtime_incref")
            .map(|counter_id| {
                let counter_ptr = counter_ptr_for_id(counter_ptrs, counter_id)?;
                let cache_name = format!("py:rc:incref:{}", function.names.qualname);
                let symbol = scoped_jit_symbol(&cache_name, symbol_scope);
                build_counted_runtime_refcount_helper(
                    compile_session,
                    jit_module,
                    &symbol,
                    &cache_name,
                    &DP_JIT_INCREF_IMPORT,
                    counter_ptr,
                )
            })
            .transpose()?;

    let decref_func_id =
        lookup_runtime_counter_id(counter_defs, function.function_id, "runtime_decref")
            .map(|counter_id| {
                let counter_ptr = counter_ptr_for_id(counter_ptrs, counter_id)?;
                let cache_name = format!("py:rc:decref:{}", function.names.qualname);
                let symbol = scoped_jit_symbol(&cache_name, symbol_scope);
                build_counted_runtime_refcount_helper(
                    compile_session,
                    jit_module,
                    &symbol,
                    &cache_name,
                    &DP_JIT_DECREF_IMPORT,
                    counter_ptr,
                )
            })
            .transpose()?;

    Ok(CountedRefcountHelpers {
        incref_func_id,
        decref_func_id,
    })
}

fn emit_raw_cell_object_for_name(
    fb: &mut FunctionBuilder<'_>,
    name: &ResolvedName,
    local_names: &[String],
    local_values: &[ir::Value],
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let Some(location) = name.cell_location() else {
        panic!(
            "raw cell access should target a cell-backed name, got {} at {:?}",
            name.id, name.location
        );
    };
    emit_raw_cell_object_for_location(
        fb,
        location,
        name.id.as_str(),
        local_names,
        local_values,
        ctx,
    )
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

fn emit_raw_cell_object_for_location(
    fb: &mut FunctionBuilder<'_>,
    location: CellLocation,
    debug_name: &str,
    local_names: &[String],
    local_values: &[ir::Value],
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    match location {
        CellLocation::Owned(slot) => {
            let ptr_ty = ctx.consts.ptr_ty;
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
                if let Some(slot_index) = local_names
                    .iter()
                    .position(|candidate| candidate == *candidate_name)
                {
                    let slot_value = local_values[slot_index];
                    fb.ins().call(ctx.incref_ref, &[slot_value]);
                    return slot_value;
                }
                if let Some(slot_value) = load_stack_slot_value(
                    fb,
                    &ctx.stack_slots,
                    candidate_name,
                    ptr_ty,
                    false,
                    ctx.incref_ref,
                ) {
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

fn emit_function_data_slot_owned_or_null(
    fb: &mut FunctionBuilder<'_>,
    function_data: ir::Value,
    slot: usize,
    ptr_ty: ir::Type,
    incref_ref: ir::FuncRef,
) -> ir::Value {
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let borrowed = emit_function_data_slot_borrowed(fb, function_data, slot, ptr_ty);
    let is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, borrowed, null_ptr);
    let null_block = fb.create_block();
    let owned_block = fb.create_block();
    let done_block = fb.create_block();
    fb.append_block_param(done_block, ptr_ty);
    fb.ins().brif(is_null, null_block, &[], owned_block, &[]);

    fb.switch_to_block(null_block);
    fb.ins().jump(done_block, &[ir::BlockArg::Value(null_ptr)]);

    fb.switch_to_block(owned_block);
    fb.ins().call(incref_ref, &[borrowed]);
    fb.ins().jump(done_block, &[ir::BlockArg::Value(borrowed)]);

    fb.switch_to_block(done_block);
    fb.block_params(done_block)[0]
}

fn emit_pack_current_values_tuple(
    fb: &mut FunctionBuilder<'_>,
    values: &[ir::Value],
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    if values.is_empty() {
        fb.ins()
            .call(ctx.incref_ref, &[ctx.consts.empty_tuple_const]);
        return ctx.consts.empty_tuple_const;
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
    let set_fail_block = fb.create_block();
    fb.append_block_param(set_fail_block, ptr_ty);
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
    let set_inst = fb
        .ins()
        .call(ctx.tuple_set_item_ref, &[body_tuple, body_index, value]);
    let set_result = fb.inst_results(set_inst)[0];
    let set_failed = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::NotEqual, set_result, 0);
    let next_index = fb.ins().iadd_imm(body_index, 1);
    fb.ins().brif(
        set_failed,
        set_fail_block,
        &[ir::BlockArg::Value(body_tuple)],
        loop_block,
        &[
            ir::BlockArg::Value(next_index),
            ir::BlockArg::Value(body_tuple),
        ],
    );

    fb.switch_to_block(set_fail_block);
    let failed_tuple = fb.block_params(set_fail_block)[0];
    let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
    fb.ins().call(
        ctx.decref_ref,
        &[ctx.consts.thread_state_value, failed_tuple],
    );
    emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(done_block);
    fb.block_params(done_block)[0]
}

fn emit_call_args_tuple_from_values(
    fb: &mut FunctionBuilder<'_>,
    arg_values: &[(ir::Value, bool)],
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
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
        let set_inst = fb.ins().call(
            ctx.tuple_set_item_ref,
            &[call_args_tuple, item_index, *value],
        );
        let set_result = fb.inst_results(set_inst)[0];
        let set_failed = fb
            .ins()
            .icmp_imm(ir::condcodes::IntCC::NotEqual, set_result, 0);
        let set_ok_block = fb.create_block();
        let set_fail_block = fb.create_block();
        fb.append_block_param(set_fail_block, ptr_ty);
        fb.ins().brif(
            set_failed,
            set_fail_block,
            &[ir::BlockArg::Value(call_args_tuple)],
            set_ok_block,
            &[],
        );
        fb.switch_to_block(set_fail_block);
        let failed_tuple = fb.block_params(set_fail_block)[0];
        let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
        fb.ins().call(
            ctx.decref_ref,
            &[ctx.consts.thread_state_value, failed_tuple],
        );
        emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
        fb.ins()
            .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));
        fb.switch_to_block(set_ok_block);
    }

    call_args_tuple
}

fn emit_positional_vectorcall(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrCodegen],
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let mut arg_values: Vec<ir::Value> = Vec::with_capacity(args.len());
    let mut arg_borrowed: Vec<bool> = Vec::with_capacity(args.len());
    for arg in args {
        let borrowed_arg = codegen_expr_is_borrowable(
            arg,
            local_names,
            &ctx.stack_slots,
            ctx.storage_layout.as_ref(),
        );
        arg_borrowed.push(borrowed_arg);
        arg_values.push(emit_codegen_expr(
            fb,
            arg,
            local_names,
            local_values,
            ctx,
            borrowed_arg,
            jit_module,
            func_imports,
        ));
    }
    emit_positional_vectorcall_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        ctx,
    )
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
    let mut arg_values: Vec<ir::Value> = Vec::with_capacity(args.len());
    let mut arg_borrowed: Vec<bool> = Vec::with_capacity(args.len());
    for arg in args {
        let borrowed_arg = codegen_expr_is_borrowable_from_local_env(
            arg,
            local_env,
            &ctx.stack_slots,
            ctx.storage_layout.as_ref(),
        );
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
    emit_positional_vectorcall_with_arg_values(
        fb,
        callable,
        callable_is_borrowed,
        arg_values,
        arg_borrowed,
        ctx,
    )
}

fn emit_positional_vectorcall_with_arg_values(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    arg_values: Vec<ir::Value>,
    arg_borrowed: Vec<bool>,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
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
    let call_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
    let call_null_block = fb.create_block();
    let call_ok_block = fb.create_block();
    fb.append_block_param(call_ok_block, ptr_ty);
    fb.ins().brif(
        call_is_null,
        call_null_block,
        &[],
        call_ok_block,
        &[ir::BlockArg::Value(call_value)],
    );

    fb.switch_to_block(call_null_block);
    let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
    for (value, borrowed_arg) in arg_values.iter().copied().zip(arg_borrowed.iter().copied()) {
        if !borrowed_arg {
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, value]);
        }
    }
    if !callable_is_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, callable]);
    }
    emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(call_ok_block);
    for (value, borrowed_arg) in arg_values.into_iter().zip(arg_borrowed.into_iter()) {
        if !borrowed_arg {
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, value]);
        }
    }
    if !callable_is_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, callable]);
    }
    fb.block_params(call_ok_block)[0]
}

fn emit_object_call_with_tuple_args(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    call_args_tuple: ir::Value,
    kwargs_obj: Option<ir::Value>,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
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
    emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        func_ref,
        args.as_slice(),
        owned_inputs.as_slice(),
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
    let call_value = emit_checked_owned_pyobject_call_with_cleanup(
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
    );
    fb.ins()
        .call(ctx.decref_ref, &[ctx.consts.thread_state_value, call_value]);
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
    let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
    fb.ins().call(
        ctx.decref_ref,
        &[ctx.consts.thread_state_value, failed_kwargs],
    );
    for value in cleanup_on_error {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, *value]);
    }
    emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
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
    let mut tuple_items: Vec<(ir::Value, bool)> = Vec::with_capacity(args.len());
    for arg in args {
        let borrowed_arg = codegen_expr_is_borrowable_from_local_env(
            arg,
            local_env,
            &ctx.stack_slots,
            ctx.storage_layout.as_ref(),
        );
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
        let value_borrowed = codegen_expr_is_borrowable_from_local_env(
            value_expr,
            local_env,
            &ctx.stack_slots,
            ctx.storage_layout.as_ref(),
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

    emit_object_call_with_tuple_args(
        fb,
        callable,
        callable_is_borrowed,
        call_args_tuple,
        Some(kwargs_obj),
        ctx,
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
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    let list_callable = emit_checked_runtime_name_object(fb, "list", ctx);
    let args_list = emit_checked_owned_pyobject_call_with_cleanup(
        fb,
        ctx,
        ctx.py_call_object_ref,
        &[list_callable, ctx.consts.empty_tuple_const],
        &[list_callable],
    );

    let kwargs_obj = if keywords.is_empty() {
        None
    } else {
        Some(emit_empty_dict_with_args_tuple(
            fb,
            ctx.consts.empty_tuple_const,
            true,
            ctx,
        ))
    };

    for arg in args {
        let (value_expr, method_name) = match arg {
            CallArgPositional::Positional(value_expr) => (value_expr, b"append".as_slice()),
            CallArgPositional::Starred(value_expr) => (value_expr, b"extend".as_slice()),
        };
        let value_borrowed = codegen_expr_is_borrowable_from_local_env(
            value_expr,
            local_env,
            &ctx.stack_slots,
            ctx.storage_layout.as_ref(),
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
                let value_borrowed = codegen_expr_is_borrowable_from_local_env(
                    value,
                    local_env,
                    &ctx.stack_slots,
                    ctx.storage_layout.as_ref(),
                );
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
                let value_borrowed = codegen_expr_is_borrowable_from_local_env(
                    value_expr,
                    local_env,
                    &ctx.stack_slots,
                    ctx.storage_layout.as_ref(),
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

    emit_object_call_with_tuple_args(
        fb,
        callable,
        callable_is_borrowed,
        call_args_tuple,
        kwargs_obj,
        ctx,
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
    let bool_value = fb
        .ins()
        .select(is_true, ctx.consts.true_const, ctx.consts.false_const);
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
            let callable_is_borrowed = codegen_expr_is_borrowable_from_local_env(
                op.value.as_ref(),
                local_env,
                &ctx.stack_slots,
                ctx.storage_layout.as_ref(),
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
            let callee_id = emit_callee_function_id_checked(fb, callable, ctx);
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

fn emit_checked_i32_result(
    fb: &mut FunctionBuilder<'_>,
    result: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let errored = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, result, -1);
    let ok_block = fb.create_block();
    fb.append_block_param(ok_block, ctx.consts.i32_ty);
    fb.ins().brif(
        errored,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        ok_block,
        &[ir::BlockArg::Value(result)],
    );
    fb.switch_to_block(ok_block);
    fb.block_params(ok_block)[0]
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
    let instr_id = call.semantic_instr_id();
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
        out.extend(
            owner_types
                .into_iter()
                .map(|owner| DirectMethodSpecialization {
                    function_id,
                    descriptor_function: owner.function_obj as ObjPtr,
                    owner_type: owner.owner_type,
                    type_version: owner.type_version,
                    arg_plan: arg_plan.clone(),
                }),
        );
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
    let instr_id = call.semantic_instr_id();
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
        out.extend(
            owner_types
                .into_iter()
                .map(|owner| DirectConstructorSpecialization {
                    function_id,
                    init_function: owner.init_function_obj as ObjPtr,
                    owner_type: owner.owner_type,
                    type_version: owner.type_version,
                    arg_plan: arg_plan.clone(),
                }),
        );
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

fn codegen_expr_const_u64(
    expr: &InstrCodegen,
    module_constants: &ModuleCodegenConstants,
) -> Option<u64> {
    match expr {
        InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_u64_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn collect_make_function_targets(
    function: &BlockPyFunction<CodegenModuleShape>,
    module_constants: &ModuleCodegenConstants,
) -> HashSet<FunctionId> {
    struct MakeFunctionTargetCollector<'a> {
        module_constants: &'a ModuleCodegenConstants,
        out: &'a mut HashSet<FunctionId>,
    }

    impl Visit<InstrCodegen> for MakeFunctionTargetCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            if let InstrCodegen::Call(call) = expr
                && codegen_expr_helper_name(call.func.as_ref(), self.module_constants)
                    == Some("make_function")
                && let Some(CallArgPositional::Positional(function_id_expr)) = call.args.first()
                && let Some(packed_function_id) =
                    codegen_expr_const_u64(function_id_expr, self.module_constants)
            {
                self.out.insert(FunctionId::from_packed(packed_function_id));
            }
            expr.visit_children(self);
        }
    }

    let mut out = HashSet::new();
    let mut collector = MakeFunctionTargetCollector {
        module_constants,
        out: &mut out,
    };
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

fn load_call_target_specializations(
    module_name: &str,
    function_id: FunctionId,
) -> Result<HashMap<InstrId, Vec<FunctionId>>, String> {
    if specialization_mode_is_profile() {
        return Ok(HashMap::new());
    }
    let Some(path) = counter_dump_input_path_from_env() else {
        return Ok(HashMap::new());
    };
    let path = path.as_path();
    if !path.exists() {
        return Ok(HashMap::new());
    }
    read_call_target_specializations_from_file(path, module_name, function_id)
}

fn load_operator_specializations(
    module_name: &str,
    function_id: FunctionId,
) -> Result<HashMap<InstrId, Vec<u64>>, String> {
    if specialization_mode_is_profile() {
        return Ok(HashMap::new());
    }
    let Some(path) = counter_dump_input_path_from_env() else {
        return Ok(HashMap::new());
    };
    let path = path.as_path();
    if !path.exists() {
        return Ok(HashMap::new());
    }
    read_operator_specializations_from_file(path, module_name, function_id)
}

fn load_branch_preferences(
    module_name: &str,
    function_id: FunctionId,
) -> Result<HashMap<InstrId, bool>, String> {
    if specialization_mode_is_profile() {
        return Ok(HashMap::new());
    }
    let Some(path) = counter_dump_input_path_from_env() else {
        return Ok(HashMap::new());
    };
    let path = path.as_path();
    if !path.exists() {
        return Ok(HashMap::new());
    }
    read_branch_preferences_from_file(path, module_name, function_id)
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

    Ok(Some(FieldIndexSpecialization {
        expected_index,
        owner_type,
        type_version,
    }))
}

fn load_field_index_specializations()
-> Result<HashMap<String, Vec<FieldIndexSpecialization>>, String> {
    if specialization_mode_is_profile() {
        return Ok(HashMap::new());
    }
    let Some(path) = counter_dump_input_path_from_env() else {
        return Ok(HashMap::new());
    };
    let path = path.as_path();
    if !path.exists() {
        return Ok(HashMap::new());
    }
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

fn specialization_mode_from_env() -> Option<String> {
    env::var("SOAC_OPT_MODE")
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty() && raw != "none")
}

fn specialization_mode_is_profile() -> bool {
    specialization_mode_from_env().as_deref() == Some("profile")
}

fn behavior_change_indexed_stores_enabled() -> bool {
    specialization_mode_from_env().as_deref() == Some("apply")
}

fn counter_dump_input_path_from_env() -> Option<std::path::PathBuf> {
    match specialization_mode_from_env().as_deref() {
        Some("verify" | "apply") => soac_work_dir_from_env().map(|dir| dir.join("profile.bin")),
        _ => None,
    }
}

fn soac_work_dir_from_env() -> Option<std::path::PathBuf> {
    env::var_os("SOAC_WORK_DIR")
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn emit_callee_function_id_checked(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    ctx: &JitEmitCtx<'_>,
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
    let py_function_type = fb
        .ins()
        .iconst(ptr_ty, ctx.consts.py_function_type_ptr as i64);
    let is_function = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, callable_type, py_function_type);
    fb.ins()
        .brif(is_function, function_block, &[], maybe_method_block, &[]);

    fb.switch_to_block(function_block);
    fb.ins()
        .jump(function_value_block, &[ir::BlockArg::Value(callable)]);

    fb.switch_to_block(maybe_method_block);
    let py_method_type = fb
        .ins()
        .iconst(ptr_ty, ctx.consts.py_method_type_ptr as i64);
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
    let py_type_type = fb.ins().iconst(ptr_ty, ctx.consts.py_type_type_ptr as i64);
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
    let Some(&counter_ptr) = ctx.top_value_counter_ptrs.get(counter_id.0) else {
        return;
    };
    if counter_ptr.is_null() {
        return;
    }
    let counter_value = fb.ins().iconst(ctx.consts.ptr_ty, counter_ptr as i64);
    fb.ins().call(
        ctx.record_top_value_sample_ref,
        &[counter_value, observed_value],
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
    target_function: &BlockPyFunction<CodegenModuleShape>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
) -> ir::Value {
    debug_assert_eq!(arg_values.len(), target_function.params.len());
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
    let direct_func_id = ctx
        .direct_call_functions
        .get(&target_function.function_id)
        .map(|function| function.func_id)
        .expect("direct call emission requires a predeclared process-JIT function symbol");
    ctx.direct_edge_stats.record_resolved_direct_edge();

    let function_env = emit_direct_function_env_load_or_slow_path(fb, callable, ctx);
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
    let func_ref = jit_module.declare_func_in_func(direct_func_id, &mut fb.func);
    let call_inst = fb.ins().call(func_ref, &call_args);
    let call_value = fb.inst_results(call_inst)[0];
    fb.ins()
        .call(ctx.leave_recursive_ref, &[ctx.consts.thread_state_value]);

    for (value, borrowed_arg) in arg_values.into_iter().zip(arg_borrowed.into_iter()) {
        if !borrowed_arg {
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, value]);
        }
    }
    if !callable_is_borrowed {
        fb.ins()
            .call(ctx.decref_ref, &[ctx.consts.thread_state_value, callable]);
    }

    call_value
}

fn emit_direct_call_resolved_with_arg_values(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    arg_values: Vec<ir::Value>,
    arg_borrowed: Vec<bool>,
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
        target_function,
        ctx,
        jit_module,
    );
    let call_is_null = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
    let call_fail_block = fb.create_block();
    let call_ok_block = fb.create_block();
    fb.append_block_param(call_ok_block, ptr_ty);
    fb.ins().brif(
        call_is_null,
        call_fail_block,
        &[],
        call_ok_block,
        &[ir::BlockArg::Value(call_value)],
    );
    fb.switch_to_block(call_fail_block);
    let error_value = emit_take_current_raised_exception_or_trap(
        fb,
        ctx.consts.ptr_ty,
        ctx.consts.thread_state_value,
    );
    fb.ins().call(
        ctx.set_raised_exception_ref,
        &[ctx.consts.thread_state_value, error_value],
    );
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

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
    let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
    for (value, borrowed_arg) in arg_values.iter().copied().zip(arg_borrowed.iter().copied()) {
        if !borrowed_arg {
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, value]);
        }
    }
    emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
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
    let init_callable = fb.ins().iconst(ptr_ty, specialization.init_function as i64);
    let init_result = emit_direct_call_resolved_raw_with_arg_values(
        fb,
        init_callable,
        true,
        init_arg_values,
        init_arg_borrowed,
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
    let error_value = emit_take_current_raised_exception_or_trap(
        fb,
        ctx.consts.ptr_ty,
        ctx.consts.thread_state_value,
    );
    fb.ins()
        .call(ctx.decref_ref, &[ctx.consts.thread_state_value, allocated]);
    fb.ins().call(
        ctx.set_raised_exception_ref,
        &[ctx.consts.thread_state_value, error_value],
    );
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

fn emit_direct_call_resolved_with_arg_plan(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&InstrCodegen],
    arg_plan: &DirectCallArgPlan,
    target_function: &BlockPyFunction<CodegenModuleShape>,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let mut provided_arg_values: Vec<ir::Value> = Vec::with_capacity(args.len());
    let mut provided_arg_borrowed: Vec<bool> = Vec::with_capacity(args.len());
    for arg in args {
        let borrowed_arg = codegen_expr_is_borrowable(
            arg,
            local_names,
            &ctx.stack_slots,
            ctx.storage_layout.as_ref(),
        );
        provided_arg_borrowed.push(borrowed_arg);
        provided_arg_values.push(emit_codegen_expr(
            fb,
            arg,
            local_names,
            local_values,
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
        target_function,
        ctx,
        jit_module,
    )
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
        let borrowed_arg = codegen_expr_is_borrowable_from_local_env(
            arg,
            local_env,
            &ctx.stack_slots,
            ctx.storage_layout.as_ref(),
        );
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
        let borrowed_arg = codegen_expr_is_borrowable_from_local_env(
            arg,
            local_env,
            &ctx.stack_slots,
            ctx.storage_layout.as_ref(),
        );
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
        let borrowed_arg = codegen_expr_is_borrowable_from_local_env(
            arg,
            local_env,
            &ctx.stack_slots,
            ctx.storage_layout.as_ref(),
        );
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
    let callable = fb
        .ins()
        .iconst(ptr_ty, specialization.descriptor_function as i64);
    emit_direct_call_resolved_with_arg_values(
        fb,
        callable,
        true,
        arg_values,
        arg_borrowed,
        target_function,
        ctx,
        jit_module,
    )
}

fn emit_call_direct_expr(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_blockpy::block_py::CallDirect<InstrCodegen>,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let mut fallback = || {
        let fallback = InstrCodegen::Call(
            soac_blockpy::block_py::Call::new(
                (*call.callable).clone(),
                call.args.clone(),
                call.keywords.clone(),
            )
            .with_meta(call.meta()),
        );
        emit_codegen_expr(
            fb,
            &fallback,
            local_names,
            local_values,
            ctx,
            false,
            jit_module,
            func_imports,
        )
    };

    let Some(target_function) = direct_call_target_function(ctx, call.function_id) else {
        ctx.direct_edge_stats
            .record_call_direct_missing_target_fallback();
        return fallback();
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
        Err(DirectCallIncompatibility::MissingPredeclared) => {
            ctx.direct_edge_stats
                .record_call_direct_missing_predeclared_fallback();
            return fallback();
        }
        Err(_) => {
            ctx.direct_edge_stats
                .record_call_direct_unsupported_shape_fallback();
            return fallback();
        }
    };

    let callable_is_borrowed = codegen_expr_is_borrowable(
        call.callable.as_ref(),
        local_names,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
    );
    let callable = emit_codegen_expr(
        fb,
        call.callable.as_ref(),
        local_names,
        local_values,
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
    emit_direct_call_resolved_with_arg_plan(
        fb,
        callable,
        callable_is_borrowed,
        args.as_slice(),
        &arg_plan,
        target_function,
        local_names,
        local_values,
        ctx,
        jit_module,
        func_imports,
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
        Err(DirectCallIncompatibility::MissingPredeclared) => {
            ctx.direct_edge_stats
                .record_call_direct_missing_predeclared_fallback();
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

    let callable_is_borrowed = codegen_expr_is_borrowable_from_local_env(
        call.callable.as_ref(),
        local_env,
        &ctx.stack_slots,
        ctx.storage_layout.as_ref(),
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

fn emit_codegen_expr(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrCodegen,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let _value_facts = ctx.value_facts_for_expr(expr);
    let incref_ref = ctx.incref_ref;
    let decref_ref = ctx.decref_ref;
    let thread_state_value = ctx.consts.thread_state_value;
    let py_call_ref = ctx.py_call_positional_three_ref;
    let step_null_block = ctx.consts.step_null_block;
    let ptr_ty = ctx.consts.ptr_ty;
    let i64_ty = ctx.consts.i64_ty;
    let deleted_const = ctx.consts.deleted_const;
    let empty_tuple_const = ctx.consts.empty_tuple_const;
    let block_const = ctx.consts.block_const;
    let pyobject_getattr_ref = ctx.pyobject_getattr_ref;
    let pyobject_setitem_ref = ctx.pyobject_setitem_ref;
    let raise_deleted_name_error_ref = ctx.raise_deleted_name_error_ref;
    let py_call_object_ref = ctx.py_call_object_ref;
    let py_call_with_kw_ref = ctx.py_call_with_kw_ref;
    let tuple_new_ref = ctx.tuple_new_ref;
    let tuple_set_item_ref = ctx.tuple_set_item_ref;

    match expr {
        InstrCodegen::Load(op) => {
            return emit_codegen_located_name_load(
                fb,
                &op.name,
                op.semantic_instr_id(),
                local_names,
                local_values,
                ctx,
                borrowed,
            );
        }
        InstrCodegen::IncrementCounter(op) => {
            assert!(
                !borrowed,
                "increment_counter must not request a borrowed result"
            );
            return emit_increment_counter(fb, op.counter_id, ctx);
        }
        expr @ (InstrCodegen::BinOp(_)
        | InstrCodegen::UnaryOp(_)
        | InstrCodegen::CalleeFunctionId(_)
        | InstrCodegen::GetAttr(_)
        | InstrCodegen::SetAttr(_)
        | InstrCodegen::GetItem(_)
        | InstrCodegen::SetItem(_)
        | InstrCodegen::DelItem(_)
        | InstrCodegen::Store(_)
        | InstrCodegen::Del(_)
        | InstrCodegen::MakeCell(_)
        | InstrCodegen::CellRef(_)
        | InstrCodegen::MakeFunction(_)) => {
            assert!(
                !borrowed,
                "codegen operation expression must not use borrowed result"
            );
            let mut intrinsic_state = CodegenIntrinsicEmitState {
                fb,
                local_names,
                local_values,
                ctx,
                jit_module,
                func_imports,
            };
            if matches!(expr, InstrCodegen::MakeFunction(_)) {
                panic!("MakeFunction should lower to a regular call before codegen");
            }
            if let Some(value) = intrinsics::emit_operation(expr, &mut intrinsic_state) {
                return value;
            }
            match expr {
                InstrCodegen::CellRef(op) => emit_raw_cell_object_for_location(
                    intrinsic_state.fb,
                    op.location,
                    "cell_ref",
                    intrinsic_state.local_names,
                    intrinsic_state.local_values,
                    intrinsic_state.ctx,
                ),
                InstrCodegen::Store(op) => {
                    if let Some(location) = op.name.local_location() {
                        let layout =
                            intrinsic_state.ctx.storage_layout.as_ref().expect(
                                "Store local slot should have storage layout during codegen",
                            );
                        let name = local_name_for_location(layout, location);
                        let value_obj = emit_codegen_expr(
                            intrinsic_state.fb,
                            &op.value,
                            intrinsic_state.local_names,
                            intrinsic_state.local_values,
                            intrinsic_state.ctx,
                            false,
                            intrinsic_state.jit_module,
                            intrinsic_state.func_imports,
                        );
                        bind_local_value(
                            intrinsic_state.fb,
                            intrinsic_state.local_names,
                            intrinsic_state.local_values,
                            name,
                            value_obj,
                            &intrinsic_state.ctx.stack_slots,
                            intrinsic_state.ctx.consts.ptr_ty,
                            intrinsic_state.ctx.consts.thread_state_value,
                            intrinsic_state.ctx.incref_ref,
                            intrinsic_state.ctx.decref_ref,
                        );
                        intrinsic_state.fb.ins().call(
                            intrinsic_state.ctx.incref_ref,
                            &[intrinsic_state.ctx.consts.none_const],
                        );
                        return intrinsic_state.ctx.consts.none_const;
                    }
                    let Some(location) = op.name.cell_location() else {
                        panic!("Store should be resolved before codegen: {op:?}");
                    };
                    if location.is_owned() && matches!(op.value.as_ref(), InstrCodegen::MakeCell(_))
                    {
                        let layout = intrinsic_state.ctx.storage_layout.as_ref().expect(
                            "Store owned cell slot should have storage layout during codegen",
                        );
                        let closure_slot =
                            layout.local_cell_slot(location.slot()).unwrap_or_else(|| {
                                panic!(
                                    "missing owned cell slot mapping for owned cell location {}",
                                    location.slot()
                                )
                            });
                        let value_obj = emit_codegen_expr(
                            intrinsic_state.fb,
                            &op.value,
                            intrinsic_state.local_names,
                            intrinsic_state.local_values,
                            intrinsic_state.ctx,
                            false,
                            intrinsic_state.jit_module,
                            intrinsic_state.func_imports,
                        );
                        bind_local_value(
                            intrinsic_state.fb,
                            intrinsic_state.local_names,
                            intrinsic_state.local_values,
                            closure_slot.storage_name.as_str(),
                            value_obj,
                            &intrinsic_state.ctx.stack_slots,
                            intrinsic_state.ctx.consts.ptr_ty,
                            intrinsic_state.ctx.consts.thread_state_value,
                            intrinsic_state.ctx.incref_ref,
                            intrinsic_state.ctx.decref_ref,
                        );
                        intrinsic_state.fb.ins().call(
                            intrinsic_state.ctx.incref_ref,
                            &[intrinsic_state.ctx.consts.none_const],
                        );
                        return intrinsic_state.ctx.consts.none_const;
                    }
                    let raw_cell = emit_raw_cell_object_for_location(
                        intrinsic_state.fb,
                        location,
                        "Store",
                        intrinsic_state.local_names,
                        intrinsic_state.local_values,
                        intrinsic_state.ctx,
                    );
                    let value_borrowed = codegen_expr_is_borrowable(
                        &op.value,
                        intrinsic_state.local_names,
                        &intrinsic_state.ctx.stack_slots,
                        intrinsic_state.ctx.storage_layout.as_ref(),
                    );
                    let value_obj = emit_codegen_expr(
                        intrinsic_state.fb,
                        &op.value,
                        intrinsic_state.local_names,
                        intrinsic_state.local_values,
                        intrinsic_state.ctx,
                        value_borrowed,
                        intrinsic_state.jit_module,
                        intrinsic_state.func_imports,
                    );
                    let call_inst = intrinsic_state
                        .fb
                        .ins()
                        .call(intrinsic_state.ctx.store_cell_ref, &[raw_cell, value_obj]);
                    intrinsic_state.fb.ins().call(
                        intrinsic_state.ctx.decref_ref,
                        &[intrinsic_state.ctx.consts.thread_state_value, raw_cell],
                    );
                    if !value_borrowed {
                        intrinsic_state.fb.ins().call(
                            intrinsic_state.ctx.decref_ref,
                            &[intrinsic_state.ctx.consts.thread_state_value, value_obj],
                        );
                    }
                    let call_value = intrinsic_state.fb.inst_results(call_inst)[0];
                    intrinsics::OperationEmitState::finish_owned_result(
                        &mut intrinsic_state,
                        call_value,
                    )
                }
                InstrCodegen::Del(op) => {
                    if let Some(location) = op.name.local_location() {
                        let layout = intrinsic_state
                            .ctx
                            .storage_layout
                            .as_ref()
                            .expect("Del local slot should have storage layout during codegen");
                        let name = local_name_for_location(layout, location);
                        delete_local_value(
                            intrinsic_state.fb,
                            intrinsic_state.local_names,
                            intrinsic_state.local_values,
                            name,
                            &intrinsic_state.ctx.stack_slots,
                            intrinsic_state.ctx.consts.ptr_ty,
                            intrinsic_state.ctx.consts.thread_state_value,
                            intrinsic_state.ctx.decref_ref,
                        )
                        .unwrap_or_else(|error| panic!("{error}"));
                        intrinsic_state.fb.ins().call(
                            intrinsic_state.ctx.incref_ref,
                            &[intrinsic_state.ctx.consts.none_const],
                        );
                        return intrinsic_state.ctx.consts.none_const;
                    }
                    let Some(location) = op.name.cell_location() else {
                        panic!("Del should be resolved before codegen: {op:?}");
                    };
                    let raw_cell = emit_raw_cell_object_for_location(
                        intrinsic_state.fb,
                        location,
                        "Del",
                        intrinsic_state.local_names,
                        intrinsic_state.local_values,
                        intrinsic_state.ctx,
                    );
                    intrinsics::emit_del_deref_raw_cell(raw_cell, op.quietly, &mut intrinsic_state)
                }
                InstrCodegen::MakeFunction(_) => {
                    unreachable!("MakeFunction should panic before intrinsic fallback")
                }
                _ => {
                    panic!("operation {expr:?} should have been handled by direct emitter")
                }
            }
        }
        InstrCodegen::CallDirect(call) => {
            assert!(
                !borrowed,
                "codegen direct-call expression must not use borrowed result"
            );
            return emit_call_direct_expr(
                fb,
                call,
                local_names,
                local_values,
                ctx,
                jit_module,
                func_imports,
            );
        }
        InstrCodegen::Call(call) => {
            assert!(
                !borrowed,
                "codegen call expression must not use borrowed result"
            );
            let null_ptr = fb.ins().iconst(ptr_ty, 0);
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
                    CallArgKeyword::Named { arg, value } => {
                        simple_keywords.push((arg.as_str(), value))
                    }
                    CallArgKeyword::Starred(_) => has_unpack = true,
                }
            }
            let args = simple_args.clone();
            let keywords = simple_keywords.clone();

            if !has_unpack
                && simple_keywords.is_empty()
                && simple_args.is_empty()
                && codegen_expr_runtime_helper(call.func.as_ref(), ctx)
                    == Some(RuntimeHelperId::Globals)
            {
                fb.ins().call(incref_ref, &[block_const]);
                return block_const;
            }

            if !has_unpack
                && simple_keywords.is_empty()
                && simple_args.len() == 3
                && matches!(
                    codegen_expr_helper_name(call.func.as_ref(), ctx.module_constants),
                    Some("call_super")
                )
            {
                return emit_codegen_super_helper_call(
                    fb,
                    call.func.as_ref(),
                    simple_args[0],
                    simple_args[1],
                    simple_args[2],
                    local_names,
                    local_values,
                    ctx,
                    jit_module,
                    func_imports,
                );
            }

            if !has_unpack
                && simple_keywords.is_empty()
                && codegen_expr_runtime_helper(call.func.as_ref(), ctx)
                    == Some(RuntimeHelperId::Str)
                && simple_args.len() == 1
            {
                if let Some(value) = codegen_expr_const_string(simple_args[0], ctx.module_constants)
                {
                    return emit_owned_module_constant(
                        fb,
                        ctx.module_constants
                            .require_unicode_constant_id(value.as_str()),
                        ctx,
                    );
                }
            }

            if !has_unpack
                && simple_keywords.is_empty()
                && simple_args.len() == 1
                && codegen_expr_runtime_helper(call.func.as_ref(), ctx)
                    == Some(RuntimeHelperId::NextOrSentinel)
            {
                let iterator_expr = simple_args[0];
                let iterator_is_borrowed = codegen_expr_is_borrowable(
                    iterator_expr,
                    local_names,
                    &ctx.stack_slots,
                    ctx.storage_layout.as_ref(),
                );
                let iterator = emit_codegen_expr(
                    fb,
                    iterator_expr,
                    local_names,
                    local_values,
                    ctx,
                    iterator_is_borrowed,
                    jit_module,
                    func_imports,
                );
                let sentinel = emit_owned_module_constant(
                    fb,
                    ctx.module_constants
                        .require_runtime_name_constant_id("ITER_COMPLETE"),
                    ctx,
                );
                let next_or_sentinel_ref = func_imports.get_or_panic(
                    jit_module,
                    &mut fb.func,
                    &DP_JIT_NEXT_OR_SENTINEL_IMPORT,
                );
                let next_inst = fb.ins().call(next_or_sentinel_ref, &[iterator, sentinel]);
                let mut owned_inputs = Vec::with_capacity(2);
                if !iterator_is_borrowed {
                    owned_inputs.push(iterator);
                }
                owned_inputs.push(sentinel);
                let next_value = emit_decref_owned_inputs_after_nullable_result(
                    fb,
                    ctx,
                    fb.inst_results(next_inst)[0],
                    &owned_inputs,
                );
                let value_is_null =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, next_value, null_ptr);
                let value_ok_block = fb.create_block();
                fb.append_block_param(value_ok_block, ptr_ty);
                fb.ins().brif(
                    value_is_null,
                    step_null_block,
                    &step_null_block_args(ctx),
                    value_ok_block,
                    &[ir::BlockArg::Value(next_value)],
                );
                fb.switch_to_block(value_ok_block);
                return fb.block_params(value_ok_block)[0];
            }

            if has_unpack {
                let callable_is_borrowed = codegen_expr_is_borrowable(
                    call.func.as_ref(),
                    local_names,
                    &ctx.stack_slots,
                    ctx.storage_layout.as_ref(),
                );
                let callable = emit_codegen_expr(
                    fb,
                    call.func.as_ref(),
                    local_names,
                    local_values,
                    ctx,
                    callable_is_borrowed,
                    jit_module,
                    func_imports,
                );
                let list_name_obj = emit_owned_module_constant(
                    fb,
                    ctx.module_constants.require_unicode_constant_id("list"),
                    ctx,
                );
                let list_callable_inst = fb.ins().call(ctx.load_runtime_obj_ref, &[list_name_obj]);
                let list_callable = emit_decref_owned_input_after_nullable_result(
                    fb,
                    ctx,
                    fb.inst_results(list_callable_inst)[0],
                    list_name_obj,
                );
                let list_callable_is_null =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, list_callable, null_ptr);
                let list_callable_ok = fb.create_block();
                fb.append_block_param(list_callable_ok, ptr_ty);
                fb.ins().brif(
                    list_callable_is_null,
                    step_null_block,
                    &step_null_block_args(ctx),
                    list_callable_ok,
                    &[ir::BlockArg::Value(list_callable)],
                );
                fb.switch_to_block(list_callable_ok);
                let list_callable = fb.block_params(list_callable_ok)[0];
                let args_list_inst = fb
                    .ins()
                    .call(py_call_object_ref, &[list_callable, empty_tuple_const]);
                let args_list = emit_decref_owned_input_after_nullable_result(
                    fb,
                    ctx,
                    fb.inst_results(args_list_inst)[0],
                    list_callable,
                );
                let args_list_is_null =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, args_list, null_ptr);
                let args_list_ok = fb.create_block();
                fb.append_block_param(args_list_ok, ptr_ty);
                fb.ins().brif(
                    args_list_is_null,
                    step_null_block,
                    &step_null_block_args(ctx),
                    args_list_ok,
                    &[ir::BlockArg::Value(args_list)],
                );
                fb.switch_to_block(args_list_ok);
                let args_list = fb.block_params(args_list_ok)[0];

                let needs_kwargs = !call.keywords.is_empty();
                let kwargs_obj = if needs_kwargs {
                    let dict_name_obj = emit_owned_module_constant(
                        fb,
                        ctx.module_constants.require_unicode_constant_id("dict"),
                        ctx,
                    );
                    let dict_callable_inst =
                        fb.ins().call(ctx.load_runtime_obj_ref, &[dict_name_obj]);
                    let dict_callable = emit_decref_owned_input_after_nullable_result(
                        fb,
                        ctx,
                        fb.inst_results(dict_callable_inst)[0],
                        dict_name_obj,
                    );
                    let dict_callable_is_null =
                        fb.ins()
                            .icmp(ir::condcodes::IntCC::Equal, dict_callable, null_ptr);
                    let dict_callable_ok = fb.create_block();
                    fb.append_block_param(dict_callable_ok, ptr_ty);
                    fb.ins().brif(
                        dict_callable_is_null,
                        step_null_block,
                        &step_null_block_args(ctx),
                        dict_callable_ok,
                        &[ir::BlockArg::Value(dict_callable)],
                    );
                    fb.switch_to_block(dict_callable_ok);
                    let dict_callable = fb.block_params(dict_callable_ok)[0];
                    let kwargs_inst = fb
                        .ins()
                        .call(py_call_object_ref, &[dict_callable, empty_tuple_const]);
                    let kwargs_obj = emit_decref_owned_input_after_nullable_result(
                        fb,
                        ctx,
                        fb.inst_results(kwargs_inst)[0],
                        dict_callable,
                    );
                    let kwargs_is_null =
                        fb.ins()
                            .icmp(ir::condcodes::IntCC::Equal, kwargs_obj, null_ptr);
                    let kwargs_ok = fb.create_block();
                    fb.append_block_param(kwargs_ok, ptr_ty);
                    fb.ins().brif(
                        kwargs_is_null,
                        step_null_block,
                        &step_null_block_args(ctx),
                        kwargs_ok,
                        &[ir::BlockArg::Value(kwargs_obj)],
                    );
                    fb.switch_to_block(kwargs_ok);
                    Some(fb.block_params(kwargs_ok)[0])
                } else {
                    None
                };

                for arg in &call.args {
                    let (value_expr, method_name) = match arg {
                        CallArgPositional::Positional(value_expr) => {
                            (value_expr, b"append".as_slice())
                        }
                        CallArgPositional::Starred(value_expr) => {
                            (value_expr, b"extend".as_slice())
                        }
                    };
                    let method_name_obj = emit_owned_module_constant(
                        fb,
                        ctx.module_constants
                            .require_unicode_constant_id_for_bytes(method_name),
                        ctx,
                    );
                    let method_inst = fb
                        .ins()
                        .call(pyobject_getattr_ref, &[args_list, method_name_obj]);
                    let method_obj = emit_decref_owned_input_after_nullable_result(
                        fb,
                        ctx,
                        fb.inst_results(method_inst)[0],
                        method_name_obj,
                    );
                    let method_is_null =
                        fb.ins()
                            .icmp(ir::condcodes::IntCC::Equal, method_obj, null_ptr);
                    let method_ok = fb.create_block();
                    fb.append_block_param(method_ok, ptr_ty);
                    fb.ins().brif(
                        method_is_null,
                        step_null_block,
                        &step_null_block_args(ctx),
                        method_ok,
                        &[ir::BlockArg::Value(method_obj)],
                    );
                    fb.switch_to_block(method_ok);
                    let method_obj = fb.block_params(method_ok)[0];
                    let value_borrowed = codegen_expr_is_borrowable(
                        value_expr,
                        local_names,
                        &ctx.stack_slots,
                        ctx.storage_layout.as_ref(),
                    );
                    let value_obj = emit_codegen_expr(
                        fb,
                        value_expr,
                        local_names,
                        local_values,
                        ctx,
                        value_borrowed,
                        jit_module,
                        func_imports,
                    );
                    let call_inst = fb.ins().call(
                        py_call_ref,
                        &[
                            ctx.consts.thread_state_value,
                            method_obj,
                            value_obj,
                            null_ptr,
                            null_ptr,
                            null_ptr,
                        ],
                    );
                    let mut owned_inputs = Vec::with_capacity(2);
                    if !value_borrowed {
                        owned_inputs.push(value_obj);
                    }
                    owned_inputs.push(method_obj);
                    let call_value = emit_decref_owned_inputs_after_nullable_result(
                        fb,
                        ctx,
                        fb.inst_results(call_inst)[0],
                        &owned_inputs,
                    );
                    let call_is_null =
                        fb.ins()
                            .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
                    let call_ok = fb.create_block();
                    fb.append_block_param(call_ok, ptr_ty);
                    fb.ins().brif(
                        call_is_null,
                        step_null_block,
                        &step_null_block_args(ctx),
                        call_ok,
                        &[ir::BlockArg::Value(call_value)],
                    );
                    fb.switch_to_block(call_ok);
                    let call_value = fb.block_params(call_ok)[0];
                    fb.ins().call(decref_ref, &[thread_state_value, call_value]);
                }

                for keyword in &call.keywords {
                    match keyword {
                        CallArgKeyword::Named { arg, value } => {
                            let kwargs_obj =
                                kwargs_obj.expect("kwargs object must exist for named kw part");
                            let key_obj = emit_owned_module_constant(
                                fb,
                                ctx.module_constants
                                    .require_unicode_constant_id(arg.as_str()),
                                ctx,
                            );
                            let value_borrowed = codegen_expr_is_borrowable(
                                value,
                                local_names,
                                &ctx.stack_slots,
                                ctx.storage_layout.as_ref(),
                            );
                            let value_obj = emit_codegen_expr(
                                fb,
                                value,
                                local_names,
                                local_values,
                                ctx,
                                value_borrowed,
                                jit_module,
                                func_imports,
                            );
                            let set_inst = fb
                                .ins()
                                .call(pyobject_setitem_ref, &[kwargs_obj, key_obj, value_obj]);
                            fb.ins().call(decref_ref, &[thread_state_value, key_obj]);
                            if !value_borrowed {
                                fb.ins().call(decref_ref, &[thread_state_value, value_obj]);
                            }
                            let set_value = fb.inst_results(set_inst)[0];
                            let set_failed =
                                fb.ins()
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
                            let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
                            fb.ins()
                                .call(decref_ref, &[thread_state_value, failed_kwargs]);
                            fb.ins().call(decref_ref, &[thread_state_value, args_list]);
                            if !callable_is_borrowed {
                                fb.ins().call(decref_ref, &[thread_state_value, callable]);
                            }
                            emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
                            fb.ins().jump(step_null_block, &step_null_block_args(ctx));
                            fb.switch_to_block(set_ok);
                            fb.ins().call(decref_ref, &[thread_state_value, set_value]);
                        }
                        CallArgKeyword::Starred(value_expr) => {
                            let kwargs_obj =
                                kwargs_obj.expect("kwargs object must exist for kwstar part");
                            let update_name_obj = emit_owned_module_constant(
                                fb,
                                ctx.module_constants.require_unicode_constant_id("update"),
                                ctx,
                            );
                            let update_inst = fb
                                .ins()
                                .call(pyobject_getattr_ref, &[kwargs_obj, update_name_obj]);
                            let update_obj = emit_decref_owned_input_after_nullable_result(
                                fb,
                                ctx,
                                fb.inst_results(update_inst)[0],
                                update_name_obj,
                            );
                            let update_is_null =
                                fb.ins()
                                    .icmp(ir::condcodes::IntCC::Equal, update_obj, null_ptr);
                            let update_ok = fb.create_block();
                            fb.append_block_param(update_ok, ptr_ty);
                            fb.ins().brif(
                                update_is_null,
                                step_null_block,
                                &step_null_block_args(ctx),
                                update_ok,
                                &[ir::BlockArg::Value(update_obj)],
                            );
                            fb.switch_to_block(update_ok);
                            let update_obj = fb.block_params(update_ok)[0];
                            let value_borrowed = codegen_expr_is_borrowable(
                                value_expr,
                                local_names,
                                &ctx.stack_slots,
                                ctx.storage_layout.as_ref(),
                            );
                            let value_obj = emit_codegen_expr(
                                fb,
                                value_expr,
                                local_names,
                                local_values,
                                ctx,
                                value_borrowed,
                                jit_module,
                                func_imports,
                            );
                            let call_inst = fb.ins().call(
                                py_call_ref,
                                &[
                                    ctx.consts.thread_state_value,
                                    update_obj,
                                    value_obj,
                                    null_ptr,
                                    null_ptr,
                                    null_ptr,
                                ],
                            );
                            let mut owned_inputs = Vec::with_capacity(2);
                            if !value_borrowed {
                                owned_inputs.push(value_obj);
                            }
                            owned_inputs.push(update_obj);
                            let call_value = emit_decref_owned_inputs_after_nullable_result(
                                fb,
                                ctx,
                                fb.inst_results(call_inst)[0],
                                &owned_inputs,
                            );
                            let call_is_null =
                                fb.ins()
                                    .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
                            let call_ok = fb.create_block();
                            fb.append_block_param(call_ok, ptr_ty);
                            fb.ins().brif(
                                call_is_null,
                                step_null_block,
                                &step_null_block_args(ctx),
                                call_ok,
                                &[ir::BlockArg::Value(call_value)],
                            );
                            fb.switch_to_block(call_ok);
                            let call_value = fb.block_params(call_ok)[0];
                            fb.ins().call(decref_ref, &[thread_state_value, call_value]);
                        }
                    }
                }

                let tuple_name_obj = emit_owned_module_constant(
                    fb,
                    ctx.module_constants
                        .require_unicode_constant_id("tuple_from_iter"),
                    ctx,
                );
                let tuple_callable_inst =
                    fb.ins().call(ctx.load_runtime_obj_ref, &[tuple_name_obj]);
                let tuple_callable = emit_decref_owned_input_after_nullable_result(
                    fb,
                    ctx,
                    fb.inst_results(tuple_callable_inst)[0],
                    tuple_name_obj,
                );
                let tuple_callable_is_null =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, tuple_callable, null_ptr);
                let tuple_callable_ok = fb.create_block();
                fb.append_block_param(tuple_callable_ok, ptr_ty);
                fb.ins().brif(
                    tuple_callable_is_null,
                    step_null_block,
                    &step_null_block_args(ctx),
                    tuple_callable_ok,
                    &[ir::BlockArg::Value(tuple_callable)],
                );
                fb.switch_to_block(tuple_callable_ok);
                let tuple_callable = fb.block_params(tuple_callable_ok)[0];
                let tuple_call_inst = fb.ins().call(
                    py_call_ref,
                    &[
                        ctx.consts.thread_state_value,
                        tuple_callable,
                        args_list,
                        null_ptr,
                        null_ptr,
                        null_ptr,
                    ],
                );
                let call_args_tuple = emit_decref_owned_inputs_after_nullable_result(
                    fb,
                    ctx,
                    fb.inst_results(tuple_call_inst)[0],
                    &[tuple_callable, args_list],
                );
                let call_args_tuple_is_null =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, call_args_tuple, null_ptr);
                let call_args_tuple_ok = fb.create_block();
                fb.append_block_param(call_args_tuple_ok, ptr_ty);
                fb.ins().brif(
                    call_args_tuple_is_null,
                    step_null_block,
                    &step_null_block_args(ctx),
                    call_args_tuple_ok,
                    &[ir::BlockArg::Value(call_args_tuple)],
                );
                fb.switch_to_block(call_args_tuple_ok);
                let call_args_tuple = fb.block_params(call_args_tuple_ok)[0];

                let call_inst = if let Some(kwargs_obj) = kwargs_obj {
                    fb.ins().call(
                        py_call_with_kw_ref,
                        &[callable, call_args_tuple, kwargs_obj],
                    )
                } else {
                    fb.ins()
                        .call(py_call_object_ref, &[callable, call_args_tuple])
                };
                let call_value = fb.inst_results(call_inst)[0];
                let call_is_null = fb
                    .ins()
                    .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
                let call_fail_block = fb.create_block();
                let call_ok_block = fb.create_block();
                fb.append_block_param(call_ok_block, ptr_ty);
                fb.ins().brif(
                    call_is_null,
                    call_fail_block,
                    &[],
                    call_ok_block,
                    &[ir::BlockArg::Value(call_value)],
                );
                fb.switch_to_block(call_fail_block);
                let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
                if let Some(kwargs_obj) = kwargs_obj {
                    fb.ins().call(decref_ref, &[thread_state_value, kwargs_obj]);
                }
                fb.ins()
                    .call(decref_ref, &[thread_state_value, call_args_tuple]);
                if !callable_is_borrowed {
                    fb.ins().call(decref_ref, &[thread_state_value, callable]);
                }
                emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
                fb.ins().jump(step_null_block, &step_null_block_args(ctx));

                fb.switch_to_block(call_ok_block);
                if let Some(kwargs_obj) = kwargs_obj {
                    fb.ins().call(decref_ref, &[thread_state_value, kwargs_obj]);
                }
                fb.ins()
                    .call(decref_ref, &[thread_state_value, call_args_tuple]);
                if !callable_is_borrowed {
                    fb.ins().call(decref_ref, &[thread_state_value, callable]);
                }
                return fb.block_params(call_ok_block)[0];
            }

            if let Some(helper_id) = codegen_expr_runtime_helper(call.func.as_ref(), ctx) {
                if keywords.is_empty() && helper_id == RuntimeHelperId::Str && args.len() == 1 {
                    if let Some(value) = codegen_expr_const_string(args[0], ctx.module_constants) {
                        return emit_owned_module_constant(
                            fb,
                            ctx.module_constants
                                .require_unicode_constant_id(value.as_str()),
                            ctx,
                        );
                    }
                }
                if keywords.is_empty() && args.is_empty() && helper_id == RuntimeHelperId::Globals {
                    fb.ins().call(incref_ref, &[block_const]);
                    return block_const;
                }
                if !has_unpack && keywords.is_empty() {
                    if helper_id == RuntimeHelperId::TupleValues {
                        let mut arg_values: Vec<ir::Value> = Vec::with_capacity(args.len());
                        let mut borrowed_args: Vec<bool> = Vec::with_capacity(args.len());
                        for arg in &args {
                            let borrowed_arg = codegen_expr_is_borrowable(
                                arg,
                                local_names,
                                &ctx.stack_slots,
                                ctx.storage_layout.as_ref(),
                            );
                            let value = emit_codegen_expr(
                                fb,
                                arg,
                                local_names,
                                local_values,
                                ctx,
                                borrowed_arg,
                                jit_module,
                                func_imports,
                            );
                            arg_values.push(value);
                            borrowed_args.push(borrowed_arg);
                        }
                        let tuple_value =
                            emit_pack_current_values_tuple(fb, arg_values.as_slice(), ctx);
                        for (value, borrowed_arg) in
                            arg_values.into_iter().zip(borrowed_args.into_iter())
                        {
                            if !borrowed_arg {
                                fb.ins().call(decref_ref, &[thread_state_value, value]);
                            }
                        }
                        return tuple_value;
                    }
                    if helper_id == RuntimeHelperId::LoadDeletedName && args.len() == 2 {
                        if let Some(name) = codegen_expr_const_string(args[0], ctx.module_constants)
                        {
                            let name_obj = emit_owned_module_constant(
                                fb,
                                ctx.module_constants
                                    .require_unicode_constant_id(name.as_str()),
                                ctx,
                            );
                            let value_borrowed = codegen_expr_is_borrowable(
                                args[1],
                                local_names,
                                &ctx.stack_slots,
                                ctx.storage_layout.as_ref(),
                            );
                            let value_obj = emit_codegen_expr(
                                fb,
                                args[1],
                                local_names,
                                local_values,
                                ctx,
                                value_borrowed,
                                jit_module,
                                func_imports,
                            );
                            let value_is_deleted_sentinel = fb.ins().icmp(
                                ir::condcodes::IntCC::Equal,
                                value_obj,
                                deleted_const,
                            );
                            let null_ptr = fb.ins().iconst(ptr_ty, 0);
                            let value_is_null =
                                fb.ins()
                                    .icmp(ir::condcodes::IntCC::Equal, value_obj, null_ptr);
                            let value_is_deleted =
                                fb.ins().bor(value_is_deleted_sentinel, value_is_null);
                            let deleted_block = fb.create_block();
                            let value_ok_block = fb.create_block();
                            fb.append_block_param(value_ok_block, ptr_ty);
                            fb.ins().brif(
                                value_is_deleted,
                                deleted_block,
                                &[],
                                value_ok_block,
                                &[ir::BlockArg::Value(value_obj)],
                            );

                            fb.switch_to_block(deleted_block);
                            fb.ins().call(raise_deleted_name_error_ref, &[name_obj]);
                            let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
                            fb.ins().call(decref_ref, &[thread_state_value, name_obj]);
                            if !value_borrowed {
                                emit_decref_if_not_null(
                                    fb,
                                    ptr_ty,
                                    decref_ref,
                                    thread_state_value,
                                    value_obj,
                                );
                            }
                            emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
                            fb.ins().jump(step_null_block, &step_null_block_args(ctx));

                            fb.switch_to_block(value_ok_block);
                            let value_obj = fb.block_params(value_ok_block)[0];
                            fb.ins().call(decref_ref, &[thread_state_value, name_obj]);
                            if value_borrowed {
                                fb.ins().call(incref_ref, &[value_obj]);
                            }
                            return value_obj;
                        }
                    }
                    if helper_id == RuntimeHelperId::CellRef && args.len() == 1 {
                        let cell_expr = &args[0];
                        let InstrCodegen::Load(cell_name) = cell_expr else {
                            panic!(
                                "cell_ref should lower to a located load arg, got {:?}",
                                cell_expr
                            );
                        };
                        if cell_name.name.cell_location().is_some() {
                            assert!(!borrowed, "cell_ref should produce an owned cell object");
                            return emit_raw_cell_object_for_name(
                                fb,
                                &cell_name.name,
                                local_names,
                                local_values,
                                ctx,
                            );
                        }
                        panic!(
                            "cell_ref should target a cell-backed name, got {} at {:?}",
                            cell_name.name.id, cell_name.name.location
                        );
                    }
                }
            }

            if keywords.is_empty() {
                let site_instr_id = call.semantic_instr_id();
                let counter_id = ctx.call_target_counter_ids.get(&site_instr_id).copied();
                let direct_hit_counter_id =
                    ctx.call_direct_hit_counter_ids.get(&site_instr_id).copied();
                let direct_fallback_counter_id = ctx
                    .call_direct_fallback_counter_ids
                    .get(&site_instr_id)
                    .copied();
                let direct_method_specializations =
                    direct_method_specializations_for_call_site(call, ctx);
                if !direct_method_specializations.is_empty() {
                    let InstrCodegen::GetAttr(getattr) = call.func.as_ref() else {
                        unreachable!("direct method specializations require GetAttr call target");
                    };
                    let receiver_is_borrowed = codegen_expr_is_borrowable(
                        getattr.value.as_ref(),
                        local_names,
                        &ctx.stack_slots,
                        ctx.storage_layout.as_ref(),
                    );
                    let receiver = emit_codegen_expr(
                        fb,
                        getattr.value.as_ref(),
                        local_names,
                        local_values,
                        ctx,
                        receiver_is_borrowed,
                        jit_module,
                        func_imports,
                    );
                    let result_block = fb.create_block();
                    fb.append_block_param(result_block, ptr_ty);
                    let generic_block = fb.create_block();
                    for (index, specialization) in direct_method_specializations.iter().enumerate()
                    {
                        let direct_block = fb.create_block();
                        let miss_block = if index + 1 == direct_method_specializations.len() {
                            generic_block
                        } else {
                            fb.create_block()
                        };
                        let expected_type =
                            fb.ins().iconst(ptr_ty, specialization.owner_type as i64);
                        let expected_version = fb
                            .ins()
                            .iconst(ctx.consts.i64_ty, specialization.type_version as i64);
                        let guard_inst = fb.ins().call(
                            ctx.guard_method_type_version_ref,
                            &[receiver, expected_type, expected_version],
                        );
                        let guard_result =
                            emit_checked_i32_result(fb, fb.inst_results(guard_inst)[0], ctx);
                        let is_match =
                            fb.ins()
                                .icmp_imm(ir::condcodes::IntCC::NotEqual, guard_result, 0);
                        fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

                        fb.switch_to_block(direct_block);
                        let target_function =
                            direct_call_target_function(ctx, specialization.function_id)
                                .expect("direct method specialization target should exist");
                        if let Some(counter_id) = counter_id {
                            let callee_id = fb.ins().iconst(
                                ctx.consts.i64_ty,
                                specialization.function_id.packed() as i64,
                            );
                            emit_record_call_target_sample(fb, counter_id, callee_id, ctx);
                        }
                        if let Some(counter_id) = direct_hit_counter_id {
                            let _ = emit_increment_counter(fb, counter_id, ctx);
                        }
                        let mut provided_arg_values = Vec::with_capacity(args.len() + 1);
                        let mut provided_arg_borrowed = Vec::with_capacity(args.len() + 1);
                        provided_arg_values.push(receiver);
                        provided_arg_borrowed.push(receiver_is_borrowed);
                        for arg in args.as_slice() {
                            let borrowed_arg = codegen_expr_is_borrowable(
                                arg,
                                local_names,
                                &ctx.stack_slots,
                                ctx.storage_layout.as_ref(),
                            );
                            provided_arg_borrowed.push(borrowed_arg);
                            provided_arg_values.push(emit_codegen_expr(
                                fb,
                                arg,
                                local_names,
                                local_values,
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
                        let callable = fb
                            .ins()
                            .iconst(ptr_ty, specialization.descriptor_function as i64);
                        let direct_result = emit_direct_call_resolved_with_arg_values(
                            fb,
                            callable,
                            true,
                            arg_values,
                            arg_borrowed,
                            target_function,
                            ctx,
                            jit_module,
                        );
                        fb.ins()
                            .jump(result_block, &[ir::BlockArg::Value(direct_result)]);
                        if index + 1 != direct_method_specializations.len() {
                            fb.switch_to_block(miss_block);
                        }
                    }

                    fb.switch_to_block(generic_block);
                    let attr_is_borrowed = codegen_expr_is_borrowable(
                        getattr.attr.as_ref(),
                        local_names,
                        &ctx.stack_slots,
                        ctx.storage_layout.as_ref(),
                    );
                    let attr = emit_codegen_expr(
                        fb,
                        getattr.attr.as_ref(),
                        local_names,
                        local_values,
                        ctx,
                        attr_is_borrowed,
                        jit_module,
                        func_imports,
                    );
                    let getattr_inst = fb.ins().call(ctx.pyobject_getattr_ref, &[receiver, attr]);
                    let mut owned_inputs = Vec::with_capacity(2);
                    if !attr_is_borrowed {
                        owned_inputs.push(attr);
                    }
                    if !receiver_is_borrowed {
                        owned_inputs.push(receiver);
                    }
                    let callable = emit_decref_owned_inputs_after_nullable_result(
                        fb,
                        ctx,
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
                        ctx.consts.step_null_block,
                        &step_null_block_args(ctx),
                        callable_ok_block,
                        &[ir::BlockArg::Value(callable)],
                    );
                    fb.switch_to_block(callable_ok_block);
                    let callable = fb.block_params(callable_ok_block)[0];
                    if let Some(counter_id) = counter_id {
                        let callee_id = emit_callee_function_id_checked(fb, callable, ctx);
                        emit_record_call_target_sample(fb, counter_id, callee_id, ctx);
                    }
                    if let Some(counter_id) = direct_fallback_counter_id {
                        let _ = emit_increment_counter(fb, counter_id, ctx);
                    }
                    let generic_result = emit_positional_vectorcall(
                        fb,
                        callable,
                        false,
                        args.as_slice(),
                        local_names,
                        local_values,
                        ctx,
                        jit_module,
                        func_imports,
                    );
                    fb.ins()
                        .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
                    fb.switch_to_block(result_block);
                    return fb.block_params(result_block)[0];
                }
                let callable_is_borrowed = codegen_expr_is_borrowable(
                    call.func.as_ref(),
                    local_names,
                    &ctx.stack_slots,
                    ctx.storage_layout.as_ref(),
                );
                let callable = emit_codegen_expr(
                    fb,
                    call.func.as_ref(),
                    local_names,
                    local_values,
                    ctx,
                    callable_is_borrowed,
                    jit_module,
                    func_imports,
                );
                let constructor_specializations =
                    direct_constructor_specializations_for_call_site(call, ctx);
                let direct_specializations = ctx
                    .call_target_specializations
                    .get(&site_instr_id)
                    .map(|targets| {
                        targets
                            .iter()
                            .copied()
                            .filter_map(|function_id| {
                                let Some(target_function) =
                                    direct_call_target_function(ctx, function_id)
                                else {
                                    ctx.direct_edge_stats
                                        .record_profiled_missing_target_candidate();
                                    return None;
                                };
                                let arg_plan = match validate_direct_call_compatibility(
                                    target_function,
                                    ctx.direct_call_functions,
                                    args.len(),
                                    0,
                                    has_unpack,
                                    !call.keywords.is_empty(),
                                ) {
                                    Ok(arg_plan) => arg_plan,
                                    Err(incompatibility) => {
                                        record_profiled_direct_call_incompatibility(
                                            ctx.direct_edge_stats,
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

                if counter_id.is_some()
                    || !constructor_specializations.is_empty()
                    || !direct_specializations.is_empty()
                {
                    let callee_id = emit_callee_function_id_checked(fb, callable, ctx);
                    if let Some(counter_id) = counter_id {
                        emit_record_call_target_sample(fb, counter_id, callee_id, ctx);
                    }
                    if !constructor_specializations.is_empty() || !direct_specializations.is_empty()
                    {
                        let result_block = fb.create_block();
                        fb.append_block_param(result_block, ptr_ty);
                        let generic_block = fb.create_block();
                        let mut direct_chain_start = None;
                        if !constructor_specializations.is_empty() {
                            let mut next_miss_block = fb.create_block();
                            for (index, specialization) in
                                constructor_specializations.iter().enumerate()
                            {
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
                                let expected_type =
                                    fb.ins().iconst(ptr_ty, specialization.owner_type as i64);
                                let is_exact_type = fb.ins().icmp(
                                    ir::condcodes::IntCC::Equal,
                                    callable,
                                    expected_type,
                                );
                                fb.ins().brif(
                                    is_exact_type,
                                    type_match_block,
                                    &[],
                                    miss_block,
                                    &[],
                                );

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
                                    direct_call_target_function(ctx, specialization.function_id)
                                        .expect(
                                            "direct constructor specialization target should exist",
                                        );
                                if let Some(counter_id) = direct_hit_counter_id {
                                    let _ = emit_increment_counter(fb, counter_id, ctx);
                                }
                                let mut arg_values = Vec::with_capacity(args.len());
                                let mut arg_borrowed = Vec::with_capacity(args.len());
                                for arg in args.as_slice() {
                                    let borrowed_arg = codegen_expr_is_borrowable(
                                        arg,
                                        local_names,
                                        &ctx.stack_slots,
                                        ctx.storage_layout.as_ref(),
                                    );
                                    arg_borrowed.push(borrowed_arg);
                                    arg_values.push(emit_codegen_expr(
                                        fb,
                                        arg,
                                        local_names,
                                        local_values,
                                        ctx,
                                        borrowed_arg,
                                        jit_module,
                                        func_imports,
                                    ));
                                }
                                let direct_result =
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
                            for (index, specialization) in direct_specializations.iter().enumerate()
                            {
                                let direct_block = fb.create_block();
                                let miss_block = if index + 1 == direct_specializations.len() {
                                    generic_block
                                } else {
                                    fb.create_block()
                                };
                                let is_match = fb.ins().icmp_imm(
                                    ir::condcodes::IntCC::Equal,
                                    callee_id,
                                    specialization.function_id.packed() as i64,
                                );
                                fb.ins().brif(is_match, direct_block, &[], miss_block, &[]);

                                fb.switch_to_block(direct_block);
                                let target_function =
                                    direct_call_target_function(ctx, specialization.function_id)
                                        .expect("direct specialization target should exist");
                                if let Some(counter_id) = direct_hit_counter_id {
                                    let _ = emit_increment_counter(fb, counter_id, ctx);
                                }
                                let direct_result = emit_direct_call_resolved_with_arg_plan(
                                    fb,
                                    callable,
                                    callable_is_borrowed,
                                    args.as_slice(),
                                    &specialization.arg_plan,
                                    target_function,
                                    local_names,
                                    local_values,
                                    ctx,
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

                        fb.switch_to_block(generic_block);
                        ctx.direct_edge_stats
                            .record_guarded_generic_fallback_block();
                        if let Some(counter_id) = direct_fallback_counter_id {
                            let _ = emit_increment_counter(fb, counter_id, ctx);
                        }
                        let generic_result = emit_positional_vectorcall(
                            fb,
                            callable,
                            callable_is_borrowed,
                            args.as_slice(),
                            local_names,
                            local_values,
                            ctx,
                            jit_module,
                            func_imports,
                        );
                        fb.ins()
                            .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
                        fb.switch_to_block(result_block);
                        return fb.block_params(result_block)[0];
                    }
                }

                return emit_positional_vectorcall(
                    fb,
                    callable,
                    callable_is_borrowed,
                    args.as_slice(),
                    local_names,
                    local_values,
                    ctx,
                    jit_module,
                    func_imports,
                );
            }

            let callable_is_borrowed = codegen_expr_is_borrowable(
                call.func.as_ref(),
                local_names,
                &ctx.stack_slots,
                ctx.storage_layout.as_ref(),
            );
            let callable = emit_codegen_expr(
                fb,
                call.func.as_ref(),
                local_names,
                local_values,
                ctx,
                callable_is_borrowed,
                jit_module,
                func_imports,
            );

            let tuple_len = fb.ins().iconst(i64_ty, args.len() as i64);
            let tuple_inst = fb.ins().call(tuple_new_ref, &[tuple_len]);
            let call_args_tuple = fb.inst_results(tuple_inst)[0];
            let tuple_is_null =
                fb.ins()
                    .icmp(ir::condcodes::IntCC::Equal, call_args_tuple, null_ptr);
            let tuple_ok_block = fb.create_block();
            fb.append_block_param(tuple_ok_block, ptr_ty);
            fb.ins().brif(
                tuple_is_null,
                step_null_block,
                &step_null_block_args(ctx),
                tuple_ok_block,
                &[ir::BlockArg::Value(call_args_tuple)],
            );
            fb.switch_to_block(tuple_ok_block);
            let call_args_tuple = fb.block_params(tuple_ok_block)[0];
            let mut tuple_items: Vec<(ir::Value, bool)> = Vec::with_capacity(args.len());
            for arg in args {
                let borrowed_arg = codegen_expr_is_borrowable(
                    arg,
                    local_names,
                    &ctx.stack_slots,
                    ctx.storage_layout.as_ref(),
                );
                let value = emit_codegen_expr(
                    fb,
                    arg,
                    local_names,
                    local_values,
                    ctx,
                    borrowed_arg,
                    jit_module,
                    func_imports,
                );
                tuple_items.push((value, borrowed_arg));
            }
            for (index, (value, borrowed_arg)) in tuple_items.iter().enumerate() {
                if *borrowed_arg {
                    fb.ins().call(incref_ref, &[*value]);
                }
                let item_index = fb.ins().iconst(i64_ty, index as i64);
                let set_inst = fb
                    .ins()
                    .call(tuple_set_item_ref, &[call_args_tuple, item_index, *value]);
                let set_result = fb.inst_results(set_inst)[0];
                let set_failed = fb
                    .ins()
                    .icmp_imm(ir::condcodes::IntCC::NotEqual, set_result, 0);
                let set_ok_block = fb.create_block();
                let set_fail_block = fb.create_block();
                fb.append_block_param(set_fail_block, ptr_ty);
                fb.ins().brif(
                    set_failed,
                    set_fail_block,
                    &[ir::BlockArg::Value(call_args_tuple)],
                    set_ok_block,
                    &[],
                );
                fb.switch_to_block(set_fail_block);
                let failed_tuple = fb.block_params(set_fail_block)[0];
                let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
                fb.ins()
                    .call(decref_ref, &[thread_state_value, failed_tuple]);
                emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
                fb.ins().jump(step_null_block, &step_null_block_args(ctx));
                fb.switch_to_block(set_ok_block);
            }
            let mut call_kwargs_obj = None;
            let call_inst = if keywords.is_empty() {
                fb.ins()
                    .call(py_call_object_ref, &[callable, call_args_tuple])
            } else {
                let dict_name_obj = emit_owned_module_constant(
                    fb,
                    ctx.module_constants.require_unicode_constant_id("dict"),
                    ctx,
                );
                let dict_callable_inst = fb.ins().call(ctx.load_runtime_obj_ref, &[dict_name_obj]);
                let dict_callable = emit_decref_owned_input_after_nullable_result(
                    fb,
                    ctx,
                    fb.inst_results(dict_callable_inst)[0],
                    dict_name_obj,
                );
                let dict_callable_is_null =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, dict_callable, null_ptr);
                let dict_callable_ok = fb.create_block();
                fb.append_block_param(dict_callable_ok, ptr_ty);
                fb.ins().brif(
                    dict_callable_is_null,
                    step_null_block,
                    &step_null_block_args(ctx),
                    dict_callable_ok,
                    &[ir::BlockArg::Value(dict_callable)],
                );
                fb.switch_to_block(dict_callable_ok);
                let dict_callable = fb.block_params(dict_callable_ok)[0];

                let empty_tuple_len = fb.ins().iconst(i64_ty, 0);
                let empty_tuple_inst = fb.ins().call(tuple_new_ref, &[empty_tuple_len]);
                let empty_tuple = fb.inst_results(empty_tuple_inst)[0];
                let empty_tuple_is_null =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, empty_tuple, null_ptr);
                let empty_tuple_ok = fb.create_block();
                fb.append_block_param(empty_tuple_ok, ptr_ty);
                fb.ins().brif(
                    empty_tuple_is_null,
                    step_null_block,
                    &step_null_block_args(ctx),
                    empty_tuple_ok,
                    &[ir::BlockArg::Value(empty_tuple)],
                );
                fb.switch_to_block(empty_tuple_ok);
                let empty_tuple = fb.block_params(empty_tuple_ok)[0];

                let kwargs_inst = fb
                    .ins()
                    .call(py_call_object_ref, &[dict_callable, empty_tuple]);
                let kwargs_obj = emit_decref_owned_inputs_after_nullable_result(
                    fb,
                    ctx,
                    fb.inst_results(kwargs_inst)[0],
                    &[empty_tuple, dict_callable],
                );
                let kwargs_is_null =
                    fb.ins()
                        .icmp(ir::condcodes::IntCC::Equal, kwargs_obj, null_ptr);
                let kwargs_ok = fb.create_block();
                fb.append_block_param(kwargs_ok, ptr_ty);
                fb.ins().brif(
                    kwargs_is_null,
                    step_null_block,
                    &step_null_block_args(ctx),
                    kwargs_ok,
                    &[ir::BlockArg::Value(kwargs_obj)],
                );
                fb.switch_to_block(kwargs_ok);
                let kwargs_obj = fb.block_params(kwargs_ok)[0];

                for (name, value_expr) in keywords {
                    let key_obj = emit_owned_module_constant(
                        fb,
                        ctx.module_constants.require_unicode_constant_id(name),
                        ctx,
                    );

                    let value_borrowed = codegen_expr_is_borrowable(
                        value_expr,
                        local_names,
                        &ctx.stack_slots,
                        ctx.storage_layout.as_ref(),
                    );
                    let value_obj = emit_codegen_expr(
                        fb,
                        value_expr,
                        local_names,
                        local_values,
                        ctx,
                        value_borrowed,
                        jit_module,
                        func_imports,
                    );
                    let set_inst = fb
                        .ins()
                        .call(pyobject_setitem_ref, &[kwargs_obj, key_obj, value_obj]);
                    fb.ins().call(decref_ref, &[thread_state_value, key_obj]);
                    if !value_borrowed {
                        fb.ins().call(decref_ref, &[thread_state_value, value_obj]);
                    }
                    let set_value = fb.inst_results(set_inst)[0];
                    let set_failed =
                        fb.ins()
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
                    let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
                    fb.ins()
                        .call(decref_ref, &[thread_state_value, failed_kwargs]);
                    fb.ins()
                        .call(decref_ref, &[thread_state_value, call_args_tuple]);
                    if !callable_is_borrowed {
                        fb.ins().call(decref_ref, &[thread_state_value, callable]);
                    }
                    emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
                    fb.ins().jump(step_null_block, &step_null_block_args(ctx));
                    fb.switch_to_block(set_ok);
                    fb.ins().call(decref_ref, &[thread_state_value, set_value]);
                }

                let call_inst = fb.ins().call(
                    py_call_with_kw_ref,
                    &[callable, call_args_tuple, kwargs_obj],
                );
                call_kwargs_obj = Some(kwargs_obj);
                call_inst
            };
            let call_value = fb.inst_results(call_inst)[0];
            let call_is_null = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
            let call_fail_block = fb.create_block();
            let call_ok_block = fb.create_block();
            fb.append_block_param(call_ok_block, ptr_ty);
            fb.ins().brif(
                call_is_null,
                call_fail_block,
                &[],
                call_ok_block,
                &[ir::BlockArg::Value(call_value)],
            );
            fb.switch_to_block(call_fail_block);
            let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
            if let Some(kwargs_obj) = call_kwargs_obj {
                fb.ins().call(decref_ref, &[thread_state_value, kwargs_obj]);
            }
            fb.ins()
                .call(decref_ref, &[thread_state_value, call_args_tuple]);
            if !callable_is_borrowed {
                fb.ins().call(decref_ref, &[thread_state_value, callable]);
            }
            emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
            fb.ins().jump(step_null_block, &step_null_block_args(ctx));

            fb.switch_to_block(call_ok_block);
            if let Some(kwargs_obj) = call_kwargs_obj {
                fb.ins().call(decref_ref, &[thread_state_value, kwargs_obj]);
            }
            fb.ins()
                .call(decref_ref, &[thread_state_value, call_args_tuple]);
            if !callable_is_borrowed {
                fb.ins().call(decref_ref, &[thread_state_value, callable]);
            }
            fb.block_params(call_ok_block)[0]
        }
    }
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

fn emit_prepare_target_args_codegen_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    target_params: &[String],
    full_target_params: Option<&[String]>,
    explicit_args: Option<&[BlockArg]>,
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
    _jit_module: &mut JITModule,
    _func_imports: &mut FuncBuildImports<'_>,
) -> Option<Vec<ir::BlockArg>> {
    let mut args = Vec::with_capacity(target_params.len());
    let mut forwarded_local_indices = HashMap::new();
    let explicit_arg_offsets = match (full_target_params, explicit_args) {
        (Some(full_target_params), Some(explicit_args)) => {
            let explicit_start = full_target_params.len().saturating_sub(explicit_args.len());
            Some(
                full_target_params[explicit_start..]
                    .iter()
                    .enumerate()
                    .map(|(offset, name)| (name.as_str(), offset))
                    .collect::<HashMap<_, _>>(),
            )
        }
        _ => None,
    };
    for name in target_params {
        if let Some(explicit_arg) = explicit_args.and_then(|args| {
            explicit_arg_offsets
                .as_ref()
                .and_then(|offsets| offsets.get(name.as_str()).copied())
                .and_then(|offset| args.get(offset))
        }) {
            let value = match explicit_arg {
                BlockArg::Name(source_name) => {
                    if let Some(value_index) = local_env.entry_index_for_block_arg_name(source_name)
                    {
                        let entry = &local_env.entries[value_index];
                        let forwarded_count =
                            forwarded_local_indices.entry(value_index).or_insert(0usize);
                        if local_ref_kind_needs_incref_for_forward(entry.ref_kind, *forwarded_count)
                        {
                            fb.ins().call(ctx.incref_ref, &[entry.value]);
                        }
                        *forwarded_count += 1;
                        entry.value
                    } else if let Some(value) = load_stack_slot_value(
                        fb,
                        &ctx.stack_slots,
                        source_name,
                        ctx.consts.ptr_ty,
                        false,
                        ctx.incref_ref,
                    ) {
                        value
                    } else {
                        return None;
                    }
                }
                BlockArg::None => {
                    fb.ins().call(ctx.incref_ref, &[ctx.consts.none_const]);
                    ctx.consts.none_const
                }
                BlockArg::CurrentException => return None,
                BlockArg::AbruptKind(kind) => emit_owned_module_constant(
                    fb,
                    ctx.module_constants
                        .require_int_constant_id(abrupt_kind_tag(*kind)),
                    ctx,
                ),
            };
            args.push(ir::BlockArg::Value(value));
            continue;
        }
        if let Some(value_index) = local_env.entry_index_for_block_arg_name(name) {
            let entry = &local_env.entries[value_index];
            let forwarded_count = forwarded_local_indices.entry(value_index).or_insert(0usize);
            if local_ref_kind_needs_incref_for_forward(entry.ref_kind, *forwarded_count) {
                fb.ins().call(ctx.incref_ref, &[entry.value]);
            }
            *forwarded_count += 1;
            args.push(ir::BlockArg::Value(entry.value));
            continue;
        }
        if let Some(value) = load_stack_slot_value(
            fb,
            &ctx.stack_slots,
            name,
            ctx.consts.ptr_ty,
            false,
            ctx.incref_ref,
        ) {
            args.push(ir::BlockArg::Value(value));
            continue;
        }
        fb.ins().call(ctx.incref_ref, &[ctx.consts.none_const]);
        args.push(ir::BlockArg::Value(ctx.consts.none_const));
    }
    Some(args)
}

fn emit_explicit_target_slot_writes_codegen_from_local_env(
    fb: &mut FunctionBuilder<'_>,
    full_target_params: &[String],
    runtime_target_params: &[String],
    explicit_args: &[BlockArg],
    local_env: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
    _jit_module: &mut JITModule,
    _func_imports: &mut FuncBuildImports<'_>,
) -> Option<()> {
    let explicit_start = full_target_params.len().saturating_sub(explicit_args.len());
    for (offset, arg) in explicit_args.iter().enumerate() {
        let target_name = &full_target_params[explicit_start + offset];
        if runtime_target_params.iter().any(|name| name == target_name) {
            continue;
        }
        let (value, owned_value) = match arg {
            BlockArg::Name(source_name) => {
                if let Some(index) = local_env.entry_index_for_block_arg_name(source_name) {
                    (local_env.entries[index].value, false)
                } else if let Some(value) = load_stack_slot_value(
                    fb,
                    &ctx.stack_slots,
                    source_name,
                    ctx.consts.ptr_ty,
                    true,
                    ctx.incref_ref,
                ) {
                    (value, false)
                } else {
                    return None;
                }
            }
            BlockArg::None => (ctx.consts.none_const, false),
            BlockArg::CurrentException => return None,
            BlockArg::AbruptKind(kind) => (
                emit_owned_module_constant(
                    fb,
                    ctx.module_constants
                        .require_int_constant_id(abrupt_kind_tag(*kind)),
                    ctx,
                ),
                true,
            ),
        };
        ctx.stack_slots
            .replace_cloned_value(
                fb,
                target_name,
                value,
                ctx.consts.ptr_ty,
                ctx.consts.thread_state_value,
                ctx.incref_ref,
                ctx.decref_ref,
            )
            .expect("explicit edge slot target missing from stack slots");
        if owned_value {
            fb.ins()
                .call(ctx.decref_ref, &[ctx.consts.thread_state_value, value]);
        }
    }
    Some(())
}

fn emit_decref_unforwarded_local_env(
    fb: &mut FunctionBuilder<'_>,
    local_env: &LocalEnv,
    target_params: &[String],
    thread_state_value: ir::Value,
    decref_ref: ir::FuncRef,
) {
    let forwarded_local_indices = target_params
        .iter()
        .filter_map(|name| local_env.entry_index_for_block_arg_name(name))
        .collect::<HashSet<_>>();
    for (index, entry) in local_env.entries.iter().enumerate() {
        if forwarded_local_indices.contains(&index) {
            continue;
        }
        if transient_local_needs_decref(entry.ref_kind) {
            fb.ins()
                .call(decref_ref, &[thread_state_value, entry.value]);
        }
    }
}

fn emit_exception_dispatch_slot_writes(
    fb: &mut FunctionBuilder<'_>,
    slot_writes: &[(String, BlockArg)],
    dispatch_exc: ir::Value,
    stack_slots: &StackSlots,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    none_const: ir::Value,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
) -> Result<(), String> {
    for (target_name, source) in slot_writes {
        let value = match source {
            BlockArg::Name(source_name) => load_stack_slot_value(
                fb,
                stack_slots,
                source_name,
                ptr_ty,
                true,
                incref_ref,
            )
            .ok_or_else(|| {
                format!(
                    "missing exception dispatch slot source {source_name} for target {target_name}"
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
    target_params: &[String],
    ctx: &JitEmitCtx<'_>,
) {
    let Some(exception_name) = current_exception_name else {
        return;
    };
    if target_params.iter().any(|name| name == exception_name) {
        return;
    }
    emit_pop_handled_exception(fb, exception_name, ctx);
}

fn emit_planned_stack_slot_releases_for_reason(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    reason: &RefcountReleaseReason,
    emit_ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    emit_planned_stack_slot_releases_for_reason_from_parts(
        fb,
        source_label,
        reason,
        emit_ctx.refcount_plan,
        &emit_ctx.stack_slots,
        emit_ctx.consts.ptr_ty,
        emit_ctx.consts.thread_state_value,
        emit_ctx.decref_ref,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_planned_stack_slot_releases_for_reason_from_parts(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    reason: &RefcountReleaseReason,
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
        let is_true = fb
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, value, ctx.consts.true_const);
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
    let error_value = emit_take_error_before_local_null_cleanup(fb, ctx);
    emit_release_pyobject_if_owned(fb, value, py_facts, owned, ctx);
    emit_restore_error_after_local_null_cleanup(fb, ctx, error_value);
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
    let facts = emit_ctx
        .value_facts_for_expr(expr)
        .and_then(ValueFacts::as_pyobj)
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

fn emit_codegen_simple_call_with_local_env(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_blockpy::block_py::Call<InstrCodegen>,
    local_env: &mut LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Option<ir::Value> {
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
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
        let iterator_is_borrowed = codegen_expr_is_borrowable_from_local_env(
            iterator_expr,
            local_env,
            &emit_ctx.stack_slots,
            emit_ctx.storage_layout.as_ref(),
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
        let callable_is_borrowed = codegen_expr_is_borrowable_from_local_env(
            call.func.as_ref(),
            local_env,
            &emit_ctx.stack_slots,
            emit_ctx.storage_layout.as_ref(),
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
        if helper_id == RuntimeHelperId::TupleValues {
            let mut arg_values: Vec<ir::Value> = Vec::with_capacity(simple_args.len());
            let mut borrowed_args: Vec<bool> = Vec::with_capacity(simple_args.len());
            for arg in &simple_args {
                let borrowed_arg = codegen_expr_is_borrowable_from_local_env(
                    arg,
                    local_env,
                    &emit_ctx.stack_slots,
                    emit_ctx.storage_layout.as_ref(),
                );
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
            return Some(tuple_value);
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
            let value_borrowed = codegen_expr_is_borrowable_from_local_env(
                simple_args[1],
                local_env,
                &emit_ctx.stack_slots,
                emit_ctx.storage_layout.as_ref(),
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
            let value_is_deleted_sentinel = fb.ins().icmp(
                ir::condcodes::IntCC::Equal,
                value_obj,
                emit_ctx.consts.deleted_const,
            );
            let value_is_null = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, value_obj, null_ptr);
            let value_is_deleted = fb.ins().bor(value_is_deleted_sentinel, value_is_null);
            let deleted_block = fb.create_block();
            let value_ok_block = fb.create_block();
            fb.append_block_param(value_ok_block, ptr_ty);
            fb.ins().brif(
                value_is_deleted,
                deleted_block,
                &[],
                value_ok_block,
                &[ir::BlockArg::Value(value_obj)],
            );

            fb.switch_to_block(deleted_block);
            fb.ins()
                .call(emit_ctx.raise_deleted_name_error_ref, &[name_obj]);
            let error_value = emit_take_error_before_local_null_cleanup(fb, emit_ctx);
            fb.ins().call(
                emit_ctx.decref_ref,
                &[emit_ctx.consts.thread_state_value, name_obj],
            );
            if !value_borrowed {
                emit_decref_if_not_null(
                    fb,
                    emit_ctx.consts.ptr_ty,
                    emit_ctx.decref_ref,
                    emit_ctx.consts.thread_state_value,
                    value_obj,
                );
            }
            emit_restore_error_after_local_null_cleanup(fb, emit_ctx, error_value);
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
        let site_instr_id = call.semantic_instr_id();
        let call_target_counter = emit_ctx
            .call_target_counter_ids
            .get(&site_instr_id)
            .copied();
        let direct_hit_counter_id = emit_ctx
            .call_direct_hit_counter_ids
            .get(&site_instr_id)
            .copied();
        let direct_fallback_counter_id = emit_ctx
            .call_direct_fallback_counter_ids
            .get(&site_instr_id)
            .copied();
        let constructor_specializations =
            direct_constructor_specializations_for_call_site(call, emit_ctx);
        let direct_specializations = emit_ctx
            .call_target_specializations
            .get(&site_instr_id)
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
            let receiver_is_borrowed = codegen_expr_is_borrowable_from_local_env(
                getattr.value.as_ref(),
                local_env,
                &emit_ctx.stack_slots,
                emit_ctx.storage_layout.as_ref(),
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
            for (index, specialization) in direct_method_specializations.iter().enumerate() {
                let direct_block = fb.create_block();
                let miss_block = if index + 1 == direct_method_specializations.len() {
                    generic_block
                } else {
                    fb.create_block()
                };
                let expected_type = fb.ins().iconst(ptr_ty, specialization.owner_type as i64);
                let expected_version = fb
                    .ins()
                    .iconst(emit_ctx.consts.i64_ty, specialization.type_version as i64);
                let guard_inst = fb.ins().call(
                    emit_ctx.guard_method_type_version_ref,
                    &[receiver, expected_type, expected_version],
                );
                let guard_result =
                    emit_checked_i32_result(fb, fb.inst_results(guard_inst)[0], emit_ctx);
                let is_match = fb
                    .ins()
                    .icmp_imm(ir::condcodes::IntCC::NotEqual, guard_result, 0);
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

            fb.switch_to_block(generic_block);
            let attr_is_borrowed = codegen_expr_is_borrowable_from_local_env(
                getattr.attr.as_ref(),
                local_env,
                &emit_ctx.stack_slots,
                emit_ctx.storage_layout.as_ref(),
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
                let callee_id = emit_callee_function_id_checked(fb, callable, emit_ctx);
                emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
            }
            if let Some(counter_id) = direct_fallback_counter_id {
                let _ = emit_increment_counter(fb, counter_id, emit_ctx);
            }
            let generic_result = emit_positional_vectorcall_with_local_env(
                fb,
                callable,
                false,
                simple_args.as_slice(),
                local_env,
                emit_ctx,
                jit_module,
                func_imports,
            );
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
            fb.switch_to_block(result_block);
            return Some(fb.block_params(result_block)[0]);
        }
        let callable_is_borrowed = codegen_expr_is_borrowable_from_local_env(
            call.func.as_ref(),
            local_env,
            &emit_ctx.stack_slots,
            emit_ctx.storage_layout.as_ref(),
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
        let callee_id =
            should_emit_callee_id.then(|| emit_callee_function_id_checked(fb, callable, emit_ctx));
        if let Some(counter_id) = call_target_counter {
            let callee_id = callee_id.expect("callee id should exist for call target counter");
            emit_record_call_target_sample(fb, counter_id, callee_id, emit_ctx);
        }
        if !constructor_specializations.is_empty() || !direct_specializations.is_empty() {
            let result_block = fb.create_block();
            fb.append_block_param(result_block, ptr_ty);
            let generic_block = fb.create_block();
            let mut direct_chain_start = None;
            if !constructor_specializations.is_empty() {
                let mut next_miss_block = fb.create_block();
                for (index, specialization) in constructor_specializations.iter().enumerate() {
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
                    let expected_type = fb.ins().iconst(ptr_ty, specialization.owner_type as i64);
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
                for (index, specialization) in direct_specializations.iter().enumerate() {
                    let direct_block = fb.create_block();
                    let miss_block = if index + 1 == direct_specializations.len() {
                        generic_block
                    } else {
                        fb.create_block()
                    };
                    let is_match = fb.ins().icmp_imm(
                        ir::condcodes::IntCC::Equal,
                        callee_id,
                        specialization.function_id.packed() as i64,
                    );
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

            fb.switch_to_block(generic_block);
            emit_ctx
                .direct_edge_stats
                .record_guarded_generic_fallback_block();
            if let Some(counter_id) = direct_fallback_counter_id {
                let _ = emit_increment_counter(fb, counter_id, emit_ctx);
            }
            let generic_result = emit_positional_vectorcall_with_local_env(
                fb,
                callable,
                callable_is_borrowed,
                simple_args.as_slice(),
                local_env,
                emit_ctx,
                jit_module,
                func_imports,
            );
            fb.ins()
                .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
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
        let callable_is_borrowed = codegen_expr_is_borrowable_from_local_env(
            call.func.as_ref(),
            local_env,
            &emit_ctx.stack_slots,
            emit_ctx.storage_layout.as_ref(),
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
        if let Some(value) = emit_codegen_non_local_name_load(
            fb,
            &op.name,
            op.semantic_instr_id(),
            emit_ctx,
            borrowed,
        ) {
            return value;
        }
        if let Some(location) = op.name.local_location() {
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
        if op.name.cell_location().is_some() {
            assert!(
                !borrowed,
                "cell-backed name loads must produce owned references"
            );
            let cell_obj =
                emit_raw_cell_object_for_name_with_local_env(fb, &op.name, local_env, emit_ctx);
            return emit_cell_value_load_from_raw_cell(fb, cell_obj, emit_ctx);
        }
    }
    if let InstrCodegen::IncrementCounter(op) = expr {
        assert!(
            !borrowed,
            "increment_counter must not request a borrowed result"
        );
        return emit_increment_counter(fb, op.counter_id, emit_ctx);
    }
    if let InstrCodegen::CalleeFunctionId(op) = expr {
        assert!(
            !borrowed,
            "callee_function_id must not request a borrowed result"
        );
        let callable_is_borrowed = codegen_expr_is_borrowable_from_local_env(
            op.value.as_ref(),
            local_env,
            &emit_ctx.stack_slots,
            emit_ctx.storage_layout.as_ref(),
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
        let callee_id = emit_callee_function_id_checked(fb, callable, emit_ctx);
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
    if matches!(expr, InstrCodegen::MakeFunction(_)) {
        panic!("MakeFunction should lower to a regular call before codegen");
    }
    if let InstrCodegen::Store(op) = expr {
        if let Some(location) = op.name.local_location() {
            let layout = emit_ctx
                .storage_layout
                .as_ref()
                .expect("Store local slot should have storage layout during codegen");
            let name = local_name_for_location(layout, location);
            let value = emit_codegen_expr_with_local_env(
                fb,
                &op.value,
                local_env,
                emit_ctx,
                false,
                jit_module,
                func_imports,
            );
            local_env.store_location(
                fb,
                location,
                name,
                value,
                &emit_ctx.stack_slots,
                emit_ctx.consts.ptr_ty,
                emit_ctx.consts.thread_state_value,
                emit_ctx.incref_ref,
                emit_ctx.decref_ref,
            );
            fb.ins()
                .call(emit_ctx.incref_ref, &[emit_ctx.consts.none_const]);
            return emit_ctx.consts.none_const;
        }
        let Some(location) = op.name.cell_location() else {
            panic!("Store should be resolved before codegen: {op:?}");
        };
        if location.is_owned() && matches!(op.value.as_ref(), InstrCodegen::MakeCell(_)) {
            let layout = emit_ctx
                .storage_layout
                .as_ref()
                .expect("Store owned cell slot should have storage layout during codegen");
            let closure_slot = layout.local_cell_slot(location.slot()).unwrap_or_else(|| {
                panic!(
                    "missing owned cell slot mapping for owned cell location {}",
                    location.slot()
                )
            });
            let value = emit_codegen_expr_with_local_env(
                fb,
                &op.value,
                local_env,
                emit_ctx,
                false,
                jit_module,
                func_imports,
            );
            local_env.store_name(
                fb,
                closure_slot.storage_name.as_str(),
                value,
                &emit_ctx.stack_slots,
                emit_ctx.consts.ptr_ty,
                emit_ctx.consts.thread_state_value,
                emit_ctx.incref_ref,
                emit_ctx.decref_ref,
            );
            fb.ins()
                .call(emit_ctx.incref_ref, &[emit_ctx.consts.none_const]);
            return emit_ctx.consts.none_const;
        }
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
        if let Some(location) = op.name.local_location() {
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
            fb.ins()
                .call(emit_ctx.incref_ref, &[emit_ctx.consts.none_const]);
            return emit_ctx.consts.none_const;
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
            if let Some(location) = op.name.local_location() {
                let layout = emit_ctx
                    .storage_layout
                    .as_ref()
                    .expect("Store local slot should have storage layout during codegen");
                let name = local_name_for_location(layout, location);
                let value = emit_codegen_expr_with_local_env(
                    fb,
                    &op.value,
                    local_env,
                    emit_ctx,
                    false,
                    jit_module,
                    func_imports,
                );
                local_env.store_location(
                    fb,
                    location,
                    name,
                    value,
                    &emit_ctx.stack_slots,
                    emit_ctx.consts.ptr_ty,
                    emit_ctx.consts.thread_state_value,
                    emit_ctx.incref_ref,
                    emit_ctx.decref_ref,
                );
                fb.ins()
                    .call(emit_ctx.incref_ref, &[emit_ctx.consts.none_const]);
                return emit_ctx.consts.none_const;
            }
        }
        InstrCodegen::Del(op) => {
            if let Some(location) = op.name.local_location() {
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
                fb.ins()
                    .call(emit_ctx.incref_ref, &[emit_ctx.consts.none_const]);
                return emit_ctx.consts.none_const;
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

fn local_failure_cleanup_emit_ctx<'mc>(
    fb: &mut FunctionBuilder<'_>,
    emit_ctx: &JitEmitCtx<'mc>,
    local_env: &LocalEnv,
    pre_cleanup_null_block: ir::Block,
    cleanup_null_block: ir::Block,
    pending_local_failure_cleanups: &mut Vec<PendingLocalFailureCleanup>,
) -> Option<JitEmitCtx<'mc>> {
    if emit_ctx.consts.step_null_block != pre_cleanup_null_block {
        return None;
    }
    let cleanup_values = local_env.local_only_cleanup_values();
    if cleanup_values.is_empty() {
        return None;
    }

    let cleanup_block = fb.create_block();
    for _ in &cleanup_values {
        fb.append_block_param(cleanup_block, emit_ctx.consts.ptr_ty);
    }
    pending_local_failure_cleanups.push(PendingLocalFailureCleanup {
        block: cleanup_block,
        cleanup_null_block,
    });
    Some(emit_ctx.with_step_null_target(cleanup_block, cleanup_values))
}

fn emit_codegen_ops(
    fb: &mut FunctionBuilder<'_>,
    ops: &[InstrCodegen],
    local_env: &mut LocalEnv,
    _stack_slots: &StackSlots,
    emit_ctx: &JitEmitCtx<'_>,
    pre_cleanup_null_block: ir::Block,
    cleanup_null_block: ir::Block,
    pending_local_failure_cleanups: &mut Vec<PendingLocalFailureCleanup>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    for expr in ops {
        let stmt_emit_ctx = local_failure_cleanup_emit_ctx(
            fb,
            emit_ctx,
            local_env,
            pre_cleanup_null_block,
            cleanup_null_block,
            pending_local_failure_cleanups,
        );
        let stmt_emit_ctx = stmt_emit_ctx.as_ref().unwrap_or(emit_ctx);
        let value = emit_codegen_stmt_with_local_env(
            fb,
            expr,
            local_env,
            stmt_emit_ctx,
            jit_module,
            func_imports,
        );
        fb.ins().call(
            emit_ctx.decref_ref,
            &[emit_ctx.consts.thread_state_value, value],
        );
    }
    Ok(())
}

fn emit_codegen_if_target_arm(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    arm_name: &str,
    branch_block: ir::Block,
    target_label: BlockLabel,
    release_reason: RefcountReleaseReason,
    current_exception_name: Option<&str>,
    exec_blocks: &[ir::Block],
    runtime_block_param_names: &[Vec<String>],
    local_env: &LocalEnv,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    fb.switch_to_block(branch_block);
    let target_index = target_label.index();
    let target_params = &runtime_block_param_names[target_index];
    let mut jump_args = Vec::with_capacity(target_params.len());
    jump_args.extend(
        emit_prepare_target_args_codegen_from_local_env(
            fb,
            target_params,
            None,
            None,
            local_env,
            emit_ctx,
            jit_module,
            func_imports,
        )
        .ok_or_else(|| {
            format!(
                "missing local mapping for {arm_name}-branch block params in block {source_label}"
            )
        })?,
    );
    emit_decref_unforwarded_local_env(
        fb,
        local_env,
        target_params,
        emit_ctx.consts.thread_state_value,
        emit_ctx.decref_ref,
    );
    emit_planned_stack_slot_releases_for_reason(fb, source_label, &release_reason, emit_ctx)?;
    emit_pop_handled_exception_if_leaving(fb, current_exception_name, target_params, emit_ctx);
    fb.ins().jump(exec_blocks[target_index], &jump_args);
    Ok(())
}

fn emit_codegen_term(
    fb: &mut FunctionBuilder<'_>,
    source_label: BlockLabel,
    term: &BlockTerm<InstrCodegen>,
    exec_blocks: &[ir::Block],
    runtime_block_param_names: &[Vec<String>],
    full_block_param_names: &[Vec<String>],
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
    let thread_state_value = emit_ctx.consts.thread_state_value;
    let i64_ty = emit_ctx.consts.i64_ty;
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    match term {
        BlockTerm::Jump(target_label) => {
            let target_index = target_label.target.index();
            let target_params = &runtime_block_param_names[target_index];
            let full_target_params = &full_block_param_names[target_index];
            emit_explicit_target_slot_writes_codegen_from_local_env(
                fb,
                full_target_params,
                target_params,
                &target_label.args,
                local_env,
                emit_ctx,
                jit_module,
                func_imports,
            )
            .ok_or_else(|| {
                format!("missing local mapping for jump slot updates in block {source_label}")
            })?;
            let mut jump_args = Vec::with_capacity(target_params.len());
            jump_args.extend(
                emit_prepare_target_args_codegen_from_local_env(
                    fb,
                    target_params,
                    Some(full_target_params),
                    Some(&target_label.args),
                    local_env,
                    emit_ctx,
                    jit_module,
                    func_imports,
                )
                .ok_or_else(|| {
                    format!("missing local mapping for jump block params in block {source_label}")
                })?,
            );
            emit_decref_unforwarded_local_env(
                fb,
                local_env,
                target_params,
                thread_state_value,
                decref_ref,
            );
            let release_reason = RefcountReleaseReason::Jump {
                target: target_label.target,
            };
            emit_planned_stack_slot_releases_for_reason(
                fb,
                source_label,
                &release_reason,
                emit_ctx,
            )?;
            emit_pop_handled_exception_if_leaving(
                fb,
                current_exception_name,
                target_params,
                emit_ctx,
            );
            fb.ins().jump(exec_blocks[target_index], &jump_args);
        }
        BlockTerm::IfTerm(if_term) => {
            let test_instr_id = if_term.test.semantic_instr_id();
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
            if let Some(counter_id) = emit_ctx
                .branch_outcome_counter_ids
                .get(&test_instr_id)
                .copied()
            {
                emit_record_branch_outcome_sample(fb, counter_id, truth_i32, emit_ctx);
            }

            let prefer_true = emit_ctx
                .branch_prefer_true
                .get(&test_instr_id)
                .copied()
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
                ("then", if_term.then_label, "else", if_term.else_label)
            } else {
                ("else", if_term.else_label, "then", if_term.then_label)
            };
            emit_codegen_if_target_arm(
                fb,
                source_label,
                hot_name,
                hot_branch,
                hot_label,
                if hot_label == if_term.then_label {
                    RefcountReleaseReason::IfThen { target: hot_label }
                } else {
                    RefcountReleaseReason::IfElse { target: hot_label }
                },
                current_exception_name,
                exec_blocks,
                runtime_block_param_names,
                local_env,
                emit_ctx,
                jit_module,
                func_imports,
            )?;
            emit_codegen_if_target_arm(
                fb,
                source_label,
                cold_name,
                cold_branch,
                cold_label,
                if cold_label == if_term.then_label {
                    RefcountReleaseReason::IfThen { target: cold_label }
                } else {
                    RefcountReleaseReason::IfElse { target: cold_label }
                },
                current_exception_name,
                exec_blocks,
                runtime_block_param_names,
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
            let mut case_blocks = Vec::with_capacity(branch.targets.len());
            for (case_index, _) in branch.targets.iter().enumerate() {
                let case_block = fb.create_block();
                switch.set_entry(case_index as u128, case_block);
                case_blocks.push(case_block);
            }

            fb.switch_to_block(dispatch_block);
            let dispatch_value = fb.block_params(dispatch_block)[0];
            switch.emit(fb, dispatch_value, default_block);

            for (target_label, case_block) in branch.targets.iter().zip(case_blocks.iter()) {
                fb.switch_to_block(*case_block);
                let target_index = target_label.index();
                let target_params = &runtime_block_param_names[target_index];
                let mut case_jump_args = Vec::with_capacity(target_params.len());
                case_jump_args.extend(
                    emit_prepare_target_args_codegen_from_local_env(
                        fb,
                        target_params,
                        None,
                        None,
                        local_env,
                        emit_ctx,
                        jit_module,
                        func_imports,
                    )
                    .ok_or_else(|| {
                        format!(
                            "missing local mapping for br_table case block params in block {source_label}"
                        )
                    })?,
                );
                emit_decref_unforwarded_local_env(
                    fb,
                    local_env,
                    target_params,
                    thread_state_value,
                    decref_ref,
                );
                let release_reason = RefcountReleaseReason::BranchCase {
                    target: *target_label,
                };
                emit_planned_stack_slot_releases_for_reason(
                    fb,
                    source_label,
                    &release_reason,
                    emit_ctx,
                )?;
                emit_pop_handled_exception_if_leaving(
                    fb,
                    current_exception_name,
                    target_params,
                    emit_ctx,
                );
                fb.ins().jump(exec_blocks[target_index], &case_jump_args);
            }

            fb.switch_to_block(default_block);
            let default_index = branch.default_label.index();
            let default_params = &runtime_block_param_names[default_index];
            let mut default_jump_args = Vec::with_capacity(default_params.len());
            default_jump_args.extend(
                emit_prepare_target_args_codegen_from_local_env(
                    fb,
                    default_params,
                    None,
                    None,
                    local_env,
                    emit_ctx,
                    jit_module,
                    func_imports,
                )
                .ok_or_else(|| {
                    format!(
                        "missing local mapping for br_table default block params in block {source_label}"
                    )
                })?,
            );
            emit_decref_unforwarded_local_env(
                fb,
                local_env,
                default_params,
                thread_state_value,
                decref_ref,
            );
            let release_reason = RefcountReleaseReason::BranchDefault {
                target: branch.default_label,
            };
            emit_planned_stack_slot_releases_for_reason(
                fb,
                source_label,
                &release_reason,
                emit_ctx,
            )?;
            emit_pop_handled_exception_if_leaving(
                fb,
                current_exception_name,
                default_params,
                emit_ctx,
            );
            fb.ins()
                .jump(exec_blocks[default_index], &default_jump_args);
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
            emit_decref_unforwarded_local_env(fb, local_env, &[], thread_state_value, decref_ref);
            let release_reason = RefcountReleaseReason::Return;
            emit_planned_stack_slot_releases_for_reason(
                fb,
                source_label,
                &release_reason,
                emit_ctx,
            )?;
            emit_pop_handled_exception_if_leaving(fb, current_exception_name, &[], emit_ctx);
            fb.ins().return_(&[ret_value]);
        }
        BlockTerm::Raise(raise_stmt) => {
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
            fb.ins()
                .call(decref_ref, &[thread_state_value, raise_name_obj]);
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
            let rfo_raise_fn = fb.block_params(raise_fn_ok)[0];
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
                fb.ins()
                    .call(emit_ctx.incref_ref, &[emit_ctx.consts.none_const]);
                emit_ctx.consts.none_const
            };
            fb.ins()
                .call(emit_ctx.incref_ref, &[emit_ctx.consts.none_const]);
            let cause_value = emit_ctx.consts.none_const;
            let raise_call_inst = fb.ins().call(
                emit_ctx.py_call_positional_three_ref,
                &[
                    emit_ctx.consts.thread_state_value,
                    rfo_raise_fn,
                    exc_value,
                    cause_value,
                    null_ptr,
                    null_ptr,
                ],
            );
            let raise_exc_obj = fb.inst_results(raise_call_inst)[0];
            fb.ins()
                .call(decref_ref, &[thread_state_value, cause_value]);
            fb.ins().call(decref_ref, &[thread_state_value, exc_value]);
            fb.ins()
                .call(decref_ref, &[thread_state_value, rfo_raise_fn]);
            let raise_exc_null =
                fb.ins()
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

            fb.switch_to_block(raise_rc_fail);
            emit_pop_handled_exception_if_leaving(fb, current_exception_name, &[], emit_ctx);
            fb.ins().jump(
                emit_ctx.consts.step_null_block,
                &step_null_block_args(emit_ctx),
            );

            fb.switch_to_block(raise_rc_ok);
            emit_decref_unforwarded_local_env(fb, local_env, &[], thread_state_value, decref_ref);
            let release_reason = RefcountReleaseReason::Raise;
            emit_planned_stack_slot_releases_for_reason(
                fb,
                source_label,
                &release_reason,
                emit_ctx,
            )?;
            emit_pop_handled_exception_if_leaving(fb, current_exception_name, &[], emit_ctx);
            fb.ins().jump(
                emit_ctx.consts.step_null_block,
                &step_null_block_args(emit_ctx),
            );
        }
    }
    Ok(())
}

fn new_jit_builder() -> Result<JITBuilder, String> {
    let mut flag_builder = settings::builder();
    let opt_level = env::var("SOAC_CRANELIFT_OPT_LEVEL").unwrap_or_else(|_| "speed".to_string());
    flag_builder
        .set("opt_level", opt_level.as_str())
        .map_err(|err| format!("failed to configure Cranelift flags: {err}"))?;
    flag_builder
        .set("is_pic", "false")
        .map_err(|err| format!("failed to configure Cranelift flags: {err}"))?;
    flag_builder
        .set("preserve_frame_pointers", "true")
        .map_err(|err| format!("failed to configure Cranelift flags: {err}"))?;
    flag_builder
        .set("machine_code_cfg_info", "true")
        .map_err(|err| format!("failed to configure Cranelift flags: {err}"))?;
    let isa_builder = cranelift_native::builder().map_err(|err| format!("{err}"))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|err| format!("failed to finish ISA: {err}"))?;
    let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    if let Ok(provider) = ArenaMemoryProvider::new_with_size(JIT_ARENA_BYTES) {
        builder.memory_provider(Box::new(provider));
    }
    builder.symbol("_Py_Dealloc", py_dealloc_symbol());
    builder.symbol(
        "_PyDict_IndexedValueTombstone",
        std::ptr::addr_of_mut!(_PyDict_IndexedValueTombstone).cast::<u8>(),
    );
    register_specialized_jit_symbols(&mut builder);
    Ok(builder)
}

fn new_jit_module(compile_session: &crate::session::CompileSession) -> Result<JITModule, String> {
    let mut jit_module = JITModule::new(new_jit_builder()?);
    load_runtime_support_clif(compile_session, &mut jit_module)?;
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
    module_constants: &ModuleCodegenConstants,
    direct_call_resolver: Option<&'a crate::module_type::SharedModuleState>,
) -> Result<Vec<ProcessJitBatchFunction<'a>>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
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
                queue.push_back(function);
            }
        }
        let batch_module_constants = batch_function
            .source
            .shared_state()
            .map(|shared_state| &shared_state.codegen_constants)
            .unwrap_or(module_constants);
        for function_id in
            collect_make_function_targets(&batch_function.function, batch_module_constants)
        {
            if seen.contains(&function_id) {
                continue;
            }
            if let Some(function) =
                resolve_process_jit_batch_function(session, direct_call_resolver, function_id)?
            {
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
        compile_session: &crate::session::CompileSession,
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
        let entry = define_shared_vectorcall_trampoline(
            compile_session,
            &mut state.jit_module,
            param_count,
            &symbol,
        )?;
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
        counter_ptrs: &[*mut u64],
        direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
    ) -> Result<DirectFunctionCompileResult, String> {
        let batch_functions = collect_process_jit_batch_functions(
            session,
            function,
            module_constants,
            direct_call_resolver,
        )?;
        let root_function_id = function.function_id;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "process JIT module lock poisoned".to_string())?;
        if let Some(compiled_handle) = state.ready_direct_function(function) {
            return Ok(DirectFunctionCompileResult {
                handle: compiled_handle,
                compiled: false,
            });
        }
        let _guard = ProcessJitCompileGuard::enter();
        let mut predeclared = HashMap::new();
        let mut functions_to_define = Vec::new();
        for batch_function in &batch_functions {
            let function = &batch_function.function;
            let declared = state.declare_direct_function(function)?;
            if !state.is_direct_function_ready(function.function_id) {
                functions_to_define.push(batch_function);
            }
            predeclared.insert(function.function_id, declared);
        }

        let mut defined_functions = Vec::with_capacity(functions_to_define.len());
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
            let owned_counter_ptrs;
            let (
                function_module,
                function_module_constants,
                function_counter_defs,
                function_module_constant_ptrs,
                function_counter_ptrs,
                function_direct_call_resolver,
            ) = if let Some(shared_state) = batch_function.source.shared_state() {
                owned_module_constant_ptrs = shared_state.module_constant_ptrs();
                owned_counter_ptrs = shared_state.counter_ptrs();
                (
                    &shared_state.lowered_module,
                    &shared_state.codegen_constants,
                    shared_state.lowered_module.counter_defs.as_slice(),
                    owned_module_constant_ptrs.as_slice(),
                    owned_counter_ptrs.as_slice(),
                    Some(shared_state),
                )
            } else {
                (
                    module,
                    module_constants,
                    counter_defs,
                    module_constant_ptrs,
                    counter_ptrs,
                    None,
                )
            };
            let built = build_cranelift_run_bb_specialized_function(
                &mut state.jit_module,
                function_blocks,
                function_module,
                function,
                function_module_constants,
                function_counter_defs,
                function_module_constant_ptrs,
                function_counter_ptrs,
                session.as_ref(),
                function_direct_call_resolver,
                None,
                Some(&predeclared),
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
            let artifact = define_function_with_incremental_cache(
                session.as_ref(),
                &mut state.jit_module,
                main_id,
                &mut ctx,
                &format!(
                    "direct:{}:{}",
                    function.names.qualname,
                    function.params.len()
                ),
                CraneliftCompileCachePolicy::Disabled {
                    reason: "direct function body embeds per-run module constants and counter pointers",
                },
                "failed to define specialized jit run_bb function",
            )
            .map_err(|err| {
                format!(
                    "{err} [function={} id={}]",
                    function.names.qualname, function.function_id
                )
            })?;
            state.jit_module.clear_context(&mut ctx);
            defined_functions.push(DefinedJitFunction {
                function_id: function.function_id,
                function_qualname: function.names.qualname.clone(),
                param_count: function.params.len(),
                main_id,
                main_symbol,
                artifact,
            });
        }

        state
            .jit_module
            .finalize_definitions()
            .map_err(|err| format!("failed to finalize specialized jit run_bb function: {err}"))?;
        let mut root_handle = None;
        for defined in defined_functions {
            let code_ptr = state.jit_module.get_finalized_function(defined.main_id);
            let compiled_handle = state.mark_direct_function_ready(
                session,
                defined.function_id,
                code_ptr,
                defined.param_count,
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
            );
            if defined.function_id == function.function_id {
                root_handle = Some(compiled_handle);
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

fn define_function_with_incremental_cache(
    compile_session: &crate::session::CompileSession,
    jit_module: &mut JITModule,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    cache_name: &str,
    cache_policy: CraneliftCompileCachePolicy,
    err_prefix: &str,
) -> Result<DefinedFunctionArtifact, String> {
    inline_runtime_support_calls(jit_module, ctx, err_prefix)?;
    ctx.func.name = stable_cranelift_compile_cache_name(cache_name);
    let func_for_relocs = ctx.func.clone();
    let func_name = ctx.func.name.clone();
    let mut ctrl_plane = ControlPlane::default();
    let compiled =
        if compile_session.cranelift_compile_cache().is_enabled() && cache_policy.is_enabled() {
            let mut cache_store = compile_session.cranelift_compile_cache().store();
            let (compiled, cache_hit) = ctx
                .compile_with_cache(jit_module.isa(), &mut cache_store, &mut ctrl_plane)
                .map_err(|err| format!("{err_prefix}: {err:?}"))?;
            if cache_hit {
                info!(
                    target: "soac_jit_compile_cache",
                    function = ?func_name,
                    cache_name,
                    func_id = func_id.as_u32(),
                    request = %err_prefix,
                    code_size = compiled.code_buffer().len(),
                    "Cranelift compile cache hit"
                );
            }
            compiled
        } else {
            if compile_session.cranelift_compile_cache().is_enabled()
                && let Some(reason) = cache_policy.disabled_reason()
            {
                tracing::debug!(
                    target: "soac_jit_compile_cache",
                    function = ?func_name,
                    cache_name,
                    func_id = func_id.as_u32(),
                    request = %err_prefix,
                    reason,
                    "Cranelift compile cache skipped for function"
                );
            }
            ctx.compile(jit_module.isa(), &mut ctrl_plane)
                .map_err(|err| format!("{err_prefix}: {err:?}"))?
        };
    let (code_bb_offsets, code_bb_edges) = compiled.get_code_bb_layout();
    let alignment = compiled.buffer.alignment as u64;
    let relocs = compiled
        .buffer
        .relocs()
        .iter()
        .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &func_for_relocs, func_id))
        .collect::<Vec<_>>();
    let systemv_unwind_info = compiled
        .create_unwind_info(jit_module.isa())
        .map_err(|err| format!("{err_prefix}: failed to create unwind info: {err:?}"))?
        .and_then(|unwind_info| match unwind_info {
            cranelift_codegen::isa::unwind::UnwindInfo::SystemV(info) => Some(info),
            _ => None,
        });
    jit_module
        .define_function_bytes(func_id, alignment, compiled.code_buffer(), &relocs)
        .map_err(|err| format!("{err_prefix}: {err}"))?;
    Ok(DefinedFunctionArtifact {
        code_size: compiled.code_buffer().len(),
        code_bb_offsets,
        code_bb_edges,
        systemv_unwind_info,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CraneliftCompileCachePolicy {
    Enabled,
    Disabled { reason: &'static str },
}

impl CraneliftCompileCachePolicy {
    fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }

    fn disabled_reason(self) -> Option<&'static str> {
        match self {
            Self::Enabled => None,
            Self::Disabled { reason } => Some(reason),
        }
    }
}

fn stable_cranelift_compile_cache_name(cache_name: &str) -> ir::UserFuncName {
    let hash = stable_compile_cache_hash(cache_name.as_bytes());
    ir::UserFuncName::user((hash >> 32) as u32, hash as u32)
}

fn stable_compile_cache_hash(bytes: &[u8]) -> u64 {
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
) {
    let Some(dir) = soac_work_dir_from_env() else {
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

#[derive(Debug)]
struct RuntimeSupportInliner {
    inlineable: HashMap<ir::UserExternalName, ir::Function>,
}

impl RuntimeSupportInliner {
    fn for_module(jit_module: &mut JITModule) -> Result<Self, String> {
        let library = runtime_support_library()?;
        let mut import_func_ids = HashMap::new();
        let mut import_data_ids = HashMap::new();
        let mut inlineable = HashMap::new();
        for parsed in &library.functions {
            if !matches!(
                parsed.symbol.as_str(),
                SOAC_RUNTIME_INCREF_SYMBOL
                    | SOAC_RUNTIME_DECREF_SYMBOL
                    | SOAC_RUNTIME_CALLEE_FUNCTION_ID_SYMBOL
                    | SOAC_RUNTIME_FUNCTION_DATA_BLOCK_SYMBOL
                    | SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL
                    | SOAC_RUNTIME_LOAD_GLOBAL_INDEXED_SYMBOL
                    | SOAC_RUNTIME_STORE_GLOBAL_SYMBOL
                    | SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL
                    | SOAC_RUNTIME_LOAD_FIELD_INDEXED_SYMBOL
                    | SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL
                    | SOAC_RUNTIME_GUARD_TYPE_VERSION_SYMBOL
            ) {
                continue;
            }
            let func_id = jit_module
                .declare_function(&parsed.symbol, Linkage::Local, &parsed.function.signature)
                .map_err(|err| {
                    format!(
                        "failed to declare inlineable runtime CLIF function {}: {err}",
                        parsed.symbol
                    )
                })?;
            let mut function = parsed.function.clone();
            remap_runtime_clif_extern_user_names(
                jit_module,
                &mut function,
                &parsed.extern_symbols,
                &parsed.global_extern_symbols,
                &mut import_func_ids,
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

fn define_import_trampoline_fn(
    jit_module: &mut JITModule,
    symbol: &str,
    sig: &ir::Signature,
) -> Result<FuncId, String> {
    let import_id = declare_import_fn(jit_module, symbol, sig)?;
    let symbol_suffix = symbol.replace(|ch: char| !ch.is_ascii_alphanumeric(), "_");
    let trampoline_index = NEXT_IMPORT_TRAMPOLINE_ID.fetch_add(1, Ordering::Relaxed);
    let data_symbol = format!("__soac_import_target_{symbol_suffix}_{trampoline_index}");
    let data_id = jit_module
        .declare_data(&data_symbol, Linkage::Local, false, false)
        .map_err(|err| format!("failed to declare import target data for {symbol}: {err}"))?;
    let mut data = DataDescription::new();
    data.define_zeroinit(std::mem::size_of::<usize>());
    data.set_align(std::mem::align_of::<usize>() as u64);
    let import_ref = jit_module.declare_func_in_data(import_id, &mut data);
    data.write_function_addr(0, import_ref);
    jit_module
        .define_data(data_id, &data)
        .map_err(|err| format!("failed to define import target data for {symbol}: {err}"))?;

    let trampoline_symbol = format!("__soac_import_trampoline_{symbol_suffix}_{trampoline_index}");
    let trampoline_id = declare_local_fn(jit_module, &trampoline_symbol, sig)?;
    let ptr_ty = jit_module.target_config().pointer_type();
    let mut ctx = jit_module.make_context();
    ctx.func.signature = sig.clone();
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);

        let target_data = jit_module.declare_data_in_func(data_id, &mut fb.func);
        let target_slot = fb.ins().global_value(ptr_ty, target_data);
        let target = fb
            .ins()
            .load(ptr_ty, ir::MemFlags::trusted(), target_slot, 0);
        let sig_ref = fb.import_signature(sig.clone());
        let args = fb.block_params(entry).to_vec();
        let call_inst = fb.ins().call_indirect(sig_ref, target, &args);
        let results = fb.inst_results(call_inst).to_vec();
        fb.ins().return_(&results);
        fb.finalize();
    }
    jit_module
        .define_function(trampoline_id, &mut ctx)
        .map_err(|err| format!("failed to define import trampoline for {symbol}: {err}"))?;
    jit_module.clear_context(&mut ctx);
    Ok(trampoline_id)
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

fn direct_function_symbol_scope(function_id: FunctionId, symbol_id: u64) -> String {
    format!("fn_{}_{}", function_id.packed(), symbol_id)
}

fn declare_direct_function(
    jit_module: &mut JITModule,
    function: &BlockPyFunction<CodegenModuleShape>,
    symbol_scope: Option<&str>,
) -> Result<(ir::Signature, DeclaredJitFunction), String> {
    let sig = make_direct_function_signature(jit_module, function);
    let symbol = direct_function_symbol(function, symbol_scope);
    let func_id = declare_local_fn(jit_module, &symbol, &sig)?;
    Ok((sig, DeclaredJitFunction { func_id, symbol }))
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
pub(crate) const SOAC_RUNTIME_SET_RAISED_EXCEPTION_SYMBOL: &str =
    "soac_runtime_set_raised_exception";
pub(crate) const SOAC_RUNTIME_CALLEE_FUNCTION_ID_SYMBOL: &str = "soac_runtime_callee_function_id";
pub(crate) const SOAC_RUNTIME_FUNCTION_DATA_BLOCK_SYMBOL: &str = "soac_runtime_function_data_block";
pub(crate) const SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL: &str = "soac_runtime_load_global";
pub(crate) const SOAC_RUNTIME_LOAD_GLOBAL_INDEXED_SYMBOL: &str = "soac_runtime_load_global_indexed";
pub(crate) const SOAC_RUNTIME_STORE_GLOBAL_SYMBOL: &str = "soac_runtime_store_global";
pub(crate) const SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL: &str =
    "soac_runtime_store_global_indexed";
pub(crate) const SOAC_RUNTIME_LOAD_FIELD_INDEXED_SYMBOL: &str = "soac_runtime_load_field_indexed";
pub(crate) const SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL: &str = "soac_runtime_store_field_indexed";
pub(crate) const SOAC_RUNTIME_GUARD_TYPE_VERSION_SYMBOL: &str = "soac_runtime_guard_type_version";

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
        if !line.contains("::{extern#") {
            continue;
        }
        if !line.contains(" = u") {
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
    let symbol = rest.rsplit("::").next()?;
    let symbol_end = symbol
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .unwrap_or(symbol.len());
    let symbol = symbol.get(..symbol_end)?;
    if symbol.is_empty() {
        return None;
    }
    Some(symbol.to_string())
}

fn remap_runtime_clif_extern_user_names(
    jit_module: &mut JITModule,
    function: &mut ir::Function,
    extern_symbols: &HashMap<ir::UserExternalName, String>,
    global_extern_symbols: &HashMap<u32, String>,
    import_func_ids: &mut HashMap<String, FuncId>,
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
        let mapped_name = if let Some(symbol) = extern_symbols.get(&original_name) {
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

fn load_runtime_support_clif(
    compile_session: &crate::session::CompileSession,
    jit_module: &mut JITModule,
) -> Result<(), String> {
    let library = runtime_support_library()?;
    let mut import_func_ids = HashMap::new();
    let mut import_data_ids = HashMap::new();
    for parsed in library.functions.iter().cloned() {
        let func_id = jit_module
            .declare_function(&parsed.symbol, Linkage::Local, &parsed.function.signature)
            .map_err(|err| {
                format!(
                    "failed to declare runtime CLIF function {}: {err}",
                    parsed.symbol
                )
            })?;
        let mut function = parsed.function;
        remap_runtime_clif_extern_user_names(
            jit_module,
            &mut function,
            &parsed.extern_symbols,
            &parsed.global_extern_symbols,
            &mut import_func_ids,
            &mut import_data_ids,
        )?;
        let mut ctx = jit_module.make_context();
        ctx.func = function;
        let _ = define_function_with_incremental_cache(
            compile_session,
            jit_module,
            func_id,
            &mut ctx,
            &parsed.symbol,
            CraneliftCompileCachePolicy::Enabled,
            &format!("failed to define runtime CLIF function {}", parsed.symbol),
        )?;
        jit_module.clear_context(&mut ctx);
    }
    Ok(())
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
    let _ = define_function_with_incremental_cache(
        &compile_session,
        &mut jit_module,
        function_id,
        &mut ctx,
        "jit-smoke",
        CraneliftCompileCachePolicy::Enabled,
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

fn build_cranelift_run_bb_specialized_function(
    jit_module: &mut JITModule,
    blocks: &[ObjPtr],
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
    module_constants: &ModuleCodegenConstants,
    counter_defs: &[CounterDef],
    module_constant_ptrs: &[*mut ffi::PyObject],
    counter_ptrs: &[*mut u64],
    compile_session: &crate::session::CompileSession,
    direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
    symbol_scope: Option<&str>,
    predeclared_direct_functions: Option<&HashMap<FunctionId, DeclaredJitFunction>>,
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
    if module_constant_ptrs.len() != module_constants.len() {
        return Err(format!(
            "specialized JIT module constant pointer length mismatch: {} != {}",
            module_constant_ptrs.len(),
            module_constants.len()
        ));
    }
    for block in &function.blocks {
        for expr in &block.body {
            if let InstrCodegen::IncrementCounter(op) = expr {
                if op.counter_id.0 >= counter_ptrs.len() {
                    return Err(format!(
                        "specialized JIT counter pointer length mismatch: missing counter id {} for function {}",
                        op.counter_id.0, function.names.qualname
                    ));
                }
            }
        }
    }

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
    let branch_outcome_counter_ids =
        collect_runtime_counter_ids_by_kind(counter_defs, function.function_id, "branch_outcomes");
    let call_target_specializations = match direct_call_resolver {
        Some(shared_state) => load_call_target_specializations(
            shared_state.module_name.as_str(),
            function.function_id,
        )?,
        None => HashMap::new(),
    };
    let operator_specializations = match direct_call_resolver {
        Some(shared_state) => {
            load_operator_specializations(shared_state.module_name.as_str(), function.function_id)?
        }
        None => HashMap::new(),
    };
    let field_index_specializations = load_field_index_specializations()?;
    let branch_prefer_true = match direct_call_resolver {
        Some(shared_state) => {
            load_branch_preferences(shared_state.module_name.as_str(), function.function_id)?
        }
        None => HashMap::new(),
    };
    let behavior_change_indexed_stores = behavior_change_indexed_stores_enabled()
        && function.scope.scope_kind != CallableScopeKind::Module;
    let function_runtime_data_layout = FunctionRuntimeDataLayout::from_function(function);
    let true_constant_id = module_constants.require_runtime_name_constant_id("TRUE");
    let false_constant_id = module_constants.require_runtime_name_constant_id("FALSE");
    let none_constant_id = module_constants.require_runtime_name_constant_id("NONE");
    let deleted_constant_id = module_constants.require_runtime_name_constant_id("DELETED");
    let empty_tuple_constant_id = module_constants.require_runtime_name_constant_id("EMPTY_TUPLE");

    let mut direct_call_targets = collect_call_direct_targets(function);
    for targets in call_target_specializations.values() {
        direct_call_targets.extend(targets.iter().copied());
    }
    let empty_direct_functions = HashMap::new();
    let direct_call_functions = predeclared_direct_functions.unwrap_or(&empty_direct_functions);
    let value_facts = infer_jit_value_facts(module);
    let local_plan = plan_function_locals(function, &value_facts);
    let refcount_plan = plan_function_refcount_ownership(module, function, &value_facts)?;
    let _refcount_plan_check = check_refcount_plan_against_current_jit(function, &refcount_plan)?;

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
    let top_value_counter_ptrs = direct_call_resolver
        .map(|shared_state| shared_state.top_value_counter_ptrs())
        .unwrap_or_else(|| placeholder_top_value_counter_ptrs(counter_ptrs.len()));

    let ptr_ty = jit_module.target_config().pointer_type();
    let i64_ty = ir::types::I64;
    let mut module_imports = ModuleFuncImports::new();

    let (main_sig, main_id, main_symbol) = match predeclared_direct_functions
        .and_then(|functions| functions.get(&function.function_id))
    {
        Some(declared) => (
            make_direct_function_signature(jit_module, function),
            declared.func_id,
            declared.symbol.clone(),
        ),
        None => {
            let (sig, declared) = declare_direct_function(jit_module, function, symbol_scope)?;
            (sig, declared.func_id, declared.symbol)
        }
    };
    let counted_refcount_helpers = build_counted_runtime_refcount_helpers(
        compile_session,
        jit_module,
        function,
        counter_defs,
        counter_ptrs,
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
        let runtime_block_param_names = function
            .blocks
            .iter()
            .map(jit_param_names_for_block)
            .collect::<Vec<_>>();
        let full_block_param_names = function
            .blocks
            .iter()
            .map(CodegenBlock::param_name_vec)
            .collect::<Vec<_>>();
        let exc_dispatches = function
            .blocks
            .iter()
            .map(|block| exc_dispatch_plan(function, block))
            .collect::<Vec<_>>();
        let mut pre_cleanup_null_blocks = Vec::with_capacity(block_count);
        let mut cleanup_null_blocks = Vec::with_capacity(block_count);
        for _ in 0..block_count {
            exec_blocks.push(fb.create_block());
            pre_cleanup_null_blocks.push(fb.create_block());
            cleanup_null_blocks.push(fb.create_block());
        }
        let step_null_block = fb.create_block();
        let raise_exc_direct_block = fb.create_block();
        let stack_slots = StackSlots::new(
            &mut fb,
            function
                .storage_layout()
                .as_ref()
                .map(|layout| layout.stack_slots())
                .unwrap_or(&[]),
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
            let param_names = if runtime_block_param_names[index].is_empty() {
                full_block_param_names[index].clone()
            } else {
                runtime_block_param_names[index].clone()
            };
            register_block_display_annotation(
                &mut block_annotations,
                *block,
                function.blocks[index].label.to_string(),
                param_names,
            );
        }
        for (index, block) in cleanup_null_blocks.iter().enumerate() {
            register_block_display_annotation(
                &mut block_annotations,
                pre_cleanup_null_blocks[index],
                format!("pre_cleanup_null::{}", function.blocks[index].label),
                Vec::new(),
            );
            register_block_display_annotation(
                &mut block_annotations,
                *block,
                format!("cleanup_null::{}", function.blocks[index].label),
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
            for _ in &runtime_block_param_names[index] {
                fb.append_block_param(*block, ptr_ty);
            }
        }
        fb.append_block_param(step_null_block, ptr_ty); // args
        fb.append_block_param(raise_exc_direct_block, ptr_ty); // args
        fb.append_block_param(raise_exc_direct_block, ptr_ty); // exc
        for block in &cleanup_null_blocks {
            fb.append_block_param(*block, ptr_ty); // error
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
        let direct_function_context_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_DIRECT_FUNCTION_CONTEXT_IMPORT,
        );
        let enter_recursive_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_ENTER_RECURSIVE_CALL_IMPORT,
        );
        let leave_recursive_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_LEAVE_RECURSIVE_CALL_IMPORT,
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
        let load_global_indexed_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_LOAD_GLOBAL_INDEXED_IMPORT,
        );
        let load_global_slow_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_LOAD_GLOBAL_SLOW_IMPORT,
        );
        let store_global_indexed_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_STORE_GLOBAL_INDEXED_IMPORT,
        );
        let load_field_indexed_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_LOAD_FIELD_INDEXED_IMPORT,
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
        let guard_method_type_version_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_GUARD_TYPE_VERSION_IMPORT,
        );
        let record_top_value_sample_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT,
        );
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
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_TUPLE_NEW_IMPORT);
        let tuple_set_item_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_TUPLE_SET_ITEM_IMPORT);
        let set_raised_exception_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT,
        );
        let fallthrough_abrupt_kind_const = stack_slots.has_try_abrupt_kind_name().then(|| {
            emit_owned_module_constant_from_parts(
                &mut fb,
                module_constants.require_int_constant_id(abrupt_kind_tag(AbruptKind::Fallthrough)),
                module_constant_ptrs,
                ptr_ty,
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
        for (param_index, (param, value)) in function
            .params
            .iter()
            .zip(direct_entry_args.iter())
            .enumerate()
        {
            let default_slot = match param.kind {
                ParamKind::PosOnly | ParamKind::Any => function_runtime_data_layout
                    .positional_default_slot_for_param_index(param_index),
                ParamKind::KwOnly => function_runtime_data_layout.kwonly_default_slot(&param.name),
                ParamKind::VarArg | ParamKind::KwArg => None,
            };

            let Some(default_slot) = default_slot else {
                stack_slots
                    .replace_cloned_value(
                        &mut fb,
                        param.name.as_str(),
                        *value,
                        ptr_ty,
                        thread_state_value,
                        incref_ref,
                        decref_ref,
                    )
                    .expect("entry slot missing from stack slots");
                continue;
            };

            let arg_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, *value, null_ptr);
            let use_default_block = fb.create_block();
            let use_arg_block = fb.create_block();
            let after_block = fb.create_block();
            fb.ins()
                .brif(arg_is_null, use_default_block, &[], use_arg_block, &[]);

            fb.switch_to_block(use_default_block);
            let default_value = emit_function_data_slot_owned_or_null(
                &mut fb,
                function_data_value,
                default_slot,
                ptr_ty,
                incref_ref,
            );
            let default_is_null =
                fb.ins()
                    .icmp(ir::condcodes::IntCC::Equal, default_value, null_ptr);
            let default_ok_block = fb.create_block();
            fb.append_block_param(default_ok_block, ptr_ty);
            fb.ins().brif(
                default_is_null,
                entry_failure_block,
                &block_arg_values(&entry_failure_args),
                default_ok_block,
                &[ir::BlockArg::Value(default_value)],
            );
            fb.switch_to_block(default_ok_block);
            let default_value = fb.block_params(default_ok_block)[0];
            stack_slots
                .replace_cloned_value(
                    &mut fb,
                    param.name.as_str(),
                    default_value,
                    ptr_ty,
                    thread_state_value,
                    incref_ref,
                    decref_ref,
                )
                .expect("entry slot missing from stack slots");
            fb.ins()
                .call(decref_ref, &[thread_state_value, default_value]);
            fb.ins().jump(after_block, &[]);

            fb.switch_to_block(use_arg_block);
            stack_slots
                .replace_cloned_value(
                    &mut fb,
                    param.name.as_str(),
                    *value,
                    ptr_ty,
                    thread_state_value,
                    incref_ref,
                    decref_ref,
                )
                .expect("entry slot missing from stack slots");
            fb.ins().jump(after_block, &[]);

            fb.switch_to_block(after_block);
        }
        let mut entry_jump_args = Vec::with_capacity(runtime_block_param_names[0].len());
        for param_name in &runtime_block_param_names[0] {
            let value =
                load_stack_slot_value(&mut fb, &stack_slots, param_name, ptr_ty, false, incref_ref)
                    .expect("entry runtime param missing from stack slots");
            entry_jump_args.push(ir::BlockArg::Value(value));
        }
        fb.ins().jump(exec_blocks[0], &entry_jump_args);

        let mut exception_dispatch_blocks: Vec<Option<ir::Block>> = vec![None; exec_blocks.len()];
        let mut pending_local_failure_cleanups = Vec::new();
        for (index, maybe_dispatch) in exc_dispatches.iter().enumerate() {
            if maybe_dispatch.is_some() {
                let dispatch_block = fb.create_block();
                register_block_display_annotation(
                    &mut block_annotations,
                    dispatch_block,
                    format!("exc_dispatch::{}", function.blocks[index].label),
                    Vec::new(),
                );
                exception_dispatch_blocks[index] = Some(dispatch_block);
            }
        }

        for (index, block) in exec_blocks.iter().enumerate() {
            fb.switch_to_block(*block);
            let codegen_block = &function.blocks[index];
            let block_local_plan = local_plan.block(codegen_block.label);
            let mut local_env = LocalEnv::default();
            let block_param_values = fb.block_params(*block).to_vec();
            for (param_name, param_value) in runtime_block_param_names[index]
                .iter()
                .zip(block_param_values.iter())
            {
                if let Some(binding) =
                    planned_entry_binding_for_block_arg_name(block_local_plan, param_name)
                {
                    local_env.bind_entry_location(
                        binding.location,
                        binding.name.as_str(),
                        *param_value,
                        local_ref_kind_for_stack_mirror(binding.ref_kind),
                        LocalEnvStorage::StackMirror,
                    );
                    // Keep a stack-slot mirror until failure cleanup consumes LocalEnv directly.
                    stack_slots
                        .replace_cloned_value(
                            &mut fb,
                            binding.name.as_str(),
                            *param_value,
                            ptr_ty,
                            thread_state_value,
                            incref_ref,
                            decref_ref,
                        )
                        .expect("runtime block param missing from stack slots");
                    fb.ins()
                        .call(decref_ref, &[thread_state_value, *param_value]);
                } else {
                    stack_slots
                        .replace_cloned_value(
                            &mut fb,
                            param_name,
                            *param_value,
                            ptr_ty,
                            thread_state_value,
                            incref_ref,
                            decref_ref,
                        )
                        .expect("runtime block param missing from stack slots");
                    fb.ins()
                        .call(decref_ref, &[thread_state_value, *param_value]);
                }
            }
            let block_const = globals_value;
            let none_const = emit_owned_module_constant_from_parts(
                &mut fb,
                none_constant_id,
                module_constant_ptrs,
                ptr_ty,
            );
            let true_const = emit_owned_module_constant_from_parts(
                &mut fb,
                true_constant_id,
                module_constant_ptrs,
                ptr_ty,
            );
            let false_const = emit_owned_module_constant_from_parts(
                &mut fb,
                false_constant_id,
                module_constant_ptrs,
                ptr_ty,
            );
            let deleted_const = emit_owned_module_constant_from_parts(
                &mut fb,
                deleted_constant_id,
                module_constant_ptrs,
                ptr_ty,
            );
            let empty_tuple_const = emit_owned_module_constant_from_parts(
                &mut fb,
                empty_tuple_constant_id,
                module_constant_ptrs,
                ptr_ty,
            );
            let fast_step_null_block =
                exception_dispatch_blocks[index].unwrap_or(pre_cleanup_null_blocks[index]);
            let fast_step_null_args = Vec::new();
            let emit_ctx = JitEmitCtx {
                module,
                function_id: function.function_id,
                shared_state: direct_call_resolver,
                module_constants,
                module_constant_ptrs,
                value_facts: &value_facts,
                refcount_plan: &refcount_plan,
                counter_ptrs,
                top_value_counter_ptrs: &top_value_counter_ptrs,
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
                    function_data_value,
                    thread_state_value,
                    none_const,
                    true_const,
                    false_const,
                    deleted_const,
                    empty_tuple_const,
                    block_const,
                    py_function_type_ptr: std::ptr::addr_of_mut!(PyFunction_Type),
                    py_method_type_ptr: std::ptr::addr_of_mut!(PyMethod_Type),
                    py_type_type_ptr: std::ptr::addr_of_mut!(PyType_Type),
                    py_long_type_ptr: std::ptr::addr_of_mut!(PyLong_Type),
                },
                load_global_fast_ref,
                load_global_indexed_ref,
                load_global_slow_ref,
                store_global_indexed_ref,
                load_field_indexed_ref,
                store_field_indexed_ref,
                load_runtime_obj_ref,
                direct_function_context_ref,
                enter_recursive_ref,
                leave_recursive_ref,
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
                guard_method_type_version_ref,
                record_top_value_sample_ref,
                tuple_new_ref,
                tuple_set_item_ref,
                set_raised_exception_ref,
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
                global_indexed_hit_counter_ids: &global_indexed_hit_counter_ids,
                global_indexed_fallback_counter_ids: &global_indexed_fallback_counter_ids,
                field_indexed_hit_counter_ids: &field_indexed_hit_counter_ids,
                field_indexed_fallback_counter_ids: &field_indexed_fallback_counter_ids,
                branch_outcome_counter_ids: &branch_outcome_counter_ids,
                branch_prefer_true: &branch_prefer_true,
                field_index_specializations: &field_index_specializations,
                behavior_change_indexed_stores,
            };
            let _block_refcount_plan = emit_ctx.refcount_plan.block(codegen_block.label);

            emit_codegen_ops(
                &mut fb,
                &codegen_block.body,
                &mut local_env,
                &stack_slots,
                &emit_ctx,
                pre_cleanup_null_blocks[index],
                cleanup_null_blocks[index],
                &mut pending_local_failure_cleanups,
                jit_module,
                &mut func_imports,
            )?;

            let term_emit_ctx = local_failure_cleanup_emit_ctx(
                &mut fb,
                &emit_ctx,
                &local_env,
                pre_cleanup_null_blocks[index],
                cleanup_null_blocks[index],
                &mut pending_local_failure_cleanups,
            );
            let term_emit_ctx = term_emit_ctx.as_ref().unwrap_or(&emit_ctx);
            emit_codegen_term(
                &mut fb,
                codegen_block.label,
                &codegen_block.term,
                &exec_blocks,
                &runtime_block_param_names,
                &full_block_param_names,
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
            let null_ptr = fb.ins().iconst(ptr_ty, 0);
            let none_const = emit_owned_module_constant_from_parts(
                &mut fb,
                none_constant_id,
                module_constant_ptrs,
                ptr_ty,
            );
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
            emit_exception_dispatch_slot_writes(
                &mut fb,
                &dispatch_plan.slot_writes,
                dispatch_exc,
                &stack_slots,
                ptr_ty,
                thread_state_value,
                none_const,
                incref_ref,
                decref_ref,
            )?;
            let source_label = function.blocks[index].label;
            let release_reason = RefcountReleaseReason::ExceptionEdge {
                target: function.blocks[dispatch_plan.target_index].label,
            };
            emit_planned_stack_slot_releases_for_reason_from_parts(
                &mut fb,
                source_label,
                &release_reason,
                &refcount_plan,
                &stack_slots,
                ptr_ty,
                thread_state_value,
                decref_ref,
            )?;
            let target_runtime_params = &runtime_block_param_names[dispatch_plan.target_index];
            let mut target_jump_args = Vec::with_capacity(target_runtime_params.len());
            if target_runtime_params.is_empty() {
                fb.ins()
                    .call(decref_ref, &[thread_state_value, dispatch_exc]);
            } else {
                target_jump_args.push(ir::BlockArg::Value(dispatch_exc));
            }
            fb.ins()
                .jump(exec_blocks[dispatch_plan.target_index], &target_jump_args);
        }

        for cleanup in &pending_local_failure_cleanups {
            fb.switch_to_block(cleanup.block);
            let error_value =
                emit_take_current_raised_exception_or_trap(&mut fb, ptr_ty, thread_state_value);
            let cleanup_values = fb.block_params(cleanup.block).to_vec();
            for value in cleanup_values {
                fb.ins().call(decref_ref, &[thread_state_value, value]);
            }
            fb.ins().jump(
                cleanup.cleanup_null_block,
                &[ir::BlockArg::Value(error_value)],
            );
        }

        for (index, block) in pre_cleanup_null_blocks.iter().enumerate() {
            fb.switch_to_block(*block);
            let error_value =
                emit_take_current_raised_exception_or_trap(&mut fb, ptr_ty, thread_state_value);
            fb.ins().jump(
                cleanup_null_blocks[index],
                &[ir::BlockArg::Value(error_value)],
            );
        }

        for (index, block) in cleanup_null_blocks.iter().enumerate() {
            fb.switch_to_block(*block);
            let error_value = fb.block_params(*block)[0];
            let cleanup_args = fb.block_params(*block)[1..].to_vec();
            for value in cleanup_args {
                fb.ins().call(decref_ref, &[thread_state_value, value]);
            }
            if let Some(exception_name) = function.blocks[index].exception_param() {
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
        import_id_to_symbol: module_imports.debug_symbols().clone(),
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
    let counter_ptrs = runtime_state
        .map(SharedModuleState::counter_ptrs)
        .unwrap_or_else(|| {
            placeholder_counter_ptrs(
                function
                    .blocks
                    .iter()
                    .flat_map(|block| block.body.iter())
                    .filter_map(|expr| match expr {
                        InstrCodegen::IncrementCounter(op) => Some(op.counter_id.0),
                        _ => None,
                    })
                    .max()
                    .map_or(0, |max_counter_id| max_counter_id + 1),
            )
        });
    let counter_defs = runtime_state
        .map(|state| state.lowered_module.counter_defs.as_slice())
        .unwrap_or(&[]);
    let built = build_cranelift_run_bb_specialized_function(
        &mut jit_module,
        blocks,
        module,
        function,
        module_constants,
        counter_defs,
        &module_constant_ptrs,
        &counter_ptrs,
        compile_session,
        runtime_state,
        None,
        None,
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
    inline_runtime_support_calls(
        jit_module,
        &mut ctx,
        "failed to render specialized jit run_bb function",
    )?;
    let mut ctrl_plane = ControlPlane::default();
    ctx.optimize(jit_module.isa(), &mut ctrl_plane)
        .map_err(|err| format!("failed to optimize specialized jit run_bb function: {err:?}"))?;

    let cfg_dot = CFGPrinter::new(&ctx.func).to_string();

    let mut clif = String::new();
    clif.push_str("; ---- post-opt CLIF fed to Cranelift backend ----\n");
    let clif_display =
        rewrite_import_fn_aliases(ctx.func.display().to_string().as_str(), import_id_to_symbol);
    clif.push_str(&rewrite_block_header_annotations(
        &clif_display,
        block_annotations,
    ));

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
    counter_ptrs: &[*mut u64],
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
            counter_ptrs,
            direct_call_resolver,
        )
    }
}

fn compiled_direct_runner_info(compiled_handle: ObjPtr) -> Result<(*const u8, usize), String> {
    if compiled_handle.is_null() {
        return Err("invalid null compiled handle for direct vectorcall trampoline".to_string());
    }
    let compiled = unsafe { &*(compiled_handle as *const CompiledSpecializedRunner) };
    match compiled.entry {
        Some(CompiledRunnerEntry::Direct {
            code_ptr,
            param_count,
        }) => Ok((code_ptr, param_count)),
        None => Err("invalid compiled handle without entrypoint".to_string()),
    }
}

pub(crate) fn compiled_direct_code_ptr(compiled_handle: ObjPtr) -> Result<ObjPtr, String> {
    compiled_direct_runner_info(compiled_handle).map(|(code_ptr, _)| code_ptr as ObjPtr)
}

fn define_shared_vectorcall_trampoline(
    compile_session: &crate::session::CompileSession,
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
        let function_extra_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_VECTORCALL_FUNCTION_EXTRA_IMPORT,
        );
        let enter_recursive_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_ENTER_RECURSIVE_CALL_IMPORT,
        );
        let leave_recursive_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_LEAVE_RECURSIVE_CALL_IMPORT,
        );
        let decref_ref = func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_DECREF_IMPORT);
        let thread_state_get_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_PY_THREAD_STATE_GET_IMPORT);
        let function_env_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_VECTORCALL_FUNCTION_ENV_IMPORT,
        );
        let set_raised_exception_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT,
        );

        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let function_extra_inst = fb.ins().call(function_extra_ref, &[callable_val]);
        let function_extra_val = fb.inst_results(function_extra_inst)[0];
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
        let function_env_inst = fb
            .ins()
            .call(function_env_ref, &[callable_val, function_extra_val]);
        let function_env_val = fb.inst_results(function_env_inst)[0];
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
        fb.ins().call(leave_recursive_ref, &[thread_state_val]);
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
        let callee_ptr = load_function_env_obj(
            &mut fb,
            ptr_ty,
            function_env_val,
            FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET,
        );
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
        fb.ins().call(leave_recursive_ref, &[thread_state_val]);
        fb.ins()
            .call(set_raised_exception_ref, &[thread_state_val, error_value]);
        fb.ins().return_(&[result]);

        fb.switch_to_block(direct_ok_block);
        for value in owned_args {
            fb.ins().call(decref_ref, &[thread_state_val, value]);
        }
        fb.ins().call(leave_recursive_ref, &[thread_state_val]);
        fb.ins().return_(&[result]);
        fb.seal_all_blocks();
        fb.finalize();
    }

    let main_artifact = define_function_with_incremental_cache(
        compile_session,
        jit_module,
        main_id,
        &mut ctx,
        &format!("direct-vectorcall-trampoline:{param_count}"),
        CraneliftCompileCachePolicy::Enabled,
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
