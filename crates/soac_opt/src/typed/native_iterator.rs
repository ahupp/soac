//! Selection for the inline-only, canonical native-iterator template.
//!
//! Runtime-name membership discovers candidates; it does not authorize an
//! operation. Every committed template still guards the two evaluated callee
//! objects. The original expression remains the cold fallback, including its
//! ordinary callback and native input activation.

use soac_core::block_py::{
    BlockPyFunction, CallArgPositional, ChildVisitable, ConstantExpr, HasSemanticInstrId, InstrId,
    NameLocation, RuntimeName, Visit, VisitMut,
};
use soac_ir_typed::{
    InstrTyped, NativeIteratorMaterializer, NativeIteratorStage, TypedBlockPyModuleShape,
    TypedCall, TypedCallAccessPlan, TypedNativeIteratorPipelinePlan,
};
use std::collections::{HashMap, HashSet};

/// Conservative expansion reservation shared with the ordinary inline budget.
/// These are template costs, not the size of an unadmitted Python helper body.
pub const NATIVE_ITERATOR_PIPELINE_BLOCK_BUDGET: usize = 40;
pub const NATIVE_ITERATOR_PIPELINE_BODY_BUDGET: usize = 192;

pub struct NativeIteratorPipelineOperands<'a> {
    pub materializer_call: &'a TypedCall<InstrTyped>,
    pub stage_call: &'a TypedCall<InstrTyped>,
    pub callback: &'a InstrTyped,
    pub iterable: &'a InstrTyped,
}

fn runtime_name(expr: &InstrTyped, constants: &[ConstantExpr]) -> Option<RuntimeName> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    if matches!(
        load.name.location,
        NameLocation::GlobalName | NameLocation::Global(_)
    ) {
        // Only candidate membership. The live global/builtin lookup is still
        // evaluated once and both actual objects are guarded before effects.
        // In particular an appended global can shadow the builtin at any call.
        return RuntimeName::from_name(load.name.id.as_str());
    }
    load.name.runtime_name_id().or_else(|| {
        let index = load.name.location.as_constant()?;
        match constants.get(index as usize)? {
            ConstantExpr::RuntimeName(name) => Some(*name),
            _ => None,
        }
    })
}

fn generic_call(expr: &InstrTyped) -> Option<&TypedCall<InstrTyped>> {
    let InstrTyped::CallTyped(call) = expr else {
        return None;
    };
    (matches!(call.access, TypedCallAccessPlan::Generic)
        && call.keywords.is_empty()
        && call.frame_namespace.is_none()
        && call.extra.source_call.is_none()
        && call.extra.builtin_implementation_plan().is_none())
    .then_some(call)
}

fn candidate<'a>(
    expr: &'a InstrTyped,
    constants: &[ConstantExpr],
) -> Option<(
    TypedNativeIteratorPipelinePlan,
    NativeIteratorPipelineOperands<'a>,
)> {
    let materializer_call = generic_call(expr)?;
    let materializer = match runtime_name(&materializer_call.func, constants)? {
        RuntimeName::List => NativeIteratorMaterializer::List,
        RuntimeName::Tuple => NativeIteratorMaterializer::Tuple,
        _ => return None,
    };
    let [CallArgPositional::Positional(stage_expr)] = materializer_call.args.as_slice() else {
        return None;
    };
    let stage_call = generic_call(stage_expr)?;
    let stage = match runtime_name(&stage_call.func, constants)? {
        RuntimeName::Map => NativeIteratorStage::Map,
        RuntimeName::Filter => NativeIteratorStage::Filter,
        _ => return None,
    };
    let [
        CallArgPositional::Positional(callback),
        CallArgPositional::Positional(iterable),
    ] = stage_call.args.as_slice()
    else {
        return None;
    };
    let plan = TypedNativeIteratorPipelinePlan::proposal(
        materializer_call.try_semantic_instr_id()?,
        stage_call.try_semantic_instr_id()?,
        stage,
        materializer,
    );
    Some((
        plan,
        NativeIteratorPipelineOperands {
            materializer_call,
            stage_call,
            callback,
            iterable,
        },
    ))
}

/// Validate the fixed template against its actual, still-nested source calls.
/// This does not by itself prove that the origin is unique in the function.
pub fn native_iterator_pipeline_operands<'a>(
    expr: &'a InstrTyped,
    constants: &[ConstantExpr],
    plan: &TypedNativeIteratorPipelinePlan,
) -> Result<NativeIteratorPipelineOperands<'a>, String> {
    let Some((expected, operands)) = candidate(expr, constants) else {
        return Err(format!(
            "native iterator pipeline {:?} no longer has its closed native call operands",
            plan.source
        ));
    };
    if expected != *plan {
        return Err(format!(
            "native iterator pipeline {:?} has a stale template, guard, or use proof",
            plan.source
        ));
    }
    if plan.source == plan.stage_source {
        return Err("native iterator wrapper and consumer must have distinct origins".to_owned());
    }
    Ok(operands)
}

