#![allow(unsafe_op_in_unsafe_fn)]

mod jit_runtime;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyModule;
use serde_json::json;
use soac_blockpy::{lower_python_to_blockpy_for_testing, ruff_ast_to_string};
use soac_jit::counter_dump::CounterDumpFile;
use std::path::Path;
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

#[pyfunction]
fn inspect_counter_dump_json(path: &str) -> PyResult<String> {
    let dump = CounterDumpFile::open(Path::new(path)).map_err(PyRuntimeError::new_err)?;
    let records = dump.records().map_err(PyRuntimeError::new_err)?;
    let mut json_records = Vec::new();
    for record in records.iter() {
        let mut module_keys = Vec::new();
        for key_index in 0..record.module_key_count() {
            let key = record
                .module_key(key_index)
                .map_err(PyRuntimeError::new_err)?;
            module_keys.push(json!({
                "owner": key.owner,
                "key": key.key,
                "index": key.index,
            }));
        }

        let mut type_keys = Vec::new();
        for key_index in 0..record.type_key_count() {
            let key = record
                .type_key(key_index)
                .map_err(PyRuntimeError::new_err)?;
            type_keys.push(json!({
                "owner_type_id": key.owner_type_id,
                "key": key.key,
                "index": key.index,
            }));
        }

        let mut type_table = Vec::new();
        for entry_index in 0..record.type_table_count() {
            let entry = record
                .type_table_entry(entry_index)
                .map_err(PyRuntimeError::new_err)?;
            type_table.push(json!({
                "type_id": entry.type_id,
                "module_name": entry.module_name,
                "qualname": entry.qualname,
            }));
        }

        let mut rows = Vec::new();
        for row_index in 0..record.row_count() {
            let row = record.row(row_index).map_err(PyRuntimeError::new_err)?;
            rows.push(json!({
                "counter_id": row.counter_id,
                "scope": row.scope,
                "kind": row.kind,
                "site_kind": row.site_kind,
                "function_id": row.function_id.map(|function_id| function_id.packed()),
                "current_function_id": row.current_function_id.map(|function_id| function_id.packed()),
                "instr_id": row.instr_id.map(|instr_id| instr_id.to_string()),
                "function_qualname": row.function_qualname,
                "block_label": row.block_label,
                "value": row.value,
                "observed_value": row.observed_value,
                "max_overcount": row.max_overcount,
            }));
        }

        json_records.push(json!({
            "module_name": record.module_name().map_err(PyRuntimeError::new_err)?,
            "package_name": record.package_name().map_err(PyRuntimeError::new_err)?,
            "module_keys": module_keys,
            "type_keys": type_keys,
            "type_table": type_table,
            "rows": rows,
        }));
    }
    serde_json::to_string(&json!({ "records": json_records })).map_err(|err| {
        PyRuntimeError::new_err(format!("failed to encode counter dump JSON: {err}"))
    })
}

#[pymodule]
fn _soac_ext(py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    soac_blockpy::init_logging();
    module.add(
        "IndexedModuleType",
        soac_jit::module_type::indexed_module_type_for_python(py)?,
    )?;
    module.add_function(wrap_pyfunction!(transform_source_with_name, module)?)?;
    module.add_function(wrap_pyfunction!(inspect_counter_dump_json, module)?)?;
    jit_runtime::add_module_functions(module)?;
    Ok(())
}
