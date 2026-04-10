use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyModule, PyTuple};
use soac_blockpy::block_py::{
    AbruptKind, BlockArg, BlockPyFunction, BlockPyModule, BlockTerm, CallArgKeyword,
    ChildVisitable, InstrCodegen, InstrResolved, Literal, NameLike, NumberLiteralValue,
    ParamDefaultSource, operation as blockpy_intrinsics,
};
use soac_blockpy::passes::CodegenModuleShape;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_int};
use std::ptr;

unsafe extern "C" {
    fn _Py_SetImmortal(op: *mut ffi::PyObject);
    fn PyUnstable_IsImmortal(op: *mut ffi::PyObject) -> c_int;
}

const ALWAYS_REQUIRED_UNICODE_CONSTANTS: &[&str] = &[
    "dict",
    "list",
    "raise_from",
    "tuple_from_iter",
    "append",
    "extend",
    "update",
];
const ALWAYS_REQUIRED_RUNTIME_NAME_CONSTANTS: &[&str] = &[
    "TRUE",
    "FALSE",
    "NONE",
    "DELETED",
    "EMPTY_TUPLE",
    "ITER_COMPLETE",
];
const SOAC_RUNTIME_BOOTSTRAP_HELPER_NAMES: &[&str] = &[
    "tuple_values",
    "make_function",
    "create_class",
    "import_",
    "import_attr",
    "class_lookup_global",
    "class_lookup_cell",
];

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct ModuleConstantId(pub usize);

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum ModuleConstantValue {
    Unicode(Vec<u8>),
    Bytes(Vec<u8>),
    Int(i64),
    BigInt(String),
    FloatBits(u64),
    RuntimeName(Vec<u8>),
}

#[derive(Debug, Clone, Copy)]
enum RuntimeNameConstantMode {
    ImportRuntime,
    BootstrapSoacRuntime,
}

#[derive(Debug, Clone, Default)]
pub struct ModuleCodegenConstants {
    values: Vec<ModuleConstantValue>,
    ids: HashMap<ModuleConstantValue, ModuleConstantId>,
}

impl ModuleCodegenConstants {
    pub fn collect_from_module(module: &BlockPyModule<CodegenModuleShape>) -> Self {
        Self::collect_from_module_with_runtime_prelude(module, true)
    }

    pub fn collect_from_runtime_module(module: &BlockPyModule<CodegenModuleShape>) -> Self {
        Self::collect_from_module_with_runtime_prelude(module, true)
    }

    fn collect_from_module_with_runtime_prelude(
        module: &BlockPyModule<CodegenModuleShape>,
        include_runtime_name_prelude: bool,
    ) -> Self {
        let mut collector = ModuleConstantCollector::default();
        for expr in &module.module_constants {
            collector.constants.push_explicit_constant_expr(expr);
        }
        for name in ALWAYS_REQUIRED_UNICODE_CONSTANTS {
            collector.constants.intern_unicode_bytes(name.as_bytes());
        }
        if include_runtime_name_prelude {
            for name in ALWAYS_REQUIRED_RUNTIME_NAME_CONSTANTS {
                collector
                    .constants
                    .intern_runtime_name_bytes(name.as_bytes());
            }
        }
        for function in &module.callable_defs {
            collector.collect_function(function);
        }
        collector.constants
    }

