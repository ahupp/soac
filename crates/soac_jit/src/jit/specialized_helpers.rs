#![cfg_attr(test, allow(dead_code, unused_imports))]

use super::RuntimeJitDeoptInvocation;
use crate::module_constants::raise_name_error_for_missing_name;
use crate::module_constants::{load_runtime_name_owned, load_runtime_name_owned_by_id};
use crate::preserved_state;
use cranelift_jit::JITBuilder;
use libc;
use pyo3::ffi;
use std::ffi::{CStr, c_char, c_void};
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

#[repr(C)]
struct RawPyRangeIterObject {
    ob_base: ffi::PyObject,
    start: libc::c_long,
    step: libc::c_long,
    len: libc::c_long,
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
    tstate: ObjPtr,
    callable: ObjPtr,
    arg1: ObjPtr,
    arg2: ObjPtr,
    arg3: ObjPtr,
) -> ObjPtr {
    if tstate.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"invalid null tstate in dp_jit_py_call_positional_three\0"
                .as_ptr()
                .cast(),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuardedGeneratorBuiltin {
    Any,
    All,
}

unsafe fn guarded_generator_builtin_kind(
    callable: ObjPtr,
    args: ObjPtr,
    nargsf: ObjPtr,
    kwnames: ObjPtr,
) -> Option<GuardedGeneratorBuiltin> {
    if !kwnames.is_null()
        || args.is_null()
        || callable.is_null()
        || ffi::PyVectorcall_NARGS(nargsf as usize) != 1
    {
        return None;
    }

    let callable = callable.cast::<ffi::PyObject>();
    if (*callable).ob_type != ptr::addr_of_mut!(ffi::PyCFunction_Type) {
        return None;
    }
    let function = &*callable.cast::<ffi::PyCFunctionObject>();
    if function.m_ml.is_null()
        || (*function.m_ml).ml_name.is_null()
        || *(*function.m_ml).ml_name != b'a'
        || (*function.m_ml).ml_flags != ffi::METH_O
    {
        return None;
    }
    match CStr::from_ptr((*function.m_ml).ml_name).to_bytes() {
        b"any" => Some(GuardedGeneratorBuiltin::Any),
        b"all" => Some(GuardedGeneratorBuiltin::All),
        _ => None,
    }
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
            b"invalid null tstate in dp_jit_py_vectorcall\0"
                .as_ptr()
                .cast(),
        );
        return ptr::null_mut();
    }
    if kwnames.is_null() && !args.is_null() {
        let nargs = ffi::PyVectorcall_NARGS(nargsf as usize);
        if nargs == 1 || nargs == 2 {
            if let Some(result) = fast_builtin_next_range_iter(callable, args, nargsf, kwnames) {
                return result;
            }
            if nargs == 2 {
                if let Some(result) =
                    fast_runtime_stop_iteration_match(callable, args, nargsf, kwnames)
                {
                    return result;
                }
            } else if let Some(kind) =
                guarded_generator_builtin_kind(callable, args, nargsf, kwnames)
                && let Some(result) =
                    fast_guarded_generator_builtin_consumption(callable, args, kind)
            {
                return result;
            }
        }
    }
    ffi::_PyObject_VectorcallTstate(
        tstate as *mut ffi::PyThreadState,
        callable as *mut ffi::PyObject,
        args as *const *mut ffi::PyObject,
        nargsf as usize,
        kwnames as *mut ffi::PyObject,
    ) as ObjPtr
}

#[cfg(test)]
#[test]
fn canonical_generator_consumers_use_the_actual_vectorcall_dispatch() {
    use pyo3::prelude::*;

    let guard = crate::python_runtime_test_lock().lock().unwrap();
    crate::initialize_test_python();
    let selections = Python::attach(|py| unsafe {
        let builtins = py.import("builtins").expect("builtins should import");
        let any = builtins.getattr("any").expect("builtin any should exist");
        let all = builtins.getattr("all").expect("builtin all should exist");
        let len = builtins.getattr("len").expect("builtin len should exist");
        let any_values = py
            .eval(c"(value for value in (0, 1))", None, None)
            .expect("an actual Python generator should be created");
        let all_values = py
            .eval(c"(value for value in (1, 0))", None, None)
            .expect("another actual Python generator should be created");
        let tstate = ffi::PyThreadState_Get().cast::<c_void>();
        assert!(
            !tstate.is_null(),
            "an attached test thread should have state"
        );

        let any_args = [any_values.as_ptr()];
        let any_result = dp_jit_py_vectorcall(
            tstate,
            any.as_ptr().cast(),
            any_args.as_ptr().cast::<c_void>().cast_mut(),
            1usize as ObjPtr,
            ptr::null_mut(),
        );
        assert_eq!(any_result, ffi::Py_True().cast());
        ffi::Py_DECREF(any_result.cast());

        let all_args = [all_values.as_ptr()];
        let all_result = dp_jit_py_vectorcall(
            tstate,
            all.as_ptr().cast(),
            all_args.as_ptr().cast::<c_void>().cast_mut(),
            1usize as ObjPtr,
            ptr::null_mut(),
        );
        assert_eq!(all_result, ffi::Py_False().cast());
        ffi::Py_DECREF(all_result.cast());

        let any_selected = guarded_generator_builtin_kind(
            any.as_ptr().cast(),
            any_args.as_ptr().cast::<c_void>().cast_mut(),
            1usize as ObjPtr,
            ptr::null_mut(),
        );
        let all_selected = guarded_generator_builtin_kind(
            all.as_ptr().cast(),
            all_args.as_ptr().cast::<c_void>().cast_mut(),
            1usize as ObjPtr,
            ptr::null_mut(),
        );
        assert_eq!(
            guarded_generator_builtin_kind(
                len.as_ptr().cast(),
                any_args.as_ptr().cast::<c_void>().cast_mut(),
                1usize as ObjPtr,
                ptr::null_mut(),
            ),
            None,
            "unrelated builtin calls must never enter generator consumption"
        );
        assert_eq!(
            guarded_generator_builtin_kind(
                any.as_ptr().cast(),
                any_args.as_ptr().cast::<c_void>().cast_mut(),
                2usize as ObjPtr,
                ptr::null_mut(),
            ),
            None,
            "wrong-arity builtin calls must retain ordinary vectorcall errors"
        );
        (any_selected, all_selected)
    });
    drop(guard);

    assert_eq!(
        selections,
        (
            Some(GuardedGeneratorBuiltin::Any),
            Some(GuardedGeneratorBuiltin::All),
        ),
        "the production vectorcall hook must recognize canonical any/all without changing real generator results"
    );
}

unsafe fn cached_builtin_next() -> *mut ffi::PyObject {
    static BUILTIN_NEXT: OnceLock<usize> = OnceLock::new();
    *BUILTIN_NEXT.get_or_init(|| {
        let builtins = ffi::PyEval_GetBuiltins();
        if builtins.is_null() {
            return 0;
        }
        let next = ffi::PyDict_GetItemString(builtins, c"next".as_ptr());
        if next.is_null() {
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
            }
            return 0;
        }
        ffi::Py_INCREF(next);
        next as usize
    }) as *mut ffi::PyObject
}

