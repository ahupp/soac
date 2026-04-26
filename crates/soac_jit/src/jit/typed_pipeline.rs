use super::operation_specializations::OptV3ResolvedIndexedFieldAccess;
use super::planning::{
    PlannedJitDeoptResumeModule, PlannedJitModuleLocals, PreparedJitTypedModulePlan,
    plan_jit_typed_module,
};
use super::{SpecializationProfile, annotate_typed_profiled_cold_blocks};
use crate::module_constants::ModuleCodegenConstants;
use soac_config::SoacEnvConfig;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, BlockTerm, CallableScopeKind, ChildVisitable,
    HasSemanticInstrId, InstrId, RuntimeFunctionId, VisitMut,
};
use soac_ir_blockpy::CodegenModuleShape;
use soac_ir_typed::plan_v3::{
    ExactListItemAccessKind, IndexedFieldAccessKind as PlanV3IndexedFieldAccessKind,
    IndexedGlobalAccessKind as PlanV3IndexedGlobalAccessKind,
};
use soac_ir_typed::{
    FactStore, InstrTyped, TypedAttrAccessPlan, TypedCallEmissionPlans, TypedCodegenModuleShape,
    TypedExactIntBranchPlan, TypedExactIntPlanSource, TypedExactIntReturnPlan,
    TypedExactIntScalarThreadPlan, TypedExactListItemAccessPlan, TypedExactListItemPlanSource,
    TypedIndexedFieldPlanSource, TypedIndexedGlobalAccessPlan, TypedIndexedGlobalPlanSource,
    assign_missing_typed_function_instr_ids,
};
use soac_opt::access_emission_v3::{
    ExactListItemAccessPlan as OptV3ExactListItemAccessPlan,
    IndexedGlobalAccessPlan as OptV3IndexedGlobalAccessPlan,
};
use soac_opt::artifacts_v3::ExactIntBranchV3Artifacts;
use soac_opt::call_emission_v3::typed_call_emission_plans_from_v3;
use soac_opt::passes::{
    inline_typed_function_direct_call_stores, lower_typed_function_call_emission_plans,
    refresh_typed_function_value_facts, validate_typed_function_value_facts,
};
use soac_opt::region_emission_v3::{
    ExactIntBranchSelection as OptV3ExactIntBranchSelection,
    ExactIntReturnSelection as OptV3ExactIntReturnSelection,
    ScalarThreadSelection as OptV3ScalarThreadSelection,
    exact_int_branch_selection_for_source as opt_v3_exact_int_branch_selection_for_source,
    exact_int_return_selection_for_source as opt_v3_exact_int_return_selection_for_source,
    scalar_thread_selection_for_store_branch as opt_v3_scalar_thread_selection_for_store_branch,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn typed_call_emission_plans_for_profile_function(
    profile: &SpecializationProfile<'_>,
    function_id: RuntimeFunctionId,
) -> Result<TypedCallEmissionPlans, String> {
    let opt_v3_direct_calls_by_instr = profile.typed_call_emission_direct_calls(function_id);
    typed_call_emission_plans_from_v3(&opt_v3_direct_calls_by_instr)
}

pub(super) fn apply_profile_call_emission_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let call_emissions =
        typed_call_emission_plans_for_profile_function(profile, function.function_id)?;
    lower_typed_function_call_emission_plans(function, &call_emissions)?;
    Ok(())
}

