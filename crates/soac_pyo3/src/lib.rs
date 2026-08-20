#![allow(unsafe_op_in_unsafe_fn)]

mod jit_runtime;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyModule;
use serde_json::json;
use soac_core::profile::CounterDumpFile;
use std::path::Path;

#[cfg(test)]
mod test;

pub(crate) fn lowering_error_to_pyerr(err: soac_lowering::LoweringError) -> PyErr {
    match err {
        soac_lowering::LoweringError::Parse(parse_error) => {
            pyo3::exceptions::PySyntaxError::new_err(parse_error.to_string())
        }
        soac_lowering::LoweringError::StrictAuthentication(message) => {
            Python::attach(|py| soac_jit::strict_runtime_unavailable(py, message))
        }
        soac_lowering::LoweringError::Other(err) => {
            pyo3::exceptions::PyRuntimeError::new_err(err.to_string())
        }
    }
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
                "function_id": row.function_id.map(|function_id| function_id.to_packed_runtime_u64()),
                "current_function_id": row.current_function_id.map(|function_id| function_id.to_packed_runtime_u64()),
                "instr_id": row.instr_id.map(|instr_id| instr_id.to_string()),
                "function_qualname": row.function_qualname,
                "block_label": row.block_label,
                "value": row.value,
                "branches": row.branch_values.iter().map(|branch| {
                    (branch.branch.to_string(), json!(branch.value))
                }).collect::<serde_json::Map<String, serde_json::Value>>(),
                "observed_value": row.observed_value,
                "max_overcount": row.max_overcount,
            }));
        }

        json_records.push(json!({
            "source_hash": format!("0x{:016x}", record.source_hash()),
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

#[pyfunction]
fn flush_counter_dump_outputs() -> PyResult<()> {
    soac_jit::CompileSession::process()
        .flush_counter_dump_outputs()
        .map_err(PyRuntimeError::new_err)
}

#[pymodule]
fn _soac_ext(py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    soac_config::init_logging().map_err(PyRuntimeError::new_err)?;
    soac_jit::install_sigill_diagnostics().map_err(PyRuntimeError::new_err)?;
    soac_jit::initialize_strict_runtime(py)?;
    unsafe extern "C" {
        fn PySoac_GetStrictMutationError() -> *mut pyo3::ffi::PyObject;
        fn PySoac_GetStrictRuntimeUnavailableError() -> *mut pyo3::ffi::PyObject;
    }
    for (name, exception) in [
        ("StrictMutationError", unsafe {
            PySoac_GetStrictMutationError()
        }),
        ("StrictRuntimeUnavailableError", unsafe {
            PySoac_GetStrictRuntimeUnavailableError()
        }),
    ] {
        if exception.is_null() {
            return Err(PyErr::fetch(py));
        }
        // Export the native per-interpreter classes themselves. A Python-side
        // substitute must not split the exception identity used by barriers.
        module.add(name, unsafe {
            Bound::<PyAny>::from_borrowed_ptr(py, exception)
        })?;
    }
    PyModule::import(py, "soac.bootstrap")?;
    module.add(
        "IndexedModuleType",
        soac_jit::module_type::indexed_module_type_for_python(py)?,
    )?;
    module.add_function(wrap_pyfunction!(inspect_counter_dump_json, module)?)?;
    jit_runtime::add_module_functions(module)?;
    let flush_callback = wrap_pyfunction!(flush_counter_dump_outputs, module)?;
    PyModule::import(py, "atexit")?.call_method1("register", (flush_callback,))?;
    Ok(())
}
