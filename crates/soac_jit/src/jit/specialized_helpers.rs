#![cfg_attr(test, allow(dead_code, unused_imports))]

use super::RuntimeJitDeoptInvocation;
use crate::handled_exception::{
    HandledExceptionRecord, HandledExceptionRegion, HandledExceptionState,
};
use crate::module_constants::raise_name_error_for_missing_name;
use crate::module_constants::{load_runtime_name_owned, load_runtime_name_owned_by_id};
use crate::preserved_state;
use cranelift_jit::JITBuilder;
use libc;
use pyo3::ffi;
use std::ffi::{CStr, c_char, c_void};
use std::ptr;

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
    fn _PyDict_HasNoLookupAliases(dict: *mut ffi::PyObject) -> libc::c_int;
    fn PyDict_GetItemRef(
        dict: *mut ffi::PyObject,
        key: *mut ffi::PyObject,
        result: *mut *mut ffi::PyObject,
    ) -> libc::c_int;
    fn PyMapping_GetOptionalItem(
        mapping: *mut ffi::PyObject,
        key: *mut ffi::PyObject,
        result: *mut *mut ffi::PyObject,
    ) -> libc::c_int;
    fn _PyEval_UnpackIterableStackRef(
        tstate: *mut ffi::PyThreadState,
        value: *mut ffi::PyObject,
        count: libc::c_int,
        count_after: libc::c_int,
        stack_pointer: *mut usize,
    ) -> libc::c_int;
    fn _PyTuple_FromStackRefStealOnSuccess(
        stack_refs: *const usize,
        count: ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject;
    fn PySoac_VectorcallWithContext(
        callable: *mut ffi::PyObject,
        args: *const *mut ffi::PyObject,
        nargsf: usize,
        kwnames: *mut ffi::PyObject,
        globals: *mut ffi::PyObject,
        namespace: *mut ffi::PyObject,
        builtins: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_ObjectCallWithContext(
        callable: *mut ffi::PyObject,
        args: *mut ffi::PyObject,
        kwargs: *mut ffi::PyObject,
        globals: *mut ffi::PyObject,
        namespace: *mut ffi::PyObject,
        builtins: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
}
unsafe extern "C" {
    static mut PyCell_Type: ffi::PyTypeObject;
    fn PyCell_New(obj: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyCell_Get(cell: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyCell_Set(cell: *mut ffi::PyObject, value: *mut ffi::PyObject) -> libc::c_int;
}

pub type ObjPtr = *mut c_void;

#[cold]
pub(super) unsafe extern "C" fn dp_jit_unpack_fixed_slow(
    tstate: ObjPtr,
    iterable: ObjPtr,
    arity: i64,
) -> ObjPtr {
    let Ok(count) = libc::c_int::try_from(arity) else {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_OverflowError,
                c"fixed unpack target count is outside CPython's supported range".as_ptr(),
            );
        }
        return ptr::null_mut();
    };
    if count < 0 {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_ValueError,
                c"fixed unpack target count cannot be negative".as_ptr(),
            );
        }
        return ptr::null_mut();
    }

    let count = count as usize;
    let mut stack_refs = Vec::<usize>::new();
    if stack_refs.try_reserve_exact(count).is_err() {
        unsafe { ffi::PyErr_NoMemory() };
        return ptr::null_mut();
    }
    stack_refs.resize(count, 0);

    let success = unsafe {
        _PyEval_UnpackIterableStackRef(
            tstate.cast(),
            iterable.cast(),
            count as libc::c_int,
            -1,
            stack_refs.as_mut_ptr().add(count),
        )
    };
    if success == 0 {
        return ptr::null_mut();
    }

    stack_refs.reverse();
    let tuple = unsafe {
        _PyTuple_FromStackRefStealOnSuccess(stack_refs.as_ptr(), count as ffi::Py_ssize_t)
    };
    if tuple.is_null() {
        let saved_error = unsafe { ffi::PyErr_GetRaisedException() };
        for stack_ref in stack_refs {
            let object = (stack_ref & !1usize) as *mut ffi::PyObject;
            unsafe { ffi::Py_DECREF(object) };
        }
        unsafe { ffi::PyErr_SetRaisedException(saved_error) };
    }
    tuple.cast()
}

unsafe fn owned_none_hook() -> ObjPtr {
    let none = unsafe { ffi::Py_None() };
    unsafe {
        ffi::Py_INCREF(none);
    }
    none.cast()
}

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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_set_runtime_error_static(message: *const c_char) {
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
            b"expected cell object\0".as_ptr().cast(),
        );
    }
}
unsafe extern "C" fn py_call_positional_three_hook(
    callable: ObjPtr,
    arg1: ObjPtr,
    arg2: ObjPtr,
    arg3: ObjPtr,
    globals: ObjPtr,
    namespace: ObjPtr,
    builtins: ObjPtr,
) -> ObjPtr {
    let args = [
        arg1 as *mut ffi::PyObject,
        arg2 as *mut ffi::PyObject,
        arg3 as *mut ffi::PyObject,
    ];
    let nargs = args
        .iter()
        .position(|arg| arg.is_null())
        .unwrap_or(args.len());
    PySoac_VectorcallWithContext(
        callable as *mut ffi::PyObject,
        if nargs == 0 {
            ptr::null()
        } else {
            args.as_ptr()
        },
        nargs,
        ptr::null_mut(),
        globals.cast(),
        namespace.cast(),
        builtins.cast(),
    ) as ObjPtr
}
unsafe extern "C" fn py_call_object_hook(
    callable: ObjPtr,
    args: ObjPtr,
    globals: ObjPtr,
    namespace: ObjPtr,
    builtins: ObjPtr,
) -> ObjPtr {
    PySoac_ObjectCallWithContext(
        callable.cast(),
        args.cast(),
        ptr::null_mut(),
        globals.cast(),
        namespace.cast(),
        builtins.cast(),
    ) as ObjPtr
}

