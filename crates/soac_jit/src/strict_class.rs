//! The compiler-owned class-construction operation.
//!
//! Preparation and callbacks use CPython's actual class protocol. Only an
//! authenticated namespace function may request a construction plan, and only
//! a supported actual namespace/base combination receives the native handle.
//! Decorated classes remain dynamic until their explicit trusted adapter can
//! authenticate decorator values before construction and adopt the final type.

use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use soac_contracts::{
    ClassOpenness, ClassTypeFact, DescriptorFact, DescriptorKind, Fingerprint, MethodBinding,
    SourceIdentity,
};
use soac_core::block_py::{CallableSourceRole, RuntimeFunctionId};

use crate::strict_class_state::{self, StrictClassState};
use crate::strict_dataclass::{
    DataclassConstruction, DataclassNamespace, DataclassSlotsConstruction, RawDataclassFrameView,
};
use crate::strict_function::{
    AuthenticatedStrictFunction, StrictFunctionCall, authenticate_class_candidate_function,
    authenticate_strict_function, finalize_eligible_function, take_class_construction_captures,
};
use crate::strict_module::StrictPendingKind;
use crate::strict_namespace::{NamespaceExecution, NamespaceHandle};
use crate::strict_runtime_unavailable;

pub(crate) const CONSTRUCT_CLASS_SYMBOL: &str = "soac_jit_construct_class";

/// Descriptor ownership is checked before, during, and after the one-way
/// pre-Ready commit. A copied namespace is not yet evidence of a descriptor seal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClassNamespacePhase {
    Input,
    Copied,
    BeforeTransform,
    Adopted,
}

#[repr(C)]
pub(crate) struct RawSoacTypeContractSpec {
    pub(crate) flags: u32,
    pub(crate) dictionary_mode: u32,
    pub(crate) fields: *mut ffi::PyObject,
    pub(crate) protected_names: *mut ffi::PyObject,
    pub(crate) final_methods: *mut ffi::PyObject,
    pub(crate) object_slot_fields: *mut ffi::PyObject,
    pub(crate) check_instance_write: Option<
        unsafe extern "C" fn(
            *mut ffi::PyObject,
            *mut ffi::PyObject,
            *mut ffi::PyObject,
            *mut ffi::PyObject,
        ) -> c_int,
    >,
    pub(crate) new_instance_dict:
        Option<unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject) -> *mut ffi::PyObject>,
    pub(crate) prepare_instance_dictionary_policy: Option<
        unsafe extern "C" fn(
            *mut ffi::PyObject,
            *mut ffi::PyObject,
            *mut ffi::PyObject,
            *const strict_class_state::RawPySoacInstanceDictPolicy,
            *mut strict_class_state::RawPySoacInstanceDictPolicy,
        ) -> c_int,
    >,
}

impl RawSoacTypeContractSpec {
    const fn pending() -> Self {
        Self {
            flags: 0,
            dictionary_mode: 0,
            fields: ptr::null_mut(),
            protected_names: ptr::null_mut(),
            final_methods: ptr::null_mut(),
            object_slot_fields: ptr::null_mut(),
            check_instance_write: None,
            new_instance_dict: None,
            prepare_instance_dictionary_policy: None,
        }
    }
}

pub(crate) type RawSoacFinalTypeCommit = unsafe extern "C" fn(
    *mut ffi::PyObject,
    *mut ffi::PyObject,
    *const RawSoacTypeContractSpec,
) -> c_int;

#[repr(C)]
struct RawSoacTypeConstructionSpec {
    abi_version: u32,
    struct_size: u32,
    construction_mode: u32,
    reserved: u32,
    owner: *mut ffi::PyObject,
    namespace_function: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    bases: *mut ffi::PyObject,
    namespace_dict: *mut ffi::PyObject,
    keywords: *mut ffi::PyObject,
    bind_type: Option<unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject) -> c_int>,
    commit_final: Option<RawSoacFinalTypeCommit>,
    contract: RawSoacTypeContractSpec,
}

#[cfg(test)]
pub(crate) const TYPE_CONSTRUCTION_ABI_LAYOUT: [(&str, usize); 6] = [
    ("abi_version", 4),
    (
        "spec_size",
        std::mem::size_of::<RawSoacTypeConstructionSpec>(),
    ),
    (
        "object_slot_fields",
        std::mem::offset_of!(RawSoacTypeConstructionSpec, contract)
            + std::mem::offset_of!(RawSoacTypeContractSpec, object_slot_fields),
    ),
    (
        "contract_size",
        std::mem::size_of::<RawSoacTypeContractSpec>(),
    ),
    (
        "contract",
        std::mem::offset_of!(RawSoacTypeConstructionSpec, contract),
    ),
    (
        "commit_final",
        std::mem::offset_of!(RawSoacTypeConstructionSpec, commit_final),
    ),
];

/// Temporary native name operands. Only these metadata tuples are owned here;
/// actual types, namespaces and functions stay in their ordinary caller slots.
struct PreparedTypeContract<'py> {
    flags: u32,
    dictionary_mode: u32,
    fields: Bound<'py, PyTuple>,
    protected_names: Bound<'py, PyTuple>,
    final_methods: Bound<'py, PyTuple>,
    object_slot_fields: Bound<'py, PyTuple>,
}

impl<'py> PreparedTypeContract<'py> {
    fn new(state: &StrictClassState<'py>) -> PyResult<Self> {
        Ok(Self {
            flags: u32::from(state.fact().openness == ClassOpenness::DeclaredFinal),
            dictionary_mode: state.dictionary_mode()?,
            fields: state.fields()?,
            protected_names: state.protected_names()?,
            final_methods: state.final_methods()?,
            object_slot_fields: state.object_fields()?,
        })
    }

    fn native(&self) -> RawSoacTypeContractSpec {
        RawSoacTypeContractSpec {
            flags: self.flags,
            dictionary_mode: self.dictionary_mode,
            fields: self.fields.as_ptr(),
            protected_names: self.protected_names.as_ptr(),
            final_methods: self.final_methods.as_ptr(),
            object_slot_fields: self.object_slot_fields.as_ptr(),
            check_instance_write: Some(strict_class_state::check_instance_write),
            new_instance_dict: (self.dictionary_mode == 1)
                .then_some(strict_class_state::new_instance_dict),
            prepare_instance_dictionary_policy: (self.dictionary_mode == 2)
                .then_some(strict_class_state::prepare_instance_dictionary_policy),
        }
    }
}

