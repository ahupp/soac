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
    function_template: Option<Arc<FunctionInstantiationTemplate>>,
) -> PyResult<()> {
    unsafe {
        crate::register_clif_vectorcall_with_template(
            func.as_ptr(),
            function_id,
            module_runtime,
            function_template,
        )
        .map_err(|_| {
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
    function_template: Option<&FunctionInstantiationTemplate>,
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
    let is_non_jit_function = function_template.map_or_else(
        || {
            module_runtime
                .shared_module_state_owner
                .lookup_function(function_id)
                .is_some_and(|function| function.execution_mode() != FunctionExecutionMode::Jit)
        },
        |template| template.function().execution_mode() != FunctionExecutionMode::Jit,
    );
    if is_non_jit_function {
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
    function_template: Option<&Arc<FunctionInstantiationTemplate>>,
) -> PyResult<()> {
    prepare_bootstrap_factory_origin(py, module_runtime, function_id)?;
    let owned_runtime = unsafe { clone_module_runtime_context(module_runtime) }.map_err(|_| {
        if unsafe { ffi::PyErr_Occurred() }.is_null() {
            PyRuntimeError::new_err("failed to clone module runtime context")
        } else {
            PyErr::fetch(py)
        }
    })?;
    match register_clif_vectorcall_raw(
        py,
        func,
        function_id,
        owned_runtime,
        function_template.cloned(),
    ) {
        Ok(()) => maybe_eager_compile_clif_entry(
            py,
            func,
            module_runtime,
            function_id,
            function_template.map(Arc::as_ref),
        ),
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
    runtime_owner_type: usize,
    runtime_owner_type_version: u32,
    code_factory: Py<PyAny>,
    code: Py<PyAny>,
}

pub(crate) struct PreparedOriginalCode {
    code: Py<PyAny>,
    capture_layout_matches: bool,
    has_prepared_metadata: bool,
}

pub(crate) struct PreparedBootstrapFactoryOrigin {
    code: Py<PyAny>,
    cache: Py<PyAny>,
    cache_key: Py<PyString>,
    builtins_key: Py<PyString>,
}

pub(crate) struct PreparedEagerComprehension {
    capsule: Py<PyAny>,
    compile_session_id: crate::CompileSessionId,
    runtime_module: usize,
    runtime_owner: usize,
    runtime_owner_version: u32,
    factory: usize,
    parent_code: usize,
    origin_code: usize,
    origin_cache: usize,
    cache_key: usize,
    builtins_key: usize,
}

const MAX_EAGER_COMPREHENSION_CAPTURES: usize = 8;
const EAGER_COMPREHENSION_CAPSULE_NAME: &CStr = c"soac.eager_comprehension_direct_entry";

struct EagerComprehensionDirectEntry {
    method: ffi::PyMethodDef,
    _compiled_function: Arc<crate::jit::CompiledFunctionHandle>,
    direct_code_ptr: *const u8,
    default_direct_code_ptr: *const u8,
    deopt_table_ptr: *const c_void,
    late_bound_owner_cells: *const crate::module_type::LateBoundOwnerFieldCell,
    closure_slots: [usize; MAX_EAGER_COMPREHENSION_CAPTURES],
    closure_count: usize,
}

#[repr(C)]
struct EagerComprehensionStackEnv {
    header: crate::FunctionEnvAbiHeader,
    runtime_objects: [*mut ffi::PyObject; MAX_EAGER_COMPREHENSION_CAPTURES + 1],
}

pub(crate) struct PreparedRuntimeLookupKeys {
    runtime_module: Py<PyString>,
    bootstrap_module: Py<PyString>,
    code_factory: Py<PyString>,
}

fn intern_eager_runtime_name(py: Python<'_>, name: &'static CStr) -> PyResult<Py<PyString>> {
    let value = unsafe { ffi::PyUnicode_InternFromString(name.as_ptr()) };
    let value = unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, value)? };
    Ok(value.cast_into::<PyString>()?.unbind())
}

unsafe fn eager_unicode_dict_item(
    dict: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    if dict.is_null() || unsafe { ffi::PyDict_CheckExact(dict) } == 0 {
        return std::ptr::null_mut();
    }
    let keys = unsafe { (*dict.cast::<ffi::PyDictObject>()).ma_keys }.cast::<RawPyDictKeysPrefix>();
    if keys.is_null() || unsafe { (*keys).dk_kind } == 0 {
        return std::ptr::null_mut();
    }
    unsafe { ffi::PyDict_GetItem(dict, key) }
}

fn prepare_bootstrap_factory_origin(
    py: Python<'_>,
    module_runtime: &ModuleRuntimeContext,
    function_id: RuntimeFunctionId,
) -> PyResult<()> {
    if module_runtime.shared_module_state_owner.module_name != "soac.runtime" {
        return Ok(());
    }
    let Some(function) = module_runtime
        .shared_module_state_owner
        .lookup_function(function_id)
    else {
        return Ok(());
    };
    if function.names.bind_name != "_dp_module_init" {
        return Ok(());
    }
    let Some(template) = module_runtime
        .shared_module_state_owner
        .lookup_function_template(function_id)
        .map_err(PyRuntimeError::new_err)?
    else {
        return Ok(());
    };
    if template.prepared_bootstrap_factory_origin.get().is_some() {
        return Ok(());
    }

    let lookup_keys = prepared_runtime_lookup_keys(py, template.as_ref())?;
    let globals = module_runtime.mod_ctx.globals_obj.cast::<ffi::PyObject>();
    let factory =
        unsafe { eager_unicode_dict_item(globals, lookup_keys.code_factory.bind(py).as_ptr()) };
    let modules = unsafe { ffi::PyImport_GetModuleDict() };
    let bootstrap =
        unsafe { eager_unicode_dict_item(modules, lookup_keys.bootstrap_module.bind(py).as_ptr()) };
    if factory.is_null()
        || unsafe { ffi::PyFunction_Check(factory) } == 0
        || bootstrap.is_null()
        || unsafe { ffi::PyModule_CheckExact(bootstrap) } == 0
    {
        return Ok(());
    }
    let bootstrap_globals = unsafe { ffi::PyModule_GetDict(bootstrap) };
    if unsafe {
        eager_unicode_dict_item(
            bootstrap_globals,
            lookup_keys.code_factory.bind(py).as_ptr(),
        )
    } != factory
    {
        return Ok(());
    }
    let raw_factory = factory.cast::<ffi::PyFunctionObject>();
    let factory_code = unsafe { (*raw_factory).func_code };
    if factory_code.is_null()
        || unsafe { (*raw_factory).func_globals } != bootstrap_globals
        || !unsafe { (*raw_factory).func_defaults }.is_null()
        || !unsafe { (*raw_factory).func_kwdefaults }.is_null()
        || !unsafe { (*raw_factory).func_closure }.is_null()
    {
        return Ok(());
    }
    let code = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, factory_code) };
    if !unicode_equals_str(&code.getattr("co_name")?, "code_with_freevars")?
        || !unicode_equals_str(&code.getattr("co_qualname")?, "code_with_freevars")?
    {
        return Ok(());
    }
    let code_filename = code.getattr("co_filename")?;
    let bootstrap_filename =
        unsafe { ffi::PyDict_GetItemString(bootstrap_globals, c"__file__".as_ptr()) };
    if bootstrap_filename.is_null()
        || unsafe { ffi::PyUnicode_CheckExact(bootstrap_filename) } == 0
        || unsafe { ffi::PyUnicode_CheckExact(code_filename.as_ptr()) } == 0
        || unsafe { ffi::PyUnicode_Compare(code_filename.as_ptr(), bootstrap_filename) } != 0
    {
        return Ok(());
    }

    let cache_key = intern_eager_runtime_name(py, c"_DP_CODE_WITH_FREEVARS_CACHE")?;
    let builtins_key = intern_eager_runtime_name(py, c"__builtins__")?;
    let cache = unsafe { eager_unicode_dict_item(bootstrap_globals, cache_key.bind(py).as_ptr()) };
    if cache.is_null() || unsafe { ffi::PyDict_CheckExact(cache) } == 0 {
        return Ok(());
    }
    let origin = PreparedBootstrapFactoryOrigin {
        code: code.unbind(),
        cache: unsafe { Bound::<PyAny>::from_borrowed_ptr(py, cache) }.unbind(),
        cache_key,
        builtins_key,
    };
    // This runs while soac.runtime's module init is being registered, before its body or any
    // user module executes. Keep only the immutable code and compiler-private cache, not modules.
    let _ = template.prepared_bootstrap_factory_origin.set(origin);
    Ok(())
}

fn source_parent_code_for_eager_comprehension(
    shared_state: &SharedModuleState,
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> Option<*mut ffi::PyObject> {
    let mut qualname = function.names.qualname.as_str();
    while let Some((parent, _)) = qualname.rsplit_once(".<locals>.") {
        if let Some(code) = shared_state
            .lowered_module
            .callable_defs
            .iter()
            .find(|candidate| candidate.names.qualname == parent)
            .and_then(|candidate| shared_state.lookup_original_code(candidate.function_id))
        {
            return Some(code.as_ptr());
        }
        qualname = parent;
    }
    None
}

fn runtime_bootstrap_origin_template(
    compile_session: &CompileSession,
) -> PyResult<Option<Arc<FunctionInstantiationTemplate>>> {
    for shared_state in compile_session
        .shared_module_states_snapshot()
        .map_err(PyRuntimeError::new_err)?
    {
        if shared_state.module_name != "soac.runtime" {
            continue;
        }
        let Some(function) = shared_state
            .lowered_module
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "_dp_module_init")
        else {
            continue;
        };
        let Some(template) = shared_state
            .lookup_function_template(function.function_id)
            .map_err(PyRuntimeError::new_err)?
        else {
            continue;
        };
        if template.prepared_bootstrap_factory_origin.get().is_some() {
            return Ok(Some(template));
        }
    }
    Ok(None)
}

fn eager_comprehension_target_is_compiler_owned(
    module_runtime: &ModuleRuntimeContext,
    function_template: &FunctionInstantiationTemplate,
) -> bool {
    let function = function_template.function();
    if function.lowered_kind() != &FunctionKind::Function
        || function.execution_mode() != FunctionExecutionMode::Jit
    {
        return false;
    }
    let prefix = match function.names.display_name.as_str() {
        "<listcomp>" => "_dp_listcomp_",
        "<setcomp>" => "_dp_setcomp_",
        "<dictcomp>" => "_dp_dictcomp_",
        _ => return false,
    };
    let Some(suffix) = function.names.bind_name.strip_prefix(prefix) else {
        return false;
    };
    let [parameter] = function.params.params.as_slice() else {
        return false;
    };
    !suffix.is_empty()
        && suffix.bytes().all(|byte| byte.is_ascii_digit())
        && matches!(parameter.kind, ParamKind::Any | ParamKind::PosOnly)
        && !parameter.has_default
        && parameter.name.starts_with("_dp_iter_")
        && module_runtime
            .shared_module_state_owner
            .lookup_original_code(function.function_id)
            .is_none()
}

unsafe extern "C" fn drop_eager_comprehension_capsule(capsule: *mut ffi::PyObject) {
    let state =
        unsafe { ffi::PyCapsule_GetPointer(capsule, EAGER_COMPREHENSION_CAPSULE_NAME.as_ptr()) }
            .cast::<EagerComprehensionDirectEntry>();
    if state.is_null() {
        unsafe { ffi::PyErr_Clear() };
        return;
    }
    drop(unsafe { Box::from_raw(state) });
}

unsafe extern "C" fn call_eager_comprehension_direct(
    owner: *mut ffi::PyObject,
    argument: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    match panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if owner.is_null() || argument.is_null() || ffi::PyTuple_CheckExact(owner) == 0 {
            set_runtime_error("eager comprehension callable has an invalid owner or argument");
            return std::ptr::null_mut();
        }
        let capsule = ffi::PyTuple_GetItem(owner, 0);
        let globals = ffi::PyTuple_GetItem(owner, 1);
        let builtins = ffi::PyTuple_GetItem(owner, 2);
        let captures = ffi::PyTuple_GetItem(owner, 3);
        if capsule.is_null() || globals.is_null() || builtins.is_null() || captures.is_null() {
            return std::ptr::null_mut();
        }
        let state = ffi::PyCapsule_GetPointer(capsule, EAGER_COMPREHENSION_CAPSULE_NAME.as_ptr())
            .cast::<EagerComprehensionDirectEntry>();
        if state.is_null() {
            return std::ptr::null_mut();
        }
        let state = &*state;
        let mut environment = EagerComprehensionStackEnv {
            header: crate::FunctionEnvAbiHeader {
                direct_code_ptr: state.direct_code_ptr,
                default_direct_code_ptr: state.default_direct_code_ptr,
                deopt_table_ptr: state.deopt_table_ptr,
                globals_obj: globals,
                builtins_obj: builtins,
                late_bound_owner_cells: state.late_bound_owner_cells,
            },
            runtime_objects: [std::ptr::null_mut(); MAX_EAGER_COMPREHENSION_CAPTURES + 1],
        };
        for index in 0..state.closure_count {
            let pair = ffi::PyTuple_GetItem(captures, index as ffi::Py_ssize_t);
            if pair.is_null() {
                return std::ptr::null_mut();
            }
            let cell = ffi::PyTuple_GetItem(pair, 1);
            if cell.is_null() {
                return std::ptr::null_mut();
            }
            environment.runtime_objects[state.closure_slots[index]] = cell;
        }
        let thread_state = ffi::PyThreadState_Get();
        if thread_state.is_null() {
            set_runtime_error("eager comprehension callable requires an attached thread state");
            return std::ptr::null_mut();
        }
        let direct: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(state.direct_code_ptr);
        direct(
            std::ptr::addr_of_mut!(environment.header).cast(),
            thread_state.cast(),
            argument.cast(),
        )
        .cast::<ffi::PyObject>()
    })) {
        Ok(value) => value,
        Err(_) => {
            set_runtime_error("panic in eager comprehension direct callable");
            std::ptr::null_mut()
        }
    }
}

