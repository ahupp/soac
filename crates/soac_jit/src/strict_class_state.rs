//! GC-owned class authority and independently owned instance storage policies.
//!
//! The class owner binds one authenticated construction to one actual type.
//! An instance dictionary owns only a minimal storage policy: escaping `vars`
//! must not keep the receiver, its class, or its module alive. Source catalogs
//! describe requirements; neither those catalogs nor an inherited dictionary
//! factory confer a strict receiver or a checked-value proof.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, c_int, c_uint, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::{Arc, OnceLock};

use pyo3::exceptions::PyUnicodeEncodeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple};
use soac_contracts::{
    BaseReference, BuiltinType, ClassDictionarySemantics, ClassMemberKind, ClassReference,
    ClassTypeFact, DescriptorFact, DescriptorKind, DynamicClassReason, FieldKind, Fingerprint,
    MetaclassFact, MethodBinding, ParticipationProposal, ResolvedStrictPolicy, SourceIdentity,
    StaticType,
};
use soac_core::block_py::CallableSourceRole;

use crate::strict_dataclass::{
    DataclassAdoptedMembers, DataclassClassState, DataclassConstruction, DataclassNamespace,
    DataclassSlotsConstruction,
};
use crate::strict_field_bindings::{StrictFieldBinding, prepare_own_field_bindings};
use crate::strict_fields::{
    StrictFieldChecks, own_checked_fields, prepare_field_checks, selected_field_contract,
};
use crate::strict_function::{
    AuthenticatedStrictFunction, ClassConstructionCaptures, authenticate_strict_function,
};
use crate::strict_slots::ObjectSlotPlan;
use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{StrictModuleExecutionRef, VerifiedStrictModule, strict_runtime_unavailable};

// These are native C ABI values, not Python-visible policy switches.
const VALIDATE_INITIAL: c_int = 0;
const SET: c_int = 1;
const DELETE: c_int = 2;
const CLEAR: c_int = 3;
const TERMINAL_TEARDOWN: c_int = 4;
const SET_EXISTING: c_int = 5;
const ATTRIBUTE_SET: c_int = 8;
const ATTRIBUTE_SET_EXISTING: c_int = 9;
const ALLOW_NONSTRING_KEYS: c_uint = 1;
// Pinned CPython Include/object.h; PyO3 does not expose this flag. The same
// native bit is mirrored by the raw runtime's instance-layout guard.
const PY_TPFLAGS_INLINE_VALUES: std::ffi::c_ulong = 1 << 2;

const MODULE_POLICY: usize = 0;
const STORAGE_POLICY: usize = 1;
const ACTUAL_TYPE_WEAKREF: usize = 2;
/// Shared ABI4 dictionary-policy view. The native caller owns the actual
/// dictionary; existing is borrowed and out.owner transfers one metadata
/// reference. Neither view is an instance/dictionary/code ownership grant.
#[repr(C)]
pub(crate) struct RawPySoacInstanceDictPolicy {
    pub(crate) owner: *mut ffi::PyObject,
    pub(crate) validate: Option<InstanceDictionaryValidator>,
}

pub(crate) type InstanceDictionaryValidator = unsafe extern "C" fn(
    *mut ffi::PyObject,
    *mut ffi::PyObject,
    *mut ffi::PyObject,
    *mut ffi::PyObject,
    c_int,
    *mut ffi::PyObject,
) -> c_int;

/// Opaque native GC object. Only an owned NewV1 result can be transferred to
/// the actual allocation factory; a raw pointer is never storage authority.
#[repr(C)]
struct RawPyTypeState {
    _private: [u8; 0],
}

type TypeStateFieldValidator =
    unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject, *mut ffi::PyObject) -> c_int;

type StorageStateFactory =
    unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject, *mut *mut RawPyTypeState) -> c_int;

#[repr(C)]
struct RawPyTypeStateSlotSpecV1 {
    expected_class_owner: *mut ffi::PyObject,
    field_index: ffi::Py_ssize_t,
    offset: ffi::Py_ssize_t,
    canonical_name: *mut ffi::PyObject,
    rule_owner: *mut ffi::PyObject,
    validate: Option<TypeStateFieldValidator>,
}

#[repr(C)]
struct RawPyTypeStateSpecV1 {
    abi_version: u32,
    struct_size: u32,
    dictionary_owner: *mut ffi::PyObject,
    validate_dictionary: Option<InstanceDictionaryValidator>,
    validate_inline: Option<TypeStateFieldValidator>,
    slot_count: ffi::Py_ssize_t,
    slots: *const RawPyTypeStateSlotSpecV1,
}

unsafe extern "C" {
    fn PyType_SetSoacStorageStateFactoryV1(
        actual_type: *mut ffi::PyObject,
        expected_owner: *mut ffi::PyObject,
        prepare: StorageStateFactory,
    ) -> c_int;
    fn PyTypeState_NewV1(
        actual_type: *mut ffi::PyObject,
        spec: *const RawPyTypeStateSpecV1,
        spec_size: usize,
    ) -> *mut RawPyTypeState;
    fn PyType_GetSoacConstructionInfoV1(
        class: *mut ffi::PyObject,
        out: *mut RawSoacTypeConstructionInfo,
        size: usize,
    ) -> c_int;
    fn PyType_FailSoacPendingV1(root_construction: *mut ffi::PyObject) -> c_int;
    fn PyType_DisposeSoacProvisionalV1(
        class: *mut ffi::PyObject,
        expected_owner: *mut ffi::PyObject,
        expected_root_construction: *mut ffi::PyObject,
    ) -> c_int;
    fn PyType_GetSoacContractOwner(class: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyType_SealSoacContract(class: *mut ffi::PyObject, owner: *mut ffi::PyObject) -> c_int;
    fn PyType_IsSoacSealed(class: *mut ffi::PyObject) -> c_int;
    fn PyType_GetSoacObjectSlotOffset(
        class: *mut ffi::PyObject,
        expected_owner: *mut ffi::PyObject,
        field_index: ffi::Py_ssize_t,
        offset: *mut ffi::Py_ssize_t,
    ) -> c_int;
    fn PyType_MatchesSoacObjectSlotDescriptor(
        class: *mut ffi::PyObject,
        expected_owner: *mut ffi::PyObject,
        field_index: ffi::Py_ssize_t,
        descriptor: *mut ffi::PyObject,
    ) -> c_int;
    fn PyDict_SetSoacPolicy(
        dictionary: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
        validate: InstanceDictionaryValidator,
        flags: c_uint,
    ) -> c_int;
    fn PyDict_MatchesSoacPolicy(
        dictionary: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
        validate: InstanceDictionaryValidator,
        flags: c_uint,
    ) -> c_int;
    fn PyDict_MatchesSoacClassNamespace(
        dictionary: *mut ffi::PyObject,
        expected_owner: *mut ffi::PyObject,
    ) -> c_int;
    fn _PyObject_GetDictPtr(object: *mut ffi::PyObject) -> *mut *mut ffi::PyObject;
    fn PyType_GetDict(class: *mut ffi::PyTypeObject) -> *mut ffi::PyObject;
    fn _PyDict_NewIndexedKeySet(names: *mut ffi::PyObject) -> *mut c_void;
    fn _PyDict_NewWithIndexedKeySet(keys: *mut c_void) -> *mut ffi::PyObject;
    fn _PyDictKeys_DecRef(keys: *mut c_void);
    fn _PyDict_NewFromIndexedSchema(template: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn _PyDict_IndexedKeyIndex(
        dictionary: *mut ffi::PyObject,
        name: *mut ffi::PyObject,
    ) -> ffi::Py_ssize_t;
    fn _PyDict_GetIndexedItem(
        dictionary: *mut ffi::PyObject,
        index: ffi::Py_ssize_t,
        result: *mut *mut ffi::PyObject,
    ) -> c_int;
    fn _PySoac_IsLayoutDescriptor(
        name: *mut ffi::PyObject,
        descriptor: *mut ffi::PyObject,
    ) -> c_int;
    fn PySoac_GetStrictMutationError() -> *mut ffi::PyObject;
    static mut PyStaticMethod_Type: ffi::PyTypeObject;
    static mut PyClassMethod_Type: ffi::PyTypeObject;
    static mut PyProperty_Type: ffi::PyTypeObject;
    static mut PyGetSetDescr_Type: ffi::PyTypeObject;
    static mut PyMemberDescr_Type: ffi::PyTypeObject;
}

/// The storage model is selected from the actual backend before native type
/// construction. Ordinary storage is never an indexed capability or template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageModel {
    Indexed,
    Ordinary,
}

/// One physical prefix or ordinary name set and its logical requirements. These source
/// types are not runtime write authority: the storage owner separately retains
/// every actual FieldChecks owner, including equal source types from distinct
/// class-factory executions. Values live only in the actual dictionary.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StorageLayout {
    fields: Vec<String>,
    object_fields: Vec<String>,
    dictionary_bearing: bool,
    declared_slots: bool,
    checks: BTreeMap<String, Vec<StaticType>>,
}

impl Default for StorageLayout {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            object_fields: Vec::new(),
            dictionary_bearing: true,
            declared_slots: false,
            checks: BTreeMap::new(),
        }
    }
}

impl StorageLayout {
    fn merge<'a>(
        model: StorageModel,
        bases: impl IntoIterator<Item = &'a Self>,
    ) -> Result<Self, DynamicClassReason> {
        let mut merged = Self::default();
        for base in bases {
            if model == StorageModel::Indexed {
                if merged
                    .fields
                    .iter()
                    .zip(&base.fields)
                    .any(|(left, right)| left != right)
                {
                    return Err(DynamicClassReason::ConflictingLayout);
                }
                if base.fields.len() > merged.fields.len() {
                    merged
                        .fields
                        .extend_from_slice(&base.fields[merged.fields.len()..]);
                }
            } else {
                for name in &base.fields {
                    merged.append(name);
                }
            }
            for name in &base.object_fields {
                if !merged.object_fields.contains(name) {
                    merged.object_fields.push(name.clone());
                }
            }
            for (name, requirements) in &base.checks {
                let combined = merged.checks.entry(name.clone()).or_default();
                for requirement in requirements {
                    if !combined.contains(requirement) {
                        combined.push(requirement.clone());
                    }
                }
            }
        }
        Ok(merged)
    }

    fn append(&mut self, name: &str) {
        if !self.fields.iter().any(|existing| existing == name) {
            self.fields.push(name.to_owned());
        }
    }
}

#[derive(Clone, Debug, Default)]
struct NamePolicy {
    protected: BTreeSet<String>,
    final_methods: BTreeSet<String>,
}

/// Rust-only data. Indexed storage alone has a template. All remaining GC
/// edges are the actual field-check owners. Neither storage nor those check owners retain a receiver
/// or module. Required nominal target types, including direct self when selected,
/// are retained only in the corresponding field-check owner's traversed vector.
struct StoragePolicyData {
    interpreter_id: i64,
    layout: StorageLayout,
    model: StorageModel,
    template: Option<usize>,
    // Entry i selects dictionary fields for check_reference(i). The same
    // check still applies to a canonical native member with that field name.
    // A new slot can shadow an inherited dictionary prefix without making
    // its own predicate a requirement on the hidden mapping entry.
    dictionary_fields_by_check: Vec<BTreeSet<String>>,
}

impl StoragePolicyData {
    fn check_reference(&self, index: usize) -> usize {
        index + usize::from(self.template.is_some())
    }

    fn dictionary_mode(&self) -> u32 {
        if !self.layout.dictionary_bearing {
            return 0;
        }
        match self.model {
            StorageModel::Indexed => 1,
            StorageModel::Ordinary
                if self
                    .dictionary_fields_by_check
                    .iter()
                    .any(|fields| !fields.is_empty()) =>
            {
                2
            }
            StorageModel::Ordinary => 0,
        }
    }
}

// SAFETY: Neither the immutable layout nor the interpreter ID owns Python
// references. Template/check owners are retained only by the traversed vector.
unsafe impl StrictStateData for StoragePolicyData {
    const TYPE_NAME: &'static CStr = c"soac._StrictStoragePolicy";
}

type StoragePolicyOwner<'py> = StrictStateRef<'py, StoragePolicyData>;

/// Temporary preparation data. Only the check owner goes into the GC shell;
/// the per-storage field selection is copied into its immutable Rust payload.
struct SelectedStorageCheck<'py> {
    check: StrictFieldChecks<'py>,
    dictionary_fields: BTreeSet<String>,
}

impl<'py> SelectedStorageCheck<'py> {
    fn own(check: StrictFieldChecks<'py>, layout: &StorageLayout) -> Self {
        let dictionary_fields = layout
            .fields
            .iter()
            .filter(|name| !layout.object_fields.contains(name) && check.contains_field(name))
            .cloned()
            .collect();
        Self {
            check,
            dictionary_fields,
        }
    }

    fn owner(&self) -> &Bound<'py, PyAny> {
        self.check.owner()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClassPhase {
    Prepared,
    Pending,
    Admitting,
    Bound,
    Sealed,
    Discarded,
    Terminal,
}

#[repr(C)]
struct RawSoacTypeConstructionInfo {
    abi_version: u32,
    struct_size: u32,
    phase: u32,
    permanent_contract_published: u32,
    owner: *mut ffi::PyObject,
    root_construction: *mut ffi::PyObject,
}

const NATIVE_TYPE_PENDING: u32 = 1;
const NATIVE_TYPE_ADMITTING: u32 = 2;
const NATIVE_TYPE_ENFORCED: u32 = 3;
const NATIVE_TYPE_FAILED: u32 = 4;

/// The actual type operand supports every returned pointer. No callback or
/// allocation may intervene before their use; never retain these as owners.
fn native_construction_info(
    py: Python<'_>,
    class: &Bound<'_, PyAny>,
) -> PyResult<Option<RawSoacTypeConstructionInfo>> {
    let mut info = RawSoacTypeConstructionInfo {
        abi_version: 0,
        struct_size: 0,
        phase: 0,
        permanent_contract_published: 0,
        owner: ptr::null_mut(),
        root_construction: ptr::null_mut(),
    };
    match unsafe {
        PyType_GetSoacConstructionInfoV1(
            class.as_ptr(),
            &mut info,
            std::mem::size_of::<RawSoacTypeConstructionInfo>(),
        )
    } {
        0 => Ok(None),
        1 if info.abi_version == 1
            && info.struct_size as usize == std::mem::size_of::<RawSoacTypeConstructionInfo>() =>
        {
            Ok(Some(info))
        }
        1 => Err(strict_runtime_unavailable(
            py,
            "native type construction info ABI differs",
        )),
        _ => Err(PyErr::fetch(py)),
    }
}

/// One actual type construction, independent of the source namespace body.
/// A recognized replacement reuses declaring-source provenance but must never
/// reuse the original type's physical-layout or method-locator witness.
struct ActualClassConstruction;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClassConstructionKind {
    SourceNamespace,
    DataclassSlotsReplacement,
}

struct StrictClassData {
    verified: Arc<VerifiedStrictModule>,
    execution: StrictModuleExecutionRef,
    fact: ClassTypeFact,
    names: NamePolicy,
    phase: Cell<ClassPhase>,
    // Comparison only. A dictionary may outlive its former class; the GC
    // policy owner must not keep that class alive through a reverse edge.
    actual_type: Cell<usize>,
    construction: Arc<ActualClassConstruction>,
    construction_kind: ClassConstructionKind,
    // Reserved before native construction. Source Pending types bind these
    // once at final admission, on either backend.
    // Never guessed from annotations, profiles, or dictionary indices.
    object_offsets: Vec<Cell<ffi::Py_ssize_t>>,
    namespace_execution: Arc<crate::strict_namespace::NamespaceExecution>,
    // Native source construction queues the real type before callbacks. This
    // Rust-only identity also prevents optional JIT publication on this lane.
    interpreter_invocation: OnceLock<Arc<crate::strict_interpreter::InterpreterInvocationIdentity>>,
    // Only own declaration snapshots. Inherited consumers select their
    // actual declaring base through the live native MRO, not a source registry.
    own_field_bindings: Vec<usize>,
    // An index in STORAGE_POLICY's GC vector, not another owning edge. Only
    // this construction's policy may bind direct self; inherited owners stay
    // attached to their original actual types.
    own_field_checks: Option<usize>,
    // One temporary traversed invocation edge becomes weak member witnesses
    // after actual Apply. This never confers source-function/JIT ownership.
    dataclass: Option<DataclassClassState>,
    // One-way optional dispatch publication. These rows contain Rust-only
    // identities/locators, never extra type/function/default Python edges.
    method_families: OnceLock<SealedMethodFamilies>,
}

// SAFETY: Authenticated facts and the execution reference contain Rust data
// only. All Python edges, including the original actual-type weak witness, are
// GC-traversed. A recorded type address or exposed owner alone is not authority.
unsafe impl StrictStateData for StrictClassData {
    const TYPE_NAME: &'static CStr = c"soac._StrictClassState";

    fn on_terminal(&self) {
        let native_pending = self.interpreter_invocation.get().is_some()
            && matches!(
                self.phase.get(),
                ClassPhase::Pending | ClassPhase::Admitting
            );
        self.phase.set(ClassPhase::Terminal);
        self.actual_type.set(0);
        if self.construction_kind == ClassConstructionKind::SourceNamespace {
            self.namespace_execution.invalidate_class_dictionary();
        }
        // A slots transformation may naturally release its unselected native
        // original before the real final CALL result arrives. Its own metadata
        // retirement must not fail the linked replacement. Native lineage and
        // the exact active adapter own failure; a cleared actual owner still
        // fails every subsequent private-state/actual-type authentication.
        if !native_pending && let Some(dataclass) = &self.dataclass {
            dataclass.fail();
        }
    }
}

/// The selected read still falls back to ordinary getattr when the instance
/// slot is UNSET. A class default supplies a precedence guard, not a copied
/// value or a checked-result proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SealedFieldPrecedence {
    NoClassBinding,
    GuardPlainClassBinding {
        mro_index: u32,
        namespace_index: u32,
    },
}

/// The common actual construction witness. It owns no Python object and must
/// be matched against a live native type before any recorded locator is used.
/// The expected address belongs to each ABI descriptor, not a duplicated slot
/// here. Native owner authentication also checks its original type's weak
/// witness, so reattaching an exposed owner at a reused address cannot recover
/// authority. The construction Arc separately distinguishes a recognized
/// replacement sharing the original declaring namespace execution.
struct SealedClassWitness {
    execution: Arc<crate::strict_namespace::NamespaceExecution>,
    construction: Arc<ActualClassConstruction>,
    interpreter_id: i64,
}

impl SealedClassWitness {
    fn matches_receiver(
        &self,
        receiver: &Bound<'_, PyAny>,
        expected_type: usize,
    ) -> PyResult<bool> {
        let py = receiver.py();
        require_interpreter(py, self.interpreter_id)?;
        let actual = unsafe { ffi::Py_TYPE(receiver.as_ptr()) };
        if actual as usize != expected_type {
            return Ok(false);
        }
        let actual = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, actual.cast()) };
        let Some(state) = for_actual_type(py, &actual)? else {
            return Ok(false);
        };
        if !state.is_finalized()
            || !Arc::ptr_eq(state.namespace_execution(), &self.execution)
            || !Arc::ptr_eq(&state.state.data().construction, &self.construction)
        {
            return Ok(false);
        }
        if !mro_is_sealed(&actual)? {
            return Err(strict_runtime_unavailable(
                py,
                "sealed member capability lost its permanent MRO contract",
            ));
        }
        Ok(true)
    }
}

/// Immutable ABI-shaped prefix read mechanically by generated code, only
/// after a successful capability match. This layout alone is not authority.
#[repr(usize)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SealedFieldStorageKind {
    IndexedDictionary,
    NativeObjectMember,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawSealedFieldLayout {
    pub(crate) expected_type: usize,
    pub(crate) storage_kind: SealedFieldStorageKind,
    pub(crate) field_index: usize,
    pub(crate) object_offset: usize,
    pub(crate) default_mro_index: isize,
    pub(crate) default_namespace_index: isize,
}

/// A runtime construction witness, never a deserializable source/profile fact.
///
/// The Arc keeps only a Rust execution identity alive. The two Python addresses
/// are comparison operands, not owning or independently dereferenceable refs:
/// the actual receiver, native owner, and execution identity must first match.
/// In particular, retaining compiled code cannot keep a dead class or any of
/// its defaults, methods, globals, or storage owners alive through this object.
#[repr(C)]
pub(crate) struct SealedFieldCapability {
    layout: RawSealedFieldLayout,
    witness: SealedClassWitness,
    storage_owner: usize,
    field_name: String,
}

impl SealedFieldCapability {
    pub(crate) const EXPECTED_TYPE_OFFSET: usize = std::mem::offset_of!(Self, layout)
        + std::mem::offset_of!(RawSealedFieldLayout, expected_type);
    pub(crate) const FIELD_INDEX_OFFSET: usize = std::mem::offset_of!(Self, layout)
        + std::mem::offset_of!(RawSealedFieldLayout, field_index);
    pub(crate) const STORAGE_KIND_OFFSET: usize = std::mem::offset_of!(Self, layout)
        + std::mem::offset_of!(RawSealedFieldLayout, storage_kind);
    pub(crate) const OBJECT_OFFSET_OFFSET: usize = std::mem::offset_of!(Self, layout)
        + std::mem::offset_of!(RawSealedFieldLayout, object_offset);
    pub(crate) const DEFAULT_MRO_INDEX_OFFSET: usize = std::mem::offset_of!(Self, layout)
        + std::mem::offset_of!(RawSealedFieldLayout, default_mro_index);
    pub(crate) const DEFAULT_NAMESPACE_INDEX_OFFSET: usize = std::mem::offset_of!(Self, layout)
        + std::mem::offset_of!(RawSealedFieldLayout, default_namespace_index);

    pub(crate) fn source(&self) -> &SourceIdentity {
        self.witness.execution.source()
    }

    pub(crate) fn field_name(&self) -> &str {
        &self.field_name
    }

    pub(crate) fn field_index(&self) -> u32 {
        self.layout.field_index as u32
    }

    pub(crate) fn storage_kind(&self) -> SealedFieldStorageKind {
        self.layout.storage_kind
    }

    pub(crate) fn precedence(&self) -> SealedFieldPrecedence {
        match (
            self.layout.default_mro_index,
            self.layout.default_namespace_index,
        ) {
            (-1, -1) => SealedFieldPrecedence::NoClassBinding,
            (mro_index, namespace_index) => SealedFieldPrecedence::GuardPlainClassBinding {
                mro_index: mro_index as u32,
                namespace_index: namespace_index as u32,
            },
        }
    }

    /// Authenticate this exact receiver before the no-effect raw probe.
    ///
    /// A normal mismatch (including an ordinary subclass) is a generic-read
    /// fallback. Broken/terminal class authority is an error, not deoptimization.
    /// A dictionary currently in a write transaction also takes the generic
    /// path: its callback may legitimately read the old value, but the permanent
    /// write capability alone is not an optional raw-read proof at that point.
    pub(crate) fn matches_receiver(&self, receiver: &Bound<'_, PyAny>) -> PyResult<bool> {
        let py = receiver.py();
        if !self
            .witness
            .matches_receiver(receiver, self.layout.expected_type)?
        {
            return Ok(false);
        }
        if self.layout.storage_kind == SealedFieldStorageKind::NativeObjectMember {
            // The permanent, exact actual construction binds both the native
            // member representation and its descriptor precedence. Native
            // members have no mutable Python descriptor-type state to guard.
            return Ok(true);
        }
        // Only the separately retained indexed-storage factory disables inline
        // values and eagerly installs its real dictionary. Ordinary optional
        // type state never publishes this indexed capability. GetDictPtr is
        // effect-free only after rechecking that original non-inline layout.
        let actual_pointer = unsafe { ffi::Py_TYPE(receiver.as_ptr()) };
        if !raw_dictionary_location_supported(actual_pointer) {
            return Err(strict_runtime_unavailable(
                py,
                "sealed field capability has incompatible instance storage",
            ));
        }
        let dictionary = unsafe { _PyObject_GetDictPtr(receiver.as_ptr()) };
        if dictionary.is_null() || unsafe { (*dictionary).is_null() } {
            return Ok(false);
        }
        // Pass the recorded address only to the native identity predicate. It
        // checks the live owned edge; never reconstruct a Rust/Python owner
        // reference from this non-owning address (which could be stale).
        Ok(unsafe {
            PyDict_MatchesSoacPolicy(
                *dictionary,
                self.storage_owner as *mut ffi::PyObject,
                validate_instance_dictionary,
                ALLOW_NONSTRING_KEYS,
            ) == 1
        })
    }
}

const _: () = assert!(std::mem::offset_of!(SealedFieldCapability, layout) == 0);

/// A plain, protected instance method of one actual sealed receiver class.
/// It permits lookup/binding elimination only. Argument evaluation, binding,
/// default liveness, and required checks still belong to its public call entry.
/// All addresses are comparison operands; the live receiver owns the MRO and
/// its frozen namespace owns the resolved function. No such edge is duplicated
/// here, even when compiled code outlives a factory-created class.
pub(crate) struct SealedMethodCapability {
    witness: SealedClassWitness,
    declaring_execution: Arc<crate::strict_namespace::NamespaceExecution>,
    declaring_construction: Arc<ActualClassConstruction>,
    expected_type: usize,
    function_identity: usize,
    mro_index: u32,
    namespace_index: u32,
    name: String,
    method_source: SourceIdentity,
}

impl SealedMethodCapability {
    pub(crate) fn source(&self) -> &SourceIdentity {
        self.witness.execution.source()
    }

    pub(crate) fn method_source(&self) -> &SourceIdentity {
        &self.method_source
    }

    pub(crate) fn method_name(&self) -> &str {
        &self.name
    }

    /// Recover only the actual frozen binding, never execute or preflight a
    /// call here. In particular, a keyword-default liveness error must remain
    /// after argument evaluation at the public function entry, not move into
    /// a preceding attribute lookup.
    pub(crate) fn resolve<'py>(
        &self,
        receiver: &Bound<'py, PyAny>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        if !self
            .witness
            .matches_receiver(receiver, self.expected_type)?
        {
            return Ok(None);
        }
        let py = receiver.py();
        let actual = unsafe {
            Bound::<PyAny>::from_borrowed_ptr(py, ffi::Py_TYPE(receiver.as_ptr()).cast())
        };
        let mro = actual_mro(&actual)?;
        let declaring = mro.get_item(self.mro_index as usize)?;
        let Some(state) = for_actual_type(py, &declaring)? else {
            return Err(strict_runtime_unavailable(
                py,
                "sealed method lost its declaring owner",
            ));
        };
        if !Arc::ptr_eq(state.namespace_execution(), &self.declaring_execution)
            || !Arc::ptr_eq(
                &state.state.data().construction,
                &self.declaring_construction,
            )
        {
            return Err(strict_runtime_unavailable(
                py,
                "sealed method declaring execution changed",
            ));
        }
        let namespace = unsafe { (*declaring.as_ptr().cast::<ffi::PyTypeObject>()).tp_dict };
        if namespace.is_null() || unsafe { ffi::PyDict_CheckExact(namespace) } == 0 {
            return Err(strict_runtime_unavailable(
                py,
                "sealed method has no actual namespace",
            ));
        }
        let mut function = ptr::null_mut();
        let found = unsafe {
            _PyDict_GetIndexedItem(namespace, self.namespace_index as isize, &mut function)
        };
        if found < 0 {
            return Err(PyErr::fetch(py));
        }
        if found == 0 || function.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "sealed method binding disappeared",
            ));
        }
        let function = unsafe { Bound::<PyAny>::from_owned_ptr(py, function) };
        if function.as_ptr() as usize != self.function_identity
            || unsafe { ffi::PyFunction_Check(function.as_ptr()) } == 0
        {
            return Err(strict_runtime_unavailable(
                py,
                "sealed method function identity changed",
            ));
        }
        Ok(Some(function))
    }
}

/// Canonical slots belong to one actual class construction, not its source
/// name or a process-wide profile identifier. Holding a family cannot keep
/// any Python member of that construction alive.
struct SealedMethodFamily {
    execution: Arc<crate::strict_namespace::NamespaceExecution>,
    interpreter_id: i64,
    source_digest: Fingerprint,
    names: Box<[String]>,
}

struct SealedMethodFamilyRow {
    family: Arc<SealedMethodFamily>,
    // Targets are rebuilt from this receiver's sealed MRO. Copying the base's
    // exact-receiver capability would either miss every derived receiver or
    // silently dispatch to an overridden implementation.
    targets: Box<[Option<Arc<SealedMethodCapability>>]>,
}

struct SealedMethodFamilies {
    own: Arc<SealedMethodFamily>,
    // Keys are coordinates of retained Arcs; the resolver still compares the
    // actual family Arc before using a row. No address is dereferenced here.
    rows: BTreeMap<usize, SealedMethodFamilyRow>,
}

/// A virtual method request admitted by an actual sealed ancestor. Each live
/// strict receiver must independently supply a row for this exact family.
/// Missing rows/targets (including a declared shadowable field) are ordinary
/// lookup misses, not reasons to weaken a class or revise an existing row.
pub(crate) struct SealedVirtualMethodCapability {
    family: Arc<SealedMethodFamily>,
    slot: usize,
}

impl SealedVirtualMethodCapability {
    fn for_family(family: &Arc<SealedMethodFamily>, name: &str) -> Option<Self> {
        let slot = family
            .names
            .binary_search_by(|candidate| candidate.as_str().cmp(name))
            .ok()?;
        Some(Self {
            family: Arc::clone(family),
            slot,
        })
    }

    pub(crate) fn source(&self) -> &SourceIdentity {
        self.family.execution.source()
    }

    pub(crate) fn method_name(&self) -> &str {
        &self.family.names[self.slot]
    }

    pub(crate) fn resolve<'py>(
        &self,
        receiver: &Bound<'py, PyAny>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let py = receiver.py();
        require_interpreter(py, self.family.interpreter_id)?;
        let actual = unsafe {
            Bound::<PyAny>::from_borrowed_ptr(py, ffi::Py_TYPE(receiver.as_ptr()).cast())
        };
        let Some(state) = for_actual_type(py, &actual)? else {
            return Ok(None);
        };
        state.ensure_live()?;
        if !state.is_finalized() {
            return Ok(None);
        }
        let Some(families) = state.state.data().method_families.get() else {
            return Ok(None);
        };
        let Some(row) = families.rows.get(&(Arc::as_ptr(&self.family) as usize)) else {
            return Ok(None);
        };
        if !Arc::ptr_eq(&row.family, &self.family) {
            return Err(strict_runtime_unavailable(
                py,
                "sealed method family row changed construction identity",
            ));
        }
        let target = row.targets.get(self.slot).ok_or_else(|| {
            strict_runtime_unavailable(py, "sealed method family row has no declared slot")
        })?;
        let Some(target) = target else {
            return Ok(None);
        };
        // Reuse the exact receiver/declarer/namespace/function kernel. It has
        // no user-code effect between owner validation and the stable locator.
        target.resolve(receiver)
    }
}

/// Borrowed lookup output, not a bound method or an unchecked-body token.
/// The caller INCREFs `callee` before evaluating arguments, then compares its
/// current public vectorcall with `entry` immediately before invocation. A
/// changed public pointer uses normal dispatch on that same captured callee.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct RawSealedMethodTarget {
    pub(crate) callee: *mut ffi::PyObject,
    pub(crate) entry: Option<ffi::vectorcallfunc>,
}

impl RawSealedMethodTarget {
    pub(crate) const CALLEE_OFFSET: usize = std::mem::offset_of!(Self, callee);
    pub(crate) const ENTRY_OFFSET: usize = std::mem::offset_of!(Self, entry);

    fn empty() -> Self {
        Self {
            callee: ptr::null_mut(),
            entry: None,
        }
    }
}

pub(crate) struct StrictClassState<'py> {
    state: StrictStateRef<'py, StrictClassData>,
    // A per-use view, never stored in the GC owner or published capability.
    // Prepared states and native instance callbacks need only owner data;
    // for_actual_type supplies this guard for layout/seal/nominal operations.
    actual_type: Option<Bound<'py, PyAny>>,
}

