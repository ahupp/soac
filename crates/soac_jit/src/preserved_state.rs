use pyo3::ffi;
use std::ffi::{c_int, c_void};
use std::ptr;

const PRESERVED_STATE_CAPSULE_NAME: &std::ffi::CStr = c"soac.PreservedState";
const PRESERVED_KIND_BITS: usize = u64::BITS as usize;

unsafe extern "C" {
    fn _PyCapsule_SetTraverse(
        capsule: *mut ffi::PyObject,
        traverse: ffi::traverseproc,
        clear: ffi::inquiry,
    ) -> c_int;
}

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
    // The original contiguous payload words come first, followed by one
    // object-kind bit per slot. Keeping the Vec avoids a second allocation.
    storage: Vec<u64>,
}

pub(crate) struct PreservedStateBuilder {
    storage: Vec<u64>,
    slot_count: usize,
    initialized_slots: usize,
}

impl PreservedStateBuilder {
    pub(crate) fn with_capacity(slot_count: usize) -> Result<Self, ()> {
        let kind_words =
            slot_count / PRESERVED_KIND_BITS + usize::from(slot_count % PRESERVED_KIND_BITS != 0);
        let Some(word_count) = slot_count.checked_add(kind_words) else {
            unsafe { ffi::PyErr_NoMemory() };
            return Err(());
        };
        let mut storage = Vec::new();
        if storage.try_reserve_exact(word_count).is_err() {
            unsafe { ffi::PyErr_NoMemory() };
            return Err(());
        }
        storage.resize(word_count, 0);
        Ok(Self {
            storage,
            slot_count,
            initialized_slots: 0,
        })
    }

    pub(crate) unsafe fn push_owned_object(&mut self, value: *mut ffi::PyObject) {
        debug_assert!(!value.is_null());
        debug_assert!(self.initialized_slots < self.slot_count);
        let slot = self.initialized_slots;
        self.storage[slot] = value as usize as u64;
        self.storage[self.slot_count + slot / PRESERVED_KIND_BITS] |=
            1_u64 << (slot % PRESERVED_KIND_BITS);
        self.initialized_slots += 1;
    }

    pub(crate) fn push_i64(&mut self, value: i64) {
        debug_assert!(self.initialized_slots < self.slot_count);
        self.storage[self.initialized_slots] = value as u64;
        self.initialized_slots += 1;
    }

    pub(crate) unsafe fn into_capsule(mut self) -> *mut ffi::PyObject {
        debug_assert_eq!(self.initialized_slots, self.slot_count);
        let state = Box::new(PreservedState {
            storage: std::mem::take(&mut self.storage),
        });
        self.initialized_slots = 0;
        unsafe { capsule_from_preserved_state(state) }
    }
}

impl Drop for PreservedStateBuilder {
    fn drop(&mut self) {
        let storage = self.storage.as_mut_ptr();
        for slot in 0..self.initialized_slots {
            let kind_word = unsafe { *storage.add(self.slot_count + slot / PRESERVED_KIND_BITS) };
            if kind_word & (1_u64 << (slot % PRESERVED_KIND_BITS)) == 0 {
                continue;
            }
            let value_slot = unsafe { storage.add(slot) };
            let value = unsafe { *value_slot };
            if value != 0 {
                unsafe {
                    *value_slot = 0;
                    ffi::Py_DECREF(value as usize as *mut ffi::PyObject);
                }
            }
        }
    }
}

impl PreservedState {
    fn slot_count(&self) -> usize {
        let word_count = self.storage.len();
        // n payload words require ceil(n / 64) kind words; equivalently the
        // packed allocation has ceil(word_count / 65) trailing kind words.
        let kind_words = word_count / (PRESERVED_KIND_BITS + 1)
            + usize::from(word_count % (PRESERVED_KIND_BITS + 1) != 0);
        word_count - kind_words
    }