fn prepare_eager_comprehension_callable<'template>(
    py: Python<'_>,
    module_runtime: &ModuleRuntimeContext,
    function_template: &'template FunctionInstantiationTemplate,
    runtime_module: &Bound<'_, PyModule>,
    origin: &PreparedBootstrapFactoryOrigin,
    factory: *mut ffi::PyObject,
    parent_code: *mut ffi::PyObject,
    owner: usize,
    owner_version: u32,
) -> PyResult<Option<&'template PreparedEagerComprehension>> {
    if let Some(prepared) = function_template.prepared_eager_comprehension.get() {
        return Ok(Some(prepared));
    }
    let function = function_template.function();
    let engine = module_runtime
        .compile_session
        .process_jit()
        .map_err(PyRuntimeError::new_err)?;
    let Some(compiled_function) = engine
        .lookup_ready_direct_function(function)
        .map_err(PyRuntimeError::new_err)?
    else {
        return Ok(None);
    };
    let layout = function_template.runtime_data_layout();
    let mut closure_slots = [0; MAX_EAGER_COMPREHENSION_CAPTURES];
    for (index, slot) in closure_slots
        .iter_mut()
        .take(function_template.capture_names().len())
        .enumerate()
    {
        *slot = layout.closure_cell_slot(index);
    }
    let direct_code_ptr = compiled_function
        .direct_code_ptr()
        .map_err(PyRuntimeError::new_err)?
        .cast::<u8>();
    let default_direct_code_ptr = compiled_function
        .default_direct_code_ptr()
        .map_err(PyRuntimeError::new_err)?
        .cast::<u8>();
    let deopt_table_ptr = compiled_function
        .direct_deopt_table_ptr()
        .map_err(PyRuntimeError::new_err)?
        .cast::<c_void>();
    let state = Box::new(EagerComprehensionDirectEntry {
        method: ffi::PyMethodDef {
            ml_name: c"<eager comprehension>".as_ptr(),
            ml_meth: ffi::PyMethodDefPointer {
                PyCFunction: call_eager_comprehension_direct,
            },
            ml_flags: ffi::METH_O,
            ml_doc: std::ptr::null(),
        },
        _compiled_function: compiled_function,
        direct_code_ptr,
        default_direct_code_ptr,
        deopt_table_ptr,
        late_bound_owner_cells: module_runtime
            .shared_module_state_owner
            .late_bound_owner_fields
            .cells
            .as_ptr(),
        closure_slots,
        closure_count: function_template.capture_names().len(),
    });
    let state = Box::into_raw(state);
    let capsule = unsafe {
        ffi::PyCapsule_New(
            state.cast::<c_void>(),
            EAGER_COMPREHENSION_CAPSULE_NAME.as_ptr(),
            Some(drop_eager_comprehension_capsule),
        )
    };
    if capsule.is_null() {
        drop(unsafe { Box::from_raw(state) });
        return Err(PyErr::fetch(py));
    }
    let prepared = PreparedEagerComprehension {
        capsule: unsafe { Bound::<PyAny>::from_owned_ptr(py, capsule) }.unbind(),
        compile_session_id: module_runtime.compile_session.id(),
        runtime_module: runtime_module.as_ptr() as usize,
        runtime_owner: owner,
        runtime_owner_version: owner_version,
        factory: factory as usize,
        parent_code: parent_code as usize,
        origin_code: origin.code.as_ptr() as usize,
        origin_cache: origin.cache.as_ptr() as usize,
        cache_key: origin.cache_key.as_ptr() as usize,
        builtins_key: origin.builtins_key.as_ptr() as usize,
    };
    let _ = function_template.prepared_eager_comprehension.set(prepared);
    Ok(function_template.prepared_eager_comprehension.get())
}

