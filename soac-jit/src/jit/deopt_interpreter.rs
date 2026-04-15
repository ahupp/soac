use super::{
    FunctionRuntimeDataLayout, RuntimeJitDeoptCursor, RuntimeJitDeoptInvocation,
    RuntimeJitDeoptLocals, specialized_helpers::ObjPtr,
};
use crate::module_constants::load_runtime_name_owned;
use pyo3::ffi;
use soac_blockpy::block_py::{
    AbruptKind, BinOp, BinOpKind, BlockArg, BlockEdge, BlockTerm, CallArgKeyword,
    CallArgPositional, CalleeFunctionId, CellLocation, InstrCodegen, LocalLocation, NameLocation,
    UnaryOp, UnaryOpKind,
};
use std::ffi::{c_int, c_void};
use std::ptr;

unsafe extern "C" {
    static mut PyMethod_Type: ffi::PyTypeObject;

    fn PyMethod_Function(meth: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyCell_New(obj: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyErr_SetRaisedException(exc: *mut ffi::PyObject);
    fn _PyDict_MergeEx(
        mp: *mut ffi::PyObject,
        other: *mut ffi::PyObject,
        override_: c_int,
    ) -> c_int;
    fn _PyEval_FormatKwargsError(
        tstate: *mut ffi::PyThreadState,
        func: *mut ffi::PyObject,
        kwargs: *mut ffi::PyObject,
    );
}

#[cold]
pub(super) fn execute_deopt_invocation(
    invocation: &RuntimeJitDeoptInvocation<'_>,
) -> Result<ObjPtr, String> {
    let mut frame = BlockPyDeoptFrame::new(invocation)?;
    let result = frame.execute();
    unsafe {
        frame.release_frame_owned_values();
    }
    result
}

struct BlockPyDeoptFrame<'inv, 'data> {
    invocation: &'inv RuntimeJitDeoptInvocation<'data>,
    locals: RuntimeJitDeoptLocals<'inv>,
    current_exception: ObjPtr,
}

impl<'inv, 'data> BlockPyDeoptFrame<'inv, 'data> {
    #[cold]
    fn new(invocation: &'inv RuntimeJitDeoptInvocation<'data>) -> Result<Self, String> {
        let locals = invocation.materialize_locals()?;
        Ok(Self {
            invocation,
            locals,
            current_exception: ptr::null_mut(),
        })
    }

    #[cold]
    fn execute(&mut self) -> Result<ObjPtr, String> {
        let Some(cursor) = self.invocation.record().initial_cursor() else {
            return Err(format!(
                "{}, {}",
                self.invocation.describe(),
                self.locals.describe()
            ));
        };
        unsafe { self.execute_from_cursor(cursor) }
    }

    #[cold]
    unsafe fn execute_from_cursor(
        &mut self,
        mut cursor: RuntimeJitDeoptCursor,
    ) -> Result<ObjPtr, String> {
        let function = self.invocation.function();
        'execute: loop {
            let block_label = cursor.block();
            let start_body_index = cursor.body_index();
            let block = function
                .blocks
                .iter()
                .find(|candidate| candidate.label == block_label)
                .ok_or_else(|| {
                    format!(
                        "deopt continuation expected block {block_label} in function {}",
                        function.function_id
                    )
                })?;
            let body_tail = block.body.get(start_body_index..).ok_or_else(|| {
                format!(
                    "deopt continuation start body index {start_body_index} is outside block {block_label}"
                )
            })?;
            for instr in body_tail {
                let value = unsafe { self.execute_expr_owned(instr)? };
                if value.is_null() {
                    if let Some(next_cursor) =
                        unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                    {
                        cursor = next_cursor;
                        continue 'execute;
                    }
                    return Ok(ptr::null_mut());
                }
                unsafe {
                    ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                }
            }
            match &block.term {
                BlockTerm::Return(value) => {
                    let value = unsafe { self.execute_expr_owned(value)? };
                    if value.is_null() {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                    }
                    return Ok(value);
                }
                BlockTerm::Jump(edge) => {
                    let Some(next_cursor) = (unsafe { self.execute_jump_edge(edge)? }) else {
                        return Ok(ptr::null_mut());
                    };
                    cursor = next_cursor;
                }
                BlockTerm::IfTerm(if_term) => {
                    let test = unsafe { self.execute_expr_owned(&if_term.test)? };
                    if test.is_null() {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                        return Ok(ptr::null_mut());
                    }
                    let truth = unsafe { ffi::PyObject_IsTrue(test.cast::<ffi::PyObject>()) };
                    unsafe {
                        ffi::Py_DECREF(test.cast::<ffi::PyObject>());
                    }
                    if truth < 0 {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                        return Ok(ptr::null_mut());
                    }
                    let next_block = if truth != 0 {
                        if_term.then_label
                    } else {
                        if_term.else_label
                    };
                    cursor = RuntimeJitDeoptCursor::at_block_entry(next_block);
                }
                BlockTerm::BranchTable(branch) => {
                    let index_obj = unsafe { self.execute_expr_owned(&branch.index)? };
                    if index_obj.is_null() {
                        return Ok(ptr::null_mut());
                    }
                    let index =
                        unsafe { ffi::PyLong_AsLongLong(index_obj.cast::<ffi::PyObject>()) };
                    unsafe {
                        ffi::Py_DECREF(index_obj.cast::<ffi::PyObject>());
                    }
                    if index == -1 && !unsafe { ffi::PyErr_Occurred() }.is_null() {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                        return Ok(ptr::null_mut());
                    }
                    let next_block = usize::try_from(index)
                        .ok()
                        .and_then(|index| branch.targets.get(index).copied())
                        .unwrap_or(branch.default_label);
                    cursor = RuntimeJitDeoptCursor::at_block_entry(next_block);
                }
                BlockTerm::Raise(raise) => {
                    let value = unsafe { self.execute_raise_term_owned(raise)? };
                    if value.is_null() {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                    }
                    return Ok(value);
                }
            }
        }
    }

    #[cold]
    unsafe fn try_dispatch_exception_edge(
        &mut self,
        edge: Option<BlockEdge>,
    ) -> Result<Option<RuntimeJitDeoptCursor>, String> {
        if edge.is_none() || unsafe { ffi::PyErr_Occurred() }.is_null() {
            return Ok(None);
        }
        if !unsafe { self.capture_current_exception_for_dispatch() } {
            return Ok(None);
        }
        unsafe { self.execute_jump_edge(&edge.expect("edge checked above")) }
    }

    #[cold]
    unsafe fn capture_current_exception_for_dispatch(&mut self) -> bool {
        unsafe {
            if !self.current_exception.is_null() {
                ffi::Py_DECREF(self.current_exception.cast::<ffi::PyObject>());
                self.current_exception = ptr::null_mut();
            }
        }
        self.current_exception = unsafe { take_current_raised_exception_owned() };
        !self.current_exception.is_null()
    }

    #[cold]
    unsafe fn execute_jump_edge(
        &mut self,
        edge: &BlockEdge,
    ) -> Result<Option<RuntimeJitDeoptCursor>, String> {
        let function = self.invocation.function();
        let target_block = function
            .blocks
            .iter()
            .find(|candidate| candidate.label == edge.target)
            .ok_or_else(|| {
                format!(
                    "deopt continuation expected jump target {} in function {}",
                    edge.target, function.function_id
                )
            })?;
        let target_params = target_block.params.clone();
        if target_params.len() != edge.args.len() {
            return Err(format!(
                "deopt continuation jump to {} has {} args for {} target params",
                edge.target,
                edge.args.len(),
                target_params.len()
            ));
        }
        for param in &target_params {
            if self.locals.get_by_name(param.name.as_str()).is_none() {
                return Err(format!(
                    "deopt continuation jump to {} targets param {}, but it was not materialized: {}",
                    edge.target,
                    param.name,
                    self.locals.describe()
                ));
            }
        }

        let mut values = Vec::with_capacity(edge.args.len());
        for arg in &edge.args {
            let value = match arg {
                BlockArg::Name(name) => unsafe { self.execute_block_arg_name_owned(name)? },
                BlockArg::None => owned_none(),
                BlockArg::AbruptKind(kind) => unsafe { execute_abrupt_kind_arg_owned(*kind) },
                BlockArg::CurrentException => unsafe { self.current_exception_arg_owned() },
            };
            if value.is_null() {
                unsafe {
                    release_owned_values(values);
                }
                return Ok(None);
            }
            values.push(value);
        }

        for (param, value) in target_params.iter().zip(values.into_iter()) {
            let local = self
                .locals
                .get_by_name_mut(param.name.as_str())
                .expect("jump target params were prevalidated against materialized locals");
            unsafe {
                local.replace_with_owned_value(value);
            }
        }
        Ok(Some(RuntimeJitDeoptCursor::at_block_entry(edge.target)))
    }

    #[cold]
    unsafe fn execute_block_arg_name_owned(&self, name: &str) -> Result<ObjPtr, String> {
        let Some(local) = self.locals.get_by_name(name) else {
            return Err(format!(
                "deopt continuation jump expected local {name}, but it was not materialized: {}",
                self.locals.describe()
            ));
        };
        let value = local.value();
        if value.is_null() {
            set_deopt_unbound_local_error(name);
            return Ok(ptr::null_mut());
        }
        unsafe {
            ffi::Py_INCREF(value.cast::<ffi::PyObject>());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_expr_owned(&mut self, expr: &InstrCodegen) -> Result<ObjPtr, String> {
        match expr {
            InstrCodegen::Load(load) => unsafe {
                self.execute_load_owned(load.name.id.as_str(), load.name.location)
            },
            InstrCodegen::BinOp(binop) => unsafe { self.execute_binop_owned(binop) },
            InstrCodegen::UnaryOp(unary) => unsafe { self.execute_unary_op_owned(unary) },
            InstrCodegen::GetAttr(getattr) => unsafe { self.execute_getattr_owned(getattr) },
            InstrCodegen::GetItem(getitem) => unsafe { self.execute_getitem_owned(getitem) },
            InstrCodegen::SetAttr(setattr) => unsafe { self.execute_setattr_owned(setattr) },
            InstrCodegen::SetItem(setitem) => unsafe { self.execute_setitem_owned(setitem) },
            InstrCodegen::DelItem(delitem) => unsafe { self.execute_delitem_owned(delitem) },
            InstrCodegen::CalleeFunctionId(callee) => unsafe {
                self.execute_callee_function_id_owned(callee)
            },
            InstrCodegen::Call(call) => unsafe { self.execute_call_owned(call) },
            InstrCodegen::CallDirect(call) => unsafe { self.execute_call_direct_owned(call) },
            InstrCodegen::Store(store) => unsafe { self.execute_store_owned(store) },
            InstrCodegen::Del(del) => unsafe { self.execute_del_owned(del) },
            InstrCodegen::IncrementCounter(_) => unsafe { execute_runtime_name_deopt("NONE") },
            InstrCodegen::MakeCell(make_cell) => unsafe { self.execute_make_cell_owned(make_cell) },
            InstrCodegen::MakeFunctionWithClosure(make_function) => unsafe {
                self.execute_make_function_with_closure_owned(make_function)
            },
            InstrCodegen::CellRef(cell_ref) => unsafe { self.execute_cell_ref_owned(cell_ref) },
            _ => Err(format!(
                "deopt continuation only supports simple load/binop/call/store/del expressions, got {expr:?}"
            )),
        }
    }

    #[cold]
    unsafe fn execute_make_cell_owned(
        &mut self,
        make_cell: &soac_blockpy::block_py::MakeCell<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let Some(initial_value_expr) = make_cell.initial_value.as_ref() else {
            let cell = unsafe { PyCell_New(ptr::null_mut()) };
            return Ok(cell.cast());
        };
        let initial_value = unsafe { self.execute_expr_owned(initial_value_expr)? };
        if initial_value.is_null() {
            return Ok(ptr::null_mut());
        }
        let cell = unsafe { PyCell_New(initial_value.cast::<ffi::PyObject>()) };
        unsafe {
            ffi::Py_DECREF(initial_value.cast::<ffi::PyObject>());
        }
        Ok(cell.cast())
    }

    #[cold]
    unsafe fn execute_make_function_with_closure_owned(
        &mut self,
        make_function: &soac_blockpy::block_py::MakeFunctionWithClosure<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let callable = unsafe { execute_runtime_name_deopt("make_function")? };
        if callable.is_null() {
            return Ok(ptr::null_mut());
        }
        let function_id =
            unsafe { ffi::PyLong_FromUnsignedLongLong(make_function.function_id().packed()) };
        if function_id.is_null() {
            unsafe { ffi::Py_DECREF(callable.cast::<ffi::PyObject>()) };
            return Ok(ptr::null_mut());
        }
        let kind_name = make_function.kind.make_function_kind_name();
        let kind_len = match ffi::Py_ssize_t::try_from(kind_name.len()) {
            Ok(kind_len) => kind_len,
            Err(_) => {
                unsafe {
                    ffi::Py_DECREF(function_id);
                    ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                }
                return Err(format!(
                    "make-function deopt kind name {kind_name:?} is too large to materialize"
                ));
            }
        };
        let kind = unsafe { ffi::PyUnicode_FromStringAndSize(kind_name.as_ptr().cast(), kind_len) };
        if kind.is_null() {
            unsafe {
                ffi::Py_DECREF(function_id);
                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let captures = unsafe { self.execute_expr_owned(make_function.captures.as_ref())? };
        if captures.is_null() {
            unsafe {
                ffi::Py_DECREF(kind);
                ffi::Py_DECREF(function_id);
                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let param_defaults =
            unsafe { self.execute_expr_owned(make_function.param_defaults.as_ref())? };
        if param_defaults.is_null() {
            unsafe {
                ffi::Py_DECREF(captures.cast::<ffi::PyObject>());
                ffi::Py_DECREF(kind);
                ffi::Py_DECREF(function_id);
                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let annotate_fn = unsafe { self.execute_expr_owned(make_function.annotate_fn.as_ref())? };
        if annotate_fn.is_null() {
            unsafe {
                ffi::Py_DECREF(param_defaults.cast::<ffi::PyObject>());
                ffi::Py_DECREF(captures.cast::<ffi::PyObject>());
                ffi::Py_DECREF(kind);
                ffi::Py_DECREF(function_id);
                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let globals = self.invocation.globals_obj();
        if globals.is_null() {
            unsafe {
                ffi::Py_DECREF(annotate_fn.cast::<ffi::PyObject>());
                ffi::Py_DECREF(param_defaults.cast::<ffi::PyObject>());
                ffi::Py_DECREF(captures.cast::<ffi::PyObject>());
                ffi::Py_DECREF(kind);
                ffi::Py_DECREF(function_id);
                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
            }
            return Err("deopt continuation expected module globals for make_function".to_string());
        }
        unsafe {
            ffi::Py_INCREF(globals.cast::<ffi::PyObject>());
        }
        unsafe {
            execute_owned_positional_call(
                callable,
                vec![
                    function_id.cast(),
                    kind.cast(),
                    captures,
                    param_defaults,
                    annotate_fn,
                    globals,
                ],
            )
        }
    }

    #[cold]
    unsafe fn execute_cell_ref_owned(
        &self,
        cell_ref: &soac_blockpy::block_py::CellRef,
    ) -> Result<ObjPtr, String> {
        unsafe { self.execute_raw_cell_object_for_location_owned(cell_ref.location, "cell_ref") }
    }

    #[cold]
    unsafe fn execute_raw_cell_object_for_location_owned(
        &self,
        location: CellLocation,
        debug_name: &str,
    ) -> Result<ObjPtr, String> {
        match location {
            CellLocation::Owned(slot) => unsafe {
                self.execute_owned_raw_cell_object_for_slot_owned(slot, debug_name)
            },
            CellLocation::Closure(slot) | CellLocation::CapturedSource(slot) => unsafe {
                self.execute_closure_raw_cell_object_for_slot_owned(slot, debug_name)
            },
        }
    }

    #[cold]
    unsafe fn execute_owned_raw_cell_object_for_slot_owned(
        &self,
        slot: u32,
        debug_name: &str,
    ) -> Result<ObjPtr, String> {
        let function = self.invocation.function();
        let layout = function.storage_layout.as_ref().ok_or_else(|| {
            format!(
                "deopt continuation expected storage layout for owned {debug_name} slot {slot} in function {}",
                function.function_id
            )
        })?;
        let closure_slot = layout.local_cell_slot(slot).ok_or_else(|| {
            format!(
                "deopt continuation expected owned {debug_name} slot {slot} in function {} storage layout",
                function.function_id
            )
        })?;
        let mut candidate_names = vec![closure_slot.storage_name.as_str()];
        if closure_slot.logical_name != closure_slot.storage_name {
            candidate_names.push(closure_slot.logical_name.as_str());
        }
        for candidate_name in &candidate_names {
            if let Some(local) = self.locals.get_by_name(candidate_name) {
                let value = local.value();
                if value.is_null() {
                    set_deopt_unbound_local_error(candidate_name);
                    return Ok(ptr::null_mut());
                }
                unsafe {
                    ffi::Py_INCREF(value.cast::<ffi::PyObject>());
                }
                return Ok(value);
            }
        }
        Err(format!(
            "deopt continuation expected owned {debug_name} slot {slot} via names {:?}, but locals were {}",
            candidate_names,
            self.locals.describe()
        ))
    }

    #[cold]
    unsafe fn execute_closure_raw_cell_object_for_slot_owned(
        &self,
        slot: u32,
        debug_name: &str,
    ) -> Result<ObjPtr, String> {
        let function_data = self.invocation.function_data_obj();
        if function_data.is_null() {
            return Err(format!(
                "deopt continuation expected function data for closure {debug_name} slot {slot}"
            ));
        }
        let function = self.invocation.function();
        let runtime_layout = FunctionRuntimeDataLayout::from_function(function);
        if slot as usize >= runtime_layout.closure_len() {
            return Err(format!(
                "deopt continuation expected closure {debug_name} slot {slot} in function {} with {} closure slots",
                function.function_id,
                runtime_layout.closure_len()
            ));
        }
        let data_slot = runtime_layout.closure_cell_slot(slot as usize);
        let raw_cell = unsafe { *function_data.cast::<ObjPtr>().add(data_slot) };
        if raw_cell.is_null() {
            return Err(format!(
                "deopt continuation expected non-null closure {debug_name} slot {slot} in function {}",
                function.function_id
            ));
        }
        unsafe {
            ffi::Py_INCREF(raw_cell.cast::<ffi::PyObject>());
        }
        Ok(raw_cell)
    }

    #[cold]
    unsafe fn execute_binop_owned(
        &mut self,
        binop: &BinOp<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let left = unsafe { self.execute_expr_owned(&binop.left)? };
        if left.is_null() {
            return Ok(ptr::null_mut());
        }
        let right = unsafe { self.execute_expr_owned(&binop.right)? };
        if right.is_null() {
            unsafe {
                ffi::Py_DECREF(left.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let result = unsafe { execute_binop_kind_owned(binop.kind, left, right)? };
        unsafe {
            ffi::Py_DECREF(left.cast::<ffi::PyObject>());
            ffi::Py_DECREF(right.cast::<ffi::PyObject>());
        }
        Ok(result)
    }

    #[cold]
    unsafe fn execute_unary_op_owned(
        &mut self,
        unary: &UnaryOp<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let operand = unsafe { self.execute_expr_owned(&unary.operand)? };
        if operand.is_null() {
            return Ok(ptr::null_mut());
        }
        let result = unsafe { execute_unary_op_kind_owned(unary.kind, operand)? };
        unsafe {
            ffi::Py_DECREF(operand.cast::<ffi::PyObject>());
        }
        Ok(result)
    }

    #[cold]
    unsafe fn execute_getattr_owned(
        &mut self,
        getattr: &soac_blockpy::block_py::GetAttr<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(&getattr.value)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let attr = unsafe { self.execute_expr_owned(&getattr.attr)? };
        if attr.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let result = unsafe {
            ffi::PyObject_GetAttr(value.cast::<ffi::PyObject>(), attr.cast::<ffi::PyObject>())
        };
        unsafe {
            ffi::Py_DECREF(attr.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_getitem_owned(
        &mut self,
        getitem: &soac_blockpy::block_py::GetItem<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(&getitem.value)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let index = unsafe { self.execute_expr_owned(&getitem.index)? };
        if index.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let result = unsafe {
            ffi::PyObject_GetItem(value.cast::<ffi::PyObject>(), index.cast::<ffi::PyObject>())
        };
        unsafe {
            ffi::Py_DECREF(index.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_setattr_owned(
        &mut self,
        setattr: &soac_blockpy::block_py::SetAttr<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(&setattr.value)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let attr = unsafe { self.execute_expr_owned(&setattr.attr)? };
        if attr.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let replacement = unsafe { self.execute_expr_owned(&setattr.replacement)? };
        if replacement.is_null() {
            unsafe {
                ffi::Py_DECREF(attr.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let rc = unsafe {
            ffi::PyObject_SetAttr(
                value.cast::<ffi::PyObject>(),
                attr.cast::<ffi::PyObject>(),
                replacement.cast::<ffi::PyObject>(),
            )
        };
        unsafe {
            ffi::Py_DECREF(replacement.cast::<ffi::PyObject>());
            ffi::Py_DECREF(attr.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
        if rc != 0 {
            return Ok(ptr::null_mut());
        }
        Ok(owned_none())
    }

    #[cold]
    unsafe fn execute_setitem_owned(
        &mut self,
        setitem: &soac_blockpy::block_py::SetItem<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(&setitem.value)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let index = unsafe { self.execute_expr_owned(&setitem.index)? };
        if index.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let replacement = unsafe { self.execute_expr_owned(&setitem.replacement)? };
        if replacement.is_null() {
            unsafe {
                ffi::Py_DECREF(index.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let rc = unsafe {
            ffi::PyObject_SetItem(
                value.cast::<ffi::PyObject>(),
                index.cast::<ffi::PyObject>(),
                replacement.cast::<ffi::PyObject>(),
            )
        };
        unsafe {
            ffi::Py_DECREF(replacement.cast::<ffi::PyObject>());
            ffi::Py_DECREF(index.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
        if rc != 0 {
            return Ok(ptr::null_mut());
        }
        Ok(owned_none())
    }

    #[cold]
    unsafe fn execute_delitem_owned(
        &mut self,
        delitem: &soac_blockpy::block_py::DelItem<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(&delitem.value)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let index = unsafe { self.execute_expr_owned(&delitem.index)? };
        if index.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let rc = unsafe {
            ffi::PyObject_DelItem(value.cast::<ffi::PyObject>(), index.cast::<ffi::PyObject>())
        };
        unsafe {
            ffi::Py_DECREF(index.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
        if rc != 0 {
            return Ok(ptr::null_mut());
        }
        Ok(owned_none())
    }

    #[cold]
    unsafe fn execute_call_owned(
        &mut self,
        call: &soac_blockpy::block_py::Call<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        unsafe { self.execute_call_parts_owned(&call.func, &call.args, &call.keywords) }
    }

    #[cold]
    unsafe fn execute_call_direct_owned(
        &mut self,
        call: &soac_blockpy::block_py::CallDirect<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        unsafe { self.execute_call_parts_owned(&call.callable, &call.args, &call.keywords) }
    }

    #[cold]
    unsafe fn execute_callee_function_id_owned(
        &mut self,
        callee: &CalleeFunctionId<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let callable = unsafe { self.execute_expr_owned(&callee.value)? };
        if callable.is_null() {
            return Ok(ptr::null_mut());
        }
        let packed = unsafe { callable_soac_function_id(callable.cast::<ffi::PyObject>()) };
        unsafe {
            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
        }
        Ok(unsafe { ffi::PyLong_FromLongLong(packed as i64).cast() })
    }

    #[cold]
    unsafe fn execute_call_parts_owned(
        &mut self,
        callable_expr: &InstrCodegen,
        positional_args: &[CallArgPositional<InstrCodegen>],
        keyword_args: &[CallArgKeyword<InstrCodegen>],
    ) -> Result<ObjPtr, String> {
        let callable = unsafe { self.execute_expr_owned(callable_expr)? };
        if callable.is_null() {
            return Ok(ptr::null_mut());
        }

        let mut args = Vec::with_capacity(positional_args.len());
        for arg in positional_args {
            match arg {
                CallArgPositional::Positional(expr) => {
                    let value = unsafe { self.execute_expr_owned(expr)? };
                    if value.is_null() {
                        unsafe {
                            release_owned_values(args);
                            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                        }
                        return Ok(ptr::null_mut());
                    }
                    args.push(value);
                }
                CallArgPositional::Starred(expr) => {
                    let value = unsafe { self.execute_expr_owned(expr)? };
                    if value.is_null() {
                        unsafe {
                            release_owned_values(args);
                            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                        }
                        return Ok(ptr::null_mut());
                    }
                    let tuple = unsafe { ffi::PySequence_Tuple(value.cast::<ffi::PyObject>()) };
                    unsafe {
                        ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                    }
                    if tuple.is_null() {
                        unsafe {
                            release_owned_values(args);
                            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                        }
                        return Ok(ptr::null_mut());
                    }
                    let tuple_len = unsafe { ffi::PyTuple_Size(tuple) };
                    if tuple_len < 0 {
                        unsafe {
                            ffi::Py_DECREF(tuple);
                            release_owned_values(args);
                            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                        }
                        return Ok(ptr::null_mut());
                    }
                    for index in 0..tuple_len {
                        let item = unsafe { ffi::PyTuple_GetItem(tuple, index) };
                        if item.is_null() {
                            unsafe {
                                ffi::Py_DECREF(tuple);
                                release_owned_values(args);
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        unsafe {
                            ffi::Py_INCREF(item);
                        }
                        args.push(item.cast());
                    }
                    unsafe {
                        ffi::Py_DECREF(tuple);
                    }
                }
            };
        }

        let args_len = match ffi::Py_ssize_t::try_from(args.len()) {
            Ok(args_len) => args_len,
            Err(_) => {
                unsafe {
                    release_owned_values(args);
                    ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                }
                return Err(format!(
                    "deopt continuation call has too many positional args: {}",
                    positional_args.len()
                ));
            }
        };
        let tuple = unsafe { ffi::PyTuple_New(args_len) };
        if tuple.is_null() {
            unsafe {
                release_owned_values(args);
                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        for (index, arg) in args.into_iter().enumerate() {
            let index = ffi::Py_ssize_t::try_from(index)
                .expect("tuple arg index should fit after tuple length conversion");
            // Use the exported API rather than the layout macro; this path must match the
            // vendored CPython tuple layout even when PyO3's cfgs lag a CPython change.
            if unsafe { ffi::PyTuple_SetItem(tuple, index, arg.cast::<ffi::PyObject>()) } != 0 {
                unsafe {
                    ffi::Py_DECREF(tuple);
                    ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                }
                return Ok(ptr::null_mut());
            }
        }
        let kwargs = if keyword_args.is_empty() {
            ptr::null_mut()
        } else {
            let kwargs = unsafe { ffi::PyDict_New() };
            if kwargs.is_null() {
                unsafe {
                    ffi::Py_DECREF(tuple);
                    ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                }
                return Ok(ptr::null_mut());
            }
            for keyword in keyword_args {
                match keyword {
                    CallArgKeyword::Named { arg, value } => {
                        let value = unsafe { self.execute_expr_owned(value)? };
                        if value.is_null() {
                            unsafe {
                                ffi::Py_DECREF(kwargs);
                                ffi::Py_DECREF(tuple);
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let name_len = match ffi::Py_ssize_t::try_from(arg.as_str().len()) {
                            Ok(name_len) => name_len,
                            Err(_) => {
                                unsafe {
                                    ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                                    ffi::Py_DECREF(kwargs);
                                    ffi::Py_DECREF(tuple);
                                    ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                                }
                                return Err(format!(
                                    "deopt continuation keyword name {:?} is too large to materialize as PyUnicode",
                                    arg.as_str()
                                ));
                            }
                        };
                        let key = unsafe {
                            ffi::PyUnicode_FromStringAndSize(arg.as_str().as_ptr().cast(), name_len)
                        };
                        if key.is_null() {
                            unsafe {
                                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                                ffi::Py_DECREF(kwargs);
                                ffi::Py_DECREF(tuple);
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let one_keyword = unsafe { ffi::PyDict_New() };
                        if one_keyword.is_null() {
                            unsafe {
                                ffi::Py_DECREF(key);
                                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                                ffi::Py_DECREF(kwargs);
                                ffi::Py_DECREF(tuple);
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let rc = unsafe {
                            ffi::PyDict_SetItem(one_keyword, key, value.cast::<ffi::PyObject>())
                        };
                        unsafe {
                            ffi::Py_DECREF(key);
                            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                        }
                        if rc != 0 {
                            unsafe {
                                ffi::Py_DECREF(one_keyword);
                                ffi::Py_DECREF(kwargs);
                                ffi::Py_DECREF(tuple);
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let merged = unsafe {
                            merge_kwargs_or_format_error(
                                callable.cast::<ffi::PyObject>(),
                                kwargs,
                                one_keyword,
                            )
                        };
                        unsafe {
                            ffi::Py_DECREF(one_keyword);
                        }
                        if !merged {
                            unsafe {
                                ffi::Py_DECREF(kwargs);
                                ffi::Py_DECREF(tuple);
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                    }
                    CallArgKeyword::Starred(value_expr) => {
                        let value = unsafe { self.execute_expr_owned(value_expr)? };
                        if value.is_null() {
                            unsafe {
                                ffi::Py_DECREF(kwargs);
                                ffi::Py_DECREF(tuple);
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let merged = unsafe {
                            merge_kwargs_or_format_error(
                                callable.cast::<ffi::PyObject>(),
                                kwargs,
                                value.cast::<ffi::PyObject>(),
                            )
                        };
                        unsafe {
                            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                        }
                        if !merged {
                            unsafe {
                                ffi::Py_DECREF(kwargs);
                                ffi::Py_DECREF(tuple);
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                    }
                }
            }
            kwargs
        };
        let result = if kwargs.is_null() {
            unsafe { ffi::PyObject_CallObject(callable.cast::<ffi::PyObject>(), tuple) }
        } else {
            unsafe { ffi::PyObject_Call(callable.cast::<ffi::PyObject>(), tuple, kwargs) }
        };
        unsafe {
            if !kwargs.is_null() {
                ffi::Py_DECREF(kwargs);
            }
            ffi::Py_DECREF(tuple);
            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
        }
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_raise_term_owned(
        &mut self,
        raise: &soac_blockpy::block_py::TermRaise<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        let Some(exc_expr) = &raise.exc else {
            let exc = unsafe { self.current_exception_arg_owned() };
            if exc.is_null() {
                return Ok(ptr::null_mut());
            }
            unsafe {
                PyErr_SetRaisedException(exc.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        };
        let exc = unsafe { self.execute_expr_owned(exc_expr)? };
        if exc.is_null() {
            return Ok(ptr::null_mut());
        }
        unsafe {
            set_raise_exception_owned(exc);
        }
        Ok(ptr::null_mut())
    }

    #[cold]
    unsafe fn execute_load_owned(
        &mut self,
        name: &str,
        location: NameLocation,
    ) -> Result<ObjPtr, String> {
        match location {
            NameLocation::Local(location) => self.execute_return_local(name, location),
            NameLocation::Global(slot) => unsafe {
                self.execute_return_global(name, i64::from(slot.slot()))
            },
            NameLocation::GlobalName => unsafe { self.execute_return_global(name, -1) },
            NameLocation::RuntimeName => unsafe { execute_runtime_name_deopt(name) },
            NameLocation::Constant(constant_index) => self.execute_module_constant(constant_index),
            NameLocation::Cell(location) => unsafe { self.execute_cell_load_owned(name, location) },
        }
    }

    #[cold]
    unsafe fn execute_cell_load_owned(
        &self,
        name: &str,
        location: CellLocation,
    ) -> Result<ObjPtr, String> {
        let cell = unsafe { self.execute_raw_cell_object_for_location_owned(location, name)? };
        if cell.is_null() {
            return Ok(ptr::null_mut());
        }
        let value = unsafe { super::specialized_helpers::dp_jit_load_cell(cell) };
        unsafe {
            ffi::Py_DECREF(cell.cast::<ffi::PyObject>());
        }
        Ok(value)
    }

    #[cold]
    fn execute_return_local(&self, name: &str, location: LocalLocation) -> Result<ObjPtr, String> {
        let local = self.locals.get_by_location(location).ok_or_else(|| {
            format!(
                "deopt continuation expected local {name} at {location:?}, but it was not materialized: {}",
                self.locals.describe()
            )
        })?;
        if local.binding().name != *name {
            return Err(format!(
                "deopt continuation expected local {name} at {location:?}, but materialized {}",
                local.binding().name
            ));
        }
        let value = local.value();
        if value.is_null() {
            set_deopt_unbound_local_error(name);
            return Ok(ptr::null_mut());
        }
        unsafe { ffi::Py_INCREF(value.cast::<ffi::PyObject>()) };
        Ok(value)
    }

    #[cold]
    fn execute_module_constant(&self, constant_index: u32) -> Result<ObjPtr, String> {
        let value = self.invocation.module_constant_ptr(constant_index)?;
        if value.is_null() {
            return Err(format!(
                "deopt continuation expected non-null module constant {constant_index}"
            ));
        }
        unsafe { ffi::Py_INCREF(value.cast::<ffi::PyObject>()) };
        Ok(value)
    }

    #[cold]
    unsafe fn execute_del_owned(
        &mut self,
        del: &soac_blockpy::block_py::Del<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        match del.name.location {
            NameLocation::Local(location) => unsafe {
                self.execute_local_del_owned(del.name.id.as_str(), location, del.quietly)
            },
            NameLocation::Global(_) | NameLocation::GlobalName => unsafe {
                self.execute_global_del_owned(del.name.id.as_str(), del.quietly)
            },
            NameLocation::Cell(location) => unsafe {
                self.execute_cell_del_owned(del.name.id.as_str(), location, del.quietly)
            },
            location => Err(format!(
                "deopt continuation does not support deleting {location:?} for {:?}",
                del.name.id.as_str()
            )),
        }
    }

    #[cold]
    unsafe fn execute_local_del_owned(
        &mut self,
        name: &str,
        location: LocalLocation,
        quietly: bool,
    ) -> Result<ObjPtr, String> {
        let Some(local) = self.locals.get_by_location_mut(location) else {
            if !quietly {
                set_deopt_unbound_local_error(name);
                return Ok(ptr::null_mut());
            }
            return unsafe { execute_runtime_name_deopt("NONE") };
        };
        if local.binding().name != *name {
            return Err(format!(
                "deopt continuation expected local {name} at {location:?}, but materialized {}",
                local.binding().name
            ));
        }
        if local.value().is_null() {
            if !quietly {
                set_deopt_unbound_local_error(name);
                return Ok(ptr::null_mut());
            }
        } else {
            unsafe {
                local.delete_value();
            }
        }
        unsafe { execute_runtime_name_deopt("NONE") }
    }

    #[cold]
    unsafe fn execute_cell_del_owned(
        &self,
        name: &str,
        location: CellLocation,
        quietly: bool,
    ) -> Result<ObjPtr, String> {
        let cell = unsafe { self.execute_raw_cell_object_for_location_owned(location, name)? };
        if cell.is_null() {
            return Ok(ptr::null_mut());
        }
        let result = if quietly {
            unsafe { super::specialized_helpers::dp_jit_del_deref_quietly(cell) }
        } else {
            unsafe { super::specialized_helpers::dp_jit_del_deref(cell) }
        };
        unsafe {
            ffi::Py_DECREF(cell.cast::<ffi::PyObject>());
        }
        Ok(result)
    }

    #[cold]
    unsafe fn execute_global_del_owned(&self, name: &str, quietly: bool) -> Result<ObjPtr, String> {
        let globals_obj = self.invocation.globals_obj();
        if globals_obj.is_null() {
            return Err(format!(
                "deopt continuation expected module globals for global delete {name:?}"
            ));
        }
        let name_len = ffi::Py_ssize_t::try_from(name.len()).map_err(|_| {
            format!("global-delete deopt name {name:?} is too large to materialize as PyUnicode")
        })?;
        let name_obj = unsafe { ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name_len) };
        if name_obj.is_null() {
            return Ok(ptr::null_mut());
        }
        let result = if quietly {
            unsafe {
                super::specialized_helpers::dp_jit_del_global_quietly(
                    globals_obj,
                    name_obj.cast::<c_void>(),
                    -1,
                )
            }
        } else {
            unsafe {
                super::specialized_helpers::dp_jit_del_global(
                    globals_obj,
                    name_obj.cast::<c_void>(),
                    -1,
                )
            }
        };
        unsafe { ffi::Py_DECREF(name_obj) };
        Ok(result)
    }

    #[cold]
    unsafe fn execute_store_owned(
        &mut self,
        store: &soac_blockpy::block_py::Store<InstrCodegen>,
    ) -> Result<ObjPtr, String> {
        match store.name.location {
            NameLocation::Local(location) => unsafe {
                self.execute_local_store_owned(
                    store.name.id.as_str(),
                    location,
                    store.value.as_ref(),
                )
            },
            NameLocation::Global(_) | NameLocation::GlobalName => unsafe {
                self.execute_global_store_owned(store.name.id.as_str(), store.value.as_ref())
            },
            NameLocation::Cell(location) => unsafe {
                self.execute_cell_store_owned(
                    store.name.id.as_str(),
                    location,
                    store.value.as_ref(),
                )
            },
            location => Err(format!(
                "deopt continuation does not support storing {location:?} for {:?}",
                store.name.id.as_str()
            )),
        }
    }

    #[cold]
    unsafe fn execute_local_store_owned(
        &mut self,
        name: &str,
        location: LocalLocation,
        value_expr: &InstrCodegen,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(value_expr)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let Some(local) = self.locals.get_by_location_mut(location) else {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Err(format!(
                "deopt continuation expected local {name} at {location:?} for store, but it was not materialized: {}",
                self.locals.describe()
            ));
        };
        if local.binding().name != *name {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Err(format!(
                "deopt continuation expected local {name} at {location:?}, but materialized {}",
                local.binding().name
            ));
        }
        unsafe {
            local.replace_with_owned_value(value);
            ffi::Py_INCREF(value.cast::<ffi::PyObject>());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_cell_store_owned(
        &mut self,
        name: &str,
        location: CellLocation,
        value_expr: &InstrCodegen,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(value_expr)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let cell = unsafe { self.execute_raw_cell_object_for_location_owned(location, name)? };
        if cell.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let result = unsafe { super::specialized_helpers::dp_jit_store_cell(cell, value) };
        unsafe {
            ffi::Py_DECREF(cell.cast::<ffi::PyObject>());
        }
        if result.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        unsafe {
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_global_store_owned(
        &mut self,
        name: &str,
        value_expr: &InstrCodegen,
    ) -> Result<ObjPtr, String> {
        let globals_obj = self.invocation.globals_obj();
        if globals_obj.is_null() {
            return Err(format!(
                "deopt continuation expected module globals for global store {name:?}"
            ));
        }
        let value = unsafe { self.execute_expr_owned(value_expr)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let name_len = match ffi::Py_ssize_t::try_from(name.len()) {
            Ok(name_len) => name_len,
            Err(_) => {
                unsafe { ffi::Py_DECREF(value.cast::<ffi::PyObject>()) };
                return Err(format!(
                    "global-store deopt name {name:?} is too large to materialize as PyUnicode"
                ));
            }
        };
        let name_obj = unsafe { ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name_len) };
        if name_obj.is_null() {
            unsafe { ffi::Py_DECREF(value.cast::<ffi::PyObject>()) };
            return Ok(ptr::null_mut());
        }
        let rc = unsafe {
            ffi::PyObject_SetItem(
                globals_obj.cast::<ffi::PyObject>(),
                name_obj,
                value.cast::<ffi::PyObject>(),
            )
        };
        unsafe { ffi::Py_DECREF(name_obj) };
        if rc != 0 {
            unsafe { ffi::Py_DECREF(value.cast::<ffi::PyObject>()) };
            return Ok(ptr::null_mut());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_return_global(
        &self,
        name: &str,
        expected_index: i64,
    ) -> Result<ObjPtr, String> {
        let globals_obj = self.invocation.globals_obj();
        if globals_obj.is_null() {
            return Err(format!(
                "deopt continuation expected module globals for return-global {name:?}"
            ));
        }
        let name_len = ffi::Py_ssize_t::try_from(name.len()).map_err(|_| {
            format!("return-global deopt name {name:?} is too large to materialize as PyUnicode")
        })?;
        let name_obj = unsafe { ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name_len) };
        if name_obj.is_null() {
            return Ok(ptr::null_mut());
        }
        let result = unsafe {
            super::specialized_helpers::soac_runtime_load_global_slow(
                globals_obj,
                name_obj.cast::<c_void>(),
                expected_index,
            )
        };
        unsafe { ffi::Py_DECREF(name_obj) };
        Ok(result)
    }

    #[cold]
    unsafe fn release_frame_owned_values(&mut self) {
        unsafe {
            self.locals.release_frame_owned_values();
            if !self.current_exception.is_null() {
                ffi::Py_DECREF(self.current_exception.cast::<ffi::PyObject>());
                self.current_exception = ptr::null_mut();
            }
        }
    }
}

unsafe fn execute_abrupt_kind_arg_owned(kind: AbruptKind) -> ObjPtr {
    unsafe { ffi::PyLong_FromLongLong(super::abrupt_kind_tag(kind)).cast() }
}

impl BlockPyDeoptFrame<'_, '_> {
    unsafe fn current_exception_arg_owned(&mut self) -> ObjPtr {
        if self.current_exception.is_null() {
            self.current_exception = unsafe { take_current_raised_exception_owned() };
            if self.current_exception.is_null() {
                return ptr::null_mut();
            }
        }
        unsafe {
            ffi::Py_INCREF(self.current_exception.cast::<ffi::PyObject>());
        }
        self.current_exception
    }
}

unsafe fn take_current_raised_exception_owned() -> ObjPtr {
    let tstate = unsafe { ffi::PyThreadState_Get() };
    let current_exception_slot = unsafe {
        tstate
            .cast::<u8>()
            .add(super::PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET as usize)
            .cast::<*mut ffi::PyObject>()
    };
    let current_exception = unsafe { *current_exception_slot };
    if current_exception.is_null() {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"No active exception to reraise".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    unsafe {
        *current_exception_slot = ptr::null_mut();
    }
    current_exception.cast()
}

fn owned_none() -> ObjPtr {
    unsafe {
        let none = ffi::Py_None();
        ffi::Py_INCREF(none);
        none.cast()
    }
}

unsafe fn release_owned_values(values: Vec<ObjPtr>) {
    for value in values {
        unsafe {
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
    }
}

unsafe fn set_raise_exception_owned(exc: ObjPtr) {
    let exc = exc.cast::<ffi::PyObject>();
    unsafe {
        if ffi::PyExceptionClass_Check(exc) != 0 {
            ffi::PyErr_SetObject(exc, ptr::null_mut());
            ffi::Py_DECREF(exc);
        } else if ffi::PyExceptionInstance_Check(exc) != 0 {
            let exc_type = ffi::PyExceptionInstance_Class(exc);
            ffi::PyErr_SetObject(exc_type, exc);
            ffi::Py_DECREF(exc);
        } else {
            ffi::Py_DECREF(exc);
            ffi::PyErr_SetString(
                ffi::PyExc_TypeError,
                c"exceptions must derive from BaseException".as_ptr(),
            );
        }
    }
}

unsafe fn callable_soac_function_id(callable: *mut ffi::PyObject) -> u64 {
    unsafe {
        if ffi::PyFunction_Check(callable) != 0 {
            return crate::PyFunction_GetSoacFunctionId(callable);
        }

        if ffi::Py_TYPE(callable) == ptr::addr_of_mut!(PyMethod_Type) {
            let function = PyMethod_Function(callable);
            if !function.is_null() && ffi::PyFunction_Check(function) != 0 {
                return crate::PyFunction_GetSoacFunctionId(function);
            }
        }

        if ffi::PyType_Check(callable) != 0 {
            let type_obj = callable.cast::<ffi::PyTypeObject>();
            if ((*type_obj).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE as std::ffi::c_ulong) != 0 {
                return crate::PyType_GetSoacFunctionId(callable);
            }
        }
    }
    0
}

unsafe fn merge_kwargs_or_format_error(
    callable: *mut ffi::PyObject,
    kwargs: *mut ffi::PyObject,
    update: *mut ffi::PyObject,
) -> bool {
    unsafe {
        if _PyDict_MergeEx(kwargs, update, 2) == 0 {
            return true;
        }
        _PyEval_FormatKwargsError(ffi::PyThreadState_Get(), callable, update);
    }
    false
}

#[cold]
unsafe fn execute_unary_op_kind_owned(
    kind: UnaryOpKind,
    operand: ObjPtr,
) -> Result<ObjPtr, String> {
    let operand = operand.cast::<ffi::PyObject>();
    let result = unsafe {
        match kind {
            UnaryOpKind::Pos => ffi::PyNumber_Positive(operand),
            UnaryOpKind::Neg => ffi::PyNumber_Negative(operand),
            UnaryOpKind::Invert => ffi::PyNumber_Invert(operand),
            UnaryOpKind::Not | UnaryOpKind::Truth => {
                let truth = ffi::PyObject_IsTrue(operand);
                if truth < 0 {
                    ptr::null_mut()
                } else {
                    let bool_value = if kind == UnaryOpKind::Not {
                        truth == 0
                    } else {
                        truth != 0
                    };
                    ffi::PyBool_FromLong(bool_value as libc::c_long)
                }
            }
        }
    };
    Ok(result.cast())
}

#[cold]
unsafe fn execute_binop_kind_owned(
    kind: BinOpKind,
    left: ObjPtr,
    right: ObjPtr,
) -> Result<ObjPtr, String> {
    let left = left.cast::<ffi::PyObject>();
    let right = right.cast::<ffi::PyObject>();
    let result = unsafe {
        match kind {
            BinOpKind::Add => ffi::PyNumber_Add(left, right),
            BinOpKind::Sub => ffi::PyNumber_Subtract(left, right),
            BinOpKind::Mul => ffi::PyNumber_Multiply(left, right),
            BinOpKind::MatMul => ffi::PyNumber_MatrixMultiply(left, right),
            BinOpKind::TrueDiv => ffi::PyNumber_TrueDivide(left, right),
            BinOpKind::FloorDiv => ffi::PyNumber_FloorDivide(left, right),
            BinOpKind::Mod => ffi::PyNumber_Remainder(left, right),
            BinOpKind::Pow => ffi::PyNumber_Power(left, right, ffi::Py_None()),
            BinOpKind::LShift => ffi::PyNumber_Lshift(left, right),
            BinOpKind::RShift => ffi::PyNumber_Rshift(left, right),
            BinOpKind::Or => ffi::PyNumber_Or(left, right),
            BinOpKind::Xor => ffi::PyNumber_Xor(left, right),
            BinOpKind::And => ffi::PyNumber_And(left, right),
            BinOpKind::Eq => ffi::PyObject_RichCompare(left, right, ffi::Py_EQ),
            BinOpKind::Ne => ffi::PyObject_RichCompare(left, right, ffi::Py_NE),
            BinOpKind::Lt => ffi::PyObject_RichCompare(left, right, ffi::Py_LT),
            BinOpKind::Le => ffi::PyObject_RichCompare(left, right, ffi::Py_LE),
            BinOpKind::Gt => ffi::PyObject_RichCompare(left, right, ffi::Py_GT),
            BinOpKind::Ge => ffi::PyObject_RichCompare(left, right, ffi::Py_GE),
            BinOpKind::Contains => {
                let contains = ffi::PySequence_Contains(right, left);
                if contains < 0 {
                    ptr::null_mut()
                } else {
                    ffi::PyBool_FromLong((contains != 0) as libc::c_long)
                }
            }
            BinOpKind::Is => ffi::PyBool_FromLong((left == right) as libc::c_long),
            BinOpKind::InplaceAdd => ffi::PyNumber_InPlaceAdd(left, right),
            BinOpKind::InplaceSub => ffi::PyNumber_InPlaceSubtract(left, right),
            BinOpKind::InplaceMul => ffi::PyNumber_InPlaceMultiply(left, right),
            BinOpKind::InplaceMatMul => ffi::PyNumber_InPlaceMatrixMultiply(left, right),
            BinOpKind::InplaceTrueDiv => ffi::PyNumber_InPlaceTrueDivide(left, right),
            BinOpKind::InplaceFloorDiv => ffi::PyNumber_InPlaceFloorDivide(left, right),
            BinOpKind::InplaceMod => ffi::PyNumber_InPlaceRemainder(left, right),
            BinOpKind::InplacePow => ffi::PyNumber_InPlacePower(left, right, ffi::Py_None()),
            BinOpKind::InplaceLShift => ffi::PyNumber_InPlaceLshift(left, right),
            BinOpKind::InplaceRShift => ffi::PyNumber_InPlaceRshift(left, right),
            BinOpKind::InplaceOr => ffi::PyNumber_InPlaceOr(left, right),
            BinOpKind::InplaceXor => ffi::PyNumber_InPlaceXor(left, right),
            BinOpKind::InplaceAnd => ffi::PyNumber_InPlaceAnd(left, right),
        }
    };
    Ok(result.cast())
}

#[cold]
unsafe fn execute_owned_positional_call(
    callable: ObjPtr,
    args: Vec<ObjPtr>,
) -> Result<ObjPtr, String> {
    let args_len = match ffi::Py_ssize_t::try_from(args.len()) {
        Ok(args_len) => args_len,
        Err(_) => {
            unsafe {
                release_owned_values(args);
                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
            }
            return Err("deopt continuation positional call has too many args".to_string());
        }
    };
    let tuple = unsafe { ffi::PyTuple_New(args_len) };
    if tuple.is_null() {
        unsafe {
            release_owned_values(args);
            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
        }
        return Ok(ptr::null_mut());
    }
    for (index, arg) in args.into_iter().enumerate() {
        let index = ffi::Py_ssize_t::try_from(index)
            .expect("tuple arg index should fit after tuple length conversion");
        if unsafe { ffi::PyTuple_SetItem(tuple, index, arg.cast::<ffi::PyObject>()) } != 0 {
            unsafe {
                ffi::Py_DECREF(tuple);
                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
    }
    let result = unsafe { ffi::PyObject_CallObject(callable.cast::<ffi::PyObject>(), tuple) };
    unsafe {
        ffi::Py_DECREF(tuple);
        ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
    }
    Ok(result.cast())
}

#[cold]
unsafe fn execute_runtime_name_deopt(name: &str) -> Result<ObjPtr, String> {
    let name_len = ffi::Py_ssize_t::try_from(name.len()).map_err(|_| {
        format!("runtime-name deopt name {name:?} is too large to materialize as PyUnicode")
    })?;
    let name_obj = unsafe { ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name_len) };
    if name_obj.is_null() {
        return Ok(ptr::null_mut());
    }
    let result = unsafe { load_runtime_name_owned(name_obj) as ObjPtr };
    unsafe { ffi::Py_DECREF(name_obj) };
    Ok(result)
}

#[cold]
fn set_deopt_unbound_local_error(name: &str) {
    let message =
        format!("cannot access local variable {name:?} where it is not associated with a value");
    if let Ok(c_message) = std::ffi::CString::new(message) {
        unsafe {
            ffi::PyErr_SetString(ffi::PyExc_UnboundLocalError, c_message.as_ptr());
        }
    } else {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_UnboundLocalError,
                b"cannot access local variable before assignment\0".as_ptr() as *const i8,
            );
        }
    }
}