unsafe extern "C" {
    static mut PyCell_Type: ffi::PyTypeObject;
    fn PySoac_PrepareClass(
        name: *mut ffi::PyObject,
        bases: *mut ffi::PyObject,
        keywords: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_CompleteClassNamespace(
        preparation: *mut ffi::PyObject,
        original_bases: *mut ffi::PyObject,
    ) -> c_int;
    fn PySoac_FinishClass(
        name: *mut ffi::PyObject,
        returned_class_cell: *mut ffi::PyObject,
        class: *mut ffi::PyObject,
    ) -> c_int;
    fn PyType_NewSoacConstructionHandle(
        spec: *const RawSoacTypeConstructionSpec,
    ) -> *mut ffi::PyObject;
    fn PyType_AdmitSoacPendingV1(
        actual_type: *mut ffi::PyObject,
        expected_owner: *mut ffi::PyObject,
        expected_root_construction: *mut ffi::PyObject,
        contract: *const RawSoacTypeContractSpec,
        contract_size: usize,
        expected_commit: Option<RawSoacFinalTypeCommit>,
    ) -> c_int;
    fn PyType_FromSoacConstructionHandle(
        handle: *mut ffi::PyObject,
        namespace_function: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PyType_NewSoacDataclassSlotsHandle(
        producer: *const RawDataclassFrameView,
        spec: *const RawSoacTypeConstructionSpec,
    ) -> *mut ffi::PyObject;
    fn PyType_GetDict(class: *mut ffi::PyTypeObject) -> *mut ffi::PyObject;
    fn _PySoac_StaticMethodFunction(descriptor: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn _PySoac_ClassMethodFunction(descriptor: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn _PySoac_PropertyFunction(
        descriptor: *mut ffi::PyObject,
        accessor: c_int,
    ) -> *mut ffi::PyObject;
    fn _PySoac_IsDescriptorSealed(descriptor: *mut ffi::PyObject) -> c_int;
    static mut PyStaticMethod_Type: ffi::PyTypeObject;
    static mut PyClassMethod_Type: ffi::PyTypeObject;
    static mut PyProperty_Type: ffi::PyTypeObject;
}

/// Reuse the production native ABI for class-state kernel tests, including the
/// actual Pending binding/admission callbacks. The caller owns a test-only
/// prepared state and decides when optional module-final publication runs.
#[cfg(test)]
pub(crate) fn construct_type_for_class_state_test<'py>(
    state: &StrictClassState<'py>,
    bases: &Bound<'py, PyTuple>,
    namespace: &Bound<'py, PyDict>,
) -> PyResult<Bound<'py, PyAny>> {
    let py = state.owner().py();
    let name = pyo3::types::PyString::new(
        py,
        state
            .source()
            .lexical_qualname
            .rsplit('.')
            .next()
            .expect("fixture class name"),
    );
    let namespace_function = py.eval(c"lambda: None", None, None)?;
    let keywords = PyDict::new(py);
    let handle = new_source_type_handle(
        py,
        state,
        &namespace_function,
        name.as_any(),
        bases,
        namespace.as_any(),
        &keywords,
        state.fact(),
    )?;
    let class = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            PyType_FromSoacConstructionHandle(handle.as_ptr(), namespace_function.as_ptr()),
        )
    }?;
    let pending =
        strict_class_state::for_constructed_type(py, &class)?.expect("actual Pending fixture");
    assert!(pending.is_pending_type());
    assert!(strict_class_state::for_actual_type(py, &class)?.is_none());
    assert!(
        class.call0().is_err(),
        "fixture bypassed the native Pending allocation barrier"
    );
    assert!(admit_class(py, &class, state.source())?);
    Ok(class)
}

/// Keep independent references while disposing the native tuple so its reverse
/// tuple destruction cannot reorder the stock preparation cleanup. No mutable
/// namespace is retained in a published class capability.
struct Preparation<'py> {
    native: Option<Bound<'py, PyTuple>>,
    meta: Option<Bound<'py, PyAny>>,
    namespace: Option<Bound<'py, PyAny>>,
    bases: Option<Bound<'py, PyTuple>>,
    keywords: Option<Bound<'py, PyDict>>,
}

impl<'py> Preparation<'py> {
    fn new(
        py: Python<'py>,
        name: &Bound<'py, PyAny>,
        bases: &Bound<'py, PyAny>,
        keywords: &Bound<'py, PyAny>,
    ) -> PyResult<Self> {
        // The class rewrite uses None for an absent keyword operand, just as
        // types.prepare_class accepts it. The native protocol takes an exact
        // dictionary so it can snapshot evaluated keyword bindings before
        // running any __mro_entries__ callback. Normalize only that compiler
        // omission; the native boundary still rejects arbitrary mappings.
        let empty_keywords = keywords.is_none().then(|| PyDict::new(py));
        let keywords = empty_keywords
            .as_ref()
            .map_or(keywords, |empty| empty.as_any());
        let native = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                PySoac_PrepareClass(name.as_ptr(), bases.as_ptr(), keywords.as_ptr()),
            )?
        }
        .cast_into::<PyTuple>()?;
        if native.len() != 4 {
            return Err(strict_runtime_unavailable(
                py,
                "invalid native class preparation shape",
            ));
        }
        Ok(Self {
            meta: Some(native.get_item(0)?),
            namespace: Some(native.get_item(1)?),
            bases: Some(native.get_item(2)?.cast_into::<PyTuple>()?),
            keywords: Some(native.get_item(3)?.cast_into::<PyDict>()?),
            native: Some(native),
        })
    }
    fn native(&self) -> &Bound<'py, PyTuple> {
        self.native.as_ref().unwrap()
    }
    fn meta(&self) -> &Bound<'py, PyAny> {
        self.meta.as_ref().unwrap()
    }
    fn namespace(&self) -> &Bound<'py, PyAny> {
        self.namespace.as_ref().unwrap()
    }
    fn bases(&self) -> &Bound<'py, PyTuple> {
        self.bases.as_ref().unwrap()
    }
    fn keywords(&self) -> &Bound<'py, PyDict> {
        self.keywords.as_ref().unwrap()
    }
}

impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        drop(self.native.take());
        drop(self.namespace.take());
        drop(self.meta.take());
        drop(self.keywords.take());
        drop(self.bases.take());
    }
}

pub(crate) fn method_function<'py>(
    value: &Bound<'py, PyAny>,
    binding: MethodBinding,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let kind = unsafe { ffi::Py_TYPE(value.as_ptr()) };
    match binding {
        MethodBinding::Instance if unsafe { ffi::PyFunction_Check(value.as_ptr()) } != 0 => {
            Ok(Some(value.clone()))
        }
        MethodBinding::Static if kind == ptr::addr_of_mut!(PyStaticMethod_Type) => {
            Ok(Some(borrowed_component(value, unsafe {
                _PySoac_StaticMethodFunction(value.as_ptr())
            })?))
        }
        MethodBinding::Class if kind == ptr::addr_of_mut!(PyClassMethod_Type) => {
            Ok(Some(borrowed_component(value, unsafe {
                _PySoac_ClassMethodFunction(value.as_ptr())
            })?))
        }
        MethodBinding::PropertyGetter if kind == ptr::addr_of_mut!(PyProperty_Type) => {
            Ok(Some(property_function(value, 0)?))
        }
        _ => Ok(None),
    }
}

fn borrowed_component<'py>(
    owner: &Bound<'py, PyAny>,
    component: *mut ffi::PyObject,
) -> PyResult<Bound<'py, PyAny>> {
    if component.is_null() {
        return Err(PyErr::fetch(owner.py()));
    }
    Ok(unsafe { Bound::<PyAny>::from_borrowed_ptr(owner.py(), component) })
}

fn property_function<'py>(
    property: &Bound<'py, PyAny>,
    accessor: c_int,
) -> PyResult<Bound<'py, PyAny>> {
    borrowed_component(property, unsafe {
        _PySoac_PropertyFunction(property.as_ptr(), accessor)
    })
}

