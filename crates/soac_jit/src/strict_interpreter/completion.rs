//! Actual source definition completion and permanent module publication.
//! Registries are weak and invocation-scoped. Completion never reopens a seal.

use std::ptr::NonNull;
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use soac_contracts::{DefinitionKind, SourceIdentity};
use soac_core::block_py::CallableSourceRole;

use super::call::InterpreterCall;
use super::native::RawInterpreterFrameInfo;
use super::{InterpreterInvocationIdentity, RootExecutionData, RootPhase};
use crate::module_type::SoacExtModule;
use crate::strict_function::{
    self, authenticate_borrowed_strict_function, finalize_eligible_function,
};
use crate::strict_interpreter_source::{
    InterpreterCodeRole, InterpreterDefinitionStore, StrictInterpreterSource,
};
use crate::strict_module::StrictPendingKind;
use crate::strict_state::StrictStateRef;
use crate::{StrictModuleExecutionRef, strict_runtime_unavailable};

unsafe extern "C" {
    fn PyFunction_GetSoacStrictOwner(function: *mut ffi::PyObject) -> *mut ffi::PyObject;
}

fn finalize_pending(
    py: Python<'_>,
    source: &Arc<StrictInterpreterSource>,
    execution: &StrictModuleExecutionRef,
    kind: &StrictPendingKind,
    object: &Bound<'_, PyAny>,
) -> PyResult<()> {
    match kind {
        StrictPendingKind::Function { .. } => Err(strict_runtime_unavailable(
            py,
            "compiler function identity reached native interpreter completion",
        )),
        StrictPendingKind::InterpreterFunction {
            native_code_ordinal,
        } => {
            let auth = authenticate_borrowed_strict_function(py, object.as_borrowed())?
                .ok_or_else(|| {
                    strict_runtime_unavailable(py, "pending native function lost its actual owner")
                })?;
            if !Arc::ptr_eq(auth.native_source()?, source)
                || !execution.same_execution(auth.execution_ref())
                || auth.native_code_ordinal()? != *native_code_ordinal
            {
                return Err(strict_runtime_unavailable(
                    py,
                    "pending native function changed source execution",
                ));
            }
            if auth.awaits_module_nominals() {
                return crate::strict_nominal::complete_module_nominals(py, &auth);
            }
            // A permitted ordinary code replacement has no source-execution
            // authority. Its actual function still has authenticated birth
            // provenance and must complete its permanent metadata contract.
            let Some(origin) = auth.source() else {
                return Ok(());
            };
            let facts = source.verified().type_facts().facts();
            if origin.role == CallableSourceRole::TypeParameterScope
                || (origin.role == CallableSourceRole::SourceFunction
                    && facts.source_class_owner(&origin.definition).is_some())
                || (origin.role == CallableSourceRole::AnnotationProvider
                    && matches!(
                        origin.definition.definition_kind,
                        DefinitionKind::Function
                            | DefinitionKind::Class
                            | DefinitionKind::TypeAlias
                            | DefinitionKind::Parameter
                    ))
                || !strict_function::eligible_source_function(source.verified(), Some(origin))
            {
                // Actual class/parent-function adoption owns these providers
                // and methods. Unsupported classes never acquire a late seal.
                return Ok(());
            }
            if !finalize_eligible_function(py, object, &origin.definition)? {
                return Err(strict_runtime_unavailable(
                    py,
                    "eligible native function could not complete adoption",
                ));
            }
            Ok(())
        }
        StrictPendingKind::Class { source: definition } => {
            let state =
                crate::strict_class_state::for_constructed_type(py, object)?.ok_or_else(|| {
                    strict_runtime_unavailable(py, "pending native class lost its construction")
                })?;
            if !state.is_interpreter_construction()
                || state.source() != definition
                || !Arc::ptr_eq(state.verified_module(), source.verified())
                || !execution.same_execution(state.execution_ref())
            {
                return Err(strict_runtime_unavailable(
                    py,
                    "pending class belongs to another native source execution",
                ));
            }
            if !state.is_finalized() {
                // Only the actual post-decoration definition Store may admit
                // the final type. A weak inventory entry is not that receipt.
                // Native will dispose it only if its own lineage already
                // resolved successfully to another independently guarded type.
                state.dispose_unselected_provisional()?;
            }
            Ok(())
        }
    }
}

