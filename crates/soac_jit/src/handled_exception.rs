//! One explicit handled-exception stack per executing/suspended activation.
//!
//! Normal Python functions share the caller's native exception item. Only a
//! suspended body owns a separate item, linked for the duration of a resume.
//! In particular, saving the *resolved* PyErr_GetHandledException value loses
//! the distinction between an empty current item and its inherited caller.

use pyo3::ffi;
use soac_core::block_py::{BlockPyFunction, ModuleShape};
use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::Arc;

unsafe extern "C" {
    fn PyErr_GetHandledException() -> *mut ffi::PyObject;
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct RawPyErrStackItem {
    pub(crate) exc_value: *mut ffi::PyObject,
    pub(crate) previous_item: *mut RawPyErrStackItem,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct HandledExceptionRecord {
    scope: usize,
    previous: *mut ffi::PyObject,
}

/// Borrowed block-entry operands; the current locals pin every value until
/// selection returns. A continuing scope needs only its semantic identity:
/// its original caught-object operand may already have been reset to None.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct HandledExceptionRegion {
    pub(crate) scope: usize,
    pub(crate) exception: *mut ffi::PyObject,
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandledExceptionTransition {
    Leave = 0,
    Enter = 1,
    Unwind = 2,
}

impl HandledExceptionTransition {
    pub(crate) fn from_abi(value: i64) -> Option<Self> {
        [Self::Leave, Self::Enter, Self::Unwind]
            .into_iter()
            .find(|phase| *phase as i64 == value)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HandledExceptionPlan {
    scopes: HashMap<String, usize>,
    source_layout: Arc<[String]>,
}

impl HandledExceptionPlan {
    pub(crate) fn for_function(function: &BlockPyFunction<impl ModuleShape>) -> Self {
        let mut scopes = HashMap::new();
        let mut source_layout = Vec::new();
        for block in &function.blocks {
            for param in block.handled_exception_params() {
                if !scopes.contains_key(&param.name) {
                    source_layout.push(param.name.clone());
                    scopes.insert(param.name.clone(), source_layout.len());
                }
            }
        }
        Self {
            scopes,
            source_layout: source_layout.into(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.scopes.len()
    }

    pub(crate) fn scope(&self, name: &str) -> usize {
        self.scopes[name]
    }

    pub(crate) fn source_scopes(&self) -> usize {
        self.source_layout.len()
    }

    pub(crate) fn include_native_regions(&mut self, function: &BlockPyFunction<impl ModuleShape>) {
        for block in &function.blocks {
            for param in block.handled_exception_params() {
                let next = self.scopes.len() + 1;
                self.scopes.entry(param.name.clone()).or_insert(next);
            }
        }
    }
}

/// Raw ABI between generated code and the cold/deopt executor. Normal frames
/// allocate this header and its records on their native stack; suspended
/// frames put the same data in their GC-traversed preserved-state owner.
#[repr(C)]
pub(crate) struct HandledExceptionState {
    item: *mut RawPyErrStackItem,
    records: *mut HandledExceptionRecord,
    depth: usize,
    capacity: usize,
    pending_scope: usize,
    suspended: bool,
    active: bool,
    terminal: bool,
}

impl HandledExceptionState {
    unsafe fn current_item_slot() -> *mut *mut RawPyErrStackItem {
        unsafe {
            ffi::PyThreadState_Get()
                .cast::<u8>()
                .add(crate::jit::PY_THREAD_STATE_EXC_INFO_OFFSET as usize)
                .cast()
        }
    }

    pub(crate) unsafe fn initialize_normal(
        state: *mut Self,
        records: *mut HandledExceptionRecord,
        capacity: usize,
    ) -> *mut Self {
        unsafe {
            state.write(Self {
                item: *Self::current_item_slot(),
                records,
                depth: 0,
                capacity,
                pending_scope: 0,
                suspended: false,
                active: true,
                terminal: false,
            });
        }
        state
    }

    pub(crate) unsafe fn mark_raised(state: *mut Self, scope: usize) {
        if !state.is_null() {
            unsafe { (*state).pending_scope = scope };
        }
    }

    unsafe fn pop_to(state: *mut Self, depth: usize) {
        while unsafe { (*state).depth > depth } {
            unsafe {
                (*state).depth -= 1;
                let record = (*state).records.add((*state).depth);
                let previous = ptr::replace(&mut (*record).previous, ptr::null_mut());
                (*record).scope = 0;
                let old = ptr::replace(&mut (*(*state).item).exc_value, previous);
                // Publish the restored state before an exception's finalizer.
                ffi::Py_XDECREF(old);
            }
        }
    }

    pub(crate) unsafe fn select(
        state: *mut Self,
        regions: &[HandledExceptionRegion],
        transition: HandledExceptionTransition,
    ) -> c_int {
        if state.is_null() {
            debug_assert!(regions.is_empty());
            return 0;
        }
        if unsafe { !(*state).active || (*state).terminal } {
            return unsafe { state_error(c"handled-exception activation is no longer live") };
        }
        // POP_EXCEPT may execute finalizers. Keep the independent raised-error
        // indicator outside their callbacks while changing the handled item.
        let saved_error = unsafe { ffi::PyErr_GetRaisedException() };
        let has_value = |region: &HandledExceptionRegion| {
            !region.exception.is_null() && region.exception != unsafe { ffi::Py_None() }
        };
        let mut common = 0;
        let mut fresh_start = regions.len();
        for (index, region) in regions.iter().enumerate() {
            if common < unsafe { (*state).depth }
                && (transition == HandledExceptionTransition::Unwind
                    || region.scope != unsafe { (*state).pending_scope })
                && region.scope == unsafe { (*(*state).records.add(common)).scope }
            {
                // The native item owns the *current* value, including a C-API
                // replacement. Requiring the old operand to stay non-None
                // would turn every caught object into a frame-lifetime root.
                common += 1;
                continue;
            }
            if transition == HandledExceptionTransition::Unwind {
                // An inactive finally region can carry an incoming raised
                // object in the same slot. Unwind uses only active identities,
                // never that new value, and leaves its pending marker intact.
                continue;
            }
            if has_value(region) || region.scope == unsafe { (*state).pending_scope } {
                fresh_start = index;
                break;
            }
        }
        // A fresh None operand still denotes an unentered region (for example
        // a finally path without an exception). Only already-active identities
        // can continue without their original object value.
        let fresh = || {
            regions[fresh_start..]
                .iter()
                .filter(|region| has_value(region))
        };
        // POP_EXCEPT can release an exception and execute a finalizer. Pin the
        // already-evaluated target operands across that transition.
        for region in fresh() {
            unsafe { ffi::Py_INCREF(region.exception) };
        }
        unsafe { Self::pop_to(state, common) };
        let mut status = 0;
        if transition == HandledExceptionTransition::Enter {
            for region in fresh() {
                if unsafe { (*state).terminal || (*state).depth == (*state).capacity } {
                    status = unsafe { state_error(c"invalid handled-exception region transition") };
                    break;
                }
                unsafe {
                    let record = (*state).records.add((*state).depth);
                    ffi::Py_INCREF(region.exception);
                    let previous = ptr::replace(&mut (*(*state).item).exc_value, region.exception);
                    record.write(HandledExceptionRecord {
                        scope: region.scope,
                        previous,
                    });
                    (*state).depth += 1;
                }
            }
            unsafe { (*state).pending_scope = 0 };
        }
        let error = unsafe { ffi::PyErr_GetRaisedException() };
        for region in fresh() {
            unsafe { ffi::Py_DECREF(region.exception) };
        }
        if status < 0 {
            unsafe {
                ffi::Py_XDECREF(saved_error);
                ffi::PyErr_SetRaisedException(error);
            }
        } else {
            unsafe {
                ffi::Py_XDECREF(error);
                ffi::PyErr_SetRaisedException(saved_error);
            }
        }
        status
    }

    /// Retire language-level handled scopes before invocation ownership is
    /// released. POP_EXCEPT finalizers still run with the restored own item
    /// linked; only then detach a suspended activation from its caller. Its
    /// residual C-API-selected item survives until `release_residual`, after
    /// ordinary local cleanup. A yield only detaches and preserves scopes.
    pub(crate) unsafe fn retire_scopes_and_detach(state: *mut Self, yielded: bool) {
        if state.is_null() || !unsafe { (*state).active } {
            return;
        }
        let error = unsafe { ffi::PyErr_GetRaisedException() };
        if unsafe { (*state).suspended } {
            if !yielded || unsafe { (*state).terminal } {
                unsafe {
                    (*state).terminal = true;
                    Self::pop_to(state, 0);
                    (*state).pending_scope = 0;
                }
            }
            unsafe {
                let item = (*state).item;
                debug_assert_eq!(*Self::current_item_slot(), item);
                *Self::current_item_slot() = (*item).previous_item;
                (*item).previous_item = ptr::null_mut();
                (*state).active = false;
            }
        } else {
            unsafe {
                Self::pop_to(state, 0);
                (*state).pending_scope = 0;
                (*state).active = false;
                (*state).item = ptr::null_mut();
            }
        }
        unsafe { ffi::PyErr_SetRaisedException(error) };
    }

    /// The terminal suspended activation's private exception item is a
    /// residual activation owner, not a source POP_EXCEPT operation. Release it
    /// after ordinary invocation/local cleanup. Normal
    /// calls share their caller's item and must never clear it here.
    pub(crate) unsafe fn release_residual(state: *mut Self) {
        if state.is_null()
            || unsafe { !(*state).suspended || (*state).active || !(*state).terminal }
        {
            return;
        }
        let error = unsafe { ffi::PyErr_GetRaisedException() };
        // An abandoned suspended frame may never execute semantic unwinding.
        // Its saved prior values are frame-stack owners, released before the
        // remaining current item. Do not replay POP_EXCEPT while unlinked.
        while unsafe { (*state).depth > 0 } {
            unsafe {
                (*state).depth -= 1;
                let record = (*state).records.add((*state).depth);
                let previous = ptr::replace(&mut (*record).previous, ptr::null_mut());
                (*record).scope = 0;
                ffi::Py_XDECREF(previous);
            }
        }
        if unsafe { !(*state).item.is_null() } {
            let value = unsafe { ptr::replace(&mut (*(*state).item).exc_value, ptr::null_mut()) };
            unsafe { ffi::Py_XDECREF(value) };
        }
        unsafe {
            (*state).pending_scope = 0;
            ffi::PyErr_SetRaisedException(error);
        }
    }
}

pub(crate) struct OwnedHandledExceptionState {
    state: HandledExceptionState,
    item: RawPyErrStackItem,
    records: Vec<HandledExceptionRecord>,
    source_layout: Arc<[String]>,
}

impl OwnedHandledExceptionState {
    pub(crate) fn new(plan: &HandledExceptionPlan, suspended: bool) -> Result<Box<Self>, ()> {
        let capacity = plan.len();
        let mut records = Vec::new();
        if records.try_reserve_exact(capacity).is_err() {
            unsafe { ffi::PyErr_NoMemory() };
            return Err(());
        }
        records.resize(capacity, HandledExceptionRecord::default());
        Ok(Box::new(Self {
            state: HandledExceptionState {
                item: ptr::null_mut(),
                records: records.as_mut_ptr(),
                depth: 0,
                capacity,
                pending_scope: 0,
                suspended,
                active: false,
                terminal: false,
            },
            item: RawPyErrStackItem::default(),
            records,
            source_layout: Arc::clone(&plan.source_layout),
        }))
    }

    pub(crate) fn prepare_plan(&mut self, plan: &HandledExceptionPlan) -> Result<(), ()> {
        let capacity = plan.len();
        if self.state.active
            || self.source_layout != plan.source_layout
            || self.records[..self.state.depth]
                .iter()
                .any(|record| record.scope > plan.source_scopes())
        {
            unsafe { state_error(c"suspended frame changed its source handled-region plan") };
            return Err(());
        }
        if capacity > self.state.capacity {
            if self
                .records
                .try_reserve_exact(capacity - self.records.len())
                .is_err()
            {
                unsafe { ffi::PyErr_NoMemory() };
                return Err(());
            }
            self.records
                .resize(capacity, HandledExceptionRecord::default());
            self.state.records = self.records.as_mut_ptr();
            self.state.capacity = capacity;
        }
        Ok(())
    }

    pub(crate) unsafe fn enter(owner: *mut Self) -> Result<*mut HandledExceptionState, ()> {
        let state = unsafe { ptr::addr_of_mut!((*owner).state) };
        if unsafe { (*state).active || (*state).terminal } {
            unsafe { state_error(c"suspended handled-exception state is running or closed") };
            return Err(());
        }
        if unsafe { (*state).suspended } {
            unsafe {
                let item = ptr::addr_of_mut!((*owner).item);
                (*state).item = item;
                (*item).previous_item = *HandledExceptionState::current_item_slot();
                *HandledExceptionState::current_item_slot() = item;
                (*state).active = true;
            }
        } else {
            unsafe {
                HandledExceptionState::initialize_normal(
                    state,
                    (*owner).records.as_mut_ptr(),
                    (*state).capacity,
                );
            }
        }
        Ok(state)
    }

    /// Consume a native-protocol exception in this suspended activation. The
    /// topmost resolved handled value is not enough: an empty private item
    /// must not inherit its caller's error as a new implicit context.
    pub(crate) unsafe fn inject_managed_exception_owned(
        owner: *mut Self,
        exception: *mut ffi::PyObject,
    ) -> c_int {
        let valid_owner = !owner.is_null()
            && unsafe {
                (*owner).state.suspended
                    && (*owner).state.active
                    && !(*owner).state.terminal
                    && (*owner).state.item == ptr::addr_of_mut!((*owner).item)
                    && *HandledExceptionState::current_item_slot() == (*owner).state.item
            };
        let invalid = if !valid_owner {
            unsafe { state_error(c"managed exception injection requires its active native item") };
            true
        } else if exception.is_null() || unsafe { ffi::PyExceptionInstance_Check(exception) } == 0 {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_TypeError,
                    c"managed exception injection requires a normalized exception".as_ptr(),
                );
            }
            true
        } else {
            false
        };
        if invalid {
            let error = unsafe { ffi::PyErr_GetRaisedException() };
            unsafe {
                ffi::Py_XDECREF(exception);
                ffi::PyErr_SetRaisedException(error);
            }
            return -1;
        }
        let own_exception = unsafe { (*owner).item.exc_value };
        if own_exception.is_null() || own_exception == unsafe { ffi::Py_None() } {
            unsafe { ffi::PyErr_SetRaisedException(exception) };
        } else {
            // This exact item is the current native top and is nonempty, so
            // SetObject can perform CPython's cycle-safe chaining without
            // walking to an inherited caller. It retains rather than steals.
            unsafe {
                ffi::PyErr_SetObject(ffi::Py_TYPE(exception).cast(), exception);
                let raised = ffi::PyErr_GetRaisedException();
                ffi::Py_DECREF(exception);
                ffi::PyErr_SetRaisedException(raised);
            }
        }
        // No owner pointer is read after a C API that can release a context
        // and run Python finalizers. The supplied owned reference is gone.
        0
    }

    pub(crate) unsafe fn mark_terminal(owner: *mut Self) {
        unsafe { (*owner).state.terminal = true };
    }

    /// Post-frame cleanup. The capsule marks terminal before clearing other
    /// owners, but residual exception decrefs happen only after those owners.
    pub(crate) unsafe fn clear(owner: *mut Self) {
        unsafe { Self::mark_terminal(owner) };
        // An explicit clear during a callback must not detach the executing
        // frame's native item. Its pinned activation finishes the release.
        if unsafe { (*owner).state.active } {
            return;
        }
        unsafe {
            let state = ptr::addr_of_mut!((*owner).state);
            HandledExceptionState::release_residual(state);
        }
    }

    /// Project this suspended activation's own item, never the caller value
    /// inherited by CPython's topmost-exception lookup. No snapshot is kept:
    /// PyErr_SetHandledException changes the value returned by the next read.
    pub(crate) unsafe fn suspended_exception_owned(&self) -> *mut ffi::PyObject {
        debug_assert!(self.state.suspended);
        let value = if self.state.terminal || self.item.exc_value.is_null() {
            unsafe { ffi::Py_None() }
        } else {
            self.item.exc_value
        };
        unsafe { ffi::Py_INCREF(value) };
        value
    }

    pub(crate) unsafe fn traverse(
        owner: *const Self,
        visit: ffi::visitproc,
        arg: *mut c_void,
    ) -> c_int {
        let value = unsafe { (*owner).item.exc_value };
        if !value.is_null() {
            let status = unsafe { visit(value, arg) };
            if status != 0 {
                return status;
            }
        }
        for index in 0..unsafe { (*owner).state.depth } {
            let previous = unsafe { (*(*owner).state.records.add(index)).previous };
            if !previous.is_null() {
                let status = unsafe { visit(previous, arg) };
                if status != 0 {
                    return status;
                }
            }
        }
        0
    }
}

impl Drop for OwnedHandledExceptionState {
    fn drop(&mut self) {
        debug_assert!(!self.state.active);
        unsafe { Self::clear(self) };
    }
}

unsafe fn state_error(message: &std::ffi::CStr) -> c_int {
    unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, message.as_ptr()) };
    -1
}

pub(crate) unsafe fn reraise_current() {
    let exception = unsafe { PyErr_GetHandledException() };
    if exception.is_null() {
        unsafe { state_error(c"No active exception to reraise") };
    } else {
        unsafe { ffi::PyErr_SetRaisedException(exception) };
    }
}

/// Forward a normalized escaping error or compiler completion, not a new
/// source raise. The caller's handled exception must not become its context.
pub(crate) unsafe fn restore_raised_exception(exception: *mut ffi::PyObject) -> c_int {
    if exception.is_null() || unsafe { ffi::PyExceptionInstance_Check(exception) } == 0 {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_TypeError,
                c"exceptions must derive from BaseException".as_ptr(),
            )
        };
        return -1;
    }
    unsafe {
        ffi::Py_INCREF(exception);
        ffi::PyErr_SetRaisedException(exception);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::prelude::*;

    unsafe extern "C" {
        fn PyErr_SetHandledException(exception: *mut ffi::PyObject);
    }

    fn plan(names: &[&str]) -> HandledExceptionPlan {
        HandledExceptionPlan {
            scopes: names
                .iter()
                .enumerate()
                .map(|(index, name)| (name.to_string(), index + 1))
                .collect(),
            source_layout: names
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
                .into(),
        }
    }

    struct ActiveTestState(Box<OwnedHandledExceptionState>);

    impl ActiveTestState {
        fn state(&mut self) -> *mut HandledExceptionState {
            &mut self.0.state
        }
        unsafe fn enter(&mut self) {
            unsafe {
                OwnedHandledExceptionState::enter(self.0.as_mut()).unwrap();
            }
        }
        unsafe fn finish(&mut self, yielded: bool) {
            unsafe {
                HandledExceptionState::retire_scopes_and_detach(self.state(), yielded);
                HandledExceptionState::release_residual(self.state());
            }
        }
    }

    impl Drop for ActiveTestState {
        fn drop(&mut self) {
            unsafe { self.finish(false) };
        }
    }

    struct CallerItemGuard {
        item: *mut RawPyErrStackItem,
        previous: *mut ffi::PyObject,
    }

    impl CallerItemGuard {
        unsafe fn new() -> Self {
            let item = unsafe { *HandledExceptionState::current_item_slot() };
            Self {
                item,
                previous: unsafe { ptr::replace(&mut (*item).exc_value, ptr::null_mut()) },
            }
        }
    }

    impl Drop for CallerItemGuard {
        fn drop(&mut self) {
            unsafe {
                assert_eq!(*HandledExceptionState::current_item_slot(), self.item);
                let replaced = ptr::replace(&mut (*self.item).exc_value, self.previous);
                ffi::Py_XDECREF(replaced);
            }
        }
    }

    unsafe fn set_handled(value: *mut ffi::PyObject) {
        unsafe {
            PyErr_SetHandledException(value);
        }
    }

    unsafe fn assert_handled(value: *mut ffi::PyObject) {
        let current = unsafe { PyErr_GetHandledException() };
        assert_eq!(current, value);
        unsafe { ffi::Py_XDECREF(current) };
    }

    unsafe extern "C" fn injection_input_destructor(capsule: *mut ffi::PyObject) {
        let destroyed = unsafe {
            ffi::PyCapsule_GetPointer(capsule, c"managed-injection-invalid-input".as_ptr())
                .cast::<Arc<std::sync::atomic::AtomicBool>>()
        };
        if !destroyed.is_null() {
            let destroyed = unsafe { Box::from_raw(destroyed) };
            destroyed.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        unsafe {
            ffi::PyErr_SetString(ffi::PyExc_ValueError, c"input finalizer error".as_ptr());
        }
    }

    #[test]
    fn terminal_scope_retirement_precedes_detach_and_residual_release() -> PyResult<()> {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let _caller_guard = CallerItemGuard::new();
            let module = pyo3::types::PyModule::from_code(
                py,
                c"import sys\nevents = []\nclass Observed(Exception):\n    def __del__(self):\n        current = sys.exception()\n        events.append((self.args[0], None if current is None else current.args[0]))\n",
                c"handled_terminal_phases.py",
                c"handled_terminal_phases",
            )?;
            let caller = pyo3::exceptions::PyKeyError::new_err("caller");
            set_handled(caller.value(py).as_ptr());
            let mut state = ActiveTestState(
                OwnedHandledExceptionState::new(&plan(&["source_handler"]), true).unwrap(),
            );
            state.enter();
            let observed = module.getattr("Observed")?;
            let residual = observed.call1(("residual",))?;
            set_handled(residual.as_ptr());
            drop(residual);
            let semantic = observed.call1(("semantic",))?;
            let semantic_identity = semantic.as_ptr();
            assert_eq!(
                HandledExceptionState::select(
                    state.state(),
                    &[HandledExceptionRegion {
                        scope: 1,
                        exception: semantic.as_ptr()
                    }],
                    HandledExceptionTransition::Enter,
                ),
                0
            );
            drop(semantic);
            let pending = pyo3::exceptions::PyValueError::new_err("independent raised error");
            let pending = pending.value(py).as_ptr();
            // PyErr's lazy normalization is itself a native raise while the
            // semantic item is active, so it legitimately pins that item as
            // context. This fixture needs an independent raised error: prove
            // and remove only that edge before testing the retirement phases.
            let context = ffi::PyException_GetContext(pending);
            assert_eq!(context, semantic_identity);
            ffi::Py_XDECREF(context);
            ffi::PyException_SetContext(pending, ptr::null_mut());
            ffi::Py_INCREF(pending);
            ffi::PyErr_SetRaisedException(pending);

            HandledExceptionState::retire_scopes_and_detach(state.state(), false);
            let raised = ffi::PyErr_GetRaisedException();
            assert_eq!(raised, pending);
            assert_handled(caller.value(py).as_ptr());
            assert!(
                !state.0.item.exc_value.is_null(),
                "residual must survive until after source/frame cleanup"
            );
            assert_eq!(
                module
                    .getattr("events")?
                    .extract::<Vec<(String, Option<String>)>>()?,
                vec![("semantic".into(), Some("residual".into()))],
                "semantic POP_EXCEPT finalizer runs before thread detach with restored own item"
            );
            ffi::PyErr_SetRaisedException(raised);

            HandledExceptionState::release_residual(state.state());
            let raised = ffi::PyErr_GetRaisedException();
            assert_eq!(raised, pending);
            ffi::Py_DECREF(raised);
            assert!(state.0.item.exc_value.is_null());
            assert_handled(caller.value(py).as_ptr());
            assert_eq!(
                module
                    .getattr("events")?
                    .extract::<Vec<(String, Option<String>)>>()?,
                vec![
                    ("semantic".into(), Some("residual".into())),
                    ("residual".into(), Some("caller".into())),
                ]
            );
            HandledExceptionState::release_residual(state.state());
            assert_eq!(module.getattr("events")?.len()?, 2);
            Ok(())
        })
    }

    #[test]
    fn managed_injection_preserves_validation_error_while_consuming_its_last_input_ref() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| unsafe {
            let _caller_guard = CallerItemGuard::new();
            let mut activation =
                ActiveTestState(OwnedHandledExceptionState::new(&plan(&[]), true).unwrap());
            activation.enter();
            let destroyed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let payload = Box::into_raw(Box::new(Arc::clone(&destroyed)));
            // Deliberately not an exception or a strict-state capsule. The
            // rejected input's sole owned reference must still be consumed.
            let input = ffi::PyCapsule_New(
                payload.cast(),
                c"managed-injection-invalid-input".as_ptr(),
                Some(injection_input_destructor),
            );
            if input.is_null() {
                drop(Box::from_raw(payload));
                panic!("invalid input fixture must allocate");
            }
            assert_eq!(
                OwnedHandledExceptionState::inject_managed_exception_owned(
                    activation.0.as_mut(),
                    input
                ),
                -1
            );
            assert!(
                destroyed.load(std::sync::atomic::Ordering::SeqCst),
                "rejected input must not retain its transferred owner"
            );
            assert_ne!(ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError), 0);
            ffi::PyErr_Clear();
            assert_eq!(
                OwnedHandledExceptionState::inject_managed_exception_owned(
                    activation.0.as_mut(),
                    ptr::null_mut()
                ),
                -1
            );
            assert_ne!(ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError), 0);
            ffi::PyErr_Clear();
        });
    }

    #[test]
    fn managed_injection_uses_only_its_own_item_and_consumes_the_input() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| unsafe {
            let _caller_guard = CallerItemGuard::new();
            let caller = ffi::PyObject_CallNoArgs(ffi::PyExc_RuntimeError);
            let previous_context = ffi::PyObject_CallNoArgs(ffi::PyExc_KeyError);
            let local_context = ffi::PyObject_CallNoArgs(ffi::PyExc_LookupError);
            assert!(!caller.is_null() && !previous_context.is_null() && !local_context.is_null());
            let before_caller = ffi::Py_REFCNT(caller);
            let before_previous = ffi::Py_REFCNT(previous_context);
            let before_local = ffi::Py_REFCNT(local_context);
            set_handled(caller);

            for own in [ptr::null_mut(), ffi::Py_None(), local_context] {
                let mut activation =
                    ActiveTestState(OwnedHandledExceptionState::new(&plan(&[]), true).unwrap());
                activation.enter();
                ffi::Py_XINCREF(own);
                activation.0.item.exc_value = own;
                let exception = ffi::PyObject_CallNoArgs(ffi::PyExc_ValueError);
                assert!(!exception.is_null());
                ffi::Py_INCREF(previous_context);
                ffi::PyException_SetContext(exception, previous_context);
                let before_exception = ffi::Py_REFCNT(exception);
                ffi::Py_INCREF(exception);
                assert_eq!(
                    OwnedHandledExceptionState::inject_managed_exception_owned(
                        activation.0.as_mut(),
                        exception
                    ),
                    0,
                );
                let raised = ffi::PyErr_GetRaisedException();
                assert_eq!(raised, exception);
                let context = ffi::PyException_GetContext(exception);
                assert_eq!(
                    context,
                    if own == local_context {
                        local_context
                    } else {
                        previous_context
                    },
                );
                ffi::Py_XDECREF(context);
                ffi::Py_DECREF(raised);
                assert_eq!(ffi::Py_REFCNT(exception), before_exception);
                ffi::Py_DECREF(exception);
                drop(activation);
                assert_handled(caller);
                assert_eq!(ffi::Py_REFCNT(previous_context), before_previous);
                assert_eq!(ffi::Py_REFCNT(local_context), before_local);
            }
            set_handled(ptr::null_mut());
            assert_eq!(ffi::Py_REFCNT(caller), before_caller);
            ffi::Py_DECREF(caller);
            ffi::Py_DECREF(previous_context);
            ffi::Py_DECREF(local_context);
        });
    }

    #[test]
    fn managed_injection_rejects_inactive_normal_and_non_top_owners_without_leaking() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| unsafe {
            let _caller_guard = CallerItemGuard::new();
            let exception = ffi::PyObject_CallNoArgs(ffi::PyExc_ValueError);
            assert!(!exception.is_null());
            let before = ffi::Py_REFCNT(exception);
            let reject = |owner: *mut OwnedHandledExceptionState| {
                ffi::Py_INCREF(exception);
                assert_eq!(
                    OwnedHandledExceptionState::inject_managed_exception_owned(owner, exception),
                    -1
                );
                assert_ne!(ffi::PyErr_ExceptionMatches(ffi::PyExc_RuntimeError), 0);
                ffi::PyErr_Clear();
                assert_eq!(ffi::Py_REFCNT(exception), before);
            };
            reject(ptr::null_mut());
            let mut inactive = OwnedHandledExceptionState::new(&plan(&[]), true).unwrap();
            reject(inactive.as_mut());
            let mut normal =
                ActiveTestState(OwnedHandledExceptionState::new(&plan(&[]), false).unwrap());
            normal.enter();
            reject(normal.0.as_mut());
            drop(normal);
            let mut outer =
                ActiveTestState(OwnedHandledExceptionState::new(&plan(&[]), true).unwrap());
            outer.enter();
            let mut inner =
                ActiveTestState(OwnedHandledExceptionState::new(&plan(&[]), true).unwrap());
            inner.enter();
            reject(outer.0.as_mut());
            drop(inner);
            outer.finish(false);
            reject(outer.0.as_mut());
            drop(outer);
            ffi::Py_DECREF(exception);
        });
    }

    #[test]
    fn original_region_ids_survive_native_reordering_and_added_regions() {
        use soac_core::block_py::{BlockParam, BlockParamRole};
        let module = soac_lowering::lower_python_to_blockpy_for_testing(
            "def f():\n    try:\n        raise ValueError()\n    except ValueError:\n        try:\n            raise TypeError()\n        except TypeError:\n            return 1\n",
        ).unwrap().blockpy_module;
        let original = module
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "f")
            .unwrap();
        let mut shared = HandledExceptionPlan::for_function(original);
        assert!(shared.source_scopes() >= 2);
        let original_layout = Arc::clone(&shared.source_layout);
        let original_ids = shared.scopes.clone();
        let mut native = original.clone();
        native.blocks.reverse();
        native.blocks[0].params.push(BlockParam {
            name: "native_inlined_exception".into(),
            role: BlockParamRole::EnclosingException,
        });
        shared.include_native_regions(&native);
        assert_eq!(shared.source_layout, original_layout);
        for (name, id) in original_ids {
            assert_eq!(shared.scope(&name), id);
        }
        assert_eq!(
            shared.scope("native_inlined_exception"),
            shared.source_scopes() + 1
        );
    }

    #[test]
    fn suspended_state_rejects_equal_count_changed_layout_and_allows_native_capacity_growth() {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| unsafe {
            let original = plan(&["outer", "inner"]);
            let mut state =
                ActiveTestState(OwnedHandledExceptionState::new(&original, true).unwrap());
            for other in [plan(&["inner", "outer"]), plan(&["outer", "different"])] {
                assert!(state.0.prepare_plan(&other).is_err());
                assert_ne!(ffi::PyErr_ExceptionMatches(ffi::PyExc_RuntimeError), 0);
                ffi::PyErr_Clear();
                assert_eq!(state.0.source_layout, original.source_layout);
            }
            let mut grown = original.clone();
            grown.scopes.insert("native_only".into(), 3);
            state.0.prepare_plan(&grown).unwrap();
            assert_eq!(state.0.state.capacity, 3);
            assert_eq!(state.0.state.records, state.0.records.as_mut_ptr());
            assert_eq!(state.0.source_layout, original.source_layout);
        });
    }

    #[test]
    fn suspended_current_item_preserves_nested_capi_changes_and_detaches_the_caller() {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let _caller = CallerItemGuard::new();
            let outer = pyo3::exceptions::PyValueError::new_err("outer");
            let inner = pyo3::exceptions::PyTypeError::new_err("inner");
            let replacement = pyo3::exceptions::PyRuntimeError::new_err("replacement");
            let caller = pyo3::exceptions::PyOSError::new_err("caller");
            let outer = outer.value(py).as_ptr();
            let inner = inner.value(py).as_ptr();
            let replacement = replacement.value(py).as_ptr();
            let caller = caller.value(py).as_ptr();
            let replacement_refs = ffi::Py_REFCNT(replacement);
            let original = plan(&["outer", "inner"]);
            let mut state =
                ActiveTestState(OwnedHandledExceptionState::new(&original, true).unwrap());
            let outer_region = HandledExceptionRegion {
                scope: 1,
                exception: outer,
            };
            let inner_region = HandledExceptionRegion {
                scope: 2,
                exception: inner,
            };
            state.enter();
            assert_eq!(
                HandledExceptionState::select(
                    state.state(),
                    &[outer_region],
                    HandledExceptionTransition::Enter
                ),
                0
            );
            set_handled(replacement);
            assert_eq!(
                HandledExceptionState::select(
                    state.state(),
                    &[outer_region, inner_region],
                    HandledExceptionTransition::Enter
                ),
                0
            );
            assert_handled(inner);
            state.finish(true);
            assert_handled(ptr::null_mut());
            assert!(state.0.item.previous_item.is_null());
            set_handled(caller);
            let mut grown = original.clone();
            grown.scopes.insert("native_only".into(), 3);
            state.0.prepare_plan(&grown).unwrap();
            assert_eq!(state.0.records[0].scope, 1);
            assert_eq!(state.0.records[1].scope, 2);
            state.enter();
            assert_handled(inner);
            assert_eq!(
                HandledExceptionState::select(
                    state.state(),
                    &[outer_region],
                    HandledExceptionTransition::Enter
                ),
                0
            );
            assert_handled(replacement);
            assert_eq!(
                HandledExceptionState::select(
                    state.state(),
                    &[],
                    HandledExceptionTransition::Enter
                ),
                0
            );
            assert!(state.0.item.exc_value.is_null());
            assert_handled(caller);
            state.finish(false);
            assert_handled(caller);
            assert!(state.0.item.previous_item.is_null());
            assert_eq!(ffi::Py_REFCNT(replacement), replacement_refs);
        });
    }

    #[test]
    fn suspended_exception_projection_follows_capi_replacement_without_inheriting_caller() {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let _caller = CallerItemGuard::new();
            let outside = pyo3::exceptions::PyOSError::new_err("outside");
            let caught = pyo3::exceptions::PyValueError::new_err("caught");
            let replacement = pyo3::exceptions::PyRuntimeError::new_err("replacement");
            let pending = pyo3::exceptions::PyTypeError::new_err("independent pending error");
            let outside = outside.value(py).as_ptr();
            let caught = caught.value(py).as_ptr();
            let replacement = replacement.value(py).as_ptr();
            let pending = pending.value(py).as_ptr();
            let caught_refs = ffi::Py_REFCNT(caught);
            let replacement_refs = ffi::Py_REFCNT(replacement);
            set_handled(outside);
            let mut owner = ActiveTestState(
                OwnedHandledExceptionState::new(&plan(&["handler"]), true).unwrap(),
            );
            let empty = owner.0.suspended_exception_owned();
            assert_eq!(empty, ffi::Py_None());
            ffi::Py_DECREF(empty);
            owner.enter();
            // A NULL own item resolves to the caller through the public C API,
            // but the explicit activation projection must still return None.
            assert_handled(outside);
            let empty = owner.0.suspended_exception_owned();
            assert_eq!(empty, ffi::Py_None());
            ffi::Py_DECREF(empty);
            assert_eq!(
                HandledExceptionState::select(
                    owner.state(),
                    &[HandledExceptionRegion {
                        scope: 1,
                        exception: caught
                    }],
                    HandledExceptionTransition::Enter,
                ),
                0,
            );
            let observed = owner.0.suspended_exception_owned();
            assert_eq!(observed, caught);
            assert_eq!(ffi::Py_REFCNT(caught), caught_refs + 2);
            ffi::Py_DECREF(observed);
            set_handled(replacement);
            assert_eq!(ffi::Py_REFCNT(caught), caught_refs);
            owner.finish(true);
            assert_handled(outside);
            ffi::Py_INCREF(pending);
            ffi::PyErr_SetRaisedException(pending);
            let observed = owner.0.suspended_exception_owned();
            assert_eq!(observed, replacement);
            assert_eq!(ffi::Py_REFCNT(replacement), replacement_refs + 2);
            ffi::Py_DECREF(observed);
            let raised = ffi::PyErr_GetRaisedException();
            assert_eq!(raised, pending);
            ffi::Py_DECREF(raised);
            owner.enter();
            set_handled(ffi::Py_None());
            let empty = owner.0.suspended_exception_owned();
            assert_eq!(empty, ffi::Py_None());
            ffi::Py_DECREF(empty);
            owner.finish(false);
            let closed = owner.0.suspended_exception_owned();
            assert_eq!(closed, ffi::Py_None());
            ffi::Py_DECREF(closed);
            assert_eq!(ffi::Py_REFCNT(replacement), replacement_refs);
            assert_handled(outside);
        });
    }

    #[test]
    fn active_region_identity_survives_retired_payload_and_capi_replacement() {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            for suspended in [false, true] {
                let _caller = CallerItemGuard::new();
                let caller = pyo3::exceptions::PyOSError::new_err("caller");
                let caught = pyo3::exceptions::PyValueError::new_err("caught");
                let replacement = pyo3::exceptions::PyRuntimeError::new_err("replacement");
                let caller = caller.value(py).as_ptr();
                let caught = caught.value(py).as_ptr();
                let replacement = replacement.value(py).as_ptr();
                let caught_refs = ffi::Py_REFCNT(caught);
                set_handled(caller);
                let mut state = ActiveTestState(
                    OwnedHandledExceptionState::new(&plan(&["handler"]), suspended).unwrap(),
                );
                state.enter();
                assert_eq!(
                    HandledExceptionState::select(
                        state.state(),
                        &[HandledExceptionRegion {
                            scope: 1,
                            exception: caught
                        },],
                        HandledExceptionTransition::Enter
                    ),
                    0
                );
                set_handled(replacement);
                assert_eq!(ffi::Py_REFCNT(caught), caught_refs);
                if suspended {
                    state.finish(true);
                    assert_handled(caller);
                    state.enter();
                }
                // The scope is still active, but its original caught-object
                // operand has no remaining semantic reads and has been reset.
                let retired = [HandledExceptionRegion {
                    scope: 1,
                    exception: ffi::Py_None(),
                }];
                assert_eq!(
                    HandledExceptionState::select(
                        state.state(),
                        &retired,
                        HandledExceptionTransition::Leave
                    ),
                    0
                );
                assert_eq!(
                    HandledExceptionState::select(
                        state.state(),
                        &retired,
                        HandledExceptionTransition::Enter
                    ),
                    0
                );
                let current = PyErr_GetHandledException();
                ffi::Py_XDECREF(current);
                assert_eq!(
                    current, replacement,
                    "continuing a scope must keep its C-API-selected value without the old transport"
                );
                assert_eq!((*state.state()).depth, 1);
                state.finish(false);
                assert_handled(caller);
            }
        });
    }

    #[test]
    fn trim_only_unwind_preserves_pending_reentry_and_the_current_native_value() {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            for suspended in [false, true] {
                let _caller = CallerItemGuard::new();
                let caller = pyo3::exceptions::PyOSError::new_err("caller");
                let outer = pyo3::exceptions::PyValueError::new_err("outer");
                let inner = pyo3::exceptions::PyTypeError::new_err("inner");
                let replacement = pyo3::exceptions::PyRuntimeError::new_err("replacement");
                let incoming = pyo3::exceptions::PyLookupError::new_err("incoming");
                let caller = caller.value(py).as_ptr();
                let outer = outer.value(py).as_ptr();
                let inner = inner.value(py).as_ptr();
                let replacement = replacement.value(py).as_ptr();
                let incoming = incoming.value(py).as_ptr();
                let incoming_refs = ffi::Py_REFCNT(incoming);
                set_handled(caller);
                let mut state = ActiveTestState(
                    OwnedHandledExceptionState::new(&plan(&["outer", "inner"]), suspended).unwrap(),
                );
                state.enter();
                let outer_region = HandledExceptionRegion {
                    scope: 1,
                    exception: outer,
                };
                assert_eq!(
                    HandledExceptionState::select(
                        state.state(),
                        &[outer_region],
                        HandledExceptionTransition::Enter
                    ),
                    0
                );
                set_handled(replacement);
                assert_eq!(
                    HandledExceptionState::select(
                        state.state(),
                        &[
                            outer_region,
                            HandledExceptionRegion {
                                scope: 2,
                                exception: inner
                            },
                        ],
                        HandledExceptionTransition::Enter
                    ),
                    0
                );
                HandledExceptionState::mark_raised(state.state(), 1);
                ffi::Py_INCREF(incoming);
                ffi::PyErr_SetRaisedException(incoming);
                // This operand is the newly raised object, not the currently
                // active outer value. A trim must not enter or consume it.
                let target = [HandledExceptionRegion {
                    scope: 1,
                    exception: incoming,
                }];
                assert_eq!(
                    HandledExceptionState::select(
                        state.state(),
                        &target,
                        HandledExceptionTransition::Unwind
                    ),
                    0
                );
                assert_eq!((*state.state()).depth, 1);
                assert_eq!((*state.state()).pending_scope, 1);
                assert_handled(replacement);
                let raised = ffi::PyErr_GetRaisedException();
                assert_eq!(raised, incoming);
                ffi::Py_DECREF(raised);
                assert_eq!(ffi::Py_REFCNT(incoming), incoming_refs);
                assert_eq!(
                    HandledExceptionState::select(
                        state.state(),
                        &target,
                        HandledExceptionTransition::Enter
                    ),
                    0
                );
                assert_eq!((*state.state()).pending_scope, 0);
                assert_handled(incoming);
                state.finish(false);
                assert_handled(caller);
                assert_eq!(ffi::Py_REFCNT(incoming), incoming_refs);
            }
        });
    }

    #[test]
    fn normal_calls_share_the_current_item_and_cleanup_preserves_pending_errors() {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let caller_guard = CallerItemGuard::new();
            let caller = pyo3::exceptions::PyOSError::new_err("caller");
            let caught = pyo3::exceptions::PyValueError::new_err("caught");
            let pending = pyo3::exceptions::PyRuntimeError::new_err("raised independently");
            let caller = caller.value(py).as_ptr();
            let caught = caught.value(py).as_ptr();
            let pending = pending.value(py).as_ptr();
            let mut state = ActiveTestState(
                OwnedHandledExceptionState::new(&plan(&["handler"]), false).unwrap(),
            );
            state.enter();
            assert_eq!(state.0.state.item, caller_guard.item);
            set_handled(caller);
            let region = HandledExceptionRegion {
                scope: 1,
                exception: caught,
            };
            assert_eq!(
                HandledExceptionState::select(
                    state.state(),
                    &[region],
                    HandledExceptionTransition::Enter
                ),
                0
            );
            assert_handled(caught);
            ffi::Py_INCREF(pending);
            ffi::PyErr_SetRaisedException(pending);
            assert_eq!(
                HandledExceptionState::select(
                    state.state(),
                    &[],
                    HandledExceptionTransition::Leave
                ),
                0
            );
            assert_handled(caller);
            state.finish(false);
            OwnedHandledExceptionState::clear(state.0.as_mut());
            let actual = ffi::PyErr_GetRaisedException();
            assert_eq!(actual, pending);
            ffi::Py_DECREF(actual);
            assert_handled(caller);
        });
    }

    #[test]
    fn raw_handled_item_matches_selected_native_header_probe() {
        // Actual C offsetof/sizeof evidence is exercised by
        // test_selected_native_handled_exception_layout, compiled against the
        // selected interpreter. This is the matching Rust mirror, not a second
        // inferred topmost-exception ABI.
        assert_eq!(crate::jit::PY_THREAD_STATE_EXC_INFO_OFFSET, 136);
        assert_eq!(std::mem::size_of::<RawPyErrStackItem>(), 16);
        assert_eq!(std::mem::align_of::<RawPyErrStackItem>(), 8);
        assert_eq!(std::mem::offset_of!(RawPyErrStackItem, exc_value), 0);
        assert_eq!(std::mem::offset_of!(RawPyErrStackItem, previous_item), 8);
    }
}
