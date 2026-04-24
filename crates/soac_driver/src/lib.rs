pub mod codegen_cache;

use crate::codegen_cache::{
    CachedCodegenModule, CachedCodegenModuleMetadata, PythonModuleCacheSource,
    cached_module_paths_under_root, hash_module_source, load_codegen_module_cache,
    module_optimization_plan_v3_path, module_optimized_codegen_v3_path,
    pre_optimization_module_cache_metadata, pre_optimization_module_cache_path,
    remap_cached_codegen_module_function_ids, store_codegen_module_cache,
    validate_codegen_module_cache_metadata,
};
use anyhow::{Context, Result as AnyhowResult};
use soac_config::SoacEnvConfig;
use soac_core::block_py::{
    BlockPyModule, CounterDef, CounterId, CounterScope, CounterSite, DeoptEntrySource,
    FunctionExecutionMode, ModuleNameGen,
};
use soac_core::pass_tracker::{NoopPassTracker, PassTracker};
use soac_instrument::{
    CounterBuilder, ExplicitCounterPlacement, InstrumentationConfig,
    define_typed_module_counter_defs, instrument_codegen_module_with_tracker,
};
use soac_lowering::passes::{self, CodegenModuleShape, InstrCodegen};
pub use soac_lowering::{LoweringError, Result};
use soac_opt::artifacts_v3::write_optimization_artifacts_v3;
use soac_opt::passes as opt_passes;
use soac_opt::pipeline_v3::{ModuleOptimizationInput, optimize_modules_v3_from_raw_evidence};
use soac_opt::plan::ProfileEvidenceStore;
use soac_opt::plan_v3::ModulePlanIdentity;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodegenPreparationOptions {
    pub lowering: soac_lowering::LoweringOptions,
    pub pre_optimization_cache: Option<PreOptimizationCacheRequest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreOptimizationCacheRequest {
    pub cache_root: PathBuf,
    pub source: PythonModuleCacheSource,
    pub module_name: String,
    pub build_identity: String,
}

impl PreOptimizationCacheRequest {
    pub fn new(
        cache_root: PathBuf,
        source: PythonModuleCacheSource,
        module_name: impl Into<String>,
        build_identity: impl Into<String>,
    ) -> Self {
        Self {
            cache_root,
            source,
            module_name: module_name.into(),
            build_identity: build_identity.into(),
        }
    }

    fn target(
        &self,
        source_hash: u64,
        runtime_names_as_globals: bool,
    ) -> soac_lowering::Result<PreOptimizationCacheTarget> {
        let path = pre_optimization_module_cache_path(
            self.cache_root.as_path(),
            self.source,
            self.module_name.as_str(),
            source_hash,
            self.build_identity.as_str(),
            runtime_names_as_globals,
        )
        .map_err(anyhow::Error::msg)?;
        let metadata = pre_optimization_module_cache_metadata(
            self.source,
            self.module_name.as_str(),
            source_hash,
            self.build_identity.as_str(),
            runtime_names_as_globals,
        );
        Ok(PreOptimizationCacheTarget { path, metadata })
    }
}

impl CodegenPreparationOptions {
    pub fn with_runtime_names_as_globals(mut self, runtime_names_as_globals: bool) -> Self {
        self.lowering.runtime_names_as_globals = runtime_names_as_globals;
        self
    }

    pub fn with_pre_optimization_cache(
        mut self,
        cache_root: PathBuf,
        source: PythonModuleCacheSource,
        module_name: impl Into<String>,
        build_identity: impl Into<String>,
    ) -> Self {
        self.pre_optimization_cache = Some(PreOptimizationCacheRequest::new(
            cache_root,
            source,
            module_name,
            build_identity,
        ));
        self
    }
}

impl From<soac_lowering::LoweringOptions> for CodegenPreparationOptions {
    fn from(lowering: soac_lowering::LoweringOptions) -> Self {
        Self {
            lowering,
            ..Self::default()
        }
    }
}

struct PreOptimizationCacheTarget {
    path: PathBuf,
    metadata: CachedCodegenModuleMetadata,
}

pub fn prepare_codegen_module(
    source: &str,
    module_name_gen: ModuleNameGen,
    options: CodegenPreparationOptions,
    env_config: &SoacEnvConfig,
    pass_tracker: &mut impl PassTracker,
) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
    let pre_optimization =
        load_or_lower_pre_optimization_module(source, module_name_gen, pass_tracker, options)?;
    finish_pre_optimization_module(pre_optimization, pass_tracker, env_config)
}