/// The native binding callback must not allocate even temporary Python lookup
/// strings. Scan the exact dict and compare codepoints, without hashing or
/// running arbitrary equality. The returned value is pinned before use.
pub(crate) fn namespace_item<'py>(
    namespace: &Bound<'py, PyDict>,
    name: &str,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let py = namespace.py();
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    while unsafe { ffi::PyDict_Next(namespace.as_ptr(), &mut position, &mut key, &mut value) } != 0
    {
        if unsafe { ffi::PyUnicode_CheckExact(key) } == 0 {
            return Err(strict_runtime_unavailable(
                py,
                "class namespace contains a non-exact string key",
            ));
        }
        let key = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, key) };
        if strict_class_state::exact_name_matches(&key, name) {
            return Ok(Some(unsafe {
                Bound::<PyAny>::from_borrowed_ptr(py, value)
            }));
        }
    }
    Ok(None)
}

fn methods_match(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    fact: &ClassTypeFact,
    phase: ClassNamespacePhase,
    execution: &Arc<NamespaceExecution>,
    dataclass: Option<&DataclassNamespace<'_>>,
) -> PyResult<bool> {
    for method in &fact.methods {
        let Some(expected) = &method.implementation else {
            if method.generated.is_some() {
                if method.declaring_class.definition != fact.identity {
                    // A separately authenticated actual base owns an inherited
                    // generated entry; copying it into this namespace would
                    // need a distinct actual-component adoption proof.
                    if namespace_item(namespace, &method.name)?.is_some() {
                        return Ok(false);
                    }
                } else if !dataclass.is_some_and(|proof| proof.generated_method(&method.name)) {
                    return Ok(false);
                }
            }
            continue;
        };
        let Some(value) = namespace_item(namespace, &method.name)? else {
            if method.declaring_class.definition == fact.identity {
                return Ok(false);
            }
            continue; // An actual protected base owns this inherited entry.
        };
        // An inherited method explicitly copied into this namespace needs
        // an actual base-component witness, not source membership alone.
        if method.declaring_class.definition != fact.identity {
            return Ok(false);
        }
        let implicit_raw = phase == ClassNamespacePhase::Input
            && unsafe { ffi::PyFunction_Check(value.as_ptr()) } != 0
            && matches!(
                (method.name.as_str(), method.binding),
                ("__new__", MethodBinding::Static)
                    | (
                        "__init_subclass__" | "__class_getitem__",
                        MethodBinding::Class
                    )
            );
        let function = if implicit_raw {
            let Some(auth) = authenticate_class_candidate_function(py, &value)? else {
                return Ok(false);
            };
            if !auth
                .verified_module()
                .type_facts()
                .facts()
                .functions
                .iter()
                .any(|function| &function.identity == expected && function.decorators.is_empty())
            {
                return Ok(false);
            }
            value.clone()
        } else {
            let Some(function) = method_function(&value, method.binding)? else {
                return Ok(false);
            };
            if method.binding != MethodBinding::Instance
                && !descriptor_matches(
                    py,
                    &value,
                    &function,
                    &method.name,
                    method.binding,
                    phase,
                    execution,
                )?
            {
                return Ok(false);
            }
            function
        };
        if !source_function_matches(
            py,
            &function,
            expected,
            method.declaring_class.source_digest,
            execution,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn implicit_descriptor_function(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    name: &str,
    binding: MethodBinding,
) -> PyResult<bool> {
    if !strict_class_state::is_implicit_wrapper(binding, name) {
        return Ok(false);
    }
    let Some(auth) = authenticate_class_candidate_function(py, function)? else {
        return Ok(false);
    };
    Ok(auth.origin().is_some_and(|origin| {
        auth.verified_module()
            .type_facts()
            .facts()
            .functions
            .iter()
            .any(|fact| fact.identity == origin.definition && fact.decorators.is_empty())
    }))
}

fn descriptor_matches(
    py: Python<'_>,
    descriptor: &Bound<'_, PyAny>,
    function: &Bound<'_, PyAny>,
    name: &str,
    binding: MethodBinding,
    phase: ClassNamespacePhase,
    execution: &Arc<NamespaceExecution>,
) -> PyResult<bool> {
    if implicit_descriptor_function(py, function, name, binding)? {
        // Only type_new's actual wrapper creation grants this seal; an input
        // wrapper around the same undecorated source function is not eligible.
        return Ok(phase != ClassNamespacePhase::Input
            && unsafe { _PySoac_IsDescriptorSealed(descriptor.as_ptr()) } == 1);
    }
    Ok(
        crate::strict_descriptor::matches_birth(py, descriptor, function, execution)?
            && (phase != ClassNamespacePhase::Adopted
                || unsafe { _PySoac_IsDescriptorSealed(descriptor.as_ptr()) } == 1),
    )
}

fn source_function_matches(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    expected: &SourceIdentity,
    digest: Fingerprint,
    execution: &Arc<NamespaceExecution>,
) -> PyResult<bool> {
    let Some(actual) = authenticate_class_candidate_function(py, function)? else {
        return Ok(false);
    };
    Ok(actual.can_finalize()
        && execution.is_completed()
        && actual
            .creation_execution()
            .is_some_and(|creation| Arc::ptr_eq(creation, execution))
        && actual.verified_module().type_facts().facts().source_digest == digest
        && actual.origin().is_some_and(|origin| {
            origin.role == CallableSourceRole::SourceFunction && &origin.definition == expected
        }))
}

/// Namespace validation has authenticated every actual component before this
/// pre-Ready commit. Never accept an inherited or borrowed source function.
fn validate_class_function_owners_at_phase(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    fact: &ClassTypeFact,
    execution: &Arc<NamespaceExecution>,
    phase: ClassNamespacePhase,
) -> PyResult<()> {
    for (name, binding, _) in own_method_components(fact) {
        let value = namespace_item(namespace, name)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "admitted method disappeared before owner validation")
        })?;
        let function =
            construction_method_function(&value, name, binding, phase)?.ok_or_else(|| {
                strict_runtime_unavailable(py, "admitted method changed descriptor kind")
            })?;
        crate::strict_function::validate_class_function_owner(py, &function, execution)?;
    }
    Ok(())
}

/// Before type_new_set_attrs, CPython has not yet made its implicit wrappers.
/// Full namespace/source validation precedes this exact component selection.
pub(crate) fn construction_method_function<'py>(
    value: &Bound<'py, PyAny>,
    name: &str,
    binding: MethodBinding,
    phase: ClassNamespacePhase,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    if phase == ClassNamespacePhase::Input
        && strict_class_state::is_implicit_wrapper(binding, name)
        && unsafe { ffi::PyFunction_Check(value.as_ptr()) } != 0
    {
        Ok(Some(value.clone()))
    } else {
        method_function(value, binding)
    }
}

