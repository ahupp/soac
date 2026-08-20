//! Mandatory write policies, independent of normalized logical layouts.
//!
//! Two executions of a source factory can require distinct actual targets.
//! Storage owners inherit these policies by identity; each policy owns only
//! the field-binding snapshots needed to check its own selected writes.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CStr;
use std::ptr::NonNull;
use std::sync::Arc;

use pyo3::exceptions::PyTypeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use soac_contracts::{
    AnnotationOrigin, CheckedFieldPolicy, ClassReference, ClassTypeFact, DynamicClassReason,
    FieldTypeFact, StaticType,
};
use soac_core::block_py::CallableSourceRole;

use crate::strict_field_bindings::{StrictFieldBinding, field_reference, nominal_classes};
use crate::strict_function::AuthenticatedStrictFunction;
use crate::strict_namespace::NamespaceExecution;
use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{StrictNominalTypeResolver, strict_runtime_unavailable, strict_value_guard};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    Bound,
    Terminal,
}

struct FieldCheck {
    value_type: StaticType,
    binding: Option<usize>,
}

/// Identity of one actual declaring predicate, shared only with its storage
/// projections. This owns no Python object and grants no class/source authority.
/// Equal source facts from separate factory executions get different identities.
struct ActualFieldCheck;

struct FieldCheckData {
    interpreter: i64,
    phase: Cell<Phase>,
    actual: Arc<ActualFieldCheck>,
    checks: BTreeMap<String, FieldCheck>,
}

// SAFETY: All Python edges are indices into the native owner's GC vector.
unsafe impl StrictStateData for FieldCheckData {
    const TYPE_NAME: &'static CStr = c"soac._StrictFieldChecks";

    fn on_terminal(&self) {
        self.phase.set(Phase::Terminal);
    }
}

pub(crate) struct StrictFieldChecks<'py> {
    state: StrictStateRef<'py, FieldCheckData>,
}

impl<'py> StrictFieldChecks<'py> {
    pub(crate) fn owner(&self) -> &Bound<'py, PyAny> {
        self.state.owner()
    }

    /// Scalar selection only, not a successful value check or instance grant.
    pub(crate) fn contains_field(&self, name: &str) -> bool {
        self.state.data().checks.contains_key(name)
    }

    pub(crate) fn from_owner(owner: Bound<'py, PyAny>) -> PyResult<Self> {
        Ok(Self {
            state: StrictStateRef::from_owner(owner)?,
        })
    }

    /// Compare an original actual predicate with one of its projections. This
    /// is not structural type equality and does not by itself prove coverage.
    pub(crate) fn same_actual_check(&self, other: &Self) -> PyResult<bool> {
        self.ensure_live()?;
        other.ensure_live()?;
        Ok(Arc::ptr_eq(
            &self.state.data().actual,
            &other.state.data().actual,
        ))
    }

    /// Copy only already-bound selected predicates and their necessary nominal
    /// binding owners. In particular, an escaped dictionary must not retain a
    /// receiver or unrelated native-slot targets through the original owner.
    pub(crate) fn project_fields(&self, fields: &BTreeSet<String>) -> PyResult<Option<Self>> {
        self.ensure_live()?;
        let data = self.state.data();
        let py = self.owner().py();
        if data.phase.get() != Phase::Bound {
            return Err(strict_runtime_unavailable(
                py,
                "storage projection preceded actual field binding",
            ));
        }
        if fields.iter().any(|name| !data.checks.contains_key(name)) {
            return Err(strict_runtime_unavailable(
                py,
                "storage projection requested an absent field predicate",
            ));
        }
        if fields.is_empty() {
            return Ok(None);
        }
        if fields.len() == data.checks.len() {
            return Self::from_owner(self.owner().clone()).map(Some);
        }
        let mut checks = BTreeMap::new();
        let mut references = Vec::new();
        let mut bindings = BTreeMap::new();
        for name in fields {
            let check = &data.checks[name];
            let binding = if let Some(original) = check.binding {
                Some(match bindings.get(&original) {
                    Some(&index) => index,
                    None => {
                        let binding = self.state.reference(original)?;
                        if !StrictFieldBinding::from_owner(binding.clone())?.is_bound()? {
                            return Err(strict_runtime_unavailable(
                                py,
                                "storage projection lost its actual nominal binding",
                            ));
                        }
                        let index = references.len();
                        references.push(binding.unbind());
                        bindings.insert(original, index);
                        index
                    }
                })
            } else {
                None
            };
            checks.insert(
                name.clone(),
                FieldCheck {
                    value_type: check.value_type.clone(),
                    binding,
                },
            );
        }
        let projected = Self {
            state: StrictStateRef::new(
                py,
                FieldCheckData {
                    interpreter: data.interpreter,
                    phase: Cell::new(Phase::Bound),
                    actual: Arc::clone(&data.actual),
                    checks,
                },
                references,
            )?,
        };
        // Creating the GC shell may collect/reenter. Never publish a projection
        // from metadata that became terminal during its allocation.
        self.ensure_live()?;
        Ok(Some(projected))
    }

    fn ensure_live(&self) -> PyResult<()> {
        self.state.ensure_live()?;
        let interpreter = unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
        if interpreter < 0 {
            return Err(PyErr::fetch(self.owner().py()));
        }
        if interpreter != self.state.data().interpreter {
            return Err(strict_runtime_unavailable(
                self.owner().py(),
                "field policy belongs to another interpreter",
            ));
        }
        Ok(())
    }

    /// Declaring snapshots were bound by the class's pre-Ready callback first.
    /// This transition activates writes but never rebinds a nominal target.
    pub(crate) fn bind_actual_class(&self, actual: &Bound<'py, PyAny>) -> PyResult<()> {
        self.ensure_live()?;
        if self.state.data().phase.get() != Phase::Prepared
            || unsafe { ffi::PyType_Check(actual.as_ptr()) } == 0
        {
            return Err(strict_runtime_unavailable(
                actual.py(),
                "field policy class binding was replayed or invalid",
            ));
        }
        for check in self.state.data().checks.values() {
            if let Some(index) = check.binding
                && !StrictFieldBinding::from_owner(self.state.reference(index)?)?.is_bound()?
            {
                return Err(strict_runtime_unavailable(
                    actual.py(),
                    "field policy preceded its declaring snapshot binding",
                ));
            }
        }
        self.state.data().phase.set(Phase::Bound);
        Ok(())
    }

    pub(crate) fn check(&self, name: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.ensure_live()?;
        let Some(check) = self.state.data().checks.get(name) else {
            return Ok(());
        };
        if self.state.data().phase.get() != Phase::Bound {
            return Err(strict_runtime_unavailable(
                value.py(),
                "field write preceded its native class binding",
            ));
        }
        let binding = check
            .binding
            .map(|index| StrictFieldBinding::from_owner(self.state.reference(index)?))
            .transpose()?;
        if strict_value_guard(value, &check.value_type, &FieldResolver(binding.as_ref()))?.is_none()
        {
            return Err(PyTypeError::new_err(format!(
                "strict field value contract rejected {name}"
            )));
        }
        Ok(())
    }
}

