use crate::block_py::{
    BinOpKind, Block, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, ChildVisitable,
    FunctionId, HasSemanticInstrId, InstrCodegen, InstrKey, InstrResolved, Literal, LocalLocation,
    NameLike, NumberLiteralValue, UnaryOpKind, Visit,
};
use crate::passes::CodegenModuleShape;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TruthinessFact {
    Unknown,
    AlwaysTrue,
    AlwaysFalse,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PyExactType {
    NoneType,
    Bool,
    Str,
    Bytes,
    Int,
    Float,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TypeFact {
    Unknown,
    Exact(PyExactType),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RuntimeSingleton {
    None,
    True,
    False,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum NoneFact {
    Unknown,
    IsNone,
    IsNotNone,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BoolSingletonFact {
    Unknown,
    IsTrue,
    IsFalse,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RefcountFact {
    Unknown,
    Immortal,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProvenanceFact {
    Unknown,
    RuntimeSingleton(RuntimeSingleton),
    ModuleConstant(u32),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PyObjFacts {
    pub ty: TypeFact,
    pub truthiness: TruthinessFact,
    pub none: NoneFact,
    pub bool_singleton: BoolSingletonFact,
    pub refcount: RefcountFact,
    pub provenance: ProvenanceFact,
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
        }
    }

    pub const fn with_module_constant(mut self, index: u32) -> Self {
        self.provenance = ProvenanceFact::ModuleConstant(index);
        self
    }

    pub const fn known_not_none() -> Self {
        Self {
            ty: TypeFact::Unknown,
            truthiness: TruthinessFact::Unknown,
            none: NoneFact::IsNotNone,
            bool_singleton: BoolSingletonFact::Unknown,
            refcount: RefcountFact::Unknown,
            provenance: ProvenanceFact::Unknown,
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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ValueFacts {
    PyObj(PyObjFacts),
}

impl ValueFacts {
    pub const fn unknown_pyobj() -> Self {
        Self::PyObj(PyObjFacts::unknown())
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
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

    fn with_local_pyobj_fact(location: LocalLocation, facts: PyObjFacts) -> Self {
        let mut env = Self::default();
        env.local_pyobj_facts.insert(location, facts);
        env
    }

    fn intersect_with(&mut self, other: &Self) {
        self.local_pyobj_facts
            .retain(|location, facts| other.local_pyobj_fact(*location) == Some(*facts));
    }
}

#[derive(Debug, Clone, Default)]
pub struct FactStore {
    expr_facts: HashMap<InstrKey, ValueFacts>,
    block_entry_facts: HashMap<(FunctionId, BlockLabel), EnvFacts>,
}

impl FactStore {
    pub fn fact_for(&self, key: InstrKey) -> Option<ValueFacts> {
        self.expr_facts.get(&key).copied()
    }

    pub fn block_entry_fact(
        &self,
        function_id: FunctionId,
        label: BlockLabel,
    ) -> Option<&EnvFacts> {
        self.block_entry_facts.get(&(function_id, label))
    }

    pub fn expr_facts(&self) -> impl Iterator<Item = (InstrKey, ValueFacts)> + '_ {
        self.expr_facts.iter().map(|(key, facts)| (*key, *facts))
    }

    pub fn block_entry_facts(&self) -> impl Iterator<Item = ((FunctionId, BlockLabel), &EnvFacts)> {
        self.block_entry_facts
            .iter()
            .map(|(key, facts)| (*key, facts))
    }

    fn merge_block_entry_facts(
        &mut self,
        function_id: FunctionId,
        label: BlockLabel,
        facts: EnvFacts,
    ) {
        self.block_entry_facts
            .entry((function_id, label))
            .and_modify(|existing| existing.intersect_with(&facts))
            .or_insert(facts);
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
            _ => ValueFacts::unknown_pyobj(),
        }
    }

    fn infer_block_edge_facts(&mut self, block: &Block<InstrCodegen>) {
        let BlockTerm::IfTerm(if_term) = &block.term else {
            return;
        };
        let Some((location, then_is_none)) = self.infer_none_branch_test(&if_term.test) else {
            return;
        };
        let none_facts = EnvFacts::with_local_pyobj_fact(location, PyObjFacts::none_singleton());
        let not_none_facts =
            EnvFacts::with_local_pyobj_fact(location, PyObjFacts::known_not_none());
        let (then_facts, else_facts) = if then_is_none {
            (none_facts, not_none_facts)
        } else {
            (not_none_facts, none_facts)
        };
        self.store.merge_block_entry_facts(
            self.function.function_id,
            if_term.then_label,
            then_facts,
        );
        self.store.merge_block_entry_facts(
            self.function.function_id,
            if_term.else_label,
            else_facts,
        );
    }

    fn infer_none_branch_test(&self, test: &InstrCodegen) -> Option<(LocalLocation, bool)> {
        match test {
            InstrCodegen::BinOp(op) if op.kind == BinOpKind::Is => {
                infer_local_is_none_comparison(&op.left, &op.right, self)
            }
            InstrCodegen::UnaryOp(op) if op.kind == UnaryOpKind::Not => self
                .infer_none_branch_test(&op.operand)
                .map(|(location, then_is_none)| (location, !then_is_none)),
            _ => None,
        }
    }
}

impl Visit<InstrCodegen> for FunctionFactInferer<'_> {
    fn visit_instr(&mut self, expr: &InstrCodegen)
    where
        InstrCodegen: ChildVisitable<InstrCodegen>,
    {
        let key = expr.semantic_instr_key(self.function.function_id);
        let facts = self.infer_expr_facts(expr);
        self.store.expr_facts.insert(key, facts);
        crate::block_py::walk_expr(self, expr);
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
        inferer.infer_block_edge_facts(block);
    }
    for block in &function.blocks {
        inferer
            .store
            .block_entry_facts
            .entry((function.function_id, block.label))
            .or_insert_with(EnvFacts::default);
    }
    inferer.store
}

fn infer_local_is_none_comparison(
    left: &InstrCodegen,
    right: &InstrCodegen,
    inferer: &FunctionFactInferer<'_>,
) -> Option<(LocalLocation, bool)> {
    if expr_is_none(right, inferer) {
        local_load_location(left).map(|location| (location, true))
    } else if expr_is_none(left, inferer) {
        local_load_location(right).map(|location| (location, true))
    } else {
        None
    }
}

fn expr_is_none(expr: &InstrCodegen, inferer: &FunctionFactInferer<'_>) -> bool {
    match inferer.infer_expr_facts(expr) {
        ValueFacts::PyObj(py_facts) => py_facts.is_none(),
    }
}

fn local_load_location(expr: &InstrCodegen) -> Option<LocalLocation> {
    match expr {
        InstrCodegen::Load(op) => op.name.local_location(),
        _ => None,
    }
}

fn infer_runtime_name_load_facts(name: &impl NameLike) -> Option<ValueFacts> {
    if name.is_runtime_symbol("NONE") {
        Some(ValueFacts::PyObj(PyObjFacts::none_singleton()))
    } else if name.is_runtime_symbol("TRUE") {
        Some(ValueFacts::PyObj(PyObjFacts::bool_singleton(true)))
    } else if name.is_runtime_symbol("FALSE") {
        Some(ValueFacts::PyObj(PyObjFacts::bool_singleton(false)))
    } else {
        None
    }
}

fn module_constant_load_fact(index: u32, module_constant_facts: &[ValueFacts]) -> ValueFacts {
    module_constant_facts
        .get(index as usize)
        .copied()
        .map(|facts| match facts {
            ValueFacts::PyObj(py_facts) => ValueFacts::PyObj(py_facts.with_module_constant(index)),
        })
        .unwrap_or_else(|| ValueFacts::PyObj(PyObjFacts::module_constant(index)))
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
        infer_module_value_facts, BoolSingletonFact, EnvFacts, PyExactType, PyObjFacts,
        RefcountFact, ValueFacts,
    };
    use crate::block_py::{BlockTerm, ChildVisitable, HasSemanticInstrId, InstrCodegen, Visit};
    use crate::lower_python_to_blockpy_for_testing;

    struct ReturnExprFinder {
        key: Option<crate::block_py::InstrKey>,
        function_id: crate::block_py::FunctionId,
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

    fn branch_entry_py_facts(condition: &str) -> (PyObjFacts, PyObjFacts) {
        let lowered = lower_python_to_blockpy_for_testing(
            format!(
                r#"
def f(x):
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
            .find_map(|block| match &block.term {
                BlockTerm::IfTerm(if_term) => Some(if_term),
                _ => None,
            })
            .expect("expected lowered conditional branch");
        let facts = infer_module_value_facts(&lowered);
        let then_entry = facts
            .block_entry_fact(function.function_id, if_term.then_label)
            .expect("missing then-entry facts");
        let else_entry = facts
            .block_entry_fact(function.function_id, if_term.else_label)
            .expect("missing else-entry facts");
        (
            sole_local_py_fact(then_entry),
            sole_local_py_fact(else_entry),
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
}