/// The pending bind hook authenticated the real copied namespace and owns its
/// callback-free weak type witness. Bind only that execution's functions and
/// original annotation providers; never a borrowed foreign function.
pub(crate) unsafe fn bind_pending_namespace_function_witnesses(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    fact: &ClassTypeFact,
    execution: &Arc<NamespaceExecution>,
    witness: &Bound<'_, PyAny>,
) -> PyResult<()> {
    for (name, binding, _) in own_method_components(fact) {
        let value = namespace_item(namespace, name)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "pending class method disappeared before binding")
        })?;
        let function =
            construction_method_function(&value, name, binding, ClassNamespacePhase::Input)?
                .ok_or_else(|| {
                    strict_runtime_unavailable(py, "pending class method changed kind")
                })?;
        unsafe {
            crate::strict_function::bind_class_weak_witness(py, &function, execution, witness)?;
        }
        let auth = crate::strict_function::authenticate_borrowed_strict_function(
            py,
            function.as_borrowed(),
        )?
        .ok_or_else(|| strict_runtime_unavailable(py, "pending class function lost its owner"))?;
        if auth.is_interpreter() {
            if let Some(provider) = unsafe { auth.borrowed_native_annotation_provider()? } {
                unsafe {
                    crate::strict_function::bind_class_weak_witness(
                        py,
                        provider.function(),
                        execution,
                        witness,
                    )?;
                }
            }
        } else if let Some(provider) = auth.owned_annotation_provider()? {
            // Retained construction keeps its existing provider ownership.
            unsafe {
                crate::strict_function::bind_class_weak_witness(
                    py,
                    provider.function(),
                    execution,
                    witness,
                )?;
            }
        }
    }
    for name in ["__annotate__", "__annotate_func__"] {
        if fact.methods.iter().any(|method| method.name == name) {
            continue;
        }
        if let Some(provider) = namespace_item(namespace, name)?
            && !provider.is_none()
        {
            unsafe {
                crate::strict_function::bind_class_weak_witness(py, &provider, execution, witness)?;
            }
        }
    }
    Ok(())
}

/// Source-method and descriptor-getter requirements share one preparation and
/// activation path. Some checker catalogs represent a property only as a field
/// or class-member descriptor; those getters must not lose mandatory checks.
pub(crate) fn own_method_components(
    fact: &ClassTypeFact,
) -> impl Iterator<Item = (&str, MethodBinding, &SourceIdentity)> {
    let methods = fact.methods.iter().filter_map(|method| {
        (method.declaring_class.definition == fact.identity)
            .then_some(method.implementation.as_ref())
            .flatten()
            .map(|source| (method.name.as_str(), method.binding, source))
    });
    let properties = fact
        .instance_fields
        .iter()
        .filter(|field| field.declaring_class.definition == fact.identity)
        .map(|field| (field.name.as_str(), &field.descriptor))
        .chain(
            fact.class_members
                .iter()
                .map(|member| (member.name.as_str(), &member.descriptor)),
        )
        .filter(|(name, descriptor)| {
            descriptor.kind == DescriptorKind::Property
                && !fact.methods.iter().any(|method| method.name == *name)
        })
        .filter_map(|(name, descriptor)| {
            descriptor
                .getter
                .as_ref()
                .map(|getter| (name, MethodBinding::PropertyGetter, getter))
        });
    methods.chain(properties)
}

/// All actual namespace components have been validated at this phase. This
/// callback-free commit seals only this construction's fresh explicit wrappers.
/// Input implicit methods still are functions; type_new seals their wrappers
/// when it creates them, before any class callbacks.
pub(crate) fn adopt_class_descriptors(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    fact: &ClassTypeFact,
    execution: &Arc<NamespaceExecution>,
    phase: ClassNamespacePhase,
) -> PyResult<()> {
    for (name, binding, _) in own_method_components(fact) {
        if binding == MethodBinding::Instance {
            continue;
        }
        let value = namespace_item(namespace, name)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "admitted descriptor disappeared before adoption")
        })?;
        let function =
            construction_method_function(&value, name, binding, phase)?.ok_or_else(|| {
                strict_runtime_unavailable(py, "admitted descriptor changed kind before adoption")
            })?;
        if !implicit_descriptor_function(py, &function, name, binding)? {
            crate::strict_descriptor::adopt(py, &value, &function, execution)?;
        }
    }
    Ok(())
}

fn property_facts<'a>(
    fact: &'a ClassTypeFact,
    own_digest: Fingerprint,
) -> impl Iterator<Item = (&'a str, &'a DescriptorFact, &'a SourceIdentity, Fingerprint)> {
    fact.instance_fields
        .iter()
        .map(|field| {
            (
                field.name.as_str(),
                &field.descriptor,
                &field.declaring_class.definition,
                field.declaring_class.source_digest,
            )
        })
        .chain(fact.class_members.iter().map(move |member| {
            (
                member.name.as_str(),
                &member.descriptor,
                &fact.identity,
                own_digest,
            )
        }))
        .filter(|(_, descriptor, _, _)| descriptor.kind == DescriptorKind::Property)
}

/// Revalidate the actual copied type namespace before native callbacks, as
/// well as at final adoption. Allocation between the first admission and
/// type_new can run GC finalizers that mutate an escaped input namespace.
pub(crate) fn validate_actual_class_namespace(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    verified: &crate::VerifiedStrictModule,
    fact: &ClassTypeFact,
    phase: ClassNamespacePhase,
    execution: &Arc<NamespaceExecution>,
    dataclass: Option<&DataclassNamespace<'_>>,
) -> PyResult<bool> {
    validate_class_namespace(py, namespace, verified, fact, phase, execution, dataclass)
}

fn validate_class_namespace(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    verified: &crate::VerifiedStrictModule,
    fact: &ClassTypeFact,
    phase: ClassNamespacePhase,
    execution: &Arc<NamespaceExecution>,
    dataclass: Option<&DataclassNamespace<'_>>,
) -> PyResult<bool> {
    if let Some(proof) = dataclass {
        if !proof.matches(fact, verified.type_facts().facts().source_digest, execution)
            || !proof.validate(namespace, phase)?
        {
            return Ok(false);
        }
    }
    if !namespace
        .iter()
        .all(|(key, _)| unsafe { ffi::PyUnicode_CheckExact(key.as_ptr()) } != 0)
        || execution.source() != &fact.identity
        || !execution.is_completed()
        || !methods_match(py, namespace, fact, phase, execution, dataclass)?
    {
        return Ok(false);
    }
    for (name, descriptor, declaring, digest) in
        property_facts(fact, verified.type_facts().facts().source_digest)
    {
        let Some(property) = namespace_item(namespace, name)? else {
            if declaring == &fact.identity {
                return Ok(false);
            }
            continue;
        };
        if unsafe { ffi::Py_TYPE(property.as_ptr()) } != ptr::addr_of_mut!(PyProperty_Type) {
            return Ok(false);
        }
        for (slot, expected) in [
            (0, &descriptor.getter),
            (1, &descriptor.setter),
            (2, &descriptor.deleter),
        ] {
            let actual = property_function(&property, slot)?;
            match expected {
                None if actual.is_none() => {}
                Some(expected)
                    if expected.module == declaring.module
                        && declaring == &fact.identity
                        && source_function_matches(py, &actual, expected, digest, execution)? => {}
                _ => return Ok(false),
            }
        }
        if !descriptor_matches(
            py,
            &property,
            &property_function(&property, 0)?,
            name,
            MethodBinding::PropertyGetter,
            phase,
            execution,
        )? {
            return Ok(false);
        }
    }
    for name in ["__annotate__", "__annotate_func__"] {
        if fact.methods.iter().any(|method| method.name == name) {
            continue;
        }
        if let Some(provider) = namespace_item(namespace, name)? {
            if phase != ClassNamespacePhase::Input
                && name == "__annotate_func__"
                && provider.is_none()
            {
                // CPython lazily caches the absence of class annotations as
                // __annotate_func__ = None. This is not an executable provider
                // and grants no function authority. Callable providers still
                // need this exact source and namespace-execution witness.
                continue;
            }
            let Some(auth) = authenticate_strict_function(py, &provider)? else {
                return Ok(false);
            };
            if auth.verified_module().type_facts().facts().source_digest
                != verified.type_facts().facts().source_digest
                || !auth
                    .creation_execution()
                    .is_some_and(|creation| Arc::ptr_eq(creation, execution))
                || !auth.origin().is_some_and(|origin| {
                    origin.role == CallableSourceRole::AnnotationProvider
                        && origin.definition == fact.identity
                })
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn construct_type<'py>(
    py: Python<'py>,
    state: &StrictClassState<'py>,
    namespace_function: &Bound<'py, PyAny>,
    name: &Bound<'py, PyAny>,
    prepared: &Preparation<'py>,
    fact: &ClassTypeFact,
) -> PyResult<Bound<'py, PyAny>> {
    let handle = new_source_type_handle(
        py,
        state,
        namespace_function,
        name,
        prepared.bases(),
        prepared.namespace(),
        prepared.keywords(),
        fact,
    )?;
    let result = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            PyType_FromSoacConstructionHandle(handle.as_ptr(), namespace_function.as_ptr()),
        )
    };
    drop(handle);
    result
}

