use crate::jit;
use log::info;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyTuple;
use soac_blockpy::block_py::{FunctionId, ParamKind};
use soac_blockpy::passes::CodegenBlockPyPass;
use std::any::Any;
use std::cell::RefCell;
use std::ffi::{CString, c_char, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::time::Instant;

unsafe extern "C" {
    fn PyFunction_SetVectorcall(func: *mut ffi::PyFunctionObject, vectorcall: ffi::vectorcallfunc);
}

fn panic_payload_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn set_runtime_error<T>(msg: &str) -> Result<T, ()> {
    unsafe {
        ffi::PyErr_SetString(ffi::PyExc_RuntimeError, CString::new(msg).unwrap().as_ptr());
    }
    Err(())
}

const CLIF_VECTORCALL_CAPSULE_NAME: &[u8] = b"soac.clif_vectorcall_data\0";
const CLIF_VECTORCALL_ATTR: &[u8] = b"__dp_clif_vectorcall_data\0";

thread_local! {
    static ACTIVE_MODULE_RUNTIME_STACK: RefCell<Vec<*mut jit::ModuleRuntimeContext>> = const {
        RefCell::new(Vec::new())
    };
}

struct ClifFunctionData {
    function: soac_blockpy::block_py::BlockPyFunction<CodegenBlockPyPass>,
    module_runtime: jit::ModuleRuntimeContext,
    compiled_handle: *mut c_void,
    compiled_vectorcall_handle: *mut c_void,
    compiled_vectorcall_entry: Option<jit::VectorcallEntryFn>,
}

fn set_type_error<T>(msg: &str) -> Result<T, ()> {
    unsafe {
        ffi::PyErr_SetString(ffi::PyExc_TypeError, CString::new(msg).unwrap().as_ptr());
    }
    Err(())
}

unsafe fn free_clif_function_data(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    let data = unsafe { Box::from_raw(ptr as *mut ClifFunctionData) };
    unsafe { jit::free_cranelift_run_bb_specialized_cached(data.compiled_handle) };
    unsafe { jit::free_cranelift_vectorcall_trampoline(data.compiled_vectorcall_handle) };
}

unsafe extern "C" fn free_clif_vectorcall_capsule(capsule: *mut ffi::PyObject) {
    if capsule.is_null() {
        return;
    }
    let ptr = unsafe {
        ffi::PyCapsule_GetPointer(
            capsule,
            CLIF_VECTORCALL_CAPSULE_NAME.as_ptr() as *const c_char,
        )
    };
    if !ptr.is_null() {
        unsafe { free_clif_function_data(ptr) };
    } else if !unsafe { ffi::PyErr_Occurred() }.is_null() {
        unsafe { ffi::PyErr_Clear() };
    }
}

unsafe fn py_string(obj: *mut ffi::PyObject) -> Result<String, ()> {
    if ffi::PyUnicode_Check(obj) == 0 {
        return set_type_error("expected string metadata while registering CLIF vectorcall");
    }
    let mut len = 0;
    let ptr = ffi::PyUnicode_AsUTF8AndSize(obj, &mut len);
    if ptr.is_null() {
        return Err(());
    }
    let bytes = std::slice::from_raw_parts(ptr as *const u8, len as usize);
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

unsafe fn lookup_deleted_sentinel() -> Result<*mut ffi::PyObject, ()> {
    let runtime = ffi::PyImport_ImportModule(c"soac.runtime".as_ptr());
    if runtime.is_null() {
        return set_runtime_error(
            "failed to import soac.runtime while resolving CLIF deleted sentinel",
        );
    }
    let deleted_obj = ffi::PyObject_GetAttrString(runtime, c"DELETED".as_ptr());
    ffi::Py_DECREF(runtime);
    if deleted_obj.is_null() {
        return set_runtime_error("missing soac.runtime.DELETED while registering CLIF vectorcall");
    }
    Ok(deleted_obj)
}

struct ActiveModuleVmCtxGuard;

impl Drop for ActiveModuleVmCtxGuard {
    fn drop(&mut self) {
        ACTIVE_MODULE_RUNTIME_STACK.with(|stack| {
            stack
                .borrow_mut()
                .pop()
                .expect("active module runtime stack should not underflow");
        });
    }
}

fn push_active_module_runtime_context(
    runtime: *mut jit::ModuleRuntimeContext,
) -> ActiveModuleVmCtxGuard {
    ACTIVE_MODULE_RUNTIME_STACK.with(|stack| stack.borrow_mut().push(runtime));
    ActiveModuleVmCtxGuard
}

pub unsafe fn with_active_module_runtime_context<R>(
    runtime: *mut jit::ModuleRuntimeContext,
    f: impl FnOnce() -> R,
) -> R {
    let _guard = push_active_module_runtime_context(runtime);
    f()
}

pub unsafe fn with_current_module_runtime_context<R>(
    f: impl FnOnce(&jit::ModuleRuntimeContext) -> R,
) -> Result<R, ()> {
    ACTIVE_MODULE_RUNTIME_STACK.with(|stack| {
        let stack = stack.borrow();
        let Some(runtime) = stack.last().copied() else {
            return set_runtime_error("missing active module runtime context");
        };
        Ok(f(unsafe { &*runtime }))
    })
}

pub unsafe fn clone_module_runtime_context(
    runtime: &jit::ModuleRuntimeContext,
) -> Result<jit::ModuleRuntimeContext, ()> {
    if runtime.vmctx.shared_module_state.is_null()
        || runtime.vmctx.globals_obj.is_null()
        || runtime.vmctx.global_slots.is_null()
        || runtime.vmctx.true_obj.is_null()
        || runtime.vmctx.false_obj.is_null()
        || runtime.vmctx.none_obj.is_null()
        || runtime.vmctx.deleted_obj.is_null()
        || runtime.vmctx.empty_tuple_obj.is_null()
    {
        return set_runtime_error("cannot clone incomplete module runtime context");
    }
    unsafe {
        ffi::Py_INCREF(runtime.vmctx.globals_obj as *mut ffi::PyObject);
        ffi::Py_INCREF(runtime.vmctx.true_obj as *mut ffi::PyObject);
        ffi::Py_INCREF(runtime.vmctx.false_obj as *mut ffi::PyObject);
        ffi::Py_INCREF(runtime.vmctx.none_obj as *mut ffi::PyObject);
        ffi::Py_INCREF(runtime.vmctx.deleted_obj as *mut ffi::PyObject);
        ffi::Py_INCREF(runtime.vmctx.empty_tuple_obj as *mut ffi::PyObject);
    }
    let shared_module_state_owner = runtime.shared_module_state_owner.clone();
    let global_cache_owner = runtime.global_cache_owner.clone();
    Ok(jit::ModuleRuntimeContext {
        vmctx: jit::JitModuleVmCtx {
            shared_module_state: std::sync::Arc::as_ptr(&shared_module_state_owner),
            globals_obj: runtime.vmctx.globals_obj,
            global_slots: runtime.vmctx.global_slots,
            true_obj: runtime.vmctx.true_obj,
            false_obj: runtime.vmctx.false_obj,
            none_obj: runtime.vmctx.none_obj,
            deleted_obj: runtime.vmctx.deleted_obj,
            empty_tuple_obj: runtime.vmctx.empty_tuple_obj,
        },
        shared_module_state_owner,
        global_cache_owner,
    })
}

pub unsafe fn build_module_runtime_context_for_module(
    module: *mut ffi::PyObject,
) -> Result<jit::ModuleRuntimeContext, ()> {
    let py = Python::assume_attached();
    if module.is_null() {
        return set_runtime_error("missing transformed module while building runtime context");
    }
    let module = unsafe { Bound::from_borrowed_ptr(py, module) };
    let shared_module_state =
        crate::module_type::SoacExtModule::clone_shared_state(module.as_any()).map_err(|err| {
            err.restore(py);
        })?;
    let globals_obj = unsafe { ffi::PyModule_GetDict(module.as_ptr()) };
    if globals_obj.is_null() {
        if unsafe { ffi::PyErr_Occurred() }.is_null() {
            return set_runtime_error(
                "missing transformed module globals while building runtime context",
            );
        }
        return Err(());
    };
    unsafe { ffi::Py_INCREF(globals_obj) };
    let true_obj = unsafe { ffi::PyBool_FromLong(1) };
    if true_obj.is_null() {
        return Err(());
    }
    let false_obj = unsafe { ffi::PyBool_FromLong(0) };
    if false_obj.is_null() {
        unsafe { ffi::Py_DECREF(true_obj) };
        return Err(());
    }
    let none_obj = py.None().as_ptr();
    unsafe { ffi::Py_INCREF(none_obj) };
    let deleted_obj = match unsafe { lookup_deleted_sentinel() } {
        Ok(value) => value,
        Err(()) => {
            unsafe {
                ffi::Py_DECREF(true_obj);
                ffi::Py_DECREF(false_obj);
                ffi::Py_DECREF(none_obj);
                ffi::Py_DECREF(globals_obj);
            }
            return Err(());
        }
    };
    let global_cache =
        crate::module_type::SoacExtModule::clone_or_init_global_cache(module.as_any(), globals_obj)
            .map_err(|err| {
                err.restore(py);
                unsafe {
                    ffi::Py_DECREF(true_obj);
                    ffi::Py_DECREF(false_obj);
                    ffi::Py_DECREF(none_obj);
                    ffi::Py_DECREF(deleted_obj);
                    ffi::Py_DECREF(globals_obj);
                }
            })?;
    let empty_tuple_obj = PyTuple::empty(py).as_ptr();
    unsafe { ffi::Py_INCREF(empty_tuple_obj) };
    Ok(jit::ModuleRuntimeContext {
        vmctx: jit::JitModuleVmCtx {
            shared_module_state: std::sync::Arc::as_ptr(&shared_module_state),
            globals_obj: globals_obj as *mut c_void,
            global_slots: global_cache.slots_ptr() as *mut c_void,
            true_obj: true_obj as *mut c_void,
            false_obj: false_obj as *mut c_void,
            none_obj: none_obj as *mut c_void,
            deleted_obj: deleted_obj as *mut c_void,
            empty_tuple_obj: empty_tuple_obj as *mut c_void,
        },
        shared_module_state_owner: shared_module_state,
        global_cache_owner: global_cache,
    })
}

unsafe fn make_clif_function_data(
    _callable: *mut ffi::PyObject,
    function_id: FunctionId,
    module_runtime: jit::ModuleRuntimeContext,
) -> Result<*mut c_void, ()> {
    let Some(blockpy_function) = module_runtime
        .shared_module_state_owner
        .lookup_function(function_id)
        .cloned()
    else {
        let module_name = module_runtime
            .shared_module_state_owner
            .module_name
            .as_str();
        let msg = format!(
            "no specialized JIT plan found: module={module_name:?} function_id={function_id:?}"
        );
        if let Ok(c_msg) = CString::new(msg) {
            ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
        } else {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"no specialized JIT plan found\0".as_ptr() as *const c_char,
            );
        }
        return Err(());
    };
    let clif_data = Box::new(ClifFunctionData {
        function: blockpy_function,
        module_runtime,
        compiled_handle: ptr::null_mut(),
        compiled_vectorcall_handle: ptr::null_mut(),
        compiled_vectorcall_entry: None,
    });
    Ok(Box::into_raw(clif_data) as *mut c_void)
}

unsafe fn clif_vectorcall_data(
    function: *mut ffi::PyObject,
) -> Result<&'static mut ClifFunctionData, ()> {
    if ffi::PyFunction_Check(function) == 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            b"expected Python function for CLIF vectorcall data lookup\0".as_ptr() as *const i8,
        );
        return Err(());
    }
    let dict = (*(function as *mut ffi::PyFunctionObject)).func_dict;
    if dict.is_null() {
        return set_runtime_error("missing CLIF vectorcall metadata dictionary");
    }
    let capsule = ffi::PyDict_GetItemString(dict, CLIF_VECTORCALL_ATTR.as_ptr() as *const c_char);
    if capsule.is_null() {
        return set_runtime_error("missing CLIF vectorcall metadata capsule");
    }
    let ptr = ffi::PyCapsule_GetPointer(
        capsule,
        CLIF_VECTORCALL_CAPSULE_NAME.as_ptr() as *const c_char,
    );
    if ptr.is_null() {
        return Err(());
    }
    Ok(&mut *(ptr as *mut ClifFunctionData))
}

