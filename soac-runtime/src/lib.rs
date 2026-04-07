#![no_std]

use core::ffi::c_void;

#[repr(C)]
#[cfg(all(target_pointer_width = "64", target_endian = "little"))]
#[derive(Clone, Copy)]
union PyObjectObRefcnt {
    ob_refcnt_full: i64,
    refcnt_and_flags: PyObjectObFlagsAndRefcnt,
}

#[repr(C)]
#[cfg(all(target_pointer_width = "64", target_endian = "big"))]
#[derive(Clone, Copy)]
union PyObjectObRefcnt {
    ob_refcnt_full: i64,
    refcnt_and_flags: PyObjectObFlagsAndRefcnt,
}

#[repr(C)]
#[cfg(all(target_pointer_width = "64", target_endian = "little"))]
#[derive(Clone, Copy)]
struct PyObjectObFlagsAndRefcnt {
    ob_refcnt: u32,
    ob_overflow: u16,
    ob_flags: u16,
}

#[repr(C)]
#[cfg(all(target_pointer_width = "64", target_endian = "big"))]
#[derive(Clone, Copy)]
struct PyObjectObFlagsAndRefcnt {
    ob_flags: u16,
    ob_overflow: u16,
    ob_refcnt: u32,
}

#[repr(C)]
struct PyObject {
    #[cfg(target_pointer_width = "64")]
    ob_refcnt: PyObjectObRefcnt,
    #[cfg(target_pointer_width = "32")]
    ob_refcnt: isize,
    ob_type: *mut c_void,
}

#[repr(C)]
struct PyFunctionObject {
    ob_base: PyObject,
    func_globals: *mut c_void,
    func_builtins: *mut c_void,
    func_name: *mut c_void,
    func_qualname: *mut c_void,
    func_code: *mut c_void,
    func_defaults: *mut c_void,
    func_kwdefaults: *mut c_void,
    func_closure: *mut c_void,
    func_doc: *mut c_void,
    func_dict: *mut c_void,
    func_weakreflist: *mut c_void,
    func_module: *mut c_void,
    func_annotations: *mut c_void,
    func_annotate: *mut c_void,
    func_typeparams: *mut c_void,
    vectorcall: *mut c_void,
    func_soac_metadata: *mut c_void,
    func_soac_metadata_destructor: *mut c_void,
    func_soac_function_id: u64,
}

#[repr(C)]
struct PyDictObject {
    ob_base: PyObject,
    ma_used: isize,
    ma_watcher_tag: u64,
    ma_keys: *mut PyDictKeysObject,
    ma_values: *mut PyDictIndexedValues,
}

#[repr(C)]
struct PyDictKeysObject {
    dk_refcnt: isize,
    dk_log2_size: u8,
    dk_log2_index_bytes: u8,
    dk_kind: u8,
    dk_version: u32,
    dk_usable: isize,
    dk_nentries: isize,
}

#[repr(C)]
struct PyDictUnicodeEntry {
    me_key: *mut PyObject,
    me_value: *mut PyObject,
}

#[repr(C)]
struct PyDictIndexedValues {
    capacity: isize,
    order_size: isize,
    values: [*mut PyObject; 1],
}

#[repr(C)]
struct PyMethodObject {
    ob_base: PyObject,
    im_func: *mut PyObject,
    im_self: *mut PyObject,
    im_weakreflist: *mut c_void,
    vectorcall: *mut c_void,
}

#[repr(C)]
struct ClifFunctionData {
    runtime_objects: *mut c_void,
}

unsafe extern "C" {
    fn _Py_Dealloc(obj: *mut PyObject);
    fn soac_runtime_load_global_slow(
        dict: *mut c_void,
        key: *mut c_void,
        index: isize,
    ) -> *mut c_void;
    fn dp_jit_store_global(
        globals_obj: *mut c_void,
        name: *mut c_void,
        slot_index: i64,
        value: *mut c_void,
    ) -> *mut c_void;
    static mut PyFunction_Type: c_void;
    static mut PyMethod_Type: c_void;
    static mut _PyDict_IndexedValueTombstone: c_void;
}

#[inline(always)]
unsafe fn dict_unicode_entries(keys: *mut PyDictKeysObject) -> *mut PyDictUnicodeEntry {
    let indices = unsafe { keys.cast::<u8>().add(core::mem::size_of::<PyDictKeysObject>()) };
    let entries = unsafe { indices.add(1usize << (*keys).dk_log2_index_bytes) };
    entries.cast::<PyDictUnicodeEntry>()
}

#[inline(always)]
unsafe fn indexed_key(keys: *mut PyDictKeysObject, index: isize) -> *mut PyObject {
    unsafe { (*dict_unicode_entries(keys).offset(index)).me_key }
}

