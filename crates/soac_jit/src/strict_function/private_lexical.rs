//! Private-only lexical cells for source functions with no native capture.
//!
//! Public closure projections are deliberately absent here: an active source
//! frame copies those after ordinary argument binding, including legitimate
//! pre-seal closure replacement. All persistent edges below are GC-visible.

use super::*;
use soac_core::block_py::{LexicalCellBinding, PrivateLexicalScope};

pub(super) enum PrivateCaptureState {
    Absent,
    Uninstalled(PrivateLexicalScope),
    Ready(Vec<usize>),
}

impl PrivateCaptureState {
    pub(super) fn initial(scope: &soac_core::block_py::CallableScopeInfo) -> Self {
        if !scope
            .source_origin
            .as_ref()
            .is_some_and(|origin| origin.role == CallableSourceRole::SourceFunction)
        {
            return Self::Absent;
        }
        scope
            .private_lexical
            .as_ref()
            .filter(|scope| scope.private_captures().next().is_some())
            .cloned()
            .map_or(Self::Absent, Self::Uninstalled)
    }
}

pub(crate) fn required_leaf<'a>(
    facts: &'a soac_contracts::ModuleTypeFacts,
    index: u32,
    binding: &LexicalCellBinding,
) -> Option<&'a NominalBindingFact> {
    let leaf = facts.nominal_bindings.get(index as usize)?;
    if leaf.name != binding.name || leaf.binding_scope != binding.scope {
        return None;
    }
    let soac_contracts::NominalBindingOwner::Field { field } = &leaf.owner else {
        return None;
    };
    let class = facts.classes.iter().find(|class| {
        class.identity == field.declaring_class.definition
            && class.participation == soac_contracts::ParticipationProposal::Candidate
    })?;
    class
        .required_field_bindings(&facts.language_policy)
        .into_iter()
        .any(|candidate| candidate.annotation_reference().as_ref() == Some(field))
        .then_some(leaf)
}

pub(crate) struct PreparedPrivateLexicalCaptures<'py> {
    projection: PrivateLexicalScope,
    function: RuntimeFunctionId,
    definition: SourceIdentity,
    shared: usize,
    template: usize,
    cells: Vec<Bound<'py, PyAny>>,
    creation: Option<Arc<crate::strict_namespace::NamespaceExecution>>,
}

pub(crate) fn prepare_private_lexical_captures<'py>(
    py: Python<'py>,
    shared: &Arc<SharedModuleState>,
    template: &Arc<FunctionInstantiationTemplate>,
    active: Option<&StrictFunctionCall>,
    cells: &[Bound<'_, PyAny>],
) -> PyResult<Option<PreparedPrivateLexicalCaptures<'py>>> {
    let function = template.function();
    let scope = function.scope.private_lexical.as_ref().filter(|scope| {
        scope.private_captures().next().is_some()
            && function
                .scope
                .source_origin
                .as_ref()
                .is_some_and(|origin| origin.role == CallableSourceRole::SourceFunction)
    });
    let Some(scope) = scope else {
        if !cells.is_empty() {
            return Err(strict_runtime_unavailable(
                py,
                "unexpected source-function private cells",
            ));
        }
        return Ok(None);
    };
    let active = active.ok_or_else(|| {
        strict_runtime_unavailable(py, "private lexical creation has no active source frame")
    })?;
    let producer = active.captured_owner(py)?;
    if producer.source() != Some(&scope.creator)
        || producer.data().function_identity != active.function() as usize
        || !active
            .active_module_state()
            .is_some_and(|actual| Arc::ptr_eq(actual, shared))
        || !shared.admits_function(function)
        || scope.private_captures().count() != cells.len()
        || cells
            .iter()
            .any(|cell| !crate::function_instantiation::is_cell_object(cell.as_ptr()))
    {
        return Err(strict_runtime_unavailable(
            py,
            "private lexical creation changed its source/cell projection",
        ));
    }
    let verified = shared.verified_strict_module().ok_or_else(|| {
        strict_runtime_unavailable(py, "private lexical creation lacks verified source")
    })?;
    let mut seen = BTreeSet::new();
    let mut original = Vec::with_capacity(cells.len());
    for (capture, cell) in scope.private_captures().zip(cells) {
        if capture.nominal_binding_indices.is_empty() || !seen.insert(capture.binding.clone()) {
            return Err(strict_runtime_unavailable(
                py,
                "private lexical projection is empty or repeated",
            ));
        }
        for &index in &capture.nominal_binding_indices {
            required_leaf(verified.type_facts().facts(), index, &capture.binding).ok_or_else(
                || {
                    strict_runtime_unavailable(
                        py,
                        "private lexical cell has no selected signed field consumer",
                    )
                },
            )?;
        }
        // Only identity is captured. No Python callback or contents read.
        original.push(unsafe { Bound::from_borrowed_ptr(py, cell.as_ptr()) });
    }
    Ok(Some(PreparedPrivateLexicalCaptures {
        projection: scope.clone(),
        function: function.function_id,
        definition: function
            .scope
            .source_origin
            .as_ref()
            .expect("source origin")
            .definition
            .clone(),
        shared: Arc::as_ptr(shared) as usize,
        template: Arc::as_ptr(template) as usize,
        cells: original,
        creation: active.environment().namespace_execution.clone(),
    }))
}

