use crate::passes::{CodegenModuleShape, InstrCodegen, InstrResolved};
use soac_core::block_py::literal::{Literal, NumberLiteralValue};
use soac_core::block_py::{
    BinOpKind, Block, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, ChildVisitable,
    HasSemanticInstrId, InstrKey, LocalLocation, NameLike, RuntimeFunctionId, UnaryOpKind, Visit,
};
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
    NextOrSentinel,
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
            b"next_or_sentinel" => Some(Self::NextOrSentinel),
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
        | RuntimeHelperId::NextOrSentinel
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
        | RuntimeHelperId::NextOrSentinel
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

    fn set_local_pyobj_fact(&mut self, location: LocalLocation, facts: PyObjFacts) {
        if facts.is_uninformative_for_local_env() {
            self.local_pyobj_facts.remove(&location);
        } else {
            self.local_pyobj_facts.insert(location, facts);
        }
    }

    fn remove_local_pyobj_fact(&mut self, location: LocalLocation) {
        self.local_pyobj_facts.remove(&location);
    }

    fn intersect_with(&mut self, other: &Self) {
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

struct FunctionFactInferer<'a> {
    function: &'a BlockPyFunction<CodegenModuleShape>,
    module_constant_facts: &'a [ValueFacts],
    store: FactStore,
}

