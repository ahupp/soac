use crate::call_emission_v3::{
    ResolvedV3ConstructorCallPlan, ResolvedV3DirectCallPlan, ResolvedV3MethodCallPlan,
    constructor_call_targets, inline_direct_call_targets, inline_method_call_targets,
    merge_call_target_specializations,
};
use crate::plan_v3::CallBodyKind;
use soac_core::block_py::{
    BlockLabel, BlockPyFunction, BlockPyModule, InstrId, RuntimeFunctionId, RuntimeName, VisitMut,
};
use soac_lowering::passes::{
    CodegenModuleShape, DirectCallStoreRewriteStats, InlineCallee, InlinePlanModule,
    InlineRewriteStats, InstrCodegen, InstrResolved, ProfiledMethodInlineRewriteStats,
    ProfiledOwnerAttrKey, ProfiledOwnerAttrSpecialization, ProfiledRuntimeIterConstructorCall,
    ScalarReplacementStats, collect_profiled_runtime_iter_method_target_ids,
    inline_direct_call_stores_with_callees, plan_module_inlining,
    rewrite_profiled_function_call_store_sites,
    rewrite_profiled_function_call_store_sites_with_constructor_targets,
    rewrite_profiled_no_arg_method_call_store_sites,
    scalar_replace_non_escaping_constructor_allocations, summarize_module_escapes,
    validate_codegen_instr_ids,
};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy)]
pub struct V3CallInliningProfile<'a> {
    pub direct_calls_by_function:
        &'a HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
    pub constructor_calls_by_function:
        &'a HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3ConstructorCallPlan>>>,
    pub method_calls_by_function:
        &'a HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3MethodCallPlan>>>,
    pub exact_int_branch_function_ids: &'a HashSet<RuntimeFunctionId>,
}

#[derive(Debug, Clone)]
pub struct V3ExternalInlineTarget {
    pub function: soac_core::block_py::BlockPyFunction<CodegenModuleShape>,
    pub module_constants: Vec<InstrResolved>,
    pub inline_plan: InlinePlanModule,
}

