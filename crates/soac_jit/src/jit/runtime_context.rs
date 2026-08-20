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
use std::ffi::{c_int, c_uint, c_void};
use std::mem::{offset_of, size_of};
use std::ptr;
use std::sync::Arc;

#[repr(C)]
struct RawPyThreadStateCurrentExceptionPrefix {
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
    exc_info: *mut c_void,
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
    soac_dataclass_role: c_uint,
    soac_dataclass_invocation: *mut ffi::PyObject,
    soac_dataclass_checked_activation: *mut ffi::PyObject,
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
pub(super) const FUNCTION_ENV_STRICT_FIELD_SLOTS_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, strict_field_slots) as i32;
pub(super) const FUNCTION_ENV_STRICT_FIELD_SLOT_COUNT_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, strict_field_slot_count) as i32;
pub(super) const FUNCTION_ENV_STRICT_METHOD_SLOTS_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, strict_method_slots) as i32;
pub(super) const FUNCTION_ENV_STRICT_METHOD_SLOT_COUNT_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, strict_method_slot_count) as i32;
pub(super) const FUNCTION_ENV_ACTIVE_STRICT_CALL_OFFSET: i32 =
    offset_of!(crate::FunctionEnvAbiHeader, active_strict_call) as i32;
pub(super) const PY_FUNCTION_VECTORCALL_OFFSET: i32 =
    offset_of!(ffi::PyFunctionObject, vectorcall) as i32;
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
pub(super) const PY_THREAD_STATE_EVAL_BREAKER_OFFSET: i32 =
    offset_of!(RawPyThreadStateCurrentExceptionPrefix, eval_breaker) as i32;
pub const PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET: i32 =
    offset_of!(RawPyThreadStateCurrentExceptionPrefix, current_exception) as i32;
pub(crate) const PY_THREAD_STATE_EXC_INFO_OFFSET: i32 =
    offset_of!(RawPyThreadStateCurrentExceptionPrefix, exc_info) as i32;
pub(super) const PY_THREAD_STATE_BASE_FRAME_OFFSET: i32 =
    offset_of!(RawPyThreadStateCurrentExceptionPrefix, base_frame) as i32;
pub(super) const PY_BASE_FRAME_C_STACK_SOFT_LIMIT_OFFSET: i32 =
    offset_of!(RawPyThreadStateEmbeddedFrameTail, c_stack_soft_limit) as i32;

#[cfg(test)]
pub(super) fn assert_recursion_frame_abi_matches_native(
    py: pyo3::Python<'_>,
) -> pyo3::PyResult<()> {
    use pyo3::prelude::*;

    let native: HashMap<String, usize> = py
        .import("_testinternalcapi")?
        .call_method0("soac_dataclass_frame_offsets")?
        .extract()?;
    for (name, actual) in [
        ("frame_size", size_of::<RawPyInterpreterFrameForRecursion>()),
        (
            "role",
            offset_of!(RawPyInterpreterFrameForRecursion, soac_dataclass_role),
        ),
        (
            "invocation",
            offset_of!(RawPyInterpreterFrameForRecursion, soac_dataclass_invocation),
        ),
        (
            "checked_activation",
            offset_of!(
                RawPyInterpreterFrameForRecursion,
                soac_dataclass_checked_activation
            ),
        ),
        (
            "localsplus",
            offset_of!(RawPyInterpreterFrameForRecursion, locals_and_stack),
        ),
        (
            "thread_base_frame_pointer",
            PY_THREAD_STATE_BASE_FRAME_OFFSET as usize,
        ),
        (
            "thread_soft_limit_from_base_frame",
            PY_BASE_FRAME_C_STACK_SOFT_LIMIT_OFFSET as usize,
        ),
    ] {
        let expected = native.get(name).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "selected CPython recursion ABI probe omitted {name}: {native:?}"
            ))
        })?;
        assert_eq!(actual, *expected, "selected CPython recursion ABI: {name}");
    }
    Ok(())
}

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
    // Opaque metadata and its numeric profile ID are public extension slots.
    // Only the owning destructor identifies our allocation type. This is an
    // observational probe, not source/contract authentication: foreign data
    // must take the ordinary miss path without a private-payload dereference.
    let destructor = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        function_obj,
        offset_of!(
            RawPyFunctionObjectSoacMetadataPrefix,
            func_soac_metadata_destructor
        ) as i32,
    );
    let expected = fb.ins().iconst(
        ptr_ty,
        crate::free_clif_function_data as *const () as usize as i64,
    );
    let owned = fb
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, destructor, expected);
    let metadata = fb.ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        function_obj,
        offset_of!(RawPyFunctionObjectSoacMetadataPrefix, func_soac_metadata) as i32,
    );
    let absent = fb.ins().iconst(ptr_ty, 0);
    fb.ins().select(owned, metadata, absent)
}