/// A single native type-construction specification for both execution lanes.
/// The existing native handle copies these supported C operands; native
/// interpreter consumption removes duplicate input pins before callbacks.
fn new_source_type_handle<'py>(
    py: Python<'py>,
    state: &StrictClassState<'py>,
    namespace_function: &Bound<'py, PyAny>,
    name: &Bound<'py, PyAny>,
    bases: &Bound<'py, PyTuple>,
    namespace: &Bound<'py, PyAny>,
    keywords: &Bound<'py, PyDict>,
    _fact: &ClassTypeFact,
) -> PyResult<Bound<'py, PyAny>> {
    let spec = RawSoacTypeConstructionSpec {
        abi_version: 4,
        struct_size: std::mem::size_of::<RawSoacTypeConstructionSpec>() as u32,
        construction_mode: 1, // Authenticated source types always start Pending.
        reserved: 0,
        owner: state.owner().as_ptr(),
        namespace_function: namespace_function.as_ptr(),
        name: name.as_ptr(),
        bases: bases.as_ptr(),
        namespace_dict: namespace.as_ptr(),
        keywords: keywords.as_ptr(),
        bind_type: Some(strict_class_state::bind_pending_type),
        commit_final: Some(strict_class_state::commit_pending_type),
        contract: RawSoacTypeContractSpec::pending(),
    };
    unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, PyType_NewSoacConstructionHandle(&spec)) }
}

/// The caller has authenticated the actual native CLASS callsite and the
/// transferred namespace activation. Optional declines occur before any
/// irreversible type policy; construction itself stays inside __build_class__.
pub(crate) fn prepare_interpreter_type_handle<'py>(
    py: Python<'py>,
    auth: &AuthenticatedStrictFunction<'_, 'py>,
    execution: &Arc<NamespaceExecution>,
    invocation: &Arc<crate::strict_interpreter::InterpreterInvocationIdentity>,
    namespace_function: &Bound<'py, PyAny>,
    metaclass: &Bound<'py, PyAny>,
    name: &Bound<'py, PyAny>,
    bases: &Bound<'py, PyTuple>,
    namespace: &Bound<'py, PyAny>,
    keywords: Option<&Bound<'py, PyDict>>,
    dataclass: Option<&DataclassConstruction<'py>>,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let origin = auth
        .origin()
        .filter(|origin| origin.role == CallableSourceRole::ClassNamespace)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "native class has no authenticated namespace source")
        })?;
    let verified = auth.verified_module();
    let fact = verified
        .type_facts()
        .facts()
        .classes
        .iter()
        .find(|fact| fact.identity == origin.definition)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "native class source has no checked proposal")
        })?;
    if !auth.interpreter_source_authority()? {
        return Ok(None);
    }
    if metaclass.as_ptr() != ptr::addr_of_mut!(ffi::PyType_Type).cast()
        || unsafe { ffi::PyDict_CheckExact(namespace.as_ptr()) } == 0
        || (!fact.decorators.is_empty() && dataclass.is_none())
    {
        return Ok(None);
    }
    let namespace = namespace.cast::<PyDict>()?;
    let dataclass_namespace = dataclass.map(DataclassConstruction::namespace);
    if !validate_class_namespace(
        py,
        namespace,
        verified,
        fact,
        ClassNamespacePhase::Input,
        execution,
        dataclass_namespace.as_ref(),
    )? {
        return Ok(None);
    }
    let state = match strict_class_state::prepare_class_state(
        py, auth, fact, bases, namespace, execution, None, dataclass,
    )? {
        Ok(state) => state,
        Err(reason) => {
            tracing::debug!(?reason, class = %fact.identity.lexical_qualname,
                "native strict class uses dynamic construction");
            return Ok(None);
        }
    };
    state.select_interpreter_completion(invocation)?;
    // All dynamic declines are complete. Commit independent selected method
    // checks before native handle/type allocation can dispatch a GC callback.
    // A subsequent construction failure must not revoke these function checks.
    validate_class_function_owners_at_phase(
        py,
        namespace,
        fact,
        execution,
        ClassNamespacePhase::Input,
    )?;
    // No native keyword mapping exists for a bare class call. The existing
    // handle owns this exact synthetic empty mapping until it is destroyed;
    // the private native borrow mask does not mistake it for a C-owned input.
    let empty_keywords;
    let keywords = match keywords {
        Some(keywords) => keywords,
        None => {
            empty_keywords = PyDict::new(py);
            &empty_keywords
        }
    };
    new_source_type_handle(
        py,
        &state,
        namespace_function,
        name,
        bases,
        namespace.as_any(),
        keywords,
        fact,
    )
    .map(Some)
}

/// Prepare the distinct handle consumed by the same authenticated native
/// `_add_slots` call. The original namespace function is neither retained nor
/// impersonated: this construction has its own native replacement mode and
/// actual class witness, linked to the completed declaring source execution.
pub(crate) fn prepare_dataclass_slots_type_handle<'py>(
    py: Python<'py>,
    producer: *const RawDataclassFrameView,
    original: &Bound<'py, PyAny>,
    name: &Bound<'py, PyAny>,
    bases: &Bound<'py, PyTuple>,
    namespace: &Bound<'py, PyDict>,
    proof: &DataclassSlotsConstruction<'py>,
) -> PyResult<Bound<'py, PyAny>> {
    let original = strict_class_state::for_constructed_type(py, original)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "dataclass replacement has no actual original contract")
    })?;
    if !original.is_pending_type() {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass replacement requires its actual pending original",
        ));
    }
    let state =
        strict_class_state::prepare_replacement_class_state(&original, bases, namespace, proof)?;
    let keywords = PyDict::new(py);
    let spec = RawSoacTypeConstructionSpec {
        abi_version: 4,
        struct_size: std::mem::size_of::<RawSoacTypeConstructionSpec>() as u32,
        construction_mode: 1, // Linked replacement shares the still-closed Pending lineage.
        reserved: 0,
        owner: state.owner().as_ptr(),
        namespace_function: ptr::null_mut(),
        name: name.as_ptr(),
        bases: bases.as_ptr(),
        namespace_dict: namespace.as_ptr(),
        keywords: keywords.as_ptr(),
        bind_type: Some(strict_class_state::bind_pending_type),
        commit_final: Some(strict_class_state::commit_pending_type),
        contract: RawSoacTypeContractSpec::pending(),
    };
    unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            PyType_NewSoacDataclassSlotsHandle(producer, &spec),
        )
    }
}

