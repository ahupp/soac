//! One source-selected invocation. Python edges live in the traversed owner;
//! all callback authority is explicit in the native view and this phase state.

use std::cell::{Cell, OnceCell};
use std::ffi::CStr;
use std::ptr;
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use soac_contracts::{ClassTypeFact, DataclassOptions};

use crate::strict_field_bindings::StrictFieldBinding;
use crate::strict_function::{AuthenticatedStrictFunction, ClassConstructionCaptures};
use crate::strict_interpreter::InterpreterInvocationIdentity;
use crate::strict_namespace::NamespaceExecution;
use crate::strict_runtime_unavailable;
use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{StrictModuleExecutionRef, VerifiedStrictModule};

use super::StdlibRecipes;
use super::adoption::{
    ClassPlan, DataclassConstruction, MemberKind, MemberPlan, Phase as ClassPhase,
};
use super::catalog::{Helper, HelperCatalog, dictionary_value, text_is};
use super::fields::FieldProjection;
use super::generation::GenerationPlan;
use super::native;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Preparing,
    Factory,
    Prepared,
    Construction,
    Bound,
    Applying,
    Completing,
    Complete,
    Declined,
    Failed,
}

/// The retained backend keeps its existing traversed globals edge. The native
/// backend keeps only the already-authenticated source/execution identities;
/// every query must supply a genuinely supported current globals dictionary.
pub(super) enum SourceGlobals {
    Retained(usize),
    Interpreter {
        verified: Arc<VerifiedStrictModule>,
        execution: StrictModuleExecutionRef,
        invocation: Arc<InterpreterInvocationIdentity>,
    },
}

enum SourceGlobalsInput<'a, 'py> {
    Retained(&'a Bound<'py, PyAny>),
    Interpreter {
        verified: Arc<VerifiedStrictModule>,
        execution: StrictModuleExecutionRef,
        invocation: Arc<InterpreterInvocationIdentity>,
    },
}

pub(super) struct InvocationData {
    pub(super) fact: ClassTypeFact,
    pub(super) options: DataclassOptions,
    pub(super) catalog: HelperCatalog,
    pub(super) phase: Cell<Phase>,
    pub(super) factory: bool,
    pub(super) root_entered: Cell<bool>,
    pub(super) invocation: usize,
    pub(super) source_globals: SourceGlobals,
    pub(super) generated_code: usize,
    pub(super) decorator_weak: usize,
    pub(super) factory_weak: usize,
    pub(super) decorator_created: Cell<bool>,
    pub(super) factory_created: Cell<bool>,
    pub(super) active_reference_count: Cell<usize>,
    pub(super) plan: OnceCell<Arc<ClassPlan>>,
    pub(super) replacement: OnceCell<super::slots::Replacement>,
    pub(super) slots_layout: OnceCell<super::slots::SlotsLayout>,
    pub(super) code: OnceCell<super::transcript::GeneratedCode>,
    pub(super) produced: OnceCell<super::produced::GeneratedMethods>,
    pub(super) own_field_bindings: OnceCell<Vec<usize>>,
}

unsafe impl StrictStateData for InvocationData {
    const TYPE_NAME: &'static CStr = c"soac._DataclassInvocation";

    fn on_terminal(&self) {
        if !matches!(self.phase.get(), Phase::Complete | Phase::Declined) {
            self.phase.set(Phase::Failed);
            if let Some(plan) = self.plan.get() {
                plan.fail();
            }
            if let Some(replacement) = self.replacement.get() {
                replacement.plan.fail();
            }
        }
    }
}

pub(super) type Owner<'py> = StrictStateRef<'py, InvocationData>;

pub(crate) struct PreparedDecorator<'py> {
    pub(crate) decorator: Bound<'py, PyAny>,
    pub(crate) owner: Bound<'py, PyAny>,
}

