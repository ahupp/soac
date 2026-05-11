use crate::passes::ast_to_ast::ast_rewrite::rewrite_with_pass;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::rewrite_class_def;
use crate::passes::ast_to_ast::rewrite_expr::ScopedHelperExprPass;
use crate::passes::ast_to_ast::{
    body::Suite, rewrite_future_annotations, rewrite_stmt, semantic::SemanticAstState,
};
use crate::passes::core_await_lower::lower_awaits_in_core_blockpy_module;
use crate::passes::ruff_to_blockpy::rewrite_ast_to_core_blockpy_module_with_module;
use crate::passes::{
    CoreModuleShape, CoreModuleShapeWithAwaitAndYield, CoreModuleShapeWithYield,
    ResolvedStorageModuleShape,
};
use anyhow::Error as AnyhowError;
use ruff_python_ast::{self as ast, Stmt};
use ruff_python_parser::{parse_module, ParseError};
use soac_core::block_py::{BlockPyModule, ModuleNameGen, PrettyPrint, PrettyPrinter};
use soac_core::pass_tracker::{PassTracker, RecordingPassTracker};
use soac_ir_blockpy::BlockPyModuleShape;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum LoweringError {
    Parse(ParseError),
    Other(AnyhowError),
}

pub type Result<T> = std::result::Result<T, LoweringError>;

impl std::fmt::Display for LoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => err.fmt(f),
            Self::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for LoweringError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(err) => Some(err),
            Self::Other(err) => Some(err.as_ref()),
        }
    }
}

impl From<ParseError> for LoweringError {
    fn from(value: ParseError) -> Self {
        Self::Parse(value)
    }
}

impl From<AnyhowError> for LoweringError {
    fn from(value: AnyhowError) -> Self {
        Self::Other(value)
    }
}

pub struct LoweringResult<P = RecordingPassTracker> {
    pub total_time: Duration,
    pub blockpy_module: BlockPyModule<BlockPyModuleShape>,
    pub pass_tracker: P,
}

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

pub fn lower_python_to_blockpy_with_tracker_and_options<P>(
    source: &str,
    module_name_gen: ModuleNameGen,
    mut pass_tracker: P,
    options: LoweringOptions,
) -> Result<LoweringResult<P>>
where
    P: PassTracker,
{
    let total_start = Instant::now();

    let blockpy_module = lower_source_to_blockpy_module_with_tracker(
        source,
        module_name_gen,
        &mut pass_tracker,
        options,
    )?;

    Ok(LoweringResult {
        total_time: total_start.elapsed(),
        blockpy_module,
        pass_tracker,
    })
}

pub fn lower_python_to_blockpy_for_testing(source: &str) -> Result<LoweringResult> {
    lower_python_to_blockpy_with_tracker(source, ModuleNameGen::new(0), RecordingPassTracker::new())
}

impl PrettyPrint for AstToAstPassResult {
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> std::fmt::Result {
        std::fmt::Write::write_str(printer, &crate::ruff_ast::ruff_ast_to_string(&self.module))
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

pub fn lower_source_to_blockpy_module_with_tracker(
    source: &str,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
    options: LoweringOptions,
) -> Result<BlockPyModule<BlockPyModuleShape>> {
    crate::namegen::reset_namegen_state();
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
     Convert generators into a state machine backed by an internal resume body.

     Suspension state is modeled separately from lexical closure cells as preserved slots. The
     runtime wrapper owns those values across suspension, and a native resume handle reloads them
     into the internal resume body without instantiating a second Python function object.

    */
    let core_blockpy_without_await_or_yield: BlockPyModule<CoreModuleShape> = pass_tracker
        .run_pass("core_blockpy", || {
            crate::passes::blockpy_generators::lower_yield_in_lowered_core_blockpy_module_bundle(
                core_blockpy_without_await,
            )
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
            crate::passes::name_binding::lower_name_binding_in_core_blockpy_module_with_options(
                core_blockpy_without_await_or_yield,
                options.runtime_names_as_globals,
            )
        });

    let global_index: BlockPyModule<ResolvedStorageModuleShape> =
        pass_tracker.run_pass("global_index", || {
            crate::passes::global_index::lower_global_index_in_resolved_module_default(
                name_binding.clone(),
            )
        });

    let bb_prepared: BlockPyModule<ResolvedStorageModuleShape> =
        pass_tracker.run_pass("bb_prepared", || {
            crate::passes::blockpy_to_bb::exception_pass::lower_try_jump_exception_flow(
                &global_index,
            )
        });
    let blockpy: BlockPyModule<BlockPyModuleShape> = pass_tracker.run_pass("blockpy", || {
        let mut blockpy: BlockPyModule<BlockPyModuleShape> =
            crate::passes::blockpy_to_bb::strings::hoist_module_constants(&bb_prepared);
        soac_ir_blockpy::ensure_constructor_entry_functions(&mut blockpy);
        crate::block_py::cfg::relabel_dense_bb_module(&mut blockpy);
        soac_ir_blockpy::assign_blockpy_module_instr_ids(blockpy)
    });
    pass_tracker.record_timing("validate", || {
        crate::block_py::validate::validate_blockpy_module(&blockpy).map_err(anyhow::Error::msg)
    })?;

    Ok(blockpy)
}

pub(crate) fn wrap_module_init(semantic_state: &mut SemanticAstState, module: &mut Suite) {
    let mut init_body = std::mem::take(module);
    if init_body.is_empty() {
        init_body.push(crate::template::py_stmt!("pass"));
    }

    let module_init: ast::StmtFunctionDef = crate::template::py_stmt_typed!(
        r#"
def _dp_module_init():
    {init_body:stmt}
"#,
        init_body = init_body,
    );
    semantic_state.synthesize_module_init_scope(&module_init);

    *module = vec![Stmt::FunctionDef(module_init)];
}
