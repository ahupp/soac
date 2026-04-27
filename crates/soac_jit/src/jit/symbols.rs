use super::{
    PyFunction_Type, PyList_Type, PyLong_Type, PyMethod_Type, PyThreadState_GetUnchecked,
    PyType_Type, PyUnicode_Type,
};
use crate::module_type::SharedModuleState;
use pyo3::ffi;
use soac_core::block_py::{BlockPyFunction, ModuleShape, RuntimeFunctionId};
use soac_core::profile::CounterDumpTypeKey;
use soac_ir_typed::TypedAttrOwnerRef;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum CpythonTypeSymbol {
    Function,
    Method,
    Type,
    Long,
    List,
    Unicode,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum RelocTypeRef {
    CpythonTypeSymbol(CpythonTypeSymbol),
    TypeKey(CounterDumpTypeKey),
}

pub(super) fn typed_attr_owner_ref_from_reloc_type_ref(
    owner_type_ref: &RelocTypeRef,
) -> TypedAttrOwnerRef {
    match owner_type_ref {
        RelocTypeRef::CpythonTypeSymbol(symbol) => {
            TypedAttrOwnerRef::CpythonTypeSymbol(cpython_type_symbol_name(*symbol).to_string())
        }
        RelocTypeRef::TypeKey(type_key) => TypedAttrOwnerRef::TypeKey {
            module_name: type_key.module_name.clone(),
            qualname: type_key.qualname.clone(),
        },
    }
}

pub(super) fn reloc_type_ref_from_typed_attr_owner_ref(
    owner_type_ref: &TypedAttrOwnerRef,
) -> Option<RelocTypeRef> {
    match owner_type_ref {
        TypedAttrOwnerRef::CpythonTypeSymbol(symbol_name) => {
            cpython_type_symbol_from_name(symbol_name).map(RelocTypeRef::CpythonTypeSymbol)
        }
        TypedAttrOwnerRef::TypeKey {
            module_name,
            qualname,
        } => Some(RelocTypeRef::TypeKey(CounterDumpTypeKey {
            module_name: module_name.clone(),
            qualname: qualname.clone(),
        })),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum RelocCallableRef {
    OwnerAttr {
        owner_type_ref: RelocTypeRef,
        attr_name: String,
    },
}

static JIT_DATA_SYMBOLS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
static TYPE_KEY_RUNTIME_REGISTRY: OnceLock<Mutex<HashMap<CounterDumpTypeKey, usize>>> =
    OnceLock::new();

unsafe extern "C" {
    fn _Py_Dealloc(obj: *mut ffi::PyObject);
}

pub(super) fn py_dealloc_symbol() -> *const u8 {
    _Py_Dealloc as *const u8
}

fn jit_data_symbols() -> &'static Mutex<HashMap<String, usize>> {
    JIT_DATA_SYMBOLS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn type_key_runtime_registry() -> &'static Mutex<HashMap<CounterDumpTypeKey, usize>> {
    TYPE_KEY_RUNTIME_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn register_jit_data_symbol(symbol: &str, ptr: *const u8) {
    let mut symbols = jit_data_symbols()
        .lock()
        .expect("JIT data symbol registry lock poisoned");
    symbols.insert(symbol.to_string(), ptr as usize);
}

pub(super) fn lookup_registered_jit_data_symbol(symbol: &str) -> Option<*const u8> {
    let symbols = jit_data_symbols()
        .lock()
        .expect("JIT data symbol registry lock poisoned");
    symbols.get(symbol).copied().map(|ptr| ptr as *const u8)
}

pub(super) fn cpython_type_symbol_name(symbol: CpythonTypeSymbol) -> &'static str {
    match symbol {
        CpythonTypeSymbol::Function => "PyFunction_Type",
        CpythonTypeSymbol::Method => "PyMethod_Type",
        CpythonTypeSymbol::Type => "PyType_Type",
        CpythonTypeSymbol::Long => "PyLong_Type",
        CpythonTypeSymbol::List => "PyList_Type",
        CpythonTypeSymbol::Unicode => "PyUnicode_Type",
    }
}

pub(super) fn cpython_type_symbol_from_name(name: &str) -> Option<CpythonTypeSymbol> {
    match name {
        "PyFunction_Type" => Some(CpythonTypeSymbol::Function),
        "PyMethod_Type" => Some(CpythonTypeSymbol::Method),
        "PyType_Type" => Some(CpythonTypeSymbol::Type),
        "PyLong_Type" => Some(CpythonTypeSymbol::Long),
        "PyList_Type" => Some(CpythonTypeSymbol::List),
        "PyUnicode_Type" => Some(CpythonTypeSymbol::Unicode),
        _ => None,
    }
}

pub(super) fn push_symbol_component_hex(out: &mut String, component: &str) {
    for byte in component.as_bytes() {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("upper hex digit should exist"));
        out.push(
            char::from_digit(u32::from(byte & 0x0f), 16).expect("lower hex digit should exist"),
        );
    }
}

pub(super) const JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT: &str = "d";
pub(super) const SOAC_RUNTIME_INCREF_SYMBOL: &str = "soac_runtime_incref";
pub(super) const SOAC_RUNTIME_DECREF_SYMBOL: &str = "soac_runtime_decref";
pub(super) const SOAC_RUNTIME_INCREF_APPLIED_SYMBOL: &str = "soac_runtime_incref_applied";
pub(super) const SOAC_RUNTIME_DECREF_APPLIED_SYMBOL: &str = "soac_runtime_decref_applied";
pub(super) const SOAC_RUNTIME_SET_RAISED_EXCEPTION_SYMBOL: &str =
    "soac_runtime_set_raised_exception";
pub(super) const SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL: &str = "soac_runtime_load_global";
pub(super) const SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL: &str =
    "soac_runtime_probe_global_indexed";
pub(super) const SOAC_RUNTIME_STORE_GLOBAL_SYMBOL: &str = "soac_runtime_store_global";
pub(super) const SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL: &str =
    "soac_runtime_store_global_indexed";
pub(super) const SOAC_RUNTIME_PROBE_FIELD_INDEXED_SYMBOL: &str = "soac_runtime_probe_field_indexed";
pub(super) const SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL: &str = "soac_runtime_store_field_indexed";
pub(super) const SOAC_RUNTIME_TUPLE_NEW_SYMBOL: &str = "soac_runtime_tuple_new";
pub(super) const SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_SYMBOL: &str =
    "soac_runtime_tuple_set_item_stolen";
#[cfg(test)]
pub(super) const SOAC_RUNTIME_PYLONG_AS_I64_SYMBOL: &str = "soac_runtime_pylong_as_i64";
pub(super) const SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_SYMBOL: &str =
    "soac_runtime_pylong_as_i64_saturating";

pub(super) fn jit_python_perf_symbol_name(kind: &str, qualname: &str) -> String {
    format!("py:{kind}:{qualname}")
}

pub(super) fn direct_function_symbol(
    function: &BlockPyFunction<impl ModuleShape>,
    symbol_scope: Option<&str>,
) -> String {
    let base =
        jit_python_perf_symbol_name(JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT, &function.names.qualname);
    scoped_jit_symbol(&base, symbol_scope)
}

pub(super) fn default_direct_function_symbol(
    function: &BlockPyFunction<impl ModuleShape>,
    symbol_scope: Option<&str>,
) -> String {
    let base = format!(
        "{}:defaults",
        jit_python_perf_symbol_name(JIT_PYTHON_PERF_SYMBOL_KIND_DIRECT, &function.names.qualname)
    );
    scoped_jit_symbol(&base, symbol_scope)
}

pub(super) fn direct_function_symbol_scope(
    function_id: RuntimeFunctionId,
    symbol_id: u64,
) -> String {
    format!("fn_{}_{}", function_id.to_packed_runtime_u64(), symbol_id)
}

pub(super) fn direct_function_backend_name(
    function: &BlockPyFunction<impl ModuleShape>,
    shared_state: Option<&SharedModuleState>,
) -> String {
    let mut name = String::from("direct:");
    match shared_state {
        Some(shared_state) => push_direct_function_module_identity(
            &mut name,
            shared_state.module_name.as_str(),
            shared_state.source_hash(),
        ),
        None => {
            name.push_str("module_id:");
            name.push_str(
                function
                    .function_id
                    .runtime_module_id()
                    .as_u32()
                    .to_string()
                    .as_str(),
            );
        }
    }
    name.push(':');
    name.push_str(function.names.qualname.as_str());
    name.push(':');
    name.push_str(function.params.len().to_string().as_str());
    name
}

pub(super) fn push_direct_function_module_identity(
    out: &mut String,
    module_name: &str,
    source_hash: u64,
) {
    push_symbol_component_hex(out, module_name);
    out.push(':');
    out.push_str(format!("{source_hash:016x}").as_str());
}

pub(super) fn scoped_jit_symbol(base: &str, symbol_scope: Option<&str>) -> String {
    match symbol_scope {
        Some(scope) => format!("{base}:{scope}"),
        None => base.to_string(),
    }
}

pub(super) fn is_clif_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(super) fn reloc_type_ref_symbol_name(type_ref: &RelocTypeRef) -> Cow<'static, str> {
    match type_ref {
        RelocTypeRef::CpythonTypeSymbol(symbol) => Cow::Borrowed(cpython_type_symbol_name(*symbol)),
        RelocTypeRef::TypeKey(type_key) => {
            let mut symbol = String::from("__soac_typekey_");
            push_symbol_component_hex(&mut symbol, type_key.module_name.as_str());
            symbol.push('_');
            push_symbol_component_hex(&mut symbol, type_key.qualname.as_str());
            Cow::Owned(symbol)
        }
    }
}

