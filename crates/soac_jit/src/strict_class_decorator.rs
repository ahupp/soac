//! Source-selected class-transform operands and invocation lifetime.
//!
//! A proposal never authenticates the actual decorator or its generated code.
//! Unless an individual invocation has a native adapter witness, construction
//! stays dynamic. The carrier owns the already evaluated decorator and its
//! optional, explicitly traversed invocation until Apply/Discard, and
//! application temporarily transfers that edge to the ordinary callable
//! operand. The enclosing explicit cleanup region discards it after argument
//! cleanup. Factory arguments and the factory keep their ordinary call lifetimes.

use std::cell::Cell;
use std::ffi::{CStr, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use soac_contracts::{
    ClassTypeFact, DecoratorKind, ParticipationProposal, TransformKind, UncertaintyReason,
};
use soac_core::block_py::{CallableSourceRole, RuntimeFunctionId};

use crate::strict_function::{AuthenticatedStrictFunction, ClassConstructionCaptures};
use crate::strict_namespace::NamespaceExecution;
use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{FunctionEnvAbiHeader, StrictModuleExecutionRef, strict_runtime_unavailable};

pub(crate) const PREPARE_CLASS_DECORATOR_SYMBOL: &str = "soac_jit_prepare_class_decorator";
pub(crate) const PREPARE_CLASS_DECORATOR_UNPACKED_SYMBOL: &str =
    "soac_jit_prepare_class_decorator_unpacked";
pub(crate) const APPLY_CLASS_DECORATOR_SYMBOL: &str = "soac_jit_apply_class_decorator";
pub(crate) const DISCARD_CLASS_DECORATOR_SYMBOL: &str = "soac_jit_discard_class_decorator";

unsafe extern "C" {
    fn PySoac_VectorcallWithContext(
        callable: *mut ffi::PyObject,
        args: *const *mut ffi::PyObject,
        nargsf: usize,
        kwnames: *mut ffi::PyObject,
        globals: *mut ffi::PyObject,
        locals: *mut ffi::PyObject,
        builtins: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn _PyStack_UnpackDict(
        tstate: *mut ffi::PyThreadState,
        args: *const *mut ffi::PyObject,
        nargs: ffi::Py_ssize_t,
        kwargs: *mut ffi::PyObject,
        kwnames: *mut *mut ffi::PyObject,
    ) -> *const *mut ffi::PyObject;
    fn _PyStack_UnpackDict_FreeNoDecRef(
        args: *const *mut ffi::PyObject,
        kwnames: *mut ffi::PyObject,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    Constructing,
    Constructed,
    Applying,
    Applied,
    Discarded,
    Failed,
}

/// Rust-only source/execution coordinates. They select a proposal, never a
/// capability for the Python decorator, class, or any generated method.
struct Site {
    fact: ClassTypeFact,
    construction_function: RuntimeFunctionId,
    verified: std::sync::Arc<crate::VerifiedStrictModule>,
    execution: StrictModuleExecutionRef,
    globals_identity: usize,
}

struct PreparationData {
    site: Site,
    phase: Cell<Phase>,
    // Compare only while Apply owns the actual result of this construction.
    // Dynamic metaclasses may return non-types, so this is not a type witness.
    constructed_identity: Cell<usize>,
}

unsafe impl StrictStateData for PreparationData {
    const TYPE_NAME: &'static CStr = c"soac._ClassDecoratorPreparation";

    fn on_terminal(&self) {
        self.phase.set(Phase::Failed);
        self.constructed_identity.set(0);
    }
}

const DECORATOR: usize = 0;
const DATACLASS: usize = 1;

/// # Safety
/// `environment` is the current compiler-passed active FunctionEnv ABI header,
/// kept alive throughout this call. No Python value supplies this pointer.
unsafe fn active_site(
    py: Python<'_>,
    environment: *const c_void,
    construction_function: RuntimeFunctionId,
) -> PyResult<Site> {
    let header = unsafe { environment.cast::<FunctionEnvAbiHeader>().as_ref() }
        .ok_or_else(|| strict_runtime_unavailable(py, "class decorator has no active frame"))?;
    let active = unsafe { header.active_strict_call.as_ref() }.ok_or_else(|| {
        strict_runtime_unavailable(py, "class decorator frame is unauthenticated")
    })?;
    if active.environment().header() as *const FunctionEnvAbiHeader != header as *const _ {
        return Err(strict_runtime_unavailable(
            py,
            "class decorator frame identity changed",
        ));
    }
    // Use the owner captured at entry. A callback may replace an unsealed
    // function's idle code without changing the already-running frame.
    let owner = active.captured_owner(py)?;
    let shared = active.active_module_state().ok_or_else(|| {
        strict_runtime_unavailable(py, "class decorator frame lost its module execution")
    })?;
    let verified = shared.strict_module.as_ref().ok_or_else(|| {
        strict_runtime_unavailable(py, "class decorator frame has no verified source")
    })?;
    let constructor = shared
        .lookup_function(construction_function)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "class decorator construction template is absent")
        })?;
    let origin = constructor
        .scope
        .source_origin
        .as_ref()
        .filter(|origin| origin.role == CallableSourceRole::ClassConstruction)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "class decorator has the wrong source role")
        })?;
    let caller = owner.source().ok_or_else(|| {
        strict_runtime_unavailable(py, "class decorator caller has no source definition")
    })?;
    if caller.definition.module != origin.definition.module
        || caller.definition.source_range.start > origin.definition.source_range.start
        || caller.definition.source_range.end < origin.definition.source_range.end
        || !owner
            .module_source()?
            .matches_verified(verified.type_facts())
    {
        return Err(strict_runtime_unavailable(
            py,
            "class decorator belongs to another source frame",
        ));
    }
    let fact = verified
        .type_facts()
        .facts()
        .classes
        .iter()
        .find(|fact| fact.identity == origin.definition)
        .ok_or_else(|| strict_runtime_unavailable(py, "class decorator proposal is absent"))?;
    if fact.participation != ParticipationProposal::Candidate
        || fact
            .uncertainty
            .iter()
            .any(|reason| *reason != UncertaintyReason::OpenWorld)
        || !matches!(fact.decorators.as_slice(), [decorator]
            if decorator.kind == DecoratorKind::StdlibDataclass && decorator.uncertainty.is_empty())
        || !fact.transform.as_ref().is_some_and(|transform| {
            transform.kind == TransformKind::StdlibDataclass
                && transform.dataclass_options.is_some()
        })
    {
        return Err(strict_runtime_unavailable(
            py,
            "class decorator has no selected transform proposal",
        ));
    }
    let globals = owner.global_dictionary()?;
    if globals.as_ptr() != header.globals_obj {
        return Err(strict_runtime_unavailable(
            py,
            "class decorator globals changed",
        ));
    }
    owner
        .execution()
        .validate_owner(py, &owner.module_policy_owner()?, verified)?;
    Ok(Site {
        fact: fact.clone(),
        construction_function,
        verified: std::sync::Arc::clone(verified),
        execution: owner.execution().clone(),
        globals_identity: globals.as_ptr() as usize,
    })
}

