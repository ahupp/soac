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
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::emit_v3::MechanicalRegionEmission;
use soac_ir_typed::plan_v3::{
    DirectCallCallee, ExactListItemAccessKind,
    IndexedFieldAccessKind as PlanV3IndexedFieldAccessKind,
    IndexedGlobalAccessKind as PlanV3IndexedGlobalAccessKind, RegionInputSource, RegionPlan,
    RegionSource,
};
use soac_ir_typed::{
    FactStore, InstrTyped, TypedAttrAccessPlan, TypedBlockPyModuleShape, TypedCallEmissionPlan,
    TypedCallEmissionPlans, TypedDirectMethodCallGuard, TypedExactIntBranchPlan,
    TypedExactIntPlanSource, TypedExactIntReturnPlan, TypedExactListItemAccessPlan,
    TypedExactListItemCounterSource, TypedExactListItemPlanSource, TypedIndexedFieldPlanSource,
    TypedIndexedGlobalAccessPlan, TypedIndexedGlobalPlanSource,
    assign_missing_typed_function_instr_ids,
};
use soac_opt::access_emission_v3::{
    ExactListItemAccessPlan as OptV3ExactListItemAccessPlan,
    IndexedGlobalAccessPlan as OptV3IndexedGlobalAccessPlan,
};
use soac_opt::artifacts_v3::ExactIntBranchV3Artifacts;
use soac_opt::call_emission_v3::{ResolvedV3DirectCallPlan, typed_call_emission_plans_from_v3};
use soac_opt::passes::{
    TypedInlineInstrIdMapping, TypedInlineLocalMapping, inline_typed_function_direct_call_stores,
    lower_typed_function_call_emission_plans, refresh_typed_function_value_facts,
    validate_typed_function_value_facts,
};
use soac_opt::region_emission_v3::{
    ExactIntBranchSelection as OptV3ExactIntBranchSelection,
    ExactIntReturnSelection as OptV3ExactIntReturnSelection,
    exact_int_branch_selection_for_source as opt_v3_exact_int_branch_selection_for_source,
    exact_int_return_selection_for_source as opt_v3_exact_int_return_selection_for_source,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn method_guards_for_v3_direct_call(
    plan: &ResolvedV3DirectCallPlan,
    method_name: &str,
) -> Result<Vec<TypedDirectMethodCallGuard>, String> {
    let owners = unsafe { crate::lookup_exact_owner_types_for_method(plan.target, method_name) }
        .map_err(|_| {
            format!(
                "failed to resolve owner types for method {} target {}",
                method_name, plan.target
            )
        })?;
    let mut guards = Vec::with_capacity(owners.len());
    for owner in owners {
        let Some(owner_type_ref) = super::symbols::reloc_type_ref_for_type(owner.owner_type)?
        else {
            continue;
        };
        if !super::symbols::ensure_reloc_callable_symbol_registered(
            &super::symbols::RelocCallableRef::OwnerAttr {
                owner_type_ref: owner_type_ref.clone(),
                attr_name: method_name.to_string(),
            },
        )? {
            continue;
        }
        guards.push(TypedDirectMethodCallGuard {
            function_id: plan.target,
            owner_type_ref: super::symbols::typed_attr_owner_ref_from_reloc_type_ref(
                &owner_type_ref,
            ),
            type_version: owner.type_version,
            arg_plan: plan.arg_plan.clone(),
        });
    }
    Ok(guards)
}

fn insert_method_guards(
    emissions: &mut TypedCallEmissionPlans,
    source: InstrId,
    method_name: String,
    guards: Vec<TypedDirectMethodCallGuard>,
) -> Result<(), String> {
    if guards.is_empty() {
        return Ok(());
    }
    let plan = emissions
        .by_source
        .entry(source)
        .or_insert_with(|| TypedCallEmissionPlan::Method {
            method_name: method_name.clone(),
            method_guards: Vec::new(),
        });
    let TypedCallEmissionPlan::Method {
        method_name: existing_name,
        method_guards,
    } = plan
    else {
        return Err(format!(
            "method-call emission source {source:?} already has non-method plan"
        ));
    };
    if existing_name != &method_name {
        return Err(format!(
            "method-call emission source {source:?} has conflicting method names {existing_name:?} and {method_name:?}"
        ));
    }
    method_guards.extend(guards);
    Ok(())
}

fn typed_call_emission_plans_for_profile_function(
    profile: &SpecializationProfile<'_>,
    function_id: RuntimeFunctionId,
) -> Result<TypedCallEmissionPlans, String> {
    let opt_v3_direct_calls_by_instr = profile.typed_call_emission_direct_calls(function_id);
    let mut ordinary_direct_calls_by_instr =
        HashMap::<InstrId, Vec<ResolvedV3DirectCallPlan>>::new();
    let mut method_guards_by_instr =
        HashMap::<InstrId, HashMap<String, Vec<TypedDirectMethodCallGuard>>>::new();
    for (source, plans) in opt_v3_direct_calls_by_instr {
        for plan in plans {
            match &plan.callee {
                DirectCallCallee::Function => {
                    ordinary_direct_calls_by_instr
                        .entry(source)
                        .or_default()
                        .push(plan);
                }
                DirectCallCallee::Method { method_name } => {
                    let method_guards = method_guards_for_v3_direct_call(&plan, method_name)?;
                    method_guards_by_instr
                        .entry(source)
                        .or_default()
                        .entry(method_name.clone())
                        .or_default()
                        .extend(method_guards);
                }
            }
        }
    }
    let mut emissions = typed_call_emission_plans_from_v3(&ordinary_direct_calls_by_instr)?;
    for (source, guards_by_method) in method_guards_by_instr {
        for (method_name, guards) in guards_by_method {
            insert_method_guards(&mut emissions, source, method_name, guards)?;
        }
    }
    Ok(emissions)
}

pub(super) fn apply_profile_call_emission_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let call_emissions =
        typed_call_emission_plans_for_profile_function(profile, function.function_id)?;
    lower_typed_function_call_emission_plans(function, &call_emissions)?;
    Ok(())
}

