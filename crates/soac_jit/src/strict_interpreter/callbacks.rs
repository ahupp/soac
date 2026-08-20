//! Explicit callbacks from actual native interpreter operations. No Python
//! attribute, mutable helper or ambient thread-local context conveys authority.

use std::ffi::c_int;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use super::call::{self, CallPhase, InterpreterCallData};
use super::native::{
    self, RawInterpreterCallInfo, RawInterpreterCallView, RawInterpreterFrameInfo,
    RawInterpreterFrameView,
};
use super::{InterpreterInvocationIdentity, RootExecutionData, RootPhase};
use crate::module_type::{SoacExtModule, ensure_module_builtins};
use crate::strict_function::{self, StrictFunctionData};
use crate::strict_interpreter_source::{InterpreterCodeRole, StrictInterpreterSource};
use crate::strict_module::StrictPendingKind;
use crate::strict_namespace::NamespaceExecution;
use crate::strict_runtime_unavailable;
use crate::strict_state::StrictStateRef;

fn status(operation: impl FnOnce(Python<'_>) -> PyResult<()>) -> c_int {
    let py = unsafe { Python::assume_attached() };
    match catch_unwind(AssertUnwindSafe(|| operation(py))) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            error.restore(py);
            -1
        }
        Err(_) => {
            strict_runtime_unavailable(py, "panic in native interpreter contract callback")
                .restore(py);
            -1
        }
    }
}

pub(super) unsafe extern "C" fn root_begin(
    owner: *mut ffi::PyObject,
    module: *mut ffi::PyObject,
    code: *mut ffi::PyObject,
) -> c_int {
    status(|py| {
        if owner.is_null() || module.is_null() || code.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null native module initialization operand",
            ));
        }
        let module = unsafe { Borrowed::<PyAny>::from_ptr(py, module) };
        let owner = StrictStateRef::<RootExecutionData>::from_owner(unsafe {
            Bound::from_borrowed_ptr(py, owner)
        })?;
        let data = owner.data();
        if data.module_identity != module.as_ptr() as usize
            || data.code_identity != code as usize
            || data.phase.get() != RootPhase::Ready
        {
            return Err(strict_runtime_unavailable(
                py,
                "native module initialization owner changed or was replayed",
            ));
        }
        SoacExtModule::with_interpreter_state(&module, |state| {
            let state = state.ok_or_else(|| {
                strict_runtime_unavailable(py, "module has no native execution state")
            })?;
            if state.owner.as_ptr() != owner.owner().as_ptr()
                || !std::sync::Arc::ptr_eq(&state.source, &data.source)
                || !state
                    .runtime
                    .matches_verified_source(state.source.verified())
            {
                return Err(strict_runtime_unavailable(
                    py,
                    "module/source execution identities disagree",
                ));
            }
            Ok(())
        })?;
        let globals = unsafe {
            Borrowed::<PyAny>::from_ptr_or_err(py, ffi::PyModule_GetDict(module.as_ptr()))?
        }
        .cast::<PyDict>()?;
        // The identity snapshot above holds no Rust module-state borrow across
        // validation. The actual loader module supplies this dictionary view.
        data.execution
            .begin_execution(py, &globals, data.source.verified())?;
        data.phase.set(RootPhase::Entering);
        data.execution
            .validate_owner(py, &owner.reference(0)?, data.source.verified())?;
        ensure_module_builtins(globals.as_any())
    })
}

pub(super) unsafe extern "C" fn root_end(owner: *mut ffi::PyObject, succeeded: c_int) {
    // No allocation, Python reference acquisition/release, or exception change.
    unsafe {
        StrictStateRef::<RootExecutionData>::inspect_for_teardown(owner, |data| {
            data.phase.set(if succeeded != 0 {
                RootPhase::Returned
            } else {
                RootPhase::Failed
            });
        });
    }
}

fn require_frame_operands(py: Python<'_>, info: &RawInterpreterFrameInfo) -> PyResult<()> {
    if info.abi_version != 1
        || info.function.is_null()
        || info.code.is_null()
        || info.globals.is_null()
        || info.builtins.is_null()
        || unsafe { ffi::PyFunction_Check(info.function) } == 0
        || unsafe { ffi::PyDict_CheckExact(info.globals) } == 0
    {
        return Err(strict_runtime_unavailable(
            py,
            "incomplete native interpreter frame operands",
        ));
    }
    Ok(())
}

