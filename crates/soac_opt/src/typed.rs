use crate::passes::value_facts;
#[allow(unused_imports)]
use soac_core::block_py;
#[allow(unused_imports)]
use soac_core::block_py::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule,
    BlockTerm, Call, CallArgKeyword, CallArgPositional, CallDirect, CalleeFunctionId,
    ChildVisitable, ConstantExpr, Del, HasMeta, HasSemanticInstrId, Instr, InstrId, InstrKey,
    InstrWithConstantNone, Load, LocalLocation, MapInstr, Mappable, Meta, NameLike, NameLocation,
    ParamKind, PrettyPrint, PrettyPrinter, ResolvedName, RuntimeFunctionId, RuntimeName, SetAttr,
    Store, TermIf, TryMapInstr, TryMapModule, TryMapTerm, Visit, VisitMut, WithMeta,
};
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};
use soac_ir_typed::emit_v3::MechanicalExitKind;
use soac_ir_typed::plan_v3::Rep;
use soac_ir_typed::{
    BoolFacts, FactStore, InstrTyped, PyObjFacts, TypedBlock, TypedBlockExtra,
    TypedBlockPyModuleShape, TypedCall, TypedCallAccessPlan, TypedCallEmissionPlan,
    TypedCallEmissionPlans, TypedDirectCallArgPlan, TypedDirectCallArgSource,
    TypedDirectCallGuardTest, TypedDirectCallGuardTestKind, TypedDirectCallableCall,
    TypedDirectCallableCallGuard, TypedDirectMethodCall, TypedDirectMethodCallGuard, TypedGetAttr,
    TypedGuardedCallableCall, TypedGuardedMethodCall, TypedPlannedResult,
    TypedPyObjectOwnershipPlan, TypedResultDemand, TypedTruthy, ValueFacts,
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
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[allow(dead_code)]
pub enum TypedInlineUnsupportedReason {
    MissingCallerStorageLayout,
    MissingCalleeStorageLayout,
    MissingCalleeLocal(LocalLocation),
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
    external_callees: &HashMap<RuntimeFunctionId, BlockPyFunction<TypedBlockPyModuleShape>>,
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
        let provided_values =
            typed_inline_provided_values(&candidate.call, &receiver_temp, &arg_temps);
        let Ok(bindings) = bind_typed_direct_call_inline_values(
            callee,
            &plan.arg_plan,
            provided_values.as_slice(),
        ) else {
            stats.skipped_candidates += 1;
            caller.storage_layout = original_storage_layout;
            return TypedInlineBlockRewrite::Unchanged(original_block);
        };
        let inline_instance = *next_inline_instance;
        *next_inline_instance = next_inline_instance
            .checked_add(1)
            .expect("typed inline instance count should fit in u32");
        let Ok(mut fragment) = build_typed_direct_call_inline_fragment_to_target(
            caller,
            callee,
            cleanup_label,
            &bindings,
            return_target.clone(),
            inline_instance,
            instr_id_allocator,
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

fn typed_inline_callee<'a>(
    module: &'a BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &'a HashMap<RuntimeFunctionId, BlockPyFunction<TypedBlockPyModuleShape>>,
    function_id: RuntimeFunctionId,
) -> Option<&'a BlockPyFunction<TypedBlockPyModuleShape>> {
    module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .or_else(|| external_callees.get(&function_id))
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
    BlockTerm::IfTerm(TermIf {
        test: InstrTyped::DirectCallGuardTest(
            TypedDirectCallGuardTest::new(
                typed_load_temp(callable_temp),
                TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id },
            )
            .with_meta(source_meta),
        ),
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
            BlockTerm::IfTerm(TermIf {
                test: InstrTyped::DirectCallGuardTest(
                    TypedDirectCallGuardTest::new(
                        typed_load_temp(&receiver_temp),
                        TypedDirectCallGuardTestKind::ExactTypeVersion {
                            function_id: plan.target,
                            owner_type_ref: guard.owner_type_ref.clone(),
                            type_version: guard.type_version,
                        },
                    )
                    .with_meta(source_meta),
                ),
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
        if !matches!(param.kind, ParamKind::PosOnly | ParamKind::Any) {
            return Err(TypedInlineUnsupportedReason::UnsupportedParameterKind);
        }
        let TypedDirectCallArgSource::Provided(index) = source else {
            return Err(TypedInlineUnsupportedReason::DefaultArguments);
        };
        let Some(value) = values.get(*index) else {
            return Err(TypedInlineUnsupportedReason::ArityMismatch);
        };
        let location = typed_parameter_local_location(callee, &param.name)?;
        bindings.insert(location, value.clone());
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
    let mut remapper =
        TypedInlineLocalRemapper::new(&locals, value_bindings, &mut instr_id_remapper);
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
    let mut remapper =
        TypedInlineLocalRemapper::new(&locals, value_bindings, &mut instr_id_remapper);
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
                let bound_name = typed_inline_value_binding_name(callee_location, value)?;
                let Some(location) = bound_name.local_location() else {
                    return Err(TypedInlineUnsupportedReason::NonLocalValueBinding(
                        callee_location,
                    ));
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

struct TypedInlineLocalRemapper<'locals, 'bindings, 'remapper, 'allocator> {
    locals: &'locals HashMap<LocalLocation, TypedTempLocal>,
    value_bindings: &'bindings TypedInlineValueBindings,
    instr_id_remapper: &'remapper mut TypedInlineInstrIdRemapper<'allocator>,
}

impl<'locals, 'bindings, 'remapper, 'allocator>
    TypedInlineLocalRemapper<'locals, 'bindings, 'remapper, 'allocator>
{
    fn new(
        locals: &'locals HashMap<LocalLocation, TypedTempLocal>,
        value_bindings: &'bindings TypedInlineValueBindings,
        instr_id_remapper: &'remapper mut TypedInlineInstrIdRemapper<'allocator>,
    ) -> Self {
        Self {
            locals,
            value_bindings,
            instr_id_remapper,
        }
    }
}

impl TryMapInstr<InstrTyped, InstrTyped, TypedInlineUnsupportedReason>
    for TypedInlineLocalRemapper<'_, '_, '_, '_>
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
    let InstrTyped::CallTyped(call) = expr else {
        return false;
    };
    if !typed_expr_is_runtime_name_load(
        call.func.as_ref(),
        RuntimeName::ExceptionMatches,
        module_constants,
    ) || !call.keywords.is_empty()
        || call.args.len() != 2
    {
        return false;
    }
    let Some(exc) = typed_positional_arg_expr(call.args.first()) else {
        return false;
    };
    let Some(expected) = typed_positional_arg_expr(call.args.get(1)) else {
        return false;
    };
    typed_expr_loads_name(exc, exception_name)
        && typed_expr_is_runtime_name_load(expected, RuntimeName::StopIteration, module_constants)
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
    for source in &plan.sources {
        match source {
            TypedDirectCallArgSource::Provided(index)
                if *index >= provided_positional_arg_count =>
            {
                return Err(format!(
                    "direct call arg plan references provided arg {index}, but only {provided_positional_arg_count} args are available"
                ));
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
    use soac_core::block_py::{ChildVisitable, InstrId, InstrWithConstantNone, Visit, VisitMut};
    use soac_ir_typed::{
        TypedAttrOwnerRef, TypedDirectFunctionCallGuard, TypedDirectMethodCallGuard,
        lower_blockpy_module_to_typed,
    };
    use std::collections::HashMap;

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
