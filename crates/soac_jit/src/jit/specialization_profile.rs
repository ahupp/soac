use super::operation_specializations::{
    FieldIndexSpecialization, OptV3ResolvedIndexedFieldAccess,
    field_index_specialization_from_opt_v3,
};
use super::precompile::PrecompileModuleIndex;
use crate::config::{SpecializationMode, pre_optimization_module_cache_identity};
use crate::module_type::SharedModuleState;
use crate::session::PlannedOptimizationInputsCacheKey;
use soac_config::SoacEnvConfig;
use soac_core::block_py::{
    BlockLabel, BlockPyFunction, BlockPyModule, InstrId, ModuleShape, PersistentFunctionId,
    RuntimeFunctionId, RuntimeModuleId, SerializedFunctionId,
};
use soac_core::profile::read_block_entry_counts_from_file;
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::TypedDirectCallArgPlan;
use soac_ir_typed::plan_v3::{CallBodyKind, ModulePlanIdentity};
use soac_opt::access_emission_v3::{
    ExactListItemAccessPlan as OptV3ExactListItemAccessPlan,
    IndexedFieldAccessPlan as OptV3IndexedFieldAccessPlan,
    IndexedGlobalAccessPlan as OptV3IndexedGlobalAccessPlan,
    exact_list_items_for_function_from_artifacts as opt_v3_emitted_exact_list_items_for_function,
    indexed_fields_for_function_from_artifacts as opt_v3_emitted_indexed_fields_for_function,
    indexed_globals_for_function_from_artifacts as opt_v3_emitted_indexed_globals_for_function,
    prepare_indexed_field_accesses_for_codegen as opt_v3_prepare_indexed_field_accesses_for_codegen,
};
use soac_opt::alternatives_v3::AlternativeCatalog;
use soac_opt::artifacts_v3::{
    ExactIntBranchV3Artifacts, single_function_optimization_artifacts_v3,
};
use soac_opt::call_emission_v3::{
    ResolvedV3DirectCallPlan, direct_call_targets as opt_v3_direct_call_targets,
    direct_calls_for_function_from_artifacts as opt_v3_emitted_direct_calls_for_function,
};
use soac_opt::pipeline_v3::{
    ModuleOptimizationInput, optimize_modules_v3_from_raw_evidence,
    plan_and_emit_module_v3_from_raw_evidence,
};
use soac_opt::plan::ProfileEvidenceStore;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

const COLD_BLOCK_ENTRY_RATE_DENOMINATOR: u64 = 100;

#[derive(Clone)]
pub(super) struct SpecializationProfile<'a> {
    pub(super) module_name: Option<&'a str>,
    pub(super) counter_dump_path: Option<Cow<'a, Path>>,
    pub(super) direct_call_emission_scope: DirectCallEmissionScope,
    pub(super) opt_v3_emitted_direct_calls:
        HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
    pub(super) opt_v3_emitted_exact_list_items:
        HashMap<RuntimeFunctionId, HashMap<InstrId, OptV3ExactListItemAccessPlan>>,
    pub(super) opt_v3_emitted_indexed_fields:
        HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<OptV3IndexedFieldAccessPlan>>>,
    pub(super) opt_v3_emitted_indexed_globals:
        HashMap<RuntimeFunctionId, HashMap<InstrId, OptV3IndexedGlobalAccessPlan>>,
    pub(super) opt_v3_exact_int_branch_artifacts:
        HashMap<RuntimeFunctionId, Arc<ExactIntBranchV3Artifacts>>,
    pub(super) behavior_change_indexed_stores: bool,
    pub(super) profiled_cold_blocks: bool,
    pub(super) guard_miss_deopt: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DirectCallEmissionScope {
    AllDirectCallCandidates,
}

#[derive(Clone, Default)]
pub(crate) struct PlannedOptimizationInputs {
    pub(super) opt_v3_emitted_direct_calls:
        HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
    pub(super) opt_v3_emitted_exact_list_items:
        HashMap<RuntimeFunctionId, HashMap<InstrId, OptV3ExactListItemAccessPlan>>,
    pub(super) opt_v3_emitted_indexed_fields:
        HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<OptV3IndexedFieldAccessPlan>>>,
    pub(super) opt_v3_emitted_indexed_globals:
        HashMap<RuntimeFunctionId, HashMap<InstrId, OptV3IndexedGlobalAccessPlan>>,
    pub(super) opt_v3_exact_int_branch_artifacts:
        HashMap<RuntimeFunctionId, Arc<ExactIntBranchV3Artifacts>>,
}

