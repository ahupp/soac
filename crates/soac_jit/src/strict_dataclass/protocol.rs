//! Actual invocation, frame, call-site and operand proofs for native callbacks.
//!
//! Shared stdlib helpers receive only an invocation-scoped frame role. Fresh
//! functions require the separate native creation record and one-way Created
//! publication. Neither code equality nor a public owner pointer can replay an
//! invocation. Every successful callback is free of Python calls; allocation
//! is confined to unpublished Created and explicit replacement preparation.

use std::ffi::{c_int, c_uint};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::strict_runtime_unavailable;

use super::adoption::{ClassPlan, MemberKind};
use super::catalog::{Helper, StructType, dictionary_value, text_is};
use super::edges::{CodeRole, Edge, Template};
use super::generation::FieldRole;
use super::invocation::{self, Owner, Phase, native_invocation, option_values};
use super::native::{self, Frame, RawFrameView};

unsafe extern "C" {
    static mut PyCell_Type: ffi::PyTypeObject;
    fn PyCell_Get(cell: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn _PyFunction_Vectorcall(
        function: *mut ffi::PyObject,
        args: *const *mut ffi::PyObject,
        nargs: usize,
        names: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
}

/// These values belong to this callback table, not to Python code names or
/// the native generated-function role namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum Role {
    Dataclass = 1,
    Decorator = 2,
    Process = 3,
    Install = 4,
    SetReplace = 5,
    GeneratedExec = 6,
    GeneratedFactory = 7,
    SetMatchArgs = 8,
    Init = 9,
    FieldInit = 10,
    Frozen = 11,
    HashAdd = 12,
    HashNone = 13,
    RecursiveRepr = 14,
    ReprDecorator = 15,
    Annotate = 16,
    SetGenerated = 17,
    Slots = 18,
    AddInit = 32,
    AddRepr = 33,
    AddEquality = 34,
    AddLess = 35,
    AddLessEqual = 36,
    AddGreater = 37,
    AddGreaterEqual = 38,
    AddHash = 39,
    AddFrozenSetattr = 40,
    AddFrozenDelattr = 41,
}

impl Role {
    fn from_native(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Dataclass),
            2 => Some(Self::Decorator),
            3 => Some(Self::Process),
            4 => Some(Self::Install),
            5 => Some(Self::SetReplace),
            6 => Some(Self::GeneratedExec),
            7 => Some(Self::GeneratedFactory),
            8 => Some(Self::SetMatchArgs),
            9 => Some(Self::Init),
            10 => Some(Self::FieldInit),
            11 => Some(Self::Frozen),
            12 => Some(Self::HashAdd),
            13 => Some(Self::HashNone),
            14 => Some(Self::RecursiveRepr),
            15 => Some(Self::ReprDecorator),
            16 => Some(Self::Annotate),
            17 => Some(Self::SetGenerated),
            18 => Some(Self::Slots),
            32 => Some(Self::AddInit),
            33 => Some(Self::AddRepr),
            34 => Some(Self::AddEquality),
            35 => Some(Self::AddLess),
            36 => Some(Self::AddLessEqual),
            37 => Some(Self::AddGreater),
            38 => Some(Self::AddGreaterEqual),
            39 => Some(Self::AddHash),
            40 => Some(Self::AddFrozenSetattr),
            41 => Some(Self::AddFrozenDelattr),
            _ => None,
        }
    }

    pub(super) fn code(self) -> Option<CodeRole> {
        Some(match self {
            Self::Dataclass => CodeRole::Helper(Helper::Dataclass),
            Self::Decorator => CodeRole::Template(Template::DataclassWrapper),
            Self::Process => CodeRole::Helper(Helper::ProcessClass),
            Self::Install => CodeRole::Helper(Helper::BuilderInstall),
            Self::SetReplace | Self::SetMatchArgs | Self::SetGenerated => {
                CodeRole::Helper(Helper::SetNewAttribute)
            }
            Self::Init => CodeRole::Helper(Helper::Init),
            Self::FieldInit => CodeRole::Helper(Helper::FieldInit),
            Self::Frozen => CodeRole::Helper(Helper::Frozen),
            Self::HashAdd => CodeRole::Helper(Helper::HashAdd),
            Self::HashNone => CodeRole::Helper(Helper::HashNone),
            Self::RecursiveRepr => CodeRole::Helper(Helper::RecursiveRepr),
            Self::ReprDecorator => CodeRole::Template(Template::ReprDecorator),
            Self::Annotate => CodeRole::Helper(Helper::MakeAnnotate),
            Self::Slots => CodeRole::Helper(Helper::AddSlots),
            Self::AddInit
            | Self::AddRepr
            | Self::AddEquality
            | Self::AddLess
            | Self::AddLessEqual
            | Self::AddGreater
            | Self::AddGreaterEqual
            | Self::AddHash
            | Self::AddFrozenSetattr
            | Self::AddFrozenDelattr => CodeRole::Helper(Helper::BuilderAdd),
            Self::GeneratedExec | Self::GeneratedFactory => return None,
        })
    }

    pub(super) fn add(role: super::generation::GeneratedRole) -> Self {
        use super::generation::GeneratedRole as Generated;
        match role {
            Generated::Init => Self::AddInit,
            Generated::Repr => Self::AddRepr,
            Generated::Equality => Self::AddEquality,
            Generated::Less => Self::AddLess,
            Generated::LessEqual => Self::AddLessEqual,
            Generated::Greater => Self::AddGreater,
            Generated::GreaterEqual => Self::AddGreaterEqual,
            Generated::Hash => Self::AddHash,
            Generated::FrozenSetattr => Self::AddFrozenSetattr,
            Generated::FrozenDelattr => Self::AddFrozenDelattr,
        }
    }
}

pub(super) static CALLBACKS: native::Callbacks = native::Callbacks {
    abi_version: native::CALLBACKS_ABI,
    enter,
    create,
    validate_member,
    bridge,
    compiled,
    created,
    validate_component,
    prepare_slots,
};

fn mark_failed(owner: &Owner<'_>) {
    if !matches!(owner.data().phase.get(), Phase::Complete | Phase::Declined) {
        owner.data().phase.set(Phase::Failed);
        if let Some(plan) = owner.data().plan.get() {
            plan.fail();
        }
        if let Some(replacement) = owner.data().replacement.get() {
            replacement.plan.fail();
        }
    }
    // Native marks its invocation failed before releasing its frame/class
    // edges. Do not decref this owner's active graph while callback views are
    // borrowed; the enclosing root/discard operation clears it on return.
}