pub(super) fn option_values(options: &DataclassOptions) -> [(&'static str, bool); 10] {
    [
        ("init", options.init),
        ("repr", options.repr),
        ("eq", options.eq),
        ("order", options.order),
        ("unsafe_hash", options.unsafe_hash),
        ("frozen", options.frozen),
        ("match_args", options.match_args),
        ("kw_only", options.kw_only),
        ("slots", options.slots),
        ("weakref_slot", options.weakref_slot),
    ]
}

/// Validate the ordinary binding graph without creating argument containers
/// or invoking __bool__. Unknown arguments go through the ordinary factory
/// exactly once; they do not become an adapter's option authority.
unsafe fn options_match(
    options: &DataclassOptions,
    factory: bool,
    args: *const *mut ffi::PyObject,
    nargs: usize,
    names: *mut ffi::PyObject,
) -> bool {
    if !factory {
        return nargs == 0 && names.is_null() && options == &DataclassOptions::default();
    }
    if nargs > 1
        || (nargs != 0 && (args.is_null() || unsafe { *args } != unsafe { ffi::Py_None() }))
    {
        return false;
    }
    if !names.is_null() && unsafe { ffi::PyTuple_CheckExact(names) } == 0 {
        return false;
    }
    let count = if names.is_null() {
        0
    } else {
        (unsafe { ffi::PyTuple_Size(names) }) as usize
    };
    if count != 0 && args.is_null() {
        return false;
    }
    let expected = option_values(options);
    let mut actual = option_values(&DataclassOptions::default());
    let mut seen = [false; 10];
    let mut class_seen = nargs != 0;
    for index in 0..count {
        let name = unsafe { ffi::PyTuple_GetItem(names, index as ffi::Py_ssize_t) };
        let value = unsafe { *args.add(nargs + index) };
        if unsafe { text_is(name, "cls") } {
            if class_seen || value != unsafe { ffi::Py_None() } {
                return false;
            }
            class_seen = true;
            continue;
        }
        let Some(option) = actual
            .iter()
            .position(|(expected, _)| unsafe { text_is(name, expected) })
        else {
            return false;
        };
        if seen[option] {
            return false;
        }
        seen[option] = true;
        actual[option].1 = if value == unsafe { ffi::Py_True() } {
            true
        } else if value == unsafe { ffi::Py_False() } {
            false
        } else {
            return false;
        };
    }
    actual == expected
}

/// The caller has already authenticated its exact active source/site and
/// keeps the raw factory operands alive. None means no factory was executed.
unsafe fn prepare_owner<'py>(
    py: Python<'py>,
    fact: &ClassTypeFact,
    source_globals: SourceGlobalsInput<'_, 'py>,
    factory: bool,
    callable: &Bound<'py, PyAny>,
    args: *const *mut ffi::PyObject,
    nargs: usize,
    names: *mut ffi::PyObject,
) -> PyResult<Option<Owner<'py>>> {
    let Some(options) = fact
        .transform
        .as_ref()
        .and_then(|transform| transform.dataclass_options.as_ref())
    else {
        return Ok(None);
    };
    if !unsafe { options_match(options, factory, args, nargs, names) } {
        return Ok(None);
    }
    if unsafe { ffi::PyFunction_Check(callable.as_ptr()) } == 0 {
        return Ok(None);
    }
    let recipes = StdlibRecipes::load(py)?;
    let Some(captured) = HelperCatalog::capture(py, callable, &recipes)? else {
        return Ok(None);
    };
    let mut references = captured.references;
    let invocation = references.len();
    references.push(py.None().into_bound(py));
    let source_globals = match source_globals {
        SourceGlobalsInput::Retained(value) => {
            let index = references.len();
            references.push(value.clone());
            SourceGlobals::Retained(index)
        }
        SourceGlobalsInput::Interpreter {
            verified,
            execution,
            invocation,
        } => SourceGlobals::Interpreter {
            verified,
            execution,
            invocation,
        },
    };
    let generated_code = references.len();
    references.push(py.None().into_bound(py));
    let decorator_weak = references.len();
    references.push(py.None().into_bound(py));
    let factory_weak = references.len();
    references.push(py.None().into_bound(py));
    let active_reference_count = references.len();
    let owner = Owner::new(
        py,
        InvocationData {
            fact: fact.clone(),
            options: options.clone(),
            catalog: captured.catalog,
            phase: Cell::new(Phase::Preparing),
            factory,
            root_entered: Cell::new(false),
            invocation,
            source_globals,
            generated_code,
            decorator_weak,
            factory_weak,
            decorator_created: Cell::new(false),
            factory_created: Cell::new(false),
            active_reference_count: Cell::new(active_reference_count),
            plan: OnceCell::new(),
            replacement: OnceCell::new(),
            slots_layout: OnceCell::new(),
            code: OnceCell::new(),
            produced: OnceCell::new(),
            own_field_bindings: OnceCell::new(),
        },
        references.into_iter().map(Bound::unbind).collect(),
    )?;
    native::status(py, unsafe {
        native::PySoac_SetDataclassCallbacks(&super::protocol::CALLBACKS)
    })?;
    let native = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            native::PySoac_NewDataclassInvocation(owner.owner().as_ptr()),
        )?
    };
    owner.bind_reserved_reference(invocation, native)?;
    if !owner.data().catalog.validate(py, &owner, callable)? {
        decline_owner(&owner)?;
        return Ok(None);
    }
    owner.data().phase.set(if factory {
        Phase::Factory
    } else {
        Phase::Prepared
    });
    owner.data().root_entered.set(false);
    Ok(Some(owner))
}

