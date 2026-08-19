use pyo3::ffi;
use std::ffi::c_void;
use std::ptr;

const PRESERVED_STATE_CAPSULE_NAME: &std::ffi::CStr = c"soac.PreservedState";

pub const PYOBJECT_OR_NULL_KIND_TAG: i64 = 0;
pub const I64_KIND_TAG: i64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreservedSlotKind {
    PyObjectOrNull,
    I64,
}

impl PreservedSlotKind {
    fn from_tag(tag: i64) -> Option<Self> {
        match tag {
            PYOBJECT_OR_NULL_KIND_TAG => Some(Self::PyObjectOrNull),
            I64_KIND_TAG => Some(Self::I64),
            _ => None,
        }
    }
}

struct PreservedState {
    kinds: Box<[PreservedSlotKind]>,
    // Every payload slot is one machine word. Object slots store owned
    // `PyObject*` values; i64 slots store the integer bits directly.
    values: Box<[u64]>,
}

pub(crate) struct PreservedStateBuilder {
    kinds: Vec<PreservedSlotKind>,
    values: Vec<u64>,
}

impl PreservedStateBuilder {
    pub(crate) fn with_capacity(slot_count: usize) -> Result<Self, ()> {
        let mut kinds = Vec::new();
        let mut values = Vec::new();
        if kinds.try_reserve_exact(slot_count).is_err()
            || values.try_reserve_exact(slot_count).is_err()
        {
            unsafe { ffi::PyErr_NoMemory() };
            return Err(());
        }
        Ok(Self { kinds, values })
    }

    pub(crate) unsafe fn push_owned_object(&mut self, value: *mut ffi::PyObject) {
        debug_assert!(!value.is_null());
        self.kinds.push(PreservedSlotKind::PyObjectOrNull);
        self.values.push(value as usize as u64);
    }

    pub(crate) fn push_i64(&mut self, value: i64) {
        self.kinds.push(PreservedSlotKind::I64);
        self.values.push(value as u64);
    }

    pub(crate) unsafe fn into_capsule(mut self) -> *mut ffi::PyObject {
        let state = Box::new(PreservedState {
            kinds: std::mem::take(&mut self.kinds).into_boxed_slice(),
            values: std::mem::take(&mut self.values).into_boxed_slice(),
        });
        unsafe { capsule_from_preserved_state(state) }
    }
}

impl Drop for PreservedStateBuilder {
    fn drop(&mut self) {
        unsafe { cleanup_partial_state(&self.kinds, &mut self.values) };
    }
}

impl PreservedState {
    unsafe fn clear(&mut self) {
        for (kind, value) in self.kinds.iter().zip(self.values.iter_mut()) {
            if *kind == PreservedSlotKind::PyObjectOrNull && *value != 0 {
                unsafe {
                    ffi::Py_DECREF((*value as usize as *mut ffi::PyObject).cast());
                }
            }
            *value = 0;
        }
    }
}

unsafe extern "C" fn preserved_state_capsule_destructor(capsule: *mut ffi::PyObject) {
    let state_ptr = unsafe {
        ffi::PyCapsule_GetPointer(capsule, PRESERVED_STATE_CAPSULE_NAME.as_ptr())
            as *mut PreservedState
    };
    if state_ptr.is_null() {
        unsafe {
            ffi::PyErr_Clear();
        }
        return;
    }
    let mut state = unsafe { Box::from_raw(state_ptr) };
    unsafe {
        state.clear();
    }
}

unsafe fn capsule_from_preserved_state(state: Box<PreservedState>) -> *mut ffi::PyObject {
    let state_ptr = Box::into_raw(state);
    let capsule = unsafe {
        ffi::PyCapsule_New(
            state_ptr.cast::<c_void>(),
            PRESERVED_STATE_CAPSULE_NAME.as_ptr(),
            Some(preserved_state_capsule_destructor),
        )
    };
    if capsule.is_null() {
        let mut state = unsafe { Box::from_raw(state_ptr) };
        unsafe { state.clear() };
    }
    capsule
}

unsafe fn state_from_capsule(
    capsule: *mut ffi::PyObject,
) -> Result<&'static mut PreservedState, ()> {
    let state_ptr = unsafe {
        ffi::PyCapsule_GetPointer(capsule, PRESERVED_STATE_CAPSULE_NAME.as_ptr())
            as *mut PreservedState
    };
    if state_ptr.is_null() {
        return Err(());
    }
    Ok(unsafe { &mut *state_ptr })
}

unsafe fn py_long_as_i64(value: *mut ffi::PyObject) -> Result<i64, ()> {
    let raw = unsafe { ffi::PyLong_AsLongLong(value) };
    if !unsafe { ffi::PyErr_Occurred() }.is_null() {
        return Err(());
    }
    Ok(raw)
}