fn construct_class<'py>(
    py: Python<'py>,
    construction_function: RuntimeFunctionId,
    active: &StrictFunctionCall,
    arguments: [&Bound<'py, PyAny>; 7],
    decorator_preparation: Option<&Bound<'py, PyAny>>,
    globals: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let [
        name,
        namespace_function,
        original_bases,
        keywords,
        needs_cell,
        needs_dict_cell,
        _first_line,
    ] = arguments;
    let auth = authenticate_strict_function(py, namespace_function)?.ok_or_else(|| {
        strict_runtime_unavailable(
            py,
            "class namespace function has no authenticated execution owner",
        )
    })?;
    let origin = auth
        .origin()
        .filter(|origin| origin.role == CallableSourceRole::ClassNamespace)
        .ok_or_else(|| {
            strict_runtime_unavailable(
                py,
                "class construction requires its actual namespace function",
            )
        })?;
    let constructor = auth
        .module_state()?
        .lookup_function(construction_function)
        .ok_or_else(|| {
            strict_runtime_unavailable(
                py,
                "class construction site is absent from its actual module",
            )
        })?;
    let caller = active.captured_owner(py)?;
    let expected_module = auth.module_state()?;
    let same_module = active
        .active_module_state()
        .is_some_and(|module| Arc::ptr_eq(module, expected_module));
    if !constructor
        .scope
        .source_origin
        .as_ref()
        .is_some_and(|site| {
            site.role == CallableSourceRole::ClassConstruction
                && site.definition == origin.definition
        })
        || !same_module
        || caller.function_id()? != construction_function
        || !caller.source().is_some_and(|source| {
            source.role == CallableSourceRole::ClassConstruction
                && source.definition == origin.definition
        })
        || caller.global_dictionary()?.as_ptr() != globals.as_ptr()
        || auth.globals()?.as_ptr() != globals.as_ptr()
    {
        return Err(strict_runtime_unavailable(
            py,
            "class construction belongs to another source or module execution",
        ));
    }
    let verified = auth.verified_module().clone();
    let fact = verified
        .type_facts()
        .facts()
        .classes
        .iter()
        .find(|fact| fact.identity == origin.definition)
        .ok_or_else(|| strict_runtime_unavailable(py, "authenticated class plan is absent"))?
        .clone();
    // Consume this helper's original cells and paired namespace identity before
    // preparation can run a metaclass callback. Contents remain live until the
    // body has executed; only the field binders below select their values.
    let construction_captures = take_class_construction_captures(py, active, &auth, &fact)?;
    let decorator = decorator_preparation
        .map(|preparation| {
            crate::strict_class_decorator::begin_construction(
                py,
                preparation,
                construction_function,
                &auth,
            )
        })
        .transpose()?;
    let needs_cell = if needs_cell.as_ptr() == unsafe { ffi::Py_True() } {
        true
    } else if needs_cell.as_ptr() == unsafe { ffi::Py_False() } {
        false
    } else {
        return Err(strict_runtime_unavailable(
            py,
            "invalid compiler class-cell operand",
        ));
    };
    let needs_dict_cell = if needs_dict_cell.as_ptr() == unsafe { ffi::Py_True() } {
        true
    } else if needs_dict_cell.as_ptr() == unsafe { ffi::Py_False() } {
        false
    } else {
        return Err(strict_runtime_unavailable(
            py,
            "invalid compiler class-dictionary-cell operand",
        ));
    };
    let namespace_template = auth
        .module_state()?
        .lookup_function(auth.function_id()?)
        .ok_or_else(|| strict_runtime_unavailable(py, "class namespace template is absent"))?;
    let bindings = namespace_template
        .scope
        .class_bindings
        .as_ref()
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "class namespace has no canonical native binding recipe")
        })?;
    let exports = |kind| {
        bindings
            .recipe
            .exports
            .iter()
            .any(|export| export.kind == kind)
    };
    if needs_cell != exports(soac_core::block_py::ClassBindingExportKind::ClassCell)
        || needs_dict_cell != exports(soac_core::block_py::ClassBindingExportKind::ClassDictCell)
    {
        return Err(strict_runtime_unavailable(
            py,
            "class construction cell declarations differ from the native recipe",
        ));
    }
    // Preparation has no cells. Their exact allocation/copy order belongs to
    // the namespace body's native binding recipe, after its handle is bound.
    let prepared = Preparation::new(py, name, original_bases, keywords)?;
    let handle = NamespaceHandle::new(
        py,
        &auth,
        namespace_function,
        prepared.namespace(),
        construction_captures.as_ref(),
    )?;
    let args = [prepared.namespace().as_ptr(), handle.argument().as_ptr()];
    let returned_class_cell = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            ffi::PyObject_Vectorcall(
                namespace_function.as_ptr(),
                args.as_ptr(),
                args.len(),
                ptr::null_mut(),
            ),
        )?
    };
    if (unsafe { ffi::Py_TYPE(returned_class_cell.as_ptr()) } == ptr::addr_of_mut!(PyCell_Type))
        != needs_cell
        || (!needs_cell && !returned_class_cell.is_none())
    {
        return Err(strict_runtime_unavailable(
            py,
            "class body did not return its native current class cell",
        ));
    }
    let execution = handle.complete()?;
    drop(handle);
    if unsafe { PySoac_CompleteClassNamespace(prepared.native().as_ptr(), original_bases.as_ptr()) }
        < 0
    {
        return Err(PyErr::fetch(py));
    }

    let builtin_namespace = prepared.meta().as_ptr() == ptr::addr_of_mut!(ffi::PyType_Type).cast()
        && unsafe { ffi::PyDict_CheckExact(prepared.namespace().as_ptr()) } != 0;
    let dataclass = if builtin_namespace {
        if let Some(decorator) = &decorator {
            decorator.prepare_dataclass(
                &auth,
                prepared.namespace().cast::<PyDict>()?,
                prepared.bases(),
                &execution,
                construction_captures.as_ref(),
            )?
        } else {
            None
        }
    } else {
        None
    };
    let dataclass_namespace = dataclass.as_ref().map(|proof| proof.namespace());
    let state = if builtin_namespace
        && ((decorator.is_none() && fact.decorators.is_empty()) || dataclass.is_some())
    {
        let namespace = prepared.namespace().cast::<PyDict>()?;
        // Do not run a user key's equality during authority admission. The
        // ordinary metaclass remains responsible for its own namespace error.
        if validate_class_namespace(
            py,
            namespace,
            &verified,
            &fact,
            ClassNamespacePhase::Input,
            &execution,
            dataclass_namespace.as_ref(),
        )? {
            match strict_class_state::prepare_class_state(
                py,
                &auth,
                &fact,
                prepared.bases(),
                namespace,
                &execution,
                construction_captures.as_ref(),
                dataclass.as_ref(),
            )? {
                Ok(state) => Some(state),
                Err(reason) => {
                    tracing::debug!(?reason, class = %fact.identity.lexical_qualname, "strict class uses dynamic construction");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };
    if state.is_none()
        && let Some(decorator) = &decorator
    {
        // All optional declines precede installation. Applying the same
        // evaluated decorator afterward still uses ordinary call semantics.
        decorator.decline_dataclass()?;
    }
    let class = if let Some(state) = &state {
        validate_class_function_owners_at_phase(
            py,
            prepared.namespace().cast::<PyDict>()?,
            &fact,
            &execution,
            ClassNamespacePhase::Input,
        )?;
        construct_type(py, state, namespace_function, name, &prepared, &fact)?
    } else {
        let args = [
            name.as_ptr(),
            prepared.bases().as_ptr(),
            prepared.namespace().as_ptr(),
        ];
        unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                ffi::PyObject_VectorcallDict(
                    prepared.meta().as_ptr(),
                    args.as_ptr(),
                    args.len(),
                    prepared.keywords().as_ptr(),
                ),
            )?
        }
    };
    let completed = (|| -> PyResult<()> {
        if unsafe {
            PySoac_FinishClass(name.as_ptr(), returned_class_cell.as_ptr(), class.as_ptr())
        } < 0
        {
            return Err(PyErr::fetch(py));
        }
        drop(returned_class_cell);
        drop(prepared);
        if let Some(state) = &state {
            auth.execution_ref().register_pending(
                py,
                &*auth.globals()?,
                &verified,
                StrictPendingKind::Class {
                    source: fact.identity.clone(),
                },
                &class,
            )?;
            if !state.pending_dataclass() && !admit_class(py, &class, &fact.identity)? {
                return Err(strict_runtime_unavailable(
                    py,
                    "source class lost its actual result before final admission",
                ));
            }
            if auth
                .execution_ref()
                .is_ready(py, &*auth.globals()?, &verified)?
                && !state.pending_dataclass()
            {
                finalize_class(py, &class, &fact.identity)?;
                auth.execution_ref().remove_pending(
                    py,
                    &*auth.globals()?,
                    &verified,
                    &StrictPendingKind::Class {
                        source: fact.identity.clone(),
                    },
                    &class,
                )?;
            }
        }
        if let Some(decorator) = &decorator {
            decorator.complete(&class)?;
        }
        // The compiler-owned construction path replaces runtime.create_class,
        // including its optional observation of real split-key insertions.
        // Observing the actual result neither seals a dynamic class nor grants
        // indexed storage; an absent profile must not be fabricated from its
        // declaring namespace or from optional constructor/function IDs.
        unsafe { crate::module_type::watch_split_keys_for_type(class.as_ptr()) }.map_err(|()| {
            if unsafe { ffi::PyErr_Occurred() }.is_null() {
                strict_runtime_unavailable(py, "failed to watch created type key layout")
            } else {
                PyErr::fetch(py)
            }
        })?;
        Ok(())
    })();
    if let Err(error) = completed {
        if let Some(state) = &state {
            let _ = state.fail_unfinished_type();
        }
        return Err(error);
    }
    Ok(class)
}