fn make_eager_comprehension_callable(
    py: Python<'_>,
    module_runtime: &ModuleRuntimeContext,
    function_template: &FunctionInstantiationTemplate,
    runtime_module: &Bound<'_, PyModule>,
    captures: &Bound<'_, PyAny>,
    param_defaults: &Bound<'_, PyAny>,
    annotate_fn: &Bound<'_, PyAny>,
    module_globals: &Bound<'_, PyAny>,
) -> PyResult<Option<Py<PyAny>>> {
    if !eager_comprehension_target_is_compiler_owned(module_runtime, function_template)
        || crate::entry_interpreter_vectorcall_requested(function_template.function())
        || !annotate_fn.is_none()
        || unsafe { ffi::PyTuple_CheckExact(param_defaults.as_ptr()) } == 0
        || unsafe { ffi::PyTuple_Size(param_defaults.as_ptr()) } != 0
        || unsafe { ffi::PyTuple_CheckExact(captures.as_ptr()) } == 0
    {
        return Ok(None);
    }

    let capture_names = function_template.capture_names();
    let capture_count = capture_names.len();
    let layout = function_template.runtime_data_layout();
    if capture_count > MAX_EAGER_COMPREHENSION_CAPTURES
        || layout.positional_default_count() != 1
        || layout.closure_len() != capture_count
        || layout.total_len() != capture_count + 1
        || unsafe { ffi::PyTuple_Size(captures.as_ptr()) } != capture_count as ffi::Py_ssize_t
    {
        return Ok(None);
    }
    for (index, expected_name) in capture_names.iter().enumerate() {
        if matches!(expected_name.as_str(), "__class__" | "_dp_classcell") {
            return Ok(None);
        }
        let pair = unsafe { ffi::PyTuple_GetItem(captures.as_ptr(), index as ffi::Py_ssize_t) };
        if pair.is_null()
            || unsafe { ffi::PyTuple_CheckExact(pair) } == 0
            || unsafe { ffi::PyTuple_Size(pair) } != 2
        {
            return Ok(None);
        }
        let name = unsafe { ffi::PyTuple_GetItem(pair, 0) };
        let cell = unsafe { ffi::PyTuple_GetItem(pair, 1) };
        if name.is_null()
            || cell.is_null()
            || unsafe { ffi::PyUnicode_CheckExact(name) } == 0
            || !unicode_equals_str(
                &unsafe { Bound::<PyAny>::from_borrowed_ptr(py, name) },
                expected_name,
            )?
            || !is_cell_object(cell)
        {
            return Ok(None);
        }
    }

    let lookup_keys = prepared_runtime_lookup_keys(py, function_template)?;
    let existing = function_template.prepared_eager_comprehension.get();
    let origin_template = if existing.is_none() {
        runtime_bootstrap_origin_template(module_runtime.compile_session.as_ref())?
    } else {
        None
    };
    let origin = origin_template
        .as_ref()
        .and_then(|template| template.prepared_bootstrap_factory_origin.get());
    let (origin_code, origin_cache, cache_key, builtins_key, parent_code) =
        if let Some(prepared) = existing {
            if prepared.compile_session_id != module_runtime.compile_session.id()
                || prepared.runtime_module != runtime_module.as_ptr() as usize
            {
                return Ok(None);
            }
            (
                prepared.origin_code as *mut ffi::PyObject,
                prepared.origin_cache as *mut ffi::PyObject,
                prepared.cache_key as *mut ffi::PyObject,
                prepared.builtins_key as *mut ffi::PyObject,
                prepared.parent_code as *mut ffi::PyObject,
            )
        } else if let Some(origin) = origin {
            let Some(parent_code) = source_parent_code_for_eager_comprehension(
                module_runtime.shared_module_state_owner.as_ref(),
                function_template.function(),
            ) else {
                return Ok(None);
            };
            (
                origin.code.as_ptr(),
                origin.cache.as_ptr(),
                origin.cache_key.as_ptr(),
                origin.builtins_key.as_ptr(),
                parent_code,
            )
        } else {
            return Ok(None);
        };
    if unsafe { crate::jit::raw_py_function_activation_is_observed(parent_code) } {
        return Ok(None);
    }

    let (owner, owner_version) = if let Some(prepared) = existing {
        let owner = unsafe { ffi::Py_TYPE(runtime_module.as_ptr()) };
        if owner as usize != prepared.runtime_owner
            || unsafe { (*owner).tp_version_tag } != prepared.runtime_owner_version
            || unsafe { (*owner).tp_version_tag } == 0
        {
            return Ok(None);
        }
        (prepared.runtime_owner, prepared.runtime_owner_version)
    } else {
        let guard = prepared_synthetic_runtime_owner_guard(py, runtime_module, lookup_keys);
        if guard.0 == 0 {
            return Ok(None);
        }
        guard
    };
    let runtime_globals = unsafe { ffi::PyModule_GetDict(runtime_module.as_ptr()) };
    let factory = unsafe {
        eager_unicode_dict_item(runtime_globals, lookup_keys.code_factory.bind(py).as_ptr())
    };
    if factory.is_null()
        || unsafe { ffi::PyFunction_Check(factory) } == 0
        || existing.is_some_and(|prepared| prepared.factory != factory as usize)
    {
        return Ok(None);
    }
    let modules = unsafe { ffi::PyImport_GetModuleDict() };
    let bootstrap =
        unsafe { eager_unicode_dict_item(modules, lookup_keys.bootstrap_module.bind(py).as_ptr()) };
    if bootstrap.is_null() || unsafe { ffi::PyModule_CheckExact(bootstrap) } == 0 {
        return Ok(None);
    }
    let bootstrap_globals = unsafe { ffi::PyModule_GetDict(bootstrap) };
    let raw_factory = factory.cast::<ffi::PyFunctionObject>();
    if unsafe {
        eager_unicode_dict_item(
            bootstrap_globals,
            lookup_keys.code_factory.bind(py).as_ptr(),
        )
    } != factory
        || unsafe { (*raw_factory).func_globals } != bootstrap_globals
        || unsafe { (*raw_factory).func_code } != origin_code
        || !unsafe { (*raw_factory).func_defaults }.is_null()
        || !unsafe { (*raw_factory).func_kwdefaults }.is_null()
        || !unsafe { (*raw_factory).func_closure }.is_null()
    {
        return Ok(None);
    }
    let current_cache = unsafe { eager_unicode_dict_item(bootstrap_globals, cache_key) };
    if current_cache != origin_cache || unsafe { ffi::PyDict_CheckExact(current_cache) } == 0 {
        return Ok(None);
    }

    let mut builtins = unsafe { eager_unicode_dict_item(module_globals.as_ptr(), builtins_key) };
    if builtins.is_null() {
        return Ok(None);
    }
    if unsafe { ffi::PyModule_CheckExact(builtins) } != 0 {
        builtins = unsafe { ffi::PyModule_GetDict(builtins) };
    }
    if builtins.is_null() || unsafe { ffi::PyDict_CheckExact(builtins) } == 0 {
        return Ok(None);
    }

    let prepared = if let Some(prepared) = existing {
        prepared
    } else {
        let Some(origin) = origin else {
            return Ok(None);
        };
        let Some(prepared) = prepare_eager_comprehension_callable(
            py,
            module_runtime,
            function_template,
            runtime_module,
            origin,
            factory,
            parent_code,
            owner,
            owner_version,
        )?
        else {
            return Ok(None);
        };
        prepared
    };
    let owner = unsafe { ffi::PyTuple_New(4) };
    let owner = unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, owner)? };
    for (index, value) in [
        prepared.capsule.as_ptr(),
        module_globals.as_ptr(),
        builtins,
        captures.as_ptr(),
    ]
    .into_iter()
    .enumerate()
    {
        unsafe { ffi::Py_INCREF(value) };
        if unsafe { ffi::PyTuple_SetItem(owner.as_ptr(), index as ffi::Py_ssize_t, value) } != 0 {
            return Err(PyErr::fetch(py));
        }
    }
    let state = unsafe {
        ffi::PyCapsule_GetPointer(
            prepared.capsule.as_ptr(),
            EAGER_COMPREHENSION_CAPSULE_NAME.as_ptr(),
        )
    }
    .cast::<EagerComprehensionDirectEntry>();
    if state.is_null() {
        return Err(PyErr::fetch(py));
    }
    let callable = unsafe {
        ffi::PyCFunction_NewEx(
            std::ptr::addr_of_mut!((*state).method),
            owner.as_ptr(),
            std::ptr::null_mut(),
        )
    };
    Ok(Some(
        unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, callable)? }.unbind(),
    ))
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

