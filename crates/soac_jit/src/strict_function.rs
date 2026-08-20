//! Per-execution strict function provenance and authenticated public entries.
//!
//! Construction attaches provenance, not a finalized callable capability.
//! Finalization is a separate post-decoration/adoption operation. Python-
//! supplied function IDs cannot invoke the authenticated construction path.

use std::cell::{Cell, OnceCell, RefCell, UnsafeCell};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, c_int, c_void};
use std::num::NonZeroU64;
use std::ptr::{self, NonNull};
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use soac_contracts::{DecoratorKind, NominalBindingFact, SourceIdentity, UncertaintyReason};
use soac_core::block_py::{
    BlockPyFunction, CallableSourceOrigin, CallableSourceRole, RuntimeFunctionId,
    StrictModuleSource,
};
use soac_ir_blockpy::BlockPyModuleShape;

use crate::module_type::SharedModuleState;
use crate::strict_interpreter::InterpreterInvocationIdentity;
use crate::strict_interpreter_source::{InterpreterCodeRole, StrictInterpreterSource};
use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{FunctionInstantiationTemplate, strict_runtime_unavailable};

mod class_construction;
mod interpreter;
pub(crate) use interpreter::{
    authenticate_captured_interpreter_owner, authenticate_interpreter_entry,
    prepare_interpreter_function_owner, record_native_function_attribute,
};
mod private_lexical;
pub(crate) use class_construction::{
    DISCARD_CLASS_CONSTRUCTION_CAPTURES_SYMBOL, discard_class_construction_captures,
    prepare_class_construction_captures, take_class_construction_captures,
};
pub(crate) use private_lexical::{
    prepare_private_lexical_captures, required_leaf as required_lexical_field_leaf,
};

const CODE: usize = 0;
const GLOBALS: usize = 1;
const DEFAULTS: usize = 2;
const KEYWORD_DEFAULTS: usize = 3;
const CLOSURE: usize = 4;
const MODULE_POLICY: usize = 5;
const ANNOTATION_PROVIDER_WITNESS: usize = 6;
pub(crate) const COMPLETE_FUNCTION_DEFINITION_SYMBOL: &str =
    "soac_jit_complete_function_definition";

unsafe extern "C" {
    fn PyFunction_SetSoacStrictOwner(
        function: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
    ) -> c_int;
    fn PyFunction_GetSoacStrictOwner(function: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyFunction_HasSoacDataclassCreation(function: *mut ffi::PyObject) -> c_int;
    fn PyFunction_CheckSoacStrictDefaults(function: *mut ffi::PyObject) -> c_int;
    fn PyFunction_SealSoacStrict(function: *mut ffi::PyObject, identity: u64) -> c_int;
    fn PyFunction_GetSoacStrictId(function: *mut ffi::PyObject) -> u64;
    fn PyCode_GetSoacStrictSourceId(code: *mut ffi::PyObject) -> u64;
    fn PyCode_GetVarnames(code: *mut ffi::PyCodeObject) -> *mut ffi::PyObject;
}

pub(crate) struct StrictFunctionData {
    source: Option<CallableSourceOrigin>,
    verified: Arc<crate::VerifiedStrictModule>,
    execution: crate::StrictModuleExecutionRef,
    function_identity: usize,
    implementation: StrictFunctionImplementation,
    references: FunctionMetadataReferences,
    finalized: Cell<bool>,
    // Body/default metadata can be sealed at class admission before the
    // module's bindings become final. Only unbound module leaves remain
    // entry-snapshotted until that same execution's one global-sealing step.
    // This flag adds no function/type/module primary and never permits a
    // previously bound target or sealed metadata to be replaced.
    capability_globals_pending: Cell<bool>,
    failed_pending: Cell<bool>,
    eligible: bool,
    call_counters: crate::strict_call::StrictCallCounters,
    capability_nominals: Vec<NominalBindingFact>,
    nominal_bindings: RefCell<BTreeMap<NominalBindingFact, usize>>,
    creation_execution: Option<Arc<crate::strict_namespace::NamespaceExecution>>,
}

struct FunctionMetadataReferences {
    module_policy: usize,
    annotation_provider: usize,
    self_weak: Option<usize>,
    // Class-owned functions reserve this existing GC-vector edge on both backends.
    // The callback-free weakref never keeps the actual class or namespace alive.
    class_weak: Option<usize>,
}

struct SoacFunctionImplementation {
    module_source: StrictModuleSource,
    function_id: RuntimeFunctionId,
    template_identity: usize,
    shared_state_identity: usize,
    class_captures: RefCell<class_construction::CaptureState>,
    private_captures: RefCell<private_lexical::PrivateCaptureState>,
}

struct InterpreterFunctionImplementation {
    source: Arc<StrictInterpreterSource>,
    native_code_ordinal: u32,
    original_code_identity: usize,
    globals_identity: usize,
    builtins_identity: usize,
    birth_execution: Arc<InterpreterInvocationIdentity>,
    frozen_metadata: OnceCell<NativeFrozenFunctionIdentity>,
    original_code_entered: Cell<bool>,
}

enum StrictFunctionImplementation {
    Soac(SoacFunctionImplementation),
    Cpython(InterpreterFunctionImplementation),
}

/// A native seal is not a compiler/runtime function ID. Only the actual native
/// owner/source execution gives this nonzero scalar a meaning.
#[derive(Clone, Copy, PartialEq, Eq)]
struct NativeFunctionSealIdentity(NonZeroU64);

#[derive(Clone, Copy, PartialEq, Eq)]
struct NativeFrozenFunctionIdentity {
    code: usize,
    defaults: usize,
    keyword_defaults: usize,
    closure: usize,
    ordinary_replacement: bool,
    seal_identity: NativeFunctionSealIdentity,
}

// SAFETY: Python metadata edges live only in StrictStateRef's GC vector.
// The Cpython arm stores Rust-only source/invocation data and scalar identities;
// ordinary native frames/functions keep every code/map/default/closure primary.
unsafe impl StrictStateData for StrictFunctionData {
    const TYPE_NAME: &'static CStr = c"soac._StrictFunctionOwner";
}

impl StrictFunctionData {
    pub(crate) fn original_code_entered(&self) -> Option<bool> {
        match &self.implementation {
            StrictFunctionImplementation::Cpython(native) => {
                Some(native.original_code_entered.get())
            }
            StrictFunctionImplementation::Soac(_) => None,
        }
    }

    /// Scalar-only, called by ROOT's inspect_live after captured frame/owner
    /// authentication. It does not inspect mutable idle function metadata.
    pub(crate) fn mark_original_code_entered(&self) -> bool {
        if self.failed_pending.get() {
            return false;
        }
        match &self.implementation {
            StrictFunctionImplementation::Cpython(native) => {
                native.original_code_entered.set(true);
                true
            }
            StrictFunctionImplementation::Soac(_) => false,
        }
    }

    pub(crate) fn mark_failed_pending(&self) -> bool {
        if self.finalized.get() {
            return false; // An already published contract is never revoked.
        }
        if matches!(
            &self.implementation,
            StrictFunctionImplementation::Cpython(_)
        ) {
            self.failed_pending.set(true);
            true
        } else {
            false
        }
    }
}

/// Preserve the existing SOAC temporary-owned view, while native callbacks
/// borrow their actual C/frame operand. The same wrapper lets a native globals
/// view avoid a map pin without changing the old SOAC owned-global behavior.
pub(crate) enum SupportedOperand<'operand, 'py, T = PyAny> {
    Owned(Bound<'py, T>),
    Borrowed(Borrowed<'operand, 'py, T>),
}

impl<'py, T> std::ops::Deref for SupportedOperand<'_, 'py, T> {
    type Target = Bound<'py, T>;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(value) => value,
            Self::Borrowed(value) => value,
        }
    }
}

pub(crate) struct AuthenticatedStrictFunction<'operand, 'py> {
    owner: StrictStateRef<'py, StrictFunctionData>,
    _function: SupportedOperand<'operand, 'py>,
    implementation: AuthenticatedImplementation,
}

enum AuthenticatedImplementation {
    Soac(Arc<SharedModuleState>),
    Cpython { source_authority: bool },
}

/// An ephemeral construction-only view of original lexical cells.
///
/// Only this module may mint a view, after consuming the exact fresh helper's
/// private carrier and validating its paired namespace function/native birth.
/// The full signed leaf is the lookup key; names or equal class sources never
/// authorize substitution. Cells remain mutable: field binding samples their
/// contents only after __prepare__ and the namespace body have completed.
/// No cell, helper, or namespace edge is retained by a finalized field binding.
pub(crate) struct ClassConstructionCaptures<'py> {
    py: Python<'py>,
    interpreter: i64,
    cells: BTreeMap<NominalBindingFact, Bound<'py, PyAny>>,
    namespace: Bound<'py, PyAny>,
    namespace_owner: usize,
    namespace_cells_taken: Cell<bool>,
}

impl<'py> ClassConstructionCaptures<'py> {
    /// Transfer the exact namespace's private projection once. The view and
    /// handle are ephemeral; neither binds or reads the mutable cell contents.
    pub(crate) fn take_namespace_cells(
        &self,
        auth: &AuthenticatedStrictFunction<'_, 'py>,
    ) -> PyResult<Vec<Bound<'py, PyAny>>> {
        if self.namespace_cells_taken.replace(true)
            || auth.function().as_ptr() != self.namespace.as_ptr()
            || auth.owner().as_ptr() as usize != self.namespace_owner
        {
            return Err(strict_runtime_unavailable(
                self.py,
                "namespace private cells were replayed or transferred",
            ));
        }
        let expected = auth.namespace_private_cell_count()?;
        let function = auth
            .module_state()?
            .lookup_function(auth.function_id()?)
            .ok_or_else(|| {
                strict_runtime_unavailable(self.py, "namespace private projection has no template")
            })?;
        let Some(scope) = &function.scope.private_lexical else {
            return Ok(Vec::new());
        };
        let facts = auth.verified_module().type_facts().facts();
        let mut result = Vec::with_capacity(expected);
        for capture in scope.private_captures() {
            let mut selected: Option<Bound<'py, PyAny>> = None;
            for &index in &capture.nominal_binding_indices {
                let leaf = facts
                    .nominal_bindings
                    .get(index as usize)
                    .filter(|leaf| {
                        leaf.binding_scope == capture.binding.scope
                            && leaf.name == capture.binding.name
                    })
                    .ok_or_else(|| {
                        strict_runtime_unavailable(
                            self.py,
                            "namespace private projection lost its signed leaf",
                        )
                    })?;
                let cell = self.cell_for(leaf)?.ok_or_else(|| {
                    strict_runtime_unavailable(
                        self.py,
                        "namespace private cell was not supplied by its constructor",
                    )
                })?;
                if selected
                    .as_ref()
                    .is_some_and(|previous| previous.as_ptr() != cell.as_ptr())
                {
                    return Err(strict_runtime_unavailable(
                        self.py,
                        "one namespace lexical binding has different cell identities",
                    ));
                }
                selected = Some(cell);
            }
            result.push(selected.ok_or_else(|| {
                strict_runtime_unavailable(self.py, "namespace private cell has no signed consumer")
            })?);
        }
        Ok(result)
    }

    pub(crate) fn cell_for(
        &self,
        leaf: &NominalBindingFact,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        unsafe extern "C" {
            static mut PyCell_Type: ffi::PyTypeObject;
        }

        let interpreter = unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
        if interpreter < 0 {
            return Err(PyErr::fetch(self.py));
        }
        if interpreter != self.interpreter {
            return Err(strict_runtime_unavailable(
                self.py,
                "class construction captures belong to another interpreter",
            ));
        }
        let Some(cell) = self.cells.get(leaf) else {
            return Ok(None);
        };
        if unsafe { ffi::Py_TYPE(cell.as_ptr()) } != ptr::addr_of_mut!(PyCell_Type) {
            return Err(strict_runtime_unavailable(
                self.py,
                "class construction capture is not its original native cell",
            ));
        }
        Ok(Some(cell.clone()))
    }
}