    pub fn collect_from_functions<'a>(
        functions: impl IntoIterator<Item = &'a BlockPyFunction<CodegenModuleShape>>,
    ) -> Self {
        let mut collector = ModuleConstantCollector::default();
        for name in ALWAYS_REQUIRED_UNICODE_CONSTANTS {
            collector.constants.intern_unicode_bytes(name.as_bytes());
        }
        for name in ALWAYS_REQUIRED_RUNTIME_NAME_CONSTANTS {
            collector
                .constants
                .intern_runtime_name_bytes(name.as_bytes());
        }
        for function in functions {
            collector.collect_function(function);
        }
        collector.constants
    }

    pub fn build_python_constants(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        self.build_python_constants_with_runtime_names(py, RuntimeNameConstantMode::ImportRuntime)
    }

    pub fn build_python_constants_for_soac_runtime(
        &self,
        py: Python<'_>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        self.build_python_constants_with_runtime_names(
            py,
            RuntimeNameConstantMode::BootstrapSoacRuntime,
        )
    }

    fn build_python_constants_with_runtime_names(
        &self,
        py: Python<'_>,
        runtime_name_mode: RuntimeNameConstantMode,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let mut out = Vec::with_capacity(self.values.len());
        for value in &self.values {
            out.push(match value {
                ModuleConstantValue::Unicode(bytes) => build_unicode_constant(py, bytes)?.unbind(),
                ModuleConstantValue::Bytes(bytes) => {
                    let ptr = unsafe {
                        ffi::PyBytes_FromStringAndSize(
                            bytes.as_ptr() as *const i8,
                            bytes.len() as ffi::Py_ssize_t,
                        )
                    };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
                ModuleConstantValue::Int(value) => {
                    let ptr = unsafe { ffi::PyLong_FromLongLong(*value as std::ffi::c_longlong) };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
                ModuleConstantValue::BigInt(value) => {
                    let value = std::ffi::CString::new(value.as_str())
                        .expect("big int literal should not contain NUL");
                    let mut end_ptr = std::ptr::null_mut();
                    let ptr = unsafe { ffi::PyLong_FromString(value.as_ptr(), &mut end_ptr, 0) };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
                ModuleConstantValue::FloatBits(bits) => {
                    let ptr = unsafe { ffi::PyFloat_FromDouble(f64::from_bits(*bits)) };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
                ModuleConstantValue::RuntimeName(bytes) => {
                    build_runtime_name_constant(py, bytes, runtime_name_mode)?
                }
            });
        }
        mark_constants_immortal(&out);
        Ok(out)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn require_unicode_constant_id(&self, value: &str) -> ModuleConstantId {
        self.require_unicode_constant_id_for_bytes(value.as_bytes())
    }

    pub fn require_unicode_constant_id_for_bytes(&self, value: &[u8]) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::Unicode(value.to_vec()))
            .unwrap_or_else(|| {
                panic!(
                    "missing module unicode constant in codegen pool: {:?}",
                    String::from_utf8_lossy(value)
                )
            })
    }

    pub fn require_bytes_constant_id(&self, value: &[u8]) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::Bytes(value.to_vec()))
            .unwrap_or_else(|| panic!("missing module bytes constant in codegen pool"))
    }

    pub fn require_int_constant_id(&self, value: i64) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::Int(value))
            .unwrap_or_else(|| panic!("missing module int constant in codegen pool: {value}"))
    }

    pub fn require_big_int_constant_id(&self, value: &str) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::BigInt(value.to_string()))
            .unwrap_or_else(|| panic!("missing module big-int constant in codegen pool: {value}"))
    }

    pub fn require_float_constant_id(&self, value: f64) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::FloatBits(value.to_bits()))
            .unwrap_or_else(|| panic!("missing module float constant in codegen pool: {value}"))
    }

    pub fn require_runtime_name_constant_id(&self, value: &str) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::RuntimeName(value.as_bytes().to_vec()))
            .unwrap_or_else(|| {
                panic!("missing runtime-name module constant in codegen pool: {value}")
            })
    }

    pub fn constant_bytes_value(&self, constant_id: ModuleConstantId) -> Option<&[u8]> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::Bytes(bytes) => Some(bytes.as_slice()),
            ModuleConstantValue::Unicode(_)
            | ModuleConstantValue::Int(_)
            | ModuleConstantValue::BigInt(_)
            | ModuleConstantValue::FloatBits(_)
            | ModuleConstantValue::RuntimeName(_) => None,
        }
    }

    pub fn constant_string_bytes_value(&self, constant_id: ModuleConstantId) -> Option<&[u8]> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::Unicode(bytes) | ModuleConstantValue::Bytes(bytes) => {
                Some(bytes.as_slice())
            }
            ModuleConstantValue::Int(_)
            | ModuleConstantValue::BigInt(_)
            | ModuleConstantValue::FloatBits(_)
            | ModuleConstantValue::RuntimeName(_) => None,
        }
    }

    pub fn constant_string_value(&self, constant_id: ModuleConstantId) -> Option<String> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::Unicode(bytes) | ModuleConstantValue::Bytes(bytes) => {
                String::from_utf8(bytes.clone()).ok()
            }
            ModuleConstantValue::Int(_)
            | ModuleConstantValue::BigInt(_)
            | ModuleConstantValue::FloatBits(_)
            | ModuleConstantValue::RuntimeName(_) => None,
        }
    }

    pub fn constant_runtime_name_value(&self, constant_id: ModuleConstantId) -> Option<&str> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::RuntimeName(bytes) => std::str::from_utf8(bytes).ok(),
            ModuleConstantValue::Unicode(_)
            | ModuleConstantValue::Bytes(_)
            | ModuleConstantValue::Int(_)
            | ModuleConstantValue::BigInt(_)
            | ModuleConstantValue::FloatBits(_) => None,
        }
    }

    fn lookup_id(&self, value: &ModuleConstantValue) -> Option<ModuleConstantId> {
        self.ids.get(value).copied()
    }

    fn push_explicit_constant_expr(&mut self, expr: &InstrResolved) -> ModuleConstantId {
        let value = match expr {
            InstrResolved::Literal(literal) => match literal.as_literal() {
                Literal::StringLiteral(string) => {
                    ModuleConstantValue::Unicode(string.value.as_bytes().to_vec())
                }
                Literal::BytesLiteral(bytes) => ModuleConstantValue::Bytes(bytes.value.clone()),
                Literal::NumberLiteral(number) => match &number.value {
                    NumberLiteralValue::Int(value) => {
                        if let Some(value) = value.as_i64() {
                            ModuleConstantValue::Int(value)
                        } else {
                            ModuleConstantValue::BigInt(value.to_string())
                        }
                    }
                    NumberLiteralValue::Float(value) => {
                        ModuleConstantValue::FloatBits(value.to_bits())
                    }
                },
            },
            InstrResolved::Load(op) if op.name.is_runtime_name() => {
                ModuleConstantValue::RuntimeName(op.name.id_str().as_bytes().to_vec())
            }
            _ => {
                panic!(
                    "unsupported explicit module constant expr after codegen lowering: {expr:?}"
                );
            }
        };
        let id = ModuleConstantId(self.values.len());
        self.values.push(value.clone());
        self.ids.entry(value).or_insert(id);
        id
    }

    fn intern(&mut self, value: ModuleConstantValue) -> ModuleConstantId {
        if let Some(existing) = self.ids.get(&value).copied() {
            return existing;
        }
        let id = ModuleConstantId(self.values.len());
        self.values.push(value.clone());
        self.ids.insert(value, id);
        id
    }

    fn intern_unicode_bytes(&mut self, value: &[u8]) -> ModuleConstantId {
        self.intern(ModuleConstantValue::Unicode(value.to_vec()))
    }

    fn intern_runtime_name_bytes(&mut self, value: &[u8]) -> ModuleConstantId {
        self.intern(ModuleConstantValue::RuntimeName(value.to_vec()))
    }

    fn intern_int(&mut self, value: i64) -> ModuleConstantId {
        self.intern(ModuleConstantValue::Int(value))
    }
}

