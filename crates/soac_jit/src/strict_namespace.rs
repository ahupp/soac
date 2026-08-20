//! One authenticated invocation of a compiler-created class namespace.
//!
//! Source identity is not execution ownership: the same class factory can run
//! repeatedly, and a callback can borrow a method from an earlier dynamic
//! class. An explicit, single-use argument binds this invocation to its actual
//! namespace function, namespace, and module execution. Native class cells are
//! created only by the bound namespace body's explicit recipe. Only the active
//! namespace frame propagates its Rust-only identity to MakeFunction.

use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use pyo3::ffi;
use pyo3::prelude::*;
use soac_contracts::SourceIdentity;
use soac_core::block_py::CallableSourceRole;

use crate::module_type::SharedModuleState;
use crate::strict_function::{AuthenticatedStrictFunction, ClassConstructionCaptures};
use crate::strict_runtime_unavailable;
use crate::strict_state::{StrictStateData, StrictStateRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum NamespacePhase {
    Prepared,
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Copy)]
struct ClassDictionaryCoordinates {
    owner: usize,
    dictionary: usize,
}

/// Coordinates only, not a reference or a live-object capability. The native
/// class-policy predicate must authenticate a pinned dictionary and its owner
/// before a consumer dereferences that owner. A final private Rust TypeId and
/// execution-Arc check is still required to exclude address reuse.
#[derive(Default)]
struct ClassDictionaryWitness {
    coordinates: OnceLock<ClassDictionaryCoordinates>,
    terminal: AtomicBool,
}

impl ClassDictionaryWitness {
    fn record(&self, owner: usize, dictionary: usize) -> Result<(), &'static str> {
        if owner == 0 || dictionary == 0 || self.terminal.load(Ordering::Acquire) {
            return Err("class dictionary binding is unavailable");
        }
        self.coordinates
            .set(ClassDictionaryCoordinates { owner, dictionary })
            .map_err(|_| "class dictionary binding was already recorded")
    }

    fn candidate_owner(&self, dictionary: usize) -> Option<usize> {
        if self.terminal.load(Ordering::Acquire) {
            return None;
        }
        let coordinates = self.coordinates.get()?;
        (coordinates.dictionary == dictionary).then_some(coordinates.owner)
    }

    fn invalidate(&self) {
        self.terminal.store(true, Ordering::Release);
    }
}

/// This object never owns Python references. Its Arc allocation is the
/// execution identity; no process-global counter or Python-visible integer
/// can substitute for it. Completed identities can outlive the namespace and
/// its function without keeping those objects or the module alive.
pub(crate) struct NamespaceExecution {
    source: SourceIdentity,
    interpreter: i64,
    source_execution: Option<(
        Arc<crate::VerifiedStrictModule>,
        crate::StrictModuleExecutionRef,
    )>,
    globals: usize,
    function_owner: usize,
    // The runtime performs transitions under the GIL, but completed identity
    // witnesses also live in background-compiled immutable capability slots.
    // Do not make a Cell-containing witness Send/Sync with an unsafe impl.
    phase: AtomicU8,
    class_dictionary: ClassDictionaryWitness,
}

impl NamespaceExecution {
    /// Identity only for isolated native class-state kernel tests. This does
    /// not represent an authenticated namespace invocation; production source
    /// and single-use handle authentication are exercised by integration tests.
    #[cfg(test)]
    pub(crate) fn completed_identity_for_test(
        source: SourceIdentity,
        interpreter: i64,
    ) -> Arc<Self> {
        Arc::new(Self {
            source,
            interpreter,
            source_execution: None, // Isolated test identity, never source authority.
            globals: 0,
            function_owner: 0,
            phase: AtomicU8::new(NamespacePhase::Completed as u8),
            class_dictionary: ClassDictionaryWitness::default(),
        })
    }

