use crate::block_py::pretty::BlockPyPrettyPrint;
use crate::block_py::{BlockPyModule, CounterScope, ModuleNameGen};
use crate::codegen_cache::{
    load_codegen_module_cache, remap_cached_codegen_module_function_ids,
    store_codegen_module_cache, validate_codegen_module_cache_metadata, CachedCodegenModule,
    CachedCodegenModuleMetadata, CachedPreparedCodegen,
};
use crate::pass_tracker::PassTracker;
use crate::passes::ast_to_ast::ast_rewrite::rewrite_with_pass;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::rewrite_class_def;
use crate::passes::ast_to_ast::rewrite_expr::ScopedHelperExprPass;
use crate::passes::ast_to_ast::{
    body::Suite, rewrite_future_annotations, rewrite_stmt, semantic::SemanticAstState,
    string_templates,
};
use crate::passes::core_await_lower::lower_awaits_in_core_blockpy_module;
use crate::passes::ruff_to_blockpy::rewrite_ast_to_core_blockpy_module_with_module;
use crate::passes::{
    self, CodegenModuleShape, CodegenUnidentifiedModuleShape, CoreModuleShape,
    CoreModuleShapeWithAwaitAndYield, CoreModuleShapeWithYield, ResolvedStorageModuleShape,
};
use crate::{ParseError, Result};
use ruff_python_ast::{self as ast, Stmt};
use ruff_python_parser::parse_module;
use soac_config::SoacEnvConfig;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Clone)]
pub(crate) struct AstToAstPassResult {
    pub(crate) module: Suite,
    semantic_state: SemanticAstState,
}

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

struct PreOptimizationModule {
    module: BlockPyModule<CodegenModuleShape>,
    prepared: Option<CachedPreparedCodegen>,
    cache_path_for_store: Option<PathBuf>,
    cache_metadata_for_store: Option<CachedCodegenModuleMetadata>,
}

impl BlockPyPrettyPrint for AstToAstPassResult {
    fn pretty_print(&self) -> String {
        crate::ruff_ast_to_string(&self.module)
    }
}

fn rewrite_ast_to_ast_module(context: &Context, mut module: Suite) -> AstToAstPassResult {
    // Rewrite names like "__foo" in class bodies to "_<class_name>__foo"
    rewrite_class_def::private::rewrite_private_names(context, &mut module);

    // Replace annotated assignments ("x: int = 1") with regular assignments,
    // and either drop the annotations (in functions) or generate an
    // __annotate__ function (in modules and classes)
    rewrite_stmt::annotation::rewrite_ann_assign_to_dunder_annotate(context, &mut module);

    // Lower helper-scoped expressions that synthesize nested defs for Python
    // scoping semantics before the more direct BlockPy expr lowering boundary.
    rewrite_with_pass(context, None, Some(&ScopedHelperExprPass), &mut module);

    let mut semantic_state = SemanticAstState::from_ruff(&mut module);

    /*

    Wrap the module body in a synthesized `_dp_module_init` function.  It is assigned the same scope table as the
    module body so everythign remains e.g globals instead of locals.  This (combined with the similar but much
    more complicated) class rewrite below, lets us only deal with functions throughout the rest of the pipeline.
     */
    wrap_module_init(&mut semantic_state, &mut module);

    rewrite_class_def::class_body::rewrite_class_body_scopes(
        context,
        &mut semantic_state,
        &mut module,
    );

    AstToAstPassResult {
        module,
        semantic_state,
    }
}

pub(crate) fn rewrite_module_with_tracker_with_options(
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

        let module = rewrite_pre_optimization_module_from_source(
            source,
            module_name_gen,
            pass_tracker,
            options.runtime_names_as_globals,
        )?;
        Ok(PreOptimizationModule {
            module,
            prepared: None,
            cache_path_for_store: Some(cache_path.clone()),
            cache_metadata_for_store: options.pre_optimization_cache_metadata.clone(),
        })
    } else {
        let module = rewrite_pre_optimization_module_from_source(
            source,
            module_name_gen,
            pass_tracker,
            options.runtime_names_as_globals,
        )?;
        Ok(PreOptimizationModule {
            module,
            prepared: None,
            cache_path_for_store: None,
            cache_metadata_for_store: None,
        })
    }
}