unsafe fn fast_builtin_next_range_iter(
    callable: ObjPtr,
    args: ObjPtr,
    nargsf: ObjPtr,
    kwnames: ObjPtr,
) -> Option<ObjPtr> {
    if !kwnames.is_null() || args.is_null() {
        return None;
    }
    let nargs = ffi::PyVectorcall_NARGS(nargsf as usize);
    if !(nargs == 1 || nargs == 2) {
        return None;
    }
    let next = cached_builtin_next();
    if next.is_null() || callable as *mut ffi::PyObject != next {
        return None;
    }
    let args = args as *const *mut ffi::PyObject;
    let iter = *args;
    if iter.is_null() || ffi::Py_TYPE(iter) != std::ptr::addr_of_mut!(ffi::PyRangeIter_Type) {
        return None;
    }

    let range_iter = iter as *mut RawPyRangeIterObject;
    if (*range_iter).len <= 0 {
        if nargs == 2 {
            let default = *args.add(1);
            ffi::Py_INCREF(default);
            return Some(default as ObjPtr);
        }
        ffi::PyErr_SetNone(ffi::PyExc_StopIteration);
        return Some(ptr::null_mut());
    }
    let result = (*range_iter).start;
    (*range_iter).start = result + (*range_iter).step;
    (*range_iter).len -= 1;
    Some(ffi::PyLong_FromLong(result) as ObjPtr)
}

#[repr(C)]
struct RawPyDictIndexedValues {
    capacity: ffi::Py_ssize_t,
    order_size: ffi::Py_ssize_t,
    values: [*mut ffi::PyObject; 1],
}

unsafe fn stop_iteration_unicode_entries(
    keys: *mut super::RawPyDictKeysObjectForJit,
) -> *mut super::RawPyDictUnicodeEntryForJit {
    keys.cast::<u8>()
        .add(
            std::mem::size_of::<super::RawPyDictKeysObjectForJit>()
                + (1usize << (*keys).dk_log2_index_bytes),
        )
        .cast()
}

unsafe fn stop_iteration_indexed_value(
    values: *mut RawPyDictIndexedValues,
    index: usize,
) -> *mut ffi::PyObject {
    *(&raw const (*values).values)
        .cast::<*mut ffi::PyObject>()
        .add(index)
}

unsafe fn prepare_stop_iteration_runtime_entry(
    data: &crate::PyFunctionJitExtra,
    keys: *mut super::RawPyDictKeysObjectForJit,
    values: *mut RawPyDictIndexedValues,
    name: &'static CStr,
) -> Option<crate::PreparedStopIterationDictionaryEntry> {
    let name_str = name.to_str().ok()?;
    let index = data
        .module_state
        .lowered_module
        .global_names
        .iter()
        .position(|global| global == name_str)?;
    if index >= (*keys).dk_nentries as usize || index >= (*values).capacity as usize {
        return None;
    }
    let entry = &*stop_iteration_unicode_entries(keys).add(index);
    if entry.me_key.is_null()
        || ffi::PyUnicode_CheckExact(entry.me_key) == 0
        || ffi::PyUnicode_CompareWithASCIIString(entry.me_key, name.as_ptr()) != 0
    {
        return None;
    }
    let value = stop_iteration_indexed_value(values, index);
    let value = if value.is_null()
        || value.cast::<i8>() == ptr::addr_of_mut!(super::_PyDict_IndexedValueTombstone)
    {
        0
    } else {
        value as usize
    };
    Some(crate::PreparedStopIterationDictionaryEntry {
        index,
        key: entry.me_key as usize,
        value,
    })
}

unsafe fn prepare_stop_iteration_builtin_entry(
    keys: *mut super::RawPyDictKeysObjectForJit,
    name: &'static CStr,
) -> Option<crate::PreparedStopIterationDictionaryEntry> {
    for index in 0..(*keys).dk_nentries as usize {
        let entry = &*stop_iteration_unicode_entries(keys).add(index);
        if entry.me_key.is_null()
            || ffi::PyUnicode_CheckExact(entry.me_key) == 0
            || ffi::PyUnicode_CompareWithASCIIString(entry.me_key, name.as_ptr()) != 0
        {
            continue;
        }
        if entry.me_value.is_null() {
            return None;
        }
        return Some(crate::PreparedStopIterationDictionaryEntry {
            index,
            key: entry.me_key as usize,
            value: entry.me_value as usize,
        });
    }
    None
}

unsafe fn stop_iteration_exact_builtin(
    function: *mut ffi::PyObject,
    name: &'static CStr,
    builtins: *mut ffi::PyObject,
) -> bool {
    if function.is_null() || ffi::PyCFunction_CheckExact(function) == 0 {
        return false;
    }
    let function = &*function.cast::<ffi::PyCFunctionObject>();
    if function.m_ml.is_null()
        || (*function.m_ml).ml_name.is_null()
        || CStr::from_ptr((*function.m_ml).ml_name) != name
        || function.m_self.is_null()
        || ffi::PyModule_CheckExact(function.m_self) == 0
    {
        return false;
    }
    ffi::PyModule_GetDict(function.m_self) == builtins
}

