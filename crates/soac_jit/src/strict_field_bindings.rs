//! Source-bound nominal targets for one actual field declaration.
//!
//! A selected field-write declaration retains its actual construction-time
//! nominal binding, not a later annotation lookup. InitVar and function
//! signatures do not install runtime value predicates.
//! Each owner retains only its own required target types. It has no receiver,
//! provider, module, default, or class-owner edge; direct self is an intentional
//! traversed type edge bound once before native class callbacks.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CStr;
use std::ptr::NonNull;
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple};
use soac_contracts::{
    AnnotationOrigin, ClassReference, ClassTypeFact, DefinitionKind, DynamicClassReason,
    FieldReference, FieldTypeFact, NominalBindingFact, NominalBindingOwner, StaticType,
};
use soac_core::block_py::CallableSourceRole;

use crate::strict_annotation::validated_annotation_capture_schema;
use crate::strict_function::{
    AuthenticatedStrictFunction, ClassConstructionCaptures, authenticate_strict_function,
};
use crate::strict_namespace::NamespaceExecution;
use crate::strict_nominal::{captured_cell, cell_contents, dictionary_binding};
use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{StrictNominalTypeResolver, strict_runtime_unavailable};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    Bound,
    Terminal,
}

struct BoundLeaf {
    source: NominalBindingFact,
    target: usize,
}

struct BindingData {
    interpreter: i64,
    field: FieldReference,
    value_type: StaticType,
    phase: Cell<Phase>,
    leaves: Vec<BoundLeaf>,
    self_targets: Vec<usize>,
}

// SAFETY: All Python edges are indices into the native owner's GC vector.
unsafe impl StrictStateData for BindingData {
    const TYPE_NAME: &'static CStr = c"soac._StrictFieldBinding";

    fn on_terminal(&self) {
        self.phase.set(Phase::Terminal);
    }
}

pub(crate) struct StrictFieldBinding<'py> {
    state: StrictStateRef<'py, BindingData>,
}

impl<'py> StrictFieldBinding<'py> {
    pub(crate) fn owner(&self) -> &Bound<'py, PyAny> {
        self.state.owner()
    }

    pub(crate) fn from_owner(owner: Bound<'py, PyAny>) -> PyResult<Self> {
        Ok(Self {
            state: StrictStateRef::from_owner(owner)?,
        })
    }

    pub(crate) fn field(&self) -> &FieldReference {
        &self.state.data().field
    }

    pub(crate) fn value_type(&self) -> &StaticType {
        &self.state.data().value_type
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
                "field binding belongs to another interpreter",
            ));
        }
        Ok(())
    }

    pub(crate) fn is_bound(&self) -> PyResult<bool> {
        self.ensure_live()?;
        Ok(self.state.data().phase.get() == Phase::Bound)
    }

    /// Only the original declaring class's native pre-Ready callback may call
    /// this. Successful binding allocates nothing and releases only None.
    pub(crate) fn bind_actual_class(&self, actual: &Bound<'py, PyAny>) -> PyResult<()> {
        self.ensure_live()?;
        if self.state.data().phase.get() != Phase::Prepared
            || unsafe { ffi::PyType_Check(actual.as_ptr()) } == 0
        {
            return Err(strict_runtime_unavailable(
                actual.py(),
                "field binding was replayed or has no actual type",
            ));
        }
        for &slot in &self.state.data().self_targets {
            self.state.bind_reserved_reference(slot, actual.clone())?;
        }
        self.state.data().phase.set(Phase::Bound);
        Ok(())
    }
}