fn prepared_synthetic_runtime_owner_guard(
    py: Python<'_>,
    runtime: &Bound<'_, PyModule>,
    lookup_keys: &PreparedRuntimeLookupKeys,
) -> (usize, u32) {
    unsafe {
        let owner = ffi::Py_TYPE(runtime.as_ptr());
        if owner.is_null() || (*owner).tp_base != std::ptr::addr_of_mut!(ffi::PyModule_Type) {
            return (0, 0);
        }
        let Some(actual_getattr) = (*owner).tp_getattro else {
            return (0, 0);
        };
        let Some(module_getattr) = ffi::PyModule_Type.tp_getattro else {
            return (0, 0);
        };
        if !std::ptr::fn_addr_eq(actual_getattr, module_getattr) {
            return (0, 0);
        }

        let Ok(canonical_owner) = crate::module_type::indexed_module_type_for_python(py) else {
            return (0, 0);
        };
        if owner != canonical_owner.as_ptr().cast::<ffi::PyTypeObject>() {
            return (0, 0);
        }

        // Static builtin types keep their per-interpreter dictionaries outside tp_dict. This
        // exact heap type directly inherits the immutable module/object hierarchy, whose static
        // dictionaries do not define code_with_freevars, so only its own dict can add a binding.
        let class_dict = (*owner).tp_dict;
        if class_dict.is_null() || ffi::PyDict_CheckExact(class_dict) == 0 {
            return (0, 0);
        }
        let keys = (*class_dict.cast::<ffi::PyDictObject>())
            .ma_keys
            .cast::<RawPyDictKeysPrefix>();
        if keys.is_null() || (*keys).dk_kind == 0 {
            return (0, 0);
        }
        if !ffi::PyDict_GetItem(class_dict, lookup_keys.code_factory.bind(py).as_ptr()).is_null() {
            return (0, 0);
        }

        if (*owner).tp_version_tag == 0 && crate::PyUnstable_Type_AssignVersionTag(owner) == 0 {
            return (0, 0);
        }
        let version = (*owner).tp_version_tag;
        if version == 0 {
            return (0, 0);
        }
        (owner as usize, version)
    }
}