impl<'py> std::ops::Deref for AuthenticatedStrictFunction<'_, 'py> {
    type Target = StrictStateRef<'py, StrictFunctionData>;
    fn deref(&self) -> &Self::Target {
        &self.owner
    }
}

impl<'py> AuthenticatedStrictFunction<'_, 'py> {
    pub(crate) fn origin(&self) -> Option<&CallableSourceOrigin> {
        if matches!(
            &self.implementation,
            AuthenticatedImplementation::Cpython {
                source_authority: false
            }
        ) {
            return None;
        }
        self.source()
    }
    pub(crate) fn can_finalize(&self) -> bool {
        self.data().eligible
    }
    pub(crate) fn capability_nominal_bindings(&self) -> &[NominalBindingFact] {
        &self.data().capability_nominals
    }
    pub(crate) fn bound_nominal_target(
        &self,
        binding: &NominalBindingFact,
    ) -> PyResult<Option<Bound<'_, PyAny>>> {
        let index = self.data().nominal_bindings.borrow().get(binding).copied();
        let Some(index) = index else {
            return Ok(None);
        };
        let value = self.reference(index)?;
        // Pre-Ready construction reserves a None edge. It is not a resolved
        // type until the exact native class binding callback fills it.
        Ok((!value.is_none()).then_some(value))
    }
    pub(crate) fn has_nominal_reservation(&self, binding: &NominalBindingFact) -> bool {
        self.data().nominal_bindings.borrow().contains_key(binding)
    }

    pub(crate) fn function(&self) -> &Bound<'py, PyAny> {
        &self._function
    }

    pub(crate) fn owner_ref(&self) -> &StrictStateRef<'py, StrictFunctionData> {
        &self.owner
    }

    pub(crate) fn is_interpreter(&self) -> bool {
        matches!(
            &self.implementation,
            AuthenticatedImplementation::Cpython { .. }
        )
    }

    pub(crate) fn interpreter_source_authority(&self) -> PyResult<bool> {
        match self.implementation {
            AuthenticatedImplementation::Cpython { source_authority } => Ok(source_authority),
            AuthenticatedImplementation::Soac(_) => Err(strict_runtime_unavailable(
                self.function().py(),
                "compiled function has no native interpreter entry identity",
            )),
        }
    }

    pub(crate) fn same_source_execution(
        &self,
        other: &AuthenticatedStrictFunction<'_, '_>,
    ) -> bool {
        Arc::ptr_eq(self.verified_module(), other.verified_module())
            && self.execution_ref().same_execution(other.execution_ref())
    }

    pub(crate) fn namespace_private_cell_count(&self) -> PyResult<usize> {
        let py = self.function().py();
        let shared = self.module_state()?;
        let function = shared
            .lookup_function(self.function_id()?)
            .filter(|function| shared.admits_function(*function))
            .ok_or_else(|| {
                strict_runtime_unavailable(
                    py,
                    "namespace private cells have no authenticated template",
                )
            })?;
        if self
            .origin()
            .is_none_or(|origin| origin.role != CallableSourceRole::ClassNamespace)
        {
            return Err(strict_runtime_unavailable(
                py,
                "private namespace count requested for another source role",
            ));
        }
        let Some(scope) = &function.scope.private_lexical else {
            return Ok(0);
        };
        if scope
            .captures
            .iter()
            .any(|capture| capture.native_closure.is_some())
        {
            return Err(strict_runtime_unavailable(
                py,
                "namespace private cells cannot use public helper closure metadata",
            ));
        }
        Ok(scope.private_captures().count())
    }

    /// # Safety
    /// Native only; no callback/target mutation until every returned borrowed
    /// provider/closure view is discarded.
    pub(crate) unsafe fn borrowed_native_annotation_provider(
        &self,
    ) -> PyResult<Option<AuthenticatedStrictFunction<'_, 'py>>> {
        unsafe { interpreter::borrowed_annotation_provider(self) }
    }

    /// The actual original provider operand, not another invocation's provider
    /// with equal source and globals. This creates only temporary owning views.
    pub(crate) fn owned_annotation_provider(
        &self,
    ) -> PyResult<Option<AuthenticatedStrictFunction<'static, 'py>>> {
        let py = self.owner().py();
        if self.is_interpreter() {
            return Err(strict_runtime_unavailable(
                py,
                "native provider requires a callback-free borrowed view",
            ));
        }
        let witness = self.reference(self.data().references.annotation_provider)?;
        if witness.is_none() {
            return Ok(None);
        }
        let mut provider = ptr::null_mut();
        if unsafe { crate::PyWeakref_GetRef(witness.as_ptr(), &mut provider) } < 0 {
            return Err(PyErr::fetch(py));
        }
        if provider.is_null() {
            return Ok(None);
        }
        let provider = unsafe { Bound::<PyAny>::from_owned_ptr(py, provider) };
        let current =
            unsafe { (*self._function.as_ptr().cast::<ffi::PyFunctionObject>()).func_annotate };
        if provider.as_ptr() != current {
            return Ok(None);
        }
        let auth = authenticate_strict_function(py, &provider)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "owned annotation provider lost its native owner")
        })?;
        let same_creation = match (auth.creation_execution(), self.creation_execution()) {
            (None, None) => true,
            (Some(provider), Some(function)) => Arc::ptr_eq(provider, function),
            _ => false,
        };
        if !same_creation
            || !auth.same_source_execution(self)
            || !auth.origin().is_some_and(|origin| {
                origin.role == CallableSourceRole::AnnotationProvider
                    && self
                        .origin()
                        .is_some_and(|target| target.definition == origin.definition)
            })
            || auth.globals()?.as_ptr() != self.globals()?.as_ptr()
        {
            return Err(strict_runtime_unavailable(
                py,
                "owned annotation provider changed its execution identity",
            ));
        }
        Ok(Some(auth))
    }
    pub(crate) fn verified_module(&self) -> &Arc<crate::VerifiedStrictModule> {
        &self.data().verified
    }
    /// Only the compiled arm retains the existing shared implementation view.
    /// Do not move a SharedModuleState Arc into an opaque GC payload: its
    /// existing Python edges belong to the ordinary runtime ownership graph.
    pub(crate) fn module_state(&self) -> PyResult<&Arc<SharedModuleState>> {
        match &self.implementation {
            AuthenticatedImplementation::Soac(shared) => Ok(shared),
            AuthenticatedImplementation::Cpython { .. } => Err(strict_runtime_unavailable(
                self.function().py(),
                "native interpreter function has no compiled module state",
            )),
        }
    }
    pub(crate) fn globals(&self) -> PyResult<SupportedOperand<'_, 'py, PyDict>> {
        if self.is_interpreter() {
            // Native func_globals is immutable and supported by the actual
            // function operand. Never dereference a cached scalar map address.
            let pointer =
                unsafe { (*self.function().as_ptr().cast::<ffi::PyFunctionObject>()).func_globals };
            if pointer.is_null() || unsafe { ffi::PyDict_CheckExact(pointer) } == 0 {
                return Err(strict_runtime_unavailable(
                    self.function().py(),
                    "native function globals are not a live exact dictionary",
                ));
            }
            Ok(SupportedOperand::Borrowed(unsafe {
                Borrowed::from_ptr(self.function().py(), pointer).cast_unchecked()
            }))
        } else {
            self.global_dictionary().map(SupportedOperand::Owned)
        }
    }
    pub(crate) fn execution_ref(&self) -> &crate::StrictModuleExecutionRef {
        self.execution()
    }

    pub(crate) fn creation_execution(
        &self,
    ) -> Option<&Arc<crate::strict_namespace::NamespaceExecution>> {
        self.data().creation_execution.as_ref()
    }
}

impl<'py> StrictStateRef<'py, StrictFunctionData> {
    pub(crate) fn awaits_module_nominals(&self) -> bool {
        self.data().capability_globals_pending.get()
    }

    pub(crate) fn source(&self) -> Option<&CallableSourceOrigin> {
        self.data().source.as_ref()
    }
    fn soac_implementation(&self) -> PyResult<&SoacFunctionImplementation> {
        self.ensure_live()?;
        match &self.data().implementation {
            StrictFunctionImplementation::Soac(implementation) => Ok(implementation),
            StrictFunctionImplementation::Cpython(_) => Err(strict_runtime_unavailable(
                self.owner().py(),
                "native interpreter owner has no compiled implementation",
            )),
        }
    }
    fn native_implementation(&self) -> PyResult<&InterpreterFunctionImplementation> {
        self.ensure_live()?;
        match &self.data().implementation {
            StrictFunctionImplementation::Cpython(implementation) => Ok(implementation),
            StrictFunctionImplementation::Soac(_) => Err(strict_runtime_unavailable(
                self.owner().py(),
                "compiled owner has no native interpreter implementation",
            )),
        }
    }
    pub(crate) fn function_id(&self) -> PyResult<RuntimeFunctionId> {
        Ok(self.soac_implementation()?.function_id)
    }
    pub(crate) fn module_source(&self) -> PyResult<&StrictModuleSource> {
        Ok(&self.soac_implementation()?.module_source)
    }
    pub(crate) fn native_source(&self) -> PyResult<&Arc<StrictInterpreterSource>> {
        Ok(&self.native_implementation()?.source)
    }
    pub(crate) fn native_code_ordinal(&self) -> PyResult<u32> {
        Ok(self.native_implementation()?.native_code_ordinal)
    }
    pub(crate) fn native_birth_execution(&self) -> PyResult<&Arc<InterpreterInvocationIdentity>> {
        Ok(&self.native_implementation()?.birth_execution)
    }
    /// Comparison only; live source/code validation occurs at authenticated entry.
    pub(crate) fn is_original_interpreter_entry(
        &self,
        captured_code: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        Ok(self.native_implementation()?.original_code_identity == captured_code as usize)
    }
    pub(crate) fn class_weak_witness(&self) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.ensure_live()?;
        let Some(index) = self.data().references.class_weak else {
            return Ok(None);
        };
        let witness = self.reference(index)?;
        Ok((!witness.is_none()).then_some(witness))
    }
    pub(crate) fn is_failed_pending(&self) -> bool {
        self.data().failed_pending.get()
    }
    pub(crate) fn execution(&self) -> &crate::StrictModuleExecutionRef {
        &self.data().execution
    }
    pub(crate) fn global_dictionary(&self) -> PyResult<Bound<'py, PyDict>> {
        self.soac_implementation()?;
        self.reference(GLOBALS)?
            .cast_into::<PyDict>()
            .map_err(Into::into)
    }
    pub(crate) fn module_policy_owner(&self) -> PyResult<Bound<'py, PyAny>> {
        self.reference(self.data().references.module_policy)
    }
    pub(crate) fn is_finalized(&self) -> bool {
        self.data().finalized.get()
    }
    pub(crate) fn call_statistics(&self) -> crate::StrictFunctionCallStatistics {
        self.data().call_counters.snapshot()
    }
}

