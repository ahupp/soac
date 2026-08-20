//! Exact native collection insertion ownership, shared by comprehension
//! operations and the inline-only iterator materializer. Containers are
//! borrowed; each non-null key/value operand is consumed on every return.

use super::imports::{ImportSpec, SigType};
use cranelift_jit::JITBuilder;
use pyo3::ffi;
use soac_core::block_py::{BuildCollectionKind, ComprehensionInsertKind};
use std::ptr;

unsafe extern "C" {
    fn _PyList_AppendTakeRefListResize(
        list: *mut ffi::PyListObject,
        item: *mut ffi::PyObject,
    ) -> i32;
    fn _PySet_AddTakeRef(set: *mut ffi::PyObject, item: *mut ffi::PyObject) -> i32;
    fn _PyDict_SetItem_Take2(
        dict: *mut ffi::PyObject,
        key: *mut ffi::PyObject,
        value: *mut ffi::PyObject,
    ) -> i32;
    fn _PyDict_FromItems(
        keys: *const *mut ffi::PyObject,
        key_stride: ffi::Py_ssize_t,
        values: *const *mut ffi::PyObject,
        value_stride: ffi::Py_ssize_t,
        count: ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject;
}

pub(super) static BUILD: ImportSpec = ImportSpec::new(
    "dp_jit_build_collection",
    &[SigType::I32, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);

pub(super) static INSERT: ImportSpec = ImportSpec::new(
    "dp_jit_comprehension_insert",
    &[
        SigType::I32,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::I32],
);

pub(super) const fn kind_tag(kind: ComprehensionInsertKind) -> i32 {
    match kind {
        ComprehensionInsertKind::ListAppend => 0,
        ComprehensionInsertKind::SetAdd => 1,
        ComprehensionInsertKind::DictSetItem => 2,
    }
}

pub(super) const fn build_kind_tag(kind: BuildCollectionKind) -> i32 {
    match kind {
        BuildCollectionKind::List => 0,
        BuildCollectionKind::Set => 1,
        BuildCollectionKind::Dict => 2,
    }
}

unsafe fn release_reverse(values: &mut [*mut ffi::PyObject]) {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    for value in values.iter_mut().rev() {
        let value = std::mem::replace(value, ptr::null_mut());
        unsafe { ffi::Py_XDECREF(value) };
    }
    unsafe { ffi::PyErr_SetRaisedException(error) };
}

/// Shared native construction used by deopt and generated code. The input
/// array owns every non-null entry and is empty on both returns. Nothing here
/// loads an ambient Python helper or creates an intermediate tuple of values.
pub(super) unsafe fn build_owned(
    kind: BuildCollectionKind,
    values: &mut [*mut ffi::PyObject],
) -> *mut ffi::PyObject {
    unsafe {
        dp_jit_build_collection(
            build_kind_tag(kind),
            values.as_mut_ptr(),
            values.len() as ffi::Py_ssize_t,
        )
    }
}

unsafe extern "C" fn dp_jit_build_collection(
    kind: i32,
    values: *mut *mut ffi::PyObject,
    count: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    if count < 0 || (count != 0 && values.is_null()) {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_SystemError,
                c"invalid collection input array".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let values = if count == 0 {
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(values, count as usize) }
    };
    if !(0..=2).contains(&kind)
        || (kind == 2 && values.len() % 2 != 0)
        || values.iter().any(|value| value.is_null())
    {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_SystemError,
                c"invalid native collection shape".as_ptr(),
            );
            release_reverse(values);
        }
        return ptr::null_mut();
    }
    match kind {
        0 => {
            let list = unsafe { ffi::PyList_New(count) };
            if list.is_null() {
                // BUILD_LIST leaves the input stack in place on allocation
                // failure; its enclosing unwind closes that stack in reverse.
                unsafe { release_reverse(values) };
                return ptr::null_mut();
            }
            let list_layout = list.cast::<ffi::PyListObject>();
            for (index, value) in values.iter_mut().enumerate() {
                unsafe {
                    (*list_layout)
                        .ob_item
                        .add(index)
                        .write(std::mem::replace(value, ptr::null_mut()));
                }
            }
            list
        }
        1 => {
            let set = unsafe { ffi::PySet_New(ptr::null_mut()) };
            if set.is_null() {
                unsafe { release_reverse(values) };
                return ptr::null_mut();
            }
            let mut status = 0;
            for value in values {
                let value = std::mem::replace(value, ptr::null_mut());
                if status == 0 {
                    status = unsafe { _PySet_AddTakeRef(set, value) };
                } else {
                    // After insertion has started, BUILD_SET consumes the
                    // remaining values forward, before releasing the set.
                    let error = unsafe { ffi::PyErr_GetRaisedException() };
                    unsafe {
                        ffi::Py_DECREF(value);
                        ffi::PyErr_SetRaisedException(error);
                    }
                }
            }
            if status < 0 {
                let error = unsafe { ffi::PyErr_GetRaisedException() };
                unsafe {
                    ffi::Py_DECREF(set);
                    ffi::PyErr_SetRaisedException(error);
                }
                ptr::null_mut()
            } else {
                set
            }
        }
        2 => {
            let dict = if values.is_empty() {
                unsafe { ffi::PyDict_New() }
            } else {
                unsafe {
                    _PyDict_FromItems(values.as_ptr(), 2, values.as_ptr().add(1), 2, count / 2)
                }
            };
            // _Py_BuildMap_StackRefSteal retains all stack inputs while the
            // exact dictionary is populated, then closes them in reverse.
            unsafe { release_reverse(values) };
            dict
        }
        _ => unreachable!(),
    }
}