pub(crate) struct ConstructionDecorator<'py> {
    state: StrictStateRef<'py, PreparationData>,
}

impl<'py> ConstructionDecorator<'py> {
    /// Called only after the actual source namespace body completed. Prepare
    /// did not create that function early or lend authority to arbitrary
    /// decorator results; this consumes the exact recorded namespace execution.
    pub(crate) fn prepare_dataclass(
        &self,
        auth: &AuthenticatedStrictFunction<'_, 'py>,
        namespace: &Bound<'py, PyDict>,
        bases: &Bound<'py, PyTuple>,
        execution: &Arc<NamespaceExecution>,
        construction_captures: Option<&ClassConstructionCaptures<'py>>,
    ) -> PyResult<Option<crate::strict_dataclass::DataclassConstruction<'py>>> {
        self.state.ensure_live()?;
        if self.state.data().phase.get() != Phase::Constructing {
            return Err(strict_runtime_unavailable(
                namespace.py(),
                "dataclass preparation is outside its construction",
            ));
        }
        let owner = self.state.reference(DATACLASS)?;
        if owner.is_none() {
            return Ok(None);
        }
        crate::strict_dataclass::prepare_construction(
            namespace.py(),
            &owner,
            auth,
            namespace,
            bases,
            execution,
            construction_captures,
        )
    }

    /// Unsupported actual metaclass/namespace/base/layout graphs decline
    /// before the ordinary constructor is called. An already-bound invocation
    /// cannot use this operation to revoke its native class protection.
    pub(crate) fn decline_dataclass(&self) -> PyResult<()> {
        self.state.ensure_live()?;
        let owner = self.state.reference(DATACLASS)?;
        if owner.is_none() {
            return Ok(());
        }
        crate::strict_dataclass::decline(&owner)?;
        self.state.set_reference(
            DATACLASS,
            self.state
                .owner()
                .py()
                .None()
                .into_bound(self.state.owner().py()),
        )
    }

    pub(crate) fn complete(&self, class: &Bound<'_, PyAny>) -> PyResult<()> {
        self.state.ensure_live()?;
        if self.state.data().phase.get() != Phase::Constructing {
            return Err(strict_runtime_unavailable(
                class.py(),
                "class decorator construction was interrupted",
            ));
        }
        self.state
            .data()
            .constructed_identity
            .set(class.as_ptr() as usize);
        self.state.data().phase.set(Phase::Constructed);
        Ok(())
    }
}