impl PlannedOptimizationInputs {
    fn extend(&mut self, other: PlannedOptimizationInputs) {
        self.opt_v3_emitted_direct_calls
            .extend(other.opt_v3_emitted_direct_calls);
        self.opt_v3_emitted_exact_list_items
            .extend(other.opt_v3_emitted_exact_list_items);
        self.opt_v3_emitted_indexed_fields
            .extend(other.opt_v3_emitted_indexed_fields);
        self.opt_v3_emitted_indexed_globals
            .extend(other.opt_v3_emitted_indexed_globals);
        self.opt_v3_exact_int_branch_artifacts
            .extend(other.opt_v3_exact_int_branch_artifacts);
    }

    fn v3_direct_function_call_targets(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
        self.opt_v3_emitted_direct_calls
            .get(&function_id)
            .map(opt_v3_direct_call_targets)
            .unwrap_or_default()
    }

    pub(super) fn direct_call_targets_for_batch(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<RuntimeFunctionId>> {
        self.v3_direct_function_call_targets(function_id)
    }
}

fn env_config_for_session(
    compile_session: Option<&crate::session::CompileSession>,
) -> Result<Cow<'_, SoacEnvConfig>, String> {
    match compile_session {
        Some(session) => Ok(Cow::Borrowed(session.env_config()?)),
        None => Ok(Cow::Owned(SoacEnvConfig::from_env()?)),
    }
}

pub(super) fn load_planned_optimization_inputs_for_runtime_state(
    shared_state: Option<&SharedModuleState>,
    compile_session: Option<&crate::session::CompileSession>,
    env_config: &SoacEnvConfig,
    specialization_mode: Option<SpecializationMode>,
) -> Result<PlannedOptimizationInputs, String> {
    if !matches!(
        specialization_mode,
        Some(SpecializationMode::Verify | SpecializationMode::Apply)
    ) {
        return Ok(PlannedOptimizationInputs::default());
    }
    let Some(shared_state) = shared_state else {
        return Ok(PlannedOptimizationInputs::default());
    };
    let Some(counter_dump_path) = env_config.counter_dump_input_path() else {
        return Ok(PlannedOptimizationInputs::default());
    };
    if !counter_dump_path.exists() {
        return Ok(PlannedOptimizationInputs::default());
    }
    if let Some(compile_session) = compile_session
        && let Some(cache_key) = PlannedOptimizationInputsCacheKey::new(
            shared_state.storage_instance_key(),
            compile_session.shared_module_registry_epoch(),
            counter_dump_path.clone(),
            specialization_mode.expect("checked verify/apply specialization mode"),
        )
    {
        return compile_session.cached_planned_optimization_inputs(cache_key, || {
            planned_typed_v3_runtime_inputs_from_raw_evidence(
                shared_state,
                Some(compile_session),
                counter_dump_path.as_path(),
            )
        });
    }
    planned_typed_v3_runtime_inputs_from_raw_evidence(
        shared_state,
        compile_session,
        counter_dump_path.as_path(),
    )
}

fn planned_typed_v3_runtime_inputs_from_raw_evidence(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    counter_dump_path: &Path,
) -> Result<PlannedOptimizationInputs, String> {
    let evidence_store = ProfileEvidenceStore::from_counter_dump(counter_dump_path)
        .map_err(|err| err.to_string())?;
    if let Some(compile_session) = compile_session {
        return planned_typed_v3_runtime_inputs_from_raw_evidence_with_session_modules(
            shared_state,
            compile_session,
            &evidence_store,
        );
    }
    let artifacts = plan_and_emit_module_v3_from_raw_evidence(
        &AlternativeCatalog::default_v3(),
        module_plan_identity_for_shared_state(shared_state),
        &shared_state.lowered_module,
        &evidence_store,
    )
    .map_err(|err| err.to_string())?;
    planned_optimization_inputs_from_v3_artifacts(&artifacts, shared_state, compile_session)
}