unsafe fn prepare_stop_iteration_matcher(
    helper: *mut ffi::PyObject,
    data: &crate::PyFunctionJitExtra,
) -> Option<crate::PreparedStopIterationMatcher> {
    if data.module_state.module_name != "soac.runtime"
        || data.function_template.function().names.qualname != "exception_matches"
        || crate::PyFunction_GetSoacFunctionId(helper) != data.function_id.to_packed_runtime_u64()
    {
        return None;
    }
    let helper_function = &*helper.cast::<ffi::PyFunctionObject>();
    let helper_code = helper_function.func_code;
    if helper_code != data.registered_code
        || data
            .module_state
            .lookup_original_code(data.function_id)
            .map(pyo3::Py::as_ptr)
            != Some(helper_code)
        || super::raw_py_function_activation_is_observed(helper_code)
    {
        return None;
    }

    let globals = helper_function.func_globals;
    let builtins = helper_function.func_builtins;
    if globals.is_null()
        || builtins.is_null()
        || globals != data.function_env.globals_obj()
        || builtins != data.function_env.builtins_obj()
        || ffi::PyDict_CheckExact(globals) == 0
        || ffi::PyDict_CheckExact(builtins) == 0
    {
        return None;
    }
    let globals_dict = &*globals.cast::<ffi::PyDictObject>();
    let runtime_keys = globals_dict
        .ma_keys
        .cast::<super::RawPyDictKeysObjectForJit>();
    let runtime_values = globals_dict.ma_values.cast::<RawPyDictIndexedValues>();
    if runtime_keys.is_null()
        || runtime_values.is_null()
        || (*runtime_keys).dk_kind != 3
        || (*runtime_keys).dk_nentries < 0
        || (*runtime_values).capacity < 0
    {
        return None;
    }

    let builtin_dict = &*builtins.cast::<ffi::PyDictObject>();
    let builtin_keys = builtin_dict
        .ma_keys
        .cast::<super::RawPyDictKeysObjectForJit>();
    if builtin_keys.is_null()
        || !builtin_dict.ma_values.is_null()
        || (*builtin_keys).dk_kind != 1
        || (*builtin_keys).dk_nentries < 0
    {
        return None;
    }

    let runtime_entries = [
        prepare_stop_iteration_runtime_entry(
            data,
            runtime_keys,
            runtime_values,
            c"_validate_exception_type",
        )?,
        prepare_stop_iteration_runtime_entry(data, runtime_keys, runtime_values, c"isinstance")?,
        prepare_stop_iteration_runtime_entry(data, runtime_keys, runtime_values, c"tuple")?,
        prepare_stop_iteration_runtime_entry(data, runtime_keys, runtime_values, c"type")?,
        prepare_stop_iteration_runtime_entry(data, runtime_keys, runtime_values, c"issubclass")?,
        prepare_stop_iteration_runtime_entry(data, runtime_keys, runtime_values, c"BaseException")?,
        prepare_stop_iteration_runtime_entry(
            data,
            runtime_keys,
            runtime_values,
            c"RecursionError",
        )?,
    ];
    let builtin_entries = [
        prepare_stop_iteration_builtin_entry(builtin_keys, c"isinstance")?,
        prepare_stop_iteration_builtin_entry(builtin_keys, c"issubclass")?,
        prepare_stop_iteration_builtin_entry(builtin_keys, c"BaseException")?,
        prepare_stop_iteration_builtin_entry(builtin_keys, c"RecursionError")?,
    ];
    if runtime_entries[1].value != builtin_entries[0].value
        || !stop_iteration_exact_builtin(
            runtime_entries[1].value as *mut ffi::PyObject,
            c"isinstance",
            builtins,
        )
        || runtime_entries[2].value
            != ptr::addr_of_mut!(ffi::PyTuple_Type).cast::<ffi::PyObject>() as usize
        || runtime_entries[3].value
            != ptr::addr_of_mut!(ffi::PyType_Type).cast::<ffi::PyObject>() as usize
        || runtime_entries[4].value != 0
        || runtime_entries[5].value != 0
        || runtime_entries[6].value != 0
        || !stop_iteration_exact_builtin(
            builtin_entries[1].value as *mut ffi::PyObject,
            c"issubclass",
            builtins,
        )
        || builtin_entries[2].value != ffi::PyExc_BaseException as usize
        || builtin_entries[3].value != ffi::PyExc_RecursionError as usize
    {
        return None;
    }

    let validator = runtime_entries[0].value as *mut ffi::PyObject;
    if validator.is_null() || ffi::PyFunction_Check(validator) == 0 {
        return None;
    }
    let validator_metadata = crate::PyFunction_GetSoacMetadata(validator);
    if validator_metadata.is_null() {
        return None;
    }
    let validator_data = &*validator_metadata.cast::<crate::PyFunctionJitExtra>();
    let validator_function = &*validator.cast::<ffi::PyFunctionObject>();
    if validator_data.compile_session.id() != data.compile_session.id()
        || validator_data.module_state.module_name != "soac.runtime"
        || validator_data.function_template.function().names.qualname != "_validate_exception_type"
        || crate::PyFunction_GetSoacFunctionId(validator)
            != validator_data.function_id.to_packed_runtime_u64()
        || validator_function.func_globals != globals
        || validator_function.func_builtins != builtins
        || validator_function.func_code != validator_data.registered_code
        || validator_data
            .module_state
            .lookup_original_code(validator_data.function_id)
            .map(pyo3::Py::as_ptr)
            != Some(validator_function.func_code)
        || super::raw_py_function_activation_is_observed(validator_function.func_code)
    {
        return None;
    }

    Some(crate::PreparedStopIterationMatcher {
        compile_session_id: data.compile_session.id(),
        helper_function_id: data.function_id,
        helper: helper as usize,
        helper_code: helper_code as usize,
        validator_function_id: validator_data.function_id,
        validator: validator as usize,
        validator_code: validator_function.func_code as usize,
        runtime_globals: globals as usize,
        runtime_keys: runtime_keys as usize,
        runtime_values: runtime_values as usize,
        builtins: builtins as usize,
        builtin_keys: builtin_keys as usize,
        runtime_entries,
        builtin_entries,
    })
}

unsafe fn stop_iteration_matcher_still_canonical(
    prepared: &crate::PreparedStopIterationMatcher,
    helper: *mut ffi::PyObject,
    data: &crate::PyFunctionJitExtra,
) -> bool {
    if helper as usize != prepared.helper
        || data.compile_session.id() != prepared.compile_session_id
        || crate::PyFunction_GetSoacFunctionId(helper)
            != prepared.helper_function_id.to_packed_runtime_u64()
        || (*helper.cast::<ffi::PyFunctionObject>()).func_code as usize != prepared.helper_code
        || (*helper.cast::<ffi::PyFunctionObject>()).func_globals as usize
            != prepared.runtime_globals
        || (*helper.cast::<ffi::PyFunctionObject>()).func_builtins as usize != prepared.builtins
        || super::raw_py_function_activation_is_observed(prepared.helper_code as *mut ffi::PyObject)
    {
        return false;
    }

    let globals = &*(prepared.runtime_globals as *mut ffi::PyDictObject);
    let runtime_keys = globals.ma_keys.cast::<super::RawPyDictKeysObjectForJit>();
    let runtime_values = globals.ma_values.cast::<RawPyDictIndexedValues>();
    if runtime_keys as usize != prepared.runtime_keys
        || runtime_values as usize != prepared.runtime_values
        || (*runtime_keys).dk_kind != 3
        || (*runtime_keys).dk_nentries < 0
        || (*runtime_values).capacity < 0
    {
        return false;
    }
    if !stop_iteration_runtime_entries_still_match(
        runtime_keys,
        runtime_values,
        &prepared.runtime_entries,
    ) {
        return false;
    }

    let builtins = &*(prepared.builtins as *mut ffi::PyDictObject);
    let builtin_keys = builtins.ma_keys.cast::<super::RawPyDictKeysObjectForJit>();
    if builtin_keys as usize != prepared.builtin_keys
        || !builtins.ma_values.is_null()
        || (*builtin_keys).dk_kind != 1
        || (*builtin_keys).dk_nentries < 0
        || !stop_iteration_builtin_entries_still_match(builtin_keys, &prepared.builtin_entries)
    {
        return false;
    }

    let validator = prepared.validator as *mut ffi::PyObject;
    ffi::PyFunction_Check(validator) != 0
        && crate::PyFunction_GetSoacFunctionId(validator)
            == prepared.validator_function_id.to_packed_runtime_u64()
        && (*validator.cast::<ffi::PyFunctionObject>()).func_code as usize
            == prepared.validator_code
        && (*validator.cast::<ffi::PyFunctionObject>()).func_globals as usize
            == prepared.runtime_globals
        && (*validator.cast::<ffi::PyFunctionObject>()).func_builtins as usize == prepared.builtins
        && !super::raw_py_function_activation_is_observed(
            prepared.validator_code as *mut ffi::PyObject,
        )
}