#[cfg(test)]
#[test]
fn runtime_call_helpers_preserve_explicit_context_and_public_binding() {
    use pyo3::exceptions::{PyNotImplementedError, PyTypeError};
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyTuple};

    let _guard = crate::python_runtime_test_lock().lock().unwrap();
    crate::initialize_test_python();
    Python::attach(|py| {
        let actual_globals = PyDict::new(py);
        let actual_namespace = PyDict::new(py);
        let builtins = py.import("builtins").unwrap();
        let captured_builtins = PyDict::new(py);
        let marker = py.eval(c"object()", None, None).unwrap();
        captured_builtins
            .set_item("captured_marker", &marker)
            .unwrap();
        actual_globals
            .set_item("__builtins__", builtins.dict())
            .unwrap();
        let ordinary = PyDict::new(py);
        py.run(
            cr#"def globals(*values):
    return values
class View:
    @property
    def __dict__(self):
        return 'ordinary descriptor'
view = View()
"#,
            Some(&ordinary),
            None,
        )
        .unwrap();

        for positional in [true, false] {
            let invoke = |callable: &Bound<'_, PyAny>,
                          arguments: &[&Bound<'_, PyAny>],
                          namespace: Option<&Bound<'_, PyDict>>|
             -> PyResult<Bound<'_, PyAny>> {
                let namespace = namespace.map_or(ptr::null_mut(), Bound::as_ptr);
                let result = if positional {
                    let mut slots = [ptr::null_mut(); 3];
                    for (slot, value) in slots.iter_mut().zip(arguments) {
                        *slot = value.as_ptr().cast();
                    }
                    unsafe {
                        py_call_positional_three_hook(
                            callable.as_ptr().cast(),
                            slots[0],
                            slots[1],
                            slots[2],
                            actual_globals.as_ptr().cast(),
                            namespace.cast(),
                            captured_builtins.as_ptr().cast(),
                        )
                    }
                } else {
                    let tuple = PyTuple::new(py, arguments.iter().copied())?;
                    unsafe {
                        py_call_object_hook(
                            callable.as_ptr().cast(),
                            tuple.as_ptr().cast(),
                            actual_globals.as_ptr().cast(),
                            namespace.cast(),
                            captured_builtins.as_ptr().cast(),
                        )
                    }
                };
                unsafe { Bound::from_owned_ptr_or_err(py, result.cast()) }
            };

            let globals = builtins.getattr("globals").unwrap();
            let result = invoke(&globals, &[], Some(&actual_namespace)).unwrap();
            assert_eq!(result.as_ptr(), actual_globals.as_ptr());
            let none = py.None().into_bound(py);
            assert!(
                invoke(&globals, &[&none], Some(&actual_namespace))
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );

            for name in ["locals", "vars"] {
                let callable = builtins.getattr(name).unwrap();
                let result = invoke(&callable, &[], Some(&actual_namespace)).unwrap();
                assert_eq!(result.as_ptr(), actual_namespace.as_ptr());
                assert!(
                    invoke(&callable, &[], None)
                        .unwrap_err()
                        .is_instance_of::<PyNotImplementedError>(py)
                );
            }
            let eval = builtins.getattr("eval").unwrap();
            assert!(
                invoke(&eval, &[], Some(&actual_namespace))
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            let code = py
                .eval(
                    c"compile('captured_marker', '<context>', 'eval', dont_inherit=True)",
                    None,
                    None,
                )
                .unwrap();
            let target_globals = PyDict::new(py);
            let value = invoke(&eval, &[&code, target_globals.as_any()], None).unwrap();
            assert!(value.is(&marker));
            assert!(
                target_globals
                    .get_item("__builtins__")
                    .unwrap()
                    .unwrap()
                    .is(&captured_builtins)
            );
            let replacement_builtins = PyDict::new(py);
            replacement_builtins
                .set_item("captured_marker", py.None())
                .unwrap();
            target_globals
                .set_item("__builtins__", &replacement_builtins)
                .unwrap();
            assert!(
                invoke(&eval, &[&code, target_globals.as_any()], None)
                    .unwrap()
                    .is_none()
            );
            assert!(
                target_globals
                    .get_item("__builtins__")
                    .unwrap()
                    .unwrap()
                    .is(&replacement_builtins)
            );

            let ordinary_globals = ordinary.get_item("globals").unwrap().unwrap();
            let result = invoke(&ordinary_globals, &[&none], None).unwrap();
            assert_eq!(result.cast::<PyTuple>().unwrap().len(), 1);
            assert!(result.get_item(0).unwrap().is_none());
            let vars = builtins.getattr("vars").unwrap();
            let view = ordinary.get_item("view").unwrap().unwrap();
            assert_eq!(
                invoke(&vars, &[&view], None)
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "ordinary descriptor"
            );
        }
    });
}

#[cfg(test)]
#[test]
fn indexed_globals_preserve_alias_lookup_and_do_not_retry_policy_errors() {
    use pyo3::prelude::*;
    use pyo3::types::PyDict;

    let _guard = crate::python_runtime_test_lock().lock().unwrap();
    crate::initialize_test_python();
    Python::attach(|py| {
        let namespace = PyDict::new(py);
        py.run(
            cr#"import _testcapi, _testinternalcapi
class Alias:
    active = False
    def __hash__(self): return hash('field')
    def __eq__(self, other): return self.active and other == 'field'
alias = Alias()
globals_dict = _testinternalcapi.dict_new_indexed(('field',))
globals_dict[alias] = ['alias']
globals_dict['field'] = ['canonical']
alias.active = True
name = 'field'
builtins_dict = {'field': ['builtin']}
replacement = ['replacement']
calls = []
def reject(d, key, value, operation):
    calls.append(key)
    raise ValueError('owner refused write')
protected = {'field': 1}
owner = _testcapi.dict_set_soac_policy(protected, {'field': int}, (), reject)
"#,
            Some(&namespace),
            None,
        )
        .unwrap();
        let get = |name| namespace.get_item(name).unwrap().unwrap();
        let globals = get("globals_dict");
        let builtins = get("builtins_dict");
        let name = get("name");
        let replacement = get("replacement");
        unsafe {
            assert_eq!(
                guarded_indexed_global_slot(globals.as_ptr().cast(), name.as_ptr(), 0),
                -1
            );
            let value = soac_runtime_load_global_slow(
                globals.as_ptr().cast(),
                builtins.as_ptr().cast(),
                name.as_ptr().cast(),
                0,
            );
            assert!(!value.is_null());
            let value = Bound::<pyo3::PyAny>::from_owned_ptr(py, value.cast());
            assert_eq!(value.extract::<Vec<String>>().unwrap(), ["alias"]);
            let stored = store_global_hook(
                globals.as_ptr().cast(),
                name.as_ptr().cast(),
                0,
                replacement.as_ptr().cast(),
            );
            assert_eq!(stored, replacement.as_ptr().cast());
            ffi::Py_DECREF(stored.cast());
            let mut physical = ptr::null_mut();
            assert_eq!(
                _PyDict_GetIndexedItem(globals.as_ptr(), 0, &mut physical),
                1
            );
            let physical = Bound::<pyo3::PyAny>::from_owned_ptr(py, physical);
            assert_eq!(physical.extract::<Vec<String>>().unwrap(), ["canonical"]);

            let protected = get("protected");
            let value = ffi::PyLong_FromLong(2);
            let rejected = store_global_hook(
                protected.as_ptr().cast(),
                name.as_ptr().cast(),
                0,
                value.cast(),
            );
            assert!(rejected.is_null());
            let error = PyErr::fetch(py);
            assert!(error.is_instance_of::<pyo3::exceptions::PyValueError>(py));
            ffi::Py_DECREF(value);
            assert_eq!(get("calls").extract::<Vec<String>>().unwrap(), ["field"]);
        }
    });
}

unsafe extern "C" fn enter_recursive_call_hook(_tstate: ObjPtr) -> i32 {
    ffi::Py_EnterRecursiveCall(b" while calling a Python object\0".as_ptr().cast())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_enter_recursive_call(tstate: ObjPtr) -> i32 {
    enter_recursive_call_hook(tstate)
}

unsafe extern "C" fn guard_method_type_version_hook(
    receiver: ObjPtr,
    expected_type: ObjPtr,
    expected_version: i64,
) -> i32 {
    if receiver.is_null() || expected_type.is_null() || expected_version < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_guard_method_type_version\0"
                .as_ptr()
                .cast(),
        );
        return -1;
    }
    let receiver_type = ffi::Py_TYPE(receiver as *mut ffi::PyObject);
    if receiver_type != expected_type as *mut ffi::PyTypeObject {
        return 0;
    }
    ((*receiver_type).tp_version_tag == expected_version as u32) as i32
}

unsafe extern "C" fn record_top_value_sample_hook(counter: ObjPtr, value: i64) {
    if counter.is_null() || value < 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_record_top_value_sample\0"
                .as_ptr()
                .cast(),
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
                b"failed to record top-value sample\0".as_ptr().cast(),
            );
        }
    }
}

unsafe extern "C" fn protocol_next_function_id_hook(receiver: ObjPtr) -> i64 {
    protocol_method_function_id_hook(receiver, c"__next__")
}

unsafe extern "C" fn protocol_iter_function_id_hook(receiver: ObjPtr) -> i64 {
    protocol_method_function_id_hook(receiver, c"__iter__")
}

unsafe fn protocol_method_function_id_hook(receiver: ObjPtr, method_name: &std::ffi::CStr) -> i64 {
    if receiver.is_null() || !ffi::PyErr_Occurred().is_null() {
        return 0;
    }
    let receiver_type = ffi::Py_TYPE(receiver as *mut ffi::PyObject);
    if receiver_type.is_null() {
        return 0;
    }
    let dict = (*receiver_type).tp_dict;
    if dict.is_null() {
        return 0;
    }
    // An optional observation must not invoke equality/hash callbacks on
    // arbitrary class-dictionary keys, or run a descriptor a second time.
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    while ffi::PyDict_Next(dict, &mut position, &mut key, &mut descriptor) != 0 {
        if ffi::PyUnicode_CheckExact(key) != 0
            && ffi::PyUnicode_CompareWithASCIIString(key, method_name.as_ptr()) == 0
        {
            return profile_callable_function_id_hook(descriptor.cast());
        }
    }
    0
}