unsafe fn slot_index(len: usize, slot: i64) -> Result<usize, ()> {
    let Ok(slot) = usize::try_from(slot) else {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"invalid negative preserved-state slot".as_ptr(),
            );
        }
        return Err(());
    };
    if slot >= len {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"preserved-state slot out of range".as_ptr(),
            );
        }
        return Err(());
    }
    Ok(slot)
}

unsafe fn owned_none() -> *mut ffi::PyObject {
    let none = unsafe { ffi::Py_None() };
    unsafe {
        ffi::Py_INCREF(none);
    }
    none
}

pub unsafe fn new_preserved_state(
    initial_values: *mut ffi::PyObject,
    kind_values: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let slot_count = unsafe { ffi::PyTuple_Size(initial_values) };
    if slot_count < 0 {
        return ptr::null_mut();
    }
    if unsafe { ffi::PyTuple_Size(kind_values) } != slot_count {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_ValueError,
                c"preserved-state kind count did not match value count".as_ptr(),
            );
        }
        return ptr::null_mut();
    }

    let mut kinds = Vec::with_capacity(slot_count as usize);
    let mut values = Vec::with_capacity(slot_count as usize);
    for index in 0..slot_count {
        let kind_obj = unsafe { ffi::PyTuple_GetItem(kind_values, index) };
        if kind_obj.is_null() {
            unsafe {
                cleanup_partial_state(&kinds, &mut values);
            }
            return ptr::null_mut();
        }
        let Ok(kind_tag) = (unsafe { py_long_as_i64(kind_obj) }) else {
            unsafe {
                cleanup_partial_state(&kinds, &mut values);
            }
            return ptr::null_mut();
        };
        let Some(kind) = PreservedSlotKind::from_tag(kind_tag) else {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_ValueError,
                    c"invalid preserved-state slot kind".as_ptr(),
                );
                cleanup_partial_state(&kinds, &mut values);
            }
            return ptr::null_mut();
        };

        let value_obj = unsafe { ffi::PyTuple_GetItem(initial_values, index) };
        if value_obj.is_null() {
            unsafe {
                cleanup_partial_state(&kinds, &mut values);
            }
            return ptr::null_mut();
        }
        let value = match kind {
            PreservedSlotKind::PyObjectOrNull => {
                unsafe {
                    ffi::Py_INCREF(value_obj);
                }
                value_obj as usize as u64
            }
            PreservedSlotKind::I64 => {
                let Ok(value) = (unsafe { py_long_as_i64(value_obj) }) else {
                    unsafe {
                        cleanup_partial_state(&kinds, &mut values);
                    }
                    return ptr::null_mut();
                };
                value as u64
            }
        };
        kinds.push(kind);
        values.push(value);
    }

    let state = Box::new(PreservedState {
        kinds: kinds.into_boxed_slice(),
        values: values.into_boxed_slice(),
    });
    unsafe { capsule_from_preserved_state(state) }
}

unsafe fn cleanup_partial_state(kinds: &[PreservedSlotKind], values: &mut [u64]) {
    for (kind, value) in kinds.iter().zip(values.iter_mut()) {
        if *kind == PreservedSlotKind::PyObjectOrNull && *value != 0 {
            unsafe {
                ffi::Py_DECREF((*value as usize as *mut ffi::PyObject).cast());
            }
        }
        *value = 0;
    }
}

pub unsafe fn load_preserved_state_owned(
    capsule: *mut ffi::PyObject,
    slot: i64,
) -> *mut ffi::PyObject {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        return ptr::null_mut();
    };
    let Ok(slot) = (unsafe { slot_index(state.values.len(), slot) }) else {
        return ptr::null_mut();
    };
    match state.kinds[slot] {
        PreservedSlotKind::PyObjectOrNull => {
            let value = state.values[slot] as usize as *mut ffi::PyObject;
            if value.is_null() {
                unsafe {
                    ffi::PyErr_SetString(
                        ffi::PyExc_UnboundLocalError,
                        c"preserved state is not initialized".as_ptr(),
                    );
                }
                return ptr::null_mut();
            }
            unsafe {
                ffi::Py_INCREF(value);
            }
            value
        }
        PreservedSlotKind::I64 => unsafe { ffi::PyLong_FromLongLong(state.values[slot] as i64) },
    }
}

pub unsafe fn preserved_values_ptr(capsule: *mut ffi::PyObject) -> *mut u64 {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        return ptr::null_mut();
    };
    state.values.as_mut_ptr()
}

