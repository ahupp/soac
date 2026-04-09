use crate::lowering_error_to_pyerr;
use pyo3::exceptions::{
    PyAttributeError, PyNotImplementedError, PyOSError, PyRuntimeError, PyTypeError, PyValueError,
};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyFunction, PyModule, PyString, PyTuple};
use soac_blockpy::block_py::{BlockPyFunction, BlockPyModule, FunctionId, FunctionKind, ParamKind};
use soac_blockpy::passes::CodegenModuleShape;
use soac_blockpy::{LoweringOptions, lower_python_to_blockpy_recorded_with_options};
use soac_jit::module_type::{ModuleInfo, SharedModuleState, SoacExtModule, hash_module_source};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::info;

unsafe extern "C" {
    static mut PyCell_Type: ffi::PyTypeObject;
    fn PyCell_New(obj: *mut ffi::PyObject) -> *mut ffi::PyObject;
}

fn is_cell_object(obj: *mut ffi::PyObject) -> bool {
    unsafe { !obj.is_null() && ffi::Py_TYPE(obj) == std::ptr::addr_of_mut!(PyCell_Type) }
}

fn import_dp_module<'py>(py: Python<'py>) -> PyResult<Bound<'py, PyModule>> {
    PyModule::import(py, "soac.runtime")
}

fn install_soac_runtime_bootstrap_sentinel<'py>(
    py: Python<'py>,
    globals: &Bound<'py, PyDict>,
    shared_state: &SharedModuleState,
    name: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let id = shared_state
        .codegen_constants
        .require_runtime_name_constant_id(name);
    let obj = shared_state.module_constant_obj(id).ok_or_else(|| {
        PyRuntimeError::new_err(format!("missing runtime bootstrap constant {name:?}"))
    })?;
    let obj = obj.bind(py).clone();
    globals.set_item(name, &obj)?;
    Ok(obj)
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
    shared_state: &SharedModuleState,
) -> PyResult<SoacRuntimeBootstrapGlobals> {
    let bootstrap = soac_jit::module_constants::build_soac_runtime_bootstrap_module(py)?;
    let deleted = install_soac_runtime_bootstrap_sentinel(py, globals, shared_state, "DELETED")?;
    install_soac_runtime_bootstrap_sentinel(py, globals, shared_state, "ITER_COMPLETE")?;
    bootstrap.setattr("DELETED", deleted)?;
    let mut helpers = Vec::new();
    for name in [
        "_entry_template",
        "code_with_freevars",
        "tuple_values",
        "make_function",
        "create_class",
        "import_",
        "import_attr",
        "class_lookup_global",
        "class_lookup_cell",
    ] {
        let helper = bootstrap.getattr(name)?;
        globals.set_item(name, &helper)?;
        helpers.push((name, helper.unbind()));
    }
    Ok(SoacRuntimeBootstrapGlobals { helpers })
}

fn tuple_from_owned_objects<'py>(
    py: Python<'py>,
    objects: Vec<Py<PyAny>>,
) -> PyResult<Bound<'py, PyTuple>> {
    let tuple = unsafe { ffi::PyTuple_New(objects.len() as ffi::Py_ssize_t) };
    let tuple = unsafe { Bound::from_owned_ptr_or_err(py, tuple)? }.cast_into::<PyTuple>()?;
    for (index, object) in objects.into_iter().enumerate() {
        if unsafe {
            ffi::PyTuple_SetItem(tuple.as_ptr(), index as ffi::Py_ssize_t, object.into_ptr())
        } != 0
        {
            return Err(PyErr::fetch(py));
        }
    }
    Ok(tuple)
}

type OriginalCodeMap = HashMap<FunctionId, Py<PyAny>>;
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

    let consts = code.getattr("co_consts")?.cast_into::<PyTuple>()?;
    for item in consts.iter() {
        if item.is_instance(code_type)? {
            collect_original_code_objects(&item, code_type, by_qualname)?;
        }
    }
    Ok(())
}

