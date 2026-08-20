//! Minimal actual nominal bindings for selected dataclass field writes.
//!
//! Generated function signatures do not select runtime value constraints.
//! Inherited fields retain their existing declaring storage owners.

use std::collections::BTreeSet;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyDict;
use soac_contracts::{ClassTypeFact, FieldTypeFact};

use crate::strict_field_bindings::{
    StrictFieldBinding, nominal_classes, prepare_own_field_bindings,
};
use crate::strict_function::{AuthenticatedStrictFunction, ClassConstructionCaptures};
use crate::strict_namespace::NamespaceExecution;

use super::invocation::Owner;

pub(super) struct PreparedBindings<'py> {
    own: Vec<StrictFieldBinding<'py>>,
}

fn has_nominal(field: &FieldTypeFact) -> bool {
    let mut classes = BTreeSet::new();
    nominal_classes(&field.value_type, &mut classes);
    !classes.is_empty()
}

impl<'py> PreparedBindings<'py> {
    pub(super) fn prepare(
        auth: &AuthenticatedStrictFunction<'_, 'py>,
        fact: &ClassTypeFact,
        namespace: &Bound<'py, PyDict>,
        execution: &Arc<NamespaceExecution>,
        construction_captures: Option<&ClassConstructionCaptures<'py>>,
    ) -> PyResult<Option<Self>> {
        let policy = &auth.verified_module().type_facts().facts().language_policy;
        let own_fields = fact
            .required_field_bindings(policy)
            .into_iter()
            .filter(|field| has_nominal(field))
            .cloned()
            .collect::<Vec<_>>();
        let own = match prepare_own_field_bindings(
            auth,
            fact,
            namespace,
            execution,
            &own_fields,
            construction_captures,
        )? {
            Ok(bindings) => bindings,
            Err(_) => return Ok(None),
        };
        Ok(Some(Self { own }))
    }

    /// The native class takes these exact minimal owners before Ready. The
    /// temporary invocation drops its copies at Apply completion/failure.
    pub(super) fn publish_own(&self, owner: &Owner<'py>) -> PyResult<Vec<usize>> {
        let mut indices = Vec::with_capacity(self.own.len());
        for binding in &self.own {
            let index = owner.add_reference(binding.owner().clone())?;
            owner.data().active_reference_count.set(index + 1);
            indices.push(index);
        }
        Ok(indices)
    }
}
