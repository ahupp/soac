use pyo3::ffi;
use std::ffi::{c_int, c_void};
use std::ptr;

use soac_core::block_py::PreservedLocation;

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
    // Exact compiler operand roles in acquisition order. Suspended cleanup
    // releases these in reverse. Slot spelling never selects this role.
    operand_slots: Vec<usize>,
    // Fixed raw reference slots in the same GC owner as the execution payload.
    // There is no separately clearable Python snapshot shell and none of these
    // references is exposed as a public mutable preserved slot.
    strict_resume: Option<Box<crate::strict_function::StrictSuspendedFunctionSnapshot>>,
    strict_closed_slot: Option<usize>,
    // Inline in this boxed owner: stable while installed, taken before callbacks.
    managed: Option<crate::managed_generator::Binding>,
    handled: Option<Box<crate::handled_exception::OwnedHandledExceptionState>>,
    terminal: bool,
    clearing: bool,
}

pub(crate) struct PreservedStateBuilder {
    storage: Vec<u64>,
    operand_slots: Vec<usize>,
    slot_count: usize,
    initialized_slots: usize,
}

impl PreservedStateBuilder {
    pub(crate) fn with_capacity(slot_count: usize, operand_slots: &[usize]) -> Result<Self, ()> {
        let mut owned_operand_slots = Vec::new();
        if owned_operand_slots
            .try_reserve_exact(operand_slots.len())
            .is_err()
        {
            unsafe { ffi::PyErr_NoMemory() };
            return Err(());
        }
        for &slot in operand_slots {
            if slot >= slot_count || owned_operand_slots.contains(&slot) {
                unsafe {
                    ffi::PyErr_SetString(
                        ffi::PyExc_ValueError,
                        c"invalid or duplicate preserved operand slot".as_ptr(),
                    );
                }
                return Err(());
            }
            owned_operand_slots.push(slot);
        }
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
            operand_slots: owned_operand_slots,
            slot_count,
            initialized_slots: 0,
        })
    }

    pub(crate) unsafe fn push_owned_object(&mut self, value: *mut ffi::PyObject) {
        debug_assert!(!value.is_null());
        debug_assert!(!self.operand_slots.contains(&self.initialized_slots));
        debug_assert!(self.initialized_slots < self.slot_count);
        let slot = self.initialized_slots;
        self.storage[slot] = value as usize as u64;
        self.storage[self.slot_count + slot / PRESERVED_KIND_BITS] |=
            1_u64 << (slot % PRESERVED_KIND_BITS);
        self.initialized_slots += 1;
    }

    pub(crate) fn push_empty_operand(&mut self) {
        debug_assert!(self.operand_slots.contains(&self.initialized_slots));
        debug_assert!(self.initialized_slots < self.slot_count);
        let slot = self.initialized_slots;
        self.storage[self.slot_count + slot / PRESERVED_KIND_BITS] |=
            1_u64 << (slot % PRESERVED_KIND_BITS);
        self.initialized_slots += 1;
    }

    pub(crate) fn push_i64(&mut self, value: i64) {
        debug_assert!(self.initialized_slots < self.slot_count);
        debug_assert!(!self.operand_slots.contains(&self.initialized_slots));
        self.storage[self.initialized_slots] = value as u64;
        self.initialized_slots += 1;
    }

    pub(crate) unsafe fn into_capsule(mut self) -> *mut ffi::PyObject {
        debug_assert_eq!(self.initialized_slots, self.slot_count);
        if self.operand_slots.iter().any(|&slot| {
            self.storage[slot] != 0
                || self.storage[self.slot_count + slot / PRESERVED_KIND_BITS]
                    & (1_u64 << (slot % PRESERVED_KIND_BITS))
                    == 0
        }) {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_ValueError,
                    c"preserved operand slots must start as empty object owners".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        let state = Box::new(PreservedState {
            storage: std::mem::take(&mut self.storage),
            operand_slots: std::mem::take(&mut self.operand_slots),
            strict_resume: None,
            strict_closed_slot: None,
            managed: None,
            handled: None,
            terminal: false,
            clearing: false,
        });
        self.initialized_slots = 0;
        unsafe { capsule_from_preserved_state(state) }
    }
}

