use crate::passes::{BlockPyModuleShape, InstrBlockPy};
use soac_core::block_py::literal::{Literal, NumberLiteralValue};
use soac_core::block_py::{
    BinOpKind, Block, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, ChildVisitable,
    ConstantExpr, HasSemanticInstrId, InstrKey, LocalLocation, NameLike, RuntimeName, UnaryOpKind,
    Visit,
};
use soac_core::block_py::{visit_operand_takes, visit_term_operand_takes};
#[allow(unused_imports)]
use soac_ir_typed::value_facts::{
    BoolFacts, BoolSingletonFact, CallableFact, EnvFacts, FactStore, I32Facts, I64Facts, NoneFact,
    ProvenanceFact, PyExactType, PyObjFacts, RefcountFact, RuntimeHelperId, RuntimeHelperSignature,
    RuntimeSingleton, ThrowSpec, TruthinessFact, TypeFact, ValueFacts,
};
use std::collections::HashMap;

struct FunctionFactInferer<'a> {
    function: &'a BlockPyFunction<BlockPyModuleShape>,
    module_constant_facts: &'a [ValueFacts],
    store: FactStore,
}

impl FunctionFactInferer<'_> {
    fn infer_expr_facts(&self, expr: &InstrBlockPy) -> ValueFacts {
        match expr {
            InstrBlockPy::Load(op) => {
                infer_runtime_name_load_facts(&op.name).unwrap_or_else(|| {
                    op.name
                        .location
                        .as_constant()
                        .map(|index| module_constant_load_fact(index, self.module_constant_facts))
                        .unwrap_or_else(ValueFacts::unknown_pyobj)
                })
            }
            InstrBlockPy::Call(op) => {
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
            InstrBlockPy::BinOp(op) => infer_binop_result_facts(
                op.kind,
                self.infer_expr_facts(&op.left),
                self.infer_expr_facts(&op.right),
            )
            .unwrap_or_else(ValueFacts::unknown_pyobj),
            InstrBlockPy::UnaryOp(op) => {
                infer_unary_result_facts(op.kind, self.infer_expr_facts(&op.operand))
                    .unwrap_or_else(ValueFacts::unknown_pyobj)
            }
            InstrBlockPy::SetAttr(_)
            | InstrBlockPy::SetItem(_)
            | InstrBlockPy::DelItem(_)
            | InstrBlockPy::Del(_) => ValueFacts::PyObj(PyObjFacts::none_singleton()),
            InstrBlockPy::Tuple(_) => ValueFacts::PyObj(PyObjFacts::known_not_none()),
            _ => ValueFacts::unknown_pyobj(),
        }
    }

    fn infer_expr_facts_in_env(&self, expr: &InstrBlockPy, env: &EnvFacts) -> ValueFacts {
        match expr {
            InstrBlockPy::Load(op) => op
                .name
                .local_location()
                .and_then(|location| env.local_pyobj_fact(location))
                .map(ValueFacts::PyObj)
                .unwrap_or_else(|| self.infer_expr_facts(expr)),
            InstrBlockPy::BinOp(op) => infer_binop_result_facts(
                op.kind,
                self.infer_expr_facts_in_env(&op.left, env),
                self.infer_expr_facts_in_env(&op.right, env),
            )
            .unwrap_or_else(ValueFacts::unknown_pyobj),
            InstrBlockPy::UnaryOp(op) => {
                infer_unary_result_facts(op.kind, self.infer_expr_facts_in_env(&op.operand, env))
                    .unwrap_or_else(ValueFacts::unknown_pyobj)
            }
            _ => self.infer_expr_facts(expr),
        }
    }

    fn transfer_block_env(&self, block: &Block<InstrBlockPy>, entry: &EnvFacts) -> EnvFacts {
        let mut env = entry.clone();
        for instr in &block.body {
            self.transfer_instr_env(instr, &mut env);
        }
        visit_term_operand_takes(&block.term, |location| {
            if let soac_core::block_py::OperandLocation::Local(location) = location {
                env.remove_local_pyobj_fact(location);
            }
        });
        env
    }

    fn transfer_instr_env(&self, instr: &InstrBlockPy, env: &mut EnvFacts) {
        visit_operand_takes(instr, |location| {
            if let soac_core::block_py::OperandLocation::Local(location) = location {
                env.remove_local_pyobj_fact(location);
            }
        });
        match instr {
            InstrBlockPy::Store(op) => {
                let Some(location) = op.name.local_location() else {
                    return;
                };
                match self.infer_expr_facts_in_env(&op.value, env).as_pyobj() {
                    Some(py_facts) => env.set_local_pyobj_fact(location, py_facts),
                    None => env.remove_local_pyobj_fact(location),
                }
            }
            InstrBlockPy::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    env.remove_local_pyobj_fact(location);
                }
            }
            InstrBlockPy::CallArgumentOp(op) => {
                for name in op.written_names() {
                    if let Some(location) = name.local_location() {
                        env.remove_local_pyobj_fact(location);
                    }
                }
            }
            _ => {}
        }
    }

    fn successor_envs(
        &self,
        block: &Block<InstrBlockPy>,
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
            BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {
                Vec::new()
            }
        }
    }

    fn infer_if_edge_facts(
        &self,
        if_term: &soac_core::block_py::TermIf<InstrBlockPy>,
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
        test: &InstrBlockPy,
    ) -> Option<(LocalLocation, Option<PyObjFacts>, Option<PyObjFacts>)> {
        match test {
            InstrBlockPy::BinOp(op) if op.kind == BinOpKind::Is => {
                infer_local_is_singleton_comparison(&op.left, &op.right, self)
            }
            InstrBlockPy::UnaryOp(op) if op.kind == UnaryOpKind::Not => self
                .infer_branch_local_fact(&op.operand)
                .map(|(location, then_fact, else_fact)| (location, else_fact, then_fact)),
            _ => None,
        }
    }
}

