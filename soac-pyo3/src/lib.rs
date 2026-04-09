#![allow(unsafe_op_in_unsafe_fn)]

mod jit_runtime;

use pyo3::prelude::*;
use pyo3::types::PyModule;
use soac_blockpy::{lower_python_to_blockpy_for_testing, ruff_ast_to_string};
use tracing::trace;

#[cfg(test)]
mod test;

pub(crate) fn lowering_error_to_pyerr(err: soac_blockpy::LoweringError) -> PyErr {
    match err {
        soac_blockpy::LoweringError::Parse(parse_error) => {
            pyo3::exceptions::PySyntaxError::new_err(parse_error.to_string())
        }
        soac_blockpy::LoweringError::Other(err) => {
            pyo3::exceptions::PyRuntimeError::new_err(err.to_string())
        }
    }
}

fn lower_source(source: &str) -> PyResult<soac_blockpy::LoweringResult> {
    lower_python_to_blockpy_for_testing(source).map_err(lowering_error_to_pyerr)
}

fn rendered_ast_to_ast_source(source: &str, output: &soac_blockpy::LoweringResult) -> String {
    output
        .pass_tracker
        .pass_ast_to_ast()
        .map(|module| ruff_ast_to_string(&module.body))
        .unwrap_or_else(|| source.to_string())
}

#[pyfunction]
fn transform_source_with_name(source: &str, module_name: &str) -> PyResult<String> {
    let preview = source.get(..100).unwrap_or(source);
    trace!("transform_source_with_name({module_name}): {}", preview);
    let output = lower_source(source)?;
    Ok(rendered_ast_to_ast_source(source, &output))
}

#[pymodule]
fn _soac_ext(py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    soac_blockpy::init_logging();
    module.add(
        "IndexedModuleType",
        soac_jit::module_type::indexed_module_type_for_python(py)?,
    )?;
    module.add_function(wrap_pyfunction!(transform_source_with_name, module)?)?;
    jit_runtime::add_module_functions(module)?;
    Ok(())
}