impl FunctionFactInferer<'_> {
    fn infer_expr_facts(&self, expr: &InstrCodegen) -> ValueFacts {
        match expr {
            InstrCodegen::Load(op) => {
                infer_runtime_name_load_facts(&op.name).unwrap_or_else(|| {
                    op.name
                        .location
                        .as_constant()
                        .map(|index| module_constant_load_fact(index, self.module_constant_facts))
                        .unwrap_or_else(ValueFacts::unknown_pyobj)
                })
            }
            InstrCodegen::Call(op) => {
                if op.keywords.is_empty()
                    && op.args.iter().all(|arg| {
                        matches!(arg, soac_core::block_py::CallArgPositional::Positional(_))
                    })
                {
                    self.infer_expr_facts(op.func.as_ref())
                        .runtime_helper()
                        .map(|helper| helper.signature().result)
                        .unwrap_or_else(ValueFacts::unknown_pyobj)
                } else {
                    ValueFacts::unknown_pyobj()
                }
            }
            InstrCodegen::BinOp(op) => infer_binop_result_facts(
                op.kind,
                self.infer_expr_facts(&op.left),
                self.infer_expr_facts(&op.right),
            )
            .unwrap_or_else(ValueFacts::unknown_pyobj),
            InstrCodegen::UnaryOp(op) => {
                infer_unary_result_facts(op.kind, self.infer_expr_facts(&op.operand))
                    .unwrap_or_else(ValueFacts::unknown_pyobj)
            }
            InstrCodegen::SetAttr(_)
            | InstrCodegen::SetItem(_)
            | InstrCodegen::DelItem(_)
            | InstrCodegen::Del(_) => ValueFacts::PyObj(PyObjFacts::none_singleton()),
            InstrCodegen::Tuple(_) => ValueFacts::PyObj(PyObjFacts::known_not_none()),
            _ => ValueFacts::unknown_pyobj(),
        }
    }

    fn infer_expr_facts_in_env(&self, expr: &InstrCodegen, env: &EnvFacts) -> ValueFacts {
        match expr {
            InstrCodegen::Load(op) => op
                .name
                .local_location()
                .and_then(|location| env.local_pyobj_fact(location))
                .map(ValueFacts::PyObj)
                .unwrap_or_else(|| self.infer_expr_facts(expr)),
            InstrCodegen::BinOp(op) => infer_binop_result_facts(
                op.kind,
                self.infer_expr_facts_in_env(&op.left, env),
                self.infer_expr_facts_in_env(&op.right, env),
            )
            .unwrap_or_else(ValueFacts::unknown_pyobj),
            InstrCodegen::UnaryOp(op) => {
                infer_unary_result_facts(op.kind, self.infer_expr_facts_in_env(&op.operand, env))
                    .unwrap_or_else(ValueFacts::unknown_pyobj)
            }
            _ => self.infer_expr_facts(expr),
        }
    }

    fn transfer_block_env(&self, block: &Block<InstrCodegen>, entry: &EnvFacts) -> EnvFacts {
        let mut env = entry.clone();
        for instr in &block.body {
            self.transfer_instr_env(instr, &mut env);
        }
        env
    }

    fn transfer_instr_env(&self, instr: &InstrCodegen, env: &mut EnvFacts) {
        match instr {
            InstrCodegen::Store(op) => {
                let Some(location) = op.name.local_location() else {
                    return;
                };
                match self.infer_expr_facts_in_env(&op.value, env).as_pyobj() {
                    Some(py_facts) => env.set_local_pyobj_fact(location, py_facts),
                    None => env.remove_local_pyobj_fact(location),
                }
            }
            InstrCodegen::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    env.remove_local_pyobj_fact(location);
                }
            }
            _ => {}
        }
    }

    fn successor_envs(
        &self,
        block: &Block<InstrCodegen>,
        exit: &EnvFacts,
    ) -> Vec<(BlockLabel, EnvFacts)> {
        match &block.term {
            BlockTerm::Jump(edge) => vec![(edge.target, exit.clone())],
            BlockTerm::IfTerm(if_term) => {
                let (then_facts, else_facts) = self.infer_if_edge_facts(if_term, exit);
                vec![
                    (if_term.then_label, then_facts),
                    (if_term.else_label, else_facts),
                ]
            }
            BlockTerm::BranchTable(branch) => {
                let mut out = branch
                    .targets
                    .iter()
                    .copied()
                    .map(|target| (target, exit.clone()))
                    .collect::<Vec<_>>();
                out.push((branch.default_label, exit.clone()));
                out
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => Vec::new(),
        }
    }

    fn infer_if_edge_facts(
        &self,
        if_term: &soac_core::block_py::TermIf<InstrCodegen>,
        exit: &EnvFacts,
    ) -> (EnvFacts, EnvFacts) {
        let Some((location, then_fact, else_fact)) = self.infer_branch_local_fact(&if_term.test)
        else {
            return (exit.clone(), exit.clone());
        };
        let mut then_facts = exit.clone();
        let mut else_facts = exit.clone();
        if let Some(fact) = then_fact {
            then_facts.set_local_pyobj_fact(location, fact);
        }
        if let Some(fact) = else_fact {
            else_facts.set_local_pyobj_fact(location, fact);
        }
        (then_facts, else_facts)
    }

    fn infer_block_entry_facts(&self) -> HashMap<BlockLabel, EnvFacts> {
        let Some(entry_block) = self.function.blocks.first() else {
            return HashMap::new();
        };
        let mut entries = HashMap::from([(entry_block.label, EnvFacts::default())]);
        let mut changed = true;
        while changed {
            changed = false;
            for block in &self.function.blocks {
                let Some(entry) = entries.get(&block.label).cloned() else {
                    continue;
                };
                let exit = self.transfer_block_env(block, &entry);
                for (target, incoming) in self.successor_envs(block, &exit) {
                    match entries.get_mut(&target) {
                        Some(existing) => {
                            let before = existing.clone();
                            existing.intersect_with(&incoming);
                            changed |= *existing != before;
                        }
                        None => {
                            entries.insert(target, incoming);
                            changed = true;
                        }
                    }
                }
            }
        }
        entries
    }

    fn infer_branch_local_fact(
        &self,
        test: &InstrCodegen,
    ) -> Option<(LocalLocation, Option<PyObjFacts>, Option<PyObjFacts>)> {
        match test {
            InstrCodegen::BinOp(op) if op.kind == BinOpKind::Is => {
                infer_local_is_singleton_comparison(&op.left, &op.right, self)
            }
            InstrCodegen::UnaryOp(op) if op.kind == UnaryOpKind::Not => self
                .infer_branch_local_fact(&op.operand)
                .map(|(location, then_fact, else_fact)| (location, else_fact, then_fact)),
            _ => None,
        }
    }
}