fn build_unicode_constant<'py>(py: Python<'py>, bytes: &[u8]) -> PyResult<Bound<'py, PyAny>> {
    let mut ptr = unsafe {
        ffi::PyUnicode_DecodeUTF8(
            bytes.as_ptr() as *const i8,
            bytes.len() as ffi::Py_ssize_t,
            c"surrogatepass".as_ptr(),
        )
    };
    if ptr.is_null() {
        return unsafe { Bound::from_owned_ptr_or_err(py, ptr) };
    }
    unsafe {
        ffi::PyUnicode_InternInPlace(&mut ptr);
        Bound::from_owned_ptr_or_err(py, ptr)
    }
}

fn build_runtime_name_constant(
    py: Python<'_>,
    bytes: &[u8],
    mode: RuntimeNameConstantMode,
) -> PyResult<Py<PyAny>> {
    if matches!(mode, RuntimeNameConstantMode::BootstrapSoacRuntime) {
        return build_soac_runtime_bootstrap_runtime_name(py, bytes);
    }
    let name = build_unicode_constant(py, bytes)?;
    let ptr = unsafe { load_runtime_name_owned(name.as_ptr()) };
    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
    Ok(bound.unbind())
}

fn build_soac_runtime_bootstrap_runtime_name(py: Python<'_>, bytes: &[u8]) -> PyResult<Py<PyAny>> {
    let name = std::str::from_utf8(bytes)
        .map_err(|_| PyRuntimeError::new_err("runtime-name constant is not UTF-8"))?;
    if SOAC_RUNTIME_BOOTSTRAP_HELPER_NAMES.contains(&name) {
        return Ok(build_soac_runtime_bootstrap_module(py)?
            .getattr(name)?
            .unbind());
    }
    match name {
        "TRUE" => {
            let ptr = unsafe { ffi::PyBool_FromLong(1) };
            let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
            Ok(bound.unbind())
        }
        "FALSE" => {
            let ptr = unsafe { ffi::PyBool_FromLong(0) };
            let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
            Ok(bound.unbind())
        }
        "NONE" => Ok(py.None().into_any()),
        "ELLIPSIS" => Ok(PyModule::import(py, "builtins")?
            .getattr("Ellipsis")?
            .unbind()),
        "EMPTY_TUPLE" => Ok(PyTuple::empty(py).clone().into_any().unbind()),
        "DELETED" | "ITER_COMPLETE" => Ok(PyModule::import(py, "builtins")?
            .getattr("object")?
            .call0()?
            .unbind()),
        other => Err(PyRuntimeError::new_err(format!(
            "soac.runtime bootstrap cannot build runtime-name constant {other:?}; \
             it should have been lowered as a global name"
        ))),
    }
}