pub(super) fn reloc_callable_ref_symbol_name(callable_ref: &RelocCallableRef) -> String {
    match callable_ref {
        RelocCallableRef::OwnerAttr {
            owner_type_ref,
            attr_name,
        } => {
            let mut symbol = String::from("__soac_callable_owner_attr_");
            let owner_symbol = reloc_type_ref_symbol_name(owner_type_ref);
            push_symbol_component_hex(&mut symbol, owner_symbol.as_ref());
            symbol.push('_');
            push_symbol_component_hex(&mut symbol, attr_name.as_str());
            symbol
        }
    }
}

pub(super) fn resolve_type_key_to_type(
    type_key: &CounterDumpTypeKey,
) -> Result<Option<*mut ffi::PyTypeObject>, String> {
    if type_key.module_name.is_empty()
        || type_key.qualname.is_empty()
        || type_key.qualname.split('.').any(|part| part == "<locals>")
    {
        return Ok(None);
    }
    if unsafe { PyThreadState_GetUnchecked() }.is_null() {
        return Ok(None);
    }

    let module_name = CString::new(type_key.module_name.as_str())
        .map_err(|_| format!("type key module contains NUL: {:?}", type_key.module_name))?;
    let modules = unsafe { ffi::PyImport_GetModuleDict() };
    if modules.is_null() {
        if unsafe { !ffi::PyErr_Occurred().is_null() } {
            return Err("failed to read sys.modules while resolving type key".to_string());
        }
        return Ok(None);
    }
    let mut current = unsafe { ffi::PyDict_GetItemString(modules, module_name.as_ptr()) };
    if current.is_null() {
        return Ok(None);
    }
    unsafe { ffi::Py_INCREF(current) };

    for part in type_key.qualname.split('.') {
        if part.is_empty() {
            unsafe { ffi::Py_DECREF(current) };
            return Ok(None);
        }
        let part = CString::new(part)
            .map_err(|_| format!("type key qualname contains NUL: {:?}", type_key.qualname))?;
        let next = unsafe { ffi::PyObject_GetAttrString(current, part.as_ptr()) };
        unsafe { ffi::Py_DECREF(current) };
        if next.is_null() {
            unsafe { ffi::PyErr_Clear() };
            return Ok(None);
        }
        current = next;
    }

    if unsafe { ffi::PyType_Check(current) } == 0 {
        unsafe { ffi::Py_DECREF(current) };
        return Ok(None);
    }
    let owner_type = current as *mut ffi::PyTypeObject;
    unsafe { ffi::Py_DECREF(current) };
    Ok(Some(owner_type))
}