fn callback(
    raw_owner: *mut ffi::PyObject,
    operation: impl FnOnce(&Owner<'_>) -> PyResult<c_int>,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        if raw_owner.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "dataclass callback has no owner",
            ));
        }
        let owner = Owner::from_owner(unsafe { Bound::from_borrowed_ptr(py, raw_owner) })?;
        let result = operation(&owner);
        if result.is_err() {
            mark_failed(&owner);
        }
        result
    }));
    match result {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            error.restore(py);
            -1
        }
        Err(_) => {
            strict_runtime_unavailable(py, "panic in dataclass invocation callback").restore(py);
            -1
        }
    }
}

pub(super) fn require(owner: &Owner<'_>, condition: bool, reason: &'static str) -> PyResult<()> {
    if condition {
        Ok(())
    } else {
        Err(strict_runtime_unavailable(owner.owner().py(), reason))
    }
}

pub(super) fn plan<'a>(owner: &'a Owner<'_>) -> PyResult<&'a ClassPlan> {
    owner.data().plan.get().map(AsRef::as_ref).ok_or_else(|| {
        strict_runtime_unavailable(owner.owner().py(), "dataclass class plan is not prepared")
    })
}

fn frame<'a>(owner: &Owner<'_>, raw: *const RawFrameView) -> PyResult<Frame<'a>> {
    let view = unsafe { Frame::from_raw(raw) }.ok_or_else(|| {
        strict_runtime_unavailable(
            owner.owner().py(),
            "dataclass callback has no explicit frame",
        )
    })?;
    require(
        owner,
        view.invocation() == native_invocation(owner)?.as_ptr(),
        "dataclass callback belongs to a different actual invocation",
    )?;
    Ok(view)
}

pub(super) fn plain_entry(function: *mut ffi::PyObject) -> bool {
    !function.is_null()
        && unsafe { ffi::PyFunction_Check(function) } != 0
        && unsafe { (*function.cast::<ffi::PyFunctionObject>()).vectorcall }.is_some_and(|entry| {
            ptr::fn_addr_eq(entry, _PyFunction_Vectorcall as ffi::vectorcallfunc)
        })
}

pub(super) fn no_defaults(function: *mut ffi::PyObject) -> bool {
    let raw = function.cast::<ffi::PyFunctionObject>();
    let defaults = unsafe { (*raw).func_defaults };
    let keywords = unsafe { (*raw).func_kwdefaults };
    (defaults.is_null()
        || (unsafe { ffi::PyTuple_CheckExact(defaults) } != 0
            && unsafe { ffi::PyTuple_Size(defaults) } == 0))
        && (keywords.is_null()
            || (unsafe { ffi::PyDict_CheckExact(keywords) } != 0
                && unsafe { ffi::PyDict_Size(keywords) } == 0))
}

pub(super) fn no_closure(function: *mut ffi::PyObject) -> bool {
    let closure = unsafe { (*function.cast::<ffi::PyFunctionObject>()).func_closure };
    closure.is_null()
        || (unsafe { ffi::PyTuple_CheckExact(closure) } != 0
            && unsafe { ffi::PyTuple_Size(closure) } == 0)
}

fn weak_function<'py>(owner: &Owner<'py>, index: usize) -> PyResult<Option<Bound<'py, PyAny>>> {
    let py = owner.owner().py();
    let weak = owner.reference(index)?;
    if weak.is_none() {
        return Ok(None);
    }
    require(
        owner,
        unsafe { ffi::PyWeakref_CheckRefExact(weak.as_ptr()) } != 0,
        "dataclass actual-function witness is not a native weak reference",
    )?;
    let mut function = ptr::null_mut();
    match unsafe { ffi::PyWeakref_GetRef(weak.as_ptr(), &mut function) } {
        0 => Ok(None),
        1 => Ok(Some(unsafe { Bound::from_owned_ptr(py, function) })),
        _ => Err(PyErr::fetch(py)),
    }
}

fn recorded_function(
    owner: &Owner<'_>,
    function: *mut ffi::PyObject,
    role: u32,
    weak: usize,
) -> PyResult<bool> {
    if function.is_null()
        || unsafe { ffi::PyFunction_Check(function) } == 0
        || !weak_function(owner, weak)?.is_some_and(|actual| actual.as_ptr() == function)
    {
        return Ok(false);
    }
    native::predicate(owner.owner().py(), unsafe {
        native::PyFunction_MatchesSoacDataclassCreation(
            function,
            native_invocation(owner)?.as_ptr(),
            role,
        )
    })
}

pub(super) fn closure_value(
    function: *mut ffi::PyObject,
    index: usize,
) -> Option<*mut ffi::PyObject> {
    let closure = unsafe { (*function.cast::<ffi::PyFunctionObject>()).func_closure };
    if closure.is_null()
        || unsafe { ffi::PyTuple_CheckExact(closure) } == 0
        || index >= unsafe { ffi::PyTuple_Size(closure) } as usize
    {
        return None;
    }
    let cell = unsafe { ffi::PyTuple_GetItem(closure, index as ffi::Py_ssize_t) };
    if unsafe { ffi::Py_TYPE(cell) } != ptr::addr_of_mut!(PyCell_Type) {
        return None;
    }
    // PyCell_Get is an INCREF-only native read. The pinned actual function's
    // closure still owns this value, and there is no callback between this
    // read and balancing that temporary reference, so DECREF cannot finalize
    // it. Return a borrow without introducing a lasting extra closure root.
    let value = unsafe { PyCell_Get(cell) };
    if !value.is_null() {
        unsafe { ffi::Py_DECREF(value) };
    }
    (!value.is_null()).then_some(value)
}

pub(super) fn matches_decorator(owner: &Owner<'_>, function: &Bound<'_, PyAny>) -> PyResult<bool> {
    let py = owner.owner().py();
    if !owner.data().factory {
        return owner.data().catalog.matches_function(
            py,
            owner,
            Helper::Dataclass,
            function.as_ptr(),
        );
    }
    matches_wrapper(owner, function.as_ptr())
}

