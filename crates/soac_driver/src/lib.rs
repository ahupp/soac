pub mod codegen_cache;

use crate::codegen_cache::{
    CachedCodegenModule, CachedCodegenModuleMetadata, load_codegen_module_cache,
    remap_cached_codegen_module_function_ids, store_codegen_module_cache,
    validate_codegen_module_cache_metadata,
};
use soac_config::{SoacEnvConfig, init_logging_with_config};
use soac_core::block_py::{BlockPyModule, CounterDef, CounterId, CounterScope, ModuleNameGen};
use soac_lowering::pass_tracker::{NoopPassTracker, PassTracker, RecordingPassTracker};
use soac_lowering::passes::{self, CodegenModuleShape, InstrCodegen};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{info, warn};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodegenPreparationOptions {
    pub lowering: soac_lowering::LoweringOptions,
    pub pre_optimization_cache_path: Option<PathBuf>,
    pub pre_optimization_cache_metadata: Option<CachedCodegenModuleMetadata>,
}

impl CodegenPreparationOptions {
    pub fn with_runtime_names_as_globals(mut self, runtime_names_as_globals: bool) -> Self {
        self.lowering.runtime_names_as_globals = runtime_names_as_globals;
        self
    }

    pub fn with_pre_optimization_cache(
        mut self,
        path: PathBuf,
        metadata: CachedCodegenModuleMetadata,
    ) -> Self {
        self.pre_optimization_cache_path = Some(path);
        self.pre_optimization_cache_metadata = Some(metadata);
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

struct PreOptimizationModule {
    module: BlockPyModule<CodegenModuleShape>,
    cache_path_for_store: Option<PathBuf>,
    cache_metadata_for_store: Option<CachedCodegenModuleMetadata>,
}

fn prepare_codegen_module_with_tracker<P>(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: P,
) -> soac_lowering::Result<soac_lowering::LoweringResult<P>>
where
    P: PassTracker,
{
    prepare_codegen_module_with_tracker_and_options(
        source,
        module_name_gen,
        pass_tracker,
        CodegenPreparationOptions::default(),
    )
}

fn prepare_codegen_module_with_tracker_and_options<P>(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: P,
    options: CodegenPreparationOptions,
) -> soac_lowering::Result<soac_lowering::LoweringResult<P>>
where
    P: PassTracker,
{
    let env_config = SoacEnvConfig::from_env().map_err(anyhow::Error::msg)?;
    prepare_codegen_module_with_tracker_options_and_config(
        source,
        module_name_gen,
        pass_tracker,
        options,
        &env_config,
    )
}

fn prepare_codegen_module_with_tracker_options_and_config<P>(
    source: &str,
    module_name_gen: ModuleNameGen,
    mut pass_tracker: P,
    options: CodegenPreparationOptions,
    env_config: &SoacEnvConfig,
) -> soac_lowering::Result<soac_lowering::LoweringResult<P>>
where
    P: PassTracker,
{
    init_logging_with_config(env_config).map_err(anyhow::Error::msg)?;
    soac_lowering::reset_lowering_state();
    let total_start = Instant::now();

    let codegen_module = rewrite_module_with_tracker_with_options(
        source,
        module_name_gen,
        &mut pass_tracker,
        options,
        env_config,
    )?;

    Ok(soac_lowering::LoweringResult {
        total_time: total_start.elapsed(),
        codegen_module,
        pass_tracker,
    })
}

pub fn prepare_codegen_module_for_testing_with_config(
    source: &str,
    env_config: &SoacEnvConfig,
) -> soac_lowering::Result<soac_lowering::LoweringResult> {
    prepare_codegen_module_with_tracker_options_and_config(
        source,
        ModuleNameGen::new(0),
        RecordingPassTracker::new(),
        CodegenPreparationOptions::default(),
        env_config,
    )
}

pub fn prepare_codegen_module(
    source: &str,
    module_name_gen: ModuleNameGen,
) -> soac_lowering::Result<soac_lowering::LoweringResult<NoopPassTracker>> {
    prepare_codegen_module_with_tracker(source, module_name_gen, NoopPassTracker::new())
}

pub fn prepare_codegen_module_recorded(
    source: &str,
    module_name_gen: ModuleNameGen,
) -> soac_lowering::Result<soac_lowering::LoweringResult<RecordingPassTracker>> {
    prepare_codegen_module_with_tracker(source, module_name_gen, RecordingPassTracker::new())
}

pub fn prepare_codegen_module_recorded_with_options(
    source: &str,
    module_name_gen: ModuleNameGen,
    options: CodegenPreparationOptions,
) -> soac_lowering::Result<soac_lowering::LoweringResult<RecordingPassTracker>> {
    prepare_codegen_module_with_tracker_and_options(
        source,
        module_name_gen,
        RecordingPassTracker::new(),
        options,
    )
}

fn rewrite_module_with_tracker_with_options(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
    options: CodegenPreparationOptions,
    env_config: &SoacEnvConfig,
) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
    let pre_optimization =
        rewrite_pre_optimization_module_with_cache(source, module_name_gen, pass_tracker, options)?;
    finish_codegen_module_with_tracker(pre_optimization, pass_tracker, env_config)
}

fn rewrite_pre_optimization_module_with_cache(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
    options: CodegenPreparationOptions,
) -> soac_lowering::Result<PreOptimizationModule> {
    if let Some(cache_path) = &options.pre_optimization_cache_path {
        let cache_exists =
            pass_tracker.record_timing("bb_codegen_cache_lookup", || cache_path.is_file());
        if cache_exists {
            let loaded = pass_tracker.record_timing("bb_codegen_cache_load", || {
                load_codegen_module_cache(cache_path)
            });
            match loaded {
                Ok(mut cache) => {
                    let metadata_mismatch = if let Some(expected) =
                        &options.pre_optimization_cache_metadata
                    {
                        match validate_codegen_module_cache_metadata(&cache.metadata, expected) {
                            Ok(()) => None,
                            Err(err) => Some(err),
                        }
                    } else {
                        None
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
                    } else {
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
                        return Ok(PreOptimizationModule {
                            module: pass_tracker.run_pass("bb_codegen", || module),
                            cache_path_for_store: None,
                            cache_metadata_for_store: None,
                        });
                    }
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
                }
            }
        } else {
            info!(
                target: "soac_blockpy_module_cache",
                event = "soac.blockpy_module_cache",
                cache_hit = false,
                path = %cache_path.display(),
                "blockpy_module_cache_miss",
            );
        }

        let module = soac_lowering::lower_source_to_codegen_module_with_tracker(
            source,
            module_name_gen,
            pass_tracker,
            options.lowering.clone(),
        )?;
        Ok(PreOptimizationModule {
            module,
            cache_path_for_store: Some(cache_path.clone()),
            cache_metadata_for_store: options.pre_optimization_cache_metadata.clone(),
        })
    } else {
        let module = soac_lowering::lower_source_to_codegen_module_with_tracker(
            source,
            module_name_gen,
            pass_tracker,
            options.lowering.clone(),
        )?;
        Ok(PreOptimizationModule {
            module,
            cache_path_for_store: None,
            cache_metadata_for_store: None,
        })
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

fn finish_codegen_module_with_tracker(
    pre_optimization: PreOptimizationModule,
    pass_tracker: &mut impl PassTracker,
    env_config: &SoacEnvConfig,
) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
    let PreOptimizationModule {
        module: bb_codegen,
        cache_path_for_store,
        cache_metadata_for_store,
    } = pre_optimization;
    pass_tracker.record_timing("validate_codegen_instr_ids", || {
        passes::validate_codegen_instr_ids(&bb_codegen).map_err(anyhow::Error::msg)
    })?;

    if let Some(cache_path) = &cache_path_for_store {
        if let Some(metadata) = &cache_metadata_for_store {
            store_pre_optimization_cache(cache_path, metadata, &bb_codegen, pass_tracker);
        }
    }

    let bb_traced: BlockPyModule<CodegenModuleShape> =
        if let Some(config) = passes::parse_trace_env(env_config) {
            pass_tracker.run_pass("bb_trace", || {
                let mut traced = bb_codegen;
                passes::instrument_bb_module_for_trace(&mut traced, &config);
                traced
            })
        } else {
            bb_codegen
        };

    let bb_call_target_counted: BlockPyModule<CodegenModuleShape> =
        if passes::call_target_counter_instrumentation_enabled(env_config) {
            pass_tracker.run_pass("bb_call_target_counters", || {
                let mut counted = bb_traced;
                passes::instrument_bb_module_with_call_target_counters(&mut counted);
                counted
            })
        } else {
            bb_traced
        };

    let bb_locality_counted: BlockPyModule<CodegenModuleShape> =
        if passes::locality_counter_instrumentation_enabled(env_config) {
            pass_tracker.run_pass("bb_locality_counters", || {
                let mut counted = bb_call_target_counted;
                if env_config.profiled_cold_blocks_enabled() {
                    passes::instrument_bb_module_with_block_entry_counters(&mut counted);
                }
                passes::instrument_bb_module_with_locality_counters(&mut counted);
                counted
            })
        } else {
            bb_call_target_counted
        };

    let bb_refcount_counted: BlockPyModule<CodegenModuleShape> =
        if passes::refcount_counter_instrumentation_enabled(env_config) {
            pass_tracker.record_timing("bb_refcount_counters", || {
                let mut counted = bb_locality_counted;
                passes::instrument_bb_module_with_refcount_counters(
                    &mut counted,
                    CounterScope::Function,
                )
                .map_err(anyhow::Error::msg)?;
                Ok::<BlockPyModule<CodegenModuleShape>, anyhow::Error>(counted)
            })?
        } else {
            bb_locality_counted
        };

    pass_tracker.record_timing("validate", || {
        soac_lowering::block_py::validate::validate_codegen_module(&bb_refcount_counted)
            .map_err(anyhow::Error::msg)
    })?;

    Ok(bb_refcount_counted)
}

fn define_deopt_entry_counters_for_current_module(
    module: &mut BlockPyModule<CodegenModuleShape>,
    pass_tracker: &mut impl PassTracker,
) -> soac_lowering::Result<()> {
    let value_facts = pass_tracker.record_timing("deopt_entry_value_facts", || {
        passes::infer_module_value_facts(module)
    });
    let local_env_plan = pass_tracker.record_timing("deopt_entry_local_env_plan", || {
        passes::plan_local_env_module(module, &value_facts)
    });
    pass_tracker.record_timing("validate_deopt_entry_local_env_plan", || {
        passes::validate_local_env_module_plan(module, &value_facts, &local_env_plan)
            .map_err(anyhow::Error::msg)
    })?;
    let local_env_resume_plan = pass_tracker
        .record_timing("deopt_entry_local_env_resume_plan", || {
            passes::plan_local_env_resume_module(module, &local_env_plan, &value_facts)
        });
    pass_tracker.record_timing("validate_deopt_entry_local_env_resume_plan", || {
        passes::validate_local_env_resume_module_plan(
            module,
            &local_env_plan,
            &value_facts,
            &local_env_resume_plan,
        )
        .map_err(anyhow::Error::msg)
    })?;
    passes::define_bb_module_deopt_entry_counters(module, &local_env_resume_plan);
    Ok(())
}

pub fn finish_cached_codegen_module_for_runtime(
    module: BlockPyModule<CodegenModuleShape>,
    env_config: &SoacEnvConfig,
) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
    finish_codegen_module_with_tracker(
        PreOptimizationModule {
            module,
            cache_path_for_store: None,
            cache_metadata_for_store: None,
        },
        &mut NoopPassTracker::new(),
        env_config,
    )
}

pub fn finish_cached_codegen_module_for_runtime_with_counter_defs(
    module: BlockPyModule<CodegenModuleShape>,
    env_config: &SoacEnvConfig,
    counter_defs: &[CounterDef],
) -> soac_lowering::Result<BlockPyModule<CodegenModuleShape>> {
    let mut module = finish_cached_codegen_module_for_runtime(module, env_config)?;
    retain_defined_explicit_counter_increments(&mut module, counter_defs);
    module.counter_defs = counter_defs.to_vec();
    if passes::deopt_entry_counter_instrumentation_enabled(env_config) {
        define_deopt_entry_counters_for_current_module(&mut module, &mut NoopPassTracker::new())?;
    }
    Ok(module)
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
    use soac_config::{SoacEnvConfig, SoacLogConfig, SpecializationMode};
    use soac_core::block_py::{CounterSite, FunctionExecutionMode, ModuleNameGen};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn pre_optimization_cache_metadata_mismatch_rebuilds_and_replaces_cache() {
        let source = "def f():\n    return 1\n";
        let module_name = "cache_metadata_mismatch_test";
        let cache_path = unique_temp_dir()
            .join("project")
            .join(module_name)
            .join("mod.blockpy");
        let stale_metadata = metadata(module_name, "old-build");
        let expected_metadata = metadata(module_name, "new-build");
        let stale_module =
            soac_lowering::lower_python_to_blockpy_recorded(source, ModuleNameGen::new(1))
                .expect("initial lowering should succeed")
                .codegen_module;
        store_codegen_module_cache(cache_path.as_path(), &stale_metadata, &stale_module)
            .expect("stale cache should be writable");

        if let Err(err) = prepare_codegen_module_recorded_with_options(
            source,
            ModuleNameGen::new(2),
            CodegenPreparationOptions::default()
                .with_pre_optimization_cache(cache_path.clone(), expected_metadata.clone()),
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
        let lowered = prepare_codegen_module_recorded(source, ModuleNameGen::new(0))
            .expect("transform should succeed");
        let pass_names = lowered.pass_tracker.pass_names().collect::<Vec<_>>();
        let timing_names = lowered
            .pass_tracker
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
            let lowered = prepare_codegen_module_for_testing_with_config(source, &config)
                .expect("transform should succeed")
                .codegen_module;
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
            let lowered = prepare_codegen_module_for_testing_with_config(source, &config)
                .expect("transform should succeed")
                .codegen_module;

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
    fn verify_mode_adds_refcount_counters_only_in_verify() {
        let source = "def f(x):\n    y = x\n    return y\n";

        {
            let config =
                SoacEnvConfig::default().with_specialization_mode(Some(SpecializationMode::Verify));
            assert!(passes::refcount_counter_instrumentation_enabled(&config));
            let lowered = prepare_codegen_module_for_testing_with_config(source, &config)
                .expect("transform should succeed")
                .codegen_module;
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
            assert!(!passes::refcount_counter_instrumentation_enabled(&config));
            let lowered = prepare_codegen_module_for_testing_with_config(source, &config)
                .expect("transform should succeed")
                .codegen_module;
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
        let lowered = prepare_codegen_module_for_testing_with_config(source, &config)
            .expect("transform should succeed")
            .codegen_module;

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

    fn metadata(module_name: &str, cache_identity: &str) -> CachedCodegenModuleMetadata {
        CachedCodegenModuleMetadata {
            source: PythonModuleCacheSource::Project,
            module_name: module_name.to_string(),
            source_hash: 0x1234,
            cache_identity: cache_identity.to_string(),
        }
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
