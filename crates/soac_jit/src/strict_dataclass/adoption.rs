//! The class-layout proof is distinct from source-function eligibility.
//!
//! A construction proof owns a live invocation only until Apply finishes. The
//! permanent class edge then owns callback-free weak member witnesses, never
//! the invocation's class, builder, helper catalog or default operands.

use std::cell::{Cell, OnceCell};
use std::ffi::CStr;
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use soac_contracts::{ClassTypeFact, Fingerprint};

use crate::strict_class::ClassNamespacePhase;
use crate::strict_class_state::StrictClassState;
use crate::strict_field_bindings::StrictFieldBinding;
use crate::strict_namespace::NamespaceExecution;
use crate::strict_runtime_unavailable;
use crate::strict_state::{StrictStateData, StrictStateRef};

use super::catalog::{Helper, WeakIdentity, dictionary_value, text_is};
use super::generation::{GeneratedRole, GenerationPlan};
use super::native::{PyFunction_GetSoacStrictId, PyFunction_HasSoacDataclassCreation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Prepared,
    Bound,
    Completing,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MemberKind {
    Generated(GeneratedRole),
    Shared(Helper),
}

#[derive(Clone)]
pub(super) struct MemberPlan {
    pub(super) name: String,
    pub(super) kind: MemberKind,
}

pub(super) struct ClassPlan {
    pub(super) fact: ClassTypeFact,
    pub(super) source_digest: Fingerprint,
    pub(super) namespace: Arc<NamespaceExecution>,
    pub(super) generation: Arc<GenerationPlan>,
    pub(super) members: Vec<MemberPlan>,
    pub(super) phase: Cell<Phase>,
    pub(super) actual_class: Cell<usize>,
    pub(super) actual_owner: Cell<usize>,
    // A replacement has its own phase/type/owner and permanent witnesses.
    // This Rust-only declaring link does not retain the original Python type,
    // namespace function, class dictionary or invocation owner.
    pub(super) replacement_of: Option<Arc<ClassPlan>>,
    // Filled only after every actual member and owned component has passed
    // live native provenance/check-plan validation and permanent adoption.
    completed_members: OnceCell<Vec<MemberWitness>>,
}

impl ClassPlan {
    pub(super) fn new(
        fact: ClassTypeFact,
        source_digest: Fingerprint,
        namespace: Arc<NamespaceExecution>,
        generation: GenerationPlan,
        members: Vec<MemberPlan>,
    ) -> Self {
        Self {
            fact,
            source_digest,
            namespace,
            generation: Arc::new(generation),
            members,
            phase: Cell::new(Phase::Prepared),
            actual_class: Cell::new(0),
            actual_owner: Cell::new(0),
            replacement_of: None,
            completed_members: OnceCell::new(),
        }
    }

    pub(super) fn slots_replacement(original: &Arc<ClassPlan>) -> Self {
        Self {
            fact: original.fact.clone(),
            source_digest: original.source_digest,
            namespace: Arc::clone(&original.namespace),
            generation: Arc::clone(&original.generation),
            members: original.members.clone(),
            phase: Cell::new(Phase::Prepared),
            actual_class: Cell::new(0),
            actual_owner: Cell::new(0),
            replacement_of: Some(Arc::clone(original)),
            completed_members: OnceCell::new(),
        }
    }

    pub(super) fn fail(&self) {
        if self.phase.get() != Phase::Complete {
            self.phase.set(Phase::Failed);
        }
    }

    fn matches_source(&self, fact: &ClassTypeFact, execution: &Arc<NamespaceExecution>) -> bool {
        self.fact == *fact
            && Arc::ptr_eq(&self.namespace, execution)
            && self.namespace.is_completed()
            && self.phase.get() != Phase::Failed
    }
}

struct MemberWitness {
    function: WeakIdentity,
    sealed_id: Option<u64>,
}

struct AdoptedMembers {
    plan: Arc<ClassPlan>,
}

unsafe impl StrictStateData for AdoptedMembers {
    const TYPE_NAME: &'static CStr = c"soac._DataclassMembers";
}

/// An unpublished or committed permanent member owner, minted only by the
/// adapter after validating actual generated functions. The class publication
/// path checks this typed proof against its own plan before replacing its
/// temporary invocation edge.
pub(crate) struct DataclassAdoptedMembers<'py> {
    plan: Arc<ClassPlan>,
    owner: Bound<'py, PyAny>,
}

