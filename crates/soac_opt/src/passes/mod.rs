mod escape_analysis;
mod inline_plan;
mod inline_transform;
mod local_env_plan;
mod ownership_effects;
pub(crate) mod value_facts;

use soac_core::block_py::{BlockPyFunction, LocalLocation, NameLocation, ResolvedName};
pub use soac_ir_blockpy::{
    BlockPyModuleShape, InstrBlockPy, assign_missing_blockpy_function_instr_ids,
    reassign_blockpy_function_instr_ids, validate_blockpy_instr_ids,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CodegenTempLocal {
    pub name: String,
    pub location: LocalLocation,
}

impl CodegenTempLocal {
    pub fn resolved_name(&self) -> ResolvedName {
        ResolvedName {
            id: self.name.clone().into(),
            location: NameLocation::Local(self.location),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CodegenTempAllocationError {
    MissingStorageLayout,
}

pub fn try_allocate_codegen_stack_temp(
    function: &mut BlockPyFunction<BlockPyModuleShape>,
    prefix: &str,
) -> Result<CodegenTempLocal, CodegenTempAllocationError> {
    let name = function.name_gen.next_tmp_name(prefix).as_str().to_string();
    let layout = function
        .storage_layout
        .as_mut()
        .ok_or(CodegenTempAllocationError::MissingStorageLayout)?;
    let location = LocalLocation(
        u32::try_from(layout.stack_slots().len())
            .expect("codegen stack slot index should fit in u32"),
    );
    layout.ensure_stack_slot(name.clone());
    Ok(CodegenTempLocal { name, location })
}

pub fn allocate_codegen_stack_temp(
    function: &mut BlockPyFunction<BlockPyModuleShape>,
    prefix: &str,
) -> CodegenTempLocal {
    try_allocate_codegen_stack_temp(function, prefix)
        .expect("codegen function should have storage before allocating stack temp")
}

pub use crate::typed::{
    TypedConstructorFieldBinding, TypedConstructorFieldBindings,
    TypedConstructorInitBodyInlineStats, TypedExternalInlineCallee, TypedFieldScalarizationStats,
    TypedFullyVirtualObjectLoweringStats, TypedHotContinuationClone,
    TypedHotContinuationSplitStats, TypedInlineInstanceSource, TypedInlineInstrIdMapping,
    TypedInlineLocalMapping, TypedInlineRewriteStats, TypedVirtualBodyInstr,
    TypedVirtualBodyMaterializationStats, TypedVirtualBoundaryKind, TypedVirtualBoundaryLocation,
    TypedVirtualConstructorStats, TypedVirtualFieldEdge, TypedVirtualFieldRef,
    TypedVirtualFieldStateAnalysis, TypedVirtualMaterializationBoundary,
    TypedVirtualMaterializationRecipe, TypedVirtualObjectId, TypedVirtualObjectLoweringStats,
    TypedVirtualObjectPlan, TypedVirtualReturnMaterializationStats, TypedVirtualState,
    TypedVirtualStoreMaterializationStats, TypedVirtualizationPlan,
    annotate_typed_function_planned_results, annotate_typed_function_result_demands,
    annotate_typed_function_value_facts, annotate_typed_module_value_facts,
    inline_typed_constructor_init_bodies_with_external_callees,
    inline_typed_function_direct_call_stores,
    inline_typed_function_direct_call_stores_with_external_callees,
    inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls,
    lower_typed_fully_virtual_objects_to_locals_with_plan,
    lower_typed_function_call_access_plan_instrs, lower_typed_function_call_emission_plans,
    lower_typed_function_if_tests_to_truthy, lower_typed_if_tests_to_truthy,
    lower_typed_virtual_fields_to_locals_with_plan,
    lower_typed_virtual_objects_to_locals_with_plan,
    materialize_typed_virtual_body_boundaries_with_plan,
    materialize_typed_virtual_return_boundaries_with_plan,
    materialize_typed_virtual_store_boundaries_with_plan, plan_typed_fully_virtual_objects,
    plan_typed_virtual_objects, refresh_typed_function_value_facts,
    rewrite_typed_stop_iteration_raises_to_handler_jumps, simplify_typed_virtual_tuple_ops,
    split_typed_alias_hot_continuations, split_typed_alias_hot_continuations_with_budget,
    split_typed_constructor_hot_continuations,
    split_typed_constructor_hot_continuations_with_budget,
    split_typed_inline_cleanup_hot_continuations,
    split_typed_inline_cleanup_hot_continuations_for_labels,
    split_typed_inline_cleanup_hot_continuations_for_labels_with_budget,
    sync_typed_module_value_facts, try_lower_typed_instr_to_codegen_legacy,
    try_lower_typed_module_to_codegen_legacy, try_lower_typed_term_to_codegen_legacy,
    typed_constructor_field_bindings_from_inline_stats,
    typed_constructor_field_bindings_from_inline_stats_with_external_callees,
    typed_constructor_init_plans_from_inline_stats_with_external_callees,
    validate_typed_function_call_access_plans, validate_typed_function_value_facts,
    validate_typed_module_call_access_plans,
};
pub use escape_analysis::{
    ConstructorFieldStore, ConstructorFieldValue, EscapeSummaryModule,
    FieldInitializerConstructorSummary, FunctionEscapeSummary, NonEscapingConstructorSummary,
    straightline_field_initializer_rejection_reason, summarize_module_escapes,
};
pub use inline_plan::{
    FunctionInlinePlan, InlinePlanModule, StraightlineConstructorInlinePlan, plan_module_inlining,
};
pub use inline_transform::{
    InlineFragment, InlineLocal, InlineUnsupportedReason, InlineValueBindings,
    bind_simple_direct_call_inline_args, bind_simple_direct_method_inline_args,
    build_cross_module_direct_call_inline_fragment_to_target,
    build_cross_module_direct_method_inline_fragment_to_target,
    build_direct_call_inline_fragment_to_target, build_direct_method_inline_fragment_to_target,
    build_single_block_inline_fragment, build_single_block_inline_fragment_to_target,
    build_single_block_inline_fragment_with_bindings,
};
pub use local_env_plan::{
    BlockLocalPlan, BlockParamFacts, FunctionLocalEnvResumePlan, FunctionLocalPlan,
    LocalEnvModulePlan, LocalEnvResumeBinding, LocalEnvResumeBindingState, LocalEnvResumeEntry,
    LocalEnvResumeModulePlan, LocalEnvResumePoint, LocalEnvResumeStatePrecision,
    LocalEnvResumeValueSource, LocalRefKind, ParamBindingFacts, ParamProvenance,
    PlannedLocalBinding, PlannedLocalStorage, plan_function_local_env_resume, plan_function_locals,
    plan_local_env_module, plan_local_env_resume_module, plan_typed_function_local_env_resume,
    plan_typed_function_locals, plan_typed_local_env_module, plan_typed_local_env_resume_module,
    render_local_env_function_plan, render_local_env_module_plan,
    render_local_env_resume_function_plan, render_local_env_resume_module_plan,
    render_planned_local_binding, validate_local_env_module_plan,
    validate_local_env_resume_module_plan, validate_typed_local_env_module_plan,
    validate_typed_local_env_resume_module_plan,
};
pub use ownership_effects::{
    BlockRefcountPlan, FunctionRefcountPlan, LocalRefState, REFCOUNT_STACK_SLOT_CLEAR_PREVIOUS,
    REFCOUNT_STACK_SLOT_DECREF_PURPOSES, REFCOUNT_STACK_SLOT_EXIT_SWEEP,
    REFCOUNT_STACK_SLOT_REPLACE_CLONED_PREVIOUS, REFCOUNT_STACK_SLOT_REPLACE_MOVED_PREVIOUS,
    REFCOUNT_STACK_SLOT_REPLACE_TRANSFERRED_PREVIOUS, RefcountAction, RefcountActionKind,
    RefcountLocal, RefcountPlan, RefcountReleaseReason, RefcountSite,
    compute_function_local_live_ins, compute_function_local_must_bound_ins,
    compute_typed_function_local_live_ins, compute_typed_function_local_must_bound_ins,
    plan_ownership_effects, plan_typed_ownership_effects, refcount_release_location_branch_name,
    refcount_release_reason_label, refcount_stack_slot_location_branch_name,
    validate_ownership_effects, validate_typed_ownership_effects,
};
pub use value_facts::infer_module_value_facts;