impl Drop for ConstructionDecorator<'_> {
    fn drop(&mut self) {
        if self.state.data().phase.get() == Phase::Constructing {
            self.state.data().phase.set(Phase::Failed);
            if let Ok(owner) = self.state.reference(DATACLASS) {
                if !owner.is_none() {
                    let _ = crate::strict_dataclass::discard(&owner);
                }
            }
        }
    }
}

/// Consume only this source/execution's preparation. The returned guard has
/// not yet admitted a class: its actual body/bases/metaclass remain to validate.
pub(crate) fn begin_construction<'py>(
    py: Python<'py>,
    preparation: &Bound<'py, PyAny>,
    construction_function: RuntimeFunctionId,
    namespace: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<ConstructionDecorator<'py>> {
    let state = StrictStateRef::<PreparationData>::from_owner(preparation.clone())?;
    let data = state.data();
    if data.phase.get() != Phase::Prepared
        || data.site.construction_function != construction_function
        || namespace.origin().map(|origin| &origin.definition) != Some(&data.site.fact.identity)
        || !std::sync::Arc::ptr_eq(&data.site.verified, namespace.verified_module())
        || data.site.globals_identity != namespace.globals()?.as_ptr() as usize
    {
        data.phase.set(Phase::Failed);
        return Err(strict_runtime_unavailable(
            py,
            "class decorator preparation was replayed or transferred",
        ));
    }
    data.site.execution.validate_owner(
        py,
        &namespace.module_policy_owner()?,
        namespace.verified_module(),
    )?;
    data.phase.set(Phase::Constructing);
    Ok(ConstructionDecorator { state })
}

fn callback_result<'py>(
    py: Python<'py>,
    operation: impl FnOnce() -> PyResult<Bound<'py, PyAny>>,
) -> *mut ffi::PyObject {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => value.into_ptr(),
        Ok(Err(error)) => {
            error.restore(py);
            ptr::null_mut()
        }
        Err(_) => {
            strict_runtime_unavailable(py, "panic in class decorator boundary").restore(py);
            ptr::null_mut()
        }
    }
}

/// The caller has evaluated the callable and every argument exactly once and
/// keeps their ordinary operand roots until this helper returns. Selection and
/// any invocation authentication happen before a factory is called, so fresh
/// decorator closures can receive their creation witness before escaping.
pub(crate) unsafe extern "C" fn prepare_class_decorator(
    construction_function: u64,
    environment: *const c_void,
    factory: i32,
    callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: usize,
    kwnames: *mut ffi::PyObject,
    frame_namespace: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    callback_result(py, || {
        if callable.is_null() || !matches!(factory, 0 | 1) {
            return Err(strict_runtime_unavailable(
                py,
                "invalid class decorator preparation operand",
            ));
        }
        let site = unsafe {
            active_site(
                py,
                environment,
                RuntimeFunctionId::from_packed_runtime_u64(construction_function),
            )?
        };
        if factory == 0 && (nargs != 0 || !kwnames.is_null()) {
            return Err(strict_runtime_unavailable(
                py,
                "bare class decorator preparation has call arguments",
            ));
        }
        let callable_value = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, callable) };
        let globals = unsafe {
            Bound::<PyAny>::from_borrowed_ptr(py, site.globals_identity as *mut ffi::PyObject)
        };
        let prepared = unsafe {
            crate::strict_dataclass::prepare(
                py,
                &site.fact,
                &globals,
                factory == 1,
                &callable_value,
                args,
                nargs,
                kwnames,
            )?
        };
        let (decorator, adapter) = if let Some(prepared) = prepared {
            (prepared.decorator, Some(prepared.owner))
        } else if factory == 1 {
            let decorator = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    PySoac_VectorcallWithContext(
                        callable,
                        args,
                        nargs,
                        kwnames,
                        site.globals_identity as *mut ffi::PyObject,
                        frame_namespace,
                        (*environment.cast::<FunctionEnvAbiHeader>()).builtins_obj,
                    ),
                )?
            };
            (decorator, None)
        } else {
            (callable_value.clone(), None)
        };
        let state = match StrictStateRef::new(
            py,
            PreparationData {
                site,
                phase: Cell::new(Phase::Prepared),
                constructed_identity: Cell::new(0),
            },
            vec![py.None(), py.None()],
        ) {
            Ok(state) => state,
            Err(error) => {
                if let Some(owner) = adapter {
                    let _ = crate::strict_dataclass::discard(&owner);
                }
                return Err(error);
            }
        };
        state.set_reference(DECORATOR, decorator)?;
        if let Some(owner) = adapter {
            state.bind_reserved_reference(DATACLASS, owner)?;
        }
        Ok(state.owner().clone())
    })
}