fn cpython_type_symbol_for_type(owner_type: *mut ffi::PyTypeObject) -> Option<CpythonTypeSymbol> {
    match owner_type {
        ptr if ptr == std::ptr::addr_of_mut!(PyFunction_Type) => Some(CpythonTypeSymbol::Function),
        ptr if ptr == std::ptr::addr_of_mut!(PyMethod_Type) => Some(CpythonTypeSymbol::Method),
        ptr if ptr == std::ptr::addr_of_mut!(PyType_Type) => Some(CpythonTypeSymbol::Type),
        ptr if ptr == std::ptr::addr_of_mut!(PyLong_Type) => Some(CpythonTypeSymbol::Long),
        ptr if ptr == std::ptr::addr_of_mut!(PyList_Type) => Some(CpythonTypeSymbol::List),
        ptr if ptr == std::ptr::addr_of_mut!(PyUnicode_Type) => Some(CpythonTypeSymbol::Unicode),
        _ => None,
    }
}

fn resolve_cpython_type_symbol(symbol: CpythonTypeSymbol) -> *mut ffi::PyTypeObject {
    match symbol {
        CpythonTypeSymbol::Function => std::ptr::addr_of_mut!(PyFunction_Type),
        CpythonTypeSymbol::Method => std::ptr::addr_of_mut!(PyMethod_Type),
        CpythonTypeSymbol::Type => std::ptr::addr_of_mut!(PyType_Type),
        CpythonTypeSymbol::Long => std::ptr::addr_of_mut!(PyLong_Type),
        CpythonTypeSymbol::List => std::ptr::addr_of_mut!(PyList_Type),
        CpythonTypeSymbol::Unicode => std::ptr::addr_of_mut!(PyUnicode_Type),
    }
}

