//! Actual native CALL operands joined to the same authenticated source map.
//!
//! No call operand, Python frame, code, globals, descriptor, class or result is
//! stored here. Callback-local borrowed views and existing metadata edges are
//! retired before the evaluator resumes. A missing source receipt is not a
//! class/decorator/factory grant and never triggers a name/stack scan.

use std::ffi::c_int;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple};
use soac_contracts::{
    ClassTypeFact, DecoratorKind, ParticipationProposal, SourceRange, StdlibDataclassPolicy,
    TransformKind,
};

use super::InterpreterInvocationIdentity;
use super::call::{self, CallPhase, InterpreterCall};
use super::native::{
    self, RawInterpreterCallDecision, RawInterpreterCallInfo, RawInterpreterCallOperand,
    RawInterpreterCallSite, RawInterpreterCallView, RawInterpreterFrameInfo,
    RawInterpreterFrameView,
};
use crate::strict_dataclass;
use crate::strict_function::{self, AuthenticatedStrictFunction, ClassConstructionCaptures};
use crate::strict_interpreter_source::{
    InterpreterCallChannel, InterpreterCallForm, InterpreterCallReceipt, InterpreterCallRole,
    InterpreterCodeRole,
};
use crate::strict_namespace::NamespaceExecution;
use crate::strict_runtime_unavailable;

const ROOT_FACTORY: u32 = 1;
const ROOT_APPLY: u32 = 2;
const CREATED_DECORATOR: u32 = 256;

fn callback(operation: impl FnOnce(Python<'_>) -> PyResult<()>) -> c_int {
    let py = unsafe { Python::assume_attached() };
    match catch_unwind(AssertUnwindSafe(|| operation(py))) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            error.restore(py);
            -1
        }
        Err(_) => {
            strict_runtime_unavailable(py, "panic in native CALL contract join").restore(py);
            -1
        }
    }
}

fn count(py: Python<'_>, value: ffi::Py_ssize_t) -> PyResult<usize> {
    usize::try_from(value).map_err(|_| strict_runtime_unavailable(py, "negative native CALL count"))
}

fn ordinal(py: Python<'_>, frame: &RawInterpreterFrameInfo) -> PyResult<u32> {
    u32::try_from(frame.instruction_ordinal)
        .map_err(|_| strict_runtime_unavailable(py, "native CALL has no exact instruction ordinal"))
}

/// No physical-location or owner-bearing operand is guessed. In particular a
/// non-NULL runtime channel does not decide MethodSelf versus LeadingArgument.
fn shape_matches(receipt: &InterpreterCallReceipt, site: &RawInterpreterCallSite) -> bool {
    if site.reserved != 0 || site.positional_count < 0 || site.keyword_count < 0 {
        return false;
    }
    let form = match receipt.form {
        InterpreterCallForm::Positional => native::CALL_VECTOR,
        InterpreterCallForm::Keywords => native::CALL_VECTOR_KW,
        InterpreterCallForm::Expanded => native::CALL_EXPANDED,
    };
    if site.form != form {
        return false;
    }
    let channel = match receipt.input.channel {
        InterpreterCallChannel::Null => site.channel == native::CALL_NULL_CHANNEL,
        InterpreterCallChannel::LeadingArgument => site.channel == native::CALL_VALUE_CHANNEL,
        InterpreterCallChannel::MethodSelfOrNull => matches!(
            site.channel,
            native::CALL_NULL_CHANNEL | native::CALL_VALUE_CHANNEL
        ),
    };
    if !channel {
        return false;
    }
    if form == native::CALL_EXPANDED {
        return receipt.native_value_argument_count.is_none()
            && site.instruction_argument == 0
            && site.channel == native::CALL_NULL_CHANNEL;
    }
    let Some(total) = site.positional_count.checked_add(site.keyword_count) else {
        return false;
    };
    receipt.native_value_argument_count == Some(site.instruction_argument)
        && total
            == i64::from(site.instruction_argument) as ffi::Py_ssize_t
                + (site.channel == native::CALL_VALUE_CHANNEL) as ffi::Py_ssize_t
        && if form == native::CALL_VECTOR {
            site.keyword_count == 0
        } else {
            receipt
                .input
                .keyword_names
                .as_ref()
                .is_some_and(|names| names.len() == site.keyword_count as usize)
        }
}

unsafe fn actual_call<'py>(
    py: Python<'py>,
    site: &RawInterpreterCallSite,
) -> PyResult<(InterpreterCall<'py>, RawInterpreterFrameInfo)> {
    let info = unsafe { native::frame_info(py, site.frame)? };
    let state = unsafe { call::captured_call(py, &info)? };
    if info.phase != native::RUNNING
        || state.data().phase.get() != CallPhase::Running
        || !state.data().has_source_authority()
    {
        return Err(strict_runtime_unavailable(
            py,
            "native CALL has no active original source",
        ));
    }
    Ok((state, info))
}