unsafe fn profile_callable_function_id_hook(callable: ObjPtr) -> i64 {
    // Preserve a pre-existing body error. Any error from optional owner
    // authentication is consumed here; the eventual public call independently
    // validates its authority and reports a required failure in ordinary order.
    if !ffi::PyErr_Occurred().is_null() {
        return 0;
    }
    match std::panic::catch_unwind(|| crate::observed_strict_function_id(callable.cast())) {
        Ok(Some(function)) => function.to_packed_runtime_u64() as i64,
        Ok(None) | Err(_) => {
            ffi::PyErr_Clear();
            0
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
    builtins_obj: ObjPtr,
    name_obj: *mut ffi::PyObject,
    slot_index: i64,
) -> ObjPtr {
    if globals_obj.is_null() || builtins_obj.is_null() || name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to soac_runtime_load_global_slow\0"
                .as_ptr()
                .cast(),
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
        if rc == 0 {
            return load_builtin_slow(builtins_obj.cast::<ffi::PyObject>(), name_obj).cast();
        }
        if rc < 0 {
            return ptr::null_mut();
        }
    }
    load_global_slow(
        globals_obj as *mut ffi::PyObject,
        builtins_obj as *mut ffi::PyObject,
        name_obj,
    ) as ObjPtr
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
    if expected_index < 0 || _PyDict_HasNoLookupAliases(globals_obj as *mut ffi::PyObject) == 0 {
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
unsafe fn load_global_slow(
    globals_obj: *mut ffi::PyObject,
    builtins_obj: *mut ffi::PyObject,
    name_obj: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let mut value = ptr::null_mut();
    let result =
        if ffi::PyDict_CheckExact(globals_obj) != 0 && ffi::PyDict_CheckExact(builtins_obj) != 0 {
            PyDict_GetItemRef(globals_obj, name_obj, ptr::addr_of_mut!(value))
        } else {
            PyMapping_GetOptionalItem(globals_obj, name_obj, ptr::addr_of_mut!(value))
        };
    match result {
        1 => value,
        0 => load_builtin_slow(builtins_obj, name_obj),
        _ => ptr::null_mut(),
    }
}

unsafe fn load_builtin_slow(
    builtins_obj: *mut ffi::PyObject,
    name_obj: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let mut value = ptr::null_mut();
    let result = if ffi::PyDict_CheckExact(builtins_obj) != 0 {
        PyDict_GetItemRef(builtins_obj, name_obj, ptr::addr_of_mut!(value))
    } else {
        PyMapping_GetOptionalItem(builtins_obj, name_obj, ptr::addr_of_mut!(value))
    };
    match result {
        1 => value,
        0 => {
            raise_name_error_for_missing_name(name_obj);
            ptr::null_mut()
        }
        _ => ptr::null_mut(),
    }
}
unsafe extern "C" fn pyobject_getattr_hook(obj: ObjPtr, attr: ObjPtr) -> ObjPtr {
    if obj.is_null() || attr.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_getattr\0"
                .as_ptr()
                .cast(),
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
            b"invalid arguments to dp_jit_pyobject_setattr\0"
                .as_ptr()
                .cast(),
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
unsafe fn new_none() -> ObjPtr {
    let none = ffi::Py_None();
    ffi::Py_INCREF(none);
    none as ObjPtr
}
unsafe extern "C" fn pyobject_getitem_hook(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    if obj.is_null() || key.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_getitem\0"
                .as_ptr()
                .cast(),
        );
        return ptr::null_mut();
    }
    let result =
        ffi::PyObject_GetItem(obj as *mut ffi::PyObject, key as *mut ffi::PyObject) as ObjPtr;
    result
}
unsafe extern "C" fn pyobject_setitem_hook(obj: ObjPtr, key: ObjPtr, value: ObjPtr) -> ObjPtr {
    if obj.is_null() || key.is_null() || value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_setitem\0"
                .as_ptr()
                .cast(),
        );
        return ptr::null_mut();
    }
    let rc = ffi::PyObject_SetItem(
        obj as *mut ffi::PyObject,
        key as *mut ffi::PyObject,
        value as *mut ffi::PyObject,
    );
    if rc == 0 { new_none() } else { ptr::null_mut() }
}

unsafe fn preserved_state_ref(state: ObjPtr) -> ObjPtr {
    if state.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"invalid null preserved-state object".as_ptr(),
        );
        return ptr::null_mut();
    }
    state
}

unsafe extern "C" fn pyobject_delitem_hook(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    if obj.is_null() || key.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_pyobject_delitem\0"
                .as_ptr()
                .cast(),
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
            b"invalid arguments to dp_jit_store_global\0"
                .as_ptr()
                .cast(),
        );
        return ptr::null_mut();
    }
    let slot_index =
        guarded_indexed_global_slot(globals_obj, name as *mut ffi::PyObject, slot_index);
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
        // A policy/allocator/callback error is final, not a layout miss. A
        // fallback retry could execute validation or user effects twice.
        return ptr::null_mut();
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
            b"invalid arguments to dp_jit_del_quietly\0".as_ptr().cast(),
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
                b"invalid arguments to dp_jit_del_global_quietly\0"
                    .as_ptr()
                    .cast()
            } else {
                b"invalid arguments to dp_jit_del_global\0".as_ptr().cast()
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
            b"invalid null value for dp_jit_pyobject_to_i64\0"
                .as_ptr()
                .cast(),
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
unsafe extern "C" fn raise_unbound_local_error_hook(name_obj: ObjPtr) {
    if name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid arguments to dp_jit_raise_unbound_local_error\0"
                .as_ptr()
                .cast(),
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
        b"cannot access local variable before assignment\0"
            .as_ptr()
            .cast(),
    );
}

unsafe extern "C" fn del_preserved_hook(
    state: ObjPtr,
    slot: i64,
    name_obj: ObjPtr,
    quietly: bool,
) -> ObjPtr {
    let state = preserved_state_ref(state);
    if state.is_null() {
        return ptr::null_mut();
    }
    let result = preserved_state::clear_preserved_slot(state.cast(), slot);
    if result < 0 {
        return ptr::null_mut();
    }
    if result == 0 && !quietly {
        raise_unbound_local_error_hook(name_obj);
        return ptr::null_mut();
    }
    owned_none_hook()
}

unsafe extern "C" fn raise_missing_required_argument_hook() {
    ffi::PyErr_SetString(
        ffi::PyExc_TypeError,
        c"missing required argument in direct JIT call".as_ptr(),
    );
}

unsafe extern "C" fn raise_super_arg_deleted_hook() {
    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c"super(): arg[0] deleted".as_ptr());
}

