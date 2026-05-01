#![no_std]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("soac_jit_runtime raw CPython layout support requires a 64-bit target");

use core::ffi::{c_int, c_void};

#[repr(C)]
#[derive(Clone, Copy)]
union PyObjectObRefcnt {
    ob_refcnt_full: i64,
    refcnt_and_flags: PyObjectObFlagsAndRefcnt,
}

#[repr(C)]
#[cfg(target_endian = "little")]
#[derive(Clone, Copy)]
struct PyObjectObFlagsAndRefcnt {
    ob_refcnt: u32,
    ob_overflow: u16,
    ob_flags: u16,
}

#[repr(C)]
#[cfg(target_endian = "big")]
#[derive(Clone, Copy)]
struct PyObjectObFlagsAndRefcnt {
    ob_flags: u16,
    ob_overflow: u16,
    ob_refcnt: u32,
}

#[repr(C)]
struct RawPyObject {
    ob_refcnt: PyObjectObRefcnt,
    ob_type: *mut c_void,
}

#[repr(C)]
struct RawPyVarObject {
    ob_base: RawPyObject,
    ob_size: isize,
}

#[repr(C)]
struct RawPyTupleObject {
    ob_base: RawPyVarObject,
    ob_hash: isize,
    ob_item: [*mut RawPyObject; 1],
}

#[repr(C)]
struct RawPyTypeObject {
    ob_base: RawPyVarObject,
    tp_name: *const u8,
    tp_basicsize: isize,
    tp_itemsize: isize,
    tp_dealloc: *mut c_void,
    tp_vectorcall_offset: isize,
    tp_getattr: *mut c_void,
    tp_setattr: *mut c_void,
    tp_as_async: *mut c_void,
    tp_repr: *mut c_void,
    tp_as_number: *mut c_void,
    tp_as_sequence: *mut c_void,
    tp_as_mapping: *mut c_void,
    tp_hash: *mut c_void,
    tp_call: *mut c_void,
    tp_str: *mut c_void,
    tp_getattro: *mut c_void,
    tp_setattro: *mut c_void,
    tp_as_buffer: *mut c_void,
    tp_flags: usize,
    tp_doc: *const u8,
    tp_traverse: *mut c_void,
    tp_clear: *mut c_void,
    tp_richcompare: *mut c_void,
    tp_weaklistoffset: isize,
    tp_iter: *mut c_void,
    tp_iternext: *mut c_void,
    tp_methods: *mut c_void,
    tp_members: *mut c_void,
    tp_getset: *mut c_void,
    tp_base: *mut c_void,
    tp_dict: *mut c_void,
    tp_descr_get: *mut c_void,
    tp_descr_set: *mut c_void,
    tp_dictoffset: isize,
    tp_init: *mut c_void,
    tp_alloc: *mut c_void,
    tp_new: *mut c_void,
    tp_free: *mut c_void,
    tp_is_gc: *mut c_void,
    tp_bases: *mut c_void,
    tp_mro: *mut c_void,
    tp_cache: *mut c_void,
    tp_subclasses: *mut c_void,
    tp_weaklist: *mut c_void,
    tp_del: *mut c_void,
    tp_version_tag: u32,
    tp_finalize: *mut c_void,
    tp_vectorcall: *mut c_void,
    tp_watched: u8,
    tp_versions_used: u16,
}