fn receipt<'a>(
    py: Python<'_>,
    state: &'a InterpreterCall<'_>,
    frame: &RawInterpreterFrameInfo,
    site: &RawInterpreterCallSite,
) -> PyResult<Option<&'a InterpreterCallReceipt>> {
    let code = unsafe { Borrowed::<PyAny>::from_ptr_or_err(py, frame.code)? };
    let selected = state.data().source.call(py, &code, ordinal(py, frame)?)?;
    if selected.is_some_and(|receipt| !shape_matches(receipt, site)) {
        return Err(strict_runtime_unavailable(
            py,
            "native CALL operands differ from their exact source receipt",
        ));
    }
    Ok(selected)
}

unsafe fn info<'a>(
    py: Python<'_>,
    raw: *const RawInterpreterCallInfo,
    phase: u32,
) -> PyResult<&'a RawInterpreterCallInfo> {
    let value = unsafe { raw.as_ref() }
        .ok_or_else(|| strict_runtime_unavailable(py, "null native CALL info"))?;
    if value.abi_version != 1
        || value.phase != phase
        || value.reserved != 0
        || value.current.frame.is_null()
        || value.decorator_count < 0
        || (phase == native::CALL_SELECT
            && (value.decorator_source != native::DECORATORS_NONE || value.decorator_count != 0))
    {
        return Err(strict_runtime_unavailable(
            py,
            "invalid native CALL phase or layout",
        ));
    }
    Ok(value)
}

unsafe fn operand(
    py: Python<'_>,
    view: *const RawInterpreterCallView,
    kind: u32,
    index: usize,
) -> PyResult<RawInterpreterCallOperand> {
    let value = unsafe { native::call_operand(py, view, kind, index)? };
    if value.value.is_null() && kind != native::OPERAND_EXPANDED_KWARGS {
        return Err(strict_runtime_unavailable(
            py,
            "selected native CALL operand is absent",
        ));
    }
    Ok(value)
}

fn class_fact<'a>(
    state: &'a InterpreterCall<'_>,
    selected: &InterpreterCallReceipt,
) -> Option<&'a ClassTypeFact> {
    state
        .data()
        .source
        .verified()
        .type_facts()
        .facts()
        .classes
        .iter()
        .find(|fact| &fact.identity == selected.source_definition())
}

fn supported_dataclass(state: &InterpreterCall<'_>, fact: &ClassTypeFact) -> bool {
    supported_dataclass_proposal(
        state
            .data()
            .source
            .verified()
            .type_facts()
            .facts()
            .language_policy
            .adapters
            .dataclasses,
        fact,
    )
}

/// Value-only prefilter. Actual CALL operands, helper graph, source execution,
/// bases and namespace remain independently authenticated by the later joins.
fn supported_dataclass_proposal(policy: StdlibDataclassPolicy, fact: &ClassTypeFact) -> bool {
    policy == StdlibDataclassPolicy::Stdlib
        && fact.participation == ParticipationProposal::Candidate
        && fact
            .uncertainty
            .iter()
            .all(|reason| *reason == soac_contracts::UncertaintyReason::OpenWorld)
        && fact.transform.as_ref().is_some_and(|transform| {
            transform.kind == TransformKind::StdlibDataclass
                && transform.dataclass_options.is_some()
        })
        && matches!(fact.decorators.as_slice(), [decorator]
            if decorator.kind == DecoratorKind::StdlibDataclass && decorator.uncertainty.is_empty())
}

fn factory_fact<'a>(
    state: &'a InterpreterCall<'_>,
    range: SourceRange,
) -> Option<&'a ClassTypeFact> {
    let mut found = state
        .data()
        .source
        .verified()
        .type_facts()
        .facts()
        .classes
        .iter()
        .filter(|fact| {
            supported_dataclass(state, fact) && fact.decorators[0].expression_range == range
        });
    let one = found.next()?;
    found.next().is_none().then_some(one)
}

/// The one synchronous incoming edge is supplied by native after real argument
/// consumption. The outer source receipt must target this exact current helper.
/// No ancestor walk or matching source/name substitutes for the child ordinal.
unsafe fn class_parent<'py>(
    py: Python<'py>,
    current: &InterpreterCall<'py>,
    frame: &RawInterpreterFrameInfo,
    selected: &InterpreterCallReceipt,
    call_info: &RawInterpreterCallInfo,
) -> PyResult<Option<(InterpreterCall<'py>, RawInterpreterFrameInfo, u32)>> {
    let code = unsafe { Borrowed::<PyAny>::from_ptr_or_err(py, frame.code)? };
    let current_code = current.data().source.code(py, &code)?;
    if current_code.role() != InterpreterCodeRole::TypeParameterScope {
        return Ok(Some((
            unsafe { call::captured_call(py, frame)? },
            *frame,
            native::DECORATORS_CURRENT,
        )));
    }
    let Some(site) = (unsafe { call_info.direct_caller.as_ref() }) else {
        return Ok(None);
    };
    let (parent, parent_frame) = unsafe { actual_call(py, site)? };
    if !Arc::ptr_eq(&parent.data().source, &current.data().source)
        || !parent
            .data()
            .execution
            .same_execution(&current.data().execution)
    {
        return Err(strict_runtime_unavailable(
            py,
            "generic class caller has a foreign source execution",
        ));
    }
    let Some(incoming) = receipt(py, &parent, &parent_frame, site)? else {
        return Ok(None);
    };
    if incoming.generic_scope_ordinal() != Some(current_code.ordinal())
        || incoming.source_definition() != selected.source_definition()
    {
        return Err(strict_runtime_unavailable(
            py,
            "generic class did not consume its exact incoming CALL",
        ));
    }
    Ok(Some((
        parent,
        parent_frame,
        native::DECORATORS_DIRECT_CALLER,
    )))
}