/// A completion error may refuse only an unfinished child. It cannot revoke a
/// published function or class. Native captures its primary body exception
/// before this path and reports a secondary failure unraisably afterward.
fn refuse_unfinished_definition(
    py: Python<'_>,
    source: &Arc<StrictInterpreterSource>,
    execution: &StrictModuleExecutionRef,
    object: &Bound<'_, PyAny>,
) {
    if unsafe { ffi::PyType_Check(object.as_ptr()) } != 0 {
        if let Ok(Some(state)) = crate::strict_class_state::for_constructed_type(py, object)
            && state.is_interpreter_construction()
            && Arc::ptr_eq(state.verified_module(), source.verified())
            && execution.same_execution(state.execution_ref())
        {
            let _ = state.fail_unfinished_type();
        }
        return;
    }
    if unsafe { ffi::PyFunction_Check(object.as_ptr()) } == 0 {
        return;
    }
    let owner = unsafe { PyFunction_GetSoacStrictOwner(object.as_ptr()) };
    if owner.is_null() {
        return;
    }
    // The actual function's permanent native edge supports this owner. Do not
    // require mutable idle func_code to authenticate the captured birth here.
    if let Ok(owner) = unsafe {
        strict_function::authenticate_captured_interpreter_owner(
            py,
            NonNull::new(object.as_ptr()).expect("live function operand"),
            owner as usize,
            source,
            execution,
        )
    } {
        owner.data().mark_failed_pending();
    }
}

fn refuse_remaining_invocation(
    py: Python<'_>,
    source: &Arc<StrictInterpreterSource>,
    execution: &StrictModuleExecutionRef,
    globals: &Bound<'_, PyDict>,
    invocation: &Arc<InterpreterInvocationIdentity>,
) {
    while let Ok(Some((_, object))) =
        execution.next_interpreter_pending(py, globals, source.verified(), invocation)
    {
        refuse_unfinished_definition(py, source, execution, object.bind(py));
        object.drop_ref(py);
    }
}

fn complete_remaining_classes(
    py: Python<'_>,
    state: &InterpreterCall<'_>,
    globals: &Bound<'_, PyDict>,
    source: Option<&SourceIdentity>,
) -> PyResult<()> {
    let data = state.data();
    while let Some((kind, object)) = data.execution.next_interpreter_pending_class(
        py,
        globals,
        data.source.verified(),
        &data.invocation,
        source,
    )? {
        let object = object.into_bound(py);
        if let Err(error) = finalize_pending(py, &data.source, &data.execution, &kind, &object) {
            refuse_unfinished_definition(py, &data.source, &data.execution, &object);
            // Keep unrelated free functions for the module's final bindings.
            // This is the same weak inventory, not a class-owner registry.
            while let Ok(Some((_, remaining))) = data.execution.next_interpreter_pending_class(
                py,
                globals,
                data.source.verified(),
                &data.invocation,
                None,
            ) {
                refuse_unfinished_definition(py, &data.source, &data.execution, remaining.bind(py));
                remaining.drop_ref(py);
            }
            return Err(error);
        }
    }
    Ok(())
}

/// Called only by the already authenticated dataclass Apply selection while
/// the original class remains the real native CALL operand. The active owner
/// is borrowed from that invocation, never recovered from a numeric identity.
pub(super) fn dataclass_source_members(
    py: Python<'_>,
    call: &InterpreterCall<'_>,
    frame: &RawInterpreterFrameInfo,
    owner: Borrowed<'_, '_, PyAny>,
    original: Borrowed<'_, '_, PyAny>,
) -> PyResult<()> {
    let data = call.data();
    let globals: Borrowed<'_, '_, PyDict> =
        unsafe { Borrowed::from_ptr(py, frame.globals).cast_unchecked() };
    let _module = data
        .execution
        .acquire_owner(py, &globals, data.source.verified())?;
    let class =
        crate::strict_class_state::for_constructed_type(py, &original)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "dataclass source handoff has no pending native type")
        })?;
    if !class.is_interpreter_construction()
        || !class.is_pending_type()
        || !Arc::ptr_eq(class.verified_module(), data.source.verified())
        || !class.execution_ref().same_execution(&data.execution)
        || !class.matches_interpreter_completion(&data.invocation)
        || !class.matches_active_dataclass_owner(&owner)?
    {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass source handoff belongs to another actual execution",
        ));
    }
    crate::strict_class::snapshot_dataclass_source_members(py, &original, &class)
}