unsafe fn stop_iteration_runtime_entries_still_match(
    runtime_keys: *mut super::RawPyDictKeysObjectForJit,
    runtime_values: *mut RawPyDictIndexedValues,
    entries: &[crate::PreparedStopIterationDictionaryEntry],
) -> bool {
    let runtime_key_entries = stop_iteration_unicode_entries(runtime_keys);
    for expected in entries {
        if expected.index >= (*runtime_keys).dk_nentries as usize
            || expected.index >= (*runtime_values).capacity as usize
            || (*runtime_key_entries.add(expected.index)).me_key as usize != expected.key
        {
            return false;
        }
        let value = stop_iteration_indexed_value(runtime_values, expected.index);
        if expected.value == 0 {
            if !value.is_null()
                && value.cast::<i8>() != ptr::addr_of_mut!(super::_PyDict_IndexedValueTombstone)
            {
                return false;
            }
        } else if value as usize != expected.value {
            return false;
        }
    }
    true
}

unsafe fn stop_iteration_builtin_entries_still_match(
    builtin_keys: *mut super::RawPyDictKeysObjectForJit,
    entries: &[crate::PreparedStopIterationDictionaryEntry],
) -> bool {
    let builtin_key_entries = stop_iteration_unicode_entries(builtin_keys);
    for expected in entries {
        if expected.index >= (*builtin_keys).dk_nentries as usize {
            return false;
        }
        let entry = &*builtin_key_entries.add(expected.index);
        if entry.me_key as usize != expected.key || entry.me_value as usize != expected.value {
            return false;
        }
    }
    true
}

unsafe fn fast_runtime_stop_iteration_match(
    callable: ObjPtr,
    args: ObjPtr,
    nargsf: ObjPtr,
    kwnames: ObjPtr,
) -> Option<ObjPtr> {
    if !kwnames.is_null() || args.is_null() {
        return None;
    }
    let nargs = ffi::PyVectorcall_NARGS(nargsf as usize);
    if nargs != 2 {
        return None;
    }
    let args = args as *const *mut ffi::PyObject;
    let exc = *args;
    let exc_type = *args.add(1);
    if exc.is_null()
        || exc_type != ffi::PyExc_StopIteration
        || ffi::Py_TYPE(exc) != ffi::PyExc_StopIteration.cast::<ffi::PyTypeObject>()
    {
        // Even a real StopIteration subclass can override __class__, observed by the
        // helper's initial isinstance(exc, RecursionError) check.
        return None;
    }

    let helper = callable.cast::<ffi::PyObject>();
    if helper.is_null() || ffi::PyFunction_Check(helper) == 0 {
        return None;
    }
    let metadata = crate::PyFunction_GetSoacMetadata(helper);
    if metadata.is_null() {
        return None;
    }
    let data = &*metadata.cast::<crate::PyFunctionJitExtra>();
    let prepared = match data.function_template.prepared_stop_iteration_matcher.get() {
        Some(prepared) => prepared,
        None => {
            let prepared = prepare_stop_iteration_matcher(helper, data)?;
            let _ = data
                .function_template
                .prepared_stop_iteration_matcher
                .set(prepared);
            data.function_template
                .prepared_stop_iteration_matcher
                .get()?
        }
    };
    if !stop_iteration_matcher_still_canonical(prepared, helper, data) {
        return None;
    }
    Some(ffi::PyBool_FromLong(1) as ObjPtr)
}

const GENERATOR_RUNTIME_CLASS: usize = 0;
const GENERATOR_RUNTIME_CLOSED: usize = 1;
const GENERATOR_RUNTIME_RERAISE: usize = 2;
const GENERATOR_RUNTIME_RESUME: usize = 3;
const GENERATOR_RUNTIME_LOAD_STATE: usize = 4;
const GENERATOR_RUNTIME_NO_DEFAULT: usize = 5;
const GENERATOR_RUNTIME_BOOL: usize = 6;
const GENERATOR_RUNTIME_BASE_EXCEPTION: usize = 7;
const GENERATOR_RUNTIME_STOP_ITERATION: usize = 8;

unsafe fn prepare_generator_consumer_method(
    data: &crate::PyFunctionJitExtra,
    function: *mut ffi::PyObject,
    expected_qualname: &str,
) -> Option<crate::PreparedGeneratorConsumerMethod> {
    if function.is_null() || ffi::PyFunction_Check(function) == 0 {
        return None;
    }
    let metadata = crate::PyFunction_GetSoacMetadata(function);
    if metadata.is_null() {
        return None;
    }
    let method_data = &*metadata.cast::<crate::PyFunctionJitExtra>();
    let method = &*function.cast::<ffi::PyFunctionObject>();
    if method_data.compile_session.id() != data.compile_session.id()
        || method_data.module_state.module_name != "soac.runtime"
        || !ptr::eq(
            method_data.module_state.as_ref(),
            data.module_state.as_ref(),
        )
        || method_data.function_template.function().names.qualname != expected_qualname
        || crate::PyFunction_GetSoacFunctionId(function)
            != method_data.function_id.to_packed_runtime_u64()
        || method.func_code != method_data.registered_code
        || method_data
            .module_state
            .lookup_original_code(method_data.function_id)
            .map(pyo3::Py::as_ptr)
            != Some(method.func_code)
        || method.func_globals != data.function_env.globals_obj()
        || method.func_builtins != data.function_env.builtins_obj()
    {
        return None;
    }
    Some(crate::PreparedGeneratorConsumerMethod {
        function: function as usize,
        code: method.func_code as usize,
        function_id: method_data.function_id,
    })
}

