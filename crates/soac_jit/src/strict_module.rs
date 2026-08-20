//! Permanent policy for the actual dictionary of one authenticated module.
//!
//! The module state owns and traverses its dictionary and opaque native owner.
//! The dictionary also owns that owner through the native policy edge. The
//! owner payload contains only Rust data; pending callable/class weakrefs live
//! in its explicit GC edge vector, so no hidden Python cycle exists.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::ffi::{CStr, CString, c_int, c_uint, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use pyo3::exceptions::PyUnicodeEncodeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple};
use soac_contracts::{
    Fingerprint, GlobalMutability, ModuleTypeFacts, SourceDialect, SourceIdentity,
};
use soac_core::block_py::RuntimeFunctionId;

use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{StrictFunctionEntryKind, VerifiedStrictModule, strict_runtime_unavailable};

const VALIDATE_INITIAL: c_int = 0;
const SET: c_int = 1;
const DELETE: c_int = 2;
const CLEAR: c_int = 3;
const TERMINAL_TEARDOWN: c_int = 4;
const SET_EXISTING: c_int = 5;
const CACHE_INSERT: c_int = 6;
const CACHE_REPLACE: c_int = 7;

type PolicyCallback = unsafe extern "C" fn(
    *mut ffi::PyObject,
    *mut ffi::PyObject,
    *mut ffi::PyObject,
    *mut ffi::PyObject,
    c_int,
    *mut ffi::PyObject,
) -> c_int;

unsafe extern "C" {
    fn PyDict_SetSoacPolicy(
        dictionary: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
        validate: PolicyCallback,
        flags: c_uint,
    ) -> c_int;
    fn PyDict_MatchesSoacPolicy(
        dictionary: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
        validate: PolicyCallback,
        flags: c_uint,
    ) -> c_int;
    fn PyDict_SealSoacNamespace(dictionary: *mut ffi::PyObject) -> c_int;
    fn _PyDict_ReserveSoacNamespaceKeys(
        dictionary: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
        names: *mut ffi::PyObject,
    ) -> c_int;
    fn PySoac_GetStrictMutationError() -> *mut ffi::PyObject;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Phase {
    Discovered,
    Initializing,
    Sealing,
    Sealed,
    Failed,
    Terminal,
}

struct NamespacePolicy {
    interpreter_id: i64,
    dictionary_identity: usize,
    module_name: String,
    startup_identity: Fingerprint,
    source_digest: Fingerprint,
    mutable_names: BTreeSet<String>,
    reserved_names: BTreeSet<String>,
    execution_started: AtomicBool,
    phase: AtomicU8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StrictPendingKind {
    Function { function_id: RuntimeFunctionId },
    InterpreterFunction { native_code_ordinal: u32 },
    Class { source: SourceIdentity },
}

struct PendingRecord {
    kind: StrictPendingKind,
    edge: usize,
    // Native completion may drain only children of that actual invocation.
    // A shared source ordinal cannot distinguish nested/reentrant factories.
    interpreter_invocation: Option<Arc<crate::strict_interpreter::InterpreterInvocationIdentity>>,
}

#[derive(Clone, Copy)]
enum PendingPhase {
    Finalize,
    PublishCapabilities,
}

#[derive(Default)]
struct PendingRegistry {
    records: VecDeque<PendingRecord>,
    capabilities: VecDeque<PendingRecord>,
    free_edges: Vec<usize>,
}

impl PendingRegistry {
    fn records_mut(&mut self, phase: PendingPhase) -> &mut VecDeque<PendingRecord> {
        match phase {
            PendingPhase::Finalize => &mut self.records,
            PendingPhase::PublishCapabilities => &mut self.capabilities,
        }
    }
}

struct NamespaceOwnerData {
    policy: Arc<NamespacePolicy>,
    pending: RefCell<PendingRegistry>,
}

// SAFETY: The policy, pending kinds, and edge indices contain Rust data only.
// Every Python weakref is held in StrictStateRef's traversed edge vector.
unsafe impl StrictStateData for NamespaceOwnerData {
    const TYPE_NAME: &'static CStr = c"soac._StrictNamespaceOwner";

    fn on_terminal(&self) {
        self.policy.terminal();
    }
}

type NamespaceOwner<'py> = StrictStateRef<'py, NamespaceOwnerData>;

fn begin_namespace_sealing(
    py: Python<'_>,
    policy: &NamespacePolicy,
    actual_globals: *mut ffi::PyObject,
) -> PyResult<()> {
    if !policy.execution_started.load(Ordering::Acquire) {
        return Err(strict_runtime_unavailable(
            py,
            "strict module body has not started",
        ));
    }
    if !policy.transition(Phase::Initializing, Phase::Sealing) {
        return Err(strict_runtime_unavailable(
            py,
            "strict module is not initializing",
        ));
    }
    if unsafe { PyDict_SealSoacNamespace(actual_globals) } < 0 {
        policy.phase.store(Phase::Failed as u8, Ordering::Release);
        return Err(PyErr::fetch(py));
    }
    Ok(())
}

fn finish_namespace_sealing(py: Python<'_>, owner: &NamespaceOwner<'_>) -> PyResult<()> {
    if {
        let pending = owner.data().pending.borrow();
        !pending.records.is_empty() || !pending.capabilities.is_empty()
    } {
        return Err(strict_runtime_unavailable(
            py,
            "strict module still has pending callable/class finalization",
        ));
    }
    if !owner
        .data()
        .policy
        .transition(Phase::Sealing, Phase::Sealed)
    {
        return Err(strict_runtime_unavailable(
            py,
            "strict module is not sealing",
        ));
    }
    Ok(())
}

fn register_pending(
    owner: &NamespaceOwner<'_>,
    kind: StrictPendingKind,
    object: &Bound<'_, PyAny>,
) -> PyResult<()> {
    register_pending_at_phase(owner, kind, object, PendingPhase::Finalize, None, None)
}

fn register_pending_before(
    owner: &NamespaceOwner<'_>,
    kind: StrictPendingKind,
    object: &Bound<'_, PyAny>,
    before: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let py = owner.owner().py();
    owner.ensure_live()?;
    let candidates: Vec<_> = owner
        .data()
        .pending
        .borrow()
        .records
        .iter()
        .filter(|record| record.kind == kind)
        .map(|record| (record.edge, record.interpreter_invocation.clone()))
        .collect();
    for (edge, invocation) in candidates {
        let reference = owner.reference(edge)?;
        let mut target = ptr::null_mut();
        let status = unsafe { ffi::PyWeakref_GetRef(reference.as_ptr(), &mut target) };
        if status < 0 {
            return Err(PyErr::fetch(py));
        }
        if status == 0 {
            continue;
        }
        let target = unsafe { Bound::<PyAny>::from_owned_ptr(py, target) };
        if target.as_ptr() == before.as_ptr() {
            return register_pending_at_phase(
                owner,
                kind,
                object,
                PendingPhase::Finalize,
                Some(edge),
                invocation.as_ref(),
            );
        }
    }
    Err(strict_runtime_unavailable(
        py,
        "replacement class has no matching pending original",
    ))
}

fn register_pending_at_phase(
    owner: &NamespaceOwner<'_>,
    kind: StrictPendingKind,
    object: &Bound<'_, PyAny>,
    phase: PendingPhase,
    before_edge: Option<usize>,
    invocation: Option<&Arc<crate::strict_interpreter::InterpreterInvocationIdentity>>,
) -> PyResult<()> {
    let py = owner.owner().py();
    let reference = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            ffi::PyWeakref_NewRef(object.as_ptr(), ptr::null_mut()),
        )
    }?;
    let before_position = before_edge
        .map(|before| {
            owner
                .data()
                .pending
                .borrow_mut()
                .records_mut(phase)
                .iter()
                .position(|record| record.edge == before)
                .ok_or_else(|| strict_runtime_unavailable(py, "pending original changed"))
        })
        .transpose()?;
    // Weakref creation and insertion into this GC shell do not call Python.
    // Insertion never removes the original record, even if preparation fails.
    let reusable = owner.data().pending.borrow_mut().free_edges.pop();
    let edge = if let Some(edge) = reusable {
        owner.set_reference(edge, reference)?;
        edge
    } else {
        owner.add_reference(reference)?
    };
    let mut pending = owner.data().pending.borrow_mut();
    let records = pending.records_mut(phase);
    let record = PendingRecord {
        kind,
        edge,
        interpreter_invocation: invocation.cloned(),
    };
    if let Some(position) = before_position {
        records.insert(position, record);
    } else {
        records.push_back(record);
    }
    Ok(())
}

fn next_pending(owner: &NamespaceOwner<'_>) -> PyResult<Option<(StrictPendingKind, Py<PyAny>)>> {
    next_pending_at_phase(owner, PendingPhase::Finalize, None)
}

fn next_pending_at_phase(
    owner: &NamespaceOwner<'_>,
    phase: PendingPhase,
    invocation: Option<&Arc<crate::strict_interpreter::InterpreterInvocationIdentity>>,
) -> PyResult<Option<(StrictPendingKind, Py<PyAny>)>> {
    next_pending_matching(owner, phase, invocation, |_| true)
}