struct FieldResolver<'a, 'py>(Option<&'a StrictFieldBinding<'py>>);

// SAFETY: An optional declaration-bound resolver is the only nominal source;
// absence never fabricates a target or waives a selected nominal predicate.
unsafe impl StrictNominalTypeResolver for FieldResolver<'_, '_> {
    fn visit_targets(
        &self,
        py: Python<'_>,
        class: &ClassReference,
        visitor: &mut dyn FnMut(NonNull<ffi::PyTypeObject>),
    ) -> PyResult<bool> {
        match self.0 {
            Some(binding) => binding.visit_targets(py, class, visitor),
            None => Ok(false),
        }
    }
}

/// Logical selection only. Actual nominal snapshots are independently
/// required before a selected write predicate can be installed.
pub(crate) fn selected_field_contract(
    policy: CheckedFieldPolicy,
    origin: AnnotationOrigin,
    value_type: &StaticType,
) -> Option<StaticType> {
    policy.required_type(origin, value_type).cloned()
}

pub(crate) fn own_checked_fields(
    auth: &AuthenticatedStrictFunction<'_, '_>,
    fact: &ClassTypeFact,
) -> Vec<FieldTypeFact> {
    let verified = auth.verified_module();
    let facts = verified.type_facts().facts();
    fact.instance_fields
        .iter()
        .filter(|field| {
            field.declaring_class.definition == fact.identity
                && field.declaring_class.source_digest == facts.source_digest
                && field
                    .required_write_type(facts.language_policy.checked_fields)
                    .is_some()
        })
        .cloned()
        .collect()
}