    fn slot_kind(&self, slot: usize) -> PreservedSlotKind {
        let kind_word = self.storage[self.slot_count() + slot / PRESERVED_KIND_BITS];
        if kind_word & (1_u64 << (slot % PRESERVED_KIND_BITS)) != 0 {
            PreservedSlotKind::PyObjectOrNull
        } else {
            PreservedSlotKind::I64
        }
    }

    unsafe fn clear(state: *mut Self) {
        let slot_count = unsafe { (*state).slot_count() };
        let storage = unsafe { (*state).storage.as_mut_ptr() };
        for slot in 0..slot_count {
            let kind_word = unsafe { *storage.add(slot_count + slot / PRESERVED_KIND_BITS) };
            let value_slot = unsafe { storage.add(slot) };
            let value = unsafe { *value_slot };
            // Py_CLEAR ordering is required: DECREF can run arbitrary Python
            // finalizers which reenter this same preserved-state capsule.
            unsafe { *value_slot = 0 };
            if kind_word & (1_u64 << (slot % PRESERVED_KIND_BITS)) != 0 && value != 0 {
                unsafe { ffi::Py_DECREF(value as usize as *mut ffi::PyObject) };
            }
        }
    }
}

unsafe extern "C" fn preserved_state_capsule_traverse(
    capsule: *mut ffi::PyObject,
    visit: ffi::visitproc,
    arg: *mut c_void,
) -> c_int {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        unsafe { ffi::PyErr_Clear() };
        return 0;
    };
    let slot_count = unsafe { (*state).slot_count() };
    let storage = unsafe { (*state).storage.as_ptr() };
    for slot in 0..slot_count {
        let kind_word = unsafe { *storage.add(slot_count + slot / PRESERVED_KIND_BITS) };
        if kind_word & (1_u64 << (slot % PRESERVED_KIND_BITS)) == 0 {
            continue;
        }
        let value = unsafe { *storage.add(slot) } as usize as *mut ffi::PyObject;
        if !value.is_null() {
            let result = unsafe { visit(value, arg) };
            if result != 0 {
                return result;
            }
        }
    }
    0
}

unsafe extern "C" fn preserved_state_capsule_clear(capsule: *mut ffi::PyObject) -> c_int {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        unsafe { ffi::PyErr_Clear() };
        return 0;
    };
    unsafe { PreservedState::clear(state) };
    0
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
    unsafe { PreservedState::clear(state_ptr) };
    drop(unsafe { Box::from_raw(state_ptr) });
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
        unsafe { PreservedState::clear(state_ptr) };
        drop(unsafe { Box::from_raw(state_ptr) });
        return ptr::null_mut();
    }
    if unsafe {
        _PyCapsule_SetTraverse(
            capsule,
            preserved_state_capsule_traverse,
            preserved_state_capsule_clear,
        )
    } != 0
    {
        // The successful capsule now owns the state; its destructor releases
        // the Box and object slots exactly once on this failure path.
        unsafe { ffi::Py_DECREF(capsule) };
        return ptr::null_mut();
    }
    capsule
}

unsafe fn state_from_capsule(capsule: *mut ffi::PyObject) -> Result<*mut PreservedState, ()> {
    let state_ptr = unsafe {
        ffi::PyCapsule_GetPointer(capsule, PRESERVED_STATE_CAPSULE_NAME.as_ptr())
            as *mut PreservedState
    };
    if state_ptr.is_null() {
        return Err(());
    }
    Ok(state_ptr)
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

    let Ok(mut state) = PreservedStateBuilder::with_capacity(slot_count as usize) else {
        return ptr::null_mut();
    };
    for index in 0..slot_count {
        let kind_obj = unsafe { ffi::PyTuple_GetItem(kind_values, index) };
        if kind_obj.is_null() {
            return ptr::null_mut();
        }
        let Ok(kind_tag) = (unsafe { py_long_as_i64(kind_obj) }) else {
            return ptr::null_mut();
        };
        let Some(kind) = PreservedSlotKind::from_tag(kind_tag) else {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_ValueError,
                    c"invalid preserved-state slot kind".as_ptr(),
                );
            }
            return ptr::null_mut();
        };

        let value_obj = unsafe { ffi::PyTuple_GetItem(initial_values, index) };
        if value_obj.is_null() {
            return ptr::null_mut();
        }
        match kind {
            PreservedSlotKind::PyObjectOrNull => unsafe {
                ffi::Py_INCREF(value_obj);
                state.push_owned_object(value_obj);
            },
            PreservedSlotKind::I64 => {
                let Ok(value) = (unsafe { py_long_as_i64(value_obj) }) else {
                    return ptr::null_mut();
                };
                state.push_i64(value);
            }
        }
    }

    unsafe { state.into_capsule() }
}