unsafe fn validate_names(
    py: Python<'_>,
    view: *const RawInterpreterCallView,
    selected: &InterpreterCallReceipt,
    site: &RawInterpreterCallSite,
) -> PyResult<*mut ffi::PyObject> {
    if site.form != native::CALL_VECTOR_KW {
        return Ok(ptr::null_mut());
    }
    let names = unsafe { operand(py, view, native::OPERAND_KEYWORD_NAMES, 0)? }.value;
    if unsafe { ffi::PyTuple_CheckExact(names) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "native keyword names are not an exact tuple",
        ));
    }
    let expected = selected.input.keyword_names.as_ref().ok_or_else(|| {
        strict_runtime_unavailable(py, "keyword CALL lost its native names receipt")
    })?;
    if unsafe { ffi::PyTuple_Size(names) } != expected.len() as ffi::Py_ssize_t {
        return Err(strict_runtime_unavailable(
            py,
            "native keyword count changed",
        ));
    }
    for (index, expected) in expected.iter().enumerate() {
        let name = unsafe { ffi::PyTuple_GetItem(names, index as ffi::Py_ssize_t) };
        let name = unsafe { Borrowed::<PyAny>::from_ptr_or_err(py, name)? };
        if !name.is_exact_instance_of::<PyString>()
            || name.cast::<PyString>()?.to_str()? != expected
        {
            return Err(strict_runtime_unavailable(
                py,
                "native keyword name differs from the original CALL",
            ));
        }
    }
    Ok(names)
}