impl<'py> DataclassAdoptedMembers<'py> {
    pub(crate) fn owner(&self) -> &Bound<'py, PyAny> {
        &self.owner
    }

    pub(crate) fn matches(&self, namespace: &DataclassNamespace<'_>) -> bool {
        Arc::ptr_eq(&self.plan, &namespace.plan)
            && self.plan.phase.get() == Phase::Complete
            && self.plan.completed_members.get().is_some()
    }
}

/// The allocating half of publication. Preparing both classes first lets the
/// slots carrier revalidate their actual members before any one-way commit.
pub(super) struct PreparedMembers<'py> {
    adopted: DataclassAdoptedMembers<'py>,
    witnesses: Vec<MemberWitness>,
}

impl<'py> PreparedMembers<'py> {
    pub(super) fn stage(self) -> PyResult<DataclassAdoptedMembers<'py>> {
        let py = self.adopted.owner.py();
        self.adopted
            .plan
            .completed_members
            .set(self.witnesses)
            .map_err(|_| {
                strict_runtime_unavailable(py, "dataclass members were already adopted")
            })?;
        Ok(self.adopted)
    }
}

/// Private proof minted by the adapter's live, source-bound construction
/// decision. No public Python value or compiler proposal can construct it.
pub(crate) struct DataclassConstruction<'py> {
    pub(super) plan: Arc<ClassPlan>,
    pub(super) invocation_owner: Bound<'py, PyAny>,
}

impl<'py> DataclassConstruction<'py> {
    /// Exact declaring-field owners prepared by this live invocation. The
    /// class keeps only these minimal GC owners, never the active adapter graph.
    pub(crate) fn own_field_bindings(&self) -> PyResult<Vec<StrictFieldBinding<'py>>> {
        super::invocation::own_field_bindings(&self.invocation_owner, &self.plan)
    }

    pub(crate) fn namespace(&self) -> DataclassNamespace<'py> {
        DataclassNamespace {
            plan: Arc::clone(&self.plan),
            owner: self.invocation_owner.clone(),
        }
    }

    pub(crate) fn matches(
        &self,
        fact: &ClassTypeFact,
        digest: Fingerprint,
        execution: &Arc<NamespaceExecution>,
    ) -> bool {
        self.plan.phase.get() == Phase::Prepared
            && self.plan.source_digest == digest
            && self.plan.matches_source(fact, execution)
    }

    pub(crate) fn protected_names(&self) -> impl Iterator<Item = &str> {
        self.plan.members.iter().map(|member| member.name.as_str())
    }

    pub(crate) fn attach(&self, references: &mut Vec<Py<PyAny>>) -> DataclassClassState {
        let reference = references.len();
        references.push(self.invocation_owner.clone().unbind());
        DataclassClassState {
            plan: Arc::clone(&self.plan),
            reference,
        }
    }
}

/// A distinct replacement-construction proof. It can be minted only at the
/// authenticated native slots bridge. Declaring source execution is shared;
/// actual construction identity, native owner and member publication are not.
/// The ephemeral native FrameView is deliberately absent from this payload.
pub(crate) struct DataclassSlotsConstruction<'py> {
    pub(super) plan: Arc<ClassPlan>,
    pub(super) original: Arc<ClassPlan>,
    pub(super) invocation_owner: Bound<'py, PyAny>,
}