/// Enforce the actual selected construct/Apply result before returning it.
/// Native policy/seal publish before instances. Retained optional capabilities
/// still wait for the existing module-finalization boundary.
pub(crate) fn admit_class(
    py: Python<'_>,
    class: &Bound<'_, PyAny>,
    source: &SourceIdentity,
) -> PyResult<bool> {
    complete_class(py, class, source, false)
}

pub(crate) fn finalize_class(
    py: Python<'_>,
    class: &Bound<'_, PyAny>,
    source: &SourceIdentity,
) -> PyResult<bool> {
    complete_class(py, class, source, true)
}

fn complete_class(
    py: Python<'_>,
    class: &Bound<'_, PyAny>,
    source: &SourceIdentity,
    finalize_optional: bool,
) -> PyResult<bool> {
    let Some(state) = strict_class_state::for_constructed_type(py, class)? else {
        return Ok(false);
    };
    if state.source() != source {
        return Err(strict_runtime_unavailable(
            py,
            "class adoption source mismatch",
        ));
    }
    let dictionary = unsafe { PyType_GetDict(class.as_ptr().cast()) };
    if dictionary.is_null() {
        return Err(PyErr::fetch(py));
    }
    // PyType_GetDict returns a new reference, unlike the pre-Ready tp_dict
    // borrow used by the native binding callback. Retaining that extra edge
    // would keep the class's functions and module globals alive indefinitely.
    let namespace =
        unsafe { Bound::<PyAny>::from_owned_ptr(py, dictionary) }.cast_into::<PyDict>()?;
    if state.is_finalized() {
        if finalize_optional {
            publish_class_function_capabilities(py, &namespace, &state)?;
        }
        return Ok(true);
    }
    let dataclass = state.dataclass_namespace()?;
    let pending = state.is_pending_type();
    if !validate_actual_class_namespace(
        py,
        &namespace,
        state.verified_module(),
        state.fact(),
        if pending && dataclass.is_none() {
            ClassNamespacePhase::Copied
        } else {
            ClassNamespacePhase::Adopted
        },
        state.namespace_execution(),
        dataclass.as_ref(),
    )? {
        return Err(strict_runtime_unavailable(
            py,
            format!(
                "installed class methods no longer match their authenticated implementations: {}",
                source.lexical_qualname
            ),
        ));
    }
    if pending {
        adopt_class_descriptors(
            py,
            &namespace,
            state.fact(),
            state.namespace_execution(),
            ClassNamespacePhase::Copied,
        )?;
        state.bind_final_type_requirements()?;
    }
    adopt_source_members(py, &namespace, source, &state)?;
    if pending {
        // Every Python-object allocation, required function/default policy and
        // descriptor adoption precedes the final native admission. Its captured
        // callback rechecks this exact payload and actual dictionary after all
        // native preparation, before native enables instances.
        let prepared = PreparedTypeContract::new(&state)?;
        let contract = prepared.native();
        let root = state.begin_pending_admission()?;
        let status = unsafe {
            PyType_AdmitSoacPendingV1(
                class.as_ptr(),
                state.owner().as_ptr(),
                root.as_ptr(),
                &contract,
                std::mem::size_of::<RawSoacTypeContractSpec>(),
                Some(strict_class_state::commit_pending_type),
            )
        };
        if status < 0 {
            let error = PyErr::fetch(py);
            let _ = state.fail_unfinished_type();
            return Err(error);
        }
    }
    if finalize_optional {
        state.seal()?;
        publish_class_function_capabilities(py, &namespace, &state)?;
    }
    Ok(true)
}