impl Drop for PreservedStateBuilder {
    fn drop(&mut self) {
        let error = unsafe { ffi::PyErr_GetRaisedException() };
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
        unsafe { ffi::PyErr_SetRaisedException(error) };
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
        // A finalizer can recursively invoke tp_clear. It must not skip the
        // remaining evaluation-stack operands and start releasing locals.
        if unsafe { (*state).clearing } {
            return;
        }
        unsafe { (*state).clearing = true };
        // A finalizer can reenter through a still-visible capsule. Reject
        // attachment/resume before releasing any source, cell, or value edge.
        unsafe { (*state).terminal = true };
        let operand_count = unsafe { (*state).operand_slots.len() };
        for index in (0..operand_count).rev() {
            let slot = unsafe { *(*state).operand_slots.as_ptr().add(index) };
            let address = unsafe { (*state).storage.as_mut_ptr().add(slot) };
            let value = unsafe { ptr::replace(address, 0) } as usize as *mut ffi::PyObject;
            unsafe { ffi::Py_XDECREF(value) };
        }
        // Unpublish every callback-visible association before releasing any
        // object. Reentry must not find a partially retired native binding or
        // reuse a strict snapshot while a payload finalizer is running.
        let managed = unsafe { (*state).managed.take() };
        let strict_resume = unsafe { (*state).strict_resume.take() };
        if let Some(handled) = unsafe { (*state).handled.as_deref_mut() } {
            unsafe { crate::handled_exception::OwnedHandledExceptionState::mark_terminal(handled) };
        }
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
        drop(strict_resume);
        drop(managed);
        if let Some(handled) = unsafe { (*state).handled.as_deref_mut() } {
            unsafe { crate::handled_exception::OwnedHandledExceptionState::clear(handled) };
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
    if let Some(strict_resume) = unsafe { (*state).strict_resume.as_deref().map(ptr::from_ref) } {
        let result = unsafe {
            crate::strict_function::StrictSuspendedFunctionSnapshot::traverse(
                strict_resume,
                visit,
                arg,
            )
        };
        if result != 0 {
            return result;
        }
    }
    if let Some(managed) = unsafe { (*state).managed.as_ref().map(ptr::from_ref) } {
        let result = unsafe { crate::managed_generator::Binding::traverse(managed, visit, arg) };
        if result != 0 {
            return result;
        }
    }
    if let Some(handled) = unsafe { (*state).handled.as_deref() } {
        return unsafe {
            crate::handled_exception::OwnedHandledExceptionState::traverse(handled, visit, arg)
        };
    }
    0
}

unsafe extern "C" fn preserved_state_capsule_clear(capsule: *mut ffi::PyObject) -> c_int {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        unsafe { ffi::PyErr_SetRaisedException(error) };
        return 0;
    };
    unsafe { PreservedState::clear(state) };
    unsafe { ffi::PyErr_SetRaisedException(error) };
    0
}

unsafe extern "C" fn preserved_state_capsule_destructor(capsule: *mut ffi::PyObject) {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    let state_ptr = unsafe {
        ffi::PyCapsule_GetPointer(capsule, PRESERVED_STATE_CAPSULE_NAME.as_ptr())
            as *mut PreservedState
    };
    if state_ptr.is_null() {
        unsafe {
            ffi::PyErr_SetRaisedException(error);
        }
        return;
    }
    unsafe { PreservedState::clear(state_ptr) };
    drop(unsafe { Box::from_raw(state_ptr) });
    unsafe { ffi::PyErr_SetRaisedException(error) };
}

unsafe fn strict_state_error(message: &'static str) {
    let py = unsafe { pyo3::Python::assume_attached() };
    crate::strict_runtime_unavailable(py, message).restore(py);
}

pub(crate) unsafe fn enter_handled_exception_state(
    capsule: *mut ffi::PyObject,
    plan: &crate::handled_exception::HandledExceptionPlan,
) -> Result<*mut crate::handled_exception::HandledExceptionState, ()> {
    let state = unsafe { state_from_capsule(capsule)? };
    if unsafe { (*state).terminal } {
        unsafe { strict_state_error("suspended frame state has been cleared") };
        return Err(());
    }
    if unsafe { (*state).handled.is_none() } {
        // Rust allocation cannot run Python or GC. Publish the owner before
        // linking its stable native exception item into the current thread.
        let handled = crate::handled_exception::OwnedHandledExceptionState::new(plan, true)?;
        unsafe { (*state).handled = Some(handled) };
    }
    let handled = unsafe {
        (*state)
            .handled
            .as_deref_mut()
            .expect("installed handled state")
    };
    handled.prepare_plan(plan)?;
    unsafe { crate::handled_exception::OwnedHandledExceptionState::enter(handled) }
}

pub(crate) unsafe fn attach_strict_resume_state(
    capsule: *mut ffi::PyObject,
    owner: Box<crate::strict_function::StrictSuspendedFunctionSnapshot>,
    closed_slot: usize,
) -> Result<(), ()> {
    let admission = (|| {
        let state = unsafe { state_from_capsule(capsule)? };
        if unsafe { (*state).terminal || (*state).strict_resume.is_some() }
            || closed_slot >= unsafe { (*state).slot_count() }
            || unsafe { (*state).slot_kind(closed_slot) } != PreservedSlotKind::I64
            || unsafe { *(*state).storage.as_ptr().add(closed_slot) } != 0
        {
            unsafe { strict_state_error("strict suspended state cannot be replaced or revived") };
            return Err(());
        }
        Ok(state)
    })();
    match admission {
        Ok(state) => {
            unsafe {
                (*state).strict_resume = Some(owner);
                (*state).strict_closed_slot = Some(closed_slot);
            }
            Ok(())
        }
        Err(()) => {
            let error = unsafe { ffi::PyErr_GetRaisedException() };
            drop(owner);
            unsafe { ffi::PyErr_SetRaisedException(error) };
            Err(())
        }
    }
}

/// Borrow only while the caller pins this exact capsule and has not started
/// terminal cleanup. Mutating source-function closure/code fields cannot change
/// these execution-owned references. No Python owner wrapper is manufactured.
pub(crate) unsafe fn strict_resume_snapshot(
    capsule: *mut ffi::PyObject,
) -> Result<*mut crate::strict_function::StrictSuspendedFunctionSnapshot, ()> {
    let state = unsafe { state_from_capsule(capsule)? };
    if unsafe { (*state).terminal || (*state).strict_resume.is_none() } {
        unsafe { strict_state_error("strict suspended state is absent or terminal") };
        return Err(());
    }
    Ok(unsafe {
        ptr::from_mut(
            (*state)
                .strict_resume
                .as_deref_mut()
                .expect("live snapshot"),
        )
    })
}

/// One source activation owner, reused across all native/deopt resumes. Only
/// compiler construction can install it after the exact strict snapshot is
/// attached and before any native generator callback observes this capsule.
/// Retire only secondary snapshot/protocol edges. Actual preserved source
/// locals must already have passed their ordinary cleanup; this operation must
/// precede the final suspended C-API handled-item release. The native Binding
/// itself stays installed so its step can validate delivery and consume any
/// terminal protocol error after the Rust body returns.
pub(crate) unsafe fn retire_terminal_protocol_roots(capsule: *mut ffi::PyObject) -> Result<(), ()> {
    let state = unsafe { state_from_capsule(capsule)? };
    // Mark/unpublish before the first decref.
    unsafe { (*state).terminal = true };
    let snapshot = unsafe { (*state).strict_resume.take() };
    let function = if let Some(binding) = unsafe { (*state).managed.as_mut().map(ptr::from_mut) } {
        unsafe {
            ptr::replace(
                crate::managed_generator::Binding::function_owner_slot(binding),
                ptr::null_mut(),
            )
        }
    } else {
        ptr::null_mut()
    };
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    // The taken Binding function ref pins the actual callable until all old
    // snapshot cells and duplicate source-owner references have been released.
    drop(snapshot);
    unsafe {
        ffi::Py_XDECREF(function);
        ffi::PyErr_SetRaisedException(error);
    }
    Ok(())
}

/// Raw access for the native step protocol, not permission to enter a body.
/// Completion can release strict_resume before the step inspects its consumed
/// exception and returns to native clear, so terminal bindings remain visible
/// here until their exact clear callback takes them.
pub(crate) unsafe fn managed_binding(
    capsule: *mut ffi::PyObject,
) -> Result<Option<*mut crate::managed_generator::Binding>, ()> {
    let state = unsafe { state_from_capsule(capsule)? };
    Ok(unsafe { (*state).managed.as_mut().map(ptr::from_mut) })
}

unsafe fn strict_closed_slot(state: *const PreservedState) -> Result<usize, ()> {
    let Some(slot) = (unsafe { (*state).strict_closed_slot }) else {
        unsafe { strict_state_error("preserved state has no strict suspended snapshot") };
        return Err(());
    };
    if unsafe { (*state).strict_resume.is_none() }
        || slot >= unsafe { (*state).slot_count() }
        || unsafe { (*state).slot_kind(slot) } != PreservedSlotKind::I64
    {
        unsafe { strict_state_error("strict suspended state lost its closed-slot layout") };
        return Err(());
    }
    Ok(slot)
}

pub(crate) unsafe fn strict_state_is_closed(capsule: *mut ffi::PyObject) -> Result<bool, ()> {
    let state = unsafe { state_from_capsule(capsule)? };
    if unsafe { (*state).terminal } {
        return Ok(true);
    }
    let slot = unsafe { strict_closed_slot(state)? };
    Ok(unsafe { *(*state).storage.as_ptr().add(slot) != 0 })
}

pub(crate) unsafe fn install_managed_binding(
    capsule: *mut ffi::PyObject,
    binding: crate::managed_generator::Binding,
) -> Result<(), ()> {
    let admission = (|| -> Result<*mut PreservedState, ()> {
        let state = unsafe { state_from_capsule(capsule)? };
        let slot = unsafe { strict_closed_slot(state)? };
        if unsafe {
            (*state).terminal
                || (*state).managed.is_some()
                || *(*state).storage.as_ptr().add(slot) != 0
                || !crate::managed_generator::Binding::is_prepared_unbound(&binding)
        } {
            unsafe {
                strict_state_error("managed suspended execution cannot be replaced or revived")
            };
            return Err(());
        }
        Ok(state)
    })();
    match admission {
        Ok(state) => {
            unsafe { (*state).managed = Some(binding) };
            Ok(())
        }
        Err(()) => {
            let error = unsafe { ffi::PyErr_GetRaisedException() };
            drop(binding);
            unsafe { ffi::PyErr_SetRaisedException(error) };
            Err(())
        }
    }
}

/// Failed construction must not retire a different, already-bound native
/// generator. Taking only the binding would reopen the ordinary-resume path;
/// an aborted preparation therefore retires its entire suspended execution.
pub(crate) unsafe fn abort_managed_preparation(capsule: *mut ffi::PyObject) {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    if let Ok(state) = unsafe { state_from_capsule(capsule) }
        && unsafe {
            (*state).managed.as_ref().is_some_and(|binding| {
                crate::managed_generator::Binding::is_prepared_unbound(binding)
            })
        }
    {
        unsafe { PreservedState::clear(state) };
    }
    unsafe { ffi::PyErr_SetRaisedException(error) };
}

pub(crate) unsafe fn clear_managed_binding(
    capsule: *mut ffi::PyObject,
    generator: *mut ffi::PyObject,
) {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    if let Ok(state) = unsafe { state_from_capsule(capsule) }
        && unsafe {
            (*state).managed.as_ref().is_some_and(|binding| {
                crate::managed_generator::Binding::matches_clear(binding, generator)
            })
        }
    {
        unsafe { PreservedState::clear(state) };
    }
    unsafe { ffi::PyErr_SetRaisedException(error) };
}

/// The native protocol has already taken this owned normalized exception out
/// of its GC-visible binding. Every path consumes it, including bad capsules
/// or missing activations, without letting a decref replace the diagnostic.
pub(crate) unsafe fn inject_managed_exception_owned(
    capsule: *mut ffi::PyObject,
    exception: *mut ffi::PyObject,
) -> c_int {
    let activation =
        (|| -> Result<*mut crate::handled_exception::OwnedHandledExceptionState, ()> {
            let state = unsafe { state_from_capsule(capsule)? };
            if unsafe {
                (*state).terminal || (*state).managed.is_none() || (*state).strict_resume.is_none()
            } {
                unsafe { strict_state_error("managed exception has no live suspended execution") };
                return Err(());
            }
            unsafe { (*state).handled.as_deref_mut().map(ptr::from_mut) }.ok_or_else(|| {
                unsafe { strict_state_error("managed exception has no active handled state") };
            })
        })();
    match activation {
        Ok(owner) => unsafe {
            crate::handled_exception::OwnedHandledExceptionState::inject_managed_exception_owned(
                owner, exception,
            )
        },
        Err(()) => {
            let error = unsafe { ffi::PyErr_GetRaisedException() };
            unsafe {
                ffi::Py_XDECREF(exception);
                ffi::PyErr_SetRaisedException(error);
            }
            -1
        }
    }
}

/// Drop the frame's captured cells at completion, not at eventual generator
/// collection. The active resume still pins its cells until its body unwinds.
pub(crate) unsafe fn finish_strict_resume(capsule: *mut ffi::PyObject) {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    if let Ok(state) = unsafe { state_from_capsule(capsule) }
        && let Some(closed_slot) = unsafe { (*state).strict_closed_slot }
        && unsafe { *(*state).storage.as_ptr().add(closed_slot) } != 0
    {
        unsafe { (*state).terminal = true };
        let owner = unsafe { (*state).strict_resume.take() };
        drop(owner);
    }
    unsafe { ffi::PyErr_SetRaisedException(error) };
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
    operand_slots: &[usize],
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

    let Ok(mut state) = PreservedStateBuilder::with_capacity(slot_count as usize, operand_slots)
    else {
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
        if operand_slots.contains(&(index as usize)) {
            if kind != PreservedSlotKind::PyObjectOrNull || value_obj != unsafe { ffi::Py_None() } {
                unsafe {
                    ffi::PyErr_SetString(
                        ffi::PyExc_ValueError,
                        c"preserved operand initialization must be None with object storage"
                            .as_ptr(),
                    );
                }
                return ptr::null_mut();
            }
            state.push_empty_operand();
            continue;
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

/// Return a new reference to the activation's current handled exception, or
/// None for an unstarted, empty, or closed activation. This read-only view
/// owns no duplicate snapshot and never inherits the caller's handled item.
pub unsafe fn suspended_handled_exception_owned(capsule: *mut ffi::PyObject) -> *mut ffi::PyObject {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        return ptr::null_mut();
    };
    if !unsafe { (*state).terminal }
        && let Some(handled) = unsafe { (*state).handled.as_deref() }
    {
        return unsafe { handled.suspended_exception_owned() };
    }
    let none = unsafe { ffi::Py_None() };
    unsafe { ffi::Py_INCREF(none) };
    none
}

pub unsafe fn preserved_values_ptr(capsule: *mut ffi::PyObject) -> *mut u64 {
    let Ok(state) = (unsafe { state_from_capsule(capsule) }) else {
        return ptr::null_mut();
    };
    unsafe { (*state).storage.as_mut_ptr() }
}

/// Borrow the actual nullable owning slot for a previously validated compiler
/// operand. The caller pins the capsule, publishes replacements before any
/// release, and never holds a Rust reference across Python callbacks.
pub(crate) unsafe fn operand_owner_slot(
    capsule: *mut ffi::PyObject,
    location: PreservedLocation,
) -> Result<*mut *mut ffi::PyObject, ()> {
    let state = unsafe { state_from_capsule(capsule)? };
    let slot = location.slot() as usize;
    if unsafe { (*state).terminal }
        || slot >= unsafe { (*state).slot_count() }
        || unsafe { (*state).slot_kind(slot) } != PreservedSlotKind::PyObjectOrNull
        || !unsafe { (*state).operand_slots.contains(&slot) }
    {
        unsafe { strict_state_error("suspended expression operand lost its physical owner role") };
        return Err(());
    }
    Ok(unsafe { (*state).storage.as_mut_ptr().add(slot).cast() })
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
    use pyo3::{Bound, Python};

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
    fn suspended_exception_projection_handles_unstarted_closed_and_invalid_capsules() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| unsafe {
            let capsule = PreservedStateBuilder::with_capacity(0, &[])
                .unwrap()
                .into_capsule();
            assert!(!capsule.is_null());
            let unstarted = suspended_handled_exception_owned(capsule);
            assert_eq!(unstarted, ffi::Py_None());
            ffi::Py_DECREF(unstarted);
            assert_eq!(preserved_state_capsule_clear(capsule), 0);
            let closed = suspended_handled_exception_owned(capsule);
            assert_eq!(closed, ffi::Py_None());
            ffi::Py_DECREF(closed);
            ffi::Py_DECREF(capsule);
            assert!(suspended_handled_exception_owned(ffi::Py_None()).is_null());
            assert!(!ffi::PyErr_Occurred().is_null());
            ffi::PyErr_Clear();
        });
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
            let mut builder = PreservedStateBuilder::with_capacity(SLOT_COUNT, &[])
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

    struct TerminalObservation {
        state: *mut ffi::PyObject,
        saw_terminal: bool,
        could_not_replace: bool,
    }

    unsafe extern "C" fn observe_terminal_resume_owner(owner: *mut ffi::PyObject) {
        let observation = unsafe { ffi::PyCapsule_GetContext(owner) }.cast::<TerminalObservation>();
        if observation.is_null() {
            unsafe { ffi::PyErr_Clear() };
            return;
        }
        let state = unsafe { (*observation).state };
        let observed = unsafe { strict_resume_snapshot(state) };
        unsafe {
            (*observation).saw_terminal = observed.is_err() && !ffi::PyErr_Occurred().is_null();
            ffi::PyErr_Clear();
            let py = Python::assume_attached();
            let replacement =
                crate::strict_function::StrictSuspendedFunctionSnapshot::snapshot_with_references(
                    py,
                    Vec::new(),
                );
            (*observation).could_not_replace =
                attach_strict_resume_state(state, replacement, 0).is_err();
            ffi::PyErr_Clear();
        }
    }

    #[test]
    fn strict_suspended_state_is_traversed_and_terminal_before_reentrant_release() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        soac_cpython::initialize_test_python("soac_jit-strict-preserved-state-gc-test")
            .expect("test Python should initialize");
        Python::attach(|py| unsafe {
            let mut builder = PreservedStateBuilder::with_capacity(1, &[]).unwrap();
            builder.push_i64(0);
            let state = builder.into_capsule();
            assert!(!state.is_null());
            let mut observation = TerminalObservation {
                state,
                saw_terminal: false,
                could_not_replace: false,
            };
            let owner = ffi::PyCapsule_New(
                ptr::from_mut(&mut observation).cast(),
                c"soac.test.StrictSuspendedOwner".as_ptr(),
                Some(observe_terminal_resume_owner),
            );
            assert!(!owner.is_null());
            assert_eq!(
                ffi::PyCapsule_SetContext(owner, ptr::from_mut(&mut observation).cast()),
                0
            );
            let snapshot =
                crate::strict_function::StrictSuspendedFunctionSnapshot::snapshot_with_references(
                    py,
                    vec![Bound::from_borrowed_ptr(py, owner).unbind()],
                );
            attach_strict_resume_state(state, snapshot, 0).unwrap();
            let mut observed = Vec::<usize>::new();
            let traverse = (*ffi::Py_TYPE(state)).tp_traverse.unwrap();
            assert_eq!(
                traverse(
                    state,
                    collect_preserved_object_visit,
                    ptr::from_mut(&mut observed).cast()
                ),
                0
            );
            assert_eq!(observed, [owner as usize]);
            ffi::Py_DECREF(owner);

            ffi::PyErr_SetString(ffi::PyExc_ValueError, c"existing body exception".as_ptr());
            let clear = (*ffi::Py_TYPE(state)).tp_clear.unwrap();
            assert_eq!(clear(state), 0);
            assert!(observation.saw_terminal);
            assert!(observation.could_not_replace);
            assert_ne!(ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError), 0);
            ffi::PyErr_Clear();
            assert_eq!(clear(state), 0);
            ffi::Py_DECREF(state);
        });
    }

