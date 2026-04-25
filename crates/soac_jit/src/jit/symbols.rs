use super::*;

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