    /// Begin the real ordinary __build_class__ namespace activation. The native
    /// caller already authenticated its actual function/code/globals and owns
    /// those objects; this Arc stores Rust metadata and scalar comparisons only.
    pub(crate) fn begin_native(
        py: Python<'_>,
        verified: Arc<crate::VerifiedStrictModule>,
        execution: crate::StrictModuleExecutionRef,
        source: SourceIdentity,
        globals_identity: usize,
        function_owner: usize,
    ) -> PyResult<Arc<Self>> {
        if source.definition_kind != soac_contracts::DefinitionKind::Class
            || globals_identity == 0
            || function_owner == 0
            || !verified
                .type_facts()
                .facts()
                .classes
                .iter()
                .any(|class| class.identity == source)
            || unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) }
                != verified.interpreter_id()
        {
            return Err(strict_runtime_unavailable(
                py,
                "native namespace lacks its authenticated actual class execution",
            ));
        }
        Ok(Arc::new(Self {
            source,
            interpreter: verified.interpreter_id(),
            source_execution: Some((verified, execution)),
            globals: globals_identity,
            function_owner,
            phase: AtomicU8::new(NamespacePhase::Running as u8),
            class_dictionary: ClassDictionaryWitness::default(),
        }))
    }

    /// Callback/error/refcount-free one-use completion for the native leave seam.
    pub(crate) fn complete_native(&self) -> bool {
        self.transition(NamespacePhase::Running, NamespacePhase::Completed)
    }

    pub(crate) fn fail_native_unfinished(&self) {
        self.fail_unfinished();
    }

    pub(crate) fn matches_source_execution(
        &self,
        verified: &Arc<crate::VerifiedStrictModule>,
        execution: &crate::StrictModuleExecutionRef,
    ) -> bool {
        self.source_execution
            .as_ref()
            .is_some_and(|(expected, actual)| {
                Arc::ptr_eq(expected, verified) && actual.same_execution(execution)
            })
    }

    pub(crate) fn validate_native_creation(
        &self,
        py: Python<'_>,
        verified: &Arc<crate::VerifiedStrictModule>,
        execution: &crate::StrictModuleExecutionRef,
        globals_identity: usize,
    ) -> PyResult<()> {
        if self.phase() != NamespacePhase::Running
            || self.interpreter != verified.interpreter_id()
            || !self.matches_source_execution(verified, execution)
            || self.globals != globals_identity
        {
            return Err(strict_runtime_unavailable(
                py,
                "native birth is outside its actual active class namespace",
            ));
        }
        Ok(())
    }

    fn phase(&self) -> NamespacePhase {
        match self.phase.load(Ordering::Acquire) {
            phase if phase == NamespacePhase::Prepared as u8 => NamespacePhase::Prepared,
            phase if phase == NamespacePhase::Running as u8 => NamespacePhase::Running,
            phase if phase == NamespacePhase::Completed as u8 => NamespacePhase::Completed,
            _ => NamespacePhase::Failed,
        }
    }

    fn transition(&self, from: NamespacePhase, to: NamespacePhase) -> bool {
        self.phase
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn fail_unfinished(&self) {
        let _ = self
            .phase
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |phase| {
                (phase == NamespacePhase::Prepared as u8 || phase == NamespacePhase::Running as u8)
                    .then_some(NamespacePhase::Failed as u8)
            });
    }

    pub(crate) fn source(&self) -> &SourceIdentity {
        &self.source
    }

    pub(crate) fn is_completed(&self) -> bool {
        self.phase() == NamespacePhase::Completed
    }

    /// Called by the class owner after validating its actual copied namespace
    /// in the native pre-Ready binding callback. The successful path neither
    /// allocates nor runs Python. This records expected coordinates only: the
    /// native dictionary policy is installed after that callback returns.
    pub(crate) fn record_class_dictionary(
        &self,
        py: Python<'_>,
        owner: &Bound<'_, PyAny>,
        dictionary: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let interpreter = unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
        if interpreter < 0 {
            return Err(PyErr::fetch(py));
        }
        if !self.is_completed()
            || interpreter != self.interpreter
            || unsafe { ffi::PyDict_CheckExact(dictionary.as_ptr()) } == 0
        {
            return Err(strict_runtime_unavailable(
                py,
                "class dictionary does not belong to a completed namespace execution",
            ));
        }
        self.class_dictionary
            .record(owner.as_ptr() as usize, dictionary.as_ptr() as usize)
            .map_err(|message| strict_runtime_unavailable(py, message))
    }

    /// Only a candidate for the native class-policy predicate. Never
    /// dereference this address until that predicate has pinned/proved the
    /// private owner, and then require its actual NamespaceExecution Arc to
    /// equal this execution. Source or address equality alone is insufficient.
    pub(crate) fn class_dictionary_owner_candidate(&self, dictionary: usize) -> Option<usize> {
        self.is_completed()
            .then(|| self.class_dictionary.candidate_owner(dictionary))
            .flatten()
    }

    /// Additional fail-closed invalidation before the Rust class owner's edges
    /// are released. It does not replace the native actual-type lifetime guard:
    /// a dictionary/owner can survive after the type itself has died.
    pub(crate) fn invalidate_class_dictionary(&self) {
        self.class_dictionary.invalidate();
    }

    /// Called before attaching this identity to a newly created function.
    /// Merely calling an existing method must never reactivate its creation
    /// identity or confer it on functions made by that ordinary method call.
    pub(crate) fn validate_creation(
        &self,
        py: Python<'_>,
        shared: &Arc<SharedModuleState>,
        globals: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let interpreter = unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
        if interpreter < 0 {
            return Err(PyErr::fetch(py));
        }
        if self.phase() != NamespacePhase::Running
            || self.interpreter != interpreter
            || !shared.strict_module.as_ref().is_some_and(|verified| {
                shared
                    .strict_execution
                    .as_ref()
                    .is_some_and(|execution| self.matches_source_execution(verified, execution))
            })
            || self.globals != globals.as_ptr() as usize
        {
            return Err(strict_runtime_unavailable(
                py,
                "function creation is outside its active class namespace execution",
            ));
        }
        Ok(())
    }
}