impl<'py> DataclassSlotsConstruction<'py> {
    pub(crate) fn matches(
        &self,
        original: &StrictClassState<'_>,
        fact: &ClassTypeFact,
        digest: Fingerprint,
        execution: &Arc<NamespaceExecution>,
    ) -> PyResult<bool> {
        let Some(original_namespace) = original.dataclass_namespace()? else {
            return Ok(false);
        };
        Ok(self.plan.phase.get() == Phase::Prepared
            && self.original.phase.get() == Phase::Bound
            && self.plan.source_digest == digest
            && self.plan.matches_source(fact, execution)
            && !Arc::ptr_eq(&self.plan, &self.original)
            && self
                .plan
                .replacement_of
                .as_ref()
                .is_some_and(|plan| Arc::ptr_eq(plan, &self.original))
            && Arc::ptr_eq(&original_namespace.plan, &self.original)
            && original_namespace.owner.as_ptr() == self.invocation_owner.as_ptr()
            && self.original.actual_owner.get() == original.owner().as_ptr() as usize
            && self.original.actual_class.get() == original.actual_type()?.as_ptr() as usize
            && super::slots::matches_construction_owner(
                &self.invocation_owner,
                &self.plan,
                &self.original,
            )?)
    }

    pub(crate) fn protected_names(&self) -> impl Iterator<Item = &str> {
        self.plan.members.iter().map(|member| member.name.as_str())
    }

    pub(crate) fn namespace(&self) -> DataclassNamespace<'py> {
        DataclassNamespace {
            plan: Arc::clone(&self.plan),
            owner: self.invocation_owner.clone(),
        }
    }

    pub(crate) fn attach(&self, references: &mut Vec<Py<PyAny>>) -> DataclassClassState {
        let reference = references.len();
        references.push(self.invocation_owner.clone().unbind());
        DataclassClassState {
            plan: Arc::clone(&self.plan),
            reference,
        }
    }
}

/// Rust-only class payload. Its one traversed edge starts as the active
/// invocation owner and is replaced, once, by weak permanent member witnesses.
pub(crate) struct DataclassClassState {
    plan: Arc<ClassPlan>,
    pub(crate) reference: usize,
}

impl DataclassClassState {
    pub(crate) fn namespace<'py>(&self, owner: Bound<'py, PyAny>) -> DataclassNamespace<'py> {
        DataclassNamespace {
            plan: Arc::clone(&self.plan),
            owner,
        }
    }

    pub(crate) fn pending(&self) -> bool {
        self.plan.phase.get() != Phase::Complete
    }

    pub(crate) fn fail(&self) {
        self.plan.fail();
    }
}

/// A pinned validation view, usable both before native Ready and after Apply.
/// Source methods still follow their existing independent namespace checks.
pub(crate) struct DataclassNamespace<'py> {
    pub(super) plan: Arc<ClassPlan>,
    pub(super) owner: Bound<'py, PyAny>,
}

impl<'py> DataclassNamespace<'py> {
    /// The caller supplies an authenticated actual constructed type. This
    /// exposes only its existing transient invocation metadata, never a type
    /// recovered from a stored address or another invocation's namespace.
    pub(crate) fn owner_for_native_apply(
        &self,
        actual: &StrictClassState<'py>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        if !actual.is_interpreter_construction()
            || !actual.matches_active_dataclass_owner(&self.owner)?
            || actual.source() != &self.plan.fact.identity
            || !Arc::ptr_eq(actual.namespace_execution(), &self.plan.namespace)
            || self.plan.actual_owner.get() != actual.owner().as_ptr() as usize
            || self.plan.actual_class.get() != actual.actual_type()?.as_ptr() as usize
            || !super::invocation::matches_bound_owner(&self.owner, &self.plan)?
        {
            return Ok(None);
        }
        Ok(Some(self.owner.clone()))
    }

    pub(crate) fn matches(
        &self,
        fact: &ClassTypeFact,
        digest: Fingerprint,
        execution: &Arc<NamespaceExecution>,
    ) -> bool {
        self.plan.source_digest == digest && self.plan.matches_source(fact, execution)
    }

    pub(crate) fn generated_method(&self, name: &str) -> bool {
        self.plan.members.iter().any(|member| member.name == name)
    }

    pub(crate) fn generated_attribute_hook(&self, name: &Bound<'_, PyAny>) -> bool {
        self.has_adopted_member_shape()
            && self.plan.members.iter().any(|member| {
                matches!(
                    member.kind,
                    MemberKind::Generated(
                        GeneratedRole::FrozenSetattr | GeneratedRole::FrozenDelattr
                    )
                ) && unsafe { text_is(name.as_ptr(), &member.name) }
            })
    }

