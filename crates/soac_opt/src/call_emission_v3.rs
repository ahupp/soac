use crate::plan_v3::{
    CallBodyKind, CallBodyPlan, ConstructorCallFallbackKind, ConstructorCallGuardKind,
    ConstructorCallOwnerType, DirectCallArgPlan, DirectCallArgSource, MethodCallFallbackKind,
    MethodCallGuardKind, MethodCallOwnerType,
};
use soac_core::block_py::{InstrId, RuntimeFunctionId};
use soac_lowering::passes::{
    TypedCallEmissionPlan, TypedCallEmissionPlans, TypedDirectCallArgPlan,
    TypedDirectCallArgSource, TypedDirectConstructorCallGuard, TypedDirectFunctionCallGuard,
    TypedDirectMethodCallGuard,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedV3DirectCallPlan {
    pub source: InstrId,
    pub target: RuntimeFunctionId,
    pub arg_plan: TypedDirectCallArgPlan,
    pub body: CallBodyPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedV3ConstructorCallPlan {
    pub source: InstrId,
    pub target: RuntimeFunctionId,
    pub owner_type: ConstructorCallOwnerType,
    pub arg_plan: TypedDirectCallArgPlan,
    pub guard: ConstructorCallGuardKind,
    pub fallback: ConstructorCallFallbackKind,
    pub body: CallBodyPlan,
    pub inline_target: Option<RuntimeFunctionId>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedV3MethodCallPlan {
    pub source: InstrId,
    pub target: RuntimeFunctionId,
    pub method_name: String,
    pub owner_type: MethodCallOwnerType,
    pub arg_plan: TypedDirectCallArgPlan,
    pub guard: MethodCallGuardKind,
    pub fallback: MethodCallFallbackKind,
    pub body: CallBodyPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedV3ConstructorCallPlan {
    pub guards: Vec<TypedDirectConstructorCallGuard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedV3MethodCallPlan {
    pub method_name: String,
    pub guards: Vec<TypedDirectMethodCallGuard>,
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
                DirectCallArgSource::DefaultSentinel => TypedDirectCallArgSource::DefaultSentinel,
            })
            .collect(),
    }
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

pub fn constructor_call_targets(
    constructor_calls_by_source: &HashMap<InstrId, Vec<ResolvedV3ConstructorCallPlan>>,
) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
    constructor_calls_by_source
        .iter()
        .map(|(source, constructor_calls)| {
            (
                *source,
                constructor_calls
                    .iter()
                    .map(|constructor_call| constructor_call.target)
                    .collect(),
            )
        })
        .collect()
}

pub fn method_call_targets(
    method_calls_by_source: &HashMap<InstrId, Vec<ResolvedV3MethodCallPlan>>,
) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
    method_calls_by_source
        .iter()
        .map(|(source, method_calls)| {
            (
                *source,
                method_calls
                    .iter()
                    .map(|method_call| method_call.target)
                    .collect(),
            )
        })
        .collect()
}

pub fn inline_method_call_targets(
    method_calls_by_source: &HashMap<InstrId, Vec<ResolvedV3MethodCallPlan>>,
) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
    method_calls_by_source
        .iter()
        .filter_map(|(source, method_calls)| {
            let targets = method_calls
                .iter()
                .filter(|method_call| method_call.body.kind == CallBodyKind::Inline)
                .map(|method_call| method_call.target)
                .collect::<Vec<_>>();
            (!targets.is_empty()).then_some((*source, targets))
        })
        .collect()
}

pub fn merge_call_target_specializations(
    mut left: HashMap<InstrId, Vec<RuntimeFunctionId>>,
    right: HashMap<InstrId, Vec<RuntimeFunctionId>>,
) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
    for (source, targets) in right {
        let entry = left.entry(source).or_default();
        for target in targets {
            if !entry.contains(&target) {
                entry.push(target);
            }
        }
    }
    left
}

pub fn prepare_constructor_call_plans_for_codegen(
    constructor_calls_by_instr: &HashMap<InstrId, Vec<ResolvedV3ConstructorCallPlan>>,
    mut guard_from_plan: impl FnMut(
        &ResolvedV3ConstructorCallPlan,
    ) -> Result<TypedDirectConstructorCallGuard, String>,
    mut validate_guard: impl FnMut(&TypedDirectConstructorCallGuard) -> Result<(), String>,
) -> Result<HashMap<InstrId, PreparedV3ConstructorCallPlan>, String> {
    let mut prepared = HashMap::new();
    for (source, constructor_calls) in constructor_calls_by_instr {
        let constructor_calls = constructor_calls
            .iter()
            .filter(|constructor_call| constructor_call.body.kind == CallBodyKind::DirectCall)
            .collect::<Vec<_>>();
        if constructor_calls.is_empty() {
            continue;
        }
        let mut guards = Vec::new();
        for constructor_call in constructor_calls {
            let guard = guard_from_plan(constructor_call)?;
            validate_guard(&guard)?;
            if !guards.contains(&guard) {
                guards.push(guard);
            }
        }
        prepared.insert(*source, PreparedV3ConstructorCallPlan { guards });
    }
    Ok(prepared)
}

