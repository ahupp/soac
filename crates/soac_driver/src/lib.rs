pub mod codegen_cache;

use crate::codegen_cache::{
    CachedCodegenModule, CachedCodegenModuleMetadata, CachedPreparedCodegen,
    load_codegen_module_cache, remap_cached_codegen_module_function_ids,
    store_codegen_module_cache, validate_codegen_module_cache_metadata,
};
use soac_config::{SoacEnvConfig, init_logging_with_config};
use soac_core::block_py::{BlockPyModule, CounterScope, ModuleNameGen};
use soac_lowering::pass_tracker::{NoopPassTracker, PassTracker, RecordingPassTracker};
use soac_lowering::passes::{self, CodegenModuleShape};
pub use soac_lowering::{LoweringError, LoweringResult, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{info, warn};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoweringOptions {
    /// If `true`, compile `soac.runtime` bootstrap references as ordinary
    /// globals instead of `RuntimeName` constants. `RuntimeName` constants load
    /// from `soac.runtime`, so using them while compiling `soac.runtime` itself
    /// would make module initialization circular.
    pub runtime_names_as_globals: bool,
    pub pre_optimization_cache_path: Option<PathBuf>,
    pub pre_optimization_cache_metadata: Option<CachedCodegenModuleMetadata>,
}

impl LoweringOptions {
    const fn lowering_options(&self) -> soac_lowering::LoweringOptions {
        soac_lowering::LoweringOptions {
            runtime_names_as_globals: self.runtime_names_as_globals,
        }
    }
}

struct PreOptimizationModule {
    module: BlockPyModule<CodegenModuleShape>,
    prepared: Option<CachedPreparedCodegen>,
    cache_path_for_store: Option<PathBuf>,
    cache_metadata_for_store: Option<CachedCodegenModuleMetadata>,
}

fn lower_python_to_blockpy_with_tracker<P>(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: P,
) -> Result<LoweringResult<P>>
where
    P: PassTracker,
{
    lower_python_to_blockpy_with_tracker_and_options(
        source,
        module_name_gen,
        pass_tracker,
        LoweringOptions::default(),
    )
}

fn lower_python_to_blockpy_with_tracker_and_options<P>(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: P,
    options: LoweringOptions,
) -> Result<LoweringResult<P>>
where
    P: PassTracker,
{
    let env_config = SoacEnvConfig::from_env().map_err(anyhow::Error::msg)?;
    lower_python_to_blockpy_with_tracker_options_and_config(
        source,
        module_name_gen,
        pass_tracker,
        options,
        &env_config,
    )
}

fn lower_python_to_blockpy_with_tracker_options_and_config<P>(
    source: &str,
    module_name_gen: ModuleNameGen,
    mut pass_tracker: P,
    options: LoweringOptions,
    env_config: &SoacEnvConfig,
) -> Result<LoweringResult<P>>
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

    Ok(LoweringResult {
        total_time: total_start.elapsed(),
        codegen_module,
        pass_tracker,
    })
}

pub fn lower_python_to_blockpy_for_testing(source: &str) -> Result<LoweringResult> {
    lower_python_to_blockpy_with_tracker(source, ModuleNameGen::new(0), RecordingPassTracker::new())
}

pub fn lower_python_to_blockpy_for_testing_with_config(
    source: &str,
    env_config: &SoacEnvConfig,
) -> Result<LoweringResult> {
    lower_python_to_blockpy_with_tracker_options_and_config(
        source,
        ModuleNameGen::new(0),
        RecordingPassTracker::new(),
        LoweringOptions::default(),
        env_config,
    )
}

pub fn lower_python_to_blockpy(
    source: &str,
    module_name_gen: ModuleNameGen,
) -> Result<LoweringResult<NoopPassTracker>> {
    lower_python_to_blockpy_with_tracker(source, module_name_gen, NoopPassTracker::new())
}

pub fn lower_python_to_blockpy_recorded(
    source: &str,
    module_name_gen: ModuleNameGen,
) -> Result<LoweringResult<RecordingPassTracker>> {
    lower_python_to_blockpy_with_tracker(source, module_name_gen, RecordingPassTracker::new())
}