/// Only prepares the same metadata invocation. The interpreter performs the
/// actual consuming native call and supplies its borrowed result after teardown.
pub(crate) unsafe fn prepare_native<'py>(
    py: Python<'py>,
    fact: &ClassTypeFact,
    verified: Arc<VerifiedStrictModule>,
    execution: StrictModuleExecutionRef,
    invocation: Arc<InterpreterInvocationIdentity>,
    actual_globals: Borrowed<'_, 'py, PyDict>,
    factory: bool,
    callable: Borrowed<'_, 'py, PyAny>,
    args: *const *mut ffi::PyObject,
    nargs: usize,
    names: *mut ffi::PyObject,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    drop(execution.acquire_owner(py, &actual_globals, &verified)?);
    let state = unsafe {
        prepare_owner(
            py,
            fact,
            SourceGlobalsInput::Interpreter {
                verified,
                execution,
                invocation,
            },
            factory,
            &callable,
            args,
            nargs,
            names,
        )
    }?;
    Ok(state.map(|state| state.owner().clone()))
}

/// Legacy retained transport delegates the same preparation/finish state
/// machine, but keeps its ordinary borrowed-vector calling convention.
pub(crate) unsafe fn prepare<'py>(
    py: Python<'py>,
    fact: &ClassTypeFact,
    source_globals: &Bound<'py, PyAny>,
    factory: bool,
    callable: &Bound<'py, PyAny>,
    args: *const *mut ffi::PyObject,
    nargs: usize,
    names: *mut ffi::PyObject,
) -> PyResult<Option<PreparedDecorator<'py>>> {
    let Some(owner) = (unsafe {
        prepare_owner(
            py,
            fact,
            SourceGlobalsInput::Retained(source_globals),
            factory,
            callable,
            args,
            nargs,
            names,
        )
    })?
    else {
        return Ok(None);
    };
    let decorator = if factory {
        let result = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                native::PySoac_DataclassVectorcall(
                    native_invocation(&owner)?.as_ptr(),
                    native::ROOT_FACTORY,
                    callable.as_ptr(),
                    args,
                    nargs,
                    names,
                ),
            )
        };
        let result = match result {
            Ok(value) => value,
            Err(error) => {
                let _ = fail_owner(&owner);
                return Err(error);
            }
        };
        finish_factory(owner.owner(), &result)?;
        result
    } else {
        callable.clone()
    };
    Ok(Some(PreparedDecorator {
        decorator,
        owner: owner.owner().clone(),
    }))
}

pub(crate) fn finish_factory(owner: &Bound<'_, PyAny>, result: &Bound<'_, PyAny>) -> PyResult<()> {
    let state = Owner::from_owner(owner.clone())?;
    if state.data().phase.get() != Phase::Factory
        || !state.data().factory
        || !state.data().root_entered.get()
        || !super::protocol::matches_decorator(&state, result)?
    {
        fail_owner(&state)?;
        return Err(strict_runtime_unavailable(
            owner.py(),
            "dataclass factory returned an unrecorded or changed decorator",
        ));
    }
    state.data().phase.set(Phase::Prepared);
    state.data().root_entered.set(false);
    Ok(())
}

/// The supplied object is borrowed from an actual native frame/function/EXEC
/// operand. This routine never recovers a dictionary from a stored address.
pub(super) unsafe fn matches_source_globals(
    owner: &Owner<'_>,
    actual: *mut ffi::PyObject,
) -> PyResult<bool> {
    match &owner.data().source_globals {
        SourceGlobals::Retained(index) => Ok(owner.reference(*index)?.as_ptr() == actual),
        SourceGlobals::Interpreter {
            verified,
            execution,
            ..
        } => {
            if actual.is_null() || unsafe { ffi::PyDict_CheckExact(actual) } == 0 {
                return Ok(false);
            }
            let py = owner.owner().py();
            let globals = unsafe { Borrowed::<PyAny>::from_ptr(py, actual) }.cast::<PyDict>()?;
            drop(execution.acquire_owner(py, &globals, verified)?);
            Ok(true)
        }
    }
}

pub(crate) fn native_invocation_for<'py>(owner: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    native_invocation(&Owner::from_owner(owner.clone())?)
}

fn same_native_source(
    state: &Owner<'_>,
    fact: &ClassTypeFact,
    verified: &Arc<VerifiedStrictModule>,
    execution: &StrictModuleExecutionRef,
    invocation: &Arc<InterpreterInvocationIdentity>,
) -> bool {
    state.data().fact == *fact
        && matches!(&state.data().source_globals,
        SourceGlobals::Interpreter { verified: own_source, execution: own_execution,
            invocation: own_invocation }
            if Arc::ptr_eq(own_source, verified)
                && own_execution.same_execution(execution)
                && Arc::ptr_eq(own_invocation, invocation))
}