/// CALL_FUNCTION_EX already owns its positional tuple and keyword dictionary.
/// Borrow their existing entries into the same raw preparation boundary rather
/// than create another observable Python argument container. Plain calls never
/// use this bridge.
pub(crate) unsafe extern "C" fn prepare_class_decorator_unpacked(
    construction_function: u64,
    environment: *const c_void,
    factory: i32,
    callable: *mut ffi::PyObject,
    positional: *mut ffi::PyObject,
    keywords: *mut ffi::PyObject,
    frame_namespace: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    callback_result(py, || {
        if positional.is_null()
            || unsafe { ffi::PyTuple_CheckExact(positional) } == 0
            || (!keywords.is_null() && unsafe { ffi::PyDict_CheckExact(keywords) } == 0)
        {
            return Err(strict_runtime_unavailable(
                py,
                "invalid unpacked class decorator operands",
            ));
        }
        let count = unsafe { ffi::PyTuple_Size(positional) };
        let arguments = (0..count)
            .map(|index| unsafe { ffi::PyTuple_GetItem(positional, index) })
            .collect::<Vec<_>>();
        let mut names = ptr::null_mut();
        let values = if !keywords.is_null() && unsafe { ffi::PyDict_Size(keywords) } != 0 {
            unsafe {
                _PyStack_UnpackDict(
                    ffi::PyThreadState_Get(),
                    arguments.as_ptr(),
                    count,
                    keywords,
                    &mut names,
                )
            }
        } else {
            arguments.as_ptr()
        };
        if values.is_null() {
            return Err(PyErr::fetch(py));
        }
        let result = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                prepare_class_decorator(
                    construction_function,
                    environment,
                    factory,
                    callable,
                    values,
                    arguments.len(),
                    names,
                    frame_namespace,
                ),
            )
        };
        if !names.is_null() {
            // The dictionary still owns the values. Only the native unpacker's
            // appended keyword roots and its names tuple belong to this bridge.
            let keyword_count = unsafe { ffi::PyTuple_Size(names) };
            for index in (0..keyword_count).rev() {
                unsafe { ffi::Py_DECREF(*values.add(arguments.len() + index as usize)) };
            }
            unsafe { _PyStack_UnpackDict_FreeNoDecRef(values, names) };
        }
        result
    })
}