pub unsafe fn load_preserved_state_owned(
    capsule: *mut ffi::PyObject,
    slot: i64,
) -> *mut ffi::PyObject {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        return ptr::null_mut();
    };
    let Ok(slot) = (unsafe { slot_index((*state).slot_count(), slot) }) else {
        return ptr::null_mut();
    };
    match unsafe { (*state).slot_kind(slot) } {
        PreservedSlotKind::PyObjectOrNull => {
            let value =
                unsafe { *(*state).storage.as_ptr().add(slot) } as usize as *mut ffi::PyObject;
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
        PreservedSlotKind::I64 => unsafe {
            ffi::PyLong_FromLongLong(*(*state).storage.as_ptr().add(slot) as i64)
        },
    }
}

pub unsafe fn preserved_values_ptr(capsule: *mut ffi::PyObject) -> *mut u64 {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        return ptr::null_mut();
    };
    unsafe { (*state).storage.as_mut_ptr() }
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
    let Ok(slot) = (unsafe { slot_index((*state).slot_count(), slot) }) else {
        return ptr::null_mut();
    };
    match unsafe { (*state).slot_kind(slot) } {
        PreservedSlotKind::PyObjectOrNull => {
            unsafe {
                ffi::Py_INCREF(value);
            }
            let value_slot = unsafe { (*state).storage.as_mut_ptr().add(slot) };
            let old_value = unsafe { *value_slot } as usize as *mut ffi::PyObject;
            unsafe { *value_slot = value as usize as u64 };
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
            unsafe { *(*state).storage.as_mut_ptr().add(slot) = value as u64 };
        }
    }
    unsafe { owned_none() }
}

