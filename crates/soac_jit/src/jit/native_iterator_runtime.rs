//! Raw primitives for the versioned inline-only native iterator CFG.
//!
//! No helper here iterates a pipeline or admits a Python function. Calls use
//! canonical C operations; neither runtime.py nor a mutable builtins dictionary
//! supplies an internal operation. The selected pinned CPython build has a GIL.

use super::imports::{ImportSpec, SigType};
use cranelift_jit::JITBuilder;
use pyo3::ffi;
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;

pub(super) const MAP: i32 = 0;
pub(super) const FILTER: i32 = 1;
pub(super) const LIST: i32 = 0;
pub(super) const TUPLE: i32 = 1;

unsafe extern "C" {
    static mut PyMap_Type: ffi::PyTypeObject;
    static mut PyFilter_Type: ffi::PyTypeObject;
    fn _PyTuple_FromArraySteal(
        items: *const *mut ffi::PyObject,
        count: ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject;
    fn _PyList_AsTupleAndClear(list: *mut ffi::PyListObject) -> *mut ffi::PyObject;
}

/// Each non-null entry is exactly one owned edge. This object is an ephemeral
/// native stack buffer, never a capsule or a Python-visible iterator wrapper.
/// On append failure the input is consumed; abort owns all remaining partial
/// result edges. Finish consumes the state even if allocating the tuple fails.
#[repr(C)]
pub(super) struct RawNativeIteratorMaterializer {
    kind: i32,
    list: *mut ffi::PyListObject,
    buffered: ffi::Py_ssize_t,
    items: [*mut ffi::PyObject; 8],
}

pub(super) static GUARD: ImportSpec = ImportSpec::new(
    "dp_jit_native_iterator_guard",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::I32,
        SigType::I32,
    ],
    &[SigType::I32],
);
pub(super) static NEXT_SLOT: ImportSpec = ImportSpec::new(
    "dp_jit_native_iterator_next_slot",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static FILTER_TRUTH: ImportSpec = ImportSpec::new(
    "dp_jit_native_iterator_filter_truth",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);
pub(super) static EXHAUSTED: ImportSpec =
    ImportSpec::new("dp_jit_native_iterator_exhausted", &[], &[SigType::I32]);
pub(super) static MATERIALIZER_INIT: ImportSpec = ImportSpec::new(
    "dp_jit_native_iterator_materializer_init",
    &[SigType::Pointer, SigType::I32],
    &[SigType::I32],
);
pub(super) static MATERIALIZER_APPEND: ImportSpec = ImportSpec::new(
    "dp_jit_native_iterator_materializer_append",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);
pub(super) static MATERIALIZER_FINISH: ImportSpec = ImportSpec::new(
    "dp_jit_native_iterator_materializer_finish",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static MATERIALIZER_ABORT: ImportSpec = ImportSpec::new(
    "dp_jit_native_iterator_materializer_abort",
    &[SigType::Pointer],
    &[],
);
pub(super) static GET_ITER: ImportSpec =
    ImportSpec::new("PyObject_GetIter", &[SigType::Pointer], &[SigType::Pointer]);
pub(super) static MAP_CALL: ImportSpec = ImportSpec::new(
    "PyObject_Vectorcall",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);

unsafe fn decref_preserving_error(value: *mut ffi::PyObject) {
    if !value.is_null() {
        let error = unsafe { ffi::PyErr_GetRaisedException() };
        unsafe {
            ffi::Py_DECREF(value);
            ffi::PyErr_SetRaisedException(error);
        }
    }
}

unsafe extern "C" fn dp_jit_native_iterator_guard(
    materializer: *mut ffi::PyObject,
    stage: *mut ffi::PyObject,
    stage_kind: i32,
    materializer_kind: i32,
) -> i32 {
    let expected_stage = match stage_kind {
        MAP => ptr::addr_of_mut!(PyMap_Type),
        FILTER => ptr::addr_of_mut!(PyFilter_Type),
        _ => return 0,
    };
    let expected_materializer = match materializer_kind {
        LIST => ptr::addr_of_mut!(ffi::PyList_Type),
        TUPLE => ptr::addr_of_mut!(ffi::PyTuple_Type),
        _ => return 0,
    };
    i32::from(stage == expected_stage.cast() && materializer == expected_materializer.cast())
}

unsafe extern "C" fn dp_jit_native_iterator_next_slot(
    iterator: *mut ffi::PyObject,
) -> *const c_void {
    // PyObject_GetIter has already checked the protocol. Reload at each native
    // map/filter next request, not for every rejected filter item.
    unsafe {
        (*ffi::Py_TYPE(iterator))
            .tp_iternext
            .expect("validated native iterator has tp_iternext") as *const c_void
    }
}

unsafe extern "C" fn dp_jit_native_iterator_filter_truth(
    callback: *mut ffi::PyObject,
    item: *mut ffi::PyObject,
) -> i32 {
    if callback == unsafe { ffi::Py_None() }
        || callback == ptr::addr_of_mut!(ffi::PyBool_Type).cast()
    {
        return unsafe { ffi::PyObject_IsTrue(item) };
    }
    let predicate = unsafe { ffi::PyObject_CallOneArg(callback, item) };
    if predicate.is_null() {
        return -1;
    }
    let truth = unsafe { ffi::PyObject_IsTrue(predicate) };
    // filter_next drops the predicate before returning or dropping the item.
    unsafe {
        decref_preserving_error(predicate);
    }
    truth
}

unsafe extern "C" fn dp_jit_native_iterator_exhausted() -> i32 {
    if unsafe { ffi::PyErr_Occurred().is_null() } {
        return 1;
    }
    if unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_StopIteration) } != 0 {
        unsafe {
            ffi::PyErr_Clear();
        }
        return 1;
    }
    0
}

