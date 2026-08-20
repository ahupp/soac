//! Opaque, GC-owned state for native strict capabilities.
//!
//! Rust payloads contain no owning Python references. Every Python edge is in
//! one explicitly traversed vector. Clearing is terminal and occurs before
//! decrefs, including when a finalizer re-enters an escaped owner.

use std::any::TypeId;
use std::cell::{Cell, UnsafeCell};
use std::ffi::{CStr, c_int, c_void};
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

use pyo3::ffi;
use pyo3::prelude::*;

/// # Safety
/// The payload and its destructor must contain/retain no owning Python edges
/// and must not call Python. Store those edges in the owner's reference vector
/// instead. Interior mutation is permitted only under the attached GIL, with
/// no mutable Rust borrow held across a Python call or decref.
pub(crate) unsafe trait StrictStateData: 'static {
    const TYPE_NAME: &'static CStr;
    /// Rust-only notification, before any owning edge is decrefed.
    fn on_terminal(&self) {}
}

#[repr(C)]
struct NativeStrictState {
    object: ffi::PyObject,
    payload: *mut c_void,
    payload_type: std::mem::MaybeUninit<TypeId>,
}

struct Payload<T> {
    terminal: Cell<bool>,
    data: T,
    references: UnsafeCell<Vec<Py<PyAny>>>,
}

/// This view owns a Python reference while Rust data is borrowed. The heap
/// type is immutable and has no Python constructor, attributes, or setters.
pub(crate) struct StrictStateRef<'py, T: StrictStateData> {
    owner: Bound<'py, PyAny>,
    payload: NonNull<Payload<T>>,
    _kind: PhantomData<T>,
}

impl<'py, T: StrictStateData> StrictStateRef<'py, T> {
    /// Borrow a live metadata payload for a callback-free native observation
    /// or scalar transition. Unlike teardown inspection, terminal shells do
    /// not authorize an observation. No Python edge or exception is touched.
    ///
    /// # Safety
    /// The native caller supports `owner` for this whole operation. `inspect`
    /// must not call Python, acquire/release Python references, or let a
    /// payload borrow escape. Its result may contain only copied Rust data.
    pub(crate) unsafe fn inspect_live<R>(
        owner: *mut ffi::PyObject,
        inspect: impl FnOnce(&T) -> R,
    ) -> Option<R> {
        if owner.is_null() || !unsafe { has_native_payload_type::<T>(owner) } {
            return None;
        }
        let raw = owner.cast::<NativeStrictState>();
        let payload = NonNull::new(unsafe { (*raw).payload.cast::<Payload<T>>() })?;
        if unsafe { *(*raw).payload_type.assume_init_ref() } != TypeId::of::<T>() {
            return None;
        }
        let payload = unsafe { payload.as_ref() };
        if payload.terminal.get() {
            return None;
        }
        Some(inspect(&payload.data))
    }

    /// Scalar-only native retirement hooks must not allocate, acquire Python
    /// edges, or replace the pending exception. A dead/foreign payload has
    /// nothing left to retire. This is not live execution authentication.
    ///
    /// # Safety
    /// `owner` is a live C-owned Python object throughout this call. `inspect`
    /// must not call Python, release Python references, or let this borrow
    /// escape; it may only retire Rust metadata through interior mutability.
    pub(crate) unsafe fn inspect_for_teardown<R>(
        owner: *mut ffi::PyObject,
        inspect: impl FnOnce(&T) -> R,
    ) -> Option<R> {
        if owner.is_null() {
            return None;
        }
        if !unsafe { has_native_payload_type::<T>(owner) } {
            return None;
        }
        let raw = owner.cast::<NativeStrictState>();
        let payload = NonNull::new(unsafe { (*raw).payload.cast::<Payload<T>>() })?;
        if unsafe { *(*raw).payload_type.assume_init_ref() } != TypeId::of::<T>() {
            return None;
        }
        Some(inspect(&unsafe { payload.as_ref() }.data))
    }