/// Native has retired the real Apply activation and its consumed original
/// operand. Only the actual returned type is borrowed here. This handoff
/// publishes generated-member evidence, never type admission or an original
/// type recovered from an address/weak inventory entry.
pub(super) fn dataclass_application_finished(
    py: Python<'_>,
    call: &InterpreterCall<'_>,
    frame: &RawInterpreterFrameInfo,
    owner: Borrowed<'_, '_, PyAny>,
    result: Borrowed<'_, '_, PyAny>,
) -> PyResult<()> {
    let data = call.data();
    let globals: Borrowed<'_, '_, PyDict> =
        unsafe { Borrowed::from_ptr(py, frame.globals).cast_unchecked() };
    let _module = data
        .execution
        .acquire_owner(py, &globals, data.source.verified())?;
    let class = crate::strict_class_state::for_constructed_type(py, &result)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "dataclass result has no actual pending native type")
    })?;
    if !class.is_interpreter_construction()
        || !class.is_pending_type()
        || !Arc::ptr_eq(class.verified_module(), data.source.verified())
        || !class.execution_ref().same_execution(&data.execution)
        || !class.matches_interpreter_completion(&data.invocation)
        || !class.matches_active_dataclass_owner(&owner)?
    {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass result handoff belongs to another actual execution",
        ));
    }
    crate::strict_dataclass::complete_native_application(&owner, &result)
}

/// The selected native callback has already authenticated this exact Apply
/// owner/source/caller. Failure has terminalized that graph before cleanup.
/// Remove its weak receipts, not its native barriers or unrelated definitions;
/// in particular no finalization is retried later at the caller's return.
pub(super) fn forget_failed_dataclass(
    py: Python<'_>,
    call: &InterpreterCall<'_>,
    frame: &RawInterpreterFrameInfo,
    owner: Borrowed<'_, '_, PyAny>,
    definition: &SourceIdentity,
) -> PyResult<()> {
    let data = call.data();
    let globals: Borrowed<'_, '_, PyDict> =
        unsafe { Borrowed::from_ptr(py, frame.globals).cast_unchecked() };
    data.execution.remove_interpreter_pending_class_matching(
        py,
        &globals,
        data.source.verified(),
        &data.invocation,
        definition,
        |class| {
            crate::strict_class_state::matches_failed_interpreter_dataclass(
                py,
                class,
                &owner,
                definition,
                data.source.verified(),
                &data.execution,
                &data.invocation,
            )
        },
    )?;
    Ok(())
}

pub(super) fn complete_invocation(
    py: Python<'_>,
    state: &InterpreterCall<'_>,
    frame: &RawInterpreterFrameInfo,
) -> PyResult<()> {
    let data = state.data();
    let globals: Borrowed<'_, '_, PyDict> =
        unsafe { Borrowed::from_ptr(py, frame.globals).cast_unchecked() };
    complete_remaining_classes(py, state, &globals, None)?;
    if !data
        .execution
        .is_sealed(py, &globals, data.source.verified())?
    {
        // Forward module bindings become final only in the one module seal.
        return Ok(());
    }
    while let Some((kind, object)) = data.execution.next_interpreter_pending(
        py,
        &globals,
        data.source.verified(),
        &data.invocation,
    )? {
        let object = object.into_bound(py);
        if let Err(error) = finalize_pending(py, &data.source, &data.execution, &kind, &object) {
            refuse_unfinished_definition(py, &data.source, &data.execution, &object);
            refuse_remaining_invocation(
                py,
                &data.source,
                &data.execution,
                &globals,
                &data.invocation,
            );
            return Err(error);
        }
    }
    Ok(())
}