unsafe fn reject_owned_inputs(key: *mut ffi::PyObject, value: *mut ffi::PyObject) -> i32 {
    unsafe {
        ffi::PyErr_SetString(
            ffi::PyExc_SystemError,
            c"invalid exact-container comprehension insertion".as_ptr(),
        );
        let error = ffi::PyErr_GetRaisedException();
        // This is an explicit malformed-operation rejection, not a generic
        // Python call. Detach all inputs before their callbacks can reenter.
        ffi::Py_XDECREF(value);
        ffi::Py_XDECREF(key);
        ffi::PyErr_SetRaisedException(error);
    }
    -1
}

/// The GIL branch of _PyList_AppendTakeRef. The native fast path is static
/// inline; its exported resize path consumes item on failure too. Neither
/// branch acquires a second reference to the surviving list or item.
pub(super) unsafe fn append_take_ref(
    list: *mut ffi::PyListObject,
    item: *mut ffi::PyObject,
) -> i32 {
    if list.is_null()
        || item.is_null()
        || unsafe { ffi::Py_TYPE(list.cast()) } != ptr::addr_of_mut!(ffi::PyList_Type)
    {
        return unsafe { reject_owned_inputs(ptr::null_mut(), item) };
    }
    let length = unsafe { (*list).ob_base.ob_size };
    if length < unsafe { (*list).allocated } {
        unsafe {
            (*list).ob_item.offset(length).write(item);
            (*list).ob_base.ob_size = length + 1;
        }
        return 0;
    }
    unsafe { _PyList_AppendTakeRefListResize(list, item) }
}

/// Typed entry used by the deopt interpreter. Validation of the physical
/// Operand owner does not prove its Python type; check that independently.
pub(super) unsafe fn insert_owned(
    kind: ComprehensionInsertKind,
    container: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
) -> i32 {
    unsafe { dp_jit_comprehension_insert(kind_tag(kind), container, key, value) }
}

unsafe extern "C" fn dp_jit_comprehension_insert(
    kind: i32,
    container: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
) -> i32 {
    if container.is_null() || value.is_null() {
        return unsafe { reject_owned_inputs(key, value) };
    }
    let container_type = unsafe { ffi::Py_TYPE(container) };
    match kind {
        0 if key.is_null() && container_type == ptr::addr_of_mut!(ffi::PyList_Type) => unsafe {
            append_take_ref(container.cast(), value)
        },
        1 if key.is_null() && container_type == ptr::addr_of_mut!(ffi::PySet_Type) => unsafe {
            _PySet_AddTakeRef(container, value)
        },
        2 if !key.is_null() && container_type == ptr::addr_of_mut!(ffi::PyDict_Type) => unsafe {
            _PyDict_SetItem_Take2(container, key, value)
        },
        _ => unsafe { reject_owned_inputs(key, value) },
    }
}

