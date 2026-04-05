use crate::SOAC_RUNTIME_CLIF;
use crate::module_constants::{ModuleCodegenConstants, ModuleConstantId};
use cranelift_codegen::cfg_printer::CFGPrinter;
use cranelift_codegen::incremental_cache::CacheKvStore;
use cranelift_codegen::inline::{Inline, InlineCommand};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_control::ControlPlane;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, ModuleReloc};
use cranelift_reader::parse_functions;
use pyo3::ffi;
use soac_blockpy::block_py::{
    AbruptKind, BlockArg, BlockPyFunction, BlockPyModule, BlockTerm, CallArgKeyword,
    CallArgPositional, CellLocation, CodegenBlock, CodegenBlockPyExpr, CounterDef, CounterId,
    ChildVisitable, CounterScope, CounterSite, FunctionId, HasMeta, InstrId, LocalLocation,
    ResolvedName, NameLocation, ParamDefaultSource, StorageLayout, Visit, WithMeta, BlockLabel,
    operation as blockpy_intrinsics,
};
use soac_blockpy::passes::CodegenBlockPyPass;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

mod intrinsics;
mod planning;
mod specialized_helpers;
mod vmctx;

pub use planning::{
    BlockExcDispatchPlan, exc_dispatch_plan, jit_param_names_for_block, lookup_blockpy_function,
    lookup_blockpy_module, register_clif_module_plans,
};
pub use specialized_helpers::ObjPtr;
use specialized_helpers::register_specialized_jit_symbols;
use vmctx::{
    DELETED_OBJ_OFFSET, EMPTY_TUPLE_OBJ_OFFSET, FALSE_OBJ_OFFSET, GLOBAL_SLOTS_OFFSET,
    GLOBALS_OBJ_OFFSET, NONE_OBJ_OFFSET, TRUE_OBJ_OFFSET,
};
pub use vmctx::{JitModuleVmCtx, ModuleRuntimeContext};

static INCREMENTAL_CLIF_CACHE: OnceLock<Mutex<HashMap<Vec<u8>, Vec<u8>>>> = OnceLock::new();
static RUNTIME_SUPPORT_LIBRARY: OnceLock<Result<RuntimeSupportLibrary, String>> = OnceLock::new();
static NEXT_IMPORT_SPEC_ID: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    fn _Py_Dealloc(obj: *mut ffi::PyObject);
}

fn py_dealloc_symbol() -> *const u8 {
    _Py_Dealloc as *const u8
}

fn incremental_clif_cache() -> &'static Mutex<HashMap<Vec<u8>, Vec<u8>>> {
    INCREMENTAL_CLIF_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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

struct GlobalIncrementalCacheStore<'a> {
    map: &'a Mutex<HashMap<Vec<u8>, Vec<u8>>>,
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
static DP_JIT_DECREF_IMPORT: ImportSpec =
    ImportSpec::local(SOAC_RUNTIME_DECREF_SYMBOL, &[SigType::Pointer], &[]);
static DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_py_call_positional_three",
    &[
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
    ],
    &[SigType::Pointer],
);
static DP_JIT_PY_CALL_WITH_KW_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_py_call_with_kw",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_GET_RAISED_EXCEPTION_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_get_raised_exception", &[], &[SigType::Pointer]);
static DP_JIT_LOAD_GLOBAL_OBJ_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_load_global_obj",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
    ],
    &[SigType::Pointer],
);
static DP_JIT_LOAD_RUNTIME_OBJ_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_load_runtime_obj",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_FUNCTION_CLOSURE_CELL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_function_closure_cell",
    &[SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
static DP_JIT_FUNCTION_POSITIONAL_DEFAULT_OBJ_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_function_positional_default_obj",
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
static DP_JIT_FUNCTION_KWONLY_DEFAULT_OBJ_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_function_kwonly_default_obj",
    &[SigType::Pointer, SigType::Pointer],
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
static DP_JIT_CALLEE_FUNCTION_ID_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_callee_function_id", &[SigType::Pointer], &[SigType::I64]);
static DP_JIT_RECORD_COUNTER_VALUE_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_record_counter_value",
    &[SigType::Pointer, SigType::I64, SigType::I64],
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

impl CacheKvStore for GlobalIncrementalCacheStore<'_> {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        let map = self.map.lock().ok()?;
        map.get(key).map(|value| Cow::Owned(value.clone()))
    }

    fn insert(&mut self, key: &[u8], val: Vec<u8>) {
        if let Ok(mut map) = self.map.lock() {
            map.insert(key.to_vec(), val);
        }
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
    import_id_to_symbol: HashMap<u32, &'static str>,
    block_annotations: ClifBlockDisplayAnnotations,
}

struct CompiledSpecializedRunner {
    _jit_module: JITModule,
    entry: Option<CompiledRunnerEntry>,
}

pub type VectorcallEntryFn = unsafe extern "C" fn(ObjPtr, *const ObjPtr, usize, ObjPtr) -> ObjPtr;

struct CompiledVectorcallRunner {
    _jit_module: JITModule,
}

#[derive(Clone, Copy)]
enum CompiledRunnerEntry {
    Direct {
        code_ptr: *const u8,
        param_count: usize,
    },
}

fn codegen_expr_is_borrowable(
    expr: &CodegenBlockPyExpr,
    local_names: &[String],
    stack_slots: &StackSlots,
    storage_layout: Option<&StorageLayout>,
) -> bool {
    match expr {
        CodegenBlockPyExpr::Load(op) => op
            .name
            .local_location()
            .and_then(|location| storage_layout?.stack_slots().get(location.slot() as usize))
            .is_some_and(|name| {
                local_names.iter().any(|candidate| candidate == name) || stack_slots.has_name(name)
            }),
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
    if let Some(slot_value) = load_stack_slot_value(
        fb,
        &ctx.stack_slots,
        name,
        ctx.consts.ptr_ty,
        borrowed,
        ctx.incref_ref,
    ) {
        return slot_value;
    }
    panic!("missing local {name} in direct JIT state");
}

fn emit_codegen_located_name_load(
    fb: &mut FunctionBuilder<'_>,
    name: &ResolvedName,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    borrowed: bool,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

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
            let value_inst = fb.ins().call(ctx.load_cell_ref, &[cell_obj]);
            let value = fb.inst_results(value_inst)[0];
            fb.ins().call(ctx.decref_ref, &[cell_obj]);
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
        NameLocation::Constant(index) => {
            assert!(
                !borrowed,
                "constant-backed name loads must produce owned references"
            );
            emit_owned_module_constant(fb, ModuleConstantId(index as usize), ctx)
        }
        NameLocation::Cell(_) => {
            unreachable!("all cell location cases should be handled above");
        }
        NameLocation::Global(slot) => {
            let globals_obj = ctx.consts.block_const;
            let global_slots = ctx.consts.global_slots_const;
            let slot_offset = i64::from(slot.slot()) * i64::from(ptr_ty.bytes());
            let slot_addr = fb.ins().iadd_imm(global_slots, slot_offset);
            let cached = fb.ins().load(ptr_ty, ir::MemFlags::trusted(), slot_addr, 0);
            let cached_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, cached, null_ptr);
            let cached_hit_block = fb.create_block();
            let slowpath_block = fb.create_block();
            let value_ok_block = fb.create_block();
            fb.append_block_param(value_ok_block, ptr_ty);
            fb.ins()
                .brif(cached_is_null, slowpath_block, &[], cached_hit_block, &[]);

            fb.switch_to_block(cached_hit_block);
            if let Some(counter_ptr) = ctx.consts.global_load_hit_counter_ptr {
                emit_increment_counter_ptr(fb, ptr_ty, counter_ptr);
            }
            fb.ins().call(ctx.incref_ref, &[cached]);
            fb.ins()
                .jump(value_ok_block, &[ir::BlockArg::Value(cached)]);

            fb.switch_to_block(slowpath_block);
            if let Some(counter_ptr) = ctx.consts.global_load_miss_counter_ptr {
                emit_increment_counter_ptr(fb, ptr_ty, counter_ptr);
            }
            let name_obj = emit_owned_module_constant(
                fb,
                ctx.module_constants
                    .require_unicode_constant_id(name.id.as_str()),
                ctx,
            );
            let slot_index = fb.ins().iconst(ir::types::I64, i64::from(slot.slot()));
            let value_inst = fb.ins().call(
                ctx.load_global_obj_ref,
                &[globals_obj, global_slots, name_obj, slot_index],
            );
            fb.ins().call(ctx.decref_ref, &[name_obj]);
            let value = fb.inst_results(value_inst)[0];
            let value_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
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
        NameLocation::RuntimeName => {
            let name_obj = emit_owned_module_constant(
                fb,
                ctx.module_constants
                    .require_unicode_constant_id(name.id.as_str()),
                ctx,
            );
            let value_inst = fb.ins().call(ctx.load_runtime_obj_ref, &[name_obj]);
            fb.ins().call(ctx.decref_ref, &[name_obj]);
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
            fb.block_params(value_ok_block)[0]
        }
    }
}