pub fn prepare_method_call_plans_for_codegen(
    method_calls_by_instr: &HashMap<InstrId, Vec<ResolvedV3MethodCallPlan>>,
    mut guard_from_plan: impl FnMut(
        &ResolvedV3MethodCallPlan,
    ) -> Result<TypedDirectMethodCallGuard, String>,
    mut validate_guard: impl FnMut(&TypedDirectMethodCallGuard, &str) -> Result<(), String>,
) -> Result<HashMap<InstrId, PreparedV3MethodCallPlan>, String> {
    let mut prepared = HashMap::new();
    for (source, method_calls) in method_calls_by_instr {
        let method_calls = method_calls
            .iter()
            .filter(|method_call| method_call.body.kind == CallBodyKind::DirectCall)
            .collect::<Vec<_>>();
        let Some(first_method_call) = method_calls.first() else {
            continue;
        };
        let method_name = first_method_call.method_name.clone();
        if let Some(mismatched_method) = method_calls
            .iter()
            .find(|method_call| method_call.method_name != method_name)
        {
            return Err(format!(
                "optimizer v3 emitted method-call at {} for method {}, but another plan uses {}",
                source, method_name, mismatched_method.method_name
            ));
        }
        let mut guards = Vec::new();
        for method_call in method_calls {
            let guard = guard_from_plan(method_call)?;
            validate_guard(&guard, &method_name)?;
            if !guards.contains(&guard) {
                guards.push(guard);
            }
        }
        prepared.insert(
            *source,
            PreparedV3MethodCallPlan {
                method_name,
                guards,
            },
        );
    }
    Ok(prepared)
}

pub fn typed_call_emission_plans_from_v3(
    direct_calls_by_instr: &HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>,
    constructor_calls_by_instr: &HashMap<InstrId, PreparedV3ConstructorCallPlan>,
    method_calls_by_instr: &HashMap<InstrId, PreparedV3MethodCallPlan>,
) -> Result<TypedCallEmissionPlans, String> {
    let mut by_source = HashMap::<InstrId, TypedCallEmissionPlan>::new();
    for (source, direct_calls) in direct_calls_by_instr {
        let plan = by_source
            .entry(*source)
            .or_insert_with(|| TypedCallEmissionPlan::Callable {
                function_guards: Vec::new(),
                constructor_guards: Vec::new(),
            });
        let TypedCallEmissionPlan::Callable {
            function_guards, ..
        } = plan
        else {
            return Err(format!(
                "optimizer v3 call source {source} has both direct and method emissions"
            ));
        };
        function_guards.extend(direct_calls.iter().map(|direct_call| {
            TypedDirectFunctionCallGuard {
                function_id: direct_call.target,
                arg_plan: direct_call.arg_plan.clone(),
            }
        }));
    }
    for (source, constructor_calls) in constructor_calls_by_instr {
        let plan = by_source
            .entry(*source)
            .or_insert_with(|| TypedCallEmissionPlan::Callable {
                function_guards: Vec::new(),
                constructor_guards: Vec::new(),
            });
        let TypedCallEmissionPlan::Callable {
            constructor_guards, ..
        } = plan
        else {
            return Err(format!(
                "optimizer v3 call source {source} has both constructor and method emissions"
            ));
        };
        constructor_guards.extend(constructor_calls.guards.clone());
    }
    for (source, method_calls) in method_calls_by_instr {
        if by_source.contains_key(source) {
            return Err(format!(
                "optimizer v3 call source {source} has both method and callable emissions"
            ));
        }
        by_source.insert(
            *source,
            TypedCallEmissionPlan::Method {
                method_name: method_calls.method_name.clone(),
                method_guards: method_calls.guards.clone(),
            },
        );
    }
    Ok(TypedCallEmissionPlans { by_source })
}

pub fn v3_call_emission_sources(
    direct_calls: Option<&HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
    constructor_calls: Option<&HashMap<InstrId, Vec<ResolvedV3ConstructorCallPlan>>>,
    method_calls: Option<&HashMap<InstrId, Vec<ResolvedV3MethodCallPlan>>>,
) -> HashSet<InstrId> {
    let mut sources = HashSet::new();
    if let Some(direct_calls) = direct_calls {
        sources.extend(direct_calls.keys().copied());
    }
    if let Some(constructor_calls) = constructor_calls {
        sources.extend(constructor_calls.keys().copied());
    }
    if let Some(method_calls) = method_calls {
        sources.extend(method_calls.keys().copied());
    }
    sources
}
