use crate::block_py::pretty::BlockPyPrettyPrint;
use crate::block_py::{BlockPyModule, CounterScope, ModuleNameGen};
use crate::codegen_cache::{load_codegen_module_cache, store_codegen_module_cache};
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
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Clone)]
pub(crate) struct AstToAstPassResult {
    pub(crate) module: Suite,
    semantic_state: SemanticAstState,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoweringOptions {
    pub runtime_names_as_globals: bool,
    pub pre_optimization_cache_path: Option<PathBuf>,
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

    string_templates::rewrite_surrogate_escape_string_literals(context, &mut module);

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
) -> Result<BlockPyModule<CodegenModuleShape>> {
    let bb_codegen =
        rewrite_pre_optimization_module_with_cache(source, module_name_gen, pass_tracker, options)?;
    finish_codegen_module_with_tracker(bb_codegen, pass_tracker)
}

fn rewrite_pre_optimization_module_with_cache(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
    options: LoweringOptions,
) -> Result<BlockPyModule<CodegenModuleShape>> {
    if let Some(cache_path) = &options.pre_optimization_cache_path {
        let cache_exists =
            pass_tracker.record_timing("bb_codegen_cache_lookup", || cache_path.is_file());
        if cache_exists {
            let loaded = pass_tracker.record_timing("bb_codegen_cache_load", || {
                load_codegen_module_cache(cache_path)
            });
            match loaded {
                Ok(mut module) => {
                    crate::codegen_cache::remap_codegen_module_function_ids(
                        &mut module,
                        module_name_gen,
                    );
                    info!(
                        target: "soac_blockpy_module_cache",
                        event = "soac.blockpy_module_cache",
                        cache_hit = true,
                        path = %cache_path.display(),
                        "blockpy_module_cache_hit",
                    );
                    return Ok(pass_tracker.run_pass("bb_codegen", || module));
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
        store_pre_optimization_cache(cache_path, &module, pass_tracker);
        Ok(module)
    } else {
        rewrite_pre_optimization_module_from_source(
            source,
            module_name_gen,
            pass_tracker,
            options.runtime_names_as_globals,
        )
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

       "def" is replaced by a call to
       `__soac__.make_function(function_id, kind, closure, param_defaults, annotate_fn)`.

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
    let name_binding: BlockPyModule<ResolvedStorageModuleShape> =
        pass_tracker.run_pass("name_binding", || {
            if runtime_names_as_globals {
                passes::lower_name_binding_in_core_blockpy_module_with_options(
                    core_blockpy_without_await_or_yield,
                    true,
                )
            } else {
                passes::lower_name_binding_in_core_blockpy_module(
                    core_blockpy_without_await_or_yield,
                )
            }
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
    module: &BlockPyModule<CodegenModuleShape>,
    pass_tracker: &mut impl PassTracker,
) {
    let stored = pass_tracker.record_timing("bb_codegen_cache_store", || {
        store_codegen_module_cache(cache_path, module)
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
    bb_codegen: BlockPyModule<CodegenModuleShape>,
    pass_tracker: &mut impl PassTracker,
) -> Result<BlockPyModule<CodegenModuleShape>> {
    pass_tracker.record_timing("validate_codegen_instr_ids", || {
        passes::validate_codegen_instr_ids(&bb_codegen).map_err(anyhow::Error::msg)
    })?;
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

    let bb_traced: BlockPyModule<CodegenModuleShape> =
        if let Some(config) = passes::parse_trace_env() {
            pass_tracker.run_pass("bb_trace", || {
                let mut traced = bb_codegen;
                passes::instrument_bb_module_for_trace(&mut traced, &config);
                traced
            })
        } else {
            bb_codegen
        };

    let bb_call_target_counted: BlockPyModule<CodegenModuleShape> =
        if passes::call_target_counter_instrumentation_enabled() {
            pass_tracker.run_pass("bb_call_target_counters", || {
                let mut counted = bb_traced;
                passes::instrument_bb_module_with_call_target_counters(&mut counted);
                counted
            })
        } else {
            bb_traced
        };

    let bb_locality_counted: BlockPyModule<CodegenModuleShape> =
        if passes::locality_counter_instrumentation_enabled() {
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
        if passes::refcount_counter_instrumentation_enabled() {
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
        crate::block_py::validate_module(&bb_refcount_counted).map_err(anyhow::Error::msg)
    })?;

    Ok(bb_refcount_counted)
}

pub(crate) fn rewrite_module_with_tracker(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
) -> Result<BlockPyModule<CodegenModuleShape>> {
    rewrite_module_with_tracker_with_options(
        source,
        module_name_gen,
        pass_tracker,
        LoweringOptions::default(),
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