pub fn build_soac_runtime_bootstrap_module<'py>(py: Python<'py>) -> PyResult<Bound<'py, PyModule>> {
    PyModule::from_code(
        py,
        c"
import builtins as _builtins
from soac import _soac_ext
import sys as _sys
import types as _types

DELETED = object()

def _entry_template(*args, **kwargs):
    raise RuntimeError('SOAC runtime bootstrap entry template executed')

_DP_CODE_WITH_FREEVARS_CACHE = {}
_CLIF_ENTRY_RUNTIME_ERROR = 'CLIF entry executed without vectorcall interception'
_PYTHON_KEYWORDS = frozenset((
    'False', 'None', 'True', 'and', 'as', 'assert', 'async', 'await',
    'break', 'class', 'continue', 'def', 'del', 'elif', 'else',
    'except', 'finally', 'for', 'from', 'global', 'if', 'import',
    'in', 'is', 'lambda', 'nonlocal', 'not', 'or', 'pass', 'raise',
    'return', 'try', 'while', 'with', 'yield',
))

def code_with_freevars(names, is_async, is_generator):
    names = tuple(names)
    is_async = bool(is_async)
    is_generator = bool(is_generator)
    cache_key = (names, is_async, is_generator)
    cached = _DP_CODE_WITH_FREEVARS_CACHE.get(cache_key)
    if cached is not None:
        return cached
    for name in names:
        if not isinstance(name, str):
            raise TypeError(f'freevar names must be str, got {type(name)!r}')
        if not name.isidentifier() or name in _PYTHON_KEYWORDS:
            raise ValueError(f'invalid freevar name: {name!r}')
    if len(set(names)) != len(names):
        raise ValueError('freevar names must be unique')

    outer_lines = ['def __dp_make_code():']
    for name in names:
        outer_lines.append(f'    {name} = None')
    if is_async:
        outer_lines.append('    async def wrapped(*args, **kwargs):')
    else:
        outer_lines.append('    def wrapped(*args, **kwargs):')
    if names:
        outer_lines.append('        if False:')
        for name in names:
            outer_lines.append(f'            {name}')
    if is_async and is_generator:
        outer_lines.append('        if False:')
        outer_lines.append('            yield None')
    elif is_generator:
        outer_lines.append('        if False:')
        outer_lines.append('            yield None')
    outer_lines.append(f'        raise RuntimeError({_CLIF_ENTRY_RUNTIME_ERROR!r})')
    outer_lines.append('    return wrapped.__code__')

    ns = {}
    exec('\\n'.join(outer_lines), {}, ns)
    code = ns['__dp_make_code']()
    if code.co_freevars != names:
        code = code.replace(co_freevars=names)
    _DP_CODE_WITH_FREEVARS_CACHE[cache_key] = code
    return code

def tuple_values(*values):
    return tuple(values)

def make_function(function_id, kind, captures, param_defaults, annotate_fn=None, module_globals=None):
    func = _soac_ext.make_bb_function(function_id, captures, param_defaults, annotate_fn, module_globals)
    if kind == 'coroutine':
        from asyncio import coroutines as _coroutines
        func._is_coroutine = _coroutines._is_coroutine
    return func

def create_class(
    name,
    namespace_fn,
    bases,
    kwds,
    requires_class_cell,
    firstlineno=None,
    static_attributes=(),
):
    resolved_bases = _types.resolve_bases(bases)
    meta, ns, meta_kwds = _types.prepare_class(name, resolved_bases, kwds)
    class_cell = ns.get('__classcell__', None)
    if requires_class_cell and class_cell is None:
        class_cell = _types.CellType()
        ns['__classcell__'] = class_cell
    namespace_fn(ns, class_cell)
    if '__firstlineno__' not in ns and firstlineno is not None:
        ns['__firstlineno__'] = firstlineno
    if '__static_attributes__' not in ns:
        ns['__static_attributes__'] = static_attributes
    if resolved_bases is not bases and '__orig_bases__' not in ns:
        ns['__orig_bases__'] = bases
    cls = meta(name, resolved_bases, ns, **meta_kwds)
    if cls is not None:
        ns.pop('__classcell__', None)
        if class_cell is not None:
            if isinstance(class_cell, _types.CellType):
                class_cell.cell_contents = cls
            else:
                raise TypeError('__classcell__ must be a cell')
        _soac_ext.profile_watch_type_key_layout(cls)
    return cls

def import_(name, spec, fromlist=None, level=0):
    if fromlist is None:
        fromlist = []
    globals_dict = {'__spec__': spec}
    if spec is not None:
        package = spec.parent
        if not package and getattr(spec, 'submodule_search_locations', None):
            package = spec.name
        globals_dict['__package__'] = package
        globals_dict['__name__'] = spec.name
    return _builtins.__import__(name, globals_dict, {}, fromlist, level)

def import_attr(module, attr):
    try:
        return getattr(module, attr)
    except AttributeError:
        module_name = getattr(module, '__name__', None)
        if module_name:
            submodule = _sys.modules.get(f'{module_name}.{attr}')
            if submodule is not None:
                return submodule
        raise

def class_lookup_global(class_ns, name, globals_dict):
    try:
        return class_ns[name]
    except KeyError:
        try:
            return globals_dict[name]
        except KeyError:
            try:
                return _builtins.__dict__[name]
            except KeyError as exc:
                raise NameError(f'name {name!r} is not defined') from exc

def class_lookup_cell(class_ns, name, cell):
    try:
        return class_ns[name]
    except KeyError:
        pass
    try:
        value = cell.cell_contents
    except ValueError as exc:
        raise NameError(
            f'cannot access free variable {name!r} where it is not associated with a value in enclosing scope'
        ) from exc
    if value is DELETED:
        raise NameError(
            f'cannot access free variable {name!r} where it is not associated with a value in enclosing scope'
        )
    return value
",
        c"<soac.runtime bootstrap>",
        c"soac.runtime._bootstrap",
    )
}

