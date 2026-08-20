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
use ruff_text_size::{Ranged, TextRange};
use soac_contracts::{Fingerprint, SourceDialect, VerifiedModuleTypeFacts};
use soac_core::block_py::{BlockPyModule, ModuleNameGen, PrettyPrint, PrettyPrinter};
use soac_core::pass_tracker::{PassTracker, RecordingPassTracker};
use soac_ir_blockpy::BlockPyModuleShape;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum LoweringError {
    Parse(ParseError),
    StrictAuthentication(String),
    Other(AnyhowError),
}

pub type Result<T> = std::result::Result<T, LoweringError>;

impl std::fmt::Display for LoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => err.fmt(f),
            Self::StrictAuthentication(message) => f.write_str(message),
            Self::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for LoweringError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(err) => Some(err),
            Self::StrictAuthentication(_) => None,
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
    /// Authenticated offline proposal for this exact source. This does not
    /// grant any runtime layout, type-check, or call-target capability.
    pub strict_facts: Option<Arc<VerifiedModuleTypeFacts>>,
    /// Canonical future-annotation strings from the same native source parse
    /// that the runtime subsequently matches against this lowered module.
    /// These values carry no source-authentication or optimization authority.
    pub canonical_annotations: Option<Arc<crate::CanonicalAnnotationStrings>>,
    /// Value-only class cell and closure recipes from the same original native
    /// tree. Source binding validates the optional input, not execution authority.
    pub canonical_class_bindings: Option<Arc<crate::CanonicalClassBindings>>,
}

/// Syntax accepted by the pinned parser whose execution protocol has not yet
/// been implemented in SOAC. Keep this check on the original AST, before any
/// rewrite can erase the distinction from an eager import or comprehension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnsupportedSyntax {
    LazyImport(TextRange),
    UnpackingComprehension(TextRange),
}

impl std::fmt::Display for UnsupportedSyntax {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (feature, range) = match self {
            Self::LazyImport(range) => ("lazy imports", range),
            Self::UnpackingComprehension(range) => ("unpacking comprehensions", range),
        };
        write!(
            formatter,
            "SOAC does not yet support {feature} at source bytes {range:?}"
        )
    }
}

impl std::error::Error for UnsupportedSyntax {}