/// Actual nominal targets used only to publish independently guarded field or
/// method capabilities. No function annotation establishes a value invariant.
fn selected_capability_nominal_bindings(
    verified: &crate::VerifiedStrictModule,
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> Vec<NominalBindingFact> {
    let Some(origin) = function
        .scope
        .source_origin
        .as_ref()
        .filter(|origin| origin.role == CallableSourceRole::SourceFunction)
    else {
        return Vec::new();
    };
    if !eligible_source_function(verified, Some(origin)) {
        return Vec::new();
    }
    let facts = verified.type_facts().facts();
    let classes: BTreeSet<_> = crate::strict_optimization::field_sites(facts, function)
        .into_iter()
        .map(|site| site.receiver_class)
        .chain(
            crate::strict_optimization::method_sites(facts, function)
                .into_iter()
                .map(|site| site.receiver_class),
        )
        .collect();
    facts
        .nominal_bindings
        .iter()
        .filter(|binding| {
            binding
                .owner
                .as_function()
                .is_some_and(|(source, _)| source == &origin.definition)
                && classes.contains(&binding.class)
        })
        .cloned()
        .collect()
}

/// Source-policy eligibility only, not runtime authentication or permission to
/// adopt a function. Pending targets use this before inspecting mutable native
/// metadata so deliberately dynamic functions can remain ordinary.
pub(crate) fn eligible_function(
    shared: &SharedModuleState,
    origin: Option<&CallableSourceOrigin>,
) -> bool {
    shared
        .verified_strict_module()
        .is_some_and(|verified| eligible_source_function(verified, origin))
}

pub(crate) fn eligible_source_function(
    verified: &crate::VerifiedStrictModule,
    origin: Option<&CallableSourceOrigin>,
) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    if origin.role == CallableSourceRole::TypeParameterScope {
        return false;
    }
    if origin.role == CallableSourceRole::AnnotationProvider
        && matches!(
            origin.definition.definition_kind,
            soac_contracts::DefinitionKind::TypeAlias | soac_contracts::DefinitionKind::Parameter
        )
    {
        // Lazy type-expression provenance authorizes only the actual factory
        // and ordinary replay. It is not a promise of immutable evaluation.
        return false;
    }
    if origin.role != CallableSourceRole::SourceFunction {
        return true;
    }
    Some(verified)
        .and_then(|module| {
            if module
                .type_facts()
                .facts()
                .function_has_statically_dynamic_class_owner(&origin.definition)
            {
                return None;
            }
            module
                .type_facts()
                .facts()
                .functions
                .iter()
                .find(|function| function.identity == origin.definition)
        })
        .is_some_and(|function| {
            !function.uncertainty.iter().any(|reason| {
                matches!(
                    reason,
                    UncertaintyReason::IgnoredDiagnostic
                        | UncertaintyReason::DynamicDecorator
                        | UncertaintyReason::DynamicDescriptor
                )
            }) && function.decorators.iter().all(|decorator| {
                decorator.uncertainty.is_empty()
                    && !matches!(
                        decorator.kind,
                        DecoratorKind::Other
                            | DecoratorKind::Unknown
                            | DecoratorKind::DataclassTransform
                    )
            })
        })
}

fn owned_or_none(py: Python<'_>, value: *mut ffi::PyObject) -> Py<PyAny> {
    if value.is_null() {
        py.None()
    } else {
        unsafe { Bound::<PyAny>::from_borrowed_ptr(py, value) }.unbind()
    }
}

/// The compiler passes the actual provider it just installed, not a name to
/// resolve later. Keep only a callback-free weak witness: replacing the
/// provider before adoption must release its closure at the ordinary boundary.
fn initial_annotation_provider_witness(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    provider: &Bound<'_, PyAny>,
    shared: &Arc<SharedModuleState>,
    origin: Option<&CallableSourceOrigin>,
) -> PyResult<Py<PyAny>> {
    if provider.is_none() {
        return Ok(py.None());
    }
    let current = unsafe { (*function.as_ptr().cast::<ffi::PyFunctionObject>()).func_annotate };
    let expected = origin.ok_or_else(|| {
        strict_runtime_unavailable(py, "annotation provider has no compiler source owner")
    })?;
    if current != provider.as_ptr() {
        return Err(strict_runtime_unavailable(
            py,
            "compiler annotation provider was not installed on its function",
        ));
    }
    let auth = authenticate_strict_function(py, provider)?.ok_or_else(|| {
        strict_runtime_unavailable(
            py,
            "compiler annotation provider has no native source owner",
        )
    })?;
    if !Arc::ptr_eq(auth.module_state()?, shared)
        || auth.origin().is_none_or(|origin| {
            origin.role != CallableSourceRole::AnnotationProvider
                || origin.definition != expected.definition
        })
        || auth.globals()?.as_ptr()
            != unsafe { (*function.as_ptr().cast::<ffi::PyFunctionObject>()).func_globals }
    {
        return Err(strict_runtime_unavailable(
            py,
            "annotation provider belongs to a different source function execution",
        ));
    }
    unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            crate::PyWeakref_NewRef(provider.as_ptr(), ptr::null_mut()),
        )
    }
    .map(Bound::unbind)
}

/// Called only by the explicit compiler function-construction operation, after
/// ordinary code/default/annotation-provider installation and before exposure.
/// It intentionally does not seal: decorators have not run yet.
pub(crate) fn install_strict_function_owner(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    shared: &Arc<SharedModuleState>,
    template: &Arc<FunctionInstantiationTemplate>,
    annotation_provider: &Bound<'_, PyAny>,
    creation_execution: Option<&Arc<crate::strict_namespace::NamespaceExecution>>,
) -> PyResult<()> {
    let Some(verified) = shared.strict_module.as_ref() else {
        return Ok(());
    };
    if !shared
        .lowered_module
        .strict_source
        .as_ref()
        .is_some_and(|source| source.matches_verified(verified.type_facts()))
    {
        return Err(strict_runtime_unavailable(
            py,
            "strict function IR belongs to a different verified artifact",
        ));
    }
    let blockpy = template.function();
    let capability_nominals = selected_capability_nominal_bindings(verified, blockpy);
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    if unsafe { ffi::PyFunction_Check(function.as_ptr()) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "strict constructor did not create a native function",
        ));
    }
    let globals = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, (*raw).func_globals) }
        .cast_into::<PyDict>()?;
    let execution = shared.strict_execution.as_ref().ok_or_else(|| {
        strict_runtime_unavailable(py, "strict function has no explicit module execution owner")
    })?;
    let policy_owner = execution.acquire_owner(py, &globals, verified)?;
    if let Some(creation) = creation_execution {
        creation.validate_creation(py, shared, globals.as_any())?;
    }
    let code = unsafe { (*raw).func_code };
    if blockpy.scope.source_origin.as_ref().is_some_and(|origin| {
        matches!(
            origin.role,
            CallableSourceRole::SourceFunction
                | CallableSourceRole::AnnotationProvider
                | CallableSourceRole::TypeParameterScope
        )
    }) {
        if !shared
            .lookup_original_code(blockpy.function_id)
            .is_some_and(|original| original.as_ptr() == code)
            || unsafe { PyCode_GetSoacStrictSourceId(code) } == 0
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict source function requires its authenticated native code/capture layout",
            ));
        }
    }
    let provider_witness = initial_annotation_provider_witness(
        py,
        function,
        annotation_provider,
        shared,
        blockpy.scope.source_origin.as_ref(),
    )?;
    let mut references = vec![
        owned_or_none(py, code),
        globals.unbind().into_any(),
        py.None(),
        py.None(),
        py.None(),
        policy_owner,
        provider_witness,
    ];
    // Append only: the compiled CODE/GLOBALS/default/closure indices and
    // their existing primary ownership are unchanged.
    let class_weak = creation_execution.map(|_| {
        let index = references.len();
        references.push(py.None());
        index
    });
    let owner = StrictStateRef::new(
        py,
        StrictFunctionData {
            source: blockpy.scope.source_origin.clone(),
            verified: Arc::clone(verified),
            execution: execution.clone(),
            function_identity: function.as_ptr() as usize,
            implementation: StrictFunctionImplementation::Soac(SoacFunctionImplementation {
                module_source: shared.lowered_module.strict_source.clone().ok_or_else(|| {
                    strict_runtime_unavailable(py, "strict function has no source stamp")
                })?,
                function_id: blockpy.function_id,
                template_identity: Arc::as_ptr(template) as usize,
                shared_state_identity: Arc::as_ptr(shared) as usize,
                class_captures: RefCell::new(class_construction::CaptureState::initial(
                    blockpy.scope.class_construction.as_ref(),
                )),
                private_captures: RefCell::new(private_lexical::PrivateCaptureState::initial(
                    &blockpy.scope,
                )),
            }),
            references: FunctionMetadataReferences {
                module_policy: MODULE_POLICY,
                annotation_provider: ANNOTATION_PROVIDER_WITNESS,
                self_weak: None,
                class_weak,
            },
            finalized: Cell::new(false),
            capability_globals_pending: Cell::new(false),
            failed_pending: Cell::new(false),
            eligible: eligible_function(shared, blockpy.scope.source_origin.as_ref()),
            call_counters: crate::strict_call::StrictCallCounters::default(),
            capability_nominals,
            nominal_bindings: RefCell::new(BTreeMap::new()),
            creation_execution: creation_execution.cloned(),
        },
        references,
    )?;
    if unsafe { PyFunction_SetSoacStrictOwner(function.as_ptr(), owner.owner().as_ptr()) } < 0 {
        return Err(PyErr::fetch(py));
    }
    Ok(())
}