fn mark_constants_immortal(constants: &[Py<PyAny>]) {
    for obj in constants {
        unsafe {
            _Py_SetImmortal(obj.as_ptr());
            debug_assert_ne!(PyUnstable_IsImmortal(obj.as_ptr()), 0);
        }
    }
}

pub(crate) unsafe fn raise_name_error_for_missing_name(name_obj: *mut ffi::PyObject) {
    let repr = ffi::PyObject_Repr(name_obj);
    if !repr.is_null() {
        let repr_utf8 = ffi::PyUnicode_AsUTF8(repr);
        if !repr_utf8.is_null() {
            let repr_text = std::ffi::CStr::from_ptr(repr_utf8).to_string_lossy();
            let message = format!("name {repr_text} is not defined");
            ffi::Py_DECREF(repr);
            if let Ok(c_message) = CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_NameError, c_message.as_ptr());
                return;
            }
        } else {
            ffi::PyErr_Clear();
        }
        ffi::Py_DECREF(repr);
    } else {
        ffi::PyErr_Clear();
    }
    ffi::PyErr_SetString(ffi::PyExc_NameError, c"name is not defined".as_ptr());
}

pub(crate) unsafe fn load_runtime_name_owned(name_obj: *mut ffi::PyObject) -> *mut ffi::PyObject {
    if name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"invalid runtime name constant".as_ptr(),
        );
        return ptr::null_mut();
    }
    let runtime_module_name = c"soac.runtime".as_ptr();
    let mut runtime_obj = ptr::null_mut();
    let modules = ffi::PyImport_GetModuleDict();
    if !modules.is_null() {
        runtime_obj = ffi::PyDict_GetItemString(modules, runtime_module_name);
        if !runtime_obj.is_null() {
            ffi::Py_INCREF(runtime_obj);
        }
    }
    if runtime_obj.is_null() {
        runtime_obj = ffi::PyImport_ImportModule(runtime_module_name);
    }
    if runtime_obj.is_null() {
        return ptr::null_mut();
    }
    let runtime_value = ffi::PyObject_GetAttr(runtime_obj, name_obj);
    ffi::Py_DECREF(runtime_obj);
    if !runtime_value.is_null() {
        return runtime_value;
    }
    if ffi::PyErr_ExceptionMatches(ffi::PyExc_AttributeError) == 0 {
        return ptr::null_mut();
    }
    ffi::PyErr_Clear();
    let is_builtins_name = {
        let name_utf8 = ffi::PyUnicode_AsUTF8(name_obj);
        !name_utf8.is_null() && CStr::from_ptr(name_utf8).to_bytes() == b"builtins"
    };
    if is_builtins_name {
        return ffi::PyImport_ImportModule(c"builtins".as_ptr());
    }
    let builtins_dict = ffi::PyEval_GetBuiltins();
    if builtins_dict.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"PyEval_GetBuiltins returned null".as_ptr(),
        );
        return ptr::null_mut();
    }
    let builtin_value = ffi::PyObject_GetItem(builtins_dict as *mut ffi::PyObject, name_obj);
    if !builtin_value.is_null() {
        return builtin_value;
    }
    if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0 {
        return ptr::null_mut();
    }
    ffi::PyErr_Clear();
    raise_name_error_for_missing_name(name_obj);
    ptr::null_mut()
}