pub(crate) fn native_source_matches(
    owner: &Bound<'_, PyAny>,
    actual_invocation: *mut ffi::PyObject,
    fact: &ClassTypeFact,
    verified: &Arc<VerifiedStrictModule>,
    execution: &StrictModuleExecutionRef,
    invocation: &Arc<InterpreterInvocationIdentity>,
) -> PyResult<bool> {
    let state = Owner::from_owner(owner.clone())?;
    Ok(matches!(
        state.data().phase.get(),
        Phase::Factory | Phase::Prepared | Phase::Construction | Phase::Bound | Phase::Applying
    ) && same_native_source(&state, fact, verified, execution, invocation)
        && native_invocation(&state)?.as_ptr() == actual_invocation)
}

/// Only the closed native selected-call completion invokes this operation.
/// C independently proves owner is the SAME existing invocation.owner edge,
/// including failure. A failed Rust owner may already have cleared its outgoing
/// invocation slot; no missing edge is treated as an execution/admission grant.
pub(crate) fn native_completion_matches(
    owner: &Bound<'_, PyAny>,
    actual_invocation: *mut ffi::PyObject,
    fact: &ClassTypeFact,
    verified: &Arc<VerifiedStrictModule>,
    execution: &StrictModuleExecutionRef,
    invocation: &Arc<InterpreterInvocationIdentity>,
) -> PyResult<bool> {
    let state = Owner::from_owner(owner.clone())?;
    if actual_invocation.is_null()
        || !matches!(
            state.data().phase.get(),
            Phase::Factory | Phase::Applying | Phase::Failed
        )
        || !same_native_source(&state, fact, verified, execution, invocation)
    {
        return Ok(false);
    }
    let edge = state.reference(state.data().invocation)?;
    Ok(edge.as_ptr() == actual_invocation
        || (state.data().phase.get() == Phase::Failed && edge.is_none()))
}

pub(crate) fn fail_native_call(owner: &Bound<'_, PyAny>) -> PyResult<()> {
    fail_owner(&Owner::from_owner(owner.clone())?)
}

pub(super) fn native_invocation<'py>(owner: &Owner<'py>) -> PyResult<Bound<'py, PyAny>> {
    let value = owner.reference(owner.data().invocation)?;
    if value.is_none() {
        return Err(strict_runtime_unavailable(
            owner.owner().py(),
            "dataclass invocation is no longer active",
        ));
    }
    Ok(value)
}

pub(super) fn validate_catalog(owner: &Owner<'_>) -> PyResult<bool> {
    let py = owner.owner().py();
    let Some(root) = owner
        .data()
        .catalog
        .function(py, owner, Helper::Dataclass)?
    else {
        return Ok(false);
    };
    owner.data().catalog.validate(py, owner, &root)
}

