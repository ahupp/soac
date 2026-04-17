#![allow(unsafe_op_in_unsafe_fn)]

mod jit_runtime;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyModule;
use serde_json::json;
use soac_blockpy::codegen_cache::{load_codegen_module_cache, module_optimization_plan_path};
use soac_blockpy::{lower_python_to_blockpy_for_testing, ruff_ast_to_string};
use soac_jit::counter_dump::CounterDumpFile;
use soac_jit::optimization_plan::{OptimizationPlan, ProfileEvidenceStore};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
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
                "function_id": row.function_id.map(|function_id| function_id.to_packed_runtime_u64()),
                "current_function_id": row.current_function_id.map(|function_id| function_id.to_packed_runtime_u64()),
                "instr_id": row.instr_id.map(|instr_id| instr_id.to_string()),
                "function_qualname": row.function_qualname,
                "block_label": row.block_label,
                "value": row.value,
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
fn decide_optimizations_for_counter_dump(
    counters_path: &str,
    module_root: &str,
    out_root: Option<&str>,
) -> PyResult<usize> {
    let counters_path = Path::new(counters_path);
    let module_root = Path::new(module_root);
    let out_root = out_root
        .map(PathBuf::from)
        .unwrap_or_else(|| module_root.to_path_buf());
    let evidence_store = ProfileEvidenceStore::from_counter_dump(counters_path)
        .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
    let module_paths =
        cached_module_paths_under_root(module_root).map_err(PyRuntimeError::new_err)?;
    let mut written = 0usize;
    for module_path in module_paths {
        let cache = load_codegen_module_cache(module_path.as_path())
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
        let Some(source_hash) =
            evidence_store.module_source_hash(cache.metadata.module_name.as_str())
        else {
            continue;
        };
        if source_hash != cache.metadata.source_hash {
            return Err(PyRuntimeError::new_err(format!(
                "counter dump source hash for module {} is 0x{source_hash:016x}, but cached BlockPy module has 0x{:016x}",
                cache.metadata.module_name, cache.metadata.source_hash
            )));
        }
        let plan = OptimizationPlan::from_evidence(&cache.metadata, &cache.module, &evidence_store);
        let output_path = module_optimization_plan_path(
            out_root.as_path(),
            cache.metadata.source,
            cache.metadata.module_name.as_str(),
        )
        .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
        write_optimization_plan(output_path.as_path(), &plan).map_err(PyRuntimeError::new_err)?;
        written += 1;
    }
    Ok(written)
}

fn cached_module_paths_under_root(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    collect_cached_module_paths(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_cached_module_paths(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|err| format!("read module cache path metadata {}: {err}", path.display()))?;
    if metadata.is_file() {
        if path.file_name().and_then(|name| name.to_str()) == Some("mod.blockpy") {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path)
        .map_err(|err| format!("read module cache directory {}: {err}", path.display()))?
    {
        let entry = entry.map_err(|err| format!("read entry in {}: {err}", path.display()))?;
        collect_cached_module_paths(entry.path().as_path(), out)?;
    }
    Ok(())
}

fn write_optimization_plan(path: &Path, plan: &OptimizationPlan) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create optimization plan dir {}: {err}", parent.display()))?;
    }
    let archive = rkyv::to_bytes::<rkyv::rancor::Error>(plan)
        .map_err(|err| format!("serialize optimization plan: {err}"))?;
    let temp_path = path.with_extension("opt.tmp");
    {
        let mut temp_file = File::create(temp_path.as_path()).map_err(|err| {
            format!(
                "create temporary optimization plan {}: {err}",
                temp_path.display()
            )
        })?;
        temp_file
            .write_all(archive.as_ref())
            .map_err(|err| format!("write optimization plan {}: {err}", temp_path.display()))?;
    }
    fs::rename(temp_path.as_path(), path).map_err(|err| {
        format!(
            "publish optimization plan {} -> {}: {err}",
            temp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

#[pymodule]
fn _soac_ext(py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    soac_blockpy::init_logging().map_err(PyRuntimeError::new_err)?;
    soac_jit::install_sigill_diagnostics().map_err(PyRuntimeError::new_err)?;
    PyModule::import(py, "soac.bootstrap")?;
    module.add(
        "IndexedModuleType",
        soac_jit::module_type::indexed_module_type_for_python(py)?,
    )?;
    module.add_function(wrap_pyfunction!(transform_source_with_name, module)?)?;
    module.add_function(wrap_pyfunction!(inspect_counter_dump_json, module)?)?;
    module.add_function(wrap_pyfunction!(
        decide_optimizations_for_counter_dump,
        module
    )?)?;
    jit_runtime::add_module_functions(module)?;
    Ok(())
}