fn rewrite_pre_optimization_module_from_source(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
    runtime_names_as_globals: bool,
) -> Result<BlockPyModule<CodegenModuleShape>> {
    let module =
        pass_tracker.record_timing("parse", || -> std::result::Result<_, ParseError> {
            let mut module = parse_module(source).map(|module| module.into_syntax())?;
            rewrite_future_annotations::rewrite(&mut module.body)?;
            let context = Context::new(source);
            string_templates::reject_lone_surrogate_string_literals(&context, &mut module.body)?;
            Ok(module)
        })?;

    let context = Context::new(source);

    let AstToAstPassResult {
        module,
        semantic_state,
    } = pass_tracker.run_pass("ast-to-ast", || {
        rewrite_ast_to_ast_module(&context, module.body)
    });

    /*

       Convert all flow control into a block-and-jump structure.  For example,

       ```
       x = 0
       while (y := x + 1) < 5:
           print(x)
           x += 1
       ```

       would turn into something like:

       ```
       block start:
           y = x + 1
           if y < 5:
               jump body
           else:
               jump end
       block body:
           print(x)
           x += 1
           jump start
       block end:
           return None
       ```

       This removes while/with/for from the AST, as well as expressions that
       interact with the block structure like walrus, and those that short circuit like bool ops.

       "def" is replaced by a `MakeFunction` operation, and name binding
       resolves that into `MakeFunctionWithClosure` with explicit closure
       capture construction.

       try/except are replaced by an exception handling block, and each block in the `try` has exc_edge
       set to that handler.  except block has it's own exc_edge to ensure exceptions in except
       still jump to finally.
    */

    let core_blockpy: BlockPyModule<CoreModuleShapeWithAwaitAndYield> =
        pass_tracker.run_pass("core_blockpy_with_await_and_yield", || {
            rewrite_ast_to_core_blockpy_module_with_module(
                &context,
                module,
                &semantic_state,
                module_name_gen,
            )
        });

    /*
      A very simple pass to rewrite `await foo` into `yield from __soac__.await_iter(foo)`
    */
    let core_blockpy_without_await: BlockPyModule<CoreModuleShapeWithYield> = pass_tracker
        .run_pass("core_blockpy_with_yield", || {
            lower_awaits_in_core_blockpy_module(core_blockpy)
        });

    /*
     Convert generators into a state machine, driven by an internal `resume(send, throw)` function.

     `resume` carries state in closure cells, with blocks split at yield/resume points.

    */
    let core_blockpy_without_await_or_yield: BlockPyModule<CoreModuleShape> = pass_tracker
        .run_pass("core_blockpy", || {
            passes::lower_yield_in_lowered_core_blockpy_module_bundle(core_blockpy_without_await)
        });

    /*
     Resolve Names into specific storage operations:
       - globals become LoadName / StoreName / DelName
       - cellvars (locals that are captured by inner functions) become MakeCell / LoadLocation / StoreLocation / DelLocation
         against a cell stored in local variables
       - freevars (captures from outer scopes) become LoadLocation / StoreLocation / DelLocation against a slot in the closure tuple
       - locals are assigned stack slots, and become LoadLocation / StoreLocation / DelLocation with local-slot locations.

    */
    // `soac.runtime` is compiled by the import hook too. While compiling that
    // module, runtime-name constants cannot be materialized by importing
    // `soac.runtime`, so the bootstrap mode leaves those loads as globals.
    let name_binding: BlockPyModule<ResolvedStorageModuleShape> =
        pass_tracker.run_pass("name_binding", || {
            passes::lower_name_binding_in_core_blockpy_module_with_options(
                core_blockpy_without_await_or_yield,
                runtime_names_as_globals,
            )
        });

    let global_index: BlockPyModule<ResolvedStorageModuleShape> = pass_tracker
        .run_pass("global_index", || {
            passes::lower_global_index_in_resolved_module_default(name_binding.clone())
        });

    let bb_prepared: BlockPyModule<ResolvedStorageModuleShape> = pass_tracker
        .run_pass("bb_prepared", || {
            passes::lower_try_jump_exception_flow(&global_index)
        });
    let bb_codegen: BlockPyModule<CodegenModuleShape> = pass_tracker.run_pass("bb_codegen", || {
        let mut bb_codegen: BlockPyModule<CodegenUnidentifiedModuleShape> =
            passes::normalize_bb_module_strings(&bb_prepared);
        passes::relabel_dense_bb_module(&mut bb_codegen);
        passes::assign_module_instr_ids(bb_codegen)
    });

    Ok(bb_codegen)
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
        module: mut bb_codegen,
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
        let initial_inline_plan = pass_tracker.record_timing("inline_candidate_plan", || {
            let escape_summary = passes::summarize_module_escapes(&bb_codegen);
            passes::plan_module_inlining(&escape_summary)
        });
        let scalar_replacement_stats =
            pass_tracker.record_timing("scalar_replace_constructor_allocations", || {
                passes::scalar_replace_non_escaping_constructor_allocations(
                    &mut bb_codegen,
                    &initial_inline_plan,
                )
            });
        let inline_rewrite_stats = pass_tracker.record_timing("inline_direct_call_stores", || {
            passes::inline_simple_direct_call_stores(&mut bb_codegen, &initial_inline_plan)
        });
        if scalar_replacement_stats.replaced_allocations != 0
            || inline_rewrite_stats.rewritten_stores != 0
        {
            pass_tracker.record_timing("validate_codegen_instr_ids_after_inline", || {
                passes::validate_codegen_instr_ids(&bb_codegen).map_err(anyhow::Error::msg)
            })?;
        }
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
        crate::block_py::validate_module(&bb_deopt_entry_counted).map_err(anyhow::Error::msg)
    })?;

    Ok(bb_deopt_entry_counted)
}