    pub(crate) fn generated_descriptor(
        &self,
        name: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> bool {
        self.has_adopted_member_shape()
            && unsafe { ffi::PyFunction_Check(value.as_ptr()) } != 0
            && self.plan.members.iter().any(|member| {
                (unsafe { text_is(name.as_ptr(), &member.name) })
                    && (self.plan.phase.get() == Phase::Complete
                        || matches!(member.kind, MemberKind::Shared(_))
                        || unsafe { PyFunction_GetSoacStrictId(value.as_ptr()) } != 0)
            })
    }

    fn has_adopted_member_shape(&self) -> bool {
        self.plan.phase.get() == Phase::Complete
            || (self.plan.phase.get() == Phase::Prepared && self.plan.replacement_of.is_some())
    }

    pub(crate) fn validate(
        &self,
        namespace: &Bound<'py, PyDict>,
        phase: ClassNamespacePhase,
    ) -> PyResult<bool> {
        let py = namespace.py();
        match phase {
            ClassNamespacePhase::Input | ClassNamespacePhase::Copied => {
                if self.plan.phase.get() != Phase::Prepared {
                    return Ok(false);
                }
                if self.plan.replacement_of.is_some() {
                    return super::slots::validate_copied_namespace(
                        &self.owner,
                        &self.plan,
                        namespace,
                    );
                }
                if !super::invocation::matches_construction_owner(&self.owner, &self.plan)? {
                    return Ok(false);
                }
                // These entries are the ones this actual transform is meant
                // to create. Source-defined overrides were excluded from the
                // plan; an injected value cannot count as a generated method.
                Ok(self.plan.members.iter().all(|member| unsafe {
                    dictionary_value(namespace.as_ptr(), &member.name).is_none()
                }))
            }
            ClassNamespacePhase::BeforeTransform => {
                // The caller separately proves this is the actual original
                // type's namespace and active invocation edge. No stored type
                // address is dereferenced or treated as lifetime support here.
                if !super::invocation::matches_bound_owner(&self.owner, &self.plan)? {
                    return Ok(false);
                }
                Ok(self.plan.members.iter().all(|member| unsafe {
                    dictionary_value(namespace.as_ptr(), &member.name).is_none()
                }))
            }
            ClassNamespacePhase::Adopted => {
                if self.plan.phase.get() != Phase::Complete {
                    return Ok(false);
                }
                let owner = StrictStateRef::<AdoptedMembers>::from_owner(self.owner.clone())?;
                if !Arc::ptr_eq(&self.plan, &owner.data().plan) {
                    return Ok(false);
                }
                let Some(witnesses) = self.plan.completed_members.get() else {
                    return Ok(false);
                };
                for (member, witness) in self.plan.members.iter().zip(witnesses) {
                    let Some(actual) =
                        (unsafe { dictionary_value(namespace.as_ptr(), &member.name) })
                    else {
                        return Ok(false);
                    };
                    if !witness.function.matches(py, &owner, actual)? {
                        return Ok(false);
                    }
                    if let Some(id) = witness.sealed_id {
                        match unsafe { PyFunction_HasSoacDataclassCreation(actual) } {
                            1 => {}
                            0 => return Ok(false),
                            _ => return Err(PyErr::fetch(py)),
                        }
                        if unsafe { PyFunction_GetSoacStrictId(actual) } != id {
                            return Ok(false);
                        }
                    }
                }
                Ok(true)
            }
        }
    }

    pub(crate) fn bind_class(
        &self,
        class: &Bound<'py, PyAny>,
        class_owner: &Bound<'py, PyAny>,
    ) -> PyResult<()> {
        if self.plan.replacement_of.is_some() {
            super::slots::bind_class(&self.owner, &self.plan, class, class_owner)
        } else {
            super::invocation::bind_class(&self.owner, &self.plan, class, class_owner)
        }
    }

    /// Allocation can run callbacks, so native provenance is checked again
    /// after all weak witnesses are prepared. Staging/commit stays separate so
    /// a replacement cannot finish one class while the other is unvalidated.
    pub(super) fn prepare_members(
        &self,
        class: &Bound<'py, PyAny>,
        namespace: &Bound<'py, PyDict>,
    ) -> PyResult<PreparedMembers<'py>> {
        let py = class.py();
        if self.plan.phase.get() != Phase::Bound
            || self.plan.actual_class.get() != class.as_ptr() as usize
        {
            return Err(strict_runtime_unavailable(
                py,
                "dataclass application has no matching bound class",
            ));
        }
        super::invocation::validate_completed_members(&self.owner, &self.plan, class, namespace)?;
        let mut references = Vec::with_capacity(self.plan.members.len());
        let mut witnesses = Vec::with_capacity(self.plan.members.len());
        for member in &self.plan.members {
            let function = unsafe {
                dictionary_value(namespace.as_ptr(), &member.name)
                    .map(|value| Bound::<PyAny>::from_borrowed_ptr(py, value))
            }
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "generated dataclass member disappeared")
            })?;
            let sealed_id = match member.kind {
                MemberKind::Generated(_) => {
                    let id = unsafe { PyFunction_GetSoacStrictId(function.as_ptr()) };
                    if id == 0 {
                        return Err(strict_runtime_unavailable(
                            py,
                            "generated dataclass member is not sealed",
                        ));
                    }
                    Some(id)
                }
                MemberKind::Shared(_) => None,
            };
            witnesses.push(MemberWitness {
                function: WeakIdentity::capture(py, &function, &mut references)?,
                sealed_id,
            });
        }
        let owner = StrictStateRef::new(
            py,
            AdoptedMembers {
                plan: Arc::clone(&self.plan),
            },
            references.into_iter().map(Bound::unbind).collect(),
        )?;
        super::invocation::validate_completed_members(&self.owner, &self.plan, class, namespace)?;
        Ok(PreparedMembers {
            adopted: DataclassAdoptedMembers {
                plan: Arc::clone(&self.plan),
                owner: owner.owner().clone(),
            },
            witnesses,
        })
    }
}