pub(super) fn annotate_typed_attr_accesses(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
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
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
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
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
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
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
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

#[derive(Clone)]
struct ProfileExactListItemAccessPlan {
    plan: OptV3ExactListItemAccessPlan,
    counter_source: Option<TypedExactListItemCounterSource>,
}

fn typed_exact_list_item_access_plan_from_opt_v3(
    plan: &OptV3ExactListItemAccessPlan,
    counter_source: Option<TypedExactListItemCounterSource>,
) -> TypedExactListItemAccessPlan {
    TypedExactListItemAccessPlan {
        source: TypedExactListItemPlanSource::OptimizationPlanV3,
        instr_id: plan.source,
        counter_source,
        access: plan.access,
        shape: plan.shape,
        guard: plan.guard,
        fallback: plan.fallback,
    }
}

fn annotate_typed_exact_list_item_accesses(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    exact_list_items_by_instr: &HashMap<InstrId, ProfileExactListItemAccessPlan>,
) -> Result<usize, String> {
    struct Annotator<'a> {
        exact_list_items_by_instr: &'a HashMap<InstrId, ProfileExactListItemAccessPlan>,
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
            if plan.plan.access != expected_access {
                self.error = Some(format!(
                    "optimizer v3 exact-list item plan for {instr_id} expected {:?}, but typed node requires {:?}",
                    plan.plan.access, expected_access
                ));
                return None;
            }
            self.used.insert(instr_id);
            Some(typed_exact_list_item_access_plan_from_opt_v3(
                &plan.plan,
                plan.counter_source,
            ))
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
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    remapped_exact_list_items: Option<&HashMap<InstrId, ProfileExactListItemAccessPlan>>,
) -> Result<(), String> {
    let mut exact_list_items_by_instr = profile
        .opt_v3_emitted_exact_list_items
        .get(&function.function_id)
        .map(|plans| {
            plans
                .iter()
                .map(|(instr_id, plan)| {
                    (
                        *instr_id,
                        ProfileExactListItemAccessPlan {
                            plan: plan.clone(),
                            counter_source: None,
                        },
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    if let Some(remapped_exact_list_items) = remapped_exact_list_items {
        for (instr_id, plan) in remapped_exact_list_items {
            if exact_list_items_by_instr
                .insert(*instr_id, plan.clone())
                .is_some()
            {
                return Err(format!(
                    "remapped optimizer v3 exact-list item plan for {} collides with an existing caller plan",
                    instr_id
                ));
            }
        }
    }
    if exact_list_items_by_instr.is_empty() {
        return Ok(());
    }
    annotate_typed_exact_list_item_accesses(function, &exact_list_items_by_instr)?;
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

pub(super) fn annotate_typed_exact_int_selections(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
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
    for block in &mut function.blocks {
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
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
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

fn annotate_typed_remapped_exact_int_selections(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    branch_plans: Option<&HashMap<InstrId, TypedExactIntBranchPlan>>,
    return_plans: Option<&HashMap<InstrId, TypedExactIntReturnPlan>>,
) -> Result<(), String> {
    let empty_branch_plans = HashMap::new();
    let empty_return_plans = HashMap::new();
    let branch_plans = branch_plans.unwrap_or(&empty_branch_plans);
    let return_plans = return_plans.unwrap_or(&empty_return_plans);
    if branch_plans.is_empty() && return_plans.is_empty() {
        return Ok(());
    }

    struct Annotator<'a> {
        branch_plans: &'a HashMap<InstrId, TypedExactIntBranchPlan>,
        return_plans: &'a HashMap<InstrId, TypedExactIntReturnPlan>,
        used_branches: HashSet<InstrId>,
        used_returns: HashSet<InstrId>,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn attach_branch_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let Some(plan) = self.branch_plans.get(&instr_id).cloned() else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "inlined optimizer v3 exact-int branch plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            if let Some(existing) = extra.exact_int_branch_plan()
                && existing != &plan
            {
                self.error = Some(format!(
                    "inlined optimizer v3 exact-int branch plan for {instr_id} collides with an existing branch plan"
                ));
                return;
            }
            extra.set_exact_int_branch_plan(plan);
            self.used_branches.insert(instr_id);
        }

        fn attach_return_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let Some(plan) = self.return_plans.get(&instr_id).cloned() else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "inlined optimizer v3 exact-int return plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            if let Some(existing) = extra.exact_int_return_plan()
                && existing != &plan
            {
                self.error = Some(format!(
                    "inlined optimizer v3 exact-int return plan for {instr_id} collides with an existing return plan"
                ));
                return;
            }
            extra.set_exact_int_return_plan(plan);
            self.used_returns.insert(instr_id);
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
        branch_plans,
        return_plans,
        used_branches: HashSet::new(),
        used_returns: HashSet::new(),
        error: None,
    };
    for block in &mut function.blocks {
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
    if annotator.used_branches.len() != branch_plans.len() {
        let missing = branch_plans
            .keys()
            .filter(|instr_id| !annotator.used_branches.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "inlined optimizer v3 exact-int branch plans were not attached to typed nodes: {missing}"
        ));
    }
    if annotator.used_returns.len() != return_plans.len() {
        let missing = return_plans
            .keys()
            .filter(|instr_id| !annotator.used_returns.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "inlined optimizer v3 exact-int return plans were not attached to typed nodes: {missing}"
        ));
    }
    Ok(())
}

fn apply_profile_access_and_scalar_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    remapped_exact_list_items: Option<&HashMap<InstrId, ProfileExactListItemAccessPlan>>,
    remapped_exact_int_branches: Option<&HashMap<InstrId, TypedExactIntBranchPlan>>,
    remapped_exact_int_returns: Option<&HashMap<InstrId, TypedExactIntReturnPlan>>,
) -> Result<(), String> {
    annotate_typed_indexed_field_accesses_from_profile(function, profile)?;
    annotate_typed_indexed_global_accesses_from_profile(function, profile)?;
    annotate_typed_exact_list_item_accesses_from_profile(
        function,
        profile,
        remapped_exact_list_items,
    )?;
    annotate_typed_exact_int_selections_from_profile(function, profile)?;
    annotate_typed_remapped_exact_int_selections(
        function,
        remapped_exact_int_branches,
        remapped_exact_int_returns,
    )?;
    Ok(())
}

pub(super) fn apply_profile_typed_block_metadata_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    annotate_typed_profiled_cold_blocks(function, profile)?;
    Ok(())
}

pub(super) fn apply_profile_typed_guard_miss_policy_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
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

#[derive(Clone, Default)]
struct TypedInlineExactIntRemapContext {
    instr_ids: HashMap<InstrId, InstrId>,
    local_names: HashMap<String, String>,
}

fn typed_inline_exact_int_remap_contexts(
    instr_mappings: &[TypedInlineInstrIdMapping],
    local_mappings: &[TypedInlineLocalMapping],
) -> Result<HashMap<(RuntimeFunctionId, u32), TypedInlineExactIntRemapContext>, String> {
    let mut contexts = HashMap::<(RuntimeFunctionId, u32), TypedInlineExactIntRemapContext>::new();
    for mapping in instr_mappings {
        let context = contexts
            .entry((mapping.callee, mapping.inline_instance))
            .or_default();
        context
            .instr_ids
            .entry(mapping.callee_instr_id)
            .or_insert(mapping.caller_instr_id);
    }
    for mapping in local_mappings {
        let context = contexts
            .entry((mapping.callee, mapping.inline_instance))
            .or_default();
        if let Some(existing) = context
            .local_names
            .insert(mapping.callee_name.clone(), mapping.caller_name.clone())
            && existing != mapping.caller_name
        {
            return Err(format!(
                "typed inline instance {} for callee {} maps local {:?} to both {:?} and {:?}",
                mapping.inline_instance,
                mapping.callee,
                mapping.callee_name,
                existing,
                mapping.caller_name
            ));
        }
    }
    Ok(contexts)
}

fn remapped_typed_inline_instr_id(
    source: InstrId,
    context: &TypedInlineExactIntRemapContext,
    label: &str,
) -> Result<InstrId, String> {
    context.instr_ids.get(&source).copied().ok_or_else(|| {
        format!(
            "inlined optimizer v3 exact-int {label} references unmapped callee instruction {source}"
        )
    })
}

fn remap_optional_typed_inline_instr_id(
    source: &mut Option<InstrId>,
    context: &TypedInlineExactIntRemapContext,
    label: &str,
) -> Result<(), String> {
    let Some(original) = *source else {
        return Ok(());
    };
    *source = Some(remapped_typed_inline_instr_id(original, context, label)?);
    Ok(())
}

fn remap_exact_int_region_plan(
    region: &RegionPlan,
    context: &TypedInlineExactIntRemapContext,
) -> Result<RegionPlan, String> {
    let mut remapped = region.clone();
    if let RegionSource::Instr { instr_id } = &mut remapped.source {
        *instr_id = remapped_typed_inline_instr_id(*instr_id, context, "region source")?;
    }
    for input in &mut remapped.inputs {
        match &mut input.source {
            RegionInputSource::FunctionParam {
                name: Some(name), ..
            } => {
                let Some(mapped_name) = context.local_names.get(name.as_str()) else {
                    return Err(format!(
                        "inlined optimizer v3 exact-int region input references unmapped callee local {name:?}"
                    ));
                };
                *name = mapped_name.clone();
            }
            RegionInputSource::FunctionParam { name: None, .. } => {
                return Err(
                    "inlined optimizer v3 exact-int region input has unnamed local source"
                        .to_string(),
                );
            }
            RegionInputSource::IndexedGlobal { source, .. } => {
                *source = remapped_typed_inline_instr_id(*source, context, "indexed-global input")?;
            }
            RegionInputSource::ModuleConstant { .. }
            | RegionInputSource::CapturedValue { .. }
            | RegionInputSource::Synthetic { .. } => {}
        }
    }
    for exit in &mut remapped.exits {
        remap_optional_typed_inline_instr_id(&mut exit.source, context, "region exit")?;
    }
    Ok(remapped)
}

fn remap_exact_int_mechanical_region(
    region: &MechanicalRegionEmission,
    context: &TypedInlineExactIntRemapContext,
) -> Result<MechanicalRegionEmission, String> {
    let mut remapped = region.clone();
    for step in &mut remapped.steps {
        remap_optional_typed_inline_instr_id(&mut step.source, context, "mechanical step")?;
    }
    for exit in &mut remapped.exits {
        remap_optional_typed_inline_instr_id(&mut exit.source, context, "mechanical exit")?;
    }
    Ok(remapped)
}

fn remap_typed_exact_int_branch_plan(
    instr_id: InstrId,
    selection: OptV3ExactIntBranchSelection<'_>,
    context: &TypedInlineExactIntRemapContext,
) -> Result<TypedExactIntBranchPlan, String> {
    Ok(TypedExactIntBranchPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: remap_exact_int_region_plan(selection.hot_plan, context)?,
        hot_region: remap_exact_int_mechanical_region(selection.hot_region, context)?,
        fallback_plan: remap_exact_int_region_plan(selection.fallback_plan, context)?,
        fallback_region: remap_exact_int_mechanical_region(selection.fallback_region, context)?,
    })
}

fn remap_typed_exact_int_return_plan(
    instr_id: InstrId,
    selection: OptV3ExactIntReturnSelection<'_>,
    context: &TypedInlineExactIntRemapContext,
) -> Result<TypedExactIntReturnPlan, String> {
    Ok(TypedExactIntReturnPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: remap_exact_int_region_plan(selection.hot_plan, context)?,
        hot_region: remap_exact_int_mechanical_region(selection.hot_region, context)?,
        fallback_plan: remap_exact_int_region_plan(selection.fallback_plan, context)?,
        fallback_region: remap_exact_int_mechanical_region(selection.fallback_region, context)?,
    })
}

fn remap_inlined_exact_int_selections(
    caller_function_id: RuntimeFunctionId,
    instr_mappings: &[TypedInlineInstrIdMapping],
    local_mappings: &[TypedInlineLocalMapping],
    profile: &SpecializationProfile<'_>,
    remapped_branches: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntBranchPlan>>,
    remapped_returns: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntReturnPlan>>,
) -> Result<usize, String> {
    let contexts = typed_inline_exact_int_remap_contexts(instr_mappings, local_mappings)?;
    let mut count = 0;
    for mapping in instr_mappings {
        let Some(artifacts) = profile
            .opt_v3_exact_int_branch_artifacts
            .get(&mapping.callee)
        else {
            continue;
        };
        let context = contexts
            .get(&(mapping.callee, mapping.inline_instance))
            .ok_or_else(|| {
                format!(
                    "typed inline instance {} for callee {} has instruction mappings but no remap context",
                    mapping.inline_instance, mapping.callee
                )
            })?;
        let mut context = context.clone();
        context
            .instr_ids
            .insert(mapping.callee_instr_id, mapping.caller_instr_id);
        if let Some(selection) =
            opt_v3_exact_int_branch_selection_for_source(artifacts, mapping.callee_instr_id)?
        {
            let plan =
                remap_typed_exact_int_branch_plan(mapping.caller_instr_id, selection, &context)?;
            if remapped_branches
                .entry(caller_function_id)
                .or_default()
                .insert(mapping.caller_instr_id, plan)
                .is_some()
            {
                return Err(format!(
                    "inlined exact-int branch plan for callee {} instruction {} collides at caller instruction {}",
                    mapping.callee, mapping.callee_instr_id, mapping.caller_instr_id
                ));
            }
            count += 1;
        }
        if let Some(selection) =
            opt_v3_exact_int_return_selection_for_source(artifacts, mapping.callee_instr_id)?
        {
            let plan =
                remap_typed_exact_int_return_plan(mapping.caller_instr_id, selection, &context)?;
            if remapped_returns
                .entry(caller_function_id)
                .or_default()
                .insert(mapping.caller_instr_id, plan)
                .is_some()
            {
                return Err(format!(
                    "inlined exact-int return plan for callee {} instruction {} collides at caller instruction {}",
                    mapping.callee, mapping.callee_instr_id, mapping.caller_instr_id
                ));
            }
            count += 1;
        }
    }
    Ok(count)
}

fn remap_inlined_exact_list_item_accesses(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    profile: &SpecializationProfile<'_>,
    remapped_exact_list_items: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, ProfileExactListItemAccessPlan>,
    >,
) -> Result<usize, String> {
    let mut count = 0;
    for mapping in mappings {
        let Some(callee_items) = profile.opt_v3_emitted_exact_list_items.get(&mapping.callee)
        else {
            continue;
        };
        let Some(plan) = callee_items.get(&mapping.callee_instr_id) else {
            continue;
        };
        let mut remapped = plan.clone();
        remapped.source = mapping.caller_instr_id;
        let remapped = ProfileExactListItemAccessPlan {
            plan: remapped,
            counter_source: Some(TypedExactListItemCounterSource {
                function_id: mapping.callee,
                instr_id: mapping.callee_instr_id,
            }),
        };
        if remapped_exact_list_items
            .entry(caller_function_id)
            .or_default()
            .insert(mapping.caller_instr_id, remapped)
            .is_some()
        {
            return Err(format!(
                "inlined exact-list item plan for callee {} instruction {} collides at caller instruction {}",
                mapping.callee, mapping.callee_instr_id, mapping.caller_instr_id
            ));
        }
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
pub(super) fn apply_profile_typed_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: Option<&SpecializationProfile<'_>>,
) -> Result<(), String> {
    let Some(profile) = profile else {
        return Ok(());
    };
    apply_profile_call_emission_plans_to_typed_function(function, profile)?;
    apply_profile_access_and_scalar_plans_to_typed_function(function, profile, None, None, None)?;
    apply_profile_typed_block_metadata_to_typed_function(function, profile)?;
    apply_profile_typed_guard_miss_policy_to_typed_function(function, profile);
    Ok(())
}

pub(crate) struct JitModulePlan {
    pub(super) module: Arc<BlockPyModule<TypedBlockPyModuleShape>>,
    pub(super) value_facts: FactStore,
    pub(super) locals: PlannedJitModuleLocals,
    pub(super) deopt_resume: PlannedJitDeoptResumeModule,
}

pub(super) fn collect_codegen_constants_for_module_name(
    module_name: &str,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
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

pub(super) fn optimize_blockpy(
    module: &BlockPyModule<BlockPyModuleShape>,
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
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let callee_module = module.clone();
    let external_callees = HashMap::new();
    let mut remapped_exact_list_items =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, ProfileExactListItemAccessPlan>>::new();
    let mut remapped_exact_int_branches =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedExactIntBranchPlan>>::new();
    let mut remapped_exact_int_returns =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedExactIntReturnPlan>>::new();
    for function in &mut module.callable_defs {
        apply_profile_call_emission_plans_to_typed_function(function, profile)?;
        let inline_direct_calls = profile.typed_inline_resolved_direct_calls(function.function_id);
        if !inline_direct_calls.is_empty() {
            let caller_function_id = function.function_id;
            let inline_targets = profile.typed_inline_direct_calls(function.function_id);
            let stats = inline_typed_function_direct_call_stores(
                function,
                &callee_module,
                &external_callees,
                &inline_targets,
            );
            if !stats.instr_id_mappings.is_empty() {
                remap_inlined_exact_list_item_accesses(
                    caller_function_id,
                    &stats.instr_id_mappings,
                    profile,
                    &mut remapped_exact_list_items,
                )?;
                remap_inlined_exact_int_selections(
                    caller_function_id,
                    &stats.instr_id_mappings,
                    &stats.local_mappings,
                    profile,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                )?;
            }
            if stats.rewritten_stores != 0 || stats.rewritten_effect_only_calls != 0 {
                assign_missing_typed_function_instr_ids(function);
                refresh_typed_function_value_facts(function);
            }
        }
        apply_profile_access_and_scalar_plans_to_typed_function(
            function,
            profile,
            remapped_exact_list_items.get(&function.function_id),
            remapped_exact_int_branches.get(&function.function_id),
            remapped_exact_int_returns.get(&function.function_id),
        )?;
        apply_profile_typed_block_metadata_to_typed_function(function, profile)?;
        apply_profile_typed_guard_miss_policy_to_typed_function(function, profile);
    }
    Ok(())
}