fn module_plan_identity_for_shared_state(shared_state: &SharedModuleState) -> ModulePlanIdentity {
    ModulePlanIdentity {
        module_name: shared_state.module_name.clone(),
        source_hash: shared_state.source_hash,
        cache_identity: pre_optimization_module_cache_identity(
            env!("SOAC_BUILD_IDENTITY"),
            shared_state.module_name == "soac.runtime",
        ),
    }
}

fn planned_typed_v3_runtime_inputs_from_raw_evidence_with_session_modules(
    shared_state: &SharedModuleState,
    compile_session: &crate::session::CompileSession,
    evidence_store: &ProfileEvidenceStore,
) -> Result<PlannedOptimizationInputs, String> {
    let current_identity = module_plan_identity_for_shared_state(shared_state);
    let external_states = compile_session.shared_module_states_snapshot()?;
    let mut module_inputs = vec![ModuleOptimizationInput::new(
        current_identity.clone(),
        &shared_state.lowered_module,
        true,
    )];
    for external_state in &external_states {
        if external_state.module_name == shared_state.module_name
            && external_state.source_hash == shared_state.source_hash
        {
            continue;
        }
        module_inputs.push(ModuleOptimizationInput::new(
            module_plan_identity_for_shared_state(external_state),
            &external_state.lowered_module,
            false,
        ));
    }
    let planned = optimize_modules_v3_from_raw_evidence(evidence_store, module_inputs)
        .map_err(|err| err.to_string())?;
    let mut inputs = PlannedOptimizationInputs::default();
    for module in planned.modules {
        let module_shared_state_owner;
        let module_shared_state = if module.identity.module_name == shared_state.module_name
            && module.identity.source_hash == shared_state.source_hash
        {
            shared_state
        } else {
            let Some(found) = compile_session.shared_module_state_for_identity(
                &module.identity.module_name,
                module.identity.source_hash,
            )?
            else {
                continue;
            };
            module_shared_state_owner = found;
            module_shared_state_owner.as_ref()
        };
        inputs.extend(planned_optimization_inputs_from_v3_artifacts(
            &module.artifacts,
            module_shared_state,
            Some(compile_session),
        )?);
    }
    Ok(inputs)
}

pub(super) fn planned_optimization_inputs_from_v3_artifacts(
    artifacts: &ExactIntBranchV3Artifacts,
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
) -> Result<PlannedOptimizationInputs, String> {
    let mut inputs = PlannedOptimizationInputs::default();
    for planned_function in &artifacts.plan.functions {
        let local_function_id = planned_function.function.function.local_function_id();
        let current_function_id = RuntimeFunctionId::new(
            RuntimeModuleId::new(shared_state.module_id()),
            local_function_id,
        );
        shared_state
            .lookup_function(current_function_id)
            .ok_or_else(|| {
                format!(
                    "optimization plan v3 for module {} references missing function id {} ({})",
                    artifacts.plan.module.module_name,
                    local_function_id,
                    planned_function
                        .function
                        .debug_name
                        .as_deref()
                        .unwrap_or("<unknown>")
                )
            })?;
        let Some(function_artifacts) =
            opt_v3_single_function_artifacts(artifacts, planned_function.function.function)?
        else {
            continue;
        };
        if let Some(direct_calls) =
            opt_v3_emitted_direct_calls_for_function(&function_artifacts, |target| {
                resolve_opt_v3_runtime_function_target(shared_state, compile_session, target)
            })?
        {
            inputs
                .opt_v3_emitted_direct_calls
                .insert(current_function_id, direct_calls);
        }
        if let Some(items) = opt_v3_emitted_exact_list_items_for_function(&function_artifacts)? {
            inputs
                .opt_v3_emitted_exact_list_items
                .insert(current_function_id, items);
        }
        if let Some(indexed_fields) =
            opt_v3_emitted_indexed_fields_for_function(&function_artifacts)?
        {
            inputs
                .opt_v3_emitted_indexed_fields
                .insert(current_function_id, indexed_fields);
        }
        if let Some(indexed_globals) =
            opt_v3_emitted_indexed_globals_for_function(&function_artifacts)?
        {
            inputs
                .opt_v3_emitted_indexed_globals
                .insert(current_function_id, indexed_globals);
        }
        inputs
            .opt_v3_exact_int_branch_artifacts
            .insert(current_function_id, Arc::new(function_artifacts));
    }
    Ok(inputs)
}

