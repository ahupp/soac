//! Authenticated source execution on ordinary CPython frames.
//!
//! This backend has no BlockPy module, compiler function IDs, lowered cache,
//! optimization plan or JIT entry. Native object ownership and the existing
//! permanent contract policies remain the enforcement boundary.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_int, c_void};
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::module_type::{SoacExtModule, compile_verified_native_details, new_strict_module};
use crate::strict_interpreter_source::StrictInterpreterSource;
use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{StrictModuleRuntimeState, VerifiedStrictModule, strict_runtime_unavailable};

mod call;
mod call_join;
mod callbacks;
mod completion;
mod native;

use completion::finalize_interpreter_module;

/// Allocation identity for one actual ordinary native activation. It owns no
/// Python objects and is never serialized or inferred from a source location.
/// Compare the enclosing `Arc`s by pointer identity, not this zero-sized value.
#[derive(Debug)]
pub(crate) struct InterpreterInvocationIdentity {
    _private: (),
}

impl InterpreterInvocationIdentity {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { _private: () })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RootPhase {
    Ready,
    Entering,
    Running,
    Returned,
    Failed,
}

struct RootExecutionData {
    source: Arc<StrictInterpreterSource>,
    execution: crate::StrictModuleExecutionRef,
    module_identity: usize,
    code_identity: usize,
    phase: Cell<RootPhase>,
    original_code_entered: Cell<bool>,
}

// SAFETY: All Python references live in the GC vector. Source/execution data
// and remembered addresses are Rust-only; no address is independently live.
unsafe impl StrictStateData for RootExecutionData {
    const TYPE_NAME: &'static CStr = c"soac._InterpreterModuleExecution";

    fn on_terminal(&self) {
        self.phase.set(RootPhase::Failed);
    }
}

/// One variant of the real native module state. The root code is consumed by
/// its one initialization attempt, not retained as a hidden code-catalog pin.
/// The policy guard has relinquished its extra globals edge to actual md_dict.
pub(crate) struct InterpreterModuleState {
    source: Arc<StrictInterpreterSource>,
    runtime: StrictModuleRuntimeState,
    root: RefCell<Option<Py<PyAny>>>,
    owner: Py<PyAny>,
}

impl InterpreterModuleState {
    pub(crate) unsafe fn traverse(&self, visit: ffi::visitproc, arg: *mut c_void) -> c_int {
        let result = unsafe { self.runtime.traverse(visit, arg) };
        if result != 0 {
            return result;
        }
        let result = unsafe { visit(self.owner.as_ptr(), arg) };
        if result != 0 {
            return result;
        }
        if let Some(root) = self.root.borrow().as_ref() {
            return unsafe { visit(root.as_ptr(), arg) };
        }
        0
    }

    /// The module state slot has already been retired before this call.
    pub(crate) unsafe fn release_from_native(self, py: Python<'_>) {
        let Self {
            source,
            runtime,
            root,
            owner,
        } = self;
        unsafe { runtime.release_from_native(py) };
        if let Some(root) = root.into_inner() {
            root.drop_ref(py);
        }
        owner.drop_ref(py);
        drop(source);
    }
}

pub fn create_interpreter_module(
    py: Python<'_>,
    spec: &Bound<'_, PyAny>,
    verified: Arc<VerifiedStrictModule>,
) -> PyResult<Py<PyAny>> {
    native::initialize(py)?;
    let name: String = spec.getattr("name")?.extract()?;
    let package: String = spec.getattr("parent")?.extract()?;
    if name != verified.type_facts().facts().module.module_name {
        return Err(strict_runtime_unavailable(
            py,
            "native module spec differs from verified source",
        ));
    }
    let details = compile_verified_native_details(py, &verified)?;
    let root = details.get_item(0)?;
    let source = Arc::new(StrictInterpreterSource::from_native_details(
        py,
        verified.clone(),
        &root,
        &details.get_item(2)?,
    )?);
    let module = new_strict_module(py, spec, &name, &package)?;
    let runtime = StrictModuleRuntimeState::install(py, &module, &verified)?
        .handoff_globals_to_module(py, &module, &verified)?;
    let globals =
        unsafe { Borrowed::<PyAny>::from_ptr_or_err(py, ffi::PyModule_GetDict(module.as_ptr()))? }
            .cast::<PyDict>()?;
    let execution = runtime.execution_ref();
    let policy = execution.acquire_owner(py, &globals, &verified)?;
    let owner = StrictStateRef::new(
        py,
        RootExecutionData {
            source: source.clone(),
            execution,
            module_identity: module.as_ptr() as usize,
            code_identity: root.as_ptr() as usize,
            phase: Cell::new(RootPhase::Ready),
            original_code_entered: Cell::new(false),
        },
        vec![policy],
    )?;
    SoacExtModule::install_interpreter_state(
        &module,
        InterpreterModuleState {
            source,
            runtime,
            root: RefCell::new(Some(root.unbind())),
            owner: owner.owner().clone().unbind(),
        },
    )?;
    Ok(module.unbind())
}

