use crate::block_py::{BlockPyModule, ModuleNameGen};
use crate::driver::rewrite_module_with_tracker;
use crate::pass_tracker::{NoopPassTracker, PassTracker, RecordingPassTracker};
use crate::passes::CodegenModuleShape;
use anyhow::Error as AnyhowError;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_python_codegen::{Generator, Indentation};
pub use ruff_python_parser::ParseError;
use ruff_source_file::LineEnding;
use ruff_text_size::TextRange;
use std::sync::Once;
use std::time::{Duration, Instant};

pub mod block_py;
pub mod codegen_cache;
mod driver;
pub mod fixture;
mod namegen;
pub mod pass_tracker;
pub mod passes;
mod template;
#[cfg(test)]
mod test_util;
pub(crate) mod transformer;

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

static INIT_LOGGER: Once = Once::new();

pub fn init_logging() {
    INIT_LOGGER.call_once(|| {
        let mut builder =
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(""));
        if cfg!(test) {
            builder.is_test(true);
        }
        let _ = builder.try_init();
    });
}

pub struct LoweringResult<P = RecordingPassTracker> {
    pub total_time: Duration,
    pub codegen_module: BlockPyModule<CodegenModuleShape>,
    pub pass_tracker: P,
}

fn lower_python_to_blockpy_with_tracker<P>(
    source: &str,
    module_name_gen: ModuleNameGen,
    mut pass_tracker: P,
) -> Result<LoweringResult<P>>
where
    P: PassTracker,
{
    init_logging();
    namegen::reset_namegen_state();
    let total_start = Instant::now();

    let codegen_module = rewrite_module_with_tracker(source, module_name_gen, &mut pass_tracker)?;

    Ok(LoweringResult {
        total_time: total_start.elapsed(),
        codegen_module,
        pass_tracker,
    })
}

pub fn lower_python_to_blockpy_for_testing(source: &str) -> Result<LoweringResult> {
    lower_python_to_blockpy_with_tracker(source, ModuleNameGen::new(0), RecordingPassTracker::new())
}

pub fn lower_python_to_blockpy(
    source: &str,
    module_name_gen: ModuleNameGen,
) -> Result<LoweringResult<NoopPassTracker>> {
    lower_python_to_blockpy_with_tracker(source, module_name_gen, NoopPassTracker::new())
}

pub trait ToRuffAst {
    fn to_ruff_ast(&self) -> Vec<Stmt>;
}

impl ToRuffAst for Expr {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        vec![Stmt::Expr(ast::StmtExpr {
            value: Box::new(self.clone()),
            range: TextRange::default(),
            node_index: ast::AtomicNodeIndex::default(),
        })]
    }
}

impl ToRuffAst for Stmt {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        vec![self.clone()]
    }
}

impl ToRuffAst for &Stmt {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        vec![self.to_owned().clone()]
    }
}

impl ToRuffAst for &Vec<Stmt> {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        self.to_vec()
    }
}

impl ToRuffAst for &Expr {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        let expr = self.to_owned().clone();
        vec![Stmt::Expr(ast::StmtExpr {
            value: Box::new(expr),
            range: TextRange::default(),
            node_index: ast::AtomicNodeIndex::default(),
        })]
    }
}

impl ToRuffAst for &[Stmt] {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        self.to_vec()
    }
}

#[cfg(test)]
mod test;

/// Convert a ruff AST ModModule to a pretty-printed string.
pub fn ruff_ast_to_string(module: impl ToRuffAst) -> String {
    let module = module.to_ruff_ast();
    // Use default stylist settings for pretty printing
    let indent = Indentation::new("    ".to_string());
    let mut output = String::new();
    for stmt in module {
        let gen = Generator::new(&indent, LineEnding::default());
        output.push_str(&gen.stmt(&stmt));
        output.push_str(LineEnding::default().as_str());
    }
    output
}