pub(super) unsafe extern "C" fn select(
    raw_state: *mut ffi::PyObject,
    raw_info: *const RawInterpreterCallInfo,
    view: *const RawInterpreterCallView,
    output: *mut RawInterpreterCallDecision,
    output_size: usize,
) -> c_int {
    callback(|py| {
        if output.is_null() || output_size != size_of::<RawInterpreterCallDecision>() {
            return Err(strict_runtime_unavailable(
                py,
                "native CALL decision has an incompatible size",
            ));
        }
        let info = unsafe { info(py, raw_info, native::CALL_SELECT)? };
        let (state, frame) = unsafe { actual_call(py, &info.current)? };
        if raw_state != state.owner().as_ptr() {
            return Err(strict_runtime_unavailable(
                py,
                "native CALL state is not its actual caller",
            ));
        }
        let mut decision = RawInterpreterCallDecision::ordinary();
        let Some(selected) = receipt(py, &state, &frame, &info.current)? else {
            unsafe {
                *output = decision;
            }
            return Ok(());
        };
        match selected.origin.role {
            InterpreterCallRole::ClassConstruction { .. } => {
                decision.kind = native::CALL_CLASS;
                if let Some((_, _, source)) =
                    unsafe { class_parent(py, &state, &frame, selected, info)? }
                {
                    decision.decorator_source = source;
                    decision.decorator_count = class_fact(&state, selected)
                        .map_or(0, |fact| fact.decorators.len())
                        as ffi::Py_ssize_t;
                } else {
                    // No incoming edge means no decorator window or participation
                    // proof. PREPARE_TYPE explicitly returns Declined.
                    decision.decorator_source = native::DECORATORS_CURRENT;
                }
            }
            InterpreterCallRole::GenericScopeInvocation { scope_ordinal } => {
                let callable = unsafe { operand(py, view, native::OPERAND_CALLABLE, 0)? }.value;
                let callable = unsafe { Borrowed::<PyAny>::from_ptr(py, callable) };
                if let Some(auth) =
                    strict_function::authenticate_borrowed_strict_function(py, callable)?
                {
                    if auth.is_interpreter()
                        && auth.interpreter_source_authority()?
                        && Arc::ptr_eq(auth.native_source()?, &state.data().source)
                        && auth.native_code_ordinal()? == scope_ordinal
                        && Arc::ptr_eq(auth.native_birth_execution()?, &state.data().invocation)
                        && auth.globals()?.as_ptr() == frame.globals
                    {
                        decision.kind = native::CALL_GENERIC_SCOPE;
                    }
                }
            }
            InterpreterCallRole::SourceExpression => {
                if let Some(fact) = factory_fact(&state, selected.origin.original_range) {
                    // Expanded factory option syntax remains an ordinary
                    // pre-participation fallback until a no-container parser is
                    // selected. CALL/KW consume their actual native values.
                    if info.current.form != native::CALL_EXPANDED {
                        let callable =
                            unsafe { operand(py, view, native::OPERAND_CALLABLE, 0)? }.value;
                        let callable = unsafe { Borrowed::<PyAny>::from_ptr(py, callable) };
                        let mut args = Vec::new();
                        for index in 0..count(py, info.current.positional_count)? {
                            args.push(
                                unsafe { operand(py, view, native::OPERAND_POSITIONAL, index)? }
                                    .value,
                            );
                        }
                        for index in 0..count(py, info.current.keyword_count)? {
                            args.push(
                                unsafe { operand(py, view, native::OPERAND_KEYWORD_VALUE, index)? }
                                    .value,
                            );
                        }
                        let names = unsafe { validate_names(py, view, selected, &info.current)? };
                        let globals =
                            unsafe { Borrowed::<PyAny>::from_ptr_or_err(py, frame.globals)? }
                                .cast::<PyDict>()?;
                        if let Some(owner) = unsafe {
                            strict_dataclass::prepare_native(
                                py,
                                fact,
                                state.data().source.verified().clone(),
                                state.data().execution.clone(),
                                state.data().invocation.clone(),
                                globals,
                                true,
                                callable,
                                args.as_ptr(),
                                count(py, info.current.positional_count)?,
                                names,
                            )?
                        } {
                            decision.kind = native::CALL_DATACLASS_ROOT;
                            decision.dataclass_stage = ROOT_FACTORY;
                            decision.metadata =
                                strict_dataclass::native_invocation_for(&owner)?.into_ptr();
                        }
                    }
                }
            }
            InterpreterCallRole::Decorator {
                index,
                expression_range,
            } => {
                if info.current.positional_count != 1 || info.current.keyword_count != 0 {
                    return Err(strict_runtime_unavailable(
                        py,
                        "original decorator CALL has another operand shape",
                    ));
                }
                let callable = unsafe { operand(py, view, native::OPERAND_CALLABLE, 0)? };
                let actual = unsafe { operand(py, view, native::OPERAND_POSITIONAL, 0)? }.value;
                let factory = unsafe { Borrowed::<PyAny>::from_ptr(py, callable.value) };
                let actual = unsafe { Borrowed::<PyAny>::from_ptr(py, actual) };
                if let Some(fact) = class_fact(&state, selected) {
                    if index == 0
                        && supported_dataclass(&state, fact)
                        && fact.decorators[0].expression_range == expression_range
                    {
                        if let Some(class) =
                            crate::strict_class_state::for_constructed_type(py, &actual)?
                        {
                            if let Some(namespace) = class.dataclass_namespace()? {
                                if let Some(owner) = namespace.owner_for_native_apply(&class)? {
                                    let invocation =
                                        strict_dataclass::native_invocation_for(&owner)?;
                                    let creation_matches = match callable.creation_status {
                                        native::CREATION_NONE => true, // exact bare helper is rechecked by begin_apply
                                        native::CREATION_LIVE => {
                                            callable.creation_role == CREATED_DECORATOR
                                                && callable.dataclass_owner == owner.as_ptr()
                                                && callable.dataclass_invocation
                                                    == invocation.as_ptr()
                                        }
                                        _ => false,
                                    };
                                    if !creation_matches
                                        || !strict_dataclass::native_source_matches(
                                            &owner,
                                            invocation.as_ptr(),
                                            fact,
                                            state.data().source.verified(),
                                            &state.data().execution,
                                            &state.data().invocation,
                                        )?
                                    {
                                        return Err(strict_runtime_unavailable(
                                            py,
                                            "dataclass decorator belongs to another source invocation",
                                        ));
                                    }
                                    if strict_dataclass::begin_apply(
                                        &owner,
                                        &factory,
                                        &actual,
                                        || {
                                            super::completion::dataclass_source_members(
                                                py,
                                                &state,
                                                &frame,
                                                owner.as_borrowed(),
                                                actual,
                                            )
                                        },
                                    )? {
                                        decision.kind = native::CALL_DATACLASS_ROOT;
                                        decision.dataclass_stage = ROOT_APPLY;
                                        decision.metadata = invocation.into_ptr();
                                    }
                                }
                            }
                        }
                    }
                } else if index == 0 {
                    if let Some(namespace) = &state.data().namespace {
                        if let Some(prepared) = crate::strict_descriptor::prepare_native_descriptor(
                            py,
                            &state.data().source,
                            &state.data().execution,
                            &state.data().invocation,
                            namespace,
                            selected.source_definition(),
                            frame.globals,
                            factory,
                            actual,
                        )? {
                            decision.kind = native::CALL_BUILTIN_DESCRIPTOR;
                            decision.metadata = prepared.witness.into_ptr();
                            decision.expected_function_owner = prepared.function_owner;
                            decision.verified_code = prepared.code;
                        }
                    }
                }
            }
        }
        unsafe {
            *output = decision;
        }
        Ok(())
    })
}

pub(super) enum ClassTransform<'py> {
    Declined,
    Ordinary,
    Dataclass(strict_dataclass::DataclassConstruction<'py>),
}

pub(super) struct PreparedClassTransform<'py> {
    pub(super) transform: ClassTransform<'py>,
    pub(super) completion_invocation: Arc<InterpreterInvocationIdentity>,
}