fn next_pending_matching(
    owner: &NamespaceOwner<'_>,
    phase: PendingPhase,
    invocation: Option<&Arc<crate::strict_interpreter::InterpreterInvocationIdentity>>,
    selected: impl Fn(&StrictPendingKind) -> bool,
) -> PyResult<Option<(StrictPendingKind, Py<PyAny>)>> {
    let py = owner.owner().py();
    owner.ensure_live()?;
    loop {
        let record = {
            let mut pending = owner.data().pending.borrow_mut();
            let records = pending.records_mut(phase);
            records
                .iter()
                .position(|record| {
                    selected(&record.kind)
                        && invocation.is_none_or(|invocation| {
                            record
                                .interpreter_invocation
                                .as_ref()
                                .is_some_and(|candidate| Arc::ptr_eq(candidate, invocation))
                        })
                })
                .and_then(|position| records.remove(position))
        };
        let Some(record) = record else {
            return Ok(None);
        };
        let reference = owner.reference(record.edge)?;
        let mut object = ptr::null_mut();
        let status = unsafe { ffi::PyWeakref_GetRef(reference.as_ptr(), &mut object) };
        if status < 0 {
            return Err(PyErr::fetch(py));
        }
        let object = if status != 0 {
            Some(unsafe { Bound::<PyAny>::from_owned_ptr(py, object) }.unbind())
        } else {
            None
        };
        // Drop this weak edge before returning exactly one target. Other
        // records remain weak while its finalization may run callbacks. No
        // mutable Rust borrow may cross set_reference's possible decrefs.
        owner.set_reference(record.edge, py.None().into_bound(py))?;
        owner
            .data()
            .pending
            .borrow_mut()
            .free_edges
            .push(record.edge);
        if let Some(object) = object {
            return Ok(Some((record.kind, object)));
        }
    }
}

fn remove_pending(
    owner: &NamespaceOwner<'_>,
    kind: &StrictPendingKind,
    object: &Bound<'_, PyAny>,
) -> PyResult<bool> {
    remove_pending_matching(owner, kind, None, |target| {
        Ok(target.as_ptr() == object.as_ptr())
    })
}

/// Remove only matching weak receipts, without first consuming nonmatches.
/// The successful object predicate must be allocation/Python-free. Each weak
/// target is supported separately; another graph never becomes a temporary
/// inventory of strong type edges across cleanup callbacks.
fn remove_pending_matching(
    owner: &NamespaceOwner<'_>,
    kind: &StrictPendingKind,
    invocation: Option<&Arc<crate::strict_interpreter::InterpreterInvocationIdentity>>,
    selected: impl Fn(&Bound<'_, PyAny>) -> PyResult<bool>,
) -> PyResult<bool> {
    let py = owner.owner().py();
    owner.ensure_live()?;
    let matches_record = |record: &PendingRecord| {
        &record.kind == kind
            && invocation.is_none_or(|expected| {
                record
                    .interpreter_invocation
                    .as_ref()
                    .is_some_and(|actual| Arc::ptr_eq(actual, expected))
            })
    };
    let candidates: Vec<_> = owner
        .data()
        .pending
        .borrow()
        .records
        .iter()
        .filter(|record| matches_record(record))
        .map(|record| record.edge)
        .collect();
    let mut removed = false;
    for edge in candidates {
        // Releasing a previous weak upgrade can run finalizers. An edge index
        // may then be recycled; recheck this exact scope before observing it.
        if !owner
            .data()
            .pending
            .borrow()
            .records
            .iter()
            .any(|record| record.edge == edge && matches_record(record))
        {
            continue;
        }
        let reference = owner.reference(edge)?;
        let mut target = ptr::null_mut();
        let status = unsafe { ffi::PyWeakref_GetRef(reference.as_ptr(), &mut target) };
        if status < 0 {
            return Err(PyErr::fetch(py));
        }
        if status == 0 {
            continue;
        }
        let target = unsafe { Bound::<PyAny>::from_owned_ptr(py, target) };
        if !selected(&target)? {
            continue;
        }
        // Even a future predicate must not let a replaced weak edge stand in
        // for the actual object just checked. No mutable borrow crosses it.
        if unsafe { owner.reference_ptr(edge)? }.as_ptr() != reference.as_ptr() {
            continue;
        }
        {
            let mut pending = owner.data().pending.borrow_mut();
            if let Some(position) = pending
                .records
                .iter()
                .position(|record| record.edge == edge && matches_record(record))
            {
                // Preserve declaration order among still-pending values.
                let _ = pending.records.remove(position);
            } else {
                continue;
            }
        }
        owner.set_reference(edge, py.None().into_bound(py))?;
        owner.data().pending.borrow_mut().free_edges.push(edge);
        removed = true;
    }
    Ok(removed)
}

impl NamespacePolicy {
    fn from_facts(
        interpreter_id: i64,
        dictionary_identity: usize,
        startup_identity: Fingerprint,
        facts: &ModuleTypeFacts,
    ) -> Result<Self, &'static str> {
        if facts.source_dialect != SourceDialect::SoacStrict {
            return Err("strict namespace requires authenticated strict source");
        }
        let mut mutable_names = BTreeSet::new();
        let mut reserved_names = BTreeSet::new();
        for binding in &facts.global_bindings {
            if !reserved_names.insert(binding.name.clone()) {
                return Err("duplicate authenticated global binding");
            }
            match binding.mutability {
                GlobalMutability::ExplicitlyMutable => {
                    mutable_names.insert(binding.name.clone());
                }
                GlobalMutability::FinalAfterSeal | GlobalMutability::LateAppendOnly => {}
                GlobalMutability::Unknown => {
                    return Err("strict source has an unresolved global mutation policy");
                }
            }
        }
        Ok(Self {
            interpreter_id,
            dictionary_identity,
            module_name: facts.module.module_name.clone(),
            startup_identity,
            source_digest: facts.source_digest,
            mutable_names,
            reserved_names,
            execution_started: AtomicBool::new(false),
            phase: AtomicU8::new(Phase::Discovered as u8),
        })
    }

    fn phase(&self) -> Phase {
        match self.phase.load(Ordering::Acquire) {
            value if value == Phase::Discovered as u8 => Phase::Discovered,
            value if value == Phase::Initializing as u8 => Phase::Initializing,
            value if value == Phase::Sealing as u8 => Phase::Sealing,
            value if value == Phase::Sealed as u8 => Phase::Sealed,
            value if value == Phase::Failed as u8 => Phase::Failed,
            _ => Phase::Terminal,
        }
    }

    fn transition(&self, from: Phase, to: Phase) -> bool {
        self.phase
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn terminal(&self) {
        self.phase.store(Phase::Terminal as u8, Ordering::Release);
    }

    fn start_execution(&self) -> bool {
        self.phase() == Phase::Initializing
            && self
                .execution_started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }

    fn matches_verified_source(&self, verified: &VerifiedStrictModule) -> bool {
        self.interpreter_id == verified.interpreter_id()
            && self.startup_identity == verified.startup_identity()
            && self.source_digest == verified.type_facts().facts().source_digest
            && self.module_name == verified.type_facts().facts().module.module_name
    }

    /// This is a read-only decision. Native bulk prevalidation may call it more
    /// than once, and a later allocation may still fail before a write commits.
    fn permits(&self, operation: c_int, name: Option<&str>) -> bool {
        if operation == TERMINAL_TEARDOWN {
            return true;
        }
        match self.phase() {
            Phase::Discovered => operation == VALIDATE_INITIAL,
            Phase::Initializing => matches!(operation, SET | SET_EXISTING | DELETE | CLEAR),
            Phase::Sealing | Phase::Sealed => match operation {
                SET => true,
                SET_EXISTING | DELETE => name.is_some_and(|name| self.mutable_names.contains(name)),
                _ => false,
            },
            Phase::Failed | Phase::Terminal => false,
        }
    }
}

fn strict_mutation_error(py: Python<'_>, message: impl AsRef<str>) -> PyErr {
    let exception = unsafe { PySoac_GetStrictMutationError() };
    if exception.is_null() {
        return PyErr::fetch(py);
    }
    let message = CString::new(message.as_ref().replace('\0', "\\0"))
        .expect("exception text has no embedded NUL");
    unsafe { ffi::PyErr_SetString(exception, message.as_ptr()) };
    PyErr::fetch(py)
}

fn current_interpreter_id(py: Python<'_>) -> PyResult<i64> {
    let identity = unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
    if identity < 0 {
        return Err(strict_runtime_unavailable(
            py,
            "cannot identify strict module interpreter",
        ));
    }
    Ok(identity)
}

unsafe extern "C" fn validate_namespace(
    owner: *mut ffi::PyObject,
    dictionary: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    _value: *mut ffi::PyObject,
    operation: c_int,
    provenance: *mut ffi::PyObject,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<()> {
        let owner = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, owner) };
        let owner = if operation == TERMINAL_TEARDOWN {
            NamespaceOwner::from_owner_for_teardown(owner)?
        } else {
            NamespaceOwner::from_owner(owner)?
        };
        let policy = &owner.data().policy;
        if operation == TERMINAL_TEARDOWN {
            // Native teardown cannot report an error. It precedes dictionary
            // clearing, and revokes execution, never grants public mutation.
            policy.terminal();
            return Ok(());
        }
        if dictionary as usize != policy.dictionary_identity
            || current_interpreter_id(py)? != policy.interpreter_id
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict namespace owner identity mismatch",
            ));
        }
        if !provenance.is_null() || matches!(operation, CACHE_INSERT | CACHE_REPLACE) {
            return Err(strict_mutation_error(
                py,
                "module globals have no private cache mutation permit",
            ));
        }
        if !matches!(
            policy.phase(),
            Phase::Discovered | Phase::Initializing | Phase::Sealing | Phase::Sealed
        ) {
            return Err(strict_runtime_unavailable(
                py,
                "strict module is failed or terminal",
            ));
        }
        if operation == CLEAR {
            if policy.permits(operation, None) {
                return Ok(());
            }
        } else if !key.is_null() && unsafe { ffi::PyUnicode_CheckExact(key) } != 0 {
            // Exact str extraction never calls __str__, __hash__, or equality.
            // Surrogate-containing mapping keys cannot name a source global,
            // but remain valid final append-only strings under this policy.
            let key = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, key) };
            let key = key.cast_into::<PyString>()?;
            let name = match key.to_str() {
                Ok(name) => Some(name),
                Err(error) if error.is_instance_of::<PyUnicodeEncodeError>(py) => None,
                Err(error) => return Err(error),
            };
            if policy.permits(operation, name) {
                return Ok(());
            }
        }
        Err(strict_mutation_error(
            py,
            format!(
                "cannot mutate a final binding in strict module {}",
                policy.module_name
            ),
        ))
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            error.restore(py);
            -1
        }
        Err(_) => {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_SystemError,
                    c"panic in strict module policy".as_ptr(),
                )
            };
            -1
        }
    }
}