struct NamespaceHandleData {
    execution: Arc<NamespaceExecution>,
    private_cell_count: usize,
}

// SAFETY: The execution identity is Rust-only. The short-lived Python edges
// are all in the GC shell, and are cleared as soon as binding consumes them.
unsafe impl StrictStateData for NamespaceHandleData {
    const TYPE_NAME: &'static CStr = c"soac._StrictNamespaceExecution";

    fn on_terminal(&self) {
        self.execution.fail_unfinished();
    }
}

const NAMESPACE_FUNCTION: usize = 0;
const NAMESPACE: usize = 1;
const PRIVATE_CELLS: usize = 2;

fn clear_handle_references(state: &StrictStateRef<'_, NamespaceHandleData>) -> PyResult<()> {
    let py = state.owner().py();
    let count = PRIVATE_CELLS + state.data().private_cell_count;
    let mut result = Ok(());
    for index in 0..count {
        let cleared = state.set_reference(index, py.None().into_bound(py));
        if result.is_ok() {
            result = cleared;
        }
    }
    result
}

/// Original cells remain owned only by the active namespace environment. The
/// execution identity can later outlive it without retaining lexical values.
pub(crate) struct ConsumedNamespace<'py> {
    pub(crate) execution: Arc<NamespaceExecution>,
    pub(crate) private_cells: Vec<Bound<'py, PyAny>>,
}

pub(crate) struct NamespaceHandle<'py> {
    state: StrictStateRef<'py, NamespaceHandleData>,
    execution: Arc<NamespaceExecution>,
}

impl<'py> NamespaceHandle<'py> {
    pub(crate) fn new(
        py: Python<'py>,
        auth: &AuthenticatedStrictFunction<'_, 'py>,
        function: &Bound<'py, PyAny>,
        namespace: &Bound<'py, PyAny>,
        construction_captures: Option<&ClassConstructionCaptures<'py>>,
    ) -> PyResult<Self> {
        let source = auth
            .origin()
            .filter(|origin| origin.role == CallableSourceRole::ClassNamespace)
            .ok_or_else(|| strict_runtime_unavailable(py, "namespace handle has no source role"))?
            .definition
            .clone();
        let expected_private_cells = auth.namespace_private_cell_count()?;
        let private_cells = match construction_captures {
            Some(captures) => captures.take_namespace_cells(auth)?,
            None if expected_private_cells == 0 => Vec::new(),
            None => {
                return Err(strict_runtime_unavailable(
                    py,
                    "class namespace has no authenticated private lexical cells",
                ));
            }
        };
        if private_cells.len() != expected_private_cells {
            return Err(strict_runtime_unavailable(
                py,
                "class namespace private lexical projection changed",
            ));
        }
        let execution = Arc::new(NamespaceExecution {
            source,
            interpreter: auth.verified_module().interpreter_id(),
            source_execution: Some((
                Arc::clone(auth.verified_module()),
                auth.execution_ref().clone(),
            )),
            globals: auth.globals()?.as_ptr() as usize,
            function_owner: auth.owner().as_ptr() as usize,
            phase: AtomicU8::new(NamespacePhase::Prepared as u8),
            class_dictionary: ClassDictionaryWitness::default(),
        });
        let mut references = vec![function.clone().unbind(), namespace.clone().unbind()];
        references.extend(private_cells.into_iter().map(Bound::unbind));
        let state = StrictStateRef::new(
            py,
            NamespaceHandleData {
                execution: Arc::clone(&execution),
                private_cell_count: expected_private_cells,
            },
            references,
        )?;
        Ok(Self { state, execution })
    }