pub(crate) unsafe extern "C" fn apply_class_decorator(
    construction_function: u64,
    environment: *const c_void,
    preparation: *mut ffi::PyObject,
    class: *mut ffi::PyObject,
    frame_namespace: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    callback_result(py, || {
        if preparation.is_null() || class.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null class decorator application operand",
            ));
        }
        let current = unsafe {
            active_site(
                py,
                environment,
                RuntimeFunctionId::from_packed_runtime_u64(construction_function),
            )?
        };
        let state = StrictStateRef::<PreparationData>::from_owner(unsafe {
            Bound::from_borrowed_ptr(py, preparation)
        })?;
        let data = state.data();
        if data.phase.get() != Phase::Constructed
            || data.constructed_identity.get() != class as usize
            || data.site.fact.identity != current.fact.identity
            || data.site.construction_function != current.construction_function
            || data.site.globals_identity != current.globals_identity
            || !data.site.execution.same_execution(&current.execution)
            || !std::sync::Arc::ptr_eq(&data.site.verified, &current.verified)
        {
            data.phase.set(Phase::Failed);
            return Err(strict_runtime_unavailable(
                py,
                "class decorator application was replayed or transferred",
            ));
        }
        let decorator = state.reference(DECORATOR)?;
        data.phase.set(Phase::Applying);
        data.constructed_identity.set(0);
        // During the call, own exactly the ordinary callable operand rather
        // than a second carrier edge. After the call, transfer it back so the
        // explicit Discard operation runs after the caller's argument cleanup.
        state.set_reference(DECORATOR, py.None().into_bound(py))?;
        let adapter = state.reference(DATACLASS)?;
        let class_value = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, class) };
        let mut replacement_for_cleanup = None;
        let result = (|| {
            if !adapter.is_none() {
                if let Some(result) =
                    crate::strict_dataclass::apply(&adapter, &decorator, &class_value)?
                {
                    if result.as_ptr() != class_value.as_ptr() {
                        replacement_for_cleanup = Some(result.clone());
                    }
                    let actual = crate::strict_class_state::for_constructed_type(py, &result)?
                        .ok_or_else(|| {
                            strict_runtime_unavailable(
                                py,
                                "dataclass application returned an unowned class",
                            )
                        })?;
                    if actual.source() != &current.fact.identity || !actual.pending_dataclass() {
                        return Err(strict_runtime_unavailable(
                            py,
                            "dataclass application lost its pending construction",
                        ));
                    }
                    let completed = crate::strict_dataclass::complete_application(
                        &adapter,
                        &class_value,
                        &result,
                    )?;
                    let globals = unsafe {
                        Bound::<PyAny>::from_borrowed_ptr(
                            py,
                            current.globals_identity as *mut ffi::PyObject,
                        )
                    }
                    .cast_into::<PyDict>()?;
                    let kind = crate::strict_module::StrictPendingKind::Class {
                        source: current.fact.identity.clone(),
                    };
                    if result.as_ptr() != class_value.as_ptr() {
                        // Prepare the selected weak edge while both native
                        // lineages are still pending. Keep the original record
                        // intact if allocation fails; no instance is admitted.
                        current.execution.register_pending_before(
                            py,
                            &globals,
                            &current.verified,
                            kind.clone(),
                            &result,
                            &class_value,
                        )?;
                    }
                    if !crate::strict_class::admit_class(py, &result, &current.fact.identity)? {
                        return Err(strict_runtime_unavailable(
                            py,
                            "completed dataclass lost its selected pending type",
                        ));
                    }
                    if result.as_ptr() != class_value.as_ptr() {
                        // Preserve the ordinary retained operand roots in
                        // completed until the exact lineage disposition ends.
                        // Never finalize or retarget the provisional original.
                        let original =
                            crate::strict_class_state::for_constructed_type(py, &class_value)?
                                .ok_or_else(|| {
                                    strict_runtime_unavailable(
                                        py,
                                        "unselected dataclass original lost its construction",
                                    )
                                })?;
                        original.dispose_unselected_provisional()?;
                        current.execution.remove_pending(
                            py,
                            &globals,
                            &current.verified,
                            &kind,
                            &class_value,
                        )?;
                    }
                    if current
                        .execution
                        .is_sealed(py, &globals, &current.verified)?
                    {
                        if !crate::strict_class::finalize_class(
                            py,
                            &result,
                            &current.fact.identity,
                        )? {
                            return Err(strict_runtime_unavailable(
                                py,
                                "selected dataclass could not finish class finalization",
                            ));
                        }
                        current.execution.remove_pending(
                            py,
                            &globals,
                            &current.verified,
                            &kind,
                            &result,
                        )?;
                    }
                    drop(completed);
                    return Ok(result);
                }
            }
            let args = [class];
            unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    PySoac_VectorcallWithContext(
                        decorator.as_ptr(),
                        args.as_ptr(),
                        args.len(),
                        ptr::null_mut(),
                        current.globals_identity as *mut ffi::PyObject,
                        frame_namespace,
                        (*environment.cast::<FunctionEnvAbiHeader>()).builtins_obj,
                    ),
                )
            }
        })();
        if result.is_err() && !adapter.is_none() {
            // Cleanup preserves the actual application error. Removing only
            // this weak pending record prevents a caught failure from poisoning
            // unrelated later module drains; installed native protection is
            // never removed, even when the failed class escaped a callback.
            if replacement_for_cleanup.is_none() {
                replacement_for_cleanup = crate::strict_dataclass::failed_replacement(&adapter)
                    .ok()
                    .flatten();
            }
            let _ = crate::strict_dataclass::discard(&adapter);
            let _ = forget_failed_dataclass(&current, &class_value);
            if let Some(class) = &replacement_for_cleanup {
                let _ = forget_failed_dataclass(&current, class);
            }
        }
        state.bind_reserved_reference(DECORATOR, decorator)?;
        if data.phase.get() != Phase::Applying {
            data.phase.set(Phase::Failed);
            return Err(strict_runtime_unavailable(
                py,
                "class decorator application was interrupted",
            ));
        }
        data.phase.set(if result.is_ok() {
            Phase::Applied
        } else {
            Phase::Failed
        });
        result
    })
}