/// Build the *own* write contract from the already captured own declarations.
/// Inherited policies keep their original owner and never enter this binder.
pub(crate) fn prepare_field_checks<'py>(
    auth: &AuthenticatedStrictFunction<'_, 'py>,
    fact: &ClassTypeFact,
    namespace: &Bound<'py, PyDict>,
    execution: &Arc<NamespaceExecution>,
    bindings: &[StrictFieldBinding<'py>],
) -> PyResult<Result<Option<StrictFieldChecks<'py>>, DynamicClassReason>> {
    let py = namespace.py();
    let verified = auth.verified_module();
    let facts = verified.type_facts().facts();
    if !execution.is_completed()
        || execution.source() != &fact.identity
        || !auth.origin().is_some_and(|origin| {
            origin.role == CallableSourceRole::ClassNamespace && origin.definition == fact.identity
        })
        || !facts.classes.iter().any(|expected| expected == fact)
    {
        return Err(strict_runtime_unavailable(
            py,
            "field checks have no authenticated class execution",
        ));
    }
    let selected = own_checked_fields(auth, fact);
    if selected.is_empty() {
        return Ok(Ok(None));
    }
    let mut checks = BTreeMap::new();
    let mut references = Vec::new();
    for field in selected {
        let mut classes = BTreeSet::new();
        nominal_classes(&field.value_type, &mut classes);
        let binding = if classes.is_empty() {
            None
        } else {
            let Some(source) = field_reference(&field) else {
                return Ok(Err(DynamicClassReason::UnresolvedAnalysis));
            };
            let Some(binding) = bindings.iter().find(|binding| binding.field() == &source) else {
                return Ok(Err(DynamicClassReason::UnresolvedAnalysis));
            };
            if binding.value_type() != &field.value_type {
                return Err(strict_runtime_unavailable(
                    py,
                    "field write predicate differs from its declaring snapshot",
                ));
            }
            let index = references.len();
            references.push(binding.owner().clone());
            Some(index)
        };
        checks.insert(
            field.name,
            FieldCheck {
                value_type: field.value_type,
                binding,
            },
        );
    }
    let state = StrictStateRef::new(
        py,
        FieldCheckData {
            interpreter: verified.interpreter_id(),
            phase: Cell::new(Phase::Prepared),
            actual: Arc::new(ActualFieldCheck),
            checks,
        },
        references.into_iter().map(Bound::unbind).collect(),
    )?;
    Ok(Ok(Some(StrictFieldChecks { state })))
}

#[cfg(test)]
impl<'py> StrictFieldChecks<'py> {
    /// Native write-kernel fixtures have no nominal/source authority.
    pub(crate) fn builtin_fixture(
        py: Python<'py>,
        checks: BTreeMap<String, StaticType>,
    ) -> PyResult<Self> {
        let checks = checks
            .into_iter()
            .map(|(name, value_type)| {
                let mut classes = BTreeSet::new();
                nominal_classes(&value_type, &mut classes);
                assert!(
                    classes.is_empty(),
                    "a fixture cannot authenticate nominal field targets"
                );
                (
                    name,
                    FieldCheck {
                        value_type,
                        binding: None,
                    },
                )
            })
            .collect();
        Ok(Self {
            state: StrictStateRef::new(
                py,
                FieldCheckData {
                    interpreter: unsafe {
                        ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get())
                    },
                    phase: Cell::new(Phase::Bound),
                    actual: Arc::new(ActualFieldCheck),
                    checks,
                },
                Vec::new(),
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_contracts::BuiltinType;

    #[test]
    fn storage_projection_keeps_actual_identity_without_a_full_owner_edge() -> PyResult<()> {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let make = || {
                StrictFieldChecks::builtin_fixture(
                    py,
                    BTreeMap::from([
                        (
                            "dictionary".into(),
                            StaticType::NominalBuiltin {
                                builtin: BuiltinType::Int,
                                allow_subclasses: true,
                            },
                        ),
                        (
                            "slot".into(),
                            StaticType::NominalBuiltin {
                                builtin: BuiltinType::Str,
                                allow_subclasses: true,
                            },
                        ),
                    ]),
                )
            };
            let original = make()?;
            let other = make()?;
            let fields = BTreeSet::from(["dictionary".into()]);
            let projected = original.project_fields(&fields)?.unwrap();
            assert_ne!(projected.owner().as_ptr(), original.owner().as_ptr());
            assert!(projected.same_actual_check(&original)?);
            assert!(!projected.same_actual_check(&other)?);
            assert!(projected.contains_field("dictionary"));
            assert!(!projected.contains_field("slot"));
            // Primitive-only projections own no Python references at all;
            // neither the original/full owner nor a receiver is retained.
            assert!(projected.state.reference(0).is_err());
            assert!(original.project_fields(&BTreeSet::new())?.is_none());
            assert!(
                original
                    .project_fields(&BTreeSet::from(["absent".into()]))
                    .is_err()
            );
            Ok(())
        })
    }

    #[test]
    fn storage_projection_requires_bound_original_predicates() -> PyResult<()> {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let check = StrictFieldChecks::builtin_fixture(
                py,
                BTreeMap::from([(
                    "field".into(),
                    StaticType::NominalBuiltin {
                        builtin: BuiltinType::Int,
                        allow_subclasses: true,
                    },
                )]),
            )?;
            check.state.data().phase.set(Phase::Prepared);
            assert!(
                check
                    .project_fields(&BTreeSet::from(["field".into()]))
                    .is_err()
            );
            Ok(())
        })
    }
}
