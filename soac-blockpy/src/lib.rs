#![recursion_limit = "256"]

use crate::block_py::{BlockPyModule, ModuleNameGen};
pub use crate::driver::LoweringOptions;
use crate::driver::{rewrite_module_with_tracker, rewrite_module_with_tracker_with_options};
use crate::pass_tracker::{NoopPassTracker, PassTracker, RecordingPassTracker};
use crate::passes::CodegenModuleShape;
use anyhow::Error as AnyhowError;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_python_codegen::{Generator, Indentation};
pub use ruff_python_parser::ParseError;
use ruff_source_file::LineEnding;
use ruff_text_size::TextRange;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub mod block_py;
pub mod codegen_cache;
mod driver;
pub mod env_config;
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

fn open_soac_log_file(path: &Path) -> std::io::Result<Arc<std::fs::File>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(Arc::new)
}

pub fn init_logging() -> std::result::Result<(), String> {
    let config = env_config::SoacEnvConfig::from_env()?;
    init_logging_with_config(&config)
}

pub fn init_logging_with_config(
    config: &env_config::SoacEnvConfig,
) -> std::result::Result<(), String> {
    let env_config::SoacLogConfig { filter, json_path } = config.soac_log().clone();
    let filter = EnvFilter::builder()
        .parse(filter)
        .map_err(|err| format!("failed to parse SOAC_LOG filter: {err}"))?;
    let registry = tracing_subscriber::registry().with(filter);
    if let Some(json_path) = json_path {
        match open_soac_log_file(&json_path) {
            Ok(json_file) => {
                let json_layer = fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_writer(move || {
                        json_file
                            .try_clone()
                            .expect("failed to clone SOAC_LOG json file")
                    });
                let _ = registry.with(json_layer).try_init();
            }
            Err(err) => {
                eprintln!(
                    "[soac logging] failed to open SOAC_LOG json file {}: {err}",
                    json_path.display()
                );
                return Err(format!(
                    "failed to open SOAC_LOG json file {}: {err}",
                    json_path.display()
                ));
            }
        }
    } else {
        let fmt_layer = fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(!cfg!(test));
        let _ = registry.with(fmt_layer).try_init();
    }
    Ok(())
}

pub struct LoweringResult<P = RecordingPassTracker> {
    pub total_time: Duration,
    pub codegen_module: BlockPyModule<CodegenModuleShape>,
    pub pass_tracker: P,
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
    mut pass_tracker: P,
    options: LoweringOptions,
) -> Result<LoweringResult<P>>
where
    P: PassTracker,
{
    let env_config = env_config::SoacEnvConfig::from_env().map_err(anyhow::Error::msg)?;
    init_logging_with_config(&env_config).map_err(anyhow::Error::msg)?;
    namegen::reset_namegen_state();
    let total_start = Instant::now();

    let codegen_module = if options == LoweringOptions::default() {
        rewrite_module_with_tracker(source, module_name_gen, &mut pass_tracker, &env_config)?
    } else {
        rewrite_module_with_tracker_with_options(
            source,
            module_name_gen,
            &mut pass_tracker,
            options,
            &env_config,
        )?
    };

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
