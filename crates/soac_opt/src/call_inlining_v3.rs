use crate::call_emission_v3::{ResolvedV3DirectCallPlan, inline_direct_call_targets};
use soac_core::block_py::{
    BlockLabel, BlockPyFunction, BlockPyModule, InstrId, RuntimeFunctionId, VisitMut,
};
use soac_lowering::passes::{
    CodegenModuleShape, DirectCallStoreRewriteStats, InlineCallee, InlinePlanModule,
    InlineRewriteStats, InstrCodegen, InstrResolved, ScalarReplacementStats,
    inline_direct_call_stores_with_callees, plan_module_inlining,
    rewrite_profiled_function_call_store_sites,
    rewrite_profiled_function_call_store_sites_with_constructor_targets,
    scalar_replace_non_escaping_constructor_allocations, summarize_module_escapes,
    validate_codegen_instr_ids,
};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy)]
pub struct V3CallInliningProfile<'a> {
    pub direct_calls_by_function:
        &'a HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
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
    pub direct_rewrites: Vec<V3DirectCallRewriteReport>,
    pub inline_callee_specializations: Vec<V3InlineCalleeSpecializationReport>,
    pub inline_rewrite: InlineRewriteStats,
    pub scalar_replacement: ScalarReplacementStats,
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

pub fn rewrite_v3_call_inlining_for_module<ResolveExternal>(
    module: &BlockPyModule<CodegenModuleShape>,
    profile: V3CallInliningProfile<'_>,
    mut resolve_external_target: ResolveExternal,
) -> Result<V3CallInliningRewriteOutput, String>
where
    ResolveExternal: FnMut(RuntimeFunctionId) -> Result<Option<V3ExternalInlineTarget>, String>,
{
    let mut planned_module = module.clone();
    let (callees, inline_callees, external_inline_plan, inline_callee_specializations) =
        build_v3_inline_callee_maps(&planned_module, profile, &mut resolve_external_target)?;
    let mut summary = V3CallInliningRewriteSummary {
        inline_callee_specializations,
        ..V3CallInliningRewriteSummary::default()
    };
    let mut rewritten_store_count = 0usize;
    for function in &mut planned_module.callable_defs {
        if profile
            .exact_int_branch_function_ids
            .contains(&function.function_id)
        {
            continue;
        }
        let direct_call_rewrite_targets =
            v3_inline_direct_function_call_targets(profile, function.function_id);
        if direct_call_rewrite_targets.is_empty() {
            continue;
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
        let mut inline_plan = plan_v3_call_inlining(&planned_module);
        inline_plan
            .functions
            .extend(external_inline_plan.functions.clone());
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

    Ok(V3CallInliningRewriteOutput {
        module: planned_module,
        summary,
    })
}

fn build_v3_inline_callee_maps<ResolveExternal>(
    module: &BlockPyModule<CodegenModuleShape>,
    profile: V3CallInliningProfile<'_>,
    resolve_external_target: &mut ResolveExternal,
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
    use crate::plan_v3::{CallBodyKind, CallBodyPlan, Cost};
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
        let exact_int_branch_function_ids = HashSet::new();

        let output = rewrite_v3_call_inlining_for_module(
            &module,
            V3CallInliningProfile {
                direct_calls_by_function: &direct_calls,
                exact_int_branch_function_ids: &exact_int_branch_function_ids,
            },
            |_| Ok(None),
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