pub(super) fn planned_optimization_inputs_from_v3_artifacts_for_blockpy_module(
    artifacts: &ExactIntBranchV3Artifacts,
    module: &BlockPyModule<BlockPyModuleShape>,
    module_name: &str,
    source_hash: u64,
    module_index: Option<&PrecompileModuleIndex>,
) -> Result<PlannedOptimizationInputs, String> {
    let mut inputs = PlannedOptimizationInputs::default();
    let module_id = RuntimeModuleId::new(module.module_name_gen.module_id());
    for planned_function in &artifacts.plan.functions {
        let local_function_id = planned_function.function.function.local_function_id();
        let current_function_id = RuntimeFunctionId::new(module_id, local_function_id);
        module
            .callable_defs
            .iter()
            .find(|function| function.function_id == current_function_id)
            .ok_or_else(|| {
                format!(
                    "optimization plan v3 for module {} references missing function id {} ({})",
                    artifacts.plan.module.module_name,
                    local_function_id,
                    planned_function
                        .function
                        .debug_name
                        .as_deref()
                        .unwrap_or("<unknown>")
                )
            })?;
        let Some(function_artifacts) =
            opt_v3_single_function_artifacts(artifacts, planned_function.function.function)?
        else {
            continue;
        };
        if let Some(direct_calls) =
            opt_v3_emitted_direct_calls_for_function(&function_artifacts, |target| {
                resolve_opt_v3_codegen_module_function_target(
                    module_name,
                    source_hash,
                    module_id,
                    module,
                    module_index,
                    target,
                )
            })?
        {
            inputs
                .opt_v3_emitted_direct_calls
                .insert(current_function_id, direct_calls);
        }
        if let Some(items) = opt_v3_emitted_exact_list_items_for_function(&function_artifacts)? {
            inputs
                .opt_v3_emitted_exact_list_items
                .insert(current_function_id, items);
        }
        if let Some(indexed_fields) =
            opt_v3_emitted_indexed_fields_for_function(&function_artifacts)?
        {
            inputs
                .opt_v3_emitted_indexed_fields
                .insert(current_function_id, indexed_fields);
        }
        if let Some(indexed_globals) =
            opt_v3_emitted_indexed_globals_for_function(&function_artifacts)?
        {
            inputs
                .opt_v3_emitted_indexed_globals
                .insert(current_function_id, indexed_globals);
        }
        inputs
            .opt_v3_exact_int_branch_artifacts
            .insert(current_function_id, Arc::new(function_artifacts));
    }
    Ok(inputs)
}

fn resolve_opt_v3_runtime_function_target(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    target: PersistentFunctionId,
) -> Result<Option<RuntimeFunctionId>, String> {
    let target_shared_state_owner;
    let target_shared_state = if target.module.module_name == shared_state.module_name
        && target.module.source_hash == shared_state.source_hash
    {
        shared_state
    } else {
        let Some(compile_session) = compile_session else {
            return Ok(None);
        };
        let Some(target_shared_state) = compile_session.shared_module_state_for_identity(
            &target.module.module_name,
            target.module.source_hash,
        )?
        else {
            return Ok(None);
        };
        target_shared_state_owner = target_shared_state;
        target_shared_state_owner.as_ref()
    };
    let function_id = RuntimeFunctionId::new(
        RuntimeModuleId::new(target_shared_state.module_id()),
        target.local,
    );
    Ok(target_shared_state
        .lookup_function(function_id)
        .map(|function| function.function_id))
}

fn resolve_opt_v3_codegen_module_function_target(
    module_name: &str,
    source_hash: u64,
    module_id: RuntimeModuleId,
    module: &BlockPyModule<BlockPyModuleShape>,
    module_index: Option<&PrecompileModuleIndex>,
    target: PersistentFunctionId,
) -> Result<Option<RuntimeFunctionId>, String> {
    if target.module.module_name != module_name || target.module.source_hash != source_hash {
        return Ok(
            module_index.and_then(|module_index| module_index.function_id_for_target(&target))
        );
    }
    let target_function_id = RuntimeFunctionId::new(module_id, target.local);
    Ok(module
        .callable_defs
        .iter()
        .any(|function| function.function_id == target_function_id)
        .then_some(target_function_id))
}