/// Validate actual method ownership for this admitted copied namespace.
/// Called by the native pre-Ready callback after all actual components match;
/// the successful path allocates no Python object and cannot invoke Python.
pub(crate) fn validate_class_function_owner(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    execution: &Arc<crate::strict_namespace::NamespaceExecution>,
) -> PyResult<()> {
    let owner =
        authenticate_borrowed_strict_function(py, function.as_borrowed())?.ok_or_else(|| {
            strict_runtime_unavailable(py, "admitted method has no actual function owner")
        })?;
    if !owner
        .creation_execution()
        .is_some_and(|creation| Arc::ptr_eq(creation, execution))
        || !owner.origin().is_some_and(|origin| {
            origin.role == CallableSourceRole::SourceFunction
                && owner
                    .verified_module()
                    .type_facts()
                    .facts()
                    .source_class_owner(&origin.definition)
                    .is_some_and(|class| class.identity == *execution.source())
        })
    {
        return Err(strict_runtime_unavailable(
            py,
            "admitted method belongs to a different class execution",
        ));
    }
    Ok(())
}

/// Bind the already reserved class-namespace witness. An alias of one
/// source method may reach this function twice, but it cannot bind another type.
/// There is no new Python allocation, target owner, or callback on success.
///
/// # Safety
/// The caller has just authenticated the actual pending type, native class
/// owner, copied namespace and execution. `witness` is its exact callback-free
/// weakref, created under the native pending barrier and revalidated afterward.
pub(crate) unsafe fn bind_class_weak_witness(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    execution: &Arc<crate::strict_namespace::NamespaceExecution>,
    witness: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let auth = authenticate_borrowed_strict_function(py, function.as_borrowed())?
        .ok_or_else(|| strict_runtime_unavailable(py, "class member lost its actual owner"))?;
    if (auth.is_interpreter() && !auth.interpreter_source_authority()?)
        || !auth
            .creation_execution()
            .is_some_and(|actual| Arc::ptr_eq(actual, execution))
    {
        return Err(strict_runtime_unavailable(
            py,
            "class witness belongs to another namespace execution",
        ));
    }
    let index = auth.data().references.class_weak.ok_or_else(|| {
        strict_runtime_unavailable(py, "class member did not reserve its namespace witness")
    })?;
    let previous = auth.reference(index)?;
    if !previous.is_none() {
        return if previous.as_ptr() == witness.as_ptr() {
            Ok(())
        } else {
            Err(strict_runtime_unavailable(
                py,
                "class member cannot change its original class witness",
            ))
        };
    }
    auth.bind_reserved_reference(index, witness.clone())
}

/// Select only an already published function's outstanding module stage.
/// This scalar inspection is not source/template authority: the weak-registry
/// caller must still fully authenticate the actual function before completion.
/// Unparticipating framework methods may have mutable code and remain skipped.
pub(crate) fn function_awaits_module_nominals(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
) -> PyResult<bool> {
    let owner = unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) };
    if owner.is_null() {
        return if unsafe { ffi::PyErr_Occurred() }.is_null() {
            Ok(false)
        } else {
            Err(PyErr::fetch(py))
        };
    }
    Ok(unsafe {
        StrictStateRef::<StrictFunctionData>::inspect_live(owner, |data| {
            data.finalized.get() && data.capability_globals_pending.get()
        })
    }
    .unwrap_or(false))
}

/// Preserve the existing caller-independent owned view after common native
/// owner, actual function/code/environment, source and implementation validation.
/// Native callbacks use the borrowed accessor below instead of adding a pin.
/// An ordinary function has no owner; a cleared or mismatched owner is an
/// error, never permission to treat a former strict function as ordinary.
pub(crate) fn authenticate_strict_function<'py>(
    py: Python<'py>,
    function: &Bound<'py, PyAny>,
) -> PyResult<Option<AuthenticatedStrictFunction<'static, 'py>>> {
    Ok(
        authenticate_borrowed_strict_function(py, function.as_borrowed())?.map(|auth| {
            AuthenticatedStrictFunction {
                owner: auth.owner,
                implementation: auth.implementation,
                _function: SupportedOperand::Owned(function.clone()),
            }
        }),
    )
}

/// Borrow the caller's actual operand; no function alias is acquired.
pub(crate) fn authenticate_borrowed_strict_function<'a, 'py>(
    py: Python<'py>,
    function: Borrowed<'a, 'py, PyAny>,
) -> PyResult<Option<AuthenticatedStrictFunction<'a, 'py>>> {
    let Some(owner) = authenticate_borrowed_function_owner(py, function)? else {
        return Ok(None);
    };
    if owner.is_interpreter() {
        return Ok(Some(owner));
    }
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    if owner.reference(CODE)?.as_ptr() != unsafe { (*raw).func_code }
        || (owner.data().finalized.get()
            && (!optional_reference_matches(&owner, DEFAULTS, unsafe { (*raw).func_defaults })?
                || !optional_reference_matches(&owner, KEYWORD_DEFAULTS, unsafe {
                    (*raw).func_kwdefaults
                })?
                || !optional_reference_matches(&owner, CLOSURE, unsafe { (*raw).func_closure })?))
    {
        return Err(strict_runtime_unavailable(
            py,
            "strict function native metadata changed",
        ));
    }
    Ok(Some(owner))
}

/// Admission can decline a never-activated method whose ordinary namespace
/// callback replaced its code. This is not source-execution authentication:
/// replacement calls still pass the existing checked trampoline's dedicated
/// ordinary-code authorization, and already committed contracts cannot decline.
pub(crate) fn authenticate_class_candidate_function<'py>(
    py: Python<'py>,
    function: &Bound<'py, PyAny>,
) -> PyResult<Option<AuthenticatedStrictFunction<'static, 'py>>> {
    let Some(owner) = authenticate_function_owner(py, function)? else {
        return Ok(None);
    };
    if owner.is_interpreter() {
        return Ok(owner.interpreter_source_authority()?.then_some(owner));
    }
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    if owner.reference(CODE)?.as_ptr() != unsafe { (*raw).func_code } {
        let metadata = unsafe { crate::py_function_jit_extra(function.as_ptr()) }
            .map_err(|()| PyErr::fetch(py))?;
        authorize_ordinary_replacement(py, function, metadata)?;
        return Ok(None);
    }
    authenticate_strict_function(py, function)
}

/// This private partial authentication does not authorize original source
/// execution, class construction, or source-based optimization. Only the full
/// accessor above publishes that view to other runtime components.
fn authenticate_function_owner<'py>(
    py: Python<'py>,
    function: &Bound<'py, PyAny>,
) -> PyResult<Option<AuthenticatedStrictFunction<'static, 'py>>> {
    Ok(
        authenticate_borrowed_function_owner(py, function.as_borrowed())?.map(|auth| {
            AuthenticatedStrictFunction {
                owner: auth.owner,
                implementation: auth.implementation,
                _function: SupportedOperand::Owned(function.clone()),
            }
        }),
    )
}

fn authenticate_borrowed_function_owner<'a, 'py>(
    py: Python<'py>,
    function: Borrowed<'a, 'py, PyAny>,
) -> PyResult<Option<AuthenticatedStrictFunction<'a, 'py>>> {
    if unsafe { ffi::PyFunction_Check(function.as_ptr()) } == 0 {
        return Ok(None);
    }
    let owner = unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) };
    if owner.is_null() {
        if !unsafe { ffi::PyErr_Occurred() }.is_null() {
            return Err(PyErr::fetch(py));
        }
        return Ok(None);
    }
    // A frozen keyword-default dictionary can be shared with a module whose
    // terminal teardown clears it. A new call must not consume that former
    // contract. Active/suspended calls use their captured owner and values;
    // this check deliberately does not live in the native owner getter.
    if unsafe { PyFunction_CheckSoacStrictDefaults(function.as_ptr()) } < 0 {
        return Err(PyErr::fetch(py));
    }
    match unsafe { PyFunction_HasSoacDataclassCreation(function.as_ptr()) } {
        // This exact native record proves generated creation only. It is not
        // a source-function owner, JIT template, checked-body proof, or permit
        // to execute original strict bytecode. Complete/declined records can
        // still describe an ordinary stock function with no such capability.
        1 => return Ok(None),
        0 => (),
        -1 => return Err(PyErr::fetch(py)),
        _ => {
            return Err(strict_runtime_unavailable(
                py,
                "invalid native dataclass owner status",
            ));
        }
    }
    let owner = StrictStateRef::<StrictFunctionData>::from_owner(unsafe {
        Bound::from_borrowed_ptr(py, owner)
    })?;
    if owner.data().function_identity != function.as_ptr() as usize || owner.is_failed_pending() {
        return Err(strict_runtime_unavailable(
            py,
            "strict function birth is foreign or terminal",
        ));
    }
    if matches!(
        &owner.data().implementation,
        StrictFunctionImplementation::Cpython(_)
    ) {
        let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
        let code = unsafe { Borrowed::from_ptr(py, (*raw).func_code) };
        let globals =
            unsafe { Borrowed::from_ptr(py, (*raw).func_globals).cast_unchecked::<PyDict>() };
        let builtins = unsafe { Borrowed::from_ptr(py, (*raw).func_builtins) };
        let source_authority =
            interpreter::validate_entry(py, &owner, function, code, globals, builtins)?;
        return Ok(Some(AuthenticatedStrictFunction {
            owner,
            _function: SupportedOperand::Borrowed(function),
            implementation: AuthenticatedImplementation::Cpython { source_authority },
        }));
    }
    let (function_id, shared, template) = {
        let metadata = unsafe { crate::py_function_jit_extra(function.as_ptr()) }
            .map_err(|()| PyErr::fetch(py))?;
        // Allocation-type ownership is not execution authority. Clone the
        // compiler owners before fallible source/owner authentication below.
        let metadata = unsafe { &*metadata };
        (
            metadata.function_id,
            Arc::clone(&metadata.module_state),
            Arc::clone(&metadata.function_template),
        )
    };
    if owner.function_id()? != function_id {
        return Err(strict_runtime_unavailable(
            py,
            "strict function compiler identity mismatch",
        ));
    }
    authenticate_expected_owner(py, &function, &owner, &shared, &template)?;
    Ok(Some(AuthenticatedStrictFunction {
        owner,
        _function: SupportedOperand::Borrowed(function),
        implementation: AuthenticatedImplementation::Soac(shared),
    }))
}