fn matches_wrapper(owner: &Owner<'_>, function: *mut ffi::PyObject) -> PyResult<bool> {
    let py = owner.owner().py();
    if !plain_entry(function) || !no_defaults(function) {
        return Ok(false);
    }
    let role = CodeRole::Template(Template::DataclassWrapper);
    let raw = function.cast::<ffi::PyFunctionObject>();
    if !owner
        .data()
        .catalog
        .matches_code(py, owner, role, unsafe { (*raw).func_code })?
    {
        return Ok(false);
    }
    let Some(root) = owner
        .data()
        .catalog
        .function(py, owner, Helper::Dataclass)?
    else {
        return Ok(false);
    };
    let root = root.as_ptr().cast::<ffi::PyFunctionObject>();
    if unsafe {
        (*raw).func_globals != (*root).func_globals || (*raw).func_builtins != (*root).func_builtins
    } {
        return Ok(false);
    }
    let recipe = owner.data().catalog.recipe(role);
    let closure = unsafe { (*raw).func_closure };
    if closure.is_null()
        || unsafe { ffi::PyTuple_CheckExact(closure) } == 0
        || unsafe { ffi::PyTuple_Size(closure) } as usize != recipe.closure_len()
    {
        return Ok(false);
    }
    for (name, expected) in option_values(&owner.data().options) {
        let Some(index) = recipe.closure_index(name) else {
            return Ok(false);
        };
        if closure_value(function, index) != Some(boolean(expected)) {
            return Ok(false);
        }
    }
    recorded_function(
        owner,
        function,
        native::DECORATOR,
        owner.data().decorator_weak,
    )
}

pub(super) fn prepared_decorator_matches(owner: &Owner<'_>) -> PyResult<bool> {
    if !owner.data().factory {
        return invocation::validate_catalog(owner);
    }
    let Some(function) = weak_function(owner, owner.data().decorator_weak)? else {
        return Ok(false);
    };
    matches_wrapper(owner, function.as_ptr())
}

fn boolean(value: bool) -> *mut ffi::PyObject {
    unsafe {
        if value {
            ffi::Py_True()
        } else {
            ffi::Py_False()
        }
    }
}