impl Visit<InstrBlockPy> for FunctionFactInferer<'_> {
    fn visit_instr(&mut self, expr: &InstrBlockPy)
    where
        InstrBlockPy: ChildVisitable<InstrBlockPy>,
    {
        // Synthetic trace/counter instrumentation is inserted after semantic ID
        // assignment. It should not receive fake expression facts of its own.
        if let Some(instr_id) = expr.try_semantic_instr_id() {
            let key = InstrKey::new(self.function.function_id, instr_id);
            let facts = self.infer_expr_facts(expr);
            self.store.insert_expr_fact(key, facts);
        }
        soac_core::block_py::walk_expr(self, expr);
    }
}

fn infer_function_value_facts(
    function: &BlockPyFunction<BlockPyModuleShape>,
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
        inferer.store.insert_block_entry_fact(
            function.function_id,
            block.label,
            block_entry_facts
                .get(&block.label)
                .cloned()
                .unwrap_or_default(),
        );
    }
    inferer.store
}

fn infer_local_is_singleton_comparison(
    left: &InstrBlockPy,
    right: &InstrBlockPy,
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
    expr: &InstrBlockPy,
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

fn local_load_location(expr: &InstrBlockPy) -> Option<LocalLocation> {
    match expr {
        InstrBlockPy::Load(op) => op.name.local_location(),
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

fn infer_module_constant_facts(expr: &ConstantExpr) -> ValueFacts {
    match expr {
        ConstantExpr::RuntimeName(name) => {
            infer_runtime_name_facts(*name).unwrap_or_else(ValueFacts::unknown_pyobj)
        }
        ConstantExpr::Literal(op) => infer_literal_facts(op.as_literal()),
    }
}

fn infer_runtime_name_facts(name: RuntimeName) -> Option<ValueFacts> {
    match name {
        RuntimeName::None => Some(ValueFacts::PyObj(PyObjFacts::none_singleton())),
        RuntimeName::True => Some(ValueFacts::PyObj(PyObjFacts::bool_singleton(true))),
        RuntimeName::False => Some(ValueFacts::PyObj(PyObjFacts::bool_singleton(false))),
        _ => RuntimeHelperId::from_runtime_symbol(name.name())
            .map(PyObjFacts::runtime_helper)
            .map(ValueFacts::PyObj),
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

pub fn infer_module_value_facts(module: &BlockPyModule<BlockPyModuleShape>) -> FactStore {
    let mut store = FactStore::default();
    let module_constant_facts = module
        .module_constants
        .iter()
        .map(infer_module_constant_facts)
        .collect::<Vec<_>>();
    for function in &module.callable_defs {
        let function_store = infer_function_value_facts(function, &module_constant_facts);
        store.extend_expr_facts(function_store.expr_facts());
        store.extend_block_entry_facts(
            function_store
                .block_entry_facts()
                .map(|(key, facts)| (key, facts.clone())),
        );
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
    use soac_ir_blockpy::InstrBlockPy;
    use soac_lowering::lower_python_to_blockpy_for_testing;

    struct ReturnExprFinder {
        key: Option<soac_core::block_py::InstrKey>,
        function_id: soac_core::block_py::RuntimeFunctionId,
    }

    impl Visit<InstrBlockPy> for ReturnExprFinder {
        fn visit_return_term(&mut self, value: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            self.key = Some(value.semantic_instr_key(self.function_id));
            self.visit_instr(value);
        }
    }

    struct FirstMatchingInstrFinder {
        key: Option<soac_core::block_py::InstrKey>,
        function_id: soac_core::block_py::RuntimeFunctionId,
        matches: fn(&InstrBlockPy) -> bool,
    }

    impl Visit<InstrBlockPy> for FirstMatchingInstrFinder {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
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
        .blockpy_module;
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
        matches: fn(&InstrBlockPy) -> bool,
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
        .blockpy_module;
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
        .blockpy_module;
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
                (|expr| matches!(expr, InstrBlockPy::SetAttr(_))) as fn(&InstrBlockPy) -> bool,
            ),
            (
                "    obj[key] = value",
                (|expr| matches!(expr, InstrBlockPy::SetItem(_))) as fn(&InstrBlockPy) -> bool,
            ),
            (
                "    del obj[key]",
                (|expr| matches!(expr, InstrBlockPy::DelItem(_))) as fn(&InstrBlockPy) -> bool,
            ),
            (
                "    del value",
                (|expr| matches!(expr, InstrBlockPy::Del(_))) as fn(&InstrBlockPy) -> bool,
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
        assert_eq!(RuntimeHelperId::from_runtime_symbol("not_a_helper"), None);
    }

    #[test]
    fn runtime_helper_signatures_declare_result_and_throw_policy() {
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