unsafe fn prepare_guarded_generator_builtin_consumer(
    owner_type: *mut ffi::PyTypeObject,
    data: &crate::PyFunctionJitExtra,
) -> Option<crate::PreparedGeneratorBuiltinConsumer> {
    if data.module_state.module_name != "soac.runtime"
        || !data
            .function_template
            .function()
            .names
            .qualname
            .starts_with("ClosureGenerator.__soac_constructor_entry__#")
        || crate::PyType_GetSoacFunctionId(owner_type.cast())
            != data.function_id.to_packed_runtime_u64()
        || (*owner_type).tp_dict.is_null()
        || (*owner_type).tp_version_tag == 0
    {
        return None;
    }

    let globals = data.function_env.globals_obj();
    let builtins = data.function_env.builtins_obj();
    if globals.is_null()
        || builtins.is_null()
        || ffi::PyDict_CheckExact(globals) == 0
        || ffi::PyDict_CheckExact(builtins) == 0
    {
        return None;
    }

    let globals_dict = &*globals.cast::<ffi::PyDictObject>();
    let runtime_keys = globals_dict
        .ma_keys
        .cast::<super::RawPyDictKeysObjectForJit>();
    let runtime_values = globals_dict.ma_values.cast::<RawPyDictIndexedValues>();
    if runtime_keys.is_null()
        || runtime_values.is_null()
        || (*runtime_keys).dk_kind != 3
        || (*runtime_keys).dk_nentries < 0
        || (*runtime_values).capacity < 0
    {
        return None;
    }

    let builtins_dict = &*builtins.cast::<ffi::PyDictObject>();
    let builtin_keys = builtins_dict
        .ma_keys
        .cast::<super::RawPyDictKeysObjectForJit>();
    if builtin_keys.is_null()
        || !builtins_dict.ma_values.is_null()
        || (*builtin_keys).dk_kind != 1
        || (*builtin_keys).dk_nentries < 0
    {
        return None;
    }

    let runtime_entries = [
        prepare_stop_iteration_runtime_entry(
            data,
            runtime_keys,
            runtime_values,
            c"ClosureGenerator",
        )?,
        prepare_stop_iteration_runtime_entry(
            data,
            runtime_keys,
            runtime_values,
            c"_is_generator_closed",
        )?,
        prepare_stop_iteration_runtime_entry(
            data,
            runtime_keys,
            runtime_values,
            c"_reraise_control_flow",
        )?,
        prepare_stop_iteration_runtime_entry(
            data,
            runtime_keys,
            runtime_values,
            c"resume_generator",
        )?,
        prepare_stop_iteration_runtime_entry(
            data,
            runtime_keys,
            runtime_values,
            c"load_preserved_state",
        )?,
        prepare_stop_iteration_runtime_entry(data, runtime_keys, runtime_values, c"NO_DEFAULT")?,
        prepare_stop_iteration_runtime_entry(data, runtime_keys, runtime_values, c"bool")?,
        prepare_stop_iteration_runtime_entry(data, runtime_keys, runtime_values, c"BaseException")?,
        prepare_stop_iteration_runtime_entry(data, runtime_keys, runtime_values, c"StopIteration")?,
    ];
    let builtin_entries = [
        prepare_stop_iteration_builtin_entry(builtin_keys, c"any")?,
        prepare_stop_iteration_builtin_entry(builtin_keys, c"all")?,
        prepare_stop_iteration_builtin_entry(builtin_keys, c"bool")?,
        prepare_stop_iteration_builtin_entry(builtin_keys, c"BaseException")?,
        prepare_stop_iteration_builtin_entry(builtin_keys, c"StopIteration")?,
    ];
    if runtime_entries[GENERATOR_RUNTIME_CLASS].value != owner_type as usize
        || runtime_entries[GENERATOR_RUNTIME_BOOL].value != 0
        || runtime_entries[GENERATOR_RUNTIME_BASE_EXCEPTION].value != 0
        || runtime_entries[GENERATOR_RUNTIME_STOP_ITERATION].value != 0
        || runtime_entries[GENERATOR_RUNTIME_NO_DEFAULT].value == 0
        || !stop_iteration_exact_builtin(
            builtin_entries[0].value as *mut ffi::PyObject,
            c"any",
            builtins,
        )
        || !stop_iteration_exact_builtin(
            builtin_entries[1].value as *mut ffi::PyObject,
            c"all",
            builtins,
        )
        || builtin_entries[2].value
            != ptr::addr_of_mut!(ffi::PyBool_Type).cast::<ffi::PyObject>() as usize
        || builtin_entries[3].value != ffi::PyExc_BaseException as usize
        || builtin_entries[4].value != ffi::PyExc_StopIteration as usize
    {
        return None;
    }

    for (entry, expected_name) in [
        (GENERATOR_RUNTIME_RESUME, c"resume_generator"),
        (GENERATOR_RUNTIME_LOAD_STATE, c"load_preserved_state"),
    ] {
        let function = runtime_entries[entry].value as *mut ffi::PyObject;
        if function.is_null() || ffi::PyCFunction_CheckExact(function) == 0 {
            return None;
        }
        let function = &*function.cast::<ffi::PyCFunctionObject>();
        if function.m_ml.is_null()
            || (*function.m_ml).ml_name.is_null()
            || CStr::from_ptr((*function.m_ml).ml_name) != expected_name
        {
            return None;
        }
    }

    let dict = (*owner_type).tp_dict;
    let init = ffi::PyDict_GetItemString(dict, c"__init__".as_ptr());
    if init.is_null()
        || ffi::PyFunction_Check(init) == 0
        || (*init.cast::<ffi::PyFunctionObject>()).func_code != data.registered_code
    {
        return None;
    }
    let methods = [
        prepare_generator_consumer_method(
            data,
            ffi::PyDict_GetItemString(dict, c"__iter__".as_ptr()),
            "ClosureGenerator.__iter__",
        )?,
        prepare_generator_consumer_method(
            data,
            ffi::PyDict_GetItemString(dict, c"__next__".as_ptr()),
            "ClosureGenerator.__next__",
        )?,
        prepare_generator_consumer_method(
            data,
            ffi::PyDict_GetItemString(dict, c"send".as_ptr()),
            "ClosureGenerator.send",
        )?,
        prepare_generator_consumer_method(
            data,
            runtime_entries[GENERATOR_RUNTIME_CLOSED].value as *mut ffi::PyObject,
            "_is_generator_closed",
        )?,
        prepare_generator_consumer_method(
            data,
            runtime_entries[GENERATOR_RUNTIME_RERAISE].value as *mut ffi::PyObject,
            "_reraise_control_flow",
        )?,
    ];

    let resume_name = std::ffi::CString::new("_resume_function").ok()?;
    let preserved_name = std::ffi::CString::new("_preserved_values").ok()?;
    let closed_name = std::ffi::CString::new("_closed_slot").ok()?;
    let resume_function_offset = crate::late_bound_slot_offset_for_owner(
        owner_type,
        &resume_name,
        crate::IndexedFieldAccessKind::Load,
    )?;
    let preserved_values_offset = crate::late_bound_slot_offset_for_owner(
        owner_type,
        &preserved_name,
        crate::IndexedFieldAccessKind::Load,
    )?;
    let closed_slot_offset = crate::late_bound_slot_offset_for_owner(
        owner_type,
        &closed_name,
        crate::IndexedFieldAccessKind::Load,
    )?;

    Some(crate::PreparedGeneratorBuiltinConsumer {
        compile_session_id: data.compile_session.id(),
        constructor_function_id: data.function_id,
        owner_type: owner_type as usize,
        owner_type_version: (*owner_type).tp_version_tag,
        runtime_globals: globals as usize,
        runtime_keys: runtime_keys as usize,
        runtime_values: runtime_values as usize,
        builtins: builtins as usize,
        builtin_keys: builtin_keys as usize,
        runtime_entries,
        builtin_entries,
        methods,
        resume_function_offset,
        preserved_values_offset,
        closed_slot_offset,
    })
}