fn options_in_frame(
    owner: &Owner<'_>,
    frame: Frame<'_>,
    role: Role,
    entering: bool,
) -> PyResult<bool> {
    let py = owner.owner().py();
    let Some(code) = role.code() else {
        return Ok(false);
    };
    let recipe = owner.data().catalog.recipe(code);
    for (name, expected) in option_values(&owner.data().options) {
        let actual = if role == Role::Decorator {
            let Some(index) = recipe.closure_index(name) else {
                return Ok(false);
            };
            closure_value(frame.function(), index).unwrap_or(ptr::null_mut())
        } else if entering {
            frame.parameter(py, recipe, name)?
        } else {
            frame.executing(py, recipe, name)?
        };
        if actual != boolean(expected) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Only post-bind validation of an actually supported native operand. Early
/// bind_pending_type still uses its explicit native construction-info witness;
/// it has not published StrictClassData::Pending and must not use this helper.
pub(super) fn matches_native_pending_class(
    owner: &Owner<'_>,
    expected: &ClassPlan,
    class: &Bound<'_, PyAny>,
) -> PyResult<bool> {
    let invocation::SourceGlobals::Interpreter {
        verified,
        execution,
        invocation,
    } = &owner.data().source_globals
    else {
        return Ok(false);
    };
    if !matches!(owner.data().phase.get(), Phase::Bound | Phase::Applying)
        || expected.phase.get() != super::adoption::Phase::Bound
        || expected.actual_class.get() != class.as_ptr() as usize
    {
        return Ok(false);
    }
    let Some(actual) = crate::strict_class_state::for_constructed_type(class.py(), class)? else {
        return Ok(false);
    };
    if !actual.is_interpreter_construction()
        || !actual.is_pending_type()
        || actual.owner().as_ptr() as usize != expected.actual_owner.get()
        || actual.source() != &expected.fact.identity
        || !std::sync::Arc::ptr_eq(actual.verified_module(), verified)
        || !actual.execution_ref().same_execution(execution)
        || !actual.matches_interpreter_completion(invocation)
        || !std::sync::Arc::ptr_eq(actual.namespace_execution(), &expected.namespace)
        || !actual.matches_active_dataclass_owner(owner.owner())?
    {
        return Ok(false);
    }
    let Some(namespace) = actual.dataclass_namespace()? else {
        return Ok(false);
    };
    Ok(namespace.owner.as_ptr() == owner.owner().as_ptr()
        && std::ptr::eq(namespace.plan.as_ref(), expected))
}

/// Retained Apply keeps its actual original/copy operand support. This
/// independent Pending proof does not widen the native result-only helper or
/// recover a type from the recorded comparison address.
pub(super) fn matches_retained_pending_class(
    owner: &Owner<'_>,
    expected: &ClassPlan,
    class: &Bound<'_, PyAny>,
) -> PyResult<bool> {
    let invocation::SourceGlobals::Retained(globals_index) = &owner.data().source_globals else {
        return Ok(false);
    };
    if !matches!(owner.data().phase.get(), Phase::Bound | Phase::Applying)
        || expected.phase.get() != super::adoption::Phase::Bound
        || expected.actual_class.get() != class.as_ptr() as usize
    {
        return Ok(false);
    }
    let py = class.py();
    let Some(actual) = crate::strict_class_state::for_constructed_type(py, class)? else {
        return Ok(false);
    };
    if actual.is_interpreter_construction()
        || !actual.is_pending_type()
        || actual.owner().as_ptr() as usize != expected.actual_owner.get()
        || actual.fact() != &expected.fact
        || actual.verified_module().type_facts().facts().source_digest != expected.source_digest
        || !std::sync::Arc::ptr_eq(actual.namespace_execution(), &expected.namespace)
        || !expected
            .namespace
            .matches_source_execution(actual.verified_module(), actual.execution_ref())
        || !actual.matches_active_dataclass_owner(owner.owner())?
    {
        return Ok(false);
    }
    // The retained invocation already owns this actual globals edge. Validate
    // its one module policy; a source/digest or reused raw address is not enough.
    let globals = owner.reference(*globals_index)?.cast_into::<PyDict>()?;
    drop(
        actual
            .execution_ref()
            .acquire_owner(py, &globals, actual.verified_module())?,
    );
    let Some(namespace) = actual.dataclass_namespace()? else {
        return Ok(false);
    };
    Ok(namespace.owner.as_ptr() == owner.owner().as_ptr()
        && std::ptr::eq(namespace.plan.as_ref(), expected))
}

pub(super) fn matches_class(owner: &Owner<'_>, class: *mut ffi::PyObject) -> PyResult<bool> {
    let expected = plan(owner)?;
    if class.is_null()
        || expected.actual_class.get() != class as usize
        || unsafe { ffi::PyType_Check(class) } == 0
    {
        return Ok(false);
    }
    if matches!(
        &owner.data().source_globals,
        invocation::SourceGlobals::Interpreter { .. }
    ) {
        let actual = unsafe { Borrowed::<PyAny>::from_ptr(owner.owner().py(), class) };
        return matches_native_pending_class(owner, expected, &actual);
    }
    let actual = unsafe { Borrowed::<PyAny>::from_ptr(owner.owner().py(), class) };
    matches_retained_pending_class(owner, expected, &actual)
}

/// A callback-bound borrowed dictionary. For native post-Apply validation the
/// real selected-result token, held by the native completion caller, supports
/// it throughout this callback. The temporary weak upgrade does not provide
/// support for a later callback and this pointer is never stored.
pub(super) fn actual_class_dictionary(owner: &Owner<'_>) -> PyResult<*mut ffi::PyObject> {
    if let Some(class) = super::slots::completed_native_result(owner)? {
        let dictionary = unsafe { (*class.as_ptr().cast::<ffi::PyTypeObject>()).tp_dict };
        require(
            owner,
            !dictionary.is_null() && unsafe { ffi::PyDict_CheckExact(dictionary) } != 0,
            "native selected dataclass namespace changed",
        )?;
        return Ok(dictionary);
    }
    let class = plan(owner)?.actual_class.get() as *mut ffi::PyObject;
    require(
        owner,
        matches_class(owner, class)?,
        "dataclass actual bound class changed",
    )?;
    Ok(unsafe { (*class.cast::<ffi::PyTypeObject>()).tp_dict })
}

pub(super) fn matches_fields(owner: &Owner<'_>) -> PyResult<bool> {
    let py = owner.owner().py();
    let fields = &plan(owner)?.generation.fields;
    let Some(actual) =
        (unsafe { dictionary_value(actual_class_dictionary(owner)?, "__dataclass_fields__") })
    else {
        return Ok(false);
    };
    if unsafe { ffi::PyDict_CheckExact(actual) } == 0
        || unsafe { ffi::PyDict_Size(actual) } as usize != fields.len()
    {
        return Ok(false);
    }
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    let mut generator_order = fields
        .iter()
        .filter(|field| field.role != FieldRole::ClassVariable);
    while unsafe { ffi::PyDict_Next(actual, &mut position, &mut key, &mut value) } != 0 {
        let Some(expected) = fields
            .iter()
            .find(|field| unsafe { text_is(key, &field.name) })
        else {
            return Ok(false);
        };
        let actual = unsafe { Bound::from_borrowed_ptr(py, value) };
        if !super::fields::matches_field(py, &owner.data().catalog, owner, expected, &actual)? {
            return Ok(false);
        }
        if expected.role != FieldRole::ClassVariable
            && generator_order
                .next()
                .is_none_or(|next| next.name != expected.name)
        {
            return Ok(false);
        }
    }
    Ok(generator_order.next().is_none())
}

pub(super) fn matches_parameters(owner: &Owner<'_>) -> PyResult<bool> {
    let py = owner.owner().py();
    let Some(parameters) =
        (unsafe { dictionary_value(actual_class_dictionary(owner)?, "__dataclass_params__") })
    else {
        return Ok(false);
    };
    let parameters = unsafe { Bound::from_borrowed_ptr(py, parameters) };
    for (name, expected) in option_values(&owner.data().options) {
        let Some(actual) =
            owner
                .data()
                .catalog
                .member(py, owner, &parameters, StructType::Parameters, name)?
        else {
            return Ok(false);
        };
        if actual.as_ptr() != boolean(expected) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn matches_helper_frame(
    owner: &Owner<'_>,
    frame: Frame<'_>,
    role: Role,
) -> PyResult<bool> {
    let py = owner.owner().py();
    let Some(code) = role.code() else {
        return Ok(false);
    };
    if !owner
        .data()
        .catalog
        .matches_code(py, owner, code, frame.code())?
    {
        return Ok(false);
    }
    let matched = match code {
        CodeRole::Helper(helper) => {
            owner
                .data()
                .catalog
                .matches_function(py, owner, helper, frame.function())?
        }
        CodeRole::Template(Template::DataclassWrapper) => matches_wrapper(owner, frame.function())?,
        CodeRole::Template(Template::ReprDecorator) => {
            super::method_values::repr_decorator_matches(owner, frame.function())?
        }
        CodeRole::Template(_) => false,
    };
    if !matched {
        return Ok(false);
    }
    let raw = frame.function().cast::<ffi::PyFunctionObject>();
    Ok(unsafe {
        (*raw).func_code == frame.code()
            && (*raw).func_globals == frame.globals()
            && (*raw).func_builtins == frame.builtins()
    })
}

pub(super) fn active_role(owner: &Owner<'_>, frame: Frame<'_>) -> PyResult<Role> {
    let role = Role::from_native(frame.role()).ok_or_else(|| {
        strict_runtime_unavailable(owner.owner().py(), "unknown dataclass frame role")
    })?;
    let matched = if role.code().is_some() {
        matches_helper_frame(owner, frame, role)?
    } else {
        matches_generated_frame(owner, frame, role)?
    };
    require(
        owner,
        matched,
        "dataclass executing frame changed after admission",
    )?;
    Ok(role)
}

fn matches_generated_frame(owner: &Owner<'_>, frame: Frame<'_>, role: Role) -> PyResult<bool> {
    let py = owner.owner().py();
    if !plain_entry(frame.function())
        || !no_defaults(frame.function())
        || !no_closure(frame.function())
    {
        return Ok(false);
    }
    let raw = frame.function().cast::<ffi::PyFunctionObject>();
    if unsafe { (*raw).func_code } != frame.code()
        || unsafe { (*raw).func_globals } != frame.globals()
        || unsafe { (*raw).func_builtins } != frame.builtins()
        || !unsafe { super::invocation::matches_source_globals(owner, frame.globals())? }
    {
        return Ok(false);
    }
    let Some(code) = owner.data().code.get() else {
        return Ok(false);
    };
    let tree = owner.reference(owner.data().generated_code)?;
    match role {
        Role::GeneratedExec => code.matches_root(py, tree.as_ptr(), frame.code()),
        Role::GeneratedFactory => Ok(code.matches_factory(py, tree.as_ptr(), frame.code())?
            && recorded_function(
                owner,
                frame.function(),
                native::GENERATED_FACTORY,
                owner.data().factory_weak,
            )?),
        _ => Ok(false),
    }
}

fn expected_attribute(
    owner: &Owner<'_>,
    role: Role,
    name: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
) -> PyResult<bool> {
    if role == Role::SetReplace {
        return Ok(unsafe { text_is(name, "__replace__") }
            && owner.data().catalog.matches_function(
                owner.owner().py(),
                owner,
                Helper::Replace,
                value,
            )?);
    }
    if role != Role::SetMatchArgs
        || !unsafe { text_is(name, "__match_args__") }
        || !owner.data().options.match_args
        || value.is_null()
        || unsafe { ffi::PyTuple_CheckExact(value) } == 0
    {
        return Ok(false);
    }
    let fields = plan(owner)?
        .generation
        .fields
        .iter()
        .filter(|field| field.role != FieldRole::ClassVariable && field.init && !field.kw_only);
    let mut count = 0;
    for field in fields {
        if count >= unsafe { ffi::PyTuple_Size(value) }
            || !unsafe { text_is(ffi::PyTuple_GetItem(value, count), &field.name) }
        {
            return Ok(false);
        }
        count += 1;
    }
    Ok(count == unsafe { ffi::PyTuple_Size(value) })
}

unsafe extern "C" fn enter(
    raw_owner: *mut ffi::PyObject,
    stage: c_uint,
    parent: *const RawFrameView,
    child: *const RawFrameView,
    output: *mut c_uint,
) -> c_int {
    callback(raw_owner, |owner| {
        require(
            owner,
            !output.is_null(),
            "dataclass entry has no role output",
        )?;
        let child = frame(owner, child)?;
        let py = owner.owner().py();
        let role = if parent.is_null() {
            let expected = match stage {
                native::ROOT_FACTORY
                    if owner.data().phase.get() == Phase::Factory && owner.data().factory =>
                {
                    Role::Dataclass
                }
                native::ROOT_APPLY if owner.data().phase.get() == Phase::Applying => {
                    if owner.data().factory {
                        Role::Decorator
                    } else {
                        Role::Dataclass
                    }
                }
                _ => {
                    return Err(strict_runtime_unavailable(
                        py,
                        "dataclass root phase was replayed",
                    ));
                }
            };
            require(
                owner,
                !owner.data().root_entered.get()
                    && invocation::validate_catalog(owner)?
                    && matches_helper_frame(owner, child, expected)?
                    && options_in_frame(owner, child, expected, true)?,
                "dataclass root operands did not authenticate",
            )?;
            let class = child.parameter(
                py,
                owner.data().catalog.recipe(expected.code().unwrap()),
                "cls",
            )?;
            require(
                owner,
                if stage == native::ROOT_FACTORY {
                    class == unsafe { ffi::Py_None() }
                } else {
                    matches_class(owner, class)?
                },
                "dataclass root class changed",
            )?;
            owner.data().root_entered.set(true);
            expected
        } else {
            require(
                owner,
                owner.data().phase.get() == Phase::Applying,
                "dataclass child outlived application",
            )?;
            let parent = frame(owner, parent)?;
            let parent_role = active_role(owner, parent)?;
            if stage == native::GENERATED_EXEC {
                let edge = parent_role.code().and_then(|role| {
                    parent
                        .instruction()
                        .and_then(|offset| owner.data().catalog.edge(role, offset))
                });
                require(
                    owner,
                    edge == Some(Edge::ExecuteSource)
                        && matches_generated_frame(owner, child, Role::GeneratedExec)?
                        && matches_fields(owner)?
                        && matches_parameters(owner)?,
                    "dataclass generated exec edge changed",
                )?;
                let code = owner.data().code.get().unwrap();
                require(
                    owner,
                    !code.exec_entered.replace(true),
                    "dataclass generated exec was replayed",
                )?;
                Role::GeneratedExec
            } else {
                require(owner, stage == 0, "unknown dataclass child entry stage")?;
                if parent_role == Role::GeneratedFactory {
                    let Some(selected) =
                        super::producer_protocol::enter_factory_child(owner, parent, child)?
                    else {
                        return Ok(0);
                    };
                    unsafe {
                        *output = selected as c_uint;
                    }
                    return Ok(1);
                }
                let Some(edge) = parent_role.code().and_then(|role| {
                    parent
                        .instruction()
                        .and_then(|offset| owner.data().catalog.edge(role, offset))
                }) else {
                    return Ok(0); // Ordinary annotation/inspect/user callbacks have no role.
                };
                let selected = match edge {
                    Edge::BareDataclassApply => Role::Decorator,
                    Edge::ProcessClass => Role::Process,
                    Edge::InstallMethods => Role::Install,
                    Edge::InstallReplace => Role::SetReplace,
                    Edge::InstallMatchArgs => Role::SetMatchArgs,
                    Edge::PrepareSlots => Role::Slots,
                    Edge::InvokeFactory => Role::GeneratedFactory,
                    _ => {
                        let selected = super::producer_protocol::enter(owner, parent, child, edge)?;
                        unsafe {
                            *output = selected as c_uint;
                        }
                        return Ok(1);
                    }
                };
                let matched = if selected.code().is_some() {
                    matches_helper_frame(owner, child, selected)?
                } else {
                    matches_generated_frame(owner, child, selected)?
                };
                require(
                    owner,
                    invocation::validate_catalog(owner)? && matched,
                    "dataclass child callee changed",
                )?;
                if matches!(selected, Role::Decorator | Role::Process) {
                    require(
                        owner,
                        options_in_frame(owner, child, selected, true)?,
                        "dataclass child options changed",
                    )?;
                }
                if let Some(role) = selected.code() {
                    let recipe = owner.data().catalog.recipe(role);
                    require(
                        owner,
                        matches_class(owner, child.parameter(py, recipe, "cls")?)?,
                        "dataclass child class changed",
                    )?;
                    if selected == Role::Install {
                        let this = child.parameter(py, recipe, "self")?;
                        let parent_recipe =
                            owner.data().catalog.recipe(parent_role.code().unwrap());
                        require(
                            owner,
                            this == parent.executing(py, parent_recipe, "func_builder")?
                                && owner.data().catalog.matches_structure(
                                    py,
                                    owner,
                                    StructType::Builder,
                                    this,
                                )?
                                && matches_fields(owner)?
                                && matches_parameters(owner)?,
                            "dataclass builder operands changed",
                        )?;
                    } else if selected == Role::Slots {
                        super::slots::enter(owner, parent, child)?;
                    } else if matches!(selected, Role::SetReplace | Role::SetMatchArgs) {
                        require(
                            owner,
                            matches_fields(owner)?
                                && matches_parameters(owner)?
                                && expected_attribute(
                                    owner,
                                    selected,
                                    child.parameter(py, recipe, "name")?,
                                    child.parameter(py, recipe, "value")?,
                                )?,
                            "dataclass metadata assignment changed",
                        )?;
                    }
                } else {
                    let code = owner.data().code.get().unwrap();
                    require(
                        owner,
                        super::operands::factory_values(owner, child, true)?
                            && matches_fields(owner)?
                            && !code.factory_entered.replace(true),
                        "dataclass generated factory arguments changed or replayed",
                    )?;
                }
                selected
            }
        };
        unsafe {
            *output = role as c_uint;
        }
        Ok(1)
    })
}

fn creation_role(
    owner: &Owner<'_>,
    producer: Frame<'_>,
    code: *mut ffi::PyObject,
) -> PyResult<Option<c_uint>> {
    let py = owner.owner().py();
    if let Some(selected) = super::producer_protocol::creation(owner, producer, code)? {
        return Ok(Some(selected.native(owner)?));
    }
    match active_role(owner, producer)? {
        Role::Dataclass => {
            require(
                owner,
                matches!(owner.data().phase.get(), Phase::Factory | Phase::Applying)
                    && options_in_frame(owner, producer, Role::Dataclass, false)?,
                "dataclass decorator creation options changed",
            )?;
            if owner.data().catalog.matches_code(
                py,
                owner,
                CodeRole::Template(Template::DataclassWrapper),
                code,
            )? {
                Ok(Some(native::DECORATOR))
            } else {
                Ok(None)
            }
        }
        Role::GeneratedExec => {
            let generated = owner.data().code.get().unwrap();
            let tree = owner.reference(owner.data().generated_code)?;
            require(
                owner,
                generated.matches_factory(py, tree.as_ptr(), code)?,
                "dataclass exec attempted to create an unplanned factory",
            )?;
            Ok(Some(native::GENERATED_FACTORY))
        }
        // The process helper creates ordinary generator expressions. They do
        // not acquire invocation records or become generated-member evidence.
        Role::Process | Role::Decorator | Role::Install | Role::SetReplace | Role::SetMatchArgs => {
            Ok(None)
        }
        _ => Ok(None), // Ordinary generator expressions acquire no creation role.
    }
}

unsafe extern "C" fn create(
    raw_owner: *mut ffi::PyObject,
    producer: *const RawFrameView,
    code: *mut ffi::PyObject,
    output: *mut c_uint,
) -> c_int {
    callback(raw_owner, |owner| {
        require(
            owner,
            !output.is_null(),
            "dataclass creation has no role output",
        )?;
        let producer = frame(owner, producer)?;
        let Some(role) = creation_role(owner, producer, code)? else {
            return Ok(0);
        };
        unsafe {
            *output = role;
        }
        Ok(1)
    })
}

unsafe extern "C" fn created(
    raw_owner: *mut ffi::PyObject,
    invocation: *mut ffi::PyObject,
    producer: *const RawFrameView,
    function: *mut ffi::PyObject,
    role: c_uint,
) -> c_int {
    callback(raw_owner, |owner| {
        let py = owner.owner().py();
        let producer = frame(owner, producer)?;
        require(
            owner,
            invocation == native_invocation(owner)?.as_ptr()
                && !function.is_null()
                && unsafe { ffi::PyFunction_Check(function) } != 0,
            "dataclass unpublished function has foreign invocation coordinates",
        )?;
        let raw = function.cast::<ffi::PyFunctionObject>();
        require(
            owner,
            creation_role(owner, producer, unsafe { (*raw).func_code })? == Some(role)
                && native::predicate(py, unsafe {
                    native::PyFunction_MatchesSoacDataclassCreation(function, invocation, role)
                })?
                && unsafe { (*raw).func_globals } == producer.globals()
                && unsafe { (*raw).func_builtins } == producer.builtins(),
            "dataclass unpublished function changed",
        )?;
        if super::producer_protocol::created(owner, producer, function, role)? {
            return Ok(0);
        }
        let (claimed, index) = match role {
            native::DECORATOR => (&owner.data().decorator_created, owner.data().decorator_weak),
            native::GENERATED_FACTORY => (&owner.data().factory_created, owner.data().factory_weak),
            _ => {
                return Err(strict_runtime_unavailable(
                    py,
                    "unplanned dataclass function publication",
                ));
            }
        };
        // Create is called repeatedly around allocations and stays pure.
        // Created alone consumes this slot, before native GCtrack/CREATE.
        require(
            owner,
            !claimed.replace(true),
            "dataclass function birth was replayed",
        )?;
        let weak = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                ffi::PyWeakref_NewRef(function, ptr::null_mut()),
            )?
        };
        require(
            owner,
            creation_role(owner, producer, unsafe { (*raw).func_code })? == Some(role)
                && native::predicate(py, unsafe {
                    native::PyFunction_MatchesSoacDataclassCreation(function, invocation, role)
                })?,
            "dataclass function changed while publishing its weak witness",
        )?;
        owner.bind_reserved_reference(index, weak)?;
        Ok(0)
    })
}

fn bridge_operands(
    owner: &Owner<'_>,
    parent: Frame<'_>,
    builtin: *mut ffi::PyObject,
    kind: c_uint,
    args: &[*mut ffi::PyObject],
) -> PyResult<()> {
    let py = owner.owner().py();
    require(
        owner,
        owner.data().phase.get() == Phase::Applying
            && invocation::validate_catalog(owner)?
            && builtin == unsafe { native::PySoac_GetDataclassBuiltin(kind) },
        "dataclass native bridge identity changed",
    )?;
    let role = active_role(owner, parent)?;
    let Some(code_role) = role.code() else {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass bridge has no stdlib producer",
        ));
    };
    let edge = parent
        .instruction()
        .and_then(|offset| owner.data().catalog.edge(code_role, offset));
    let recipe = owner.data().catalog.recipe(code_role);
    match kind {
        native::NEW_SLOTS => super::slots::bridge(owner, parent, args),
        native::SOURCE => {
            require(
                owner,
                edge == Some(Edge::RecordSource),
                "dataclass source bridge has another producer",
            )?;
            super::producer_protocol::source(owner, parent, args)
        }
        native::EXEC => {
            let code =
                owner.data().code.get().ok_or_else(|| {
                    strict_runtime_unavailable(py, "dataclass transcript is absent")
                })?;
            require(
                owner,
                edge == Some(Edge::ExecuteSource)
                    && args.len() == 4
                    && args[0]
                        == unsafe { native::PySoac_GetDataclassBuiltin(native::BUILTIN_EXEC) }
                    && code.matches_source(args[1])
                    && code.source_count.get() == plan(owner)?.generation.fragments.len()
                    && unsafe { super::invocation::matches_source_globals(owner, args[2])? }
                    && args[1] == parent.executing(py, recipe, "txt")?
                    && args[3] == parent.executing(py, recipe, "ns")?
                    && unsafe { ffi::PyDict_CheckExact(args[3]) } != 0
                    && unsafe { ffi::PyDict_Size(args[3]) } == 0
                    && matches_fields(owner)?
                    && matches_parameters(owner)?,
                "dataclass exec source or operands changed",
            )
        }
        native::MEMBER => {
            require(
                owner,
                args.len() == 4
                    && args[0]
                        == unsafe { native::PySoac_GetDataclassBuiltin(native::BUILTIN_SETATTR) }
                    && matches_class(owner, args[1])?
                    && args[1] == parent.executing(py, recipe, "cls")?
                    && args[2] == parent.executing(py, recipe, "name")?,
                "dataclass member bridge operands changed",
            )?;
            if matches!(role, Role::SetReplace | Role::SetMatchArgs) {
                return require(
                    owner,
                    edge == Some(Edge::SetMember)
                        && args[3] == parent.executing(py, recipe, "value")?
                        && expected_attribute(owner, role, args[2], args[3])?,
                    "dataclass metadata member bridge changed",
                );
            }
            require(
                owner,
                (role == Role::SetGenerated
                    && edge == Some(Edge::SetMember)
                    && args[3] == parent.executing(py, recipe, "value")?)
                    || (role == Role::Install
                        && edge == Some(Edge::InstallUnconditional)
                        && args[3] == parent.executing(py, recipe, "fn")?),
                "dataclass method install has another producer",
            )?;
            let index = super::produced::fragment_index(owner, args[2])
                .ok_or_else(|| strict_runtime_unavailable(py, "dataclass member role is absent"))?;
            let fragment = &plan(owner)?.generation.fragments[index];
            require(
                owner,
                (role != Role::Install || fragment.unconditional)
                    && member_matches(
                        owner,
                        args[1],
                        plan(owner)?.actual_owner.get() as *mut ffi::PyObject,
                        args[2],
                        args[3],
                        super::produced::native_role(fragment.role),
                        false,
                    )?,
                "generated member does not match its declared role",
            )?;
            // These exact components have NULL kwdefaults; the native API's
            // successful path is allocation-free and invokes only our pure
            // component validator. Repeated bridge checks do not re-adopt.
            super::method_values::adopt_components(owner, index, args[3])?;
            require(
                owner,
                member_matches(
                    owner,
                    args[1],
                    plan(owner)?.actual_owner.get() as *mut ffi::PyObject,
                    args[2],
                    args[3],
                    super::produced::native_role(fragment.role),
                    true,
                )?,
                "generated member components changed during adoption",
            )
        }
        _ => Err(strict_runtime_unavailable(
            py,
            "dataclass generated-value/source bridge is not admitted by this plan",
        )),
    }
}