/// True only for an owned interpreter module. Terminal owned state is an
/// error, never permission to retry through an ordinary or SOAC loader.
pub fn exec_interpreter_module(py: Python<'_>, module: &Bound<'_, PyAny>) -> PyResult<bool> {
    let snapshot = SoacExtModule::with_interpreter_state(module, |state| {
        let Some(state) = state else {
            return Ok(None);
        };
        let root = state.root.borrow_mut().take().ok_or_else(|| {
            strict_runtime_unavailable(py, "strict module body execution is single-use")
        })?;
        Ok(Some((
            root.into_bound(py),
            state.owner.clone_ref(py).into_bound(py),
        )))
    })?;
    let Some((root, owner)) = snapshot else {
        return Ok(false);
    };
    // Do not borrow Rust module state across arbitrary source callbacks or a
    // supported native clear. The native API supports these exact arguments.
    let result = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            native::PySoac_EvalInterpreterModuleV1(module.as_ptr(), root.as_ptr(), owner.as_ptr()),
        )
    };
    let result = result.and_then(|returned| {
        drop(returned);
        finalize_interpreter_module(py, module)
    });
    if result.is_err() {
        let _ = SoacExtModule::with_interpreter_state(module, |state| {
            if let Some(state) = state {
                state.runtime.fail_unfinished();
            }
            Ok(())
        });
    }
    result.map(|()| true)
}

pub fn interpreter_module_diagnostics<'py>(
    py: Python<'py>,
    module: &Bound<'py, PyAny>,
) -> PyResult<Option<Bound<'py, PyDict>>> {
    let snapshot = SoacExtModule::with_interpreter_state(module, |state| {
        let Some(state) = state else {
            return Ok(None);
        };
        Ok(Some((
            state.source.verified().clone(),
            state.runtime.execution_ref(),
            state.owner.clone_ref(py).into_bound(py),
        )))
    })?;
    let Some((verified, execution, owner)) = snapshot else {
        return Ok(None);
    };
    // Snapshot metadata only; never hold a module-state Rust borrow across
    // validation/error construction, and never add a dictionary owner.
    let globals =
        unsafe { Borrowed::<PyAny>::from_ptr_or_err(py, ffi::PyModule_GetDict(module.as_ptr()))? }
            .cast::<PyDict>()?;
    let sealed = execution.is_sealed(py, &globals, &verified)?;
    let owner = StrictStateRef::<RootExecutionData>::from_owner(owner)?;
    if owner.data().module_identity != module.as_ptr() as usize {
        return Err(strict_runtime_unavailable(
            py,
            "native module execution identity mismatch",
        ));
    }
    let entered = owner.data().original_code_entered.get();
    let facts = verified.type_facts();
    let result = PyDict::new(py);
    result.set_item("schema", 1)?;
    result.set_item("backend", "cpython")?;
    result.set_item("sealed", sealed)?;
    result.set_item("initializer_entry_kind", entered.then_some("original_code"))?;
    result.set_item("original_code_entered", entered)?;
    result.set_item("module_name", &facts.facts().module.module_name)?;
    result.set_item(
        "source_path",
        verified.source_path().to_string_lossy().as_ref(),
    )?;
    result.set_item("source_sha256", facts.facts().source_digest.to_hex())?;
    result.set_item(
        "artifact_generation",
        facts.generation().fingerprint().to_hex(),
    )?;
    result.set_item("startup_identity", verified.startup_identity().to_hex())?;
    result.set_item("interpreter_id", verified.interpreter_id())?;
    Ok(Some(result))
}

/// Read-only evidence from the actual native function's permanent owner. This
/// observes the selected implementation and real entry witness; it never
/// admits a function or infers execution from missing JIT metadata.
pub fn interpreter_function_diagnostics<'py>(
    py: Python<'py>,
    function: &Bound<'py, PyAny>,
) -> PyResult<Option<Bound<'py, PyDict>>> {
    let Some(auth) =
        crate::strict_function::authenticate_borrowed_strict_function(py, function.as_borrowed())?
    else {
        return Ok(None);
    };
    if !auth.is_interpreter() {
        return Ok(None);
    }
    let verified = auth.verified_module();
    let facts = verified.type_facts();
    let result = PyDict::new(py);
    result.set_item("schema", 2)?;
    result.set_item("backend", "cpython")?;
    result.set_item(
        "entry_kind",
        if auth.interpreter_source_authority()? {
            "original_code"
        } else {
            "ordinary_replacement"
        },
    )?;
    result.set_item(
        "original_code_entered",
        auth.data().original_code_entered().ok_or_else(|| {
            strict_runtime_unavailable(py, "native function lost its entry witness")
        })?,
    )?;
    result.set_item("finalized", auth.is_finalized())?;
    result.set_item("native_code_ordinal", auth.native_code_ordinal()?)?;
    result.set_item(
        "source_path",
        verified.source_path().to_string_lossy().as_ref(),
    )?;
    result.set_item("source_sha256", facts.facts().source_digest.to_hex())?;
    result.set_item(
        "artifact_generation",
        facts.generation().fingerprint().to_hex(),
    )?;
    result.set_item("startup_identity", verified.startup_identity().to_hex())?;
    result.set_item("interpreter_id", verified.interpreter_id())?;
    Ok(Some(result))
}