    pub(crate) fn new(py: Python<'py>, data: T, references: Vec<Py<PyAny>>) -> PyResult<Self> {
        // Native interpreter/JIT callbacks can own the GIL without a PyO3
        // attachment registration. Keep stack edges Bound until ownership is
        // published so allocation failures release them now, not at a later
        // unrelated Python::attach. The native GC vector owns Py values only
        // after successful allocation, with explicit native teardown below.
        let references: Vec<_> = references
            .into_iter()
            .map(|reference| reference.into_bound(py))
            .collect();
        // A per-owner type keeps interpreter ownership explicit. A future
        // module-execution-owned type cache may amortize this cold allocation;
        // a process-global Python reference is deliberately not introduced.
        let mut slots = [
            ffi::PyType_Slot {
                slot: ffi::Py_tp_dealloc,
                pfunc: deallocate::<T> as *mut c_void,
            },
            ffi::PyType_Slot {
                slot: ffi::Py_tp_traverse,
                pfunc: traverse::<T> as *mut c_void,
            },
            ffi::PyType_Slot {
                slot: ffi::Py_tp_clear,
                pfunc: clear::<T> as *mut c_void,
            },
            ffi::PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = ffi::PyType_Spec {
            name: T::TYPE_NAME.as_ptr(),
            basicsize: std::mem::size_of::<NativeStrictState>() as c_int,
            itemsize: 0,
            flags: (ffi::Py_TPFLAGS_DEFAULT
                | ffi::Py_TPFLAGS_HAVE_GC
                | ffi::Py_TPFLAGS_IMMUTABLETYPE
                | ffi::Py_TPFLAGS_DISALLOW_INSTANTIATION) as _,
            slots: slots.as_mut_ptr(),
        };
        let owner_type =
            unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, ffi::PyType_FromSpec(&mut spec))? };
        let owner = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                ffi::PyType_GenericAlloc(owner_type.as_ptr().cast(), 0),
            )?
        };
        let payload = NonNull::from(Box::leak(Box::new(Payload {
            terminal: Cell::new(false),
            data,
            references: UnsafeCell::new(references.into_iter().map(Bound::unbind).collect()),
        })));
        // GenericAlloc zeroes the tail, so a GC traversal before publication
        // sees a null payload rather than uninitialized Rust storage.
        unsafe {
            (*owner.as_ptr().cast::<NativeStrictState>())
                .payload_type
                .write(TypeId::of::<T>());
            (*owner.as_ptr().cast::<NativeStrictState>()).payload = payload.as_ptr().cast();
        }
        Ok(Self {
            owner,
            payload,
            _kind: PhantomData,
        })
    }

    pub(crate) fn from_owner(owner: Bound<'py, PyAny>) -> PyResult<Self> {
        let state = Self::from_owner_for_teardown(owner)?;
        state.ensure_live()?;
        Ok(state)
    }

    /// Optional participation may decline a foreign owner or another payload
    /// role. A recognized destroyed/terminal owner remains an error, never a
    /// reason to weaken an already installed contract.
    pub(crate) fn try_from_owner(owner: Bound<'py, PyAny>) -> PyResult<Option<Self>> {
        let Some(state) = Self::try_from_owner_for_teardown(owner)? else {
            return Ok(None);
        };
        state.ensure_live()?;
        Ok(Some(state))
    }

    /// Only terminal dictionary/GC callbacks may use this view. It validates
    /// the native role but intentionally does not confer a live capability.
    pub(crate) fn from_owner_for_teardown(owner: Bound<'py, PyAny>) -> PyResult<Self> {
        let py = owner.py();
        Self::try_from_owner_for_teardown(owner)?.ok_or_else(|| {
            crate::strict_runtime_unavailable(
                py,
                "native strict owner has the wrong immutable type or payload role",
            )
        })
    }

    fn try_from_owner_for_teardown(owner: Bound<'py, PyAny>) -> PyResult<Option<Self>> {
        let py = owner.py();
        if !unsafe { has_native_payload_type::<T>(owner.as_ptr()) } {
            return Ok(None);
        }
        let raw = owner.as_ptr().cast::<NativeStrictState>();
        let payload =
            NonNull::new(unsafe { (*raw).payload.cast::<Payload<T>>() }).ok_or_else(|| {
                crate::strict_runtime_unavailable(py, "native strict owner was destroyed")
            })?;
        // Function addresses may be merged by the linker; the private Rust
        // TypeId is the definitive payload discriminator after checking that
        // this is one of our immutable native owner types.
        if unsafe { *(*raw).payload_type.assume_init_ref() } != TypeId::of::<T>() {
            return Ok(None);
        }
        Ok(Some(Self {
            owner,
            payload,
            _kind: PhantomData,
        }))
    }

    pub(crate) fn ensure_live(&self) -> PyResult<()> {
        if unsafe { self.payload.as_ref() }.terminal.get() {
            Err(crate::strict_runtime_unavailable(
                self.owner.py(),
                "native strict owner is terminal",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn owner(&self) -> &Bound<'py, PyAny> {
        &self.owner
    }
    pub(crate) fn data(&self) -> &T {
        &unsafe { self.payload.as_ref() }.data
    }

    /// Borrow a GC-vector edge without acquiring another Python owner.
    ///
    /// # Safety
    /// Use the pointer only for immediate validation while this owning shell
    /// guard and the edge's current support remain live. It must not escape or
    /// survive a callback, replacement, clear, or retirement of that support.
    /// A metadata guard alone does not pin objects removed from its GC vector.
    pub(crate) unsafe fn reference_ptr(&self, index: usize) -> PyResult<NonNull<ffi::PyObject>> {
        self.ensure_live()?;
        let pointer = {
            let references = unsafe { &*self.payload.as_ref().references.get() };
            references.get(index).map(|value| value.as_ptr())
        };
        pointer.and_then(NonNull::new).ok_or_else(|| {
            crate::strict_runtime_unavailable(
                self.owner.py(),
                "native strict owner borrowed reference is absent",
            )
        })
    }

    pub(crate) fn reference(&self, index: usize) -> PyResult<Bound<'py, PyAny>> {
        self.ensure_live()?;
        let references = unsafe { &*self.payload.as_ref().references.get() };
        references
            .get(index)
            .map(|value| value.clone_ref(self.owner.py()).into_bound(self.owner.py()))
            .ok_or_else(|| {
                crate::strict_runtime_unavailable(
                    self.owner.py(),
                    "native strict owner reference is absent",
                )
            })
    }

    pub(crate) fn add_reference(&self, reference: Bound<'py, PyAny>) -> PyResult<usize> {
        self.ensure_live()?;
        let references = unsafe { &mut *self.payload.as_ref().references.get() };
        let index = references.len();
        references.push(reference.unbind());
        Ok(index)
    }

    /// Use a `None` edge as a reserved/removed slot. Callers must not retain a
    /// mutable borrow of their Rust data across this operation: decref of the
    /// replaced object can re-enter Python after the new edge is installed.
    pub(crate) fn set_reference(&self, index: usize, reference: Bound<'py, PyAny>) -> PyResult<()> {
        self.ensure_live()?;
        let previous = {
            let references = unsafe { &mut *self.payload.as_ref().references.get() };
            let slot = references.get_mut(index).ok_or_else(|| {
                crate::strict_runtime_unavailable(
                    self.owner.py(),
                    "native strict owner reference is absent",
                )
            })?;
            std::mem::replace(slot, reference.unbind())
        };
        unsafe {
            release_references(self.owner.as_ptr(), [previous]);
        }
        self.ensure_live()
    }

    /// Adopt a preallocated `None` edge during native pre-Ready binding. The
    /// successful path allocates nothing and can release only the immortal
    /// None singleton, so it cannot call Python or expose a half-bound type.
    pub(crate) fn bind_reserved_reference(
        &self,
        index: usize,
        reference: Bound<'py, PyAny>,
    ) -> PyResult<()> {
        self.ensure_live()?;
        let references = unsafe { &*self.payload.as_ref().references.get() };
        if references
            .get(index)
            .is_none_or(|previous| previous.as_ptr() != unsafe { ffi::Py_None() })
        {
            return Err(crate::strict_runtime_unavailable(
                self.owner.py(),
                "native strict binding requires a reserved None edge",
            ));
        }
        let previous = unsafe {
            let references = &mut *self.payload.as_ref().references.get();
            std::mem::replace(&mut references[index], reference.unbind())
        };
        unsafe {
            ffi::Py_DECREF(previous.into_ptr());
        }
        Ok(())
    }
}

/// Callback-free layout check shared by live and terminal views. The private
/// payload TypeId still has to match before casting the payload itself.
unsafe fn has_native_payload_type<T: StrictStateData>(owner: *mut ffi::PyObject) -> bool {
    let kind = unsafe { ffi::Py_TYPE(owner) };
    unsafe {
        (*kind).tp_basicsize == std::mem::size_of::<NativeStrictState>() as ffi::Py_ssize_t
            && (*kind).tp_flags & ffi::Py_TPFLAGS_IMMUTABLETYPE != 0
            && (*kind).tp_flags & ffi::Py_TPFLAGS_BASETYPE == 0
            && (*kind)
                .tp_dealloc
                .is_some_and(|slot| std::ptr::fn_addr_eq(slot, deallocate::<T> as ffi::destructor))
            && (*kind)
                .tp_traverse
                .is_some_and(|slot| std::ptr::fn_addr_eq(slot, traverse::<T> as ffi::traverseproc))
    }
}

#[cfg(test)]
mod tests {
    use pyo3::exceptions::PyUnicodeDecodeError;
    use pyo3::types::PyModule;

    use super::*;

    struct BirthRole;
    struct AnotherRole;

    // SAFETY: These identity-only fixtures contain no Python edges.
    unsafe impl StrictStateData for BirthRole {
        const TYPE_NAME: &'static CStr = c"soac._TestBirthRole";
    }
    unsafe impl StrictStateData for AnotherRole {
        const TYPE_NAME: &'static CStr = c"soac._TestAnotherRole";
    }

    struct InvalidTypeName;

    // SAFETY: This failure fixture contains no Python edges. The bytes are a
    // valid C string but deliberately cannot become a Python Unicode name.
    unsafe impl StrictStateData for InvalidTypeName {
        const TYPE_NAME: &'static CStr =
            unsafe { CStr::from_bytes_with_nul_unchecked(b"soac.\xff\0") };
    }

    #[test]
    fn failed_owner_allocation_releases_edges_in_an_unregistered_native_attachment() -> PyResult<()>
    {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        struct Attachment(ffi::PyGILState_STATE);
        impl Drop for Attachment {
            fn drop(&mut self) {
                unsafe { ffi::PyGILState_Release(self.0) };
            }
        }
        // Model an interpreter/JIT callback: CPython owns the GIL, but there
        // is no enclosing Python::attach registration in PyO3's Rust TLS.
        let _attachment = Attachment(unsafe { ffi::PyGILState_Ensure() });
        let py = unsafe { Python::assume_attached() };
        let module = PyModule::from_code(
            py,
            c"events = []\nclass Value:\n    def __del__(self):\n        events.append('released')\nvalue = Value()\n",
            c"<failed native state allocation>",
            c"failed_native_state_allocation",
        )?;
        let value = module.getattr("value")?;
        module.delattr("value")?;
        let Err(error) = StrictStateRef::new(py, InvalidTypeName, vec![value.unbind()]) else {
            panic!("invalid Unicode native owner name was accepted");
        };
        assert!(error.is_instance_of::<PyUnicodeDecodeError>(py));
        assert_eq!(
            module.getattr("events")?.extract::<Vec<String>>()?,
            ["released"],
            "a failed owner deferred its only Python edge until another attachment"
        );
        Ok(())
    }

    #[test]
    fn borrowed_gc_reference_does_not_pin_and_stops_at_support_retirement() -> PyResult<()> {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"events = []\nclass Value:\n    def __del__(self):\n        events.append('released')\nvalue = Value()\n",
                c"<borrowed strict metadata reference>",
                c"borrowed_strict_metadata_reference",
            )?;
            let value = module.getattr("value")?;
            module.delattr("value")?;
            let identity = value.as_ptr();
            let count = unsafe { ffi::Py_REFCNT(identity) };
            let state = StrictStateRef::new(py, BirthRole, vec![value.unbind()])?;
            assert_eq!(unsafe { state.reference_ptr(0)? }.as_ptr(), identity);
            assert_eq!(unsafe { ffi::Py_REFCNT(identity) }, count);
            let owned = state.reference(0)?;
            assert_eq!(unsafe { ffi::Py_REFCNT(identity) }, count + 1);
            drop(owned);
            let primary = pyo3::exceptions::PyValueError::new_err("borrowed read primary");
            let primary_identity = primary.value(py).as_ptr();
            primary.restore(py);
            assert_eq!(unsafe { state.reference_ptr(0)? }.as_ptr(), identity);
            assert_eq!(unsafe { ffi::Py_REFCNT(identity) }, count);
            assert_eq!(PyErr::fetch(py).value(py).as_ptr(), primary_identity);

            // No use of identity after the actual GC edge is replaced.
            state.set_reference(0, py.None().into_bound(py))?;
            assert_eq!(
                module.getattr("events")?.extract::<Vec<String>>()?,
                ["released"],
            );
            assert_eq!(unsafe { state.reference_ptr(0)? }.as_ptr(), unsafe {
                ffi::Py_None()
            });
            assert_eq!(unsafe { clear::<BirthRole>(state.owner().as_ptr()) }, 0);
            assert!(unsafe { state.reference_ptr(0) }.is_err());
            Ok(())
        })
    }

    #[test]
    fn optional_owner_role_declines_foreign_values_but_never_terminal_authority() -> PyResult<()> {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            assert!(
                StrictStateRef::<BirthRole>::try_from_owner(py.None().into_bound(py))?.is_none()
            );
            let other = StrictStateRef::new(py, AnotherRole, Vec::new())?;
            assert!(StrictStateRef::<BirthRole>::try_from_owner(other.owner().clone())?.is_none());
            let birth = StrictStateRef::new(py, BirthRole, Vec::new())?;
            assert!(StrictStateRef::<BirthRole>::try_from_owner(birth.owner().clone())?.is_some());
            let primary = pyo3::exceptions::PyValueError::new_err("native callback primary");
            let identity = primary.value(py).as_ptr();
            primary.restore(py);
            assert_eq!(
                unsafe {
                    StrictStateRef::<BirthRole>::inspect_live(birth.owner().as_ptr(), |_| 17)
                },
                Some(17)
            );
            assert!(
                unsafe {
                    StrictStateRef::<BirthRole>::inspect_live(other.owner().as_ptr(), |_| 17)
                }
                .is_none()
            );
            assert_eq!(PyErr::fetch(py).value(py).as_ptr(), identity);
            assert_eq!(unsafe { clear::<BirthRole>(birth.owner().as_ptr()) }, 0);
            assert!(
                unsafe {
                    StrictStateRef::<BirthRole>::inspect_live(birth.owner().as_ptr(), |_| 17)
                }
                .is_none()
            );
            assert_eq!(
                unsafe {
                    StrictStateRef::<BirthRole>::inspect_for_teardown(
                        birth.owner().as_ptr(),
                        |_| 17,
                    )
                },
                Some(17)
            );
            assert!(StrictStateRef::<BirthRole>::try_from_owner(birth.owner().clone()).is_err());
            assert!(
                StrictStateRef::<BirthRole>::from_owner_for_teardown(birth.owner().clone()).is_ok()
            );
            Ok(())
        })
    }
}