unsafe extern "C" fn bridge(
    raw_owner: *mut ffi::PyObject,
    parent: *const RawFrameView,
    builtin: *mut ffi::PyObject,
    kind: c_uint,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> c_int {
    callback(raw_owner, |owner| {
        require(
            owner,
            nargs >= 0 && (nargs == 0 || !args.is_null()),
            "invalid dataclass native bridge argument buffer",
        )?;
        let args = if nargs == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(args, nargs as usize) }
        };
        bridge_operands(owner, frame(owner, parent)?, builtin, kind, args)?;
        Ok(0)
    })
}

unsafe extern "C" fn prepare_slots(
    raw_owner: *mut ffi::PyObject,
    producer: *const RawFrameView,
    metaclass: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    bases: *mut ffi::PyObject,
    namespace: *mut ffi::PyObject,
    original: *mut ffi::PyObject,
    output: *mut *mut ffi::PyObject,
) -> c_int {
    callback(raw_owner, |owner| {
        require(
            owner,
            !output.is_null(),
            "slots preparation has no handle output",
        )?;
        let producer = frame(owner, producer)?;
        let args = [metaclass, name, bases, namespace, original];
        bridge_operands(
            owner,
            producer,
            unsafe { native::PySoac_GetDataclassBuiltin(native::NEW_SLOTS) },
            native::NEW_SLOTS,
            &args,
        )?;
        let handle = super::slots::prepare(owner, producer, &args)?;
        unsafe {
            *output = handle.into_ptr();
        }
        Ok(0)
    })
}