#[repr(C)]
struct RawPyHeapTypeObject {
    ht_type: RawPyTypeObject,
    as_async: [*mut c_void; 4],
    as_number: [*mut c_void; 36],
    as_mapping: [*mut c_void; 3],
    as_sequence: [*mut c_void; 10],
    as_buffer: [*mut c_void; 2],
    ht_name: *mut c_void,
    ht_slots: *mut c_void,
    ht_qualname: *mut c_void,
    ht_cached_keys: *mut RawPyDictKeysObject,
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
struct RawPyThreadState {
    prev: *mut c_void,
    next: *mut c_void,
    interp: *mut c_void,
    eval_breaker: usize,
    status: u32,
    holds_gil: i32,
    gil_requested: i32,
    whence: i32,
    state: i32,
    py_recursion_remaining: i32,
    py_recursion_limit: i32,
    recursion_headroom: i32,
    tracing: i32,
    what_event: i32,
    current_frame: *mut c_void,
    base_frame: *mut c_void,
    last_profiled_frame: *mut c_void,
    c_profilefunc: *mut c_void,
    c_tracefunc: *mut c_void,
    c_profileobj: *mut RawPyObject,
    c_traceobj: *mut RawPyObject,
    current_exception: *mut RawPyObject,
}

#[repr(C)]
struct RawPyAsciiObject {
    ob_base: RawPyObject,
    length: isize,
    hash: isize,
    state: u32,
}

#[repr(C)]
struct RawPyBytesObject {
    ob_base: RawPyVarObject,
    ob_shash: isize,
    ob_sval: [u8; 1],
}

#[repr(C)]
struct RawPyByteArrayObject {
    ob_base: RawPyVarObject,
    ob_alloc: isize,
    ob_bytes: *mut u8,
    ob_start: *mut u8,
    ob_exports: isize,
    ob_bytes_object: *mut RawPyObject,
}

unsafe extern "C" {
    fn _Py_Dealloc(obj: *mut RawPyObject);
    fn PyErr_SetNone(exception: *mut c_void);
    fn PyObject_RichCompareBool(left: *mut c_void, right: *mut c_void, opid: c_int) -> c_int;
    fn PyUnicode_FromOrdinal(ordinal: i32) -> *mut c_void;
    fn PyUnicode_GetLength(unicode: *mut c_void) -> isize;
    fn PyUnicode_ReadChar(unicode: *mut c_void, index: isize) -> u32;
    fn PyObject_GetIter(obj: *mut c_void) -> *mut c_void;
    fn PyObject_Size(obj: *mut c_void) -> isize;
    fn PyTuple_New(size: isize) -> *mut c_void;
    fn PyLong_AsLongLong(obj: *mut c_void) -> i64;
    fn PyLong_AsLongLongAndOverflow(obj: *mut c_void, overflow: *mut c_int) -> i64;
    fn memcmp(lhs: *const c_void, rhs: *const c_void, n: usize) -> c_int;
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
    static mut PyExc_TypeError: *mut c_void;
    static mut PyExc_ValueError: *mut c_void;
    static mut _PyDict_IndexedValueTombstone: c_void;
    static mut PyBytes_Type: c_void;
    static mut PyByteArray_Type: c_void;
    static mut PyUnicode_Type: c_void;
}

const PY_TPFLAGS_MANAGED_DICT: usize = 1 << 4;
const PY_TPFLAGS_INLINE_VALUES: usize = 1 << 2;

const MANAGED_DICT_OFFSET: isize = -3 * (core::mem::size_of::<*mut c_void>() as isize);
const PY_UNICODE_STATE_COMPACT_MASK: u32 = 1 << 5;
const PY_UNICODE_STATE_ASCII_MASK: u32 = 1 << 6;

#[inline(always)]
unsafe fn dict_unicode_entries(keys: *mut RawPyDictKeysObject) -> *mut RawPyDictUnicodeEntry {
    let indices = unsafe {
        keys.cast::<u8>()
            .add(core::mem::size_of::<RawPyDictKeysObject>())
    };
    let entries = unsafe { indices.add(1usize << (*keys).dk_log2_index_bytes) };
    entries.cast::<RawPyDictUnicodeEntry>()
}

#[inline(always)]
unsafe fn indexed_key(keys: *mut RawPyDictKeysObject, index: isize) -> *mut RawPyObject {
    unsafe { (*dict_unicode_entries(keys).offset(index)).me_key }
}

#[inline(always)]
unsafe fn dict_key_matches(actual: *mut RawPyObject, expected: *mut RawPyObject) -> bool {
    const PY_EQ: c_int = 2;
    actual == expected
        || (!actual.is_null()
            && !expected.is_null()
            && unsafe {
                PyObject_RichCompareBool(actual.cast::<c_void>(), expected.cast::<c_void>(), PY_EQ)
                    == 1
            })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_compare_compact_ascii_unicode(
    lhs: *mut c_void,
    rhs: *mut c_void,
) -> i32 {
    let lhs = lhs.cast::<RawPyAsciiObject>();
    let rhs = rhs.cast::<RawPyAsciiObject>();
    debug_assert!(!lhs.is_null());
    debug_assert!(!rhs.is_null());
    debug_assert!(
        unsafe { (*lhs).state } & (PY_UNICODE_STATE_COMPACT_MASK | PY_UNICODE_STATE_ASCII_MASK)
            == (PY_UNICODE_STATE_COMPACT_MASK | PY_UNICODE_STATE_ASCII_MASK)
    );
    debug_assert!(
        unsafe { (*rhs).state } & (PY_UNICODE_STATE_COMPACT_MASK | PY_UNICODE_STATE_ASCII_MASK)
            == (PY_UNICODE_STATE_COMPACT_MASK | PY_UNICODE_STATE_ASCII_MASK)
    );

    let lhs_len = unsafe { (*lhs).length } as usize;
    let rhs_len = unsafe { (*rhs).length } as usize;
    let lhs_data = unsafe { lhs.add(1).cast::<u8>() };
    let rhs_data = unsafe { rhs.add(1).cast::<u8>() };
    let min_len = if lhs_len < rhs_len { lhs_len } else { rhs_len };
    if min_len != 0 {
        let compare = unsafe {
            memcmp(
                lhs_data.cast::<c_void>() as *const c_void,
                rhs_data.cast::<c_void>() as *const c_void,
                min_len,
            )
        };
        if compare != 0 {
            return compare;
        }
    }
    if lhs_len == rhs_len {
        0
    } else if lhs_len < rhs_len {
        -1
    } else {
        1
    }
}

#[inline(always)]
unsafe fn can_skip_incref(obj: *mut RawPyObject) -> bool {
    const PY_IMMORTAL_INITIAL_REFCNT: u32 = 3u32 << 30;
    unsafe { (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt >= PY_IMMORTAL_INITIAL_REFCNT }
}

#[inline(always)]
unsafe fn can_skip_decref(obj: *mut RawPyObject) -> bool {
    unsafe { ((*obj).ob_refcnt.refcnt_and_flags.ob_refcnt as i32) < 0 }
}

macro_rules! decref_raw_without_error_preservation {
    ($obj:expr) => {{
        #[allow(unused_unsafe)]
        {
            let obj: *mut RawPyObject = $obj;
            if !obj.is_null() && !unsafe { can_skip_decref(obj) } {
                unsafe {
                    let next_refcnt = (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt.wrapping_sub(1);
                    (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt = next_refcnt;
                    if next_refcnt == 0 {
                        _Py_Dealloc(obj);
                    }
                }
            }
        }
    }};
}

macro_rules! set_raised_exception_direct {
    ($tstate:expr, $exc:expr) => {{
        #[allow(unused_unsafe)]
        {
            let tstate: *mut RawPyThreadState = $tstate;
            debug_assert!(!tstate.is_null());
            let old_exc = unsafe { (*tstate).current_exception };
            unsafe { (*tstate).current_exception = $exc };
            decref_raw_without_error_preservation!(old_exc);
        }
    }};
}

macro_rules! decref_raw_with_tstate {
    ($tstate:expr, $obj:expr) => {{
        let tstate: *mut RawPyThreadState = $tstate;
        let obj: *mut RawPyObject = $obj;
        let _ = unsafe { decref_impl(tstate, obj) };
    }};
}

#[unsafe(no_mangle)]
#[inline(never)]
pub unsafe extern "C" fn soac_runtime_decref_dealloc_preserving_error(
    tstate: *mut c_void,
    obj: *mut c_void,
) {
    let tstate = tstate.cast::<RawPyThreadState>();
    let obj = obj.cast::<RawPyObject>();
    debug_assert!(!tstate.is_null());

    let saved_error = unsafe { (*tstate).current_exception };
    unsafe { (*tstate).current_exception = core::ptr::null_mut() };
    unsafe { _Py_Dealloc(obj) };
    if !saved_error.is_null() {
        set_raised_exception_direct!(tstate, saved_error);
    }
}

#[inline(always)]
unsafe fn decref_impl(tstate: *mut RawPyThreadState, obj: *mut RawPyObject) -> bool {
    if obj.is_null() || unsafe { can_skip_decref(obj) } {
        return false;
    }

    unsafe {
        let next_refcnt = (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt.wrapping_sub(1);
        (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt = next_refcnt;
        if next_refcnt == 0 {
            soac_runtime_decref_dealloc_preserving_error(tstate.cast(), obj.cast());
        }
    }
    true
}

#[inline(always)]
unsafe fn incref_impl(obj: *mut RawPyObject) -> bool {
    if obj.is_null() || unsafe { can_skip_incref(obj) } {
        return false;
    }

    unsafe {
        let cur_refcnt = (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt;
        (*obj).ob_refcnt.refcnt_and_flags.ob_refcnt = cur_refcnt.wrapping_add(1);
    }
    true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_decref(tstate: *mut c_void, obj: *mut c_void) {
    let _ = unsafe { decref_impl(tstate.cast::<RawPyThreadState>(), obj.cast::<RawPyObject>()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_incref(obj: *mut c_void) {
    let _ = unsafe { incref_impl(obj.cast::<RawPyObject>()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_decref_applied(tstate: *mut c_void, obj: *mut c_void) -> i32 {
    if unsafe { decref_impl(tstate.cast::<RawPyThreadState>(), obj.cast::<RawPyObject>()) } {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_incref_applied(obj: *mut c_void) -> i32 {
    if unsafe { incref_impl(obj.cast::<RawPyObject>()) } {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_set_raised_exception(tstate: *mut c_void, exc: *mut c_void) {
    set_raised_exception_direct!(tstate.cast::<RawPyThreadState>(), exc.cast());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_tuple_new(size: isize) -> *mut c_void {
    unsafe { PyTuple_New(size) }
}

// Fresh-tuple construction helper: steals `value`; caller proves the slot is in bounds and unset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_tuple_set_item_stolen(
    tuple: *mut c_void,
    index: isize,
    value: *mut c_void,
) {
    debug_assert!(!tuple.is_null());
    debug_assert!(!value.is_null());
    debug_assert!(index >= 0);
    debug_assert!(index < unsafe { (*tuple.cast::<RawPyTupleObject>()).ob_base.ob_size });
    let items = unsafe {
        tuple
            .cast::<u8>()
            .add(core::mem::offset_of!(RawPyTupleObject, ob_item))
            .cast::<*mut RawPyObject>()
    };
    unsafe {
        *items.offset(index) = value.cast::<RawPyObject>();
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub unsafe extern "C" fn soac_runtime_example_known_value_source() -> i64 {
    core::hint::black_box(7)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub unsafe extern "C" fn soac_runtime_example_offset_known_value() -> i64 {
    (unsafe { soac_runtime_example_known_value_source() }) + 5
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_builtin_ord_i64(
    _tstate: *mut c_void,
    obj: *mut c_void,
) -> i64 {
    debug_assert!(!obj.is_null());
    let mut actual_type = unsafe { (*obj.cast::<RawPyObject>()).ob_type }.cast::<RawPyTypeObject>();
    let bytes_type = core::ptr::addr_of_mut!(PyBytes_Type);
    while !actual_type.is_null() {
        if actual_type.cast::<c_void>() == bytes_type {
            let bytes = obj.cast::<RawPyBytesObject>();
            let size = unsafe { (*bytes).ob_base.ob_size };
            if size != 1 {
                unsafe { PyErr_SetNone(PyExc_TypeError) };
                return 0;
            }
            return unsafe { (*bytes).ob_sval[0] }.into();
        }
        actual_type = unsafe { (*actual_type).tp_base.cast::<RawPyTypeObject>() };
    }
    if unsafe { (*obj.cast::<RawPyObject>()).ob_type } == core::ptr::addr_of_mut!(PyUnicode_Type) {
        let unicode = obj.cast::<RawPyAsciiObject>();
        let state = unsafe { (*unicode).state };
        if state & (PY_UNICODE_STATE_COMPACT_MASK | PY_UNICODE_STATE_ASCII_MASK)
            == (PY_UNICODE_STATE_COMPACT_MASK | PY_UNICODE_STATE_ASCII_MASK)
        {
            if unsafe { (*unicode).length } != 1 {
                unsafe { PyErr_SetNone(PyExc_TypeError) };
                return 0;
            }
            let data = unsafe { unicode.add(1).cast::<u8>() };
            return unsafe { *data }.into();
        }
    }
    let mut actual_type = unsafe { (*obj.cast::<RawPyObject>()).ob_type }.cast::<RawPyTypeObject>();
    let bytearray_type = core::ptr::addr_of_mut!(PyByteArray_Type);
    while !actual_type.is_null() {
        if actual_type.cast::<c_void>() == bytearray_type {
            let bytearray = obj.cast::<RawPyByteArrayObject>();
            let size = unsafe { (*bytearray).ob_base.ob_size };
            if size != 1 {
                unsafe { PyErr_SetNone(PyExc_TypeError) };
                return 0;
            }
            let data = unsafe { (*bytearray).ob_start };
            if data.is_null() {
                return 0;
            }
            return unsafe { *data }.into();
        }
        actual_type = unsafe { (*actual_type).tp_base.cast::<RawPyTypeObject>() };
    }
    let length = unsafe { PyUnicode_GetLength(obj) };
    if length < 0 {
        return 0;
    }
    if length != 1 {
        unsafe { PyErr_SetNone(PyExc_TypeError) };
        return 0;
    }

    let codepoint = unsafe { PyUnicode_ReadChar(obj, 0) };
    if codepoint == u32::MAX {
        return 0;
    }
    codepoint as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_builtin_len_i64(
    _tstate: *mut c_void,
    obj: *mut c_void,
) -> i64 {
    debug_assert!(!obj.is_null());
    unsafe { PyObject_Size(obj) as i64 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_builtin_iter_object(
    _tstate: *mut c_void,
    obj: *mut c_void,
) -> *mut c_void {
    debug_assert!(!obj.is_null());
    unsafe { PyObject_GetIter(obj) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_builtin_chr_i64(
    _tstate: *mut c_void,
    value: i64,
) -> *mut c_void {
    if value < 0 || value > 0x10ffff {
        unsafe { PyErr_SetNone(PyExc_ValueError) };
        return core::ptr::null_mut();
    }
    unsafe { PyUnicode_FromOrdinal(value as i32) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_pylong_as_i64(_tstate: *mut c_void, obj: *mut c_void) -> i64 {
    unsafe { PyLong_AsLongLong(obj) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_pylong_as_i64_saturating(
    tstate: *mut c_void,
    obj: *mut c_void,
) -> i64 {
    let mut overflow = 0;
    let value = unsafe { PyLong_AsLongLongAndOverflow(obj, &mut overflow) };
    if value == -1
        && !unsafe {
            (*tstate.cast::<RawPyThreadState>())
                .current_exception
                .is_null()
        }
    {
        return -1;
    }
    if overflow < 0 {
        return i64::MIN;
    }
    if overflow > 0 {
        return i64::MAX;
    }
    value
}

#[inline(always)]
unsafe fn dict_guarded_index(dict: *mut RawPyDictObject, index: isize) -> i64 {
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
unsafe fn set_indexed_value(
    values: *mut RawPyDictIndexedValues,
    index: isize,
    value: *mut RawPyObject,
) {
    let values_ptr = unsafe { (&raw mut (*values).values).cast::<*mut RawPyObject>() };
    unsafe { *values_ptr.offset(index) = value };
}

#[inline(always)]
unsafe fn split_value(values: *mut RawPyDictSplitValues, index: isize) -> *mut RawPyObject {
    let values_ptr = unsafe { (&raw const (*values).values).cast::<*mut RawPyObject>() };
    unsafe { *values_ptr.offset(index) }
}

#[inline(always)]
unsafe fn set_split_value(
    values: *mut RawPyDictSplitValues,
    index: isize,
    value: *mut RawPyObject,
) {
    let values_ptr = unsafe { (&raw mut (*values).values).cast::<*mut RawPyObject>() };
    unsafe { *values_ptr.offset(index) = value };
}

#[inline(always)]
unsafe fn split_values_insertion_order_array(values: *mut RawPyDictSplitValues) -> *mut u8 {
    let values_ptr = unsafe { (&raw mut (*values).values).cast::<*mut RawPyObject>() };
    unsafe { values_ptr.offset((*values).capacity.into()).cast::<u8>() }
}

#[inline(always)]
unsafe fn add_split_value_to_insertion_order(
    values: *mut RawPyDictSplitValues,
    index: isize,
) -> bool {
    let size = unsafe { (*values).size };
    let capacity = unsafe { (*values).capacity };
    if size >= capacity || index < 0 || index > u8::MAX as isize {
        return false;
    }
    let order = unsafe { split_values_insertion_order_array(values) };
    unsafe { *order.add(size.into()) = index as u8 };
    unsafe { (*values).size = size + 1 };
    true
}

macro_rules! probe_indexed_dict_value {
    ($dict:expr, $index:expr) => {{
        let dict = $dict;
        let index = $index;
        if unsafe { dict_guarded_index(dict, index) } < 0 {
            core::ptr::null_mut()
        } else {
            let values = unsafe { (*dict).ma_values.cast::<RawPyDictIndexedValues>() };
            let value = unsafe { indexed_value(values, index) };
            if value.is_null() || value.cast::<c_void>() == (&raw mut _PyDict_IndexedValueTombstone)
            {
                core::ptr::null_mut()
            } else {
                value
            }
        }
    }};
}

macro_rules! probe_split_values {
    ($keys:expr, $values:expr, $key:expr, $index:expr) => {{
        const DICT_KEYS_SPLIT: u8 = 2;

        let keys = $keys;
        let values = $values;
        let key = $key;
        let index = $index;
        if keys.is_null()
            || values.is_null()
            || unsafe { (*keys).dk_kind } != DICT_KEYS_SPLIT
            || index < 0
            || unsafe { index >= (*values).capacity.into() }
            || !unsafe { dict_key_matches(indexed_key(keys, index), key) }
        {
            core::ptr::null_mut()
        } else {
            unsafe { split_value(values, index) }
        }
    }};
}

macro_rules! probe_split_dict_value {
    ($dict:expr, $key:expr, $index:expr) => {{
        let dict = $dict;
        let key = $key;
        let index = $index;
        let keys = unsafe { (*dict).ma_keys };
        let values = unsafe { (*dict).ma_values.cast::<RawPyDictSplitValues>() };
        probe_split_values!(keys, values, key, index)
    }};
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_probe_global_indexed(
    dict: *mut c_void,
    key: *mut c_void,
    index: isize,
) -> *mut c_void {
    let dict = dict.cast::<RawPyDictObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!dict.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(index >= 0);

    let _ = key;
    probe_indexed_dict_value!(dict, index).cast::<c_void>()
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

    let value = probe_indexed_dict_value!(dict, index);
    if !value.is_null() {
        unsafe { incref_impl(value) };
        return value.cast::<c_void>();
    }

    unsafe { soac_runtime_load_global_slow(dict.cast::<c_void>(), key.cast::<c_void>(), index) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_global_indexed(
    tstate: *mut c_void,
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
    let values = unsafe { (*dict_obj).ma_values.cast::<RawPyDictIndexedValues>() };
    let old_value = unsafe { indexed_value(values, index) };

    // BEHAVIOR_CHANGE: this is a raw slot store for verify/apply JIT code.
    // First insert, insertion order, ma_used, watchers, and versions are skipped.
    let value = value.cast::<RawPyObject>();
    unsafe { incref_impl(value) };
    unsafe { set_indexed_value(values, index, value) };
    if !old_value.is_null()
        && old_value.cast::<c_void>() != (&raw mut _PyDict_IndexedValueTombstone)
    {
        decref_raw_with_tstate!(tstate.cast::<RawPyThreadState>(), old_value);
    }
    unsafe { incref_impl(value) };
    value.cast::<c_void>()
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

macro_rules! object_dict {
    ($obj:expr) => {{
        let obj = $obj;
        let obj_type = unsafe { (*obj).ob_type.cast::<RawPyTypeObject>() };
        let dict = if unsafe { (*obj_type).tp_flags } & PY_TPFLAGS_MANAGED_DICT != 0 {
            let dict_ptr = unsafe {
                obj.cast::<u8>()
                    .offset(MANAGED_DICT_OFFSET)
                    .cast::<*mut RawPyObject>()
            };
            unsafe { *dict_ptr }
        } else {
            let dict_offset = unsafe { (*obj_type).tp_dictoffset };
            if dict_offset <= 0 {
                core::ptr::null_mut()
            } else {
                let dict_ptr = unsafe {
                    obj.cast::<u8>()
                        .offset(dict_offset)
                        .cast::<*mut RawPyObject>()
                };
                unsafe { *dict_ptr }
            }
        };
        if dict.is_null() {
            core::ptr::null_mut()
        } else {
            dict.cast::<RawPyDictObject>()
        }
    }};
}

macro_rules! inline_values {
    ($obj:expr, $obj_type:expr) => {{
        let obj = $obj;
        let obj_type = $obj_type;
        unsafe {
            obj.cast::<u8>()
                .offset((*obj_type).tp_basicsize)
                .cast::<RawPyDictSplitValues>()
        }
    }};
}

macro_rules! cached_keys {
    ($obj_type:expr) => {{ unsafe { (*$obj_type.cast::<RawPyHeapTypeObject>()).ht_cached_keys } }};
}

macro_rules! probe_field_value {
    ($obj:expr, $key:expr, $index:expr) => {{
        let obj = $obj;
        let key = $key;
        let index = $index;
        let dict = object_dict!(obj);
        if !dict.is_null() {
            probe_split_dict_value!(dict, key, index)
        } else {
            let obj_type = unsafe { (*obj).ob_type.cast::<RawPyTypeObject>() };
            if unsafe { (*obj_type).tp_flags } & PY_TPFLAGS_INLINE_VALUES == 0 {
                core::ptr::null_mut()
            } else {
                let values = inline_values!(obj, obj_type);
                if unsafe { (*values).valid } == 0 {
                    core::ptr::null_mut()
                } else {
                    let keys = cached_keys!(obj_type);
                    probe_split_values!(keys, values, key, index)
                }
            }
        }
    }};
}

macro_rules! inline_values_for_unmaterialized_field {
    ($obj:expr) => {{
        let obj = $obj;
        let obj_type = unsafe { (*obj).ob_type.cast::<RawPyTypeObject>() };
        let required_flags = PY_TPFLAGS_INLINE_VALUES | PY_TPFLAGS_MANAGED_DICT;
        if unsafe { (*obj_type).tp_flags } & required_flags != required_flags {
            core::ptr::null_mut()
        } else {
            let dict_ptr = unsafe {
                obj.cast::<u8>()
                    .offset(MANAGED_DICT_OFFSET)
                    .cast::<*mut RawPyObject>()
            };
            if unsafe { !(*dict_ptr).is_null() } {
                core::ptr::null_mut()
            } else {
                let values = inline_values!(obj, obj_type);
                if unsafe { (*values).valid } == 0 {
                    core::ptr::null_mut()
                } else {
                    values
                }
            }
        }
    }};
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_probe_field_indexed(
    obj: *mut c_void,
    key: *mut c_void,
    index: isize,
) -> *mut c_void {
    let obj = obj.cast::<RawPyObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!obj.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(index >= 0);

    probe_field_value!(obj, key, index).cast::<c_void>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_probe_field_indexed_inline_values(
    obj: *mut c_void,
    key: *mut c_void,
    index: isize,
) -> *mut c_void {
    let obj = obj.cast::<RawPyObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!obj.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(index >= 0);

    let values = inline_values_for_unmaterialized_field!(obj);
    if values.is_null() {
        return core::ptr::null_mut();
    }
    let obj_type = unsafe { (*obj).ob_type.cast::<RawPyTypeObject>() };
    let keys = cached_keys!(obj_type);
    probe_split_values!(keys, values, key, index).cast::<c_void>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_field_indexed(
    tstate: *mut c_void,
    obj: *mut c_void,
    key: *mut c_void,
    index: isize,
    value: *mut c_void,
) -> i32 {
    let tstate = tstate.cast::<RawPyThreadState>();
    let obj = obj.cast::<RawPyObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!tstate.is_null());
    debug_assert!(!obj.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(!value.is_null());
    debug_assert!(index >= 0);

    let dict = object_dict!(obj);
    const DICT_KEYS_SPLIT: u8 = 2;

    let (keys, values) = if dict.is_null() {
        let obj_type = unsafe { (*obj).ob_type.cast::<RawPyTypeObject>() };
        if unsafe { (*obj_type).tp_flags } & PY_TPFLAGS_INLINE_VALUES == 0 {
            return 0;
        }
        let values = inline_values!(obj, obj_type);
        if unsafe { (*values).valid } == 0 {
            return 0;
        }
        (cached_keys!(obj_type), values)
    } else {
        (unsafe { (*dict).ma_keys }, unsafe {
            (*dict).ma_values.cast::<RawPyDictSplitValues>()
        })
    };

    if keys.is_null()
        || values.is_null()
        || unsafe { (*keys).dk_kind } != DICT_KEYS_SPLIT
        || unsafe { index >= (*values).capacity.into() }
        || !unsafe { dict_key_matches(indexed_key(keys, index), key) }
    {
        return 0;
    }

    let old_value = unsafe { split_value(values, index) };
    let value = value.cast::<RawPyObject>();
    if old_value.is_null() {
        let size = unsafe { (*values).size };
        let capacity = unsafe { (*values).capacity };
        if size >= capacity || index > u8::MAX as isize {
            return 0;
        }
    }

    // BEHAVIOR_CHANGE: this is a raw split-slot store for verify/apply JIT code.
    // Existing values skip CPython watcher/version bookkeeping. First inserts
    // keep split-value insertion order and split-dict ma_used in sync once the
    // class shared-key layout has already been established.
    unsafe { incref_impl(value) };
    unsafe { set_split_value(values, index, value) };
    if old_value.is_null() {
        if !unsafe { add_split_value_to_insertion_order(values, index) } {
            return 0;
        }
        if !dict.is_null() {
            unsafe { (*dict).ma_used += 1 };
        }
    } else {
        decref_raw_with_tstate!(tstate, old_value);
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_field_indexed_inline_values_trusted(
    tstate: *mut c_void,
    obj: *mut c_void,
    index: isize,
    value: *mut c_void,
) -> i32 {
    let tstate = tstate.cast::<RawPyThreadState>();
    let obj = obj.cast::<RawPyObject>();
    debug_assert!(!tstate.is_null());
    debug_assert!(!obj.is_null());
    debug_assert!(!value.is_null());
    debug_assert!(index >= 0);

    let values = inline_values_for_unmaterialized_field!(obj);
    if values.is_null() || index < 0 || unsafe { index >= (*values).capacity.into() } {
        return 0;
    }

    let old_value = unsafe { split_value(values, index) };
    let value = value.cast::<RawPyObject>();
    if old_value.is_null() {
        let size = unsafe { (*values).size };
        let capacity = unsafe { (*values).capacity };
        if size >= capacity || index > u8::MAX as isize {
            return 0;
        }
        unsafe { incref_impl(value) };
        unsafe { set_split_value(values, index, value) };
        if !unsafe { add_split_value_to_insertion_order(values, index) } {
            return 0;
        }
    } else {
        unsafe { incref_impl(value) };
        unsafe { set_split_value(values, index, value) };
        decref_raw_with_tstate!(tstate, old_value);
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_field_indexed_inline_values(
    tstate: *mut c_void,
    obj: *mut c_void,
    key: *mut c_void,
    index: isize,
    value: *mut c_void,
) -> i32 {
    let tstate = tstate.cast::<RawPyThreadState>();
    let obj = obj.cast::<RawPyObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!tstate.is_null());
    debug_assert!(!obj.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(!value.is_null());
    debug_assert!(index >= 0);

    let values = inline_values_for_unmaterialized_field!(obj);
    if values.is_null() {
        return 0;
    }
    let obj_type = unsafe { (*obj).ob_type.cast::<RawPyTypeObject>() };
    let keys = cached_keys!(obj_type);
    const DICT_KEYS_SPLIT: u8 = 2;
    if keys.is_null()
        || unsafe { (*keys).dk_kind } != DICT_KEYS_SPLIT
        || unsafe { index >= (*values).capacity.into() }
        || !unsafe { dict_key_matches(indexed_key(keys, index), key) }
    {
        return 0;
    }

    let old_value = unsafe { split_value(values, index) };
    let value = value.cast::<RawPyObject>();
    if old_value.is_null() {
        let size = unsafe { (*values).size };
        let capacity = unsafe { (*values).capacity };
        if size >= capacity || index > u8::MAX as isize {
            return 0;
        }
    }

    unsafe { incref_impl(value) };
    unsafe { set_split_value(values, index, value) };
    if old_value.is_null() {
        if !unsafe { add_split_value_to_insertion_order(values, index) } {
            return 0;
        }
    } else {
        decref_raw_with_tstate!(tstate, old_value);
    }
    1
}
