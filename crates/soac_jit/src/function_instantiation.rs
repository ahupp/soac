use crate::jit::{
    FIRST_VALID_CPYTHON_FUNCTION_VERSION, ModuleJitContext, ModuleRuntimeContext,
    raw_py_code_freevar_count, raw_py_code_version,
};
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
use std::ffi::{CStr, CString, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, trace};

pub(crate) const SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL: &str =
    "soac_jit_make_function_with_closure";
const MAX_COUNTERED_SOURCE_RUNTIME_HELPER_BLOCKS: usize = 17;
unsafe extern "C" {
    static mut PyCell_Type: ffi::PyTypeObject;
    fn PyCell_New(obj: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn _PyFunction_SetVersion(func: *mut ffi::PyFunctionObject, version: u32);
}

pub(crate) fn is_cell_object(obj: *mut ffi::PyObject) -> bool {
    unsafe { !obj.is_null() && ffi::Py_TYPE(obj) == std::ptr::addr_of_mut!(PyCell_Type) }
}

#[repr(C)]
struct RawPyDictKeysPrefix {
    dk_refcnt: ffi::Py_ssize_t,
    dk_log2_size: u8,
    dk_log2_index_bytes: u8,
    dk_kind: u8,
}

unsafe fn runtime_lookup_dict_item(
    dict: *mut ffi::PyObject,
    key: &Bound<'_, PyString>,
    legacy_key: &'static CStr,
) -> *mut ffi::PyObject {
    if !dict.is_null() && unsafe { ffi::PyDict_CheckExact(dict) } != 0 {
        let keys =
            unsafe { (*dict.cast::<ffi::PyDictObject>()).ma_keys }.cast::<RawPyDictKeysPrefix>();
        if !keys.is_null() && unsafe { (*keys).dk_kind } != 0 {
            return unsafe { ffi::PyDict_GetItem(dict, key.as_ptr()) };
        }
    }

    // GENERAL dictionaries can contain arbitrary keys whose equality hooks observe the fresh
    // Unicode argument. Preserve the existing GetItemString identity and error suppression.
    unsafe { ffi::PyDict_GetItemString(dict, legacy_key.as_ptr()) }
}

fn import_dp_module<'py>(
    py: Python<'py>,
    function_template: &FunctionInstantiationTemplate,
) -> PyResult<Bound<'py, PyModule>> {
    let lookup_keys = prepared_runtime_lookup_keys(py, function_template)?;
    unsafe {
        let modules = ffi::PyImport_GetModuleDict();
        if !modules.is_null() {
            let runtime = runtime_lookup_dict_item(
                modules,
                lookup_keys.runtime_module.bind(py),
                c"soac.runtime",
            );
            if !runtime.is_null() {
                let runtime = Bound::from_borrowed_ptr(py, runtime);
                if let Ok(runtime) = runtime.cast_into::<PyModule>() {
                    return Ok(runtime);
                }
            }
        }
    }
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

fn maybe_attach_ready_clif_direct_entry(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    module_runtime: &ModuleRuntimeContext,
    function_id: RuntimeFunctionId,
) -> PyResult<()> {
    if module_runtime.shared_module_state_owner.module_name == "soac.runtime" {
        return Ok(());
    }
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
    let _ = attach_ready_clif_direct_entry_raw(py, func)?;
    Ok(())
}