/// Validate against independently supplied compiler objects. Registration
/// cannot read JIT metadata that it has not installed yet; ordinary entry and
/// profiling supply those same objects from the function's existing metadata.
fn authenticate_expected_owner(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    owner: &StrictStateRef<'_, StrictFunctionData>,
    shared: &Arc<SharedModuleState>,
    template: &Arc<FunctionInstantiationTemplate>,
) -> PyResult<()> {
    owner.ensure_live()?;
    let data = owner.data();
    let implementation = owner.soac_implementation()?;
    let blockpy = template.function();
    if data.function_identity != function.as_ptr() as usize
        || implementation.function_id != blockpy.function_id
        || implementation.template_identity != Arc::as_ptr(template) as usize
        || implementation.shared_state_identity != Arc::as_ptr(shared) as usize
        || data.source.as_ref() != blockpy.scope.source_origin.as_ref()
        || !shared.admits_function(blockpy)
    {
        return Err(strict_runtime_unavailable(
            py,
            "strict function is outside its authenticated compiler catalogue",
        ));
    }
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    if owner.reference(GLOBALS)?.as_ptr() != unsafe { (*raw).func_globals } {
        return Err(strict_runtime_unavailable(
            py,
            "strict function native globals changed",
        ));
    }
    let verified = shared
        .verified_strict_module()
        .ok_or_else(|| strict_runtime_unavailable(py, "strict function lost verified source"))?;
    if !implementation
        .module_source
        .matches_verified(verified.type_facts())
    {
        return Err(strict_runtime_unavailable(
            py,
            "strict function source stamp changed",
        ));
    }
    if data.source.as_ref().is_some_and(|origin| {
        matches!(
            origin.role,
            CallableSourceRole::SourceFunction
                | CallableSourceRole::AnnotationProvider
                | CallableSourceRole::TypeParameterScope
        )
    }) {
        let original = shared
            .lookup_original_code(implementation.function_id)
            .ok_or_else(|| {
                strict_runtime_unavailable(
                    py,
                    "strict function lost its original native code witness",
                )
            })?;
        if owner.reference(CODE)?.as_ptr() != original.as_ptr()
            || unsafe { PyCode_GetSoacStrictSourceId(original.as_ptr()) } == 0
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict function original-code provenance changed",
            ));
        }
    }
    let globals = owner.reference(GLOBALS)?.cast_into::<PyDict>()?;
    let policy = data.execution.acquire_owner(py, &globals, verified)?;
    if policy.as_ptr() != owner.reference(MODULE_POLICY)?.as_ptr() {
        return Err(strict_runtime_unavailable(
            py,
            "strict function module execution changed",
        ));
    }
    Ok(())
}

/// Authenticate one actual strict function before its first JIT metadata is
/// created. A catalogue entry alone is not a live function/execution owner.
pub(crate) fn authenticate_registration(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    shared: &Arc<SharedModuleState>,
    template: &Arc<FunctionInstantiationTemplate>,
) -> PyResult<()> {
    if unsafe { ffi::PyFunction_Check(function.as_ptr()) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "strict registration requires an exact native function",
        ));
    }
    let owner = unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) };
    if owner.is_null() {
        if !unsafe { ffi::PyErr_Occurred() }.is_null() {
            return Err(PyErr::fetch(py));
        }
        return Err(strict_runtime_unavailable(
            py,
            "strict registration requires its actual native owner",
        ));
    }
    let owner = StrictStateRef::<StrictFunctionData>::from_owner(unsafe {
        Bound::from_borrowed_ptr(py, owner)
    })?;
    authenticate_expected_owner(py, function, &owner, shared, template)?;
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    if owner.reference(CODE)?.as_ptr() != unsafe { (*raw).func_code }
        || unsafe { PyFunction_CheckSoacStrictDefaults(function.as_ptr()) } < 0
    {
        if !unsafe { ffi::PyErr_Occurred() }.is_null() {
            return Err(PyErr::fetch(py));
        }
        return Err(strict_runtime_unavailable(
            py,
            "strict registration native code changed before publication",
        ));
    }
    Ok(())
}

/// Authorize only an ordinary native call of replacement code, never source
/// execution. Keep the authenticating trampoline installed for every later
/// call; a terminal owner or a newly installed native contract must still fail.
pub(crate) fn authorize_ordinary_replacement(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    expected_metadata: *const crate::PyFunctionJitExtra,
) -> PyResult<()> {
    let current = unsafe { crate::py_function_jit_extra(function.as_ptr()) }
        .map_err(|()| PyErr::fetch(py))?;
    if current.cast_const() != expected_metadata {
        return Err(strict_runtime_unavailable(
            py,
            "replacement function does not own this entry metadata",
        ));
    }
    let owner = authenticate_function_owner(py, function)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "replacement function lost its native source owner")
    })?;
    let native_sealed = unsafe { PyFunction_GetSoacStrictId(function.as_ptr()) };
    if !unsafe { ffi::PyErr_Occurred() }.is_null() {
        return Err(PyErr::fetch(py));
    }
    if owner.is_finalized() || native_sealed != 0 {
        return Err(strict_runtime_unavailable(
            py,
            "finalized strict function cannot use replacement-code fallback",
        ));
    }
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    if owner.reference(CODE)?.as_ptr() == unsafe { (*raw).func_code } {
        return Err(strict_runtime_unavailable(
            py,
            "ordinary replacement fallback cannot execute original source code",
        ));
    }
    Ok(())
}

fn optional_reference_matches(
    owner: &StrictStateRef<'_, StrictFunctionData>,
    index: usize,
    actual: *mut ffi::PyObject,
) -> PyResult<bool> {
    let expected = owner.reference(index)?;
    Ok(if actual.is_null() {
        expected.is_none()
    } else {
        expected.as_ptr() == actual
    })
}

/// Adopt only the original provider still attached to this particular function
/// object. A provider from another invocation of the same source definition is
/// foreign too; equal source IDs and globals do not establish this relationship.
fn finalize_owned_annotation_provider(
    py: Python<'_>,
    owner: &AuthenticatedStrictFunction<'_, '_>,
    expected: &SourceIdentity,
) -> PyResult<()> {
    if owner.is_interpreter() {
        return interpreter::finalize_provider(py, owner);
    }
    if let Some(auth) = owner.owned_annotation_provider()? {
        let provider = auth.function();
        if !finalize_eligible_function(py, provider, expected)? {
            return Err(strict_runtime_unavailable(
                py,
                "owned annotation provider cannot be adopted",
            ));
        }
        auth.execution_ref().remove_pending(
            py,
            &*auth.globals()?,
            auth.verified_module(),
            &crate::StrictPendingKind::Function {
                function_id: auth.function_id()?,
            },
            provider,
        )?;
    }
    // Releasing this callback-free weakref cannot retain or release a provider
    // closure. The sealed function itself already owns any adopted provider.
    owner.set_reference(ANNOTATION_PROVIDER_WITNESS, py.None().into_bound(py))?;
    Ok(())
}

/// This must be called at module SEALING or at an explicit post-decoration
/// adoption boundary, not from MakeFunction. Unknown decorators stay dynamic.
pub(crate) fn finalize_eligible_function(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    expected: &SourceIdentity,
) -> PyResult<bool> {
    let Some(owner) = authenticate_borrowed_strict_function(py, function.as_borrowed())? else {
        return Ok(false);
    };
    if owner
        .source()
        .is_none_or(|origin| &origin.definition != expected)
    {
        return Err(strict_runtime_unavailable(
            py,
            "function adoption source mismatch",
        ));
    }
    if !owner.data().eligible {
        return Ok(false);
    }
    if owner.data().finalized.get() {
        crate::strict_nominal::complete_module_nominals(py, &owner)?;
        if !owner.is_interpreter() {
            crate::strict_optimization::bind_nominal_function_capabilities(py, function)?;
        }
        return Ok(true);
    }
    if !owner.is_interpreter() || owner.interpreter_source_authority()? {
        crate::strict_nominal::bind_lexical_function_nominals_with_auth(py, &owner)?;
        finalize_owned_annotation_provider(py, &owner, expected)?;
    }
    owner.data().capability_globals_pending.set(
        crate::strict_nominal::globals_pending_at_adoption(py, &owner)?,
    );
    if owner.is_interpreter() {
        interpreter::freeze(py, &owner)?;
        return Ok(true); // No optional JIT capability or compiler ID.
    }
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    owner.set_reference(
        DEFAULTS,
        owned_or_none(py, unsafe { (*raw).func_defaults }).into_bound(py),
    )?;
    owner.set_reference(
        KEYWORD_DEFAULTS,
        owned_or_none(py, unsafe { (*raw).func_kwdefaults }).into_bound(py),
    )?;
    owner.set_reference(
        CLOSURE,
        owned_or_none(py, unsafe { (*raw).func_closure }).into_bound(py),
    )?;
    let identity = owner.function_id()?.to_packed_runtime_u64();
    if identity == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "strict function identity is reserved",
        ));
    }
    if unsafe { PyFunction_SealSoacStrict(function.as_ptr(), identity) } < 0 {
        return Err(PyErr::fetch(py));
    }
    owner.data().finalized.set(true);
    crate::strict_optimization::bind_nominal_function_capabilities(py, function)?;
    Ok(true)
}

/// Completion of the actual SSA value produced by an undecorated definition.
/// The compiler emits this only after all metadata setup, before source STORE.
/// Numeric/source identity alone cannot authorize a Python-supplied value.
fn complete_function_definition<'py>(
    py: Python<'py>,
    function_id: RuntimeFunctionId,
    function: &Bound<'py, PyAny>,
    globals: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    // Partial authentication identifies deliberately dynamic/class-owned
    // values without imposing the original-code requirement on them. It does
    // not grant source execution or adoption; finalization authenticates fully.
    let owner = authenticate_function_owner(py, function)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "function completion has no actual creation owner")
    })?;
    let origin = owner
        .origin()
        .filter(|origin| origin.role == CallableSourceRole::SourceFunction)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "function completion has no source definition")
        })?;
    if owner.function_id()? != function_id || owner.globals()?.as_ptr() != globals.as_ptr() {
        return Err(strict_runtime_unavailable(
            py,
            "function completion changed creation identity",
        ));
    }
    let facts = owner.verified_module().type_facts().facts();
    let fact = facts
        .functions
        .iter()
        .find(|fact| fact.identity == origin.definition)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "function completion is outside the source catalogue")
        })?;
    if !owner.can_finalize()
        || !fact.decorators.is_empty()
        || owner.creation_execution().is_some()
        || facts.source_class_owner(&origin.definition).is_some()
        || !owner
            .execution_ref()
            .is_sealed(py, &*owner.globals()?, owner.verified_module())?
    {
        return Ok(function.clone());
    }
    if !finalize_eligible_function(py, function, &origin.definition)? {
        return Err(strict_runtime_unavailable(
            py,
            "source function could not complete adoption",
        ));
    }
    owner.execution_ref().remove_pending(
        py,
        &*owner.globals()?,
        owner.verified_module(),
        &crate::StrictPendingKind::Function { function_id },
        function,
    )?;
    Ok(function.clone())
}

