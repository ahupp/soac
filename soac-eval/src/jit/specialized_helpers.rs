use cranelift_jit::JITBuilder;
use libc;
use pyo3::ffi;
use std::ffi::c_void;
use std::ptr;
use std::sync::OnceLock;

#[cfg(not(test))]
use crate::module_globals::ModuleGlobalCache;

use std::sync::atomic::{AtomicPtr, Ordering};
use crate::module_constants::raise_name_error_for_missing_name;
#[cfg(not(test))]
use crate::module_constants::load_runtime_name_owned;

#[cfg(not(test))]
unsafe extern "C" {
    static mut PyCell_Type: ffi::PyTypeObject;
    static mut PyMethod_Type: ffi::PyTypeObject;
    fn PyCell_New(obj: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyCell_Get(cell: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyCell_Set(cell: *mut ffi::PyObject, value: *mut ffi::PyObject) -> libc::c_int;
    fn PyErr_SetRaisedException(exc: *mut ffi::PyObject);
    fn PyMethod_Function(method: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyFunction_GetSoacFunctionId(function: *mut ffi::PyObject) -> u64;
}

pub type ObjPtr = *mut c_void;

#[cfg(not(test))]
unsafe fn is_cell_object(obj: *mut ffi::PyObject) -> bool {
    !obj.is_null() && ffi::Py_TYPE(obj) == std::ptr::addr_of_mut!(PyCell_Type)
}

#[cfg(not(test))]
unsafe fn object_type_name(obj: *mut ffi::PyObject) -> String {
    if obj.is_null() {
        return "<null>".to_string();
    }
    let ty = ffi::Py_TYPE(obj);
    if ty.is_null() || (*ty).tp_name.is_null() {
        return "<unknown>".to_string();
    }
    std::ffi::CStr::from_ptr((*ty).tp_name)
        .to_string_lossy()
        .into_owned()
}

#[cfg(not(test))]
unsafe fn raise_expected_cell(where_name: &str, obj: *mut ffi::PyObject) {
    let type_name = object_type_name(obj);
    let message = format!("{where_name} expected cell object, got {type_name}");
    if let Ok(c_message) = std::ffi::CString::new(message) {
        ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_message.as_ptr());
    } else {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"expected cell object\0".as_ptr() as *const i8,
        );
    }
}

#[cfg(not(test))]
unsafe extern "C" fn py_call_positional_three_hook(
    callable: ObjPtr,
    arg1: ObjPtr,
    arg2: ObjPtr,
    arg3: ObjPtr,
) -> ObjPtr {
    ffi::PyObject_CallFunctionObjArgs(
        callable as *mut ffi::PyObject,
        arg1 as *mut ffi::PyObject,
        arg2 as *mut ffi::PyObject,
        arg3 as *mut ffi::PyObject,
        ptr::null_mut::<ffi::PyObject>(),
    ) as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn py_call_object_hook(callable: ObjPtr, args: ObjPtr) -> ObjPtr {
    ffi::PyObject_CallObject(callable as *mut ffi::PyObject, args as *mut ffi::PyObject) as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn py_vectorcall_hook(
    callable: ObjPtr,
    args: ObjPtr,
    nargsf: ObjPtr,
    kwnames: ObjPtr,
) -> ObjPtr {
    ffi::PyObject_Vectorcall(
        callable as *mut ffi::PyObject,
        args as *const *mut ffi::PyObject,
        nargsf as usize,
        kwnames as *mut ffi::PyObject,
    ) as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn callee_function_id_hook(callable: ObjPtr) -> i64 {
    let callable = callable as *mut ffi::PyObject;
    if callable.is_null() {
        return i64::MIN;
    }
    let function = if ffi::PyFunction_Check(callable) != 0 {
        callable
    } else if ffi::Py_TYPE(callable) == std::ptr::addr_of_mut!(PyMethod_Type) {
        PyMethod_Function(callable)
    } else {
        return 0;
    };
    if function.is_null() {
        return i64::MIN;
    }
    let packed_plus_one = PyFunction_GetSoacFunctionId(function);
    if packed_plus_one == 0 {
        return 0;
    }
    (packed_plus_one - 1) as i64
}

#[cfg(not(test))]
unsafe extern "C" fn guard_method_type_version_hook(
    receiver: ObjPtr,
    expected_type: ObjPtr,
    expected_version: i64,
) -> i32 {
    if receiver.is_null() || expected_type.is_null() || expected_version < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_guard_method_type_version\0".as_ptr() as *const i8,
        );
        return -1;
    }
    let receiver_type = ffi::Py_TYPE(receiver as *mut ffi::PyObject);
    if receiver_type != expected_type as *mut ffi::PyTypeObject {
        return 0;
    }
    ((*receiver_type).tp_version_tag == expected_version as u32) as i32
}

#[cfg(not(test))]
unsafe extern "C" fn lookup_direct_code_ptr_hook(vmctx: ObjPtr, function_id: i64) -> ObjPtr {
    if vmctx.is_null() || function_id < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_lookup_direct_code_ptr\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let vmctx = &*(vmctx as *const super::vmctx::JitModuleVmCtx);
    let Some(shared_state) = vmctx.shared_module_state.as_ref() else {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"missing shared module state for direct JIT call\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    };
    match shared_state.lookup_or_compile_direct_code_ptr(soac_blockpy::block_py::FunctionId::from_packed(function_id as u64))
    {
        Ok(Some(code_ptr)) => code_ptr,
        Ok(None) => ptr::null_mut(),
        Err(err) => {
            if let Ok(message) = std::ffi::CString::new(err) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, message.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"failed to resolve direct JIT code pointer\0".as_ptr() as *const i8,
                );
            }
            ptr::null_mut()
        }
    }
}

#[cfg(not(test))]
unsafe extern "C" fn py_call_with_kw_hook(
    callable: ObjPtr,
    args: ObjPtr,
    kwargs: ObjPtr,
) -> ObjPtr {
    ffi::PyObject_Call(
        callable as *mut ffi::PyObject,
        args as *mut ffi::PyObject,
        kwargs as *mut ffi::PyObject,
    ) as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn record_counter_value_hook(vmctx: ObjPtr, counter_id: i64, value: i64) {
    if vmctx.is_null() || counter_id < 0 || value < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_record_counter_value\0".as_ptr() as *const i8,
        );
        return;
    }
    let vmctx = &*(vmctx as *const super::vmctx::JitModuleVmCtx);
    let Some(shared_state) = vmctx.shared_module_state.as_ref() else {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"missing shared module state for counter recording\0".as_ptr() as *const i8,
        );
        return;
    };
    if let Err(err) = shared_state.record_call_target_counter(
        soac_blockpy::block_py::CounterId(counter_id as usize),
        value as u64,
    ) {
        if let Ok(message) = std::ffi::CString::new(err) {
            ffi::PyErr_SetString(ffi::PyExc_RuntimeError, message.as_ptr());
        } else {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"failed to record counter value\0".as_ptr() as *const i8,
            );
        }
    }
}