/// The unique native module-state handle. Do not put it in a global registry or
/// an Arc: the owning module must traverse its Python edges and drop this handle
/// before clearing them. Its opaque owner's shared payload contains Rust only.
pub struct StrictModuleRuntimeState {
    policy: Arc<NamespacePolicy>,
    owner: Py<PyAny>,
    // The SOAC backend keeps its existing owning model. Native interpreter
    // installation hands this edge back to the real module's md_dict once;
    // no borrowed dictionary address is stored in its place.
    globals: Option<Py<PyDict>>,
    initializer_entry: OnceLock<StrictFunctionEntryKind>,
}

/// An execution lookup, not an owning Python edge or a freely constructible
/// capability. A module's explicit native state is its only producer. Keeping
/// this in shared Rust metadata cannot conceal a Python reference cycle.
#[derive(Clone)]
pub(crate) struct StrictModuleExecutionRef {
    policy: Arc<NamespacePolicy>,
    owner_identity: usize,
}

impl StrictModuleExecutionRef {
    /// Compare two already established execution identities without recovering
    /// a Python owner. This comparison alone does not establish live authority.
    pub(crate) fn same_execution(&self, other: &Self) -> bool {
        self.owner_identity == other.owner_identity && Arc::ptr_eq(&self.policy, &other.policy)
    }

    /// Validate an already-owned policy edge without recovering or touching a
    /// dictionary address. Native registration permanently bound that owner;
    /// dictionary teardown notifies its Rust state before releasing the edge.
    pub(crate) fn validate_owner(
        &self,
        py: Python<'_>,
        owned_policy: &Bound<'_, PyAny>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<()> {
        if current_interpreter_id(py)? != self.policy.interpreter_id
            || !self.policy.matches_verified_source(verified)
            || !matches!(
                self.policy.phase(),
                Phase::Initializing | Phase::Sealing | Phase::Sealed
            )
            || owned_policy.as_ptr() as usize != self.owner_identity
        {
            return Err(strict_runtime_unavailable(
                py,
                "owned strict module policy does not match this live execution",
            ));
        }
        let owner = NamespaceOwner::from_owner(owned_policy.clone())?;
        if !Arc::ptr_eq(&owner.data().policy, &self.policy) {
            return Err(strict_runtime_unavailable(
                py,
                "owned strict namespace payload identity changed",
            ));
        }
        Ok(())
    }