pub(super) fn primitive_bindings() -> [(&'static ImportSpec, *const u8); 2] {
    [
        (&BUILD, dp_jit_build_collection as *const u8),
        (&INSERT, dp_jit_comprehension_insert as *const u8),
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
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyModule};

    const SOURCE: &std::ffi::CStr = cr#"
events = []
container = None
error = MemoryError('insertion hash failed')
def observe_container():
    return id(container)
class Key:
    def __init__(self, value): self.value = value
    def __hash__(self):
        events.append(('hash', self.value))
        if container is not None:
            events.append(('container', observe_container()))
        return 1
    def __eq__(self, other):
        events.append(('eq', self.value, other.value))
        if container is not None:
            events.append(('container', observe_container()))
        return self.value == other.value
    def __del__(self): events.append(('drop-key', self.value))
class Value:
    def __init__(self, value): self.value = value
    def __del__(self): events.append(('drop-value', self.value))
class BadKey:
    def __hash__(self): raise error
    def __del__(self): events.append(('drop-bad-key',))
class DerivedList(list): pass
def ordinary(kind):
    if kind == 0:
        return [Key(index) for index in range(2)]
    if kind == 1:
        return {Key(1) for index in range(2)}
    return {Key(1): Value(index) for index in range(2)}
def container_observations():
    return [event[1] for event in events if event[0] == 'container']
def event_observations(recorded, cleanup):
    callbacks = [event for event in recorded if not event[0].startswith('drop-')]
    finalizers = sorted(event for event in recorded if event[0].startswith('drop-'))
    return callbacks, finalizers if cleanup else None
def collection_values(value):
    if type(value) is dict:
        return [(key.value, item.value) for key, item in value.items()]
    items = [item.value for item in value]
    return sorted(items) if type(value) is set else items
def ordinary_build(kind):
    if kind == 0:
        return [Key(0), Key(1), Key(2)]
    if kind == 1:
        return {Key(0), Key(1), Key(2)}
    return {Key(1): Value(0), Key(1): Value(1)}
def ordinary_build_failure(kind):
    if kind == 1:
        return {BadKey(), Value(1), Value(2)}
    return {BadKey(): Value(0), Key(1): Value(1)}
"#;

    fn python(test: impl FnOnce(Python<'_>, Bound<'_, PyModule>)) {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::from_code(py, SOURCE, c"collection.py", c"collection").unwrap();
            test(py, module);
        });
    }

    fn assert_events(
        module: &Bound<'_, PyModule>,
        actual: &Bound<'_, PyAny>,
        expected: &Bound<'_, PyAny>,
        cleanup: bool,
    ) {
        let observe = module.getattr("event_observations").unwrap();
        let actual = observe.call1((actual, cleanup)).unwrap();
        let expected = observe.call1((expected, cleanup)).unwrap();
        assert!(
            actual.eq(&expected).unwrap(),
            "callbacks and required cleanup differ"
        );
    }

    unsafe fn new_collection(py: Python<'_>, kind: ComprehensionInsertKind) -> Bound<'_, PyAny> {
        let object = unsafe {
            match kind {
                ComprehensionInsertKind::ListAppend => ffi::PyList_New(0),
                ComprehensionInsertKind::SetAdd => ffi::PySet_New(ptr::null_mut()),
                ComprehensionInsertKind::DictSetItem => ffi::PyDict_New(),
            }
        };
        unsafe { Bound::from_owned_ptr_or_err(py, object) }.unwrap()
    }

    #[test]
    fn comprehension_insert_preserves_callbacks_values_and_required_cleanup() {
        python(|py, module| unsafe {
            let events = module.getattr("events").unwrap();
            let key_type = module.getattr("Key").unwrap();
            let value_type = module.getattr("Value").unwrap();
            for kind in [
                ComprehensionInsertKind::ListAppend,
                ComprehensionInsertKind::SetAdd,
                ComprehensionInsertKind::DictSetItem,
            ] {
                events.call_method0("clear").unwrap();
                let expected = module
                    .getattr("ordinary")
                    .unwrap()
                    .call1((kind_tag(kind),))
                    .unwrap();
                let expected_live = events.call_method0("copy").unwrap();
                let expected_len = expected.len().unwrap();
                let expected_values = module
                    .getattr("collection_values")
                    .unwrap()
                    .call1((&expected,))
                    .unwrap();
                drop(expected);
                let expected_closed = events.call_method0("copy").unwrap();

                events.call_method0("clear").unwrap();
                let actual = new_collection(py, kind);
                for index in 0..2 {
                    let item_index = if kind == ComprehensionInsertKind::ListAppend {
                        index
                    } else {
                        1
                    };
                    let item = key_type.call1((item_index,)).unwrap().into_ptr();
                    let (key, value) = if kind == ComprehensionInsertKind::DictSetItem {
                        (item, value_type.call1((index,)).unwrap().into_ptr())
                    } else {
                        (ptr::null_mut(), item)
                    };
                    assert_eq!(insert_owned(kind, actual.as_ptr(), key, value), 0);
                }
                assert_eq!(actual.len().unwrap(), expected_len);
                assert!(
                    module
                        .getattr("collection_values")
                        .unwrap()
                        .call1((&actual,))
                        .unwrap()
                        .eq(&expected_values)
                        .unwrap(),
                    "collection values: {kind:?}",
                );
                assert_events(&module, &events, &expected_live, false);
                drop(actual);
                assert_events(&module, &events, &expected_closed, true);
            }
        });
    }

    #[test]
    fn comprehension_insert_preserves_container_identity_without_retention() {
        python(|py, module| unsafe {
            let key_type = module.getattr("Key").unwrap();
            let events = module.getattr("events").unwrap();
            for kind in [
                ComprehensionInsertKind::SetAdd,
                ComprehensionInsertKind::DictSetItem,
            ] {
                events.call_method0("clear").unwrap();
                let container = new_collection(py, kind);
                module.setattr("container", &container).unwrap();
                let expected = module
                    .getattr("observe_container")
                    .unwrap()
                    .call0()
                    .unwrap()
                    .extract::<usize>()
                    .unwrap();
                let owners = ffi::Py_REFCNT(container.as_ptr());
                for _ in 0..2 {
                    let item = key_type.call1((1,)).unwrap().into_ptr();
                    let (key, value) = if kind == ComprehensionInsertKind::DictSetItem {
                        (item, ffi::Py_NewRef(ffi::Py_None()))
                    } else {
                        (ptr::null_mut(), item)
                    };
                    assert_eq!(insert_owned(kind, container.as_ptr(), key, value), 0);
                }
                let observed = module
                    .getattr("container_observations")
                    .unwrap()
                    .call0()
                    .unwrap()
                    .extract::<Vec<usize>>()
                    .unwrap();
                assert_eq!(
                    observed,
                    vec![expected; 3],
                    "two hashes and duplicate equality observe the same container"
                );
                assert_eq!(ffi::Py_REFCNT(container.as_ptr()), owners);
                module.setattr("container", py.None()).unwrap();
                drop(container);
            }
        });
    }

    #[test]
    fn comprehension_insert_failures_consume_inputs_and_preserve_native_error() {
        python(|py, module| unsafe {
            let events = module.getattr("events").unwrap();
            let value_type = module.getattr("Value").unwrap();
            let container = new_collection(py, ComprehensionInsertKind::DictSetItem);
            let key = module
                .getattr("BadKey")
                .unwrap()
                .call0()
                .unwrap()
                .into_ptr();
            let value = value_type.call1((7,)).unwrap().into_ptr();
            let error = module.getattr("error").unwrap();
            assert_eq!(
                insert_owned(
                    ComprehensionInsertKind::DictSetItem,
                    container.as_ptr(),
                    key,
                    value
                ),
                -1
            );
            let pending = ffi::PyErr_GetRaisedException();
            let exact_error = pending == error.as_ptr();
            ffi::Py_XDECREF(pending);
            assert!(exact_error);
            assert_eq!(container.len().unwrap(), 0);
            let globals = PyDict::new(py);
            globals.set_item("events", &events).unwrap();
            assert!(
                py.eval(c"events == [('drop-value', 7)]", Some(&globals), None)
                    .unwrap()
                    .extract::<bool>()
                    .unwrap()
            );
            // The failing __hash__ traceback, not the insertion helper, now
            // owns key. Clearing it releases that final edge immediately.
            error.setattr("__traceback__", py.None()).unwrap();
            assert!(
                py.eval(
                    c"events == [('drop-value', 7), ('drop-bad-key',)]",
                    Some(&globals),
                    None
                )
                .unwrap()
                .extract::<bool>()
                .unwrap()
            );

            events.call_method0("clear").unwrap();
            let wrong_container = module.getattr("DerivedList").unwrap().call0().unwrap();
            let value = value_type.call1((9,)).unwrap().into_ptr();
            assert_eq!(
                insert_owned(
                    ComprehensionInsertKind::ListAppend,
                    wrong_container.as_ptr(),
                    ptr::null_mut(),
                    value
                ),
                -1
            );
            let rejected = PyErr::fetch(py);
            assert!(rejected.is_instance_of::<pyo3::exceptions::PySystemError>(py));
            assert_eq!(wrong_container.len().unwrap(), 0);
            assert!(
                py.eval(c"events == [('drop-value', 9)]", Some(&globals), None)
                    .unwrap()
                    .extract::<bool>()
                    .unwrap()
            );
        });
    }

    #[test]
    fn build_collection_preserves_construction_values_and_duplicate_cleanup() {
        python(|py, module| unsafe {
            let events = module.getattr("events").unwrap();
            let key_type = module.getattr("Key").unwrap();
            let value_type = module.getattr("Value").unwrap();
            for kind in [
                BuildCollectionKind::List,
                BuildCollectionKind::Set,
                BuildCollectionKind::Dict,
            ] {
                events.call_method0("clear").unwrap();
                let expected = module
                    .getattr("ordinary_build")
                    .unwrap()
                    .call1((build_kind_tag(kind),))
                    .unwrap();
                let expected_live = events.call_method0("copy").unwrap();
                let expected_len = expected.len().unwrap();
                let expected_values = module
                    .getattr("collection_values")
                    .unwrap()
                    .call1((&expected,))
                    .unwrap();
                drop(expected);
                let expected_closed = events.call_method0("copy").unwrap();
                events.call_method0("clear").unwrap();
                let mut values = Vec::new();
                if kind == BuildCollectionKind::Dict {
                    for index in 0..2 {
                        values.push(key_type.call1((1,)).unwrap().into_ptr());
                        values.push(value_type.call1((index,)).unwrap().into_ptr());
                    }
                } else {
                    for index in 0..3 {
                        values.push(key_type.call1((index,)).unwrap().into_ptr());
                    }
                }
                let actual =
                    Bound::from_owned_ptr_or_err(py, build_owned(kind, &mut values)).unwrap();
                assert!(values.iter().all(|value| value.is_null()));
                assert_eq!(actual.len().unwrap(), expected_len);
                assert!(
                    module
                        .getattr("collection_values")
                        .unwrap()
                        .call1((&actual,))
                        .unwrap()
                        .eq(&expected_values)
                        .unwrap(),
                    "collection values: {kind:?}",
                );
                assert_events(&module, &events, &expected_live, false);
                drop(actual);
                assert_events(&module, &events, &expected_closed, true);
            }
        });
    }

    #[test]
    fn build_collection_failure_consumes_inputs_and_keeps_exact_error() {
        python(|py, module| unsafe {
            let events = module.getattr("events").unwrap();
            let error = module.getattr("error").unwrap();
            for kind in [BuildCollectionKind::Set, BuildCollectionKind::Dict] {
                events.call_method0("clear").unwrap();
                let ordinary = module
                    .getattr("ordinary_build_failure")
                    .unwrap()
                    .call1((build_kind_tag(kind),))
                    .unwrap_err();
                assert!(ordinary.is_instance_of::<pyo3::exceptions::PyMemoryError>(py));
                let expected_failed = events.call_method0("copy").unwrap();
                error.setattr("__traceback__", py.None()).unwrap();
                drop(ordinary);
                let expected_closed = events.call_method0("copy").unwrap();

                events.call_method0("clear").unwrap();
                let mut values = vec![
                    module
                        .getattr("BadKey")
                        .unwrap()
                        .call0()
                        .unwrap()
                        .into_ptr(),
                ];
                if kind == BuildCollectionKind::Dict {
                    values.push(
                        module
                            .getattr("Value")
                            .unwrap()
                            .call1((0,))
                            .unwrap()
                            .into_ptr(),
                    );
                    values.push(
                        module
                            .getattr("Key")
                            .unwrap()
                            .call1((1,))
                            .unwrap()
                            .into_ptr(),
                    );
                    values.push(
                        module
                            .getattr("Value")
                            .unwrap()
                            .call1((1,))
                            .unwrap()
                            .into_ptr(),
                    );
                } else {
                    for index in 1..3 {
                        values.push(
                            module
                                .getattr("Value")
                                .unwrap()
                                .call1((index,))
                                .unwrap()
                                .into_ptr(),
                        );
                    }
                }
                let result = build_owned(kind, &mut values);
                assert!(result.is_null());
                let pending = ffi::PyErr_GetRaisedException();
                assert_eq!(pending, error.as_ptr());
                ffi::Py_DECREF(pending);
                assert!(values.iter().all(|value| value.is_null()));
                assert_events(&module, &events, &expected_failed, false);
                error.setattr("__traceback__", py.None()).unwrap();
                assert_events(&module, &events, &expected_closed, true);
            }
        });
    }
}
