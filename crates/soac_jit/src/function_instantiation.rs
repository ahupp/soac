use crate::jit::{ModuleJitContext, ModuleRuntimeContext};
use crate::module_type::SharedModuleState;
use crate::{
    CompileSession, FunctionInstantiationTemplate, clone_module_runtime_context,
    compile_clif_vectorcall,
};
use pyo3::exceptions::{
    PyAttributeError, PyNotImplementedError, PyRuntimeError, PyTypeError, PyValueError,
};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyFunction, PyModule, PyString, PyTuple};
use soac_core::block_py::{
    BlockPyFunction, FunctionExecutionMode, FunctionKind, ParamKind, RuntimeFunctionId,
};
use soac_ir_blockpy::BlockPyModuleShape;
use std::ffi::{CString, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, trace};

pub(crate) const SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL: &str =
    "soac_jit_make_function_with_closure";

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

fn tuple_from_strings<'py>(py: Python<'py>, values: &[String]) -> PyResult<Bound<'py, PyTuple>> {
    // Keep this on the CPython tuple API because this code runs against vendored
    // CPython builds where PyO3 tuple iteration/construction can lag layout changes.
    let tuple = unsafe { ffi::PyTuple_New(values.len() as ffi::Py_ssize_t) };
    if tuple.is_null() {
        return Err(PyErr::fetch(py));
    }
    for (index, value) in values.iter().enumerate() {
        let value_len = ffi::Py_ssize_t::try_from(value.len())
            .map_err(|_| PyValueError::new_err("tuple string value is too large"))?;
        let string = unsafe { ffi::PyUnicode_FromStringAndSize(value.as_ptr().cast(), value_len) };
        if string.is_null() {
            unsafe { ffi::Py_DECREF(tuple) };
            return Err(PyErr::fetch(py));
        }
        if unsafe { ffi::PyTuple_SetItem(tuple, index as ffi::Py_ssize_t, string) } != 0 {
            unsafe { ffi::Py_DECREF(tuple) };
            return Err(PyErr::fetch(py));
        }
    }
    Ok(unsafe { Bound::from_owned_ptr_or_err(py, tuple)? }.cast_into::<PyTuple>()?)
}

fn tuple_strings(py: Python<'_>, tuple: &Bound<'_, PyTuple>) -> PyResult<Vec<String>> {
    // See tuple_from_strings: use the CPython tuple API at this ABI boundary.
    let mut values = Vec::with_capacity(tuple.len());
    for index in 0..tuple.len() {
        let item = unsafe { ffi::PyTuple_GetItem(tuple.as_ptr(), index as ffi::Py_ssize_t) };
        if item.is_null() {
            return Err(PyErr::fetch(py));
        }
        let item = unsafe { Bound::from_borrowed_ptr(py, item) };
        values.push(item.extract::<String>()?);
    }
    Ok(values)
}

fn register_clif_vectorcall_raw(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    function_id: RuntimeFunctionId,
    module_runtime: ModuleRuntimeContext,
) -> PyResult<()> {
    unsafe {
        crate::register_clif_vectorcall(func.as_ptr(), function_id, module_runtime).map_err(|_| {
            if ffi::PyErr_Occurred().is_null() {
                PyRuntimeError::new_err("failed to register CLIF vectorcall")
            } else {
                PyErr::fetch(py)
            }
        })
    }
}

fn attach_ready_clif_direct_entry_raw(py: Python<'_>, func: &Bound<'_, PyAny>) -> PyResult<bool> {
    unsafe { crate::attach_ready_clif_direct_entry(func.as_ptr()) }.map_err(|_| {
        if unsafe { ffi::PyErr_Occurred() }.is_null() {
            PyRuntimeError::new_err("failed to attach ready CLIF direct entry")
        } else {
            PyErr::fetch(py)
        }
    })
}