unsafe extern "C" fn make_cell_hook(value: ObjPtr) -> ObjPtr {
    PyCell_New(value as *mut ffi::PyObject) as ObjPtr
}
unsafe extern "C" fn load_cell_hook(cell: ObjPtr, name: ObjPtr, binding_kind: i64) -> ObjPtr {
    if name.is_null()
        || ffi::PyUnicode_CheckExact(name.cast()) == 0
        || !matches!(binding_kind, 0 | 1)
    {
        if ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_SetString(
                ffi::PyExc_SystemError,
                c"dp_jit_load_cell requires an exact name and source binding kind".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    if !is_cell_object(cell as *mut ffi::PyObject) {
        raise_expected_cell("dp_jit_load_cell", cell as *mut ffi::PyObject);
        return ptr::null_mut();
    }
    let value = PyCell_Get(cell as *mut ffi::PyObject);
    if value.is_null() && ffi::PyErr_Occurred().is_null() {
        // The binding kind belongs to the original Load operation, not the
        // current physical cell slot (which an inline clone may have remapped).
        let (exception, message) = if binding_kind == 0 {
            (
                ffi::PyExc_UnboundLocalError,
                c"cannot access local variable '%U' where it is not associated with a value",
            )
        } else {
            (
                ffi::PyExc_NameError,
                c"cannot access free variable '%U' where it is not associated with a value in enclosing scope",
            )
        };
        ffi::PyErr_Format(exception, message.as_ptr(), name.cast::<ffi::PyObject>());
        if binding_kind == 1 {
            // Match _PyEval_FormatExcCheckArg: only the free-variable
            // NameError gets .name; local UnboundLocalError leaves it None.
            let error = ffi::PyErr_GetRaisedException();
            if !error.is_null() {
                if ffi::PyErr_GivenExceptionMatches(error, ffi::PyExc_NameError) != 0 {
                    let _ = ffi::PyObject_SetAttrString(error, c"name".as_ptr(), name.cast());
                }
                ffi::PyErr_SetRaisedException(error);
            }
        }
    }
    value as ObjPtr
}

#[cfg(test)]
#[test]
fn cell_load_errors_use_the_original_binding_and_preserve_pending_errors() -> pyo3::PyResult<()> {
    use pyo3::prelude::*;
    use pyo3::types::PyString;

    let _guard = crate::python_runtime_test_lock().lock().unwrap();
    crate::initialize_test_python();
    Python::attach(|py| unsafe {
        let cell = Bound::<PyAny>::from_owned_ptr_or_err(py, PyCell_New(ptr::null_mut()))?;
        let name = PyString::new(py, "value");
        for (kind, exception, message) in [
            (
                0,
                ffi::PyExc_UnboundLocalError,
                "cannot access local variable 'value' where it is not associated with a value",
            ),
            (
                1,
                ffi::PyExc_NameError,
                "cannot access free variable 'value' where it is not associated with a value in enclosing scope",
            ),
        ] {
            assert!(dp_jit_load_cell(cell.as_ptr().cast(), name.as_ptr().cast(), kind).is_null());
            let error = PyErr::fetch(py);
            assert_eq!(error.get_type(py).as_ptr(), exception);
            assert_eq!(
                error.value(py).getattr("args")?.extract::<(String,)>()?.0,
                message
            );
            let actual_name = error.value(py).getattr("name")?;
            if kind == 1 {
                assert!(actual_name.is(&name));
            } else {
                assert!(actual_name.is_none());
            }
        }

        let pending = pyo3::exceptions::PyValueError::new_err("existing lookup error");
        let pending_value = pending.value(py).clone();
        pending.restore(py);
        assert!(dp_jit_load_cell(cell.as_ptr().cast(), name.as_ptr().cast(), 1).is_null());
        assert!(PyErr::fetch(py).value(py).is(&pending_value));

        assert!(dp_jit_load_cell(cell.as_ptr().cast(), name.as_ptr().cast(), 2).is_null());
        assert!(PyErr::fetch(py).is_instance_of::<pyo3::exceptions::PySystemError>(py));

        assert_eq!(PyCell_Set(cell.as_ptr(), ffi::Py_None()), 0);
        let value = Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            dp_jit_load_cell(cell.as_ptr().cast(), name.as_ptr().cast(), 1).cast(),
        )?;
        assert!(value.is_none());
        Ok(())
    })
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
        b"cell_contents\0".as_ptr().cast(),
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
            b"local variable referenced before assignment\0"
                .as_ptr()
                .cast(),
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
        b"cell_contents\0".as_ptr().cast(),
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
unsafe extern "C" fn dict_new_hook() -> ObjPtr {
    ffi::PyDict_New() as ObjPtr
}
unsafe extern "C" fn dict_set_item_hook(dict_obj: ObjPtr, key: ObjPtr, value: ObjPtr) -> i32 {
    if dict_obj.is_null() || key.is_null() || value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid dict_set_item arguments in JIT\0".as_ptr().cast(),
        );
        return -1;
    }
    ffi::PyDict_SetItem(
        dict_obj as *mut ffi::PyObject,
        key as *mut ffi::PyObject,
        value as *mut ffi::PyObject,
    )
}
fn immutable_singleton_truthiness(value: ObjPtr) -> Option<i32> {
    unsafe {
        if ptr::eq(value, ffi::Py_True().cast()) {
            Some(1)
        } else if ptr::eq(value, ffi::Py_False().cast()) || ptr::eq(value, ffi::Py_None().cast()) {
            Some(0)
        } else {
            None
        }
    }
}

unsafe extern "C" fn is_true_hook(value: ObjPtr) -> i32 {
    if value.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid null value for dp_jit_is_true\0".as_ptr().cast(),
        );
        return -1;
    }
    if let Some(truth) = immutable_singleton_truthiness(value) {
        return truth;
    }
    ffi::PyObject_IsTrue(value as *mut ffi::PyObject)
}

#[cfg(test)]
#[test]
fn immutable_singleton_truthiness_preserves_the_exported_python_protocol() {
    use pyo3::exceptions::{PyRuntimeError, PyValueError};
    use pyo3::prelude::*;

    let _guard = crate::python_runtime_test_lock().lock().unwrap();
    crate::initialize_test_python();
    Python::attach(|py| unsafe {
        let true_value = ffi::Py_True();
        let false_value = ffi::Py_False();
        let none_value = ffi::Py_None();
        let singleton_refcounts = [
            ffi::Py_REFCNT(true_value),
            ffi::Py_REFCNT(false_value),
            ffi::Py_REFCNT(none_value),
        ];

        assert_eq!(dp_jit_is_true(true_value.cast()), 1);
        assert_eq!(dp_jit_is_true(false_value.cast()), 0);
        assert_eq!(dp_jit_is_true(none_value.cast()), 0);
        assert_eq!(
            [
                ffi::Py_REFCNT(true_value),
                ffi::Py_REFCNT(false_value),
                ffi::Py_REFCNT(none_value),
            ],
            singleton_refcounts,
            "truthiness never takes ownership of immutable singleton arguments"
        );

        let custom = py
            .eval(
                c"type('TrackedTruth', (), {'__bool__': lambda self: setattr(self, 'calls', getattr(self, 'calls', 0) + 1) or True})()",
                None,
                None,
            )
            .expect("a custom truthiness callback should be created");
        let custom_refcount = ffi::Py_REFCNT(custom.as_ptr());
        assert_eq!(immutable_singleton_truthiness(custom.as_ptr().cast()), None);
        assert_eq!(dp_jit_is_true(custom.as_ptr().cast()), 1);
        assert_eq!(
            custom
                .getattr("calls")
                .expect("the custom callback should update its instance")
                .extract::<usize>()
                .expect("callback count should be an integer"),
            1,
            "ordinary Python truthiness callbacks must run exactly once"
        );
        assert_eq!(ffi::Py_REFCNT(custom.as_ptr()), custom_refcount);

        let empty = py
            .eval(
                c"type('SizedTruth', (), {'__len__': lambda self: 0})()",
                None,
                None,
            )
            .expect("a length-based truthiness object should be created");
        assert_eq!(immutable_singleton_truthiness(empty.as_ptr().cast()), None);
        assert_eq!(dp_jit_is_true(empty.as_ptr().cast()), 0);

        let raising = py
            .eval(
                c"type('RaisingTruth', (), {'__bool__': lambda self: (_ for _ in ()).throw(ValueError('truth exploded'))})()",
                None,
                None,
            )
            .expect("a raising truthiness object should be created");
        assert_eq!(
            immutable_singleton_truthiness(raising.as_ptr().cast()),
            None
        );
        assert_eq!(dp_jit_is_true(raising.as_ptr().cast()), -1);
        let raised = PyErr::fetch(py);
        assert!(raised.is_instance_of::<PyValueError>(py));
        assert_eq!(raised.to_string(), "ValueError: truth exploded");

        assert_eq!(immutable_singleton_truthiness(ptr::null_mut()), None);
        assert_eq!(dp_jit_is_true(ptr::null_mut()), -1);
        let null_error = PyErr::fetch(py);
        assert!(null_error.is_instance_of::<PyRuntimeError>(py));
        assert_eq!(
            null_error.to_string(),
            "RuntimeError: invalid null value for dp_jit_is_true"
        );

        assert_eq!(
            immutable_singleton_truthiness(true_value.cast()),
            Some(1),
            "the real immutable True singleton must bypass generic truthiness"
        );
        assert_eq!(
            immutable_singleton_truthiness(false_value.cast()),
            Some(0),
            "the real immutable False singleton must bypass generic truthiness"
        );
        assert_eq!(
            immutable_singleton_truthiness(none_value.cast()),
            Some(0),
            "the real immutable None singleton must bypass generic truthiness"
        );
    });
}

