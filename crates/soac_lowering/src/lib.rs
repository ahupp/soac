#![recursion_limit = "256"]
#![deny(unreachable_pub)]

mod block_py;
mod driver;
#[cfg(test)]
mod fixture;
mod namegen;
#[cfg(test)]
mod pass_tracker;
pub mod passes;
mod ruff_ast;
mod template;
#[cfg(test)]
mod test_util;
mod transformer;

pub use crate::driver::{
    lower_python_to_blockpy_for_testing, lower_python_to_blockpy_with_tracker_and_options,
    lower_source_to_codegen_module_with_tracker, LoweringError, LoweringOptions, LoweringResult,
    Result,
};
pub use crate::passes::instr_id::{
    assign_missing_codegen_function_instr_ids, reassign_codegen_function_instr_ids,
    reassign_codegen_module_instr_ids, validate_codegen_instr_ids,
};

#[cfg(test)]
mod test;