pub(crate) fn prepare_construction<'py>(
    py: Python<'py>,
    owner: &Bound<'py, PyAny>,
    auth: &AuthenticatedStrictFunction<'_, 'py>,
    namespace: &Bound<'py, PyDict>,
    bases: &Bound<'py, PyTuple>,
    execution: &Arc<NamespaceExecution>,
    construction_captures: Option<&ClassConstructionCaptures<'py>>,
) -> PyResult<Option<DataclassConstruction<'py>>> {
    let state = Owner::from_owner(owner.clone())?;
    if state.data().phase.get() != Phase::Prepared
        || auth.origin().map(|origin| &origin.definition) != Some(&state.data().fact.identity)
        || !unsafe { matches_source_globals(&state, auth.globals()?.as_ptr())? }
        || !auth
            .verified_module()
            .type_facts()
            .facts()
            .classes
            .iter()
            .any(|fact| fact == &state.data().fact)
        || execution.source() != &state.data().fact.identity
        || !execution.is_completed()
    {
        fail_owner(&state)?;
        return Err(strict_runtime_unavailable(
            py,
            "dataclass construction was transferred or replayed",
        ));
    }
    // Both authenticated source paths now install the same Pending handle.
    // Keep declarations unresolved until the actual selected final result;
    // the source/execution join is still required before any native type exists.
    let pending_source = match &state.data().source_globals {
        SourceGlobals::Interpreter {
            verified,
            execution: source_execution,
            ..
        } => {
            auth.is_interpreter()
                && auth.interpreter_source_authority()?
                && Arc::ptr_eq(verified, auth.verified_module())
                && source_execution.same_execution(auth.execution_ref())
                && execution.matches_source_execution(verified, source_execution)
        }
        SourceGlobals::Retained(_) => {
            !auth.is_interpreter()
                && execution.matches_source_execution(auth.verified_module(), auth.execution_ref())
        }
    };
    if !pending_source {
        fail_owner(&state)?;
        return Err(strict_runtime_unavailable(
            py,
            "dataclass declaration has no matching pending source construction",
        ));
    }
    if !validate_catalog(&state)? || !super::protocol::prepared_decorator_matches(&state)? {
        decline_owner(&state)?;
        return Ok(None);
    }
    let Some(fields) = FieldProjection::capture(
        py,
        &state.data().catalog,
        &state,
        &state.data().fact,
        &state.data().options,
        namespace,
        bases,
    )?
    else {
        decline_owner(&state)?;
        return Ok(None);
    };
    let hash = unsafe { dictionary_value(namespace.as_ptr(), "__hash__") };
    let explicit_hash = hash.is_some_and(|hash| {
        hash != unsafe { ffi::Py_None() }
            || unsafe { dictionary_value(namespace.as_ptr(), "__eq__") }.is_none()
    });
    let has_post_init = unsafe { dictionary_value(namespace.as_ptr(), "__post_init__") }.is_some()
        || inherited_attribute(bases, "__post_init__");
    let Some(generation) = GenerationPlan::build(
        py,
        &state.data().options,
        fields.fields,
        has_post_init,
        explicit_hash,
    )?
    else {
        decline_owner(&state)?;
        return Ok(None);
    };
    if generation.hash_action == super::generation::HashAction::Error {
        decline_owner(&state)?;
        return Ok(None);
    }
    let Some(bindings) = super::nominal::PreparedBindings::prepare(
        auth,
        &state.data().fact,
        namespace,
        execution,
        construction_captures,
    )?
    else {
        decline_owner(&state)?;
        return Ok(None);
    };
    let mut members = Vec::new();
    for fragment in &generation.fragments {
        let present =
            unsafe { dictionary_value(namespace.as_ptr(), fragment.role.name()) }.is_some();
        if present && !fragment.unconditional {
            if fragment.overwrite != super::generation::Overwrite::Allowed {
                decline_owner(&state)?;
                return Ok(None);
            }
            continue;
        }
        let Some(_identity) = state
            .data()
            .fact
            .methods
            .iter()
            .find(|method| method.name == fragment.role.name() && method.implementation.is_none())
            .and_then(|method| method.generated.as_ref())
            .filter(|identity| {
                identity.class.definition == state.data().fact.identity
                    && identity.transform == soac_contracts::TransformKind::StdlibDataclass
            })
        else {
            decline_owner(&state)?;
            return Ok(None);
        };
        members.push(MemberPlan {
            name: fragment.role.name().to_owned(),
            kind: MemberKind::Generated(fragment.role),
        });
    }
    if unsafe { dictionary_value(namespace.as_ptr(), "__replace__") }.is_none() {
        if !state.data().fact.methods.iter().any(|method| {
            method.name == "__replace__"
                && method.implementation.is_none()
                && method.generated.as_ref().is_some_and(|generated| {
                    generated.class.definition == state.data().fact.identity
                        && generated.name == "__replace__"
                        && generated.transform == soac_contracts::TransformKind::StdlibDataclass
                })
        }) {
            decline_owner(&state)?;
            return Ok(None);
        }
        members.push(MemberPlan {
            name: "__replace__".to_owned(),
            kind: MemberKind::Shared(Helper::Replace),
        });
    }
    let slots_layout = if state.data().options.slots {
        let Some(layout) =
            super::slots::SlotsLayout::prepare(&state, &generation, namespace, bases)?
        else {
            decline_owner(&state)?;
            return Ok(None);
        };
        Some(layout)
    } else {
        None
    };
    let code = super::transcript::GeneratedCode::prepare(py, &generation)?;
    let produced = super::produced::GeneratedMethods::prepare(&state, &generation)?;
    let own_field_bindings = bindings.publish_own(&state)?;
    let fields_still_match = FieldProjection::capture(
        py,
        &state.data().catalog,
        &state,
        &state.data().fact,
        &state.data().options,
        namespace,
        bases,
    )?
    .is_some_and(|fields| fields.fields == generation.fields);
    if state.data().phase.get() != Phase::Prepared {
        let _ = fail_owner(&state);
        return Err(strict_runtime_unavailable(
            py,
            "dataclass construction was interrupted during preparation",
        ));
    }
    if !validate_catalog(&state)?
        || !super::protocol::prepared_decorator_matches(&state)?
        || !fields_still_match
        || !slots_layout
            .as_ref()
            .map(|layout| layout.matches_input(&state, namespace))
            .transpose()?
            .unwrap_or(true)
    {
        decline_owner(&state)?;
        return Ok(None);
    }
    let plan = Arc::new(ClassPlan::new(
        state.data().fact.clone(),
        auth.verified_module().type_facts().facts().source_digest,
        Arc::clone(execution),
        generation,
        members,
    ));
    state.data().plan.set(Arc::clone(&plan)).map_err(|_| {
        strict_runtime_unavailable(py, "dataclass construction was already prepared")
    })?;
    state
        .data()
        .code
        .set(code)
        .map_err(|_| strict_runtime_unavailable(py, "dataclass generation was already prepared"))?;
    state.data().produced.set(produced).map_err(|_| {
        strict_runtime_unavailable(py, "dataclass birth slots were already prepared")
    })?;
    if let Some(layout) = slots_layout {
        state.data().slots_layout.set(layout).map_err(|_| {
            strict_runtime_unavailable(py, "dataclass slots projection was already prepared")
        })?;
    }
    // The class adopts these exact minimal declaring-field owners before
    // Ready. This temporary invocation releases its copies at Apply completion.
    state
        .data()
        .own_field_bindings
        .set(own_field_bindings)
        .map_err(|_| {
            strict_runtime_unavailable(py, "dataclass field bindings were already prepared")
        })?;
    state.data().phase.set(Phase::Construction);
    Ok(Some(DataclassConstruction {
        plan,
        invocation_owner: owner.clone(),
    }))
}

