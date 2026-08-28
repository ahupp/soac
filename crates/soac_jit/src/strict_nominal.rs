//! Bind optional guarded capability targets to actual lexical/class executions.
//!
//! Source references request targets, never establish argument or result types. The
//! class-construction owner supplies the actual type and namespace execution;
//! only that class's source-owned methods can consume this witness. Other
//! leaves consume explicit checker-resolved lexical bindings, never name-only
//! authority, annotation evaluation, or a latest-by-source class registry.

use std::ptr::{self, NonNull};
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyString, PyTuple};
use soac_contracts::{
    ClassReference, ClassTypeFact, DefinitionKind, DescriptorFact, GlobalMutability,
    NominalBindingFact, SourceIdentity,
};
use soac_core::block_py::CallableSourceRole;

use crate::strict_annotation::{AnnotationCaptureKind, validated_annotation_capture_schema};
use crate::strict_class_state::{StrictClassState, matches_function_class_namespace};
use crate::strict_function::{
    AuthenticatedStrictFunction, authenticate_borrowed_strict_function, bind_strict_nominal_type,
    reserve_strict_nominal_types,
};
use crate::{VerifiedStrictModule, strict_runtime_unavailable};

unsafe extern "C" {
    fn PyCell_Get(cell: *mut ffi::PyObject) -> *mut ffi::PyObject;
}

fn reference(class: &StrictClassState<'_>) -> ClassReference {
    ClassReference {
        definition: class.source().clone(),
        source_digest: class.verified_module().type_facts().facts().source_digest,
    }
}

fn descriptor_owns(descriptor: &DescriptorFact, source: &SourceIdentity) -> bool {
    [&descriptor.getter, &descriptor.setter, &descriptor.deleter]
        .into_iter()
        .any(|component| component.as_ref() == Some(source))
}

fn owns_method(fact: &ClassTypeFact, source: &SourceIdentity) -> bool {
    fact.methods.iter().any(|method| {
        method.declaring_class.definition == fact.identity
            && method.implementation.as_ref() == Some(source)
    }) || fact.instance_fields.iter().any(|field| {
        field.declaring_class.definition == fact.identity
            && descriptor_owns(&field.descriptor, source)
    }) || fact
        .class_members
        .iter()
        .any(|member| descriptor_owns(&member.descriptor, source))
}

fn own_leaves(
    auth: &AuthenticatedStrictFunction<'_, '_>,
    own: &ClassReference,
) -> Vec<NominalBindingFact> {
    auth.capability_nominal_bindings()
        .iter()
        .filter(|leaf| &leaf.class == own && leaf.binding == own.definition)
        .cloned()
        .collect()
}

fn validate_method_execution(
    py: Python<'_>,
    auth: &AuthenticatedStrictFunction<'_, '_>,
    verified: &Arc<VerifiedStrictModule>,
    fact: &ClassTypeFact,
    execution: &Arc<crate::strict_namespace::NamespaceExecution>,
) -> PyResult<()> {
    if !auth.can_finalize()
        || !Arc::ptr_eq(auth.verified_module(), verified)
        || !auth
            .creation_execution()
            .is_some_and(|creation| Arc::ptr_eq(creation, execution))
        || !execution.is_completed()
        || !auth.origin().is_some_and(|origin| {
            origin.role == CallableSourceRole::SourceFunction
                && owns_method(fact, &origin.definition)
        })
    {
        return Err(strict_runtime_unavailable(
            py,
            "nominal method belongs to a different class source execution",
        ));
    }
    Ok(())
}

/// Reserve only requested direct-self capability targets before native construction. The
/// function owns the unresolved slot; final selected-type admission validates
/// the actual method and binds it. No class-side method inventory is retained.
pub(crate) fn prepare_owned_method_nominals(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    verified: &Arc<VerifiedStrictModule>,
    fact: &ClassTypeFact,
    execution: &Arc<crate::strict_namespace::NamespaceExecution>,
) -> PyResult<bool> {
    let auth =
        authenticate_borrowed_strict_function(py, function.as_borrowed())?.ok_or_else(|| {
            strict_runtime_unavailable(py, "nominal method preparation has no function owner")
        })?;
    let own = ClassReference {
        definition: fact.identity.clone(),
        source_digest: verified.type_facts().facts().source_digest,
    };
    let leaves = own_leaves(&auth, &own);
    if leaves.is_empty() {
        return Ok(false);
    }
    validate_method_execution(py, &auth, verified, fact, execution)?;
    reserve_strict_nominal_types(py, function, &leaves).map(|()| true)
}

