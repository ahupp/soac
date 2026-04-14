use super::{
    RuntimeJitDeoptCursor, RuntimeJitDeoptInvocation, RuntimeJitDeoptLocals,
    specialized_helpers::ObjPtr,
};
use crate::module_constants::load_runtime_name_owned;
use pyo3::ffi;
use soac_blockpy::block_py::{
    BinOp, BinOpKind, BlockTerm, CallArgKeyword, CallArgPositional, InstrCodegen, LocalLocation,
    NameLocation, UnaryOp, UnaryOpKind,
};
use std::ffi::c_void;
use std::ptr;

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
}

impl<'inv, 'data> BlockPyDeoptFrame<'inv, 'data> {
    #[cold]
    fn new(invocation: &'inv RuntimeJitDeoptInvocation<'data>) -> Result<Self, String> {
        let locals = invocation.materialize_locals()?;
        Ok(Self { invocation, locals })
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
        loop {
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
                    return Ok(ptr::null_mut());
                }
                unsafe {
                    ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                }
            }
            match &block.term {
                BlockTerm::Return(value) => return unsafe { self.execute_expr_owned(value) },
                BlockTerm::Jump(edge) if edge.args.is_empty() => {
                    cursor = RuntimeJitDeoptCursor::at_block_entry(edge.target);
                }
                BlockTerm::IfTerm(if_term) => {
                    let test = unsafe { self.execute_expr_owned(&if_term.test)? };
                    if test.is_null() {
                        return Ok(ptr::null_mut());
                    }
                    let truth = unsafe { ffi::PyObject_IsTrue(test.cast::<ffi::PyObject>()) };
                    unsafe {
                        ffi::Py_DECREF(test.cast::<ffi::PyObject>());
                    }
                    if truth < 0 {
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
                        return Ok(ptr::null_mut());
                    }
                    let next_block = usize::try_from(index)
                        .ok()
                        .and_then(|index| branch.targets.get(index).copied())
                        .unwrap_or(branch.default_label);
                    cursor = RuntimeJitDeoptCursor::at_block_entry(next_block);
                }
                BlockTerm::Jump(edge) => {
                    return Err(format!(
                        "deopt continuation for block {block_label} does not support jump args {:?}",
                        edge.args
                    ));
                }
                _ => {
                    return Err(format!(
                        "deopt continuation for block {block_label} only supports return, if, branch-table, or no-arg jump terms"
                    ));
                }
            }
        }
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
            InstrCodegen::Call(call) => unsafe { self.execute_call_owned(call) },
            InstrCodegen::CallDirect(call) => unsafe { self.execute_call_direct_owned(call) },
            InstrCodegen::Store(store) => unsafe { self.execute_store_owned(store) },
            InstrCodegen::Del(del) => unsafe { self.execute_del_owned(del) },
            _ => Err(format!(
                "deopt continuation only supports simple load/binop/call/store/del expressions, got {expr:?}"
            )),
        }
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
            let CallArgPositional::Positional(expr) = arg else {
                unsafe {
                    release_owned_values(args);
                    ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                }
                return Err(
                    "deopt continuation does not support starred call arguments".to_string()
                );
            };
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
                let CallArgKeyword::Named { arg, value } = keyword else {
                    unsafe {
                        ffi::Py_DECREF(kwargs);
                        ffi::Py_DECREF(tuple);
                        ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                    }
                    return Err(
                        "deopt continuation does not support starred keyword arguments".to_string(),
                    );
                };
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
                let rc = unsafe { ffi::PyDict_SetItem(kwargs, key, value.cast::<ffi::PyObject>()) };
                unsafe {
                    ffi::Py_DECREF(key);
                    ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                }
                if rc != 0 {
                    unsafe {
                        ffi::Py_DECREF(kwargs);
                        ffi::Py_DECREF(tuple);
                        ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                    }
                    return Ok(ptr::null_mut());
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
            NameLocation::Cell(_) => Err(format!(
                "deopt continuation does not support loading {location:?} for {name:?}"
            )),
        }
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
        }
    }
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
