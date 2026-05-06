use soac_core::block_py::{BlockLabel, InstrKey, LocalLocation, RuntimeFunctionId};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum TruthinessFact {
    Unknown,
    AlwaysTrue,
    AlwaysFalse,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PyExactType {
    NoneType,
    Bool,
    Str,
    Bytes,
    Int,
    Float,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum TypeFact {
    Unknown,
    Exact(PyExactType),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RuntimeSingleton {
    None,
    True,
    False,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum NoneFact {
    Unknown,
    IsNone,
    IsNotNone,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PyObjectNullabilityFact {
    Unknown,
    NonNull,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum BoolSingletonFact {
    Unknown,
    IsTrue,
    IsFalse,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RefcountFact {
    Unknown,
    Immortal,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ProvenanceFact {
    Unknown,
    RuntimeSingleton(RuntimeSingleton),
    ModuleConstant(u32),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RuntimeHelperId {
    Globals,
    Index,
    Str,
    CellRef,
    TupleFromIter,
    MakeFunction,
    CreateClass,
    Import,
    ImportAttr,
    ClassLookupGlobal,
    ClassLookupCell,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ThrowSpec {
    Never,
    ThrowsOnNullPyObj,
    ThrowsOnI32Sentinel(i32),
    ThrowsOnI64Sentinel(i64),
    ThrowsOnNonZeroI32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RuntimeHelperSignature {
    pub helper: RuntimeHelperId,
    pub result: ValueFacts,
    pub throws: ThrowSpec,
}

impl RuntimeHelperId {
    pub fn from_runtime_symbol(name: &str) -> Option<Self> {
        match name.as_bytes() {
            b"globals" => Some(Self::Globals),
            b"_index" => Some(Self::Index),
            b"str" => Some(Self::Str),
            b"cell_ref" => Some(Self::CellRef),
            b"tuple_from_iter" => Some(Self::TupleFromIter),
            b"make_function" => Some(Self::MakeFunction),
            b"create_class" => Some(Self::CreateClass),
            b"import_" => Some(Self::Import),
            b"import_attr" => Some(Self::ImportAttr),
            b"class_lookup_global" => Some(Self::ClassLookupGlobal),
            b"class_lookup_cell" => Some(Self::ClassLookupCell),
            _ => None,
        }
    }

    pub const fn signature(self) -> RuntimeHelperSignature {
        RuntimeHelperSignature {
            helper: self,
            result: runtime_helper_result(self),
            throws: runtime_helper_throw_spec(self),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CallableFact {
    Unknown,
    RuntimeHelper(RuntimeHelperId),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PyObjFacts {
    pub ty: TypeFact,
    pub truthiness: TruthinessFact,
    pub none: NoneFact,
    pub nullability: PyObjectNullabilityFact,
    pub bool_singleton: BoolSingletonFact,
    pub refcount: RefcountFact,
    pub provenance: ProvenanceFact,
    pub callable: CallableFact,
}

impl PyObjFacts {
    pub const fn unknown() -> Self {
        Self {
            ty: TypeFact::Unknown,
            truthiness: TruthinessFact::Unknown,
            none: NoneFact::Unknown,
            nullability: PyObjectNullabilityFact::Unknown,
            bool_singleton: BoolSingletonFact::Unknown,
            refcount: RefcountFact::Unknown,
            provenance: ProvenanceFact::Unknown,
            callable: CallableFact::Unknown,
        }
    }

    pub const fn none_singleton() -> Self {
        Self {
            ty: TypeFact::Exact(PyExactType::NoneType),
            truthiness: TruthinessFact::AlwaysFalse,
            none: NoneFact::IsNone,
            nullability: PyObjectNullabilityFact::NonNull,
            bool_singleton: BoolSingletonFact::Unknown,
            refcount: RefcountFact::Immortal,
            provenance: ProvenanceFact::RuntimeSingleton(RuntimeSingleton::None),
            callable: CallableFact::Unknown,
        }
    }

    pub const fn bool_singleton(value: bool) -> Self {
        Self {
            ty: TypeFact::Exact(PyExactType::Bool),
            truthiness: if value {
                TruthinessFact::AlwaysTrue
            } else {
                TruthinessFact::AlwaysFalse
            },
            none: NoneFact::IsNotNone,
            nullability: PyObjectNullabilityFact::NonNull,
            bool_singleton: if value {
                BoolSingletonFact::IsTrue
            } else {
                BoolSingletonFact::IsFalse
            },
            refcount: RefcountFact::Immortal,
            provenance: ProvenanceFact::RuntimeSingleton(if value {
                RuntimeSingleton::True
            } else {
                RuntimeSingleton::False
            }),
            callable: CallableFact::Unknown,
        }
    }

    pub const fn bool_object() -> Self {
        Self {
            ty: TypeFact::Exact(PyExactType::Bool),
            truthiness: TruthinessFact::Unknown,
            none: NoneFact::IsNotNone,
            nullability: PyObjectNullabilityFact::NonNull,
            bool_singleton: BoolSingletonFact::Unknown,
            refcount: RefcountFact::Immortal,
            provenance: ProvenanceFact::Unknown,
            callable: CallableFact::Unknown,
        }
    }

    pub const fn exact_type(exact_type: PyExactType) -> Self {
        Self {
            ty: TypeFact::Exact(exact_type),
            truthiness: TruthinessFact::Unknown,
            none: none_fact_for_exact_type(exact_type),
            nullability: PyObjectNullabilityFact::NonNull,
            bool_singleton: BoolSingletonFact::Unknown,
            refcount: RefcountFact::Unknown,
            provenance: ProvenanceFact::Unknown,
            callable: CallableFact::Unknown,
        }
    }

    pub const fn exact_type_with_truthiness(
        exact_type: PyExactType,
        truthiness: TruthinessFact,
    ) -> Self {
        Self {
            ty: TypeFact::Exact(exact_type),
            truthiness,
            none: none_fact_for_exact_type(exact_type),
            nullability: PyObjectNullabilityFact::NonNull,
            bool_singleton: BoolSingletonFact::Unknown,
            refcount: RefcountFact::Unknown,
            provenance: ProvenanceFact::Unknown,
            callable: CallableFact::Unknown,
        }
    }

    pub const fn module_constant(index: u32) -> Self {
        Self {
            ty: TypeFact::Unknown,
            truthiness: TruthinessFact::Unknown,
            none: NoneFact::Unknown,
            nullability: PyObjectNullabilityFact::Unknown,
            bool_singleton: BoolSingletonFact::Unknown,
            refcount: RefcountFact::Unknown,
            provenance: ProvenanceFact::ModuleConstant(index),
            callable: CallableFact::Unknown,
        }
    }

    pub const fn with_module_constant(mut self, index: u32) -> Self {
        self.provenance = ProvenanceFact::ModuleConstant(index);
        self
    }

    pub const fn with_immortal_refcount(mut self) -> Self {
        self.refcount = RefcountFact::Immortal;
        self
    }

    pub const fn runtime_helper(helper: RuntimeHelperId) -> Self {
        Self {
            ty: TypeFact::Unknown,
            truthiness: TruthinessFact::AlwaysTrue,
            none: NoneFact::IsNotNone,
            nullability: PyObjectNullabilityFact::NonNull,
            bool_singleton: BoolSingletonFact::Unknown,
            refcount: RefcountFact::Unknown,
            provenance: ProvenanceFact::Unknown,
            callable: CallableFact::RuntimeHelper(helper),
        }
    }

    pub const fn known_not_none() -> Self {
        Self {
            ty: TypeFact::Unknown,
            truthiness: TruthinessFact::Unknown,
            none: NoneFact::IsNotNone,
            nullability: PyObjectNullabilityFact::NonNull,
            bool_singleton: BoolSingletonFact::Unknown,
            refcount: RefcountFact::Unknown,
            provenance: ProvenanceFact::Unknown,
            callable: CallableFact::Unknown,
        }
    }

    pub const fn is_none(self) -> bool {
        matches!(self.none, NoneFact::IsNone)
    }

    pub const fn is_known_not_none(self) -> bool {
        matches!(self.none, NoneFact::IsNotNone)
    }

    pub const fn with_non_null_ref(mut self) -> Self {
        self.nullability = PyObjectNullabilityFact::NonNull;
        self
    }

    pub const fn is_non_null_ref(self) -> bool {
        matches!(self.nullability, PyObjectNullabilityFact::NonNull)
    }

    pub const fn is_truthy(self) -> Option<bool> {
        match self.truthiness {
            TruthinessFact::AlwaysTrue => Some(true),
            TruthinessFact::AlwaysFalse => Some(false),
            TruthinessFact::Unknown => None,
        }
    }

    pub const fn is_immortal(self) -> bool {
        matches!(self.refcount, RefcountFact::Immortal)
    }

    pub const fn is_exact_type(self, expected: PyExactType) -> bool {
        match self.ty {
            TypeFact::Exact(actual) => actual as u8 == expected as u8,
            TypeFact::Unknown => false,
        }
    }

    pub const fn is_true_singleton(self) -> bool {
        matches!(self.bool_singleton, BoolSingletonFact::IsTrue)
    }

    pub const fn is_false_singleton(self) -> bool {
        matches!(self.bool_singleton, BoolSingletonFact::IsFalse)
    }

    fn is_uninformative_for_local_env(self) -> bool {
        self.ty == TypeFact::Unknown
            && self.truthiness == TruthinessFact::Unknown
            && self.none == NoneFact::Unknown
            && self.nullability == PyObjectNullabilityFact::Unknown
            && self.bool_singleton == BoolSingletonFact::Unknown
            && self.refcount == RefcountFact::Unknown
            && self.callable == CallableFact::Unknown
    }
}

const fn none_fact_for_exact_type(exact_type: PyExactType) -> NoneFact {
    match exact_type {
        PyExactType::NoneType => NoneFact::IsNone,
        PyExactType::Bool
        | PyExactType::Str
        | PyExactType::Bytes
        | PyExactType::Int
        | PyExactType::Float => NoneFact::IsNotNone,
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct I32Facts {
    pub sentinel: Option<i32>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct I64Facts {
    pub sentinel: Option<i64>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BoolFacts;

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ValueFacts {
    PyObj(PyObjFacts),
    I32(I32Facts),
    I64(I64Facts),
    Bool(BoolFacts),
}

impl ValueFacts {
    pub const fn unknown_pyobj() -> Self {
        Self::PyObj(PyObjFacts::unknown())
    }

    pub const fn as_pyobj(self) -> Option<PyObjFacts> {
        match self {
            Self::PyObj(py_facts) => Some(py_facts),
            Self::I32(_) | Self::I64(_) | Self::Bool(_) => None,
        }
    }

    pub const fn runtime_helper(self) -> Option<RuntimeHelperId> {
        match self {
            Self::PyObj(PyObjFacts {
                callable: CallableFact::RuntimeHelper(helper),
                ..
            }) => Some(helper),
            Self::PyObj(_) | Self::I32(_) | Self::I64(_) | Self::Bool(_) => None,
        }
    }
}

const fn runtime_helper_result(helper: RuntimeHelperId) -> ValueFacts {
    match helper {
        RuntimeHelperId::Index => ValueFacts::PyObj(PyObjFacts::exact_type(PyExactType::Int)),
        RuntimeHelperId::Str => ValueFacts::PyObj(PyObjFacts::exact_type(PyExactType::Str)),
        RuntimeHelperId::Globals
        | RuntimeHelperId::CellRef
        | RuntimeHelperId::TupleFromIter
        | RuntimeHelperId::MakeFunction
        | RuntimeHelperId::CreateClass
        | RuntimeHelperId::Import
        | RuntimeHelperId::ImportAttr
        | RuntimeHelperId::ClassLookupGlobal
        | RuntimeHelperId::ClassLookupCell => ValueFacts::PyObj(PyObjFacts::known_not_none()),
    }
}

const fn runtime_helper_throw_spec(helper: RuntimeHelperId) -> ThrowSpec {
    match helper {
        RuntimeHelperId::Globals | RuntimeHelperId::CellRef => ThrowSpec::Never,
        RuntimeHelperId::Index
        | RuntimeHelperId::Str
        | RuntimeHelperId::TupleFromIter
        | RuntimeHelperId::MakeFunction
        | RuntimeHelperId::CreateClass
        | RuntimeHelperId::Import
        | RuntimeHelperId::ImportAttr
        | RuntimeHelperId::ClassLookupGlobal
        | RuntimeHelperId::ClassLookupCell => ThrowSpec::ThrowsOnNullPyObj,
    }
}

#[derive(
    Debug, Clone, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct EnvFacts {
    local_pyobj_facts: HashMap<LocalLocation, PyObjFacts>,
}

impl EnvFacts {
    pub fn local_pyobj_fact(&self, location: LocalLocation) -> Option<PyObjFacts> {
        self.local_pyobj_facts.get(&location).copied()
    }

    pub fn local_pyobj_facts(&self) -> impl Iterator<Item = (LocalLocation, PyObjFacts)> + '_ {
        self.local_pyobj_facts
            .iter()
            .map(|(location, facts)| (*location, *facts))
    }

    pub fn set_local_pyobj_fact(&mut self, location: LocalLocation, facts: PyObjFacts) {
        if facts.is_uninformative_for_local_env() {
            self.local_pyobj_facts.remove(&location);
        } else {
            self.local_pyobj_facts.insert(location, facts);
        }
    }

    pub fn remove_local_pyobj_fact(&mut self, location: LocalLocation) {
        self.local_pyobj_facts.remove(&location);
    }

    pub fn intersect_with(&mut self, other: &Self) {
        self.local_pyobj_facts
            .retain(|location, facts| other.local_pyobj_fact(*location) == Some(*facts));
    }
}

#[derive(
    Debug, Clone, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct FactStore {
    expr_facts: HashMap<InstrKey, ValueFacts>,
    block_entry_facts: HashMap<(RuntimeFunctionId, BlockLabel), EnvFacts>,
}

impl FactStore {
    pub fn fact_for(&self, key: InstrKey) -> Option<ValueFacts> {
        self.expr_facts.get(&key).copied()
    }

    pub fn block_entry_fact(
        &self,
        function_id: RuntimeFunctionId,
        label: BlockLabel,
    ) -> Option<&EnvFacts> {
        self.block_entry_facts.get(&(function_id, label))
    }

    pub fn expr_facts(&self) -> impl Iterator<Item = (InstrKey, ValueFacts)> + '_ {
        self.expr_facts.iter().map(|(key, facts)| (*key, *facts))
    }

    pub fn block_entry_facts(
        &self,
    ) -> impl Iterator<Item = ((RuntimeFunctionId, BlockLabel), &EnvFacts)> {
        self.block_entry_facts
            .iter()
            .map(|(key, facts)| (*key, facts))
    }

    pub fn insert_expr_fact(&mut self, key: InstrKey, facts: ValueFacts) {
        self.expr_facts.insert(key, facts);
    }

    pub fn insert_block_entry_fact(
        &mut self,
        function_id: RuntimeFunctionId,
        label: BlockLabel,
        facts: EnvFacts,
    ) {
        self.block_entry_facts.insert((function_id, label), facts);
    }

    pub fn extend_expr_facts(&mut self, facts: impl IntoIterator<Item = (InstrKey, ValueFacts)>) {
        self.expr_facts.extend(facts);
    }

    pub fn extend_block_entry_facts(
        &mut self,
        facts: impl IntoIterator<Item = ((RuntimeFunctionId, BlockLabel), EnvFacts)>,
    ) {
        self.block_entry_facts.extend(facts);
    }

    pub fn remap_function_ids(
        &mut self,
        remap: impl Fn(RuntimeFunctionId) -> RuntimeFunctionId + Copy,
    ) {
        self.expr_facts = std::mem::take(&mut self.expr_facts)
            .into_iter()
            .map(|(mut key, facts)| {
                key.function_id = remap(key.function_id);
                (key, facts)
            })
            .collect();
        self.block_entry_facts = std::mem::take(&mut self.block_entry_facts)
            .into_iter()
            .map(|((function_id, label), facts)| ((remap(function_id), label), facts))
            .collect();
    }
}