pub unsafe fn clear_preserved_slot(capsule: *mut ffi::PyObject, slot: i64) -> i32 {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        return -1;
    };
    let Ok(slot) = (unsafe { slot_index((*state).slot_count(), slot) }) else {
        return -1;
    };
    match unsafe { (*state).slot_kind(slot) } {
        PreservedSlotKind::PyObjectOrNull => {
            let value_slot = unsafe { (*state).storage.as_mut_ptr().add(slot) };
            let old_value = unsafe { *value_slot } as usize as *mut ffi::PyObject;
            if old_value.is_null() {
                return 0;
            }
            unsafe { *value_slot = 0 };
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

    unsafe extern "C" fn collect_preserved_object_visit(
        object: *mut ffi::PyObject,
        observed: *mut c_void,
    ) -> i32 {
        unsafe { (*observed.cast::<Vec<usize>>()).push(object as usize) };
        0
    }

    unsafe extern "C" fn reject_preserved_object_visit(
        _object: *mut ffi::PyObject,
        _observed: *mut c_void,
    ) -> i32 {
        37
    }

    #[test]
    fn compact_preserved_state_tracks_owned_objects_and_cells_across_bitmap_words() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        soac_cpython::initialize_test_python("soac_jit-compact-preserved-state-gc-test")
            .expect("test Python should initialize");
        Python::attach(|_| unsafe {
            let object = ffi::PyList_New(0);
            assert!(!object.is_null(), "object test value should allocate");
            let cell = crate::PyCell_New(object);
            assert!(!cell.is_null(), "owned lexical cell should allocate");
            let object_before_state = ffi::Py_REFCNT(object);
            let cell_before_state = ffi::Py_REFCNT(cell);

            const SLOT_COUNT: usize = 130;
            const OBJECT_SLOTS: [usize; 7] = [0, 63, 64, 65, 127, 128, 129];
            let mut builder = PreservedStateBuilder::with_capacity(SLOT_COUNT)
                .expect("packed preserved-state slots should allocate once");
            let mut expected_objects = Vec::new();
            for index in 0..SLOT_COUNT {
                if OBJECT_SLOTS.contains(&index) {
                    let value = if index == 65 || index == 129 {
                        cell
                    } else {
                        object
                    };
                    ffi::Py_INCREF(value);
                    builder.push_owned_object(value);
                    expected_objects.push(value as usize);
                } else {
                    let value = match index {
                        1 => i64::MIN,
                        2 => i64::MAX,
                        66 => object as usize as i64,
                        _ => index as i64 - 80,
                    };
                    builder.push_i64(value);
                }
            }

            let capsule = builder.into_capsule();
            assert!(!capsule.is_null(), "packed preserved state should allocate");
            assert_eq!(
                ffi::PyObject_GC_IsTracked(capsule),
                1,
                "real preserved-state capsules must expose owned object and cell edges to cyclic GC"
            );
            assert_eq!(
                std::mem::size_of::<PreservedState>(),
                std::mem::size_of::<Vec<u64>>(),
                "payload words and arbitrary-size kind bits must share one compact allocation"
            );
            assert_eq!(ffi::Py_REFCNT(object), object_before_state + 5);
            assert_eq!(ffi::Py_REFCNT(cell), cell_before_state + 2);

            let values = preserved_values_ptr(capsule);
            assert_eq!(*values, object as usize as u64);
            assert_eq!(*values.add(1), i64::MIN as u64);
            assert_eq!(*values.add(2), i64::MAX as u64);
            assert_eq!(*values.add(63), object as usize as u64);
            assert_eq!(*values.add(64), object as usize as u64);
            assert_eq!(*values.add(65), cell as usize as u64);
            assert_eq!(*values.add(66), object as usize as u64);
            assert_eq!(*values.add(127), object as usize as u64);
            assert_eq!(*values.add(128), object as usize as u64);
            assert_eq!(*values.add(129), cell as usize as u64);

            let traverse = (*ffi::Py_TYPE(capsule))
                .tp_traverse
                .expect("capsule type should expose its actual GC visitor");
            let mut observed_objects = Vec::<usize>::new();
            assert_eq!(
                traverse(
                    capsule,
                    collect_preserved_object_visit,
                    ptr::from_mut(&mut observed_objects).cast(),
                ),
                0
            );
            assert_eq!(
                observed_objects, expected_objects,
                "GC must visit exact owned objects and cells without treating scalar bits as pointers"
            );
            assert_eq!(
                traverse(capsule, reject_preserved_object_visit, ptr::null_mut()),
                37,
                "capsule traversal must propagate the real visitor's early-stop result"
            );

            let clear = (*ffi::Py_TYPE(capsule))
                .tp_clear
                .expect("tracked capsule type should expose its actual GC clear callback");
            assert_eq!(clear(capsule), 0);
            assert_eq!(clear(capsule), 0, "GC clearing must be idempotent");
            assert_eq!(ffi::Py_REFCNT(object), object_before_state);
            assert_eq!(ffi::Py_REFCNT(cell), cell_before_state);
            for index in OBJECT_SLOTS {
                assert_eq!(*values.add(index), 0);
            }

            ffi::Py_DECREF(capsule);
            assert_eq!(ffi::Py_REFCNT(object), object_before_state);
            assert_eq!(ffi::Py_REFCNT(cell), cell_before_state);
            ffi::Py_DECREF(cell);
            ffi::Py_DECREF(object);
        });
    }

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

            PreservedState::clear(
                state_from_capsule(state).expect("preserved state should remain valid"),
            );
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