/// These are actual supported frame operands, not pointer identities recovered
/// from metadata. No additional function/code/globals/builtins owner is made.
unsafe fn frame_operands<'a, 'py>(
    py: Python<'py>,
    info: &RawInterpreterFrameInfo,
) -> (
    Borrowed<'a, 'py, PyAny>,
    Borrowed<'a, 'py, PyAny>,
    Borrowed<'a, 'py, PyDict>,
    Borrowed<'a, 'py, PyAny>,
) {
    unsafe {
        (
            Borrowed::from_ptr(py, info.function),
            Borrowed::from_ptr(py, info.code),
            Borrowed::from_ptr(py, info.globals).cast_unchecked(),
            Borrowed::from_ptr(py, info.builtins),
        )
    }
}

/// Authenticate the real native CLASS call, not an arbitrary call to a
/// same-named builtin. CodeNode IDs are correspondence, never body grants.
fn validate_class_site(
    py: Python<'_>,
    source: &StrictInterpreterSource,
    parent: &RawInterpreterFrameInfo,
    expected_body: u32,
) -> PyResult<()> {
    let ordinal = u32::try_from(parent.instruction_ordinal).map_err(|_| {
        strict_runtime_unavailable(py, "native class call has no actual instruction ordinal")
    })?;
    let code = unsafe { Borrowed::from_ptr(py, parent.code) };
    let receipt = source.class_call(py, &code, ordinal)?;
    if receipt.class_body_ordinal() != Some(expected_body) {
        return Err(strict_runtime_unavailable(
            py,
            "class call is not the actual namespace producer",
        ));
    }
    Ok(())
}