// SAFETY: This resolver is closed over one signed declaration and its actual
// construction. Every leaf is pinned by the GC owner. Native MRO membership is
// checked afresh; no persistent value or receiver-layout proof is granted.
unsafe impl StrictNominalTypeResolver for StrictFieldBinding<'_> {
    fn visit_targets(
        &self,
        _py: Python<'_>,
        class: &ClassReference,
        visitor: &mut dyn FnMut(NonNull<ffi::PyTypeObject>),
    ) -> PyResult<bool> {
        self.ensure_live()?;
        if self.state.data().phase.get() != Phase::Bound {
            return Ok(false);
        }
        let selected = |leaf: &&BoundLeaf| {
            &leaf.source.class == class
                && matches!(&leaf.source.owner, NominalBindingOwner::Field { field } if field == self.field())
        };
        let mut found = false;
        for leaf in self.state.data().leaves.iter().filter(selected) {
            found = true;
            let target = self.state.reference(leaf.target)?;
            if unsafe { ffi::PyType_Check(target.as_ptr()) } == 0 {
                return Ok(false);
            }
        }
        if !found {
            return Ok(false);
        }
        for leaf in self.state.data().leaves.iter().filter(selected) {
            let target = self.state.reference(leaf.target)?;
            visitor(NonNull::new(target.as_ptr().cast()).expect("live field binding"));
        }
        Ok(true)
    }
}

pub(crate) fn field_reference(field: &FieldTypeFact) -> Option<FieldReference> {
    field
        .annotation_definition
        .as_ref()
        .map(|definition| FieldReference {
            declaring_class: field.declaring_class.clone(),
            annotation_definition: definition.clone(),
            name: field.name.clone(),
        })
}

pub(crate) fn nominal_classes(value: &StaticType, result: &mut BTreeSet<ClassReference>) {
    match value {
        StaticType::NominalClass(class) | StaticType::ExactClass(class) => {
            result.insert(class.clone());
        }
        StaticType::Union(alternatives) => {
            for alternative in alternatives {
                nominal_classes(alternative, result);
            }
        }
        StaticType::Optional(alternative) => nominal_classes(alternative, result),
        _ => {}
    }
}

fn class_provider<'a, 'py>(
    auth: &AuthenticatedStrictFunction<'_, 'py>,
    namespace: &'a Bound<'py, PyDict>,
    fact: &ClassTypeFact,
    execution: &Arc<NamespaceExecution>,
) -> PyResult<Option<AuthenticatedStrictFunction<'a, 'py>>> {
    let py = namespace.py();
    // Prepare BOTH keys before any native borrowed function is observed.
    let keys = ["__annotate__", "__annotate_func__"].map(|name| PyString::new(py, name));
    let mut found: Option<AuthenticatedStrictFunction<'a, 'py>> = None;
    for name in &keys {
        let provider = if auth.is_interpreter() {
            let Some(value) = (unsafe {
                crate::strict_nominal::dictionary_binding_ptr(py, namespace.as_any(), name)?
            }) else {
                continue;
            };
            if value.as_ptr() == unsafe { ffi::Py_None() } {
                continue;
            }
            crate::strict_function::authenticate_borrowed_strict_function(py, unsafe {
                Borrowed::from_ptr(py, value.as_ptr())
            })?
        } else {
            let Some(value) = dictionary_binding(py, namespace.as_any(), name)? else {
                continue;
            };
            if value.is_none() {
                continue;
            }
            authenticate_strict_function(py, &value)?
        };
        let Some(provider) = provider else {
            return Ok(None);
        };
        if !provider.same_source_execution(auth)
            || !provider.origin().is_some_and(|origin| {
                origin.role == CallableSourceRole::AnnotationProvider
                    && origin.definition == fact.identity
            })
            || !provider
                .creation_execution()
                .is_some_and(|actual| Arc::ptr_eq(actual, execution))
            || provider.globals()?.as_ptr() != auth.globals()?.as_ptr()
            || found.as_ref().is_some_and(|previous| {
                previous.function().as_ptr() != provider.function().as_ptr()
            })
        {
            return Ok(None);
        }
        found = Some(provider);
    }
    Ok(found)
}

struct PreparedBinding<'py> {
    field: FieldReference,
    value_type: StaticType,
    leaves: Vec<BoundLeaf>,
    targets: Vec<Bound<'py, PyAny>>,
    self_targets: Vec<usize>,
}