fn load_or_lower_pre_optimization_module(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
    options: CodegenPreparationOptions,
) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
    let cache_target = options
        .pre_optimization_cache
        .as_ref()
        .map(|cache| {
            cache.target(
                hash_module_source(source),
                options.lowering.runtime_names_as_globals,
            )
        })
        .transpose()?;

    if let Some(cache_target) = &cache_target {
        if let Some(module) =
            try_load_pre_optimization_cache(cache_target, module_name_gen.clone(), pass_tracker)
        {
            return Ok(module);
        }
    }

    let module = soac_lowering::lower_source_to_codegen_module_with_tracker(
        source,
        module_name_gen,
        pass_tracker,
        options.lowering.clone(),
    )?;

    if let Some(cache_target) = &cache_target {
        store_pre_optimization_cache(
            &cache_target.path,
            &cache_target.metadata,
            &module,
            pass_tracker,
        );
    }

    Ok(module)
}

fn try_load_pre_optimization_cache(
    cache_target: &PreOptimizationCacheTarget,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
) -> Option<BlockPyModule<CodegenModuleShape>> {
    let cache_path = &cache_target.path;
    let cache_exists =
        pass_tracker.record_timing("bb_codegen_cache_lookup", || cache_path.is_file());
    if !cache_exists {
        info!(
            target: "soac_blockpy_module_cache",
            event = "soac.blockpy_module_cache",
            cache_hit = false,
            path = %cache_path.display(),
            "blockpy_module_cache_miss",
        );
        return None;
    }

    let loaded = pass_tracker.record_timing("bb_codegen_cache_load", || {
        load_codegen_module_cache(cache_path)
    });
    match loaded {
        Ok(mut cache) => {
            let metadata_mismatch = match validate_codegen_module_cache_metadata(
                &cache.metadata,
                &cache_target.metadata,
            ) {
                Ok(()) => None,
                Err(err) => Some(err),
            };
            if let Some(err) = metadata_mismatch {
                warn!(
                    target: "soac_blockpy_module_cache",
                    event = "soac.blockpy_module_cache",
                    cache_hit = false,
                    path = %cache_path.display(),
                    error = %err,
                    "blockpy_module_cache_metadata_mismatch",
                );
                return None;
            }

            remap_cached_codegen_module_function_ids(&mut cache, module_name_gen);
            info!(
                target: "soac_blockpy_module_cache",
                event = "soac.blockpy_module_cache",
                cache_hit = true,
                path = %cache_path.display(),
                "blockpy_module_cache_hit",
            );
            let CachedCodegenModule {
                metadata: _,
                module,
            } = cache;
            Some(pass_tracker.run_pass("bb_codegen", || module))
        }
        Err(err) => {
            warn!(
                target: "soac_blockpy_module_cache",
                event = "soac.blockpy_module_cache",
                cache_hit = false,
                path = %cache_path.display(),
                error = %err,
                "blockpy_module_cache_load_failed",
            );
            None
        }
    }
}

fn store_pre_optimization_cache(
    cache_path: &Path,
    metadata: &CachedCodegenModuleMetadata,
    module: &BlockPyModule<CodegenModuleShape>,
    pass_tracker: &mut impl PassTracker,
) {
    let stored = pass_tracker.record_timing("bb_codegen_cache_store", || {
        store_codegen_module_cache(cache_path, metadata, module)
    });
    match stored {
        Ok(()) => {
            info!(
                target: "soac_blockpy_module_cache",
                event = "soac.blockpy_module_cache_store",
                path = %cache_path.display(),
                "blockpy_module_cache_store",
            );
        }
        Err(err) => {
            warn!(
                target: "soac_blockpy_module_cache",
                event = "soac.blockpy_module_cache_store",
                path = %cache_path.display(),
                error = %err,
                "blockpy_module_cache_store_failed",
            );
        }
    }
}