fn py_string_attr_owned(
    obj: *mut ffi::PyObject,
    attr_name: &CStr,
) -> Result<Option<String>, String> {
    let attr = unsafe { ffi::PyObject_GetAttrString(obj, attr_name.as_ptr()) };
    if attr.is_null() {
        unsafe { ffi::PyErr_Clear() };
        return Ok(None);
    }
    if unsafe { ffi::PyUnicode_Check(attr) } == 0 {
        unsafe { ffi::Py_DECREF(attr) };
        return Ok(None);
    }
    let mut size = 0isize;
    let data = unsafe { ffi::PyUnicode_AsUTF8AndSize(attr, &mut size) };
    if data.is_null() {
        unsafe { ffi::Py_DECREF(attr) };
        return Err(format!(
            "failed to read Python string attribute {} as UTF-8",
            attr_name.to_string_lossy()
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) };
    let value = match std::str::from_utf8(bytes) {
        Ok(value) => value.to_owned(),
        Err(err) => {
            unsafe { ffi::Py_DECREF(attr) };
            return Err(format!(
                "Python string attribute {} was not valid UTF-8: {err}",
                attr_name.to_string_lossy()
            ));
        }
    };
    unsafe { ffi::Py_DECREF(attr) };
    Ok(Some(value))
}

pub(super) fn type_key_for_type(
    owner_type: *mut ffi::PyTypeObject,
) -> Result<Option<CounterDumpTypeKey>, String> {
    if owner_type.is_null() {
        return Ok(None);
    }
    let owner_obj = owner_type.cast::<ffi::PyObject>();
    let Some(module_name) = py_string_attr_owned(owner_obj, c"__module__")? else {
        return Ok(None);
    };
    let Some(qualname) = py_string_attr_owned(owner_obj, c"__qualname__")? else {
        return Ok(None);
    };
    if module_name.is_empty()
        || qualname.is_empty()
        || qualname.split('.').any(|part| part == "<locals>")
    {
        return Ok(None);
    }
    Ok(Some(CounterDumpTypeKey {
        module_name,
        qualname,
    }))
}

pub(super) fn register_runtime_type_for_key(
    type_key: &CounterDumpTypeKey,
    owner_type: *mut ffi::PyTypeObject,
) {
    let mut registry = type_key_runtime_registry()
        .lock()
        .expect("type key runtime registry lock poisoned");
    registry.insert(type_key.clone(), owner_type as usize);
}

fn lookup_runtime_type_for_key(type_key: &CounterDumpTypeKey) -> Option<*mut ffi::PyTypeObject> {
    let registry = type_key_runtime_registry()
        .lock()
        .expect("type key runtime registry lock poisoned");
    registry
        .get(type_key)
        .copied()
        .map(|ptr| ptr as *mut ffi::PyTypeObject)
}

pub(super) fn reloc_type_ref_for_type(
    owner_type: *mut ffi::PyTypeObject,
) -> Result<Option<RelocTypeRef>, String> {
    if let Some(symbol) = cpython_type_symbol_for_type(owner_type) {
        return Ok(Some(RelocTypeRef::CpythonTypeSymbol(symbol)));
    }
    let Some(type_key) = type_key_for_type(owner_type)? else {
        return Ok(None);
    };
    register_runtime_type_for_key(&type_key, owner_type);
    Ok(Some(RelocTypeRef::TypeKey(type_key)))
}

pub(super) fn resolve_reloc_type_ref_to_type(
    owner_type_ref: &RelocTypeRef,
) -> Result<Option<*mut ffi::PyTypeObject>, String> {
    match owner_type_ref {
        RelocTypeRef::CpythonTypeSymbol(symbol) => Ok(Some(resolve_cpython_type_symbol(*symbol))),
        RelocTypeRef::TypeKey(type_key) => {
            if let Some(owner_type) = lookup_runtime_type_for_key(type_key) {
                return Ok(Some(owner_type));
            }
            resolve_type_key_to_type(type_key)
        }
    }
}

pub(super) fn ensure_reloc_type_symbol_registered(
    owner_type_ref: &RelocTypeRef,
) -> Result<bool, String> {
    match owner_type_ref {
        RelocTypeRef::CpythonTypeSymbol(_) => Ok(true),
        RelocTypeRef::TypeKey(_) => {
            let symbol = reloc_type_ref_symbol_name(owner_type_ref);
            if lookup_registered_jit_data_symbol(symbol.as_ref()).is_some() {
                return Ok(true);
            }
            let Some(owner_type) = resolve_reloc_type_ref_to_type(owner_type_ref)? else {
                return Ok(false);
            };
            register_jit_data_symbol(symbol.as_ref(), owner_type.cast::<u8>());
            Ok(true)
        }
    }
}

fn resolve_reloc_callable_ref_to_object(
    callable_ref: &RelocCallableRef,
) -> Result<Option<*mut ffi::PyObject>, String> {
    match callable_ref {
        RelocCallableRef::OwnerAttr {
            owner_type_ref,
            attr_name,
        } => {
            let Some(owner_type) = resolve_reloc_type_ref_to_type(owner_type_ref)? else {
                return Ok(None);
            };
            let attr_name = CString::new(attr_name.as_str()).map_err(|_| {
                format!("callable attr contains NUL and cannot be resolved: {attr_name:?}")
            })?;
            let dict = unsafe { (*owner_type).tp_dict };
            if dict.is_null() {
                return Ok(None);
            }
            let value = unsafe { ffi::PyDict_GetItemString(dict, attr_name.as_ptr()) };
            if value.is_null() || unsafe { ffi::PyFunction_Check(value) } == 0 {
                return Ok(None);
            }
            Ok(Some(value))
        }
    }
}

pub(super) fn ensure_reloc_callable_symbol_registered(
    callable_ref: &RelocCallableRef,
) -> Result<bool, String> {
    let symbol = reloc_callable_ref_symbol_name(callable_ref);
    if lookup_registered_jit_data_symbol(symbol.as_str()).is_some() {
        return Ok(true);
    }
    let Some(callable) = resolve_reloc_callable_ref_to_object(callable_ref)? else {
        return Ok(false);
    };
    register_jit_data_symbol(symbol.as_str(), callable.cast::<u8>());
    Ok(true)
}