fn codegen_expr_const_string(
    expr: &CodegenBlockPyExpr,
    module_constants: &ModuleCodegenConstants,
) -> Option<String> {
    match expr {
        CodegenBlockPyExpr::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_string_value(ModuleConstantId(index as usize))
        }),
        CodegenBlockPyExpr::Call(call) => {
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
    expr: &'a CodegenBlockPyExpr,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a str> {
    match expr {
        CodegenBlockPyExpr::Load(op)
            if op.name.location.is_global() || op.name.location.is_runtime_name() =>
        {
            Some(op.name.id.as_str())
        }
        CodegenBlockPyExpr::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_runtime_name_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn load_vmctx_obj(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    vmctx_value: ir::Value,
    offset: i32,
) -> ir::Value {
    fb.ins()
        .load(ptr_ty, ir::MemFlags::trusted(), vmctx_value, offset)
}

struct JitEmitConsts {
    step_null_block: ir::Block,
    step_null_args: Vec<ir::Value>,
    ptr_ty: ir::Type,
    i64_ty: ir::Type,
    vmctx_value: ir::Value,
    callable_value: ir::Value,
    none_const: ir::Value,
    true_const: ir::Value,
    false_const: ir::Value,
    deleted_const: ir::Value,
    empty_tuple_const: ir::Value,
    block_const: ir::Value,
    global_slots_const: ir::Value,
    global_load_hit_counter_ptr: Option<*mut u64>,
    global_load_miss_counter_ptr: Option<*mut u64>,
}

struct JitEmitCtx<'mc> {
    module: &'mc BlockPyModule<CodegenBlockPyPass>,
    module_constants: &'mc ModuleCodegenConstants,
    module_constant_ptrs: &'mc [*mut ffi::PyObject],
    counter_ptrs: &'mc [*mut u64],
    storage_layout: Option<StorageLayout>,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
    py_call_positional_three_ref: ir::FuncRef,
    py_vectorcall_ref: ir::FuncRef,
    consts: JitEmitConsts,
    load_global_obj_ref: ir::FuncRef,
    load_runtime_obj_ref: ir::FuncRef,
    function_closure_cell_ref: ir::FuncRef,
    pyobject_getattr_ref: ir::FuncRef,
    pyobject_setattr_ref: ir::FuncRef,
    pyobject_getitem_ref: ir::FuncRef,
    pyobject_setitem_ref: ir::FuncRef,
    raise_deleted_name_error_ref: ir::FuncRef,
    make_cell_ref: ir::FuncRef,
    load_cell_ref: ir::FuncRef,
    store_cell_ref: ir::FuncRef,
    py_call_object_ref: ir::FuncRef,
    py_call_with_kw_ref: ir::FuncRef,
    callee_function_id_ref: ir::FuncRef,
    record_counter_value_ref: ir::FuncRef,
    tuple_new_ref: ir::FuncRef,
    tuple_set_item_ref: ir::FuncRef,
    stack_slots: StackSlots,
    direct_call_code_ptrs: &'mc HashMap<FunctionId, ObjPtr>,
    call_target_counter_ids: &'mc HashMap<InstrId, CounterId>,
    call_target_specializations: &'mc HashMap<InstrId, Vec<FunctionId>>,
    call_direct_hit_counter_ids: &'mc HashMap<InstrId, CounterId>,
    call_direct_fallback_counter_ids: &'mc HashMap<InstrId, CounterId>,
}

struct CodegenIntrinsicEmitState<'a, 'b, 'mc, 'c, 'd> {
    fb: &'a mut FunctionBuilder<'b>,
    local_names: &'c mut Vec<String>,
    local_values: &'c mut Vec<ir::Value>,
    ctx: &'c JitEmitCtx<'mc>,
    jit_module: &'a mut JITModule,
    func_imports: &'a mut FuncBuildImports<'d>,
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

    fn initialize_all_to_value(
        &self,
        fb: &mut FunctionBuilder<'_>,
        value: ir::Value,
        incref_ref: ir::FuncRef,
    ) {
        for slot in &self.slots {
            fb.ins().call(incref_ref, &[value]);
            fb.ins().stack_store(value, *slot, 0);
        }
    }

    fn replace_cloned_value(
        &self,
        fb: &mut FunctionBuilder<'_>,
        name: &str,
        value: ir::Value,
        ptr_ty: ir::Type,
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
    ) -> Option<()> {
        let slot = self.slot_for_name(name)?;
        let previous = fb.ins().stack_load(ptr_ty, slot, 0);
        fb.ins().call(incref_ref, &[value]);
        fb.ins().stack_store(value, slot, 0);
        fb.ins().call(decref_ref, &[previous]);
        Some(())
    }

    fn decref_all(&self, fb: &mut FunctionBuilder<'_>, ptr_ty: ir::Type, decref_ref: ir::FuncRef) {
        for slot in &self.slots {
            let value = fb.ins().stack_load(ptr_ty, *slot, 0);
            fb.ins().call(decref_ref, &[value]);
        }
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
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
) {
    if let Some(existing_index) = local_names.iter().position(|candidate| candidate == name) {
        let previous = local_values.remove(existing_index);
        local_names.remove(existing_index);
        fb.ins().call(decref_ref, &[previous]);
    }
    if stack_slots.has_name(name) {
        stack_slots
            .replace_cloned_value(fb, name, value, ptr_ty, incref_ref, decref_ref)
            .expect("slot-backed local missing from stack slots");
        fb.ins().call(decref_ref, &[value]);
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
    deleted_const: ir::Value,
    ptr_ty: ir::Type,
    incref_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
) -> Result<(), String> {
    if let Some(index) = local_names.iter().position(|candidate| candidate == name) {
        let previous = local_values.remove(index);
        local_names.remove(index);
        fb.ins().call(decref_ref, &[previous]);
    } else if !stack_slots.has_name(name) {
        return Err(format!("missing local binding for delete target: {name}"));
    }
    if stack_slots.has_name(name) {
        stack_slots
            .replace_cloned_value(fb, name, deleted_const, ptr_ty, incref_ref, decref_ref)
            .expect("slot-backed delete target missing from stack slots");
    }
    Ok(())
}

impl<'a, 'b, 'mc, 'c, 'd> intrinsics::OperationEmitState<'b, CodegenBlockPyExpr>
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

    fn emit_arg_values(&mut self, args: &[&CodegenBlockPyExpr]) -> Vec<(ir::Value, bool)> {
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
                self.fb.ins().call(self.ctx.decref_ref, &[*value]);
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
        fb.ins().call(incref_ref, &[value]);
    }
    Some(value)
}

fn is_try_exception_alias_name(name: &str) -> bool {
    name.starts_with("_dp_try_exc_")
}

fn local_name_index_for_block_arg(name: &str, local_names: &[String]) -> Option<usize> {
    local_names
        .iter()
        .position(|candidate| candidate == name)
        .or_else(|| {
            if !is_try_exception_alias_name(name) {
                return None;
            }
            let mut matches = local_names
                .iter()
                .enumerate()
                .filter(|(_, candidate)| is_try_exception_alias_name(candidate));
            let first = matches.next().map(|(index, _)| index);
            debug_assert!(
                matches.next().is_none(),
                "expected at most one current-exception block param"
            );
            first
        })
}

fn block_arg_values(values: &[ir::Value]) -> Vec<ir::BlockArg> {
    values.iter().copied().map(ir::BlockArg::Value).collect()
}

fn step_null_block_args(ctx: &JitEmitCtx<'_>) -> Vec<ir::BlockArg> {
    block_arg_values(&ctx.consts.step_null_args)
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

fn lookup_global_runtime_counter_ptr(
    counter_defs: &[CounterDef],
    counter_ptrs: &[*mut u64],
    kind: &str,
) -> Result<Option<*mut u64>, String> {
    lookup_counter_id(
        counter_defs,
        CounterScope::Global,
        kind,
        &CounterSite::Runtime {
            function_id: None,
            instr_id: None,
        },
    )
    .map(|counter_id| counter_ptr_for_id(counter_ptrs, counter_id))
    .transpose()
}

fn build_counted_runtime_refcount_helper(
    jit_module: &mut JITModule,
    symbol_name: &str,
    runtime_import: &'static ImportSpec,
    counter_ptr: *mut u64,
) -> Result<FuncId, String> {
    let ptr_ty = jit_module.target_config().pointer_type();
    let mut sig = jit_module.make_signature();
    sig.params.push(ir::AbiParam::new(ptr_ty));
    let helper_id = declare_local_fn(jit_module, symbol_name, &sig)?;

    let mut ctx = jit_module.make_context();
    ctx.func.signature = sig;
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = fb.create_block();
        fb.append_block_params_for_function_params(entry_block);
        fb.switch_to_block(entry_block);
        let obj = fb.block_params(entry_block)[0];
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
        fb.ins().call(runtime_ref, &[obj]);
        fb.ins().return_(&[]);
        fb.seal_all_blocks();
        fb.finalize();
    }

    define_function_with_incremental_cache(
        jit_module,
        helper_id,
        &mut ctx,
        "failed to define counted runtime refcount helper",
    )?;
    jit_module.clear_context(&mut ctx);
    Ok(helper_id)
}

fn build_counted_runtime_refcount_helpers(
    jit_module: &mut JITModule,
    function: &BlockPyFunction<CodegenBlockPyPass>,
    counter_defs: &[CounterDef],
    counter_ptrs: &[*mut u64],
) -> Result<CountedRefcountHelpers, String> {
    let incref_func_id =
        lookup_runtime_counter_id(counter_defs, function.function_id, "runtime_incref")
            .map(|counter_id| {
                let counter_ptr = counter_ptr_for_id(counter_ptrs, counter_id)?;
                build_counted_runtime_refcount_helper(
                    jit_module,
                    &format!("py:rc:incref:{}", function.names.qualname),
                    &DP_JIT_INCREF_IMPORT,
                    counter_ptr,
                )
            })
            .transpose()?;

    let decref_func_id =
        lookup_runtime_counter_id(counter_defs, function.function_id, "runtime_decref")
            .map(|counter_id| {
                let counter_ptr = counter_ptr_for_id(counter_ptrs, counter_id)?;
                build_counted_runtime_refcount_helper(
                    jit_module,
                    &format!("py:rc:decref:{}", function.names.qualname),
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

fn emit_raw_cell_object_for_location(
    fb: &mut FunctionBuilder<'_>,
    location: CellLocation,
    debug_name: &str,
    local_names: &[String],
    local_values: &[ir::Value],
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let i64_ty = ctx.consts.i64_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

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
            let slot_value = fb.ins().iconst(i64_ty, slot as i64);
            let raw_cell_inst = fb.ins().call(
                ctx.function_closure_cell_ref,
                &[ctx.consts.callable_value, slot_value],
            );
            let raw_cell_value = fb.inst_results(raw_cell_inst)[0];
            let raw_cell_is_null =
                fb.ins()
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
            fb.block_params(raw_cell_ok_block)[0]
        }
    }
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
    fb.ins().call(ctx.decref_ref, &[failed_tuple]);
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(done_block);
    fb.block_params(done_block)[0]
}

fn emit_positional_vectorcall(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&CodegenBlockPyExpr],
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
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
        &[callable, args_ptr, nargsf, null_ptr],
    );
    for (value, borrowed_arg) in arg_values.into_iter().zip(arg_borrowed.into_iter()) {
        if !borrowed_arg {
            fb.ins().call(ctx.decref_ref, &[value]);
        }
    }
    if !callable_is_borrowed {
        fb.ins().call(ctx.decref_ref, &[callable]);
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

fn emit_owned_bool_from_cond(
    fb: &mut FunctionBuilder<'_>,
    cond: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let bool_value = fb
        .ins()
        .select(cond, ctx.consts.true_const, ctx.consts.false_const);
    fb.ins().call(ctx.incref_ref, &[bool_value]);
    bool_value
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
    let is_true = fb.ins().icmp_imm(ir::condcodes::IntCC::NotEqual, result, 0);
    emit_owned_bool_from_cond(fb, is_true, ctx)
}

fn emit_branch_index_i64(
    fb: &mut FunctionBuilder<'_>,
    expr: &CodegenBlockPyExpr,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
    pyobject_to_i64_ref: ir::FuncRef,
) -> ir::Value {
    match expr {
        CodegenBlockPyExpr::CalleeFunctionId(op) => {
            let callable_is_borrowed = codegen_expr_is_borrowable(
                op.value.as_ref(),
                local_names,
                &ctx.stack_slots,
                ctx.storage_layout.as_ref(),
            );
            let callable = emit_codegen_expr(
                fb,
                op.value.as_ref(),
                local_names,
                local_values,
                ctx,
                callable_is_borrowed,
                jit_module,
                func_imports,
            );
            let call_inst = fb.ins().call(ctx.callee_function_id_ref, &[callable]);
            if !callable_is_borrowed {
                fb.ins().call(ctx.decref_ref, &[callable]);
            }
            fb.inst_results(call_inst)[0]
        }
        _ => {
            let index_obj = emit_codegen_expr(
                fb,
                expr,
                local_names,
                local_values,
                ctx,
                false,
                jit_module,
                func_imports,
            );
            let index_i64_inst = fb.ins().call(pyobject_to_i64_ref, &[index_obj]);
            let index_i64 = fb.inst_results(index_i64_inst)[0];
            fb.ins().call(ctx.decref_ref, &[index_obj]);
            index_i64
        }
    }
}

fn collect_call_direct_targets(
    function: &BlockPyFunction<CodegenBlockPyPass>,
) -> HashSet<FunctionId> {
    struct CallDirectTargetCollector<'a> {
        out: &'a mut HashSet<FunctionId>,
    }

    impl Visit<CodegenBlockPyExpr> for CallDirectTargetCollector<'_> {
        fn visit_instr(&mut self, expr: &CodegenBlockPyExpr) {
            if let CodegenBlockPyExpr::CallDirect(call) = expr {
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

fn parse_call_target_specializations_env(
    module_name: &str,
    function_id: FunctionId,
) -> Result<HashMap<InstrId, Vec<FunctionId>>, String> {
    let Ok(raw) = env::var("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS") else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    for entry in raw.split(';').map(str::trim).filter(|entry| !entry.is_empty()) {
        let Some((site, targets)) = entry.split_once('=') else {
            return Err(format!("invalid call target specialization entry: {entry}"));
        };
        let mut site_parts = site.split('|');
        let Some(entry_module_name) = site_parts.next() else {
            return Err(format!("missing module in specialization entry: {entry}"));
        };
        let Some(entry_function_id) = site_parts.next() else {
            return Err(format!("missing function_id in specialization entry: {entry}"));
        };
        let Some(entry_block_label) = site_parts.next() else {
            return Err(format!("missing block label in specialization entry: {entry}"));
        };
        let Some(entry_instr_index) = site_parts.next() else {
            return Err(format!("missing instr index in specialization entry: {entry}"));
        };
        if site_parts.next().is_some() {
            return Err(format!("too many site fields in specialization entry: {entry}"));
        }
        if entry_module_name != module_name {
            continue;
        }
        let parsed_function_id = entry_function_id
            .parse::<u64>()
            .map(FunctionId::from_packed)
            .map_err(|err| format!("invalid function_id in specialization entry {entry}: {err}"))?;
        if parsed_function_id != function_id {
            continue;
        }
        let block_label = entry_block_label
            .parse::<u32>()
            .map_err(|err| format!("invalid block label in specialization entry {entry}: {err}"))?;
        let instr_index = entry_instr_index
            .parse::<u32>()
            .map_err(|err| format!("invalid instr index in specialization entry {entry}: {err}"))?;
        let targets = targets
            .split(',')
            .map(str::trim)
            .filter(|target| !target.is_empty())
            .map(|target| {
                target
                    .parse::<u64>()
                    .map(FunctionId::from_packed)
                    .map_err(|err| {
                        format!("invalid hot target function id in specialization entry {entry}: {err}")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if targets.is_empty() {
            continue;
        }
        out.insert(
            InstrId::new(BlockLabel::from_index(block_label as usize), instr_index),
            targets,
        );
    }
    Ok(out)
}

fn emit_callee_function_id_checked(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    ctx: &JitEmitCtx<'_>,
) -> ir::Value {
    let call_inst = fb.ins().call(ctx.callee_function_id_ref, &[callable]);
    let callee_id = fb.inst_results(call_inst)[0];
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

fn emit_record_call_target_counter(
    fb: &mut FunctionBuilder<'_>,
    counter_id: CounterId,
    callee_id: ir::Value,
    ctx: &JitEmitCtx<'_>,
) {
    let counter_id_value = fb.ins().iconst(ctx.consts.i64_ty, counter_id.0 as i64);
    fb.ins().call(
        ctx.record_counter_value_ref,
        &[ctx.consts.vmctx_value, counter_id_value, callee_id],
    );
}

fn emit_direct_call_resolved(
    fb: &mut FunctionBuilder<'_>,
    callable: ir::Value,
    callable_is_borrowed: bool,
    args: &[&CodegenBlockPyExpr],
    target_function: &BlockPyFunction<CodegenBlockPyPass>,
    target_code_ptr: ObjPtr,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let ptr_ty = ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);
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

    let mut direct_sig = jit_module.make_signature();
    direct_sig.params.push(ir::AbiParam::special(
        ptr_ty,
        ir::ArgumentPurpose::VMContext,
    ));
    direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    for _ in target_function.params.iter() {
        direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    }
    direct_sig.returns.push(ir::AbiParam::new(ptr_ty));
    let direct_sig_ref = fb.import_signature(direct_sig);

    let mut call_args = Vec::with_capacity(arg_values.len() + 2);
    call_args.push(ctx.consts.vmctx_value);
    call_args.push(callable);
    call_args.extend(arg_values.iter().copied());

    let callee_ptr = fb.ins().iconst(ptr_ty, target_code_ptr as i64);
    let call_inst = fb
        .ins()
        .call_indirect(direct_sig_ref, callee_ptr, &call_args);
    let call_value = fb.inst_results(call_inst)[0];

    for (value, borrowed_arg) in arg_values.into_iter().zip(arg_borrowed.into_iter()) {
        if !borrowed_arg {
            fb.ins().call(ctx.decref_ref, &[value]);
        }
    }
    if !callable_is_borrowed {
        fb.ins().call(ctx.decref_ref, &[callable]);
    }

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

fn emit_call_direct_expr(
    fb: &mut FunctionBuilder<'_>,
    call: &soac_blockpy::block_py::CallDirect<CodegenBlockPyExpr>,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let mut fallback = || {
        let fallback = CodegenBlockPyExpr::Call(
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

    let Some(target_function) = ctx
        .module
        .callable_defs
        .iter()
        .find(|function| function.function_id == call.function_id)
    else {
        return fallback();
    };

    let supports_direct_call = call.keywords.is_empty()
        && call.args.len() <= target_function.params.len()
        && call
            .args
            .iter()
            .all(|arg| matches!(arg, CallArgPositional::Positional(_)));
    if !supports_direct_call {
        return fallback();
    }

    let Some(&target_code_ptr) = ctx.direct_call_code_ptrs.get(&call.function_id) else {
        return fallback();
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
            CallArgPositional::Starred(_) => unreachable!(
                "non-positional direct args should have used generic fallback"
            ),
        })
        .collect::<Vec<_>>();
    emit_direct_call_resolved(
        fb,
        callable,
        callable_is_borrowed,
        args.as_slice(),
        target_function,
        target_code_ptr,
        local_names,
        local_values,
        ctx,
        jit_module,
        func_imports,
    )
}

fn emit_codegen_expr(
    fb: &mut FunctionBuilder<'_>,
    expr: &CodegenBlockPyExpr,
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    ctx: &JitEmitCtx<'_>,
    borrowed: bool,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> ir::Value {
    let incref_ref = ctx.incref_ref;
    let decref_ref = ctx.decref_ref;
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
            CodegenBlockPyExpr::Load(op) => {
                return emit_codegen_located_name_load(
                fb,
                &op.name,
                local_names,
                local_values,
                ctx,
                borrowed,
            );
        }
        CodegenBlockPyExpr::IncrementCounter(op) => {
            assert!(
                !borrowed,
                "increment_counter must not request a borrowed result"
            );
            return emit_increment_counter(fb, op.counter_id, ctx);
        }
        expr @ (CodegenBlockPyExpr::BinOp(_)
        | CodegenBlockPyExpr::UnaryOp(_)
        | CodegenBlockPyExpr::CalleeFunctionId(_)
        | CodegenBlockPyExpr::GetAttr(_)
        | CodegenBlockPyExpr::SetAttr(_)
        | CodegenBlockPyExpr::GetItem(_)
        | CodegenBlockPyExpr::SetItem(_)
        | CodegenBlockPyExpr::DelItem(_)
        | CodegenBlockPyExpr::Store(_)
        | CodegenBlockPyExpr::Del(_)
        | CodegenBlockPyExpr::MakeCell(_)
        | CodegenBlockPyExpr::CellRef(_)
        | CodegenBlockPyExpr::MakeFunction(_)) => {
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
            if matches!(expr, CodegenBlockPyExpr::MakeFunction(_)) {
                panic!("MakeFunction should lower to a regular call before codegen");
            }
            if let Some(value) = intrinsics::emit_operation(expr, &mut intrinsic_state) {
                return value;
            }
            match expr {
                CodegenBlockPyExpr::CellRef(op) => emit_raw_cell_object_for_location(
                    intrinsic_state.fb,
                    op.location,
                    "cell_ref",
                    intrinsic_state.local_names,
                    intrinsic_state.local_values,
                    intrinsic_state.ctx,
                ),
                CodegenBlockPyExpr::Store(op) => {
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
                    if location.is_owned()
                        && matches!(op.value.as_ref(), CodegenBlockPyExpr::MakeCell(_))
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
                    intrinsic_state
                        .fb
                        .ins()
                        .call(intrinsic_state.ctx.decref_ref, &[raw_cell]);
                    if !value_borrowed {
                        intrinsic_state
                            .fb
                            .ins()
                            .call(intrinsic_state.ctx.decref_ref, &[value_obj]);
                    }
                    let call_value = intrinsic_state.fb.inst_results(call_inst)[0];
                    intrinsics::OperationEmitState::finish_owned_result(
                        &mut intrinsic_state,
                        call_value,
                    )
                }
                CodegenBlockPyExpr::Del(op) => {
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
                            intrinsic_state.ctx.consts.deleted_const,
                            intrinsic_state.ctx.consts.ptr_ty,
                            intrinsic_state.ctx.incref_ref,
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
                CodegenBlockPyExpr::MakeFunction(_) => {
                    unreachable!("MakeFunction should panic before intrinsic fallback")
                }
                _ => {
                    panic!("operation {expr:?} should have been handled by direct emitter")
                }
            }
        }
        CodegenBlockPyExpr::CallDirect(call) => {
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
        CodegenBlockPyExpr::Call(call) => {
            assert!(
                !borrowed,
                "codegen call expression must not use borrowed result"
            );
            let null_ptr = fb.ins().iconst(ptr_ty, 0);
            let mut simple_args: Vec<&CodegenBlockPyExpr> = Vec::new();
            let mut simple_keywords: Vec<(&str, &CodegenBlockPyExpr)> = Vec::new();
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
                && matches!(
                    codegen_expr_helper_name(call.func.as_ref(), ctx.module_constants),
                    Some("globals")
                )
            {
                fb.ins().call(incref_ref, &[block_const]);
                return block_const;
            }

            if !has_unpack
                && simple_keywords.is_empty()
                && codegen_expr_helper_name(call.func.as_ref(), ctx.module_constants) == Some("str")
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
                let uncached_slot = fb.ins().iconst(ir::types::I64, -1);
                let list_callable_inst = fb.ins().call(
                    ctx.load_global_obj_ref,
                    &[
                        block_const,
                        ctx.consts.global_slots_const,
                        list_name_obj,
                        uncached_slot,
                    ],
                );
                fb.ins().call(decref_ref, &[list_name_obj]);
                let list_callable = fb.inst_results(list_callable_inst)[0];
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
                fb.ins().call(decref_ref, &[list_callable]);
                let args_list = fb.inst_results(args_list_inst)[0];
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
                    let uncached_slot = fb.ins().iconst(ir::types::I64, -1);
                    let dict_callable_inst = fb.ins().call(
                        ctx.load_global_obj_ref,
                        &[
                            block_const,
                            ctx.consts.global_slots_const,
                            dict_name_obj,
                            uncached_slot,
                        ],
                    );
                    fb.ins().call(decref_ref, &[dict_name_obj]);
                    let dict_callable = fb.inst_results(dict_callable_inst)[0];
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
                    fb.ins().call(decref_ref, &[dict_callable]);
                    let kwargs_obj = fb.inst_results(kwargs_inst)[0];
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
                    fb.ins().call(decref_ref, &[method_name_obj]);
                    let method_obj = fb.inst_results(method_inst)[0];
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
                        &[method_obj, value_obj, null_ptr, null_ptr, null_ptr],
                    );
                    if !value_borrowed {
                        fb.ins().call(decref_ref, &[value_obj]);
                    }
                    fb.ins().call(decref_ref, &[method_obj]);
                    let call_value = fb.inst_results(call_inst)[0];
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
                    fb.ins().call(decref_ref, &[call_value]);
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
                            fb.ins().call(decref_ref, &[key_obj]);
                            if !value_borrowed {
                                fb.ins().call(decref_ref, &[value_obj]);
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
                            fb.ins().call(decref_ref, &[failed_kwargs]);
                            fb.ins().call(decref_ref, &[args_list]);
                            if !callable_is_borrowed {
                                fb.ins().call(decref_ref, &[callable]);
                            }
                            fb.ins().jump(step_null_block, &step_null_block_args(ctx));
                            fb.switch_to_block(set_ok);
                            fb.ins().call(decref_ref, &[set_value]);
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
                            fb.ins().call(decref_ref, &[update_name_obj]);
                            let update_obj = fb.inst_results(update_inst)[0];
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
                                &[update_obj, value_obj, null_ptr, null_ptr, null_ptr],
                            );
                            if !value_borrowed {
                                fb.ins().call(decref_ref, &[value_obj]);
                            }
                            fb.ins().call(decref_ref, &[update_obj]);
                            let call_value = fb.inst_results(call_inst)[0];
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
                            fb.ins().call(decref_ref, &[call_value]);
                        }
                    }
                }

                let tuple_name_obj = emit_owned_module_constant(
                    fb,
                    ctx.module_constants
                        .require_unicode_constant_id("tuple_from_iter"),
                    ctx,
                );
                let uncached_slot = fb.ins().iconst(ir::types::I64, -1);
                let tuple_callable_inst = fb.ins().call(
                    ctx.load_global_obj_ref,
                    &[
                        block_const,
                        ctx.consts.global_slots_const,
                        tuple_name_obj,
                        uncached_slot,
                    ],
                );
                fb.ins().call(decref_ref, &[tuple_name_obj]);
                let tuple_callable = fb.inst_results(tuple_callable_inst)[0];
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
                    &[tuple_callable, args_list, null_ptr, null_ptr, null_ptr],
                );
                fb.ins().call(decref_ref, &[tuple_callable]);
                fb.ins().call(decref_ref, &[args_list]);
                let call_args_tuple = fb.inst_results(tuple_call_inst)[0];
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
                    let call_inst = fb.ins().call(
                        py_call_with_kw_ref,
                        &[callable, call_args_tuple, kwargs_obj],
                    );
                    fb.ins().call(decref_ref, &[kwargs_obj]);
                    call_inst
                } else {
                    fb.ins()
                        .call(py_call_object_ref, &[callable, call_args_tuple])
                };
                fb.ins().call(decref_ref, &[call_args_tuple]);
                if !callable_is_borrowed {
                    fb.ins().call(decref_ref, &[callable]);
                }
                let call_value = fb.inst_results(call_inst)[0];
                let call_is_null = fb
                    .ins()
                    .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
                let call_ok_block = fb.create_block();
                fb.append_block_param(call_ok_block, ptr_ty);
                fb.ins().brif(
                    call_is_null,
                    step_null_block,
                    &step_null_block_args(ctx),
                    call_ok_block,
                    &[ir::BlockArg::Value(call_value)],
                );
                fb.switch_to_block(call_ok_block);
                return fb.block_params(call_ok_block)[0];
            }

            if let Some(func_name) =
                codegen_expr_helper_name(call.func.as_ref(), ctx.module_constants)
            {
                if keywords.is_empty() && func_name == "str" && args.len() == 1 {
                    if let Some(value) = codegen_expr_const_string(args[0], ctx.module_constants) {
                        return emit_owned_module_constant(
                            fb,
                            ctx.module_constants
                                .require_unicode_constant_id(value.as_str()),
                            ctx,
                        );
                    }
                }
                if keywords.is_empty() && args.is_empty() && func_name == "globals" {
                    fb.ins().call(incref_ref, &[block_const]);
                    return block_const;
                }
                if keywords.is_empty() {
                    if func_name == "tuple_values" {
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
                                fb.ins().call(decref_ref, &[value]);
                            }
                        }
                        return tuple_value;
                    }
                    if func_name == "load_deleted_name" && args.len() == 2 {
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
                            let value_is_deleted = fb.ins().icmp(
                                ir::condcodes::IntCC::Equal,
                                value_obj,
                                deleted_const,
                            );
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
                            fb.ins().call(decref_ref, &[name_obj]);
                            if !value_borrowed {
                                fb.ins().call(decref_ref, &[value_obj]);
                            }
                            fb.ins().jump(step_null_block, &step_null_block_args(ctx));

                            fb.switch_to_block(value_ok_block);
                            let value_obj = fb.block_params(value_ok_block)[0];
                            fb.ins().call(decref_ref, &[name_obj]);
                            if value_borrowed {
                                fb.ins().call(incref_ref, &[value_obj]);
                            }
                            return value_obj;
                        }
                    }
                    if func_name == "cell_ref" && args.len() == 1 {
                        let cell_expr = &args[0];
                        let CodegenBlockPyExpr::Load(cell_name) = cell_expr else {
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
            if keywords.is_empty() {
                let site_instr_id = call.meta().instr_id;
                let counter_id =
                    site_instr_id.and_then(|instr_id| ctx.call_target_counter_ids.get(&instr_id).copied());
                let direct_hit_counter_id = site_instr_id
                    .and_then(|instr_id| ctx.call_direct_hit_counter_ids.get(&instr_id).copied());
                let direct_fallback_counter_id = site_instr_id.and_then(|instr_id| {
                    ctx.call_direct_fallback_counter_ids
                        .get(&instr_id)
                        .copied()
                });
                let direct_specializations = site_instr_id
                    .and_then(|instr_id| ctx.call_target_specializations.get(&instr_id))
                    .map(|targets| {
                        targets
                            .iter()
                            .copied()
                            .filter_map(|function_id| {
                                let target_function = ctx
                                    .module
                                    .callable_defs
                                    .iter()
                                    .find(|function| function.function_id == function_id)?;
                                if args.len() != target_function.params.len() {
                                    return None;
                                }
                                let &target_code_ptr = ctx.direct_call_code_ptrs.get(&function_id)?;
                                Some((function_id, target_code_ptr))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                if counter_id.is_some() || !direct_specializations.is_empty() {
                    let callee_id = emit_callee_function_id_checked(fb, callable, ctx);
                    if let Some(counter_id) = counter_id {
                        emit_record_call_target_counter(fb, counter_id, callee_id, ctx);
                    }
                    if !direct_specializations.is_empty() {
                        let result_block = fb.create_block();
                        fb.append_block_param(result_block, ptr_ty);
                        let generic_block = fb.create_block();
                        for (index, &(function_id, target_code_ptr)) in
                            direct_specializations.iter().enumerate()
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
                                function_id.packed() as i64,
                            );
                            fb.ins()
                                .brif(is_match, direct_block, &[], miss_block, &[]);

                            fb.switch_to_block(direct_block);
                            let target_function = ctx
                                .module
                                .callable_defs
                                .iter()
                                .find(|function| function.function_id == function_id)
                                .expect("direct specialization target should exist");
                            if let Some(counter_id) = direct_hit_counter_id {
                                let _ = emit_increment_counter(fb, counter_id, ctx);
                            }
                            let direct_result = emit_direct_call_resolved(
                                fb,
                                callable,
                                callable_is_borrowed,
                                args.as_slice(),
                                target_function,
                                target_code_ptr,
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

                        fb.switch_to_block(generic_block);
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
                fb.ins().call(decref_ref, &[failed_tuple]);
                fb.ins().jump(step_null_block, &step_null_block_args(ctx));
                fb.switch_to_block(set_ok_block);
            }
            let call_inst = if keywords.is_empty() {
                fb.ins()
                    .call(py_call_object_ref, &[callable, call_args_tuple])
            } else {
                let dict_name_obj = emit_owned_module_constant(
                    fb,
                    ctx.module_constants.require_unicode_constant_id("dict"),
                    ctx,
                );
                let uncached_slot = fb.ins().iconst(ir::types::I64, -1);
                let dict_callable_inst = fb.ins().call(
                    ctx.load_global_obj_ref,
                    &[
                        block_const,
                        ctx.consts.global_slots_const,
                        dict_name_obj,
                        uncached_slot,
                    ],
                );
                fb.ins().call(decref_ref, &[dict_name_obj]);
                let dict_callable = fb.inst_results(dict_callable_inst)[0];
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
                fb.ins().call(decref_ref, &[empty_tuple]);
                fb.ins().call(decref_ref, &[dict_callable]);
                let kwargs_obj = fb.inst_results(kwargs_inst)[0];
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
                    fb.ins().call(decref_ref, &[key_obj]);
                    if !value_borrowed {
                        fb.ins().call(decref_ref, &[value_obj]);
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
                    fb.ins().call(decref_ref, &[failed_kwargs]);
                    fb.ins().call(decref_ref, &[call_args_tuple]);
                    if !callable_is_borrowed {
                        fb.ins().call(decref_ref, &[callable]);
                    }
                    fb.ins().jump(step_null_block, &step_null_block_args(ctx));
                    fb.switch_to_block(set_ok);
                    fb.ins().call(decref_ref, &[set_value]);
                }

                let call_inst = fb.ins().call(
                    py_call_with_kw_ref,
                    &[callable, call_args_tuple, kwargs_obj],
                );
                fb.ins().call(decref_ref, &[kwargs_obj]);
                call_inst
            };
            fb.ins().call(decref_ref, &[call_args_tuple]);
            if !callable_is_borrowed {
                fb.ins().call(decref_ref, &[callable]);
            }
            let call_value = fb.inst_results(call_inst)[0];
            let call_is_null = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, call_value, null_ptr);
            let call_ok_block = fb.create_block();
            fb.append_block_param(call_ok_block, ptr_ty);
            fb.ins().brif(
                call_is_null,
                step_null_block,
                &step_null_block_args(ctx),
                call_ok_block,
                &[ir::BlockArg::Value(call_value)],
            );
            fb.switch_to_block(call_ok_block);
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

fn emit_prepare_target_args_codegen(
    fb: &mut FunctionBuilder<'_>,
    target_params: &[String],
    full_target_params: Option<&[String]>,
    explicit_args: Option<&[BlockArg]>,
    local_names: &[String],
    local_values: &[ir::Value],
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
                    if let Some(value_index) =
                        local_name_index_for_block_arg(source_name, local_names)
                    {
                        let value = local_values[value_index];
                        let forwarded_count =
                            forwarded_local_indices.entry(value_index).or_insert(0usize);
                        if *forwarded_count > 0 {
                            fb.ins().call(ctx.incref_ref, &[value]);
                        }
                        *forwarded_count += 1;
                        value
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
        if let Some(value_index) = local_names.iter().position(|candidate| candidate == name) {
            let value = local_values[value_index];
            let forwarded_count = forwarded_local_indices.entry(value_index).or_insert(0usize);
            if *forwarded_count > 0 {
                fb.ins().call(ctx.incref_ref, &[value]);
            }
            *forwarded_count += 1;
            args.push(ir::BlockArg::Value(value));
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

fn emit_explicit_target_slot_writes_codegen(
    fb: &mut FunctionBuilder<'_>,
    full_target_params: &[String],
    runtime_target_params: &[String],
    explicit_args: &[BlockArg],
    local_names: &[String],
    local_values: &[ir::Value],
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
                if let Some(index) = local_name_index_for_block_arg(source_name, local_names) {
                    (local_values[index], false)
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
                ctx.incref_ref,
                ctx.decref_ref,
            )
            .expect("explicit edge slot target missing from stack slots");
        if owned_value {
            fb.ins().call(ctx.decref_ref, &[value]);
        }
    }
    Some(())
}

fn emit_exception_dispatch_slot_writes(
    fb: &mut FunctionBuilder<'_>,
    slot_writes: &[(String, BlockArg)],
    dispatch_exc: ir::Value,
    stack_slots: &StackSlots,
    ptr_ty: ir::Type,
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
            .replace_cloned_value(fb, target_name, value, ptr_ty, incref_ref, decref_ref)
            .expect("exception dispatch slot target missing from stack slots");
    }
    Ok(())
}

fn emit_decref_unforwarded_locals(
    fb: &mut FunctionBuilder<'_>,
    local_values: &[ir::Value],
    local_names: &[String],
    target_params: &[String],
    decref_ref: ir::FuncRef,
) {
    let mut forwarded_local_indices = HashMap::new();
    for name in target_params {
        if let Some(index) = local_names.iter().position(|candidate| candidate == name) {
            *forwarded_local_indices.entry(index).or_insert(0usize) += 1;
        }
    }
    for (index, value) in local_values.iter().enumerate() {
        if forwarded_local_indices.contains_key(&index) {
            continue;
        }
        fb.ins().call(decref_ref, &[*value]);
    }
}

fn emit_truthy_from_owned(
    fb: &mut FunctionBuilder<'_>,
    owned_value: ir::Value,
    is_true_ref: ir::FuncRef,
    decref_ref: ir::FuncRef,
    step_null_block: ir::Block,
    step_null_args: &[ir::Value],
    i32_ty: ir::Type,
) -> ir::Value {
    let truth_inst = fb.ins().call(is_true_ref, &[owned_value]);
    let truth_value = fb.inst_results(truth_inst)[0];
    fb.ins().call(decref_ref, &[owned_value]);
    let truth_error = fb.ins().iconst(i32_ty, -1);
    let is_error = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, truth_value, truth_error);
    let truth_ok_block = fb.create_block();
    fb.append_block_param(truth_ok_block, i32_ty);
    fb.ins().brif(
        is_error,
        step_null_block,
        &block_arg_values(step_null_args),
        truth_ok_block,
        &[ir::BlockArg::Value(truth_value)],
    );
    fb.switch_to_block(truth_ok_block);
    let truth_ok_value = fb.block_params(truth_ok_block)[0];
    let zero_i32 = fb.ins().iconst(i32_ty, 0);
    fb.ins().icmp(
        ir::condcodes::IntCC::SignedGreaterThan,
        truth_ok_value,
        zero_i32,
    )
}

fn emit_codegen_ops(
    fb: &mut FunctionBuilder<'_>,
    ops: &[CodegenBlockPyExpr],
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    _stack_slots: &StackSlots,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    for expr in ops {
        let value = emit_codegen_expr(
            fb,
            expr,
            local_names,
            local_values,
            emit_ctx,
            false,
            jit_module,
            func_imports,
        );
        fb.ins().call(emit_ctx.decref_ref, &[value]);
    }
    Ok(())
}

fn emit_codegen_term(
    fb: &mut FunctionBuilder<'_>,
    block_label: &str,
    term: &BlockTerm<CodegenBlockPyExpr>,
    exec_blocks: &[ir::Block],
    runtime_block_param_names: &[Vec<String>],
    full_block_param_names: &[Vec<String>],
    local_names: &mut Vec<String>,
    local_values: &mut Vec<ir::Value>,
    emit_ctx: &JitEmitCtx<'_>,
    jit_module: &mut JITModule,
    func_imports: &mut FuncBuildImports<'_>,
    is_true_ref: ir::FuncRef,
    pyobject_to_i64_ref: ir::FuncRef,
    raise_exc_ref: ir::FuncRef,
) -> Result<(), String> {
    let decref_ref = emit_ctx.decref_ref;
    let i64_ty = emit_ctx.consts.i64_ty;
    let i32_ty = ir::types::I32;
    let ptr_ty = emit_ctx.consts.ptr_ty;
    let null_ptr = fb.ins().iconst(ptr_ty, 0);

    match term {
        BlockTerm::Jump(target_label) => {
            let target_index = target_label.target.index();
            let target_params = &runtime_block_param_names[target_index];
            let full_target_params = &full_block_param_names[target_index];
            emit_explicit_target_slot_writes_codegen(
                fb,
                full_target_params,
                target_params,
                &target_label.args,
                local_names,
                local_values,
                emit_ctx,
                jit_module,
                func_imports,
            )
            .ok_or_else(|| {
                format!("missing local mapping for jump slot updates in block {block_label}")
            })?;
            let mut jump_args = Vec::with_capacity(target_params.len());
            jump_args.extend(
                emit_prepare_target_args_codegen(
                    fb,
                    target_params,
                    Some(full_target_params),
                    Some(&target_label.args),
                    local_names,
                    local_values,
                    emit_ctx,
                    jit_module,
                    func_imports,
                )
                .ok_or_else(|| {
                    format!("missing local mapping for jump block params in block {block_label}")
                })?,
            );
            emit_decref_unforwarded_locals(
                fb,
                local_values,
                local_names,
                target_params,
                decref_ref,
            );
            fb.ins().jump(exec_blocks[target_index], &jump_args);
        }
        BlockTerm::IfTerm(if_term) => {
            let test_value = emit_codegen_expr(
                fb,
                &if_term.test,
                local_names,
                local_values,
                emit_ctx,
                false,
                jit_module,
                func_imports,
            );
            let is_true = emit_truthy_from_owned(
                fb,
                test_value,
                is_true_ref,
                decref_ref,
                emit_ctx.consts.step_null_block,
                &emit_ctx.consts.step_null_args,
                i32_ty,
            );

            let then_branch = fb.create_block();
            let else_branch = fb.create_block();
            fb.ins().brif(is_true, then_branch, &[], else_branch, &[]);

            fb.switch_to_block(then_branch);
            let then_index = if_term.then_label.index();
            let then_params = &runtime_block_param_names[then_index];
            let mut then_jump_args = Vec::with_capacity(then_params.len());
            then_jump_args.extend(
                emit_prepare_target_args_codegen(
                    fb,
                    then_params,
                    None,
                    None,
                    local_names,
                    local_values,
                    emit_ctx,
                    jit_module,
                    func_imports,
                )
                .ok_or_else(|| {
                    format!(
                        "missing local mapping for then-branch block params in block {block_label}"
                    )
                })?,
            );
            emit_decref_unforwarded_locals(fb, local_values, local_names, then_params, decref_ref);
            fb.ins().jump(exec_blocks[then_index], &then_jump_args);

            fb.switch_to_block(else_branch);
            let else_index = if_term.else_label.index();
            let else_params = &runtime_block_param_names[else_index];
            let mut else_jump_args = Vec::with_capacity(else_params.len());
            else_jump_args.extend(
                emit_prepare_target_args_codegen(
                    fb,
                    else_params,
                    None,
                    None,
                    local_names,
                    local_values,
                    emit_ctx,
                    jit_module,
                    func_imports,
                )
                .ok_or_else(|| {
                    format!(
                        "missing local mapping for else-branch block params in block {block_label}"
                    )
                })?,
            );
            emit_decref_unforwarded_locals(fb, local_values, local_names, else_params, decref_ref);
            fb.ins().jump(exec_blocks[else_index], &else_jump_args);
        }
        BlockTerm::BranchTable(branch) => {
            let index_i64 = emit_branch_index_i64(
                fb,
                &branch.index,
                local_names,
                local_values,
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
                    emit_prepare_target_args_codegen(
                        fb,
                        target_params,
                        None,
                        None,
                        local_names,
                        local_values,
                        emit_ctx,
                        jit_module,
                        func_imports,
                    )
                    .ok_or_else(|| {
                        format!(
                            "missing local mapping for br_table case block params in block {block_label}"
                        )
                    })?,
                );
                emit_decref_unforwarded_locals(
                    fb,
                    local_values,
                    local_names,
                    target_params,
                    decref_ref,
                );
                fb.ins().jump(exec_blocks[target_index], &case_jump_args);
            }

            fb.switch_to_block(default_block);
            let default_index = branch.default_label.index();
            let default_params = &runtime_block_param_names[default_index];
            let mut default_jump_args = Vec::with_capacity(default_params.len());
            default_jump_args.extend(
                emit_prepare_target_args_codegen(
                    fb,
                    default_params,
                    None,
                    None,
                    local_names,
                    local_values,
                    emit_ctx,
                    jit_module,
                    func_imports,
                )
                .ok_or_else(|| {
                    format!(
                        "missing local mapping for br_table default block params in block {block_label}"
                    )
                })?,
            );
            emit_decref_unforwarded_locals(
                fb,
                local_values,
                local_names,
                default_params,
                decref_ref,
            );
            fb.ins()
                .jump(exec_blocks[default_index], &default_jump_args);
        }
        BlockTerm::Return(value) => {
            let ret_value = emit_codegen_expr(
                fb,
                value,
                local_names,
                local_values,
                emit_ctx,
                false,
                jit_module,
                func_imports,
            );
            for value in local_values {
                fb.ins().call(decref_ref, &[*value]);
            }
            emit_ctx.stack_slots.decref_all(fb, ptr_ty, decref_ref);
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
            fb.ins().call(decref_ref, &[raise_name_obj]);
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
                emit_codegen_expr(
                    fb,
                    exc_expr,
                    local_names,
                    local_values,
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
                &[rfo_raise_fn, exc_value, cause_value, null_ptr, null_ptr],
            );
            let raise_exc_obj = fb.inst_results(raise_call_inst)[0];
            fb.ins().call(decref_ref, &[cause_value]);
            fb.ins().call(decref_ref, &[exc_value]);
            fb.ins().call(decref_ref, &[rfo_raise_fn]);
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
            fb.ins().call(decref_ref, &[reo_exc_obj]);
            let raise_rc_fail = fb.create_block();
            let raise_rc_ok = fb.create_block();
            let raise_ok = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, raise_rc, 0);
            fb.ins()
                .brif(raise_ok, raise_rc_ok, &[], raise_rc_fail, &[]);

            fb.switch_to_block(raise_rc_fail);
            fb.ins().jump(
                emit_ctx.consts.step_null_block,
                &step_null_block_args(emit_ctx),
            );

            fb.switch_to_block(raise_rc_ok);
            emit_decref_unforwarded_locals(fb, local_values, local_names, &[], decref_ref);
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
    flag_builder
        .set("opt_level", "speed")
        .map_err(|err| format!("failed to configure Cranelift flags: {err}"))?;
    flag_builder
        .set("is_pic", "false")
        .map_err(|err| format!("failed to configure Cranelift flags: {err}"))?;
    flag_builder
        .set("preserve_frame_pointers", "true")
        .map_err(|err| format!("failed to configure Cranelift flags: {err}"))?;
    let isa_builder = cranelift_native::builder().map_err(|err| format!("{err}"))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|err| format!("failed to finish ISA: {err}"))?;
    let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    builder.symbol("_Py_Dealloc", py_dealloc_symbol());
    register_specialized_jit_symbols(&mut builder);
    Ok(builder)
}

fn new_jit_module() -> Result<JITModule, String> {
    let mut jit_module = JITModule::new(new_jit_builder()?);
    load_runtime_support_clif(&mut jit_module)?;
    Ok(jit_module)
}

fn define_function_with_incremental_cache(
    jit_module: &mut JITModule,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    err_prefix: &str,
) -> Result<(), String> {
    inline_runtime_support_calls(jit_module, ctx, err_prefix)?;
    let func_for_relocs = ctx.func.clone();
    let mut ctrl_plane = ControlPlane::default();
    let mut cache_store = GlobalIncrementalCacheStore {
        map: incremental_clif_cache(),
    };
    let (compiled, _cache_hit) = ctx
        .compile_with_cache(jit_module.isa(), &mut cache_store, &mut ctrl_plane)
        .map_err(|err| format!("{err_prefix}: {err:?}"))?;
    let alignment = compiled.buffer.alignment as u64;
    let relocs = compiled
        .buffer
        .relocs()
        .iter()
        .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &func_for_relocs, func_id))
        .collect::<Vec<_>>();
    jit_module
        .define_function_bytes(func_id, alignment, compiled.code_buffer(), &relocs)
        .map_err(|err| format!("{err_prefix}: {err}"))?;
    Ok(())
}

const RUNTIME_SUPPORT_INLINE_MAX_INSTS: usize = 32;

#[derive(Debug)]
struct RuntimeSupportInliner {
    inlineable: HashMap<ir::UserExternalName, ir::Function>,
}

impl RuntimeSupportInliner {
    fn for_module(jit_module: &mut JITModule) -> Result<Self, String> {
        let library = runtime_support_library()?;
        let mut import_func_ids = HashMap::new();
        let mut inlineable = HashMap::new();
        for parsed in &library.functions {
            if !matches!(
                parsed.symbol.as_str(),
                SOAC_RUNTIME_INCREF_SYMBOL | SOAC_RUNTIME_DECREF_SYMBOL
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
                &mut import_func_ids,
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

fn declare_local_fn(
    jit_module: &mut JITModule,
    symbol: &str,
    sig: &ir::Signature,
) -> Result<FuncId, String> {
    jit_module
        .declare_function(symbol, Linkage::Local, sig)
        .map_err(|err| format!("failed to declare local {symbol} function: {err}"))
}

fn is_clif_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(crate) const JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT: &str = "d";
pub(crate) const JIT_PYTHON_PERF_SYMBOL_KIND_VECTORCALL: &str = "v";
pub(crate) const SOAC_RUNTIME_INCREF_SYMBOL: &str = "soac_runtime_incref";
pub(crate) const SOAC_RUNTIME_DECREF_SYMBOL: &str = "soac_runtime_decref";

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
    import_func_ids: &mut HashMap<String, FuncId>,
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
    Ok(())
}

fn load_runtime_support_clif(jit_module: &mut JITModule) -> Result<(), String> {
    let library = runtime_support_library()?;
    let mut import_func_ids = HashMap::new();
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
            &mut import_func_ids,
        )?;
        let mut ctx = jit_module.make_context();
        ctx.func = function;
        define_function_with_incremental_cache(
            jit_module,
            func_id,
            &mut ctx,
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
        let Some(eq_pos) = line.find(" = u") else {
            continue;
        };
        let alias = &line[..eq_pos];
        if alias.is_empty() {
            continue;
        }
        let rest = &line[(eq_pos + 4)..];
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

pub fn run_cranelift_smoke(module: &BlockPyModule<CodegenBlockPyPass>) -> Result<(), String> {
    let function_count = module.callable_defs.len() as i64;
    let block_count = module
        .callable_defs
        .iter()
        .map(|f| f.blocks.len() as i64)
        .sum::<i64>();
    let sentinel = (function_count << 32) ^ block_count;

    let mut jit_module = new_jit_module()?;
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
    define_function_with_incremental_cache(
        &mut jit_module,
        function_id,
        &mut ctx,
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
    module: &BlockPyModule<CodegenBlockPyPass>,
    function: &BlockPyFunction<CodegenBlockPyPass>,
    module_constants: &ModuleCodegenConstants,
    counter_defs: &[CounterDef],
    module_constant_ptrs: &[*mut ffi::PyObject],
    counter_ptrs: &[*mut u64],
    direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
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
            if let CodegenBlockPyExpr::IncrementCounter(op) = expr {
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
    let call_target_specializations = match direct_call_resolver {
        Some(shared_state) => {
            parse_call_target_specializations_env(shared_state.module_name.as_str(), function.function_id)?
        }
        None => HashMap::new(),
    };

    let mut direct_call_targets = collect_call_direct_targets(function);
    for targets in call_target_specializations.values() {
        direct_call_targets.extend(targets.iter().copied());
    }

    let mut direct_call_code_ptrs = HashMap::new();
    for function_id in direct_call_targets {
        let maybe_code_ptr = match direct_call_resolver {
            Some(shared_state) => shared_state.lookup_or_compile_direct_code_ptr(function_id)?,
            None => None,
        };
        if let Some(code_ptr) = maybe_code_ptr {
            direct_call_code_ptrs.insert(function_id, code_ptr);
        }
    }

    let ptr_ty = jit_module.target_config().pointer_type();
    let i64_ty = ir::types::I64;
    let mut module_imports = ModuleFuncImports::new();

    let mut main_sig = jit_module.make_signature();
    main_sig.params.push(ir::AbiParam::special(
        ptr_ty,
        ir::ArgumentPurpose::VMContext,
    ));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    for _ in function.params.iter() {
        main_sig.params.push(ir::AbiParam::new(ptr_ty));
    }
    main_sig.returns.push(ir::AbiParam::new(ptr_ty));

    let main_symbol =
        jit_python_perf_symbol_name(JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT, &function.names.qualname);
    let main_id = declare_local_fn(jit_module, &main_symbol, &main_sig)?;
    let counted_refcount_helpers =
        build_counted_runtime_refcount_helpers(jit_module, function, counter_defs, counter_ptrs)?;

    let mut ctx = jit_module.make_context();
    ctx.func.signature = main_sig;
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut block_annotations = ClifBlockDisplayAnnotations::new();
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
        let mut cleanup_null_blocks = Vec::with_capacity(block_count);
        for _ in 0..block_count {
            exec_blocks.push(fb.create_block());
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

        register_block_display_annotation(
            &mut block_annotations,
            entry_block,
            "jit_entry",
            vec![
                "vmctx".into(),
                "callable".into(),
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
                *block,
                format!("cleanup_null::{}", function.blocks[index].label),
                vec!["value".into()],
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

        fb.switch_to_block(entry_block);
        let entry_block_params = fb.block_params(entry_block).to_vec();
        let vmctx_value = entry_block_params[0];
        let callable = entry_block_params[1];
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
        let get_raised_exception_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_GET_RAISED_EXCEPTION_IMPORT,
        );
        let load_global_obj_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_LOAD_GLOBAL_OBJ_IMPORT);
        let load_runtime_obj_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_LOAD_RUNTIME_OBJ_IMPORT);
        let is_true_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_IS_TRUE_IMPORT);
        let raise_exc_ref =
            func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_RAISE_FROM_EXC_IMPORT);
        let function_closure_cell_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_FUNCTION_CLOSURE_CELL_IMPORT,
        );
        let function_positional_default_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_FUNCTION_POSITIONAL_DEFAULT_OBJ_IMPORT,
        );
        let function_kwonly_default_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_FUNCTION_KWONLY_DEFAULT_OBJ_IMPORT,
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
        let callee_function_id_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_CALLEE_FUNCTION_ID_IMPORT,
        );
        let record_counter_value_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_RECORD_COUNTER_VALUE_IMPORT,
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

        let entry_deleted_const = load_vmctx_obj(&mut fb, ptr_ty, vmctx_value, DELETED_OBJ_OFFSET);
        stack_slots.initialize_all_to_value(&mut fb, entry_deleted_const, incref_ref);

        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let entry_failure_block = cleanup_null_blocks[0];
        let entry_failure_args = Vec::new();
        assert_eq!(
            direct_entry_args.len(),
            function.params.len(),
            "direct JIT entry arity does not match entry params",
        );
        for ((param, default_source), value) in function
            .params
            .iter_with_default_sources()
            .zip(direct_entry_args.iter())
        {
            match default_source {
                Some(ParamDefaultSource::Positional(default_index)) => {
                    let arg_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, *value, null_ptr);
                    let use_default_block = fb.create_block();
                    let use_arg_block = fb.create_block();
                    let after_block = fb.create_block();
                    fb.ins()
                        .brif(arg_is_null, use_default_block, &[], use_arg_block, &[]);

                    fb.switch_to_block(use_default_block);
                    let name_obj = emit_owned_module_constant_from_parts(
                        &mut fb,
                        module_constants.require_unicode_constant_id(param.name.as_str()),
                        module_constant_ptrs,
                        ptr_ty,
                    );
                    let default_index_val = fb.ins().iconst(i64_ty, default_index as i64);
                    let default_inst = fb.ins().call(
                        function_positional_default_ref,
                        &[callable, name_obj, default_index_val],
                    );
                    fb.ins().call(decref_ref, &[name_obj]);
                    let default_value = fb.inst_results(default_inst)[0];
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
                            incref_ref,
                            decref_ref,
                        )
                        .expect("entry slot missing from stack slots");
                    fb.ins().call(decref_ref, &[default_value]);
                    fb.ins().jump(after_block, &[]);

                    fb.switch_to_block(use_arg_block);
                    stack_slots
                        .replace_cloned_value(
                            &mut fb,
                            param.name.as_str(),
                            *value,
                            ptr_ty,
                            incref_ref,
                            decref_ref,
                        )
                        .expect("entry slot missing from stack slots");
                    fb.ins().jump(after_block, &[]);

                    fb.switch_to_block(after_block);
                }
                Some(ParamDefaultSource::KeywordOnly(default_name)) => {
                    let arg_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, *value, null_ptr);
                    let use_default_block = fb.create_block();
                    let use_arg_block = fb.create_block();
                    let after_block = fb.create_block();
                    fb.ins()
                        .brif(arg_is_null, use_default_block, &[], use_arg_block, &[]);

                    fb.switch_to_block(use_default_block);
                    let name_obj = emit_owned_module_constant_from_parts(
                        &mut fb,
                        module_constants.require_unicode_constant_id(default_name),
                        module_constant_ptrs,
                        ptr_ty,
                    );
                    let default_inst = fb
                        .ins()
                        .call(function_kwonly_default_ref, &[callable, name_obj]);
                    fb.ins().call(decref_ref, &[name_obj]);
                    let default_value = fb.inst_results(default_inst)[0];
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
                            incref_ref,
                            decref_ref,
                        )
                        .expect("entry slot missing from stack slots");
                    fb.ins().call(decref_ref, &[default_value]);
                    fb.ins().jump(after_block, &[]);

                    fb.switch_to_block(use_arg_block);
                    stack_slots
                        .replace_cloned_value(
                            &mut fb,
                            param.name.as_str(),
                            *value,
                            ptr_ty,
                            incref_ref,
                            decref_ref,
                        )
                        .expect("entry slot missing from stack slots");
                    fb.ins().jump(after_block, &[]);

                    fb.switch_to_block(after_block);
                }
                None => {
                    stack_slots
                        .replace_cloned_value(
                            &mut fb,
                            param.name.as_str(),
                            *value,
                            ptr_ty,
                            incref_ref,
                            decref_ref,
                        )
                        .expect("entry slot missing from stack slots");
                }
            }
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
            let block_param_values = fb.block_params(*block).to_vec();
            for (param_name, param_value) in runtime_block_param_names[index]
                .iter()
                .zip(block_param_values.iter())
            {
                stack_slots
                    .replace_cloned_value(
                        &mut fb,
                        param_name,
                        *param_value,
                        ptr_ty,
                        incref_ref,
                        decref_ref,
                    )
                    .expect("runtime block param missing from stack slots");
                fb.ins().call(decref_ref, &[*param_value]);
            }
            let block_const = load_vmctx_obj(&mut fb, ptr_ty, vmctx_value, GLOBALS_OBJ_OFFSET);
            let global_slots_const =
                load_vmctx_obj(&mut fb, ptr_ty, vmctx_value, GLOBAL_SLOTS_OFFSET);
            let none_const = load_vmctx_obj(&mut fb, ptr_ty, vmctx_value, NONE_OBJ_OFFSET);
            let true_const = load_vmctx_obj(&mut fb, ptr_ty, vmctx_value, TRUE_OBJ_OFFSET);
            let false_const = load_vmctx_obj(&mut fb, ptr_ty, vmctx_value, FALSE_OBJ_OFFSET);
            let deleted_const = load_vmctx_obj(&mut fb, ptr_ty, vmctx_value, DELETED_OBJ_OFFSET);
            let empty_tuple_const =
                load_vmctx_obj(&mut fb, ptr_ty, vmctx_value, EMPTY_TUPLE_OBJ_OFFSET);
            let global_load_hit_counter_ptr =
                lookup_global_runtime_counter_ptr(counter_defs, counter_ptrs, "global_load_hit")?;
            let global_load_miss_counter_ptr =
                lookup_global_runtime_counter_ptr(counter_defs, counter_ptrs, "global_load_miss")?;
            let fast_step_null_block =
                exception_dispatch_blocks[index].unwrap_or(cleanup_null_blocks[index]);
            let fast_step_null_args = Vec::new();
            let emit_ctx = JitEmitCtx {
                module,
                module_constants,
                module_constant_ptrs,
                counter_ptrs,
                storage_layout: function.storage_layout().clone(),
                incref_ref,
                decref_ref,
                py_call_positional_three_ref,
                py_vectorcall_ref,
                consts: JitEmitConsts {
                    step_null_block: fast_step_null_block,
                    step_null_args: fast_step_null_args,
                    ptr_ty,
                    i64_ty,
                    vmctx_value,
                    callable_value: callable,
                    none_const,
                    true_const,
                    false_const,
                    deleted_const,
                    empty_tuple_const,
                    block_const,
                    global_slots_const,
                    global_load_hit_counter_ptr,
                    global_load_miss_counter_ptr,
                },
                load_global_obj_ref,
                load_runtime_obj_ref,
                function_closure_cell_ref,
                pyobject_getattr_ref,
                pyobject_setattr_ref,
                pyobject_getitem_ref,
                pyobject_setitem_ref,
                raise_deleted_name_error_ref,
                make_cell_ref,
                load_cell_ref,
                store_cell_ref,
                py_call_object_ref,
                py_call_with_kw_ref,
                callee_function_id_ref,
                record_counter_value_ref,
                tuple_new_ref,
                tuple_set_item_ref,
                stack_slots: stack_slots.clone(),
                direct_call_code_ptrs: &direct_call_code_ptrs,
                call_target_counter_ids: &call_target_counter_ids,
                call_target_specializations: &call_target_specializations,
                call_direct_hit_counter_ids: &call_direct_hit_counter_ids,
                call_direct_fallback_counter_ids: &call_direct_fallback_counter_ids,
            };
            let block = &function.blocks[index];
            let mut local_names = Vec::new();
            let mut local_values = Vec::new();

            emit_codegen_ops(
                &mut fb,
                &block.body,
                &mut local_names,
                &mut local_values,
                &stack_slots,
                &emit_ctx,
                jit_module,
                &mut func_imports,
            )?;

            emit_codegen_term(
                &mut fb,
                block.label.to_string().as_str(),
                &block.term,
                &exec_blocks,
                &runtime_block_param_names,
                &full_block_param_names,
                &mut local_names,
                &mut local_values,
                &emit_ctx,
                jit_module,
                &mut func_imports,
                is_true_ref,
                pyobject_to_i64_ref,
                raise_exc_ref,
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
            let none_const = load_vmctx_obj(&mut fb, ptr_ty, vmctx_value, NONE_OBJ_OFFSET);
            let dispatch_step_null_args = Vec::new();

            let raised_exc_inst = fb.ins().call(get_raised_exception_ref, &[]);
            let raised_exc = fb.inst_results(raised_exc_inst)[0];
            let raised_exc_null = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, raised_exc, null_ptr);
            let raised_exc_ok = fb.create_block();
            fb.append_block_param(raised_exc_ok, ptr_ty);
            fb.ins().brif(
                raised_exc_null,
                cleanup_null_blocks[index],
                &dispatch_step_null_args,
                raised_exc_ok,
                &[ir::BlockArg::Value(raised_exc)],
            );

            fb.switch_to_block(raised_exc_ok);
            let dispatch_exc = fb.block_params(raised_exc_ok)[0];
            emit_exception_dispatch_slot_writes(
                &mut fb,
                &dispatch_plan.slot_writes,
                dispatch_exc,
                &stack_slots,
                ptr_ty,
                none_const,
                incref_ref,
                decref_ref,
            )?;
            let target_runtime_params = &runtime_block_param_names[dispatch_plan.target_index];
            let mut target_jump_args = Vec::with_capacity(target_runtime_params.len());
            if target_runtime_params.is_empty() {
                fb.ins().call(decref_ref, &[dispatch_exc]);
            } else {
                target_jump_args.push(ir::BlockArg::Value(dispatch_exc));
            }
            fb.ins()
                .jump(exec_blocks[dispatch_plan.target_index], &target_jump_args);
        }

        for block in &cleanup_null_blocks {
            fb.switch_to_block(*block);
            let cleanup_args = fb.block_params(*block).to_vec();
            for value in cleanup_args {
                fb.ins().call(decref_ref, &[value]);
            }
            stack_slots.decref_all(&mut fb, ptr_ty, decref_ref);
            let null_ptr = fb.ins().iconst(ptr_ty, 0);
            fb.ins().return_(&[null_ptr]);
        }

        fb.switch_to_block(step_null_block);
        let step_null_args = fb.block_params(step_null_block)[0];
        stack_slots.decref_all(&mut fb, ptr_ty, decref_ref);
        fb.ins().call(decref_ref, &[step_null_args]);
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
        fb.ins().call(decref_ref, &[red_set_exc]);
        fb.ins().jump(red_done_block, &[]);
        fb.switch_to_block(red_done_block);
        fb.ins().call(decref_ref, &[red_args]);
        stack_slots.decref_all(&mut fb, ptr_ty, decref_ref);
        fb.ins().return_(&[red_null]);

        fb.seal_all_blocks();
        fb.finalize();
    }

    Ok(BuiltSpecializedFunction {
        ctx,
        main_id,
        import_id_to_symbol: module_imports.debug_symbols().clone(),
        block_annotations,
    })
}

pub unsafe fn render_cranelift_run_bb_specialized_with_cfg(
    blocks: &[ObjPtr],
    module: &BlockPyModule<CodegenBlockPyPass>,
    function: &soac_blockpy::block_py::BlockPyFunction<CodegenBlockPyPass>,
    module_constants: &ModuleCodegenConstants,
) -> Result<RenderedSpecializedClif, String> {
    if blocks.is_empty() {
        return Err("specialized JIT run_bb requires at least one block".to_string());
    }

    let builder = new_jit_builder()?;
    let mut jit_module = JITModule::new(builder);
    let module_constant_ptrs = placeholder_module_constant_ptrs(module_constants.len());
    let counter_ptrs = placeholder_counter_ptrs(
        function
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|expr| match expr {
                CodegenBlockPyExpr::IncrementCounter(op) => Some(op.counter_id.0),
                _ => None,
            })
            .max()
            .map_or(0, |max_counter_id| max_counter_id + 1),
    );
    let built = build_cranelift_run_bb_specialized_function(
        &mut jit_module,
        blocks,
        module,
        function,
        module_constants,
        &[],
        &module_constant_ptrs,
        &counter_ptrs,
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

pub unsafe fn compile_cranelift_run_bb_specialized_cached(
    blocks: &[ObjPtr],
    module: &BlockPyModule<CodegenBlockPyPass>,
    function: &soac_blockpy::block_py::BlockPyFunction<CodegenBlockPyPass>,
    module_constants: &ModuleCodegenConstants,
    counter_defs: &[CounterDef],
    module_constant_ptrs: &[*mut ffi::PyObject],
    counter_ptrs: &[*mut u64],
    direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
) -> Result<ObjPtr, String> {
    let mut compiled = Box::new(CompiledSpecializedRunner {
        _jit_module: new_jit_module()?,
        entry: None,
    });
    let built = build_cranelift_run_bb_specialized_function(
        &mut compiled._jit_module,
        blocks,
        module,
        function,
        module_constants,
        counter_defs,
        module_constant_ptrs,
        counter_ptrs,
        direct_call_resolver,
    )?;
    let mut ctx = built.ctx;
    let main_id = built.main_id;
    define_function_with_incremental_cache(
        &mut compiled._jit_module,
        main_id,
        &mut ctx,
        "failed to define specialized jit run_bb function",
    )?;
    compiled._jit_module.clear_context(&mut ctx);
    compiled
        ._jit_module
        .finalize_definitions()
        .map_err(|err| format!("failed to finalize specialized jit run_bb function: {err}"))?;
    let code_ptr = compiled._jit_module.get_finalized_function(main_id);
    compiled.entry = Some(CompiledRunnerEntry::Direct {
        code_ptr,
        param_count: function.params.len(),
    });
    Ok(Box::into_raw(compiled) as ObjPtr)
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

pub unsafe fn compile_cranelift_vectorcall_direct_trampoline(
    bind_direct_args_fn: unsafe extern "C" fn(
        ObjPtr,
        *const ObjPtr,
        usize,
        ObjPtr,
        ObjPtr,
        *mut ObjPtr,
        i64,
    ) -> i32,
    data_ptr: ObjPtr,
    vmctx_ptr: ObjPtr,
    compiled_handle: ObjPtr,
    symbol_name: &str,
) -> Result<(ObjPtr, VectorcallEntryFn), String> {
    if data_ptr.is_null() {
        return Err("invalid null vectorcall data pointer".to_string());
    }
    if vmctx_ptr.is_null() {
        return Err("invalid null vectorcall vmctx pointer".to_string());
    }
    let (direct_code_ptr, param_count) = compiled_direct_runner_info(compiled_handle)?;

    let mut builder = new_jit_builder()?;
    builder.symbol(
        "dp_jit_vectorcall_bind_direct_args",
        bind_direct_args_fn as *const u8,
    );
    let mut jit_module = JITModule::new(builder);
    load_runtime_support_clif(&mut jit_module)?;
    let ptr_ty = jit_module.target_config().pointer_type();
    let i64_ty = ir::types::I64;
    let mut module_imports = ModuleFuncImports::new();

    let mut main_sig = jit_module.make_signature();
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.returns.push(ir::AbiParam::new(ptr_ty));

    let main_id = declare_local_fn(&mut jit_module, symbol_name, &main_sig)?;

    let mut direct_sig = jit_module.make_signature();
    direct_sig.params.push(ir::AbiParam::special(
        ptr_ty,
        ir::ArgumentPurpose::VMContext,
    ));
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
            &mut jit_module,
            &mut fb.func,
            &DP_JIT_VECTORCALL_BIND_DIRECT_ARGS_IMPORT,
        );
        let decref_ref =
            func_imports.get_or_panic(&mut jit_module, &mut fb.func, &DP_JIT_DECREF_IMPORT);

        let data_const = fb.ins().iconst(ptr_ty, data_ptr as i64);
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
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
                data_const,
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
        let vmctx_const = fb.ins().iconst(ptr_ty, vmctx_ptr as i64);
        call_args.push(vmctx_const);
        call_args.push(callable_val);
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
        let callee_ptr = fb.ins().iconst(ptr_ty, direct_code_ptr as i64);
        let call_inst = fb
            .ins()
            .call_indirect(direct_sig_ref, callee_ptr, &call_args);
        let result = fb.inst_results(call_inst)[0];
        for value in owned_args {
            fb.ins().call(decref_ref, &[value]);
        }
        fb.ins().return_(&[result]);
        fb.seal_all_blocks();
        fb.finalize();
    }

    define_function_with_incremental_cache(
        &mut jit_module,
        main_id,
        &mut ctx,
        "failed to define direct vectorcall trampoline",
    )?;
    jit_module.clear_context(&mut ctx);
    jit_module
        .finalize_definitions()
        .map_err(|err| format!("failed to finalize direct vectorcall trampoline: {err}"))?;

    let code_ptr = jit_module.get_finalized_function(main_id);
    let entry: VectorcallEntryFn = std::mem::transmute(code_ptr);
    let compiled = Box::new(CompiledVectorcallRunner {
        _jit_module: jit_module,
    });
    Ok((Box::into_raw(compiled) as ObjPtr, entry))
}

pub unsafe fn free_cranelift_vectorcall_trampoline(compiled_handle: ObjPtr) {
    if compiled_handle.is_null() {
        return;
    }
    let _ = Box::from_raw(compiled_handle as *mut CompiledVectorcallRunner);
}

pub unsafe fn free_cranelift_run_bb_specialized_cached(compiled_handle: ObjPtr) {
    if compiled_handle.is_null() {
        return;
    }
    let _ = Box::from_raw(compiled_handle as *mut CompiledSpecializedRunner);
}

#[cfg(test)]
mod test;