/// Private generated-code ABI; no Python callable or mutable helper attribute
/// exposes this boundary. Inputs are borrowed and the result is an owned alias.
pub(crate) unsafe extern "C" fn soac_jit_complete_function_definition(
    function_id: u64,
    function: *mut ffi::PyObject,
    globals: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if function.is_null() || globals.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null function completion operand",
            ));
        }
        complete_function_definition(
            py,
            RuntimeFunctionId::from_packed_runtime_u64(function_id),
            &unsafe { Bound::<PyAny>::from_borrowed_ptr(py, function) },
            &unsafe { Bound::<PyAny>::from_borrowed_ptr(py, globals) },
        )
    }));
    match result {
        Ok(Ok(value)) => value.into_ptr(),
        Ok(Err(error)) => {
            error.restore(py);
            ptr::null_mut()
        }
        Err(_) => {
            strict_runtime_unavailable(py, "panic in function definition completion").restore(py);
            ptr::null_mut()
        }
    }
}

/// Bind once using the authenticated actual lexical operand or direct-self
/// class-construction witness. A source/name prediction alone cannot authorize
/// this operation; the operand need not have a strict layout or method policy.
///
/// # Safety
/// The caller has independently authenticated `actual_type` as this exact
/// annotation leaf's lexical value (including factory instances and aliases).
pub(crate) unsafe fn bind_strict_nominal_type(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    binding: &NominalBindingFact,
    actual_type: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let owner =
        authenticate_borrowed_strict_function(py, function.as_borrowed())?.ok_or_else(|| {
            strict_runtime_unavailable(py, "nominal binding requires a strict function")
        })?;
    if !owner.capability_nominal_bindings().contains(binding) {
        return Err(strict_runtime_unavailable(
            py,
            "nominal binding was not selected by an authenticated capability request",
        ));
    }
    if unsafe { ffi::PyType_Check(actual_type.as_ptr()) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "nominal binding requires an actual native type",
        ));
    }
    let previous = owner.data().nominal_bindings.borrow().get(binding).copied();
    let can_bind = !owner.is_finalized()
        || (owner.awaits_module_nominals()
            && binding.binding_scope
                == owner
                    .verified_module()
                    .type_facts()
                    .facts()
                    .module_body_identity()
            && {
                let globals = owner.globals()?;
                owner
                    .execution_ref()
                    .bindings_are_final(py, &globals, owner.verified_module())?
            });
    if let Some(index) = previous {
        let previous = owner.reference(index)?;
        if previous.as_ptr() == actual_type.as_ptr() {
            return Ok(());
        }
        if previous.is_none() && can_bind {
            return owner.bind_reserved_reference(index, actual_type.clone());
        }
        return Err(strict_runtime_unavailable(
            py,
            "nominal class binding cannot be replaced",
        ));
    }
    if !can_bind {
        return Err(strict_runtime_unavailable(
            py,
            "finalized nominal requirements cannot acquire a new target",
        ));
    }
    let existing = owner
        .data()
        .nominal_bindings
        .borrow()
        .values()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut shared_index = None;
    for index in existing {
        if owner.reference(index)?.as_ptr() == actual_type.as_ptr() {
            shared_index = Some(index);
            break;
        }
    }
    let index = match shared_index {
        Some(index) => index,
        None => owner.add_reference(actual_type.clone())?,
    };
    owner
        .data()
        .nominal_bindings
        .borrow_mut()
        .insert(binding.clone(), index);
    Ok(())
}

/// Called only after the authenticated execution's global-sealing boundary
/// bound every available still-pending module leaf. Missing leaves now remain
/// permanently unresolved, as do any non-module leaves unresolved at adoption.
pub(crate) fn finish_module_nominals(auth: &AuthenticatedStrictFunction<'_, '_>) -> PyResult<()> {
    auth.owner_ref().ensure_live()?;
    auth.data().capability_globals_pending.set(false);
    Ok(())
}

/// Keep direct-self leaves explicitly unresolved until final selected-type
/// admission. The actual function owns the reserved GC slot, not the class.
pub(crate) fn reserve_strict_nominal_types(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    bindings: &[NominalBindingFact],
) -> PyResult<()> {
    let owner =
        authenticate_borrowed_strict_function(py, function.as_borrowed())?.ok_or_else(|| {
            strict_runtime_unavailable(py, "nominal reservation requires a strict function")
        })?;
    if owner.is_finalized()
        || bindings.is_empty()
        || bindings.iter().any(|binding| {
            !owner.capability_nominal_bindings().contains(binding)
                || owner.data().nominal_bindings.borrow().contains_key(binding)
        })
    {
        return Err(strict_runtime_unavailable(
            py,
            "nominal reservation is not a fresh required function contract",
        ));
    }
    let index = owner.add_reference(py.None().into_bound(py))?;
    for binding in bindings {
        owner
            .data()
            .nominal_bindings
            .borrow_mut()
            .insert(binding.clone(), index);
    }
    Ok(())
}

/// A call-stack root, never persistent function metadata or a GC payload.
/// The binder owns each selected default directly; no unselected defaults are
/// retained here. Actual closure cells are copied only after binding finishes,
/// matching COPY_FREE_VARS after CPython's argument binder. Reentrant calls get
/// different environments.
pub(crate) struct StrictFunctionCall {
    environment: Option<Box<crate::FunctionEnv>>,
    // Identity and ownership are separate: terminal invocation cleanup
    // can take the nullable owner slots without changing captured identities.
    function: NonNull<ffi::PyObject>,
    function_reference: UnsafeCell<*mut ffi::PyObject>,
    source_owner: NonNull<ffi::PyObject>,
    source_owner_reference: UnsafeCell<*mut ffi::PyObject>,
    preserved_state: Option<NonNull<ffi::PyObject>>,
    binding_complete: bool,
    module_state: Option<Arc<SharedModuleState>>,
    terminal_protocol_releases: UnsafeCell<Box<[*mut ffi::PyObject]>>,
}

/// Immutable execution identities, copied before a resume can allocate or run
/// callbacks. These integers do not authorize a different function or capsule.
#[derive(Clone, Copy)]
struct StrictSuspendedFunctionIdentity {
    function_identity: usize,
    template_identity: usize,
    shared_state_identity: usize,
    function_id: RuntimeFunctionId,
    preserved_identity: usize,
    closure_len: usize,
    private_cell_len: usize,
}

const SUSPENDED_FUNCTION: usize = 0;
const SUSPENDED_SOURCE_OWNER: usize = 1;
const SUSPENDED_GLOBALS: usize = 2;
const SUSPENDED_BUILTINS: usize = 3;
const SUSPENDED_CODE: usize = 4;
const SUSPENDED_CELLS: usize = 5;

/// The one preserved capsule owns and traverses this fixed reference array.
/// There is no second Python shell whose GC clear could race the capsule's
/// terminal cleanup. Nullable raw slots are real owned references, not
/// casts into PyO3 wrappers, and remain address-stable until capsule teardown.
pub(crate) struct StrictSuspendedFunctionSnapshot {
    identity: StrictSuspendedFunctionIdentity,
    references: Box<[*mut ffi::PyObject]>,
}

impl StrictSuspendedFunctionSnapshot {
    fn new(
        py: Python<'_>,
        identity: StrictSuspendedFunctionIdentity,
        references: Vec<Py<PyAny>>,
    ) -> PyResult<Box<Self>> {
        let mut slots = Vec::new();
        slots.try_reserve_exact(references.len()).map_err(|_| {
            unsafe { ffi::PyErr_NoMemory() };
            PyErr::fetch(py)
        })?;
        // Every fallible allocation precedes the transfer from PyO3 owners.
        slots.extend(references.into_iter().map(Py::into_ptr));
        Ok(Box::new(Self {
            identity,
            references: slots.into_boxed_slice(),
        }))
    }

    unsafe fn reference<'py>(
        snapshot: *const Self,
        py: Python<'py>,
        index: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let value = unsafe { (&(*snapshot).references).get(index).copied() }
            .filter(|value| !value.is_null())
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "strict suspended snapshot reference is retired")
            })?;
        Ok(unsafe { Bound::from_borrowed_ptr(py, value) })
    }

    pub(crate) unsafe fn traverse(
        snapshot: *const Self,
        visit: ffi::visitproc,
        arg: *mut c_void,
    ) -> c_int {
        let count = unsafe { (&(*snapshot).references).len() };
        let references = unsafe { (*snapshot).references.as_ptr() };
        for index in 0..count {
            let object = unsafe { *references.add(index) };
            if !object.is_null() {
                let status = unsafe { visit(object, arg) };
                if status != 0 {
                    return status;
                }
            }
        }
        0
    }

    /// Native ownership/GC tests exercise this container without minting a
    /// source admission: zero identities never pass the production resume gate.
    #[cfg(test)]
    pub(crate) fn snapshot_with_references(
        py: Python<'_>,
        references: Vec<Py<PyAny>>,
    ) -> Box<Self> {
        let closure_len = references.len().saturating_sub(SUSPENDED_CELLS);
        Self::new(
            py,
            StrictSuspendedFunctionIdentity {
                function_identity: 0,
                template_identity: 0,
                shared_state_identity: 0,
                function_id: RuntimeFunctionId::from_packed_runtime_u64(0),
                preserved_identity: 0,
                closure_len,
                private_cell_len: 0,
            },
            references,
        )
        .expect("test snapshot references allocate")
    }
}

impl Drop for StrictSuspendedFunctionSnapshot {
    fn drop(&mut self) {
        let error = unsafe { ffi::PyErr_GetRaisedException() };
        // This snapshot is already unpublished from its capsule. Retire
        // captured cells and duplicate provenance/mapping pins before the
        // final actual-function reference. Each owned slot is cleared before
        // its release can invoke Python, including replaced closure cells.
        for index in (SUSPENDED_CELLS..self.references.len()).chain([
            SUSPENDED_SOURCE_OWNER,
            SUSPENDED_GLOBALS,
            SUSPENDED_BUILTINS,
            SUSPENDED_CODE,
            SUSPENDED_FUNCTION,
        ]) {
            if let Some(reference) = self.references.get_mut(index) {
                let value = std::mem::replace(reference, ptr::null_mut());
                unsafe { ffi::Py_XDECREF(value) };
            }
        }
        unsafe { ffi::PyErr_SetRaisedException(error) };
    }
}