unsafe extern "C" fn dp_jit_native_iterator_materializer_init(
    state: *mut RawNativeIteratorMaterializer,
    kind: i32,
) -> i32 {
    unsafe {
        state.write(RawNativeIteratorMaterializer {
            kind,
            list: ptr::null_mut(),
            buffered: 0,
            items: [ptr::null_mut(); 8],
        });
    }
    if kind == TUPLE {
        return 0;
    }
    assert_eq!(kind, LIST, "validated native iterator materializer kind");
    // The source list_vectorcall uses GenericAlloc, not PyList_New's freelist.
    // This is a surviving result object, so retain its native allocation path.
    let list = unsafe { ffi::PyType_GenericAlloc(ptr::addr_of_mut!(ffi::PyList_Type), 0) }
        .cast::<ffi::PyListObject>();
    if list.is_null() {
        return -1;
    }
    // list(map/filter(...)) asks the exact wrapper for a length hint, not the
    // input. Both canonical types have no length hint, so the native default
    // eight uses list_preallocate_exact's PyMem_Malloc allocation.
    let items = unsafe { ffi::PyMem_Malloc(8 * size_of::<*mut ffi::PyObject>()) }
        .cast::<*mut ffi::PyObject>();
    if items.is_null() {
        unsafe {
            ffi::Py_DECREF(list.cast());
            ffi::PyErr_NoMemory();
        }
        return -1;
    }
    unsafe {
        (*list).ob_item = items;
        (*list).allocated = 8;
        (*state).list = list;
    }
    0
}

unsafe extern "C" fn dp_jit_native_iterator_materializer_append(
    state: *mut RawNativeIteratorMaterializer,
    item: *mut ffi::PyObject,
) -> i32 {
    let state = unsafe { &mut *state };
    if !state.list.is_null() {
        return unsafe { super::collection_runtime::append_take_ref(state.list, item) };
    }
    assert_eq!(state.kind, TUPLE);
    assert!(state.buffered < 8);
    state.items[state.buffered as usize] = item;
    state.buffered += 1;
    if state.buffered != 8 {
        return 0;
    }
    // PySequence_Tuple allocates this buffer after exactly eight items, before
    // asking for a ninth (even when the input has exactly eight items).
    let list = unsafe { ffi::PyList_New(16) }.cast::<ffi::PyListObject>();
    if list.is_null() {
        return -1;
    }
    unsafe {
        (*list).ob_base.ob_size = 8;
    }
    for (index, item) in state.items.iter_mut().enumerate() {
        unsafe {
            (*list)
                .ob_item
                .add(index)
                .write(std::mem::replace(item, ptr::null_mut()));
        }
    }
    state.list = list;
    state.buffered = 0;
    0
}

unsafe fn shrink_native_list(list: *mut ffi::PyListObject) {
    let list = unsafe { &mut *list };
    let size = list.ob_base.ob_size;
    if size >= list.allocated || size >= (list.allocated >> 1) {
        return;
    }
    // Exact GIL branch of the pinned list_resize for newsize == Py_SIZE(list).
    // Shrinking never raises, including allocation failure. Reusing append
    // instead would leave observable __sizeof__ differences for short lists.
    let allocated = if size == 0 {
        0
    } else {
        (size + (size >> 3) + 6) & !3
    };
    let items = unsafe {
        ffi::PyMem_Realloc(
            list.ob_item.cast(),
            (allocated as usize) * size_of::<*mut ffi::PyObject>(),
        )
    }
    .cast::<*mut ffi::PyObject>();
    if !items.is_null() {
        list.ob_item = items;
        list.allocated = allocated;
    }
}