unsafe extern "C" fn traverse<T: StrictStateData>(
    owner: *mut ffi::PyObject,
    visit: ffi::visitproc,
    argument: *mut c_void,
) -> c_int {
    let result = unsafe { visit(ffi::Py_TYPE(owner).cast(), argument) };
    if result != 0 {
        return result;
    }
    let payload = unsafe {
        (*owner.cast::<NativeStrictState>())
            .payload
            .cast::<Payload<T>>()
    };
    if payload.is_null() {
        return 0;
    }
    for reference in unsafe { &*(*payload).references.get() } {
        let result = unsafe { visit(reference.as_ptr(), argument) };
        if result != 0 {
            return result;
        }
    }
    0
}

unsafe fn take_references<T: StrictStateData>(payload: *mut Payload<T>) -> Vec<Py<PyAny>> {
    // Cell/UnsafeCell make terminalization legal even if a native readonly
    // view exists during GC. No mutable borrow survives into Python decrefs.
    if !unsafe { (*payload).terminal.replace(true) } {
        unsafe {
            (*payload).data.on_terminal();
        }
    }
    unsafe { std::mem::take(&mut *(*payload).references.get()) }
}

/// Release GIL-owned edges immediately, including from raw native callbacks
/// outside PyO3's attachment registration. `owner` may be null when there is
/// no surviving object to identify in an unraisable cleanup error.
pub(crate) unsafe fn release_references(
    owner: *mut ffi::PyObject,
    references: impl IntoIterator<Item = Py<PyAny>>,
) {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    // Native GC can enter from a Python thread outside a PyO3 attachment
    // scope. The GIL is held; do not defer these GC-owned decrefs to a queue.
    for reference in references {
        unsafe {
            ffi::Py_DECREF(reference.into_ptr());
        }
    }
    if !unsafe { ffi::PyErr_Occurred() }.is_null() {
        unsafe {
            ffi::PyErr_WriteUnraisable(owner);
        }
    }
    unsafe {
        ffi::PyErr_SetRaisedException(error);
    }
}