fn maybe_eager_compile_clif_entry(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
    module_runtime: &ModuleRuntimeContext,
    function_id: RuntimeFunctionId,
) -> PyResult<()> {
    if module_runtime.shared_module_state_owner.module_name == "soac.runtime" {
        return Ok(());
    }
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
    has_prepared_code_metadata: bool,
) -> PyResult<()> {
    if !has_prepared_code_metadata {
        ignore_attr_or_type_error(py, func.setattr("__qualname__", qualname))?;
        ignore_attr_or_type_error(py, func.setattr("__name__", name))?;
        if func.cast::<PyFunction>().is_ok() {
            let code = func.getattr("__code__")?;
            let has_matching_name = code.getattr("co_name")?.eq(name)?;
            let has_matching_qualname = code.getattr("co_qualname")?.eq(qualname)?;
            if !has_matching_name || !has_matching_qualname {
                let kwargs = PyDict::new(py);
                kwargs.set_item("co_name", name)?;
                kwargs.set_item("co_qualname", qualname)?;
                if let Some(replaced) =
                    ignore_attr_or_value_error(py, code.call_method("replace", (), Some(&kwargs)))?
                {
                    ignore_attr_or_type_error(py, func.setattr("__code__", replaced))?;
                }
            }
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

fn unicode_equals_str(obj: &Bound<'_, PyAny>, expected: &str) -> PyResult<bool> {
    if unsafe { ffi::PyUnicode_Check(obj.as_ptr()) } == 0 {
        return Err(PyTypeError::new_err("expected capture name to be a string"));
    }
    let mut len = 0;
    let ptr = unsafe { ffi::PyUnicode_AsUTF8AndSize(obj.as_ptr(), &mut len) };
    if ptr.is_null() {
        return Err(PyErr::fetch(obj.py()));
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
    Ok(bytes == expected.as_bytes())
}

fn code_freevars_match_names(code: &Bound<'_, PyAny>, expected_names: &[String]) -> PyResult<bool> {
    let freevar_count = unsafe { raw_py_code_freevar_count(code.as_ptr()) };
    if usize::try_from(freevar_count).ok() != Some(expected_names.len()) {
        return Ok(false);
    }
    if expected_names.is_empty() {
        return Ok(true);
    }
    let freevars_obj = code.getattr("co_freevars")?;
    let freevars = freevars_obj.cast::<PyTuple>()?;
    debug_assert_eq!(freevars.len(), expected_names.len());
    for (index, expected_name) in expected_names.iter().enumerate() {
        let item = unsafe { ffi::PyTuple_GetItem(freevars.as_ptr(), index as ffi::Py_ssize_t) };
        if item.is_null() {
            return Err(PyErr::fetch(code.py()));
        }
        let item = unsafe { Bound::from_borrowed_ptr(code.py(), item) };
        if !unicode_equals_str(&item, expected_name)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn build_ordered_capture_values<'py>(
    py: Python<'py>,
    captures: &Bound<'py, PyAny>,
    expected_names: &[String],
) -> PyResult<Option<Vec<Bound<'py, PyAny>>>> {
    let captures = captures.cast::<PyTuple>().map_err(|_| {
        PyTypeError::new_err(format!(
            "bb captures must be a tuple, got {:?}",
            captures.get_type()
        ))
    })?;
    if captures.len() != expected_names.len() {
        return Ok(None);
    }
    let mut values = Vec::with_capacity(expected_names.len());
    for (index, expected_name) in expected_names.iter().enumerate() {
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
        let name = item.get_item(0)?;
        if !unicode_equals_str(&name, expected_name)? {
            return Ok(None);
        }
        let value = item.get_item(1)?;
        values.push(normalize_class_cell_capture(expected_name.as_str(), value)?);
    }
    Ok(Some(values))
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
    let mut kwdefaults = None;
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
            ParamKind::KwOnly => kwdefaults
                .get_or_insert_with(|| PyDict::new(py))
                .set_item(param.name.as_str(), &value)?,
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
    Ok((positional_defaults, kwdefaults))
}

fn make_lazy_clif_entry<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    function_name: &str,
    module_globals: &Bound<'py, PyAny>,
    original_code: Option<&Bound<'py, PyAny>>,
    has_prepared_code_metadata: bool,
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
        if !has_prepared_code_metadata {
            func.setattr("__name__", function_name)?;
        }
        Ok(func)
    }
}

struct InstantiatedEntry<'py> {
    entry: Bound<'py, PyAny>,
    has_prepared_code_metadata: bool,
}

pub(crate) struct PreparedSyntheticCode {
    runtime_module: usize,
    code_factory: Py<PyAny>,
    code: Py<PyAny>,
}

pub(crate) struct PreparedOriginalCode {
    code: Py<PyAny>,
    capture_layout_matches: bool,
    has_prepared_metadata: bool,
}

pub(crate) struct PreparedRuntimeLookupKeys {
    runtime_module: Py<PyString>,
    bootstrap_module: Py<PyString>,
    code_factory: Py<PyString>,
}

fn prepared_runtime_lookup_keys<'template>(
    py: Python<'_>,
    function_template: &'template FunctionInstantiationTemplate,
) -> PyResult<&'template PreparedRuntimeLookupKeys> {
    if let Some(prepared) = function_template.prepared_runtime_lookup_keys.get() {
        return Ok(prepared);
    }

    let intern = |name: &'static CStr| -> PyResult<Py<PyString>> {
        let value = unsafe { ffi::PyUnicode_InternFromString(name.as_ptr()) };
        let value = unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, value)? };
        Ok(value.cast_into::<PyString>()?.unbind())
    };
    let prepared = PreparedRuntimeLookupKeys {
        runtime_module: intern(c"soac.runtime")?,
        bootstrap_module: intern(c"soac.bootstrap")?,
        code_factory: intern(c"code_with_freevars")?,
    };

    // Unicode allocation can invoke Python-visible callbacks. Prepare outside OnceLock so
    // reentrant function creation cannot deadlock or expose partially initialized keys.
    let _ = function_template.prepared_runtime_lookup_keys.set(prepared);
    Ok(function_template
        .prepared_runtime_lookup_keys
        .get()
        .expect("prepared runtime lookup keys should be initialized"))
}

fn prepared_original_code_for_template<'template>(
    py: Python<'_>,
    function_template: &'template FunctionInstantiationTemplate,
    shared_state: &SharedModuleState,
) -> PyResult<Option<&'template PreparedOriginalCode>> {
    if let Some(prepared) = function_template.prepared_original_code.get() {
        return Ok(prepared.as_ref());
    }

    let prepared = match shared_state.lookup_original_code(function_template.function().function_id)
    {
        Some(code) => {
            let code = code.bind(py);
            let capture_layout_matches =
                code_freevars_match_names(code, function_template.capture_names())?;
            let has_prepared_metadata = if capture_layout_matches {
                let name = code.getattr("co_name")?;
                let qualname = code.getattr("co_qualname")?;
                unicode_equals_str(
                    &name,
                    function_template.function().names.display_name.as_str(),
                )? && unicode_equals_str(
                    &qualname,
                    function_template.function().names.qualname.as_str(),
                )?
            } else {
                false
            };
            Some(PreparedOriginalCode {
                code: code.clone().unbind(),
                capture_layout_matches,
                has_prepared_metadata,
            })
        }
        None => None,
    };

    // Preparing code metadata can invoke Python and must never run while a OnceLock
    // initialization lock is held: callbacks may re-enter this same function template.
    let _ = function_template.prepared_original_code.set(prepared);
    Ok(function_template
        .prepared_original_code
        .get()
        .expect("original code metadata should be initialized")
        .as_ref())
}

