use crate::artifacts_v3::ExactIntBranchV3Artifacts;
use soac_core::block_py::{InstrId, PersistentFunctionId, RuntimeFunctionId};
use soac_ir_typed::plan_v3::{
    CallBodyKind, CallBodyPlan, DirectCallArgPlan, DirectCallArgSource, DirectCallCallee,
};
use soac_ir_typed::{
    TypedCallEmissionPlan, TypedCallEmissionPlans, TypedDirectCallArgPlan,
    TypedDirectCallArgSource, TypedDirectFunctionCallGuard,
};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedV3DirectCallPlan {
    pub source: InstrId,
    pub target: RuntimeFunctionId,
    pub callee: DirectCallCallee,
    pub arg_plan: TypedDirectCallArgPlan,
    pub body: CallBodyPlan,
    pub reason: String,
}

pub fn typed_direct_call_arg_plan_from_v3(plan: &DirectCallArgPlan) -> TypedDirectCallArgPlan {
    TypedDirectCallArgPlan {
        sources: plan
            .sources
            .iter()
            .map(|source| match source {
                DirectCallArgSource::Provided(index) => {
                    TypedDirectCallArgSource::Provided(*index as usize)
                }
                DirectCallArgSource::PackedRest { start } => TypedDirectCallArgSource::PackedRest {
                    start: *start as usize,
                },
                DirectCallArgSource::DefaultSentinel => TypedDirectCallArgSource::DefaultSentinel,
            })
            .collect(),
    }
}

pub fn direct_calls_for_function_from_artifacts(
    artifacts: &ExactIntBranchV3Artifacts,
    mut resolve_target: impl FnMut(PersistentFunctionId) -> Result<Option<RuntimeFunctionId>, String>,
) -> Result<Option<HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>, String> {
    let emitted_function = &artifacts.emission.functions[0];
    if emitted_function.direct_calls.is_empty() {
        return Ok(None);
    }

    let mut direct_calls = HashMap::<InstrId, Vec<ResolvedV3DirectCallPlan>>::new();
    for direct_call in &emitted_function.direct_calls {
        let persistent_target = artifacts
            .plan
            .identity_tables
            .persistent_function_id(direct_call.target)
            .map_err(|err| {
                format!(
                    "optimization plan v3 emitted direct-call target {} at {} cannot resolve identity: {err}",
                    direct_call.target, direct_call.source
                )
            })?;
        let Some(target) = resolve_target(persistent_target.clone())? else {
            return Err(format!(
                "optimization plan v3 emitted direct-call target {} ({:?}) at {} does not exist in loaded module set",
                direct_call.target, persistent_target, direct_call.source
            ));
        };
        let plans = direct_calls.entry(direct_call.source).or_default();
        if !plans.iter().any(|plan| plan.target == target) {
            plans.push(ResolvedV3DirectCallPlan {
                source: direct_call.source,
                target,
                callee: direct_call.callee.clone(),
                arg_plan: typed_direct_call_arg_plan_from_v3(&direct_call.arg_plan),
                body: direct_call.body.clone(),
                reason: direct_call.reason.clone(),
            });
        }
    }
    Ok(Some(direct_calls))
}

pub fn direct_call_targets(
    direct_calls_by_source: &HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>,
) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
    direct_calls_by_source
        .iter()
        .map(|(source, direct_calls)| {
            (
                *source,
                direct_calls
                    .iter()
                    .map(|direct_call| direct_call.target)
                    .collect(),
            )
        })
        .collect()
}

pub fn inline_direct_call_targets(
    direct_calls_by_source: &HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>,
) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
    direct_calls_by_source
        .iter()
        .filter_map(|(source, direct_calls)| {
            let targets = direct_calls
                .iter()
                .filter(|direct_call| direct_call.body.kind == CallBodyKind::Inline)
                .map(|direct_call| direct_call.target)
                .collect::<Vec<_>>();
            (!targets.is_empty()).then_some((*source, targets))
        })
        .collect()
}

pub fn direct_call_body_plans(
    direct_calls_by_source: &HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>,
) -> HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>> {
    direct_calls_by_source
        .iter()
        .filter_map(|(source, direct_calls)| {
            let plans = direct_calls
                .iter()
                .filter(|direct_call| direct_call.body.kind == CallBodyKind::DirectCall)
                .cloned()
                .collect::<Vec<_>>();
            (!plans.is_empty()).then_some((*source, plans))
        })
        .collect()
}

pub fn typed_call_emission_plans_from_v3(
    direct_calls_by_instr: &HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>,
) -> Result<TypedCallEmissionPlans, String> {
    let mut by_source = HashMap::<InstrId, TypedCallEmissionPlan>::new();
    for (source, direct_calls) in direct_calls_by_instr {
        let plan = by_source
            .entry(*source)
            .or_insert_with(|| TypedCallEmissionPlan::Callable {
                function_guards: Vec::new(),
            });
        let TypedCallEmissionPlan::Callable {
            function_guards, ..
        } = plan
        else {
            return Err(format!(
                "direct-call emission source {source:?} already has non-callable plan"
            ));
        };
        function_guards.extend(direct_calls.iter().map(|direct_call| {
            TypedDirectFunctionCallGuard {
                function_id: direct_call.target,
                arg_plan: direct_call.arg_plan.clone(),
            }
        }));
    }
    Ok(TypedCallEmissionPlans { by_source })
}