fn inherited_attribute(bases: &Bound<'_, PyTuple>, name: &str) -> bool {
    for base in bases.iter() {
        if unsafe { ffi::PyType_Check(base.as_ptr()) } == 0 {
            continue;
        }
        let mro = unsafe { (*base.as_ptr().cast::<ffi::PyTypeObject>()).tp_mro };
        if mro.is_null() || unsafe { ffi::PyTuple_CheckExact(mro) } == 0 {
            continue;
        }
        for index in 0..unsafe { ffi::PyTuple_Size(mro) } {
            let class = unsafe { ffi::PyTuple_GetItem(mro, index).cast::<ffi::PyTypeObject>() };
            if unsafe { dictionary_value((*class).tp_dict, name) }.is_some() {
                return true;
            }
        }
    }
    false
}

pub(super) fn matches_construction_owner(
    owner: &Bound<'_, PyAny>,
    plan: &Arc<ClassPlan>,
) -> PyResult<bool> {
    let state = Owner::from_owner(owner.clone())?;
    Ok(state.data().phase.get() == Phase::Construction
        && state
            .data()
            .plan
            .get()
            .is_some_and(|actual| Arc::ptr_eq(actual, plan))
        && validate_catalog(&state)?)
}

pub(super) fn matches_bound_owner(
    owner: &Bound<'_, PyAny>,
    plan: &Arc<ClassPlan>,
) -> PyResult<bool> {
    let state = Owner::from_owner(owner.clone())?;
    Ok(state.data().phase.get() == Phase::Bound
        && plan.phase.get() == ClassPhase::Bound
        && plan.replacement_of.is_none()
        && state.data().replacement.get().is_none()
        && state
            .data()
            .plan
            .get()
            .is_some_and(|actual| Arc::ptr_eq(actual, plan))
        && !native_invocation(&state)?.is_none()
        && validate_catalog(&state)?)
}

pub(super) fn own_field_bindings<'py>(
    owner: &Bound<'py, PyAny>,
    plan: &Arc<ClassPlan>,
) -> PyResult<Vec<StrictFieldBinding<'py>>> {
    if !matches_construction_owner(owner, plan)? {
        return Err(strict_runtime_unavailable(
            owner.py(),
            "dataclass field bindings have no live construction proof",
        ));
    }
    let state = Owner::from_owner(owner.clone())?;
    let indices = state.data().own_field_bindings.get().ok_or_else(|| {
        strict_runtime_unavailable(owner.py(), "dataclass field bindings were not prepared")
    })?;
    indices
        .iter()
        .map(|&index| StrictFieldBinding::from_owner(state.reference(index)?))
        .collect()
}

pub(super) fn bind_class<'py>(
    owner: &Bound<'py, PyAny>,
    plan: &Arc<ClassPlan>,
    class: &Bound<'py, PyAny>,
    class_owner: &Bound<'py, PyAny>,
) -> PyResult<()> {
    let py = class.py();
    let state = Owner::from_owner(owner.clone())?;
    if !matches_construction_owner(owner, plan)? || plan.phase.get() != ClassPhase::Prepared {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass class binding has no live construction proof",
        ));
    }
    native::status(py, unsafe {
        native::PySoac_DataclassBindClass(
            native_invocation(&state)?.as_ptr(),
            class.as_ptr(),
            class_owner.as_ptr(),
        )
    })?;
    plan.actual_class.set(class.as_ptr() as usize);
    plan.actual_owner.set(class_owner.as_ptr() as usize);
    plan.phase.set(ClassPhase::Bound);
    state.data().phase.set(Phase::Bound);
    Ok(())
}