fn canonical_bootstrap_code_factory(
    factory: &Bound<'_, PyAny>,
    lookup_keys: &PreparedRuntimeLookupKeys,
) -> bool {
    unsafe {
        let modules = ffi::PyImport_GetModuleDict();
        if modules.is_null() {
            return false;
        }
        let bootstrap = runtime_lookup_dict_item(
            modules,
            lookup_keys.bootstrap_module.bind(factory.py()),
            c"soac.bootstrap",
        );
        if bootstrap.is_null()
            || ffi::Py_TYPE(bootstrap) != std::ptr::addr_of_mut!(ffi::PyModule_Type)
        {
            return false;
        }
        let globals = ffi::PyModule_GetDict(bootstrap);
        if globals.is_null() {
            return false;
        }
        let original = runtime_lookup_dict_item(
            globals,
            lookup_keys.code_factory.bind(factory.py()),
            c"code_with_freevars",
        );
        !original.is_null() && original == factory.as_ptr()
    }
}

fn synthetic_code_for_template<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    function_template: &FunctionInstantiationTemplate,
    captured_names: &[String],
) -> PyResult<(Bound<'py, PyAny>, bool)> {
    let factory = dp.getattr("code_with_freevars")?;
    let (is_async, is_generator) = match function.lowered_kind() {
        FunctionKind::Function => (false, false),
        FunctionKind::Coroutine => (true, false),
        FunctionKind::Generator => (false, true),
        FunctionKind::AsyncGenerator => (true, true),
    };

    let lookup_keys = prepared_runtime_lookup_keys(py, function_template)?;
    if !canonical_bootstrap_code_factory(&factory, lookup_keys) {
        let code = factory.call1((
            tuple_from_strings(py, captured_names)?,
            is_async,
            is_generator,
        ))?;
        return Ok((code, false));
    }

    if let Some(prepared) = function_template.prepared_synthetic_code.get() {
        if prepared.runtime_module == dp.as_ptr() as usize
            && prepared.code_factory.as_ptr() == factory.as_ptr()
        {
            return Ok((prepared.code.bind(py).clone(), true));
        }
        let code = factory.call1((
            tuple_from_strings(py, captured_names)?,
            is_async,
            is_generator,
        ))?;
        return Ok((code, false));
    }

    let code = factory.call1((
        tuple_from_strings(py, captured_names)?,
        is_async,
        is_generator,
    ))?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("co_name", function.names.display_name.as_str())?;
    kwargs.set_item("co_qualname", function.names.qualname.as_str())?;
    let code = code.call_method("replace", (), Some(&kwargs))?;
    let code_factory_ptr = factory.as_ptr();
    let prepared = PreparedSyntheticCode {
        runtime_module: dp.as_ptr() as usize,
        code_factory: factory.unbind(),
        code: code.clone().unbind(),
    };

    // Initializing the code invokes Python and can re-enter this same template through an audit
    // hook or a customized bootstrap cache. Never hold a OnceLock initialization lock there.
    if function_template
        .prepared_synthetic_code
        .set(prepared)
        .is_err()
        && let Some(prepared) = function_template.prepared_synthetic_code.get()
        && prepared.runtime_module == dp.as_ptr() as usize
        && prepared.code_factory.as_ptr() == code_factory_ptr
    {
        return Ok((prepared.code.bind(py).clone(), true));
    }
    Ok((code, true))
}

fn build_closure_shaped_entry_from_ordered_captures<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    function_template: &FunctionInstantiationTemplate,
    module_globals: &Bound<'py, PyAny>,
    qualname: &str,
    captured_names: &[String],
    captured_values: &[Bound<'py, PyAny>],
    original_code: Option<&PreparedOriginalCode>,
) -> PyResult<InstantiatedEntry<'py>> {
    debug_assert!(!captured_names.is_empty());
    debug_assert_eq!(captured_names.len(), captured_values.len());
    let (code, has_prepared_code_metadata) = if let Some(prepared) = original_code {
        if prepared.capture_layout_matches {
            (
                prepared.code.bind(py).clone(),
                prepared.has_prepared_metadata,
            )
        } else {
            synthetic_code_for_template(py, dp, function, function_template, captured_names)?
        }
    } else {
        synthetic_code_for_template(py, dp, function, function_template, captured_names)?
    };
    let mut closure_cells = Vec::with_capacity(captured_values.len());
    for value in captured_values {
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
    let qualname = (!has_prepared_code_metadata).then(|| PyString::new(py, qualname));
    let func = unsafe {
        let ptr = ffi::PyFunction_NewWithQualName(
            code.as_ptr(),
            module_globals.as_ptr(),
            qualname
                .as_ref()
                .map_or(std::ptr::null_mut(), |value| value.as_ptr()),
        );
        if ptr.is_null() {
            return Err(PyErr::fetch(py));
        }
        Bound::from_owned_ptr(py, ptr)
    };
    if unsafe { ffi::PyFunction_SetClosure(func.as_ptr(), closure.as_ptr()) } != 0 {
        return Err(PyErr::fetch(py));
    }
    Ok(InstantiatedEntry {
        entry: func.into_any(),
        has_prepared_code_metadata,
    })
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
) -> PyResult<InstantiatedEntry<'py>> {
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
    Ok(InstantiatedEntry {
        entry: func.into_any(),
        has_prepared_code_metadata: false,
    })
}