#[derive(Debug, Clone, Default)]
pub struct V3CallInliningRewriteSummary {
    pub method_rewrites: Vec<V3MethodCallInlineRewriteReport>,
    pub direct_rewrites: Vec<V3DirectCallRewriteReport>,
    pub inline_callee_specializations: Vec<V3InlineCalleeSpecializationReport>,
    pub inline_rewrite: InlineRewriteStats,
    pub scalar_replacement: ScalarReplacementStats,
    pub runtime_constructor_function_ids: HashSet<RuntimeFunctionId>,
    pub runtime_constructor_inline_plan_hits: usize,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct V3MethodCallInlineRewriteReport {
    pub function_id: RuntimeFunctionId,
    pub qualname: String,
    pub stats: ProfiledMethodInlineRewriteStats,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct V3DirectCallRewriteReport {
    pub function_id: RuntimeFunctionId,
    pub qualname: String,
    pub stats: DirectCallStoreRewriteStats,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct V3InlineCalleeSpecializationReport {
    pub function_id: RuntimeFunctionId,
    pub qualname: String,
    pub stats: DirectCallStoreRewriteStats,
}

#[derive(Debug, Clone)]
pub struct V3CallInliningRewriteOutput {
    pub module: BlockPyModule<CodegenModuleShape>,
    pub summary: V3CallInliningRewriteSummary,
}

pub fn rewrite_v3_call_inlining_for_module<ResolveExternal, RuntimeConstructor, IterTarget>(
    module: &BlockPyModule<CodegenModuleShape>,
    profile: V3CallInliningProfile<'_>,
    direct_owner_attr_specializations_by_function: &HashMap<
        RuntimeFunctionId,
        HashMap<ProfiledOwnerAttrKey, Vec<ProfiledOwnerAttrSpecialization>>,
    >,
    mut resolve_external_target: ResolveExternal,
    mut constructor_for_runtime_name: RuntimeConstructor,
    mut iter_target_for_constructor_guard: IterTarget,
) -> Result<V3CallInliningRewriteOutput, String>
where
    ResolveExternal: FnMut(RuntimeFunctionId) -> Result<Option<V3ExternalInlineTarget>, String>,
    RuntimeConstructor: FnMut(RuntimeName) -> Option<RuntimeFunctionId>,
    IterTarget: FnMut(&ProfiledOwnerAttrSpecialization) -> Option<RuntimeFunctionId>,
{
    let mut planned_module = module.clone();
    let (callees, inline_callees, mut external_inline_plan, inline_callee_specializations) =
        build_v3_inline_callee_maps(
            &planned_module,
            profile,
            direct_owner_attr_specializations_by_function,
            &mut resolve_external_target,
            &mut iter_target_for_constructor_guard,
        )?;
    let mut summary = V3CallInliningRewriteSummary {
        inline_callee_specializations,
        ..V3CallInliningRewriteSummary::default()
    };
    let mut straightline_constructor_ids =
        straightline_constructor_ids_in_plan(&plan_v3_call_inlining(&planned_module));
    straightline_constructor_ids
        .extend(straightline_constructor_ids_in_plan(&external_inline_plan));

    let mut rewritten_store_count = 0usize;
    let mut runtime_constructor_function_ids = HashSet::new();
    let module_constants = &mut planned_module.module_constants;
    let empty_profiled_owner_attr_specializations = HashMap::new();
    for function in &mut planned_module.callable_defs {
        if profile
            .exact_int_branch_function_ids
            .contains(&function.function_id)
        {
            continue;
        }
        let inline_constructor_calls = v3_inline_constructor_calls(profile, function.function_id);
        let method_call_rewrite_targets = merge_call_target_specializations(
            v3_inline_method_call_targets(profile, function.function_id),
            constructor_call_targets(&inline_constructor_calls),
        );
        let direct_call_rewrite_targets =
            v3_inline_direct_function_call_targets(profile, function.function_id);
        if method_call_rewrite_targets.is_empty() && direct_call_rewrite_targets.is_empty() {
            continue;
        }
        if !method_call_rewrite_targets.is_empty() {
            let direct_owner_attr_specializations = direct_owner_attr_specializations_by_function
                .get(&function.function_id)
                .unwrap_or(&empty_profiled_owner_attr_specializations);
            let inline_constructor_calls =
                profiled_runtime_iter_constructor_calls_for_lowering(&inline_constructor_calls);
            let stats = rewrite_profiled_no_arg_method_call_store_sites(
                function,
                module_constants,
                &method_call_rewrite_targets,
                direct_owner_attr_specializations,
                &inline_callees,
                &straightline_constructor_ids,
                &inline_constructor_calls,
                &mut runtime_constructor_function_ids,
                &mut constructor_for_runtime_name,
                &mut iter_target_for_constructor_guard,
            );
            if stats.total_attempts() != 0 {
                summary
                    .method_rewrites
                    .push(V3MethodCallInlineRewriteReport {
                        function_id: function.function_id,
                        qualname: function.names.qualname.clone(),
                        stats,
                    });
            }
            rewritten_store_count += stats.rewritten_stores;
        }

        let stats = rewrite_profiled_function_call_store_sites(
            function,
            &direct_call_rewrite_targets,
            &callees,
        );
        if direct_rewrite_stats_has_activity(&stats) {
            summary.direct_rewrites.push(V3DirectCallRewriteReport {
                function_id: function.function_id,
                qualname: function.names.qualname.clone(),
                stats,
            });
        }
        rewritten_store_count += stats.rewritten_stores;
    }

    if rewritten_store_count != 0 {
        normalize_module_block_labels_dense(&mut planned_module);
        validate_codegen_instr_ids(&planned_module)
            .map_err(|err| format!("v3 call-inline rewrite validation failed: {err}"))?;
        extend_external_inline_plan_for_function_ids(
            &mut external_inline_plan,
            &runtime_constructor_function_ids,
            &mut resolve_external_target,
        )?;
        let mut inline_plan = plan_v3_call_inlining(&planned_module);
        inline_plan
            .functions
            .extend(external_inline_plan.functions.clone());
        summary.runtime_constructor_inline_plan_hits = runtime_constructor_function_ids
            .iter()
            .filter(|function_id| {
                inline_plan
                    .straightline_constructor(**function_id)
                    .is_some()
            })
            .count();
        summary.inline_rewrite = inline_direct_call_stores_with_callees(
            &mut planned_module,
            &inline_plan,
            &inline_callees,
        );
        summary.scalar_replacement =
            scalar_replace_non_escaping_constructor_allocations(&mut planned_module, &inline_plan);
        if summary.inline_rewrite.rewritten_stores != 0
            || summary.scalar_replacement.replaced_allocations != 0
        {
            normalize_module_block_labels_dense(&mut planned_module);
            validate_codegen_instr_ids(&planned_module)
                .map_err(|err| format!("v3 call-inline final validation failed: {err}"))?;
        }
    }

    summary.runtime_constructor_function_ids = runtime_constructor_function_ids;
    Ok(V3CallInliningRewriteOutput {
        module: planned_module,
        summary,
    })
}

fn build_v3_inline_callee_maps<ResolveExternal, IterTarget>(
    module: &BlockPyModule<CodegenModuleShape>,
    profile: V3CallInliningProfile<'_>,
    direct_owner_attr_specializations_by_function: &HashMap<
        RuntimeFunctionId,
        HashMap<ProfiledOwnerAttrKey, Vec<ProfiledOwnerAttrSpecialization>>,
    >,
    resolve_external_target: &mut ResolveExternal,
    iter_target_for_constructor_guard: &mut IterTarget,
) -> Result<
    (
        HashMap<RuntimeFunctionId, soac_core::block_py::BlockPyFunction<CodegenModuleShape>>,
        HashMap<RuntimeFunctionId, InlineCallee>,
        InlinePlanModule,
        Vec<V3InlineCalleeSpecializationReport>,
    ),
    String,
>
where
    ResolveExternal: FnMut(RuntimeFunctionId) -> Result<Option<V3ExternalInlineTarget>, String>,
    IterTarget: FnMut(&ProfiledOwnerAttrSpecialization) -> Option<RuntimeFunctionId>,
{
    let mut callee_functions = module
        .callable_defs
        .iter()
        .map(|function| (function.function_id, function.clone()))
        .collect::<HashMap<_, _>>();
    let mut inline_callees = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                InlineCallee::same_module(function.clone()),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut target_ids = HashSet::new();
    let mut external_inline_plan = InlinePlanModule::default();
    for function in &module.callable_defs {
        let direct_call_rewrite_targets =
            v3_inline_direct_function_call_targets(profile, function.function_id);
        for targets in direct_call_rewrite_targets.values() {
            target_ids.extend(targets.iter().copied());
        }
        for plans in v3_inline_constructor_calls(profile, function.function_id).values() {
            for plan in plans {
                target_ids.insert(plan.target);
                if let Some(inline_target) = plan.inline_target {
                    target_ids.insert(inline_target);
                }
            }
        }
        for targets in v3_inline_method_call_targets(profile, function.function_id).values() {
            target_ids.extend(targets.iter().copied());
        }
        if let Some(direct_owner_attr_specializations) =
            direct_owner_attr_specializations_by_function.get(&function.function_id)
        {
            let inline_constructor_calls =
                v3_inline_constructor_calls(profile, function.function_id);
            let constructor_call_targets = constructor_call_targets(&inline_constructor_calls);
            target_ids.extend(collect_profiled_runtime_iter_method_target_ids(
                function,
                module.module_constants.as_slice(),
                direct_owner_attr_specializations,
                &constructor_call_targets,
                iter_target_for_constructor_guard,
            ));
        }
    }

    let mut pending_target_ids = target_ids.iter().copied().collect::<VecDeque<_>>();
    while let Some(function_id) = pending_target_ids.pop_front() {
        if callee_functions.contains_key(&function_id) {
            continue;
        }
        let Some(target) = resolve_external_target(function_id)? else {
            continue;
        };
        external_inline_plan
            .functions
            .extend(target.inline_plan.functions.clone());
        callee_functions.insert(function_id, target.function.clone());
        inline_callees.insert(
            function_id,
            InlineCallee::cross_module(target.function, target.module_constants),
        );
        let direct_call_rewrite_targets =
            v3_inline_direct_function_call_targets(profile, function_id);
        for targets in direct_call_rewrite_targets.values() {
            for target_id in targets {
                if target_ids.insert(*target_id) {
                    pending_target_ids.push_back(*target_id);
                }
            }
        }
    }

    let specialization_reports =
        specialize_v3_inline_callees(profile, &mut callee_functions, &mut inline_callees);
    Ok((
        callee_functions,
        inline_callees,
        external_inline_plan,
        specialization_reports,
    ))
}

fn specialize_v3_inline_callees(
    profile: V3CallInliningProfile<'_>,
    callee_functions: &mut HashMap<
        RuntimeFunctionId,
        soac_core::block_py::BlockPyFunction<CodegenModuleShape>,
    >,
    inline_callees: &mut HashMap<RuntimeFunctionId, InlineCallee>,
) -> Vec<V3InlineCalleeSpecializationReport> {
    let mut reports = Vec::new();
    let function_ids = inline_callees.keys().copied().collect::<Vec<_>>();
    for function_id in function_ids {
        let direct_call_rewrite_targets =
            v3_inline_direct_function_call_targets(profile, function_id);
        if direct_call_rewrite_targets.is_empty() {
            continue;
        }
        let Some(inline_callee) = inline_callees.get(&function_id).cloned() else {
            continue;
        };
        let mut function = inline_callee.function().clone();
        let stats = rewrite_profiled_function_call_store_sites_with_constructor_targets(
            &mut function,
            &direct_call_rewrite_targets,
            callee_functions,
            true,
        );
        if stats.rewritten_stores == 0 {
            continue;
        }
        reports.push(V3InlineCalleeSpecializationReport {
            function_id: function.function_id,
            qualname: function.names.qualname.clone(),
            stats,
        });
        callee_functions.insert(function_id, function.clone());
        inline_callees.insert(function_id, inline_callee.with_function(function));
    }
    reports
}

fn extend_external_inline_plan_for_function_ids<ResolveExternal>(
    external_inline_plan: &mut InlinePlanModule,
    function_ids: &HashSet<RuntimeFunctionId>,
    resolve_external_target: &mut ResolveExternal,
) -> Result<(), String>
where
    ResolveExternal: FnMut(RuntimeFunctionId) -> Result<Option<V3ExternalInlineTarget>, String>,
{
    for function_id in function_ids {
        if external_inline_plan.functions.contains_key(function_id) {
            continue;
        }
        let Some(target) = resolve_external_target(*function_id)? else {
            continue;
        };
        external_inline_plan
            .functions
            .extend(target.inline_plan.functions);
    }
    Ok(())
}

fn v3_inline_direct_function_call_targets(
    profile: V3CallInliningProfile<'_>,
    function_id: RuntimeFunctionId,
) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
    profile
        .direct_calls_by_function
        .get(&function_id)
        .map(inline_direct_call_targets)
        .unwrap_or_default()
}

fn v3_inline_constructor_calls(
    profile: V3CallInliningProfile<'_>,
    function_id: RuntimeFunctionId,
) -> HashMap<InstrId, Vec<ResolvedV3ConstructorCallPlan>> {
    profile
        .constructor_calls_by_function
        .get(&function_id)
        .map(|constructor_calls_by_source| {
            constructor_calls_by_source
                .iter()
                .filter_map(|(source, constructor_calls)| {
                    let plans = constructor_calls
                        .iter()
                        .filter(|constructor_call| {
                            constructor_call.body.kind == CallBodyKind::Inline
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    (!plans.is_empty()).then_some((*source, plans))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn v3_inline_method_call_targets(
    profile: V3CallInliningProfile<'_>,
    function_id: RuntimeFunctionId,
) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
    profile
        .method_calls_by_function
        .get(&function_id)
        .map(inline_method_call_targets)
        .unwrap_or_default()
}

fn profiled_runtime_iter_constructor_calls_for_lowering(
    source: &HashMap<InstrId, Vec<ResolvedV3ConstructorCallPlan>>,
) -> HashMap<InstrId, Vec<ProfiledRuntimeIterConstructorCall>> {
    source
        .iter()
        .filter_map(|(instr_id, plans)| {
            let plans = plans
                .iter()
                .filter(|plan| plan.body.kind == CallBodyKind::Inline)
                .map(|plan| ProfiledRuntimeIterConstructorCall {
                    constructor_function_id: plan.target,
                    inline_target: plan.inline_target,
                })
                .collect::<Vec<_>>();
            (!plans.is_empty()).then_some((*instr_id, plans))
        })
        .collect()
}

fn straightline_constructor_ids_in_plan(
    inline_plan: &InlinePlanModule,
) -> HashSet<RuntimeFunctionId> {
    inline_plan
        .functions
        .iter()
        .filter_map(|(function_id, plan)| {
            plan.straightline_constructor.as_ref().map(|_| *function_id)
        })
        .collect()
}

fn plan_v3_call_inlining(module: &BlockPyModule<CodegenModuleShape>) -> InlinePlanModule {
    let escape_summary = summarize_module_escapes(module);
    plan_module_inlining(&escape_summary)
}

fn direct_rewrite_stats_has_activity(stats: &DirectCallStoreRewriteStats) -> bool {
    stats.rewritten_stores != 0
        || stats.skipped_empty_targets != 0
        || stats.skipped_incompatible_targets != 0
        || stats.skipped_missing_callee_targets != 0
        || stats.skipped_arity_mismatch_targets != 0
        || stats.skipped_unsupported_init_targets != 0
        || stats.skipped_missing_storage_layout_targets != 0
        || stats.skipped_unsupported_param_kind_targets != 0
        || stats.skipped_missing_param_storage_targets != 0
}

fn normalize_module_block_labels_dense(module: &mut BlockPyModule<CodegenModuleShape>) {
    for function in &mut module.callable_defs {
        normalize_function_block_labels_dense(function);
    }
}

fn normalize_function_block_labels_dense(function: &mut BlockPyFunction<CodegenModuleShape>) {
    let remap = function
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            let dense = BlockLabel::from_index(index);
            (block.label != dense).then_some((block.label, dense))
        })
        .collect::<HashMap<_, _>>();
    if remap.is_empty() {
        return;
    }

    struct LabelRemapper<'a> {
        remap: &'a HashMap<BlockLabel, BlockLabel>,
    }

    impl VisitMut<InstrCodegen> for LabelRemapper<'_> {
        fn visit_label_mut(&mut self, label: &mut BlockLabel) {
            if let Some(dense) = self.remap.get(label).copied() {
                *label = dense;
            }
        }
    }

    let mut remapper = LabelRemapper { remap: &remap };
    for (index, block) in function.blocks.iter_mut().enumerate() {
        block.label = BlockLabel::from_index(index);
        remapper.visit_term_mut(&mut block.term);
        if let Some(exc_edge) = &mut block.exc_edge {
            remapper.visit_edge_mut(exc_edge);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_v3::{CallBodyPlan, Cost};
    use soac_core::block_py::{ChildVisitable, HasSemanticInstrId, Visit};
    use soac_lowering::lower_python_to_blockpy_for_testing;
    use soac_lowering::passes::{
        InstrCodegen, TypedDirectCallArgPlan, TypedDirectCallArgSource, validate_codegen_instr_ids,
    };

    fn lowered_module(source: &str) -> BlockPyModule<CodegenModuleShape> {
        lower_python_to_blockpy_for_testing(source)
            .expect("test source should lower")
            .codegen_module
    }

    fn function_index_by_qualname(
        module: &BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> usize {
        module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing lowered function {qualname}"))
    }

    fn inline_body() -> CallBodyPlan {
        CallBodyPlan {
            kind: CallBodyKind::Inline,
            cost: Cost {
                hot_path: 1,
                miss_path: 0,
                deopt: 0,
                materialization: 0,
                ownership: 0,
                code_size: 1,
                compile: 1,
            },
            inline_target: None,
            reason: "test inline body".to_string(),
        }
    }

    #[derive(Default)]
    struct CallInstrCollector {
        calls: Vec<InstrId>,
    }

    impl Visit<InstrCodegen> for CallInstrCollector {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if matches!(expr, InstrCodegen::Call(_)) {
                self.calls.push(expr.semantic_instr_id());
            }
            expr.visit_children(self);
        }
    }

    #[test]
    fn rewrites_and_inlines_v3_direct_calls_in_soac_opt() {
        let module = lowered_module(
            "def callee(x):\n    return x\n\n\
def caller(fn, x):\n    y = fn(x)\n    return y\n",
        );
        let callee_id =
            module.callable_defs[function_index_by_qualname(&module, "callee")].function_id;
        let caller_index = function_index_by_qualname(&module, "caller");
        let caller_id = module.callable_defs[caller_index].function_id;
        let mut collector = CallInstrCollector::default();
        collector.visit_fn(&module.callable_defs[caller_index]);
        let source = collector
            .calls
            .first()
            .copied()
            .expect("caller should contain a generic call");

        let direct_calls = HashMap::from([(
            caller_id,
            HashMap::from([(
                source,
                vec![ResolvedV3DirectCallPlan {
                    source,
                    target: callee_id,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                    body: inline_body(),
                    reason: "test direct-call inline selection".to_string(),
                }],
            )]),
        )]);
        let constructor_calls = HashMap::new();
        let method_calls = HashMap::new();
        let exact_int_branch_function_ids = HashSet::new();
        let owner_attr_specializations = HashMap::new();

        let output = rewrite_v3_call_inlining_for_module(
            &module,
            V3CallInliningProfile {
                direct_calls_by_function: &direct_calls,
                constructor_calls_by_function: &constructor_calls,
                method_calls_by_function: &method_calls,
                exact_int_branch_function_ids: &exact_int_branch_function_ids,
            },
            &owner_attr_specializations,
            |_| Ok(None),
            |_| None,
            |_| None,
        )
        .expect("v3 call inlining should rewrite the module");

        assert_eq!(output.summary.direct_rewrites.len(), 1);
        assert_eq!(output.summary.direct_rewrites[0].stats.rewritten_stores, 1);
        assert_eq!(output.summary.inline_rewrite.rewritten_stores, 1);
        validate_codegen_instr_ids(&output.module)
            .expect("soac-opt v3 call inlining should preserve valid instruction ids");
        soac_lowering::block_py::validate::validate_codegen_module(&output.module)
            .expect("soac-opt v3 call inlining should leave dense codegen CFG labels");
    }
}