pub(crate) struct PrivateCaptureInstallationGuard<'py> {
    owner: StrictStateRef<'py, StrictFunctionData>,
    committed: bool,
}

impl PrivateCaptureInstallationGuard<'_> {
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for PrivateCaptureInstallationGuard<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // This private guard is minted only after the fallible SOAC check.
        // Teardown must not allocate an exception or touch pending PyErr.
        let StrictFunctionImplementation::Soac(implementation) = &self.owner.data().implementation
        else {
            return;
        };
        let state = std::mem::replace(
            &mut *implementation.private_captures.borrow_mut(),
            PrivateCaptureState::Absent,
        );
        if let PrivateCaptureState::Ready(indices) = state {
            let py = self.owner.owner().py();
            for index in indices {
                let _ = self.owner.set_reference(index, py.None().into_bound(py));
            }
        }
    }
}

impl<'py> PreparedPrivateLexicalCaptures<'py> {
    pub(crate) fn install(
        self,
        function: &Bound<'py, PyAny>,
    ) -> PyResult<PrivateCaptureInstallationGuard<'py>> {
        let py = function.py();
        let auth = authenticate_strict_function(py, function)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "created source function lacks a native owner")
        })?;
        if auth.function_id()? != self.function
            || auth.soac_implementation()?.shared_state_identity != self.shared
            || auth.soac_implementation()?.template_identity != self.template
            || auth.origin().is_none_or(|origin| {
                origin.role != CallableSourceRole::SourceFunction
                    || origin.definition != self.definition
            })
            || !class_construction::same_creation(auth.creation_execution(), self.creation.as_ref())
            || !matches!(&*auth.soac_implementation()?.private_captures.borrow(), PrivateCaptureState::Uninstalled(scope) if scope == &self.projection)
        {
            return Err(strict_runtime_unavailable(
                py,
                "created function changed its private lexical installation identity",
            ));
        }
        let mut indices = Vec::with_capacity(self.cells.len());
        // GC-vector appends and Ready publication do not execute Python.
        for cell in self.cells {
            indices.push(auth.add_reference(cell)?);
        }
        *auth.soac_implementation()?.private_captures.borrow_mut() =
            PrivateCaptureState::Ready(indices);
        Ok(PrivateCaptureInstallationGuard {
            owner: auth.owner,
            committed: false,
        })
    }
}

pub(super) fn active_private_cells<'py>(
    owner: &StrictStateRef<'py, StrictFunctionData>,
    expected: usize,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    let indices = match &*owner.soac_implementation()?.private_captures.borrow() {
        PrivateCaptureState::Absent if expected == 0 => return Ok(Vec::new()),
        PrivateCaptureState::Ready(indices) if indices.len() == expected => indices.clone(),
        _ => {
            return Err(strict_runtime_unavailable(
                owner.owner().py(),
                "source function private cells are not installed",
            ));
        }
    };
    indices
        .into_iter()
        .map(|index| owner.reference(index))
        .collect()
}