fn apply_function_defaults(
    func: &Bound<'_, PyAny>,
    positional_defaults: Option<&Bound<'_, PyTuple>>,
    kwdefaults: Option<&Bound<'_, PyDict>>,
) -> PyResult<()> {
    let raw_function = func.as_ptr().cast::<ffi::PyFunctionObject>();
    unsafe {
        if positional_defaults.is_some() && !(*raw_function).func_defaults.is_null() {
            return Err(PyRuntimeError::new_err(
                "new Python function already has positional defaults",
            ));
        }
        if kwdefaults.is_some() && !(*raw_function).func_kwdefaults.is_null() {
            return Err(PyRuntimeError::new_err(
                "new Python function already has keyword defaults",
            ));
        }
        if let Some(defaults) = positional_defaults {
            ffi::Py_INCREF(defaults.as_ptr());
            (*raw_function).func_defaults = defaults.as_ptr();
        }
        if let Some(defaults) = kwdefaults {
            ffi::Py_INCREF(defaults.as_ptr());
            (*raw_function).func_kwdefaults = defaults.as_ptr();
        }
    }
    Ok(())
}

fn restore_source_generator_cpython_function_version(
    py: Python<'_>,
    func: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let code = unsafe { ffi::PyFunction_GetCode(func.as_ptr()) };
    if code.is_null() {
        return Err(PyErr::fetch(py));
    }
    let version = unsafe { raw_py_code_version(code) };
    if version >= FIRST_VALID_CPYTHON_FUNCTION_VERSION {
        unsafe { _PyFunction_SetVersion(func.as_ptr().cast::<ffi::PyFunctionObject>(), version) };
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
    instantiate_bb_function_inner(
        py,
        dp,
        module_name,
        function,
        None,
        captures,
        param_defaults,
        module_globals,
        annotate_fn,
        module_runtime,
    )
}

fn instantiate_bb_function_with_template(
    py: Python<'_>,
    dp: &Bound<'_, PyModule>,
    module_name: &str,
    function_template: &FunctionInstantiationTemplate,
    captures: &Bound<'_, PyAny>,
    param_defaults: &Bound<'_, PyAny>,
    module_globals: &Bound<'_, PyAny>,
    annotate_fn: &Bound<'_, PyAny>,
    module_runtime: &ModuleRuntimeContext,
) -> PyResult<Py<PyAny>> {
    instantiate_bb_function_inner(
        py,
        dp,
        module_name,
        function_template.function(),
        Some(function_template),
        captures,
        param_defaults,
        module_globals,
        annotate_fn,
        module_runtime,
    )
}

fn instantiate_bb_function_inner(
    py: Python<'_>,
    dp: &Bound<'_, PyModule>,
    module_name: &str,
    function: &BlockPyFunction<BlockPyModuleShape>,
    function_template: Option<&FunctionInstantiationTemplate>,
    captures: &Bound<'_, PyAny>,
    param_defaults: &Bound<'_, PyAny>,
    module_globals: &Bound<'_, PyAny>,
    annotate_fn: &Bound<'_, PyAny>,
    module_runtime: &ModuleRuntimeContext,
) -> PyResult<Py<PyAny>> {
    let specialization_mode = module_runtime
        .compile_session
        .env_config()
        .map_err(PyRuntimeError::new_err)?
        .specialization_mode();
    let records_specialization_counters =
        specialization_mode.is_some_and(|mode| mode.records_counters());
    let has_original_runtime_code = module_runtime
        .shared_module_state_owner
        .lookup_original_code(function.function_id)
        .is_some();
    let keep_source_runtime_helper = keep_source_runtime_helper_vectorcall(
        module_name,
        function.blocks.len(),
        has_original_runtime_code,
        records_specialization_counters,
    );
    let keep_source_generator = keep_source_generator_vectorcall(
        function.lowered_kind(),
        function.names.display_name.as_str(),
        has_original_runtime_code,
        records_specialization_counters,
    );
    let instantiated_entry = instantiate_closure_backed_entry(
        py,
        dp,
        function,
        captures,
        module_globals,
        module_runtime,
        function_template,
        function.names.display_name.as_str(),
        function.names.qualname.as_str(),
    )?;
    let entry = instantiated_entry.entry;
    let (positional_defaults, kwdefaults) = split_param_defaults(py, function, param_defaults)?;
    apply_function_defaults(&entry, positional_defaults.as_ref(), kwdefaults.as_ref())?;
    update_function_metadata(
        py,
        &entry,
        function.names.qualname.as_str(),
        function.names.display_name.as_str(),
        function.doc.as_deref(),
        annotate_fn,
        instantiated_entry.has_prepared_code_metadata,
    )?;
    let existing_module = unsafe { (*entry.as_ptr().cast::<ffi::PyFunctionObject>()).func_module };
    let module_already_matches = !existing_module.is_null()
        && unsafe { ffi::PyUnicode_CheckExact(existing_module) } != 0
        && unicode_equals_str(
            &unsafe { Bound::from_borrowed_ptr(py, existing_module) },
            module_name,
        )?;
    if !module_already_matches {
        entry.setattr("__module__", module_name)?;
    }
    // soac.runtime's source helpers are the runtime ABI for other transformed
    // modules. Keep them on their source implementation outside countered
    // specialization runs so calls from generated code do not implicitly
    // replace their vectorcall entry. In profile/verify mode, small transformed
    // helper bodies can run so their own call-site counters feed later
    // cross-module inline decisions, while larger bootstrap helpers stay on the
    // source path.
    if keep_source_generator {
        trace!(
            module_name,
            function_id = %function.function_id,
            function_qualname = %function.names.qualname,
            "keeping source generator vectorcall"
        );
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
        maybe_attach_ready_clif_direct_entry(py, &entry, module_runtime, function.function_id)?;
        restore_source_generator_cpython_function_version(py, &entry)?;
    } else if keep_source_runtime_helper {
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
        maybe_attach_ready_clif_direct_entry(py, &entry, module_runtime, function.function_id)?;
    } else {
        register_jit_vectorcall(py, &entry, function.function_id, module_runtime)?;
    }
    Ok(entry.unbind())
}

fn keep_source_runtime_helper_vectorcall(
    module_name: &str,
    block_count: usize,
    has_original_runtime_code: bool,
    records_specialization_counters: bool,
) -> bool {
    if module_name != "soac.runtime" || !has_original_runtime_code {
        return false;
    }
    if !records_specialization_counters {
        return true;
    }
    block_count > MAX_COUNTERED_SOURCE_RUNTIME_HELPER_BLOCKS
}

fn keep_source_generator_vectorcall(
    function_kind: &FunctionKind,
    display_name: &str,
    has_original_runtime_code: bool,
    records_specialization_counters: bool,
) -> bool {
    has_original_runtime_code
        && *function_kind == FunctionKind::Generator
        && display_name != "<genexpr>"
        && !records_specialization_counters
}

fn instantiate_closure_backed_entry<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    captures: &Bound<'py, PyAny>,
    module_globals: &Bound<'py, PyAny>,
    module_runtime: &ModuleRuntimeContext,
    function_template: Option<&FunctionInstantiationTemplate>,
    entry_name: &str,
    qualname: &str,
) -> PyResult<InstantiatedEntry<'py>> {
    let prepared_original_code = function_template
        .map(|template| {
            prepared_original_code_for_template(
                py,
                template,
                module_runtime.shared_module_state_owner.as_ref(),
            )
        })
        .transpose()?
        .flatten();
    let original_code = if function_template.is_some() {
        prepared_original_code.map(|prepared| prepared.code.bind(py))
    } else {
        module_runtime
            .shared_module_state_owner
            .lookup_original_code(function.function_id)
            .map(|code| code.bind(py))
    };
    if let Some(function_template) = function_template {
        let captured_names = function_template.capture_names();
        if let Some(captured_values) = build_ordered_capture_values(py, captures, captured_names)? {
            if captured_names.is_empty() {
                let prepared_original_code =
                    prepared_original_code.filter(|prepared| prepared.capture_layout_matches);
                let original_code_without_freevars =
                    prepared_original_code.map(|prepared| prepared.code.bind(py).as_any());
                let has_prepared_code_metadata =
                    prepared_original_code.is_some_and(|prepared| prepared.has_prepared_metadata);
                let entry = make_lazy_clif_entry(
                    py,
                    dp,
                    entry_name,
                    module_globals,
                    original_code_without_freevars,
                    has_prepared_code_metadata,
                )?;
                return Ok(InstantiatedEntry {
                    entry,
                    has_prepared_code_metadata,
                });
            } else {
                return build_closure_shaped_entry_from_ordered_captures(
                    py,
                    dp,
                    function,
                    function_template,
                    module_globals,
                    qualname,
                    captured_names,
                    captured_values.as_slice(),
                    prepared_original_code,
                );
            }
        }
    }

    let (captured_names, closure_values) = build_capture_map(py, captures)?;
    if captured_names.is_empty() {
        let original_code_without_freevars = match original_code.as_ref() {
            Some(code) if code_freevars_match_names(code.as_any(), &[])? => Some(code.as_any()),
            None => None,
            Some(_) => None,
        };
        let entry = make_lazy_clif_entry(
            py,
            dp,
            entry_name,
            module_globals,
            original_code_without_freevars,
            false,
        )?;
        return Ok(InstantiatedEntry {
            entry,
            has_prepared_code_metadata: false,
        });
    } else {
        return build_closure_shaped_entry(
            py,
            dp,
            function,
            module_globals,
            qualname,
            &captured_names,
            &closure_values,
            original_code.as_ref().map(|code| code.as_any()),
        );
    }
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