fn validate_supported_syntax(body: &mut Suite) -> std::result::Result<(), UnsupportedSyntax> {
    use crate::transformer::{walk_expr, walk_stmt, Transformer};

    #[derive(Default)]
    struct Preflight {
        error: Option<UnsupportedSyntax>,
    }

    impl Transformer for Preflight {
        fn visit_stmt(&mut self, stmt: &mut Stmt) {
            if self.error.is_some() {
                return;
            }
            match stmt {
                Stmt::Import(node) if node.is_lazy => {
                    self.error = Some(UnsupportedSyntax::LazyImport(node.range));
                }
                Stmt::ImportFrom(node) if node.is_lazy => {
                    self.error = Some(UnsupportedSyntax::LazyImport(node.range));
                }
                _ => walk_stmt(self, stmt),
            }
        }

        fn visit_expr(&mut self, expr: &mut ast::Expr) {
            if self.error.is_some() {
                return;
            }
            let unpacking = match expr {
                ast::Expr::DictComp(node) => node.key.is_none(),
                ast::Expr::ListComp(node) => matches!(&*node.elt, ast::Expr::Starred(_)),
                ast::Expr::SetComp(node) => matches!(&*node.elt, ast::Expr::Starred(_)),
                ast::Expr::Generator(node) => matches!(&*node.elt, ast::Expr::Starred(_)),
                _ => false,
            };
            if unpacking {
                self.error = Some(UnsupportedSyntax::UnpackingComprehension(expr.range()));
            } else {
                walk_expr(self, expr);
            }
        }
    }

    let mut preflight = Preflight::default();
    preflight.visit_body(body);
    match preflight.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
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
    // CPython records static attributes from the original compiler scopes and
    // unmangled attribute names, before later AST rewrites can change either.
    rewrite_class_def::record_class_static_attributes(context, &mut module);

    // Rewrite names like "__foo" in class bodies to "_<class_name>__foo"
    rewrite_class_def::private::rewrite_private_names(context, &mut module);
    context.record_original_function_origins(&mut module);

    // Replace annotated assignments ("x: int = 1") with regular assignments,
    // and either drop the annotations (in functions) or generate an
    // __annotate__ function (in modules and classes)
    rewrite_stmt::annotation::rewrite_ann_assign_to_dunder_annotate(context, &mut module);

    // Lower helper-scoped expressions that synthesize nested defs for Python
    // scoping semantics before the more direct BlockPy expr lowering boundary.
    rewrite_with_pass(context, None, Some(&ScopedHelperExprPass), &mut module);

    let mut semantic_state = SemanticAstState::from_ruff_with_lambda_bodies(
        &mut module,
        context.source_names(),
        context.take_lowered_lambda_bodies(),
    );

    /*

    Wrap the module body in a synthesized `_dp_module_init` function.  It is assigned the same scope table as the
    module body so everythign remains e.g globals instead of locals.  This (combined with the similar but much
    more complicated) class rewrite below, lets us only deal with functions throughout the rest of the pipeline.
     */
    wrap_module_init(&mut semantic_state, &mut module);
    if let Some(Stmt::FunctionDef(module_init)) = module.first_mut() {
        context.record_module_body_origin(module_init);
    }

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
    let (module, original_body, future_annotations, original_tokens) =
        pass_tracker.record_timing("parse", || -> Result<_> {
            let parsed = parse_module(source)?;
            soac_source::validate_source_literals(source, parsed.tokens())
                .map_err(AnyhowError::new)?;
            let original_tokens = options
                .strict_facts
                .as_ref()
                .map(|_| parsed.tokens().clone());
            let mut module = parsed.into_syntax();
            validate_supported_syntax(&mut module.body).map_err(AnyhowError::new)?;
            crate::passes::ast_to_ast::semantic::ensure_node_indices_for_suite(&mut module.body);
            let original_body = module.body.clone();
            if options
                .canonical_annotations
                .as_ref()
                .is_some_and(|annotations| !annotations.matches_source(source))
            {
                return Err(LoweringError::StrictAuthentication(
                    "native annotation strings do not match the actual source being lowered".into(),
                ));
            }
            if options
                .canonical_class_bindings
                .as_ref()
                .is_some_and(|bindings| !bindings.matches_source(source))
            {
                return Err(LoweringError::StrictAuthentication(
                    "native class bindings do not match the actual source being lowered".into(),
                ));
            }
            if options.strict_facts.is_some() && options.canonical_class_bindings.is_none() {
                struct ClassDefinitions(bool);
                impl crate::transformer::Transformer for ClassDefinitions {
                    fn visit_stmt(&mut self, statement: &mut ast::Stmt) {
                        if matches!(statement, ast::Stmt::ClassDef(_)) {
                            self.0 = true;
                        } else if !self.0 {
                            crate::transformer::walk_stmt(self, statement);
                        }
                    }
                }
                let mut classes = ClassDefinitions(false);
                crate::transformer::Transformer::visit_body(&mut classes, &mut module.body);
                if classes.0 {
                    return Err(LoweringError::StrictAuthentication(
                        "strict class lowering requires source-bound native class bindings".into(),
                    ));
                }
            }
            let features = rewrite_future_annotations::rewrite(
                &mut module.body,
                options.canonical_annotations.as_deref(),
            )?;
            match (&options.strict_facts, features.contains("strict")) {
                (None, true) => {
                    return Err(LoweringError::StrictAuthentication(
                        "strict source requires authenticated offline type facts".into(),
                    ))
                }
                (Some(_), false) => {
                    return Err(LoweringError::StrictAuthentication(
                        "strict artifact cannot authorize ordinary source".into(),
                    ))
                }
                (Some(verified), true) => {
                    let facts = verified.facts();
                    if facts.source_dialect != SourceDialect::SoacStrict
                        || facts.source_digest != Fingerprint::digest(source.as_bytes())
                        || facts.source_size as usize != source.len()
                        || facts.module.source_hash
                            != soac_contracts::legacy_source_hash(source.as_bytes())
                    {
                        return Err(LoweringError::StrictAuthentication(
                            "strict artifact does not match the actual source being lowered".into(),
                        ));
                    }
                }
                (None, false) => {}
            }
            Ok((
                module,
                original_body,
                features.contains("annotations"),
                original_tokens,
            ))
        })?;

    let mut context = Context::with_strict_facts(
        source,
        options.strict_facts.clone(),
        &original_body,
        future_annotations,
        original_tokens.as_ref(),
    )?;
    context.set_canonical_class_bindings(options.canonical_class_bindings.clone());

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
     runtime wrapper owns those values across suspension, and the single lowered function reloads
     them into its internal resume body without instantiating a second Python function object.

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
    // Source-backed cell operations must be represented before the infallible
    // mapper chooses physical storage. Compiler-tail operations may have no
    // original Name access receipt even though their class has a FREE slot.
    pass_tracker.record_timing("validate_native_class_cells", || {
        crate::passes::name_binding::validate_native_class_cell_operations(
            &core_blockpy_without_await_or_yield,
        )
        .map_err(LoweringError::StrictAuthentication)
    })?;

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
        crate::passes::strict_construction::resolve_strict_construction(&mut blockpy);
        soac_ir_blockpy::ensure_constructor_entry_functions(&mut blockpy);
        crate::block_py::cfg::relabel_dense_bb_module(&mut blockpy);
        let blockpy = soac_ir_blockpy::assign_blockpy_module_instr_ids(blockpy);
        blockpy
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

    *module = [Stmt::FunctionDef(module_init)].into();
}