/// ROOT validates the actual namespace/function/type-construction operands.
/// This joins only the finite evaluated decorator window and the exact generic
/// completion context. Declined MUST NOT be reinterpreted as Ordinary.
pub(super) unsafe fn prepare_class_transform<'py>(
    py: Python<'py>,
    current: &InterpreterCall<'py>,
    raw_info: *const RawInterpreterCallInfo,
    view: *const RawInterpreterCallView,
    auth: &AuthenticatedStrictFunction<'_, 'py>,
    namespace_execution: &Arc<NamespaceExecution>,
    namespace: &Bound<'py, PyDict>,
    bases: &Bound<'py, PyTuple>,
    captures: Option<&ClassConstructionCaptures<'py>>,
) -> PyResult<PreparedClassTransform<'py>> {
    let info = unsafe { info(py, raw_info, native::CALL_PREPARE_TYPE)? };
    let (checked, frame) = unsafe { actual_call(py, &info.current)? };
    if checked.owner().as_ptr() != current.owner().as_ptr() {
        return Err(strict_runtime_unavailable(
            py,
            "class decorator view has another caller",
        ));
    }
    let selected = receipt(py, current, &frame, &info.current)?
        .filter(|receipt| receipt.class_body_ordinal().is_some())
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "class preparation is not an original CLASS CALL")
        })?;
    if Some(auth.native_code_ordinal()?) != selected.class_body_ordinal()
        || auth
            .origin()
            .is_none_or(|origin| &origin.definition != selected.source_definition())
    {
        return Err(strict_runtime_unavailable(
            py,
            "class decorator view has another namespace function",
        ));
    }
    let Some((parent, parent_frame, decorator_source)) =
        (unsafe { class_parent(py, current, &frame, selected, info)? })
    else {
        return Ok(PreparedClassTransform {
            transform: ClassTransform::Declined,
            completion_invocation: current.data().invocation.clone(),
        });
    };
    let mut result = PreparedClassTransform {
        transform: ClassTransform::Declined,
        completion_invocation: parent.data().invocation.clone(),
    };
    let Some(fact) = class_fact(current, selected) else {
        return Ok(result);
    };
    if info.decorator_source != decorator_source
        || count(py, info.decorator_count)? != fact.decorators.len()
    {
        return Err(strict_runtime_unavailable(
            py,
            "class decorator window does not match its original declaration",
        ));
    }
    if fact.decorators.is_empty() {
        result.transform = ClassTransform::Ordinary;
        return Ok(result);
    }
    if !supported_dataclass(current, fact) {
        return Ok(result);
    }
    let actual = unsafe { operand(py, view, native::OPERAND_DECORATOR, 0)? };
    let decorator = unsafe { Borrowed::<PyAny>::from_ptr(py, actual.value) };
    let owner = match actual.creation_status {
        native::CREATION_LIVE if actual.creation_role == CREATED_DECORATOR => {
            let owner = unsafe { Borrowed::<PyAny>::from_ptr(py, actual.dataclass_owner) };
            if !strict_dataclass::native_source_matches(
                &owner,
                actual.dataclass_invocation,
                fact,
                parent.data().source.verified(),
                &parent.data().execution,
                &parent.data().invocation,
            )? {
                return Ok(result);
            }
            owner.to_owned()
        }
        native::CREATION_NONE => {
            let globals = unsafe { Borrowed::<PyAny>::from_ptr_or_err(py, parent_frame.globals)? }
                .cast::<PyDict>()?;
            let Some(owner) = (unsafe {
                strict_dataclass::prepare_native(
                    py,
                    fact,
                    parent.data().source.verified().clone(),
                    parent.data().execution.clone(),
                    parent.data().invocation.clone(),
                    globals,
                    false,
                    decorator,
                    ptr::null(),
                    0,
                    ptr::null_mut(),
                )
            })?
            else {
                return Ok(result);
            };
            owner
        }
        _ => return Ok(result),
    };
    if let Some(plan) = strict_dataclass::prepare_construction(
        py,
        &owner,
        auth,
        namespace,
        bases,
        namespace_execution,
        captures,
    )? {
        result.transform = ClassTransform::Dataclass(plan);
    }
    Ok(result)
}