    pub(crate) fn argument(&self) -> &Bound<'py, PyAny> {
        self.state.owner()
    }

    pub(crate) fn complete(&self) -> PyResult<Arc<NamespaceExecution>> {
        if !self
            .execution
            .transition(NamespacePhase::Running, NamespacePhase::Completed)
        {
            return Err(strict_runtime_unavailable(
                self.argument().py(),
                "class namespace did not consume its intended execution handle",
            ));
        }
        Ok(Arc::clone(&self.execution))
    }
}

impl Drop for NamespaceHandle<'_> {
    fn drop(&mut self) {
        // A leaked handle cannot be used after an exception or a failed call.
        self.execution.fail_unfinished();
        // It also cannot keep the namespace, function, or original lexical
        // cells alive. Publish failure/completion before any decref reentry.
        let _ = clear_handle_references(&self.state);
    }
}

/// Run after ordinary argument binding and before any namespace instruction.
/// The second parameter exists only on the authenticated compiler helper, not
/// on user functions. No namespace lookup, annotation evaluation, or Python
/// callback supplies authority here.
pub(crate) fn consume_handle<'py>(
    py: Python<'py>,
    auth: &AuthenticatedStrictFunction<'_, 'py>,
    function: *mut ffi::PyObject,
    parameters: &[*mut ffi::PyObject],
) -> PyResult<Option<ConsumedNamespace<'py>>> {
    let Some(origin) = auth.origin() else {
        return Ok(None);
    };
    if origin.role != CallableSourceRole::ClassNamespace {
        return Ok(None);
    }
    if parameters.len() != 2 || parameters.iter().any(|parameter| parameter.is_null()) {
        return Err(strict_runtime_unavailable(
            py,
            "class namespace requires its explicit namespace-and-handle binding",
        ));
    }
    let state = StrictStateRef::<NamespaceHandleData>::from_owner(unsafe {
        Bound::from_borrowed_ptr(py, parameters[1])
    })?;
    let execution = &state.data().execution;
    if execution.phase() != NamespacePhase::Prepared
        || execution.interpreter != auth.verified_module().interpreter_id()
        || !execution.matches_source_execution(auth.verified_module(), auth.execution_ref())
        || execution.globals != auth.globals()?.as_ptr() as usize
        || execution.function_owner != auth.owner().as_ptr() as usize
        || execution.source != origin.definition
        || state.data().private_cell_count != auth.namespace_private_cell_count()?
        || state.reference(NAMESPACE_FUNCTION)?.as_ptr() != function
        || state.reference(NAMESPACE)?.as_ptr() != parameters[0]
    {
        return Err(strict_runtime_unavailable(
            py,
            "class namespace execution handle was replayed or transferred",
        ));
    }
    let private_cells = (0..state.data().private_cell_count)
        .map(|index| state.reference(PRIVATE_CELLS + index))
        .collect::<PyResult<Vec<_>>>()?;
    if !execution.transition(NamespacePhase::Prepared, NamespacePhase::Running) {
        return Err(strict_runtime_unavailable(
            py,
            "class namespace execution handle was already consumed",
        ));
    }
    let execution = Arc::clone(execution);
    // Ordinary operands are already owned by the caller; the returned view
    // pins private cells before removing the handle's temporary GC edges.
    if let Err(error) = clear_handle_references(&state) {
        execution.fail_unfinished();
        return Err(error);
    }
    Ok(Some(ConsumedNamespace {
        execution,
        private_cells,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyDict;
    use soac_contracts::{DefinitionKind, ModuleContentId, SourceRange};

    #[test]
    fn escaped_namespace_handle_releases_every_edge_on_drop() -> PyResult<()> {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            for phase in [
                NamespacePhase::Prepared,
                NamespacePhase::Running,
                NamespacePhase::Completed,
            ] {
                // Exercise the actual handle teardown, including a call that
                // failed before binding. This synthetic identity supplies no
                // admission authority; real namespace calls have integration
                // coverage for both compiled and interpreted source bodies.
                let execution = NamespaceExecution::completed_identity_for_test(
                    SourceIdentity {
                        module: ModuleContentId::new("namespace_release_fixture", 1),
                        lexical_qualname: "Factory.Class".into(),
                        source_range: SourceRange::new(0, 1),
                        definition_kind: DefinitionKind::Class,
                    },
                    0,
                );
                execution.phase.store(phase as u8, Ordering::Release);
                let value = PyDict::new(py).into_any();
                let before = unsafe { ffi::Py_REFCNT(value.as_ptr()) };
                let private_cell_count = 2;
                let count = PRIVATE_CELLS + private_cell_count;
                let state = StrictStateRef::new(
                    py,
                    NamespaceHandleData {
                        execution: Arc::clone(&execution),
                        private_cell_count,
                    },
                    (0..count).map(|_| value.clone().unbind()).collect(),
                )?;
                let escaped =
                    StrictStateRef::<NamespaceHandleData>::from_owner(state.owner().clone())?;
                drop(NamespaceHandle {
                    state,
                    execution: Arc::clone(&execution),
                });
                let expected = if phase == NamespacePhase::Completed {
                    NamespacePhase::Completed
                } else {
                    NamespacePhase::Failed
                };
                assert_eq!(execution.phase(), expected);
                for index in 0..count {
                    assert!(escaped.reference(index)?.is_none());
                }
                assert_eq!(unsafe { ffi::Py_REFCNT(value.as_ptr()) }, before);
            }
            Ok(())
        })
    }

    #[test]
    fn completed_namespace_identity_is_thread_safe_and_never_reopened_or_failed() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NamespaceExecution>();
        let source = SourceIdentity {
            module: ModuleContentId::new("namespace_identity_fixture", 1),
            lexical_qualname: "Factory.Class".into(),
            source_range: SourceRange::new(0, 1),
            definition_kind: DefinitionKind::Class,
        };
        let execution = NamespaceExecution {
            source,
            interpreter: 0,
            source_execution: None, // Isolated test identity, never source authority.
            globals: 0,
            function_owner: 0,
            phase: AtomicU8::new(NamespacePhase::Prepared as u8),
            class_dictionary: ClassDictionaryWitness::default(),
        };
        assert!(execution.transition(NamespacePhase::Prepared, NamespacePhase::Running));
        assert!(!execution.transition(NamespacePhase::Prepared, NamespacePhase::Running));
        assert!(execution.transition(NamespacePhase::Running, NamespacePhase::Completed));
        execution.fail_unfinished();
        assert!(execution.is_completed());
        assert!(!execution.transition(NamespacePhase::Running, NamespacePhase::Failed));

        let unfinished = NamespaceExecution {
            phase: AtomicU8::new(NamespacePhase::Running as u8),
            ..execution
        };
        unfinished.fail_unfinished();
        assert_eq!(unfinished.phase(), NamespacePhase::Failed);
        assert!(!unfinished.transition(NamespacePhase::Running, NamespacePhase::Completed));
    }

    #[test]
    fn copied_class_dictionary_coordinates_are_one_time_and_terminal() {
        // These integers deliberately grant no native object authority. The
        // integration regression separately exercises actual dictionary policy
        // and execution authentication before any alias can be consumed.
        let witness = ClassDictionaryWitness::default();
        assert!(witness.record(0, 2).is_err());
        assert!(witness.record(1, 0).is_err());
        assert_eq!(witness.candidate_owner(2), None);
        witness.record(1, 2).unwrap();
        assert_eq!(witness.candidate_owner(2), Some(1));
        assert_eq!(witness.candidate_owner(3), None);
        assert!(witness.record(1, 2).is_err());
        assert!(witness.record(3, 4).is_err());
        witness.invalidate();
        assert_eq!(witness.candidate_owner(2), None);
        assert!(witness.record(1, 2).is_err());

        let unbound = ClassDictionaryWitness::default();
        unbound.invalidate();
        assert!(unbound.record(1, 2).is_err());
    }
}