pub unsafe fn registered_clif_function_id(
    function: *mut ffi::PyObject,
) -> Result<Option<FunctionId>, ()> {
    if ffi::PyFunction_Check(function) == 0 {
        return Ok(None);
    }
    let dict = (*(function as *mut ffi::PyFunctionObject)).func_dict;
    if dict.is_null() {
        return Ok(None);
    }
    let capsule = ffi::PyDict_GetItemString(dict, CLIF_VECTORCALL_ATTR.as_ptr() as *const c_char);
    if capsule.is_null() {
        return Ok(None);
    }
    let ptr = ffi::PyCapsule_GetPointer(
        capsule,
        CLIF_VECTORCALL_CAPSULE_NAME.as_ptr() as *const c_char,
    );
    if ptr.is_null() {
        return Err(());
    }
    let data = &*(ptr as *const ClifFunctionData);
    Ok(Some(data.function.function_id))
}

unsafe fn ensure_clif_vectorcall_compiled(
    _py: Python<'_>,
    callable: *mut ffi::PyObject,
    data: &mut ClifFunctionData,
) -> Result<(), ()> {
    if data.compiled_handle.is_null() {
        let compile_start = Instant::now();
        let block_ptrs = vec![ptr::null_mut::<c_void>(); data.function.blocks.len()];
        let module_constant_ptrs = data
            .module_runtime
            .shared_module_state_owner
            .module_constant_ptrs();
        let counter_ptrs = data.module_runtime.shared_module_state_owner.counter_ptrs();
        data.compiled_handle = match jit::compile_cranelift_run_bb_specialized_cached(
            block_ptrs.as_slice(),
            &data.module_runtime.shared_module_state_owner.lowered_module,
            &data.function,
            &data
                .module_runtime
                .shared_module_state_owner
                .codegen_constants,
            &data
                .module_runtime
                .shared_module_state_owner
                .lowered_module
                .counter_defs,
            &module_constant_ptrs,
            &counter_ptrs,
            Some(data.module_runtime.shared_module_state_owner.as_ref()),
        ) {
            Ok(handle) => handle,
            Err(err) => {
                if let Ok(c_msg) = CString::new(err) {
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                } else {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"failed to compile CLIF function body\0".as_ptr() as *const i8,
                    );
                }
                return Err(());
            }
        };
        let elapsed_ms = compile_start.elapsed().as_secs_f64() * 1000.0;
        info!(
            "soac_jit_precompile module={} qualname={} blocks={} elapsed_ms={elapsed_ms:.3}",
            data.module_runtime.shared_module_state_owner.module_name,
            data.function.names.qualname,
            data.function.blocks.len(),
        );
    }
    if data.compiled_vectorcall_handle.is_null() {
        let vectorcall_symbol = jit::jit_python_perf_symbol_name(
            jit::JIT_PYTHON_PERF_SYMBOL_KIND_VECTORCALL,
            data.function.names.qualname.as_str(),
        );
        let (handle, entry) = match jit::compile_cranelift_vectorcall_direct_trampoline(
            bind_direct_args_from_vectorcall,
            data as *mut ClifFunctionData as *mut c_void,
            ptr::addr_of!(data.module_runtime.vmctx) as *mut c_void,
            data.compiled_handle,
            &vectorcall_symbol,
        ) {
            Ok(value) => value,
            Err(err) => {
                if let Ok(c_msg) = CString::new(err) {
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                } else {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"failed to compile direct CLIF vectorcall trampoline\0".as_ptr()
                            as *const i8,
                    );
                }
                return Err(());
            }
        };
        data.compiled_vectorcall_handle = handle;
        data.compiled_vectorcall_entry = Some(entry);
        let vectorcall_entry: ffi::vectorcallfunc = std::mem::transmute(entry);
        PyFunction_SetVectorcall(callable as *mut ffi::PyFunctionObject, vectorcall_entry);
    }
    Ok(())
}