/// Snapshot only selected *own* supported declarations. Inherited users must
/// reuse their actual declaring base's earlier snapshot; this function cannot
/// lazily bind an inherited annotation against a new class or changed cell.
pub(crate) fn prepare_own_field_bindings<'py>(
    auth: &AuthenticatedStrictFunction<'_, 'py>,
    fact: &ClassTypeFact,
    namespace: &Bound<'py, PyDict>,
    execution: &Arc<NamespaceExecution>,
    selected: &[FieldTypeFact],
    construction_captures: Option<&ClassConstructionCaptures<'py>>,
) -> PyResult<Result<Vec<StrictFieldBinding<'py>>, DynamicClassReason>> {
    let py = namespace.py();
    let verified = auth.verified_module();
    let facts = verified.type_facts().facts();
    if !execution.is_completed()
        || execution.source() != &fact.identity
        || !auth.origin().is_some_and(|origin| {
            origin.role == CallableSourceRole::ClassNamespace && origin.definition == fact.identity
        })
        || !facts.classes.iter().any(|expected| expected == fact)
        || selected.iter().any(|field| {
            field.declaring_class.definition != fact.identity
                || field.declaring_class.source_digest != facts.source_digest
                || field.annotation_origin != AnnotationOrigin::Explicit
                || !field.value_type.has_supported_value_shape()
                || !fact.instance_fields.contains(field)
        })
    {
        return Err(strict_runtime_unavailable(
            py,
            "field bindings have no authenticated own declaration",
        ));
    }
    let own = ClassReference {
        definition: fact.identity.clone(),
        source_digest: facts.source_digest,
    };
    let mut declarations = Vec::new();
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    for field in selected {
        let mut classes = BTreeSet::new();
        nominal_classes(&field.value_type, &mut classes);
        if classes.is_empty() {
            continue;
        }
        let Some(source) = field_reference(field) else {
            return Ok(Err(DynamicClassReason::UnresolvedAnalysis));
        };
        if !seen.insert(source.clone()) {
            return Err(strict_runtime_unavailable(
                py,
                "duplicate selected field binding",
            ));
        }
        let leaves: Vec<_> = facts
            .nominal_bindings
            .iter()
            .filter(|leaf| {
                matches!(&leaf.owner, NominalBindingOwner::Field { field } if field == &source)
                    && classes.contains(&leaf.class)
            })
            .cloned()
            .collect();
        if classes
            .iter()
            .any(|class| !leaves.iter().any(|leaf| &leaf.class == class))
        {
            return Ok(Err(DynamicClassReason::UnresolvedAnalysis));
        }
        names.push(
            leaves
                .iter()
                .map(|leaf| PyString::new(py, &leaf.name))
                .collect::<Vec<_>>(),
        );
        declarations.push((source, field.value_type.clone(), leaves));
    }
    let mut private_cells = BTreeMap::new();
    if let Some(construction_captures) = construction_captures {
        for (_, _, leaves) in &declarations {
            for leaf in leaves {
                if leaf.binding_scope.definition_kind == DefinitionKind::Function
                    && !(leaf.class == own && leaf.binding == own.definition)
                    && let Some(cell) = construction_captures.cell_for(leaf)?
                {
                    private_cells.insert(leaf.clone(), cell);
                }
            }
        }
    }
    let needs_provider = declarations.iter().any(|(_, _, leaves)| {
        leaves.iter().any(|leaf| {
            leaf.binding_scope.definition_kind == DefinitionKind::Function
                && !(leaf.class == own && leaf.binding == own.definition)
                && !private_cells.contains_key(leaf)
        })
    });
    let provider = if needs_provider {
        class_provider(auth, namespace, fact, execution)?
    } else {
        None
    };
    if needs_provider && provider.is_none() {
        return Ok(Err(DynamicClassReason::UnresolvedAnalysis));
    }
    // All allocating key preparation precedes this callback-free snapshot.
    // Stack targets stay Bound until publication so an early decline releases
    // them immediately even when reached through Python::assume_attached.
    let captures = provider
        .as_ref()
        .map(validated_annotation_capture_schema)
        .transpose()?
        .unwrap_or_default();
    let closure = provider
        .as_ref()
        .and_then(|provider| {
            NonNull::new(unsafe {
                (*provider.function().as_ptr().cast::<ffi::PyFunctionObject>()).func_closure
            })
        })
        .map(|closure| {
            if auth.is_interpreter() {
                unsafe { Borrowed::<PyAny>::from_ptr(py, closure.as_ptr()) }
                    .cast::<PyTuple>()
                    .map(crate::strict_function::SupportedOperand::Borrowed)
                    .map_err(PyErr::from)
            } else {
                unsafe { Bound::<PyAny>::from_borrowed_ptr(py, closure.as_ptr()) }
                    .cast_into::<PyTuple>()
                    .map(crate::strict_function::SupportedOperand::Owned)
                    .map_err(PyErr::from)
            }
        })
        .transpose()?;
    let globals = auth.globals()?;
    let mut prepared = Vec::with_capacity(declarations.len());
    for ((field, value_type, leaves), names) in declarations.into_iter().zip(names) {
        let mut selected = PreparedBinding {
            field,
            value_type,
            leaves: Vec::with_capacity(leaves.len()),
            targets: Vec::with_capacity(leaves.len()),
            self_targets: Vec::new(),
        };
        for (leaf, key) in leaves.into_iter().zip(names) {
            let direct_self = leaf.class == own && leaf.binding == own.definition;
            let value = if direct_self {
                None
            } else {
                match leaf.binding_scope.definition_kind {
                    DefinitionKind::Module
                        if leaf.binding_scope == facts.module_body_identity()
                            && facts.global_bindings.iter().any(|global| {
                                global.name == leaf.name
                                    && global.definition.as_ref() == Some(&leaf.binding)
                            }) =>
                    {
                        // The signed leaf identifies this lexical operand,
                        // not a permanently immutable global. Required field
                        // predicates capture its actual type now and retain
                        // that target after later module rebinding.
                        dictionary_binding(py, globals.as_any(), &key)?
                    }
                    DefinitionKind::Function => {
                        if let Some(cell) = private_cells.get(&leaf) {
                            cell_contents(cell)?
                        } else {
                            match captures.iter().find(|slot| {
                                slot.matches_lexical_binding(&leaf.name, &leaf.binding_scope)
                            }) {
                                Some(slot) => captured_cell(
                                    py,
                                    closure.as_ref().map(|value| &**value),
                                    slot.cell_index,
                                )?,
                                None => None,
                            }
                        }
                    }
                    DefinitionKind::Class if leaf.binding_scope == fact.identity => {
                        dictionary_binding(py, namespace.as_any(), &key)?
                    }
                    _ => None,
                }
            };
            if !direct_self
                && value
                    .as_ref()
                    .is_none_or(|value| unsafe { ffi::PyType_Check(value.as_ptr()) } == 0)
            {
                return Ok(Err(DynamicClassReason::UnresolvedAnalysis));
            }
            let target = selected.targets.len();
            if direct_self {
                selected.self_targets.push(target);
            }
            selected
                .targets
                .push(value.unwrap_or_else(|| py.None().into_bound(py)));
            selected.leaves.push(BoundLeaf {
                source: leaf,
                target,
            });
        }
        prepared.push(selected);
    }
    let mut result = Vec::with_capacity(prepared.len());
    for selected in prepared {
        let state = StrictStateRef::new(
            py,
            BindingData {
                interpreter: verified.interpreter_id(),
                field: selected.field,
                value_type: selected.value_type,
                phase: Cell::new(Phase::Prepared),
                leaves: selected.leaves,
                self_targets: selected.self_targets,
            },
            selected.targets.into_iter().map(Bound::unbind).collect(),
        )?;
        result.push(StrictFieldBinding { state });
    }
    Ok(Ok(result))
}