unsafe extern "C" fn raise_from_exc_hook(exc: ObjPtr) -> i32 {
    if exc.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"missing exception for dp_jit_raise_from_exc\0"
                .as_ptr()
                .cast(),
        );
        return -1;
    }
    let exc_obj = exc as *mut ffi::PyObject;
    // The native raise path chains the actual handled exception, including
    // supported C-API changes, before any handler transition is unwound.
    ffi::PyErr_SetObject(ffi::Py_TYPE(exc_obj).cast(), exc_obj);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_handled_state_init(
    preserved: ObjPtr,
    state: ObjPtr,
    records: ObjPtr,
    capacity: i64,
    deopt_table: ObjPtr,
) -> ObjPtr {
    let Ok(capacity) = usize::try_from(capacity) else {
        ffi::PyErr_SetString(
            ffi::PyExc_SystemError,
            c"invalid handled-state capacity".as_ptr(),
        );
        return ptr::null_mut();
    };
    if preserved.is_null() && (state.is_null() || records.is_null()) {
        ffi::PyErr_SetString(
            ffi::PyExc_SystemError,
            c"invalid handled-state storage".as_ptr(),
        );
        return ptr::null_mut();
    }
    if !preserved.is_null() {
        let empty_plan = crate::handled_exception::HandledExceptionPlan::default();
        // FunctionEnv owns the immutable original-first plan throughout this
        // call. A standalone handler-free body needs no region identities.
        let plan = if deopt_table.is_null() && capacity == 0 {
            &empty_plan
        } else if let Some(table) = unsafe {
            deopt_table
                .cast::<super::deopt::RuntimeJitDeoptTable>()
                .as_ref()
        } {
            &table.handled_plan
        } else {
            ffi::PyErr_SetString(
                ffi::PyExc_SystemError,
                c"suspended body is missing its handled-region plan".as_ptr(),
            );
            return ptr::null_mut();
        };
        if plan.len() != capacity {
            ffi::PyErr_SetString(
                ffi::PyExc_SystemError,
                c"suspended body has a different handled-region capacity".as_ptr(),
            );
            return ptr::null_mut();
        }
        return match unsafe {
            preserved_state::enter_handled_exception_state(preserved.cast(), plan)
        } {
            Ok(state) => state.cast(),
            Err(()) => ptr::null_mut(),
        };
    }
    unsafe {
        HandledExceptionState::initialize_normal(
            state.cast(),
            records.cast::<HandledExceptionRecord>(),
            capacity,
        )
        .cast()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_handled_state_select(
    state: ObjPtr,
    regions: ObjPtr,
    count: i64,
    transition: i64,
) -> i32 {
    let Ok(count) = usize::try_from(count) else {
        ffi::PyErr_SetString(
            ffi::PyExc_SystemError,
            c"invalid handled-region count".as_ptr(),
        );
        return -1;
    };
    let Some(transition) =
        crate::handled_exception::HandledExceptionTransition::from_abi(transition)
    else {
        ffi::PyErr_SetString(
            ffi::PyExc_SystemError,
            c"invalid handled-region transition".as_ptr(),
        );
        return -1;
    };
    let regions = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(regions.cast::<HandledExceptionRegion>(), count) }
    };
    unsafe { HandledExceptionState::select(state.cast(), regions, transition) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_handled_state_raised(state: ObjPtr, scope: i64) {
    unsafe { HandledExceptionState::mark_raised(state.cast(), scope as usize) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_handled_state_finish(
    state: ObjPtr,
    yielded: i64,
    preserved: ObjPtr,
) {
    if yielded == 0 {
        unsafe { crate::managed_generator::notify_terminal(preserved.cast()) };
    }
    unsafe { HandledExceptionState::retire_scopes_and_detach(state.cast(), yielded != 0) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_handled_state_release_residual(state: ObjPtr) {
    unsafe { HandledExceptionState::release_residual(state.cast()) };
}

/// Release completed invocation and suspension ownership, without reconstructing
/// native frames or specifying implicit finalizer timing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_retire_terminal_roots(environment: ObjPtr) -> i32 {
    let Some(header) = (unsafe { environment.cast::<crate::FunctionEnvAbiHeader>().as_ref() })
    else {
        return 0;
    };
    let Some(activation) = (unsafe { header.active_strict_call.as_ref() }) else {
        return 0;
    };
    let preserved = activation.preserved_state();
    if unsafe { activation.retire_terminal_protocol_roots() }.is_err() {
        return -1;
    }
    if !preserved.is_null()
        && unsafe { crate::preserved_state::retire_terminal_protocol_roots(preserved) }.is_err()
    {
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_reraise_current() {
    unsafe { crate::handled_exception::reraise_current() };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_restore_raised_exception(exception: ObjPtr) -> i32 {
    unsafe { crate::handled_exception::restore_raised_exception(exception.cast()) }
}

/// Consume the owned completion value after the generator/coroutine activation
/// and its frame roots have been released. CPython chains the caller's handled
/// exception for None completion, but installs a non-None StopIteration value
/// directly (preserving tuple and exception-object identity).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_generator_return(owned_value: ObjPtr) -> ObjPtr {
    unsafe {
        if owned_value.is_null() {
            if ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_SetString(
                    ffi::PyExc_SystemError,
                    c"generator completion is missing its return value".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        if ffi::PyErr_Occurred().is_null() {
            if owned_value.cast::<ffi::PyObject>() == ffi::Py_None() {
                ffi::PyErr_SetNone(ffi::PyExc_StopIteration);
            } else {
                let exception =
                    ffi::PyObject_CallOneArg(ffi::PyExc_StopIteration, owned_value.cast());
                if !exception.is_null() {
                    ffi::PyErr_SetRaisedException(exception);
                }
            }
        }
        // Allocation failure can make this the value's final reference. Keep
        // the exact pending error while a finalizer runs during its release.
        let error = ffi::PyErr_GetRaisedException();
        ffi::Py_DECREF(owned_value.cast());
        ffi::PyErr_SetRaisedException(error);
        ptr::null_mut()
    }
}

#[cfg(test)]
#[test]
fn generator_return_preserves_native_completion_context_value_and_owned_input() {
    use pyo3::exceptions::{PyMemoryError, PyStopIteration, PyValueError};
    use pyo3::prelude::*;

    unsafe extern "C" {
        fn PyErr_GetHandledException() -> *mut ffi::PyObject;
        fn PyErr_SetHandledException(exception: *mut ffi::PyObject);
    }
    struct RestoreHandled(*mut ffi::PyObject);
    impl Drop for RestoreHandled {
        fn drop(&mut self) {
            unsafe {
                PyErr_SetHandledException(self.0);
                ffi::Py_XDECREF(self.0);
            }
        }
    }

    let _guard = crate::python_runtime_test_lock().lock().unwrap();
    crate::initialize_test_python();
    Python::attach(|py| unsafe {
        let _restore = RestoreHandled(PyErr_GetHandledException());
        let outer = PyValueError::new_err("caller handled exception").into_value(py);
        PyErr_SetHandledException(outer.as_ptr());
        for expression in [
            c"None",
            c"True",
            c"(object(), object())",
            c"ValueError('return value')",
        ] {
            let value = py.eval(expression, None, None).unwrap();
            let before = ffi::Py_REFCNT(value.as_ptr());
            {
                assert!(dp_jit_generator_return(value.clone().into_ptr().cast()).is_null());
                let error = PyErr::fetch(py);
                assert!(error.is_instance_of::<PyStopIteration>(py));
                let exception = error.value(py);
                assert!(exception.getattr("value").unwrap().is(&value));
                let context = exception.getattr("__context__").unwrap();
                if value.is_none() {
                    assert!(context.is(outer.bind(py)));
                    assert_eq!(exception.getattr("args").unwrap().len().unwrap(), 0);
                } else {
                    assert!(context.is_none());
                    let args = exception.getattr("args").unwrap();
                    assert_eq!(args.len().unwrap(), 1);
                    assert!(args.get_item(0).unwrap().is(&value));
                }
                let current = PyErr_GetHandledException();
                assert_eq!(current, outer.as_ptr());
                ffi::Py_XDECREF(current);
            }
            assert_eq!(
                ffi::Py_REFCNT(value.as_ptr()),
                before,
                "completion must consume exactly the input reference"
            );
        }

        let value = py.eval(c"object()", None, None).unwrap();
        let before = ffi::Py_REFCNT(value.as_ptr());
        let memory_error = PyMemoryError::new_err("completion allocation").into_value(py);
        ffi::PyErr_SetRaisedException(memory_error.clone_ref(py).into_ptr());
        assert!(dp_jit_generator_return(value.clone().into_ptr().cast()).is_null());
        let error = PyErr::fetch(py);
        assert!(error.value(py).is(memory_error.bind(py)));
        assert_eq!(
            ffi::Py_REFCNT(value.as_ptr()),
            before,
            "pending-error cleanup must also consume the input reference"
        );
    });
}

#[cfg(test)]
mod test_only_export_stubs {
    use super::ObjPtr;

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
    panic_dual_i32_export!(dp_jit_guard_method_type_version, dp_jit_guard_method_type_version_with_frame(
        receiver: ObjPtr,
        expected_type: ObjPtr,
        expected_version: i64
    ));
    panic_dual_obj_export!(dp_jit_py_call_positional_three, dp_jit_py_call_positional_three_with_frame(
        callable: ObjPtr,
        arg1: ObjPtr,
        arg2: ObjPtr,
        arg3: ObjPtr,
        globals: ObjPtr,
        namespace: ObjPtr,
        builtins: ObjPtr,
    ));
    panic_dual_obj_export!(dp_jit_py_call_object, dp_jit_py_call_object_with_frame(
        callable: ObjPtr,
        args: ObjPtr,
        globals: ObjPtr,
        namespace: ObjPtr,
        builtins: ObjPtr,
    ));
    panic_unit_export!(dp_jit_record_top_value_sample(counter: ObjPtr, value: i64));
    panic_dual_obj_export!(dp_jit_get_arg_item, dp_jit_get_arg_item_with_frame(
        args: ObjPtr,
        index: i64
    ));
    panic_obj_export!(dp_jit_load_runtime_obj(name: ObjPtr));
    panic_obj_export!(dp_jit_load_runtime_obj_by_id(runtime_name_id: i64));
    panic_obj_export!(dp_jit_pyobject_getattr(obj: ObjPtr, attr: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_setattr(obj: ObjPtr, attr: ObjPtr, value: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_getitem(obj: ObjPtr, key: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_setitem(obj: ObjPtr, key: ObjPtr, value: ObjPtr));
    panic_obj_export!(dp_jit_preserved_values_ptr(state: ObjPtr));
    panic_obj_export!(dp_jit_del_preserved(owner: ObjPtr, slot: i64, name: ObjPtr));
    panic_obj_export!(dp_jit_del_preserved_quietly(owner: ObjPtr, slot: i64, name: ObjPtr));
    panic_obj_export!(dp_jit_pyobject_delitem(obj: ObjPtr, key: ObjPtr));
    panic_obj_export!(soac_runtime_load_global_slow(
        globals_obj: ObjPtr,
        builtins_obj: ObjPtr,
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
    panic_unit_export!(dp_jit_raise_unbound_local_error(name: ObjPtr));
    panic_unit_export!(dp_jit_raise_missing_required_argument());
    panic_unit_export!(dp_jit_raise_super_arg_deleted());
    panic_obj_export!(dp_jit_load_cell(cell: ObjPtr, name: ObjPtr, binding_kind: i64));
    panic_obj_export!(dp_jit_store_cell(cell: ObjPtr, value: ObjPtr));
    panic_obj_export!(dp_jit_del_deref(cell: ObjPtr));
    panic_obj_export!(dp_jit_del_deref_quietly(cell: ObjPtr));
    panic_obj_export!(dp_jit_deopt_resume(
        deopt_table: ObjPtr,
        globals_obj: ObjPtr,
        builtins_obj: ObjPtr,
        function_data_obj: ObjPtr,
        record_ordinal: i64,
        live_values: ObjPtr,
        live_value_count: i64,
        handled_state: ObjPtr,
        strict_activation: ObjPtr
    ));
    panic_obj_export!(dp_jit_dict_new());
    panic_i32_export!(dp_jit_dict_set_item(dict_obj: ObjPtr, key: ObjPtr, value: ObjPtr));
    panic_i32_export!(dp_jit_is_true(value: ObjPtr));
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
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $fast($($arg: $ty),*) -> $ret {
            $body
        }

        #[inline(never)]
        #[unsafe(no_mangle)]
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
        callable: ObjPtr,
        arg1: ObjPtr,
        arg2: ObjPtr,
        arg3: ObjPtr,
        globals: ObjPtr,
        namespace: ObjPtr,
        builtins: ObjPtr
    ) => py_call_positional_three_hook(callable, arg1, arg2, arg3, globals, namespace, builtins)
);
define_perf_toggle_export!(
    ObjPtr,
    dp_jit_py_call_object,
    dp_jit_py_call_object_with_frame(
        callable: ObjPtr,
        args: ObjPtr,
        globals: ObjPtr,
        namespace: ObjPtr,
        builtins: ObjPtr
    ) => py_call_object_hook(callable, args, globals, namespace, builtins)
);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_record_top_value_sample(counter: ObjPtr, value: i64) {
    record_top_value_sample_hook(counter, value)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_profile_callable_function_id(callable: ObjPtr) -> i64 {
    profile_callable_function_id_hook(callable)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_protocol_next_function_id(receiver: ObjPtr) -> i64 {
    protocol_next_function_id_hook(receiver)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_protocol_iter_function_id(receiver: ObjPtr) -> i64 {
    protocol_iter_function_id_hook(receiver)
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_load_runtime_obj_by_id(runtime_name_id: i64) -> ObjPtr {
    let Ok(runtime_name_id) = u16::try_from(runtime_name_id) else {
        ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c"invalid runtime name id".as_ptr());
        return ptr::null_mut();
    };
    let Some(runtime_name) = soac_core::block_py::RuntimeName::from_id(runtime_name_id) else {
        ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c"unknown runtime name id".as_ptr());
        return ptr::null_mut();
    };
    load_runtime_name_owned_by_id(runtime_name) as ObjPtr
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_pyobject_getattr(obj: ObjPtr, attr: ObjPtr) -> ObjPtr {
    pyobject_getattr_hook(obj, attr)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_pyobject_setattr(
    obj: ObjPtr,
    attr: ObjPtr,
    value: ObjPtr,
) -> ObjPtr {
    pyobject_setattr_hook(obj, attr, value)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_pyobject_getitem(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    pyobject_getitem_hook(obj, key)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_pyobject_setitem(
    obj: ObjPtr,
    key: ObjPtr,
    value: ObjPtr,
) -> ObjPtr {
    pyobject_setitem_hook(obj, key, value)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_preserved_values_ptr(state: ObjPtr) -> ObjPtr {
    let state = preserved_state_ref(state);
    if state.is_null() {
        return ptr::null_mut();
    }
    unsafe { preserved_state::preserved_values_ptr(state.cast::<ffi::PyObject>()).cast() }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_del_preserved(owner: ObjPtr, slot: i64, name: ObjPtr) -> ObjPtr {
    del_preserved_hook(owner, slot, name, false)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_del_preserved_quietly(
    owner: ObjPtr,
    slot: i64,
    name: ObjPtr,
) -> ObjPtr {
    del_preserved_hook(owner, slot, name, true)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_pytype_generic_alloc(callable: ObjPtr, nitems: i64) -> ObjPtr {
    if callable.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"invalid null type in direct constructor allocation".as_ptr(),
        );
        return ptr::null_mut();
    }
    ffi::PyType_GenericAlloc(
        callable as *mut ffi::PyTypeObject,
        nitems as ffi::Py_ssize_t,
    ) as ObjPtr
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_finish_constructor_init(
    allocated: ObjPtr,
    init_result: ObjPtr,
) -> ObjPtr {
    if allocated.is_null() || init_result.is_null() {
        if !allocated.is_null() {
            ffi::Py_DECREF(allocated as *mut ffi::PyObject);
        }
        return ptr::null_mut();
    }
    let none = ffi::Py_None();
    if ptr::eq(init_result as *mut ffi::PyObject, none) {
        ffi::Py_DECREF(init_result as *mut ffi::PyObject);
        return allocated;
    }
    ffi::Py_DECREF(init_result as *mut ffi::PyObject);
    ffi::Py_DECREF(allocated as *mut ffi::PyObject);
    ffi::PyErr_SetString(
        ffi::PyExc_TypeError,
        c"__init__() should return None".as_ptr(),
    );
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_pyobject_delitem(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    pyobject_delitem_hook(obj, key)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_load_global_slow(
    globals_obj: ObjPtr,
    builtins_obj: ObjPtr,
    name: ObjPtr,
    expected_index: i64,
) -> ObjPtr {
    let name_obj = name as *mut ffi::PyObject;
    if globals_obj.is_null() || builtins_obj.is_null() || name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"invalid arguments to soac_runtime_load_global_slow".as_ptr(),
        );
        return ptr::null_mut();
    }
    let slot_index = guarded_indexed_global_slot(globals_obj, name_obj, expected_index);
    let result = load_global_obj_impl(globals_obj, builtins_obj, name_obj, slot_index);
    ensure_global_load_error(result, name_obj)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_store_global(
    globals_obj: ObjPtr,
    name: ObjPtr,
    slot_index: i64,
    value: ObjPtr,
) -> ObjPtr {
    store_global_hook(globals_obj, name, slot_index, value)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_del_global(
    globals_obj: ObjPtr,
    key: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    del_global_hook(globals_obj, key, slot_index, false)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_del_global_quietly(
    globals_obj: ObjPtr,
    key: ObjPtr,
    slot_index: i64,
) -> ObjPtr {
    del_global_hook(globals_obj, key, slot_index, true)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_del_quietly(obj: ObjPtr, key: ObjPtr) -> ObjPtr {
    del_quietly_hook(obj, key)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_pyobject_to_i64(value: ObjPtr) -> i64 {
    pyobject_to_i64_hook(value)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_make_cell(value: ObjPtr) -> ObjPtr {
    make_cell_hook(value)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_raise_unbound_local_error(name: ObjPtr) {
    raise_unbound_local_error_hook(name)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_raise_missing_required_argument() {
    raise_missing_required_argument_hook()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_raise_super_arg_deleted() {
    raise_super_arg_deleted_hook()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_load_cell(cell: ObjPtr, name: ObjPtr, binding_kind: i64) -> ObjPtr {
    load_cell_hook(cell, name, binding_kind)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_store_cell(cell: ObjPtr, value: ObjPtr) -> ObjPtr {
    store_cell_hook(cell, value)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_del_deref(cell: ObjPtr) -> ObjPtr {
    del_deref_hook(cell)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_del_deref_quietly(cell: ObjPtr) -> ObjPtr {
    del_deref_quietly_hook(cell)
}

#[cold]
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_deopt_resume(
    deopt_table: ObjPtr,
    globals_obj: ObjPtr,
    builtins_obj: ObjPtr,
    function_data_obj: ObjPtr,
    record_ordinal: i64,
    live_values: ObjPtr,
    live_value_count: i64,
    handled_state: ObjPtr,
    strict_activation: ObjPtr,
) -> ObjPtr {
    match unsafe {
        run_deopt_resume(
            deopt_table,
            globals_obj,
            builtins_obj,
            function_data_obj,
            record_ordinal,
            live_values,
            live_value_count,
            handled_state,
            strict_activation,
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
    builtins_obj: ObjPtr,
    function_data_obj: ObjPtr,
    record_ordinal: i64,
    live_values: ObjPtr,
    live_value_count: i64,
    handled_state: ObjPtr,
    strict_activation: ObjPtr,
) -> Result<ObjPtr, String> {
    let invocation = unsafe {
        RuntimeJitDeoptInvocation::from_raw(
            deopt_table,
            globals_obj,
            builtins_obj,
            function_data_obj,
            record_ordinal,
            live_values,
            live_value_count,
            handled_state.cast(),
            strict_activation.cast(),
        )?
    };
    // This is a diagnostic of the actual cold handoff, after validating its
    // immutable table ordinal and incoming buffer shape. It is not a hot-path
    // guard or evidence used to select an optimization in this process.
    invocation.record_native_entry();
    super::deopt_interpreter::execute_deopt_invocation(&invocation)
}

#[cold]
fn set_deopt_unsupported_continuation_error(detail: String) {
    if unsafe { !ffi::PyErr_Occurred().is_null() } {
        // In particular, a NULL boxed scalar can arrive with MemoryError.
        // The diagnostic describes admission failure, not a replacement for
        // the original Python error after the incoming owners are released.
        return;
    }
    let message = format!("JIT deopt helper is not implemented: {detail}");
    if let Ok(c_message) = std::ffi::CString::new(message) {
        unsafe {
            ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_message.as_ptr());
        }
    } else {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"JIT deopt helper is not implemented\0".as_ptr().cast(),
            );
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_dict_new() -> ObjPtr {
    dict_new_hook()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_dict_set_item(dict_obj: ObjPtr, key: ObjPtr, value: ObjPtr) -> i32 {
    dict_set_item_hook(dict_obj, key, value)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_is_true(value: ObjPtr) -> i32 {
    is_true_hook(value)
}
unsafe fn load_python_capi_symbol(name: &'static [u8]) -> usize {
    libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr().cast()) as usize
}

fn python_capi_symbol(name: &'static [u8]) -> *const u8 {
    let symbol = unsafe { load_python_capi_symbol(name) };
    if symbol == 0 {
        let symbol_name = CStr::from_bytes_with_nul(name)
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|_| "<invalid symbol name>".into());
        panic!("CPython C-API symbol is not visible to the JIT: {symbol_name}");
    }
    symbol as *const u8
}

unsafe fn raise_i64_overflow_error() {
    // BEHAVIOR_CHANGE: optimized SOAC exact-int arithmetic intentionally raises
    // on i64 overflow instead of falling back to CPython's arbitrary-precision int.
    ffi::PyErr_SetString(
        ffi::PyExc_OverflowError,
        c"SOAC optimized integer arithmetic overflowed i64".as_ptr(),
    );
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_raise_i64_overflow() {
    raise_i64_overflow_error();
}

fn chosen_helper_symbol(fast: *const u8, with_frame: *const u8) -> *const u8 {
    if cfg!(test) {
        return fast;
    }
    if soac_config::SoacEnvConfig::from_env()
        .map(|config| config.jit_perf_helper_frames_enabled())
        .unwrap_or_else(|err| panic!("invalid SOAC_JIT_PERF_HELPER_FRAMES config: {err}"))
    {
        with_frame
    } else {
        fast
    }
}

pub fn register_specialized_jit_symbols(builder: &mut JITBuilder) {
    super::native_iterator_runtime::register_symbols(builder);
    super::collection_runtime::register_symbols(builder);
    super::iteration_runtime::register_symbols(builder);
    super::call_arguments_runtime::register_symbols(builder);
    builder.symbol(
        "dp_jit_unpack_fixed_slow",
        dp_jit_unpack_fixed_slow as *const u8,
    );
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
        "dp_jit_match_sealed_field_capability",
        crate::strict_class_state::dp_jit_match_sealed_field_capability as *const u8,
    );
    builder.symbol(
        "dp_jit_resolve_sealed_virtual_method_capability",
        crate::strict_class_state::dp_jit_resolve_sealed_virtual_method_capability as *const u8,
    );
    builder.symbol(
        "dp_jit_make_generator_instance_from_vectorcall",
        crate::generator_factory_vectorcall as *const u8,
    );
    builder.symbol(
        "dp_jit_enter_recursive_call",
        enter_recursive_call_hook as *const u8,
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
        "dp_jit_load_runtime_obj_by_id",
        dp_jit_load_runtime_obj_by_id as *const u8,
    );
    builder.symbol(
        "dp_jit_checked_function_metadata",
        crate::checked_function_metadata as *const u8,
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
        "dp_jit_strict_finish_call",
        crate::strict_function::strict_finish_call as *const u8,
    );
    builder.symbol(
        "dp_jit_prepare_strict_direct_call",
        crate::strict_call::dp_jit_prepare_strict_direct_call as *const u8,
    );
    builder.symbol(
        "dp_jit_finish_strict_direct_call",
        crate::strict_call::dp_jit_finish_strict_direct_call as *const u8,
    );
    builder.symbol(
        "dp_jit_retire_strict_call_arguments",
        crate::strict_call::dp_jit_retire_strict_call_arguments as *const u8,
    );
    builder.symbol(
        "dp_jit_vectorcall_previous_for_changed_code",
        crate::vectorcall_previous_for_changed_code as *const u8,
    );
    builder.symbol(
        "dp_jit_direct_compile_function_env",
        crate::direct_compile_function_env as *const u8,
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
        "dp_jit_preserved_values_ptr",
        dp_jit_preserved_values_ptr as *const u8,
    );
    builder.symbol("dp_jit_del_preserved", dp_jit_del_preserved as *const u8);
    builder.symbol(
        "dp_jit_del_preserved_quietly",
        dp_jit_del_preserved_quietly as *const u8,
    );
    builder.symbol(
        "dp_jit_pytype_generic_alloc",
        dp_jit_pytype_generic_alloc as *const u8,
    );
    builder.symbol(
        "dp_jit_finish_constructor_init",
        dp_jit_finish_constructor_init as *const u8,
    );
    builder.symbol(
        "dp_jit_pyobject_delitem",
        dp_jit_pyobject_delitem as *const u8,
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
        "dp_jit_profile_callable_function_id",
        dp_jit_profile_callable_function_id as *const u8,
    );
    builder.symbol(
        "dp_jit_protocol_next_function_id",
        dp_jit_protocol_next_function_id as *const u8,
    );
    builder.symbol(
        "dp_jit_protocol_iter_function_id",
        dp_jit_protocol_iter_function_id as *const u8,
    );
    builder.symbol(
        "dp_jit_raise_unbound_local_error",
        dp_jit_raise_unbound_local_error as *const u8,
    );
    builder.symbol(
        "dp_jit_raise_super_arg_deleted",
        dp_jit_raise_super_arg_deleted as *const u8,
    );
    builder.symbol(
        "dp_jit_raise_missing_required_argument",
        dp_jit_raise_missing_required_argument as *const u8,
    );
    builder.symbol(
        "dp_jit_raise_super_arg_deleted",
        dp_jit_raise_super_arg_deleted as *const u8,
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
        "dp_jit_handled_state_init",
        dp_jit_handled_state_init as *const u8,
    );
    builder.symbol(
        "dp_jit_handled_state_select",
        dp_jit_handled_state_select as *const u8,
    );
    builder.symbol(
        "dp_jit_handled_state_raised",
        dp_jit_handled_state_raised as *const u8,
    );
    builder.symbol(
        "dp_jit_handled_state_finish",
        dp_jit_handled_state_finish as *const u8,
    );
    builder.symbol(
        "dp_jit_handled_state_release_residual",
        dp_jit_handled_state_release_residual as *const u8,
    );
    builder.symbol(
        "dp_jit_retire_terminal_roots",
        dp_jit_retire_terminal_roots as *const u8,
    );
    builder.symbol(
        "dp_jit_reraise_current",
        dp_jit_reraise_current as *const u8,
    );
    builder.symbol(
        "dp_jit_restore_raised_exception",
        dp_jit_restore_raised_exception as *const u8,
    );
    builder.symbol(
        "dp_jit_generator_return",
        dp_jit_generator_return as *const u8,
    );
    builder.symbol(
        "PyObject_RichCompare",
        python_capi_symbol(b"PyObject_RichCompare\0"),
    );
    builder.symbol(
        "PyUnicode_Compare",
        python_capi_symbol(b"PyUnicode_Compare\0"),
    );
    builder.symbol(
        "PySequence_Contains",
        python_capi_symbol(b"PySequence_Contains\0"),
    );
    builder.symbol(
        "PyLong_FromLongLong",
        python_capi_symbol(b"PyLong_FromLongLong\0"),
    );
    builder.symbol("PyObject_Not", python_capi_symbol(b"PyObject_Not\0"));
    builder.symbol("PyObject_IsTrue", python_capi_symbol(b"PyObject_IsTrue\0"));
    builder.symbol("PyNumber_Add", python_capi_symbol(b"PyNumber_Add\0"));
    builder.symbol(
        "PyNumber_Subtract",
        python_capi_symbol(b"PyNumber_Subtract\0"),
    );
    builder.symbol(
        "PyNumber_Multiply",
        python_capi_symbol(b"PyNumber_Multiply\0"),
    );
    builder.symbol(
        "PyNumber_MatrixMultiply",
        python_capi_symbol(b"PyNumber_MatrixMultiply\0"),
    );
    builder.symbol(
        "PyNumber_TrueDivide",
        python_capi_symbol(b"PyNumber_TrueDivide\0"),
    );
    builder.symbol(
        "PyNumber_FloorDivide",
        python_capi_symbol(b"PyNumber_FloorDivide\0"),
    );
    builder.symbol(
        "PyNumber_Remainder",
        python_capi_symbol(b"PyNumber_Remainder\0"),
    );
    builder.symbol("PyNumber_Power", python_capi_symbol(b"PyNumber_Power\0"));
    builder.symbol("PyNumber_Lshift", python_capi_symbol(b"PyNumber_Lshift\0"));
    builder.symbol("PyNumber_Rshift", python_capi_symbol(b"PyNumber_Rshift\0"));
    builder.symbol("PyNumber_Or", python_capi_symbol(b"PyNumber_Or\0"));
    builder.symbol("PyNumber_Xor", python_capi_symbol(b"PyNumber_Xor\0"));
    builder.symbol("PyNumber_And", python_capi_symbol(b"PyNumber_And\0"));
    builder.symbol(
        "PyNumber_InPlaceAdd",
        python_capi_symbol(b"PyNumber_InPlaceAdd\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceSubtract",
        python_capi_symbol(b"PyNumber_InPlaceSubtract\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceMultiply",
        python_capi_symbol(b"PyNumber_InPlaceMultiply\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceMatrixMultiply",
        python_capi_symbol(b"PyNumber_InPlaceMatrixMultiply\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceTrueDivide",
        python_capi_symbol(b"PyNumber_InPlaceTrueDivide\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceFloorDivide",
        python_capi_symbol(b"PyNumber_InPlaceFloorDivide\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceRemainder",
        python_capi_symbol(b"PyNumber_InPlaceRemainder\0"),
    );
    builder.symbol(
        "PyNumber_InPlacePower",
        python_capi_symbol(b"PyNumber_InPlacePower\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceLshift",
        python_capi_symbol(b"PyNumber_InPlaceLshift\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceRshift",
        python_capi_symbol(b"PyNumber_InPlaceRshift\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceOr",
        python_capi_symbol(b"PyNumber_InPlaceOr\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceXor",
        python_capi_symbol(b"PyNumber_InPlaceXor\0"),
    );
    builder.symbol(
        "PyNumber_InPlaceAnd",
        python_capi_symbol(b"PyNumber_InPlaceAnd\0"),
    );
    builder.symbol(
        "PyNumber_Positive",
        python_capi_symbol(b"PyNumber_Positive\0"),
    );
    builder.symbol(
        "PyNumber_Negative",
        python_capi_symbol(b"PyNumber_Negative\0"),
    );
    builder.symbol("PyNumber_Invert", python_capi_symbol(b"PyNumber_Invert\0"));
}
