#![recursion_limit = "256"]
#![deny(unreachable_pub)]

pub use crate::driver::{lower_source_to_codegen_module_with_tracker, LoweringOptions};
use crate::passes::CodegenModuleShape;
use anyhow::Error as AnyhowError;
pub use ruff_python_parser::ParseError;
use soac_core::block_py::{BlockPyModule, ModuleNameGen};
use soac_core::pass_tracker::{PassTracker, RecordingPassTracker};
use std::time::{Duration, Instant};

pub(crate) mod block_py;
mod driver;
#[cfg(test)]
pub(crate) mod fixture;
mod namegen;
#[cfg(test)]
mod pass_tracker;
pub mod passes;
pub(crate) mod ruff_ast;
mod template;
#[cfg(test)]
mod test_util;
mod transformer;

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

    let codegen_module = lower_source_to_codegen_module_with_tracker(
        source,
        module_name_gen,
        &mut pass_tracker,
        options,
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

#[cfg(test)]
mod test;