impl Visit<InstrCodegen> for FunctionFactInferer<'_> {
    fn visit_instr(&mut self, expr: &InstrCodegen)
    where
        InstrCodegen: ChildVisitable<InstrCodegen>,
    {
        // Synthetic trace/counter instrumentation is inserted after semantic ID
        // assignment. It should not receive fake expression facts of its own.
        if let Some(instr_id) = expr.try_semantic_instr_id() {
            let key = InstrKey::new(self.function.function_id, instr_id);
            let facts = self.infer_expr_facts(expr);
            self.store.expr_facts.insert(key, facts);
        }
        soac_core::block_py::walk_expr(self, expr);
    }
}

fn infer_function_value_facts(
    function: &BlockPyFunction<CodegenModuleShape>,
    module_constant_facts: &[ValueFacts],
) -> FactStore {
    let mut inferer = FunctionFactInferer {
        function,
        module_constant_facts,
        store: FactStore::default(),
    };
    for block in &function.blocks {
        inferer.visit_block(block);
    }
    let block_entry_facts = inferer.infer_block_entry_facts();
    for block in &function.blocks {
        inferer.store.block_entry_facts.insert(
            (function.function_id, block.label),
            block_entry_facts
                .get(&block.label)
                .cloned()
                .unwrap_or_default(),
        );
    }
    inferer.store
}

fn infer_local_is_singleton_comparison(
    left: &InstrCodegen,
    right: &InstrCodegen,
    inferer: &FunctionFactInferer<'_>,
) -> Option<(LocalLocation, Option<PyObjFacts>, Option<PyObjFacts>)> {
    if let Some((then_fact, else_fact)) = expr_singleton_branch_facts(right, inferer) {
        local_load_location(left).map(|location| (location, then_fact, else_fact))
    } else if let Some((then_fact, else_fact)) = expr_singleton_branch_facts(left, inferer) {
        local_load_location(right).map(|location| (location, then_fact, else_fact))
    } else {
        None
    }
}

fn expr_singleton_branch_facts(
    expr: &InstrCodegen,
    inferer: &FunctionFactInferer<'_>,
) -> Option<(Option<PyObjFacts>, Option<PyObjFacts>)> {
    match inferer.infer_expr_facts(expr) {
        ValueFacts::PyObj(py_facts) if py_facts.is_none() => Some((
            Some(PyObjFacts::none_singleton()),
            Some(PyObjFacts::known_not_none()),
        )),
        ValueFacts::PyObj(py_facts) if py_facts.is_true_singleton() => {
            Some((Some(PyObjFacts::bool_singleton(true)), None))
        }
        ValueFacts::PyObj(py_facts) if py_facts.is_false_singleton() => {
            Some((Some(PyObjFacts::bool_singleton(false)), None))
        }
        ValueFacts::PyObj(_) | ValueFacts::I32(_) | ValueFacts::I64(_) | ValueFacts::Bool(_) => {
            None
        }
    }
}

fn local_load_location(expr: &InstrCodegen) -> Option<LocalLocation> {
    match expr {
        InstrCodegen::Load(op) => op.name.local_location(),
        _ => None,
    }
}

fn is_exact_int_fact(facts: ValueFacts) -> bool {
    facts
        .as_pyobj()
        .is_some_and(|py_facts| py_facts.is_exact_type(PyExactType::Int))
}