pub(crate) unsafe fn invalidate_py_function_soac_function_id(function: *mut ffi::PyFunctionObject) {
    let raw_function = function.cast::<RawPyFunctionObjectSoacMetadataPrefix>();
    unsafe { (*raw_function).func_soac_function_id = 0 };
}

#[cfg(test)]
#[test]
fn metadata_probe_selects_only_the_owned_allocation_type() {
    let mut function = ir::Function::new();
    function
        .signature
        .params
        .push(ir::AbiParam::new(ir::types::I64));
    function
        .signature
        .returns
        .push(ir::AbiParam::new(ir::types::I64));
    let mut context = cranelift_frontend::FunctionBuilderContext::new();
    let (callable, metadata);
    {
        let mut fb = FunctionBuilder::new(&mut function, &mut context);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);
        callable = fb.block_params(entry)[0];
        metadata = load_py_function_soac_metadata_obj(&mut fb, ir::types::I64, callable);
        fb.ins().return_(&[metadata]);
        fb.finalize();
    }
    let defining_instruction = |value| match function.dfg.value_def(value) {
        ir::ValueDef::Result(instruction, _) => instruction,
        other => panic!("expected generated metadata instruction, got {other:?}"),
    };
    let selection = defining_instruction(metadata);
    assert_eq!(function.dfg.insts[selection].opcode(), ir::Opcode::Select);
    let [condition, payload, absent] = function.dfg.inst_args(selection) else {
        panic!("metadata probe must select an owned payload or NULL");
    };
    let comparison = defining_instruction(*condition);
    assert!(matches!(
        function.dfg.insts[comparison],
        ir::InstructionData::IntCompare {
            cond: ir::condcodes::IntCC::Equal,
            ..
        }
    ));
    let [destructor, expected] = function.dfg.inst_args(comparison) else {
        panic!("metadata probe must compare the actual owning destructor");
    };
    for (value, offset) in [
        (
            *destructor,
            offset_of!(
                RawPyFunctionObjectSoacMetadataPrefix,
                func_soac_metadata_destructor
            ),
        ),
        (
            *payload,
            offset_of!(RawPyFunctionObjectSoacMetadataPrefix, func_soac_metadata),
        ),
    ] {
        let instruction = defining_instruction(value);
        assert!(matches!(
            function.dfg.insts[instruction],
            ir::InstructionData::Load { offset: actual, .. } if actual == (offset as i32).into()
        ));
        assert_eq!(function.dfg.inst_args(instruction), &[callable]);
    }
    for (value, expected_value) in [
        (
            *expected,
            crate::free_clif_function_data as *const () as usize as i64,
        ),
        (*absent, 0),
    ] {
        assert!(matches!(
            function.dfg.insts[defining_instruction(value)],
            ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm }
                if imm.bits() == expected_value
        ));
    }
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

#[derive(Clone, Debug)]
pub(crate) struct FunctionRuntimeDataLayout {
    positional_default_count: usize,
    positional_default_slots_by_param_index: HashMap<usize, usize>,
    kwonly_default_slots: HashMap<String, usize>,
    closure_start: usize,
    closure_len: usize,
    private_cell_len: usize,
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
        let private_cell_len = function
            .scope
            .private_lexical
            .as_ref()
            .map_or(0, |scope| scope.private_captures().count());
        let total_len = closure_start + closure_len + private_cell_len;
        Self {
            positional_default_count,
            positional_default_slots_by_param_index,
            kwonly_default_slots,
            closure_start,
            closure_len,
            private_cell_len,
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

    pub(crate) fn private_cell_len(&self) -> usize {
        self.private_cell_len
    }

    pub(crate) fn private_cell_slot(&self, index: usize) -> usize {
        assert!(index < self.private_cell_len);
        self.closure_start + self.closure_len + index
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
                CellLocation::Owned(_) | CellLocation::Preserved(_) | CellLocation::Private(_) => {}
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
                CellLocation::Owned(_) | CellLocation::Preserved(_) | CellLocation::Private(_) => {}
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