fn opt_v3_single_function_artifacts(
    artifacts: &ExactIntBranchV3Artifacts,
    function: SerializedFunctionId,
) -> Result<Option<ExactIntBranchV3Artifacts>, String> {
    single_function_optimization_artifacts_v3(artifacts, function).map_err(|err| err.to_string())
}

fn planned_typed_v3_precompile_inputs_from_raw_evidence(
    module_name: &str,
    source_hash: u64,
    cache_identity: &str,
    module: &BlockPyModule<BlockPyModuleShape>,
    module_index: Option<&PrecompileModuleIndex>,
    counter_dump_path: Option<&Path>,
) -> Result<PlannedOptimizationInputs, String> {
    let Some(counter_dump_path) = counter_dump_path.filter(|path| path.exists()) else {
        return Ok(PlannedOptimizationInputs::default());
    };
    let evidence_store = ProfileEvidenceStore::from_counter_dump(counter_dump_path)
        .map_err(|err| err.to_string())?;
    let artifacts = plan_and_emit_module_v3_from_raw_evidence(
        &AlternativeCatalog::default_v3(),
        ModulePlanIdentity {
            module_name: module_name.to_string(),
            source_hash,
            cache_identity: cache_identity.to_string(),
        },
        module,
        &evidence_store,
    )
    .map_err(|err| err.to_string())?;
    planned_optimization_inputs_from_v3_artifacts_for_blockpy_module(
        &artifacts,
        module,
        module_name,
        source_hash,
        module_index,
    )
}

impl<'a> SpecializationProfile<'a> {
    pub(super) fn typed_specializations_embedded(&self) -> bool {
        self.direct_call_emission_scope == DirectCallEmissionScope::AllDirectCallCandidates
    }

    pub(super) fn from_runtime_state_with_session(
        shared_state: Option<&'a SharedModuleState>,
        compile_session: Option<&crate::session::CompileSession>,
    ) -> Result<Self, String> {
        let env_config = env_config_for_session(compile_session)?;
        let specialization_mode = env_config.specialization_mode();
        let planned_inputs = load_planned_optimization_inputs_for_runtime_state(
            shared_state,
            compile_session,
            &env_config,
            specialization_mode,
        )?;
        let counter_dump_path = env_config
            .counter_dump_input_path()
            .filter(|path| path.exists())
            .map(Cow::Owned);
        Ok(Self {
            module_name: shared_state.map(|shared_state| shared_state.module_name.as_str()),
            counter_dump_path,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: planned_inputs.opt_v3_emitted_direct_calls,
            opt_v3_emitted_exact_list_items: planned_inputs.opt_v3_emitted_exact_list_items,
            opt_v3_emitted_indexed_fields: planned_inputs.opt_v3_emitted_indexed_fields,
            opt_v3_emitted_indexed_globals: planned_inputs.opt_v3_emitted_indexed_globals,
            opt_v3_exact_int_branch_artifacts: planned_inputs.opt_v3_exact_int_branch_artifacts,
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: env_config.profiled_cold_blocks_enabled(),
            guard_miss_deopt: false,
        })
    }

    pub(super) fn from_precompile(
        env_config: &SoacEnvConfig,
        module_name: &'a str,
        source_hash: u64,
        cache_identity: &str,
        module: &BlockPyModule<BlockPyModuleShape>,
        module_index: Option<&PrecompileModuleIndex>,
        counter_dump_path: Option<&'a Path>,
    ) -> Result<Self, String> {
        let planned_inputs = planned_typed_v3_precompile_inputs_from_raw_evidence(
            module_name,
            source_hash,
            cache_identity,
            module,
            module_index,
            counter_dump_path,
        )?;
        Ok(Self {
            module_name: Some(module_name),
            counter_dump_path: counter_dump_path.map(Cow::Borrowed),
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: planned_inputs.opt_v3_emitted_direct_calls,
            opt_v3_emitted_exact_list_items: planned_inputs.opt_v3_emitted_exact_list_items,
            opt_v3_emitted_indexed_fields: planned_inputs.opt_v3_emitted_indexed_fields,
            opt_v3_emitted_indexed_globals: planned_inputs.opt_v3_emitted_indexed_globals,
            opt_v3_exact_int_branch_artifacts: planned_inputs.opt_v3_exact_int_branch_artifacts,
            behavior_change_indexed_stores: true,
            profiled_cold_blocks: env_config.profiled_cold_blocks_enabled(),
            guard_miss_deopt: true,
        })
    }