pub(crate) fn infer_binop_result_facts(
    kind: BinOpKind,
    left: ValueFacts,
    right: ValueFacts,
) -> Option<ValueFacts> {
    if !(is_exact_int_fact(left) && is_exact_int_fact(right)) {
        return None;
    }
    let py_facts = match kind {
        BinOpKind::Eq
        | BinOpKind::Ne
        | BinOpKind::Lt
        | BinOpKind::Le
        | BinOpKind::Gt
        | BinOpKind::Ge => PyObjFacts::bool_object(),
        BinOpKind::Add
        | BinOpKind::Sub
        | BinOpKind::Mul
        | BinOpKind::FloorDiv
        | BinOpKind::Mod
        | BinOpKind::LShift
        | BinOpKind::RShift
        | BinOpKind::Or
        | BinOpKind::Xor
        | BinOpKind::And
        | BinOpKind::InplaceAdd
        | BinOpKind::InplaceSub
        | BinOpKind::InplaceMul
        | BinOpKind::InplaceFloorDiv
        | BinOpKind::InplaceMod
        | BinOpKind::InplaceLShift
        | BinOpKind::InplaceRShift
        | BinOpKind::InplaceOr
        | BinOpKind::InplaceXor
        | BinOpKind::InplaceAnd => PyObjFacts::exact_type(PyExactType::Int),
        BinOpKind::TrueDiv | BinOpKind::InplaceTrueDiv => {
            PyObjFacts::exact_type(PyExactType::Float)
        }
        BinOpKind::Pow
        | BinOpKind::InplacePow
        | BinOpKind::MatMul
        | BinOpKind::InplaceMatMul
        | BinOpKind::Contains
        | BinOpKind::Is => return None,
    };
    Some(ValueFacts::PyObj(py_facts))
}

pub(crate) fn infer_unary_result_facts(
    kind: UnaryOpKind,
    operand: ValueFacts,
) -> Option<ValueFacts> {
    if !is_exact_int_fact(operand) {
        return None;
    }
    let py_facts = match kind {
        UnaryOpKind::Pos | UnaryOpKind::Neg | UnaryOpKind::Invert => {
            PyObjFacts::exact_type(PyExactType::Int)
        }
        UnaryOpKind::Not | UnaryOpKind::Truth => PyObjFacts::bool_object(),
    };
    Some(ValueFacts::PyObj(py_facts))
}

fn infer_runtime_name_load_facts(name: &impl NameLike) -> Option<ValueFacts> {
    if name.is_runtime_symbol("NONE") {
        Some(ValueFacts::PyObj(PyObjFacts::none_singleton()))
    } else if name.is_runtime_symbol("TRUE") {
        Some(ValueFacts::PyObj(PyObjFacts::bool_singleton(true)))
    } else if name.is_runtime_symbol("FALSE") {
        Some(ValueFacts::PyObj(PyObjFacts::bool_singleton(false)))
    } else if name.is_runtime_name() {
        RuntimeHelperId::from_runtime_symbol(name.id_str())
            .map(PyObjFacts::runtime_helper)
            .map(ValueFacts::PyObj)
    } else {
        None
    }
}

fn module_constant_load_fact(index: u32, module_constant_facts: &[ValueFacts]) -> ValueFacts {
    module_constant_facts
        .get(index as usize)
        .copied()
        .map(|facts| match facts {
            ValueFacts::PyObj(py_facts) => ValueFacts::PyObj(
                py_facts
                    .with_module_constant(index)
                    .with_immortal_refcount(),
            ),
            ValueFacts::I32(_) | ValueFacts::I64(_) | ValueFacts::Bool(_) => facts,
        })
        .unwrap_or_else(|| {
            ValueFacts::PyObj(PyObjFacts::module_constant(index).with_immortal_refcount())
        })
}

fn infer_module_constant_facts(expr: &InstrResolved) -> ValueFacts {
    match expr {
        InstrResolved::Load(op) => {
            infer_runtime_name_load_facts(&op.name).unwrap_or_else(ValueFacts::unknown_pyobj)
        }
        InstrResolved::Literal(op) => infer_literal_facts(op.as_literal()),
        _ => ValueFacts::unknown_pyobj(),
    }
}