pub fn lower_python_to_blockpy_recorded_with_options(
    source: &str,
    module_name_gen: ModuleNameGen,
    options: LoweringOptions,
) -> Result<LoweringResult<RecordingPassTracker>> {
    lower_python_to_blockpy_with_tracker_and_options(
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
    options: LoweringOptions,
    env_config: &SoacEnvConfig,
) -> Result<BlockPyModule<CodegenModuleShape>> {
    let pre_optimization =
        rewrite_pre_optimization_module_with_cache(source, module_name_gen, pass_tracker, options)?;
    finish_codegen_module_with_tracker(pre_optimization, pass_tracker, env_config)
}

fn rewrite_pre_optimization_module_with_cache(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
    options: LoweringOptions,
) -> Result<PreOptimizationModule> {
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
                        let has_prepared = cache.prepared.is_some();
                        info!(
                            target: "soac_blockpy_module_cache",
                            event = "soac.blockpy_module_cache",
                            cache_hit = true,
                            prepared = has_prepared,
                            path = %cache_path.display(),
                            "blockpy_module_cache_hit",
                        );
                        let CachedCodegenModule {
                            metadata: _,
                            module,
                            prepared,
                        } = cache;
                        return Ok(PreOptimizationModule {
                            module: pass_tracker.run_pass("bb_codegen", || module),
                            prepared,
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
            options.lowering_options(),
        )?;
        Ok(PreOptimizationModule {
            module,
            prepared: None,
            cache_path_for_store: Some(cache_path.clone()),
            cache_metadata_for_store: options.pre_optimization_cache_metadata.clone(),
        })
    } else {
        let module = soac_lowering::lower_source_to_codegen_module_with_tracker(
            source,
            module_name_gen,
            pass_tracker,
            options.lowering_options(),
        )?;
        Ok(PreOptimizationModule {
            module,
            prepared: None,
            cache_path_for_store: None,
            cache_metadata_for_store: None,
        })
    }
}

fn store_pre_optimization_cache(
    cache_path: &Path,
    metadata: &CachedCodegenModuleMetadata,
    module: &BlockPyModule<CodegenModuleShape>,
    prepared: &CachedPreparedCodegen,
    pass_tracker: &mut impl PassTracker,
) {
    let stored = pass_tracker.record_timing("bb_codegen_cache_store", || {
        store_codegen_module_cache(cache_path, metadata, module, Some(prepared))
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
) -> Result<BlockPyModule<CodegenModuleShape>> {
    let PreOptimizationModule {
        module: bb_codegen,
        prepared,
        cache_path_for_store,
        cache_metadata_for_store,
    } = pre_optimization;
    pass_tracker.record_timing("validate_codegen_instr_ids", || {
        passes::validate_codegen_instr_ids(&bb_codegen).map_err(anyhow::Error::msg)
    })?;
    let prepared = if let Some(prepared) = prepared {
        pass_tracker.record_timing("prepared_codegen_cache_use", || prepared)
    } else {
        let escape_summary = pass_tracker.run_pass("escape_summary", || {
            passes::summarize_module_escapes(&bb_codegen)
        });
        let inline_plan = pass_tracker.run_pass("inline_plan", || {
            passes::plan_module_inlining(&escape_summary)
        });
        let value_facts: passes::FactStore = pass_tracker.record_timing("value_facts", || {
            passes::infer_module_value_facts(&bb_codegen)
        });
        let ownership_plan: passes::RefcountPlan = pass_tracker
            .record_timing("ownership_effects", || {
                passes::plan_ownership_effects(&bb_codegen, &value_facts)
            });
        pass_tracker.record_timing("validate_ownership_effects", || {
            passes::validate_ownership_effects(&bb_codegen, &value_facts, &ownership_plan)
                .map_err(anyhow::Error::msg)
        })?;
        let local_env_plan: passes::LocalEnvModulePlan = pass_tracker
            .record_timing("local_env_plan", || {
                passes::plan_local_env_module(&bb_codegen, &value_facts)
            });
        pass_tracker.record_timing("validate_local_env_plan", || {
            passes::validate_local_env_module_plan(&bb_codegen, &value_facts, &local_env_plan)
                .map_err(anyhow::Error::msg)
        })?;
        let local_env_resume_plan: passes::LocalEnvResumeModulePlan = pass_tracker
            .record_timing("local_env_resume_plan", || {
                passes::plan_local_env_resume_module(&bb_codegen, &local_env_plan, &value_facts)
            });
        pass_tracker.record_timing("validate_local_env_resume_plan", || {
            passes::validate_local_env_resume_module_plan(
                &bb_codegen,
                &local_env_plan,
                &value_facts,
                &local_env_resume_plan,
            )
            .map_err(anyhow::Error::msg)
        })?;
        CachedPreparedCodegen {
            escape_summary,
            inline_plan,
            value_facts,
            ownership_plan,
            local_env_plan,
            local_env_resume_plan,
        }
    };

    if let Some(cache_path) = &cache_path_for_store {
        if let Some(metadata) = &cache_metadata_for_store {
            store_pre_optimization_cache(
                cache_path,
                metadata,
                &bb_codegen,
                &prepared,
                pass_tracker,
            );
        }
    }

    let CachedPreparedCodegen {
        local_env_resume_plan,
        ..
    } = prepared;

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
                passes::instrument_bb_module_with_block_entry_counters(&mut counted);
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

    let bb_deopt_entry_counted: BlockPyModule<CodegenModuleShape> =
        if passes::deopt_entry_counter_instrumentation_enabled(env_config) {
            pass_tracker.record_timing("bb_deopt_entry_counters", || {
                let mut counted = bb_refcount_counted;
                passes::define_bb_module_deopt_entry_counters(&mut counted, &local_env_resume_plan);
                counted
            })
        } else {
            bb_refcount_counted
        };

    pass_tracker.record_timing("validate", || {
        soac_lowering::block_py::validate::validate_codegen_module(&bb_deopt_entry_counted)
            .map_err(anyhow::Error::msg)
    })?;

    Ok(bb_deopt_entry_counted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen_cache::{
        PythonModuleCacheSource, load_codegen_module_cache, store_codegen_module_cache,
    };
    use soac_config::{SoacEnvConfig, SoacLogConfig, SpecializationMode};
    use soac_core::block_py::{CounterSite, ModuleNameGen};
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
        store_codegen_module_cache(cache_path.as_path(), &stale_metadata, &stale_module, None)
            .expect("stale cache should be writable");

        if let Err(err) = lower_python_to_blockpy_recorded_with_options(
            source,
            ModuleNameGen::new(2),
            LoweringOptions {
                runtime_names_as_globals: false,
                pre_optimization_cache_path: Some(cache_path.clone()),
                pre_optimization_cache_metadata: Some(expected_metadata.clone()),
            },
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
    fn pre_optimization_lowering_does_not_run_inline_rewrite_cleanup() {
        let source = "def callee(x):\n    return x\n\ndef caller(x):\n    return callee(x)\n";
        let lowered = lower_python_to_blockpy_recorded(source, ModuleNameGen::new(0))
            .expect("transform should succeed");
        let pass_names = lowered.pass_tracker.pass_names().collect::<Vec<_>>();
        let timing_names = lowered
            .pass_tracker
            .pass_timings()
            .map(|timing| timing.name)
            .collect::<Vec<_>>();

        assert!(
            pass_names.contains(&"inline_plan"),
            "driver should still compute inline analysis facts for cached prepared codegen"
        );
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
    fn profile_mode_adds_block_entry_counters() {
        let config =
            SoacEnvConfig::default().with_specialization_mode(Some(SpecializationMode::Profile));
        let source = "def f(x):\n    if x:\n        return 1\n    return 0\n";
        let lowered = lower_python_to_blockpy_for_testing_with_config(source, &config)
            .expect("transform should succeed")
            .codegen_module;

        let block_entry_counters = lowered
            .counter_defs
            .iter()
            .filter(|counter| counter.kind == "block_entry")
            .collect::<Vec<_>>();
        let total_blocks = lowered
            .callable_defs
            .iter()
            .map(|function| function.blocks.len())
            .sum::<usize>();
        assert_eq!(
            block_entry_counters.len(),
            total_blocks,
            "profile lowering should attach one block_entry counter per lowered block"
        );
        assert!(block_entry_counters.iter().all(|counter| {
            counter.scope == CounterScope::This
                && matches!(counter.site, CounterSite::BlockEntry { .. })
        }));
        assert_eq!(
            lowered
                .counter_defs
                .iter()
                .filter(|counter| counter.kind == "branch_outcomes")
                .count(),
            1,
            "profile lowering should still add branch_outcomes counters"
        );
    }

    #[test]
    fn verify_mode_adds_refcount_counters_only_in_verify() {
        let source = "def f(x):\n    y = x\n    return y\n";

        {
            let config =
                SoacEnvConfig::default().with_specialization_mode(Some(SpecializationMode::Verify));
            assert!(passes::refcount_counter_instrumentation_enabled(&config));
            let lowered = lower_python_to_blockpy_for_testing_with_config(source, &config)
                .expect("transform should succeed")
                .codegen_module;
            let refcount_counters = lowered
                .counter_defs
                .iter()
                .filter(|counter| {
                    counter.kind == "runtime_incref" || counter.kind == "runtime_decref"
                })
                .collect::<Vec<_>>();
            assert_eq!(refcount_counters.len(), lowered.callable_defs.len() * 2);
            assert!(refcount_counters.iter().all(|counter| {
                counter.scope == CounterScope::Function
                    && matches!(
                        counter.site,
                        CounterSite::Runtime {
                            function_id: Some(_),
                            instr_id: None,
                        }
                    )
            }));
        }

        for mode in [SpecializationMode::Profile, SpecializationMode::Apply] {
            let config = SoacEnvConfig::default().with_specialization_mode(Some(mode));
            assert!(!passes::refcount_counter_instrumentation_enabled(&config));
            let lowered = lower_python_to_blockpy_for_testing_with_config(source, &config)
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
    fn apply_mode_keeps_only_deopt_entry_counters_even_with_runtime_logging() {
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
        let lowered = lower_python_to_blockpy_for_testing_with_config(source, &config)
            .expect("transform should succeed")
            .codegen_module;

        assert!(
            lowered
                .counter_defs
                .iter()
                .all(|counter| counter.kind == "deopt_entry_guard_miss"),
            "apply lowering should keep only deopt-entry counters just because runtime logging is enabled: {:?}",
            lowered
                .counter_defs
                .iter()
                .map(|counter| counter.kind.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            lowered
                .counter_defs
                .iter()
                .all(|counter| matches!(counter.site, CounterSite::DeoptEntry { .. })),
            "apply deopt-entry counters should carry source metadata: {:?}",
            lowered.counter_defs
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