fn finish_pre_optimization_module(
    bb_codegen: BlockPyModule<CodegenModuleShape>,
    pass_tracker: &mut impl PassTracker,
    env_config: &SoacEnvConfig,
) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
    pass_tracker.record_timing("validate_codegen_instr_ids", || {
        passes::validate_codegen_instr_ids(&bb_codegen).map_err(anyhow::Error::msg)
    })?;

    let instrumentation_config = InstrumentationConfig::from_env_config(env_config);
    let bb_codegen = if instrumentation_config.explicit_counter_placement
        == ExplicitCounterPlacement::Typed
    {
        let mut typed_for_counters = opt_passes::lower_codegen_module_to_typed(bb_codegen.clone());
        define_typed_module_counter_defs(&mut typed_for_counters, &instrumentation_config)
            .map_err(anyhow::Error::msg)?;
        let mut bb_codegen = bb_codegen;
        bb_codegen.counter_defs = typed_for_counters.counter_defs;
        bb_codegen
    } else {
        bb_codegen
    };
    instrument_codegen_module_with_tracker(bb_codegen, &instrumentation_config, pass_tracker)
        .map_err(anyhow::Error::msg)
        .map_err(Into::into)
}

fn define_deopt_entry_counters_for_current_module(
    module: &mut BlockPyModule<CodegenModuleShape>,
    pass_tracker: &mut impl PassTracker,
) -> soac_lowering::Result<()> {
    let value_facts = pass_tracker.record_timing("deopt_entry_value_facts", || {
        opt_passes::infer_module_value_facts(module)
    });
    let local_env_plan = pass_tracker.record_timing("deopt_entry_local_env_plan", || {
        opt_passes::plan_local_env_module(module, &value_facts)
    });
    pass_tracker.record_timing("validate_deopt_entry_local_env_plan", || {
        opt_passes::validate_local_env_module_plan(module, &value_facts, &local_env_plan)
            .map_err(anyhow::Error::msg)
    })?;
    let local_env_resume_plan = pass_tracker
        .record_timing("deopt_entry_local_env_resume_plan", || {
            opt_passes::plan_local_env_resume_module(module, &local_env_plan, &value_facts)
        });
    pass_tracker.record_timing("validate_deopt_entry_local_env_resume_plan", || {
        opt_passes::validate_local_env_resume_module_plan(
            module,
            &local_env_plan,
            &value_facts,
            &local_env_resume_plan,
        )
        .map_err(anyhow::Error::msg)
    })?;
    define_bb_module_deopt_entry_counters(module, &local_env_resume_plan);
    Ok(())
}

fn define_bb_module_deopt_entry_counters(
    module: &mut BlockPyModule<CodegenModuleShape>,
    resume_plan: &opt_passes::LocalEnvResumeModulePlan,
) {
    let mut counters = CounterBuilder::new(&mut module.counter_defs);
    for function in module
        .callable_defs
        .iter()
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
    {
        let Some(function_plan) = resume_plan.function(function.function_id) else {
            continue;
        };
        for entry in &function_plan.entries {
            counters.define_if_missing(
                CounterScope::This,
                "deopt_entry_guard_miss",
                CounterSite::DeoptEntry {
                    function_id: function.function_id,
                    source: deopt_entry_source_for_resume_point(entry.point),
                },
            );
        }
    }
}

fn deopt_entry_source_for_resume_point(point: opt_passes::LocalEnvResumePoint) -> DeoptEntrySource {
    match point {
        opt_passes::LocalEnvResumePoint::BlockEntry { block, .. } => {
            DeoptEntrySource::BlockEntry { block_label: block }
        }
        opt_passes::LocalEnvResumePoint::BeforeInstr { key } => DeoptEntrySource::BeforeInstr {
            instr_id: key.instr_id,
        },
        opt_passes::LocalEnvResumePoint::BeforeTerm { block, .. } => {
            DeoptEntrySource::BeforeTerm { block_label: block }
        }
    }
}

pub fn finish_cached_codegen_module_for_runtime(
    module: BlockPyModule<CodegenModuleShape>,
    env_config: &SoacEnvConfig,
) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
    finish_pre_optimization_module(module, &mut NoopPassTracker::new(), env_config)
}

pub fn finish_cached_codegen_module_for_runtime_with_counter_defs(
    module: BlockPyModule<CodegenModuleShape>,
    env_config: &SoacEnvConfig,
    counter_defs: &[CounterDef],
) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
    let mut module = finish_cached_codegen_module_for_runtime(module, env_config)?;
    retain_defined_explicit_counter_increments(&mut module, counter_defs);
    module.counter_defs = counter_defs.to_vec();
    let instrumentation_config = InstrumentationConfig::from_env_config(env_config);
    if instrumentation_config.deopt_entry_counters_enabled() {
        define_deopt_entry_counters_for_current_module(&mut module, &mut NoopPassTracker::new())?;
    }
    Ok(module)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedModuleOptimizationInput {
    pub module_path: PathBuf,
    pub strict: bool,
}