/// Called at actual method adoption, before freezing the native function.
/// Only selected guarded member sites may acquire GC-visible type references;
/// annotations alone do not retain nominal targets or establish value types.
/// Only the direct class-definition leaf denotes this actual self type. An
/// alias with the same ClassReference can name another factory execution and
/// must use its signed lexical binding instead.
pub(crate) fn bind_owned_method_nominals(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    class: &StrictClassState<'_>,
) -> PyResult<()> {
    let auth =
        authenticate_borrowed_strict_function(py, function.as_borrowed())?.ok_or_else(|| {
            strict_runtime_unavailable(py, "nominal method adoption has no function owner")
        })?;
    let own = reference(class);
    let leaves = own_leaves(&auth, &own);
    if leaves.is_empty() {
        return Ok(());
    }
    validate_method_execution(
        py,
        &auth,
        class.verified_module(),
        class.fact(),
        class.namespace_execution(),
    )?;

    let actual = class.actual_type()?;
    for leaf in leaves {
        // SAFETY: The native class owner and precise creating namespace Arc
        // authenticate this exact signed direct self-class binding.
        // A later operation still checks its actual receiver and storage;
        // this binding alone grants no exact receiver or layout proof.
        unsafe { bind_strict_nominal_type(py, function, &leaf, &actual)? };
    }
    Ok(())
}

/// Temporary roots for cold capability publication, never an argument/return
/// type check or a per-entry snapshot. Adoption copies resolved targets into
/// the function's GC vector; unresolved requests grant no capability.
pub(crate) struct StrictNominalSnapshot {
    entries: Vec<(NominalBindingFact, Option<Py<PyAny>>)>,
}

impl Drop for StrictNominalSnapshot {
    fn drop(&mut self) {
        // These are stack-only roots created and released with the GIL held.
        // A raw JIT callback need not have registered a PyO3 attachment;
        // ordinary Py::drop would then defer DECREF and keep class/cell cycles
        // alive until an unrelated PyO3 entry. Do not put active-call roots in
        // that non-GC-visible queue, including on partial snapshot failure.
        unsafe {
            crate::strict_state::release_references(
                ptr::null_mut(),
                self.entries
                    .iter_mut()
                    .filter_map(|(_, target)| target.take()),
            );
        }
    }
}

/// Read only an exact string-key dictionary, with no hashing or rich equality
/// on arbitrary keys. A substituted mapping with custom keys is unresolved,
/// not an invitation to execute Python while resolving a source-owned target.
pub(crate) fn dictionary_binding<'py>(
    py: Python<'py>,
    dictionary: &Bound<'py, PyAny>,
    name: &Bound<'py, PyString>,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    Ok(unsafe { dictionary_binding_ptr(py, dictionary, name)? }
        .map(|value| unsafe { Bound::from_borrowed_ptr(py, value.as_ptr()) }))
}

/// # Safety
/// Use the returned borrowed value only before any callback or dictionary
/// mutation, while the actual caller-supported dictionary remains live.
pub(crate) unsafe fn dictionary_binding_ptr(
    py: Python<'_>,
    dictionary: &Bound<'_, PyAny>,
    name: &Bound<'_, PyString>,
) -> PyResult<Option<NonNull<ffi::PyObject>>> {
    if unsafe { ffi::PyDict_CheckExact(dictionary.as_ptr()) } == 0 {
        return Ok(None);
    }
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    let mut selected = ptr::null_mut();
    while unsafe { ffi::PyDict_Next(dictionary.as_ptr(), &mut position, &mut key, &mut value) } != 0
    {
        if unsafe { ffi::PyUnicode_CheckExact(key) } == 0 {
            return Ok(None);
        }
        let equal = if key == name.as_ptr() {
            true
        } else {
            let comparison = unsafe { ffi::PyUnicode_Compare(key, name.as_ptr()) };
            if comparison == -1 && !unsafe { ffi::PyErr_Occurred() }.is_null() {
                return Err(PyErr::fetch(py));
            }
            comparison == 0
        };
        if equal {
            selected = value;
        }
    }
    Ok(NonNull::new(selected))
}

pub(crate) fn captured_cell<'py>(
    _py: Python<'py>,
    closure: Option<&Bound<'py, PyTuple>>,
    index: usize,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let Some(closure) = closure else {
        return Ok(None);
    };
    let cell = closure.get_item(index)?;
    cell_contents(&cell)
}

/// Read a selected original lexical cell without evaluating annotations or
/// invoking Python. Its authenticated producer has already validated the cell
/// identity; contents are intentionally sampled only at the binding boundary.
pub(crate) fn cell_contents<'py>(cell: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyAny>>> {
    let py = cell.py();
    // The authenticated capture producer has already checked every exact cell.
    // PyCell_Get only INCREFs the current value; it cannot invoke user code.
    let value = unsafe { PyCell_Get(cell.as_ptr()) };
    if value.is_null() {
        if unsafe { ffi::PyErr_Occurred() }.is_null() {
            Ok(None)
        } else {
            Err(PyErr::fetch(py))
        }
    } else {
        Ok(Some(unsafe { Bound::from_owned_ptr(py, value) }))
    }
}

