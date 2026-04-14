use super::{
    RuntimeJitDeoptContinuation, RuntimeJitDeoptInvocation, RuntimeJitDeoptLocals,
    specialized_helpers::ObjPtr,
};
use crate::module_constants::load_runtime_name_owned;
use pyo3::ffi;
use soac_blockpy::block_py::{BlockLabel, BlockTerm, InstrCodegen, LocalLocation, NameLocation};
use std::ffi::c_void;
use std::ptr;

#[cold]
pub(super) fn execute_deopt_invocation(
    invocation: &RuntimeJitDeoptInvocation<'_>,
) -> Result<ObjPtr, String> {
    let locals = invocation.materialize_locals()?;
    execute_blockpy_deopt_continuation(invocation, &locals)
}

#[cold]
fn execute_blockpy_deopt_continuation(
    invocation: &RuntimeJitDeoptInvocation<'_>,
    locals: &RuntimeJitDeoptLocals<'_>,
) -> Result<ObjPtr, String> {
    match invocation.record().continuation() {
        RuntimeJitDeoptContinuation::ReturnLocal { name, location } => {
            execute_return_local_deopt(locals, name, *location)
        }
        RuntimeJitDeoptContinuation::ReturnGlobal {
            name,
            expected_index,
        } => unsafe { execute_return_global_deopt(invocation, name, *expected_index) },
        RuntimeJitDeoptContinuation::ResumeBlockTail {
            block,
            start_body_index,
        } => unsafe { execute_block_tail_deopt(invocation, locals, *block, *start_body_index) },
        RuntimeJitDeoptContinuation::Unimplemented => {
            Err(format!("{}, {}", invocation.describe(), locals.describe()))
        }
    }
}

#[cold]
unsafe fn execute_block_tail_deopt(
    invocation: &RuntimeJitDeoptInvocation<'_>,
    locals: &RuntimeJitDeoptLocals<'_>,
    block_label: BlockLabel,
    start_body_index: usize,
) -> Result<ObjPtr, String> {
    let function = invocation.function();
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
        let value = unsafe { execute_deopt_expr_owned(invocation, locals, instr)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        unsafe {
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
    }
    match &block.term {
        BlockTerm::Return(value) => unsafe { execute_deopt_expr_owned(invocation, locals, value) },
        _ => Err(format!(
            "deopt continuation for block {block_label} only supports return terms"
        )),
    }
}

#[cold]
unsafe fn execute_deopt_expr_owned(
    invocation: &RuntimeJitDeoptInvocation<'_>,
    locals: &RuntimeJitDeoptLocals<'_>,
    expr: &InstrCodegen,
) -> Result<ObjPtr, String> {
    match expr {
        InstrCodegen::Load(load) => unsafe {
            execute_deopt_load_owned(
                invocation,
                locals,
                load.name.id.as_str(),
                load.name.location,
            )
        },
        _ => Err(format!(
            "deopt continuation only supports simple load expressions, got {expr:?}"
        )),
    }
}

#[cold]
unsafe fn execute_deopt_load_owned(
    invocation: &RuntimeJitDeoptInvocation<'_>,
    locals: &RuntimeJitDeoptLocals<'_>,
    name: &str,
    location: NameLocation,
) -> Result<ObjPtr, String> {
    match location {
        NameLocation::Local(location) => execute_return_local_deopt(locals, name, location),
        NameLocation::Global(slot) => unsafe {
            execute_return_global_deopt(invocation, name, i64::from(slot.slot()))
        },
        NameLocation::GlobalName => unsafe { execute_return_global_deopt(invocation, name, -1) },
        NameLocation::RuntimeName => unsafe { execute_runtime_name_deopt(name) },
        NameLocation::Cell(_) | NameLocation::Constant(_) => Err(format!(
            "deopt continuation does not support loading {location:?} for {name:?}"
        )),
    }
}

#[cold]
fn execute_return_local_deopt(
    locals: &RuntimeJitDeoptLocals<'_>,
    name: &str,
    location: LocalLocation,
) -> Result<ObjPtr, String> {
    let local = locals.get_by_location(location).ok_or_else(|| {
        format!(
            "deopt continuation expected local {name} at {location:?}, but it was not materialized: {}",
            locals.describe()
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
unsafe fn execute_return_global_deopt(
    invocation: &RuntimeJitDeoptInvocation<'_>,
    name: &str,
    expected_index: i64,
) -> Result<ObjPtr, String> {
    let globals_obj = invocation.globals_obj();
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
