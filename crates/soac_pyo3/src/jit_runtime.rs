use crate::lowering_error_to_pyerr;
use pyo3::exceptions::{PyOSError, PyRuntimeError, PyTypeError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyModule, PyTuple};
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, FunctionExecutionMode, RuntimeFunctionId,
};
use soac_core::pass_tracker::RecordingPassTracker;
use soac_driver::codegen_cache::{PythonModuleCacheSource, hash_module_source};
use soac_driver::{CodegenPreparationOptions, PreOptimizationCacheRequest, prepare_codegen_module};
use soac_jit::module_type::{ModuleInfo, SoacExtModule};
use soac_lowering::passes::CodegenModuleShape;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::info;

const SOAC_BUILD_IDENTITY: &str = env!("SOAC_BUILD_IDENTITY");

thread_local! {
    static EXEC_MODULE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct ExecModuleDepthGuard {
    outermost: bool,
}

impl ExecModuleDepthGuard {
    fn enter() -> Self {
        EXEC_MODULE_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current + 1);
            Self {
                outermost: current == 0,
            }
        })
    }

    fn is_outermost(&self) -> bool {
        self.outermost
    }
}

impl Drop for ExecModuleDepthGuard {
    fn drop(&mut self) {
        EXEC_MODULE_DEPTH.with(|depth| {
            let current = depth.get();
            debug_assert!(current > 0);
            depth.set(current.saturating_sub(1));
        });
    }
}

fn import_dp_module<'py>(py: Python<'py>) -> PyResult<Bound<'py, PyModule>> {
    PyModule::import(py, "soac.runtime")
}

