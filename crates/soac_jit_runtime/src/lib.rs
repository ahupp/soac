#![no_std]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("soac_jit_runtime raw CPython layout support requires a 64-bit target");

use core::ffi::{c_int, c_void};

const MAX_AFFINE_DISTINCT_PERMUTATION_WIDTH: usize = 8;

/// Count permutations of `0..width` whose `value + index` and `value - index`
/// projections are each distinct. Returns `-1` outside the supported range.
///
/// Keep this exported raw helper self-contained: runtime CLIF loading can
/// resolve other `soac_runtime_*` functions and declared C externs, but not
/// private Rust helpers or panic paths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_count_affine_distinct_permutations_i64(width: i64) -> i64 {
    if width < 0 || width > MAX_AFFINE_DISTINCT_PERMUTATION_WIDTH as i64 {
        return -1;
    }
    let width = width as usize;
    if width == 0 {
        return 1;
    }

    let mut next_values = [0_usize; MAX_AFFINE_DISTINCT_PERMUTATION_WIDTH];
    let mut selected_value_bits = [0_u16; MAX_AFFINE_DISTINCT_PERMUTATION_WIDTH];
    let mut selected_sum_bits = [0_u16; MAX_AFFINE_DISTINCT_PERMUTATION_WIDTH];
    let mut selected_difference_bits = [0_u16; MAX_AFFINE_DISTINCT_PERMUTATION_WIDTH];
    let mut used_values = 0_u16;
    let mut used_sums = 0_u16;
    let mut used_differences = 0_u16;
    let mut index = 0_usize;
    let mut count = 0_i64;

    loop {
        // `width <= 8`, `index` starts at zero, and the only increment is
        // guarded by `index + 1 != width`, so every access is in bounds.
        let next_value = unsafe { next_values.get_unchecked_mut(index) };
        if *next_value == width {
            *next_value = 0;
            if index == 0 {
                break;
            }
            index -= 1;
            used_values ^= unsafe { *selected_value_bits.get_unchecked(index) };
            used_sums ^= unsafe { *selected_sum_bits.get_unchecked(index) };
            used_differences ^= unsafe { *selected_difference_bits.get_unchecked(index) };
            continue;
        }

        let value = *next_value;
        *next_value += 1;
        let value_bit = 1_u16 << value;
        let sum_bit = 1_u16 << (value + index);
        let difference_bit = 1_u16 << (value + width - 1 - index);

        if used_values & value_bit != 0
            || used_sums & sum_bit != 0
            || used_differences & difference_bit != 0
        {
            continue;
        }
        if index + 1 == width {
            count += 1;
            continue;
        }

        unsafe { *selected_value_bits.get_unchecked_mut(index) = value_bit };
        unsafe { *selected_sum_bits.get_unchecked_mut(index) = sum_bit };
        unsafe { *selected_difference_bits.get_unchecked_mut(index) = difference_bit };
        used_values |= value_bit;
        used_sums |= sum_bit;
        used_differences |= difference_bit;
        index += 1;
    }

    count
}

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
    prefix_keys: *mut RawPyDictKeysObject,
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
    fn PyList_AsTuple(list: *mut c_void) -> *mut c_void;
    fn PyLong_AsLongLong(obj: *mut c_void) -> i64;
    fn PyLong_AsLongLongAndOverflow(obj: *mut c_void, overflow: *mut c_int) -> i64;
    fn memcmp(lhs: *const c_void, rhs: *const c_void, n: usize) -> c_int;
    fn _PyDict_SetIndexedItem(dict: *mut c_void, index: isize, value: *mut c_void) -> i32;
    fn soac_runtime_load_global_slow(
        dict: *mut c_void,
        builtins: *mut c_void,
        key: *mut c_void,
        index: isize,
    ) -> *mut c_void;
    fn dp_jit_store_global(
        globals_obj: *mut c_void,
        name: *mut c_void,
        slot_index: i64,
        value: *mut c_void,
    ) -> *mut c_void;
    fn dp_jit_unpack_fixed_slow(
        tstate: *mut c_void,
        iterable: *mut c_void,
        arity: i64,
    ) -> *mut c_void;
    static mut PyExc_TypeError: *mut c_void;
    static mut PyExc_ValueError: *mut c_void;
    static mut _PyDict_IndexedValueTombstone: c_void;
    static mut PyBytes_Type: c_void;
    static mut PyByteArray_Type: c_void;
    static mut PyUnicode_Type: c_void;
    static mut PyTuple_Type: c_void;
    static mut PyList_Type: c_void;
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
pub unsafe extern "C" fn soac_runtime_unpack_fixed(
    tstate: *mut c_void,
    iterable: *mut c_void,
    arity: i64,
) -> *mut c_void {
    debug_assert!(!tstate.is_null());
    debug_assert!(!iterable.is_null());

    let object = iterable.cast::<RawPyObject>();
    let object_type = unsafe { (*object).ob_type };
    if object_type == core::ptr::addr_of_mut!(PyTuple_Type).cast()
        && unsafe { (*iterable.cast::<RawPyVarObject>()).ob_size } as i64 == arity
    {
        unsafe { incref_impl(object) };
        return iterable;
    }
    if object_type == core::ptr::addr_of_mut!(PyList_Type).cast()
        && unsafe { (*iterable.cast::<RawPyVarObject>()).ob_size } as i64 == arity
    {
        return unsafe { PyList_AsTuple(iterable) };
    }

    unsafe { dp_jit_unpack_fixed_slow(tstate, iterable, arity) }
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

// These checks must be part of the exported helper's CLIF, not a private Rust
// call. `inline(always)` is not a guarantee in the rustc-codegen-cranelift
// emission path; only explicit runtime symbols and C externs can be linked.
macro_rules! dict_guarded_index {
    ($dict:expr, $index:expr) => {{
        const DICT_KEYS_INDEXED_UNICODE: u8 = 3;
        const DICT_KEYS_INDEXED_GENERAL: u8 = 4;
        const DICT_LOOKUP_ALIASES: u64 = 1 << 13;

        let dict = $dict;
        let index = $index;
        let keys = unsafe { (*dict).ma_keys };
        let values = unsafe { (*dict).ma_values.cast::<RawPyDictIndexedValues>() };
        if !keys.is_null()
            && !values.is_null()
            && matches!(
                unsafe { (*keys).dk_kind },
                DICT_KEYS_INDEXED_UNICODE | DICT_KEYS_INDEXED_GENERAL
            )
            && unsafe { (*dict).ma_watcher_tag } & DICT_LOOKUP_ALIASES == 0
            && index >= 0
            && unsafe { index < (*values).capacity }
            && !unsafe { (*values).prefix_keys }.is_null()
            && unsafe { index < (*(*values).prefix_keys).dk_nentries }
        {
            index as i64
        } else {
            -1
        }
    }};
}

#[inline(always)]
unsafe fn indexed_value(values: *mut RawPyDictIndexedValues, index: isize) -> *mut RawPyObject {
    let values_ptr = unsafe { (&raw const (*values).values).cast::<*mut RawPyObject>() };
    unsafe { *values_ptr.offset(index) }
}

macro_rules! indexed_name_matches {
    ($values:expr, $index:expr, $key:expr) => {{
        let values: *mut RawPyDictIndexedValues = $values;
        let index = $index;
        let key: *mut RawPyObject = $key;
        let actual = unsafe { indexed_key((*values).prefix_keys, index) };
        actual == key
            || (!key.is_null()
                && unsafe { (*key).ob_type } == (&raw mut PyUnicode_Type).cast()
                && unsafe { dict_key_matches(actual, key) })
    }};
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
    ($dict:expr, $key:expr, $index:expr) => {{
        let dict = $dict;
        let key = $key;
        let index = $index;
        if dict_guarded_index!(dict, index) < 0 {
            core::ptr::null_mut()
        } else {
            let values = unsafe { (*dict).ma_values.cast::<RawPyDictIndexedValues>() };
            if !indexed_name_matches!(values, index, key) {
                core::ptr::null_mut()
            } else {
                // The name comparison only compares exact strings. No Python
                // effect can intervene between the alias guard and this load.
                let value = unsafe { indexed_value(values, index) };
                if value.is_null()
                    || value.cast::<c_void>() == (&raw mut _PyDict_IndexedValueTombstone)
                {
                    core::ptr::null_mut()
                } else {
                    value
                }
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

    probe_indexed_dict_value!(dict, key, index).cast::<c_void>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_load_global(
    dict: *mut c_void,
    builtins: *mut c_void,
    key: *mut c_void,
    index: isize,
) -> *mut c_void {
    let dict = dict.cast::<RawPyDictObject>();
    let key = key.cast::<RawPyObject>();
    debug_assert!(!dict.is_null());
    debug_assert!(!key.is_null());
    debug_assert!(index >= 0);

    let value = probe_indexed_dict_value!(dict, key, index);
    if !value.is_null() {
        unsafe { incref_impl(value) };
        return value.cast::<c_void>();
    }

    unsafe {
        soac_runtime_load_global_slow(dict.cast::<c_void>(), builtins, key.cast::<c_void>(), index)
    }
}

macro_rules! store_global_indexed_body {
    ($tstate:expr, $dict:expr, $key:expr, $index:expr, $value:expr, $value_is_stolen:expr) => {{
        let dict_obj = $dict.cast::<RawPyDictObject>();
        let key = $key.cast::<RawPyObject>();
        debug_assert!(!dict_obj.is_null());
        debug_assert!(!key.is_null());
        debug_assert!(!$value.is_null());
        debug_assert!($index >= 0);

        let value = $value.cast::<RawPyObject>();
        let guarded = dict_guarded_index!(dict_obj, $index) >= 0
            && indexed_name_matches!(unsafe { (*dict_obj).ma_values.cast() }, $index, key);
        // Index/profile facts are not write capabilities. Always use the
        // authoritative native kernel, preserving owner checks, first-insert
        // bookkeeping, watchers, and post-commit finalizer behavior.
        let result = if guarded {
            if unsafe { _PyDict_SetIndexedItem(dict_obj.cast(), $index, value.cast()) } < 0 {
                core::ptr::null_mut()
            } else {
                unsafe { incref_impl(value) };
                value.cast::<c_void>()
            }
        } else {
            unsafe { dp_jit_store_global(dict_obj.cast(), key.cast(), -1, value.cast()) }
        };
        if $value_is_stolen {
            decref_raw_with_tstate!($tstate.cast::<RawPyThreadState>(), value);
        }
        result
    }};
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_global_indexed(
    tstate: *mut c_void,
    dict: *mut c_void,
    key: *mut c_void,
    index: isize,
    value: *mut c_void,
) -> *mut c_void {
    store_global_indexed_body!(tstate, dict, key, index, value, false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_store_global_indexed_stolen(
    tstate: *mut c_void,
    dict: *mut c_void,
    key: *mut c_void,
    index: isize,
    value: *mut c_void,
) -> *mut c_void {
    store_global_indexed_body!(tstate, dict, key, index, value, true)
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

    let guarded_index = if dict_guarded_index!(dict, index) >= 0
        && indexed_name_matches!(unsafe { (*dict).ma_values.cast() }, index, key)
    {
        index as i64
    } else {
        -1
    };
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

// The private sealed-capability match is a separate, dominating runtime guard.
// These macros remain explicit in the exported probe's CLIF; they do not
// manufacture authority from a type pointer, a profile index, or a dict shape.
macro_rules! stable_indexed_receiver_type {
    ($obj:expr, $expected_type:expr) => {{
        let obj: *mut RawPyObject = $obj;
        let expected_type: *mut RawPyTypeObject = $expected_type;
        if obj.is_null()
            || expected_type.is_null()
            || unsafe { (*obj).ob_type.cast::<RawPyTypeObject>() } != expected_type
        {
            core::ptr::null_mut::<RawPyTypeObject>()
        } else {
            let flags = unsafe { (*expected_type).tp_flags };
            if flags & PY_TPFLAGS_INLINE_VALUES != 0
                || (flags & PY_TPFLAGS_MANAGED_DICT == 0
                    && unsafe { (*expected_type).tp_dictoffset <= 0 })
            {
                core::ptr::null_mut::<RawPyTypeObject>()
            } else {
                expected_type
            }
        }
    }};
}

macro_rules! indexed_class_default_dictionary {
    ($owner_type:expr, $mro_index:expr, $namespace_index:expr) => {{
        let owner_type: *mut RawPyTypeObject = $owner_type;
        let mro_index: isize = $mro_index;
        let namespace_index: isize = $namespace_index;
        let mro = unsafe { (*owner_type).tp_mro.cast::<RawPyTupleObject>() };
        if mro.is_null()
            || mro_index < 0
            || namespace_index < 0
            || unsafe { mro_index >= (*mro).ob_base.ob_size }
        {
            core::ptr::null_mut::<RawPyDictObject>()
        } else {
            let classes = unsafe { (&raw const (*mro).ob_item).cast::<*mut RawPyTypeObject>() };
            let declaring_type = unsafe { *classes.offset(mro_index) };
            let dictionary = if declaring_type.is_null() {
                core::ptr::null_mut::<RawPyDictObject>()
            } else {
                // The published locator excludes static builtin namespaces:
                // their dictionaries are per-interpreter, not in tp_dict.
                unsafe { (*declaring_type).tp_dict.cast::<RawPyDictObject>() }
            };
            if dictionary.is_null() || dict_guarded_index!(dictionary, namespace_index) < 0 {
                core::ptr::null_mut::<RawPyDictObject>()
            } else {
                dictionary
            }
        }
    }};
}

macro_rules! plain_class_default_value {
    ($value:expr) => {{
        let value: *mut RawPyObject = $value;
        if value.is_null() {
            false
        } else {
            let value_type = unsafe { (*value).ob_type.cast::<RawPyTypeObject>() };
            !value_type.is_null()
                && unsafe {
                    (*value_type).tp_descr_get.is_null() && (*value_type).tp_descr_set.is_null()
                }
        }
    }};
}

/// Probe one initialized stable prefix slot, returning a borrowed reference or
/// NULL for the original getattr path. This function never raises, allocates,
/// hashes a key, invokes a Python callback, or changes reference ownership.
///
/// # Safety
/// A successful private sealed-field capability match must dominate this call
/// with no intervening Python effect. All locator operands must come from that
/// same live capability; a source/profile offset is not sufficient. The caller
/// keeps the receiver and name alive and INCREFs a hit before releasing either
/// input or allowing a Python effect. The expected-type address alone is not a
/// construction identity and is not safe against address reuse without that
/// dominating match. Both default indices are -1 for NoClassBinding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_probe_stable_indexed_field(
    receiver: *mut c_void,
    expected_type: *mut c_void,
    name: *mut c_void,
    index: isize,
    default_mro_index: isize,
    default_namespace_index: isize,
) -> *mut c_void {
    let receiver = receiver.cast::<RawPyObject>();
    let owner_type = stable_indexed_receiver_type!(receiver, expected_type.cast());
    if owner_type.is_null() || name.is_null() {
        return core::ptr::null_mut();
    }
    let dictionary = object_dict!(receiver);
    if dictionary.is_null() {
        return core::ptr::null_mut();
    }
    // This checks the positive NoLookupAliases condition and actual name, and
    // reloads the current values base. Prefix indices survive overflow growth;
    // the values allocation itself does not. UNSET must use generic getattr.
    let value = probe_indexed_dict_value!(dictionary, name.cast(), index);
    if value.is_null() {
        return core::ptr::null_mut();
    }
    if default_mro_index == -1 && default_namespace_index == -1 {
        return value.cast();
    }
    let default_dictionary =
        indexed_class_default_dictionary!(owner_type, default_mro_index, default_namespace_index);
    if default_dictionary.is_null() {
        return core::ptr::null_mut();
    }
    let default =
        probe_indexed_dict_value!(default_dictionary, name.cast(), default_namespace_index);
    // Even a frozen binding can hold an object whose type gains descriptor
    // slots or whose __class__ changes. Recheck its CURRENT type before using
    // the instance value; a new data descriptor could otherwise take priority.
    if !plain_class_default_value!(default) {
        return core::ptr::null_mut();
    }
    value.cast()
}

/// Read one actual native T_OBJECT_EX member, returning a borrowed reference
/// or NULL for the original attribute operation (including an unbound slot).
/// No dictionary, member lookup, callback, allocation or refcount is involved.
///
/// # Safety
/// The same live sealed-field capability must have matched the exact receiver
/// immediately before this call. Its NativeObjectMember variant supplies the
/// byte offset, whose actual native member and data-descriptor precedence were
/// permanently bound by construction. The caller pins the receiver and INCREFs
/// a hit before any Python effect. A guessed offset or type address is not a
/// capability, including when it passes the defensive range checks below.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_runtime_load_native_object_slot(
    receiver: *mut c_void,
    expected_type: *mut c_void,
    offset: isize,
) -> *mut c_void {
    if receiver.is_null() || expected_type.is_null() {
        return core::ptr::null_mut();
    }
    let object = receiver.cast::<RawPyObject>();
    let actual_type = unsafe { (*object).ob_type.cast::<RawPyTypeObject>() };
    if actual_type.cast::<c_void>() != expected_type
        || offset < core::mem::size_of::<RawPyObject>() as isize
        || offset % core::mem::size_of::<*mut c_void>() as isize != 0
        || offset
            > unsafe { (*actual_type).tp_basicsize } - core::mem::size_of::<*mut c_void>() as isize
    {
        return core::ptr::null_mut();
    }
    unsafe { *receiver.cast::<u8>().offset(offset).cast::<*mut c_void>() }
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
    ($obj:expr, $write:expr) => {{
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
                // PREPARING=2 remains readable by probes but no raw writer
                // may bypass the native attachment transaction.
                if unsafe { (*values).valid } == 0 || ($write && unsafe { (*values).valid } != 1) {
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

    let values = inline_values_for_unmaterialized_field!(obj, false);
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
    const DICT_SOAC_POLICY: u64 = 1 << 12;
    if !dict.is_null() && unsafe { (*dict).ma_watcher_tag } & DICT_SOAC_POLICY != 0 {
        // The generic checked attribute path owns descriptor precedence and
        // native dictionary policy checks; a profile is not authority to skip it.
        return 0;
    }

    let (keys, values) = if dict.is_null() {
        let obj_type = unsafe { (*obj).ob_type.cast::<RawPyTypeObject>() };
        let values = inline_values_for_unmaterialized_field!(obj, true);
        if values.is_null() {
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

    let values = inline_values_for_unmaterialized_field!(obj, true);
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

#[cfg(test)]
mod tests {
    use super::{
        MAX_AFFINE_DISTINCT_PERMUTATION_WIDTH, PY_TPFLAGS_INLINE_VALUES, PY_TPFLAGS_MANAGED_DICT,
        RawPyDictIndexedValues, RawPyDictKeysObject, RawPyDictObject, RawPyObject,
        RawPyTupleObject, RawPyTypeObject,
    };

    #[test]
    fn raw_incref_preserves_object_flags_and_overflow() {
        // This is a POD header fixture, never an exposed CPython object or a
        // late trailer installation. Native tests separately create real
        // stateful allocations with the audited HAS_TYPE_STATE_SLOT bit.
        // Native-linked soac_jit tests exercise the real decrement/deallocator
        // path; this raw crate has no Python linker or fake deallocator stub.
        for flags in [0, 0x10, 0x8010, u16::MAX] {
            let mut object: RawPyObject = unsafe { core::mem::zeroed() };
            object.ob_refcnt = super::PyObjectObRefcnt {
                refcnt_and_flags: super::PyObjectObFlagsAndRefcnt {
                    ob_refcnt: 7,
                    ob_overflow: 0x5a5a,
                    ob_flags: flags,
                },
            };
            let pointer = &raw mut object;
            unsafe {
                assert!(super::incref_impl(pointer));
                assert_eq!(object.ob_refcnt.refcnt_and_flags.ob_refcnt, 8);
                assert_eq!(object.ob_refcnt.refcnt_and_flags.ob_overflow, 0x5a5a);
                assert_eq!(object.ob_refcnt.refcnt_and_flags.ob_flags, flags);
            }
        }
    }

    #[test]
    fn raw_refcount_immortality_ignores_object_layout_flags() {
        for flags in [0, 0x10, 0x8010, u16::MAX] {
            for (count, skip_incref, skip_decref) in [
                (7, false, false),
                (1u32 << 31, false, true),
                (3u32 << 30, true, true),
                (u32::MAX, true, true),
            ] {
                let mut object: RawPyObject = unsafe { core::mem::zeroed() };
                object.ob_refcnt = super::PyObjectObRefcnt {
                    refcnt_and_flags: super::PyObjectObFlagsAndRefcnt {
                        ob_refcnt: count,
                        ob_overflow: 0xa5a5,
                        ob_flags: flags,
                    },
                };
                let pointer = &raw mut object;
                unsafe {
                    assert_eq!(super::can_skip_incref(pointer), skip_incref);
                    assert_eq!(super::can_skip_decref(pointer), skip_decref);
                    if skip_incref {
                        assert!(!super::incref_impl(pointer));
                    }
                    assert_eq!(object.ob_refcnt.refcnt_and_flags.ob_refcnt, count);
                    assert_eq!(object.ob_refcnt.refcnt_and_flags.ob_overflow, 0xa5a5);
                    assert_eq!(object.ob_refcnt.refcnt_and_flags.ob_flags, flags);
                }
            }
        }
    }

    #[test]
    fn inline_storage_preparing_is_readable_but_not_writable() {
        use super::{MANAGED_DICT_OFFSET, RawPyDictSplitValues};

        #[repr(C)]
        struct Receiver {
            preheader: [*mut RawPyObject; 4],
            object: RawPyObject,
            values: RawPyDictSplitValues,
        }
        let mut native_type: RawPyTypeObject = unsafe { core::mem::zeroed() };
        native_type.tp_flags = PY_TPFLAGS_INLINE_VALUES | PY_TPFLAGS_MANAGED_DICT;
        native_type.tp_basicsize = core::mem::size_of::<RawPyObject>() as isize;
        let mut receiver: Receiver = unsafe { core::mem::zeroed() };
        receiver.object.ob_type = (&raw mut native_type).cast();
        let object = &raw mut receiver.object;
        let values = &raw mut receiver.values;
        assert_eq!(
            (values as usize) - (object as usize),
            native_type.tp_basicsize as usize,
        );
        for valid in [0, 1, 2] {
            unsafe { (*values).valid = valid };
            // Both exported raw writers select the writable arm; probes keep
            // observing live native values during callback-scoped preparation.
            let read = inline_values_for_unmaterialized_field!(object, false);
            let write = inline_values_for_unmaterialized_field!(object, true);
            assert_eq!(!read.is_null(), valid != 0);
            assert_eq!(!write.is_null(), valid == 1);
            assert_eq!(unsafe { (*values).valid }, valid);
        }
    }

    #[test]
    fn stable_field_receiver_guard_requires_exact_noninline_dictionary_owner() {
        let mut owner: RawPyTypeObject = unsafe { core::mem::zeroed() };
        owner.tp_dictoffset = core::mem::size_of::<RawPyObject>() as isize;
        let owner_pointer = &raw mut owner;
        let mut receiver: RawPyObject = unsafe { core::mem::zeroed() };
        receiver.ob_type = owner_pointer.cast();
        let receiver_pointer = &raw mut receiver;
        assert_eq!(
            stable_indexed_receiver_type!(receiver_pointer, owner_pointer),
            owner_pointer
        );

        let mut ordinary_child: RawPyTypeObject = unsafe { core::mem::zeroed() };
        ordinary_child.tp_base = owner_pointer.cast();
        ordinary_child.tp_dictoffset = owner.tp_dictoffset;
        unsafe { (*receiver_pointer).ob_type = (&raw mut ordinary_child).cast() };
        assert!(stable_indexed_receiver_type!(receiver_pointer, owner_pointer).is_null());
        unsafe { (*receiver_pointer).ob_type = owner_pointer.cast() };

        unsafe { (*owner_pointer).tp_flags = PY_TPFLAGS_INLINE_VALUES };
        assert!(stable_indexed_receiver_type!(receiver_pointer, owner_pointer).is_null());
        unsafe {
            (*owner_pointer).tp_flags = 0;
            (*owner_pointer).tp_dictoffset = -8;
        }
        assert!(stable_indexed_receiver_type!(receiver_pointer, owner_pointer).is_null());
        unsafe { (*owner_pointer).tp_flags = PY_TPFLAGS_MANAGED_DICT };
        assert_eq!(
            stable_indexed_receiver_type!(receiver_pointer, owner_pointer),
            owner_pointer
        );
        assert!(stable_indexed_receiver_type!(core::ptr::null_mut(), owner_pointer).is_null());
        assert!(stable_indexed_receiver_type!(receiver_pointer, core::ptr::null_mut()).is_null());
    }

    #[test]
    fn stable_class_default_locator_uses_reserved_prefix_bounds_not_visible_order() {
        let mut keys: RawPyDictKeysObject = unsafe { core::mem::zeroed() };
        keys.dk_kind = 3;
        let keys_pointer = &raw mut keys;
        let mut prefix: RawPyDictKeysObject = unsafe { core::mem::zeroed() };
        prefix.dk_nentries = 3;
        // This guard inspects headers only. One visible entry can occupy
        // prefix index two after earlier namespace bindings were deleted.
        let mut values = RawPyDictIndexedValues {
            capacity: 3,
            order_size: 1,
            prefix_keys: &raw mut prefix,
            values: [core::ptr::null_mut()],
        };
        let mut dictionary: RawPyDictObject = unsafe { core::mem::zeroed() };
        dictionary.ma_keys = keys_pointer;
        dictionary.ma_values = (&raw mut values).cast();
        let dictionary_pointer = &raw mut dictionary;
        let mut declaring_type: RawPyTypeObject = unsafe { core::mem::zeroed() };
        declaring_type.tp_dict = dictionary_pointer.cast();
        let declaring_pointer = &raw mut declaring_type;
        let mut mro: RawPyTupleObject = unsafe { core::mem::zeroed() };
        mro.ob_base.ob_size = 1;
        mro.ob_item[0] = declaring_pointer.cast();
        let mut owner: RawPyTypeObject = unsafe { core::mem::zeroed() };
        owner.tp_mro = (&raw mut mro).cast();
        let owner_pointer = &raw mut owner;

        assert_eq!(
            indexed_class_default_dictionary!(owner_pointer, 0, 2),
            dictionary_pointer
        );
        assert!(indexed_class_default_dictionary!(owner_pointer, 1, 2).is_null());
        assert!(indexed_class_default_dictionary!(owner_pointer, 0, 3).is_null());
        assert!(indexed_class_default_dictionary!(owner_pointer, -1, 2).is_null());
        unsafe { (*dictionary_pointer).ma_watcher_tag = 1 << 13 };
        assert!(indexed_class_default_dictionary!(owner_pointer, 0, 2).is_null());
        unsafe { (*dictionary_pointer).ma_watcher_tag = 0 };
        for ordinary_kind in [0, 1, 2] {
            unsafe { (*keys_pointer).dk_kind = ordinary_kind };
            assert!(indexed_class_default_dictionary!(owner_pointer, 0, 2).is_null());
        }
        unsafe { (*keys_pointer).dk_kind = 4 };
        assert_eq!(
            indexed_class_default_dictionary!(owner_pointer, 0, 2),
            dictionary_pointer
        );
        unsafe { (*declaring_pointer).tp_dict = core::ptr::null_mut() };
        assert!(indexed_class_default_dictionary!(owner_pointer, 0, 2).is_null());
    }

    #[test]
    fn stable_class_default_guard_rechecks_slots_and_current_value_type() {
        let mut plain_type: RawPyTypeObject = unsafe { core::mem::zeroed() };
        let plain_pointer = &raw mut plain_type;
        let mut value: RawPyObject = unsafe { core::mem::zeroed() };
        value.ob_type = plain_pointer.cast();
        let value_pointer = &raw mut value;
        assert!(plain_class_default_value!(value_pointer));

        // Only nullness is inspected; no descriptor callback is executed.
        let mut callback_marker = 0_u8;
        unsafe { (*plain_pointer).tp_descr_get = (&raw mut callback_marker).cast() };
        assert!(!plain_class_default_value!(value_pointer));
        unsafe {
            (*plain_pointer).tp_descr_get = core::ptr::null_mut();
            (*plain_pointer).tp_descr_set = (&raw mut callback_marker).cast();
        }
        assert!(!plain_class_default_value!(value_pointer));
        unsafe { (*plain_pointer).tp_descr_set = core::ptr::null_mut() };
        assert!(plain_class_default_value!(value_pointer));

        let mut descriptor_type: RawPyTypeObject = unsafe { core::mem::zeroed() };
        descriptor_type.tp_descr_get = (&raw mut callback_marker).cast();
        unsafe { (*value_pointer).ob_type = (&raw mut descriptor_type).cast() };
        assert!(!plain_class_default_value!(value_pointer));
        assert!(!plain_class_default_value!(core::ptr::null_mut()));
    }

    #[test]
    fn indexed_lookup_guard_requires_a_reserved_slot_without_aliases() {
        use super::{RawPyDictIndexedValues, RawPyDictKeysObject, RawPyDictObject};

        // Only the headers are inspected by this guard; no Python API is used.
        let mut keys: RawPyDictKeysObject = unsafe { core::mem::zeroed() };
        let mut prefix: RawPyDictKeysObject = unsafe { core::mem::zeroed() };
        prefix.dk_nentries = 1;
        let keys_ptr = &raw mut keys;
        let prefix_ptr = &raw mut prefix;
        let mut values = RawPyDictIndexedValues {
            capacity: 1,
            order_size: 0,
            prefix_keys: prefix_ptr,
            values: [core::ptr::null_mut()],
        };
        let mut dict: RawPyDictObject = unsafe { core::mem::zeroed() };
        dict.ma_keys = keys_ptr;
        dict.ma_values = (&raw mut values).cast();

        for kind in [3, 4] {
            unsafe { (*keys_ptr).dk_kind = kind };
            assert_eq!(dict_guarded_index!(&raw mut dict, 0), 0);
            assert_eq!(dict_guarded_index!(&raw mut dict, -1), -1);
            assert_eq!(dict_guarded_index!(&raw mut dict, 1), -1);
            dict.ma_watcher_tag = 1 << 13;
            assert_eq!(dict_guarded_index!(&raw mut dict, 0), -1);
            dict.ma_watcher_tag = 0;
            unsafe { (*prefix_ptr).dk_nentries = 0 };
            assert_eq!(dict_guarded_index!(&raw mut dict, 0), -1);
            unsafe { (*prefix_ptr).dk_nentries = 1 };
        }
        unsafe { (*keys_ptr).dk_kind = 2 };
        assert_eq!(dict_guarded_index!(&raw mut dict, 0), -1);
    }

    fn count_affine_distinct_permutations(width: usize) -> Option<u64> {
        let Ok(width) = i64::try_from(width) else {
            return None;
        };
        let count = unsafe { super::soac_runtime_count_affine_distinct_permutations_i64(width) };
        (count >= 0).then_some(count as u64)
    }

    #[test]
    fn affine_distinct_permutation_counts_match_known_widths() {
        let expected = [1, 1, 0, 0, 2, 10, 4, 40, 92];
        for (width, expected) in expected.into_iter().enumerate() {
            assert_eq!(
                count_affine_distinct_permutations(width),
                Some(expected),
                "width {width}"
            );
        }
    }

    #[test]
    fn affine_distinct_permutation_count_rejects_invalid_widths() {
        assert_eq!(
            count_affine_distinct_permutations(MAX_AFFINE_DISTINCT_PERMUTATION_WIDTH + 1),
            None
        );
        assert_eq!(count_affine_distinct_permutations(usize::MAX), None);
    }

    #[test]
    fn affine_distinct_permutation_abi_returns_exact_counts() {
        assert_eq!(
            unsafe { super::soac_runtime_count_affine_distinct_permutations_i64(0) },
            1
        );
        assert_eq!(
            unsafe { super::soac_runtime_count_affine_distinct_permutations_i64(4) },
            2
        );
        assert_eq!(
            unsafe { super::soac_runtime_count_affine_distinct_permutations_i64(8) },
            92
        );
    }

    #[test]
    fn affine_distinct_permutation_abi_rejects_invalid_widths() {
        assert_eq!(
            unsafe { super::soac_runtime_count_affine_distinct_permutations_i64(-1) },
            -1
        );
        assert_eq!(
            unsafe { super::soac_runtime_count_affine_distinct_permutations_i64(9) },
            -1
        );
        assert_eq!(
            unsafe { super::soac_runtime_count_affine_distinct_permutations_i64(i64::MAX) },
            -1
        );
    }
}