fn forget_failed_dataclass(site: &Site, class: &Bound<'_, PyAny>) -> PyResult<()> {
    let py = class.py();
    let terminalized = (|| -> PyResult<()> {
        if let Some(state) = crate::strict_class_state::for_constructed_type(py, class)? {
            if state.source() != &site.fact.identity || state.dataclass_namespace()?.is_none() {
                return Err(strict_runtime_unavailable(
                    py,
                    "failed dataclass cleanup has another construction",
                ));
            }
            state.fail_unfinished_type()?;
        }
        Ok(())
    })();
    // Native failure after permanent publication makes the owner query above
    // fallible too. Even then the authenticated Site and caller-supported actual
    // original/result operand identify this exact weak receipt. Removing it
    // neither grants execution nor revokes any installed contract.
    let globals = unsafe {
        Bound::<PyAny>::from_borrowed_ptr(py, site.globals_identity as *mut ffi::PyObject)
    }
    .cast_into::<PyDict>()?;
    let removed = site.execution.remove_pending(
        py,
        &globals,
        &site.verified,
        &crate::strict_module::StrictPendingKind::Class {
            source: site.fact.identity.clone(),
        },
        class,
    );
    // Attempt removal before returning either cleanup error. The caller keeps
    // the original application error and never substitutes this secondary one.
    terminalized.and(removed.map(|_| ()))
}

/// Exercise the cleanup-only path with an actual authenticated retained
/// function/execution and a caller-supported native failed operand. This does
/// not authorize construction or supply an invented compiler function ID.
#[cfg(test)]
pub(crate) fn forget_failed_registered_class_for_test(
    auth: &AuthenticatedStrictFunction<'_, '_>,
    fact: &ClassTypeFact,
    class: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let globals = auth.globals()?;
    let site = Site {
        fact: fact.clone(),
        construction_function: auth.function_id()?,
        verified: Arc::clone(auth.verified_module()),
        execution: auth.execution_ref().clone(),
        globals_identity: globals.as_ptr() as usize,
    };
    forget_failed_dataclass(&site, class)
}

/// Clear the operand edge even if the private preparation object escaped. This
/// operation grants no authority, so cleanup does not reauthenticate the active
/// source frame (which may itself be unwinding a terminal-runtime error).
pub(crate) unsafe extern "C" fn discard_class_decorator(
    preparation: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    callback_result(py, || {
        if preparation.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null class decorator discard operand",
            ));
        }
        let state = StrictStateRef::<PreparationData>::from_owner(unsafe {
            Bound::from_borrowed_ptr(py, preparation)
        })?;
        let data = state.data();
        if data.phase.get() == Phase::Applying {
            // A reentrant private operation cannot finish an active call. Its
            // caller will restore the callable edge and unwind to this cleanup.
            data.phase.set(Phase::Failed);
            return Err(strict_runtime_unavailable(
                py,
                "class decorator discarded during application",
            ));
        }
        data.phase.set(Phase::Discarded);
        data.constructed_identity.set(0);
        let adapter = state.reference(DATACLASS)?;
        let discarded = if adapter.is_none() {
            Ok(())
        } else {
            crate::strict_dataclass::discard(&adapter)
        };
        let cleared_adapter = state.set_reference(DATACLASS, py.None().into_bound(py));
        let cleared_decorator = state.set_reference(DECORATOR, py.None().into_bound(py));
        discarded.and(cleared_adapter).and(cleared_decorator)?;
        Ok(py.None().into_bound(py))
    })
}