impl StrictFunctionCall {
    /// This owner was authenticated when this call began. It authorizes only
    /// the already-active frame, not a later call after code replacement.
    pub(crate) fn captured_owner<'py>(
        &self,
        py: Python<'py>,
    ) -> PyResult<StrictStateRef<'py, StrictFunctionData>> {
        let owner = unsafe { *self.source_owner_reference.get() };
        if owner.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "strict source activation ownership has been retired",
            ));
        }
        StrictStateRef::from_owner(unsafe { Bound::from_borrowed_ptr(py, owner) })
    }

    pub(crate) fn record_direct_body(&self, py: Python<'_>, fixed_target: bool) -> PyResult<()> {
        self.captured_owner(py)?
            .data()
            .call_counters
            .direct_body(fixed_target);
        Ok(())
    }

    /// The binder's original arguments no longer support this activation view.
    pub(crate) fn retire_bound_arguments(&mut self) {
        self.environment_mut().header_mut().active_strict_call = ptr::null();
    }

    pub(crate) unsafe fn new(
        py: Python<'_>,
        function: *mut ffi::PyObject,
        metadata: &crate::FunctionCallMetadata,
    ) -> PyResult<Box<Self>> {
        let function = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, function) };
        let owner = authenticate_strict_function(py, &function)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "strict call has no live source owner")
        })?;
        authenticate_expected_owner(
            py,
            &function,
            &owner.owner,
            &metadata.module_state,
            &metadata.function_template,
        )?;
        let layout = metadata.function_template.runtime_data_layout();
        // Looking up keyword defaults can execute arbitrary equality methods.
        // Do not read defaults or cells before the normal binder needs them.
        let values = vec![ptr::null_mut(); layout.total_len()].into_boxed_slice();
        let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
        unsafe {
            Self::from_snapshot(
                py,
                function.clone(),
                owner.owner().clone(),
                metadata,
                values,
                (*raw).func_globals,
                (*raw).func_builtins,
                false,
            )
        }
    }

    unsafe fn from_snapshot(
        py: Python<'_>,
        function: Bound<'_, PyAny>,
        source_owner: Bound<'_, PyAny>,
        metadata: &crate::FunctionCallMetadata,
        values: Box<[*mut ffi::PyObject]>,
        globals: *mut ffi::PyObject,
        builtins: *mut ffi::PyObject,
        binding_complete: bool,
    ) -> PyResult<Box<Self>> {
        let mut environment = Box::new(
            unsafe {
                crate::FunctionEnv::new(
                    globals,
                    builtins,
                    metadata.module_state.late_bound_owner_fields.cells.as_ptr(),
                    values,
                    true,
                )
            }
            .map_err(|()| PyErr::fetch(py))?,
        );
        environment.set_direct_code_ptr(metadata.direct_code_ptr);
        environment.set_default_direct_code_ptr(metadata.default_direct_code_ptr);
        environment.set_deopt_table_ptr(metadata.deopt_table_ptr);
        environment.compiled_function = metadata.compiled_function.clone();
        environment.set_strict_field_capabilities(metadata.strict_field_capabilities.clone());
        environment.set_strict_method_capabilities(metadata.strict_method_capabilities.clone());
        let release_count = environment
            .runtime_object_len
            .checked_add(4)
            .ok_or_else(|| {
                unsafe { ffi::PyErr_NoMemory() };
                PyErr::fetch(py)
            })?;
        let mut terminal_releases = Vec::new();
        terminal_releases
            .try_reserve_exact(release_count)
            .map_err(|_| {
                unsafe { ffi::PyErr_NoMemory() };
                PyErr::fetch(py)
            })?;
        terminal_releases.resize(release_count, ptr::null_mut());
        let function = NonNull::new(function.into_ptr()).unwrap();
        let source_owner = NonNull::new(source_owner.into_ptr()).unwrap();
        let mut activation = Box::new(Self {
            environment: Some(environment),
            function,
            function_reference: UnsafeCell::new(function.as_ptr()),
            source_owner,
            source_owner_reference: UnsafeCell::new(source_owner.as_ptr()),
            preserved_state: None,
            binding_complete,
            module_state: Some(metadata.module_state.clone()),
            terminal_protocol_releases: UnsafeCell::new(terminal_releases.into_boxed_slice()),
        });
        let identity = ptr::from_ref(activation.as_ref());
        activation.environment_mut().header_mut().active_strict_call = identity;
        Ok(activation)
    }

    /// Attach once, before any Python generator-construction callback can see
    /// the preserved state. The pointer identifies this owned capsule, not a
    /// user-supplied source ID or Python attribute.
    pub(crate) fn attach_suspended_state(
        &self,
        py: Python<'_>,
        template: &Arc<FunctionInstantiationTemplate>,
        shared: &Arc<SharedModuleState>,
        preserved: &Bound<'_, PyAny>,
        source_code: &Bound<'_, PyAny>,
        closed_slot: usize,
    ) -> PyResult<()> {
        if !self.binding_complete {
            return Err(strict_runtime_unavailable(
                py,
                "suspended strict frame has not completed argument binding",
            ));
        }
        let layout = template.runtime_data_layout();
        let mut references = vec![
            unsafe { Bound::<PyAny>::from_borrowed_ptr(py, self.function()) }.unbind(),
            unsafe { Bound::<PyAny>::from_borrowed_ptr(py, self.source_owner.as_ptr()) }.unbind(),
            unsafe { Bound::<PyAny>::from_borrowed_ptr(py, self.environment().globals_obj()) }
                .unbind(),
            unsafe { Bound::<PyAny>::from_borrowed_ptr(py, self.environment().builtins_obj()) }
                .unbind(),
            source_code.clone().unbind(),
        ];
        for index in 0..layout.closure_len() {
            let cell = unsafe {
                self.environment()
                    .runtime_object(layout.closure_cell_slot(index))
            };
            if !crate::function_instantiation::is_cell_object(cell) {
                return Err(strict_runtime_unavailable(
                    py,
                    "strict suspended frame has no owned closure cell",
                ));
            }
            references.push(unsafe { Bound::<PyAny>::from_borrowed_ptr(py, cell) }.unbind());
        }
        for index in 0..layout.private_cell_len() {
            let cell = unsafe {
                self.environment()
                    .runtime_object(layout.private_cell_slot(index))
            };
            if !crate::function_instantiation::is_cell_object(cell) {
                return Err(strict_runtime_unavailable(
                    py,
                    "strict suspended frame has no private original cell",
                ));
            }
            references.push(unsafe { Bound::<PyAny>::from_borrowed_ptr(py, cell) }.unbind());
        }
        let snapshot = StrictSuspendedFunctionSnapshot::new(
            py,
            StrictSuspendedFunctionIdentity {
                function_identity: self.function() as usize,
                template_identity: Arc::as_ptr(template) as usize,
                shared_state_identity: Arc::as_ptr(shared) as usize,
                function_id: template.function().function_id,
                preserved_identity: preserved.as_ptr() as usize,
                closure_len: layout.closure_len(),
                private_cell_len: layout.private_cell_len(),
            },
            references,
        )?;
        unsafe {
            crate::preserved_state::attach_strict_resume_state(
                preserved.as_ptr(),
                snapshot,
                closed_slot,
            )
        }
        .map_err(|()| PyErr::fetch(py))?;
        Ok(())
    }

    pub(crate) fn original_code<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let owner = self.captured_owner(py)?;
        owner.reference(CODE)
    }

    /// Source functions and annotation providers retain the actual native parameter-name objects from
    /// the code authenticated at entry. Compiler-generated roles deliberately
    /// use their logical parameter plan instead: placeholder code objects do
    /// not describe those helpers' signatures.
    pub(crate) fn original_parameter_names<'py>(
        &self,
        py: Python<'py>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let owner = self.captured_owner(py)?;
        if !owner.source().is_some_and(|origin| {
            matches!(
                origin.role,
                CallableSourceRole::SourceFunction
                    | CallableSourceRole::AnnotationProvider
                    | CallableSourceRole::TypeParameterScope
            )
        }) {
            return Ok(None);
        }
        let code = owner.reference(CODE)?;
        let names = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(py, PyCode_GetVarnames(code.as_ptr().cast()))?
        };
        if unsafe { ffi::PyTuple_CheckExact(names.as_ptr()) } == 0 {
            return Err(strict_runtime_unavailable(
                py,
                "strict source parameter names are not a native tuple",
            ));
        }
        Ok(Some(names))
    }

    /// Validate the actual suspended frame, then pin its original closure cells
    /// for one resume. Later changes to the source function's defaults, code,
    /// or closure tuple cannot replace the frame that was already created.
    pub(crate) unsafe fn for_resume(
        py: Python<'_>,
        function: *mut ffi::PyObject,
        metadata: &crate::FunctionCallMetadata,
        preserved: *mut ffi::PyObject,
    ) -> PyResult<Box<Self>> {
        // Pin the sole GC owner before borrowing its fixed Rust snapshot.
        // No Rust reference into it may survive a Python callback/GC boundary.
        let preserved_pin = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, preserved) };
        let snapshot = unsafe { crate::preserved_state::strict_resume_snapshot(preserved) }
            .map_err(|()| PyErr::fetch(py))?;
        let data = unsafe { (*snapshot).identity };
        let layout = metadata.function_template.runtime_data_layout();
        if data.function_identity != function as usize
            || data.preserved_identity != preserved as usize
            || data.function_id != metadata.function_id
            || data.template_identity != Arc::as_ptr(&metadata.function_template) as usize
            || data.shared_state_identity != Arc::as_ptr(&metadata.module_state) as usize
            || data.closure_len != layout.closure_len()
            || data.private_cell_len != layout.private_cell_len()
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict suspended frame does not own this resume implementation",
            ));
        }
        let function = unsafe {
            StrictSuspendedFunctionSnapshot::reference(snapshot, py, SUSPENDED_FUNCTION)?
        };
        let source_owner = unsafe {
            StrictSuspendedFunctionSnapshot::reference(snapshot, py, SUSPENDED_SOURCE_OWNER)?
        };
        if function.as_ptr() as usize != data.function_identity {
            return Err(strict_runtime_unavailable(
                py,
                "strict suspended snapshot changed its actual function",
            ));
        }
        let actual_owner = unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) };
        if actual_owner.is_null() && !unsafe { ffi::PyErr_Occurred() }.is_null() {
            return Err(PyErr::fetch(py));
        }
        if actual_owner != source_owner.as_ptr() {
            return Err(strict_runtime_unavailable(
                py,
                "strict suspended frame lost its native source owner",
            ));
        }
        let owner = StrictStateRef::<StrictFunctionData>::from_owner(source_owner.clone())?;
        let verified = metadata
            .module_state
            .verified_strict_module()
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "strict suspended frame lost verified source")
            })?;
        if owner.function_id()? != data.function_id
            || owner.data().function_identity != data.function_identity
            || !owner
                .soac_implementation()?
                .module_source
                .matches_verified(verified.type_facts())
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict suspended frame source identity changed",
            ));
        }
        let globals =
            unsafe { StrictSuspendedFunctionSnapshot::reference(snapshot, py, SUSPENDED_GLOBALS)? }
                .cast_into::<PyDict>()?;
        let builtins = unsafe {
            StrictSuspendedFunctionSnapshot::reference(snapshot, py, SUSPENDED_BUILTINS)?
        };
        let policy = owner
            .data()
            .execution
            .acquire_owner(py, &globals, verified)?;
        if policy.as_ptr() != owner.reference(MODULE_POLICY)?.as_ptr()
            || globals.as_ptr() != owner.reference(GLOBALS)?.as_ptr()
        {
            return Err(strict_runtime_unavailable(
                py,
                "strict suspended frame module execution changed",
            ));
        }
        let cells = (0..layout.closure_len())
            .map(|index| unsafe {
                StrictSuspendedFunctionSnapshot::reference(snapshot, py, SUSPENDED_CELLS + index)
            })
            .collect::<PyResult<Vec<_>>>()?;
        let private_cells = (0..layout.private_cell_len())
            .map(|index| unsafe {
                StrictSuspendedFunctionSnapshot::reference(
                    snapshot,
                    py,
                    SUSPENDED_CELLS + layout.closure_len() + index,
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        let mut values = vec![ptr::null_mut(); layout.total_len()].into_boxed_slice();
        for (index, cell) in cells.into_iter().enumerate() {
            values[layout.closure_cell_slot(index)] = cell.into_ptr();
        }
        for (index, cell) in private_cells.into_iter().enumerate() {
            values[layout.private_cell_slot(index)] = cell.into_ptr();
        }
        let mut activation = unsafe {
            Self::from_snapshot(
                py,
                function,
                source_owner,
                metadata,
                values,
                globals.as_ptr(),
                builtins.as_ptr(),
                true,
            )
        }?;
        activation.preserved_state = NonNull::new(preserved_pin.into_ptr());
        Ok(activation)
    }

    pub(crate) fn function(&self) -> *mut ffi::PyObject {
        // A terminal transaction may have retired the actual owning edge.
        // Return NULL, never the potentially dead identity pointer, to an
        // accidental post-seam consumer.
        let function = unsafe { *self.function_reference.get() };
        debug_assert!(function.is_null() || function == self.function.as_ptr());
        function
    }
    pub(crate) fn preserved_state(&self) -> *mut ffi::PyObject {
        self.preserved_state
            .map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    /// Retire invocation-owned references at terminal completion. Take every
    /// nullable slot before callbacks so reentrant cleanup cannot release it twice.
    pub(crate) unsafe fn retire_terminal_protocol_roots(&self) -> Result<(), ()> {
        let py = unsafe { Python::assume_attached() };
        let releases = unsafe { &mut *self.terminal_protocol_releases.get() };
        if releases.iter().any(|value| !value.is_null()) {
            strict_runtime_unavailable(py, "terminal protocol root release reentered its drain")
                .restore(py);
            return Err(());
        }
        let releases = releases.as_mut_ptr();
        let environment = self.environment();
        let header = environment.abi.as_ptr();
        let objects = environment
            .runtime_objects_ptr()
            .cast::<*mut ffi::PyObject>();
        let count = environment.runtime_object_len;
        // Take every secondary edge before the first possible callback.
        unsafe {
            *releases.add(0) = ptr::replace(self.function_reference.get(), ptr::null_mut());
            *releases.add(1) = ptr::replace(self.source_owner_reference.get(), ptr::null_mut());
            *releases.add(2) = ptr::replace(&mut (*header).globals_obj, ptr::null_mut());
            *releases.add(3) = ptr::replace(&mut (*header).builtins_obj, ptr::null_mut());
        }
        for index in 0..count {
            unsafe { *releases.add(4 + index) = ptr::replace(objects.add(index), ptr::null_mut()) };
        }
        let error = unsafe { ffi::PyErr_GetRaisedException() };
        // Slots are already unpublished; callbacks can only observe retired state.
        for index in 4..4 + count {
            let value = unsafe { ptr::replace(releases.add(index), ptr::null_mut()) };
            unsafe { ffi::Py_XDECREF(value) };
        }
        for index in [1, 2, 3, 0] {
            let value = unsafe { ptr::replace(releases.add(index), ptr::null_mut()) };
            unsafe { ffi::Py_XDECREF(value) };
        }
        unsafe { ffi::PyErr_SetRaisedException(error) };
        Ok(())
    }

    pub(crate) fn active_module_state(&self) -> Option<&Arc<SharedModuleState>> {
        self.module_state.as_ref()
    }
    pub(crate) fn environment(&self) -> &crate::FunctionEnv {
        self.environment.as_deref().unwrap()
    }
    pub(crate) fn environment_mut(&mut self) -> &mut crate::FunctionEnv {
        self.environment.as_deref_mut().unwrap()
    }

    fn bind_private_cells(
        &mut self,
        py: Python<'_>,
        layout: &crate::jit::FunctionRuntimeDataLayout,
        cells: Vec<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        if cells.len() != layout.private_cell_len()
            || cells
                .iter()
                .any(|cell| !crate::function_instantiation::is_cell_object(cell.as_ptr()))
        {
            return Err(strict_runtime_unavailable(
                py,
                "active frame private cell projection changed",
            ));
        }
        for (index, cell) in cells.into_iter().enumerate() {
            let slot = layout.private_cell_slot(index);
            debug_assert!(unsafe { self.environment().runtime_object(slot) }.is_null());
            self.environment_mut().runtime_objects_mut()[slot] = cell.into_ptr();
        }
        Ok(())
    }

    /// Pin the then-current closure only after all argument/default lookups
    /// have finished. A keyword-default equality callback can replace cells;
    /// those changes affect this new call, but never an already-active frame.
    pub(crate) unsafe fn complete_binding(
        &mut self,
        py: Python<'_>,
        layout: &crate::jit::FunctionRuntimeDataLayout,
        parameters: &[*mut ffi::PyObject],
    ) -> PyResult<()> {
        if self.binding_complete {
            return Err(strict_runtime_unavailable(
                py,
                "strict argument binding was completed more than once",
            ));
        }
        let raw = self.function().cast::<ffi::PyFunctionObject>();
        let closure = unsafe { (*raw).func_closure };
        let count = if closure.is_null() {
            0
        } else if unsafe { ffi::PyTuple_Check(closure) } != 0 {
            unsafe { ffi::PyTuple_GET_SIZE(closure) as usize }
        } else {
            return Err(strict_runtime_unavailable(
                py,
                "strict closure is not a tuple",
            ));
        };
        if count != layout.closure_len() {
            return Err(strict_runtime_unavailable(
                py,
                "strict closure no longer matches the active source frame",
            ));
        }
        // No Python callbacks occur between validating the tuple and copying
        // its cells. All destination slots are still empty on this first bind.
        for index in 0..count {
            let cell = unsafe { ffi::PyTuple_GET_ITEM(closure, index as ffi::Py_ssize_t) };
            if !crate::function_instantiation::is_cell_object(cell) {
                return Err(strict_runtime_unavailable(
                    py,
                    "strict closure contains a non-cell value",
                ));
            }
        }
        for index in 0..count {
            let cell = unsafe { ffi::PyTuple_GET_ITEM(closure, index as ffi::Py_ssize_t) };
            let slot = layout.closure_cell_slot(index);
            debug_assert!(self.environment().runtime_object(slot).is_null());
            unsafe { ffi::Py_INCREF(cell) };
            self.environment_mut().runtime_objects_mut()[slot] = cell;
        }
        let source = StrictStateRef::<StrictFunctionData>::from_owner(unsafe {
            Bound::from_borrowed_ptr(py, self.source_owner.as_ptr())
        })?;
        if source
            .source()
            .is_some_and(|origin| origin.role == CallableSourceRole::ClassNamespace)
        {
            let function = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, self.function()) };
            let auth = authenticate_strict_function(py, &function)?.ok_or_else(|| {
                strict_runtime_unavailable(py, "class namespace lost its execution owner")
            })?;
            let consumed =
                crate::strict_namespace::consume_handle(py, &auth, self.function(), parameters)?
                    .ok_or_else(|| {
                        strict_runtime_unavailable(
                            py,
                            "class namespace did not bind its execution handle",
                        )
                    })?;
            self.bind_private_cells(py, layout, consumed.private_cells)?;
            self.environment_mut()
                .set_namespace_execution(consumed.execution);
        } else if source
            .source()
            .is_some_and(|origin| origin.role == CallableSourceRole::TypeParameterScope)
            && let Some(creation) = source.data().creation_execution.as_ref()
        {
            // Native generic scaffolding inside a running class forwards that
            // exact class execution to its children. This is not ambient
            // context for arbitrary source functions or an already completed
            // class: validate the captured creation handle before forwarding.
            let shared = self.module_state.as_ref().ok_or_else(|| {
                strict_runtime_unavailable(py, "type-parameter scope lost its module execution")
            })?;
            creation.validate_creation(py, shared, &source.reference(GLOBALS)?)?;
            self.environment_mut()
                .set_namespace_execution(creation.clone());
        }
        if source
            .source()
            .is_none_or(|origin| origin.role != CallableSourceRole::ClassNamespace)
        {
            let cells = private_lexical::active_private_cells(&source, layout.private_cell_len())?;
            self.bind_private_cells(py, layout, cells)?;
        }
        self.binding_complete = true;
        Ok(())
    }
}