unsafe extern "C" fn compiled(
    raw_owner: *mut ffi::PyObject,
    parent: *const RawFrameView,
    source: *mut ffi::PyObject,
    root: *mut ffi::PyObject,
    weak_tree: *mut ffi::PyObject,
) -> c_int {
    callback(raw_owner, |owner| {
        let py = owner.owner().py();
        let parent = frame(owner, parent)?;
        require(
            owner,
            active_role(owner, parent)? == Role::Install,
            "dataclass compiler result has another producer",
        )?;
        let recipe = owner
            .data()
            .catalog
            .recipe(CodeRole::Helper(Helper::BuilderInstall));
        // The native EXEC bridge revalidates its ACTUAL already-evaluated
        // argv after compilation and weak-tree/cache allocation, immediately
        // before this callback (with no allocation or Python callback gap).
        // In particular, bridge_operands still authenticates args[2] against
        // the source globals. The compiler-result receipt does not expose that
        // operand: do not manufacture argv from the helper frame's globals or
        // reread a mutable Builder attribute after exec captured its value.
        let namespace = parent.executing(py, recipe, "ns")?;
        let code = owner
            .data()
            .code
            .get()
            .ok_or_else(|| strict_runtime_unavailable(py, "dataclass transcript is absent"))?;
        require(
            owner,
            owner.data().phase.get() == Phase::Applying
                && invocation::validate_catalog(owner)?
                && parent.instruction().and_then(|offset| {
                    owner
                        .data()
                        .catalog
                        .edge(CodeRole::Helper(Helper::BuilderInstall), offset)
                }) == Some(Edge::ExecuteSource)
                && code.matches_source(source)
                && code.source_count.get() == plan(owner)?.generation.fragments.len()
                && source == parent.executing(py, recipe, "txt")?
                && !namespace.is_null()
                && unsafe { ffi::PyDict_CheckExact(namespace) } != 0
                && unsafe { ffi::PyDict_Size(namespace) } == 0
                && matches_fields(owner)?
                && matches_parameters(owner)?,
            "dataclass compiler source or producer operands changed",
        )?;
        require(
            owner,
            code.bind_compiled(py, &plan(owner)?.generation, root, weak_tree)?,
            "dataclass native compiler result changed",
        )?;
        owner.bind_reserved_reference(owner.data().generated_code, unsafe {
            Bound::from_borrowed_ptr(py, weak_tree)
        })?;
        Ok(0)
    })
}