unsafe fn cleanup_state_values(state_values: &mut [*mut ffi::PyObject]) {
    for value in state_values.iter_mut() {
        if !value.is_null() {
            ffi::Py_DECREF(*value);
            *value = ptr::null_mut();
        }
    }
}

unsafe fn bound_arg_value_from_borrowed(
    bound_args: &mut [*mut ffi::PyObject],
    param_index: usize,
    value: *mut ffi::PyObject,
) {
    ffi::Py_INCREF(value);
    bound_args[param_index] = value;
}

unsafe fn bound_arg_value_from_owned(
    bound_args: &mut [*mut ffi::PyObject],
    param_index: usize,
    value: *mut ffi::PyObject,
) {
    bound_args[param_index] = value;
}

unsafe fn build_function_bound_args(
    callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
    function: &soac_blockpy::block_py::BlockPyFunction<CodegenBlockPyPass>,
) -> Result<Vec<*mut ffi::PyObject>, ()> {
    if callable.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"null callable in CLIF function binding\0".as_ptr() as *const i8,
        );
        return Err(());
    }
    let params = &function.params;
    let callable_name = function.names.display_name.as_str();
    let nargs = ffi::PyVectorcall_NARGS(nargsf) as usize;
    let nkw = if kwnames.is_null() {
        0
    } else {
        ffi::PyTuple_GET_SIZE(kwnames) as usize
    };
    let mut bound_args = vec![ptr::null_mut(); params.len()];
    let mut assigned = vec![false; params.len()];
    let positional_param_indices = params.positional_param_indices();
    let positional_capacity = positional_param_indices.len();
    let varargs_param = params.vararg_index();
    let varkw_param = params.kwarg_index();

    if varargs_param.is_none() && nargs > positional_capacity {
        cleanup_state_values(&mut bound_args);
        let msg = format!(
            "{}() takes {} positional argument{} but {} {} given",
            callable_name,
            positional_capacity,
            if positional_capacity == 1 { "" } else { "s" },
            nargs,
            if nargs == 1 { "was" } else { "were" }
        );
        let _ = set_type_error::<()>(&msg);
        return Err(());
    }

    let positional_bound = nargs.min(positional_capacity);
    for position in 0..positional_bound {
        let param_index = positional_param_indices[position];
        let value = *args.add(position);
        if value.is_null() {
            cleanup_state_values(&mut bound_args);
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"null vectorcall positional argument\0".as_ptr() as *const i8,
            );
            return Err(());
        }
        bound_arg_value_from_borrowed(&mut bound_args, param_index, value);
        assigned[param_index] = true;
    }

    if let Some(varargs_param) = varargs_param {
        let extras = nargs.saturating_sub(positional_capacity);
        let extra_tuple = ffi::PyTuple_New(extras as ffi::Py_ssize_t);
        if extra_tuple.is_null() {
            cleanup_state_values(&mut bound_args);
            return Err(());
        }
        for offset in 0..extras {
            let value = *args.add(positional_capacity + offset);
            if value.is_null() {
                ffi::Py_DECREF(extra_tuple);
                cleanup_state_values(&mut bound_args);
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"null vectorcall positional vararg\0".as_ptr() as *const i8,
                );
                return Err(());
            }
            ffi::Py_INCREF(value);
            if ffi::PyTuple_SetItem(extra_tuple, offset as ffi::Py_ssize_t, value) != 0 {
                ffi::Py_DECREF(value);
                ffi::Py_DECREF(extra_tuple);
                cleanup_state_values(&mut bound_args);
                return Err(());
            }
        }
        bound_arg_value_from_owned(&mut bound_args, varargs_param, extra_tuple);
        assigned[varargs_param] = true;
    }

    let has_varkw = varkw_param.is_some();
    let mut varkw_dict = ptr::null_mut();
    if let Some(varkw_param) = varkw_param {
        varkw_dict = ffi::PyDict_New();
        if varkw_dict.is_null() {
            cleanup_state_values(&mut bound_args);
            return Err(());
        }
        bound_arg_value_from_owned(&mut bound_args, varkw_param, varkw_dict);
        assigned[varkw_param] = true;
    }

    for kw_index in 0..nkw {
        let key = ffi::PyTuple_GetItem(kwnames, kw_index as ffi::Py_ssize_t);
        if key.is_null() {
            cleanup_state_values(&mut bound_args);
            return Err(());
        }
        let value = *args.add(nargs + kw_index);
        if value.is_null() {
            cleanup_state_values(&mut bound_args);
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"null vectorcall keyword argument\0".as_ptr() as *const i8,
            );
            return Err(());
        }
        let key_name = match py_string(key) {
            Ok(name) => name,
            Err(()) => {
                cleanup_state_values(&mut bound_args);
                return Err(());
            }
        };
        if let Some(param_index) = params.param_index(key_name.as_str()) {
            let param = &params.params[param_index];
            match param.kind {
                ParamKind::PosOnly | ParamKind::VarArg => {
                    if !has_varkw {
                        cleanup_state_values(&mut bound_args);
                        let msg = format!(
                            "{}() got an unexpected keyword argument '{}'",
                            callable_name, key_name
                        );
                        let _ = set_type_error::<()>(&msg);
                        return Err(());
                    }
                    if !varkw_dict.is_null() && ffi::PyDict_SetItem(varkw_dict, key, value) != 0 {
                        cleanup_state_values(&mut bound_args);
                        return Err(());
                    }
                }
                ParamKind::Any | ParamKind::KwOnly => {
                    if assigned[param_index] {
                        cleanup_state_values(&mut bound_args);
                        let msg = format!(
                            "{}() got multiple values for argument '{}'",
                            callable_name, key_name
                        );
                        let _ = set_type_error::<()>(&msg);
                        return Err(());
                    }
                    bound_arg_value_from_borrowed(&mut bound_args, param_index, value);
                    assigned[param_index] = true;
                }
                ParamKind::KwArg => {
                    if !varkw_dict.is_null() && ffi::PyDict_SetItem(varkw_dict, key, value) != 0 {
                        cleanup_state_values(&mut bound_args);
                        return Err(());
                    }
                }
            }
        } else if has_varkw {
            if !varkw_dict.is_null() && ffi::PyDict_SetItem(varkw_dict, key, value) != 0 {
                cleanup_state_values(&mut bound_args);
                return Err(());
            }
        } else {
            cleanup_state_values(&mut bound_args);
            let msg = format!(
                "{}() got an unexpected keyword argument '{}'",
                callable_name, key_name
            );
            let _ = set_type_error::<()>(&msg);
            return Err(());
        }
    }

    for (param_index, param) in params.iter().enumerate() {
        if assigned[param_index] {
            continue;
        }
        match param.kind {
            ParamKind::VarArg | ParamKind::KwArg => {}
            _ => {
                if param.has_default {
                    assigned[param_index] = true;
                    continue;
                }
                cleanup_state_values(&mut bound_args);
                let msg = format!(
                    "{}() missing required argument '{}'",
                    callable_name, param.name
                );
                let _ = set_type_error::<()>(&msg);
                return Err(());
            }
        }
    }
    Ok(bound_args)
}