impl<'py> StrictClassState<'py> {
    pub(crate) fn owner(&self) -> &Bound<'py, PyAny> {
        self.state.owner()
    }

    pub(crate) fn source(&self) -> &SourceIdentity {
        &self.state.data().fact.identity
    }

    pub(crate) fn fact(&self) -> &ClassTypeFact {
        &self.state.data().fact
    }

    pub(crate) fn verified_module(&self) -> &Arc<VerifiedStrictModule> {
        &self.state.data().verified
    }

    pub(crate) fn execution_ref(&self) -> &StrictModuleExecutionRef {
        &self.state.data().execution
    }

    pub(crate) fn namespace_execution(&self) -> &Arc<crate::strict_namespace::NamespaceExecution> {
        &self.state.data().namespace_execution
    }

    pub(crate) fn is_interpreter_construction(&self) -> bool {
        self.state.data().interpreter_invocation.get().is_some()
    }

    pub(crate) fn matches_interpreter_completion(
        &self,
        invocation: &Arc<crate::strict_interpreter::InterpreterInvocationIdentity>,
    ) -> bool {
        self.state
            .data()
            .interpreter_invocation
            .get()
            .is_some_and(|actual| Arc::ptr_eq(actual, invocation))
    }

    pub(crate) fn select_interpreter_completion(
        &self,
        invocation: &Arc<crate::strict_interpreter::InterpreterInvocationIdentity>,
    ) -> PyResult<()> {
        self.ensure_live()?;
        if self.state.data().phase.get() != ClassPhase::Prepared {
            return Err(strict_runtime_unavailable(
                self.owner().py(),
                "native class completion must be selected before construction",
            ));
        }
        self.state
            .data()
            .interpreter_invocation
            .set(invocation.clone())
            .map_err(|_| {
                strict_runtime_unavailable(
                    self.owner().py(),
                    "native class completion was already selected",
                )
            })
    }

    pub(crate) fn is_finalized(&self) -> bool {
        self.state.data().phase.get() == ClassPhase::Sealed
    }

    pub(crate) fn is_pending_type(&self) -> bool {
        self.state.data().phase.get() == ClassPhase::Pending
    }

    /// Only the actual final decorated operand selects direct-self and field
    /// nominal targets. A slots replacement must not inherit the provisional
    /// original's identity merely because it reuses its logical declarations.
    pub(crate) fn bind_final_type_requirements(&self) -> PyResult<()> {
        self.ensure_live()?;
        if !self.is_pending_type() {
            return Err(strict_runtime_unavailable(
                self.owner().py(),
                "final nominal binding requires a pending actual type",
            ));
        }
        let actual = self.actual_type()?;
        for &index in &self.state.data().own_field_bindings {
            StrictFieldBinding::from_owner(self.state.reference(index)?)?
                .bind_actual_class(&actual)?;
        }
        if let Some(index) = self.state.data().own_field_checks {
            let storage = self.storage()?;
            StrictFieldChecks::from_owner(storage.reference(index)?)?.bind_actual_class(&actual)?;
        }
        Ok(())
    }

    /// All allocation/callback-capable preparation must precede this call.
    /// The returned temporary pins metadata, not the provisional/final type.
    /// The caller immediately enters the recorded native admission operation.
    pub(crate) fn begin_pending_admission(&self) -> PyResult<Bound<'py, PyAny>> {
        self.ensure_live()?;
        let py = self.owner().py();
        let actual = self.actual_type()?;
        let info = native_construction_info(py, &actual)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "final admission has no actual native construction")
        })?;
        if self.state.data().phase.get() != ClassPhase::Pending
            || info.phase != NATIVE_TYPE_PENDING
            || info.permanent_contract_published != 0
            || info.owner != self.owner().as_ptr()
            || info.root_construction.is_null()
            || self.pending_dataclass()
        {
            return Err(strict_runtime_unavailable(
                py,
                "final type is not ready for native admission",
            ));
        }
        let root = unsafe { Bound::from_borrowed_ptr(py, info.root_construction) };
        self.state.data().phase.set(ClassPhase::Admitting);
        Ok(root)
    }

    /// An invocation may retire an unselected provisional only after the same
    /// native lineage selected and enforced its final type. This never admits
    /// a class merely because it remained in the weak pending inventory.
    pub(crate) fn dispose_unselected_provisional(&self) -> PyResult<()> {
        self.ensure_live()?;
        let py = self.owner().py();
        let actual = self.actual_type()?;
        let info = native_construction_info(py, &actual)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "provisional disposition lost native construction")
        })?;
        if self.state.data().phase.get() != ClassPhase::Pending
            || info.phase != NATIVE_TYPE_PENDING
            || info.permanent_contract_published != 0
            || info.owner != self.owner().as_ptr()
            || info.root_construction.is_null()
        {
            return Err(strict_runtime_unavailable(
                py,
                "only an uncommitted provisional can be disposed",
            ));
        }
        // Native validates resolved lineage/selection before any effect and
        // publishes DYNAMIC before releasing its metadata. This temporary
        // self/actual view supports the Rust state across that release.
        if unsafe {
            PyType_DisposeSoacProvisionalV1(
                actual.as_ptr(),
                self.owner().as_ptr(),
                info.root_construction,
            )
        } < 0
        {
            return Err(PyErr::fetch(py));
        }
        self.state.data().phase.set(ClassPhase::Discarded);
        Ok(())
    }

    /// Failure is a scalar native lineage transition. Published types and
    /// independently sealed functions remain enforced; no contract is revoked.
    pub(crate) fn fail_unfinished_type(&self) -> PyResult<()> {
        if !matches!(
            self.state.data().phase.get(),
            ClassPhase::Pending | ClassPhase::Admitting
        ) {
            return Ok(());
        }
        let py = self.owner().py();
        let actual = bound_type_witness(&self.state)?;
        let Some(info) = native_construction_info(py, &actual)? else {
            return Ok(());
        };
        if info.owner != self.owner().as_ptr() || info.root_construction.is_null() {
            return Ok(());
        }
        if unsafe { PyType_FailSoacPendingV1(info.root_construction) } < 0 {
            return Err(PyErr::fetch(py));
        }
        self.state.data().phase.set(ClassPhase::Terminal);
        Ok(())
    }

    pub(crate) fn dataclass_namespace(&self) -> PyResult<Option<DataclassNamespace<'py>>> {
        dataclass_namespace(&self.state)
    }

    pub(crate) fn pending_dataclass(&self) -> bool {
        self.state
            .data()
            .dataclass
            .as_ref()
            .is_some_and(DataclassClassState::pending)
    }

    pub(crate) fn matches_active_dataclass_owner(
        &self,
        owner: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        self.ensure_live()?;
        let Some(dataclass) = &self.state.data().dataclass else {
            return Ok(false);
        };
        Ok(dataclass.pending()
            && self.state.reference(dataclass.reference)?.as_ptr() == owner.as_ptr())
    }

    /// Publish one prevalidated, permanently adopted component set. A slots
    /// adapter prepares both original/replacement owners before committing
    /// either, and pins the active invocation across these callback-free edge
    /// swaps. This never transfers the other class's layout authority.
    pub(crate) fn publish_dataclass_members(
        &self,
        class: &Bound<'py, PyAny>,
        expected_invocation_owner: &Bound<'py, PyAny>,
        adopted: &DataclassAdoptedMembers<'py>,
    ) -> PyResult<()> {
        self.ensure_live()?;
        let py = class.py();
        if self.actual_type()?.as_ptr() != class.as_ptr() {
            return Err(strict_runtime_unavailable(
                py,
                "dataclass member publication has another actual type",
            ));
        }
        let dataclass = self.state.data().dataclass.as_ref().ok_or_else(|| {
            strict_runtime_unavailable(py, "class has no installed dataclass construction")
        })?;
        let current = self.state.reference(dataclass.reference)?;
        let view = dataclass.namespace(current.clone());
        if current.as_ptr() != expected_invocation_owner.as_ptr() || !adopted.matches(&view) {
            return Err(strict_runtime_unavailable(
                py,
                "dataclass members belong to another active construction",
            ));
        }
        self.state
            .set_reference(dataclass.reference, adopted.owner().clone())
    }

    /// A temporary owning view for construction/finalization and actual
    /// nominal binding. This is not a published layout or dispatch capability:
    /// the type may still be Bound, and callers must not retain this edge in
    /// unrelated runtime metadata merely to recover it by source name.
    pub(crate) fn actual_type(&self) -> PyResult<Bound<'py, PyAny>> {
        self.ensure_live()?;
        if !matches!(
            self.state.data().phase.get(),
            ClassPhase::Pending | ClassPhase::Admitting | ClassPhase::Bound | ClassPhase::Sealed
        ) {
            return Err(strict_runtime_unavailable(
                self.owner().py(),
                "strict class has not bound its actual type",
            ));
        }
        let actual = self.actual_type.as_ref().ok_or_else(|| {
            strict_runtime_unavailable(
                self.owner().py(),
                "strict class operation requires a pinned actual type",
            )
        })?;
        if actual.as_ptr() as usize != self.state.data().actual_type.get() {
            return Err(strict_runtime_unavailable(
                self.owner().py(),
                "strict class actual type identity changed",
            ));
        }
        require_actual_owner(self.owner().py(), actual, self.owner())?;
        Ok(actual.clone())
    }

    fn ensure_live(&self) -> PyResult<()> {
        self.state.ensure_live()?;
        let py = self.owner().py();
        if matches!(
            self.state.data().phase.get(),
            ClassPhase::Discarded | ClassPhase::Terminal
        ) {
            return Err(strict_runtime_unavailable(
                py,
                "strict class construction is no longer live",
            ));
        }
        require_interpreter(py, self.verified_module().interpreter_id())?;
        if matches!(
            self.state.data().phase.get(),
            ClassPhase::Pending | ClassPhase::Admitting | ClassPhase::Bound | ClassPhase::Sealed
        ) {
            let actual = bound_type_witness(&self.state)?;
            if actual.as_ptr() as usize != self.state.data().actual_type.get() {
                return Err(strict_runtime_unavailable(
                    py,
                    "strict class construction identity changed",
                ));
            }
            require_actual_owner(py, &actual, self.owner())?;
        }
        // A sealed methodless class can outlive the module globals. Its
        // permanent native contract is self-contained; keeping globals alive
        // or treating their later teardown as class revocation is incorrect.
        if !self.is_finalized() {
            self.state.data().execution.validate_owner(
                py,
                &self.state.reference(MODULE_POLICY)?,
                self.verified_module(),
            )?;
        }
        Ok(())
    }

    fn storage(&self) -> PyResult<StoragePolicyOwner<'py>> {
        StoragePolicyOwner::from_owner(self.state.reference(STORAGE_POLICY)?)
    }

    pub(crate) fn fields(&self) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_live()?;
        PyTuple::new(self.owner().py(), &self.storage()?.data().layout.fields)
    }

    pub(crate) fn object_fields(&self) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_live()?;
        PyTuple::new(
            self.owner().py(),
            &self.storage()?.data().layout.object_fields,
        )
    }

    pub(crate) fn dictionary_bearing(&self) -> PyResult<bool> {
        self.ensure_live()?;
        Ok(self.storage()?.data().layout.dictionary_bearing)
    }

    pub(crate) fn dictionary_mode(&self) -> PyResult<u32> {
        self.ensure_live()?;
        Ok(self.storage()?.data().dictionary_mode())
    }

    pub(crate) fn protected_names(&self) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_live()?;
        PyTuple::new(self.owner().py(), &self.state.data().names.protected)
    }

    pub(crate) fn final_methods(&self) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_live()?;
        PyTuple::new(self.owner().py(), &self.state.data().names.final_methods)
    }

    fn sealed_witness(&self) -> PyResult<Option<(SealedClassWitness, Bound<'py, PyAny>)>> {
        self.ensure_live()?;
        if !self.is_finalized() || self.is_interpreter_construction() {
            return Ok(None);
        }
        let actual = self.actual_type()?;
        if unsafe { PyType_IsSoacSealed(actual.as_ptr()) } != 1 {
            return Err(strict_runtime_unavailable(
                self.owner().py(),
                "sealed capability publication requires the actual native class seal",
            ));
        }
        if !mro_is_sealed(&actual)? {
            return Ok(None);
        }
        Ok(Some((
            SealedClassWitness {
                execution: Arc::clone(self.namespace_execution()),
                construction: Arc::clone(&self.state.data().construction),
                interpreter_id: self.verified_module().interpreter_id(),
            },
            actual,
        )))
    }

    /// Publish an optional structural load capability only after actual native
    /// sealing. Source types, profile offsets, and class names cannot call a
    /// separate constructor. Mandatory field checks are deliberately not
    /// translated into a checked-result or representation proof here.
    pub(crate) fn sealed_field(&self, name: &str) -> PyResult<Option<SealedFieldCapability>> {
        let Some((witness, actual)) = self.sealed_witness()? else {
            return Ok(None);
        };
        if matches!(
            name,
            "__annotations__" | "__annotations_cache__" | "__annotate__" | "__annotate_func__"
        ) {
            return Ok(None);
        }
        let storage = self.storage()?;
        if let Some(index) = storage
            .data()
            .layout
            .object_fields
            .iter()
            .position(|field| field == name)
        {
            for base in actual_mro(&actual)?.iter() {
                let dictionary = unsafe { (*base.as_ptr().cast::<ffi::PyTypeObject>()).tp_dict };
                if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
                    return Ok(None);
                }
                let dictionary =
                    unsafe { Bound::<PyAny>::from_borrowed_ptr(actual.py(), dictionary) }
                        .cast_into::<PyDict>()?;
                let Some(descriptor) = crate::strict_class::namespace_item(&dictionary, name)?
                else {
                    continue;
                };
                match unsafe {
                    PyType_MatchesSoacObjectSlotDescriptor(
                        actual.as_ptr(),
                        self.owner().as_ptr(),
                        index as ffi::Py_ssize_t,
                        descriptor.as_ptr(),
                    )
                } {
                    -1 => return Err(PyErr::fetch(actual.py())),
                    1 => {
                        let offset = self.state.data().object_offsets[index].get();
                        if offset < 0 {
                            return Err(strict_runtime_unavailable(
                                actual.py(),
                                "sealed native member has no physical binding",
                            ));
                        }
                        return Ok(Some(SealedFieldCapability {
                            layout: RawSealedFieldLayout {
                                expected_type: actual.as_ptr() as usize,
                                storage_kind: SealedFieldStorageKind::NativeObjectMember,
                                field_index: index,
                                object_offset: offset as usize,
                                default_mro_index: -1,
                                default_namespace_index: -1,
                            },
                            witness,
                            storage_owner: 0,
                            field_name: name.to_owned(),
                        }));
                    }
                    _ => return Ok(None),
                }
            }
            return Ok(None);
        }
        if storage.data().model != StorageModel::Indexed
            || storage.data().dictionary_mode() != 1
            || storage.data().template.is_none()
            || !raw_dictionary_location_supported(actual.as_ptr().cast())
        {
            return Ok(None);
        }
        let Some(index) = storage
            .data()
            .layout
            .fields
            .iter()
            .position(|field| field == name)
        else {
            return Ok(None);
        };
        if u32::try_from(index).is_err() || isize::try_from(index).is_err() {
            return Ok(None);
        }
        let Some(precedence) = sealed_field_precedence(&actual, name)? else {
            return Ok(None);
        };
        let (default_mro_index, default_namespace_index) = match precedence {
            SealedFieldPrecedence::NoClassBinding => (-1, -1),
            SealedFieldPrecedence::GuardPlainClassBinding {
                mro_index,
                namespace_index,
            } => (mro_index as isize, namespace_index as isize),
        };
        Ok(Some(SealedFieldCapability {
            layout: RawSealedFieldLayout {
                expected_type: actual.as_ptr() as usize,
                storage_kind: SealedFieldStorageKind::IndexedDictionary,
                field_index: index,
                object_offset: 0,
                default_mro_index,
                default_namespace_index,
            },
            witness,
            storage_owner: storage.owner().as_ptr() as usize,
            field_name: name.to_owned(),
        }))
    }

    /// Resolve an actual protected plain method only after both class and
    /// function adoption. Source membership nominates a binding; the final MRO,
    /// native function owner, and that declaring construction authorize it.
    pub(crate) fn sealed_method(&self, name: &str) -> PyResult<Option<SealedMethodCapability>> {
        let Some((witness, actual)) = self.sealed_witness()? else {
            return Ok(None);
        };
        if !self.state.data().names.protected.contains(name) {
            return Ok(None);
        }
        let Some(method) = self
            .fact()
            .methods
            .iter()
            .find(|method| method.name == name)
        else {
            return Ok(None);
        };
        if method.binding != MethodBinding::Instance {
            return Ok(None);
        }
        let Some(expected) = &method.implementation else {
            return Ok(None);
        };
        let py = self.owner().py();
        for (mro_index, declaring) in actual_mro(&actual)?.iter().enumerate() {
            let namespace = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(py, PyType_GetDict(declaring.as_ptr().cast()))
            }?
            .cast_into::<PyDict>()?;
            let Some((key, function)) = namespace
                .iter()
                .find(|(key, _)| exact_name_matches(key, name))
            else {
                continue;
            };
            if unsafe { ffi::PyFunction_Check(function.as_ptr()) } == 0 {
                return Ok(None);
            }
            let Some(declaring_state) = for_actual_type(py, &declaring)? else {
                return Ok(None);
            };
            if declaring_state.source() != &method.declaring_class.definition
                || declaring_state
                    .verified_module()
                    .type_facts()
                    .facts()
                    .source_digest
                    != method.declaring_class.source_digest
            {
                return Ok(None);
            }
            let Some(function_owner) = authenticate_strict_function(py, &function)? else {
                return Ok(None);
            };
            if !function_owner.is_finalized()
                || function_owner.awaits_module_nominals()
                || !function_owner.execution_ref().bindings_are_final(
                    py,
                    &*function_owner.globals()?,
                    function_owner.verified_module(),
                )?
                || !function_owner
                    .creation_execution()
                    .is_some_and(|execution| {
                        Arc::ptr_eq(execution, declaring_state.namespace_execution())
                    })
                || function_owner
                    .verified_module()
                    .type_facts()
                    .facts()
                    .source_digest
                    != method.declaring_class.source_digest
                || !function_owner.origin().is_some_and(|origin| {
                    origin.role == CallableSourceRole::SourceFunction
                        && &origin.definition == expected
                })
            {
                return Ok(None);
            }
            let namespace_index =
                unsafe { _PyDict_IndexedKeyIndex(namespace.as_ptr(), key.as_ptr()) };
            if namespace_index < 0 {
                return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
                    strict_runtime_unavailable(
                        py,
                        "sealed method has no stable native namespace index",
                    )
                } else {
                    PyErr::fetch(py)
                });
            }
            let (Ok(mro_index), Ok(namespace_index)) =
                (u32::try_from(mro_index), u32::try_from(namespace_index))
            else {
                return Ok(None);
            };
            return Ok(Some(SealedMethodCapability {
                witness,
                declaring_execution: Arc::clone(declaring_state.namespace_execution()),
                declaring_construction: Arc::clone(&declaring_state.state.data().construction),
                expected_type: actual.as_ptr() as usize,
                function_identity: function.as_ptr() as usize,
                mro_index,
                namespace_index,
                name: name.to_owned(),
                method_source: expected.clone(),
            }));
        }
        Ok(None)
    }

    /// Publish a request only from this class's completed native seal and
    /// immutable own-family row. Source facts alone cannot create a slot.
    pub(crate) fn sealed_virtual_method(
        &self,
        name: &str,
    ) -> PyResult<Option<SealedVirtualMethodCapability>> {
        if self.sealed_witness()?.is_none() {
            return Ok(None);
        }
        let Some(families) = self.state.data().method_families.get() else {
            return Ok(None);
        };
        Ok(SealedVirtualMethodCapability::for_family(
            &families.own,
            name,
        ))
    }

    /// Bind a source request only to a witnessed family in this actual MRO.
    /// Equal source across two factory executions does not choose between
    /// them: an ambiguous MRO request remains an ordinary lookup.
    pub(crate) fn sealed_virtual_method_for_source(
        &self,
        class: &ClassReference,
        name: &str,
    ) -> PyResult<Option<SealedVirtualMethodCapability>> {
        if self.sealed_witness()?.is_none() {
            return Ok(None);
        }
        let Some(families) = self.state.data().method_families.get() else {
            return Ok(None);
        };
        let mut matching = families.rows.values().filter(|row| {
            row.family.execution.source() == &class.definition
                && row.family.source_digest == class.source_digest
        });
        let Some(row) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some() {
            return Ok(None);
        }
        Ok(SealedVirtualMethodCapability::for_family(&row.family, name))
    }

    fn publish_method_families(&self) -> PyResult<()> {
        if self.state.data().method_families.get().is_some() {
            return Ok(());
        }
        let Some((_, actual)) = self.sealed_witness()? else {
            // An unfinished/unsupported MRO does not authorize an optional
            // dispatch table. No later receiver lookup manufactures one.
            return Ok(());
        };
        let names: BTreeSet<_> = self
            .fact()
            .methods
            .iter()
            .filter(|method| {
                method.binding == MethodBinding::Instance
                    && self.state.data().names.protected.contains(&method.name)
            })
            .map(|method| method.name.clone())
            .collect();
        let mut targets = BTreeMap::new();
        let mut admitted_names = Vec::with_capacity(names.len());
        for name in names {
            if let Some(target) = self.sealed_method(&name)? {
                targets.insert(name.clone(), Some(Arc::new(target)));
                admitted_names.push(name);
            }
        }
        let own = Arc::new(SealedMethodFamily {
            execution: Arc::clone(self.namespace_execution()),
            interpreter_id: self.verified_module().interpreter_id(),
            source_digest: self.verified_module().type_facts().facts().source_digest,
            names: admitted_names.into_boxed_slice(),
        });
        let mut families = vec![Arc::clone(&own)];
        for base in actual_mro(&actual)?.iter().skip(1) {
            if let Some(base) = for_actual_type(self.owner().py(), &base)?
                && let Some(published) = base.state.data().method_families.get()
            {
                families.push(Arc::clone(&published.own));
            }
        }
        let mut rows = BTreeMap::new();
        for family in families {
            let mut row = Vec::with_capacity(family.names.len());
            for name in family.names.iter() {
                if !targets.contains_key(name) {
                    targets.insert(name.clone(), self.sealed_method(name)?.map(Arc::new));
                }
                row.push(targets.get(name).expect("resolved method name").clone());
            }
            rows.insert(
                Arc::as_ptr(&family) as usize,
                SealedMethodFamilyRow {
                    family,
                    targets: row.into_boxed_slice(),
                },
            );
        }
        if self
            .state
            .data()
            .method_families
            .set(SealedMethodFamilies { own, rows })
            .is_err()
        {
            return Err(strict_runtime_unavailable(
                self.owner().py(),
                "sealed method families were already published",
            ));
        }
        Ok(())
    }

    /// The orchestrator must first authenticate the final actual members and
    /// finish function/decorator adoption. This transition never installs or
    /// retrofits a layout, and cannot restore unrestricted behavior on failure.
    pub(crate) fn seal(&self) -> PyResult<()> {
        self.ensure_live()?;
        let py = self.owner().py();
        if !matches!(
            self.state.data().phase.get(),
            ClassPhase::Bound | ClassPhase::Sealed
        ) {
            return Err(strict_runtime_unavailable(
                py,
                "strict class has not bound its actual type",
            ));
        }
        let actual_type = self.actual_type()?;
        if self.is_finalized() {
            if unsafe { PyType_IsSoacSealed(actual_type.as_ptr()) } != 1 {
                return Err(strict_runtime_unavailable(
                    py,
                    "sealed class lost its permanent native contract",
                ));
            }
            return Ok(());
        }
        // Function adoption can allocate after the orchestrator's earlier
        // admission. Recheck the actual dictionary immediately before the
        // native one-way seal, with no successful-path allocation or callback.
        validate_copied_namespace(&self.state, &actual_type)?;
        if unsafe { PyType_SealSoacContract(actual_type.as_ptr(), self.owner().as_ptr()) } < 0 {
            return Err(PyErr::fetch(py));
        }
        self.state.data().phase.set(ClassPhase::Sealed);
        if self.is_interpreter_construction() {
            Ok(())
        } else {
            self.publish_method_families()
        }
    }
}

fn raw_dictionary_location_supported(class: *mut ffi::PyTypeObject) -> bool {
    let flags = unsafe { (*class).tp_flags };
    flags & PY_TPFLAGS_INLINE_VALUES == 0
        && (flags & ffi::Py_TPFLAGS_MANAGED_DICT != 0 || unsafe { (*class).tp_dictoffset > 0 })
}

fn actual_mro<'py>(class: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    let py = class.py();
    let mro = unsafe { (*class.as_ptr().cast::<ffi::PyTypeObject>()).tp_mro };
    if mro.is_null() || unsafe { ffi::PyTuple_CheckExact(mro) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "strict class has no exact actual MRO",
        ));
    }
    unsafe { Bound::<PyAny>::from_borrowed_ptr(py, mro) }
        .cast_into::<PyTuple>()
        .map_err(Into::into)
}

fn mro_is_sealed(class: &Bound<'_, PyAny>) -> PyResult<bool> {
    for base in actual_mro(class)?.iter() {
        if base.as_ptr() == ptr::addr_of_mut!(ffi::PyBaseObject_Type).cast() {
            continue;
        }
        if unsafe { PyType_IsSoacSealed(base.as_ptr()) } != 1 {
            // A terminal ancestor reports its native error. An ordinary or
            // still-constructing base merely makes publication ineligible.
            if unsafe { PyType_GetSoacContractOwner(base.as_ptr()) }.is_null()
                && !unsafe { ffi::PyErr_Occurred() }.is_null()
            {
                return Err(PyErr::fetch(class.py()));
            }
            return Ok(false);
        }
    }
    Ok(true)
}

fn sealed_field_precedence(
    class: &Bound<'_, PyAny>,
    name: &str,
) -> PyResult<Option<SealedFieldPrecedence>> {
    let py = class.py();
    for (mro_index, base) in actual_mro(class)?.iter().enumerate() {
        // The accessor also handles the per-interpreter static object type.
        // Never read a static builtin's null tp_dict as a Python pointer.
        let namespace = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(py, PyType_GetDict(base.as_ptr().cast()))
        }?
        .cast_into::<PyDict>()?;
        let Some((key, value)) = namespace
            .iter()
            .find(|(key, _)| exact_name_matches(key, name))
        else {
            continue;
        };
        if base.as_ptr() == ptr::addr_of_mut!(ffi::PyBaseObject_Type).cast() {
            // Static builtin tables have a different locator/lifetime model;
            // leave these accesses generic rather than invent a raw index.
            return Ok(None);
        }
        let value_type = unsafe { ffi::Py_TYPE(value.as_ptr()) };
        if unsafe { (*value_type).tp_descr_get.is_some() || (*value_type).tp_descr_set.is_some() } {
            return Ok(None);
        }
        // Native class policy installation converts its exact namespace to an
        // indexed table. Iteration positions are NOT storage indices: deletion
        // and reinsertion can leave reserved invisible prefix slots.
        let namespace_index = unsafe { _PyDict_IndexedKeyIndex(namespace.as_ptr(), key.as_ptr()) };
        if namespace_index < 0 {
            return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
                strict_runtime_unavailable(py, "sealed class binding has no stable native index")
            } else {
                PyErr::fetch(py)
            });
        }
        let (Ok(mro_index), Ok(namespace_index)) =
            (u32::try_from(mro_index), u32::try_from(namespace_index))
        else {
            return Ok(None);
        };
        return Ok(Some(SealedFieldPrecedence::GuardPlainClassBinding {
            mro_index,
            namespace_index,
        }));
    }
    Ok(Some(SealedFieldPrecedence::NoClassBinding))
}

/// The caller pins this private Rust capability for the entire machine-code
/// use. Returns 1 for a matching live receiver, 0 for original-getattr fallback,
/// and -1 with the original error set. Only a successful result may dominate
/// the raw stable-prefix probe, with no intervening Python effect.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn dp_jit_match_sealed_field_capability(
    receiver: *mut ffi::PyObject,
    capability: *const SealedFieldCapability,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<bool> {
        if receiver.is_null() || capability.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null sealed field guard operand",
            ));
        }
        let receiver = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, receiver) };
        unsafe { &*capability }.matches_receiver(&receiver)
    }));
    match result {
        Ok(Ok(matches)) => c_int::from(matches),
        Ok(Err(error)) => {
            error.restore(py);
            -1
        }
        Err(_) => {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_SystemError,
                    c"panic in sealed field guard".as_ptr(),
                );
            }
            -1
        }
    }
}

/// Resolve a protected method for a checked public call. Success writes a
/// borrowed callee retained by the live receiver's frozen MRO and returns 1.
/// A miss returns 0; -1 preserves an error and must not retry lookup. The caller
/// pins the capability and receiver, and INCREFs the callee before allowing
/// effects or releasing operands. No argument/default checks run in lookup.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn dp_jit_resolve_sealed_method_capability(
    receiver: *mut ffi::PyObject,
    capability: *const SealedMethodCapability,
    callee: *mut *mut ffi::PyObject,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<bool> {
        if callee.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null sealed method result operand",
            ));
        }
        unsafe {
            *callee = ptr::null_mut();
        }
        if receiver.is_null() || capability.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null sealed method guard operand",
            ));
        }
        let receiver = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, receiver) };
        let Some(function) = unsafe { &*capability }.resolve(&receiver)? else {
            return Ok(false);
        };
        // The namespace retains this exact function; releasing the temporary
        // indexed getter reference cannot trigger a finalizer on the hit path.
        unsafe {
            *callee = function.as_ptr();
        }
        Ok(true)
    }));
    match result {
        Ok(Ok(matches)) => c_int::from(matches),
        Ok(Err(error)) => {
            error.restore(py);
            -1
        }
        Err(_) => {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_SystemError,
                    c"panic in sealed method resolver".as_ptr(),
                );
            }
            -1
        }
    }
}

/// Resolve the actual family implementation and an authenticated checked
/// entry before argument evaluation. Neither default lookup/liveness checks nor
/// compilation occurs here. 1 supplies borrowed output, 0 requests ordinary
/// lookup, and -1 preserves an exception without retrying lookup.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn dp_jit_resolve_sealed_virtual_method_capability(
    receiver: *mut ffi::PyObject,
    capability: *const SealedVirtualMethodCapability,
    target: *mut RawSealedMethodTarget,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<bool> {
        if target.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null sealed virtual method result operand",
            ));
        }
        unsafe {
            *target = RawSealedMethodTarget::empty();
        }
        if receiver.is_null() || capability.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null sealed virtual method guard operand",
            ));
        }
        let receiver = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, receiver) };
        let Some(function) = unsafe { &*capability }.resolve(&receiver)? else {
            return Ok(false);
        };
        let Some(entry) = crate::private_checked_vectorcall_entry(&function)? else {
            return Ok(false);
        };
        // The frozen namespace retains this function after the temporary
        // getter reference is released. The caller must pin it before effects.
        unsafe {
            *target = RawSealedMethodTarget {
                callee: function.as_ptr(),
                entry: Some(entry),
            };
        }
        Ok(true)
    }));
    match result {
        Ok(Ok(matches)) => c_int::from(matches),
        Ok(Err(error)) => {
            error.restore(py);
            -1
        }
        Err(_) => {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_SystemError,
                    c"panic in sealed virtual method resolver".as_ptr(),
                );
            }
            -1
        }
    }
}

fn require_interpreter(py: Python<'_>, expected: i64) -> PyResult<()> {
    let actual = unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
    if actual < 0 {
        return Err(PyErr::fetch(py));
    }
    if actual != expected {
        return Err(strict_runtime_unavailable(
            py,
            "strict class policy belongs to another interpreter",
        ));
    }
    Ok(())
}

fn require_actual_owner(
    py: Python<'_>,
    class: &Bound<'_, PyAny>,
    owner: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let actual = unsafe { PyType_GetSoacContractOwner(class.as_ptr()) };
    if actual.is_null() && !unsafe { ffi::PyErr_Occurred() }.is_null() {
        return Err(PyErr::fetch(py));
    }
    if actual == owner.as_ptr() {
        return Ok(());
    }
    // A pending construction is not a permanent contract. It can support
    // explicit construction/nominal operations, never for_actual_type's seal
    // or an optional storage/dispatch capability.
    if actual.is_null()
        && let Some(info) = native_construction_info(py, class)?
        && info.owner == owner.as_ptr()
        && !info.root_construction.is_null()
        && matches!(info.phase, NATIVE_TYPE_PENDING | NATIVE_TYPE_ADMITTING)
    {
        return Ok(());
    }
    Err(strict_runtime_unavailable(
        py,
        "strict class actual type/owner identity mismatch",
    ))
}

/// Return an ephemeral pin on the originally bound type, never an address
/// lookup. Its callback-free weakref cannot be retargeted by a public native
/// constructor that reuses this exposed owner after the type has died.
fn bound_type_witness<'py>(
    state: &StrictStateRef<'py, StrictClassData>,
) -> PyResult<Bound<'py, PyAny>> {
    let py = state.owner().py();
    let reference = state.reference(ACTUAL_TYPE_WEAKREF)?;
    let mut actual = ptr::null_mut();
    let status = unsafe { ffi::PyWeakref_GetRef(reference.as_ptr(), &mut actual) };
    if status < 0 {
        return Err(PyErr::fetch(py));
    }
    if status == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "strict class construction has ended",
        ));
    }
    Ok(unsafe { Bound::from_owned_ptr(py, actual) })
}

/// The native getter does not inherit authority to ordinary subclasses. The
/// private Rust payload, original actual-type weak witness, and recorded type
/// identity also have to agree. A retained owner and reused address cannot do.
pub(crate) fn for_actual_type<'py>(
    py: Python<'py>,
    class: &Bound<'py, PyAny>,
) -> PyResult<Option<StrictClassState<'py>>> {
    let owner = unsafe { PyType_GetSoacContractOwner(class.as_ptr()) };
    if owner.is_null() {
        return if unsafe { ffi::PyErr_Occurred() }.is_null() {
            Ok(None)
        } else {
            Err(PyErr::fetch(py))
        };
    }
    let state = StrictClassState {
        state: StrictStateRef::from_owner(unsafe { Bound::from_borrowed_ptr(py, owner) })?,
        actual_type: Some(class.clone()),
    };
    state.ensure_live()?;
    if state.state.data().actual_type.get() != class.as_ptr() as usize
        || !matches!(
            state.state.data().phase.get(),
            ClassPhase::Bound | ClassPhase::Sealed
        )
        || state.is_finalized() && unsafe { PyType_IsSoacSealed(class.as_ptr()) } != 1
    {
        return Err(strict_runtime_unavailable(
            py,
            "native strict class binding is inconsistent",
        ));
    }
    Ok(Some(state))
}

/// Actual construction ownership without implying a permanent type contract.
/// Only construction/adoption and nominal binding may use this view. Base,
/// receiver, layout and dispatch consumers keep using for_actual_type.
pub(crate) fn for_constructed_type<'py>(
    py: Python<'py>,
    class: &Bound<'py, PyAny>,
) -> PyResult<Option<StrictClassState<'py>>> {
    if unsafe { ffi::PyType_Check(class.as_ptr()) } == 0 {
        return Ok(None);
    }
    let Some(info) = native_construction_info(py, class)? else {
        return Ok(None);
    };
    if !matches!(info.phase, NATIVE_TYPE_PENDING | NATIVE_TYPE_ADMITTING) {
        return for_actual_type(py, class);
    }
    if info.owner.is_null() || info.root_construction.is_null() {
        return Err(strict_runtime_unavailable(
            py,
            "pending type lost its construction owner",
        ));
    }
    let state = StrictClassState {
        state: StrictStateRef::from_owner(unsafe { Bound::from_borrowed_ptr(py, info.owner) })?,
        actual_type: Some(class.clone()),
    };
    state.ensure_live()?;
    if state.state.data().actual_type.get() != class.as_ptr() as usize
        || !matches!(
            state.state.data().phase.get(),
            ClassPhase::Pending | ClassPhase::Admitting
        )
    {
        return Err(strict_runtime_unavailable(
            py,
            "pending native type has an inconsistent source binding",
        ));
    }
    Ok(Some(state))
}

/// Cleanup-only identity check for a failed native Apply graph. A FAILED
/// lineage intentionally cannot pass for_constructed_type, but its actual
/// native type still supports the original metadata owner. This predicate
/// returns no construction/admission capability and never reopens the barrier.
/// Successful checks allocate/call Python nowhere; the weak inventory's one
/// upgraded actual type supports every native/metadata observation.
pub(crate) fn matches_failed_interpreter_dataclass(
    py: Python<'_>,
    class: &Bound<'_, PyAny>,
    dataclass_owner: &Bound<'_, PyAny>,
    source: &SourceIdentity,
    verified: &Arc<VerifiedStrictModule>,
    execution: &StrictModuleExecutionRef,
    invocation: &Arc<crate::strict_interpreter::InterpreterInvocationIdentity>,
) -> PyResult<bool> {
    if unsafe { ffi::PyType_Check(class.as_ptr()) } == 0 {
        return Ok(false);
    }
    let Some(info) = native_construction_info(py, class)? else {
        return Ok(false);
    };
    if info.phase != NATIVE_TYPE_FAILED || info.owner.is_null() || info.root_construction.is_null()
    {
        return Ok(false);
    }
    let Some(state) = StrictStateRef::<StrictClassData>::try_from_owner(unsafe {
        Bound::from_borrowed_ptr(py, info.owner)
    })?
    else {
        return Ok(false);
    };
    let data = state.data();
    if data.actual_type.get() != class.as_ptr() as usize
        || &data.fact.identity != source
        || !Arc::ptr_eq(&data.verified, verified)
        || !data.execution.same_execution(execution)
        || !data
            .interpreter_invocation
            .get()
            .is_some_and(|actual| Arc::ptr_eq(actual, invocation))
    {
        return Ok(false);
    }
    let Some(dataclass) = &data.dataclass else {
        return Ok(false);
    };
    if !dataclass.pending()
        || unsafe { state.reference_ptr(dataclass.reference)? }.as_ptr() != dataclass_owner.as_ptr()
    {
        return Ok(false);
    }
    // The same source can produce multiple independent graphs in one call.
    // Match the original live weak witness as well as the current native edge;
    // comparison-only addresses never recover a type or confer authority.
    Ok(bound_type_witness(&state)?.as_ptr() == class.as_ptr())
}