fn synthetic_runtime_code_factory<'py>(
    py: Python<'py>,
    runtime: &Bound<'py, PyModule>,
    function_template: &FunctionInstantiationTemplate,
) -> PyResult<Bound<'py, PyAny>> {
    if let Some(prepared) = function_template.prepared_synthetic_code.get()
        && let Some(lookup_keys) = function_template.prepared_runtime_lookup_keys.get()
        && prepared.runtime_module == runtime.as_ptr() as usize
        && prepared.runtime_owner_type != 0
    {
        unsafe {
            let owner = ffi::Py_TYPE(runtime.as_ptr());
            if owner as usize == prepared.runtime_owner_type
                && (*owner).tp_version_tag == prepared.runtime_owner_type_version
                && (*owner).tp_version_tag != 0
                && (*owner).tp_getattro.is_some_and(|getattr| {
                    ffi::PyModule_Type
                        .tp_getattro
                        .is_some_and(|module_getattr| std::ptr::fn_addr_eq(getattr, module_getattr))
                })
            {
                let globals = ffi::PyModule_GetDict(runtime.as_ptr());
                if !globals.is_null() && ffi::PyDict_CheckExact(globals) != 0 {
                    let keys = (*globals.cast::<ffi::PyDictObject>())
                        .ma_keys
                        .cast::<RawPyDictKeysPrefix>();
                    if !keys.is_null() && (*keys).dk_kind != 0 {
                        let factory = ffi::PyDict_GetItem(
                            globals,
                            lookup_keys.code_factory.bind(py).as_ptr(),
                        );
                        if !factory.is_null() {
                            return Ok(Bound::from_borrowed_ptr(py, factory));
                        }
                    }
                }
            }
        }
    }

    runtime.getattr("code_with_freevars")
}

