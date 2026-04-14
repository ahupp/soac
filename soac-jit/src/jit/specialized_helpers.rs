#![cfg_attr(test, allow(dead_code, unused_imports))]

use super::RuntimeJitDeoptInvocation;
use crate::module_constants::load_runtime_name_owned;
use crate::module_constants::raise_name_error_for_missing_name;
use crate::operator_specialization::{ExactIntBinaryOpKind, ExactIntUnaryOpKind};
use cranelift_jit::JITBuilder;
use libc;
use pyo3::ffi;
use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::OnceLock;

unsafe extern "C" {
    static mut PyFunction_Type: ffi::PyTypeObject;
    static mut PyMethod_Type: ffi::PyTypeObject;
    fn _PyDict_GetIndexedItem(
        dict: *mut ffi::PyObject,
        index: ffi::Py_ssize_t,
        result: *mut *mut ffi::PyObject,
    ) -> libc::c_int;
    fn _PyDict_SetIndexedItem(
        dict: *mut ffi::PyObject,
        index: ffi::Py_ssize_t,
        value: *mut ffi::PyObject,
    ) -> libc::c_int;
    fn _PyDict_IndexedKeyIndex(
        dict: *mut ffi::PyObject,
        key: *mut ffi::PyObject,
    ) -> ffi::Py_ssize_t;
    fn PyType_GenericAlloc(
        type_obj: *mut ffi::PyTypeObject,
        nitems: ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject;
    fn PyDict_GetItemRef(
        dict: *mut ffi::PyObject,
        key: *mut ffi::PyObject,
        result: *mut *mut ffi::PyObject,
    ) -> libc::c_int;
    fn PyIter_NextItem(iterator: *mut ffi::PyObject, item: *mut *mut ffi::PyObject) -> libc::c_int;
}
unsafe extern "C" {
    static mut PyLong_Type: ffi::PyTypeObject;
}
unsafe extern "C" {
    static mut PyCell_Type: ffi::PyTypeObject;
    fn PyCell_New(obj: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyCell_Get(cell: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyCell_Set(cell: *mut ffi::PyObject, value: *mut ffi::PyObject) -> libc::c_int;
    fn PyErr_GetHandledException() -> *mut ffi::PyObject;
    fn PyErr_SetHandledException(exc: *mut ffi::PyObject);
    fn PyErr_SetRaisedException(exc: *mut ffi::PyObject);
}

pub type ObjPtr = *mut c_void;

#[repr(C)]
struct SoacPyLongValue {
    lv_tag: usize,
    ob_digit: [u32; 1],
}
#[repr(C)]
struct SoacPyLongObject {
    ob_base: ffi::PyObject,
    long_value: SoacPyLongValue,
}
const PY_LONG_SIGN_MASK: usize = 3;
const PY_LONG_NON_SIZE_BITS: usize = 3;
unsafe fn is_cell_object(obj: *mut ffi::PyObject) -> bool {
    !obj.is_null() && ffi::Py_TYPE(obj) == std::ptr::addr_of_mut!(PyCell_Type)
}

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

#[cold]
#[inline(never)]
unsafe extern "C" fn soac_runtime_set_runtime_error_static(message: *const c_char) {
    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, message);
}
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
unsafe extern "C" fn py_call_positional_three_hook(
    tstate: ObjPtr,
    callable: ObjPtr,
    arg1: ObjPtr,
    arg2: ObjPtr,
    arg3: ObjPtr,
) -> ObjPtr {
    if tstate.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid null tstate in dp_jit_py_call_positional_three\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let args = [
        arg1 as *mut ffi::PyObject,
        arg2 as *mut ffi::PyObject,
        arg3 as *mut ffi::PyObject,
    ];
    let nargs = args
        .iter()
        .position(|arg| arg.is_null())
        .unwrap_or(args.len());
    ffi::_PyObject_VectorcallTstate(
        tstate as *mut ffi::PyThreadState,
        callable as *mut ffi::PyObject,
        if nargs == 0 {
            ptr::null()
        } else {
            args.as_ptr()
        },
        nargs,
        ptr::null_mut(),
    ) as ObjPtr
}
unsafe extern "C" fn py_call_object_hook(callable: ObjPtr, args: ObjPtr) -> ObjPtr {
    ffi::PyObject_CallObject(callable as *mut ffi::PyObject, args as *mut ffi::PyObject) as ObjPtr
}
unsafe extern "C" fn py_vectorcall_hook(
    tstate: ObjPtr,
    callable: ObjPtr,
    args: ObjPtr,
    nargsf: ObjPtr,
    kwnames: ObjPtr,
) -> ObjPtr {
    if tstate.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid null tstate in dp_jit_py_vectorcall\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    ffi::_PyObject_VectorcallTstate(
        tstate as *mut ffi::PyThreadState,
        callable as *mut ffi::PyObject,
        args as *const *mut ffi::PyObject,
        nargsf as usize,
        kwnames as *mut ffi::PyObject,
    ) as ObjPtr
}
unsafe extern "C" fn next_or_sentinel_hook(iterator: ObjPtr, sentinel: ObjPtr) -> ObjPtr {
    if iterator.is_null() || sentinel.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_next_or_sentinel\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let mut item: *mut ffi::PyObject = ptr::null_mut();
    match PyIter_NextItem(iterator as *mut ffi::PyObject, ptr::addr_of_mut!(item)) {
        1 => item as ObjPtr,
        0 => {
            ffi::Py_INCREF(sentinel as *mut ffi::PyObject);
            sentinel
        }
        _ => ptr::null_mut(),
    }
}

unsafe extern "C" fn enter_recursive_call_hook(_tstate: ObjPtr) -> i32 {
    ffi::Py_EnterRecursiveCall(b" while calling a Python object\0".as_ptr() as *const i8)
}

unsafe extern "C" fn pytype_generic_alloc_hook(type_obj: ObjPtr, nitems: i64) -> ObjPtr {
    if type_obj.is_null() || nitems < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pytype_generic_alloc\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    PyType_GenericAlloc(
        type_obj as *mut ffi::PyTypeObject,
        nitems as ffi::Py_ssize_t,
    ) as ObjPtr
}

unsafe extern "C" fn finish_constructor_init_hook(obj: ObjPtr, init_result: ObjPtr) -> ObjPtr {
    if obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid constructor object in dp_jit_finish_constructor_init\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let obj = obj as *mut ffi::PyObject;
    let init_result = init_result as *mut ffi::PyObject;
    if init_result.is_null() {
        ffi::Py_DECREF(obj);
        return ptr::null_mut();
    }
    if init_result != ffi::Py_None() {
        let type_name = object_type_name(init_result);
        let message = format!("__init__() should return None, not '{type_name}'");
        if let Ok(c_message) = std::ffi::CString::new(message) {
            ffi::PyErr_SetString(ffi::PyExc_TypeError, c_message.as_ptr());
        } else {
            ffi::PyErr_SetString(
                ffi::PyExc_TypeError,
                b"__init__() should return None\0".as_ptr() as *const i8,
            );
        }
        ffi::Py_DECREF(init_result);
        ffi::Py_DECREF(obj);
        return ptr::null_mut();
    }
    ffi::Py_DECREF(init_result);
    obj as ObjPtr
}
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
unsafe extern "C" fn py_call_with_kw_hook(
    callable: ObjPtr,
    args: ObjPtr,
    kwargs: ObjPtr,
) -> ObjPtr {
    let result = ffi::PyObject_Call(
        callable as *mut ffi::PyObject,
        args as *mut ffi::PyObject,
        kwargs as *mut ffi::PyObject,
    ) as ObjPtr;
    result
}
unsafe extern "C" fn record_top_value_sample_hook(counter: ObjPtr, value: i64) {
    if counter.is_null() || value < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_record_top_value_sample\0".as_ptr() as *const i8,
        );
        return;
    }
    if let Err(err) = crate::module_type::record_top_value_sample_counter_ptr(counter, value as u64)
    {
        if let Ok(message) = std::ffi::CString::new(err) {
            ffi::PyErr_SetString(ffi::PyExc_RuntimeError, message.as_ptr());
        } else {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"failed to record top-value sample\0".as_ptr() as *const i8,
            );
        }
    }
}
unsafe extern "C" fn get_arg_item_hook(args: ObjPtr, index: i64) -> ObjPtr {
    if args.is_null() {
        return ptr::null_mut();
    }
    ffi::PySequence_GetItem(args as *mut ffi::PyObject, index as ffi::Py_ssize_t) as ObjPtr
}
unsafe fn load_global_obj_impl(
    globals_obj: ObjPtr,
    name_obj: *mut ffi::PyObject,
    slot_index: i64,
) -> ObjPtr {
    if globals_obj.is_null() || name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_load_global_obj\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    if slot_index >= 0 {
        let mut value = ptr::null_mut();
        let rc = _PyDict_GetIndexedItem(
            globals_obj as *mut ffi::PyObject,
            slot_index as ffi::Py_ssize_t,
            ptr::addr_of_mut!(value),
        );
        if rc > 0 {
            return value as ObjPtr;
        }
        if rc < 0 {
            // The module dict can be promoted after an unprofiled key insertion.
            // Fall back to the mapping lookup so the JIT remains semantically CPython-like.
            ffi::PyErr_Clear();
        }
    }
    load_global_slow(globals_obj as *mut ffi::PyObject, name_obj) as ObjPtr
}
unsafe fn ensure_global_load_error(result: ObjPtr, name_obj: *mut ffi::PyObject) -> ObjPtr {
    if result.is_null() && ffi::PyErr_Occurred().is_null() {
        raise_name_error_for_missing_name(name_obj);
    }
    result
}
unsafe fn guarded_indexed_global_slot(
    globals_obj: ObjPtr,
    name_obj: *mut ffi::PyObject,
    expected_index: i64,
) -> i64 {
    if expected_index < 0 {
        return -1;
    }
    let actual_index = _PyDict_IndexedKeyIndex(globals_obj as *mut ffi::PyObject, name_obj) as i64;
    if actual_index == expected_index {
        return actual_index;
    }
    if !ffi::PyErr_Occurred().is_null() {
        ffi::PyErr_Clear();
    }
    -1
}
unsafe fn globals_builtins_owned(globals_obj: *mut ffi::PyObject) -> *mut ffi::PyObject {
    if ffi::PyDict_Check(globals_obj) != 0 {
        let builtins = ffi::PyDict_GetItemString(globals_obj, c"__builtins__".as_ptr());
        if !builtins.is_null() {
            ffi::Py_INCREF(builtins);
            return builtins;
        }
        if !ffi::PyErr_Occurred().is_null() {
            return ptr::null_mut();
        }
    } else {
        let key = ffi::PyUnicode_FromString(c"__builtins__".as_ptr());
        if key.is_null() {
            return ptr::null_mut();
        }
        let builtins = ffi::PyObject_GetItem(globals_obj, key);
        ffi::Py_DECREF(key);
        if !builtins.is_null() {
            return builtins;
        }
        if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0 {
            return ptr::null_mut();
        }
        ffi::PyErr_Clear();
    }

    let builtins = ffi::PyEval_GetBuiltins();
    if builtins.is_null() {
        ptr::null_mut()
    } else {
        ffi::Py_INCREF(builtins);
        builtins
    }
}
unsafe fn load_global_slow(
    globals_obj: *mut ffi::PyObject,
    name_obj: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    if ffi::PyDict_Check(globals_obj) != 0 {
        let mut value = ptr::null_mut();
        let rc = PyDict_GetItemRef(globals_obj, name_obj, ptr::addr_of_mut!(value));
        if rc > 0 {
            return value;
        }
        if rc < 0 {
            return ptr::null_mut();
        }
    } else {
        let value = ffi::PyObject_GetItem(globals_obj, name_obj);
        if !value.is_null() {
            return value;
        }
        if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0 {
            return ptr::null_mut();
        }
        ffi::PyErr_Clear();
    }

    let builtins = globals_builtins_owned(globals_obj);
    if builtins.is_null() {
        return ptr::null_mut();
    }
    let value = if ffi::PyDict_Check(builtins) != 0 {
        let mut value = ptr::null_mut();
        let rc = PyDict_GetItemRef(builtins, name_obj, ptr::addr_of_mut!(value));
        ffi::Py_DECREF(builtins);
        if rc > 0 {
            return value;
        }
        if rc < 0 {
            return ptr::null_mut();
        }
        raise_name_error_for_missing_name(name_obj);
        return ptr::null_mut();
    } else {
        ffi::PyObject_GetAttr(builtins, name_obj)
    };
    ffi::Py_DECREF(builtins);
    if !value.is_null() {
        return value;
    }
    if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0
        && ffi::PyErr_ExceptionMatches(ffi::PyExc_AttributeError) == 0
    {
        return ptr::null_mut();
    }
    ffi::PyErr_Clear();
    raise_name_error_for_missing_name(name_obj);
    ptr::null_mut()
}
unsafe extern "C" fn pyobject_getattr_hook(obj: ObjPtr, attr: ObjPtr) -> ObjPtr {
    if obj.is_null() || attr.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_getattr\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let result =
        ffi::PyObject_GetAttr(obj as *mut ffi::PyObject, attr as *mut ffi::PyObject) as ObjPtr;
    result
}
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
unsafe fn exact_compact_long_value(obj: *mut ffi::PyObject) -> Option<ffi::Py_ssize_t> {
    if ffi::PyLong_CheckExact(obj) == 0 {
        return None;
    }
    let long = obj as *const SoacPyLongObject;
    let long_value = &(*long).long_value;
    if long_value.lv_tag >= (2 << PY_LONG_NON_SIZE_BITS) {
        return None;
    }

    let sign = 1isize - (long_value.lv_tag & PY_LONG_SIGN_MASK) as isize;
    Some(sign * long_value.ob_digit[0] as ffi::Py_ssize_t)
}
unsafe fn exact_list_index(
    obj: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
) -> Option<ffi::Py_ssize_t> {
    if ffi::PyList_CheckExact(obj) == 0 {
        return None;
    }

    let mut index = if let Some(index) = exact_compact_long_value(key) {
        index
    } else {
        let index = ffi::PyLong_AsSsize_t(key);
        if index == -1 && !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        index
    };

    let len = ffi::PyList_GET_SIZE(obj);
    if index < 0 {
        index += len;
    }
    (0 <= index && index < len).then_some(index)
}
unsafe fn new_none() -> ObjPtr {
    let none = ffi::Py_None();
    ffi::Py_INCREF(none);
    none as ObjPtr
}
unsafe extern "C" fn pyobject_getitem_hook(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    if obj.is_null() || key.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_getitem\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let obj = obj as *mut ffi::PyObject;
    let key = key as *mut ffi::PyObject;
    if let Some(index) = exact_list_index(obj, key) {
        return ffi::Py_NewRef(ffi::PyList_GET_ITEM(obj, index)) as ObjPtr;
    }
    let result = ffi::PyObject_GetItem(obj, key) as ObjPtr;
    result
}
unsafe extern "C" fn pyobject_setitem_hook(obj: ObjPtr, key: ObjPtr, value: ObjPtr) -> ObjPtr {
    if obj.is_null() || key.is_null() || value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_setitem\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let obj = obj as *mut ffi::PyObject;
    let key = key as *mut ffi::PyObject;
    let value = value as *mut ffi::PyObject;
    if let Some(index) = exact_list_index(obj, key) {
        let old_value = ffi::PyList_GET_ITEM(obj, index);
        ffi::PyList_SET_ITEM(obj, index, ffi::Py_NewRef(value));
        ffi::Py_DECREF(old_value);
        return new_none();
    }

    let rc = ffi::PyObject_SetItem(obj, key, value);
    if rc == 0 { new_none() } else { ptr::null_mut() }
}
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
unsafe extern "C" fn store_global_hook(
    globals_obj: ObjPtr,
    name: ObjPtr,
    slot_index: i64,
    value: ObjPtr,
) -> ObjPtr {
    if globals_obj.is_null() || name.is_null() || value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_store_global\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    if slot_index >= 0 {
        let rc = _PyDict_SetIndexedItem(
            globals_obj as *mut ffi::PyObject,
            slot_index as ffi::Py_ssize_t,
            value as *mut ffi::PyObject,
        );
        if rc == 0 {
            ffi::Py_INCREF(value as *mut ffi::PyObject);
            return value;
        }
        ffi::PyErr_Clear();
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
unsafe extern "C" fn del_global_hook(
    globals_obj: ObjPtr,
    key: ObjPtr,
    _slot_index: i64,
    quietly: bool,
) -> ObjPtr {
    if globals_obj.is_null() || key.is_null() {
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
    if quietly {
        return del_quietly_hook(globals_obj, key);
    }
    pyobject_delitem_hook(globals_obj, key)
}
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

unsafe extern "C" fn raise_missing_required_argument_hook() {
    ffi::PyErr_SetString(
        ffi::PyExc_TypeError,
        c"missing required argument in direct JIT call".as_ptr(),
    );
}
unsafe extern "C" fn make_cell_hook(value: ObjPtr) -> ObjPtr {
    PyCell_New(value as *mut ffi::PyObject) as ObjPtr
}
unsafe extern "C" fn load_cell_hook(cell: ObjPtr) -> ObjPtr {
    if !is_cell_object(cell as *mut ffi::PyObject) {
        raise_expected_cell("dp_jit_load_cell", cell as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    let value = PyCell_Get(cell as *mut ffi::PyObject);
    if value.is_null() {
        if ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) != 0 {
            ffi::PyErr_Clear();
        }
        ffi::PyErr_SetString(
            ffi::PyExc_UnboundLocalError,
            b"local variable referenced before assignment\0".as_ptr() as *const i8,
        );
    }
    value as ObjPtr
}
unsafe extern "C" fn store_cell_hook(cell: ObjPtr, value: ObjPtr) -> ObjPtr {
    if !is_cell_object(cell as *mut ffi::PyObject) {
        raise_expected_cell("dp_jit_store_cell", cell as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    if PyCell_Set(cell as *mut ffi::PyObject, value as *mut ffi::PyObject) < 0 {
        return ptr::null_mut();
    }
    let none = ffi::Py_None();
    ffi::Py_INCREF(none);
    none as ObjPtr
}
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
unsafe extern "C" fn load_global_obj_hook(
    globals_obj: ObjPtr,
    name: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    let name_obj = name as *mut ffi::PyObject;
    let result = load_global_obj_impl(globals_obj, name_obj, slot_index);
    ensure_global_load_error(result, name_obj)
}
unsafe extern "C" fn tuple_new_hook(size: i64) -> ObjPtr {
    if size < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid tuple size in JIT\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let result = ffi::PyTuple_New(size as ffi::Py_ssize_t) as ObjPtr;
    result
}
unsafe extern "C" fn tuple_set_item_hook(tuple_obj: ObjPtr, index: i64, value: ObjPtr) -> i32 {
    if tuple_obj.is_null() || value.is_null() || index < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid tuple_set_item arguments in JIT\0".as_ptr() as *const i8,
        );
        return -1;
    }
    let result = ffi::PyTuple_SetItem(
        tuple_obj as *mut ffi::PyObject,
        index as ffi::Py_ssize_t,
        value as *mut ffi::PyObject,
    );
    result
}
unsafe extern "C" fn dict_new_hook() -> ObjPtr {
    ffi::PyDict_New() as ObjPtr
}
unsafe extern "C" fn dict_set_item_hook(dict_obj: ObjPtr, key: ObjPtr, value: ObjPtr) -> i32 {
    if dict_obj.is_null() || key.is_null() || value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid dict_set_item arguments in JIT\0".as_ptr() as *const i8,
        );
        return -1;
    }
    ffi::PyDict_SetItem(
        dict_obj as *mut ffi::PyObject,
        key as *mut ffi::PyObject,
        value as *mut ffi::PyObject,
    )
}
unsafe extern "C" fn is_true_hook(value: ObjPtr) -> i32 {
    if value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid null value for dp_jit_is_true\0".as_ptr() as *const i8,
        );
        return -1;
    }
    ffi::PyObject_IsTrue(value as *mut ffi::PyObject)
}
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
unsafe fn attach_implicit_exception_context(exc: *mut ffi::PyObject, previous: *mut ffi::PyObject) {
    if previous.is_null() || ptr::eq(exc, previous) {
        return;
    }

    let suppress = ffi::PyObject_GetAttrString(exc, c"__suppress_context__".as_ptr());
    if suppress.is_null() {
        ffi::PyErr_Clear();
    } else {
        let is_suppressed = ffi::PyObject_IsTrue(suppress);
        ffi::Py_DECREF(suppress);
        if is_suppressed > 0 {
            return;
        }
        if is_suppressed < 0 {
            ffi::PyErr_Clear();
            return;
        }
    }

    let context = ffi::PyObject_GetAttrString(exc, c"__context__".as_ptr());
    if context.is_null() {
        ffi::PyErr_Clear();
        return;
    }
    let has_context = !ptr::eq(context, ffi::Py_None());
    ffi::Py_DECREF(context);
    if has_context {
        return;
    }

    if ffi::PyObject_SetAttrString(exc, c"__context__".as_ptr(), previous) != 0 {
        ffi::PyErr_Clear();
    }
}
unsafe extern "C" fn push_handled_exception_hook(exc: ObjPtr) -> ObjPtr {
    if exc.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"missing exception for dp_jit_push_handled_exception\0".as_ptr() as *const i8,
        );
        return ptr::null_mut();
    }
    let previous = PyErr_GetHandledException();
    attach_implicit_exception_context(exc as *mut ffi::PyObject, previous);
    PyErr_SetHandledException(exc as *mut ffi::PyObject);
    previous as ObjPtr
}
unsafe extern "C" fn pop_handled_exception_hook(previous: ObjPtr) {
    let previous = previous as *mut ffi::PyObject;
    PyErr_SetHandledException(previous);
    if !previous.is_null() {
        ffi::Py_DECREF(previous);
    }
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

    macro_rules! panic_dual_obj_export {
        (
            $fast:ident,
            $with_frame:ident($($arg:ident : $ty:ty),* $(,)?)
        ) => {
            panic_obj_export!($fast($($arg : $ty),*));
            panic_obj_export!($with_frame($($arg : $ty),*));
        };
    }

    macro_rules! panic_dual_i32_export {
        (
            $fast:ident,
            $with_frame:ident($($arg:ident : $ty:ty),* $(,)?)
        ) => {
            panic_i32_export!($fast($($arg : $ty),*));
            panic_i32_export!($with_frame($($arg : $ty),*));
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

    panic_dual_i32_export!(dp_jit_raise_from_exc, dp_jit_raise_from_exc_with_frame(
        exc: ObjPtr
    ));
    panic_obj_export!(dp_jit_push_handled_exception(exc: ObjPtr));
    panic_unit_export!(dp_jit_pop_handled_exception(previous: ObjPtr));
    panic_dual_i32_export!(dp_jit_guard_method_type_version, dp_jit_guard_method_type_version_with_frame(
        receiver: ObjPtr,
        expected_type: ObjPtr,
        expected_version: i64
    ));
    panic_dual_obj_export!(dp_jit_py_call_positional_three, dp_jit_py_call_positional_three_with_frame(
        tstate: ObjPtr,
        callable: ObjPtr,
        arg1: ObjPtr,
        arg2: ObjPtr,
        arg3: ObjPtr,
        sentinel: ObjPtr,
    ));
    panic_dual_obj_export!(dp_jit_py_call_object, dp_jit_py_call_object_with_frame(
        callable: ObjPtr,
        args: ObjPtr
    ));
    panic_dual_obj_export!(dp_jit_py_vectorcall, dp_jit_py_vectorcall_with_frame(
        tstate: ObjPtr,
        callable: ObjPtr,
        args: ObjPtr,
        nargsf: ObjPtr,
        kwnames: ObjPtr
    ));
    panic_dual_obj_export!(dp_jit_next_or_sentinel, dp_jit_next_or_sentinel_with_frame(
        iterator: ObjPtr,
        sentinel: ObjPtr
    ));
    panic_dual_obj_export!(dp_jit_py_call_with_kw, dp_jit_py_call_with_kw_with_frame(
        callable: ObjPtr,
        args: ObjPtr,
        kw: ObjPtr
    ));
    panic_unit_export!(dp_jit_record_top_value_sample(counter: ObjPtr, value: i64));
    panic_dual_obj_export!(dp_jit_get_arg_item, dp_jit_get_arg_item_with_frame(
        args: ObjPtr,
        index: i64
    ));
    panic_obj_export!(dp_jit_load_runtime_obj(name: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_getattr(obj: ObjPtr, attr: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_setattr(obj: ObjPtr, attr: ObjPtr, value: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_getitem(obj: ObjPtr, key: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_setitem(obj: ObjPtr, key: ObjPtr, value: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_delitem(obj: ObjPtr, key: ObjPtr));
    panic_obj_export!(dp_jit_load_global_obj(
        globals_obj: ObjPtr,
        name: ObjPtr,
        slot_index: i64
    ));
    panic_obj_export!(soac_runtime_load_global_slow(
        globals_obj: ObjPtr,
        name: ObjPtr,
        expected_index: i64
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
    panic_unit_export!(dp_jit_raise_i64_overflow());
    panic_obj_export!(dp_jit_make_cell(value: ObjPtr));
    panic_unit_export!(dp_jit_raise_deleted_name_error(name: ObjPtr));
    panic_unit_export!(dp_jit_raise_missing_required_argument());
    panic_obj_export!(dp_jit_load_cell(cell: ObjPtr));
    panic_obj_export!(dp_jit_store_cell(cell: ObjPtr, value: ObjPtr));
    panic_obj_export!(dp_jit_del_deref(cell: ObjPtr));
    panic_obj_export!(dp_jit_del_deref_quietly(cell: ObjPtr));
    panic_obj_export!(dp_jit_deopt_resume(
        deopt_table: ObjPtr,
        globals_obj: ObjPtr,
        record_ordinal: i64,
        live_values: ObjPtr,
        live_value_count: i64
    ));
    panic_obj_export!(dp_jit_tuple_new(size: i64));
    panic_i32_export!(dp_jit_tuple_set_item(tuple_obj: ObjPtr, index: i64, item: ObjPtr));
    panic_obj_export!(dp_jit_dict_new());
    panic_i32_export!(dp_jit_dict_set_item(dict_obj: ObjPtr, key: ObjPtr, value: ObjPtr));
    panic_i32_export!(dp_jit_is_true(value: ObjPtr));
    panic_dual_obj_export!(dp_jit_exact_long_binary_op, dp_jit_exact_long_binary_op_with_frame(
        kind: i64,
        lhs: ObjPtr,
        rhs: ObjPtr
    ));
    panic_obj_export!(dp_jit_exact_long_add_slot(lhs: ObjPtr, rhs: ObjPtr));
    panic_obj_export!(dp_jit_exact_long_sub_slot(lhs: ObjPtr, rhs: ObjPtr));
    panic_obj_export!(dp_jit_exact_long_mul_slot(lhs: ObjPtr, rhs: ObjPtr));
    panic_obj_export!(dp_jit_exact_long_true_div_slot(lhs: ObjPtr, rhs: ObjPtr));
    panic_obj_export!(dp_jit_exact_long_richcompare_slot(lhs: ObjPtr, rhs: ObjPtr, op: i32));
    panic_dual_obj_export!(dp_jit_exact_long_unary_op, dp_jit_exact_long_unary_op_with_frame(
        kind: i64,
        operand: ObjPtr
    ));
}

// Keep thin exported helpers as real call/return wrappers so perf can attribute
// time to them instead of tail-collapsing directly into the C API callee.
macro_rules! preserve_helper_frame {
    ($expr:expr) => {{
        let result = $expr;
        std::hint::black_box(result);
        result
    }};
}
macro_rules! define_perf_toggle_export {
    (
        $ret:ty,
        $fast:ident,
        $with_frame:ident($($arg:ident : $ty:ty),* $(,)?) => $body:expr
    ) => {
        pub unsafe extern "C" fn $fast($($arg: $ty),*) -> $ret {
            $body
        }

        #[inline(never)]
        pub unsafe extern "C" fn $with_frame($($arg: $ty),*) -> $ret {
            preserve_helper_frame!($body)
        }
    };
}
define_perf_toggle_export!(
    i32,
    dp_jit_raise_from_exc,
    dp_jit_raise_from_exc_with_frame(exc: ObjPtr) => raise_from_exc_hook(exc)
);
define_perf_toggle_export!(
    i32,
    dp_jit_guard_method_type_version,
    dp_jit_guard_method_type_version_with_frame(
        receiver: ObjPtr,
        expected_type: ObjPtr,
        expected_version: i64
    ) => guard_method_type_version_hook(receiver, expected_type, expected_version)
);
define_perf_toggle_export!(
    ObjPtr,
    dp_jit_py_call_positional_three,
    dp_jit_py_call_positional_three_with_frame(
        tstate: ObjPtr,
        callable: ObjPtr,
        arg1: ObjPtr,
        arg2: ObjPtr,
        arg3: ObjPtr,
        _sentinel: ObjPtr
    ) => py_call_positional_three_hook(tstate, callable, arg1, arg2, arg3)
);
define_perf_toggle_export!(
    ObjPtr,
    dp_jit_py_call_object,
    dp_jit_py_call_object_with_frame(callable: ObjPtr, args: ObjPtr) => py_call_object_hook(callable, args)
);
define_perf_toggle_export!(
    ObjPtr,
    dp_jit_py_vectorcall,
    dp_jit_py_vectorcall_with_frame(
        tstate: ObjPtr,
        callable: ObjPtr,
        args: ObjPtr,
        nargsf: ObjPtr,
        kwnames: ObjPtr
    ) => py_vectorcall_hook(tstate, callable, args, nargsf, kwnames)
);
define_perf_toggle_export!(
    ObjPtr,
    dp_jit_next_or_sentinel,
    dp_jit_next_or_sentinel_with_frame(iterator: ObjPtr, sentinel: ObjPtr) => next_or_sentinel_hook(iterator, sentinel)
);
define_perf_toggle_export!(
    ObjPtr,
    dp_jit_py_call_with_kw,
    dp_jit_py_call_with_kw_with_frame(callable: ObjPtr, args: ObjPtr, kw: ObjPtr) => py_call_with_kw_hook(callable, args, kw)
);
pub unsafe extern "C" fn dp_jit_record_top_value_sample(counter: ObjPtr, value: i64) {
    record_top_value_sample_hook(counter, value)
}
define_perf_toggle_export!(
    ObjPtr,
    dp_jit_get_arg_item,
    dp_jit_get_arg_item_with_frame(args: ObjPtr, index: i64) => get_arg_item_hook(args, index)
);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_load_runtime_obj(name: ObjPtr) -> ObjPtr {
    load_runtime_name_owned(name as *mut ffi::PyObject) as ObjPtr
}
pub unsafe extern "C" fn dp_jit_pyobject_getattr(obj: ObjPtr, attr: ObjPtr) -> ObjPtr {
    pyobject_getattr_hook(obj, attr)
}
pub unsafe extern "C" fn dp_jit_pyobject_setattr(
    obj: ObjPtr,
    attr: ObjPtr,
    value: ObjPtr,
) -> ObjPtr {
    pyobject_setattr_hook(obj, attr, value)
}
pub unsafe extern "C" fn dp_jit_pyobject_getitem(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    pyobject_getitem_hook(obj, key)
}
pub unsafe extern "C" fn dp_jit_pyobject_setitem(
    obj: ObjPtr,
    key: ObjPtr,
    value: ObjPtr,
) -> ObjPtr {
    pyobject_setitem_hook(obj, key, value)
}
pub unsafe extern "C" fn dp_jit_pyobject_delitem(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    pyobject_delitem_hook(obj, key)
}
pub unsafe extern "C" fn dp_jit_load_global_obj(
    globals_obj: ObjPtr,
    name: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    load_global_obj_hook(globals_obj, name, slot_index)
}
pub unsafe extern "C" fn soac_runtime_load_global_slow(
    globals_obj: ObjPtr,
    name: ObjPtr,
    expected_index: i64,
) -> ObjPtr {
    let name_obj = name as *mut ffi::PyObject;
    let slot_index = guarded_indexed_global_slot(globals_obj, name_obj, expected_index);
    let result = load_global_obj_impl(globals_obj, name_obj, slot_index);
    ensure_global_load_error(result, name_obj)
}
pub unsafe extern "C" fn dp_jit_store_global(
    globals_obj: ObjPtr,
    name: ObjPtr,
    slot_index: i64,
    value: ObjPtr,
) -> ObjPtr {
    store_global_hook(globals_obj, name, slot_index, value)
}
pub unsafe extern "C" fn dp_jit_del_global(
    globals_obj: ObjPtr,
    key: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    del_global_hook(globals_obj, key, slot_index, false)
}
pub unsafe extern "C" fn dp_jit_del_global_quietly(
    globals_obj: ObjPtr,
    key: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    del_global_hook(globals_obj, key, slot_index, true)
}
pub unsafe extern "C" fn dp_jit_del_quietly(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    del_quietly_hook(obj, key)
}
pub unsafe extern "C" fn dp_jit_pyobject_to_i64(value: ObjPtr) -> i64 {
    pyobject_to_i64_hook(value)
}
pub unsafe extern "C" fn dp_jit_make_cell(value: ObjPtr) -> ObjPtr {
    make_cell_hook(value)
}
pub unsafe extern "C" fn dp_jit_raise_deleted_name_error(name: ObjPtr) {
    raise_deleted_name_error_hook(name)
}
pub unsafe extern "C" fn dp_jit_raise_missing_required_argument() {
    raise_missing_required_argument_hook()
}
pub unsafe extern "C" fn dp_jit_load_cell(cell: ObjPtr) -> ObjPtr {
    load_cell_hook(cell)
}
pub unsafe extern "C" fn dp_jit_store_cell(cell: ObjPtr, value: ObjPtr) -> ObjPtr {
    store_cell_hook(cell, value)
}
pub unsafe extern "C" fn dp_jit_del_deref(cell: ObjPtr) -> ObjPtr {
    del_deref_hook(cell)
}
pub unsafe extern "C" fn dp_jit_del_deref_quietly(cell: ObjPtr) -> ObjPtr {
    del_deref_quietly_hook(cell)
}
#[cold]
#[inline(never)]
pub unsafe extern "C" fn dp_jit_deopt_resume(
    deopt_table: ObjPtr,
    globals_obj: ObjPtr,
    record_ordinal: i64,
    live_values: ObjPtr,
    live_value_count: i64,
) -> ObjPtr {
    match unsafe {
        run_deopt_resume(
            deopt_table,
            globals_obj,
            record_ordinal,
            live_values,
            live_value_count,
        )
    } {
        Ok(value) => value,
        Err(detail) => {
            set_deopt_unsupported_continuation_error(detail);
            ptr::null_mut()
        }
    }
}

#[cold]
unsafe fn run_deopt_resume(
    deopt_table: ObjPtr,
    globals_obj: ObjPtr,
    record_ordinal: i64,
    live_values: ObjPtr,
    live_value_count: i64,
) -> Result<ObjPtr, String> {
    let invocation = unsafe {
        RuntimeJitDeoptInvocation::from_raw(
            deopt_table,
            globals_obj,
            record_ordinal,
            live_values,
            live_value_count,
        )?
    };
    super::deopt_interpreter::execute_deopt_invocation(&invocation)
}

#[cold]
fn set_deopt_unsupported_continuation_error(detail: String) {
    let message = format!("JIT deopt helper is not implemented: {detail}");
    if let Ok(c_message) = std::ffi::CString::new(message) {
        unsafe {
            ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_message.as_ptr());
        }
    } else {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"JIT deopt helper is not implemented\0".as_ptr() as *const i8,
            );
        }
    }
}

pub unsafe extern "C" fn dp_jit_tuple_new(size: i64) -> ObjPtr {
    tuple_new_hook(size)
}
pub unsafe extern "C" fn dp_jit_tuple_set_item(tuple_obj: ObjPtr, index: i64, item: ObjPtr) -> i32 {
    tuple_set_item_hook(tuple_obj, index, item)
}
pub unsafe extern "C" fn dp_jit_dict_new() -> ObjPtr {
    dict_new_hook()
}
pub unsafe extern "C" fn dp_jit_dict_set_item(dict_obj: ObjPtr, key: ObjPtr, value: ObjPtr) -> i32 {
    dict_set_item_hook(dict_obj, key, value)
}
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
unsafe fn exact_long_type_mismatch_error() {
    ffi::PyErr_SetString(
        ffi::PyExc_RuntimeError,
        c"exact long specialization received a non-int operand".as_ptr(),
    );
}
unsafe fn exact_long_missing_slot_error() {
    ffi::PyErr_SetString(
        ffi::PyExc_RuntimeError,
        c"exact long specialization missing slot".as_ptr(),
    );
}
unsafe fn exact_long_i64_overflow_error() {
    // BEHAVIOR_CHANGE: optimized SOAC exact-int arithmetic intentionally raises
    // on i64 overflow instead of falling back to CPython's arbitrary-precision int.
    ffi::PyErr_SetString(
        ffi::PyExc_OverflowError,
        c"SOAC optimized integer arithmetic overflowed i64".as_ptr(),
    );
}
pub unsafe extern "C" fn dp_jit_raise_i64_overflow() {
    exact_long_i64_overflow_error();
}
#[inline(never)]
unsafe extern "C" fn exact_long_binary_op_hook(kind: i64, lhs: ObjPtr, rhs: ObjPtr) -> ObjPtr {
    let lhs = lhs as *mut ffi::PyObject;
    let rhs = rhs as *mut ffi::PyObject;
    if lhs.is_null() || rhs.is_null() {
        return ptr::null_mut();
    }
    let long_type = std::ptr::addr_of_mut!(PyLong_Type);
    if ffi::Py_TYPE(lhs) != long_type || ffi::Py_TYPE(rhs) != long_type {
        exact_long_type_mismatch_error();
        return ptr::null_mut();
    }
    let methods = (*long_type).tp_as_number;
    if methods.is_null() {
        exact_long_missing_slot_error();
        return ptr::null_mut();
    }

    let result = match kind {
        x if x == ExactIntBinaryOpKind::Add as i64 => (*methods).nb_add.map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::Sub as i64 => {
            (*methods).nb_subtract.map(|slot| slot(lhs, rhs))
        }
        x if x == ExactIntBinaryOpKind::Mul as i64 => {
            (*methods).nb_multiply.map(|slot| slot(lhs, rhs))
        }
        x if x == ExactIntBinaryOpKind::TrueDiv as i64 => {
            (*methods).nb_true_divide.map(|slot| slot(lhs, rhs))
        }
        x if x == ExactIntBinaryOpKind::FloorDiv as i64 => {
            (*methods).nb_floor_divide.map(|slot| slot(lhs, rhs))
        }
        x if x == ExactIntBinaryOpKind::Mod as i64 => {
            (*methods).nb_remainder.map(|slot| slot(lhs, rhs))
        }
        x if x == ExactIntBinaryOpKind::Pow as i64 => (*methods)
            .nb_power
            .map(|slot| slot(lhs, rhs, ffi::Py_None())),
        x if x == ExactIntBinaryOpKind::LShift as i64 => {
            (*methods).nb_lshift.map(|slot| slot(lhs, rhs))
        }
        x if x == ExactIntBinaryOpKind::RShift as i64 => {
            (*methods).nb_rshift.map(|slot| slot(lhs, rhs))
        }
        x if x == ExactIntBinaryOpKind::Or as i64 => (*methods).nb_or.map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::Xor as i64 => (*methods).nb_xor.map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::And as i64 => (*methods).nb_and.map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceAdd as i64 => (*methods)
            .nb_inplace_add
            .or((*methods).nb_add)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceSub as i64 => (*methods)
            .nb_inplace_subtract
            .or((*methods).nb_subtract)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceMul as i64 => (*methods)
            .nb_inplace_multiply
            .or((*methods).nb_multiply)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceTrueDiv as i64 => (*methods)
            .nb_inplace_true_divide
            .or((*methods).nb_true_divide)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceFloorDiv as i64 => (*methods)
            .nb_inplace_floor_divide
            .or((*methods).nb_floor_divide)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceMod as i64 => (*methods)
            .nb_inplace_remainder
            .or((*methods).nb_remainder)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplacePow as i64 => (*methods)
            .nb_inplace_power
            .or((*methods).nb_power)
            .map(|slot| slot(lhs, rhs, ffi::Py_None())),
        x if x == ExactIntBinaryOpKind::InplaceLShift as i64 => (*methods)
            .nb_inplace_lshift
            .or((*methods).nb_lshift)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceRShift as i64 => (*methods)
            .nb_inplace_rshift
            .or((*methods).nb_rshift)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceOr as i64 => (*methods)
            .nb_inplace_or
            .or((*methods).nb_or)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceXor as i64 => (*methods)
            .nb_inplace_xor
            .or((*methods).nb_xor)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::InplaceAnd as i64 => (*methods)
            .nb_inplace_and
            .or((*methods).nb_and)
            .map(|slot| slot(lhs, rhs)),
        x if x == ExactIntBinaryOpKind::Eq as i64 => (*long_type)
            .tp_richcompare
            .map(|slot| slot(lhs, rhs, ffi::Py_EQ)),
        x if x == ExactIntBinaryOpKind::Ne as i64 => (*long_type)
            .tp_richcompare
            .map(|slot| slot(lhs, rhs, ffi::Py_NE)),
        x if x == ExactIntBinaryOpKind::Lt as i64 => (*long_type)
            .tp_richcompare
            .map(|slot| slot(lhs, rhs, ffi::Py_LT)),
        x if x == ExactIntBinaryOpKind::Le as i64 => (*long_type)
            .tp_richcompare
            .map(|slot| slot(lhs, rhs, ffi::Py_LE)),
        x if x == ExactIntBinaryOpKind::Gt as i64 => (*long_type)
            .tp_richcompare
            .map(|slot| slot(lhs, rhs, ffi::Py_GT)),
        x if x == ExactIntBinaryOpKind::Ge as i64 => (*long_type)
            .tp_richcompare
            .map(|slot| slot(lhs, rhs, ffi::Py_GE)),
        _ => None,
    };
    let Some(result) = result else {
        exact_long_missing_slot_error();
        return ptr::null_mut();
    };
    result as ObjPtr
}
define_perf_toggle_export!(
    ObjPtr,
    dp_jit_exact_long_binary_op,
    dp_jit_exact_long_binary_op_with_frame(kind: i64, lhs: ObjPtr, rhs: ObjPtr) => exact_long_binary_op_hook(kind, lhs, rhs)
);
#[inline(never)]
unsafe extern "C" fn exact_long_unary_op_hook(kind: i64, operand: ObjPtr) -> ObjPtr {
    let operand = operand as *mut ffi::PyObject;
    if operand.is_null() {
        return ptr::null_mut();
    }
    let long_type = std::ptr::addr_of_mut!(PyLong_Type);
    if ffi::Py_TYPE(operand) != long_type {
        exact_long_type_mismatch_error();
        return ptr::null_mut();
    }
    let methods = (*long_type).tp_as_number;
    if methods.is_null() {
        exact_long_missing_slot_error();
        return ptr::null_mut();
    }
    match kind {
        x if x == ExactIntUnaryOpKind::Pos as i64 => {
            let Some(slot) = (*methods).nb_positive else {
                exact_long_missing_slot_error();
                return ptr::null_mut();
            };
            slot(operand) as ObjPtr
        }
        x if x == ExactIntUnaryOpKind::Neg as i64 => {
            let Some(slot) = (*methods).nb_negative else {
                exact_long_missing_slot_error();
                return ptr::null_mut();
            };
            slot(operand) as ObjPtr
        }
        x if x == ExactIntUnaryOpKind::Invert as i64 => {
            let Some(slot) = (*methods).nb_invert else {
                exact_long_missing_slot_error();
                return ptr::null_mut();
            };
            slot(operand) as ObjPtr
        }
        x if x == ExactIntUnaryOpKind::Not as i64 || x == ExactIntUnaryOpKind::Truth as i64 => {
            let Some(slot) = (*methods).nb_bool else {
                exact_long_missing_slot_error();
                return ptr::null_mut();
            };
            let truth = slot(operand);
            if truth < 0 {
                return ptr::null_mut();
            }
            let truth = if x == ExactIntUnaryOpKind::Not as i64 {
                (truth == 0) as libc::c_long
            } else {
                (truth != 0) as libc::c_long
            };
            ffi::PyBool_FromLong(truth) as ObjPtr
        }
        _ => {
            exact_long_missing_slot_error();
            ptr::null_mut()
        }
    }
}
define_perf_toggle_export!(
    ObjPtr,
    dp_jit_exact_long_unary_op,
    dp_jit_exact_long_unary_op_with_frame(kind: i64, operand: ObjPtr) => exact_long_unary_op_hook(kind, operand)
);

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

macro_rules! define_i64_obj_wrapper {
    ($fn_name:ident, $symbol:literal) => {
        unsafe extern "C" fn $fn_name(value: i64) -> ObjPtr {
            type Func = unsafe extern "C" fn(libc::c_longlong) -> *mut ffi::PyObject;
            static SYMBOL: OnceLock<usize> = OnceLock::new();
            let symbol = *SYMBOL.get_or_init(|| unsafe {
                load_python_capi_symbol(concat!($symbol, "\0").as_bytes())
            });
            if symbol == 0 {
                return ptr::null_mut();
            }
            let func: Func = unsafe { std::mem::transmute(symbol) };
            func(value as libc::c_longlong) as ObjPtr
        }
    };
}

define_i64_obj_wrapper!(pylong_from_longlong_wrapper, "PyLong_FromLongLong");
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

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|raw| {
            let trimmed = raw.trim();
            !(trimmed.is_empty() || trimmed == "0")
        })
        .unwrap_or(false)
}

fn should_preserve_perf_helper_frames() -> bool {
    env_flag_enabled("SOAC_JIT_PERF_HELPER_FRAMES")
}

fn chosen_helper_symbol(fast: *const u8, with_frame: *const u8) -> *const u8 {
    if cfg!(test) {
        return fast;
    }
    if should_preserve_perf_helper_frames() {
        with_frame
    } else {
        fast
    }
}
unsafe fn exact_long_number_slot_symbol(
    slot_name: &str,
    slot: Option<
        unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject) -> *mut ffi::PyObject,
    >,
) -> *const u8 {
    slot.unwrap_or_else(|| panic!("PyLong_Type is missing required number slot {slot_name}"))
        as *const u8
}
unsafe fn exact_long_richcompare_slot_symbol() -> *const u8 {
    let long_type = std::ptr::addr_of_mut!(PyLong_Type);
    (*long_type)
        .tp_richcompare
        .expect("PyLong_Type is missing required tp_richcompare slot") as *const u8
}
unsafe fn register_exact_long_slot_symbols(builder: &mut JITBuilder) {
    let long_type = std::ptr::addr_of_mut!(PyLong_Type);
    let number = (*long_type).tp_as_number;
    assert!(
        !number.is_null(),
        "PyLong_Type is missing required tp_as_number table"
    );

    builder.symbol(
        "dp_jit_exact_long_add_slot",
        exact_long_number_slot_symbol("nb_add", (*number).nb_add),
    );
    builder.symbol(
        "dp_jit_exact_long_sub_slot",
        exact_long_number_slot_symbol("nb_subtract", (*number).nb_subtract),
    );
    builder.symbol(
        "dp_jit_exact_long_mul_slot",
        exact_long_number_slot_symbol("nb_multiply", (*number).nb_multiply),
    );
    builder.symbol(
        "dp_jit_exact_long_true_div_slot",
        exact_long_number_slot_symbol("nb_true_divide", (*number).nb_true_divide),
    );
    builder.symbol(
        "dp_jit_exact_long_richcompare_slot",
        exact_long_richcompare_slot_symbol(),
    );
}

pub fn register_specialized_jit_symbols(builder: &mut JITBuilder) {
    builder.symbol(
        "PyFunction_Type",
        std::ptr::addr_of_mut!(PyFunction_Type) as *const u8,
    );
    builder.symbol(
        "PyMethod_Type",
        std::ptr::addr_of_mut!(PyMethod_Type) as *const u8,
    );
    builder.symbol(
        "soac_runtime_set_runtime_error_static",
        soac_runtime_set_runtime_error_static as *const u8,
    );
    builder.symbol(
        "dp_jit_py_call_positional_three",
        chosen_helper_symbol(
            dp_jit_py_call_positional_three as *const u8,
            dp_jit_py_call_positional_three_with_frame as *const u8,
        ),
    );
    builder.symbol(
        "dp_jit_py_call_object",
        chosen_helper_symbol(
            dp_jit_py_call_object as *const u8,
            dp_jit_py_call_object_with_frame as *const u8,
        ),
    );
    builder.symbol(
        "dp_jit_py_vectorcall",
        chosen_helper_symbol(
            dp_jit_py_vectorcall as *const u8,
            dp_jit_py_vectorcall_with_frame as *const u8,
        ),
    );
    builder.symbol(
        "dp_jit_next_or_sentinel",
        chosen_helper_symbol(
            dp_jit_next_or_sentinel as *const u8,
            dp_jit_next_or_sentinel_with_frame as *const u8,
        ),
    );
    builder.symbol(
        "dp_jit_enter_recursive_call",
        enter_recursive_call_hook as *const u8,
    );
    builder.symbol(
        "dp_jit_pytype_generic_alloc",
        pytype_generic_alloc_hook as *const u8,
    );
    builder.symbol(
        "dp_jit_finish_constructor_init",
        finish_constructor_init_hook as *const u8,
    );
    builder.symbol(
        "dp_jit_py_call_with_kw",
        chosen_helper_symbol(
            dp_jit_py_call_with_kw as *const u8,
            dp_jit_py_call_with_kw_with_frame as *const u8,
        ),
    );
    builder.symbol(
        "dp_jit_get_arg_item",
        chosen_helper_symbol(
            dp_jit_get_arg_item as *const u8,
            dp_jit_get_arg_item_with_frame as *const u8,
        ),
    );
    builder.symbol(
        "dp_jit_load_runtime_obj",
        dp_jit_load_runtime_obj as *const u8,
    );
    builder.symbol(
        "dp_jit_vectorcall_bind_direct_args",
        crate::bind_direct_args_from_vectorcall as *const u8,
    );
    builder.symbol(
        "dp_jit_vectorcall_compile_function_env",
        crate::vectorcall_compile_function_env as *const u8,
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
    builder.symbol(
        "soac_runtime_load_global_slow",
        soac_runtime_load_global_slow as *const u8,
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
        "dp_jit_raise_i64_overflow",
        dp_jit_raise_i64_overflow as *const u8,
    );
    builder.symbol(
        "dp_jit_guard_method_type_version",
        chosen_helper_symbol(
            dp_jit_guard_method_type_version as *const u8,
            dp_jit_guard_method_type_version_with_frame as *const u8,
        ),
    );
    builder.symbol(
        "dp_jit_record_top_value_sample",
        dp_jit_record_top_value_sample as *const u8,
    );
    builder.symbol(
        "dp_jit_raise_deleted_name_error",
        dp_jit_raise_deleted_name_error as *const u8,
    );
    builder.symbol(
        "dp_jit_raise_missing_required_argument",
        dp_jit_raise_missing_required_argument as *const u8,
    );
    builder.symbol("dp_jit_make_cell", dp_jit_make_cell as *const u8);
    builder.symbol("dp_jit_load_cell", dp_jit_load_cell as *const u8);
    builder.symbol("dp_jit_store_cell", dp_jit_store_cell as *const u8);
    builder.symbol("dp_jit_del_deref", dp_jit_del_deref as *const u8);
    builder.symbol(
        "dp_jit_del_deref_quietly",
        dp_jit_del_deref_quietly as *const u8,
    );
    builder.symbol("dp_jit_deopt_resume", dp_jit_deopt_resume as *const u8);
    builder.symbol("dp_jit_tuple_new", dp_jit_tuple_new as *const u8);
    builder.symbol("dp_jit_tuple_set_item", dp_jit_tuple_set_item as *const u8);
    builder.symbol("dp_jit_dict_new", dp_jit_dict_new as *const u8);
    builder.symbol("dp_jit_dict_set_item", dp_jit_dict_set_item as *const u8);
    builder.symbol("dp_jit_is_true", dp_jit_is_true as *const u8);
    builder.symbol(
        "dp_jit_raise_from_exc",
        chosen_helper_symbol(
            dp_jit_raise_from_exc as *const u8,
            dp_jit_raise_from_exc_with_frame as *const u8,
        ),
    );
    builder.symbol(
        "dp_jit_push_handled_exception",
        push_handled_exception_hook as *const u8,
    );
    builder.symbol(
        "dp_jit_pop_handled_exception",
        pop_handled_exception_hook as *const u8,
    );
    builder.symbol(
        "PyObject_RichCompare",
        pyobject_richcompare_wrapper as *const u8,
    );
    builder.symbol(
        "PySequence_Contains",
        pysequence_contains_wrapper as *const u8,
    );
    builder.symbol(
        "PyLong_FromLongLong",
        pylong_from_longlong_wrapper as *const u8,
    );
    builder.symbol("PyObject_Not", pyobject_not_wrapper as *const u8);
    builder.symbol("PyObject_IsTrue", pyobject_is_true_wrapper as *const u8);
    builder.symbol(
        "dp_jit_exact_long_binary_op",
        chosen_helper_symbol(
            dp_jit_exact_long_binary_op as *const u8,
            dp_jit_exact_long_binary_op_with_frame as *const u8,
        ),
    );
    unsafe {
        register_exact_long_slot_symbols(builder);
    }
    builder.symbol(
        "dp_jit_exact_long_unary_op",
        chosen_helper_symbol(
            dp_jit_exact_long_unary_op as *const u8,
            dp_jit_exact_long_unary_op_with_frame as *const u8,
        ),
    );
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

    #[test]
    fn perf_helper_frames_env_respects_falsey_values() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "perf_helper_frames_env_respects_falsey_values",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let prior = std::env::var_os("SOAC_JIT_PERF_HELPER_FRAMES");
        unsafe { std::env::remove_var("SOAC_JIT_PERF_HELPER_FRAMES") };
        assert!(!should_preserve_perf_helper_frames());

        unsafe { std::env::set_var("SOAC_JIT_PERF_HELPER_FRAMES", "1") };
        assert!(should_preserve_perf_helper_frames());

        unsafe { std::env::set_var("SOAC_JIT_PERF_HELPER_FRAMES", "0") };
        assert!(!should_preserve_perf_helper_frames());

        unsafe { std::env::set_var("SOAC_JIT_PERF_HELPER_FRAMES", "") };
        assert!(!should_preserve_perf_helper_frames());

        match prior {
            Some(value) => unsafe { std::env::set_var("SOAC_JIT_PERF_HELPER_FRAMES", value) },
            None => unsafe { std::env::remove_var("SOAC_JIT_PERF_HELPER_FRAMES") },
        }
    }
}