unsafe fn write_owned_bound_args_to_buffer(
    mut bound_args: Vec<*mut ffi::PyObject>,
    out_args: *mut *mut ffi::PyObject,
    out_len: usize,
) -> Result<(), ()> {
    if bound_args.len() != out_len {
        cleanup_state_values(&mut bound_args);
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"bound CLIF argument count did not match direct entry arity\0".as_ptr() as *const i8,
        );
        return Err(());
    }
    if out_len == 0 {
        cleanup_state_values(&mut bound_args);
        return Ok(());
    }
    if out_args.is_null() {
        cleanup_state_values(&mut bound_args);
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"missing output buffer for direct CLIF function arguments\0".as_ptr() as *const i8,
        );
        return Err(());
    }
    for (index, value) in bound_args.iter_mut().enumerate() {
        let owned = *value;
        *out_args.add(index) = owned;
        *value = ptr::null_mut();
    }
    cleanup_state_values(&mut bound_args);
    Ok(())
}

unsafe extern "C" fn bind_direct_args_from_vectorcall(
    callable: *mut c_void,
    args: *const *mut c_void,
    nargsf: usize,
    kwnames: *mut c_void,
    data_ptr: *mut c_void,
    out_args: *mut *mut c_void,
    out_len: i64,
) -> i32 {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        if callable.is_null() || data_ptr.is_null() || out_len < 0 {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"invalid direct vectorcall bind input\0".as_ptr() as *const i8,
            );
            return 0;
        }
        let data = &mut *(data_ptr as *mut ClifFunctionData);
        let bound_args = match build_function_bound_args(
            callable as *mut ffi::PyObject,
            args as *const *mut ffi::PyObject,
            nargsf,
            kwnames as *mut ffi::PyObject,
            &data.function,
        ) {
            Ok(value) => value,
            Err(()) => return 0,
        };
        match write_owned_bound_args_to_buffer(
            bound_args,
            out_args as *mut *mut ffi::PyObject,
            out_len as usize,
        ) {
            Ok(()) => 1,
            Err(()) => 0,
        }
    })) {
        Ok(value) => value,
        Err(payload) => {
            let message = format!(
                "panic in bind_direct_args_from_vectorcall: {}",
                panic_payload_to_string(payload)
            );
            if let Ok(c_msg) = CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"panic in bind_direct_args_from_vectorcall\0".as_ptr() as *const i8,
                );
            }
            0
        }
    }
}