/// Authenticate the original captured class dictionary before a permanent
/// class-dictionary policy exists. The actual function owns only a cached
/// callback-free weakref, not the type or namespace. Its actual live referent
/// supports the native query; a recorded address is used for comparison only.
pub(crate) fn matches_function_class_namespace(
    py: Python<'_>,
    dictionary: &Bound<'_, PyAny>,
    function: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<bool> {
    let Some(execution) = function.creation_execution() else {
        return Ok(false);
    };
    let Some(expected_owner) =
        execution.class_dictionary_owner_candidate(dictionary.as_ptr() as usize)
    else {
        return Ok(false);
    };
    let Some(witness) = function.class_weak_witness()? else {
        return Ok(false);
    };
    let mut class = ptr::null_mut();
    match unsafe { ffi::PyWeakref_GetRef(witness.as_ptr(), &mut class) } {
        0 => return Ok(false),
        1 => (),
        _ => return Err(PyErr::fetch(py)),
    }
    let class = unsafe { Bound::<PyAny>::from_owned_ptr(py, class) };
    let Some(info) = native_construction_info(py, &class)? else {
        return Ok(false);
    };
    if info.owner as usize != expected_owner
        || !matches!(
            info.phase,
            NATIVE_TYPE_PENDING | NATIVE_TYPE_ADMITTING | NATIVE_TYPE_ENFORCED
        )
        || unsafe { (*class.as_ptr().cast::<ffi::PyTypeObject>()).tp_dict } != dictionary.as_ptr()
    {
        return Ok(false);
    }
    if info.phase == NATIVE_TYPE_ENFORCED && !matches_class_namespace(py, dictionary, execution)? {
        return Ok(false);
    }
    // Pending has no permanent namespace policy; enforced types also passed
    // that existing exact policy predicate. Native success on the actual
    // weakly supported type permits the owner read below.
    Ok(unsafe {
        StrictStateRef::<StrictClassData>::inspect_live(info.owner, |state| {
            state.actual_type.get() == class.as_ptr() as usize
                && matches!(
                    state.phase.get(),
                    ClassPhase::Pending
                        | ClassPhase::Admitting
                        | ClassPhase::Bound
                        | ClassPhase::Sealed
                )
                && Arc::ptr_eq(&state.namespace_execution, execution)
                && Arc::ptr_eq(&state.verified, function.verified_module())
                && state.execution.same_execution(function.execution_ref())
        })
    }
    .unwrap_or(false))
}

/// Authenticate a pinned class-dictionary cell value without retaining the
/// class or trusting recorded addresses. A native match proves that the
/// expected owner is currently live and owns this actual class namespace;
/// its private payload and execution Arc then reject address reuse. This is
/// a construction binding, not a sealed-class optimization capability.
///
/// The caller must read the selected dictionary entry without a Python
/// effect after this check, or repeat the check after that effect.
pub(crate) fn matches_class_namespace(
    py: Python<'_>,
    dictionary: &Bound<'_, PyAny>,
    execution: &Arc<crate::strict_namespace::NamespaceExecution>,
) -> PyResult<bool> {
    let Some(owner) = execution.class_dictionary_owner_candidate(dictionary.as_ptr() as usize)
    else {
        return Ok(false);
    };
    match unsafe {
        PyDict_MatchesSoacClassNamespace(dictionary.as_ptr(), owner as *mut ffi::PyObject)
    } {
        0 => return Ok(false),
        1 => (),
        _ => return Err(PyErr::fetch(py)),
    }
    // The native predicate compares the untrusted address without dereferencing
    // it. Only its success permits the borrowed owner view; no allocation or
    // Python callback occurs between that proof and the private TypeId check.
    let state = StrictClassState {
        state: StrictStateRef::from_owner(unsafe {
            Bound::<PyAny>::from_borrowed_ptr(py, owner as *mut ffi::PyObject)
        })?,
        actual_type: None,
    };
    state.ensure_live()?;
    if state.state.data().actual_type.get() == 0
        || !matches!(
            state.state.data().phase.get(),
            ClassPhase::Bound | ClassPhase::Sealed
        )
    {
        return Err(strict_runtime_unavailable(
            py,
            "native class namespace has an inconsistent construction owner",
        ));
    }
    Ok(Arc::ptr_eq(state.namespace_execution(), execution))
}

fn catalog_name_supported(name: &str) -> bool {
    // TODO: Consume a checker-authenticated canonical storage name. Upstream
    // private-name mangling has not yet been proved; do not guess or mangle
    // an already-mangled spelling a second time.
    !(name.starts_with("__") && !name.ends_with("__")) && !name.contains('\0')
}

pub(crate) fn is_implicit_wrapper(binding: MethodBinding, name: &str) -> bool {
    matches!(
        (binding, name),
        (MethodBinding::Static, "__new__")
            | (
                MethodBinding::Class,
                "__init_subclass__" | "__class_getitem__"
            )
    )
}

fn getter_only_property(descriptor: &DescriptorFact) -> bool {
    descriptor.kind == DescriptorKind::Property
        && descriptor.getter.is_some()
        && descriptor.setter.is_none()
        && descriptor.deleter.is_none()
}

fn class_plan(
    fact: &ClassTypeFact,
    policy: &ResolvedStrictPolicy,
    bases: &[(&StorageLayout, &NamePolicy)],
    dataclass: Option<&DataclassConstruction<'_>>,
) -> Result<(StorageLayout, NamePolicy), DynamicClassReason> {
    class_plan_with_slots(StorageModel::Indexed, fact, policy, bases, dataclass, None)
}

fn class_plan_with_slots(
    model: StorageModel,
    fact: &ClassTypeFact,
    policy: &ResolvedStrictPolicy,
    bases: &[(&StorageLayout, &NamePolicy)],
    dataclass: Option<&DataclassConstruction<'_>>,
    slots: Option<&ObjectSlotPlan>,
) -> Result<(StorageLayout, NamePolicy), DynamicClassReason> {
    if let ParticipationProposal::Dynamic(reasons) = &fact.participation {
        return Err(reasons
            .first()
            .copied()
            .unwrap_or(DynamicClassReason::UnresolvedAnalysis));
    }
    if fact.metaclass != MetaclassFact::BuiltinType {
        return Err(DynamicClassReason::NonParticipatingMetaclass);
    }
    if !fact.decorators.is_empty() && dataclass.is_none() {
        return Err(DynamicClassReason::UnknownDecorator);
    }
    if fact.transform.is_some() && dataclass.is_none() {
        return Err(DynamicClassReason::FrameworkManaged);
    }
    if fact.dictionary != ClassDictionarySemantics::DictionaryBearing
        && !(fact.dictionary == ClassDictionarySemantics::ExplicitSlots && slots.is_some())
    {
        return Err(DynamicClassReason::ConflictingLayout);
    }
    if !fact.inheritance.complete {
        return Err(DynamicClassReason::UnknownBase);
    }
    let mut layout = StorageLayout::merge(model, bases.iter().map(|(layout, _)| *layout))?;
    if let Some(slots) = slots {
        layout.object_fields = slots.names.clone();
        layout.dictionary_bearing = slots.dictionary;
        layout.declared_slots = slots.declared;
        if !slots.dictionary && !layout.fields.is_empty() {
            return Err(DynamicClassReason::ConflictingLayout);
        }
    }
    let inherited_fields: BTreeSet<_> = layout.fields.iter().cloned().collect();
    let mut names = NamePolicy::default();
    for (_, base) in bases {
        names.protected.extend(base.protected.iter().cloned());
        names
            .final_methods
            .extend(base.final_methods.iter().cloned());
    }
    let mut own_protected = BTreeSet::new();
    let mut own_properties = BTreeSet::new();
    if let Some(dataclass) = dataclass {
        own_protected.extend(dataclass.protected_names().map(str::to_owned));
    }
    for field in &fact.instance_fields {
        if !catalog_name_supported(&field.name) {
            return Err(DynamicClassReason::UnresolvedAnalysis);
        }
        if field.descriptor.kind == DescriptorKind::Property {
            if !getter_only_property(&field.descriptor) {
                return Err(DynamicClassReason::UnsupportedDescriptor);
            }
            if field.declaring_class.definition == fact.identity {
                own_properties.insert(field.name.clone());
            }
            // The descriptor owns access. Its return annotation is a function
            // boundary, not a physical instance-dictionary value requirement.
            continue;
        }
        match field.field_kind {
            FieldKind::InstanceField
            | FieldKind::CallableInstanceField
            | FieldKind::ShadowableClassDefault => {
                if field.descriptor.kind != DescriptorKind::None {
                    return Err(DynamicClassReason::UnsupportedDescriptor);
                }
                if !layout.object_fields.contains(&field.name) {
                    if !layout.dictionary_bearing {
                        // A logical declaration outside actual source slots
                        // grants neither storage nor a successful-write fact.
                        continue;
                    }
                    layout.append(&field.name);
                }
                if field.declaring_class.definition == fact.identity {
                    if let Some(requirement) = selected_field_contract(
                        policy.checked_fields(fact.identity.source_range),
                        field.annotation_origin,
                        &field.value_type,
                    ) {
                        let requirements = layout.checks.entry(field.name.clone()).or_default();
                        if !requirements.contains(&requirement) {
                            requirements.push(requirement);
                        }
                    }
                }
            }
            FieldKind::ClassVariable => {
                own_protected.insert(field.name.clone());
            }
            FieldKind::InitOnly => {}
            FieldKind::CachedDescriptorField => {
                return Err(DynamicClassReason::UnsupportedDescriptor);
            }
            FieldKind::FrameworkPrivate => return Err(DynamicClassReason::FrameworkManaged),
            FieldKind::Dynamic => return Err(DynamicClassReason::UnresolvedAnalysis),
        }
    }
    for member in &fact.class_members {
        if !catalog_name_supported(&member.name) {
            return Err(DynamicClassReason::UnresolvedAnalysis);
        }
        if member.name == "__slots__" && layout.declared_slots {
            continue;
        }
        if member.descriptor.kind == DescriptorKind::Property {
            if !getter_only_property(&member.descriptor) {
                return Err(DynamicClassReason::UnsupportedDescriptor);
            }
            own_properties.insert(member.name.clone());
            continue;
        }
        match member.kind {
            ClassMemberKind::ClassVariable => {
                own_protected.insert(member.name.clone());
            }
            ClassMemberKind::ShadowableDefault | ClassMemberKind::NestedClass => {
                if member.descriptor.kind != DescriptorKind::None {
                    return Err(DynamicClassReason::UnsupportedDescriptor);
                }
                // The checker also catalogs a pseudo-field's default as a
                // class binding. That second view must not turn InitVar or
                // ClassVar into physical instance storage. Existing inherited
                // positions remain intact; only this prospective append is
                // suppressed.
                if fact.instance_fields.iter().any(|field| {
                    field.name == member.name
                        && matches!(
                            field.field_kind,
                            FieldKind::InitOnly | FieldKind::ClassVariable
                        )
                }) {
                    continue;
                }
                if layout.object_fields.contains(&member.name) || !layout.dictionary_bearing {
                    continue;
                }
                layout.append(&member.name);
            }
            ClassMemberKind::Descriptor => return Err(DynamicClassReason::UnsupportedDescriptor),
            ClassMemberKind::Dynamic => return Err(DynamicClassReason::UnresolvedAnalysis),
        }
    }
    for method in &fact.methods {
        if !catalog_name_supported(&method.name) {
            return Err(DynamicClassReason::UnresolvedAnalysis);
        }
        match method.binding {
            MethodBinding::Instance | MethodBinding::Class | MethodBinding::Static => {
                // The orchestrator separately verifies a native descriptor
                // birth tied to this exact namespace execution, or type_new's
                // own implicit wrapper. A proposed kind alone grants nothing.
                if method.declaring_class.definition == fact.identity {
                    own_protected.insert(method.name.clone());
                } else {
                    names.protected.insert(method.name.clone());
                }
            }
            MethodBinding::PropertyGetter => {
                if method.declaring_class.definition == fact.identity {
                    own_properties.insert(method.name.clone());
                }
            }
            MethodBinding::Descriptor => {
                return Err(DynamicClassReason::UnsupportedDescriptor);
            }
        }
        if method.declared_final && method.declaring_class.definition == fact.identity {
            names.final_methods.insert(method.name.clone());
        }
    }
    // The current native ABI uses its fields catalog for instance-field
    // precedence as well as descriptor transition protection. It cannot yet
    // express a new protected name over an inherited physical field slot.
    if !own_protected.is_disjoint(&inherited_fields)
        || !own_properties.is_disjoint(&inherited_fields)
        || layout
            .fields
            .iter()
            .any(|name| own_properties.contains(name))
        || layout
            .fields
            .iter()
            .any(|name| names.final_methods.contains(name))
        || layout
            .fields
            .iter()
            .any(|name| matches!(name.as_str(), "__dict__" | "__weakref__" | "__class__"))
        || layout.object_fields.iter().any(|name| {
            own_properties.contains(name)
                || own_protected.contains(name)
                || names.final_methods.contains(name)
                || matches!(name.as_str(), "__dict__" | "__weakref__" | "__class__")
        })
    {
        return Err(DynamicClassReason::ConflictingLayout);
    }
    names.protected.extend(own_protected);
    for field in &layout.fields {
        names.protected.remove(field);
    }
    for field in &layout.object_fields {
        names.protected.remove(field);
    }
    Ok((layout, names))
}

/// Comparing codepoints avoids allocating a Python key or a Unicode UTF-8
/// cache in the pre-Ready callback. Exact str never runs custom equality.
pub(crate) fn exact_name_matches(name: &Bound<'_, PyAny>, expected: &str) -> bool {
    if unsafe { ffi::PyUnicode_CheckExact(name.as_ptr()) } == 0
        || unsafe { ffi::PyUnicode_GetLength(name.as_ptr()) }
            != expected.chars().count() as ffi::Py_ssize_t
    {
        return false;
    }
    expected.chars().enumerate().all(|(index, character)| {
        (unsafe { ffi::PyUnicode_ReadChar(name.as_ptr(), index as ffi::Py_ssize_t) })
            == u32::from(character)
    })
}

fn descriptor_kind_supported(
    name: &Bound<'_, PyAny>,
    value: &Bound<'_, PyAny>,
    fact: &ClassTypeFact,
    layout: &StorageLayout,
    actual_type: *mut ffi::PyTypeObject,
    dataclass: Option<&DataclassNamespace<'_>>,
) -> bool {
    let kind = unsafe { ffi::Py_TYPE(value.as_ptr()) };
    if unsafe { (*kind).tp_descr_get.is_none() && (*kind).tp_descr_set.is_none() } {
        return true;
    }
    if layout
        .object_fields
        .iter()
        .any(|field| exact_name_matches(name, field))
    {
        // Before Ready, an input descriptor has no native slot authority.
        // Ready publishes only the native catalog's canonical member through
        // its checked class-dictionary transition; the permanent barrier then
        // preserves that binding. A member-like object is not sufficient.
        return !actual_type.is_null() && kind == ptr::addr_of_mut!(PyMemberDescr_Type);
    }
    if layout
        .fields
        .iter()
        .any(|field| exact_name_matches(name, field))
    {
        return false;
    }
    if !actual_type.is_null() && kind == ptr::addr_of_mut!(PyGetSetDescr_Type) {
        // type_new adds interpreter-cached __dict__/__weakref__ descriptors
        // whose d_type is object, not this class. Require the exact matching
        // cached descriptor, not merely its kind, declared owner, or spelling.
        // The predicate can fail only for NULL operands; both Bound references
        // here are valid, and its valid-operand path runs no Python callbacks.
        return (unsafe { _PySoac_IsLayoutDescriptor(name.as_ptr(), value.as_ptr()) }) == 1;
    }
    if unsafe { ffi::PyFunction_Check(value.as_ptr()) } != 0
        && ["__annotate__", "__annotate_func__"]
            .iter()
            .any(|expected| exact_name_matches(name, expected))
    {
        // Only a kind check here. The orchestrator separately authenticates
        // the actual AnnotationProvider origin against this class and digest.
        return true;
    }
    if dataclass.is_some_and(|proof| proof.generated_descriptor(name, value)) {
        // Kind filtering only. The same admission independently matches each
        // actual member against its permanently adopted native birth witness.
        return true;
    }
    // These are kind/name filters only. Full namespace validation separately
    // proves the native birth, exact source component and NamespaceExecution.
    if kind == ptr::addr_of_mut!(PyProperty_Type) {
        return fact.methods.iter().any(|method| {
            method.binding == MethodBinding::PropertyGetter
                && exact_name_matches(name, &method.name)
        }) || fact.instance_fields.iter().any(|field| {
            getter_only_property(&field.descriptor) && exact_name_matches(name, &field.name)
        }) || fact.class_members.iter().any(|member| {
            getter_only_property(&member.descriptor) && exact_name_matches(name, &member.name)
        });
    }
    fact.methods.iter().any(|method| {
        exact_name_matches(name, &method.name)
            && match method.binding {
                MethodBinding::Instance => (unsafe { ffi::PyFunction_Check(value.as_ptr()) }) != 0,
                MethodBinding::Class | MethodBinding::Static => {
                    if actual_type.is_null()
                        && is_implicit_wrapper(method.binding, &method.name)
                        && unsafe { ffi::PyFunction_Check(value.as_ptr()) } != 0
                    {
                        true
                    } else {
                        kind == if method.binding == MethodBinding::Class {
                            ptr::addr_of_mut!(PyClassMethod_Type)
                        } else {
                            ptr::addr_of_mut!(PyStaticMethod_Type)
                        }
                    }
                }
                MethodBinding::PropertyGetter | MethodBinding::Descriptor => false,
            }
    })
}

fn namespace_admission(
    namespace: &Bound<'_, PyDict>,
    fact: &ClassTypeFact,
    layout: &StorageLayout,
    actual_type: *mut ffi::PyTypeObject,
    dataclass: Option<&DataclassNamespace<'_>>,
) -> Result<(), DynamicClassReason> {
    // Walk actual keys/values, including names absent from checker proposals.
    // No hashing, equality hooks, attribute access, or allocation is needed.
    for (name, value) in namespace.iter() {
        if unsafe { ffi::PyUnicode_CheckExact(name.as_ptr()) } == 0 {
            return Err(DynamicClassReason::UnresolvedAnalysis);
        }
        if exact_name_matches(&name, "__slots__") && !layout.declared_slots {
            return Err(DynamicClassReason::ConflictingLayout);
        }
        if actual_type.is_null()
            && layout
                .object_fields
                .iter()
                .any(|field| exact_name_matches(&name, field))
        {
            // A source default or foreign descriptor cannot stand in for the
            // native member that Ready will publish. Ordinary construction
            // retains CPython's conflict/error behavior for this shape.
            return Err(DynamicClassReason::ConflictingLayout);
        }
        if [
            "__getattribute__",
            "__getattr__",
            "__setattr__",
            "__delattr__",
        ]
        .iter()
        .any(|hook| exact_name_matches(&name, hook))
            && !dataclass.is_some_and(|proof| proof.generated_attribute_hook(&name))
        {
            return Err(DynamicClassReason::CustomAttributeHooks);
        }
        if !descriptor_kind_supported(&name, &value, fact, layout, actual_type, dataclass) {
            return Err(DynamicClassReason::UnsupportedDescriptor);
        }
    }
    Ok(())
}

fn dataclass_namespace<'py>(
    state: &StrictStateRef<'py, StrictClassData>,
) -> PyResult<Option<DataclassNamespace<'py>>> {
    state
        .data()
        .dataclass
        .as_ref()
        .map(|dataclass| Ok(dataclass.namespace(state.reference(dataclass.reference)?)))
        .transpose()
}

fn validate_copied_namespace<'py>(
    state: &StrictStateRef<'py, StrictClassData>,
    actual_type: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyDict>> {
    let py = actual_type.py();
    let dictionary = unsafe { (*actual_type.as_ptr().cast::<ffi::PyTypeObject>()).tp_dict };
    if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "strict type has no exact copied namespace",
        ));
    }
    let dictionary =
        unsafe { Bound::<PyAny>::from_borrowed_ptr(py, dictionary) }.cast_into::<PyDict>()?;
    let storage = StoragePolicyOwner::from_owner(state.reference(STORAGE_POLICY)?)?;
    let dataclass = dataclass_namespace(state)?;
    if namespace_admission(
        &dictionary,
        &state.data().fact,
        &storage.data().layout,
        actual_type.as_ptr().cast(),
        dataclass.as_ref(),
    )
    .is_err()
        || !crate::strict_class::validate_actual_class_namespace(
            py,
            &dictionary,
            &state.data().verified,
            &state.data().fact,
            if state.data().phase.get() == ClassPhase::Prepared {
                crate::strict_class::ClassNamespacePhase::Copied
            } else {
                crate::strict_class::ClassNamespacePhase::Adopted
            },
            &state.data().namespace_execution,
            dataclass.as_ref(),
        )?
    {
        return Err(strict_runtime_unavailable(
            py,
            "actual strict class namespace differs from its admitted contract",
        ));
    }
    Ok(dictionary)
}

fn prepare_method_nominals(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    verified: &Arc<VerifiedStrictModule>,
    fact: &ClassTypeFact,
    execution: &Arc<crate::strict_namespace::NamespaceExecution>,
) -> PyResult<()> {
    // Temporary cached weakrefs distinguish actual aliases without retaining
    // replaced functions or using a recyclable address as identity. The
    // selected function's own GC vector keeps the unresolved direct-self slot;
    // final admission authenticates and binds that slot on the selected type.
    let mut prepared: Vec<Bound<'_, PyAny>> = Vec::new();
    for (name, binding, _) in crate::strict_class::own_method_components(fact) {
        let value = crate::strict_class::namespace_item(namespace, name)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "nominal method disappeared before preparation")
        })?;
        // Input __new__/__init_subclass__/__class_getitem__ functions have not
        // yet acquired their native implicit wrappers. Copied-namespace
        // validation independently authenticates/seals those wrappers.
        let function = if unsafe { ffi::PyFunction_Check(value.as_ptr()) } != 0 {
            value
        } else {
            crate::strict_class::method_function(&value, binding)?.ok_or_else(|| {
                strict_runtime_unavailable(py, "nominal method changed descriptor kind")
            })?
        };
        let mut alias = false;
        for previous in &prepared {
            let mut referent = ptr::null_mut();
            let status = unsafe { ffi::PyWeakref_GetRef(previous.as_ptr(), &mut referent) };
            if status < 0 {
                return Err(PyErr::fetch(py));
            }
            if status != 0 {
                let referent = unsafe { Bound::<PyAny>::from_owned_ptr(py, referent) };
                if referent.as_ptr() == function.as_ptr() {
                    alias = true;
                    break;
                }
            }
        }
        if alias
            || !crate::strict_nominal::prepare_owned_method_nominals(
                py, &function, verified, fact, execution,
            )?
        {
            continue;
        }
        let weak = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                ffi::PyWeakref_NewRef(function.as_ptr(), ptr::null_mut()),
            )
        }?;
        prepared.push(weak);
    }
    Ok(())
}

/// Prepare logical/native ownership only after the authenticated namespace
/// body has finished. All ordinary dynamic declines precede native handle
/// creation and irreversible installation. Namespace/function references are
/// temporary and are not retained by the resulting state.
pub(crate) fn prepare_class_state<'py>(
    py: Python<'py>,
    auth: &AuthenticatedStrictFunction<'_, 'py>,
    fact: &ClassTypeFact,
    actual_bases: &Bound<'py, PyTuple>,
    actual_namespace: &Bound<'py, PyDict>,
    namespace_execution: &Arc<crate::strict_namespace::NamespaceExecution>,
    construction_captures: Option<&ClassConstructionCaptures<'py>>,
    dataclass: Option<&DataclassConstruction<'py>>,
) -> PyResult<Result<StrictClassState<'py>, DynamicClassReason>> {
    let verified = auth.verified_module();
    if !auth.origin().is_some_and(|origin| {
        origin.role == CallableSourceRole::ClassNamespace && origin.definition == fact.identity
    }) || !verified
        .type_facts()
        .facts()
        .classes
        .iter()
        .any(|expected| expected == fact)
    {
        return Err(strict_runtime_unavailable(
            py,
            "class state is not the actual authenticated namespace plan",
        ));
    }
    if dataclass.is_some_and(|proof| {
        !proof.matches(
            fact,
            verified.type_facts().facts().source_digest,
            namespace_execution,
        )
    }) {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass construction proof belongs to another namespace",
        ));
    }
    if unsafe { ffi::PyTuple_CheckExact(actual_bases.as_ptr()) } == 0
        || unsafe { ffi::PyDict_CheckExact(actual_namespace.as_ptr()) } == 0
    {
        return Ok(Err(DynamicClassReason::UnresolvedAnalysis));
    }
    let module_owner = auth
        .execution_ref()
        .acquire_owner(py, &*auth.globals()?, verified)?;
    let mut bases = Vec::with_capacity(actual_bases.len());
    if actual_bases.len() != fact.bases.len() {
        return Ok(Err(DynamicClassReason::UnknownBase));
    }
    for (actual, expected) in actual_bases.iter().zip(&fact.bases) {
        let expected = match expected {
            BaseReference::Builtin(BuiltinType::Object)
                if actual.as_ptr() == ptr::addr_of_mut!(ffi::PyBaseObject_Type).cast() =>
            {
                continue;
            }
            BaseReference::Builtin(_) => return Ok(Err(DynamicClassReason::UnknownBase)),
            BaseReference::Class(expected) => expected,
        };
        if unsafe { ffi::Py_TYPE(actual.as_ptr()) } != ptr::addr_of_mut!(ffi::PyType_Type) {
            return Ok(Err(DynamicClassReason::NonParticipatingMetaclass));
        }
        let Some(base) = for_actual_type(py, &actual)? else {
            return Ok(Err(DynamicClassReason::MutableBase));
        };
        if base.source() != &expected.definition
            || base.verified_module().type_facts().facts().source_digest != expected.source_digest
        {
            return Ok(Err(DynamicClassReason::UnknownBase));
        }
        bases.push(base);
    }
    let base_storage = bases
        .iter()
        .map(StrictClassState::storage)
        .collect::<PyResult<Vec<_>>>()?;
    let plans: Vec<_> = base_storage
        .iter()
        .zip(&bases)
        .map(|(storage, base)| (&storage.data().layout, &base.state.data().names))
        .collect();
    let slots = match ObjectSlotPlan::prepare(
        fact,
        actual_namespace,
        actual_bases,
        base_storage
            .iter()
            .map(|storage| storage.data().layout.object_fields.as_slice()),
        false,
    )? {
        Ok(slots) => slots,
        Err(reason) => return Ok(Err(reason)),
    };
    // Source types begin Pending on either backend and keep their actual
    // source-requested storage. Final admission cannot introduce an indexed
    // requirement or flip INLINE_VALUES after native Ready.
    let model = StorageModel::Ordinary;
    if base_storage.iter().any(|storage| {
        storage.data().model != model
            && storage.data().dictionary_mode() != 0
            && !storage.data().layout.fields.is_empty()
    }) {
        return Ok(Err(DynamicClassReason::ConflictingLayout));
    }
    let (layout, names) = match class_plan_with_slots(
        model,
        fact,
        &verified.type_facts().facts().language_policy,
        &plans,
        dataclass,
        Some(&slots),
    ) {
        Ok(plan) => plan,
        Err(reason) => return Ok(Err(reason)),
    };
    let dataclass_namespace = dataclass.map(DataclassConstruction::namespace);
    if let Err(reason) = namespace_admission(
        actual_namespace,
        fact,
        &layout,
        ptr::null_mut(),
        dataclass_namespace.as_ref(),
    ) {
        return Ok(Err(reason));
    }
    let bindings = if let Some(dataclass) = dataclass {
        dataclass.own_field_bindings()?
    } else {
        match prepare_own_field_bindings(
            auth,
            fact,
            actual_namespace,
            namespace_execution,
            &own_checked_fields(auth, fact),
            construction_captures,
        )? {
            Ok(bindings) => bindings,
            Err(reason) => return Ok(Err(reason)),
        }
    };
    let own_checks =
        match prepare_field_checks(auth, fact, actual_namespace, namespace_execution, &bindings)? {
            Ok(checks) => checks,
            Err(reason) => return Ok(Err(reason)),
        };
    let mut checks = inherited_field_checks(&base_storage)?;
    let own_check_ordinal = own_checks.map(|own| {
        let ordinal = checks.len();
        checks.push(SelectedStorageCheck::own(own, &layout));
        ordinal
    });
    let object_offsets = layout.object_fields.iter().map(|_| Cell::new(-1)).collect();
    let storage = new_storage_owner(py, verified.interpreter_id(), model, layout, &checks)?;
    let own_field_checks = own_check_ordinal.map(|index| storage.data().check_reference(index));
    // Actual bases stay alive through native construction and the type's MRO.
    // A field policy retains only its selected nominal targets, not the base
    // owner or module that originally supplied that mandatory contract.
    let mut references = vec![module_owner, storage.owner().clone().unbind(), py.None()];
    let own_field_bindings = bindings
        .iter()
        .map(|binding| {
            let index = references.len();
            references.push(binding.owner().clone().unbind());
            index
        })
        .collect();
    prepare_method_nominals(py, actual_namespace, verified, fact, namespace_execution)?;
    let dataclass = dataclass.map(|proof| proof.attach(&mut references));
    let state = StrictStateRef::new(
        py,
        StrictClassData {
            verified: verified.clone(),
            execution: auth.execution_ref().clone(),
            fact: fact.clone(),
            names,
            phase: Cell::new(ClassPhase::Prepared),
            actual_type: Cell::new(0),
            construction: Arc::new(ActualClassConstruction),
            construction_kind: ClassConstructionKind::SourceNamespace,
            object_offsets,
            namespace_execution: Arc::clone(namespace_execution),
            interpreter_invocation: OnceLock::new(),
            own_field_bindings,
            own_field_checks,
            dataclass,
            method_families: OnceLock::new(),
        },
        references,
    )?;
    Ok(Ok(StrictClassState {
        state,
        actual_type: None,
    }))
}

/// The recognized `_add_slots` call replaces physical storage but does not
/// repeat the declaring source execution. Every required nominal binding and
/// method boundary already belongs to that original execution and remains
/// unchanged. The proof is minted only by the actual native invocation.
pub(crate) fn prepare_replacement_class_state<'py>(
    original: &StrictClassState<'py>,
    actual_bases: &Bound<'py, PyTuple>,
    actual_namespace: &Bound<'py, PyDict>,
    proof: &DataclassSlotsConstruction<'py>,
) -> PyResult<StrictClassState<'py>> {
    let actual_original = original.actual_type()?;
    let py = actual_original.py();
    let verified = original.verified_module();
    let fact = original.fact();
    if original.state.data().construction_kind != ClassConstructionKind::SourceNamespace
        || !proof.matches(
            original,
            fact,
            verified.type_facts().facts().source_digest,
            original.namespace_execution(),
        )?
        || actual_bases.as_ptr()
            != unsafe { (*actual_original.as_ptr().cast::<ffi::PyTypeObject>()).tp_bases }
    {
        return Err(strict_runtime_unavailable(
            py,
            "slots replacement does not match its actual original construction",
        ));
    }
    let mut base_storage = Vec::new();
    for actual in actual_bases.iter() {
        if actual.as_ptr() == ptr::addr_of_mut!(ffi::PyBaseObject_Type).cast() {
            continue;
        }
        let base = for_actual_type(py, &actual)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "slots replacement lost an actual protected base")
        })?;
        base_storage.push(base.storage()?);
    }
    let slots = ObjectSlotPlan::prepare(
        fact,
        actual_namespace,
        actual_bases,
        base_storage
            .iter()
            .map(|storage| storage.data().layout.object_fields.as_slice()),
        true,
    )?
    .map_err(|_| {
        strict_runtime_unavailable(
            py,
            "slots replacement has an unsupported physical declaration",
        )
    })?;
    let source_storage = original.storage()?;
    let model = source_storage.data().model;
    let inherited = StorageLayout::merge(
        model,
        base_storage.iter().map(|storage| &storage.data().layout),
    )
    .map_err(|_| {
        strict_runtime_unavailable(py, "slots replacement has conflicting inherited prefixes")
    })?;
    let mut layout = source_storage.data().layout.clone();
    // Keep inherited dictionary positions even when a native member now hides
    // that spelling. No slot value is copied into that dictionary; explicit
    // dictionary writes keep their independent inherited write policy.
    layout.fields = inherited.fields;
    if slots.dictionary {
        for name in &source_storage.data().layout.fields {
            if !slots.names.contains(name) {
                layout.append(name);
            }
        }
    } else if !layout.fields.is_empty() {
        return Err(strict_runtime_unavailable(
            py,
            "slots replacement removed an inherited dictionary",
        ));
    }
    layout.object_fields = slots.names;
    layout.dictionary_bearing = slots.dictionary;
    layout.declared_slots = true;
    let namespace = proof.namespace();
    if namespace_admission(
        actual_namespace,
        fact,
        &layout,
        ptr::null_mut(),
        Some(&namespace),
    )
    .is_err()
        || !crate::strict_class::validate_actual_class_namespace(
            py,
            actual_namespace,
            verified,
            fact,
            // The invocation proved an exact copy of the already-adopted
            // original namespace. Its implicit wrappers were sealed by the
            // first type_new; they are not fresh source input descriptors.
            crate::strict_class::ClassNamespacePhase::Copied,
            original.namespace_execution(),
            Some(&namespace),
        )?
    {
        return Err(strict_runtime_unavailable(
            py,
            "slots replacement changed an admitted source or generated component",
        ));
    }
    let own_check = original
        .state
        .data()
        .own_field_checks
        .map(|index| source_storage.reference(index))
        .transpose()?;
    let own_check_identity = own_check.as_ref().map(|check| check.as_ptr());
    let mut checks = inherited_field_checks(&[source_storage])?;
    if let Some(own_check) = own_check {
        let own = checks
            .iter_mut()
            .find(|check| check.owner().as_ptr() == own_check.as_ptr())
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "replacement lost its own field policy")
            })?;
        // Reuse the declaring predicate, not the original's dictionary routing.
        // A native pending own-self predicate is bound only to the final type;
        // inherited dictionary obligations keep their actual declaring owner.
        own.dictionary_fields
            .retain(|name| layout.fields.contains(name) && !layout.object_fields.contains(name));
    }
    let object_offsets = layout.object_fields.iter().map(|_| Cell::new(-1)).collect();
    let storage = new_storage_owner(py, verified.interpreter_id(), model, layout, &checks)?;
    // A pending native slots replacement shares the original declaring check,
    // but its GC vector may have a different template offset/routing. Resolve
    // the actual check identity into THIS storage owner, never copy an index.
    let own_field_checks = own_check_identity
        .map(|identity| {
            checks
                .iter()
                .position(|check| check.owner().as_ptr() == identity)
                .map(|index| storage.data().check_reference(index))
                .ok_or_else(|| {
                    strict_runtime_unavailable(py, "replacement lost its declaring field check")
                })
        })
        .transpose()?;
    let mut references = vec![
        original.state.reference(MODULE_POLICY)?.unbind(),
        storage.owner().clone().unbind(),
        py.None(),
    ];
    let mut own_field_bindings = Vec::new();
    for &index in &original.state.data().own_field_bindings {
        let binding = StrictFieldBinding::from_owner(original.state.reference(index)?)?;
        if !binding.is_bound()? && !original.is_pending_type() {
            return Err(strict_runtime_unavailable(
                py,
                "slots replacement preceded its declaring-field binding",
            ));
        }
        own_field_bindings.push(references.len());
        references.push(binding.owner().clone().unbind());
    }
    let mut names = original.state.data().names.clone();
    names
        .protected
        .extend(proof.protected_names().map(str::to_owned));
    let dataclass = proof.attach(&mut references);
    let state = StrictStateRef::new(
        py,
        StrictClassData {
            verified: Arc::clone(verified),
            execution: original.state.data().execution.clone(),
            fact: fact.clone(),
            names,
            phase: Cell::new(ClassPhase::Prepared),
            actual_type: Cell::new(0),
            construction: Arc::new(ActualClassConstruction),
            construction_kind: ClassConstructionKind::DataclassSlotsReplacement,
            object_offsets,
            namespace_execution: Arc::clone(original.namespace_execution()),
            interpreter_invocation: original.state.data().interpreter_invocation.clone(),
            own_field_bindings,
            own_field_checks,
            dataclass: Some(dataclass),
            method_families: OnceLock::new(),
        },
        references,
    )?;
    Ok(StrictClassState {
        state,
        actual_type: None,
    })
}