pub(crate) fn rewrite_module_with_tracker(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
    env_config: &SoacEnvConfig,
) -> Result<BlockPyModule<CodegenModuleShape>> {
    rewrite_module_with_tracker_with_options(
        source,
        module_name_gen,
        pass_tracker,
        LoweringOptions::default(),
        env_config,
    )
}

pub(crate) fn wrap_module_init(semantic_state: &mut SemanticAstState, module: &mut Suite) {
    let mut init_body = std::mem::take(module);
    if init_body.is_empty() {
        init_body.push(crate::py_stmt!("pass"));
    }

    let module_init: ast::StmtFunctionDef = crate::py_stmt_typed!(
        r#"
def _dp_module_init():
    {init_body:stmt}
"#,
        init_body = init_body,
    );
    semantic_state.synthesize_module_init_scope(&module_init);

    *module = vec![Stmt::FunctionDef(module_init)];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::ModuleNameGen;
    use crate::codegen_cache::{
        load_codegen_module_cache, store_codegen_module_cache, CachedCodegenModuleMetadata,
        PythonModuleCacheSource,
    };
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
        let stale_module = crate::lower_python_to_blockpy_recorded(source, ModuleNameGen::new(1))
            .expect("initial lowering should succeed")
            .codegen_module;
        store_codegen_module_cache(cache_path.as_path(), &stale_metadata, &stale_module, None)
            .expect("stale cache should be writable");

        if let Err(err) = crate::lower_python_to_blockpy_recorded_with_options(
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

    fn metadata(module_name: &str, cache_identity: &str) -> CachedCodegenModuleMetadata {
        CachedCodegenModuleMetadata {
            source: PythonModuleCacheSource::Project,
            module_name: module_name.to_string(),
            source_hash: 0x1234,
            cache_identity: cache_identity.to_string(),
        }
    }

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac-blockpy-cache-test-{}-{nanos}",
            std::process::id()
        ))
    }
}