/// Retained Apply keeps its existing owned original/result operands through
/// publication. Only the selected result receives permanent member metadata;
/// admitting both would bind the shared declarations to the provisional type.
pub(crate) fn complete_application<'py>(
    owner: &Bound<'py, PyAny>,
    original: &Bound<'py, PyAny>,
    result: &Bound<'py, PyAny>,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    let py = owner.py();
    let invocation = super::invocation::Owner::from_owner(owner.clone())?;
    let original_plan = invocation.data().plan.get().ok_or_else(|| {
        strict_runtime_unavailable(py, "dataclass completion has no construction")
    })?;
    if !super::protocol::matches_retained_pending_class(&invocation, original_plan, original)? {
        return Err(strict_runtime_unavailable(
            py,
            "retained dataclass completion lost its actual pending original",
        ));
    }
    let (classes, expected_plan) = if invocation.data().options.slots {
        if !super::slots::matches_result(&invocation, result)? {
            return Err(strict_runtime_unavailable(
                py,
                "dataclass completion lost its replacement operand",
            ));
        }
        let replacement = invocation.data().replacement.get().ok_or_else(|| {
            strict_runtime_unavailable(py, "dataclass completion has no replacement plan")
        })?;
        (vec![result.clone(), original.clone()], &replacement.plan)
    } else {
        if original.as_ptr() != result.as_ptr() {
            return Err(strict_runtime_unavailable(
                py,
                "dictionary dataclass completion changed its operand",
            ));
        }
        (vec![original.clone()], original_plan)
    };
    complete_selected_application(
        owner,
        &invocation,
        original_plan,
        expected_plan,
        result,
        false,
    )?;
    Ok(classes)
}