#[inline(always)]
unsafe fn can_skip_incref(obj: *mut PyObject) -> bool {
    #[cfg(target_pointer_width = "64")]
    {
        const PY_IMMORTAL_INITIAL_REFCNT: u32 = 3u32 << 30;
        unsafe { (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt >= PY_IMMORTAL_INITIAL_REFCNT }
    }

    #[cfg(target_pointer_width = "32")]
    {
        const PY_IMMORTAL_MINIMUM_REFCNT: isize = 1isize << 30;
        unsafe { (*obj).ob_refcnt >= PY_IMMORTAL_MINIMUM_REFCNT }
    }
}

#[inline(always)]
unsafe fn can_skip_decref(obj: *mut PyObject) -> bool {
    #[cfg(target_pointer_width = "64")]
    {
        unsafe { ((*obj).ob_refcnt.refcnt_and_flags.ob_refcnt as i32) < 0 }
    }

    #[cfg(target_pointer_width = "32")]
    {
        const PY_IMMORTAL_MINIMUM_REFCNT: isize = 1isize << 30;
        unsafe { (*obj).ob_refcnt >= PY_IMMORTAL_MINIMUM_REFCNT }
    }
}

#[inline(always)]
unsafe fn incref_impl(obj: *mut PyObject) {
    if obj.is_null() || unsafe { can_skip_incref(obj) } {
        return;
    }

    #[cfg(target_pointer_width = "64")]
    unsafe {
        let cur_refcnt = (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt;
        (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt = cur_refcnt.wrapping_add(1);
    }

    #[cfg(target_pointer_width = "32")]
    unsafe {
        (*obj).ob_refcnt = (*obj).ob_refcnt.wrapping_add(1);
    }
}

#[inline(always)]
unsafe fn decref_impl(obj: *mut PyObject) {
    if obj.is_null() || unsafe { can_skip_decref(obj) } {
        return;
    }

    #[cfg(target_pointer_width = "64")]
    unsafe {
        let next_refcnt = (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt.wrapping_sub(1);
        (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt = next_refcnt;
        if next_refcnt == 0 {
            _Py_Dealloc(obj);
        }
    }

    #[cfg(target_pointer_width = "32")]
    unsafe {
        let next_refcnt = (*obj).ob_refcnt.wrapping_sub(1);
        (*obj).ob_refcnt = next_refcnt;
        if next_refcnt == 0 {
            _Py_Dealloc(obj);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_incref(obj: *mut c_void) {
    unsafe { incref_impl(obj.cast::<PyObject>()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_decref(obj: *mut c_void) {
    unsafe { decref_impl(obj.cast::<PyObject>()) };
}

#[inline(always)]
unsafe fn dict_guarded_index(
    dict: *mut PyDictObject,
    key: *mut PyObject,
    index: isize,
) -> i64 {
    let values = unsafe { (*dict).ma_values };
    if !values.is_null() && unsafe { index < (*values).capacity } {
        debug_assert!(unsafe { index < (*values).capacity });
        let slot_key = unsafe { indexed_key((*dict).ma_keys, index) };
        if slot_key == key {
            return index as i64;
        }
    }
    -1
}

#[inline(always)]
unsafe fn indexed_value(values: *mut PyDictIndexedValues, index: isize) -> *mut PyObject {
    let values_ptr = unsafe { (&raw const (*values).values).cast::<*mut PyObject>() };
    unsafe { *values_ptr.offset(index) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_load_global(
    dict: *mut c_void,
    key: *mut c_void,
    index: isize,
) -> *mut c_void {
    let dict = dict.cast::<PyDictObject>();
    let key = key.cast::<PyObject>();
    debug_assert!(!dict.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(index >= 0);

    if unsafe { dict_guarded_index(dict, key, index) } >= 0 {
        let values = unsafe { (*dict).ma_values };
        let value = unsafe { indexed_value(values, index) };
        if !value.is_null()
            && value.cast::<c_void>() != (&raw mut _PyDict_IndexedValueTombstone)
        {
            unsafe { incref_impl(value) };
            return value.cast::<c_void>();
        }
    }

    unsafe { soac_runtime_load_global_slow(dict.cast::<c_void>(), key.cast::<c_void>(), index) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_global(
    dict: *mut c_void,
    key: *mut c_void,
    index: isize,
    value: *mut c_void,
) -> *mut c_void {
    let dict = dict.cast::<PyDictObject>();
    let key = key.cast::<PyObject>();
    debug_assert!(!dict.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(!value.is_null());
    debug_assert!(index >= 0);

    let guarded_index = unsafe { dict_guarded_index(dict, key, index) };
    unsafe {
        dp_jit_store_global(
            dict.cast::<c_void>(),
            key.cast::<c_void>(),
            guarded_index,
            value,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_callee_function_id(callable: *mut c_void) -> i64 {
    let callable = callable.cast::<PyObject>();
    if callable.is_null() {
        return i64::MIN;
    }
    let function = if unsafe { (*callable).ob_type } == (&raw mut PyFunction_Type).cast::<c_void>()
    {
        callable
    } else if unsafe { (*callable).ob_type } == (&raw mut PyMethod_Type).cast::<c_void>() {
        unsafe { (*(callable as *mut PyMethodObject)).im_func }
    } else {
        return 0;
    };
    if function.is_null() {
        return i64::MIN;
    }
    let packed = unsafe { (*(function as *mut PyFunctionObject)).func_soac_function_id };
    if packed == 0 {
        return 0;
    }
    packed as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_function_data_block(callable: *mut c_void) -> *mut c_void {
    let callable = callable.cast::<PyObject>();
    if callable.is_null() {
        return core::ptr::null_mut();
    }
    let function = if unsafe { (*callable).ob_type } == (&raw mut PyFunction_Type).cast::<c_void>()
    {
        callable
    } else if unsafe { (*callable).ob_type } == (&raw mut PyMethod_Type).cast::<c_void>() {
        unsafe { (*(callable as *mut PyMethodObject)).im_func }
    } else {
        return core::ptr::null_mut();
    };
    if function.is_null() {
        return core::ptr::null_mut();
    }
    let metadata = unsafe { (*(function as *mut PyFunctionObject)).func_soac_metadata };
    if metadata.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { (*(metadata as *mut ClifFunctionData)).runtime_objects }
}