pub fn propose_typed_native_iterator_pipelines(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    constants: &[ConstantExpr],
) -> Vec<TypedNativeIteratorPipelinePlan> {
    struct Collector<'a> {
        constants: &'a [ConstantExpr],
        plans: Vec<TypedNativeIteratorPipelinePlan>,
    }
    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some((plan, _)) = candidate(expr, self.constants) {
                self.plans.push(plan);
            }
            expr.visit_children(self);
        }
    }
    let mut collector = Collector {
        constants,
        plans: Vec::new(),
    };
    collector.visit_fn(function);
    collector.plans
}

/// A direct tree edge is the version-one MustEliminate proof: the wrapper has
/// exactly one allocation origin and is the consumer's sole positional input.
/// An alias, observed wrapper, second origin, or changed guard rejects the whole
/// proposal set. No public/module membership or profile observation is a grant.
pub fn validate_typed_native_iterator_pipeline_plans(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    constants: &[ConstantExpr],
    plans: &[TypedNativeIteratorPipelinePlan],
) -> Result<(), String> {
    let mut requested = HashMap::new();
    let mut wrappers = HashSet::new();
    for plan in plans {
        if requested.insert(plan.source, plan).is_some() || !wrappers.insert(plan.stage_source) {
            return Err(
                "native iterator pipeline proposals overlap a consumer or wrapper origin"
                    .to_owned(),
            );
        }
    }
    struct Validator<'a> {
        constants: &'a [ConstantExpr],
        requested: &'a HashMap<InstrId, &'a TypedNativeIteratorPipelinePlan>,
        counts: HashMap<InstrId, usize>,
        error: Option<String>,
    }
    impl Visit<InstrTyped> for Validator<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(id) = expr.try_semantic_instr_id() {
                *self.counts.entry(id).or_default() += 1;
                if let Some(plan) = self.requested.get(&id) {
                    if let Err(error) =
                        native_iterator_pipeline_operands(expr, self.constants, plan)
                    {
                        self.error.get_or_insert(error);
                    }
                }
            }
            expr.visit_children(self);
        }
    }
    let mut validator = Validator {
        constants,
        requested: &requested,
        counts: HashMap::new(),
        error: None,
    };
    validator.visit_fn(function);
    if let Some(error) = validator.error {
        return Err(error);
    }
    for plan in plans {
        if validator.counts.get(&plan.source) != Some(&1)
            || validator.counts.get(&plan.stage_source) != Some(&1)
        {
            return Err(format!(
                "native iterator pipeline {:?} does not own unique current consumer/wrapper origins",
                plan.source
            ));
        }
    }
    Ok(())
}

/// Commit only after every proposal validates. A decline changes no instructions,
/// storage, IDs, plans, or telemetry. Successful replacement also removes stale
/// plans that no longer belong to the current function after other rewrites.
pub fn commit_typed_native_iterator_pipeline_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    constants: &[ConstantExpr],
    plans: &[TypedNativeIteratorPipelinePlan],
) -> Result<usize, String> {
    validate_typed_native_iterator_pipeline_plans(function, constants, plans)?;
    struct Commit<'a> {
        plans: HashMap<InstrId, &'a TypedNativeIteratorPipelinePlan>,
        changed: usize,
    }
    impl VisitMut<InstrTyped> for Commit<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            let plan = expr
                .try_semantic_instr_id()
                .and_then(|id| self.plans.get(&id))
                .copied();
            if let Some(extra) = expr.typed_extra_mut() {
                self.changed += usize::from(match plan {
                    Some(plan) => extra.set_native_iterator_pipeline_plan(plan.clone()),
                    None => extra.clear_native_iterator_pipeline_plan(),
                });
            }
            expr.visit_children_mut(self);
        }
    }
    let mut commit = Commit {
        plans: plans.iter().map(|plan| (plan.source, plan)).collect(),
        changed: 0,
    };
    commit.visit_fn_mut(function);
    Ok(commit.changed)
}

pub fn typed_native_iterator_pipeline_reserved_budget(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> (usize, usize) {
    #[derive(Default)]
    struct Count(usize);
    impl Visit<InstrTyped> for Count {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            self.0 += usize::from(expr.native_iterator_pipeline_plan().is_some());
            expr.visit_children(self);
        }
    }
    let mut count = Count::default();
    count.visit_fn(function);
    (
        count
            .0
            .saturating_mul(NATIVE_ITERATOR_PIPELINE_BLOCK_BUDGET),
        count.0.saturating_mul(NATIVE_ITERATOR_PIPELINE_BODY_BUDGET),
    )
}

/// Reserve expansion cost before the ordinary inliner spends its CFG budget.
pub fn select_typed_native_iterator_pipelines(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    constants: &[ConstantExpr],
    available_blocks: usize,
    available_body_instrs: usize,
) -> Result<usize, String> {
    let mut proposals = propose_typed_native_iterator_pipelines(function, constants);
    validate_typed_native_iterator_pipeline_plans(function, constants, &proposals)?;
    let count = (available_blocks / NATIVE_ITERATOR_PIPELINE_BLOCK_BUDGET)
        .min(available_body_instrs / NATIVE_ITERATOR_PIPELINE_BODY_BUDGET);
    proposals.truncate(count);
    commit_typed_native_iterator_pipeline_plans(function, constants, &proposals)?;
    Ok(proposals.len())
}