fn infer_literal_facts(literal: &Literal) -> ValueFacts {
    let py_facts = match literal {
        Literal::StringLiteral(value) => PyObjFacts::exact_type_with_truthiness(
            PyExactType::Str,
            truthiness(!value.value.is_empty()),
        ),
        Literal::BytesLiteral(value) => PyObjFacts::exact_type_with_truthiness(
            PyExactType::Bytes,
            truthiness(!value.value.is_empty()),
        ),
        Literal::NumberLiteral(number) => match &number.value {
            NumberLiteralValue::Int(value) => PyObjFacts::exact_type_with_truthiness(
                PyExactType::Int,
                truthiness(value.as_i64().is_none_or(|value| value != 0)),
            ),
            NumberLiteralValue::Float(value) => PyObjFacts::exact_type_with_truthiness(
                PyExactType::Float,
                truthiness(*value != 0.0),
            ),
        },
    };
    ValueFacts::PyObj(py_facts)
}

const fn truthiness(is_truthy: bool) -> TruthinessFact {
    if is_truthy {
        TruthinessFact::AlwaysTrue
    } else {
        TruthinessFact::AlwaysFalse
    }
}

pub fn infer_module_value_facts(module: &BlockPyModule<CodegenModuleShape>) -> FactStore {
    let mut store = FactStore::default();
    let module_constant_facts = module
        .module_constants
        .iter()
        .map(infer_module_constant_facts)
        .collect::<Vec<_>>();
    for function in &module.callable_defs {
        let function_store = infer_function_value_facts(function, &module_constant_facts);
        store.expr_facts.extend(function_store.expr_facts);
        store
            .block_entry_facts
            .extend(function_store.block_entry_facts);
    }
    store
}

#[cfg(test)]
mod test {
    use super::{
        BoolSingletonFact, CallableFact, EnvFacts, ProvenanceFact, PyExactType, PyObjFacts,
        RefcountFact, RuntimeHelperId, ThrowSpec, ValueFacts, infer_module_value_facts,
    };
    use soac_core::block_py::{BlockTerm, ChildVisitable, HasSemanticInstrId, Visit};
    use soac_lowering::lower_python_to_blockpy_for_testing;
    use soac_lowering::passes::InstrCodegen;

    struct ReturnExprFinder {
        key: Option<soac_core::block_py::InstrKey>,
        function_id: soac_core::block_py::RuntimeFunctionId,
    }

    impl Visit<InstrCodegen> for ReturnExprFinder {
        fn visit_return_term(&mut self, value: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            self.key = Some(value.semantic_instr_key(self.function_id));
            self.visit_instr(value);
        }
    }

    struct FirstMatchingInstrFinder {
        key: Option<soac_core::block_py::InstrKey>,
        function_id: soac_core::block_py::RuntimeFunctionId,
        matches: fn(&InstrCodegen) -> bool,
    }