#[derive(Default)]
struct ModuleConstantCollector {
    constants: ModuleCodegenConstants,
}

impl ModuleConstantCollector {
    fn collect_function(&mut self, function: &BlockPyFunction<CodegenModuleShape>) {
        for (param, default_source) in function.params.iter_with_default_sources() {
            match default_source {
                Some(ParamDefaultSource::Positional(_)) => {
                    self.constants.intern_unicode_bytes(param.name.as_bytes());
                }
                Some(ParamDefaultSource::KeywordOnly(name)) => {
                    self.constants.intern_unicode_bytes(name.as_bytes());
                }
                None => {}
            }
        }
        for block in &function.blocks {
            for stmt in &block.body {
                self.collect_stmt(stmt);
            }
            self.collect_term(&block.term);
        }
    }

    fn collect_stmt(&mut self, stmt: &InstrCodegen) {
        self.collect_expr(stmt);
    }

    fn collect_term(&mut self, term: &BlockTerm<InstrCodegen>) {
        match term {
            BlockTerm::Jump(edge) => self.collect_block_args(&edge.args),
            BlockTerm::IfTerm(if_term) => self.collect_expr(&if_term.test),
            BlockTerm::BranchTable(branch_table) => self.collect_expr(&branch_table.index),
            BlockTerm::Raise(raise_stmt) => {
                if let Some(exc) = &raise_stmt.exc {
                    self.collect_expr(exc);
                }
            }
            BlockTerm::Return(value) => self.collect_expr(value),
        }
    }

    fn collect_block_args(&mut self, args: &[BlockArg]) {
        for arg in args {
            if let BlockArg::AbruptKind(kind) = arg {
                self.constants.intern_int(abrupt_kind_tag(*kind));
            }
        }
    }