unsafe extern "C" fn clear<T: StrictStateData>(owner: *mut ffi::PyObject) -> c_int {
    let payload = unsafe {
        (*owner.cast::<NativeStrictState>())
            .payload
            .cast::<Payload<T>>()
    };
    if !payload.is_null() {
        let references = unsafe { take_references(payload) };
        unsafe {
            release_references(owner, references);
        }
    }
    0
}

unsafe extern "C" fn deallocate<T: StrictStateData>(owner: *mut ffi::PyObject) {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    unsafe {
        ffi::PyObject_GC_UnTrack(owner.cast());
    }
    let payload = unsafe {
        (*owner.cast::<NativeStrictState>())
            .payload
            .cast::<Payload<T>>()
    };
    unsafe {
        (*owner.cast::<NativeStrictState>()).payload = ptr::null_mut();
    }
    if !payload.is_null() {
        let references = unsafe { take_references(payload) };
        unsafe {
            release_references(owner, references);
        }
        drop(unsafe { Box::from_raw(payload) });
    }
    let kind = unsafe { ffi::Py_TYPE(owner) };
    unsafe {
        ffi::PyObject_GC_Del(owner.cast());
    }
    unsafe {
        ffi::Py_DECREF(kind.cast());
    }
    if !unsafe { ffi::PyErr_Occurred() }.is_null() {
        unsafe {
            ffi::PyErr_WriteUnraisable(ptr::null_mut());
        }
    }
    unsafe {
        ffi::PyErr_SetRaisedException(error);
    }
}