fn maybe_eager_compile_clif_entry(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    module_runtime: &ModuleRuntimeContext,
    function_id: RuntimeFunctionId,
) -> PyResult<()> {
    if !module_runtime
        .compile_session
        .env_config()
        .map_err(PyRuntimeError::new_err)?
        .eager_clif_compile_requested()
    {
        return Ok(());
    }
    if module_runtime
        .shared_module_state_owner
        .lookup_function(function_id)
        .is_some_and(|function| function.execution_mode() != FunctionExecutionMode::Jit)
    {
        return Ok(());
    }
    if attach_ready_clif_direct_entry_raw(py, func)? {
        return Ok(());
    }
    let start = Instant::now();
    let compile_result = unsafe {
        compile_clif_vectorcall(func.as_ptr()).map_err(|_| {
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

fn register_jit_vectorcall(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    function_id: RuntimeFunctionId,
    module_runtime: &ModuleRuntimeContext,
) -> PyResult<()> {
    let owned_runtime = unsafe { clone_module_runtime_context(module_runtime) }.map_err(|_| {
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
            "failed to register CLIF vectorcall for {module_name} function_id={function_id}: {err}",
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

fn module_runtime_from_shared_state(
    compile_session: Arc<CompileSession>,
    shared_state: Arc<SharedModuleState>,
    module_globals: &Bound<'_, PyAny>,
) -> ModuleRuntimeContext {
    unsafe { ffi::Py_INCREF(module_globals.as_ptr()) };
    ModuleRuntimeContext {
        mod_ctx: ModuleJitContext {
            shared_module_state: Arc::as_ptr(&shared_state),
            globals_obj: module_globals.as_ptr().cast::<c_void>(),
        },
        compile_session,
        shared_module_state_owner: shared_state,
    }
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
    for index in 0..captures.len() {
        let item = unsafe { ffi::PyTuple_GetItem(captures.as_ptr(), index as ffi::Py_ssize_t) };
        if item.is_null() {
            return Err(PyErr::fetch(py));
        }
        let item = unsafe { Bound::from_borrowed_ptr(py, item) };
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
        let value = normalize_class_cell_capture(name.as_str(), value)?;
        closure_values.set_item(name.as_str(), &value)?;
        captured_names.push(name);
    }
    Ok((captured_names, closure_values))
}

fn normalize_class_cell_capture<'py>(
    name: &str,
    value: Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    if !matches!(name, "__class__" | "_dp_classcell") || !is_cell_object(value.as_ptr()) {
        return Ok(value);
    }
    let py = value.py();
    let cell_contents = match value.getattr("cell_contents") {
        Ok(cell_contents) => cell_contents,
        Err(err)
            if err
                .matches(py, py.get_type::<PyValueError>())
                .unwrap_or(false) =>
        {
            return Ok(value);
        }
        Err(err) => return Err(err),
    };
    if is_cell_object(cell_contents.as_ptr()) {
        Ok(cell_contents)
    } else {
        Ok(value)
    }
}

fn split_param_defaults<'py>(
    py: Python<'py>,
    function: &BlockPyFunction<BlockPyModuleShape>,
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

fn build_closure_shaped_entry<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    module_globals: &Bound<'py, PyAny>,
    qualname: &str,
    captured_names: &[String],
    captured_values: &Bound<'py, PyDict>,
    original_code: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    debug_assert!(!captured_names.is_empty());
    let generated_code;
    let original_code_matches_captures = match original_code {
        Some(code) => {
            let freevars_obj = code.getattr("co_freevars")?;
            let freevars = tuple_strings(py, freevars_obj.cast::<PyTuple>()?)?;
            freevars == captured_names
        }
        None => false,
    };
    let code = if original_code_matches_captures {
        original_code
            .expect("original code should exist after matching captured names")
            .clone()
    } else {
        let (is_async, is_generator) = match function.lowered_kind() {
            FunctionKind::Function => (false, false),
            FunctionKind::Coroutine => (true, false),
            FunctionKind::Generator => (false, true),
            FunctionKind::AsyncGenerator => (true, true),
        };
        generated_code = dp.getattr("code_with_freevars")?.call1((
            tuple_from_strings(py, captured_names)?,
            is_async,
            is_generator,
        ))?;
        generated_code
    };
    let freevars_obj = code.getattr("co_freevars")?;
    let freevars = freevars_obj.cast::<PyTuple>()?;
    let mut closure_cells = Vec::with_capacity(freevars.len());
    for name in tuple_strings(py, freevars)?.into_iter() {
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

pub fn instantiate_bb_function(
    py: Python<'_>,
    dp: &Bound<'_, PyModule>,
    module_name: &str,
    function: &BlockPyFunction<BlockPyModuleShape>,
    captures: &Bound<'_, PyAny>,
    param_defaults: &Bound<'_, PyAny>,
    module_globals: &Bound<'_, PyAny>,
    annotate_fn: &Bound<'_, PyAny>,
    module_runtime: &ModuleRuntimeContext,
) -> PyResult<Py<PyAny>> {
    let keep_source_runtime_helper = module_name == "soac.runtime"
        && module_runtime
            .shared_module_state_owner
            .lookup_original_code(function.function_id)
            .is_some();
    let entry = instantiate_closure_backed_entry(
        py,
        dp,
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
    update_function_metadata(
        py,
        &entry,
        function.names.qualname.as_str(),
        function.names.display_name.as_str(),
        function.doc.as_deref(),
        annotate_fn,
    )?;
    entry.setattr("__module__", module_name)?;
    // soac.runtime's source helpers are the runtime ABI for other transformed
    // modules. Keep them on their source implementation so calls from generated
    // code do not implicitly replace their vectorcall entry. Still attach full
    // direct-call metadata so profiled generated code can explicitly compile
    // and call runtime class methods such as range.__iter__ and IterRange.__next__.
    if keep_source_runtime_helper {
        let owned_runtime =
            unsafe { clone_module_runtime_context(module_runtime) }.map_err(|_| {
                if unsafe { ffi::PyErr_Occurred() }.is_null() {
                    PyRuntimeError::new_err("failed to clone module runtime context")
                } else {
                    PyErr::fetch(py)
                }
            })?;
        unsafe {
            crate::register_clif_direct_metadata(
                entry.as_ptr(),
                function.function_id,
                owned_runtime,
            )
        }
        .map_err(|()| PyErr::fetch(py))?;
    } else {
        register_jit_vectorcall(py, &entry, function.function_id, module_runtime)?;
    }
    Ok(entry.unbind())
}

fn instantiate_closure_backed_entry<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    captures: &Bound<'py, PyAny>,
    module_globals: &Bound<'py, PyAny>,
    module_runtime: &ModuleRuntimeContext,
    entry_name: &str,
    qualname: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let (captured_names, closure_values) = build_capture_map(py, captures)?;
    let original_code = module_runtime
        .shared_module_state_owner
        .lookup_original_code(function.function_id)
        .map(|code| code.bind(py));
    let entry = if captured_names.is_empty() {
        let original_code_without_freevars = match original_code.as_ref() {
            Some(code) => {
                let freevars_obj = code.getattr("co_freevars")?;
                let freevars = freevars_obj.cast::<PyTuple>()?;
                freevars.is_empty().then_some(code.as_any())
            }
            None => None,
        };
        make_lazy_clif_entry(
            py,
            dp,
            entry_name,
            module_globals,
            original_code_without_freevars,
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
    Ok(entry)
}

pub fn function_kind_name(kind: FunctionKind) -> &'static str {
    match kind {
        FunctionKind::Function => "function",
        FunctionKind::Coroutine => "coroutine",
        FunctionKind::Generator => "generator",
        FunctionKind::AsyncGenerator => "async_generator",
    }
}

pub(crate) fn make_function_kind_abi_tag(kind: FunctionKind) -> i64 {
    match kind {
        FunctionKind::Function => 0,
        FunctionKind::Coroutine => 1,
        FunctionKind::Generator => 2,
        FunctionKind::AsyncGenerator => 3,
    }
}

fn function_kind_from_abi_tag(tag: i64) -> Option<FunctionKind> {
    match tag {
        0 => Some(FunctionKind::Function),
        1 => Some(FunctionKind::Coroutine),
        2 => Some(FunctionKind::Generator),
        3 => Some(FunctionKind::AsyncGenerator),
        _ => None,
    }
}

fn function_kind_from_name(name: &str) -> Option<FunctionKind> {
    match name {
        "function" => Some(FunctionKind::Function),
        "coroutine" => Some(FunctionKind::Coroutine),
        "generator" => Some(FunctionKind::Generator),
        "async_generator" => Some(FunctionKind::AsyncGenerator),
        _ => None,
    }
}

fn mark_coroutine_function(py: Python<'_>, func: &Bound<'_, PyAny>) -> PyResult<()> {
    let coroutines = PyModule::import(py, "asyncio.coroutines")?;
    let marker = coroutines.getattr("_is_coroutine")?;
    func.setattr("_is_coroutine", marker)
}

fn lookup_shared_function_template(
    compile_session: &Arc<CompileSession>,
    function_id: RuntimeFunctionId,
) -> PyResult<(Arc<SharedModuleState>, Arc<FunctionInstantiationTemplate>)> {
    let shared_state = compile_session
        .shared_module_state_for_function_id(function_id)
        .map_err(PyRuntimeError::new_err)?
        .ok_or_else(|| {
            PyRuntimeError::new_err(format!(
                "JIT basic-block function instantiation failed to resolve static function metadata for fn#{function_id}"
            ))
        })?;
    let template = shared_state
        .lookup_function_template(function_id)
        .map_err(PyRuntimeError::new_err)?
        .ok_or_else(|| {
            PyRuntimeError::new_err(format!(
                "JIT basic-block function instantiation failed to resolve static function metadata for fn#{function_id}"
            ))
        })?;
    Ok((shared_state, template))
}

fn instantiate_shared_function(
    py: Python<'_>,
    compile_session: Arc<CompileSession>,
    shared_state: Arc<SharedModuleState>,
    function_template: Arc<FunctionInstantiationTemplate>,
    expected_kind: FunctionKind,
    captures: &Bound<'_, PyAny>,
    param_defaults: &Bound<'_, PyAny>,
    annotate_fn: &Bound<'_, PyAny>,
    module_globals: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let function = function_template.function();
    if *function.lowered_kind() != expected_kind {
        return Err(PyRuntimeError::new_err(format!(
            "JIT basic-block function instantiation expected kind {:?} for fn#{}, got {:?}",
            function_kind_name(*function.lowered_kind()),
            function.function_id,
            function_kind_name(expected_kind)
        )));
    }
    module_globals.cast::<PyDict>().map_err(|_| {
        PyTypeError::new_err("JIT basic-block function instantiation requires module globals dict")
    })?;
    trace!(
        target: "soac_function_create",
        event = "soac.function_create",
        module_name = shared_state.module_name.as_str(),
        function_id = %function.function_id,
        function_qualname = function.names.qualname.as_str(),
        "make_function"
    );
    let dp = import_dp_module(py)?;
    let module_name = shared_state.module_name.clone();
    let module_runtime =
        module_runtime_from_shared_state(compile_session, shared_state, module_globals);
    let func = instantiate_bb_function(
        py,
        &dp,
        &module_name,
        function,
        captures,
        param_defaults,
        module_globals,
        annotate_fn,
        &module_runtime,
    )?;
    if *function.lowered_kind() == FunctionKind::Coroutine {
        mark_coroutine_function(py, func.bind(py))?;
    }
    Ok(func)
}

pub fn make_function(
    py: Python<'_>,
    function_id: RuntimeFunctionId,
    expected_kind: FunctionKind,
    captures: &Bound<'_, PyAny>,
    param_defaults: &Bound<'_, PyAny>,
    annotate_fn: &Bound<'_, PyAny>,
    module_globals: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let compile_session = CompileSession::process();
    let (shared_state, function_template) =
        lookup_shared_function_template(&compile_session, function_id)?;
    instantiate_shared_function(
        py,
        compile_session,
        shared_state,
        function_template,
        expected_kind,
        captures,
        param_defaults,
        annotate_fn,
        module_globals,
    )
}

pub(crate) fn make_function_in_shared_state(
    py: Python<'_>,
    compile_session: Arc<CompileSession>,
    shared_state: Arc<SharedModuleState>,
    function_id: RuntimeFunctionId,
    expected_kind: FunctionKind,
    captures: &Bound<'_, PyAny>,
    param_defaults: &Bound<'_, PyAny>,
    annotate_fn: &Bound<'_, PyAny>,
    module_globals: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let function_template = shared_state
        .lookup_function_template(function_id)
        .map_err(PyRuntimeError::new_err)?
        .ok_or_else(|| {
            PyRuntimeError::new_err(format!(
                "JIT basic-block function instantiation failed to resolve explicit static function metadata for fn#{function_id}"
            ))
        })?;
    instantiate_shared_function(
        py,
        compile_session,
        shared_state,
        function_template,
        expected_kind,
        captures,
        param_defaults,
        annotate_fn,
        module_globals,
    )
}

pub fn make_function_from_python_args(
    py: Python<'_>,
    function_id: u64,
    kind: &str,
    captures: Py<PyAny>,
    param_defaults: Py<PyAny>,
    annotate_fn: Option<Py<PyAny>>,
    module_globals: Option<Py<PyAny>>,
) -> PyResult<Py<PyAny>> {
    let expected_kind = function_kind_from_name(kind).ok_or_else(|| {
        PyRuntimeError::new_err(format!(
            "JIT basic-block function instantiation got unknown kind {kind:?}"
        ))
    })?;
    let annotate_fn = annotate_fn.unwrap_or_else(|| py.None());
    let module_globals = module_globals.unwrap_or_else(|| py.None());
    make_function(
        py,
        RuntimeFunctionId::from_packed_runtime_u64(function_id),
        expected_kind,
        captures.bind(py).as_any(),
        param_defaults.bind(py).as_any(),
        annotate_fn.bind(py),
        module_globals.bind(py),
    )
}

fn set_runtime_error(message: impl Into<String>) {
    let message = message.into();
    if let Ok(c_message) = CString::new(message) {
        unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_message.as_ptr()) };
    } else {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"invalid make_function error message".as_ptr(),
            )
        };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_jit_make_function_with_closure(
    function_id: u64,
    kind_tag: i64,
    captures: *mut ffi::PyObject,
    param_defaults: *mut ffi::PyObject,
    annotate_fn: *mut ffi::PyObject,
    module_globals: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        let Some(expected_kind) = function_kind_from_abi_tag(kind_tag) else {
            set_runtime_error(format!(
                "JIT basic-block function instantiation got unknown ABI kind tag {kind_tag}"
            ));
            return std::ptr::null_mut();
        };
        if captures.is_null()
            || param_defaults.is_null()
            || annotate_fn.is_null()
            || module_globals.is_null()
        {
            set_runtime_error("JIT basic-block function instantiation got a null argument");
            return std::ptr::null_mut();
        }
        let py = Python::assume_attached();
        let captures = unsafe { Bound::from_borrowed_ptr(py, captures) };
        let param_defaults = unsafe { Bound::from_borrowed_ptr(py, param_defaults) };
        let annotate_fn = unsafe { Bound::from_borrowed_ptr(py, annotate_fn) };
        let module_globals = unsafe { Bound::from_borrowed_ptr(py, module_globals) };
        match make_function(
            py,
            RuntimeFunctionId::from_packed_runtime_u64(function_id),
            expected_kind,
            &captures,
            &param_defaults,
            &annotate_fn,
            &module_globals,
        ) {
            Ok(func) => func.into_ptr(),
            Err(err) => {
                err.restore(py);
                std::ptr::null_mut()
            }
        }
    })) {
        Ok(value) => value,
        Err(_) => {
            set_runtime_error("panic in soac_jit_make_function_with_closure");
            std::ptr::null_mut()
        }
    }
}