fn synthetic_code_for_template<'py>(
    py: Python<'py>,
    dp: &Bound<'py, PyModule>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    function_template: &FunctionInstantiationTemplate,
    captured_names: &[String],
) -> PyResult<(Bound<'py, PyAny>, bool)> {
    let factory = synthetic_runtime_code_factory(py, dp, function_template)?;
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
    // Factory invocation and code replacement can run arbitrary audit callbacks. Capture the
    // exact module owner only after they finish, without holding the template initialization lock.
    let (runtime_owner_type, runtime_owner_type_version) =
        prepared_synthetic_runtime_owner_guard(py, dp, lookup_keys);
    let prepared = PreparedSyntheticCode {
        runtime_module: dp.as_ptr() as usize,
        runtime_owner_type,
        runtime_owner_type_version,
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
    function_template: &Arc<FunctionInstantiationTemplate>,
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
    function_template: Option<&Arc<FunctionInstantiationTemplate>>,
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
    let has_original_runtime_code = function_template
        .and_then(|template| template.prepared_original_code.get())
        .map(Option::is_some)
        .unwrap_or_else(|| {
            module_runtime
                .shared_module_state_owner
                .lookup_original_code(function.function_id)
                .is_some()
        });
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
        function_template.map(Arc::as_ref),
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
        register_jit_vectorcall(
            py,
            &entry,
            function.function_id,
            module_runtime,
            function_template,
        )?;
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
    let module_runtime =
        module_runtime_from_shared_state(compile_session, shared_state, module_globals);
    if let Some(callable) = make_eager_comprehension_callable(
        py,
        &module_runtime,
        function_template.as_ref(),
        &dp,
        captures,
        param_defaults,
        annotate_fn,
        module_globals,
    )? {
        return Ok(callable);
    }
    let func = instantiate_bb_function_with_template(
        py,
        &dp,
        module_runtime
            .shared_module_state_owner
            .module_name
            .as_str(),
        &function_template,
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
        synthetic_code_for_template, synthetic_runtime_code_factory,
    };
    use pyo3::ffi;
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyModule};
    use soac_core::block_py::FunctionKind;
    use std::ffi::c_void;

    #[test]
    fn synthetic_code_caches_the_exact_indexed_runtime_module_attribute_guard() {
        unsafe extern "C" {
            fn _PyDict_NewIndexedKeySet(keys: *mut ffi::PyObject) -> *mut c_void;
            fn _PyDict_NewWithIndexedKeySet(keys: *mut c_void) -> *mut ffi::PyObject;
            fn _PyDictKeys_DecRef(keys: *mut c_void);
        }

        #[repr(C)]
        struct RawPyDictIndexedValuesForTest {
            capacity: ffi::Py_ssize_t,
            order_size: ffi::Py_ssize_t,
            values: [*mut ffi::PyObject; 1],
        }

        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();

        Python::attach(|py| {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                "def outer(offset):\n    return [offset + value for value in (1, 2)]\n",
            )
            .expect("synthetic runtime-attribute fixture should lower")
            .blockpy_module;
            let function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.qualname.contains("_dp_listcomp"))
                .expect(
                    "runtime-attribute fixture should contain its actual lowered comprehension",
                );
            let template = crate::FunctionInstantiationTemplate::from_function(function)
                .expect("runtime-attribute fixture should prepare its actual function template");
            let lookup_keys = prepared_runtime_lookup_keys(py, &template)
                .expect("runtime-attribute fixture should prepare its existing interned keys");

            let owner_type = crate::module_type::indexed_module_type_for_python(py)
                .expect("runtime-attribute fixture should obtain SOAC's actual module heap type");
            let runtime = owner_type
                .bind(py)
                .call1(("synthetic_runtime_attribute_fixture",))
                .and_then(|module| module.cast_into::<PyModule>().map_err(Into::into))
                .expect("runtime-attribute fixture should allocate an actual IndexedModuleType");
            assert_eq!(
                unsafe { ffi::Py_TYPE(runtime.as_ptr()) },
                owner_type.as_ptr().cast()
            );

            let key_names = [
                c"__name__",
                c"__doc__",
                c"__package__",
                c"__loader__",
                c"__spec__",
                c"code_with_freevars",
                c"__getattr__",
            ];
            let indexed_name_tuple =
                unsafe { ffi::PyTuple_New(key_names.len() as ffi::Py_ssize_t) };
            assert!(!indexed_name_tuple.is_null());
            let indexed_name_tuple =
                unsafe { Bound::<PyAny>::from_owned_ptr(py, indexed_name_tuple) };
            for (index, name) in key_names.iter().enumerate() {
                let key = unsafe { ffi::PyUnicode_InternFromString(name.as_ptr()) };
                assert!(!key.is_null());
                assert_eq!(
                    unsafe {
                        ffi::PyTuple_SetItem(
                            indexed_name_tuple.as_ptr(),
                            index as ffi::Py_ssize_t,
                            key,
                        )
                    },
                    0
                );
            }

            let indexed_keys = unsafe { _PyDict_NewIndexedKeySet(indexed_name_tuple.as_ptr()) };
            assert!(!indexed_keys.is_null());
            let indexed_dict = unsafe { _PyDict_NewWithIndexedKeySet(indexed_keys) };
            unsafe { _PyDictKeys_DecRef(indexed_keys) };
            assert!(!indexed_dict.is_null());
            let indexed_dict = unsafe { Bound::<PyAny>::from_owned_ptr(py, indexed_dict) };
            let previous_dict = unsafe { ffi::PyModule_GetDict(runtime.as_ptr()) };
            assert!(!previous_dict.is_null());
            assert_eq!(
                unsafe { ffi::PyDict_Update(indexed_dict.as_ptr(), previous_dict) },
                0
            );
            let owner = unsafe { ffi::Py_TYPE(runtime.as_ptr()) };
            let dict_offset = unsafe { (*owner).tp_dictoffset };
            assert!(dict_offset > 0);
            let dict_slot = unsafe {
                runtime
                    .as_ptr()
                    .cast::<u8>()
                    .offset(dict_offset as isize)
                    .cast::<*mut ffi::PyObject>()
            };
            assert_eq!(unsafe { *dict_slot }, previous_dict);
            unsafe {
                ffi::Py_INCREF(indexed_dict.as_ptr());
                *dict_slot = indexed_dict.as_ptr();
                ffi::Py_DECREF(previous_dict);
            }
            let actual_keys =
                unsafe { (*indexed_dict.as_ptr().cast::<ffi::PyDictObject>()).ma_keys }
                    .cast::<RawPyDictKeysPrefix>();
            assert_eq!(
                unsafe { (*actual_keys).dk_kind },
                3,
                "the production-path fixture must use SOAC's actual custom indexed dictionary"
            );

            let namespace = PyDict::new(py);
            namespace
                .set_item("cached_name", lookup_keys.code_factory.bind(py))
                .expect("runtime-attribute fixture should expose its existing interned name");
            py.import("builtins")
                .and_then(|builtins| builtins.getattr("exec"))
                .and_then(|exec| {
                    exec.call1((
                        r#"
import types

calls = []
missing_name_identities = []
subclass_name_identities = []
class_name_identities = []
property_events = []
collision_identities = []

def code_with_freevars(names, is_async, is_generator):
    calls.append(tuple(names))
    def placeholder():
        return None
    return placeholder.__code__

def replacement_factory(names, is_async, is_generator):
    return code_with_freevars(names, is_async, is_generator)

def missing_getattr(name):
    missing_name_identities.append(name is cached_name)
    return replacement_factory

class ObservedModule(types.ModuleType):
    def __getattribute__(self, name):
        if name == "code_with_freevars":
            subclass_name_identities.append(name is cached_name)
        return types.ModuleType.__getattribute__(self, name)

def observed_getattribute(self, name):
    if name == "code_with_freevars":
        class_name_identities.append(name is cached_name)
    return types.ModuleType.__getattribute__(self, name)

def observed_property(self):
    property_events.append("property")
    return replacement_factory

class CollisionKey:
    raise_error = False

    def __hash__(self):
        return hash(cached_name)

    def __eq__(self, other):
        collision_identities.append(other is cached_name)
        if self.raise_error:
            raise RuntimeError("runtime module attribute collision")
        return False
"#,
                        &namespace,
                    ))
                })
                .expect("runtime-attribute fixture should define a real code-producing factory");
            let factory = namespace
                .get_item(lookup_keys.code_factory.bind(py))
                .expect("fixture factory lookup should succeed")
                .expect("fixture factory should exist");
            runtime
                .setattr("code_with_freevars", &factory)
                .expect("actual indexed runtime dictionary should accept its factory");

            let bootstrap = PyModule::new(py, "synthetic_runtime_attribute_bootstrap")
                .expect("canonical bootstrap fixture should allocate");
            bootstrap
                .setattr("code_with_freevars", &factory)
                .expect("canonical bootstrap fixture should expose the same factory");
            let modules = py
                .import("sys")
                .and_then(|sys| sys.getattr("modules"))
                .and_then(|modules| modules.cast_into::<PyDict>().map_err(Into::into))
                .expect("fixture should access the real module dictionary");
            let original_bootstrap = modules
                .get_item("soac.bootstrap")
                .expect("original bootstrap lookup should succeed");

            let actual = (|| -> PyResult<(usize, usize, usize, u32, Vec<Vec<String>>)> {
                modules.set_item("soac.bootstrap", &bootstrap)?;
                let (first, first_prepared) = synthetic_code_for_template(
                    py,
                    &runtime,
                    function,
                    &template,
                    template.capture_names(),
                )?;
                let (second, second_prepared) = synthetic_code_for_template(
                    py,
                    &runtime,
                    function,
                    &template,
                    template.capture_names(),
                )?;
                assert!(first_prepared && second_prepared);
                let prepared = template
                    .prepared_synthetic_code
                    .get()
                    .expect("actual production synthetic-code creation should prepare its cache");
                let calls = namespace
                    .get_item("calls")?
                    .expect("fixture factory calls should exist")
                    .extract::<Vec<Vec<String>>>()?;
                Ok((
                    first.as_ptr() as usize,
                    second.as_ptr() as usize,
                    prepared.runtime_owner_type,
                    prepared.runtime_owner_type_version,
                    calls,
                ))
            })();
            match original_bootstrap {
                Some(original_bootstrap) => modules
                    .set_item("soac.bootstrap", original_bootstrap)
                    .expect("the original bootstrap module should restore before assertions"),
                None => modules
                    .del_item("soac.bootstrap")
                    .expect("the temporary bootstrap module should remove before assertions"),
            }

            let (first, second, actual_owner, owner_version, calls) =
                actual.expect("both actual production synthetic-code requests should succeed");
            assert_eq!(
                first, second,
                "the existing prepared code must remain shared"
            );
            assert_eq!(calls, vec![template.capture_names().to_vec()]);
            assert_eq!(
                actual_owner,
                owner_type.as_ptr() as usize,
                "the actual synthetic-code cache must guard the exact canonical indexed module type"
            );
            assert_ne!(
                owner_version, 0,
                "the cached canonical module owner must retain a live nonzero type version"
            );

            let direct_refcount = unsafe { ffi::Py_REFCNT(factory.as_ptr()) };
            let direct = synthetic_runtime_code_factory(py, &runtime, &template)
                .expect("the canonical indexed runtime should use its prepared factory");
            assert_eq!(direct.as_ptr(), factory.as_ptr());
            assert_eq!(
                unsafe { ffi::Py_REFCNT(factory.as_ptr()) },
                direct_refcount + 1,
                "the borrowed indexed dictionary value must become one owned lookup result"
            );
            drop(direct);
            assert_eq!(unsafe { ffi::Py_REFCNT(factory.as_ptr()) }, direct_refcount);

            let replacement = namespace
                .get_item("replacement_factory")
                .expect("replacement factory lookup should succeed")
                .expect("replacement factory should exist");
            let dict = unsafe { &*indexed_dict.as_ptr().cast::<ffi::PyDictObject>() };
            let values = dict.ma_values.cast::<RawPyDictIndexedValuesForTest>();
            assert!(!values.is_null());
            let factory_index = unsafe {
                crate::_PyDict_IndexedKeyIndex(
                    indexed_dict.as_ptr(),
                    lookup_keys.code_factory.bind(py).as_ptr(),
                )
            };
            assert!(factory_index >= 0);
            let factory_slot = unsafe {
                (&raw mut (*values).values)
                    .cast::<*mut ffi::PyObject>()
                    .add(factory_index as usize)
            };
            assert_eq!(unsafe { *factory_slot }, factory.as_ptr());
            let original_used = dict.ma_used;
            unsafe {
                ffi::Py_INCREF(replacement.as_ptr());
                *factory_slot = replacement.as_ptr();
                ffi::Py_DECREF(factory.as_ptr());
            }
            let live_replacement = synthetic_runtime_code_factory(py, &runtime, &template)
                .expect("a watcher-free indexed value replacement must remain observable");
            let replacement_ptr = live_replacement.as_ptr();
            drop(live_replacement);
            unsafe {
                ffi::Py_INCREF(factory.as_ptr());
                *factory_slot = factory.as_ptr();
                ffi::Py_DECREF(replacement.as_ptr());
            }
            assert_eq!(replacement_ptr, replacement.as_ptr());
            assert_eq!(dict.ma_used, original_used);
            assert_eq!(unsafe { (*owner).tp_version_tag }, owner_version);

            let missing_hook = namespace
                .get_item("missing_getattr")
                .expect("missing attribute hook lookup should succeed")
                .expect("missing attribute hook should exist");
            runtime
                .setattr("__getattr__", &missing_hook)
                .expect("the actual indexed runtime should install its module-level hook");
            runtime
                .delattr("code_with_freevars")
                .expect("the indexed runtime should delete its factory value");
            let missing = synthetic_runtime_code_factory(py, &runtime, &template);
            runtime
                .setattr("code_with_freevars", &factory)
                .expect("the canonical runtime factory should restore");
            runtime
                .delattr("__getattr__")
                .expect("the module-level fallback hook should restore");
            assert_eq!(
                missing
                    .expect("missing indexed values must use module __getattr__")
                    .as_ptr(),
                replacement.as_ptr()
            );
            assert_eq!(
                namespace
                    .get_item("missing_name_identities")
                    .expect("missing-name observations lookup should succeed")
                    .expect("missing-name observations should exist")
                    .extract::<Vec<bool>>()
                    .expect("missing-name observations should extract"),
                vec![false],
                "module __getattr__ must receive the original fresh Python attribute name"
            );

            let custom_type = namespace
                .get_item("ObservedModule")
                .expect("custom module subclass lookup should succeed")
                .expect("custom module subclass should exist");
            let custom_runtime = custom_type
                .call1(("custom_runtime_attribute_fixture",))
                .and_then(|module| module.cast_into::<PyModule>().map_err(Into::into))
                .expect("custom module subclass fixture should instantiate");
            custom_runtime
                .setattr("code_with_freevars", &factory)
                .expect("custom module subclass should accept its factory");
            assert_eq!(
                synthetic_runtime_code_factory(py, &custom_runtime, &template)
                    .expect("user-created module subclasses must keep ordinary getattr")
                    .as_ptr(),
                factory.as_ptr()
            );
            assert_eq!(
                namespace
                    .get_item("subclass_name_identities")
                    .expect("subclass-name observations lookup should succeed")
                    .expect("subclass-name observations should exist")
                    .extract::<Vec<bool>>()
                    .expect("subclass-name observations should extract"),
                vec![false],
                "a custom module __getattribute__ must receive its original fresh name"
            );

            let collision = namespace
                .get_item("CollisionKey")
                .expect("collision-key class lookup should succeed")
                .expect("collision-key class should exist")
                .call0()
                .expect("collision-key instance should allocate");
            let general_dict = PyDict::new(py);
            general_dict
                .set_item(&collision, py.None())
                .expect("the adversarial non-Unicode collision should force a GENERAL dict");
            general_dict
                .call_method1("update", (&indexed_dict,))
                .expect("the GENERAL module dictionary should retain all current values");
            namespace
                .get_item("collision_identities")
                .expect("collision observations lookup should succeed")
                .expect("collision observations should exist")
                .call_method0("clear")
                .expect("collision setup observations should clear");
            let general_keys = unsafe {
                (*general_dict.as_ptr().cast::<ffi::PyDictObject>())
                    .ma_keys
                    .cast::<RawPyDictKeysPrefix>()
            };
            assert_eq!(unsafe { (*general_keys).dk_kind }, 0);
            unsafe {
                ffi::Py_INCREF(general_dict.as_ptr());
                *dict_slot = general_dict.as_ptr();
                ffi::Py_DECREF(indexed_dict.as_ptr());
            }
            let general = synthetic_runtime_code_factory(py, &runtime, &template);
            collision
                .setattr("raise_error", true)
                .expect("the collision key should enable its raising equality hook");
            let general_error = synthetic_runtime_code_factory(py, &runtime, &template);
            unsafe {
                ffi::Py_INCREF(indexed_dict.as_ptr());
                *dict_slot = indexed_dict.as_ptr();
                ffi::Py_DECREF(general_dict.as_ptr());
            }
            assert_eq!(
                general
                    .expect("GENERAL dictionaries must retain the original module lookup")
                    .as_ptr(),
                factory.as_ptr()
            );
            assert!(
                general_error
                    .expect_err("module attribute lookup must propagate a raising equality hook")
                    .to_string()
                    .contains("runtime module attribute collision")
            );
            assert_eq!(
                namespace
                    .get_item("collision_identities")
                    .expect("collision observations lookup should succeed")
                    .expect("collision observations should exist")
                    .extract::<Vec<bool>>()
                    .expect("collision observations should extract"),
                vec![false, false],
                "GENERAL module dictionaries must keep both original fresh attribute names"
            );

            let property_descriptor = py
                .import("builtins")
                .and_then(|builtins| builtins.getattr("property"))
                .and_then(|property| {
                    property.call1((namespace
                        .get_item("observed_property")?
                        .expect("property callback should exist"),))
                })
                .expect("the canonical heap-type property should allocate");
            owner_type
                .bind(py)
                .setattr("code_with_freevars", &property_descriptor)
                .expect("the mutable canonical heap type should accept its data descriptor");
            let invalidated_version = unsafe { (*owner).tp_version_tag };
            let property_result = synthetic_runtime_code_factory(py, &runtime, &template);
            owner_type
                .bind(py)
                .delattr("code_with_freevars")
                .expect("the GLOBAL canonical heap type must restore before assertions");
            assert_ne!(invalidated_version, owner_version);
            assert_eq!(
                property_result
                    .expect("a canonical heap-type data descriptor must execute")
                    .as_ptr(),
                replacement.as_ptr()
            );
            assert_eq!(
                namespace
                    .get_item("property_events")
                    .expect("property observations lookup should succeed")
                    .expect("property observations should exist")
                    .extract::<Vec<String>>()
                    .expect("property observations should extract"),
                vec!["property"]
            );

            let custom_getattribute = namespace
                .get_item("observed_getattribute")
                .expect("canonical heap-type hook lookup should succeed")
                .expect("canonical heap-type hook should exist");
            owner_type
                .bind(py)
                .setattr("__getattribute__", &custom_getattribute)
                .expect("the mutable canonical heap type should accept __getattribute__");
            let class_result = synthetic_runtime_code_factory(py, &runtime, &template);
            owner_type
                .bind(py)
                .delattr("__getattribute__")
                .expect("the GLOBAL canonical heap type must restore before assertions");
            assert_eq!(
                class_result
                    .expect("a canonical heap-type __getattribute__ must execute")
                    .as_ptr(),
                factory.as_ptr()
            );
            assert_eq!(
                namespace
                    .get_item("class_name_identities")
                    .expect("heap-type name observations lookup should succeed")
                    .expect("heap-type name observations should exist")
                    .extract::<Vec<bool>>()
                    .expect("heap-type name observations should extract"),
                vec![false],
                "a mutated canonical __getattribute__ must receive its fresh original name"
            );
        });
    }

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