pub(crate) fn begin_apply(
    owner: &Bound<'_, PyAny>,
    decorator: &Bound<'_, PyAny>,
    class: &Bound<'_, PyAny>,
    before_transform: impl FnOnce() -> PyResult<()>,
) -> PyResult<bool> {
    let py = class.py();
    let state = Owner::from_owner(owner.clone())?;
    if state.data().phase.get() == Phase::Declined {
        return Ok(false);
    }
    let Some(plan) = state.data().plan.get() else {
        decline_owner(&state)?;
        return Ok(false);
    };
    let valid = || -> PyResult<bool> {
        Ok(state.data().phase.get() == Phase::Bound
            && plan.phase.get() == ClassPhase::Bound
            && plan.actual_class.get() == class.as_ptr() as usize
            && validate_catalog(&state)?
            && super::protocol::matches_decorator(&state, decorator)?)
    };
    if !valid()? {
        fail_owner(&state)?;
        return Err(strict_runtime_unavailable(
            py,
            "dataclass application has a changed bound graph",
        ));
    }
    // The original type is supported by the real CALL operand here. This hook
    // may snapshot lexical targets/adopt descriptors, never freeze source
    // functions or bind own/self leaves to the provisional original.
    if let Err(error) = before_transform().and_then(|()| {
        valid().and_then(|valid| {
            if valid {
                Ok(())
            } else {
                Err(strict_runtime_unavailable(
                    py,
                    "dataclass graph changed before native application",
                ))
            }
        })
    }) {
        let _ = fail_owner(&state);
        return Err(error);
    }
    state.data().root_entered.set(false);
    state.data().phase.set(Phase::Applying);
    Ok(true)
}

pub(crate) fn finish_apply(owner: &Bound<'_, PyAny>, result: &Bound<'_, PyAny>) -> PyResult<()> {
    let state = Owner::from_owner(owner.clone())?;
    let matched = state.data().phase.get() == Phase::Applying
        && state.data().root_entered.get()
        && if state.data().options.slots {
            super::slots::matches_result(&state, result)?
        } else {
            state
                .data()
                .plan
                .get()
                .is_some_and(|plan| plan.actual_class.get() == result.as_ptr() as usize)
        };
    if !matched {
        fail_owner(&state)?;
        return Err(strict_runtime_unavailable(
            owner.py(),
            "dataclass returned a class without its actual construction association",
        ));
    }
    if let Some(replacement) = state.data().replacement.get() {
        replacement.apply_returned.set(true);
    }
    Ok(())
}

pub(crate) fn apply<'py>(
    owner: &Bound<'py, PyAny>,
    decorator: &Bound<'py, PyAny>,
    class: &Bound<'py, PyAny>,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    if !begin_apply(owner, decorator, class, || {
        let state = crate::strict_class_state::for_constructed_type(class.py(), class)?
            .ok_or_else(|| {
                strict_runtime_unavailable(
                    class.py(),
                    "retained dataclass Apply lost its actual pending original",
                )
            })?;
        if state.is_interpreter_construction() || !state.is_pending_type() {
            return Err(strict_runtime_unavailable(
                class.py(),
                "retained dataclass Apply has the wrong construction phase",
            ));
        }
        crate::strict_class::snapshot_dataclass_source_members(class.py(), class, &state)
    })? {
        return Ok(None);
    }
    let py = class.py();
    let state = Owner::from_owner(owner.clone())?;
    let args = [class.as_ptr()];
    let result = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            native::PySoac_DataclassVectorcall(
                native_invocation(&state)?.as_ptr(),
                native::ROOT_APPLY,
                decorator.as_ptr(),
                args.as_ptr(),
                1,
                ptr::null_mut(),
            ),
        )
    };
    match result {
        Ok(value) => {
            finish_apply(owner, &value)?;
            Ok(Some(value))
        }
        Err(error) => {
            let _ = fail_owner(&state);
            Err(error)
        }
    }
}

pub(super) fn validate_completed_members<'py>(
    owner: &Bound<'py, PyAny>,
    plan: &Arc<ClassPlan>,
    class: &Bound<'py, PyAny>,
    namespace: &Bound<'py, PyDict>,
) -> PyResult<()> {
    let state = Owner::from_owner(owner.clone())?;
    if plan.replacement_of.is_some() {
        return super::protocol::require(
            &state,
            super::slots::validate_completed(&state, plan, class, namespace)?,
            "dataclass replacement completion does not match its copied members",
        );
    }
    if state.data().phase.get() != Phase::Applying
        || !state
            .data()
            .plan
            .get()
            .is_some_and(|actual| Arc::ptr_eq(actual, plan))
        || plan.actual_class.get() != class.as_ptr() as usize
        || !super::protocol::validate_completed(&state, class, namespace)?
    {
        return Err(strict_runtime_unavailable(
            class.py(),
            "dataclass completion does not match its generated members",
        ));
    }
    Ok(())
}