impl Drop for StrictFunctionCall {
    fn drop(&mut self) {
        let error = unsafe { ffi::PyErr_GetRaisedException() };
        // Retire the binder view before environment cleanup can invoke Python.
        self.retire_bound_arguments();
        drop(self.environment.take());
        unsafe {
            let function = std::mem::replace(self.function_reference.get_mut(), ptr::null_mut());
            let source_owner =
                std::mem::replace(self.source_owner_reference.get_mut(), ptr::null_mut());
            ffi::Py_XDECREF(function);
            ffi::Py_XDECREF(source_owner);
            if let Some(preserved) = self.preserved_state {
                ffi::Py_DECREF(preserved.as_ptr());
            }
            drop(self.module_state.take());
            ffi::PyErr_SetRaisedException(error);
        }
    }
}

pub(crate) unsafe extern "C" fn strict_finish_call(
    activation: *mut c_void,
    result: *mut c_void,
) -> *mut c_void {
    let py = unsafe { Python::assume_attached() };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let activation = Box::from_raw(activation.cast::<StrictFunctionCall>());
        drop(activation);
        result
    })) {
        Ok(result) => result,
        Err(_) => {
            unsafe { ffi::Py_XDECREF(result.cast()) };
            strict_runtime_unavailable(py, "panic retiring strict source activation").restore(py);
            ptr::null_mut()
        }
    }
}