unsafe fn guarded_generator_consumer_still_canonical(
    prepared: &crate::PreparedGeneratorBuiltinConsumer,
    data: &crate::PyFunctionJitExtra,
    owner_type: *mut ffi::PyTypeObject,
) -> bool {
    if crate::entry_interpreter_vectorcall_for_tests_enabled()
        || data.compile_session.id() != prepared.compile_session_id
        || owner_type as usize != prepared.owner_type
        || (*owner_type).tp_version_tag != prepared.owner_type_version
        || crate::PyType_GetSoacMetadata(owner_type.cast()) != ptr::from_ref(data).cast_mut().cast()
        || crate::PyType_GetSoacFunctionId(owner_type.cast())
            != prepared.constructor_function_id.to_packed_runtime_u64()
        || data.function_env.globals_obj() as usize != prepared.runtime_globals
        || data.function_env.builtins_obj() as usize != prepared.builtins
    {
        return false;
    }

    let globals = &*(prepared.runtime_globals as *mut ffi::PyDictObject);
    let keys = globals.ma_keys.cast::<super::RawPyDictKeysObjectForJit>();
    let values = globals.ma_values.cast::<RawPyDictIndexedValues>();
    if keys as usize != prepared.runtime_keys
        || values as usize != prepared.runtime_values
        || (*keys).dk_kind != 3
        || (*keys).dk_nentries < 0
        || (*values).capacity < 0
        || !stop_iteration_runtime_entries_still_match(keys, values, &prepared.runtime_entries)
    {
        return false;
    }

    let builtins = &*(prepared.builtins as *mut ffi::PyDictObject);
    let builtin_keys = builtins.ma_keys.cast::<super::RawPyDictKeysObjectForJit>();
    if builtin_keys as usize != prepared.builtin_keys
        || !builtins.ma_values.is_null()
        || (*builtin_keys).dk_kind != 1
        || (*builtin_keys).dk_nentries < 0
        || !stop_iteration_builtin_entries_still_match(builtin_keys, &prepared.builtin_entries)
    {
        return false;
    }

    prepared.methods.iter().all(|method| {
        let function = method.function as *mut ffi::PyObject;
        ffi::PyFunction_Check(function) != 0
            && crate::PyFunction_GetSoacFunctionId(function)
                == method.function_id.to_packed_runtime_u64()
            && (*function.cast::<ffi::PyFunctionObject>()).func_code as usize == method.code
            && !super::raw_py_function_activation_is_observed(method.code as *mut ffi::PyObject)
    })
}

unsafe fn generator_consumer_object_slot(
    owner: *mut ffi::PyObject,
    offset: usize,
) -> *mut ffi::PyObject {
    *owner.cast::<u8>().add(offset).cast::<*mut ffi::PyObject>()
}

unsafe fn guarded_generator_direct_next(
    prepared: &crate::PreparedGeneratorBuiltinConsumer,
    data: &crate::PyFunctionJitExtra,
    iterator: *mut ffi::PyObject,
) -> Option<*mut ffi::PyObject> {
    if (*iterator).ob_type as usize != prepared.owner_type
        || !guarded_generator_consumer_still_canonical(prepared, data, (*iterator).ob_type)
    {
        return None;
    }

    let resume = generator_consumer_object_slot(iterator, prepared.resume_function_offset);
    let preserved = generator_consumer_object_slot(iterator, prepared.preserved_values_offset);
    let closed = generator_consumer_object_slot(iterator, prepared.closed_slot_offset);
    if resume.is_null()
        || preserved.is_null()
        || closed.is_null()
        || ffi::PyFunction_Check(resume) == 0
        || ffi::PyLong_CheckExact(closed) == 0
        || ffi::PyCapsule_IsValid(preserved, c"soac.PreservedState".as_ptr()) == 0
    {
        return None;
    }

    let resume_metadata = crate::PyFunction_GetSoacMetadata(resume);
    if resume_metadata.is_null() {
        return None;
    }
    let resume_data = &*resume_metadata.cast::<crate::PyFunctionJitExtra>();
    let resume_function = &*resume.cast::<ffi::PyFunctionObject>();
    if resume_data.compile_session.id() != data.compile_session.id()
        || *resume_data.function_template.function().lowered_kind()
            != soac_core::block_py::FunctionKind::Generator
        || resume_data.function_template.function().names.display_name != "<genexpr>"
        || crate::PyFunction_GetSoacFunctionId(resume)
            != resume_data.function_id.to_packed_runtime_u64()
        || resume_function.func_code != resume_data.registered_code
        || resume_data
            .module_state
            .lookup_original_code(resume_data.function_id)
            .map(pyo3::Py::as_ptr)
            != Some(resume_function.func_code)
        || super::raw_py_function_activation_is_observed(resume_function.func_code)
    {
        return None;
    }

    let closed_slot = ffi::PyLong_AsSsize_t(closed);
    if closed_slot < 0 {
        if !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
        }
        return None;
    }
    let Some(layout) = resume_data
        .function_template
        .function()
        .public_storage_layout()
    else {
        return None;
    };
    let Some(slot) = layout.preserved_slots.get(closed_slot as usize) else {
        return None;
    };
    if slot.logical_name != "_dp_is_closed"
        || slot.storage != soac_core::block_py::PreservedSlotStorage::I64
    {
        return None;
    }
    // The capsule is a mutable public instance attribute: another valid
    // preserved state need not have this generator's expected slot count.
    let closed_value = preserved_state::load_preserved_state_owned(preserved, closed_slot as i64);
    if closed_value.is_null() {
        if !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
        }
        return None;
    }
    if ffi::PyLong_CheckExact(closed_value) == 0 {
        ffi::Py_DECREF(closed_value);
        return None;
    }
    let closed_state = ffi::PyLong_AsLongLong(closed_value);
    ffi::Py_DECREF(closed_value);
    if !ffi::PyErr_Occurred().is_null() {
        ffi::PyErr_Clear();
        return None;
    }
    if closed_state != 0 {
        ffi::PyErr_SetNone(ffi::PyExc_StopIteration);
        return Some(ptr::null_mut());
    }

    let no_default =
        prepared.runtime_entries[GENERATOR_RUNTIME_NO_DEFAULT].value as *mut ffi::PyObject;
    // The Python send wrapper evaluates these attributes into owned call
    // arguments. Keep the same owners alive if the generator body reenters and
    // replaces an instance slot or the runtime sentinel while its native frame
    // is active.
    ffi::Py_INCREF(resume);
    ffi::Py_INCREF(preserved);
    ffi::Py_INCREF(no_default);
    let result = crate::resume_generator(resume, iterator, preserved, ffi::Py_None(), no_default);
    ffi::Py_DECREF(no_default);
    ffi::Py_DECREF(preserved);
    ffi::Py_DECREF(resume);
    if !result.is_null() {
        return Some(result);
    }
    let error = ffi::PyErr_GetRaisedException();
    if error.is_null() {
        return Some(ptr::null_mut());
    }
    if (*error).ob_type == ffi::PyExc_StopIteration.cast::<ffi::PyTypeObject>() {
        // A native CPython generator does not run SOAC's cancellation helper when
        // ordinary iteration ends. Preserve the pending exception until the owner
        // is released, exactly as builtin any()/all() do.
        ffi::PyErr_SetRaisedException(error);
        return Some(ptr::null_mut());
    }

    // The body can replace the helper or resize/promote its runtime globals.
    // Resolve the current global by its actual interned name instead of
    // dereferencing cached dictionary storage after arbitrary Python code.
    let helper_name = ffi::PyUnicode_InternFromString(c"_reraise_control_flow".as_ptr());
    if helper_name.is_null() {
        ffi::Py_DECREF(error);
        return Some(ptr::null_mut());
    }
    let reraise = load_global_slow(
        data.function_env.globals_obj(),
        data.function_env.builtins_obj(),
        helper_name,
    );
    ffi::Py_DECREF(helper_name);
    if reraise.is_null() {
        ffi::Py_DECREF(error);
        return Some(ptr::null_mut());
    }
    let handled = ffi::PyObject_CallOneArg(reraise, error);
    ffi::Py_DECREF(reraise);
    ffi::Py_DECREF(error);
    if handled.is_null() {
        return Some(ptr::null_mut());
    }
    ffi::Py_DECREF(handled);
    let none = ffi::Py_None();
    ffi::Py_INCREF(none);
    Some(none)
}

