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
struct RawPyObject {
    #[cfg(target_pointer_width = "64")]
    ob_refcnt: PyObjectObRefcnt,
    #[cfg(target_pointer_width = "32")]
    ob_refcnt: isize,
    ob_type: *mut c_void,
}

#[repr(C)]
struct RawPyFunctionObject {
    ob_base: RawPyObject,
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
struct RawPyDictObject {
    ob_base: RawPyObject,
    ma_used: isize,
    ma_watcher_tag: u64,
    ma_keys: *mut RawPyDictKeysObject,
    ma_values: *mut c_void,
}

#[repr(C)]
struct RawPyDictKeysObject {
    dk_refcnt: isize,
    dk_log2_size: u8,
    dk_log2_index_bytes: u8,
    dk_kind: u8,
    dk_version: u32,
    dk_usable: isize,
    dk_nentries: isize,
}

#[repr(C)]
struct RawPyDictUnicodeEntry {
    me_key: *mut RawPyObject,
    me_value: *mut RawPyObject,
}

#[repr(C)]
struct RawPyDictIndexedValues {
    capacity: isize,
    order_size: isize,
    values: [*mut RawPyObject; 1],
}

#[repr(C)]
struct RawPyDictSplitValues {
    capacity: u8,
    size: u8,
    embedded: u8,
    valid: u8,
    values: [*mut RawPyObject; 1],
}

#[repr(C)]
struct RawPyMethodObject {
    ob_base: RawPyObject,
    im_func: *mut RawPyObject,
    im_self: *mut RawPyObject,
    im_weakreflist: *mut c_void,
    vectorcall: *mut c_void,
}

#[repr(C)]
struct ClifFunctionData {
    runtime_objects: *mut c_void,
}

unsafe extern "C" {
    fn _Py_Dealloc(obj: *mut RawPyObject);
    fn _PyObject_GetDictPtr(obj: *mut RawPyObject) -> *mut *mut RawPyObject;
    fn _PyDict_SetIndexedItem(dict: *mut c_void, index: isize, value: *mut c_void) -> i32;
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
unsafe fn dict_unicode_entries(keys: *mut RawPyDictKeysObject) -> *mut RawPyDictUnicodeEntry {
    let indices = unsafe { keys.cast::<u8>().add(core::mem::size_of::<RawPyDictKeysObject>()) };
    let entries = unsafe { indices.add(1usize << (*keys).dk_log2_index_bytes) };
    entries.cast::<RawPyDictUnicodeEntry>()
}

#[inline(always)]
unsafe fn indexed_key(keys: *mut RawPyDictKeysObject, index: isize) -> *mut RawPyObject {
    unsafe { (*dict_unicode_entries(keys).offset(index)).me_key }
}

#[inline(always)]
unsafe fn can_skip_incref(obj: *mut RawPyObject) -> bool {
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
unsafe fn can_skip_decref(obj: *mut RawPyObject) -> bool {
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
unsafe fn incref_impl(obj: *mut RawPyObject) {
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
unsafe fn decref_impl(obj: *mut RawPyObject) {
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
    unsafe { incref_impl(obj.cast::<RawPyObject>()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_decref(obj: *mut c_void) {
    unsafe { decref_impl(obj.cast::<RawPyObject>()) };
}

#[inline(always)]
unsafe fn dict_guarded_index(
    dict: *mut RawPyDictObject,
    index: isize,
) -> i64 {
    const DICT_KEYS_INDEXED_UNICODE: u8 = 3;

    let keys = unsafe { (*dict).ma_keys };
    let values = unsafe { (*dict).ma_values.cast::<RawPyDictIndexedValues>() };
    if !keys.is_null()
        && !values.is_null()
        && unsafe { (*keys).dk_kind } == DICT_KEYS_INDEXED_UNICODE
        && index >= 0
        && unsafe { index < (*values).capacity }
    {
        return index as i64;
    }
    -1
}

#[inline(always)]
unsafe fn indexed_value(values: *mut RawPyDictIndexedValues, index: isize) -> *mut RawPyObject {
    let values_ptr = unsafe { (&raw const (*values).values).cast::<*mut RawPyObject>() };
    unsafe { *values_ptr.offset(index) }
}

#[inline(always)]
unsafe fn split_value(values: *mut RawPyDictSplitValues, index: isize) -> *mut RawPyObject {
    let values_ptr = unsafe { (&raw const (*values).values).cast::<*mut RawPyObject>() };
    unsafe { *values_ptr.offset(index) }
}

macro_rules! load_indexed_dict_value_owned {
    ($dict:expr, $key:expr, $index:expr) => {{
        let _ = $key;
        if unsafe { dict_guarded_index($dict, $index) } < 0 {
            core::ptr::null_mut()
        } else {
            let values = unsafe { (*$dict).ma_values.cast::<RawPyDictIndexedValues>() };
            let value = unsafe { indexed_value(values, $index) };
            if value.is_null()
                || value.cast::<c_void>() == (&raw mut _PyDict_IndexedValueTombstone)
            {
                core::ptr::null_mut()
            } else {
                unsafe { incref_impl(value) };
                value.cast::<c_void>()
            }
        }
    }};
}

macro_rules! load_split_dict_value_owned {
    ($dict:expr, $key:expr, $index:expr) => {{
        const DICT_KEYS_SPLIT: u8 = 2;

        let dict = $dict;
        let key = $key;
        let index = $index;
        let keys = unsafe { (*dict).ma_keys };
        let values = unsafe { (*dict).ma_values.cast::<RawPyDictSplitValues>() };
        if keys.is_null()
            || values.is_null()
            || unsafe { (*keys).dk_kind } != DICT_KEYS_SPLIT
            || index < 0
            || unsafe { index >= (*values).capacity.into() }
            || unsafe { indexed_key(keys, index) } != key
        {
            core::ptr::null_mut()
        } else {
            let value = unsafe { split_value(values, index) };
            if value.is_null() {
                core::ptr::null_mut()
            } else {
                unsafe { incref_impl(value) };
                value.cast::<c_void>()
            }
        }
    }};
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_load_global_indexed(
    dict: *mut c_void,
    key: *mut c_void,
    index: isize,
) -> *mut c_void {
    let dict = dict.cast::<RawPyDictObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!dict.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(index >= 0);

    load_indexed_dict_value_owned!(dict, key, index)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_load_global(
    dict: *mut c_void,
    key: *mut c_void,
    index: isize,
) -> *mut c_void {
    let dict = dict.cast::<RawPyDictObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!dict.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(index >= 0);

    let value = load_indexed_dict_value_owned!(dict, key, index);
    if !value.is_null() {
        return value;
    }

    unsafe { soac_runtime_load_global_slow(dict.cast::<c_void>(), key.cast::<c_void>(), index) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_global_indexed(
    dict: *mut c_void,
    key: *mut c_void,
    index: isize,
    value: *mut c_void,
) -> *mut c_void {
    let dict_obj = dict.cast::<RawPyDictObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!dict_obj.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(!value.is_null());
    debug_assert!(index >= 0);

    if unsafe { dict_guarded_index(dict_obj, index) } < 0 {
        return core::ptr::null_mut();
    }
    let rc = unsafe { _PyDict_SetIndexedItem(dict, index, value) };
    if rc != 0 {
        return core::ptr::null_mut();
    }
    unsafe { incref_impl(value.cast::<RawPyObject>()) };
    value
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_global(
    dict: *mut c_void,
    key: *mut c_void,
    index: isize,
    value: *mut c_void,
) -> *mut c_void {
    let dict = dict.cast::<RawPyDictObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!dict.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(!value.is_null());
    debug_assert!(index >= 0);

    let guarded_index = unsafe { dict_guarded_index(dict, index) };
    unsafe {
        dp_jit_store_global(
            dict.cast::<c_void>(),
            key.cast::<c_void>(),
            guarded_index,
            value,
        )
    }
}

#[inline(always)]
unsafe fn object_dict(obj: *mut RawPyObject) -> *mut RawPyDictObject {
    let dict_ptr = unsafe { _PyObject_GetDictPtr(obj) };
    if dict_ptr.is_null() {
        return core::ptr::null_mut();
    }
    let dict = unsafe { *dict_ptr };
    if dict.is_null() {
        return core::ptr::null_mut();
    }
    dict.cast::<RawPyDictObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_load_field_indexed(
    obj: *mut c_void,
    key: *mut c_void,
    index: isize,
) -> *mut c_void {
    let obj = obj.cast::<RawPyObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!obj.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(index >= 0);

    let dict = unsafe { object_dict(obj) };
    if dict.is_null() {
        return core::ptr::null_mut();
    }
    load_split_dict_value_owned!(dict, key, index)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_field_indexed(
    obj: *mut c_void,
    key: *mut c_void,
    index: isize,
    value: *mut c_void,
) -> *mut c_void {
    let obj = obj.cast::<RawPyObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!obj.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(!value.is_null());
    debug_assert!(index >= 0);

    let _ = (obj, key, index, value);

    // Split-dict stores are more than an indexed pointer write: CPython must update
    // ma_used, split-values insertion order, dict/object watchers, and promotion state.
    // Keep stores on the generic attribute path until that bookkeeping is exposed here.
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_callee_function_id(callable: *mut c_void) -> i64 {
    let callable = callable.cast::<RawPyObject>();
    if callable.is_null() {
        return i64::MIN;
    }
    let function = if unsafe { (*callable).ob_type } == (&raw mut PyFunction_Type).cast::<c_void>()
    {
        callable
    } else if unsafe { (*callable).ob_type } == (&raw mut PyMethod_Type).cast::<c_void>() {
        unsafe { (*(callable as *mut RawPyMethodObject)).im_func }
    } else {
        return 0;
    };
    if function.is_null() {
        return i64::MIN;
    }
    let packed = unsafe { (*(function as *mut RawPyFunctionObject)).func_soac_function_id };
    if packed == 0 {
        return 0;
    }
    packed as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_function_data_block(callable: *mut c_void) -> *mut c_void {
    let callable = callable.cast::<RawPyObject>();
    if callable.is_null() {
        return core::ptr::null_mut();
    }
    let function = if unsafe { (*callable).ob_type } == (&raw mut PyFunction_Type).cast::<c_void>()
    {
        callable
    } else if unsafe { (*callable).ob_type } == (&raw mut PyMethod_Type).cast::<c_void>() {
        unsafe { (*(callable as *mut RawPyMethodObject)).im_func }
    } else {
        return core::ptr::null_mut();
    };
    if function.is_null() {
        return core::ptr::null_mut();
    }
    let metadata = unsafe { (*(function as *mut RawPyFunctionObject)).func_soac_metadata };
    if metadata.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { (*(metadata as *mut ClifFunctionData)).runtime_objects }
}
