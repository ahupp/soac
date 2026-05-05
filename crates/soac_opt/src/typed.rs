use crate::passes::{ConstructorFieldValue, InlinePlanModule, value_facts};
#[allow(unused_imports)]
use soac_core::block_py;
#[allow(unused_imports)]
use soac_core::block_py::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule,
    BlockTerm, Call, CallArgKeyword, CallArgPositional, CallDirect, CalleeFunctionId,
    ChildVisitable, ConstantExpr, Del, HasMeta, HasSemanticInstrId, Instr, InstrId, InstrKey,
    InstrWithConstantNone, Literal, Load, LocalLocation, MapInstr, Mappable, Meta, NameLike,
    NameLocation, ParamKind, PrettyPrint, PrettyPrinter, ResolvedName, RuntimeFunctionId,
    RuntimeName, SetAttr, Store, TermIf, TryMapInstr, TryMapModule, TryMapTerm, Tuple, Visit,
    VisitMut, WithMeta,
};
use soac_ir_blockpy::{
    BlockPyModuleShape, InstrBlockPy, constructor_init_function_id_for_entry_function,
    is_constructor_entry_function,
};
use soac_ir_typed::emit_v3::MechanicalExitKind;
use soac_ir_typed::plan_v3::Rep;
use soac_ir_typed::{
    BoolFacts, FactStore, InstrTyped, PyObjFacts, TypedAttrAccessPlan, TypedAttrOwnerRef,
    TypedBlock, TypedBlockExtra, TypedBlockPyModuleShape, TypedCall, TypedCallAccessPlan,
    TypedCallEmissionPlan, TypedCallEmissionPlans, TypedConstructorInitPlan,
    TypedConstructorInitPlanSource, TypedDirectCallArgPlan, TypedDirectCallArgSource,
    TypedDirectCallGuardTest, TypedDirectCallGuardTestKind, TypedDirectCallableCall,
    TypedDirectCallableCallGuard, TypedDirectMethodCall, TypedDirectMethodCallGuard, TypedGetAttr,
    TypedGuardedCallableCall, TypedGuardedMethodCall, TypedPlannedResult,
    TypedPyObjectOwnershipPlan, TypedResultDemand, TypedSetAttr, TypedTruthy, ValueFacts,
};
use std::collections::{HashMap, HashSet};

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
                TypedCallEmissionPlan::Method {
                    method_name,
                    method_guards,
                } => {
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

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedInlineRewriteStats {
    pub rewritten_stores: usize,
    pub rewritten_effect_only_calls: usize,
    pub skipped_candidates: usize,
    pub skipped_exception_edges: usize,
    pub instr_id_mappings: Vec<TypedInlineInstrIdMapping>,
    pub local_mappings: Vec<TypedInlineLocalMapping>,
    pub hot_state_cleanup_labels: Vec<BlockLabel>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TypedHotContinuationClone {
    pub hot_block: BlockLabel,
    pub original_entry: BlockLabel,
    pub cloned_entry: BlockLabel,
    pub cloned_blocks: usize,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedHotContinuationSplitStats {
    pub cloned_blocks: usize,
    pub clones: Vec<TypedHotContinuationClone>,
    pub instr_id_mappings: Vec<TypedInlineInstrIdMapping>,
    pub label_mappings: Vec<(BlockLabel, BlockLabel)>,
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
pub struct TypedExternalInlineCallee {
    pub function: BlockPyFunction<TypedBlockPyModuleShape>,
    pub module_constants: Vec<ConstantExpr>,
    pub inline_plan: Option<InlinePlanModule>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedFieldScalarizationStats {
    pub seeded_objects: usize,
    pub scalar_slots: usize,
    pub inserted_scalar_stores: usize,
    pub rewritten_loads: usize,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TypedVirtualConstructorPlan {
    pub source: InstrId,
    pub root: ResolvedName,
    pub materialization_block: BlockLabel,
    pub materialization_index: usize,
    pub reachable_blocks: HashSet<BlockLabel>,
    pub virtual_locations: HashSet<LocalLocation>,
    pub virtual_names: HashSet<String>,
    pub assumed_owner_type: Option<TypedAttrOwnerRef>,
    pub guard_blocks: HashSet<BlockLabel>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedVirtualConstructorStats {
    pub planned_objects: usize,
    pub removed_materializations: usize,
    pub removed_field_stores: usize,
    pub removed_alias_stores: usize,
    pub removed_dels: usize,
    pub removed_guards: usize,
    pub removed_block_params: usize,
    pub removed_block_args: usize,
}

impl TypedVirtualConstructorStats {
    pub fn changed(&self) -> bool {
        self.removed_materializations != 0
            || self.removed_field_stores != 0
            || self.removed_alias_stores != 0
            || self.removed_dels != 0
            || self.removed_guards != 0
            || self.removed_block_params != 0
            || self.removed_block_args != 0
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[allow(dead_code)]
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
    ClosureStorage,
    CrossModuleGlobalName(String),
    TooManyCallerConstants,
}

#[derive(Clone)]
struct TypedInlineDirectCallPlan {
    target: RuntimeFunctionId,
    arg_plan: TypedDirectCallArgPlan,
    guard: TypedInlineGuardPlan,
}

#[derive(Clone)]
enum TypedInlineGuardPlan {
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
) -> TypedInlineRewriteStats {
    if direct_calls_by_instr_id.is_empty() {
        return TypedInlineRewriteStats::default();
    }

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
}

#[derive(Clone)]
enum TypedInlineCall {
    Callable(TypedGuardedCallableCall<InstrTyped>),
    Method {
        call: TypedGuardedMethodCall<InstrTyped>,
        receiver: InstrTyped,
        attr: InstrTyped,
    },
    RuntimeProtocolMethod {
        call: TypedCall<InstrTyped>,
        receiver: InstrTyped,
    },
}

impl TypedInlineCall {
    fn meta(&self) -> Meta {
        match self {
            Self::Callable(call) => call.meta(),
            Self::Method { call, .. } => call.meta(),
            Self::RuntimeProtocolMethod { call, .. } => call.meta(),
        }
    }

    fn args(&self) -> Vec<CallArgPositional<InstrTyped>> {
        match self {
            Self::Callable(call) => call.args.clone(),
            Self::Method { call, .. } => call.args.clone(),
            Self::RuntimeProtocolMethod { call, .. } => runtime_protocol_explicit_args(call)
                .unwrap_or_default()
                .to_vec(),
        }
    }

    fn keywords(&self) -> &[CallArgKeyword<InstrTyped>] {
        match self {
            Self::Callable(call) => call.keywords.as_slice(),
            Self::Method { call, .. } => call.keywords.as_slice(),
            Self::RuntimeProtocolMethod { call, .. } => call.keywords.as_slice(),
        }
    }
}

fn build_typed_direct_call_inline_rewrite(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    mut caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    external_callees: TypedInlineExternalCallees<'_>,
    block: TypedBlock,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    next_inline_instance: &mut u32,
    stats: &mut TypedInlineRewriteStats,
) -> TypedInlineBlockRewrite {
    let original_block = block.clone();
    let original_storage_layout = caller.storage_layout.clone();
    let Some(candidate) =
        find_typed_inline_candidate(&block, caller.function_id, direct_calls_by_instr_id)
    else {
        return TypedInlineBlockRewrite::Unchanged(block);
    };
    let original_exc_edge = block.exc_edge.clone();
    if !candidate.call.keywords().is_empty() {
        stats.skipped_candidates += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    }
    let Some(positional_arg_exprs) = typed_positional_arg_exprs(candidate.call.args()) else {
        stats.skipped_candidates += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    };

    let receiver_temp = match &candidate.call {
        TypedInlineCall::Callable(_) => None,
        TypedInlineCall::Method { .. } | TypedInlineCall::RuntimeProtocolMethod { .. } => {
            match try_allocate_typed_stack_temp(caller, "typed_inline_receiver") {
                Ok(temp) => Some(temp),
                Err(_) => {
                    stats.skipped_candidates += 1;
                    return TypedInlineBlockRewrite::Unchanged(block);
                }
            }
        }
    };
    let callable_temp = match &candidate.call {
        TypedInlineCall::Callable(_) => {
            match try_allocate_typed_stack_temp(caller, "typed_inline_callable") {
                Ok(temp) => Some(temp),
                Err(_) => {
                    stats.skipped_candidates += 1;
                    caller.storage_layout = original_storage_layout;
                    return TypedInlineBlockRewrite::Unchanged(block);
                }
            }
        }
        TypedInlineCall::Method { .. } => None,
        TypedInlineCall::RuntimeProtocolMethod { .. } => None,
    };
    let arg_temps = match (0..positional_arg_exprs.len())
        .map(|_| try_allocate_typed_stack_temp(caller, "typed_inline_arg"))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(temps) => temps,
        Err(_) => {
            stats.skipped_candidates += 1;
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
                    caller.storage_layout = original_storage_layout;
                    return TypedInlineBlockRewrite::Unchanged(block);
                }
            };
            let return_target = result_temp.resolved_name();
            (return_target.clone(), Some(return_target))
        }
    };
    let continuation_label = caller.name_gen.next_block_name();
    let generic_label = caller.name_gen.next_block_name();
    let cleanup_label = caller.name_gen.next_block_name();
    let guard_labels = (0..candidate.inline_plans.len().saturating_sub(1))
        .map(|_| caller.name_gen.next_block_name())
        .collect::<Vec<_>>();
    let hot_labels = candidate
        .inline_plans
        .iter()
        .map(|_| caller.name_gen.next_block_name())
        .collect::<Vec<_>>();
    let mut instr_id_mappings = Vec::new();
    let mut local_mappings = Vec::new();
    let original_caller_module_constants = caller_module_constants
        .as_deref()
        .map(|constants| constants.to_vec());
    let mut cleanup_carries_hot_state = receiver_temp.is_some();

    let mut before = block.body;
    let after = before.split_off(candidate.instr_index + 1);
    before.truncate(candidate.instr_index);
    match &candidate.call {
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
        | TypedInlineCall::RuntimeProtocolMethod { receiver, .. } => {
            let receiver_temp = receiver_temp
                .as_ref()
                .expect("method inline candidate should allocate receiver temp");
            before.push(
                Store::new(receiver_temp.resolved_name(), receiver.clone())
                    .with_meta(Meta::synthetic())
                    .into(),
            );
        }
    }
    for (arg_temp, arg_expr) in arg_temps.iter().zip(positional_arg_exprs) {
        before.push(
            Store::new(arg_temp.resolved_name(), arg_expr)
                .with_meta(Meta::synthetic())
                .into(),
        );
    }

    let entry = Block::new_with_extra(
        block.label,
        before,
        typed_inline_guard_term(
            &candidate.call,
            &candidate.inline_plans[0],
            callable_temp.as_ref(),
            receiver_temp.as_ref(),
            candidate.call.meta(),
            hot_labels[0],
            guard_labels.first().copied().unwrap_or(generic_label),
        ),
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
            .unwrap_or(generic_label);
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
        let Ok(bindings) = bind_typed_direct_call_inline_values(
            callee.function,
            &plan.arg_plan,
            provided_values.as_slice(),
        ) else {
            stats.skipped_candidates += 1;
            if let (Some(constants), Some(original)) = (
                caller_module_constants.as_deref_mut(),
                original_caller_module_constants.as_ref(),
            ) {
                *constants = original.clone();
            }
            caller.storage_layout = original_storage_layout;
            return TypedInlineBlockRewrite::Unchanged(original_block);
        };
        let inline_instance = *next_inline_instance;
        *next_inline_instance = next_inline_instance
            .checked_add(1)
            .expect("typed inline instance count should fit in u32");
        let Ok(mut fragment) = build_typed_direct_call_inline_fragment_to_target(
            caller,
            callee.function,
            cleanup_label,
            &bindings,
            return_target.clone(),
            inline_instance,
            instr_id_allocator,
            caller_module_constants.as_deref_mut(),
            callee.module_constants,
        ) else {
            stats.skipped_candidates += 1;
            caller.storage_layout = original_storage_layout;
            return TypedInlineBlockRewrite::Unchanged(original_block);
        };
        for block in &mut fragment.blocks {
            block.exc_edge = original_exc_edge.clone();
        }
        if let Some(entry) = fragment.blocks.first_mut() {
            entry.label = hot_label;
        }
        instr_id_mappings.extend(fragment.instr_id_mappings);
        local_mappings.extend(fragment.local_mappings);
        blocks.extend(fragment.blocks);
    }

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
        block.term,
        Vec::new(),
        original_exc_edge,
        TypedBlockExtra::default(),
    ));

    match candidate.result {
        TypedInlineResult::StoreTo(_) => stats.rewritten_stores += 1,
        TypedInlineResult::EffectOnly => stats.rewritten_effect_only_calls += 1,
    }
    stats.instr_id_mappings.extend(instr_id_mappings);
    stats.local_mappings.extend(local_mappings);
    TypedInlineBlockRewrite::Rewritten(blocks)
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
) -> TypedHotContinuationSplitStats {
    split_typed_alias_hot_continuations_impl(function, None)
}

pub fn split_typed_alias_hot_continuations_with_budget(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    max_cloned_blocks: usize,
) -> TypedHotContinuationSplitStats {
    if max_cloned_blocks == 0 {
        return TypedHotContinuationSplitStats::default();
    }
    split_typed_alias_hot_continuations_impl(function, Some(max_cloned_blocks))
}

fn split_typed_alias_hot_continuations_impl(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    max_cloned_blocks: Option<usize>,
) -> TypedHotContinuationSplitStats {
    let mut stats = TypedHotContinuationSplitStats::default();
    let mut instr_id_allocator = TypedInlineInstrIdAllocator::from_function(function);
    loop {
        let Some(candidate) = find_typed_alias_hot_continuation_split_candidate(function) else {
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
    function.blocks.iter().find_map(|block| {
        let original_entry = typed_constructor_hot_continuation_entry(
            function,
            &labels,
            &predecessors,
            block,
            module_constants,
        )?;
        let reachable = typed_hot_reachable_block_labels(function, &labels, original_entry)?;
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
) -> Option<TypedHotContinuationSplitCandidate> {
    let labels = typed_block_indices_by_label(function);
    let predecessors = typed_block_predecessors(function);
    function.blocks.iter().find_map(|block| {
        let original_entry =
            typed_alias_hot_continuation_entry(function, &labels, &predecessors, block)?;
        let reachable = typed_hot_reachable_block_labels(function, &labels, original_entry)?;
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

fn find_typed_inline_cleanup_hot_continuation_split_candidate(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    cleanup_labels: Option<&HashSet<BlockLabel>>,
) -> Option<TypedHotContinuationSplitCandidate> {
    let labels = typed_block_indices_by_label(function);
    let predecessors = typed_block_predecessors(function);
    let hot_path_labels = typed_direct_call_hot_path_labels(function, &labels);
    function.blocks.iter().find_map(|block| {
        if cleanup_labels.is_some_and(|cleanup_labels| !cleanup_labels.contains(&block.label)) {
            return None;
        }
        let original_entry = typed_inline_cleanup_hot_continuation_entry(&hot_path_labels, block)?;
        let reachable = typed_hot_reachable_block_labels(function, &labels, original_entry)?;
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
) -> Option<BlockLabel> {
    if !typed_block_contains_local_alias_store(block)
        || !typed_block_is_direct_call_guard_hot_successor(function, labels, predecessors, block)
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
        typed_expr_is_runtime_name_load(
            call.func.as_ref(),
            RuntimeName::ConstructorCall,
            module_constants,
        )
    })
}

fn typed_block_contains_local_alias_store(block: &TypedBlock) -> bool {
    block.body.iter().any(|instr| {
        let InstrTyped::Store(store) = instr else {
            return false;
        };
        if store.name.location.as_local().is_none() {
            return false;
        }
        typed_instr_local_load_location(store.value.as_ref()).is_some()
    })
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

#[cfg(test)]
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
            .filter_map(|store| {
                let index = match &store.value {
                    ConstructorFieldValue::Param { index, .. } => *index,
                    ConstructorFieldValue::Local { .. }
                    | ConstructorFieldValue::Constant { .. }
                    | ConstructorFieldValue::Other => return None,
                };
                let callee_location = LocalLocation(
                    u32::try_from(index).expect("constructor parameter index should fit in u32"),
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
            .collect::<Vec<_>>();
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
            if let InstrTyped::CallTyped(call) = expr
                && typed_expr_is_runtime_name_load(
                    call.func.as_ref(),
                    RuntimeName::ConstructorCall,
                    self.module_constants,
                )
                && let Some(instr_id) = call.try_semantic_instr_id()
            {
                self.instr_ids.insert(instr_id);
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

pub fn scalarize_typed_hot_constructor_field_loads(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
) -> TypedFieldScalarizationStats {
    let mut constructor_field_bindings = constructor_field_bindings.clone();
    scalarize_typed_hot_constructor_field_loads_with_bindings(
        function,
        module_constants,
        &mut constructor_field_bindings,
    )
}

pub fn scalarize_typed_hot_constructor_field_loads_with_bindings(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &mut HashMap<InstrId, TypedConstructorFieldBindings>,
) -> TypedFieldScalarizationStats {
    if constructor_field_bindings.is_empty() {
        return TypedFieldScalarizationStats::default();
    }
    let scalar_slots =
        allocate_typed_constructor_field_scalar_slots(function, constructor_field_bindings);
    let predecessors = typed_normal_block_predecessors(function);
    let mut in_states = vec![None::<TypedFieldScalarState>; function.blocks.len()];
    let mut out_states = vec![None::<TypedFieldScalarState>; function.blocks.len()];
    let labels = typed_block_indices_by_label(function);
    let entry_label = function.blocks.first().map(|block| block.label);
    loop {
        let mut changed = false;
        for (block_index, block) in function.blocks.iter().enumerate() {
            let in_state = typed_field_scalar_in_state_for_block(
                block.label,
                entry_label,
                &predecessors,
                &labels,
                &out_states,
            );
            if in_states[block_index] != in_state {
                in_states[block_index] = in_state.clone();
                changed = true;
            }
            let out_state = in_state.map(|mut out_state| {
                let mut block_clone = block.clone();
                let mut ignored_stats = TypedFieldScalarizationStats::default();
                transfer_typed_field_scalar_block(
                    &mut block_clone,
                    &mut out_state,
                    module_constants,
                    constructor_field_bindings,
                    &mut ignored_stats,
                    false,
                );
                out_state
            });
            if out_states[block_index] != out_state {
                out_states[block_index] = out_state;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut stats = TypedFieldScalarizationStats {
        scalar_slots,
        ..TypedFieldScalarizationStats::default()
    };
    for (block_index, block) in function.blocks.iter_mut().enumerate() {
        let mut state = in_states
            .get(block_index)
            .cloned()
            .flatten()
            .unwrap_or_else(TypedFieldScalarState::default);
        transfer_typed_field_scalar_block(
            block,
            &mut state,
            module_constants,
            constructor_field_bindings,
            &mut stats,
            true,
        );
    }
    stats
}

pub fn plan_typed_virtual_constructors(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
) -> Vec<TypedVirtualConstructorPlan> {
    if constructor_field_bindings.is_empty() {
        return Vec::new();
    }
    let labels = typed_block_indices_by_label(function);
    let predecessors = typed_block_predecessors(function);
    let mut plans = Vec::new();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some((source, root, bindings)) =
                typed_virtual_constructor_materialization(instr, constructor_field_bindings)
            else {
                continue;
            };
            if bindings.fields.iter().any(|field| field.scalar.is_none()) {
                continue;
            }
            let Some(root_location) = root.local_location() else {
                continue;
            };
            let BlockTerm::Jump(edge) = &block.term else {
                continue;
            };
            let Some(reachable) = typed_hot_reachable_block_labels(function, &labels, edge.target)
            else {
                continue;
            };
            if reachable.contains(&block.label)
                || reachable.len() > MAX_TYPED_HOT_CONTINUATION_CLONE_BLOCKS
                || !typed_reachable_subgraph_has_external_predecessor(
                    &reachable,
                    &predecessors,
                    block.label,
                )
            {
                continue;
            }
            let mut plan = TypedVirtualConstructorPlan {
                source,
                root: root.clone(),
                materialization_block: block.label,
                materialization_index: instr_index,
                reachable_blocks: reachable,
                virtual_locations: HashSet::from([root_location]),
                virtual_names: HashSet::from([root.id_str().to_string()]),
                assumed_owner_type: None,
                guard_blocks: HashSet::new(),
            };
            if complete_typed_virtual_constructor_plan(
                function,
                module_constants,
                bindings,
                &mut plan,
            ) {
                plans.push(plan);
            }
        }
    }
    plans
}

pub fn virtualize_typed_hot_constructor_objects(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
) -> TypedVirtualConstructorStats {
    let plans =
        plan_typed_virtual_constructors(function, module_constants, constructor_field_bindings);
    let mut stats = TypedVirtualConstructorStats {
        planned_objects: plans.len(),
        ..TypedVirtualConstructorStats::default()
    };
    if plans.is_empty() {
        return stats;
    }
    let param_removals = typed_virtual_constructor_param_removals(function, &plans);
    for block in &mut function.blocks {
        if let Some(remove) = param_removals.get(&block.label) {
            let before = block.params.len();
            block.params = block
                .params
                .iter()
                .enumerate()
                .filter_map(|(index, param)| (!remove.contains(&index)).then_some(param.clone()))
                .collect();
            stats.removed_block_params += before.saturating_sub(block.params.len());
        }
    }
    for block in &mut function.blocks {
        rewrite_typed_virtual_constructor_edges(block, &param_removals, &mut stats);
    }
    for block in &mut function.blocks {
        rewrite_typed_virtual_constructor_block(
            block,
            module_constants,
            constructor_field_bindings,
            &plans,
            &mut stats,
        );
    }
    stats
}

fn typed_virtual_constructor_materialization<'a>(
    instr: &'a InstrTyped,
    constructor_field_bindings: &'a HashMap<InstrId, TypedConstructorFieldBindings>,
) -> Option<(InstrId, &'a ResolvedName, &'a TypedConstructorFieldBindings)> {
    let InstrTyped::Store(store) = instr else {
        return None;
    };
    let InstrTyped::CallTyped(call) = store.value.as_ref() else {
        return None;
    };
    let instr_id = call.try_semantic_instr_id()?;
    let bindings = constructor_field_bindings.get(&instr_id)?;
    Some((instr_id, &store.name, bindings))
}

fn complete_typed_virtual_constructor_plan(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &mut TypedVirtualConstructorPlan,
) -> bool {
    loop {
        let before_locations = plan.virtual_locations.len();
        let before_names = plan.virtual_names.len();
        for block in &function.blocks {
            if !typed_virtual_constructor_plan_covers_block(plan, block.label) {
                continue;
            }
            let start = if block.label == plan.materialization_block {
                plan.materialization_index + 1
            } else {
                0
            };
            for instr in block.body.iter().skip(start) {
                if !scan_typed_virtual_constructor_instr(
                    function,
                    module_constants,
                    bindings,
                    plan,
                    instr,
                ) {
                    return false;
                }
            }
            if !scan_typed_virtual_constructor_term(function, bindings, plan, block) {
                return false;
            }
            if let Some(edge) = &block.exc_edge
                && !scan_typed_virtual_constructor_edge(function, plan, edge)
            {
                return false;
            }
        }
        if plan.virtual_locations.len() == before_locations
            && plan.virtual_names.len() == before_names
        {
            break;
        }
    }
    true
}

fn typed_virtual_constructor_plan_covers_block(
    plan: &TypedVirtualConstructorPlan,
    label: BlockLabel,
) -> bool {
    label == plan.materialization_block || plan.reachable_blocks.contains(&label)
}

fn scan_typed_virtual_constructor_instr(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &mut TypedVirtualConstructorPlan,
    instr: &InstrTyped,
) -> bool {
    match instr {
        InstrTyped::Store(store) => {
            if typed_virtual_constructor_alias_store(function, plan, store) {
                return true;
            }
            !typed_expr_uses_virtual_constructor_identity(store.value.as_ref(), plan)
                && !typed_resolved_name_is_virtual_constructor(&store.name, plan)
        }
        InstrTyped::Del(del) if typed_resolved_name_is_virtual_constructor(&del.name, plan) => true,
        InstrTyped::SetAttrTyped(op)
            if typed_virtual_constructor_field_store(op, module_constants, bindings, plan) =>
        {
            !typed_expr_uses_virtual_constructor_identity(op.attr.as_ref(), plan)
                && !typed_expr_uses_virtual_constructor_identity(op.replacement.as_ref(), plan)
        }
        _ => !typed_expr_uses_virtual_constructor_identity(instr, plan),
    }
}

fn scan_typed_virtual_constructor_term(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    bindings: &TypedConstructorFieldBindings,
    plan: &mut TypedVirtualConstructorPlan,
    block: &TypedBlock,
) -> bool {
    let term_ok = match &block.term {
        BlockTerm::IfTerm(if_term) => {
            if let InstrTyped::DirectCallGuardTest(guard) = &if_term.test
                && typed_expr_is_virtual_constructor_load(guard.value.as_ref(), plan)
            {
                let TypedDirectCallGuardTestKind::ExactTypeVersion { owner_type_ref, .. } =
                    &guard.kind
                else {
                    return false;
                };
                if plan
                    .assumed_owner_type
                    .as_ref()
                    .is_some_and(|existing| existing != owner_type_ref)
                {
                    return false;
                }
                plan.assumed_owner_type = Some(owner_type_ref.clone());
                plan.guard_blocks.insert(block.label);
                return true;
            }
            !typed_expr_uses_virtual_constructor_identity(&if_term.test, plan)
        }
        BlockTerm::Jump(edge) => scan_typed_virtual_constructor_edge(function, plan, edge),
        BlockTerm::BranchTable(branch) => {
            !typed_expr_uses_virtual_constructor_identity(&branch.index, plan)
        }
        BlockTerm::Raise(raise) => raise
            .exc
            .as_ref()
            .is_none_or(|exc| !typed_expr_uses_virtual_constructor_identity(exc, plan)),
        BlockTerm::Return(value) => !typed_expr_uses_virtual_constructor_identity(value, plan),
    };
    term_ok && bindings.fields.iter().all(|field| field.scalar.is_some())
}

fn scan_typed_virtual_constructor_edge(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &mut TypedVirtualConstructorPlan,
    edge: &BlockEdge,
) -> bool {
    for arg in &edge.args {
        if let BlockArg::Name(name) = arg
            && plan.virtual_names.contains(name)
            && !plan.reachable_blocks.contains(&edge.target)
        {
            return false;
        }
    }
    let Some(target) = function
        .blocks
        .iter()
        .find(|block| block.label == edge.target)
    else {
        return true;
    };
    for (index, arg) in edge.args.iter().enumerate() {
        let BlockArg::Name(name) = arg else {
            continue;
        };
        if !plan.virtual_names.contains(name) {
            continue;
        }
        let Some(param) = target.params.get(index) else {
            continue;
        };
        if param.role != BlockParamRole::Value {
            return false;
        }
        plan.virtual_names.insert(param.name.clone());
        if let Some(location) = typed_local_location_for_name(function, &param.name) {
            plan.virtual_locations.insert(location);
        }
    }
    true
}

fn typed_virtual_constructor_alias_store(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &mut TypedVirtualConstructorPlan,
    store: &Store<InstrTyped>,
) -> bool {
    if !typed_expr_is_virtual_constructor_load(store.value.as_ref(), plan) {
        return false;
    }
    let Some(location) = store.name.local_location() else {
        return false;
    };
    plan.virtual_locations.insert(location);
    plan.virtual_names.insert(store.name.id_str().to_string());
    if let Some(location) = typed_local_location_for_name(function, store.name.id_str()) {
        plan.virtual_locations.insert(location);
    }
    true
}

fn typed_virtual_constructor_field_store(
    op: &TypedSetAttr<InstrTyped>,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &TypedVirtualConstructorPlan,
) -> bool {
    if !typed_expr_is_virtual_constructor_load(op.value.as_ref(), plan)
        || !typed_attr_access_is_indexed_field(&op.access)
    {
        return false;
    }
    let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants) else {
        return false;
    };
    bindings
        .fields
        .iter()
        .any(|field| field.field_name == field_name && field.scalar.is_some())
}

fn typed_expr_is_virtual_constructor_load(
    expr: &InstrTyped,
    plan: &TypedVirtualConstructorPlan,
) -> bool {
    let InstrTyped::Load(load) = expr else {
        return false;
    };
    typed_resolved_name_is_virtual_constructor(&load.name, plan)
}

fn typed_resolved_name_is_virtual_constructor(
    name: &ResolvedName,
    plan: &TypedVirtualConstructorPlan,
) -> bool {
    name.local_location()
        .is_some_and(|location| plan.virtual_locations.contains(&location))
        || plan.virtual_names.contains(name.id_str())
}

fn typed_expr_uses_virtual_constructor_identity(
    expr: &InstrTyped,
    plan: &TypedVirtualConstructorPlan,
) -> bool {
    struct Finder<'a> {
        plan: &'a TypedVirtualConstructorPlan,
        found: bool,
    }

    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }
            if typed_expr_is_virtual_constructor_load(expr, self.plan) {
                self.found = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder { plan, found: false };
    finder.visit_instr(expr);
    finder.found
}

fn typed_local_location_for_name(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    name: &str,
) -> Option<LocalLocation> {
    let layout = function.storage_layout.as_ref()?;
    layout
        .stack_slots()
        .iter()
        .position(|slot_name| slot_name == name)
        .map(|slot| {
            LocalLocation(
                u32::try_from(slot).expect("stack slot index should fit in LocalLocation"),
            )
        })
}

fn typed_virtual_constructor_param_removals(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plans: &[TypedVirtualConstructorPlan],
) -> HashMap<BlockLabel, HashSet<usize>> {
    let mut removals = HashMap::<BlockLabel, HashSet<usize>>::new();
    for block in &function.blocks {
        for plan in plans {
            if !plan.reachable_blocks.contains(&block.label) {
                continue;
            }
            for (index, param) in block.params.iter().enumerate() {
                if param.role == BlockParamRole::Value && plan.virtual_names.contains(&param.name) {
                    removals.entry(block.label).or_default().insert(index);
                }
            }
        }
    }
    removals
}

fn rewrite_typed_virtual_constructor_edges(
    block: &mut TypedBlock,
    param_removals: &HashMap<BlockLabel, HashSet<usize>>,
    stats: &mut TypedVirtualConstructorStats,
) {
    match &mut block.term {
        BlockTerm::Jump(edge) => {
            stats.removed_block_args += rewrite_typed_virtual_constructor_edge(edge, param_removals)
        }
        BlockTerm::IfTerm(_)
        | BlockTerm::BranchTable(_)
        | BlockTerm::Raise(_)
        | BlockTerm::Return(_) => {}
    }
    if let Some(edge) = &mut block.exc_edge {
        stats.removed_block_args += rewrite_typed_virtual_constructor_edge(edge, param_removals);
    }
}

fn rewrite_typed_virtual_constructor_edge(
    edge: &mut BlockEdge,
    param_removals: &HashMap<BlockLabel, HashSet<usize>>,
) -> usize {
    let Some(remove) = param_removals.get(&edge.target) else {
        return 0;
    };
    let before = edge.args.len();
    edge.args = edge
        .args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| (!remove.contains(&index)).then_some(arg.clone()))
        .collect();
    before.saturating_sub(edge.args.len())
}

fn rewrite_typed_virtual_constructor_block(
    block: &mut TypedBlock,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    plans: &[TypedVirtualConstructorPlan],
    stats: &mut TypedVirtualConstructorStats,
) {
    let old_body = std::mem::take(&mut block.body);
    let mut new_body = Vec::with_capacity(old_body.len());
    for (instr_index, instr) in old_body.into_iter().enumerate() {
        if typed_virtual_constructor_should_remove_instr(
            block.label,
            instr_index,
            &instr,
            module_constants,
            constructor_field_bindings,
            plans,
            stats,
        ) {
            continue;
        }
        new_body.push(instr);
    }
    block.body = new_body;
    let guard_then_label = match &block.term {
        BlockTerm::IfTerm(if_term)
            if plans
                .iter()
                .any(|plan| plan.guard_blocks.contains(&block.label)) =>
        {
            Some(if_term.then_label)
        }
        _ => None,
    };
    if let Some(then_label) = guard_then_label {
        block.term = BlockTerm::Jump(BlockEdge::new(then_label));
        stats.removed_guards += 1;
    }
}

fn typed_virtual_constructor_should_remove_instr(
    label: BlockLabel,
    instr_index: usize,
    instr: &InstrTyped,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    plans: &[TypedVirtualConstructorPlan],
    stats: &mut TypedVirtualConstructorStats,
) -> bool {
    for plan in plans {
        if label == plan.materialization_block
            && instr_index == plan.materialization_index
            && typed_virtual_constructor_materialization(instr, constructor_field_bindings)
                .is_some_and(|(source, _, _)| source == plan.source)
        {
            stats.removed_materializations += 1;
            return true;
        }
        if !typed_virtual_constructor_plan_covers_block(plan, label)
            || (label == plan.materialization_block && instr_index <= plan.materialization_index)
        {
            continue;
        }
        match instr {
            InstrTyped::Store(store)
                if typed_expr_is_virtual_constructor_load(store.value.as_ref(), plan) =>
            {
                stats.removed_alias_stores += 1;
                return true;
            }
            InstrTyped::Del(del) if typed_resolved_name_is_virtual_constructor(&del.name, plan) => {
                stats.removed_dels += 1;
                return true;
            }
            InstrTyped::SetAttrTyped(op)
                if constructor_field_bindings
                    .get(&plan.source)
                    .is_some_and(|bindings| {
                        typed_virtual_constructor_field_store(op, module_constants, bindings, plan)
                    }) =>
            {
                stats.removed_field_stores += 1;
                return true;
            }
            _ => {}
        }
    }
    false
}

fn typed_field_scalar_in_state_for_block(
    label: BlockLabel,
    entry_label: Option<BlockLabel>,
    predecessors: &HashMap<BlockLabel, HashSet<BlockLabel>>,
    labels: &HashMap<BlockLabel, usize>,
    out_states: &[Option<TypedFieldScalarState>],
) -> Option<TypedFieldScalarState> {
    let Some(predecessors) = predecessors.get(&label) else {
        return (Some(label) == entry_label).then(TypedFieldScalarState::default);
    };
    let computed = predecessors
        .iter()
        .filter_map(|label| labels.get(label).and_then(|index| out_states.get(*index)))
        .filter_map(|state| state.as_ref())
        .collect::<Vec<_>>();
    if computed.is_empty() {
        None
    } else {
        Some(merge_typed_field_scalar_states(computed.into_iter()))
    }
}

fn allocate_typed_constructor_field_scalar_slots(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    constructor_field_bindings: &mut HashMap<InstrId, TypedConstructorFieldBindings>,
) -> usize {
    if function.storage_layout.is_none() {
        return 0;
    }
    let mut allocated = 0;
    for bindings in constructor_field_bindings.values_mut() {
        for field in &mut bindings.fields {
            if field.scalar.is_some() {
                continue;
            }
            let Ok(temp) = try_allocate_typed_stack_temp(function, "_dp_typed_scalar_field") else {
                continue;
            };
            field.scalar = Some(temp.resolved_name());
            allocated += 1;
        }
    }
    allocated
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TypedFieldScalarState {
    aliases: HashMap<LocalLocation, LocalLocation>,
    value_aliases: HashMap<LocalLocation, ResolvedName>,
    fields: HashMap<(LocalLocation, String), ResolvedName>,
    field_scalars: HashMap<(LocalLocation, String), ResolvedName>,
}

impl TypedFieldScalarState {
    fn root_for_location(&self, location: LocalLocation) -> Option<LocalLocation> {
        let mut current = location;
        let mut seen = HashSet::new();
        loop {
            let next = *self.aliases.get(&current)?;
            if next == current {
                return Some(current);
            }
            if !seen.insert(current) {
                return None;
            }
            current = next;
        }
    }

    fn rebind_local(&mut self, location: LocalLocation) {
        self.value_aliases.remove(&location);
        self.value_aliases.retain(|_, value| {
            value
                .local_location()
                .is_none_or(|source| source != location)
        });
        self.fields.retain(|_, value| {
            value
                .local_location()
                .is_none_or(|source| source != location)
        });
        if self.aliases.get(&location) == Some(&location) {
            self.rebind_root_local(location);
        } else {
            self.aliases.remove(&location);
        }
    }

    fn rebind_root_local(&mut self, root: LocalLocation) {
        let replacement = self
            .aliases
            .iter()
            .filter_map(|(location, mapped_root)| {
                (*mapped_root == root && *location != root).then_some(*location)
            })
            .min_by_key(|location| location.slot());
        let Some(replacement) = replacement else {
            self.invalidate_root(root);
            return;
        };

        self.aliases.remove(&root);
        for mapped_root in self.aliases.values_mut() {
            if *mapped_root == root {
                *mapped_root = replacement;
            }
        }
        self.aliases.insert(replacement, replacement);
        rekey_typed_field_scalar_map_root(&mut self.fields, root, replacement);
        rekey_typed_field_scalar_map_root(&mut self.field_scalars, root, replacement);
    }

    fn set_alias(&mut self, location: LocalLocation, root: LocalLocation) {
        if location == root {
            self.aliases.insert(root, root);
            return;
        }
        self.rebind_local(location);
        self.aliases.entry(root).or_insert(root);
        self.aliases.insert(location, root);
    }

    fn set_value_alias(&mut self, location: LocalLocation, value: &ResolvedName) {
        self.rebind_local(location);
        let value = self.resolve_scalar_name(value);
        self.value_aliases.insert(location, value);
    }

    fn resolve_scalar_name(&self, value: &ResolvedName) -> ResolvedName {
        let Some(mut location) = value.local_location() else {
            return value.clone();
        };
        let mut resolved = value.clone();
        let mut seen = HashSet::new();
        while seen.insert(location) {
            let Some(mapped) = self.value_aliases.get(&location) else {
                break;
            };
            resolved = mapped.clone();
            let Some(mapped_location) = mapped.local_location() else {
                return mapped.clone();
            };
            location = mapped_location;
        }
        resolved
    }

    fn seed_object(&mut self, location: LocalLocation, bindings: &TypedConstructorFieldBindings) {
        self.rebind_local(location);
        self.aliases.insert(location, location);
        for field in &bindings.fields {
            let value = field
                .scalar
                .as_ref()
                .cloned()
                .unwrap_or_else(|| self.resolve_scalar_name(&field.value));
            self.fields
                .insert((location, field.field_name.clone()), value);
            if let Some(scalar) = &field.scalar {
                self.field_scalars
                    .insert((location, field.field_name.clone()), scalar.clone());
            }
        }
    }

    fn field_value(&self, root: LocalLocation, field_name: &str) -> Option<&ResolvedName> {
        self.fields.get(&(root, field_name.to_string()))
    }

    fn field_scalar(&self, root: LocalLocation, field_name: &str) -> Option<&ResolvedName> {
        self.field_scalars.get(&(root, field_name.to_string()))
    }

    fn set_field(&mut self, root: LocalLocation, field_name: String, value: ResolvedName) {
        let value = self.resolve_scalar_name(&value);
        self.fields.insert((root, field_name), value);
    }

    fn invalidate_field(&mut self, root: LocalLocation, field_name: &str) {
        self.fields.remove(&(root, field_name.to_string()));
    }

    fn invalidate_root(&mut self, root: LocalLocation) {
        self.aliases
            .retain(|location, mapped_root| *location != root && *mapped_root != root);
        self.fields.retain(|(field_root, _), _| *field_root != root);
        self.field_scalars
            .retain(|(field_root, _), _| *field_root != root);
    }

    fn invalidate_roots(&mut self, roots: HashSet<LocalLocation>) {
        for root in roots {
            self.invalidate_root(root);
        }
    }
}

fn merge_typed_field_scalar_states<'a>(
    mut states: impl Iterator<Item = &'a TypedFieldScalarState>,
) -> TypedFieldScalarState {
    let Some(first) = states.next() else {
        return TypedFieldScalarState::default();
    };
    let rest = states.collect::<Vec<_>>();
    let mut merged = first.clone();
    merged.aliases.retain(|location, root| {
        rest.iter()
            .all(|state| state.aliases.get(location) == Some(root))
    });
    merged.value_aliases.retain(|location, value| {
        rest.iter()
            .all(|state| state.value_aliases.get(location) == Some(value))
    });
    merged.fields.retain(|field, value| {
        rest.iter()
            .all(|state| state.fields.get(field) == Some(value))
    });
    merged.field_scalars.retain(|field, value| {
        rest.iter()
            .all(|state| state.field_scalars.get(field) == Some(value))
    });
    merged
}

fn rekey_typed_field_scalar_map_root<T>(
    map: &mut HashMap<(LocalLocation, String), T>,
    old_root: LocalLocation,
    new_root: LocalLocation,
) {
    let entries = std::mem::take(map);
    *map = entries
        .into_iter()
        .map(|((root, field_name), value)| {
            let root = if root == old_root { new_root } else { root };
            ((root, field_name), value)
        })
        .collect();
}

fn typed_normal_block_predecessors(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, HashSet<BlockLabel>> {
    let mut predecessors = HashMap::<BlockLabel, HashSet<BlockLabel>>::new();
    for block in &function.blocks {
        for successor in typed_term_successors(&block.term) {
            predecessors
                .entry(successor)
                .or_default()
                .insert(block.label);
        }
    }
    predecessors
}

fn transfer_typed_field_scalar_block(
    block: &mut TypedBlock,
    state: &mut TypedFieldScalarState,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    let body = std::mem::take(&mut block.body);
    let mut new_body = Vec::with_capacity(body.len());
    for mut instr in body {
        let mut inserted_before = Vec::new();
        let mut inserted_after = Vec::new();
        transfer_typed_field_scalar_instr(
            &mut instr,
            state,
            module_constants,
            constructor_field_bindings,
            stats,
            rewrite,
            &mut inserted_before,
            &mut inserted_after,
        );
        if rewrite {
            new_body.extend(inserted_before);
        }
        new_body.push(instr);
        if rewrite {
            new_body.extend(inserted_after);
        }
    }
    block.body = new_body;
    transfer_typed_field_scalar_term(&mut block.term, state, module_constants, stats, rewrite);
}

fn transfer_typed_field_scalar_instr(
    instr: &mut InstrTyped,
    state: &mut TypedFieldScalarState,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
    inserted_before: &mut Vec<InstrTyped>,
    inserted_after: &mut Vec<InstrTyped>,
) {
    match instr {
        InstrTyped::Store(store) => {
            if let Some(bindings) =
                typed_constructor_field_bindings_for_store(store, constructor_field_bindings)
            {
                state.invalidate_roots(typed_virtual_roots_in_expr(store.value.as_ref(), state));
                if let Some(location) = store.name.local_location() {
                    state.seed_object(location, bindings);
                    if rewrite {
                        stats.seeded_objects += 1;
                        append_typed_constructor_field_scalar_stores(
                            inserted_after,
                            bindings,
                            state,
                            stats,
                        );
                    }
                }
                return;
            }
            rewrite_typed_field_scalar_expr(
                store.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            let Some(target) = store.name.local_location() else {
                return;
            };
            if let Some(source) = typed_instr_local_load_location(store.value.as_ref())
                && let Some(root) = state.root_for_location(source)
            {
                state.set_alias(target, root);
            } else if let InstrTyped::Load(source) = store.value.as_ref() {
                state.set_value_alias(target, &source.name);
            } else {
                state.rebind_local(target);
            }
        }
        InstrTyped::Del(del) => {
            if let Some(location) = del.name.local_location() {
                state.rebind_local(location);
            }
        }
        InstrTyped::SetAttrTyped(op) => rewrite_typed_field_scalar_setattr(
            op,
            state,
            module_constants,
            stats,
            rewrite,
            Some(inserted_before),
            Some(inserted_after),
        ),
        _ => rewrite_typed_field_scalar_expr(instr, state, module_constants, stats, rewrite),
    }
}

fn append_typed_constructor_field_scalar_stores(
    body: &mut Vec<InstrTyped>,
    bindings: &TypedConstructorFieldBindings,
    state: &TypedFieldScalarState,
    stats: &mut TypedFieldScalarizationStats,
) {
    for field in &bindings.fields {
        let Some(scalar) = field.scalar.as_ref() else {
            continue;
        };
        let source = state.resolve_scalar_name(&field.value);
        body.push(typed_store_temp(scalar.clone(), typed_load_temp(&source)));
        stats.inserted_scalar_stores += 1;
    }
}

fn rewrite_typed_field_scalar_setattr(
    op: &mut TypedSetAttr<InstrTyped>,
    state: &mut TypedFieldScalarState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
    inserted_before: Option<&mut Vec<InstrTyped>>,
    inserted_after: Option<&mut Vec<InstrTyped>>,
) {
    rewrite_typed_field_scalar_expr(
        op.replacement.as_mut(),
        state,
        module_constants,
        stats,
        rewrite,
    );
    state.invalidate_roots(typed_virtual_roots_in_expr(op.attr.as_ref(), state));
    let Some(receiver) = typed_instr_local_load_location(op.value.as_ref()) else {
        state.invalidate_roots(typed_virtual_roots_in_expr(op.value.as_ref(), state));
        return;
    };
    let Some(root) = state.root_for_location(receiver) else {
        return;
    };
    let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants) else {
        state.invalidate_root(root);
        return;
    };
    if !typed_attr_access_is_indexed_field(&op.access) {
        state.invalidate_root(root);
        return;
    }
    if let Some(scalar) = state.field_scalar(root, field_name).cloned() {
        if let Some(replacement) = typed_scalar_field_replacement_name(op.replacement.as_ref()) {
            if rewrite && let Some(inserted_after) = inserted_after {
                inserted_after.push(typed_store_temp(
                    scalar.clone(),
                    typed_load_temp(replacement),
                ));
                stats.inserted_scalar_stores += 1;
            }
            state.set_field(root, field_name.to_string(), scalar);
        } else if typed_scalar_field_can_precompute_replacement(op.replacement.as_ref()) {
            if rewrite && let Some(inserted_before) = inserted_before {
                let replacement =
                    std::mem::replace(op.replacement.as_mut(), typed_load_temp(&scalar));
                inserted_before.push(typed_store_temp(scalar.clone(), replacement));
                stats.inserted_scalar_stores += 1;
            }
            state.set_field(root, field_name.to_string(), scalar);
        } else {
            state.invalidate_field(root, field_name);
        }
    } else if let Some(replacement) = typed_scalar_field_replacement_name(op.replacement.as_ref()) {
        state.set_field(root, field_name.to_string(), replacement.clone());
    } else {
        state.invalidate_field(root, field_name);
    }
}

fn typed_constructor_field_bindings_for_store<'a>(
    store: &Store<InstrTyped>,
    constructor_field_bindings: &'a HashMap<InstrId, TypedConstructorFieldBindings>,
) -> Option<&'a TypedConstructorFieldBindings> {
    let InstrTyped::CallTyped(call) = store.value.as_ref() else {
        return None;
    };
    let instr_id = call.try_semantic_instr_id()?;
    constructor_field_bindings.get(&instr_id)
}

fn transfer_typed_field_scalar_term(
    term: &mut BlockTerm<InstrTyped>,
    state: &mut TypedFieldScalarState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    match term {
        BlockTerm::IfTerm(if_term) => {
            rewrite_typed_field_scalar_expr(
                &mut if_term.test,
                state,
                module_constants,
                stats,
                rewrite,
            );
            if !matches!(if_term.test, InstrTyped::DirectCallGuardTest(_)) {
                state.invalidate_roots(typed_virtual_roots_in_expr(&if_term.test, state));
            }
        }
        BlockTerm::BranchTable(branch) => {
            rewrite_typed_field_scalar_expr(
                &mut branch.index,
                state,
                module_constants,
                stats,
                rewrite,
            );
            state.invalidate_roots(typed_virtual_roots_in_expr(&branch.index, state));
        }
        BlockTerm::Raise(raise) => {
            if let Some(exc) = raise.exc.as_mut() {
                rewrite_typed_field_scalar_expr(exc, state, module_constants, stats, rewrite);
                state.invalidate_roots(typed_virtual_roots_in_expr(exc, state));
            }
        }
        BlockTerm::Return(value) => {
            rewrite_typed_field_scalar_expr(value, state, module_constants, stats, rewrite);
            state.invalidate_roots(typed_virtual_roots_in_expr(value, state));
        }
        BlockTerm::Jump(_) => {}
    }
}

fn rewrite_typed_field_scalar_expr(
    expr: &mut InstrTyped,
    state: &mut TypedFieldScalarState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    match expr {
        InstrTyped::Load(_) | InstrTyped::IncrementCounter(_) | InstrTyped::CellRef(_) => {}
        InstrTyped::GetAttrTyped(op) => {
            rewrite_typed_field_scalar_getattr(op, state, module_constants, stats, rewrite);
            if let Some(replacement) =
                typed_field_scalar_getattr_replacement(op, state, module_constants)
            {
                if rewrite {
                    stats.rewritten_loads += 1;
                }
                *expr = typed_load_temp(&replacement);
            }
        }
        InstrTyped::SetAttrTyped(op) => {
            rewrite_typed_field_scalar_setattr(
                op,
                state,
                module_constants,
                stats,
                rewrite,
                None,
                None,
            );
        }
        InstrTyped::DirectCallGuardTest(op) => {
            rewrite_typed_field_scalar_expr(
                op.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        _ => {
            rewrite_typed_field_scalar_children(expr, state, module_constants, stats, rewrite);
            state.invalidate_roots(typed_virtual_roots_in_expr(expr, state));
        }
    }
}

fn rewrite_typed_field_scalar_getattr(
    op: &mut TypedGetAttr<InstrTyped>,
    state: &mut TypedFieldScalarState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    if typed_instr_local_load_location(op.value.as_ref()).is_none() {
        rewrite_typed_field_scalar_expr(op.value.as_mut(), state, module_constants, stats, rewrite);
    }
    rewrite_typed_field_scalar_expr(op.attr.as_mut(), state, module_constants, stats, rewrite);
    if typed_field_scalar_getattr_replacement(op, state, module_constants).is_some() {
        return;
    }
    if let Some(receiver) = typed_instr_local_load_location(op.value.as_ref())
        && let Some(root) = state.root_for_location(receiver)
    {
        if typed_attr_access_is_indexed_field(&op.access)
            && let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants)
        {
            state.invalidate_field(root, field_name);
            return;
        }
        state.invalidate_root(root);
    } else {
        state.invalidate_roots(typed_virtual_roots_in_expr(op.value.as_ref(), state));
    }
}

fn typed_field_scalar_getattr_replacement(
    op: &TypedGetAttr<InstrTyped>,
    state: &TypedFieldScalarState,
    module_constants: &[ConstantExpr],
) -> Option<ResolvedName> {
    if !typed_attr_access_is_indexed_field(&op.access) {
        return None;
    }
    let receiver = typed_instr_local_load_location(op.value.as_ref())?;
    let root = state.root_for_location(receiver)?;
    let field_name = typed_constant_string(op.attr.as_ref(), module_constants)?;
    state.field_value(root, field_name).cloned()
}

fn typed_attr_access_is_indexed_field(access: &TypedAttrAccessPlan) -> bool {
    matches!(access, TypedAttrAccessPlan::IndexedField { .. })
}

fn typed_scalar_field_replacement_name(expr: &InstrTyped) -> Option<&ResolvedName> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    load.name.local_location()?;
    Some(&load.name)
}

fn typed_scalar_field_can_precompute_replacement(expr: &InstrTyped) -> bool {
    match expr {
        InstrTyped::Load(_) => true,
        InstrTyped::BinOp(op) => {
            typed_scalar_field_can_precompute_replacement(op.left.as_ref())
                && typed_scalar_field_can_precompute_replacement(op.right.as_ref())
        }
        InstrTyped::UnaryOp(op) => {
            typed_scalar_field_can_precompute_replacement(op.operand.as_ref())
        }
        InstrTyped::Tuple(op) => op
            .values
            .iter()
            .all(typed_scalar_field_can_precompute_replacement),
        _ => false,
    }
}

fn rewrite_typed_field_scalar_children(
    expr: &mut InstrTyped,
    state: &mut TypedFieldScalarState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    match expr {
        InstrTyped::Truthy(op) => rewrite_typed_field_scalar_expr(
            op.value.as_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        ),
        InstrTyped::BinOp(op) => {
            rewrite_typed_field_scalar_expr(
                op.left.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.right.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::Tuple(op) => {
            for value in &mut op.values {
                rewrite_typed_field_scalar_expr(value, state, module_constants, stats, rewrite);
            }
        }
        InstrTyped::UnaryOp(op) => rewrite_typed_field_scalar_expr(
            op.operand.as_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        ),
        InstrTyped::CalleeFunctionId(op) => rewrite_typed_field_scalar_expr(
            op.value.as_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        ),
        InstrTyped::CallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.func.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_call_args(
                &mut op.args,
                &mut op.keywords,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::GuardedCallableCallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.func.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_call_args(
                &mut op.args,
                &mut op.keywords,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::GuardedMethodCallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.func.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_call_args(
                &mut op.args,
                &mut op.keywords,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::DirectCallableCallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.func.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_positional_args(
                &mut op.args,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::DirectMethodCallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.receiver.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_positional_args(
                &mut op.args,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::CallDirect(op) => {
            rewrite_typed_field_scalar_expr(
                op.callable.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_call_args(
                &mut op.args,
                &mut op.keywords,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::GetItem(op) => {
            rewrite_typed_field_scalar_expr(
                op.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.index.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::SetItem(op) => {
            rewrite_typed_field_scalar_expr(
                op.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.index.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.replacement.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::DelItem(op) => {
            rewrite_typed_field_scalar_expr(
                op.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.index.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::Store(op) => rewrite_typed_field_scalar_expr(
            op.value.as_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        ),
        InstrTyped::MakeFunctionWithClosure(op) => {
            rewrite_typed_field_scalar_expr(
                op.captures.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.param_defaults.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.annotate_fn.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::Load(_)
        | InstrTyped::GetAttrTyped(_)
        | InstrTyped::SetAttrTyped(_)
        | InstrTyped::DirectCallGuardTest(_)
        | InstrTyped::Del(_)
        | InstrTyped::MakeCell(_)
        | InstrTyped::IncrementCounter(_)
        | InstrTyped::CellRef(_) => {}
    }
}

fn rewrite_typed_field_scalar_call_args(
    args: &mut [CallArgPositional<InstrTyped>],
    keywords: &mut [CallArgKeyword<InstrTyped>],
    state: &mut TypedFieldScalarState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    rewrite_typed_field_scalar_positional_args(args, state, module_constants, stats, rewrite);
    for keyword in keywords {
        rewrite_typed_field_scalar_expr(
            keyword.expr_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        );
    }
}

fn rewrite_typed_field_scalar_positional_args(
    args: &mut [CallArgPositional<InstrTyped>],
    state: &mut TypedFieldScalarState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    for arg in args {
        rewrite_typed_field_scalar_expr(arg.expr_mut(), state, module_constants, stats, rewrite);
    }
}

fn typed_virtual_roots_in_expr(
    expr: &InstrTyped,
    state: &TypedFieldScalarState,
) -> HashSet<LocalLocation> {
    struct Collector<'a> {
        state: &'a TypedFieldScalarState,
        roots: HashSet<LocalLocation>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(location) = typed_instr_local_load_location(expr)
                && let Some(root) = self.state.root_for_location(location)
            {
                self.roots.insert(root);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        state,
        roots: HashSet::new(),
    };
    collector.visit_instr(expr);
    collector.roots
}

fn typed_constant_string<'a>(
    expr: &InstrTyped,
    module_constants: &'a [ConstantExpr],
) -> Option<&'a str> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    let constant_index = load.name.location.as_constant()? as usize;
    match module_constants.get(constant_index)? {
        ConstantExpr::Literal(value) => match value.as_literal() {
            Literal::StringLiteral(value) => Some(value.value.as_str()),
            Literal::BytesLiteral(_) | Literal::NumberLiteral(_) => None,
        },
        ConstantExpr::RuntimeName(_) => None,
    }
}

fn find_typed_inline_candidate(
    block: &TypedBlock,
    caller_id: RuntimeFunctionId,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
) -> Option<TypedInlineStoreCandidate> {
    block
        .body
        .iter()
        .enumerate()
        .find_map(|(instr_index, instr)| {
            let InstrTyped::Store(store) = instr else {
                if let InstrTyped::GuardedCallableCallTyped(call) = instr {
                    return typed_inline_candidate_for_callable_call(
                        instr_index,
                        TypedInlineResult::EffectOnly,
                        call,
                        caller_id,
                        direct_calls_by_instr_id,
                    );
                }
                if let InstrTyped::GuardedMethodCallTyped(call) = instr {
                    return typed_inline_candidate_for_method_call(
                        instr_index,
                        TypedInlineResult::EffectOnly,
                        call,
                        caller_id,
                        direct_calls_by_instr_id,
                    );
                }
                if let InstrTyped::CallTyped(call) = instr
                    && let Some(candidate) = typed_inline_candidate_for_runtime_protocol_call(
                        instr_index,
                        TypedInlineResult::EffectOnly,
                        call,
                        caller_id,
                        direct_calls_by_instr_id,
                    )
                {
                    return Some(candidate);
                }
                return None;
            };
            match store.value.as_ref() {
                InstrTyped::GuardedCallableCallTyped(call) => {
                    typed_inline_candidate_for_callable_call(
                        instr_index,
                        TypedInlineResult::StoreTo(store.name.clone()),
                        call,
                        caller_id,
                        direct_calls_by_instr_id,
                    )
                }
                InstrTyped::GuardedMethodCallTyped(call) => typed_inline_candidate_for_method_call(
                    instr_index,
                    TypedInlineResult::StoreTo(store.name.clone()),
                    call,
                    caller_id,
                    direct_calls_by_instr_id,
                ),
                InstrTyped::CallTyped(call) => typed_inline_candidate_for_runtime_protocol_call(
                    instr_index,
                    TypedInlineResult::StoreTo(store.name.clone()),
                    call,
                    caller_id,
                    direct_calls_by_instr_id,
                ),
                _ => None,
            }
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

fn runtime_protocol_explicit_args(
    call: &TypedCall<InstrTyped>,
) -> Option<&[CallArgPositional<InstrTyped>]> {
    match &call.access {
        TypedCallAccessPlan::GuardedRuntimeProtocolMethod { .. } => {}
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
) -> Option<TypedInlineStoreCandidate> {
    let TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
        runtime_name: _,
        method_name: _,
        method_guards,
    } = &call.access
    else {
        return None;
    };
    let instr_id = call.try_semantic_instr_id()?;
    let receiver = runtime_protocol_receiver(call)?.clone();
    typed_positional_arg_exprs(runtime_protocol_explicit_args(call)?.to_vec())?;
    let plans = direct_calls_by_instr_id.get(&instr_id)?;
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
    then_label: BlockLabel,
    else_label: BlockLabel,
) -> BlockTerm<InstrTyped> {
    let mut guard = TypedDirectCallGuardTest::new(
        typed_load_temp(callable_temp),
        TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id },
    );
    guard.extra.set_guard_miss_deopt_enabled(true);
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
        (TypedInlineGuardPlan::Callable, TypedInlineCall::Callable(_)) => {
            let callable_temp = callable_temp
                .expect("callable inline guard requires callable temp")
                .resolved_name();
            typed_direct_call_guard_term(
                &callable_temp,
                plan.target,
                source_meta,
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
        TypedInlineCall::Callable(_) => {
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
        Del::new(temp_name.clone(), false)
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
                TypedInlineCall::Method { .. } | TypedInlineCall::RuntimeProtocolMethod { .. }
            )),
    );
    if matches!(
        call,
        TypedInlineCall::Method { .. } | TypedInlineCall::RuntimeProtocolMethod { .. }
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
    return_target: ResolvedName,
    inline_instance: u32,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    callee_module_constants: Option<&[ConstantExpr]>,
) -> Result<TypedInlineFragment, TypedInlineUnsupportedReason> {
    if callee.blocks.len() == 1 {
        return build_single_block_typed_inline_fragment_to_target(
            caller,
            callee,
            continuation,
            value_bindings,
            return_target,
            inline_instance,
            instr_id_allocator,
            caller_module_constants,
            callee_module_constants,
        );
    }
    build_multi_block_typed_inline_fragment_to_target(
        caller,
        callee,
        continuation,
        value_bindings,
        return_target,
        inline_instance,
        instr_id_allocator,
        caller_module_constants,
        callee_module_constants,
    )
}

struct TypedInlineFragment {
    blocks: Vec<TypedBlock>,
    instr_id_mappings: Vec<TypedInlineInstrIdMapping>,
    local_mappings: Vec<TypedInlineLocalMapping>,
}

fn build_single_block_typed_inline_fragment_to_target(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    callee: &BlockPyFunction<TypedBlockPyModuleShape>,
    continuation: BlockLabel,
    value_bindings: &TypedInlineValueBindings,
    return_target: ResolvedName,
    inline_instance: u32,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    callee_module_constants: Option<&[ConstantExpr]>,
) -> Result<TypedInlineFragment, TypedInlineUnsupportedReason> {
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    if typed_inline_callee_has_closure_storage(callee_layout) {
        return Err(TypedInlineUnsupportedReason::ClosureStorage);
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

    let locals = allocate_typed_inline_locals(caller, callee_layout, value_bindings)?;
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
        &locals,
        value_bindings,
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
    body.push(
        Store::new(return_target, Box::new(return_value))
            .with_meta(return_meta)
            .into(),
    );

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
        local_mappings,
    })
}

fn build_multi_block_typed_inline_fragment_to_target(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    callee: &BlockPyFunction<TypedBlockPyModuleShape>,
    continuation: BlockLabel,
    value_bindings: &TypedInlineValueBindings,
    return_target: ResolvedName,
    inline_instance: u32,
    instr_id_allocator: &mut TypedInlineInstrIdAllocator,
    caller_module_constants: Option<&mut Vec<ConstantExpr>>,
    callee_module_constants: Option<&[ConstantExpr]>,
) -> Result<TypedInlineFragment, TypedInlineUnsupportedReason> {
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    if typed_inline_callee_has_closure_storage(callee_layout) {
        return Err(TypedInlineUnsupportedReason::ClosureStorage);
    }
    for location in value_bindings.keys().copied() {
        if location.slot() as usize >= callee_layout.stack_slots().len() {
            return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(location));
        }
    }
    for block in &callee.blocks {
        if !block.params.is_empty() {
            return Err(TypedInlineUnsupportedReason::BlockParams);
        }
        if block.exc_edge.is_some() {
            return Err(TypedInlineUnsupportedReason::ExceptionEdge);
        }
        if typed_term_has_jump_args(&block.term) {
            return Err(TypedInlineUnsupportedReason::JumpArgs);
        }
    }

    let locals = allocate_typed_inline_locals(caller, callee_layout, value_bindings)?;
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
        &locals,
        value_bindings,
        &mut instr_id_remapper,
        &mut constant_scope,
    );
    let mut blocks: Vec<TypedBlock> = Vec::with_capacity(callee.blocks.len());
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
                body.push(
                    Store::new(return_target.clone(), Box::new(return_value))
                        .with_meta(return_meta)
                        .into(),
                );
                BlockTerm::Jump(BlockEdge::new(continuation))
            }
            term => {
                typed_remap_inline_term_labels(remapper.try_map_term(term.clone())?, &label_map)?
            }
        };
        blocks.push(Block::new_with_extra(
            label,
            body,
            term,
            Vec::new(),
            None,
            callee_block.extra.clone(),
        ));
    }
    Ok(TypedInlineFragment {
        blocks,
        instr_id_mappings: instr_id_remapper.finish(),
        local_mappings,
    })
}

fn typed_inline_callee_has_closure_storage(
    storage_layout: &soac_core::block_py::StorageLayout,
) -> bool {
    !storage_layout.freevars.is_empty()
        || !storage_layout.cellvars.is_empty()
        || !storage_layout.runtime_cells.is_empty()
}

fn allocate_typed_inline_locals(
    caller: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    callee_layout: &soac_core::block_py::StorageLayout,
    value_bindings: &TypedInlineValueBindings,
) -> Result<HashMap<LocalLocation, TypedTempLocal>, TypedInlineUnsupportedReason> {
    let mut locals = HashMap::new();
    for (slot, _name) in callee_layout.stack_slots().iter().enumerate() {
        let location =
            LocalLocation(u32::try_from(slot).expect("callee stack slot index should fit in u32"));
        if value_bindings.contains_key(&location) {
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

fn typed_term_has_jump_args(term: &BlockTerm<InstrTyped>) -> bool {
    match term {
        BlockTerm::Jump(edge) => !edge.args.is_empty(),
        BlockTerm::IfTerm(_)
        | BlockTerm::BranchTable(_)
        | BlockTerm::Raise(_)
        | BlockTerm::Return(_) => false,
    }
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
) -> Result<BlockTerm<InstrTyped>, TypedInlineUnsupportedReason> {
    Ok(match term {
        BlockTerm::Jump(edge) => BlockTerm::Jump(BlockEdge::new(typed_remapped_label(
            label_map,
            edge.target,
        )?)),
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
    locals: &'locals HashMap<LocalLocation, TypedTempLocal>,
    value_bindings: &'bindings TypedInlineValueBindings,
    instr_id_remapper: &'remapper mut TypedInlineInstrIdRemapper<'allocator>,
    constant_scope: &'remapper mut TypedInlineConstantScope<'constants>,
}

impl<'locals, 'bindings, 'constants, 'remapper, 'allocator>
    TypedInlineLocalRemapper<'locals, 'bindings, 'constants, 'remapper, 'allocator>
{
    fn new(
        locals: &'locals HashMap<LocalLocation, TypedTempLocal>,
        value_bindings: &'bindings TypedInlineValueBindings,
        instr_id_remapper: &'remapper mut TypedInlineInstrIdRemapper<'allocator>,
        constant_scope: &'remapper mut TypedInlineConstantScope<'constants>,
    ) -> Self {
        Self {
            locals,
            value_bindings,
            instr_id_remapper,
            constant_scope,
        }
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
            InstrTyped::CellRef(op) => InstrTyped::CellRef(op),
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
    use crate::passes::infer_module_value_facts;
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

    fn constructor_call_stores_in_virtual_plan(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &[ConstantExpr],
        plan: &TypedVirtualConstructorPlan,
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
        plan: &TypedVirtualConstructorPlan,
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
        plan: &TypedVirtualConstructorPlan,
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
    fn typed_field_scalar_state_promotes_root_when_original_local_is_deleted() {
        let root = LocalLocation(1);
        let alias = LocalLocation(2);
        let scalar = typed_test_local("scalar_current", LocalLocation(3));
        let mut state = TypedFieldScalarState::default();
        state.seed_object(
            root,
            &TypedConstructorFieldBindings {
                fields: vec![TypedConstructorFieldBinding {
                    field_name: "current".to_string(),
                    value: typed_test_local("current", LocalLocation(4)),
                    scalar: Some(scalar.clone()),
                }],
            },
        );
        state.set_alias(alias, root);

        state.rebind_local(root);

        assert_eq!(state.root_for_location(root), None);
        assert_eq!(state.root_for_location(alias), Some(alias));
        assert_eq!(state.field_value(alias, "current"), Some(&scalar));
        assert_eq!(state.field_scalar(alias, "current"), Some(&scalar));
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
        let split_stats = split_typed_alias_hot_continuations(caller);
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
            split_typed_alias_hot_continuations(caller).cloned_blocks,
            0,
            "a hot alias path whose successor is already private should not be cloned again"
        );
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

        let scalar_stats = scalarize_typed_hot_constructor_field_loads(
            caller,
            &module_constants,
            &constructor_field_bindings,
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
        let scalar_stats = scalarize_typed_hot_constructor_field_loads(
            caller,
            &module_constants,
            &constructor_field_bindings,
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
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let constructor_inline_stats = inline_typed_function_direct_call_stores(
            caller,
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
        let alias_split = split_typed_alias_hot_continuations(caller);
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
        let hot_loop_clones = hot_continuation_clones
            .iter()
            .copied()
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

        let scalar_stats = scalarize_typed_hot_constructor_field_loads_with_bindings(
            caller,
            &module_constants,
            &mut constructor_field_bindings,
        );
        assert_eq!(scalar_stats.seeded_objects, 1);
        assert_eq!(scalar_stats.scalar_slots, 3);
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
        let virtual_plans =
            plan_typed_virtual_constructors(caller, &module_constants, &constructor_field_bindings);
        let virtual_plan = virtual_plans
            .iter()
            .find(|plan| {
                constructor_call_stores_in_virtual_plan(caller, &module_constants, plan) > 0
                    && direct_call_guards_in_virtual_plan(caller, plan) > 0
            })
            .cloned()
            .expect("hot cloned iterator path should have a virtual constructor candidate");
        assert!(
            setattrs_for_field_in_virtual_plan(caller, &module_constants, &virtual_plan, "current")
                > 0,
            "before virtualizing, the hot iterator clone should still store current on the object"
        );
        let virtual_stats = virtualize_typed_hot_constructor_objects(
            caller,
            &module_constants,
            &constructor_field_bindings,
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
