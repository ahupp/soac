use crate::passes::{
    ConstructorFieldValue, InlinePlanModule, compute_typed_function_local_must_bound_ins,
    value_facts,
};
#[allow(unused_imports)]
use soac_core::block_py;
#[allow(unused_imports)]
use soac_core::block_py::{
    BinOpKind, Block, BlockArg, BlockEdge, BlockLabel, BlockParam, BlockParamRole, BlockPyFunction,
    BlockPyModule, BlockTerm, Call, CallArgKeyword, CallArgPositional, CallDirect,
    CalleeFunctionId, CellLocation, ChildVisitable, ClosureInit, ClosureSlot, ConstantExpr, Del,
    HasMeta, HasSemanticInstrId, Instr, InstrId, InstrKey, InstrWithConstantNone, IntLiteral,
    Literal, LiteralValue, Load, LocalLocation, MakeCell, MapInstr, Mappable, Meta, NameLike,
    NameLocation, NumberLiteral, NumberLiteralValue, ParamKind, PreservedLocation,
    PreservedSlotStorage, PrettyPrint, PrettyPrinter, ResolvedName, RuntimeFunctionId, RuntimeName,
    SetAttr, Store, TermIf, TryMapInstr, TryMapModule, TryMapTerm, Tuple, UnaryOpKind, Visit,
    VisitMut, WithMeta,
};
use soac_ir_blockpy::{
    BlockPyModuleShape, InstrBlockPy, constructor_init_function_id_for_entry_function,
    is_constructor_entry_function,
};
use soac_ir_typed::emit_v3::MechanicalExitKind;
use soac_ir_typed::plan_v3::Rep;
use soac_ir_typed::{
    BoolFacts, FactStore, InstrTyped, PyExactType, PyObjFacts, TruthinessFact, TypedAttrAccessPlan,
    TypedAttrOwnerRef, TypedBlock, TypedBlockExtra, TypedBlockPyModuleShape, TypedCall,
    TypedCallAccessPlan, TypedCallEmissionPlan, TypedCallEmissionPlans, TypedConstructorInitPlan,
    TypedConstructorInitPlanSource, TypedDirectCallArgPlan, TypedDirectCallArgSource,
    TypedDirectCallGuardTest, TypedDirectCallGuardTestKind, TypedDirectCallableCall,
    TypedDirectCallableCallGuard, TypedDirectMethodCall, TypedDirectMethodCallGuard, TypedGetAttr,
    TypedGuardedCallableCall, TypedGuardedMethodCall, TypedInstrExtra, TypedPlannedResult,
    TypedPyObjectOwnershipPlan, TypedResultDemand, TypedSetAttr, TypedTruthy, ValueFacts,
};
use std::collections::{HashMap, HashSet};

mod linearize;
mod trusted_owner;
mod virtual_objects;

pub use linearize::*;
pub use trusted_owner::*;
pub use virtual_objects::*;

pub fn annotate_typed_module_value_facts(
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
    facts: &FactStore,
) -> usize {
    module
        .callable_defs
        .iter_mut()
        .map(|function| annotate_typed_function_value_facts(function, facts))
        .sum()
}

pub fn annotate_typed_function_value_facts(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    facts: &FactStore,
) -> usize {
    struct Annotator<'a> {
        function_id: RuntimeFunctionId,
        facts: &'a FactStore,
        changed: usize,
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            let instr_id = expr.meta().instr_id;
            if let Some(instr_id) = instr_id {
                if let Some(facts) = self
                    .facts
                    .fact_for(InstrKey::new(self.function_id, instr_id))
                {
                    if let Some(extra) = expr.typed_extra_mut() {
                        self.changed += usize::from(extra.refine_result_facts(facts));
                    }
                }
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        function_id: function.function_id,
        facts,
        changed: 0,
    };
    annotator.visit_fn_mut(function);
    annotator.changed
}

fn set_typed_instr_demand(expr: &mut InstrTyped, demand: TypedResultDemand) -> usize {
    expr.typed_extra_mut()
        .map(|extra| usize::from(extra.set_demand(demand)))
        .unwrap_or(0)
}

fn set_typed_instr_planned_result(
    expr: &mut InstrTyped,
    planned_result: TypedPlannedResult,
) -> usize {
    expr.typed_extra_mut()
        .map(|extra| usize::from(extra.set_planned_result(planned_result)))
        .unwrap_or(0)
}

fn clear_typed_instr_planned_result(expr: &mut InstrTyped) -> usize {
    expr.typed_extra_mut()
        .map(|extra| usize::from(extra.clear_planned_result()))
        .unwrap_or(0)
}

fn annotate_call_arg_input_demands(
    args: &mut [CallArgPositional<InstrTyped>],
    keywords: &mut [CallArgKeyword<InstrTyped>],
) -> usize {
    let mut changed = 0;
    for arg in args {
        changed += annotate_pyobject_borrowed_input_demand(arg.expr_mut());
    }
    for keyword in keywords {
        changed += annotate_pyobject_borrowed_input_demand(keyword.expr_mut());
    }
    changed
}

fn annotate_pyobject_borrowed_input_demand(expr: &mut InstrTyped) -> usize {
    let mut changed = set_typed_instr_demand(expr, TypedResultDemand::PYOBJECT_BORROWED_OK);
    changed += annotate_typed_child_demands(expr);
    changed
}

fn annotate_typed_child_demands(expr: &mut InstrTyped) -> usize {
    match expr {
        InstrTyped::BinOp(op) => {
            annotate_pyobject_borrowed_input_demand(op.left.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.right.as_mut())
        }
        InstrTyped::Truthy(op) => annotate_pyobject_borrowed_input_demand(op.value.as_mut()),
        InstrTyped::UnaryOp(op) => annotate_pyobject_borrowed_input_demand(op.operand.as_mut()),
        InstrTyped::Tuple(op) => op
            .values
            .iter_mut()
            .map(annotate_pyobject_borrowed_input_demand)
            .sum(),
        InstrTyped::CalleeFunctionId(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
        }
        InstrTyped::Store(store) => {
            let mut changed =
                set_typed_instr_demand(store.value.as_mut(), TypedResultDemand::PYOBJECT_OWNED);
            changed += annotate_typed_child_demands(store.value.as_mut());
            changed
        }
        InstrTyped::CallTyped(call) => {
            let mut changed =
                set_typed_instr_demand(call.func.as_mut(), TypedResultDemand::PYOBJECT_BORROWED_OK);
            changed += annotate_typed_child_demands(call.func.as_mut());
            changed += annotate_call_arg_input_demands(
                call.args.as_mut_slice(),
                call.keywords.as_mut_slice(),
            );
            changed
        }
        InstrTyped::GuardedCallableCallTyped(call) => {
            let mut changed =
                set_typed_instr_demand(call.func.as_mut(), TypedResultDemand::PYOBJECT_BORROWED_OK);
            changed += annotate_typed_child_demands(call.func.as_mut());
            changed += annotate_call_arg_input_demands(
                call.args.as_mut_slice(),
                call.keywords.as_mut_slice(),
            );
            changed
        }
        InstrTyped::GuardedMethodCallTyped(call) => {
            let mut changed =
                set_typed_instr_demand(call.func.as_mut(), TypedResultDemand::PYOBJECT_BORROWED_OK);
            changed += annotate_typed_child_demands(call.func.as_mut());
            changed += annotate_call_arg_input_demands(
                call.args.as_mut_slice(),
                call.keywords.as_mut_slice(),
            );
            changed
        }
        InstrTyped::DirectCallableCallTyped(call) => {
            let mut changed =
                set_typed_instr_demand(call.func.as_mut(), TypedResultDemand::PYOBJECT_BORROWED_OK);
            changed += annotate_typed_child_demands(call.func.as_mut());
            changed += annotate_call_arg_input_demands(call.args.as_mut_slice(), &mut []);
            changed
        }
        InstrTyped::DirectMethodCallTyped(call) => {
            let mut changed = set_typed_instr_demand(
                call.receiver.as_mut(),
                TypedResultDemand::PYOBJECT_BORROWED_OK,
            );
            changed += annotate_typed_child_demands(call.receiver.as_mut());
            changed += annotate_call_arg_input_demands(call.args.as_mut_slice(), &mut []);
            changed
        }
        InstrTyped::DirectCallGuardTest(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
        }
        InstrTyped::CallDirect(call) => {
            let mut changed = set_typed_instr_demand(
                call.callable.as_mut(),
                TypedResultDemand::PYOBJECT_BORROWED_OK,
            );
            changed += annotate_typed_child_demands(call.callable.as_mut());
            changed += annotate_call_arg_input_demands(
                call.args.as_mut_slice(),
                call.keywords.as_mut_slice(),
            );
            changed
        }
        InstrTyped::GetAttrTyped(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.attr.as_mut())
        }
        InstrTyped::SetAttrTyped(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.attr.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.replacement.as_mut())
        }
        InstrTyped::GetItem(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.index.as_mut())
        }
        InstrTyped::SetItem(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.index.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.replacement.as_mut())
        }
        InstrTyped::DelItem(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.index.as_mut())
        }
        InstrTyped::MakeFunctionWithClosure(op) => {
            annotate_pyobject_borrowed_input_demand(op.captures.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.param_defaults.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.annotate_fn.as_mut())
        }
        _ => 0,
    }
}

#[allow(dead_code)]
pub fn annotate_typed_module_result_demands(
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
) -> usize {
    module
        .callable_defs
        .iter_mut()
        .map(annotate_typed_function_result_demands)
        .sum()
}

pub fn annotate_typed_function_result_demands(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> usize {
    let mut changed = 0;
    for block in &mut function.blocks {
        for expr in &mut block.body {
            changed += set_typed_instr_demand(expr, TypedResultDemand::EffectOnly);
            changed += annotate_typed_child_demands(expr);
        }
        match &mut block.term {
            BlockTerm::IfTerm(if_term) => {
                changed += set_typed_instr_demand(&mut if_term.test, TypedResultDemand::I32_BOOL01);
                changed += annotate_typed_child_demands(&mut if_term.test);
            }
            BlockTerm::BranchTable(branch) => {
                changed += set_typed_instr_demand(&mut branch.index, TypedResultDemand::I64_INDEX);
                changed += annotate_typed_child_demands(&mut branch.index);
            }
            BlockTerm::Return(value) => {
                changed += set_typed_instr_demand(value, TypedResultDemand::PYOBJECT_BORROWED_OK);
                changed += annotate_typed_child_demands(value);
            }
            BlockTerm::Raise(raise_stmt) => {
                if let Some(exc) = raise_stmt.exc.as_mut() {
                    changed += set_typed_instr_demand(exc, TypedResultDemand::PYOBJECT_BORROWED_OK);
                    changed += annotate_typed_child_demands(exc);
                }
            }
            BlockTerm::Jump(_) => {}
        }
    }
    changed
}

#[allow(dead_code)]
pub fn annotate_typed_module_planned_results(
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
) -> usize {
    module
        .callable_defs
        .iter_mut()
        .map(annotate_typed_function_planned_results)
        .sum()
}

pub fn annotate_typed_function_planned_results(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> usize {
    struct Annotator {
        changed: usize,
    }

    impl VisitMut<InstrTyped> for Annotator {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);
            if let Some(planned_result) = plan_typed_instr_result(expr) {
                self.changed += set_typed_instr_planned_result(expr, planned_result);
            } else {
                self.changed += clear_typed_instr_planned_result(expr);
            }
        }
    }

    let mut annotator = Annotator { changed: 0 };
    annotator.visit_fn_mut(function);
    annotator.changed
}

fn plan_typed_instr_result(expr: &InstrTyped) -> Option<TypedPlannedResult> {
    let demand = expr.result_demand()?;
    Some(match demand {
        TypedResultDemand::EffectOnly => TypedPlannedResult::EffectOnly,
        TypedResultDemand::PyObject { borrowed_ok } => {
            let ownership = if typed_expr_exact_int_return_is_immortal_pyobject(expr) {
                TypedPyObjectOwnershipPlan::Immortal
            } else {
                match expr.result_facts().and_then(ValueFacts::as_pyobj) {
                    Some(py_facts) if py_facts.is_immortal() => {
                        TypedPyObjectOwnershipPlan::Immortal
                    }
                    _ if borrowed_ok => {
                        if let Some(location) = typed_instr_local_load_location(expr) {
                            TypedPyObjectOwnershipPlan::BorrowedLocal { location }
                        } else {
                            TypedPyObjectOwnershipPlan::Owned
                        }
                    }
                    _ => TypedPyObjectOwnershipPlan::Owned,
                }
            };
            TypedPlannedResult::PyObject { ownership }
        }
        TypedResultDemand::I32Bool01 => TypedPlannedResult::I32Bool01,
        TypedResultDemand::I64 | TypedResultDemand::I64Index => TypedPlannedResult::I64,
    })
}

pub(crate) fn typed_expr_planned_pyobject_ownership(
    expr: &InstrTyped,
) -> Option<TypedPyObjectOwnershipPlan> {
    if let Some(TypedPlannedResult::PyObject { ownership }) = expr.planned_result() {
        return Some(ownership);
    }
    typed_expr_exact_int_return_is_immortal_pyobject(expr)
        .then_some(TypedPyObjectOwnershipPlan::Immortal)
}

fn typed_expr_exact_int_return_is_immortal_pyobject(expr: &InstrTyped) -> bool {
    let Some(plan) = expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
    else {
        return false;
    };
    let [exit] = plan.hot_region.exits.as_slice() else {
        return false;
    };
    let MechanicalExitKind::Return { value } = exit.kind else {
        return false;
    };
    value.rep == Rep::PyObjectImmortal
}

fn typed_instr_local_load_location(expr: &InstrTyped) -> Option<LocalLocation> {
    match expr {
        InstrTyped::Load(op) => op.name.local_location(),
        _ => None,
    }
}

pub fn refresh_typed_function_value_facts(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> usize {
    struct Refresher {
        changed: usize,
    }

    impl VisitMut<InstrTyped> for Refresher {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);
            let Some(facts) = infer_typed_instr_result_facts(expr) else {
                return;
            };
            if let Some(extra) = expr.typed_extra_mut() {
                self.changed += usize::from(extra.refine_result_facts(facts));
            }
        }
    }

    let mut refresher = Refresher { changed: 0 };
    refresher.visit_fn_mut(function);
    refresher.changed
}

pub fn sync_typed_module_value_facts(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    facts: &mut FactStore,
) -> usize {
    struct Collector<'a> {
        function_id: RuntimeFunctionId,
        facts: &'a mut FactStore,
        changed: usize,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(instr_id) = expr.try_semantic_instr_id()
                && let Some(result_facts) = expr.result_facts()
            {
                let key = InstrKey::new(self.function_id, instr_id);
                if self.facts.fact_for(key) != Some(result_facts) {
                    self.facts.insert_expr_fact(key, result_facts);
                    self.changed += 1;
                }
            }
            expr.visit_children(self);
        }
    }

    let mut changed = 0;
    for function in &module.callable_defs {
        let mut collector = Collector {
            function_id: function.function_id,
            facts,
            changed: 0,
        };
        collector.visit_fn(function);
        changed += collector.changed;
    }
    changed
}

fn infer_typed_instr_result_facts(expr: &InstrTyped) -> Option<ValueFacts> {
    match expr {
        InstrTyped::Truthy(_) => Some(ValueFacts::Bool(BoolFacts)),
        InstrTyped::Load(op) => Some(typed_load_result_facts(op)),
        InstrTyped::BinOp(op) => value_facts::infer_binop_result_facts(
            op.kind,
            op.left.result_facts()?,
            op.right.result_facts()?,
        )
        .or(Some(ValueFacts::unknown_pyobj())),
        InstrTyped::UnaryOp(op) => {
            value_facts::infer_unary_result_facts(op.kind, op.operand.result_facts()?)
                .or(Some(ValueFacts::unknown_pyobj()))
        }
        InstrTyped::Tuple(_) => Some(ValueFacts::PyObj(PyObjFacts::known_not_none())),
        InstrTyped::CallTyped(op) => infer_typed_call_result_facts(
            op.func.as_ref(),
            op.args.as_slice(),
            op.keywords.as_slice(),
        )
        .or(Some(ValueFacts::unknown_pyobj())),
        InstrTyped::CallDirect(op) => infer_typed_call_result_facts(
            op.callable.as_ref(),
            op.args.as_slice(),
            op.keywords.as_slice(),
        )
        .or(Some(ValueFacts::unknown_pyobj())),
        InstrTyped::DirectCallGuardTest(_) => Some(ValueFacts::Bool(BoolFacts)),
        InstrTyped::SetAttrTyped(_)
        | InstrTyped::Store(_)
        | InstrTyped::SetItem(_)
        | InstrTyped::DelItem(_)
        | InstrTyped::Del(_) => Some(ValueFacts::PyObj(PyObjFacts::none_singleton())),
        _ => expr.typed_extra().map(|_| ValueFacts::unknown_pyobj()),
    }
}

fn typed_load_result_facts(op: &Load<InstrTyped>) -> ValueFacts {
    if let Some(index) = op.name.location.as_constant() {
        let py_facts = op
            .extra()
            .result_facts()
            .and_then(ValueFacts::as_pyobj)
            .unwrap_or_else(PyObjFacts::unknown)
            .with_module_constant(index)
            .with_immortal_refcount();
        return ValueFacts::PyObj(py_facts);
    }
    op.extra()
        .result_facts()
        .unwrap_or_else(ValueFacts::unknown_pyobj)
}

fn infer_typed_call_result_facts(
    func: &InstrTyped,
    args: &[CallArgPositional<InstrTyped>],
    keywords: &[CallArgKeyword<InstrTyped>],
) -> Option<ValueFacts> {
    if !keywords.is_empty()
        || !args
            .iter()
            .all(|arg| matches!(arg, CallArgPositional::Positional(_)))
    {
        return None;
    }
    func.result_facts()?
        .runtime_helper()
        .map(|helper| helper.signature().result)
}

pub fn validate_typed_function_value_facts(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> Result<(), String> {
    struct Validator<'a> {
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        errors: Vec<String>,
    }

    impl Visit<InstrTyped> for Validator<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(instr_id) = expr.meta().instr_id {
                if let Some(extra) = expr.typed_extra() {
                    if extra.result_facts().is_none() {
                        self.errors.push(format!(
                            "typed instruction {} in function {} has no embedded result facts",
                            instr_id, self.function.names.qualname
                        ));
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    let mut validator = Validator {
        function,
        errors: Vec::new(),
    };
    validator.visit_fn(function);
    if validator.errors.is_empty() {
        Ok(())
    } else {
        Err(validator.errors.join("; "))
    }
}

pub fn lower_typed_function_if_tests_to_truthy(
    mut function: BlockPyFunction<TypedBlockPyModuleShape>,
) -> BlockPyFunction<TypedBlockPyModuleShape> {
    for block in &mut function.blocks {
        if let BlockTerm::IfTerm(if_term) = &mut block.term {
            if matches!(if_term.test, InstrTyped::Truthy(_)) {
                continue;
            }
            let old_test = std::mem::replace(&mut if_term.test, InstrTyped::constant_none());
            let meta = old_test.meta();
            let mut truthy = TypedTruthy::new(old_test).with_meta(meta);
            truthy
                .extra
                .refine_result_facts(ValueFacts::Bool(BoolFacts));
            if_term.test = InstrTyped::Truthy(truthy);
        }
    }
    function
}

pub fn lower_typed_if_tests_to_truthy(
    mut module: BlockPyModule<TypedBlockPyModuleShape>,
) -> BlockPyModule<TypedBlockPyModuleShape> {
    module.callable_defs = module
        .callable_defs
        .into_iter()
        .map(lower_typed_function_if_tests_to_truthy)
        .collect();
    module
}

pub fn lower_typed_function_call_emission_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    plans: &TypedCallEmissionPlans,
) -> Result<usize, String> {
    if plans.is_empty() {
        return Ok(0);
    }

    struct Rewriter<'a> {
        plans: &'a TypedCallEmissionPlans,
        count: usize,
        error: Option<String>,
    }

    impl VisitMut<InstrTyped> for Rewriter<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            expr.visit_children_mut(self);
            let InstrTyped::CallTyped(call) = expr else {
                return;
            };
            let Some(instr_id) = call.try_semantic_instr_id() else {
                return;
            };
            let Some(plan) = self.plans.by_source.get(&instr_id) else {
                return;
            };
            if plan.is_empty() {
                return;
            }

            let old_expr = std::mem::replace(expr, InstrTyped::constant_none());
            let InstrTyped::CallTyped(call) = old_expr else {
                unreachable!("checked call shape before replacing typed instruction")
            };
            match plan {
                TypedCallEmissionPlan::Callable { function_guards } => {
                    *expr = InstrTyped::GuardedCallableCallTyped(
                        TypedGuardedCallableCall::from_typed_call(call, function_guards.clone()),
                    );
                    self.count += 1;
                }
                TypedCallEmissionPlan::DirectCallable { function_guard } => {
                    if !call.keywords.is_empty() {
                        self.error = Some(
                            "typed direct callable emission does not support keyword args"
                                .to_string(),
                        );
                        return;
                    }
                    *expr = InstrTyped::DirectCallableCallTyped(
                        TypedDirectCallableCall::from_typed_call(
                            call,
                            TypedDirectCallableCallGuard::Function(function_guard.clone()),
                        ),
                    );
                    self.count += 1;
                }
                TypedCallEmissionPlan::Method {
                    method_name,
                    method_guards,
                } => {
                    if !matches!(call.func.as_ref(), InstrTyped::GetAttrTyped(_)) {
                        *expr = InstrTyped::CallTyped(call);
                        return;
                    }
                    *expr = InstrTyped::GuardedMethodCallTyped(
                        TypedGuardedMethodCall::from_typed_call(
                            call,
                            method_name.clone(),
                            method_guards.clone(),
                        ),
                    );
                    self.count += 1;
                }
                TypedCallEmissionPlan::RuntimeProtocolMethod {
                    runtime_name,
                    method_name,
                    method_guards,
                } => {
                    let mut call = call;
                    call.access = TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                        runtime_name: *runtime_name,
                        method_name: method_name.clone(),
                        method_guards: method_guards.clone(),
                    };
                    *expr = InstrTyped::CallTyped(call);
                    self.count += 1;
                }
            }
        }
    }

    let mut rewriter = Rewriter {
        plans,
        count: 0,
        error: None,
    };
    rewriter.visit_fn_mut(function);
    if let Some(err) = rewriter.error {
        return Err(err);
    }
    Ok(rewriter.count)
}

pub fn lower_typed_function_call_access_plan_instrs(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> usize {
    struct Rewriter {
        count: usize,
    }

    impl VisitMut<InstrTyped> for Rewriter {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);
            let InstrTyped::CallTyped(call) = expr else {
                return;
            };
            let should_lower = matches!(
                call.access,
                TypedCallAccessPlan::GuardedCallable { .. }
                    | TypedCallAccessPlan::GuardedMethod { .. }
            );
            if !should_lower {
                return;
            }
            let old_expr = std::mem::replace(expr, InstrTyped::constant_none());
            let InstrTyped::CallTyped(mut call) = old_expr else {
                unreachable!("checked call shape before replacing typed instruction")
            };
            match std::mem::replace(&mut call.access, TypedCallAccessPlan::Generic) {
                TypedCallAccessPlan::GuardedCallable { function_guards } => {
                    *expr = InstrTyped::GuardedCallableCallTyped(
                        TypedGuardedCallableCall::from_typed_call(call, function_guards),
                    );
                }
                TypedCallAccessPlan::GuardedMethod {
                    method_name,
                    method_guards,
                } => {
                    *expr = InstrTyped::GuardedMethodCallTyped(
                        TypedGuardedMethodCall::from_typed_call(call, method_name, method_guards),
                    );
                }
                _ => unreachable!("checked guarded call access before replacing typed instruction"),
            };
            self.count += 1;
        }
    }

    let mut rewriter = Rewriter { count: 0 };
    rewriter.visit_fn_mut(function);
    rewriter.count
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct TypedInlineLocalMapping {
    pub callee: RuntimeFunctionId,
    pub inline_instance: u32,
    pub callee_location: LocalLocation,
    pub callee_name: String,
    pub caller_location: LocalLocation,
    pub caller_name: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TypedInlineInstrIdMapping {
    pub callee: RuntimeFunctionId,
    pub inline_instance: u32,
    pub callee_instr_id: InstrId,
    pub caller_instr_id: InstrId,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TypedInlineConstantMapping {
    pub callee: RuntimeFunctionId,
    pub inline_instance: u32,
    pub callee_index: u32,
    pub caller_index: u32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TypedInlineInstanceSource {
    pub inline_instance: u32,
    pub source_instr_id: InstrId,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TypedInlineSyntheticInstrId {
    pub inline_instance: u32,
    pub instr_id: InstrId,
}

#[derive(Debug, Clone)]
pub struct TypedInlineMaterializedGeneratorArg {
    pub generator_origin: InstrId,
    pub target: ResolvedName,
    pub call: TypedCall<InstrTyped>,
    pub closure_cell_bindings: Option<HashMap<u32, CellLocation>>,
}

#[derive(Debug, Clone, Default)]
pub struct TypedInlineRewriteStats {
    pub rewritten_stores: usize,
    pub rewritten_effect_only_calls: usize,
    pub rewritten_returns: usize,
    pub skipped_candidates: usize,
    pub skipped_exception_edges: usize,
    pub inline_instance_sources: Vec<TypedInlineInstanceSource>,
    pub materialized_generator_args: Vec<TypedInlineMaterializedGeneratorArg>,
    pub instr_id_mappings: Vec<TypedInlineInstrIdMapping>,
    pub synthetic_instr_ids: Vec<TypedInlineSyntheticInstrId>,
    pub constant_mappings: Vec<TypedInlineConstantMapping>,
    pub local_mappings: Vec<TypedInlineLocalMapping>,
    pub hot_state_cleanup_labels: Vec<BlockLabel>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedNestedBuiltinImplementationHoistStats {
    pub hoisted_calls: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TypedHoistBoundLocation {
    Local(LocalLocation),
    Preserved(PreservedLocation),
}

pub fn hoist_typed_nested_builtin_implementation_calls(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> TypedNestedBuiltinImplementationHoistStats {
    let mut stats = TypedNestedBuiltinImplementationHoistStats::default();
    let must_bound_ins = compute_typed_function_local_must_bound_ins(function);
    let preserved_must_bound_ins = compute_typed_function_preserved_must_bound_ins(function);
    let original_blocks = std::mem::take(&mut function.blocks);
    let mut rewritten_blocks = Vec::with_capacity(original_blocks.len());
    for mut block in original_blocks {
        let mut bound_locations = must_bound_ins
            .get(&block.label)
            .cloned()
            .unwrap_or_default();
        let mut bound_locations = bound_locations
            .drain()
            .map(TypedHoistBoundLocation::Local)
            .collect::<HashSet<_>>();
        bound_locations.extend(
            preserved_must_bound_ins
                .get(&block.label)
                .into_iter()
                .flatten()
                .copied()
                .map(TypedHoistBoundLocation::Preserved),
        );
        let original_body = std::mem::take(&mut block.body);
        let mut rewritten_body = Vec::with_capacity(original_body.len());
        for mut instr in original_body {
            let InstrTyped::Store(store) = &mut instr else {
                update_typed_hoist_bound_locations(&instr, &mut bound_locations);
                rewritten_body.push(instr);
                continue;
            };

            let mut hoisted = Vec::new();
            let mut available_bound_locations = bound_locations.clone();
            while can_replace_first_nested_builtin_implementation_call(
                store.value.as_ref(),
                true,
                &available_bound_locations,
            ) {
                let temp = match try_allocate_typed_stack_temp(
                    function,
                    "typed_nested_builtin_implementation",
                ) {
                    Ok(temp) => temp,
                    Err(_) => break,
                };
                let replacement = typed_load_temp(&temp.resolved_name());
                let nested_call = replace_first_nested_builtin_implementation_call(
                    store.value.as_mut(),
                    &replacement,
                    true,
                    &available_bound_locations,
                )
                .expect("hoistability check should match the nested builtin implementation call");
                let temp_name = temp.resolved_name();
                available_bound_locations.insert(TypedHoistBoundLocation::Local(temp.location));
                hoisted.push((
                    temp,
                    Store::new(temp_name, nested_call)
                        .with_meta(Meta::synthetic())
                        .into(),
                ));
                stats.hoisted_calls += 1;
            }

            let hoisted_temps = hoisted
                .iter()
                .map(|(temp, _)| temp.clone())
                .collect::<Vec<_>>();
            rewritten_body.extend(hoisted.into_iter().map(|(_, instr)| instr));
            update_typed_hoist_bound_locations(&instr, &mut bound_locations);
            rewritten_body.push(instr);
            append_typed_cleanup_dels_to_body(&mut rewritten_body, &hoisted_temps);
        }
        block.body = rewritten_body;
        rewritten_blocks.push(block);
    }
    function.blocks = rewritten_blocks;
    stats
}

fn compute_typed_function_preserved_must_bound_ins(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, HashSet<PreservedLocation>> {
    let labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<Vec<_>>();
    let entry = function.entry_block().label;
    let predecessors = typed_block_predecessors(function);
    let universe = function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|instr| match instr {
            InstrTyped::Load(load) => load.name.preserved_location(),
            InstrTyped::Store(store) => store.name.preserved_location(),
            InstrTyped::Del(del) => del.name.preserved_location(),
            _ => None,
        })
        .collect::<HashSet<_>>();

    let mut must_bound_in = labels
        .iter()
        .copied()
        .map(|label| {
            (
                label,
                if label == entry {
                    HashSet::new()
                } else {
                    universe.clone()
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let mut must_bound_out = labels
        .iter()
        .copied()
        .map(|label| {
            let block = function
                .blocks
                .iter()
                .find(|block| block.label == label)
                .expect("preserved must-bound block should exist");
            let incoming = must_bound_in
                .get(&label)
                .expect("preserved must-bound input should exist");
            (
                label,
                transfer_typed_preserved_must_bound_through_block(block, incoming),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut changed = true;
    while changed {
        changed = false;
        for block in &function.blocks {
            let new_in = if block.label == entry {
                HashSet::new()
            } else {
                let block_predecessors =
                    predecessors.get(&block.label).cloned().unwrap_or_default();
                let mut block_predecessors = block_predecessors.into_iter();
                match block_predecessors.next() {
                    Some(first) => {
                        let mut intersection =
                            must_bound_out.get(&first).cloned().unwrap_or_default();
                        for predecessor in block_predecessors {
                            let predecessor_out = must_bound_out
                                .get(&predecessor)
                                .expect("preserved must-bound predecessor output should exist");
                            intersection.retain(|location| predecessor_out.contains(location));
                        }
                        intersection
                    }
                    None => HashSet::new(),
                }
            };
            let new_out = transfer_typed_preserved_must_bound_through_block(block, &new_in);
            let in_entry = must_bound_in
                .get_mut(&block.label)
                .expect("preserved must-bound input should exist");
            if *in_entry != new_in {
                *in_entry = new_in;
                changed = true;
            }
            let out_entry = must_bound_out
                .get_mut(&block.label)
                .expect("preserved must-bound output should exist");
            if *out_entry != new_out {
                *out_entry = new_out;
                changed = true;
            }
        }
    }

    must_bound_in
}

fn transfer_typed_preserved_must_bound_through_block(
    block: &TypedBlock,
    incoming: &HashSet<PreservedLocation>,
) -> HashSet<PreservedLocation> {
    let mut bound = incoming.clone();
    for instr in &block.body {
        match instr {
            InstrTyped::Store(store) => {
                if let Some(location) = store.name.preserved_location() {
                    bound.insert(location);
                }
            }
            InstrTyped::Del(del) => {
                if let Some(location) = del.name.preserved_location() {
                    bound.remove(&location);
                }
            }
            _ => {}
        }
    }
    bound
}

fn update_typed_hoist_bound_locations(
    instr: &InstrTyped,
    bound_locations: &mut HashSet<TypedHoistBoundLocation>,
) {
    match instr {
        InstrTyped::Store(store) => {
            if let Some(location) = store.name.location.as_local() {
                bound_locations.insert(TypedHoistBoundLocation::Local(location));
            }
            if let Some(location) = store.name.preserved_location() {
                bound_locations.insert(TypedHoistBoundLocation::Preserved(location));
            }
        }
        InstrTyped::Del(del) => {
            if let Some(location) = del.name.location.as_local() {
                bound_locations.remove(&TypedHoistBoundLocation::Local(location));
            }
            if let Some(location) = del.name.preserved_location() {
                bound_locations.remove(&TypedHoistBoundLocation::Preserved(location));
            }
        }
        _ => {}
    }
}

fn can_replace_first_nested_builtin_implementation_call(
    expr: &InstrTyped,
    is_root: bool,
    bound_locations: &HashSet<TypedHoistBoundLocation>,
) -> bool {
    if !is_root && expr.builtin_implementation_plan().is_some() {
        return true;
    }

    match expr {
        InstrTyped::Truthy(op) => {
            can_replace_first_nested_builtin_implementation_call(op.value(), false, bound_locations)
        }
        InstrTyped::UnaryOp(op) => can_replace_first_nested_builtin_implementation_call(
            op.operand.as_ref(),
            false,
            bound_locations,
        ),
        InstrTyped::BinOp(op) => {
            can_replace_first_nested_builtin_implementation_call(
                op.left.as_ref(),
                false,
                bound_locations,
            ) || (typed_expr_is_hoist_safe_prefix(op.left.as_ref(), bound_locations)
                && can_replace_first_nested_builtin_implementation_call(
                    op.right.as_ref(),
                    false,
                    bound_locations,
                ))
        }
        InstrTyped::Tuple(tuple) => can_replace_first_nested_builtin_implementation_call_in_order(
            tuple.values.iter(),
            bound_locations,
        ),
        InstrTyped::CallTyped(call) => {
            can_replace_first_nested_builtin_implementation_call_in_call(
                call.func.as_ref(),
                call.args.iter().map(CallArgPositional::expr),
                call.keywords.iter().map(CallArgKeyword::expr),
                bound_locations,
            )
        }
        InstrTyped::GuardedCallableCallTyped(call) => {
            can_replace_first_nested_builtin_implementation_call_in_call(
                call.func.as_ref(),
                call.args.iter().map(CallArgPositional::expr),
                call.keywords.iter().map(CallArgKeyword::expr),
                bound_locations,
            )
        }
        InstrTyped::DirectCallableCallTyped(call) => {
            can_replace_first_nested_builtin_implementation_call_in_call(
                call.func.as_ref(),
                call.args.iter().map(CallArgPositional::expr),
                std::iter::empty::<&InstrTyped>(),
                bound_locations,
            )
        }
        InstrTyped::GetAttrTyped(op) => {
            can_replace_first_nested_builtin_implementation_call(
                op.value.as_ref(),
                false,
                bound_locations,
            ) || (typed_expr_is_hoist_safe_prefix(op.value.as_ref(), bound_locations)
                && can_replace_first_nested_builtin_implementation_call(
                    op.attr.as_ref(),
                    false,
                    bound_locations,
                ))
        }
        InstrTyped::GetItem(op) => {
            can_replace_first_nested_builtin_implementation_call(
                op.value.as_ref(),
                false,
                bound_locations,
            ) || (typed_expr_is_hoist_safe_prefix(op.value.as_ref(), bound_locations)
                && can_replace_first_nested_builtin_implementation_call(
                    op.index.as_ref(),
                    false,
                    bound_locations,
                ))
        }
        _ => false,
    }
}

fn can_replace_first_nested_builtin_implementation_call_in_call<'a, A, K>(
    func: &'a InstrTyped,
    args: A,
    keywords: K,
    bound_locations: &HashSet<TypedHoistBoundLocation>,
) -> bool
where
    A: IntoIterator<Item = &'a InstrTyped>,
    K: IntoIterator<Item = &'a InstrTyped>,
{
    can_replace_first_nested_builtin_implementation_call(func, false, bound_locations)
        || (typed_expr_is_hoist_safe_prefix(func, bound_locations)
            && (can_replace_first_nested_builtin_implementation_call_in_order(
                args,
                bound_locations,
            ) || can_replace_first_nested_builtin_implementation_call_in_order(
                keywords,
                bound_locations,
            )))
}

fn can_replace_first_nested_builtin_implementation_call_in_order<'a, I>(
    exprs: I,
    bound_locations: &HashSet<TypedHoistBoundLocation>,
) -> bool
where
    I: IntoIterator<Item = &'a InstrTyped>,
{
    for expr in exprs {
        if can_replace_first_nested_builtin_implementation_call(expr, false, bound_locations) {
            return true;
        }
        if !typed_expr_is_hoist_safe_prefix(expr, bound_locations) {
            return false;
        }
    }
    false
}

fn replace_first_nested_builtin_implementation_call(
    expr: &mut InstrTyped,
    replacement: &InstrTyped,
    is_root: bool,
    bound_locations: &HashSet<TypedHoistBoundLocation>,
) -> Option<InstrTyped> {
    if !is_root && expr.builtin_implementation_plan().is_some() {
        let nested_call = expr.clone();
        *expr = replacement.clone();
        return Some(nested_call);
    }

    match expr {
        InstrTyped::Truthy(op) => replace_first_nested_builtin_implementation_call(
            &mut op.value,
            replacement,
            false,
            bound_locations,
        ),
        InstrTyped::UnaryOp(op) => replace_first_nested_builtin_implementation_call(
            &mut op.operand,
            replacement,
            false,
            bound_locations,
        ),
        InstrTyped::BinOp(op) => replace_first_nested_builtin_implementation_call(
            &mut op.left,
            replacement,
            false,
            bound_locations,
        )
        .or_else(|| {
            typed_expr_is_hoist_safe_prefix(op.left.as_ref(), bound_locations).then(|| {
                replace_first_nested_builtin_implementation_call(
                    &mut op.right,
                    replacement,
                    false,
                    bound_locations,
                )
            })?
        }),
        InstrTyped::Tuple(tuple) => replace_first_nested_builtin_implementation_call_in_order(
            tuple.values.iter_mut(),
            replacement,
            bound_locations,
        ),
        InstrTyped::CallTyped(call) => replace_first_nested_builtin_implementation_call_in_call(
            &mut call.func,
            call.args.iter_mut(),
            call.keywords.iter_mut(),
            replacement,
            bound_locations,
        ),
        InstrTyped::GuardedCallableCallTyped(call) => {
            replace_first_nested_builtin_implementation_call_in_call(
                &mut call.func,
                call.args.iter_mut(),
                call.keywords.iter_mut(),
                replacement,
                bound_locations,
            )
        }
        InstrTyped::DirectCallableCallTyped(call) => {
            replace_first_nested_builtin_implementation_call_in_call(
                &mut call.func,
                call.args.iter_mut(),
                std::iter::empty::<&mut CallArgKeyword<InstrTyped>>(),
                replacement,
                bound_locations,
            )
        }
        InstrTyped::GetAttrTyped(op) => replace_first_nested_builtin_implementation_call(
            &mut op.value,
            replacement,
            false,
            bound_locations,
        )
        .or_else(|| {
            typed_expr_is_hoist_safe_prefix(op.value.as_ref(), bound_locations).then(|| {
                replace_first_nested_builtin_implementation_call(
                    &mut op.attr,
                    replacement,
                    false,
                    bound_locations,
                )
            })?
        }),
        InstrTyped::GetItem(op) => replace_first_nested_builtin_implementation_call(
            &mut op.value,
            replacement,
            false,
            bound_locations,
        )
        .or_else(|| {
            typed_expr_is_hoist_safe_prefix(op.value.as_ref(), bound_locations).then(|| {
                replace_first_nested_builtin_implementation_call(
                    &mut op.index,
                    replacement,
                    false,
                    bound_locations,
                )
            })?
        }),
        _ => None,
    }
}

fn replace_first_nested_builtin_implementation_call_in_call<'a, A, K>(
    func: &mut Box<InstrTyped>,
    args: A,
    keywords: K,
    replacement: &InstrTyped,
    bound_locations: &HashSet<TypedHoistBoundLocation>,
) -> Option<InstrTyped>
where
    A: Iterator<Item = &'a mut CallArgPositional<InstrTyped>>,
    K: Iterator<Item = &'a mut CallArgKeyword<InstrTyped>>,
{
    if let Some(nested_call) =
        replace_first_nested_builtin_implementation_call(func, replacement, false, bound_locations)
    {
        return Some(nested_call);
    }
    if !typed_expr_is_hoist_safe_prefix(func.as_ref(), bound_locations) {
        return None;
    }
    if let Some(nested_call) = replace_first_nested_builtin_implementation_call_in_order(
        args.map(CallArgPositional::expr_mut),
        replacement,
        bound_locations,
    ) {
        return Some(nested_call);
    }
    replace_first_nested_builtin_implementation_call_in_order(
        keywords.map(CallArgKeyword::expr_mut),
        replacement,
        bound_locations,
    )
}

fn replace_first_nested_builtin_implementation_call_in_order<'a, I>(
    exprs: I,
    replacement: &InstrTyped,
    bound_locations: &HashSet<TypedHoistBoundLocation>,
) -> Option<InstrTyped>
where
    I: IntoIterator<Item = &'a mut InstrTyped>,
{
    let mut exprs = exprs.into_iter();
    while let Some(expr) = exprs.next() {
        if let Some(nested_call) = replace_first_nested_builtin_implementation_call(
            expr,
            replacement,
            false,
            bound_locations,
        ) {
            return Some(nested_call);
        }
        if !typed_expr_is_hoist_safe_prefix(expr, bound_locations) {
            return None;
        }
    }
    None
}

fn typed_expr_is_hoist_safe_prefix(
    expr: &InstrTyped,
    bound_locations: &HashSet<TypedHoistBoundLocation>,
) -> bool {
    matches!(
        expr,
        InstrTyped::Load(load)
            if matches!(
                load.name.location,
                NameLocation::RuntimeName(_) | NameLocation::Constant(_)
            ) || load
                .name
                .location
                .as_local()
                .is_some_and(|location| {
                    bound_locations.contains(&TypedHoistBoundLocation::Local(location))
                })
                || load
                    .name
                    .preserved_location()
                    .is_some_and(|location| {
                        bound_locations
                            .contains(&TypedHoistBoundLocation::Preserved(location))
                    })
    ) || matches!(expr, InstrTyped::CalleeFunctionId(_))
}

#[derive(Debug, Clone, Default)]
pub struct TypedConstructorInitBodyInlineStats {
    pub inline_stats: TypedInlineRewriteStats,
    pub inlined_constructor_init_calls: Vec<InstrId>,
    pub constructor_field_bindings: HashMap<InstrId, TypedConstructorFieldBindings>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TypedHotContinuationClone {
    pub hot_block: BlockLabel,
    pub original_entry: BlockLabel,
    pub cloned_entry: BlockLabel,
    pub cloned_blocks: usize,
    pub cyclic_hot_region: bool,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedHotContinuationSplitStats {
    pub cloned_blocks: usize,
    pub clones: Vec<TypedHotContinuationClone>,
    pub instr_id_mappings: Vec<TypedInlineInstrIdMapping>,
    pub label_mappings: Vec<(BlockLabel, BlockLabel)>,
    pub alias_store_instr_ids: HashSet<InstrId>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TypedConstructorFieldBinding {
    pub field_name: String,
    pub value: ResolvedName,
    pub scalar: Option<ResolvedName>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TypedConstructorFieldBindings {
    pub fields: Vec<TypedConstructorFieldBinding>,
}

#[derive(Debug, Clone)]
pub struct TypedGeneratorStateLoweringPlan {
    pub generator_origin: InstrId,
    pub function_id: RuntimeFunctionId,
    pub body_instr_ids: HashSet<InstrId>,
    pub pending_alias_use_instr_ids: HashSet<InstrId>,
    pub alias_cleanup_active_blocks: Option<HashSet<BlockLabel>>,
    pub materialized_constructor: Option<TypedGeneratorStateConstructor>,
}

#[derive(Debug, Clone)]
pub struct TypedGeneratorStateConstructor {
    pub target: ResolvedName,
    pub call: TypedCall<InstrTyped>,
    pub closure_cell_bindings: Option<HashMap<u32, CellLocation>>,
}

fn typed_generator_state_constructor_call(expr: &InstrTyped) -> Option<TypedCall<InstrTyped>> {
    match expr {
        InstrTyped::CallTyped(call) if call.extra.generator_instance_plan().is_some() => {
            Some(call.clone())
        }
        InstrTyped::GuardedCallableCallTyped(call)
            if call.extra.generator_instance_plan().is_some() =>
        {
            Some(call.clone().into_typed_call())
        }
        InstrTyped::DirectCallableCallTyped(call)
            if call.extra.generator_instance_plan().is_some() =>
        {
            let mut normalized = TypedCall::generic(
                call.func.clone(),
                call.args.clone(),
                Vec::<CallArgKeyword<InstrTyped>>::new(),
            )
            .with_meta(call.meta());
            normalized.extra = call.extra.clone();
            Some(normalized)
        }
        _ => None,
    }
}

pub fn typed_generator_constructor_capture_bindings_by_origin(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<InstrId, HashMap<u32, CellLocation>> {
    struct Collector<'a> {
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        bindings_by_origin: HashMap<InstrId, HashMap<u32, CellLocation>>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(call) = typed_generator_state_constructor_call(expr)
                && let Some(generator_origin) = expr.try_semantic_instr_id()
                && let Some(plan) = call.extra.generator_instance_plan()
                && let Some(bindings) = typed_inline_generator_constructor_capture_bindings_snapshot(
                    self.function,
                    call.func.as_ref(),
                    plan.function_id,
                )
            {
                self.bindings_by_origin.insert(generator_origin, bindings);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        function,
        bindings_by_origin: HashMap::new(),
    };
    collector.visit_fn(function);
    collector.bindings_by_origin
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedGeneratorStateLoweringStats {
    pub lowered_generators: usize,
    pub initialized_slots: usize,
    pub remapped_instrs: usize,
    pub removed_owner_stores: usize,
}

impl TypedGeneratorStateLoweringStats {
    pub fn changed(&self) -> bool {
        self.lowered_generators != 0
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedGeneratorStateLoweringOutcome {
    pub stats: TypedGeneratorStateLoweringStats,
    pub preserved_locals_by_origin: HashMap<InstrId, HashMap<PreservedLocation, ResolvedName>>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedGeneratorResumeStateLoweringStats {
    pub lowered_functions: usize,
    pub lowered_slots: usize,
    pub entry_transfers: usize,
    pub boundary_writebacks: usize,
    pub remapped_instrs: usize,
}

impl TypedGeneratorResumeStateLoweringStats {
    pub fn changed(&self) -> bool {
        self.lowered_slots != 0
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedGeneratorResumeStateLoweringOutcome {
    pub stats: TypedGeneratorResumeStateLoweringStats,
    pub preserved_locals: HashMap<PreservedLocation, ResolvedName>,
}

#[derive(Debug, Clone)]
pub struct TypedExternalInlineCallee {
    pub function: BlockPyFunction<TypedBlockPyModuleShape>,
    pub module_constants: Vec<ConstantExpr>,
    pub inline_plan: Option<InlinePlanModule>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum TypedInlineUnsupportedReason {
    MissingCallerStorageLayout,
    MissingCalleeStorageLayout,
    MissingCalleeLocal(LocalLocation),
    MissingCalleeConstant(u32),
    MissingParameterLocal,
    UnsupportedValueBinding(LocalLocation),
    NonLocalValueBinding(LocalLocation),
    RebindsBoundLocal(LocalLocation),
    ArityMismatch,
    KeywordArguments,
    StarredArguments,
    DefaultArguments,
    UnsupportedParameterKind,
    TooManyBlocks,
    MultipleBlocks,
    UnknownLabel(BlockLabel),
    BlockParams,
    JumpArgs,
    ExceptionEdge,
    NonReturnTerm,
    NonStackStorage,
    CrossModuleGlobalName(String),
    UnknownBlockName(String),
    TooManyCallerConstants,
    PreservedOwnerConflict,
    UnsupportedGeneratorClosureCapture,
}

#[derive(Clone)]
struct TypedInlineDirectCallPlan {
    target: RuntimeFunctionId,
    arg_plan: TypedDirectCallArgPlan,
    guard: TypedInlineGuardPlan,
}

#[derive(Clone)]
enum TypedInlineGuardPlan {
    Direct,
    TrustedRuntime,
    Callable,
    Method(TypedDirectMethodCallGuard),
}

struct TypedInlineInstrIdAllocator {
    next_instr_index: u32,
    used: HashSet<InstrId>,
}

impl TypedInlineInstrIdAllocator {
    fn from_function(function: &BlockPyFunction<TypedBlockPyModuleShape>) -> Self {
        struct Collector {
            next_instr_index: u32,
            used: HashSet<InstrId>,
        }

        impl Visit<InstrTyped> for Collector {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let Some(instr_id) = expr.try_semantic_instr_id() {
                    self.used.insert(instr_id);
                    self.next_instr_index = self.next_instr_index.max(
                        instr_id
                            .index()
                            .checked_add(1)
                            .expect("per-function instruction count should fit in u32"),
                    );
                }
                expr.visit_children(self);
            }
        }

        let mut collector = Collector {
            next_instr_index: 0,
            used: HashSet::new(),
        };
        collector.visit_fn(function);
        Self {
            next_instr_index: collector.next_instr_index,
            used: collector.used,
        }
    }

    fn alloc(&mut self) -> InstrId {
        while self.used.contains(&InstrId::new(self.next_instr_index)) {
            self.next_instr_index = self
                .next_instr_index
                .checked_add(1)
                .expect("per-function instruction count should fit in u32");
        }
        let instr_id = InstrId::new(self.next_instr_index);
        self.used.insert(instr_id);
        self.next_instr_index = self
            .next_instr_index
            .checked_add(1)
            .expect("per-function instruction count should fit in u32");
        instr_id
    }
}

fn typed_instr_with_fresh_synthetic_instr_id(
    instr: InstrTyped,
    allocator: &mut TypedInlineInstrIdAllocator,
) -> InstrTyped {
    let mut meta = instr.meta();
    meta.instr_id = Some(allocator.alloc());
    instr.with_meta(meta)
}

struct TypedInlineInstrIdRemapper<'a> {
    callee: RuntimeFunctionId,
    inline_instance: u32,
    allocator: &'a mut TypedInlineInstrIdAllocator,
    assigned: HashMap<InstrId, InstrId>,
    mappings: Vec<TypedInlineInstrIdMapping>,
}

impl<'a> TypedInlineInstrIdRemapper<'a> {
    fn new(
        callee: RuntimeFunctionId,
        inline_instance: u32,
        allocator: &'a mut TypedInlineInstrIdAllocator,
    ) -> Self {
        Self {
            callee,
            inline_instance,
            allocator,
            assigned: HashMap::new(),
            mappings: Vec::new(),
        }
    }

    fn remap_instr_id(&mut self, instr: InstrTyped) -> InstrTyped {
        let Some(callee_instr_id) = instr.try_semantic_instr_id() else {
            return instr;
        };
        let caller_instr_id =
            if let Some(caller_instr_id) = self.assigned.get(&callee_instr_id).copied() {
                caller_instr_id
            } else {
                let caller_instr_id = self.allocator.alloc();
                self.assigned.insert(callee_instr_id, caller_instr_id);
                self.mappings.push(TypedInlineInstrIdMapping {
                    callee: self.callee,
                    inline_instance: self.inline_instance,
                    callee_instr_id,
                    caller_instr_id,
                });
                caller_instr_id
            };
        let mut meta = instr.meta();
        meta.instr_id = Some(caller_instr_id);
        instr.with_meta(meta)
    }

    fn assign_fresh_instr_id(&mut self, instr: InstrTyped) -> InstrTyped {
        let mut meta = instr.meta();
        meta.instr_id = Some(self.allocator.alloc());
        instr.with_meta(meta)
    }

    fn finish(self) -> Vec<TypedInlineInstrIdMapping> {
        self.mappings
    }
}

pub fn inline_typed_function_direct_call_stores(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, BlockPyFunction<TypedBlockPyModuleShape>>,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
) -> TypedInlineRewriteStats {
    inline_typed_function_direct_call_stores_impl(
        function,
        module,
        None,
        TypedInlineExternalCallees::Plain(external_callees),
        direct_calls_by_instr_id,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
    )
}

pub fn inline_typed_function_direct_call_stores_with_external_callees(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    caller_module_constants: &mut Vec<ConstantExpr>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
) -> TypedInlineRewriteStats {
    inline_typed_function_direct_call_stores_impl(
        function,
        module,
        Some(caller_module_constants),
        TypedInlineExternalCallees::Contextual(external_callees),
        direct_calls_by_instr_id,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
    )
}

pub fn inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    caller_module_constants: &mut Vec<ConstantExpr>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
    trusted_direct_method_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_direct_method_call_origin_candidates: &HashMap<InstrId, Vec<InstrId>>,
    trusted_direct_method_call_resume_functions: &HashMap<InstrId, RuntimeFunctionId>,
    materialized_generator_constructors: &HashMap<InstrId, TypedGeneratorStateConstructor>,
) -> TypedInlineRewriteStats {
    inline_typed_function_direct_call_stores_impl(
        function,
        module,
        Some(caller_module_constants),
        TypedInlineExternalCallees::Contextual(external_callees),
        direct_calls_by_instr_id,
        trusted_direct_method_calls,
        trusted_direct_method_call_origin_candidates,
        trusted_direct_method_call_resume_functions,
        materialized_generator_constructors,
    )
}

#[derive(Clone, Copy)]
enum TypedInlineExternalCallees<'a> {
    Plain(&'a HashMap<RuntimeFunctionId, BlockPyFunction<TypedBlockPyModuleShape>>),
    Contextual(&'a HashMap<RuntimeFunctionId, TypedExternalInlineCallee>),
}

fn inline_typed_function_direct_call_stores_impl(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    mut caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    external_callees: TypedInlineExternalCallees<'_>,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
    trusted_direct_method_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_direct_method_call_origin_candidates: &HashMap<InstrId, Vec<InstrId>>,
    trusted_direct_method_call_resume_functions: &HashMap<InstrId, RuntimeFunctionId>,
    materialized_generator_constructors: &HashMap<InstrId, TypedGeneratorStateConstructor>,
) -> TypedInlineRewriteStats {
    let mut stats = TypedInlineRewriteStats::default();
    let mut instr_id_allocator = TypedInlineInstrIdAllocator::from_function(function);
    let mut next_inline_instance = 0;
    let original_blocks = std::mem::take(&mut function.blocks);
    let mut rewritten_blocks = Vec::with_capacity(original_blocks.len());
    for block in original_blocks {
        match build_typed_direct_call_inline_rewrite(
            function,
            module,
            caller_module_constants.as_deref_mut(),
            external_callees,
            block,
            direct_calls_by_instr_id,
            trusted_direct_method_calls,
            trusted_direct_method_call_origin_candidates,
            trusted_direct_method_call_resume_functions,
            materialized_generator_constructors,
            &mut instr_id_allocator,
            &mut next_inline_instance,
            &mut stats,
        ) {
            TypedInlineBlockRewrite::Rewritten(blocks) => {
                rewritten_blocks.extend(blocks);
            }
            TypedInlineBlockRewrite::Unchanged(block) => rewritten_blocks.push(block),
        }
    }
    function.blocks = rewritten_blocks;
    stats
}

enum TypedInlineBlockRewrite {
    Rewritten(Vec<TypedBlock>),
    Unchanged(TypedBlock),
}

struct TypedInlineStoreCandidate {
    instr_index: usize,
    result: TypedInlineResult,
    call: TypedInlineCall,
    inline_plans: Vec<TypedInlineDirectCallPlan>,
}

#[derive(Clone)]
enum TypedInlineResult {
    StoreTo(ResolvedName),
    EffectOnly,
    Return,
}

#[derive(Clone)]
enum TypedInlineCall {
    DirectCallable(TypedDirectCallableCall<InstrTyped>),
    Callable(TypedGuardedCallableCall<InstrTyped>),
    BuiltinImplementation(TypedCall<InstrTyped>),
    Method {
        call: TypedGuardedMethodCall<InstrTyped>,
        receiver: InstrTyped,
        attr: InstrTyped,
    },
    DirectMethod {
        call: TypedCall<InstrTyped>,
        receiver: InstrTyped,
        linearized_func_temp: Option<ResolvedName>,
    },
    RuntimeProtocolMethod {
        call: TypedCall<InstrTyped>,
        receiver: InstrTyped,
    },
    DirectRuntimeProtocolMethod {
        call: TypedCall<InstrTyped>,
        receiver: InstrTyped,
    },
    GeneratorResume(TypedCall<InstrTyped>),
}

impl TypedInlineCall {
    fn meta(&self) -> Meta {
        match self {
            Self::DirectCallable(call) => call.meta(),
            Self::Callable(call) => call.meta(),
            Self::BuiltinImplementation(call) => call.meta(),
            Self::Method { call, .. } => call.meta(),
            Self::DirectMethod { call, .. } => call.meta(),
            Self::RuntimeProtocolMethod { call, .. } => call.meta(),
            Self::DirectRuntimeProtocolMethod { call, .. } => call.meta(),
            Self::GeneratorResume(call) => call.meta(),
        }
    }

    fn try_semantic_instr_id(&self) -> Option<InstrId> {
        match self {
            Self::DirectCallable(call) => call.try_semantic_instr_id(),
            Self::Callable(call) => call.try_semantic_instr_id(),
            Self::BuiltinImplementation(call) => call.try_semantic_instr_id(),
            Self::Method { call, .. } => call.try_semantic_instr_id(),
            Self::DirectMethod { call, .. } => call.try_semantic_instr_id(),
            Self::RuntimeProtocolMethod { call, .. } => call.try_semantic_instr_id(),
            Self::DirectRuntimeProtocolMethod { call, .. } => call.try_semantic_instr_id(),
            Self::GeneratorResume(call) => call.try_semantic_instr_id(),
        }
    }

    fn args(&self) -> Vec<CallArgPositional<InstrTyped>> {
        match self {
            Self::DirectCallable(call) => call.args.clone(),
            Self::Callable(call) => call.args.clone(),
            Self::BuiltinImplementation(call) => call.args.clone(),
            Self::Method { call, .. } => call.args.clone(),
            Self::DirectMethod { call, .. } => call.args.clone(),
            Self::RuntimeProtocolMethod { call, .. }
            | Self::DirectRuntimeProtocolMethod { call, .. } => {
                runtime_protocol_explicit_args(call)
                    .unwrap_or_default()
                    .to_vec()
            }
            Self::GeneratorResume(call) => call.args.clone(),
        }
    }

    fn keywords(&self) -> &[CallArgKeyword<InstrTyped>] {
        match self {
            Self::DirectCallable(_) => &[],
            Self::Callable(call) => call.keywords.as_slice(),
            Self::BuiltinImplementation(call) => call.keywords.as_slice(),
            Self::Method { call, .. } => call.keywords.as_slice(),
            Self::DirectMethod { call, .. } => call.keywords.as_slice(),
            Self::RuntimeProtocolMethod { call, .. } => call.keywords.as_slice(),
            Self::DirectRuntimeProtocolMethod { call, .. } => call.keywords.as_slice(),
            Self::GeneratorResume(call) => call.keywords.as_slice(),
        }
    }
}

fn trace_builtin_implementation_inline_skip(
    candidate: &TypedInlineStoreCandidate,
    reason: &'static str,
) {
    if matches!(candidate.call, TypedInlineCall::BuiltinImplementation(_)) {
        tracing::debug!(
            target: "soac_builtin_consumer_planning",
            source_instr_id = ?candidate.call.try_semantic_instr_id(),
            reason,
            "typed_builtin_generator_consumer_inline_skip",
        );
    } else if matches!(candidate.call, TypedInlineCall::GeneratorResume(_)) {
        tracing::info!(
            target: "soac_generator_state_lowering",
            source_instr_id = ?candidate.call.try_semantic_instr_id(),
            reason,
            "typed_generator_resume_inline_skip",
        );
    }
}

fn build_typed_direct_call_inline_rewrite(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    mut caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    external_callees: TypedInlineExternalCallees<'_>,
    block: TypedBlock,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
    trusted_direct_method_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_direct_method_call_origin_candidates: &HashMap<InstrId, Vec<InstrId>>,
    trusted_direct_method_call_resume_functions: &HashMap<InstrId, RuntimeFunctionId>,
    materialized_generator_constructors: &HashMap<InstrId, TypedGeneratorStateConstructor>,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    next_inline_instance: &mut u32,
    stats: &mut TypedInlineRewriteStats,
) -> TypedInlineBlockRewrite {
    let candidates = find_typed_inline_candidates(
        &block,
        caller.function_id,
        direct_calls_by_instr_id,
        trusted_direct_method_calls,
    );
    for candidate in candidates {
        match try_build_typed_direct_call_inline_rewrite_for_candidate(
            caller,
            module,
            caller_module_constants.as_deref_mut(),
            external_callees,
            block.clone(),
            candidate,
            trusted_direct_method_call_origin_candidates,
            trusted_direct_method_call_resume_functions,
            materialized_generator_constructors,
            instr_id_allocator,
            next_inline_instance,
            stats,
        ) {
            TypedInlineBlockRewrite::Rewritten(blocks) => {
                return TypedInlineBlockRewrite::Rewritten(blocks);
            }
            TypedInlineBlockRewrite::Unchanged(_) => {}
        }
    }
    TypedInlineBlockRewrite::Unchanged(block)
}

#[allow(clippy::too_many_arguments)]
fn try_build_typed_direct_call_inline_rewrite_for_candidate(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    mut caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    external_callees: TypedInlineExternalCallees<'_>,
    block: TypedBlock,
    candidate: TypedInlineStoreCandidate,
    trusted_direct_method_call_origin_candidates: &HashMap<InstrId, Vec<InstrId>>,
    trusted_direct_method_call_resume_functions: &HashMap<InstrId, RuntimeFunctionId>,
    materialized_generator_constructors: &HashMap<InstrId, TypedGeneratorStateConstructor>,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    next_inline_instance: &mut u32,
    stats: &mut TypedInlineRewriteStats,
) -> TypedInlineBlockRewrite {
    let original_block = block.clone();
    let original_storage_layout = caller.storage_layout.clone();
    let original_exc_edge = block.exc_edge.clone();
    if !candidate.call.keywords().is_empty() {
        stats.skipped_candidates += 1;
        trace_builtin_implementation_inline_skip(&candidate, "keywords");
        return TypedInlineBlockRewrite::Unchanged(block);
    }
    let Some(positional_arg_exprs) = typed_positional_arg_exprs(candidate.call.args()) else {
        stats.skipped_candidates += 1;
        trace_builtin_implementation_inline_skip(&candidate, "non_positional_args");
        return TypedInlineBlockRewrite::Unchanged(block);
    };
    tracing::info!(
        target: "soac_typed_inline_args",
        caller_function = ?caller.function_id,
        source_instr_id = ?candidate.call.try_semantic_instr_id(),
        inline_targets = ?candidate.inline_plans
            .iter()
            .map(|plan| plan.target)
            .collect::<Vec<_>>(),
        positional_arg_names = ?positional_arg_exprs
            .iter()
            .map(|expr| match expr {
                InstrTyped::Load(load) => load.name.id_str().to_string(),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>(),
        positional_arg_exprs = ?positional_arg_exprs,
        "typed_inline_candidate_args",
    );

    let receiver_temp = match &candidate.call {
        TypedInlineCall::DirectCallable(_)
        | TypedInlineCall::Callable(_)
        | TypedInlineCall::BuiltinImplementation(_)
        | TypedInlineCall::GeneratorResume(_) => None,
        TypedInlineCall::Method { .. }
        | TypedInlineCall::DirectMethod { .. }
        | TypedInlineCall::RuntimeProtocolMethod { .. }
        | TypedInlineCall::DirectRuntimeProtocolMethod { .. } => {
            match try_allocate_typed_stack_temp(caller, "typed_inline_receiver") {
                Ok(temp) => Some(temp),
                Err(_) => {
                    stats.skipped_candidates += 1;
                    trace_builtin_implementation_inline_skip(&candidate, "receiver_temp_alloc");
                    return TypedInlineBlockRewrite::Unchanged(block);
                }
            }
        }
    };
    let callable_temp = match &candidate.call {
        TypedInlineCall::DirectCallable(_) | TypedInlineCall::Callable(_) => {
            match try_allocate_typed_stack_temp(caller, "typed_inline_callable") {
                Ok(temp) => Some(temp),
                Err(_) => {
                    stats.skipped_candidates += 1;
                    trace_builtin_implementation_inline_skip(&candidate, "callable_temp_alloc");
                    caller.storage_layout = original_storage_layout;
                    return TypedInlineBlockRewrite::Unchanged(block);
                }
            }
        }
        TypedInlineCall::Method { .. }
        | TypedInlineCall::DirectMethod { .. }
        | TypedInlineCall::RuntimeProtocolMethod { .. }
        | TypedInlineCall::DirectRuntimeProtocolMethod { .. }
        | TypedInlineCall::BuiltinImplementation(_)
        | TypedInlineCall::GeneratorResume(_) => None,
    };
    let arg_temps = match (0..positional_arg_exprs.len())
        .map(|_| try_allocate_typed_stack_temp(caller, "typed_inline_arg"))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(temps) => temps,
        Err(_) => {
            stats.skipped_candidates += 1;
            trace_builtin_implementation_inline_skip(&candidate, "arg_temp_alloc");
            caller.storage_layout = original_storage_layout;
            return TypedInlineBlockRewrite::Unchanged(block);
        }
    };
    let (return_target, discard_result) = match &candidate.result {
        TypedInlineResult::StoreTo(target) => (target.clone(), None),
        TypedInlineResult::EffectOnly => {
            let result_temp = match try_allocate_typed_stack_temp(caller, "typed_inline_result") {
                Ok(temp) => temp,
                Err(_) => {
                    stats.skipped_candidates += 1;
                    trace_builtin_implementation_inline_skip(&candidate, "effect_temp_alloc");
                    caller.storage_layout = original_storage_layout;
                    return TypedInlineBlockRewrite::Unchanged(block);
                }
            };
            let return_target = result_temp.resolved_name();
            (return_target.clone(), Some(return_target))
        }
        TypedInlineResult::Return => {
            let result_temp = match try_allocate_typed_stack_temp(caller, "typed_inline_return") {
                Ok(temp) => temp,
                Err(_) => {
                    stats.skipped_candidates += 1;
                    trace_builtin_implementation_inline_skip(&candidate, "return_temp_alloc");
                    caller.storage_layout = original_storage_layout;
                    return TypedInlineBlockRewrite::Unchanged(block);
                }
            };
            (result_temp.resolved_name(), None)
        }
    };
    let continuation_label = caller.name_gen.next_block_name();
    let has_trusted_runtime_target = matches!(&candidate.call, TypedInlineCall::DirectCallable(_))
        && candidate
            .inline_plans
            .iter()
            .all(|plan| matches!(plan.guard, TypedInlineGuardPlan::TrustedRuntime));
    let has_generic_fallback = !has_trusted_runtime_target
        && !matches!(
            candidate.call,
            TypedInlineCall::BuiltinImplementation(_)
                | TypedInlineCall::DirectMethod { .. }
                | TypedInlineCall::DirectRuntimeProtocolMethod { .. }
                | TypedInlineCall::GeneratorResume(_)
        );
    let generic_label = has_generic_fallback.then(|| caller.name_gen.next_block_name());
    let cleanup_label = caller.name_gen.next_block_name();
    let guard_labels = (0..if has_generic_fallback {
        candidate.inline_plans.len().saturating_sub(1)
    } else {
        0
    })
        .map(|_| caller.name_gen.next_block_name())
        .collect::<Vec<_>>();
    let hot_labels = candidate
        .inline_plans
        .iter()
        .map(|_| caller.name_gen.next_block_name())
        .collect::<Vec<_>>();
    let mut instr_id_mappings = Vec::new();
    let mut local_mappings = Vec::new();
    let mut extra_cleanup_temps = Vec::new();
    let original_caller_module_constants = caller_module_constants
        .as_deref()
        .map(|constants| constants.to_vec());
    let mut cleanup_carries_hot_state = receiver_temp.is_some();

    let return_candidate = matches!(&candidate.result, TypedInlineResult::Return);
    let mut before = block.body;
    let (after, continuation_term) = if return_candidate {
        (
            Vec::new(),
            BlockTerm::Return(typed_load_temp(&return_target)),
        )
    } else {
        let after = before.split_off(candidate.instr_index + 1);
        before.truncate(candidate.instr_index);
        (after, block.term)
    };
    if let TypedInlineCall::DirectMethod {
        linearized_func_temp: Some(linearized_func_temp),
        ..
    } = &candidate.call
        && let Some(store_index) = before.iter().rposition(|instr| {
            matches!(
                instr,
                InstrTyped::Store(store)
                    if store.name == *linearized_func_temp
                        && matches!(store.value.as_ref(), InstrTyped::GetAttrTyped(_))
            )
        })
    {
        before.remove(store_index);
    }
    match &candidate.call {
        TypedInlineCall::DirectCallable(call) => {
            let callable_temp = callable_temp
                .as_ref()
                .expect("direct callable inline candidate should allocate callable temp");
            before.push(
                Store::new(callable_temp.resolved_name(), *call.func.clone())
                    .with_meta(Meta::synthetic())
                    .into(),
            );
        }
        TypedInlineCall::Callable(call) => {
            let callable_temp = callable_temp
                .as_ref()
                .expect("callable inline candidate should allocate callable temp");
            before.push(
                Store::new(callable_temp.resolved_name(), *call.func.clone())
                    .with_meta(Meta::synthetic())
                    .into(),
            );
        }
        TypedInlineCall::Method { receiver, .. }
        | TypedInlineCall::DirectMethod { receiver, .. }
        | TypedInlineCall::RuntimeProtocolMethod { receiver, .. }
        | TypedInlineCall::DirectRuntimeProtocolMethod { receiver, .. } => {
            let receiver_temp = receiver_temp
                .as_ref()
                .expect("method inline candidate should allocate receiver temp");
            let mut receiver = receiver.clone();
            if let Some(candidate_origins) = candidate
                .call
                .try_semantic_instr_id()
                .and_then(|instr_id| trusted_direct_method_call_origin_candidates.get(&instr_id))
                && let Some(extra) = receiver.typed_extra_mut()
            {
                extra.set_trusted_object_origin_candidates(candidate_origins.clone());
                if let Some(function_id) = candidate
                    .call
                    .try_semantic_instr_id()
                    .and_then(|instr_id| trusted_direct_method_call_resume_functions.get(&instr_id))
                {
                    extra.set_trusted_generator_resume_function(*function_id);
                }
            }
            before.push(
                Store::new(receiver_temp.resolved_name(), receiver)
                    .with_meta(Meta::synthetic())
                    .into(),
            );
        }
        TypedInlineCall::BuiltinImplementation(_) | TypedInlineCall::GeneratorResume(_) => {}
    }
    for (arg_temp, arg_expr) in arg_temps.iter().zip(positional_arg_exprs) {
        if let Some(call) = typed_generator_state_constructor_call(&arg_expr)
            && let Some(generator_origin) = arg_expr.try_semantic_instr_id()
        {
            let closure_cell_bindings = call.extra.generator_instance_plan().and_then(|plan| {
                typed_inline_generator_constructor_capture_bindings_snapshot(
                    caller,
                    call.func.as_ref(),
                    plan.function_id,
                )
            });
            tracing::info!(
                target: "soac_generator_state_lowering",
                generator_origin = ?generator_origin,
                target = ?arg_temp,
                has_closure_cell_bindings = closure_cell_bindings.is_some(),
                "typed_generator_state_constructor_snapshot_from_inline_arg",
            );
            stats
                .materialized_generator_args
                .push(TypedInlineMaterializedGeneratorArg {
                    generator_origin,
                    target: arg_temp.resolved_name(),
                    call,
                    closure_cell_bindings,
                });
        }
        before.push(
            Store::new(arg_temp.resolved_name(), arg_expr)
                .with_meta(Meta::synthetic())
                .into(),
        );
    }

    let entry_term = if has_trusted_runtime_target
        || matches!(
            candidate.call,
            TypedInlineCall::BuiltinImplementation(_)
                | TypedInlineCall::DirectMethod { .. }
                | TypedInlineCall::DirectRuntimeProtocolMethod { .. }
                | TypedInlineCall::GeneratorResume(_)
        ) {
        debug_assert_eq!(candidate.inline_plans.len(), 1);
        BlockTerm::Jump(BlockEdge::new(hot_labels[0]))
    } else {
        typed_inline_guard_term(
            &candidate.call,
            &candidate.inline_plans[0],
            callable_temp.as_ref(),
            receiver_temp.as_ref(),
            candidate.call.meta(),
            hot_labels[0],
            guard_labels
                .first()
                .copied()
                .or(generic_label)
                .expect("guarded inline candidate should have a fallback label"),
        )
    };
    let entry = Block::new_with_extra(
        block.label,
        before,
        entry_term,
        block.params,
        original_exc_edge.clone(),
        block.extra,
    );

    let mut blocks: Vec<TypedBlock> = Vec::new();
    blocks.push(entry);

    for (guard_index, guard_label) in guard_labels.iter().copied().enumerate() {
        let target_index = guard_index + 1;
        let else_label = guard_labels
            .get(guard_index + 1)
            .copied()
            .or(generic_label)
            .expect("guarded inline candidate should have a fallback label");
        blocks.push(Block::new_with_extra(
            guard_label,
            Vec::new(),
            typed_inline_guard_term(
                &candidate.call,
                &candidate.inline_plans[target_index],
                callable_temp.as_ref(),
                receiver_temp.as_ref(),
                candidate.call.meta(),
                hot_labels[target_index],
                else_label,
            ),
            Vec::new(),
            original_exc_edge.clone(),
            TypedBlockExtra::default(),
        ));
    }

    for (plan, hot_label) in candidate.inline_plans.iter().zip(hot_labels) {
        let Some(callee) = typed_inline_callee(module, external_callees, plan.target) else {
            stats.skipped_candidates += 1;
            trace_builtin_implementation_inline_skip(&candidate, "missing_callee");
            caller.storage_layout = original_storage_layout;
            return TypedInlineBlockRewrite::Unchanged(original_block);
        };
        let mut provided_values =
            typed_inline_provided_values(&candidate.call, &receiver_temp, &arg_temps);
        if is_constructor_entry_function(callee.function) {
            cleanup_carries_hot_state = true;
            let callable_temp = callable_temp
                .as_ref()
                .expect("constructor callable inline candidate should allocate callable temp");
            provided_values.insert(0, typed_load_temp(&callable_temp.resolved_name()));
        }
        let (bindings, preassigned_locals, prologue, preserved_abi_temps, closure_cell_bindings) =
            match &candidate.call {
                TypedInlineCall::GeneratorResume(_) => {
                    let bound = bind_typed_generator_resume_inline_values(
                        caller,
                        callee.function,
                        &plan.arg_plan,
                        provided_values.as_slice(),
                    );
                    let bound = match bound {
                        Ok(bound) => bound,
                        Err(_) => {
                            stats.skipped_candidates += 1;
                            trace_builtin_implementation_inline_skip(
                                &candidate,
                                "generator_resume_binding",
                            );
                            if let (Some(constants), Some(original)) = (
                                caller_module_constants.as_deref_mut(),
                                original_caller_module_constants.as_ref(),
                            ) {
                                *constants = original.clone();
                            }
                            caller.storage_layout = original_storage_layout;
                            return TypedInlineBlockRewrite::Unchanged(original_block);
                        }
                    };
                    let closure_cell_bindings = match typed_generator_resume_inline_closure_bindings(
                        caller,
                        callee.function,
                        match &candidate.call {
                            TypedInlineCall::GeneratorResume(call) => call,
                            _ => unreachable!(
                                "generator resume branch should retain the resume call"
                            ),
                        },
                        materialized_generator_constructors,
                    ) {
                        Ok(bindings) => bindings,
                        Err(_) => {
                            stats.skipped_candidates += 1;
                            trace_builtin_implementation_inline_skip(
                                &candidate,
                                "generator_resume_closure_bindings",
                            );
                            if let (Some(constants), Some(original)) = (
                                caller_module_constants.as_deref_mut(),
                                original_caller_module_constants.as_ref(),
                            ) {
                                *constants = original.clone();
                            }
                            caller.storage_layout = original_storage_layout;
                            return TypedInlineBlockRewrite::Unchanged(original_block);
                        }
                    };
                    (
                        bound.0,
                        bound.1,
                        bound.2,
                        bound.3,
                        Some(closure_cell_bindings),
                    )
                }
                _ => {
                    let bindings = match bind_typed_direct_call_inline_values(
                        callee.function,
                        &plan.arg_plan,
                        provided_values.as_slice(),
                    ) {
                        Ok(bindings) => bindings,
                        Err(_) => {
                            stats.skipped_candidates += 1;
                            trace_builtin_implementation_inline_skip(&candidate, "direct_binding");
                            if let (Some(constants), Some(original)) = (
                                caller_module_constants.as_deref_mut(),
                                original_caller_module_constants.as_ref(),
                            ) {
                                *constants = original.clone();
                            }
                            caller.storage_layout = original_storage_layout;
                            return TypedInlineBlockRewrite::Unchanged(original_block);
                        }
                    };
                    (bindings, HashMap::new(), Vec::new(), Vec::new(), None)
                }
            };
        tracing::info!(
            target: "soac_typed_inline_bindings",
            caller_function = ?caller.function_id,
            source_instr_id = ?candidate.call.try_semantic_instr_id(),
            callee_function = ?callee.function.function_id,
            callee_qualname = %callee.function.names.qualname,
            arg_plan = ?plan.arg_plan,
            provided_values = ?provided_values,
            bindings = ?bindings,
            "typed_inline_fragment_bindings",
        );
        let inline_instance = *next_inline_instance;
        *next_inline_instance = next_inline_instance
            .checked_add(1)
            .expect("typed inline instance count should fit in u32");
        if let Some(source_instr_id) = candidate.call.try_semantic_instr_id() {
            stats
                .inline_instance_sources
                .push(TypedInlineInstanceSource {
                    inline_instance,
                    source_instr_id,
                });
        }
        let mut fragment = match build_typed_direct_call_inline_fragment_to_target(
            caller,
            callee.function,
            cleanup_label,
            &bindings,
            &preassigned_locals,
            return_target.clone(),
            inline_instance,
            instr_id_allocator,
            caller_module_constants.as_deref_mut(),
            callee.module_constants,
            matches!(candidate.call, TypedInlineCall::GeneratorResume(_)),
            closure_cell_bindings.as_ref(),
        ) {
            Ok(fragment) => fragment,
            Err(error) => {
                stats.skipped_candidates += 1;
                trace_builtin_implementation_inline_skip(&candidate, "inline_fragment");
                if matches!(candidate.call, TypedInlineCall::BuiltinImplementation(_)) {
                    tracing::debug!(
                        target: "soac_builtin_consumer_planning",
                        source_instr_id = ?candidate.call.try_semantic_instr_id(),
                        error = ?error,
                        "typed_builtin_generator_consumer_inline_fragment_error",
                    );
                }
                caller.storage_layout = original_storage_layout;
                return TypedInlineBlockRewrite::Unchanged(original_block);
            }
        };
        for block in &mut fragment.blocks {
            if block.exc_edge.is_none() {
                block.exc_edge = original_exc_edge.clone();
            }
        }
        if let Some(entry) = fragment.blocks.first_mut() {
            entry.label = hot_label;
            if !prologue.is_empty() {
                entry.body.splice(0..0, prologue);
            }
        }
        extra_cleanup_temps.extend(preserved_abi_temps);
        instr_id_mappings.extend(fragment.instr_id_mappings);
        stats
            .synthetic_instr_ids
            .extend(fragment.synthetic_instr_ids.into_iter().map(|instr_id| {
                TypedInlineSyntheticInstrId {
                    inline_instance,
                    instr_id,
                }
            }));
        stats.constant_mappings.extend(fragment.constant_mappings);
        local_mappings.extend(fragment.local_mappings);
        blocks.extend(fragment.blocks);
    }

    if let Some(generic_label) = generic_label {
        blocks.push(Block::new_with_extra(
            generic_label,
            typed_inline_generic_fallback_body(
                &candidate.call,
                &return_target,
                callable_temp.as_ref(),
                receiver_temp.as_ref(),
                &arg_temps,
                discard_result.as_ref(),
            ),
            BlockTerm::Jump(BlockEdge::new(continuation_label)),
            Vec::new(),
            original_exc_edge.clone(),
            TypedBlockExtra::default(),
        ));
    }

    let mut cleanup_body = Vec::new();
    if let Some(discard_result) = &discard_result {
        append_typed_cleanup_del_to_body(&mut cleanup_body, discard_result);
    }
    append_typed_cleanup_dels_to_body(&mut cleanup_body, &arg_temps);
    if let Some(receiver_temp) = &receiver_temp {
        append_typed_cleanup_del_to_body(&mut cleanup_body, &receiver_temp.resolved_name());
    }
    if let Some(callable_temp) = &callable_temp {
        append_typed_cleanup_del_to_body(&mut cleanup_body, &callable_temp.resolved_name());
    }
    append_typed_cleanup_dels_to_body(&mut cleanup_body, &extra_cleanup_temps);
    blocks.push(Block::new_with_extra(
        cleanup_label,
        cleanup_body,
        BlockTerm::Jump(BlockEdge::new(continuation_label)),
        Vec::new(),
        original_exc_edge.clone(),
        TypedBlockExtra::default(),
    ));
    if cleanup_carries_hot_state {
        stats.hot_state_cleanup_labels.push(cleanup_label);
    }
    blocks.push(Block::new_with_extra(
        continuation_label,
        after,
        continuation_term,
        Vec::new(),
        original_exc_edge,
        TypedBlockExtra::default(),
    ));

    match candidate.result {
        TypedInlineResult::StoreTo(_) => stats.rewritten_stores += 1,
        TypedInlineResult::EffectOnly => stats.rewritten_effect_only_calls += 1,
        TypedInlineResult::Return => stats.rewritten_returns += 1,
    }
    stats.instr_id_mappings.extend(instr_id_mappings);
    stats.local_mappings.extend(local_mappings);
    TypedInlineBlockRewrite::Rewritten(blocks)
}

pub fn inline_typed_constructor_init_bodies_with_external_callees(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    caller_module_constants: &mut Vec<ConstantExpr>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    skip_constructor_call_ids: &HashSet<InstrId>,
) -> TypedConstructorInitBodyInlineStats {
    let mut stats = TypedConstructorInitBodyInlineStats::default();
    let mut instr_id_allocator = TypedInlineInstrIdAllocator::from_function(function);
    let mut next_inline_instance = 0;
    let original_blocks = std::mem::take(&mut function.blocks);
    let mut rewritten_blocks = Vec::with_capacity(original_blocks.len());
    for block in original_blocks {
        match build_typed_constructor_init_body_inline_rewrite(
            function,
            module,
            caller_module_constants,
            external_callees,
            skip_constructor_call_ids,
            block,
            &mut instr_id_allocator,
            &mut next_inline_instance,
            &mut stats,
        ) {
            TypedInlineBlockRewrite::Rewritten(blocks) => rewritten_blocks.extend(blocks),
            TypedInlineBlockRewrite::Unchanged(block) => rewritten_blocks.push(block),
        }
    }
    function.blocks = rewritten_blocks;
    stats
}

#[derive(Clone)]
struct TypedConstructorInitBodyCandidate {
    instr_index: usize,
    root: ResolvedName,
    args: Vec<CallArgPositional<InstrTyped>>,
    instr_id: InstrId,
    plan: TypedConstructorInitPlan,
}

#[allow(clippy::too_many_arguments)]
fn build_typed_constructor_init_body_inline_rewrite(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    caller_module_constants: &mut Vec<ConstantExpr>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    skip_constructor_call_ids: &HashSet<InstrId>,
    block: TypedBlock,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    next_inline_instance: &mut u32,
    stats: &mut TypedConstructorInitBodyInlineStats,
) -> TypedInlineBlockRewrite {
    let original_block = block.clone();
    let original_storage_layout = caller.storage_layout.clone();
    let original_caller_module_constants = caller_module_constants.clone();
    let Some(candidate) = find_typed_constructor_init_body_candidate(
        &block,
        caller.function_id,
        caller_module_constants,
        skip_constructor_call_ids,
    ) else {
        return TypedInlineBlockRewrite::Unchanged(block);
    };
    let Some(callee) = typed_inline_callee(
        module,
        TypedInlineExternalCallees::Contextual(external_callees),
        candidate.plan.init_function_id,
    ) else {
        stats.inline_stats.skipped_candidates += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    };
    let callee_module_constants = callee
        .module_constants
        .unwrap_or(module.module_constants.as_slice());
    if !typed_function_returns_only_none(callee.function, callee_module_constants) {
        stats.inline_stats.skipped_candidates += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    }
    let Some(positional_args) = typed_positional_arg_exprs(candidate.args.clone()) else {
        stats.inline_stats.skipped_candidates += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    };
    let Ok((bindings, prologue)) = bind_typed_constructor_init_body_inline_values(
        caller,
        callee.function,
        &candidate.root,
        &positional_args[1..],
    ) else {
        stats.inline_stats.skipped_candidates += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    };
    let Ok(return_temp) = try_allocate_typed_stack_temp(caller, "typed_inline_init_result") else {
        stats.inline_stats.skipped_candidates += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    };
    let continuation_label = caller.name_gen.next_block_name();
    let inline_entry_label = caller.name_gen.next_block_name();
    let original_exc_edge = block.exc_edge.clone();
    let mut before = block.body;
    let after = before.split_off(candidate.instr_index + 1);
    mark_typed_constructor_call_init_body_inlined(
        &mut before[candidate.instr_index],
        candidate.plan.init_function_id,
    );

    let inline_instance = *next_inline_instance;
    *next_inline_instance = next_inline_instance
        .checked_add(1)
        .expect("typed inline instance count should fit in u32");
    let Ok(mut fragment) = build_typed_direct_call_inline_fragment_to_target(
        caller,
        callee.function,
        continuation_label,
        &bindings,
        &HashMap::new(),
        return_temp.resolved_name(),
        inline_instance,
        instr_id_allocator,
        Some(caller_module_constants),
        Some(callee_module_constants),
        false,
        None,
    ) else {
        stats.inline_stats.skipped_candidates += 1;
        caller.storage_layout = original_storage_layout;
        *caller_module_constants = original_caller_module_constants;
        return TypedInlineBlockRewrite::Unchanged(original_block);
    };
    for block in &mut fragment.blocks {
        if block.exc_edge.is_none() {
            block.exc_edge = original_exc_edge.clone();
        }
    }
    if let Some(entry) = fragment.blocks.first_mut() {
        entry.label = inline_entry_label;
        if !prologue.is_empty() {
            entry.body.splice(0..0, prologue);
        }
    }
    let constructor_call_id = candidate.instr_id;
    if let Some(bindings) = typed_constructor_init_body_field_bindings(
        constructor_call_id,
        &candidate.root,
        fragment.blocks.as_slice(),
        caller_module_constants,
    ) {
        stats
            .constructor_field_bindings
            .insert(constructor_call_id, bindings);
    }

    let mut blocks = Vec::with_capacity(fragment.blocks.len() + 2);
    blocks.push(Block::new_with_extra(
        block.label,
        before,
        BlockTerm::Jump(BlockEdge::new(inline_entry_label)),
        block.params,
        original_exc_edge.clone(),
        block.extra,
    ));
    stats
        .inline_stats
        .instr_id_mappings
        .extend(fragment.instr_id_mappings);
    stats
        .inline_stats
        .constant_mappings
        .extend(fragment.constant_mappings);
    stats
        .inline_stats
        .local_mappings
        .extend(fragment.local_mappings);
    blocks.extend(fragment.blocks);
    let mut continuation_body = Vec::with_capacity(after.len() + 1);
    append_typed_cleanup_del_to_body(&mut continuation_body, &return_temp.resolved_name());
    continuation_body.extend(after);
    blocks.push(Block::new_with_extra(
        continuation_label,
        continuation_body,
        block.term,
        Vec::new(),
        original_exc_edge,
        TypedBlockExtra::default(),
    ));
    stats.inline_stats.rewritten_stores += 1;
    stats
        .inlined_constructor_init_calls
        .push(constructor_call_id);
    TypedInlineBlockRewrite::Rewritten(blocks)
}

fn find_typed_constructor_init_body_candidate(
    block: &TypedBlock,
    caller_function_id: RuntimeFunctionId,
    module_constants: &[ConstantExpr],
    skip_constructor_call_ids: &HashSet<InstrId>,
) -> Option<TypedConstructorInitBodyCandidate> {
    block
        .body
        .iter()
        .enumerate()
        .find_map(|(instr_index, instr)| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            if store.name.local_location().is_none() {
                return None;
            }
            let (func, args, instr_id, plan) =
                typed_constructor_init_body_call_parts(store.value.as_ref())?;
            if skip_constructor_call_ids.contains(&instr_id) {
                return None;
            }
            if plan.source != TypedConstructorInitPlanSource::InlinedConstructorEntry
                || plan.init_function_id == caller_function_id
                || !typed_expr_is_runtime_name_load(
                    func,
                    RuntimeName::ConstructorCall,
                    module_constants,
                )
            {
                return None;
            }
            let positional_args = typed_positional_arg_exprs(args.clone())?;
            if positional_args.is_empty()
                || !positional_args
                    .iter()
                    .all(|arg| typed_instr_local_load_location(arg).is_some())
            {
                return None;
            }
            Some(TypedConstructorInitBodyCandidate {
                instr_index,
                root: store.name.clone(),
                args: args.clone(),
                instr_id,
                plan,
            })
        })
}

fn typed_constructor_init_body_call_parts(
    expr: &InstrTyped,
) -> Option<(
    &InstrTyped,
    &Vec<CallArgPositional<InstrTyped>>,
    InstrId,
    TypedConstructorInitPlan,
)> {
    match expr {
        InstrTyped::CallTyped(call) if call.keywords.is_empty() => Some((
            call.func.as_ref(),
            &call.args,
            call.try_semantic_instr_id()?,
            call.extra.constructor_init_plan()?,
        )),
        InstrTyped::DirectCallableCallTyped(call) => Some((
            call.func.as_ref(),
            &call.args,
            call.try_semantic_instr_id()?,
            call.extra.constructor_init_plan()?,
        )),
        _ => None,
    }
}

fn typed_function_returns_only_none(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> bool {
    function.blocks.iter().all(|block| match &block.term {
        BlockTerm::Return(value) => typed_expr_is_known_none_value(value, module_constants),
        BlockTerm::Jump(_)
        | BlockTerm::IfTerm(_)
        | BlockTerm::BranchTable(_)
        | BlockTerm::Raise(_) => true,
    })
}

fn typed_expr_is_known_none_value(expr: &InstrTyped, module_constants: &[ConstantExpr]) -> bool {
    if typed_expr_is_runtime_name_load(expr, RuntimeName::None, module_constants) {
        return true;
    }
    if expr
        .result_facts()
        .and_then(|facts| facts.as_pyobj())
        .is_some_and(PyObjFacts::is_none)
    {
        return true;
    }
    let InstrTyped::Load(load) = expr else {
        return false;
    };
    if load.name.id_str() == "NONE"
        && matches!(
            load.name.location,
            NameLocation::RuntimeName(_) | NameLocation::GlobalName | NameLocation::Global(_)
        )
    {
        return true;
    }
    let Some(index) = load.name.location.as_constant() else {
        return false;
    };
    module_constants
        .get(index as usize)
        .is_some_and(|constant| matches!(constant, ConstantExpr::RuntimeName(RuntimeName::None)))
}

fn bind_typed_constructor_init_body_inline_values(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    init_function: &BlockPyFunction<TypedBlockPyModuleShape>,
    root: &ResolvedName,
    user_args: &[InstrTyped],
) -> Result<(TypedInlineValueBindings, Vec<InstrTyped>), TypedInlineUnsupportedReason> {
    let first_param = init_function
        .params
        .iter()
        .next()
        .ok_or(TypedInlineUnsupportedReason::ArityMismatch)?;
    if !matches!(first_param.kind, ParamKind::PosOnly | ParamKind::Any) {
        return Err(TypedInlineUnsupportedReason::UnsupportedParameterKind);
    }
    let mut bindings = TypedInlineValueBindings::new();
    let mut prologue = Vec::new();
    bindings.insert(
        typed_parameter_local_location(init_function, &first_param.name)?,
        typed_load_temp(root),
    );

    let mut next_user_arg = 0usize;
    let mut packed_rest = false;
    for param in init_function.params.iter().skip(1) {
        let value = match param.kind {
            ParamKind::PosOnly | ParamKind::Any => {
                if packed_rest {
                    return Err(TypedInlineUnsupportedReason::UnsupportedParameterKind);
                }
                let value = user_args
                    .get(next_user_arg)
                    .cloned()
                    .ok_or(TypedInlineUnsupportedReason::ArityMismatch)?;
                next_user_arg += 1;
                value
            }
            ParamKind::VarArg => {
                if packed_rest {
                    return Err(TypedInlineUnsupportedReason::UnsupportedParameterKind);
                }
                packed_rest = true;
                let rest = user_args
                    .get(next_user_arg..)
                    .ok_or(TypedInlineUnsupportedReason::ArityMismatch)?
                    .to_vec();
                next_user_arg = user_args.len();
                let temp = try_allocate_typed_stack_temp(caller, "typed_inline_varargs")
                    .map_err(|_| TypedInlineUnsupportedReason::MissingCallerStorageLayout)?;
                prologue.push(
                    Store::new(temp.resolved_name(), InstrTyped::Tuple(Tuple::new(rest)))
                        .with_meta(Meta::synthetic())
                        .into(),
                );
                typed_load_temp(&temp.resolved_name())
            }
            ParamKind::KwOnly | ParamKind::KwArg => {
                return Err(TypedInlineUnsupportedReason::UnsupportedParameterKind);
            }
        };
        bindings.insert(
            typed_parameter_local_location(init_function, &param.name)?,
            value,
        );
    }
    if next_user_arg != user_args.len() {
        return Err(TypedInlineUnsupportedReason::ArityMismatch);
    }
    Ok((bindings, prologue))
}

fn mark_typed_constructor_call_init_body_inlined(
    instr: &mut InstrTyped,
    init_function_id: RuntimeFunctionId,
) {
    let InstrTyped::Store(store) = instr else {
        return;
    };
    match store.value.as_mut() {
        InstrTyped::CallTyped(call) => {
            call.extra
                .set_constructor_init_plan(TypedConstructorInitPlan {
                    source:
                        TypedConstructorInitPlanSource::InlinedConstructorEntryWithInlinedInitBody,
                    init_function_id,
                });
        }
        InstrTyped::DirectCallableCallTyped(call) => {
            call.extra
                .set_constructor_init_plan(TypedConstructorInitPlan {
                    source:
                        TypedConstructorInitPlanSource::InlinedConstructorEntryWithInlinedInitBody,
                    init_function_id,
                });
        }
        _ => {}
    }
}

fn typed_constructor_init_body_field_bindings(
    constructor_call_id: InstrId,
    root: &ResolvedName,
    blocks: &[TypedBlock],
    module_constants: &[ConstantExpr],
) -> Option<TypedConstructorFieldBindings> {
    let mut fields = HashMap::<String, ResolvedName>::new();
    for block in blocks {
        for instr in &block.body {
            let InstrTyped::SetAttrTyped(op) = instr else {
                continue;
            };
            if !typed_expr_loads_resolved_name(op.value.as_ref(), root) {
                continue;
            }
            let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants) else {
                continue;
            };
            let Some(value) = typed_expr_local_load_name(op.replacement.as_ref()) else {
                continue;
            };
            if fields
                .insert(field_name.to_string(), value.clone())
                .is_some()
            {
                return None;
            }
        }
    }
    if fields.is_empty() {
        return None;
    }
    let mut fields = fields
        .into_iter()
        .map(|(field_name, value)| TypedConstructorFieldBinding {
            field_name,
            value,
            scalar: None,
        })
        .collect::<Vec<_>>();
    fields.sort_by(|left, right| left.field_name.cmp(&right.field_name));
    let _ = constructor_call_id;
    Some(TypedConstructorFieldBindings { fields })
}

fn typed_expr_loads_resolved_name(expr: &InstrTyped, name: &ResolvedName) -> bool {
    matches!(expr, InstrTyped::Load(load) if load.name == *name)
}

fn typed_expr_contains_resolved_name_load(expr: &InstrTyped, name: &ResolvedName) -> bool {
    struct Finder<'a> {
        name: &'a ResolvedName,
        found: bool,
    }

    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }
            if typed_expr_loads_resolved_name(expr, self.name) {
                self.found = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder { name, found: false };
    finder.visit_instr(expr);
    finder.found
}

fn typed_expr_local_load_name(expr: &InstrTyped) -> Option<&ResolvedName> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    load.name.local_location()?;
    Some(&load.name)
}

const MAX_TYPED_HOT_CONTINUATION_CLONE_BLOCKS: usize = 256;

pub fn split_typed_constructor_hot_continuations(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> TypedHotContinuationSplitStats {
    split_typed_constructor_hot_continuations_impl(function, module_constants, None)
}

pub fn split_typed_constructor_hot_continuations_with_budget(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    max_cloned_blocks: usize,
) -> TypedHotContinuationSplitStats {
    if max_cloned_blocks == 0 {
        return TypedHotContinuationSplitStats::default();
    }
    split_typed_constructor_hot_continuations_impl(
        function,
        module_constants,
        Some(max_cloned_blocks),
    )
}

fn split_typed_constructor_hot_continuations_impl(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    max_cloned_blocks: Option<usize>,
) -> TypedHotContinuationSplitStats {
    let mut stats = TypedHotContinuationSplitStats::default();
    let mut instr_id_allocator = TypedInlineInstrIdAllocator::from_function(function);
    loop {
        let Some(candidate) =
            find_typed_constructor_hot_continuation_split_candidate(function, module_constants)
        else {
            break;
        };
        if max_cloned_blocks
            .is_some_and(|max| stats.cloned_blocks + candidate.reachable.len() > max)
        {
            break;
        }
        let Some(cloned) = clone_typed_hot_continuation(
            function,
            candidate,
            stats.clones.len() as u32,
            &mut instr_id_allocator,
        ) else {
            break;
        };
        stats.cloned_blocks += cloned.clone.cloned_blocks;
        stats.instr_id_mappings.extend(cloned.instr_id_mappings);
        stats.label_mappings.extend(cloned.label_mappings);
        stats.clones.push(cloned.clone);
    }
    stats
}

pub fn split_typed_alias_hot_continuations(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> TypedHotContinuationSplitStats {
    split_typed_alias_hot_continuations_impl(function, module_constants, &HashSet::new(), None)
}

pub fn split_typed_alias_hot_continuations_with_budget(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    suppressed_alias_store_instr_ids: &HashSet<InstrId>,
    max_cloned_blocks: usize,
) -> TypedHotContinuationSplitStats {
    if max_cloned_blocks == 0 {
        return TypedHotContinuationSplitStats::default();
    }
    split_typed_alias_hot_continuations_impl(
        function,
        module_constants,
        suppressed_alias_store_instr_ids,
        Some(max_cloned_blocks),
    )
}

pub fn split_typed_generator_alias_hot_continuations(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> TypedHotContinuationSplitStats {
    split_typed_generator_alias_hot_continuations_impl(
        function,
        module_constants,
        &HashSet::new(),
        None,
    )
}

pub fn split_typed_generator_alias_hot_continuations_with_budget(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    suppressed_generator_alias_store_instr_ids: &HashSet<InstrId>,
    max_cloned_blocks: usize,
) -> TypedHotContinuationSplitStats {
    if max_cloned_blocks == 0 {
        return TypedHotContinuationSplitStats::default();
    }
    split_typed_generator_alias_hot_continuations_impl(
        function,
        module_constants,
        suppressed_generator_alias_store_instr_ids,
        Some(max_cloned_blocks),
    )
}

fn split_typed_generator_alias_hot_continuations_impl(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    suppressed_generator_alias_store_instr_ids: &HashSet<InstrId>,
    max_cloned_blocks: Option<usize>,
) -> TypedHotContinuationSplitStats {
    let mut stats = TypedHotContinuationSplitStats::default();
    let mut instr_id_allocator = TypedInlineInstrIdAllocator::from_function(function);
    loop {
        let generator_protocol_alias_use_locations =
            typed_generator_protocol_alias_use_locations_for_hot_continuation_split(
                function,
                module_constants,
            );
        let generator_alias_locations = typed_generator_alias_locations_for_hot_continuation_split(
            function,
            module_constants,
            &generator_protocol_alias_use_locations,
        );
        let Some(candidate) = find_typed_generator_alias_hot_continuation_split_candidate(
            function,
            module_constants,
            &generator_alias_locations,
            &generator_protocol_alias_use_locations,
            suppressed_generator_alias_store_instr_ids,
        ) else {
            break;
        };
        if max_cloned_blocks
            .is_some_and(|max| stats.cloned_blocks + candidate.reachable.len() > max)
        {
            break;
        }
        let hot_block = candidate.hot_block;
        let candidate_alias_store_instr_ids = function
            .blocks
            .iter()
            .filter(|block| block.label == hot_block || candidate.reachable.contains(&block.label))
            .flat_map(|block| {
                typed_block_generator_alias_store_instr_ids(
                    block,
                    module_constants,
                    &generator_alias_locations,
                )
            })
            .collect::<HashSet<_>>();
        let Some(cloned) = clone_typed_hot_continuation(
            function,
            candidate,
            stats.clones.len() as u32,
            &mut instr_id_allocator,
        ) else {
            break;
        };
        stats.cloned_blocks += cloned.clone.cloned_blocks;
        stats
            .alias_store_instr_ids
            .extend(candidate_alias_store_instr_ids.iter().copied());
        stats.alias_store_instr_ids.extend(
            cloned
                .instr_id_mappings
                .iter()
                .filter(|mapping| {
                    candidate_alias_store_instr_ids.contains(&mapping.callee_instr_id)
                })
                .map(|mapping| mapping.caller_instr_id),
        );
        stats.instr_id_mappings.extend(cloned.instr_id_mappings);
        stats.label_mappings.extend(cloned.label_mappings);
        stats.clones.push(cloned.clone);
    }
    stats
}

fn split_typed_alias_hot_continuations_impl(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    suppressed_alias_store_instr_ids: &HashSet<InstrId>,
    max_cloned_blocks: Option<usize>,
) -> TypedHotContinuationSplitStats {
    let mut stats = TypedHotContinuationSplitStats::default();
    let mut instr_id_allocator = TypedInlineInstrIdAllocator::from_function(function);
    let mut cyclic_hot_blocks = HashSet::new();
    loop {
        let Some(candidate) = find_typed_alias_hot_continuation_split_candidate(
            function,
            module_constants,
            suppressed_alias_store_instr_ids,
            &cyclic_hot_blocks,
        ) else {
            break;
        };
        if max_cloned_blocks
            .is_some_and(|max| stats.cloned_blocks + candidate.reachable.len() > max)
        {
            break;
        }
        let cyclic_hot_region = candidate.reachable.contains(&candidate.hot_block);
        let hot_block = candidate.hot_block;
        let candidate_alias_store_instr_ids = function
            .blocks
            .iter()
            .filter(|block| block.label == hot_block || candidate.reachable.contains(&block.label))
            .flat_map(|block| typed_block_local_alias_store_instr_ids(block, module_constants))
            .into_iter()
            .collect::<HashSet<_>>();
        let Some(cloned) = clone_typed_hot_continuation(
            function,
            candidate,
            stats.clones.len() as u32,
            &mut instr_id_allocator,
        ) else {
            break;
        };
        if cyclic_hot_region {
            cyclic_hot_blocks.insert(hot_block);
            if let Some((_, cloned_hot_block)) = cloned
                .label_mappings
                .iter()
                .find(|(source, _)| *source == hot_block)
            {
                cyclic_hot_blocks.insert(*cloned_hot_block);
            }
        }
        stats.cloned_blocks += cloned.clone.cloned_blocks;
        stats
            .alias_store_instr_ids
            .extend(candidate_alias_store_instr_ids.iter().copied());
        stats.alias_store_instr_ids.extend(
            cloned
                .instr_id_mappings
                .iter()
                .filter(|mapping| {
                    candidate_alias_store_instr_ids.contains(&mapping.callee_instr_id)
                })
                .map(|mapping| mapping.caller_instr_id),
        );
        stats.instr_id_mappings.extend(cloned.instr_id_mappings);
        stats.label_mappings.extend(cloned.label_mappings);
        stats.clones.push(cloned.clone);
    }
    stats
}

pub fn split_typed_inline_cleanup_hot_continuations(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> TypedHotContinuationSplitStats {
    split_typed_inline_cleanup_hot_continuations_impl(function, None, None)
}

pub fn split_typed_inline_cleanup_hot_continuations_for_labels(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    cleanup_labels: &HashSet<BlockLabel>,
) -> TypedHotContinuationSplitStats {
    if cleanup_labels.is_empty() {
        return TypedHotContinuationSplitStats::default();
    }
    split_typed_inline_cleanup_hot_continuations_impl(function, Some(cleanup_labels), None)
}

pub fn split_typed_inline_cleanup_hot_continuations_for_labels_with_budget(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    cleanup_labels: &HashSet<BlockLabel>,
    max_cloned_blocks: usize,
) -> TypedHotContinuationSplitStats {
    if cleanup_labels.is_empty() || max_cloned_blocks == 0 {
        return TypedHotContinuationSplitStats::default();
    }
    split_typed_inline_cleanup_hot_continuations_impl(
        function,
        Some(cleanup_labels),
        Some(max_cloned_blocks),
    )
}

fn split_typed_inline_cleanup_hot_continuations_impl(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    cleanup_labels: Option<&HashSet<BlockLabel>>,
    max_cloned_blocks: Option<usize>,
) -> TypedHotContinuationSplitStats {
    let mut stats = TypedHotContinuationSplitStats::default();
    let mut instr_id_allocator = TypedInlineInstrIdAllocator::from_function(function);
    loop {
        let Some(candidate) =
            find_typed_inline_cleanup_hot_continuation_split_candidate(function, cleanup_labels)
        else {
            break;
        };
        if max_cloned_blocks
            .is_some_and(|max| stats.cloned_blocks + candidate.reachable.len() > max)
        {
            break;
        }
        let Some(cloned) = clone_typed_hot_continuation(
            function,
            candidate,
            stats.clones.len() as u32,
            &mut instr_id_allocator,
        ) else {
            break;
        };
        stats.cloned_blocks += cloned.clone.cloned_blocks;
        stats.instr_id_mappings.extend(cloned.instr_id_mappings);
        stats.label_mappings.extend(cloned.label_mappings);
        stats.clones.push(cloned.clone);
    }
    stats
}

#[derive(Debug, Clone)]
struct TypedHotContinuationSplitCandidate {
    hot_block: BlockLabel,
    original_entry: BlockLabel,
    reachable: HashSet<BlockLabel>,
}

struct TypedHotContinuationCloneResult {
    clone: TypedHotContinuationClone,
    instr_id_mappings: Vec<TypedInlineInstrIdMapping>,
    label_mappings: Vec<(BlockLabel, BlockLabel)>,
}

fn find_typed_constructor_hot_continuation_split_candidate(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> Option<TypedHotContinuationSplitCandidate> {
    let labels = typed_block_indices_by_label(function);
    let predecessors = typed_block_predecessors(function);
    let hot_predecessors = typed_hot_block_predecessors(function);
    let mut reachable_by_entry = HashMap::new();
    function.blocks.iter().find_map(|block| {
        let original_entry = typed_constructor_hot_continuation_entry(
            function,
            &labels,
            &predecessors,
            block,
            module_constants,
        )?;
        let reachable = typed_hot_clone_block_labels_cached(
            function,
            &labels,
            &hot_predecessors,
            original_entry,
            &mut reachable_by_entry,
        )?;
        if reachable.contains(&block.label)
            || reachable.len() > MAX_TYPED_HOT_CONTINUATION_CLONE_BLOCKS
            || !typed_reachable_subgraph_has_external_predecessor(
                &reachable,
                &predecessors,
                block.label,
            )
        {
            return None;
        }
        Some(TypedHotContinuationSplitCandidate {
            hot_block: block.label,
            original_entry,
            reachable,
        })
    })
}

fn find_typed_alias_hot_continuation_split_candidate(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    suppressed_alias_store_instr_ids: &HashSet<InstrId>,
    cyclic_hot_blocks: &HashSet<BlockLabel>,
) -> Option<TypedHotContinuationSplitCandidate> {
    let labels = typed_block_indices_by_label(function);
    let predecessors = typed_block_predecessors(function);
    let hot_predecessors = typed_hot_block_predecessors(function);
    let none_placeholder_alias_targets = typed_none_placeholder_alias_target_locations(function);
    let mut reachable_by_entry = HashMap::new();
    function.blocks.iter().find_map(|block| {
        if cyclic_hot_blocks.contains(&block.label) {
            return None;
        }
        let original_entry = typed_alias_hot_continuation_entry(
            function,
            &labels,
            &predecessors,
            block,
            &none_placeholder_alias_targets,
            module_constants,
            suppressed_alias_store_instr_ids,
        )?;
        let reachable = typed_hot_clone_block_labels_cached(
            function,
            &labels,
            &hot_predecessors,
            original_entry,
            &mut reachable_by_entry,
        )?;
        let exceeds_budget = reachable.len() > MAX_TYPED_HOT_CONTINUATION_CLONE_BLOCKS;
        let has_external_predecessor = typed_reachable_subgraph_has_external_predecessor(
            &reachable,
            &predecessors,
            block.label,
        );
        let cyclic_region = reachable.contains(&block.label);
        let has_real_external_predecessor = !cyclic_region
            || typed_cyclic_alias_region_has_real_external_predecessor(
                function,
                &labels,
                &reachable,
                &predecessors,
                original_entry,
                module_constants,
            );
        // Alias stores can sit on the backedge of the hot loop they seed. In
        // that case cloning the SCC is exactly what separates the post-store
        // alias path from the cold/pre-initialized path.
        //
        // After that cyclic clone, the old hot alias block points straight at
        // the cloned entry. Treat that as a breadcrumb of work already done,
        // not as a reason to split the cloned SCC again on the next pass.
        if exceeds_budget || !has_external_predecessor || !has_real_external_predecessor {
            return None;
        }
        Some(TypedHotContinuationSplitCandidate {
            hot_block: block.label,
            original_entry,
            reachable,
        })
    })
}

fn find_typed_generator_alias_hot_continuation_split_candidate(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    generator_alias_locations: &HashSet<LocalLocation>,
    generator_protocol_alias_use_locations: &HashSet<LocalLocation>,
    suppressed_generator_alias_store_instr_ids: &HashSet<InstrId>,
) -> Option<TypedHotContinuationSplitCandidate> {
    if generator_alias_locations.is_empty() || generator_protocol_alias_use_locations.is_empty() {
        return None;
    }
    let labels = typed_block_indices_by_label(function);
    let predecessors = typed_block_predecessors(function);
    let generator_alias_store_blocks = function
        .blocks
        .iter()
        .filter(|block| {
            typed_block_contains_generator_alias_store(
                block,
                module_constants,
                generator_alias_locations,
                suppressed_generator_alias_store_instr_ids,
            )
        })
        .map(|block| block.label)
        .collect::<HashSet<_>>();
    function.blocks.iter().find_map(|block| {
        if !typed_block_contains_generator_alias_store(
            block,
            module_constants,
            generator_alias_locations,
            suppressed_generator_alias_store_instr_ids,
        ) {
            return None;
        }
        let BlockTerm::Jump(edge) = &block.term else {
            return None;
        };
        let original_entry = edge.target;
        let reachable = typed_generator_alias_clone_block_labels(
            function,
            &labels,
            original_entry,
            &generator_alias_store_blocks,
        )?;
        let has_external_predecessor = typed_reachable_subgraph_has_external_predecessor(
            &reachable,
            &predecessors,
            block.label,
        );
        if reachable.len() > MAX_TYPED_HOT_CONTINUATION_CLONE_BLOCKS || !has_external_predecessor {
            return None;
        }
        Some(TypedHotContinuationSplitCandidate {
            hot_block: block.label,
            original_entry,
            reachable,
        })
    })
}

fn typed_generator_alias_clone_block_labels(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    entry: BlockLabel,
    generator_alias_store_blocks: &HashSet<BlockLabel>,
) -> Option<HashSet<BlockLabel>> {
    let mut seen = HashSet::new();
    let mut pending = vec![entry];
    while let Some(label) = pending.pop() {
        if generator_alias_store_blocks.contains(&label) || !seen.insert(label) {
            continue;
        }
        let block = block_by_label(function, labels, label)?;
        pending.extend(typed_hot_normal_successors(block));
    }
    Some(seen)
}

fn typed_cyclic_alias_region_has_real_external_predecessor(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    reachable: &HashSet<BlockLabel>,
    predecessors: &HashMap<BlockLabel, HashSet<BlockLabel>>,
    original_entry: BlockLabel,
    module_constants: &[ConstantExpr],
) -> bool {
    reachable.iter().any(|label| {
        predecessors.get(label).is_some_and(|label_predecessors| {
            label_predecessors.iter().any(|predecessor| {
                if reachable.contains(predecessor) {
                    return false;
                }
                let Some(predecessor_block) = labels
                    .get(predecessor)
                    .and_then(|index| function.blocks.get(*index))
                else {
                    return true;
                };
                let is_clone_breadcrumb =
                    typed_block_contains_local_alias_store(predecessor_block, module_constants)
                        && matches!(
                            &predecessor_block.term,
                            BlockTerm::Jump(edge) if edge.target == original_entry
                        );
                !is_clone_breadcrumb
            })
        })
    })
}

fn find_typed_inline_cleanup_hot_continuation_split_candidate(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    cleanup_labels: Option<&HashSet<BlockLabel>>,
) -> Option<TypedHotContinuationSplitCandidate> {
    let labels = typed_block_indices_by_label(function);
    let predecessors = typed_block_predecessors(function);
    let hot_predecessors = typed_hot_block_predecessors(function);
    let hot_path_labels = typed_direct_call_hot_path_labels(function, &labels);
    let mut reachable_by_entry = HashMap::new();
    function.blocks.iter().find_map(|block| {
        if cleanup_labels.is_some_and(|cleanup_labels| !cleanup_labels.contains(&block.label)) {
            return None;
        }
        let original_entry = typed_inline_cleanup_hot_continuation_entry(&hot_path_labels, block)?;
        let reachable = typed_hot_clone_block_labels_cached(
            function,
            &labels,
            &hot_predecessors,
            original_entry,
            &mut reachable_by_entry,
        )?;
        if reachable.contains(&block.label)
            || reachable.len() > MAX_TYPED_HOT_CONTINUATION_CLONE_BLOCKS
            || !typed_reachable_subgraph_has_external_predecessor(
                &reachable,
                &predecessors,
                block.label,
            )
        {
            return None;
        }
        Some(TypedHotContinuationSplitCandidate {
            hot_block: block.label,
            original_entry,
            reachable,
        })
    })
}

fn typed_constructor_hot_continuation_entry(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    predecessors: &HashMap<BlockLabel, HashSet<BlockLabel>>,
    block: &TypedBlock,
    module_constants: &[ConstantExpr],
) -> Option<BlockLabel> {
    if !typed_block_contains_constructor_call_store(block, module_constants)
        || !typed_block_is_direct_call_guard_hot_successor(function, labels, predecessors, block)
    {
        return None;
    }
    let BlockTerm::Jump(edge) = &block.term else {
        return None;
    };
    Some(edge.target)
}

fn typed_alias_hot_continuation_entry(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    predecessors: &HashMap<BlockLabel, HashSet<BlockLabel>>,
    block: &TypedBlock,
    none_placeholder_alias_targets: &HashSet<LocalLocation>,
    module_constants: &[ConstantExpr],
    suppressed_alias_store_instr_ids: &HashSet<InstrId>,
) -> Option<BlockLabel> {
    let contains_local_alias_store =
        typed_block_contains_local_alias_store(block, module_constants);
    let alias_store_instr_ids = typed_block_local_alias_store_instr_ids(block, module_constants);
    let unsuppressed_alias_store = alias_store_instr_ids.is_empty()
        || alias_store_instr_ids
            .into_iter()
            .any(|instr_id| !suppressed_alias_store_instr_ids.contains(&instr_id));
    let direct_call_guard_hot_successor =
        typed_block_is_direct_call_guard_hot_successor(function, labels, predecessors, block);
    let contains_none_placeholder_alias_store = typed_block_contains_none_placeholder_alias_store(
        block,
        none_placeholder_alias_targets,
        module_constants,
    );
    if !contains_local_alias_store
        || (!suppressed_alias_store_instr_ids.is_empty() && !unsuppressed_alias_store)
        || (!direct_call_guard_hot_successor && !contains_none_placeholder_alias_store)
    {
        return None;
    }
    let BlockTerm::Jump(edge) = &block.term else {
        return None;
    };
    Some(edge.target)
}

fn typed_inline_cleanup_hot_continuation_entry(
    hot_path_labels: &HashSet<BlockLabel>,
    block: &TypedBlock,
) -> Option<BlockLabel> {
    if !typed_block_is_inline_cleanup(block) || !hot_path_labels.contains(&block.label) {
        return None;
    }
    let BlockTerm::Jump(edge) = &block.term else {
        return None;
    };
    Some(edge.target)
}

fn typed_block_contains_constructor_call_store(
    block: &TypedBlock,
    module_constants: &[ConstantExpr],
) -> bool {
    block.body.iter().any(|instr| {
        let InstrTyped::Store(store) = instr else {
            return false;
        };
        let InstrTyped::CallTyped(call) = store.value.as_ref() else {
            return false;
        };
        call.extra.generator_instance_plan().is_some()
            || typed_expr_is_runtime_name_load(
                call.func.as_ref(),
                RuntimeName::ConstructorCall,
                module_constants,
            )
    })
}

fn typed_block_contains_local_alias_store(
    block: &TypedBlock,
    module_constants: &[ConstantExpr],
) -> bool {
    block.body.iter().any(|instr| {
        let InstrTyped::Store(store) = instr else {
            return false;
        };
        if store.name.location.as_local().is_none() {
            return false;
        }
        typed_expr_local_alias_candidate(store.value.as_ref(), module_constants)
    })
}

fn typed_generator_alias_locations_for_hot_continuation_split(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    generator_protocol_alias_use_locations: &HashSet<LocalLocation>,
) -> HashSet<LocalLocation> {
    let mut aliases = function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|instr| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            let location = store.name.local_location()?;
            store
                .value
                .generator_instance_plan()
                .is_some()
                .then_some(location)
        })
        .collect::<HashSet<_>>();
    aliases.extend(generator_protocol_alias_use_locations.iter().copied());
    loop {
        let mut changed = false;
        for instr in function.blocks.iter().flat_map(|block| block.body.iter()) {
            let InstrTyped::Store(store) = instr else {
                continue;
            };
            let Some(target) = store.name.local_location() else {
                continue;
            };
            if aliases.contains(&target) {
                for source in
                    typed_generator_alias_source_locations(store.value.as_ref(), module_constants)
                {
                    changed |= aliases.insert(source);
                }
            }
            changed |= typed_generator_alias_expr(store.value.as_ref(), module_constants, &aliases)
                && aliases.insert(target);
        }
        if !changed {
            return aliases;
        }
    }
}

fn typed_generator_alias_source_locations(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Vec<LocalLocation> {
    if let Some(location) = typed_instr_local_load_location(expr) {
        return vec![location];
    }
    let Some((func, args, keywords)) = typed_callable_call_parts(expr) else {
        return Vec::new();
    };
    if !keywords.is_empty()
        || !typed_expr_is_runtime_name_load(func, RuntimeName::Iter, module_constants)
    {
        return Vec::new();
    }
    match args {
        [CallArgPositional::Positional(owner)] => {
            typed_instr_local_load_location(owner).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

fn typed_block_contains_generator_alias_store(
    block: &TypedBlock,
    module_constants: &[ConstantExpr],
    generator_alias_locations: &HashSet<LocalLocation>,
    suppressed_generator_alias_store_instr_ids: &HashSet<InstrId>,
) -> bool {
    block.body.iter().any(|instr| {
        if instr
            .try_semantic_instr_id()
            .is_some_and(|instr_id| suppressed_generator_alias_store_instr_ids.contains(&instr_id))
        {
            return false;
        }
        let InstrTyped::Store(store) = instr else {
            return false;
        };
        let Some(target) = store.name.local_location() else {
            return false;
        };
        generator_alias_locations.contains(&target)
            && typed_generator_alias_expr(
                store.value.as_ref(),
                module_constants,
                generator_alias_locations,
            )
    })
}

fn typed_block_generator_alias_store_instr_ids(
    block: &TypedBlock,
    module_constants: &[ConstantExpr],
    generator_alias_locations: &HashSet<LocalLocation>,
) -> Vec<InstrId> {
    block
        .body
        .iter()
        .filter_map(|instr| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            let target = store.name.local_location()?;
            (generator_alias_locations.contains(&target)
                && typed_generator_alias_expr(
                    store.value.as_ref(),
                    module_constants,
                    generator_alias_locations,
                ))
            .then(|| instr.try_semantic_instr_id())
            .flatten()
        })
        .collect()
}

fn typed_generator_protocol_alias_use_locations_for_hot_continuation_split(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> HashSet<LocalLocation> {
    struct Finder<'a> {
        module_constants: &'a [ConstantExpr],
        locations: HashSet<LocalLocation>,
    }

    impl Finder<'_> {
        fn collect_protocol_alias_args(
            &mut self,
            func: &InstrTyped,
            args: &[CallArgPositional<InstrTyped>],
        ) {
            if ![
                RuntimeName::Iter,
                RuntimeName::Next,
                RuntimeName::ResumeGenerator,
            ]
            .into_iter()
            .any(|runtime_name| {
                typed_expr_is_runtime_name_load(func, runtime_name, self.module_constants)
            }) {
                return;
            }
            self.locations.extend(args.iter().filter_map(|arg| {
                let CallArgPositional::Positional(InstrTyped::Load(load)) = arg else {
                    return None;
                };
                load.name.local_location()
            }));
        }
    }

    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            match expr {
                InstrTyped::CallTyped(call) => {
                    self.collect_protocol_alias_args(call.func.as_ref(), &call.args);
                }
                InstrTyped::GuardedCallableCallTyped(call) => {
                    self.collect_protocol_alias_args(call.func.as_ref(), &call.args);
                }
                InstrTyped::DirectCallableCallTyped(call) => {
                    self.collect_protocol_alias_args(call.func.as_ref(), &call.args);
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        module_constants,
        locations: HashSet::new(),
    };
    finder.visit_fn(function);
    finder.locations
}

fn typed_block_local_alias_store_instr_ids(
    block: &TypedBlock,
    module_constants: &[ConstantExpr],
) -> Vec<InstrId> {
    block
        .body
        .iter()
        .filter_map(|instr| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            if store.name.location.as_local().is_none()
                || !typed_expr_local_alias_candidate(store.value.as_ref(), module_constants)
            {
                return None;
            }
            instr.try_semantic_instr_id()
        })
        .collect()
}

fn typed_none_placeholder_alias_target_locations(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashSet<LocalLocation> {
    function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|instr| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            let target = store.name.local_location()?;
            let InstrTyped::Load(load) = store.value.as_ref() else {
                return None;
            };
            (load.name.id_str() == "NONE").then_some(target)
        })
        .collect()
}

fn typed_block_contains_none_placeholder_alias_store(
    block: &TypedBlock,
    none_placeholder_alias_targets: &HashSet<LocalLocation>,
    module_constants: &[ConstantExpr],
) -> bool {
    block.body.iter().any(|instr| {
        let InstrTyped::Store(store) = instr else {
            return false;
        };
        let Some(target) = store.name.local_location() else {
            return false;
        };
        none_placeholder_alias_targets.contains(&target)
            && typed_expr_local_alias_candidate(store.value.as_ref(), module_constants)
    })
}

fn typed_expr_local_alias_candidate(expr: &InstrTyped, module_constants: &[ConstantExpr]) -> bool {
    typed_instr_local_load_location(expr).is_some()
        || typed_iter_local_alias_call(expr, module_constants)
}

fn typed_iter_local_alias_call(expr: &InstrTyped, module_constants: &[ConstantExpr]) -> bool {
    let Some((func, args, keywords)) = typed_callable_call_parts(expr) else {
        return false;
    };
    keywords.is_empty()
        && typed_expr_is_runtime_name_load(func, RuntimeName::Iter, module_constants)
        && matches!(
            args,
            [CallArgPositional::Positional(owner)]
                if typed_instr_local_load_location(owner).is_some()
        )
}

fn typed_block_is_inline_cleanup(block: &TypedBlock) -> bool {
    block.params.is_empty()
        && matches!(block.term, BlockTerm::Jump(_))
        && !block.body.is_empty()
        && block
            .body
            .iter()
            .all(|instr| matches!(instr, InstrTyped::Del(_)))
}

fn typed_block_is_direct_call_guard_hot_successor(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    predecessors: &HashMap<BlockLabel, HashSet<BlockLabel>>,
    block: &TypedBlock,
) -> bool {
    predecessors
        .get(&block.label)
        .into_iter()
        .flat_map(|predecessors| predecessors.iter())
        .filter_map(|predecessor| block_by_label(function, labels, *predecessor))
        .any(|predecessor| {
            typed_block_direct_call_guard_then_label(predecessor) == Some(block.label)
        })
}

fn typed_block_direct_call_guard_then_label(block: &TypedBlock) -> Option<BlockLabel> {
    let BlockTerm::IfTerm(if_term) = &block.term else {
        return None;
    };
    matches!(if_term.test, InstrTyped::DirectCallGuardTest(_)).then_some(if_term.then_label)
}

fn typed_block_indices_by_label(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, usize> {
    function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect()
}

fn typed_block_successors(block: &TypedBlock) -> Vec<BlockLabel> {
    let mut successors = typed_term_successors(&block.term);
    if let Some(edge) = &block.exc_edge {
        successors.push(edge.target);
    }
    successors
}

fn typed_block_predecessors(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, HashSet<BlockLabel>> {
    let mut predecessors = HashMap::<BlockLabel, HashSet<BlockLabel>>::new();
    for block in &function.blocks {
        for successor in typed_block_successors(block) {
            predecessors
                .entry(successor)
                .or_default()
                .insert(block.label);
        }
    }
    predecessors
}

fn typed_reachable_block_labels(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    entry: BlockLabel,
) -> Option<HashSet<BlockLabel>> {
    let mut seen = HashSet::new();
    let mut pending = vec![entry];
    while let Some(label) = pending.pop() {
        if !seen.insert(label) {
            continue;
        }
        let block = block_by_label(function, labels, label)?;
        pending.extend(typed_block_successors(block));
    }
    Some(seen)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypedReachableBlockView {
    labels: HashSet<BlockLabel>,
}

impl TypedReachableBlockView {
    pub fn for_function(function: &BlockPyFunction<TypedBlockPyModuleShape>) -> Self {
        let Some(entry) = function.blocks.first().map(|block| block.label) else {
            return Self::default();
        };
        let labels = typed_block_indices_by_label(function);
        Self {
            labels: typed_reachable_block_labels(function, &labels, entry).unwrap_or_default(),
        }
    }

    pub fn contains(&self, label: BlockLabel) -> bool {
        self.labels.contains(&label)
    }

    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    pub fn labels(&self) -> &HashSet<BlockLabel> {
        &self.labels
    }

    pub fn iter_blocks<'a>(
        &'a self,
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
    ) -> impl Iterator<Item = &'a TypedBlock> + 'a {
        function
            .blocks
            .iter()
            .filter(move |block| self.contains(block.label))
    }
}

fn typed_direct_call_hot_path_labels(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
) -> HashSet<BlockLabel> {
    let mut seen = HashSet::new();
    for block in &function.blocks {
        if let Some(hot_label) = typed_block_direct_call_guard_then_label(block) {
            collect_typed_hot_reachable_block_labels(function, labels, hot_label, &mut seen);
        }
    }
    seen
}

fn typed_hot_reachable_block_labels(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    entry: BlockLabel,
) -> Option<HashSet<BlockLabel>> {
    let mut seen = HashSet::new();
    collect_typed_hot_reachable_block_labels(function, labels, entry, &mut seen)?;
    Some(seen)
}

fn typed_hot_clone_block_labels(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    predecessors: &HashMap<BlockLabel, HashSet<BlockLabel>>,
    entry: BlockLabel,
) -> Option<HashSet<BlockLabel>> {
    let reachable = typed_hot_reachable_block_labels(function, labels, entry)?;
    let mut region = HashSet::new();
    let mut pending = vec![entry];
    let mut cyclic_components = HashMap::<BlockLabel, Option<HashSet<BlockLabel>>>::new();
    while let Some(label) = pending.pop() {
        if !region.insert(label) {
            continue;
        }
        let component = if let Some(component) = cyclic_components.get(&label) {
            component.clone()
        } else {
            let component =
                typed_hot_cyclic_component(function, labels, predecessors, &reachable, label)?;
            if let Some(component_labels) = component.as_ref() {
                for component_label in component_labels {
                    cyclic_components.insert(*component_label, Some(component_labels.clone()));
                }
            } else {
                cyclic_components.insert(label, None);
            }
            component
        };
        if let Some(component) = component {
            region.extend(component);
            continue;
        }
        pending.extend(
            typed_hot_normal_successors(block_by_label(function, labels, label)?)
                .into_iter()
                .filter(|successor| reachable.contains(successor)),
        );
    }
    Some(region)
}

fn typed_hot_clone_block_labels_cached(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    predecessors: &HashMap<BlockLabel, HashSet<BlockLabel>>,
    entry: BlockLabel,
    reachable_by_entry: &mut HashMap<BlockLabel, Option<HashSet<BlockLabel>>>,
) -> Option<HashSet<BlockLabel>> {
    if let Some(reachable) = reachable_by_entry.get(&entry) {
        return reachable.clone();
    }
    let reachable = typed_hot_clone_block_labels(function, labels, predecessors, entry);
    reachable_by_entry.insert(entry, reachable.clone());
    reachable
}

fn typed_hot_block_predecessors(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, HashSet<BlockLabel>> {
    let mut predecessors = HashMap::<BlockLabel, HashSet<BlockLabel>>::new();
    for block in &function.blocks {
        for successor in typed_hot_normal_successors(block) {
            predecessors
                .entry(successor)
                .or_default()
                .insert(block.label);
        }
    }
    predecessors
}

fn typed_hot_cyclic_component(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    predecessors: &HashMap<BlockLabel, HashSet<BlockLabel>>,
    reachable: &HashSet<BlockLabel>,
    entry: BlockLabel,
) -> Option<Option<HashSet<BlockLabel>>> {
    let mut forward = HashSet::new();
    collect_typed_hot_reachable_block_labels(function, labels, entry, &mut forward)?;
    forward.retain(|label| reachable.contains(label));

    let mut backward = HashSet::new();
    let mut pending = vec![entry];
    while let Some(label) = pending.pop() {
        if !backward.insert(label) {
            continue;
        }
        pending.extend(
            predecessors
                .get(&label)
                .into_iter()
                .flat_map(|predecessors| predecessors.iter().copied())
                .filter(|predecessor| reachable.contains(predecessor)),
        );
    }

    let component = forward
        .intersection(&backward)
        .copied()
        .collect::<HashSet<_>>();
    let self_loop =
        typed_hot_normal_successors(block_by_label(function, labels, entry)?).contains(&entry);
    if component.len() > 1 || self_loop {
        Some(Some(component))
    } else {
        Some(None)
    }
}

fn collect_typed_hot_reachable_block_labels(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    entry: BlockLabel,
    seen: &mut HashSet<BlockLabel>,
) -> Option<()> {
    let mut pending = vec![entry];
    while let Some(label) = pending.pop() {
        if !seen.insert(label) {
            continue;
        }
        let block = block_by_label(function, labels, label)?;
        pending.extend(typed_hot_normal_successors(block));
    }
    Some(())
}

fn typed_hot_normal_successors(block: &TypedBlock) -> Vec<BlockLabel> {
    if let Some(hot_label) = typed_block_direct_call_guard_then_label(block) {
        return vec![hot_label];
    }
    typed_term_successors(&block.term)
}

fn typed_reachable_subgraph_has_external_predecessor(
    reachable: &HashSet<BlockLabel>,
    predecessors: &HashMap<BlockLabel, HashSet<BlockLabel>>,
    source: BlockLabel,
) -> bool {
    reachable.iter().any(|label| {
        predecessors.get(label).is_some_and(|label_predecessors| {
            label_predecessors
                .iter()
                .any(|predecessor| *predecessor != source && !reachable.contains(predecessor))
        })
    })
}

fn clone_typed_hot_continuation(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    candidate: TypedHotContinuationSplitCandidate,
    clone_instance: u32,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
) -> Option<TypedHotContinuationCloneResult> {
    let label_map = candidate
        .reachable
        .iter()
        .map(|label| (*label, function.name_gen.next_block_name()))
        .collect::<HashMap<_, _>>();
    let cloned_entry = *label_map.get(&candidate.original_entry)?;
    let hot_block = function
        .blocks
        .iter_mut()
        .find(|block| block.label == candidate.hot_block)?;
    let BlockTerm::Jump(edge) = &mut hot_block.term else {
        return None;
    };
    if edge.target != candidate.original_entry {
        return None;
    }
    edge.target = cloned_entry;

    let mut instr_id_remapper =
        TypedInlineInstrIdRemapper::new(function.function_id, clone_instance, instr_id_allocator);
    let mut cloned_blocks = Vec::with_capacity(candidate.reachable.len());
    for block in function
        .blocks
        .iter()
        .filter(|block| candidate.reachable.contains(&block.label))
    {
        let mut cloned = block.clone();
        cloned.label = *label_map.get(&block.label)?;
        let mut label_remapper = TypedContinuationCloneLabelRemapper { labels: &label_map };
        label_remapper.visit_block_mut(&mut cloned);
        let mut instr_remapper = TypedContinuationCloneInstrIdRemapper {
            remapper: &mut instr_id_remapper,
        };
        instr_remapper.visit_block_mut(&mut cloned);
        cloned_blocks.push(cloned);
    }
    let cloned_block_count = cloned_blocks.len();
    function.blocks.extend(cloned_blocks);
    Some(TypedHotContinuationCloneResult {
        clone: TypedHotContinuationClone {
            hot_block: candidate.hot_block,
            original_entry: candidate.original_entry,
            cloned_entry,
            cloned_blocks: cloned_block_count,
            cyclic_hot_region: candidate.reachable.contains(&candidate.hot_block),
        },
        instr_id_mappings: instr_id_remapper.finish(),
        label_mappings: label_map.into_iter().collect(),
    })
}

struct TypedContinuationCloneLabelRemapper<'a> {
    labels: &'a HashMap<BlockLabel, BlockLabel>,
}

impl VisitMut<InstrTyped> for TypedContinuationCloneLabelRemapper<'_> {
    fn visit_label_mut(&mut self, label: &mut BlockLabel) {
        if let Some(mapped) = self.labels.get(label) {
            *label = *mapped;
        }
    }
}

struct TypedContinuationCloneInstrIdRemapper<'a, 'b> {
    remapper: &'a mut TypedInlineInstrIdRemapper<'b>,
}

impl VisitMut<InstrTyped> for TypedContinuationCloneInstrIdRemapper<'_, '_> {
    fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
        expr.visit_children_mut(self);
        *expr = self.remapper.remap_instr_id(expr.clone());
    }
}

pub fn typed_constructor_field_bindings_from_inline_stats(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    inline_plan: &InlinePlanModule,
    module_constants: &[ConstantExpr],
    stats: &TypedInlineRewriteStats,
) -> HashMap<InstrId, TypedConstructorFieldBindings> {
    typed_constructor_field_bindings_from_inline_stats_with_external_callees(
        module,
        inline_plan,
        module_constants,
        &HashMap::new(),
        stats,
    )
}

pub fn typed_constructor_init_plans_from_inline_stats_with_external_callees(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    stats: &TypedInlineRewriteStats,
) -> HashMap<InstrId, TypedConstructorInitPlan> {
    let functions = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                TypedConstructorCallContext {
                    function,
                    module_constants,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let mut plans = HashMap::new();
    for mapping in &stats.instr_id_mappings {
        let external_context;
        let context = if let Some(context) = functions.get(&mapping.callee).copied() {
            context
        } else {
            let Some(callee) = external_callees.get(&mapping.callee) else {
                continue;
            };
            external_context = TypedConstructorCallContext {
                function: &callee.function,
                module_constants: callee.module_constants.as_slice(),
            };
            external_context
        };
        let Some(init_function_id) =
            constructor_init_function_id_for_entry_function(context.function)
        else {
            continue;
        };
        let constructor_call_instr_ids =
            typed_constructor_call_instr_ids(context.function, context.module_constants);
        if !constructor_call_instr_ids.contains(&mapping.callee_instr_id) {
            continue;
        }
        plans.insert(
            mapping.caller_instr_id,
            TypedConstructorInitPlan {
                source: TypedConstructorInitPlanSource::InlinedConstructorEntry,
                init_function_id,
            },
        );
    }
    plans
}

pub fn typed_constructor_field_bindings_from_inline_stats_with_external_callees(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    inline_plan: &InlinePlanModule,
    module_constants: &[ConstantExpr],
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    stats: &TypedInlineRewriteStats,
) -> HashMap<InstrId, TypedConstructorFieldBindings> {
    let functions = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                TypedConstructorInlineContext {
                    function,
                    inline_plan,
                    module_constants,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let local_mappings = stats
        .local_mappings
        .iter()
        .map(|mapping| {
            (
                (
                    mapping.callee,
                    mapping.inline_instance,
                    mapping.callee_location,
                ),
                mapping,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut bindings = HashMap::new();
    for mapping in &stats.instr_id_mappings {
        let external_context;
        let context = if let Some(context) = functions.get(&mapping.callee).copied() {
            context
        } else {
            let Some(callee) = external_callees.get(&mapping.callee) else {
                continue;
            };
            let Some(inline_plan) = callee.inline_plan.as_ref() else {
                continue;
            };
            external_context = TypedConstructorInlineContext {
                function: &callee.function,
                inline_plan,
                module_constants: callee.module_constants.as_slice(),
            };
            external_context
        };
        let Some(init_function_id) =
            constructor_init_function_id_for_entry_function(context.function)
        else {
            continue;
        };
        let constructor_call_instr_ids =
            typed_constructor_call_instr_ids(context.function, context.module_constants);
        if !constructor_call_instr_ids.contains(&mapping.callee_instr_id) {
            continue;
        }
        let Some(plan) = context
            .inline_plan
            .straightline_constructor(init_function_id)
        else {
            continue;
        };
        let fields = plan
            .field_stores
            .iter()
            .map(|store| {
                let ConstructorFieldValue::Param { index, .. } = &store.value else {
                    return None;
                };
                let callee_location = LocalLocation(
                    u32::try_from(*index).expect("constructor parameter index should fit in u32"),
                );
                let local_mapping = local_mappings.get(&(
                    mapping.callee,
                    mapping.inline_instance,
                    callee_location,
                ))?;
                Some(TypedConstructorFieldBinding {
                    field_name: store.field_name.clone(),
                    value: ResolvedName {
                        id: local_mapping.caller_name.clone().into(),
                        location: NameLocation::Local(local_mapping.caller_location),
                    },
                    scalar: None,
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(fields) = fields else {
            continue;
        };
        if !fields.is_empty() {
            bindings.insert(
                mapping.caller_instr_id,
                TypedConstructorFieldBindings { fields },
            );
        }
    }
    bindings
}

#[derive(Clone, Copy)]
struct TypedConstructorInlineContext<'a> {
    function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
    inline_plan: &'a InlinePlanModule,
    module_constants: &'a [ConstantExpr],
}

#[derive(Clone, Copy)]
struct TypedConstructorCallContext<'a> {
    function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &'a [ConstantExpr],
}

fn typed_constructor_call_instr_ids(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> HashSet<InstrId> {
    struct Collector<'a> {
        module_constants: &'a [ConstantExpr],
        instr_ids: HashSet<InstrId>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            match expr {
                InstrTyped::CallTyped(call)
                    if typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ConstructorCall,
                        self.module_constants,
                    ) && call.keywords.is_empty() =>
                {
                    if let Some(instr_id) = call.try_semantic_instr_id() {
                        self.instr_ids.insert(instr_id);
                    }
                }
                InstrTyped::DirectCallableCallTyped(call)
                    if typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ConstructorCall,
                        self.module_constants,
                    ) =>
                {
                    if let Some(instr_id) = call.try_semantic_instr_id() {
                        self.instr_ids.insert(instr_id);
                    }
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        module_constants,
        instr_ids: HashSet::new(),
    };
    collector.visit_fn(function);
    collector.instr_ids
}

type TypedLinearizedGetAttrDefs<'a> = HashMap<ResolvedName, (&'a InstrTyped, &'a InstrTyped)>;

fn typed_inline_call_get_attr_parts<'a>(
    func: &'a InstrTyped,
    linearized_get_attrs: &'a TypedLinearizedGetAttrDefs<'a>,
) -> Option<(&'a InstrTyped, &'a InstrTyped, Option<ResolvedName>)> {
    match func {
        InstrTyped::GetAttrTyped(get_attr) => {
            Some((get_attr.value.as_ref(), get_attr.attr.as_ref(), None))
        }
        InstrTyped::Load(load) => linearized_get_attrs
            .get(&load.name)
            .copied()
            .map(|(value, attr)| (value, attr, Some(load.name.clone()))),
        _ => None,
    }
}

fn update_typed_inline_linearized_get_attr_defs<'a>(
    defs: &mut TypedLinearizedGetAttrDefs<'a>,
    instr: &'a InstrTyped,
) {
    match instr {
        InstrTyped::Store(store) => {
            if let InstrTyped::GetAttrTyped(get_attr) = store.value.as_ref() {
                defs.insert(
                    store.name.clone(),
                    (get_attr.value.as_ref(), get_attr.attr.as_ref()),
                );
            } else {
                defs.remove(&store.name);
            }
        }
        InstrTyped::Del(del) => {
            defs.remove(&del.name);
        }
        _ => {}
    }
}

fn typed_inline_candidate_for_expr(
    instr_index: usize,
    result: TypedInlineResult,
    expr: &InstrTyped,
    caller_id: RuntimeFunctionId,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
    trusted_direct_method_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    linearized_get_attrs: &TypedLinearizedGetAttrDefs<'_>,
) -> Option<TypedInlineStoreCandidate> {
    match expr {
        InstrTyped::DirectCallableCallTyped(call) => {
            typed_inline_candidate_for_direct_builtin_implementation_call(
                instr_index,
                result.clone(),
                call,
                caller_id,
            )
            .or_else(|| {
                typed_inline_candidate_for_direct_callable_call(
                    instr_index,
                    result,
                    call,
                    caller_id,
                    direct_calls_by_instr_id,
                )
            })
        }
        InstrTyped::GuardedCallableCallTyped(call) => {
            typed_inline_candidate_for_guarded_builtin_implementation_call(
                instr_index,
                result.clone(),
                call,
                caller_id,
            )
            .or_else(|| {
                typed_inline_candidate_for_callable_call(
                    instr_index,
                    result,
                    call,
                    caller_id,
                    direct_calls_by_instr_id,
                )
            })
        }
        InstrTyped::GuardedMethodCallTyped(call) => typed_inline_candidate_for_method_call(
            instr_index,
            result,
            call,
            caller_id,
            direct_calls_by_instr_id,
        ),
        InstrTyped::CallTyped(call) => typed_inline_candidate_for_builtin_implementation_call(
            instr_index,
            result.clone(),
            call,
            caller_id,
        )
        .or_else(|| {
            typed_inline_candidate_for_generator_resume_call(
                instr_index,
                result.clone(),
                call,
                caller_id,
            )
        })
        .or_else(|| {
            typed_inline_candidate_for_direct_method_call(
                instr_index,
                result.clone(),
                call,
                caller_id,
                direct_calls_by_instr_id,
                trusted_direct_method_calls,
                linearized_get_attrs,
            )
        })
        .or_else(|| {
            typed_inline_candidate_for_runtime_protocol_call(
                instr_index,
                result,
                call,
                caller_id,
                direct_calls_by_instr_id,
                trusted_direct_method_calls,
            )
        }),
        _ => None,
    }
}

fn typed_builtin_inline_candidate_for_expr(
    instr_index: usize,
    result: TypedInlineResult,
    expr: &InstrTyped,
    caller_id: RuntimeFunctionId,
) -> Option<TypedInlineStoreCandidate> {
    match expr {
        InstrTyped::DirectCallableCallTyped(call) => {
            typed_inline_candidate_for_direct_builtin_implementation_call(
                instr_index,
                result,
                call,
                caller_id,
            )
        }
        InstrTyped::GuardedCallableCallTyped(call) => {
            typed_inline_candidate_for_guarded_builtin_implementation_call(
                instr_index,
                result,
                call,
                caller_id,
            )
        }
        InstrTyped::CallTyped(call) => typed_inline_candidate_for_builtin_implementation_call(
            instr_index,
            result,
            call,
            caller_id,
        ),
        _ => None,
    }
}

fn find_typed_inline_candidates(
    block: &TypedBlock,
    caller_id: RuntimeFunctionId,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
    trusted_direct_method_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
) -> Vec<TypedInlineStoreCandidate> {
    let mut candidates = Vec::new();
    candidates.extend(
        block
            .body
            .iter()
            .enumerate()
            .filter_map(|(instr_index, instr)| {
                let InstrTyped::Store(store) = instr else {
                    return typed_builtin_inline_candidate_for_expr(
                        instr_index,
                        TypedInlineResult::EffectOnly,
                        instr,
                        caller_id,
                    );
                };
                typed_builtin_inline_candidate_for_expr(
                    instr_index,
                    TypedInlineResult::StoreTo(store.name.clone()),
                    store.value.as_ref(),
                    caller_id,
                )
            }),
    );
    if let BlockTerm::Return(value) = &block.term
        && let Some(candidate) = typed_builtin_inline_candidate_for_expr(
            block.body.len(),
            TypedInlineResult::Return,
            value,
            caller_id,
        )
    {
        candidates.push(candidate);
    }
    let mut linearized_get_attrs = TypedLinearizedGetAttrDefs::new();
    for (instr_index, instr) in block.body.iter().enumerate() {
        let candidate = if let InstrTyped::Store(store) = instr {
            typed_inline_candidate_for_expr(
                instr_index,
                TypedInlineResult::StoreTo(store.name.clone()),
                store.value.as_ref(),
                caller_id,
                direct_calls_by_instr_id,
                trusted_direct_method_calls,
                &linearized_get_attrs,
            )
        } else {
            typed_inline_candidate_for_expr(
                instr_index,
                TypedInlineResult::EffectOnly,
                instr,
                caller_id,
                direct_calls_by_instr_id,
                trusted_direct_method_calls,
                &linearized_get_attrs,
            )
        };
        if let Some(candidate) = candidate {
            candidates.push(candidate);
        }
        update_typed_inline_linearized_get_attr_defs(&mut linearized_get_attrs, instr);
    }
    if let BlockTerm::Return(value) = &block.term
        && let Some(candidate) = typed_inline_candidate_for_expr(
            block.body.len(),
            TypedInlineResult::Return,
            value,
            caller_id,
            direct_calls_by_instr_id,
            trusted_direct_method_calls,
            &linearized_get_attrs,
        )
    {
        candidates.push(candidate);
    }
    candidates
}

fn typed_inline_candidate_for_direct_callable_call(
    instr_index: usize,
    result: TypedInlineResult,
    call: &TypedDirectCallableCall<InstrTyped>,
    caller_id: RuntimeFunctionId,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
) -> Option<TypedInlineStoreCandidate> {
    let instr_id = call.try_semantic_instr_id()?;
    let plans = direct_calls_by_instr_id.get(&instr_id)?;
    let TypedDirectCallableCallGuard::Function(guard) = &call.guard;
    if guard.function_id == caller_id
        || !plans
            .iter()
            .any(|(target, arg_plan)| *target == guard.function_id && *arg_plan == guard.arg_plan)
    {
        return None;
    }
    Some(TypedInlineStoreCandidate {
        instr_index,
        result,
        call: TypedInlineCall::DirectCallable(call.clone()),
        inline_plans: vec![TypedInlineDirectCallPlan {
            target: guard.function_id,
            arg_plan: guard.arg_plan.clone(),
            guard: if matches!(
                call.func.as_ref(),
                InstrTyped::Load(load) if load.name.runtime_name_id().is_some()
            ) {
                TypedInlineGuardPlan::TrustedRuntime
            } else {
                TypedInlineGuardPlan::Direct
            },
        }],
    })
}

fn typed_inline_candidate_for_callable_call(
    instr_index: usize,
    result: TypedInlineResult,
    call: &TypedGuardedCallableCall<InstrTyped>,
    caller_id: RuntimeFunctionId,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
) -> Option<TypedInlineStoreCandidate> {
    let instr_id = call.try_semantic_instr_id()?;
    let plans = direct_calls_by_instr_id.get(&instr_id)?;
    let inline_plans = plans
        .iter()
        .filter_map(|(target, arg_plan)| {
            if *target == caller_id
                || !call
                    .function_guards
                    .iter()
                    .any(|guard| guard.function_id == *target)
            {
                return None;
            }
            Some(TypedInlineDirectCallPlan {
                target: *target,
                arg_plan: arg_plan.clone(),
                guard: TypedInlineGuardPlan::Callable,
            })
        })
        .collect::<Vec<_>>();
    (!inline_plans.is_empty()).then_some(TypedInlineStoreCandidate {
        instr_index,
        result,
        call: TypedInlineCall::Callable(call.clone()),
        inline_plans,
    })
}

fn typed_inline_candidate_for_method_call(
    instr_index: usize,
    result: TypedInlineResult,
    call: &TypedGuardedMethodCall<InstrTyped>,
    caller_id: RuntimeFunctionId,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
) -> Option<TypedInlineStoreCandidate> {
    let instr_id = call.try_semantic_instr_id()?;
    let InstrTyped::GetAttrTyped(get_attr) = call.func.as_ref() else {
        return None;
    };
    let plans = direct_calls_by_instr_id.get(&instr_id)?;
    let inline_plans = plans
        .iter()
        .filter_map(|(target, arg_plan)| {
            if *target == caller_id {
                return None;
            }
            let guard = call
                .method_guards
                .iter()
                .find(|guard| guard.function_id == *target)?;
            Some(TypedInlineDirectCallPlan {
                target: *target,
                arg_plan: arg_plan.clone(),
                guard: TypedInlineGuardPlan::Method(guard.clone()),
            })
        })
        .collect::<Vec<_>>();
    (!inline_plans.is_empty()).then_some(TypedInlineStoreCandidate {
        instr_index,
        result,
        call: TypedInlineCall::Method {
            call: call.clone(),
            receiver: get_attr.value.as_ref().clone(),
            attr: get_attr.attr.as_ref().clone(),
        },
        inline_plans,
    })
}

fn typed_inline_candidate_for_direct_method_call(
    instr_index: usize,
    result: TypedInlineResult,
    call: &TypedCall<InstrTyped>,
    caller_id: RuntimeFunctionId,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
    trusted_direct_method_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    linearized_get_attrs: &TypedLinearizedGetAttrDefs<'_>,
) -> Option<TypedInlineStoreCandidate> {
    if !matches!(call.access, TypedCallAccessPlan::Generic) {
        return None;
    }
    let instr_id = call.try_semantic_instr_id()?;
    trusted_direct_method_calls.get(&instr_id)?;
    let (get_attr_value, _, linearized_func_temp) =
        typed_inline_call_get_attr_parts(call.func.as_ref(), linearized_get_attrs)?;
    let [(target, arg_plan)] = direct_calls_by_instr_id.get(&instr_id)?.as_slice() else {
        return None;
    };
    if *target == caller_id {
        return None;
    }
    Some(TypedInlineStoreCandidate {
        instr_index,
        result,
        call: TypedInlineCall::DirectMethod {
            call: call.clone(),
            receiver: get_attr_value.clone(),
            linearized_func_temp,
        },
        inline_plans: vec![TypedInlineDirectCallPlan {
            target: *target,
            arg_plan: arg_plan.clone(),
            guard: TypedInlineGuardPlan::Direct,
        }],
    })
}

fn typed_inline_candidate_for_generator_resume_call(
    instr_index: usize,
    result: TypedInlineResult,
    call: &TypedCall<InstrTyped>,
    caller_id: RuntimeFunctionId,
) -> Option<TypedInlineStoreCandidate> {
    let plan = call.extra.generator_resume_plan()?;
    if plan.function_id == caller_id || call.args.len() != 5 || !call.keywords.is_empty() {
        return None;
    }
    if plan.generator_origin.is_none() && !matches!(plan.candidate_origins.as_slice(), [_]) {
        tracing::info!(
            target: "soac_generator_state_lowering",
            caller = ?caller_id,
            source_instr_id = ?call.try_semantic_instr_id(),
            callee = ?plan.function_id,
            candidate_origins = ?plan.candidate_origins,
            "typed_generator_resume_inline_rejected_ambiguous_state_origin",
        );
        return None;
    }
    if typed_positional_arg_exprs(call.args.clone()).is_none() {
        return None;
    }
    Some(TypedInlineStoreCandidate {
        instr_index,
        result,
        call: TypedInlineCall::GeneratorResume(call.clone()),
        inline_plans: vec![TypedInlineDirectCallPlan {
            target: plan.function_id,
            arg_plan: TypedDirectCallArgPlan {
                sources: vec![
                    TypedDirectCallArgSource::Provided(1),
                    TypedDirectCallArgSource::Provided(2),
                    TypedDirectCallArgSource::Provided(3),
                    TypedDirectCallArgSource::Provided(4),
                ],
            },
            guard: TypedInlineGuardPlan::Direct,
        }],
    })
}

fn typed_inline_candidate_for_builtin_implementation_call(
    instr_index: usize,
    result: TypedInlineResult,
    call: &TypedCall<InstrTyped>,
    caller_id: RuntimeFunctionId,
) -> Option<TypedInlineStoreCandidate> {
    let plan = call.extra.builtin_implementation_plan()?;
    if plan.function_id == caller_id || !call.keywords.is_empty() {
        return None;
    }
    typed_positional_arg_exprs(call.args.clone())?;
    Some(TypedInlineStoreCandidate {
        instr_index,
        result,
        call: TypedInlineCall::BuiltinImplementation(call.clone()),
        inline_plans: vec![TypedInlineDirectCallPlan {
            target: plan.function_id,
            arg_plan: plan.arg_plan.clone(),
            guard: TypedInlineGuardPlan::Direct,
        }],
    })
}

fn typed_inline_candidate_for_guarded_builtin_implementation_call(
    instr_index: usize,
    result: TypedInlineResult,
    call: &TypedGuardedCallableCall<InstrTyped>,
    caller_id: RuntimeFunctionId,
) -> Option<TypedInlineStoreCandidate> {
    let plan = call.extra.builtin_implementation_plan()?;
    if plan.function_id == caller_id || !call.keywords.is_empty() {
        return None;
    }
    typed_positional_arg_exprs(call.args.clone())?;
    let mut generic_call = TypedCall::generic(
        call.func.as_ref().clone(),
        call.args.clone(),
        call.keywords.clone(),
    )
    .with_meta(call.meta());
    generic_call.extra = call.extra.clone();
    Some(TypedInlineStoreCandidate {
        instr_index,
        result,
        call: TypedInlineCall::BuiltinImplementation(generic_call),
        inline_plans: vec![TypedInlineDirectCallPlan {
            target: plan.function_id,
            arg_plan: plan.arg_plan.clone(),
            guard: TypedInlineGuardPlan::Direct,
        }],
    })
}

fn typed_inline_candidate_for_direct_builtin_implementation_call(
    instr_index: usize,
    result: TypedInlineResult,
    call: &TypedDirectCallableCall<InstrTyped>,
    caller_id: RuntimeFunctionId,
) -> Option<TypedInlineStoreCandidate> {
    let plan = call.extra.builtin_implementation_plan()?;
    if plan.function_id == caller_id {
        return None;
    }
    typed_positional_arg_exprs(call.args.clone())?;
    let mut generic_call =
        TypedCall::generic(call.func.as_ref().clone(), call.args.clone(), Vec::new())
            .with_meta(call.meta());
    generic_call.extra = call.extra.clone();
    Some(TypedInlineStoreCandidate {
        instr_index,
        result,
        call: TypedInlineCall::BuiltinImplementation(generic_call),
        inline_plans: vec![TypedInlineDirectCallPlan {
            target: plan.function_id,
            arg_plan: plan.arg_plan.clone(),
            guard: TypedInlineGuardPlan::Direct,
        }],
    })
}

fn runtime_protocol_explicit_args(
    call: &TypedCall<InstrTyped>,
) -> Option<&[CallArgPositional<InstrTyped>]> {
    match &call.access {
        TypedCallAccessPlan::GuardedRuntimeProtocolMethod { .. } | TypedCallAccessPlan::Generic => {
        }
        _ => return None,
    }
    if call.args.is_empty() {
        return None;
    }
    Some(&call.args[1..])
}

fn runtime_protocol_receiver(call: &TypedCall<InstrTyped>) -> Option<&InstrTyped> {
    let CallArgPositional::Positional(receiver) = call.args.first()? else {
        return None;
    };
    Some(receiver)
}

fn typed_inline_candidate_for_runtime_protocol_call(
    instr_index: usize,
    result: TypedInlineResult,
    call: &TypedCall<InstrTyped>,
    caller_id: RuntimeFunctionId,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
    trusted_direct_method_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
) -> Option<TypedInlineStoreCandidate> {
    let instr_id = call.try_semantic_instr_id()?;
    let receiver = runtime_protocol_receiver(call)?.clone();
    typed_positional_arg_exprs(runtime_protocol_explicit_args(call)?.to_vec())?;
    let plans = direct_calls_by_instr_id.get(&instr_id)?;
    if let Some(owner_type_ref) = trusted_direct_method_calls.get(&instr_id) {
        if matches!(call.access, TypedCallAccessPlan::Generic) {
            if let [(target, arg_plan)] = plans.as_slice() {
                if *target == caller_id {
                    return None;
                }
                return Some(TypedInlineStoreCandidate {
                    instr_index,
                    result,
                    call: TypedInlineCall::DirectRuntimeProtocolMethod {
                        call: call.clone(),
                        receiver,
                    },
                    inline_plans: vec![TypedInlineDirectCallPlan {
                        target: *target,
                        arg_plan: arg_plan.clone(),
                        guard: TypedInlineGuardPlan::Direct,
                    }],
                });
            }
        }
        let TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
            runtime_name: _,
            method_name: _,
            method_guards,
        } = &call.access
        else {
            return None;
        };
        let direct_plans = plans
            .iter()
            .filter_map(|(target, arg_plan)| {
                if *target == caller_id {
                    return None;
                }
                method_guards
                    .iter()
                    .find(|guard| {
                        guard.function_id == *target
                            && guard.arg_plan == *arg_plan
                            && guard.owner_type_ref == *owner_type_ref
                    })
                    .map(|_| TypedInlineDirectCallPlan {
                        target: *target,
                        arg_plan: arg_plan.clone(),
                        guard: TypedInlineGuardPlan::Direct,
                    })
            })
            .collect::<Vec<_>>();
        if let [direct_plan] = direct_plans.as_slice() {
            return Some(TypedInlineStoreCandidate {
                instr_index,
                result,
                call: TypedInlineCall::DirectRuntimeProtocolMethod {
                    call: call.clone(),
                    receiver,
                },
                inline_plans: vec![direct_plan.clone()],
            });
        }
    }
    let TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
        runtime_name: _,
        method_name: _,
        method_guards,
    } = &call.access
    else {
        return None;
    };
    let inline_plans = plans
        .iter()
        .filter_map(|(target, arg_plan)| {
            if *target == caller_id {
                return None;
            }
            let guard = method_guards
                .iter()
                .find(|guard| guard.function_id == *target)?;
            Some(TypedInlineDirectCallPlan {
                target: *target,
                arg_plan: arg_plan.clone(),
                guard: TypedInlineGuardPlan::Method(guard.clone()),
            })
        })
        .collect::<Vec<_>>();
    (!inline_plans.is_empty()).then_some(TypedInlineStoreCandidate {
        instr_index,
        result,
        call: TypedInlineCall::RuntimeProtocolMethod {
            call: call.clone(),
            receiver,
        },
        inline_plans,
    })
}

#[derive(Clone, Copy)]
struct TypedInlineCalleeRef<'a> {
    function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: Option<&'a [ConstantExpr]>,
}

fn typed_inline_callee<'a>(
    module: &'a BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: TypedInlineExternalCallees<'a>,
    function_id: RuntimeFunctionId,
) -> Option<TypedInlineCalleeRef<'a>> {
    module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .map(|function| TypedInlineCalleeRef {
            function,
            module_constants: None,
        })
        .or_else(|| match external_callees {
            TypedInlineExternalCallees::Plain(external_callees) => external_callees
                .get(&function_id)
                .map(|function| TypedInlineCalleeRef {
                    function,
                    module_constants: None,
                }),
            TypedInlineExternalCallees::Contextual(external_callees) => external_callees
                .get(&function_id)
                .map(|callee| TypedInlineCalleeRef {
                    function: &callee.function,
                    module_constants: Some(callee.module_constants.as_slice()),
                }),
        })
}

fn typed_positional_arg_exprs(args: Vec<CallArgPositional<InstrTyped>>) -> Option<Vec<InstrTyped>> {
    args.into_iter()
        .map(|arg| match arg {
            CallArgPositional::Positional(expr) => Some(expr),
            CallArgPositional::Starred(_) => None,
        })
        .collect()
}

fn typed_direct_call_guard_term(
    callable_temp: &ResolvedName,
    function_id: RuntimeFunctionId,
    source_meta: Meta,
    guard_miss_deopt: bool,
    then_label: BlockLabel,
    else_label: BlockLabel,
) -> BlockTerm<InstrTyped> {
    let mut guard = TypedDirectCallGuardTest::new(
        typed_load_temp(callable_temp),
        TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id },
    );
    if guard_miss_deopt {
        guard.extra.set_guard_miss_deopt_enabled(true);
    }
    BlockTerm::IfTerm(TermIf {
        test: InstrTyped::DirectCallGuardTest(guard.with_meta(source_meta)),
        then_label,
        else_label,
    })
}

fn typed_inline_guard_term(
    call: &TypedInlineCall,
    plan: &TypedInlineDirectCallPlan,
    callable_temp: Option<&TypedTempLocal>,
    receiver_temp: Option<&TypedTempLocal>,
    source_meta: Meta,
    then_label: BlockLabel,
    else_label: BlockLabel,
) -> BlockTerm<InstrTyped> {
    match (&plan.guard, call) {
        (TypedInlineGuardPlan::Direct, TypedInlineCall::DirectCallable(_)) => {
            let callable_temp = callable_temp
                .expect("direct callable inline guard requires callable temp")
                .resolved_name();
            typed_direct_call_guard_term(
                &callable_temp,
                plan.target,
                source_meta,
                false,
                then_label,
                else_label,
            )
        }
        (
            TypedInlineGuardPlan::Direct,
            TypedInlineCall::BuiltinImplementation(_)
            | TypedInlineCall::DirectMethod { .. }
            | TypedInlineCall::DirectRuntimeProtocolMethod { .. }
            | TypedInlineCall::GeneratorResume(_),
        ) => BlockTerm::Jump(BlockEdge::new(then_label)),
        (TypedInlineGuardPlan::Callable, TypedInlineCall::Callable(_)) => {
            let callable_temp = callable_temp
                .expect("callable inline guard requires callable temp")
                .resolved_name();
            typed_direct_call_guard_term(
                &callable_temp,
                plan.target,
                source_meta,
                true,
                then_label,
                else_label,
            )
        }
        (
            TypedInlineGuardPlan::Method(guard),
            TypedInlineCall::Method { .. } | TypedInlineCall::RuntimeProtocolMethod { .. },
        ) => {
            let receiver_temp = receiver_temp
                .expect("method inline guard requires receiver temp")
                .resolved_name();
            let mut guard = TypedDirectCallGuardTest::new(
                typed_load_temp(&receiver_temp),
                TypedDirectCallGuardTestKind::ExactTypeVersion {
                    function_id: plan.target,
                    owner_type_ref: guard.owner_type_ref.clone(),
                    type_version: guard.type_version,
                },
            );
            guard.extra.set_guard_miss_deopt_enabled(true);
            BlockTerm::IfTerm(TermIf {
                test: InstrTyped::DirectCallGuardTest(guard.with_meta(source_meta)),
                then_label,
                else_label,
            })
        }
        _ => unreachable!("inline guard kind must match inline call kind"),
    }
}

fn typed_generic_call_fallback_body(
    target: &ResolvedName,
    callable_temp: &ResolvedName,
    arg_temps: &[TypedTempLocal],
    discard_result: Option<&ResolvedName>,
) -> Vec<InstrTyped> {
    let mut body = vec![
        Store::new(
            target.clone(),
            Box::new(InstrTyped::CallTyped(TypedCall::generic(
                typed_load_temp(callable_temp),
                typed_load_temp_args(arg_temps),
                Vec::<CallArgKeyword<InstrTyped>>::new(),
            ))),
        )
        .with_meta(Meta::synthetic())
        .into(),
    ];
    if let Some(discard_result) = discard_result {
        append_typed_cleanup_del_to_body(&mut body, discard_result);
    }
    append_typed_cleanup_dels_to_body(&mut body, arg_temps);
    append_typed_cleanup_del_to_body(&mut body, callable_temp);
    body
}

fn typed_inline_generic_fallback_body(
    call: &TypedInlineCall,
    target: &ResolvedName,
    callable_temp: Option<&TypedTempLocal>,
    receiver_temp: Option<&TypedTempLocal>,
    arg_temps: &[TypedTempLocal],
    discard_result: Option<&ResolvedName>,
) -> Vec<InstrTyped> {
    match call {
        TypedInlineCall::BuiltinImplementation(_) => {
            unreachable!("builtin implementation inlining does not emit a generic fallback")
        }
        TypedInlineCall::DirectMethod { .. } => {
            unreachable!("direct method inlining does not emit a generic fallback")
        }
        TypedInlineCall::DirectRuntimeProtocolMethod { .. } => {
            unreachable!("direct runtime-protocol inlining does not emit a generic fallback")
        }
        TypedInlineCall::GeneratorResume(_) => {
            unreachable!("generator-resume inlining does not emit a generic fallback")
        }
        TypedInlineCall::DirectCallable(_) | TypedInlineCall::Callable(_) => {
            let callable_temp = callable_temp
                .expect("callable inline fallback requires callable temp")
                .resolved_name();
            typed_generic_call_fallback_body(target, &callable_temp, arg_temps, discard_result)
        }
        TypedInlineCall::Method { attr, .. } => {
            let receiver_temp = receiver_temp
                .expect("method inline fallback requires receiver temp")
                .resolved_name();
            let func = InstrTyped::GetAttrTyped(
                TypedGetAttr::generic(typed_load_temp(&receiver_temp), attr.clone())
                    .with_meta(Meta::synthetic()),
            );
            let mut body = vec![
                Store::new(
                    target.clone(),
                    Box::new(InstrTyped::CallTyped(TypedCall::generic(
                        func,
                        typed_load_temp_args(arg_temps),
                        Vec::<CallArgKeyword<InstrTyped>>::new(),
                    ))),
                )
                .with_meta(Meta::synthetic())
                .into(),
            ];
            if let Some(discard_result) = discard_result {
                append_typed_cleanup_del_to_body(&mut body, discard_result);
            }
            append_typed_cleanup_dels_to_body(&mut body, arg_temps);
            append_typed_cleanup_del_to_body(&mut body, &receiver_temp);
            body
        }
        TypedInlineCall::RuntimeProtocolMethod { call, .. } => {
            let receiver_temp = receiver_temp
                .expect("runtime protocol inline fallback requires receiver temp")
                .resolved_name();
            let mut args = Vec::with_capacity(1 + arg_temps.len());
            args.push(CallArgPositional::Positional(typed_load_temp(
                &receiver_temp,
            )));
            args.extend(typed_load_temp_args(arg_temps));
            let mut fallback_call =
                TypedCall::generic(call.func.as_ref().clone(), args, Vec::new());
            fallback_call = fallback_call.with_meta(call.meta());
            let mut body = vec![
                Store::new(
                    target.clone(),
                    Box::new(InstrTyped::CallTyped(fallback_call)),
                )
                .with_meta(Meta::synthetic())
                .into(),
            ];
            if let Some(discard_result) = discard_result {
                append_typed_cleanup_del_to_body(&mut body, discard_result);
            }
            append_typed_cleanup_dels_to_body(&mut body, arg_temps);
            append_typed_cleanup_del_to_body(&mut body, &receiver_temp);
            body
        }
    }
}

fn typed_load_temp(temp_name: &ResolvedName) -> InstrTyped {
    InstrTyped::Load(Load::new(temp_name.clone()).with_meta(Meta::synthetic()))
}

fn typed_store_temp(temp_name: ResolvedName, value: InstrTyped) -> InstrTyped {
    Store::new(temp_name, Box::new(value))
        .with_meta(Meta::synthetic())
        .into()
}

fn typed_load_temp_args(temp_names: &[TypedTempLocal]) -> Vec<CallArgPositional<InstrTyped>> {
    temp_names
        .iter()
        .map(|temp| CallArgPositional::Positional(typed_load_temp(&temp.resolved_name())))
        .collect()
}

fn append_typed_cleanup_dels_to_body(body: &mut Vec<InstrTyped>, temp_names: &[TypedTempLocal]) {
    for temp_name in temp_names.iter().rev() {
        append_typed_cleanup_del_to_body(body, &temp_name.resolved_name());
    }
}

fn append_typed_cleanup_del_to_body(body: &mut Vec<InstrTyped>, temp_name: &ResolvedName) {
    body.push(
        Del::new(temp_name.clone(), true)
            .with_meta(Meta::synthetic())
            .into(),
    );
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct TypedTempLocal {
    name: String,
    location: LocalLocation,
}

impl TypedTempLocal {
    fn resolved_name(&self) -> ResolvedName {
        ResolvedName {
            id: self.name.clone().into(),
            location: NameLocation::Local(self.location),
        }
    }
}

fn try_allocate_typed_stack_temp(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    prefix: &str,
) -> Result<TypedTempLocal, TypedInlineUnsupportedReason> {
    let name = function.name_gen.next_tmp_name(prefix).as_str().to_string();
    let layout = function
        .storage_layout
        .as_mut()
        .ok_or(TypedInlineUnsupportedReason::MissingCallerStorageLayout)?;
    let location = LocalLocation(
        u32::try_from(layout.stack_slots().len())
            .expect("typed stack slot index should fit in u32"),
    );
    layout.ensure_stack_slot(name.clone());
    Ok(TypedTempLocal { name, location })
}

type TypedInlineValueBindings = HashMap<LocalLocation, InstrTyped>;

fn expand_synthetic_typed_starred_tuple_args(
    args: Vec<CallArgPositional<InstrTyped>>,
) -> Vec<CallArgPositional<InstrTyped>> {
    let mut expanded = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            CallArgPositional::Starred(InstrTyped::Tuple(tuple))
                if tuple.meta().instr_id.is_none() =>
            {
                expanded.extend(tuple.values.into_iter().map(CallArgPositional::Positional));
            }
            other => expanded.push(other),
        }
    }
    expanded
}

fn typed_inline_provided_values(
    call: &TypedInlineCall,
    receiver_temp: &Option<TypedTempLocal>,
    arg_temps: &[TypedTempLocal],
) -> Vec<InstrTyped> {
    let mut values = Vec::with_capacity(
        arg_temps.len()
            + usize::from(matches!(
                call,
                TypedInlineCall::Method { .. }
                    | TypedInlineCall::DirectMethod { .. }
                    | TypedInlineCall::RuntimeProtocolMethod { .. }
                    | TypedInlineCall::DirectRuntimeProtocolMethod { .. }
            )),
    );
    if matches!(
        call,
        TypedInlineCall::Method { .. }
            | TypedInlineCall::DirectMethod { .. }
            | TypedInlineCall::RuntimeProtocolMethod { .. }
            | TypedInlineCall::DirectRuntimeProtocolMethod { .. }
    ) {
        let receiver_temp = receiver_temp
            .as_ref()
            .expect("method inline candidate should have receiver temp");
        values.push(typed_load_temp(&receiver_temp.resolved_name()));
    }
    values.extend(
        arg_temps
            .iter()
            .map(|temp| typed_load_temp(&temp.resolved_name())),
    );
    values
}

fn bind_typed_direct_call_inline_values(
    callee: &BlockPyFunction<TypedBlockPyModuleShape>,
    arg_plan: &TypedDirectCallArgPlan,
    values: &[InstrTyped],
) -> Result<TypedInlineValueBindings, TypedInlineUnsupportedReason> {
    if arg_plan.sources.len() != callee.params.len() {
        return Err(TypedInlineUnsupportedReason::ArityMismatch);
    }
    let mut bindings = TypedInlineValueBindings::new();
    for (param, source) in callee.params.iter().zip(&arg_plan.sources) {
        let value = match (param.kind, source) {
            (ParamKind::PosOnly | ParamKind::Any, TypedDirectCallArgSource::Provided(index)) => {
                values
                    .get(*index)
                    .cloned()
                    .ok_or(TypedInlineUnsupportedReason::ArityMismatch)?
            }
            (ParamKind::VarArg, TypedDirectCallArgSource::PackedRest { start }) => {
                let rest = values
                    .get(*start..)
                    .ok_or(TypedInlineUnsupportedReason::ArityMismatch)?
                    .to_vec();
                InstrTyped::Tuple(Tuple::new(rest))
            }
            (_, TypedDirectCallArgSource::DefaultSentinel) => {
                return Err(TypedInlineUnsupportedReason::DefaultArguments);
            }
            (_, _) => return Err(TypedInlineUnsupportedReason::UnsupportedParameterKind),
        };
        let location = typed_parameter_local_location(callee, &param.name)?;
        bindings.insert(location, value);
    }
    Ok(bindings)
}

fn bind_typed_generator_resume_inline_values(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    callee: &BlockPyFunction<TypedBlockPyModuleShape>,
    arg_plan: &TypedDirectCallArgPlan,
    values: &[InstrTyped],
) -> Result<
    (
        TypedInlineValueBindings,
        HashMap<LocalLocation, TypedTempLocal>,
        Vec<InstrTyped>,
        Vec<TypedTempLocal>,
    ),
    TypedInlineUnsupportedReason,
> {
    if arg_plan.sources.len() != callee.body_params().len() {
        return Err(TypedInlineUnsupportedReason::ArityMismatch);
    }
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    let needs_preserved_owner = !callee_layout.preserved_slots.is_empty();
    let mut preassigned_locals = HashMap::new();
    let mut prologue = Vec::new();
    let mut preserved_abi_temps = Vec::new();
    let mut bindings = TypedInlineValueBindings::new();
    for (param, source) in callee.body_params().iter().zip(&arg_plan.sources) {
        let location = typed_parameter_local_location(callee, &param.name)?;
        if needs_preserved_owner && matches!(param.name.as_str(), "_dp_self" | "_dp_state") {
            let owner = allocate_typed_preserved_abi_local(caller)?;
            let value = typed_inline_value_for_arg_source(param.kind, source, values)?;
            prologue.push(typed_store_temp(owner.resolved_name(), value));
            preassigned_locals.insert(location, owner.clone());
            preserved_abi_temps.push(owner);
            continue;
        }
        bindings.insert(
            location,
            typed_inline_value_for_arg_source(param.kind, source, values)?,
        );
    }
    Ok((bindings, preassigned_locals, prologue, preserved_abi_temps))
}

fn typed_generator_resume_inline_closure_bindings(
    caller: &BlockPyFunction<TypedBlockPyModuleShape>,
    callee: &BlockPyFunction<TypedBlockPyModuleShape>,
    call: &TypedCall<InstrTyped>,
    materialized_generator_constructors: &HashMap<InstrId, TypedGeneratorStateConstructor>,
) -> Result<HashMap<u32, CellLocation>, TypedInlineUnsupportedReason> {
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    if !callee_layout.cellvars.is_empty() {
        return Err(TypedInlineUnsupportedReason::UnsupportedGeneratorClosureCapture);
    }
    if callee_layout.freevars.is_empty() {
        return Ok(HashMap::new());
    }
    let resume_plan = call
        .extra
        .generator_resume_plan()
        .ok_or(TypedInlineUnsupportedReason::UnsupportedGeneratorClosureCapture)?;
    if let Some(generator_origin) = resume_plan.generator_origin {
        if let Some((_, _, _, constructor_call)) =
            find_typed_generator_constructor_store(caller, generator_origin)
            && let Some(bindings) = typed_inline_capture_cell_bindings_for_generator_constructor(
                caller,
                constructor_call.func.as_ref(),
                callee.function_id,
                callee_layout.freevars.len(),
            )
        {
            return Ok(bindings);
        }
        if let Some(constructor) = materialized_generator_constructors.get(&generator_origin)
            && let Some(bindings) = constructor.closure_cell_bindings.as_ref()
            && bindings.len() == callee_layout.freevars.len()
        {
            tracing::info!(
                target: "soac_generator_state_lowering",
                caller = ?caller.function_id,
                generator_origin = ?generator_origin,
                callee = ?callee.function_id,
                bindings = ?bindings,
                "typed_generator_resume_capture_bindings_used_materialized_snapshot",
            );
            return Ok(bindings.clone());
        }
        if let Some(constructor) = materialized_generator_constructors.get(&generator_origin)
            && let Some(bindings) = typed_inline_capture_cell_bindings_for_generator_constructor(
                caller,
                constructor.call.func.as_ref(),
                callee.function_id,
                callee_layout.freevars.len(),
            )
        {
            tracing::info!(
                target: "soac_generator_state_lowering",
                caller = ?caller.function_id,
                generator_origin = ?generator_origin,
                callee = ?callee.function_id,
                bindings = ?bindings,
                "typed_generator_resume_capture_bindings_used_materialized_constructor",
            );
            return Ok(bindings);
        }
    }
    if let Some(bindings) = typed_inline_resume_capture_cell_bindings_from_candidate_origins(
        caller,
        materialized_generator_constructors,
        callee.function_id,
        callee_layout.freevars.len(),
        &resume_plan.candidate_origins,
    )
    .or_else(|| {
        typed_inline_resume_capture_cell_bindings_from_compatible_materialized_snapshots(
            materialized_generator_constructors,
            callee.function_id,
            callee_layout.freevars.len(),
        )
    })
    .or_else(|| {
        typed_inline_resume_capture_cell_bindings_from_compatible_materializations(
            caller,
            callee.function_id,
            callee_layout.freevars.len(),
        )
    }) {
        tracing::info!(
            target: "soac_generator_state_lowering",
            caller = ?caller.function_id,
            callee = ?callee.function_id,
            bindings = ?bindings,
            "typed_generator_resume_capture_bindings_used_compatible_materializations",
        );
        return Ok(bindings);
    }
    Err(TypedInlineUnsupportedReason::UnsupportedGeneratorClosureCapture)
}

fn typed_inline_resume_capture_cell_bindings_from_candidate_origins(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    materialized_generator_constructors: &HashMap<InstrId, TypedGeneratorStateConstructor>,
    callee_function_id: RuntimeFunctionId,
    expected_capture_count: usize,
    candidate_origins: &[InstrId],
) -> Option<HashMap<u32, CellLocation>> {
    let mut expected = None;
    for generator_origin in candidate_origins {
        let bindings = if let Some((_, _, _, constructor_call)) =
            find_typed_generator_constructor_store(function, *generator_origin)
        {
            typed_inline_capture_cell_bindings_for_generator_constructor(
                function,
                constructor_call.func.as_ref(),
                callee_function_id,
                expected_capture_count,
            )
        } else {
            materialized_generator_constructors
                .get(generator_origin)
                .and_then(|constructor| {
                    constructor
                        .closure_cell_bindings
                        .as_ref()
                        .filter(|bindings| bindings.len() == expected_capture_count)
                        .cloned()
                        .or_else(|| {
                            typed_inline_capture_cell_bindings_for_generator_constructor(
                                function,
                                constructor.call.func.as_ref(),
                                callee_function_id,
                                expected_capture_count,
                            )
                        })
                })
        }?;
        if let Some(expected) = expected.as_ref()
            && bindings != *expected
        {
            return None;
        }
        expected.get_or_insert_with(|| bindings.clone());
    }
    expected
}

fn typed_inline_resume_capture_cell_bindings_from_compatible_materialized_snapshots(
    materialized_generator_constructors: &HashMap<InstrId, TypedGeneratorStateConstructor>,
    callee_function_id: RuntimeFunctionId,
    expected_capture_count: usize,
) -> Option<HashMap<u32, CellLocation>> {
    let mut expected = None;
    for constructor in materialized_generator_constructors.values() {
        let Some(function_id) = constructor
            .call
            .extra
            .generator_instance_plan()
            .map(|plan| plan.function_id)
        else {
            continue;
        };
        if function_id != callee_function_id {
            continue;
        }
        let Some(bindings) = constructor
            .closure_cell_bindings
            .as_ref()
            .filter(|bindings| bindings.len() == expected_capture_count)
        else {
            continue;
        };
        if let Some(expected) = expected.as_ref()
            && bindings != expected
        {
            return None;
        }
        expected.get_or_insert_with(|| bindings.clone());
    }
    expected
}

fn typed_inline_resume_capture_cell_bindings_from_compatible_materializations(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    callee_function_id: RuntimeFunctionId,
    expected_capture_count: usize,
) -> Option<HashMap<u32, CellLocation>> {
    let mut expected = None;
    let mut compatible_materializations = 0;
    for expr in function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|instr| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            Some(store.value.as_ref())
        })
    {
        if typed_make_function_capture_count(expr, callee_function_id)
            != Some(expected_capture_count)
        {
            continue;
        }
        let bindings = typed_inline_capture_cell_bindings_from_make_function_expr(
            function,
            expr,
            callee_function_id,
            expected_capture_count,
        )?;
        if let Some(expected) = expected.as_ref()
            && bindings != *expected
        {
            tracing::info!(
                target: "soac_generator_state_lowering",
                callee_function_id = ?callee_function_id,
                expected_bindings = ?expected,
                conflicting_bindings = ?bindings,
                "typed_generator_resume_capture_bindings_conflicting_materializations",
            );
            return None;
        }
        expected.get_or_insert_with(|| bindings.clone());
        compatible_materializations += 1;
    }
    let bindings = expected?;
    tracing::info!(
        target: "soac_generator_state_lowering",
        callee_function_id = ?callee_function_id,
        compatible_materializations,
        "typed_generator_resume_capture_bindings_materializations_snapshotted",
    );
    Some(bindings)
}

fn typed_inline_capture_cell_bindings_for_generator_constructor(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    constructor_func: &InstrTyped,
    callee_function_id: RuntimeFunctionId,
    expected_capture_count: usize,
) -> Option<HashMap<u32, CellLocation>> {
    if let Some(bindings) = typed_inline_capture_cell_bindings_from_make_function_expr(
        function,
        constructor_func,
        callee_function_id,
        expected_capture_count,
    ) {
        return Some(bindings);
    }

    let InstrTyped::Load(load) = constructor_func else {
        tracing::info!(
            target: "soac_generator_state_lowering",
            callee_function_id = ?callee_function_id,
            constructor_func = ?constructor_func,
            "typed_generator_resume_capture_bindings_not_constructor_load",
        );
        return None;
    };
    let Some(bindings) = typed_inline_constructor_store_capture_bindings(
        function,
        load,
        callee_function_id,
        expected_capture_count,
    ) else {
        tracing::info!(
            target: "soac_generator_state_lowering",
            callee_function_id = ?callee_function_id,
            constructor_load = ?load,
            "typed_generator_resume_capture_bindings_missing_compatible_constructor_stores",
        );
        return None;
    };
    Some(bindings)
}

fn typed_inline_generator_constructor_capture_bindings_snapshot(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    constructor_func: &InstrTyped,
    callee_function_id: RuntimeFunctionId,
) -> Option<HashMap<u32, CellLocation>> {
    if let Some(expected_capture_count) =
        typed_make_function_capture_count(constructor_func, callee_function_id)
    {
        return typed_inline_capture_cell_bindings_from_make_function_expr(
            function,
            constructor_func,
            callee_function_id,
            expected_capture_count,
        );
    }

    let InstrTyped::Load(load) = constructor_func else {
        return None;
    };
    let local_location = load.name.local_location();
    let preserved_location = load.name.preserved_location();
    if local_location.is_none() && preserved_location.is_none() {
        return None;
    }

    let mut expected = None;
    let mut compatible_store_count = 0;
    for store_value in function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|instr| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            let same_local =
                local_location.is_some() && store.name.local_location() == local_location;
            let same_preserved = preserved_location.is_some()
                && store.name.preserved_location() == preserved_location;
            (same_local || same_preserved).then_some(store.value.as_ref())
        })
    {
        let Some(expected_capture_count) =
            typed_make_function_capture_count(store_value, callee_function_id)
        else {
            continue;
        };
        let bindings = typed_inline_capture_cell_bindings_from_make_function_expr(
            function,
            store_value,
            callee_function_id,
            expected_capture_count,
        )?;
        if let Some(expected) = expected.as_ref()
            && bindings != *expected
        {
            tracing::info!(
                target: "soac_generator_state_lowering",
                callee_function_id = ?callee_function_id,
                constructor_load = ?load,
                expected_bindings = ?expected,
                conflicting_bindings = ?bindings,
                "typed_generator_state_constructor_snapshot_conflicting_capture_bindings",
            );
            return None;
        }
        expected.get_or_insert_with(|| bindings.clone());
        compatible_store_count += 1;
    }
    let snapshot = expected?;
    tracing::info!(
        target: "soac_generator_state_lowering",
        callee_function_id = ?callee_function_id,
        constructor_load = ?load,
        compatible_store_count,
        "typed_generator_state_constructor_capture_bindings_snapshotted",
    );
    Some(snapshot)
}

fn typed_make_function_capture_count(
    expr: &InstrTyped,
    callee_function_id: RuntimeFunctionId,
) -> Option<usize> {
    let InstrTyped::MakeFunctionWithClosure(make_function) = expr else {
        return None;
    };
    if make_function.function_id() != callee_function_id {
        return None;
    }
    let InstrTyped::Tuple(captures) = make_function.captures.as_ref() else {
        return None;
    };
    Some(captures.values.len())
}

fn typed_inline_constructor_store_capture_bindings(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    load: &Load<InstrTyped>,
    callee_function_id: RuntimeFunctionId,
    expected_capture_count: usize,
) -> Option<HashMap<u32, CellLocation>> {
    let local_location = load.name.local_location();
    let preserved_location = load.name.preserved_location();
    if local_location.is_none() && preserved_location.is_none() {
        return None;
    }
    let store_values = function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|instr| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            let same_local =
                local_location.is_some() && store.name.local_location() == local_location;
            let same_preserved = preserved_location.is_some()
                && store.name.preserved_location() == preserved_location;
            (same_local || same_preserved).then_some(store.value.as_ref())
        })
        .collect::<Vec<_>>();
    let mut expected = None;
    let mut compatible_store_count = 0;
    let mut ignored_store_count = 0;
    for store_value in store_values.iter().copied() {
        if !matches!(store_value, InstrTyped::MakeFunctionWithClosure(_)) {
            ignored_store_count += 1;
            continue;
        }
        let Some(bindings) = typed_inline_capture_cell_bindings_from_make_function_expr(
            function,
            store_value,
            callee_function_id,
            expected_capture_count,
        ) else {
            return None;
        };
        if let Some(expected) = expected.as_ref()
            && bindings != *expected
        {
            tracing::info!(
                target: "soac_generator_state_lowering",
                callee_function_id = ?callee_function_id,
                constructor_load = ?load,
                expected_bindings = ?expected,
                conflicting_bindings = ?bindings,
                "typed_generator_resume_capture_bindings_conflicting_constructor_store_bindings",
            );
            return None;
        }
        expected.get_or_insert_with(|| bindings.clone());
        compatible_store_count += 1;
    }
    let Some(expected) = expected else {
        tracing::info!(
            target: "soac_generator_state_lowering",
            callee_function_id = ?callee_function_id,
            constructor_load = ?load,
            candidate_store_count = store_values.len(),
            compatible_store_count,
            ignored_store_count,
            "typed_generator_resume_capture_bindings_no_make_function_constructor_store",
        );
        return None;
    };
    if compatible_store_count == 1 {
        tracing::info!(
            target: "soac_generator_state_lowering",
            callee_function_id = ?callee_function_id,
            constructor_load = ?load,
            ignored_store_count,
            "typed_generator_resume_capture_bindings_used_single_constructor_store",
        );
    } else {
        tracing::info!(
            target: "soac_generator_state_lowering",
            callee_function_id = ?callee_function_id,
            constructor_load = ?load,
            compatible_store_count,
            ignored_store_count,
            "typed_generator_resume_capture_bindings_used_compatible_constructor_stores",
        );
    }
    Some(expected)
}

fn typed_inline_capture_cell_bindings_from_make_function_expr(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    expr: &InstrTyped,
    callee_function_id: RuntimeFunctionId,
    expected_capture_count: usize,
) -> Option<HashMap<u32, CellLocation>> {
    let InstrTyped::MakeFunctionWithClosure(make_function) = expr else {
        tracing::info!(
            target: "soac_generator_state_lowering",
            callee_function_id = ?callee_function_id,
            constructor_expr = ?expr,
            "typed_generator_resume_capture_bindings_constructor_store_not_make_function",
        );
        return None;
    };
    if make_function.function_id() != callee_function_id {
        tracing::info!(
            target: "soac_generator_state_lowering",
            callee_function_id = ?callee_function_id,
            constructor_function_id = ?make_function.function_id(),
            "typed_generator_resume_capture_bindings_function_id_mismatch",
        );
        return None;
    }
    let InstrTyped::Tuple(captures) = make_function.captures.as_ref() else {
        tracing::info!(
            target: "soac_generator_state_lowering",
            callee_function_id = ?callee_function_id,
            captures = ?make_function.captures,
            "typed_generator_resume_capture_bindings_captures_not_tuple",
        );
        return None;
    };
    if captures.values.len() != expected_capture_count {
        tracing::info!(
            target: "soac_generator_state_lowering",
            callee_function_id = ?callee_function_id,
            expected_capture_count,
            actual_capture_count = captures.values.len(),
            "typed_generator_resume_capture_bindings_capture_count_mismatch",
        );
        return None;
    }

    captures
        .values
        .iter()
        .enumerate()
        .map(|(slot, capture)| {
            let InstrTyped::Tuple(capture_tuple) = capture else {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    callee_function_id = ?callee_function_id,
                    slot,
                    capture = ?capture,
                    "typed_generator_resume_capture_bindings_capture_not_tuple",
                );
                return None;
            };
            let [_, capture_cell] = capture_tuple.values.as_slice() else {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    callee_function_id = ?callee_function_id,
                    slot,
                    capture = ?capture_tuple.values,
                    "typed_generator_resume_capture_bindings_capture_tuple_bad_arity",
                );
                return None;
            };
            let Some(location) = typed_inline_capture_cell_location(function, capture_cell) else {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    callee_function_id = ?callee_function_id,
                    slot,
                    capture_cell = ?capture_cell,
                    "typed_generator_resume_capture_bindings_capture_cell_unresolved",
                );
                return None;
            };
            Some((u32::try_from(slot).ok()?, location))
        })
        .collect()
}

fn typed_inline_capture_cell_location(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    expr: &InstrTyped,
) -> Option<CellLocation> {
    match expr {
        InstrTyped::CellRef(cell_ref) => Some(cell_ref.location),
        InstrTyped::Load(load) => {
            if let Some(location) = load.name.location.as_cell() {
                return Some(location);
            }
            if let Some(location) = load.name.preserved_location() {
                let preserved_slot = function
                    .storage_layout
                    .as_ref()?
                    .preserved_slots
                    .get(usize::try_from(location.0).ok()?)?;
                return (preserved_slot.storage == PreservedSlotStorage::PyCellObject)
                    .then_some(CellLocation::Preserved(location.0));
            }
            let storage_layout = function.storage_layout.as_ref()?;
            let load_name = load.name.id_str();
            let slot = storage_layout.cellvars.iter().position(|slot| {
                slot.storage_name == load_name || slot.logical_name == load_name
            })?;
            Some(CellLocation::Owned(u32::try_from(slot).ok()?))
        }
        _ => None,
    }
}

fn typed_inline_value_for_arg_source(
    param_kind: ParamKind,
    source: &TypedDirectCallArgSource,
    values: &[InstrTyped],
) -> Result<InstrTyped, TypedInlineUnsupportedReason> {
    match (param_kind, source) {
        (ParamKind::PosOnly | ParamKind::Any, TypedDirectCallArgSource::Provided(index)) => values
            .get(*index)
            .cloned()
            .ok_or(TypedInlineUnsupportedReason::ArityMismatch),
        (ParamKind::VarArg, TypedDirectCallArgSource::PackedRest { start }) => {
            let rest = values
                .get(*start..)
                .ok_or(TypedInlineUnsupportedReason::ArityMismatch)?
                .to_vec();
            Ok(InstrTyped::Tuple(Tuple::new(rest)))
        }
        (_, TypedDirectCallArgSource::DefaultSentinel) => {
            Err(TypedInlineUnsupportedReason::DefaultArguments)
        }
        (_, _) => Err(TypedInlineUnsupportedReason::UnsupportedParameterKind),
    }
}

fn allocate_typed_preserved_abi_local(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> Result<TypedTempLocal, TypedInlineUnsupportedReason> {
    // Each inlined resume body needs its own owner/state scratch local. Reusing a
    // caller-level `_dp_self` / `_dp_state` slot lets nested resume inlining
    // overwrite the outer body's preserved-state pointer mid-fragment.
    try_allocate_typed_stack_temp(caller, "typed_inline_preserved_abi")
}

fn typed_parameter_local_location(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    name: &str,
) -> Result<LocalLocation, TypedInlineUnsupportedReason> {
    let layout = function
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    let Some(slot) = layout
        .stack_slots()
        .iter()
        .position(|slot_name| slot_name == name)
    else {
        return Err(TypedInlineUnsupportedReason::MissingParameterLocal);
    };
    Ok(LocalLocation(
        u32::try_from(slot).expect("parameter stack slot index should fit in u32"),
    ))
}

fn build_typed_direct_call_inline_fragment_to_target(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    callee: &BlockPyFunction<TypedBlockPyModuleShape>,
    continuation: BlockLabel,
    value_bindings: &TypedInlineValueBindings,
    preassigned_locals: &HashMap<LocalLocation, TypedTempLocal>,
    return_target: ResolvedName,
    inline_instance: u32,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    callee_module_constants: Option<&[ConstantExpr]>,
    allow_nonstack_storage: bool,
    closure_cell_bindings: Option<&HashMap<u32, CellLocation>>,
) -> Result<TypedInlineFragment, TypedInlineUnsupportedReason> {
    if callee.blocks.len() == 1 {
        return build_single_block_typed_inline_fragment_to_target(
            caller,
            callee,
            continuation,
            value_bindings,
            preassigned_locals,
            return_target,
            inline_instance,
            instr_id_allocator,
            caller_module_constants,
            callee_module_constants,
            allow_nonstack_storage,
            closure_cell_bindings,
        );
    }
    build_multi_block_typed_inline_fragment_to_target(
        caller,
        callee,
        continuation,
        value_bindings,
        preassigned_locals,
        return_target,
        inline_instance,
        instr_id_allocator,
        caller_module_constants,
        callee_module_constants,
        allow_nonstack_storage,
        closure_cell_bindings,
    )
}

struct TypedInlineFragment {
    blocks: Vec<TypedBlock>,
    instr_id_mappings: Vec<TypedInlineInstrIdMapping>,
    synthetic_instr_ids: Vec<InstrId>,
    constant_mappings: Vec<TypedInlineConstantMapping>,
    local_mappings: Vec<TypedInlineLocalMapping>,
}

fn build_single_block_typed_inline_fragment_to_target(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    callee: &BlockPyFunction<TypedBlockPyModuleShape>,
    continuation: BlockLabel,
    value_bindings: &TypedInlineValueBindings,
    preassigned_locals: &HashMap<LocalLocation, TypedTempLocal>,
    return_target: ResolvedName,
    inline_instance: u32,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    callee_module_constants: Option<&[ConstantExpr]>,
    allow_nonstack_storage: bool,
    closure_cell_bindings: Option<&HashMap<u32, CellLocation>>,
) -> Result<TypedInlineFragment, TypedInlineUnsupportedReason> {
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    if !allow_nonstack_storage && typed_inline_callee_has_nonstack_storage(callee, callee_layout) {
        return Err(TypedInlineUnsupportedReason::NonStackStorage);
    }
    for location in value_bindings.keys().copied() {
        if location.slot() as usize >= callee_layout.stack_slots().len() {
            return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(location));
        }
    }
    if callee.blocks.len() != 1 {
        return Err(TypedInlineUnsupportedReason::MultipleBlocks);
    }
    let callee_block = &callee.blocks[0];
    if !callee_block.params.is_empty() {
        return Err(TypedInlineUnsupportedReason::BlockParams);
    }
    if callee_block.exc_edge.is_some() {
        return Err(TypedInlineUnsupportedReason::ExceptionEdge);
    }
    let BlockTerm::Return(return_value) = &callee_block.term else {
        return Err(TypedInlineUnsupportedReason::NonReturnTerm);
    };
    let locals =
        allocate_typed_inline_locals(caller, callee_layout, value_bindings, preassigned_locals)?;
    let local_mappings = typed_inline_local_mappings(
        callee.function_id,
        inline_instance,
        callee_layout,
        &locals,
        value_bindings,
    )?;
    let mut instr_id_remapper =
        TypedInlineInstrIdRemapper::new(callee.function_id, inline_instance, instr_id_allocator);
    let mut constant_scope =
        typed_inline_constant_scope(caller_module_constants, callee_module_constants)?;
    let mut remapper = TypedInlineLocalRemapper::new(
        callee_layout,
        &locals,
        value_bindings,
        closure_cell_bindings,
        &mut instr_id_remapper,
        &mut constant_scope,
    );
    let mut body = callee_block
        .body
        .iter()
        .cloned()
        .filter(|instr| !matches!(instr, InstrTyped::IncrementCounter(_)))
        .map(|instr| remapper.try_map_instr(instr))
        .collect::<Result<Vec<_>, _>>()?;
    let return_value = remapper.try_map_instr(return_value.clone())?;
    let return_meta = return_value.meta();
    let return_store = remapper.assign_fresh_instr_id(
        Store::new(return_target, Box::new(return_value))
            .with_meta(return_meta)
            .into(),
    );
    let synthetic_instr_ids = vec![
        return_store
            .try_semantic_instr_id()
            .expect("synthetic inline return store should carry an instruction id"),
    ];
    body.push(return_store);

    Ok(TypedInlineFragment {
        blocks: vec![Block::new_with_extra(
            caller.name_gen.next_block_name(),
            body,
            BlockTerm::Jump(BlockEdge::new(continuation)),
            Vec::new(),
            None,
            TypedBlockExtra::default(),
        )],
        instr_id_mappings: instr_id_remapper.finish(),
        synthetic_instr_ids,
        constant_mappings: constant_scope.mappings(callee.function_id, inline_instance),
        local_mappings,
    })
}

fn build_multi_block_typed_inline_fragment_to_target(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    callee: &BlockPyFunction<TypedBlockPyModuleShape>,
    continuation: BlockLabel,
    value_bindings: &TypedInlineValueBindings,
    preassigned_locals: &HashMap<LocalLocation, TypedTempLocal>,
    return_target: ResolvedName,
    inline_instance: u32,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    callee_module_constants: Option<&[ConstantExpr]>,
    allow_nonstack_storage: bool,
    closure_cell_bindings: Option<&HashMap<u32, CellLocation>>,
) -> Result<TypedInlineFragment, TypedInlineUnsupportedReason> {
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    if !allow_nonstack_storage && typed_inline_callee_has_nonstack_storage(callee, callee_layout) {
        return Err(TypedInlineUnsupportedReason::NonStackStorage);
    }
    for location in value_bindings.keys().copied() {
        if location.slot() as usize >= callee_layout.stack_slots().len() {
            return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(location));
        }
    }
    let locals =
        allocate_typed_inline_locals(caller, callee_layout, value_bindings, preassigned_locals)?;
    let local_mappings = typed_inline_local_mappings(
        callee.function_id,
        inline_instance,
        callee_layout,
        &locals,
        value_bindings,
    )?;
    let label_map = callee
        .blocks
        .iter()
        .map(|block| (block.label, caller.name_gen.next_block_name()))
        .collect::<HashMap<_, _>>();
    let mut instr_id_remapper =
        TypedInlineInstrIdRemapper::new(callee.function_id, inline_instance, instr_id_allocator);
    let mut constant_scope =
        typed_inline_constant_scope(caller_module_constants, callee_module_constants)?;
    let mut remapper = TypedInlineLocalRemapper::new(
        callee_layout,
        &locals,
        value_bindings,
        closure_cell_bindings,
        &mut instr_id_remapper,
        &mut constant_scope,
    );
    let mut blocks: Vec<TypedBlock> = Vec::with_capacity(callee.blocks.len());
    let mut synthetic_instr_ids = Vec::new();
    for callee_block in &callee.blocks {
        let label = typed_remapped_label(&label_map, callee_block.label)?;
        let mut body = callee_block
            .body
            .iter()
            .cloned()
            .filter(|instr| !matches!(instr, InstrTyped::IncrementCounter(_)))
            .map(|instr| remapper.try_map_instr(instr))
            .collect::<Result<Vec<_>, _>>()?;
        let term = match &callee_block.term {
            BlockTerm::Return(value) => {
                let return_value = remapper.try_map_instr(value.clone())?;
                let return_meta = return_value.meta();
                let return_store = remapper.assign_fresh_instr_id(
                    Store::new(return_target.clone(), Box::new(return_value))
                        .with_meta(return_meta)
                        .into(),
                );
                synthetic_instr_ids.push(
                    return_store
                        .try_semantic_instr_id()
                        .expect("synthetic inline return store should carry an instruction id"),
                );
                body.push(return_store);
                BlockTerm::Jump(BlockEdge::new(continuation))
            }
            term => typed_remap_inline_term_labels(
                remapper.try_map_term(term.clone())?,
                &label_map,
                &mut remapper,
            )?,
        };
        let params = callee_block
            .params
            .iter()
            .cloned()
            .map(|param| remapper.try_map_block_param(param))
            .collect::<Result<Vec<_>, _>>()?;
        let exc_edge = callee_block
            .exc_edge
            .clone()
            .map(|edge| typed_remap_inline_edge(edge, &label_map, &mut remapper))
            .transpose()?;
        blocks.push(Block::new_with_extra(
            label,
            body,
            term,
            params,
            exc_edge,
            callee_block.extra.clone(),
        ));
    }
    Ok(TypedInlineFragment {
        blocks,
        instr_id_mappings: instr_id_remapper.finish(),
        synthetic_instr_ids,
        constant_mappings: constant_scope.mappings(callee.function_id, inline_instance),
        local_mappings,
    })
}

fn typed_inline_callee_has_nonstack_storage(
    callee: &BlockPyFunction<TypedBlockPyModuleShape>,
    storage_layout: &soac_core::block_py::StorageLayout,
) -> bool {
    if !storage_layout.freevars.is_empty() || !storage_layout.cellvars.is_empty() {
        return true;
    }

    // A generator factory may describe preserved wrapper slots without using
    // them. Its resume body, however, must only be inlined through the explicit
    // generator-resume path, which binds and remaps the preserved owner.
    struct PreservedStorageFinder {
        found: bool,
    }

    impl Visit<InstrTyped> for PreservedStorageFinder {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }

            self.found = match expr {
                InstrTyped::Load(load) => {
                    load.name.preserved_location().is_some()
                        || matches!(load.name.cell_location(), Some(CellLocation::Preserved(_)))
                }
                InstrTyped::Store(store) => {
                    store.name.preserved_location().is_some()
                        || matches!(store.name.cell_location(), Some(CellLocation::Preserved(_)))
                }
                InstrTyped::Del(del) => {
                    del.name.preserved_location().is_some()
                        || matches!(del.name.cell_location(), Some(CellLocation::Preserved(_)))
                }
                InstrTyped::CellRef(cell_ref) => cell_ref.location.is_preserved(),
                _ => false,
            };

            if !self.found {
                expr.visit_children(self);
            }
        }
    }

    let mut finder = PreservedStorageFinder { found: false };
    finder.visit_fn(callee);
    finder.found
}

fn allocate_typed_inline_locals(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    callee_layout: &soac_core::block_py::StorageLayout,
    value_bindings: &TypedInlineValueBindings,
    preassigned_locals: &HashMap<LocalLocation, TypedTempLocal>,
) -> Result<HashMap<LocalLocation, TypedTempLocal>, TypedInlineUnsupportedReason> {
    let mut locals = preassigned_locals.clone();
    for (slot, _name) in callee_layout.stack_slots().iter().enumerate() {
        let location =
            LocalLocation(u32::try_from(slot).expect("callee stack slot index should fit in u32"));
        if value_bindings.contains_key(&location) || locals.contains_key(&location) {
            continue;
        }
        locals.insert(
            location,
            try_allocate_typed_stack_temp(caller, "typed_inline")?,
        );
    }
    Ok(locals)
}

fn typed_inline_local_mappings(
    callee: RuntimeFunctionId,
    inline_instance: u32,
    callee_layout: &soac_core::block_py::StorageLayout,
    locals: &HashMap<LocalLocation, TypedTempLocal>,
    value_bindings: &TypedInlineValueBindings,
) -> Result<Vec<TypedInlineLocalMapping>, TypedInlineUnsupportedReason> {
    let mut mappings = Vec::with_capacity(callee_layout.stack_slots().len());
    for (slot, callee_name) in callee_layout.stack_slots().iter().enumerate() {
        let callee_location =
            LocalLocation(u32::try_from(slot).expect("callee stack slot index should fit in u32"));
        let (caller_location, caller_name) =
            if let Some(value) = value_bindings.get(&callee_location) {
                let Ok(bound_name) = typed_inline_value_binding_name(callee_location, value) else {
                    continue;
                };
                let Some(location) = bound_name.local_location() else {
                    continue;
                };
                (location, bound_name.id.as_str().to_string())
            } else {
                let Some(fresh) = locals.get(&callee_location) else {
                    return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(
                        callee_location,
                    ));
                };
                (fresh.location, fresh.name.clone())
            };
        mappings.push(TypedInlineLocalMapping {
            callee,
            inline_instance,
            callee_location,
            callee_name: callee_name.clone(),
            caller_location,
            caller_name,
        });
    }
    Ok(mappings)
}

fn typed_inline_value_binding_name(
    callee_location: LocalLocation,
    value: &InstrTyped,
) -> Result<&ResolvedName, TypedInlineUnsupportedReason> {
    let InstrTyped::Load(load) = value else {
        return Err(TypedInlineUnsupportedReason::UnsupportedValueBinding(
            callee_location,
        ));
    };
    Ok(&load.name)
}

fn typed_remapped_label(
    label_map: &HashMap<BlockLabel, BlockLabel>,
    label: BlockLabel,
) -> Result<BlockLabel, TypedInlineUnsupportedReason> {
    label_map
        .get(&label)
        .copied()
        .ok_or(TypedInlineUnsupportedReason::UnknownLabel(label))
}

fn typed_remap_inline_term_labels(
    term: BlockTerm<InstrTyped>,
    label_map: &HashMap<BlockLabel, BlockLabel>,
    remapper: &mut TypedInlineLocalRemapper<'_, '_, '_, '_, '_>,
) -> Result<BlockTerm<InstrTyped>, TypedInlineUnsupportedReason> {
    Ok(match term {
        BlockTerm::Jump(edge) => {
            BlockTerm::Jump(typed_remap_inline_edge(edge, label_map, remapper)?)
        }
        BlockTerm::IfTerm(mut term) => {
            term.then_label = typed_remapped_label(label_map, term.then_label)?;
            term.else_label = typed_remapped_label(label_map, term.else_label)?;
            BlockTerm::IfTerm(term)
        }
        BlockTerm::BranchTable(mut term) => {
            for target in &mut term.targets {
                *target = typed_remapped_label(label_map, *target)?;
            }
            term.default_label = typed_remapped_label(label_map, term.default_label)?;
            BlockTerm::BranchTable(term)
        }
        BlockTerm::Raise(term) => BlockTerm::Raise(term),
        BlockTerm::Return(_) => return Err(TypedInlineUnsupportedReason::NonReturnTerm),
    })
}

fn typed_remap_inline_edge(
    mut edge: BlockEdge,
    label_map: &HashMap<BlockLabel, BlockLabel>,
    remapper: &mut TypedInlineLocalRemapper<'_, '_, '_, '_, '_>,
) -> Result<BlockEdge, TypedInlineUnsupportedReason> {
    edge.target = typed_remapped_label(label_map, edge.target)?;
    edge.args = edge
        .args
        .into_iter()
        .map(|arg| remapper.try_map_block_arg(arg))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(edge)
}

enum TypedInlineConstantScope<'a> {
    SameModule,
    CrossModule(TypedInlineConstantRemapper<'a>),
}

impl TypedInlineConstantScope<'_> {
    fn is_cross_module(&self) -> bool {
        matches!(self, Self::CrossModule(_))
    }

    fn remap_location(
        &mut self,
        location: NameLocation,
    ) -> Result<NameLocation, TypedInlineUnsupportedReason> {
        match (self, location) {
            (Self::SameModule, location) => Ok(location),
            (Self::CrossModule(remapper), NameLocation::Constant(index)) => {
                Ok(NameLocation::Constant(remapper.remap(index)?))
            }
            (Self::CrossModule(_), location) => Ok(location),
        }
    }

    fn mappings(
        &self,
        callee: RuntimeFunctionId,
        inline_instance: u32,
    ) -> Vec<TypedInlineConstantMapping> {
        match self {
            Self::SameModule => Vec::new(),
            Self::CrossModule(remapper) => remapper.mappings(callee, inline_instance),
        }
    }
}

struct TypedInlineConstantRemapper<'a> {
    caller_constants: &'a mut Vec<ConstantExpr>,
    callee_constants: &'a [ConstantExpr],
    mapped_indices: HashMap<u32, u32>,
}

impl<'a> TypedInlineConstantRemapper<'a> {
    fn new(
        caller_constants: &'a mut Vec<ConstantExpr>,
        callee_constants: &'a [ConstantExpr],
    ) -> Self {
        Self {
            caller_constants,
            callee_constants,
            mapped_indices: HashMap::new(),
        }
    }

    fn remap(&mut self, callee_index: u32) -> Result<u32, TypedInlineUnsupportedReason> {
        if let Some(caller_index) = self.mapped_indices.get(&callee_index).copied() {
            return Ok(caller_index);
        }
        let constant = self
            .callee_constants
            .get(callee_index as usize)
            .ok_or(TypedInlineUnsupportedReason::MissingCalleeConstant(
                callee_index,
            ))?
            .clone();
        let caller_index = u32::try_from(self.caller_constants.len())
            .map_err(|_| TypedInlineUnsupportedReason::TooManyCallerConstants)?;
        self.caller_constants.push(constant);
        self.mapped_indices.insert(callee_index, caller_index);
        Ok(caller_index)
    }

    fn mappings(
        &self,
        callee: RuntimeFunctionId,
        inline_instance: u32,
    ) -> Vec<TypedInlineConstantMapping> {
        let mut mappings = self
            .mapped_indices
            .iter()
            .map(
                |(&callee_index, &caller_index)| TypedInlineConstantMapping {
                    callee,
                    inline_instance,
                    callee_index,
                    caller_index,
                },
            )
            .collect::<Vec<_>>();
        mappings.sort_by_key(|mapping| mapping.callee_index);
        mappings
    }
}

fn typed_inline_constant_scope<'a>(
    caller_constants: Option<&'a mut Vec<ConstantExpr>>,
    callee_constants: Option<&'a [ConstantExpr]>,
) -> Result<TypedInlineConstantScope<'a>, TypedInlineUnsupportedReason> {
    match (caller_constants, callee_constants) {
        (_, None) => Ok(TypedInlineConstantScope::SameModule),
        (Some(caller_constants), Some(callee_constants)) => {
            Ok(TypedInlineConstantScope::CrossModule(
                TypedInlineConstantRemapper::new(caller_constants, callee_constants),
            ))
        }
        (None, Some(_)) => Err(TypedInlineUnsupportedReason::TooManyCallerConstants),
    }
}

struct TypedInlineLocalRemapper<'locals, 'bindings, 'constants, 'remapper, 'allocator> {
    callee_layout: &'locals soac_core::block_py::StorageLayout,
    locals: &'locals HashMap<LocalLocation, TypedTempLocal>,
    value_bindings: &'bindings TypedInlineValueBindings,
    closure_cell_bindings: Option<&'bindings HashMap<u32, CellLocation>>,
    instr_id_remapper: &'remapper mut TypedInlineInstrIdRemapper<'allocator>,
    constant_scope: &'remapper mut TypedInlineConstantScope<'constants>,
}

impl<'locals, 'bindings, 'constants, 'remapper, 'allocator>
    TypedInlineLocalRemapper<'locals, 'bindings, 'constants, 'remapper, 'allocator>
{
    fn new(
        callee_layout: &'locals soac_core::block_py::StorageLayout,
        locals: &'locals HashMap<LocalLocation, TypedTempLocal>,
        value_bindings: &'bindings TypedInlineValueBindings,
        closure_cell_bindings: Option<&'bindings HashMap<u32, CellLocation>>,
        instr_id_remapper: &'remapper mut TypedInlineInstrIdRemapper<'allocator>,
        constant_scope: &'remapper mut TypedInlineConstantScope<'constants>,
    ) -> Self {
        Self {
            callee_layout,
            locals,
            value_bindings,
            closure_cell_bindings,
            instr_id_remapper,
            constant_scope,
        }
    }

    fn callee_local_location_by_name(&self, name: &str) -> Option<LocalLocation> {
        self.callee_layout
            .stack_slots()
            .iter()
            .position(|slot_name| slot_name == name)
            .map(|slot| {
                LocalLocation(
                    u32::try_from(slot).expect("callee stack slot index should fit in u32"),
                )
            })
    }

    fn try_map_block_local_name(
        &self,
        name: String,
    ) -> Result<String, TypedInlineUnsupportedReason> {
        let Some(location) = self.callee_local_location_by_name(name.as_str()) else {
            return Err(TypedInlineUnsupportedReason::UnknownBlockName(name));
        };
        if let Some(value) = self.value_bindings.get(&location) {
            let bound_name = typed_inline_value_binding_name(location, value)?;
            if bound_name.local_location().is_none() {
                return Err(TypedInlineUnsupportedReason::NonLocalValueBinding(location));
            }
            return Ok(bound_name.id.as_str().to_string());
        }
        let Some(fresh) = self.locals.get(&location) else {
            return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(location));
        };
        Ok(fresh.name.clone())
    }

    fn try_map_block_param(
        &self,
        mut param: BlockParam,
    ) -> Result<BlockParam, TypedInlineUnsupportedReason> {
        let Some(location) = self.callee_local_location_by_name(param.name.as_str()) else {
            return Err(TypedInlineUnsupportedReason::UnknownBlockName(param.name));
        };
        if self.value_bindings.contains_key(&location) {
            return Err(TypedInlineUnsupportedReason::RebindsBoundLocal(location));
        }
        let Some(fresh) = self.locals.get(&location) else {
            return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(location));
        };
        param.name = fresh.name.clone();
        Ok(param)
    }

    fn try_map_block_arg(&self, arg: BlockArg) -> Result<BlockArg, TypedInlineUnsupportedReason> {
        Ok(match arg {
            BlockArg::Name(name) => BlockArg::Name(self.try_map_block_local_name(name)?),
            BlockArg::None => BlockArg::None,
            BlockArg::CurrentException => BlockArg::CurrentException,
            BlockArg::AbruptKind(kind) => BlockArg::AbruptKind(kind),
        })
    }

    fn try_map_cell_location(
        &self,
        location: CellLocation,
    ) -> Result<CellLocation, TypedInlineUnsupportedReason> {
        match location {
            CellLocation::Closure(slot) | CellLocation::CapturedSource(slot) => self
                .closure_cell_bindings
                .and_then(|bindings| bindings.get(&slot).copied())
                .ok_or(TypedInlineUnsupportedReason::UnsupportedGeneratorClosureCapture),
            CellLocation::Owned(_) | CellLocation::Preserved(_) => Ok(location),
        }
    }

    fn assign_fresh_instr_id(&mut self, instr: InstrTyped) -> InstrTyped {
        self.instr_id_remapper.assign_fresh_instr_id(instr)
    }
}

impl TryMapInstr<InstrTyped, InstrTyped, TypedInlineUnsupportedReason>
    for TypedInlineLocalRemapper<'_, '_, '_, '_, '_>
{
    fn try_map_instr(
        &mut self,
        instr: InstrTyped,
    ) -> Result<InstrTyped, TypedInlineUnsupportedReason> {
        let mapped = match instr {
            InstrTyped::Truthy(op) => InstrTyped::Truthy(op.try_map_children(self)?),
            InstrTyped::Load(op) => {
                if let Some(location) = op.name.local_location()
                    && let Some(value) = self.value_bindings.get(&location)
                {
                    return Ok(clear_typed_instr_ids(value.clone()));
                }
                InstrTyped::Load(op.try_map_children(self)?)
            }
            InstrTyped::BinOp(op) => InstrTyped::BinOp(op.try_map_children(self)?),
            InstrTyped::Tuple(op) => InstrTyped::Tuple(op.try_map_children(self)?),
            InstrTyped::UnaryOp(op) => InstrTyped::UnaryOp(op.try_map_children(self)?),
            InstrTyped::CalleeFunctionId(op) => {
                InstrTyped::CalleeFunctionId(op.try_map_children(self)?)
            }
            InstrTyped::CallTyped(op) => {
                let mut op = op.try_map_children(self)?;
                op.args = expand_synthetic_typed_starred_tuple_args(op.args);
                InstrTyped::CallTyped(op)
            }
            InstrTyped::GuardedCallableCallTyped(op) => {
                let mut op = op.try_map_children(self)?;
                op.args = expand_synthetic_typed_starred_tuple_args(op.args);
                InstrTyped::GuardedCallableCallTyped(op)
            }
            InstrTyped::GuardedMethodCallTyped(op) => {
                let mut op = op.try_map_children(self)?;
                op.args = expand_synthetic_typed_starred_tuple_args(op.args);
                InstrTyped::GuardedMethodCallTyped(op)
            }
            InstrTyped::DirectCallableCallTyped(op) => {
                let mut op = op.try_map_children(self)?;
                op.args = expand_synthetic_typed_starred_tuple_args(op.args);
                InstrTyped::DirectCallableCallTyped(op)
            }
            InstrTyped::DirectMethodCallTyped(op) => {
                let mut op = op.try_map_children(self)?;
                op.args = expand_synthetic_typed_starred_tuple_args(op.args);
                InstrTyped::DirectMethodCallTyped(op)
            }
            InstrTyped::DirectCallGuardTest(op) => {
                InstrTyped::DirectCallGuardTest(op.try_map_children(self)?)
            }
            InstrTyped::CallDirect(op) => {
                let mut op = op.try_map_children(self)?;
                op.args = expand_synthetic_typed_starred_tuple_args(op.args);
                InstrTyped::CallDirect(op)
            }
            InstrTyped::GetAttrTyped(op) => InstrTyped::GetAttrTyped(op.try_map_children(self)?),
            InstrTyped::SetAttrTyped(op) => InstrTyped::SetAttrTyped(op.try_map_children(self)?),
            InstrTyped::GetItem(op) => InstrTyped::GetItem(op.try_map_children(self)?),
            InstrTyped::SetItem(op) => InstrTyped::SetItem(op.try_map_children(self)?),
            InstrTyped::DelItem(op) => InstrTyped::DelItem(op.try_map_children(self)?),
            InstrTyped::Store(op) => {
                if let Some(location) = op.name.local_location()
                    && self.value_bindings.contains_key(&location)
                {
                    return Err(TypedInlineUnsupportedReason::RebindsBoundLocal(location));
                }
                InstrTyped::Store(op.try_map_children(self)?)
            }
            InstrTyped::Del(op) => {
                if let Some(location) = op.name.local_location()
                    && self.value_bindings.contains_key(&location)
                {
                    return Err(TypedInlineUnsupportedReason::RebindsBoundLocal(location));
                }
                InstrTyped::Del(op.try_map_children(self)?)
            }
            InstrTyped::MakeCell(op) => InstrTyped::MakeCell(op.try_map_children(self)?),
            InstrTyped::IncrementCounter(op) => InstrTyped::IncrementCounter(op),
            InstrTyped::CellRef(mut op) => {
                op.location = self.try_map_cell_location(op.location)?;
                InstrTyped::CellRef(op)
            }
            InstrTyped::MakeFunctionWithClosure(op) => {
                InstrTyped::MakeFunctionWithClosure(op.try_map_children(self)?)
            }
        };
        Ok(self.instr_id_remapper.remap_instr_id(mapped))
    }

    fn try_map_name(
        &mut self,
        mut name: ResolvedName,
    ) -> Result<ResolvedName, TypedInlineUnsupportedReason> {
        name.location = self.constant_scope.remap_location(name.location)?;
        if self.constant_scope.is_cross_module()
            && (name.location.is_global() || name.location.is_global_name())
        {
            let Some(runtime_name) = RuntimeName::from_name(name.id.as_str()) else {
                return Err(TypedInlineUnsupportedReason::CrossModuleGlobalName(
                    name.id.to_string(),
                ));
            };
            name.location = NameLocation::RuntimeName(runtime_name);
        }
        if let NameLocation::Cell(location) = name.location {
            name.location = NameLocation::Cell(self.try_map_cell_location(location)?);
            return Ok(name);
        }
        let Some(location) = name.location.as_local() else {
            return Ok(name);
        };
        if self.value_bindings.contains_key(&location) {
            return Err(TypedInlineUnsupportedReason::RebindsBoundLocal(location));
        }
        let Some(fresh) = self.locals.get(&location) else {
            return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(location));
        };
        name.id = fresh.name.clone().into();
        name.location = NameLocation::Local(fresh.location);
        Ok(name)
    }
}

fn clear_typed_instr_ids(mut instr: InstrTyped) -> InstrTyped {
    struct Scrubber;
    impl VisitMut<InstrTyped> for Scrubber {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);
            let mut meta = expr.meta();
            meta.instr_id = None;
            *expr = expr.clone().with_meta(meta);
        }
    }
    Scrubber.visit_instr_mut(&mut instr);
    instr
}

pub fn simplify_typed_virtual_tuple_ops(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &mut Vec<ConstantExpr>,
) -> usize {
    let virtual_tuple_defs = collect_replayable_typed_tuple_local_defs(function);
    let dominators = typed_block_dominators(function);

    struct Simplifier<'a> {
        virtual_tuple_defs: &'a HashMap<LocalLocation, Vec<TypedTupleLocalDef>>,
        dominators: &'a TypedBlockDominators,
        module_constants: &'a mut Vec<ConstantExpr>,
        block: BlockLabel,
        instr_index: usize,
        changed: usize,
    }

    impl VisitMut<InstrTyped> for Simplifier<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);

            if let Some(replacement) = simplify_typed_virtual_tuple_len(
                expr,
                self.virtual_tuple_defs,
                self.module_constants,
                self.block,
                self.instr_index,
                self.dominators,
            ) {
                *expr = replacement;
                self.changed += 1;
                return;
            }

            if let Some(replacement) = simplify_typed_virtual_tuple_getitem(
                expr,
                self.virtual_tuple_defs,
                self.module_constants,
                self.block,
                self.instr_index,
                self.dominators,
            ) {
                *expr = replacement;
                self.changed += 1;
                return;
            }

            if let Some(replacement) =
                simplify_typed_exact_int_index_call(expr, self.module_constants)
            {
                *expr = replacement;
                self.changed += 1;
            }
        }
    }

    let tuple_changed = {
        let mut changed = 0;
        for block in &mut function.blocks {
            for (instr_index, instr) in block.body.iter_mut().enumerate() {
                let mut simplifier = Simplifier {
                    virtual_tuple_defs: &virtual_tuple_defs,
                    dominators: &dominators,
                    module_constants,
                    block: block.label,
                    instr_index,
                    changed: 0,
                };
                simplifier.visit_instr_mut(instr);
                changed += simplifier.changed;
            }
            let mut simplifier = Simplifier {
                virtual_tuple_defs: &virtual_tuple_defs,
                dominators: &dominators,
                module_constants,
                block: block.label,
                instr_index: block.body.len(),
                changed: 0,
            };
            simplifier.visit_term_mut(&mut block.term);
            changed += simplifier.changed;
        }
        changed
    };
    let mut changed = tuple_changed;
    let constant_locals = collect_typed_i64_constant_local_defs(function, module_constants);
    changed += rewrite_dominated_typed_constant_loads(function, &constant_locals, &dominators);
    changed += fold_typed_constant_branches(function, module_constants);
    changed += remove_unused_replayable_typed_tuple_stores(function, &virtual_tuple_defs);

    changed
}

#[derive(Clone)]
struct TypedTupleLocalDef {
    values: Vec<InstrTyped>,
    def_block: BlockLabel,
    def_index: usize,
}

fn collect_replayable_typed_tuple_local_defs(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<LocalLocation, Vec<TypedTupleLocalDef>> {
    let mut candidates = HashMap::<LocalLocation, Vec<TypedTupleLocalDef>>::new();
    let mut invalid = HashSet::<LocalLocation>::new();

    for block in &function.blocks {
        for (index, instr) in block.body.iter().enumerate() {
            match instr {
                InstrTyped::Store(store) => {
                    let Some(location) = store.name.local_location() else {
                        continue;
                    };
                    if invalid.contains(&location) {
                        continue;
                    }
                    let InstrTyped::Tuple(tuple) = store.value.as_ref() else {
                        invalid.insert(location);
                        candidates.remove(&location);
                        continue;
                    };
                    if !typed_tuple_values_are_replayable_loads(tuple.values.as_slice()) {
                        invalid.insert(location);
                        candidates.remove(&location);
                        continue;
                    }
                    candidates
                        .entry(location)
                        .or_default()
                        .push(TypedTupleLocalDef {
                            values: tuple.values.clone(),
                            def_block: block.label,
                            def_index: index,
                        });
                }
                InstrTyped::Del(del) => {
                    if let Some(location) = del.name.local_location() {
                        invalid.insert(location);
                        candidates.remove(&location);
                    }
                }
                _ => {}
            }
        }
    }

    candidates
}

fn typed_tuple_values_are_replayable_loads(values: &[InstrTyped]) -> bool {
    values
        .iter()
        .all(|value| matches!(value, InstrTyped::Load(_)))
}

fn remove_unused_replayable_typed_tuple_stores(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    virtual_tuple_defs: &HashMap<LocalLocation, Vec<TypedTupleLocalDef>>,
) -> usize {
    if virtual_tuple_defs.is_empty() {
        return 0;
    }

    let candidate_locations = virtual_tuple_defs.keys().copied().collect::<HashSet<_>>();
    let loaded_locations = collect_typed_loaded_local_locations(function);
    let removable_locations = candidate_locations
        .difference(&loaded_locations)
        .copied()
        .collect::<HashSet<_>>();
    if removable_locations.is_empty() {
        return 0;
    }

    let mut removed = 0;
    let function_id = function.function_id;
    for block in &mut function.blocks {
        let block_label = block.label;
        block.body.retain(|instr| {
            let removable = matches!(
                instr,
                InstrTyped::Store(store)
                    if store
                        .name
                        .local_location()
                        .is_some_and(|location| removable_locations.contains(&location))
                        && matches!(
                            store.value.as_ref(),
                            InstrTyped::Tuple(tuple)
                                if typed_tuple_values_are_replayable_loads(&tuple.values)
                        )
            );
            if removable {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    function = ?function_id,
                    block = ?block_label,
                    instr = ?instr,
                    "typed_virtual_tuple_store_removed",
                );
            }
            removed += usize::from(removable);
            !removable
        });
    }
    removed
}

fn collect_typed_loaded_local_locations(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashSet<LocalLocation> {
    struct Collector {
        locations: HashSet<LocalLocation>,
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::Load(load) = expr
                && let Some(location) = load.name.local_location()
            {
                self.locations.insert(location);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        locations: HashSet::new(),
    };
    collector.visit_fn(function);
    collector.locations
}

fn simplify_typed_virtual_tuple_len(
    expr: &InstrTyped,
    virtual_tuple_defs: &HashMap<LocalLocation, Vec<TypedTupleLocalDef>>,
    module_constants: &mut Vec<ConstantExpr>,
    use_block: BlockLabel,
    use_index: usize,
    dominators: &TypedBlockDominators,
) -> Option<InstrTyped> {
    let InstrTyped::CallTyped(call) = expr else {
        return None;
    };
    if !typed_expr_is_runtime_name_load(call.func.as_ref(), RuntimeName::Len, module_constants)
        || !call.keywords.is_empty()
        || call.args.len() != 1
    {
        return None;
    }
    let CallArgPositional::Positional(arg) = &call.args[0] else {
        return None;
    };
    let values =
        typed_virtual_tuple_values(arg, virtual_tuple_defs, use_block, use_index, dominators)?;
    Some(typed_i64_constant_load(
        module_constants,
        i64::try_from(values.len()).expect("tuple length should fit in i64"),
        expr.meta(),
    ))
}

fn simplify_typed_virtual_tuple_getitem(
    expr: &InstrTyped,
    virtual_tuple_defs: &HashMap<LocalLocation, Vec<TypedTupleLocalDef>>,
    module_constants: &[ConstantExpr],
    use_block: BlockLabel,
    use_index: usize,
    dominators: &TypedBlockDominators,
) -> Option<InstrTyped> {
    let InstrTyped::GetItem(op) = expr else {
        return None;
    };
    let values = typed_virtual_tuple_values(
        op.value.as_ref(),
        virtual_tuple_defs,
        use_block,
        use_index,
        dominators,
    )?;
    let index = typed_expr_const_i64(op.index.as_ref(), module_constants)?;
    let normalized = if index < 0 {
        i64::try_from(values.len()).ok()?.checked_add(index)?
    } else {
        index
    };
    let index = usize::try_from(normalized).ok()?;
    let value = values.get(index)?;
    Some(clear_typed_instr_ids(value.clone()))
}

fn simplify_typed_exact_int_index_call(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<InstrTyped> {
    let InstrTyped::CallTyped(call) = expr else {
        return None;
    };
    if !typed_expr_is_runtime_name_load(call.func.as_ref(), RuntimeName::Index, module_constants)
        || !call.keywords.is_empty()
        || call.args.len() != 1
    {
        return None;
    }
    let CallArgPositional::Positional(arg) = &call.args[0] else {
        return None;
    };
    if !typed_expr_is_exact_int(arg) {
        return None;
    }
    Some(clear_typed_instr_ids(arg.clone()))
}

fn typed_expr_is_exact_int(expr: &InstrTyped) -> bool {
    expr.result_facts()
        .and_then(|facts| facts.as_pyobj())
        .is_some_and(|facts| facts.is_exact_type(PyExactType::Int))
}

fn typed_virtual_tuple_values<'a>(
    expr: &'a InstrTyped,
    virtual_tuple_defs: &'a HashMap<LocalLocation, Vec<TypedTupleLocalDef>>,
    use_block: BlockLabel,
    use_index: usize,
    dominators: &TypedBlockDominators,
) -> Option<&'a [InstrTyped]> {
    match expr {
        InstrTyped::Tuple(tuple) if typed_tuple_values_are_replayable_loads(&tuple.values) => {
            Some(tuple.values.as_slice())
        }
        InstrTyped::Load(load) => load
            .name
            .local_location()
            .and_then(|location| {
                dominating_typed_tuple_def_for_use(
                    virtual_tuple_defs.get(&location)?,
                    use_block,
                    use_index,
                    dominators,
                )
            })
            .map(|def| def.values.as_slice()),
        _ => None,
    }
}

fn typed_expr_const_i64(expr: &InstrTyped, module_constants: &[ConstantExpr]) -> Option<i64> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    let index = load.name.location.as_constant()?;
    typed_module_constant_i64_value(module_constants, index)
}

fn typed_module_constant_i64_value(module_constants: &[ConstantExpr], index: u32) -> Option<i64> {
    let ConstantExpr::Literal(literal) = module_constants.get(index as usize)? else {
        return None;
    };
    let Literal::NumberLiteral(number) = literal.as_literal() else {
        return None;
    };
    let NumberLiteralValue::Int(value) = &number.value else {
        return None;
    };
    value.as_i64()
}

fn fold_typed_constant_branches(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> usize {
    let mut changed = 0;
    for block in &mut function.blocks {
        let BlockTerm::IfTerm(if_term) = &block.term else {
            continue;
        };
        let Some(truthy) = typed_expr_const_truthiness(&if_term.test, module_constants) else {
            continue;
        };
        let target = if truthy {
            if_term.then_label
        } else {
            if_term.else_label
        };
        block.term = BlockTerm::Jump(BlockEdge::new(target));
        changed += 1;
    }
    changed + prune_unreachable_typed_blocks(function)
}

fn typed_expr_const_truthiness(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<bool> {
    match expr {
        InstrTyped::Truthy(op) => typed_expr_const_truthiness(op.value.as_ref(), module_constants),
        InstrTyped::UnaryOp(op) if op.kind == UnaryOpKind::Not => Some(
            !typed_expr_const_truthiness(op.operand.as_ref(), module_constants)?,
        ),
        InstrTyped::UnaryOp(op) if op.kind == UnaryOpKind::Truth => {
            typed_expr_const_truthiness(op.operand.as_ref(), module_constants)
        }
        InstrTyped::BinOp(op) => typed_i64_binop_const_bool(
            op.kind,
            op.left.as_ref(),
            op.right.as_ref(),
            module_constants,
        ),
        _ => typed_expr_const_i64(expr, module_constants).map(|value| value != 0),
    }
}

fn typed_i64_binop_const_bool(
    kind: BinOpKind,
    left: &InstrTyped,
    right: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<bool> {
    let left = typed_expr_const_i64(left, module_constants)?;
    let right = typed_expr_const_i64(right, module_constants)?;
    match kind {
        BinOpKind::Eq => Some(left == right),
        BinOpKind::Ne => Some(left != right),
        BinOpKind::Lt => Some(left < right),
        BinOpKind::Le => Some(left <= right),
        BinOpKind::Gt => Some(left > right),
        BinOpKind::Ge => Some(left >= right),
        _ => None,
    }
}

pub fn prune_unreachable_typed_blocks(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> usize {
    let reachable = TypedReachableBlockView::for_function(function);
    if reachable.is_empty() {
        return 0;
    }
    let before = function.blocks.len();
    function
        .blocks
        .retain(|block| reachable.contains(block.label));
    before - function.blocks.len()
}

fn typed_i64_constant_load(
    module_constants: &mut Vec<ConstantExpr>,
    value: i64,
    mut meta: Meta,
) -> InstrTyped {
    let index =
        u32::try_from(module_constants.len()).expect("module constant count should fit u32");
    module_constants.push(ConstantExpr::Literal(LiteralValue::new(
        Literal::NumberLiteral(NumberLiteral {
            value: NumberLiteralValue::Int(IntLiteral::from_i64(value)),
        }),
    )));
    meta.instr_id = None;
    let truthiness = if value == 0 {
        TruthinessFact::AlwaysFalse
    } else {
        TruthinessFact::AlwaysTrue
    };
    let mut extra = TypedInstrExtra::default();
    extra.refine_result_facts(ValueFacts::PyObj(
        PyObjFacts::exact_type_with_truthiness(PyExactType::Int, truthiness)
            .with_module_constant(index)
            .with_immortal_refcount(),
    ));
    InstrTyped::Load(
        Load::new(ResolvedName {
            id: "__dp_constant".into(),
            location: NameLocation::Constant(index),
        })
        .with_extra(extra)
        .with_meta(meta),
    )
}

pub fn lower_typed_generator_state_to_locals_with_plan(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &mut Vec<ConstantExpr>,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    plans: &[TypedGeneratorStateLoweringPlan],
) -> TypedGeneratorStateLoweringStats {
    lower_typed_generator_state_to_locals_with_plan_and_collect_preserved_locals(
        function,
        module_constants,
        callee_module,
        external_callees,
        plans,
    )
    .stats
}

pub fn lower_typed_generator_state_to_locals_with_plan_and_collect_preserved_locals(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &mut Vec<ConstantExpr>,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    plans: &[TypedGeneratorStateLoweringPlan],
) -> TypedGeneratorStateLoweringOutcome {
    let mut outcome = TypedGeneratorStateLoweringOutcome::default();
    for plan in plans {
        let Some((next_stats, preserved_locals)) = lower_typed_generator_state_origin_to_locals(
            function,
            module_constants,
            callee_module,
            external_callees,
            plan,
        ) else {
            continue;
        };
        outcome.stats.lowered_generators += next_stats.lowered_generators;
        outcome.stats.initialized_slots += next_stats.initialized_slots;
        outcome.stats.remapped_instrs += next_stats.remapped_instrs;
        outcome.stats.removed_owner_stores += next_stats.removed_owner_stores;
        outcome
            .preserved_locals_by_origin
            .insert(plan.generator_origin, preserved_locals);
    }
    outcome
}

pub fn typed_generator_state_origin_can_lower_aliases(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    generator_origin: InstrId,
    inlined_resume_instr_ids: &HashSet<InstrId>,
) -> bool {
    typed_generator_state_origin_can_lower_aliases_in_blocks(
        function,
        module_constants,
        generator_origin,
        inlined_resume_instr_ids,
        None,
    )
}

pub fn typed_generator_state_origin_can_lower_aliases_in_blocks(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    generator_origin: InstrId,
    inlined_resume_instr_ids: &HashSet<InstrId>,
    active_blocks: Option<&HashSet<BlockLabel>>,
) -> bool {
    let Some((_, _, constructor_target, _)) =
        find_typed_generator_constructor_store(function, generator_origin)
    else {
        tracing::info!(
            target: "soac_generator_state_lowering",
            caller = ?function.function_id,
            generator_origin = ?generator_origin,
            resume_instr_count = inlined_resume_instr_ids.len(),
            reason = "missing_constructor_store",
            "typed_generator_state_origin_cannot_lower_aliases",
        );
        return false;
    };
    let Some(generator_location) = constructor_target.local_location() else {
        tracing::info!(
            target: "soac_generator_state_lowering",
            caller = ?function.function_id,
            generator_origin = ?generator_origin,
            resume_instr_count = inlined_resume_instr_ids.len(),
            reason = "non_local_constructor_target",
            "typed_generator_state_origin_cannot_lower_aliases",
        );
        return false;
    };
    let can_lower = typed_generator_alias_cleanup(
        function,
        module_constants,
        generator_location,
        inlined_resume_instr_ids,
        active_blocks,
    )
    .is_some();
    if !can_lower {
        tracing::info!(
            target: "soac_generator_state_lowering",
            caller = ?function.function_id,
            generator_origin = ?generator_origin,
            resume_instr_count = inlined_resume_instr_ids.len(),
            reason = "residual_alias_use",
            "typed_generator_state_origin_cannot_lower_aliases",
        );
    }
    can_lower
}

fn typed_generator_alias_locations_for_origin(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    generator_origin: InstrId,
) -> Option<HashSet<LocalLocation>> {
    let (_, _, constructor_target, _) =
        find_typed_generator_constructor_store(function, generator_origin)?;
    let generator_location = constructor_target.local_location()?;
    Some(collect_typed_generator_alias_locations(
        function,
        module_constants,
        generator_location,
        None,
    ))
}

pub fn typed_generator_alias_ignored_instr_ids_by_origin(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    pending_alias_use_instr_ids_by_origin: &HashMap<InstrId, HashSet<InstrId>>,
) -> HashMap<InstrId, HashSet<InstrId>> {
    let mut alias_locations_by_origin = HashMap::<InstrId, HashSet<LocalLocation>>::new();
    let mut shared_alias_use_instr_ids_by_location =
        HashMap::<LocalLocation, HashSet<InstrId>>::new();

    for (&generator_origin, pending_alias_use_instr_ids) in pending_alias_use_instr_ids_by_origin {
        let Some(alias_locations) = typed_generator_alias_locations_for_origin(
            function,
            module_constants,
            generator_origin,
        ) else {
            continue;
        };
        for alias_location in &alias_locations {
            shared_alias_use_instr_ids_by_location
                .entry(*alias_location)
                .or_default()
                .extend(pending_alias_use_instr_ids.iter().copied());
        }
        alias_locations_by_origin.insert(generator_origin, alias_locations);
    }

    pending_alias_use_instr_ids_by_origin
        .iter()
        .map(|(&generator_origin, pending_alias_use_instr_ids)| {
            let grouped_alias_use_instr_ids = alias_locations_by_origin
                .get(&generator_origin)
                .map(|alias_locations| {
                    alias_locations
                        .iter()
                        .filter_map(|location| shared_alias_use_instr_ids_by_location.get(location))
                        .flatten()
                        .copied()
                        .collect::<HashSet<_>>()
                })
                .filter(|grouped| !grouped.is_empty())
                .unwrap_or_else(|| pending_alias_use_instr_ids.clone());
            (generator_origin, grouped_alias_use_instr_ids)
        })
        .collect()
}

fn lower_typed_generator_state_origin_to_locals(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &mut Vec<ConstantExpr>,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    plan: &TypedGeneratorStateLoweringPlan,
) -> Option<(
    TypedGeneratorStateLoweringStats,
    HashMap<PreservedLocation, ResolvedName>,
)> {
    let callee = typed_inline_callee(
        callee_module,
        TypedInlineExternalCallees::Contextual(external_callees),
        plan.function_id,
    )
    .map(|callee| callee.function)
    .or_else(|| {
        trace_typed_generator_state_lowering_skip(function, plan, "missing_callee");
        None
    })?;
    let public_layout = callee.public_storage_layout().or_else(|| {
        trace_typed_generator_state_lowering_skip(function, plan, "missing_public_layout");
        None
    })?;
    let (constructor_block_index, constructor_instr_index, constructor_target, constructor_call) =
        find_typed_generator_constructor_store(function, plan.generator_origin)
            .or_else(|| {
                plan.materialized_constructor
                    .as_ref()
                    .and_then(|constructor| {
                        find_typed_materialized_generator_constructor_store(
                            function,
                            plan.generator_origin,
                            constructor,
                        )
                    })
            })
            .or_else(|| {
                trace_typed_generator_constructor_candidates(function, plan);
                trace_typed_generator_state_lowering_skip(
                    function,
                    plan,
                    "missing_constructor_store",
                );
                None
            })?;
    let generator_location = constructor_target.local_location().or_else(|| {
        trace_typed_generator_state_lowering_skip(function, plan, "non_local_constructor_target");
        None
    })?;
    let dominated_body_instr_ids = typed_generator_state_constructor_dominated_body_instr_ids(
        function,
        constructor_block_index,
        constructor_instr_index,
        &plan.body_instr_ids,
        plan.alias_cleanup_active_blocks.as_ref(),
    );
    if dominated_body_instr_ids.is_empty() {
        trace_typed_generator_state_lowering_skip(
            function,
            plan,
            "constructor_does_not_dominate_body",
        );
        return None;
    }
    let mut effective_plan = plan.clone();
    if dominated_body_instr_ids.len() != plan.body_instr_ids.len() {
        effective_plan.pending_alias_use_instr_ids.extend(
            plan.body_instr_ids
                .difference(&dominated_body_instr_ids)
                .copied(),
        );
        effective_plan.body_instr_ids = dominated_body_instr_ids;
    }
    let ignored_alias_use_instr_ids = effective_plan
        .body_instr_ids
        .iter()
        .chain(effective_plan.pending_alias_use_instr_ids.iter())
        .copied()
        .collect::<HashSet<_>>();
    let alias_cleanup = typed_generator_alias_cleanup(
        function,
        module_constants,
        generator_location,
        &ignored_alias_use_instr_ids,
        plan.alias_cleanup_active_blocks.as_ref(),
    )
    .or_else(|| {
        trace_typed_generator_state_lowering_skip(function, plan, "residual_alias_use");
        None
    })?;

    let values = typed_positional_arg_exprs(constructor_call.args.clone()).or_else(|| {
        trace_typed_generator_state_lowering_skip(function, plan, "non_positional_args");
        None
    })?;
    if constructor_call
        .args
        .iter()
        .any(|arg| matches!(arg, CallArgPositional::Starred(_)))
        || !constructor_call.keywords.is_empty()
    {
        trace_typed_generator_state_lowering_skip(function, plan, "starred_or_keyword_args");
        return None;
    }
    let generator_instance = constructor_call
        .extra
        .generator_instance_plan()
        .or_else(|| {
            trace_typed_generator_state_lowering_skip(
                function,
                plan,
                "missing_generator_instance_plan",
            );
            None
        })?;
    if generator_instance.function_id != plan.function_id
        || generator_instance.arg_plan.sources.len() != callee.params.len()
    {
        trace_typed_generator_state_lowering_skip(function, plan, "generator_plan_mismatch");
        return None;
    }

    let mut arg_temps = Vec::with_capacity(values.len());
    let mut replacement = Vec::new();
    for value in values {
        let temp = try_allocate_typed_stack_temp(function, "typed_gen_arg").ok()?;
        replacement.push(typed_store_temp(temp.resolved_name(), value));
        arg_temps.push(temp);
    }
    let arg_values = arg_temps
        .iter()
        .map(|temp| typed_load_temp(&temp.resolved_name()))
        .collect::<Vec<_>>();
    let public_params = callee.params.iter().collect::<Vec<_>>();
    let public_param_indices = public_params
        .iter()
        .enumerate()
        .map(|(index, param)| (param.name.as_str(), index))
        .collect::<HashMap<_, _>>();

    let mut preserved_locals = HashMap::new();
    for (slot_index, slot) in public_layout.preserved_slots.iter().enumerate() {
        let temp = try_allocate_typed_stack_temp(function, "typed_gen_state").ok()?;
        let value = match (slot.storage, slot.init.clone()) {
            (PreservedSlotStorage::PyCellObject, soac_core::block_py::ClosureInit::Parameter) => {
                let param_index = *public_param_indices.get(slot.logical_name.as_str())?;
                let initial_value = typed_inline_value_for_arg_source(
                    public_params[param_index].kind,
                    generator_instance.arg_plan.sources.get(param_index)?,
                    arg_values.as_slice(),
                )
                .ok()?;
                InstrTyped::MakeCell(
                    MakeCell::with_initial_value(initial_value).with_meta(constructor_call.meta()),
                )
            }
            (PreservedSlotStorage::PyCellObject, soac_core::block_py::ClosureInit::EmptyCell) => {
                InstrTyped::MakeCell(MakeCell::empty().with_meta(constructor_call.meta()))
            }
            (
                PreservedSlotStorage::PyCellObject,
                soac_core::block_py::ClosureInit::InheritedCapture
                | soac_core::block_py::ClosureInit::RuntimePcUnstarted
                | soac_core::block_py::ClosureInit::RuntimeAbruptKindFallthrough
                | soac_core::block_py::ClosureInit::RuntimeZero
                | soac_core::block_py::ClosureInit::RuntimeNone
                | soac_core::block_py::ClosureInit::Deferred,
            ) => {
                trace_typed_generator_state_lowering_skip(
                    function,
                    plan,
                    "unsupported_pycell_slot_init",
                );
                return None;
            }
            (
                PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::I64,
                soac_core::block_py::ClosureInit::Parameter,
            ) => {
                let param_index = *public_param_indices.get(slot.logical_name.as_str())?;
                typed_inline_value_for_arg_source(
                    public_params[param_index].kind,
                    generator_instance.arg_plan.sources.get(param_index)?,
                    arg_values.as_slice(),
                )
                .ok()?
            }
            (
                PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::I64,
                soac_core::block_py::ClosureInit::RuntimePcUnstarted,
            ) => typed_i64_constant_load(module_constants, 1, constructor_call.meta()),
            (
                PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::I64,
                soac_core::block_py::ClosureInit::RuntimeAbruptKindFallthrough,
            ) => typed_i64_constant_load(module_constants, 0, constructor_call.meta()),
            (
                PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::I64,
                soac_core::block_py::ClosureInit::RuntimeZero,
            ) => typed_i64_constant_load(module_constants, 0, constructor_call.meta()),
            (
                PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::I64,
                soac_core::block_py::ClosureInit::RuntimeNone
                | soac_core::block_py::ClosureInit::Deferred,
            ) => InstrTyped::constant_none(),
            (
                PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::I64,
                soac_core::block_py::ClosureInit::InheritedCapture
                | soac_core::block_py::ClosureInit::EmptyCell,
            ) => {
                trace_typed_generator_state_lowering_skip(
                    function,
                    plan,
                    "unsupported_non_cell_slot_init",
                );
                return None;
            }
        };
        replacement.push(typed_store_temp(temp.resolved_name(), value));
        tracing::info!(
            target: "soac_generator_state_remap",
            generator_origin = ?plan.generator_origin,
            callee = ?plan.function_id,
            preserved_location = slot_index,
            logical_name = slot.logical_name.as_str(),
            storage = ?slot.storage,
            init = ?slot.init,
            local = temp.resolved_name().id_str(),
            "typed_generator_preserved_local_created",
        );
        let preserved_location = PreservedLocation(
            u32::try_from(slot_index).expect("preserved slot index should fit in u32"),
        );
        if slot.storage == PreservedSlotStorage::PyCellObject {
            ensure_typed_owned_cell_alias_for_preserved_local(function, temp.resolved_name());
        }
        preserved_locals.insert(preserved_location, temp);
    }
    let preserved_local_names = preserved_locals
        .iter()
        .map(|(location, local)| (*location, local.resolved_name()))
        .collect::<HashMap<_, _>>();
    let preserved_locals_by_name = public_layout
        .preserved_slots
        .iter()
        .enumerate()
        .filter_map(|(slot_index, slot)| {
            preserved_locals
                .get(&PreservedLocation(
                    u32::try_from(slot_index).expect("preserved slot index should fit in u32"),
                ))
                .map(TypedTempLocal::resolved_name)
                .map(|local| (slot.logical_name.clone(), local))
        })
        .collect::<HashMap<_, _>>();
    append_typed_cleanup_dels_to_body(&mut replacement, &arg_temps);
    function.blocks[constructor_block_index].body.splice(
        constructor_instr_index..=constructor_instr_index,
        replacement,
    );

    let helper_remaps = rewrite_typed_generator_state_helper_calls(
        function,
        &alias_cleanup.alias_locations,
        &alias_cleanup.is_closed_value_locations,
        &preserved_locals_by_name,
    );
    let removed_owner_stores = if effective_plan.pending_alias_use_instr_ids.is_empty() {
        remove_typed_generator_alias_setup(function, module_constants, &alias_cleanup)
    } else {
        0
    };
    let remapped_instrs = helper_remaps
        + remap_typed_generator_preserved_instrs(
            function,
            &effective_plan.body_instr_ids,
            &preserved_local_names,
        );
    trace_typed_generator_state_remaining_preserved(function, plan);
    Some((
        TypedGeneratorStateLoweringStats {
            lowered_generators: 1,
            initialized_slots: preserved_locals.len(),
            remapped_instrs,
            removed_owner_stores,
        },
        preserved_local_names,
    ))
}

fn typed_generator_state_constructor_dominated_body_instr_ids(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    constructor_block_index: usize,
    constructor_instr_index: usize,
    body_instr_ids: &HashSet<InstrId>,
    active_blocks: Option<&HashSet<BlockLabel>>,
) -> HashSet<InstrId> {
    if body_instr_ids.is_empty() {
        return HashSet::new();
    }
    let Some(constructor_block) = function.blocks.get(constructor_block_index) else {
        return HashSet::new();
    };
    let constructor_label = constructor_block.label;
    let dominators = typed_block_dominators(function);
    let mut dominated = HashSet::new();

    for block in &function.blocks {
        if active_blocks.is_some_and(|active_blocks| !active_blocks.contains(&block.label)) {
            continue;
        }
        for (instr_index, instr) in block.body.iter().enumerate() {
            let instruction_is_dominated = if block.label == constructor_label {
                constructor_instr_index < instr_index
            } else {
                dominators.block_dominates(constructor_label, block.label)
            };
            if !instruction_is_dominated {
                continue;
            }
            dominated.extend(typed_instr_matching_instr_ids(instr, body_instr_ids));
        }
        if block.label == constructor_label
            || dominators.block_dominates(constructor_label, block.label)
        {
            dominated.extend(typed_term_matching_instr_ids(&block.term, body_instr_ids));
        }
    }

    dominated
}

fn trace_typed_generator_state_lowering_skip(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &TypedGeneratorStateLoweringPlan,
    reason: &'static str,
) {
    tracing::info!(
        target: "soac_generator_state_lowering",
        caller = ?function.function_id,
        generator_origin = ?plan.generator_origin,
        callee = ?plan.function_id,
        has_materialized_constructor = plan.materialized_constructor.is_some(),
        materialized_constructor_target = ?plan
            .materialized_constructor
            .as_ref()
            .map(|constructor| constructor.target.id_str()),
        reason,
        "typed_generator_state_lowering_skipped",
    );
}

fn trace_typed_generator_constructor_candidates(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &TypedGeneratorStateLoweringPlan,
) {
    struct Finder<'a> {
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        plan: &'a TypedGeneratorStateLoweringPlan,
    }

    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(plan_candidate) = expr.generator_instance_plan() {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    caller = ?self.function.function_id,
                    generator_origin = ?self.plan.generator_origin,
                    callee = ?self.plan.function_id,
                    candidate_instr_id = ?expr.try_semantic_instr_id(),
                    candidate_function = ?plan_candidate.function_id,
                    candidate = ?expr,
                    "typed_generator_state_lowering_constructor_candidate",
                );
            } else if expr.try_semantic_instr_id() == Some(self.plan.generator_origin) {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    caller = ?self.function.function_id,
                    generator_origin = ?self.plan.generator_origin,
                    callee = ?self.plan.function_id,
                    candidate = ?expr,
                    "typed_generator_state_lowering_origin_candidate_without_plan",
                );
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder { function, plan };
    finder.visit_fn(function);
    for block in &function.blocks {
        for instr in &block.body {
            let InstrTyped::Store(store) = instr else {
                continue;
            };
            if !store.name.id_str().contains("typed_inline_arg") {
                continue;
            }
            tracing::info!(
                target: "soac_generator_state_lowering",
                caller = ?function.function_id,
                generator_origin = ?plan.generator_origin,
                callee = ?plan.function_id,
                target_name = store.name.id_str(),
                value = ?store.value,
                "typed_generator_state_lowering_inline_arg_store",
            );
        }
    }
}

fn trace_typed_generator_state_remaining_preserved(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &TypedGeneratorStateLoweringPlan,
) {
    #[derive(Clone, Copy)]
    enum CurrentTopLevel<'a> {
        Instr(&'a InstrTyped),
        Term(&'a BlockTerm<InstrTyped>),
    }

    impl std::fmt::Debug for CurrentTopLevel<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Instr(instr) => instr.fmt(f),
                Self::Term(term) => term.fmt(f),
            }
        }
    }

    struct Finder<'a> {
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        plan: &'a TypedGeneratorStateLoweringPlan,
        current_top_level: Option<CurrentTopLevel<'a>>,
    }

    impl<'a> Finder<'a> {
        fn visit_top_level_instr(&mut self, expr: &'a InstrTyped) {
            self.current_top_level = Some(CurrentTopLevel::Instr(expr));
            self.visit_instr(expr);
        }

        fn visit_top_level_term(&mut self, term: &'a BlockTerm<InstrTyped>) {
            self.current_top_level = Some(CurrentTopLevel::Term(term));
            self.visit_term(term);
        }

        fn trace_current(&self, kind: &'static str) {
            tracing::info!(
                target: "soac_generator_state_lowering",
                caller = ?self.function.function_id,
                generator_origin = ?self.plan.generator_origin,
                callee = ?self.plan.function_id,
                kind,
                top_level = ?self.current_top_level,
                "typed_generator_state_lowering_remaining_preserved",
            );
        }
    }

    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            match expr {
                InstrTyped::Load(load) if load.name.preserved_location().is_some() => {
                    self.trace_current("load");
                    return;
                }
                InstrTyped::Store(store) if store.name.preserved_location().is_some() => {
                    self.trace_current("store");
                    return;
                }
                InstrTyped::Del(del) if del.name.preserved_location().is_some() => {
                    self.trace_current("del");
                    return;
                }
                InstrTyped::Load(load)
                    if matches!(load.name.cell_location(), Some(CellLocation::Preserved(_))) =>
                {
                    self.trace_current("cell_load");
                    return;
                }
                InstrTyped::Store(store)
                    if matches!(store.name.cell_location(), Some(CellLocation::Preserved(_))) =>
                {
                    self.trace_current("cell_store");
                    return;
                }
                InstrTyped::Del(del)
                    if matches!(del.name.cell_location(), Some(CellLocation::Preserved(_))) =>
                {
                    self.trace_current("cell_del");
                    return;
                }
                InstrTyped::CellRef(cell_ref) if cell_ref.location.is_preserved() => {
                    self.trace_current("cell_ref");
                    return;
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        function,
        plan,
        current_top_level: None,
    };
    for block in &function.blocks {
        for instr in &block.body {
            finder.visit_top_level_instr(instr);
        }
        finder.visit_top_level_term(&block.term);
    }
}

fn find_typed_generator_constructor_store(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    generator_origin: InstrId,
) -> Option<(usize, usize, ResolvedName, TypedCall<InstrTyped>)> {
    function
        .blocks
        .iter()
        .enumerate()
        .find_map(|(block_index, block)| {
            block
                .body
                .iter()
                .enumerate()
                .find_map(|(instr_index, instr)| {
                    let InstrTyped::Store(store) = instr else {
                        return None;
                    };
                    let call = typed_generator_state_constructor_call(store.value.as_ref())?;
                    (store.value.try_semantic_instr_id() == Some(generator_origin))
                        .then(|| (block_index, instr_index, store.name.clone(), call))
                })
        })
}

fn find_typed_materialized_generator_constructor_store(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    generator_origin: InstrId,
    constructor: &TypedGeneratorStateConstructor,
) -> Option<(usize, usize, ResolvedName, TypedCall<InstrTyped>)> {
    let matching_store = function
        .blocks
        .iter()
        .enumerate()
        .find_map(|(block_index, block)| {
            block
                .body
                .iter()
                .enumerate()
                .find_map(|(instr_index, instr)| {
                    let InstrTyped::Store(store) = instr else {
                        return None;
                    };
                    (store.name == constructor.target
                        && store.value.try_semantic_instr_id() == Some(generator_origin))
                    .then(|| {
                        (
                            block_index,
                            instr_index,
                            store.name.clone(),
                            constructor.call.clone(),
                        )
                    })
                })
        });
    matching_store.or_else(|| {
        function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block_index, block)| {
                block
                    .body
                    .iter()
                    .enumerate()
                    .find_map(|(instr_index, instr)| {
                        let InstrTyped::Store(store) = instr else {
                            return None;
                        };
                        (store.name == constructor.target).then(|| {
                            (
                                block_index,
                                instr_index,
                                store.name.clone(),
                                constructor.call.clone(),
                            )
                        })
                    })
            })
    })
}

struct TypedGeneratorAliasCleanup {
    alias_locations: HashSet<LocalLocation>,
    resume_function_locations: HashSet<LocalLocation>,
    owner_locations: HashSet<LocalLocation>,
    state_locations: HashSet<LocalLocation>,
    state_value_locations: HashSet<LocalLocation>,
    is_closed_value_locations: HashSet<LocalLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypedGeneratorStateHelperKind {
    IsClosed,
    CurrentYieldFrom,
    CurrentThrowContext,
}

impl TypedGeneratorStateHelperKind {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "_is_generator_closed" => Some(Self::IsClosed),
            "_current_yieldfrom" => Some(Self::CurrentYieldFrom),
            "_current_throw_context" => Some(Self::CurrentThrowContext),
            _ => None,
        }
    }

    fn preserved_logical_name(self) -> &'static str {
        match self {
            Self::IsClosed => "_dp_is_closed",
            Self::CurrentYieldFrom => "_dp_yieldfrom",
            Self::CurrentThrowContext => "_dp_throw_context",
        }
    }
}

fn typed_generator_alias_cleanup(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    generator_location: LocalLocation,
    ignored_alias_use_instr_ids: &HashSet<InstrId>,
    active_blocks: Option<&HashSet<BlockLabel>>,
) -> Option<TypedGeneratorAliasCleanup> {
    let alias_locations = collect_typed_generator_alias_locations(
        function,
        module_constants,
        generator_location,
        active_blocks,
    );
    let resume_function_locations = collect_typed_local_copy_closure(
        function,
        function
            .blocks
            .iter()
            .filter(|block| {
                active_blocks.is_none_or(|active_blocks| active_blocks.contains(&block.label))
            })
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let location = store.name.local_location()?;
                typed_generator_resume_function_attr_load(
                    store.value.as_ref(),
                    module_constants,
                    &alias_locations,
                )
                .then_some(location)
            })
            .collect::<HashSet<_>>(),
    );
    let state_value_locations = collect_typed_local_copy_closure(
        function,
        function
            .blocks
            .iter()
            .filter(|block| {
                active_blocks.is_none_or(|active_blocks| active_blocks.contains(&block.label))
            })
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let location = store.name.local_location()?;
                typed_generator_preserved_state_attr_load(
                    store.value.as_ref(),
                    module_constants,
                    &alias_locations,
                )
                .then_some(location)
            })
            .collect::<HashSet<_>>(),
    );
    let is_closed_value_locations = function
        .blocks
        .iter()
        .filter(|block| {
            active_blocks.is_none_or(|active_blocks| active_blocks.contains(&block.label))
        })
        .flat_map(|block| block.body.iter())
        .filter_map(|instr| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            let location = store.name.local_location()?;
            (typed_generator_state_helper_alias_call(store.value.as_ref(), &alias_locations)
                == Some(TypedGeneratorStateHelperKind::IsClosed))
            .then_some(location)
        })
        .collect::<HashSet<_>>();
    #[derive(Clone, Copy)]
    enum CurrentTopLevel<'a> {
        Instr(&'a InstrTyped),
        Term(&'a BlockTerm<InstrTyped>),
    }

    impl std::fmt::Debug for CurrentTopLevel<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Instr(instr) => instr.fmt(f),
                Self::Term(term) => term.fmt(f),
            }
        }
    }

    struct Uses<'a> {
        alias_locations: HashSet<LocalLocation>,
        resume_function_locations: HashSet<LocalLocation>,
        owner_locations: HashSet<LocalLocation>,
        state_locations: HashSet<LocalLocation>,
        state_value_locations: HashSet<LocalLocation>,
        is_closed_value_locations: HashSet<LocalLocation>,
        module_constants: &'a [ConstantExpr],
        ignored_alias_use_instr_ids: &'a HashSet<InstrId>,
        current_top_level: Option<CurrentTopLevel<'a>>,
        residual: bool,
    }

    impl<'a> Uses<'a> {
        fn visit_top_level_instr(&mut self, expr: &'a InstrTyped) {
            self.current_top_level = Some(CurrentTopLevel::Instr(expr));
            match expr {
                InstrTyped::Store(store)
                    if store
                        .name
                        .local_location()
                        .is_some_and(|location| self.alias_locations.contains(&location))
                        && typed_generator_alias_expr(
                            store.value.as_ref(),
                            self.module_constants,
                            &self.alias_locations,
                        ) =>
                {
                    return;
                }
                InstrTyped::Store(store)
                    if store.name.local_location().is_some_and(|location| {
                        self.resume_function_locations.contains(&location)
                    }) && typed_generator_resume_function_attr_load(
                        store.value.as_ref(),
                        &self.module_constants,
                        &self.alias_locations,
                    ) =>
                {
                    return;
                }
                InstrTyped::Store(store)
                    if store.name.local_location().is_some_and(|location| {
                        self.resume_function_locations.contains(&location)
                    }) && typed_generator_resume_function_value_load(
                        store.value.as_ref(),
                        &self.resume_function_locations,
                    ) =>
                {
                    return;
                }
                InstrTyped::Store(store)
                    if store
                        .name
                        .local_location()
                        .is_some_and(|location| self.state_value_locations.contains(&location))
                        && (typed_generator_preserved_state_attr_load(
                            store.value.as_ref(),
                            &self.module_constants,
                            &self.alias_locations,
                        ) || typed_generator_preserved_state_value_load(
                            store.value.as_ref(),
                            &self.state_value_locations,
                        )) =>
                {
                    return;
                }
                InstrTyped::Store(store)
                    if store.name.local_location().is_some_and(|location| {
                        self.is_closed_value_locations.contains(&location)
                    }) && typed_generator_is_closed_value_expr(
                        store.value.as_ref(),
                        &self.alias_locations,
                        &self.is_closed_value_locations,
                    ) =>
                {
                    return;
                }
                InstrTyped::Store(store)
                    if store.name.id_str() == "_dp_self"
                        && typed_generator_alias_expr(
                            store.value.as_ref(),
                            self.module_constants,
                            &self.alias_locations,
                        ) =>
                {
                    if let Some(location) = store.name.local_location() {
                        self.owner_locations.insert(location);
                    }
                    return;
                }
                InstrTyped::Store(store)
                    if store.name.id_str() == "_dp_state"
                        && (typed_generator_alias_expr(
                            store.value.as_ref(),
                            self.module_constants,
                            &self.alias_locations,
                        ) || typed_generator_preserved_state_attr_load(
                            store.value.as_ref(),
                            &self.module_constants,
                            &self.alias_locations,
                        ) || typed_generator_preserved_state_value_load(
                            store.value.as_ref(),
                            &self.state_value_locations,
                        )) =>
                {
                    if let Some(location) = store.name.local_location() {
                        self.state_locations.insert(location);
                    }
                    return;
                }
                InstrTyped::Del(del)
                    if del.name.local_location().is_some_and(|location| {
                        self.alias_locations.contains(&location)
                            || self.resume_function_locations.contains(&location)
                            || self.owner_locations.contains(&location)
                            || self.state_locations.contains(&location)
                            || self.state_value_locations.contains(&location)
                            || self.is_closed_value_locations.contains(&location)
                    }) =>
                {
                    return;
                }
                _ => {}
            }
            self.visit_instr(expr);
        }

        fn visit_top_level_term(&mut self, term: &'a BlockTerm<InstrTyped>) {
            self.current_top_level = Some(CurrentTopLevel::Term(term));
            self.visit_term(term);
        }
    }

    impl Visit<InstrTyped> for Uses<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.residual {
                return;
            }
            if expr
                .try_semantic_instr_id()
                .is_some_and(|instr_id| self.ignored_alias_use_instr_ids.contains(&instr_id))
            {
                return;
            }
            if let InstrTyped::Store(store) = expr
                && store
                    .value
                    .try_semantic_instr_id()
                    .is_some_and(|instr_id| self.ignored_alias_use_instr_ids.contains(&instr_id))
            {
                return;
            }
            if let InstrTyped::Truthy(truthy) = expr
                && typed_generator_is_closed_value_expr(
                    truthy.value(),
                    &self.alias_locations,
                    &self.is_closed_value_locations,
                )
            {
                return;
            }
            if typed_generator_is_closed_value_expr(
                expr,
                &self.alias_locations,
                &self.is_closed_value_locations,
            ) {
                return;
            }
            if matches!(
                typed_generator_state_helper_alias_call(expr, &self.alias_locations),
                Some(
                    TypedGeneratorStateHelperKind::CurrentYieldFrom
                        | TypedGeneratorStateHelperKind::CurrentThrowContext
                )
            ) {
                return;
            }
            if let InstrTyped::Load(load) = expr
                && load.name.local_location().is_some_and(|location| {
                    self.alias_locations.contains(&location)
                        || self.resume_function_locations.contains(&location)
                        || self.owner_locations.contains(&location)
                        || self.state_locations.contains(&location)
                        || self.state_value_locations.contains(&location)
                        || self.is_closed_value_locations.contains(&location)
                })
            {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    alias_location = ?load.name.local_location(),
                    top_level = ?self.current_top_level,
                    "typed_generator_state_lowering_residual_alias_use",
                );
                self.residual = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut uses = Uses {
        alias_locations,
        resume_function_locations,
        owner_locations: HashSet::new(),
        state_locations: HashSet::new(),
        state_value_locations,
        is_closed_value_locations,
        module_constants,
        ignored_alias_use_instr_ids,
        current_top_level: None,
        residual: false,
    };
    for block in &function.blocks {
        if active_blocks.is_some_and(|active_blocks| !active_blocks.contains(&block.label)) {
            continue;
        }
        for instr in &block.body {
            uses.visit_top_level_instr(instr);
        }
        uses.visit_top_level_term(&block.term);
    }
    (!uses.residual).then_some(TypedGeneratorAliasCleanup {
        alias_locations: uses.alias_locations,
        resume_function_locations: uses.resume_function_locations,
        owner_locations: uses.owner_locations,
        state_locations: uses.state_locations,
        state_value_locations: uses.state_value_locations,
        is_closed_value_locations: uses.is_closed_value_locations,
    })
}

fn collect_typed_generator_alias_locations(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    generator_location: LocalLocation,
    active_blocks: Option<&HashSet<BlockLabel>>,
) -> HashSet<LocalLocation> {
    let mut aliases = HashSet::from([generator_location]);
    loop {
        let mut changed = false;
        for instr in function
            .blocks
            .iter()
            .filter(|block| {
                active_blocks.is_none_or(|active_blocks| active_blocks.contains(&block.label))
            })
            .flat_map(|block| block.body.iter())
        {
            let InstrTyped::Store(store) = instr else {
                continue;
            };
            if matches!(store.name.id_str(), "_dp_self" | "_dp_state") {
                continue;
            }
            let Some(target) = store.name.local_location() else {
                continue;
            };
            changed |= typed_generator_alias_expr(store.value.as_ref(), module_constants, &aliases)
                && aliases.insert(target);
        }
        if !changed {
            return aliases;
        }
    }
}

fn collect_typed_local_copy_closure(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    mut locations: HashSet<LocalLocation>,
) -> HashSet<LocalLocation> {
    loop {
        let mut changed = false;
        for instr in function.blocks.iter().flat_map(|block| block.body.iter()) {
            let InstrTyped::Store(store) = instr else {
                continue;
            };
            let Some(target) = store.name.local_location() else {
                continue;
            };
            changed |= typed_generator_preserved_state_value_load(store.value.as_ref(), &locations)
                && locations.insert(target);
        }
        if !changed {
            return locations;
        }
    }
}

fn typed_generator_alias_load(expr: &InstrTyped, aliases: &HashSet<LocalLocation>) -> bool {
    matches!(
        expr,
        InstrTyped::Load(load)
            if load.name.local_location().is_some_and(|location| aliases.contains(&location))
    )
}

fn typed_generator_alias_expr(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
    aliases: &HashSet<LocalLocation>,
) -> bool {
    typed_generator_alias_load(expr, aliases)
        || typed_generator_iter_alias_call(expr, module_constants, aliases)
}

fn typed_generator_iter_alias_call(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
    aliases: &HashSet<LocalLocation>,
) -> bool {
    let Some((func, args, keywords)) = typed_callable_call_parts(expr) else {
        return false;
    };
    keywords.is_empty()
        && typed_expr_is_runtime_name_load(func, RuntimeName::Iter, module_constants)
        && matches!(
            args,
            [CallArgPositional::Positional(owner)]
                if typed_generator_alias_load(owner, aliases)
        )
}

fn typed_generator_resume_function_attr_load(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
    aliases: &HashSet<LocalLocation>,
) -> bool {
    let InstrTyped::GetAttrTyped(get_attr) = expr else {
        return false;
    };
    typed_constant_string(get_attr.attr.as_ref(), module_constants) == Some("_resume_function")
        && typed_generator_alias_load(get_attr.value.as_ref(), aliases)
}

fn typed_generator_preserved_state_attr_load(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
    aliases: &HashSet<LocalLocation>,
) -> bool {
    let InstrTyped::GetAttrTyped(get_attr) = expr else {
        return false;
    };
    typed_constant_string(get_attr.attr.as_ref(), module_constants) == Some("_preserved_values")
        && typed_generator_alias_load(get_attr.value.as_ref(), aliases)
}

fn typed_generator_preserved_state_value_load(
    expr: &InstrTyped,
    locations: &HashSet<LocalLocation>,
) -> bool {
    matches!(
        expr,
        InstrTyped::Load(load)
            if load.name.local_location().is_some_and(|location| locations.contains(&location))
    )
}

fn typed_generator_resume_function_value_load(
    expr: &InstrTyped,
    locations: &HashSet<LocalLocation>,
) -> bool {
    matches!(
        expr,
        InstrTyped::Load(load)
            if load.name.local_location().is_some_and(|location| locations.contains(&location))
    )
}

fn typed_generator_state_helper_alias_call(
    expr: &InstrTyped,
    aliases: &HashSet<LocalLocation>,
) -> Option<TypedGeneratorStateHelperKind> {
    let InstrTyped::DirectCallableCallTyped(call) = expr else {
        return None;
    };
    let InstrTyped::Load(func) = call.func.as_ref() else {
        return None;
    };
    let helper = TypedGeneratorStateHelperKind::from_name(func.name.id_str())?;
    let [CallArgPositional::Positional(owner)] = call.args.as_slice() else {
        return None;
    };
    typed_generator_alias_load(owner, aliases).then_some(helper)
}

fn typed_generator_is_closed_value_load(
    expr: &InstrTyped,
    locations: &HashSet<LocalLocation>,
) -> bool {
    matches!(
        expr,
        InstrTyped::Load(load)
            if load.name.local_location().is_some_and(|location| locations.contains(&location))
    )
}

fn typed_generator_is_closed_value_expr(
    expr: &InstrTyped,
    aliases: &HashSet<LocalLocation>,
    locations: &HashSet<LocalLocation>,
) -> bool {
    typed_generator_state_helper_alias_call(expr, aliases)
        == Some(TypedGeneratorStateHelperKind::IsClosed)
        || typed_generator_is_closed_value_load(expr, locations)
}

fn remove_typed_generator_alias_setup(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    cleanup: &TypedGeneratorAliasCleanup,
) -> usize {
    let mut removed_owner_stores = 0;
    for block in &mut function.blocks {
        block.body.retain(|instr| {
            if let InstrTyped::Store(store) = instr
                && store.name.id_str() == "cols"
            {
                let location = store.name.local_location();
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    location = ?location,
                    alias = location.is_some_and(|location| cleanup.alias_locations.contains(&location)),
                    resume = location.is_some_and(|location| cleanup.resume_function_locations.contains(&location)),
                    owner = location.is_some_and(|location| cleanup.owner_locations.contains(&location)),
                    state = location.is_some_and(|location| cleanup.state_locations.contains(&location)),
                    state_value = location.is_some_and(|location| cleanup.state_value_locations.contains(&location)),
                    value = ?store.value,
                    "typed_generator_alias_cleanup_cols_candidate",
                );
            }
            let remove =
                match instr {
                    InstrTyped::Store(store)
                        if store.name.local_location().is_some_and(|location| {
                            cleanup.alias_locations.contains(&location)
                        }) && typed_generator_alias_expr(
                            store.value.as_ref(),
                            module_constants,
                            &cleanup.alias_locations,
                        ) =>
                    {
                        true
                    }
                    InstrTyped::Store(store)
                        if store.name.local_location().is_some_and(|location| {
                            cleanup.resume_function_locations.contains(&location)
                        }) && typed_generator_resume_function_attr_load(
                            store.value.as_ref(),
                            module_constants,
                            &cleanup.alias_locations,
                        ) =>
                    {
                        true
                    }
                    InstrTyped::Store(store)
                        if store.name.local_location().is_some_and(|location| {
                            cleanup.resume_function_locations.contains(&location)
                        }) && typed_generator_resume_function_value_load(
                            store.value.as_ref(),
                            &cleanup.resume_function_locations,
                        ) =>
                    {
                        true
                    }
                    InstrTyped::Store(store)
                        if store.name.local_location().is_some_and(|location| {
                            cleanup.state_value_locations.contains(&location)
                        }) && (typed_generator_preserved_state_attr_load(
                            store.value.as_ref(),
                            module_constants,
                            &cleanup.alias_locations,
                        ) || typed_generator_preserved_state_value_load(
                            store.value.as_ref(),
                            &cleanup.state_value_locations,
                        )) =>
                    {
                        true
                    }
                    InstrTyped::Store(store)
                        if store.name.local_location().is_some_and(|location| {
                            cleanup.is_closed_value_locations.contains(&location)
                        }) && typed_generator_is_closed_value_expr(
                            store.value.as_ref(),
                            &cleanup.alias_locations,
                            &cleanup.is_closed_value_locations,
                        ) =>
                    {
                        true
                    }
                    InstrTyped::Store(store)
                        if store.name.local_location().is_some_and(|location| {
                            cleanup.owner_locations.contains(&location)
                        }) && typed_generator_alias_expr(
                            store.value.as_ref(),
                            module_constants,
                            &cleanup.alias_locations,
                        ) =>
                    {
                        removed_owner_stores += 1;
                        true
                    }
                    InstrTyped::Store(store)
                        if store.name.local_location().is_some_and(|location| {
                            cleanup.state_locations.contains(&location)
                        }) && (typed_generator_alias_expr(
                            store.value.as_ref(),
                            module_constants,
                            &cleanup.alias_locations,
                        ) || typed_generator_preserved_state_attr_load(
                            store.value.as_ref(),
                            module_constants,
                            &cleanup.alias_locations,
                        ) || typed_generator_preserved_state_value_load(
                            store.value.as_ref(),
                            &cleanup.state_value_locations,
                        )) =>
                    {
                        removed_owner_stores += 1;
                        true
                    }
                    InstrTyped::Del(del)
                        if del.name.local_location().is_some_and(|location| {
                            cleanup.alias_locations.contains(&location)
                                || cleanup.resume_function_locations.contains(&location)
                                || cleanup.owner_locations.contains(&location)
                                || cleanup.state_locations.contains(&location)
                                || cleanup.state_value_locations.contains(&location)
                                || cleanup.is_closed_value_locations.contains(&location)
                        }) =>
                    {
                        true
                    }
                    _ => false,
                };
            !remove
        });
    }
    removed_owner_stores
}

fn rewrite_typed_generator_state_helper_calls(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    aliases: &HashSet<LocalLocation>,
    is_closed_value_locations: &HashSet<LocalLocation>,
    preserved_locals_by_name: &HashMap<String, ResolvedName>,
) -> usize {
    struct Rewriter<'a> {
        aliases: &'a HashSet<LocalLocation>,
        is_closed_value_locations: &'a HashSet<LocalLocation>,
        preserved_locals_by_name: &'a HashMap<String, ResolvedName>,
        changed: usize,
    }

    impl Rewriter<'_> {
        fn local_for(&self, helper: TypedGeneratorStateHelperKind) -> Option<&ResolvedName> {
            self.preserved_locals_by_name
                .get(helper.preserved_logical_name())
        }
    }

    impl VisitMut<InstrTyped> for Rewriter<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if let InstrTyped::Truthy(truthy) = expr
                && typed_generator_is_closed_value_expr(
                    truthy.value(),
                    self.aliases,
                    self.is_closed_value_locations,
                )
                && let Some(local) = self.local_for(TypedGeneratorStateHelperKind::IsClosed)
            {
                let meta = truthy.meta();
                *expr =
                    InstrTyped::Truthy(TypedTruthy::new(typed_load_temp(local)).with_meta(meta));
                self.changed += 1;
                return;
            }
            if typed_generator_is_closed_value_expr(
                expr,
                self.aliases,
                self.is_closed_value_locations,
            ) && let Some(local) = self.local_for(TypedGeneratorStateHelperKind::IsClosed)
            {
                *expr = typed_load_temp(local);
                self.changed += 1;
                return;
            }
            if let Some(helper @ TypedGeneratorStateHelperKind::CurrentYieldFrom)
            | Some(helper @ TypedGeneratorStateHelperKind::CurrentThrowContext) =
                typed_generator_state_helper_alias_call(expr, self.aliases)
                && let Some(local) = self.local_for(helper)
            {
                *expr = typed_load_temp(local);
                self.changed += 1;
                return;
            }
            expr.visit_children_mut(self);
        }
    }

    let mut rewriter = Rewriter {
        aliases,
        is_closed_value_locations,
        preserved_locals_by_name,
        changed: 0,
    };
    rewriter.visit_fn_mut(function);
    rewriter.changed
}

pub fn rewrite_lowered_typed_generator_state_helper_calls_with_existing_constructor(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor: &TypedGeneratorStateConstructor,
    preserved_locals_by_name: &HashMap<String, ResolvedName>,
    ignored_alias_use_instr_ids: &HashSet<InstrId>,
) -> usize {
    let Some(generator_location) = constructor.target.local_location() else {
        return 0;
    };
    let Some(cleanup) = typed_generator_alias_cleanup(
        function,
        module_constants,
        generator_location,
        ignored_alias_use_instr_ids,
        None,
    ) else {
        return 0;
    };
    rewrite_typed_generator_state_helper_calls(
        function,
        &cleanup.alias_locations,
        &cleanup.is_closed_value_locations,
        preserved_locals_by_name,
    )
}

pub fn remap_typed_generator_preserved_instrs_with_existing_locals(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    body_instr_ids: &HashSet<InstrId>,
    preserved_locals: &HashMap<PreservedLocation, ResolvedName>,
) -> usize {
    remap_typed_generator_preserved_instrs(function, body_instr_ids, preserved_locals)
}

pub fn cleanup_lowered_typed_generator_alias_setup_with_existing_constructor(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor: &TypedGeneratorStateConstructor,
    ignored_alias_use_instr_ids: &HashSet<InstrId>,
) -> usize {
    let Some(generator_location) = constructor.target.local_location() else {
        return 0;
    };
    let Some(cleanup) = typed_generator_alias_cleanup(
        function,
        module_constants,
        generator_location,
        ignored_alias_use_instr_ids,
        None,
    ) else {
        return 0;
    };
    remove_typed_generator_alias_setup(function, module_constants, &cleanup)
}

fn remap_typed_generator_preserved_instrs(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    body_instr_ids: &HashSet<InstrId>,
    preserved_locals: &HashMap<PreservedLocation, ResolvedName>,
) -> usize {
    let mut remapped = 0;
    let preserved_cell_aliases = typed_preserved_local_cell_aliases(function, preserved_locals);
    for block in &mut function.blocks {
        for instr in &mut block.body {
            if !typed_instr_contains_any_instr_id(instr, body_instr_ids) {
                continue;
            }
            let mut mapper = TypedGeneratorPreservedLocalRemapper::selective(
                preserved_locals,
                &preserved_cell_aliases,
                body_instr_ids,
            );
            let old = std::mem::replace(instr, InstrTyped::constant_none());
            *instr = mapper
                .try_map_instr(old)
                .expect("generator preserved-local remapping should be total");
            remapped += usize::from(mapper.changed);
        }
        if typed_term_contains_any_instr_id(&block.term, body_instr_ids) {
            let mut mapper = TypedGeneratorPreservedLocalRemapper::selective(
                preserved_locals,
                &preserved_cell_aliases,
                body_instr_ids,
            );
            let old = std::mem::replace(
                &mut block.term,
                BlockTerm::Return(InstrTyped::constant_none()),
            );
            block.term = mapper
                .try_map_term(old)
                .expect("generator preserved-local term remapping should be total");
            remapped += usize::from(mapper.changed);
        }
    }
    remapped
}

pub fn lower_typed_generator_resume_preserved_state_to_locals(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> TypedGeneratorResumeStateLoweringStats {
    lower_typed_generator_resume_preserved_state_to_locals_and_collect_preserved_locals(function)
        .stats
}

pub fn lower_typed_generator_resume_preserved_state_to_locals_and_collect_preserved_locals(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> TypedGeneratorResumeStateLoweringOutcome {
    let Some(public_layout) = function.public_storage_layout().cloned() else {
        return TypedGeneratorResumeStateLoweringOutcome::default();
    };
    if public_layout.preserved_slots.is_empty() {
        return TypedGeneratorResumeStateLoweringOutcome::default();
    }

    let terminal_boundary_blocks = function
        .blocks
        .iter()
        .filter(|block| typed_generator_resume_terminal_boundary_block(block))
        .map(|block| block.label)
        .collect::<HashSet<_>>();
    let preserved_delete_blocks = typed_preserved_delete_blocks(function);
    let mut lowered_slots = Vec::new();
    for (slot_index, slot) in public_layout.preserved_slots.iter().enumerate() {
        let location = PreservedLocation(
            u32::try_from(slot_index).expect("preserved slot index should fit in u32"),
        );
        let delete_blocks = preserved_delete_blocks.get(&location);
        if !typed_generator_resume_slot_can_live_locally(
            slot.storage,
            &slot.init,
            delete_blocks,
            &terminal_boundary_blocks,
        ) {
            continue;
        }
        let Ok(local) = try_allocate_typed_stack_temp(function, "typed_resume_state") else {
            return TypedGeneratorResumeStateLoweringOutcome::default();
        };
        if slot.storage == PreservedSlotStorage::PyCellObject {
            ensure_typed_owned_cell_alias_for_preserved_local(function, local.resolved_name());
        }
        lowered_slots.push((location, slot.storage_name.clone(), slot.storage, local));
    }
    if lowered_slots.is_empty() {
        return TypedGeneratorResumeStateLoweringOutcome::default();
    }

    let preserved_locals = lowered_slots
        .iter()
        .map(|(location, _, _, local)| (*location, local.resolved_name()))
        .collect::<HashMap<_, _>>();
    let remapped_instrs =
        remap_typed_generator_preserved_instrs_everywhere(function, &preserved_locals);
    let mut instr_id_allocator = TypedInlineInstrIdAllocator::from_function(function);

    let entry_label = function.entry_block().label;
    let entry = function
        .blocks
        .iter_mut()
        .find(|block| block.label == entry_label)
        .expect("typed generator resume entry block should exist");
    let mut entry_transfers = Vec::new();
    for (location, storage_name, storage, local) in &lowered_slots {
        let preserved_name = typed_preserved_slot_name(storage_name, *location);
        entry_transfers.push(typed_instr_with_fresh_synthetic_instr_id(
            typed_store_temp(
                local.resolved_name(),
                InstrTyped::Load(Load::new(preserved_name.clone()).with_meta(Meta::synthetic())),
            ),
            &mut instr_id_allocator,
        ));
        if matches!(
            storage,
            PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::PyCellObject
        ) {
            // The resume-local slot owns the active value while the function is running.
            // Clearing the preserved owner here keeps overwrite/delete decrefs observable
            // at the original statement boundary instead of delaying them until yield.
            entry_transfers.push(typed_instr_with_fresh_synthetic_instr_id(
                Del::new(preserved_name, true)
                    .with_meta(Meta::synthetic())
                    .into(),
                &mut instr_id_allocator,
            ));
        }
    }
    let entry_transfer_count = entry_transfers.len();
    entry.body.splice(0..0, entry_transfers);

    let mut boundary_writebacks = 0;
    for block in &mut function.blocks {
        if !typed_generator_resume_boundary_block(block) {
            continue;
        }
        let mut writebacks = Vec::new();
        for (location, storage_name, _, local) in &lowered_slots {
            let terminally_cleared_here = preserved_delete_blocks
                .get(location)
                .is_some_and(|blocks| blocks.contains(&block.label));
            if terminally_cleared_here {
                continue;
            }
            writebacks.push(typed_instr_with_fresh_synthetic_instr_id(
                Store::new(
                    typed_preserved_slot_name(storage_name, *location),
                    Box::new(typed_load_temp(&local.resolved_name())),
                )
                .with_meta(Meta::synthetic())
                .into(),
                &mut instr_id_allocator,
            ));
        }
        boundary_writebacks += writebacks.len();
        block.body.extend(writebacks);
    }

    TypedGeneratorResumeStateLoweringOutcome {
        stats: TypedGeneratorResumeStateLoweringStats {
            lowered_functions: 1,
            lowered_slots: lowered_slots.len(),
            entry_transfers: entry_transfer_count,
            boundary_writebacks,
            remapped_instrs,
        },
        preserved_locals,
    }
}

pub fn ensure_typed_generator_resume_boundary_writebacks(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    preserved_locals: &HashMap<PreservedLocation, ResolvedName>,
) -> usize {
    if preserved_locals.is_empty() {
        return 0;
    }
    let Some(public_layout) = function.public_storage_layout().cloned() else {
        return 0;
    };
    let preserved_delete_blocks = typed_preserved_delete_blocks(function);
    let lowered_local_delete_blocks =
        typed_generator_resume_lowered_local_delete_blocks(function, preserved_locals);
    let mut instr_id_allocator = TypedInlineInstrIdAllocator::from_function(function);
    let mut inserted = 0;

    for block in &mut function.blocks {
        if !typed_generator_resume_boundary_block(block) {
            continue;
        }
        for (location, local) in preserved_locals {
            let Some(slot) = public_layout
                .preserved_slots
                .get(usize::try_from(location.0).expect("preserved slot should fit in usize"))
            else {
                continue;
            };
            let terminally_cleared_here = preserved_delete_blocks
                .get(location)
                .is_some_and(|blocks| blocks.contains(&block.label))
                || lowered_local_delete_blocks
                    .get(location)
                    .is_some_and(|blocks| blocks.contains(&block.label));
            if terminally_cleared_here
                || typed_generator_resume_boundary_has_writeback(block, *location, local)
            {
                continue;
            }
            block.body.push(typed_instr_with_fresh_synthetic_instr_id(
                Store::new(
                    typed_preserved_slot_name(slot.storage_name.as_str(), *location),
                    Box::new(typed_load_temp(local)),
                )
                .with_meta(Meta::synthetic())
                .into(),
                &mut instr_id_allocator,
            ));
            inserted += 1;
        }
    }

    inserted
}

fn typed_generator_resume_lowered_local_delete_blocks(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    preserved_locals: &HashMap<PreservedLocation, ResolvedName>,
) -> HashMap<PreservedLocation, HashSet<BlockLabel>> {
    struct Collector<'a> {
        preserved_locals: &'a HashMap<PreservedLocation, ResolvedName>,
        deleted: HashSet<PreservedLocation>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::Del(del) = expr {
                for (location, local) in self.preserved_locals {
                    if del.name == *local {
                        self.deleted.insert(*location);
                        return;
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    let mut delete_blocks = HashMap::<PreservedLocation, HashSet<BlockLabel>>::new();
    for block in &function.blocks {
        let mut collector = Collector {
            preserved_locals,
            deleted: HashSet::new(),
        };
        for instr in &block.body {
            collector.visit_instr(instr);
        }
        collector.visit_term(&block.term);
        for location in collector.deleted {
            delete_blocks
                .entry(location)
                .or_default()
                .insert(block.label);
        }
    }
    delete_blocks
}

fn typed_generator_resume_boundary_has_writeback(
    block: &TypedBlock,
    location: PreservedLocation,
    local: &ResolvedName,
) -> bool {
    block.body.iter().any(|instr| {
        let InstrTyped::Store(store) = instr else {
            return false;
        };
        if store.name.preserved_location() != Some(location) {
            return false;
        }
        matches!(
            store.value.as_ref(),
            InstrTyped::Load(load) if load.name == *local
        )
    })
}

fn typed_generator_resume_slot_can_live_locally(
    storage: PreservedSlotStorage,
    init: &ClosureInit,
    delete_blocks: Option<&HashSet<BlockLabel>>,
    terminal_boundary_blocks: &HashSet<BlockLabel>,
) -> bool {
    let has_only_terminal_deletes = delete_blocks
        .map(|blocks| {
            !blocks.is_empty()
                && blocks
                    .iter()
                    .all(|block| terminal_boundary_blocks.contains(block))
        })
        .unwrap_or(true);
    match storage {
        PreservedSlotStorage::I64 => {
            delete_blocks.is_none()
                && matches!(
                    init,
                    ClosureInit::RuntimePcUnstarted
                        | ClosureInit::RuntimeAbruptKindFallthrough
                        | ClosureInit::RuntimeZero
                )
        }
        PreservedSlotStorage::PyObjectOrNull => {
            has_only_terminal_deletes
                && matches!(init, ClosureInit::Parameter | ClosureInit::RuntimeNone)
        }
        PreservedSlotStorage::PyCellObject => {
            has_only_terminal_deletes
                && matches!(init, ClosureInit::Parameter | ClosureInit::EmptyCell)
        }
    }
}

fn typed_generator_resume_boundary_block(block: &TypedBlock) -> bool {
    matches!(block.term, BlockTerm::Return(_))
        || typed_generator_resume_terminal_boundary_block(block)
}

fn typed_generator_resume_terminal_boundary_block(block: &TypedBlock) -> bool {
    matches!(block.term, BlockTerm::Raise(_)) && block.exc_edge.is_none()
}

fn typed_preserved_delete_blocks(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<PreservedLocation, HashSet<BlockLabel>> {
    struct Collector {
        deleted: HashSet<PreservedLocation>,
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::Del(del) = expr
                && let Some(location) = del.name.preserved_location()
            {
                self.deleted.insert(location);
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut delete_blocks = HashMap::<PreservedLocation, HashSet<BlockLabel>>::new();
    for block in &function.blocks {
        let mut collector = Collector {
            deleted: HashSet::new(),
        };
        for instr in &block.body {
            collector.visit_instr(instr);
        }
        collector.visit_term(&block.term);
        for location in collector.deleted {
            delete_blocks
                .entry(location)
                .or_default()
                .insert(block.label);
        }
    }
    delete_blocks
}

fn typed_preserved_slot_name(storage_name: &str, location: PreservedLocation) -> ResolvedName {
    ResolvedName {
        id: storage_name.into(),
        location: NameLocation::Preserved(location),
    }
}

fn ensure_typed_owned_cell_alias_for_preserved_local(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    local: ResolvedName,
) -> CellLocation {
    let layout = function
        .storage_layout
        .as_mut()
        .expect("localized preserved cells should have caller storage");
    if let Some((slot, _)) = layout
        .cellvars
        .iter()
        .enumerate()
        .find(|(_, slot)| slot.storage_name == local.id_str())
    {
        return CellLocation::Owned(
            u32::try_from(slot).expect("owned preserved-cell alias slot should fit in u32"),
        );
    }
    let slot = u32::try_from(layout.cellvars.len())
        .expect("owned preserved-cell alias slot should fit in u32");
    layout.cellvars.push(ClosureSlot {
        logical_name: local.id_str().to_string(),
        storage_name: local.id_str().to_string(),
        init: ClosureInit::Deferred,
    });
    CellLocation::Owned(slot)
}

fn typed_preserved_local_cell_aliases(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    preserved_locals: &HashMap<PreservedLocation, ResolvedName>,
) -> HashMap<PreservedLocation, CellLocation> {
    let Some(layout) = function.storage_layout.as_ref() else {
        return HashMap::new();
    };
    preserved_locals
        .iter()
        .filter_map(|(location, local)| {
            layout
                .cellvars
                .iter()
                .position(|slot| slot.storage_name == local.id_str())
                .map(|slot| {
                    (
                        *location,
                        CellLocation::Owned(
                            u32::try_from(slot)
                                .expect("owned preserved-cell alias slot should fit in u32"),
                        ),
                    )
                })
        })
        .collect()
}

fn remap_typed_generator_preserved_instrs_everywhere(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    preserved_locals: &HashMap<PreservedLocation, ResolvedName>,
) -> usize {
    let mut remapped = 0;
    let preserved_cell_aliases = typed_preserved_local_cell_aliases(function, preserved_locals);
    for block in &mut function.blocks {
        for instr in &mut block.body {
            let mut mapper = TypedGeneratorPreservedLocalRemapper::everywhere(
                preserved_locals,
                &preserved_cell_aliases,
            );
            let old = std::mem::replace(instr, InstrTyped::constant_none());
            *instr = mapper
                .try_map_instr(old)
                .expect("generator resume preserved-local remapping should be total");
            remapped += 1;
        }
        let mut mapper = TypedGeneratorPreservedLocalRemapper::everywhere(
            preserved_locals,
            &preserved_cell_aliases,
        );
        let old = std::mem::replace(
            &mut block.term,
            BlockTerm::Return(InstrTyped::constant_none()),
        );
        block.term = mapper
            .try_map_term(old)
            .expect("generator resume preserved-local term remapping should be total");
        remapped += 1;
    }
    remapped
}

fn typed_instr_contains_any_instr_id(instr: &InstrTyped, instr_ids: &HashSet<InstrId>) -> bool {
    !typed_instr_matching_instr_ids(instr, instr_ids).is_empty()
}

fn typed_instr_matching_instr_ids(
    instr: &InstrTyped,
    instr_ids: &HashSet<InstrId>,
) -> HashSet<InstrId> {
    struct Collector<'a> {
        instr_ids: &'a HashSet<InstrId>,
        matched: HashSet<InstrId>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if expr
                .try_semantic_instr_id()
                .is_some_and(|instr_id| self.instr_ids.contains(&instr_id))
            {
                self.matched.insert(
                    expr.try_semantic_instr_id()
                        .expect("matched instruction should retain its semantic id"),
                );
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        instr_ids,
        matched: HashSet::new(),
    };
    collector.visit_instr(instr);
    collector.matched
}

fn typed_term_contains_any_instr_id(
    term: &BlockTerm<InstrTyped>,
    instr_ids: &HashSet<InstrId>,
) -> bool {
    !typed_term_matching_instr_ids(term, instr_ids).is_empty()
}

fn typed_term_matching_instr_ids(
    term: &BlockTerm<InstrTyped>,
    instr_ids: &HashSet<InstrId>,
) -> HashSet<InstrId> {
    struct Collector<'a> {
        instr_ids: &'a HashSet<InstrId>,
        matched: HashSet<InstrId>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if expr
                .try_semantic_instr_id()
                .is_some_and(|instr_id| self.instr_ids.contains(&instr_id))
            {
                self.matched.insert(
                    expr.try_semantic_instr_id()
                        .expect("matched instruction should retain its semantic id"),
                );
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        instr_ids,
        matched: HashSet::new(),
    };
    collector.visit_term(term);
    collector.matched
}

struct TypedGeneratorPreservedLocalRemapper<'a> {
    preserved_locals: &'a HashMap<PreservedLocation, ResolvedName>,
    preserved_cell_aliases: &'a HashMap<PreservedLocation, CellLocation>,
    body_instr_ids: Option<&'a HashSet<InstrId>>,
    active: bool,
    current_instr_id: Option<InstrId>,
    changed: bool,
}

impl<'a> TypedGeneratorPreservedLocalRemapper<'a> {
    fn everywhere(
        preserved_locals: &'a HashMap<PreservedLocation, ResolvedName>,
        preserved_cell_aliases: &'a HashMap<PreservedLocation, CellLocation>,
    ) -> Self {
        Self {
            preserved_locals,
            preserved_cell_aliases,
            body_instr_ids: None,
            active: true,
            current_instr_id: None,
            changed: false,
        }
    }

    fn selective(
        preserved_locals: &'a HashMap<PreservedLocation, ResolvedName>,
        preserved_cell_aliases: &'a HashMap<PreservedLocation, CellLocation>,
        body_instr_ids: &'a HashSet<InstrId>,
    ) -> Self {
        Self {
            preserved_locals,
            preserved_cell_aliases,
            body_instr_ids: Some(body_instr_ids),
            active: false,
            current_instr_id: None,
            changed: false,
        }
    }

    fn active_for(&self, instr: &InstrTyped) -> bool {
        match self.body_instr_ids {
            None => true,
            Some(body_instr_ids) => instr
                .try_semantic_instr_id()
                .map_or(self.active, |instr_id| body_instr_ids.contains(&instr_id)),
        }
    }
}

impl TryMapInstr<InstrTyped, InstrTyped, ()> for TypedGeneratorPreservedLocalRemapper<'_> {
    fn try_map_instr(&mut self, instr: InstrTyped) -> Result<InstrTyped, ()> {
        let previous_active = self.active;
        let previous_instr_id = self.current_instr_id;
        self.current_instr_id = instr.try_semantic_instr_id().or(previous_instr_id);
        self.active = self.active_for(&instr);
        let mapped = match instr {
            InstrTyped::Truthy(op) => InstrTyped::Truthy(op.try_map_children(self)?),
            InstrTyped::Load(op) => InstrTyped::Load(op.try_map_children(self)?),
            InstrTyped::BinOp(op) => InstrTyped::BinOp(op.try_map_children(self)?),
            InstrTyped::Tuple(op) => InstrTyped::Tuple(op.try_map_children(self)?),
            InstrTyped::UnaryOp(op) => InstrTyped::UnaryOp(op.try_map_children(self)?),
            InstrTyped::CalleeFunctionId(op) => {
                InstrTyped::CalleeFunctionId(op.try_map_children(self)?)
            }
            InstrTyped::CallTyped(op) => InstrTyped::CallTyped(op.try_map_children(self)?),
            InstrTyped::GuardedCallableCallTyped(op) => {
                InstrTyped::GuardedCallableCallTyped(op.try_map_children(self)?)
            }
            InstrTyped::GuardedMethodCallTyped(op) => {
                InstrTyped::GuardedMethodCallTyped(op.try_map_children(self)?)
            }
            InstrTyped::DirectCallableCallTyped(op) => {
                InstrTyped::DirectCallableCallTyped(op.try_map_children(self)?)
            }
            InstrTyped::DirectMethodCallTyped(op) => {
                InstrTyped::DirectMethodCallTyped(op.try_map_children(self)?)
            }
            InstrTyped::DirectCallGuardTest(op) => {
                InstrTyped::DirectCallGuardTest(op.try_map_children(self)?)
            }
            InstrTyped::CallDirect(op) => InstrTyped::CallDirect(op.try_map_children(self)?),
            InstrTyped::GetAttrTyped(op) => InstrTyped::GetAttrTyped(op.try_map_children(self)?),
            InstrTyped::SetAttrTyped(op) => InstrTyped::SetAttrTyped(op.try_map_children(self)?),
            InstrTyped::GetItem(op) => InstrTyped::GetItem(op.try_map_children(self)?),
            InstrTyped::SetItem(op) => InstrTyped::SetItem(op.try_map_children(self)?),
            InstrTyped::DelItem(op) => InstrTyped::DelItem(op.try_map_children(self)?),
            InstrTyped::Store(op) => InstrTyped::Store(op.try_map_children(self)?),
            InstrTyped::Del(op) => InstrTyped::Del(op.try_map_children(self)?),
            InstrTyped::MakeCell(op) => InstrTyped::MakeCell(op.try_map_children(self)?),
            InstrTyped::IncrementCounter(op) => InstrTyped::IncrementCounter(op),
            InstrTyped::CellRef(op) if self.active => match op.location {
                CellLocation::Preserved(slot) => self
                    .preserved_locals
                    .get(&PreservedLocation(slot))
                    .map(|local| {
                        self.changed = true;
                        tracing::info!(
                            target: "soac_generator_state_remap",
                            instr_id = ?self.current_instr_id,
                            preserved_location = slot,
                            local = local.id_str(),
                            kind = "cell_ref",
                            "typed_generator_preserved_local_remap",
                        );
                        typed_load_temp(local)
                    })
                    .unwrap_or(InstrTyped::CellRef(op)),
                _ => InstrTyped::CellRef(op),
            },
            InstrTyped::CellRef(op) => InstrTyped::CellRef(op),
            InstrTyped::MakeFunctionWithClosure(op) => {
                InstrTyped::MakeFunctionWithClosure(op.try_map_children(self)?)
            }
        };
        self.active = previous_active;
        self.current_instr_id = previous_instr_id;
        Ok(mapped)
    }

    fn try_map_name(&mut self, name: ResolvedName) -> Result<ResolvedName, ()> {
        if !self.active {
            return Ok(name);
        }
        if let Some(location) = name.location.as_preserved()
            && let Some(local) = self.preserved_locals.get(&location)
        {
            self.changed = true;
            tracing::info!(
                target: "soac_generator_state_remap",
                instr_id = ?self.current_instr_id,
                preserved_location = location.0,
                local = local.id_str(),
                kind = "name",
                "typed_generator_preserved_local_remap",
            );
            return Ok(local.clone());
        }
        if let Some(CellLocation::Preserved(slot)) = name.location.as_cell() {
            let location = PreservedLocation(slot);
            if let (Some(local), Some(cell_location)) = (
                self.preserved_locals.get(&location),
                self.preserved_cell_aliases.get(&location),
            ) {
                self.changed = true;
                tracing::info!(
                    target: "soac_generator_state_remap",
                    instr_id = ?self.current_instr_id,
                    preserved_location = location.0,
                    local = local.id_str(),
                    kind = "cell_name",
                    "typed_generator_preserved_local_remap",
                );
                return Ok(ResolvedName {
                    id: local.id.clone(),
                    location: NameLocation::Cell(*cell_location),
                });
            }
        }
        Ok(name)
    }
}

#[derive(Clone)]
struct TypedConstantLocal {
    value: InstrTyped,
    def_block: BlockLabel,
    def_index: usize,
}

fn collect_typed_i64_constant_local_defs(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> HashMap<LocalLocation, Vec<TypedConstantLocal>> {
    let mut candidates = HashMap::<LocalLocation, Vec<TypedConstantLocal>>::new();
    let mut constant_values = HashMap::<LocalLocation, i64>::new();
    let mut invalid = HashSet::<LocalLocation>::new();

    for block in &function.blocks {
        for (index, instr) in block.body.iter().enumerate() {
            match instr {
                InstrTyped::Store(store) => {
                    let Some(location) = store.name.local_location() else {
                        continue;
                    };
                    if invalid.contains(&location) {
                        continue;
                    }
                    let Some(constant_value) =
                        typed_expr_const_i64(store.value.as_ref(), module_constants)
                    else {
                        invalid.insert(location);
                        candidates.remove(&location);
                        constant_values.remove(&location);
                        continue;
                    };
                    if let Some(existing) = constant_values.get(&location)
                        && *existing != constant_value
                    {
                        invalid.insert(location);
                        candidates.remove(&location);
                        constant_values.remove(&location);
                        continue;
                    }
                    constant_values.entry(location).or_insert(constant_value);
                    candidates
                        .entry(location)
                        .or_default()
                        .push(TypedConstantLocal {
                            value: clear_typed_instr_ids(*store.value.clone()),
                            def_block: block.label,
                            def_index: index,
                        });
                }
                InstrTyped::Del(del) => {
                    if let Some(location) = del.name.local_location() {
                        invalid.insert(location);
                        candidates.remove(&location);
                        constant_values.remove(&location);
                    }
                }
                _ => {}
            }
        }
    }

    candidates
}

fn rewrite_dominated_typed_constant_loads(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    constant_locals: &HashMap<LocalLocation, Vec<TypedConstantLocal>>,
    dominators: &TypedBlockDominators,
) -> usize {
    struct Rewriter<'a> {
        constant_locals: &'a HashMap<LocalLocation, Vec<TypedConstantLocal>>,
        dominators: &'a TypedBlockDominators,
        block: BlockLabel,
        instr_index: usize,
        changed: usize,
    }

    impl VisitMut<InstrTyped> for Rewriter<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if let InstrTyped::Load(load) = expr
                && load.name.location.as_constant().is_none()
                && let Some(location) = load.name.local_location()
                && let Some(constants) = self.constant_locals.get(&location)
                && let Some(constant) = dominating_typed_constant_def_for_use(
                    constants,
                    self.block,
                    self.instr_index,
                    self.dominators,
                )
            {
                *expr = clear_typed_instr_ids(constant.value.clone());
                self.changed += 1;
                return;
            }
            expr.visit_children_mut(self);
        }
    }

    let mut changed = 0;
    for block in &mut function.blocks {
        for (instr_index, instr) in block.body.iter_mut().enumerate() {
            let mut rewriter = Rewriter {
                constant_locals,
                dominators,
                block: block.label,
                instr_index,
                changed: 0,
            };
            rewriter.visit_instr_mut(instr);
            changed += rewriter.changed;
        }
        let mut rewriter = Rewriter {
            constant_locals,
            dominators,
            block: block.label,
            instr_index: block.body.len(),
            changed: 0,
        };
        rewriter.visit_term_mut(&mut block.term);
        changed += rewriter.changed;
    }
    changed
}

trait TypedLocalDef {
    fn def_block(&self) -> BlockLabel;
    fn def_index(&self) -> usize;
}

impl TypedLocalDef for TypedTupleLocalDef {
    fn def_block(&self) -> BlockLabel {
        self.def_block
    }

    fn def_index(&self) -> usize {
        self.def_index
    }
}

impl TypedLocalDef for TypedConstantLocal {
    fn def_block(&self) -> BlockLabel {
        self.def_block
    }

    fn def_index(&self) -> usize {
        self.def_index
    }
}

fn dominating_typed_tuple_def_for_use<'a>(
    defs: &'a [TypedTupleLocalDef],
    use_block: BlockLabel,
    use_index: usize,
    dominators: &TypedBlockDominators,
) -> Option<&'a TypedTupleLocalDef> {
    select_dominating_typed_local_def(defs, use_block, use_index, dominators)
}

fn dominating_typed_constant_def_for_use<'a>(
    defs: &'a [TypedConstantLocal],
    use_block: BlockLabel,
    use_index: usize,
    dominators: &TypedBlockDominators,
) -> Option<&'a TypedConstantLocal> {
    select_dominating_typed_local_def(defs, use_block, use_index, dominators)
}

fn select_dominating_typed_local_def<'a, T: TypedLocalDef>(
    defs: &'a [T],
    use_block: BlockLabel,
    use_index: usize,
    dominators: &TypedBlockDominators,
) -> Option<&'a T> {
    let dominating = defs
        .iter()
        .enumerate()
        .filter(|(_, def)| {
            typed_local_def_dominates_use(
                def.def_block(),
                def.def_index(),
                use_block,
                use_index,
                dominators,
            )
        })
        .collect::<Vec<_>>();
    if dominating.is_empty() {
        return None;
    }
    let maximal = dominating
        .iter()
        .filter(|(index, def)| {
            !dominating.iter().any(|(other_index, other)| {
                index != other_index
                    && typed_local_def_dominates_use(
                        def.def_block(),
                        def.def_index(),
                        other.def_block(),
                        other.def_index(),
                        dominators,
                    )
            })
        })
        .map(|(_, def)| *def)
        .collect::<Vec<_>>();
    match maximal.as_slice() {
        [def] => Some(*def),
        _ => None,
    }
}

fn typed_local_def_dominates_use(
    def_block: BlockLabel,
    def_index: usize,
    use_block: BlockLabel,
    use_index: usize,
    dominators: &TypedBlockDominators,
) -> bool {
    if def_block == use_block {
        return def_index < use_index;
    }
    dominators.block_dominates(def_block, use_block)
}

#[derive(Default)]
struct TypedBlockDominators {
    enter: HashMap<BlockLabel, usize>,
    exit: HashMap<BlockLabel, usize>,
}

impl TypedBlockDominators {
    fn block_dominates(&self, dominator: BlockLabel, block: BlockLabel) -> bool {
        let (Some(dominator_enter), Some(dominator_exit), Some(block_enter), Some(block_exit)) = (
            self.enter.get(&dominator),
            self.exit.get(&dominator),
            self.enter.get(&block),
            self.exit.get(&block),
        ) else {
            return false;
        };
        dominator_enter <= block_enter && block_exit <= dominator_exit
    }
}

fn typed_block_dominators(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> TypedBlockDominators {
    let Some(entry) = function.blocks.first().map(|block| block.label) else {
        return TypedBlockDominators::default();
    };
    let labels = typed_block_indices_by_label(function);
    let reverse_postorder = typed_block_reverse_postorder(function, &labels, entry);
    if reverse_postorder.is_empty() {
        return TypedBlockDominators::default();
    }
    let reachable = reverse_postorder.iter().copied().collect::<HashSet<_>>();
    let reverse_postorder_indices = reverse_postorder
        .iter()
        .copied()
        .enumerate()
        .map(|(index, label)| (label, index))
        .collect::<HashMap<_, _>>();
    let predecessors = typed_block_predecessors(function);
    let mut immediate_dominators =
        HashMap::<BlockLabel, Option<BlockLabel>>::with_capacity(reverse_postorder.len());
    immediate_dominators.insert(entry, Some(entry));
    for label in reverse_postorder.iter().copied().skip(1) {
        immediate_dominators.insert(label, None);
    }

    let mut changed = true;
    while changed {
        changed = false;
        for label in reverse_postorder.iter().copied().skip(1) {
            let Some(block_predecessors) = predecessors.get(&label) else {
                continue;
            };
            let mut processed_predecessors = block_predecessors.iter().copied().filter(|pred| {
                reachable.contains(pred)
                    && immediate_dominators.get(pred).copied().flatten().is_some()
            });
            let Some(mut new_dominator) = processed_predecessors.next() else {
                continue;
            };
            for predecessor in processed_predecessors {
                new_dominator = intersect_typed_immediate_dominators(
                    predecessor,
                    new_dominator,
                    &immediate_dominators,
                    &reverse_postorder_indices,
                );
            }
            if immediate_dominators.get(&label).copied().flatten() != Some(new_dominator) {
                immediate_dominators.insert(label, Some(new_dominator));
                changed = true;
            }
        }
    }

    let mut children = HashMap::<BlockLabel, Vec<BlockLabel>>::new();
    for label in reverse_postorder.iter().copied().skip(1) {
        if let Some(parent) = immediate_dominators.get(&label).copied().flatten() {
            children.entry(parent).or_default().push(label);
        }
    }

    let mut dominators = TypedBlockDominators::default();
    let mut next_interval = 0usize;
    let mut stack = vec![(entry, false)];
    while let Some((label, exiting)) = stack.pop() {
        if exiting {
            dominators.exit.insert(label, next_interval);
            next_interval += 1;
            continue;
        }
        dominators.enter.insert(label, next_interval);
        next_interval += 1;
        stack.push((label, true));
        if let Some(block_children) = children.get(&label) {
            stack.extend(
                block_children
                    .iter()
                    .rev()
                    .copied()
                    .map(|child| (child, false)),
            );
        }
    }
    dominators
}

fn typed_block_reverse_postorder(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    entry: BlockLabel,
) -> Vec<BlockLabel> {
    let mut seen = HashSet::new();
    let mut postorder = Vec::new();
    let mut stack = vec![(entry, false)];
    while let Some((label, exiting)) = stack.pop() {
        if exiting {
            postorder.push(label);
            continue;
        }
        if !seen.insert(label) {
            continue;
        }
        stack.push((label, true));
        let Some(block) = block_by_label(function, labels, label) else {
            continue;
        };
        stack.extend(
            typed_block_successors(block)
                .into_iter()
                .rev()
                .filter(|successor| labels.contains_key(successor))
                .map(|successor| (successor, false)),
        );
    }
    postorder.reverse();
    postorder
}

fn intersect_typed_immediate_dominators(
    mut left: BlockLabel,
    mut right: BlockLabel,
    immediate_dominators: &HashMap<BlockLabel, Option<BlockLabel>>,
    reverse_postorder_indices: &HashMap<BlockLabel, usize>,
) -> BlockLabel {
    while left != right {
        while reverse_postorder_indices[&left] > reverse_postorder_indices[&right] {
            left = immediate_dominators[&left]
                .expect("processed predecessor should have an immediate dominator");
        }
        while reverse_postorder_indices[&right] > reverse_postorder_indices[&left] {
            right = immediate_dominators[&right]
                .expect("processed predecessor should have an immediate dominator");
        }
    }
    left
}

pub fn rewrite_typed_stop_iteration_raises_to_handler_jumps(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> usize {
    let labels = function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect::<HashMap<_, _>>();
    let rewrite_edges = function
        .blocks
        .iter()
        .filter_map(|block| {
            if !typed_block_term_is_stop_iteration_raise(&block.term, module_constants) {
                return None;
            }
            let dispatch = block.exc_edge.as_ref()?;
            let edge = stop_iteration_handler_jump_edge_for_raise(
                function,
                module_constants,
                &labels,
                block,
                dispatch,
            )?;
            Some((block.label, edge))
        })
        .collect::<HashMap<_, _>>();

    if rewrite_edges.is_empty() {
        return 0;
    }

    let mut rewritten = 0;
    for block in &mut function.blocks {
        let Some(edge) = rewrite_edges.get(&block.label) else {
            continue;
        };
        block.term = BlockTerm::Jump(edge.clone());
        rewritten += 1;
    }
    prune_unreachable_typed_blocks(function);
    rewritten
}

fn typed_block_term_is_stop_iteration_raise(
    term: &BlockTerm<InstrTyped>,
    module_constants: &[ConstantExpr],
) -> bool {
    let BlockTerm::Raise(raise) = term else {
        return false;
    };
    let Some(exc) = raise.exc.as_ref() else {
        return false;
    };
    typed_expr_is_runtime_name_load(exc, RuntimeName::StopIteration, module_constants)
        || typed_expr_is_zero_arg_runtime_call(exc, RuntimeName::StopIteration, module_constants)
}

fn typed_expr_is_zero_arg_runtime_call(
    expr: &InstrTyped,
    runtime_name: RuntimeName,
    module_constants: &[ConstantExpr],
) -> bool {
    let Some((func, args, keywords)) = typed_callable_call_parts(expr) else {
        return false;
    };
    args.is_empty()
        && keywords.is_empty()
        && typed_expr_is_runtime_name_load(func, runtime_name, module_constants)
}

fn stop_iteration_handler_jump_edge_for_raise(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    labels: &HashMap<BlockLabel, usize>,
    raise_block: &TypedBlock,
    dispatch: &BlockEdge,
) -> Option<BlockEdge> {
    if !dispatch
        .args
        .iter()
        .any(|arg| matches!(arg, BlockArg::CurrentException))
    {
        return None;
    }
    let dispatch_block = block_by_label(function, labels, dispatch.target)?;
    let exception_name = dispatch_block.exception_param()?;
    let handler_label =
        stop_iteration_match_handler_label(&dispatch_block.term, exception_name, module_constants)?;
    let handler_block = block_by_label(function, labels, handler_label)?;
    if handler_region_uses_exception_value(function, labels, handler_label, exception_name) {
        return None;
    }
    let args = direct_handler_jump_args(raise_block, handler_block, exception_name)?;
    Some(BlockEdge::with_args(handler_label, args))
}

fn block_by_label<'a>(
    function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    label: BlockLabel,
) -> Option<&'a TypedBlock> {
    labels
        .get(&label)
        .and_then(|index| function.blocks.get(*index))
}

fn stop_iteration_match_handler_label(
    term: &BlockTerm<InstrTyped>,
    exception_name: &str,
    module_constants: &[ConstantExpr],
) -> Option<BlockLabel> {
    let BlockTerm::IfTerm(if_term) = term else {
        return None;
    };
    typed_expr_is_exception_matches_stop_iteration(&if_term.test, exception_name, module_constants)
        .then_some(if_term.then_label)
}

fn typed_expr_is_exception_matches_stop_iteration(
    expr: &InstrTyped,
    exception_name: &str,
    module_constants: &[ConstantExpr],
) -> bool {
    if let InstrTyped::Truthy(op) = expr {
        return typed_expr_is_exception_matches_stop_iteration(
            op.value(),
            exception_name,
            module_constants,
        );
    }
    let Some((func, args, keywords)) = typed_callable_call_parts(expr) else {
        return false;
    };
    if !typed_expr_is_runtime_name_load(func, RuntimeName::ExceptionMatches, module_constants)
        || !keywords.is_empty()
        || args.len() != 2
    {
        return false;
    }
    let Some(exc) = typed_positional_arg_expr(args.first()) else {
        return false;
    };
    let Some(expected) = typed_positional_arg_expr(args.get(1)) else {
        return false;
    };
    typed_expr_loads_name(exc, exception_name)
        && typed_expr_is_runtime_name_load(expected, RuntimeName::StopIteration, module_constants)
}

fn typed_callable_call_parts<'a>(
    expr: &'a InstrTyped,
) -> Option<(
    &'a InstrTyped,
    &'a [CallArgPositional<InstrTyped>],
    &'a [CallArgKeyword<InstrTyped>],
)> {
    match expr {
        InstrTyped::CallTyped(call) => Some((call.func.as_ref(), &call.args, &call.keywords)),
        InstrTyped::GuardedCallableCallTyped(call) => {
            Some((call.func.as_ref(), &call.args, &call.keywords))
        }
        InstrTyped::DirectCallableCallTyped(call) => Some((call.func.as_ref(), &call.args, &[])),
        _ => None,
    }
}

fn typed_positional_arg_expr(arg: Option<&CallArgPositional<InstrTyped>>) -> Option<&InstrTyped> {
    match arg? {
        CallArgPositional::Positional(expr) => Some(expr),
        CallArgPositional::Starred(_) => None,
    }
}

fn typed_expr_is_runtime_name_load(
    expr: &InstrTyped,
    runtime_name: RuntimeName,
    module_constants: &[ConstantExpr],
) -> bool {
    let InstrTyped::Load(load) = expr else {
        return false;
    };
    if load.name.runtime_name_id() == Some(runtime_name) {
        return true;
    }
    let Some(index) = load.name.location.as_constant() else {
        return false;
    };
    matches!(
        module_constants.get(index as usize),
        Some(ConstantExpr::RuntimeName(name)) if *name == runtime_name
    )
}

fn typed_expr_loads_name(expr: &InstrTyped, name: &str) -> bool {
    let InstrTyped::Load(load) = expr else {
        return false;
    };
    load.name.id_str() == name
}

fn handler_region_uses_exception_value(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    entry: BlockLabel,
    exception_name: &str,
) -> bool {
    let mut pending = vec![entry];
    let mut seen = HashSet::new();
    while let Some(label) = pending.pop() {
        if !seen.insert(label) {
            continue;
        }
        let Some(block) = block_by_label(function, labels, label) else {
            continue;
        };
        if block.exception_param() != Some(exception_name) {
            continue;
        }
        if typed_block_uses_name(block, exception_name) {
            return true;
        }
        pending.extend(typed_term_successors(&block.term).into_iter());
    }
    false
}

fn typed_block_uses_name(block: &TypedBlock, name: &str) -> bool {
    let mut finder = TypedNameUseFinder { name, found: false };
    for instr in &block.body {
        finder.visit_instr(instr);
        if finder.found {
            return true;
        }
    }
    finder.visit_term(&block.term);
    finder.found
}

struct TypedNameUseFinder<'a> {
    name: &'a str,
    found: bool,
}

impl Visit<InstrTyped> for TypedNameUseFinder<'_> {
    fn visit_instr(&mut self, expr: &InstrTyped) {
        if self.found {
            return;
        }
        if typed_expr_loads_name(expr, self.name) {
            self.found = true;
            return;
        }
        expr.visit_children(self);
    }
}

fn typed_term_successors(term: &BlockTerm<InstrTyped>) -> Vec<BlockLabel> {
    match term {
        BlockTerm::Jump(edge) => vec![edge.target],
        BlockTerm::IfTerm(if_term) => vec![if_term.then_label, if_term.else_label],
        BlockTerm::BranchTable(branch) => {
            let mut labels = branch.targets.clone();
            labels.push(branch.default_label);
            labels
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => Vec::new(),
    }
}

fn direct_handler_jump_args(
    source_block: &TypedBlock,
    target_block: &TypedBlock,
    exception_name: &str,
) -> Option<Vec<BlockArg>> {
    let source_params = source_block.param_name_vec();
    let source_has_owner = source_params
        .iter()
        .any(|param| param == "_dp_self" || param == "_dp_state");
    target_block
        .params
        .iter()
        .map(|param| {
            if param.role == BlockParamRole::Exception || param.name == exception_name {
                Some(BlockArg::None)
            } else if source_params.iter().any(|source| source == &param.name) || source_has_owner {
                Some(BlockArg::Name(param.name.clone()))
            } else {
                None
            }
        })
        .collect()
}

pub fn validate_typed_function_call_access_plans(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> Result<(), String> {
    struct Validator {
        function_id: RuntimeFunctionId,
        error: Option<String>,
    }

    impl Visit<InstrTyped> for Validator {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.error.is_some() {
                return;
            }
            if let InstrTyped::CallTyped(call) = expr {
                if let Err(err) = validate_typed_call_access_plan(call) {
                    self.error = Some(format!(
                        "invalid typed call access plan in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            if let InstrTyped::GuardedCallableCallTyped(op) = expr {
                let call = op.clone().into_typed_call();
                if let Err(err) = validate_typed_call_access_plan(&call) {
                    self.error = Some(format!(
                        "invalid typed call access plan in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            if let InstrTyped::GuardedMethodCallTyped(op) = expr {
                let call = op.clone().into_typed_call();
                if let Err(err) = validate_typed_call_access_plan(&call) {
                    self.error = Some(format!(
                        "invalid typed call access plan in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            if let InstrTyped::DirectCallableCallTyped(op) = expr {
                if let Err(err) = validate_typed_direct_callable_call(op) {
                    self.error = Some(format!(
                        "invalid typed direct callable call in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            if let InstrTyped::DirectMethodCallTyped(op) = expr {
                if let Err(err) = validate_typed_direct_method_call(op) {
                    self.error = Some(format!(
                        "invalid typed direct method call in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            expr.visit_children(self);
        }
    }

    let mut validator = Validator {
        function_id: function.function_id,
        error: None,
    };
    for block in &function.blocks {
        for instr in &block.body {
            validator.visit_instr(instr);
            if let Some(err) = validator.error.take() {
                return Err(err);
            }
        }
        validator.visit_term(&block.term);
        if let Some(err) = validator.error.take() {
            return Err(err);
        }
    }
    Ok(())
}

pub fn validate_typed_module_call_access_plans(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
) -> Result<(), String> {
    for function in &module.callable_defs {
        validate_typed_function_call_access_plans(function)?;
    }
    Ok(())
}

fn validate_typed_call_access_plan(call: &TypedCall<InstrTyped>) -> Result<(), String> {
    match &call.access {
        TypedCallAccessPlan::Generic => Ok(()),
        TypedCallAccessPlan::GuardedCallable { function_guards } => {
            validate_typed_call_simple_shape(call)?;
            for guard in function_guards {
                validate_typed_callable_direct_call_arg_plan(call, &guard.arg_plan)?;
            }
            Ok(())
        }
        TypedCallAccessPlan::GuardedMethod {
            method_name,
            method_guards,
        } => {
            validate_typed_call_simple_shape(call)?;
            if method_name.is_empty() {
                return Err("guarded method call requires a non-empty method name".to_string());
            }
            if !matches!(call.func.as_ref(), InstrTyped::GetAttrTyped(_)) {
                return Err("guarded method call requires a GetAttr call target".to_string());
            }
            for guard in method_guards {
                validate_typed_direct_call_arg_plan(call, &guard.arg_plan, 1)?;
            }
            Ok(())
        }
        TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
            runtime_name,
            method_name,
            method_guards,
        } => {
            validate_typed_call_simple_shape(call)?;
            let expected_method = match runtime_name {
                RuntimeName::Iter => "__iter__",
                RuntimeName::Next => "__next__",
                _ => {
                    return Err(format!(
                        "guarded runtime protocol call does not support runtime name {runtime_name:?}"
                    ));
                }
            };
            if method_name != expected_method {
                return Err(format!(
                    "guarded {runtime_name:?} protocol call requires method {expected_method}, got {method_name}"
                ));
            }
            if method_name.is_empty() {
                return Err(
                    "guarded runtime protocol call requires a non-empty method name".to_string(),
                );
            }
            let explicit_positional_arg_count =
                validate_typed_direct_call_positional_args(call.args.as_slice())?;
            if explicit_positional_arg_count != 1 {
                return Err(format!(
                    "guarded {runtime_name:?} protocol call requires exactly one receiver arg, got {explicit_positional_arg_count}"
                ));
            }
            for guard in method_guards {
                validate_typed_direct_call_arg_sources(&guard.arg_plan, 1)?;
            }
            Ok(())
        }
    }
}

fn validate_typed_call_simple_shape(call: &TypedCall<InstrTyped>) -> Result<(), String> {
    validate_typed_direct_call_positional_args(call.args.as_slice())?;
    if !call.keywords.is_empty() {
        return Err("guarded direct call plans do not support keyword args".to_string());
    }
    Ok(())
}

fn validate_typed_direct_callable_call(
    call: &TypedDirectCallableCall<InstrTyped>,
) -> Result<(), String> {
    let explicit_positional_arg_count =
        validate_typed_direct_call_positional_args(call.args.as_slice())?;
    match &call.guard {
        TypedDirectCallableCallGuard::Function(guard) => {
            validate_typed_direct_call_arg_sources(&guard.arg_plan, explicit_positional_arg_count)
                .or_else(|_| {
                    validate_typed_direct_call_arg_sources(
                        &guard.arg_plan,
                        explicit_positional_arg_count + 1,
                    )
                })
        }
    }
}

fn validate_typed_direct_method_call(
    call: &TypedDirectMethodCall<InstrTyped>,
) -> Result<(), String> {
    if call.method_name.is_empty() {
        return Err("typed direct method call requires a non-empty method name".to_string());
    }
    let explicit_positional_arg_count =
        validate_typed_direct_call_positional_args(call.args.as_slice())?;
    validate_typed_direct_call_arg_sources(&call.guard.arg_plan, explicit_positional_arg_count + 1)
}

fn validate_typed_direct_call_positional_args(
    args: &[CallArgPositional<InstrTyped>],
) -> Result<usize, String> {
    let mut explicit_positional_arg_count = 0;
    for arg in args {
        match arg {
            CallArgPositional::Positional(_) => explicit_positional_arg_count += 1,
            CallArgPositional::Starred(_) => {
                return Err("guarded direct call plans do not support starred args".to_string());
            }
        }
    }
    Ok(explicit_positional_arg_count)
}

fn validate_typed_direct_call_arg_plan(
    call: &TypedCall<InstrTyped>,
    plan: &TypedDirectCallArgPlan,
    implicit_positional_arg_count: usize,
) -> Result<(), String> {
    let explicit_positional_arg_count =
        validate_typed_direct_call_positional_args(call.args.as_slice())?;
    let provided_positional_arg_count =
        implicit_positional_arg_count + explicit_positional_arg_count;
    validate_typed_direct_call_arg_sources(plan, provided_positional_arg_count)
}

fn validate_typed_callable_direct_call_arg_plan(
    call: &TypedCall<InstrTyped>,
    plan: &TypedDirectCallArgPlan,
) -> Result<(), String> {
    match validate_typed_direct_call_arg_plan(call, plan, 0) {
        Ok(()) => Ok(()),
        Err(err) => {
            // Synthetic constructor entries are ordinary callable direct-call
            // targets, but their first argument is the type object being called.
            // Plan construction has the target metadata needed to decide when
            // that implicit argument is legal; typed-plan validation only sees
            // the already-lowered guard.
            validate_typed_direct_call_arg_plan(call, plan, 1).map_err(|_| err)
        }
    }
}

fn validate_typed_direct_call_arg_sources(
    plan: &TypedDirectCallArgPlan,
    provided_positional_arg_count: usize,
) -> Result<(), String> {
    let mut saw_packed_rest = false;
    for source in &plan.sources {
        match source {
            TypedDirectCallArgSource::Provided(index)
                if saw_packed_rest || *index >= provided_positional_arg_count =>
            {
                if saw_packed_rest {
                    return Err(
                        "direct call arg plan references provided arg after packed rest"
                            .to_string(),
                    );
                }
                return Err(format!(
                    "direct call arg plan references provided arg {index}, but only {provided_positional_arg_count} args are available"
                ));
            }
            TypedDirectCallArgSource::PackedRest { start } => {
                if saw_packed_rest {
                    return Err(
                        "direct call arg plan packs provided rest more than once".to_string()
                    );
                }
                if *start > provided_positional_arg_count {
                    return Err(format!(
                        "direct call arg plan packs provided rest from arg {start}, but only {provided_positional_arg_count} args are available"
                    ));
                }
                saw_packed_rest = true;
            }
            TypedDirectCallArgSource::Provided(_) | TypedDirectCallArgSource::DefaultSentinel => {}
        }
    }
    Ok(())
}

struct TypedToBlockPy;

impl TryMapInstr<InstrTyped, InstrBlockPy, String> for TypedToBlockPy {
    fn try_map_instr(&mut self, instr: InstrTyped) -> Result<InstrBlockPy, String> {
        Ok(match instr {
            InstrTyped::Truthy(_) => {
                return Err(
                    "typed truthiness instruction requires typed codegen emission".to_string(),
                );
            }
            InstrTyped::Load(op) => InstrBlockPy::Load(op.try_map_children(self)?),
            InstrTyped::BinOp(op) => InstrBlockPy::BinOp(op.try_map_children(self)?),
            InstrTyped::Tuple(op) => InstrBlockPy::Tuple(op.try_map_children(self)?),
            InstrTyped::UnaryOp(op) => InstrBlockPy::UnaryOp(op.try_map_children(self)?),
            InstrTyped::CalleeFunctionId(_) => {
                return Err("typed callee function id requires typed codegen emission".to_string());
            }
            InstrTyped::DirectCallGuardTest(op) => match op.kind {
                TypedDirectCallGuardTestKind::RuntimeFunctionId { .. }
                | TypedDirectCallGuardTestKind::ExactTypeVersion { .. } => {
                    return Err(
                        "typed direct-call guard requires typed codegen emission".to_string()
                    );
                }
            },
            InstrTyped::CallTyped(op) => {
                InstrBlockPy::Call(op.try_map_children(self)?.into_legacy())
            }
            InstrTyped::GuardedCallableCallTyped(_) => {
                return Err(
                    "typed guarded callable call requires typed codegen emission".to_string(),
                );
            }
            InstrTyped::GuardedMethodCallTyped(_) => {
                return Err("typed guarded method call requires typed codegen emission".to_string());
            }
            InstrTyped::DirectCallableCallTyped(_) => {
                return Err(
                    "typed direct callable call requires typed codegen emission".to_string()
                );
            }
            InstrTyped::DirectMethodCallTyped(_) => {
                return Err("typed direct method call requires typed codegen emission".to_string());
            }
            InstrTyped::CallDirect(_) => {
                return Err("typed direct call requires typed codegen emission".to_string());
            }
            InstrTyped::GetAttrTyped(op) => {
                InstrBlockPy::GetAttr(op.try_map_children(self)?.into_legacy())
            }
            InstrTyped::SetAttrTyped(op) => {
                InstrBlockPy::SetAttr(op.try_map_children(self)?.into_legacy())
            }
            InstrTyped::GetItem(op) => InstrBlockPy::GetItem(op.try_map_children(self)?),
            InstrTyped::SetItem(op) => InstrBlockPy::SetItem(op.try_map_children(self)?),
            InstrTyped::DelItem(op) => InstrBlockPy::DelItem(op.try_map_children(self)?),
            InstrTyped::Store(op) => InstrBlockPy::Store(op.try_map_children(self)?),
            InstrTyped::Del(op) => InstrBlockPy::Del(op.try_map_children(self)?),
            InstrTyped::MakeCell(op) => InstrBlockPy::MakeCell(op.try_map_children(self)?),
            InstrTyped::IncrementCounter(op) => InstrBlockPy::IncrementCounter(op),
            InstrTyped::CellRef(op) => InstrBlockPy::CellRef(op),
            InstrTyped::MakeFunctionWithClosure(op) => {
                InstrBlockPy::MakeFunctionWithClosure(op.try_map_children(self)?)
            }
        })
    }

    fn try_map_name(&mut self, name: ResolvedName) -> Result<ResolvedName, String> {
        Ok(name)
    }
}

#[track_caller]
pub fn try_lower_typed_instr_to_codegen_legacy(instr: InstrTyped) -> Result<InstrBlockPy, String> {
    let caller = std::panic::Location::caller();
    TypedToBlockPy.try_map_instr(instr).map_err(|err| {
        format!(
            "{err} [typed_to_codegen_legacy caller={}:{}]",
            caller.file(),
            caller.line()
        )
    })
}

#[track_caller]
pub fn try_lower_typed_term_to_codegen_legacy(
    term: BlockTerm<InstrTyped>,
) -> Result<BlockTerm<InstrBlockPy>, String> {
    let caller = std::panic::Location::caller();
    TypedToBlockPy.try_map_term(term).map_err(|err| {
        format!(
            "{err} [typed_to_codegen_legacy caller={}:{}]",
            caller.file(),
            caller.line()
        )
    })
}

pub fn try_lower_typed_module_to_codegen_legacy(
    module: BlockPyModule<TypedBlockPyModuleShape>,
) -> Result<BlockPyModule<BlockPyModuleShape>, String> {
    validate_typed_module_call_access_plans(&module)?;
    TypedToBlockPy.try_map_module(module)
}

#[cfg(test)]
mod typed_codegen_tests {
    use super::*;
    use crate::passes::{infer_module_value_facts, plan_module_inlining, summarize_module_escapes};
    use soac_core::block_py::{
        ChildVisitable, InstrId, InstrWithConstantNone, ModuleNameGen, Visit, VisitMut,
    };
    use soac_core::pass_tracker::NoopPassTracker;
    use soac_ir_blockpy::constructor_entry_function_id_for_init;
    use soac_ir_typed::{
        TypedAttrOwnerRef, TypedDirectFunctionCallGuard, TypedDirectMethodCallGuard,
        TypedIndexedFieldGuard, TypedIndexedFieldPlanSource, lower_blockpy_module_to_typed,
    };
    use std::collections::{HashMap, HashSet};

    #[derive(Default)]
    struct TypedInstrCounter {
        total: usize,
        truthy: usize,
        loads: usize,
        binops: usize,
        tuples: usize,
        unary_ops: usize,
        typed_calls: usize,
        guarded_callable_calls: usize,
        guarded_method_calls: usize,
        direct_callable_calls: usize,
        direct_method_calls: usize,
        direct_call_guard_tests: usize,
        typed_getattrs: usize,
        typed_setattrs: usize,
        getitems: usize,
        setitems: usize,
        delitems: usize,
        stores: usize,
        dels: usize,
        make_cells: usize,
        increment_counters: usize,
        cell_refs: usize,
        make_functions_with_closure: usize,
        first_class: usize,
    }

    impl Visit<InstrTyped> for TypedInstrCounter {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            self.total += 1;
            if matches!(expr, InstrTyped::Truthy(_)) {
                self.truthy += 1;
            }
            if matches!(expr, InstrTyped::Load(_)) {
                self.loads += 1;
            }
            self.first_class += 1;
            if matches!(expr, InstrTyped::BinOp(_)) {
                self.binops += 1;
            }
            if matches!(expr, InstrTyped::Tuple(_)) {
                self.tuples += 1;
            }
            if matches!(expr, InstrTyped::UnaryOp(_)) {
                self.unary_ops += 1;
            }
            if matches!(expr, InstrTyped::CallTyped(_)) {
                self.typed_calls += 1;
            }
            if matches!(expr, InstrTyped::GuardedCallableCallTyped(_)) {
                self.guarded_callable_calls += 1;
            }
            if matches!(expr, InstrTyped::GuardedMethodCallTyped(_)) {
                self.guarded_method_calls += 1;
            }
            if matches!(expr, InstrTyped::DirectCallableCallTyped(_)) {
                self.direct_callable_calls += 1;
            }
            if matches!(expr, InstrTyped::DirectMethodCallTyped(_)) {
                self.direct_method_calls += 1;
            }
            if matches!(expr, InstrTyped::DirectCallGuardTest(_)) {
                self.direct_call_guard_tests += 1;
            }
            if matches!(expr, InstrTyped::GetAttrTyped(_)) {
                self.typed_getattrs += 1;
            }
            if matches!(expr, InstrTyped::SetAttrTyped(_)) {
                self.typed_setattrs += 1;
            }
            if matches!(expr, InstrTyped::GetItem(_)) {
                self.getitems += 1;
            }
            if matches!(expr, InstrTyped::SetItem(_)) {
                self.setitems += 1;
            }
            if matches!(expr, InstrTyped::DelItem(_)) {
                self.delitems += 1;
            }
            if matches!(expr, InstrTyped::Store(_)) {
                self.stores += 1;
            }
            if matches!(expr, InstrTyped::Del(_)) {
                self.dels += 1;
            }
            if matches!(expr, InstrTyped::MakeCell(_)) {
                self.make_cells += 1;
            }
            if matches!(expr, InstrTyped::IncrementCounter(_)) {
                self.increment_counters += 1;
            }
            if matches!(expr, InstrTyped::CellRef(_)) {
                self.cell_refs += 1;
            }
            if matches!(expr, InstrTyped::MakeFunctionWithClosure(_)) {
                self.make_functions_with_closure += 1;
            }
            expr.visit_children(self);
        }
    }

    fn first_class_count(counter: &TypedInstrCounter) -> usize {
        counter.truthy
            + counter.loads
            + counter.binops
            + counter.tuples
            + counter.unary_ops
            + counter.typed_calls
            + counter.guarded_callable_calls
            + counter.guarded_method_calls
            + counter.direct_callable_calls
            + counter.direct_method_calls
            + counter.direct_call_guard_tests
            + counter.typed_getattrs
            + counter.typed_setattrs
            + counter.getitems
            + counter.setitems
            + counter.delitems
            + counter.stores
            + counter.dels
            + counter.make_cells
            + counter.increment_counters
            + counter.cell_refs
            + counter.make_functions_with_closure
    }

    #[derive(Default, Eq, PartialEq, Debug)]
    struct BlockPyInstrCounter {
        total: usize,
        binops: usize,
        calls: usize,
    }

    impl Visit<InstrBlockPy> for BlockPyInstrCounter {
        fn visit_instr(&mut self, expr: &InstrBlockPy) {
            self.total += 1;
            match expr {
                InstrBlockPy::BinOp(_) => self.binops += 1,
                InstrBlockPy::Call(_) => self.calls += 1,
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    fn count_blockpy_instrs(module: &BlockPyModule<BlockPyModuleShape>) -> BlockPyInstrCounter {
        let mut counter = BlockPyInstrCounter::default();
        for function in &module.callable_defs {
            for block in &function.blocks {
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }
        counter
    }

    #[derive(Default)]
    struct TypedExtraFactCounter {
        extras: usize,
        facts: usize,
        none_singletons: usize,
        bools: usize,
    }

    impl Visit<InstrTyped> for TypedExtraFactCounter {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(extra) = expr.typed_extra() {
                self.extras += 1;
                if let Some(facts) = extra.result_facts() {
                    self.facts += 1;
                    if facts.as_pyobj().is_some_and(PyObjFacts::is_none) {
                        self.none_singletons += 1;
                    }
                    if matches!(facts, ValueFacts::Bool(_)) {
                        self.bools += 1;
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    fn count_typed_extra_facts(
        module: &BlockPyModule<TypedBlockPyModuleShape>,
    ) -> TypedExtraFactCounter {
        let mut counter = TypedExtraFactCounter::default();
        for function in &module.callable_defs {
            counter.visit_fn(function);
        }
        counter
    }

    fn typed_function_by_qualname_mut<'a>(
        module: &'a mut BlockPyModule<TypedBlockPyModuleShape>,
        qualname: &str,
    ) -> &'a mut BlockPyFunction<TypedBlockPyModuleShape> {
        module
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing typed function {qualname}"))
    }

    fn blockpy_function_id_by_qualname(
        module: &BlockPyModule<BlockPyModuleShape>,
        qualname: &str,
    ) -> RuntimeFunctionId {
        module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing codegen function {qualname}"))
            .function_id
    }

    fn lower_test_module_with_id(
        source: &str,
        module_id: u32,
    ) -> BlockPyModule<BlockPyModuleShape> {
        soac_lowering::lower_python_to_blockpy_with_tracker_and_options(
            source,
            ModuleNameGen::new(module_id),
            NoopPassTracker::new(),
            soac_lowering::LoweringOptions::default(),
        )
        .expect("source should lower")
        .blockpy_module
    }

    fn replace_first_typed_call_access(
        function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
        access: TypedCallAccessPlan,
    ) {
        struct Replacer {
            access: Option<TypedCallAccessPlan>,
        }

        impl VisitMut<InstrTyped> for Replacer {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if let Some(access) = self.access.take() {
                    if let InstrTyped::CallTyped(call) = expr {
                        call.access = access;
                        return;
                    }
                    self.access = Some(access);
                }
                expr.visit_children_mut(self);
            }
        }

        let mut replacer = Replacer {
            access: Some(access),
        };
        replacer.visit_fn_mut(function);
        assert!(
            replacer.access.is_none(),
            "test function should contain a typed call"
        );
    }

    fn replace_first_typed_call_access_where(
        function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
        access: TypedCallAccessPlan,
        mut predicate: impl FnMut(&TypedCall<InstrTyped>) -> bool,
    ) -> InstrId {
        struct Replacer<'a, P> {
            access: Option<TypedCallAccessPlan>,
            instr_id: Option<InstrId>,
            predicate: &'a mut P,
        }

        impl<P> VisitMut<InstrTyped> for Replacer<'_, P>
        where
            P: FnMut(&TypedCall<InstrTyped>) -> bool,
        {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if let Some(access) = self.access.take() {
                    if let InstrTyped::CallTyped(call) = expr
                        && (self.predicate)(call)
                    {
                        self.instr_id = call.try_semantic_instr_id();
                        call.access = access;
                        return;
                    }
                    self.access = Some(access);
                }
                expr.visit_children_mut(self);
            }
        }

        let mut replacer = Replacer {
            access: Some(access),
            instr_id: None,
            predicate: &mut predicate,
        };
        replacer.visit_fn_mut(function);
        assert!(
            replacer.access.is_none(),
            "test function should contain a matching typed call"
        );
        replacer
            .instr_id
            .expect("matching typed call should have an instruction id")
    }

    fn typed_call_func_is_getattr(call: &TypedCall<InstrTyped>) -> bool {
        matches!(call.func.as_ref(), InstrTyped::GetAttrTyped(_))
    }

    fn inline_next_protocol_call(source: &str) -> BlockPyModule<TypedBlockPyModuleShape> {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                    runtime_name: RuntimeName::Next,
                    method_name: "__next__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: next_id,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "__main__".to_string(),
                            qualname: "IterRange".to_string(),
                        },
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
                |call| call.args.len() == 1 && call.keywords.is_empty(),
            );
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    next_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );
        assert_eq!(stats.rewritten_stores, 1);
        typed
    }

    #[test]
    fn inlines_generator_resume_calls_with_isolated_preserved_resume_abi_storage() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    yield limit\n\n\
def helper(fn, owner, state, value, exc):\n    return value\n\n\
def caller(fn, owner, state):\n    return helper(fn, owner, state, None, None)\n",
        )
        .expect("source should lower");
        let values_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "values");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        struct Marker {
            call_id: InstrId,
            function_id: RuntimeFunctionId,
        }
        impl VisitMut<InstrTyped> for Marker {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr
                    && call.try_semantic_instr_id() == Some(self.call_id)
                {
                    call.extra
                        .set_generator_resume_plan(soac_ir_typed::TypedGeneratorResumePlan {
                            function_id: self.function_id,
                            generator_origin: Some(InstrId::new(99)),
                            candidate_origins: vec![InstrId::new(99)],
                        });
                    return;
                }
                expr.visit_children_mut(self);
            }
        }
        Marker {
            call_id,
            function_id: values_id,
        }
        .visit_fn_mut(caller);

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(stats.rewritten_returns, 1);
        let layout = caller
            .storage_layout
            .as_ref()
            .expect("inlined caller should keep a storage layout");
        assert!(
            layout
                .stack_slots()
                .iter()
                .filter(|name| name.starts_with("_dp_typed_inline_preserved_abi_"))
                .count()
                >= 2,
            "resume inlining should allocate isolated preserved-owner scratch locals"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(
                    instr,
                    InstrTyped::Store(store)
                        if store.name.id_str().starts_with("_dp_typed_inline_preserved_abi_")
                )),
            "resume inlining should seed the preserved ABI scratch locals before the inlined body"
        );
    }

    #[test]
    fn generator_resume_inline_bindings_allocate_distinct_preserved_abi_scratch_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    yield limit\n\n\
def caller():\n    return None\n",
        )
        .expect("source should lower");
        let values_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "values");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let callee_module = typed.clone();
        let callee = callee_module
            .callable_defs
            .iter()
            .find(|function| function.function_id == values_id)
            .expect("generator resume body should exist");
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let arg_plan = TypedDirectCallArgPlan {
            sources: (1..=callee.body_params().len())
                .map(TypedDirectCallArgSource::Provided)
                .collect(),
        };
        let values = vec![InstrTyped::constant_none(); callee.body_params().len() + 1];

        let (_, _, _, first) =
            bind_typed_generator_resume_inline_values(caller, callee, &arg_plan, values.as_slice())
                .expect("first resume binding should succeed");
        let (_, _, _, second) =
            bind_typed_generator_resume_inline_values(caller, callee, &arg_plan, values.as_slice())
                .expect("second resume binding should succeed");

        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);
        let locations = first
            .iter()
            .chain(second.iter())
            .map(|local| local.location)
            .collect::<HashSet<_>>();
        assert_eq!(
            locations.len(),
            first.len() + second.len(),
            "each inlined resume should get distinct owner/state scratch locals"
        );
    }

    #[test]
    fn inlines_builtin_implementation_calls_without_callable_guards() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def helper(value):\n    return value\n\n\
def caller(value):\n    return list(value)\n",
        )
        .expect("source should lower");
        let helper_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "helper");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        struct Marker {
            call_id: InstrId,
            function_id: RuntimeFunctionId,
        }
        impl VisitMut<InstrTyped> for Marker {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr
                    && call.try_semantic_instr_id() == Some(self.call_id)
                {
                    call.extra.set_builtin_implementation_plan(
                        soac_ir_typed::TypedBuiltinImplementationPlan {
                            source: RuntimeName::List,
                            function_id: self.function_id,
                            arg_plan: TypedDirectCallArgPlan {
                                sources: vec![TypedDirectCallArgSource::Provided(0)],
                            },
                        },
                    );
                    return;
                }
                expr.visit_children_mut(self);
            }
        }
        Marker {
            call_id,
            function_id: helper_id,
        }
        .visit_fn_mut(caller);

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(stats.rewritten_returns, 1);
        assert!(
            caller.blocks.iter().all(|block| {
                !matches!(
                    &block.term,
                    BlockTerm::Return(InstrTyped::CallTyped(call))
                        if call.try_semantic_instr_id() == Some(call_id)
                )
            }),
            "builtin implementation inlining should replace the original call"
        );
    }

    #[test]
    fn generator_resume_capture_bindings_follow_preserved_constructor_functions() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def outer(rows):\n    offset = len(rows)\n    for row in rows:\n        yield set(item + offset for item in row)\n",
        )
        .expect("source should lower");
        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let outer = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "outer")
            .expect("outer should lower");
        let (preserved_location, callee_function_id) = outer
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let preserved_location = store.name.preserved_location()?;
                let InstrTyped::MakeFunctionWithClosure(make_function) = store.value.as_ref()
                else {
                    return None;
                };
                (make_function.kind == soac_core::block_py::FunctionKind::Generator)
                    .then_some((preserved_location, make_function.function_id()))
            })
            .expect("outer should preserve its nested generator constructor function");
        let callee = typed
            .callable_defs
            .iter()
            .find(|function| function.function_id == callee_function_id)
            .expect("nested generator resume body should exist");
        let expected_capture_count = callee
            .storage_layout
            .as_ref()
            .expect("nested generator should have storage layout")
            .freevars
            .len();
        assert_ne!(
            expected_capture_count, 0,
            "the regression should cover closure-backed nested generators"
        );
        let constructor_func: InstrTyped = Load::new(ResolvedName {
            id: "_dp_nested_generator_ctor".into(),
            location: NameLocation::preserved(preserved_location.0),
        })
        .into();
        let bindings = typed_inline_capture_cell_bindings_for_generator_constructor(
            outer,
            &constructor_func,
            callee_function_id,
            expected_capture_count,
        )
        .expect("preserved constructor function loads should recover closure captures");
        assert_eq!(bindings.len(), expected_capture_count);
    }

    #[test]
    fn normalized_inline_capture_loads_resolve_to_owned_cells() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def holder(value):\n    def inner():\n        return value\n    return inner\n",
        )
        .expect("source should lower");
        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let holder = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "holder")
            .expect("holder should lower");
        let caller_cellvar = holder
            .storage_layout
            .as_ref()
            .and_then(|layout| layout.cellvars.first())
            .expect("holder should own the nested closure cell");
        let normalized_capture: InstrTyped = Load::new(ResolvedName {
            id: caller_cellvar.storage_name.clone().into(),
            location: NameLocation::local(0),
        })
        .into();
        assert_eq!(
            typed_inline_capture_cell_location(holder, &normalized_capture),
            Some(CellLocation::Owned(0)),
            "post-remap local cell holders should still resolve to caller-owned cells"
        );
    }

    #[test]
    fn lowers_inlined_generator_preserved_state_to_caller_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    yield limit\n\n\
def caller(limit):\n    gen = values(limit)\n    return None\n",
        )
        .expect("source should lower");
        let values_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "values");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let (origin, generator_name) = caller
            .blocks
            .iter_mut()
            .flat_map(|block| block.body.iter_mut())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                    return None;
                };
                let origin = call.try_semantic_instr_id()?;
                call.extra
                    .set_generator_instance_plan(soac_ir_typed::TypedGeneratorInstancePlan {
                        function_id: values_id,
                        kind: soac_core::block_py::FunctionKind::Generator,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    });
                Some((origin, store.name.clone()))
            })
            .expect("caller should construct the generator");
        let remapped_instr_id = InstrId::new(900);
        let state_location = {
            let layout = caller
                .storage_layout
                .as_mut()
                .expect("caller should have storage");
            let location = LocalLocation(
                u32::try_from(layout.stack_slots().len())
                    .expect("test state slot index should fit in u32"),
            );
            layout.ensure_stack_slot("_dp_state");
            location
        };
        let entry = caller
            .blocks
            .first_mut()
            .expect("caller should have an entry block");
        entry.body.push(typed_store_temp(
            ResolvedName {
                id: "_dp_state".into(),
                location: NameLocation::Local(state_location),
            },
            typed_load_temp(&generator_name),
        ));
        entry.body.push(
            Load::new(ResolvedName {
                id: "limit".into(),
                location: NameLocation::preserved(0),
            })
            .with_meta(Meta {
                instr_id: Some(remapped_instr_id),
                ..Meta::synthetic()
            })
            .into(),
        );

        let callee_module = typed.clone();
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let stats = {
            let (module_constants, callable_defs) =
                (&mut typed.module_constants, &mut typed.callable_defs);
            lower_typed_generator_state_to_locals_with_plan(
                &mut callable_defs[caller_index],
                module_constants,
                &callee_module,
                &HashMap::new(),
                &[TypedGeneratorStateLoweringPlan {
                    generator_origin: origin,
                    function_id: values_id,
                    body_instr_ids: HashSet::from([remapped_instr_id]),
                    pending_alias_use_instr_ids: HashSet::new(),
                    alias_cleanup_active_blocks: None,
                    materialized_constructor: None,
                }],
            )
        };
        let caller = &typed.callable_defs[caller_index];
        assert_eq!(stats.lowered_generators, 1);
        assert_eq!(stats.removed_owner_stores, 1);
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .all(|instr| !matches!(
                    instr,
                    InstrTyped::Store(store) if store.name.id_str() == "_dp_state"
                )),
            "scalarized generator state should not keep the preserved-state seed"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(
                    instr,
                    InstrTyped::Load(load)
                        if load.try_semantic_instr_id() == Some(remapped_instr_id)
                            && load.name.local_location().is_some()
                )),
            "inlined preserved-state loads should become ordinary caller-local loads"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(
                    instr,
                    InstrTyped::Del(del)
                        if del.name.id_str().starts_with("_dp_typed_gen_arg_")
                )),
            "generator-call argument spills should be cleaned immediately after state init"
        );
    }

    #[test]
    fn generator_alias_lowering_ignores_linearized_next_store_protocol_uses() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    yield limit\n\n\
def caller(limit):\n    gen = values(limit)\n    item = next(gen)\n    return item\n",
        )
        .expect("source should lower");
        let values_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "values");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let origin = caller
            .blocks
            .iter_mut()
            .flat_map(|block| block.body.iter_mut())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                    return None;
                };
                let origin = call.try_semantic_instr_id()?;
                call.extra
                    .set_generator_instance_plan(soac_ir_typed::TypedGeneratorInstancePlan {
                        function_id: values_id,
                        kind: soac_core::block_py::FunctionKind::Generator,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    });
                Some(origin)
            })
            .expect("caller should construct the generator");
        let next_instr_id =
            typed_runtime_name_call_instr_ids(caller, RuntimeName::Next, &module_constants)
                .into_iter()
                .next()
                .expect("caller should contain the linearized next(generator) call");

        assert!(
            typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                origin,
                &HashSet::from([next_instr_id]),
            ),
            "the protocol-call source slated for rewrite should not count as residual generator-alias use"
        );
    }

    #[test]
    fn generator_alias_lowering_ignores_residual_uses_outside_active_blocks() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    yield limit\n\n\
def caller(limit):\n    gen = values(limit)\n    item = next(gen)\n    return item\n",
        )
        .expect("source should lower");
        let values_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "values");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let (origin, generator_name) = caller
            .blocks
            .iter_mut()
            .flat_map(|block| block.body.iter_mut())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                    return None;
                };
                let origin = call.try_semantic_instr_id()?;
                call.extra
                    .set_generator_instance_plan(soac_ir_typed::TypedGeneratorInstancePlan {
                        function_id: values_id,
                        kind: soac_core::block_py::FunctionKind::Generator,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    });
                Some((origin, store.name.clone()))
            })
            .expect("caller should construct the generator");
        let next_instr_id =
            typed_runtime_name_call_instr_ids(caller, RuntimeName::Next, &module_constants)
                .into_iter()
                .next()
                .expect("caller should contain the linearized next(generator) call");

        let next_label_index = caller
            .blocks
            .iter()
            .map(|block| block.label.as_u32())
            .filter(|index| *index != BlockLabel::FALLTHROUGH_INDEX)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let inactive_label = BlockLabel::from_index(
            usize::try_from(next_label_index).expect("block label should fit usize"),
        );
        let mut inactive_block = caller
            .blocks
            .first()
            .cloned()
            .expect("caller should have an entry block");
        inactive_block.label = inactive_label;
        inactive_block.body = vec![
            Load::new(generator_name)
                .with_meta(Meta::synthetic())
                .into(),
        ];
        caller.blocks.push(inactive_block);

        let ignored_resume_instr_ids = HashSet::from([next_instr_id]);
        assert!(
            !typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                origin,
                &ignored_resume_instr_ids,
            ),
            "the default scan should keep seeing residual alias uses in physically present blocks"
        );

        let active_blocks = caller
            .blocks
            .iter()
            .map(|block| block.label)
            .filter(|label| *label != inactive_label)
            .collect::<HashSet<_>>();
        assert!(
            typed_generator_state_origin_can_lower_aliases_in_blocks(
                caller,
                &module_constants,
                origin,
                &ignored_resume_instr_ids,
                Some(&active_blocks),
            ),
            "the semantic block filter should ignore residual alias uses outside the trusted-owner view"
        );
    }

    #[test]
    fn generator_alias_lowering_groups_pending_protocol_uses_for_shared_constructor_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    yield limit\n\n\
def caller(limit):\n    gen = values(limit)\n    item = next(gen)\n    return item\n",
        )
        .expect("source should lower");
        let values_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "values");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let (first_origin, generator_store) = caller
            .blocks
            .iter_mut()
            .flat_map(|block| block.body.iter_mut())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                    return None;
                };
                let origin = call.try_semantic_instr_id()?;
                call.extra
                    .set_generator_instance_plan(soac_ir_typed::TypedGeneratorInstancePlan {
                        function_id: values_id,
                        kind: soac_core::block_py::FunctionKind::Generator,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    });
                Some((origin, InstrTyped::Store(store.clone())))
            })
            .expect("caller should construct the generator");
        let (first_next_instr_id, next_store) = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(call) = store.value.as_ref() else {
                    return None;
                };
                typed_expr_is_runtime_name_load(
                    call.func.as_ref(),
                    RuntimeName::Next,
                    &module_constants,
                )
                .then(|| {
                    call.try_semantic_instr_id()
                        .map(|instr_id| (instr_id, InstrTyped::Store(store.clone())))
                })
                .flatten()
            })
            .expect("caller should contain the linearized next(generator) call");

        let second_origin = InstrId::new(910);
        let second_next_instr_id = InstrId::new(911);
        let mut second_generator_store = generator_store.clone();
        let InstrTyped::Store(second_generator_store_inner) = &mut second_generator_store else {
            unreachable!("generator store should stay a store");
        };
        second_generator_store_inner.value = Box::new(
            second_generator_store_inner
                .value
                .as_ref()
                .clone()
                .with_meta(Meta {
                    instr_id: Some(second_origin),
                    ..Meta::synthetic()
                }),
        );
        let mut second_next_store = next_store.clone();
        let InstrTyped::Store(second_next_store_inner) = &mut second_next_store else {
            unreachable!("next store should stay a store");
        };
        second_next_store_inner.value = Box::new(
            second_next_store_inner
                .value
                .as_ref()
                .clone()
                .with_meta(Meta {
                    instr_id: Some(second_next_instr_id),
                    ..Meta::synthetic()
                }),
        );
        let entry = caller
            .blocks
            .first_mut()
            .expect("caller should have an entry block");
        entry.body.push(second_generator_store);
        entry.body.push(second_next_store);

        let pending_alias_use_instr_ids_by_origin = HashMap::from([
            (first_origin, HashSet::from([first_next_instr_id])),
            (second_origin, HashSet::from([second_next_instr_id])),
        ]);
        assert!(
            !typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                first_origin,
                pending_alias_use_instr_ids_by_origin
                    .get(&first_origin)
                    .expect("first origin should have its own pending protocol use"),
            ),
            "a sibling pending protocol use should block per-origin alias lowering before grouping"
        );
        assert!(
            !typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                second_origin,
                pending_alias_use_instr_ids_by_origin
                    .get(&second_origin)
                    .expect("second origin should have its own pending protocol use"),
            ),
            "a sibling pending protocol use should block per-origin alias lowering before grouping"
        );

        let grouped_alias_use_instr_ids_by_origin =
            typed_generator_alias_ignored_instr_ids_by_origin(
                caller,
                &module_constants,
                &pending_alias_use_instr_ids_by_origin,
            );
        let expected_grouped_alias_use_instr_ids =
            HashSet::from([first_next_instr_id, second_next_instr_id]);
        assert_eq!(
            grouped_alias_use_instr_ids_by_origin
                .get(&first_origin)
                .expect("first origin should retain grouped alias ids"),
            &expected_grouped_alias_use_instr_ids,
        );
        assert_eq!(
            grouped_alias_use_instr_ids_by_origin
                .get(&second_origin)
                .expect("second origin should retain grouped alias ids"),
            &expected_grouped_alias_use_instr_ids,
        );
        assert!(
            typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                first_origin,
                grouped_alias_use_instr_ids_by_origin
                    .get(&first_origin)
                    .expect("first origin should retain grouped alias ids"),
            ),
            "shared-constructor sibling protocol uses should be ignored together for the first origin"
        );
        assert!(
            typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                second_origin,
                grouped_alias_use_instr_ids_by_origin
                    .get(&second_origin)
                    .expect("second origin should retain grouped alias ids"),
            ),
            "shared-constructor sibling protocol uses should be ignored together for the second origin"
        );
    }

    #[test]
    fn generator_alias_lowering_groups_pending_protocol_uses_for_shared_alias_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    yield limit\n\n\
def caller(limit):\n    gen = values(limit)\n    alias = gen\n    item = next(alias)\n    return item\n",
        )
        .expect("source should lower");
        let values_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "values");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let (first_origin, generator_store, generator_name) = caller
            .blocks
            .iter_mut()
            .flat_map(|block| block.body.iter_mut())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                    return None;
                };
                let origin = call.try_semantic_instr_id()?;
                call.extra
                    .set_generator_instance_plan(soac_ir_typed::TypedGeneratorInstancePlan {
                        function_id: values_id,
                        kind: soac_core::block_py::FunctionKind::Generator,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    });
                Some((origin, InstrTyped::Store(store.clone()), store.name.clone()))
            })
            .expect("caller should construct the generator");
        let (alias_store, alias_location) = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::Load(load) = store.value.as_ref() else {
                    return None;
                };
                (load.name == generator_name).then_some((
                    InstrTyped::Store(store.clone()),
                    store
                        .name
                        .local_location()
                        .expect("alias assignment should use a local"),
                ))
            })
            .expect("caller should copy the generator through an alias local");
        let (first_next_instr_id, next_store) = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(call) = store.value.as_ref() else {
                    return None;
                };
                typed_expr_is_runtime_name_load(
                    call.func.as_ref(),
                    RuntimeName::Next,
                    &module_constants,
                )
                .then(|| {
                    call.try_semantic_instr_id()
                        .map(|instr_id| (instr_id, InstrTyped::Store(store.clone())))
                })
                .flatten()
            })
            .expect("caller should contain the linearized next(alias) call");

        let second_origin = InstrId::new(920);
        let second_next_instr_id = InstrId::new(921);
        let second_generator_name = typed_test_local("other_gen", LocalLocation(920));
        let mut second_generator_store = generator_store.clone();
        let InstrTyped::Store(second_generator_store_inner) = &mut second_generator_store else {
            unreachable!("generator store should stay a store");
        };
        second_generator_store_inner.name = second_generator_name.clone();
        second_generator_store_inner.value = Box::new(
            second_generator_store_inner
                .value
                .as_ref()
                .clone()
                .with_meta(Meta {
                    instr_id: Some(second_origin),
                    ..Meta::synthetic()
                }),
        );

        let mut second_alias_store = alias_store.clone();
        let InstrTyped::Store(second_alias_store_inner) = &mut second_alias_store else {
            unreachable!("alias store should stay a store");
        };
        second_alias_store_inner.name = typed_test_local("shared_alias", alias_location);
        second_alias_store_inner.value = Box::new(
            Load::new(second_generator_name)
                .with_meta(Meta::synthetic())
                .into(),
        );

        let mut second_next_store = next_store.clone();
        let InstrTyped::Store(second_next_store_inner) = &mut second_next_store else {
            unreachable!("next store should stay a store");
        };
        second_next_store_inner.value = Box::new(
            second_next_store_inner
                .value
                .as_ref()
                .clone()
                .with_meta(Meta {
                    instr_id: Some(second_next_instr_id),
                    ..Meta::synthetic()
                }),
        );

        let entry = caller
            .blocks
            .first_mut()
            .expect("caller should have an entry block");
        entry.body.push(second_generator_store);
        entry.body.push(second_alias_store);
        entry.body.push(second_next_store);

        let pending_alias_use_instr_ids_by_origin = HashMap::from([
            (first_origin, HashSet::from([first_next_instr_id])),
            (second_origin, HashSet::from([second_next_instr_id])),
        ]);
        assert!(
            !typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                first_origin,
                pending_alias_use_instr_ids_by_origin
                    .get(&first_origin)
                    .expect("first origin should have its own pending protocol use"),
            ),
            "a sibling next(alias) use should block per-origin alias lowering before grouping"
        );
        assert!(
            !typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                second_origin,
                pending_alias_use_instr_ids_by_origin
                    .get(&second_origin)
                    .expect("second origin should have its own pending protocol use"),
            ),
            "shared alias locals should still expose sibling protocol uses before grouping"
        );

        let grouped_alias_use_instr_ids_by_origin =
            typed_generator_alias_ignored_instr_ids_by_origin(
                caller,
                &module_constants,
                &pending_alias_use_instr_ids_by_origin,
            );
        let expected_grouped_alias_use_instr_ids =
            HashSet::from([first_next_instr_id, second_next_instr_id]);
        assert_eq!(
            grouped_alias_use_instr_ids_by_origin
                .get(&first_origin)
                .expect("first origin should retain grouped alias ids"),
            &expected_grouped_alias_use_instr_ids,
        );
        assert_eq!(
            grouped_alias_use_instr_ids_by_origin
                .get(&second_origin)
                .expect("second origin should retain grouped alias ids"),
            &expected_grouped_alias_use_instr_ids,
        );
        assert!(
            typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                first_origin,
                grouped_alias_use_instr_ids_by_origin
                    .get(&first_origin)
                    .expect("first origin should retain grouped alias ids"),
            ),
            "shared alias sibling protocol uses should be ignored together for the first origin"
        );
        assert!(
            typed_generator_state_origin_can_lower_aliases(
                caller,
                &module_constants,
                second_origin,
                grouped_alias_use_instr_ids_by_origin
                    .get(&second_origin)
                    .expect("second origin should retain grouped alias ids"),
            ),
            "shared alias sibling protocol uses should be ignored together for the second origin"
        );
    }

    #[test]
    fn lowers_inlined_generator_preserved_cell_refs_to_caller_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    def inner():\n        return limit\n    yield inner\n\n\
def caller(limit):\n    gen = values(limit)\n    return None\n",
        )
        .expect("source should lower");
        let values_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "values");
        let preserved_cell_slot = lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.function_id == values_id)
            .and_then(BlockPyFunction::public_storage_layout)
            .and_then(|layout| {
                layout
                    .preserved_slots
                    .iter()
                    .position(|slot| slot.storage == PreservedSlotStorage::PyCellObject)
            })
            .map(|slot| u32::try_from(slot).expect("preserved cell slot should fit in u32"))
            .expect("values should preserve a lexical cell");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let (origin, generator_name) = caller
            .blocks
            .iter_mut()
            .flat_map(|block| block.body.iter_mut())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                    return None;
                };
                let origin = call.try_semantic_instr_id()?;
                call.extra
                    .set_generator_instance_plan(soac_ir_typed::TypedGeneratorInstancePlan {
                        function_id: values_id,
                        kind: soac_core::block_py::FunctionKind::Generator,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    });
                Some((origin, store.name.clone()))
            })
            .expect("caller should construct the generator");
        let remapped_instr_id = InstrId::new(901);
        let remapped_cell_name_instr_id = InstrId::new(902);
        let state_location = {
            let layout = caller
                .storage_layout
                .as_mut()
                .expect("caller should have storage");
            let location = LocalLocation(
                u32::try_from(layout.stack_slots().len())
                    .expect("test state slot index should fit in u32"),
            );
            layout.ensure_stack_slot("_dp_state");
            location
        };
        let remapped_target = try_allocate_typed_stack_temp(caller, "typed_gen_cellref")
            .unwrap_or_else(|_| panic!("test should allocate a stack temp"))
            .resolved_name();
        let remapped_cell_name_target =
            try_allocate_typed_stack_temp(caller, "typed_gen_cell_name")
                .unwrap_or_else(|_| panic!("test should allocate a stack temp"))
                .resolved_name();
        let entry = caller
            .blocks
            .first_mut()
            .expect("caller should have an entry block");
        entry.body.push(typed_store_temp(
            ResolvedName {
                id: "_dp_state".into(),
                location: NameLocation::Local(state_location),
            },
            typed_load_temp(&generator_name),
        ));
        entry.body.push(
            Store::new(
                remapped_target.clone(),
                Box::new(InstrTyped::CellRef(soac_core::block_py::CellRef::new(
                    CellLocation::Preserved(preserved_cell_slot),
                ))),
            )
            .with_meta(Meta {
                instr_id: Some(remapped_instr_id),
                ..Meta::synthetic()
            })
            .into(),
        );
        entry.body.push(
            Store::new(
                remapped_cell_name_target,
                Box::new(InstrTyped::Load(Load::new(ResolvedName {
                    id: "captured_cell".into(),
                    location: NameLocation::Cell(CellLocation::Preserved(preserved_cell_slot)),
                }))),
            )
            .with_meta(Meta {
                instr_id: Some(remapped_cell_name_instr_id),
                ..Meta::synthetic()
            })
            .into(),
        );

        let callee_module = typed.clone();
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let stats = {
            let (module_constants, callable_defs) =
                (&mut typed.module_constants, &mut typed.callable_defs);
            lower_typed_generator_state_to_locals_with_plan(
                &mut callable_defs[caller_index],
                module_constants,
                &callee_module,
                &HashMap::new(),
                &[TypedGeneratorStateLoweringPlan {
                    generator_origin: origin,
                    function_id: values_id,
                    body_instr_ids: HashSet::from([remapped_instr_id, remapped_cell_name_instr_id]),
                    pending_alias_use_instr_ids: HashSet::new(),
                    alias_cleanup_active_blocks: None,
                    materialized_constructor: None,
                }],
            )
        };
        let caller = &typed.callable_defs[caller_index];
        assert_eq!(stats.lowered_generators, 1);
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .all(|instr| !matches!(
                    instr,
                    InstrTyped::Store(store)
                        if store.try_semantic_instr_id() == Some(remapped_instr_id)
                            && matches!(
                                store.value.as_ref(),
                                InstrTyped::CellRef(cell_ref)
                                    if matches!(
                                        cell_ref.location,
                                        CellLocation::Preserved(slot)
                                            if slot == preserved_cell_slot
                                    )
                            )
                )),
            "inlined preserved CellRef ops should not keep generator-owner storage"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(
                    instr,
                    InstrTyped::Store(store)
                        if store.try_semantic_instr_id() == Some(remapped_instr_id)
                            && matches!(
                                store.value.as_ref(),
                                InstrTyped::Load(load)
                                    if load.name.local_location().is_some()
                            )
                )),
            "preserved CellRef ops should become ordinary loads from the caller-local cell temp"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(
                    instr,
                    InstrTyped::Store(store)
                        if store.try_semantic_instr_id() == Some(remapped_cell_name_instr_id)
                            && matches!(
                                store.value.as_ref(),
                                InstrTyped::Load(load)
                                    if matches!(
                                        load.name.cell_location(),
                                        Some(CellLocation::Owned(_))
                                    )
                            )
                )),
            "inlined preserved cell-value loads should use caller-owned cell aliases"
        );
    }

    #[test]
    fn selective_generator_preserved_remap_respects_nested_instr_ownership() {
        let parent_instr_id = InstrId::new(920);
        let nested_instr_id = InstrId::new(921);
        let preserved_local = ResolvedName {
            id: "_dp_nested_cell".into(),
            location: NameLocation::Local(LocalLocation(0)),
        };
        let preserved_locals = HashMap::from([(PreservedLocation(0), preserved_local.clone())]);
        let preserved_cell_aliases = HashMap::new();

        let foreign_nested = InstrTyped::Tuple(
            Tuple::new(vec![InstrTyped::CellRef(
                soac_core::block_py::CellRef::new(CellLocation::Preserved(0)).with_meta(Meta {
                    instr_id: Some(nested_instr_id),
                    ..Meta::synthetic()
                }),
            )])
            .with_meta(Meta {
                instr_id: Some(parent_instr_id),
                ..Meta::synthetic()
            }),
        );
        let parent_owned_instrs = HashSet::from([parent_instr_id]);
        let mut parent_owned_mapper = TypedGeneratorPreservedLocalRemapper::selective(
            &preserved_locals,
            &preserved_cell_aliases,
            &parent_owned_instrs,
        );
        let mapped_foreign_nested = parent_owned_mapper
            .try_map_instr(foreign_nested)
            .expect("nested ownership remap should succeed");
        assert!(matches!(
            mapped_foreign_nested,
            InstrTyped::Tuple(tuple)
                if matches!(
                    tuple.values.first(),
                    Some(InstrTyped::CellRef(cell_ref))
                        if cell_ref.location == CellLocation::Preserved(0)
                )
        ));

        let owned_nested = InstrTyped::Tuple(
            Tuple::new(vec![InstrTyped::CellRef(
                soac_core::block_py::CellRef::new(CellLocation::Preserved(0)).with_meta(Meta {
                    instr_id: Some(nested_instr_id),
                    ..Meta::synthetic()
                }),
            )])
            .with_meta(Meta {
                instr_id: Some(parent_instr_id),
                ..Meta::synthetic()
            }),
        );
        let nested_owned_instrs = HashSet::from([nested_instr_id]);
        let mut nested_owned_mapper = TypedGeneratorPreservedLocalRemapper::selective(
            &preserved_locals,
            &preserved_cell_aliases,
            &nested_owned_instrs,
        );
        let mapped_owned_nested = nested_owned_mapper
            .try_map_instr(owned_nested)
            .expect("targeted nested remap should succeed");
        assert!(matches!(
            mapped_owned_nested,
            InstrTyped::Tuple(tuple)
                if matches!(
                    tuple.values.first(),
                    Some(InstrTyped::Load(load))
                        if load.name == preserved_local
                )
        ));
    }

    #[test]
    fn lowers_non_inlined_generator_resume_preserved_state_to_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    yield limit\n    return limit\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let mut module_constants = typed.module_constants.clone();
        let function = typed_function_by_qualname_mut(&mut typed, "values");
        let public_layout = function
            .public_storage_layout()
            .expect("generator resume body should expose public preserved storage")
            .clone();
        let pc_location = public_layout
            .preserved_slots
            .iter()
            .position(|slot| slot.logical_name == "_dp_pc")
            .map(|slot| PreservedLocation(u32::try_from(slot).expect("pc slot should fit")))
            .expect("generator resume body should preserve its program counter");
        let limit_location = public_layout
            .preserved_slots
            .iter()
            .position(|slot| slot.logical_name == "limit")
            .map(|slot| PreservedLocation(u32::try_from(slot).expect("limit slot should fit")))
            .expect("generator resume body should preserve the argument");

        let stats = lower_typed_generator_resume_preserved_state_to_locals(function);
        assert_eq!(stats.lowered_functions, 1);
        assert!(
            stats.lowered_slots >= 2,
            "resume lowering should hoist runtime state plus bound preserved locals"
        );
        assert!(stats.boundary_writebacks != 0);

        let entry_label = function.entry_block().label;
        let entry = function
            .blocks
            .iter()
            .find(|block| block.label == entry_label)
            .expect("entry block should remain present");
        assert!(
            entry.body.iter().any(|instr| matches!(
                instr,
                InstrTyped::Store(store)
                    if store.name.local_location().is_some()
                        && matches!(
                            store.value.as_ref(),
                            InstrTyped::Load(load)
                                if load.name.preserved_location() == Some(pc_location)
                        )
            )),
            "resume entry should hydrate _dp_pc into an ordinary local"
        );
        assert!(
            entry.body.iter().any(|instr| matches!(
                instr,
                InstrTyped::Del(del)
                    if del.quietly
                        && del.name.preserved_location() == Some(limit_location)
            )),
            "object preserved values should transfer ownership off preserved storage on entry"
        );
        assert!(
            function.blocks.iter().any(|block| matches!(
                &block.term,
                BlockTerm::BranchTable(branch)
                    if matches!(
                        &branch.index,
                        InstrTyped::Load(load) if load.name.local_location().is_some()
                    )
            )),
            "resume dispatch should read its program counter through the local state"
        );

        for block in &function.blocks {
            for instr in &block.body {
                if matches!(
                    instr,
                    InstrTyped::Load(load)
                        if load.name.preserved_location() == Some(pc_location)
                ) {
                    assert_eq!(
                        block.label, entry_label,
                        "_dp_pc preserved loads should only remain in the resume prologue"
                    );
                }
                if matches!(
                    instr,
                    InstrTyped::Store(store)
                        if store.name.preserved_location() == Some(pc_location)
                ) {
                    assert!(
                        typed_generator_resume_boundary_block(block),
                        "_dp_pc preserved stores should only remain at generator boundaries"
                    );
                }
            }
        }

        simplify_typed_virtual_tuple_ops(function, &mut module_constants);
        assert!(
            function.blocks.iter().any(|block| matches!(
                &block.term,
                BlockTerm::BranchTable(branch)
                    if matches!(
                        &branch.index,
                        InstrTyped::Load(load) if load.name.local_location().is_some()
                    )
            )),
            "loop-carried resume state must not fold the dispatch PC to its entry constant"
        );
    }

    #[test]
    fn keeps_maybe_unbound_generator_resume_slots_in_preserved_storage() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(flag):\n    if flag:\n        value = flag\n    yield flag\n    return value if flag else None\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let function = typed_function_by_qualname_mut(&mut typed, "values");
        let public_layout = function
            .public_storage_layout()
            .expect("generator resume body should expose public preserved storage")
            .clone();
        let deferred_value_location = public_layout
            .preserved_slots
            .iter()
            .position(|slot| slot.logical_name == "value" && slot.init == ClosureInit::Deferred)
            .map(|slot| PreservedLocation(u32::try_from(slot).expect("value slot should fit")))
            .expect("value should remain a maybe-unbound preserved slot");

        let stats = lower_typed_generator_resume_preserved_state_to_locals(function);
        assert!(
            stats.changed(),
            "runtime-private resume state should still lower"
        );
        let entry_label = function.entry_block().label;
        let entry = function
            .blocks
            .iter()
            .find(|block| block.label == entry_label)
            .expect("entry block should remain present");
        assert!(
            entry.body.iter().all(|instr| !matches!(
                instr,
                InstrTyped::Store(store)
                    if matches!(
                        store.value.as_ref(),
                        InstrTyped::Load(load)
                            if load.name.preserved_location() == Some(deferred_value_location)
                    )
            )),
            "maybe-unbound slots should stay in preserved storage until the transform can encode nullable writeback"
        );
    }

    #[test]
    fn repaired_generator_resume_writebacks_do_not_revive_terminally_deleted_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def values(limit):\n    yield limit\n    return limit\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let function = typed_function_by_qualname_mut(&mut typed, "values");
        let public_layout = function
            .public_storage_layout()
            .expect("generator resume body should expose public preserved storage")
            .clone();
        let limit_location = public_layout
            .preserved_slots
            .iter()
            .position(|slot| slot.logical_name == "limit")
            .map(|slot| PreservedLocation(u32::try_from(slot).expect("limit slot should fit")))
            .expect("generator resume body should preserve the argument");

        let outcome =
            lower_typed_generator_resume_preserved_state_to_locals_and_collect_preserved_locals(
                function,
            );
        let limit_local = outcome
            .preserved_locals
            .get(&limit_location)
            .cloned()
            .expect("limit should lower into a resume local");
        let terminal_clear_block = function
            .blocks
            .iter()
            .find(|block| {
                typed_generator_resume_terminal_boundary_block(block)
                    && block.body.iter().any(|instr| {
                        matches!(
                            instr,
                            InstrTyped::Del(del) if del.name == limit_local
                        )
                    })
            })
            .map(|block| block.label)
            .expect("terminal resume block should clear the localized limit slot");

        let inserted =
            ensure_typed_generator_resume_boundary_writebacks(function, &outcome.preserved_locals);
        assert_eq!(inserted, 0);
        let terminal_block = function
            .blocks
            .iter()
            .find(|block| block.label == terminal_clear_block)
            .expect("terminal clear block should remain present");
        assert!(
            terminal_block.body.iter().all(|instr| !matches!(
                instr,
                InstrTyped::Store(store)
                    if store.name.preserved_location() == Some(limit_location)
                        && matches!(
                            store.value.as_ref(),
                            InstrTyped::Load(load) if load.name == limit_local
                        )
            )),
            "repair should not reinsert a preserved-slot writeback after the localized slot was terminally deleted"
        );
    }

    #[test]
    fn inlines_trusted_runtime_protocol_calls_from_return_terms() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    return next(it)\n",
        )
        .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let call_id = first_typed_call_instr_id(&typed.callable_defs[caller_index]);
        let callee_module = typed.clone();
        let stats = {
            let (module_constants, callable_defs) =
                (&mut typed.module_constants, &mut typed.callable_defs);
            inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls(
                &mut callable_defs[caller_index],
                &callee_module,
                module_constants,
                &HashMap::new(),
                &HashMap::from([(
                    call_id,
                    vec![(
                        next_id,
                        TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    )],
                )]),
                &HashMap::from([(
                    call_id,
                    TypedAttrOwnerRef::TypeKey {
                        module_name: "__main__".to_string(),
                        qualname: "IterRange".to_string(),
                    },
                )]),
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
            )
        };

        assert_eq!(stats.rewritten_returns, 1);
    }

    fn first_typed_call_instr_id(function: &BlockPyFunction<TypedBlockPyModuleShape>) -> InstrId {
        struct Finder {
            instr_id: Option<InstrId>,
        }

        impl Visit<InstrTyped> for Finder {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if self.instr_id.is_none() {
                    if let InstrTyped::CallTyped(call) = expr {
                        self.instr_id = call.try_semantic_instr_id();
                        return;
                    }
                }
                expr.visit_children(self);
            }
        }

        let mut finder = Finder { instr_id: None };
        finder.visit_fn(function);
        finder
            .instr_id
            .expect("test function should contain a typed call with an instruction id")
    }

    fn typed_runtime_name_call_instr_ids(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        runtime_name: RuntimeName,
        module_constants: &[ConstantExpr],
    ) -> Vec<InstrId> {
        struct Finder<'a> {
            runtime_name: RuntimeName,
            module_constants: &'a [ConstantExpr],
            instr_ids: Vec<InstrId>,
        }

        impl Visit<InstrTyped> for Finder<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr
                    && typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        self.runtime_name,
                        self.module_constants,
                    )
                    && let Some(instr_id) = call.try_semantic_instr_id()
                {
                    self.instr_ids.push(instr_id);
                    return;
                }
                expr.visit_children(self);
            }
        }

        let mut finder = Finder {
            runtime_name,
            module_constants,
            instr_ids: Vec::new(),
        };
        finder.visit_fn(function);
        finder.instr_ids
    }

    fn annotate_constructor_init_plans_for_test(
        function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
        plans: &HashMap<InstrId, TypedConstructorInitPlan>,
    ) {
        struct Annotator<'a> {
            plans: &'a HashMap<InstrId, TypedConstructorInitPlan>,
        }

        impl VisitMut<InstrTyped> for Annotator<'_> {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if matches!(
                    expr,
                    InstrTyped::CallTyped(_) | InstrTyped::DirectCallableCallTyped(_)
                ) && let Some(instr_id) = expr.try_semantic_instr_id()
                    && let Some(plan) = self.plans.get(&instr_id)
                {
                    expr.typed_extra_mut()
                        .expect("typed constructor call should have typed metadata")
                        .set_constructor_init_plan(*plan);
                }
                expr.visit_children_mut(self);
            }
        }

        Annotator { plans }.visit_fn_mut(function);
    }

    fn constructor_call_plan_sources(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
    ) -> Vec<TypedConstructorInitPlanSource> {
        struct Finder {
            sources: Vec<TypedConstructorInitPlanSource>,
        }

        impl Visit<InstrTyped> for Finder {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                match expr {
                    InstrTyped::CallTyped(call) => {
                        if let Some(plan) = call.extra.constructor_init_plan() {
                            self.sources.push(plan.source);
                        }
                    }
                    InstrTyped::DirectCallableCallTyped(call) => {
                        if let Some(plan) = call.extra.constructor_init_plan() {
                            self.sources.push(plan.source);
                        }
                    }
                    _ => {}
                }
                expr.visit_children(self);
            }
        }

        let mut finder = Finder {
            sources: Vec::new(),
        };
        finder.visit_fn(function);
        finder.sources
    }

    fn stop_iteration_raise_terms(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &[ConstantExpr],
    ) -> usize {
        function
            .blocks
            .iter()
            .filter(|block| typed_block_term_is_stop_iteration_raise(&block.term, module_constants))
            .count()
    }

    fn mark_indexed_field_accesses_for_field(
        function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &[ConstantExpr],
        field_name: &str,
    ) -> usize {
        struct Marker<'a> {
            module_constants: &'a [ConstantExpr],
            field_name: &'a str,
            count: usize,
        }

        impl VisitMut<InstrTyped> for Marker<'_> {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if let InstrTyped::GetAttrTyped(op) = expr
                    && typed_constant_string(op.attr.as_ref(), self.module_constants)
                        == Some(self.field_name)
                {
                    op.access = TypedAttrAccessPlan::IndexedField {
                        source: TypedIndexedFieldPlanSource::OptimizationPlanV3,
                        counter_source: None,
                        guards: vec![TypedIndexedFieldGuard {
                            expected_index: 0,
                            owner_type_ref: TypedAttrOwnerRef::TypeKey {
                                module_name: "__main__".to_string(),
                                qualname: "Box".to_string(),
                            },
                            type_version: 1,
                        }],
                    };
                    self.count += 1;
                }
                if let InstrTyped::SetAttrTyped(op) = expr
                    && typed_constant_string(op.attr.as_ref(), self.module_constants)
                        == Some(self.field_name)
                {
                    op.access = TypedAttrAccessPlan::IndexedField {
                        source: TypedIndexedFieldPlanSource::OptimizationPlanV3,
                        counter_source: None,
                        guards: vec![TypedIndexedFieldGuard {
                            expected_index: 0,
                            owner_type_ref: TypedAttrOwnerRef::TypeKey {
                                module_name: "__main__".to_string(),
                                qualname: "Box".to_string(),
                            },
                            type_version: 1,
                        }],
                    };
                    self.count += 1;
                }
                expr.visit_children_mut(self);
            }
        }

        let mut marker = Marker {
            module_constants,
            field_name,
            count: 0,
        };
        marker.visit_fn_mut(function);
        marker.count
    }

    fn getattrs_for_field_in_reachable_blocks(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        entry: BlockLabel,
        module_constants: &[ConstantExpr],
        field_name: &str,
    ) -> usize {
        struct Counter<'a> {
            module_constants: &'a [ConstantExpr],
            field_name: &'a str,
            count: usize,
        }

        impl Visit<InstrTyped> for Counter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::GetAttrTyped(op) = expr
                    && typed_constant_string(op.attr.as_ref(), self.module_constants)
                        == Some(self.field_name)
                {
                    self.count += 1;
                }
                expr.visit_children(self);
            }
        }

        let labels = typed_block_indices_by_label(function);
        let reachable = typed_reachable_block_labels(function, &labels, entry)
            .expect("test entry should be reachable");
        let mut counter = Counter {
            module_constants,
            field_name,
            count: 0,
        };
        for block in &function.blocks {
            if reachable.contains(&block.label) {
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }
        counter.count
    }

    fn getattrs_for_field_in_hot_reachable_blocks(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        entry: BlockLabel,
        module_constants: &[ConstantExpr],
        field_name: &str,
    ) -> usize {
        struct Counter<'a> {
            module_constants: &'a [ConstantExpr],
            field_name: &'a str,
            count: usize,
        }

        impl Visit<InstrTyped> for Counter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::GetAttrTyped(op) = expr
                    && typed_constant_string(op.attr.as_ref(), self.module_constants)
                        == Some(self.field_name)
                {
                    self.count += 1;
                }
                expr.visit_children(self);
            }
        }

        let labels = typed_block_indices_by_label(function);
        let reachable = typed_hot_reachable_block_labels(function, &labels, entry)
            .expect("test entry should be hot-reachable");
        let mut counter = Counter {
            module_constants,
            field_name,
            count: 0,
        };
        for block in &function.blocks {
            if reachable.contains(&block.label) {
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }
        counter.count
    }

    fn getattrs_for_field_in_virtual_plan(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &[ConstantExpr],
        plan: &TypedVirtualObjectPlan,
        field_name: &str,
    ) -> usize {
        struct Counter<'a> {
            module_constants: &'a [ConstantExpr],
            field_name: &'a str,
            count: usize,
        }

        impl Visit<InstrTyped> for Counter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::GetAttrTyped(op) = expr
                    && typed_constant_string(op.attr.as_ref(), self.module_constants)
                        == Some(self.field_name)
                {
                    self.count += 1;
                }
                expr.visit_children(self);
            }
        }

        let mut counter = Counter {
            module_constants,
            field_name,
            count: 0,
        };
        for block in &function.blocks {
            if typed_virtual_constructor_plan_covers_block(plan, block.label) {
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }
        counter.count
    }

    fn constructor_call_stores_in_virtual_plan(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &[ConstantExpr],
        plan: &TypedVirtualObjectPlan,
    ) -> usize {
        function
            .blocks
            .iter()
            .filter(|block| typed_virtual_constructor_plan_covers_block(plan, block.label))
            .flat_map(|block| block.body.iter())
            .filter(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return false;
                };
                let InstrTyped::CallTyped(call) = store.value.as_ref() else {
                    return false;
                };
                typed_expr_is_runtime_name_load(
                    call.func.as_ref(),
                    RuntimeName::ConstructorCall,
                    module_constants,
                )
            })
            .count()
    }

    fn setattrs_for_field_in_virtual_plan(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &[ConstantExpr],
        plan: &TypedVirtualObjectPlan,
        field_name: &str,
    ) -> usize {
        function
            .blocks
            .iter()
            .filter(|block| typed_virtual_constructor_plan_covers_block(plan, block.label))
            .flat_map(|block| block.body.iter())
            .filter(|instr| {
                let InstrTyped::SetAttrTyped(op) = instr else {
                    return false;
                };
                typed_constant_string(op.attr.as_ref(), module_constants) == Some(field_name)
            })
            .count()
    }

    fn direct_call_guards_in_virtual_plan(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        plan: &TypedVirtualObjectPlan,
    ) -> usize {
        function
            .blocks
            .iter()
            .filter(|block| typed_virtual_constructor_plan_covers_block(plan, block.label))
            .filter(|block| {
                matches!(
                    &block.term,
                    BlockTerm::IfTerm(if_term)
                        if matches!(if_term.test, InstrTyped::DirectCallGuardTest(_))
                )
            })
            .count()
    }

    fn typed_test_local(name: &str, location: LocalLocation) -> ResolvedName {
        ResolvedName {
            id: name.to_string().into(),
            location: NameLocation::Local(location),
        }
    }

    #[test]
    fn typed_field_scalar_state_preserves_object_when_original_local_is_deleted() {
        let root = LocalLocation(1);
        let alias = LocalLocation(2);
        let object = TypedVirtualObjectId(InstrId::new(7).index());
        let scalar = typed_test_local("scalar_current", LocalLocation(3));
        let mut state = TypedVirtualLoweringState::default();
        state.seed_object(
            object,
            root,
            &TypedConstructorFieldBindings {
                fields: vec![TypedConstructorFieldBinding {
                    field_name: "current".to_string(),
                    value: typed_test_local("current", LocalLocation(4)),
                    scalar: Some(scalar.clone()),
                }],
            },
            true,
            false,
        );
        state.set_alias(alias, object);

        state.rebind_local(root);

        assert_eq!(state.object_for_location(root), None);
        assert_eq!(state.object_for_location(alias), Some(object));
        assert_eq!(state.field_value(object, "current"), Some(&scalar));
        assert_eq!(state.field_scalar(object, "current"), Some(&scalar));
    }

    #[test]
    fn lower_codegen_module_to_typed_keeps_loads_first_class() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f(a, b):\n    return a + b\n")
                .expect("source should lower");
        let function_count = lowered.blockpy_module.callable_defs.len();
        let global_names = lowered.blockpy_module.global_names.clone();

        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);

        assert_eq!(typed.callable_defs.len(), function_count);
        assert_eq!(typed.global_names, global_names);

        let mut counter = TypedInstrCounter::default();
        for function in &typed.callable_defs {
            for block in &function.blocks {
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }

        assert!(counter.total > 0);
        assert!(counter.binops > 0);
        assert!(counter.loads > 0);
        assert_eq!(counter.truthy, 0);
        assert_eq!(counter.first_class, first_class_count(&counter));
    }

    #[test]
    fn typed_instr_extras_start_without_result_facts() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f():\n    return None\n")
                .expect("source should lower");

        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let counter = count_typed_extra_facts(&typed);

        assert!(counter.extras > 0);
        assert_eq!(counter.facts, 0);
    }

    #[test]
    fn annotate_typed_module_value_facts_materializes_result_facts() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f():\n    return None\n")
                .expect("source should lower");
        let facts = infer_module_value_facts(&lowered.blockpy_module);
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);

        let changed = annotate_typed_module_value_facts(&mut typed, &facts);
        let counter = count_typed_extra_facts(&typed);

        assert!(changed > 0);
        assert!(counter.facts > 0);
        assert!(counter.none_singletons > 0);
    }

    #[test]
    fn validate_typed_function_value_facts_requires_embedded_result_facts() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f():\n    return None\n")
                .expect("source should lower");
        let facts = infer_module_value_facts(&lowered.blockpy_module);
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);

        let function = typed_function_by_qualname_mut(&mut typed, "f");
        assert!(
            validate_typed_function_value_facts(function).is_err(),
            "typed facts validation should reject an unannotated typed function"
        );

        annotate_typed_function_value_facts(function, &facts);
        validate_typed_function_value_facts(function)
            .expect("annotated typed function should validate");
    }

    #[test]
    fn lower_typed_if_tests_to_truthy_embeds_bool_result_facts() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def f(value):\n    if value:\n        return None\n    return None\n",
        )
        .expect("source should lower");
        let facts = infer_module_value_facts(&lowered.blockpy_module);
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let function = typed_function_by_qualname_mut(&mut typed, "f");

        annotate_typed_function_value_facts(function, &facts);
        let function = lower_typed_function_if_tests_to_truthy(function.clone());
        validate_typed_function_value_facts(&function)
            .expect("truthy-lowered typed function should carry result facts");

        let mut counter = TypedExtraFactCounter::default();
        counter.visit_fn(&function);
        assert!(counter.bools > 0);
    }

    #[test]
    fn refresh_typed_function_value_facts_recovers_binop_result_facts() {
        struct FirstBinOpFactClearer {
            cleared: bool,
        }

        impl VisitMut<InstrTyped> for FirstBinOpFactClearer {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if !self.cleared {
                    if let InstrTyped::BinOp(op) = expr {
                        self.cleared = op.extra_mut().clear_result_facts();
                        return;
                    }
                }
                expr.visit_children_mut(self);
            }
        }

        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f():\n    return 1 + 2\n")
                .expect("source should lower");
        let facts = infer_module_value_facts(&lowered.blockpy_module);
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let function = typed_function_by_qualname_mut(&mut typed, "f");

        annotate_typed_function_value_facts(function, &facts);
        let mut clearer = FirstBinOpFactClearer { cleared: false };
        clearer.visit_fn_mut(function);
        assert!(
            clearer.cleared,
            "test function should contain an annotated typed binop"
        );
        assert!(
            validate_typed_function_value_facts(function).is_err(),
            "clearing binop facts should break typed fact validation"
        );

        assert!(refresh_typed_function_value_facts(function) > 0);
        validate_typed_function_value_facts(function)
            .expect("refreshed typed function should validate");
    }

    #[test]
    fn refresh_typed_function_value_facts_recovers_module_constant_immortal_facts() {
        struct FirstConstantLoadFactForcer {
            forced: bool,
        }

        impl VisitMut<InstrTyped> for FirstConstantLoadFactForcer {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if !self.forced {
                    if let InstrTyped::Load(op) = expr
                        && op.name.location.as_constant().is_some()
                    {
                        self.forced = op
                            .extra_mut()
                            .refine_result_facts(ValueFacts::unknown_pyobj());
                        return;
                    }
                }
                expr.visit_children_mut(self);
            }
        }

        struct FirstConstantLoadFacts {
            facts: Option<ValueFacts>,
        }

        impl Visit<InstrTyped> for FirstConstantLoadFacts {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if self.facts.is_none()
                    && let InstrTyped::Load(op) = expr
                    && op.name.location.as_constant().is_some()
                {
                    self.facts = op.extra().result_facts();
                    return;
                }
                expr.visit_children(self);
            }
        }

        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f():\n    return 'field'\n")
                .expect("source should lower");
        let facts = infer_module_value_facts(&lowered.blockpy_module);
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let function = typed_function_by_qualname_mut(&mut typed, "f");

        annotate_typed_function_value_facts(function, &facts);
        let mut forcer = FirstConstantLoadFactForcer { forced: false };
        forcer.visit_fn_mut(function);
        assert!(
            forcer.forced,
            "test function should contain an annotated module-constant load"
        );

        let mut before = FirstConstantLoadFacts { facts: None };
        before.visit_fn(function);
        assert!(matches!(before.facts, Some(ValueFacts::PyObj(py)) if !py.is_immortal()));

        assert!(refresh_typed_function_value_facts(function) > 0);

        let mut after = FirstConstantLoadFacts { facts: None };
        after.visit_fn(function);
        assert!(matches!(after.facts, Some(ValueFacts::PyObj(py)) if py.is_immortal()));
    }

    #[test]
    fn lower_codegen_module_to_typed_makes_attrs_first_class() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def f(obj, value):\n    obj.x = value\n    return obj.x\n",
        )
        .expect("source should lower");

        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);

        let mut counter = TypedInstrCounter::default();
        for function in &typed.callable_defs {
            for block in &function.blocks {
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }

        assert!(counter.typed_getattrs > 0);
        assert!(counter.typed_setattrs > 0);
    }

    #[test]
    fn lower_codegen_module_to_typed_makes_core_ops_first_class() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def outer(seq, value):
    x = (-value, value)
    seq[0] = value
    y = seq[0]
    del seq[0]
    del y
    def inner():
        return x
    return inner
"#,
        )
        .expect("source should lower");

        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);

        let mut counter = TypedInstrCounter::default();
        for function in &typed.callable_defs {
            counter.visit_fn(function);
        }

        assert!(
            counter.tuples > 0,
            "tuple ops should be first-class typed ops"
        );
        assert!(
            counter.unary_ops > 0,
            "unary ops should be first-class typed ops"
        );
        assert!(
            counter.getitems > 0,
            "getitem ops should be first-class typed ops"
        );
        assert!(
            counter.setitems > 0,
            "setitem ops should be first-class typed ops"
        );
        assert!(
            counter.delitems > 0,
            "delitem ops should be first-class typed ops"
        );
        assert!(
            counter.stores > 0,
            "store ops should be first-class typed ops"
        );
        assert!(counter.dels > 0, "del ops should be first-class typed ops");
        assert!(
            counter.make_cells > 0,
            "make-cell ops should be first-class typed ops"
        );
        assert!(
            counter.cell_refs > 0,
            "cell-ref ops should be first-class typed ops"
        );
        assert!(
            counter.make_functions_with_closure > 0,
            "closure function creation should be a first-class typed op"
        );
    }

    #[test]
    fn typed_legacy_module_round_trips_to_codegen_shape() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def f(a, b):\n    return g(a + b)\n",
        )
        .expect("source should lower");
        let original_counts = count_blockpy_instrs(&lowered.blockpy_module);

        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let round_tripped = try_lower_typed_module_to_codegen_legacy(typed)
            .expect("legacy typed module should map");

        assert_eq!(count_blockpy_instrs(&round_tripped), original_counts);
        assert!(original_counts.binops > 0);
        assert!(original_counts.calls > 0);
    }

    #[test]
    fn lower_typed_if_tests_to_truthy_wraps_branch_conditions() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def f(x):\n    if x:\n        return 1\n    return 0\n",
        )
        .expect("source should lower");
        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);

        let typed = lower_typed_if_tests_to_truthy(typed);

        let mut counter = TypedInstrCounter::default();
        for function in &typed.callable_defs {
            for block in &function.blocks {
                if let BlockTerm::IfTerm(if_term) = &block.term {
                    assert!(
                        matches!(if_term.test, InstrTyped::Truthy(_)),
                        "typed if test should be wrapped in an explicit truthiness op"
                    );
                    assert!(
                        try_lower_typed_term_to_codegen_legacy(block.term.clone()).is_err(),
                        "typed truthiness terms should require typed term emission"
                    );
                }
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }

        assert!(counter.truthy > 0);
        assert!(counter.loads > 0);
        assert_eq!(counter.first_class, first_class_count(&counter));
        assert!(
            try_lower_typed_module_to_codegen_legacy(typed).is_err(),
            "typed truthiness should not silently lower through the legacy adapter"
        );
    }

    #[test]
    fn validates_guarded_method_typed_call_access_plan_shape() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    return it.__next__()\n",
        )
        .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedMethod {
                method_name: "__next__".to_string(),
                method_guards: vec![TypedDirectMethodCallGuard {
                    function_id: next_id,
                    owner_type_ref: TypedAttrOwnerRef::TypeKey {
                        module_name: "__main__".to_string(),
                        qualname: "IterRange".to_string(),
                    },
                    type_version: 1,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                }],
            },
        );

        validate_typed_function_call_access_plans(caller).expect("guarded method shape is valid");
    }

    #[test]
    fn validates_guarded_next_runtime_protocol_typed_call_access_plan_shape() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    return next(it)\n",
        )
        .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                runtime_name: RuntimeName::Next,
                method_name: "__next__".to_string(),
                method_guards: vec![TypedDirectMethodCallGuard {
                    function_id: next_id,
                    owner_type_ref: TypedAttrOwnerRef::TypeKey {
                        module_name: "__main__".to_string(),
                        qualname: "IterRange".to_string(),
                    },
                    type_version: 1,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                }],
            },
        );

        validate_typed_function_call_access_plans(caller)
            .expect("guarded next protocol shape is valid");
        assert_eq!(
            lower_typed_function_call_access_plan_instrs(caller),
            0,
            "runtime protocol access plans stay on CallTyped for codegen/inlining"
        );
    }

    #[test]
    fn lowers_guarded_callable_typed_call_access_plan_to_instr() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def add(a, b):\n    return a + b\n\n\
def caller(a, b):\n    return add(a, b)\n",
        )
        .expect("source should lower");
        let add_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "add");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedCallable {
                function_guards: vec![TypedDirectFunctionCallGuard {
                    function_id: add_id,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                        ],
                    },
                }],
            },
        );

        assert_eq!(lower_typed_function_call_access_plan_instrs(caller), 1);
        validate_typed_function_call_access_plans(caller)
            .expect("lowered guarded callable shape is valid");

        let mut counter = TypedInstrCounter::default();
        for block in &caller.blocks {
            for instr in &block.body {
                counter.visit_instr(instr);
            }
            counter.visit_term(&block.term);
        }
        assert_eq!(counter.typed_calls, 0);
        assert_eq!(counter.guarded_callable_calls, 1);
    }

    #[test]
    fn typed_direct_call_inlining_skips_closure_callees() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def outer(value):\n    strings = []\n    def append(item):\n        strings.append(item)\n    append(value)\n    return strings\n",
        )
        .expect("source should lower");
        let append_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "outer.<locals>.append");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let outer = typed_function_by_qualname_mut(&mut typed, "outer");
            call_id = first_typed_call_instr_id(outer);
            replace_first_typed_call_access(
                outer,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: append_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(outer);
        }

        let callee_module = typed.clone();
        let outer = typed_function_by_qualname_mut(&mut typed, "outer");
        let original_storage_layout = outer.storage_layout.clone();
        let stats = inline_typed_function_direct_call_stores(
            outer,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    append_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 0);
        assert_eq!(stats.rewritten_effect_only_calls, 0);
        assert!(stats.skipped_candidates > 0);
        assert_eq!(outer.storage_layout, original_storage_layout);
    }

    #[test]
    fn typed_direct_call_inlining_allows_generator_factories_with_preserved_state() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def gen():\n    yield 1\n\n\
def make_gen():\n    return gen()\n\n\
def caller():\n    value = make_gen()\n    return value\n",
        )
        .expect("source should lower");
        let factory_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "make_gen");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let generator_resume = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "gen")
            .expect("missing typed generator resume");
        assert!(generator_resume.body_params.is_some());
        assert!(
            !generator_resume
                .public_storage_layout()
                .expect("generator should record public storage layout")
                .preserved_slots
                .is_empty()
        );
        let generator_factory = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "make_gen")
            .expect("missing typed generator factory");
        assert!(matches!(
            generator_factory.kind,
            soac_core::block_py::FunctionKind::Function
        ));
        let layout = generator_factory
            .storage_layout
            .as_ref()
            .expect("generator factory should record storage layout");
        assert!(layout.freevars.is_empty(), "{layout:?}");
        assert!(layout.cellvars.is_empty(), "{layout:?}");
        assert!(layout.preserved_slots.is_empty(), "{layout:?}");

        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: factory_id,
                        arg_plan: TypedDirectCallArgPlan { sources: vec![] },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(factory_id, TypedDirectCallArgPlan { sources: vec![] })],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 1);
        assert_eq!(stats.skipped_candidates, 0);
    }

    #[test]
    fn typed_direct_call_inlining_rejects_unbound_generator_preserved_storage() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def gen():\n    yield 1\n\n\
def caller():\n    value = gen()\n    return value\n",
        )
        .expect("source should lower");
        let gen_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "gen");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        {
            let generator_factory = typed_function_by_qualname_mut(&mut typed, "gen");
            let layout = generator_factory
                .storage_layout
                .as_ref()
                .expect("generator factory should record storage layout");
            assert!(!layout.preserved_slots.is_empty(), "{layout:?}");
            generator_factory.blocks[0].body.insert(
                0,
                InstrTyped::Load(Load::new(ResolvedName {
                    id: "unbound_generator_state".into(),
                    location: NameLocation::Preserved(PreservedLocation(0)),
                })),
            );
        }

        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: gen_id,
                        arg_plan: TypedDirectCallArgPlan { sources: vec![] },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let original_storage_layout = caller.storage_layout.clone();
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(gen_id, TypedDirectCallArgPlan { sources: vec![] })],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 0);
        assert!(stats.skipped_candidates > 0);
        assert_eq!(caller.storage_layout, original_storage_layout);
        assert!(caller.blocks.iter().all(|block| {
            block.body.iter().all(|instr| {
                !matches!(instr, InstrTyped::Load(load) if load.name.preserved_location().is_some())
            })
        }));
    }

    #[test]
    fn typed_direct_callable_inlining_guards_mutable_function_identity() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def add(a):\n    return a\n\n\
def caller(a):\n    value = add(a)\n    return value\n",
        )
        .expect("source should lower");
        let add_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "add");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            let plans = TypedCallEmissionPlans {
                by_source: HashMap::from([(
                    call_id,
                    TypedCallEmissionPlan::DirectCallable {
                        function_guard: TypedDirectFunctionCallGuard {
                            function_id: add_id,
                            arg_plan: TypedDirectCallArgPlan {
                                sources: vec![TypedDirectCallArgSource::Provided(0)],
                            },
                        },
                    },
                )]),
            };
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed direct callable emission plan should lower");
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    add_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 1);
        assert!(caller.blocks.iter().any(|block| {
            matches!(
                &block.term,
                BlockTerm::IfTerm(if_term)
                    if matches!(
                        &if_term.test,
                        InstrTyped::DirectCallGuardTest(guard)
                            if matches!(
                                &guard.kind,
                                TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id }
                                    if *function_id == add_id
                            ) && !if_term.test.guard_miss_deopt_enabled()
                    )
            )
        }));
        assert!(caller.blocks.iter().any(|block| {
            block.body.iter().any(|instr| match instr {
                InstrTyped::Store(store) => matches!(
                    store.value.as_ref(),
                    InstrTyped::CallTyped(call)
                        if call.access == TypedCallAccessPlan::Generic
                ),
                InstrTyped::CallTyped(call) => call.access == TypedCallAccessPlan::Generic,
                _ => false,
            })
        }));
    }

    #[test]
    fn typed_trusted_runtime_callable_inlining_omits_guard_and_generic_fallback() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def runtime_target(a):\n    return a\n\n\
def caller(a):\n    value = runtime_target(a)\n    return value\n",
        )
        .expect("source should lower");
        let target_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "runtime_target");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            let plans = TypedCallEmissionPlans {
                by_source: HashMap::from([(
                    call_id,
                    TypedCallEmissionPlan::DirectCallable {
                        function_guard: TypedDirectFunctionCallGuard {
                            function_id: target_id,
                            arg_plan: TypedDirectCallArgPlan {
                                sources: vec![TypedDirectCallArgSource::Provided(0)],
                            },
                        },
                    },
                )]),
            };
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed trusted runtime callable emission plan should lower");

            let direct_call = caller
                .blocks
                .iter_mut()
                .flat_map(|block| block.body.iter_mut())
                .find_map(|instr| match instr {
                    InstrTyped::Store(store) => match store.value.as_mut() {
                        InstrTyped::DirectCallableCallTyped(call) => Some(call),
                        _ => None,
                    },
                    _ => None,
                })
                .expect("caller should contain the lowered direct callable");
            direct_call.func = Box::new(InstrTyped::Load(
                Load::new(ResolvedName::runtime_name("range")).with_meta(Meta::synthetic()),
            ));
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    target_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 1);
        assert!(caller.blocks.iter().all(|block| {
            !matches!(
                &block.term,
                BlockTerm::IfTerm(if_term)
                    if matches!(if_term.test, InstrTyped::DirectCallGuardTest(_))
            ) && block.body.iter().all(|instr| match instr {
                InstrTyped::Store(store) => !matches!(
                    store.value.as_ref(),
                    InstrTyped::CallTyped(call)
                        if call.access == TypedCallAccessPlan::Generic
                ),
                InstrTyped::CallTyped(call) => call.access != TypedCallAccessPlan::Generic,
                _ => true,
            })
        }));
    }

    #[test]
    fn typed_trusted_runtime_protocol_inlining_omits_guard_and_generic_fallback() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    value = next(it)\n    return value\n",
        )
        .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                    runtime_name: RuntimeName::Next,
                    method_name: "__next__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: next_id,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "__main__".to_string(),
                            qualname: "IterRange".to_string(),
                        },
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
                |call| call.args.len() == 1 && call.keywords.is_empty(),
            );
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores_impl(
            caller,
            &callee_module,
            None,
            TypedInlineExternalCallees::Plain(&HashMap::new()),
            &HashMap::from([(
                call_id,
                vec![(
                    next_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
            &HashMap::from([(
                call_id,
                TypedAttrOwnerRef::TypeKey {
                    module_name: "__main__".to_string(),
                    qualname: "IterRange".to_string(),
                },
            )]),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(stats.rewritten_stores, 1);
        assert!(caller.blocks.iter().all(|block| {
            !matches!(
                &block.term,
                BlockTerm::IfTerm(if_term)
                    if matches!(if_term.test, InstrTyped::DirectCallGuardTest(_))
            )
        }));
        assert!(caller.blocks.iter().all(|block| {
            block.body.iter().all(|instr| {
                !matches!(instr, InstrTyped::CallTyped(call) if call.access == TypedCallAccessPlan::Generic)
            })
        }));
    }

    #[test]
    fn lowers_guarded_method_typed_call_access_plan_to_instr() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    return it.__next__()\n",
        )
        .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedMethod {
                method_name: "__next__".to_string(),
                method_guards: vec![TypedDirectMethodCallGuard {
                    function_id: next_id,
                    owner_type_ref: TypedAttrOwnerRef::TypeKey {
                        module_name: "__main__".to_string(),
                        qualname: "IterRange".to_string(),
                    },
                    type_version: 1,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                }],
            },
        );

        assert_eq!(lower_typed_function_call_access_plan_instrs(caller), 1);
        validate_typed_function_call_access_plans(caller)
            .expect("lowered guarded method shape is valid");

        let mut counter = TypedInstrCounter::default();
        for block in &caller.blocks {
            for instr in &block.body {
                counter.visit_instr(instr);
            }
            counter.visit_term(&block.term);
        }
        assert_eq!(counter.typed_calls, 0);
        assert_eq!(counter.guarded_method_calls, 1);
    }

    #[test]
    fn typed_direct_call_inlining_rewrites_methods_under_exception_edges() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    try:\n        value = it.__next__()\n    except StopIteration:\n        return 0\n    return value\n",
        )
        .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedMethod {
                    method_name: "__next__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: next_id,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "__main__".to_string(),
                            qualname: "IterRange".to_string(),
                        },
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
                typed_call_func_is_getattr,
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    next_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 1);
        assert_eq!(stats.skipped_exception_edges, 0);
        assert!(caller.blocks.iter().any(|block| {
            block.exc_edge.is_some()
                && matches!(
                    &block.term,
                    BlockTerm::IfTerm(if_term)
                        if matches!(if_term.test, InstrTyped::DirectCallGuardTest(_))
                )
        }));
        let synthetic_cleanup_dels = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| match instr {
                InstrTyped::Del(del) if del.name.id_str().starts_with("_dp_typed_inline_") => {
                    Some(del)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            !synthetic_cleanup_dels.is_empty(),
            "method inline cleanup should delete optimizer-owned temps"
        );
        assert!(
            synthetic_cleanup_dels.iter().all(|del| del.quietly),
            "optimizer-owned inline cleanup must tolerate paths that bypass temp initialization"
        );
    }

    #[test]
    fn typed_direct_call_inlining_rewrites_next_protocol_under_exception_edges() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    try:\n        value = next(it)\n    except StopIteration:\n        return 0\n    return value\n",
        )
        .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                    runtime_name: RuntimeName::Next,
                    method_name: "__next__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: next_id,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "__main__".to_string(),
                            qualname: "IterRange".to_string(),
                        },
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
                |call| call.args.len() == 1 && call.keywords.is_empty(),
            );
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    next_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 1);
        assert_eq!(stats.skipped_exception_edges, 0);
        assert!(caller.blocks.iter().any(|block| {
            block.exc_edge.is_some()
                && matches!(
                    &block.term,
                    BlockTerm::IfTerm(if_term)
                        if matches!(if_term.test, InstrTyped::DirectCallGuardTest(_))
                )
        }));
    }

    #[test]
    fn typed_direct_call_inlining_keeps_duplicate_semantic_ids_together() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        if self.current >= self.stop:\n            raise StopIteration\n        return self.current\n\n\
def caller(it):\n    try:\n        value = next(it)\n    except StopIteration:\n        return 0\n    return value\n",
        )
        .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                    runtime_name: RuntimeName::Next,
                    method_name: "__next__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: next_id,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "__main__".to_string(),
                            qualname: "IterRange".to_string(),
                        },
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
                |call| call.args.len() == 1 && call.keywords.is_empty(),
            );
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    next_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 1);
        let mut seen = HashSet::new();
        for mapping in &stats.instr_id_mappings {
            assert!(
                seen.insert((mapping.inline_instance, mapping.callee_instr_id)),
                "semantic instruction {} in inline instance {} was remapped more than once",
                mapping.callee_instr_id,
                mapping.inline_instance
            );
        }
    }

    #[test]
    fn typed_direct_call_inlining_preserves_callee_exception_edges() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def callee(value):\n    try:\n        result = value.__index__()\n    except AttributeError:\n        raise TypeError('bad')\n    return result\n\n\
def caller(value):\n    result = callee(value)\n    return result\n",
        )
        .expect("source should lower");
        let callee_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "callee");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let callee = typed
            .callable_defs
            .iter()
            .find(|function| function.function_id == callee_id)
            .expect("typed callee should exist");
        assert!(
            callee.blocks.iter().any(|block| block.exc_edge.is_some()),
            "callee should contain an internal try/except exception edge"
        );
        assert!(
            callee.blocks.iter().any(|block| !block.params.is_empty()),
            "callee exception handler should carry exception block params"
        );
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: callee_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
                |call| typed_expr_loads_name(call.func.as_ref(), "callee"),
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    callee_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 1);
        let labels = caller
            .blocks
            .iter()
            .map(|block| block.label)
            .collect::<HashSet<_>>();
        assert!(
            caller.blocks.iter().any(|block| {
                block.exc_edge.as_ref().is_some_and(|edge| {
                    labels.contains(&edge.target)
                        && caller
                            .blocks
                            .iter()
                            .any(|target| target.label == edge.target && !target.params.is_empty())
                })
            }),
            "inlined caller should retain the callee's internal exception-handler CFG"
        );
    }

    #[test]
    fn typed_constructor_hot_continuation_split_clones_joined_successors() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(value):\n    obj = Box(value)\n    if value:\n        return obj.value\n    return 0\n",
        )
        .expect("source should lower");
        let init_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "Box.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(stats.rewritten_stores, 1);

        let before_blocks = caller.blocks.len();
        let split_stats = split_typed_constructor_hot_continuations(caller, &module_constants);
        assert_eq!(split_stats.clones.len(), 1);
        assert!(split_stats.cloned_blocks > 0);
        assert!(!split_stats.instr_id_mappings.is_empty());
        assert_eq!(
            caller.blocks.len(),
            before_blocks + split_stats.cloned_blocks
        );

        let clone = split_stats.clones[0];
        let labels = typed_block_indices_by_label(caller);
        let hot_block = block_by_label(caller, &labels, clone.hot_block)
            .expect("hot constructor block should remain in the function");
        assert!(matches!(
            &hot_block.term,
            BlockTerm::Jump(edge) if edge.target == clone.cloned_entry
        ));
        assert!(
            labels.contains_key(&clone.original_entry),
            "generic fallback should still use the original successor graph"
        );
        assert!(
            labels.contains_key(&clone.cloned_entry),
            "hot constructor path should jump into the cloned successor graph"
        );
        assert_eq!(
            split_typed_constructor_hot_continuations(caller, &module_constants).cloned_blocks,
            0,
            "a hot constructor path whose successor is already private should not be cloned again"
        );
    }

    #[test]
    fn typed_constructor_hot_continuation_split_clones_only_hot_loop_region() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(value):\n    obj = Box(value)\n    while value:\n        value = value - 1\n    return obj.value\n",
        )
        .expect("source should lower");
        let init_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "Box.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let inline_stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);

        let labels = typed_block_indices_by_label(caller);
        let candidate =
            find_typed_constructor_hot_continuation_split_candidate(caller, &module_constants)
                .expect("constructor hot path should need continuation splitting");
        let full_reachable =
            typed_hot_reachable_block_labels(caller, &labels, candidate.original_entry)
                .expect("constructor continuation should be hot-reachable");
        assert!(
            candidate.reachable.len() < full_reachable.len(),
            "loop splitting should clone only the cyclic hot region, not the full hot suffix"
        );

        let split_stats = split_typed_constructor_hot_continuations(caller, &module_constants);
        assert_eq!(split_stats.clones.len(), 1);
        assert_eq!(split_stats.cloned_blocks, candidate.reachable.len());
    }

    #[test]
    fn selected_cleanup_split_skips_plain_callable_inline_cleanup() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def add(value):\n    return value + 1\n\n\
def caller(flag, value):\n    result = add(value)\n    if flag:\n        return result\n    return value\n",
        )
        .expect("source should lower");
        let add_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "add");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: add_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    add_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );

        assert_eq!(stats.rewritten_stores, 1);
        assert!(
            stats.hot_state_cleanup_labels.is_empty(),
            "plain callable inline cleanup should not be eligible for hot-loop cloning"
        );
        assert_eq!(
            split_typed_inline_cleanup_hot_continuations_for_labels(caller, &HashSet::new())
                .clones
                .len(),
            0
        );
    }

    #[test]
    fn typed_alias_hot_continuation_split_clones_joined_successors() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __iter__(self):\n        return self\n\n\
def caller(it):\n    iterator = iter(it)\n    return iterator\n",
        )
        .expect("source should lower");
        let iter_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__iter__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                    runtime_name: RuntimeName::Iter,
                    method_name: "__iter__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: iter_id,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "__main__".to_string(),
                            qualname: "IterRange".to_string(),
                        },
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
            );
        }

        let callee_module = typed.clone();
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let inline_stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    iter_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);

        let before_blocks = caller.blocks.len();
        let split_stats = split_typed_alias_hot_continuations(caller, &module_constants);
        assert_eq!(split_stats.clones.len(), 1);
        assert!(split_stats.cloned_blocks > 0);
        assert_eq!(
            caller.blocks.len(),
            before_blocks + split_stats.cloned_blocks
        );
        let labels = typed_block_indices_by_label(caller);
        let clone = split_stats.clones[0];
        let hot_block = block_by_label(caller, &labels, clone.hot_block)
            .expect("hot alias block should still exist");
        assert!(matches!(
            &hot_block.term,
            BlockTerm::Jump(edge) if edge.target == clone.cloned_entry
        ));
        assert!(
            labels.contains_key(&clone.original_entry),
            "generic fallback should still use the original successor graph"
        );
        assert!(
            labels.contains_key(&clone.cloned_entry),
            "hot alias path should jump into the cloned successor graph"
        );
        assert_eq!(
            split_typed_alias_hot_continuations(caller, &module_constants).cloned_blocks,
            0,
            "a hot alias path whose successor is already private should not be cloned again"
        );
    }

    #[test]
    fn typed_alias_hot_continuation_split_clones_loop_regions_containing_the_alias_store() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __iter__(self):\n        return self\n\n\
def caller(it, flag):\n    result = it\n    while flag:\n        result = iter(result)\n        flag = flag - 1\n    return result\n",
        )
        .expect("source should lower");
        let iter_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__iter__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                    runtime_name: RuntimeName::Iter,
                    method_name: "__iter__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: iter_id,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "__main__".to_string(),
                            qualname: "IterRange".to_string(),
                        },
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
            );
        }

        let callee_module = typed.clone();
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let inline_stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    iter_id,
                    TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);

        let candidate = find_typed_alias_hot_continuation_split_candidate(
            caller,
            &module_constants,
            &HashSet::new(),
            &HashSet::new(),
        )
        .expect("loop-carried alias continuation should be splittable");
        assert!(
            candidate.reachable.contains(&candidate.hot_block),
            "the regression should exercise a cyclic hot alias region"
        );

        let before_blocks = caller.blocks.len();
        let split_stats = split_typed_alias_hot_continuations(caller, &module_constants);
        assert_eq!(split_stats.clones.len(), 1);
        assert!(split_stats.cloned_blocks > 0);
        assert_eq!(
            caller.blocks.len(),
            before_blocks + split_stats.cloned_blocks
        );
        let second_candidate = find_typed_alias_hot_continuation_split_candidate(
            caller,
            &module_constants,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert!(
            second_candidate.is_none(),
            "a cloned cyclic alias loop should not immediately become fresh split work"
        );
    }

    #[test]
    fn typed_iter_local_alias_calls_count_as_alias_candidates() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def caller(value):
    iterator = iter(value)
    return iterator
"#,
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let iter_alias_value = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(_) = store.value.as_ref() else {
                    return None;
                };
                Some(store.value.as_ref())
            })
            .expect("caller should contain an iter(value) store");
        assert!(typed_iter_local_alias_call(
            iter_alias_value,
            &module_constants,
        ));
        assert!(typed_expr_local_alias_candidate(
            iter_alias_value,
            &module_constants,
        ));
    }

    #[test]
    fn typed_field_scalarization_rewrites_indexed_loads_on_hot_constructor_clone() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(value):\n    obj = Box(value)\n    if value:\n        return obj.value\n    return 0\n",
        )
        .expect("source should lower");
        let inline_plan = crate::passes::plan_module_inlining(
            &crate::passes::summarize_module_escapes(&lowered.blockpy_module),
        );
        let init_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "Box.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let inline_stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);
        let constructor_field_bindings = typed_constructor_field_bindings_from_inline_stats(
            &callee_module,
            &inline_plan,
            &module_constants,
            &inline_stats,
        );
        assert_eq!(constructor_field_bindings.len(), 1);

        let split_stats = split_typed_constructor_hot_continuations(caller, &module_constants);
        assert_eq!(split_stats.clones.len(), 1);
        assert!(
            mark_indexed_field_accesses_for_field(caller, &module_constants, "value") >= 2,
            "both original and cloned continuations should still contain the field load before scalarization"
        );
        let clone = split_stats.clones[0];
        assert!(
            getattrs_for_field_in_reachable_blocks(
                caller,
                clone.cloned_entry,
                &module_constants,
                "value",
            ) > 0,
            "the cloned hot continuation should contain the field load before scalarization"
        );

        let mut virtualization_plan =
            plan_typed_virtual_objects(caller, &module_constants, &constructor_field_bindings);
        assert_eq!(
            virtualization_plan.objects.len(),
            1,
            "an already-private constructor continuation should virtualize without requiring a prior split"
        );
        let scalar_stats = lower_typed_virtual_fields_to_locals_with_plan(
            caller,
            &module_constants,
            &mut virtualization_plan,
        );
        assert_eq!(scalar_stats.seeded_objects, 1);
        assert_eq!(scalar_stats.scalar_slots, 1);
        assert_eq!(scalar_stats.inserted_scalar_stores, 1);
        assert_eq!(scalar_stats.rewritten_loads, 1);
        assert_eq!(
            getattrs_for_field_in_reachable_blocks(
                caller,
                clone.cloned_entry,
                &module_constants,
                "value",
            ),
            0,
            "the private hot continuation should use the constructor scalar instead of reloading the field"
        );
        assert!(
            getattrs_for_field_in_reachable_blocks(
                caller,
                clone.original_entry,
                &module_constants,
                "value",
            ) > 0,
            "the generic continuation should keep the original field load"
        );
    }

    #[test]
    fn typed_field_scalarization_ignores_deopting_direct_call_fallback_edges() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(value):\n    obj = Box(value)\n    return obj.value\n",
        )
        .expect("source should lower");
        let inline_plan = crate::passes::plan_module_inlining(
            &crate::passes::summarize_module_escapes(&lowered.blockpy_module),
        );
        let init_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "Box.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let inline_stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);
        let constructor_field_bindings = typed_constructor_field_bindings_from_inline_stats(
            &callee_module,
            &inline_plan,
            &module_constants,
            &inline_stats,
        );
        assert_eq!(constructor_field_bindings.len(), 1);
        assert!(
            mark_indexed_field_accesses_for_field(caller, &module_constants, "value") > 0,
            "test should contain the field load before scalarization"
        );

        let mut virtualization_plan =
            plan_typed_virtual_objects(caller, &module_constants, &constructor_field_bindings);
        let scalar_stats = lower_typed_virtual_fields_to_locals_with_plan(
            caller,
            &module_constants,
            &mut virtualization_plan,
        );
        assert_eq!(scalar_stats.seeded_objects, 1);
        assert_eq!(scalar_stats.scalar_slots, 1);
        assert_eq!(
            scalar_stats.rewritten_loads, 1,
            "deopting guard fallback edges should not kill hot constructor scalar state"
        );
        let entry = caller
            .blocks
            .first()
            .expect("caller should contain blocks")
            .label;
        assert_eq!(
            getattrs_for_field_in_reachable_blocks(caller, entry, &module_constants, "value"),
            0,
            "the hot constructor field should be read from its scalar temp"
        );
    }

    #[test]
    fn typed_fully_virtual_lowering_erases_trusted_non_escaping_objects_without_materialization() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(value):\n    obj = Box(value)\n    return obj.value\n",
        )
        .expect("source should lower");
        let inline_plan = crate::passes::plan_module_inlining(
            &crate::passes::summarize_module_escapes(&lowered.blockpy_module),
        );
        let init_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "Box.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let inline_stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);
        let mut constructor_field_bindings = typed_constructor_field_bindings_from_inline_stats(
            &callee_module,
            &inline_plan,
            &module_constants,
            &inline_stats,
        );
        assert_eq!(constructor_field_bindings.len(), 1);
        let split_stats = split_typed_constructor_hot_continuations(caller, &module_constants);
        assert!(
            split_stats.cloned_blocks >= 1,
            "trusted constructor paths should be isolated from the generic fallback before fully virtual lowering"
        );
        for mapping in &split_stats.instr_id_mappings {
            if let Some(bindings) = constructor_field_bindings
                .get(&mapping.callee_instr_id)
                .cloned()
            {
                constructor_field_bindings.insert(mapping.caller_instr_id, bindings);
            }
        }
        assert!(
            mark_indexed_field_accesses_for_field(caller, &module_constants, "value") > 0,
            "test should contain the field load before lowering"
        );

        let mut trusted_sources = HashSet::from([call_id]);
        let trusted_inline_instances = inline_stats
            .inline_instance_sources
            .iter()
            .filter_map(|mapping| {
                trusted_sources
                    .contains(&mapping.source_instr_id)
                    .then_some(mapping.inline_instance)
            })
            .collect::<HashSet<_>>();
        trusted_sources.extend(inline_stats.instr_id_mappings.iter().filter_map(|mapping| {
            (trusted_inline_instances.contains(&mapping.inline_instance)
                && constructor_field_bindings.contains_key(&mapping.caller_instr_id))
            .then_some(mapping.caller_instr_id)
        }));
        for mapping in &split_stats.instr_id_mappings {
            if trusted_sources.contains(&mapping.callee_instr_id) {
                trusted_sources.insert(mapping.caller_instr_id);
            }
        }
        let mut plan = plan_typed_fully_virtual_objects(
            caller,
            &module_constants,
            &constructor_field_bindings,
            &trusted_sources,
        );
        assert_eq!(plan.objects.len(), 1);
        assert!(plan.materializing_objects.is_empty());
        assert!(plan.materialization_boundaries().is_empty());
        let stats = lower_typed_fully_virtual_objects_to_locals_with_plan(
            caller,
            &module_constants,
            &mut plan,
        );
        assert!(stats.changed());
        assert_eq!(stats.field_lowering.seeded_objects, 1);
        assert!(
            stats.virtualization.removed_materializations >= 1,
            "fully virtual lowering should erase the trusted allocation"
        );
        assert_eq!(
            getattrs_for_field_in_virtual_plan(
                caller,
                &module_constants,
                &plan.objects[0],
                "value"
            ),
            0,
            "fully virtual lowering should leave an ordinary local read on the trusted path"
        );
    }

    #[test]
    fn typed_cross_module_constructor_inline_feeds_field_scalarization() {
        let callee_module = lower_test_module_with_id(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n",
            1,
        );
        let inline_plan = crate::passes::plan_module_inlining(
            &crate::passes::summarize_module_escapes(&callee_module),
        );
        let init_id = blockpy_function_id_by_qualname(&callee_module, "Box.__init__");
        let constructor_entry_id = constructor_entry_function_id_for_init(&callee_module, init_id)
            .expect("class lowering should add a constructor entry");
        let callee_typed = lower_blockpy_module_to_typed(callee_module);
        let constructor_entry = callee_typed
            .callable_defs
            .iter()
            .find(|function| function.function_id == constructor_entry_id)
            .expect("typed callee module should contain constructor entry")
            .clone();
        let external_callees = HashMap::from([(
            constructor_entry_id,
            TypedExternalInlineCallee {
                function: constructor_entry,
                module_constants: callee_typed.module_constants.clone(),
                inline_plan: Some(inline_plan),
            },
        )]);

        let caller_module = lower_test_module_with_id(
            "def caller(factory, value):\n    obj = factory(value)\n    if value:\n        return obj.value\n    return 0\n",
            2,
        );
        let mut caller_typed = lower_blockpy_module_to_typed(caller_module);
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut caller_typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let caller_callee_module = caller_typed.clone();
        let caller_index = caller_typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let inline_stats = inline_typed_function_direct_call_stores_with_external_callees(
            &mut caller_typed.callable_defs[caller_index],
            &caller_callee_module,
            &mut caller_typed.module_constants,
            &external_callees,
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);
        let constructor_field_bindings =
            typed_constructor_field_bindings_from_inline_stats_with_external_callees(
                &caller_callee_module,
                &crate::passes::InlinePlanModule::default(),
                &caller_typed.module_constants,
                &external_callees,
                &inline_stats,
            );
        assert_eq!(constructor_field_bindings.len(), 1);
        let constructor_init_plans =
            typed_constructor_init_plans_from_inline_stats_with_external_callees(
                &caller_callee_module,
                &caller_typed.module_constants,
                &external_callees,
                &inline_stats,
            );
        assert_eq!(constructor_init_plans.len(), 1);
        assert!(
            constructor_init_plans
                .values()
                .any(|plan| plan.init_function_id == init_id),
            "cross-module constructor-entry inline should retain the original __init__ target for direct init codegen"
        );

        let module_constants = caller_typed.module_constants.clone();
        let caller = &mut caller_typed.callable_defs[caller_index];
        let split_stats = split_typed_constructor_hot_continuations(caller, &module_constants);
        assert_eq!(split_stats.clones.len(), 1);
        assert!(
            mark_indexed_field_accesses_for_field(caller, &module_constants, "value") >= 2,
            "original and hot-cloned continuations should contain the field load before scalarization"
        );
        let clone = split_stats.clones[0];
        let mut virtualization_plan =
            plan_typed_virtual_objects(caller, &module_constants, &constructor_field_bindings);
        let scalar_stats = lower_typed_virtual_fields_to_locals_with_plan(
            caller,
            &module_constants,
            &mut virtualization_plan,
        );
        assert_eq!(scalar_stats.seeded_objects, 1);
        assert_eq!(scalar_stats.scalar_slots, 1);
        assert_eq!(scalar_stats.rewritten_loads, 1);
        assert_eq!(
            getattrs_for_field_in_reachable_blocks(
                caller,
                clone.cloned_entry,
                &module_constants,
                "value",
            ),
            0,
            "cross-module constructor scalar should replace the hot cloned field load"
        );
    }

    #[test]
    fn typed_vararg_constructor_inline_expands_packed_rest_star_call() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class Record:\n    def __init__(self, *args):\n        self.value = args[0]\n\n\
def caller(value):\n    obj = Record(value)\n    return obj\n",
        )
        .expect("source should lower");
        let init_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "Record.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::PackedRest { start: 1 },
                            ],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let inline_stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::PackedRest { start: 1 },
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);

        #[derive(Default)]
        struct ConstructorCallShape {
            calls: usize,
            starred_args: usize,
            positional_counts: Vec<usize>,
        }
        struct ConstructorCallShapeCollector<'a> {
            module_constants: &'a [ConstantExpr],
            shape: ConstructorCallShape,
        }
        impl Visit<InstrTyped> for ConstructorCallShapeCollector<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr
                    && typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ConstructorCall,
                        self.module_constants,
                    )
                {
                    self.shape.calls += 1;
                    self.shape.starred_args += call
                        .args
                        .iter()
                        .filter(|arg| matches!(arg, CallArgPositional::Starred(_)))
                        .count();
                    self.shape.positional_counts.push(call.args.len());
                }
                expr.visit_children(self);
            }
        }

        let mut collector = ConstructorCallShapeCollector {
            module_constants: &module_constants,
            shape: ConstructorCallShape::default(),
        };
        collector.visit_fn(caller);
        assert!(
            collector.shape.calls > 0,
            "constructor-entry inline should retain the constructor_call"
        );
        assert_eq!(
            collector.shape.starred_args, 0,
            "packed vararg rest should become ordinary positional constructor_call arguments"
        );
        assert!(
            collector.shape.positional_counts.contains(&2),
            "constructor_call should receive the class plus the original user argument"
        );
    }

    #[test]
    fn typed_constructor_init_body_inline_exposes_non_straightline_field_stores() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class RangeLike:\n    def __init__(self, *args):\n        argc = len(args)\n        if argc == 1:\n            start = 0\n            stop = args[0]\n            step = 1\n        elif argc == 2:\n            start = args[0]\n            stop = args[1]\n            step = 1\n        else:\n            raise ValueError('bad argc')\n        self.start = start\n        self.stop = stop\n        self.step = step\n\n\
def caller(stop):\n    obj = RangeLike(0, stop)\n    return obj.stop\n",
        )
        .expect("source should lower");
        let init_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "RangeLike.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let call_id;
        {
            let caller = &mut typed.callable_defs[caller_index];
            call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::PackedRest { start: 1 },
                            ],
                        },
                    }],
                },
                |call| typed_expr_loads_name(call.func.as_ref(), "RangeLike"),
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let constructor_inline_stats = inline_typed_function_direct_call_stores(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::PackedRest { start: 1 },
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(constructor_inline_stats.rewritten_stores, 1);
        let constructor_init_plans =
            typed_constructor_init_plans_from_inline_stats_with_external_callees(
                &callee_module,
                &typed.module_constants,
                &HashMap::new(),
                &constructor_inline_stats,
            );
        assert_eq!(constructor_init_plans.len(), 1);
        annotate_constructor_init_plans_for_test(
            &mut typed.callable_defs[caller_index],
            &constructor_init_plans,
        );
        assert!(
            constructor_call_plan_sources(&typed.callable_defs[caller_index])
                .contains(&TypedConstructorInitPlanSource::InlinedConstructorEntry),
            "direct constructor_call should retain the constructor-init plan before body inlining"
        );

        let init_body_stats = inline_typed_constructor_init_bodies_with_external_callees(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &mut typed.module_constants,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(init_body_stats.inline_stats.rewritten_stores, 1);
        assert_eq!(init_body_stats.inlined_constructor_init_calls.len(), 1);
        assert!(
            init_body_stats
                .inline_stats
                .local_mappings
                .iter()
                .any(|mapping| mapping.callee == init_id
                    && mapping.callee_name == "args"
                    && mapping.caller_name.contains("typed_inline_varargs")),
            "packed *args should be materialized once into a temp for the inlined init body"
        );
        let vararg_location = init_body_stats
            .inline_stats
            .local_mappings
            .iter()
            .find(|mapping| {
                mapping.callee == init_id
                    && mapping.callee_name == "args"
                    && mapping.caller_name.contains("typed_inline_varargs")
            })
            .expect("packed *args mapping should exist")
            .caller_location;
        let tuple_simplifications = simplify_typed_virtual_tuple_ops(
            &mut typed.callable_defs[caller_index],
            &mut typed.module_constants,
        );
        assert!(
            tuple_simplifications >= 4,
            "known packed tuple length and indexed reads should simplify"
        );

        struct RuntimeLenCounter<'a> {
            module_constants: &'a [ConstantExpr],
            count: usize,
        }
        impl Visit<InstrTyped> for RuntimeLenCounter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr
                    && typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Len,
                        self.module_constants,
                    )
                {
                    self.count += 1;
                }
                expr.visit_children(self);
            }
        }

        let mut len_counter = RuntimeLenCounter {
            module_constants: &typed.module_constants,
            count: 0,
        };
        len_counter.visit_fn(&typed.callable_defs[caller_index]);
        assert_eq!(
            len_counter.count, 0,
            "len(args) on a known packed tuple should become a constant load"
        );

        struct TupleTempGetItemCounter {
            location: LocalLocation,
            count: usize,
        }
        impl Visit<InstrTyped> for TupleTempGetItemCounter {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::GetItem(op) = expr
                    && typed_instr_local_load_location(op.value.as_ref()) == Some(self.location)
                {
                    self.count += 1;
                }
                expr.visit_children(self);
            }
        }

        let mut getitem_counter = TupleTempGetItemCounter {
            location: vararg_location,
            count: 0,
        };
        getitem_counter.visit_fn(&typed.callable_defs[caller_index]);
        assert_eq!(
            getitem_counter.count, 0,
            "indexed reads from the packed tuple temp should become the original argument loads"
        );
        assert_eq!(
            typed.callable_defs[caller_index]
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .filter(|instr| {
                    matches!(
                        instr,
                        InstrTyped::Store(store)
                            if store.name.local_location() == Some(vararg_location)
                                && matches!(store.value.as_ref(), InstrTyped::Tuple(_))
                    )
                })
                .count(),
            0,
            "once packed tuple reads simplify away, the tuple materialization should disappear"
        );

        let remaining_argc_if_terms = typed.callable_defs[caller_index]
            .blocks
            .iter()
            .filter(|block| {
                matches!(
                    &block.term,
                    BlockTerm::IfTerm(if_term)
                        if !matches!(if_term.test, InstrTyped::DirectCallGuardTest(_))
                )
            })
            .count();
        assert_eq!(
            remaining_argc_if_terms, 0,
            "constant argc tests should fold to jumps and unreachable arms should be pruned"
        );
        let fields = init_body_stats
            .constructor_field_bindings
            .values()
            .flat_map(|bindings| {
                bindings
                    .fields
                    .iter()
                    .map(|field| field.field_name.as_str())
            })
            .collect::<HashSet<_>>();
        assert_eq!(
            fields,
            HashSet::from(["start", "stop", "step"]),
            "non-straightline init body should expose constructor field bindings"
        );
        assert!(
            constructor_call_plan_sources(&typed.callable_defs[caller_index]).contains(
                &TypedConstructorInitPlanSource::InlinedConstructorEntryWithInlinedInitBody
            ),
            "constructor_call should switch to allocation-only codegen once the init body is explicit"
        );
    }

    #[test]
    fn typed_constructor_init_body_inline_accepts_direct_callable_constructor_call() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class RangeLike:\n    def __init__(self, *args):\n        argc = len(args)\n        if argc == 1:\n            start = 0\n            stop = args[0]\n            step = 1\n        elif argc == 2:\n            start = args[0]\n            stop = args[1]\n            step = 1\n        else:\n            raise ValueError('bad argc')\n        self.start = start\n        self.stop = stop\n        self.step = step\n\n\
def caller(stop):\n    obj = RangeLike(0, stop)\n    return obj.stop\n",
        )
        .expect("source should lower");
        let init_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "RangeLike.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let call_id;
        {
            let caller = &mut typed.callable_defs[caller_index];
            call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::PackedRest { start: 1 },
                            ],
                        },
                    }],
                },
                |call| typed_expr_loads_name(call.func.as_ref(), "RangeLike"),
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let constructor_inline_stats = inline_typed_function_direct_call_stores(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::PackedRest { start: 1 },
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(constructor_inline_stats.rewritten_stores, 1);
        let constructor_init_plans =
            typed_constructor_init_plans_from_inline_stats_with_external_callees(
                &callee_module,
                &typed.module_constants,
                &HashMap::new(),
                &constructor_inline_stats,
            );
        assert_eq!(constructor_init_plans.len(), 1);

        let constructor_call_id = *constructor_init_plans
            .keys()
            .next()
            .expect("constructor entry inline should identify constructor_call");
        let direct_plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                constructor_call_id,
                TypedCallEmissionPlan::DirectCallable {
                    function_guard: TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    },
                },
            )]),
        };
        assert_eq!(
            lower_typed_function_call_emission_plans(
                &mut typed.callable_defs[caller_index],
                &direct_plans,
            )
            .expect("constructor call emission should lower"),
            1
        );
        annotate_constructor_init_plans_for_test(
            &mut typed.callable_defs[caller_index],
            &constructor_init_plans,
        );

        let init_body_stats = inline_typed_constructor_init_bodies_with_external_callees(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &mut typed.module_constants,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(init_body_stats.inline_stats.rewritten_stores, 1);
        assert_eq!(init_body_stats.inlined_constructor_init_calls.len(), 1);
        assert!(
            constructor_call_plan_sources(&typed.callable_defs[caller_index]).contains(
                &TypedConstructorInitPlanSource::InlinedConstructorEntryWithInlinedInitBody
            ),
            "direct constructor_call should switch to allocation-only codegen once the init body is explicit"
        );
    }

    #[test]
    fn typed_virtual_tuple_simplifier_keeps_escaping_packed_rest_materialization() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class Box:\n    def __init__(self, *args):\n        self.args = args\n\n\
def caller(value):\n    obj = Box(value)\n    return obj\n",
        )
        .expect("source should lower");
        let init_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "Box.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let call_id;
        {
            let caller = &mut typed.callable_defs[caller_index];
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::PackedRest { start: 1 },
                            ],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let inline_stats = inline_typed_function_direct_call_stores(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::PackedRest { start: 1 },
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);
        let constructor_init_plans =
            typed_constructor_init_plans_from_inline_stats_with_external_callees(
                &callee_module,
                &typed.module_constants,
                &HashMap::new(),
                &inline_stats,
            );
        annotate_constructor_init_plans_for_test(
            &mut typed.callable_defs[caller_index],
            &constructor_init_plans,
        );
        let init_body_stats = inline_typed_constructor_init_bodies_with_external_callees(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &mut typed.module_constants,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(init_body_stats.inlined_constructor_init_calls.len(), 1);
        assert_eq!(
            simplify_typed_virtual_tuple_ops(
                &mut typed.callable_defs[caller_index],
                &mut typed.module_constants,
            ),
            0,
            "escaping packed tuples should not simplify"
        );
        struct TupleCounter {
            count: usize,
        }
        impl Visit<InstrTyped> for TupleCounter {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                self.count += usize::from(matches!(expr, InstrTyped::Tuple(_)));
                expr.visit_children(self);
            }
        }

        let mut tuple_counter = TupleCounter { count: 0 };
        tuple_counter.visit_fn(&typed.callable_defs[caller_index]);
        assert_eq!(
            tuple_counter.count, 1,
            "a packed tuple stored onto an escaping object must remain materialized"
        );
    }

    #[test]
    fn typed_direct_constructor_entry_inline_exposes_init_body() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class RangeLike:\n    def __init__(self, *args):\n        argc = len(args)\n        if argc == 1:\n            start = 0\n            stop = args[0]\n            step = 1\n        elif argc == 2:\n            start = args[0]\n            stop = args[1]\n            step = 1\n        else:\n            raise ValueError('bad argc')\n        self.start = start\n        self.stop = stop\n        self.step = step\n\n\
def caller(stop):\n    obj = RangeLike(0, stop)\n    return obj.stop\n",
        )
        .expect("source should lower");
        let init_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "RangeLike.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let call_id = first_typed_call_instr_id(&typed.callable_defs[caller_index]);
        let direct_plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::DirectCallable {
                    function_guard: TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::PackedRest { start: 1 },
                            ],
                        },
                    },
                },
            )]),
        };
        assert_eq!(
            lower_typed_function_call_emission_plans(
                &mut typed.callable_defs[caller_index],
                &direct_plans,
            )
            .expect("outer constructor call should lower to a direct callable call"),
            1
        );

        let callee_module = typed.clone();
        let constructor_inline_stats = inline_typed_function_direct_call_stores(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::PackedRest { start: 1 },
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(constructor_inline_stats.rewritten_stores, 1);
        let constructor_init_plans =
            typed_constructor_init_plans_from_inline_stats_with_external_callees(
                &callee_module,
                &typed.module_constants,
                &HashMap::new(),
                &constructor_inline_stats,
            );
        assert_eq!(constructor_init_plans.len(), 1);
        annotate_constructor_init_plans_for_test(
            &mut typed.callable_defs[caller_index],
            &constructor_init_plans,
        );

        let init_body_stats = inline_typed_constructor_init_bodies_with_external_callees(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &mut typed.module_constants,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(init_body_stats.inline_stats.rewritten_stores, 1);
        assert_eq!(init_body_stats.inlined_constructor_init_calls.len(), 1);
    }

    #[test]
    fn typed_constructor_init_body_inline_skips_existing_field_bindings() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(value):\n    obj = Box(value)\n    return obj.value\n",
        )
        .expect("source should lower");
        let init_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "Box.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let inline_plan = plan_module_inlining(&summarize_module_escapes(&lowered.blockpy_module));
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let call_id;
        {
            let caller = &mut typed.callable_defs[caller_index];
            call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    }],
                },
                |call| typed_expr_loads_name(call.func.as_ref(), "Box"),
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let constructor_inline_stats = inline_typed_function_direct_call_stores(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(constructor_inline_stats.rewritten_stores, 1);
        let constructor_init_plans =
            typed_constructor_init_plans_from_inline_stats_with_external_callees(
                &callee_module,
                &typed.module_constants,
                &HashMap::new(),
                &constructor_inline_stats,
            );
        annotate_constructor_init_plans_for_test(
            &mut typed.callable_defs[caller_index],
            &constructor_init_plans,
        );
        let constructor_field_bindings = typed_constructor_field_bindings_from_inline_stats(
            &callee_module,
            &inline_plan,
            &typed.module_constants,
            &constructor_inline_stats,
        );
        assert_eq!(constructor_field_bindings.len(), 1);

        let skip_constructor_call_ids = constructor_field_bindings
            .keys()
            .copied()
            .collect::<HashSet<_>>();
        let init_body_stats = inline_typed_constructor_init_bodies_with_external_callees(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &mut typed.module_constants,
            &HashMap::new(),
            &skip_constructor_call_ids,
        );
        assert_eq!(init_body_stats.inline_stats.rewritten_stores, 0);
        assert!(init_body_stats.inlined_constructor_init_calls.is_empty());
    }

    #[test]
    fn typed_constructor_field_bindings_keep_generator_wrapper_state_as_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class ClosureGenerator:\n    def __init__(self, resume, preserved_values):\n        is_closed = False\n        self._resume_fn = resume\n        self._is_closed = is_closed\n        self._preserved_values = preserved_values\n\n\
def caller(resume, preserved_values):\n    gen = ClosureGenerator(resume, preserved_values)\n    return gen._preserved_values\n",
        )
        .expect("source should lower");
        let inline_plan = crate::passes::plan_module_inlining(
            &crate::passes::summarize_module_escapes(&lowered.blockpy_module),
        );
        let init_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "ClosureGenerator.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let call_id;
        {
            let caller = &mut typed.callable_defs[caller_index];
            call_id = first_typed_call_instr_id(caller);
            replace_first_typed_call_access(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                                TypedDirectCallArgSource::Provided(2),
                            ],
                        },
                    }],
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }
        let callee_module = typed.clone();
        let inline_stats = inline_typed_function_direct_call_stores(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                            TypedDirectCallArgSource::Provided(2),
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(inline_stats.rewritten_stores, 1);

        let constructor_field_bindings = typed_constructor_field_bindings_from_inline_stats(
            &callee_module,
            &inline_plan,
            &module_constants,
            &inline_stats,
        );
        assert!(
            constructor_field_bindings.is_empty(),
            "non-straightline wrapper init should use explicit init-body inlining"
        );
        let constructor_init_plans =
            typed_constructor_init_plans_from_inline_stats_with_external_callees(
                &callee_module,
                &module_constants,
                &HashMap::new(),
                &inline_stats,
            );
        assert_eq!(constructor_init_plans.len(), 1);
        annotate_constructor_init_plans_for_test(
            &mut typed.callable_defs[caller_index],
            &constructor_init_plans,
        );
        let init_body_stats = inline_typed_constructor_init_bodies_with_external_callees(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &mut typed.module_constants,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(init_body_stats.inline_stats.rewritten_stores, 1);
        assert_eq!(init_body_stats.inlined_constructor_init_calls.len(), 1);
        let fields = init_body_stats
            .constructor_field_bindings
            .values()
            .flat_map(|bindings| {
                bindings
                    .fields
                    .iter()
                    .map(|field| field.field_name.as_str())
            })
            .collect::<HashSet<_>>();
        assert_eq!(
            fields,
            HashSet::from(["_is_closed", "_preserved_values", "_resume_fn"]),
            "generator-wrapper constructor fields should remain visible to virtualization"
        );

        let constructor_field_bindings = init_body_stats.constructor_field_bindings;
        let module_constants = typed.module_constants.clone();
        let caller = &mut typed.callable_defs[caller_index];
        let split_stats = split_typed_constructor_hot_continuations(caller, &module_constants);
        assert_eq!(split_stats.clones.len(), 1);
        let generic_plan =
            plan_typed_virtual_objects(caller, &module_constants, &constructor_field_bindings);
        assert!(
            generic_plan.objects.is_empty(),
            "ordinary constructor virtualization should still require indexed-field evidence"
        );
        let mut trusted_sources = HashSet::from([call_id]);
        let trusted_inline_instances = inline_stats
            .inline_instance_sources
            .iter()
            .filter_map(|mapping| {
                trusted_sources
                    .contains(&mapping.source_instr_id)
                    .then_some(mapping.inline_instance)
            })
            .collect::<HashSet<_>>();
        trusted_sources.extend(inline_stats.instr_id_mappings.iter().filter_map(|mapping| {
            (trusted_inline_instances.contains(&mapping.inline_instance)
                && constructor_field_bindings.contains_key(&mapping.caller_instr_id))
            .then_some(mapping.caller_instr_id)
        }));
        for mapping in &split_stats.instr_id_mappings {
            if trusted_sources.contains(&mapping.callee_instr_id) {
                trusted_sources.insert(mapping.caller_instr_id);
            }
        }
        let mut fully_virtual_plan = plan_typed_fully_virtual_objects(
            caller,
            &module_constants,
            &constructor_field_bindings,
            &trusted_sources,
        );
        assert_eq!(
            fully_virtual_plan.objects.len(),
            1,
            "trusted generated wrapper constructors should virtualize without waiting for indexed-field replay"
        );
        assert!(fully_virtual_plan.materialization_boundaries().is_empty());
        assert!(
            getattrs_for_field_in_reachable_blocks(
                caller,
                split_stats.clones[0].cloned_entry,
                &module_constants,
                "_preserved_values",
            ) > 0,
            "the hot cloned continuation should still read wrapper state before lowering"
        );
        let stats = lower_typed_fully_virtual_objects_to_locals_with_plan(
            caller,
            &module_constants,
            &mut fully_virtual_plan,
        );
        assert!(stats.changed());
        assert_eq!(stats.field_lowering.seeded_objects, 1);
        assert!(
            stats.virtualization.removed_materializations >= 1,
            "trusted generated wrappers should lower to locals without leaving the hot allocation alive"
        );
        assert_eq!(
            getattrs_for_field_in_reachable_blocks(
                caller,
                split_stats.clones[0].cloned_entry,
                &module_constants,
                "_preserved_values",
            ),
            0,
            "the hot cloned continuation should use the wrapper-state local after lowering"
        );
    }

    #[test]
    fn typed_field_scalarization_preserves_iterator_state_on_hot_loop_backedge() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __init__(self, current, stop, step):\n        self.current = current\n        self.stop = stop\n        self.step = step\n\n    def __next__(self):\n        current = self.current\n        stop = self.stop\n        step = self.step\n        if current >= stop:\n            raise StopIteration\n        self.current = current + step\n        return current\n\n\
def caller(i):\n    it = IterRange(0, i, 1)\n    total = 0\n    while True:\n        try:\n            value = next(it)\n        except StopIteration:\n            return total\n        total += value\n",
        )
        .expect("source should lower");
        let inline_plan = crate::passes::plan_module_inlining(
            &crate::passes::summarize_module_escapes(&lowered.blockpy_module),
        );
        let init_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__init__");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered.blockpy_module, init_id)
                .expect("class lowering should add a constructor entry");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let constructor_call_id;
        {
            let caller = typed_function_by_qualname_mut(&mut typed, "caller");
            constructor_call_id = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedCallable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: constructor_entry_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                                TypedDirectCallArgSource::Provided(2),
                                TypedDirectCallArgSource::Provided(3),
                            ],
                        },
                    }],
                },
                |call| typed_expr_loads_name(call.func.as_ref(), "IterRange"),
            );
            let _ = replace_first_typed_call_access_where(
                caller,
                TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                    runtime_name: RuntimeName::Next,
                    method_name: "__next__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: next_id,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "__main__".to_string(),
                            qualname: "IterRange".to_string(),
                        },
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
                |call| {
                    typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        &module_constants,
                    )
                },
            );
            lower_typed_function_call_access_plan_instrs(caller);
        }

        let callee_module = typed.clone();
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let constructor_inline_stats = inline_typed_function_direct_call_stores(
            &mut typed.callable_defs[caller_index],
            &callee_module,
            &HashMap::new(),
            &HashMap::from([(
                constructor_call_id,
                vec![(
                    constructor_entry_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                            TypedDirectCallArgSource::Provided(2),
                            TypedDirectCallArgSource::Provided(3),
                        ],
                    },
                )],
            )]),
        );
        assert_eq!(constructor_inline_stats.rewritten_stores, 1);
        let mut constructor_hot_state_cleanup_labels = constructor_inline_stats
            .hot_state_cleanup_labels
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        assert!(
            !constructor_hot_state_cleanup_labels.is_empty(),
            "constructor-entry inline should mark hot-state cleanup for continuation splitting"
        );
        let mut constructor_field_bindings = typed_constructor_field_bindings_from_inline_stats(
            &callee_module,
            &inline_plan,
            &module_constants,
            &constructor_inline_stats,
        );
        assert_eq!(constructor_field_bindings.len(), 1);
        let constructor_init_plans =
            typed_constructor_init_plans_from_inline_stats_with_external_callees(
                &callee_module,
                &typed.module_constants,
                &HashMap::new(),
                &constructor_inline_stats,
            );
        annotate_constructor_init_plans_for_test(
            &mut typed.callable_defs[caller_index],
            &constructor_init_plans,
        );
        let init_body_stats = {
            let (module_constants, callable_defs) =
                (&mut typed.module_constants, &mut typed.callable_defs);
            inline_typed_constructor_init_bodies_with_external_callees(
                &mut callable_defs[caller_index],
                &callee_module,
                module_constants,
                &HashMap::new(),
                &HashSet::new(),
            )
        };
        assert_eq!(init_body_stats.inline_stats.rewritten_stores, 1);
        constructor_field_bindings.extend(init_body_stats.constructor_field_bindings);
        let module_constants = typed.module_constants.clone();
        let caller = &mut typed.callable_defs[caller_index];

        let constructor_split =
            split_typed_constructor_hot_continuations(caller, &module_constants);
        assert_eq!(constructor_split.clones.len(), 1);
        let mut hot_continuation_clones = constructor_split.clones.clone();
        for (source, target) in &constructor_split.label_mappings {
            if constructor_hot_state_cleanup_labels.contains(source) {
                constructor_hot_state_cleanup_labels.insert(*target);
            }
        }
        let next_direct_calls =
            typed_runtime_name_call_instr_ids(caller, RuntimeName::Next, &module_constants)
                .into_iter()
                .map(|instr_id| {
                    (
                        instr_id,
                        vec![(
                            next_id,
                            TypedDirectCallArgPlan {
                                sources: vec![TypedDirectCallArgSource::Provided(0)],
                            },
                        )],
                    )
                })
                .collect::<HashMap<_, _>>();
        assert!(
            next_direct_calls.len() >= 2,
            "constructor continuation split should leave original and hot-cloned next() calls"
        );
        let next_inline_stats = inline_typed_function_direct_call_stores(
            caller,
            &callee_module,
            &HashMap::new(),
            &next_direct_calls,
        );
        assert!(
            next_inline_stats.rewritten_stores >= 1,
            "hot next() call should inline after constructor continuation splitting"
        );
        let mut hot_state_cleanup_labels = next_inline_stats
            .hot_state_cleanup_labels
            .iter()
            .copied()
            .chain(constructor_hot_state_cleanup_labels)
            .collect::<HashSet<_>>();
        assert!(
            !hot_state_cleanup_labels.is_empty(),
            "runtime protocol next() inline should mark hot-state cleanup for continuation splitting"
        );
        let constructor_split =
            split_typed_constructor_hot_continuations(caller, &module_constants);
        hot_continuation_clones.extend(constructor_split.clones.iter().copied());
        for (source, target) in &constructor_split.label_mappings {
            if hot_state_cleanup_labels.contains(source) {
                hot_state_cleanup_labels.insert(*target);
            }
        }
        let alias_split = split_typed_alias_hot_continuations(caller, &module_constants);
        hot_continuation_clones.extend(alias_split.clones.iter().copied());
        for (source, target) in &alias_split.label_mappings {
            if hot_state_cleanup_labels.contains(source) {
                hot_state_cleanup_labels.insert(*target);
            }
        }
        let cleanup_split = split_typed_inline_cleanup_hot_continuations_for_labels(
            caller,
            &hot_state_cleanup_labels,
        );
        hot_continuation_clones.extend(cleanup_split.clones.iter().copied());
        assert!(
            !hot_continuation_clones.is_empty(),
            "pending constructor or next() hot state should split the loop continuation"
        );
        for field in ["current", "stop", "step"] {
            assert!(
                mark_indexed_field_accesses_for_field(caller, &module_constants, field) > 0,
                "test should contain {field} field loads before scalarization"
            );
        }
        assert!(
            rewrite_typed_stop_iteration_raises_to_handler_jumps(caller, &module_constants) >= 1
        );
        let labels = typed_block_indices_by_label(caller);
        let hot_loop_clones = hot_continuation_clones
            .iter()
            .copied()
            .filter(|clone| labels.contains_key(&clone.cloned_entry))
            .filter(|clone| {
                getattrs_for_field_in_hot_reachable_blocks(
                    caller,
                    clone.cloned_entry,
                    &module_constants,
                    "current",
                ) > 0
            })
            .collect::<Vec<_>>();
        assert!(
            !hot_loop_clones.is_empty(),
            "hot-state split should clone the hot loop containing __next__ field loads"
        );

        let mut virtualization_plan =
            plan_typed_virtual_objects(caller, &module_constants, &constructor_field_bindings);
        let removable_plan = virtualization_plan
            .objects
            .first()
            .cloned()
            .expect("iterator hot path should discover a removable virtual object");
        let mut escaping_caller = caller.clone();
        let escaping_return = escaping_caller
            .blocks
            .iter_mut()
            .find(|block| block.label == removable_plan.materialization_block)
            .expect("virtual constructor region should contain its allocation block");
        escaping_return.term = BlockTerm::Return(typed_load_temp(&removable_plan.root));
        let mut escaping_plan = plan_typed_virtual_objects(
            &escaping_caller,
            &module_constants,
            &constructor_field_bindings,
        );
        assert!(
            escaping_plan.objects.is_empty(),
            "returning the object should keep it out of the removable-object subset"
        );
        assert_eq!(
            escaping_plan.materializing_objects.len(),
            0,
            "a return escape after inline-temp cleanup cannot use late materialization safely"
        );
        let escaping_scalar_stats = lower_typed_virtual_fields_to_locals_with_plan(
            &mut escaping_caller,
            &module_constants,
            &mut escaping_plan,
        );
        assert_eq!(escaping_scalar_stats.rewritten_loads, 0);
        let escaping_materialization_stats = materialize_typed_virtual_return_boundaries_with_plan(
            &mut escaping_caller,
            &module_constants,
            &escaping_plan,
        );
        assert_eq!(escaping_materialization_stats.materialized_objects, 0);
        let mut materializing_store_caller = caller.clone();
        let materializing_store_block = materializing_store_caller
            .blocks
            .iter_mut()
            .find(|block| block.label == removable_plan.materialization_block)
            .expect("virtual constructor plan should retain its allocation block");
        materializing_store_block.body.insert(
            removable_plan.materialization_index + 1,
            Store::new(
                ResolvedName {
                    id: "sink".to_string().into(),
                    location: NameLocation::GlobalName,
                },
                typed_load_temp(&removable_plan.root),
            )
            .with_meta(Meta::synthetic())
            .into(),
        );
        let mut materializing_store_plan = plan_typed_virtual_objects(
            &materializing_store_caller,
            &module_constants,
            &constructor_field_bindings,
        );
        assert_eq!(
            materializing_store_plan.materializing_objects.len(),
            1,
            "the escaping store should still be recognized as a materialization boundary"
        );
        let materializing_store_scalar_stats = lower_typed_virtual_fields_to_locals_with_plan(
            &mut materializing_store_caller,
            &module_constants,
            &mut materializing_store_plan,
        );
        assert_eq!(
            materializing_store_scalar_stats.rewritten_loads, 0,
            "field accesses after an immediate global escape must remain observable"
        );
        let materializing_store_stats = materialize_typed_virtual_store_boundaries_with_plan(
            &mut materializing_store_caller,
            &module_constants,
            &materializing_store_plan,
        );
        assert_eq!(
            materializing_store_stats.materialized_objects, 0,
            "the original concrete allocation should stay in place when explicit init values are not yet available at the boundary"
        );
        let mut escaping_store_caller = caller.clone();
        let (escaping_block_index, escaping_store_index, escaping_root) = escaping_store_caller
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                typed_virtual_constructor_plan_covers_block(&removable_plan, block.label)
            })
            .find_map(|(block_index, block)| {
                let mut seen_fields = 0_u8;
                for (instr_index, instr) in block.body.iter().enumerate() {
                    let InstrTyped::Store(store) = instr else {
                        continue;
                    };
                    let InstrTyped::GetAttrTyped(get_attr) = store.value.as_ref() else {
                        continue;
                    };
                    let field_bit =
                        match typed_constant_string(get_attr.attr.as_ref(), &module_constants) {
                            Some("current") => 0b001,
                            Some("stop") => 0b010,
                            Some("step") => 0b100,
                            _ => continue,
                        };
                    let InstrTyped::Load(receiver) = get_attr.value.as_ref() else {
                        continue;
                    };
                    let Some(location) = receiver.name.local_location() else {
                        continue;
                    };
                    if !removable_plan.virtual_locations.contains(&location) {
                        continue;
                    }
                    seen_fields |= field_bit;
                    if seen_fields == 0b111 {
                        return Some((block_index, instr_index + 1, receiver.name.clone()));
                    }
                }
                None
            })
            .expect("iterator hot block should read current, stop, and step before escape");
        escaping_store_caller.blocks[escaping_block_index]
            .body
            .insert(
                escaping_store_index,
                Store::new(
                    ResolvedName {
                        id: "sink".to_string().into(),
                        location: NameLocation::GlobalName,
                    },
                    typed_load_temp(&escaping_root),
                )
                .with_meta(Meta::synthetic())
                .into(),
            );
        let mut escaping_store_plan = plan_typed_virtual_objects(
            &escaping_store_caller,
            &module_constants,
            &constructor_field_bindings,
        );
        assert_eq!(
            escaping_store_plan.materializing_objects.len(),
            0,
            "an escaping store after inline-temp cleanup cannot use late materialization safely"
        );
        let escaping_store_scalar_stats = lower_typed_virtual_fields_to_locals_with_plan(
            &mut escaping_store_caller,
            &module_constants,
            &mut escaping_store_plan,
        );
        assert_eq!(
            escaping_store_scalar_stats.rewritten_loads, 0,
            "a global escape on a loop backedge must invalidate fields on later iterations"
        );
        let escaping_store_materialization_stats =
            materialize_typed_virtual_store_boundaries_with_plan(
                &mut escaping_store_caller,
                &module_constants,
                &escaping_store_plan,
            );
        assert_eq!(escaping_store_materialization_stats.materialized_objects, 0);
        let mut escaping_body_caller = caller.clone();
        let escaping_body_block = escaping_body_caller
            .blocks
            .iter_mut()
            .find(|block| block.label == removable_plan.materialization_block)
            .expect("virtual constructor plan should retain its allocation block");
        let escaping_body_index = removable_plan.materialization_index + 1;
        escaping_body_block.body.insert(
            escaping_body_index,
            InstrTyped::Tuple(Tuple::new(vec![typed_load_temp(&removable_plan.root)])),
        );
        let mut escaping_body_plan = plan_typed_virtual_objects(
            &escaping_body_caller,
            &module_constants,
            &constructor_field_bindings,
        );
        assert_eq!(
            escaping_body_plan.materializing_objects.len(),
            1,
            "a root-valued unsupported body use before inline-temp cleanup should allow explicit materialization"
        );
        let escaping_body_scalar_stats = lower_typed_virtual_fields_to_locals_with_plan(
            &mut escaping_body_caller,
            &module_constants,
            &mut escaping_body_plan,
        );
        assert_eq!(escaping_body_scalar_stats.rewritten_loads, 0);
        let escaping_body_materialization_stats =
            materialize_typed_virtual_body_boundaries_with_plan(
                &mut escaping_body_caller,
                &module_constants,
                &escaping_body_plan,
            );
        assert_eq!(
            escaping_body_materialization_stats.materialized_objects, 0,
            "unsupported root-valued body uses before explicit init values exist should keep the concrete allocation"
        );
        let mut deopt_body_caller = caller.clone();
        let deopt_body_block = deopt_body_caller
            .blocks
            .iter_mut()
            .find(|block| block.label == removable_plan.materialization_block)
            .expect("virtual constructor plan should retain its allocation block");
        let mut deopt_guard = TypedDirectCallGuardTest::new(
            typed_load_temp(&removable_plan.root),
            TypedDirectCallGuardTestKind::RuntimeFunctionId {
                function_id: caller.function_id,
            },
        );
        deopt_guard.extra.set_guard_miss_deopt_enabled(true);
        deopt_body_block.body.insert(
            removable_plan.materialization_index + 1,
            InstrTyped::DirectCallGuardTest(deopt_guard.with_meta(Meta::synthetic())),
        );
        let mut deopt_body_plan = plan_typed_virtual_objects(
            &deopt_body_caller,
            &module_constants,
            &constructor_field_bindings,
        );
        assert_eq!(
            deopt_body_plan.materialization_boundaries()[0].kind,
            TypedVirtualBoundaryKind::DeoptResumeUse,
            "deopt-enabled guards should become explicit virtual materialization boundaries"
        );
        let deopt_body_scalar_stats = lower_typed_virtual_fields_to_locals_with_plan(
            &mut deopt_body_caller,
            &module_constants,
            &mut deopt_body_plan,
        );
        assert!(
            deopt_body_scalar_stats.rewritten_loads >= 3,
            "field lowering should still scalarize uses before the explicit deopt materialization boundary"
        );
        let deopt_body_materialization_stats = materialize_typed_virtual_body_boundaries_with_plan(
            &mut deopt_body_caller,
            &module_constants,
            &deopt_body_plan,
        );
        assert_eq!(
            deopt_body_materialization_stats.materialized_objects, 0,
            "deopt-enabled guards before explicit init values exist should keep the concrete allocation"
        );
        assert!(
            virtualization_plan
                .objects
                .iter()
                .flat_map(|object| object.field_bindings.fields.iter())
                .all(|field| field.scalar.is_none()),
            "virtualization planning should run before virtual field locals are allocated"
        );
        assert!(
            virtualization_plan
                .field_states
                .as_ref()
                .is_some_and(|states| {
                    !states.block_in.is_empty()
                        && !states.block_out.is_empty()
                        && !states.edge_out.is_empty()
                }),
            "virtualization planning should publish explicit block and edge virtual field state before lowering"
        );
        assert!(
            virtualization_plan
                .field_states
                .as_ref()
                .is_some_and(|states| {
                    states.block_in.values().all(|state| {
                        state
                            .fields
                            .values()
                            .all(|value| !value.id_str().contains("_dp_vfield"))
                    })
                }),
            "pre-lowering virtual field state should describe the object-shaped program before block params exist"
        );
        let scalar_stats = lower_typed_virtual_fields_to_locals_with_plan(
            caller,
            &module_constants,
            &mut virtualization_plan,
        );
        assert!(
            virtualization_plan
                .field_states
                .as_ref()
                .is_some_and(|states| {
                    !states.block_in.is_empty()
                        && !states.block_out.is_empty()
                        && !states.edge_out.is_empty()
                }),
            "virtual-to-locals lowering should retain explicit block and edge virtual field state"
        );
        assert_eq!(scalar_stats.seeded_objects, 1);
        assert_eq!(scalar_stats.scalar_slots, 3);
        assert_eq!(
            scalar_stats.inserted_scalar_stores, 4,
            "explicit inlined init-body stores plus the loop-carried current update should write scalars once each, without eager stores at allocation"
        );
        assert!(
            scalar_stats.inserted_block_args >= scalar_stats.inserted_block_params,
            "each synthesized virtual field param should receive explicit edge args: {scalar_stats:?}"
        );
        let virtual_field_param_names = caller
            .blocks
            .iter()
            .flat_map(|block| block.params.iter())
            .filter(|param| param.name.contains("_dp_vfield"))
            .map(|param| param.name.clone())
            .collect::<HashSet<_>>();
        if !virtual_field_param_names.is_empty() {
            assert!(
                caller.blocks.iter().any(|block| {
                    matches!(
                        &block.term,
                        BlockTerm::Jump(edge)
                            if edge.args.iter().any(|arg| {
                                matches!(
                                    arg,
                                    BlockArg::Name(name) if virtual_field_param_names.contains(name)
                                )
                            })
                    )
                }),
                "loop-carried virtual field params should be fed by explicit jump edge args"
            );
            assert!(
                virtualization_plan
                    .field_states
                    .as_ref()
                    .is_some_and(|states| {
                        states.block_in.values().any(|state| {
                            state
                                .fields
                                .values()
                                .any(|value| value.id_str().contains("_dp_vfield"))
                        })
                    }),
                "the analyzed virtual field state should re-enter loop headers through the synthesized params"
            );
        }
        assert!(
            scalar_stats.rewritten_loads >= 3,
            "current/stop/step loads in the hot iterator loop should scalarize: {scalar_stats:?}"
        );
        assert!(
            hot_loop_clones.iter().any(|clone| {
                ["current", "stop", "step"].iter().all(|field| {
                    getattrs_for_field_in_hot_reachable_blocks(
                        caller,
                        clone.cloned_entry,
                        &module_constants,
                        field,
                    ) == 0
                })
            }),
            "at least one hot iterator loop clone should use scalar state for current/stop/step"
        );
        assert!(
            virtualization_plan
                .objects
                .iter()
                .all(|plan| plan.object_id == TypedVirtualObjectId(plan.source.index())),
            "virtual object ids should remain tied to the stable allocation source"
        );
        let virtual_plan = virtualization_plan
            .objects
            .iter()
            .find(|plan| {
                constructor_call_stores_in_virtual_plan(caller, &module_constants, plan) > 0
                    && direct_call_guards_in_virtual_plan(caller, plan) > 0
            })
            .cloned()
            .expect("hot cloned iterator path should have a virtual constructor candidate");
        assert_eq!(
            virtual_plan
                .field_bindings
                .fields
                .iter()
                .map(|field| field.field_name.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["current", "stop", "step"]),
            "the virtualization artifact should carry constructor field bindings"
        );
        assert!(
            virtual_plan
                .field_bindings
                .fields
                .iter()
                .all(|field| field.scalar.is_some()),
            "virtual-to-locals lowering should populate field locals on the analysis artifact"
        );
        assert!(
            setattrs_for_field_in_virtual_plan(caller, &module_constants, &virtual_plan, "current")
                > 0,
            "before virtualizing, the hot iterator clone should still store current on the object"
        );
        let virtual_stats = virtualize_typed_hot_constructor_plans(
            caller,
            &module_constants,
            &virtualization_plan.objects,
        );
        assert!(
            virtual_stats.removed_materializations >= 1,
            "virtual constructor pass should remove the IterRange allocation: {virtual_stats:?}"
        );
        assert!(
            virtual_stats.removed_field_stores >= 1,
            "virtual constructor pass should remove scalarized field stores on the virtual object: {virtual_stats:?}"
        );
        assert!(
            virtual_stats.removed_guards >= 1,
            "virtual constructor pass should remove redundant method guards on the virtual object: {virtual_stats:?}"
        );
        assert_eq!(
            constructor_call_stores_in_virtual_plan(caller, &module_constants, &virtual_plan),
            0,
            "the virtualized hot iterator path should not materialize IterRange"
        );
        assert_eq!(
            setattrs_for_field_in_virtual_plan(caller, &module_constants, &virtual_plan, "current"),
            0,
            "the virtualized hot iterator path should update current through scalar state only"
        );
        assert_eq!(
            direct_call_guards_in_virtual_plan(caller, &virtual_plan),
            0,
            "the virtualized hot iterator path should not need object-identity method guards"
        );
    }

    #[test]
    fn typed_stop_iteration_raise_rewrite_jumps_to_matching_handler_after_inlining() {
        let mut typed = inline_next_protocol_call(
            "class IterRange:\n    def __next__(self):\n        if self.current >= self.stop:\n            raise StopIteration\n        return self.current\n\n\
def caller(it):\n    try:\n        value = next(it)\n    except StopIteration:\n        return 0\n    return value\n",
        );
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");

        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 1);
        assert_eq!(
            rewrite_typed_stop_iteration_raises_to_handler_jumps(caller, &module_constants),
            1
        );
        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 0);
        assert!(caller.blocks.iter().any(|block| {
            matches!(
                &block.term,
                BlockTerm::Jump(edge) if edge.args.iter().any(|arg| matches!(arg, BlockArg::None))
            )
        }));
    }

    #[test]
    fn typed_stop_iteration_raise_rewrite_accepts_guarded_exception_match() {
        let mut typed = inline_next_protocol_call(
            "class IterRange:\n    def __next__(self):\n        if self.current >= self.stop:\n            raise StopIteration\n        return self.current\n\n\
def caller(it):\n    try:\n        value = next(it)\n    except StopIteration:\n        return 0\n    return value\n",
        );
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let exception_matches_id = typed_runtime_name_call_instr_ids(
            caller,
            RuntimeName::ExceptionMatches,
            &module_constants,
        )
        .into_iter()
        .next()
        .expect("inlined try handler should call exception_matches");
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                exception_matches_id,
                TypedCallEmissionPlan::Callable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: RuntimeFunctionId::from_raw_parts(0, 1),
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    }],
                },
            )]),
        };
        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("exception_matches call emission should lower"),
            1
        );

        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 1);
        assert_eq!(
            rewrite_typed_stop_iteration_raises_to_handler_jumps(caller, &module_constants),
            1
        );
        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 0);
    }

    #[test]
    fn typed_stop_iteration_raise_rewrite_accepts_direct_exception_match() {
        let mut typed = inline_next_protocol_call(
            "class IterRange:\n    def __next__(self):\n        if self.current >= self.stop:\n            raise StopIteration\n        return self.current\n\n\\
def caller(it):\n    try:\n        value = next(it)\n    except StopIteration:\n        return 0\n    return value\n",
        );
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let exception_matches_id = typed_runtime_name_call_instr_ids(
            caller,
            RuntimeName::ExceptionMatches,
            &module_constants,
        )
        .into_iter()
        .next()
        .expect("inlined try handler should call exception_matches");
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                exception_matches_id,
                TypedCallEmissionPlan::DirectCallable {
                    function_guard: TypedDirectFunctionCallGuard {
                        function_id: RuntimeFunctionId::from_raw_parts(0, 1),
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![
                                TypedDirectCallArgSource::Provided(0),
                                TypedDirectCallArgSource::Provided(1),
                            ],
                        },
                    },
                },
            )]),
        };
        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("exception_matches direct-call emission should lower"),
            1
        );

        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 1);
        assert_eq!(
            rewrite_typed_stop_iteration_raises_to_handler_jumps(caller, &module_constants),
            1
        );
        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 0);
    }

    #[test]
    fn typed_stop_iteration_raise_rewrite_accepts_zero_arg_constructor() {
        let mut typed = inline_next_protocol_call(
            "class IterRange:\n    def __next__(self):\n        if self.current >= self.stop:\n            raise StopIteration()\n        return self.current\n\n\\
def caller(it):\n    try:\n        value = next(it)\n    except StopIteration:\n        return 0\n    return value\n",
        );
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");

        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 1);
        assert_eq!(
            rewrite_typed_stop_iteration_raises_to_handler_jumps(caller, &module_constants),
            1
        );
        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 0);
    }

    #[test]
    fn typed_stop_iteration_raise_rewrite_keeps_value_constructor() {
        let mut typed = inline_next_protocol_call(
            "class IterRange:\n    def __next__(self):\n        if self.current >= self.stop:\n            raise StopIteration(41)\n        return self.current\n\n\\
def caller(it):\n    try:\n        value = next(it)\n    except StopIteration:\n        return 0\n    return value\n",
        );
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");

        assert_eq!(
            rewrite_typed_stop_iteration_raises_to_handler_jumps(caller, &module_constants),
            0,
            "a value-bearing StopIteration constructor may evaluate observable arguments",
        );
        assert!(
            caller
                .blocks
                .iter()
                .any(|block| matches!(block.term, BlockTerm::Raise(_))),
            "the value-bearing raise should remain in the CFG",
        );
    }

    #[test]
    fn typed_stop_iteration_raise_rewrite_keeps_observed_exception_object() {
        let mut typed = inline_next_protocol_call(
            "class IterRange:\n    def __next__(self):\n        if self.current:\n            raise StopIteration\n        return 1\n\n\
def caller(it):\n    try:\n        value = next(it)\n    except StopIteration as exc:\n        return exc\n    return value\n",
        );
        let module_constants = typed.module_constants.clone();
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");

        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 1);
        assert_eq!(
            rewrite_typed_stop_iteration_raises_to_handler_jumps(caller, &module_constants),
            0
        );
        assert_eq!(stop_iteration_raise_terms(caller, &module_constants), 1);
    }

    #[test]
    fn lowers_typed_call_emission_plan_to_guarded_callable_instr() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def add(a):\n    return a\n\n\
def caller(a):\n    return add(a)\n",
        )
        .expect("source should lower");
        let add_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "add");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::Callable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: add_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed call emission plan should lower"),
            1
        );
        assert_eq!(
            lower_typed_function_call_access_plan_instrs(caller),
            0,
            "mechanical call emission lowering should not round-trip through access plans"
        );

        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 0);
        assert_eq!(counter.guarded_callable_calls, 1);
    }

    #[test]
    fn lowers_typed_call_emission_plan_to_direct_callable_instr() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def add(a):\n    return a\n\n\
def caller(a):\n    return add(a)\n",
        )
        .expect("source should lower");
        let add_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "add");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::DirectCallable {
                    function_guard: TypedDirectFunctionCallGuard {
                        function_id: add_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    },
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed direct callable emission plan should lower"),
            1
        );

        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 0);
        assert_eq!(counter.direct_callable_calls, 1);
        assert_eq!(counter.guarded_callable_calls, 0);
    }

    #[test]
    fn lowers_typed_call_emission_plan_with_multiple_function_guards() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(a):\n    return callable(a)\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let direct_target = RuntimeFunctionId::from_raw_parts(0, 7);
        let other_direct_target = RuntimeFunctionId::from_raw_parts(0, 8);
        let arg_plan = TypedDirectCallArgPlan {
            sources: vec![TypedDirectCallArgSource::Provided(0)],
        };
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::Callable {
                    function_guards: vec![
                        TypedDirectFunctionCallGuard {
                            function_id: direct_target,
                            arg_plan: arg_plan.clone(),
                        },
                        TypedDirectFunctionCallGuard {
                            function_id: other_direct_target,
                            arg_plan,
                        },
                    ],
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed callable emission plan should lower"),
            1
        );
        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.guarded_callable_calls, 1);
    }

    #[test]
    fn lowers_typed_call_emission_plan_to_guarded_method_instr() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(box):\n    return box.get(1)\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let target = RuntimeFunctionId::from_raw_parts(0, 9);
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::Method {
                    method_name: "get".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: target,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "test".to_string(),
                            qualname: "Box".to_string(),
                        },
                        type_version: 11,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed method emission plan should lower"),
            1
        );
        assert_eq!(
            lower_typed_function_call_access_plan_instrs(caller),
            0,
            "mechanical method emission lowering should not round-trip through access plans"
        );

        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 0);
        assert_eq!(counter.guarded_method_calls, 1);
    }

    #[test]
    fn linearized_method_emission_plan_leaves_generic_call_in_place() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(box):\n    return box.get(1)\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        linearize_typed_function_expressions(caller)
            .expect("typed expression linearization should succeed");
        let call_id = first_typed_call_instr_id(caller);
        let target = RuntimeFunctionId::from_raw_parts(0, 9);
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::Method {
                    method_name: "get".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: target,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "test".to_string(),
                            qualname: "Box".to_string(),
                        },
                        type_version: 11,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("linearized method emission should fall back locally"),
            0
        );
        validate_typed_function_call_access_plans(caller)
            .expect("linearized method fallback should remain valid");

        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 1);
        assert_eq!(counter.guarded_method_calls, 0);
    }

    #[test]
    fn linearization_demotes_guarded_method_access_when_getattr_target_is_lifted() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(box):\n    return box.get(1)\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedMethod {
                method_name: "get".to_string(),
                method_guards: vec![TypedDirectMethodCallGuard {
                    function_id: RuntimeFunctionId::from_raw_parts(0, 9),
                    owner_type_ref: TypedAttrOwnerRef::TypeKey {
                        module_name: "test".to_string(),
                        qualname: "Box".to_string(),
                    },
                    type_version: 11,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                }],
            },
        );

        linearize_typed_function_expressions(caller)
            .expect("typed expression linearization should succeed");
        validate_typed_function_call_access_plans(caller)
            .expect("linearization should keep guarded method invariants valid");

        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 1);
        assert_eq!(counter.guarded_method_calls, 0);
        assert!(caller.blocks.iter().all(|block| {
            block.body.iter().all(|instr| {
                !matches!(
                    instr,
                    InstrTyped::CallTyped(call)
                        if matches!(call.access, TypedCallAccessPlan::GuardedMethod { .. })
                )
            })
        }));
    }

    #[test]
    fn lowers_typed_call_emission_plan_to_runtime_protocol_access_plan() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    return next(it)\n",
        )
        .expect("source should lower");
        let next_id =
            blockpy_function_id_by_qualname(&lowered.blockpy_module, "IterRange.__next__");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::RuntimeProtocolMethod {
                    runtime_name: RuntimeName::Next,
                    method_name: "__next__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: next_id,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "__main__".to_string(),
                            qualname: "IterRange".to_string(),
                        },
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed runtime protocol emission plan should lower"),
            1
        );
        validate_typed_function_call_access_plans(caller)
            .expect("runtime protocol access plan should validate");

        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 1);
        assert_eq!(counter.guarded_method_calls, 0);
    }

    #[test]
    fn empty_typed_call_emission_plan_leaves_generic_call_in_place() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(box):\n    return box.get(1)\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::Method {
                    method_name: "get".to_string(),
                    method_guards: Vec::new(),
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("empty typed emission plan should be a local fallback"),
            0
        );
        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 1);
        assert_eq!(counter.guarded_method_calls, 0);
    }

    #[test]
    fn rejects_guarded_method_typed_call_access_without_getattr_target() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(fn):\n    return fn()\n",
        )
        .expect("source should lower");
        let caller_id = blockpy_function_id_by_qualname(&lowered.blockpy_module, "caller");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedMethod {
                method_name: "__call__".to_string(),
                method_guards: vec![TypedDirectMethodCallGuard {
                    function_id: caller_id,
                    owner_type_ref: TypedAttrOwnerRef::TypeKey {
                        module_name: "__main__".to_string(),
                        qualname: "Caller".to_string(),
                    },
                    type_version: 1,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                }],
            },
        );

        let err = validate_typed_function_call_access_plans(caller)
            .expect_err("guarded method without GetAttr should be rejected");
        assert!(err.contains("requires a GetAttr call target"));
    }

    #[test]
    fn direct_call_guard_test_is_first_class_typed_instr() {
        let load = InstrTyped::constant_none();
        let guard = InstrTyped::DirectCallGuardTest(TypedDirectCallGuardTest::new(
            load,
            TypedDirectCallGuardTestKind::RuntimeFunctionId {
                function_id: RuntimeFunctionId::from_raw_parts(0, 7),
            },
        ));

        let mut counter = TypedInstrCounter::default();
        counter.visit_instr(&guard);

        assert_eq!(counter.direct_call_guard_tests, 1);
        assert_eq!(
            counter.first_class,
            counter.loads + counter.direct_call_guard_tests
        );
        assert!(
            try_lower_typed_instr_to_codegen_legacy(guard).is_err(),
            "function-id direct-call guards should not silently lower through the legacy adapter"
        );
    }

    #[test]
    fn direct_callable_call_is_first_class_typed_instr() {
        let func = InstrTyped::constant_none();
        let arg = InstrTyped::constant_none();
        let direct_call = InstrTyped::DirectCallableCallTyped(TypedDirectCallableCall::new(
            func,
            vec![CallArgPositional::Positional(arg)],
            TypedDirectCallableCallGuard::Function(TypedDirectFunctionCallGuard {
                function_id: RuntimeFunctionId::from_raw_parts(0, 8),
                arg_plan: TypedDirectCallArgPlan {
                    sources: vec![TypedDirectCallArgSource::Provided(0)],
                },
            }),
        ));

        let mut counter = TypedInstrCounter::default();
        counter.visit_instr(&direct_call);

        assert_eq!(counter.direct_callable_calls, 1);
        assert_eq!(counter.loads, 2);
        assert_eq!(
            counter.first_class,
            counter.loads + counter.direct_callable_calls
        );
        assert!(
            try_lower_typed_instr_to_codegen_legacy(direct_call).is_err(),
            "typed direct callable calls should not silently lower through the legacy adapter"
        );
    }

    #[test]
    fn global_none_load_is_known_none_value() {
        let none = InstrTyped::Load(Load::new(ResolvedName {
            id: "NONE".into(),
            location: NameLocation::GlobalName,
        }));
        let other = InstrTyped::Load(Load::new(ResolvedName {
            id: "other".into(),
            location: NameLocation::GlobalName,
        }));

        assert!(typed_expr_is_known_none_value(&none, &[]));
        assert!(!typed_expr_is_known_none_value(&other, &[]));
    }

    #[test]
    fn direct_method_call_is_first_class_typed_instr() {
        let receiver = InstrTyped::constant_none();
        let arg = InstrTyped::constant_none();
        let direct_call = InstrTyped::DirectMethodCallTyped(TypedDirectMethodCall::new(
            receiver,
            vec![CallArgPositional::Positional(arg)],
            "__next__",
            TypedDirectMethodCallGuard {
                function_id: RuntimeFunctionId::from_raw_parts(0, 9),
                owner_type_ref: TypedAttrOwnerRef::TypeKey {
                    module_name: "__main__".to_string(),
                    qualname: "IterRange".to_string(),
                },
                type_version: 1,
                arg_plan: TypedDirectCallArgPlan {
                    sources: vec![
                        TypedDirectCallArgSource::Provided(0),
                        TypedDirectCallArgSource::Provided(1),
                    ],
                },
            },
        ));

        let mut counter = TypedInstrCounter::default();
        counter.visit_instr(&direct_call);

        assert_eq!(counter.direct_method_calls, 1);
        assert_eq!(counter.loads, 2);
        assert_eq!(
            counter.first_class,
            counter.loads + counter.direct_method_calls
        );
        assert!(
            try_lower_typed_instr_to_codegen_legacy(direct_call).is_err(),
            "typed direct method calls should not silently lower through the legacy adapter"
        );
    }
}
