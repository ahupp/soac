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
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};
use soac_ir_typed::{InstrTyped, TypedBlockPyModuleShape};
use std::collections::HashMap;
use std::ffi::{c_int, c_void};
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
struct RawPyInterpreterFrameForRecursion {
    executable: usize,
    previous: *mut c_void,
    function: usize,
    globals: *mut ffi::PyObject,
    builtins: *mut ffi::PyObject,
    locals: *mut ffi::PyObject,
    frame_object: *mut ffi::PyObject,
    instruction_pointer: *mut c_void,
    stack_pointer: *mut c_void,
    return_offset: u16,
    owner: u8,
    visited: u8,
    locals_and_stack: [usize; 1],
}

#[repr(C)]
struct RawPyThreadStateEmbeddedFrameTail {
    base_frame: RawPyInterpreterFrameForRecursion,
    refcount: ffi::Py_ssize_t,
    c_stack_top: usize,
    c_stack_soft_limit: usize,
}

#[repr(C)]
pub struct ModuleJitContext {
    pub shared_module_state: *const SharedModuleState,
    pub globals_obj: ObjPtr,
}

#[repr(C)]
struct PyFunctionJitExtraPrefix {
    function_env: ObjPtr,
    function_id: u64,
}

#[repr(C)]
struct RawPyFunctionObjectSoacMetadataPrefix {
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
    func_soac_metadata_destructor: *mut c_void,
    func_soac_function_id: u64,
    func_version: u32,
}

#[repr(C)]
struct RawPyCodeVersionPrefix {
    ob_base: ffi::PyVarObject,
    co_consts: *mut ffi::PyObject,
    co_names: *mut ffi::PyObject,
    co_exceptiontable: *mut ffi::PyObject,
    co_flags: c_int,
    co_argcount: c_int,
    co_posonlyargcount: c_int,
    co_kwonlyargcount: c_int,
    co_stacksize: c_int,
    co_firstlineno: c_int,
    co_nlocalsplus: c_int,
    co_framesize: c_int,
    co_nlocals: c_int,
    co_ncellvars: c_int,
    co_nfreevars: c_int,
    co_version: u32,
    co_localsplusnames: *mut ffi::PyObject,
    co_localspluskinds: *mut ffi::PyObject,
    co_filename: *mut ffi::PyObject,
    co_name: *mut ffi::PyObject,
    co_qualname: *mut ffi::PyObject,
    co_linetable: *mut ffi::PyObject,
    co_weakreflist: *mut ffi::PyObject,
    co_executors: *mut c_void,
    co_cached: *mut c_void,
    co_instrumentation_version: usize,
    co_monitoring: *mut c_void,
}

#[repr(C)]
struct RawPyWeakRefForJit {
    ob_base: ffi::PyObject,
    wr_object: *mut ffi::PyObject,
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
    offset_of!(crate::FunctionEnvAbiHeader, direct_code_ptr) as i32;
pub const FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, default_direct_code_ptr) as i32;
pub const FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, deopt_table_ptr) as i32;
pub const FUNCTION_ENV_GLOBALS_OBJ_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, globals_obj) as i32;
pub const FUNCTION_ENV_BUILTINS_OBJ_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, builtins_obj) as i32;
pub const FUNCTION_ENV_LATE_BOUND_OWNER_CELLS_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, late_bound_owner_cells) as i32;
pub const LATE_BOUND_OWNER_FIELD_CELL_SIZE: i32 =
    size_of::<crate::module_type::LateBoundOwnerFieldCell>() as i32;
pub const LATE_BOUND_OWNER_FIELD_WEAKREF_OFFSET: i32 =
    offset_of!(crate::module_type::LateBoundOwnerFieldCell, owner_weakref) as i32;
pub const LATE_BOUND_OWNER_FIELD_TYPE_VERSION_OFFSET: i32 =
    offset_of!(crate::module_type::LateBoundOwnerFieldCell, type_version) as i32;
pub const LATE_BOUND_OWNER_FIELD_SLOT_OFFSET_OFFSET: i32 =
    offset_of!(crate::module_type::LateBoundOwnerFieldCell, slot_offset) as i32;
pub const RAW_PY_WEAKREF_OBJECT_OFFSET: i32 = offset_of!(RawPyWeakRefForJit, wr_object) as i32;
pub const FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET: i32 =
    size_of::<crate::FunctionEnvAbiHeader>() as i32;
pub const PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET: i32 =
    offset_of!(PyFunctionJitExtraPrefix, function_env) as i32;
pub const PY_FUNCTION_CODE_OFFSET: i32 =
    offset_of!(RawPyFunctionObjectSoacMetadataPrefix, func_code) as i32;