pub(crate) fn lookup_shared_function_template(
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
    let dp = import_dp_module(py, function_template.as_ref())?;
    let module_name = shared_state.module_name.clone();
    let module_runtime =
        module_runtime_from_shared_state(compile_session, shared_state, module_globals);
    let func = instantiate_bb_function_with_template(
        py,
        &dp,
        &module_name,
        function_template.as_ref(),
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

#[cfg(test)]
mod tests {
    use super::{
        MAX_COUNTERED_SOURCE_RUNTIME_HELPER_BLOCKS, RawPyDictKeysPrefix,
        canonical_bootstrap_code_factory, code_freevars_match_names, import_dp_module,
        keep_source_generator_vectorcall, keep_source_runtime_helper_vectorcall,
        make_lazy_clif_entry, prepared_runtime_lookup_keys, runtime_lookup_dict_item,
    };
    use pyo3::ffi;
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyModule};
    use soac_core::block_py::FunctionKind;

    #[test]
    fn runtime_module_lookup_reuses_interned_template_keys_without_retaining_modules() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();

        Python::attach(|py| {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                "def source_function():\n    return 1\n",
            )
            .expect("runtime lookup fixture should lower")
            .blockpy_module;
            let source_function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.qualname == "source_function")
                .expect("runtime lookup fixture should contain its source function");
            let template = crate::FunctionInstantiationTemplate::from_function(source_function)
                .expect("runtime lookup fixture should prepare its immutable function template");
            let modules = py
                .import("sys")
                .and_then(|sys| sys.getattr("modules"))
                .and_then(|modules| modules.cast_into::<PyDict>().map_err(Into::into))
                .expect("the interpreter should expose its real module dictionary");
            let original_runtime = modules
                .get_item("soac.runtime")
                .expect("runtime module lookup should succeed");
            let first = PyModule::new(py, "runtime_lookup_first")
                .expect("the first replacement runtime should allocate");
            let second = PyModule::new(py, "runtime_lookup_second")
                .expect("the second replacement runtime should allocate");

            let imported = (|| -> PyResult<(usize, usize)> {
                modules.set_item("soac.runtime", &first)?;
                let first_import = import_dp_module(py, &template)?;
                let first_ptr = first_import.as_ptr() as usize;
                drop(first_import);

                modules.set_item("soac.runtime", &second)?;
                let second_import = import_dp_module(py, &template)?;
                let second_ptr = second_import.as_ptr() as usize;
                drop(second_import);
                Ok((first_ptr, second_ptr))
            })();
            match original_runtime {
                Some(original_runtime) => modules
                    .set_item("soac.runtime", original_runtime)
                    .expect("the original runtime module should be restored"),
                None => modules
                    .del_item("soac.runtime")
                    .expect("the temporary runtime module should be removed"),
            }
            let (first_import, second_import) =
                imported.expect("both actual production module lookups should succeed");
            assert_eq!(first_import, first.as_ptr() as usize);
            assert_eq!(second_import, second.as_ptr() as usize);

            let prepared = template
                .prepared_runtime_lookup_keys
                .get()
                .expect("production runtime lookup must prepare reusable interned Unicode keys");
            for (name, actual) in [
                ("soac.runtime", prepared.runtime_module.bind(py)),
                ("soac.bootstrap", prepared.bootstrap_module.bind(py)),
                ("code_with_freevars", prepared.code_factory.bind(py)),
            ] {
                let expected = py
                    .import("sys")
                    .and_then(|sys| sys.call_method1("intern", (name,)))
                    .expect("the expected runtime lookup key should intern");
                assert_eq!(
                    actual.as_ptr(),
                    expected.as_ptr(),
                    "the {name} lookup key must reuse CPython's exact interned Unicode object"
                );
            }

            let builtins = py.import("builtins").expect("builtins should import");
            let original_factory = builtins
                .getattr("len")
                .expect("the first fixture factory should exist");
            let replacement_factory = builtins
                .getattr("repr")
                .expect("the replacement fixture factory should exist");
            let first_bootstrap = PyModule::new(py, "runtime_lookup_bootstrap_first")
                .expect("the first replacement bootstrap should allocate");
            let second_bootstrap = PyModule::new(py, "runtime_lookup_bootstrap_second")
                .expect("the second replacement bootstrap should allocate");
            first_bootstrap
                .setattr("code_with_freevars", &original_factory)
                .expect("the first bootstrap should accept its factory");
            second_bootstrap
                .setattr("code_with_freevars", &replacement_factory)
                .expect("the second bootstrap should accept its factory");
            let original_bootstrap = modules
                .get_item("soac.bootstrap")
                .expect("bootstrap module lookup should succeed");
            let factory_matches = (|| -> PyResult<(bool, bool, bool)> {
                modules.set_item("soac.bootstrap", &first_bootstrap)?;
                let initial = canonical_bootstrap_code_factory(&original_factory, prepared);
                first_bootstrap.setattr("code_with_freevars", &replacement_factory)?;
                let replaced_factory =
                    canonical_bootstrap_code_factory(&original_factory, prepared);
                modules.set_item("soac.bootstrap", &second_bootstrap)?;
                let replaced_module =
                    canonical_bootstrap_code_factory(&replacement_factory, prepared);
                Ok((initial, replaced_factory, replaced_module))
            })();
            match original_bootstrap {
                Some(original_bootstrap) => modules
                    .set_item("soac.bootstrap", original_bootstrap)
                    .expect("the original bootstrap module should be restored"),
                None => modules
                    .del_item("soac.bootstrap")
                    .expect("the temporary bootstrap module should be removed"),
            }
            assert_eq!(
                factory_matches.expect("actual canonical factory probes should succeed"),
                (true, false, true),
                "cached names must reread the current bootstrap module and factory on every call"
            );

            let weak_first = py
                .import("weakref")
                .and_then(|weakref| weakref.call_method1("ref", (&first,)))
                .expect("runtime modules should support weak references");
            drop(first);
            assert!(
                weak_first
                    .call0()
                    .expect("the runtime module weak reference should be callable")
                    .is_none(),
                "prepared lookup keys must not retain replaced runtime modules"
            );
        });
    }

    #[test]
    fn runtime_module_lookup_preserves_general_dict_collision_identity_and_error_suppression() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();

        Python::attach(|py| {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                "def source_function():\n    return 1\n",
            )
            .expect("collision lookup fixture should lower")
            .blockpy_module;
            let source_function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.qualname == "source_function")
                .expect("collision lookup fixture should contain its source function");
            let template = crate::FunctionInstantiationTemplate::from_function(source_function)
                .expect("collision lookup fixture should prepare its immutable function template");
            let prepared = prepared_runtime_lookup_keys(py, &template)
                .expect("collision lookup fixture should prepare its interned names");
            let namespace = PyDict::new(py);
            py.import("builtins")
                .and_then(|builtins| builtins.getattr("exec"))
                .and_then(|exec| {
                    exec.call1((
                        r#"
class CollisionKey:
    def __init__(self, target):
        self.target = target
        self.identities = []
        self.raise_error = False

    def __hash__(self):
        return hash(self.target)

    def __eq__(self, other):
        self.identities.append(other is self.target)
        if self.raise_error:
            raise RuntimeError("runtime lookup collision")
        return False

class DictSubclass(dict):
    pass

unraisable_errors = []

def observe_unraisable(error):
    unraisable_errors.append(error.exc_type.__name__)
"#,
                        &namespace,
                    ))
                })
                .expect("collision lookup fixture should define its adversarial Python objects");
            let collision_class = namespace
                .get_item("CollisionKey")
                .expect("collision class lookup should succeed")
                .expect("collision class should exist");
            let collision = collision_class
                .call1((prepared.runtime_module.bind(py),))
                .expect("collision key should allocate");
            let dict = PyDict::new(py);
            dict.set_item(&collision, py.None())
                .expect("collision key should enter the fixture dictionary first");
            let value = PyModule::new(py, "runtime_lookup_collision_value")
                .expect("collision fixture value should allocate");
            dict.set_item(prepared.runtime_module.bind(py), &value)
                .expect("the actual Unicode key should enter the fixture dictionary");
            collision
                .getattr("identities")
                .and_then(|identities| identities.call_method0("clear"))
                .expect("fixture insertion observations should clear");

            let keys = unsafe { (*dict.as_ptr().cast::<ffi::PyDictObject>()).ma_keys }
                .cast::<RawPyDictKeysPrefix>();
            assert_eq!(
                unsafe { (*keys).dk_kind },
                0,
                "a non-Unicode collision key must force CPython's GENERAL dictionary layout"
            );
            let found = unsafe {
                runtime_lookup_dict_item(
                    dict.as_ptr(),
                    prepared.runtime_module.bind(py),
                    c"soac.runtime",
                )
            };
            assert_eq!(found, value.as_ptr());
            assert_eq!(
                collision
                    .getattr("identities")
                    .and_then(|identities| identities.extract::<Vec<bool>>())
                    .expect("collision identity observations should extract"),
                vec![false],
                "GENERAL dictionaries must receive the original freshly allocated Unicode key"
            );

            let subclass = namespace
                .get_item("DictSubclass")
                .expect("dictionary subclass lookup should succeed")
                .expect("dictionary subclass should exist")
                .call0()
                .expect("dictionary subclass should allocate");
            subclass
                .call_method1("update", (&dict,))
                .expect("dictionary subclass should copy its collision keys");
            collision
                .getattr("identities")
                .and_then(|identities| identities.call_method0("clear"))
                .expect("subclass setup observations should clear");
            let subclass_found = unsafe {
                runtime_lookup_dict_item(
                    subclass.as_ptr(),
                    prepared.runtime_module.bind(py),
                    c"soac.runtime",
                )
            };
            assert_eq!(subclass_found, value.as_ptr());
            assert_eq!(
                collision
                    .getattr("identities")
                    .and_then(|identities| identities.extract::<Vec<bool>>())
                    .expect("subclass collision identity observations should extract"),
                vec![false],
                "dictionary subclasses must retain the original fresh-key lookup"
            );

            collision
                .setattr("raise_error", true)
                .expect("collision fixture should enable its raising equality hook");
            let sys = py.import("sys").expect("sys should import");
            let original_unraisable_hook = sys
                .getattr("unraisablehook")
                .expect("the existing unraisable hook should exist");
            let replacement_unraisable_hook = namespace
                .get_item("observe_unraisable")
                .expect("replacement unraisable hook lookup should succeed")
                .expect("replacement unraisable hook should exist");
            sys.setattr("unraisablehook", replacement_unraisable_hook)
                .expect("the temporary unraisable hook should install");
            let failed = unsafe {
                runtime_lookup_dict_item(
                    dict.as_ptr(),
                    prepared.runtime_module.bind(py),
                    c"soac.runtime",
                )
            };
            let raised = unsafe { ffi::PyErr_Occurred() };
            sys.setattr("unraisablehook", original_unraisable_hook)
                .expect("the original unraisable hook should restore");
            assert!(failed.is_null());
            assert!(
                raised.is_null(),
                "legacy PyDict_GetItemString must keep suppressing collision errors"
            );
            assert_eq!(
                namespace
                    .get_item("unraisable_errors")
                    .expect("unraisable observations lookup should succeed")
                    .expect("unraisable observations should exist")
                    .extract::<Vec<String>>()
                    .expect("unraisable observations should extract"),
                vec!["RuntimeError"],
                "the legacy lookup must preserve CPython's existing unraisable-hook behavior"
            );
        });
    }

    #[test]
    fn source_backed_zero_freevar_function_preserves_code_metadata_identity() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();

        Python::attach(|py| {
            let globals = PyDict::new(py);
            globals
                .set_item("__name__", "source_function_template_structured")
                .expect("source globals should contain a module name");
            PyModule::import(py, "builtins")
                .and_then(|builtins| builtins.getattr("exec"))
                .and_then(|exec| exec.call1(("def original(value):\n    return value\n", &globals)))
                .expect("source function should compile");
            let source_function = globals
                .get_item("original")
                .expect("source function lookup should succeed")
                .expect("source function should exist");
            let source_code = source_function
                .getattr("__code__")
                .expect("source function should expose its immutable code");
            assert!(
                code_freevars_match_names(&source_code, &[])
                    .expect("source code should expose its freevar layout")
            );

            let runtime = PyModule::new(py, "unused_source_function_runtime")
                .expect("unused runtime module should allocate");
            let instantiated = make_lazy_clif_entry(
                py,
                &runtime,
                "original",
                globals.as_any(),
                Some(source_code.as_any()),
                true,
            )
            .expect("source-backed function should instantiate");
            let function_name = instantiated
                .getattr("__name__")
                .expect("instantiated function should expose its name");
            let code_name = source_code
                .getattr("co_name")
                .expect("original source code should expose its name");
            assert_eq!(
                function_name.as_ptr(),
                code_name.as_ptr(),
                "a source-backed function must reuse its immutable code name"
            );
        });
    }

    #[test]
    fn source_runtime_helpers_keep_cpython_vectorcall_outside_counter_modes() {
        assert!(keep_source_runtime_helper_vectorcall(
            "soac.runtime",
            1,
            true,
            false
        ));
    }

    #[test]
    fn small_source_runtime_helpers_use_transformed_vectorcall_in_counter_modes() {
        assert!(!keep_source_runtime_helper_vectorcall(
            "soac.runtime",
            MAX_COUNTERED_SOURCE_RUNTIME_HELPER_BLOCKS,
            true,
            true
        ));
    }

    #[test]
    fn large_source_runtime_helpers_keep_cpython_vectorcall_in_counter_modes() {
        assert!(keep_source_runtime_helper_vectorcall(
            "soac.runtime",
            MAX_COUNTERED_SOURCE_RUNTIME_HELPER_BLOCKS + 1,
            true,
            true
        ));
    }

    #[test]
    fn source_runtime_helper_policy_only_applies_to_original_runtime_code() {
        assert!(!keep_source_runtime_helper_vectorcall(
            "other.module",
            1,
            true,
            false
        ));
        assert!(!keep_source_runtime_helper_vectorcall(
            "soac.runtime",
            1,
            false,
            false
        ));
    }

    #[test]
    fn source_named_generators_keep_cpython_vectorcall_outside_counter_modes() {
        assert!(keep_source_generator_vectorcall(
            &FunctionKind::Generator,
            "items",
            true,
            false,
        ));
    }

    #[test]
    fn source_named_generators_use_transformed_vectorcall_in_counter_modes() {
        assert!(!keep_source_generator_vectorcall(
            &FunctionKind::Generator,
            "items",
            true,
            true,
        ));
    }

    #[test]
    fn source_generator_expressions_always_use_transformed_vectorcall() {
        assert!(!keep_source_generator_vectorcall(
            &FunctionKind::Generator,
            "<genexpr>",
            true,
            false,
        ));
        assert!(!keep_source_generator_vectorcall(
            &FunctionKind::Generator,
            "<genexpr>",
            true,
            true,
        ));
    }

    #[test]
    fn generated_or_non_generator_functions_still_use_transformed_vectorcall() {
        assert!(!keep_source_generator_vectorcall(
            &FunctionKind::Generator,
            "items",
            false,
            false,
        ));
        assert!(!keep_source_generator_vectorcall(
            &FunctionKind::Function,
            "items",
            true,
            false,
        ));
    }
}