unsafe fn consume_guarded_generator_builtin(
    prepared: &crate::PreparedGeneratorBuiltinConsumer,
    data: &crate::PyFunctionJitExtra,
    iterable: *mut ffi::PyObject,
    kind: GuardedGeneratorBuiltin,
) -> *mut ffi::PyObject {
    let iterator = ffi::PyObject_GetIter(iterable);
    if iterator.is_null() {
        return ptr::null_mut();
    }
    let Some(iternext) = (*(*iterator).ob_type).tp_iternext else {
        ffi::Py_DECREF(iterator);
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            c"iter() returned a non-iterator".as_ptr(),
        );
        return ptr::null_mut();
    };

    loop {
        let item = match guarded_generator_direct_next(prepared, data, iterator) {
            Some(item) => item,
            None => iternext(iterator),
        };
        if item.is_null() {
            break;
        }
        let truth = ffi::PyObject_IsTrue(item);
        ffi::Py_DECREF(item);
        if truth < 0 {
            ffi::Py_DECREF(iterator);
            return ptr::null_mut();
        }
        if (kind == GuardedGeneratorBuiltin::Any && truth > 0)
            || (kind == GuardedGeneratorBuiltin::All && truth == 0)
        {
            ffi::Py_DECREF(iterator);
            return ffi::PyBool_FromLong((kind == GuardedGeneratorBuiltin::Any) as libc::c_long);
        }
    }

    ffi::Py_DECREF(iterator);
    if !ffi::PyErr_Occurred().is_null() {
        if ffi::PyErr_ExceptionMatches(ffi::PyExc_StopIteration) == 0 {
            return ptr::null_mut();
        }
        ffi::PyErr_Clear();
    }
    ffi::PyBool_FromLong((kind == GuardedGeneratorBuiltin::All) as libc::c_long)
}

unsafe fn fast_guarded_generator_builtin_consumption(
    callable: ObjPtr,
    args: ObjPtr,
    kind: GuardedGeneratorBuiltin,
) -> Option<ObjPtr> {
    if crate::entry_interpreter_vectorcall_for_tests_enabled() {
        return None;
    }
    let iterable = *args.cast::<*mut ffi::PyObject>();
    if iterable.is_null() {
        return None;
    }
    let owner_type = (*iterable).ob_type;
    if owner_type.is_null() || (*owner_type).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE == 0 {
        return None;
    }
    let metadata = crate::PyType_GetSoacMetadata(owner_type.cast());
    if metadata.is_null() {
        return None;
    }
    let data = &*metadata.cast::<crate::PyFunctionJitExtra>();
    if data.module_state.module_name != "soac.runtime" {
        return None;
    }
    let prepared = match data
        .function_template
        .prepared_generator_builtin_consumer
        .get()
    {
        Some(prepared) => prepared,
        None => {
            let prepared = prepare_guarded_generator_builtin_consumer(owner_type, data)?;
            // Preparation may allocate Python names; never initialize a OnceLock
            // while callbacks can reenter another generator consumer.
            let _ = data
                .function_template
                .prepared_generator_builtin_consumer
                .set(prepared);
            data.function_template
                .prepared_generator_builtin_consumer
                .get()?
        }
    };
    let callable_index = match kind {
        GuardedGeneratorBuiltin::Any => 0,
        GuardedGeneratorBuiltin::All => 1,
    };
    if callable as usize != prepared.builtin_entries[callable_index].value
        || !guarded_generator_consumer_still_canonical(prepared, data, owner_type)
    {
        return None;
    }

    if ffi::Py_EnterRecursiveCall(c" while calling a Python object".as_ptr()) != 0 {
        return Some(ptr::null_mut());
    }
    let result = consume_guarded_generator_builtin(prepared, data, iterable, kind);
    ffi::Py_LeaveRecursiveCall();
    Some(result.cast())
}