#[cfg(not(test))]
unsafe extern "C" fn py_get_raised_exception_hook() -> ObjPtr {
    ffi::PyErr_GetRaisedException() as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn get_arg_item_hook(args: ObjPtr, index: i64) -> ObjPtr {
    if args.is_null() {
        return ptr::null_mut();
    }
    ffi::PySequence_GetItem(args as *mut ffi::PyObject, index as ffi::Py_ssize_t) as ObjPtr
}

unsafe fn load_global_obj_impl(
    globals_obj: ObjPtr,
    global_slots_obj: ObjPtr,
    name_obj: *mut ffi::PyObject,
    slot_index: i64,
) -> ObjPtr {
    if globals_obj.is_null() || global_slots_obj.is_null() || name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_load_global_obj\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let value = ffi::PyObject_GetItem(globals_obj as *mut ffi::PyObject, name_obj);
    if !value.is_null() {
        if slot_index >= 0 {
            let entry =
                (global_slots_obj as *mut AtomicPtr<ffi::PyObject>).add(slot_index as usize);
            let old = (*entry).swap(value, Ordering::AcqRel);
            if !old.is_null() {
                ffi::Py_DECREF(old);
            }
            ffi::Py_INCREF(value);
        }
        return value as ObjPtr;
    }
    if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0 {
        return ptr::null_mut();
    }
    ffi::PyErr_Clear();
    let builtins_dict = ffi::PyEval_GetBuiltins();
    if builtins_dict.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"PyEval_GetBuiltins returned null\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let builtin_value = ffi::PyObject_GetItem(builtins_dict as *mut ffi::PyObject, name_obj);
    if !builtin_value.is_null() {
        if slot_index >= 0 {
            let entry =
                (global_slots_obj as *mut AtomicPtr<ffi::PyObject>).add(slot_index as usize);
            let old = (*entry).swap(builtin_value, Ordering::AcqRel);
            if !old.is_null() {
                ffi::Py_DECREF(old);
            }
            ffi::Py_INCREF(builtin_value);
        }
        return builtin_value as ObjPtr;
    }
    if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0 {
        return ptr::null_mut();
    }
    ffi::PyErr_Clear();
    raise_name_error_for_missing_name(name_obj);
    ptr::null_mut()
}

#[cfg(not(test))]
unsafe fn resolve_function_object(callable: ObjPtr) -> ObjPtr {
    if callable.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid null callable for JIT function lookup\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let function = ffi::PyObject_GetAttrString(
        callable as *mut ffi::PyObject,
        b"__func__\0".as_ptr() as *const i8,
    );
    if !function.is_null() {
        return function as ObjPtr;
    }
    if ffi::PyErr_ExceptionMatches(ffi::PyExc_AttributeError) == 0 {
        return ptr::null_mut();
    }
    ffi::PyErr_Clear();
    ffi::Py_INCREF(callable as *mut ffi::PyObject);
    callable
}

#[cfg(not(test))]
unsafe fn resolve_function_defaults_owner(callable: ObjPtr) -> ObjPtr {
    if callable.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid null callable for JIT function default lookup\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    resolve_function_object(callable)
}

#[cfg(not(test))]
unsafe fn raise_missing_function_default_obj(name_obj: *mut ffi::PyObject) {
    if name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            b"missing required argument\0".as_ptr() as *const i8,
        );
        return;
    }
    let repr = ffi::PyObject_Repr(name_obj);
    if !repr.is_null() {
        let repr_utf8 = ffi::PyUnicode_AsUTF8(repr);
        if !repr_utf8.is_null() {
            let repr_text = std::ffi::CStr::from_ptr(repr_utf8).to_string_lossy();
            let message = format!("missing required argument {repr_text}");
            ffi::Py_DECREF(repr);
            if let Ok(c_msg) = std::ffi::CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_TypeError, c_msg.as_ptr());
                return;
            }
        } else {
            ffi::PyErr_Clear();
        }
        ffi::Py_DECREF(repr);
    }
    ffi::PyErr_SetString(
        ffi::PyExc_TypeError,
        b"missing required argument\0".as_ptr() as *const i8,
    );
}