fn new_storage_owner<'py>(
    py: Python<'py>,
    interpreter_id: i64,
    model: StorageModel,
    layout: StorageLayout,
    checks: &[SelectedStorageCheck<'py>],
) -> PyResult<StoragePolicyOwner<'py>> {
    require_interpreter(py, interpreter_id)?;
    let mut references =
        Vec::with_capacity(checks.len() + usize::from(model == StorageModel::Indexed));
    let template = if model == StorageModel::Indexed {
        let names = PyTuple::new(py, &layout.fields)?;
        let keys = unsafe { _PyDict_NewIndexedKeySet(names.as_ptr()) };
        if keys.is_null() {
            return Err(PyErr::fetch(py));
        }
        let template = unsafe { _PyDict_NewWithIndexedKeySet(keys) };
        unsafe {
            _PyDictKeys_DecRef(keys);
        }
        references.push(unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, template) }?.unbind());
        Some(0)
    } else {
        None
    };
    references.extend(checks.iter().map(|check| check.owner().clone().unbind()));
    StoragePolicyOwner::new(
        py,
        StoragePolicyData {
            interpreter_id,
            layout,
            model,
            template,
            dictionary_fields_by_check: checks
                .iter()
                .map(|check| check.dictionary_fields.clone())
                .collect(),
        },
        references,
    )
}

fn inherited_field_checks<'py>(
    owners: &[StoragePolicyOwner<'py>],
) -> PyResult<Vec<SelectedStorageCheck<'py>>> {
    let mut checks: Vec<SelectedStorageCheck<'py>> = Vec::new();
    for owner in owners {
        for (index, dictionary_fields) in owner.data().dictionary_fields_by_check.iter().enumerate()
        {
            let check = StrictFieldChecks::from_owner(
                owner.reference(owner.data().check_reference(index))?,
            )?;
            if let Some(existing) = checks
                .iter_mut()
                .find(|existing| existing.owner().as_ptr() == check.owner().as_ptr())
            {
                existing
                    .dictionary_fields
                    .extend(dictionary_fields.iter().cloned());
            } else {
                checks.push(SelectedStorageCheck {
                    check,
                    dictionary_fields: dictionary_fields.clone(),
                });
            }
        }
    }
    Ok(checks)
}

/// A shared ordinary dictionary may keep stronger existing obligations, but
/// every newly required field must use that actual predicate and route. A
/// minimal storage projection has its own GC shell, not a new actual binding.
/// Equal source types or names never merge distinct nominal factory bindings.
fn supports_dictionary_checks(
    existing: &StoragePolicyOwner<'_>,
    required: &[SelectedStorageCheck<'_>],
) -> PyResult<bool> {
    existing.ensure_live()?;
    if existing.data().model != StorageModel::Ordinary {
        return Ok(false);
    }
    for check in required {
        if check.dictionary_fields.is_empty() {
            continue;
        }
        let mut found = false;
        for (index, fields) in existing
            .data()
            .dictionary_fields_by_check
            .iter()
            .enumerate()
        {
            let previous = StrictFieldChecks::from_owner(
                existing.reference(existing.data().check_reference(index))?,
            )?;
            if previous.same_actual_check(&check.check)?
                && check.dictionary_fields.is_subset(fields)
            {
                found = true;
                break;
            }
        }
        if !found {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Build a dictionary-only view of bound actual predicates. A mask on a full
/// owner would enforce writes but still retain unrelated slot nominal targets.
/// Native caches this immutable projection per actual allocation type, not per
/// receiver. Legacy replacement dictionaries reuse the same rule semantics.
fn dictionary_storage_projection<'py>(
    owner: &StoragePolicyOwner<'py>,
) -> PyResult<Option<StoragePolicyOwner<'py>>> {
    owner.ensure_live()?;
    if owner.data().dictionary_mode() != 2 {
        return Ok(None);
    }
    let mut checks = Vec::new();
    let mut selected = BTreeSet::new();
    for check in inherited_field_checks(std::slice::from_ref(owner))? {
        if let Some(projected) = check.check.project_fields(&check.dictionary_fields)? {
            selected.extend(check.dictionary_fields.iter().cloned());
            checks.push(SelectedStorageCheck {
                check: projected,
                dictionary_fields: check.dictionary_fields,
            });
        }
    }
    let mut layout = owner.data().layout.clone();
    layout.fields.retain(|name| selected.contains(name));
    layout.object_fields.clear();
    layout.checks.retain(|name, _| selected.contains(name));
    layout.declared_slots = false;
    let projected = new_storage_owner(
        owner.owner().py(),
        owner.data().interpreter_id,
        StorageModel::Ordinary,
        layout,
        &checks,
    )?;
    owner.ensure_live()?;
    Ok(Some(projected))
}

/// One native declaring catalogue row keeps exactly the predicates used by
/// that original declaration's callback for this canonical field. Do not
/// combine same-spelling slots from different physical offsets or factories.
fn member_storage_projection<'py>(
    owner: &StoragePolicyOwner<'py>,
    name: &str,
) -> PyResult<StoragePolicyOwner<'py>> {
    owner.ensure_live()?;
    let fields = BTreeSet::from([name.to_owned()]);
    let mut checks = Vec::new();
    for check in inherited_field_checks(std::slice::from_ref(owner))? {
        if check.check.contains_field(name) {
            checks.push(SelectedStorageCheck {
                check: check.check.project_fields(&fields)?.ok_or_else(|| {
                    strict_runtime_unavailable(
                        owner.owner().py(),
                        "native member projection is empty",
                    )
                })?,
                dictionary_fields: BTreeSet::new(),
            });
        }
    }
    let projected = new_storage_owner(
        owner.owner().py(),
        owner.data().interpreter_id,
        StorageModel::Ordinary,
        StorageLayout {
            fields: Vec::new(),
            object_fields: vec![name.to_owned()],
            dictionary_bearing: false,
            declared_slots: true,
            checks: BTreeMap::new(),
        },
        &checks,
    )?;
    owner.ensure_live()?;
    Ok(projected)
}

fn new_policy_dictionary<'py>(owner: &StoragePolicyOwner<'py>) -> PyResult<Bound<'py, PyAny>> {
    owner.ensure_live()?;
    let py = owner.owner().py();
    require_interpreter(py, owner.data().interpreter_id)?;
    let index = owner.data().template.ok_or_else(|| {
        strict_runtime_unavailable(
            py,
            "ordinary field storage has no indexed dictionary factory",
        )
    })?;
    let template = owner.reference(index)?;
    let dictionary = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(py, _PyDict_NewFromIndexedSchema(template.as_ptr()))?
    };
    if unsafe {
        PyDict_SetSoacPolicy(
            dictionary.as_ptr(),
            owner.owner().as_ptr(),
            validate_instance_dictionary,
            ALLOW_NONSTRING_KEYS,
        )
    } < 0
    {
        return Err(PyErr::fetch(py));
    }
    Ok(dictionary)
}

/// Resolve actual native declarations, not names from a source catalogue. A
/// temporary MRO pin supports cold GC-capable projection preparation, but no
/// receiver/type/MRO is retained by the returned storage policies.
fn storage_classes_for_type<'py>(
    state: &StrictClassState<'py>,
    actual_type: Borrowed<'_, 'py, PyAny>,
) -> PyResult<Vec<StrictClassState<'py>>> {
    state.ensure_live()?;
    let py = actual_type.py();
    if unsafe { ffi::PyType_Check(actual_type.as_ptr()) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "storage preparation has no actual type",
        ));
    }

    // Ordinary subclasses inherit physical requirements, not strict receiver
    // authority. Inspect the real MRO, never Python __mro__/__class__ hooks.
    let mro = unsafe { (*actual_type.as_ptr().cast::<ffi::PyTypeObject>()).tp_mro };
    if mro.is_null() || unsafe { ffi::PyTuple_CheckExact(mro) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "instance storage allocation preceded its actual MRO",
        ));
    }
    let mro = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, mro) }.cast_into::<PyTuple>()?;
    let mut classes = Vec::new();
    let mut requested_owner_found = false;
    for base in mro.iter() {
        if let Some(base) = for_actual_type(py, &base)? {
            requested_owner_found |= base.owner().as_ptr() == state.owner().as_ptr();
            classes.push(base);
        }
    }
    if !requested_owner_found {
        return Err(strict_runtime_unavailable(
            py,
            "instance factory owner is absent from its actual MRO",
        ));
    }
    Ok(classes)
}

/// Legacy no-trailer storage still uses the same actual declarations. The
/// native direct-state path never calls this receiver/MRO-discovery helper.
fn instance_storage_owners<'py>(
    state: &StrictClassState<'py>,
    instance: Borrowed<'_, 'py, PyAny>,
) -> PyResult<Vec<StoragePolicyOwner<'py>>> {
    state.ensure_live()?;
    let py = instance.py();
    let actual_type =
        unsafe { Borrowed::<PyAny>::from_ptr(py, ffi::Py_TYPE(instance.as_ptr()).cast()) };
    if actual_type.as_ptr() as usize == state.state.data().actual_type.get() {
        require_actual_owner(py, &actual_type, state.owner())?;
        return Ok(vec![state.storage()?]);
    }
    storage_classes_for_type(state, actual_type)?
        .iter()
        .map(StrictClassState::storage)
        .collect()
}

fn policy_from_storage_owners<'py>(
    state: &StrictClassState<'py>,
    owners: Vec<StoragePolicyOwner<'py>>,
) -> PyResult<StoragePolicyOwner<'py>> {
    let py = state.owner().py();
    let model = state.storage()?.data().model;
    if owners.iter().any(|owner| {
        owner.data().model != model
            && owner.data().dictionary_mode() != 0
            && !owner.data().layout.fields.is_empty()
    }) {
        return Err(mutation_error(
            py,
            c"incompatible ordinary/indexed instance dictionary requirements",
        ));
    }
    let layout = StorageLayout::merge(model, owners.iter().map(|owner| &owner.data().layout))
        .map_err(|_| {
            mutation_error(
                py,
                c"ordinary subclass has incompatible inherited strict field prefixes",
            )
        })?;
    let checks = inherited_field_checks(&owners)?;
    for owner in owners {
        if owner.data().model == model
            && owner.data().layout == layout
            && owner.data().dictionary_fields_by_check.len() == checks.len()
        {
            let mut same = true;
            for (index, check) in checks.iter().enumerate() {
                same &= owner
                    .reference(owner.data().check_reference(index))?
                    .as_ptr()
                    == check.owner().as_ptr();
                same &= owner.data().dictionary_fields_by_check[index] == check.dictionary_fields;
            }
            if same {
                return Ok(owner);
            }
        }
    }
    // The cold multiple-inheritance path needs a combined policy, but never a
    // strong receiver-type edge. A detached dict must not prolong class life.
    new_storage_owner(
        py,
        state.verified_module().interpreter_id(),
        model,
        layout,
        &checks,
    )
}

fn policy_for_instance<'py>(
    state: &StrictClassState<'py>,
    instance: Borrowed<'_, 'py, PyAny>,
) -> PyResult<StoragePolicyOwner<'py>> {
    policy_from_storage_owners(state, instance_storage_owners(state, instance)?)
}

/// Native invokes this registered factory only for a fresh supported default
/// allocation after final admission. Preparation uses actual declaration
/// owners and physical slots; the result contains no receiver lookup recipe.
unsafe extern "C" fn prepare_storage_state(
    owner: *mut ffi::PyObject,
    actual_type: *mut ffi::PyObject,
    out: *mut *mut RawPyTypeState,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    if !out.is_null() {
        unsafe {
            out.write(ptr::null_mut());
        }
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<()> {
        if owner.is_null()
            || actual_type.is_null()
            || out.is_null()
            || unsafe { ffi::PyType_Check(actual_type) } == 0
        {
            return Err(strict_runtime_unavailable(
                py,
                "invalid storage state factory operands",
            ));
        }
        let state = StrictClassState {
            state: StrictStateRef::from_owner(unsafe { Bound::from_borrowed_ptr(py, owner) })?,
            actual_type: None,
        };
        state.ensure_live()?;
        if !matches!(
            state.state.data().phase.get(),
            ClassPhase::Bound | ClassPhase::Sealed
        ) {
            return Err(strict_runtime_unavailable(
                py,
                "storage state preceded final type admission",
            ));
        }
        let actual = unsafe { Borrowed::<PyAny>::from_ptr(py, actual_type) };
        let native_type = actual_type.cast::<ffi::PyTypeObject>();
        let mro = unsafe { (*native_type).tp_mro };
        if mro.is_null() || unsafe { ffi::PyTuple_CheckExact(mro) } == 0 {
            return Err(strict_runtime_unavailable(
                py,
                "storage state has no actual native MRO",
            ));
        }
        let mro = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, mro) };
        let classes = storage_classes_for_type(&state, actual)?;
        let owners = classes
            .iter()
            .map(StrictClassState::storage)
            .collect::<PyResult<Vec<_>>>()?;
        let effective = policy_from_storage_owners(&state, owners)?;
        if effective.data().model != StorageModel::Ordinary {
            return Err(strict_runtime_unavailable(
                py,
                "direct type state requires ordinary source storage",
            ));
        }
        let dictionary = dictionary_storage_projection(&effective)?;
        validate_storage_preparation(&classes, native_type, &mro)?;
        let mut rows = Vec::new();
        let mut row_support = Vec::new();
        for declaring in &classes {
            let storage = declaring.storage()?;
            for (index, name) in storage.data().layout.object_fields.iter().enumerate() {
                let reserved_offset = declaring
                    .state
                    .data()
                    .object_offsets
                    .get(index)
                    .ok_or_else(|| {
                        strict_runtime_unavailable(
                            py,
                            "storage state has no reserved declaring slot",
                        )
                    })?
                    .get();
                let index = ffi::Py_ssize_t::try_from(index).map_err(|_| {
                    strict_runtime_unavailable(py, "native slot index exceeds the ABI")
                })?;
                let mut offset = -1;
                let matched = unsafe {
                    PyType_GetSoacObjectSlotOffset(
                        actual_type,
                        declaring.owner().as_ptr(),
                        index,
                        &mut offset,
                    )
                };
                if matched < 0 {
                    return Err(PyErr::fetch(py));
                }
                if matched != 1 || offset < 0 || reserved_offset != offset {
                    return Err(strict_runtime_unavailable(
                        py,
                        "storage state lost its actual declaring slot",
                    ));
                }
                let name_size = ffi::Py_ssize_t::try_from(name.len()).map_err(|_| {
                    strict_runtime_unavailable(py, "native slot name exceeds the ABI")
                })?;
                let canonical_name = unsafe {
                    Bound::<PyAny>::from_owned_ptr_or_err(
                        py,
                        ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name_size),
                    )?
                }
                .cast_into::<PyString>()?;
                validate_storage_preparation(&classes, native_type, &mro)?;
                let rule_owner = member_storage_projection(&storage, name)?;
                validate_storage_preparation(&classes, native_type, &mro)?;
                rows.push(RawPyTypeStateSlotSpecV1 {
                    expected_class_owner: declaring.owner().as_ptr(),
                    field_index: index,
                    offset,
                    canonical_name: canonical_name.as_ptr(),
                    rule_owner: rule_owner.owner().as_ptr(),
                    validate: Some(validate_type_state_member),
                });
                row_support.push((canonical_name, rule_owner));
            }
        }
        validate_storage_preparation(&classes, native_type, &mro)?;
        if dictionary.is_none() && rows.is_empty() {
            return Ok(());
        }
        let spec = RawPyTypeStateSpecV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<RawPyTypeStateSpecV1>() as u32,
            dictionary_owner: dictionary
                .as_ref()
                .map_or(ptr::null_mut(), |owner| owner.owner().as_ptr()),
            validate_dictionary: dictionary
                .as_ref()
                .map(|_| validate_instance_dictionary as InstanceDictionaryValidator),
            validate_inline: dictionary
                .as_ref()
                .map(|_| validate_type_state_inline as TypeStateFieldValidator),
            slot_count: ffi::Py_ssize_t::try_from(rows.len()).map_err(|_| {
                strict_runtime_unavailable(py, "native slot catalogue exceeds the ABI")
            })?,
            slots: if rows.is_empty() {
                ptr::null()
            } else {
                rows.as_ptr()
            },
        };
        let prepared = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                PyTypeState_NewV1(actual_type, &spec, std::mem::size_of_val(&spec)).cast(),
            )?
        };
        // Native independently validates completeness and rechecks allocation
        // eligibility/type version before cache publication. Keep Rust source
        // owners and the original MRO live through its GC-capable constructor.
        validate_storage_preparation(&classes, native_type, &mro)?;
        drop(row_support);
        unsafe {
            out.write(prepared.into_ptr().cast());
        }
        Ok(())
    }));
    callback_status(py, result, c"panic in strict storage state preparation")
}

fn validate_storage_preparation(
    classes: &[StrictClassState<'_>],
    actual_type: *mut ffi::PyTypeObject,
    mro: &Bound<'_, PyAny>,
) -> PyResult<()> {
    for declaring in classes {
        declaring.ensure_live()?;
        if !matches!(
            declaring.state.data().phase.get(),
            ClassPhase::Bound | ClassPhase::Sealed
        ) {
            return Err(strict_runtime_unavailable(
                mro.py(),
                "storage state lost an admitted declaring owner",
            ));
        }
    }
    if unsafe { (*actual_type).tp_mro } != mro.as_ptr() {
        return Err(strict_runtime_unavailable(
            mro.py(),
            "actual MRO changed during storage preparation",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldWriteStorage {
    Dictionary,
    NativeObjectMember,
}

unsafe fn validate_prepared_field(
    owner: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
    storage: FieldWriteStorage,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<()> {
        if owner.is_null() || name.is_null() || unsafe { ffi::PyUnicode_Check(name) } == 0 {
            return Err(strict_runtime_unavailable(
                py,
                "invalid prepared storage write operands",
            ));
        }
        let owner = StoragePolicyOwner::from_owner(unsafe { Bound::from_borrowed_ptr(py, owner) })?;
        owner.ensure_live()?;
        require_interpreter(py, owner.data().interpreter_id)?;
        if !value.is_null() {
            let name = unsafe { Borrowed::<PyAny>::from_ptr(py, name) };
            let value = unsafe { Borrowed::<PyAny>::from_ptr(py, value) };
            check_unicode_field_value(&owner, (&*name).cast::<PyString>()?, &value, storage)?;
        }
        Ok(())
    }));
    callback_status(py, result, c"panic in prepared strict storage write")
}

unsafe extern "C" fn validate_type_state_inline(
    owner: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
) -> c_int {
    unsafe { validate_prepared_field(owner, name, value, FieldWriteStorage::Dictionary) }
}

unsafe extern "C" fn validate_type_state_member(
    owner: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
) -> c_int {
    unsafe { validate_prepared_field(owner, name, value, FieldWriteStorage::NativeObjectMember) }
}

fn check_value(
    owner: &StoragePolicyOwner<'_>,
    key: &Bound<'_, PyAny>,
    value: &Bound<'_, PyAny>,
) -> PyResult<()> {
    if owner.data().dictionary_fields_by_check.is_empty() {
        return Ok(());
    }
    // A stored Unicode subclass still names a selected field. Exactness is
    // not permission to skip its predicate after canonical-key resolution.
    if unsafe { ffi::PyUnicode_Check(key.as_ptr()) } == 0 {
        // The native kernel passes the once-resolved canonical stored key.
        // New non-string keys stay ordinary overflow. Their mutable equality
        // requires NoLookupAliases or a fresh resolved-value guard on reads;
        // this write policy does not manufacture that separate value proof.
        return Ok(());
    }
    check_unicode_field_value(
        owner,
        key.cast::<PyString>()?,
        value,
        FieldWriteStorage::Dictionary,
    )
}

/// Interpret the Unicode payload directly. In particular, an attribute name
/// may be a str subclass; inspecting it must not call its conversion, hash, or
/// equality hooks again after the native dictionary's one guarded lookup.
fn check_unicode_field_value(
    owner: &StoragePolicyOwner<'_>,
    key: &Bound<'_, PyString>,
    value: &Bound<'_, PyAny>,
    storage: FieldWriteStorage,
) -> PyResult<()> {
    if owner.data().dictionary_fields_by_check.is_empty() {
        return Ok(());
    }
    let name = match key.to_str() {
        Ok(name) => name,
        Err(error) if error.is_instance_of::<PyUnicodeEncodeError>(key.py()) => return Ok(()),
        Err(error) => return Err(error),
    };
    if storage == FieldWriteStorage::Dictionary
        && !owner.data().layout.fields.iter().any(|field| field == name)
    {
        // A dictionary-bearing slotted receiver can contain a hidden mapping
        // entry with the member's spelling. It is not the member value. Only
        // an independently inherited dictionary-prefix obligation constrains
        // such an entry; never silently mirror native member requirements.
        return Ok(());
    }
    for (index, dictionary_fields) in owner.data().dictionary_fields_by_check.iter().enumerate() {
        if storage != FieldWriteStorage::Dictionary || dictionary_fields.contains(name) {
            StrictFieldChecks::from_owner(owner.reference(owner.data().check_reference(index))?)?
                .check(name, value)?;
        }
    }
    Ok(())
}

fn mutation_error(py: Python<'_>, message: &CStr) -> PyErr {
    let kind = unsafe { PySoac_GetStrictMutationError() };
    if !kind.is_null() {
        unsafe {
            ffi::PyErr_SetString(kind, message.as_ptr());
        }
    }
    PyErr::fetch(py)
}

fn callback_status(
    py: Python<'_>,
    result: std::thread::Result<PyResult<()>>,
    panic_message: &CStr,
) -> c_int {
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            error.restore(py);
            -1
        }
        Err(_) => {
            unsafe {
                ffi::PyErr_SetString(ffi::PyExc_SystemError, panic_message.as_ptr());
            }
            -1
        }
    }
}

/// Reentrant preparation under the already installed native pending barrier.
/// The copied namespace is real, but type_new_set_attrs has not made implicit
/// wrappers, released __classcell__, installed members, or run Ready callbacks.
/// Namespace witnesses and fresh explicit descriptor seals bind here; direct-self
/// and physical constraints wait for the final decorated type's admission.
pub(crate) unsafe extern "C" fn bind_pending_type(
    owner: *mut ffi::PyObject,
    actual_type: *mut ffi::PyObject,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<()> {
        if owner.is_null() || actual_type.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null pending type binding operand",
            ));
        }
        let state = StrictStateRef::<StrictClassData>::from_owner(unsafe {
            Bound::from_borrowed_ptr(py, owner)
        })?;
        let actual_type = unsafe { Borrowed::<PyAny>::from_ptr(py, actual_type) };
        let initial = native_construction_info(py, &actual_type)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "pending bind has no native construction")
        })?;
        if state.data().phase.get() != ClassPhase::Prepared
            || initial.phase != NATIVE_TYPE_PENDING
            || initial.permanent_contract_published != 0
            || initial.owner != owner
            || initial.root_construction.is_null()
        {
            return Err(strict_runtime_unavailable(
                py,
                "pending type binding was replayed or is foreign",
            ));
        }
        // Register before the first allocating callback. Write-once native
        // registration rejects an earlier foreign registration and remains
        // closed after this bind; exposed owner metadata cannot replace it.
        if unsafe {
            PyType_SetSoacStorageStateFactoryV1(actual_type.as_ptr(), owner, prepare_storage_state)
        } < 0
        {
            return Err(PyErr::fetch(py));
        }
        let witness = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                ffi::PyWeakref_NewRef(actual_type.as_ptr(), ptr::null_mut()),
            )?
        };
        let current = native_construction_info(py, &actual_type)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "pending construction was cleared during preparation")
        })?;
        if state.data().phase.get() != ClassPhase::Prepared
            || current.phase != NATIVE_TYPE_PENDING
            || current.permanent_contract_published != 0
            || current.owner != owner
            || current.root_construction != initial.root_construction
        {
            return Err(strict_runtime_unavailable(
                py,
                "pending construction changed while preparing its weak witness",
            ));
        }
        let dictionary = unsafe { (*actual_type.as_ptr().cast::<ffi::PyTypeObject>()).tp_dict };
        if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
            return Err(strict_runtime_unavailable(
                py,
                "pending type has no actual copied namespace",
            ));
        }
        let namespace =
            unsafe { Borrowed::<PyAny>::from_ptr(py, dictionary).cast_unchecked::<PyDict>() };
        let dataclass = dataclass_namespace(&state)?;
        let namespace_phase =
            if state.data().construction_kind == ClassConstructionKind::SourceNamespace {
                crate::strict_class::ClassNamespacePhase::Input
            } else {
                crate::strict_class::ClassNamespacePhase::Copied
            };
        if !crate::strict_class::validate_actual_class_namespace(
            py,
            &namespace,
            &state.data().verified,
            &state.data().fact,
            namespace_phase,
            &state.data().namespace_execution,
            dataclass.as_ref(),
        )? {
            return Err(strict_runtime_unavailable(
                py,
                "pending copied namespace changed before early binding",
            ));
        }
        state.data().execution.validate_owner(
            py,
            &state.reference(MODULE_POLICY)?,
            &state.data().verified,
        )?;
        // This actual input namespace has been checked after weak-type
        // allocation and still has its unbound adapter phase. Seal fresh
        // explicit descriptor births before adapter binding advances that
        // phase or type_new can run __set_name__/__init_subclass__.
        // Native seals implicit wrappers when it creates them later.
        crate::strict_class::adopt_class_descriptors(
            py,
            &namespace,
            &state.data().fact,
            &state.data().namespace_execution,
            namespace_phase,
        )?;
        state.bind_reserved_reference(ACTUAL_TYPE_WEAKREF, witness.clone())?;
        if state.data().construction_kind == ClassConstructionKind::SourceNamespace {
            unsafe {
                crate::strict_class::bind_pending_namespace_function_witnesses(
                    py,
                    &namespace,
                    &state.data().fact,
                    &state.data().namespace_execution,
                    &witness,
                )?;
            }
        }
        if let Some(dataclass) = &dataclass {
            dataclass.bind_class(&actual_type, state.owner())?;
        }
        // Dataclass weak-witness preparation is also reentrant. Revalidate the
        // actual sidecar after it, before publishing the shared Rust witness.
        let current = native_construction_info(py, &actual_type)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "pending constructor disappeared during adapter binding")
        })?;
        if current.phase != NATIVE_TYPE_PENDING
            || current.owner != owner
            || current.root_construction != initial.root_construction
            || state.data().phase.get() != ClassPhase::Prepared
        {
            return Err(strict_runtime_unavailable(
                py,
                "pending constructor changed during adapter binding",
            ));
        }
        if state.data().construction_kind == ClassConstructionKind::SourceNamespace {
            state.data().namespace_execution.record_class_dictionary(
                py,
                state.owner(),
                namespace.as_any(),
            )?;
        }
        state.data().actual_type.set(actual_type.as_ptr() as usize);
        state.data().phase.set(ClassPhase::Pending);
        if let Some(invocation) = state.data().interpreter_invocation.get() {
            state.data().execution.register_interpreter_pending(
                py,
                &state.reference(MODULE_POLICY)?,
                &state.data().verified,
                crate::StrictPendingKind::Class {
                    source: state.data().fact.identity.clone(),
                },
                &actual_type,
                invocation,
            )?;
        }
        // Retained construct/Apply owns the actual result and its active
        // globals through registration/completion. Queue its existing weak
        // receipt there, never by recovering a dictionary from an address.
        Ok(())
    }));
    callback_status(py, result, c"panic in pending type preparation")
}

/// Match exact immutable metadata without hashing, user equality, allocating
/// Python names, or trusting the addresses of caller-supplied tuple contents.
fn contract_names_match<'a>(
    py: Python<'_>,
    tuple: *mut ffi::PyObject,
    names: impl ExactSizeIterator<Item = &'a String>,
) -> bool {
    if tuple.is_null()
        || unsafe { ffi::PyTuple_CheckExact(tuple) } == 0
        || unsafe { ffi::PyTuple_Size(tuple) } != names.len() as ffi::Py_ssize_t
    {
        return false;
    }
    names.enumerate().all(|(index, expected)| {
        let key = unsafe { ffi::PyTuple_GetItem(tuple, index as ffi::Py_ssize_t) };
        if key.is_null() {
            return false;
        }
        let key = unsafe { Borrowed::<PyAny>::from_ptr(py, key) };
        exact_name_matches(&key, expected)
    })
}

/// The only final validator captured by this actual pending construction.
/// Native has prepared permanent policies while keeping instance admission
/// closed. Never accept a caller's weaker payload merely because its exposed
/// owner and source identities match. Successful publication is callback-free.
pub(crate) unsafe extern "C" fn commit_pending_type(
    owner: *mut ffi::PyObject,
    actual_type: *mut ffi::PyObject,
    actual_contract: *const crate::strict_class::RawSoacTypeContractSpec,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<()> {
        if owner.is_null() || actual_type.is_null() || actual_contract.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null final type commit operand",
            ));
        }
        let state = StrictClassState {
            state: StrictStateRef::<StrictClassData>::from_owner(unsafe {
                Bound::from_borrowed_ptr(py, owner)
            })?,
            actual_type: Some(unsafe { Bound::from_borrowed_ptr(py, actual_type) }),
        };
        state.ensure_live()?;
        let actual = state.actual_type()?;
        let info = native_construction_info(py, &actual)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "final commit lost its actual pending construction")
        })?;
        if state.state.data().phase.get() != ClassPhase::Admitting
            || info.phase != NATIVE_TYPE_ADMITTING
            || info.permanent_contract_published != 1
            || info.owner != owner
            || info.root_construction.is_null()
        {
            return Err(strict_runtime_unavailable(
                py,
                "final commit lacks selected admission authority",
            ));
        }
        let contract = unsafe { &*actual_contract };
        let storage = state.storage()?;
        let mode = state.dictionary_mode()?;
        let expected_flags =
            u32::from(state.fact().openness == soac_contracts::ClassOpenness::DeclaredFinal);
        if contract.flags != expected_flags
            || contract.dictionary_mode != mode
            || !contract_names_match(py, contract.fields, storage.data().layout.fields.iter())
            || !contract_names_match(
                py,
                contract.object_slot_fields,
                storage.data().layout.object_fields.iter(),
            )
            || !contract_names_match(
                py,
                contract.protected_names,
                state.state.data().names.protected.iter(),
            )
            || !contract_names_match(
                py,
                contract.final_methods,
                state.state.data().names.final_methods.iter(),
            )
            || contract
                .check_instance_write
                .map(|callback| callback as usize)
                != Some(check_instance_write as *const () as usize)
            || contract.new_instance_dict.map(|callback| callback as usize)
                != (mode == 1).then_some(new_instance_dict as *const () as usize)
            || contract
                .prepare_instance_dictionary_policy
                .map(|callback| callback as usize)
                != (mode == 2).then_some(prepare_instance_dictionary_policy as *const () as usize)
        {
            return Err(strict_runtime_unavailable(
                py,
                "final native policy differs from selected immutable requirements",
            ));
        }
        // All function/provider/descriptor adoption happened before NativeAdmit.
        // Its policy construction and cache notifications can reenter, so check
        // the actual final dictionary again at this last callback-free point.
        validate_copied_namespace(&state.state, &actual)?;
        for (index, reserved) in state.state.data().object_offsets.iter().enumerate() {
            let mut offset = -1;
            let matched = unsafe {
                PyType_GetSoacObjectSlotOffset(
                    actual.as_ptr(),
                    owner,
                    index as ffi::Py_ssize_t,
                    &mut offset,
                )
            };
            if matched < 0 {
                return Err(PyErr::fetch(py));
            }
            if matched != 1 || offset < 0 || reserved.get() != -1 {
                return Err(strict_runtime_unavailable(
                    py,
                    "final native member lost its actual physical location",
                ));
            }
            reserved.set(offset);
        }
        // This Rust-only transition is last. Native still holds every support
        // operand and opens allocation only after this callback returns.
        state.state.data().phase.set(ClassPhase::Bound);
        Ok(())
    }));
    callback_status(py, result, c"panic in final strict type admission")
}

pub(crate) unsafe extern "C" fn new_instance_dict(
    owner: *mut ffi::PyObject,
    instance: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<Bound<'_, PyAny>> {
        if owner.is_null() || instance.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null native instance factory operand",
            ));
        }
        let state = StrictClassState {
            state: StrictStateRef::from_owner(unsafe { Bound::from_borrowed_ptr(py, owner) })?,
            actual_type: None,
        };
        let instance = unsafe { Borrowed::<PyAny>::from_ptr(py, instance) };
        new_policy_dictionary(&policy_for_instance(&state, instance)?)
    }));
    match result {
        Ok(Ok(dictionary)) => dictionary.into_ptr(),
        Ok(Err(error)) => {
            error.restore(py);
            ptr::null_mut()
        }
        Err(_) => {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_SystemError,
                    c"panic in strict instance dictionary factory".as_ptr(),
                );
            }
            ptr::null_mut()
        }
    }
}