/// Capture the actual selected bindings at one boundary. All provider/code
/// validation and name allocation precede reading the values, and the reads
/// themselves cannot run Python. These targets authorize only independently
/// guarded capability publication, never a function value-type assumption.
pub(crate) fn snapshot_function_nominals(
    py: Python<'_>,
    auth: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<StrictNominalSnapshot> {
    snapshot_function_nominals_except(py, auth, None)
}

fn snapshot_function_nominals_except(
    py: Python<'_>,
    auth: &AuthenticatedStrictFunction<'_, '_>,
    deferred_self: Option<&SourceIdentity>,
) -> PyResult<StrictNominalSnapshot> {
    if auth.capability_nominal_bindings().is_empty() {
        return Ok(StrictNominalSnapshot {
            entries: Vec::new(),
        });
    }
    let leaves: Vec<_> = auth
        .capability_nominal_bindings()
        .iter()
        .filter(|leaf| deferred_self.is_none_or(|deferred| &leaf.binding != deferred))
        .collect();
    let facts = auth.verified_module().type_facts().facts();
    let globals = auth.globals()?;
    let globals_are_final =
        auth.execution_ref()
            .bindings_are_final(py, &globals, auth.verified_module())?;
    let needs_provider = !auth.is_finalized()
        && leaves
            .iter()
            .any(|leaf| leaf.binding_scope.definition_kind != DefinitionKind::Module);
    let provider = if needs_provider {
        auth.owned_annotation_provider()?
    } else {
        None
    };
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
            unsafe { Bound::<PyAny>::from_borrowed_ptr(py, closure.as_ptr()) }
                .cast_into::<PyTuple>()
        })
        .transpose()?;
    let names: Vec<_> = leaves
        .iter()
        .map(|leaf| PyString::new(py, &leaf.name))
        .collect();
    let mut values = Vec::with_capacity(leaves.len());
    for (leaf, name) in leaves.iter().zip(&names) {
        if let Some(target) = auth.bound_nominal_target(leaf)? {
            values.push(((**leaf).clone(), Some(target)));
            continue;
        }
        let value = if auth.has_nominal_reservation(leaf)
            || (auth.is_finalized()
                && !(auth.awaits_module_nominals()
                    && leaf.binding_scope == facts.module_body_identity()))
        {
            None
        } else {
            match leaf.binding_scope.definition_kind {
                DefinitionKind::Module
                    if globals_are_final
                        && leaf.binding_scope == facts.module_body_identity()
                        && facts.global_bindings.iter().any(|global| {
                            global.name == leaf.name
                                && global.definition.as_ref() == Some(&leaf.binding)
                                && matches!(
                                    global.mutability,
                                    GlobalMutability::FinalAfterSeal
                                        | GlobalMutability::LateAppendOnly
                                )
                        }) =>
                {
                    dictionary_binding(py, globals.as_any(), name)?
                }
                DefinitionKind::Function => {
                    let slot = captures
                        .iter()
                        .find(|slot| slot.matches_lexical_binding(&leaf.name, &leaf.binding_scope));
                    match slot {
                        Some(slot) => captured_cell(py, closure.as_ref(), slot.cell_index)?,
                        None => None,
                    }
                }
                DefinitionKind::Class => {
                    let slot = captures
                        .iter()
                        .find(|slot| slot.kind == AnnotationCaptureKind::ClassDictionary);
                    match (auth.creation_execution(), slot) {
                        (Some(execution), Some(slot))
                            if execution.source() == &leaf.binding_scope =>
                        {
                            // Python may replace the genuine captured cell's
                            // value. Only the actual copied tp_dict, its live
                            // native class policy, and this exact namespace
                            // execution authorize a class-scope binding.
                            match captured_cell(py, closure.as_ref(), slot.cell_index)? {
                                Some(dictionary)
                                    if matches_function_class_namespace(py, &dictionary, auth)? =>
                                {
                                    dictionary_binding(py, &dictionary, name)?
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        values.push(((**leaf).clone(), value));
    }
    let mut snapshot = StrictNominalSnapshot {
        entries: Vec::with_capacity(values.len()),
    };
    for (leaf, value) in values {
        let target = match value {
            // The signed leaf authenticates this exact lexical operand, not
            // a predicted runtime class. Nominal membership works for an
            // ordinary or framework type just as for a participating type.
            // A custom metaclass, an alias holding another factory result,
            // or a different actual annotation binding grants no layout or
            // method proof. Optional capability publication independently
            // requires a sealed native class with the matching source/digest.
            Some(value) if unsafe { ffi::PyType_Check(value.as_ptr()) } != 0 => {
                Some(value.unbind())
            }
            _ => None,
        };
        snapshot.entries.push((leaf, target));
    }
    Ok(snapshot)
}

/// Class admission seals the method's native metadata before module globals
/// are final. Keep only missing signed module leaves in this explicit stage;
/// direct-self reservations and class/closure bindings never use a name-only
/// fallback to another construction's type.
pub(crate) fn globals_pending_at_adoption(
    py: Python<'_>,
    auth: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<bool> {
    if auth.is_interpreter() && !auth.interpreter_source_authority()? {
        return Ok(false);
    }
    if !auth
        .verified_module()
        .type_facts()
        .facts()
        .language_policy
        .strict_assign
    {
        // Mutable module bindings never become nominal capabilities. There
        // is no future publication boundary to await; existing field checks
        // retain their independently captured actual declaration targets.
        return Ok(false);
    }
    let globals = auth.globals()?;
    if auth
        .execution_ref()
        .bindings_are_final(py, &globals, auth.verified_module())?
    {
        return Ok(false);
    }
    let module = auth
        .verified_module()
        .type_facts()
        .facts()
        .module_body_identity();
    Ok(auth
        .capability_nominal_bindings()
        .iter()
        .any(|leaf| leaf.binding_scope == module && !auth.has_nominal_reservation(leaf)))
}

/// The same weak function inventory reaches this operation during the one
/// authenticated module seal. Earlier calls used ordinary fallback before
/// target publication. Existing targets are never rebound, and the
/// already sealed function/default/provider metadata is not reopened.
pub(crate) fn complete_module_nominals(
    py: Python<'_>,
    auth: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<()> {
    if !auth.awaits_module_nominals() {
        return Ok(());
    }
    let globals = auth.globals()?;
    if !auth
        .execution_ref()
        .bindings_are_final(py, &globals, auth.verified_module())?
    {
        return Ok(());
    }
    if !auth.is_finalized() || (auth.is_interpreter() && !auth.interpreter_source_authority()?) {
        return Err(strict_runtime_unavailable(
            py,
            "module nominal completion has no sealed source function",
        ));
    }
    let snapshot = snapshot_function_nominals(py, auth)?;
    let module = auth
        .verified_module()
        .type_facts()
        .facts()
        .module_body_identity();
    for (leaf, target) in &snapshot.entries {
        if leaf.binding_scope == module
            && !auth.has_nominal_reservation(leaf)
            && let Some(target) = target
        {
            // SAFETY: The exact source-owned function's actual globals were
            // authenticated at this execution's final binding boundary.
            unsafe {
                bind_strict_nominal_type(py, auth.function(), leaf, target.bind(py))?;
            }
        }
    }
    crate::strict_function::finish_module_nominals(auth)
}

/// An exact trusted transformation may release its provisional type before
/// its final result reaches the caller. Preserve only requested lexical alias
/// targets while that actual source dictionary is still supported. Direct-self
/// leaves are deliberately neither read nor bound here: the final decorated
/// type will fill their existing reservations once, before function freezing.
pub(crate) fn bind_pretransform_method_nominals(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    class: &StrictClassState<'_>,
) -> PyResult<()> {
    let auth =
        authenticate_borrowed_strict_function(py, function.as_borrowed())?.ok_or_else(|| {
            strict_runtime_unavailable(py, "dataclass source snapshot has no function owner")
        })?;
    if !class.is_pending_type()
        || class.is_interpreter_construction() != auth.is_interpreter()
        || auth.is_finalized()
    {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass source snapshot is not an unfinished source method",
        ));
    }
    validate_method_execution(
        py,
        &auth,
        class.verified_module(),
        class.fact(),
        class.namespace_execution(),
    )?;
    let snapshot = snapshot_function_nominals_except(py, &auth, Some(class.source()))?;
    for (leaf, target) in &snapshot.entries {
        if let Some(target) = target {
            // SAFETY: This exact source method/provider, actual pending
            // namespace and active adapter authenticated the lexical operand.
            unsafe {
                bind_strict_nominal_type(py, function, leaf, target.bind(py))?;
            }
        }
    }
    Ok(())
}

/// Consume signed lexical operands at actual final adoption. A source-level
/// class identity never substitutes for the object held by that exact alias.
pub(crate) fn bind_lexical_function_nominals_with_auth(
    py: Python<'_>,
    auth: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<()> {
    if auth.capability_nominal_bindings().is_empty() {
        return Ok(());
    }
    let function = auth.function();
    let snapshot = snapshot_function_nominals(py, auth)?;
    for (leaf, target) in &snapshot.entries {
        if let Some(target) = target {
            // SAFETY: The snapshot resolved this signed leaf through the
            // function's actual lexical operand and native type identity.
            // Final adoption fixes that selected target, not the mutable cell.
            unsafe { bind_strict_nominal_type(py, function, leaf, target.bind(py))? };
        }
    }
    Ok(())
}