fn original_code_lookup_key(function: &BlockPyFunction<CodegenModuleShape>) -> Option<&str> {
    let qualname = function.names.qualname.as_str();
    if qualname == "_dp_module_init"
        || qualname == "__annotate__"
        || qualname.ends_with(".__annotate_func__")
        || function.names.fn_name == "_dp_resume"
        || function.names.bind_name.starts_with("_dp_class_ns_")
        || function.names.bind_name.starts_with("_dp_define_class_")
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

pub(crate) fn register_lowered_module_plans<P>(
    output: &soac_blockpy::LoweringResult<P>,
    module_name: &str,
) -> PyResult<()> {
    register_blockpy_module_plans(module_name, &output.codegen_module)
}

fn register_blockpy_module_plans(
    module_name: &str,
    module: &BlockPyModule<CodegenModuleShape>,
) -> PyResult<()> {
    soac_jit::register_clif_module_plans(module_name, module).map_err(|err| {
        pyo3::exceptions::PyRuntimeError::new_err(format!(
            "failed to register BB plans for {module_name}: {err}"
        ))
    })?;
    if module_name.ends_with(".__main__") && module_name != "__main__" {
        soac_jit::register_clif_module_plans("__main__", module).map_err(|err| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "failed to register BB plans alias for __main__ from {module_name}: {err}"
            ))
        })?;
    }
    Ok(())
}

fn make_lazy_clif_entry<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    function_name: &str,
    module_globals: &Bound<'py, PyAny>,
    original_code: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let module_globals = module_globals
        .cast::<PyDict>()
        .map_err(|_| PyTypeError::new_err("module_globals must be a dict"))?;
    let template;
    let template_code;
    let code = match original_code {
        Some(code) => code,
        None => {
            template = dp.getattr("_entry_template")?;
            template_code = template.getattr("__code__")?;
            &template_code
        }
    };
    unsafe {
        let func = ffi::PyFunction_New(code.as_ptr(), module_globals.as_ptr());
        if func.is_null() {
            return Err(PyErr::fetch(py));
        }
        let func = Bound::from_owned_ptr(py, func);
        func.setattr("__name__", function_name)?;
        Ok(func)
    }
}

fn register_clif_vectorcall_raw(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    function_id: FunctionId,
    module_runtime: soac_jit::ModuleRuntimeContext,
) -> PyResult<()> {
    unsafe {
        soac_jit::register_clif_vectorcall(func.as_ptr(), function_id, module_runtime).map_err(
            |_| {
                if ffi::PyErr_Occurred().is_null() {
                    PyRuntimeError::new_err("failed to register CLIF vectorcall")
                } else {
                    PyErr::fetch(py)
                }
            },
        )
    }
}

fn eager_clif_compile_requested() -> bool {
    std::env::var("SOAC_COMPILE_MODE")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("eager"))
        .unwrap_or(false)
}

fn maybe_eager_compile_clif_entry(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    module_runtime: &soac_jit::ModuleRuntimeContext,
    function_id: FunctionId,
) -> PyResult<()> {
    if !eager_clif_compile_requested() {
        return Ok(());
    }
    let start = Instant::now();
    let compile_result = unsafe {
        soac_jit::compile_clif_vectorcall(func.as_ptr()).map_err(|_| {
            if ffi::PyErr_Occurred().is_null() {
                PyRuntimeError::new_err("failed to eagerly compile CLIF entry")
            } else {
                PyErr::fetch(py)
            }
        })
    };
    match compile_result {
        Ok(()) => {
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            info!(
                "soac_jit_eager_compile module={} function_id={} elapsed_ms={elapsed_ms:.3}",
                module_runtime.shared_module_state_owner.module_name, function_id
            );
            Ok(())
        }
        Err(err) if err.is_instance_of::<PyNotImplementedError>(py) => Err(err),
        Err(err) => Err(PyRuntimeError::new_err(format!(
            "failed to eagerly compile CLIF entry for {module_name} function_id={function_id}: {err}",
            module_name = module_runtime.shared_module_state_owner.module_name,
            function_id = function_id
        ))),
    }
}

