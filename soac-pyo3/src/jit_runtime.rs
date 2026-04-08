use crate::lowering_error_to_pyerr;
use log::info;
use pyo3::exceptions::{
    PyAttributeError, PyNotImplementedError, PyOSError, PyRuntimeError, PyTypeError, PyValueError,
};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyFunction, PyModule, PyString, PyTuple};
use soac_blockpy::block_py::{BlockPyFunction, BlockPyModule, FunctionId, FunctionKind, ParamKind};
use soac_blockpy::lower_python_to_blockpy;
use soac_blockpy::pass_tracker::NoopPassTracker;
use soac_blockpy::passes::CodegenModuleShape;
use soac_jit::module_type::{hash_module_source, ModuleInfo, SoacExtModule};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::time::Instant;

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
    std::env::var("DIET_PYTHON_JIT_COMPILE_MODE")
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
    _module_name: &str,
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
    register_lazy_vectorcall(py, &entry, function.function_id, module_runtime)?;
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
    let source = fs::read_to_string(path)
        .map_err(|err| PyOSError::new_err(format!("could not read source for {path}: {err}")))?;
    let module_info = ModuleInfo {
        hash: hash_module_source(&source),
        indexed_module_keys: Vec::new(),
    };
    let session = soac_jit::CompileSession::new();
    let output: soac_blockpy::LoweringResult<NoopPassTracker> =
        lower_python_to_blockpy(&source, session.module_name_gen())
            .map_err(lowering_error_to_pyerr)?;
    let module_code = compile_original_module_code(py, &source, path)?;
    let original_code_by_function_id =
        match_original_code_to_functions(py, module_code.bind(py), &output.codegen_module)?;
    SoacExtModule::new(
        py,
        spec.bind(py).as_any(),
        output.codegen_module,
        module_info,
        original_code_by_function_id,
    )
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
    let module_globals = module.getattr("__dict__")?;
    ensure_module_builtins(&module_globals)?;
    SoacExtModule::with_data(module.as_any(), |module_data| {
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
        register_blockpy_module_plans(&module_name, &module_data.shared_state.lowered_module)?;
        let function =
            lookup_module_init_function(&module_data.shared_state.lowered_module, &module_name)?;
        let dp = import_dp_module(py)?;
        let empty = PyTuple::empty(py);
        let none = py.None();
        let mut module_runtime =
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
            )?;
        let module_init = instantiate_bb_function(
            py,
            &dp,
            &module_name,
            &function,
            empty.as_any(),
            empty.as_any(),
            &module_globals,
            none.bind(py),
            &module_runtime,
        )?;
        let result = unsafe {
            soac_jit::with_active_module_runtime_context(
                std::ptr::addr_of_mut!(module_runtime),
                || module_init.call0(py),
            )
        };
        result?;
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
        )?;
        Ok(())
    })
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