    impl Visit<InstrCodegen> for FirstMatchingInstrFinder {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if self.key.is_none() && (self.matches)(expr) {
                self.key = Some(expr.semantic_instr_key(self.function_id));
            }
            expr.visit_children(self);
        }
    }

    fn returned_py_facts(source: &str) -> PyObjFacts {
        let lowered = lower_python_to_blockpy_for_testing(
            format!(
                r#"
def f():
    return {source}
"#,
            )
            .as_str(),
        )
        .expect("transform should succeed")
        .codegen_module;
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f");
        let mut finder = ReturnExprFinder {
            key: None,
            function_id: function.function_id,
        };
        finder.visit_fn(function);
        let none_key = finder.key.expect("expected a return expression");

        let facts = infer_module_value_facts(&lowered);
        let Some(ValueFacts::PyObj(py_facts)) = facts.fact_for(none_key) else {
            panic!("missing facts for returned expression");
        };
        py_facts
    }

    fn first_matching_instr_py_facts(
        function_body: &str,
        matches: fn(&InstrCodegen) -> bool,
    ) -> PyObjFacts {
        let lowered = lower_python_to_blockpy_for_testing(
            format!(
                r#"
def f(obj, key, value):
{function_body}
"#,
            )
            .as_str(),
        )
        .expect("transform should succeed")
        .codegen_module;
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f");
        let mut finder = FirstMatchingInstrFinder {
            key: None,
            function_id: function.function_id,
            matches,
        };
        finder.visit_fn(function);
        let key = finder.key.expect("expected matching instruction");

        let facts = infer_module_value_facts(&lowered);
        let Some(ValueFacts::PyObj(py_facts)) = facts.fact_for(key) else {
            panic!("missing facts for matching instruction");
        };
        py_facts
    }

    fn branch_entry_envs(prefix: &str, condition: &str) -> (EnvFacts, EnvFacts) {
        let prefix = prefix
            .lines()
            .map(|line| format!("    {line}\n"))
            .collect::<String>();
        let lowered = lower_python_to_blockpy_for_testing(
            format!(
                r#"
def f(x, flag):
{prefix}
    if {condition}:
        return 1
    return 2
"#,
            )
            .as_str(),
        )
        .expect("transform should succeed")
        .codegen_module;
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f");
        let if_term = function
            .blocks
            .iter()
            .filter_map(|block| match &block.term {
                BlockTerm::IfTerm(if_term) => Some(if_term),
                _ => None,
            })
            .last()
            .expect("expected lowered conditional branch");
        let facts = infer_module_value_facts(&lowered);
        let then_entry = facts
            .block_entry_fact(function.function_id, if_term.then_label)
            .expect("missing then-entry facts");
        let else_entry = facts
            .block_entry_fact(function.function_id, if_term.else_label)
            .expect("missing else-entry facts");
        (then_entry.clone(), else_entry.clone())
    }

    fn branch_entry_py_facts(condition: &str) -> (PyObjFacts, PyObjFacts) {
        let (then_entry, else_entry) = branch_entry_envs("", condition);
        (
            sole_local_py_fact(&then_entry),
            sole_local_py_fact(&else_entry),
        )
    }

    fn sole_local_py_fact(env: &EnvFacts) -> PyObjFacts {
        let mut facts = env.local_pyobj_facts();
        let Some((_, fact)) = facts.next() else {
            panic!("expected one local fact");
        };
        assert!(
            facts.next().is_none(),
            "expected exactly one local fact for test"
        );
        fact
    }

    fn local_py_facts(env: &EnvFacts) -> Vec<PyObjFacts> {
        env.local_pyobj_facts()
            .map(|(_, facts)| facts)
            .collect::<Vec<_>>()
    }

    #[test]
    fn infers_none_singleton_fact_for_module_constant_load() {
        let py_facts = returned_py_facts("None");
        assert!(py_facts.is_none());
        assert!(py_facts.is_exact_type(PyExactType::NoneType));
        assert_eq!(py_facts.is_truthy(), Some(false));
        assert!(py_facts.is_immortal());
    }

    #[test]
    fn infers_bool_singleton_facts_for_module_constant_loads() {
        let py_facts = returned_py_facts("True");
        assert!(py_facts.is_exact_type(PyExactType::Bool));
        assert!(py_facts.is_known_not_none());
        assert_eq!(py_facts.bool_singleton, BoolSingletonFact::IsTrue);
        assert_eq!(py_facts.refcount, RefcountFact::Immortal);
        assert_eq!(py_facts.is_truthy(), Some(true));

        let py_facts = returned_py_facts("False");
        assert!(py_facts.is_exact_type(PyExactType::Bool));
        assert!(py_facts.is_known_not_none());
        assert_eq!(py_facts.bool_singleton, BoolSingletonFact::IsFalse);
        assert_eq!(py_facts.refcount, RefcountFact::Immortal);
        assert_eq!(py_facts.is_truthy(), Some(false));
    }

    #[test]
    fn infers_none_singleton_facts_for_side_effect_operation_results() {
        for (source, matches) in [
            (
                "    obj.attr = value",
                (|expr| matches!(expr, InstrCodegen::SetAttr(_))) as fn(&InstrCodegen) -> bool,
            ),
            (
                "    obj[key] = value",
                (|expr| matches!(expr, InstrCodegen::SetItem(_))) as fn(&InstrCodegen) -> bool,
            ),
            (
                "    del obj[key]",
                (|expr| matches!(expr, InstrCodegen::DelItem(_))) as fn(&InstrCodegen) -> bool,
            ),
            (
                "    del value",
                (|expr| matches!(expr, InstrCodegen::Del(_))) as fn(&InstrCodegen) -> bool,
            ),
        ] {
            let py_facts = first_matching_instr_py_facts(source, matches);
            assert!(py_facts.is_none(), "{source}");
            assert!(py_facts.is_exact_type(PyExactType::NoneType), "{source}");
            assert!(py_facts.is_immortal(), "{source}");
        }
    }

    #[test]
    fn infers_immortal_refcount_for_module_constant_loads() {
        let py_facts = returned_py_facts("'field'");
        assert!(py_facts.is_exact_type(PyExactType::Str));
        assert_eq!(py_facts.refcount, RefcountFact::Immortal);
        assert!(matches!(
            py_facts.provenance,
            ProvenanceFact::ModuleConstant(_)
        ));
    }

    #[test]
    fn bool_object_facts_are_immortal_without_known_value() {
        let py_facts = PyObjFacts::bool_object();

        assert!(py_facts.is_exact_type(PyExactType::Bool));
        assert!(py_facts.is_known_not_none());
        assert_eq!(py_facts.bool_singleton, BoolSingletonFact::Unknown);
        assert_eq!(py_facts.is_truthy(), None);
        assert!(py_facts.is_immortal());
    }

    #[test]
    fn infers_exact_builtin_types_for_literal_module_constant_loads() {
        let py_facts = returned_py_facts("'red'");
        assert!(py_facts.is_exact_type(PyExactType::Str));
        assert!(py_facts.is_known_not_none());

        let py_facts = returned_py_facts("42");
        assert!(py_facts.is_exact_type(PyExactType::Int));
        assert!(py_facts.is_known_not_none());
    }

    #[test]
    fn infers_literal_truthiness_for_module_constant_loads() {
        let py_facts = returned_py_facts("''");
        assert_eq!(py_facts.is_truthy(), Some(false));

        let py_facts = returned_py_facts("b'x'");
        assert_eq!(py_facts.is_truthy(), Some(true));

        let py_facts = returned_py_facts("0");
        assert_eq!(py_facts.is_truthy(), Some(false));

        let py_facts = returned_py_facts("0.5");
        assert_eq!(py_facts.is_truthy(), Some(true));
    }

    #[test]
    fn runtime_helper_facts_mark_helpers_as_callable_py_objects() {
        let py_facts = PyObjFacts::runtime_helper(RuntimeHelperId::Globals);
        assert!(py_facts.is_known_not_none());
        assert_eq!(py_facts.is_truthy(), Some(true));
        assert_eq!(
            py_facts.callable,
            CallableFact::RuntimeHelper(RuntimeHelperId::Globals)
        );
    }

    #[test]
    fn runtime_helper_ids_are_resolved_from_runtime_symbols() {
        assert_eq!(
            RuntimeHelperId::from_runtime_symbol("_index"),
            Some(RuntimeHelperId::Index)
        );
        assert_eq!(
            RuntimeHelperId::from_runtime_symbol("next_or_sentinel"),
            Some(RuntimeHelperId::NextOrSentinel)
        );
        assert_eq!(RuntimeHelperId::from_runtime_symbol("not_a_helper"), None);
    }

    #[test]
    fn runtime_helper_signatures_declare_result_and_throw_policy() {
        let signature = RuntimeHelperId::NextOrSentinel.signature();
        assert_eq!(signature.throws, ThrowSpec::ThrowsOnNullPyObj);
        let ValueFacts::PyObj(result_facts) = signature.result else {
            panic!("next_or_sentinel should return a Python object");
        };
        assert!(result_facts.is_known_not_none());

        let signature = RuntimeHelperId::Str.signature();
        assert_eq!(signature.throws, ThrowSpec::ThrowsOnNullPyObj);
        let ValueFacts::PyObj(result_facts) = signature.result else {
            panic!("str should return a Python object");
        };
        assert!(result_facts.is_exact_type(PyExactType::Str));

        let signature = RuntimeHelperId::Index.signature();
        assert_eq!(signature.throws, ThrowSpec::ThrowsOnNullPyObj);
        let ValueFacts::PyObj(result_facts) = signature.result else {
            panic!("_index should return a Python object");
        };
        assert!(result_facts.is_exact_type(PyExactType::Int));
    }

    #[test]
    fn infers_exact_int_operator_facts_from_exact_int_locals() {
        let (then_entry, _) = branch_entry_envs("x = 1\ny = 2\nz = x + y\nw = z < y", "w is True");
        let facts = local_py_facts(&then_entry);

        assert!(
            facts
                .iter()
                .any(|fact| fact.is_exact_type(PyExactType::Int)),
            "expected at least one propagated exact-int local fact"
        );
        assert!(
            facts
                .iter()
                .any(|fact| fact.is_exact_type(PyExactType::Bool)),
            "expected at least one propagated bool local fact"
        );
    }

    #[test]
    fn narrows_none_fact_across_is_none_branch_edges() {
        let (then_facts, else_facts) = branch_entry_py_facts("x is None");

        assert!(then_facts.is_none());
        assert!(then_facts.is_exact_type(PyExactType::NoneType));
        assert!(then_facts.is_immortal());
        assert!(else_facts.is_known_not_none());
    }

    #[test]
    fn narrows_none_fact_across_is_not_none_branch_edges() {
        let (then_facts, else_facts) = branch_entry_py_facts("x is not None");

        assert!(then_facts.is_known_not_none());
        assert!(else_facts.is_none());
        assert!(else_facts.is_exact_type(PyExactType::NoneType));
    }

    #[test]
    fn narrows_bool_singleton_fact_across_is_true_branch_edge() {
        let (then_entry, else_entry) = branch_entry_envs("", "x is True");

        let then_facts = sole_local_py_fact(&then_entry);
        assert!(then_facts.is_true_singleton());
        assert!(then_facts.is_exact_type(PyExactType::Bool));
        assert_eq!(then_facts.is_truthy(), Some(true));
        assert!(then_facts.is_immortal());
        assert_eq!(local_py_facts(&else_entry).len(), 0);
    }

    #[test]
    fn narrows_bool_singleton_fact_across_is_not_false_branch_else_edge() {
        let (then_entry, else_entry) = branch_entry_envs("", "x is not False");

        assert_eq!(local_py_facts(&then_entry).len(), 0);
        let else_facts = sole_local_py_fact(&else_entry);
        assert!(else_facts.is_false_singleton());
        assert!(else_facts.is_exact_type(PyExactType::Bool));
        assert_eq!(else_facts.is_truthy(), Some(false));
        assert!(else_facts.is_immortal());
    }

    #[test]
    fn transfers_local_store_facts_to_successor_entries() {
        let (then_entry, else_entry) = branch_entry_envs("x = None", "flag");

        assert!(sole_local_py_fact(&then_entry).is_none());
        assert!(sole_local_py_fact(&else_entry).is_none());
    }

    #[test]
    fn transfers_local_load_copy_facts_to_successor_entries() {
        let (then_entry, else_entry) = branch_entry_envs("x = None\ny = x", "flag");

        let then_facts = local_py_facts(&then_entry);
        let else_facts = local_py_facts(&else_entry);
        assert_eq!(then_facts.len(), 2);
        assert_eq!(else_facts.len(), 2);
        assert!(then_facts.iter().all(|facts| facts.is_none()));
        assert!(else_facts.iter().all(|facts| facts.is_none()));
    }

    #[test]
    fn local_delete_removes_facts_from_successor_entries() {
        let (then_entry, else_entry) = branch_entry_envs("x = None\ndel x", "flag");

        let then_facts = local_py_facts(&then_entry);
        let else_facts = local_py_facts(&else_entry);
        assert_eq!(then_facts.len(), 0, "{then_facts:?}");
        assert_eq!(else_facts.len(), 0, "{else_facts:?}");
    }
}