pub(super) fn validate_completed(
    owner: &Owner<'_>,
    class: &Bound<'_, PyAny>,
    namespace: &Bound<'_, PyDict>,
) -> PyResult<bool> {
    if owner.data().phase.get() != Phase::Applying
        || !matches_class(owner, class.as_ptr())?
        || namespace.as_ptr() != actual_class_dictionary(owner)?
        || !invocation::validate_catalog(owner)?
        || !matches_fields(owner)?
        || !matches_parameters(owner)?
    {
        return Ok(false);
    }
    let Some(code) = owner.data().code.get() else {
        return Ok(false);
    };
    if !code.compiled.get()
        || !code.exec_entered.get()
        || !code.factory_entered.get()
        || !owner.data().factory_created.get()
    {
        return Ok(false);
    }
    for member in &plan(owner)?.members {
        let Some(actual) = (unsafe { dictionary_value(namespace.as_ptr(), &member.name) }) else {
            return Ok(false);
        };
        match member.kind {
            MemberKind::Shared(helper) => {
                if !owner.data().catalog.matches_function(
                    owner.owner().py(),
                    owner,
                    helper,
                    actual,
                )? {
                    return Ok(false);
                }
            }
            MemberKind::Generated(role) => {
                if unsafe { ffi::PyFunction_Check(actual) } == 0 {
                    return Ok(false);
                }
                let name = unsafe { (*actual.cast::<ffi::PyFunctionObject>()).func_name };
                if !member_matches(
                    owner,
                    class.as_ptr(),
                    plan(owner)?.actual_owner.get() as *mut ffi::PyObject,
                    name,
                    actual,
                    super::produced::native_role(role),
                    true,
                )? || unsafe { native::PyFunction_GetSoacStrictId(actual) } == 0
                {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

/// The real native Apply result is still the completion caller's operand.
/// Validate its existing plan/births directly; an expired original is neither
/// recovered nor replaced by an extra owner. Retained completion remains above.
pub(super) fn validate_native_slots_members(
    owner: &Owner<'_>,
    selected_plan: &ClassPlan,
    class: &Bound<'_, PyAny>,
    namespace: &Bound<'_, PyDict>,
) -> PyResult<bool> {
    let Some(selected) = super::slots::completed_native_result(owner)? else {
        return Ok(false);
    };
    let original_plan = plan(owner)?;
    if selected.as_ptr() != class.as_ptr()
        || !matches_native_pending_class(owner, selected_plan, class)?
        || selected_plan
            .replacement_of
            .as_ref()
            .is_none_or(|original| !std::ptr::eq(original.as_ref(), original_plan))
        || namespace.as_ptr() != actual_class_dictionary(owner)?
        || !invocation::validate_catalog(owner)?
        || !matches_fields(owner)?
        || !matches_parameters(owner)?
    {
        return Ok(false);
    }
    let Some(code) = owner.data().code.get() else {
        return Ok(false);
    };
    if !code.compiled.get()
        || !code.exec_entered.get()
        || !code.factory_entered.get()
        || !owner.data().factory_created.get()
    {
        return Ok(false);
    }
    for member in &selected_plan.members {
        let Some(actual) = (unsafe { dictionary_value(namespace.as_ptr(), &member.name) }) else {
            return Ok(false);
        };
        match member.kind {
            MemberKind::Shared(helper) => {
                if !owner.data().catalog.matches_function(
                    owner.owner().py(),
                    owner,
                    helper,
                    actual,
                )? {
                    return Ok(false);
                }
            }
            MemberKind::Generated(role) => {
                if unsafe { ffi::PyFunction_Check(actual) } == 0 {
                    return Ok(false);
                }
                let name = unsafe { (*actual.cast::<ffi::PyFunctionObject>()).func_name };
                if !member_function_matches(
                    owner,
                    name,
                    actual,
                    super::produced::native_role(role),
                    true,
                )? || unsafe { native::PyFunction_GetSoacStrictId(actual) } == 0
                {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

unsafe extern "C" fn validate_member(
    raw_owner: *mut ffi::PyObject,
    class: *mut ffi::PyObject,
    class_owner: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    function: *mut ffi::PyObject,
    role: c_uint,
) -> c_int {
    callback(raw_owner, |owner| {
        require(
            owner,
            member_matches(owner, class, class_owner, name, function, role, true)?,
            "generated member policy did not authenticate",
        )?;
        Ok(0)
    })
}

fn member_matches(
    owner: &Owner<'_>,
    class: *mut ffi::PyObject,
    class_owner: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    function: *mut ffi::PyObject,
    role: u32,
    components: bool,
) -> PyResult<bool> {
    if owner.data().phase.get() != Phase::Applying
        || !matches_class(owner, class)?
        || plan(owner)?.actual_owner.get() != class_owner as usize
        || !invocation::validate_catalog(owner)?
        || !matches_fields(owner)?
        || !matches_parameters(owner)?
    {
        return Ok(false);
    }
    member_function_matches(owner, name, function, role, components)
}

/// Same immutable creation/code/default/closure/component proof for the
/// original's publication and the actual returned replacement's namespace.
fn member_function_matches(
    owner: &Owner<'_>,
    name: *mut ffi::PyObject,
    function: *mut ffi::PyObject,
    role: u32,
    components: bool,
) -> PyResult<bool> {
    let Some(index) = super::produced::fragment_index(owner, name) else {
        return Ok(false);
    };
    let fragment = &plan(owner)?.generation.fragments[index];
    if role != super::produced::native_role(fragment.role)
        || !plan(owner)?
            .members
            .iter()
            .any(|member| member.kind == MemberKind::Generated(fragment.role))
        || !super::method_values::function_matches(owner, index, function, false)?
    {
        return Ok(false);
    }
    let birth = &super::produced::methods(owner)?.methods[index];
    if components && !birth.components_adopted.get() {
        return Ok(false);
    }
    if let Some(annotation) = &birth.annotation {
        let Some(component) = annotation.function(owner)? else {
            return Ok(false);
        };
        if unsafe { (*function.cast::<ffi::PyFunctionObject>()).func_annotate }
            != component.as_ptr()
            || !super::method_values::annotation_matches(owner, index, component.as_ptr())?
            || (components
                && unsafe { native::PyFunction_GetSoacStrictId(component.as_ptr()) } == 0)
        {
            return Ok(false);
        }
    }
    if let Some(implementation) = &birth.implementation {
        let Some(component) = implementation.function(owner)? else {
            return Ok(false);
        };
        if !super::method_values::function_matches(owner, index, component.as_ptr(), true)?
            || (components
                && unsafe { native::PyFunction_GetSoacStrictId(component.as_ptr()) } == 0)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

unsafe extern "C" fn validate_component(
    raw_owner: *mut ffi::PyObject,
    method: *mut ffi::PyObject,
    component: *mut ffi::PyObject,
    kind: c_uint,
    closure_index: ffi::Py_ssize_t,
) -> c_int {
    callback(raw_owner, |owner| {
        require(
            owner,
            owner.data().phase.get() == Phase::Applying
                && invocation::validate_catalog(owner)?
                && super::method_values::component_matches(
                    owner,
                    method,
                    component,
                    kind,
                    closure_index,
                )?,
            "generated component does not match its owned relationship",
        )?;
        Ok(0)
    })
}