    /// Acquire the actual dictionary-owned policy for GC-owned callable/class
    /// state. The returned reference must be visited by that state's GC
    /// traversal. No module wrapper is retained: ordinary CPython functions
    /// keep their globals usable after the wrapper and its weakrefs are gone.
    pub(crate) fn acquire_owner(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<Py<PyAny>> {
        if !self.policy.matches_verified_source(verified) {
            return Err(strict_runtime_unavailable(
                py,
                "strict execution reference belongs to different authenticated source",
            ));
        }
        self.acquire_live_owner(py, actual_globals)
    }

    fn acquire_live_owner(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
    ) -> PyResult<Py<PyAny>> {
        if current_interpreter_id(py)? != self.policy.interpreter_id
            || !matches!(
                self.policy.phase(),
                Phase::Initializing | Phase::Sealing | Phase::Sealed
            )
            || actual_globals.as_ptr() as usize != self.policy.dictionary_identity
            || unsafe { ffi::PyDict_CheckExact(actual_globals.as_ptr()) } == 0
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict execution reference has no matching live module namespace",
            ));
        }
        let owner = self.owner_identity as *mut ffi::PyObject;
        // This predicate compares the expected owner address without touching
        // it. A successful match proves the actual dictionary still retains
        // that exact owner. Do not dereference the borrowed identity first.
        if unsafe {
            PyDict_MatchesSoacPolicy(actual_globals.as_ptr(), owner, validate_namespace, 0)
        } != 1
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict execution reference lost its native namespace policy",
            ));
        }
        let owner = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, owner) };
        let owner = NamespaceOwner::from_owner(owner)?;
        if !Arc::ptr_eq(&owner.data().policy, &self.policy) {
            return Err(strict_runtime_unavailable(
                py,
                "strict namespace payload identity changed",
            ));
        }
        Ok(owner.owner().clone().unbind())
    }

    pub(crate) fn register_pending(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
        kind: StrictPendingKind,
        object: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let owner = self.acquire_owner(py, actual_globals, verified)?;
        let owner = NamespaceOwner::from_owner(owner.into_bound(py))?;
        register_pending(&owner, kind, object)
    }

    /// Queue an actual interpreter birth/construction using its already-owned
    /// policy edge. This works before type callbacks without retaining a
    /// namespace or recovering a remembered dictionary address.
    pub(crate) fn register_interpreter_pending(
        &self,
        py: Python<'_>,
        owned_policy: &Bound<'_, PyAny>,
        verified: &VerifiedStrictModule,
        kind: StrictPendingKind,
        object: &Bound<'_, PyAny>,
        invocation: &Arc<crate::strict_interpreter::InterpreterInvocationIdentity>,
    ) -> PyResult<()> {
        self.validate_owner(py, owned_policy, verified)?;
        if matches!(kind, StrictPendingKind::Function { .. }) {
            return Err(strict_runtime_unavailable(
                py,
                "native pending definition cannot carry a compiler function identity",
            ));
        }
        let owner = NamespaceOwner::from_owner(owned_policy.clone())?;
        register_pending_at_phase(
            &owner,
            kind,
            object,
            PendingPhase::Finalize,
            None,
            Some(invocation),
        )
    }

    /// Finalize only this completed native invocation's weak children. Another
    /// active call, even of the same source function, retains its own records.
    pub(crate) fn next_interpreter_pending(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
        invocation: &Arc<crate::strict_interpreter::InterpreterInvocationIdentity>,
    ) -> PyResult<Option<(StrictPendingKind, Py<PyAny>)>> {
        let owner = self.acquire_owner(py, actual_globals, verified)?;
        let owner = NamespaceOwner::from_owner(owner.into_bound(py))?;
        next_pending_at_phase(&owner, PendingPhase::Finalize, Some(invocation))
    }

    /// Pending types cannot wait for module sealing: once a definition's
    /// invocation finishes, every remaining provisional must be disposed by
    /// its successfully resolved native lineage or fail closed. Leave free
    /// functions in this SAME weak inventory for final global bindings.
    pub(crate) fn next_interpreter_pending_class(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
        invocation: &Arc<crate::strict_interpreter::InterpreterInvocationIdentity>,
        source: Option<&SourceIdentity>,
    ) -> PyResult<Option<(StrictPendingKind, Py<PyAny>)>> {
        let owner = self.acquire_owner(py, actual_globals, verified)?;
        let owner = NamespaceOwner::from_owner(owner.into_bound(py))?;
        next_pending_matching(&owner, PendingPhase::Finalize, Some(invocation), |kind| {
            matches!(kind, StrictPendingKind::Class { source: actual }
                if source.is_none_or(|expected| expected == actual))
        })
    }

    /// Failed Apply cleanup may remove only this declaration and actual caller
    /// invocation, plus a callback-free proof of the same failed dataclass
    /// graph. This does not drain other classes or authorize final admission.
    pub(crate) fn remove_interpreter_pending_class_matching(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
        invocation: &Arc<crate::strict_interpreter::InterpreterInvocationIdentity>,
        source: &SourceIdentity,
        selected: impl Fn(&Bound<'_, PyAny>) -> PyResult<bool>,
    ) -> PyResult<bool> {
        let owner = self.acquire_owner(py, actual_globals, verified)?;
        let owner = NamespaceOwner::from_owner(owner.into_bound(py))?;
        remove_pending_matching(
            &owner,
            &StrictPendingKind::Class {
                source: source.clone(),
            },
            Some(invocation),
            selected,
        )
    }

    /// Consume the original module's one initialization attempt using the
    /// dictionary actually supported by the loader. The execution identity
    /// never recovers a dictionary pointer from stored scalar metadata.
    pub(crate) fn begin_execution(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<()> {
        self.acquire_owner(py, actual_globals, verified)?;
        if !self.policy.start_execution() {
            return Err(strict_runtime_unavailable(
                py,
                "strict module body execution is single-use",
            ));
        }
        Ok(())
    }

    /// The interpreter loader holds the actual module/dictionary, not a Rust
    /// borrow into native module state, across sealing and callable adoption.
    pub(crate) fn begin_interpreter_sealing(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<()> {
        self.acquire_owner(py, actual_globals, verified)?;
        begin_namespace_sealing(py, &self.policy, actual_globals.as_ptr())
    }

    pub(crate) fn finish_interpreter_sealing(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<()> {
        let owner = self.acquire_owner(py, actual_globals, verified)?;
        let owner = NamespaceOwner::from_owner(owner.into_bound(py))?;
        finish_namespace_sealing(py, &owner)
    }

    /// A completed replacement shares declaring provenance, not its original
    /// class's layout. Drain it first so shared source functions can acquire
    /// absent optional witnesses for the returned class. The original remains
    /// queued and protected if replacement weakref allocation fails.
    pub(crate) fn register_pending_before(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
        kind: StrictPendingKind,
        object: &Bound<'_, PyAny>,
        before: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let owner = self.acquire_owner(py, actual_globals, verified)?;
        let owner = NamespaceOwner::from_owner(owner.into_bound(py))?;
        register_pending_before(&owner, kind, object, before)
    }

    pub(crate) fn is_sealed(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<bool> {
        self.acquire_owner(py, actual_globals, verified)?;
        Ok(self.policy.phase() == Phase::Sealed)
    }

    pub(crate) fn bindings_are_final(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<bool> {
        self.acquire_owner(py, actual_globals, verified)?;
        Ok(matches!(
            self.policy.phase(),
            Phase::Sealing | Phase::Sealed
        ))
    }

    pub(crate) fn remove_pending(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
        kind: &StrictPendingKind,
        object: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        let owner = self.acquire_owner(py, actual_globals, verified)?;
        let owner = NamespaceOwner::from_owner(owner.into_bound(py))?;
        remove_pending(&owner, kind, object)
    }

    pub(crate) fn next_pending(
        &self,
        py: Python<'_>,
        actual_globals: &Bound<'_, PyDict>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<Option<(StrictPendingKind, Py<PyAny>)>> {
        let owner = self.acquire_owner(py, actual_globals, verified)?;
        let owner = NamespaceOwner::from_owner(owner.into_bound(py))?;
        next_pending(&owner)
    }
}

impl StrictModuleRuntimeState {
    /// Install before body execution or callbacks can expose the actual module.
    /// Source verification is necessary but does not itself publish a sealed
    /// capability; callers must complete the explicit sealing transition.
    pub fn install(
        py: Python<'_>,
        module: &Bound<'_, PyAny>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<Self> {
        let interpreter_id = current_interpreter_id(py)?;
        if interpreter_id != verified.interpreter_id() {
            return Err(strict_runtime_unavailable(
                py,
                "verified source belongs to another interpreter",
            ));
        }
        if unsafe { ffi::Py_TYPE(module.as_ptr()) } != ptr::addr_of_mut!(ffi::PyModule_Type) {
            return Err(strict_runtime_unavailable(
                py,
                "strict module requires the exact native ModuleType",
            ));
        }
        let dictionary = unsafe { ffi::PyModule_GetDict(module.as_ptr()) };
        if dictionary.is_null() {
            return Err(PyErr::fetch(py));
        }
        let globals =
            unsafe { Bound::<PyAny>::from_borrowed_ptr(py, dictionary) }.cast_into::<PyDict>()?;
        let facts = verified.type_facts().facts();
        let name = unsafe { ffi::PyModule_GetNameObject(module.as_ptr()) };
        let name = unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, name) }?;
        let name: String = name.extract()?;
        if name != facts.module.module_name {
            return Err(strict_runtime_unavailable(
                py,
                "actual module name differs from verified source",
            ));
        }
        let policy = NamespacePolicy::from_facts(
            interpreter_id,
            dictionary as usize,
            verified.startup_identity(),
            facts,
        )
        .map_err(|message| strict_runtime_unavailable(py, message))?;
        Self::install_policy(py, globals, policy)
    }

    fn install_policy(
        py: Python<'_>,
        globals: Bound<'_, PyDict>,
        policy: NamespacePolicy,
    ) -> PyResult<Self> {
        let policy = Arc::new(policy);
        let owner = NamespaceOwner::new(
            py,
            NamespaceOwnerData {
                policy: policy.clone(),
                pending: RefCell::default(),
            },
            Vec::new(),
        )?;
        // Construct the terminalizing handle before installing any native edge.
        // A registration/reservation failure can never leave an escaped module
        // with an initializing mutation policy.
        let state = Self {
            policy,
            owner: owner.owner().clone().unbind(),
            globals: Some(globals.unbind()),
            initializer_entry: OnceLock::new(),
        };
        if unsafe {
            PyDict_SetSoacPolicy(
                state.owned_globals(py)?.as_ptr(),
                state.owner.as_ptr(),
                validate_namespace,
                0,
            )
        } < 0
        {
            return Err(PyErr::fetch(py));
        }
        let names = PyTuple::new(py, state.policy.reserved_names.iter())?;
        if unsafe {
            _PyDict_ReserveSoacNamespaceKeys(
                state.owned_globals(py)?.as_ptr(),
                state.owner.as_ptr(),
                names.as_ptr(),
            )
        } < 0
        {
            return Err(PyErr::fetch(py));
        }
        if !state
            .policy
            .transition(Phase::Discovered, Phase::Initializing)
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict module became terminal during installation",
            ));
        }
        Ok(state)
    }

    /// Finish interpreter-backend installation without a second permanent
    /// globals owner. The ordinary module's actual md_dict remains the primary;
    /// this same guard still terminalizes unfinished policy on every error or
    /// native clear. All native runtime checks must supply actual live globals.
    ///
    /// This consumes the handle so a failed identity check cannot leave an
    /// initializing policy behind. No callback occurs on the successful path:
    /// the actual exact module supports its dictionary through the last DECREF.
    pub(crate) fn handoff_globals_to_module(
        mut self,
        py: Python<'_>,
        actual_module: &Bound<'_, PyAny>,
        verified: &VerifiedStrictModule,
    ) -> PyResult<Self> {
        self.check_owner(py)?;
        if !self.matches_verified_source(verified)
            || unsafe { ffi::Py_TYPE(actual_module.as_ptr()) }
                != ptr::addr_of_mut!(ffi::PyModule_Type)
            || self.policy.phase() != Phase::Initializing
            || self.policy.execution_started.load(Ordering::Acquire)
        {
            return Err(strict_runtime_unavailable(
                py,
                "globals handoff requires the original unentered module installation",
            ));
        }
        let globals = unsafe {
            Borrowed::<PyAny>::from_ptr_or_err(py, ffi::PyModule_GetDict(actual_module.as_ptr()))?
        }
        .cast::<PyDict>()?;
        if globals.as_ptr() != self.owned_globals(py)?.as_ptr() {
            return Err(strict_runtime_unavailable(
                py,
                "globals handoff does not match the actual module dictionary",
            ));
        }
        // Publish the absent edge before its close. The source of the Borrowed
        // view is md_dict, never policy.dictionary_identity or a saved pointer.
        let previous = self.globals.take().ok_or_else(|| {
            strict_runtime_unavailable(py, "module globals were already handed off")
        })?;
        previous.drop_ref(py);
        Ok(self)
    }

    pub(crate) fn execution_ref(&self) -> StrictModuleExecutionRef {
        StrictModuleExecutionRef {
            policy: self.policy.clone(),
            owner_identity: self.owner.as_ptr() as usize,
        }
    }

    pub(crate) fn next_pending(
        &self,
        py: Python<'_>,
    ) -> PyResult<Option<(StrictPendingKind, Py<PyAny>)>> {
        self.check_owner(py)?;
        let owner = NamespaceOwner::from_owner(self.owner.clone_ref(py).into_bound(py))?;
        next_pending(&owner)
    }

    /// A second cold pass lets source-order-independent nominal operands
    /// acquire optional witnesses only after every pending class is sealed.
    /// These records use the same GC-visible callback-free weak edges as the
    /// initial drain; they never keep a function, class, or captured value alive.
    pub(crate) fn defer_capability_publication(
        &self,
        py: Python<'_>,
        kind: StrictPendingKind,
        object: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.check_owner(py)?;
        let owner = NamespaceOwner::from_owner(self.owner.clone_ref(py).into_bound(py))?;
        register_pending_at_phase(
            &owner,
            kind,
            object,
            PendingPhase::PublishCapabilities,
            None,
            None,
        )
    }

    pub(crate) fn next_capability_publication(
        &self,
        py: Python<'_>,
    ) -> PyResult<Option<(StrictPendingKind, Py<PyAny>)>> {
        self.check_owner(py)?;
        let owner = NamespaceOwner::from_owner(self.owner.clone_ref(py).into_bound(py))?;
        next_pending_at_phase(&owner, PendingPhase::PublishCapabilities, None)
    }

    fn owned_globals(&self, py: Python<'_>) -> PyResult<&Py<PyDict>> {
        self.globals.as_ref().ok_or_else(|| {
            strict_runtime_unavailable(
                py,
                "native module checks require caller-supported actual globals",
            )
        })
    }

    fn check_owner(&self, py: Python<'_>) -> PyResult<()> {
        let globals = self.owned_globals(py)?;
        if current_interpreter_id(py)? != self.policy.interpreter_id {
            return Err(strict_runtime_unavailable(
                py,
                "strict module belongs to another interpreter",
            ));
        }
        if unsafe {
            PyDict_MatchesSoacPolicy(globals.as_ptr(), self.owner.as_ptr(), validate_namespace, 0)
        } != 1
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict globals are not owned by the expected live policy",
            ));
        }
        if matches!(self.policy.phase(), Phase::Failed | Phase::Terminal) {
            return Err(strict_runtime_unavailable(
                py,
                "strict module is failed or terminal",
            ));
        }
        Ok(())
    }

    /// Consume the one permitted body execution, including on failure. Reload
    /// and reentrant exec_module cannot reopen an initializing module.
    pub fn begin_execution(&self, py: Python<'_>) -> PyResult<()> {
        self.check_owner(py)?;
        if !self.policy.start_execution() {
            return Err(strict_runtime_unavailable(
                py,
                "strict module body execution is single-use",
            ));
        }
        Ok(())
    }

    /// Record the actual public entry immediately before the initializer call.
    /// This immutable observation owns no Python references and cannot select
    /// an execution mode or authorize a later call. The success path performs
    /// no allocation or Python callback between observation and invocation.
    pub fn record_initializer_entry(
        &self,
        py: Python<'_>,
        kind: StrictFunctionEntryKind,
    ) -> PyResult<()> {
        self.check_owner(py)?;
        if self.policy.phase() != Phase::Initializing
            || !self.policy.execution_started.load(Ordering::Acquire)
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict module initializer is not about to execute",
            ));
        }
        self.initializer_entry.set(kind).map_err(|_| {
            strict_runtime_unavailable(py, "strict module initializer entry was already observed")
        })
    }

    /// The first initializer entry observation, or None before its actual call.
    pub fn initializer_entry_kind(
        &self,
        py: Python<'_>,
    ) -> PyResult<Option<StrictFunctionEntryKind>> {
        self.check_owner(py)?;
        Ok(self.initializer_entry.get().copied())
    }

    /// Activate final binding barriers before any class/function finalizer.
    pub fn begin_sealing(&self, py: Python<'_>) -> PyResult<()> {
        self.check_owner(py)?;
        begin_namespace_sealing(py, &self.policy, self.owned_globals(py)?.as_ptr())
    }

    /// Publish only after every required callable/class barrier has succeeded.
    pub fn finish_sealing(&self, py: Python<'_>) -> PyResult<()> {
        self.check_owner(py)?;
        let owner = NamespaceOwner::from_owner(self.owner.clone_ref(py).into_bound(py))?;
        finish_namespace_sealing(py, &owner)
    }

    /// For a module with no remaining class/function finalization work.
    pub fn seal(&self, py: Python<'_>) -> PyResult<()> {
        self.begin_sealing(py)?;
        self.finish_sealing(py)
    }

    /// An initialization/sealing failure is terminal, never an unseal request.
    pub fn fail(&self, py: Python<'_>) -> PyResult<()> {
        self.check_owner(py)?;
        match self.policy.phase() {
            Phase::Discovered | Phase::Initializing | Phase::Sealing => {
                self.policy
                    .phase
                    .store(Phase::Failed as u8, Ordering::Release);
                Ok(())
            }
            _ => Err(strict_runtime_unavailable(
                py,
                "cannot fail or reopen a published strict module",
            )),
        }
    }

    /// Failure/clear notification for the actual native module-state guard.
    /// This never authenticates an operation or recovers globals; it only
    /// terminalizes unfinished installation. It cannot alter a published or
    /// already-terminal policy and cannot replace the pending Python error.
    pub(crate) fn fail_unfinished(&self) {
        let _ = self
            .policy
            .phase
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |phase| {
                (phase == Phase::Discovered as u8
                    || phase == Phase::Initializing as u8
                    || phase == Phase::Sealing as u8)
                    .then_some(Phase::Failed as u8)
            });
    }

    /// False during initialization/sealing is a dynamic execution state, not
    /// permission to treat a failed or terminal strict module as ordinary.
    pub fn is_sealed(&self, py: Python<'_>) -> PyResult<bool> {
        self.check_owner(py)?;
        Ok(self.policy.phase() == Phase::Sealed)
    }

    pub fn matches_globals(&self, py: Python<'_>, globals: &Bound<'_, PyDict>) -> PyResult<bool> {
        self.check_owner(py)?;
        Ok(globals.as_ptr() == self.owned_globals(py)?.as_ptr())
    }

    pub fn matches_verified_source(&self, verified: &VerifiedStrictModule) -> bool {
        self.policy.matches_verified_source(verified)
    }

    /// Release the module state's owned references at its real native GC or
    /// deallocation boundary, without queuing them for a later PyO3 entry.
    /// Escaped sealed dictionaries retain their own policy independently.
    ///
    /// # Safety
    /// The caller owns the GIL in this state's interpreter and has consumed
    /// the corresponding module-state slot before any decref can re-enter.
    pub unsafe fn release_from_native(self, py: Python<'_>) {
        let state = std::mem::ManuallyDrop::new(self);
        // Move each field exactly once while suppressing the ordinary Drop,
        // which must remain safe for Rust callers outside an attachment.
        let (policy, owner, globals, _initializer_entry) = unsafe {
            (
                ptr::read(&state.policy),
                ptr::read(&state.owner),
                ptr::read(&state.globals),
                ptr::read(&state.initializer_entry),
            )
        };
        if policy.phase() != Phase::Sealed {
            policy.terminal();
        }
        owner.drop_ref(py);
        if let Some(globals) = globals {
            globals.drop_ref(py);
        }
        drop(policy);
    }

    /// Visit only references owned by this state. The interpreter-backend
    /// handoff leaves the real module as the sole md_dict owner; the SOAC
    /// backend still visits its additional dictionary edge. The policy owner
    /// separately visits pending weakrefs.
    ///
    /// # Safety
    /// `visit` and `argument` are the active CPython GC traversal callback.
    pub unsafe fn traverse(&self, visit: ffi::visitproc, argument: *mut c_void) -> c_int {
        if let Some(globals) = &self.globals {
            let result = unsafe { visit(globals.as_ptr(), argument) };
            if result != 0 {
                return result;
            }
        }
        unsafe { visit(self.owner.as_ptr(), argument) }
    }
}