pub const PY_FUNCTION_DEFAULTS_OFFSET: i32 =
    offset_of!(RawPyFunctionObjectSoacMetadataPrefix, func_defaults) as i32;
pub const PY_FUNCTION_KWDEFAULTS_OFFSET: i32 =
    offset_of!(RawPyFunctionObjectSoacMetadataPrefix, func_kwdefaults) as i32;
pub const PY_FUNCTION_SOAC_FUNCTION_ID_OFFSET: i32 =
    offset_of!(RawPyFunctionObjectSoacMetadataPrefix, func_soac_function_id) as i32;
pub const FIRST_VALID_CPYTHON_FUNCTION_VERSION: u32 = 2;
pub const PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET: i32 =
    offset_of!(PyThreadStateCurrentExceptionPrefix, current_exception) as i32;
pub(super) const PY_THREAD_STATE_BASE_FRAME_OFFSET: i32 =
    offset_of!(PyThreadStateCurrentExceptionPrefix, base_frame) as i32;
pub(super) const PY_BASE_FRAME_C_STACK_SOFT_LIMIT_OFFSET: i32 =
    offset_of!(RawPyThreadStateEmbeddedFrameTail, c_stack_soft_limit) as i32;

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
        offset_of!(RawPyFunctionObjectSoacMetadataPrefix, func_soac_metadata) as i32,
    )
}

pub(crate) unsafe fn invalidate_py_function_soac_function_id(function: *mut ffi::PyFunctionObject) {
    let raw_function = function.cast::<RawPyFunctionObjectSoacMetadataPrefix>();
    unsafe { (*raw_function).func_soac_function_id = 0 };
}

pub(crate) unsafe fn raw_py_code_version(code: *mut ffi::PyObject) -> u32 {
    unsafe { (*code.cast::<RawPyCodeVersionPrefix>()).co_version }
}

pub(crate) unsafe fn raw_py_code_freevar_count(code: *mut ffi::PyObject) -> c_int {
    unsafe { (*code.cast::<RawPyCodeVersionPrefix>()).co_nfreevars }
}

pub(crate) unsafe fn raw_py_code_flags(code: *mut ffi::PyObject) -> c_int {
    unsafe { (*code.cast::<RawPyCodeVersionPrefix>()).co_flags }
}

pub(crate) unsafe fn raw_py_code_has_function_names(
    code: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    qualname: *mut ffi::PyObject,
) -> bool {
    let code = unsafe { &*code.cast::<RawPyCodeVersionPrefix>() };
    code.co_name == name && code.co_qualname == qualname
}

pub(crate) unsafe fn raw_py_function_activation_is_observed(code: *mut ffi::PyObject) -> bool {
    let thread_state =
        unsafe { ffi::PyThreadState_Get() }.cast::<PyThreadStateCurrentExceptionPrefix>();
    if thread_state.is_null() {
        return true;
    }
    let thread_state = unsafe { &*thread_state };
    if !thread_state.c_profilefunc.is_null() || !thread_state.c_tracefunc.is_null() {
        return true;
    }
    if thread_state.interp.is_null() {
        return true;
    }

    // In the pinned CPython layout, PyInterpreterState starts with ceval and
    // ceval starts with its global instrumentation version.
    if unsafe { *thread_state.interp.cast::<usize>() } != 0 {
        return true;
    }

    // Local monitoring does not change the interpreter-global version.
    !unsafe { (*code.cast::<RawPyCodeVersionPrefix>()).co_monitoring }.is_null()
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
    pub(crate) fn from_function(function: &BlockPyFunction<BlockPyModuleShape>) -> Self {
        Self::from_parts(function, max_referenced_function_closure_slot(function))
    }

    pub(crate) fn from_typed_function(function: &BlockPyFunction<TypedBlockPyModuleShape>) -> Self {
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
            .public_storage_layout()
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

fn max_referenced_function_closure_slot(function: &BlockPyFunction<BlockPyModuleShape>) -> usize {
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
                CellLocation::Owned(_) | CellLocation::Preserved(_) => {}
            }
        }

        fn visit_name(&mut self, name: &ResolvedName) {
            if let Some(location) = name.cell_location() {
                self.visit_cell_location(location);
            }
        }
    }

    impl Visit<InstrBlockPy> for Collector {
        fn visit_instr(&mut self, expr: &InstrBlockPy) {
            match expr {
                InstrBlockPy::Load(op) => self.visit_name(&op.name),
                InstrBlockPy::Store(op) => self.visit_name(&op.name),
                InstrBlockPy::Del(op) => self.visit_name(&op.name),
                InstrBlockPy::CellRef(op) => self.visit_cell_location(op.location),
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
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
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
                CellLocation::Owned(_) | CellLocation::Preserved(_) => {}
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