/// Complete the native consuming Apply handoff for its *selected live result*.
/// The original slots operand is not recovered, weak-upgraded, or kept alive:
/// its native pending lineage is disposed separately at the final source Store.
/// This publishes generated-member metadata only, never a type contract.
pub(crate) fn complete_native_application<'py>(
    owner: &Bound<'py, PyAny>,
    result: &Bound<'py, PyAny>,
) -> PyResult<()> {
    let py = owner.py();
    let invocation = super::invocation::Owner::from_owner(owner.clone())?;
    let original_plan = invocation.data().plan.get().ok_or_else(|| {
        strict_runtime_unavailable(py, "native dataclass completion has no construction")
    })?;
    if !matches!(
        &invocation.data().source_globals,
        super::invocation::SourceGlobals::Interpreter { .. }
    ) || invocation.data().phase.get() != super::invocation::Phase::Applying
        || original_plan.phase.get() != Phase::Bound
    {
        return Err(strict_runtime_unavailable(
            py,
            "native dataclass completion has no active pending construction",
        ));
    }
    let expected_plan = if invocation.data().options.slots {
        if !super::slots::matches_result(&invocation, result)? {
            return Err(strict_runtime_unavailable(
                py,
                "native dataclass completion lost its selected replacement",
            ));
        }
        &invocation
            .data()
            .replacement
            .get()
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "native slots completion has no replacement plan")
            })?
            .plan
    } else {
        if original_plan.actual_class.get() != result.as_ptr() as usize {
            return Err(strict_runtime_unavailable(
                py,
                "native dictionary dataclass changed its selected operand",
            ));
        }
        original_plan
    };
    complete_selected_application(
        owner,
        &invocation,
        original_plan,
        expected_plan,
        result,
        true,
    )
}

/// The caller has supplied its backend's exact actual operand proof. This
/// common tail validates the same final state/plan and moves only the selected
/// class's permanent member edge before retiring the active adapter graph.
fn complete_selected_application<'py>(
    owner: &Bound<'py, PyAny>,
    invocation: &super::invocation::Owner<'py>,
    original_plan: &Arc<ClassPlan>,
    expected_plan: &Arc<ClassPlan>,
    result: &Bound<'py, PyAny>,
    interpreter: bool,
) -> PyResult<()> {
    let py = owner.py();
    if invocation.data().phase.get() != super::invocation::Phase::Applying
        || !invocation
            .data()
            .plan
            .get()
            .is_some_and(|plan| Arc::ptr_eq(plan, original_plan))
    {
        return Err(strict_runtime_unavailable(
            py,
            "selected dataclass publication lost its invocation",
        ));
    }
    let state = crate::strict_class_state::for_constructed_type(py, result)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "dataclass result has no actual construction owner")
    })?;
    if state.is_interpreter_construction() != interpreter
        || !state.is_pending_type()
        || !state.matches_active_dataclass_owner(owner)?
    {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass result has no matching pending application",
        ));
    }
    let namespace = state
        .dataclass_namespace()?
        .ok_or_else(|| strict_runtime_unavailable(py, "dataclass result has no namespace proof"))?;
    if namespace.owner.as_ptr() != owner.as_ptr()
        || !Arc::ptr_eq(&namespace.plan, expected_plan)
        || expected_plan.actual_owner.get() != state.owner().as_ptr() as usize
    {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass selected result belongs to another invocation",
        ));
    }
    // for_constructed_type validated this actual supported type. Do not cast
    // the retired original's comparison-only address to recover a dictionary.
    let dictionary = unsafe { (*result.as_ptr().cast::<ffi::PyTypeObject>()).tp_dict };
    if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass selected result has no exact namespace",
        ));
    }
    let dictionary =
        unsafe { Bound::<PyAny>::from_borrowed_ptr(py, dictionary) }.cast_into::<PyDict>()?;
    let prepared = namespace.prepare_members(result, &dictionary)?;
    super::invocation::validate_completed_members(owner, &namespace.plan, result, &dictionary)?;
    let adopted = prepared.stage()?;
    // The native Complete operation retires the existing active adapter graph.
    // Only this selected result gets a permanent member-witness edge; neither
    // this commit nor publish_dataclass_members admits instances.
    super::invocation::commit(owner, original_plan)?;
    let published = state.publish_dataclass_members(result, owner, &adopted);
    let cleared = super::invocation::finish_publication(owner);
    published.and(cleared)
}
