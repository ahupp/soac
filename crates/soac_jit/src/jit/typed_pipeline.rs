use super::*;

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

fn typed_function_with_profile_plans(
    function: &BlockPyFunction<CodegenModuleShape>,
    profile: Option<&SpecializationProfile<'_>>,
) -> Result<BlockPyFunction<TypedCodegenModuleShape>, String> {
    let mut typed_function = lower_codegen_function_to_typed(function.clone());
    apply_profile_typed_plans_to_typed_function(&mut typed_function, profile)?;
    lower_typed_function_call_access_plan_instrs(&mut typed_function);
    Ok(typed_function)
}

pub(super) fn planned_typed_function_for_reservation(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    function: &BlockPyFunction<CodegenModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<Option<BlockPyFunction<TypedCodegenModuleShape>>, String> {
    if !env_config
        .runtime_optimization_pipeline()
        .uses_legacy_plan_artifacts_runtime()
    {
        return Ok(None);
    }
    let typed_function = typed_function_with_profile_plans(function, Some(profile))?;
    predeclare_typed_direct_call_imports(jit_module, &typed_function)?;
    Ok(Some(typed_function))
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

fn build_jit_module_plan_from_owned_module(
    module: BlockPyModule<CodegenModuleShape>,
) -> Result<Arc<JitModulePlan>, String> {
    let value_facts = infer_jit_value_facts(&module);
    let prepared = plan_jit_module_from_codegen(&module, value_facts)?;
    build_jit_module_plan_from_prepared_typed_module(prepared)
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

pub(super) fn build_jit_module_plan(
    module: &BlockPyModule<CodegenModuleShape>,
) -> Result<Arc<JitModulePlan>, String> {
    build_jit_module_plan_from_owned_module(module.clone())
}

pub(super) fn build_typed_v3_jit_module_plan(
    module: &BlockPyModule<CodegenModuleShape>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
) -> Result<Arc<JitModulePlan>, String> {
    let value_facts = infer_jit_value_facts(module);
    let mut typed_module = lower_codegen_module_to_typed(module.clone());
    typed_module = instrument_typed_module(
        typed_module,
        &InstrumentationConfig::from_env_config(env_config),
    )?;
    annotate_typed_module_value_facts(&mut typed_module, &value_facts);
    typed_module = lower_typed_if_tests_to_truthy(typed_module);
    if let Some(profile) = profile {
        apply_typed_v3_module_rewrites(&mut typed_module, profile)?;
    }
    build_jit_module_plan_from_prepared_typed_module(plan_jit_typed_module(
        typed_module,
        value_facts,
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