struct SoacRuntimeBootstrapGlobals {
    helpers: Vec<(&'static str, Py<PyAny>)>,
}

impl SoacRuntimeBootstrapGlobals {
    fn restore(&self, py: Python<'_>, globals: &Bound<'_, PyDict>) -> PyResult<()> {
        for (name, value) in &self.helpers {
            globals.set_item(*name, value.bind(py))?;
        }
        Ok(())
    }
}

fn install_soac_runtime_bootstrap_globals(
    py: Python<'_>,
    globals: &Bound<'_, PyDict>,
) -> PyResult<SoacRuntimeBootstrapGlobals> {
    let native_ext = PyModule::import(py, "_soac_ext")?;
    let normal_bootstrap = PyModule::import(py, "soac.bootstrap")?;
    let mut helpers = Vec::new();
    for (source_name, global_name) in [
        (
            "ANNOTATION_FORWARDREF_MISSING",
            "_ANNOTATION_FORWARDREF_MISSING",
        ),
        ("ELLIPSIS", "ELLIPSIS"),
        ("EMPTY_TUPLE", "EMPTY_TUPLE"),
        ("FALSE", "FALSE"),
        ("NO_DEFAULT", "NO_DEFAULT"),
        ("NONE", "NONE"),
        ("TRUE", "TRUE"),
        ("_entry_template", "_entry_template"),
        ("code_with_freevars", "code_with_freevars"),
    ] {
        let helper = normal_bootstrap.getattr(source_name)?;
        globals.set_item(global_name, &helper)?;
        helpers.push((global_name, helper.unbind()));
    }
    for name in ["import_", "import_attr"] {
        let helper = native_ext.getattr(name)?;
        globals.set_item(name, &helper)?;
        helpers.push((name, helper.unbind()));
    }
    Ok(SoacRuntimeBootstrapGlobals { helpers })
}

type OriginalCodeMap = HashMap<RuntimeFunctionId, Py<PyAny>>;
type OriginalCodeByQualname = HashMap<String, VecDeque<Py<PyAny>>>;

#[derive(Clone)]
struct TimedPhase {
    name: String,
    elapsed: Duration,
}

struct PendingModuleLoadTiming {
    module_name: String,
    package_name: String,
    path: String,
    source_hash: u64,
    source_bytes: usize,
    create_started_at: Instant,
    create_module_total: Duration,
    lowering_total: Duration,
    blockpy_pass_timings: Vec<TimedPhase>,
    create_timings: Vec<TimedPhase>,
    function_count: usize,
    counter_count: usize,
    global_name_count: usize,
    original_code_count: usize,
}

static PENDING_MODULE_LOAD_TIMINGS: OnceLock<Mutex<HashMap<usize, PendingModuleLoadTiming>>> =
    OnceLock::new();

fn time_phase<T>(
    timings: &mut Vec<TimedPhase>,
    name: impl Into<String>,
    build: impl FnOnce() -> T,
) -> T {
    let name = name.into();
    let start = Instant::now();
    let value = build();
    timings.push(TimedPhase {
        name,
        elapsed: start.elapsed(),
    });
    value
}

fn elapsed_us(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

fn source_hash_hex(source_hash: u64) -> String {
    format!("0x{source_hash:016x}")
}

fn soac_repo_root() -> Option<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

fn module_cache_source_for_import_path(path: &str) -> PythonModuleCacheSource {
    let import_path = Path::new(path);
    if soac_repo_root()
        .as_deref()
        .is_some_and(|repo_root| import_path.starts_with(repo_root))
    {
        PythonModuleCacheSource::Project
    } else {
        PythonModuleCacheSource::PythonStdlib
    }
}

fn pre_optimization_module_cache(
    session: &soac_jit::CompileSession,
    module_name: &str,
    source: PythonModuleCacheSource,
) -> PyResult<Option<PreOptimizationCacheRequest>> {
    let Some(cache_root) = session
        .env_config()
        .map_err(PyRuntimeError::new_err)?
        .module_cache_root()
    else {
        return Ok(None);
    };
    Ok(Some(PreOptimizationCacheRequest::new(
        cache_root.to_path_buf(),
        source,
        module_name,
        SOAC_BUILD_IDENTITY,
    )))
}

fn pending_module_load_timings() -> &'static Mutex<HashMap<usize, PendingModuleLoadTiming>> {
    PENDING_MODULE_LOAD_TIMINGS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pending_module_load_timing_key(module: *mut ffi::PyObject) -> usize {
    module as usize
}

fn store_pending_module_load_timing(module: *mut ffi::PyObject, timing: PendingModuleLoadTiming) {
    let mut pending = pending_module_load_timings()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pending.insert(pending_module_load_timing_key(module), timing);
}

fn take_pending_module_load_timing(module: *mut ffi::PyObject) -> Option<PendingModuleLoadTiming> {
    let mut pending = pending_module_load_timings()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pending.remove(&pending_module_load_timing_key(module))
}

fn trace_module_load_phase(module_name: &str, path: &str, prefix: &str, phase: &TimedPhase) {
    info!(
        target: "soac_module_load",
        event = "soac.module_load.phase",
        module_name,
        path,
        phase = format_args!("{prefix}.{}", phase.name),
        elapsed_us = elapsed_us(phase.elapsed),
        "module_load_phase",
    );
}

fn append_completed_module_load_log(
    module: *mut ffi::PyObject,
    exec_module_total: Duration,
    exec_timings: Vec<TimedPhase>,
    result: &PyResult<()>,
) {
    let pending = take_pending_module_load_timing(module);
    if let Some(pending) = &pending {
        for phase in &pending.create_timings {
            trace_module_load_phase(&pending.module_name, &pending.path, "create_module", phase);
        }
        for phase in &pending.blockpy_pass_timings {
            trace_module_load_phase(&pending.module_name, &pending.path, "blockpy", phase);
        }
        for phase in &exec_timings {
            trace_module_load_phase(&pending.module_name, &pending.path, "exec_module", phase);
        }
        let error = result.as_ref().err().map(ToString::to_string);
        info!(
            target: "soac_module_load",
            event = "soac.module_load",
            status = if result.is_ok() { "ok" } else { "error" },
            error = error.as_deref().unwrap_or(""),
            module_name = pending.module_name,
            package_name = pending.package_name,
            path = pending.path,
            source_hash = source_hash_hex(pending.source_hash),
            source_bytes = pending.source_bytes,
            function_count = pending.function_count,
            counter_count = pending.counter_count,
            global_name_count = pending.global_name_count,
            original_code_count = pending.original_code_count,
            module_load_total_us = elapsed_us(pending.create_started_at.elapsed()),
            create_module_total_us = elapsed_us(pending.create_module_total),
            blockpy_total_us = elapsed_us(pending.lowering_total),
            exec_module_total_us = elapsed_us(exec_module_total),
            "module_load",
        );
    } else {
        let error = result.as_ref().err().map(ToString::to_string);
        info!(
            target: "soac_module_load",
            event = "soac.module_load",
            status = if result.is_ok() { "ok" } else { "error" },
            error = error.as_deref().unwrap_or(""),
            exec_module_total_us = elapsed_us(exec_module_total),
            "module_load",
        );
    }
}

fn module_spec_string_attr(spec: &Bound<'_, PyAny>, attr: &str) -> PyResult<String> {
    spec.getattr(attr)?
        .extract::<String>()
        .map_err(|_| PyTypeError::new_err(format!("expected a module spec with a string {attr:?}")))
}

fn compile_original_module_code(py: Python<'_>, source: &str, path: &str) -> PyResult<Py<PyAny>> {
    let code = PyModule::import(py, "builtins")?
        .getattr("compile")?
        .call1((source, path, "exec"))?;
    Ok(code.unbind())
}

fn collect_original_code_objects(
    code: &Bound<'_, PyAny>,
    code_type: &Bound<'_, PyAny>,
    by_qualname: &mut OriginalCodeByQualname,
) -> PyResult<()> {
    let qualname = code.getattr("co_qualname")?.extract::<String>()?;
    by_qualname
        .entry(qualname)
        .or_default()
        .push_back(code.clone().unbind());

    let consts = code.getattr("co_consts")?;
    let const_count = unsafe { ffi::PyTuple_Size(consts.as_ptr()) };
    if const_count < 0 {
        return Err(PyErr::fetch(code.py()));
    }
    for index in 0..const_count {
        let item = unsafe { ffi::PyTuple_GetItem(consts.as_ptr(), index) };
        if item.is_null() {
            return Err(PyErr::fetch(code.py()));
        }
        let item = unsafe { Bound::from_borrowed_ptr(code.py(), item) };
        if item.is_instance(code_type)? {
            collect_original_code_objects(&item, code_type, by_qualname)?;
        }
    }
    Ok(())
}

fn is_synthetic_class_helper(function: &BlockPyFunction<CodegenModuleShape>) -> bool {
    function.names.bind_name.starts_with("_dp_class_ns_")
        || function.names.bind_name.starts_with("_dp_define_class_")
}

fn original_code_lookup_key(function: &BlockPyFunction<CodegenModuleShape>) -> Option<&str> {
    if function.execution_mode() == FunctionExecutionMode::Interpreted {
        return None;
    }
    let qualname = function.names.qualname.as_str();
    if qualname == "_dp_module_init"
        || function.names.fn_name == "_dp_resume"
        || is_synthetic_class_helper(function)
    {
        return None;
    }
    Some(qualname)
}

fn match_original_code_to_functions(
    py: Python<'_>,
    module_code: &Bound<'_, PyAny>,
    lowered_module: &BlockPyModule<CodegenModuleShape>,
) -> PyResult<OriginalCodeMap> {
    let code_type = PyModule::import(py, "types")?.getattr("CodeType")?;
    let mut code_by_qualname = HashMap::new();
    collect_original_code_objects(module_code, &code_type, &mut code_by_qualname)?;

    let mut code_by_function_id = HashMap::new();
    for function in &lowered_module.callable_defs {
        let Some(qualname) = original_code_lookup_key(function) else {
            continue;
        };
        let Some(codes) = code_by_qualname.get_mut(qualname) else {
            continue;
        };
        let Some(code) = codes.pop_front() else {
            continue;
        };
        code_by_function_id.insert(function.function_id, code);
    }
    Ok(code_by_function_id)
}

fn resolve_module_name(module_globals: &Bound<'_, PyAny>, operation: &str) -> PyResult<String> {
    let globals = module_globals
        .cast::<PyDict>()
        .map_err(|_| PyTypeError::new_err("module_globals must be a dict"))?;
    let Some(module_name_obj) = globals.get_item("__name__")? else {
        return Err(PyRuntimeError::new_err(format!(
            "JIT basic-block {operation} requires module_globals['__name__']"
        )));
    };
    module_name_obj.extract::<String>().map_err(|_| {
        PyRuntimeError::new_err(format!(
            "JIT basic-block {operation} requires module_globals['__name__'] to be a str"
        ))
    })
}

fn resolve_module_package(module_globals: &Bound<'_, PyAny>, operation: &str) -> PyResult<String> {
    let globals = module_globals
        .cast::<PyDict>()
        .map_err(|_| PyTypeError::new_err("module_globals must be a dict"))?;
    let Some(module_package_obj) = globals.get_item("__package__")? else {
        return Err(PyRuntimeError::new_err(format!(
            "JIT basic-block {operation} requires module_globals['__package__']"
        )));
    };
    module_package_obj.extract::<String>().map_err(|_| {
        PyRuntimeError::new_err(format!(
            "JIT basic-block {operation} requires module_globals['__package__'] to be a str"
        ))
    })
}

fn lookup_module_init_function(
    module: &BlockPyModule<CodegenModuleShape>,
    module_name: &str,
) -> PyResult<BlockPyFunction<CodegenModuleShape>> {
    module
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "_dp_module_init")
        .cloned()
        .ok_or_else(|| {
            PyRuntimeError::new_err(format!(
                "JIT basic-block module init failed to resolve lowered _dp_module_init for {module_name}"
            ))
        })
}

#[pyfunction(signature = (function_id, kind, captures, param_defaults, annotate_fn=None, module_globals=None))]
fn make_function(
    py: Python<'_>,
    function_id: u64,
    kind: &str,
    captures: Py<PyAny>,
    param_defaults: Py<PyAny>,
    annotate_fn: Option<Py<PyAny>>,
    module_globals: Option<Py<PyAny>>,
) -> PyResult<Py<PyAny>> {
    soac_jit::make_function_from_python_args(
        py,
        function_id,
        kind,
        captures,
        param_defaults,
        annotate_fn,
        module_globals,
    )
}

#[pyfunction]
fn create_module(py: Python<'_>, path: &str, spec: Py<PyAny>) -> PyResult<Py<PyAny>> {
    let create_started_at = Instant::now();
    let create_total_start = Instant::now();
    let mut create_timings = Vec::new();
    let spec = spec.bind(py);
    let module_name = module_spec_string_attr(spec.as_any(), "name")?;
    let package_name = module_spec_string_attr(spec.as_any(), "parent")?;
    let source = time_phase(&mut create_timings, "source_read", || {
        fs::read_to_string(path)
            .map_err(|err| PyOSError::new_err(format!("could not read source for {path}: {err}")))
    })?;
    let source_bytes = source.len();
    let source_hash = hash_module_source(&source);
    let module_cache_source = module_cache_source_for_import_path(path);
    let module_info = ModuleInfo {
        hash: source_hash,
        cache_source: Some(module_cache_source),
        indexed_module_keys: Vec::new(),
    };
    let session = soac_jit::CompileSession::process();
    let runtime_names_as_globals = module_name == "soac.runtime";
    let pre_optimization_cache =
        pre_optimization_module_cache(session.as_ref(), module_name.as_str(), module_cache_source)?;
    let env_config = session.env_config().map_err(PyRuntimeError::new_err)?;
    let preparation_options = CodegenPreparationOptions {
        lowering: soac_lowering::LoweringOptions {
            runtime_names_as_globals,
        },
        pre_optimization_cache,
    };
    let mut pass_tracker = RecordingPassTracker::new();
    let lowering_start = Instant::now();
    let codegen_module = time_phase(&mut create_timings, "lower_blockpy", || {
        prepare_codegen_module(
            &source,
            session.module_name_gen(),
            preparation_options,
            env_config,
            &mut pass_tracker,
        )
        .map_err(lowering_error_to_pyerr)
    })?;
    let lowering_total = lowering_start.elapsed();
    let blockpy_pass_timings: Vec<TimedPhase> = pass_tracker
        .pass_timings()
        .map(|timing| TimedPhase {
            name: timing.name,
            elapsed: timing.elapsed,
        })
        .collect();
    let function_count = codegen_module.callable_defs.len();
    let counter_count = codegen_module.counter_defs.len();
    let global_name_count = codegen_module.global_names.len();
    let module_code = time_phase(&mut create_timings, "compile_original_code", || {
        compile_original_module_code(py, &source, path)
    })?;
    let original_code_by_function_id =
        time_phase(&mut create_timings, "match_original_code", || {
            match_original_code_to_functions(py, module_code.bind(py), &codegen_module)
        })?;
    let original_code_count = original_code_by_function_id.len();
    let module = time_phase(&mut create_timings, "soac_ext_module_create", || {
        SoacExtModule::new(
            py,
            spec.as_any(),
            &session,
            codegen_module,
            module_info,
            original_code_by_function_id,
        )
    })?;
    let create_module_total = create_total_start.elapsed();
    store_pending_module_load_timing(
        module.as_ptr(),
        PendingModuleLoadTiming {
            module_name,
            package_name,
            path: path.to_string(),
            source_hash,
            source_bytes,
            create_started_at,
            create_module_total,
            lowering_total,
            blockpy_pass_timings,
            create_timings,
            function_count,
            counter_count,
            global_name_count,
            original_code_count,
        },
    );
    Ok(module)
}

fn ensure_module_builtins(globals: &Bound<'_, PyAny>) -> PyResult<()> {
    let globals = globals
        .cast::<PyDict>()
        .map_err(|_| PyTypeError::new_err("module_globals must be a dict"))?;
    if globals.get_item("__builtins__")?.is_some() {
        return Ok(());
    }
    let builtins = unsafe { ffi::PyEval_GetBuiltins() };
    if builtins.is_null() {
        return Err(PyRuntimeError::new_err(
            "PyEval_GetBuiltins returned null while preparing module globals",
        ));
    }
    let builtins = unsafe { Bound::from_borrowed_ptr(globals.py(), builtins) };
    globals.set_item("__builtins__", builtins)
}

#[pyfunction]
fn exec_module(py: Python<'_>, module: Py<PyAny>) -> PyResult<()> {
    let module = module.bind(py);
    let exec_total_start = Instant::now();
    let mut exec_timings = Vec::new();
    let result = exec_module_inner(py, module.as_any(), &mut exec_timings);
    let exec_module_total = exec_total_start.elapsed();
    append_completed_module_load_log(module.as_ptr(), exec_module_total, exec_timings, &result);
    result
}

fn exec_module_inner(
    py: Python<'_>,
    module: &Bound<'_, PyAny>,
    exec_timings: &mut Vec<TimedPhase>,
) -> PyResult<()> {
    let exec_depth = ExecModuleDepthGuard::enter();
    let module_globals = time_phase(exec_timings, "get_module_globals", || {
        module.getattr("__dict__")
    })?;
    time_phase(exec_timings, "ensure_builtins", || {
        ensure_module_builtins(&module_globals)
    })?;
    let execute_lowered_start = Instant::now();
    let result = SoacExtModule::with_data(module.as_any(), |module_data| {
        let module_name = resolve_module_name(&module_globals, "module execution")?;
        assert_eq!(
            module_name, module_data.shared_state.module_name,
            "module.__dict__['__name__'] did not match the module spec captured at create_module time"
        );
        let package_name = resolve_module_package(&module_globals, "module execution")?;
        assert_eq!(
            package_name, module_data.shared_state.package_name,
            "module.__dict__['__package__'] did not match the module spec captured at create_module time"
        );
        let function =
            lookup_module_init_function(&module_data.shared_state.lowered_module, &module_name)?;
        let is_soac_runtime = module_name == "soac.runtime";
        let soac_runtime_bootstrap = if is_soac_runtime {
            let globals = module_globals.cast::<PyDict>()?;
            Some(time_phase(
                exec_timings,
                "install_soac_runtime_bootstrap",
                || install_soac_runtime_bootstrap_globals(py, globals),
            )?)
        } else {
            None
        };
        let dp = if is_soac_runtime {
            module.cast::<PyModule>()?.clone()
        } else {
            time_phase(exec_timings, "import_soac_runtime", || import_dp_module(py))?
        };
        let empty = PyTuple::empty(py);
        let none = py.None();
        let module_runtime = time_phase(exec_timings, "build_module_runtime_context", || {
            unsafe { soac_jit::build_module_runtime_context_for_module(module.as_ptr()) }.map_err(
                |_| {
                    if unsafe { ffi::PyErr_Occurred() }.is_null() {
                        PyRuntimeError::new_err(
                            "failed to build module runtime context for module execution",
                        )
                    } else {
                        PyErr::fetch(py)
                    }
                },
            )
        })?;
        let module_init = time_phase(exec_timings, "instantiate_module_init", || {
            soac_jit::instantiate_bb_function(
                py,
                &dp,
                &module_name,
                &function,
                empty.as_any(),
                empty.as_any(),
                &module_globals,
                none.bind(py),
                &module_runtime,
            )
        })?;
        let result = time_phase(exec_timings, "call_module_init", || module_init.call0(py));
        result?;
        if let Some(bootstrap) = &soac_runtime_bootstrap {
            let globals = module_globals.cast::<PyDict>()?;
            time_phase(exec_timings, "restore_soac_runtime_bootstrap", || {
                bootstrap.restore(py, globals)
            })?;
        }
        time_phase(exec_timings, "register_function_owner_types", || {
            unsafe {
                soac_jit::register_function_owner_types_for_module_keys(
                    module.as_ptr(),
                    &module_data.shared_state.lowered_module.global_names,
                )
            }
            .map_err(|_| {
                if unsafe { ffi::PyErr_Occurred() }.is_null() {
                    PyRuntimeError::new_err(
                        "failed to register function owner types for type invalidation",
                    )
                } else {
                    PyErr::fetch(py)
                }
            })
        })?;
        if exec_depth.is_outermost() && !is_soac_runtime {
            time_phase(exec_timings, "start_background_jit_compile", || {
                unsafe { soac_jit::start_background_jit_compile_for_module(module.as_ptr()) }
                    .map_err(|_| {
                        if unsafe { ffi::PyErr_Occurred() }.is_null() {
                            PyRuntimeError::new_err(
                                "failed to start background JIT compile for module",
                            )
                        } else {
                            PyErr::fetch(py)
                        }
                    })
            })?;
        }
        Ok(())
    });
    exec_timings.push(TimedPhase {
        name: "execute_lowered_module".to_string(),
        elapsed: execute_lowered_start.elapsed(),
    });
    result
}

#[pyfunction]
fn profile_watch_type_key_layout(type_obj: &Bound<'_, PyAny>) -> PyResult<()> {
    unsafe { soac_jit::module_type::watch_split_keys_for_type(type_obj.as_ptr()) }.map_err(|_| {
        Python::attach(|py| {
            if unsafe { ffi::PyErr_Occurred() }.is_null() {
                PyRuntimeError::new_err("failed to watch type split-key layout")
            } else {
                PyErr::fetch(py)
            }
        })
    })
}

#[pyfunction]
fn force_entry_interpreter_for_tests(enabled: bool) -> bool {
    soac_jit::force_entry_interpreter_vectorcall_for_tests(enabled)
}

#[pyfunction(signature = (name, globals, fromlist=None, level=0))]
fn import_(
    py: Python<'_>,
    name: &Bound<'_, PyAny>,
    globals: &Bound<'_, PyAny>,
    fromlist: Option<&Bound<'_, PyAny>>,
    level: i32,
) -> PyResult<Py<PyAny>> {
    soac_jit::import_helpers::import_module_level(py, name, globals, fromlist, level)
}

#[pyfunction]
fn import_attr(
    py: Python<'_>,
    module: &Bound<'_, PyAny>,
    name: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    soac_jit::import_helpers::import_from(py, module, name)
}

pub(crate) fn add_module_functions(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(create_module, module)?)?;
    module.add_function(wrap_pyfunction!(exec_module, module)?)?;
    module.add_function(wrap_pyfunction!(make_function, module)?)?;
    module.add_function(wrap_pyfunction!(profile_watch_type_key_layout, module)?)?;
    module.add_function(wrap_pyfunction!(force_entry_interpreter_for_tests, module)?)?;
    module.add_function(wrap_pyfunction!(import_, module)?)?;
    module.add_function(wrap_pyfunction!(import_attr, module)?)?;
    Ok(())
}