impl CachedModuleOptimizationInput {
    pub fn new(module_path: PathBuf, strict: bool) -> Self {
        Self {
            module_path,
            strict,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleOptimizationPlanReport {
    pub output_path: PathBuf,
    pub optimized_module_path: PathBuf,
    pub module_name: String,
    pub source_hash: u64,
    pub function_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OptimizationPlanGenerationSummary {
    pub reports: Vec<ModuleOptimizationPlanReport>,
    pub skipped: usize,
}

impl OptimizationPlanGenerationSummary {
    pub fn written(&self) -> usize {
        self.reports.len()
    }
}

struct LoadedCachedModuleOptimizationInput {
    strict: bool,
    cache: CachedCodegenModule,
}

pub fn generate_optimization_plans_v3_for_counter_dump(
    counters_path: &Path,
    module_root: &Path,
    out_root: &Path,
) -> AnyhowResult<OptimizationPlanGenerationSummary> {
    let evidence_store = ProfileEvidenceStore::from_counter_dump(counters_path)?;
    let modules = cached_module_paths_under_root(module_root)?
        .into_iter()
        .map(|module_path| CachedModuleOptimizationInput::new(module_path, false));
    generate_optimization_plans_v3_for_cached_modules(&evidence_store, modules, out_root)
}

pub fn generate_optimization_plans_v3_for_cached_modules(
    evidence_store: &ProfileEvidenceStore,
    module_inputs: impl IntoIterator<Item = CachedModuleOptimizationInput>,
    out_root: &Path,
) -> AnyhowResult<OptimizationPlanGenerationSummary> {
    let loaded = module_inputs
        .into_iter()
        .map(|input| {
            let cache =
                load_codegen_module_cache(input.module_path.as_path()).with_context(|| {
                    format!(
                        "load BlockPy module cache {} for optimizer v3",
                        input.module_path.display()
                    )
                })?;
            Ok(LoadedCachedModuleOptimizationInput {
                strict: input.strict,
                cache,
            })
        })
        .collect::<AnyhowResult<Vec<_>>>()?;
    let module_inputs = loaded
        .iter()
        .map(|input| {
            ModuleOptimizationInput::new(
                module_plan_identity_for_cached_metadata(&input.cache.metadata),
                &input.cache.module,
                input.strict,
            )
        })
        .collect::<Vec<_>>();
    let optimized = optimize_modules_v3_from_raw_evidence(evidence_store, module_inputs)?;
    let mut summary = OptimizationPlanGenerationSummary {
        reports: Vec::new(),
        skipped: optimized.skipped,
    };
    for optimized_module in optimized.modules {
        let loaded_input = loaded
            .iter()
            .find(|input| {
                module_plan_identity_for_cached_metadata(&input.cache.metadata)
                    == optimized_module.identity
            })
            .with_context(|| {
                format!(
                    "optimized module {} source_hash=0x{:016x} did not match a loaded cache input",
                    optimized_module.identity.module_name, optimized_module.identity.source_hash
                )
            })?;
        let metadata = &loaded_input.cache.metadata;
        let output_path = module_optimization_plan_v3_path(
            out_root,
            metadata.source,
            metadata.module_name.as_str(),
        )?;
        let optimized_module_path = module_optimized_codegen_v3_path(
            out_root,
            metadata.source,
            metadata.module_name.as_str(),
        )?;
        write_optimization_artifacts_v3(output_path.as_path(), &optimized_module.artifacts)?;
        store_codegen_module_cache(
            optimized_module_path.as_path(),
            metadata,
            &optimized_module.optimized_module,
        )?;
        summary.reports.push(ModuleOptimizationPlanReport {
            output_path,
            optimized_module_path,
            module_name: metadata.module_name.clone(),
            source_hash: metadata.source_hash,
            function_count: optimized_module.artifacts.plan.functions.len(),
        });
    }
    Ok(summary)
}

fn module_plan_identity_for_cached_metadata(
    metadata: &CachedCodegenModuleMetadata,
) -> ModulePlanIdentity {
    ModulePlanIdentity {
        module_name: metadata.module_name.clone(),
        source_hash: metadata.source_hash,
        cache_identity: metadata.cache_identity.clone(),
    }
}

fn retain_defined_explicit_counter_increments(
    module: &mut BlockPyModule<CodegenModuleShape>,
    counter_defs: &[CounterDef],
) {
    let valid_counter_ids = counter_defs
        .iter()
        .map(|counter| counter.id)
        .collect::<HashSet<CounterId>>();
    for function in &mut module.callable_defs {
        for block in &mut function.blocks {
            block.body.retain(|expr| match expr {
                InstrCodegen::IncrementCounter(op) => valid_counter_ids.contains(&op.counter_id),
                _ => true,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen_cache::{
        PythonModuleCacheSource, load_codegen_module_cache, store_codegen_module_cache,
    };
    use soac_config::{
        RuntimeOptimizationPipeline, SoacEnvConfig, SoacLogConfig, SpecializationMode,
    };
    use soac_core::block_py::{
        ChildVisitable, CounterSite, FunctionExecutionMode, ModuleNameGen, Visit,
    };
    use soac_core::pass_tracker::RecordingPassTracker;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn refcount_counter_instrumentation_enabled(config: &SoacEnvConfig) -> bool {
        InstrumentationConfig::from_env_config(config)
            .counters
            .refcounts
            .scope()
            .is_some()
    }

    fn prepare_for_test(
        source: &str,
        config: &SoacEnvConfig,
    ) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
        let mut pass_tracker = RecordingPassTracker::new();
        prepare_codegen_module(
            source,
            ModuleNameGen::new(0),
            CodegenPreparationOptions::default(),
            config,
            &mut pass_tracker,
        )
    }

    fn prepare_recorded_for_test(
        source: &str,
    ) -> soac_lowering::Result<(BlockPyModule<CodegenModuleShape>, RecordingPassTracker)> {
        let mut pass_tracker = RecordingPassTracker::new();
        let module = prepare_codegen_module(
            source,
            ModuleNameGen::new(0),
            CodegenPreparationOptions::default(),
            &SoacEnvConfig::default(),
            &mut pass_tracker,
        )?;
        Ok((module, pass_tracker))
    }

    #[test]
    fn pre_optimization_cache_metadata_mismatch_rebuilds_and_replaces_cache() {
        let source = "def f():\n    return 1\n";
        let module_name = "cache_metadata_mismatch_test";
        let cache_root = unique_temp_dir();
        let cache_path = cache_root
            .join("project")
            .join(module_name)
            .join("mod.blockpy");
        let stale_metadata = metadata(module_name, source, "old-build");
        let expected_metadata = metadata(module_name, source, "new-build");
        let stale_module = soac_lowering::lower_python_to_blockpy_with_tracker_and_options(
            source,
            ModuleNameGen::new(1),
            RecordingPassTracker::new(),
            soac_lowering::LoweringOptions::default(),
        )
        .expect("initial lowering should succeed")
        .codegen_module;
        store_codegen_module_cache(cache_path.as_path(), &stale_metadata, &stale_module)
            .expect("stale cache should be writable");

        let mut pass_tracker = RecordingPassTracker::new();
        if let Err(err) = prepare_codegen_module(
            source,
            ModuleNameGen::new(2),
            CodegenPreparationOptions::default().with_pre_optimization_cache(
                cache_root,
                PythonModuleCacheSource::Project,
                module_name,
                "new-build",
            ),
            &SoacEnvConfig::default(),
            &mut pass_tracker,
        ) {
            panic!(
                "stale cache metadata should be treated as a miss, not as a lowering error: {err}"
            );
        }

        let replaced = load_codegen_module_cache(cache_path.as_path())
            .expect("rebuilt cache should be readable");
        assert_eq!(replaced.metadata, expected_metadata);
    }

    #[test]
    fn pre_optimization_lowering_does_not_compute_or_store_prepared_codegen_facts() {
        let source = "def callee(x):\n    return x\n\ndef caller(x):\n    return callee(x)\n";
        let (_lowered, pass_tracker) =
            prepare_recorded_for_test(source).expect("transform should succeed");
        let pass_names = pass_tracker.pass_names().collect::<Vec<_>>();
        let timing_names = pass_tracker
            .pass_timings()
            .map(|timing| timing.name)
            .collect::<Vec<_>>();

        for removed_pass in ["escape_summary", "inline_plan"] {
            assert!(
                !pass_names.contains(&removed_pass),
                "pre-optimization lowering should not compute {removed_pass} for cached prepared codegen"
            );
        }
        for removed_timing in [
            "prepared_codegen_cache_use",
            "value_facts",
            "ownership_effects",
            "validate_ownership_effects",
            "local_env_plan",
            "validate_local_env_plan",
            "local_env_resume_plan",
            "validate_local_env_resume_plan",
        ] {
            assert!(
                !timing_names.iter().any(|name| name == removed_timing),
                "pre-optimization lowering should not compute {removed_timing} for cached prepared codegen"
            );
        }
        assert!(
            !timing_names.iter().any(|name| matches!(
                name.as_str(),
                "inline_candidate_plan"
                    | "scalar_replace_constructor_allocations"
                    | "inline_direct_call_stores"
                    | "validate_codegen_instr_ids_after_inline"
            )),
            "pre-optimization lowering should not mutate the lowered module with inline cleanup"
        );
    }

    #[test]
    fn profile_and_verify_mode_add_block_entry_counters_only_for_profiled_cold_blocks() {
        let source = "def f(x):\n    if x:\n        return 1\n    return 0\n";
        for mode in [SpecializationMode::Profile, SpecializationMode::Verify] {
            let config = SoacEnvConfig::default().with_specialization_mode(Some(mode));
            let lowered = prepare_for_test(source, &config).expect("transform should succeed");
            assert!(
                lowered
                    .counter_defs
                    .iter()
                    .all(|counter| counter.kind != "block_entry"),
                "{mode:?} lowering should not attach block_entry counters by default"
            );
            assert_eq!(
                lowered
                    .counter_defs
                    .iter()
                    .filter(|counter| counter.kind == "branch_outcomes")
                    .count(),
                1,
                "{mode:?} lowering should still add branch_outcomes counters"
            );

            let config = config.with_profiled_cold_blocks_enabled(true);
            let lowered = prepare_for_test(source, &config).expect("transform should succeed");

            let block_entry_counters = lowered
                .counter_defs
                .iter()
                .filter(|counter| counter.kind == "block_entry")
                .collect::<Vec<_>>();
            let jit_function_ids = lowered
                .callable_defs
                .iter()
                .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
                .map(|function| function.function_id)
                .collect::<Vec<_>>();
            let total_jit_blocks = lowered
                .callable_defs
                .iter()
                .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
                .map(|function| function.blocks.len())
                .sum::<usize>();
            assert_eq!(
                block_entry_counters.len(),
                total_jit_blocks,
                "{mode:?} lowering should attach one block_entry counter per lowered JIT block when profiled cold blocks are enabled"
            );
            assert!(block_entry_counters.iter().all(|counter| {
                counter.scope == CounterScope::This
                    && matches!(
                        &counter.site,
                        CounterSite::BlockEntry { function_id, .. }
                        if jit_function_ids.contains(function_id)
                    )
            }));
            assert_eq!(
                lowered
                    .counter_defs
                    .iter()
                    .filter(|counter| counter.kind == "branch_outcomes")
                    .count(),
                1,
                "{mode:?} lowering should still add branch_outcomes counters"
            );
        }
    }

    #[test]
    fn typed_v3_profiled_cold_blocks_define_counters_without_codegen_increment_instrs() {
        struct IncrementCounterProbe {
            found: bool,
        }

        impl Visit<InstrCodegen> for IncrementCounterProbe {
            fn visit_instr(&mut self, expr: &InstrCodegen) {
                self.found |= matches!(expr, InstrCodegen::IncrementCounter(_));
                expr.visit_children(self);
            }
        }

        let source = "def f(x):\n    if x:\n        return 1\n    return 0\n";
        let config = SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Profile))
            .with_profiled_cold_blocks_enabled(true)
            .with_runtime_optimization_pipeline(RuntimeOptimizationPipeline::TypedV3);
        let lowered = prepare_for_test(source, &config).expect("transform should succeed");

        assert!(
            lowered
                .counter_defs
                .iter()
                .any(|counter| counter.kind == "block_entry"),
            "typed-v3 lowering should still define block_entry counters for runtime storage"
        );
        assert!(
            lowered
                .counter_defs
                .iter()
                .any(|counter| counter.kind == "branch_outcomes"),
            "typed-v3 lowering should define locality counters from InstrTyped"
        );
        let mut probe = IncrementCounterProbe { found: false };
        for function in &lowered.callable_defs {
            probe.visit_fn(function);
        }
        assert!(
            !probe.found,
            "typed-v3 lowering should not insert explicit counter instructions into Codegen IR"
        );
    }

    #[test]
    fn verify_mode_adds_refcount_counters_only_in_verify() {
        let source = "def f(x):\n    y = x\n    return y\n";

        {
            let config =
                SoacEnvConfig::default().with_specialization_mode(Some(SpecializationMode::Verify));
            assert!(refcount_counter_instrumentation_enabled(&config));
            let lowered = prepare_for_test(source, &config).expect("transform should succeed");
            let refcount_counters = lowered
                .counter_defs
                .iter()
                .filter(|counter| {
                    counter.kind == "runtime_incref" || counter.kind == "runtime_decref"
                })
                .collect::<Vec<_>>();
            let jit_function_ids = lowered
                .callable_defs
                .iter()
                .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
                .map(|function| function.function_id)
                .collect::<Vec<_>>();
            assert_eq!(refcount_counters.len(), jit_function_ids.len() * 2);
            assert!(refcount_counters.iter().all(|counter| {
                counter.scope == CounterScope::Function
                    && matches!(
                        &counter.site,
                        CounterSite::Runtime {
                            function_id: Some(function_id),
                            instr_id: None,
                        } if jit_function_ids.contains(function_id)
                    )
            }));
        }

        for mode in [SpecializationMode::Profile, SpecializationMode::Apply] {
            let config = SoacEnvConfig::default().with_specialization_mode(Some(mode));
            assert!(!refcount_counter_instrumentation_enabled(&config));
            let lowered = prepare_for_test(source, &config).expect("transform should succeed");
            assert!(
                lowered
                    .counter_defs
                    .iter()
                    .all(|counter| counter.kind != "runtime_incref"
                        && counter.kind != "runtime_decref"),
                "{mode:?} lowering should not add refcount counters"
            );
        }
    }

    #[test]
    fn apply_mode_preoptimization_lowering_does_not_define_deopt_entry_counters() {
        let config = SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Apply))
            .with_soac_work_dir(Some(fresh_test_work_dir("apply-counter")))
            .with_soac_log(
                SoacLogConfig {
                    filter: "soac_specialization_runtime=info".to_string(),
                    json_path: None,
                },
                true,
            );
        let source = "VALUE = 7\n\ndef read(x):\n    return x + VALUE\n";
        let lowered = prepare_for_test(source, &config).expect("transform should succeed");

        assert!(
            lowered.counter_defs.is_empty(),
            "pre-optimization apply lowering should not define counters just because runtime logging is enabled: {:?}",
            lowered
                .counter_defs
                .iter()
                .map(|counter| counter.kind.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn cached_runtime_finish_defines_deopt_entry_counters_on_final_module() {
        let source = "VALUE = 7\n\ndef read(x):\n    return x + VALUE\n";
        let module = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("transform should succeed")
            .codegen_module;
        let config = SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Apply))
            .with_soac_work_dir(Some(fresh_test_work_dir("final-apply-counter")));

        let finished =
            finish_cached_codegen_module_for_runtime_with_counter_defs(module, &config, &[])
                .expect("finish cached runtime module");

        assert!(
            finished
                .counter_defs
                .iter()
                .any(|counter| counter.kind == "deopt_entry_guard_miss"
                    && matches!(counter.site, CounterSite::DeoptEntry { .. })),
            "final apply module should define deopt-entry counters from the final local-env resume plan: {:?}",
            finished.counter_defs
        );
    }

    fn metadata(
        module_name: &str,
        source: &str,
        build_identity: &str,
    ) -> CachedCodegenModuleMetadata {
        pre_optimization_module_cache_metadata(
            PythonModuleCacheSource::Project,
            module_name,
            hash_module_source(source),
            build_identity,
            false,
        )
    }

    fn fresh_test_work_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "soac_driver-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ))
    }

    fn unique_temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "soac-blockpy-cache-test-{}-{}",
            std::process::id(),
            unique_suffix()
        ))
    }

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos()
    }
}