pub(super) fn annotate_typed_attr_accesses(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    opt_v3_indexed_fields_by_instr: &HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    specialize_stores: bool,
) -> Result<usize, String> {
    struct Annotator<'a> {
        opt_v3_indexed_fields_by_instr: &'a HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
        specialize_stores: bool,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn opt_v3_guards_for_attr(&mut self, instr_id: InstrId) -> Option<TypedAttrAccessPlan> {
            let accesses = self.opt_v3_indexed_fields_by_instr.get(&instr_id)?;
            let mut guards = Vec::with_capacity(accesses.len());
            for access in accesses {
                guards.push(access.specialization.to_typed_guard());
            }
            Some(TypedAttrAccessPlan::IndexedField {
                source: TypedIndexedFieldPlanSource::OptimizationPlanV3,
                guards,
            })
        }

        fn annotate_attr(
            &mut self,
            instr_id: InstrId,
            expected_access: PlanV3IndexedFieldAccessKind,
        ) -> Option<TypedAttrAccessPlan> {
            if self.opt_v3_indexed_fields_by_instr.contains_key(&instr_id) {
                for access in self.opt_v3_indexed_fields_by_instr.get(&instr_id)? {
                    if access.access != expected_access {
                        self.error = Some(format!(
                            "optimizer v3 indexed-field for {instr_id} was prevalidated as {:?}, but typed node requires {:?}",
                            access.access, expected_access
                        ));
                        return None;
                    }
                }
                return self.opt_v3_guards_for_attr(instr_id);
            }
            None
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrTyped::GetAttrTyped(op) => {
                    if let Some(access) = self
                        .annotate_attr(op.semantic_instr_id(), PlanV3IndexedFieldAccessKind::Load)
                    {
                        op.access = access;
                        self.count += 1;
                    }
                }
                InstrTyped::SetAttrTyped(op) if self.specialize_stores => {
                    if let Some(access) = self
                        .annotate_attr(op.semantic_instr_id(), PlanV3IndexedFieldAccessKind::Store)
                    {
                        op.access = access;
                        self.count += 1;
                    }
                }
                InstrTyped::SetAttrTyped(op)
                    if self
                        .opt_v3_indexed_fields_by_instr
                        .contains_key(&op.semantic_instr_id()) =>
                {
                    self.error = Some(format!(
                        "optimizer v3 indexed-field store emission for {} cannot be consumed because indexed stores are disabled",
                        op.semantic_instr_id()
                    ));
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        opt_v3_indexed_fields_by_instr,
        specialize_stores,
        count: 0,
        error: None,
    };
    for block in &mut function.blocks {
        for instr in &mut block.body {
            annotator.visit_instr_mut(instr);
        }
        annotator.visit_term_mut(&mut block.term);
        if let Some(error) = annotator.error.take() {
            return Err(error);
        }
    }
    Ok(annotator.count)
}

fn annotate_typed_indexed_field_accesses_from_profile(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let (_, _, opt_v3_indexed_fields_by_instr) =
        profile.field_index_specialization_maps(function.function_id)?;
    if opt_v3_indexed_fields_by_instr.is_empty() {
        return Ok(());
    }
    let specialize_field_stores = profile.typed_specializations_embedded()
        || (profile.behavior_change_indexed_stores
            && function.scope.scope_kind != CallableScopeKind::Module);
    annotate_typed_attr_accesses(
        function,
        &opt_v3_indexed_fields_by_instr,
        specialize_field_stores,
    )?;
    Ok(())
}

fn typed_indexed_global_access_plan_from_opt_v3(
    plan: &OptV3IndexedGlobalAccessPlan,
) -> TypedIndexedGlobalAccessPlan {
    TypedIndexedGlobalAccessPlan {
        source: TypedIndexedGlobalPlanSource::OptimizationPlanV3,
        instr_id: plan.source,
        access: plan.access,
        module_name: plan.module_name.clone(),
        name: plan.name.clone(),
        expected_index: plan.expected_index,
        guard: plan.guard,
        fallback: plan.fallback,
    }
}

pub(super) fn annotate_typed_indexed_global_accesses(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    indexed_globals_by_instr: &HashMap<InstrId, OptV3IndexedGlobalAccessPlan>,
) -> Result<usize, String> {
    struct Annotator<'a> {
        indexed_globals_by_instr: &'a HashMap<InstrId, OptV3IndexedGlobalAccessPlan>,
        used: HashSet<InstrId>,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn plan_for_instr(
            &mut self,
            instr_id: InstrId,
            expected_access: PlanV3IndexedGlobalAccessKind,
            location_is_global: bool,
        ) -> Option<TypedIndexedGlobalAccessPlan> {
            let plan = self.indexed_globals_by_instr.get(&instr_id)?;
            if plan.access != expected_access {
                self.error = Some(format!(
                    "optimizer v3 indexed-global plan for {instr_id} expected {:?}, but typed node requires {:?}",
                    plan.access, expected_access
                ));
                return None;
            }
            if !location_is_global {
                self.error = Some(format!(
                    "optimizer v3 indexed-global plan for {instr_id} reached a non-global typed node"
                ));
                return None;
            }
            self.used.insert(instr_id);
            Some(typed_indexed_global_access_plan_from_opt_v3(plan))
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrTyped::Load(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) = self.plan_for_instr(
                            instr_id,
                            PlanV3IndexedGlobalAccessKind::Load,
                            op.name.location.is_global(),
                        )
                    {
                        op.extra_mut().set_indexed_global_access_plan(plan);
                        self.count += 1;
                    }
                }
                InstrTyped::Store(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) = self.plan_for_instr(
                            instr_id,
                            PlanV3IndexedGlobalAccessKind::Store,
                            op.name.location.is_global(),
                        )
                    {
                        op.extra_mut().set_indexed_global_access_plan(plan);
                        self.count += 1;
                    }
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        indexed_globals_by_instr,
        used: HashSet::new(),
        count: 0,
        error: None,
    };
    annotator.visit_fn_mut(function);
    if let Some(error) = annotator.error {
        return Err(error);
    }
    if annotator.used.len() != indexed_globals_by_instr.len() {
        let missing = indexed_globals_by_instr
            .keys()
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "optimizer v3 indexed-global plans were not attached to typed nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

fn annotate_typed_indexed_global_accesses_from_profile(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let Some(indexed_globals_by_instr) = profile
        .opt_v3_emitted_indexed_globals
        .get(&function.function_id)
    else {
        return Ok(());
    };
    annotate_typed_indexed_global_accesses(function, indexed_globals_by_instr)?;
    Ok(())
}

fn typed_exact_list_item_access_plan_from_opt_v3(
    plan: &OptV3ExactListItemAccessPlan,
) -> TypedExactListItemAccessPlan {
    TypedExactListItemAccessPlan {
        source: TypedExactListItemPlanSource::OptimizationPlanV3,
        instr_id: plan.source,
        access: plan.access,
        shape: plan.shape,
        guard: plan.guard,
        fallback: plan.fallback,
    }
}

fn annotate_typed_exact_list_item_accesses(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    exact_list_items_by_instr: &HashMap<InstrId, OptV3ExactListItemAccessPlan>,
) -> Result<usize, String> {
    struct Annotator<'a> {
        exact_list_items_by_instr: &'a HashMap<InstrId, OptV3ExactListItemAccessPlan>,
        used: HashSet<InstrId>,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn plan_for_instr(
            &mut self,
            instr_id: InstrId,
            expected_access: ExactListItemAccessKind,
        ) -> Option<TypedExactListItemAccessPlan> {
            let plan = self.exact_list_items_by_instr.get(&instr_id)?;
            if plan.access != expected_access {
                self.error = Some(format!(
                    "optimizer v3 exact-list item plan for {instr_id} expected {:?}, but typed node requires {:?}",
                    plan.access, expected_access
                ));
                return None;
            }
            self.used.insert(instr_id);
            Some(typed_exact_list_item_access_plan_from_opt_v3(plan))
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrTyped::GetItem(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) =
                            self.plan_for_instr(instr_id, ExactListItemAccessKind::Get)
                    {
                        op.extra_mut().set_exact_list_item_access_plan(plan);
                        self.count += 1;
                    }
                }
                InstrTyped::SetItem(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) =
                            self.plan_for_instr(instr_id, ExactListItemAccessKind::Set)
                    {
                        op.extra_mut().set_exact_list_item_access_plan(plan);
                        self.count += 1;
                    }
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        exact_list_items_by_instr,
        used: HashSet::new(),
        count: 0,
        error: None,
    };
    annotator.visit_fn_mut(function);
    if let Some(error) = annotator.error {
        return Err(error);
    }
    if annotator.used.len() != exact_list_items_by_instr.len() {
        let missing = exact_list_items_by_instr
            .keys()
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "optimizer v3 exact-list item plans were not attached to typed nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

fn annotate_typed_exact_list_item_accesses_from_profile(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let Some(exact_list_items_by_instr) = profile
        .opt_v3_emitted_exact_list_items
        .get(&function.function_id)
    else {
        return Ok(());
    };
    annotate_typed_exact_list_item_accesses(function, exact_list_items_by_instr)?;
    Ok(())
}

fn typed_exact_int_branch_plan_from_opt_v3(
    instr_id: InstrId,
    selection: OptV3ExactIntBranchSelection<'_>,
) -> TypedExactIntBranchPlan {
    TypedExactIntBranchPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: selection.hot_plan.clone(),
        hot_region: selection.hot_region.clone(),
        fallback_plan: selection.fallback_plan.clone(),
        fallback_region: selection.fallback_region.clone(),
    }
}

fn typed_exact_int_return_plan_from_opt_v3(
    instr_id: InstrId,
    selection: OptV3ExactIntReturnSelection<'_>,
) -> TypedExactIntReturnPlan {
    TypedExactIntReturnPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: selection.hot_plan.clone(),
        hot_region: selection.hot_region.clone(),
        fallback_plan: selection.fallback_plan.clone(),
        fallback_region: selection.fallback_region.clone(),
    }
}

fn typed_exact_int_scalar_thread_plan_from_opt_v3(
    store_instr_id: InstrId,
    producer_instr_id: InstrId,
    consumer_instr_id: InstrId,
    selection: OptV3ScalarThreadSelection<'_>,
) -> TypedExactIntScalarThreadPlan {
    TypedExactIntScalarThreadPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        store_instr_id,
        producer_instr_id,
        consumer_instr_id,
        thread: selection.thread.clone(),
        producer_hot_plan: selection.producer.hot_plan.clone(),
        producer_hot_region: selection.producer.hot_region.clone(),
        producer_fallback_plan: selection.producer.fallback_plan.clone(),
        producer_fallback_region: selection.producer.fallback_region.clone(),
        consumer_hot_plan: selection.consumer.hot_plan.clone(),
        consumer_hot_region: selection.consumer.hot_region.clone(),
        consumer_fallback_plan: selection.consumer.fallback_plan.clone(),
        consumer_fallback_region: selection.consumer.fallback_region.clone(),
    }
}