/// Return only one GC-visible metadata edge. Native code owns the actual
/// dictionary and its prepare/commit/abort transaction; this factory neither
/// attaches a policy nor fabricates a dictionary or retains the receiver.
pub(crate) unsafe extern "C" fn prepare_instance_dictionary_policy(
    owner: *mut ffi::PyObject,
    instance: *mut ffi::PyObject,
    dictionary: *mut ffi::PyObject,
    existing: *const RawPySoacInstanceDictPolicy,
    out: *mut RawPySoacInstanceDictPolicy,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    if !out.is_null() {
        unsafe {
            out.write(RawPySoacInstanceDictPolicy {
                owner: ptr::null_mut(),
                validate: None,
            });
        }
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<()> {
        if owner.is_null()
            || instance.is_null()
            || dictionary.is_null()
            || out.is_null()
            || unsafe { ffi::PyDict_Check(dictionary) } == 0
        {
            return Err(strict_runtime_unavailable(
                py,
                "invalid ordinary dictionary factory operands",
            ));
        }
        let state = StrictClassState {
            state: StrictStateRef::from_owner(unsafe { Bound::from_borrowed_ptr(py, owner) })?,
            actual_type: None,
        };
        let instance = unsafe { Borrowed::<PyAny>::from_ptr(py, instance) };
        let required = dictionary_storage_projection(&policy_for_instance(&state, instance)?)?
            .ok_or_else(|| {
                strict_runtime_unavailable(
                    py,
                    "ordinary dictionary factory has no selected field policy",
                )
            })?;
        let selected = if existing.is_null() {
            required
        } else {
            // The candidate's native policy supports this borrowed view; it
            // remains guarded while Rust prepares metadata. Never acquire a
            // candidate dictionary or receiver primary to keep that support.
            let view = unsafe { &*existing };
            if view.owner.is_null()
                || !view.validate.is_some_and(|callback| {
                    std::ptr::fn_addr_eq(
                        callback,
                        validate_instance_dictionary as InstanceDictionaryValidator,
                    )
                })
            {
                return Err(mutation_error(
                    py,
                    c"incompatible existing instance dictionary policy",
                ));
            }
            let previous = StoragePolicyOwner::from_owner(unsafe {
                Bound::from_borrowed_ptr(py, view.owner)
            })?;
            let checks = inherited_field_checks(&[required])?;
            if !supports_dictionary_checks(&previous, &checks)? {
                return Err(mutation_error(
                    py,
                    c"incompatible existing instance dictionary field owners",
                ));
            }
            previous
        };
        state.ensure_live()?;
        selected.ensure_live()?;
        unsafe {
            out.write(RawPySoacInstanceDictPolicy {
                owner: selected.owner().clone().into_ptr(),
                validate: Some(validate_instance_dictionary),
            });
        }
        Ok(())
    }));
    callback_status(
        py,
        result,
        c"panic in ordinary dictionary policy preparation",
    )
}

pub(crate) unsafe extern "C" fn check_instance_write(
    owner: *mut ffi::PyObject,
    instance: *mut ffi::PyObject,
    name: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<()> {
        if owner.is_null() || instance.is_null() || name.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null native instance write operand",
            ));
        }
        let state = StrictClassState {
            state: StrictStateRef::from_owner(unsafe { Bound::from_borrowed_ptr(py, owner) })?,
            actual_type: None,
        };
        state.ensure_live()?;
        let actual_type = unsafe { Borrowed::<PyAny>::from_ptr(py, ffi::Py_TYPE(instance).cast()) };
        let storage = state.storage()?;
        let name = unsafe { Borrowed::<PyAny>::from_ptr(py, name) };
        if let Some(index) = storage
            .data()
            .layout
            .object_fields
            .iter()
            .position(|expected| exact_name_matches(&name, expected))
        {
            // Native members supply their canonical name after selecting a
            // physical offset. An ordinary subclass inherits this obligation,
            // not the declaring class's strict dispatch authority.
            let mut offset = -1;
            let matched = unsafe {
                PyType_GetSoacObjectSlotOffset(
                    actual_type.as_ptr(),
                    state.owner().as_ptr(),
                    index as ffi::Py_ssize_t,
                    &mut offset,
                )
            };
            if matched < 0 {
                return Err(PyErr::fetch(py));
            }
            if matched != 1 || state.state.data().object_offsets[index].get() != offset {
                return Err(strict_runtime_unavailable(
                    py,
                    "object-slot write lost its actual physical owner",
                ));
            }
            if !value.is_null() {
                let value = unsafe { Borrowed::<PyAny>::from_ptr(py, value) };
                check_unicode_field_value(
                    &storage,
                    (&*name).cast::<PyString>()?,
                    &value,
                    FieldWriteStorage::NativeObjectMember,
                )?;
            }
            return Ok(());
        }
        if storage.data().model == StorageModel::Ordinary {
            // Only the native selected inline commit calls this ordinary
            // branch; generic descriptor/hash/key lookup has already happened.
            let instance = unsafe { Borrowed::<PyAny>::from_ptr(py, instance) };
            let owners = instance_storage_owners(&state, instance)?;
            if !value.is_null() {
                let value = unsafe { Borrowed::<PyAny>::from_ptr(py, value) };
                for storage in owners {
                    check_unicode_field_value(
                        &storage,
                        (&*name).cast::<PyString>()?,
                        &value,
                        FieldWriteStorage::Dictionary,
                    )?;
                }
            }
            return Ok(());
        }
        require_actual_owner(py, &actual_type, state.owner())?;
        if state.state.data().actual_type.get() != actual_type.as_ptr() as usize {
            return Err(strict_runtime_unavailable(
                py,
                "instance write uses another class state",
            ));
        }
        // Do not check values before dictionary lookup. That would mask a
        // key's hash/equality exception and miss str-subclass name payloads.
        // The instance dictionary's ATTRIBUTE_SET transaction checks both
        // the original name and once-resolved canonical key before commit.
        Ok(())
    }));
    callback_status(py, result, c"panic in strict instance write policy")
}

