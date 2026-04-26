use super::{CpythonTypeSymbol, RelocCallableRef, RelocTypeRef};
use crate::module_type::SharedModuleState;
use pyo3::ffi;
use soac_core::block_py::{BlockPyFunction, ModuleShape, RuntimeFunctionId};
use soac_core::profile::CounterDumpTypeKey;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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
    }
}

pub(super) fn cpython_type_symbol_from_name(name: &str) -> Option<CpythonTypeSymbol> {
    match name {
        "PyFunction_Type" => Some(CpythonTypeSymbol::Function),
        "PyMethod_Type" => Some(CpythonTypeSymbol::Method),
        "PyType_Type" => Some(CpythonTypeSymbol::Type),
        "PyLong_Type" => Some(CpythonTypeSymbol::Long),
        "PyList_Type" => Some(CpythonTypeSymbol::List),
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
