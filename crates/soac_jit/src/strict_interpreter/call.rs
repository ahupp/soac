//! One metadata shell for an ordinary native frame. The frame, not this
//! shell, owns its function, executable, maps, locals, arguments and closure.

use std::cell::Cell;
use std::ffi::CStr;
use std::ptr::NonNull;
use std::sync::Arc;

use pyo3::prelude::*;

use super::InterpreterInvocationIdentity;
use super::native::{self, RawInterpreterFrameInfo};
use crate::strict_interpreter_source::StrictInterpreterSource;
use crate::strict_namespace::NamespaceExecution;
use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{StrictModuleExecutionRef, strict_runtime_unavailable};

pub(super) const SUBJECT_OWNER: usize = 0;
pub(super) const MODULE_POLICY: usize = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CallPhase {
    Binding,
    Running,
    Returning,
    Failing,
    NamespaceTransferred,
    Retired,
}

pub(super) struct InterpreterCallData {
    pub(super) source: Arc<StrictInterpreterSource>,
    pub(super) execution: StrictModuleExecutionRef,
    pub(super) invocation: Arc<InterpreterInvocationIdentity>,
    pub(super) namespace: Option<Arc<NamespaceExecution>>,
    pub(super) kind: u32,
    pub(super) source_code_ordinal: Option<u32>,
    pub(super) subject_owner: usize,
    pub(super) function_identity: usize,
    pub(super) code_identity: usize,
    pub(super) globals_identity: usize,
    pub(super) builtins_identity: usize,
    pub(super) locals_identity: usize,
    pub(super) phase: Cell<CallPhase>,
}

// SAFETY: All permanent Python references are in the shell's GC vector. Every
// address above is a comparison-only witness requiring an actual frame view.
unsafe impl StrictStateData for InterpreterCallData {
    const TYPE_NAME: &'static CStr = c"soac._InterpreterCall";

    fn on_terminal(&self) {
        self.phase.set(CallPhase::Retired);
        if let Some(namespace) = &self.namespace {
            namespace.fail_native_unfinished();
        }
    }
}

impl InterpreterCallData {
    /// Scalar-only comparison. Native owns every frame operand for this view.
    /// An idle function's mutable code/defaults are deliberately not consulted.
    pub(super) fn matches_frame(&self, info: &RawInterpreterFrameInfo) -> bool {
        info.abi_version == 1
            && info.function as usize == self.function_identity
            && info.code as usize == self.code_identity
            && info.globals as usize == self.globals_identity
            && info.builtins as usize == self.builtins_identity
            && (self.kind != native::CLASS_NAMESPACE
                || info.locals as usize == self.locals_identity)
    }

    pub(super) fn has_source_authority(&self) -> bool {
        self.source_code_ordinal.is_some()
    }
}

pub(super) type InterpreterCall<'py> = StrictStateRef<'py, InterpreterCallData>;

/// The native callback itself supports the metadata pointer. Acquiring this
/// guard adds no primary execution-value owner and protects payload access
/// across an explicit native clear of the shell.
pub(super) unsafe fn captured_call<'py>(
    py: Python<'py>,
    info: &RawInterpreterFrameInfo,
) -> PyResult<InterpreterCall<'py>> {
    if info.call_state.is_null() {
        return Err(strict_runtime_unavailable(
            py,
            "native frame has no captured call state",
        ));
    }
    let state = StrictStateRef::<InterpreterCallData>::from_owner(unsafe {
        Bound::from_borrowed_ptr(py, info.call_state)
    })?;
    if !state.data().matches_frame(info) || state.data().phase.get() == CallPhase::Retired {
        return Err(strict_runtime_unavailable(
            py,
            "native call state no longer matches its actual frame",
        ));
    }
    state.data().execution.validate_owner(
        py,
        &state.reference(MODULE_POLICY)?,
        state.data().source.verified(),
    )?;
    if state.data().kind == native::ROOT {
        let root = StrictStateRef::<super::RootExecutionData>::from_owner(
            state.reference(SUBJECT_OWNER)?,
        )?;
        if root.owner().as_ptr() as usize != state.data().subject_owner
            || !Arc::ptr_eq(&root.data().source, &state.data().source)
            || !root
                .data()
                .execution
                .same_execution(&state.data().execution)
            || !matches!(
                root.data().phase.get(),
                super::RootPhase::Entering | super::RootPhase::Running
            )
        {
            return Err(strict_runtime_unavailable(
                py,
                "captured root execution was retired or replaced",
            ));
        }
    } else {
        unsafe {
            crate::strict_function::authenticate_captured_interpreter_owner(
                py,
                NonNull::new(info.function).ok_or_else(|| {
                    strict_runtime_unavailable(py, "captured frame has no actual function")
                })?,
                state.data().subject_owner,
                &state.data().source,
                &state.data().execution,
            )?;
        }
    }
    Ok(state)
}