unsafe extern "C" fn lazy_clif_vectorcall(
    callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        let py = Python::assume_attached();
        let data = match clif_vectorcall_data(callable) {
            Ok(value) => value,
            Err(()) => return ptr::null_mut(),
        };
        if ensure_clif_vectorcall_compiled(py, callable, data).is_err() {
            return ptr::null_mut();
        }
        let Some(entry) = data.compiled_vectorcall_entry else {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"missing compiled CLIF vectorcall entry\0".as_ptr() as *const i8,
            );
            return ptr::null_mut();
        };
        if ffi::Py_EnterRecursiveCall(b" while calling a Python object\0".as_ptr() as *const i8)
            != 0
        {
            return ptr::null_mut();
        }
        struct RecursiveCallGuard;
        impl Drop for RecursiveCallGuard {
            fn drop(&mut self) {
                unsafe { ffi::Py_LeaveRecursiveCall() };
            }
        }
        let _recursive_call_guard = RecursiveCallGuard;
        unsafe {
            let runtime = std::ptr::addr_of_mut!(data.module_runtime);
            with_active_module_runtime_context(runtime, || {
                entry(
                    callable as *mut c_void,
                    args as *const *mut c_void,
                    nargsf,
                    kwnames as *mut c_void,
                ) as *mut ffi::PyObject
            })
        }
    })) {
        Ok(value) => value,
        Err(payload) => {
            let message = format!(
                "panic in lazy_clif_vectorcall: {}",
                panic_payload_to_string(payload)
            );
            if let Ok(c_msg) = CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"panic in lazy_clif_vectorcall\0".as_ptr() as *const i8,
                );
            }
            ptr::null_mut()
        }
    }
}

