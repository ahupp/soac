pub mod blockpy_cache;
pub mod typed_runtime;

use crate::blockpy_cache::{
    PreOptimizationCacheTarget, PythonModuleCacheSource, hash_module_source,
    pre_optimization_module_cache_metadata, pre_optimization_module_cache_path,
    store_pre_optimization_cache, try_load_pre_optimization_cache,
};
use soac_config::SoacEnvConfig;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, CounterScope as BlockPyCounterScope,
    CounterSite as BlockPyCounterSite, FunctionExecutionMode as BlockPyFunctionExecutionMode,
    ModuleNameGen,
};
use soac_core::pass_tracker::PassTracker;
use soac_instrument::{
    CounterBuilder, InstrumentationConfig, RUNTIME_DECREF_LOCATION_COUNTER_KIND,
    define_typed_module_counter_defs,
};
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::{TypedBlockPyModuleShape, lower_blockpy_module_to_typed};
pub use soac_lowering::{LoweringError, Result};
use soac_opt::passes::{
    REFCOUNT_STACK_SLOT_DECREF_PURPOSES, RefcountActionKind, RefcountPlan,
    infer_module_value_facts, plan_typed_ownership_effects, refcount_release_location_branch_name,
    refcount_stack_slot_location_branch_name,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceToBlockPyOptions {
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

impl SourceToBlockPyOptions {
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

impl From<soac_lowering::LoweringOptions> for SourceToBlockPyOptions {
    fn from(lowering: soac_lowering::LoweringOptions) -> Self {
        Self {
            lowering,
            ..Self::default()
        }
    }
}

pub fn source_to_blockpy(
    source: &str,
    module_name_gen: ModuleNameGen,
    options: SourceToBlockPyOptions,
    env_config: &SoacEnvConfig,
    pass_tracker: &mut impl PassTracker,
) -> soac_lowering::Result<BlockPyModule<BlockPyModuleShape>> {
    let pre_optimization =
        load_or_lower_pre_optimization_module(source, module_name_gen, pass_tracker, options)?;
    finish_pre_optimization_module(pre_optimization, pass_tracker, env_config)
}

fn load_or_lower_pre_optimization_module(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
    options: SourceToBlockPyOptions,
) -> soac_lowering::Result<BlockPyModule<BlockPyModuleShape>> {
    let cache_target = options
        .pre_optimization_cache
        .as_ref()
        // A writable serialized IR cache is not an authenticated executable.
        // Until it has an independently authenticated IR/source binding, strict
        // imports lower the freshly verified source through the normal passes.
        .filter(|_| options.lowering.strict_facts.is_none())
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

    let module = soac_lowering::lower_source_to_blockpy_module_with_tracker(
        source,
        module_name_gen,
        pass_tracker,
        options.lowering.clone(),
    )?;

    if let Some(cache_target) = &cache_target {
        store_pre_optimization_cache(cache_target, &module, pass_tracker);
    }

    Ok(module)
}

fn finish_pre_optimization_module(
    mut blockpy: BlockPyModule<BlockPyModuleShape>,
    pass_tracker: &mut impl PassTracker,
    env_config: &SoacEnvConfig,
) -> soac_lowering::Result<BlockPyModule<BlockPyModuleShape>> {
    soac_ir_blockpy::ensure_constructor_entry_functions(&mut blockpy);

    pass_tracker.record_timing("validate_blockpy_instr_ids", || {
        soac_ir_blockpy::validate_blockpy_instr_ids(&blockpy).map_err(anyhow::Error::msg)
    })?;

    let instrumentation_config = InstrumentationConfig::from_env_config(env_config);
    let mut typed_for_counters = lower_blockpy_module_to_typed(blockpy.clone());
    define_typed_module_counter_defs(&mut typed_for_counters, &instrumentation_config)
        .map_err(anyhow::Error::msg)?;
    if matches!(
        instrumentation_config.counters.refcounts.scope(),
        Some(BlockPyCounterScope::Function)
    ) {
        let value_facts = pass_tracker.record_timing("value_facts_for_refcount_counters", || {
            infer_module_value_facts(&blockpy)
        });
        let refcount_plan = pass_tracker
            .record_timing("ownership_effects_for_refcount_counters", || {
                plan_typed_ownership_effects(&typed_for_counters, &value_facts)
            });
        define_refcount_release_location_counter_defs(&mut typed_for_counters, &refcount_plan);
    }
    blockpy.counter_defs = typed_for_counters.counter_defs;
    Ok(blockpy)
}

fn define_refcount_release_location_counter_defs(
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
    refcount_plan: &RefcountPlan,
) {
    let mut counters = CounterBuilder::new(&mut module.counter_defs);
    for function in module
        .callable_defs
        .iter()
        .filter(|function| function.execution_mode() == BlockPyFunctionExecutionMode::Jit)
    {
        let branches = refcount_release_location_branches_for_function(function, refcount_plan);
        if branches.is_empty() {
            continue;
        }
        counters.define_branch_counter_if_missing(
            BlockPyCounterScope::Function,
            RUNTIME_DECREF_LOCATION_COUNTER_KIND,
            BlockPyCounterSite::Runtime {
                function_id: Some(function.function_id),
                instr_id: None,
            },
            branches,
        );
    }
}

fn refcount_release_location_branches_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    refcount_plan: &RefcountPlan,
) -> BTreeSet<String> {
    let mut branches = BTreeSet::new();
    if let Some(storage_layout) = function.storage_layout().as_ref() {
        for (slot_index, name) in storage_layout.stack_slots().iter().enumerate() {
            for purpose in REFCOUNT_STACK_SLOT_DECREF_PURPOSES {
                branches.insert(refcount_stack_slot_location_branch_name(
                    purpose, slot_index, name,
                ));
            }
        }
    }
    if let Some(function_plan) = refcount_plan.function(function.function_id) {
        for block_plan in function_plan.blocks.values() {
            for action in &block_plan.actions {
                let RefcountActionKind::ReleaseLocal {
                    local,
                    state,
                    reason,
                } = &action.kind
                else {
                    continue;
                };
                if !state.needs_decref() {
                    continue;
                }
                branches.insert(refcount_release_location_branch_name(
                    block_plan.label,
                    local,
                    reason,
                ));
            }
        }
    }
    branches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockpy_cache::{
        CachedBlockPyModuleMetadata, PythonModuleCacheSource, load_blockpy_module_cache,
        store_blockpy_module_cache,
    };
    use soac_config::{SoacEnvConfig, SoacLogConfig, SpecializationMode};
    use soac_core::block_py::{
        ChildVisitable, CounterScope, CounterSite, FunctionExecutionMode, ModuleNameGen, Visit,
    };
    use soac_core::pass_tracker::RecordingPassTracker;
    use soac_ir_blockpy::InstrBlockPy;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Compiler-only source/phase fixture, not checker or native admission.
    fn prepare_suspended_owner_fixture(source: &str) -> BlockPyModule<BlockPyModuleShape> {
        use soac_contracts::*;
        let hash = Fingerprint::digest(b"suspended-owner-phase-test");
        let policy = ResolvedStrictPolicy::default();
        let environment = ArtifactEnvironment {
            ty_revision: "d2620d7312875790b114d821721cddf253f66423".into(),
            checker_source_fingerprint: hash,
            exporter_revision: "suspended-owner-phase-test".into(),
            python_version: PythonVersion {
                major: 3,
                minor: 15,
            },
            python_platform: "linux".into(),
            cpython_abi_fingerprint: hash,
            normalized_project_policy: hash,
            resolved_typechecker_configuration: hash,
            import_search_path: hash,
            typeshed_fingerprint: hash,
            installed_stub_fingerprint: hash,
            installed_dependency_fingerprint: hash,
            analysis: ConservativeAnalysis::default(),
        };
        let facts = ModuleTypeFacts::new(
            "suspended_owner_phase",
            source.as_bytes(),
            SourceDialect::SoacStrict,
            policy.clone(),
        )
        .unwrap();
        let shard = encode_module_shard(&facts).unwrap();
        let manifest = TypeArtifactManifest::new(
            environment.clone(),
            vec![ModuleArtifactIndex::from_shard(&shard).unwrap()],
        )
        .unwrap();
        let key = ArtifactSigningKey::from_bytes(&[91; 32]);
        let expected = ArtifactExpectations {
            generation: manifest.generation,
            environment,
        };
        let manifest = verify_manifest(
            &sign_manifest(&manifest, &key).unwrap(),
            &key.trust_anchor(),
            &expected,
        )
        .unwrap();
        let generation =
            verify_complete_generation(manifest, |_| Ok(shard.bytes().to_vec())).unwrap();
        let verified = generation
            .manifest()
            .verify_module(
                "suspended_owner_phase",
                source.as_bytes(),
                &policy,
                &[],
                shard.bytes(),
            )
            .unwrap();
        source_to_blockpy(
            source,
            ModuleNameGen::new(1),
            SourceToBlockPyOptions {
                lowering: soac_lowering::LoweringOptions {
                    strict_facts: Some(std::sync::Arc::new(verified)),
                    ..Default::default()
                },
                ..Default::default()
            },
            &SoacEnvConfig::default(),
            &mut RecordingPassTracker::new(),
        )
        .expect("actual strict source ownership producer")
    }

    fn assert_suspended_owner_transfers_survive_state_lowering(body: &str) {
        use soac_core::block_py::{
            BlockTerm, NameLocation, PreservedLocation, PreservedSlotStorage,
        };
        use soac_ir_typed::InstrTyped;
        use soac_opt::passes::{
            ensure_typed_generator_resume_boundary_writebacks,
            lower_typed_generator_resume_preserved_state_to_locals_and_collect_preserved_locals,
        };
        let source = format!("from __future__ import strict\n{body}");
        let lowered = prepare_suspended_owner_fixture(&source);
        let mut prepared = crate::typed_runtime::prepare_typed_v3_runtime_module(
            &lowered,
            &SoacEnvConfig::default(),
        )
        .unwrap();
        let function = prepared
            .module
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "values")
            .unwrap();
        let original_layout = function.public_storage_layout().unwrap().clone();
        let outcome =
            lower_typed_generator_resume_preserved_state_to_locals_and_collect_preserved_locals(
                function,
            );
        assert_eq!(
            outcome.stats.lowered_functions, 1,
            "real resume-state rewrite must run"
        );
        assert_eq!(
            function.public_storage_layout(),
            Some(&original_layout),
            "public suspended storage must not become resume-local addresses"
        );
        let promoted_source_slots = original_layout
            .preserved_slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| {
                slot.generator_control.is_none()
                    && matches!(
                        slot.storage,
                        PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::PyCellObject
                    )
            })
            .map(|(index, _)| PreservedLocation(index as u32))
            .filter(|slot| outcome.preserved_locals.contains_key(slot))
            .collect::<Vec<_>>();
        assert!(
            !promoted_source_slots.is_empty(),
            "fixture must move actual semantic owners"
        );

        for slot in &promoted_source_slots {
            let active = &outcome.preserved_locals[slot];
            let entry = function.entry_block();
            let acquire = entry.body.iter().position(|instr| {
                matches!(instr, InstrTyped::Store(store)
                    if store.name == *active && matches!(store.value.as_ref(),
                        InstrTyped::Load(load) if load.name.location == NameLocation::Preserved(*slot)))
            }).expect("active owner is acquired from preserved state");
            let retire = entry
                .body
                .iter()
                .position(|instr| {
                    matches!(instr, InstrTyped::Del(del)
                    if del.quietly && del.name.location == NameLocation::Preserved(*slot))
                })
                .expect("entry must not leave a second preserved owner pin");
            assert!(acquire < retire);
            for block in &function.blocks {
                if matches!(block.term, BlockTerm::Return(_)) {
                    assert!(
                        block.body.iter().any(|instr| {
                            matches!(instr, InstrTyped::Store(store)
                            if store.name.location == NameLocation::Preserved(*slot)
                                && matches!(store.value.as_ref(),
                                    InstrTyped::Load(load) if load.name == *active))
                        }),
                        "suspension must restore the current source owner"
                    );
                }
            }
        }

        // Exercise the same late repair used after optimizer rewrites. Restore
        // only missing suspension copies, without changing semantic cleanup.
        assert_eq!(
            ensure_typed_generator_resume_boundary_writebacks(function, &outcome.preserved_locals),
            0
        );
        let repaired_slot = promoted_source_slots[0];
        let mut removed = 0;
        for block in &mut function.blocks {
            if matches!(block.term, BlockTerm::Return(_)) {
                block.body.retain(|instr| {
                    let remove = matches!(instr, InstrTyped::Store(store)
                        if store.name.location == NameLocation::Preserved(repaired_slot));
                    removed += usize::from(remove);
                    !remove
                });
            }
        }
        assert!(removed > 0);
        assert_eq!(
            ensure_typed_generator_resume_boundary_writebacks(function, &outcome.preserved_locals),
            removed
        );
    }

    #[test]
    fn suspended_owner_transfers_survive_suspended_assignment_state_lowering() {
        assert_suspended_owner_transfers_survive_state_lowering(concat!(
            "def values(make, record, reject, key):\n",
            "    try:\n",
            "        first, reject().field = yield \"ready\"\n",
            "    except AttributeError:\n",
            "        record(\"handler\")\n",
            "    del first\n",
            "    record(\"after\")\n",
            "    yield \"done\"\n",
        ));
    }

    #[test]
    fn suspended_owner_transfers_survive_error_injection_state_lowering() {
        assert_suspended_owner_transfers_survive_state_lowering(concat!(
            "def values(observe):\n",
            "    try:\n",
            "        yield \"ready\"\n",
            "    except BaseException as error:\n",
            "        observe(error)\n",
        ));
    }

    #[test]
    fn suspended_owner_transfers_survive_owned_cell_state_lowering() {
        assert_suspended_owner_transfers_survive_state_lowering(concat!(
            "def values(observe):\n",
            "    def closure():\n",
            "        return observe\n",
            "    try:\n",
            "        yield closure\n",
            "    except BaseException as error:\n",
            "        closure()(error)\n",
        ));
    }

    #[test]
    fn suspended_owner_transfers_survive_coroutine_state_lowering() {
        assert_suspended_owner_transfers_survive_state_lowering(concat!(
            "async def values(observe, pause):\n",
            "    await pause()\n",
            "    observe()\n",
        ));
    }

    #[test]
    fn suspended_owner_transfers_survive_async_generator_state_lowering() {
        assert_suspended_owner_transfers_survive_state_lowering(concat!(
            "async def values(observe):\n",
            "    try:\n",
            "        yield 1\n",
            "    except BaseException as error:\n",
            "        observe(error)\n",
        ));
    }

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
    ) -> soac_lowering::Result<BlockPyModule<BlockPyModuleShape>> {
        let mut pass_tracker = RecordingPassTracker::new();
        source_to_blockpy(
            source,
            ModuleNameGen::new(0),
            SourceToBlockPyOptions::default(),
            config,
            &mut pass_tracker,
        )
    }

    fn prepare_recorded_for_test(
        source: &str,
    ) -> soac_lowering::Result<(BlockPyModule<BlockPyModuleShape>, RecordingPassTracker)> {
        let mut pass_tracker = RecordingPassTracker::new();
        let module = source_to_blockpy(
            source,
            ModuleNameGen::new(0),
            SourceToBlockPyOptions::default(),
            &SoacEnvConfig::default(),
            &mut pass_tracker,
        )?;
        Ok((module, pass_tracker))
    }

    #[test]
    fn pre_optimization_cache_metadata_mismatch_rebuilds_and_replaces_cache() {
        let source = "def f():\n    return 1\n";
        let module_name = "cache_metadata_mismatch_test";
        let work_dir = unique_temp_dir();
        let config = SoacEnvConfig::default().with_soac_work_dir(Some(work_dir.clone()));
        let cache_root = config
            .module_cache_root()
            .expect("work directory enables the compiler cache");
        let cache_path = work_dir
            .join("modules")
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
        .blockpy_module;
        store_blockpy_module_cache(cache_path.as_path(), &stale_metadata, &stale_module)
            .expect("stale cache should be writable");

        let mut pass_tracker = RecordingPassTracker::new();
        if let Err(err) = source_to_blockpy(
            source,
            ModuleNameGen::new(2),
            SourceToBlockPyOptions::default().with_pre_optimization_cache(
                cache_root.clone(),
                PythonModuleCacheSource::Project,
                module_name,
                "new-build",
            ),
            &config,
            &mut pass_tracker,
        ) {
            panic!(
                "stale cache metadata should be treated as a miss, not as a lowering error: {err}"
            );
        }

        let replaced = load_blockpy_module_cache(cache_path.as_path())
            .expect("rebuilt cache should be readable");
        assert_eq!(replaced.metadata, expected_metadata);

        // The ordinary compiler still reuses its cache under SOAC_WORK_DIR.
        // Strict runtime imports deliberately bypass this unauthenticated IR.
        let mut warm_tracker = RecordingPassTracker::new();
        let warm_module = source_to_blockpy(
            source,
            ModuleNameGen::new(3),
            SourceToBlockPyOptions::default().with_pre_optimization_cache(
                cache_root,
                PythonModuleCacheSource::Project,
                module_name,
                "new-build",
            ),
            &config,
            &mut warm_tracker,
        )
        .expect("matching ordinary compiler cache should load");
        assert_eq!(warm_module.module_name_gen.module_id(), 3);
        assert!(
            warm_tracker
                .pass_timings()
                .any(|timing| timing.name == "blockpy_cache_load")
        );
    }

    #[test]
    fn pre_optimization_lowering_does_not_compute_or_store_prepared_blockpy_facts() {
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
                "pre-optimization lowering should not compute {removed_pass} for cached prepared BlockPy"
            );
        }
        for removed_timing in [
            "prepared_blockpy_cache_use",
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
                "pre-optimization lowering should not compute {removed_timing} for cached prepared BlockPy"
            );
        }
        assert!(
            !timing_names.iter().any(|name| matches!(
                name.as_str(),
                "inline_candidate_plan"
                    | "scalar_replace_constructor_allocations"
                    | "inline_direct_call_stores"
                    | "validate_blockpy_instr_ids_after_inline"
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
    fn profiled_cold_blocks_define_typed_counters_without_blockpy_increment_instrs() {
        struct IncrementCounterProbe {
            found: bool,
        }

        impl Visit<InstrBlockPy> for IncrementCounterProbe {
            fn visit_instr(&mut self, expr: &InstrBlockPy) {
                self.found |= matches!(expr, InstrBlockPy::IncrementCounter(_));
                expr.visit_children(self);
            }
        }

        let source = "def f(x):\n    if x:\n        return 1\n    return 0\n";
        let config = SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Profile))
            .with_profiled_cold_blocks_enabled(true);
        let lowered = prepare_for_test(source, &config).expect("transform should succeed");

        assert!(
            lowered
                .counter_defs
                .iter()
                .any(|counter| counter.kind == "block_entry"),
            "lowering should still define block_entry counters for runtime storage"
        );
        assert!(
            lowered
                .counter_defs
                .iter()
                .any(|counter| counter.kind == "branch_outcomes"),
            "lowering should define locality counters from InstrTyped"
        );
        let mut probe = IncrementCounterProbe { found: false };
        for function in &lowered.callable_defs {
            probe.visit_fn(function);
        }
        assert!(
            !probe.found,
            "lowering should not insert explicit counter instructions into BlockPy IR"
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
            let location_counters = lowered
                .counter_defs
                .iter()
                .filter(|counter| counter.kind == RUNTIME_DECREF_LOCATION_COUNTER_KIND)
                .collect::<Vec<_>>();
            assert!(
                location_counters
                    .iter()
                    .any(|counter| !counter.branches.is_empty()),
                "verify lowering should define branch counters for refcount release locations"
            );
            assert!(
                location_counters
                    .iter()
                    .flat_map(|counter| counter.branches.iter())
                    .any(|branch| branch.name.starts_with("purpose=stack_exit_sweep;")),
                "verify lowering should define stack-slot DECREF attribution branches"
            );
            assert!(location_counters.iter().all(|counter| {
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
                        && counter.kind != "runtime_decref"
                        && counter.kind != RUNTIME_DECREF_LOCATION_COUNTER_KIND),
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

    fn metadata(
        module_name: &str,
        source: &str,
        build_identity: &str,
    ) -> CachedBlockPyModuleMetadata {
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
