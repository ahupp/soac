use super::ObjPtr;
use crate::module_type::SharedModuleState;
use crate::session::CompileSession;
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::FunctionBuilder;
use pyo3::ffi;
use soac_core::block_py::{
    BlockPyFunction, CellLocation, ChildVisitable, ModuleShape, ParamKind, ResolvedName, Visit,
};
use soac_ir_blockpy::{CodegenModuleShape, InstrCodegen};
use soac_ir_typed::{InstrTyped, TypedCodegenModuleShape};
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::ptr;
use std::sync::Arc;

#[repr(C)]
struct PyThreadStateCurrentExceptionPrefix {
    prev: *mut ffi::PyThreadState,
    next: *mut ffi::PyThreadState,
    interp: *mut ffi::PyInterpreterState,
    eval_breaker: usize,
    status: u32,
    holds_gil: i32,
    gil_requested: i32,
    whence: i32,
    state: i32,
    py_recursion_remaining: i32,
    py_recursion_limit: i32,
    recursion_headroom: i32,
    tracing: i32,
    what_event: i32,
    current_frame: *mut c_void,
    base_frame: *mut c_void,
    last_profiled_frame: *mut c_void,
    c_profilefunc: *mut c_void,
    c_tracefunc: *mut c_void,
    c_profileobj: *mut ffi::PyObject,
    c_traceobj: *mut ffi::PyObject,
    current_exception: *mut ffi::PyObject,
}

#[repr(C)]
pub struct ModuleJitContext {
    pub shared_module_state: *const SharedModuleState,
    pub globals_obj: ObjPtr,
}

#[repr(C)]
struct FunctionEnvPrefix {
    direct_code_ptr: *const u8,
    default_direct_code_ptr: *const u8,
    deopt_table_ptr: ObjPtr,
    globals_obj: ObjPtr,
}

#[repr(C)]
struct PyFunctionJitExtraPrefix {
    function_env: ObjPtr,
    function_id: u64,
}

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
    func_soac_metadata: *mut c_void,
}

pub struct ModuleRuntimeContext {
    pub mod_ctx: ModuleJitContext,
    pub compile_session: Arc<CompileSession>,
    pub shared_module_state_owner: Arc<SharedModuleState>,
}

unsafe fn decref_if_non_null(obj: ObjPtr) {
    if !obj.is_null() {
        unsafe { ffi::Py_DECREF(obj.cast::<ffi::PyObject>()) };
    }
}

impl Drop for ModuleRuntimeContext {
    fn drop(&mut self) {
        unsafe {
            decref_if_non_null(self.mod_ctx.globals_obj);
        }
        self.mod_ctx.shared_module_state = ptr::null();
        self.mod_ctx.globals_obj = ptr::null_mut::<c_void>();
    }
}

pub const FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET: i32 =
    offset_of!(FunctionEnvPrefix, direct_code_ptr) as i32;
pub const FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET: i32 =
    offset_of!(FunctionEnvPrefix, default_direct_code_ptr) as i32;
pub const FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET: i32 =
    offset_of!(FunctionEnvPrefix, deopt_table_ptr) as i32;
pub const FUNCTION_ENV_GLOBALS_OBJ_OFFSET: i32 = offset_of!(FunctionEnvPrefix, globals_obj) as i32;
pub const FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET: i32 = size_of::<FunctionEnvPrefix>() as i32;
pub const PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET: i32 =
    offset_of!(PyFunctionJitExtraPrefix, function_env) as i32;
pub const PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET: i32 =
    offset_of!(PyThreadStateCurrentExceptionPrefix, current_exception) as i32;

pub(super) fn load_function_env_obj(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    function_env_value: ir::Value,
    offset: i32,
) -> ir::Value {
    fb.ins()
        .load(ptr_ty, ir::MemFlags::trusted(), function_env_value, offset)
}

pub(super) fn load_py_function_soac_metadata_obj(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    function_obj: ir::Value,
) -> ir::Value {
    fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        function_obj,
        offset_of!(PyFunctionObjectSoacMetadataPrefix, func_soac_metadata) as i32,
    )
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

    pub(super) fn from_parts<P: ModuleShape>(
        function: &BlockPyFunction<P>,
        max_closure_slot: usize,
    ) -> Self {
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