pub unsafe fn register_clif_vectorcall(
    function: *mut ffi::PyObject,
    function_id: FunctionId,
    module_runtime: jit::ModuleRuntimeContext,
) -> Result<(), ()> {
    if ffi::PyFunction_Check(function) == 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            b"register_clif_vectorcall expects a Python function\0".as_ptr() as *const c_char,
        );
        return Err(());
    }
    let func = function as *mut ffi::PyFunctionObject;
    if !(*func).func_dict.is_null()
        && !ffi::PyDict_GetItemString(
            (*func).func_dict,
            CLIF_VECTORCALL_ATTR.as_ptr() as *const c_char,
        )
        .is_null()
    {
        PyFunction_SetVectorcall(func, lazy_clif_vectorcall);
        return Ok(());
    }

    let data_ptr = make_clif_function_data(function, function_id, module_runtime)?;
    let capsule = ffi::PyCapsule_New(
        data_ptr,
        CLIF_VECTORCALL_CAPSULE_NAME.as_ptr() as *const c_char,
        Some(free_clif_vectorcall_capsule),
    );
    if capsule.is_null() {
        free_clif_function_data(data_ptr);
        return Err(());
    }
    if (*func).func_dict.is_null() {
        (*func).func_dict = ffi::PyDict_New();
        if (*func).func_dict.is_null() {
            ffi::Py_DECREF(capsule);
            return Err(());
        }
    }
    if ffi::PyDict_SetItemString(
        (*func).func_dict,
        CLIF_VECTORCALL_ATTR.as_ptr() as *const c_char,
        capsule,
    ) != 0
    {
        ffi::Py_DECREF(capsule);
        return Err(());
    }
    ffi::Py_DECREF(capsule);
    PyFunction_SetVectorcall(func, lazy_clif_vectorcall);
    Ok(())
}

pub unsafe fn compile_clif_vectorcall(function: *mut ffi::PyObject) -> Result<(), ()> {
    if ffi::PyFunction_Check(function) == 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            b"compile_clif_vectorcall expects a Python function\0".as_ptr() as *const c_char,
        );
        return Err(());
    }
    let py = Python::assume_attached();
    let data = clif_vectorcall_data(function)?;
    ensure_clif_vectorcall_compiled(py, function, data)
}