    #[test]
    fn strict_suspended_cells_release_at_completion_not_generator_collection() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        soac_cpython::initialize_test_python("soac_jit-strict-preserved-state-completion-test")
            .expect("test Python should initialize");
        Python::attach(|py| unsafe {
            let mut builder = PreservedStateBuilder::with_capacity(1, &[]).unwrap();
            builder.push_i64(0);
            let state = builder.into_capsule();
            let owner = ffi::PyList_New(0);
            assert!(!state.is_null() && !owner.is_null());
            let original = ffi::Py_REFCNT(owner);
            let snapshot =
                crate::strict_function::StrictSuspendedFunctionSnapshot::snapshot_with_references(
                    py,
                    vec![Bound::from_borrowed_ptr(py, owner).unbind()],
                );
            attach_strict_resume_state(state, snapshot, 0).unwrap();
            finish_strict_resume(state);
            assert_eq!(ffi::Py_REFCNT(owner), original + 1);
            *preserved_values_ptr(state) = 1;
            ffi::PyErr_SetString(ffi::PyExc_StopIteration, c"completed".as_ptr());
            finish_strict_resume(state);
            assert_ne!(ffi::PyErr_ExceptionMatches(ffi::PyExc_StopIteration), 0);
            ffi::PyErr_Clear();
            assert_eq!(ffi::Py_REFCNT(owner), original);
            assert!(strict_resume_snapshot(state).is_err());
            assert!(!ffi::PyErr_Occurred().is_null());
            ffi::PyErr_Clear();
            let replacement =
                crate::strict_function::StrictSuspendedFunctionSnapshot::snapshot_with_references(
                    py,
                    Vec::new(),
                );
            assert!(attach_strict_resume_state(state, replacement, 0).is_err());
            ffi::PyErr_Clear();
            ffi::Py_DECREF(state);
            ffi::Py_DECREF(owner);
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

            let mut builder = PreservedStateBuilder::with_capacity(3, &[])
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
            let mut builder = PreservedStateBuilder::with_capacity(1, &[])
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
            let state = new_preserved_state(initial_values, kind_values, &[]);
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
            let state = new_preserved_state(initial_values, kinds, &[]);

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

    #[test]
    fn preserved_operand_initialization_and_physical_role_are_checked() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        soac_cpython::initialize_test_python("soac_jit-preserved-operand-role-test")
            .expect("test Python should initialize");
        Python::attach(|_| unsafe {
            let zero = ffi::PyLong_FromLong(0);
            let none = ffi::Py_None();
            let initial = ffi::PyTuple_Pack(2, none, none);
            let kinds = ffi::PyTuple_Pack(2, zero, zero);
            let state = new_preserved_state(initial, kinds, &[1]);
            assert!(!state.is_null());
            assert_eq!(*preserved_values_ptr(state), none as usize as u64);
            let operand = operand_owner_slot(state, PreservedLocation(1)).unwrap();
            assert!((*operand).is_null(), "unacquired Operand is NULL, not None");
            assert!(operand_owner_slot(state, PreservedLocation(0)).is_err());
            ffi::PyErr_Clear();

            let value = ffi::PyList_New(0);
            let before = ffi::Py_REFCNT(value);
            ffi::Py_INCREF(value);
            *operand = value;
            let taken = ptr::replace(operand, ptr::null_mut());
            assert_eq!(taken, value);
            assert_eq!(ffi::Py_REFCNT(value), before + 1);
            ffi::Py_DECREF(taken);
            assert_eq!(ffi::Py_REFCNT(value), before);
            assert!((*operand).is_null());

            for slots in [&[2][..], &[1, 1][..]] {
                assert!(new_preserved_state(initial, kinds, slots).is_null());
                assert_ne!(ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError), 0);
                ffi::PyErr_Clear();
            }
            let bad_initial = ffi::PyTuple_Pack(2, none, value);
            assert!(new_preserved_state(bad_initial, kinds, &[1]).is_null());
            assert_ne!(ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError), 0);
            ffi::PyErr_Clear();
            ffi::Py_DECREF(bad_initial);
            let one = ffi::PyLong_FromLong(1);
            let scalar_kinds = ffi::PyTuple_Pack(2, zero, one);
            assert!(new_preserved_state(initial, scalar_kinds, &[1]).is_null());
            assert_ne!(ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError), 0);
            ffi::PyErr_Clear();
            ffi::Py_DECREF(scalar_kinds);
            ffi::Py_DECREF(one);

            let clear = (*ffi::Py_TYPE(state)).tp_clear.unwrap();
            assert_eq!(clear(state), 0);
            assert!(operand_owner_slot(state, PreservedLocation(1)).is_err());
            ffi::PyErr_Clear();
            ffi::Py_DECREF(state);
            ffi::Py_DECREF(initial);
            ffi::Py_DECREF(kinds);
            ffi::Py_DECREF(zero);
            ffi::Py_DECREF(value);
        });
    }

    struct OperandReleaseProbe {
        state: *mut ffi::PyObject,
        slot: usize,
        reenter: bool,
        events: *mut Vec<(usize, bool, bool)>,
    }

    unsafe extern "C" fn observe_operand_release(owner: *mut ffi::PyObject) {
        let probe = unsafe {
            ffi::PyCapsule_GetPointer(owner, c"soac.test.OperandRelease".as_ptr())
                .cast::<OperandReleaseProbe>()
        };
        assert!(!probe.is_null());
        let state = unsafe { (*probe).state };
        let values = unsafe { preserved_values_ptr(state) };
        unsafe {
            (*(*probe).events).push(((*probe).slot, *values.add((*probe).slot) == 0, *values != 0));
        }
        if unsafe { (*probe).reenter } {
            let clear = unsafe { (*ffi::Py_TYPE(state)).tp_clear.unwrap() };
            assert_eq!(unsafe { clear(state) }, 0);
        }
    }

    #[test]
    fn suspended_cleanup_releases_operand_stack_before_locals_under_reentry() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        soac_cpython::initialize_test_python("soac_jit-preserved-operand-unwind-test")
            .expect("test Python should initialize");
        Python::attach(|_| unsafe {
            let mut events = Vec::new();
            let mut probes = [0, 1, 3].map(|slot| OperandReleaseProbe {
                state: ptr::null_mut(),
                slot,
                reenter: slot == 1,
                events: ptr::from_mut(&mut events),
            });
            // Acquisition order deliberately differs from physical slot order.
            let mut builder = PreservedStateBuilder::with_capacity(4, &[3, 1]).unwrap();
            let local = ffi::PyCapsule_New(
                ptr::from_mut(&mut probes[0]).cast(),
                c"soac.test.OperandRelease".as_ptr(),
                Some(observe_operand_release),
            );
            assert!(!local.is_null());
            builder.push_owned_object(local);
            builder.push_empty_operand();
            builder.push_i64(0);
            builder.push_empty_operand();
            let state = builder.into_capsule();
            assert!(!state.is_null());
            for probe in &mut probes {
                probe.state = state;
            }
            for probe in &mut probes[1..] {
                let value = ffi::PyCapsule_New(
                    ptr::from_mut(probe).cast(),
                    c"soac.test.OperandRelease".as_ptr(),
                    Some(observe_operand_release),
                );
                assert!(!value.is_null());
                *operand_owner_slot(state, PreservedLocation(probe.slot as u32)).unwrap() = value;
            }
            ffi::PyErr_SetString(ffi::PyExc_ValueError, c"original pending failure".as_ptr());
            let original = ffi::PyErr_GetRaisedException();
            ffi::Py_INCREF(original);
            ffi::PyErr_SetRaisedException(original);
            let clear = (*ffi::Py_TYPE(state)).tp_clear.unwrap();
            assert_eq!(clear(state), 0);
            assert_eq!(events, [(1, true, true), (3, true, true), (0, true, false)]);
            let after = ffi::PyErr_GetRaisedException();
            assert_eq!(after, original);
            ffi::Py_DECREF(after);
            ffi::Py_DECREF(original);
            assert_eq!(clear(state), 0);
            assert_eq!(events.len(), 3);
            ffi::Py_DECREF(state);
        });
    }
}