unsafe extern "C" fn validate_instance_dictionary(
    owner: *mut ffi::PyObject,
    dictionary: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
    operation: c_int,
    provenance: *mut ffi::PyObject,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<()> {
        if owner.is_null() || dictionary.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "null native dictionary policy operand",
            ));
        }
        let raw_owner = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, owner) };
        if operation == TERMINAL_TEARDOWN {
            // This dictionary is dying, not the shared class/storage policy.
            // Other instances and escaped dictionaries must remain usable.
            StoragePolicyOwner::from_owner_for_teardown(raw_owner)?;
            return Ok(());
        }
        let owner = StoragePolicyOwner::from_owner(raw_owner)?;
        require_interpreter(py, owner.data().interpreter_id)?;
        let attribute = matches!(operation, ATTRIBUTE_SET | ATTRIBUTE_SET_EXISTING);
        let valid_provenance = if attribute {
            !provenance.is_null() && (unsafe { ffi::PyUnicode_Check(provenance) }) != 0
        } else {
            provenance.is_null()
                && matches!(
                    operation,
                    VALIDATE_INITIAL | SET | SET_EXISTING | DELETE | CLEAR
                )
        };
        if !valid_provenance {
            return Err(mutation_error(
                py,
                c"instance dictionary has no private namespace mutation permit",
            ));
        }
        if matches!(operation, DELETE | CLEAR) {
            return Ok(());
        }
        if key.is_null() || value.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "instance dictionary write has no canonical key/value",
            ));
        }
        let key = unsafe { Borrowed::<PyAny>::from_ptr(py, key) };
        let value = unsafe { Borrowed::<PyAny>::from_ptr(py, value) };
        check_value(&owner, &key, &value)?;
        if attribute {
            let name = unsafe { Borrowed::<PyAny>::from_ptr(py, provenance) };
            check_unicode_field_value(
                &owner,
                (&*name).cast::<PyString>()?,
                &value,
                FieldWriteStorage::Dictionary,
            )?;
        }
        Ok(())
    }));
    callback_status(py, result, c"panic in strict instance dictionary policy")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use pyo3::exceptions::PyTypeError;
    use pyo3::types::PyModule;
    use soac_contracts::{
        AnnotationOrigin, ArtifactEnvironment, ArtifactExpectations, ArtifactSigningKey,
        BuiltinType, CallableSignature, CheckedFieldPolicy, ClassMemberFact, ClassOpenness,
        ClassPolicyOverride, ClassReference, ConservativeAnalysis, DefaultFact, DefinitionKind,
        DescriptorFact, FieldReadPolicy, FieldTypeFact, FieldWritePolicy, Fingerprint,
        FunctionTypeFact, InheritanceFact, InitializationPolicy, MethodTypeFact,
        ModuleArtifactIndex, ModuleContentId, ModuleTypeFacts, OverridePolicy, ParameterKind,
        ParameterTypeFact, PythonVersion, SourceDialect, SourceRange, TypeArtifactManifest,
        encode_module_shard, sign_manifest, verify_manifest,
    };

    unsafe extern "C" {
        fn PyClassMethod_New(callable: *mut ffi::PyObject) -> *mut ffi::PyObject;
        fn _PyObject_GenericSetAttrWithDict(
            receiver: *mut ffi::PyObject,
            name: *mut ffi::PyObject,
            value: *mut ffi::PyObject,
            dictionary: *mut ffi::PyObject,
        ) -> c_int;
    }

    fn nominal(builtin: BuiltinType) -> StaticType {
        StaticType::NominalBuiltin {
            builtin,
            allow_subclasses: true,
        }
    }

    fn checked_class_policy() -> ResolvedStrictPolicy {
        ResolvedStrictPolicy {
            checked_attr: true,
            ..Default::default()
        }
    }

    fn layout(names: &[&str]) -> StorageLayout {
        StorageLayout {
            fields: names.iter().map(|name| (*name).into()).collect(),
            ..StorageLayout::default()
        }
    }

    fn storage_fixture<'py>(
        py: Python<'py>,
        layout: StorageLayout,
    ) -> PyResult<StoragePolicyOwner<'py>> {
        storage_fixture_for_model(py, StorageModel::Indexed, layout)
    }

    fn storage_fixture_for_model<'py>(
        py: Python<'py>,
        model: StorageModel,
        layout: StorageLayout,
    ) -> PyResult<StoragePolicyOwner<'py>> {
        let checks = layout
            .checks
            .iter()
            .flat_map(|(name, requirements)| {
                requirements
                    .iter()
                    .map(|value_type| (name.clone(), value_type.clone()))
            })
            .map(|(name, value_type)| {
                Ok(SelectedStorageCheck::own(
                    StrictFieldChecks::builtin_fixture(py, BTreeMap::from([(name, value_type)]))?,
                    &layout,
                ))
            })
            .collect::<PyResult<Vec<_>>>()?;
        new_storage_owner(py, interpreter_id(), model, layout, &checks)
    }

    fn fact() -> ClassTypeFact {
        ClassTypeFact {
            identity: SourceIdentity {
                module: ModuleContentId::new("class_plan_fixture", 1),
                lexical_qualname: "Child".into(),
                source_range: SourceRange::new(0, 1),
                definition_kind: DefinitionKind::Class,
            },
            bases: vec![],
            metaclass: MetaclassFact::BuiltinType,
            decorators: vec![],
            participation: ParticipationProposal::Candidate,
            dictionary: ClassDictionarySemantics::DictionaryBearing,
            instance_fields: vec![],
            methods: vec![],
            class_members: vec![],
            inheritance: InheritanceFact {
                linearized_bases: vec![],
                complete: true,
            },
            openness: ClassOpenness::OpenSubclassFamily,
            transform: None,
            uncertainty: BTreeSet::new(),
        }
    }

    fn field(fact: &ClassTypeFact, name: &str, kind: FieldKind) -> FieldTypeFact {
        FieldTypeFact {
            name: name.into(),
            declaring_class: ClassReference {
                definition: fact.identity.clone(),
                source_digest: Fingerprint::digest(b"fixture"),
            },
            value_type: nominal(BuiltinType::Int),
            annotation_origin: AnnotationOrigin::Explicit,
            annotation_definition: None,
            field_kind: kind,
            read_policy: FieldReadPolicy::PythonAttribute,
            write_policy: FieldWritePolicy::PythonAttribute,
            initialization: InitializationPolicy::MayBeAbsent,
            default: DefaultFact::Missing,
            descriptor: DescriptorFact::default(),
            uncertainty: BTreeSet::new(),
        }
    }

    fn method(fact: &ClassTypeFact, name: &str, final_method: bool) -> MethodTypeFact {
        MethodTypeFact {
            name: name.into(),
            declaring_class: ClassReference {
                definition: fact.identity.clone(),
                source_digest: Fingerprint::digest(b"fixture"),
            },
            binding: MethodBinding::Instance,
            signature: CallableSignature {
                parameters: vec![],
                return_type: StaticType::Unknown,
                return_annotation_origin: AnnotationOrigin::Absent,
                uncertainty: BTreeSet::new(),
            },
            declared_final: final_method,
            override_policy: OverridePolicy::Dynamic,
            implementation: None,
            generated: None,
            uncertainty: BTreeSet::new(),
        }
    }

    #[test]
    fn dataclass_proposals_without_an_actual_invocation_cannot_install_a_layout() {
        use soac_contracts::{
            ClassTransformFact, DataclassOptions, DecoratorFact, DecoratorKind, TransformKind,
        };

        let mut proposed = fact();
        proposed.transform = Some(ClassTransformFact {
            kind: TransformKind::StdlibDataclass,
            provenance: None,
            dataclass_options: Some(DataclassOptions::default()),
            generated_methods: BTreeSet::from(["__init__".into()]),
        });
        proposed.decorators.push(DecoratorFact {
            kind: DecoratorKind::StdlibDataclass,
            expression_range: proposed.identity.source_range,
            definition: None,
            source_digest: None,
            arguments: BTreeMap::new(),
            uncertainty: BTreeSet::new(),
        });
        let policy = checked_class_policy();
        assert_eq!(
            class_plan(&proposed, &policy, &[], None).err(),
            Some(DynamicClassReason::UnknownDecorator)
        );
        proposed.decorators.clear();
        assert_eq!(
            class_plan(&proposed, &policy, &[], None).err(),
            Some(DynamicClassReason::FrameworkManaged)
        );
    }

    #[test]
    fn builtin_descriptor_plans_preserve_property_owned_access_without_a_field_slot() {
        let mut fact = fact();
        for (name, binding) in [
            ("static", MethodBinding::Static),
            ("class_method", MethodBinding::Class),
            ("read", MethodBinding::PropertyGetter),
        ] {
            let mut method = method(&fact, name, false);
            method.binding = binding;
            fact.methods.push(method);
        }
        let policy = checked_class_policy();
        let (storage, names) = class_plan(&fact, &policy, &[], None).unwrap();
        assert!(storage.fields.is_empty());
        assert!(storage.checks.is_empty());
        assert_eq!(
            names.protected,
            BTreeSet::from(["static".into(), "class_method".into()])
        );
        assert_eq!(
            class_plan(
                &fact,
                &policy,
                &[(&layout(&["read"]), &NamePolicy::default())],
                None,
            )
            .err(),
            Some(DynamicClassReason::ConflictingLayout),
        );

        let mut field = field(&fact, "property_field", FieldKind::InstanceField);
        field.descriptor.kind = DescriptorKind::Property;
        field.descriptor.getter = Some(SourceIdentity {
            lexical_qualname: "Child.property_field".into(),
            definition_kind: DefinitionKind::Function,
            ..fact.identity.clone()
        });
        fact.instance_fields.push(field);
        let (storage, names) = class_plan(&fact, &policy, &[], None).unwrap();
        assert!(storage.fields.is_empty() && storage.checks.is_empty());
        assert!(!names.protected.contains("property_field"));
        fact.instance_fields[0].descriptor.setter =
            fact.instance_fields[0].descriptor.getter.clone();
        assert_eq!(
            class_plan(&fact, &policy, &[], None).err(),
            Some(DynamicClassReason::UnsupportedDescriptor)
        );
    }

    #[test]
    fn ordinary_storage_merges_names_without_manufacturing_indexed_positions() {
        let left = layout(&["left", "shared"]);
        let right = layout(&["right", "shared"]);
        let ordinary = StorageLayout::merge(StorageModel::Ordinary, [&left, &right]).unwrap();
        assert_eq!(ordinary.fields, ["left", "shared", "right"]);
        assert_eq!(
            StorageLayout::merge(StorageModel::Indexed, [&left, &right]),
            Err(DynamicClassReason::ConflictingLayout)
        );
    }

    #[test]
    fn ordinary_storage_owner_has_only_actual_check_metadata_edges() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let logical = layout(&["field"]);
            let check = SelectedStorageCheck::own(
                StrictFieldChecks::builtin_fixture(
                    py,
                    BTreeMap::from([("field".into(), nominal(BuiltinType::Int))]),
                )?,
                &logical,
            );
            let owner = new_storage_owner(
                py,
                interpreter_id(),
                StorageModel::Ordinary,
                logical,
                &[check],
            )?;
            assert_eq!(owner.data().template, None);
            assert_eq!(owner.data().dictionary_mode(), 2);
            assert_eq!(owner.data().check_reference(0), 0);
            StrictFieldChecks::from_owner(owner.reference(0)?)?;
            assert!(
                owner.reference(1).is_err(),
                "no template or receiver primary"
            );
            assert!(
                new_policy_dictionary(&owner).is_err(),
                "ordinary storage is never fabricated"
            );
            Ok(())
        })
    }

    #[test]
    fn unchecked_ordinary_fields_do_not_install_dictionary_authority() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let owner = new_storage_owner(
                py,
                interpreter_id(),
                StorageModel::Ordinary,
                layout(&["declared_but_unchecked"]),
                &[],
            )?;
            assert_eq!(owner.data().dictionary_mode(), 0);
            assert_eq!(owner.data().template, None);
            assert!(owner.reference(0).is_err());
            Ok(())
        })
    }

    #[test]
    fn ordinary_dictionary_sharing_requires_actual_check_owners_and_field_routing() -> PyResult<()>
    {
        let _lock = native_lock();
        Python::attach(|py| {
            let logical = layout(&["field", "other"]);
            let make = || {
                StrictFieldChecks::builtin_fixture(
                    py,
                    BTreeMap::from([
                        ("field".into(), nominal(BuiltinType::Int)),
                        ("other".into(), nominal(BuiltinType::Int)),
                    ]),
                )
            };
            let first = make()?;
            let second = make()?;
            let requested = vec![SelectedStorageCheck::own(
                StrictFieldChecks::from_owner(first.owner().clone())?,
                &logical,
            )];
            let existing = new_storage_owner(
                py,
                interpreter_id(),
                StorageModel::Ordinary,
                logical.clone(),
                &requested,
            )?;
            assert!(supports_dictionary_checks(&existing, &requested)?);
            assert!(
                !supports_dictionary_checks(
                    &existing,
                    &[SelectedStorageCheck::own(second, &logical)],
                )?,
                "same source primitive is not the same actual field owner"
            );
            let partial = vec![SelectedStorageCheck {
                check: first,
                dictionary_fields: BTreeSet::from(["field".into()]),
            }];
            let partial = new_storage_owner(
                py,
                interpreter_id(),
                StorageModel::Ordinary,
                logical,
                &partial,
            )?;
            assert!(!supports_dictionary_checks(&partial, &requested)?);
            Ok(())
        })
    }

    #[test]
    fn prepared_storage_projections_keep_distinct_dictionary_and_member_rules() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let mut logical = layout(&["dictionary", "member"]);
            logical.object_fields.push("member".into());
            let original = StrictFieldChecks::builtin_fixture(
                py,
                BTreeMap::from([
                    ("dictionary".into(), nominal(BuiltinType::Int)),
                    ("member".into(), nominal(BuiltinType::Str)),
                ]),
            )?;
            let checks = vec![SelectedStorageCheck::own(original, &logical)];
            let owner = new_storage_owner(
                py,
                interpreter_id(),
                StorageModel::Ordinary,
                logical,
                &checks,
            )?;
            let dictionary = dictionary_storage_projection(&owner)?.unwrap();
            let member = member_storage_projection(&owner, "member")?;
            assert_eq!(dictionary.data().layout.fields, ["dictionary"]);
            assert!(dictionary.data().layout.object_fields.is_empty());
            assert_eq!(member.data().dictionary_mode(), 0);
            assert!(member.data().layout.fields.is_empty());
            let dictionary_check = StrictFieldChecks::from_owner(dictionary.reference(0)?)?;
            let member_check = StrictFieldChecks::from_owner(member.reference(0)?)?;
            assert!(dictionary_check.contains_field("dictionary"));
            assert!(!dictionary_check.contains_field("member"));
            assert!(member_check.contains_field("member"));
            assert!(!member_check.contains_field("dictionary"));
            assert!(dictionary_check.same_actual_check(&member_check)?);
            assert!(supports_dictionary_checks(&dictionary, &checks)?);
            assert!(supports_dictionary_checks(
                &owner,
                &inherited_field_checks(std::slice::from_ref(&dictionary))?,
            )?);

            let dictionary_name = PyString::new(py, "dictionary");
            let member_name = PyString::new(py, "member");
            let text = PyString::new(py, "text");
            let integer = 7_i32.into_pyobject(py)?.into_any();
            assert_eq!(
                unsafe {
                    validate_type_state_inline(
                        dictionary.owner().as_ptr(),
                        dictionary_name.as_ptr(),
                        integer.as_ptr(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    validate_type_state_inline(
                        dictionary.owner().as_ptr(),
                        dictionary_name.as_ptr(),
                        text.as_ptr(),
                    )
                },
                -1
            );
            assert!(PyErr::fetch(py).is_instance_of::<PyTypeError>(py));
            // The mapping entry hiding a native member has no own member rule.
            assert_eq!(
                unsafe {
                    validate_type_state_inline(
                        dictionary.owner().as_ptr(),
                        member_name.as_ptr(),
                        integer.as_ptr(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    validate_type_state_member(
                        member.owner().as_ptr(),
                        member_name.as_ptr(),
                        text.as_ptr(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    validate_type_state_member(
                        member.owner().as_ptr(),
                        member_name.as_ptr(),
                        integer.as_ptr(),
                    )
                },
                -1
            );
            assert!(PyErr::fetch(py).is_instance_of::<PyTypeError>(py));
            assert_eq!(
                unsafe {
                    validate_type_state_member(
                        member.owner().as_ptr(),
                        member_name.as_ptr(),
                        ptr::null_mut(),
                    )
                },
                0
            );
            Ok(())
        })
    }

    #[test]
    fn inherited_prefix_positions_and_all_base_checks_survive_merge() {
        let mut short = layout(&["shared"]);
        short
            .checks
            .insert("shared".into(), vec![nominal(BuiltinType::Int)]);
        let mut long = layout(&["shared", "later"]);
        long.checks
            .insert("shared".into(), vec![nominal(BuiltinType::Object)]);
        let merged = StorageLayout::merge(StorageModel::Indexed, [&short, &long, &short]).unwrap();
        assert_eq!(merged.fields, ["shared", "later"]);
        assert_eq!(
            merged.checks["shared"],
            [nominal(BuiltinType::Int), nominal(BuiltinType::Object)]
        );
        assert_eq!(
            StorageLayout::merge(StorageModel::Indexed, [&long, &layout(&["other"])]),
            Err(DynamicClassReason::ConflictingLayout)
        );
        assert_eq!(
            StorageLayout::merge(
                StorageModel::Indexed,
                [&long, &layout(&["shared", "different"])]
            ),
            Err(DynamicClassReason::ConflictingLayout)
        );
    }

    #[test]
    fn storage_merge_keeps_distinct_actual_field_policies_with_equal_source_types() -> PyResult<()>
    {
        let _lock = native_lock();
        Python::attach(|py| {
            let mut logical = layout(&["checked"]);
            logical
                .checks
                .insert("checked".into(), vec![nominal(BuiltinType::Int)]);
            let first = storage_fixture(py, logical.clone())?;
            let repeated = StoragePolicyOwner::from_owner(first.owner().clone())?;
            let second = storage_fixture(py, logical.clone())?;
            let owners = vec![first, repeated, second];
            let checks = inherited_field_checks(&owners)?;
            assert_eq!(
                checks.len(),
                2,
                "equal source types must not merge actual contract owners"
            );
            assert_ne!(checks[0].owner().as_ptr(), checks[1].owner().as_ptr());
            assert_eq!(
                StorageLayout::merge(
                    StorageModel::Indexed,
                    owners.iter().map(|owner| &owner.data().layout)
                )
                .unwrap(),
                logical
            );
            let merged = new_storage_owner(
                py,
                interpreter_id(),
                StorageModel::Indexed,
                logical,
                &checks,
            )?;
            assert_eq!(merged.data().dictionary_fields_by_check.len(), 2);
            assert_eq!(merged.reference(1)?.as_ptr(), checks[0].owner().as_ptr());
            assert_eq!(merged.reference(2)?.as_ptr(), checks[1].owner().as_ptr());
            let dictionary = new_policy_dictionary(&merged)?.cast_into::<PyDict>()?;
            dictionary.set_item("checked", 1)?;
            assert!(
                dictionary
                    .set_item("checked", "wrong")
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            Ok(())
        })
    }

    #[test]
    fn field_catalog_reserves_defaults_but_not_pseudo_fields_or_methods() {
        let mut fact = fact();
        let mut own = field(&fact, "own", FieldKind::InstanceField);
        own.annotation_origin = AnnotationOrigin::Inferred;
        let mut shared = field(&fact, "shared", FieldKind::InstanceField);
        shared.annotation_origin = AnnotationOrigin::Inferred;
        fact.instance_fields = vec![
            own,
            shared,
            field(&fact, "class_only", FieldKind::ClassVariable),
            field(&fact, "constructor_only", FieldKind::InitOnly),
        ];
        for name in ["defaulted", "constructor_only", "class_only"] {
            fact.class_members.push(ClassMemberFact {
                name: name.into(),
                kind: ClassMemberKind::ShadowableDefault,
                value_type: StaticType::Any,
                definition: None,
                descriptor: DescriptorFact::default(),
                uncertainty: BTreeSet::new(),
            });
        }
        fact.methods.push(method(&fact, "method", false));
        let base = layout(&["shared", "base_later"]);
        let base_names = NamePolicy {
            protected: BTreeSet::from(["own".into(), "inherited_method".into()]),
            final_methods: BTreeSet::new(),
        };
        let (layout, names) = class_plan(
            &fact,
            &checked_class_policy(),
            &[(&base, &base_names)],
            None,
        )
        .unwrap();
        assert_eq!(layout.fields, ["shared", "base_later", "own", "defaulted"]);
        assert!(
            layout.checks.is_empty(),
            "inferred int must not become a required write check"
        );
        assert_eq!(
            names.protected,
            BTreeSet::from([
                "class_only".into(),
                "method".into(),
                "inherited_method".into()
            ])
        );
        // An annotation-only ClassVar has no namespace default. Its field
        // record must preserve the same name protection without fake storage.
        fact.class_members
            .retain(|member| member.name != "class_only");
        let (annotation_only_layout, annotation_only_names) = class_plan(
            &fact,
            &checked_class_policy(),
            &[(&base, &base_names)],
            None,
        )
        .unwrap();
        assert_eq!(annotation_only_layout, layout);
        assert_eq!(annotation_only_names.protected, names.protected);
    }

    #[test]
    fn physical_slot_plans_keep_dictionary_shape_and_indices_independent() {
        let mut fact = fact();
        fact.dictionary = ClassDictionarySemantics::ExplicitSlots;
        fact.instance_fields = vec![
            field(&fact, "member", FieldKind::InstanceField),
            field(&fact, "dictionary_value", FieldKind::InstanceField),
        ];
        let mut slotted_base = layout(&[]);
        slotted_base.object_fields.push("member".into());
        slotted_base.dictionary_bearing = false;
        slotted_base.declared_slots = true;
        let names = NamePolicy::default();
        // Inherited logical __slots__ does not suppress a child's real dict.
        let actual = ObjectSlotPlan {
            names: vec!["member".into()],
            declared: false,
            dictionary: true,
        };
        let (child, _) = class_plan_with_slots(
            StorageModel::Indexed,
            &fact,
            &checked_class_policy(),
            &[(&slotted_base, &names)],
            None,
            Some(&actual),
        )
        .unwrap();
        assert_eq!(child.fields, ["dictionary_value"]);
        assert_eq!(child.object_fields, ["member"]);
        assert!(child.dictionary_bearing && !child.declared_slots);

        // An annotation outside a genuine dictionary-less slot declaration
        // does not invent storage or presence for that name.
        let actual = ObjectSlotPlan {
            declared: true,
            dictionary: false,
            ..actual
        };
        let (slotted, _) = class_plan_with_slots(
            StorageModel::Indexed,
            &fact,
            &checked_class_policy(),
            &[],
            None,
            Some(&actual),
        )
        .unwrap();
        assert!(slotted.fields.is_empty());
        assert_eq!(slotted.object_fields, ["member"]);

        // A hybrid keeps an inherited prefix position even when the member
        // descriptor, rather than that hidden dictionary entry, serves attrs.
        let dictionary_base = layout(&["member"]);
        let actual = ObjectSlotPlan {
            dictionary: true,
            ..actual
        };
        let (hybrid, _) = class_plan_with_slots(
            StorageModel::Indexed,
            &fact,
            &checked_class_policy(),
            &[(&dictionary_base, &names)],
            None,
            Some(&actual),
        )
        .unwrap();
        assert_eq!(hybrid.fields, ["member", "dictionary_value"]);
        assert_eq!(hybrid.object_fields, ["member"]);
    }

    #[test]
    fn hidden_dictionary_entries_do_not_acquire_native_member_value_requirements() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let mut logical = layout(&["dictionary_value"]);
            logical.object_fields.push("member".into());
            for name in ["member", "dictionary_value"] {
                logical
                    .checks
                    .insert(name.into(), vec![nominal(BuiltinType::Int)]);
            }
            let storage = storage_fixture(py, logical)?;
            let dictionary = new_policy_dictionary(&storage)?.cast_into::<PyDict>()?;
            dictionary.set_item("member", "hidden")?;
            assert!(
                dictionary
                    .set_item("dictionary_value", "wrong")
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            let key = PyString::new(py, "member");
            let wrong = PyString::new(py, "wrong");
            assert!(
                check_unicode_field_value(
                    &storage,
                    &key,
                    wrong.as_any(),
                    FieldWriteStorage::NativeObjectMember,
                )
                .unwrap_err()
                .is_instance_of::<PyTypeError>(py)
            );
            assert_eq!(
                dictionary
                    .get_item("member")?
                    .unwrap()
                    .extract::<String>()?,
                "hidden"
            );
            Ok(())
        })
    }

    #[test]
    fn shadowing_native_member_preserves_only_inherited_dictionary_requirements() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let mut base_layout = layout(&["value"]);
            base_layout
                .checks
                .insert("value".into(), vec![nominal(BuiltinType::Int)]);
            let base = storage_fixture(py, base_layout.clone())?;
            let mut hybrid_layout = base_layout;
            hybrid_layout.object_fields.push("value".into());
            hybrid_layout
                .checks
                .get_mut("value")
                .unwrap()
                .push(nominal(BuiltinType::Bool));
            let own = StrictFieldChecks::builtin_fixture(
                py,
                BTreeMap::from([("value".into(), nominal(BuiltinType::Bool))]),
            )?;
            let mut checks = inherited_field_checks(&[base])?;
            checks.push(SelectedStorageCheck::own(own, &hybrid_layout));
            let hybrid = new_storage_owner(
                py,
                interpreter_id(),
                StorageModel::Indexed,
                hybrid_layout.clone(),
                &checks,
            )?;
            let dictionary = new_policy_dictionary(&hybrid)?.cast_into::<PyDict>()?;
            dictionary.set_item("value", 42)?;
            assert!(
                dictionary
                    .set_item("value", "bad")
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            let key = PyString::new(py, "value");
            let boolean = true.into_pyobject(py)?.to_owned().into_any();
            check_unicode_field_value(
                &hybrid,
                &key,
                &boolean,
                FieldWriteStorage::NativeObjectMember,
            )?;
            let integer = 42_i32.into_pyobject(py)?.into_any();
            assert!(
                check_unicode_field_value(
                    &hybrid,
                    &key,
                    &integer,
                    FieldWriteStorage::NativeObjectMember
                )
                .unwrap_err()
                .is_instance_of::<PyTypeError>(py)
            );

            // Ordinary subclasses and multiple inheritance reuse the check
            // owner but must not broaden its dictionary routing on a merge.
            let repeated = StoragePolicyOwner::from_owner(hybrid.owner().clone())?;
            let checks = inherited_field_checks(&[hybrid, repeated])?;
            assert_eq!(checks.len(), 2);
            let merged = new_storage_owner(
                py,
                interpreter_id(),
                StorageModel::Indexed,
                hybrid_layout,
                &checks,
            )?;
            let dictionary = new_policy_dictionary(&merged)?.cast_into::<PyDict>()?;
            dictionary.set_item("value", 43)?;
            assert!(
                dictionary
                    .set_item("value", "bad")
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            assert!(
                check_unicode_field_value(
                    &merged,
                    &key,
                    &integer,
                    FieldWriteStorage::NativeObjectMember
                )
                .unwrap_err()
                .is_instance_of::<PyTypeError>(py)
            );
            Ok(())
        })
    }

    #[test]
    fn field_override_cannot_discard_finality_or_reinterpret_an_inherited_slot() {
        let mut fact = fact();
        fact.instance_fields
            .push(field(&fact, "final_method", FieldKind::InstanceField));
        let final_names = NamePolicy {
            protected: BTreeSet::from(["final_method".into()]),
            final_methods: BTreeSet::from(["final_method".into()]),
        };
        assert_eq!(
            class_plan(
                &fact,
                &checked_class_policy(),
                &[(&layout(&[]), &final_names)],
                None,
            )
            .err(),
            Some(DynamicClassReason::ConflictingLayout)
        );
        fact.instance_fields.clear();
        fact.methods.push(method(&fact, "base_field", false));
        assert_eq!(
            class_plan(
                &fact,
                &checked_class_policy(),
                &[(&layout(&["base_field"]), &NamePolicy::default())],
                None,
            )
            .err(),
            Some(DynamicClassReason::ConflictingLayout)
        );
    }

    #[test]
    fn disabled_child_checks_do_not_remove_existing_base_write_contracts() {
        let mut base = layout(&["checked"]);
        base.checks
            .insert("checked".into(), vec![nominal(BuiltinType::Int)]);
        let mut fact = fact();
        fact.instance_fields
            .push(field(&fact, "checked", FieldKind::InstanceField));
        let (child, _) = class_plan(
            &fact,
            &ResolvedStrictPolicy {
                checked_attr: true,
                class_overrides: vec![ClassPolicyOverride {
                    class_range: fact.identity.source_range,
                    checked_attr: false,
                }],
                ..Default::default()
            },
            &[(&base, &NamePolicy::default())],
            None,
        )
        .unwrap();
        assert_eq!(child.checks, base.checks);
    }

    #[test]
    fn enabled_child_selects_own_explicit_fields_without_upgrading_base_declarations() {
        let mut base_fact = fact();
        base_fact.identity.module = ModuleContentId::new("base_plan_fixture", 1);
        base_fact.identity.lexical_qualname = "Base".into();
        let mut base = layout(&["base_explicit", "base_inferred", "base_checked"]);
        base.checks
            .insert("base_checked".into(), vec![nominal(BuiltinType::Int)]);
        let mut child_fact = fact();
        let mut inferred = field(&base_fact, "base_inferred", FieldKind::InstanceField);
        inferred.annotation_origin = AnnotationOrigin::Inferred;
        let mut own_inferred = field(&child_fact, "own_inferred", FieldKind::InstanceField);
        own_inferred.annotation_origin = AnnotationOrigin::Inferred;
        let mut partial_union = field(&child_fact, "partial_union", FieldKind::InstanceField);
        partial_union.value_type =
            StaticType::Union(vec![nominal(BuiltinType::Int), StaticType::Unknown]);
        child_fact.instance_fields = vec![
            field(&base_fact, "base_explicit", FieldKind::InstanceField),
            inferred,
            field(&base_fact, "base_checked", FieldKind::InstanceField),
            field(
                &child_fact,
                "own_explicit",
                FieldKind::ShadowableClassDefault,
            ),
            own_inferred,
            partial_union,
        ];
        let policy = ResolvedStrictPolicy {
            class_overrides: vec![ClassPolicyOverride {
                class_range: child_fact.identity.source_range,
                checked_attr: true,
            }],
            ..Default::default()
        };
        let names = NamePolicy::default();
        let (child, _) = class_plan(&child_fact, &policy, &[(&base, &names)], None).unwrap();
        assert_eq!(
            child.fields,
            [
                "base_explicit",
                "base_inferred",
                "base_checked",
                "own_explicit",
                "own_inferred",
                "partial_union"
            ]
        );
        assert_eq!(
            child.checks,
            BTreeMap::from([
                ("base_checked".into(), vec![nominal(BuiltinType::Int)]),
                ("own_explicit".into(), vec![nominal(BuiltinType::Int)]),
            ])
        );

        // An actual redeclaration belongs to the child policy; merely seeing
        // an inherited explicit annotation above did not create a requirement.
        child_fact.instance_fields[0].declaring_class.definition = child_fact.identity.clone();
        let (redeclared, _) = class_plan(&child_fact, &policy, &[(&base, &names)], None).unwrap();
        assert_eq!(redeclared.fields, child.fields);
        assert_eq!(
            redeclared.checks["base_explicit"],
            [nominal(BuiltinType::Int)]
        );
        assert_eq!(
            redeclared.checks["base_checked"],
            base.checks["base_checked"]
        );
    }

    #[test]
    fn only_explicit_fully_supported_field_annotations_select_required_checks() {
        let enabled = CheckedFieldPolicy::SupportedAnnotations;
        let integer = nominal(BuiltinType::Int);
        for origin in [
            AnnotationOrigin::Inferred,
            AnnotationOrigin::Absent,
            AnnotationOrigin::Unresolved,
        ] {
            assert_eq!(selected_field_contract(enabled, origin, &integer), None);
        }
        assert_eq!(
            selected_field_contract(
                CheckedFieldPolicy::Disabled,
                AnnotationOrigin::Explicit,
                &integer
            ),
            None
        );
        for supported in [
            integer.clone(),
            StaticType::None,
            StaticType::Optional(Box::new(integer.clone())),
            StaticType::Union(vec![integer.clone(), nominal(BuiltinType::Str)]),
        ] {
            assert_eq!(
                selected_field_contract(enabled, AnnotationOrigin::Explicit, &supported),
                Some(supported.clone())
            );
        }
        for dynamic in [
            StaticType::Any,
            StaticType::Union(vec![integer.clone(), StaticType::Unknown]),
        ] {
            assert_eq!(
                selected_field_contract(enabled, AnnotationOrigin::Explicit, &dynamic),
                None
            );
        }
        let nominal =
            StaticType::NominalClass(field(&fact(), "x", FieldKind::InstanceField).declaring_class);
        assert_eq!(
            selected_field_contract(enabled, AnnotationOrigin::Explicit, &nominal),
            Some(nominal.clone())
        );
        let union = StaticType::Union(vec![integer, nominal]);
        assert_eq!(
            selected_field_contract(enabled, AnnotationOrigin::Explicit, &union),
            Some(union)
        );
    }

    #[test]
    fn private_catalog_names_decline_until_canonical_mangling_is_authenticated() {
        let mut fact = fact();
        fact.instance_fields
            .push(field(&fact, "__private", FieldKind::InstanceField));
        assert_eq!(
            class_plan(&fact, &checked_class_policy(), &[], None).err(),
            Some(DynamicClassReason::UnresolvedAnalysis)
        );
        assert!(catalog_name_supported("_Parent__private"));
        assert!(catalog_name_supported("__dunder__"));
    }

    #[test]
    fn native_class_comprehension_capture_identity_does_not_require_native_iteration_storage()
    -> PyResult<()> {
        use crate::strict_interpreter_source::{InterpreterCodeRole, StrictInterpreterSource};

        fn only_code_child<'py>(parent: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
            let constants = parent.getattr("co_consts")?.cast_into::<PyTuple>()?;
            let mut children: Vec<_> = constants
                .iter()
                .filter(|value| unsafe { ffi::PyCode_Check(value.as_ptr()) != 0 })
                .collect();
            assert_eq!(children.len(), 1, "expected the original single code child");
            Ok(children.pop().unwrap())
        }

        const SOURCE: &str = concat!(
            "from __future__ import strict\n",
            "def build(sink, prefix, source, later):\n",
            "    class C:\n",
            "        result = sink(prefix(), [lambda: item for item in source()], later())\n",
            "    return C\n",
        );
        let _lock = native_lock();
        Python::attach(|py| {
            let facts = ModuleTypeFacts::new(
                "class_prefix_plan_fixture",
                SOURCE.as_bytes(),
                SourceDialect::SoacStrict,
                ResolvedStrictPolicy {
                    strict_assign: true,
                    checked_attr: true,
                    ..Default::default()
                },
            )
            .map_err(fixture_error)?;
            let fixture = FieldCapabilityFixture::from_facts(py, SOURCE.as_bytes(), facts)?;

            // The catalogue deliberately grants no class facts. Inspect the
            // actual compiler result without executing SOURCE or manufacturing
            // a retained class-binding plan. Native observations are exercised
            // by the unchanged Python prefix observer on ordinary/native code.
            let details =
                crate::module_type::compile_verified_native_details(py, &fixture.verified)?;
            let root = details.get_item(0)?;
            let bindings = details.get_item(2)?;
            let native = StrictInterpreterSource::from_native_details(
                py,
                Arc::clone(&fixture.verified),
                &root,
                &bindings,
            )?;
            let build_code = only_code_child(&root)?;
            let class_code = only_code_child(&build_code)?;
            let lambda_code = only_code_child(&class_code)?;
            let module = native.code(py, &root)?;
            let build = native.code(py, &build_code)?;
            let class = native.code(py, &class_code)?;
            let lambda = native.code(py, &lambda_code)?;
            assert_eq!(module.role(), InterpreterCodeRole::Module);
            assert_eq!(build.role(), InterpreterCodeRole::SourceFunction);
            assert_eq!(class.role(), InterpreterCodeRole::ClassNamespace);
            assert_eq!(lambda.role(), InterpreterCodeRole::Lambda);
            assert_eq!(module.parent_ordinal(), None);
            assert_eq!(build.parent_ordinal(), Some(module.ordinal()));
            assert_eq!(class.parent_ordinal(), Some(build.ordinal()));
            assert_eq!(lambda.parent_ordinal(), Some(class.ordinal()));

            // The actual lambda's FREE ordinal and creation site identify the
            // regional current CELL. Equal spelling only corroborates that
            // physical edge; it does not choose a source owner or a recipe.
            let free = lambda.layout().free_variables().collect::<Vec<_>>();
            assert_eq!(free.len(), 1);
            assert_eq!(free[0].0, 0);
            assert_eq!(free[0].2, "item");
            let scopes = bindings.get_item(2)?.cast_into::<PyTuple>()?;
            let scope = scopes
                .get_item(class.ordinal() as usize)?
                .cast_into::<PyTuple>()?;
            assert_eq!(scope.len(), 7);
            assert_eq!(scope.get_item(0)?.extract::<u32>()?, class.ordinal());
            let regions = scope.get_item(3)?.cast_into::<PyTuple>()?;
            assert_eq!(regions.len(), 1);
            let captures = scope.get_item(4)?.cast_into::<PyTuple>()?;
            assert_eq!(captures.len(), 1);
            let capture = captures.get_item(0)?.cast_into::<PyTuple>()?;
            assert_eq!(capture.len(), 5);
            assert_eq!(capture.get_item(0)?.extract::<u32>()?, lambda.ordinal());
            assert_eq!(capture.get_item(2)?.extract::<u32>()?, free[0].0);
            let (current_tag, slot) = capture.get_item(3)?.extract::<(u32, u32)>()?;
            assert_eq!(current_tag, 0);
            let current = &class.layout().locals[slot as usize];
            assert_eq!(current.index, slot);
            assert_eq!(current.name, free[0].2);
            assert_eq!(current.kind & 0xc0, 0x40, "actual native CELL, not FREE");
            let region_id = capture.get_item(4)?.extract::<u32>()?;
            let region = regions
                .get_item(region_id as usize)?
                .cast_into::<PyTuple>()?;
            assert_eq!(region.len(), 8);
            assert_eq!(region.get_item(0)?.extract::<u32>()?, region_id);
            let lambda_text = "lambda: item";
            let start = SOURCE.find(lambda_text).unwrap() as u32;
            assert_eq!(
                lambda.source().source_range,
                SourceRange::new(start, start + lambda_text.len() as u32)
            );
            let column = SOURCE.lines().nth(3).unwrap().find(lambda_text).unwrap() as u32;
            assert_eq!(
                capture.get_item(1)?.extract::<(u32, u32, u32, u32)>()?,
                (4, column, 4, column + lambda_text.len() as u32)
            );

            // Native construction/source authority retains the original code
            // and capture identity. SOAC's ordinary helper owns the iteration
            // cell, so no class-frame carrier correspondence is required.
            let semantic = crate::class_bindings::decode(py, SOURCE, &root, bindings)?;
            let class_recipe = semantic
                .class_recipe(soac_core::block_py::NativeCodeId(class.ordinal()))
                .unwrap();
            assert!(class_recipe.captures.is_empty());
            assert!(
                class_recipe
                    .initializers
                    .iter()
                    .all(|init| init.slot.index != slot)
            );
            assert!(!fixture.module.dict().contains("build")?);
            Ok(())
        })
    }

    fn native_lock() -> std::sync::MutexGuard<'static, ()> {
        let lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        lock
    }

    fn interpreter_id() -> i64 {
        unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) }
    }

    /// Native-kernel fixture, not an alternate production authentication path.
    /// The real signature/shard verifier supplies the facts, and the existing
    /// test-only source wrapper binds their actual file bytes. Only namespace
    /// execution is identity-only in prepare(); run_body() instead lowers and
    /// executes real source, including its normal compiler handle, templates,
    /// function owners and pre-Ready class callbacks.
    struct FieldCapabilityFixture<'py> {
        directory: std::path::PathBuf,
        module: Bound<'py, PyModule>,
        module_state: crate::StrictModuleRuntimeState,
        verified: Arc<VerifiedStrictModule>,
    }

    impl<'py> FieldCapabilityFixture<'py> {
        fn new(py: Python<'py>) -> PyResult<Self> {
            Self::with_object_slots(py, false)
        }

        fn with_object_slots(py: Python<'py>, slots: bool) -> PyResult<Self> {
            let source: &[u8] = if slots {
                b"from __future__ import strict\nclass Child:\n    __slots__ = ('value', 'other', '__weakref__')\n    value: int\n    other: int\n"
            } else {
                b"from __future__ import strict\nclass Child:\n    value: int\n    other: int\n"
            };
            let mut facts = ModuleTypeFacts::new(
                "field_capability_fixture",
                source,
                SourceDialect::SoacStrict,
                ResolvedStrictPolicy {
                    strict_assign: true,
                    checked_attr: true,
                    ..Default::default()
                },
            )
            .map_err(fixture_error)?;
            let mut class = fact();
            class.identity.module = facts.module.clone();
            class.identity.source_range = SourceRange::new(
                source
                    .windows(b"class Child".len())
                    .position(|part| part == b"class Child")
                    .unwrap() as u32,
                facts.source_size,
            );
            if slots {
                class.dictionary = ClassDictionarySemantics::ExplicitSlots;
            }
            for name in ["value", "other"] {
                let mut field = field(&class, name, FieldKind::InstanceField);
                field.declaring_class.source_digest = facts.source_digest;
                class.instance_fields.push(field);
            }
            facts.classes.push(class);
            Self::from_facts(py, source, facts)
        }

        fn from_facts(py: Python<'py>, source: &[u8], facts: ModuleTypeFacts) -> PyResult<Self> {
            static NEXT_DIRECTORY: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "soac-class-capability-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&directory).map_err(fixture_error)?;
            let source_path = directory.join(format!("{}.py", facts.module.module_name));
            std::fs::write(&source_path, source).map_err(fixture_error)?;
            let source_path = source_path.canonicalize().map_err(fixture_error)?;
            let fingerprint = Fingerprint::digest(b"sealed-field native-kernel test environment");
            let environment = ArtifactEnvironment {
                ty_revision: "native-kernel-fixture".into(),
                checker_source_fingerprint: fingerprint,
                exporter_revision: "native-kernel-fixture".into(),
                python_version: PythonVersion {
                    major: 3,
                    minor: 15,
                },
                python_platform: "linux".into(),
                cpython_abi_fingerprint: fingerprint,
                normalized_project_policy: fingerprint,
                resolved_typechecker_configuration: fingerprint,
                import_search_path: fingerprint,
                typeshed_fingerprint: fingerprint,
                installed_stub_fingerprint: fingerprint,
                installed_dependency_fingerprint: fingerprint,
                analysis: ConservativeAnalysis::default(),
            };
            let shard = encode_module_shard(&facts).map_err(fixture_error)?;
            let manifest = TypeArtifactManifest::new(
                environment.clone(),
                vec![ModuleArtifactIndex::from_shard(&shard).map_err(fixture_error)?],
            )
            .map_err(fixture_error)?;
            let key = ArtifactSigningKey::from_bytes(&[83; 32]);
            let verified_manifest = verify_manifest(
                &sign_manifest(&manifest, &key).map_err(fixture_error)?,
                &key.trust_anchor(),
                &ArtifactExpectations {
                    generation: manifest.generation,
                    environment,
                },
            )
            .map_err(fixture_error)?;
            let verified_facts = verified_manifest
                .verify_module(
                    &facts.module.module_name,
                    source,
                    &facts.language_policy,
                    &[],
                    shard.bytes(),
                )
                .map_err(fixture_error)?;
            let verified = Arc::new(VerifiedStrictModule::from_verified_test_facts(
                py,
                source_path,
                Arc::from(source),
                verified_facts,
            )?);
            let module = PyModule::new(py, &facts.module.module_name)?;
            let module_state =
                crate::StrictModuleRuntimeState::install(py, module.as_any(), &verified)?;
            module_state.begin_execution(py)?;
            Ok(Self {
                directory,
                module,
                module_state,
                verified,
            })
        }

        fn new_method_source(py: Python<'py>) -> PyResult<Self> {
            Self::method_source(py, false)
        }

        /// Install callback-sensitive defaults while the actual method is
        /// still a class-suite operand, before the required admission seal.
        /// The two default-owner tests supply the ordinary callable explicitly;
        /// catalogue-only tests retain the original no-hook source.
        fn method_source(py: Python<'py>, prepare_defaults: bool) -> PyResult<Self> {
            const SOURCE: &str = "from __future__ import strict\n\ndef make():\n    class Child:\n        value = 10\n        def method(self, *, amount=1):\n            return self.value + amount\n    return Child\n\nfirst = make()\nsecond = make()\n";
            let source = if prepare_defaults {
                SOURCE.replace(
                    "    return Child",
                    "        _prepare_method_defaults(method)\n    return Child",
                )
            } else {
                SOURCE.to_owned()
            };
            let mut facts = ModuleTypeFacts::new(
                "method_capability_fixture",
                source.as_bytes(),
                SourceDialect::SoacStrict,
                ResolvedStrictPolicy {
                    strict_assign: true,
                    checked_attr: true,
                    ..Default::default()
                },
            )
            .map_err(fixture_error)?;
            let identity = |name: &str, start: &str, last: &str, definition_kind| SourceIdentity {
                module: facts.module.clone(),
                lexical_qualname: name.into(),
                source_range: SourceRange::new(
                    source.find(start).unwrap() as u32,
                    (source.find(last).unwrap() + last.len()) as u32,
                ),
                definition_kind,
            };
            let make = identity("make", "def make", "return Child", DefinitionKind::Function);
            let mut class = fact();
            class.identity = identity(
                "make.<locals>.Child",
                "class Child",
                if prepare_defaults {
                    "_prepare_method_defaults(method)"
                } else {
                    "return self.value + amount"
                },
                DefinitionKind::Class,
            );
            let implementation = identity(
                "make.<locals>.Child.method",
                "def method",
                "return self.value + amount",
                DefinitionKind::Function,
            );
            let signature = CallableSignature {
                parameters: vec![
                    ParameterTypeFact {
                        name: "self".into(),
                        kind: ParameterKind::PositionalOrKeyword,
                        value_type: StaticType::Any,
                        annotation_origin: AnnotationOrigin::Absent,
                        default: DefaultFact::Missing,
                    },
                    ParameterTypeFact {
                        name: "amount".into(),
                        kind: ParameterKind::KeywordOnly,
                        value_type: nominal(BuiltinType::Int),
                        annotation_origin: AnnotationOrigin::Absent,
                        default: DefaultFact::Value {
                            value_type: Box::new(nominal(BuiltinType::Int)),
                            literal: None,
                        },
                    },
                ],
                return_type: StaticType::Unknown,
                return_annotation_origin: AnnotationOrigin::Absent,
                uncertainty: BTreeSet::new(),
            };
            let mut method = method(&class, "method", false);
            method.declaring_class.source_digest = facts.source_digest;
            method.signature = signature.clone();
            method.implementation = Some(implementation.clone());
            class.methods.push(method);
            class.class_members.push(ClassMemberFact {
                name: "value".into(),
                kind: ClassMemberKind::ShadowableDefault,
                value_type: nominal(BuiltinType::Int),
                definition: None,
                descriptor: DescriptorFact::default(),
                uncertainty: BTreeSet::new(),
            });
            facts.classes.push(class);
            facts.functions = vec![
                FunctionTypeFact {
                    identity: make,
                    function_kind: soac_contracts::FunctionKind::Synchronous,
                    signature: CallableSignature {
                        parameters: vec![],
                        return_type: StaticType::Unknown,
                        return_annotation_origin: AnnotationOrigin::Absent,
                        uncertainty: BTreeSet::new(),
                    },
                    decorators: vec![],
                    uncertainty: BTreeSet::new(),
                },
                FunctionTypeFact {
                    identity: implementation,
                    function_kind: soac_contracts::FunctionKind::Synchronous,
                    signature,
                    decorators: vec![],
                    uncertainty: BTreeSet::new(),
                },
            ];
            Self::from_facts(py, source.as_bytes(), facts)
        }

        fn new_virtual_method_source(py: Python<'py>) -> PyResult<Self> {
            Self::virtual_method_source(py, false)
        }

        /// Optional ordinary setup runs on the actual Override.method operand
        /// before the class's mandatory admission seal, not after run_body().
        fn virtual_method_source(py: Python<'py>, prepare_defaults: bool) -> PyResult<Self> {
            const SOURCE: &str = "from __future__ import strict\n\ndef make():\n    class Base:\n        value = 10\n        def method(self, *, amount=1):\n            return self.value + amount\n    class Override(Base):\n        def method(self, *, amount=2):\n            return self.value * amount\n    class Inherited(Base):\n        pass\n    class Grandchild(Override):\n        pass\n    class Shadow(Base):\n        method = None\n    return Base, Override, Inherited, Grandchild, Shadow\n\nfirst = make()\nsecond = make()\n";
            let source = if prepare_defaults {
                SOURCE.replace(
                    "    class Inherited(Base):",
                    "        _prepare_method_defaults(method)\n    class Inherited(Base):",
                )
            } else {
                SOURCE.to_owned()
            };
            let mut facts = ModuleTypeFacts::new(
                "virtual_method_capability_fixture",
                source.as_bytes(),
                SourceDialect::SoacStrict,
                ResolvedStrictPolicy {
                    strict_assign: true,
                    checked_attr: true,
                    ..Default::default()
                },
            )
            .map_err(fixture_error)?;
            let module = facts.module.clone();
            let digest = facts.source_digest;
            let identity = |name: &str, start: &str, last: &str, definition_kind| SourceIdentity {
                module: module.clone(),
                lexical_qualname: name.into(),
                source_range: SourceRange::new(
                    source.find(start).unwrap() as u32,
                    (source.find(last).unwrap() + last.len()) as u32,
                ),
                definition_kind,
            };
            let reference = |class: &ClassTypeFact| ClassReference {
                definition: class.identity.clone(),
                source_digest: digest,
            };
            let signature = CallableSignature {
                parameters: vec![
                    ParameterTypeFact {
                        name: "self".into(),
                        kind: ParameterKind::PositionalOrKeyword,
                        value_type: StaticType::Any,
                        annotation_origin: AnnotationOrigin::Absent,
                        default: DefaultFact::Missing,
                    },
                    ParameterTypeFact {
                        name: "amount".into(),
                        kind: ParameterKind::KeywordOnly,
                        value_type: nominal(BuiltinType::Int),
                        annotation_origin: AnnotationOrigin::Absent,
                        default: DefaultFact::Value {
                            value_type: Box::new(nominal(BuiltinType::Int)),
                            literal: None,
                        },
                    },
                ],
                return_type: StaticType::Unknown,
                return_annotation_origin: AnnotationOrigin::Absent,
                uncertainty: BTreeSet::new(),
            };
            let mut base = fact();
            base.identity = identity(
                "make.<locals>.Base",
                "class Base:",
                "return self.value + amount",
                DefinitionKind::Class,
            );
            let base_method = identity(
                "make.<locals>.Base.method",
                "def method(self, *, amount=1)",
                "return self.value + amount",
                DefinitionKind::Function,
            );
            let mut base_binding = method(&base, "method", false);
            base_binding.declaring_class = reference(&base);
            base_binding.signature = signature.clone();
            base_binding.implementation = Some(base_method.clone());
            base.methods.push(base_binding.clone());
            base.class_members.push(ClassMemberFact {
                name: "value".into(),
                kind: ClassMemberKind::ShadowableDefault,
                value_type: nominal(BuiltinType::Int),
                definition: None,
                descriptor: DescriptorFact::default(),
                uncertainty: BTreeSet::new(),
            });
            let mut overriding = fact();
            overriding.identity = identity(
                "make.<locals>.Override",
                "class Override(Base):",
                if prepare_defaults {
                    "_prepare_method_defaults(method)"
                } else {
                    "return self.value * amount"
                },
                DefinitionKind::Class,
            );
            overriding.bases = vec![BaseReference::Class(reference(&base))];
            overriding.inheritance.linearized_bases = overriding.bases.clone();
            let override_method = identity(
                "make.<locals>.Override.method",
                "def method(self, *, amount=2)",
                "return self.value * amount",
                DefinitionKind::Function,
            );
            let mut override_binding = method(&overriding, "method", false);
            override_binding.declaring_class = reference(&overriding);
            override_binding.signature = signature.clone();
            override_binding.implementation = Some(override_method.clone());
            overriding.methods.push(override_binding.clone());
            let mut inherited = fact();
            inherited.identity = identity(
                "make.<locals>.Inherited",
                "class Inherited(Base):",
                "class Inherited(Base):\n        pass",
                DefinitionKind::Class,
            );
            inherited.bases = vec![BaseReference::Class(reference(&base))];
            inherited.inheritance.linearized_bases = inherited.bases.clone();
            inherited.methods.push(base_binding);
            let mut grandchild = fact();
            grandchild.identity = identity(
                "make.<locals>.Grandchild",
                "class Grandchild(Override):",
                "class Grandchild(Override):\n        pass",
                DefinitionKind::Class,
            );
            grandchild.bases = vec![BaseReference::Class(reference(&overriding))];
            grandchild.inheritance.linearized_bases = vec![
                BaseReference::Class(reference(&overriding)),
                BaseReference::Class(reference(&base)),
            ];
            grandchild.methods.push(override_binding);
            let mut shadow = fact();
            shadow.identity = identity(
                "make.<locals>.Shadow",
                "class Shadow(Base):",
                "method = None",
                DefinitionKind::Class,
            );
            shadow.bases = vec![BaseReference::Class(reference(&base))];
            shadow.inheritance.linearized_bases = shadow.bases.clone();
            shadow.class_members.push(ClassMemberFact {
                name: "method".into(),
                kind: ClassMemberKind::ShadowableDefault,
                value_type: StaticType::None,
                definition: None,
                descriptor: DescriptorFact::default(),
                uncertainty: BTreeSet::new(),
            });
            facts.classes = vec![base, overriding, inherited, grandchild, shadow];
            facts.functions = vec![FunctionTypeFact {
                identity: identity(
                    "make",
                    "def make():",
                    "return Base, Override, Inherited, Grandchild, Shadow",
                    DefinitionKind::Function,
                ),
                function_kind: soac_contracts::FunctionKind::Synchronous,
                signature: CallableSignature {
                    parameters: vec![],
                    return_type: StaticType::Unknown,
                    return_annotation_origin: AnnotationOrigin::Absent,
                    uncertainty: BTreeSet::new(),
                },
                decorators: vec![],
                uncertainty: BTreeSet::new(),
            }];
            for identity in [base_method, override_method] {
                facts.functions.push(FunctionTypeFact {
                    identity,
                    function_kind: soac_contracts::FunctionKind::Synchronous,
                    signature: signature.clone(),
                    decorators: vec![],
                    uncertainty: BTreeSet::new(),
                });
            }
            Self::from_facts(py, source.as_bytes(), facts)
        }

        /// The source path below is deliberately not a fake function owner.
        /// It uses real lowering, authenticated native code matching, shared
        /// templates, MakeFunction, the namespace activation binder, and class
        /// construction. Only deployment/signing setup is the existing fixture.
        fn run_body(&self) -> PyResult<Arc<crate::module_type::SharedModuleState>> {
            let py = self.module.py();
            let source = std::str::from_utf8(self.verified.source()).map_err(fixture_error)?;
            // MakeFunction resolves its packed runtime IDs through the same
            // process session used by the production module loader. A private
            // fixture session would register an unreachable parallel catalog.
            let session = crate::CompileSession::process();
            let compiled = crate::CompiledStrictSource::compile(py, &self.verified)?;
            let lowered = soac_lowering::lower_python_to_blockpy_with_tracker_and_options(
                source,
                session.module_name_gen(),
                soac_core::pass_tracker::RecordingPassTracker::new(),
                soac_lowering::LoweringOptions {
                    strict_facts: Some(Arc::new(self.verified.type_facts().clone())),
                    canonical_annotations: Some(compiled.canonical_annotations()),
                    canonical_class_bindings: Some(compiled.canonical_class_bindings()),
                    ..Default::default()
                },
            )
            .map_err(fixture_error)?
            .blockpy_module;
            let authenticated_code =
                compiled.into_function_catalog(py, &self.verified, &lowered)?;
            let mut shared = crate::module_type::build_shared_state_for_inspection_with_original_code_and_source_hash(
                py, lowered, &self.verified.type_facts().facts().module.module_name, "",
                self.verified.type_facts().facts().module.source_hash, Some(source), std::collections::HashMap::new(),
            )?;
            // These are the actual verified/module-policy owners; attach them
            // before this fresh shared state is visible to any constructor.
            let unpublished = Arc::get_mut(&mut shared).expect("unpublished fixture shared state");
            unpublished.original_code_by_function_id =
                crate::strict_admission::OriginalCodeStorage::Authenticated(authenticated_code);
            unpublished.strict_module = Some(Arc::clone(&self.verified));
            unpublished.strict_execution = Some(self.module_state.execution_ref());
            session
                .retain_shared_module_state(Arc::clone(&shared))
                .map_err(fixture_error)?;
            let globals = self.module.dict();
            globals.set_item("__builtins__", py.import("builtins")?.dict())?;
            let runtime = crate::ModuleRuntimeContext {
                mod_ctx: crate::ModuleJitContext {
                    shared_module_state: Arc::as_ptr(&shared),
                    globals_obj: globals.clone().into_any().into_ptr().cast(),
                },
                compile_session: session,
                shared_module_state_owner: Arc::clone(&shared),
            };
            let function = shared
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| {
                    function
                        .scope
                        .source_origin
                        .as_ref()
                        .is_some_and(|origin| origin.role == CallableSourceRole::ModuleBody)
                })
                .expect("real lowered module body");
            let dp = py.import("soac.runtime")?;
            let empty = PyTuple::empty(py);
            let module_init = crate::instantiate_bb_function(
                py,
                &dp,
                &shared.module_name,
                function,
                empty.as_any(),
                empty.as_any(),
                globals.as_any(),
                py.None().bind(py),
                &runtime,
            )?;
            module_init.call0(py)?;
            Ok(shared)
        }

        fn finalize(&self, shared: &crate::module_type::SharedModuleState) -> PyResult<()> {
            let py = self.module.py();
            self.module_state.begin_sealing(py)?;
            crate::finalize_strict_module_contents(py, &self.module_state, shared)?;
            self.module_state.finish_sealing(py)
        }

        fn prepare_with_slots(
            &self,
            slots: Option<&ObjectSlotPlan>,
        ) -> PyResult<StrictClassState<'py>> {
            let py = self.module.py();
            let fact = &self.verified.type_facts().facts().classes[0];
            let (layout, names) = class_plan_with_slots(
                StorageModel::Ordinary,
                fact,
                &self.verified.type_facts().facts().language_policy,
                &[],
                None,
                slots,
            )
            .map_err(|reason| fixture_error(format!("fixture class declined: {reason:?}")))?;
            let object_offsets = layout.object_fields.iter().map(|_| Cell::new(-1)).collect();
            let storage = storage_fixture_for_model(py, StorageModel::Ordinary, layout)?;
            let execution = self.module_state.execution_ref();
            let module_owner = execution.acquire_owner(py, &self.module.dict(), &self.verified)?;
            let state = StrictStateRef::new(
                py,
                StrictClassData {
                    verified: Arc::clone(&self.verified),
                    execution,
                    fact: fact.clone(),
                    names,
                    phase: Cell::new(ClassPhase::Prepared),
                    actual_type: Cell::new(0),
                    construction: Arc::new(ActualClassConstruction),
                    construction_kind: ClassConstructionKind::SourceNamespace,
                    object_offsets,
                    namespace_execution:
                        crate::strict_namespace::NamespaceExecution::completed_identity_for_test(
                            fact.identity.clone(),
                            interpreter_id(),
                        ),
                    interpreter_invocation: OnceLock::new(),
                    own_field_bindings: Vec::new(),
                    own_field_checks: None,
                    dataclass: None,
                    method_families: OnceLock::new(),
                },
                vec![module_owner, storage.owner().clone().unbind(), py.None()],
            )?;
            Ok(StrictClassState {
                state,
                actual_type: None,
            })
        }

        fn namespace(&self) -> PyResult<Bound<'py, PyDict>> {
            let namespace = PyDict::new(self.module.py());
            namespace.set_item("__module__", "field_capability_fixture")?;
            namespace.set_item("__qualname__", "Child")?;
            if self.verified.type_facts().facts().classes[0].dictionary
                == ClassDictionarySemantics::ExplicitSlots
            {
                namespace.set_item("__slots__", ("value", "other", "__weakref__"))?;
            }
            Ok(namespace)
        }

        fn construct(
            &self,
            namespace: &Bound<'py, PyDict>,
        ) -> PyResult<(StrictClassState<'py>, Bound<'py, PyAny>)> {
            let slots = ObjectSlotPlan::prepare(
                &self.verified.type_facts().facts().classes[0],
                namespace,
                &PyTuple::empty(self.module.py()),
                std::iter::empty(),
                false,
            )?
            .map_err(|reason| fixture_error(format!("fixture slots declined: {reason:?}")))?;
            let state = self.prepare_with_slots(Some(&slots))?;
            let class = crate::strict_class::construct_type_for_class_state_test(
                &state,
                &PyTuple::empty(self.module.py()),
                namespace,
            )?;
            let state =
                for_actual_type(self.module.py(), &class)?.expect("native actual type view");
            Ok((state, class))
        }
    }

    impl Drop for FieldCapabilityFixture<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn fixture_error(error: impl std::fmt::Display) -> PyErr {
        pyo3::exceptions::PyValueError::new_err(error.to_string())
    }

    /// This is the same two-stage ABI contract the typed consumer must use:
    /// -1 propagates an error, 0 is original getattr, and a 1 match immediately
    /// dominates the borrowed raw probe without any Python effect.
    fn probe_sealed_field(
        capability: &SealedFieldCapability,
        receiver: &Bound<'_, PyAny>,
        name: &Bound<'_, PyString>,
    ) -> PyResult<*mut ffi::PyObject> {
        let matched =
            unsafe { dp_jit_match_sealed_field_capability(receiver.as_ptr(), capability) };
        match matched {
            -1 => Err(PyErr::fetch(receiver.py())),
            0 => Ok(ptr::null_mut()),
            1 => Ok(unsafe {
                // Exercise the mechanical consumer ABI, not just Rust field
                // access: descriptor loads are dominated by the match above.
                let prefix = ptr::from_ref(capability).cast::<u8>();
                let expected_type = *prefix
                    .add(SealedFieldCapability::EXPECTED_TYPE_OFFSET)
                    .cast::<usize>();
                let kind = *prefix
                    .add(SealedFieldCapability::STORAGE_KIND_OFFSET)
                    .cast::<SealedFieldStorageKind>();
                if kind == SealedFieldStorageKind::NativeObjectMember {
                    let offset = *prefix
                        .add(SealedFieldCapability::OBJECT_OFFSET_OFFSET)
                        .cast::<usize>();
                    return Ok(soac_jit_runtime::soac_runtime_load_native_object_slot(
                        receiver.as_ptr().cast(),
                        expected_type as *mut c_void,
                        offset as isize,
                    )
                    .cast());
                }
                let index = *prefix
                    .add(SealedFieldCapability::FIELD_INDEX_OFFSET)
                    .cast::<usize>();
                let mro = *prefix
                    .add(SealedFieldCapability::DEFAULT_MRO_INDEX_OFFSET)
                    .cast::<isize>();
                let namespace = *prefix
                    .add(SealedFieldCapability::DEFAULT_NAMESPACE_INDEX_OFFSET)
                    .cast::<isize>();
                soac_jit_runtime::soac_runtime_probe_stable_indexed_field(
                    receiver.as_ptr().cast(),
                    expected_type as *mut c_void,
                    name.as_ptr().cast(),
                    index as isize,
                    mro,
                    namespace,
                )
                .cast()
            }),
            _ => unreachable!("sealed field match is a tri-state ABI"),
        }
    }

    #[test]
    fn sealed_field_capability_requires_native_seal_and_actual_execution() -> PyResult<()> {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SealedFieldCapability>();
        assert_send_sync::<SealedMethodCapability>();
        assert_send_sync::<SealedVirtualMethodCapability>();
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::with_object_slots(py, true)?;
            let namespace = fixture.namespace()?;
            let slots = ObjectSlotPlan::prepare(
                &fixture.verified.type_facts().facts().classes[0],
                &namespace,
                &PyTuple::empty(py),
                std::iter::empty(),
                false,
            )?
            .map_err(|reason| fixture_error(format!("{reason:?}")))?;
            let state = fixture.prepare_with_slots(Some(&slots))?;
            assert!(state.sealed_field("value")?.is_none());
            let class = crate::strict_class::construct_type_for_class_state_test(
                &state,
                &PyTuple::empty(py),
                &fixture.namespace()?,
            )?;
            assert_eq!(state.state.data().phase.get(), ClassPhase::Bound);
            assert_eq!(unsafe { PyType_IsSoacSealed(class.as_ptr()) }, 1);
            assert!(state.sealed_field("value")?.is_none());
            assert!(
                state.actual_type().is_err(),
                "owner alone is not an actual-type guard"
            );
            let state = for_actual_type(py, &class)?.expect("pinned actual type view");
            state.seal()?;
            let capability = state.sealed_field("value")?.expect("sealed actual field");
            assert_eq!(capability.source(), state.source());
            assert_eq!(capability.field_name(), "value");
            assert_eq!(
                capability.precedence(),
                SealedFieldPrecedence::NoClassBinding
            );
            assert!(state.sealed_field("missing")?.is_none());
            assert!(state.sealed_field("__annotations__")?.is_none());
            let receiver = class.call0()?;
            assert!(capability.matches_receiver(&receiver)?);

            let (other_state, other_class) = fixture.construct(&fixture.namespace()?)?;
            other_state.seal()?;
            assert_eq!(other_state.source(), state.source());
            assert!(!capability.matches_receiver(&other_class.call0()?)?);
            let mut reused_address = other_state.sealed_field("value")?.unwrap();
            // Model address reuse without dereferencing a dead object: the
            // current addresses and owner really match, but the old execution
            // witness must still fail. Source identity alone is identical.
            reused_address.witness.execution = Arc::clone(&capability.witness.execution);
            assert!(!reused_address.matches_receiver(&other_class.call0()?)?);

            // Source execution and all current addresses can match while a
            // copied witness still belongs to a different actual construction.
            // Replacement types must not borrow the original layout proof.
            let mut wrong_construction = other_state.sealed_field("value")?.unwrap();
            wrong_construction.witness.construction = Arc::clone(&capability.witness.construction);
            assert!(!wrong_construction.matches_receiver(&other_class.call0()?)?);

            let globals = PyDict::new(py);
            globals.set_item("Base", &class)?;
            py.run(c"class OrdinaryChild(Base): pass\n", Some(&globals), None)?;
            let ordinary_child = globals.get_item("OrdinaryChild")?.unwrap();
            let child = ordinary_child.call0()?;
            child.setattr("value", 42)?;
            assert!(!capability.matches_receiver(&child)?);
            assert!(for_actual_type(py, &ordinary_child)?.is_none());

            assert_eq!(
                unsafe { dp_jit_match_sealed_field_capability(ptr::null_mut(), &capability) },
                -1
            );
            assert!(
                PyErr::fetch(py)
                    .to_string()
                    .contains("null sealed field guard operand")
            );
            Ok(())
        })
    }

    #[test]
    fn type_construction_abi_matches_the_selected_native_headers() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let native: BTreeMap<String, usize> = py
                .import("_testinternalcapi")?
                .call_method0("soac_type_construction_layout")?
                .extract()?;
            for (name, actual) in crate::strict_class::TYPE_CONSTRUCTION_ABI_LAYOUT {
                assert_eq!(
                    native.get(name),
                    Some(&actual),
                    "selected type construction ABI: {name}"
                );
            }
            Ok(())
        })
    }

    #[test]
    fn source_slot_capability_selects_native_storage_and_preserves_unbound_and_subclass_fallback()
    -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::with_object_slots(py, true)?;
            let (state, class) = fixture.construct(&fixture.namespace()?)?;
            assert!(state.fields()?.is_empty());
            assert!(!state.dictionary_bearing()?);
            assert_eq!(state.object_fields()?.len(), 2);
            assert!(state.sealed_field("value")?.is_none());
            state.seal()?;
            let capability = state.sealed_field("value")?.expect("actual native member");
            assert_eq!(
                capability.layout.storage_kind,
                SealedFieldStorageKind::NativeObjectMember
            );
            assert_eq!(capability.layout.field_index, 0);
            assert!(capability.layout.object_offset >= std::mem::size_of::<ffi::PyObject>());
            assert_ne!(
                capability.layout.object_offset,
                capability.layout.field_index
            );
            let receiver = class.call0()?;
            let name = PyString::new(py, "value");
            assert!(!receiver.hasattr("__dict__")?);
            assert!(probe_sealed_field(&capability, &receiver, &name)?.is_null());
            assert!(
                receiver
                    .getattr(&name)
                    .unwrap_err()
                    .is_instance_of::<pyo3::exceptions::PyAttributeError>(py)
            );
            let value = py.eval(c"int('1000000')", None, None)?;
            receiver.setattr(&name, &value)?;
            let before = unsafe { ffi::Py_REFCNT(value.as_ptr()) };
            assert_eq!(
                probe_sealed_field(&capability, &receiver, &name)?,
                value.as_ptr()
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value.as_ptr()) },
                before,
                "native probe must borrow"
            );
            receiver.delattr(&name)?;
            assert!(probe_sealed_field(&capability, &receiver, &name)?.is_null());
            receiver.setattr(&name, &value)?;
            let globals = PyDict::new(py);
            globals.set_item("Base", &class)?;
            py.run(c"class Ordinary(Base): pass\n", Some(&globals), None)?;
            let ordinary = globals.get_item("Ordinary")?.unwrap().call0()?;
            ordinary.setattr(&name, &value)?;
            assert!(!capability.matches_receiver(&ordinary)?);
            assert!(probe_sealed_field(&capability, &ordinary, &name)?.is_null());
            let (other_state, other_class) = fixture.construct(&fixture.namespace()?)?;
            other_state.seal()?;
            assert_eq!(other_state.source(), state.source());
            assert!(!capability.matches_receiver(&other_class.call0()?)?);
            Ok(())
        })
    }

    #[test]
    fn source_slot_capability_has_no_class_lifetime_edge() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::with_object_slots(py, true)?;
            let (state, class) = fixture.construct(&fixture.namespace()?)?;
            state.seal()?;
            let capability = state.sealed_field("value")?.unwrap();
            let weak = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    ffi::PyWeakref_NewRef(class.as_ptr(), ptr::null_mut()),
                )
            }?;
            drop(state);
            drop(class);
            py.import("gc")?.call_method0("collect")?;
            assert!(
                weak.call0()?.is_none(),
                "a compiled native-member witness retained its class"
            );
            assert_eq!(
                capability.layout.storage_kind,
                SealedFieldStorageKind::NativeObjectMember
            );
            Ok(())
        })
    }

    #[test]
    fn escaped_owner_cannot_authenticate_a_reconstructed_type_at_a_reused_address() -> PyResult<()>
    {
        let _lock = native_lock();
        Python::attach(|py| {
            // Use the supported C constructors, not a forged Rust payload or
            // direct memory mutation. Import the existing native ABI fixture
            // before freeing the original type so imports cannot consume its
            // freed allocation and obscure the address-reuse discriminator.
            let raw = PyModule::new(py, "escaped_class_owner_replay")?;
            raw.add("_repository", concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))?;
            py.run(
                c"import ctypes\nimport sys\nsys.path.insert(0, _repository)\ntry:\n    from tests.test_strict_type_native import ConstructionSpec, TypeContractSpecV4\n    from tests.test_strict_cpython_native import native_api\nfinally:\n    sys.path.pop(0)\nnew_handle = native_api('PyType_NewSoacConstructionHandle', ctypes.py_object, ctypes.POINTER(ConstructionSpec))\nconstruct = native_api('PyType_FromSoacConstructionHandle', ctypes.py_object, ctypes.py_object, ctypes.py_object)\nseal = native_api('PyType_SealSoacContract', ctypes.c_int, ctypes.py_object, ctypes.py_object)\ndef reconstruct(owner):\n    namespace_function = lambda: None\n    namespace = {'__slots__': ('other', 'replacement', '__weakref__')}\n    payload = TypeContractSpecV4(0, 0, (), (), (), ('other', 'replacement'), None, None, None)\n    spec = ConstructionSpec(4, ctypes.sizeof(ConstructionSpec), 0, 0, owner, namespace_function, 'Reconstructed', (), namespace, {}, None, None, payload)\n    handle = new_handle(ctypes.byref(spec))\n    actual = construct(handle, namespace_function)\n    assert seal(actual, owner) == 0\n    return actual\n",
                Some(&raw.dict()),
                None,
            )?;
            let reconstruct = raw.getattr("reconstruct")?;
            let gc = py.import("gc")?;
            let fixture = FieldCapabilityFixture::with_object_slots(py, true)?;
            let (state, class) = fixture.construct(&fixture.namespace()?)?;
            state.seal()?;
            let capability = state.sealed_field("value")?.unwrap();
            let owner = state.owner().clone();
            let address = class.as_ptr() as usize;
            let weak = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    ffi::PyWeakref_NewRef(class.as_ptr(), ptr::null_mut()),
                )
            }?;

            let distinct = reconstruct.call1((&owner,))?;
            assert_ne!(distinct.as_ptr(), class.as_ptr());
            assert!(for_actual_type(py, &distinct).is_err());
            assert!(!capability.matches_receiver(&distinct.call0()?)?);
            drop(distinct);
            gc.call_method0("collect")?;
            drop(state);
            drop(class);
            gc.call_method0("collect")?;
            assert!(
                weak.call0()?.is_none(),
                "the exposed owner retained the original type"
            );

            let mut reused = None;
            let mut other_candidates = Vec::new();
            for _ in 0..4096 {
                let candidate = reconstruct.call1((&owner,))?;
                if candidate.as_ptr() as usize == address {
                    reused = Some(candidate);
                    break;
                }
                // Keep misses live: repeatedly freeing the most recent type
                // could only reuse that other address on a LIFO allocator.
                other_candidates.push(candidate);
            }
            let reconstructed =
                reused.expect("bounded native allocation did not reuse the freed type address");
            assert_eq!(
                unsafe { PyType_GetSoacContractOwner(reconstructed.as_ptr()) },
                owner.as_ptr(),
                "the native constructor did not retain the supplied exposed owner"
            );
            let receiver = reconstructed.call0()?;
            let value = pyo3::types::PyString::new(py, "not the original value slot");
            receiver.setattr("replacement", &value)?;
            assert!(!receiver.hasattr("value")?);
            let accepts_owner = matches!(for_actual_type(py, &reconstructed), Ok(Some(_)));
            let accepts_capability = capability.matches_receiver(&receiver).unwrap_or(false);
            let raw_value = probe_sealed_field(
                &capability,
                &receiver,
                &pyo3::types::PyString::new(py, "value"),
            )
            .unwrap_or(ptr::null_mut());
            assert!(
                !accepts_owner && !accepts_capability && raw_value.is_null(),
                "reused native type address authenticated an old Rust owner: owner={accepts_owner}, capability={accepts_capability}, wrong_member_read={}",
                raw_value == value.as_ptr(),
            );
            Ok(())
        })
    }

    #[test]
    fn ordinary_pending_storage_keeps_values_across_delete_clear_and_growth() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new(py)?;
            let (state, class) = fixture.construct(&fixture.namespace()?)?;
            state.seal()?;
            assert_eq!(state.storage()?.data().model, StorageModel::Ordinary);
            assert!(state.storage()?.data().template.is_none());
            assert!(state.sealed_field("value")?.is_none());
            let name = PyString::new(py, "value");
            let receiver = class.call0()?;
            let dictionary = receiver.getattr("__dict__")?.cast_into::<PyDict>()?;
            assert!(
                state.sealed_field("value")?.is_none(),
                "materialization is not indexed-layout authority"
            );
            assert!(
                receiver
                    .getattr(&name)
                    .unwrap_err()
                    .is_instance_of::<pyo3::exceptions::PyAttributeError>(py)
            );
            let value = py.eval(c"int('1000000')", None, None)?;
            dictionary.set_item(&name, &value)?;
            let before = unsafe { ffi::Py_REFCNT(value.as_ptr()) };
            assert_eq!(receiver.getattr(&name)?.as_ptr(), value.as_ptr());
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value.as_ptr()) },
                before,
                "generic lookup must release its ordinary temporary"
            );
            for index in 0..96 {
                dictionary.set_item(format!("overflow_{index}"), index)?;
            }
            assert_eq!(receiver.getattr(&name)?.as_ptr(), value.as_ptr());
            assert_eq!(
                unsafe { _PyDict_IndexedKeyIndex(dictionary.as_ptr(), name.as_ptr()) },
                -1
            );
            // This API rejects ordinary storage; consume its error before
            // continuing to exercise the dictionary's normal operations.
            assert!(PyErr::fetch(py).is_instance_of::<pyo3::exceptions::PyTypeError>(py));
            dictionary.del_item(&name)?;
            assert!(!receiver.hasattr(&name)?);
            dictionary.set_item(&name, &value)?;
            dictionary.call_method0("clear")?;
            assert!(!receiver.hasattr(&name)?);
            dictionary.set_item(&name, &value)?;
            assert_eq!(receiver.getattr(&name)?.as_ptr(), value.as_ptr());
            assert_eq!(dictionary.len(), 1);
            assert!(state.sealed_field("value")?.is_none());
            Ok(())
        })
    }

    #[test]
    fn ordinary_pending_field_keeps_default_descriptor_and_actual_type_lookup() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new(py)?;
            let globals = PyDict::new(py);
            py.run(c"class Default: pass\nclass Descriptor:\n def __get__(self, instance, owner): return 'descriptor result'\n def __set__(self, instance, value): pass\ndefault = Default()\n", Some(&globals), None)?;
            let default = globals.get_item("default")?.unwrap();
            let default_type = globals.get_item("Default")?.unwrap();
            let namespace = fixture.namespace()?;
            namespace.set_item("temporary", 1)?;
            namespace.set_item("value", &default)?;
            namespace.del_item("temporary")?;
            let (state, class) = fixture.construct(&namespace)?;
            state.seal()?;
            assert!(state.sealed_field("value")?.is_none());
            let receiver = class.call0()?;
            assert_eq!(receiver.getattr("value")?.as_ptr(), default.as_ptr());
            let value = py.eval(c"int('1000000')", None, None)?;
            receiver.setattr("value", &value)?;
            assert_eq!(receiver.getattr("value")?.as_ptr(), value.as_ptr());
            default_type.setattr(
                "__get__",
                py.eval(
                    c"lambda self, instance, owner: 'new descriptor'",
                    None,
                    None,
                )?,
            )?;
            default_type.setattr(
                "__set__",
                py.eval(c"lambda self, instance, value: None", None, None)?,
            )?;
            assert_eq!(
                receiver.getattr("value")?.extract::<String>()?,
                "new descriptor"
            );
            assert!(state.sealed_field("value")?.is_none());
            default_type.delattr("__get__")?;
            default_type.delattr("__set__")?;
            assert_eq!(receiver.getattr("value")?.as_ptr(), value.as_ptr());
            default.setattr("__class__", globals.get_item("Descriptor")?.unwrap())?;
            assert_eq!(
                receiver.getattr("value")?.extract::<String>()?,
                "descriptor result"
            );
            Ok(())
        })
    }

    #[test]
    fn ordinary_pending_field_keeps_mutable_key_aliases_on_generic_lookup() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new(py)?;
            let (state, class) = fixture.construct(&fixture.namespace()?)?;
            state.seal()?;
            assert!(state.sealed_field("value")?.is_none());
            let name = PyString::new(py, "value");
            let receiver = class.call0()?;
            let dictionary = receiver.getattr("__dict__")?.cast_into::<PyDict>()?;
            let globals = PyDict::new(py);
            py.run(c"class Alias:\n enabled = False\n def __hash__(self): return hash('value')\n def __eq__(self, other): return self.enabled and other == 'value'\nalias = Alias()\n", Some(&globals), None)?;
            let alias = globals.get_item("alias")?.unwrap();
            let ordinary = py.eval(c"int('1000001')", None, None)?;
            let aliased = py.eval(c"int('1000002')", None, None)?;
            dictionary.set_item(&alias, &aliased)?;
            dictionary.set_item(&name, &ordinary)?;
            assert_eq!(receiver.getattr("value")?.as_ptr(), ordinary.as_ptr());
            assert!(state.sealed_field("value")?.is_none());
            let size = dictionary.len();
            alias.setattr("enabled", true)?;
            assert_eq!(
                dictionary.len(),
                size,
                "alias equality changed without a dictionary write"
            );
            assert_eq!(receiver.getattr("value")?.as_ptr(), aliased.as_ptr());
            assert!(state.sealed_field("value")?.is_none());
            dictionary.del_item(&alias)?;
            assert_eq!(receiver.getattr("value")?.as_ptr(), ordinary.as_ptr());
            assert!(
                state.sealed_field("value")?.is_none(),
                "removing an alias does not manufacture an indexed capability"
            );
            Ok(())
        })
    }

    #[test]
    fn ordinary_pending_storage_does_not_keep_class_or_default_alive() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new(py)?;
            let globals = PyDict::new(py);
            py.run(c"class Default: pass\n", Some(&globals), None)?;
            let default = globals.get_item("Default")?.unwrap().call0()?;
            let namespace = fixture.namespace()?;
            namespace.set_item("value", &default)?;
            let (state, class) = fixture.construct(&namespace)?;
            state.seal()?;
            fixture.module_state.seal(py)?;
            assert!(state.sealed_field("value")?.is_none());
            let receiver = class.call0()?;
            let dictionary = receiver.getattr("__dict__")?.cast_into::<PyDict>()?;
            receiver.setattr("value", 23)?;
            let weakref = py.import("weakref")?.getattr("ref")?;
            let class_ref = weakref.call1((&class,))?;
            let default_ref = weakref.call1((&default,))?;
            drop(namespace);
            drop(default);
            drop(fixture);
            // A sealed class/storage contract survives actual module globals
            // teardown without retaining them merely to keep policy live.
            assert_eq!(receiver.getattr("value")?.extract::<i32>()?, 23);
            drop(receiver);
            drop(class);
            drop(state);
            py.import("gc")?.call_method0("collect")?;
            assert!(
                class_ref.call0()?.is_none(),
                "escaped ordinary storage must not pin the class"
            );
            assert!(
                default_ref.call0()?.is_none(),
                "escaped ordinary storage must not pin class defaults"
            );
            dictionary.set_item("value", 24)?;
            assert_eq!(dictionary.get_item("value")?.unwrap().extract::<i32>()?, 24);
            Ok(())
        })
    }

    #[test]
    fn strict_class_namespace_requires_native_binding_and_execution_identity() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new(py)?;
            let input_namespace = fixture.namespace()?;
            let (state, class) = fixture.construct(&input_namespace)?;
            let actual_namespace = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(py, PyType_GetDict(class.as_ptr().cast()))?
            }
            .cast_into::<PyDict>()?;
            assert!(!state.is_finalized());
            assert!(matches_class_namespace(
                py,
                actual_namespace.as_any(),
                state.namespace_execution(),
            )?);
            assert!(!matches_class_namespace(
                py,
                input_namespace.as_any(),
                state.namespace_execution(),
            )?);
            assert!(!matches_class_namespace(
                py,
                actual_namespace.copy()?.as_any(),
                state.namespace_execution(),
            )?);

            // Even coordinates that match the real native owner/dictionary
            // cannot authorize a different execution with identical source.
            let impostor = crate::strict_namespace::NamespaceExecution::completed_identity_for_test(
                state.source().clone(),
                interpreter_id(),
            );
            impostor.record_class_dictionary(py, state.owner(), actual_namespace.as_any())?;
            assert!(!matches_class_namespace(
                py,
                actual_namespace.as_any(),
                &impostor,
            )?);

            state.seal()?;
            assert!(matches_class_namespace(
                py,
                actual_namespace.as_any(),
                state.namespace_execution(),
            )?);
            Ok(())
        })
    }

    #[test]
    fn sealed_field_owner_and_escaped_class_namespace_do_not_keep_type_alive() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new(py)?;
            let namespace = fixture.namespace()?;
            let (state, class) = fixture.construct(&namespace)?;
            state.seal()?;
            let owner = state.owner().clone();
            let execution = Arc::clone(state.namespace_execution());
            let actual_namespace = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(py, PyType_GetDict(class.as_ptr().cast()))?
            }
            .cast_into::<PyDict>()?;
            let reference = py.import("weakref")?.getattr("ref")?.call1((&class,))?;
            assert!(!actual_namespace.is_empty());
            drop(namespace);
            drop(state);
            drop(class);
            py.import("gc")?.call_method0("collect")?;
            assert!(
                reference.call0()?.is_none(),
                "escaped policy/dict must not own the type"
            );
            assert!(actual_namespace.is_empty());
            assert!(matches_class_namespace(py, actual_namespace.as_any(), &execution).is_err());
            // GC introspection can keep the opaque owner alive. Its old
            // comparison address must not be turned back into a Python ref.
            let detached = StrictClassState {
                state: StrictStateRef::from_owner(owner)?,
                actual_type: None,
            };
            assert!(detached.actual_type().is_err());
            assert!(detached.sealed_field("value").is_err());
            Ok(())
        })
    }

    #[test]
    fn caught_final_native_commit_failure_removes_only_its_weak_pending_record() -> PyResult<()> {
        // PySoacTypeConstructionInfoV1 phase from the actual ABI4 fixture.
        const FAILED_PHASE: u32 = 4;
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new_method_source(py)?;
            let shared = fixture.run_body()?;
            fixture.finalize(&shared)?;
            let class = fixture.module.getattr("first")?;
            let method = class.getattr("method")?;
            let auth = authenticate_strict_function(py, &method)?.unwrap();
            let fact = &fixture.verified.type_facts().facts().classes[0];
            let kind = crate::StrictPendingKind::Class {
                source: fact.identity.clone(),
            };
            let execution = fixture.module_state.execution_ref();
            let raw = PyModule::new(py, "failed_pending_class_cleanup")?;
            raw.add("_repository", concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))?;
            py.run(c"import sys\nsys.path.insert(0, _repository)\ntry:\n from tests.test_strict_type_native import PendingTypeNativeTests\nfinally:\n sys.path.pop(0)\nPendingTypeNativeTests.setUpClass()\ncase = PendingTypeNativeTests()\nprimary = LookupError('final native commit marker')\ncontext = ValueError('original context')\nprimary.__context__ = context\nfailed, owner, root = case.pending(owner=case.owner(error=primary))\n",
                Some(&raw.dict()), None)?;
            let failed = raw.getattr("failed")?;
            execution.register_pending(
                py,
                &fixture.module.dict(),
                &fixture.verified,
                kind.clone(),
                &failed,
            )?;
            // An unrelated already-admitted actual source class remains in
            // the same registry; cleanup must neither drain nor revoke it.
            execution.register_pending(
                py,
                &fixture.module.dict(),
                &fixture.verified,
                kind,
                &class,
            )?;
            let case = raw.getattr("case")?;
            let error = case
                .call_method1(
                    "admit",
                    (&failed, raw.getattr("owner")?, raw.getattr("root")?),
                )
                .unwrap_err();
            let primary = raw.getattr("primary")?;
            assert_eq!(error.value(py).as_ptr(), primary.as_ptr());
            let info = case.call_method1("info", (&failed,))?;
            assert_eq!(info.getattr("phase")?.extract::<u32>()?, FAILED_PHASE);
            assert_eq!(
                info.getattr("permanent_contract_published")?
                    .extract::<u32>()?,
                1
            );
            assert!(
                for_constructed_type(py, &failed).is_err(),
                "the test must reach the terminal native-owner query refusal"
            );
            assert!(
                crate::strict_class_decorator::forget_failed_registered_class_for_test(
                    &auth, fact, &failed,
                )
                .is_err()
            );
            assert_eq!(error.value(py).as_ptr(), primary.as_ptr());
            assert_eq!(
                primary.getattr("__context__")?.as_ptr(),
                raw.getattr("context")?.as_ptr()
            );
            let Some((remaining_kind, remaining)) = fixture.module_state.next_pending(py)? else {
                panic!("cleanup discarded the unrelated live record");
            };
            assert_eq!(
                remaining_kind,
                crate::StrictPendingKind::Class {
                    source: fact.identity.clone()
                }
            );
            let remaining = remaining.into_bound(py);
            assert_eq!(
                remaining.as_ptr(),
                class.as_ptr(),
                "a terminal failed record survived cleanup and would poison the next drain"
            );
            assert!(fixture.module_state.next_pending(py)?.is_none());
            assert!(crate::strict_class::finalize_class(
                py,
                &remaining,
                &fact.identity
            )?);
            let later = fixture.module.getattr("make")?.call0()?;
            assert!(for_actual_type(py, &later)?.is_some());
            assert_eq!(
                case.call_method1("info", (&failed,))?
                    .getattr("phase")?
                    .extract::<u32>()?,
                FAILED_PHASE
            );
            assert!(
                failed.call0().is_err(),
                "cleanup revoked a failed construction barrier"
            );
            Ok(())
        })
    }

    fn assert_catalog_rejects_template_change(
        shared: &crate::module_type::SharedModuleState,
        function: &soac_core::block_py::BlockPyFunction<soac_ir_blockpy::BlockPyModuleShape>,
        label: &str,
        change: impl FnOnce(
            &mut soac_core::block_py::BlockPyFunction<soac_ir_blockpy::BlockPyModuleShape>,
        ),
    ) {
        let mut altered = function.clone();
        change(&mut altered);
        assert!(
            !shared.admits_function(&altered),
            "{label} changed the admitted template {}",
            function.names.qualname,
        );
    }

    #[test]
    fn strict_code_catalog_accepts_only_actual_source_and_helper_shapes() -> PyResult<()> {
        use soac_core::block_py::{
            ClosureInit, ClosureSlot, FunctionKind, Param, ParamKind, RuntimeFunctionId,
        };

        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new_method_source(py)?;
            let shared = fixture.run_body()?;
            let mut roles = (false, false, false);
            for function in &shared.lowered_module.callable_defs {
                assert!(shared.admits_function(function));
                match function
                    .scope
                    .source_origin
                    .as_ref()
                    .map(|origin| origin.role)
                {
                    Some(CallableSourceRole::ModuleBody) => roles.0 = true,
                    Some(CallableSourceRole::SourceFunction) => roles.1 = true,
                    Some(CallableSourceRole::ClassNamespace) => roles.2 = true,
                    _ => (),
                }
                assert_catalog_rejects_template_change(
                    &shared,
                    function,
                    "source role",
                    |altered| {
                        let origin = altered.scope.source_origin.as_mut().unwrap();
                        origin.role = if origin.role == CallableSourceRole::ModuleBody {
                            CallableSourceRole::SourceFunction
                        } else {
                            CallableSourceRole::ModuleBody
                        };
                    },
                );
                assert_catalog_rejects_template_change(
                    &shared,
                    function,
                    "source definition",
                    |altered| {
                        altered
                            .scope
                            .source_origin
                            .as_mut()
                            .unwrap()
                            .definition
                            .lexical_qualname
                            .push_str(".unrelated");
                    },
                );
                assert_catalog_rejects_template_change(
                    &shared,
                    function,
                    "function kind",
                    |altered| {
                        altered.kind = if altered.kind == FunctionKind::Function {
                            FunctionKind::Generator
                        } else {
                            FunctionKind::Function
                        };
                    },
                );
                assert_catalog_rejects_template_change(
                    &shared,
                    function,
                    "public parameters",
                    |altered| {
                        altered.params.params.push(Param {
                            name: "unrelated".into(),
                            kind: ParamKind::Any,
                            has_default: false,
                        });
                    },
                );
                assert_catalog_rejects_template_change(
                    &shared,
                    function,
                    "body parameters",
                    |altered| {
                        let mut parameters = altered.body_params().clone();
                        parameters.params.push(Param {
                            name: "unrelated".into(),
                            kind: ParamKind::Any,
                            has_default: false,
                        });
                        altered.body_params = Some(parameters);
                    },
                );
                assert_catalog_rejects_template_change(
                    &shared,
                    function,
                    "capture layout",
                    |altered| {
                        let mut layout =
                            altered.public_storage_layout().cloned().unwrap_or_default();
                        layout.freevars.push(ClosureSlot {
                            logical_name: "unrelated".into(),
                            storage_name: "unrelated".into(),
                            init: ClosureInit::InheritedCapture,
                        });
                        altered.public_storage_layout = Some(layout);
                    },
                );
                assert_catalog_rejects_template_change(
                    &shared,
                    function,
                    "capture source",
                    |altered| {
                        altered
                            .scope
                            .cell_capture_source_names
                            .insert("unrelated".into(), "binding".into());
                    },
                );
                assert_catalog_rejects_template_change(
                    &shared,
                    function,
                    "capture alias",
                    |altered| {
                        altered
                            .scope
                            .cell_value_aliases
                            .insert("unrelated".into(), "binding".into());
                    },
                );
                assert_catalog_rejects_template_change(
                    &shared,
                    function,
                    "function identity",
                    |altered| {
                        altered.function_id =
                            RuntimeFunctionId::from_raw_parts(shared.module_id(), u32::MAX - 1);
                    },
                );
            }
            assert_eq!(
                roles,
                (true, true, true),
                "fixture executes source, module, and class-namespace templates"
            );
            Ok(())
        })
    }

    #[test]
    fn strict_code_catalog_cannot_be_replaced_by_inspection_ir_and_originals() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new_method_source(py)?;
            let shared = fixture.run_body()?;
            let originals = shared
                .lowered_module
                .callable_defs
                .iter()
                .filter_map(|function| {
                    shared
                        .lookup_original_code(function.function_id)
                        .map(|code| (function.function_id, code.clone_ref(py)))
                })
                .collect();
            let mut inspection = crate::module_type::build_shared_state_for_inspection_with_original_code_and_source_hash(
                py, shared.lowered_module.clone(), &shared.module_name, &shared.package_name,
                shared.source_hash, Some(std::str::from_utf8(fixture.verified.source()).unwrap()), originals,
            )?;
            // Every other input is the actual source/execution witness. An
            // ordinary map of even genuine native code objects is not the
            // unforgeable catalogue produced by complete source matching.
            let unpublished = Arc::get_mut(&mut inspection).unwrap();
            unpublished.strict_module = Some(Arc::clone(&fixture.verified));
            unpublished.strict_execution = Some(fixture.module_state.execution_ref());
            let session = crate::CompileSession::process();
            for function in &inspection.lowered_module.callable_defs {
                assert!(!inspection.admits_function(function));
                assert!(
                    inspection
                        .lookup_or_compile_direct_function_handle(&session, function.function_id,)
                        .is_err(),
                    "even a ready authentic cache entry cannot authorize inspection state"
                );
            }

            let factory = inspection
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::SourceFunction
                            && origin.definition.lexical_qualname == "make"
                    })
                })
                .expect("actual no-argument factory template");
            let globals = fixture.module.dict();
            let runtime = crate::ModuleRuntimeContext {
                mod_ctx: crate::ModuleJitContext {
                    shared_module_state: Arc::as_ptr(&inspection),
                    globals_obj: globals.clone().into_any().into_ptr().cast(),
                },
                compile_session: session,
                shared_module_state_owner: Arc::clone(&inspection),
            };
            let empty = PyTuple::empty(py);
            let result = crate::instantiate_bb_function(
                py,
                &py.import("soac.runtime")?,
                &inspection.module_name,
                factory,
                empty.as_any(),
                empty.as_any(),
                globals.as_any(),
                py.None().bind(py),
                &runtime,
            );
            assert!(
                result.is_err(),
                "inspection code cannot register an executing source function"
            );
            let authentic = fixture.module.getattr("make")?.call0()?;
            assert!(for_actual_type(py, &authentic)?.is_some());
            Ok(())
        })
    }

    #[test]
    fn sealed_method_capability_uses_real_adoption_and_checked_public_entry() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::method_source(py, true)?;
            let globals = PyDict::new(py);
            py.run(c"events = []\nclass DefaultKey:\n def __hash__(self): return hash('amount')\n def __eq__(self, other): events.append('default'); return other == 'amount'\nclass Alias:\n def __hash__(self): return hash('method')\n def __eq__(self, other): events.append('alias'); return other == 'method'\ndefaults = {DefaultKey(): 7}\nalias = Alias()\ndef prepare_defaults(function):\n function.__kwdefaults__ = defaults\n", Some(&globals), None)?;
            fixture.module.dict().set_item(
                "_prepare_method_defaults",
                globals.get_item("prepare_defaults")?.unwrap(),
            )?;
            let shared = fixture.run_body()?;
            fixture.module.dict().del_item("_prepare_method_defaults")?;
            let first = fixture.module.getattr("first")?;
            let second = fixture.module.getattr("second")?;
            let state = for_actual_type(py, &first)?.expect("actual source class owner");
            let second_state = for_actual_type(py, &second)?.expect("second actual source owner");
            assert!(!state.is_finalized());
            assert!(state.sealed_method("method")?.is_none());
            let method = first.getattr("method")?;
            assert!(
                authenticate_strict_function(py, &method)?
                    .unwrap()
                    .is_finalized(),
                "required metadata seals before instances, not at module finalization",
            );
            // No instance fields or checked-field policy were selected here.
            // Protected methods do not require an instance-dictionary policy.
            assert_eq!(state.dictionary_mode()?, 0);
            assert!(state.sealed_field("value")?.is_none());
            // The class-suite callback installed the actual arbitrary key.
            // Required metadata is already immutable, but optional method
            // publication above still waits for module binding finality.
            let error = method
                .setattr("__kwdefaults__", PyDict::new(py))
                .unwrap_err();
            assert!(error.is_instance(
                py,
                &py.import("soac.strict")?.getattr("StrictMutationError")?
            ));
            assert_eq!(
                method.getattr("__kwdefaults__")?.as_ptr(),
                globals.get_item("defaults")?.unwrap().as_ptr(),
            );
            fixture.finalize(&shared)?;
            assert!(
                authenticate_strict_function(py, &method)?
                    .unwrap()
                    .is_finalized()
            );
            let capability = state
                .sealed_method("method")?
                .expect("adopted protected method");
            let family = state
                .sealed_virtual_method("method")?
                .expect("adopted method family");
            let checked_entry = crate::private_checked_vectorcall_entry(&method)?
                .expect("actual sealed checked native entry");
            let second_capability = second_state.sealed_method("method")?.unwrap();
            assert_eq!(capability.source(), second_capability.source());
            assert_eq!(
                capability.method_source(),
                second_capability.method_source()
            );
            assert_eq!(capability.method_name(), "method");
            assert!(state.sealed_method("value")?.is_none());
            let receiver = first.call0()?;
            receiver.setattr("value", 23)?;
            assert!(capability.resolve(&second.call0()?)?.is_none());
            assert!(second_capability.resolve(&receiver)?.is_none());

            // The profile identity is about this exact authenticated source
            // function, not an unchecked-body permission or a globals label.
            let identity = authenticate_strict_function(py, &method)?
                .unwrap()
                .function_id()?;
            assert_eq!(
                unsafe { crate::observed_strict_function_id(method.as_ptr()) },
                Some(identity)
            );
            let bound = receiver.getattr("method")?;
            assert_eq!(
                unsafe { crate::observed_strict_function_id(bound.as_ptr()) },
                Some(identity)
            );
            assert_eq!(
                unsafe { crate::registered_clif_function_id(method.as_ptr()) }.unwrap(),
                None,
                "an observed target did not mint an unchecked call token"
            );
            let copied = py.import("types")?.getattr("FunctionType")?.call1((
                method.getattr("__code__")?,
                method.getattr("__globals__")?,
                "copied",
                method.getattr("__defaults__")?,
                method.getattr("__closure__")?,
            ))?;
            assert_eq!(
                unsafe { crate::observed_strict_function_id(copied.as_ptr()) },
                None
            );
            assert!(crate::private_checked_vectorcall_entry(&copied)?.is_none());
            drop(copied);
            drop(bound);

            let before = unsafe { ffi::Py_REFCNT(method.as_ptr()) };
            let mut callee = ptr::null_mut();
            assert_eq!(
                unsafe {
                    dp_jit_resolve_sealed_method_capability(
                        receiver.as_ptr(),
                        &capability,
                        &mut callee,
                    )
                },
                1
            );
            assert_eq!(callee, method.as_ptr());
            assert_eq!(
                unsafe { ffi::Py_REFCNT(method.as_ptr()) },
                before,
                "C resolver result is borrowed"
            );
            let mut virtual_target = RawSealedMethodTarget::empty();
            assert_eq!(
                unsafe {
                    dp_jit_resolve_sealed_virtual_method_capability(
                        receiver.as_ptr(),
                        &family,
                        &mut virtual_target,
                    )
                },
                1
            );
            assert_eq!(virtual_target.callee, method.as_ptr());
            assert!(std::ptr::fn_addr_eq(
                virtual_target.entry.unwrap(),
                checked_entry
            ));
            assert_eq!(unsafe { ffi::Py_REFCNT(method.as_ptr()) }, before);
            assert!(
                globals
                    .get_item("events")?
                    .unwrap()
                    .extract::<Vec<String>>()?
                    .is_empty()
            );
            // This INCREF precedes the call/any cleanup, as required by the
            // borrowed C ABI. Calling the actual function uses its existing
            // checked vectorcall trampoline, not a direct-body entry.
            let callee = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, callee) };
            let public_entry =
                unsafe { (*callee.as_ptr().cast::<ffi::PyFunctionObject>()).vectorcall };
            assert!(std::ptr::fn_addr_eq(public_entry.unwrap(), checked_entry));
            let arguments = [receiver.as_ptr()];
            let result = unsafe {
                checked_entry(
                    callee.as_ptr(),
                    arguments.as_ptr(),
                    arguments.len(),
                    ptr::null_mut(),
                )
            };
            let result = unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, result) }?;
            assert_eq!(result.extract::<i32>()?, 30);
            assert_eq!(
                globals
                    .get_item("events")?
                    .unwrap()
                    .extract::<Vec<String>>()?,
                ["default"]
            );
            assert!(
                callee
                    .call1((&receiver, 5))
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            globals.get_item("events")?.unwrap().call_method0("clear")?;

            let dictionary = receiver.getattr("__dict__")?.cast_into::<PyDict>()?;
            dictionary.set_item(globals.get_item("alias")?.unwrap(), "hidden mapping value")?;
            let ordinary_lookup = receiver.getattr("method")?;
            assert_eq!(
                ordinary_lookup.getattr("__func__")?.as_ptr(),
                method.as_ptr()
            );
            assert_eq!(
                capability.resolve(&receiver)?.unwrap().as_ptr(),
                method.as_ptr()
            );
            assert!(
                globals
                    .get_item("events")?
                    .unwrap()
                    .extract::<Vec<String>>()?
                    .is_empty(),
                "protected method lookup skips arbitrary instance aliases and defaults"
            );

            globals.set_item("Base", &first)?;
            py.run(
                c"class OrdinaryChild(Base):\n def method(self): return -1\n",
                Some(&globals),
                None,
            )?;
            let child = globals.get_item("OrdinaryChild")?.unwrap().call0()?;
            let mut miss = method.as_ptr();
            assert_eq!(
                unsafe {
                    dp_jit_resolve_sealed_method_capability(child.as_ptr(), &capability, &mut miss)
                },
                0
            );
            assert!(miss.is_null());
            assert_eq!(child.call_method0("method")?.extract::<i32>()?, -1);
            Ok(())
        })
    }

    #[test]
    fn sealed_method_capability_does_not_keep_class_function_or_defaults_alive() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::method_source(py, true)?;
            let globals = PyDict::new(py);
            py.run(c"class Default: pass\ndef prepare_defaults(function):\n function.__kwdefaults__ = defaults\n", Some(&globals), None)?;
            let default = globals.get_item("Default")?.unwrap().call0()?;
            let defaults = PyDict::new(py);
            defaults.set_item("amount", &default)?;
            globals.set_item("defaults", &defaults)?;
            fixture.module.dict().set_item(
                "_prepare_method_defaults",
                globals.get_item("prepare_defaults")?.unwrap(),
            )?;
            let shared = fixture.run_body()?;
            fixture.module.dict().del_item("_prepare_method_defaults")?;
            globals.del_item("defaults")?;
            let class = fixture.module.getattr("first")?;
            let method = class.getattr("method")?;
            assert_eq!(
                method.getattr("__kwdefaults__")?.as_ptr(),
                defaults.as_ptr()
            );
            fixture.finalize(&shared)?;
            let state = for_actual_type(py, &class)?.unwrap();
            let capability = state.sealed_method("method")?.unwrap();
            let weakref = py.import("weakref")?.getattr("ref")?;
            let class_ref = weakref.call1((&class,))?;
            let function_ref = weakref.call1((&method,))?;
            let default_ref = weakref.call1((&default,))?;
            drop(defaults);
            drop(default);
            drop(method);
            drop(class);
            drop(state);
            drop(shared);
            drop(fixture);
            py.import("gc")?.call_method0("collect")?;
            assert!(class_ref.call0()?.is_none());
            assert!(function_ref.call0()?.is_none());
            assert!(default_ref.call0()?.is_none());
            assert_eq!(capability.method_name(), "method");
            Ok(())
        })
    }

    #[test]
    fn sealed_virtual_method_rows_follow_actual_overrides_and_field_misses() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let fixture = FieldCapabilityFixture::new_virtual_method_source(py)?;
            let shared = fixture.run_body()?;
            let first = fixture.module.getattr("first")?.cast_into::<PyTuple>()?;
            let second = fixture.module.getattr("second")?.cast_into::<PyTuple>()?;
            let base = first.get_item(0)?;
            let overriding = first.get_item(1)?;
            let shadow_class = first.get_item(4)?;
            let base_state = for_actual_type(py, &base)?.expect("actual Base owner");
            assert!(base_state.sealed_virtual_method("method")?.is_none());
            fixture.finalize(&shared)?;
            let family = base_state.sealed_virtual_method("method")?.unwrap();
            let overriding_state = for_actual_type(py, &overriding)?.unwrap();
            let overriding_family = overriding_state.sealed_virtual_method("method")?.unwrap();
            let base_reference = ClassReference {
                definition: base_state.source().clone(),
                source_digest: base_state
                    .verified_module()
                    .type_facts()
                    .facts()
                    .source_digest,
            };
            let inherited_request = overriding_state
                .sealed_virtual_method_for_source(&base_reference, "method")?
                .expect("actual Base family in Override MRO");
            assert!(Arc::ptr_eq(&family.family, &inherited_request.family));
            let mut wrong_reference = base_reference.clone();
            wrong_reference.source_digest = Fingerprint::digest(b"unrelated source");
            assert!(
                overriding_state
                    .sealed_virtual_method_for_source(&wrong_reference, "method")?
                    .is_none()
            );
            assert!(
                base_state
                    .sealed_virtual_method_for_source(
                        &ClassReference {
                            definition: overriding_state.source().clone(),
                            source_digest: base_reference.source_digest,
                        },
                        "method",
                    )?
                    .is_none()
            );

            let base_method = base.getattr("method")?;
            let override_method = overriding.getattr("method")?;
            for (index, expected_function, expected_result) in [
                (0, base_method.as_ptr(), 11),
                (1, override_method.as_ptr(), 20),
                (2, base_method.as_ptr(), 11),
                (3, override_method.as_ptr(), 20),
            ] {
                let receiver = first.get_item(index)?.call0()?;
                let resolved = family.resolve(&receiver)?.expect("actual family row");
                assert_eq!(resolved.as_ptr(), expected_function);
                assert_eq!(
                    resolved.call1((&receiver,))?.extract::<i32>()?,
                    expected_result
                );
                let override_target = overriding_family.resolve(&receiver)?;
                if matches!(index, 1 | 3) {
                    assert_eq!(override_target.unwrap().as_ptr(), override_method.as_ptr());
                } else {
                    assert!(
                        override_target.is_none(),
                        "a sibling/base is not a derived receiver"
                    );
                }
            }

            let second_base = second.get_item(0)?;
            let second_state = for_actual_type(py, &second_base)?.unwrap();
            let second_family = second_state.sealed_virtual_method("method")?.unwrap();
            assert_eq!(family.source(), second_family.source());
            assert!(!Arc::ptr_eq(&family.family, &second_family.family));
            assert!(family.resolve(&second.get_item(1)?.call0()?)?.is_none());
            assert!(second_family.resolve(&base.call0()?)?.is_none());

            let globals = PyDict::new(py);
            globals.set_item("Base", &base)?;
            py.run(
                c"class Ordinary(Base):\n def method(self): return -1\ndef field(): return 99\n",
                Some(&globals),
                None,
            )?;
            let ordinary = globals.get_item("Ordinary")?.unwrap().call0()?;
            assert!(family.resolve(&ordinary)?.is_none());
            assert_eq!(ordinary.call_method0("method")?.extract::<i32>()?, -1);
            let mut target = RawSealedMethodTarget {
                callee: base_method.as_ptr(),
                entry: crate::private_checked_vectorcall_entry(&base_method)?,
            };
            assert_eq!(
                unsafe {
                    dp_jit_resolve_sealed_virtual_method_capability(
                        ordinary.as_ptr(),
                        &family,
                        &mut target,
                    )
                },
                0
            );
            assert!(target.callee.is_null());
            assert!(target.entry.is_none());

            let shadow_state =
                for_actual_type(py, &shadow_class)?.expect("field override remains strict");
            assert!(shadow_state.sealed_virtual_method("method")?.is_none());
            let shadow_base_request = shadow_state
                .sealed_virtual_method_for_source(&base_reference, "method")?
                .expect("Base family exists even when this receiver has a row miss");
            let shadow = shadow_class.call0()?;
            shadow.setattr("method", globals.get_item("field")?.unwrap())?;
            assert!(shadow_base_request.resolve(&shadow)?.is_none());
            assert_eq!(shadow.call_method0("method")?.extract::<i32>()?, 99);
            Ok(())
        })
    }

    #[test]
    fn sealed_virtual_method_family_does_not_retain_python_ancestors_or_defaults() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let (family, references, escaped_namespace) = {
                let fixture = FieldCapabilityFixture::virtual_method_source(py, true)?;
                let globals = PyDict::new(py);
                py.run(
                    c"class Default: pass\nprepared = 0\ndef prepare_defaults(function):\n global prepared\n if prepared == 0:\n  function.__kwdefaults__ = defaults\n prepared += 1\n",
                    Some(&globals),
                    None,
                )?;
                let default = globals.get_item("Default")?.unwrap().call0()?;
                let defaults = PyDict::new(py);
                defaults.set_item("amount", &default)?;
                globals.set_item("defaults", &defaults)?;
                fixture.module.dict().set_item(
                    "_prepare_method_defaults",
                    globals.get_item("prepare_defaults")?.unwrap(),
                )?;
                let shared = fixture.run_body()?;
                fixture.module.dict().del_item("_prepare_method_defaults")?;
                globals.del_item("defaults")?;
                assert_eq!(
                    globals.get_item("prepared")?.unwrap().extract::<usize>()?,
                    2
                );
                let first = fixture.module.getattr("first")?.cast_into::<PyTuple>()?;
                let base = first.get_item(0)?;
                let overriding = first.get_item(1)?;
                let inherited = first.get_item(2)?;
                let method = overriding.getattr("method")?;
                assert_eq!(
                    method.getattr("__kwdefaults__")?.as_ptr(),
                    defaults.as_ptr()
                );
                let second = fixture.module.getattr("second")?.cast_into::<PyTuple>()?;
                assert_eq!(
                    second
                        .get_item(1)?
                        .getattr("method")?
                        .getattr("__kwdefaults__")?
                        .get_item("amount")?
                        .extract::<i32>()?,
                    2,
                    "only the original first Override receives the custom default"
                );
                fixture.finalize(&shared)?;
                let base_state = for_actual_type(py, &base)?.unwrap();
                let family = base_state.sealed_virtual_method("method")?.unwrap();
                let weakref = py.import("weakref")?.getattr("ref")?;
                let references = [&base, &overriding, &inherited, &method, &default]
                    .into_iter()
                    .map(|value| weakref.call1((value,)))
                    .collect::<PyResult<Vec<_>>>()?;
                let escaped_namespace = unsafe {
                    Bound::<PyAny>::from_owned_ptr_or_err(
                        py,
                        PyType_GetDict(inherited.as_ptr().cast()),
                    )
                }?
                .cast_into::<PyDict>()?;
                (family, references, escaped_namespace)
            };
            py.import("gc")?.call_method0("collect")?;
            for reference in references {
                assert!(reference.call0()?.is_none());
            }
            // CPython's type_clear may empty even an escaped type dictionary.
            // The surviving strict mapping must remain terminal, not keep
            // the collected class or its old contents alive.
            let error = escaped_namespace
                .set_item("after_collection", 1)
                .expect_err("a collected class namespace remains terminal");
            assert!(
                error
                    .get_type(py)
                    .is(&strict_runtime_unavailable(py, "terminal namespace").get_type(py))
            );
            assert_eq!(family.method_name(), "method");
            Ok(())
        })
    }

    #[test]
    fn copied_namespace_accepts_only_matching_native_cached_layout_descriptors() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let globals = PyDict::new(py);
            py.run(c"class Box: pass\n", Some(&globals), None)?;
            let class = globals.get_item("Box")?.unwrap();
            let class_pointer = class.as_ptr().cast::<ffi::PyTypeObject>();
            let namespace =
                unsafe { Bound::<PyAny>::from_borrowed_ptr(py, (*class_pointer).tp_dict) }
                    .cast_into::<PyDict>()?;
            let dictionary_descriptor = namespace.get_item("__dict__")?.unwrap();
            let weakref_descriptor = namespace.get_item("__weakref__")?.unwrap();
            // This CPython shares both descriptors between heap classes.
            // Their declared owner is object, not the newly constructed type.
            for descriptor in [&dictionary_descriptor, &weakref_descriptor] {
                assert_eq!(
                    unsafe { (*descriptor.as_ptr().cast::<ffi::PyDescrObject>()).d_type },
                    ptr::addr_of_mut!(ffi::PyBaseObject_Type)
                );
            }
            assert!(
                namespace_admission(&namespace, &fact(), &layout(&[]), class_pointer, None).is_ok()
            );
            // Supplying those descriptors in the input does not establish
            // native construction ownership, even when their kind is exact.
            assert_eq!(
                namespace_admission(&namespace, &fact(), &layout(&[]), ptr::null_mut(), None),
                Err(DynamicClassReason::UnsupportedDescriptor)
            );

            // Static builtin namespaces live in per-interpreter state, not
            // PyBaseObject_Type.tp_dict. The public accessor returns a new
            // reference, unlike the copied heap-type namespace borrow above.
            let object_namespace = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    PyType_GetDict(ptr::addr_of_mut!(ffi::PyBaseObject_Type)),
                )
            }?
            .cast_into::<PyDict>()?;
            let unrelated = object_namespace.get_item("__class__")?.unwrap();
            let substituted = PyDict::new(py);
            for descriptor in [&weakref_descriptor, &unrelated] {
                substituted.set_item("__dict__", descriptor)?;
                assert_eq!(
                    namespace_admission(&substituted, &fact(), &layout(&[]), class_pointer, None),
                    Err(DynamicClassReason::UnsupportedDescriptor)
                );
            }
            substituted.clear();
            substituted.set_item("__weakref__", dictionary_descriptor)?;
            assert_eq!(
                namespace_admission(&substituted, &fact(), &layout(&[]), class_pointer, None),
                Err(DynamicClassReason::UnsupportedDescriptor)
            );
            Ok(())
        })
    }

    #[test]
    fn actual_unpredicted_descriptors_and_hostile_keys_decline_without_callbacks() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let globals = PyDict::new(py);
            py.run(c"events = []\nclass Descriptor:\n def __get__(self, obj, owner): events.append('get'); return 1\nclass Key:\n def __hash__(self): events.append('hash'); return hash('late')\n def __eq__(self, other): events.append('eq'); return False\ndescriptor = Descriptor()\nhostile = {Key(): 1}\nevents.clear()\n", Some(&globals), None)?;
            let namespace = PyDict::new(py);
            namespace.set_item("late", globals.get_item("descriptor")?.unwrap())?;
            assert_eq!(
                namespace_admission(&namespace, &fact(), &layout(&[]), ptr::null_mut(), None),
                Err(DynamicClassReason::UnsupportedDescriptor)
            );
            let hostile = globals
                .get_item("hostile")?
                .unwrap()
                .cast_into::<PyDict>()?;
            assert_eq!(
                namespace_admission(&hostile, &fact(), &layout(&[]), ptr::null_mut(), None),
                Err(DynamicClassReason::UnresolvedAnalysis)
            );
            assert_eq!(globals.get_item("events")?.unwrap().len()?, 0);

            let mut implicit = fact();
            let mut hook = method(&implicit, "__init_subclass__", false);
            hook.binding = MethodBinding::Class;
            implicit.methods.push(hook);
            let input = PyDict::new(py);
            let function = py.eval(c"lambda cls: None", None, None)?;
            input.set_item("__init_subclass__", &function)?;
            assert!(
                namespace_admission(&input, &implicit, &layout(&[]), ptr::null_mut(), None).is_ok()
            );
            let wrapper = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(py, PyClassMethod_New(function.as_ptr()))
            }?;
            input.set_item("__init_subclass__", &wrapper)?;
            // Kind admission is not provenance. An explicit builtin wrapper
            // can have this kind, but this ordinary one has no source birth.
            assert!(
                namespace_admission(&input, &implicit, &layout(&[]), ptr::null_mut(), None).is_ok()
            );
            let execution =
                crate::strict_namespace::NamespaceExecution::completed_identity_for_test(
                    implicit.identity.clone(),
                    interpreter_id(),
                );
            assert!(!crate::strict_descriptor::matches_birth(
                py, &wrapper, &function, &execution
            )?);

            let unicode = PyString::new(py, "π_field");
            assert!(exact_name_matches(unicode.as_any(), "π_field"));
            assert!(!exact_name_matches(unicode.as_any(), "p_field"));
            let surrogate = py.eval(c"'\\udfff'", None, None)?;
            assert!(!exact_name_matches(&surrogate, "x"));
            assert!(unsafe { ffi::PyErr_Occurred() }.is_null());
            Ok(())
        })
    }

    #[test]
    fn native_storage_policy_checks_canonical_writes_and_preserves_overflow() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let mut layout = layout(&["checked", "optional"]);
            layout
                .checks
                .insert("checked".into(), vec![nominal(BuiltinType::Int)]);
            layout.checks.insert(
                "optional".into(),
                vec![StaticType::Union(vec![
                    nominal(BuiltinType::Str),
                    StaticType::None,
                ])],
            );
            let owner = storage_fixture(py, layout)?;
            let dictionary = new_policy_dictionary(&owner)?.cast_into::<PyDict>()?;
            assert!(dictionary.is_empty());
            dictionary.set_item("checked", true)?;
            let error = dictionary.set_item("checked", "wrong").unwrap_err();
            assert!(error.is_instance_of::<PyTypeError>(py));
            assert!(dictionary.get_item("checked")?.unwrap().is_truthy()?);
            dictionary.set_item("optional", py.None())?;
            dictionary.set_item("optional", "valid")?;
            assert!(
                dictionary
                    .set_item("optional", 4)
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            dictionary.set_item("method", "ignored instance dictionary collision")?;
            dictionary.set_item(7, "ordinary non-string overflow")?;

            let globals = PyDict::new(py);
            globals.set_item("d", &dictionary)?;
            py.run(c"events = []\nclass Alias:\n def __hash__(self): events.append('hash'); return hash('checked')\n def __eq__(self, other): events.append('eq'); return other == 'checked'\nkey = Alias()\n", Some(&globals), None)?;
            let key = globals.get_item("key")?.unwrap();
            assert!(
                dictionary
                    .set_item(&key, "wrong alias value")
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            assert_eq!(
                globals
                    .get_item("events")?
                    .unwrap()
                    .extract::<Vec<String>>()?,
                ["hash", "eq"]
            );
            py.run(c"hash_error = ValueError('hash failure')\neq_error = KeyError('equality failure')\nclass BadHash:\n def __hash__(self): raise hash_error\nclass BadEquality:\n def __hash__(self): return hash('checked')\n def __eq__(self, other): raise eq_error\nbad_hash = BadHash()\nbad_eq = BadEquality()\n", Some(&globals), None)?;
            for (key_name, error_name) in [("bad_hash", "hash_error"), ("bad_eq", "eq_error")] {
                let error = dictionary
                    .set_item(globals.get_item(key_name)?.unwrap(), "wrong value")
                    .unwrap_err();
                assert_eq!(
                    error.value(py).as_ptr(),
                    globals.get_item(error_name)?.unwrap().as_ptr(),
                    "native lookup failure must not be replaced by a field-policy error"
                );
            }
            assert!(dictionary.get_item("checked")?.unwrap().is_truthy()?);
            assert_eq!(dictionary.len(), 4);
            dictionary.del_item("checked")?;
            dictionary.set_item("checked", 8)?;
            assert_eq!(
                unsafe {
                    _PyDict_IndexedKeyIndex(
                        dictionary.as_ptr(),
                        PyString::new(py, "checked").as_ptr(),
                    )
                },
                0
            );
            dictionary.call_method0("clear")?;
            assert!(dictionary.is_empty());
            dictionary.set_item("checked", 9)?;
            Ok(())
        })
    }

    #[test]
    fn native_attribute_transaction_checks_unicode_payload_after_lookup() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let mut layout = layout(&["checked"]);
            layout
                .checks
                .insert("checked".into(), vec![nominal(BuiltinType::Int)]);
            let owner = storage_fixture(py, layout)?;
            let dictionary = new_policy_dictionary(&owner)?.cast_into::<PyDict>()?;
            let globals = PyDict::new(py);
            py.run(c"events = []\nclass Receiver: pass\nreceiver = Receiver()\nclass Name(str):\n def __hash__(self): events.append('hash'); return str.__hash__(self)\n def __eq__(self, other): events.append('eq'); return str.__eq__(self, other)\n def __str__(self): raise AssertionError('user name conversion')\nname = Name('checked')\nerror = ValueError('dictionary equality failed')\nclass BadEquality:\n def __hash__(self): return hash('checked')\n def __eq__(self, other): events.append('eq'); raise error\nkey = BadEquality()\n", Some(&globals), None)?;
            let receiver = globals.get_item("receiver")?.unwrap();
            let name = globals.get_item("name")?.unwrap();
            let wrong = PyString::new(py, "wrong value");
            let ordinary = PyDict::new(py);
            let write = |name: &Bound<'_, PyAny>,
                         value: &Bound<'_, PyAny>,
                         dictionary: &Bound<'_, PyDict>| {
                if unsafe {
                    _PyObject_GenericSetAttrWithDict(
                        receiver.as_ptr(),
                        name.as_ptr(),
                        value.as_ptr(),
                        dictionary.as_ptr(),
                    )
                } < 0
                {
                    Err(PyErr::fetch(py))
                } else {
                    Ok(())
                }
            };
            write(&name, wrong.as_any(), &ordinary)?;
            let expected = globals
                .get_item("events")?
                .unwrap()
                .extract::<Vec<String>>()?;
            globals.get_item("events")?.unwrap().call_method0("clear")?;
            assert!(
                write(&name, wrong.as_any(), &dictionary)
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            assert!(dictionary.is_empty());
            assert_eq!(
                globals
                    .get_item("events")?
                    .unwrap()
                    .extract::<Vec<String>>()?,
                expected
            );

            let valid = py.eval(c"7", None, None)?;
            write(&name, &valid, &dictionary)?;
            assert_eq!(dictionary.len(), 1);
            // Mapping writes preserve the str-subclass key and carry no
            // attribute provenance or checked-read capability. Its canonical
            // Unicode payload still selects the independent field-write check.
            let ordinary_mapping = PyDict::new(py);
            ordinary_mapping.set_item(&name, &valid)?;
            globals.get_item("events")?.unwrap().call_method0("clear")?;
            ordinary_mapping.set_item(&name, &wrong)?;
            let expected = globals
                .get_item("events")?
                .unwrap()
                .extract::<Vec<String>>()?;
            globals.get_item("events")?.unwrap().call_method0("clear")?;
            assert!(
                dictionary
                    .set_item(&name, &wrong)
                    .unwrap_err()
                    .is_instance_of::<PyTypeError>(py)
            );
            // Read events before any dictionary lookup can add callbacks.
            assert_eq!(
                globals
                    .get_item("events")?
                    .unwrap()
                    .extract::<Vec<String>>()?,
                expected
            );
            assert_eq!(dictionary.len(), 1);
            assert_eq!(
                dictionary.get_item(&name)?.unwrap().as_ptr(),
                valid.as_ptr()
            );
            assert_eq!(dictionary.iter().next().unwrap().0.as_ptr(), name.as_ptr());
            dictionary.set_item(&name, 8)?;
            assert_eq!(dictionary.get_item(&name)?.unwrap().extract::<i64>()?, 8);
            dictionary.clear();
            dictionary.set_item(globals.get_item("key")?.unwrap(), &valid)?;
            globals.get_item("events")?.unwrap().call_method0("clear")?;
            let error = write(
                PyString::new(py, "checked").as_any(),
                wrong.as_any(),
                &dictionary,
            )
            .unwrap_err();
            assert_eq!(
                error.value(py).as_ptr(),
                globals.get_item("error")?.unwrap().as_ptr()
            );
            assert_eq!(
                globals
                    .get_item("events")?
                    .unwrap()
                    .extract::<Vec<String>>()?,
                ["eq"]
            );
            assert_eq!(dictionary.len(), 1);

            // No ordinary SET or malformed native attribute operation may
            // smuggle a provenance object into this owner's checked path.
            for (operation, provenance) in [(SET, name.as_ptr()), (ATTRIBUTE_SET, ptr::null_mut())]
            {
                assert_eq!(
                    unsafe {
                        validate_instance_dictionary(
                            owner.owner().as_ptr(),
                            dictionary.as_ptr(),
                            name.as_ptr(),
                            valid.as_ptr(),
                            operation,
                            provenance,
                        )
                    },
                    -1
                );
                assert!(PyErr::fetch(py).is_instance_of::<PyTypeError>(py));
            }
            Ok(())
        })
    }

    #[test]
    fn one_instance_dictionary_teardown_does_not_terminalize_shared_storage_owner() -> PyResult<()>
    {
        let _lock = native_lock();
        Python::attach(|py| {
            let owner = storage_fixture(py, layout(&["field"]))?;
            let first = new_policy_dictionary(&owner)?.cast_into::<PyDict>()?;
            let second = new_policy_dictionary(&owner)?.cast_into::<PyDict>()?;
            let globals = PyDict::new(py);
            globals.set_item("first", &first)?;
            py.run(c"import weakref\nclass Marker: pass\nmarker = Marker()\nreference = weakref.ref(marker)\nfirst['marker'] = marker\nfirst['cycle'] = first\ndel marker\n", Some(&globals), None)?;
            globals.del_item("first")?;
            drop(first);
            py.import("gc")?.call_method0("collect")?;
            assert!(globals.get_item("reference")?.unwrap().call0()?.is_none());
            owner.ensure_live()?;
            second.set_item("field", "still live")?;
            assert_eq!(
                second.get_item("field")?.unwrap().extract::<String>()?,
                "still live"
            );
            let dictionary = new_policy_dictionary(&owner)?.into_ptr();
            let expected = py.eval(
                c"ValueError('pending during dictionary teardown')",
                None,
                None,
            )?;
            unsafe {
                ffi::PyErr_SetObject(ffi::PyExc_ValueError, expected.as_ptr());
                ffi::Py_DECREF(dictionary);
            }
            assert!(!unsafe { ffi::PyErr_Occurred() }.is_null());
            let error = PyErr::fetch(py);
            assert_eq!(error.value(py).as_ptr(), expected.as_ptr());
            owner.ensure_live()?;
            second.set_item("field", "live after exception-preserving teardown")?;
            Ok(())
        })
    }

    #[test]
    fn schema_clone_copies_no_visible_values_even_if_gc_exposes_template() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let owner = storage_fixture(py, layout(&["reserved"]))?;
            let template = owner
                .reference(owner.data().template.unwrap())?
                .cast_into::<PyDict>()?;
            template.set_item("reserved", "not an instance default")?;
            template.set_item("overflow", 12)?;
            let dictionary = new_policy_dictionary(&owner)?.cast_into::<PyDict>()?;
            assert!(dictionary.is_empty());
            assert_eq!(
                unsafe {
                    _PyDict_IndexedKeyIndex(
                        dictionary.as_ptr(),
                        PyString::new(py, "reserved").as_ptr(),
                    )
                },
                0
            );
            assert!(dictionary.get_item("overflow")?.is_none());
            dictionary.set_item("reserved", "independent value")?;
            assert_eq!(
                template
                    .get_item("reserved")?
                    .unwrap()
                    .extract::<String>()?,
                "not an instance default"
            );
            Ok(())
        })
    }

    /// Reuse the verified-source kernel fixture without exporting a constructor
    /// for an unchecked function/activation. The caller sees only the actual
    /// compiler-created callable and its matching immutable source module.
    pub(crate) fn with_strict_callable_fixture<'py>(
        py: Python<'py>,
        source: &str,
        suspended: bool,
        test: impl FnOnce(
            &Bound<'py, PyAny>,
            &Arc<crate::module_type::SharedModuleState>,
        ) -> PyResult<()>,
    ) -> PyResult<()> {
        with_strict_callable_fixture_functions(
            py,
            source,
            &[("f", &["callback", "value"], suspended)],
            test,
        )
    }

    /// Multiple original definitions use their actual source ranges and public
    /// parameters; the first is the source activation observed by the caller.
    pub(crate) fn with_strict_callable_fixture_functions<'py>(
        py: Python<'py>,
        source: &str,
        definitions: &[(&str, &[&str], bool)],
        test: impl FnOnce(
            &Bound<'py, PyAny>,
            &Arc<crate::module_type::SharedModuleState>,
        ) -> PyResult<()>,
    ) -> PyResult<()> {
        let mut facts = ModuleTypeFacts::new(
            "strict_callable_fixture",
            source.as_bytes(),
            SourceDialect::SoacStrict,
            ResolvedStrictPolicy {
                strict_assign: true,
                checked_attr: true,
                ..Default::default()
            },
        )
        .map_err(fixture_error)?;
        for &(name, parameters, suspended) in definitions {
            let marker = format!("def {name}(");
            let start = source
                .find(&marker)
                .expect("fixture defines the original callable");
            let end = source[start..]
                .find("\ndef ")
                .map_or(source.len(), |next| start + next);
            let end = source[..end].trim_end().len();
            facts.functions.push(FunctionTypeFact {
                identity: SourceIdentity {
                    module: facts.module.clone(),
                    lexical_qualname: name.into(),
                    source_range: SourceRange::new(
                        u32::try_from(start).unwrap(),
                        u32::try_from(end).unwrap(),
                    ),
                    definition_kind: DefinitionKind::Function,
                },
                function_kind: if suspended {
                    soac_contracts::FunctionKind::Generator
                } else {
                    soac_contracts::FunctionKind::Synchronous
                },
                signature: CallableSignature {
                    parameters: parameters
                        .iter()
                        .copied()
                        .map(|name| ParameterTypeFact {
                            name: name.into(),
                            kind: ParameterKind::PositionalOrKeyword,
                            value_type: StaticType::Any,
                            annotation_origin: AnnotationOrigin::Absent,
                            default: DefaultFact::Missing,
                        })
                        .collect(),
                    return_type: StaticType::Unknown,
                    return_annotation_origin: AnnotationOrigin::Absent,
                    uncertainty: BTreeSet::new(),
                },
                decorators: vec![],
                uncertainty: BTreeSet::new(),
            });
        }
        let &(name, _, suspended) = definitions
            .first()
            .expect("fixture has a source activation");
        let fixture = FieldCapabilityFixture::from_facts(py, source.as_bytes(), facts)?;
        let shared = fixture.run_body()?;
        fixture.finalize(&shared)?;
        let function = fixture.module.getattr(name)?;
        let authenticated = authenticate_strict_function(py, &function)?
            .expect("the actual MakeFunction must have installed its source owner");
        let lowered = shared
            .lowered_module
            .callable_defs
            .iter()
            .find(|candidate| candidate.names.qualname == name)
            .expect("the native catalogue and lowered source both contain the callable");
        assert_eq!(authenticated.function_id()?, lowered.function_id);
        assert_eq!(
            lowered.kind != soac_core::block_py::FunctionKind::Function,
            suspended,
        );
        test(&function, &shared)
    }
}