impl Drop for StrictModuleRuntimeState {
    fn drop(&mut self) {
        // A failed/incomplete installation cannot escape with mutable policy.
        // A published policy belongs to the actual dictionary, however, not
        // the module wrapper: dropping the latter must preserve the ordinary
        // lifetime and usability of escaped functions/globals. Native dict
        // teardown or explicit interpreter shutdown terminalizes that policy.
        if self.policy.phase() != Phase::Sealed {
            self.policy.terminal();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyModule;
    use soac_contracts::{GlobalBindingFact, ResolvedStrictPolicy, StaticType};

    unsafe extern "C" {
        fn _PyDict_IndexedKeyIndex(
            dictionary: *mut ffi::PyObject,
            key: *mut ffi::PyObject,
        ) -> ffi::Py_ssize_t;
    }

    fn facts() -> ModuleTypeFacts {
        let mut facts = ModuleTypeFacts::new(
            "strict_policy_fixture",
            b"from __future__ import strict\n",
            SourceDialect::SoacStrict,
            ResolvedStrictPolicy::default(),
        )
        .unwrap();
        for (name, mutability) in [
            ("final_value", GlobalMutability::FinalAfterSeal),
            ("mutable_value", GlobalMutability::ExplicitlyMutable),
            ("declared_but_absent", GlobalMutability::ExplicitlyMutable),
            ("__spec__", GlobalMutability::FinalAfterSeal),
        ] {
            facts.global_bindings.push(GlobalBindingFact {
                name: name.into(),
                mutability,
                value_type: StaticType::Unknown,
                definition: None,
                uncertainty: BTreeSet::new(),
            });
        }
        facts
    }

    fn policy() -> NamespacePolicy {
        NamespacePolicy::from_facts(0, 0, Fingerprint::digest(b"fixture"), &facts()).unwrap()
    }

    // The artifact loader tests authenticate source facts. These fixtures
    // exercise the real installation/callback/lifecycle kernel without
    // introducing a public constructor that could skip that authentication.
    fn installed<'py>(
        py: Python<'py>,
        facts: &ModuleTypeFacts,
    ) -> (Bound<'py, PyModule>, StrictModuleRuntimeState) {
        let module = PyModule::new(py, &facts.module.module_name).unwrap();
        let globals = module.dict();
        let policy = NamespacePolicy::from_facts(
            current_interpreter_id(py).unwrap(),
            globals.as_ptr() as usize,
            Fingerprint::digest(b"native namespace fixture"),
            facts,
        )
        .unwrap();
        let state = StrictModuleRuntimeState::install_policy(py, globals, policy).unwrap();
        (module, state)
    }

    #[test]
    fn initialization_is_mutable_but_sealing_is_already_append_once() {
        let policy = policy();
        assert!(policy.permits(VALIDATE_INITIAL, Some("__name__")));
        assert!(!policy.permits(SET, Some("early_escape")));
        assert!(policy.transition(Phase::Discovered, Phase::Initializing));
        for operation in [SET, SET_EXISTING, DELETE, CLEAR] {
            assert!(policy.permits(operation, Some("final_value")));
        }
        assert!(policy.transition(Phase::Initializing, Phase::Sealing));
        assert!(policy.permits(SET, Some("late")));
        assert!(!policy.permits(SET_EXISTING, Some("late")));
        assert!(!policy.permits(DELETE, Some("final_value")));
        assert!(!policy.permits(CLEAR, None));
        assert!(policy.transition(Phase::Sealing, Phase::Sealed));
        assert!(!policy.transition(Phase::Initializing, Phase::Sealing));
    }

    #[test]
    fn lexical_mutability_survives_deletion_without_changing_final_policy() {
        let policy = policy();
        policy.phase.store(Phase::Sealed as u8, Ordering::Release);
        for name in ["mutable_value", "declared_but_absent"] {
            assert!(policy.reserved_names.contains(name));
            for operation in [SET, SET_EXISTING, DELETE, SET] {
                assert!(policy.permits(operation, Some(name)));
            }
        }
        assert!(!policy.permits(SET_EXISTING, Some("__spec__")));
        assert!(policy.permits(SET, Some("__annotations__")));
        assert!(!policy.permits(SET_EXISTING, Some("__annotations__")));
        assert!(!policy.permits(CACHE_INSERT, Some("__annotations__")));
        assert!(!policy.permits(CACHE_REPLACE, Some("__annotations__")));
        assert!(!policy.permits(VALIDATE_INITIAL, Some("final_value")));
    }

    #[test]
    fn unknown_policy_fails_closed_and_terminal_never_restores_initialization() {
        let mut facts = facts();
        facts.global_bindings[0].mutability = GlobalMutability::Unknown;
        assert!(
            NamespacePolicy::from_facts(0, 0, Fingerprint::digest(b"fixture"), &facts).is_err()
        );
        let policy = policy();
        policy.phase.store(Phase::Failed as u8, Ordering::Release);
        assert!(!policy.permits(SET, Some("mutable_value")));
        policy.terminal();
        assert!(!policy.transition(Phase::Initializing, Phase::Sealing));
        assert!(!policy.permits(SET_EXISTING, Some("mutable_value")));
        assert!(policy.permits(TERMINAL_TEARDOWN, None));
    }

    #[test]
    fn module_execution_is_single_use_even_when_reentrant_or_failed() {
        let policy = policy();
        assert!(!policy.start_execution());
        assert!(policy.transition(Phase::Discovered, Phase::Initializing));
        assert!(policy.start_execution());
        assert!(!policy.start_execution());
        policy.phase.store(Phase::Failed as u8, Ordering::Release);
        assert!(!policy.start_execution());
        assert!(policy.execution_started.load(Ordering::Acquire));
    }

    #[test]
    fn installed_namespace_uses_native_barriers_before_finalization_finishes() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (module, state) = installed(py, &facts());
            let globals = module.dict();
            globals.set_item("final_value", 1).unwrap();
            globals.set_item("final_value", 2).unwrap();
            assert_eq!(state.initializer_entry_kind(py).unwrap(), None);
            assert!(
                state
                    .record_initializer_entry(py, StrictFunctionEntryKind::CheckedNative)
                    .is_err()
            );
            state.begin_execution(py).unwrap();
            assert!(state.begin_execution(py).is_err());
            state
                .record_initializer_entry(py, StrictFunctionEntryKind::EntryInterpreter)
                .unwrap();
            assert!(
                state
                    .record_initializer_entry(py, StrictFunctionEntryKind::CheckedNative)
                    .is_err()
            );
            state.begin_sealing(py).unwrap();
            assert!(!state.is_sealed(py).unwrap());
            let error = globals.set_item("final_value", 3).unwrap_err();
            assert_eq!(error.get_type(py).as_ptr(), unsafe {
                PySoac_GetStrictMutationError()
            });
            assert_eq!(
                globals
                    .get_item("final_value")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                2
            );
            globals.set_item("late", 1).unwrap();
            assert!(globals.set_item("late", 2).is_err());
            assert!(globals.del_item("late").is_err());
            state.finish_sealing(py).unwrap();
            assert!(state.is_sealed(py).unwrap());
            assert_eq!(
                state.initializer_entry_kind(py).unwrap(),
                Some(StrictFunctionEntryKind::EntryInterpreter)
            );
            assert!(
                state
                    .record_initializer_entry(py, StrictFunctionEntryKind::CheckedNative)
                    .is_err()
            );
            assert!(state.matches_globals(py, &globals).unwrap());
            assert!(!state.matches_globals(py, &PyDict::new(py)).unwrap());

            let surrogate = unsafe { ffi::PyUnicode_FromOrdinal(0xd800) };
            let surrogate =
                unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, surrogate) }.unwrap();
            globals.set_item(&surrogate, 1).unwrap();
            let error = globals.set_item(&surrogate, 2).unwrap_err();
            assert_eq!(error.get_type(py).as_ptr(), unsafe {
                PySoac_GetStrictMutationError()
            });
        });
    }

    #[test]
    fn mutable_global_indexes_survive_unbound_tombstones_and_late_growth() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut facts = facts();
            facts
                .global_bindings
                .iter_mut()
                .find(|binding| binding.name == "__spec__")
                .unwrap()
                .mutability = GlobalMutability::ExplicitlyMutable;
            let (module, state) = installed(py, &facts);
            let globals = module.dict();
            let key = PyString::new(py, "declared_but_absent");
            let index = unsafe { _PyDict_IndexedKeyIndex(globals.as_ptr(), key.as_ptr()) };
            assert!(index >= 0);
            assert!(!globals.contains(&key).unwrap());
            state.begin_execution(py).unwrap();
            state.seal(py).unwrap();
            for number in 0..100 {
                globals.set_item(format!("late_{number}"), number).unwrap();
            }
            globals.set_item(&key, 1).unwrap();
            globals.del_item(&key).unwrap();
            assert!(!globals.contains(&key).unwrap());
            assert_eq!(
                unsafe { _PyDict_IndexedKeyIndex(globals.as_ptr(), key.as_ptr()) },
                index
            );
            globals.set_item("after_delete", 2).unwrap();
            globals.set_item(&key, 3).unwrap();
            assert_eq!(
                unsafe { _PyDict_IndexedKeyIndex(globals.as_ptr(), key.as_ptr()) },
                index
            );
            assert_eq!(
                globals
                    .keys()
                    .get_item(globals.len() - 1)
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "declared_but_absent"
            );
            globals
                .set_item("__spec__", "explicitly mutable metadata")
                .unwrap();
        });
    }

    #[test]
    fn failure_and_owner_drop_leave_escaped_namespaces_permanently_terminal() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (module, state) = installed(py, &facts());
            let globals = module.dict();
            state.begin_execution(py).unwrap();
            globals.set_item("mutable_value", 1).unwrap();
            state.fail(py).unwrap();
            assert!(state.is_sealed(py).is_err());
            assert!(state.begin_execution(py).is_err());
            let unavailable = strict_runtime_unavailable(py, "expected").get_type(py);
            let error = globals.set_item("mutable_value", 2).unwrap_err();
            assert!(error.get_type(py).is(&unavailable));
            drop(state);
            let error = globals.set_item("late", 1).unwrap_err();
            assert!(error.get_type(py).is(&unavailable));
            assert!(!globals.contains("late").unwrap());
            assert_eq!(
                globals
                    .get_item("mutable_value")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                1
            );
        });
    }

    #[test]
    fn execution_reference_acquires_exact_gc_edges_and_rejects_unmatched_identities() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (module, state) = installed(py, &facts());
            let globals = module.dict();
            state.begin_execution(py).unwrap();
            state.seal(py).unwrap();
            let reference = state.execution_ref();
            let owner = reference.acquire_live_owner(py, &globals).unwrap();
            assert_eq!(owner.as_ptr(), state.owner.as_ptr());
            assert!(reference.acquire_live_owner(py, &PyDict::new(py)).is_err());

            // Native matching must reject an invalid expected address without
            // dereferencing the borrowed identity. These values cannot be
            // produced by the public installation path.
            let invalid = StrictModuleExecutionRef {
                policy: reference.policy.clone(),
                owner_identity: 1,
            };
            assert!(invalid.acquire_live_owner(py, &globals).is_err());

            let weakref = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    ffi::PyWeakref_NewRef(module.as_ptr(), ptr::null_mut()),
                )
            }
            .unwrap();
            drop(module);
            assert!(weakref.call0().unwrap().is_none());
            drop(state);
            assert_eq!(reference.policy.phase(), Phase::Sealed);
            assert_eq!(
                reference.acquire_live_owner(py, &globals).unwrap().as_ptr(),
                owner.as_ptr()
            );
            globals.set_item("mutable_value", 17).unwrap();
            globals.set_item("mutable_value", 23).unwrap();
        });
    }

    #[test]
    fn execution_reference_does_not_resurrect_after_native_or_rust_owner_teardown() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (module, state) = installed(py, &facts());
            let globals = module.dict();
            let reference = state.execution_ref();
            drop(state);
            assert!(reference.acquire_live_owner(py, &globals).is_err());
            drop(module);

            let (module, state) = installed(py, &facts());
            let globals = module.dict();
            let reference = state.execution_ref();
            state.begin_execution(py).unwrap();
            state.seal(py).unwrap();
            // The native owner's GC slot can clear before the dictionary's
            // slot. Its Rust-only notification must revoke this shared policy
            // before any weak edge is released, and the later dictionary
            // terminal callback must accept the already-terminal owner.
            let clear = unsafe { (*ffi::Py_TYPE(state.owner.as_ptr())).tp_clear }.unwrap();
            assert_eq!(unsafe { clear(state.owner.as_ptr()) }, 0);
            assert_eq!(reference.policy.phase(), Phase::Terminal);
            assert!(reference.acquire_live_owner(py, &globals).is_err());
            let clear = unsafe { (*ffi::Py_TYPE(globals.as_ptr())).tp_clear }.unwrap();
            assert_eq!(unsafe { clear(globals.as_ptr()) }, 0);
        });
    }

    #[test]
    fn native_clear_releases_last_dictionary_reference_before_returning_to_cpython() {
        use pyo3::types::PyCapsule;
        use std::sync::atomic::AtomicUsize;

        struct Released(Arc<AtomicUsize>);
        impl Drop for Released {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let released = Arc::new(AtomicUsize::new(0));
        let state = Python::attach(|py| {
            let (module, state) = installed(py, &facts());
            let sentinel = PyCapsule::new(py, Released(released.clone()), None).unwrap();
            module.dict().set_item("sentinel", sentinel).unwrap();
            state.begin_execution(py).unwrap();
            state.seal(py).unwrap();
            drop(module);
            state
        });
        assert_eq!(released.load(Ordering::SeqCst), 0);
        // A real CPython m_clear callback owns the GIL but need not have
        // entered through a PyO3 attachment guard. Acquire it through the C
        // API to exercise that exact boundary rather than flushing PyO3's
        // deferred reference pool before the production cleanup runs.
        let released_before_return = unsafe {
            let gil = ffi::PyGILState_Ensure();
            state.release_from_native(Python::assume_attached());
            let count = released.load(Ordering::SeqCst);
            ffi::PyGILState_Release(gil);
            count
        };
        // Also drain a broken implementation's queue before asserting, so a
        // regression failure cannot leak owning Python references into peers.
        Python::attach(|_| {});
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert_eq!(released_before_return, 1);
    }

    #[test]
    fn native_unfinished_failure_notification_preserves_the_exact_pending_error() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            for sealing in [false, true] {
                let (module, state) = installed(py, &facts());
                state.begin_execution(py).unwrap();
                if sealing {
                    state.begin_sealing(py).unwrap();
                }
                let error = pyo3::exceptions::PyRuntimeError::new_err("original module failure");
                let original = error.value(py).clone().unbind();
                error.restore(py);
                state.fail_unfinished();
                state.fail_unfinished();
                let actual = PyErr::fetch(py);
                assert_eq!(actual.value(py).as_ptr(), original.as_ptr());
                assert_eq!(state.policy.phase(), Phase::Failed);
                assert!(module.dict().set_item("after_failure", 1).is_err());
            }
        });
    }

    #[test]
    fn native_failure_notification_never_revokes_sealed_or_terminal_policy() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (module, state) = installed(py, &facts());
            module.dict().set_item("final_value", 17).unwrap();
            state.begin_execution(py).unwrap();
            state.seal(py).unwrap();
            state.fail_unfinished();
            assert!(state.is_sealed(py).unwrap());
            module
                .dict()
                .set_item("new_after_failure_notice", 1)
                .unwrap();
            assert!(module.dict().set_item("final_value", 18).is_err());

            // Exercise the existing native metadata-shell clear, not a forged
            // policy phase or a second module owner representation.
            let clear = unsafe { (*ffi::Py_TYPE(state.owner.as_ptr())).tp_clear }.unwrap();
            assert_eq!(unsafe { clear(state.owner.as_ptr()) }, 0);
            assert_eq!(state.policy.phase(), Phase::Terminal);
            state.fail_unfinished();
            assert_eq!(state.policy.phase(), Phase::Terminal);
            assert!(
                state
                    .execution_ref()
                    .acquire_live_owner(py, &module.dict())
                    .is_err()
            );
        });
    }

    #[test]
    fn pending_registry_does_not_retain_dead_functions_and_survives_module_wrapper() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (module, state) = installed(py, &facts());
            let globals = module.dict();
            let execution = state.execution_ref();
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let dead = py.eval(c"lambda: None", None, None).unwrap();
            let live = py.eval(c"lambda: None", None, None).unwrap();
            let dead_kind = StrictPendingKind::Function {
                function_id: RuntimeFunctionId::from_raw_parts(1, 1),
            };
            let live_kind = StrictPendingKind::Function {
                function_id: RuntimeFunctionId::from_raw_parts(1, 2),
            };
            register_pending(&owner, dead_kind, &dead).unwrap();
            register_pending(&owner, live_kind.clone(), &live).unwrap();
            let weak_dead = owner.reference(0).unwrap();
            drop(dead);
            assert!(weak_dead.call0().unwrap().is_none());

            state.begin_execution(py).unwrap();
            state.begin_sealing(py).unwrap();
            assert!(state.finish_sealing(py).is_err());
            let pending = next_pending(&owner).unwrap().unwrap();
            assert_eq!(pending.0, live_kind);
            assert_eq!(pending.1.as_ptr(), live.as_ptr());
            state.finish_sealing(py).unwrap();
            drop(module);
            drop(state);
            assert!(execution.acquire_live_owner(py, &globals).is_ok());
            assert!(next_pending(&owner).unwrap().is_none());
            assert!(owner.reference(0).unwrap().is_none());
            assert!(owner.reference(1).unwrap().is_none());

            // Repeated late construction reuses cleared weakref slots rather
            // than growing an interpreter-lifetime edge vector per batch.
            register_pending(&owner, live_kind, &live).unwrap();
            assert!(owner.reference(2).is_err());
            assert!(next_pending(&owner).unwrap().is_some());
        });
    }

    #[test]
    fn native_pending_class_drain_preserves_free_functions_and_other_executions() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (_module, state) = installed(py, &facts());
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let invocation = crate::strict_interpreter::InterpreterInvocationIdentity::new();
            let other_invocation = crate::strict_interpreter::InterpreterInvocationIdentity::new();
            let function = py.eval(c"lambda: None", None, None).unwrap();
            let original = py.eval(c"type('Original', (), {})", None, None).unwrap();
            let other_class = py.eval(c"type('Other', (), {})", None, None).unwrap();
            let reentrant = py.eval(c"type('Original', (), {})", None, None).unwrap();
            let definition = SourceIdentity {
                module: soac_contracts::ModuleContentId::new("pending_completion", 0),
                lexical_qualname: "Original".into(),
                source_range: soac_contracts::SourceRange::new(0, 1),
                definition_kind: soac_contracts::DefinitionKind::Class,
            };
            let class_kind = StrictPendingKind::Class {
                source: definition.clone(),
            };
            let other_kind = StrictPendingKind::Class {
                source: SourceIdentity {
                    lexical_qualname: "Other".into(),
                    ..definition.clone()
                },
            };
            let function_kind = StrictPendingKind::InterpreterFunction {
                native_code_ordinal: 7,
            };
            for (kind, object, call) in [
                (function_kind.clone(), &function, &invocation),
                (other_kind.clone(), &other_class, &invocation),
                (class_kind.clone(), &reentrant, &other_invocation),
                (class_kind.clone(), &original, &invocation),
            ] {
                register_pending_at_phase(
                    &owner,
                    kind,
                    object,
                    PendingPhase::Finalize,
                    None,
                    Some(call),
                )
                .unwrap();
            }
            let selected = |kind: &StrictPendingKind| matches!(kind, StrictPendingKind::Class { source } if source == &definition);
            let pending =
                next_pending_matching(&owner, PendingPhase::Finalize, Some(&invocation), selected)
                    .unwrap()
                    .unwrap();
            assert_eq!(pending.1.as_ptr(), original.as_ptr());
            drop(pending);
            assert!(
                next_pending_matching(&owner, PendingPhase::Finalize, Some(&invocation), selected)
                    .unwrap()
                    .is_none()
            );
            // The class-only completion did not consume a free function that
            // still needs module globals, nor another declaration or activation.
            for (call, expected_kind, expected) in [
                (&invocation, function_kind, &function),
                (&invocation, other_kind, &other_class),
                (&other_invocation, class_kind, &reentrant),
            ] {
                let pending = next_pending_at_phase(&owner, PendingPhase::Finalize, Some(call))
                    .unwrap()
                    .unwrap();
                assert_eq!(pending.0, expected_kind);
                assert_eq!(pending.1.as_ptr(), expected.as_ptr());
            }
            assert!(next_pending(&owner).unwrap().is_none());
        });
    }

    #[test]
    fn native_failed_class_cleanup_keeps_other_graphs_and_scopes() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (_module, state) = installed(py, &facts());
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let invocation = crate::strict_interpreter::InterpreterInvocationIdentity::new();
            let reentrant = crate::strict_interpreter::InterpreterInvocationIdentity::new();
            let original = py.eval(c"type('Original', (), {})", None, None).unwrap();
            let replacement = py.eval(c"type('Replacement', (), {})", None, None).unwrap();
            let other_graph = py.eval(c"type('Original', (), {})", None, None).unwrap();
            let other_source = py.eval(c"type('Other', (), {})", None, None).unwrap();
            let other_call = py.eval(c"type('Original', (), {})", None, None).unwrap();
            let function = py.eval(c"lambda: None", None, None).unwrap();
            let definition = SourceIdentity {
                module: soac_contracts::ModuleContentId::new("pending_cleanup", 0),
                lexical_qualname: "Original".into(),
                source_range: soac_contracts::SourceRange::new(0, 1),
                definition_kind: soac_contracts::DefinitionKind::Class,
            };
            let kind = StrictPendingKind::Class {
                source: definition.clone(),
            };
            let other_kind = StrictPendingKind::Class {
                source: SourceIdentity {
                    lexical_qualname: "Other".into(),
                    ..definition
                },
            };
            let function_kind = StrictPendingKind::InterpreterFunction {
                native_code_ordinal: 7,
            };
            for (record_kind, object, call) in [
                (function_kind.clone(), &function, &invocation),
                (kind.clone(), &other_graph, &invocation),
                (other_kind.clone(), &other_source, &invocation),
                (kind.clone(), &other_call, &reentrant),
                (kind.clone(), &original, &invocation),
                (kind.clone(), &replacement, &invocation),
            ] {
                register_pending_at_phase(
                    &owner,
                    record_kind,
                    object,
                    PendingPhase::Finalize,
                    None,
                    Some(call),
                )
                .unwrap();
            }

            // A failed proof must leave its record for a future authenticated
            // decision, not consume it or drain any other declaration.
            let primary = py
                .eval(c"LookupError('cleanup proof')", None, None)
                .unwrap();
            let context = py
                .eval(c"ValueError('original context')", None, None)
                .unwrap();
            primary.setattr("__context__", &context).unwrap();
            let error = remove_pending_matching(&owner, &kind, Some(&invocation), |_| {
                Err(PyErr::from_value(primary.clone()))
            })
            .unwrap_err();
            assert_eq!(error.value(py).as_ptr(), primary.as_ptr());
            assert_eq!(
                primary.getattr("__context__").unwrap().as_ptr(),
                context.as_ptr()
            );

            // These are structured registry operands, not class authority.
            // Production additionally authenticates the real FAILED native
            // owner, weak type witness and exact dataclass graph edge.
            let same_graph = |actual: &Bound<'_, PyAny>| {
                Ok(actual.as_ptr() == original.as_ptr() || actual.as_ptr() == replacement.as_ptr())
            };
            assert!(remove_pending_matching(&owner, &kind, Some(&invocation), same_graph).unwrap());
            assert!(
                !remove_pending_matching(&owner, &kind, Some(&invocation), same_graph).unwrap()
            );
            // Same source AND same invocation are insufficient: another graph
            // remains pending, along with another source, active call and kind.
            for (call, expected_kind, expected) in [
                (&invocation, function_kind, &function),
                (&invocation, kind.clone(), &other_graph),
                (&invocation, other_kind, &other_source),
                (&reentrant, kind, &other_call),
            ] {
                let pending = next_pending_at_phase(&owner, PendingPhase::Finalize, Some(call))
                    .unwrap()
                    .unwrap();
                assert_eq!(pending.0, expected_kind);
                let actual = pending.1.into_bound(py);
                assert_eq!(actual.as_ptr(), expected.as_ptr());
            }
            assert!(next_pending(&owner).unwrap().is_none());
        });
    }

    #[test]
    fn native_completion_drains_only_its_actual_invocation_and_keeps_targets_weak() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (_module, state) = installed(py, &facts());
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let first_call = crate::strict_interpreter::InterpreterInvocationIdentity::new();
            let second_call = crate::strict_interpreter::InterpreterInvocationIdentity::new();
            let first = py.eval(c"lambda: None", None, None).unwrap();
            let second = py.eval(c"lambda: None", None, None).unwrap();
            let dead = py.eval(c"lambda: None", None, None).unwrap();
            let kind = StrictPendingKind::InterpreterFunction {
                native_code_ordinal: 7,
            };
            for (object, invocation) in [
                (&second, &second_call),
                (&dead, &first_call),
                (&first, &first_call),
            ] {
                register_pending_at_phase(
                    &owner,
                    kind.clone(),
                    object,
                    PendingPhase::Finalize,
                    None,
                    Some(invocation),
                )
                .unwrap();
            }
            let dead_weak = owner.reference(1).unwrap();
            drop(dead);
            assert!(dead_weak.call0().unwrap().is_none());
            let completed =
                next_pending_at_phase(&owner, PendingPhase::Finalize, Some(&first_call))
                    .unwrap()
                    .unwrap();
            assert_eq!(completed.0, kind);
            assert_eq!(completed.1.as_ptr(), first.as_ptr());
            assert!(
                next_pending_at_phase(&owner, PendingPhase::Finalize, Some(&first_call))
                    .unwrap()
                    .is_none()
            );
            let still_active =
                next_pending_at_phase(&owner, PendingPhase::Finalize, Some(&second_call))
                    .unwrap()
                    .unwrap();
            assert_eq!(still_active.1.as_ptr(), second.as_ptr());
            assert!(next_pending(&owner).unwrap().is_none());
        });
    }

    #[test]
    fn native_pending_replacement_inherits_only_its_original_invocation() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (_module, state) = installed(py, &facts());
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let first_call = crate::strict_interpreter::InterpreterInvocationIdentity::new();
            let second_call = crate::strict_interpreter::InterpreterInvocationIdentity::new();
            let original = py.eval(c"lambda: None", None, None).unwrap();
            let replacement = py.eval(c"lambda: None", None, None).unwrap();
            let other = py.eval(c"lambda: None", None, None).unwrap();
            let kind = StrictPendingKind::InterpreterFunction {
                native_code_ordinal: 7,
            };
            for (object, invocation) in [(&other, &second_call), (&original, &first_call)] {
                register_pending_at_phase(
                    &owner,
                    kind.clone(),
                    object,
                    PendingPhase::Finalize,
                    None,
                    Some(invocation),
                )
                .unwrap();
            }
            register_pending_before(&owner, kind, &replacement, &original).unwrap();
            for expected in [&replacement, &original] {
                let completed =
                    next_pending_at_phase(&owner, PendingPhase::Finalize, Some(&first_call))
                        .unwrap()
                        .unwrap();
                assert_eq!(completed.1.as_ptr(), expected.as_ptr());
            }
            assert!(
                next_pending_at_phase(&owner, PendingPhase::Finalize, Some(&first_call))
                    .unwrap()
                    .is_none()
            );
            let untouched = next_pending(&owner).unwrap().unwrap();
            assert_eq!(untouched.1.as_ptr(), other.as_ptr());
        });
    }

    #[test]
    fn capability_publication_waits_for_finalization_without_retaining_weak_targets() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (_module, state) = installed(py, &facts());
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let first = py.eval(c"lambda: None", None, None).unwrap();
            let second = py.eval(c"lambda: None", None, None).unwrap();
            let kind = StrictPendingKind::Function {
                function_id: RuntimeFunctionId::from_raw_parts(1, 11),
            };
            state.begin_execution(py).unwrap();
            state.begin_sealing(py).unwrap();
            state
                .defer_capability_publication(py, kind.clone(), &first)
                .unwrap();
            state
                .defer_capability_publication(py, kind.clone(), &second)
                .unwrap();
            let weak_first = owner.reference(0).unwrap();
            let weak_second = owner.reference(1).unwrap();
            register_pending(&owner, kind.clone(), &second).unwrap();
            let sealing = state.next_pending(py).unwrap().unwrap();
            assert_eq!(sealing.1.as_ptr(), second.as_ptr());
            drop(sealing);
            assert!(state.next_pending(py).unwrap().is_none());
            assert!(state.finish_sealing(py).is_err());
            drop(first);
            assert!(weak_first.call0().unwrap().is_none());
            let publishing = state.next_capability_publication(py).unwrap().unwrap();
            assert_eq!(publishing.0, kind);
            assert_eq!(publishing.1.as_ptr(), second.as_ptr());
            drop(second);
            assert!(!weak_second.call0().unwrap().is_none());
            drop(publishing);
            assert!(weak_second.call0().unwrap().is_none());
            assert!(state.next_capability_publication(py).unwrap().is_none());
            state.finish_sealing(py).unwrap();
            for edge in 0..3 {
                assert!(owner.reference(edge).unwrap().is_none());
            }
        });
    }

    #[test]
    fn pending_adoption_removes_only_the_exact_kind_and_live_value() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (_module, state) = installed(py, &facts());
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let first = py.eval(c"lambda: None", None, None).unwrap();
            let second = py.eval(c"lambda: None", None, None).unwrap();
            let kind = StrictPendingKind::Function {
                function_id: RuntimeFunctionId::from_raw_parts(1, 7),
            };
            let other_kind = StrictPendingKind::Function {
                function_id: RuntimeFunctionId::from_raw_parts(1, 8),
            };
            register_pending(&owner, kind.clone(), &first).unwrap();
            register_pending(&owner, kind.clone(), &second).unwrap();
            register_pending(&owner, other_kind.clone(), &first).unwrap();
            assert!(!remove_pending(&owner, &other_kind, &second).unwrap());
            assert!(remove_pending(&owner, &kind, &first).unwrap());
            assert!(!remove_pending(&owner, &kind, &first).unwrap());
            let pending = next_pending(&owner).unwrap().unwrap();
            assert_eq!(pending.0, kind);
            assert_eq!(pending.1.as_ptr(), second.as_ptr());
            let pending = next_pending(&owner).unwrap().unwrap();
            assert_eq!(pending.0, other_kind);
            assert_eq!(pending.1.as_ptr(), first.as_ptr());
            assert!(next_pending(&owner).unwrap().is_none());
        });
    }

    #[test]
    fn replacement_pending_insertion_preserves_original_and_declaration_order_on_failure() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (_module, state) = installed(py, &facts());
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let preceding = py.eval(c"lambda: None", None, None).unwrap();
            let original = py.eval(c"lambda: None", None, None).unwrap();
            let following = py.eval(c"lambda: None", None, None).unwrap();
            let replacement = py.eval(c"lambda: None", None, None).unwrap();
            let kind = StrictPendingKind::Function {
                function_id: RuntimeFunctionId::from_raw_parts(1, 19),
            };
            let other_kind = StrictPendingKind::Function {
                function_id: RuntimeFunctionId::from_raw_parts(1, 20),
            };
            for object in [&preceding, &original, &following] {
                register_pending(&owner, kind.clone(), object).unwrap();
            }
            let original_edges: Vec<_> = owner
                .data()
                .pending
                .borrow()
                .records
                .iter()
                .map(|record| record.edge)
                .collect();
            assert!(register_pending_before(&owner, other_kind, &replacement, &original,).is_err());
            assert!(register_pending_before(
                &owner,
                kind.clone(),
                &py.None().into_bound(py),
                &original,
            ).is_err(), "a failed weakref allocation must not remove the original");
            assert_eq!(
                owner
                    .data()
                    .pending
                    .borrow()
                    .records
                    .iter()
                    .map(|record| record.edge)
                    .collect::<Vec<_>>(),
                original_edges,
            );
            register_pending_before(&owner, kind.clone(), &replacement, &original).unwrap();
            for expected in [&preceding, &replacement, &original, &following] {
                let actual = next_pending(&owner).unwrap().unwrap();
                assert_eq!(actual.0, kind);
                assert_eq!(actual.1.as_ptr(), expected.as_ptr());
            }
            assert!(next_pending(&owner).unwrap().is_none());
        });
    }

    #[test]
    fn replacement_pending_records_keep_both_class_edges_weak() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (_module, state) = installed(py, &facts());
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let original = py.eval(c"type('Original', (), {})", None, None).unwrap();
            let replacement = py.eval(c"type('Replacement', (), {})", None, None).unwrap();
            let weakref = py.import("weakref").unwrap().getattr("ref").unwrap();
            let original_weak = weakref.call1((&original,)).unwrap();
            let replacement_weak = weakref.call1((&replacement,)).unwrap();
            let kind = StrictPendingKind::Class {
                source: SourceIdentity {
                    module: soac_contracts::ModuleContentId::new("replacement_pending", 0),
                    lexical_qualname: "Original".into(),
                    source_range: soac_contracts::SourceRange::new(0, 1),
                    definition_kind: soac_contracts::DefinitionKind::Class,
                },
            };
            register_pending(&owner, kind.clone(), &original).unwrap();
            register_pending_before(&owner, kind, &replacement, &original).unwrap();
            drop(original);
            drop(replacement);
            py.import("gc").unwrap().call_method0("collect").unwrap();
            assert!(original_weak.call0().unwrap().is_none());
            assert!(replacement_weak.call0().unwrap().is_none());
            assert!(next_pending(&owner).unwrap().is_none());
        });
    }

    #[test]
    fn popping_one_pending_value_does_not_keep_other_callables_alive() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (_module, state) = installed(py, &facts());
            let owner =
                NamespaceOwner::from_owner(state.owner.clone_ref(py).into_bound(py)).unwrap();
            let first = py.eval(c"lambda: None", None, None).unwrap();
            let second = py.eval(c"lambda: None", None, None).unwrap();
            let kind = StrictPendingKind::Function {
                function_id: RuntimeFunctionId::from_raw_parts(1, 9),
            };
            register_pending(&owner, kind.clone(), &first).unwrap();
            register_pending(&owner, kind, &second).unwrap();
            let second_weakref = owner.reference(1).unwrap();
            let active = state.next_pending(py).unwrap().unwrap();
            assert_eq!(active.1.as_ptr(), first.as_ptr());
            drop(second);
            assert!(second_weakref.call0().unwrap().is_none());
            assert!(state.next_pending(py).unwrap().is_none());
        });
    }
}