pub unsafe fn store_preserved_state(
    capsule: *mut ffi::PyObject,
    slot: i64,
    value: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    if value.is_null() {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"invalid null preserved-state value".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        return ptr::null_mut();
    };
    let Ok(slot) = (unsafe { slot_index(state.values.len(), slot) }) else {
        return ptr::null_mut();
    };
    match state.kinds[slot] {
        PreservedSlotKind::PyObjectOrNull => {
            unsafe {
                ffi::Py_INCREF(value);
            }
            let old_value = state.values[slot] as usize as *mut ffi::PyObject;
            state.values[slot] = value as usize as u64;
            if !old_value.is_null() {
                unsafe {
                    ffi::Py_DECREF(old_value);
                }
            }
        }
        PreservedSlotKind::I64 => {
            let Ok(value) = (unsafe { py_long_as_i64(value) }) else {
                return ptr::null_mut();
            };
            state.values[slot] = value as u64;
        }
    }
    unsafe { owned_none() }
}

pub unsafe fn clear_preserved_slot(capsule: *mut ffi::PyObject, slot: i64) -> i32 {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        return -1;
    };
    let Ok(slot) = (unsafe { slot_index(state.values.len(), slot) }) else {
        return -1;
    };
    match state.kinds[slot] {
        PreservedSlotKind::PyObjectOrNull => {
            let old_value = state.values[slot] as usize as *mut ffi::PyObject;
            if old_value.is_null() {
                return 0;
            }
            state.values[slot] = 0;
            unsafe {
                ffi::Py_DECREF(old_value);
            }
            1
        }
        PreservedSlotKind::I64 => {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"cannot clear scalar preserved-state slot".as_ptr(),
                );
            }
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::Python;

    #[test]
    fn compiler_owned_preserved_state_initializes_raw_slots_without_python_tuples() {
        soac_cpython::initialize_test_python("soac_jit-direct-preserved-state-test")
            .expect("test Python should initialize");
        Python::attach(|_| unsafe {
            let object = ffi::PyList_New(0);
            assert!(!object.is_null(), "object test value should allocate");
            let object_before_state = ffi::Py_REFCNT(object);

            let mut builder = PreservedStateBuilder::with_capacity(3)
                .expect("direct preserved-state slots should allocate");
            ffi::Py_INCREF(object);
            builder.push_owned_object(object);
            builder.push_i64(1);
            builder.push_i64(0);

            let state = builder.into_capsule();
            assert!(
                !state.is_null(),
                "compiler-owned object and scalar slots should create a capsule without tuples"
            );
            assert_eq!(
                ffi::Py_REFCNT(object),
                object_before_state + 1,
                "the direct capsule should own exactly one object reference"
            );
            let values = preserved_values_ptr(state);
            assert_eq!(*values, object as usize as u64);
            assert_eq!(*values.add(1), 1);
            assert_eq!(*values.add(2), 0);

            ffi::Py_DECREF(state);
            assert_eq!(
                ffi::Py_REFCNT(object),
                object_before_state,
                "destroying the capsule should release its owned object exactly once"
            );
            ffi::Py_DECREF(object);

            let abandoned = ffi::PyList_New(0);
            let abandoned_before_builder = ffi::Py_REFCNT(abandoned);
            let mut builder = PreservedStateBuilder::with_capacity(1)
                .expect("partial preserved-state slots should allocate");
            ffi::Py_INCREF(abandoned);
            builder.push_owned_object(abandoned);
            drop(builder);
            assert_eq!(
                ffi::Py_REFCNT(abandoned),
                abandoned_before_builder,
                "abandoned partially initialized state must release every owned object"
            );
            ffi::Py_DECREF(abandoned);
        });
    }

    #[test]
    fn preserved_state_owns_object_slots_and_round_trips_i64_slots() {
        soac_cpython::initialize_test_python("soac_jit-preserved-state-test")
            .expect("test Python should initialize");
        Python::attach(|_| unsafe {
            let object = ffi::PyList_New(0);
            let replacement = ffi::PyList_New(0);
            let initial_i64 = ffi::PyLong_FromLongLong(7);
            let replacement_i64 = ffi::PyLong_FromLongLong(11);
            let pyobject_kind = ffi::PyLong_FromLongLong(PYOBJECT_OR_NULL_KIND_TAG);
            let i64_kind = ffi::PyLong_FromLongLong(I64_KIND_TAG);
            assert!(!object.is_null(), "object test value should allocate");
            assert!(
                !replacement.is_null(),
                "replacement object test value should allocate"
            );
            assert!(
                !initial_i64.is_null() && !replacement_i64.is_null(),
                "integer test values should allocate"
            );
            assert!(
                !pyobject_kind.is_null() && !i64_kind.is_null(),
                "slot-kind test values should allocate"
            );

            let initial_values = ffi::PyTuple_Pack(2, object, initial_i64);
            let kind_values = ffi::PyTuple_Pack(2, pyobject_kind, i64_kind);
            assert!(
                !initial_values.is_null() && !kind_values.is_null(),
                "preserved-state input tuples should allocate"
            );

            let object_before_state = ffi::Py_REFCNT(object);
            let replacement_before_store = ffi::Py_REFCNT(replacement);
            let state = new_preserved_state(initial_values, kind_values);
            assert!(!state.is_null(), "preserved state should allocate");
            assert_eq!(
                ffi::Py_REFCNT(object),
                object_before_state + 1,
                "object slots should own their initial value"
            );
            let values_ptr = preserved_values_ptr(state);
            assert!(
                !values_ptr.is_null(),
                "preserved state should expose raw slot storage"
            );
            assert_eq!(
                *values_ptr, object as usize as u64,
                "object slots should store the owned object pointer directly"
            );
            assert_eq!(
                *values_ptr.add(1),
                7_u64,
                "i64 slots should store their machine integer payload directly"
            );

            let loaded_object = load_preserved_state_owned(state, 0);
            assert_eq!(
                loaded_object, object,
                "object slots should reload the stored object"
            );
            ffi::Py_DECREF(loaded_object);

            let loaded_i64 = load_preserved_state_owned(state, 1);
            assert_eq!(
                py_long_as_i64(loaded_i64),
                Ok(7),
                "i64 slots should round-trip their initial integer payload"
            );
            ffi::Py_DECREF(loaded_i64);

            let store_object_result = store_preserved_state(state, 0, replacement);
            assert!(
                !store_object_result.is_null(),
                "object store should succeed"
            );
            ffi::Py_DECREF(store_object_result);
            assert_eq!(
                ffi::Py_REFCNT(object),
                object_before_state,
                "overwriting an object slot should release the previous value"
            );
            assert_eq!(
                ffi::Py_REFCNT(replacement),
                replacement_before_store + 1,
                "overwriting an object slot should own the replacement"
            );

            let store_i64_result = store_preserved_state(state, 1, replacement_i64);
            assert!(!store_i64_result.is_null(), "i64 store should succeed");
            ffi::Py_DECREF(store_i64_result);
            assert_eq!(
                *values_ptr, replacement as usize as u64,
                "object stores should update raw preserved slot storage in place"
            );
            assert_eq!(
                *values_ptr.add(1),
                11_u64,
                "i64 stores should update raw preserved slot storage in place"
            );
            let reloaded_i64 = load_preserved_state_owned(state, 1);
            assert_eq!(
                py_long_as_i64(reloaded_i64),
                Ok(11),
                "i64 slots should round-trip stored integer payloads"
            );
            ffi::Py_DECREF(reloaded_i64);

            state_from_capsule(state)
                .expect("preserved state should remain valid")
                .clear();
            assert_eq!(
                ffi::Py_REFCNT(replacement),
                replacement_before_store,
                "clearing preserved state should release owned object slots"
            );

            ffi::Py_DECREF(state);
            ffi::Py_DECREF(initial_values);
            ffi::Py_DECREF(kind_values);
            ffi::Py_DECREF(object);
            ffi::Py_DECREF(replacement);
            ffi::Py_DECREF(initial_i64);
            ffi::Py_DECREF(replacement_i64);
            ffi::Py_DECREF(pyobject_kind);
            ffi::Py_DECREF(i64_kind);
        });
    }

    #[test]
    fn clear_preserved_slot_tracks_empty_object_slots_and_rejects_scalar_slots() {
        soac_cpython::initialize_test_python("soac_jit-preserved-state-clear-slot-test")
            .expect("test Python should initialize");
        Python::attach(|_| unsafe {
            let object = ffi::PyList_New(0);
            let initial_i64 = ffi::PyLong_FromLongLong(7);
            let pyobject_kind = ffi::PyLong_FromLongLong(PYOBJECT_OR_NULL_KIND_TAG);
            let i64_kind = ffi::PyLong_FromLongLong(I64_KIND_TAG);
            let initial_values = ffi::PyTuple_Pack(2, object, initial_i64);
            let kinds = ffi::PyTuple_Pack(2, pyobject_kind, i64_kind);
            let state = new_preserved_state(initial_values, kinds);

            assert_eq!(clear_preserved_slot(state, 0), 1);
            assert_eq!(clear_preserved_slot(state, 0), 0);
            assert_eq!(clear_preserved_slot(state, 1), -1);
            assert!(!ffi::PyErr_Occurred().is_null());
            ffi::PyErr_Clear();

            ffi::Py_DECREF(state);
            ffi::Py_DECREF(initial_values);
            ffi::Py_DECREF(kinds);
            ffi::Py_DECREF(object);
            ffi::Py_DECREF(initial_i64);
            ffi::Py_DECREF(pyobject_kind);
            ffi::Py_DECREF(i64_kind);
        });
    }
}