#[cfg(test)]
#[test]
fn stop_iteration_live_dictionary_guards_observe_unversioned_slot_mutations() {
    unsafe extern "C" {
        fn _PyDict_NewIndexedKeySet(keys: *mut ffi::PyObject) -> *mut c_void;
        fn _PyDict_NewWithIndexedKeySet(keys: *mut c_void) -> *mut ffi::PyObject;
        fn _PyDictKeys_DecRef(keys: *mut c_void);
    }

    let _guard = crate::python_runtime_test_lock().lock().unwrap();
    crate::initialize_test_python();
    pyo3::Python::attach(|_| unsafe {
        let key_names = ffi::PyTuple_New(2);
        assert!(!key_names.is_null(), "indexed key tuple must allocate");
        for (index, name) in [c"present", c"missing"].iter().enumerate() {
            let mut key = ffi::PyUnicode_FromString(name.as_ptr());
            assert!(!key.is_null(), "indexed key must allocate");
            ffi::PyUnicode_InternInPlace(&mut key);
            assert_eq!(ffi::PyTuple_SetItem(key_names, index as isize, key), 0);
        }

        let indexed_keys = _PyDict_NewIndexedKeySet(key_names);
        assert!(
            !indexed_keys.is_null(),
            "actual indexed key set must allocate"
        );
        let indexed_dict = _PyDict_NewWithIndexedKeySet(indexed_keys);
        _PyDictKeys_DecRef(indexed_keys);
        assert!(
            !indexed_dict.is_null(),
            "actual indexed dictionary must allocate"
        );

        let present_key = ffi::PyTuple_GetItem(key_names, 0);
        let missing_key = ffi::PyTuple_GetItem(key_names, 1);
        let present_index = _PyDict_IndexedKeyIndex(indexed_dict, present_key);
        let missing_index = _PyDict_IndexedKeyIndex(indexed_dict, missing_key);
        assert!(present_index >= 0 && missing_index >= 0);

        let present = ffi::PyList_New(0);
        let replacement = ffi::PyList_New(0);
        assert!(!present.is_null() && !replacement.is_null());
        assert_eq!(
            _PyDict_SetIndexedItem(indexed_dict, present_index, present),
            0
        );
        let present_refcount = ffi::Py_REFCNT(present);
        let replacement_refcount = ffi::Py_REFCNT(replacement);
        assert_eq!(
            present_refcount, 2,
            "the local and indexed slot each own one reference"
        );
        assert_eq!(
            replacement_refcount, 1,
            "the replacement begins locally owned"
        );

        let dict = &*indexed_dict.cast::<ffi::PyDictObject>();
        let keys = dict.ma_keys.cast::<super::RawPyDictKeysObjectForJit>();
        let values = dict.ma_values.cast::<RawPyDictIndexedValues>();
        assert_eq!((*keys).dk_kind, 3);
        let key_entries = stop_iteration_unicode_entries(keys);
        let expected = [
            crate::PreparedStopIterationDictionaryEntry {
                index: present_index as usize,
                key: (*key_entries.add(present_index as usize)).me_key as usize,
                value: present as usize,
            },
            crate::PreparedStopIterationDictionaryEntry {
                index: missing_index as usize,
                key: (*key_entries.add(missing_index as usize)).me_key as usize,
                value: 0,
            },
        ];
        assert!(stop_iteration_runtime_entries_still_match(
            keys, values, &expected,
        ));

        let slots = (&raw mut (*values).values).cast::<*mut ffi::PyObject>();
        let version = (*keys).dk_version;
        let used = dict.ma_used;

        ffi::Py_INCREF(replacement);
        *slots.add(present_index as usize) = replacement;
        ffi::Py_DECREF(present);
        assert_eq!(ffi::Py_REFCNT(present), present_refcount - 1);
        assert_eq!(ffi::Py_REFCNT(replacement), replacement_refcount + 1);
        assert_eq!((*keys).dk_version, version);
        assert_eq!(dict.ma_used, used);
        assert!(
            !stop_iteration_runtime_entries_still_match(keys, values, &expected),
            "replacing a present indexed value without watcher/version updates must invalidate"
        );
        ffi::Py_INCREF(present);
        *slots.add(present_index as usize) = present;
        ffi::Py_DECREF(replacement);
        assert_eq!(ffi::Py_REFCNT(present), present_refcount);
        assert_eq!(ffi::Py_REFCNT(replacement), replacement_refcount);
        assert!(stop_iteration_runtime_entries_still_match(
            keys, values, &expected,
        ));

        let missing_slot = slots.add(missing_index as usize);
        let original_missing = *missing_slot;
        ffi::Py_INCREF(replacement);
        *missing_slot = replacement;
        assert_eq!(ffi::Py_REFCNT(replacement), replacement_refcount + 1);
        assert_eq!((*keys).dk_version, version);
        assert_eq!(dict.ma_used, used);
        assert!(
            !stop_iteration_runtime_entries_still_match(keys, values, &expected),
            "populating a formerly absent indexed shadow without metadata updates must invalidate"
        );
        *missing_slot = original_missing;
        ffi::Py_DECREF(replacement);
        assert_eq!(ffi::Py_REFCNT(replacement), replacement_refcount);
        assert!(stop_iteration_runtime_entries_still_match(
            keys, values, &expected,
        ));

        let builtin_dict = ffi::PyDict_New();
        let builtin_key = ffi::PyUnicode_FromString(c"guarded".as_ptr());
        assert!(!builtin_dict.is_null() && !builtin_key.is_null());
        assert_eq!(ffi::PyDict_SetItem(builtin_dict, builtin_key, present), 0);
        let builtin_keys = (*builtin_dict.cast::<ffi::PyDictObject>())
            .ma_keys
            .cast::<super::RawPyDictKeysObjectForJit>();
        assert_eq!((*builtin_keys).dk_kind, 1);
        let builtin_entry = prepare_stop_iteration_builtin_entry(builtin_keys, c"guarded").unwrap();
        assert!(stop_iteration_builtin_entries_still_match(
            builtin_keys,
            &[builtin_entry],
        ));
        assert_eq!(
            ffi::PyDict_SetItem(builtin_dict, builtin_key, replacement),
            0
        );
        assert_eq!(
            (*builtin_dict.cast::<ffi::PyDictObject>()).ma_keys as usize,
            builtin_keys as usize,
            "replacing an existing builtin value must retain its keys object"
        );
        assert!(
            !stop_iteration_builtin_entries_still_match(builtin_keys, &[builtin_entry]),
            "combined builtin value replacement must invalidate even when keys are unchanged"
        );

        ffi::Py_DECREF(builtin_key);
        ffi::Py_DECREF(builtin_dict);
        ffi::Py_DECREF(replacement);
        ffi::Py_DECREF(present);
        ffi::Py_DECREF(indexed_dict);
        ffi::Py_DECREF(key_names);
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
    if receiver.is_null() {
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
    let descriptor = ffi::PyDict_GetItemString(dict, method_name.as_ptr());
    if descriptor.is_null() {
        if !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
        }
        return 0;
    }
    if ffi::PyFunction_Check(descriptor) == 0 {
        return 0;
    }
    crate::PyFunction_GetSoacFunctionId(descriptor) as i64
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
            // The module dict can be promoted after an unprofiled key insertion.
            // Fall back to the mapping lookup so the JIT remains semantically CPython-like.
            ffi::PyErr_Clear();
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
            b"local variable referenced before assignment\0"
                .as_ptr()
                .cast(),
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
            b"missing exception for dp_jit_push_handled_exception\0"
                .as_ptr()
                .cast(),
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_push_handled_exception(exc: ObjPtr) -> ObjPtr {
    push_handled_exception_hook(exc)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_pop_handled_exception(previous: ObjPtr) {
    pop_handled_exception_hook(previous);
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
    panic_obj_export!(dp_jit_load_cell(cell: ObjPtr));
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
        live_value_count: i64
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
    dp_jit_py_call_with_kw,
    dp_jit_py_call_with_kw_with_frame(callable: ObjPtr, args: ObjPtr, kw: ObjPtr) => py_call_with_kw_hook(callable, args, kw)
);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_jit_record_top_value_sample(counter: ObjPtr, value: i64) {
    record_top_value_sample_hook(counter, value)
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
pub unsafe extern "C" fn dp_jit_load_cell(cell: ObjPtr) -> ObjPtr {
    load_cell_hook(cell)
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
        "dp_jit_py_vectorcall",
        chosen_helper_symbol(
            dp_jit_py_vectorcall as *const u8,
            dp_jit_py_vectorcall_with_frame as *const u8,
        ),
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
        "dp_jit_load_runtime_obj_by_id",
        dp_jit_load_runtime_obj_by_id as *const u8,
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
        "dp_jit_push_handled_exception",
        push_handled_exception_hook as *const u8,
    );
    builder.symbol(
        "dp_jit_pop_handled_exception",
        pop_handled_exception_hook as *const u8,
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