pub(super) fn complete_definition(
    py: Python<'_>,
    state: &InterpreterCall<'_>,
    frame: &RawInterpreterFrameInfo,
    receipt: &InterpreterDefinitionStore,
    value: Borrowed<'_, '_, PyAny>,
) -> PyResult<()> {
    let data = state.data();
    let globals: Borrowed<'_, '_, PyDict> =
        unsafe { Borrowed::from_ptr(py, frame.globals).cast_unchecked() };
    match receipt.role {
        InterpreterCodeRole::SourceFunction | InterpreterCodeRole::AsyncSourceFunction => {
            if !data
                .execution
                .is_sealed(py, &globals, data.source.verified())?
            {
                return Ok(());
            }
            let facts = data.source.verified().type_facts().facts();
            if facts.source_class_owner(&receipt.source).is_some() {
                return Ok(());
            }
            let Some(auth) = authenticate_borrowed_strict_function(py, value)? else {
                // A decorator's foreign result is not adopted as this function.
                return Ok(());
            };
            if !Arc::ptr_eq(auth.native_source()?, &data.source)
                || !auth.execution_ref().same_execution(&data.execution)
                || !Arc::ptr_eq(auth.native_birth_execution()?, &data.invocation)
                || auth.native_code_ordinal()? != receipt.body_code_ordinal
                || !auth.origin().is_some_and(|origin| {
                    origin.role == CallableSourceRole::SourceFunction
                        && origin.definition == receipt.source
                })
                || !auth.can_finalize()
            {
                return Ok(());
            }
            if !finalize_eligible_function(py, &value, &receipt.source)? {
                return Err(strict_runtime_unavailable(
                    py,
                    "actual native function definition could not complete",
                ));
            }
            data.execution.remove_pending(
                py,
                &globals,
                data.source.verified(),
                &StrictPendingKind::InterpreterFunction {
                    native_code_ordinal: receipt.body_code_ordinal,
                },
                &value,
            )?;
        }
        InterpreterCodeRole::ClassNamespace => {
            let Some(class) = crate::strict_class_state::for_constructed_type(py, &value)? else {
                return Ok(());
            };
            if class.source() != &receipt.source
                || !Arc::ptr_eq(class.verified_module(), data.source.verified())
                || !class.execution_ref().same_execution(&data.execution)
                || !class.matches_interpreter_completion(&data.invocation)
            {
                return Ok(());
            }
            if class.pending_dataclass() {
                return Err(strict_runtime_unavailable(
                    py,
                    "native dataclass Store preceded its actual adapter completion",
                ));
            }
            if !crate::strict_class::finalize_class(py, &value, &receipt.source)? {
                return Err(strict_runtime_unavailable(
                    py,
                    "actual native class definition lost its contract",
                ));
            }
            data.execution.remove_pending(
                py,
                &globals,
                data.source.verified(),
                &StrictPendingKind::Class {
                    source: receipt.source.clone(),
                },
                &value,
            )?;
            // Do not keep the escaped original artificially pending until
            // this surrounding factory returns. Native checks each actual
            // lineage and preserves every independently published contract.
            complete_remaining_classes(py, state, &globals, Some(&receipt.source))?;
        }
        _ => {
            return Err(strict_runtime_unavailable(
                py,
                "native definition receipt has an unsupported source role",
            ));
        }
    }
    Ok(())
}

pub(super) fn finalize_interpreter_module(
    py: Python<'_>,
    module: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let owner = SoacExtModule::with_interpreter_state(module, |state| {
        let state = state
            .ok_or_else(|| strict_runtime_unavailable(py, "native module state was cleared"))?;
        Ok(state.owner.clone_ref(py).into_bound(py))
    })?;
    let owner = StrictStateRef::<RootExecutionData>::from_owner(owner)?;
    let data = owner.data();
    if data.phase.get() != RootPhase::Returned
        || !data.original_code_entered.get()
        || data.module_identity != module.as_ptr() as usize
    {
        return Err(strict_runtime_unavailable(
            py,
            "native module did not return from its original initializer",
        ));
    }
    let globals =
        unsafe { Borrowed::<PyAny>::from_ptr_or_err(py, ffi::PyModule_GetDict(module.as_ptr()))? };
    let globals: Borrowed<'_, '_, PyDict> = globals.cast()?;
    data.execution
        .begin_interpreter_sealing(py, &globals, data.source.verified())?;
    while let Some((kind, object)) =
        data.execution
            .next_pending(py, &globals, data.source.verified())?
    {
        let object = object.into_bound(py);
        if let Err(error) = finalize_pending(py, &data.source, &data.execution, &kind, &object) {
            refuse_unfinished_definition(py, &data.source, &data.execution, &object);
            return Err(error);
        }
    }
    data.execution
        .finish_interpreter_sealing(py, &globals, data.source.verified())
}