fn register_lazy_vectorcall(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    function_id: FunctionId,
    module_runtime: &soac_jit::ModuleRuntimeContext,
) -> PyResult<()> {
    let owned_runtime =
        unsafe { soac_jit::clone_module_runtime_context(module_runtime) }.map_err(|_| {
            if unsafe { ffi::PyErr_Occurred() }.is_null() {
                PyRuntimeError::new_err("failed to clone module runtime context")
            } else {
                PyErr::fetch(py)
            }
        })?;
    match register_clif_vectorcall_raw(py, func, function_id, owned_runtime) {
        Ok(()) => maybe_eager_compile_clif_entry(py, func, module_runtime, function_id),
        Err(err) if err.is_instance_of::<PyNotImplementedError>(py) => Err(err),
        Err(err) => Err(PyRuntimeError::new_err(format!(
            "failed to register lazy CLIF vectorcall for {module_name} function_id={function_id}: {err}",
            module_name = module_runtime.shared_module_state_owner.module_name,
            function_id = function_id
        ))),
    }
}

fn ignore_attr_or_type_error(py: Python<'_>, result: PyResult<()>) -> PyResult<()> {
    match result {
        Ok(()) => Ok(()),
        Err(err)
            if err.is_instance_of::<PyAttributeError>(py)
                || err.is_instance_of::<PyTypeError>(py) =>
        {
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn ignore_attr_or_value_error<T>(py: Python<'_>, result: PyResult<T>) -> PyResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(err)
            if err.is_instance_of::<PyAttributeError>(py)
                || err.is_instance_of::<PyValueError>(py) =>
        {
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

fn update_function_metadata(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    qualname: &str,
    name: &str,
    doc: Option<&str>,
    annotate_fn: &Bound<'_, PyAny>,
) -> PyResult<()> {
    ignore_attr_or_type_error(py, func.setattr("__qualname__", qualname))?;
    ignore_attr_or_type_error(py, func.setattr("__name__", name))?;
    if func.cast::<PyFunction>().is_ok() {
        let kwargs = PyDict::new(py);
        kwargs.set_item("co_name", name)?;
        kwargs.set_item("co_qualname", qualname)?;
        if let Some(replaced) = ignore_attr_or_value_error(
            py,
            func.getattr("__code__")?
                .call_method("replace", (), Some(&kwargs)),
        )? {
            ignore_attr_or_type_error(py, func.setattr("__code__", replaced))?;
        }
    }
    if let Some(doc) = doc {
        ignore_attr_or_type_error(py, func.setattr("__doc__", doc))?;
    }
    if !annotate_fn.is_none() {
        ignore_attr_or_type_error(py, func.setattr("__annotate__", annotate_fn))?;
    }
    Ok(())
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

fn module_globals_from_runtime<'py>(
    py: Python<'py>,
    module_runtime: &soac_jit::ModuleRuntimeContext,
    operation: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let globals_ptr = module_runtime.mod_ctx.globals_obj as *mut ffi::PyObject;
    if globals_ptr.is_null() {
        return Err(PyRuntimeError::new_err(format!(
            "JIT basic-block {operation} requires module runtime globals"
        )));
    }
    Ok(unsafe { Bound::from_borrowed_ptr(py, globals_ptr) })
}

fn module_name_from_runtime(
    module_runtime: &soac_jit::ModuleRuntimeContext,
    operation: &str,
) -> PyResult<String> {
    let module_name = module_runtime
        .shared_module_state_owner
        .module_name
        .as_str();
    if module_name.is_empty() {
        return Err(PyRuntimeError::new_err(format!(
            "JIT basic-block {operation} requires shared module state"
        )));
    }
    Ok(module_name.to_string())
}

fn lookup_bb_function(
    shared_state: &soac_jit::module_type::SharedModuleState,
    function_id: FunctionId,
    operation: &str,
) -> PyResult<BlockPyFunction<CodegenModuleShape>> {
    shared_state.lookup_function(function_id).cloned().ok_or_else(|| {
        PyRuntimeError::new_err(format!(
            "JIT basic-block {operation} failed to resolve static function metadata for {}.fn#{}",
            shared_state.module_name,
            function_id
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

fn build_capture_map<'py>(
    py: Python<'py>,
    captures: &Bound<'py, PyAny>,
) -> PyResult<(Vec<String>, Bound<'py, PyDict>)> {
    let captures = captures.cast::<PyTuple>().map_err(|_| {
        PyTypeError::new_err(format!(
            "bb captures must be a tuple, got {:?}",
            captures.get_type()
        ))
    })?;
    let closure_values = PyDict::new(py);
    let mut captured_names = Vec::with_capacity(captures.len());
    for item in captures.iter() {
        let item = item
            .cast::<PyTuple>()
            .map_err(|_| PyTypeError::new_err(format!("invalid bb capture payload: {item:?}")))?;
        if item.len() != 2 {
            return Err(PyTypeError::new_err(format!(
                "invalid bb capture payload: {item:?}"
            )));
        }
        let name = item
            .get_item(0)?
            .extract::<String>()
            .map_err(|_| PyTypeError::new_err(format!("invalid bb capture payload: {item:?}")))?;
        let value = item.get_item(1)?;
        closure_values.set_item(name.as_str(), &value)?;
        captured_names.push(name);
    }
    Ok((captured_names, closure_values))
}

fn split_param_defaults<'py>(
    py: Python<'py>,
    function: &BlockPyFunction<CodegenModuleShape>,
    param_defaults: &Bound<'py, PyAny>,
) -> PyResult<(Option<Bound<'py, PyTuple>>, Option<Bound<'py, PyDict>>)> {
    let defaults = param_defaults.cast::<PyTuple>().map_err(|_| {
        PyTypeError::new_err(format!(
            "bb param defaults must be a tuple, got {:?}",
            param_defaults.get_type()
        ))
    })?;
    let mut default_index = 0usize;
    let mut positional_defaults = Vec::new();
    let kwdefaults = PyDict::new(py);
    for param in &function.params.params {
        if !param.has_default {
            continue;
        }
        let value = defaults.get_item(default_index).map_err(|_| {
            PyRuntimeError::new_err("bb param defaults payload is shorter than the param spec")
        })?;
        default_index += 1;
        match param.kind {
            ParamKind::PosOnly | ParamKind::Any => positional_defaults.push(value.unbind()),
            ParamKind::KwOnly => kwdefaults.set_item(param.name.as_str(), &value)?,
            ParamKind::VarArg | ParamKind::KwArg => {
                return Err(PyRuntimeError::new_err(format!(
                    "invalid default-bearing bb param kind: {:?}",
                    param.kind
                )));
            }
        }
    }
    if default_index != defaults.len() {
        return Err(PyRuntimeError::new_err(
            "bb param defaults payload is longer than the param spec",
        ));
    }
    let positional_defaults = if positional_defaults.is_empty() {
        None
    } else {
        Some(tuple_from_owned_objects(py, positional_defaults)?)
    };
    let kwdefaults = if kwdefaults.is_empty() {
        None
    } else {
        Some(kwdefaults)
    };
    Ok((positional_defaults, kwdefaults))
}

fn inspect_param_kind<'py>(
    inspect_module: &Bound<'py, PyModule>,
    kind: ParamKind,
) -> PyResult<Bound<'py, PyAny>> {
    let parameter = inspect_module.getattr("Parameter")?;
    match kind {
        ParamKind::PosOnly => parameter.getattr("POSITIONAL_ONLY"),
        ParamKind::Any => parameter.getattr("POSITIONAL_OR_KEYWORD"),
        ParamKind::VarArg => parameter.getattr("VAR_POSITIONAL"),
        ParamKind::KwOnly => parameter.getattr("KEYWORD_ONLY"),
        ParamKind::KwArg => parameter.getattr("VAR_KEYWORD"),
    }
}

fn build_bb_signature<'py>(
    py: Python<'py>,
    function: &BlockPyFunction<CodegenModuleShape>,
    param_defaults: &Bound<'py, PyAny>,
) -> PyResult<Py<PyAny>> {
    let inspect_module = PyModule::import(py, "inspect")?;
    let parameter = inspect_module.getattr("Parameter")?;
    let signature = inspect_module.getattr("Signature")?;
    let empty_default = inspect_module.getattr("_empty")?;
    let defaults = param_defaults.cast::<PyTuple>().map_err(|_| {
        PyTypeError::new_err(format!(
            "bb param defaults must be a tuple, got {:?}",
            param_defaults.get_type()
        ))
    })?;
    let mut default_index = 0usize;
    let mut signature_params = Vec::with_capacity(function.params.params.len());
    for param in &function.params.params {
        let kind = inspect_param_kind(&inspect_module, param.kind)?;
        let kwargs = PyDict::new(py);
        kwargs.set_item("name", param.name.as_str())?;
        kwargs.set_item("kind", &kind)?;
        if param.has_default {
            let value = defaults.get_item(default_index).map_err(|_| {
                PyRuntimeError::new_err("bb param defaults payload is shorter than the param spec")
            })?;
            default_index += 1;
            kwargs.set_item("default", &value)?;
        } else {
            kwargs.set_item("default", &empty_default)?;
        }
        signature_params.push(parameter.call((), Some(&kwargs))?.unbind());
    }
    if default_index != defaults.len() {
        return Err(PyRuntimeError::new_err(
            "bb param defaults payload is longer than the param spec",
        ));
    }
    let signature_obj = signature.call1((tuple_from_owned_objects(py, signature_params)?,))?;
    Ok(signature_obj.unbind())
}

fn build_closure_shaped_entry<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    function: &BlockPyFunction<CodegenModuleShape>,
    module_globals: &Bound<'py, PyAny>,
    qualname: &str,
    captured_names: &[String],
    captured_values: &Bound<'py, PyDict>,
    original_code: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    debug_assert!(!captured_names.is_empty());
    let generated_code;
    let code = match original_code {
        Some(code) => code.clone(),
        None => {
            let (is_async, is_generator) = match function.lowered_kind() {
                FunctionKind::Function => (false, false),
                FunctionKind::Coroutine => (true, false),
                FunctionKind::Generator => (false, true),
                FunctionKind::AsyncGenerator => (true, true),
            };
            generated_code = dp.getattr("code_with_freevars")?.call1((
                PyTuple::new(py, captured_names)?,
                is_async,
                is_generator,
            ))?;
            generated_code
        }
    };
    let freevars_obj = code.getattr("co_freevars")?;
    let freevars = freevars_obj.cast::<PyTuple>()?;
    let mut closure_cells = Vec::with_capacity(freevars.len());
    for name_obj in freevars.iter() {
        let name = name_obj.extract::<String>()?;
        let value = captured_values.get_item(name.as_str())?.ok_or_else(|| {
            PyRuntimeError::new_err(format!(
                "missing captured value for closure freevar {name:?}"
            ))
        })?;
        if is_cell_object(value.as_ptr()) {
            closure_cells.push(value.clone().unbind());
        } else {
            let cell = unsafe { PyCell_New(value.as_ptr()) };
            if cell.is_null() {
                return Err(PyErr::fetch(py));
            }
            closure_cells.push(unsafe { Bound::from_owned_ptr(py, cell) }.unbind());
        }
    }
    let closure = tuple_from_owned_objects(py, closure_cells)?;
    let qualname = PyString::new(py, qualname);
    let func = unsafe {
        let ptr = ffi::PyFunction_NewWithQualName(
            code.as_ptr(),
            module_globals.as_ptr(),
            qualname.as_ptr(),
        );
        if ptr.is_null() {
            return Err(PyErr::fetch(py));
        }
        Bound::from_owned_ptr(py, ptr)
    };
    if unsafe { ffi::PyFunction_SetClosure(func.as_ptr(), closure.as_ptr()) } != 0 {
        return Err(PyErr::fetch(py));
    }
    Ok(func.into_any())
}

fn apply_function_defaults(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    positional_defaults: Option<&Bound<'_, PyTuple>>,
    kwdefaults: Option<&Bound<'_, PyDict>>,
) -> PyResult<()> {
    let defaults_obj = positional_defaults.map_or_else(
        || py.None().into_any(),
        |value| value.clone().into_any().unbind(),
    );
    if unsafe { ffi::PyFunction_SetDefaults(func.as_ptr(), defaults_obj.as_ptr()) } != 0 {
        return Err(PyErr::fetch(py));
    }
    let kwdefaults_obj = kwdefaults.map_or_else(
        || py.None().into_any(),
        |value| value.clone().into_any().unbind(),
    );
    if unsafe { ffi::PyFunction_SetKwDefaults(func.as_ptr(), kwdefaults_obj.as_ptr()) } != 0 {
        return Err(PyErr::fetch(py));
    }
    Ok(())
}

fn instantiate_bb_function(
    py: Python<'_>,
    dp: &Bound<'_, PyModule>,
    module_name: &str,
    function: &BlockPyFunction<CodegenModuleShape>,
    captures: &Bound<'_, PyAny>,
    param_defaults: &Bound<'_, PyAny>,
    module_globals: &Bound<'_, PyAny>,
    annotate_fn: &Bound<'_, PyAny>,
    module_runtime: &soac_jit::ModuleRuntimeContext,
) -> PyResult<Py<PyAny>> {
    let signature = build_bb_signature(py, function, param_defaults)?;
    let entry = instantiate_closure_backed_entry(
        py,
        dp,
        module_name,
        function,
        captures,
        module_globals,
        module_runtime,
        function.names.display_name.as_str(),
        function.names.qualname.as_str(),
    )?;
    let (positional_defaults, kwdefaults) = split_param_defaults(py, function, param_defaults)?;
    apply_function_defaults(
        py,
        &entry,
        positional_defaults.as_ref(),
        kwdefaults.as_ref(),
    )?;
    entry.setattr("__signature__", signature.bind(py))?;
    update_function_metadata(
        py,
        &entry,
        function.names.qualname.as_str(),
        function.names.display_name.as_str(),
        function.doc.as_deref(),
        annotate_fn,
    )?;
    entry.setattr("__module__", module_name)?;
    Ok(entry.unbind())
}

fn instantiate_closure_backed_entry<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    module_name: &str,
    function: &BlockPyFunction<CodegenModuleShape>,
    captures: &Bound<'py, PyAny>,
    module_globals: &Bound<'py, PyAny>,
    module_runtime: &soac_jit::ModuleRuntimeContext,
    entry_name: &str,
    qualname: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let (captured_names, closure_values) = build_capture_map(py, captures)?;
    let original_code = module_runtime
        .shared_module_state_owner
        .lookup_original_code(function.function_id)
        .map(|code| code.bind(py));
    let entry = if captured_names.is_empty() {
        make_lazy_clif_entry(
            py,
            dp,
            entry_name,
            module_globals,
            original_code.as_ref().map(|code| code.as_any()),
        )?
    } else {
        build_closure_shaped_entry(
            py,
            dp,
            function,
            module_globals,
            qualname,
            &captured_names,
            &closure_values,
            original_code.as_ref().map(|code| code.as_any()),
        )?
    };
    // soac.runtime's source helpers are the runtime ABI for other transformed
    // modules.  Running them through lazy_vectorcall would push a soac.runtime
    // context while the caller's module context must remain current.
    if !(module_name == "soac.runtime" && original_code.is_some()) {
        register_lazy_vectorcall(py, &entry, function.function_id, module_runtime)?;
    }
    Ok(entry)
}

#[pyfunction]
fn make_bb_function(
    py: Python<'_>,
    function_id: u64,
    captures: Py<PyAny>,
    param_defaults: Py<PyAny>,
    annotate_fn: Py<PyAny>,
) -> PyResult<Py<PyAny>> {
    let dp = import_dp_module(py)?;
    unsafe {
        soac_jit::with_current_module_runtime_context(|module_runtime| {
            let module_globals =
                module_globals_from_runtime(py, module_runtime, "function instantiation")?;
            let module_name = module_name_from_runtime(module_runtime, "function instantiation")?;
            let function = lookup_bb_function(
                &module_runtime.shared_module_state_owner,
                FunctionId::from_packed(function_id),
                "function instantiation",
            )?;
            instantiate_bb_function(
                py,
                &dp,
                &module_name,
                &function,
                captures.bind(py).as_any(),
                param_defaults.bind(py).as_any(),
                &module_globals,
                annotate_fn.bind(py),
                module_runtime,
            )
        })
        .map_err(|_| {
            if ffi::PyErr_Occurred().is_null() {
                PyRuntimeError::new_err(
                    "function instantiation requires an active module runtime context",
                )
            } else {
                PyErr::fetch(py)
            }
        })?
    }
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
    let module_info = ModuleInfo {
        hash: source_hash,
        indexed_module_keys: Vec::new(),
    };
    let session = soac_jit::CompileSession::new();
    let lowering_options = LoweringOptions {
        runtime_names_as_globals: module_name == "soac.runtime",
    };
    let output = time_phase(&mut create_timings, "lower_blockpy", || {
        lower_python_to_blockpy_recorded_with_options(
            &source,
            session.module_name_gen(),
            lowering_options,
        )
        .map_err(lowering_error_to_pyerr)
    })?;
    let lowering_total = output.total_time;
    let blockpy_pass_timings: Vec<TimedPhase> = output
        .pass_tracker
        .pass_timings()
        .map(|timing| TimedPhase {
            name: timing.name,
            elapsed: timing.elapsed,
        })
        .collect();
    let function_count = output.codegen_module.callable_defs.len();
    let counter_count = output.codegen_module.counter_defs.len();
    let global_name_count = output.codegen_module.global_names.len();
    let module_code = time_phase(&mut create_timings, "compile_original_code", || {
        compile_original_module_code(py, &source, path)
    })?;
    let original_code_by_function_id =
        time_phase(&mut create_timings, "match_original_code", || {
            match_original_code_to_functions(py, module_code.bind(py), &output.codegen_module)
        })?;
    let original_code_count = original_code_by_function_id.len();
    let module = time_phase(&mut create_timings, "soac_ext_module_create", || {
        SoacExtModule::new(
            py,
            spec.as_any(),
            output.codegen_module,
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
        time_phase(exec_timings, "register_blockpy_module_plans", || {
            register_blockpy_module_plans(&module_name, &module_data.shared_state.lowered_module)
        })?;
        let function =
            lookup_module_init_function(&module_data.shared_state.lowered_module, &module_name)?;
        let is_soac_runtime = module_name == "soac.runtime";
        let soac_runtime_bootstrap = if is_soac_runtime {
            let globals = module_globals.cast::<PyDict>()?;
            Some(time_phase(
                exec_timings,
                "install_soac_runtime_bootstrap",
                || install_soac_runtime_bootstrap_globals(py, globals, &module_data.shared_state),
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
        let mut module_runtime = time_phase(exec_timings, "build_module_runtime_context", || {
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
            instantiate_bb_function(
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
        let result = time_phase(exec_timings, "call_module_init", || unsafe {
            soac_jit::with_active_module_runtime_context(
                std::ptr::addr_of_mut!(module_runtime),
                || module_init.call0(py),
            )
        });
        result?;
        if let Some(bootstrap) = &soac_runtime_bootstrap {
            let globals = module_globals.cast::<PyDict>()?;
            time_phase(exec_timings, "restore_soac_runtime_bootstrap", || {
                bootstrap.restore(py, globals)
            })?;
        }
        time_phase(exec_timings, "register_function_owner_types", || {
            unsafe { soac_jit::register_function_owner_types_for_module(module.as_ptr()) }.map_err(
                |_| {
                    if unsafe { ffi::PyErr_Occurred() }.is_null() {
                        PyRuntimeError::new_err(
                            "failed to register function owner types for type invalidation",
                        )
                    } else {
                        PyErr::fetch(py)
                    }
                },
            )
        })?;
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

pub(crate) fn add_module_functions(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(create_module, module)?)?;
    module.add_function(wrap_pyfunction!(exec_module, module)?)?;
    module.add_function(wrap_pyfunction!(make_bb_function, module)?)?;
    module.add_function(wrap_pyfunction!(profile_watch_type_key_layout, module)?)?;
    Ok(())
}