    fn collect_expr(&mut self, expr: &InstrCodegen) {
        match expr {
            InstrCodegen::IncrementCounter(_) => {}
            InstrCodegen::CalleeFunctionId(op) => {
                self.collect_expr(op.value.as_ref());
            }
            InstrCodegen::Call(call) => {
                if let Some(const_bytes) = self.string_constant_bytes_for_specialized_codegen(expr)
                {
                    self.constants.intern_unicode_bytes(const_bytes.as_slice());
                }
                if let Some(delete_name_bytes) = self.deleted_name_arg_bytes(call) {
                    self.constants
                        .intern_unicode_bytes(delete_name_bytes.as_slice());
                }
                self.collect_expr(call.func.as_ref());
                for arg in &call.args {
                    self.collect_expr(arg.expr());
                }
                for keyword in &call.keywords {
                    if let CallArgKeyword::Named { arg, .. } = keyword {
                        self.constants.intern_unicode_bytes(arg.as_str().as_bytes());
                    }
                    self.collect_expr(keyword.expr());
                }
            }
            InstrCodegen::CallDirect(call) => {
                self.collect_expr(call.callable.as_ref());
                for arg in &call.args {
                    self.collect_expr(arg.expr());
                }
                for keyword in &call.keywords {
                    if let CallArgKeyword::Named { arg, .. } = keyword {
                        self.constants.intern_unicode_bytes(arg.as_str().as_bytes());
                    }
                    self.collect_expr(keyword.expr());
                }
            }
            InstrCodegen::GetAttr(op) => {
                if let Some(attr_bytes) =
                    self.string_constant_bytes_for_specialized_codegen(op.attr.as_ref())
                {
                    self.constants.intern_unicode_bytes(attr_bytes.as_slice());
                }
                op.visit_children(self);
            }
            InstrCodegen::SetAttr(op) => {
                if let Some(attr_bytes) =
                    self.string_constant_bytes_for_specialized_codegen(op.attr.as_ref())
                {
                    self.constants.intern_unicode_bytes(attr_bytes.as_slice());
                }
                op.visit_children(self);
            }
            InstrCodegen::Load(op)
                if op.name.location.is_global() || op.name.location.is_runtime_name() =>
            {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrCodegen::Load(_) => {}
            InstrCodegen::Store(op) if op.name.location.is_global() => {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
                op.visit_children(self);
            }
            InstrCodegen::Store(op) => {
                op.visit_children(self);
            }
            InstrCodegen::Del(op) if op.name.location.is_global() => {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrCodegen::BinOp(op) => op.visit_children(self),
            InstrCodegen::UnaryOp(op) => {
                op.visit_children(self);
            }
            InstrCodegen::GetItem(op) => {
                op.visit_children(self);
            }
            InstrCodegen::SetItem(op) => {
                op.visit_children(self);
            }
            InstrCodegen::DelItem(op) => {
                op.visit_children(self);
            }
            InstrCodegen::MakeCell(op) => {
                op.visit_children(self);
            }
            InstrCodegen::MakeFunction(op) => {
                op.visit_children(self);
            }
            InstrCodegen::Del(_) | InstrCodegen::CellRef(_) => {}
        }
    }

    fn deleted_name_arg_bytes(
        &self,
        call: &blockpy_intrinsics::Call<InstrCodegen>,
    ) -> Option<Vec<u8>> {
        if helper_name_for_codegen_expr(call.func.as_ref(), &self.constants)
            != Some("load_deleted_name")
            || call.args.len() != 2
        {
            return None;
        }
        self.string_constant_bytes_for_specialized_codegen(call.args[0].expr())
    }

    fn string_constant_bytes_for_specialized_codegen(
        &self,
        expr: &InstrCodegen,
    ) -> Option<Vec<u8>> {
        match expr {
            InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
                self.constants
                    .constant_string_bytes_value(ModuleConstantId(index as usize))
                    .map(ToOwned::to_owned)
            }),
            InstrCodegen::Call(call) => {
                if helper_name_for_codegen_expr(call.func.as_ref(), &self.constants) != Some("str")
                    || call.args.len() != 1
                    || !call.keywords.is_empty()
                {
                    return None;
                }
                self.string_constant_bytes_for_specialized_codegen(call.args[0].expr())
            }
            _ => None,
        }
    }
}

impl soac_blockpy::block_py::Visit<InstrCodegen> for ModuleConstantCollector {
    fn visit_instr(&mut self, expr: &InstrCodegen) {
        self.collect_expr(expr);
    }
}

fn helper_name_for_codegen_expr<'a>(
    expr: &'a InstrCodegen,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a str> {
    match expr {
        InstrCodegen::Load(op)
            if op.name.location.is_global() || op.name.location.is_runtime_name() =>
        {
            Some(op.name.id.as_str())
        }
        InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_runtime_name_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn abrupt_kind_tag(kind: AbruptKind) -> i64 {
    match kind {
        AbruptKind::Fallthrough => 0,
        AbruptKind::Return => 1,
        AbruptKind::Exception => 2,
        AbruptKind::Break => 3,
        AbruptKind::Continue => 4,
    }
}