pub(super) unsafe extern "C" fn enter(
    kind: u32,
    subject_owner: *mut ffi::PyObject,
    frame: *const RawInterpreterFrameView,
    parent: *const RawInterpreterFrameView,
    new_call_state: *mut *mut ffi::PyObject,
) -> c_int {
    status(|py| {
        if subject_owner.is_null()
            || new_call_state.is_null()
            || !unsafe { *new_call_state }.is_null()
        {
            return Err(strict_runtime_unavailable(
                py,
                "invalid native call-state output",
            ));
        }
        let info = unsafe { native::frame_info(py, frame)? };
        require_frame_operands(py, &info)?;
        if info.phase != native::BINDING || !info.call_state.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "native entry did not precede argument binding",
            ));
        }
        let (function, code, globals, builtins) = unsafe { frame_operands(py, &info) };
        let (source, execution, code_ordinal, namespace, policy) = if kind == native::ROOT {
            if !parent.is_null() || info.locals != info.globals {
                return Err(strict_runtime_unavailable(
                    py,
                    "invalid native root frame shape",
                ));
            }
            let owner = StrictStateRef::<RootExecutionData>::from_owner(unsafe {
                Bound::from_borrowed_ptr(py, subject_owner)
            })?;
            let data = owner.data();
            if data.phase.get() != RootPhase::Entering || data.code_identity != info.code as usize {
                return Err(strict_runtime_unavailable(
                    py,
                    "root call did not consume its initialization attempt",
                ));
            }
            let native_code = data.source.code(py, &code)?;
            if native_code.role() != InterpreterCodeRole::Module {
                return Err(strict_runtime_unavailable(
                    py,
                    "root call is not its original module body",
                ));
            }
            let policy = data
                .execution
                .acquire_owner(py, &globals, data.source.verified())?;
            (
                data.source.clone(),
                data.execution.clone(),
                Some(native_code.ordinal()),
                None,
                policy,
            )
        } else {
            if !matches!(kind, native::FUNCTION | native::CLASS_NAMESPACE) {
                return Err(strict_runtime_unavailable(
                    py,
                    "unknown native function entry kind",
                ));
            }
            let auth = unsafe {
                strict_function::authenticate_interpreter_entry(
                    py, function, code, globals, builtins,
                )?
            };
            if auth.owner().as_ptr() != subject_owner {
                return Err(strict_runtime_unavailable(
                    py,
                    "native entry supplied another function owner",
                ));
            }
            let source = auth.native_source()?.clone();
            let execution = auth.execution_ref().clone();
            let original = auth.interpreter_source_authority()?;
            let native_code = if original {
                Some(source.code(py, &code)?)
            } else {
                None
            };
            let namespace = if kind == native::CLASS_NAMESPACE {
                if parent.is_null() || info.locals.is_null() {
                    return Err(strict_runtime_unavailable(
                        py,
                        "native class body has no actual class operation",
                    ));
                }
                let native_code = native_code.ok_or_else(|| {
                    strict_runtime_unavailable(
                        py,
                        "class operation cannot execute replacement code",
                    )
                })?;
                if native_code.role() != InterpreterCodeRole::ClassNamespace {
                    return Err(strict_runtime_unavailable(
                        py,
                        "class operation has the wrong source body",
                    ));
                }
                let parent_info = unsafe { native::frame_info(py, parent)? };
                let parent_state = unsafe { call::captured_call(py, &parent_info)? };
                let parent_data = parent_state.data();
                if parent_info.phase != native::RUNNING
                    || parent_data.phase.get() != CallPhase::Running
                    || !parent_data.has_source_authority()
                    || !Arc::ptr_eq(&source, &parent_data.source)
                    || !execution.same_execution(&parent_data.execution)
                    || !Arc::ptr_eq(auth.native_birth_execution()?, &parent_data.invocation)
                {
                    return Err(strict_runtime_unavailable(
                        py,
                        "class namespace belongs to another native invocation",
                    ));
                }
                validate_class_site(py, &source, &parent_info, native_code.ordinal())?;
                Some(NamespaceExecution::begin_native(
                    py,
                    source.verified().clone(),
                    execution.clone(),
                    native_code.source().clone(),
                    info.globals as usize,
                    subject_owner as usize,
                )?)
            } else {
                if native_code.is_some_and(|code| {
                    matches!(
                        code.role(),
                        InterpreterCodeRole::ClassNamespace | InterpreterCodeRole::Module
                    )
                }) {
                    return Err(strict_runtime_unavailable(
                        py,
                        "namespace bodies require their explicit native operation",
                    ));
                }
                None
            };
            let code_ordinal = native_code.map(|code| code.ordinal());
            let policy = auth.module_policy_owner()?.unbind();
            (source, execution, code_ordinal, namespace, policy)
        };
        let references = vec![
            unsafe { Bound::from_borrowed_ptr(py, subject_owner) }.unbind(),
            policy,
        ];
        let state = StrictStateRef::new(
            py,
            InterpreterCallData {
                source,
                execution,
                invocation: InterpreterInvocationIdentity::new(),
                namespace,
                kind,
                source_code_ordinal: code_ordinal,
                subject_owner: subject_owner as usize,
                function_identity: info.function as usize,
                code_identity: info.code as usize,
                globals_identity: info.globals as usize,
                builtins_identity: info.builtins as usize,
                locals_identity: info.locals as usize,
                phase: std::cell::Cell::new(CallPhase::Binding),
            },
            references,
        )?;
        // The complete metadata state is the only new reference transferred to
        // native code. Actual execution operands stayed borrowed throughout.
        unsafe {
            *new_call_state = state.owner().clone().into_ptr();
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn birth(
    parent: *const RawInterpreterFrameView,
    function: *mut ffi::PyObject,
    new_owner: *mut *mut ffi::PyObject,
) -> c_int {
    status(|py| {
        if function.is_null()
            || new_owner.is_null()
            || !unsafe { *new_owner }.is_null()
            || unsafe { ffi::PyFunction_Check(function) } == 0
        {
            return Err(strict_runtime_unavailable(
                py,
                "invalid original native function birth",
            ));
        }
        let info = unsafe { native::frame_info(py, parent)? };
        let parent = unsafe { call::captured_call(py, &info)? };
        let data = parent.data();
        if info.phase != native::RUNNING
            || data.phase.get() != CallPhase::Running
            || !data.has_source_authority()
        {
            return Err(strict_runtime_unavailable(
                py,
                "native function birth has no original active parent",
            ));
        }
        let raw = function.cast::<ffi::PyFunctionObject>();
        let code = unsafe { Borrowed::from_ptr(py, (*raw).func_code) };
        let child = data.source.code(py, &code)?;
        if child.parent_ordinal() != data.source_code_ordinal
            || unsafe { (*raw).func_globals } != info.globals
        {
            return Err(strict_runtime_unavailable(
                py,
                "native function birth is not an actual child of this frame",
            ));
        }
        // Function creation captures builtins from the current globals, which
        // may have changed since the parent activation captured its own map.
        // The new owner snapshots and validates the child's actual capture.
        let function = unsafe { Borrowed::from_ptr(py, function) };
        let owner = unsafe {
            strict_function::prepare_interpreter_function_owner(
                py,
                function,
                data.source.clone(),
                data.execution.clone(),
                child.ordinal(),
                data.invocation.clone(),
                data.namespace.clone(),
            )?
        };
        if strict_function::eligible_source_function(data.source.verified(), owner.source()) {
            data.execution.register_interpreter_pending(
                py,
                &parent.reference(call::MODULE_POLICY)?,
                data.source.verified(),
                StrictPendingKind::InterpreterFunction {
                    native_code_ordinal: child.ordinal(),
                },
                &function,
                &data.invocation,
            )?;
        }
        unsafe {
            *new_owner = owner.owner().clone().into_ptr();
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn function_attribute(
    parent: *const RawInterpreterFrameView,
    function: *mut ffi::PyObject,
    flag: u32,
    value: *mut ffi::PyObject,
) -> c_int {
    status(|py| {
        let function = NonNull::new(function)
            .ok_or_else(|| strict_runtime_unavailable(py, "null function attribute target"))?;
        let value = NonNull::new(value)
            .ok_or_else(|| strict_runtime_unavailable(py, "null installed function attribute"))?;
        let info = unsafe { native::frame_info(py, parent)? };
        let parent = unsafe { call::captured_call(py, &info)? };
        let data = parent.data();
        if info.phase != native::RUNNING
            || data.phase.get() != CallPhase::Running
            || !data.has_source_authority()
        {
            return Err(strict_runtime_unavailable(
                py,
                "function attribute has no actual source producer",
            ));
        }
        unsafe {
            strict_function::record_native_function_attribute(
                py,
                function,
                flag,
                value,
                &data.source,
                &data.execution,
                &data.invocation,
            )
        }
    })
}

pub(super) unsafe extern "C" fn started(
    owner: *mut ffi::PyObject,
    frame: *const RawInterpreterFrameView,
) -> c_int {
    status(|py| {
        let info = unsafe { native::frame_info(py, frame)? };
        // No owner guard or operand INCREF on committed native VM entry.
        let marked = unsafe {
            StrictStateRef::<InterpreterCallData>::inspect_live(owner, |data| {
                if info.call_state != owner
                    || info.phase != native::RUNNING
                    || !data.matches_frame(&info)
                    || !data.has_source_authority()
                    || !matches!(data.phase.get(), CallPhase::Binding | CallPhase::Running)
                {
                    return false;
                }
                let recorded = if data.kind == native::ROOT {
                    StrictStateRef::<RootExecutionData>::inspect_live(
                        data.subject_owner as *mut ffi::PyObject,
                        |root| {
                            if !Arc::ptr_eq(&root.source, &data.source)
                                || !root.execution.same_execution(&data.execution)
                                || !matches!(
                                    root.phase.get(),
                                    RootPhase::Entering | RootPhase::Running
                                )
                            {
                                return false;
                            }
                            root.original_code_entered.set(true);
                            root.phase.set(RootPhase::Running);
                            true
                        },
                    )
                    .unwrap_or(false)
                } else {
                    StrictStateRef::<StrictFunctionData>::inspect_live(
                        data.subject_owner as *mut ffi::PyObject,
                        StrictFunctionData::mark_original_code_entered,
                    )
                    .unwrap_or(false)
                };
                if recorded {
                    data.phase.set(CallPhase::Running);
                }
                recorded
            })
        }
        .unwrap_or(false);
        if !marked {
            return Err(strict_runtime_unavailable(
                py,
                "native original-code entry witness is unavailable",
            ));
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn returned(
    owner: *mut ffi::PyObject,
    frame: *const RawInterpreterFrameView,
    value: *mut ffi::PyObject,
) -> c_int {
    status(|py| {
        let _value = NonNull::new(value)
            .ok_or_else(|| strict_runtime_unavailable(py, "null native return operand"))?;
        let info = unsafe { native::frame_info(py, frame)? };
        let state = unsafe { call::captured_call(py, &info)? };
        if state.owner().as_ptr() != owner
            || info.phase != native::RETURNING
            || state.data().phase.get() != CallPhase::Running
            || state.data().kind != native::FUNCTION
            || !state.data().has_source_authority()
        {
            return Err(strict_runtime_unavailable(
                py,
                "native return completion did not follow the original body",
            ));
        }
        state.data().phase.set(CallPhase::Returning);
        super::completion::complete_invocation(py, &state, &info)?;
        Ok(())
    })
}

pub(super) unsafe extern "C" fn failed(
    owner: *mut ffi::PyObject,
    frame: *const RawInterpreterFrameView,
) -> c_int {
    status(|py| {
        let info = unsafe { native::frame_info(py, frame)? };
        let state = unsafe { call::captured_call(py, &info)? };
        if state.owner().as_ptr() != owner
            || info.phase != native::FAILING
            || state.data().kind != native::FUNCTION
            || !state.data().has_source_authority()
        {
            return Err(strict_runtime_unavailable(
                py,
                "native exceptional completion lacks its original call",
            ));
        }
        state.data().phase.set(CallPhase::Failing);
        super::completion::complete_invocation(py, &state, &info)
    })
}

pub(super) unsafe extern "C" fn leave(owner: *mut ffi::PyObject, reason: u32) {
    unsafe {
        StrictStateRef::<InterpreterCallData>::inspect_for_teardown(owner, |data| {
            if reason == native::NAMESPACE_TRANSFERRED && data.kind == native::CLASS_NAMESPACE {
                let completed = data
                    .namespace
                    .as_ref()
                    .is_some_and(|namespace| namespace.complete_native());
                data.phase.set(if completed {
                    CallPhase::NamespaceTransferred
                } else {
                    CallPhase::Retired
                });
            } else {
                data.phase.set(CallPhase::Retired);
                if let Some(namespace) = &data.namespace {
                    namespace.fail_native_unfinished();
                }
            }
        });
    }
}

pub(super) unsafe extern "C" fn definition_store(
    frame: *const RawInterpreterFrameView,
    lane: u32,
    value: *mut ffi::PyObject,
) -> c_int {
    status(|py| {
        let info = unsafe { native::frame_info(py, frame)? };
        let ordinal = u32::try_from(info.instruction_ordinal).map_err(|_| {
            strict_runtime_unavailable(py, "definition Store has no actual ordinal")
        })?;
        let lane = u8::try_from(lane)
            .map_err(|_| strict_runtime_unavailable(py, "definition Store lane is out of range"))?;
        let receipt = unsafe {
            StrictStateRef::<InterpreterCallData>::inspect_live(info.call_state, |data| {
                if !data.matches_frame(&info)
                    || data.phase.get() != CallPhase::Running
                    || !data.has_source_authority()
                {
                    return Err(strict_runtime_unavailable(
                        py,
                        "definition Store lacks an original source activation",
                    ));
                }
                let code = Borrowed::from_ptr(py, info.code);
                data.source
                    .definition_store(py, &code, ordinal, lane)
                    .map(|receipt| receipt.cloned())
            })
        }
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "definition Store call state is unavailable")
        })??;
        let Some(receipt) = receipt else {
            return Ok(());
        };
        let value = NonNull::new(value)
            .ok_or_else(|| strict_runtime_unavailable(py, "null definition Store value"))?;
        let state = unsafe { call::captured_call(py, &info)? };
        super::completion::complete_definition(py, &state, &info, &receipt, unsafe {
            Borrowed::from_ptr(py, value.as_ptr())
        })
    })
}

pub(super) unsafe extern "C" fn prepare_type(
    namespace_state: *mut ffi::PyObject,
    parent: *const RawInterpreterFrameView,
    call_info: *const RawInterpreterCallInfo,
    call_view: *const RawInterpreterCallView,
    function: *mut ffi::PyObject,
    metaclass: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    bases: *mut ffi::PyObject,
    namespace: *mut ffi::PyObject,
    keywords: *mut ffi::PyObject,
    new_handle: *mut *mut ffi::PyObject,
) -> c_int {
    status(|py| {
        if namespace_state.is_null()
            || call_info.is_null()
            || call_view.is_null()
            || function.is_null()
            || metaclass.is_null()
            || name.is_null()
            || bases.is_null()
            || namespace.is_null()
            || new_handle.is_null()
            || !unsafe { *new_handle }.is_null()
            || unsafe { ffi::PyTuple_CheckExact(bases) } == 0
            || (!keywords.is_null() && unsafe { ffi::PyDict_CheckExact(keywords) } == 0)
        {
            return Err(strict_runtime_unavailable(
                py,
                "invalid native type construction operands",
            ));
        }
        let state = StrictStateRef::<InterpreterCallData>::from_owner(unsafe {
            Bound::from_borrowed_ptr(py, namespace_state)
        })?;
        let data = state.data();
        let execution = data.namespace.as_ref().ok_or_else(|| {
            strict_runtime_unavailable(py, "native type has no namespace execution")
        })?;
        if data.kind != native::CLASS_NAMESPACE
            || data.phase.get() != CallPhase::NamespaceTransferred
            || !execution.is_completed()
            || data.function_identity != function as usize
            || data.locals_identity != namespace as usize
        {
            return Err(strict_runtime_unavailable(
                py,
                "native namespace completion was not transferred to this constructor",
            ));
        }
        let parent_info = unsafe { native::frame_info(py, parent)? };
        let parent_state = unsafe { call::captured_call(py, &parent_info)? };
        let parent_data = parent_state.data();
        if !Arc::ptr_eq(&data.source, &parent_data.source)
            || !data.execution.same_execution(&parent_data.execution)
            || parent_data.phase.get() != CallPhase::Running
        {
            return Err(strict_runtime_unavailable(
                py,
                "native class caller changed execution",
            ));
        }
        validate_class_site(
            py,
            &data.source,
            &parent_info,
            data.source_code_ordinal.ok_or_else(|| {
                strict_runtime_unavailable(py, "native class lost original-code authority")
            })?,
        )?;
        let function = unsafe { Borrowed::from_ptr(py, function) };
        let auth = strict_function::authenticate_borrowed_strict_function(py, function)?
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "native class function lost its actual owner")
            })?;
        if auth.owner().as_ptr() as usize != data.subject_owner
            || !auth.interpreter_source_authority()?
            || !Arc::ptr_eq(auth.native_birth_execution()?, &parent_data.invocation)
        {
            return Err(strict_runtime_unavailable(
                py,
                "native class function belongs to another birth",
            ));
        }
        let metaclass = unsafe { Borrowed::from_ptr(py, metaclass) };
        let name = unsafe { Borrowed::from_ptr(py, name) };
        let bases: Borrowed<'_, '_, PyTuple> =
            unsafe { Borrowed::from_ptr(py, bases).cast_unchecked() };
        let namespace = unsafe { Borrowed::from_ptr(py, namespace) };
        let keywords: Option<Borrowed<'_, '_, PyDict>> = if keywords.is_null() {
            None
        } else {
            Some(unsafe { Borrowed::from_ptr(py, keywords).cast_unchecked() })
        };
        // Dynamic metaclasses and non-dict namespace adapters decline before
        // any participating transform or irreversible source method policy.
        if metaclass.as_ptr() != std::ptr::addr_of_mut!(ffi::PyType_Type).cast()
            || unsafe { ffi::PyDict_CheckExact(namespace.as_ptr()) } == 0
        {
            return Ok(());
        }
        let dictionary = namespace.cast::<PyDict>()?;
        let prepared = unsafe {
            super::call_join::prepare_class_transform(
                py,
                &parent_state,
                call_info,
                call_view,
                &auth,
                execution,
                &dictionary,
                &bases,
                None,
            )?
        };
        let dataclass = match prepared.transform {
            super::call_join::ClassTransform::Declined => return Ok(()),
            super::call_join::ClassTransform::Ordinary => None,
            super::call_join::ClassTransform::Dataclass(proof) => Some(proof),
        };
        let handle = crate::strict_class::prepare_interpreter_type_handle(
            py,
            &auth,
            execution,
            &prepared.completion_invocation,
            &function,
            &metaclass,
            &name,
            &bases,
            &namespace,
            keywords.as_deref(),
            dataclass.as_ref(),
        )?;
        if let Some(handle) = handle {
            unsafe {
                *new_handle = handle.into_ptr();
            }
        }
        Ok(())
    })
}