unsafe extern "C" fn dp_jit_native_iterator_materializer_finish(
    state: *mut RawNativeIteratorMaterializer,
) -> *mut ffi::PyObject {
    let state = unsafe { &mut *state };
    let list = std::mem::replace(&mut state.list, ptr::null_mut());
    if state.kind == LIST {
        unsafe {
            shrink_native_list(list);
        }
        return list.cast();
    }
    if !list.is_null() {
        let result = unsafe { _PyList_AsTupleAndClear(list) };
        unsafe {
            decref_preserving_error(list.cast());
        }
        return result;
    }
    let count = std::mem::replace(&mut state.buffered, 0);
    // This native primitive consumes the array even on tuple allocation
    // failure, using its native forward-order allocation-error cleanup.
    unsafe { _PyTuple_FromArraySteal(state.items.as_ptr(), count) }
}

unsafe extern "C" fn dp_jit_native_iterator_materializer_abort(
    state: *mut RawNativeIteratorMaterializer,
) {
    let state = unsafe { &mut *state };
    let list = std::mem::replace(&mut state.list, ptr::null_mut());
    if !list.is_null() {
        unsafe {
            decref_preserving_error(list.cast());
        }
        return;
    }
    // An iteration error before the tuple buffer promotion uses the reverse
    // stack order, unlike _PyTuple_FromArraySteal's allocation-error cleanup.
    while state.buffered > 0 {
        state.buffered -= 1;
        let item = std::mem::replace(&mut state.items[state.buffered as usize], ptr::null_mut());
        unsafe {
            decref_preserving_error(item);
        }
    }
}