pub(super) unsafe extern "C" fn selected_call_finished(
    raw_state: *mut ffi::PyObject,
    caller: *const RawInterpreterFrameView,
    kind: u32,
    metadata: *mut ffi::PyObject,
    dataclass_owner: *mut ffi::PyObject,
    stage: u32,
    result: *mut ffi::PyObject,
) -> c_int {
    callback(|py| {
        let frame = unsafe { native::frame_info(py, caller)? };
        let state = unsafe { call::captured_call(py, &frame)? };
        if raw_state != state.owner().as_ptr()
            || frame.phase != native::RUNNING
            || state.data().phase.get() != CallPhase::Running
            || metadata.is_null()
        {
            return Err(strict_runtime_unavailable(
                py,
                "selected completion has no actual waiting caller",
            ));
        }
        let code = unsafe { Borrowed::<PyAny>::from_ptr_or_err(py, frame.code)? };
        let selected = state
            .data()
            .source
            .call(py, &code, ordinal(py, &frame)?)?
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "selected completion lost its exact CALL receipt")
            })?;
        let metadata = unsafe { Borrowed::<PyAny>::from_ptr(py, metadata) };
        let result = if result.is_null() {
            None
        } else {
            Some(unsafe { Borrowed::<PyAny>::from_ptr(py, result) })
        };
        match kind {
            native::CALL_BUILTIN_DESCRIPTOR if dataclass_owner.is_null() && stage == 0 => {
                if !matches!(
                    selected.origin.role,
                    InterpreterCallRole::Decorator { index: 0, .. }
                ) {
                    return Err(strict_runtime_unavailable(
                        py,
                        "descriptor completion has another source role",
                    ));
                }
                let namespace = state.data().namespace.as_ref().ok_or_else(|| {
                    strict_runtime_unavailable(py, "descriptor completion has no source namespace")
                })?;
                crate::strict_descriptor::finish_native_descriptor(
                    py,
                    &state.data().source,
                    namespace,
                    selected.source_definition(),
                    metadata,
                    result,
                )
            }
            native::CALL_DATACLASS_ROOT
                if !dataclass_owner.is_null() && matches!(stage, ROOT_FACTORY | ROOT_APPLY) =>
            {
                let owner = unsafe { Borrowed::<PyAny>::from_ptr(py, dataclass_owner) };
                let fact = match (stage, selected.origin.role) {
                    (ROOT_FACTORY, InterpreterCallRole::SourceExpression) => {
                        factory_fact(&state, selected.origin.original_range)
                    }
                    (ROOT_APPLY, InterpreterCallRole::Decorator { index: 0, .. }) => {
                        class_fact(&state, selected)
                            .filter(|fact| supported_dataclass(&state, fact))
                    }
                    _ => None,
                }
                .ok_or_else(|| {
                    strict_runtime_unavailable(
                        py,
                        "dataclass completion has another exact source role",
                    )
                })?;
                if !strict_dataclass::native_completion_matches(
                    &owner,
                    metadata.as_ptr(),
                    fact,
                    state.data().source.verified(),
                    &state.data().execution,
                    &state.data().invocation,
                )? {
                    return Err(strict_runtime_unavailable(
                        py,
                        "dataclass completion has a foreign invocation owner",
                    ));
                }
                let Some(result) = result else {
                    let failed = strict_dataclass::fail_native_call(&owner);
                    let forgotten = if stage == ROOT_APPLY {
                        super::completion::forget_failed_dataclass(
                            py,
                            &state,
                            &frame,
                            owner,
                            &fact.identity,
                        )
                    } else {
                        Ok(())
                    };
                    // Native detached the real body exception before invoking
                    // us and restores it after this cleanup-only callback.
                    return failed.and(forgotten);
                };
                let completed = (|| {
                    if stage == ROOT_FACTORY {
                        strict_dataclass::finish_factory(&owner, &result)
                    } else {
                        strict_dataclass::finish_apply(&owner, &result)?;
                        super::completion::dataclass_application_finished(
                            py, &state, &frame, owner, result,
                        )
                    }
                })();
                if completed.is_err() {
                    // The same authenticated metadata owner is still supported
                    // by native. Mark Rust/native failed before result cleanup;
                    // preserve the exact completion error rather than replacing
                    // it with a secondary retirement failure.
                    let _ = strict_dataclass::fail_native_call(&owner);
                    if stage == ROOT_APPLY {
                        let _ = super::completion::forget_failed_dataclass(
                            py,
                            &state,
                            &frame,
                            owner,
                            &fact.identity,
                        );
                    }
                }
                completed
            }
            _ => Err(strict_runtime_unavailable(
                py,
                "selected completion kind/owner/stage mismatch",
            )),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strict_interpreter_source::{
        InterpreterCallInputLayout, InterpreterCallOrigin, InterpreterKeywordKind,
        InterpreterPositionalKind,
    };
    use soac_contracts::{
        ClassDictionarySemantics, ClassOpenness, ClassTransformFact, DataclassOptions,
        DecoratorFact, DefinitionKind, DynamicClassReason, InheritanceFact, MetaclassFact,
        ModuleContentId, SourceIdentity, UncertaintyReason,
    };

    /// The classifier fields match the real pytest dataclass Base rows:
    /// Candidate, one uncertainty-free StdlibDataclass decorator, known options,
    /// and class OpenWorld. Other fields are minimal and confer no native owner,
    /// helper graph, base, field, or construction authority.
    fn observed_dataclass_proposal(slots: bool) -> ClassTypeFact {
        ClassTypeFact {
            identity: SourceIdentity {
                module: ModuleContentId::new("dataclass_proposal_fixture", 0),
                lexical_qualname: "Base".into(),
                source_range: SourceRange::new(0, 40),
                definition_kind: DefinitionKind::Class,
            },
            bases: Vec::new(),
            metaclass: MetaclassFact::BuiltinType,
            decorators: vec![DecoratorFact {
                kind: DecoratorKind::StdlibDataclass,
                expression_range: SourceRange::new(1, 23),
                definition: None,
                source_digest: None,
                arguments: Default::default(),
                uncertainty: Default::default(),
            }],
            participation: ParticipationProposal::Candidate,
            dictionary: ClassDictionarySemantics::DictionaryBearing,
            instance_fields: Vec::new(),
            methods: Vec::new(),
            class_members: Vec::new(),
            inheritance: InheritanceFact {
                linearized_bases: Vec::new(),
                complete: true,
            },
            openness: ClassOpenness::OpenSubclassFamily,
            transform: Some(ClassTransformFact {
                kind: TransformKind::StdlibDataclass,
                provenance: None,
                dataclass_options: Some(DataclassOptions {
                    slots,
                    ..DataclassOptions::default()
                }),
                generated_methods: Default::default(),
            }),
            uncertainty: [UncertaintyReason::OpenWorld].into(),
        }
    }

    #[test]
    fn interpreter_dataclass_proposal_keeps_observed_open_world_candidates() {
        for slots in [false, true] {
            let mut fact = observed_dataclass_proposal(slots);
            assert!(supported_dataclass_proposal(
                StdlibDataclassPolicy::Stdlib,
                &fact
            ));
            fact.uncertainty.clear();
            assert!(supported_dataclass_proposal(
                StdlibDataclassPolicy::Stdlib,
                &fact
            ));
        }
    }

    #[test]
    fn interpreter_dataclass_proposal_rejects_all_other_class_uncertainties() {
        for reason in [
            UncertaintyReason::Any,
            UncertaintyReason::Unknown,
            UncertaintyReason::CheckerTodo,
            UncertaintyReason::IgnoredDiagnostic,
            UncertaintyReason::UnresolvedImport,
            UncertaintyReason::DynamicDecorator,
            UncertaintyReason::DynamicMetaclass,
            UncertaintyReason::DynamicDescriptor,
            UncertaintyReason::UnsafeNarrowing,
            UncertaintyReason::UnsupportedType,
            UncertaintyReason::PartialInitialization,
        ] {
            let mut fact = observed_dataclass_proposal(false);
            fact.uncertainty = [reason].into();
            assert!(
                !supported_dataclass_proposal(StdlibDataclassPolicy::Stdlib, &fact),
                "unexpected class uncertainty admitted: {reason:?}"
            );
            fact.uncertainty.insert(UncertaintyReason::OpenWorld);
            assert!(
                !supported_dataclass_proposal(StdlibDataclassPolicy::Stdlib, &fact),
                "OpenWorld hid another class uncertainty: {reason:?}"
            );
        }
    }

    #[test]
    fn interpreter_dataclass_proposal_keeps_policy_transform_and_decorator_refusals() {
        let original = observed_dataclass_proposal(false);
        assert!(!supported_dataclass_proposal(
            StdlibDataclassPolicy::Dynamic,
            &original
        ));
        let mut fact = original.clone();
        fact.participation =
            ParticipationProposal::Dynamic([DynamicClassReason::UnknownDecorator].into());
        assert!(!supported_dataclass_proposal(
            StdlibDataclassPolicy::Stdlib,
            &fact
        ));
        for kind in [
            TransformKind::DataclassTransform,
            TransformKind::UnsupportedFramework,
        ] {
            let mut fact = original.clone();
            fact.transform.as_mut().unwrap().kind = kind;
            assert!(!supported_dataclass_proposal(
                StdlibDataclassPolicy::Stdlib,
                &fact
            ));
        }
        let mut fact = original.clone();
        fact.transform = None;
        assert!(!supported_dataclass_proposal(
            StdlibDataclassPolicy::Stdlib,
            &fact
        ));
        let mut fact = original.clone();
        fact.transform.as_mut().unwrap().dataclass_options = None;
        assert!(!supported_dataclass_proposal(
            StdlibDataclassPolicy::Stdlib,
            &fact
        ));
        let mut fact = original.clone();
        fact.decorators.clear();
        assert!(!supported_dataclass_proposal(
            StdlibDataclassPolicy::Stdlib,
            &fact
        ));
        let mut fact = original.clone();
        fact.decorators.push(fact.decorators[0].clone());
        assert!(!supported_dataclass_proposal(
            StdlibDataclassPolicy::Stdlib,
            &fact
        ));
        for kind in [
            DecoratorKind::TypingFinal,
            DecoratorKind::StaticMethod,
            DecoratorKind::ClassMethod,
            DecoratorKind::Property,
            DecoratorKind::StdlibCachedProperty,
            DecoratorKind::DataclassTransform,
            DecoratorKind::Other,
            DecoratorKind::Unknown,
        ] {
            let mut fact = original.clone();
            fact.decorators[0].kind = kind;
            assert!(!supported_dataclass_proposal(
                StdlibDataclassPolicy::Stdlib,
                &fact
            ));
        }
        // Even OpenWorld is not an allowed uncertainty on the decorator itself.
        for reason in [
            UncertaintyReason::Any,
            UncertaintyReason::Unknown,
            UncertaintyReason::CheckerTodo,
            UncertaintyReason::IgnoredDiagnostic,
            UncertaintyReason::UnresolvedImport,
            UncertaintyReason::DynamicDecorator,
            UncertaintyReason::DynamicMetaclass,
            UncertaintyReason::DynamicDescriptor,
            UncertaintyReason::UnsafeNarrowing,
            UncertaintyReason::UnsupportedType,
            UncertaintyReason::OpenWorld,
            UncertaintyReason::PartialInitialization,
        ] {
            let mut fact = original.clone();
            fact.decorators[0].uncertainty.insert(reason);
            assert!(
                !supported_dataclass_proposal(StdlibDataclassPolicy::Stdlib, &fact),
                "uncertain decorator admitted: {reason:?}"
            );
        }
    }

    // Value-only fixtures for the mandatory join. These cannot authenticate a
    // source, native frame, callable, descriptor or dataclass on their own.
    fn fixture(
        form: InterpreterCallForm,
        channel: InterpreterCallChannel,
        argument: Option<u32>,
    ) -> (InterpreterCallReceipt, RawInterpreterCallSite) {
        let range = SourceRange::new(0, 10);
        (
            InterpreterCallReceipt {
                origin: InterpreterCallOrigin {
                    source_definition: SourceIdentity::module_body(
                        ModuleContentId::new("native_call_shape", 0),
                        10,
                    ),
                    original_range: range,
                    role: InterpreterCallRole::SourceExpression,
                },
                instruction_ordinal: 0,
                native_byte_offset: None,
                form,
                native_value_argument_count: argument,
                input: InterpreterCallInputLayout {
                    channel,
                    preloaded_value_count: 0,
                    positional_kind: if form == InterpreterCallForm::Expanded {
                        InterpreterPositionalKind::SoleStarDeferred
                    } else {
                        InterpreterPositionalKind::Vector
                    },
                    positional_entries: Vec::new(),
                    keyword_kind: InterpreterKeywordKind::None,
                    keyword_names: None,
                    keyword_entries: Vec::new(),
                    keyword_groups: Vec::new(),
                },
                gaps: Vec::new(),
            },
            RawInterpreterCallSite {
                form: match form {
                    InterpreterCallForm::Positional => native::CALL_VECTOR,
                    InterpreterCallForm::Keywords => native::CALL_VECTOR_KW,
                    InterpreterCallForm::Expanded => native::CALL_EXPANDED,
                },
                channel: native::CALL_NULL_CHANNEL,
                instruction_argument: argument.unwrap_or(0),
                reserved: 0,
                positional_count: argument.unwrap_or(0) as ffi::Py_ssize_t,
                keyword_count: 0,
                // The pure shape check never reads a frame. actual_call is
                // separately mandatory before its production invocation.
                frame: ptr::null(),
            },
        )
    }

    #[test]
    fn interpreter_call_join_preserves_actual_method_or_leading_channel_counts() {
        for source_channel in [
            InterpreterCallChannel::Null,
            InterpreterCallChannel::LeadingArgument,
            InterpreterCallChannel::MethodSelfOrNull,
        ] {
            let (receipt, mut site) =
                fixture(InterpreterCallForm::Positional, source_channel, Some(2));
            assert_eq!(
                shape_matches(&receipt, &site),
                source_channel != InterpreterCallChannel::LeadingArgument
            );
            site.channel = native::CALL_VALUE_CHANNEL;
            site.positional_count = 3;
            assert_eq!(
                shape_matches(&receipt, &site),
                source_channel != InterpreterCallChannel::Null
            );
            site.positional_count = 2;
            assert!(!shape_matches(&receipt, &site));
            site.positional_count = 3;
            site.reserved = 1;
            assert!(!shape_matches(&receipt, &site));
        }
    }

    #[test]
    fn interpreter_call_join_keywords_do_not_change_opcode_value_count() {
        let (mut receipt, mut site) = fixture(
            InterpreterCallForm::Keywords,
            InterpreterCallChannel::Null,
            Some(3),
        );
        receipt.input.keyword_kind = InterpreterKeywordKind::NamesTuple;
        receipt.input.keyword_names = Some(vec!["second".into(), "third".into()]);
        site.positional_count = 1;
        site.keyword_count = 2;
        assert!(shape_matches(&receipt, &site));
        site.keyword_count = 1;
        assert!(!shape_matches(&receipt, &site));
        site.positional_count = 2;
        assert!(!shape_matches(&receipt, &site));
        site.keyword_count = 2;
        assert!(!shape_matches(&receipt, &site));
        site.positional_count = 1;
        site.form = native::CALL_VECTOR;
        assert!(!shape_matches(&receipt, &site));
    }

    #[test]
    fn interpreter_call_join_expansion_zero_operand_is_not_zero_runtime_values() {
        let (mut receipt, mut site) = fixture(
            InterpreterCallForm::Expanded,
            InterpreterCallChannel::Null,
            None,
        );
        for (positional, keywords) in [(0, 0), (1, 3), (20, 7)] {
            site.positional_count = positional;
            site.keyword_count = keywords;
            assert!(shape_matches(&receipt, &site));
        }
        site.instruction_argument = 1;
        assert!(!shape_matches(&receipt, &site));
        site.instruction_argument = 0;
        receipt.native_value_argument_count = Some(0);
        assert!(!shape_matches(&receipt, &site));
        receipt.native_value_argument_count = None;
        site.channel = native::CALL_VALUE_CHANNEL;
        assert!(!shape_matches(&receipt, &site));
        site.channel = native::CALL_NULL_CHANNEL;
        site.positional_count = -1;
        assert!(!shape_matches(&receipt, &site));
    }
}