#[cfg(not(test))]
unsafe extern "C" fn function_positional_default_obj_hook(
    callable: ObjPtr,
    name_obj: ObjPtr,
    index: i64,
) -> ObjPtr {
    if name_obj.is_null() || index < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_function_positional_default_obj\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let owner = resolve_function_defaults_owner(callable);
    if owner.is_null() {
        return ptr::null_mut();
    }
    let defaults = ffi::PyObject_GetAttrString(
        owner as *mut ffi::PyObject,
        b"__defaults__\0".as_ptr() as *const i8,
    );
    if defaults.is_null() {
        ffi::Py_DECREF(owner as *mut ffi::PyObject);
        if ffi::PyErr_ExceptionMatches(ffi::PyExc_AttributeError) != 0 {
            ffi::PyErr_Clear();
            raise_missing_function_default_obj(name_obj as *mut ffi::PyObject);
        }
        return ptr::null_mut();
    }
    if defaults == ffi::Py_None() || ffi::PyTuple_Check(defaults) == 0 {
        ffi::Py_DECREF(defaults);
        ffi::Py_DECREF(owner as *mut ffi::PyObject);
        raise_missing_function_default_obj(name_obj as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    let tuple_len = ffi::PyTuple_GET_SIZE(defaults);
    if index >= tuple_len as i64 {
        ffi::Py_DECREF(defaults);
        ffi::Py_DECREF(owner as *mut ffi::PyObject);
        raise_missing_function_default_obj(name_obj as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    let value = ffi::PyTuple_GetItem(defaults, index as ffi::Py_ssize_t);
    if value.is_null() {
        ffi::Py_DECREF(defaults);
        ffi::Py_DECREF(owner as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    ffi::Py_INCREF(value);
    ffi::Py_DECREF(defaults);
    ffi::Py_DECREF(owner as *mut ffi::PyObject);
    value as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn function_kwonly_default_obj_hook(
    callable: ObjPtr,
    name_obj: ObjPtr,
) -> ObjPtr {
    if name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_function_kwonly_default_obj\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let owner = resolve_function_defaults_owner(callable);
    if owner.is_null() {
        return ptr::null_mut();
    }
    let kwdefaults = ffi::PyObject_GetAttrString(
        owner as *mut ffi::PyObject,
        b"__kwdefaults__\0".as_ptr() as *const i8,
    );
    if kwdefaults.is_null() {
        ffi::Py_DECREF(owner as *mut ffi::PyObject);
        if ffi::PyErr_ExceptionMatches(ffi::PyExc_AttributeError) != 0 {
            ffi::PyErr_Clear();
            raise_missing_function_default_obj(name_obj as *mut ffi::PyObject);
        }
        return ptr::null_mut();
    }
    if kwdefaults == ffi::Py_None() || ffi::PyDict_Check(kwdefaults) == 0 {
        ffi::Py_DECREF(kwdefaults);
        ffi::Py_DECREF(owner as *mut ffi::PyObject);
        raise_missing_function_default_obj(name_obj as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    let value = ffi::PyObject_GetItem(kwdefaults, name_obj as *mut ffi::PyObject);
    if value.is_null() {
        if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) != 0 {
            ffi::PyErr_Clear();
            ffi::Py_DECREF(kwdefaults);
            ffi::Py_DECREF(owner as *mut ffi::PyObject);
            raise_missing_function_default_obj(name_obj as *mut ffi::PyObject);
            return ptr::null_mut();
        }
        ffi::Py_DECREF(kwdefaults);
        ffi::Py_DECREF(owner as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    ffi::Py_DECREF(kwdefaults);
    ffi::Py_DECREF(owner as *mut ffi::PyObject);
    value as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn function_closure_cell_hook(callable: ObjPtr, slot: i64) -> ObjPtr {
    unsafe fn closure_tuple_for_owner(owner: ObjPtr) -> Result<Option<*mut ffi::PyObject>, ()> {
        let closure = ffi::PyObject_GetAttrString(
            owner as *mut ffi::PyObject,
            b"__closure__\0".as_ptr() as *const i8,
        );
        if closure.is_null() {
            if ffi::PyErr_ExceptionMatches(ffi::PyExc_AttributeError) != 0 {
                ffi::PyErr_Clear();
                return Ok(None);
            }
            return Err(());
        }
        if closure == ffi::Py_None() {
            ffi::Py_DECREF(closure);
            return Ok(None);
        }
        if ffi::PyTuple_Check(closure) == 0 {
            ffi::Py_DECREF(closure);
            return Ok(None);
        }
        Ok(Some(closure))
    }

    if slot < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"dp_jit_function_closure_cell requires a non-negative slot\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let function = resolve_function_object(callable);
    if function.is_null() {
        return ptr::null_mut();
    }
    let (closure_owner, closure) = match closure_tuple_for_owner(function) {
        Ok(Some(closure)) => (function, closure),
        Ok(None) => {
            ffi::Py_DECREF(function as *mut ffi::PyObject);
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"callable has no closure cells\0".as_ptr() as *const i8,
            );
            return ptr::null_mut();
        }
        Err(()) => {
            ffi::Py_DECREF(function as *mut ffi::PyObject);
            return ptr::null_mut();
        }
    };
    let resolved_slot = slot as ffi::Py_ssize_t;
    let closure_len = ffi::PyTuple_GET_SIZE(closure);
    if resolved_slot < 0 || resolved_slot >= closure_len {
        ffi::Py_DECREF(closure);
        ffi::Py_DECREF(closure_owner as *mut ffi::PyObject);
        ffi::PyErr_SetString(
            ffi::PyExc_IndexError,
            b"closure slot out of range\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let cell = ffi::PyTuple_GetItem(closure, resolved_slot);
    if cell.is_null() {
        ffi::Py_DECREF(closure);
        ffi::Py_DECREF(closure_owner as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    if !is_cell_object(cell) {
        ffi::Py_DECREF(closure);
        ffi::Py_DECREF(closure_owner as *mut ffi::PyObject);
        raise_expected_cell("dp_jit_function_closure_cell", cell);
        return ptr::null_mut();
    }
    ffi::Py_INCREF(cell);
    ffi::Py_DECREF(closure);
    ffi::Py_DECREF(closure_owner as *mut ffi::PyObject);
    cell as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn pyobject_getattr_hook(obj: ObjPtr, attr: ObjPtr) -> ObjPtr {
    if obj.is_null() || attr.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_getattr\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    ffi::PyObject_GetAttr(obj as *mut ffi::PyObject, attr as *mut ffi::PyObject) as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn pyobject_setattr_hook(obj: ObjPtr, attr: ObjPtr, value: ObjPtr) -> ObjPtr {
    if obj.is_null() || attr.is_null() || value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_setattr\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let rc = ffi::PyObject_SetAttr(
        obj as *mut ffi::PyObject,
        attr as *mut ffi::PyObject,
        value as *mut ffi::PyObject,
    );
    if rc == 0 {
        let none = ffi::Py_None();
        ffi::Py_INCREF(none);
        none as ObjPtr
    } else {
        ptr::null_mut()
    }
}

#[cfg(not(test))]
unsafe extern "C" fn pyobject_getitem_hook(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    if obj.is_null() || key.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_getitem\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    ffi::PyObject_GetItem(obj as *mut ffi::PyObject, key as *mut ffi::PyObject) as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn pyobject_setitem_hook(obj: ObjPtr, key: ObjPtr, value: ObjPtr) -> ObjPtr {
    if obj.is_null() || key.is_null() || value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_setitem\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let rc = ffi::PyObject_SetItem(
        obj as *mut ffi::PyObject,
        key as *mut ffi::PyObject,
        value as *mut ffi::PyObject,
    );
    if rc == 0 {
        let none = ffi::Py_None();
        ffi::Py_INCREF(none);
        none as ObjPtr
    } else {
        ptr::null_mut()
    }
}

#[cfg(not(test))]
unsafe extern "C" fn pyobject_delitem_hook(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    if obj.is_null() || key.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_delitem\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let rc = ffi::PyObject_DelItem(obj as *mut ffi::PyObject, key as *mut ffi::PyObject);
    if rc == 0 {
        let none = ffi::Py_None();
        ffi::Py_INCREF(none);
        none as ObjPtr
    } else {
        ptr::null_mut()
    }
}

#[cfg(not(test))]
unsafe extern "C" fn store_global_hook(
    globals_obj: ObjPtr,
    name: ObjPtr,
    slot_index: i64,
    value: ObjPtr,
) -> ObjPtr {
    if globals_obj.is_null() || name.is_null() || value.is_null() || slot_index < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_store_global\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    if let Some(cache) = ModuleGlobalCache::lookup(globals_obj as *mut ffi::PyObject) {
        return cache.store_global_write_through(
            name as *mut ffi::PyObject,
            slot_index as u32,
            value as *mut ffi::PyObject,
        ) as ObjPtr;
    }
    let rc = ffi::PyObject_SetItem(
        globals_obj as *mut ffi::PyObject,
        name as *mut ffi::PyObject,
        value as *mut ffi::PyObject,
    );
    if rc == 0 {
        ffi::Py_INCREF(value as *mut ffi::PyObject);
        value
    } else {
        ptr::null_mut()
    }
}

#[cfg(not(test))]
unsafe extern "C" fn del_quietly_hook(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    if obj.is_null() || key.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_del_quietly\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let rc = ffi::PyObject_DelItem(obj as *mut ffi::PyObject, key as *mut ffi::PyObject);
    if rc != 0 {
        let suppress = ffi::PyErr_ExceptionMatches(ffi::PyExc_NameError) != 0
            || ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) != 0;
        if !suppress {
            return ptr::null_mut();
        }
        ffi::PyErr_Clear();
    }
    let none = ffi::Py_None();
    ffi::Py_INCREF(none);
    none as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn del_global_hook(
    globals_obj: ObjPtr,
    key: ObjPtr,
    slot_index: i64,
    quietly: bool,
) -> ObjPtr {
    if globals_obj.is_null() || key.is_null() || slot_index < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            if quietly {
                b"invalid arguments to dp_jit_del_global_quietly\0".as_ptr() as *const i8
            } else {
                b"invalid arguments to dp_jit_del_global\0".as_ptr() as *const i8
            },
        );
        return ptr::null_mut();
    }
    if let Some(cache) = ModuleGlobalCache::lookup(globals_obj as *mut ffi::PyObject) {
        return cache.del_global_write_through(key as *mut ffi::PyObject, slot_index as u32, quietly)
            as ObjPtr;
    }
    if quietly {
        return del_quietly_hook(globals_obj, key);
    }
    pyobject_delitem_hook(globals_obj, key)
}

#[cfg(not(test))]
unsafe extern "C" fn pyobject_to_i64_hook(value: ObjPtr) -> i64 {
    if value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid null value for dp_jit_pyobject_to_i64\0".as_ptr() as *const i8,
        );
        return i64::MIN;
    }
    let idx_obj = ffi::PyNumber_Index(value as *mut ffi::PyObject);
    if idx_obj.is_null() {
        return i64::MIN;
    }
    let out = ffi::PyLong_AsLongLong(idx_obj);
    ffi::Py_DECREF(idx_obj);
    if out == -1 && !ffi::PyErr_Occurred().is_null() {
        i64::MIN
    } else {
        out as i64
    }
}

#[cfg(not(test))]
unsafe extern "C" fn raise_deleted_name_error_hook(name_obj: ObjPtr) {
    if name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_raise_deleted_name_error\0".as_ptr() as *const i8,
        );
        return;
    }
    let repr = ffi::PyObject_Repr(name_obj as *mut ffi::PyObject);
    if !repr.is_null() {
        let repr_utf8 = ffi::PyUnicode_AsUTF8(repr);
        if !repr_utf8.is_null() {
            let repr_text = std::ffi::CStr::from_ptr(repr_utf8).to_string_lossy();
            let message = format!(
                "cannot access local variable {repr_text} where it is not associated with a value"
            );
            ffi::Py_DECREF(repr);
            if let Ok(c_msg) = std::ffi::CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_UnboundLocalError, c_msg.as_ptr());
                return;
            }
        } else {
            ffi::PyErr_Clear();
        }
        ffi::Py_DECREF(repr);
    }
    ffi::PyErr_SetString(
        ffi::PyExc_UnboundLocalError,
        b"cannot access local variable before assignment\0".as_ptr() as *const i8,
    );
}

#[cfg(not(test))]
unsafe extern "C" fn make_cell_hook(value: ObjPtr) -> ObjPtr {
    PyCell_New(value as *mut ffi::PyObject) as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn load_cell_hook(cell: ObjPtr) -> ObjPtr {
    if !is_cell_object(cell as *mut ffi::PyObject) {
        raise_expected_cell("dp_jit_load_cell", cell as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    let value = PyCell_Get(cell as *mut ffi::PyObject);
    if value.is_null() && ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) != 0 {
        ffi::PyErr_Clear();
        ffi::PyErr_SetString(
            ffi::PyExc_UnboundLocalError,
            b"local variable referenced before assignment\0".as_ptr() as *const i8,
        );
    }
    value as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn store_cell_hook(cell: ObjPtr, value: ObjPtr) -> ObjPtr {
    if !is_cell_object(cell as *mut ffi::PyObject) {
        raise_expected_cell("dp_jit_store_cell", cell as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    if PyCell_Set(cell as *mut ffi::PyObject, value as *mut ffi::PyObject) < 0 {
        return ptr::null_mut();
    }
    ffi::Py_INCREF(value as *mut ffi::PyObject);
    value
}

#[cfg(not(test))]
unsafe extern "C" fn del_deref_hook(cell: ObjPtr) -> ObjPtr {
    if !is_cell_object(cell as *mut ffi::PyObject) {
        raise_expected_cell("dp_jit_del_deref", cell as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    let rc = ffi::PyObject_DelAttrString(
        cell as *mut ffi::PyObject,
        b"cell_contents\0".as_ptr() as *const i8,
    );
    if rc == 0 {
        let none = ffi::Py_None();
        ffi::Py_INCREF(none);
        return none as ObjPtr;
    }
    if ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) != 0 {
        ffi::PyErr_Clear();
        ffi::PyErr_SetString(
            ffi::PyExc_UnboundLocalError,
            b"local variable referenced before assignment\0".as_ptr() as *const i8,
        );
    }
    ptr::null_mut()
}

#[cfg(not(test))]
unsafe extern "C" fn del_deref_quietly_hook(cell: ObjPtr) -> ObjPtr {
    if !is_cell_object(cell as *mut ffi::PyObject) {
        raise_expected_cell("dp_jit_del_deref_quietly", cell as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    let rc = ffi::PyObject_DelAttrString(
        cell as *mut ffi::PyObject,
        b"cell_contents\0".as_ptr() as *const i8,
    );
    if rc != 0 {
        if ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) == 0 {
            return ptr::null_mut();
        }
        ffi::PyErr_Clear();
    }
    let none = ffi::Py_None();
    ffi::Py_INCREF(none);
    none as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn load_global_obj_hook(
    globals_obj: ObjPtr,
    global_slots_obj: ObjPtr,
    name: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    load_global_obj_impl(
        globals_obj,
        global_slots_obj,
        name as *mut ffi::PyObject,
        slot_index,
    )
}

#[cfg(not(test))]
unsafe extern "C" fn tuple_new_hook(size: i64) -> ObjPtr {
    if size < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid tuple size in JIT\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    ffi::PyTuple_New(size as ffi::Py_ssize_t) as ObjPtr
}

#[cfg(not(test))]
unsafe extern "C" fn tuple_set_item_hook(tuple_obj: ObjPtr, index: i64, value: ObjPtr) -> i32 {
    if tuple_obj.is_null() || value.is_null() || index < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid tuple_set_item arguments in JIT\0".as_ptr() as *const i8,
        );
        return -1;
    }
    ffi::PyTuple_SetItem(
        tuple_obj as *mut ffi::PyObject,
        index as ffi::Py_ssize_t,
        value as *mut ffi::PyObject,
    )
}

#[cfg(not(test))]
unsafe extern "C" fn is_true_hook(value: ObjPtr) -> i32 {
    if value.is_null() {
        return -1;
    }
    ffi::PyObject_IsTrue(value as *mut ffi::PyObject)
}

#[cfg(not(test))]
unsafe extern "C" fn raise_from_exc_hook(exc: ObjPtr) -> i32 {
    if exc.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"missing exception for dp_jit_raise_from_exc\0".as_ptr() as *const i8,
        );
        return -1;
    }
    let exc_obj = exc as *mut ffi::PyObject;
    ffi::Py_INCREF(exc_obj);
    PyErr_SetRaisedException(exc_obj);
    0
}

#[cfg(test)]
mod test_only_export_stubs {
    use super::*;

    macro_rules! panic_obj_export {
        ($name:ident($($arg:ident : $ty:ty),* $(,)?)) => {
            pub unsafe extern "C" fn $name($($arg: $ty),*) -> ObjPtr {
                $(let _ = $arg;)*
                panic!(concat!(stringify!($name), " should not run in tests"));
            }
        };
    }

    macro_rules! panic_i32_export {
        ($name:ident($($arg:ident : $ty:ty),* $(,)?)) => {
            pub unsafe extern "C" fn $name($($arg: $ty),*) -> i32 {
                $(let _ = $arg;)*
                panic!(concat!(stringify!($name), " should not run in tests"));
            }
        };
    }

    macro_rules! panic_i64_export {
        ($name:ident($($arg:ident : $ty:ty),* $(,)?)) => {
            pub unsafe extern "C" fn $name($($arg: $ty),*) -> i64 {
                $(let _ = $arg;)*
                panic!(concat!(stringify!($name), " should not run in tests"));
            }
        };
    }

    macro_rules! panic_unit_export {
        ($name:ident($($arg:ident : $ty:ty),* $(,)?)) => {
            pub unsafe extern "C" fn $name($($arg: $ty),*) {
                $(let _ = $arg;)*
                panic!(concat!(stringify!($name), " should not run in tests"));
            }
        };
    }

    panic_i32_export!(dp_jit_raise_from_exc(exc: ObjPtr));
    panic_obj_export!(dp_jit_py_call_positional_three(
        callable: ObjPtr,
        arg1: ObjPtr,
        arg2: ObjPtr,
        arg3: ObjPtr,
        sentinel: ObjPtr,
    ));
    panic_obj_export!(dp_jit_py_call_object(callable: ObjPtr, args: ObjPtr));
    panic_obj_export!(dp_jit_py_vectorcall(
        callable: ObjPtr,
        args: ObjPtr,
        nargsf: ObjPtr,
        kwnames: ObjPtr
    ));
    panic_obj_export!(dp_jit_py_call_with_kw(callable: ObjPtr, args: ObjPtr, kw: ObjPtr));
    panic_i64_export!(dp_jit_callee_function_id(callable: ObjPtr));
    panic_i32_export!(dp_jit_guard_method_type_version(
        receiver: ObjPtr,
        expected_type: ObjPtr,
        expected_version: i64
    ));
    panic_unit_export!(dp_jit_record_counter_value(vmctx: ObjPtr, counter_id: i64, value: i64));
    panic_obj_export!(dp_jit_get_raised_exception());
    panic_obj_export!(dp_jit_get_arg_item(args: ObjPtr, index: i64));
    panic_obj_export!(dp_jit_load_runtime_obj(name: ObjPtr));
    panic_obj_export!(dp_jit_function_closure_cell(callable: ObjPtr, slot: i64));
    panic_obj_export!(dp_jit_function_positional_default_obj(
        callable: ObjPtr,
        name: ObjPtr,
        index: i64,
    ));
    panic_obj_export!(dp_jit_function_kwonly_default_obj(callable: ObjPtr, name: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_getattr(obj: ObjPtr, attr: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_setattr(obj: ObjPtr, attr: ObjPtr, value: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_getitem(obj: ObjPtr, key: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_setitem(obj: ObjPtr, key: ObjPtr, value: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_delitem(obj: ObjPtr, key: ObjPtr));
    panic_obj_export!(dp_jit_load_global_obj(
        globals_obj: ObjPtr,
        global_slots_obj: ObjPtr,
        name: ObjPtr,
        slot_index: i64
    ));
    panic_obj_export!(dp_jit_store_global(
        globals_obj: ObjPtr,
        name: ObjPtr,
        slot_index: i64,
        value: ObjPtr
    ));
    panic_obj_export!(dp_jit_del_global(globals_obj: ObjPtr, key: ObjPtr, slot_index: i64));
    panic_obj_export!(dp_jit_del_global_quietly(
        globals_obj: ObjPtr,
        key: ObjPtr,
        slot_index: i64
    ));
    panic_obj_export!(dp_jit_del_quietly(obj: ObjPtr, key: ObjPtr));
    panic_i64_export!(dp_jit_pyobject_to_i64(value: ObjPtr));
    panic_obj_export!(dp_jit_make_cell(value: ObjPtr));
    panic_unit_export!(dp_jit_raise_deleted_name_error(name: ObjPtr));
    panic_obj_export!(dp_jit_load_cell(cell: ObjPtr));
    panic_obj_export!(dp_jit_store_cell(cell: ObjPtr, value: ObjPtr));
    panic_obj_export!(dp_jit_del_deref(cell: ObjPtr));
    panic_obj_export!(dp_jit_del_deref_quietly(cell: ObjPtr));
    panic_obj_export!(dp_jit_tuple_new(size: i64));
    panic_i32_export!(dp_jit_tuple_set_item(tuple_obj: ObjPtr, index: i64, item: ObjPtr));
    panic_i32_export!(dp_jit_is_true(value: ObjPtr));
}

#[cfg(test)]
pub use test_only_export_stubs::*;

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_raise_from_exc(exc: ObjPtr) -> i32 {
    raise_from_exc_hook(exc)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_py_call_positional_three(
    callable: ObjPtr,
    arg1: ObjPtr,
    arg2: ObjPtr,
    arg3: ObjPtr,
    _sentinel: ObjPtr,
) -> ObjPtr {
    py_call_positional_three_hook(callable, arg1, arg2, arg3)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_py_call_object(callable: ObjPtr, args: ObjPtr) -> ObjPtr {
    py_call_object_hook(callable, args)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_py_vectorcall(
    callable: ObjPtr,
    args: ObjPtr,
    nargsf: ObjPtr,
    kwnames: ObjPtr,
) -> ObjPtr {
    py_vectorcall_hook(callable, args, nargsf, kwnames)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_py_call_with_kw(
    callable: ObjPtr,
    args: ObjPtr,
    kw: ObjPtr,
) -> ObjPtr {
    py_call_with_kw_hook(callable, args, kw)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_callee_function_id(callable: ObjPtr) -> i64 {
    callee_function_id_hook(callable)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_guard_method_type_version(
    receiver: ObjPtr,
    expected_type: ObjPtr,
    expected_version: i64,
) -> i32 {
    guard_method_type_version_hook(receiver, expected_type, expected_version)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_record_counter_value(vmctx: ObjPtr, counter_id: i64, value: i64) {
    record_counter_value_hook(vmctx, counter_id, value)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_get_raised_exception() -> ObjPtr {
    py_get_raised_exception_hook()
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_get_arg_item(args: ObjPtr, index: i64) -> ObjPtr {
    get_arg_item_hook(args, index)
}

#[cfg(not(test))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_load_runtime_obj(name: ObjPtr) -> ObjPtr {
    load_runtime_name_owned(name as *mut ffi::PyObject) as ObjPtr
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_function_closure_cell(callable: ObjPtr, slot: i64) -> ObjPtr {
    function_closure_cell_hook(callable, slot)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_function_positional_default_obj(
    callable: ObjPtr,
    name: ObjPtr,
    index: i64,
) -> ObjPtr {
    function_positional_default_obj_hook(callable, name, index)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_function_kwonly_default_obj(
    callable: ObjPtr,
    name: ObjPtr,
) -> ObjPtr {
    function_kwonly_default_obj_hook(callable, name)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_pyobject_getattr(obj: ObjPtr, attr: ObjPtr) -> ObjPtr {
    pyobject_getattr_hook(obj, attr)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_pyobject_setattr(
    obj: ObjPtr,
    attr: ObjPtr,
    value: ObjPtr,
) -> ObjPtr {
    pyobject_setattr_hook(obj, attr, value)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_pyobject_getitem(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    pyobject_getitem_hook(obj, key)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_pyobject_setitem(
    obj: ObjPtr,
    key: ObjPtr,
    value: ObjPtr,
) -> ObjPtr {
    pyobject_setitem_hook(obj, key, value)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_pyobject_delitem(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    pyobject_delitem_hook(obj, key)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_load_global_obj(
    globals_obj: ObjPtr,
    global_slots_obj: ObjPtr,
    name: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    load_global_obj_hook(globals_obj, global_slots_obj, name, slot_index)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_store_global(
    globals_obj: ObjPtr,
    name: ObjPtr,
    slot_index: i64,
    value: ObjPtr,
) -> ObjPtr {
    store_global_hook(globals_obj, name, slot_index, value)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_del_global(
    globals_obj: ObjPtr,
    key: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    del_global_hook(globals_obj, key, slot_index, false)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_del_global_quietly(
    globals_obj: ObjPtr,
    key: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    del_global_hook(globals_obj, key, slot_index, true)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_del_quietly(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    del_quietly_hook(obj, key)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_pyobject_to_i64(value: ObjPtr) -> i64 {
    pyobject_to_i64_hook(value)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_make_cell(value: ObjPtr) -> ObjPtr {
    make_cell_hook(value)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_raise_deleted_name_error(name: ObjPtr) {
    raise_deleted_name_error_hook(name)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_load_cell(cell: ObjPtr) -> ObjPtr {
    load_cell_hook(cell)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_store_cell(cell: ObjPtr, value: ObjPtr) -> ObjPtr {
    store_cell_hook(cell, value)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_del_deref(cell: ObjPtr) -> ObjPtr {
    del_deref_hook(cell)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_del_deref_quietly(cell: ObjPtr) -> ObjPtr {
    del_deref_quietly_hook(cell)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_tuple_new(size: i64) -> ObjPtr {
    tuple_new_hook(size)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_tuple_set_item(tuple_obj: ObjPtr, index: i64, item: ObjPtr) -> i32 {
    tuple_set_item_hook(tuple_obj, index, item)
}

#[cfg(not(test))]
pub unsafe extern "C" fn dp_jit_is_true(value: ObjPtr) -> i32 {
    is_true_hook(value)
}

unsafe extern "C" fn pyobject_richcompare_wrapper(lhs: ObjPtr, rhs: ObjPtr, op: i32) -> ObjPtr {
    if lhs.is_null() || rhs.is_null() {
        return ptr::null_mut();
    }
    type Func =
        unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject, i32) -> *mut ffi::PyObject;
    static SYMBOL: OnceLock<usize> = OnceLock::new();
    let symbol =
        *SYMBOL.get_or_init(|| unsafe { load_python_capi_symbol(b"PyObject_RichCompare\0") });
    if symbol == 0 {
        return ptr::null_mut();
    }
    let func: Func = unsafe { std::mem::transmute(symbol) };
    func(lhs as *mut ffi::PyObject, rhs as *mut ffi::PyObject, op) as ObjPtr
}

unsafe fn load_python_capi_symbol(name: &'static [u8]) -> usize {
    libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr() as *const i8) as usize
}

macro_rules! define_unary_obj_wrapper {
    ($fn_name:ident, $symbol:literal) => {
        unsafe extern "C" fn $fn_name(value: ObjPtr) -> ObjPtr {
            if value.is_null() {
                return ptr::null_mut();
            }
            type Func = unsafe extern "C" fn(*mut ffi::PyObject) -> *mut ffi::PyObject;
            static SYMBOL: OnceLock<usize> = OnceLock::new();
            let symbol = *SYMBOL.get_or_init(|| unsafe {
                load_python_capi_symbol(concat!($symbol, "\0").as_bytes())
            });
            if symbol == 0 {
                return ptr::null_mut();
            }
            let func: Func = unsafe { std::mem::transmute(symbol) };
            func(value as *mut ffi::PyObject) as ObjPtr
        }
    };
}

macro_rules! define_unary_i32_wrapper {
    ($fn_name:ident, $symbol:literal) => {
        unsafe extern "C" fn $fn_name(value: ObjPtr) -> i32 {
            if value.is_null() {
                return -1;
            }
            type Func = unsafe extern "C" fn(*mut ffi::PyObject) -> i32;
            static SYMBOL: OnceLock<usize> = OnceLock::new();
            let symbol = *SYMBOL.get_or_init(|| unsafe {
                load_python_capi_symbol(concat!($symbol, "\0").as_bytes())
            });
            if symbol == 0 {
                return -1;
            }
            let func: Func = unsafe { std::mem::transmute(symbol) };
            func(value as *mut ffi::PyObject)
        }
    };
}

macro_rules! define_binary_obj_wrapper {
    ($fn_name:ident, $symbol:literal) => {
        unsafe extern "C" fn $fn_name(lhs: ObjPtr, rhs: ObjPtr) -> ObjPtr {
            if lhs.is_null() || rhs.is_null() {
                return ptr::null_mut();
            }
            type Func =
                unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject) -> *mut ffi::PyObject;
            static SYMBOL: OnceLock<usize> = OnceLock::new();
            let symbol = *SYMBOL.get_or_init(|| unsafe {
                load_python_capi_symbol(concat!($symbol, "\0").as_bytes())
            });
            if symbol == 0 {
                return ptr::null_mut();
            }
            let func: Func = unsafe { std::mem::transmute(symbol) };
            func(lhs as *mut ffi::PyObject, rhs as *mut ffi::PyObject) as ObjPtr
        }
    };
}

macro_rules! define_binary_i32_wrapper {
    ($fn_name:ident, $symbol:literal) => {
        unsafe extern "C" fn $fn_name(lhs: ObjPtr, rhs: ObjPtr) -> i32 {
            if lhs.is_null() || rhs.is_null() {
                return -1;
            }
            type Func = unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject) -> i32;
            static SYMBOL: OnceLock<usize> = OnceLock::new();
            let symbol = *SYMBOL.get_or_init(|| unsafe {
                load_python_capi_symbol(concat!($symbol, "\0").as_bytes())
            });
            if symbol == 0 {
                return -1;
            }
            let func: Func = unsafe { std::mem::transmute(symbol) };
            func(lhs as *mut ffi::PyObject, rhs as *mut ffi::PyObject)
        }
    };
}

macro_rules! define_ternary_obj_wrapper {
    ($fn_name:ident, $symbol:literal) => {
        unsafe extern "C" fn $fn_name(lhs: ObjPtr, rhs: ObjPtr, third: ObjPtr) -> ObjPtr {
            if lhs.is_null() || rhs.is_null() || third.is_null() {
                return ptr::null_mut();
            }
            type Func = unsafe extern "C" fn(
                *mut ffi::PyObject,
                *mut ffi::PyObject,
                *mut ffi::PyObject,
            ) -> *mut ffi::PyObject;
            static SYMBOL: OnceLock<usize> = OnceLock::new();
            let symbol = *SYMBOL.get_or_init(|| unsafe {
                load_python_capi_symbol(concat!($symbol, "\0").as_bytes())
            });
            if symbol == 0 {
                return ptr::null_mut();
            }
            let func: Func = unsafe { std::mem::transmute(symbol) };
            func(
                lhs as *mut ffi::PyObject,
                rhs as *mut ffi::PyObject,
                third as *mut ffi::PyObject,
            ) as ObjPtr
        }
    };
}

define_binary_i32_wrapper!(pysequence_contains_wrapper, "PySequence_Contains");
define_unary_i32_wrapper!(pyobject_not_wrapper, "PyObject_Not");
define_unary_i32_wrapper!(pyobject_is_true_wrapper, "PyObject_IsTrue");
define_binary_obj_wrapper!(pynumber_add_wrapper, "PyNumber_Add");
define_binary_obj_wrapper!(pynumber_subtract_wrapper, "PyNumber_Subtract");
define_binary_obj_wrapper!(pynumber_multiply_wrapper, "PyNumber_Multiply");
define_binary_obj_wrapper!(pynumber_matrix_multiply_wrapper, "PyNumber_MatrixMultiply");
define_binary_obj_wrapper!(pynumber_true_divide_wrapper, "PyNumber_TrueDivide");
define_binary_obj_wrapper!(pynumber_floor_divide_wrapper, "PyNumber_FloorDivide");
define_binary_obj_wrapper!(pynumber_remainder_wrapper, "PyNumber_Remainder");
define_ternary_obj_wrapper!(pynumber_power_wrapper, "PyNumber_Power");
define_binary_obj_wrapper!(pynumber_lshift_wrapper, "PyNumber_Lshift");
define_binary_obj_wrapper!(pynumber_rshift_wrapper, "PyNumber_Rshift");
define_binary_obj_wrapper!(pynumber_or_wrapper, "PyNumber_Or");
define_binary_obj_wrapper!(pynumber_xor_wrapper, "PyNumber_Xor");
define_binary_obj_wrapper!(pynumber_and_wrapper, "PyNumber_And");
define_binary_obj_wrapper!(pynumber_inplace_add_wrapper, "PyNumber_InPlaceAdd");
define_binary_obj_wrapper!(
    pynumber_inplace_subtract_wrapper,
    "PyNumber_InPlaceSubtract"
);
define_binary_obj_wrapper!(
    pynumber_inplace_multiply_wrapper,
    "PyNumber_InPlaceMultiply"
);
define_binary_obj_wrapper!(
    pynumber_inplace_matrix_multiply_wrapper,
    "PyNumber_InPlaceMatrixMultiply"
);
define_binary_obj_wrapper!(
    pynumber_inplace_true_divide_wrapper,
    "PyNumber_InPlaceTrueDivide"
);
define_binary_obj_wrapper!(
    pynumber_inplace_floor_divide_wrapper,
    "PyNumber_InPlaceFloorDivide"
);
define_binary_obj_wrapper!(
    pynumber_inplace_remainder_wrapper,
    "PyNumber_InPlaceRemainder"
);
define_ternary_obj_wrapper!(pynumber_inplace_power_wrapper, "PyNumber_InPlacePower");
define_binary_obj_wrapper!(pynumber_inplace_lshift_wrapper, "PyNumber_InPlaceLshift");
define_binary_obj_wrapper!(pynumber_inplace_rshift_wrapper, "PyNumber_InPlaceRshift");
define_binary_obj_wrapper!(pynumber_inplace_or_wrapper, "PyNumber_InPlaceOr");
define_binary_obj_wrapper!(pynumber_inplace_xor_wrapper, "PyNumber_InPlaceXor");
define_binary_obj_wrapper!(pynumber_inplace_and_wrapper, "PyNumber_InPlaceAnd");
define_unary_obj_wrapper!(pynumber_positive_wrapper, "PyNumber_Positive");
define_unary_obj_wrapper!(pynumber_negative_wrapper, "PyNumber_Negative");
define_unary_obj_wrapper!(pynumber_invert_wrapper, "PyNumber_Invert");

pub fn register_specialized_jit_symbols(builder: &mut JITBuilder) {
    builder.symbol(
        "dp_jit_py_call_positional_three",
        dp_jit_py_call_positional_three as *const u8,
    );
    builder.symbol("dp_jit_py_call_object", dp_jit_py_call_object as *const u8);
    builder.symbol("dp_jit_py_vectorcall", dp_jit_py_vectorcall as *const u8);
    builder.symbol(
        "dp_jit_py_call_with_kw",
        dp_jit_py_call_with_kw as *const u8,
    );
    builder.symbol(
        "dp_jit_get_raised_exception",
        dp_jit_get_raised_exception as *const u8,
    );
    builder.symbol("dp_jit_get_arg_item", dp_jit_get_arg_item as *const u8);
    builder.symbol(
        "dp_jit_load_runtime_obj",
        dp_jit_load_runtime_obj as *const u8,
    );
    builder.symbol(
        "dp_jit_function_closure_cell",
        dp_jit_function_closure_cell as *const u8,
    );
    builder.symbol(
        "dp_jit_function_positional_default_obj",
        dp_jit_function_positional_default_obj as *const u8,
    );
    builder.symbol(
        "dp_jit_function_kwonly_default_obj",
        dp_jit_function_kwonly_default_obj as *const u8,
    );
    builder.symbol(
        "dp_jit_pyobject_getattr",
        dp_jit_pyobject_getattr as *const u8,
    );
    builder.symbol(
        "dp_jit_pyobject_setattr",
        dp_jit_pyobject_setattr as *const u8,
    );
    builder.symbol(
        "dp_jit_pyobject_getitem",
        dp_jit_pyobject_getitem as *const u8,
    );
    builder.symbol(
        "dp_jit_pyobject_setitem",
        dp_jit_pyobject_setitem as *const u8,
    );
    builder.symbol(
        "dp_jit_pyobject_delitem",
        dp_jit_pyobject_delitem as *const u8,
    );
    builder.symbol(
        "dp_jit_load_global_obj",
        dp_jit_load_global_obj as *const u8,
    );
    builder.symbol("dp_jit_store_global", dp_jit_store_global as *const u8);
    builder.symbol("dp_jit_del_global", dp_jit_del_global as *const u8);
    builder.symbol(
        "dp_jit_del_global_quietly",
        dp_jit_del_global_quietly as *const u8,
    );
    builder.symbol("dp_jit_del_quietly", dp_jit_del_quietly as *const u8);
    builder.symbol(
        "dp_jit_pyobject_to_i64",
        dp_jit_pyobject_to_i64 as *const u8,
    );
    builder.symbol(
        "dp_jit_callee_function_id",
        dp_jit_callee_function_id as *const u8,
    );
    builder.symbol(
        "dp_jit_guard_method_type_version",
        dp_jit_guard_method_type_version as *const u8,
    );
    builder.symbol(
        "dp_jit_record_counter_value",
        dp_jit_record_counter_value as *const u8,
    );
    builder.symbol(
        "dp_jit_raise_deleted_name_error",
        dp_jit_raise_deleted_name_error as *const u8,
    );
    builder.symbol("dp_jit_make_cell", dp_jit_make_cell as *const u8);
    builder.symbol("dp_jit_load_cell", dp_jit_load_cell as *const u8);
    builder.symbol("dp_jit_store_cell", dp_jit_store_cell as *const u8);
    builder.symbol("dp_jit_del_deref", dp_jit_del_deref as *const u8);
    builder.symbol(
        "dp_jit_del_deref_quietly",
        dp_jit_del_deref_quietly as *const u8,
    );
    builder.symbol("dp_jit_tuple_new", dp_jit_tuple_new as *const u8);
    builder.symbol("dp_jit_tuple_set_item", dp_jit_tuple_set_item as *const u8);
    builder.symbol("dp_jit_is_true", dp_jit_is_true as *const u8);
    builder.symbol("dp_jit_raise_from_exc", dp_jit_raise_from_exc as *const u8);
    builder.symbol(
        "PyObject_RichCompare",
        pyobject_richcompare_wrapper as *const u8,
    );
    builder.symbol(
        "PySequence_Contains",
        pysequence_contains_wrapper as *const u8,
    );
    builder.symbol("PyObject_Not", pyobject_not_wrapper as *const u8);
    builder.symbol("PyObject_IsTrue", pyobject_is_true_wrapper as *const u8);
    builder.symbol("PyNumber_Add", pynumber_add_wrapper as *const u8);
    builder.symbol("PyNumber_Subtract", pynumber_subtract_wrapper as *const u8);
    builder.symbol("PyNumber_Multiply", pynumber_multiply_wrapper as *const u8);
    builder.symbol(
        "PyNumber_MatrixMultiply",
        pynumber_matrix_multiply_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_TrueDivide",
        pynumber_true_divide_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_FloorDivide",
        pynumber_floor_divide_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_Remainder",
        pynumber_remainder_wrapper as *const u8,
    );
    builder.symbol("PyNumber_Power", pynumber_power_wrapper as *const u8);
    builder.symbol("PyNumber_Lshift", pynumber_lshift_wrapper as *const u8);
    builder.symbol("PyNumber_Rshift", pynumber_rshift_wrapper as *const u8);
    builder.symbol("PyNumber_Or", pynumber_or_wrapper as *const u8);
    builder.symbol("PyNumber_Xor", pynumber_xor_wrapper as *const u8);
    builder.symbol("PyNumber_And", pynumber_and_wrapper as *const u8);
    builder.symbol(
        "PyNumber_InPlaceAdd",
        pynumber_inplace_add_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceSubtract",
        pynumber_inplace_subtract_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceMultiply",
        pynumber_inplace_multiply_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceMatrixMultiply",
        pynumber_inplace_matrix_multiply_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceTrueDivide",
        pynumber_inplace_true_divide_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceFloorDivide",
        pynumber_inplace_floor_divide_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceRemainder",
        pynumber_inplace_remainder_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlacePower",
        pynumber_inplace_power_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceLshift",
        pynumber_inplace_lshift_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceRshift",
        pynumber_inplace_rshift_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceOr",
        pynumber_inplace_or_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceXor",
        pynumber_inplace_xor_wrapper as *const u8,
    );
    builder.symbol(
        "PyNumber_InPlaceAnd",
        pynumber_inplace_and_wrapper as *const u8,
    );
    builder.symbol("PyNumber_Positive", pynumber_positive_wrapper as *const u8);
    builder.symbol("PyNumber_Negative", pynumber_negative_wrapper as *const u8);
    builder.symbol("PyNumber_Invert", pynumber_invert_wrapper as *const u8);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_globals::ModuleGlobalCache;
    use pyo3::Python;
    use pyo3::types::{PyDict, PyString};
    use std::path::Path;
    use std::sync::atomic::{AtomicPtr, Ordering};

    fn repo_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace crate should have a repo-root parent")
    }

    fn vendored_python_home() -> std::path::PathBuf {
        repo_root().join("vendor").join("cpython")
    }

    fn vendored_python_build_lib_dir() -> std::path::PathBuf {
        let python_home = vendored_python_home();
        let rel_build_dir = std::fs::read_to_string(python_home.join("pybuilddir.txt"))
            .expect("vendored CPython pybuilddir.txt should exist");
        python_home.join(rel_build_dir.trim())
    }

    fn initialize_test_python() {
        let python_home = vendored_python_home();
        unsafe {
            std::env::set_var("PYTHONHOME", &python_home);
        }
        let python_path =
            std::env::join_paths([python_home.join("Lib"), vendored_python_build_lib_dir()])
                .expect("test PYTHONPATH should join");
        unsafe {
            std::env::set_var("PYTHONPATH", python_path);
        }
        Python::initialize();
    }

    unsafe fn cached_slot_value(cache: &ModuleGlobalCache, slot: u32) -> ObjPtr {
        let entry = (cache.slots_ptr() as *mut AtomicPtr<ffi::PyObject>).add(slot as usize);
        (*entry).load(Ordering::Acquire) as ObjPtr
    }

    #[test]
    fn builtin_global_miss_fills_slot() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let globals = PyDict::new(py);
            let cache = ModuleGlobalCache::new(globals.as_ptr(), &["list".into()])
                .expect("module global cache should initialize for builtin caching test");
            let name = PyString::new(py, "list");
            let result = load_global_obj_impl(
                globals.as_ptr() as ObjPtr,
                cache.slots_ptr() as ObjPtr,
                name.as_ptr(),
                0,
            );
            assert!(!result.is_null(), "builtin lookup should succeed");
            ffi::Py_DECREF(result.cast());
            assert!(
                !cached_slot_value(&cache, 0).is_null(),
                "builtin fallback should populate the global slot cache"
            );
        });
    }

    #[test]
    fn builtin_watcher_invalidates_unshadowed_cached_slot() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let globals = PyDict::new(py);
            let cache = ModuleGlobalCache::new(globals.as_ptr(), &["list".into()])
                .expect("module global cache should initialize for builtin caching test");
            let name = PyString::new(py, "list");
            let result = load_global_obj_impl(
                globals.as_ptr() as ObjPtr,
                cache.slots_ptr() as ObjPtr,
                name.as_ptr(),
                0,
            );
            assert!(!result.is_null(), "builtin lookup should succeed");
            ffi::Py_DECREF(result.cast());
            assert!(
                !cached_slot_value(&cache, 0).is_null(),
                "builtin fallback should populate the cache slot"
            );

            let builtins = ffi::PyEval_GetBuiltins();
            assert!(!builtins.is_null(), "builtins dict should exist");
            let original = ffi::PyObject_GetItem(builtins, name.as_ptr());
            assert!(!original.is_null(), "builtins.list should exist");
            let replacement = ffi::PyUnicode_FromString(b"shadowed list\0".as_ptr() as *const i8);
            assert!(
                !replacement.is_null(),
                "replacement builtin should allocate"
            );
            assert_eq!(
                ffi::PyObject_SetItem(builtins, name.as_ptr(), replacement),
                0
            );
            assert!(
                cached_slot_value(&cache, 0).is_null(),
                "builtins mutation should invalidate unshadowed cached slots"
            );
            assert_eq!(ffi::PyObject_SetItem(builtins, name.as_ptr(), original), 0);
            ffi::Py_DECREF(original);
            ffi::Py_DECREF(replacement);
        });
    }

    #[test]
    fn builtin_watcher_preserves_shadowed_cached_slot() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let globals = PyDict::new(py);
            let cache = ModuleGlobalCache::new(globals.as_ptr(), &["list".into()])
                .expect("module global cache should initialize for builtin caching test");
            let name = PyString::new(py, "list");
            let module_value = ffi::PyLong_FromLongLong(123);
            assert!(
                !module_value.is_null(),
                "module shadow value should allocate"
            );
            assert_eq!(
                ffi::PyObject_SetItem(globals.as_ptr(), name.as_ptr(), module_value),
                0
            );
            let result = load_global_obj_impl(
                globals.as_ptr() as ObjPtr,
                cache.slots_ptr() as ObjPtr,
                name.as_ptr(),
                0,
            );
            assert!(!result.is_null(), "module global lookup should succeed");
            ffi::Py_DECREF(result.cast());
            assert_eq!(
                cached_slot_value(&cache, 0),
                module_value as ObjPtr,
                "module shadow value should be cached"
            );

            let builtins = ffi::PyEval_GetBuiltins();
            assert!(!builtins.is_null(), "builtins dict should exist");
            let original = ffi::PyObject_GetItem(builtins, name.as_ptr());
            assert!(!original.is_null(), "builtins.list should exist");
            let replacement = ffi::PyUnicode_FromString(b"shadowed list\0".as_ptr() as *const i8);
            assert!(
                !replacement.is_null(),
                "replacement builtin should allocate"
            );
            assert_eq!(
                ffi::PyObject_SetItem(builtins, name.as_ptr(), replacement),
                0
            );
            assert_eq!(
                cached_slot_value(&cache, 0),
                module_value as ObjPtr,
                "builtins mutation should not invalidate a module-shadowed cached slot"
            );
            assert_eq!(ffi::PyObject_SetItem(builtins, name.as_ptr(), original), 0);

            ffi::Py_DECREF(original);
            ffi::Py_DECREF(replacement);
            ffi::Py_DECREF(module_value);
        });
    }
}