pub(super) fn annotate_typed_exact_int_selections(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    artifacts: &ExactIntBranchV3Artifacts,
) -> Result<usize, String> {
    struct Annotator<'a> {
        artifacts: &'a ExactIntBranchV3Artifacts,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn attach_branch_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let selection =
                match opt_v3_exact_int_branch_selection_for_source(self.artifacts, instr_id) {
                    Ok(selection) => selection,
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                };
            let Some(selection) = selection else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "optimizer v3 exact-int branch plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            let plan = typed_exact_int_branch_plan_from_opt_v3(instr_id, selection);
            self.count += usize::from(extra.set_exact_int_branch_plan(plan));
        }

        fn attach_return_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let selection =
                match opt_v3_exact_int_return_selection_for_source(self.artifacts, instr_id) {
                    Ok(selection) => selection,
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                };
            let Some(selection) = selection else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "optimizer v3 exact-int return plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            let plan = typed_exact_int_return_plan_from_opt_v3(instr_id, selection);
            self.count += usize::from(extra.set_exact_int_return_plan(plan));
        }

        fn attach_scalar_thread_plan(
            &mut self,
            store_expr: &mut InstrTyped,
            consumer_test: &InstrTyped,
        ) {
            let Some(store_instr_id) = store_expr.try_semantic_instr_id() else {
                return;
            };
            let InstrTyped::Store(store) = store_expr else {
                return;
            };
            let Some(producer_instr_id) = store.value.try_semantic_instr_id() else {
                return;
            };
            let Some(consumer_instr_id) = consumer_test.try_semantic_instr_id() else {
                return;
            };
            let selection = match opt_v3_scalar_thread_selection_for_store_branch(
                self.artifacts,
                producer_instr_id,
                consumer_instr_id,
                &store.name,
            ) {
                Ok(selection) => selection,
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            };
            let Some(selection) = selection else {
                return;
            };
            let plan = typed_exact_int_scalar_thread_plan_from_opt_v3(
                store_instr_id,
                producer_instr_id,
                consumer_instr_id,
                selection,
            );
            self.count += usize::from(store.extra_mut().set_exact_int_scalar_thread_plan(plan));
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            self.attach_return_plan(expr);
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        artifacts,
        count: 0,
        error: None,
    };
    let empty_if_tests_by_label = function
        .blocks
        .iter()
        .filter_map(|block| {
            if !block.body.is_empty() {
                return None;
            }
            let BlockTerm::IfTerm(if_term) = &block.term else {
                return None;
            };
            Some((block.label, if_term.test.clone()))
        })
        .collect::<HashMap<_, _>>();
    for block in &mut function.blocks {
        if let [store_expr] = block.body.as_mut_slice()
            && let BlockTerm::Jump(edge) = &block.term
            && edge.args.is_empty()
            && let Some(consumer_test) = empty_if_tests_by_label.get(&edge.target)
        {
            annotator.attach_scalar_thread_plan(store_expr, consumer_test);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        if let BlockTerm::IfTerm(if_term) = &mut block.term {
            annotator.attach_branch_plan(&mut if_term.test);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        for instr in &mut block.body {
            annotator.visit_instr_mut(instr);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        annotator.visit_term_mut(&mut block.term);
        if let Some(error) = annotator.error.take() {
            return Err(error);
        }
    }
    Ok(annotator.count)
}

fn annotate_typed_exact_int_selections_from_profile(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let Some(artifacts) = profile
        .opt_v3_exact_int_branch_artifacts
        .get(&function.function_id)
    else {
        return Ok(());
    };
    annotate_typed_exact_int_selections(function, artifacts)?;
    Ok(())
}

fn apply_profile_access_and_scalar_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    annotate_typed_indexed_field_accesses_from_profile(function, profile)?;
    annotate_typed_indexed_global_accesses_from_profile(function, profile)?;
    annotate_typed_exact_list_item_accesses_from_profile(function, profile)?;
    annotate_typed_exact_int_selections_from_profile(function, profile)?;
    Ok(())
}

pub(super) fn apply_profile_typed_block_metadata_to_typed_function(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    annotate_typed_profiled_cold_blocks(function, profile)?;
    Ok(())
}

pub(super) fn apply_profile_typed_guard_miss_policy_to_typed_function(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) {
    let enabled =
        profile.guard_miss_deopt && function.scope.scope_kind != CallableScopeKind::Module;
    if !enabled {
        return;
    }

    struct Annotator;

    impl VisitMut<InstrTyped> for Annotator {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if expr.try_semantic_instr_id().is_some()
                && let Some(extra) = expr.typed_extra_mut()
            {
                extra.set_guard_miss_deopt_enabled(true);
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator;
    annotator.visit_fn_mut(function);
}

#[cfg(test)]
pub(super) fn apply_profile_typed_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    profile: Option<&SpecializationProfile<'_>>,
) -> Result<(), String> {
    let Some(profile) = profile else {
        return Ok(());
    };
    apply_profile_call_emission_plans_to_typed_function(function, profile)?;
    apply_profile_access_and_scalar_plans_to_typed_function(function, profile)?;
    apply_profile_typed_block_metadata_to_typed_function(function, profile)?;
    apply_profile_typed_guard_miss_policy_to_typed_function(function, profile);
    Ok(())
}

pub(crate) struct JitModulePlan {
    pub(super) module: Arc<BlockPyModule<TypedCodegenModuleShape>>,
    pub(super) value_facts: FactStore,
    pub(super) locals: PlannedJitModuleLocals,
    pub(super) deopt_resume: PlannedJitDeoptResumeModule,
}

pub(super) fn collect_codegen_constants_for_module_name(
    module_name: &str,
    module: &BlockPyModule<TypedCodegenModuleShape>,
) -> ModuleCodegenConstants {
    if module_name == "soac.runtime" {
        ModuleCodegenConstants::collect_from_typed_runtime_module(module)
    } else {
        ModuleCodegenConstants::collect_from_typed_module(module)
    }
}

fn build_jit_module_plan_from_prepared_typed_module(
    prepared: PreparedJitTypedModulePlan,
) -> Result<Arc<JitModulePlan>, String> {
    for function in &prepared.module.callable_defs {
        validate_typed_function_value_facts(function)?;
    }
    Ok(Arc::new(JitModulePlan {
        module: Arc::new(prepared.module),
        value_facts: prepared.value_facts,
        locals: prepared.locals,
        deopt_resume: prepared.deopt_resume,
    }))
}

pub(super) fn build_typed_v3_jit_module_plan(
    module: &BlockPyModule<CodegenModuleShape>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
) -> Result<Arc<JitModulePlan>, String> {
    let prepared = soac_driver::typed_runtime::prepare_typed_v3_runtime_module_with_rewrites(
        module,
        env_config,
        |typed_module, _value_facts| {
            if let Some(profile) = profile {
                apply_typed_v3_module_rewrites(typed_module, profile)?;
            }
            Ok(())
        },
    )?;
    build_jit_module_plan_from_prepared_typed_module(plan_jit_typed_module(
        prepared.module,
        prepared.value_facts,
    )?)
}

fn apply_typed_v3_module_rewrites(
    module: &mut BlockPyModule<TypedCodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let callee_module = module.clone();
    let external_callees = HashMap::new();
    for function in &mut module.callable_defs {
        apply_profile_call_emission_plans_to_typed_function(function, profile)?;
        let inline_direct_calls = profile.typed_inline_resolved_direct_calls(function.function_id);
        if !inline_direct_calls.is_empty() {
            let inline_targets = profile.typed_inline_direct_calls(function.function_id);
            let stats = inline_typed_function_direct_call_stores(
                function,
                &callee_module,
                &external_callees,
                &inline_targets,
            );
            if stats.rewritten_stores != 0 {
                assign_missing_typed_function_instr_ids(function);
                refresh_typed_function_value_facts(function);
            }
        }
        apply_profile_access_and_scalar_plans_to_typed_function(function, profile)?;
        apply_profile_typed_block_metadata_to_typed_function(function, profile)?;
        apply_profile_typed_guard_miss_policy_to_typed_function(function, profile);
    }
    Ok(())
}