    pub(super) fn opt_v3_indexed_field_access_plans(&self) -> Vec<&OptV3IndexedFieldAccessPlan> {
        self.opt_v3_emitted_indexed_fields
            .values()
            .flat_map(|accesses_by_instr| accesses_by_instr.values())
            .flat_map(|accesses| accesses.iter())
            .collect()
    }

    pub(super) fn typed_call_emission_direct_calls(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>> {
        match self.direct_call_emission_scope {
            DirectCallEmissionScope::AllDirectCallCandidates => self
                .opt_v3_emitted_direct_calls
                .get(&function_id)
                .cloned()
                .unwrap_or_default(),
        }
    }

    pub(super) fn typed_inline_direct_calls(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>> {
        self.typed_inline_resolved_direct_calls(function_id)
            .into_iter()
            .map(|(source, plans)| {
                (
                    source,
                    plans
                        .into_iter()
                        .map(|plan| (plan.target, plan.arg_plan))
                        .collect(),
                )
            })
            .collect()
    }

    pub(super) fn typed_inline_resolved_direct_calls(
        &self,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>> {
        if self.direct_call_emission_scope != DirectCallEmissionScope::AllDirectCallCandidates {
            return HashMap::new();
        }
        self.opt_v3_emitted_direct_calls
            .get(&function_id)
            .map(|direct_calls| {
                direct_calls
                    .iter()
                    .filter_map(|(source, plans)| {
                        let inline_plans = plans
                            .iter()
                            .filter(|plan| plan.body.kind == CallBodyKind::Inline)
                            .cloned()
                            .collect::<Vec<_>>();
                        (!inline_plans.is_empty()).then_some((*source, inline_plans))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn field_index_specialization_maps(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Result<
        (
            HashMap<String, Vec<FieldIndexSpecialization>>,
            HashMap<InstrId, Vec<FieldIndexSpecialization>>,
            HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
        ),
        String,
    > {
        let opt_v3_by_instr = opt_v3_prepare_indexed_field_accesses_for_codegen(
            self.opt_v3_emitted_indexed_fields.get(&function_id),
            field_index_specialization_from_opt_v3,
        )?;
        Ok((HashMap::new(), HashMap::new(), opt_v3_by_instr))
    }

    pub(super) fn cold_block_labels(
        &self,
        function: &BlockPyFunction<impl ModuleShape>,
    ) -> Result<HashSet<BlockLabel>, String> {
        if !self.profiled_cold_blocks {
            return Ok(HashSet::new());
        }
        let Some(module_name) = self.module_name else {
            return Ok(HashSet::new());
        };
        let Some(path) = existing_counter_dump_path(self.counter_dump_path.as_deref()) else {
            return Ok(HashSet::new());
        };
        collect_cold_block_labels_from_path(path, function, module_name)
    }
}

fn existing_counter_dump_path(path: Option<&Path>) -> Option<&Path> {
    path.filter(|path| path.exists())
}

pub(super) fn collect_cold_block_labels_from_path(
    path: &Path,
    function: &BlockPyFunction<impl ModuleShape>,
    module_name: &str,
) -> Result<HashSet<BlockLabel>, String> {
    let block_entry_counts =
        read_block_entry_counts_from_file(path, module_name, function.function_id)?;
    let entry_label = function.entry_block().label;
    let Some(entry_count) = block_entry_counts.get(&entry_label).copied() else {
        return Ok(HashSet::new());
    };
    if entry_count == 0 {
        return Ok(HashSet::new());
    }

    Ok(function
        .blocks
        .iter()
        .filter_map(|block| {
            if block.label == entry_label {
                return None;
            }
            let block_count = block_entry_counts.get(&block.label).copied()?;
            (block_count.saturating_mul(COLD_BLOCK_ENTRY_RATE_DENOMINATOR) <= entry_count)
                .then_some(block.label)
        })
        .collect())
}