fn adopt_source_members(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    source: &SourceIdentity,
    state: &StrictClassState<'_>,
) -> PyResult<()> {
    for method in &state.fact().methods {
        if let Some(source) = &method.implementation
            && let Some(value) = namespace_item(&namespace, &method.name)?
            && let Some(function) = method_function(&value, method.binding)?
        {
            adopt_function(py, &function, source, &state)?;
        }
    }
    for (name, descriptor, _, _) in property_facts(
        state.fact(),
        state.verified_module().type_facts().facts().source_digest,
    ) {
        let Some(property) = namespace_item(&namespace, name)? else {
            continue;
        };
        for (slot, source) in [
            (0, &descriptor.getter),
            (1, &descriptor.setter),
            (2, &descriptor.deleter),
        ] {
            if let Some(source) = source {
                adopt_function(py, &property_function(&property, slot)?, source, &state)?;
            }
        }
    }
    // These are compiler-owned class providers, not every same-named callable
    // on a dynamic framework class. Admission above authenticated their exact
    // AnnotationProvider role, lexical owner, source digest, and execution.
    for name in ["__annotate__", "__annotate_func__"] {
        if state
            .fact()
            .methods
            .iter()
            .any(|method| method.name == name)
        {
            continue;
        }
        if let Some(provider) = namespace_item(&namespace, name)?
            && !provider.is_none()
        {
            adopt_function(py, &provider, source, &state)?;
        }
    }
    Ok(())
}

/// Preserve source-only witnesses while the real decorator CALL still owns
/// its original operand. This does not admit the class, bind direct self, or
/// freeze source functions before their final decorated target is selected.
pub(crate) fn snapshot_dataclass_source_members(
    py: Python<'_>,
    class: &Bound<'_, PyAny>,
    state: &StrictClassState<'_>,
) -> PyResult<()> {
    let namespace = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(py, PyType_GetDict(class.as_ptr().cast()))?
    }
    .cast_into::<PyDict>()?;
    let dataclass = state.dataclass_namespace()?.ok_or_else(|| {
        strict_runtime_unavailable(py, "source snapshot has no active dataclass plan")
    })?;
    if !validate_actual_class_namespace(
        py,
        &namespace,
        state.verified_module(),
        state.fact(),
        ClassNamespacePhase::BeforeTransform,
        state.namespace_execution(),
        Some(&dataclass),
    )? {
        return Err(strict_runtime_unavailable(
            py,
            "actual source namespace changed before dataclass Apply",
        ));
    }
    adopt_class_descriptors(
        py,
        &namespace,
        state.fact(),
        state.namespace_execution(),
        ClassNamespacePhase::BeforeTransform,
    )?;
    for (name, binding, _) in own_method_components(state.fact()) {
        let value = namespace_item(&namespace, name)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "source method disappeared before dataclass Apply")
        })?;
        let function = method_function(&value, binding)?.ok_or_else(|| {
            strict_runtime_unavailable(
                py,
                "source method changed descriptor before dataclass Apply",
            )
        })?;
        crate::strict_nominal::bind_pretransform_method_nominals(py, &function, state)?;
    }
    Ok(())
}

fn publish_class_function_capabilities(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    state: &StrictClassState<'_>,
) -> PyResult<()> {
    if state.is_interpreter_construction() {
        return Ok(());
    }
    // Optional field capabilities require the completed class seal. Unlike a
    // required nominal contract, these store only non-owning execution/layout
    // witnesses and must not keep a class alive through an escaped method.
    for method in &state.fact().methods {
        if let Some(value) = namespace_item(namespace, &method.name)?
            && let Some(function) = method_function(&value, method.binding)?
        {
            crate::strict_optimization::bind_owned_function_capabilities(py, &function, state)?;
        }
    }
    for (name, descriptor, _, _) in property_facts(
        state.fact(),
        state.verified_module().type_facts().facts().source_digest,
    ) {
        let Some(property) = namespace_item(namespace, name)? else {
            continue;
        };
        for (slot, source) in [
            (0, &descriptor.getter),
            (1, &descriptor.setter),
            (2, &descriptor.deleter),
        ] {
            if source.is_some() {
                crate::strict_optimization::bind_owned_function_capabilities(
                    py,
                    &property_function(&property, slot)?,
                    state,
                )?;
            }
        }
    }
    Ok(())
}

fn adopt_function(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    source: &SourceIdentity,
    state: &StrictClassState<'_>,
) -> PyResult<()> {
    crate::strict_nominal::bind_owned_method_nominals(py, function, state)?;
    if !finalize_eligible_function(py, function, source)? {
        return Err(strict_runtime_unavailable(
            py,
            "class target cannot acquire a frozen function contract",
        ));
    }
    if let Some(auth) =
        crate::strict_function::authenticate_borrowed_strict_function(py, function.as_borrowed())?
    {
        if auth.awaits_module_nominals() {
            // Required method metadata is already frozen. Calls themselves
            // have no runtime parameter or return-type contract.
            // The existing weak function receipt, not a new owner registry,
            // completes unresolved module leaves at the one global seal.
            return Ok(());
        }
        let kind = if auth.is_interpreter() {
            StrictPendingKind::InterpreterFunction {
                native_code_ordinal: auth.native_code_ordinal()?,
            }
        } else {
            StrictPendingKind::Function {
                function_id: auth.function_id()?,
            }
        };
        auth.execution_ref().remove_pending(
            py,
            &*auth.globals()?,
            auth.verified_module(),
            &kind,
            function,
        )?;
    }
    Ok(())
}

/// Generated-code ABI. Both the active helper frame and the actual namespace
/// function authenticate the numeric site; no Python value supplies the frame.
///
/// # Safety
/// `environment` is the current compiler-passed FunctionEnv ABI header and
/// remains live throughout the operation, including reentrant class callbacks.
pub(crate) unsafe extern "C" fn soac_jit_construct_class(
    construction_function: u64,
    environment: *const c_void,
    name: *mut ffi::PyObject,
    namespace_function: *mut ffi::PyObject,
    bases: *mut ffi::PyObject,
    keywords: *mut ffi::PyObject,
    needs_cell: *mut ffi::PyObject,
    needs_dict_cell: *mut ffi::PyObject,
    first_line: *mut ffi::PyObject,
    decorator_preparation: *mut ffi::PyObject,
    globals: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<Bound<'_, PyAny>> {
        let header = unsafe { environment.cast::<crate::FunctionEnvAbiHeader>().as_ref() }
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "class construction has no active frame")
            })?;
        let active = unsafe { header.active_strict_call.as_ref() }.ok_or_else(|| {
            strict_runtime_unavailable(py, "class construction frame is unauthenticated")
        })?;
        if !ptr::eq(active.environment().header(), header) || header.globals_obj != globals {
            return Err(strict_runtime_unavailable(
                py,
                "class construction frame identity changed",
            ));
        }
        let pointers = [
            name,
            namespace_function,
            bases,
            keywords,
            needs_cell,
            needs_dict_cell,
            first_line,
            globals,
        ];
        if pointers.iter().any(|pointer| pointer.is_null()) {
            return Err(strict_runtime_unavailable(
                py,
                "null strict construction operand",
            ));
        }
        let values =
            pointers.map(|pointer| unsafe { Bound::<PyAny>::from_borrowed_ptr(py, pointer) });
        let decorator_preparation = (!decorator_preparation.is_null())
            .then(|| unsafe { Bound::<PyAny>::from_borrowed_ptr(py, decorator_preparation) });
        construct_class(
            py,
            RuntimeFunctionId::from_packed_runtime_u64(construction_function),
            active,
            [
                &values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6],
            ],
            decorator_preparation.as_ref(),
            &values[7],
        )
    }));
    match result {
        Ok(Ok(value)) => value.into_ptr(),
        Ok(Err(error)) => {
            error.restore(py);
            ptr::null_mut()
        }
        Err(_) => {
            strict_runtime_unavailable(py, "panic in native class construction").restore(py);
            ptr::null_mut()
        }
    }
}