/// One inventory for executable addresses and the declarations frozen before
/// parallel code generation. Registering an address alone cannot authorize a
/// worker to add a missing declaration to its immutable module snapshot.
pub(super) fn primitive_bindings() -> [(&'static ImportSpec, *const u8); 10] {
    [
        (&GUARD, dp_jit_native_iterator_guard as *const u8),
        (&NEXT_SLOT, dp_jit_native_iterator_next_slot as *const u8),
        (
            &FILTER_TRUTH,
            dp_jit_native_iterator_filter_truth as *const u8,
        ),
        (&EXHAUSTED, dp_jit_native_iterator_exhausted as *const u8),
        (
            &MATERIALIZER_INIT,
            dp_jit_native_iterator_materializer_init as *const u8,
        ),
        (
            &MATERIALIZER_APPEND,
            dp_jit_native_iterator_materializer_append as *const u8,
        ),
        (
            &MATERIALIZER_FINISH,
            dp_jit_native_iterator_materializer_finish as *const u8,
        ),
        (
            &MATERIALIZER_ABORT,
            dp_jit_native_iterator_materializer_abort as *const u8,
        ),
        (&GET_ITER, ffi::PyObject_GetIter as *const u8),
        (&MAP_CALL, ffi::PyObject_Vectorcall as *const u8),
    ]
}

pub(super) fn register_symbols(builder: &mut JITBuilder) {
    for (spec, address) in primitive_bindings() {
        builder.symbol(spec.symbol, address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::exceptions::PyMemoryError;
    use pyo3::prelude::*;
    use pyo3::types::PyDict;
    use std::ffi::CString;
    use std::mem::MaybeUninit;

    fn python(test: impl FnOnce(Python<'_>)) {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(test);
    }

    #[test]
    fn native_iterator_materializers_match_native_capacity_and_buffer_boundaries() {
        python(|py| unsafe {
            for kind in [LIST, TUPLE] {
                for count in [0, 1, 2, 3, 4, 7, 8, 9, 16, 17, 40] {
                    let mut storage = MaybeUninit::<RawNativeIteratorMaterializer>::uninit();
                    let state = storage.as_mut_ptr();
                    assert_eq!(dp_jit_native_iterator_materializer_init(state, kind), 0);
                    for index in 0..count {
                        let value = ffi::PyLong_FromLong(index);
                        assert!(!value.is_null());
                        assert_eq!(dp_jit_native_iterator_materializer_append(state, value), 0);
                        if kind == TUPLE && index == 7 {
                            assert!(
                                !(*state).list.is_null(),
                                "promotion precedes the ninth next request"
                            );
                            assert_eq!((*(*state).list).ob_base.ob_size, 8);
                            assert_eq!((*(*state).list).allocated, 16);
                            assert_eq!((*state).buffered, 0);
                        }
                    }
                    let result = Bound::<PyAny>::from_owned_ptr_or_err(
                        py,
                        dp_jit_native_iterator_materializer_finish(state),
                    )
                    .unwrap();
                    let sink = if kind == LIST { "list" } else { "tuple" };
                    let expected = py
                        .eval(
                            &CString::new(format!(
                                "{sink}(map(lambda value: value, range({count})))",
                            ))
                            .unwrap(),
                            None,
                            None,
                        )
                        .unwrap();
                    assert!(result.eq(&expected).unwrap());
                    assert_eq!(
                        ffi::Py_TYPE(result.as_ptr()),
                        ffi::Py_TYPE(expected.as_ptr())
                    );
                    assert_eq!(
                        result
                            .call_method0("__sizeof__")
                            .unwrap()
                            .extract::<usize>()
                            .unwrap(),
                        expected
                            .call_method0("__sizeof__")
                            .unwrap()
                            .extract::<usize>()
                            .unwrap(),
                        "native capacity at kind={kind}, length={count}",
                    );
                    assert!((*state).list.is_null());
                    assert_eq!((*state).buffered, 0);
                }
            }
        });
    }

    #[test]
    fn native_iterator_abort_releases_partial_results_and_preserves_exact_error() {
        python(|py| unsafe {
            let globals = PyDict::new(py);
            py.run(
                cr#"
events = []
class Item:
    def __init__(self, value): self.value = value
    def __del__(self): events.append(self.value)
"#,
                Some(&globals),
                None,
            )
            .unwrap();
            let item_type = globals.get_item("Item").unwrap().unwrap();
            let events = globals.get_item("events").unwrap().unwrap();
            for kind in [LIST, TUPLE] {
                for count in [3, 8, 9] {
                    events.call_method0("clear").unwrap();
                    let mut storage = MaybeUninit::<RawNativeIteratorMaterializer>::uninit();
                    let state = storage.as_mut_ptr();
                    assert_eq!(dp_jit_native_iterator_materializer_init(state, kind), 0);
                    for index in 0..count {
                        let item = item_type.call1((index,)).unwrap().into_ptr();
                        assert_eq!(dp_jit_native_iterator_materializer_append(state, item), 0);
                    }
                    assert!(events.extract::<Vec<i32>>().unwrap().is_empty());
                    let error = PyMemoryError::new_err("native materializer iteration failure")
                        .into_value(py);
                    ffi::PyErr_SetRaisedException(ffi::Py_NewRef(error.as_ptr()));
                    dp_jit_native_iterator_materializer_abort(state);
                    let pending = ffi::PyErr_GetRaisedException();
                    let matches = pending == error.as_ptr();
                    ffi::Py_XDECREF(pending);
                    assert!(matches, "cleanup must preserve the exact pending exception");
                    assert_eq!(
                        events.extract::<Vec<i32>>().unwrap(),
                        (0..count).rev().collect::<Vec<_>>()
                    );
                    // Taken state is inert, including after a finalizer callback.
                    dp_jit_native_iterator_materializer_abort(state);
                    assert_eq!(events.extract::<Vec<i32>>().unwrap().len(), count as usize);
                }
            }
        });
    }

    #[test]
    fn native_iterator_guards_require_both_actual_canonical_objects() {
        python(|py| unsafe {
            let builtins = py.import("builtins").unwrap();
            let replacement = py.eval(c"lambda *args: ()", None, None).unwrap();
            for (stage_kind, stage_name) in [(MAP, "map"), (FILTER, "filter")] {
                for (sink_kind, sink_name) in [(LIST, "list"), (TUPLE, "tuple")] {
                    let stage = builtins.getattr(stage_name).unwrap();
                    let sink = builtins.getattr(sink_name).unwrap();
                    assert_eq!(
                        dp_jit_native_iterator_guard(
                            sink.as_ptr(),
                            stage.as_ptr(),
                            stage_kind,
                            sink_kind
                        ),
                        1
                    );
                    assert_eq!(
                        dp_jit_native_iterator_guard(
                            replacement.as_ptr(),
                            stage.as_ptr(),
                            stage_kind,
                            sink_kind
                        ),
                        0
                    );
                    assert_eq!(
                        dp_jit_native_iterator_guard(
                            sink.as_ptr(),
                            replacement.as_ptr(),
                            stage_kind,
                            sink_kind
                        ),
                        0
                    );
                }
            }
        });
    }
}