/// Only changes native/Rust terminal phases. The caller publishes every
/// prepared permanent class edge before releasing the active graph.
pub(super) fn commit(owner: &Bound<'_, PyAny>, plan: &Arc<ClassPlan>) -> PyResult<()> {
    let state = Owner::from_owner(owner.clone())?;
    if state.data().phase.get() != Phase::Applying
        || !state
            .data()
            .plan
            .get()
            .is_some_and(|actual| Arc::ptr_eq(actual, plan))
    {
        return Err(strict_runtime_unavailable(
            owner.py(),
            "dataclass completion was replayed",
        ));
    }
    let invocation = native_invocation(&state)?;
    let replacement = state.data().replacement.get();
    if replacement.is_some_and(|replacement| {
        replacement.plan.phase.get() != ClassPhase::Bound || !replacement.apply_returned.get()
    }) {
        return Err(strict_runtime_unavailable(
            owner.py(),
            "slots pair is not ready to complete",
        ));
    }
    state.data().phase.set(Phase::Completing);
    plan.phase.set(ClassPhase::Completing);
    if let Some(replacement) = replacement {
        replacement.plan.phase.set(ClassPhase::Completing);
    }
    if let Err(error) = native::status(owner.py(), unsafe {
        native::PySoac_CompleteDataclassInvocation(invocation.as_ptr())
    }) {
        let _ = fail_owner(&state);
        return Err(error);
    }
    if state.data().phase.get() != Phase::Completing
        || plan.phase.get() != ClassPhase::Completing
        || replacement
            .is_some_and(|replacement| replacement.plan.phase.get() != ClassPhase::Completing)
    {
        let _ = fail_owner(&state);
        return Err(strict_runtime_unavailable(
            owner.py(),
            "dataclass completion was interrupted",
        ));
    }
    plan.phase.set(ClassPhase::Complete);
    if let Some(replacement) = replacement {
        replacement.plan.phase.set(ClassPhase::Complete);
    }
    state.data().phase.set(Phase::Complete);
    Ok(())
}

pub(super) fn finish_publication(owner: &Bound<'_, PyAny>) -> PyResult<()> {
    clear_active(&Owner::from_owner(owner.clone())?, false)
}

pub(crate) fn decline(owner: &Bound<'_, PyAny>) -> PyResult<()> {
    decline_owner(&Owner::from_owner(owner.clone())?)
}

fn decline_owner(owner: &Owner<'_>) -> PyResult<()> {
    if owner.data().phase.get() == Phase::Declined {
        return Ok(());
    }
    if !matches!(
        owner.data().phase.get(),
        Phase::Preparing | Phase::Prepared | Phase::Construction
    ) {
        return Err(strict_runtime_unavailable(
            owner.owner().py(),
            "an installed dataclass contract cannot decline",
        ));
    }
    native::status(owner.owner().py(), unsafe {
        native::PySoac_DeclineDataclassInvocation(native_invocation(owner)?.as_ptr())
    })?;
    owner.data().phase.set(Phase::Declined);
    clear_active(owner, false)
}

pub(crate) fn discard(owner: &Bound<'_, PyAny>) -> PyResult<()> {
    let state = Owner::from_owner(owner.clone())?;
    match state.data().phase.get() {
        Phase::Complete | Phase::Declined => Ok(()),
        Phase::Preparing | Phase::Prepared | Phase::Construction => decline_owner(&state),
        _ => {
            let result = fail_owner(&state);
            result.and(clear_active(&state, false))
        }
    }
}

pub(super) fn fail_owner(owner: &Owner<'_>) -> PyResult<()> {
    if matches!(owner.data().phase.get(), Phase::Complete | Phase::Declined) {
        return Ok(());
    }
    owner.data().phase.set(Phase::Failed);
    if let Some(plan) = owner.data().plan.get() {
        plan.fail();
    }
    if let Some(replacement) = owner.data().replacement.get() {
        replacement.plan.fail();
    }
    let invocation = owner.reference(owner.data().invocation)?;
    let failed = if !invocation.is_none() {
        native::status(owner.owner().py(), unsafe {
            native::PySoac_FailDataclassInvocation(invocation.as_ptr())
        })
    } else {
        Ok(())
    };
    // Keep only the replacement's callback-free weak type witness until the
    // carrier removes that exact failed pending record. No semantic object
    // is kept alive by this cleanup-only edge.
    let cleared = clear_active(owner, true);
    failed.and(cleared)
}

fn clear_active(owner: &Owner<'_>, preserve_failed_replacement: bool) -> PyResult<()> {
    // The terminal/declined/completed phase is visible before any decref can
    // reenter. Native completion also dropped its class/builder/catalog edges.
    // Catalog weakrefs themselves are cleared, so an escaped creation record
    // cannot turn this transient owner into a lasting module/class root.
    let py = owner.owner().py();
    let retained = preserve_failed_replacement
        .then(|| {
            owner
                .data()
                .replacement
                .get()
                .and_then(|replacement| replacement.replacement_weak.get())
        })
        .flatten();
    for index in 0..owner.data().active_reference_count.get() {
        if retained == Some(index) {
            continue;
        }
        owner.set_reference(index, py.None().into_bound(py))?;
    }
    Ok(())
}
